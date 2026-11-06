//! The MCP suite against the headless server: every tool the server lists has a normal call and a
//! bad one here (a tool added without them fails the test), every resource and prompt is read,
//! the JSON-RPC errors carry their codes, huge inputs stay within their caps and in time, and the
//! bridge to a running app reconnects without running anything twice. The same calls against the
//! running app (and the .mcpb's server): `cargo xtask mcptest`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cadaddy_mcp::{Backend, Headless, Remote, Server};
#[path = "support/table.rs"]
mod table;
use serde_json::{Value, json};
use table::table;

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

/// The real file formats (and the writer for a chosen version), as `cadaddy-cli` installs them.
fn io() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, name, o| {
        cadaddy_io::write_with(d, name, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn server() -> Server {
    io();
    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    s
}

/// A tool call's result: `Ok(text)` or `Err((code or 0 for a tool error, message))`.
fn tool(s: &mut Server, name: &str, args: Value) -> Result<String, (i64, String)> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    if let Some(e) = r.get("error") {
        return Err((e["code"].as_i64().unwrap_or(0), e["message"].as_str().unwrap_or_default().to_string()));
    }
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or("<image>").to_string();
    if r["result"]["isError"] == true { Err((0, text)) } else { Ok(text) }
}

#[test]
fn every_listed_tool_has_a_normal_and_a_bad_call() {
    let dir = std::env::temp_dir().join(format!("cadaddy-mcp-suite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let mut s = server();
    let listed: Vec<String> =
        rpc(&mut s, "tools/list", json!({}))["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    let t = table(&d);
    let missing: Vec<&String> = listed.iter().filter(|n| !t.iter().any(|(m, ..)| m == n)).collect();
    assert!(missing.is_empty(), "tools without a suite entry: {missing:?}");
    let mut failed = Vec::new();
    for (name, good, bad) in t.iter().filter(|(n, ..)| listed.iter().any(|l| l == n)) {
        if let Err(e) = tool(&mut s, name, good.clone()) {
            failed.push(format!("{name} {good}: {e:?}"));
        }
        match tool(&mut s, name, bad.clone()) {
            Ok(r) => failed.push(format!("{name} {bad}: accepted ({})", r.chars().take(80).collect::<String>())),
            // Wrong types are invalid params (-32602); a well-typed call the tool refuses is a
            // tool error (code 0 here).
            Err((code, _)) if code == -32602 || code == 0 => {}
            Err(e) => failed.push(format!("{name} {bad}: unexpected error {e:?}")),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn resources_and_prompts_all_read() {
    let mut s = server();
    tool(&mut s, "command_line", json!({ "text": "circle 0,0 1" })).unwrap();
    let list = rpc(&mut s, "resources/list", json!({}));
    let uris: Vec<String> = list["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap().to_string()).collect();
    assert!(uris.len() >= 5, "{uris:?}");
    for uri in &uris {
        let r = rpc(&mut s, "resources/read", json!({ "uri": uri }));
        let text = r["result"]["contents"][0]["text"].as_str().unwrap_or_default();
        assert!(serde_json::from_str::<Value>(text).is_ok(), "{uri}: {r}");
    }
    let templates = rpc(&mut s, "resources/templates/list", json!({}));
    assert!(templates["result"]["resourceTemplates"].as_array().is_some_and(|t| !t.is_empty()), "{templates}");
    assert_eq!(rpc(&mut s, "resources/read", json!({ "uri": "cadaddy://nope" }))["error"]["code"].as_i64().map(|c| c < 0), Some(true));
    let prompts = rpc(&mut s, "prompts/list", json!({}));
    let names: Vec<(String, Vec<String>)> = prompts["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let req = p["arguments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["required"] == true)
                .map(|a| a["name"].as_str().unwrap().to_string())
                .collect();
            (p["name"].as_str().unwrap().to_string(), req)
        })
        .collect();
    assert!(!names.is_empty());
    for (name, req) in &names {
        let args: serde_json::Map<String, Value> = req.iter().map(|a| (a.clone(), json!("Sample"))).collect();
        let r = rpc(&mut s, "prompts/get", json!({ "name": name, "arguments": args }));
        assert!(r["result"]["messages"].as_array().is_some_and(|m| !m.is_empty()), "{name}: {r}");
    }
    assert_eq!(rpc(&mut s, "prompts/get", json!({ "name": "nope" }))["error"]["code"], -32602);
}

#[test]
fn protocol_errors_carry_their_codes() {
    let mut s = server();
    let code = |s: &mut Server, line: &str| -> i64 {
        let r: Value = serde_json::from_str(&s.handle_line(line).expect("a reply")).unwrap();
        r["error"]["code"].as_i64().unwrap_or(0)
    };
    assert_eq!(code(&mut s, "{not json"), -32700);
    assert_eq!(code(&mut s, r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#), -32601);
    assert_eq!(code(&mut s, r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}"#), -32602);
    assert_eq!(code(&mut s, r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#), -32602);
    // A notification (no id) gets no reply; the session goes on.
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#).is_none());
    assert!(tool(&mut s, "get_prompt", json!({})).is_ok());
}

#[test]
fn huge_inputs_stay_within_caps_and_time() {
    let mut s = server();
    let t = Instant::now();
    // A 4 MB command line: refused or ignored, never a hang or a crash.
    let _ = tool(&mut s, "command_line", json!({ "text": "x".repeat(4 << 20) }));
    // Nested JSON 5000 deep: a parse error (serde's recursion limit), not a stack overflow.
    let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
    let line =
        format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"execute","arguments":{{"command":"line","params":{deep}}}}}}}"#);
    let r: Value = serde_json::from_str(&s.handle_line(&line).expect("a reply")).unwrap();
    assert!(r.get("error").is_some() || r["result"]["isError"] == true, "{}", r.to_string().chars().take(200).collect::<String>());
    // Limits past any drawing are capped: the page is what exists.
    tool(&mut s, "command_line", json!({ "text": "circle 0,0 1" })).unwrap();
    let q: Value = serde_json::from_str(&tool(&mut s, "query_entities", json!({ "limit": 1u64 << 60, "offset": 0 })).unwrap()).unwrap();
    assert_eq!(q["returned"], q["count"]);
    // An enormous render is refused (16 to 8192 px a side).
    let r = rpc(&mut s, "tools/call", json!({ "name": "render", "arguments": { "width": 1_000_000, "height": 20 } }));
    assert!(
        r.get("error").is_some() || r["result"]["isError"] == true || r["result"]["content"][0]["type"] == "image",
        "{}",
        r.to_string().chars().take(200).collect::<String>()
    );
    // A long script: 5000 commands.
    let script: String = (0..5000).map(|i| format!("LINE {i},0 {i},1\n\n")).collect();
    tool(&mut s, "script", json!({ "text": script })).unwrap();
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
    assert!(tool(&mut s, "get_prompt", json!({})).is_ok(), "the session still answers");
}

/// A fake app on the control port: answers each line with `{"ok":true,"result":<method>}`,
/// counting what it got, and drops the first connection after its first answer.
fn fake_app() -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let got = Arc::new(Mutex::new(Vec::new()));
    let seen = got.clone();
    std::thread::spawn(move || {
        for (n, conn) in l.incoming().enumerate() {
            let Ok(c) = conn else { continue };
            let mut w = c.try_clone().unwrap();
            let mut r = BufReader::new(c);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let method = v["method"].as_str().unwrap_or_default().to_string();
                seen.lock().unwrap().push(method.clone());
                // The first connection dies before answering its third request (the version
                // handshake sends an extra ping on each connect, so the first user call that
                // loses its reply is now the third message on the wire, not the second).
                if n == 0 && seen.lock().unwrap().len() >= 3 {
                    break;
                }
                let _ = writeln!(w, "{}", json!({ "id": v["id"], "ok": true, "result": method }));
            }
        }
    });
    (addr, got)
}

#[test]
fn the_app_bridge_reconnects_without_running_anything_twice() {
    let (addr, got) = fake_app();
    let mut r = Remote::connect(&addr).unwrap();
    assert_eq!(r.call("ping", json!({})).unwrap(), json!("ping"));
    // The connection dies with a read-only call in flight: it is sent again on a new connection.
    assert_eq!(r.call("drawing.inspect", json!({})).unwrap(), json!("drawing.inspect"));
    // Later calls use the new connection.
    assert_eq!(r.call("engine.execute", json!({ "command": "line" })).unwrap(), json!("engine.execute"));
    let calls = got.lock().unwrap().clone();
    // The version handshake sends a ping on each connect, so connections 0 and 1 each start
    // with a handshake ping before the user call.  The read-only "drawing.inspect" is still the
    // only user-level call that gets repeated.
    assert_eq!(
        calls,
        ["ping", "ping", "drawing.inspect", "ping", "drawing.inspect", "engine.execute"],
        "only the read-only call was repeated (handshake pings are expected on each connect)"
    );
}

#[test]
fn a_call_that_changes_the_drawing_is_not_repeated_after_a_lost_answer() {
    let (addr, got) = fake_app();
    let mut r = Remote::connect(&addr).unwrap();
    r.call("ping", json!({})).unwrap();
    // Lost after it was sent: it may have run, so the bridge reports it rather than run it again.
    assert!(r.call("engine.execute", json!({ "command": "erase" })).is_err());
    assert_eq!(got.lock().unwrap().iter().filter(|m| *m == "engine.execute").count(), 1);
    // The next call reconnects.
    assert_eq!(r.call("cmdline.state", json!({})).unwrap(), json!("cmdline.state"));
}

/// B11-001: `save` lists `version` and `binary`, and a versioned or binary save writes what was
/// asked without warning that it was ignored.
#[test]
fn save_takes_version_and_binary() {
    let dir = std::env::temp_dir().join(format!("cadaddy-mcp-saveversion-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = server();
    let tools = rpc(&mut s, "tools/list", json!({}));
    let save = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "save").unwrap().clone();
    for k in ["version", "binary"] {
        assert!(save["inputSchema"]["properties"][k]["description"].is_string(), "{k}: {save}");
    }
    tool(&mut s, "new_drawing", json!({"metric": true})).unwrap();
    tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [0, 0], "radius": 1}})).unwrap();
    for (version, code) in [(json!("2000"), "AC1015"), (json!(2010), "AC1024")] {
        for ext in ["dwg", "dxf"] {
            let path = dir.join(format!("v{}.{ext}", code)).to_string_lossy().into_owned();
            let r = tool(&mut s, "save", json!({"path": path, "version": version, "force": true})).unwrap();
            assert!(!r.contains("is not a parameter"), "{r}");
            let bytes = std::fs::read(&path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            let got = if ext == "dwg" {
                text.get(..6).unwrap_or_default().to_string()
            } else {
                text.lines().skip_while(|l| l.trim() != "$ACADVER").nth(2).unwrap_or_default().trim().to_string()
            };
            assert_eq!(got, code, "{ext} {version}");
        }
    }
    let path = dir.join("bin.dxf").to_string_lossy().into_owned();
    let r = tool(&mut s, "save", json!({"path": path, "binary": true, "force": true})).unwrap();
    assert!(!r.contains("is not a parameter"), "{r}");
    assert!(std::fs::read(&path).unwrap().starts_with(b"AutoCAD Binary DXF"));
    // A save to the drawing's own file keeps its version: a `version` there is refused, by name.
    let (_, e) = tool(&mut s, "save", json!({"version": "2000"})).unwrap_err();
    assert!(e.contains("`version`"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
