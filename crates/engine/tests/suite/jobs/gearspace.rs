//! Foreign pipe and duct out of electrical gear's dedicated space (NEC 110.26(E)(1)): a made-up
//! room whose panel the plans letter 8 in off the south wall, so the build sets it on the wall's
//! face. A vent drawn along the wall passes over the panel where it is built (not where its
//! lettering is), a supply duct crosses straight over it, a sprinkler line too (the rule lets
//! sprinkler protection run there) and a cold water line runs well clear. The build offsets the
//! vent and the duct past the panel's side, keeps each drawn route beside the installed one, and
//! leaves the rest as drawn.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn run(system: &str, pts: &[[f64; 2]], size: [f64; 2], bottom: f64, id: &str) -> Value {
    json!({"system": system, "level": "L1", "points": pts, "size_in": size, "bottoms_ft": vec![bottom; pts.len() - 1], "id": id})
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0], "region": [-100, -100, 400, 300]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "mep": {
            "systems": [
                {"name": "V", "layer": "P-VENT", "kind": "pipe", "size_in": [3, 0], "bottom_ft": 8.5},
                {"name": "SA", "layer": "M-DUCT", "kind": "duct", "size_in": [12, 8], "bottom_ft": 8.0},
                {"name": "SPK", "layer": "F-SPK", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9.0},
                {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 8.5}
            ],
            "runs": [
                // 2 in off the wall's face: over the panel as built (y 6..10), clear of its lettering's box.
                run("V", &[[60.0, 8.0], [200.0, 8.0]], [3.0, 0.0], 8.5, "V1"),
                run("SA", &[[110.0, -20.0], [110.0, 150.0]], [12.0, 8.0], 8.0, "D1"),
                run("SPK", &[[60.0, 9.0], [200.0, 9.0]], [1.0, 0.0], 9.0, "S1"),
                run("CW", &[[60.0, 60.0], [200.0, 60.0]], [1.0, 0.0], 8.5, "W1")
            ],
            "equipment": [{
                "tag": "PX", "kind": "panel", "level": "L1", "min": [100.0, 14.0], "max": [120.0, 18.0], "bottom_ft": 4.5, "height_ft": 2.0,
                "sources": {"position": "plan lettering"}
            }]
        }
    })
}

fn job() -> (Session, Value) {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    // The room's walls: inside 6..294 x 6..180.
    rect(&mut s, [0.0, 0.0], [300.0, 186.0]);
    rect(&mut s, [6.0, 6.0], [294.0, 180.0]);
    let r = s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    let built = s.execute("model.build", &json!({})).unwrap();
    (s, built)
}

fn runs(s: &mut Session) -> Vec<Value> {
    let g = s.execute("bspec.get", &json!({})).unwrap();
    g["spec"]["mep"]["runs"].as_array().cloned().unwrap_or_default()
}

fn by_id<'a>(runs: &'a [Value], id: &str) -> &'a Value {
    runs.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("run {id}: {runs:?}"))
}

/// Whether a run's centre line comes within `grow` of the box.
fn over(r: &Value, min: [f64; 2], max: [f64; 2], grow: f64) -> bool {
    let pts: Vec<[f64; 2]> = r["points"].as_array().into_iter().flatten().filter_map(|p| Some([p[0].as_f64()?, p[1].as_f64()?])).collect();
    pts.windows(2).any(|w| {
        (0..=100).any(|k| {
            let t = f64::from(k) / 100.0;
            let p = [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t];
            p[0] > min[0] - grow && p[0] < max[0] + grow && p[1] > min[1] - grow && p[1] < max[1] + grow
        })
    })
}

#[test]
fn pipe_and_duct_drawn_over_a_panel_are_offset_past_it_with_the_drawn_route_kept() {
    let (mut s, built) = job();
    let g = &built["gearSpace"];
    assert_eq!((g["found"].clone(), g["corrected"].clone(), g["left"].clone()), (json!(2), json!(2), json!([])), "{g}");
    let rs = runs(&mut s);
    // Where the panel is built: on the south wall's face, 20 x 4 in.
    let (pmin, pmax) = ([100.0, 6.0], [120.0, 10.0]);
    for id in ["V1", "D1"] {
        let r = by_id(&rs, id);
        let half = r["size_in"][0].as_f64().unwrap_or(0.0) / 2.0;
        assert!(!over(r, pmin, pmax, half), "{id} still over the panel: {r}");
        let c = &r["corrections"][0];
        assert_eq!(c["key"], "route", "{r}");
        assert_eq!(c["rule"], "NEC 110.26(E)(1)(a)", "{c}");
        assert!(c["corrected"].as_str().is_some_and(|t| t.contains("panel PX")), "{c}");
        assert!(c["design"].as_str().is_some_and(|t| t.contains("as drawn")), "{c}");
    }
    assert!(by_id(&rs, "V1")["corrections"][0]["design"].as_str().is_some_and(|t| t.contains("60.00,8.00 → 200.00,8.00")));
    // The ends stay where the plans put them.
    let v = by_id(&rs, "V1")["points"].as_array().cloned().unwrap_or_default();
    assert_eq!((v.first(), v.last()), (Some(&json!([60.0, 8.0])), Some(&json!([200.0, 8.0]))), "{v:?}");
    // The sprinkler line may run there; the cold water line is nowhere near: both as drawn.
    for id in ["S1", "W1"] {
        let r = by_id(&rs, id);
        assert!(r.get("corrections").is_none() && r["points"].as_array().is_some_and(|p| p.len() == 2), "{id}: {r}");
    }
    // The built pieces show the difference: the route installed, the drawn one beside it.
    let p = s.execute("bim.provenance", &json!({"corrections": true})).unwrap();
    let listed = p["corrections"]["listed"].as_array().cloned().unwrap_or_default();
    let routes: Vec<&Value> = listed.iter().filter(|c| c["key"] == "route").collect();
    assert_eq!(routes.len(), 2, "{p}");
    assert!(
        routes
            .iter()
            .all(|c| c["design"].as_str().is_some_and(|d| d.contains("as drawn")) && c["reason"].as_str().is_some_and(|w| w.contains("110.26")))
    );
    // A rebuild (forced: nothing it reads changed, Q6403) finds nothing more to move and the
    // pieces still carry the correction.
    let again = s.execute("model.build", &json!({"force": true})).unwrap();
    assert_eq!(again["gearSpace"]["found"], json!(0), "{}", again["gearSpace"]);
    let p = s.execute("bim.provenance", &json!({"corrections": true})).unwrap();
    let n = p["corrections"]["listed"].as_array().map_or(0, |l| l.iter().filter(|c| c["key"] == "route").count());
    assert_eq!(n, 2, "{p}");
}
