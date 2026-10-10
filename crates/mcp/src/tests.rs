//! An agent completes realistic drafting tasks through MCP only, and verifies through MCP only.

use serde_json::{Value, json};

use crate::{Headless, Server};

fn call(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let r = s.handle_line(&line).expect("reply");
    serde_json::from_str(&r).unwrap()
}

fn tool(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, 99, "tools/call", json!({"name": name, "arguments": args}));
    let c = &r["result"]["content"][0];
    assert_ne!(r["result"]["isError"], json!(true), "{name} failed: {c}");
    if c["type"] == "text" { serde_json::from_str(c["text"].as_str().unwrap()).unwrap_or(Value::Null) } else { c.clone() }
}

#[test]
fn lifecycle_and_tools() {
    let mut s = Server::new(Box::new(Headless::default()));
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "cadcraft");
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert!(s.is_initialized());
    let tools = call(&mut s, 2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "command_line"));
    let bad = call(&mut s, 3, "nope", json!({}));
    assert_eq!(bad["error"]["code"], -32601);
}

#[test]
fn agent_draws_a_plate_with_holes() {
    let mut s = Server::new(Box::new(Headless::default()));
    // Task 1: an outline via the command line, AutoCAD style.
    tool(&mut s, "command_line", json!({"text": "rectang 0,0 100,60"}));
    // Task 2: four holes with JSON.
    for (x, y) in [(10, 10), (90, 10), (10, 50), (90, 50)] {
        tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [x, y], "radius": 4}}));
    }
    // Task 3: a layer and a centre line on it.
    tool(&mut s, "execute", json!({"command": "layer.new", "params": {"name": "Center", "color": "red", "current": true}}));
    tool(&mut s, "command_line", json!({"text": "line -5,30 105,30"}));
    tool(&mut s, "command_line", json!({"text": ""}));
    // Task 4: a script with text. TEXT keeps asking for further lines until an empty one, as in
    // AutoCAD, so the script ends it with a blank line (= Enter).
    let r = tool(&mut s, "script", json!({"text": "TEXT 0,-10 5 0 PLATE 100x60\n\n"}));
    assert_eq!(r["state"]["running"], Value::Null, "TEXT must be finished: {r}");
    // Verify only through MCP.
    let d = tool(&mut s, "inspect_drawing", json!({}));
    assert_eq!(d["counts"]["Circle"], 4);
    assert_eq!(d["counts"]["Polyline"], 1);
    assert_eq!(d["counts"]["Line"], 1);
    assert_eq!(d["counts"]["Text"], 1);
    let lines = tool(&mut s, "query_entities", json!({"type": "Line", "layer": "Center"}));
    assert_eq!(lines["count"], 1);
    // Task 5: undo the text, check, then render.
    let u = tool(&mut s, "execute", json!({"command": "undo"}));
    assert_eq!(u["undone"], json!(["Single Line Text"]), "{u}");
    let d = tool(&mut s, "inspect_drawing", json!({"entities": false}));
    assert!(d["counts"].get("Text").is_none());
    assert_eq!(d["counts"]["Circle"], 4);
    assert_eq!(d["counts"]["Polyline"], 1);
    assert_eq!(d["counts"]["Line"], 1);
    let lines = tool(&mut s, "query_entities", json!({"type": "Line", "layer": "Center"}));
    assert_eq!(lines["count"], 1, "the centre line must survive undoing the text");
    let img = tool(&mut s, "render", json!({"width": 320, "height": 200}));
    assert_eq!(img["type"], "image");
    assert!(img["data"].as_str().unwrap().len() > 100);
}

