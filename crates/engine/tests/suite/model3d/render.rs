//! RENDER (Q57): the current view path traced, the time per sample reported, the image kept for
//! the Render window and written as PNG or JPEG; RENDERCROP renders a region; bad input refused.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, EntityKind, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

fn session_with_box() -> Session {
    let mut s = Session::new();
    s.viewport_px = (320.0, 200.0);
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(4.0, 0.0, 0.0),
        v(4.0, 4.0, 0.0),
        v(0.0, 4.0, 0.0),
        v(0.0, 0.0, 2.0),
        v(4.0, 0.0, 2.0),
        v(4.0, 4.0, 2.0),
        v(0.0, 4.0, 2.0),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let st = s.state_mut().unwrap();
    std::sync::Arc::make_mut(&mut st.doc)
        .add(&Space::Model, Common::default(), EntityKind::Mesh(Mesh { vertices, faces, kind: MeshKind::Subd }))
        .unwrap();
    s.execute("view.seiso", &json!({})).unwrap();
    s.execute("zoom.extents", &json!({})).unwrap();
    s
}

#[test]
fn render_traces_the_view_and_saves_it() {
    let mut s = session_with_box();
    let r = s.execute("render", &json!({ "width": 64, "height": 40, "samples": 3, "preset": "draft" })).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["samples"].as_u64()), (Some(64), Some(40), Some(3)), "{r}");
    assert!(r["secondsPerSample"].as_f64().unwrap() > 0.0);
    let img = s.last_render.clone().unwrap().image;
    assert_eq!((img.width, img.height), (64, 40));
    // Not all one colour: the box stands out against the sky.
    let first = &img.rgba[0..3];
    assert!(img.rgba.chunks(4).any(|p| &p[0..3] != first));
    let dir = std::env::temp_dir().join(format!("cadaddy-render-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("r.png");
    s.execute("render", &json!({ "width": 32, "height": 20, "samples": 1, "output": png.to_string_lossy() })).unwrap();
    assert!(std::fs::read(&png).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
    let jpg = dir.join("r.jpg");
    s.execute("saveimg", &json!({ "path": jpg.to_string_lossy() })).unwrap();
    assert!(std::fs::read(&jpg).unwrap().starts_with(&[0xFF, 0xD8]));
    std::fs::remove_dir_all(&dir).ok();
    // RENDERCROP: only the region is traced.
    let v = s.state().unwrap().view();
    let c = v.center;
    s.execute(
        "rendercrop",
        &json!({ "corner1": [c.x - 0.5, c.y - 0.5], "corner2": [c.x + 0.5, c.y + 0.5], "width": 64, "height": 40, "samples": 1 }),
    )
    .unwrap();
    assert_eq!(s.last_render.as_ref().unwrap().samples, 1);
}

#[test]
fn bad_render_input_is_refused() {
    let mut s = session_with_box();
    for p in [
        json!({ "samples": 0 }),
        json!({ "samples": 10_000_000 }),
        json!({ "samples": 1.5 }),
        json!({ "bounces": 100 }),
        json!({ "preset": "ultra" }),
        json!({ "width": 0 }),
        json!({ "width": 100_000 }),
        json!({ "crop": [[-5, 0], [10, 10]] }),
        json!({ "crop": [[100, 100], [200, 200]], "width": 50, "height": 50, "samples": 1 }),
        json!({ "samples": 1, "width": 8, "output": "/nonexistent-dir/x.png" }),
        json!({ "samples": 1, "width": 8, "output": "x.bmp" }),
    ] {
        assert!(s.execute("render", &p).is_err(), "{p}");
    }
    assert!(Session::new().execute("saveimg", &json!({ "path": "x.png" })).is_err());
    // An empty drawing has nothing to render.
    let mut e = Session::new();
    assert!(e.execute("render", &json!({ "samples": 1 })).is_err());
}
