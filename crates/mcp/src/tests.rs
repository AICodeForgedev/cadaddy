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
    for name in ["screenshot", "ui_elements"] {
        assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == name), "{name}");
    }
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
    // Saving as CADaddy's own format names the drawing after the .cdy file.
    let cdy = dir.join("own.cdy").to_string_lossy().into_owned();
    let r = tool(&mut s, "save", json!({"path": cdy}));
    assert_eq!(r["path"], json!(cdy), "{r}");
    assert!(std::path::Path::new(&cdy).is_file());
    assert!(tool(&mut s, "list_drawings", json!({}))["drawings"][1]["title"] == json!("own.cdy"));
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

fn read(s: &mut Server, uri: &str) -> Value {
    let r = call(s, 50, "resources/read", json!({"uri": uri}));
    let text = r["result"]["contents"][0]["text"].as_str().unwrap_or_else(|| panic!("{uri}: {r}"));
    serde_json::from_str(text).unwrap()
}

#[test]
fn resources_list_and_read_the_open_drawings_layers_blocks_and_layouts() {
    let mut s = Server::new(Box::new(Headless::default()));
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["capabilities"]["resources"]["listChanged"], true);
    assert!(init["result"]["capabilities"]["prompts"].is_object());
    tool(&mut s, "command_line", json!({"text": "-layer m WALLS c 1 WALLS "}));
    tool(&mut s, "command_line", json!({"text": "rectang 0,0 10,5"}));
    let list = call(&mut s, 2, "resources/list", json!({}));
    let uris: Vec<&str> = list["result"]["resources"].as_array().unwrap().iter().filter_map(|r| r["uri"].as_str()).collect();
    for u in ["cadaddy://drawing", "cadaddy://layers", "cadaddy://blocks", "cadaddy://layouts", "cadaddy://commands", "cadaddy://drawing/0"] {
        assert!(uris.contains(&u), "{u} in {uris:?}");
    }
    let t = call(&mut s, 3, "resources/templates/list", json!({}));
    assert_eq!(t["result"]["resourceTemplates"][0]["uriTemplate"], "cadaddy://drawing/{index}");

    let layers = read(&mut s, "cadaddy://layers");
    assert!(layers["layers"].as_array().unwrap().iter().any(|l| l["name"] == "WALLS"), "{layers}");
    assert_eq!(layers["current"], "WALLS");
    assert!(read(&mut s, "cadaddy://layouts").as_array().is_some_and(|l| !l.is_empty()));
    assert!(read(&mut s, "cadaddy://blocks").is_object() || read(&mut s, "cadaddy://blocks").is_array());
    assert!(read(&mut s, "cadaddy://commands").as_array().is_some_and(|c| c.len() > 100));
    let d0 = read(&mut s, "cadaddy://drawing/0");
    assert_eq!(d0["counts"]["Polyline"], 1, "{d0}");

    // A second drawing: listed as drawing/1, read without switching away from it.
    tool(&mut s, "new_drawing", json!({}));
    tool(&mut s, "command_line", json!({"text": "circle 0,0 1"}));
    tool(&mut s, "command_line", json!({"text": "circle 5,0 1"}));
    let list = call(&mut s, 4, "resources/list", json!({}));
    assert!(list["result"]["resources"].as_array().unwrap().iter().any(|r| r["uri"] == "cadaddy://drawing/1"));
    assert_eq!(read(&mut s, "cadaddy://drawing/0")["counts"]["Polyline"], 1, "the first drawing, read by number");
    assert_eq!(read(&mut s, "cadaddy://drawing/1")["counts"]["Circle"], 2);
    assert_eq!(read(&mut s, "cadaddy://drawing")["counts"]["Circle"], 2, "the active one is still the second");

    // Unknown resources.
    for u in ["cadaddy://drawing/7", "cadaddy://drawing/x", "cadaddy://nope", "file:///etc/passwd"] {
        let r = call(&mut s, 5, "resources/read", json!({"uri": u}));
        assert_eq!(r["error"]["code"], -32002, "{u}: {r}");
    }
    assert_eq!(call(&mut s, 6, "resources/read", json!({}))["error"]["code"], -32602);
}

