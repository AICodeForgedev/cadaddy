//! G-UNDERGROUND (Q6149) on a made-up slab-on-grade job. Level 1 is one 40' × 24' room (100)
//! with panels LP and LQ at its ends. Its power plan E-101 has a keynote list (1: 2" Schedule 40
//! PVC under the slab from LP to LQ, at least 48" below grade; 2: 1" PVC under the slab to a
//! floor box, at least 18" below the slab) and a legend line type naming below-slab piping; the
//! electrical site plan ES-1 (no level) has a callout for two 4" conduits 48 IN below grade for
//! the telephone service. The model has keynote 1's run 24" deep, keynote 2's at 18" with its
//! risers at the floor box and the wall, a feeder CADaddy proposes under the slab (with an RFI),
//! and a buried run nothing calls for. The underground sheet is generated (`ugsheets.build`).

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// Level 1's plan origin on the drawing (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// The site plan sits below the power plan.
const SITE_Y: f64 = -3000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on level 1's plan, in drawing inches.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// E-101's keynote list and a legend row, and ES-1's callout.
fn notes(s: &mut Session) {
    layer(s, "E-ANNO-TEXT");
    let o = at([60.0, 30.0]);
    text(s, o, "ELECTRICAL KEYNOTES:", 9.0);
    let lines = [
        (0.0, "1. PROVIDE 2\" SCH. 40 PVC CONDUIT UNDER SLAB FROM PANEL"),
        (14.0, "LP TO PANEL LQ, MIN. 48\" BELOW GRADE."),
        (0.0, "2. PROVIDE 1\" PVC CONDUIT UNDER SLAB TO THE FLOOR BOX,"),
        (14.0, "MIN. 18\" BELOW SLAB."),
    ];
    for (i, (dx, t)) in lines.iter().enumerate() {
        text(s, [o[0] + dx, o[1] - 14.0 - 10.0 * i as f64], t, 6.0);
    }
    // A legend's line type: it names below-slab work but calls for none.
    text(s, at([60.0, -40.0]), "COLD WATER PIPING (BELOW SLAB)", 6.0);
    // The site plan's callout, its lines one text each.
    let site = [600.0, SITE_Y + 1500.0];
    for (i, t) in ["PROVIDE TWO (2) 4\" SCH. 40 PVC CONDUITS", "48 IN BELOW GRADE FOR THE TELEPHONE", "SERVICE ENTRANCE."].iter().enumerate() {
        text(s, [site[0], site[1] - 10.0 * i as f64], t, 6.0);
    }
}

fn mep() -> Value {
    let box_at = |tag: &str, a: [f64; 2], b: [f64; 2]| json!({"tag": tag, "kind": "panel", "level": "L1", "min": at(a), "max": at(b), "bottom_ft": 4.5, "height_ft": 3.0});
    json!({
        "systems": [
            {"name": "E-UG-PVC40", "layer": "E-UG", "kind": "conduit", "size_in": [1, 0], "bottom_ft": -2.0},
            {"name": "E-FEEDER", "layer": "E-FEED", "kind": "conduit", "size_in": [2, 0], "bottom_ft": 9.0}
        ],
        "equipment": [box_at("LP", [2.0, 3.0], [6.0, 7.0]), box_at("LQ", [30.0, 3.0], [34.0, 7.0])],
        "runs": [
            // Keynote 1: 2" from LP to LQ, its top 24" below the floor.
            {"system": "E-UG-PVC40", "level": "L1", "points": [at([4.0, 3.5]), at([32.0, 3.5])], "size_in": [2, 0], "bottoms_ft": [-26.0 / 12.0],
             "height_source": "note: E-101 keynote 1 (under the slab)", "size_source": "note", "source_sheet": "E-101", "id": "K1"},
            // Keynote 2: 1" from the floor box to the wall, 18" down, and its risers.
            {"system": "E-UG-PVC40", "level": "L1", "points": [at([14.0, 15.0]), at([14.0, 23.5])], "size_in": [1, 0], "bottoms_ft": [-19.0 / 12.0],
             "height_source": "note: E-101 keynote 2 (under the slab)", "size_source": "note", "source_sheet": "E-101", "id": "K2"},
            {"system": "E-UG-PVC40", "level": "L1", "points": [at([14.0, 15.0])], "size_in": [1, 0], "bottoms_ft": [-19.0 / 12.0], "top_ft": 0.0,
             "height_source": "note: E-101 keynote 2 (under the slab)", "source_sheet": "E-101", "id": "K2:up0"},
            {"system": "E-UG-PVC40", "level": "L1", "points": [at([14.0, 23.5])], "size_in": [1, 0], "bottoms_ft": [-19.0 / 12.0], "top_ft": 1.5,
             "height_source": "note: E-101 keynote 2 (under the slab)", "source_sheet": "E-101", "id": "K2:up1"},
            // A feeder CADaddy proposes under the slab.
            {"system": "E-FEEDER", "level": "L1", "points": [at([4.0, 6.5]), at([32.0, 6.5])], "size_in": [2, 0], "bottoms_ft": [-1.5],
             "height_source": "proposed", "size_source": "feeder schedule", "source_sheet": "feeder 1 LP→LQ", "id": "F1"},
            // Buried, and nothing calls for it.
            {"system": "E-UG-PVC40", "level": "L1", "points": [at([20.0, 18.0]), at([26.0, 18.0])], "size_in": [1, 0], "bottoms_ft": [-1.5],
             "height_source": "rule", "id": "X1"},
            // Overhead: not buried work.
            {"system": "E-FEEDER", "level": "L1", "points": [at([4.0, 5.0]), at([32.0, 5.0])], "size_in": [2, 0], "bottoms_ft": [9.0], "id": "OH1"}
        ]
    })
}

