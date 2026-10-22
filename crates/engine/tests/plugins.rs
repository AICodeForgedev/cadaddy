//! The sandboxed WASM plug-ins (Q157): loading, commands, the sandbox (grants for files and the
//! network, no WASI), the caps (fuel, memory, nesting), loading at session start, and agents'
//! plug-ins waiting for the user.
//!
//! Hermetic: every test keeps its plug-ins and their permission files in its own temporary
//! folder (`PluginHost::in_dir`), never the user's settings folder; the start-up test runs in a
//! child process with its own `CADADDY_CONFIG_DIR`.

#![cfg(not(target_arch = "wasm32"))]

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use cadaddy_engine::plugins::PluginHost;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The prebuilt example plug-in (examples/plugins/hello-plugin).
fn hello_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/hello-plugin/prebuilt/hello_plugin.wasm")
}

/// A fresh folder for one test.
fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-plugins-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session whose plug-ins live in `d`.
fn session(d: &Path) -> Session {
    let mut s = Session::new();
    s.plugins = PluginHost::in_dir(d);
    s
}

/// Text for a WAT string.
fn wat_str(s: &str) -> String {
    s.chars().map(|c| if c == '"' || c == '\\' { format!("\\{c}") } else { c.to_string() }).collect()
}

/// A plug-in registering command `id` whose run is one host call: `host_execute(target,
/// params)`; it answers with the host's reply (an error's message too).
fn caller_wasm(id: &str, target: &str, params: &Value) -> Vec<u8> {
    let params = params.to_string();
    wat::parse_str(format!(
        r#"(module
            (import "env" "host_register_command" (func $reg (param i32 i32 i32 i32 i32 i32)))
            (import "env" "host_execute" (func $exec (param i32 i32 i32 i32)))
            (import "env" "host_read_result" (func $read (param i32 i32) (result i32)))
            (import "env" "host_set_result" (func $set (param i32 i32)))
            (memory (export "memory") 2)
            (global $heap (mut i32) (i32.const 65536))
            (data (i32.const 0) "{id}")
            (data (i32.const 1024) "{target}")
            (data (i32.const 2048) "{params}")
            (func (export "alloc") (param $n i32) (result i32) (local $p i32)
                (local.set $p (global.get $heap))
                (global.set $heap (i32.add (global.get $heap) (local.get $n)))
                (local.get $p))
            (func (export "plugin_init") (call $reg (i32.const 0) (i32.const {idl}) (i32.const 0) (i32.const {idl}) (i32.const 0) (i32.const 0)))
            (func (export "plugin_run") (param i32 i32 i32 i32) (result i32)
                (call $exec (i32.const 1024) (i32.const {tl}) (i32.const 2048) (i32.const {pl}))
                (i32.const 2))
            (func (export "plugin_resume") (param $rc i32) (result i32) (local $n i32)
                (local.set $n (call $read (i32.const 8192) (i32.const 32768)))
                (call $set (i32.const 8192) (local.get $n))
                (local.get $rc))
        )"#,
        id = wat_str(id),
        idl = id.len(),
        target = wat_str(target),
        tl = target.len(),
        params = wat_str(&params),
        pl = params.len(),
    ))
    .unwrap()
}

/// A plug-in registering command `id` whose run is `body` (a WAT instruction sequence leaving an
/// i32).
fn body_wasm(id: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            (import "env" "host_register_command" (func $reg (param i32 i32 i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{id}")
            (func (export "plugin_init") (call $reg (i32.const 0) (i32.const {l}) (i32.const 0) (i32.const {l}) (i32.const 0) (i32.const 0)))
            (func (export "plugin_run") (param i32 i32 i32 i32) (result i32) {body})
        )"#,
        id = wat_str(id),
        l = id.len()
    ))
    .unwrap()
}

/// Write `bytes` as `<d>/<name>.wasm`.
fn wasm_file(d: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = d.join(format!("{name}.wasm"));
    std::fs::write(&p, bytes).unwrap();
    p
}

fn file_formats() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