#[test]
fn errors_are_tool_errors_not_crashes() {
    let mut s = Server::new(Box::new(Headless::default()));
    let r = call(&mut s, 1, "tools/call", json!({"name": "execute", "arguments": {"command": "circle", "params": {"center": "x"}}}));
    assert_eq!(r["result"]["isError"], true);
    let r = call(&mut s, 2, "tools/call", json!({"name": "screenshot", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true);
    assert!(s.handle_line("{not json").unwrap().contains("-32700"));
}

/// Unknown tools and arguments that don't fit a tool's schema are invalid params (-32602), never
/// guessed at; an empty batch is one Invalid Request.
#[test]
fn bad_tool_calls_are_invalid_params() {
    let mut s = Server::new(Box::new(Headless::default()));
    tool(&mut s, "command_line", json!({"text": "circle 0,0 1"}));
    let bad = [
        json!({"name": "nope", "arguments": {}}),
        json!({"name": "command_line", "arguments": {}}),
        json!({"name": "command_line", "arguments": {"text": 5}}),
        json!({"name": "command_line", "arguments": "circle 1,1 1"}),
        json!({"name": "script", "arguments": {}}),
        json!({"name": "query_entities", "arguments": {"window": [-5, -5, 5, 5]}}),
        json!({"name": "query_entities", "arguments": {"window": [[-5, -5]]}}),
        json!({"name": "query_entities", "arguments": {"window": {"min": [-5, -5], "max": [5, 5]}}}),
        json!({"name": "query_entities", "arguments": {"type": 7}}),
        json!({"name": "query_entities", "arguments": {"layer": ["0"]}}),
        json!({"name": "query_entities", "arguments": {"offset": -1}}),
    ];
    for (i, params) in bad.into_iter().enumerate() {
        let r = call(&mut s, i as u64, "tools/call", params.clone());
        assert_eq!(r["error"]["code"], -32602, "{params} -> {r}");
    }
    // Nothing was typed: the circle is still the only object and no command is running.
    let d = tool(&mut s, "inspect_drawing", json!({"entities": false}));
    assert_eq!(d["entityCount"], 1, "{d}");
    assert_eq!(s.handle_line("[]").map(|r| serde_json::from_str::<Value>(&r).unwrap()["error"]["code"].clone()), Some(json!(-32600)));
}

/// `count` is every match while `returned` is the page; a window matches geometry, not bounding boxes.
#[test]
fn query_entities_pages_and_windows_by_geometry() {
    let mut s = Server::new(Box::new(Headless::default()));
    for c in ["circle 0,0 1", "circle 100,100 1", "circle 200,0 1"] {
        tool(&mut s, "command_line", json!({"text": c}));
    }
    // An arc whose bounding box covers the window but whose curve stays outside it.
    tool(&mut s, "execute", json!({"command": "arc", "params": {"p1": [40, 0], "p2": [50, 10], "p3": [60, 0]}}));
    let r = tool(&mut s, "query_entities", json!({"type": "Circle", "limit": 1, "offset": 1}));
    assert_eq!((r["count"].clone(), r["returned"].clone()), (json!(3), json!(1)), "{r}");
    assert_eq!(r["entities"][0]["geometry"]["center"]["x"], 100.0, "{r}");
    let r = tool(&mut s, "query_entities", json!({"window": [[45, 1], [55, 5]]}));
    assert_eq!(r["count"], 0, "the arc passes above this window: {r}");
    let r = tool(&mut s, "query_entities", json!({"window": [[-5, -5], [55, 5]]}));
    assert_eq!(r["count"], 2, "the circle at 0,0 and the arc crossing the window: {r}");
    let r = tool(&mut s, "query_entities", json!({"window": [[-5, -5], [55, 5]], "crossing": false}));
    assert_eq!(r["count"], 1, "only the circle is entirely inside: {r}");
}

#[test]
fn list_commands_filter_matches_names_not_docs() {
    let mut s = Server::new(Box::new(Headless::default()));
    let r = tool(&mut s, "list_commands", json!({"filter": "circle"}));
    let ids: Vec<&str> = r.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"circle"), "{ids:?}");
    assert!(!ids.contains(&"qselect"), "QSELECT only mentions circles in its docs: {ids:?}");
    let r = tool(&mut s, "list_commands", json!({"filter": "pl"}));
    assert!(r.as_array().unwrap().iter().any(|c| c["id"] == "pline"), "aliases and ids match");
}
