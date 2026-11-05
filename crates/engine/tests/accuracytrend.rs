//! The accuracy trend (Q6193) on a made-up job: two guest rooms and a corridor, measured twice —
//! a good build (receptacles on their walls) and a worse one (two of them pulled off their wall,
//! one left out) — then the two rows compared: the worse build is a REGRESSION naming the metric,
//! both values, both commits and the merger batch between them.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
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
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "901"}, {"name": "Guest Room", "number": "902"}, {"name": "Corridor", "number": "903"}],
        "devices": {"by_type": {"DUPLEX": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]}}},
        "sheets": [
            {"number": "X-901", "title": "FLOOR PLAN - LEVEL ONE", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "X-901", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}}
        ]
    })
}

/// The made-up job built from receptacles at building points (feet).
fn build(receptacles: &[[f64; 2]]) -> Session {
    let mut s = Session::new();
    // A 100' × 12' shell of 6" walls; partitions at 15' and 30'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [100.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [99.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, 11.5]);
    line(&mut s, [30.0, 0.5], [30.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 6.0]), "901");
    text(&mut s, at([20.0, 6.0]), "902");
    text(&mut s, at([60.0, 6.0]), "903");
    layer(&mut s, "G-FRAME");
    s.execute("pline", &json!({"vertices": [[0.0, 0.0], [SW, 0.0], [SW, SH], [0.0, SH]], "closed": true})).unwrap();
    let (mut s, recs) = place(s, "RECEP", "E-POWR", receptacles, "DUPLEX");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for h in &recs {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "X-911", "kind": "receptacle"})).unwrap();
    }
    s
}

/// The good build: every receptacle on a wall face (south walls of 901 and 902, the corridor's
/// north wall).
fn good() -> Session {
    build(&[[10.0, 0.5], [5.0, 11.5], [20.0, 0.5], [25.0, 11.5], [50.0, 11.5], [70.0, 0.5]])
}

