//! Security regression suite (Q207), from outside: the real `cadaddy-cli mcp` binary driven over
//! stdio as an MCP client would. Oversized, malformed and hostile requests are answered with errors
//! and the server keeps serving; LISP and scripts outside the trusted folders don't run; links to
//! programs, to other computers and to escaped or padded program names ask first, other schemes
//! never open; a drawing's xref on another computer isn't opened. The control port, the agent
//! rails and the file-reader fuzzing are proved in their own crates (see the Q207 report).

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;

/// Send `input` (whole lines) to a fresh `cadaddy-cli mcp`, close stdin and collect the replies
/// by id (batches flattened; id-less errors under "null").
fn session_raw(input: &[u8]) -> (HashMap<String, Value>, std::process::ExitStatus) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cadaddy-cli"))
        .arg("mcp")
        .env("CADADDY_CONFIG_DIR", scratch("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cadaddy-cli");
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let out = child.wait_with_output().expect("wait");
    writer.join().ok();
    let mut by_id = HashMap::new();
    let mut nulls = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let items = if let Value::Array(a) = v { a } else { vec![v] };
        for r in items {
            match &r["id"] {
                Value::Null => nulls.push(r),
                id => {
                    by_id.insert(id.to_string(), r);
                }
            }
        }
    }
    by_id.insert("null".into(), Value::Array(nulls));
    (by_id, out.status)
}

fn session(lines: &[String]) -> HashMap<String, Value> {
    let mut input = format!("{INIT}\n");
    for l in lines {
        input.push_str(l);
        input.push('\n');
    }
    session_raw(input.as_bytes()).0
}

fn exec(id: u32, command: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "execute", "arguments": {"command": command, "params": params}}})
        .to_string()
}

/// A tool call's result (the JSON in its text content) and whether it is an error.
fn tool(r: &HashMap<String, Value>, id: u32) -> (Value, bool) {
    let v = &r[&id.to_string()]["result"];
    let text = v["content"][0]["text"].as_str().unwrap_or_default();
    (serde_json::from_str(text).unwrap_or_else(|_| json!(text)), v["isError"] == true)
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-sec078-cli-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ping(id: u32) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "ping"}).to_string()
}

#[test]
fn malformed_and_oversized_requests_are_refused_and_the_server_keeps_serving() {
    let mut input = format!("{INIT}\n").into_bytes();
    // Not JSON; not UTF-8; JSON nested far past any sane request; a number, not a request.
    input.extend_from_slice(b"{not json\n\xff\xfe\n");
    input.extend_from_slice(&b"[".repeat(100_000));
    input.extend_from_slice(b"\n42\n");
    // More than 32 MiB on one line.
    let big = format!("{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\",\"pad\":\"{}\"}}\n", "x".repeat(33 << 20));
    input.extend_from_slice(big.as_bytes());
    // A batch of 1001 requests.
    let batch: Vec<Value> = (0..1001).map(|i| json!({"jsonrpc": "2.0", "id": 1000 + i, "method": "ping"})).collect();
    input.extend_from_slice(format!("{}\n", Value::Array(batch)).as_bytes());
    input.extend_from_slice(format!("{}\n", ping(7)).as_bytes());
    let (r, status) = session_raw(&input);
    assert!(status.success(), "the server exits normally at the end of input");
    let errors: Vec<(i64, String)> = r["null"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["error"]["code"].as_i64().unwrap_or_default(), e["error"]["message"].as_str().unwrap_or_default().to_string()))
        .collect();
    let codes: Vec<i64> = errors.iter().map(|e| e.0).collect();
    assert_eq!(codes, [-32700, -32700, -32700, -32600, -32600, -32600], "{errors:?}");
    assert!(errors[4].1.contains("request too large"), "{errors:?}");
    assert!(errors[5].1.contains("batch too large"), "{errors:?}");
    assert!(!r.contains_key("9") && !r.contains_key("1000"), "neither the oversized request nor the batch ran");
    assert!(r["7"]["error"].is_null(), "and the next request is served: {:?}", r.get("7"));
}

