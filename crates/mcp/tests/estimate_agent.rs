//! Q327 + Q320: a scripted agent completes an estimate through the MCP tools only. It starts
//! from `bim_job briefing`, imports the company baseline, and works the validator's findings
//! until the estimate is clean: pricing unpriced lines and required equipment from its
//! "papers" (an invented quote sheet), deciding scope from the bid form, then drafts the bid
//! summary and exports the estimate. No model, no network: a deterministic stand-in for one.

use std::collections::BTreeMap;

use cadaddy_mcp::{Headless, Server};
use serde_json::{Value, json};

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

/// A tool call: its JSON result, or the error text.
fn call(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if r.get("error").is_some() || r["result"]["isError"] == true {
        return Err(if text.is_empty() { r["error"].to_string() } else { text });
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn exec(s: &mut Server, command: &str, params: Value) -> Result<Value, String> {
    call(s, "execute", json!({ "command": command, "params": params }))
}

/// Two floors of three duplex receptacles and a 100 ft branch conduit run each.
fn project(s: &mut Server) {
    let mut tags = Vec::new();
    let mut runs = Vec::new();
    let mut levels = Vec::new();
    for lv in 1..=2 {
        levels.push(json!({"name": format!("{lv}"), "elevation_ft": (lv - 1) * 10, "height_ft": 10}));
        for n in 0..3 {
            tags.push(json!({"id": format!("R{lv}{n}"), "kind": "receptacle", "level": format!("{lv}"), "at_ft": [5.0 * f64::from(n) + 2.0, 1.0], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "plan"}}}));
        }
        runs.push(
            json!({"system": "E-BRANCH-COND", "level": format!("{lv}"), "points": [[0, 0], [1200, 0]], "size_in": [0.75, 0], "bottoms_ft": [9]}),
        );
    }
    let spec = json!({
        "levels": levels,
        "mep": {"systems": [{"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9}], "runs": runs},
        "tags": tags
    });
    exec(s, "bspec.set", json!({ "replace": true, "spec": spec })).unwrap();
}

#[test]
fn a_scripted_agent_completes_the_estimate_through_mcp_only() {
    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    project(&mut s);
    // The company's baseline on disk: only its crew rate (the agent prices the rest from quotes).
    let dir = std::env::temp_dir().join(format!("cadaddy-estimate-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rates.json"), "{\"crew_rate\": 80.0}").unwrap();
    // The agent's papers: an invented supplier quote (material, labor hours) and an
    // equipment quote.
    let quote: BTreeMap<&str, (f64, f64)> = [
        ("Box, 4\" square", (2.5, 0.2)),
        ("Plaster ring", (1.0, 0.05)),
        ("Device plate", (0.75, 0.05)),
        ("standard duplex receptacle", (2.0, 0.3)),
        ("EMT conduit", (1.2, 0.04)),
        ("EMT coupling", (0.6, 0.03)),
        ("EMT connector", (0.8, 0.05)),
        ("Conduit strap", (0.3, 0.02)),
    ]
    .into_iter()
    .collect();
    let equipment: BTreeMap<&str, f64> = [
        ("Carbide hole saw for 3/4\" conduit", 38.0),
        ("Conduit bender 3/4\"", 60.0),
        ("Knockout set", 90.0),
        ("Label printer and tape", 120.0),
        ("Gang box", 90.0),
        ("Step ladder, 8'", 30.0),
        ("Temporary power", 400.0),
    ]
    .into_iter()
    .collect();

    // Start from the briefing: no estimate yet.
    let b = call(&mut s, "bim_job", json!({ "action": "briefing" })).unwrap();
    assert!(b.get("estimate").is_none(), "{b}");
    exec(&mut s, "estimate.import", json!({ "folder": dir.to_string_lossy() })).unwrap();
    exec(&mut s, "estimate.settings", json!({ "today": "2026-10-21", "tax_pct": 8, "overhead_pct": 10, "profit_pct": 10 })).unwrap();
    let mut calls = 0;
    let mut ok = false;
    for _round in 0..10 {
        let st = exec(&mut s, "estimate.status", json!({})).unwrap();
        if st["ok"] == json!(true) {
            ok = true;
            break;
        }
        // Lines to price: their unit from the query.
        let q = exec(&mut s, "estimate.query", json!({ "status": "unpriced", "limit": 200 })).unwrap();
        let units: BTreeMap<String, String> = q["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (l["key"].as_str().unwrap().to_string(), l["unit"].as_str().unwrap().to_string()))
            .collect();
        for f in st["findings"].as_array().unwrap() {
            let key = f["line"].as_str().unwrap_or_default();
            let parts: Vec<&str> = key.split('|').collect();
            let (item, size) = (parts.get(2).copied().unwrap_or_default(), parts.get(3).copied().unwrap_or_default());
            calls += 1;
            match f["kind"].as_str().unwrap() {
                "unpriced" => {
                    let (m, h) = quote[item];
                    let unit = &units[key];
                    exec(&mut s, "estimate.set_price", json!({ "item": item, "size": size, "unit": unit, "material": m, "labor_hours": h, "date": "2026-10-01", "source": "Invented Supply quote Q-1" })).unwrap();
                }
                "missing_equipment" => {
                    exec(&mut s, "estimate.set_price", json!({ "item": item, "unit": "each", "material": equipment[item], "labor_hours": 0, "date": "2026-10-01", "source": "Invented Rentals quote R-7" })).unwrap();
                }
                "undecided_scope" => {
                    exec(
                        &mut s,
                        "estimate.set_scope",
                        json!({ "lines": [key], "scope": "in_contract", "reason": "Division 26 is ours", "citation": "bid form" }),
                    )
                    .unwrap();
                }
                k => panic!("the agent has no plan for {k}: {f}"),
            }
        }
    }
    assert!(ok, "the agent did not finish in 10 rounds ({calls} fixes)");
    // Hand calculation (2 floors × 3 receptacles, 2 × 100 ft of 3/4" EMT):
    let st = exec(&mut s, "estimate.status", json!({})).unwrap();
    let t = &st["totals"];
    let q = exec(&mut s, "estimate.query", json!({ "limit": 200 })).unwrap();
    let line = |k: &str| q["lines"].as_array().unwrap().iter().find(|l| l["key"] == json!(k)).cloned().unwrap_or_else(|| panic!("no {k}: {q}"));
    // 200 ft + 10% waste = 220 ft × $1.20 = $264.00; 200 ft × 0.04 h = 8 h × $80 = $640.
    assert_eq!(line("electrical|conduit|EMT conduit|3/4\"")["cost"], json!(904.0));
    // 6 receptacles × ($2.00 + 0.3 h × $80) = $12 + $144 = $156.
    assert_eq!(line("electrical|devices|standard duplex receptacle|")["cost"], json!(156.0));
    // Equipment from the quote: gang boxes and ladders one per floor.
    assert_eq!(line("job|required|Gang box|")["cost"], json!(180.0));
    assert_eq!(line("job|required|Step ladder, 8'|")["cost"], json!(60.0));
    let direct: f64 = q["lines"].as_array().unwrap().iter().map(|l| l["cost"].as_f64().unwrap()).sum();
    let material: f64 = q["lines"].as_array().unwrap().iter().map(|l| l["material"].as_f64().unwrap()).sum();
    // Markups compound from the lines: tax 8% on material, overhead 10%, profit 10%.
    let expect = ((direct + material * 0.08) * 1.1 * 1.1 * 100.0).round() / 100.0;
    assert_eq!(t["total"].as_f64().unwrap(), expect, "{t}");
    // By hand: BOM material $323.00 (receptacles $12, boxes $15, rings $6, plates $4.50, conduit
    // $264, 19 couplings $11.40, 4 connectors $3.20, 23 straps $6.90) + quoted equipment $948.00
    // = $1,271.00; labor 12.78 h × $80 = $1,022.40; direct $2,293.40. Tax $101.68 → $2,395.08;
    // overhead $239.508 → $2,634.588; profit $263.4588 → $2,898.0468.
    assert_eq!(
        (
            t["material"].clone(),
            t["labor_hours"].clone(),
            t["direct"].clone(),
            t["tax"].clone(),
            t["overhead"].clone(),
            t["profit"].clone(),
            t["total"].clone()
        ),
        (json!(1271.0), json!(12.78), json!(2293.4), json!(101.68), json!(239.51), json!(263.46), json!(2898.05)),
        "{t}"
    );
    // The briefing now carries the estimate, clean.
    let b = call(&mut s, "bim_job", json!({ "action": "briefing" })).unwrap();
    assert_eq!(
        (b["estimate"]["ok"].clone(), b["estimate"]["total"].clone(), b["estimate"]["open_findings"].clone()),
        (json!(true), t["total"].clone(), json!(0)),
        "{b}"
    );
    // The human outputs, through MCP too.
    let bid = exec(&mut s, "estimate.bid_summary", json!({})).unwrap();
    assert_eq!((bid["total"].clone(), bid["open_findings"].clone()), (t["total"].clone(), json!(0)), "{bid}");
    let path = dir.join("agent-estimate.xlsx");
    let r = exec(&mut s, "estimate.export", json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!((r["lines"].clone(), r["total"].clone()), (json!(q["lines"].as_array().unwrap().len()), t["total"].clone()), "{r}");
    assert!(std::fs::metadata(&path).is_ok_and(|m| m.len() > 0));
    let _ = std::fs::remove_dir_all(&dir);
}
