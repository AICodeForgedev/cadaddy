//! The review list ranked by impact (Q6197) on a made-up job: two guest rooms and a corridor, a
//! receptacle on its wall and one 2 ft off it, a switch at the wrong height, a PTAC in one guest
//! room only (the room type's set asks one in each); two RFIs (one naming the receptacle, one
//! naming nothing), a job finding naming the switch and one naming nothing, an open question, a
//! panel CADaddy placed itself and a feeder it routed. The names, rooms and sheets are made up.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Mesh, MeshFace, RawTag, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// Block references at building points (feet), on `layer` with a TYPE attribute.
fn place(s: Session, block: &str, layer: &str, pts: &[[f64; 2]], ty: &str) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for p in pts {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: ty.into(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at(*p);
        let ins = Insert {
            block: block.into(),
            insert: Vec3::new(q[0], q[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs,
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = layer.into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "M-EQPM", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Guest Room", "number": "102"}, {"name": "Corridor", "number": "103"}],
        "devices": {"by_type": {
            "DUPLEX": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]},
            "SW": {"ceiling": false, "aff_ft": 3.0},
            "PTAC": {"ceiling": false, "aff_ft": 1.0}
        }},
        "sheets": [
            {"number": "X-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH]}
        ]
    })
}

/// The spec tag id the job gives a device at a building point.
fn tag_id(p: [f64; 2]) -> String {
    format!("device:L1:{:.1},{:.1}", p[0], p[1])
}

/// The device boxes the job builds from plan symbols: each made from its symbol (`src`) and
/// carrying its spec tag id.
fn built_devices(s: Session, made: &[(&str, [f64; 2])]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    for (src, p) in made {
        let q = at(*p);
        let vertices: Vec<Vec3> = [(q[0] - 2.0, q[1] - 1.0), (q[0] + 2.0, q[1] - 1.0), (q[0] + 2.0, q[1] + 1.0), (q[0] - 2.0, q[1] + 1.0)]
            .iter()
            .flat_map(|(x, y)| [Vec3::new(*x, *y, 18.0), Vec3::new(*x, *y, 22.0)])
            .collect();
        let faces = vec![MeshFace { v: vec![0, 2, 4, 6], hidden: 0 }, MeshFace { v: vec![1, 3, 5, 7], hidden: 0 }];
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, kind: Default::default() }));
        let tag = |v: &str| RawTag { code: 1000, value: v.into() };
        e.common.xdata =
            vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }, tag("device"), tag("L1"), tag(src), tag(&format!("devid:{}", tag_id(*p)))]
                .into();
        e.common.layer = "3D-Devices".into();
        d.model.push(e);
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

struct Job {
    s: Session,
    on_wall: String,
    off_wall: String,
    switch: String,
}

fn rule(s: &mut Session, domain: &str, key: &str, value: &str) {
    s.execute("project.rules.set", &json!({"domain": domain, "key": key, "value": value, "reason": "made-up test rule"})).unwrap();
}

