//! 2D detail levels by zoom (Q6417): a print set's sheet zoomed out is drawn as its picture and
//! with its symbols as boxes; zooming in brings the linework and the symbols' contents back for
//! what is in view. Counts, plots and snaps never see less than what is drawn.
//!
//! `zoom_detail_frame_times` (ignored) measures the screen with detail levels and without; run
//! it by hand, in release, on the 100-sheet set:
//!
//! ```text
//! ZOOMDETAIL_SHEETS=100 ZOOMDETAIL_PER=10000 cargo test --release -p cadaddy-ui-egui --test suite_panels \
//!     zoom_detail_frame_times -- --ignored --nocapture
//! ```
//!
//! `ZOOMDETAIL_GPU=integrated` (or `discrete`, or part of an adapter's name) measures the GPU
//! canvas on that adapter, `ZOOMDETAIL_BACKEND` picks its backend, and `ZOOMDETAIL_MODE=full` or
//! `detail` runs one of the two (each in its own process has its own memory peak).
//! `ZOOMDETAIL_WHOLE=1` measures the same objects as an ordinary drawing seen whole,
//! `ZOOMDETAIL_FILE=<drawing>` a drawing file instead of the synthetic set, and
//! `ZOOMDETAIL_BUILD=1` only the list build at a room's zoom, with and without detail levels.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use cadaddy_engine::doc::{EntityKind, Handle, Space};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::snap::{self, mode};
use cadaddy_engine::{Session, View, sample};
use cadaddy_render::Kind;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::{Value, json};

fn harness(d: cadaddy_engine::doc::Drawing) -> Harness<'static, CadApp> {
    let mut s = Session::new();
    s.open_drawing(d, "Sample Set", None);
    let mut app = CadApp::new(s, Services::default());
    app.canvas.zoom_pending = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// Run frames until `done` (background builds and pictures land), at most about 20 s.