// ---------------- loading and the example ----------------

#[test]
fn load_hello_plugin() {
    let d = dir("load");
    let mut host = PluginHost::in_dir(&d);
    host.load_plugin(&hello_wasm()).expect("load_plugin should succeed");
    assert_eq!(host.plugins.len(), 1);
    assert_eq!(host.plugins[0].name, "hello_plugin");
    let ids: Vec<&str> = host.plugins[0].commands.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["hello.greet", "hello.count"]);
    assert!(host.load_plugin(&hello_wasm()).is_err(), "a second load of the same name is refused");
    assert!(host.find_command("nonexistent").is_none());
}

#[test]
fn enable_disable_is_kept_in_the_plugins_own_folder() {
    let d = dir("enable");
    let mut host = PluginHost::in_dir(&d);
    host.load_plugin(&hello_wasm()).unwrap();
    host.set_enabled("hello_plugin", false).unwrap();
    assert!(host.find_command("hello.greet").is_none(), "a disabled plug-in's commands are hidden");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(d.join("hello_plugin.permissions.json")).unwrap()).unwrap();
    assert_eq!(saved, json!({"allowed": false, "files": false, "network": false}));
    // Loaded again (as at the next start): still disabled.
    let mut again = PluginHost::in_dir(&d);
    again.load_plugin(&hello_wasm()).unwrap();
    assert!(!again.plugins[0].enabled);
    again.set_enabled("hello_plugin", true).unwrap();
    assert!(again.find_command("hello.greet").is_some());
    let list = again.list();
    assert_eq!(list[0]["enabled"], true);
    assert_eq!(list[0]["grants"], json!({"files": false, "network": false}));
}

#[test]
fn plugins_command_lists() {
    let d = dir("list");
    let mut s = session(&d);
    assert_eq!(s.execute("plugins", &json!({"action": "list"})).unwrap(), json!([]));
    s.plugins.load_plugin(&hello_wasm()).unwrap();
    let cmds = s.execute("plugins", &json!({"action": "commands"})).unwrap();
    let ids: Vec<&str> = cmds.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["hello.greet", "hello.count"]);
}

#[test]
fn execute_hello_greet() {
    let d = dir("greet");
    let mut s = session(&d);
    s.plugins.load_plugin(&hello_wasm()).unwrap();
    let before = s.doc().unwrap().entity_count();
    s.execute("hello.greet", &json!({})).unwrap();
    let doc = s.doc().unwrap();
    assert_eq!(doc.entity_count(), before + 1, "exactly one new object");
    let e = doc.model.iter().last().unwrap();
    let EntityKind::Text(t) = &e.kind else { panic!("a TEXT: {:?}", e.kind) };
    assert_eq!(t.value, "Hello from a plug-in!");
    assert!((t.insert.x - 5.0).abs() < 1e-9 && (t.insert.y - 5.0).abs() < 1e-9, "at (5, 5): {:?}", t.insert);
}

#[test]
fn execute_hello_count() {
    let d = dir("count");
    let mut s = session(&d);
    s.plugins.load_plugin(&hello_wasm()).unwrap();
    for x in [0, 10, 20] {
        s.execute("line", &json!({"points": [[x, 0], [x, 5]]})).unwrap();
    }
    s.execute("circle", &json!({"center": [50, 50], "radius": 1})).unwrap();
    let handles: Vec<String> = s.doc().unwrap().model.iter().take(3).map(|e| e.handle.hex()).collect();
    s.execute("select", &json!({"handles": handles})).unwrap();
    let v = s.execute("hello.count", &json!({})).unwrap();
    assert_eq!(v["count"], json!(3), "{v}");
}

