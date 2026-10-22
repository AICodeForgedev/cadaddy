//! Q319 on the synthetic hotel: its review queue (guessed runs, clashes) stays within the answer caps, and
//! a scripted agent clears what the documents settle (batch answers citing the schedule), leaving
//! the rest for a person — with the counts as metrics.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::agentready::{ANSWER_MAX, BRIEFING_MAX};
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

/// The hotel's runs as a building spec: every horizontal duct and pipe run of the truth, half of
/// them at their system's default size (as if the reader found no tag): the review queue.
fn hotel_session() -> Session {
    let t = cadaddy_mepfix::hotel_truth_for(2, HotelSize::Full);
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .map(|l| json!({"name": format!("L{}", l.code), "elevation_ft": l.elevation_ft, "height_ft": 10, "ceiling_ft": 9, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]}))
        .collect();
    let mut systems: Vec<Value> = Vec::new();
    let mut runs = Vec::new();
    for (k, s) in t.segments.iter().filter(|s| !s.vertical && (s.kind == "duct" || s.kind == "pipe")).enumerate() {
        let name = format!("{}-{}", if s.kind == "duct" { "M-DUCT" } else { "P-PIPE" }, s.system);
        let default = if s.kind == "duct" { [12.0, 12.0] } else { [2.0, 0.0] };
        if !systems.iter().any(|x| x["name"] == json!(name)) {
            systems.push(json!({"name": name, "layer": name, "kind": s.kind, "size_in": default, "bottom_ft": 9}));
        }
        let size = if k % 2 == 0 {
            default
        } else if s.kind == "duct" {
            s.size_in
        } else {
            [s.size_in[0], 0.0]
        };
        let h = s.height.as_ref().map_or(9.0, |h| h.bottom_ft);
        let [a, b] = s.path_ft;
        runs.push(json!({"system": name, "level": format!("L{}", s.level), "points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]], "size_in": size, "bottoms_ft": [if k % 2 == 0 { 9.0 } else { h }], "objects": [format!("{:X}", 0x1000 + k)]}));
    }
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}}})).unwrap();
    s
}

#[test]
fn the_hotels_queue_fits_the_caps_and_an_agent_clears_what_documents_settle() {
    let mut s = hotel_session();
    s.execute("bim.build", &json!({})).unwrap();
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert!(b.to_string().len() <= BRIEFING_MAX, "briefing {} chars", b.to_string().len());
    let open = b["review"]["open"].as_u64().unwrap();
    // The hotel's guessed runs and its clashes (the engine's own tests page a larger queue).
    assert!(open >= 20, "a real queue: {b}");
    let first = s.execute("review.list", &json!({"limit": 10})).unwrap();
    assert!(first.to_string().len() <= ANSWER_MAX && first["next_cursor"].is_string());
    let next = s.execute("review.next", &json!({})).unwrap();
    assert!(next.to_string().len() <= ANSWER_MAX);
    // The scripted agent: the duct systems' sizes are in the schedules (cite them, batch per
    // system); the piping is left for a person.
    let mut systems: Vec<String> = Vec::new();
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
                && !systems.contains(&sys.to_string())
            {
                systems.push(sys.to_string());
            }
        }
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    let mut auto = 0;
    for sys in &systems {
        let p = json!({"kind": "mep_guess", "system": sys, "answer": {"size_in": [14, 10]}, "citation": {"kind": "schedule", "document": "M-601 duct schedule", "ref": sys}});
        let pre = s.execute("review.batch", &p).unwrap();
        let mut ok = p.clone();
        ok["confirm"] = pre["token"].clone();
        auto += s.execute("review.batch", &ok).unwrap()["applied"].as_u64().unwrap();
    }
    let after = s.execute("project.briefing", &json!({})).unwrap();
    let left = after["review"]["open"].as_u64().unwrap();
    println!("hotel queue: {open} open; the agent cleared {auto} citing the schedules; {left} left for a person");
    assert!(auto > 0 && left < open && left + auto == open, "{open} {auto} {left}");
    // Everything it did waits for a person's approval, and fits the cap.
    let log = s.execute("review.log", &json!({"author": "agent", "approved": false, "limit": 50})).unwrap();
    assert!(log["total"].as_u64().unwrap() >= auto && log.to_string().len() <= ANSWER_MAX);
    assert!(after.to_string().len() <= BRIEFING_MAX);
}
