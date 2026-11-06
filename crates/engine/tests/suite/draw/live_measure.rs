//! Live measurements on the drawing: DIST, AREA, MEASUREGEOM and the Angle tool label what they
//! measure beside the geometry while points are picked, in the drawing's units; the labels go when
//! the command ends, MEASURELABELS 0 turns them off and 2 keeps them as multiline text.

use cadaddy_engine::{LiveLabel, Session};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

fn session(lines: &[&str]) -> Session {
    let mut s = Session::new();
    for l in lines {
        s.cmdline(l).unwrap();
    }
    s
}

fn setvar(s: &mut Session, name: &str, value: Value) {
    s.execute("setvar", &json!({ "name": name, "value": value })).unwrap();
}

fn texts(labels: &[LiveLabel]) -> Vec<String> {
    labels.iter().map(LiveLabel::text).collect()
}

fn mtexts(s: &Session) -> Vec<String> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::MText(m) => Some(m.contents.replace("\\P", "\n")),
            _ => None,
        })
        .collect()
}

#[test]
fn dist_labels_the_rubber_band_and_clears_when_done() {
    let mut s = session(&["dist", "0,0"]);
    let c = Vec2::new(3.0, 4.0);
    let l = s.live_labels(c);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].at, Vec2::new(1.5, 2.0), "at the middle of the segment");
    assert_eq!(l[0].text(), "Distance = 5.0000\nAngle = 53°\nDelta X = 3.0000\nDelta Y = 4.0000");
    // The label follows the cursor.
    assert_eq!(s.live_labels(Vec2::new(0.0, 10.0))[0].lines[0].1, "10.0000");
    s.cmdline("3,4").unwrap();
    assert!(s.running.is_none());
    assert!(s.live_labels(c).is_empty(), "gone when the command ends");
    assert!(mtexts(&s).is_empty(), "nothing left on the drawing by default");

    // Units and precision follow the drawing (architectural here).
    let mut s = Session::new();
    setvar(&mut s, "LUNITS", json!(4));
    s.cmdline("dist").unwrap();
    s.cmdline("0,0").unwrap();
    assert_eq!(s.live_labels(Vec2::new(30.0, 0.0))[0].lines[0], ("Distance", "2'-6\"".to_string()));
}

#[test]
fn dist_multiple_points_labels_the_segment_and_running_total() {
    let mut s = session(&["dist", "0,0", "m", "3,4"]);
    let l = s.live_labels(Vec2::new(3.0, 10.0));
    assert_eq!(texts(&l), ["Segment = 6.0000\nTotal = 11.0000"]);
    assert_eq!(l[0].at, Vec2::new(3.0, 7.0));
    // The Length option has no cursor geometry to label.
    s.cmdline("l").unwrap();
    assert!(s.live_labels(Vec2::new(3.0, 10.0)).is_empty());
    s.cmdline("6").unwrap();
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
}

#[test]
fn area_labels_the_running_area_and_total() {
    let mut s = session(&["area", "0,0"]);
    // One side so far: its distance.
    assert_eq!(s.live_labels(Vec2::new(10.0, 0.0))[0].lines[0], ("Distance", "10.0000".to_string()));
    s.cmdline("10,0").unwrap();
    s.cmdline("10,10").unwrap();
    let l = s.live_labels(Vec2::new(0.0, 10.0));
    assert_eq!(texts(&l), ["Area = 100.0000\nPerimeter = 40.0000"]);
    assert_eq!(l[0].at, Vec2::new(5.0, 5.0), "at the middle of the boundary");
    s.cmdline("0,10").unwrap();
    s.cmdline("").unwrap();
    assert!(s.running.is_none());

    // Add mode: the running total with the area being picked.
    let mut s = session(&["area", "a", "0,0", "10,0", "10,10", "0,10", "", "0,20", "4,20"]);
    let l = s.live_labels(Vec2::new(4.0, 25.0));
    assert_eq!(texts(&l), ["Area = 10.0000\nPerimeter = 15.4031\nTotal area = 110.0000"]);
    // Subtract mode takes it away (two points enclose nothing: Enter adds no area).
    s.cmdline("").unwrap();
    s.cmdline("s").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("1,0").unwrap();
    let l = s.live_labels(Vec2::new(1.0, 1.0));
    assert!(texts(&l)[0].ends_with("Total area = 99.5000"), "{l:?}");
    // Selecting objects has no cursor geometry.
    let s = session(&["area", "o"]);
    assert!(s.live_labels(Vec2::new(1.0, 1.0)).is_empty());
}

#[test]
fn measuregeom_previews_and_labels_distance_angle_and_area() {
    let mut s = session(&["measuregeom", "d", "0,0"]);
    let c = Vec2::new(6.0, 8.0);
    assert_eq!(s.preview(c).len(), 1, "the rubber band");
    assert_eq!(s.live_labels(c)[0].lines[0], ("Distance", "10.0000".to_string()));
    s.cmdline("6,8").unwrap();
    assert!(s.log.iter().any(|l| l.starts_with("Distance = 10.0000")), "{:?}", s.log);
    assert!(s.live_labels(c).is_empty(), "back at the option prompt");

    s.cmdline("a").unwrap();
    s.cmdline("0,0").unwrap();
    assert!(s.live_labels(c).is_empty(), "no angle from the vertex alone");
    assert_eq!(s.preview(c).len(), 1);
    s.cmdline("10,0").unwrap();
    assert_eq!(s.preview(Vec2::new(0.0, 5.0)).len(), 2, "both sides");
    assert_eq!(texts(&s.live_labels(Vec2::new(0.0, 5.0))), ["Angle = 90°"]);
    assert_eq!(texts(&s.live_labels(Vec2::new(5.0, 5.0))), ["Angle = 45°"]);
    s.cmdline("5,5").unwrap();
    assert!(s.log.iter().any(|l| l.starts_with("Angle = 45")), "{:?}", s.log);

    s.cmdline("ar").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("4,0").unwrap();
    let l = s.live_labels(Vec2::new(4.0, 3.0));
    assert_eq!(texts(&l), ["Area = 6.0000\nPerimeter = 12.0000"]);
    assert_eq!(s.preview(Vec2::new(4.0, 3.0)).len(), 1, "the closed boundary");
    s.cmdline("x").unwrap();
    s.cmdline("x").unwrap();

    // MEASURELABELS 0: only the command line reports.
    let mut s = session(&["measuregeom", "d", "0,0"]);
    setvar(&mut s, "MEASURELABELS", json!(0));
    assert!(s.live_labels(c).is_empty());
    assert_eq!(s.preview(c).len(), 1, "the rubber band stays");
}

