//! LIGHTBAKE in the big-scene 3D view: the baked light reaches the background-built model's
//! base geometry, and a baked scene with instanced block references still renders (the orbit
//! benchmark draws its frames).

use cadaddy_doc::{Block, Common, Drawing, Entity, EntityKind, Handle, Insert, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

fn quad(q: [Vec3; 4]) -> Mesh {
    Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }], kind: MeshKind::Polyface }
}

/// A 4 m room open to the sky (millimetres) and three references to a block holding a box.
fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    d.sun.on = true;
    d.sun.minutes = 13 * 60;
    let v = |x: f64, y: f64, z: f64| Vec3::new(x * 1000.0, y * 1000.0, z * 1000.0);
    for q in [
        [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0)],
        [v(0.0, 4.0, 0.0), v(4.0, 4.0, 0.0), v(4.0, 4.0, 3.0), v(0.0, 4.0, 3.0)],
        [v(0.0, 0.0, 0.0), v(0.0, 4.0, 0.0), v(0.0, 4.0, 3.0), v(0.0, 0.0, 3.0)],
    ] {
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(quad(q))).unwrap();
    }
    let mut b = Block::new("LAMP");
    for q in [
        [v(0.0, 0.0, 0.0), v(0.3, 0.0, 0.0), v(0.3, 0.3, 0.0), v(0.0, 0.3, 0.0)],
        [v(0.0, 0.0, 0.3), v(0.3, 0.0, 0.3), v(0.3, 0.3, 0.3), v(0.0, 0.3, 0.3)],
    ] {
        b.entities.push(Entity::new(Handle(0x9000 + b.entities.len() as u64), EntityKind::Mesh(quad(q))));
    }
    d.blocks.insert("LAMP".into(), std::sync::Arc::new(b));
    for k in 0..3 {
        let i = Insert {
            block: "LAMP".into(),
            insert: v(1.0 + f64::from(k), 1.0, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        d.add(&Space::Model, Common::default(), EntityKind::Insert(i)).unwrap();
    }
    d
}

#[test]
fn a_baked_scene_renders_with_instancing() {
    let mut s = Session::empty();
    s.open_drawing(drawing(), "Room", None);
    let mut app = CadApp::new(s, Services::default());
    let r = app.session.execute("lightbake", &json!({"quality": "draft", "texel": 500})).unwrap();
    assert!(r["baked"].as_u64().unwrap() > 0, "{r}");
    app.session.execute("vscurrent", &json!({"style": "realistic"})).unwrap();
    app.session.execute("view.dir", &json!({"dir": [-1, -1.5, 1], "fit": true})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    // The model is built in the background: wait for it.
    for _ in 0..400 {
        h.run_steps(1);
        let st = h.state().canvas.shade.stats;
        if st.building.is_none() && st.tris_total > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let st = h.state().canvas.shade.stats;
    assert_eq!(st.instances, 3, "{st:?}");
    assert!(st.baked > 0, "the base geometry got its baked light: {st:?}");
    h.state_mut().run("ui.bench3d", json!({"frames": 6})).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().canvas.shade.stats.bench_fps.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let st = h.state().canvas.shade.stats;
    assert!(st.bench_frames > 0, "{st:?}");
}
