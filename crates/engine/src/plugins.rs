//! Sandboxed WASM plug-in host.
//!
//! A plug-in is a `.wasm` file that lives in `<config_dir>/plugins/`. It exports:
//! - `plugin_init()` — called once at load; the plug-in registers its commands here.
//! - `plugin_run(cmd_ptr, cmd_len, params_ptr, params_len) -> i32` — called when one of its
//!   commands is invoked. Returns 0 for success, 1 for error, 2 for "I need a host call" (the
//!   trampoline: the host reads the pending request, runs it, writes the result back, and calls
//!   `plugin_run` again with the original arguments).
//!
//! The plug-in calls back into the host through imported functions that **enqueue** a request
//! rather than executing synchronously (the engine crate forbids unsafe code, so we can't hold
//! a raw pointer to the Session during WASM execution):
//! - `host_execute(cmd_ptr, cmd_len, params_ptr, params_len)` — enqueue an `engine.execute` call.
//! - `host_query(params_ptr, params_len)` — enqueue a `drawing.query` call.
//! - `host_selection()` — enqueue a selection query.
//! - `host_result_len() -> i32` — length of the last host reply.
//! - `host_read_result(dst_ptr, max_len) -> i32` — copy the last host reply into plug-in memory.
//! - `host_set_result(ptr, len)` — set the plug-in's return value.
//! - `host_register_command(...)` — register a command during `plugin_init`.
//! - `host_log(ptr, len)` — write to the engine log.
//!
//! After a host_execute / host_query / host_selection call, the plug-in returns 2 from
//! `plugin_run`. The host processes the request using the Session, writes the result to the
//! host context, and re-enters `plugin_resume(result_code)`.
//!
//! Every call is bounded by a fuel limit (wasmi): a plug-in that loops forever is stopped.
//! File and network access are not available (no WASI).
//!
//! Runtime: wasmi 0.41 (MIT OR Apache-2.0), a pure-Rust WebAssembly interpreter with fuel metering.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

/// Fuel given to each plug-in call (one WASM instruction ≈ 1 fuel unit).
const CALL_FUEL: u64 = 100_000_000;

/// Maximum size of a JSON result from either direction (1 MiB).
const MAX_RESULT: usize = 1 << 20;

/// Maximum number of commands a single plug-in may register.
const MAX_COMMANDS_PER_PLUGIN: usize = 64;

/// Maximum length of a plug-in command id.
const MAX_ID_LEN: usize = 128;

/// Maximum trampoline iterations (host calls per plug-in command invocation).
const MAX_TRAMPOLINES: usize = 256;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A loaded plug-in.
pub struct Plugin {
    pub name: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub commands: Vec<PluginCommand>,
    pub permissions: PluginPermissions,
    module: wasmi::Module,
    engine: wasmi::Engine,
}

/// A command registered by a plug-in.
#[derive(Clone, Debug)]
pub struct PluginCommand {
    pub id: String,
    pub label: String,
    pub params_doc: String,
    pub plugin_idx: usize,
}

#[derive(Clone, Debug, Default)]
pub struct PluginPermissions {
    pub allowed: bool,
}

/// The plug-in host: owned by `Session`, holds every loaded plug-in.
#[derive(Default)]
pub struct PluginHost {
    pub plugins: Vec<Plugin>,
}

// ---------------------------------------------------------------------------
// Host context (no raw pointers — safe)
// ---------------------------------------------------------------------------

/// A pending request from the plug-in to the host.
#[derive(Default)]
struct PendingRequest {
    kind: RequestKind,
    cmd: String,
    params: String,
}

#[derive(Default)]
enum RequestKind {
    #[default]
    None,
    Execute,
    Query,
    Selection,
}

/// The data the host functions read and write during a single WASM call.
#[derive(Default)]
struct HostCtx {
    /// The host's JSON reply to the last host call (the plug-in reads it back).
    result_buf: Vec<u8>,
    /// Commands being registered during `plugin_init`.
    pending_commands: Vec<(String, String, String)>,
    /// Log lines emitted by the plug-in.
    log_lines: Vec<String>,
    /// A pending request for the trampoline.
    pending: PendingRequest,
}

