//! Q6662 (test finding fix-reroute-whip-runs-block-build): `conduit.reroute` wrote each
//! equipment whip (the raceway from a circuit's connection on to a fan's or a unit's terminal
//! box) with one bottom height per POINT instead of per segment, so the spec's own check refused
//! the run and every later `model.build` (even of one part) was refused before it started. The
//! routing writes runs the spec's check takes, checks its own output, and a spec error in a run
//! the software made is reported and skipped by the build instead of stopping it.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// The made-up fan's circuit on panel QP: the fan and a receptacle share it.
const CIRCUIT: &str = "QP-3";
const FAN: &str = "EF-2";

/// One storey: panel QP, an exhaust fan hung high in the ceiling space (its terminal box well
/// above its disconnect, so the whip rises into it) and a receptacle, both on QP-3.
fn building(fan_bottom_ft: f64) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let fan = json!({"tag": FAN, "kind": "exhaust_fan", "level": "L1", "min": [480.0, 240.0], "max": [504.0, 264.0],
        "height_ft": 1.5, "bottom_ft": fan_bottom_ft, "title": "FAN SCHEDULE", "properties": {"CIRCUIT": CIRCUIT, "PANEL": "QP"}});
    let row = |n: &str, d: &str, a: &str, p: u32| json!({"number": n, "description": d, "breaker": a, "poles": p, "protection": [], "source": "X-2"});
    let tags = vec![
        json!({"id": "device:L1:5.0,30.0", "kind": "receptacle", "level": "L1", "at_ft": [5.0, 30.0], "fields": {
            "symbol": {"value": "R-sym", "source": "plan"}, "sheet": {"value": "X-101", "source": "plan"}, "circuit": {"value": CIRCUIT, "source": "plan"}}}),
        json!({"id": format!("circuit:L1:X-101:{CIRCUIT}"), "kind": "circuit", "level": "L1", "at_ft": [4.0, 30.0], "fields": {
            "circuit": {"value": CIRCUIT, "source": "plan"}, "panel": {"value": "QP", "source": "plan"}}}),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {
            "equipment": [
                {"tag": "QP", "kind": "panel", "level": "L1", "min": [-24, 110], "max": [-4, 130], "height_ft": 4, "bottom_ft": 2},
                fan
            ],
            "panel_schedules": [{"panel": "QP", "source": "X-2", "circuits": [row("3", "EF-2 / RCPT", "20", 1)]}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": true})).expect("first build");
    s
}

fn runs(s: &mut Session) -> Vec<Value> {
    s.execute("bspec.get", &json!({})).expect("spec")["spec"]["mep"]["runs"].as_array().cloned().unwrap_or_default()
}

/// The spec's shape for a run: 2 points or more and one bottom per segment, or a riser's 1 point
/// and 1 bottom under its top.
fn shape_ok(r: &Value) -> bool {
    let pts = r["points"].as_array().map_or(0, Vec::len);
    let bots = r["bottoms_ft"].as_array().map_or(0, Vec::len);
    if r["top_ft"].is_number() { pts == 1 && bots == 1 } else { pts >= 2 && bots + 1 == pts }
}

/// The runs of the fan's whip: those through its end at the terminal box (the routing's
/// report gives the end).
fn whips(rs: &[Value], report: &Value) -> Vec<Value> {
    let ends: Vec<(f64, f64)> = report["equipment_whips"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["equipment"] == json!(FAN))
        .filter_map(|w| Some((w["end"][0].as_f64()?, w["end"][1].as_f64()?)))
        .collect();
    let at_end = |r: &Value| {
        r["points"].as_array().into_iter().flatten().any(|p| {
            let (x, y) = (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN));
            ends.iter().any(|(ex, ey)| (x - ex).abs() < 0.05 && (y - ey).abs() < 0.05)
        })
    };
    rs.iter().filter(|r| r["id"].as_str().is_some_and(|id| id.contains(CIRCUIT)) && at_end(r)).cloned().collect()
}

#[test]
fn a_rerouted_equipment_whip_passes_the_spec_check_and_a_part_build_runs() {
    // High in the ceiling (its disconnect hung beside it, at its height) and low on the floor
    // (its disconnect at the mounting height above it: the whip drops into the box).
    for fan_bottom in [8.0, 0.5] {
        let mut s = building(fan_bottom);
        s.execute("conduit.reroute", &json!({"feeders": "all"})).expect("reroute");
        let r = s.execute("conduit.route", &json!({})).expect("the routing's report");
        let all = runs(&mut s);
        let w = whips(&all, &r);
        assert!(!w.is_empty(), "fan at {fan_bottom} ft: its whip is routed: {:#}", r["equipment_whips"]);
        let bad: Vec<&Value> = all.iter().filter(|r| !shape_ok(r)).collect();
        assert!(bad.is_empty(), "fan at {fan_bottom} ft: every routed run has the spec's shape: {bad:#?}");
        // No piece of the whip is a repeated point: its rise is a riser of its own.
        for x in &w {
            let pts: Vec<[f64; 2]> = serde_json::from_value(x["points"].clone()).expect("points");
            assert!(pts.windows(2).all(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) > 1e-6), "fan at {fan_bottom} ft: {x:#}");
        }
        if fan_bottom < 2.0 {
            assert!(w.iter().any(|x| x["top_ft"].is_number()), "fan at {fan_bottom} ft: the whip drops into the box: {w:#?}");
        }
        assert_eq!(r["runs_checked"]["dropped"], json!(0), "{:#}", r["runs_checked"]);
        // Every build after the routing runs: one part, and the whole model.
        let one = s.execute("model.build", &json!({"parts": ["ceiling"]}));
        assert!(one.is_ok(), "fan at {fan_bottom} ft: a part build after the reroute runs: {one:?}");
        let whole = s.execute("model.build", &json!({"force": true}));
        assert!(whole.is_ok(), "fan at {fan_bottom} ft: a full build after the reroute runs: {whole:?}");
        assert!(whole.as_ref().map(|v| v.get("skipped_runs").is_none()).unwrap_or(false), "nothing to skip: {whole:?}");
    }
}

