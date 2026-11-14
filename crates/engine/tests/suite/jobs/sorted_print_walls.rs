//! Q6487: on a PDF sheet the import sorted by what it draws, the heavy cut lines (the walls) are
//! on its wall class layer; the counters, equipment and the walls' finish lines it left on its
//! other layers pair up like wall faces but are no walls. Taken as walls they boxed a small
//! building's kitchen in at its counters: rooms built at a fraction of their area. A stretch of
//! wall drawn lighter in line with a wall, a wing beyond the sorted walls, and partitions drawn
//! lighter inside them (each parting two rooms, Q6627) stay walls.

use cadaddy_engine::Session;
use serde_json::json;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6.0})).unwrap();
}

/// The counters and equipment of a 30' x 16' building (inches), on `other`: a counter run (two
/// faces 4 3/4" apart) closing the kitchen's lower left corner between the partition and the
/// outside wall, and an equipment box with a liner 4" inside it.
fn casework(s: &mut Session, other: &str) {
    layer(s, other);
    line(s, [102.0, 100.0], [250.0, 100.0]);
    line(s, [102.0, 104.75], [254.75, 104.75]);
    line(s, [250.0, 6.0], [250.0, 100.0]);
    line(s, [254.75, 6.0], [254.75, 104.75]);
    rect(s, [280.0, 120.0], [340.0, 176.0]);
    rect(s, [284.0, 124.0], [336.0, 172.0]);
}

/// The room tags: RESTROOM 703 behind the partition, KITCHEN 702 in the corner the counter closes.
fn tags(s: &mut Session) {
    layer(s, "TEXT");
    text(s, [30.0, 110.0], "RESTROOM");
    text(s, [30.0, 95.0], "703");
    text(s, [150.0, 60.0], "KITCHEN");
    text(s, [150.0, 45.0], "702");
}

/// A 30' x 16' building, 6" walls on `walls`: a 7'-6" wide room on the left behind a full-height
/// partition, a 21' kitchen on the right; its casework on `other`.
fn building(walls: &str, other: &str) -> Session {
    let mut s = Session::new();
    layer(&mut s, walls);
    rect(&mut s, [0.0, 0.0], [360.0, 192.0]);
    rect(&mut s, [6.0, 6.0], [96.0, 186.0]);
    rect(&mut s, [102.0, 6.0], [354.0, 186.0]);
    casework(&mut s, other);
    tags(&mut s);
    s
}