#[test]
fn module_without_exports_is_refused() {
    let d = dir("noexports");
    let p = wasm_file(&d, "empty", &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
    let e = PluginHost::in_dir(&d).load_plugin(&p).unwrap_err().to_string();
    assert!(e.contains("plugin_init"), "{e}");
}

// ---------------- the sandbox ----------------

/// A plug-in may not write a file through a command without the user's `files` grant: the
/// refusal names the grant; with it, the command runs. Removing another plug-in (PLUGINS, which
/// deletes its file) is refused the same way.
#[test]
fn a_plugin_needs_the_files_grant_to_write_files() {
    file_formats();
    let d = dir("files");
    let out = d.join("out.dxf");
    let mut s = session(&d);
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let writer = wasm_file(&d, "writer", &caller_wasm("writer.save", "wblock", &json!({"path": out.display().to_string()})));
    s.plugins.load_plugin(&writer).unwrap();
    let e = s.execute("writer.save", &json!({})).unwrap_err().to_string();
    assert!(
        e.contains("The plug-in writer may not run wblock: it reads or writes files")
            && e.contains(r#"PLUGINS {"action": "grant", "name": "writer", "permission": "files"}"#),
        "{e}"
    );
    assert!(!out.exists(), "nothing written");
    // Granted by the user: it runs.
    s.execute("plugins", &json!({"action": "grant", "name": "writer", "permission": "files"})).unwrap();
    s.execute("writer.save", &json!({})).unwrap();
    assert!(out.exists(), "written once granted");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(d.join("writer.permissions.json")).unwrap()).unwrap();
    assert_eq!(saved["files"], true);
    // Taken back: refused again.
    std::fs::remove_file(&out).unwrap();
    s.execute("plugins", &json!({"action": "revoke", "name": "writer", "permission": "files"})).unwrap();
    assert!(s.execute("writer.save", &json!({})).is_err());
    assert!(!out.exists());
    // PLUGINS remove (it deletes the other plug-in's file): refused without the grant.
    let hello = d.join("hello_plugin.wasm");
    std::fs::copy(hello_wasm(), &hello).unwrap();
    s.plugins.load_plugin(&hello).unwrap();
    let remover = wasm_file(&d, "remover", &caller_wasm("remover.go", "plugins", &json!({"action": "remove", "name": "hello_plugin"})));
    s.plugins.load_plugin(&remover).unwrap();
    let e = s.execute("remover.go", &json!({})).unwrap_err().to_string();
    assert!(e.contains("may not run plugins") && e.contains(r#""permission": "files""#), "{e}");
    assert!(hello.exists() && s.plugins.find_command("hello.greet").is_some(), "the other plug-in is still there");
    // A drawing command needs no grant.
    let drawer = wasm_file(&d, "drawer", &caller_wasm("drawer.go", "circle", &json!({"center": [3, 3], "radius": 1})));
    s.plugins.load_plugin(&drawer).unwrap();
    s.execute("drawer.go", &json!({})).unwrap();
}

/// What reaches files or the network is the commands' own classification (CommandSpec::reach),
/// not a list in the plug-in host.
#[test]
fn commands_say_what_they_reach() {
    use cadaddy_engine::cmd::{Access, find_command};
    for id in ["wblock", "saveas", "qsave", "open", "export", "plugins", "script", "rhai", "lisp", "appload", "convert", "linetype.define"] {
        assert_eq!(find_command(id).unwrap_or_else(|| panic!("{id}")).reach(), Access::Files, "{id}");
    }
    assert_eq!(find_command("hyperlink.open").unwrap().reach(), Access::Network);
    for id in ["line", "circle", "text", "move", "select", "drawing.query", "layer"] {
        assert_eq!(find_command(id).unwrap().reach(), Access::Drawing, "{id}");
    }
}

/// A module that imports anything but CADaddy's host functions (WASI's fd_write here) doesn't
/// load, and says why.
#[test]
fn a_module_importing_wasi_is_refused() {
    let d = dir("wasi");
    let bytes = wat::parse_str(
        r#"(module
            (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "plugin_init")))"#,
    )
    .unwrap();
    let p = wasm_file(&d, "wasi", &bytes);
    let e = PluginHost::in_dir(&d).load_plugin(&p).unwrap_err().to_string();
    assert!(e.contains("imports `wasi_snapshot_preview1.fd_write`") && e.contains("no WASI"), "{e}");
}

// ---------------- the caps ----------------

#[test]
fn fuel_limit_stops_infinite_loops() {
    let d = dir("fuel");
    // In plugin_init: it doesn't load.
    let init = wat::parse_str(r#"(module (func (export "plugin_init") (loop (br 0))) (memory (export "memory") 1))"#).unwrap();
    let e = PluginHost::in_dir(&d).load_plugin(&wasm_file(&d, "looper", &init)).unwrap_err().to_string();
    assert!(e.contains("plugin ran out of fuel"), "{e}");
    // In a command: that call fails, the session goes on.
    let mut s = session(&d);
    s.plugins.load_plugin(&wasm_file(&d, "spinner", &body_wasm("spin.forever", "(loop (br 0)) (i32.const 0)"))).unwrap();
    let e = s.execute("spin.forever", &json!({})).unwrap_err().to_string();
    assert!(e.contains("plugin ran out of fuel"), "{e}");
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
}

/// Memory past the cap: a clean error, the process goes on.
#[test]
fn memory_past_the_cap_is_a_clean_error() {
    let d = dir("memory");
    let mut s = session(&d);
    // 1600 pages of 64 KiB: 100 MiB, past the 64 MiB cap.
    s.plugins.load_plugin(&wasm_file(&d, "grower", &body_wasm("grow.big", "(drop (memory.grow (i32.const 1600))) (i32.const 0)"))).unwrap();
    let e = s.execute("grow.big", &json!({})).unwrap_err().to_string();
    assert!(e.contains("past its memory (64 MiB)"), "{e}");
    // Within the cap it's fine.
    s.plugins.load_plugin(&wasm_file(&d, "small", &body_wasm("grow.small", "(drop (memory.grow (i32.const 16))) (i32.const 0)"))).unwrap();
    s.execute("grow.small", &json!({})).unwrap();
}

/// A plug-in command that calls itself through host_execute stops at the nesting cap with an
/// error; the session goes on.
#[test]
fn a_plugin_calling_itself_is_stopped() {
    let d = dir("nest");
    let mut s = session(&d);
    s.plugins.load_plugin(&wasm_file(&d, "echo", &caller_wasm("loop.me", "loop.me", &json!({})))).unwrap();
    let e = s.execute("loop.me", &json!({})).unwrap_err().to_string();
    assert!(e.contains("Plug-in commands nest more than 4 deep"), "{e}");
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    assert_eq!(s.doc().unwrap().entity_count(), 1);
}

// ---------------- start-up and agents ----------------

/// The plug-ins installed in the settings folder are there in a new session as the apps start
/// it (Session::for_app: desktop app, MCP, CLI), without PLUGINS reload. Run in a child process
/// with its own CADADDY_CONFIG_DIR, so no other test sees it.
#[test]
fn installed_plugins_load_when_a_session_starts() {
    if std::env::var_os("CADADDY_PLUGIN_START_CHILD").is_some() {
        let s = Session::for_app();
        assert!(s.plugins.find_command("hello.greet").is_some(), "{:?}", s.log);
        return;
    }
    let config = dir("start");
    std::fs::create_dir_all(config.join("plugins")).unwrap();
    std::fs::copy(hello_wasm(), config.join("plugins/hello_plugin.wasm")).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["installed_plugins_load_when_a_session_starts", "--exact", "--nocapture"])
        .env("CADADDY_CONFIG_DIR", &config)
        .env("CADADDY_PLUGIN_START_CHILD", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.contains("1 passed"), "{text}{}", String::from_utf8_lossy(&out.stderr));
    // The user's settings folder wasn't touched: only this test's.
    assert!(config.join("plugins/hello_plugin.wasm").exists());
}

/// A plug-in an agent loads waits, disabled, for a person to enable it (an agent can't); one
/// the user loads runs at once.
#[test]
fn a_plugin_an_agent_loads_starts_disabled() {
    let d = dir("agent");
    let mut s = session(&d);
    let r = s.execute_agent("plugins", &json!({"action": "load", "path": hello_wasm().display().to_string()})).unwrap();
    assert_eq!(r["enabled"], false, "{r}");
    assert!(s.execute("hello.greet", &json!({})).is_err(), "not runnable yet");
    let e = s.execute_agent("plugins", &json!({"action": "enable", "name": "hello_plugin"})).unwrap_err().to_string();
    assert!(e.contains("only a person can enable a plug-in"), "{e}");
    let e = s.execute_agent("plugins", &json!({"action": "grant", "name": "hello_plugin", "permission": "files"})).unwrap_err().to_string();
    assert!(e.contains("only a person can grant"), "{e}");
    assert!(s.execute("hello.greet", &json!({})).is_err(), "still disabled");
    // The user enables it: it runs.
    s.execute("plugins", &json!({"action": "enable", "name": "hello_plugin"})).unwrap();
    s.execute("hello.greet", &json!({})).unwrap();
    // The user's own load: enabled at once.
    let d2 = dir("user");
    let mut u = session(&d2);
    let r = u.execute("plugins", &json!({"action": "load", "path": hello_wasm().display().to_string()})).unwrap();
    assert_eq!(r["enabled"], true, "{r}");
    u.execute("hello.greet", &json!({})).unwrap();
}

// ---- MUST 1 (Q5036): an agent can't replace an installed plug-in ----

/// The user installs and trusts hello_plugin. An agent loads a different .wasm under the same
/// file name: refused. The installed file is unchanged, and after the agent's PLUGINS reload
/// hello.greet still produces the original TEXT.
#[test]
fn agent_cannot_overwrite_installed_plugin() {
    let d = dir("overwrite");
    let mut s = session(&d);

    // User loads hello_plugin, enables it and grants files.
    s.execute("plugins", &json!({"action": "load", "path": hello_wasm().display().to_string()})).unwrap();
    s.execute("plugins", &json!({"action": "enable", "name": "hello_plugin"})).unwrap();
    s.execute("plugins", &json!({"action": "grant", "name": "hello_plugin", "permission": "files"})).unwrap();

    // Record the installed file's bytes.
    let installed = d.join("hello_plugin.wasm");
    let original_bytes = std::fs::read(&installed).unwrap();

    // An impostor: a different wasm named hello_plugin.wasm (it registers a different command
    // and draws nothing, so if it ever ran as hello_plugin the greet test would fail).
    let impostor_dir = dir("impostor");
    let impostor = wasm_file(&impostor_dir, "hello_plugin", &body_wasm("evil.noop", "(i32.const 0)"));

    // Agent tries to load it: refused.
    let e = s.execute_agent("plugins", &json!({"action": "load", "path": impostor.display().to_string()})).unwrap_err().to_string();
    assert!(e.contains("already installed") && e.contains("agent may not replace"), "{e}");

    // The installed file is byte-for-byte the original.
    assert_eq!(std::fs::read(&installed).unwrap(), original_bytes, "installed wasm unchanged");

    // After the agent's PLUGINS reload, hello.greet still produces the original TEXT.
    s.execute_agent("plugins", &json!({"action": "reload"})).unwrap();
    let before = s.doc().unwrap().entity_count();
    // Re-enable after reload (reload creates a fresh host, reads the saved permissions).
    s.execute("plugins", &json!({"action": "enable", "name": "hello_plugin"})).unwrap();
    s.execute("hello.greet", &json!({})).unwrap();
    let doc = s.doc().unwrap();
    assert_eq!(doc.entity_count(), before + 1);
    let e = doc.model.iter().last().unwrap();
    let cadaddy_engine::doc::EntityKind::Text(t) = &e.kind else { panic!("expected TEXT") };
    assert_eq!(t.value, "Hello from a plug-in!");
}

/// An agent loads a plug-in whose stale permissions file says files:true, network:true. The
/// load comes in disabled with both grants false.
#[test]
fn agent_load_clears_stale_grants() {
    let d = dir("stale");
    let mut s = session(&d);

    // Write a stale permissions file granting everything.
    std::fs::create_dir_all(&d).unwrap();
    let stale = json!({"allowed": true, "files": true, "network": true});
    std::fs::write(d.join("hello_plugin.permissions.json"), serde_json::to_string_pretty(&stale).unwrap()).unwrap();

    // Agent loads hello_plugin (it's a new name, no file in plugins dir yet).
    let r = s.execute_agent("plugins", &json!({"action": "load", "path": hello_wasm().display().to_string()})).unwrap();
    assert_eq!(r["enabled"], false, "disabled: {r}");

    // The saved JSON has all grants false.
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(d.join("hello_plugin.permissions.json")).unwrap()).unwrap();
    assert_eq!(saved, json!({"allowed": false, "files": false, "network": false}), "grants cleared: {saved}");
}