#[test]
fn opening_and_closing_drawings_notifies_the_client() {
    let mut s = Server::new(Box::new(Headless::default()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    call(&mut s, 2, "resources/list", json!({}));
    // Drawing in the same drawing: no change to the list.
    tool(&mut s, "command_line", json!({"text": "line 0,0 1,1"}));
    assert!(s.take_notifications().is_empty());
    tool(&mut s, "new_drawing", json!({}));
    let n = s.take_notifications();
    assert_eq!(n.len(), 1);
    let v: Value = serde_json::from_str(&n[0]).unwrap();
    assert_eq!(v["method"], "notifications/resources/list_changed");
    assert!(v.get("id").is_none(), "a notification has no id");
    assert!(s.take_notifications().is_empty(), "sent once");
    tool(&mut s, "execute", json!({"command": "document.close", "params": {"discard": true}}));
    assert_eq!(s.take_notifications().len(), 1);

    // Through serve(): the notification follows the reply on its own line.
    let mut out = Vec::new();
    let input = format!("{}\n", json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "new_drawing", "arguments": {}}}));
    s.serve(input.as_bytes(), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["id"], 9);
    assert_eq!(lines[1]["method"], "notifications/resources/list_changed");
}

#[test]
fn prompts_list_and_get_the_agent_recipes() {
    let mut s = Server::new(Box::new(Headless::default()));
    let list = call(&mut s, 1, "prompts/list", json!({}));
    let names: Vec<&str> = list["result"]["prompts"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    assert_eq!(names, ["floor_plan_from_sketch", "schedule_blocks", "audit_report"]);
    let plan = call(
        &mut s,
        2,
        "prompts/get",
        json!({"name": "floor_plan_from_sketch", "arguments": {"sketch": "Two rooms, 12x10 and 10x10, a door between them", "units": "feet"}}),
    );
    let text = plan["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("Two rooms, 12x10") && text.contains("feet") && text.contains("command_line"), "{text}");
    assert_eq!(plan["result"]["messages"][0]["role"], "user");
    let sched = call(&mut s, 3, "prompts/get", json!({"name": "schedule_blocks", "arguments": {"blocks": "DOOR*"}}));
    assert!(sched["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("\"blocks\": \"DOOR*\""));
    let audit = call(&mut s, 4, "prompts/get", json!({"name": "audit_report"}));
    assert!(audit["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("report only"));
    // Errors: a required argument missing, an unknown prompt, no name, a huge argument.
    assert_eq!(call(&mut s, 5, "prompts/get", json!({"name": "floor_plan_from_sketch"}))["error"]["code"], -32602);
    assert_eq!(call(&mut s, 6, "prompts/get", json!({"name": "nope"}))["error"]["code"], -32602);
    assert_eq!(call(&mut s, 7, "prompts/get", json!({}))["error"]["code"], -32602);
    let huge = "x".repeat(30_000);
    assert_eq!(call(&mut s, 8, "prompts/get", json!({"name": "floor_plan_from_sketch", "arguments": {"sketch": huge}}))["error"]["code"], -32602);
}

/// Typed LISP longer than one slice finishes without an interface ticking it (battle test
/// 0.7.7c: it never did, and every later LISP line said a routine was already running).
#[test]
fn headless_typed_lisp_runs_to_its_end() {
    let mut s = Server::new(Box::new(Headless::default()));
    let r = tool(&mut s, "command_line", json!({"text": "(progn (setq n 0) (repeat 300000 (setq n (1+ n))) (princ n))"}));
    assert!(r.to_string().contains("300000"), "{r}");
    let r = tool(&mut s, "command_line", json!({"text": "(princ (+ 1 2))"}));
    assert!(!r.to_string().contains("already running"), "{r}");
    assert!(r.to_string().contains('3'), "{r}");
}

#[test]
fn stairs_tool_detects_lists_and_confirms() {
    let mut s = Server::new(Box::new(Headless::default()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    tool(&mut s, "execute", json!({"command": "layer.new", "params": {"name": "A-FLOR-STRS", "current": true}}));
    for i in 0..=18 {
        let x = 100.0 + 11.0 * f64::from(i);
        tool(&mut s, "execute", json!({"command": "line", "params": {"points": [[x, 0.0], [x, 44.0]]}}));
    }
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [70, 20], "text": "UP", "height": 6}}));
    tool(
        &mut s,
        "execute",
        json!({"command": "bspec.set", "params": {"replace": true, "spec": {
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}, {"name": "L2", "elevation_ft": 12, "height_ft": 12, "region": [5000, 0, 6000, 1000]}],
        "layers": [{"layer": "A-FLOR-STRS", "role": "stair"}]}}}),
    );
    let found = tool(&mut s, "stairs", json!({"action": "detect", "store": true}));
    assert_eq!(found["stairs"][0]["risers"], 19, "{found}");
    let listed = tool(&mut s, "stairs", json!({"action": "list"}));
    assert_eq!(listed["stairs"][0]["confirmed"], false);
    tool(&mut s, "stairs", json!({"action": "confirm", "all": true}));
    assert_eq!(tool(&mut s, "stairs", json!({"action": "list"}))["stairs"][0]["confirmed"], true);
    let r = call(&mut s, 7, "tools/call", json!({"name": "stairs", "arguments": {}}));
    assert_eq!(r["error"]["code"], -32602, "action is required: {r}");
}

