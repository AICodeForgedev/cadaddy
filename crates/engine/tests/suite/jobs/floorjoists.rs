//! Floor joists by level and by span (Q6400): a made-up four-storey building with a
//! double-loaded corridor whose three framed floors' joists differ in depth by level (the
//! framing plans' notes, a joist layout's level headings) and span between the corridor walls,
//! one joist per span.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// Exterior wall and corridor wall thickness, inches.
const T: f64 = 7.25;
const C: f64 = 5.5;
/// The plate: 80' long, 62' wide.
const LEN: f64 = 960.0;
const WID: f64 = 744.0;
/// The corridor walls' centrelines, 6' apart about the middle (28' and 34' from the south wall).
const CORRIDOR: [f64; 2] = [336.0, 408.0];

/// One storey at `ox`: the exterior walls, the corridor's two walls the whole length, and a
/// closet wall 8' long across the joists (no bearing line).
fn storey(s: &mut Session, ox: f64) {
    layer(s, "A-WALL");
    rect(s, [ox, 0.0], [ox + LEN, WID]);
    rect(s, [ox + T, T], [ox + LEN - T, WID - T]);
    for y in CORRIDOR {
        for dy in [-C / 2.0, C / 2.0] {
            line(s, [ox + T, y + dy], [ox + LEN - T, y + dy]);
        }
    }
    for dy in [-C / 2.0, C / 2.0] {
        line(s, [ox + 120.0, 120.0 + dy], [ox + 216.0, 120.0 + dy]);
    }
}

/// The building, its floors framed as `floor_framing` says (the job's framing plans' notes).
fn building(floor_framing: Value) -> Session {
    let mut s = Session::new();
    let names = ["1", "2", "3", "4"];
    let mut levels = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let ox = i as f64 * 2000.0;
        storey(&mut s, ox);
        levels.push(
            json!({"name": name, "elevation_ft": i as f64 * 10.0, "height_ft": 10, "ceiling_ft": 9, "region": [ox - 100.0, -100, ox + 1100.0, 900]}),
        );
    }
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "stack": true,
        "levels": levels,
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs"}],
        "floor_framing": floor_framing,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn note(level: &str, depth: Option<f64>, text: &str) -> Value {
    let mut v =
        json!({"level": level, "member": "joist", "spacing_in": 24.0, "depth_in": depth.unwrap_or(11.875), "spacing_status": "read", "source": text});
    v["depth_status"] = json!(if depth.is_some() { "read" } else { "default" });
    v
}

/// A mesh element's plan box: (lo, hi) in drawing units.
fn bbox(e: &cadaddy_doc::Entity) -> Option<([f64; 2], [f64; 2])> {
    let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for v in &m.vertices {
        for (i, c) in [v.x, v.y].into_iter().enumerate() {
            lo[i] = lo[i].min(c);
            hi[i] = hi[i].max(c);
        }
    }
    lo[0].is_finite().then_some((lo, hi))
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).unwrap()
}

fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

/// A level's joists by size: (size, count) rows.
fn sizes(s: &mut Session, level: &str) -> Vec<(String, f64)> {
    let c = count(s, json!({"trade": "framing", "kind": "joist", "level": level, "group_by": "size"}));
    c["rows"].as_array().cloned().unwrap_or_default().iter().map(|r| (r["size"].as_str().unwrap_or_default().to_string(), n(r, "count"))).collect()
}

/// Joist lines at 24" across 80', 2.5" in from the ends: 40.
const LINES: f64 = 40.0;

/// Each joist line is three spans: 28' to the corridor, the 6' corridor, 28' beyond.
fn assert_spans(s: &mut Session, level: &str) {
    let all = count(s, json!({"trade": "framing", "kind": "joist", "level": level}));
    assert_eq!(n(&all, "count"), 3.0 * LINES, "level {level}: one joist per span: {all}");
    // The length is the floor's width per line, as before.
    assert!((n(&all, "length_ft") - LINES * WID / 12.0).abs() < 1.0, "level {level}: {all}");
    let d = s.doc().unwrap();
    let mut lens: Vec<f64> = d
        .model
        .iter()
        .filter(|e| e.common.xdata.iter().any(|t| t.value == "part:atom:joist"))
        .filter(|e| e.common.xdata.get(2).is_some_and(|t| t.value == level))
        .filter_map(|e| bbox(e))
        .map(|(lo, hi)| ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 12.0 * 2.0).round() / 2.0)
        .collect();
    lens.sort_by(f64::total_cmp);
    lens.dedup();
    assert_eq!(lens, vec![6.0, 28.0], "level {level}: spans of 28' and 6' (the corridor): {lens:?}");
}

