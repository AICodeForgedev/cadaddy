//! The PLUGINS command: list, load, enable, disable and remove WASM plug-ins, and give them the
//! grants (files, network) the commands they run need. Enabling and granting are the user's: an
//! agent's call is refused, and a plug-in an agent loads starts disabled.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_param};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("plugins", "Plug-ins", run)
            .files()
            .menu(&["Tools", "Plug-ins"])
            .alias(&["plugin", "appload.wasm"])
            .params("{action?: \"list\" (default) | \"load\" | \"enable\" | \"disable\" | \"grant\" | \"revoke\" | \"remove\" (deletes its file) | \"commands\" | \"reload\", name?: string, path?: string, permission?: \"files\" | \"network\"} → list: [{name, path, enabled, grants: {files, network}, commands: [{id, label, params}]}]; load: {name, commands, enabled}; commands: [{id, label, plugin}]. An agent's load starts disabled; enable and grant are the user's")
            .enabled(always)
            .noundo(),
    ]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").unwrap_or("list");
    match action {
        "list" => run_list(s),
        "load" => {
            not_from_plugin(s, "load a plug-in")?;
            run_load(s, p)
        }
        "enable" => {
            users_only(s, "enable a plug-in")?;
            run_enable(s, p, true)
        }
        "disable" => run_enable(s, p, false),
        "grant" => {
            users_only(s, "grant a plug-in files or the network")?;
            run_grant(s, p, true)
        }
        "revoke" => run_grant(s, p, false),
        "remove" => run_remove(s, p),
        "commands" => run_commands(s),
        "reload" => {
            not_from_plugin(s, "reload plug-ins")?;
            run_reload(s)
        }
        _ => Err(bad("plugins", format!("unknown action: {action}"))),
    }
}

/// Enable and grant run code with the user's trust: never on an agent's say-so and never from
/// inside a plug-in's host call. MUST 2 (Q5036): a plug-in that has `files` can't escalate
/// to `network` or enable an agent-loaded plug-in.
fn users_only(s: &Session, what: &str) -> Result<()> {
    if s.agent.from_agent {
        return Err(bad("plugins", format!("only a person can {what}: ask the user to run PLUGINS in CADaddy")));
    }
    not_from_plugin(s, what)?;
    Ok(())
}

/// Trust-changing actions (enable, grant, load, reload) may not run from inside a plug-in's
/// host call (plugin_depth > 0), regardless of that plug-in's grants.
fn not_from_plugin(s: &Session, what: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if s.plugin_depth > 0 {
        return Err(bad("plugins", format!("only a person can {what}: a plug-in may not change plug-in trust")));
    }
    let _ = s;
    Ok(())
}

fn run_grant(s: &mut Session, p: &Value, on: bool) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let name = str_param(p, "name").ok_or_else(|| bad("plugins", "`name` is required"))?;
        let permission = str_param(p, "permission").ok_or_else(|| bad("plugins", "`permission` is files or network"))?;
        s.plugins.set_grant(name, permission, on).map_err(|e| bad("plugins", e.user_message()))?;
        s.echo(format!("Plug-in {name}: {permission} {}.", if on { "granted" } else { "taken back" }));
        Ok(json!({ "name": name, "permission": permission, "granted": on }))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p, on);
        Err(crate::EngineError::Other("plug-ins are not available on the web".into()))
    }
}

fn run_list(s: &Session) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(s.plugins.list())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Ok(json!([]))
    }
}