/// A run with one bottom per point (what the routing used to write), as `made`.
fn per_point_run(made: &str) -> Value {
    let mut r = json!({"system": "E-BRANCH-TEST", "level": "L1", "points": [[0.0, 0.0], [120.0, 0.0]], "size_in": [0.75, 0.0],
        "bottoms_ft": [8.0, 8.0], "id": "circuit:L1:X-101:QP-9#4"});
    if made != "drawn" {
        r["made"] = json!(made);
        r["basis"] = json!("a test's run");
    }
    r
}

fn spec_with_run(run: Value) -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "systems": [{"name": "E-BRANCH-TEST", "layer": "E-TEST", "kind": "conduit", "size_in": [0.75, 0.0], "bottom_ft": 8.0}],
            "runs": [run, {"system": "E-BRANCH-TEST", "level": "L1", "points": [[0.0, 24.0], [120.0, 24.0]], "size_in": [0.75, 0.0], "bottoms_ft": [8.0]}]
        }
    })
}

#[test]
fn a_bad_run_the_software_made_is_skipped_not_a_refused_build() {
    for made in ["inferred", "derived"] {
        let mut s = Session::empty();
        s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
        s.execute("bspec.set", &json!({"replace": true, "spec": spec_with_run(per_point_run(made))})).expect("spec");
        let r = s.execute("model.build", &json!({"route": false})).unwrap_or_else(|e| panic!("{made}: the build runs: {e:?}"));
        let skipped = r["skipped_runs"].as_array().cloned().unwrap_or_default();
        assert_eq!(skipped.len(), 1, "{made}: the bad run is reported: {r:#}");
        assert_eq!(skipped[0]["id"], json!("circuit:L1:X-101:QP-9#4"), "{skipped:#?}");
        assert_eq!(skipped[0]["made"], json!(made), "{skipped:#?}");
        assert!(skipped[0]["errors"].to_string().contains("one bottom height per segment"), "{skipped:#?}");
        assert!(r["notes"].to_string().contains("left out of this build"), "{:#}", r["notes"]);
        // A part build skips it the same way.
        let p = s.execute("model.build", &json!({"parts": ["ceiling"], "route": false})).unwrap_or_else(|e| panic!("{made}: {e:?}"));
        assert_eq!(p["skipped_runs"].as_array().map_or(0, Vec::len), 1, "{p:#}");
    }
}

#[test]
fn a_bad_drawn_run_still_refuses_the_build() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec_with_run(per_point_run("drawn"))})).expect("spec");
    let e = s.execute("model.build", &json!({"route": false})).expect_err("a drawn run's error refuses");
    assert!(format!("{e:?}").contains("one bottom height per segment"), "{e:?}");
}
