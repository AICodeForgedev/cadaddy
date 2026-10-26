//! Q5121 the finish takeoff through the MCP `finishes_takeoff` tool: seed 3's DXF sheets read by
//! the tool, totals per trade and paged lines with their source and status; nothing read gives a
//! message saying what to load.

use cadaddy_mcp::{Headless, Server};
use serde_json::{Value, json};

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

fn tool(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if r["result"]["isError"] == true { Err(text) } else { Ok(serde_json::from_str(&text).unwrap_or(Value::Null)) }
}

#[test]
fn finish_takeoff_through_the_agent_tool() {
    let set = cadaddy_mepfix::generate(3).expect("seed 3");
    let dir = std::env::temp_dir().join(format!("cadaddy-mcp-finishes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (n, b) in set.files.iter().filter(|(n, _)| n.ends_with(".dxf")) {
        std::fs::write(dir.join(n), b).expect("write");
    }
    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    let listed = rpc(&mut s, "tools/list", json!({}));
    assert!(listed["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "finishes_takeoff"));
    tool(&mut s, "new_drawing", json!({})).expect("a drawing");

    let r = tool(&mut s, "finishes_takeoff", json!({ "dir": dir.display().to_string(), "trade": "specialties" })).expect("takeoff");
    assert_eq!(r["totals"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(r["totals"][0]["trade"], "specialties");
    let lines = r["lines"].as_array().expect("lines");
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|l| l["status"] == "rule" && l["rule"].as_str().is_some_and(|x| x.contains("standard"))), "{r}");

    let none = tool(&mut s, "finishes_takeoff", json!({})).expect("no error");
    assert!(none["message"].as_str().is_some_and(|m| m.contains("arch.read")), "{none}");
    assert!(tool(&mut s, "finishes_takeoff", json!({ "trade": "roofing" })).is_err());
}
