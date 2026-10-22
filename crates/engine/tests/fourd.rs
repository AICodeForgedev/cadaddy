//! 4D: a schedule (CSV, XLSX or rows) linked to the 3D model built from a plan — walls by level
//! and class, ducts and pipes by system, a conduit picked by hand — and the model's state on a
//! date: built, in progress, not yet. Exported day by day as frames.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::fourd;
use cadaddy_engine::doc::Handle;
use cadaddy_walk3d::fourd::{Counts, Phase, day};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 4})).unwrap();
}

/// A one-room plan with a supply duct, a cold-water pipe and a conduit, built to 3D.
fn model() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 180]]})).unwrap();
    text(&mut s, [180.0, 70.0], "24x12 BOT @ 10'-0\"");
    layer(&mut s, "P-DOMW-CW");
    s.execute("line", &json!({"points": [[100, 0], [100, 120]]})).unwrap();
    text(&mut s, [104.0, 20.0], "2\" CW CL @ 10'-6\"");
    layer(&mut s, "E-COND");
    s.execute("line", &json!({"points": [[300, 300], [400, 300]]})).unwrap();
    text(&mut s, [340.0, 306.0], "1\" C BOT @ 11'-0\"");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Handles of the model's elements of an IFC class (and system).
fn of(s: &Session, class: &str, system: Option<&str>) -> Vec<String> {
    fourd::elements(s.doc().unwrap())
        .into_iter()
        .filter(|e| e.class == class && system.is_none_or(|sy| e.systems.iter().any(|x| x == sy)))
        .map(|e| e.handle.hex())
        .collect()
}

fn rows() -> Value {
    json!([
        ["Sample Project 4D", "", "", "", "", ""],
        ["ID", "Task", "Start", "Finish", "Level", "Filter"],
        ["W10", "Level 1 walls", "2026-03-02", "2026-03-06", "L1", "class=Wall"],
        ["M10", "Supply ductwork", "3/9/2026", "3/13/2026", "", "system=M-DUCT*; class=Duct*"],
        ["P10", "Cold water", "2026-03-11", "2026-03-12", "", "class=Pipe*"],
        ["E10", "Conduit (picked by hand)", "2026-03-16", "2026-03-16", "", ""],
        ["X10", "Bad row", "someday", "2026-03-16", "", ""],
    ])
}

fn state(s: &mut Session, date: &str) -> Value {
    s.execute("fourd.state", &json!({"date": date})).unwrap()
}

fn counts(v: &Value) -> (u64, u64, u64) {
    let c = &v["counts"];
    (c["built"].as_u64().unwrap(), c["inProgress"].as_u64().unwrap(), c["notYet"].as_u64().unwrap())
}

