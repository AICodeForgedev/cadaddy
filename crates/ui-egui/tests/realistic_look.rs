//! The Realistic style's ambient occlusion, soft shadows and tone curve (VISUALSTYLES) drawn on
//! the GPU and by the CPU: each setting changes both images alike, and the two paths draw the
//! same scene about the same (image statistics, so multisampling and filtering may differ).

use cadaddy_doc::{Common, Drawing, EntityKind, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

/// A 10 m floor and a 2 m box on it (millimetres), the sun out.
fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    d.sun.on = true;
    d.sun.minutes = 14 * 60;
    let v = |x: f64, y: f64, z: f64| Vec3::new(x * 1000.0, y * 1000.0, z * 1000.0);
    let quad = |q: [Vec3; 4]| Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }], kind: MeshKind::Polyface };
    for q in [
        [v(-5.0, -5.0, 0.0), v(5.0, -5.0, 0.0), v(5.0, 5.0, 0.0), v(-5.0, 5.0, 0.0)],
        [v(-1.0, -1.0, 2.0), v(1.0, -1.0, 2.0), v(1.0, 1.0, 2.0), v(-1.0, 1.0, 2.0)],
        [v(-1.0, -1.0, 0.0), v(1.0, -1.0, 0.0), v(1.0, -1.0, 2.0), v(-1.0, -1.0, 2.0)],
        [v(-1.0, -1.0, 0.0), v(-1.0, 1.0, 0.0), v(-1.0, 1.0, 2.0), v(-1.0, -1.0, 2.0)],
        [v(1.0, -1.0, 0.0), v(1.0, 1.0, 0.0), v(1.0, 1.0, 2.0), v(1.0, -1.0, 2.0)],
        [v(-1.0, 1.0, 0.0), v(1.0, 1.0, 0.0), v(1.0, 1.0, 2.0), v(-1.0, 1.0, 2.0)],
    ] {
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(quad(q))).unwrap();
    }
    d
}

fn harness(gpu: bool) -> Harness<'static, CadApp> {
    let mut s = Session::empty();
    s.open_drawing(drawing(), "Courtyard", None);
    let mut app = CadApp::new(s, Services::default());
    let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::default());
    app.set_wgpu(&rs, None);
    if !gpu {
        app.run("ui.gpu", json!({"cpu": true})).unwrap();
    }
    app.session.execute("vscurrent", &json!({"style": "realistic"})).unwrap();
    app.session.execute("view.dir", &json!({"dir": [-1, -1.5, 1], "fit": true})).unwrap();
    Harness::builder().with_size(vec2(1000.0, 700.0)).renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    )
}

/// Frames until the model is built and drawn; the frame.
fn frame(h: &mut Harness<'static, CadApp>) -> image::RgbaImage {
    for _ in 0..400 {
        h.run_steps(1);
        let st = h.state().canvas.shade.stats;
        if st.building.is_none() && st.tris_total > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(3);
    h.render().expect("a frame")
}

fn luma(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

/// How much darker `b` is than `a`, summed over the pixels (levels).
fn darkening(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    a.pixels().zip(b.pixels()).map(|(p, q)| (luma(p.0) - luma(q.0)).max(0.0)).sum()
}

/// Pixels that differ by more than a few levels.
fn changed(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    a.pixels().zip(b.pixels()).filter(|(p, q)| (luma(p.0) - luma(q.0)).abs() > 4.0).count()
}

/// The frames with each look (in order): no occlusion and sharp shadows; occlusion; soft
/// shadows; the linear tone curve.
fn looks(gpu: bool) -> Vec<image::RgbaImage> {
    let mut h = harness(gpu);
    let mut out = Vec::new();
    for p in [
        json!({"ambientOcclusion": 0, "softShadows": 0, "toneMap": "filmic"}),
        json!({"ambientOcclusion": 1.5}),
        json!({"ambientOcclusion": 0, "softShadows": 4}),
        json!({"softShadows": 0, "toneMap": "linear"}),
    ] {
        h.state_mut().session.execute("visualstyles", &p).unwrap();
        out.push(frame(&mut h));
        assert_eq!(h.state().canvas.shade.gpu_size().is_some(), gpu, "drawn on the {}", if gpu { "GPU" } else { "CPU" });
    }
    if let Ok(dir) = std::env::var("CDY_RENDER_DUMP") {
        for (i, img) in out.iter().enumerate() {
            img.save(format!("{dir}/look-{}-{i}.png", if gpu { "gpu" } else { "cpu" })).unwrap();
        }
    }
    out
}

#[test]
fn the_gpu_and_the_cpu_draw_the_realistic_look_alike() {
    let gpu = looks(true);
    let cpu = looks(false);
    let (Some([g0, g_ao, g_soft, g_lin]), Some([c0, c_ao, c_soft, c_lin])) = (gpu.get(..4), cpu.get(..4)) else { panic!("four frames each") };
    // Ambient occlusion darkens both, about as much.
    let (dg, dc) = (darkening(g0, g_ao), darkening(c0, c_ao));
    eprintln!(
        "occlusion GPU {dg:.0} CPU {dc:.0}; soft GPU {} CPU {}; tone GPU {} CPU {}",
        changed(g0, g_soft),
        changed(c0, c_soft),
        changed(g0, g_lin),
        changed(c0, c_lin)
    );
    assert!(dg > 2000.0 && dc > 2000.0, "occlusion darkens: GPU {dg}, CPU {dc}");
    assert!(dg / dc > 0.4 && dg / dc < 2.5, "about as much: GPU {dg}, CPU {dc}");
    // Soft shadows change the shadow's edges on both, about as many pixels.
    let (sg, sc) = (changed(g0, g_soft), changed(c0, c_soft));
    assert!(sg > 100 && sc > 100, "soft shadows show: GPU {sg}, CPU {sc}");
    assert!((sg as f64 / sc as f64) > 0.3 && (sg as f64 / sc as f64) < 3.0, "about as many: GPU {sg}, CPU {sc}");
    // The tone curve changes most of the view on both.
    let (tg, tc) = (changed(g0, g_lin), changed(c0, c_lin));
    assert!(tg > 20_000 && tc > 20_000, "tone curve: GPU {tg}, CPU {tc}");
    // The same scene the same way: mean brightness within a few levels, most pixels close.
    let mean = |img: &image::RgbaImage| img.pixels().map(|p| luma(p.0)).sum::<f64>() / f64::from(img.width() * img.height());
    for (g, c) in [(g0, c0), (g_ao, c_ao), (g_lin, c_lin)] {
        assert!((mean(g) - mean(c)).abs() < 4.0, "mean GPU {} CPU {}", mean(g), mean(c));
        let close = g.pixels().zip(c.pixels()).filter(|(p, q)| (luma(p.0) - luma(q.0)).abs() <= 12.0).count();
        let share = close as f64 / f64::from(g.width() * g.height());
        assert!(share > 0.95, "{share} of the pixels close");
    }
}
