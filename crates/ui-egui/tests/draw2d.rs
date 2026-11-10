//! Where the 2D canvas spends its time on a print set (`canvas::drawstats`, the `ui.draw2d`
//! control method and the `--debug` overlay): the synthetic print set of
//! `cadaddy_engine::sample::print_set`, opened in the app with the GPU canvas (any wgpu adapter,
//! a software one included; without one the CPU canvas), drawn zoomed out to the whole set, to
//! one sheet and to one room. Each view reports its regeneration (objects considered and drawn,
//! build time by kind of object, vertex data and upload) and its frames (primitives sent and in
//! view, the canvas's and the app's time, and the headless render: egui's tessellation, the GPU
//! draw and reading the image back), with the memory and the adapter.
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

/// A headless GPU, or `None` (no adapter: the CPU canvas is measured). `DRAW2D_GPU=integrated`
/// (or `discrete`, or part of an adapter's name; `cpu` for the CPU canvas) picks the adapter,
/// `DRAW2D_BACKEND` (vulkan, dx12, metal, gl) its backend: a driver may work on one, not another.
fn render_state() -> Option<egui_wgpu::RenderState> {
    use egui_wgpu::wgpu;
    let want = std::env::var("DRAW2D_GPU").unwrap_or_default().to_ascii_lowercase();
    if want == "cpu" || want == "none" {
        return None;
    }
    let backend = std::env::var("DRAW2D_BACKEND").unwrap_or_default().to_ascii_lowercase();
    if want.is_empty() && backend.is_empty() {
        return std::panic::catch_unwind(|| {
            egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::PREDICTABLE)
        })
        .ok();
    }
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends.remove(wgpu::Backends::BROWSER_WEBGPU);
    match backend.as_str() {
        "vulkan" => setup.instance_descriptor.backends = wgpu::Backends::VULKAN,
        "dx12" => setup.instance_descriptor.backends = wgpu::Backends::DX12,
        "metal" => setup.instance_descriptor.backends = wgpu::Backends::METAL,
        "gl" => setup.instance_descriptor.backends = wgpu::Backends::GL,
        _ => {}
    }
    setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters: &[wgpu::Adapter], _| {
        let pick = |a: &&wgpu::Adapter| {
            let i = a.get_info();
            match want.as_str() {
                "" => true,
                "integrated" => i.device_type == wgpu::DeviceType::IntegratedGpu,
                "discrete" => i.device_type == wgpu::DeviceType::DiscreteGpu,
                name => i.name.to_ascii_lowercase().contains(name),
            }
        };
        adapters.iter().find(pick).cloned().ok_or_else(|| "no such adapter".to_string())
    }));
    // A measurement run by hand: no such adapter fails it.
    Some(egui_kittest::wgpu::create_render_state(egui_wgpu::WgpuSetup::CreateNew(setup), egui_wgpu::RendererOptions::PREDICTABLE))
}

/// One view's numbers.
struct Row {
    view: &'static str,
    report: Value,
    /// Medians over the measured frames: the app's update, the headless render.
    frame_ms: f64,
    render_ms: f64,
    /// The same while the view pans a few pixels each frame (inside the list's band and area:
    /// nothing is rebuilt, but every frame is drawn again; a still view shows its kept frame).
    pan_frame_ms: f64,
    pan_render_ms: f64,
    /// GPU buffer pieces the view's upload made and used again ([`cadaddy_ui_egui::gpu::upload_pieces`]).
    pieces: (u64, u64),
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
    let frames = frames.max(1);
    let before = harness.state().canvas.stats.regen.build_ms.to_bits() ^ harness.state().canvas.stats.regen.prims as u64;
    let pieces = cadaddy_ui_egui::gpu::upload_pieces();
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
    let (mut frame, mut render) = (Vec::new(), Vec::new());
    for _ in 0..frames {
        let t = std::time::Instant::now();
        harness.step();
        frame.push(ms(t));
        let t = std::time::Instant::now();
        let _ = harness.render();
        render.push(ms(t));
    }
    harness.step();
    let report = cadaddy_ui_egui::debug_overlay::draw2d_json(harness.state());
    let after = cadaddy_ui_egui::gpu::upload_pieces();
    let pieces = (after.0 - pieces.0, after.1 - pieces.1);
    // Pan right three points a frame.
    let (mut pan_frame, mut pan_render) = (Vec::new(), Vec::new());
    let step = view.height / f64::from(SIZE.1) * 3.0;
    for i in 1..=frames {
        let v = View { center: Vec2::new(view.center.x + step * i as f64, view.center.y), height: view.height };
        assert!(harness.state_mut().session.state_mut().unwrap().set_view(v));
        let t = std::time::Instant::now();
        harness.step();
        pan_frame.push(ms(t));
        let t = std::time::Instant::now();
        let _ = harness.render();
        pan_render.push(ms(t));
    }
    assert!(!harness.state().canvas.building(), "{name}: a small pan rebuilt the list");
    assert!(harness.state_mut().session.state_mut().unwrap().set_view(view));
    harness.step();
    let _ = harness.render();
    Row {
        view: name,
        report,
        frame_ms: median(frame),
        render_ms: median(render),
        pan_frame_ms: median(pan_frame),
        pan_render_ms: median(pan_render),
        pieces,
    }
}

/// An edit at the room view: one object in view moved a little. Its regeneration (the changed
/// chunk built again, the vertex data made again on the UI thread and uploaded) and the GPU
/// buffer pieces the upload made again and reused.
struct Edit {
    report: Value,
    made: u64,
    reused: u64,
    ms: f64,
}