fn set(v: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = v[key].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
    out.sort();
    out
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn links_resolve_by_rules_and_by_hand() {
    let mut s = model();
    let walls = of(&s, "IfcWall", None);
    let ducts = of(&s, "IfcDuctSegment", Some("M-DUCT-SUPP"));
    let pipes = of(&s, "IfcPipeSegment", None);
    let conduits = of(&s, "IfcCableCarrierSegment", None);
    assert!(!walls.is_empty() && ducts.len() == 1 && pipes.len() == 1 && conduits.len() == 1, "{walls:?} {ducts:?} {pipes:?} {conduits:?}");
    let r = s.execute("fourd.import", &json!({"rows": rows()})).unwrap();
    assert_eq!(r["tasks"], 4, "{r}");
    assert_eq!(r["notes"].as_array().unwrap().len(), 1, "the bad row is noted: {r}");
    assert_eq!(r["span"], json!(["2026-03-02", "2026-03-16"]));
    assert_eq!(r["empty"], json!(["E10"]), "the conduit task has no rule yet");
    // Pick the conduit by hand: by handles, and by the selection.
    let l = s.execute("fourd.link", &json!({"task": "E10", "handles": conduits})).unwrap();
    assert_eq!((l["elements"].as_u64(), l["added"].as_u64()), (Some(1), Some(1)), "{l}");
    let list = s.execute("fourd.list", &json!({})).unwrap();
    let n = |id: &str| list["tasks"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["elements"].as_u64().unwrap() as usize;
    assert_eq!((n("W10"), n("M10"), n("P10"), n("E10")), (walls.len(), 1 + of(&s, "IfcDuctFitting", None).len(), 1, 1), "{list}");
    // The duct's fittings are IfcDuctFitting: `class=Duct*` covers them too.
    let fittings = of(&s, "IfcDuctFitting", Some("M-DUCT-SUPP"));
    assert_eq!(n("M10"), 1 + fittings.len(), "{fittings:?}");
    // Take the pipe off its rule; put it back.
    s.execute("fourd.link", &json!({"task": "P10", "handles": pipes, "remove": true})).unwrap();
    assert_eq!(s.execute("fourd.list", &json!({})).unwrap()["tasks"][2]["elements"], 0);
    s.execute("fourd.link", &json!({"task": "p10", "handles": pipes})).unwrap();
    assert_eq!(s.execute("fourd.list", &json!({})).unwrap()["tasks"][2]["elements"], 1);
    // A new rule for a task.
    let r = s.execute("fourd.link", &json!({"task": "Cold water", "filter": {"system": "P-*"}, "clear": true})).unwrap();
    assert_eq!(r["elements"], 1);
    // Re-importing keeps hand picks of tasks with the same id.
    s.execute("fourd.import", &json!({"rows": rows()})).unwrap();
    assert_eq!(s.execute("fourd.list", &json!({})).unwrap()["tasks"][3]["elements"], 1);
    s.execute("fourd.import", &json!({"rows": rows(), "merge": false})).unwrap();
    assert_eq!(s.execute("fourd.list", &json!({})).unwrap()["tasks"][3]["elements"], 0);
    // Bad input is an error, never a panic.
    for p in [json!({}), json!({"rows": [["a"]]}), json!({"rows": "x"}), json!({"path": "/no/such/schedule.csv"})] {
        assert!(s.execute("fourd.import", &p).is_err(), "{p}");
    }
    assert!(s.execute("fourd.link", &json!({"task": "nope", "handles": []})).is_err());
    assert!(s.execute("fourd.link", &json!({"task": "W10", "filter": "colour=red"})).is_err());
    assert!(s.execute("fourd.state", &json!({"date": "13/45/2026"})).is_err());
}

#[test]
fn the_state_at_a_date_is_right() {
    let mut s = model();
    s.execute("fourd.import", &json!({"rows": rows()})).unwrap();
    let conduits = of(&s, "IfcCableCarrierSegment", None);
    s.execute("fourd.link", &json!({"task": "E10", "handles": conduits})).unwrap();
    let walls = sorted(of(&s, "IfcWall", None));
    // The duct run and its elbow.
    let ducts = sorted([of(&s, "IfcDuctSegment", Some("M-DUCT-SUPP")), of(&s, "IfcDuctFitting", Some("M-DUCT-SUPP"))].concat());
    assert_eq!(ducts.len(), 2);
    let pipes = of(&s, "IfcPipeSegment", None);
    let w = walls.len() as u64;

    let v = state(&mut s, "2026-03-01");
    assert_eq!(counts(&v), (0, 0, w + 4), "{v}");
    let v = state(&mut s, "2026-03-02");
    assert_eq!(set(&v, "inProgress"), walls, "the first day is in progress");
    let v = state(&mut s, "2026-03-06");
    assert_eq!(counts(&v), (0, w, 4), "so is the last");
    let v = state(&mut s, "2026-03-07");
    assert_eq!(set(&v, "built"), walls);
    assert_eq!(counts(&v), (w, 0, 4));
    let v = state(&mut s, "2026-03-11");
    assert_eq!(set(&v, "inProgress"), sorted([ducts.clone(), pipes.clone()].concat()));
    let v = state(&mut s, "2026-03-13");
    assert_eq!(counts(&v), (w + 1, 2, 1), "the pipe is done, the duct finishing: {v}");
    assert_eq!(
        v["tasks"],
        json!([{"id": "W10", "phase": "built"}, {"id": "M10", "phase": "in_progress"}, {"id": "P10", "phase": "built"}, {"id": "E10", "phase": "not_yet"}])
    );
    let v = state(&mut s, "2026-03-17");
    assert_eq!(counts(&v), (w + 4, 0, 0));
    assert!(v["counts"]["unscheduled"].as_u64().unwrap() > 0, "plan linework and tags are on no task");
    // No date: the schedule's start.
    assert_eq!(s.execute("fourd.state", &json!({"handles": false})).unwrap()["date"], "2026-03-02");

    // The staged drawing: not-yet work gone (or pale), in-progress work orange, the rest kept.
    let doc = s.doc().unwrap().clone();
    let (st, c) = fourd::staged(&doc, day("2026-03-11").unwrap(), false).unwrap();
    assert_eq!(c, Counts { built: walls.len(), in_progress: 3, not_yet: 1, unscheduled: c.unscheduled });
    let h = |x: &str| Handle(u64::from_str_radix(x, 16).unwrap());
    assert!(st.entity(h(&conduits[0])).is_none(), "the conduit isn't there yet");
    let duct = st.entity(h(&ducts[0])).unwrap();
    assert_eq!(duct.common.color, cadaddy_engine::doc::color::Color::True(cadaddy_color::Rgb(240, 140, 20)));
    assert_eq!(st.entity(h(&walls[0])).unwrap().common.color, doc.entity(h(&walls[0])).unwrap().common.color);
    let (gh, _) = fourd::staged(&doc, day("2026-03-11").unwrap(), true).unwrap();
    assert!(gh.entity(h(&conduits[0])).is_some(), "ghosted, it stays");
    let (map, _) = fourd::phases(&doc, &fourd::load(&doc).unwrap(), day("2026-03-11").unwrap());
    assert_eq!(map.get(&h(&pipes[0])), Some(&Phase::InProgress));

    // The schedule is saved with the drawing and opened again.
    let bytes = cadaddy_io::write(&doc, "Sample 4D.dxf").unwrap();
    let back = cadaddy_io::read(&bytes, "Sample 4D.dxf").unwrap();
    assert_eq!(fourd::load(&back).unwrap(), fourd::load(&doc).unwrap());
    s.execute("fourd.clear", &json!({})).unwrap();
    assert!(s.execute("fourd.state", &json!({})).is_err(), "no schedule");
}

#[test]
fn schedules_import_from_csv() {
    let dir = std::env::temp_dir().join(format!("cadaddy_fourd_import_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("Sample Schedule.csv");
    std::fs::write(&csv, "\u{feff}Task,Start,Finish,Class,System\nWalls,2026-03-02,2026-03-06,Wall,\nDuct,2026-03-09,2026-03-13,Duct*,M-DUCT*\n")
        .unwrap();
    let mut s = model();
    let r = s.execute("fourd.import", &json!({"path": csv.display().to_string()})).unwrap();
    assert_eq!((r["tasks"].as_u64(), r["empty"].as_array().map(Vec::len)), (Some(2), Some(0)), "{r}");
    // A semicolon-separated file, the filter in one column.
    let csv = dir.join("Sample Schedule EU.csv");
    std::fs::write(
        &csv,
        "Task;Start;Finish;Filter\nCold water;11.3.2026;12.3.2026;name=P-DOMW*, class=Pipe*\nPlan;11.3.2026;12.3.2026;name=P-DOMW*\n",
    )
    .unwrap();
    let r = s.execute("fourd.import", &json!({"path": csv.display().to_string()})).unwrap();
    assert_eq!(r["span"], json!(["2026-03-11", "2026-03-12"]), "{r}");
    assert_eq!(r["linked"], 3, "the pipe by its system, the plan's line and tag by their layer: {r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_build_sequence_exports_frame_by_frame() {
    let mut s = model();
    s.execute("fourd.import", &json!({"rows": rows()})).unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy_fourd_export_{}", std::process::id()));
    let r = s
        .execute(
            "fourd.export",
            &json!({"folder": dir.display().to_string(), "from": "2026-03-01", "to": "2026-03-17", "step": 4, "width": 160, "height": 90}),
        )
        .unwrap();
    assert_eq!(r["frames"], 5, "03-01, 05, 09, 13 and 17: {r}");
    let frames: Vec<image::RgbaImage> = (0..5).map(|i| image::open(dir.join(format!("frame_{i:05}.png"))).unwrap().to_rgba8()).collect();
    assert_eq!(frames[0].dimensions(), (160, 90));
    let orange = |img: &image::RgbaImage| img.pixels().filter(|p| p[0] > 180 && p[1] > 80 && p[1] < 170 && p[2] < 90).count();
    let ink = |img: &image::RgbaImage| img.pixels().filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250).count();
    assert_eq!(orange(&frames[0]), 0, "nothing under way on 03-01");
    assert!(orange(&frames[1]) > 50, "the walls are going up on 03-05");
    assert!(ink(&frames[4]) > ink(&frames[0]), "more is built by the end");
    assert!(s.execute("fourd.export", &json!({"folder": dir.display().to_string(), "step": 0})).is_err());
    assert!(s.execute("fourd.export", &json!({"folder": dir.display().to_string(), "from": "2026-04-01", "to": "2026-03-01"})).is_err());
    assert!(s.execute("fourd.export", &json!({"folder": dir.display().to_string(), "width": 100000})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
