//! A print set opened from a .cdy file in the app holds the open sheet and a few recent ones, not
//! the whole set (Q6418). A program of its own: it installs the file hooks as the app does.
//!
//! The first test is the one that failed before sheets loaded on demand: it uses only what the
//! app had then (the file written as any drawing is, opened as a person opens it).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use cadaddy_engine::doc::{Handle, entity_bounds};
use cadaddy_engine::geom::{Bounds2, Vec2};
use cadaddy_engine::{Session, sample};
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// As the app installs them.
fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    set_cdy_hooks(CdyHooks {
        save: |d, prev| {
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| None)?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as CdyState),
    });
}

/// An app with nothing open, as it starts.
fn empty_app() -> Harness<'static, CadApp> {
    hooks();
    let app = CadApp::new(Session::new(), Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    h
}

/// Frames until `done` holds (a background read or load finishing), at most a minute.
fn frames_until(h: &mut Harness<'_, CadApp>, done: impl Fn(&CadApp) -> bool) {
    let t = Instant::now();
    while !done(h.state()) && t.elapsed().as_secs() < 60 {
        h.state_mut().wait_for_work(Duration::from_millis(20));
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, CadApp>, k: Key) {
    h.event(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Where the job's records put sheet `i`.
fn region(d: &cadaddy_engine::doc::Drawing, i: usize) -> Bounds2 {
    let v: serde_json::Value = serde_json::from_str(d.agent.as_deref().unwrap()).unwrap();
    let r: Vec<f64> = v["guided_job"]["sheet_regions"][i]["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    Bounds2::new(Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3]))
}

fn on(d: &cadaddy_engine::doc::Drawing, rect: &Bounds2) -> BTreeSet<Handle> {
    d.model.iter().filter(|e| entity_bounds(d, e, 0).intersects(rect)).map(|e| e.handle).collect()
}

fn shown(app: &CadApp) -> BTreeSet<Handle> {
    app.session.view_doc().unwrap().model.iter().map(|e| e.handle).collect()
}

/// A made-up print set written to `<tmp>/<tag>/Sample Set.cdy` as any drawing is written.
fn written(tag: &str, sheets: usize, per: usize) -> (std::path::PathBuf, String, cadaddy_engine::doc::Drawing) {
    let set = sample::print_set(sheets, per);
    let dir = std::env::temp_dir().join(format!("cadaddy-sheets-on-demand-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Set.cdy").to_string_lossy().into_owned();
    std::fs::write(&path, cadaddy_io::write(&set, &path).unwrap()).unwrap();
    (dir, path, set)
}

fn open(h: &mut Harness<'_, CadApp>, path: &str) {
    h.state_mut().open_path(path);
    let opened = |app: &CadApp| app.session.state().is_ok_and(|st| st.path.as_deref() == Some(path));
    frames_until(h, |app| opened(app) && app.session.active_sheet().is_some() && app.work_running().is_none());
    assert!(opened(h.state()), "the file opened: {:?}", h.state().session.log.last());
}

#[test]
fn a_print_set_opened_from_a_file_holds_its_open_sheet_not_the_whole_set() {
    let (dir, path, set) = written("open", 6, 1500);
    let total = set.model.len();
    let mut h = empty_app();
    open(&mut h, &path);
    let held = h.state().session.doc().unwrap().model.len();
    assert!(held < total / 2, "{held} of the set's {total} objects held to show one sheet of six");
    assert_eq!(shown(h.state()), on(&set, &region(&set, 0)), "the open sheet is drawn whole");
    // Page Down opens the next sheet: it comes into memory, the set never does.
    key(&mut h, Key::PageDown);
    frames_until(&mut h, |app| app.session.active_sheet().map(|(i, _)| i) == Some(1) && app.work_running().is_none());
    assert_eq!(shown(h.state()), on(&set, &region(&set, 1)), "the second sheet is drawn whole");
    assert!(h.state().session.doc().unwrap().model.len() < total / 2);
    // A command that counts the whole set loads it first, and counts it all.
    let r = h.state_mut().run("entities", json!({"limit": 1})).unwrap();
    assert_eq!(r["count"], json!(total));
    let _ = std::fs::remove_dir_all(&dir);
}
