//! MCP resources: the open drawings (`cadaddy://drawing/<index>`, a JSON summary each; the
//! active one also as `cadaddy://drawing`), the active drawing's layers, blocks and layouts, and
//! the command catalog. Everything is read through the backend's `engine.execute`, so the
//! headless session and a running app answer alike.

use serde_json::{Value, json};

use crate::backend::Backend;

const JSON: &str = "application/json";
/// Error code for a resource that doesn't exist (MCP's "resource not found").
pub const NOT_FOUND: i64 = -32002;

fn exec(b: &mut dyn Backend, command: &str, params: Value) -> Result<Value, String> {
    b.call("engine.execute", json!({"command": command, "params": params}))
}

/// The open drawings, as `document.list` reports them.
pub fn drawings(b: &mut dyn Backend) -> Vec<Value> {
    exec(b, "document.list", json!({})).ok().and_then(|v| v["drawings"].as_array().cloned()).unwrap_or_default()
}

/// What tells whether the list of resources changed: the open drawings' titles and paths.
pub fn signature(b: &mut dyn Backend) -> Vec<(String, String)> {
    drawings(b).iter().map(|d| (d["title"].as_str().unwrap_or_default().to_string(), d["path"].as_str().unwrap_or_default().to_string())).collect()
}

/// The resources, and the open drawings' signature they were listed from.
pub fn list(b: &mut dyn Backend) -> (Value, Vec<(String, String)>) {
    let mut out = vec![
        json!({"uri": "cadaddy://drawing", "name": "Active drawing", "description": "Summary of the drawing being edited: counts, layers, extents, selection, entities", "mimeType": JSON}),
        json!({"uri": "cadaddy://layers", "name": "Layers", "description": "The active drawing's layers and their states", "mimeType": JSON}),
        json!({"uri": "cadaddy://blocks", "name": "Blocks", "description": "The active drawing's block definitions", "mimeType": JSON}),
        json!({"uri": "cadaddy://layouts", "name": "Layouts", "description": "The active drawing's layouts (paper space) and their viewports", "mimeType": JSON}),
        json!({"uri": "cadaddy://commands", "name": "Command catalog", "description": "Every command with its parameters", "mimeType": JSON}),
    ];
    let open = drawings(b);
    let sig =
        open.iter().map(|d| (d["title"].as_str().unwrap_or_default().to_string(), d["path"].as_str().unwrap_or_default().to_string())).collect();
    for d in open {
        let i = d["index"].as_u64().unwrap_or_default();
        let title = d["title"].as_str().unwrap_or("Drawing");
        out.push(json!({
            "uri": format!("cadaddy://drawing/{i}"),
            "name": title,
            "title": format!("Drawing {i}: {title}"),
            "description": format!("Open drawing {i}{}", if d["active"] == true { " (active)" } else { "" }),
            "mimeType": JSON,
        }));
    }
    (json!({ "resources": out }), sig)
}

pub fn templates() -> Value {
    json!({"resourceTemplates": [
        {"uriTemplate": "cadaddy://drawing/{index}", "name": "Open drawing", "description": "Summary of an open drawing by its number (resources/list lists them)", "mimeType": JSON},
    ]})
}

/// The contents of `uri`.
pub fn read(b: &mut dyn Backend, uri: &str) -> Result<Value, (i64, String)> {
    let internal = |e: String| (-32603, e);
    let v = match uri {
        "cadaddy://drawing" => b.call("drawing.inspect", json!({})).map_err(internal)?,
        "cadaddy://commands" => b.call("engine.commands", json!({})).map_err(internal)?,
        "cadaddy://layers" => {
            let d = exec(b, "drawing.inspect", json!({"entities": false})).map_err(internal)?;
            json!({"layers": d["layers"], "current": d["currentLayer"]})
        }
        "cadaddy://blocks" => exec(b, "blocks.list", json!({})).map_err(internal)?,
        "cadaddy://layouts" => exec(b, "layout.list", json!({})).map_err(internal)?,
        _ => {
            let Some(rest) = uri.strip_prefix("cadaddy://drawing/") else { return Err((NOT_FOUND, format!("unknown resource `{uri}`"))) };
            let i: u64 = rest.parse().map_err(|_| (NOT_FOUND, format!("unknown resource `{uri}`")))?;
            exec(b, "drawing.inspect", json!({"index": i, "limit": 200})).map_err(|e| (NOT_FOUND, e))?
        }
    };
    Ok(json!({"contents": [{"uri": uri, "mimeType": JSON, "text": v.to_string()}]}))
}
