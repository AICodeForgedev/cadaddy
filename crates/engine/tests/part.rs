//! Parametric parts (Q277): a plate sketched with constraints and driven by named parameters
//! (`width = 8*thick`), padded, holed through, a corner filleted and an edge chamfered; changing a
//! parameter rebuilds it to exactly what a from-scratch build with that value makes. Revolves,
//! the tree (suppress, rename, reorder, delete), saving in .cdy and DXF, and hostile input.

use std::f64::consts::PI;

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// A parameter's value from the tree (they come sorted by name).
fn param(t: &Value, name: &str) -> Value {
    t["parameters"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap()["value"].clone()
}

fn r(g: usize, sub: &str) -> Value {
    json!({"geom": g, "sub": sub})
}

/// A rectangle (roughly drawn: the constraints size it) with a hole in the middle.
fn plate_sketch() -> Value {
    json!({
        "name": "Base",
        "plane": "xy",
        "geometry": [
            {"type": "line", "a": [0, 0], "b": [35, 0]},
            {"type": "line", "a": [35, 0], "b": [35, 25]},
            {"type": "line", "a": [35, 25], "b": [0, 25]},
            {"type": "line", "a": [0, 25], "b": [0, 0]},
            {"type": "circle", "center": [18, 12], "radius": 3},
        ],
        "constraints": [
            {"kind": "coincident", "refs": [r(0, "end"), r(1, "start")]},
            {"kind": "coincident", "refs": [r(1, "end"), r(2, "start")]},
            {"kind": "coincident", "refs": [r(2, "end"), r(3, "start")]},
            {"kind": "coincident", "refs": [r(3, "end"), r(0, "start")]},
            {"kind": "horizontal", "refs": [r(0, "")]},
            {"kind": "horizontal", "refs": [r(2, "")]},
            {"kind": "vertical", "refs": [r(1, "")]},
            {"kind": "vertical", "refs": [r(3, "")]},
            {"kind": "fix", "refs": [r(0, "start")]},
            {"kind": "horizontal_distance", "refs": [r(0, "start"), r(0, "end")], "value": "width"},
            {"kind": "vertical_distance", "refs": [r(1, "start"), r(1, "end")], "value": "depth"},
            {"kind": "radius", "refs": [r(4, "")], "value": "hole"},
            {"kind": "horizontal_distance", "refs": [r(0, "start"), r(4, "center")], "value": "width/2"},
            {"kind": "vertical_distance", "refs": [r(0, "start"), r(4, "center")], "value": "depth/2"},
        ],
    })
}

/// The plate from scratch: parameters, sketch, pad, then a fillet on the corner at (width, depth)
/// and a chamfer on the top front edge.
fn plate(thick: f64) -> (Session, Value) {
    let mut s = Session::new();
    s.execute("part.new", &json!({"name": "Plate", "parameters": {"thick": thick, "width": "8*thick", "depth": 30, "hole": 4}})).unwrap();
    s.execute("part.sketch", &plate_sketch()).unwrap();
    let pad = s.execute("part.pad", &json!({"sketch": "Base", "length": "thick"})).unwrap();
    assert!(pad["features"].as_array().unwrap().iter().all(|f| f["state"] == "ok"), "{pad}");
    let w = 8.0 * thick;
    s.execute("part.fillet", &json!({"edges": [[w, 30.0, thick / 2.0]], "radius": 3})).unwrap();
    let last = s.execute("part.chamfer", &json!({"edges": [[w / 2.0, 0.0, thick]], "distance": 1})).unwrap();
    (s, last)
}

fn props(s: &mut Session) -> Value {
    let t = s.execute("part.tree", &json!({})).unwrap();
    let h = t["solid"].as_str().unwrap().to_string();
    let p = s.execute("massprop", &json!({"handles": [h]})).unwrap_or(Value::Null);
    json!({"volume": t["volume"], "massprop": p})
}

#[test]
fn a_parameter_change_rebuilds_to_the_from_scratch_part() {
    let (mut s, last) = plate(5.0);
    assert!(last["features"].as_array().unwrap().iter().all(|f| f["state"] == "ok"), "{last}");
    // By hand: 40 × 30 × 5, less the hole (r 4), the fillet's corner (r 3 over 5) and the
    // chamfer's prism (1 × 1 / 2 over 40). The round parts are faceted to the solid's tolerance.
    let v = last["volume"].as_f64().unwrap();
    let want = 40.0 * 30.0 * 5.0 - PI * 16.0 * 5.0 - 9.0 * (1.0 - PI / 4.0) * 5.0 - 0.5 * 40.0;
    assert!((v - want).abs() / want < 0.005, "{v} vs {want}");
    // The sketch was solved: its rectangle is now 40 × 30 and the hole centred.
    let t = s.execute("part.tree", &json!({})).unwrap();
    let g = &t["features"][0]["geometry"];
    assert!((g[0]["b"][0].as_f64().unwrap() - 40.0).abs() < 1e-6, "{g}");
    assert!((g[2]["a"][1].as_f64().unwrap() - 30.0).abs() < 1e-6, "{g}");
    assert!((g[4]["center"][0].as_f64().unwrap() - 20.0).abs() < 1e-6 && (g[4]["radius"].as_f64().unwrap() - 4.0).abs() < 1e-6, "{g}");
    assert_eq!(param(&t, "width"), json!(40.0));

    // thick 5 → 6: width follows (48), the fillet and chamfer stay on their edges.
    let after = s.execute("part.parameter", &json!({"name": "thick", "expr": 6})).unwrap();
    assert!(after["features"].as_array().unwrap().iter().all(|f| f["state"] == "ok"), "{after}");
    assert_eq!(after["parameters"]["width"]["value"], json!(48.0));
    let (mut fresh, _) = plate(6.0);
    let (a, b) = (props(&mut s), props(&mut fresh));
    let (va, vb) = (a["volume"].as_f64().unwrap(), b["volume"].as_f64().unwrap());
    assert!((va - vb).abs() < 1e-6 * vb, "rebuilt {va} vs from scratch {vb}");
    // The same solid, to the sketch solver's convergence (≈1e-8 here).
    let (sa, sb) = (&a["massprop"]["solids"][0], &b["massprop"]["solids"][0]);
    let close = |x: &Value, y: &Value| {
        let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
        (x - y).abs() <= 1e-6 + 1e-9 * x.abs().max(y.abs())
    };
    assert!(close(&sa["surfaceArea"], &sb["surfaceArea"]) && close(&sa["volume"], &sb["volume"]), "{sa} vs {sb}");
    for k in 0..3 {
        assert!(close(&sa["centroid"][k], &sb["centroid"][k]), "{sa} vs {sb}");
        assert!(
            close(&sa["boundingBox"][0][k], &sb["boundingBox"][0][k]) && close(&sa["boundingBox"][1][k], &sb["boundingBox"][1][k]),
            "{sa} vs {sb}"
        );
    }
    let want6 = 48.0 * 30.0 * 6.0 - PI * 16.0 * 6.0 - 9.0 * (1.0 - PI / 4.0) * 6.0 - 0.5 * 48.0;
    assert!((va - want6).abs() / want6 < 0.005, "{va} vs {want6}");

    // A dimension edited in the sketch's own expression and the tree reflects it.
    s.execute("part.parameter", &json!({"name": "hole", "expr": "thick/2"})).unwrap();
    let t = s.execute("part.tree", &json!({})).unwrap();
    assert!((t["features"][0]["geometry"][4]["radius"].as_f64().unwrap() - 3.0).abs() < 1e-6);
    // Bad parameters are refused and change nothing.
    assert!(s.execute("part.parameter", &json!({"name": "thick", "expr": "width"})).is_err(), "a cycle");
    assert!(s.execute("part.parameter", &json!({"name": "thick", "expr": "nope*2"})).is_err(), "an unknown name");
    assert_eq!(param(&s.execute("part.tree", &json!({})).unwrap(), "thick"), json!(6.0));
}

#[test]
fn the_tree_suppresses_renames_reorders_and_deletes() {
    let (mut s, _) = plate(5.0);
    let full = s.execute("part.tree", &json!({})).unwrap()["volume"].as_f64().unwrap();
    let t = s.execute("part.edit", &json!({"feature": "Fillet001", "suppressed": true, "name": "Corner round"})).unwrap();
    assert_eq!(t["features"][2]["state"], "suppressed");
    assert!(t["volume"].as_f64().unwrap() > full, "the corner is back");
    s.execute("part.edit", &json!({"feature": "Corner round", "suppressed": false})).unwrap();
    // Pad longer: through the expression.
    let t = s.execute("part.edit", &json!({"feature": "Pad001", "length": "thick*2"})).unwrap();
    assert!(t["volume"].as_f64().unwrap() > 1.9 * full, "{t}");
    // The pad can't move above its sketch; the chamfer can move above the fillet.
    assert!(s.execute("part.reorder", &json!({"feature": "Pad001", "to": 0})).is_err());
    s.execute("part.reorder", &json!({"feature": "Chamfer001", "to": 2})).unwrap();
    let t = s.execute("part.tree", &json!({})).unwrap();
    let names: Vec<&str> = t["features"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Base", "Pad001", "Chamfer001", "Corner round"]);
    assert!(t["features"].as_array().unwrap().iter().all(|f| f["state"] == "ok"), "{t}");
    // A sketch in use can't go; its users first.
    assert!(s.execute("part.delete", &json!({"feature": "Base"})).is_err());
    for f in ["Corner round", "Chamfer001", "Pad001"] {
        s.execute("part.delete", &json!({"feature": f})).unwrap();
    }
    let t = s.execute("part.delete", &json!({"feature": "Base"})).unwrap();
    assert_eq!(t["solid"], Value::Null, "no body, no solid in the drawing");
    assert!(s.execute("part.list", &json!({})).unwrap()["parts"][0]["features"] == 0);
}

#[test]
fn revolves_and_pockets_by_hand() {
    let mut s = Session::new();
    s.execute("part.new", &json!({"name": "Tube", "parameters": {"ri": 2, "ro": 3, "len": 10}})).unwrap();
    // A rectangle from x = ri to ro, y 0 to len, sized by constraints.
    let sk = json!({"plane": "xz", "geometry": [
    {"type": "line", "a": [1, 0], "b": [4, 0]}, {"type": "line", "a": [4, 0], "b": [4, 9]},
    {"type": "line", "a": [4, 9], "b": [1, 9]}, {"type": "line", "a": [1, 9], "b": [1, 0]}],
    "constraints": [
        {"kind": "coincident", "refs": [r(0, "end"), r(1, "start")]}, {"kind": "coincident", "refs": [r(1, "end"), r(2, "start")]},
        {"kind": "coincident", "refs": [r(2, "end"), r(3, "start")]}, {"kind": "coincident", "refs": [r(3, "end"), r(0, "start")]},
        {"kind": "horizontal", "refs": [r(0, "")]}, {"kind": "horizontal", "refs": [r(2, "")]},
        {"kind": "vertical", "refs": [r(1, "")]}, {"kind": "vertical", "refs": [r(3, "")]},
        {"kind": "vertical_distance", "refs": [r(3, "end"), r(3, "start")], "value": "len"},
        {"kind": "horizontal_distance", "refs": [r(0, "start"), r(0, "end")], "value": "ro-ri"},
    ]});
    // The rectangle's corner held at (ri, 0) by a fixed construction point is the job of a later
    // phase; here the sketch starts there and the fix holds it.
    let mut sk = sk;
    sk["geometry"][0]["a"] = json!([2, 0]);
    sk["geometry"][3]["b"] = json!([2, 0]);
    sk["constraints"].as_array_mut().unwrap().push(json!({"kind": "fix", "refs": [r(0, "start")]}));
    s.execute("part.sketch", &sk).unwrap();
    let t = s.execute("part.revolve", &json!({"sketch": 1})).unwrap();
    let v = t["volume"].as_f64().unwrap();
    let want = PI * (9.0 - 4.0) * 10.0;
    assert!((v - want).abs() / want < 0.01, "{v} vs {want}: {t}");
    // Half a turn: half the volume.
    let t = s.execute("part.edit", &json!({"feature": "Revolve001", "angle": 180})).unwrap();
    assert!((t["volume"].as_f64().unwrap() - want / 2.0).abs() / want < 0.01, "{t}");
    // A pocket from a sketch on the tube's end face, 2 deep.
    s.execute("part.edit", &json!({"feature": "Revolve001", "angle": 360})).unwrap();
    s.execute(
        "part.sketch",
        &json!({"name": "Slot", "plane": "xy", "offset": "len", "geometry": [{"type": "circle", "center": [2.5, 0], "radius": 0.25}]}),
    )
    .unwrap();
    let t = s.execute("part.pocket", &json!({"sketch": "Slot", "depth": 2})).unwrap();
    let cut = PI * 0.0625 * 2.0;
    assert!((t["volume"].as_f64().unwrap() - (want - cut)).abs() / want < 0.01, "{t}");
    // A profile across the axis is refused in the tree, not a crash.
    s.execute("part.sketch", &json!({"name": "Across", "plane": "xz", "geometry": [{"type": "circle", "center": [0, 5], "radius": 1}]})).unwrap();
    let t = s.execute("part.revolve", &json!({"sketch": "Across"})).unwrap();
    let last = t["features"].as_array().unwrap().last().unwrap();
    assert_eq!((last["name"].as_str(), last["state"].as_str()), (Some("Revolve002"), Some("error")), "{t}");
    assert!(last["error"].as_str().unwrap().contains("axis"));
}

#[test]
fn parts_are_kept_in_cdy_and_dxf() {
    hooks();
    let (mut s, last) = plate(5.0);
    let v = last["volume"].as_f64().unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-part-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for ext in ["cdy", "dxf"] {
        let p = dir.join(format!("plate.{ext}"));
        s.execute("saveas", &json!({"path": p.display().to_string()})).unwrap();
        let mut c = Session::empty();
        c.execute("open", &json!({"path": p.display().to_string()})).unwrap();
        let t = c.execute("part.tree", &json!({})).unwrap();
        assert_eq!(t["features"].as_array().unwrap().len(), 4, "{ext}");
        assert!((t["volume"].as_f64().unwrap() - v).abs() < 1e-6 * v, "{ext}");
        // Still parametric after opening.
        let after = c.execute("part.parameter", &json!({"name": "thick", "expr": 6})).unwrap();
        assert!(after["features"].as_array().unwrap().iter().all(|f| f["state"] == "ok"), "{ext}: {after}");
    }
}

#[test]
fn hostile_parameters_never_panic() {
    let mut s = Session::new();
    assert!(s.execute("part.pad", &json!({"sketch": 1, "length": 5})).is_err(), "no part");
    s.execute("part.new", &json!({"name": "H"})).unwrap();
    let junk = [
        json!({}),
        json!(null),
        json!({"sketch": "nope", "length": 1}),
        json!({"geometry": [{"type": "line", "a": [0, 0], "b": [0, 0]}]}),
        json!({"geometry": [{"type": "circle", "center": [0, 0], "radius": -1}]}),
        json!({"geometry": [{"type": "arc", "center": [1e308, 0], "radius": 1, "start": 0, "end": 90}]}),
        json!({"geometry": [{"type": "line", "a": [0, 0], "b": [1, 0]}], "constraints": [{"kind": "radius", "refs": [{"geom": 7}], "value": 1}]}),
        json!({"geometry": [{"type": "line", "a": [0, 0], "b": [1, 0]}], "constraints": [{"kind": "warp", "refs": []}]}),
        json!({"plane": {"origin": [0, 0, 0], "normal": [0, 0, 0]}, "geometry": []}),
        json!({"plane": "qq", "geometry": []}),
        json!({"face": {"point": [0, 0, 0]}, "geometry": []}),
        json!({"name": "x", "expr": "1/0"}),
        json!({"name": "1bad", "expr": 1}),
        json!({"feature": 99}),
        json!({"feature": "x", "to": -1}),
        json!({"edges": [[0, 0, 0]], "radius": 1}),
        json!({"length": "((((", "sketch": 1}),
    ];
    let cmds = [
        "part.sketch",
        "part.pad",
        "part.pocket",
        "part.revolve",
        "part.fillet",
        "part.chamfer",
        "part.parameter",
        "part.edit",
        "part.reorder",
        "part.delete",
        "part.rebuild",
        "part.tree",
    ];
    for c in cmds {
        for j in &junk {
            let _ = s.execute(c, j);
        }
    }
    // A sketch that is open, then padded: an error state in the tree, not a crash.
    s.execute(
        "part.sketch",
        &json!({"name": "Open", "geometry": [{"type": "line", "a": [0, 0], "b": [1, 0]}, {"type": "line", "a": [1, 0], "b": [1, 1]}]}),
    )
    .unwrap();
    let t = s.execute("part.pad", &json!({"sketch": "Open", "length": 1})).unwrap();
    let pad = t["features"].as_array().unwrap().iter().find(|f| f["type"] == "Pad").unwrap();
    assert_eq!(pad["state"], "error");
    assert!(pad["error"].as_str().unwrap().contains("closed"), "{pad}");
    // A huge or negative length is refused at the feature, not a crash.
    s.execute("part.sketch", &json!({"name": "Sq", "geometry": [{"type": "circle", "center": [0, 0], "radius": 1}]})).unwrap();
    assert!(s.execute("part.pad", &json!({"sketch": "Sq", "length": 1e300})).is_err());
    let t = s.execute("part.pad", &json!({"sketch": "Sq", "length": -2})).unwrap();
    assert_eq!(t["features"].as_array().unwrap().last().unwrap()["state"], "error");
}

#[test]
fn a_failed_feature_keeps_the_body() {
    let (mut s, last) = plate(5.0);
    let v = last["volume"].as_f64().unwrap();
    // A pocket on an open sketch fails; the plate stays as it was.
    s.execute("part.sketch", &json!({"name": "Open", "geometry": [{"type": "line", "a": [0, 0], "b": [5, 0]}]})).unwrap();
    let t = s.execute("part.pocket", &json!({"sketch": "Open", "depth": 1})).unwrap();
    assert_eq!(t["features"].as_array().unwrap().last().unwrap()["state"], "error");
    assert!((t["volume"].as_f64().unwrap() - v).abs() < 1e-9 * v, "{t}");
    // So does a fillet on an edge the body no longer has (its faces are gone once the pad is
    // suppressed, the rest of the tree failing on in turn).
    let t = s.execute("part.edit", &json!({"feature": "Pad001", "suppressed": true})).unwrap();
    assert_eq!(t["volume"], Value::Null);
    assert_eq!(t["solid"], Value::Null);
    let t = s.execute("part.edit", &json!({"feature": "Pad001", "suppressed": false})).unwrap();
    assert!((t["volume"].as_f64().unwrap() - v).abs() < 1e-6 * v, "{t}");
    assert!(t["solid"].is_string());
}
