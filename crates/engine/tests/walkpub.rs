//! PUBLISH WALKTHROUGH: a package that opens as the drawing, straight into Walk, with the spec,
//! the built model as glTF and the lightmap.

use cadaddy_engine::Session;
use cadaddy_walk3d::package::{self, Package};
use serde_json::json;
use std::sync::Arc;

fn install_io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

/// A 20' × 10' room of 6" walls (inches) on A-WALL.
fn room() -> Session {
    let mut s = Session::new();
    s.execute("layer.set", &json!({"name": "A-WALL", "current": true})).ok();
    s.execute("layer", &json!({"name": "A-WALL", "make": true})).ok();
    for (a, b) in [((0, 0), (240, 120)), ((6, 6), (234, 114))] {
        s.execute("rectang", &json!({"p1": [a.0, a.1], "p2": [b.0, b.1]})).unwrap();
    }
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("chprop", &json!({"handles": hs, "layer": "A-WALL"})).ok();
    s
}

#[test]
fn a_published_walkthrough_opens_into_walk_with_its_model_and_lightmap() {
    install_io();
    let mut s = room();
    let walls_layer = s.doc().unwrap().model.iter().all(|e| e.common.layer == "A-WALL");
    // A small lightmap stands in for a bake.
    let lm = cadaddy_render::lightmap::Lightmap { width: 4, height: 4, texels: vec![[0.5, 0.4, 0.3]; 16], objects: Default::default(), lighting: 7 };
    s.state_mut().unwrap().lightmap = Some(Arc::new(lm));
    let dir = std::env::temp_dir().join(format!("cadaddy-walkpub-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Lobby.cdywalk");
    let r = s.execute("walk.publish", &json!({"path": path.display().to_string()})).unwrap();
    assert_eq!(r["spec"], "drafted", "no spec in the drawing: drafted from the walk rules ({r})");
    assert_eq!(r["lightmap"], true);
    assert_eq!(r["model"], true, "{r}");
    assert!(s.doc().unwrap().building_spec.is_none(), "the drawing itself is left as it was");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(r["bytes"].as_u64(), Some(bytes.len() as u64));
    let p = Package::decode(&bytes).unwrap();
    for f in [package::MANIFEST, package::DRAWING, package::MODEL, package::SPEC, package::LIGHTMAP_PNG, package::LIGHTMAP_JSON] {
        assert!(p.get(f).is_some(), "{f} in {:?}", p.files.iter().map(|f| &f.0).collect::<Vec<_>>());
    }
    assert_eq!(p.manifest().unwrap()["title"], s.state().unwrap().title.as_str());
    // The built model: a glTF with meshes when the plan has walls.
    let glb = p.get(package::MODEL).unwrap();
    assert!(glb.starts_with(b"glTF"));
    if walls_layer {
        assert!(glb.len() > 1_000, "walls in the glTF ({} bytes)", glb.len());
    }
    // Opened (as the web build does, from bytes): the drawing, its spec, Walk, the lightmap.
    let mut o = Session::new();
    let b64 = {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in bytes.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                out.push(if i <= c.len() { T[((n >> (18 - 6 * i)) & 63) as usize] as char } else { '=' });
            }
        }
        out
    };
    o.execute("open", &json!({"data": b64, "name": "Lobby.cdywalk"})).unwrap();
    let st = o.state().unwrap();
    assert!(st.walkthrough);
    assert_eq!(st.doc.model.len(), s.doc().unwrap().model.len());
    assert!(st.doc.building_spec.is_some());
    assert_eq!(st.lightmap.as_ref().map(|l| (l.width, l.lighting)), Some((4, 7)));
    assert!(st.path.is_none(), "QSAVE asks where to save instead of overwriting the package");
    // A plain drawing isn't a walkthrough; a damaged package doesn't open.
    let mut n = Session::new();
    assert!(!n.state().unwrap().walkthrough);
    let cut = &bytes[..bytes.len() / 2];
    std::fs::write(dir.join("cut.cdywalk"), cut).unwrap();
    assert!(n.execute("open", &json!({"path": dir.join("cut.cdywalk").display().to_string()})).is_err());
    // Default path: beside the drawing.
    s.state_mut().unwrap().path = Some(dir.join("Plan.dxf").display().to_string());
    let r = s.execute("walk.publish", &json!({})).unwrap();
    assert!(r["path"].as_str().unwrap().ends_with("Plan.cdywalk"));
    let _ = std::fs::remove_dir_all(&dir);
}
