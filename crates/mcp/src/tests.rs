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
    assert_eq!(init["result"]["serverInfo"]["name"], "cadaddy");
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
    // (Model space: the layouts' viewports count in `entityCount` too.)
    assert_eq!(d["counts"], json!({"Circle": 1}), "{d}");
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

#[test]
fn find_text_reads_texts_and_render_shows_one_area() {
    let mut s = Server::new(Box::new(Headless::default()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    tool(&mut s, "execute", json!({"command": "layer.new", "params": {"name": "Texto"}}));
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [1, 1], "height": 0.15, "text": "RECAMARA 1"}}));
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [50, 1], "height": 0.15, "text": "COCINA"}}));
    tool(
        &mut s,
        "execute",
        json!({"command": "mtext", "params": {"at": [1, 5], "height": 0.2, "width": 5, "text": "{\\fArial|b1;PLANTA} BAJA\\PNIVEL 0.00"}}),
    );
    tool(&mut s, "execute", json!({"command": "rectang", "params": {"p1": [0, 0], "p2": [10, 5]}}));

    let all = tool(&mut s, "find_text", json!({}));
    assert_eq!(all["count"], 3);
    let r = tool(&mut s, "find_text", json!({"contains": "recamara"}));
    assert_eq!(r["count"], 1);
    assert_eq!(r["texts"][0]["text"], "RECAMARA 1");
    assert_eq!(r["texts"][0]["at"], json!([1.0, 1.0]));
    assert_eq!(r["texts"][0]["height"], 0.15);
    // Multiline text comes back readable, without its formatting codes.
    let m = tool(&mut s, "find_text", json!({"contains": "planta"}));
    let t = m["texts"][0]["text"].as_str().unwrap();
    assert!(t.contains("PLANTA BAJA") && t.contains("NIVEL 0.00") && !t.contains('\\') && !t.contains('{'), "{t}");
    // By area: only what is inside the window.
    let w = tool(&mut s, "find_text", json!({"window": [[-1, -1], [20, 20]]}));
    assert_eq!(w["count"], 2);
    assert_eq!(tool(&mut s, "find_text", json!({"layer": "nope"}))["count"], 0);

    // A render of one area takes the area's shape.
    let r = call(&mut s, 9, "tools/call", json!({"name": "render", "arguments": {"window": [[0, 0], [10, 5]]}}));
    let c = &r["result"]["content"][0];
    assert_eq!(c["type"], "image", "{r}");
    let png = cadaddy_engine::cmd::file::base64_decode(c["data"].as_str().unwrap()).unwrap();
    let (w, h) = (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()));
    assert_eq!((w, h), (1600, 800));
}

#[test]
fn a_lazy_bridge_starts_without_the_app_and_says_how_to_open_it() {
    // Nothing listens on a port that was just freed.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    let mut s = Server::new(Box::new(crate::Remote::lazy(&addr, false)));
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "cadaddy");
    let tools = call(&mut s, 2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "screenshot"));
    let r = call(&mut s, 3, "tools/call", json!({"name": "command_line", "arguments": {"text": "line 0,0 1,1"}}));
    assert_eq!(r["result"]["isError"], json!(true));
    let msg = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(msg.contains("not answering") && msg.contains("--control"), "{msg}");

    // Once the app listens, the next call reaches it.
    let listener = std::net::TcpListener::bind(&addr).unwrap();
    let app = std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        let (sock, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(sock.try_clone().unwrap()).read_line(&mut line).unwrap();
        let id = serde_json::from_str::<Value>(&line).unwrap()["id"].clone();
        writeln!(&sock, "{}", json!({"id": id, "ok": true, "result": {"prompt": "Command:"}})).unwrap();
    });
    let r = call(&mut s, 4, "tools/call", json!({"name": "command_line", "arguments": {"text": ""}}));
    assert_ne!(r["result"]["isError"], json!(true), "{r}");
    app.join().unwrap();
}

#[test]
fn invalid_utf8_is_a_parse_error_not_the_end_of_the_session() {
    let mut s = Server::new(Box::new(Headless::default()));
    let input: &[u8] = b"\xff\xfe\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\r\n";
    let mut out = Vec::new();
    s.serve(input, &mut out).expect("serve");
    let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["id"], 2);
    assert!(replies[1]["error"].is_null(), "{}", replies[1]);
}

