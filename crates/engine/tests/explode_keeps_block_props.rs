//! EXPLODE leaves a block's objects as defined: layer 0 and ByBlock colour stay (AutoCAD; taking
//! the reference's layer and colour is what XPLODE's Inherit option does).

use cadaddy_color::Color;
use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn exploded_layer_0_and_byblock_objects_keep_them() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "E-POWER"})).unwrap();
    let c = s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("properties.set", &json!({"handles": [c], "color": "byblock"})).unwrap();
    s.execute("block", &json!({"name": "DEV", "base": [0, 0], "handles": [c], "keep": "delete"})).unwrap();
    let i = s.execute("insert", &json!({"name": "DEV", "at": [10, 0]})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("properties.set", &json!({"handles": [i], "layer": "E-POWER", "color": 1})).unwrap();
    s.execute("explode", &json!({"handles": [i]})).unwrap();
    let d = s.doc().unwrap();
    let circles: Vec<_> = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Circle(_))).collect();
    assert_eq!(circles.len(), 1);
    assert_eq!(circles[0].common.layer, "0");
    assert_eq!(circles[0].common.color, Color::ByBlock);
}