/// Built, each room's floor area in square feet by number.
fn areas(s: &mut Session) -> Vec<(String, f64)> {
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("rooms.list", &json!({})).unwrap();
    let mut out: Vec<(String, f64)> = r["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["number"].as_str().unwrap_or("").to_string(), x["areaSqFt"].as_f64().unwrap_or(0.0)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn area(got: &[(String, f64)], number: &str) -> f64 {
    got.iter().find(|(n, _)| n == number).map_or(0.0, |(_, a)| *a)
}

/// The planned walls of Level 1: centreline ends.
fn planned(s: &mut Session) -> Vec<([f64; 2], [f64; 2])> {
    let r = s.execute("wallgen.plan", &json!({"level": "Level 1"})).unwrap();
    let p = |v: &serde_json::Value| [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()];
    r["walls"].as_array().unwrap().iter().map(|w| (p(&w["a"]), p(&w["b"]))).collect()
}

#[test]
fn import_sorted_casework_is_no_wall_and_rooms_reach_their_walls() {
    let walls = cadaddy_io::pdf_read::content_wall_layer();
    // The import's catch-all layer and its thin-stroke layer.
    for other in ["PDF-GEOMETRY", "ANNO"] {
        assert!(cadaddy_io::pdf_read::is_content_leftover_layer(other), "{other}");
        let mut s = building(walls, other);
        let got = areas(&mut s);
        // The kitchen to its walls: 21' x 15' = 315 sf, not the 96 sf corner its counter closes.
        let kitchen = area(&got, "702");
        assert!((kitchen - 315.0).abs() <= 0.03 * 315.0, "{other}: kitchen {kitchen} sf: {got:?}");
        // The room behind the partition: 7'-6" x 15'.
        let rest = area(&got, "703");
        assert!((rest - 112.5).abs() <= 0.03 * 112.5, "{other}: room 703 {rest} sf: {got:?}");
        // No wall along the counter.
        let along = planned(&mut s).iter().filter(|(a, b)| (a[1] - 102.4).abs() < 3.0 && (b[1] - 102.4).abs() < 3.0).count();
        assert_eq!(along, 0, "{other}: {:?}", planned(&mut s));
    }
}

/// The outside wall's last 10' drawn lighter (on the leftover layer, in line with the heavy wall
/// and meeting its end): still the outside wall, though the layer holds casework too.
#[test]
fn a_stretch_of_wall_drawn_lighter_in_line_stays_a_wall() {
    let walls = cadaddy_io::pdf_read::content_wall_layer();
    let mut s = Session::new();
    layer(&mut s, walls);
    // The outside wall's faces, the bottom one only to x 240.
    line(&mut s, [0.0, 192.0], [360.0, 192.0]);
    line(&mut s, [0.0, 186.0], [360.0, 186.0]);
    line(&mut s, [0.0, 0.0], [0.0, 192.0]);
    line(&mut s, [6.0, 6.0], [6.0, 186.0]);
    line(&mut s, [360.0, 0.0], [360.0, 192.0]);
    line(&mut s, [354.0, 6.0], [354.0, 186.0]);
    line(&mut s, [0.0, 0.0], [240.0, 0.0]);
    line(&mut s, [6.0, 6.0], [240.0, 6.0]);
    line(&mut s, [240.0, 0.0], [240.0, 6.0]);
    rect(&mut s, [96.0, 6.0], [102.0, 186.0]);
    layer(&mut s, "PDF-GEOMETRY");
    line(&mut s, [240.0, 0.0], [360.0, 0.0]);
    line(&mut s, [240.0, 6.0], [354.0, 6.0]);
    casework(&mut s, "PDF-GEOMETRY");
    tags(&mut s);
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let casework = sp["spec"]["layers"].as_array().unwrap().iter().any(|l| l["layer"] == "PDF-GEOMETRY");
    assert!(!casework, "the layer holds the kitchen's casework: no wall layer: {sp}");
    let walls = planned(&mut s);
    assert!(walls.iter().any(|(a, b)| (a[1] - 3.0).abs() < 1.0 && (b[1] - 3.0).abs() < 1.0 && a[0].max(b[0]) > 340.0), "{walls:?}");
}

/// A wing whose walls the import left on its catch-all layer (drawn with a lighter pen than the
/// rest: a storefront) keeps them: they lie beyond the sorted walls, and the wing's room is built.
#[test]
fn a_wing_drawn_with_a_lighter_pen_keeps_its_walls() {
    let walls = cadaddy_io::pdf_read::content_wall_layer();
    let mut s = building(walls, "PDF-GEOMETRY");
    layer(&mut s, "PDF-GEOMETRY");
    rect(&mut s, [360.0, 0.0], [600.0, 192.0]);
    rect(&mut s, [360.0, 6.0], [594.0, 186.0]);
    layer(&mut s, "TEXT");
    text(&mut s, [450.0, 100.0], "DINING");
    text(&mut s, [450.0, 85.0], "716");
    let got = areas(&mut s);
    // 19'-6" x 15'.
    let wing = area(&got, "716");
    assert!((wing - 292.5).abs() <= 0.05 * 292.5, "wing {wing} sf: {got:?}");
}

/// A print the import didn't sort (its walls on the catch-all layer, nothing on the wall class
/// layer) keeps its walls: face pairs make the layer walls as before.
#[test]
fn unsorted_print_keeps_walls_found_by_their_faces() {
    let mut s = building("PDF-GEOMETRY", "PDF-GEOMETRY");
    let got = areas(&mut s);
    assert!(area(&got, "703") > 100.0, "{got:?}");
}

/// Q6627: a row of rooms whose partitions the import left on its catch-all layer (drawn with a
/// lighter pen than the outside walls, inside them): each parts two room numbers with no sorted
/// wall between them, so the layer keeps its walls and every room is built to its partitions,
/// not one room the length of the building.
#[test]
fn partitions_drawn_lighter_inside_the_sorted_walls_stay_walls() {
    let walls = cadaddy_io::pdf_read::content_wall_layer();
    let mut s = Session::new();
    layer(&mut s, walls);
    // A 50' x 16' building, 6" outside walls.
    rect(&mut s, [0.0, 0.0], [600.0, 192.0]);
    rect(&mut s, [6.0, 6.0], [594.0, 186.0]);
    // Three 6" partitions, wall to wall, on the catch-all layer.
    layer(&mut s, "PDF-GEOMETRY");
    for x in [150.0, 300.0, 450.0] {
        line(&mut s, [x - 3.0, 6.0], [x - 3.0, 186.0]);
        line(&mut s, [x + 3.0, 6.0], [x + 3.0, 186.0]);
    }
    layer(&mut s, "TEXT");
    for (i, x) in [60.0, 210.0, 360.0, 510.0].into_iter().enumerate() {
        text(&mut s, [x, 110.0], "OFFICE");
        text(&mut s, [x, 95.0], &format!("80{}", i + 1));
    }
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let kept = sp["spec"]["layers"].as_array().unwrap().iter().any(|l| l["layer"] == "PDF-GEOMETRY" && l["role"] == "wall");
    assert!(kept, "the partitions' layer is walls: {}", sp["spec"]["layers"]);
    let got = areas(&mut s);
    // 11'-9" (end rooms) and 12' (middle rooms) by 15'.
    for (n, want) in [("801", 176.25), ("802", 180.0), ("803", 180.0), ("804", 176.25)] {
        let a = area(&got, n);
        assert!((a - want).abs() <= 0.05 * want, "room {n}: {a} sf, not {want}: {got:?}");
    }
}

/// Q6627: in the building above (casework on the catch-all layer), a storage room open to the
/// kitchen with a counter across the sightline between their numbers: the counter parts two of
/// the three numbers, but it is a small part of the layer's pairs (two equipment boxes too), so
/// the layer stays casework, no walls.
#[test]
fn one_counter_between_two_open_rooms_does_not_make_casework_walls() {
    let walls = cadaddy_io::pdf_read::content_wall_layer();
    let mut s = building(walls, "PDF-GEOMETRY");
    layer(&mut s, "PDF-GEOMETRY");
    rect(&mut s, [120.0, 120.0], [180.0, 176.0]);
    rect(&mut s, [124.0, 124.0], [176.0, 172.0]);
    layer(&mut s, "TEXT");
    text(&mut s, [300.0, 75.0], "STORAGE");
    text(&mut s, [300.0, 60.0], "704");
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let casework = sp["spec"]["layers"].as_array().unwrap().iter().any(|l| l["layer"] == "PDF-GEOMETRY");
    assert!(!casework, "the layer holds casework: no wall layer: {}", sp["spec"]["layers"]);
}