/// The worse build: receptacles of 901, 902 and the corridor pulled 3 ft off their walls, one
/// left out.
fn worse() -> Session {
    build(&[[10.0, 3.5], [5.0, 11.5], [20.0, 3.5], [25.0, 11.5], [50.0, 8.5]])
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-accuracytrend-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn measure(s: &mut Session, commit: &str, job_s: f64) -> Value {
    let rooms = json!(["901", {"room": "902", "level": "L1"}, "999"]);
    s.execute("validate.measure", &json!({"commit": commit, "label": "Project A", "rooms": rooms, "job_s": job_s, "peak_mb": 900})).unwrap()
}

fn metric(r: &Value, k: &str) -> f64 {
    r["metrics"][k].as_f64().unwrap_or_else(|| panic!("no metric {k} in {}", r["metrics"]))
}

#[test]
fn one_build_measured_is_one_row_keyed_by_its_commit() {
    let mut s = good();
    let before = s.doc().unwrap().model.len();
    let r = measure(&mut s, "1111111aaaa", 1800.0);
    assert_eq!(r["schema"], "cadaddy.accuracy/1");
    assert_eq!((r["commit"].as_str(), r["label"].as_str()), (Some("1111111aaaa"), Some("Project A")));
    assert!(r["at"].as_str().is_some_and(|t| t.ends_with('Z')), "{r}");
    // Gate A: every receptacle on its wall.
    assert_eq!(metric(&r, "gate_a.host_pass_pct"), 100.0, "{r}");
    assert_eq!(metric(&r, "gate_a.host_fail"), 0.0);
    assert_eq!(metric(&r, "gate_a.host_fail_far"), 0.0);
    assert!(metric(&r, "gate_a.host_pass") >= 6.0, "{r}");
    assert_eq!(metric(&r, "gate_a.outside_building"), 0.0);
    // Every prescreen rule over the building, and over the fixed rooms (two of three found).
    assert_eq!(metric(&r, "building.G-HOST.accuracy_pct"), 100.0);
    assert!(r["metrics"]["building.G-HEIGHT.problems"].is_number(), "{r}");
    assert_eq!((metric(&r, "rooms.listed"), metric(&r, "rooms.found")), (3.0, 2.0));
    assert!(metric(&r, "rooms.G-HOST.pass") >= 4.0, "{r}");
    assert_eq!(metric(&r, "rooms.G-HOST.accuracy_pct"), 100.0);
    let d = &r["detail"]["rooms"];
    assert!(d["by_room"]["901"]["checked"]["G-HOST"]["pass"].as_u64().is_some_and(|n| n >= 2), "{d}");
    assert!(d["by_room"]["L1/902"].is_object(), "{d}");
    assert!(d["missing"][0].as_str().is_some_and(|m| m.starts_with("999")), "{d}");
    // The takeoff by trade and system, the elements by kind, the job.
    assert!(r["metrics"].as_object().unwrap().keys().any(|k| k.starts_with("takeoff.") && k.ends_with(".elements")), "{}", r["metrics"]);
    assert!(metric(&r, "elements.total") >= 8.0, "{r}");
    // (The made-up job's receptacles are plan objects, not built boxes: the counts are the built
    // elements by kind.)
    assert!(metric(&r, "elements.wall") >= 2.0 && metric(&r, "elements.room") == 3.0, "{r}");
    assert_eq!((metric(&r, "build.job_s"), metric(&r, "build.peak_mb")), (1800.0, 900.0));
    // Measuring changes nothing; the same build measures the same.
    assert_eq!(s.doc().unwrap().model.len(), before);
    let again = measure(&mut s, "1111111aaaa", 1800.0);
    let strip = |v: &Value| {
        let mut m = v["metrics"].clone();
        m.as_object_mut().unwrap().remove("build.measure_s");
        m
    };
    assert_eq!(strip(&r), strip(&again));
}

#[test]
fn a_worse_build_is_a_regression_naming_the_metric_both_values_and_the_batch() {
    let dir = dir("two");
    let trend = dir.join("trend.jsonl");
    let log = dir.join("merge.log");
    std::fs::write(
        &log,
        "2026-01-01T00:00:00Z mbatch-1 merged alpha(made-up = a first) -> next 1111111aaaa; FLUSH run\n\
         2026-01-02T00:00:00Z mbatch-2 merged beta(made-up = moves receptacles (badly)) gamma -> next 2222222bbbb; ok\n",
    )
    .unwrap();
    let a = measure(&mut good(), "1111111aaaa", 1800.0);
    let b = measure(&mut worse(), "2222222bbbb", 1850.0);
    assert!(metric(&b, "gate_a.host_pass_pct") < metric(&a, "gate_a.host_pass_pct"), "{b}");
    assert!(metric(&b, "gate_a.host_fail") >= 3.0 && metric(&b, "gate_a.host_fail_far") >= 3.0, "{b}");
    // The first row: nothing to compare yet.
    let mut s = Session::new();
    let r = s.execute("validate.trend", &json!({"path": trend, "row": a})).unwrap();
    assert_eq!((r["rows"].as_u64(), r["appended"].as_bool(), r["regressions"].as_u64()), (Some(1), Some(true), Some(0)), "{r}");
    // The second, from its file: the worse build regresses.
    let row_file = dir.join("row.json");
    std::fs::write(&row_file, b.to_string()).unwrap();
    let table = dir.join("trend.md");
    let svg = dir.join("trend.svg");
    let r = s
        .execute(
            "validate.trend",
            &json!({"path": trend, "row_file": row_file, "merge_log": log, "commits": ["2222222 beta merge", "3333333 beta: moves"], "table": table, "svg": svg}),
        )
        .unwrap();
    assert_eq!(r["rows"], 2);
    assert_eq!(r["compared"]["from"], "1111111aaaa");
    assert_eq!(r["compared"]["to"], "2222222bbbb");
    let fs = r["findings"].as_array().unwrap();
    let host = fs.iter().find(|f| f["metric"] == "gate_a.host_pass_pct").unwrap_or_else(|| panic!("no host regression: {r}"));
    assert_eq!(host["verdict"], "regression");
    assert_eq!(host["previous"], metric(&a, "gate_a.host_pass_pct"));
    assert_eq!(host["current"], metric(&b, "gate_a.host_pass_pct"));
    assert_eq!((host["from"].as_str(), host["to"].as_str(), host["why"].as_str()), (Some("1111111aaaa"), Some("2222222bbbb"), Some("fell")));
    let issue = host["issue"].as_str().unwrap();
    assert!(issue.starts_with("REGRESSION gate_a.host_pass_pct") && issue.contains("1111111a and 2222222b"), "{issue}");
    assert!(issue.contains("mbatch-2 (beta, gamma)") && !issue.contains("mbatch-1"), "{issue}");
    assert_eq!(host["suspects"][0]["branches"], json!(["beta", "gamma"]));
    assert_eq!(host["commits"].as_array().map(Vec::len), Some(2));
    // The fixed rooms regress too, and so do the stable counts (a receptacle left out is within
    // the tolerance of 3); the job's 50 s more is within its tolerance.
    let names: Vec<&str> = fs.iter().filter_map(|f| f["metric"].as_str()).collect();
    assert!(names.contains(&"rooms.G-HOST.accuracy_pct") && names.contains(&"gate_a.host_fail_far"), "{names:?}");
    assert!(!names.contains(&"build.job_s") && !names.iter().any(|n| n.starts_with("elements.")), "{names:?}");
    // The table and the plot, written; the trend file holds both rows, one per line.
    let t = std::fs::read_to_string(&table).unwrap();
    assert!(t.contains("| 1111111a |") && t.contains("| 2222222b |") && t.contains(" (!)"), "{t}");
    assert!(std::fs::read_to_string(&svg).unwrap().contains("<polyline"));
    let text = std::fs::read_to_string(&trend).unwrap();
    assert_eq!(text.lines().count(), 2);
    // An existing table is kept unless force; the trend file is only ever appended to.
    assert!(s.execute("validate.trend", &json!({"path": trend, "table": table})).is_err());
    let r = s.execute("validate.trend", &json!({"path": trend, "table": table, "force": true, "last": 1})).unwrap();
    assert_eq!((r["rows"].as_u64(), r["appended"].as_bool()), (Some(2), Some(false)));
    assert!(!r["table"].as_str().unwrap().contains("1111111a"), "{r}");
    assert_eq!(std::fs::read_to_string(&trend).unwrap(), text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_rows_files_and_params_are_refused_and_never_panic() {
    let dir = dir("bad");
    let trend = dir.join("trend.jsonl");
    let mut s = Session::new();
    // A drawing with no model: the measure still answers, with what it couldn't measure.
    let r = s.execute("validate.measure", &json!({"commit": "abc", "rooms": ["1"]})).unwrap();
    assert!(r["detail"]["errors"].as_array().is_some_and(|e| !e.is_empty()), "{r}");
    for p in [
        json!({"rooms": "901"}),
        json!({"rooms": [null]}),
        json!({"rooms": vec!["1"; 60]}),
        json!({"job_s": -1}),
        json!({"peak_mb": "big"}),
        json!({"skip_rules": 3}),
        json!({"path": "out.dwg"}),
    ] {
        assert!(s.execute("validate.measure", &p).is_err(), "{p}");
    }
    for p in [
        json!({}),
        json!({"path": trend, "row": {"metrics": {"a": 1}}}),
        json!({"path": trend, "row": {"schema": "cadaddy.accuracy/1"}}),
        json!({"path": trend, "row": {"schema": "cadaddy.accuracy/1", "metrics": {}}, "row_file": "x.json"}),
        json!({"path": trend, "row_file": dir.join("missing.json")}),
        json!({"path": trend, "last": 0}),
        json!({"path": trend, "commits": "abc"}),
        json!({"path": trend, "merge_log": dir.join("missing.log")}),
        json!({"path": dir.join("trend.dxf")}),
    ] {
        assert!(s.execute("validate.trend", &p).is_err(), "{p}");
    }
    // Junk lines in the trend file are skipped and counted; odd numbers never panic.
    std::fs::write(&trend, "not json\n{\"metrics\": 5}\n\n{\"schema\":\"cadaddy.accuracy/1\",\"commit\":\"x\",\"metrics\":{\"gate_a.host_pass_pct\":1e308,\"elements.total\":-5}}\n").unwrap();
    let row = json!({"schema": "cadaddy.accuracy/1", "commit": "y", "metrics": {"gate_a.host_pass_pct": 0, "elements.total": 1e300}});
    let r = s.execute("validate.trend", &json!({"path": trend, "row": row, "svg": dir.join("p.svg")})).unwrap();
    assert_eq!((r["rows"].as_u64(), r["skipped_lines"].as_u64()), (Some(2), Some(2)), "{r}");
    assert!(r["regressions"].as_u64().is_some_and(|n| n >= 1), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
