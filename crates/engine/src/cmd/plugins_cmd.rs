//! The PLUGINS command: list, load, enable, disable and remove WASM plug-ins.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_param};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("plugins", "Plug-ins", run)
            .menu(&["Tools", "Plug-ins"])
            .alias(&["plugin", "appload.wasm"])
            .params("{action?: \"list\" (default) | \"load\" | \"enable\" | \"disable\" | \"remove\" | \"commands\" | \"reload\", name?: string, path?: string} → list: [{name, path, enabled, commands: [{id, label, params}]}]; load: {name, commands}; commands: [{id, label, plugin}]")
            .enabled(always)
            .noundo(),
    ]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").unwrap_or("list");
    match action {
        "list" => run_list(s),
        "load" => run_load(s, p),
        "enable" => run_enable(s, p, true),
        "disable" => run_enable(s, p, false),
        "remove" => run_remove(s, p),
        "commands" => run_commands(s),
        "reload" => run_reload(s),
        _ => Err(bad("plugins", format!("unknown action: {action}"))),
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
        let plugins_dir = crate::plugins::plugins_dir().ok_or_else(|| crate::EngineError::Other("cannot determine plugins directory".into()))?;
        let _ = std::fs::create_dir_all(&plugins_dir);
        let dest = plugins_dir.join(path.file_name().ok_or_else(|| bad("plugins", "invalid file name"))?);
        if dest != path {
            std::fs::copy(path, &dest).map_err(|e| crate::EngineError::Other(format!("cannot copy plugin: {e}")))?;
        }

        s.plugins.load_plugin(&dest)?;
        let plugin = s.plugins.plugins.last().ok_or_else(|| crate::EngineError::Other("plugin not loaded".into()))?;
        let name = plugin.name.clone();
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
        Ok(json!({ "name": name, "commands": cmds }))
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
        s.plugins = crate::plugins::PluginHost::new();
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