fn tool_err(s: &mut Server, name: &str, args: Value) -> String {
    let r = call(s, 98, "tools/call", json!({"name": name, "arguments": args}));
    assert_eq!(r["result"]["isError"], json!(true), "{name} should fail: {r}");
    r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

#[test]
fn agent_manages_drawings_without_losing_work() {
    // A stand-in file format: writing stores a marker, reading gives a fresh drawing (this crate
    // doesn't depend on the real readers; the CLI installs them).
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks { read: |_, _| Ok(cadaddy_engine::doc::Drawing::new_metric()), write: |_, _| Ok(b"saved".to_vec()), plot: None, read_pdf: None });
    let mut s = Server::new(Box::new(Headless::default()));
    tool(&mut s, "command_line", json!({"text": "circle 0,0 5"}));
    let docs = tool(&mut s, "list_drawings", json!({}));
    assert_eq!(docs["drawings"][0]["modified"], json!(true), "{docs}");
    // Closing unsaved work is refused until the agent says what to do with it.
    let e = tool_err(&mut s, "close_drawing", json!({}));
    assert!(e.contains("unsaved changes"), "{e}");
    let dir = std::env::temp_dir().join(format!("cadaddy-mcp-close-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kept.dxf").to_string_lossy().into_owned();
    let r = tool(&mut s, "close_drawing", json!({"saveAs": path}));
    assert_eq!(r["saved"], json!(path), "{r}");
    assert!(std::path::Path::new(&path).is_file());
    // It opens again, unmodified; a second drawing with changes blocks close_all.
    tool(&mut s, "open", json!({"path": path}));
    tool(&mut s, "new_drawing", json!({}));
    tool(&mut s, "command_line", json!({"text": "line 0,0 10,0 "}));
    let docs = tool(&mut s, "list_drawings", json!({}));
    assert_eq!(docs["drawings"].as_array().map(Vec::len), Some(2), "{docs}");
    tool(&mut s, "switch_drawing", json!({"path": path}));
    assert_eq!(tool(&mut s, "list_drawings", json!({}))["active"], json!(0));
    assert!(tool_err(&mut s, "close_all", json!({})).contains("unsaved changes"));
    // Undo/redo, select, variables and LISP on the second drawing.
    tool(&mut s, "switch_drawing", json!({"index": 1}));
    tool(&mut s, "undo", json!({}));
    assert!(tool(&mut s, "inspect_drawing", json!({}))["counts"].get("Line").is_none());
    tool(&mut s, "redo", json!({}));
    let sel = tool(&mut s, "select", json!({"all": true}));
    assert!(sel.to_string().contains("handles"), "{sel}");
    tool(&mut s, "set_variable", json!({"name": "LTSCALE", "value": 2}));
    assert!(tool(&mut s, "get_variable", json!({"name": "LTSCALE"})).to_string().contains('2'));
    let l = tool(&mut s, "run_lisp", json!({"code": "(+ 1 2)"}));
    assert_eq!(l["value"], json!("3"), "{l}");
    let r = tool(&mut s, "close_all", json!({"discard": true}));
    assert_eq!(r["closed"].as_array().map(Vec::len), Some(2), "{r}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn auto_works_without_an_open_app() {
    // Nothing listens on the agent port in tests: the server runs a session of its own.
    if std::net::TcpStream::connect(("127.0.0.1", cadaddy_engine::AGENT_PORT)).is_ok() {
        return;
    }
    let mut s = Server::new(Box::new(crate::Auto::default()));
    tool(&mut s, "command_line", json!({"text": "circle 0,0 5"}));
    assert_eq!(tool(&mut s, "inspect_drawing", json!({}))["counts"]["Circle"], 1);
}

#[test]
fn agent_reads_the_prompt_and_waits_for_idle() {
    let mut s = Server::new(Box::new(Headless::default()));
    tool(&mut s, "command_line", json!({"text": "pline 0,0"}));
    let p = tool(&mut s, "get_prompt", json!({}));
    assert_eq!(p["running"], json!("pline"), "{p}");
    assert!(p["keywords"].as_array().is_some_and(|k| k.iter().any(|k| k == "Undo")), "{p}");
    assert_eq!(p["accept"]["point"], json!(true), "{p}");
    let w = tool(&mut s, "wait_until_idle", json!({"timeoutMs": 1000}));
    assert_eq!(w["busy"], json!(false), "{w}");
    assert!(w.get("timedOut").is_none());
}

#[test]
fn lisp_errors_are_tool_errors() {
    let mut s = Server::new(Box::new(Headless::default()));
    assert!(tool_err(&mut s, "run_lisp", json!({"code": "(car 5)"})).contains("LISP error"));
    assert_eq!(tool(&mut s, "run_lisp", json!({"code": "(* 6 7)"}))["value"], json!("42"));
    // Unknown variables are refused, known ones read and set.
    assert!(tool_err(&mut s, "set_variable", json!({"name": "NOSUCHVAR", "value": 1})).contains("Unknown variable"));
    tool(&mut s, "set_variable", json!({"name": "FILEDIA", "value": 0}));
    tool(&mut s, "new_drawing", json!({}));
    assert!(tool(&mut s, "get_variable", json!({"name": "FILEDIA"})).to_string().contains('0'), "FILEDIA is session-wide");
    for name in ["CMDECHO", "EXPERT", "ATTDIA", "ATTREQ"] {
        let v = tool(&mut s, "get_variable", json!({"name": name}));
        assert!(!v.to_string().contains("null"), "{name}: {v}");
    }
    assert_eq!(tool(&mut s, "get_variable", json!({"name": "ATTREQ"}))["value"], json!(1));
}
