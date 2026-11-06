//! G-PRESENT off the prints (Q6133) on a made-up job: two guest rooms of one unit type and an
//! office. The type's enlarged power plan (its own sheet, drawn once) shows two receptacles, a
//! switch and a TV outlet; room 101 lies as the view is drawn, room 102 is its mirror image. In room
//! 101 one receptacle was left out of the model and one added where the plan shows none; room
//! 102 has all of it. The office has no unit type: the level's power plan (a sheet registered
//! onto the floor plan) shows two receptacles and a switch there; the model left one receptacle
//! out and has a switch the plan doesn't show.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The plan's origin on the drawing (inches): the level's base.
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// The power plan's sheet, beside the floor plan; registered back onto it.
const E101: f64 = 8000.0;
/// The enlarged plans' sheet.
const E601: f64 = 16000.0;
/// The enlarged view's middle on its sheet.
const VC: [f64; 2] = [E601 + 1500.0, 1000.0];
/// The guest rooms' middles (feet).
const C101: [f64; 2] = [10.1, 7.0];
const C102: [f64; 2] = [30.0, 7.0];

/// The unit's layout as the enlarged view draws it: (tag, kind, feet off the view's middle).
const LAYOUT: [(&str, &str, [f64; 2]); 4] =
    [("REC", "receptacle", [-8.0, -6.2]), ("REC", "receptacle", [6.0, 6.2]), ("SW", "switch", [-9.3, 3.0]), ("TV", "tv_outlet", [8.0, -6.0])];

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
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Guest Room", "number": "102"}, {"name": "Office", "number": "103"}],
        "devices": {"by_type": {
            "REC": {"ceiling": false, "aff_ft": 1.5},
            "SW": {"ceiling": false, "aff_ft": 4.0},
            "TV": {"ceiling": false, "aff_ft": 1.5}
        }},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}},
            {"number": "E-101", "title": "POWER PLAN - LEVEL 1", "discipline": "E", "level": "L1", "source": "model", "rect": [E101, 0.0, E101 + SW, SH],
             "registration": {"reference": "A-101", "method": "tags", "a": 1.0, "b": 0.0, "tx": -E101, "ty": 0.0, "matched": 4}},
            {"number": "E-601", "title": "ENLARGED PLANS", "discipline": "E", "source": "model", "rect": [E601, 0.0, E601 + SW, SH]}
        ]
    })
}

/// A symbol of the enlarged view: its sheet point.
fn on_view(off: [f64; 2]) -> [f64; 2] {
    [VC[0] + off[0] * 12.0, VC[1] + off[1] * 12.0]
}

/// A symbol of the power plan at a building point: its sheet point.
fn on_power_plan(p: [f64; 2]) -> [f64; 2] {
    let q = at(p);
    [q[0] + E101, q[1]]
}

struct Job {
    s: Session,
    /// Room 101's added receptacle.
    added: String,
    /// The office's switch no plan shows.
    stray: String,
}

/// The guest room's plan point of a layout item: as drawn (`mirrored` false) or mirrored.
fn in_room(c: [f64; 2], off: [f64; 2], mirrored: bool) -> [f64; 2] {
    [c[0] + if mirrored { -off[0] } else { off[0] }, c[1] + off[1]]
}