fn settle(h: &mut Harness<'_, CadApp>, done: impl Fn(&CadApp) -> bool) -> bool {
    for _ in 0..2000 {
        h.run_steps(1);
        if done(h.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// The symbols (block references) of the open sheet.
fn symbols(app: &CadApp) -> Vec<(Handle, Vec2)> {
    let Ok(d) = app.session.view_doc() else { return Vec::new() };
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Insert(i) if i.rotation == 0.0 => Some((e.handle, i.insert.xy())),
            _ => None,
        })
        .collect()
}

/// The primitives the canvas's list draws for `h`.
fn prims(app: &CadApp, h: Handle) -> Vec<(Kind, u32)> {
    app.canvas.list.as_ref().map(|l| l.prims.iter().filter(|p| p.handle == h).map(|p| (p.kind, p.len)).collect()).unwrap_or_default()
}

fn drawn(app: &CadApp) -> BTreeSet<Handle> {
    app.canvas.list.as_ref().map(|l| l.prims.iter().map(|p| p.handle).filter(|h| h.0 != 0).collect()).unwrap_or_default()
}

fn look_at(h: &mut Harness<'_, CadApp>, center: Vec2, height: f64) {
    if let Ok(st) = h.state_mut().session.state_mut() {
        st.set_view(View { center, height });
    }
    h.run_steps(2);
}

/// The reproduction, with only what the app had before detail levels (it fails there): zoomed
/// out on a sheet, every symbol was drawn in full, its circle and lines, as every other object.
#[test]
fn zoomed_out_a_symbol_is_drawn_as_its_box() {
    let h = harness(sample::print_set(2, 3000));
    let syms = symbols(h.state());
    assert!(syms.len() > 100, "the sheet has symbols: {}", syms.len());
    let boxes = syms.iter().filter(|(s, _)| prims(h.state(), *s) == vec![(Kind::Polyline, 5)]).count();
    assert_eq!(boxes, syms.len(), "zoomed out, a symbol is drawn as its box (one closed outline)");
}

/// Zoomed out on a sheet, its symbols are drawn as their boxes and the sheet as its picture; zoomed in on a room, a symbol's contents are drawn
/// again, and the picture gives way to the linework.
#[test]
fn zoomed_out_a_sheet_draws_less_and_zooming_in_brings_its_detail_back() {
    let mut h = harness(sample::print_set(2, 3000));
    let syms = symbols(h.state());
    assert!(syms.len() > 100, "the sheet has symbols: {}", syms.len());
    // Zoomed out (the sheet fitted): every symbol is its box, one closed outline.
    let boxes = syms.iter().filter(|(s, _)| prims(h.state(), *s) == vec![(Kind::Polyline, 5)]).count();
    assert_eq!(boxes, syms.len(), "zoomed out, a symbol is drawn as its box");
    assert!(h.state().session.screen_detail().is_some(), "the screen tells the session what it drew at");
    // And the sheet as its picture, made in the background.
    assert!(settle(&mut h, |a| a.canvas.sheet_images.shown), "zoomed out, the sheet is drawn as its picture");
    assert!(h.state().canvas.sheet_images.bytes() > 0);
    // Zoomed in on a symbol (a room's view): its contents come in, the picture goes.
    let (sym, at) = syms[syms.len() / 2];
    look_at(&mut h, at, 120.0);
    assert!(!h.state().canvas.sheet_images.shown, "zoomed in, the linework is drawn");
    assert!(
        settle(&mut h, |a| !a.canvas.building() && prims(a, sym).len() > 1),
        "zoomed in, the symbol's contents are drawn: {:?}",
        prims(h.state(), sym)
    );
    // Zoomed back out: the picture again, at once (it was kept).
    let builds = h.state().canvas.sheet_images.builds;
    let _ = h.state_mut().session.execute("zoom", &json!({"mode": "extents"}));
    h.run_steps(3);
    assert!(h.state().canvas.sheet_images.shown, "the kept picture is drawn at once");
    assert_eq!(h.state().canvas.sheet_images.builds, builds, "and not made again");
}

/// An edit makes the sheet's picture stale at once: the linework (with the edit) is drawn until
/// the new picture is ready.
#[test]
fn an_edit_is_drawn_at_once_and_the_picture_follows() {
    let mut h = harness(sample::print_set(2, 1500));
    assert!(settle(&mut h, |a| a.canvas.sheet_images.shown));
    let builds = h.state().canvas.sheet_images.builds;
    let c = h.state().session.state().map(|st| st.view().center).unwrap_or(Vec2::ZERO);
    let out = h.state_mut().session.execute("line", &json!({"points": [[c.x, c.y], [c.x + 300.0, c.y + 300.0]]}));
    assert!(out.is_ok(), "line: {out:?}");
    h.run_steps(1);
    let added = h.state().session.doc().ok().and_then(|d| d.model.iter().last().map(|e| e.handle));
    let Some(added) = added else { panic!("no line") };
    assert!(!h.state().canvas.sheet_images.shown, "a stale picture is never drawn");
    assert!(drawn(h.state()).contains(&added), "the edit is drawn at once");
    assert!(settle(&mut h, |a| a.canvas.sheet_images.shown), "the new picture comes");
    assert!(h.state().canvas.sheet_images.builds > builds);
}

/// The snaps see what is drawn: zoomed out a symbol (drawn as its box) offers its insertion point
/// only; zoomed in, its lines' endpoints too.
#[test]
fn snaps_offer_only_what_is_drawn() {
    let mut h = harness(sample::print_set(1, 1500));
    let syms = symbols(h.state());
    let (_, at) = syms[0];
    // The symbol's crossbar ends 4 units either side of its insertion point.
    let end = at + Vec2::new(4.0, 0.0);
    let snap_at = |app: &CadApp, aperture: f64| {
        let shown = app.session.view_doc().unwrap_or_else(|_| panic!("view"));
        snap::osnap_drawn(&shown, &Space::Model, end, aperture, mode::END | mode::INS, None, false, app.session.screen_detail().as_ref())
    };
    let far = snap_at(h.state(), 1.0);
    assert!(far.is_none_or(|s| s.point.dist(end) > 1e-6), "zoomed out, no endpoint inside a symbol's box: {far:?}");
    let ins = {
        let shown = h.state().session.view_doc().unwrap_or_else(|_| panic!("view"));
        snap::osnap_drawn(&shown, &Space::Model, at, 1.0, mode::INS, None, false, h.state().session.screen_detail().as_ref())
    };
    assert!(ins.is_some_and(|s| s.point.dist(at) < 1e-6), "its insertion point still snaps");
    look_at(&mut h, at, 60.0);
    assert!(settle(&mut h, |a| !a.canvas.building()));
    h.run_steps(1);
    let near = snap_at(h.state(), 1.0);
    assert!(near.is_some_and(|s| s.point.dist(end) < 1e-6), "zoomed in, the endpoint snaps: {near:?}");
    // Zoomed back out, the sheet's picture is drawn (the finer list is kept for highlights): the
    // snaps follow the picture.
    let _ = h.state_mut().session.execute("zoom", &json!({"mode": "extents"}));
    assert!(settle(&mut h, |a| a.canvas.sheet_images.shown));
    let again = snap_at(h.state(), 1.0);
    assert!(again.is_none_or(|s| s.point.dist(end) > 1e-6), "drawn as its picture, no endpoint inside a symbol's box: {again:?}");
}

/// What the commands read never depends on the zoom or on the detail levels.
#[test]
fn counts_and_takeoff_are_the_same_at_every_zoom() {
    let mut h = harness(sample::print_set(2, 1500));
    let read = |h: &mut Harness<'_, CadApp>| -> Vec<Value> {
        let s = &mut h.state_mut().session;
        let mut run = |c: &str, p: Value| s.execute(c, &p).unwrap_or_else(|e| panic!("{c}: {e}"));
        vec![
            run("entities", json!({"limit": 1}))["count"].clone(),
            run("takeoff", json!({})),
            run("takeoff", json!({"mode": "area"})),
            run("count", json!({"block": "SYM-OUTLET", "space": "model"})),
        ]
    };
    let zoomed_out = read(&mut h);
    let syms = symbols(h.state());
    look_at(&mut h, syms[0].1, 60.0);
    assert!(settle(&mut h, |a| !a.canvas.building()));
    assert_eq!(read(&mut h), zoomed_out, "zoomed in");
    h.state_mut().canvas.full_detail = true;
    h.run_steps(3);
    assert!(h.state().session.screen_detail().is_none());
    assert_eq!(read(&mut h), zoomed_out, "without detail levels");
}

