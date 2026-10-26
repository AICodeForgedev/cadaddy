//! Q5103 framing through the agent tools: a made-up print set's DXF sheets framed with the MCP
//! `framing` tool (build / list / wall / next_stud / field_point / shop_drawing / view), a known
//! wall's next-stud dimension and member counts matching the fixture's true framing under its true
//! layout, a field point kept through a rebuild and idempotent, and the framing view's layers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cadaddy_mcp::{Headless, Server};
use cadaddy_mepfix::framing::{self as fr, End, Status};
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

/// A tool's JSON answer, or the error text (a refused call or a tool error).
fn tool(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    if let Some(m) = r["error"]["message"].as_str() {
        return Err(m.to_string());
    }
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if r["result"]["isError"] == true { Err(text) } else { Ok(serde_json::from_str(&text).unwrap_or(Value::Null)) }
}

fn framing(s: &mut Server, action: &str, params: Value) -> Value {
    tool(s, "framing", json!({"action": action, "params": params})).unwrap_or_else(|e| panic!("framing {action}: {e}"))
}

fn near(a: &Value, b: [f64; 2]) -> bool {
    (a[0].as_f64().unwrap_or(f64::NAN) - b[0]).abs() < 0.1 && (a[1].as_f64().unwrap_or(f64::NAN) - b[1]).abs() < 0.1
}

#[test]
fn framing_through_the_agent_tool() {
    let set = cadaddy_mepfix::generate(2).expect("the set");
    let dir: PathBuf = std::env::temp_dir().join(format!("cadaddy-mcp-framing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (n, b) in set.files.iter().filter(|(n, _)| n.ends_with(".dxf")) {
        std::fs::write(dir.join(n), b).expect("write");
    }
    let t = set.truth;
    let mut s = server();
    tool(&mut s, "new_drawing", json!({})).expect("a drawing");
    assert!(tool(&mut s, "framing", json!({"action": "list"})).is_err_and(|e| e.contains("framing.build")), "asks for a build first");
    assert!(
        tool(&mut s, "framing", json!({"action": "frame_everything"})).is_err_and(|e| e.contains("build") && e.contains("field_point")),
        "names the actions"
    );

    let b = framing(&mut s, "build", json!({"dir": dir.display().to_string(), "limit": 5}));
    assert!(b["stud_walls"].as_u64().unwrap_or(0) > 20, "{b}");
    assert_eq!(b["list"]["walls"].as_array().map(Vec::len), Some(5), "paged");
    assert_eq!(b["list"]["next_offset"], json!(5));

    // A known wall: the truth's open framing (both layouts) and true framing (one).
    let open = fr::frame(&fr::walls_from_truth(&t), &fr::posts_from_truth(&t), &[], &[]);
    let truth = fr::truth_framing(&t);
    let level = open.walls.first().map(|w| w.wall.level.clone()).expect("a wall");
    let listed = framing(&mut s, "list", json!({"level": level, "limit": 500}));
    let rows = listed["walls"].as_array().expect("walls").clone();
    let (i, id) = open
        .walls
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            w.wall.stud_in.is_some() && w.wall.len_in() > 96.0 && [End::A, End::B].iter().all(|e| w.members.iter().any(|m| m.layout == Some(*e)))
        })
        .find_map(|(i, w)| {
            rows.iter()
                .find(|r| near(&r["a_ft"], w.wall.a_ft) && near(&r["b_ft"], w.wall.b_ft))
                .map(|r| (i, r["id"].as_str().unwrap_or_default().to_string()))
        })
        .expect("a known wall");
    let ow = open.walls.get(i).expect("open");
    let e = t.plan_walls.get(i).and_then(|w| w.layout_from.as_deref()).and_then(End::parse).expect("true layout");
    let n = framing(&mut s, "next_stud", json!({"wall": id}));
    assert_eq!(n["from_a"], json!(ow.next_stud_words(End::A)), "{n}");
    assert_eq!(n["from_b"], json!(ow.next_stud_words(End::B)), "{n}");
    let w = framing(&mut s, "wall", json!({"wall": id, "members": true, "limit": 500}));
    let mut read: BTreeMap<String, usize> = BTreeMap::new();
    for m in w["members"].as_array().expect("members") {
        if m["layout"].as_str().is_none_or(|l| l == e.letter()) {
            *read.entry(m["status"].as_str().unwrap_or_default().to_string()).or_default() += 1;
        }
    }
    let mut want: BTreeMap<String, usize> = BTreeMap::new();
    for m in &truth.walls.get(i).expect("true wall").members {
        let k = match m.status {
            Status::Fixed => "fixed",
            Status::Agreed => "agreed",
            Status::Rule => "rule",
            Status::ShopDrawing => "shop_drawing",
            Status::FieldVerified => "field_verified",
        };
        *want.entry(k.to_string()).or_default() += 1;
    }
    assert_eq!(read, want, "wall {id}, true layout {}", e.letter());

    // Field point: entered, the same again changes nothing, kept through a rebuild.
    let first = ow.next_stud_in(End::A)[if e == End::A { 0 } else { 1 }].expect("first stud");
    let p = json!({"wall": id, "tape": {"from": "a", "to_near_edge_in": first}, "author": "agent"});
    let f = framing(&mut s, "field_point", p.clone());
    assert_eq!((f["changed"].clone(), f["matches"].clone()), (json!(true), json!(e.letter())), "{f}");
    assert_eq!(framing(&mut s, "field_point", p)["changed"], json!(false));
    let rb = framing(&mut s, "build", json!({}));
    assert_eq!(rb["reapplied"]["field_points"], json!(1), "{rb}");
    let w = framing(&mut s, "wall", json!({"wall": id}));
    assert!(w["settled"].as_str().is_some_and(|x| x.starts_with("field verified")), "{w}");
    assert!(w["counts"]["field_verified"].as_u64().unwrap_or(0) > 0, "{w}");

    // Shop drawing: refused without its level; the view's layers and dimension text.
    assert!(tool(&mut s, "framing", json!({"action": "shop_drawing", "params": {"file": "missing.dxf"}})).is_err());
    let v = framing(&mut s, "view", json!({"wall": id}));
    let layers: Vec<&str> = v["layers"].as_array().expect("layers").iter().filter_map(Value::as_str).collect();
    assert!(layers.contains(&"FRAMING-DIMS") && layers.contains(&"FRAMING-FIXED") && layers.contains(&"FRAMING-VERIFIED"), "{layers:?}");
    assert!(v["texts"].as_array().is_some_and(|t| t.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("next stud ")))), "{v}");
    let _ = std::fs::remove_dir_all(&dir);
}
