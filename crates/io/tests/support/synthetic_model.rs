//! A synthetic model in the mix a large job model has, for the .cdy open measurements (Q6398):
//! plan linework and text, block definitions and their references with attributes, and BIM
//! elements (3D boxes carrying element properties as extended data). Made-up names only.
//!
//! Shared by `cadaddy-io`'s `tests/cdy_open_memory.rs` and `cadaddy-engine`'s
//! `tests/cdy_open_profile.rs` (included with `#[path]`, so both open the same model).

use cadaddy_doc::{Block, Common, Drawing, Entity, EntityKind, Handle, Layer, Line, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::Vec3;
use serde_json::json;

/// Block definitions in the model (each a symbol of a dozen objects).
const BLOCKS: usize = 300;

fn kind(v: serde_json::Value) -> EntityKind {
    serde_json::from_value(v).unwrap()
}

/// A made-up element's properties as another program keeps them on the object (extended data):
/// a type, sizes, a level and a source, the way BIM elements carry theirs.
fn element_data(i: usize) -> Vec<RawTag> {
    const KINDS: [&str; 4] = ["wall", "door", "duct", "fixture"];
    let props = json!({
        "kind": KINDS.get(i % 4),
        "type": format!("Type {}", i % 37),
        "width": 0.5 + (i % 7) as f64 * 0.125,
        "height": 9.0 + (i % 3) as f64,
        "level": format!("Level {}", i % 4 + 1),
        "source": {"kind": "derived", "basis": "a synthetic rule", "confidence": 0.9},
        "id": format!("E-{i:07}"),
    });
    vec![RawTag { code: 1001, value: "SYNTH_ELEMENT".into() }, RawTag { code: 1000, value: props.to_string() }]
}

fn element(x: f64, y: f64) -> EntityKind {
    let v = |dx: f64, dy: f64, z: f64| Vec3::new(x + dx, y + dy, z);
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    EntityKind::Mesh(Mesh {
        vertices: vec![
            v(0.0, 0.0, 0.0),
            v(4.0, 0.0, 0.0),
            v(4.0, 0.5, 0.0),
            v(0.0, 0.5, 0.0),
            v(0.0, 0.0, 9.0),
            v(4.0, 0.0, 9.0),
            v(4.0, 0.5, 9.0),
            v(0.0, 0.5, 9.0),
        ],
        faces: vec![f([0, 1, 2, 3]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
        kind: MeshKind::Polyface,
    })
}

fn text(x: f64, y: f64, value: &str) -> serde_json::Value {
    json!({"insert": {"x": x, "y": y, "z": 0.0}, "height": 0.25, "value": value})
}

/// One object of the mix, the `i`th: of every 20, 8 lines, 4 polylines, 2 texts, 2 block
/// references with attributes and 4 BIM elements.
fn object(i: usize, side: usize) -> (Common, EntityKind) {
    let (x, y) = ((i % side) as f64 * 3.0, (i / side) as f64 * 3.0);
    let layer = |n: &str| Common { layer: format!("{n}-{}", i % 6).into(), ..Common::default() };
    match i % 20 {
        0..8 => (layer("Plan"), EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 2.5, y + (i % 5) as f64 * 0.5, 0.0) })),
        8..12 => {
            let pts: Vec<_> = (0..5 + i % 4).map(|k| json!({"p": {"x": x + k as f64 * 0.4, "y": y + (k % 2) as f64 * 0.3}})).collect();
            (layer("Plan"), kind(json!({"type": "lwPolyline", "vertices": pts, "closed": i % 3 == 0})))
        }
        12..14 => {
            let mut t = text_fields(x, y, &format!("NOTE {i}"));
            t.insert("type".into(), json!("text"));
            (layer("Notes"), kind(serde_json::Value::Object(t)))
        }
        14..16 => {
            let attribs = json!([
                {"tag": "MARK", "text": text(x, y + 0.5, &format!("M{}", i % 999))},
                {"tag": "SIZE", "text": text(x, y + 0.8, "24x24")},
            ]);
            (
                layer("Symbols"),
                kind(
                    json!({"type": "insert", "block": format!("SYM{}", i % BLOCKS), "insert": {"x": x, "y": y, "z": 0.0}, "rotation": (i % 4) as f64, "attribs": attribs}),
                ),
            )
        }
        _ => {
            let mut c = layer("Elements");
            c.xdata = element_data(i).into();
            c.material = Some(format!("Material {}", i % 9)).into();
            (c, element(x, y))
        }
    }
}

/// A text's fields, to be an object of its own (with its `type`).
fn text_fields(x: f64, y: f64, value: &str) -> serde_json::Map<String, serde_json::Value> {
    match text(x, y, value) {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

/// A synthetic model of `n` model space objects in the mix above, with its blocks and layers.
pub fn model(n: usize) -> Drawing {
    let mut d = Drawing::new_imperial();
    for name in ["Plan", "Notes", "Symbols", "Elements"] {
        for k in 0..6 {
            d.layers.push(Layer { name: format!("{name}-{k}"), ..Layer::default() });
        }
    }
    for b in 0..BLOCKS {
        let mut block = Block::new(&format!("SYM{b}"));
        for k in 0..12 {
            let r = 0.5 + k as f64 * 0.1;
            block
                .entities
                .push(Entity::new(Handle(0), EntityKind::Line(Line { a: Vec3::new(-r, 0.0, 0.0), b: Vec3::new(r, (k % 3) as f64 * 0.2, 0.0) })));
        }
        d.blocks.insert(block.name.clone(), std::sync::Arc::new(block));
    }
    let side = (n as f64).sqrt().ceil().max(1.0) as usize;
    for i in 0..n {
        let (c, k) = object(i, side);
        d.add(&Space::Model, c, k).unwrap();
    }
    d
}