#[test]
fn lisp_and_scripts_outside_trusted_folders_do_not_run() {
    let dir = scratch("untrusted");
    let lsp = dir.join("tool.lsp");
    std::fs::write(&lsp, "(setq untrusted-marker 42)").unwrap();
    let scr = dir.join("run.scr");
    std::fs::write(&scr, "circle 0,0 5\n").unwrap();
    let r = session(&[
        exec(8, "drawing.inspect", json!({ "entities": false })),
        exec(1, "appload", json!({ "path": lsp })),
        exec(2, "lisp", json!({ "code": format!("(load \"{}\")", lsp.display()) })),
        exec(3, "script", json!({ "path": scr })),
        exec(4, "lisp", json!({ "code": "untrusted-marker" })),
        exec(5, "drawing.inspect", json!({ "entities": false })),
        // LISP can't loosen the rules for the next call.
        exec(6, "lisp", json!({ "code": "(setvar \"SECURELOAD\" 0)" })),
        exec(7, "appload", json!({ "path": lsp })),
    ]);
    for id in [1, 3, 7] {
        let (v, is_err) = tool(&r, id);
        assert!(is_err && v.to_string().contains("trusted folder"), "{id}: {v}");
    }
    let (v, _) = tool(&r, 2);
    assert!(v.to_string().contains("trusted folder"), "(load) is refused too: {v}");
    assert_eq!(tool(&r, 4).0["value"], "nil", "nothing ran");
    assert_eq!(tool(&r, 5).0["entityCount"], tool(&r, 8).0["entityCount"], "the script drew nothing");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn links_to_programs_and_other_computers_ask_first_and_other_schemes_never_open() {
    let cases: &[(&str, &str)] = &[
        ("https://example.com/spec", "open"),
        ("/srv/plans/spec.pdf", "open"),
        ("C:\\Temp\\setup.exe", "ask"),
        ("file:///C:/Temp/setup%2Eexe", "ask"),
        ("C:\\Temp\\setup.exe.", "ask"),
        ("/Users/x/Thing.app/", "ask"),
        ("\\\\192.0.2.1\\share\\spec.pdf", "ask"),
        ("file://192.0.2.1/share/spec.pdf", "ask"),
        ("javascript:alert(1)", "refuse"),
        ("smb://192.0.2.1/share", "refuse"),
        ("ms-settings:privacy", "refuse"),
    ];
    let mut lines = vec![exec(1, "circle", json!({ "center": [0, 0], "radius": 1 }))];
    for (i, (url, _)) in cases.iter().enumerate() {
        let n = 10 + 2 * i as u32;
        lines.push(exec(n, "hyperlink", json!({ "handles": ["$LAST"], "url": url })));
        lines.push(exec(n + 1, "hyperlink.open", json!({ "handle": "$LAST" })));
    }
    // The circle's handle, known only after it is drawn: draw first, then link it.
    let first = session(&lines[..1]);
    let h = tool(&first, 1).0["handle"].as_str().unwrap().to_string();
    let lines: Vec<String> = lines.iter().map(|l| l.replace("$LAST", &h)).collect();
    let r = session(&lines);
    for (i, (url, want)) in cases.iter().enumerate() {
        let n = 10 + 2 * i as u32 + 1;
        let (v, is_err) = tool(&r, n);
        let got = if is_err {
            "refuse"
        } else if v.get("confirm").is_some_and(|c| !c.is_null()) {
            "ask"
        } else {
            "open"
        };
        assert_eq!(got, *want, "{url}: {v}");
    }
}

#[test]
fn an_xref_on_another_computer_is_not_opened() {
    let (parts, plans) = (scratch("xparts"), scratch("xplans"));
    let child = parts.join("child.dxf");
    let host = plans.join("host.dxf");
    let r = session(&[
        exec(1, "circle", json!({ "center": [0, 0], "radius": 1 })),
        exec(2, "saveas", json!({ "path": child })),
        exec(3, "new", json!({})),
        exec(4, "xattach", json!({ "path": child })),
        exec(5, "saveas", json!({ "path": host })),
    ]);
    assert!(!tool(&r, 5).1, "{:?}", tool(&r, 5));
    // The saved path pointed at another computer, as a hostile drawing would have it.
    let text = std::fs::read_to_string(&host).unwrap();
    let saved = child.display().to_string();
    assert!(text.contains(&saved), "the xref path is in the file");
    std::fs::write(&host, text.replace(&saved, "\\\\192.0.2.1\\share\\child.dxf")).unwrap();
    let r = session(&[exec(1, "open", json!({ "path": host })), exec(2, "xref", json!({}))]);
    let (v, _) = tool(&r, 2);
    let x = &v["xrefs"][0];
    assert_ne!(x["status"], "Loaded", "{v}");
    assert!(x["reason"].as_str().unwrap_or_default().contains("another computer"), "{v}");
    for d in [&parts, &plans] {
        std::fs::remove_dir_all(d).ok();
    }
}
