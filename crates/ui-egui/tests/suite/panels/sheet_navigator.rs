//! A print set one sheet at a time (Q6416): the app opens a print set on its first sheet (its
//! sheet list waits on the rail, Q6524), draws only that sheet's objects, and steps through the
//! sheets with the list and with Page Up / Page Down. An ordinary drawing is drawn whole, as
//! before.
//!
//! `sheet_navigator_frame_times` (ignored) measures what the screen does with a whole set before
//! and after; run it by hand, in release, on the 100-sheet set:
//!
//! ```text
//! SHEETNAV_SHEETS=100 SHEETNAV_PER=10000 cargo test --release -p cadaddy-ui-egui --test suite_panels \
//!     sheet_navigator_frame_times -- --ignored --nocapture
//! ```

use std::collections::BTreeSet;
use std::time::Instant;

use cadaddy_engine::doc::{Handle, entity_bounds};
use cadaddy_engine::geom::{Bounds2, Vec2};
use cadaddy_engine::{Session, sample};
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;

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

/// The objects whose bounds touch `rect`.
fn on(app: &CadApp, rect: &Bounds2) -> BTreeSet<Handle> {
    let d = app.session.doc().unwrap();
    d.model.iter().filter(|e| entity_bounds(d, e, 0).intersects(rect)).map(|e| e.handle).collect()
}

/// The objects the canvas's display list draws.
fn drawn(app: &CadApp) -> BTreeSet<Handle> {
    app.canvas.list.as_ref().map(|l| l.prims.iter().map(|p| p.handle).filter(|h| h.0 != 0).collect()).unwrap_or_default()
}

