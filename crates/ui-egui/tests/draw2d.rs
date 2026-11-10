//! Where the 2D canvas spends its time on a print set (`canvas::drawstats`, the `ui.draw2d`
//! control method and the `--debug` overlay): the synthetic print set of
//! `cadaddy_engine::sample::print_set`, opened in the app with the GPU canvas (any wgpu adapter,
//! a software one included; without one the CPU canvas), drawn zoomed out to the whole set, to
//! one sheet and to one room. Each view reports its regeneration (objects considered and drawn,
//! build time by kind of object, vertex data and upload) and its frames (primitives sent and in
//! view, the canvas's and the app's time, and the headless render: egui's tessellation, the GPU
//! draw and reading the image back), with the memory and the adapter.
//!
//! Zoomed out, the GPU canvas draws objects under a pixel as dots and small ones as boxes
//! (`gpu::build_screen_mesh`): the whole set is measured again with every object drawn as it is
//! (`CanvasState::full_detail`), and the two compared for GPU vertices and for the share of the
//! canvas the drawing lights (it must not thin out).
//!
//! CI runs 4 sheets and checks that the report adds up; the full set is run by hand:
//!
//! ```text
//! DRAW2D_SHEETS=100 cargo test --release -p cadaddy-ui-egui --test draw2d -- --nocapture
//! ```
//!
//! A program of its own: it creates a GPU device and reports wall-clock times.

use cadaddy_engine::{Session, View};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_ui_egui::{CadApp, Services};
use egui_kittest::Harness;
use serde_json::Value;

const SIZE: (f32, f32) = (1600.0, 1000.0);

/// A headless GPU, or `None` (no adapter: the CPU canvas is measured).
fn render_state() -> Option<egui_wgpu::RenderState> {
    std::panic::catch_unwind(|| {
        egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::PREDICTABLE)
    })
    .ok()
}

/// One view's numbers.
struct Row {
    view: &'static str,
    report: Value,
    /// Medians over the measured frames: the app's update, the headless render.
    frame_ms: f64,
    render_ms: f64,
    /// Share of the canvas's pixels the drawing lights (see [`coverage`]).
    coverage: f64,
}

