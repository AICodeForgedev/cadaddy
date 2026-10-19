//! 3D exchange (Q60): glTF/GLB, OBJ+MTL and STL out and back in — the same triangles and bounds,
//! materials kept, block references as instances; the GLB is checked structurally; hostile files
//! are refused without panicking.

use cadaddy_color::Rgb;
use cadaddy_doc::material::Material;
use cadaddy_doc::*;
use cadaddy_geom::Vec3;
use cadaddy_io::mesh3d_io::{read_gltf, read_obj, read_stl, write_glb, write_gltf, write_obj, write_stl};
use serde_json::Value;

fn boxmesh(lo: Vec3, hi: Vec3) -> Mesh {
    let v = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
    let vertices = vec![
        v(false, false, false),
        v(true, false, false),
        v(true, true, false),
        v(false, true, false),
        v(false, false, true),
        v(true, false, true),
        v(true, true, true),
        v(false, true, true),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    Mesh {
        vertices,
        faces: vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
        kind: MeshKind::Subd,
    }
}

/// A painted 100 mm box, and a block of a 20 mm box referenced twice.
fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    d.header.set_i64("INSUNITS", 4);
    let mut m = Material::new("Red paint");
    m.color = Rgb(200, 0, 0);
    m.roughness = 0.3;
    d.materials.push(m);
    let c = Common { material: Some("Red paint".into()), ..Common::default() };
    d.add(&Space::Model, c, EntityKind::Mesh(boxmesh(Vec3::ZERO, Vec3::new(100.0, 100.0, 100.0)))).unwrap();
    let mut b = Block::new("Cube");
    b.entities.push(Entity::new(Handle(0x9000), EntityKind::Mesh(boxmesh(Vec3::ZERO, Vec3::new(20.0, 20.0, 20.0)))));
    d.blocks.insert("Cube".into(), std::sync::Arc::new(b));
    for x in [200.0, 300.0] {
        let ins = Insert {
            block: "Cube".into(),
            insert: Vec3::new(x, 0.0, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
    }
    d
}

/// Triangle count and bounds of the meshes in a drawing.
fn summary(d: &Drawing) -> (usize, Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    let mut n = 0;
    for e in d.model.iter() {
        if let EntityKind::Mesh(m) = &e.kind {
            n += m.triangles().len();
            for p in &m.vertices {
                lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
                hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
            }
        }
    }
    (n, lo, hi)
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).len() < 1e-3
}

/// 12 triangles a box, three boxes; from (0, 0, 0) to (320, 100, 100).
fn expected() -> (usize, Vec3, Vec3) {
    (36, Vec3::ZERO, Vec3::new(320.0, 100.0, 100.0))
}

fn check(back: &Drawing, what: &str) {
    let (n, lo, hi) = summary(back);
    let (en, elo, ehi) = expected();
    assert_eq!(n, en, "{what}");
    assert!(near(lo, elo) && near(hi, ehi), "{what}: {lo:?} {hi:?}");
}

#[test]
fn glb_is_well_formed_and_round_trips() {
    let glb = write_glb(&drawing());
    // Header and chunks.
    assert_eq!(&glb[0..4], b"glTF");
    let u32_at = |o: usize| u32::from_le_bytes([glb[o], glb[o + 1], glb[o + 2], glb[o + 3]]) as usize;
    assert_eq!(u32_at(4), 2);
    assert_eq!(u32_at(8), glb.len());
    let jlen = u32_at(12);
    assert_eq!(&glb[16..20], b"JSON");
    assert_eq!(jlen % 4, 0);
    let doc: Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
    let blen = u32_at(20 + jlen);
    assert_eq!(&glb[24 + jlen..28 + jlen], b"BIN\0");
    assert_eq!(blen % 4, 0);
    assert_eq!(doc["buffers"][0]["byteLength"].as_u64().unwrap() as usize, blen);
    // Every view inside the buffer; every accessor inside its view.
    let views = doc["bufferViews"].as_array().unwrap();
    for v in views {
        assert!(v["byteOffset"].as_u64().unwrap() + v["byteLength"].as_u64().unwrap() <= blen as u64);
    }
    for a in doc["accessors"].as_array().unwrap() {
        let comps = match a["type"].as_str().unwrap() {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            _ => 4,
        };
        let v = &views[a["bufferView"].as_u64().unwrap() as usize];
        assert!(a["count"].as_u64().unwrap() * comps * 4 <= v["byteLength"].as_u64().unwrap(), "{a}");
    }
    // POSITION accessors carry min and max; the block is one mesh placed twice.
    for m in doc["meshes"].as_array().unwrap() {
        for p in m["primitives"].as_array().unwrap() {
            let a = &doc["accessors"][p["attributes"]["POSITION"].as_u64().unwrap() as usize];
            assert!(a["min"].is_array() && a["max"].is_array());
        }
    }
    let cubes: Vec<&Value> = doc["nodes"].as_array().unwrap().iter().filter(|n| n["name"] == "Cube").collect();
    assert_eq!(cubes.len(), 2);
    assert_eq!(cubes[0]["mesh"], cubes[1]["mesh"]);
    // The material, with its colour and roughness.
    let red = doc["materials"].as_array().unwrap().iter().find(|m| m["name"] == "Red paint").unwrap();
    assert!((red["pbrMetallicRoughness"]["roughnessFactor"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    // Back in: the same triangles in the same place (millimetres), the material kept.
    let back = read_gltf(&glb, "x.glb").unwrap();
    check(&back, "glb");
    let m = back.materials.iter().find(|m| m.name == "Red paint").unwrap();
    assert!(m.color.0 >= 199 && m.color.1 <= 1, "{:?}", m.color);
    check(&read_gltf(&write_gltf(&drawing()), "x.gltf").unwrap(), "gltf");
}

#[test]
fn obj_and_stl_round_trip() {
    let dir = std::env::temp_dir().join(format!("cadaddy-x3d-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = write_obj(&drawing(), "model.mtl");
    std::fs::write(dir.join("model.obj"), &out.obj).unwrap();
    std::fs::write(dir.join("model.mtl"), &out.mtl).unwrap();
    let back = read_obj(out.obj.as_bytes(), &dir.join("model.obj").to_string_lossy()).unwrap();
    check(&back, "obj");
    assert!(back.materials.iter().any(|m| m.name == "Red_paint" && m.color.0 >= 199), "{:?}", back.materials);
    std::fs::remove_dir_all(&dir).ok();
    let stl = write_stl(&drawing());
    assert_eq!(stl.len(), 84 + 36 * 50);
    check(&read_stl(&stl).unwrap(), "stl");
    // ASCII STL.
    let ascii = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
    assert_eq!(summary(&read_stl(ascii.as_bytes()).unwrap()).0, 1);
}

#[test]
fn hostile_files_are_refused() {
    let glb = write_glb(&drawing());
    let mut tampered = glb.clone();
    // A JSON length past the end.
    tampered[12] = 0xFF;
    tampered[13] = 0xFF;
    assert!(read_gltf(&tampered, "x.glb").is_err());
    assert!(read_gltf(&glb[..30], "x.glb").is_err());
    // Accessors pointing past their buffer, absurd counts and nodes that loop.
    let bad = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"children":[0]}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":4000000000,"type":"VEC3"},{"bufferView":0,"byteOffset":99999999,"componentType":5125,"count":3,"type":"SCALAR"}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12}],"buffers":[{"byteLength":12,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAA"}]}"#;
    assert!(read_gltf(bad.as_bytes(), "x.gltf").is_err());
    assert!(read_gltf(b"not json", "x.gltf").is_err());
    assert!(read_obj(b"v 0 0 0\nf 1 2 3\nf -9 0 99999999999999999999\n", "x.obj").is_err());
    assert!(read_stl(&[0u8; 50]).is_err());
    let mut lying = vec![0u8; 84];
    lying[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read_stl(&lying).is_err());
}