/// The job: the plan, the model, the rules, then the RFIs, the job's findings and the proposed
/// work written as the build and the job write them.
fn job(run_size_in: f64) -> Job {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [100.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [99.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, 11.5]);
    line(&mut s, [30.0, 0.5], [30.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 6.0]), "101");
    text(&mut s, at([20.0, 6.0]), "102");
    text(&mut s, at([60.0, 6.0]), "103");
    let (s, recs) = place(s, "RECEP", "E-POWR", &[[10.0, 0.5], [6.0, 9.5], [27.0, 11.5]], "DUPLEX");
    let (s, sws) = place(s, "SWITCH", "E-POWR", &[[14.5, 6.0]], "SW");
    let (mut s, ptacs) = place(s, "PTACU", "M-EQPM", &[[18.0, 0.5]], "PTAC");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for (h, kind) in recs.iter().map(|h| (h, "receptacle")).chain([(&sws[0], "switch"), (&ptacs[0], "ptac")]) {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "X-101", "kind": kind})).unwrap();
    }
    let mut s = built_devices(
        s,
        &[(&recs[0], [10.0, 0.5]), (&recs[1], [6.0, 9.5]), (&recs[2], [27.0, 11.5]), (&sws[0], [14.5, 6.0]), (&ptacs[0], [18.0, 0.5])],
    );
    rule(&mut s, "custom", "prescreen.expect.guest room", "ptac=1; receptacle=1+");
    rule(&mut s, "mounting_height", "switch", "48");
    // What the build and the job leave: RFIs, a placed panel, a routed feeder, findings.
    let mut sp: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let rec_at = at([6.0, 9.5]);
    let device = |p: [f64; 2], kind: &str, ty: &str| json!({"id": tag_id(p), "kind": kind, "level": "L1", "at_ft": p, "fields": {"device_type": {"value": ty, "source": "legend"}}});
    sp["tags"] = json!([
        device([10.0, 0.5], "receptacle", "Duplex receptacle"),
        device([6.0, 9.5], "receptacle", "Duplex receptacle"),
        device([27.0, 11.5], "receptacle", "Duplex receptacle"),
        device([14.5, 6.0], "switch", "Single-pole switch"),
        device([18.0, 0.5], "ptac", "PTAC unit")
    ]);
    sp["rfis"] = json!([
        {"id": "RFI-901", "kind": "conflict", "severity": "high", "level": "L1", "at": rec_at, "sheets": ["X-101"],
         "about": "receptacle by the north wall", "issue": "the plan and the device schedule give two different receptacles here",
         "assumed": "the plan's", "status": "open", "topic": "receptacle type", "elements": [recs[1]]},
        {"id": "RFI-902", "kind": "ceiling_unspecified", "severity": "low", "level": "L1", "at": at([60.0, 6.0]), "sheets": ["X-101"],
         "about": "corridor ceiling", "issue": "no ceiling height is given for the corridor", "assumed": "9 ft", "status": "open"}
    ]);
    sp["mep"]["equipment"] = json!([
        {"tag": "XP-1", "kind": "panel", "level": "L1", "min": at([40.0, 1.0]), "max": at([41.5, 1.4]), "bottom_ft": 4.0, "height_ft": 2.0,
         "title": "PANEL XP-1", "sources": {"status": "proposed", "position": "proposed in room 103"}}
    ]);
    sp["mep"]["runs"] = json!([
        {"system": "XFEED", "level": "L1", "points": [at([40.0, 2.0]), at([40.0, 10.0]), at([70.0, 10.0])], "size_in": [run_size_in, run_size_in],
         "bottoms_ft": [8.5, 8.5], "height_source": "proposed", "size_source": "feeder schedule"}
    ]);
    s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let mut agent: Value = s.doc().unwrap().agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_else(|| json!({}));
    agent["guided_job"] = json!({
        "findings": [
            {"id": "F-devices-1", "stage": "devices", "description": "the switch's circuit is printed twice", "why": "two numbers", "severity": 4,
             "evidence": {"sheet": "X-101", "at": at([14.5, 6.0]), "handles": [sws[0]]}},
            {"id": "F-levels-2", "stage": "levels", "description": "2 level readings not used", "why": "order", "severity": 1}
        ],
        "worklist": [
            {"id": "F-devices-1", "kind": "finding", "stage": "devices", "summary": "the switch's circuit is printed twice", "status": "info"},
            {"id": "F-levels-2", "kind": "finding", "stage": "levels", "summary": "2 level readings not used", "status": "info"},
            {"id": "Q-classify-1", "kind": "question", "stage": "classify", "summary": "which level does X-101 show?", "status": "resolved"},
            {"id": "Q-rooms-2", "kind": "question", "stage": "rooms", "summary": "is 103 a corridor or a lobby?", "status": "pending"}
        ]
    });
    s.doc_mut().unwrap().agent = Some(agent.to_string());
    Job { s, on_wall: recs[0].clone(), off_wall: recs[1].clone(), switch: sws[0].clone() }
}

fn items(v: &Value) -> Vec<Value> {
    v["items"].as_array().cloned().unwrap_or_default()
}

fn list(s: &mut Session, p: Value) -> Value {
    let v = s.execute("review.impact", &p).unwrap();
    assert!(v.to_string().len() <= 16_000, "answer over the cap: {}", v.to_string().len());
    v
}

/// Every open item, page by page.
fn every(s: &mut Session, status: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cursor = json!(0);
    loop {
        let v = list(s, json!({"status": status, "limit": 50, "cursor": cursor}));
        out.extend(items(&v));
        match v.get("next_cursor") {
            Some(c) => cursor = c.clone(),
            None => break,
        }
    }
    out
}