fn job() -> Job {
    let mut s = Session::new();
    // A 60' × 14' shell of 6" walls; partitions at 20' and 40'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [60.0, 14.0]);
    rect(&mut s, [0.5, 0.5], [59.5, 13.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [20.0, 0.5], [20.0, 13.5]);
    line(&mut s, [40.0, 0.5], [40.0, 13.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([10.0, 9.0]), "101");
    text(&mut s, at([30.0, 9.0]), "102");
    text(&mut s, at([50.0, 9.0]), "103");
    layer(&mut s, "G-FRAME");
    for x in [0.0, E101, E601] {
        s.execute("pline", &json!({"vertices": [[x, 0.0], [x + SW, 0.0], [x + SW, SH], [x, SH]], "closed": true})).unwrap();
    }
    // The model. Room 101: the layout as drawn, less the north receptacle, plus one in the
    // middle of the north wall's run far from anything the view shows. Room 102: the mirrored
    // layout, all of it. The office: a receptacle and the switch the power plan shows (not its
    // second receptacle) and a switch it doesn't show.
    let (mut recs, mut sws, mut tvs) = (Vec::new(), Vec::new(), Vec::new());
    for (room, c, mirrored) in [("101", C101, false), ("102", C102, true)] {
        for (i, (tag, _, off)) in LAYOUT.iter().enumerate() {
            if room == "101" && i == 1 {
                continue;
            }
            let p = in_room(c, *off, mirrored);
            match *tag {
                "REC" => recs.push(p),
                "SW" => sws.push(p),
                _ => tvs.push(p),
            }
        }
    }
    recs.push([2.5, 13.3]); // added in 101: the view shows no receptacle there
    recs.push([45.0, 0.7]); // the office's first receptacle
    sws.push([40.3, 7.0]); // the office's switch
    sws.push([50.0, 13.3]); // a switch no plan shows
    let (s, rec_h) = place(s, "RECEP", "E-POWR", &recs, "REC");
    let (s, sw_h) = place(s, "SWITCH", "E-POWR", &sws, "SW");
    let (mut s, tv_h) = place(s, "TVOUT", "E-POWR", &tvs, "TV");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for (hs, kind) in [(&rec_h, "receptacle"), (&sw_h, "switch"), (&tv_h, "tv_outlet")] {
        for h in hs.iter() {
            s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "E-101", "kind": kind})).unwrap();
        }
    }
    // The unit type with its enlarged view, rooms 101 and 102 its instances (102 mirrored),
    // each laid in by the job's layout fit.
    let mut sp: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let view = [VC[0] - 10.75 * 12.0, VC[1] - 7.5 * 12.0, VC[0] + 10.75 * 12.0, VC[1] + 7.5 * 12.0];
    sp["unit_types"] = json!([{"id": "GUEST", "name": "GUEST ROOM",
        "views": [{"sheet": "E-601", "title": "ENLARGED GUEST ROOM POWER PLAN", "rect": view, "sheet_rect": [E601, 0.0, E601 + SW, SH]},
                  {"sheet": "E-601", "title": "GUEST ROOM - BED WALL", "rect": [E601 + 2600.0, 200.0, E601 + 3200.0, 600.0]}],
        "source": {"sheet": "E-601", "by": "made-up test"}}]);
    let fit =
        |mirrored: bool| json!({"rotation_deg": 0.0, "mirrored": mirrored, "view": "E-601 ENLARGED GUEST ROOM POWER PLAN", "by": "made-up test"});
    sp["unit_instances"] = json!([
        {"number": "101", "level": "L1", "type": "GUEST", "hand": "normal", "fit": fit(false)},
        {"number": "102", "level": "L1", "type": "GUEST", "hand": "mirrored", "fit": fit(true)}
    ]);
    s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    // What the job read: the enlarged view's symbols, and the power plan's in the office (plus
    // one in room 101, which its enlarged plan governs).
    let mut devices: Vec<Value> =
        LAYOUT.iter().map(|(tag, kind, off)| json!({"tag": tag, "kind": kind, "at": on_view(*off), "sheet": "E-601", "level": "L1"})).collect();
    for (tag, kind, p) in
        [("REC", "receptacle", [45.0, 0.7]), ("REC", "receptacle", [55.0, 13.3]), ("SW", "switch", [40.3, 7.0]), ("REC", "receptacle", [15.0, 10.0])]
    {
        devices.push(json!({"tag": tag, "kind": kind, "at": on_power_plan(p), "sheet": "E-101", "level": "L1"}));
    }
    set_job(&mut s, json!(devices));
    Job { s, added: rec_h[3].clone(), stray: sw_h[3].clone() }
}

/// The guided job's state as its devices stage leaves it: `devices` read off the sheets.
fn set_job(s: &mut Session, devices: Value) {
    let state = json!({"guided_job": {
        "id": "made-up-job", "goal": "bim", "created": "2026-01-01T00:00:00Z", "pdfs_path": "/made-up/prints",
        "stages": [], "rules": [], "audit": [], "question_seq": 0, "devices": devices
    }});
    s.doc_mut().unwrap().agent = Some(state.to_string());
}

fn present(s: &mut Session, p: Value) -> (Value, Vec<Value>) {
    let mut p = p;
    p["rules"] = json!(["G-PRESENT"]);
    p["all"] = json!(true);
    p["limit"] = json!(50);
    let r = s.execute("validate.prescreen", &p).unwrap();
    let fs = r["findings"].as_array().cloned().unwrap_or_default();
    (r, fs)
}

fn in_area<'a>(fs: &'a [Value], room: &str) -> Vec<&'a Value> {
    fs.iter().filter(|f| f["area"].as_str().is_some_and(|a| a.ends_with(room))).collect()
}

fn verdicts(fs: &[&Value]) -> (usize, usize, usize) {
    let n = |v: &str| fs.iter().filter(|f| f["verdict"] == v).count();
    (n("pass"), n("missing"), n("extra"))
}