/// A drawing at an ordinary zoom looks as it did: the sample part drawing at its extents has
/// nothing too small to show, so its list is the one drawn in full (the screenshots' scenes).
#[test]
fn an_ordinary_view_draws_as_before() {
    let mut h = harness(sample::default_sample());
    let list = |a: &CadApp| a.canvas.list.as_ref().map(|l| (l.prims.clone(), l.verts.clone(), l.tris.clone()));
    let with = list(h.state());
    assert!(h.state().canvas.list_detail.is_some());
    h.state_mut().canvas.full_detail = true;
    h.run_steps(3);
    assert!(h.state().canvas.list_detail.is_none());
    assert!(with.is_some() && with == list(h.state()), "the same list with detail levels as without");
}

/// An ordinary drawing (no sheets) gets the detail levels but no picture.
#[test]
fn an_ordinary_drawing_has_no_picture() {
    let mut d = sample::print_set(1, 1500);
    d.agent = None.into();
    let mut h = harness(d);
    h.run_steps(20);
    assert!(!h.state().canvas.sheet_images.shown && !h.state().canvas.sheet_images.making());
    assert!(h.state().canvas.list_detail.is_some());
    // Turned off, everything is drawn in full: the symbols' contents.
    h.state_mut().canvas.full_detail = true;
    h.run_steps(3);
    let syms = symbols(h.state());
    assert!(syms.iter().all(|(s, _)| prims(h.state(), *s).len() > 1));
}

