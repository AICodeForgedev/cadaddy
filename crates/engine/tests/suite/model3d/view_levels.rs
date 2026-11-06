//! A sheet that prints two floor plans (`3RD FLOOR PLAN` under one, `4TH FLOOR PLAN` under the
//! other) drafts a level per plan view, each over its own view, and builds two storeys that the
//! BIM view stacks (Q5241 MUST 2; the real-print gate's `bim.levels`). A sheet of one plan, or of
//! two views of one floor, still drafts one level.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use serde_json::{Value, json};

/// The second plan's offset up the sheet (inches).
const UP: f64 = 1000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

/// A 40' × 30' floor at height `oy` on the sheet: 6" outside walls, a 4" partition with a 3'
/// door, a tagged room each side, and the view's title under it.
fn floor(s: &mut Session, oy: f64, number: usize, title: &str) {
    layer(s, "A-WALL");
    rect(s, 0.0, oy, 480.0, oy + 360.0);
    rect(s, 6.0, oy + 6.0, 474.0, oy + 354.0);
    for x in [238.0, 242.0] {
        line(s, [x, oy + 6.0], [x, oy + 150.0]);
        line(s, [x, oy + 186.0], [x, oy + 354.0]);
    }
    layer(s, "A-DOOR");
    s.execute("arc", &json!({"center": [240.0, oy + 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    layer(s, "A-AREA-IDEN");
    for (x, name, n) in [(100.0, "OFFICE", 1), (340.0, "LOBBY", 2)] {
        s.execute("text", &json!({"at": [x, oy + 180.0], "text": name, "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [x, oy + 160.0], "text": format!("{number}0{n}"), "height": 6.0})).unwrap();
    }
    layer(s, "A-ANNO-TTLB");
    s.execute("text", &json!({"at": [0.0, oy - 40.0], "text": title, "height": 24.0})).unwrap();
}

fn draft(s: &mut Session) -> Value {
    s.execute("bspec.draft", &json!({"store": true})).unwrap()
}

fn level_names(r: &Value) -> Vec<String> {
    r["spec"]["levels"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap().to_string()).collect()
}

/// The built walls of `level` (its WALLGEN tag after the part).
fn walls_on(s: &Session, level: &str) -> usize {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("wall"))
        .filter(|e| e.common.xdata.iter().filter(|t| t.code == 1000).nth(1).is_some_and(|t| t.value == level))
        .count()
}

#[test]
fn two_floor_plans_on_one_sheet_are_two_stacked_levels() {
    let mut s = Session::new();
    floor(&mut s, 0.0, 3, "3RD FLOOR PLAN");
    floor(&mut s, UP, 4, "4TH FLOOR PLAN");
    let r = draft(&mut s);
    assert_eq!(level_names(&r), ["Level 3", "Level 4"], "{}", r["spec"]["levels"]);
    let levels = r["spec"]["levels"].as_array().unwrap();
    // Elevations from the floor numbers, 10' floor to floor (a guess, said so).
    assert_eq!(levels[0]["elevation_ft"], json!(20.0));
    assert_eq!(levels[1]["elevation_ft"], json!(30.0));
    let guessed: Vec<&str> = r["guesses"].as_array().unwrap().iter().filter_map(|g| g["key"].as_str()).collect();
    assert!(guessed.contains(&"levels/Level 3") && guessed.contains(&"levels/Level 4"), "{guessed:?}");
    // Each level's region holds its own plan and not the other's.
    let region = |i: usize| -> Vec<f64> { levels[i]["region"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
    let inside = |r: &[f64], x: f64, y: f64| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3];
    assert!(inside(&region(0), 240.0, 180.0) && !inside(&region(0), 240.0, UP + 180.0), "{:?}", region(0));
    assert!(inside(&region(1), 240.0, UP + 180.0) && !inside(&region(1), 240.0, 180.0), "{:?}", region(1));
    // Rooms take their plan's level: the same room on both floors is two rooms (the door's
    // gap makes each floor's two halves one outline, named once).
    let rooms: Vec<(String, String)> = r["spec"]["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap_or("").to_string(), r["level"].as_str().unwrap_or("").to_string()))
        .collect();
    assert_eq!(rooms.len(), 2, "{rooms:?}");
    assert!(rooms[0].0 == rooms[1].0 && rooms.iter().any(|r| r.1 == "Level 3") && rooms.iter().any(|r| r.1 == "Level 4"), "{rooms:?}");

    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["levels"], json!(2), "{b}");
    let (w3, w4) = (walls_on(&s, "Level 3"), walls_on(&s, "Level 4"));
    assert!(w3 > 0 && w4 > 0 && w3 == w4, "walls per level {w3} / {w4}");
    // The BIM view stacks the 4th floor over the 3rd (its plan drawn 1000" up the sheet).
    let offsets = cadaddy_engine::cmd::bimlook::stack_offsets(s.doc().unwrap());
    let up = offsets.iter().find(|(n, _)| n == "Level 4").map(|o| o.1);
    assert!(up.is_some_and(|o| o.x.abs() < 1e-6 && (o.y + UP).abs() < 1e-6), "{offsets:?}");
}

#[test]
fn one_plan_or_two_views_of_one_floor_draft_one_level() {
    let mut s = Session::new();
    floor(&mut s, 0.0, 3, "3RD FLOOR PLAN");
    assert_eq!(level_names(&draft(&mut s)), ["Level 1"]);

    let mut s = Session::new();
    floor(&mut s, 0.0, 3, "3RD FLOOR POWER PLAN");
    floor(&mut s, UP, 3, "3RD FLOOR LIGHTING PLAN");
    assert_eq!(level_names(&draft(&mut s)), ["Level 1"]);

    // Levels given to the draft win over the views on the sheet.
    let mut s = Session::new();
    floor(&mut s, 0.0, 3, "3RD FLOOR PLAN");
    floor(&mut s, UP, 4, "4TH FLOOR PLAN");
    let r = s
        .execute(
            "bspec.draft",
            &json!({"levels": [{"name": "Third", "elevation_ft": 0.0, "height_ft": 12.0, "region": [-100.0, -100.0, 600.0, 2000.0]}]}),
        )
        .unwrap();
    assert_eq!(level_names(&r), ["Third"]);
}

#[test]
fn hostile_view_titles_never_panic() {
    for title in ["0TH FLOOR PLAN", "B0 FLOOR PLAN", "LEVEL 99999999999999999999 PLAN", "ROOF PLAN", "FLOORS 1-200", "\u{202e}3RD FLOOR"] {
        let mut s = Session::new();
        floor(&mut s, 0.0, 3, title);
        floor(&mut s, UP, 4, "4TH FLOOR PLAN");
        let r = draft(&mut s);
        assert!(!level_names(&r).is_empty(), "{title}");
    }
}