/// The slab's exterior walls (boxes, drawing inches), as the model's shell gives them.
fn shells() -> Value {
    let b = |a: [f64; 2], c: [f64; 2]| {
        let (p, q) = (at(a), at(c));
        json!([p[0], p[1], q[0], q[1]])
    };
    json!({"L1": [b([0.0, 0.0], [40.0, 0.5]), b([0.0, 23.5], [40.0, 24.0]), b([0.0, 0.0], [0.5, 24.0]), b([39.5, 0.0], [40.0, 24.0])]})
}

fn spec(walls: bool, columns: Value) -> Value {
    let sheet = |n: &str, title: &str, level: Option<&str>, y: f64| {
        let mut v = json!({"number": n, "title": title, "discipline": "E", "source": "model", "rect": [0.0, y, SW, y + SH]});
        if let Some(l) = level {
            v["level"] = json!(l);
            v["registration"] = json!({"reference": n, "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4});
        }
        v
    };
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Exterior", "thickness_in": 6}],
        "rooms": [{"name": "Lobby", "number": "100", "level": "L1"}],
        "columns": columns,
        "shells": if walls { shells() } else { json!({"L1": []}) },
        "mep": mep(),
        "rfis": [{"id": "RFI-U1", "about": "underground plan", "issue": "The documents issue no underground plan. CADaddy proposes under the slab: feeder 1 LP→LQ."}],
        "sheets": [sheet("E-101", "POWER PLAN - L1", Some("L1"), 0.0), sheet("ES-1", "ELECTRICAL SITE PLAN", None, SITE_Y)]
    })
}

/// The job: with `walls` the model's shell gives the slab's edges (its exterior walls);
/// `columns` are the structural grid's.
fn job(walls: bool, columns: Value) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [40.0, 24.0]);
    rect(&mut s, [0.5, 0.5], [39.5, 23.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([20.0, 12.0]), "100", 6.0);
    notes(&mut s);
    let r = s.execute("bspec.set", &json!({"spec": spec(walls, columns), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    let built = s.execute("ugsheets.build", &json!({"number": "E-UG1"})).unwrap();
    assert_eq!(built["sheets"].as_array().map(Vec::len), Some(1), "{built}");
    s.execute("layout.set", &json!({"name": "Model"})).unwrap();
    s
}

fn findings(s: &mut Session) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": ["G-UNDERGROUND"]})).unwrap();
    let fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    assert!(fs.iter().all(|f| f["rule"] == "G-UNDERGROUND"), "{r}");
    (r, fs)
}

fn callout<'a>(fs: &'a [Value], words: &str) -> &'a Value {
    fs.iter()
        .find(|f| f["kind"] == "buried_run" && f["measured"]["callout"]["text"].as_str().is_some_and(|t| t.contains(words)))
        .unwrap_or_else(|| panic!("no callout finding with {words:?}: {}", Value::Array(fs.to_vec())))
}

