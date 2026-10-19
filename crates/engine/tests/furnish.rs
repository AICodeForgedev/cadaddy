//! FURNISH: 2D furniture on a synthetic plan matched to 3D library models by name and by size,
//! placed where and as turned as the plan draws them; unmatched ones reported.

use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

fn block(s: &mut Session, name: &str, pts: &[[f64; 2]]) {
    let h = s.execute("pline", &json!({"vertices": pts, "closed": true})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("block", &json!({"name": name, "base": [0, 0], "handles": [h], "keep": "delete"})).unwrap();
}

fn outline(s: &mut Session, layer: &str, center: [f64; 2], w: f64, d: f64, deg: f64) -> String {
    let (c, sn) = (deg.to_radians().cos(), deg.to_radians().sin());
    let p = |x: f64, y: f64| json!([center[0] + x * c - y * sn, center[1] + x * sn + y * c]);
    s.execute("layer.new", &json!({"name": layer, "current": true})).ok();
    s.execute("layer.set", &json!({"name": layer, "current": true})).ok();
    s.execute("pline", &json!({"vertices": [p(-w / 2.0, -d / 2.0), p(w / 2.0, -d / 2.0), p(w / 2.0, d / 2.0), p(-w / 2.0, d / 2.0)], "closed": true}))
        .unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string()
}

fn placed<'a>(r: &'a Value, model: &str) -> &'a Value {
    r["placed"].as_array().unwrap().iter().find(|p| p["model"] == model).unwrap_or_else(|| panic!("{model} not placed: {r}"))
}

#[test]
fn blocks_match_by_name_and_outlines_by_size() {
    let mut s = Session::new();
    // A queen bed drawn as the library draws it (back at Y 0, extending down), placed turned 90°.
    block(&mut s, "BED-QUEEN", &[[-30.0, -80.0], [30.0, -80.0], [30.0, 0.0], [-30.0, 0.0]]);
    s.execute("insert", &json!({"name": "BED-QUEEN", "at": [200, 100], "rotation": 90})).unwrap();
    // A toilet block drawn centred on its own box.
    block(&mut s, "TOILET-1", &[[-10.0, -14.0], [10.0, -14.0], [10.0, 14.0], [-10.0, 14.0]]);
    s.execute("insert", &json!({"name": "TOILET-1", "at": [400, 50]})).unwrap();
    // A desk outline 60 × 30 turned 30°, and a 10 × 10 box nothing matches.
    let desk = outline(&mut s, "A-FURN", [100.0, 300.0], 60.0, 30.0, 30.0);
    let odd = outline(&mut s, "A-FURN", [300.0, 300.0], 10.0, 10.0, 0.0);
    let r = s.execute("furnish", &json!({})).unwrap();
    let bed = placed(&r, "M3_BED_QUEEN");
    assert_eq!(bed["by"], "name");
    assert!((bed["rotation"].as_f64().unwrap() - 90.0).abs() < 1e-6, "{bed}");
    // The model's plan centre (0, −40) lands on the bed's centre: (240, 100).
    let at = Vec2::new(bed["at"][0].as_f64().unwrap(), bed["at"][1].as_f64().unwrap());
    assert!((at + Vec2::new(0.0, -40.0).rotate(90f64.to_radians())).dist(Vec2::new(240.0, 100.0)) < 1e-6, "{at:?}");
    assert_eq!(placed(&r, "M3_WATER_CLOSET")["by"], "name");
    let d = placed(&r, "M3_DESK");
    assert_eq!((d["by"].clone(), d["source"].clone()), (json!("size"), json!(desk)));
    let rot = d["rotation"].as_f64().unwrap();
    assert!((rot - 30.0).abs() < 1e-6 || (rot - 210.0).abs() < 1e-6, "{rot}");
    let un = r["unmatched"].as_array().unwrap();
    assert_eq!(un.len(), 1, "{r}");
    assert_eq!(un[0]["source"], json!(odd));
    // The models are in the drawing, on their sources' layers, tagged.
    let placed_inserts: Vec<_> =
        s.doc().unwrap().model.iter().filter(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block.starts_with("M3_"))).cloned().collect();
    assert_eq!(placed_inserts.len(), 3);
    assert!(placed_inserts.iter().all(|e| e.common.xdata.iter().any(|t| t.value == "CADADDY_FURNISH")));
    // Again: nothing new to place.
    let again = s.execute("furnish", &json!({})).unwrap();
    assert_eq!(again["placed"].as_array().unwrap().len(), 0, "{again}");
}

#[test]
fn an_outlines_back_faces_the_wall_and_reporting_places_nothing() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("line", &json!({"points": [[0, 112], [200, 112]]})).unwrap();
    // A bookcase 36 × 12 against the wall above it.
    outline(&mut s, "A-CASEWORK", [100.0, 105.0], 36.0, 12.0, 0.0);
    let r = s.execute("furnish", &json!({"place": false})).unwrap();
    let b = placed(&r, "M3_BOOKCASE");
    // Its back (+Y) toward the wall: not turned.
    assert!(b["rotation"].as_f64().unwrap().abs() < 1e-6 || (b["rotation"].as_f64().unwrap() - 360.0).abs() < 1e-6, "{b}");
    assert!(b.get("handle").is_none());
    assert!(s.doc().unwrap().model.iter().all(|e| !matches!(&e.kind, EntityKind::Insert(_))), "place: false places nothing");
    // The wall below instead: turned half round.
    let mut t = Session::new();
    t.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    t.execute("line", &json!({"points": [[0, 98], [200, 98]]})).unwrap();
    outline(&mut t, "A-CASEWORK", [100.0, 105.0], 36.0, 12.0, 0.0);
    let r = t.execute("furnish", &json!({"place": false})).unwrap();
    assert!((placed(&r, "M3_BOOKCASE")["rotation"].as_f64().unwrap() - 180.0).abs() < 1e-6, "{r}");
    assert!(t.execute("furnish", &json!({"tolerance": 3})).is_err());
}
