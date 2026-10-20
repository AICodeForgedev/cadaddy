//! Agent tools under load (Q211), through the MCP server as a client calls it: `execute_many`
//! with 10,000 commands (one undo step; stopOnError undoes all of it and names the failure),
//! `watch_events` (each event once, in order, a batch's changes coalesced), and `drawing_summary`
//! / `drawing_query` paging on a large drawing. Each step is timed against a budget and the times
//! are printed (`-- --nocapture`). CI uses 200,000 objects; a million by hand:
//!
//! ```text
//! AGENT_N=1000000 cargo test --release -p cadaddy-mcp --test agent_load -- --nocapture
//! ```
//!
//! The budgets are for CI machines (debug-optimized builds, shared cores).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use cadaddy_mcp::{Headless, Server};
use serde_json::{Value, json};

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

fn server() -> Server {
    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    // As a client does: the session is ready (events are sent from now on).
    s.handle_line(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string());
    s
}

/// A tool call's result (its JSON text parsed) or the error text.
fn tool(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    if let Some(e) = r.get("error") {
        return Err(e.to_string());
    }
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    let v = serde_json::from_str(&text).unwrap_or(json!(text));
    if r["result"]["isError"] == true { Err(text) } else { Ok(v) }
}

fn count(s: &mut Server) -> u64 {
    tool(s, "execute", json!({ "command": "drawing.inspect", "params": { "entities": false } })).unwrap()["entityCount"].as_u64().unwrap()
}

/// Time `f`, print it, and hold it to `budget`.
fn timed<T>(report: &mut Vec<String>, what: &str, budget: Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let took = t.elapsed();
    report.push(format!("{what}: {took:.2?} (budget {budget:.0?})"));
    assert!(took <= budget, "{what} took {took:?}, over its budget of {budget:?}");
    r
}

fn circles(n: usize, offset: usize) -> Vec<Value> {
    (0..n).map(|i| json!({ "command": "circle", "params": { "center": [(offset + i) as f64 * 3.0, 0.0], "radius": 1.0 } })).collect()
}

fn objects() -> usize {
    std::env::var("AGENT_N").ok().and_then(|n| n.parse().ok()).unwrap_or(200_000)
}

#[test]
fn ten_thousand_commands_are_one_undo_step_and_a_failure_undoes_all_of_them() {
    let mut report = Vec::new();
    let mut s = server();
    let before = count(&mut s);
    let r = timed(&mut report, "execute_many 10,000 circles", Duration::from_secs(10), || {
        tool(&mut s, "execute_many", json!({ "commands": circles(10_000, 0) })).unwrap()
    });
    assert_eq!(r["results"].as_array().map(Vec::len), Some(10_000), "{}", r.to_string().chars().take(300).collect::<String>());
    assert_eq!(count(&mut s), before + 10_000);
    // One undo takes all of them away.
    timed(&mut report, "undo of the batch", Duration::from_secs(1), || tool(&mut s, "execute", json!({ "command": "undo", "params": {} })).unwrap());
    assert_eq!(count(&mut s), before);
    // A failure at 7,000 with stopOnError: nothing stays, and the reply names it.
    let mut bad = circles(10_000, 0);
    bad[7_000] = json!({ "command": "circle", "params": { "center": [0, 0], "radius": -1 } });
    let r = timed(&mut report, "execute_many failing at 7,000", Duration::from_secs(10), || {
        tool(&mut s, "execute_many", json!({ "commands": bad, "stopOnError": true }))
    });
    let text = match r {
        Ok(v) => v.to_string(),
        Err(e) => e,
    };
    assert!(text.contains("7000") && text.contains("circle"), "names the failing command: {}", text.chars().take(400).collect::<String>());
    assert_eq!(count(&mut s), before, "everything the batch did is undone");
    // Without stopOnError the failure is that command's result and the rest run.
    let mut bad = circles(10_000, 0);
    bad[3] = json!({ "command": "nosuchcommand", "params": {} });
    let r = tool(&mut s, "execute_many", json!({ "commands": bad })).unwrap();
    assert!(r["results"][3].get("error").is_some(), "{}", r["results"][3]);
    assert_eq!(count(&mut s), before + 9_999);
    // Past the cap: refused before anything runs.
    let n = count(&mut s);
    assert!(tool(&mut s, "execute_many", json!({ "commands": circles(10_001, 0) })).is_err());
    assert_eq!(count(&mut s), n);
    eprintln!("{}", report.join("\n"));
}

