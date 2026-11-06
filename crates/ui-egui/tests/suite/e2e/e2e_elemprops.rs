//! End to end: the Properties palette shows a model element's own properties (BIM Properties),
//! each value with its source, and an edit there is the user's (`bim.props.set`).

use crate::e2e_common::E2e;
use cadaddy_engine::doc::{Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

/// A made-up 20' × 10' room of 6" walls on A-WALL, built.
fn built(e: &mut E2e) -> Handle {
    {
        let d = e.app.session.doc_mut().unwrap();
        let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
        let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
        d.ensure_layer("A-WALL");
        for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
            let p = |x: f64, y: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
            let h = d.new_handle();
            let mut l = Entity::new(h, EntityKind::Line(Line { a: p(ax, ay), b: p(bx, by) }));
            l.common.layer = "A-WALL".into();
            d.model.push(l);
        }
    }
    let rules = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    });
    e.exec("wallgen", json!({ "rules": rules, "auto": true }));
    let walls = e.exec("bim.props.find", json!({ "kind": "wall", "limit": 1 }));
    Handle::parse_hex(walls["handles"][0].as_str().unwrap()).unwrap()
}

#[test]
fn the_palette_lists_an_elements_properties_with_their_sources_and_edits_them() {
    let mut e = E2e::new();
    let w = built(&mut e);
    e.exec("bim.props.set", json!({ "handle": w.hex(), "key": "fire_rating", "value": "1 HR" }));
    e.app.session.set_selection(vec![w]);
    e.menu("properties");
    e.pump();
    let els = e.elements();
    let at = |label: &str| {
        els.iter()
            .find(|x| x["label"].as_str() == Some(label))
            .and_then(|x| x["rect"].as_array().and_then(|r| r.get(1)).and_then(serde_json::Value::as_f64))
            .unwrap_or_else(|| panic!("{label} on screen: {:?}", els.iter().filter_map(|x| x["label"].as_str()).collect::<Vec<_>>()))
    };
    // The BIM Properties group: the element's kind, then its stored values as boxes to edit.
    let kind = at("Kind: wall");
    let fire = at("1 HR");
    assert!(fire > kind && fire - kind < 40.0, "the value row follows the kind: {kind} {fire}");
    assert_eq!(els.iter().find(|x| x["label"] == "1 HR").map(|x| x["role"].clone()), Some(json!("textedit")));
    // Editing through the palette's command is the user's, as from an agent.
    e.app.run("bim.props.set", json!({ "handle": w.hex(), "key": "finish_a", "value": "paint" })).unwrap();
    e.pump();
    let g = e.exec("bim.props.get", json!({ "handle": w.hex() }));
    assert_eq!(g["elements"][0]["props"]["finish_a"]["source"]["kind"], "user");
}
