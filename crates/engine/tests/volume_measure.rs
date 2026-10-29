//! MEASUREGEOM's Volume option: the area of a polygon or closed object times a height.

use cadaddy_engine::{Input, Session};
use cadaddy_geom::Vec2;
use serde_json::json;

#[test]
fn volume_is_the_area_times_the_height() {
    let mut s = Session::new();
    let r = s.execute("volume.measure", &json!({ "points": [[0, 0], [10, 0], [10, 5], [0, 5]], "height": -3 })).unwrap();
    assert_eq!(r["volume"], 150.0);
    assert_eq!(r["area"], 50.0);
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let v = s.execute("measuregeom", &json!({ "mode": "volume", "handle": c["handle"], "height": 2 })).unwrap();
    assert!((v["volume"].as_f64().unwrap() - 2.0 * std::f64::consts::PI).abs() < 1e-9, "{v}");
}

#[test]
fn bad_volume_input_is_an_error() {
    let mut s = Session::new();
    for p in [
        json!({ "points": [[0, 0], [1, 0]], "height": 1 }),
        json!({ "points": [[0, 0], [1, 0], [1, 1]] }),
        json!({ "points": [[0, 0], [1e200, 0], [1e200, 1e200]], "height": 1e200 }),
        json!({ "points": "x", "height": 1 }),
        json!({ "height": 1 }),
    ] {
        assert!(s.execute("volume.measure", &p).is_err(), "{p}");
    }
}

#[test]
fn the_volume_tool_asks_for_points_then_a_height() {
    let mut s = Session::new();
    s.start("volume.measure").unwrap();
    for p in [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0)] {
        s.input(Input::Point(Vec2::new(p.0, p.1))).unwrap();
    }
    s.input(Input::Enter).unwrap();
    assert!(s.prompt_text().contains("Specify height"), "{}", s.prompt_text());
    // Enter says a height is required and asks again.
    s.input(Input::Enter).unwrap();
    assert!(s.log.iter().any(|l| l.contains("height is required")), "{:?}", s.log);
    assert!(s.prompt_text().contains("Specify height"));
    // The height by two points: 0,0 to 0,2.
    s.input(Input::Point(Vec2::new(0.0, 0.0))).unwrap();
    assert!(s.prompt_text().contains("second point"), "{}", s.prompt_text());
    s.input(Input::Point(Vec2::new(0.0, 2.0))).unwrap();
    assert!(s.log.iter().any(|l| l == "Volume = 12.0000"), "{:?}", s.log);
    assert!(s.current_prompt().is_none(), "the tool ends after one volume");
}