/// The event notifications queued since the last call.
fn events(s: &mut Server) -> Vec<Value> {
    s.take_notifications()
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|n| n["method"] == "notifications/cadaddy/event")
        .map(|n| n["params"].clone())
        .collect()
}

#[test]
fn events_arrive_once_in_order_and_a_batch_is_one_change() {
    let mut s = server();
    tool(&mut s, "watch_events", json!({ "on": true })).unwrap();
    events(&mut s);
    let mut all: Vec<Value> = Vec::new();
    // A single command, a selection, a 1,000-command batch, a typed command with its prompts.
    let c = tool(&mut s, "execute", json!({ "command": "circle", "params": { "center": [0, 0], "radius": 1 } })).unwrap()["handle"].clone();
    all.extend(events(&mut s));
    tool(&mut s, "execute", json!({ "command": "select", "params": { "handles": [c] } })).unwrap();
    all.extend(events(&mut s));
    tool(&mut s, "execute_many", json!({ "commands": circles(1_000, 10) })).unwrap();
    let batch = events(&mut s);
    all.extend(batch.clone());
    for line in ["line", "0,0", "10,0", ""] {
        tool(&mut s, "command_line", json!({ "text": line })).unwrap();
        all.extend(events(&mut s));
    }
    // Nothing more arrives on a call that changes nothing.
    count(&mut s);
    all.extend(events(&mut s));
    // Once each, in order.
    let seqs: Vec<u64> = all.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "in order, no repeats: {seqs:?}");
    assert_eq!(seqs.iter().collect::<HashSet<_>>().len(), seqs.len());
    // The batch's thousand circles: one coalesced change.
    let changes: Vec<&Value> = batch.iter().filter(|e| e["type"] == "drawing.changed").collect();
    assert_eq!(changes.len(), 1, "{batch:?}");
    assert_eq!(changes[0]["counts"], json!([1000, 0, 0]), "{}", changes[0]);
    let kinds: Vec<&str> = all.iter().map(|e| e["type"].as_str().unwrap()).collect();
    for k in ["command.run", "selection.changed", "drawing.changed", "command.started", "command.ended"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    // Off: nothing more.
    tool(&mut s, "watch_events", json!({ "on": false })).unwrap();
    tool(&mut s, "execute", json!({ "command": "circle", "params": { "center": [0, 0], "radius": 2 } })).unwrap();
    assert!(events(&mut s).is_empty());
}