#[test]
fn the_rule_is_built_in_with_its_depth_and_size_tolerances() {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    let r = s.execute("validate.rules", &json!({"id": "G-UNDERGROUND"})).unwrap();
    let rule = &r["rules"][0];
    assert_eq!(rule["enabled"], true, "{r}");
    assert_eq!(rule["source"], "built-in");
    assert_eq!(rule["severity"], "high");
    assert_eq!(rule["tolerance"]["depth_in"], 1.0, "{r}");
    assert_eq!(rule["tolerance"]["size_in"], 0.1, "{r}");
}

#[test]
fn a_callout_with_no_run_is_missing_and_a_shallow_run_fails_its_depth() {
    let mut s = job(true, json!([]));
    let (r, fs) = findings(&mut s);
    // The site plan's 48 IN below grade telephone conduits: nothing in the model.
    let tel = callout(&fs, "TELEPHONE");
    assert_eq!(tel["verdict"], "missing", "{tel}");
    assert_eq!(tel["measured"]["callout"]["sheet"], "ES-1");
    assert_eq!(tel["measured"]["callout"]["calls"]["min_depth_in"], 48.0, "{tel}");
    assert_eq!(tel["measured"]["callout"]["calls"]["count"], 2, "{tel}");
    assert_eq!(tel["measured"]["callout"]["calls"]["size_in"], 4.0, "{tel}");
    assert_eq!(tel["measured"]["callout"]["calls"]["serves"], "telecom", "{tel}");
    assert!(tel["handle"].is_null());
    assert!(tel["issue"].as_str().unwrap().contains("the note on ES-1"), "{tel}");
    // Keynote 1: its run is 24" deep, the note says at least 48".
    let k1 = callout(&fs, "FROM PANEL LP");
    assert_eq!(k1["verdict"], "fail", "{k1}");
    assert_eq!(k1["handle"], "K1");
    assert_eq!(k1["severity"], "high");
    assert_eq!(k1["measured"]["matched_by"], "its note built it", "{k1}");
    assert!(k1["issue"].as_str().unwrap().contains("K1 is 24\" deep (at least 48\" called)"), "{k1}");
    assert!(k1["issue"].as_str().unwrap().contains("keynote 1 on E-101"), "{k1}");
    // Keynote 2: 1" PVC 18" down, listed on the generated sheet's run schedule.
    let k2 = callout(&fs, "FLOOR BOX");
    assert_eq!(k2["verdict"], "pass", "{k2}");
    assert_eq!(k2["handle"], "K2");
    assert_eq!(k2["measured"]["sheets"], json!(["E-UG1"]), "{k2}");
    assert_eq!(k2["area"], "L1/100", "{k2}");
    // The legend's line type is no callout.
    let notes = &r["unmeasured"]["G-UNDERGROUND"];
    assert!(notes.as_object().is_some_and(|o| o.keys().any(|k| k.contains("legend"))), "{r}");
    assert!(!fs.iter().any(|f| f["measured"]["callout"]["text"].as_str().is_some_and(|t| t.contains("COLD WATER"))), "{r}");
    // Tallies: every verdict listed.
    let c = &r["checked"]["G-UNDERGROUND"];
    for v in ["pass", "fail", "missing", "extra", "proposed", "unsure"] {
        assert!(c[v].is_u64(), "{v}: {r}");
    }
}

#[test]
fn a_proposed_feeder_is_reported_with_its_rfi_never_passed_and_an_unaccounted_run_is_extra() {
    let mut s = job(true, json!([]));
    let (r, fs) = findings(&mut s);
    let p = fs.iter().find(|f| f["verdict"] == "proposed").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(p["handle"], "F1", "{p}");
    assert_eq!(p["measured"]["review_item"]["id"], "RFI-U1", "{p}");
    assert!(p["issue"].as_str().unwrap().contains("not called by the documents"), "{p}");
    assert!(p["found"].as_str().unwrap().contains("dashed on E-UG1"), "{p}");
    assert_eq!(r["checked"]["G-UNDERGROUND"]["proposed"], 1, "{r}");
    // It answers no callout, and is never counted a pass.
    assert!(!fs.iter().any(|f| f["kind"] == "buried_run" && f["verdict"] == "pass" && f["handle"] == "F1"), "{r}");
    let x = fs.iter().find(|f| f["verdict"] == "extra").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(x["handle"], "X1", "{x}");
    // Keynote runs, the proposal and the overhead feeder are no extras.
    assert_eq!(fs.iter().filter(|f| f["verdict"] == "extra").count(), 1, "{r}");
}

