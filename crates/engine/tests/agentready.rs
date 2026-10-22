//! Q319: the project as an agent with no memory needs it.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A two-level made-up building: walls, two tagged supply runs and `untagged` untagged ones per
/// level (their sizes and heights are guesses), a spec with both levels.
fn building(untagged: usize) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    s.execute("pline", &json!({"vertices": [[-12, -12], [2412, -12], [2412, 372], [-12, 372]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-6, -6], [2406, -6], [2406, 366], [-6, 366]], "closed": true})).unwrap();
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("line", &json!({"points": [[24, 60], [264, 60]]})).unwrap();
    text(&mut s, [140.0, 70.0], "24x12 BOT @ 10'-0\"");
    s.execute("line", &json!({"points": [[24, 200], [264, 200]]})).unwrap();
    text(&mut s, [140.0, 210.0], "18x10 BOT @ 10'-6\"");
    for k in 0..untagged {
        let y = 300.0 + 0.0 * k as f64;
        let x = 400.0 + 18.0 * k as f64;
        s.execute("line", &json!({"points": [[x, 40.0], [x, y]]})).unwrap();
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 3000, 1000], "base": [0, 0]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s
}

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn ids(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

fn err(r: cadaddy_engine::Result<Value>) -> String {
    match r {
        Ok(v) => panic!("expected a refusal, got {v}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn a_fresh_agent_works_the_queue_from_the_briefing_alone() {
    let mut s = building(3);
    // Nothing built: the briefing says what to call.
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert_eq!(b["next"]["call"], "bim.build {}");
    assert!(b.to_string().len() <= cadaddy_engine::cmd::agentready::BRIEFING_MAX);
    // Build; building again changes nothing.
    let first = s.execute("bim.build", &json!({})).unwrap();
    assert_eq!(first["unchanged"], false);
    let again = s.execute("bim.build", &json!({})).unwrap();
    assert_eq!((again["unchanged"].clone(), again["built"].clone()), (json!(true), first["built"].clone()));
    assert!(first["built"]["L1"]["mep"].as_u64().unwrap() >= 5, "{first}");
    // Errors that guide.
    assert!(err(s.execute("bim.build", &json!({"level": "L3"}))).contains("its levels are L1"));
    // The next item, how to answer it, and the documents to check.
    let n = s.execute("review.next", &json!({})).unwrap();
    let id = n["item"]["id"].as_str().unwrap().to_string();
    assert_eq!(n["item"]["needs_citation"], true);
    assert!(n["item"]["documents"].as_array().unwrap().len() >= 2 && n["item"]["candidates"].as_array().unwrap().len() >= 2, "{n}");
    // Code knowledge alone doesn't change a documented item.
    let e = err(s.execute("review.resolve", &json!({"id": id, "answer": {"size_in": [16, 10]}})));
    assert!(e.contains("citation") && e.contains("concern"), "{e}");
    let raised = s.execute("review.resolve", &json!({"id": id, "concern": "duct looks undersized for code velocity"})).unwrap();
    let concern = raised["raised"].as_str().unwrap().to_string();
    assert_eq!(raised["changed"], false);
    let before = s.execute("mep.list", &json!({})).unwrap();
    assert!(!before.to_string().contains("[16.0,10.0]"), "the model is unchanged");
    let open = s.execute("review.list", &json!({"limit": 50})).unwrap();
    assert!(ids(&open).contains(&concern), "the concern is in the queue");
    // A citation of a document: the run takes the answer and is reviewed.
    let cite = json!({"kind": "drawing", "document": "M-101", "ref": "tag at grid B-2"});
    let r = s.execute("review.resolve", &json!({"id": id, "answer": {"size_in": [16, 10], "bottom_ft": 10.5}, "citation": cite})).unwrap();
    assert_eq!(r["changed"], true);
    let after = s.execute("mep.list", &json!({})).unwrap();
    assert!(after["items"].as_array().unwrap().iter().any(|i| i["size_in"] == json!([16.0, 10.0]) && i["reviewed"] == true), "{after}");
    // Asking again does nothing (the item is gone from the open queue; the decision is kept).
    let open = s.execute("review.list", &json!({"limit": 50})).unwrap();
    assert!(!ids(&open).contains(&id));
    // Defer one, draft an RFI from another; the RFI then is a review item of its own.
    let rest = ids(&s.execute("review.list", &json!({"kind": "mep_guess"})).unwrap());
    let d = s.execute("review.defer", &json!({"id": rest[0], "reason": "field to measure the beam depth"})).unwrap();
    assert_eq!(d["changed"], true);
    assert_eq!(s.execute("review.defer", &json!({"id": rest[0], "reason": "again"})).unwrap()["changed"], false);
    let q = s.execute("review.rfi_draft", &json!({"id": rest[1], "question": "Confirm the duct size and bottom elevation"})).unwrap();
    assert_eq!(q["changed"], true);
    assert_eq!(
        s.execute("review.rfi_draft", &json!({"id": rest[1], "question": "Confirm the duct size and bottom elevation"})).unwrap()["changed"],
        false,
        "idempotent"
    );
    // The drafted RFI is on the list, sent: it waits for the design team (not back in the queue).
    let sent = s.execute("rfi.list", &json!({"status": "sent"})).unwrap();
    assert!(sent["total"].as_u64().unwrap() >= 1 && sent.to_string().contains("Confirm the duct size"), "{sent}");
    assert!(ids(&s.execute("review.list", &json!({"kind": "rfi"})).unwrap()).is_empty());
    // Handoff note; the next agent sees everything in the briefing.
    s.execute("project.note", &json!({"text": "Ducts on L1 reviewed to grid C; the beam depth is with the field."})).unwrap();
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert!(b["revision"].as_u64().unwrap() >= 6, "{b}");
    assert!(b["notes"][0]["text"].as_str().unwrap().contains("grid C"));
    assert_eq!(b["review"]["deferred"], 1);
    assert!(b["recent"].as_array().unwrap().iter().any(|e| e["action"] == "resolve"));
    assert!(b["text"].as_str().unwrap().contains("Next:"));
    // A person approves what agents did.
    let log = s.execute("review.log", &json!({"author": "agent", "approved": false})).unwrap();
    assert!(log["total"].as_u64().unwrap() >= 4);
    let ap = s.execute("review.approve", &json!({"all": true})).unwrap();
    assert!(ap["approved"].as_u64().unwrap() >= 3);
    assert_eq!(s.execute("review.log", &json!({"author": "agent", "approved": false})).unwrap()["total"], 0);
    // Undo takes the approval back.
    s.execute("undo", &json!({})).unwrap();
    assert!(s.execute("review.log", &json!({"author": "agent", "approved": false})).unwrap()["total"].as_u64().unwrap() >= 4);
}

#[test]
fn batch_needs_the_previews_token() {
    let mut s = building(6);
    s.execute("bim.build", &json!({})).unwrap();
    let cite = json!({"kind": "schedule", "document": "M-601 duct schedule"});
    let p = json!({"kind": "mep_guess", "system": "M-DUCT-SUPP", "answer": {"size_in": [10, 8]}, "citation": cite});
    let preview = s.execute("review.batch", &p).unwrap();
    let count = preview["preview"]["count"].as_u64().unwrap();
    assert!(count >= 6, "{preview}");
    let token = preview["token"].as_str().unwrap().to_string();
    // Without a citation a batch of documented items is refused.
    let mut bare = p.clone();
    bare.as_object_mut().unwrap().remove("citation");
    assert!(err(s.execute("review.batch", &bare)).contains("citation"));

    let mut wrong = p.clone();
    wrong["confirm"] = json!("B00000000");
    assert!(err(s.execute("review.batch", &wrong)).contains("token"));
    let mut ok = p.clone();
    ok["confirm"] = json!(token);
    let applied = s.execute("review.batch", &ok).unwrap();
    assert_eq!(applied["applied"].as_u64(), Some(count));
    // The same confirm again: the queue changed, so the token no longer matches.
    assert!(err(s.execute("review.batch", &ok)).contains("token"));
}

#[test]
fn answers_are_capped_and_paged() {
    let mut s = building(120);
    s.execute("bim.build", &json!({})).unwrap();
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert!(b.to_string().len() <= cadaddy_engine::cmd::agentready::BRIEFING_MAX, "{}", b.to_string().len());
    let mut cursor = Value::Null;
    let mut seen = Vec::new();
    loop {
        let mut q = json!({"limit": 50});
        if !cursor.is_null() {
            q["cursor"] = cursor.clone();
        }
        let page = s.execute("review.list", &q).unwrap();
        assert!(page.to_string().len() <= cadaddy_engine::cmd::agentready::ANSWER_MAX);
        assert!(page["items"].as_array().unwrap().len() <= 50);
        seen.extend(ids(&page));
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert!(seen.len() >= 120, "every item paged through: {}", seen.len());
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), seen.len(), "stable, distinct ids");
}

#[test]
fn the_agent_state_is_kept_in_the_file() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-agentready-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for ext in ["cdy", "dxf"] {
        let mut s = building(2);
        s.execute("bim.build", &json!({})).unwrap();
        let id = ids(&s.execute("review.list", &json!({})).unwrap()).remove(0);
        s.execute("review.defer", &json!({"id": id, "reason": "field"})).unwrap();
        s.execute("project.note", &json!({"text": format!("saved as {ext}")})).unwrap();
        let rev = s.execute("project.briefing", &json!({})).unwrap()["revision"].clone();
        let path = dir.join(format!("agent.{ext}"));
        s.execute("saveas", &json!({"path": path.display().to_string()})).unwrap();
        let mut t = Session::new();
        t.execute("open", &json!({"path": path.display().to_string()})).unwrap();
        let b = t.execute("project.briefing", &json!({})).unwrap();
        assert_eq!(b["revision"], rev, "{ext}");
        assert_eq!(b["notes"][0]["text"], format!("saved as {ext}"));
        assert_eq!(b["review"]["deferred"], 1, "{ext}: {b}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_params_are_refusals_not_crashes() {
    let mut s = building(2);
    let bad = [
        ("review.list", json!({"limit": -5, "cursor": "zz"})),
        ("review.list", json!({"limit": 1e12, "cursor": 99999999999u64})),
        ("review.list", json!({"kind": "everything"})),
        ("review.next", json!({"kind": 7})),
        ("review.resolve", json!({})),
        ("review.resolve", json!({"id": 123})),
        ("review.resolve", json!({"id": "RV-nope", "answer": "x"})),
        ("review.resolve", json!({"id": "RV-nope", "citation": "a string"})),
        ("review.defer", json!({"id": null})),
        ("review.rfi_draft", json!({"id": "RV-nope", "question": ""})),
        ("review.batch", json!({"kind": "mep_guess", "answer": "x".repeat(100_000), "citation": {"kind": "blog", "document": "x"}})),
        ("review.batch", json!({"answer": 1})),
        ("review.approve", json!({"ids": "all"})),
        ("review.log", json!({"limit": "ten"})),
        ("project.note", json!({"text": "y".repeat(1_000_000), "author": "root"})),
        ("project.note", json!({"text": ""})),
        ("bim.build", json!({"level": "x".repeat(10_000)})),
        ("bim.build", json!({"discipline": "plumbing"})),
    ];
    for (cmd, p) in bad {
        let _ = s.execute(cmd, &p);
    }
    // Long text is kept short.
    s.execute("project.note", &json!({"text": "z".repeat(50_000)})).unwrap();
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert!(b.to_string().len() <= cadaddy_engine::cmd::agentready::BRIEFING_MAX);
    // A citation's kind must be a document.
    let id = ids(&s.execute("review.list", &json!({})).unwrap()).remove(0);
    let e = err(s.execute("review.resolve", &json!({"id": id, "answer": {"size_in": [8, 8]}, "citation": {"kind": "code", "document": "IMC 603"}})));
    assert!(e.contains("documents outrank code knowledge"), "{e}");
}

/// Q320's weak agent found these; each is fixed and kept fixed here.
#[test]
fn what_the_weak_agent_tripped_on() {
    let mut s = building(4);
    s.execute("bim.build", &json!({})).unwrap();
    let start = ids(&s.execute("review.list", &json!({})).unwrap());
    // 1. An RFI the agent drafted waits for the design team: it doesn't come back as an open item.
    s.execute("review.rfi_draft", &json!({"id": start[0], "question": "Confirm the size"})).unwrap();
    let open = s.execute("review.list", &json!({"limit": 50})).unwrap();
    assert!(!open["items"].as_array().unwrap().iter().any(|i| i["kind"] == "rfi"), "{open}");
    let sent = s.execute("rfi.list", &json!({"status": "sent"})).unwrap();
    assert_eq!(sent["total"], 1, "{sent}");
    // 2. The same question for a second item with the same title settles that item too.
    let titles: Vec<(String, String)> =
        open["items"].as_array().unwrap().iter().map(|i| (i["id"].as_str().unwrap().into(), i["title"].as_str().unwrap().into())).collect();
    let first_title = s.execute("review.list", &json!({"status": "all", "limit": 50})).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(start[0]))
        .unwrap()["title"]
        .clone();
    if let Some((twin, _)) = titles.iter().find(|(_, t)| json!(t) == first_title) {
        let r = s.execute("review.rfi_draft", &json!({"id": twin, "question": "Confirm the size"})).unwrap();
        assert_eq!((r["changed"].clone(), r["same_as_existing"].clone()), (json!(true), json!(true)));
        let again = s.execute("review.list", &json!({"limit": 50})).unwrap();
        assert!(!ids(&again).contains(twin), "the twin left the queue");
    }
    // 3. Answers change the spec: the briefing asks for a build before the takeoff.
    for id in ids(&s.execute("review.list", &json!({"limit": 50})).unwrap()) {
        s.execute("review.resolve", &json!({"id": id, "answer": {"size_in": [10, 8]}, "citation": {"kind": "schedule", "document": "M-601"}}))
            .unwrap();
    }
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert_eq!(b["review"]["open"], 0, "{b}");
    assert_eq!(b["next"]["call"], "bim.build {}", "{b}");
    assert!(b["next"]["why"].as_str().unwrap().contains("changed since the last build"));
    s.execute("bim.build", &json!({})).unwrap();
    let b = s.execute("project.briefing", &json!({})).unwrap();
    assert!(b["next"]["call"].as_str().unwrap().starts_with("takeoff"), "{b}");
}