/// The process's memory now and at its peak, MB (Windows: the working set; Linux: VmRSS/VmHWM).
fn memory_mb() -> (f64, f64) {
    #[cfg(target_os = "windows")]
    {
        let pid = std::process::id();
        let out = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("$p = Get-Process -Id {pid}; \"$($p.WorkingSet64) $($p.PeakWorkingSet64)\""),
            ])
            .output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        let v: Vec<f64> = text.split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect();
        (v.first().copied().unwrap_or(0.0) / 1e6, v.get(1).copied().unwrap_or(0.0) / 1e6)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let kb = |key: &str| {
            status.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok()).unwrap_or(0.0)
        };
        (kb("VmRSS:") / 1e3, kb("VmHWM:") / 1e3)
    }
}

/// A headless GPU on the adapter `want` names (`integrated`, `discrete`, or a part of its name),
/// else `None` (the CPU canvas).
fn gpu(want: &str) -> Option<egui_wgpu::RenderState> {
    use egui_wgpu::wgpu;
    let want = want.to_ascii_lowercase();
    if want.is_empty() || want == "none" || want == "cpu" {
        return None;
    }
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends.remove(wgpu::Backends::BROWSER_WEBGPU);
    // ZOOMDETAIL_BACKEND: vulkan, dx12, metal or gl only (a driver may work on one, not another).
    match std::env::var("ZOOMDETAIL_BACKEND").unwrap_or_default().to_ascii_lowercase().as_str() {
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
struct Seen {
    prims: usize,
    verts: usize,
    /// The list's GPU vertex data (what the GPU canvas holds for it), and the vertices a frame
    /// draws (a picture: one quad).
    gpu_bytes: usize,
    gpu_drawn: usize,
    build_ms: f64,
    ready_ms: f64,
    frame_ms: f64,
    picture: bool,
}

/// What the screen does with `d` open: zoomed out on its first sheet, at one room, and zoomed
/// out again; on the GPU canvas when `rs` is given (each frame drawn and read back), else on the
/// CPU canvas.
fn measure(label: &str, d: cadaddy_engine::doc::Drawing, full: bool, rs: Option<&egui_wgpu::RenderState>) {
    let mut s = Session::new();
    s.open_drawing(d, "Sample Set", None);
    let mut app = CadApp::new(s, Services::default());
    app.canvas.zoom_pending = true;
    app.canvas.full_detail = full;
    if let Some(rs) = rs {
        app.set_wgpu(rs, None);
    }
    let builder = Harness::builder().with_size(vec2(1600.0, 1000.0));
    let builder = match rs {
        Some(rs) => builder.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone())),
        None => builder,
    };
    let mut h = builder.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    let gpu_on = rs.is_some();
    let frame = |h: &mut Harness<'_, CadApp>, pos: Option<egui::Pos2>| {
        if let Some(p) = pos {
            h.event(egui::Event::PointerMoved(p));
        }
        h.step();
        if gpu_on {
            let _ = h.render();
        }
    };
    let settle = |h: &mut Harness<'_, CadApp>, done: &dyn Fn(&CadApp) -> bool| {
        let t0 = Instant::now();
        for _ in 0..6000 {
            frame(h, None);
            if done(h.state()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        t0.elapsed().as_secs_f64() * 1000.0
    };
    let seen = |h: &mut Harness<'_, CadApp>, ready_ms: f64, x: &mut f32| {
        let mut ms: Vec<f64> = Vec::new();
        for _ in 0..9 {
            *x += 7.0;
            let t = Instant::now();
            frame(h, Some(egui::pos2(*x, 500.0)));
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(f64::total_cmp);
        let a = h.state();
        let picture = a.canvas.sheet_images.shown;
        let (prims, verts, gpu_bytes, gpu_drawn) = a.canvas.list.as_ref().map_or((0, 0, 0, 0), |l| {
            let m = cadaddy_ui_egui::gpu::build_mesh(l, Vec2::ZERO, cadaddy_color::Rgb(0, 0, 0), false);
            let drawn = if picture { 4 } else { m.line_vertices() + m.tri_vertices() + m.quad_instances() * 4 };
            (l.prims.len(), l.verts.len() + l.tris.len(), m.lines.len() + m.tris.len() + m.quads.len(), drawn)
        });
        Seen {
            prims,
            verts,
            gpu_bytes,
            gpu_drawn,
            build_ms: a.canvas.build_ms,
            ready_ms,
            frame_ms: ms.get(ms.len() / 2).copied().unwrap_or(0.0),
            picture,
        }
    };
    let t0 = Instant::now();
    frame(&mut h, None);
    frame(&mut h, None);
    let open_ms = t0.elapsed().as_secs_f64() * 1000.0;
    // Zoomed out: the sheet fitted; the sheet list's thumbnails and the picture finish.
    let sheets = !h.state().session.print_sheets().is_empty();
    let picture_ms = if full || !sheets { settle(&mut h, &|a| !a.canvas.building()) } else { settle(&mut h, &|a| a.canvas.sheet_images.shown) };
    for _ in 0..12 {
        frame(&mut h, None);
    }
    let mut x = 400.0f32;
    let out = seen(&mut h, picture_ms, &mut x);
    // A room: a symbol's neighbourhood, 20 ft high.
    let at = symbols(h.state()).first().map(|s| s.1).unwrap_or(Vec2::ZERO);
    if let Ok(st) = h.state_mut().session.state_mut() {
        st.set_view(View { center: at, height: 240.0 });
    }
    let room_ms = settle(&mut h, &|a| !a.canvas.building());
    let room = seen(&mut h, room_ms, &mut x);
    // Zoomed out again (ZOOM Extents fits the sheet): the frame that shows it.
    let _ = h.state_mut().session.execute("zoom", &json!({"mode": "extents"}));
    let t = Instant::now();
    frame(&mut h, None);
    let back_ms = t.elapsed().as_secs_f64() * 1000.0;
    let back_ready = settle(&mut h, &|a| !a.canvas.building());
    let back = seen(&mut h, back_ready, &mut x);
    let pictures = h.state().canvas.sheet_images.bytes();
    let (rss, peak) = memory_mb();
    let row = |view: &str, v: &Seen| {
        println!(
            "| {label} | {view} | {} | {} | {:.1} | {} | {:.0} | {:.0} | {:.1} | {} |",
            v.prims,
            v.verts,
            v.gpu_bytes as f64 / 1e6,
            v.gpu_drawn,
            v.build_ms,
            v.ready_ms,
            v.frame_ms,
            if v.picture { "picture" } else { "linework" }
        );
    };
    let whole = h.state().session.print_sheets().is_empty();
    row(if whole { "whole drawing (extents)" } else { "sheet (zoomed out)" }, &out);
    row("one room", &room);
    row(if whole { "extents again" } else { "sheet again" }, &back);
    println!(
        "| {label} | totals | opening {open_ms:.0} ms | first frame back out {back_ms:.1} ms | pictures {:.1} MB | memory now {rss:.0} MB | peak {peak:.0} MB | | | |",
        pictures as f64 / 1e6
    );
}