#[test]
fn stub_ups_need_two_dimensions_from_the_slab_or_the_grid() {
    // With the slab's edges: the panels' stub-ups are dimensioned both ways.
    let mut s = job(true, json!([]));
    let (r, fs) = findings(&mut s);
    let stubs: Vec<&Value> = fs.iter().filter(|f| f["kind"] == "stub_up").collect();
    let at_lp =
        stubs.iter().find(|f| f["handle"] == "K1" && f["at"][0].as_f64().is_some_and(|x| (x - 4.0).abs() < 0.6)).unwrap_or_else(|| panic!("{r}"));
    assert_eq!(at_lp["verdict"], "pass", "{at_lp}");
    assert_eq!(at_lp["measured"]["sheet"], "E-UG1", "{at_lp}");
    assert!(at_lp["measured"]["x"]["from"].as_str().unwrap().starts_with("slab"), "{at_lp}");
    // The floor box's riser isn't on the generated stub-up schedule: nothing dimensions it.
    let fb = stubs
        .iter()
        .find(|f| f["handle"] == "K2:up0" || f["measured"]["runs"].as_array().is_some_and(|a| a.iter().any(|x| x == "K2:up0")))
        .unwrap_or_else(|| panic!("{r}"));
    assert_eq!(fb["verdict"], "fail", "{fb}");
    assert!(fb["issue"].as_str().unwrap().contains("isn't on E-UG1's stub-up schedule"), "{fb}");

    // No shell to measure from, a grid of numbered lines only: the stub-ups have one dimension each.
    let cols = json!([
        {"level": "L1", "grid": "1", "at": at([4.0, 0.0]), "size_in": [12, 12]}, {"level": "L1", "grid": "1", "at": at([4.0, 24.0]), "size_in": [12, 12]},
        {"level": "L1", "grid": "2", "at": at([32.0, 0.0]), "size_in": [12, 12]}, {"level": "L1", "grid": "2", "at": at([32.0, 24.0]), "size_in": [12, 12]}
    ]);
    let mut s = job(false, cols);
    let (r, fs) = findings(&mut s);
    let one = fs
        .iter()
        .find(|f| f["kind"] == "stub_up" && f["handle"] == "K1" && f["at"][0].as_f64().is_some_and(|x| (x - 4.0).abs() < 0.6))
        .unwrap_or_else(|| panic!("{r}"));
    assert_eq!(one["verdict"], "fail", "{one}");
    assert!(one["issue"].as_str().unwrap().contains("has one dimension"), "{one}");
    assert!(one["measured"]["x"]["from"].as_str().is_some(), "{one}");
    assert!(one["measured"]["y"]["from"].is_null(), "{one}");
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vunder-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

#[test]
fn a_level_one_packet_carries_the_generated_underground_sheet() {
    let mut s = job(true, json!([]));
    let out = dir("lobby");
    s.execute("validate.packet", &json!({"area": "L1/100", "out_dir": out.to_string_lossy()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    let sheet = m["prints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "underground_sheet" && c["sheet"] == "E-UG1")
        .unwrap_or_else(|| panic!("no underground sheet picture: {m}"));
    let file = out.join(sheet["file"].as_str().unwrap());
    assert!(std::fs::metadata(&file).is_ok_and(|f| f.len() > 1000), "{}", file.display());
    let o = read_json(&out.join("objects.json"));
    let ug = &o["underground"];
    assert_eq!(ug["sheets"][0]["number"], "E-UG1", "{ug}");
    assert_eq!(ug["sheets"][0]["generated"], true, "{ug}");
    let texts: Vec<&str> = ug["callouts"].as_array().unwrap().iter().filter_map(|c| c["text"].as_str()).collect();
    assert_eq!(texts.len(), 3, "{ug}");
    assert!(o["prescreen"]["findings"].as_array().unwrap().iter().any(|f| f["rule"] == "G-UNDERGROUND"), "{}", o["prescreen"]);
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(prompt.contains("`underground`"), "{prompt}");
    let rules = read_json(&out.join("rules.json"));
    assert!(rules["rules"].as_array().unwrap().iter().any(|r| r["id"] == "G-UNDERGROUND"), "{rules}");
    let _ = std::fs::remove_dir_all(&out);
}