// ---- MUST 2 (Q5036): a plug-in can't change plug-in trust ----

/// A files-granted WAT plug-in (run by the user) calls PLUGINS grant to give itself the
/// network, and PLUGINS enable on an agent-loaded plug-in: both are refused, and the saved
/// permission files are unchanged.
#[test]
fn a_plugin_cannot_change_plugin_trust() {
    file_formats();
    let d = dir("trust");
    let mut s = session(&d);

    // The user loads an agent's plug-in (starts disabled).
    let agent_plugin = wasm_file(&d, "agent_loaded", &body_wasm("agent.noop", "(i32.const 0)"));
    s.execute_agent("plugins", &json!({"action": "load", "path": agent_plugin.display().to_string()})).unwrap();
    let agent_perms_before: Value = serde_json::from_str(&std::fs::read_to_string(d.join("agent_loaded.permissions.json")).unwrap()).unwrap();

    // A plug-in with the files grant that tries to escalate:
    // 1) grant itself network  2) enable the agent-loaded plug-in.
    let granter = wasm_file(
        &d,
        "granter",
        &caller_wasm("granter.self_grant", "plugins", &json!({"action": "grant", "name": "granter", "permission": "network"})),
    );
    s.plugins.load_plugin(&granter).unwrap();
    s.execute("plugins", &json!({"action": "grant", "name": "granter", "permission": "files"})).unwrap();

    // Save the granter's permissions before the attempt.
    let granter_perms_before: Value = serde_json::from_str(&std::fs::read_to_string(d.join("granter.permissions.json")).unwrap()).unwrap();

    // Attempt 1: self-grant network — refused.
    let e = s.execute("granter.self_grant", &json!({})).unwrap_err().to_string();
    let e_lc = e.to_ascii_lowercase();
    assert!(e_lc.contains("only a person can") && e_lc.contains("plug-in may not change plug-in trust"), "self-grant refused: {e}");

    // Attempt 2: enable the agent-loaded plug-in — need a second caller plug-in for this.
    let enabler = wasm_file(&d, "enabler", &caller_wasm("enabler.enable_agent", "plugins", &json!({"action": "enable", "name": "agent_loaded"})));
    s.plugins.load_plugin(&enabler).unwrap();
    s.execute("plugins", &json!({"action": "grant", "name": "enabler", "permission": "files"})).unwrap();
    let e = s.execute("enabler.enable_agent", &json!({})).unwrap_err().to_string();
    let e_lc = e.to_ascii_lowercase();
    assert!(e_lc.contains("only a person can") && e_lc.contains("plug-in may not change plug-in trust"), "enable refused: {e}");

    // Both permission files are unchanged.
    let granter_perms_after: Value = serde_json::from_str(&std::fs::read_to_string(d.join("granter.permissions.json")).unwrap()).unwrap();
    let agent_perms_after: Value = serde_json::from_str(&std::fs::read_to_string(d.join("agent_loaded.permissions.json")).unwrap()).unwrap();
    assert_eq!(granter_perms_before, granter_perms_after, "granter permissions unchanged");
    assert_eq!(agent_perms_before, agent_perms_after, "agent_loaded permissions unchanged");
}