/// The framing plans give level 2 16" joists and levels 3 and 4 11 7/8": each floor is built with
/// its own, one joist per span between the exterior and corridor walls (before: every floor one
/// depth, one 62' piece wall to wall per line).
#[test]
fn each_floor_has_its_framing_plans_depth_and_a_joist_per_span() {
    let mut s = building(json!([
        note("2", Some(16.0), "S-2: 16\" I-JOISTS @ 24\" OC TYP."),
        note("3", Some(11.875), "S-3: 11 7/8\" I-JOISTS @ 24\" OC TYP."),
        note("4", Some(11.875), "S-3: 11 7/8\" I-JOISTS @ 24\" OC TYP."),
    ]));
    assert_eq!(sizes(&mut s, "2"), vec![("16\"".to_string(), 3.0 * LINES)]);
    for level in ["3", "4"] {
        assert_eq!(sizes(&mut s, level), vec![("11 7/8\"".to_string(), 3.0 * LINES)], "level {level}");
    }
    for level in ["2", "3", "4"] {
        assert_spans(&mut s, level);
    }
    // A cut joist says what it bears on, by the bearing rule.
    let h = {
        let d = s.doc().unwrap();
        d.model
            .iter()
            .filter(|e| e.common.xdata.iter().any(|t| t.value == "part:atom:joist"))
            .filter_map(|e| bbox(e).map(|b| (e.handle, b)))
            .find(|(_, (lo, hi))| ((hi[1] - lo[1]) - 72.0).abs() < 1.0)
            .map(|(h, _)| h.hex())
            .unwrap()
    };
    let el = s.execute("bim.props.get", &json!({"handle": h})).unwrap()["elements"][0]["props"].to_string();
    assert!(el.contains("S-FLOOR-JOIST-BEARING") && el.contains("wall line"), "{el}");
    assert!(el.contains("S-2: 16") || el.contains("S-3: 11 7/8"), "the depth's note: {el}");
}

/// A joist layout's level headings give each floor its depth and series (its material list per
/// floor); a floor's plan note that differs is kept beside it as the design value; a floor neither
/// gives a depth is built at the default and says so.
#[test]
fn a_joist_layouts_level_headings_set_each_floor_and_a_floor_with_none_is_a_marked_default() {
    // Level 2's plan note says 14": the joist layout (the shop drawing) says 16".
    let mut s = building(json!([note("2", Some(14.0), "S-2: 14\" I-JOISTS @ 24\" OC TYP.")]));
    let dir = std::env::temp_dir().join(format!("cadaddy-floorjoists-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Sample Floor Joist Layout.txt"),
        "--- page 1 of 3 ---\nSAMPLE JOIST LAYOUT\n--- page 2 of 3 ---\n2ND FLOOR FRAMING\nJ2   XJ 60   2.5   16   120   28-0-0\n\
         J2- 80 @ 24\"\nJ3- 40 @ 24\"\nJ3   XJ 60   2.5   16   40   6-0-0\n2ND FLOOR FRAMING\n\
         --- page 3 of 3 ---\nLevel: THIRD & FOURTH FLOOR FRAMING\nJ5   XJ 40   2.5   11.875   240   28-0-0\nJ6   XJ 40   2.5   11.875   80   6-0-0\nJ5- 160 @ 24\"\nJ6- 80 @ 24\"\n",
    )
    .unwrap();
    let r = s.execute("bim.atoms.docs", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let facts = r["facts"].as_array().unwrap();
    let depth_of =
        |scope: &str| facts.iter().find(|f| f["key"] == json!("framing.joist_depth_in") && f["scope"] == json!(scope)).map(|f| f["number"].clone());
    assert_eq!(depth_of("level:2"), Some(json!(16.0)), "{r}");
    assert_eq!(depth_of("level:3"), Some(json!(11.875)));
    assert_eq!(depth_of("level:4"), Some(json!(11.875)));
    let b = s.execute("bim.atoms.build", &json!({})).unwrap();
    assert_eq!(sizes(&mut s, "2"), vec![("16\"".to_string(), 3.0 * LINES)], "{b}");
    for level in ["3", "4"] {
        assert_eq!(sizes(&mut s, level), vec![("11 7/8\"".to_string(), 3.0 * LINES)], "level {level}: {r}");
    }
    let series = count(&mut s, json!({"trade": "framing", "kind": "joist", "level": "3", "group_by": "type"}));
    assert_eq!(series["rows"][0]["type"], json!("XJ 40"), "{series}");
    let series2 = count(&mut s, json!({"trade": "framing", "kind": "joist", "level": "2", "group_by": "type"}));
    assert_eq!(series2["rows"][0]["type"], json!("XJ 60"), "{series2}");
    let notes = b.to_string();
    assert!(notes.contains("the design value 14\\\""), "the plan's 14\" kept as the design value: {notes}");
    // Each level's report compares its own layout's joists: level 3's 240 listed beside its 120
    // spans' worth (the layout's labels: 160 + 80).
    let rep = s.execute("bim.atoms.report", &json!({"level": "3"})).unwrap();
    let row = rep["compare"].as_array().unwrap().iter().find(|c| c["what"] == json!("floor joists")).cloned().unwrap();
    assert_eq!((row["documents"].clone(), row["model"].clone(), row["level"].clone()), (json!(240.0), json!(120.0), json!("3")), "{rep}");
    let _ = std::fs::remove_dir_all(&dir);

    // No layout and plan notes for levels 2 and 3 only: level 4 is the default, marked so.
    let mut s = building(json!([note("2", Some(16.0), "S-2: 16\" I-JOISTS @ 24\" OC"), note("3", Some(14.0), "S-3: 14\" I-JOISTS @ 24\" OC")]));
    assert_eq!(sizes(&mut s, "3"), vec![("14\"".to_string(), 3.0 * LINES)]);
    // Level 4: the default depth and spacing (16": 60 lines across 80').
    assert_eq!(sizes(&mut s, "4"), vec![("11 7/8\"".to_string(), 3.0 * 60.0)]);
    let b = s.execute("bim.atoms.build", &json!({})).unwrap().to_string();
    assert!(b.contains("4: the documents give no joist depth for this floor"), "{b}");
}