fn run_load(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = str_param(p, "path").ok_or_else(|| bad("plugins", "`path` to a .wasm file is required"))?;
        let path = std::path::Path::new(path);

        // Validate: must be a .wasm file, must exist.
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            return Err(bad("plugins", "path must point to a .wasm file"));
        }

        // Copy to the plugins directory so it persists.
        let plugins_dir = s.plugins.dir().ok_or_else(|| crate::EngineError::Other("cannot determine plugins directory".into()))?;
        let _ = std::fs::create_dir_all(&plugins_dir);
        let file_name = path.file_name().ok_or_else(|| bad("plugins", "invalid file name"))?;
        let dest = plugins_dir.join(file_name);

        // The name that load_plugin_as will derive (file stem).
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let by_agent = s.agent.from_agent;

        // MUST 1 (Q5036): refuse an agent's load if the name is already loaded or its
        // file already exists in the plugins folder — before any copy, so the agent
        // can never overwrite a trusted plug-in's code.
        if by_agent {
            if s.plugins.plugins.iter().any(|p| p.name == name) {
                return Err(bad("plugins", format!("plug-in '{name}' is already installed: an agent may not replace it")));
            }
            if dest.exists() && dest != path {
                return Err(bad(
                    "plugins",
                    format!("a file named '{}' already exists in the plugins folder: an agent may not overwrite it", file_name.to_string_lossy()),
                ));
            }
            // An agent's load always starts with no trust: disabled and no grants.
            s.plugins.save_permissions(name, &crate::plugins::PluginPermissions { allowed: false, files: false, network: false });
        }

        // Non-agent loads also check name collision (existing behaviour in
        // load_plugin_as), but they may overwrite the file (a user re-loading).
        if dest != path {
            std::fs::copy(path, &dest).map_err(|e| crate::EngineError::Other(format!("cannot copy plugin: {e}")))?;
        }

        s.plugins.load_plugin_as(&dest, by_agent)?;
        let plugin = s.plugins.plugins.last().ok_or_else(|| crate::EngineError::Other("plugin not loaded".into()))?;
        let (name, enabled) = (plugin.name.clone(), plugin.enabled);
        let cmds: Vec<Value> = plugin
            .commands
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "label": c.label,
                })
            })
            .collect();
        s.echo(format!("Loaded plug-in: {name} ({} command{})", cmds.len(), if cmds.len() == 1 { "" } else { "s" }));
        if !enabled {
            s.echo(format!("Plug-in {name} is disabled until you enable it (PLUGINS enable)."));
        }
        Ok(json!({ "name": name, "commands": cmds, "enabled": enabled }))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(crate::EngineError::Other("plug-ins are not available on the web".into()))
    }
}

fn run_enable(s: &mut Session, p: &Value, enabled: bool) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let name = str_param(p, "name").ok_or_else(|| bad("plugins", "`name` is required"))?;
        s.plugins.set_enabled(name, enabled)?;
        let action = if enabled { "enabled" } else { "disabled" };
        s.echo(format!("Plug-in {name} {action}."));
        Ok(json!({ "name": name, "enabled": enabled }))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p, enabled);
        Err(crate::EngineError::Other("plug-ins are not available on the web".into()))
    }
}

fn run_remove(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let name = str_param(p, "name").ok_or_else(|| bad("plugins", "`name` is required"))?;
        s.plugins.remove(name)?;
        s.echo(format!("Plug-in {name} removed."));
        Ok(json!({ "removed": name }))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(crate::EngineError::Other("plug-ins are not available on the web".into()))
    }
}

fn run_commands(s: &Session) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cmds: Vec<Value> = s
            .plugins
            .all_commands()
            .iter()
            .map(|c| {
                let plugin_name = s.plugins.plugins.get(c.plugin_idx).map(|p| p.name.as_str()).unwrap_or("?");
                json!({
                    "id": c.id,
                    "label": c.label,
                    "params": c.params_doc,
                    "plugin": plugin_name,
                })
            })
            .collect();
        Ok(json!(cmds))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Ok(json!([]))
    }
}

fn run_reload(s: &mut Session) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.plugins = match s.plugins.dir() {
            Some(d) => crate::plugins::PluginHost::in_dir(d),
            None => crate::plugins::PluginHost::new(),
        };
        s.plugins.load_all(&mut s.log);
        let count = s.plugins.plugins.len();
        s.echo(format!("Reloaded {count} plug-in{}.", if count == 1 { "" } else { "s" }));
        Ok(s.plugins.list())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Ok(json!([]))
    }
}