#[test]
fn a_typical_room_missing_a_device_and_holding_an_extra_one_is_told_by_its_enlarged_plan() {
    let mut j = job();
    let (r, fs) = present(&mut j.s, json!({"level": "L1"}));
    let r101 = in_area(&fs, "101");
    assert_eq!(verdicts(&r101), (3, 1, 1), "{r}");
    let missing = r101.iter().find(|f| f["verdict"] == "missing").unwrap();
    assert_eq!(missing["measured"]["basis"], "enlarged_plan", "{missing}");
    assert_eq!(missing["kind"], "receptacle");
    assert_eq!(missing["measured"]["sheet"], "E-601");
    assert_eq!(missing["measured"]["view"], "E-601 ENLARGED GUEST ROOM POWER PLAN");
    // Where on the sheet, and where in the room.
    let sa = missing["measured"]["sheet_at"].as_array().unwrap();
    let want = on_view(LAYOUT[1].2);
    assert!((sa[0].as_f64().unwrap() - want[0]).abs() < 0.1 && (sa[1].as_f64().unwrap() - want[1]).abs() < 0.1, "{missing}");
    let p = in_room(C101, LAYOUT[1].2, false);
    let a = missing["at"].as_array().unwrap();
    assert!((a[0].as_f64().unwrap() - p[0]).abs() < 1.0 && (a[1].as_f64().unwrap() - p[1]).abs() < 1.0, "{missing}");
    assert_eq!(missing["expected_source"]["sheet"], "E-601");
    assert!(missing["issue"].as_str().unwrap().contains("E-601"), "{missing}");
    assert!(missing["handle"].is_null());
    assert_eq!(missing["severity"], "high");
    let extra = r101.iter().find(|f| f["verdict"] == "extra").unwrap();
    assert!(extra["handle"].as_str().unwrap().eq_ignore_ascii_case(&j.added), "{extra}");
    assert_eq!(extra["severity"], "low");
    assert_eq!(extra["measured"]["trade"], "electrical");
    // The passes name the element and how far off it is.
    for f in r101.iter().filter(|f| f["verdict"] == "pass") {
        assert!(f["handle"].as_str().is_some_and(|h| !h.is_empty()), "{f}");
        assert!(f["measured"]["off_in"].as_f64().unwrap() <= 36.0, "{f}");
    }
    // The power plan's receptacle in room 101 is not asked for there: the enlarged plan governs.
    assert!(r101.iter().all(|f| f["measured"]["sheet"] != "E-101"), "{r}");
    // The elevation view of the type is not a plan: nothing comes from it.
    assert!(fs.iter().all(|f| f["measured"]["view"].as_str().is_none_or(|v| !v.contains("BED WALL"))), "{r}");
    // Counted per room type and per level.
    let by_type = &r["present"]["by_room_type"]["guest room"];
    assert_eq!(by_type["rooms"], 2, "{r}");
    assert_eq!(by_type["missing"], 1, "{r}");
    assert_eq!(by_type["extra"], 1, "{r}");
    assert_eq!(by_type["pass"], 7, "{r}");
    assert_eq!(r["present"]["by_level"]["L1"]["rooms"], 3, "{r}");
    assert_eq!(r["present"]["rooms_by_basis"]["enlarged_plan"], 2, "{r}");
    assert_eq!(r["present"]["rooms_by_basis"]["trade_plan"], 1, "{r}");
    // Every room in scope has a set: none is left out.
    assert_eq!(r["rooms_without_set"].as_array().map(Vec::len), Some(0), "{r}");
    assert_eq!(r["checked"]["G-PRESENT"]["pass"].as_u64().unwrap() as usize, fs.iter().filter(|f| f["verdict"] == "pass").count(), "{r}");
}

#[test]
fn a_mirrored_room_of_the_type_matches_its_mirrored_layout() {
    let mut j = job();
    let (r, fs) = present(&mut j.s, json!({"room": "102"}));
    let r102 = in_area(&fs, "102");
    assert_eq!(verdicts(&r102), (4, 0, 0), "{r}");
    // Without the fit's mirror the layout would not fit: the turn is found by the symbols, and
    // it is the mirror.
    let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["unit_instances"][1]["fit"] = json!(null);
    sp["unit_instances"][1]["hand"] = json!("normal");
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let (r, fs) = present(&mut j.s, json!({"room": "102"}));
    let r102 = in_area(&fs, "102");
    assert_eq!(verdicts(&r102), (4, 0, 0), "{r}");
    // And a fit that says the room is drawn as the view (not mirrored) is believed: the
    // mirrored room then misses what lies off its centre line.
    sp["unit_instances"][1]["fit"] = json!({"rotation_deg": 0.0, "mirrored": false, "view": "E-601 ENLARGED GUEST ROOM POWER PLAN"});
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let (r, fs) = present(&mut j.s, json!({"room": "102"}));
    let (_, missing, _) = verdicts(&in_area(&fs, "102"));
    assert!(missing >= 2, "a wrong fit shows: {r}");
}