fn edit(harness: &mut Harness<'_, CadApp>, room: Bounds2) -> Edit {
    let doc = harness.state().session.state().unwrap().doc.clone();
    let handle = doc
        .model
        .iter()
        .find(|e| matches!(e.kind, cadaddy_engine::doc::EntityKind::Line(_)) && room.contains_box(&cadaddy_engine::doc::entity_bounds(&doc, e, 0)))
        .map(|e| e.handle)
        .expect("a line in the room");
    let before = harness.state().canvas.stats.regen.build_ms.to_bits();
    let pieces = cadaddy_ui_egui::gpu::upload_pieces();
    let t = std::time::Instant::now();
    harness.state_mut().session.execute("move", &serde_json::json!({ "handles": [handle.hex()], "from": [0, 0], "to": [1, 0] })).unwrap();
    // The edit's frame builds the list and its vertex data; the next uploads them.
    for _ in 0..3 {
        harness.step();
        let _ = harness.render();
    }
    let ms = ms(t);
    assert_ne!(harness.state().canvas.stats.regen.build_ms.to_bits(), before, "the edit rebuilt the list");
    harness.step();
    let after = cadaddy_ui_egui::gpu::upload_pieces();
    Edit { report: cadaddy_ui_egui::debug_overlay::draw2d_json(harness.state()), made: after.0 - pieces.0, reused: after.1 - pieces.1, ms }
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
        "GPU pieces made / reused",
        "GPU vertices drawn / held",
        "tiles drawn / all",
        "chunks rebuilt / kept",
        "prims sent / in view",
        "cull ms",
        "canvas ms",
        "app frame ms",
        "render ms",
        "pan: app frame / render ms",
        "GPU MB",
        "RSS MB",
    ];
    println!("| {} |", head.join(" | "));
    println!("|{}", "---|".repeat(head.len()));
    for r in rows {
        let (g, fr, mem) = (&r.report["regen"], &r.report["frame"], &r.report["memory"]);
        let part = |k: &str| format!("{:.0}", f(&g["parts"][k]["ms"]));
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        println!(
            "| {} | {} / {} | {:.0} | {} / {} / {} / {} / {} / {} | {} | {:.0} | {:.0} | {} / {} | {} / {} | {} / {} | {} / {} | {} / {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} / {:.1} | {:.0} | {} |",
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
            r.pieces.0,
            r.pieces.1,
            fr["gpuVertices"],
            n(&g["lineVertices"]) + n(&g["triVertices"]) + 6 * n(&g["quadInstances"]),
            fr["tilesDrawn"],
            g["tiles"],
            g["chunksRebuilt"],
            g["chunksKept"],
            fr["primsSent"],
            fr["primsInView"],
            f(&fr["cullMs"]),
            f(&fr["canvasMs"]),
            r.frame_ms,
            r.render_ms,
            r.pan_frame_ms,
            r.pan_render_ms,
            f(&mem["gpuBytes"]) / 1e6,
            mem["rssMb"],
        );
    }
    println!(
        "\nregen = the display list built for the view (on the worker); build cpu = summed over threads; mesh = vertex data; upload = GPU buffers;"
    );
    println!(
        "cull = finding the primitives in view (the CPU canvas does it each frame; the GPU canvas draws the area tiles in view, counted in GPU vertices drawn and prims sent); render = egui tessellation + GPU draw + read back."
    );
    println!("pan = the view moved three points each frame, so each frame is drawn again (a still view shows the frame kept from before).");
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
    let rows = vec![
        measure(&mut harness, "zoomed out (whole set)", view(ext, 1.05), frames),
        measure(&mut harness, "one sheet", view(sheet, 1.05), frames),
        measure(&mut harness, "one room", view(room, 1.1), frames),
        // One zoom band further in: the tiles out of view keep their buffers.
        measure(&mut harness, "one room, zoomed in once more", view(room, 0.5), frames),
    ];
    print(&rows, sheets, objects);
    let e = edit(&mut harness, room);
    let g = &e.report["regen"];
    println!(
        "\nedit at the room (one line moved): {:.0} ms over 3 frames; build {:.1} ms, chunks rebuilt {} / kept {}; vertex data {:.0} ms, upload {:.0} ms; GPU pieces made {} / reused {}",
        e.ms,
        f(&g["buildMs"]),
        g["chunksRebuilt"],
        g["chunksKept"],
        f(&g["meshMs"]),
        f(&g["uploadMs"]),
        e.made,
        e.reused
    );
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
    // Zoomed in to a room, most of the list is off screen.
    let room_row = &rows[2].report;
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    assert!(n(&room_row["frame"]["primsInView"]) * 4 < n(&room_row["regen"]["prims"]), "{room_row}");
    // On the GPU canvas the frame draws the area tiles around it, not the whole list (Q6472),
    // and the zoom band that came with it built again only the chunks it reaches.
    if room_row["renderer"] == "gpu" {
        let (g, fr) = (&room_row["regen"], &room_row["frame"]);
        let held = n(&g["lineVertices"]) + n(&g["triVertices"]) + 6 * n(&g["quadInstances"]);
        assert!(n(&g["tiles"]) > 1 && n(&fr["tilesDrawn"]) < n(&g["tiles"]), "{room_row}");
        assert!(n(&fr["gpuVertices"]) * 3 < held, "{room_row}");
        assert!(n(&fr["primsSent"]) * 3 < n(&g["prims"]), "{room_row}");
    }
    assert!(n(&room_row["regen"]["chunksKept"]) > 0, "{room_row}");
    // An edit uploads again only the pieces of the tiles it changed.
    if room_row["renderer"] == "gpu" {
        assert!(e.made * 4 < e.reused, "pieces made {} reused {}", e.made, e.reused);
    }
}
