//! Measure and inquiry polish: MEASUREGEOM Quick (live distances under the cursor), DIST, ID,
//! LIST, MEASUREGEOM and MASSPROP output in the drawing's units, AREA's Add and Subtract modes from
//! the command API, and QuickCalc's last-measurement values.

use cadaddy_engine::Session;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

fn run(lines: &[&str]) -> (Session, Vec<String>) {
    let mut s = Session::new();
    for l in lines {
        s.cmdline(l).unwrap();
    }
    let log = s.log.clone();
    (s, log)
}

fn has(log: &[String], text: &str) -> bool {
    log.iter().any(|l| l.contains(text))
}

fn last_handle(s: &Session) -> String {
    s.doc().unwrap().model.iter().last().unwrap().handle.hex()
}

fn setvar(s: &mut Session, name: &str, value: Value) {
    s.execute("setvar", &json!({ "name": name, "value": value })).unwrap();
}

#[test]
fn quick_mode_measures_around_the_cursor_and_echoes_on_click() {
    // A 10 x 10 square with a circle of radius 1 at its middle.
    let (mut s, log) = run(&["rectang 0,0 10,10", "circle 5,5 1", "measuregeom", "q", "2,4"]);
    // The circle's bottom touches the line through the point: 3 to the right.
    assert!(has(&log, "Left = 2.0000,  Right = 3.0000,  Down = 4.0000,  Up = 6.0000"), "{log:?}");
    assert!(has(&log, "Vertical = 10.0000"), "{log:?}");
    assert!(has(&log, "Area = 100.0000, Perimeter = 40.0000"), "the square encloses the point: {log:?}");
    assert!(s.running.is_some(), "Quick keeps measuring after a click");

    // Moving the cursor previews the distances: lines to the hits and their values as text.
    let pv = s.preview(Vec2::new(3.0, 2.0));
    let texts: Vec<String> = pv.iter().filter_map(|e| serde_json::to_value(&e.kind).ok()?.get("value")?.as_str().map(str::to_string)).collect();
    assert!(texts.iter().any(|t| t == "3.0000"), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("Area = 100")), "{texts:?}");
    assert!(pv.len() >= 4 * 3, "a line, an end tick and a value per side: {}", pv.len());

    // Inside the circle the smallest closed object is the circle.
    s.cmdline("5.5,5").unwrap();
    assert!(s.log.iter().any(|l| l.starts_with("Left = 1.5000,  Right = 0.5000")), "{:?}", s.log);
    assert!(s.log.iter().any(|l| l.starts_with("Area = 3.1416")), "{:?}", s.log);

    // Another measurement from Quick's prompt, then Enter ends the command.
    s.cmdline("d").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("3,4").unwrap();
    assert!(s.log.iter().any(|l| l.starts_with("Distance = 5.0000")), "{:?}", s.log);
    s.cmdline("x").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());

    // Quick ends on Enter too, and previews nothing in the other modes.
    let (mut s, _) = run(&["line 0,0 10,0", "", "measuregeom", "q"]);
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
    let (s, _) = run(&["line 0,0 10,0", "", "measuregeom", "r"]);
    assert!(s.preview(Vec2::new(1.0, 1.0)).is_empty());
}

#[test]
fn quick_from_the_command_api() {
    let mut s = Session::new();
    s.cmdline("rectang 0,0 10,10").unwrap();
    let r = s.execute("measuregeom", &json!({ "mode": "quick", "at": [3, 4] })).unwrap();
    assert_eq!((r["left"].as_f64(), r["right"].as_f64(), r["down"].as_f64(), r["up"].as_f64()), (Some(3.0), Some(7.0), Some(4.0), Some(6.0)));
    assert_eq!((r["horizontal"].as_f64(), r["vertical"].as_f64(), r["area"].as_f64()), (Some(10.0), Some(10.0), Some(100.0)));
    // Only as far as `reach`: nothing else within 3.5 below or to the right.
    let r = s.execute("measuregeom", &json!({ "mode": "quick", "at": [3, 4], "reach": 3.5 })).unwrap();
    assert_eq!(r["left"].as_f64(), Some(3.0));
    assert!(r["right"].is_null() && r["down"].is_null() && r["horizontal"].is_null(), "{r}");
    // Outside everything: no hits, no area.
    let r = s.execute("measuregeom", &json!({ "mode": "quick", "at": [50, 50], "reach": 5 })).unwrap();
    assert!(r["left"].is_null() && r["area"].is_null(), "{r}");
    // Text is not measured to.
    s.execute("text", &json!({ "at": [1, 4], "height": 0.5, "text": "LABEL" })).ok();
    let r = s.execute("measuregeom", &json!({ "mode": "quick", "at": [3, 4.1] })).unwrap();
    assert_eq!(r["left"].as_f64(), Some(3.0));
    // Hostile parameters are refused, not obeyed.
    for p in
        [json!({ "mode": "quick" }), json!({ "mode": "quick", "at": [0, 0], "reach": -1 }), json!({ "mode": "quick", "at": [0, 0], "reach": "far" })]
    {
        assert!(s.execute("measuregeom", &p).is_err(), "{p}");
    }
    assert!(s.execute("measuregeom", &json!({ "mode": "volume" })).is_err());
}

