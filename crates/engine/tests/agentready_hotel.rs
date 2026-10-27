//! Q319 on the synthetic hotel: its review queue (guessed runs, clashes) stays within the answer
//! caps, a scripted agent answers each guessed run with the truth's size and height (citing a
//! schedule), and the model is scored against the truth.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::agentready::{ANSWER_MAX, BRIEFING_MAX};
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

/// Ground truth for a guessed run: the correct size and height from the hotel's truth data.
struct RunTruth {
    size_in: [f64; 2],
    bottom_ft: f64,
}

/// Build the hotel session and the truth lookup keyed by the hex object handle.
///
/// Even-indexed segments (k % 2 == 0) get the system default (these are the guesses in the
/// queue); odd-indexed segments get their truth values.  The object handle for segment k is
/// `format!("{:X}", 0x1000 + k)`.
fn hotel_session_with_truth() -> (Session, Vec<(String, RunTruth)>) {
    let t = cadaddy_mepfix::hotel_truth_for(2, HotelSize::Full);
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .map(|l| {
            json!({"name": format!("L{}", l.code), "elevation_ft": l.elevation_ft, "height_ft": 10,
                    "ceiling_ft": 9, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]})
        })
        .collect();
    let mut systems: Vec<Value> = Vec::new();
    let mut runs = Vec::new();
    let mut truth_map = Vec::new();
    let horiz = t.segments.iter().filter(|s| !s.vertical && (s.kind == "duct" || s.kind == "pipe"));
    for (k, seg) in horiz.enumerate() {
        let name = format!("{}-{}", if seg.kind == "duct" { "M-DUCT" } else { "P-PIPE" }, seg.system);
        let default = if seg.kind == "duct" { [12.0, 12.0] } else { [2.0, 0.0] };
        if !systems.iter().any(|x| x["name"] == json!(name)) {
            systems.push(json!({"name": name, "layer": name, "kind": seg.kind, "size_in": default, "bottom_ft": 9}));
        }
        let size = if k % 2 == 0 { default } else { if seg.kind == "duct" { seg.size_in } else { [seg.size_in[0], 0.0] } };
        let h = seg.height.as_ref().map_or(9.0, |h| h.bottom_ft);
        let [a, b] = seg.path_ft;
        let handle = format!("{:X}", 0x1000 + k);
        runs.push(json!({"system": name, "level": format!("L{}", seg.level),
                          "points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]],
                          "size_in": size, "bottoms_ft": [if k % 2 == 0 { 9.0 } else { h }],
                          "objects": [&handle]}));
        // Truth for guessed (even-indexed) runs.
        if k % 2 == 0 {
            truth_map.push((handle, RunTruth { size_in: seg.size_in, bottom_ft: seg.height.as_ref().map_or(9.0, |h| h.bottom_ft) }));
        }
    }
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}}})).unwrap();
    (s, truth_map)
}

/// Look up the truth for a run by its object handles.
fn find_truth<'a>(objects: &[Value], truth: &'a [(String, RunTruth)]) -> Option<&'a RunTruth> {
    for obj in objects {
        if let Some(h) = obj.as_str()
            && let Some((_, t)) = truth.iter().find(|(handle, _)| handle == h)
        {
            return Some(t);
        }
    }
    None
}

#[test]
fn the_hotels_queue_fits_the_caps_and_an_agent_clears_what_documents_settle() {
    let (mut s, _truth) = hotel_session_with_truth();
    s.execute("bim.build", &json!({})).unwrap();
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert!(b.to_string().len() <= BRIEFING_MAX, "briefing {} chars", b.to_string().len());
    let open = b["review"]["open"].as_u64().unwrap();
    assert!(open >= 20, "a real queue: {b}");
    let first = s.execute("review.list", &json!({"limit": 10})).unwrap();
    assert!(first.to_string().len() <= ANSWER_MAX && first["next_cursor"].is_string());
    let next = s.execute("review.next", &json!({})).unwrap();
    assert!(next.to_string().len() <= ANSWER_MAX);
    // Batch-resolve duct guesses (the original test's path, still needed for the cap/token checks).
    let mut duct_systems: Vec<String> = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut q = json!({"kind": "mep_guess", "limit": 50});
        if !cursor.is_null() {
            q["cursor"] = cursor.clone();
        }
        let page = s.execute("review.list", &q).unwrap();
        for i in page["items"].as_array().unwrap() {
            let t = i["title"].as_str().unwrap();
            if let Some(sys) = t.split(' ').next().filter(|x| x.starts_with("M-DUCT"))
                && !duct_systems.contains(&sys.to_string())
            {
                duct_systems.push(sys.to_string());
            }
        }
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    let mut auto = 0;
    for sys in &duct_systems {
        let p = json!({"kind": "mep_guess", "system": sys, "answer": {"size_in": [14, 10]},
                        "citation": {"kind": "schedule", "document": "M-601 duct schedule", "ref": sys}});
        let pre = s.execute("review.batch", &p).unwrap();
        let mut ok = p.clone();
        ok["confirm"] = pre["token"].clone();
        auto += s.execute("review.batch", &ok).unwrap()["applied"].as_u64().unwrap();
    }
    let after = s.execute("project.briefing", &json!({})).unwrap();
    let left = after["review"]["open"].as_u64().unwrap();
    assert!(auto > 0 && left < open && left + auto == open, "{open} {auto} {left}");
    let log = s.execute("review.log", &json!({"author": "agent", "approved": false, "limit": 50})).unwrap();
    assert!(log["total"].as_u64().unwrap() >= auto && log.to_string().len() <= ANSWER_MAX);
    assert!(after.to_string().len() <= BRIEFING_MAX);
}