fn by(all: &[Value], f: impl Fn(&Value) -> bool) -> Option<&Value> {
    all.iter().find(|i| f(i))
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

fn err(r: cadaddy_engine::Result<Value>) -> String {
    match r {
        Ok(v) => panic!("expected a refusal, got {v}"),
        Err(e) => e.to_string(),
    }
}

fn swing(i: &Value) -> Option<f64> {
    Some(i["range"]["low"].as_f64()?.abs().max(i["range"]["high"].as_f64()?.abs()))
}

fn handles(i: &Value) -> Vec<String> {
    i["open"]["element"]["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_ascii_uppercase).collect()
}

#[test]
fn every_source_is_one_list_ranked_by_what_it_can_move_the_estimate() {
    let mut j = job(0.75);
    let all = every(&mut j.s, "open");
    let count = |src: &str| all.iter().filter(|i| i["source"] == src).count();
    assert_eq!((count("finding"), count("question"), count("rfi"), count("proposed")), (2, 1, 2, 2), "{all:#?}");
    assert!(count("validate") >= 5, "{all:#?}");
    // A question the job already settled is not on the list.
    assert!(!all.iter().any(|i| i["title"].as_str().unwrap().contains("Q-classify-1")));
    // Sorted: the widest swing first, the unknown impacts after every priced one.
    let swings: Vec<Option<f64>> = all.iter().map(swing).collect();
    let first_unknown = swings.iter().position(Option::is_none).unwrap_or(swings.len());
    assert!(swings[first_unknown..].iter().all(Option::is_none), "unknowns last: {swings:?}");
    assert!(swings[..first_unknown].windows(2).all(|w| w[0] >= w[1]), "{swings:?}");
    for (n, i) in all.iter().enumerate() {
        assert_eq!(i["rank"], json!(n + 1));
    }
    // The receptacle off its wall: its own line and its box, ring and plate, half either way.
    let off = j.off_wall.to_ascii_uppercase();
    let host = by(&all, |i| i["rule"] == "G-HOST" && handles(i).contains(&off)).expect("G-HOST row");
    assert_eq!((host["effect"].as_str(), host["impact"]["known"].as_bool()), (Some("wrong"), Some(true)), "{host}");
    let lines: Vec<&str> = host["impact"]["lines"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(lines.iter().any(|l| l.starts_with("Duplex receptacle")) && lines.iter().any(|l| l.starts_with("Device plate")), "{host}");
    let dollars = num(&host["impact"]["dollars"]);
    assert!(dollars > 0.0 && num(&host["impact"]["labor_hours"]) > 0.0, "{host}");
    assert!((num(&host["range"]["low"]) + dollars / 2.0).abs() < 0.02 && (num(&host["range"]["high"]) - dollars / 2.0).abs() < 0.02, "{host}");
    assert_eq!(host["open"]["sheet"]["sheet"], "X-101", "{host}");
    // The receptacle on its wall is no G-HOST item.
    let on = j.on_wall.to_ascii_uppercase();
    assert!(!all.iter().any(|i| i["rule"] == "G-HOST" && handles(i).contains(&on)));
    // The PTAC room 101's type asks for: priced like the job's PTAC line, all up, nothing down.
    let ptac = by(&all, |i| i["rule"] == "G-PRESENT" && i["verdict"] == "missing").expect("G-PRESENT missing");
    assert_eq!(ptac["effect"], "missing");
    assert!(ptac["impact"]["basis"].as_str().unwrap().contains("priced like PTAC unit"), "{ptac}");
    assert_eq!(num(&ptac["range"]["low"]), 0.0);
    assert!((num(&ptac["range"]["high"]) - num(&ptac["impact"]["dollars"])).abs() < 0.02 && num(&ptac["impact"]["dollars"]) > dollars, "{ptac}");
    // The RFI naming the receptacle is priced by it; the one naming nothing says unknown.
    let rfi = |id: &str| by(&all, |i| i["rfi"] == id).unwrap_or_else(|| panic!("no {id}")).clone();
    let r1 = rfi("RFI-901");
    assert_eq!((r1["effect"].as_str(), r1["impact"]["known"].as_bool()), (Some("conflict"), Some(true)), "{r1}");
    let r2 = rfi("RFI-902");
    assert_eq!(
        (r2["impact"]["dollars"].as_str(), r2["range"]["high"].as_str(), r2["effect"].as_str()),
        (Some("unknown"), Some("unknown"), Some("gap")),
        "{r2}"
    );
    // The job's finding with the switch in its evidence is priced by the switch's built twin.
    let f1 = by(&all, |i| i["title"].as_str().unwrap().starts_with("F-devices-1")).unwrap();
    assert!(f1["impact"]["known"].as_bool().unwrap() && handles(f1) == vec![j.switch.to_ascii_uppercase()], "{f1}");
    let f2 = by(&all, |i| i["title"].as_str().unwrap().starts_with("F-levels-2")).unwrap();
    assert_eq!(f2["impact"]["dollars"], "unknown");
    // The feeder CADaddy routed: 38 ft, no takeoff line of its size, priced as the rules' run item.
    let run = by(&all, |i| i["source"] == "proposed" && i["rule"] == "XFEED").unwrap();
    assert!(run["title"].as_str().unwrap().contains("38 ft"), "{run}");
    assert!(run["impact"]["known"].as_bool().unwrap() && run["impact"]["lines"][0].as_str().unwrap().starts_with("EMT conduit 3/4\""), "{run}");
    assert_eq!(run["effect"], "proposed");
    // The panel it placed is on a line nothing prices: unknown, never zero.
    let panel = by(&all, |i| i["source"] == "proposed" && i["rule"] == "equipment").unwrap();
    assert_eq!(panel["impact"]["dollars"], "unknown", "{panel}");
    assert!(panel["impact"]["basis"].as_str().unwrap().contains("unpriced"), "{panel}");
    // The range: the priced lines moved by every open item; unknowns counted.
    let v = list(&mut j.s, json!({"limit": 1}));
    let e = &v["estimate"];
    assert!(num(&e["low"]) < num(&e["base"]) && num(&e["base"]) < num(&e["high"]), "{e}");
    assert!(
        num(&e["labor_hours"]["low"]) <= num(&e["labor_hours"]["base"]) && num(&e["labor_hours"]["base"]) <= num(&e["labor_hours"]["high"]),
        "{e}"
    );
    assert_eq!(e["unresolved"].as_u64().unwrap() as usize, all.len());
    assert_eq!(e["unknown_impact"].as_u64().unwrap() as usize, all.len() - first_unknown);
    assert_eq!(e["rate"]["dollars_per_hour"], json!(75.0));
    assert_eq!(v["effects"]["missing"]["high"], json!(1.0));
    assert!(v["next_cursor"].is_string());
    // Filters.
    let only = list(&mut j.s, json!({"source": "rfi"}));
    assert!(items(&only).iter().all(|i| i["source"] == "rfi") && only["total"] == json!(2), "{only}");
    let quick = list(&mut j.s, json!({"prescreen": false, "limit": 50}));
    assert!(items(&quick).iter().all(|i| i["source"] != "validate") && quick["note"].is_string(), "{quick}");
}

#[test]
fn a_run_of_a_size_nothing_prices_and_the_estimates_own_prices() {
    // A size no line or price book row has: unknown, said so.
    let mut j = job(7.3);
    let all = every(&mut j.s, "open");
    let run = by(&all, |i| i["source"] == "proposed" && i["rule"] == "XFEED").unwrap();
    assert_eq!(run["impact"]["dollars"], "unknown", "{run}");
    assert!(run["impact"]["basis"].as_str().unwrap().contains("no takeoff line or price of that size"), "{run}");
    // The estimate's own price for the receptacle and its crew rate: the receptacle's items move up.
    let off = j.off_wall.to_ascii_uppercase();
    let host = |all: &[Value]| {
        let n = all.iter().position(|i| i["rule"] == "G-HOST" && handles(i).contains(&off)).unwrap();
        (n, num(&all[n]["impact"]["dollars"]), num(&all[n]["impact"]["labor_dollars"]), num(&all[n]["impact"]["labor_hours"]))
    };
    let (rank0, d0, _, h0) = host(&all);
    j.s.execute(
        "estimate.set_price",
        &json!({"item": "Duplex receptacle", "unit": "each", "material": 900.0, "labor_hours": 0.5, "date": "2026-01-01", "source": "made-up quote"}),
    )
    .unwrap();
    j.s.execute("estimate.settings", &json!({"crew_rate": 100.0})).unwrap();
    let all = every(&mut j.s, "open");
    let (rank1, d1, l1, h1) = host(&all);
    assert!(d1 > d0 + 800.0 && rank1 < rank0, "rank {rank0}→{rank1}, ${d0}→${d1}");
    assert!((l1 - h1 * 100.0).abs() < 0.05 && h1 > 0.0 && (h1 - h0).abs() < 1.0, "{l1} {h1} {h0}");
    let v = list(&mut j.s, json!({"limit": 1}));
    assert_eq!(v["estimate"]["rate"]["dollars_per_hour"], json!(100.0));
}

#[test]
fn resolving_records_on_the_item_reranks_and_keeps_an_audit_trail() {
    let mut j = job(0.75);
    let before = list(&mut j.s, json!({"limit": 50}));
    let top = items(&before)[0].clone();
    let id = top["id"].as_str().unwrap().to_string();
    let r =
        j.s.execute("review.impact.resolve", &json!({"id": id, "action": "accept", "note": "the schedule settles it", "author": "user"})).unwrap();
    assert_eq!((r["changed"].as_bool(), r["status"].as_str(), r["estimate_moves"].as_bool()), (Some(true), Some("accept"), Some(false)), "{r}");
    // Again: nothing changes.
    let again = j.s.execute("review.impact.resolve", &json!({"id": id, "action": "accept"})).unwrap();
    assert_eq!(again["changed"], false);
    // Re-ranked: off the open list, out of the range, on the resolved list with its decision.
    let after = list(&mut j.s, json!({"limit": 50}));
    assert!(!items(&after).iter().any(|i| i["id"] == id.as_str()));
    assert_eq!(after["open"].as_u64().unwrap() + 1, before["open"].as_u64().unwrap());
    let (eb, ea) = (&before["estimate"], &after["estimate"]);
    assert_eq!(ea["unresolved"].as_u64().unwrap() + 1, eb["unresolved"].as_u64().unwrap());
    assert!(num(&ea["high"]) - num(&ea["low"]) < num(&eb["high"]) - num(&eb["low"]), "the range narrows: {eb} → {ea}");
    let done = list(&mut j.s, json!({"status": "resolved"}));
    let d = by(&items(&done), |i| i["id"] == id.as_str()).cloned().expect("on the resolved list");
    assert_eq!(
        (d["resolution"]["action"].as_str(), d["resolution"]["note"].as_str(), d["resolution"]["author"].as_str()),
        (Some("accept"), Some("the schedule settles it"), Some("user"))
    );
    // An RFI sent: sent on the RFI list too, still in the range until answered.
    let rfi = by(&items(&after), |i| i["rfi"] == "RFI-901").unwrap()["id"].as_str().unwrap().to_string();
    let sent = j.s.execute("review.impact.resolve", &json!({"id": rfi, "action": "rfi_sent"})).unwrap();
    assert_eq!((sent["estimate_moves"].as_bool(), sent["rfi"]["status"].as_str()), (Some(true), Some("sent")), "{sent}");
    let rl = j.s.execute("rfi.list", &json!({})).unwrap();
    assert!(rl["items"].as_array().unwrap().iter().any(|x| x["id"] == "RFI-901" && x["status"] == "sent"), "{rl}");
    let after2 = list(&mut j.s, json!({"limit": 50}));
    assert!(!items(&after2).iter().any(|i| i["id"] == rfi.as_str()));
    assert_eq!(after2["estimate"]["unresolved"], ea["unresolved"], "a sent RFI stays in the range");
    // rfi_sent on what isn't an RFI names the RFI sent.
    let other = items(&after2).iter().find(|i| i["rfi"].is_null()).unwrap()["id"].as_str().unwrap().to_string();
    assert!(err(j.s.execute("review.impact.resolve", &json!({"id": other, "action": "rfi_sent"}))).contains("note"));
    j.s.execute("review.impact.resolve", &json!({"id": other, "action": "change", "note": "moved the device"})).unwrap();
    // Reopened: back on the list; the trail keeps every step, newest first.
    j.s.execute("review.impact.resolve", &json!({"id": id, "action": "reopen", "note": "a second look"})).unwrap();
    let back = every(&mut j.s, "open");
    assert!(back.iter().any(|i| i["id"] == id.as_str()));
    let t = j.s.execute("review.impact.trail", &json!({})).unwrap();
    let acts: Vec<(&str, &str)> =
        t["entries"].as_array().unwrap().iter().map(|e| (e["action"].as_str().unwrap(), e["before"].as_str().unwrap())).collect();
    assert_eq!(acts, vec![("reopen", "accept"), ("change", "open"), ("rfi_sent", "open"), ("accept", "open")], "{t}");
    let first = &t["entries"][3];
    assert_eq!((first["id"].as_str(), first["author"].as_str()), (Some(id.as_str()), Some("user")));
    assert_eq!(first["dollars"], top["impact"]["dollars"]);
    let one = j.s.execute("review.impact.trail", &json!({"id": id})).unwrap();
    assert_eq!(one["total"], 2);
    // Kept in the drawing, undoable like any edit.
    j.s.execute("undo", &json!({})).unwrap();
    let t = j.s.execute("review.impact.trail", &json!({})).unwrap();
    assert_eq!(t["total"], 3, "undo takes the reopen back: {t}");
    assert!(!every(&mut j.s, "open").iter().any(|i| i["id"] == id.as_str()));
}

#[test]
fn an_item_opens_on_its_sheet_and_at_its_elements() {
    let mut j = job(0.75);
    let all = every(&mut j.s, "open");
    let off = j.off_wall.to_ascii_uppercase();
    let host = by(&all, |i| i["rule"] == "G-HOST" && handles(i).contains(&off)).unwrap();
    let id = host["id"].as_str().unwrap();
    let o = j.s.execute("review.impact.open", &json!({"id": id, "what": "sheet"})).unwrap();
    assert_eq!(o["sheet"], "X-101");
    let w: Vec<f64> = o["window"].as_array().unwrap().iter().map(num).collect();
    let p = at([6.0, 9.5]);
    assert!(w[0] < p[0] && p[0] < w[2] && w[1] < p[1] && p[1] < w[3], "{o}");
    let e = j.s.execute("review.impact.open", &json!({"id": id, "what": "element"})).unwrap();
    assert_eq!(e["handles"], json!([j.off_wall.to_ascii_uppercase()]), "{e}");
    assert_eq!(j.s.selection().iter().map(|h| h.hex().to_ascii_uppercase()).collect::<Vec<_>>(), vec![off.clone()]);
    // An RFI opens by its own window; one naming nothing has no element; a finding with no
    // evidence has no place.
    let r2 = by(&all, |i| i["rfi"] == "RFI-902").unwrap()["id"].as_str().unwrap().to_string();
    let z = j.s.execute("review.impact.open", &json!({"id": r2})).unwrap();
    assert!(z["window"].is_array(), "{z}");
    assert!(err(j.s.execute("review.impact.open", &json!({"id": r2, "what": "element"}))).contains("names no element"));
    let f2 = by(&all, |i| i["title"].as_str().unwrap().starts_with("F-levels-2")).unwrap()["id"].as_str().unwrap().to_string();
    assert!(err(j.s.execute("review.impact.open", &json!({"id": f2}))).contains("no place on a sheet"));
    // The proposed panel by its tag: the spec has no built box for it in this job.
    let panel = by(&all, |i| i["source"] == "proposed" && i["rule"] == "equipment").unwrap()["id"].as_str().unwrap().to_string();
    assert!(j.s.execute("review.impact.open", &json!({"id": panel, "what": "sheet"})).is_ok());
}

#[test]
fn bad_calls_are_refused_and_never_panic() {
    let mut j = job(0.75);
    let s = &mut j.s;
    for (cmd, p) in [
        ("review.impact", json!({"status": "maybe"})),
        ("review.impact", json!({"source": "rumour"})),
        ("review.impact.resolve", json!({"action": "accept"})),
        ("review.impact.resolve", json!({"id": "RV-J1", "action": "accept"})),
        ("review.impact.resolve", json!({"id": "RI-Vffffffff", "action": "accept"})),
        ("review.impact.resolve", json!({"id": "RI-Fffffffff", "action": "shrug"})),
        ("review.impact.resolve", json!({"id": "RI-Rffffffff", "action": "accept", "author": "boss"})),
        ("review.impact.open", json!({"id": "RI-Pffffffff", "what": "photo"})),
        ("review.impact.open", json!({"id": "RI-"})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
    for p in [json!({"limit": 0}), json!({"limit": 1e15}), json!({"cursor": "c99999999"}), json!({"cursor": -4}), json!({"level": "nowhere"})] {
        let v = s.execute("review.impact", &p).unwrap();
        assert!(v.to_string().len() <= 16_000, "{p}");
    }
    let id = items(&list(s, json!({})))[0]["id"].as_str().unwrap().to_string();
    assert!(s.execute("review.impact.open", &json!({"id": id, "what": "element", "member": 1e12})).is_err());
    let long = "x".repeat(50_000);
    let r = s.execute("review.impact.resolve", &json!({"id": id, "action": "change", "note": long})).unwrap();
    assert_eq!(r["changed"], true);
    let t = s.execute("review.impact.trail", &json!({"limit": 1e9, "cursor": 999_999})).unwrap();
    assert!(t["entries"].as_array().unwrap().is_empty() && t.to_string().len() <= 16_000);
    // No spec: refused, said so.
    let mut empty = Session::new();
    assert!(empty.execute("review.impact", &json!({})).is_err());
}
