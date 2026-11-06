//! LIGHTBAKE: indirect light baked onto the model's surfaces, kept with the drawing and beside
//! its file, rebaked only where the model changed.

use cadaddy_doc::{Common, Drawing, EntityKind, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use serde_json::json;

/// A 4 m room open to the sky (millimetres): a floor and four walls, one mesh each.
fn room() -> Drawing {
    let mut d = Drawing::new_metric();
    d.sun.on = true;
    d.sun.minutes = 13 * 60;
    let v = |x: f64, y: f64, z: f64| Vec3::new(x * 1000.0, y * 1000.0, z * 1000.0);
    let quads = [
        [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0)],
        [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 0.0, 3.0), v(0.0, 0.0, 3.0)],
        [v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(4.0, 4.0, 3.0), v(4.0, 0.0, 3.0)],
        [v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0), v(0.0, 4.0, 3.0), v(4.0, 4.0, 3.0)],
        [v(0.0, 4.0, 0.0), v(0.0, 0.0, 0.0), v(0.0, 0.0, 3.0), v(0.0, 4.0, 3.0)],
    ];
    for q in quads {
        let m = Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }], kind: MeshKind::Polyface };
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(m)).unwrap();
    }
    d
}

#[test]
fn a_room_bakes_and_rebakes_only_what_changed() {
    let mut s = Session::empty();
    s.open_drawing(room(), "Room", None);
    let r = s.execute("lightbake", &json!({"quality": "draft", "texel": 500})).unwrap();
    assert_eq!((r["objects"].clone(), r["baked"].clone(), r["kept"].clone()), (json!(5), json!(5), json!(0)), "{r}");
    assert!(r["texels"].as_u64().unwrap() > 0);
    assert!(r.get("saved").is_none(), "not saved: kept with the open drawing only");
    let rev = s.state().unwrap().lightmap_rev;
    assert!(s.state().unwrap().lightmap.is_some());
    let again = s.execute("lightbake", &json!({"quality": "draft", "texel": 500})).unwrap();
    assert_eq!((again["baked"].clone(), again["kept"].clone()), (json!(0), json!(5)));
    assert!(s.state().unwrap().lightmap_rev > rev, "views refresh");
    // `full` bakes everything again.
    assert_eq!(s.execute("lightbake", &json!({"quality": "draft", "texel": 500, "full": true})).unwrap()["baked"], json!(5));
    // The view's scene gets the baked light for every triangle.
    let scene = cadaddy_render::shade::scene(s.doc().unwrap(), &Space::Model, 1.0, cadaddy_doc::color::Rgb(0, 0, 0));
    let baked = cadaddy_engine::cmd::lightbake::baked_for(&mut s, &scene).unwrap();
    assert!(baked.cells.iter().all(Option::is_some));
    // Clear.
    assert_eq!(s.execute("lightbake", &json!({"clear": true})).unwrap()["cleared"], json!(true));
    assert!(s.state().unwrap().lightmap.is_none());
}

#[test]
fn the_lightmap_is_saved_beside_the_drawing_and_found_again() {
    let dir = std::env::temp_dir().join(format!("cadaddy-lightbake-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("room.dxf").display().to_string();
    let mut s = Session::empty();
    s.open_drawing(room(), "room.dxf", Some(path.clone()));
    let r = s.execute("lightbake", &json!({"quality": "draft", "texel": 500})).unwrap();
    let png = dir.join("room.lightmap.png");
    assert_eq!(r["saved"], json!(png.display().to_string()));
    assert!(png.is_file() && dir.join("room.lightmap.json").is_file());
    let baked = s.state().unwrap().lightmap.clone().unwrap();
    // Opened again: found beside it, the same cells and (RGBM-rounded) light.
    let mut t = Session::empty();
    t.open_drawing(room(), "room.dxf", Some(path.clone()));
    let found = cadaddy_engine::cmd::lightbake::current(&mut t).unwrap();
    assert_eq!((found.width, found.height, found.objects.len()), (baked.width, baked.height, baked.objects.len()));
    let sum = |m: &cadaddy_render::lightmap::Lightmap| m.texels.iter().map(|t| f64::from(t[0] + t[1] + t[2])).sum::<f64>();
    assert!((sum(&found) - sum(&baked)).abs() <= 0.03 * sum(&baked).max(1e-6), "{} {}", sum(&found), sum(&baked));
    // A rebake there keeps everything (the hashes match).
    assert_eq!(t.execute("lightbake", &json!({"quality": "draft", "texel": 500})).unwrap()["kept"], json!(5));
    // A broken index is ignored, not fatal.
    std::fs::write(dir.join("room.lightmap.json"), "{not json").unwrap();
    let mut u = Session::empty();
    u.open_drawing(room(), "room.dxf", Some(path));
    assert!(cadaddy_engine::cmd::lightbake::current(&mut u).is_none());
    // Clear removes the files.
    t.execute("lightbake", &json!({"clear": true})).unwrap();
    assert!(!png.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_input_is_an_error() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_metric(), "Empty", None);
    assert!(s.execute("lightbake", &json!({})).is_err(), "nothing to bake");
    s.open_drawing(room(), "Room", None);
    assert!(s.execute("lightbake", &json!({"quality": "ultra"})).is_err());
    assert!(s.execute("lightbake", &json!({"samples": 0})).is_err());
    assert!(s.execute("lightbake", &json!({"texel": -1})).is_err());
    // Typed: Draft.
    s.script("LIGHTBAKE D\n").unwrap();
    assert!(s.state().unwrap().lightmap.is_some());
}
