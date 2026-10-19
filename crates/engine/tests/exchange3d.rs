//! 3D exchange through EXPORT, IMPORT and STLOUT (Q60): GLB and OBJ (with its MTL beside it)
//! out and back in as meshes with their materials; STLOUT writes the selection.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::{Common, EntityKind, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn session() -> Session {
    hooks();
    let mut s = Session::new();
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![v(0.0, 0.0, 0.0), v(10.0, 0.0, 0.0), v(10.0, 10.0, 0.0), v(0.0, 10.0, 0.0), v(5.0, 5.0, 8.0)];
    let f = |a: &[u32]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f(&[0, 3, 2, 1]), f(&[0, 1, 4]), f(&[1, 2, 4]), f(&[2, 3, 4]), f(&[3, 0, 4])];
    s.execute("material.new", &json!({ "name": "Copper", "color": "#B87333", "metallic": 1, "roughness": 0.35 })).unwrap();
    let st = s.state_mut().unwrap();
    let d = std::sync::Arc::make_mut(&mut st.doc);
    d.add(
        &Space::Model,
        Common { material: Some("Copper".into()), ..Common::default() },
        EntityKind::Mesh(Mesh { vertices, faces, kind: MeshKind::Subd }),
    )
    .unwrap();
    s
}

fn mesh_tris(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.triangles().len()) } else { None }).sum()
}

#[test]
fn export_and_import_glb_and_obj() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("cadaddy-x3d-engine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["pyramid.glb", "pyramid.gltf", "pyramid.obj", "pyramid.stl"] {
        let path = dir.join(name);
        s.execute("export", &json!({ "path": path.to_string_lossy() })).unwrap();
        assert!(path.exists(), "{name}");
    }
    // OBJ's materials beside it.
    assert!(std::fs::read_to_string(dir.join("pyramid.mtl")).unwrap().contains("newmtl Copper"));
    // Into a new drawing: the same 6 triangles, the material along (not for STL).
    for (name, material) in [("pyramid.glb", true), ("pyramid.gltf", true), ("pyramid.obj", true), ("pyramid.stl", false)] {
        let mut t = Session::new();
        let r = t.execute("import", &json!({ "path": dir.join(name).to_string_lossy() })).unwrap();
        assert_eq!(mesh_tris(&t), 6, "{name}: {r}");
        assert_eq!(t.doc().unwrap().materials.iter().any(|m| m.name == "Copper"), material, "{name}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stlout_writes_the_selection() {
    let mut s = session();
    let all: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    let r = s.execute("stlout", &json!({ "handles": all })).unwrap();
    assert_eq!(r["triangles"], 6);
    // A circle has no surface.
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    assert!(s.execute("stlout", &json!({ "handles": [c["handle"]] })).is_err());
    assert!(s.execute("export", &json!({ "format": "fbx" })).is_err());
}
