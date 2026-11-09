//! A door swing a print joined to its leaf and the wall's lines is still a door (Q6289). A PDF
//! importer joins touching strokes of one pen end to end, so on a trade plan drawn over the
//! architectural background most swings arrive as one piece of a longer polyline: the leaf from
//! the hinge, the arc, and on along the jamb. The walls were built, but no door was cut where
//! the swing hung, and a whole floor came out with one door.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

fn pline(s: &mut Session, pts: &[[f64; 2]]) {
    s.execute("pline", &json!({"points": pts})).unwrap();
}

/// A quarter circle about `c` of radius `r` from `a0` degrees, flattened to 8 chords (a Bézier as
/// a PDF importer flattens it).
fn quarter(c: [f64; 2], r: f64, a0: f64) -> Vec<[f64; 2]> {
    (0..=8).map(|i| (a0 + f64::from(i) * 90.0 / 8.0).to_radians()).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect()
}

/// A 40' x 30' floor in inches, 6" outside walls, and 4" partitions at each `x` with an opening
/// from y 150 `open` wide.
fn floor(s: &mut Session, partitions: &[f64], open: f64) {
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    rect(s, 0.0, 0.0, 480.0, 360.0);
    rect(s, 6.0, 6.0, 474.0, 354.0);
    for &x in partitions {
        for x in [x - 2.0, x + 2.0] {
            line(s, [x, 6.0], [x, 150.0]);
            line(s, [x, 150.0 + open], [x, 354.0]);
        }
    }
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
}

/// The swing of a 36" door hinged at the partition's east face (`x` + 2, 150): drawn as a print
/// joins it, the open leaf from the hinge, the arc to the other jamb, and on along that jamb.
fn joined_swing(x: f64) -> Vec<[f64; 2]> {
    let h = [x + 2.0, 150.0];
    let mut pts = vec![h];
    pts.extend(quarter(h, 36.0, 0.0));
    pts.push([x + 2.0, 220.0]);
    pts
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-DOOR"})).unwrap();
    s
}

fn build(s: &mut Session) -> Value {
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 8.0, "region": [-200.0, -200.0, 800.0, 600.0]}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels})).unwrap();
    s.execute("model.build", &json!({})).unwrap()
}

fn count(r: &Value, k: &str) -> u64 {
    r[k].as_u64().unwrap_or_else(|| panic!("{k} in {r}"))
}

fn door_openings(s: &mut Session) -> usize {
    let r = s.execute("plan.openings", &json!({"limit": 1000})).unwrap();
    r["items"].as_array().map_or(0, |a| a.iter().filter(|o| o["kind"] == "door" && o["swing"].is_number()).count())
}

#[test]
fn a_swing_joined_to_its_leaf_and_jamb_cuts_its_door() {
    // Drawn apart (the arc on its own), as a CAD file has it: one door.
    let mut apart = session();
    floor(&mut apart, &[240.0], 36.0);
    pline(&mut apart, &quarter([242.0, 150.0], 36.0, 0.0));
    let r0 = build(&mut apart);
    assert_eq!(count(&r0, "doors"), 1, "{r0}");

    // Joined to its leaf and the jamb's line: the same door.
    let mut s = session();
    floor(&mut s, &[240.0], 36.0);
    pline(&mut s, &joined_swing(240.0));
    assert_eq!(door_openings(&mut s), 1);
    let r = build(&mut s);
    assert_eq!(count(&r, "doors"), 1, "{r}");
    assert_eq!(count(&r, "walls"), count(&r0, "walls"), "{r}");
}

#[test]
fn every_joined_swing_of_a_floor_is_a_door() {
    // Three partitions, each door's swing joined to its leaf and jamb, as a trade plan's
    // background arrives.
    let mut s = session();
    floor(&mut s, &[120.0, 240.0, 360.0], 36.0);
    for x in [120.0, 240.0, 360.0] {
        pline(&mut s, &joined_swing(x));
    }
    let r = build(&mut s);
    assert_eq!(count(&r, "doors"), 3, "{r}");
    // Each level's walls and openings, for the job's few-openings check.
    let l = &r["openings_by_level"][0];
    assert_eq!((l["level"].as_str(), l["doors"].as_u64(), l["windows"].as_u64()), (Some("Level 1"), Some(3), Some(0)), "{r}");
    assert!(l["walls"].as_u64().is_some_and(|w| w > 0), "{r}");
}

#[test]
fn a_pairs_two_swings_in_one_polyline_are_both_read() {
    // A 72" pair: hinges at 150 and 222, the leaves meeting in the middle, drawn leaf to leaf.
    let mut s = session();
    floor(&mut s, &[240.0], 72.0);
    let mut pts = vec![[242.0, 150.0]];
    pts.extend(quarter([242.0, 150.0], 36.0, 0.0));
    pts.extend(quarter([242.0, 222.0], 36.0, 270.0).into_iter().skip(1));
    pts.push([242.0, 222.0]);
    pline(&mut s, &pts);
    assert_eq!(door_openings(&mut s), 2);
    let r = build(&mut s);
    assert!(count(&r, "doors") >= 1, "{r}");
}