#[test]
#[ignore = "a measurement: run by hand in release (see the module docs)"]
fn zoom_detail_frame_times() {
    let n = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (sheets, per) = (n("ZOOMDETAIL_SHEETS", 10), n("ZOOMDETAIL_PER", 2000));
    // ZOOMDETAIL_GPU: integrated, discrete, a part of an adapter's name, or none (CPU canvas).
    let want = std::env::var("ZOOMDETAIL_GPU").unwrap_or_default();
    let rs = gpu(&want);
    let adapter = rs.as_ref().map_or("CPU canvas".to_string(), |r| format!("{} ({:?})", r.adapter.get_info().name, r.adapter.get_info().backend));
    // ZOOMDETAIL_MODE: full (as before), detail (with detail levels) or both. One per process
    // gives each its own memory peak.
    let mode = std::env::var("ZOOMDETAIL_MODE").unwrap_or_else(|_| "both".into());
    println!("\n{sheets} sheets x {per} objects, 1600x1000, {adapter}");
    println!("| run | view | prims | verts | list GPU MB | vertices drawn | build ms | ready ms | frame ms | drawn as |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    // ZOOMDETAIL_WHOLE=1: the same objects as an ordinary drawing (no sheet records), seen whole.
    let whole = std::env::var("ZOOMDETAIL_WHOLE").is_ok_and(|v| v == "1");
    // ZOOMDETAIL_FILE: a drawing file to measure instead of the synthetic set.
    let file = std::env::var("ZOOMDETAIL_FILE").ok().filter(|f| !f.is_empty());
    let set = || {
        let mut d = match &file {
            Some(f) => {
                let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("{f}: {e}"));
                cadaddy_io::read(&bytes, f).unwrap_or_else(|e| panic!("{f}: {e}"))
            }
            None => sample::print_set(sheets, per),
        };
        if whole {
            d.agent = None.into();
        }
        d
    };
    // ZOOMDETAIL_BUILD=1: only the list build itself, the whole drawing at a room's zoom, with
    // and without detail levels (nothing of the app in between).
    if std::env::var("ZOOMDETAIL_BUILD").is_ok_and(|v| v == "1") {
        let d = set();
        let px = 0.125;
        // The area a room's view builds for, as the canvas does (the view and half its side round).
        let at = d.model.iter().find_map(|e| match &e.kind {
            EntityKind::Insert(i) => Some(i.insert.xy()),
            _ => None,
        });
        let view = at.map(|c| {
            let half = Vec2::new(240.0 * 1.6 / 2.0, 240.0 / 2.0);
            let b = cadaddy_engine::geom::Bounds2::new(c - half, c + half);
            b.expand(b.width().max(b.height()) * 0.5)
        });
        let base = cadaddy_render::Options { tolerance: px * 0.5, min_dash: px * 2.0, view, ..Default::default() };
        let rules = std::sync::Arc::new(cadaddy_engine::cmd::fidelity::plan_rules(&d));
        let with = || cadaddy_render::Options { detail: cadaddy_render::detail::Detail::new(px, rules.clone()), ..base.clone() };
        let time = |o: &dyn Fn() -> cadaddy_render::Options| {
            let mut t: Vec<f64> = (0..5)
                .map(|_| {
                    let o = o();
                    let t0 = Instant::now();
                    let l = cadaddy_render::build(&d, &Space::Model, &o);
                    std::hint::black_box(l.prims.len());
                    t0.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            t.sort_by(f64::total_cmp);
            t
        };
        // ZOOMDETAIL_KINDS=1: each kind of object on its own.
        if std::env::var("ZOOMDETAIL_KINDS").is_ok_and(|v| v == "1") {
            let mut kinds: std::collections::BTreeMap<String, Vec<std::sync::Arc<cadaddy_engine::doc::Entity>>> = Default::default();
            for e in d.model.iter() {
                let k = format!("{:?}", e.kind);
                let k = k.split(['(', ' ', '{']).next().unwrap_or("?").to_string();
                kinds.entry(k).or_default().push(e.clone());
            }
            for (k, ents) in kinds {
                let mut sub = d.clone();
                sub.model = cadaddy_engine::doc::EntityStore::new();
                for e in ents {
                    sub.model.push_shared(e);
                }
                let best = |o: &cadaddy_render::Options| {
                    (0..5)
                        .map(|_| {
                            let t0 = Instant::now();
                            std::hint::black_box(cadaddy_render::build(&sub, &Space::Model, o).prims.len());
                            t0.elapsed().as_secs_f64() * 1000.0
                        })
                        .fold(f64::MAX, f64::min)
                };
                println!("{k:<12} {:>8} objects: without {:>7.1} ms, with {:>7.1} ms", sub.model.len(), best(&base), best(&with()));
            }
            return;
        }
        println!("build at a room's zoom, ms (5 runs, sorted): without {:?}", time(&|| base.clone()));
        println!("build at a room's zoom, ms (5 runs, sorted): with    {:?}", time(&with));
        println!("build at a room's zoom, ms (5 runs, sorted): without {:?}", time(&|| base.clone()));
        return;
    }
    // Each made on its own: nothing carries from one to the other.
    if mode != "detail" {
        measure("before: in full", set(), true, rs.as_ref());
    }
    if mode != "full" {
        measure("after: detail levels", set(), false, rs.as_ref());
    }
}
