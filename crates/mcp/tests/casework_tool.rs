//! Q301 rework, MUST 5: casework and plumbing fixtures read from an architectural floor plan
//! through the MCP `mep` tool (`action: "casework"`): a compact page of items, each with its
//! source (sheet, schedule) and status, paged by `cursor`; `place: true` makes the families.

use cadaddy_mcp::{Headless, Server};
use serde_json::{Value, json};

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

fn server() -> Server {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    s
}

/// A tool's JSON answer (and its text's length), or the error text.
fn tool(s: &mut Server, name: &str, args: Value) -> Result<(Value, usize), String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    if let Some(e) = r.get("error") {
        return Err(e["message"].as_str().unwrap_or_default().to_string());
    }
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if r["result"]["isError"] == true { Err(text) } else { Ok((serde_json::from_str(&text).unwrap_or(Value::Null), text.len())) }
}

#[test]
fn casework_through_the_mep_tool() {
    let (t, st, kinds) = cadaddy_mepfix::office_planned(1);
    let sh = t.sheets.iter().find(|s| s.number.starts_with("A-10") && s.level.as_deref() == Some("2")).expect("floor plan").clone();
    let kind = &kinds.iter().find(|(n, _)| *n == sh.number).expect("kind").1;
    let (d, _, _) = cadaddy_mepfix::draw::draw(&t, &st, &sh, kind);
    let dir = std::env::temp_dir().join(format!("cadaddy-casework-tool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("Sample Floor Plan.dxf");
    std::fs::write(&path, cadaddy_io::write_dxf(&d)).expect("write");

    let mut s = server();
    tool(&mut s, "open", json!({ "path": path.display().to_string() })).expect("open");
    let (page, len) = tool(&mut s, "mep", json!({ "action": "casework", "limit": 5 })).expect("casework");
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 5, "{page}");
    assert_eq!(page["next"], json!(5));
    assert!(len < 6000, "a compact page: {len} bytes");
    let total = page["count"].as_u64().expect("count");
    let want = t.casework.iter().filter(|c| c.level == "2").count() + cadaddy_mepfix::casework_reader::truth_fixtures(&t, "2").len();
    assert_eq!(total, want as u64);
    for it in items {
        assert_eq!(it["source"]["sheet"], json!(sh.number), "{it}");
        assert_eq!(it["status"], "schedule", "{it}");
        assert!(it["source"]["schedule"].is_string(), "{it}");
    }
    // The next page by cursor.
    let (p2, _) = tool(&mut s, "mep", json!({ "action": "casework", "limit": 5, "cursor": 5 })).expect("page 2");
    assert_eq!(p2["offset"], json!(5));
    assert_ne!(p2["items"][0]["id"], page["items"][0]["id"]);
    // Only water closets; then place the families.
    let (wc, _) = tool(&mut s, "mep", json!({ "action": "casework", "kind": "water_closet" })).expect("kind");
    assert!(wc["items"].as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|i| i["family"] == "M3_WATER_CLOSET")), "{wc}");
    let (placed, _) = tool(&mut s, "mep", json!({ "action": "casework", "place": true, "limit": 1 })).expect("place");
    assert!(placed["placed"].as_u64() >= Some(6), "{placed}");
    // A bad argument is refused with the schema's words.
    let err = tool(&mut s, "mep", json!({ "action": "casework", "place": "yes" })).expect_err("refused");
    assert!(err.contains("place"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
