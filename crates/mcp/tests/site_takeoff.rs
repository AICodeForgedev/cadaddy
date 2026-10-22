//! Q339 sitework through the agent tools: the synthetic hotel's civil sheets (C-101 site plan,
//! C-102 grading plan, C-103 utility plan) written as DXF files, then the site quantity list
//! asked for through the MCP `site_takeoff` tool and the `site.takeoff` command via `execute`:
//! paged, every line with its status, source sheets and the objects that add up to it, the
//! pipe footage and the earthwork matching the truth. No dialog, no prompt left open.

use std::path::PathBuf;

use cadaddy_mcp::{Headless, Server};
use cadaddy_mepfix::hotel::HotelSize;
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

/// A tool's JSON answer, or the error text.
fn tool(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if r["result"]["isError"] == true { Err(text) } else { Ok(serde_json::from_str(&text).unwrap_or(Value::Null)) }
}

fn hotel_sheets() -> (cadaddy_mepfix::truth::Truth, PathBuf) {
    let set = cadaddy_mepfix::generate_hotel(2, HotelSize::Small).expect("the hotel");
    let dir = std::env::temp_dir().join(format!("cadaddy-site-takeoff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for n in ["C-101", "C-102", "C-103"] {
        std::fs::write(dir.join(format!("{n}.dxf")), set.file(&format!("{n}.dxf")).expect("sheet")).expect("write");
    }
    (set.truth, dir)
}

fn line<'a>(r: &'a Value, description: &str) -> &'a Value {
    r["quantities"].as_array().and_then(|q| q.iter().find(|q| q["description"] == description)).unwrap_or_else(|| panic!("no {description} in {r}"))
}

#[test]
fn site_quantities_through_the_agent_tools() {
    let (truth, dir) = hotel_sheets();
    let paths: Vec<String> = ["C-101", "C-102", "C-103"].iter().map(|n| dir.join(format!("{n}.dxf")).display().to_string()).collect();
    let mut s = server();
    tool(&mut s, "new_drawing", json!({})).expect("a drawing");

    let all = tool(&mut s, "site_takeoff", json!({ "paths": paths, "limit": 500 })).expect("site_takeoff");
    assert_eq!(all["sheets"], json!(["C-101", "C-102", "C-103"]));
    let quantities = all["quantities"].as_array().expect("quantities");
    assert_eq!(all["total"].as_u64(), Some(quantities.len() as u64));
    assert!(all.get("next_offset").is_none());
    // Every line: a status, its sheets, and objects whose shares add up to it.
    for q in quantities {
        assert!(["read", "rule", "assumed"].contains(&q["status"].as_str().unwrap_or_default()), "{q}");
        assert!(!q["sources"].as_array().expect("sources").is_empty(), "{q}");
        let total = q["quantity"].as_f64().expect("a number");
        let sum: f64 = q["elements"].as_array().expect("elements").iter().map(|e| e["quantity"].as_f64().unwrap_or(f64::NAN)).sum();
        assert!((sum - total).abs() < 0.005, "{q}");
    }
    // Pipe by system, size and material; manholes by system; earthwork from the surfaces.
    for (kind, desc) in [
        ("underground_storm", "STORM PIPE, 12 IN. RCP"),
        ("underground_sewer", "SANITARY SEWER PIPE, 6 IN. PVC"),
        ("underground_water", "WATER PIPE, 4 IN. DIP"),
    ] {
        let want: f64 = truth.site_elements.iter().filter(|se| se.kind == kind).filter_map(|se| se.length_ft).sum();
        let got = line(&all, desc)["quantity"].as_f64().unwrap_or_default();
        assert!((got - want).abs() <= 0.02 * want, "{desc}: {got} vs {want}");
        assert_eq!(line(&all, desc)["sources"], json!(["C-101", "C-103"]));
    }
    assert_eq!(line(&all, "STORM MANHOLE")["quantity"], json!(3.0));
    assert_eq!(line(&all, "SANITARY MANHOLE")["quantity"], json!(1.0));
    let fill = truth.site_quantities.iter().find(|q| q.description == "EARTHWORK, FILL (COMPACTED)").expect("truth fill").quantity;
    let got = line(&all, "EARTHWORK, FILL (COMPACTED)")["quantity"].as_f64().unwrap_or_default();
    assert!((got - fill).abs() <= 0.02 * fill, "fill {got} vs {fill}");
    assert_eq!(all["earthwork"]["status"], "read");
    assert!(all["earthwork"]["cut_cy"].is_number() && all["earthwork"]["fill_cy"].is_number(), "{}", all["earthwork"]);
    assert_eq!(all["read"]["structures"].as_u64(), Some(truth.site_structures.len() as u64));
    assert_eq!(all["unknown"], json!([]));

    // Paged: the same lines, five at a time.
    let p1 = tool(&mut s, "site_takeoff", json!({ "paths": paths, "limit": 5, "elements": false })).expect("page 1");
    assert_eq!(p1["quantities"].as_array().map(Vec::len), Some(5));
    assert_eq!(p1["next_offset"], json!(5));
    assert!(p1["quantities"][0].get("elements").is_none() && p1["quantities"][0]["element_count"].is_u64(), "{}", p1["quantities"][0]);
    let p2 = tool(&mut s, "site_takeoff", json!({ "paths": paths, "offset": 5, "limit": 5 })).expect("page 2");
    assert_eq!(p2["quantities"][0]["description"], quantities[5]["description"]);

    // The command itself, through `execute`, on the open utility plan: structures and pipe are
    // read; with no grading plan the earthwork is insufficient data, not a number.
    tool(&mut s, "open", json!({ "path": paths[2] })).expect("open C-103");
    let one = tool(&mut s, "execute", json!({ "command": "site.takeoff", "params": { "sheet": "C-103" } })).expect("execute site.takeoff");
    assert_eq!(one["sheets"], json!(["C-103"]));
    let earth = line(&one, "EARTHWORK, CUT AND FILL");
    assert_eq!((earth["status"].as_str(), earth["quantity"].is_null()), (Some("insufficient data"), true), "{earth}");
    assert_eq!(one["earthwork"]["status"], "insufficient data");
    assert_eq!(line(&one, "STORM MANHOLE")["sources"], json!(["C-103"]));
    // No dialog or prompt was left open.
    let prompt = tool(&mut s, "get_prompt", json!({})).expect("prompt");
    assert!(prompt["command"].is_null() || prompt["command"] == "", "{prompt}");

    // A missing file is a guiding error.
    let err = tool(&mut s, "site_takeoff", json!({ "paths": [dir.join("missing.dxf").display().to_string()] })).expect_err("no such file");
    assert!(err.contains("missing.dxf"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
