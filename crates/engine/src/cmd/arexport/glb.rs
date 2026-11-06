//! The AR scene as binary glTF 2.0 (`.glb`): the Khronos Group's glTF 2.0 Specification (2017,
//! rev. 2021) — §4.4 binary container (12-byte header, a JSON chunk and a BIN chunk, each
//! 4-byte aligned), §3.6 accessors (POSITION with min/max, unit NORMALs, `UNSIGNED_INT`
//! indices), §3.7.2 meshes shared by several nodes (instancing), §3.9 materials (metallic-
//! roughness, `BLEND` for the translucent shell), §3.12 `extras` for application data.

use serde_json::{Map, Value, json};

use super::Scene;

const ARRAY_BUFFER: u32 = 34_962;
const ELEMENT_ARRAY_BUFFER: u32 = 34_963;
const FLOAT: u32 = 5_126;
const UNSIGNED_INT: u32 = 5_125;

fn f(v: f32) -> Value {
    json!(f64::from(v))
}

fn pad4(bin: &mut Vec<u8>, fill: u8) {
    while !bin.len().is_multiple_of(4) {
        bin.push(fill);
    }
}

/// The scene's glTF document and its binary buffer.
pub(crate) fn document(scene: &Scene) -> (Value, Vec<u8>) {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut meshes: Vec<Value> = Vec::new();
    let mut view = |bin: &mut Vec<u8>, bytes: &[u8], target: u32| {
        pad4(bin, 0);
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len(), "target": target}));
        views.len() - 1
    };
    for (i, g) in scene.geometries.iter().enumerate() {
        let pos: Vec<u8> = g.positions.iter().flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes())).collect();
        let nrm: Vec<u8> = g.normals.iter().flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes())).collect();
        let idx: Vec<u8> = g.indices.iter().flat_map(|v| v.to_le_bytes()).collect();
        let (lo, hi) = g.bounds();
        let pv = view(&mut bin, &pos, ARRAY_BUFFER);
        accessors
            .push(json!({"bufferView": pv, "componentType": FLOAT, "count": g.positions.len(), "type": "VEC3", "min": lo.map(f), "max": hi.map(f)}));
        let nv = view(&mut bin, &nrm, ARRAY_BUFFER);
        accessors.push(json!({"bufferView": nv, "componentType": FLOAT, "count": g.normals.len(), "type": "VEC3"}));
        let iv = view(&mut bin, &idx, ELEMENT_ARRAY_BUFFER);
        accessors.push(json!({"bufferView": iv, "componentType": UNSIGNED_INT, "count": g.indices.len(), "type": "SCALAR"}));
        let a = accessors.len();
        meshes.push(json!({
            "name": format!("part{i}"),
            "primitives": [{"attributes": {"POSITION": a - 3, "NORMAL": a - 2}, "indices": a - 1, "material": g.material, "mode": 4}],
        }));
    }
    pad4(&mut bin, 0);
    let materials: Vec<Value> = scene
        .materials
        .iter()
        .map(|m| {
            let mut v = json!({
                "name": m.name,
                "pbrMetallicRoughness": {"baseColorFactor": [f(m.rgb[0]), f(m.rgb[1]), f(m.rgb[2]), f(m.opacity)], "metallicFactor": 0.0, "roughnessFactor": 0.8},
            });
            if m.opacity < 1.0 {
                v["alphaMode"] = json!("BLEND");
                v["doubleSided"] = json!(true);
            }
            v
        })
        .collect();
    let mut nodes: Vec<Value> =
        vec![json!({"name": "CADaddy_AR", "children": (1..=scene.nodes.len()).collect::<Vec<_>>(), "extras": {"cadaddy": scene.info}})];
    for n in &scene.nodes {
        let mut node = Map::new();
        node.insert("name".into(), json!(n.name));
        node.insert("mesh".into(), json!(n.geometry));
        let t = n.translation;
        if t.iter().any(|v| *v != 0.0) {
            node.insert("translation".into(), json!(t));
        }
        node.insert("extras".into(), Value::Object(n.meta.clone()));
        nodes.push(Value::Object(node));
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "CADaddy bim.export.ar", "extras": {"cadaddy": scene.info}},
        "scene": 0,
        "scenes": [{"name": "AR export", "nodes": [0]}],
        "nodes": nodes,
    });
    if !meshes.is_empty() {
        doc["meshes"] = json!(meshes);
        doc["materials"] = json!(materials);
        doc["accessors"] = json!(accessors);
        doc["bufferViews"] = json!(views);
        doc["buffers"] = json!([{"byteLength": bin.len()}]);
    }
    (doc, bin)
}

/// The scene as a `.glb` file.
pub(crate) fn write(scene: &Scene) -> Vec<u8> {
    let (doc, bin) = document(scene);
    let mut text = serde_json::to_vec(&doc).unwrap_or_default();
    pad4(&mut text, b' ');
    let total = 12 + 8 + text.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let len32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend(len32(total));
    out.extend(len32(text.len()));
    out.extend(b"JSON");
    out.extend(text);
    if !bin.is_empty() {
        out.extend(len32(bin.len()));
        out.extend(b"BIN\0");
        out.extend(bin);
    }
    out
}
