//! A walkthrough model in a .cdy file: its meshes, materials with their texture images, the baked
//! lightmap and the building spec all travel inside the file, so it opens lit and textured on
//! another machine without regenerating (the sidecar files and the image are gone).

use cadaddy_doc::{Common, Drawing, EntityKind, Mesh, MeshFace, MeshKind, Space};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use cadaddy_geom::Vec3;
use serde_json::json;

/// A 4 m room open to the sky (millimetres), its floor a fine mesh.
fn room() -> Drawing {
    let mut d = Drawing::new_metric();
    d.sun.on = true;
    d.sun.minutes = 13 * 60;
    let v = |x: f64, y: f64, z: f64| Vec3::new(x * 1000.0, y * 1000.0, z * 1000.0);
    let n = 9u32;
    let mut floor = Mesh { kind: MeshKind::Polyface, ..Mesh::default() };
    for j in 0..n {
        for i in 0..n {
            floor.vertices.push(v(f64::from(i) * 0.5, f64::from(j) * 0.5, 0.0));
        }
    }
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = j * n + i;
            floor.faces.push(MeshFace { v: vec![a, a + 1, a + n + 1, a + n], hidden: 0 });
        }
    }
    let floor_common = Common { layer: "FLOOR".into(), ..Common::default() };
    d.add(&Space::Model, floor_common, EntityKind::Mesh(floor)).unwrap();
    let walls = [
        [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 0.0, 3.0), v(0.0, 0.0, 3.0)],
        [v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(4.0, 4.0, 3.0), v(4.0, 0.0, 3.0)],
    ];
    for q in walls {
        let m = Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }], kind: MeshKind::Polyface };
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(m)).unwrap();
    }
    d.building_spec = Some(r#"{"levels":[{"name":"L1","elevation_ft":0,"height_ft":10}]}"#.into());
    d.tours = Some(r#"[{"name":"In","stops":[]}]"#.into());
    d
}

/// File reading and writing, as the apps install it.
fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    set_cdy_hooks(CdyHooks {
        save: |d, prev| {
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| None)?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as CdyState),
    });
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let rgba = (0..w * h).flat_map(|i| [(i * 7 % 256) as u8, 120, 60, 255]).collect();
    cadaddy_render::images::Raster::new(w, h, rgba).unwrap().encode_png().unwrap()
}

#[test]
fn a_walkthrough_model_opens_lit_and_textured_from_its_cdy_alone() {
    let dir = std::env::temp_dir().join(format!("cadaddy-cdy3d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tex")).unwrap();
    let texture = dir.join("tex/floor.png");
    std::fs::write(&texture, png(8, 8)).unwrap();
    let path = dir.join("room.cdy").display().to_string();
    hooks();
    let mut s = Session::empty();
    s.open_drawing(room(), "room.cdy", Some(path.clone()));
    s.execute("material.new", &json!({"name": "Floor", "colorMap": "tex/floor.png"})).unwrap();
    s.execute("materialattach", &json!({"layer": "FLOOR", "material": "Floor"})).unwrap();
    let r = s.execute("material.embed", &json!({})).unwrap();
    assert_eq!((r["embedded"].clone(), r["missing"].clone()), (json!(["tex/floor.png"]), json!([])), "{r}");
    let r = s.execute("lightbake", &json!({"quality": "draft", "texel": 500})).unwrap();
    assert_eq!(r["baked"], json!(3), "{r}");
    s.execute("saveas", &json!({"path": path})).unwrap();
    // Only the .cdy file goes to the other machine.
    for f in ["room.lightmap.png", "room.lightmap.json", "tex/floor.png"] {
        std::fs::remove_file(dir.join(f)).unwrap();
    }
    cadaddy_render::images::forget("tex/floor.png");
    let mut t = Session::new();
    t.execute("open", &json!({"path": path})).unwrap();
    let (a, b) = (s.doc().unwrap(), t.doc().unwrap());
    assert_eq!(b.model, a.model);
    assert_eq!((&b.materials, &b.building_spec, &b.tours, &b.embedded), (&a.materials, &a.building_spec, &a.tours, &a.embedded));
    // Lit from the file's own lightmap…
    let scene = cadaddy_render::shade::scene(b, &Space::Model, 1.0, cadaddy_doc::color::Rgb(0, 0, 0));
    let baked = cadaddy_engine::cmd::lightbake::baked_for(&mut t, &scene).expect("the lightmap came with the file");
    assert!(baked.cells.iter().all(Option::is_some));
    // …and textured from its own copy of the image.
    assert_eq!(scene.textures.len(), 1, "the floor's texture");
    assert_eq!((scene.textures[0].width, scene.textures[0].height), (8, 8));
    // Removing the copies and clearing the bake take them out of the drawing.
    t.execute("material.embed", &json!({"remove": true})).unwrap();
    t.execute("lightbake", &json!({"clear": true})).unwrap();
    assert!(t.doc().unwrap().embedded.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_texture_is_reported() {
    let mut s = Session::empty();
    s.open_drawing(room(), "room", None);
    s.execute("material.new", &json!({"name": "Gone", "colorMap": "/nowhere/at/all.png"})).unwrap();
    let r = s.execute("material.embed", &json!({})).unwrap();
    assert_eq!((r["embedded"].clone(), r["missing"].clone()), (json!([]), json!(["/nowhere/at/all.png"])));
    assert!(s.doc().unwrap().embedded.is_empty());
}