#[test]
fn summary_and_query_page_through_a_large_drawing_within_budgets() {
    let n = objects();
    let mut report = vec![format!("{n} objects")];
    let mut s = server();
    // What a new drawing holds already (its layouts' viewports).
    let base = tool(&mut s, "drawing_query", json!({ "limit": 1 })).unwrap()["count"].as_u64().unwrap();
    // Built as an agent would: 10,000 commands a call, a text every 50th object.
    timed(&mut report, "build", Duration::from_secs(n as u64 / 4_000 + 20), || {
        for b in 0..n / 10_000 {
            let cmds: Vec<Value> = (0..10_000)
                .map(|i| {
                    let k = b * 10_000 + i;
                    let (x, y) = ((k % 1000) as f64 * 10.0, (k / 1000) as f64 * 10.0);
                    if k % 50 == 0 {
                        json!({ "command": "text", "params": { "at": [x, y], "height": 2, "text": format!("T{k}") } })
                    } else {
                        json!({ "command": "line", "params": { "points": [[x, y], [x + 5.0, y + 5.0]] } })
                    }
                })
                .collect();
            tool(&mut s, "execute_many", json!({ "commands": cmds })).unwrap();
        }
    });
    let n = n / 10_000 * 10_000;
    // The summary: capped, its texts paged.
    let sum = timed(&mut report, "drawing_summary", Duration::from_secs(5), || tool(&mut s, "drawing_summary", json!({})).unwrap());
    let summary_len = sum.to_string().len();
    report.push(format!("summary size: {summary_len} bytes"));
    assert!(summary_len < 4 << 20, "the summary stays small: {summary_len} bytes");
    let texts = n / 50;
    let page = timed(&mut report, "drawing_summary texts, last page", Duration::from_secs(5), || {
        tool(&mut s, "drawing_summary", json!({ "sections": ["texts"], "textOffset": texts - 100, "textLimit": 5000 })).unwrap()
    });
    let got = page.pointer("/texts/items").or_else(|| page.pointer("/texts/texts")).or_else(|| page.get("texts")).cloned().unwrap_or(Value::Null);
    report.push(format!("texts page: {}", got.to_string().chars().take(120).collect::<String>()));
    // The query: every object counted, pages of 1,000, disjoint, the last one short.
    let first =
        timed(&mut report, "drawing_query first page", Duration::from_secs(2), || tool(&mut s, "drawing_query", json!({ "limit": 1000 })).unwrap());
    let total = first["count"].as_u64().unwrap();
    assert_eq!(total, base + n as u64, "every object counted");
    let handles = |v: &Value| -> Vec<String> { v["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect() };
    let mut seen: HashSet<String> = handles(&first).into_iter().collect();
    for k in 1..20 {
        let p = tool(&mut s, "drawing_query", json!({ "limit": 1000, "offset": k * 1000 })).unwrap();
        let h = handles(&p);
        assert_eq!(h.len(), 1000);
        assert!(h.iter().all(|x| seen.insert(x.clone())), "pages don't overlap");
    }
    let last = timed(&mut report, "drawing_query deepest page", Duration::from_secs(2), || {
        tool(&mut s, "drawing_query", json!({ "limit": 1000, "offset": total - 10 })).unwrap()
    });
    assert_eq!(handles(&last).len(), 10);
    let lines = timed(&mut report, "drawing_query by type and window", Duration::from_secs(2), || {
        tool(&mut s, "drawing_query", json!({ "type": "Text", "window": [[0, 0], [995, 995]], "limit": 100 })).unwrap()
    });
    assert_eq!(lines["count"].as_u64(), Some(200), "{}", lines.to_string().chars().take(200).collect::<String>());
    // A call while watching events: the poll after it compares the drawing with the last look.
    tool(&mut s, "watch_events", json!({ "on": true })).unwrap();
    timed(&mut report, "a small edit while watching events", Duration::from_secs(1), || {
        tool(&mut s, "execute", json!({ "command": "circle", "params": { "center": [0, 0], "radius": 1 } })).unwrap()
    });
    timed(&mut report, "a query while watching events", Duration::from_secs(1), || tool(&mut s, "drawing_query", json!({ "limit": 10 })).unwrap());
    eprintln!("{}", report.join("\n"));
}

/// Headless there is no one to ask (confirmations are the app's), but a dry run holds for a
/// batch as for single calls.
#[test]
fn a_batch_in_a_dry_run_changes_nothing() {
    let mut s = server();
    let before = count(&mut s);
    tool(&mut s, "execute", json!({ "command": "agent.dryrun", "params": { "on": true } })).unwrap();
    let r = tool(&mut s, "execute_many", json!({ "commands": circles(1_000, 0) })).unwrap();
    assert_eq!(r["results"][0]["dryRun"], true, "{}", r["results"][0]);
    tool(&mut s, "execute", json!({ "command": "agent.dryrun", "params": { "on": false } })).unwrap();
    assert_eq!(count(&mut s), before);
}