/// Share of the canvas's pixels in `img` that are not the canvas background (the most common
/// colour there).
fn coverage(harness: &Harness<'_, CadApp>, img: &image::RgbaImage) -> f64 {
    let Some(rect) = harness.state().canvas.rect else { return 0.0 };
    let ppp = harness.ctx.pixels_per_point();
    let (x0, y0) = ((rect.min.x * ppp).max(0.0) as u32, (rect.min.y * ppp).max(0.0) as u32);
    let (x1, y1) = (((rect.max.x * ppp) as u32).min(img.width()), ((rect.max.y * ppp) as u32).min(img.height()));
    let mut counts = std::collections::HashMap::<[u8; 4], usize>::new();
    for y in y0..y1 {
        for x in x0..x1 {
            *counts.entry(img.get_pixel(x, y).0).or_default() += 1;
        }
    }
    let total: usize = counts.values().sum();
    let bg = counts.values().copied().max().unwrap_or(0);
    if total == 0 { 0.0 } else { (total - bg) as f64 / total as f64 }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Show `view` and wait for its regeneration, then time `frames` frames.
fn measure(harness: &mut Harness<'_, CadApp>, name: &'static str, view: View, frames: usize) -> Row {
    let before = harness.state().canvas.stats.regen.build_ms.to_bits() ^ harness.state().canvas.stats.regen.prims as u64;
    assert!(harness.state_mut().session.state_mut().unwrap().set_view(view), "{name}: view refused");
    // A view past a zoom band rebuilds the list (in the background), then draws it; a view the
    // list already serves (the first: the drawing opens zoomed to its extents) doesn't.
    let start = std::time::Instant::now();
    let mut idle = 0;
    loop {
        harness.step();
        let _ = harness.render();
        let s = harness.state();
        let now = s.canvas.stats.regen.build_ms.to_bits() ^ s.canvas.stats.regen.prims as u64;
        idle = if s.canvas.building() { 0 } else { idle + 1 };
        if s.canvas.stats.regen.prims > 0 && idle > 0 && (now != before || idle > 3) {
            break;
        }
        assert!(start.elapsed().as_secs() < 1200, "{name}: no regeneration");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // One more frame draws the new list (and uploads it).
    harness.step();
    let _ = harness.render();
    let (mut frame, mut render, mut lit) = (Vec::new(), Vec::new(), 0.0);
    for _ in 0..frames {
        let t = std::time::Instant::now();
        harness.step();
        frame.push(ms(t));
        let t = std::time::Instant::now();
        let img = harness.render();
        render.push(ms(t));
        if let Ok(img) = &img {
            lit = coverage(harness, img);
        }
    }
    harness.step();
    let report = cadaddy_ui_egui::debug_overlay::draw2d_json(harness.state());
    Row { view: name, report, frame_ms: median(frame), render_ms: median(render), coverage: lit }
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn print(rows: &[Row], sheets: usize, objects: usize) {
    let Some(first) = rows.first() else { return };
    println!("\n2D draw time, synthetic print set: {sheets} sheets, {objects} objects, {}x{} window", SIZE.0, SIZE.1);
    println!("adapter: {} ({})\n", first.report["adapter"].as_str().unwrap_or("?"), first.report["renderer"].as_str().unwrap_or("?"));
    let head = [
        "view",
        "objects considered / drew",
        "regen ms (wall)",
        "build cpu ms: curves / text / hatch / dims / blocks / other",
        "prims",
        "mesh ms",
        "upload ms",
        "GPU vertices",
        "prims sent / in view",
        "cull ms",
        "canvas ms",
        "app frame ms",
        "render ms",
        "GPU MB",
        "RSS MB",
        "sub-pixel objects + prims / dots / boxes",
        "canvas lit %",
    ];
    println!("| {} |", head.join(" | "));
    println!("|{}", "---|".repeat(head.len()));
    for r in rows {
        let (g, fr, mem) = (&r.report["regen"], &r.report["frame"], &r.report["memory"]);
        let part = |k: &str| format!("{:.0}", f(&g["parts"][k]["ms"]));
        println!(
            "| {} | {} / {} | {:.0} | {} / {} / {} / {} / {} / {} | {} | {:.0} | {:.0} | {} | {} / {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.0} | {} | {} + {} / {} / {} | {:.2} |",
            r.view,
            g["considered"],
            g["drawn"],
            f(&g["buildMs"]),
            part("curves"),
            part("text"),
            part("hatch"),
            part("dimensions"),
            part("blocks"),
            part("other"),
            g["prims"],
            f(&g["meshMs"]),
            f(&g["uploadMs"]).max(f(&fr["uploadMs"])),
            fr["gpuVertices"],
            fr["primsSent"],
            fr["primsInView"],
            f(&fr["cullMs"]),
            f(&fr["canvasMs"]),
            r.frame_ms,
            r.render_ms,
            f(&mem["gpuBytes"]) / 1e6,
            mem["rssMb"],
            g["simplified"]["dotObjects"],
            g["simplified"]["dotPrims"],
            g["simplified"]["dots"],
            g["simplified"]["boxes"],
            r.coverage * 100.0,
        );
    }
    println!(
        "\nregen = the display list built for the view (on the worker); build cpu = summed over threads; mesh = vertex data; upload = GPU buffers;"
    );
    println!(
        "cull = finding the primitives in view (the CPU canvas does it each frame, the GPU canvas sends everything); render = egui tessellation + GPU draw + read back."
    );
}

#[test]
fn a_print_set_reports_where_2d_time_goes() {
    let sheets: usize = std::env::var("DRAW2D_SHEETS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    let frames: usize = std::env::var("DRAW2D_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let set = cadaddy_engine::sample::print_set(sheets);
    let objects = set.model.len();
    let ext = set.extents(&cadaddy_engine::doc::Space::Model);
    // Roughly the objects asked for, in a mix.
    let per = cadaddy_engine::sample::PRINT_SET_OBJECTS_PER_SHEET as f64;
    assert!((objects as f64 - per * sheets as f64).abs() < per * sheets as f64 * 0.1, "{objects} objects for {sheets} sheets");

    let mut app = CadApp::new(Session::new(), Services::default());
    app.session.open_drawing(set, "Print Set", None);
    let rs = render_state();
    if let Some(rs) = &rs {
        app.set_wgpu(rs, None);
    }
    app.canvas.stats.set_on(true);
    let builder = Harness::builder().with_size(egui::vec2(SIZE.0, SIZE.1));
    let builder = match &rs {
        Some(rs) => builder.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone())),
        None => builder,
    };
    let mut harness = builder.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(3);

    // The first sheet, and the room in its lower left corner (sheet 1728 x 1152; rooms 108 x 164).
    let sheet = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(1728.0, 1152.0));
    let room = Bounds2::new(Vec2::new(72.0, 96.0), Vec2::new(180.4, 260.0));
    let view = |b: Bounds2, margin: f64| View { center: b.center(), height: b.height().max(b.width() * f64::from(SIZE.1 / SIZE.0)) * margin };
    let mut rows = vec![
        measure(&mut harness, "zoomed out (whole set)", view(ext, 1.05), frames),
        measure(&mut harness, "one sheet", view(sheet, 1.05), frames),
        measure(&mut harness, "one room", view(room, 1.1), frames),
    ];
    // The whole set again with every object drawn as it is (what the GPU canvas drew before
    // sub-pixel objects were simplified).
    harness.state_mut().canvas.full_detail = true;
    rows.push(measure(&mut harness, "zoomed out, full detail", view(ext, 1.05), frames));
    harness.state_mut().canvas.full_detail = false;
    print(&rows, sheets, objects);
    // What counting costs: the whole set's list at the zoomed-out detail, without and with it.
    let doc = harness.state().session.state().unwrap().doc.clone();
    let px = ext.height() / f64::from(SIZE.1);
    let build = |counted: bool| {
        let opts = cadaddy_render::Options {
            tolerance: px * 0.5,
            min_dash: px * 2.0,
            profile: counted.then(cadaddy_render::profile::BuildProfile::new),
            ..cadaddy_render::Options::default()
        };
        median(
            (0..3)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let _ = cadaddy_render::build(&doc, &cadaddy_engine::doc::Space::Model, &opts);
                    ms(t)
                })
                .collect(),
        )
    };
    println!("whole-set build, not counted / counted: {:.0} / {:.0} ms", build(false), build(true));

    for r in &rows {
        let (g, fr) = (&r.report["regen"], &r.report["frame"]);
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        assert_eq!(n(&g["objects"]), objects as u64, "{}: {}", r.view, r.report);
        // The build looks at objects and draws some; every object drawn is of a kind.
        assert!(n(&g["considered"]) > 0 && n(&g["drawn"]) <= n(&g["considered"]), "{}: {}", r.view, r.report);
        let kinds: u64 = ["curves", "text", "hatch", "dimensions", "blocks", "other"].iter().map(|k| n(&g["parts"][k]["objects"])).sum();
        assert!(kinds >= n(&g["drawn"]), "{}: {}", r.view, r.report);
        for k in ["curves", "text", "hatch", "blocks"] {
            assert!(n(&g["parts"][k]["objects"]) > 0, "{}: no {k} drawn: {}", r.view, r.report);
        }
        assert!(n(&fr["primsInView"]) <= n(&g["prims"]) && n(&fr["primsSent"]) > 0, "{}: {}", r.view, r.report);
        assert!(r.report["adapter"].as_str().is_some_and(|a| !a.is_empty()));
    }
    // Zoomed out, objects under a pixel are dots and small ones boxes: far fewer GPU vertices
    // than drawing every object as it is, and the drawing lights as much of the canvas.
    let (lod, full) = (&rows[0], &rows[3]);
    let verts = |r: &Row| r.report["frame"]["gpuVertices"].as_u64().unwrap_or(0);
    if rs.is_some() {
        println!(
            "zoomed out: GPU vertices {} simplified / {} full detail; canvas lit {:.2}% / {:.2}%",
            verts(lod),
            verts(full),
            lod.coverage * 100.0,
            full.coverage * 100.0
        );
        assert!(verts(lod) * 10 < verts(full) * 9, "zoomed out, sub-pixel objects are still drawn in full: {} vs {}", lod.report, full.report);
        assert!(lod.report["regen"]["simplified"]["dots"].as_u64().unwrap_or(0) > 0, "{}", lod.report);
        assert!(
            full.coverage > 0.0 && lod.coverage > full.coverage * 0.9,
            "zoomed out, the drawing thins out: {} vs {}",
            lod.coverage,
            full.coverage
        );
        assert!(lod.coverage < full.coverage * 1.25 + 0.01, "zoomed out, the drawing thickens: {} vs {}", lod.coverage, full.coverage);
    }
    // Zoomed in to a room, most of the list is off screen.
    let room_row = &rows[2].report;
    assert!(room_row["frame"]["primsInView"].as_u64().unwrap_or(0) * 4 < room_row["regen"]["prims"].as_u64().unwrap_or(0), "{room_row}");
}