/// MUST 3: the scripted agent scores the hotel against the truth, with asserted metrics.
#[test]
fn the_hotel_agent_scores_against_truth() {
    let (mut s, truth) = hotel_session_with_truth();
    s.execute("bim.build", &json!({})).unwrap();
    let briefing = s.execute("project.briefing", &json!({})).unwrap();
    let _initial_open = briefing["review"]["open"].as_u64().unwrap();

    // The scripted agent walks the mep_guess queue, answering each with the truth's size and
    // height (citing a schedule), and defers what it can't find a truth entry for.
    let mut auto_resolved: u64 = 0;
    let mut deferred: u64 = 0;
    loop {
        let n = s.execute_agent("review.next", &json!({"kind": "mep_guess"})).unwrap();
        if n["item"].is_null() {
            break;
        }
        let item = &n["item"];
        let id = item["id"].as_str().unwrap().to_string();
        let objects = item["detail"]["objects"].as_array().map(|a| a.to_vec()).unwrap_or_default();
        let sys = item["detail"]["system"].as_str().unwrap_or("MEP");
        if let Some(t) = find_truth(&objects, &truth) {
            let cite = json!({"kind": "schedule", "document": format!("{} schedule", sys), "ref": "truth fixture"});
            s.execute_agent("review.resolve", &json!({"id": id, "answer": {"size_in": t.size_in, "bottom_ft": t.bottom_ft}, "citation": cite}))
                .unwrap();
            auto_resolved += 1;
        } else {
            s.execute_agent("review.defer", &json!({"id": id, "reason": "no truth entry for this run"})).unwrap();
            deferred += 1;
        }
    }

    // Defer or draft RFIs for the remaining non-mep_guess items (clashes, etc.).
    let mut left_for_person: u64 = 0;
    loop {
        let n = s.execute_agent("review.next", &json!({})).unwrap();
        if n["item"].is_null() {
            break;
        }
        let id = n["item"]["id"].as_str().unwrap().to_string();
        let kind = n["item"]["kind"].as_str().unwrap_or("unknown");
        if kind == "clash" {
            s.execute_agent("review.defer", &json!({"id": id, "reason": "coordination decision for a person"})).unwrap();
        } else {
            s.execute_agent("review.rfi_draft", &json!({"id": id, "question": format!("Clarify item {id}")})).unwrap();
        }
        left_for_person += 1;
    }

    // Verify the metrics as asserted numbers.
    // The queue may grow (new clashes from resolved runs), so the sum can exceed initial_open.
    assert!(auto_resolved > 0, "the agent resolved at least one run: got {auto_resolved}");
    // Nothing is left open: the queue is empty.
    let after = s.execute("project.briefing", &json!({})).unwrap();
    assert_eq!(after["review"]["open"], 0, "nothing left open: {auto_resolved} resolved, {deferred} deferred, {left_for_person} left for a person");

    // Score: every resolved run in the model must equal its truth size and height.
    let runs = s.execute("mep.list", &json!({})).unwrap();
    let mut wrong_edits: u64 = 0;
    let mut verified: u64 = 0;
    let tolerance_in = 0.01; // inches
    let tolerance_ft = 0.01; // feet
    for run in runs["items"].as_array().unwrap() {
        if run["reviewed"] != true {
            continue;
        }
        let objects = run["objects"].as_array().map(|a| a.to_vec()).unwrap_or_default();
        if let Some(t) = find_truth(&objects, &truth) {
            let sz = run["size_in"].as_array().unwrap();
            let w = sz[0].as_f64().unwrap_or(0.0);
            let h = sz[1].as_f64().unwrap_or(0.0);
            let bot = run["bottoms_ft"].as_array().and_then(|a| a.first()).and_then(Value::as_f64).unwrap_or(0.0);
            let size_ok = (w - t.size_in[0]).abs() < tolerance_in && (h - t.size_in[1]).abs() < tolerance_in;
            let height_ok = (bot - t.bottom_ft).abs() < tolerance_ft;
            if !size_ok || !height_ok {
                wrong_edits += 1;
            }
            verified += 1;
        }
    }
    assert_eq!(wrong_edits, 0, "no wrong edits: verified {verified} runs against truth");
    assert!(verified > 0, "at least one run verified against truth: {verified}");
    assert_eq!(verified, auto_resolved, "every auto-resolved run verified: {verified} vs {auto_resolved}");

    // The log carries citations for the resolved items.
    let log = s.execute("review.log", &json!({"author": "agent", "approved": false, "limit": 50})).unwrap();
    let resolve_entries: Vec<&Value> = log["entries"].as_array().unwrap().iter().filter(|e| e["action"] == "resolve").collect();
    assert!(resolve_entries.len() as u64 >= auto_resolved, "log has resolve entries: {} >= {auto_resolved}", resolve_entries.len());
    assert!(
        resolve_entries.iter().all(|e| e["citation"].as_str().is_some_and(|c| c.contains("schedule"))),
        "every resolve entry has a schedule citation"
    );
}