#[test]
fn a_room_of_no_unit_type_is_checked_against_its_levels_trade_plans() {
    let mut j = job();
    let (r, fs) = present(&mut j.s, json!({"room": "103"}));
    let r103 = in_area(&fs, "103");
    assert_eq!(verdicts(&r103), (2, 1, 1), "{r}");
    let missing = r103.iter().find(|f| f["verdict"] == "missing").unwrap();
    assert_eq!(missing["measured"]["basis"], "trade_plan", "{missing}");
    assert_eq!(missing["measured"]["sheet"], "E-101");
    let a = missing["at"].as_array().unwrap();
    assert!((a[0].as_f64().unwrap() - 55.0).abs() < 0.1 && (a[1].as_f64().unwrap() - 13.3).abs() < 0.1, "placed by the registration: {missing}");
    let extra = r103.iter().find(|f| f["verdict"] == "extra").unwrap();
    assert!(extra["handle"].as_str().unwrap().eq_ignore_ascii_case(&j.stray), "{extra}");
    assert_eq!(r["present"]["by_room_type"]["office"]["rooms"], 1, "{r}");
}

#[test]
fn the_validate_run_and_its_status_count_g_present_per_room_type_and_level() {
    let mut j = job();
    let d = std::env::temp_dir().join(format!("cadaddy-present-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let run = j.s.execute("validate", &json!({"scope": "level", "level": "L1", "out_dir": d.to_string_lossy()})).unwrap();
    let measured = &run["prescreen"]["measured"];
    assert_eq!(measured["checked"]["G-PRESENT"]["missing"], 2, "{run}");
    assert_eq!(measured["checked"]["G-PRESENT"]["extra"], 2, "{run}");
    let st = j.s.execute("validate.status", &json!({})).unwrap();
    let p = &st["prescreen"]["measured"]["present"];
    assert_eq!(p["by_room_type"]["guest room"]["rooms"], 2, "{st}");
    assert_eq!(p["by_room_type"]["office"]["missing"], 1, "{st}");
    assert_eq!(p["by_level"]["L1"]["missing"], 2, "{st}");
    assert_eq!(p["by_level"]["L1"]["extra"], 2, "{st}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_prescreen_without_a_job_keeps_the_project_rule_sets_and_hostile_job_data_never_panics() {
    let mut j = job();
    // No job read: nothing off the prints; a room type's project-rule set still counts.
    j.s.doc_mut().unwrap().agent = None;
    j.s.execute(
        "project.rules.set",
        &json!({"domain": "custom", "key": "prescreen.expect.office", "value": "switch=1", "reason": "made-up test rule"}),
    )
    .unwrap();
    let (r, fs) = present(&mut j.s, json!({"level": "L1"}));
    assert!(fs.iter().all(|f| f["measured"]["basis"] == "project_rule"), "{r}");
    let office = in_area(&fs, "103");
    assert_eq!(office.len(), 1, "{r}");
    assert_eq!(office[0]["verdict"], "extra", "two switches where one is expected: {r}");
    // The guest rooms (by name) have none.
    assert_eq!(r["rooms_without_set"], json!(["Guest Room"]), "{r}");
    // Junk where the job's devices and the unit types' views and fits go.
    for devices in [
        json!([{"tag": "REC", "at": [f64::MAX, 1.0], "sheet": "E-601"}, {"tag": "", "at": [1.0], "sheet": ""}, {"at": "x"}, 7, null]),
        json!("not a list"),
        json!([{"tag": "REC", "kind": "receptacle", "at": on_view([0.0, 0.0]), "sheet": "E-601/9/9"}]),
    ] {
        set_job(&mut j.s, devices);
        let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
        sp["unit_types"][0]["views"] = json!([{"sheet": "E-601", "title": "ENLARGED", "rect": [1e308, -1e308, "x", 0]}, {"sheet": 5}, null]);
        sp["unit_instances"][0]["fit"] = json!({"rotation_deg": "ninety", "mirrored": 3, "view": 12});
        j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
        let r = j.s.execute("validate.prescreen", &json!({"rules": ["G-PRESENT"], "all": true})).unwrap();
        assert!(r["checked"]["G-PRESENT"].is_object(), "{r}");
    }
}