#[test]
fn rooms_tool_lists_gets_and_builds() {
    let mut s = Server::new(Box::new(Headless::default()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    let rect = |a: [f64; 2], b: [f64; 2]| json!({"command": "pline", "params": {"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true}});
    tool(&mut s, "execute", rect([0.0, 0.0], [300.0, 240.0]));
    tool(&mut s, "execute", rect([6.0, 6.0], [294.0, 234.0]));
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [120, 120], "text": "KITCHEN", "height": 6}}));
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [120, 100], "text": "105", "height": 6}}));
    let list = tool(&mut s, "rooms", json!({"action": "list", "outline": false}));
    assert_eq!(list["count"], 1, "{list}");
    assert_eq!(list["rooms"][0]["name"], "KITCHEN");
    let one = tool(&mut s, "rooms", json!({"action": "get", "number": "105"}));
    assert_eq!(one["name"], "KITCHEN");
    assert_eq!(tool(&mut s, "rooms", json!({"action": "build"}))["built"], 1);
    tool_err(&mut s, "rooms", json!({"action": "get", "name": "none"}));
}

/// DRAWING.SUMMARY and DRAWING.QUERY on the sample drawing, with a title block on a layout, a
/// missing image and a missing xref added.
#[test]
fn agent_reads_a_drawing_summary_then_queries_handles() {
    let mut h = Headless::default();
    h.session.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
    let dir = std::env::temp_dir().join(format!("cadaddy-mcp-summary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    {
        let d = h.session.doc_mut().unwrap();
        let mut b = cadaddy_engine::doc::Block::new("site");
        b.xref_path = Some("missing/site.dxf".into());
        d.blocks.insert("site".into(), std::sync::Arc::new(b));
    }
    let mut s = Server::new(Box::new(h));
    // A title block with attributes on Layout1.
    tool(&mut s, "execute", json!({"command": "attdef", "params": {"tag": "SHEET", "default": "A-101", "at": [0, 0], "height": 0.2}}));
    tool(&mut s, "execute", json!({"command": "attdef", "params": {"tag": "TITLE", "default": "BRACKET", "at": [0, -0.5], "height": 0.2}}));
    let defs = tool(&mut s, "query_entities", json!({"type": "Attribute Definition"}));
    let hs: Vec<Value> = defs["entities"].as_array().unwrap().iter().map(|e| e["handle"].clone()).collect();
    assert_eq!(hs.len(), 2, "{defs}");
    tool(&mut s, "execute", json!({"command": "block", "params": {"name": "TITLEBLK", "base": [0, 0], "handles": hs, "keep": "delete"}}));
    tool(&mut s, "execute", json!({"command": "layout", "params": {"option": "set", "name": "Layout1"}}));
    tool(
        &mut s,
        "execute",
        json!({"command": "insert", "params": {"name": "TITLEBLK", "at": [9, 1], "attribs": {"SHEET": "S-1", "TITLE": "PLATE"}}}),
    );
    tool(&mut s, "execute", json!({"command": "layout", "params": {"option": "set", "name": "Model"}}));
    // An image whose file is gone.
    let png = dir.join("photo.png");
    tool(&mut s, "render", json!({"width": 32, "height": 32, "path": png.to_string_lossy()}));
    tool(&mut s, "execute", json!({"command": "imageattach", "params": {"path": png.to_string_lossy(), "at": [0, 0], "scale": 1}}));
    std::fs::remove_file(&png).unwrap();

    let sum = tool(&mut s, "drawing_summary", json!({"textLimit": 2}));
    assert!(sum["units"]["name"].is_string() && sum["extents"]["Model"].is_array(), "{}", sum["units"]);
    let layers = sum["layers"].as_array().unwrap();
    let outline = layers.iter().find(|l| l["name"] == "Outline").unwrap();
    assert!(outline["objects"].as_u64().unwrap() > 0 && outline["current"] == true);
    assert!(sum["layouts"].as_array().unwrap().iter().any(|l| l["name"] == "Layout1" && l["objects"].as_u64().unwrap() >= 1), "{}", sum["layouts"]);
    let tb = sum["blocks"].as_array().unwrap().iter().find(|b| b["name"] == "TITLEBLK").unwrap();
    assert_eq!((tb["references"].as_u64(), tb["definitionObjects"].as_u64()), (Some(1), Some(2)));
    // Texts: paged, every space.
    let texts = &sum["texts"];
    assert_eq!(texts["returned"], 2);
    assert!(texts["count"].as_u64().unwrap() > 2);
    let all = tool(&mut s, "drawing_summary", json!({"sections": ["texts"], "textLimit": 5000}));
    let items = all["texts"]["items"].as_array().unwrap();
    assert!(items.iter().any(|t| t["space"] == "Layout1" && t["tag"] == "SHEET" && t["text"] == "S-1"), "{items:?}");
    assert!(all.get("layers").is_none(), "only the sections asked for");
    let found = tool(&mut s, "drawing_summary", json!({"sections": ["texts"], "textContains": "plate"}));
    assert!(found["texts"]["items"].as_array().unwrap().iter().all(|t| t["text"].as_str().unwrap().to_lowercase().contains("plate")));
    // Title block, references and dimension styles.
    let tbs = sum["titleBlocks"].as_array().unwrap();
    assert!(tbs.iter().any(|t| t["block"] == "TITLEBLK" && t["attributes"]["TITLE"] == "PLATE" && t["space"] == "Layout1"), "{tbs:?}");
    let refs = &sum["references"];
    assert_eq!(refs["xrefs"][0]["name"], "site");
    assert_eq!(refs["xrefs"][0]["found"], false);
    assert_eq!(refs["images"][0]["found"], false, "{refs}");
    assert!(sum["dimStyles"]["used"].as_object().unwrap().values().any(|n| n.as_u64().unwrap() >= 4), "{}", sum["dimStyles"]);

    // Query: by type, layer wildcard, text, block, space and window; paged handles.
    let dims = tool(&mut s, "drawing_query", json!({"type": "Dimension"}));
    let n = dims["count"].as_u64().unwrap();
    assert!(n >= 4 && dims["handles"].as_array().unwrap().len() as u64 == n);
    assert_eq!(tool(&mut s, "drawing_query", json!({"layer": "dim*"}))["count"].as_u64().unwrap(), n);
    let page = tool(&mut s, "drawing_query", json!({"layer": "dim*", "limit": 2, "offset": 1}));
    assert_eq!((page["returned"].as_u64(), page["count"].as_u64()), (Some(2), Some(n)));
    let tb = tool(&mut s, "drawing_query", json!({"block": "titleblk"}));
    assert_eq!((tb["count"].as_u64(), tb["objects"][0]["space"].as_str()), (Some(1), Some("Layout1")));
    assert_eq!(tool(&mut s, "drawing_query", json!({"text": "PLATE", "space": "model"}))["count"], 0, "the title block is on Layout1");
    assert_eq!(tool(&mut s, "drawing_query", json!({"text": "PLATE", "space": "Layout1"}))["count"], 1);
    let win = tool(&mut s, "drawing_query", json!({"window": [[-1000, -1000], [1000, 1000]], "space": "model"}));
    assert!(win["count"].as_u64().unwrap() > n);
    for bad in [json!({"window": [[0, 0]]}), json!({"space": "NoSuchLayout"}), json!({"limit": -1})] {
        let r = call(&mut s, 7, "tools/call", json!({"name": "drawing_query", "arguments": bad}));
        assert!(r["result"]["isError"] == json!(true) || r["error"].is_object(), "{bad} → {r}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn execute_many_places_many_objects_in_one_call() {
    let mut s = Server::new(Box::new(Headless::default()));
    let cmds: Vec<Value> = (0..150).map(|i| json!({"command": "circle", "params": {"center": [i * 3, 0], "radius": 1}})).collect();
    let r = tool(&mut s, "execute_many", json!({"commands": cmds}));
    assert_eq!(r["ran"], 150, "{r}");
    let d = tool(&mut s, "inspect_drawing", json!({"entities": false}));
    assert_eq!(d["counts"]["Circle"], 150);
    // One undo takes them all away.
    tool(&mut s, "execute", json!({"command": "undo", "params": {}}));
    let d = tool(&mut s, "inspect_drawing", json!({"entities": false}));
    assert!(d["counts"].get("Circle").is_none(), "{d}");
    // stopOnError: the failure named, nothing kept.
    let r = tool(
        &mut s,
        "execute_many",
        json!({"commands": [{"command": "circle", "params": {"center": [0, 0], "radius": 1}}, {"command": "nope"}], "stopOnError": true}),
    );
    assert_eq!(r["error"]["index"], 1);
    assert_eq!(r["undone"], 1);
    let r = call(&mut s, 9, "tools/call", json!({"name": "execute_many", "arguments": {}}));
    assert!(r["error"].is_object() || r["result"]["isError"] == true, "commands are required: {r}");
}

#[test]
fn watched_events_follow_tool_calls_as_notifications() {
    let mut s = Server::new(Box::new(Headless::default()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    tool(&mut s, "watch_events", json!({}));
    let _ = s.take_notifications();
    tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [0, 0], "radius": 2}}));
    let notes: Vec<Value> = s.take_notifications().iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let events: Vec<&Value> = notes.iter().filter(|n| n["method"] == "notifications/cadaddy/event").collect();
    let kinds: Vec<&str> = events.iter().map(|n| n["params"]["type"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"command.run") && kinds.contains(&"drawing.changed"), "{kinds:?}");
    // Not sent again after the next call.
    tool(&mut s, "inspect_drawing", json!({"entities": false}));
    let again: Vec<String> = s.take_notifications();
    assert!(!again.iter().any(|l| l.contains("drawing.changed")), "{again:?}");
    // Off: no more.
    tool(&mut s, "watch_events", json!({"on": false}));
    tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [9, 0], "radius": 2}}));
    assert!(!s.take_notifications().iter().any(|l| l.contains("cadaddy/event")));
}

/// The MCP client presents the control token first; a refused token is a clear error.
#[test]
fn remote_presents_the_control_token_first() {
    use std::io::{BufRead, BufReader, Write};
    for (token, answer, ok) in
        [("good", r#"{"id":0,"ok":true,"result":{"authenticated":true}}"#, true), ("bad", r#"{"id":0,"ok":false,"error":"unauthorized: no"}"#, false)]
    {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut w = s;
            writeln!(w, "{answer}").unwrap();
            line
        });
        let s = std::net::TcpStream::connect(addr).unwrap();
        let mut reader = BufReader::new(s.try_clone().unwrap());
        let mut writer = s;
        let r = crate::backend::authenticate_with(&mut reader, &mut writer, Some(token.to_string()));
        assert_eq!(r.is_ok(), ok, "{r:?}");
        let first: serde_json::Value = serde_json::from_str(server.join().unwrap().trim()).unwrap();
        assert_eq!(first["method"], "auth");
        assert_eq!(first["params"]["token"], token);
    }
}
