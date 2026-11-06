//! The 3D views under forced GPU limits (`ui.gpu`): no MSAA and a tiny texture and memory budget
//! still draw the shaded view (smaller, scaled up); forcing the CPU draws both the shaded view
//! and Walk without the GPU; a GPU error moves the 3D views to the CPU and says so.

use cadaddy_doc::{Common, Drawing, EntityKind, Line, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

/// A 4 m box of faces (millimetres), and 2D walls for Walk.
fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let v = |x: f64, y: f64, z: f64| Vec3::new(x * 1000.0, y * 1000.0, z * 1000.0);
    let quad = |q: [Vec3; 4]| Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }], kind: MeshKind::Polyface };
    for q in [
        [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0)],
        [v(0.0, 4.0, 0.0), v(4.0, 4.0, 0.0), v(4.0, 4.0, 3.0), v(0.0, 4.0, 3.0)],
        [v(0.0, 0.0, 0.0), v(0.0, 4.0, 0.0), v(0.0, 4.0, 3.0), v(0.0, 0.0, 3.0)],
    ] {
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(quad(q))).unwrap();
    }
    d
}

fn app(gpu: bool) -> (CadApp, Option<egui_wgpu::RenderState>) {
    let mut s = Session::empty();
    s.open_drawing(drawing(), "Room", None);
    let mut app = CadApp::new(s, Services::default());
    let rs = gpu.then(|| egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::default()));
    if let Some(rs) = &rs {
        app.set_wgpu(rs, None);
    }
    app.session.execute("vscurrent", &json!({"style": "realistic"})).unwrap();
    app.session.execute("view.dir", &json!({"dir": [-1, -1.5, 1], "fit": true})).unwrap();
    (app, rs)
}

fn harness(app: CadApp, rs: Option<egui_wgpu::RenderState>) -> Harness<'static, CadApp> {
    let b = Harness::builder().with_size(vec2(1000.0, 700.0));
    let b = match rs {
        Some(rs) => b.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs)),
        None => b,
    };
    b.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    )
}

/// Frames until the model is built and drawn.
fn settle(h: &mut Harness<'static, CadApp>) {
    for _ in 0..400 {
        h.run_steps(1);
        let st = h.state().canvas.shade.stats;
        if st.building.is_none() && st.tris_total > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(3);
}

/// Distinct colours in the middle of the image (a drawn model has several; an empty or black
/// view has one or two).
fn colours(img: &image::RgbaImage) -> usize {
    let (w, h) = img.dimensions();
    let mut seen = std::collections::BTreeSet::new();
    for y in (h / 3..2 * h / 3).step_by(4) {
        for x in (w / 3..2 * w / 3).step_by(4) {
            let p = img.get_pixel(x, y).0;
            seen.insert((p[0] / 8, p[1] / 8, p[2] / 8));
        }
    }
    seen.len()
}

#[test]
fn forced_limits_still_draw_the_shaded_view() {
    let (mut app, rs) = app(true);
    let caps = app.run("ui.gpu", json!({"msaa": 1, "maxTexture": 256, "budgetMb": 1})).unwrap();
    assert_eq!((caps["msaa"].clone(), caps["maxTexture"].clone(), caps["budgetMb"].clone()), (json!(1), json!(256), json!(1)), "{caps}");
    assert!(caps["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("256")));
    let mut h = harness(app, rs);
    settle(&mut h);
    let (w, gh) = h.state().canvas.shade.gpu_size().expect("drawn on the GPU");
    assert!(w <= 256 && gh <= 256, "within the forced texture size: {w}x{gh}");
    assert!(u64::from(w) * u64::from(gh) * 12 <= 1024 * 1024, "within the 1 MB budget: {w}x{gh}");
    let img = h.render().expect("a frame");
    assert!(colours(&img) >= 3, "the model is drawn, scaled up ({} colours)", colours(&img));
    // Back to what the GPU has: the full size again.
    h.state_mut().run("ui.gpu", json!({"reset": true})).unwrap();
    h.run_steps(4);
    let (w2, _) = h.state().canvas.shade.gpu_size().unwrap();
    assert!(w2 > w, "{w2} > {w}");
}

#[test]
fn forcing_the_cpu_draws_both_3d_views_without_the_gpu() {
    let (mut app, rs) = app(true);
    app.run("ui.gpu", json!({"cpu": true})).unwrap();
    let mut h = harness(app, rs);
    settle(&mut h);
    assert!(h.state().canvas.shade.gpu_size().is_none(), "not on the GPU");
    let img = h.render().expect("a frame");
    assert!(colours(&img) >= 3, "the CPU drew the model ({} colours)", colours(&img));
    // Walk: the 2D plan walked, drawn by the CPU.
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0.0, 0.0), (240.0, 0.0)), ((240.0, 0.0), (240.0, 240.0)), ((240.0, 240.0), (0.0, 240.0)), ((0.0, 240.0), (0.0, 0.0))] {
        let c = Common { layer: "A-WALL".into(), ..Common::default() };
        d.add(&Space::Model, c, EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) })).unwrap();
    }
    h.state_mut().session.open_drawing(d, "Plan", None);
    h.state_mut().run("ui.walk", serde_json::Value::Null).unwrap();
    h.run_steps(4);
    assert!(h.state().walk.as_ref().is_some_and(|w| w.on_cpu()), "Walk drawn by the CPU");
    let img = h.render().expect("a frame");
    assert!(colours(&img) >= 2, "walls and sky");
}

#[test]
fn a_gpu_error_moves_the_3d_views_to_the_cpu() {
    let (mut app, _rs) = app(false);
    // Without a GPU the CPU draws from the start.
    assert!(app.gpu_caps.cpu);
    app.gpu_caps.cpu = false;
    app.gpu_faults.error("out of memory".into());
    let ctx = egui::Context::default();
    app.gpu_fault_tick(&ctx);
    assert!(app.gpu_caps.cpu);
    assert!(app.gpu_caps.notes.iter().any(|n| n.contains("GPU error")), "{:?}", app.gpu_caps.notes);
    let r = app.run("ui.gpu", json!({})).unwrap();
    assert_eq!(r["cpu"], json!(true));
}