#[test]
fn the_angle_tool_measures_once_with_a_live_label() {
    let mut s = session(&["angle.measure", "0,0", "10,0"]);
    assert_eq!(texts(&s.live_labels(Vec2::new(0.0, 3.0))), ["Angle = 90°"]);
    s.cmdline("0,3").unwrap();
    assert!(s.log.iter().any(|l| l == "Angle = 90°"), "{:?}", s.log);
    assert!(s.running.is_none(), "one measurement, then done");
    // Enter before the last point ends it too.
    let mut s = session(&["angle.measure", "0,0"]);
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
}

#[test]
fn quick_labels_its_distances_and_area() {
    let s = session(&["rectang 0,0 10,10", "measuregeom", "q"]);
    let l = s.live_labels(Vec2::new(2.0, 4.0));
    let t = texts(&l);
    for d in ["2.0000", "8.0000", "4.0000", "6.0000"] {
        assert!(t.iter().any(|x| x == d), "{d}: {t:?}");
    }
    assert!(t.iter().any(|x| x == "Area = 100.0000\nPerimeter = 40.0000"), "{t:?}");
    // The distance to the left wall sits on its line, halfway.
    assert!(l.iter().any(|x| x.text() == "2.0000" && x.at == Vec2::new(1.0, 4.0)), "{l:?}");
}

#[test]
fn measurelabels_two_keeps_the_final_labels_as_mtext() {
    let mut s = Session::new();
    setvar(&mut s, "MEASURELABELS", json!(2));
    for l in ["dist", "0,0", "3,4"] {
        s.cmdline(l).unwrap();
    }
    assert_eq!(mtexts(&s), ["Distance = 5.0000\nAngle = 53°\nDelta X = 3.0000\nDelta Y = 4.0000"]);
    let d = s.doc().unwrap();
    let Some(cadaddy_doc::EntityKind::MText(m)) = d.model.iter().last().map(|e| e.kind.clone()) else { panic!("an mtext") };
    assert_eq!((m.insert.x, m.insert.y, m.attach), (1.5, 2.0, 5), "centred where the label was");
    assert!(m.height > 0.0);
    // One undo takes it away.
    s.cmdline("u").unwrap();
    assert!(mtexts(&s).is_empty());

    for l in ["area", "0,0", "10,0", "10,10", ""] {
        s.cmdline(l).unwrap();
    }
    assert_eq!(mtexts(&s), ["Area = 50.0000\nPerimeter = 34.1421"]);
    for l in ["measuregeom", "a", "0,0", "10,0", "0,10", "x"] {
        s.cmdline(l).unwrap();
    }
    assert!(mtexts(&s).iter().any(|t| t == "Angle = 90°"), "{:?}", mtexts(&s));

    // Measured transparently inside another command, nothing is left behind.
    let mut s = Session::new();
    setvar(&mut s, "MEASURELABELS", json!(2));
    for l in ["line", "0,0", "'dist", "0,0", "5,0"] {
        s.cmdline(l).unwrap();
    }
    assert!(s.live_labels(Vec2::new(1.0, 1.0)).is_empty(), "DIST ended; LINE has no labels");
    s.cmdline("10,0").unwrap();
    s.cmdline("").unwrap();
    assert!(mtexts(&s).is_empty(), "{:?}", mtexts(&s));
}

#[test]
fn measurelabels_is_a_checked_setting() {
    let mut s = Session::new();
    assert_eq!(s.execute("getvar", &json!({ "name": "MEASURELABELS" })).unwrap()["value"], json!(1));
    assert!(s.execute("setvar", &json!({ "name": "MEASURELABELS", "value": 3 })).is_err());
    assert!(s.execute("setvar", &json!({ "name": "MEASURELABELS", "value": -1 })).is_err());
    setvar(&mut s, "MEASURELABELS", json!(0));
    assert_eq!(s.settings.measurelabels, 0);
}

#[test]
fn hostile_points_never_panic() {
    let big = f64::MAX / 2.0;
    let mut s = Session::new();
    setvar(&mut s, "MEASURELABELS", json!(2));
    for cmd in ["dist", "area", "measuregeom", "angle.measure"] {
        s.cmdline(cmd).unwrap();
        if cmd == "measuregeom" {
            s.cmdline("ar").unwrap();
        }
        for p in [format!("{big},{big}"), format!("-{big},{big}"), "0,0".into()] {
            let _ = s.cmdline(&p);
            for c in [Vec2::new(big, -big), Vec2::new(f64::NAN, 1.0), Vec2::new(f64::INFINITY, 0.0)] {
                let _ = s.live_labels(c);
                let _ = s.preview(c);
            }
        }
        s.cancel();
    }
    assert!(s.log.iter().all(|l| !l.contains("Internal error")), "{:?}", s.log);
}