fn key(h: &mut Harness<'_, CadApp>, k: Key) {
    h.event(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Where the job's records put sheet `i` (read from them, as a reader of the file would).
fn region(d: &cadaddy_engine::doc::Drawing, i: usize) -> Bounds2 {
    let v: serde_json::Value = serde_json::from_str(d.agent.as_deref().unwrap()).unwrap();
    let r: Vec<f64> = v["guided_job"]["sheet_regions"][i]["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    Bounds2::new(Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3]))
}

/// The test that failed before the sheet navigator: opening a print set drew every sheet, and
/// Page Down did nothing. Only what the app had then is used.
#[test]
fn opening_a_print_set_draws_its_first_sheet_and_page_down_the_next() {
    let set = sample::job_print_set(3, 120);
    let (first, second) = (region(&set, 0), region(&set, 1));
    let mut h = harness(set);
    assert_eq!(drawn(h.state()), on(h.state(), &first), "only the first sheet's objects are drawn");
    key(&mut h, Key::PageDown);
    assert_eq!(drawn(h.state()), on(h.state(), &second), "Page Down: only the second sheet's");
    assert!(second.contains(h.state().session.state().unwrap().view().center));
}

#[test]
fn a_print_set_opens_on_its_first_sheet_and_draws_only_it() {
    let mut h = harness(sample::job_print_set(3, 120));
    let sheets = h.state().session.print_sheets();
    assert_eq!(sheets.len(), 3);
    assert_eq!(h.state().session.active_sheet().map(|(i, _)| i), Some(0), "opened on its first sheet");
    let first = on(h.state(), &sheets[0].rect);
    assert_eq!(drawn(h.state()), first, "only the first sheet's objects are drawn");
    // The view is the sheet, not the set.
    let v = h.state().session.state().unwrap().view();
    assert!(sheets[0].rect.contains(v.center) && v.height < sheets[0].rect.height() * 2.0, "{v:?}");
    // Its sheet list waits on the rail: opening a file opens no palette by itself (Q6524). The
    // sheet is fitted to the drawing area.
    assert!(!h.state().ui.docking.layout.contains(&cadaddy_ui_egui::docking::Panel::SheetSets));
    let canvas = h.state().canvas.rect.unwrap();
    let xf = h.state().canvas.xf.unwrap();
    let (lo, hi) = (xf.to_screen(sheets[0].rect.min), xf.to_screen(sheets[0].rect.max));
    assert!(canvas.contains(lo) && canvas.contains(hi), "the whole sheet is in view: {lo:?} {hi:?} in {canvas:?}");
    // From the rail: a row per sheet, with its number and title, docked on the left of the
    // drawing a column wide.
    h.state_mut().run("ui.dock", json!({"operation": "toggle", "panel": "sheetSets"})).unwrap();
    h.run_steps(4);
    let canvas = h.state().canvas.rect.unwrap();
    assert!(canvas.left() > 250.0 && canvas.left() < 400.0, "{canvas:?}");
    h.get_by_label("Sample Sheet 2");
    // A look at it, when asked for (`CADADDY_SHEETNAV_SHOTS=<folder>`).
    if let Some(dir) = std::env::var_os("CADADDY_SHEETNAV_SHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.run_steps(8);
        h.render().unwrap().save(dir.join("sheet-navigator.png")).unwrap();
    }
    // A click on a row opens that sheet.
    h.get_by_label("Q-002").click();
    h.run_steps(4);
    assert_eq!(h.state().session.active_sheet().map(|(i, _)| i), Some(1));
    assert_eq!(drawn(h.state()), on(h.state(), &sheets[1].rect));
}

#[test]
fn page_down_and_page_up_step_through_the_sheets() {
    let mut h = harness(sample::job_print_set(3, 60));
    let sheets = h.state().session.print_sheets();
    key(&mut h, Key::PageDown);
    assert_eq!(h.state().session.active_sheet().map(|(i, _)| i), Some(1));
    key(&mut h, Key::PageDown);
    key(&mut h, Key::PageDown);
    assert_eq!(h.state().session.active_sheet().map(|(i, _)| i), Some(2), "the last sheet is the last");
    assert_eq!(drawn(h.state()), on(h.state(), &sheets[2].rect));
    key(&mut h, Key::PageUp);
    assert_eq!(h.state().session.active_sheet().map(|(i, _)| i), Some(1));
    // A thumbnail per sheet in view, made a frame at a time.
    h.run_steps(6);
    assert!(h.state().session.sheet_thumbnail_rgba(0, 96, 64).is_some_and(|px| px.chunks(4).any(|p| p != px.get(..4).unwrap_or(&[]))));
}

#[test]
fn an_ordinary_drawing_is_drawn_whole() {
    let mut d = sample::job_print_set(3, 60);
    // The same objects without a job's records: one drawing, no sheets.
    d.agent = None.into();
    let mut h = harness(d);
    assert!(h.state().session.print_sheets().is_empty());
    assert!(h.state().session.active_sheet().is_none());
    let all: BTreeSet<Handle> = h.state().session.doc().unwrap().model.iter().map(|e| e.handle).collect();
    assert_eq!(drawn(h.state()), all);
    assert!(!h.state().ui.docking.layout.contains(&cadaddy_ui_egui::docking::Panel::SheetSets));
    // Page Down does nothing to it.
    let before = h.state().session.state().unwrap().view();
    key(&mut h, Key::PageDown);
    assert_eq!(h.state().session.state().unwrap().view(), before);
}

/// The median of `f`'s times over `n` runs, ms.
fn median_ms(n: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..n)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t.get(n / 2).copied().unwrap_or(0.0)
}

/// What the screen does with `d` open: objects drawn, display list and CPU frame times. The
/// whole drawing stays in memory (as before sheets loaded on demand, Q6418).
fn measure(label: &str, d: cadaddy_engine::doc::Drawing) {
    let mut s = Session::new();
    s.open_drawing(d, "Sample Set", None);
    let mut app = CadApp::new(s, Services::default());
    app.session.sheets_on_demand = false;
    measure_app(label, app);
}

/// [`measure`] of the .cdy file at `path` opened as the interface opens it: sheets on demand.
fn measure_file(label: &str, path: &str) {
    let mut s = Session::new();
    s.sheets_on_demand = true;
    let t = Instant::now();
    s.finish_open(cadaddy_engine::cmd::file::read_file_on_demand(path).unwrap()).unwrap();
    println!("{label}: read on demand in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    measure_app(label, CadApp::new(s, Services::default()));
}

fn measure_app(label: &str, mut app: CadApp) {
    app.canvas.zoom_pending = true;
    let ctx = egui::Context::default();
    let size = vec2(1600.0, 1000.0);
    let input = |pos: Option<egui::Pos2>| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        events: pos.map(|p| vec![Event::PointerMoved(p)]).unwrap_or_default(),
        ..Default::default()
    };
    let frame = |app: &mut CadApp, pos: Option<egui::Pos2>| {
        let mut out = ctx.run_ui(input(pos), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
    };
    let t0 = Instant::now();
    frame(&mut app, None);
    frame(&mut app, None);
    let open_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let list = app.canvas.list.as_ref();
    let (prims, verts) = list.map_or((0, 0), |l| (l.prims.len(), l.verts.len() + l.tris.len()));
    let objects = app.session.view_doc().map_or(0, |d| d.model.len());
    let total = app.session.doc().map_or(0, |d| d.model.len());
    // The sheet list's thumbnails in view are made a frame at a time: let them finish.
    for _ in 0..12 {
        frame(&mut app, None);
    }
    // Steady frames with the cursor moving over the drawing (hover picks each frame).
    let mut x = 400.0f32;
    let steady = median_ms(9, || {
        x += 7.0;
        frame(&mut app, Some(egui::pos2(x, 500.0)));
    });
    // An edit: the list is built again (the command, the drawing as the screen shows it, the
    // frame).
    let at = app.session.state().map(|st| st.view().center).unwrap_or(Vec2::ZERO);
    let (mut command, mut shown) = (Vec::new(), Vec::new());
    let edit = median_ms(3, || {
        let t = Instant::now();
        let _ = app.session.execute("line", &json!({"points": [[at.x, at.y], [at.x + 10.0, at.y + 10.0]]}));
        command.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let _ = app.session.view_doc();
        shown.push(t.elapsed().as_secs_f64() * 1000.0);
        frame(&mut app, None);
    });
    // Page Down: the next sheet, drawn.
    let switch = if app.session.print_sheets().is_empty() {
        String::new()
    } else {
        let ms = median_ms(3, || {
            let _ = app.session.execute("sheet.next", &json!({}));
            frame(&mut app, None);
        });
        let total = app.session.doc().map_or(0, |d| d.model.len());
        format!("   next sheet {ms:.1} ms (then {total} objects in memory)")
    };
    let mid = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    println!(
        "{label:<32} objects drawn {objects:>8} of {total:>8}   prims {prims:>8}   vertices {verts:>9}   first frames {open_ms:>8.0} ms   steady frame {steady:>7.1} ms   edit {edit:>7.1} ms (command {:.1}, sheet view {:.1}){switch}",
        mid(&mut command),
        mid(&mut shown)
    );
}

#[test]
#[ignore = "a measurement: run by hand in release (see the module docs)"]
fn sheet_navigator_frame_times() {
    let n = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (sheets, per) = (n("SHEETNAV_SHEETS", 10), n("SHEETNAV_PER", 2000));
    // Each made on its own: nothing (a spatial index, a display list) carries from one to the other.
    let mut whole = sample::job_print_set(sheets, per);
    println!("\n{sheets} sheets x {per} objects ({} objects)", whole.model.len());
    whole.agent = None.into();
    measure("before: the whole set (no sheets)", whole);
    measure("after: one sheet open", sample::job_print_set(sheets, per));
    // Sheets on demand (Q6418): the set saved to .cdy and opened from it.
    let dir = std::env::temp_dir().join(format!("cadaddy-sheetnav-bench-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("Sample Set.cdy").to_string_lossy().into_owned();
    let mut set = sample::job_print_set(sheets, per);
    set.sheet_store.sheets = std::sync::Arc::new(cadaddy_engine::sheetstore::rects_of(&cadaddy_engine::sheetnav::print_sheets(&set)));
    std::fs::write(&path, cadaddy_io::cdy::write_cdy_checkpoint(&set).unwrap()).unwrap();
    drop(set);
    measure_file("after Q6418: sheets on demand", &path);
    let _ = std::fs::remove_dir_all(&dir);
}