// ---------------------------------------------------------------------------
// PluginHost implementation
// ---------------------------------------------------------------------------

impl PluginHost {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    /// Load every `.wasm` file in the plugins directory. Errors per file are logged, not fatal.
    pub fn load_all(&mut self, log: &mut Vec<String>) {
        let Some(dir) = plugins_dir() else { return; };
        let Ok(entries) = std::fs::read_dir(&dir) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                match self.load_plugin(&path) {
                    Ok(()) => {
                        if let Some(p) = self.plugins.last() {
                            log.push(format!(
                                "Plugin loaded: {} ({} command{})",
                                p.name,
                                p.commands.len(),
                                if p.commands.len() == 1 { "" } else { "s" }
                            ));
                        }
                    }
                    Err(e) => {
                        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                        log.push(format!("Plugin {name} failed to load: {e}"));
                    }
                }
            }
        }
    }

    /// Load a single plug-in from a `.wasm` file.
    pub fn load_plugin(&mut self, path: &Path) -> Result<()> {
        let name = path.file_stem().and_then(|s| s.to_str())
            .ok_or_else(|| EngineError::Other("invalid plugin path".into()))?
            .to_string();
        if self.plugins.iter().any(|p| p.name == name) {
            return Err(EngineError::Other(format!("plugin '{name}' is already loaded")));
        }
        let wasm_bytes = std::fs::read(path)
            .map_err(|e| EngineError::Other(format!("cannot read {}: {e}", path.display())))?;
        if wasm_bytes.len() > 16 * 1024 * 1024 {
            return Err(EngineError::Other("plugin file exceeds 16 MiB".into()));
        }

        let engine = make_engine();
        let module = wasmi::Module::new(&engine, &wasm_bytes[..])
            .map_err(|e| EngineError::Other(format!("invalid WASM module: {e}")))?;

        let plugin_idx = self.plugins.len();
        let commands = run_init(&engine, &module, plugin_idx)?;
        let permissions = load_permissions(&name);

        self.plugins.push(Plugin {
            name, path: path.to_path_buf(), enabled: permissions.allowed,
            commands, permissions, module, engine,
        });
        Ok(())
    }

    /// Find a plug-in command by id.
    pub fn find_command(&self, id: &str) -> Option<&PluginCommand> {
        self.plugins.iter().filter(|p| p.enabled).flat_map(|p| &p.commands).find(|c| c.id == id)
    }

    /// Look up the engine and module for a plug-in command.
    pub fn command_info(&self, cmd_id: &str) -> Result<(wasmi::Engine, wasmi::Module, usize)> {
        let cmd = self.find_command(cmd_id)
            .ok_or_else(|| EngineError::UnknownCommand(cmd_id.into()))?;
        let plugin = self.plugins.get(cmd.plugin_idx)
            .ok_or_else(|| EngineError::Other("plugin index out of range".into()))?;
        if !plugin.enabled {
            return Err(EngineError::Disabled(cmd_id.into(), "plugin is disabled".into()));
        }
        Ok((plugin.engine.clone(), plugin.module.clone(), cmd.plugin_idx))
    }

    /// Enable or disable a plug-in by name.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<()> {
        let p = self.plugins.iter_mut().find(|p| p.name == name)
            .ok_or_else(|| EngineError::Other(format!("no plugin named '{name}'")))?;
        p.enabled = enabled;
        p.permissions.allowed = enabled;
        save_permissions(&p.name, &p.permissions);
        Ok(())
    }

    /// Remove (unload) a plug-in by name.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        let idx = self.plugins.iter().position(|p| p.name == name)
            .ok_or_else(|| EngineError::Other(format!("no plugin named '{name}'")))?;
        let plugin = self.plugins.remove(idx);
        for (i, p) in self.plugins.iter_mut().enumerate() {
            for c in &mut p.commands { c.plugin_idx = i; }
        }
        let _ = std::fs::remove_file(&plugin.path);
        if let Some(dir) = plugins_dir() {
            let _ = std::fs::remove_file(dir.join(format!("{}.permissions.json", plugin.name)));
        }
        Ok(())
    }

    /// List all plug-in commands (enabled plug-ins only).
    pub fn all_commands(&self) -> Vec<&PluginCommand> {
        self.plugins.iter().filter(|p| p.enabled).flat_map(|p| &p.commands).collect()
    }

    /// List info about all loaded plug-ins.
    pub fn list(&self) -> Value {
        json!(self.plugins.iter().map(|p| json!({
            "name": p.name,
            "path": p.path.display().to_string(),
            "enabled": p.enabled,
            "commands": p.commands.iter().map(|c| json!({
                "id": c.id, "label": c.label, "params": c.params_doc,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>())
    }
}

// ---------------------------------------------------------------------------
// WASM engine
// ---------------------------------------------------------------------------

fn make_engine() -> wasmi::Engine {
    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    wasmi::Engine::new(&config)
}

fn read_guest_str(
    memory: &wasmi::Memory,
    store: &impl wasmi::AsContext<Data = HostCtx>,
    ptr: u32, len: u32,
) -> std::result::Result<String, ()> {
    let len = (len as usize).min(MAX_RESULT);
    let mut buf = vec![0u8; len];
    memory.read(store, ptr as usize, &mut buf).map_err(|_| ())?;
    String::from_utf8(buf).map_err(|_| ())
}

// ---------------------------------------------------------------------------
// plugin_init
// ---------------------------------------------------------------------------

fn run_init(engine: &wasmi::Engine, module: &wasmi::Module, plugin_idx: usize) -> Result<Vec<PluginCommand>> {
    let mut store = wasmi::Store::new(engine, HostCtx::default());
    store.set_fuel(CALL_FUEL).map_err(|e| EngineError::Other(format!("fuel: {e}")))?;

    let mut linker = wasmi::Linker::<HostCtx>::new(engine);
    define_host_functions(&mut linker)?;

    let instance = linker.instantiate(&mut store, module)
        .and_then(|i| i.start(&mut store))
        .map_err(|e| EngineError::Other(format!("plugin instantiation failed: {e}")))?;

    let init = instance.get_typed_func::<(), ()>(&store, "plugin_init")
        .map_err(|_| EngineError::Other("plugin has no `plugin_init` export".into()))?;
    init.call(&mut store, ())
        .map_err(|e| EngineError::Other(format!("plugin_init failed: {e}")))?;

    let ctx = store.data();
    if ctx.pending_commands.len() > MAX_COMMANDS_PER_PLUGIN {
        return Err(EngineError::Other(format!(
            "plugin registered {} commands (max {MAX_COMMANDS_PER_PLUGIN})", ctx.pending_commands.len()
        )));
    }
    let commands: Vec<PluginCommand> = ctx.pending_commands.iter().map(|(id, label, params)| PluginCommand {
        id: id.clone(), label: label.clone(), params_doc: params.clone(), plugin_idx,
    }).collect();

    for c in &commands {
        if c.id.is_empty() || c.id.len() > MAX_ID_LEN {
            return Err(EngineError::Other(format!("invalid plugin command id: {:?}", c.id)));
        }
        if !c.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '_') {
            return Err(EngineError::Other(format!(
                "plugin command id {:?} must be lowercase alphanumeric/dot/underscore", c.id
            )));
        }
        if crate::cmd::find_command(&c.id).is_some() {
            return Err(EngineError::Other(format!(
                "plugin command {:?} conflicts with a built-in command", c.id
            )));
        }
    }
    Ok(commands)
}

// ---------------------------------------------------------------------------
// Execute a plugin command (trampoline pattern — no unsafe)
// ---------------------------------------------------------------------------

/// Execute a plug-in command. Uses a trampoline: `plugin_run` returns 2 when it needs a host
/// call; we process the request, write the result, and call `plugin_resume` until it returns 0
/// or 1 (success or error).
pub(crate) fn execute_plugin_command(
    engine: &wasmi::Engine, module: &wasmi::Module,
    cmd_id: &str, params: &Value, session: &mut Session,
) -> Result<Value> {
    let mut store = wasmi::Store::new(engine, HostCtx::default());
    store.set_fuel(CALL_FUEL).map_err(|e| EngineError::Other(format!("fuel: {e}")))?;

    let mut linker = wasmi::Linker::<HostCtx>::new(engine);
    define_host_functions(&mut linker)?;

    let instance = linker.instantiate(&mut store, module)
        .and_then(|i| i.start(&mut store))
        .map_err(|e| EngineError::Other(format!("plugin instantiation failed: {e}")))?;

    let run_fn = instance.get_typed_func::<(i32, i32, i32, i32), i32>(&store, "plugin_run")
        .map_err(|_| EngineError::Other("plugin has no `plugin_run` export".into()))?;

    let memory = instance.get_memory(&store, "memory")
        .ok_or_else(|| EngineError::Other("plugin has no `memory` export".into()))?;

    // Allocate and write command id + params into guest memory.
    let cmd_bytes = cmd_id.as_bytes();
    let params_str = serde_json::to_string(params)
        .map_err(|e| EngineError::Other(format!("JSON serialization: {e}")))?;
    let params_bytes = params_str.as_bytes();

    let (cmd_ptr, params_ptr) = alloc_in_guest(&instance, &mut store, &memory, cmd_bytes, params_bytes)?;

    memory.write(&mut store, cmd_ptr as usize, cmd_bytes)
        .map_err(|_| EngineError::Other("plugin: write cmd out of bounds".into()))?;
    memory.write(&mut store, params_ptr as usize, params_bytes)
        .map_err(|_| EngineError::Other("plugin: write params out of bounds".into()))?;

    // First call.
    let mut rc = run_fn.call(&mut store, (
        cmd_ptr as i32, cmd_bytes.len() as i32,
        params_ptr as i32, params_bytes.len() as i32,
    )).map_err(map_wasm_error)?;

    // Trampoline loop: rc == 2 means "process my host request and call plugin_resume".
    let resume_fn = instance.get_typed_func::<i32, i32>(&store, "plugin_resume").ok();

    for _ in 0..MAX_TRAMPOLINES {
        if rc != 2 { break; }
        let resume = resume_fn.as_ref()
            .ok_or_else(|| EngineError::Other("plugin returned 2 (needs host call) but has no `plugin_resume` export".into()))?;

        // Process the pending request.
        let pending = std::mem::take(&mut store.data_mut().pending);
        let host_rc = match pending.kind {
            RequestKind::Execute => {
                let p: Value = serde_json::from_str(&pending.params).unwrap_or(Value::Null);
                match session.execute(&pending.cmd, &p) {
                    Ok(v) => {
                        store.data_mut().result_buf = serde_json::to_string(&v).unwrap_or_default().into_bytes();
                        0
                    }
                    Err(e) => {
                        store.data_mut().result_buf = json!({"error": e.user_message()}).to_string().into_bytes();
                        1
                    }
                }
            }
            RequestKind::Query => {
                let p: Value = serde_json::from_str(&pending.params).unwrap_or(Value::Null);
                match session.execute("drawing.query", &p) {
                    Ok(v) => {
                        store.data_mut().result_buf = serde_json::to_string(&v).unwrap_or_default().into_bytes();
                        0
                    }
                    Err(e) => {
                        store.data_mut().result_buf = json!({"error": e.user_message()}).to_string().into_bytes();
                        1
                    }
                }
            }
            RequestKind::Selection => {
                let sel = session.selection();
                let handles: Vec<String> = sel.iter().map(|h| h.hex()).collect();
                store.data_mut().result_buf = serde_json::to_string(&handles).unwrap_or_else(|_| "[]".into()).into_bytes();
                0
            }
            RequestKind::None => {
                return Err(EngineError::Other("plugin returned 2 but made no host request".into()));
            }
        };

        rc = resume.call(&mut store, host_rc).map_err(map_wasm_error)?;
    }

    // Collect log lines.
    let log_lines: Vec<String> = std::mem::take(&mut store.data_mut().log_lines);
    for line in log_lines {
        session.log.push(line);
    }

    // Read the result.
    let result_buf = std::mem::take(&mut store.data_mut().result_buf);
    let result_json = String::from_utf8(result_buf)
        .map_err(|_| EngineError::Other("plugin result is not valid UTF-8".into()))?;

    let value: Value = if result_json.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&result_json)
            .map_err(|e| EngineError::Other(format!("plugin returned invalid JSON: {e}")))?
    };

    if rc != 0 {
        let msg = value.as_object().and_then(|o| o.get("error")).and_then(|v| v.as_str())
            .unwrap_or("plugin command failed");
        return Err(EngineError::Other(msg.to_string()));
    }
    Ok(value)
}

fn alloc_in_guest(
    instance: &wasmi::Instance, store: &mut wasmi::Store<HostCtx>,
    memory: &wasmi::Memory, a: &[u8], b: &[u8],
) -> Result<(u32, u32)> {
    if let Ok(alloc) = instance.get_typed_func::<i32, i32>(&mut *store, "alloc") {
        let pa = alloc.call(&mut *store, a.len() as i32)
            .map_err(|e| EngineError::Other(format!("plugin alloc failed: {e}")))?;
        let pb = alloc.call(&mut *store, b.len() as i32)
            .map_err(|e| EngineError::Other(format!("plugin alloc failed: {e}")))?;
        Ok((pa as u32, pb as u32))
    } else {
        let sz = memory.data_size(store);
        let need = a.len() + b.len() + 4096;
        if sz < need {
            return Err(EngineError::Other("plugin memory too small".into()));
        }
        let base = (sz - need) as u32;
        Ok((base, base + a.len() as u32 + 16))
    }
}

fn map_wasm_error(e: wasmi::Error) -> EngineError {
    let msg = e.to_string();
    if msg.contains("fuel") || msg.contains("Fuel") {
        EngineError::Other(format!("plugin ran out of fuel (infinite loop?): {msg}"))
    } else {
        EngineError::Other(format!("plugin_run failed: {msg}"))
    }
}

// ---------------------------------------------------------------------------
// Host functions (all safe — no raw pointers)
// ---------------------------------------------------------------------------

fn define_host_functions(linker: &mut wasmi::Linker<HostCtx>) -> Result<()> {
    let wrap_err = |e: wasmi::errors::LinkerError| EngineError::Other(format!("linker: {e}"));

    // host_register_command(id_ptr, id_len, label_ptr, label_len, params_ptr, params_len)
    linker.func_wrap("env", "host_register_command",
        |mut caller: wasmi::Caller<'_, HostCtx>,
         id_ptr: i32, id_len: i32, label_ptr: i32, label_len: i32, params_ptr: i32, params_len: i32| {
            let memory = caller.get_export("memory").and_then(|e| e.into_memory());
            let Some(memory) = memory else { return; };
            let id = read_guest_str(&memory, &caller, id_ptr as u32, id_len as u32);
            let label = read_guest_str(&memory, &caller, label_ptr as u32, label_len as u32);
            let params = read_guest_str(&memory, &caller, params_ptr as u32, params_len as u32);
            if let (Ok(id), Ok(label), Ok(params)) = (id, label, params)
                && caller.data().pending_commands.len() < MAX_COMMANDS_PER_PLUGIN {
                caller.data_mut().pending_commands.push((id, label, params));
            }
        },
    ).map_err(wrap_err)?;

    // host_execute(cmd_ptr, cmd_len, params_ptr, params_len) — enqueue, don't run.
    linker.func_wrap("env", "host_execute",
        |mut caller: wasmi::Caller<'_, HostCtx>,
         cmd_ptr: i32, cmd_len: i32, params_ptr: i32, params_len: i32| {
            let memory = caller.get_export("memory").and_then(|e| e.into_memory());
            let Some(memory) = memory else { return; };
            let cmd = read_guest_str(&memory, &caller, cmd_ptr as u32, cmd_len as u32).unwrap_or_default();
            let params = read_guest_str(&memory, &caller, params_ptr as u32, params_len as u32).unwrap_or_default();
            caller.data_mut().pending = PendingRequest { kind: RequestKind::Execute, cmd, params };
        },
    ).map_err(wrap_err)?;

    // host_query(params_ptr, params_len)
    linker.func_wrap("env", "host_query",
        |mut caller: wasmi::Caller<'_, HostCtx>, params_ptr: i32, params_len: i32| {
            let memory = caller.get_export("memory").and_then(|e| e.into_memory());
            let Some(memory) = memory else { return; };
            let params = read_guest_str(&memory, &caller, params_ptr as u32, params_len as u32).unwrap_or_default();
            caller.data_mut().pending = PendingRequest { kind: RequestKind::Query, cmd: String::new(), params };
        },
    ).map_err(wrap_err)?;

    // host_selection()
    linker.func_wrap("env", "host_selection",
        |mut caller: wasmi::Caller<'_, HostCtx>| {
            caller.data_mut().pending = PendingRequest { kind: RequestKind::Selection, ..Default::default() };
        },
    ).map_err(wrap_err)?;

    // host_result_len() -> i32
    linker.func_wrap("env", "host_result_len",
        |caller: wasmi::Caller<'_, HostCtx>| -> i32 {
            caller.data().result_buf.len().min(i32::MAX as usize) as i32
        },
    ).map_err(wrap_err)?;

    // host_read_result(dst_ptr, max_len) -> i32
    linker.func_wrap("env", "host_read_result",
        |mut caller: wasmi::Caller<'_, HostCtx>, dst_ptr: i32, max_len: i32| -> i32 {
            let memory = caller.get_export("memory").and_then(|e| e.into_memory());
            let Some(memory) = memory else { return 0; };
            let buf = caller.data().result_buf.clone();
            let len = buf.len().min(max_len as usize).min(MAX_RESULT);
            if memory.write(&mut caller, dst_ptr as usize, &buf[..len]).is_err() { return 0; }
            len as i32
        },
    ).map_err(wrap_err)?;

    // host_set_result(ptr, len)
    linker.func_wrap("env", "host_set_result",
        |mut caller: wasmi::Caller<'_, HostCtx>, ptr: i32, len: i32| {
            let memory = caller.get_export("memory").and_then(|e| e.into_memory());
            let Some(memory) = memory else { return; };
            if let Ok(s) = read_guest_str(&memory, &caller, ptr as u32, len as u32) {
                caller.data_mut().result_buf = s.into_bytes();
            }
        },
    ).map_err(wrap_err)?;

    // host_result_ptr() -> i32 (compat stub)
    linker.func_wrap("env", "host_result_ptr",
        |_: wasmi::Caller<'_, HostCtx>| -> i32 { 0 },
    ).map_err(wrap_err)?;

    // host_log(ptr, len)
    linker.func_wrap("env", "host_log",
        |mut caller: wasmi::Caller<'_, HostCtx>, ptr: i32, len: i32| {
            let memory = caller.get_export("memory").and_then(|e| e.into_memory());
            let Some(memory) = memory else { return; };
            if let Ok(s) = read_guest_str(&memory, &caller, ptr as u32, len as u32)
                && caller.data().log_lines.len() < 1000 {
                caller.data_mut().log_lines.push(s);
            }
        },
    ).map_err(wrap_err)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Plug-in directory and permissions
// ---------------------------------------------------------------------------

pub fn plugins_dir() -> Option<PathBuf> {
    crate::control_auth::config_dir().map(|d| d.join("plugins"))
}

fn load_permissions(name: &str) -> PluginPermissions {
    let Some(dir) = plugins_dir() else { return PluginPermissions { allowed: true }; };
    let path = dir.join(format!("{name}.permissions.json"));
    let Ok(text) = std::fs::read_to_string(&path) else { return PluginPermissions { allowed: true }; };
    let Ok(v): std::result::Result<Value, _> = serde_json::from_str(&text) else { return PluginPermissions { allowed: true }; };
    PluginPermissions { allowed: v.get("allowed").and_then(|v| v.as_bool()).unwrap_or(true) }
}

fn save_permissions(name: &str, perms: &PluginPermissions) {
    let Some(dir) = plugins_dir() else { return; };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.permissions.json"));
    let json = json!({ "allowed": perms.allowed });
    let _ = std::fs::write(path, serde_json::to_string_pretty(&json).unwrap_or_default());
}