#[test]
fn outputs_follow_the_units_settings() {
    let mut s = Session::new();
    setvar(&mut s, "LUNITS", json!(4));
    setvar(&mut s, "LUPREC", json!(0));
    // DIST and ID in architectural units.
    let r = s.execute("dist", &json!({ "p1": [0, 0], "p2": [30, 0] })).unwrap();
    assert!(r["message"].as_str().unwrap().contains("Distance = 2'-6\""), "{r}");
    let r = s.execute("id", &json!({ "at": [30, 18] })).unwrap();
    assert!(r["message"].as_str().unwrap().contains("X = 2'-6\""), "{r}");
    // LIST of a line: its points and length.
    s.cmdline("line 0,0 24,0").unwrap();
    s.cmdline("").unwrap();
    let h = last_handle(&s);
    let r = s.execute("list", &json!({ "handles": [h] })).unwrap();
    assert!(r["message"].as_str().unwrap().contains("Length = 2'"), "{r}");
    // MEASUREGEOM Radius in architectural units, Angle in degrees/minutes/seconds.
    s.cmdline("circle 0,0 18").unwrap();
    let c = last_handle(&s);
    let r = s.execute("radius.measure", &json!({ "handle": c })).unwrap();
    assert!(r["message"].as_str().unwrap().contains("Radius = 1'-6\""), "{r}");
    assert!(r["message"].as_str().unwrap().contains("Diameter = 3'"), "{r}");
    setvar(&mut s, "AUNITS", json!(1));
    let r = s.execute("angle.measure", &json!({ "vertex": [0, 0], "p1": [10, 0], "p2": [0, 10] })).unwrap();
    assert_eq!(r["message"], "Angle = 90d", "{r}");
    setvar(&mut s, "AUNITS", json!(0));
    setvar(&mut s, "AUPREC", json!(2));
    let r = s.execute("angle.measure", &json!({ "vertex": [0, 0], "p1": [10, 0], "p2": [0, 10] })).unwrap();
    assert_eq!(r["message"], "Angle = 90.00°", "{r}");

    // MASSPROP: lengths in feet and inches, the area and moments as plain numbers.
    s.cmdline("rectang 0,0 24,12").unwrap();
    let rect = last_handle(&s);
    s.execute("region", &json!({ "handles": [rect] })).unwrap();
    let reg = last_handle(&s);
    let r = s.execute("massprop", &json!({ "handles": [reg] })).unwrap();
    let m = r["message"].as_str().unwrap();
    assert!(m.contains("Area:                    288.0000\n"), "{m}");
    assert!(m.contains("Perimeter:               6'"), "{m}");
    assert!(m.contains("Centroid:             X: 1'"), "{m}");
    let moments = m.lines().find(|l| l.starts_with("Moments of inertia:")).unwrap();
    assert!(!moments.contains('\'') && moments.contains('.'), "{moments}");
}

#[test]
fn area_add_and_subtract_from_the_command_api() {
    let mut s = Session::new();
    s.cmdline("rectang 0,0 10,10").unwrap();
    let sq = last_handle(&s);
    s.cmdline("circle 5,5 1").unwrap();
    let c = last_handle(&s);
    let r = s
        .execute("area", &json!({ "add": [{ "handle": sq }, { "points": [[20, 0], [22, 0], [22, 2], [20, 2]] }], "subtract": [{ "handle": c }] }))
        .unwrap();
    let total = 100.0 + 4.0 - std::f64::consts::PI;
    assert!((r["area"].as_f64().unwrap() - total).abs() < 1e-9, "{r}");
    let m = r["message"].as_str().unwrap();
    assert!(m.contains("(ADD mode) Area = 100.0000, Perimeter = 40.0000"), "{m}");
    assert!(m.contains("Total area = 104.0000"), "{m}");
    assert!(m.contains("(SUBTRACT mode) Area = 3.1416"), "{m}");
    assert!(m.ends_with("Total area = 100.8584"), "{m}");
    // The total is what QuickCalc's Last Area reads.
    let v = s.execute("cal", &json!({ "last": "area" })).unwrap();
    assert!((v["value"].as_f64().unwrap() - total).abs() < 1e-9);
    // Subtract alone goes negative, as in AutoCAD.
    let r = s.execute("area", &json!({ "subtract": [{ "handle": sq }] })).unwrap();
    assert_eq!(r["area"].as_f64(), Some(-100.0));
    // Bad boundaries are refused.
    s.cmdline("line 0,0 5,5").unwrap();
    s.cmdline("").unwrap();
    let line = last_handle(&s);
    for p in [
        json!({ "add": "all" }),
        json!({ "add": [{ "points": [[0, 0], [1, 1]] }] }),
        json!({ "add": [{}] }),
        json!({ "add": [{ "handle": line }] }),
        json!({ "add": [{ "handle": "FFFFFF" }] }),
    ] {
        assert!(s.execute("area", &p).is_err(), "{p}");
    }
}

#[test]
fn quickcalc_reads_the_last_measurements() {
    let mut s = Session::new();
    s.execute("dist", &json!({ "p1": [0, 0], "p2": [3, 4] })).unwrap();
    assert_eq!(s.execute("cal", &json!({ "last": "distance" })).unwrap()["value"].as_f64(), Some(5.0));
    s.execute("area", &json!({ "points": [[0, 0], [4, 0], [4, 3], [0, 3]] })).unwrap();
    assert_eq!(s.execute("cal", &json!({ "last": "area" })).unwrap()["value"].as_f64(), Some(12.0));
    let r = s.execute("cal", &json!({ "last": "Perimeter" })).unwrap();
    assert_eq!((r["value"].as_f64(), r["message"].as_str()), (Some(14.0), Some("14")));
    // MEASUREGEOM's distance is remembered too.
    s.cmdline("measuregeom").unwrap();
    for l in ["d", "0,0", "6,8", "x"] {
        s.cmdline(l).unwrap();
    }
    assert_eq!(s.execute("cal", &json!({ "last": "distance" })).unwrap()["value"].as_f64(), Some(10.0));
    for p in [json!({ "last": "volume" }), json!({ "last": 3 })] {
        assert!(s.execute("cal", &p).is_err(), "{p}");
    }
}
