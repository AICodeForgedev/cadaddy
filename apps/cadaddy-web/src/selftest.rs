//! `?selftest`: the web build tests itself in the browser it runs in, as a user would use it, and
//! reports to the page that served it (`cargo xtask webtest`): open a DXF and a DWG drawing
//! (fetched, through the same inbox File ▸ Open uses), draw and undo, save (a browser download),
//! Drawing Recovery of an autosave a closed tab left in browser storage, a walkthrough package
//! opened in Walk, and autosave writing into browser storage. Each step's result goes to
//! `selftest/result?data=<JSON>` on the same site, and the page title ends with the summary.

use std::sync::Mutex;

use cadaddy_ui_egui::CadApp;
use serde_json::{Value, json};
use web_sys::js_sys;

/// What the download hook delivered while the test ran: (file name, bytes).
static SAVED: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());

/// The download hook during a selftest: noted, then downloaded as usual.
pub fn deliver(name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    if let Ok(mut s) = SAVED.lock() {
        s.push((name.to_string(), bytes.len()));
    }
    crate::download::download(name, mime, bytes)
}

fn now() -> f64 {
    js_sys::Date::now() / 1000.0
}

/// The store key prefix of autosaved drawings (`cadaddy_engine::autosave::store`).
const AUTOSAVE_PREFIX: &str = "cadaddy.autosave/";
/// The instance id of the "closed tab" whose autosave Drawing Recovery offers.
const CLOSED_TAB: &str = "424242-1";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Start,
    OpenDxf,
    OpenDwg,
    Draw,
    Save,
    Recover,
    Walk,
    Change,
    Autosave,
    Report,
    Done,
}

pub struct SelfTest {
    step: Step,
    since: f64,
    results: Vec<Value>,
    /// The active drawing before a file was fetched (its uid): the fetched one replaces it.
    before: Option<u64>,
}

fn active_uid(app: &CadApp) -> Option<u64> {
    app.session.state().ok().map(|s| s.uid)
}

fn entities(app: &CadApp) -> usize {
    app.session.doc().map_or(0, |d| d.entity_count())
}

fn title(app: &CadApp) -> String {
    app.session.state().map(|s| s.title.clone()).unwrap_or_default()
}

impl SelfTest {
    pub fn new() -> SelfTest {
        SelfTest { step: Step::Start, since: now(), results: Vec::new(), before: None }
    }

    fn record(&mut self, step: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        log::info!("selftest: {} {step}: {detail}", if ok { "PASS" } else { "FAIL" });
        self.results.push(json!({ "step": step, "ok": ok, "detail": detail }));
    }

    fn next(&mut self, step: Step) {
        self.step = step;
        self.since = now();
    }

    /// Wait for `done`, at most `secs`: Some(true) done, Some(false) timed out, None still waiting.
    fn wait(&self, done: bool, secs: f64) -> Option<bool> {
        if done {
            Some(true)
        } else if now() - self.since > secs {
            Some(false)
        } else {
            None
        }
    }

    /// One step of the test per frame.
    pub fn tick(&mut self, app: &mut CadApp, inbox: &crate::upload::Inbox, ctx: &egui::Context) {
        ctx.request_repaint();
        match self.step {
            Step::Start => {
                // Autosave a minute after a change (the shortest SAVETIME), checked at the end.
                let _ = app.run("setvar", json!({ "name": "SAVETIME", "value": 1 }));
                self.before = active_uid(app);
                inbox.fetch("selftest/plan.dxf".into());
                self.next(Step::OpenDxf);
            }
            Step::OpenDxf | Step::OpenDwg => {
                let (label, then) = if self.step == Step::OpenDxf { ("open DXF", Step::OpenDwg) } else { ("open DWG", Step::Draw) };
                // The fetched drawing is open and active (another drawing than before).
                let open = active_uid(app).is_some_and(|u| Some(u) != self.before) && entities(app) > 0;
                // The web build doesn't read DWG yet: a clear refusal (saying to use DXF) is right too.
                let refused =
                    self.step == Step::OpenDwg && app.status.as_ref().is_some_and(|s| s.0.contains("DWG files can't be opened in the web build"));
                if refused {
                    self.record(label, true, "refused with a clear message (the web build reads DXF; DWG is a known limit)");
                    self.next(then);
                } else if let Some(ok) = self.wait(open, 30.0) {
                    let why = if ok { String::new() } else { format!(" (status: {})", app.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()) };
                    self.record(label, ok, format!("{} with {} objects{why}", title(app), entities(app)));
                    if then == Step::OpenDwg {
                        self.before = active_uid(app);
                        inbox.fetch("selftest/plan.dwg".into());
                    }
                    self.next(then);
                }
            }
            Step::Draw => {
                let before = entities(app);
                let drawn = app.run("circle", json!({ "center": [0, 0], "radius": 5 })).is_ok() && entities(app) == before + 1;
                let undone = app.run("undo", json!({})).is_ok() && entities(app) == before;
                // A change that stays, for autosave.
                let kept = app.run("line", json!({ "points": [[0, 0], [10, 10]] })).is_ok() && entities(app) == before + 1;
                self.record("draw and undo", drawn && undone && kept, format!("{before} → +1 → undo {} → line {}", entities(app) - 1, entities(app)));
                self.next(Step::Save);
            }
            Step::Save => {
                let before = SAVED.lock().map_or(0, |s| s.len());
                let r = app.run("saveas", json!({ "path": "Selftest Plan.dxf" }));
                let got = SAVED.lock().ok().and_then(|s| s.get(before).cloned());
                let ok = r.is_ok() && got.as_ref().is_some_and(|(n, b)| n.ends_with(".dxf") && *b > 0);
                self.record("save (download)", ok, format!("{got:?} {:?}", r.err()));
                self.next(Step::Recover);
            }
            Step::Recover => {
                self.record_recovery(app);
                inbox.fetch("selftest/plan.cdywalk".into());
                self.next(Step::Walk);
            }
            Step::Walk => {
                if let Some(ok) = self.wait(app.walk.is_some(), 30.0) {
                    self.record("walkthrough in Walk", ok, if ok { "the .cdywalk opened in Walk" } else { "Walk did not open" });
                    let _ = app.run("ui.walk", json!({}));
                    self.next(Step::Change);
                }
            }
            // A change not saved yet, for autosave (SAVETIME 1: written a minute later).
            Step::Change => {
                let _ = app.run("circle", json!({ "center": [50, 50], "radius": 2 }));
                self.next(Step::Autosave);
            }
            Step::Autosave => {
                let store = cadaddy_engine::autosave::store::store();
                let written = store.is_some_and(|s| (s.keys)().iter().any(|k| k.starts_with(AUTOSAVE_PREFIX) && !k.contains(CLOSED_TAB)));
                if let Some(ok) = self.wait(written, 100.0) {
                    self.record("autosave to browser storage", ok, if ok { "an autosave is in local storage" } else { "nothing written in 100 s" });
                    self.next(Step::Report);
                }
            }
            Step::Report => {
                let passed = self.results.iter().filter(|r| r["ok"] == true).count();
                let report = json!({ "passed": passed, "total": self.results.len(), "steps": self.results }).to_string();
                let url = format!("selftest/result?data={}", js_sys::encode_uri_component(&report));
                if let Some(w) = web_sys::window() {
                    let _ = w.fetch_with_str(&url);
                    if let Some(d) = w.document() {
                        d.set_title(&format!("selftest: {passed}/{} passed", self.results.len()));
                    }
                }
                self.next(Step::Done);
            }
            Step::Done => {}
        }
    }

    /// Drawing Recovery: an autosave a closed tab left (no lock: it stopped) is offered and opens.
    fn record_recovery(&mut self, app: &mut CadApp) {
        let Some(store) = cadaddy_engine::autosave::store::store() else {
            self.record("drawing recovery", false, "no browser storage");
            return;
        };
        let mut d = cadaddy_engine::doc::Drawing::new_imperial();
        for i in 0..3 {
            let f = f64::from(i);
            let line = cadaddy_engine::doc::Line { a: cadaddy_engine::geom::Vec3::new(f, 0.0, 0.0), b: cadaddy_engine::geom::Vec3::new(f, 5.0, 0.0) };
            let _ = d.add(&cadaddy_engine::doc::Space::Model, Default::default(), cadaddy_engine::doc::EntityKind::Line(line));
        }
        let dxf = cadaddy_io::write(&d, "autosave.dxf").ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
        let name = cadaddy_engine::autosave::file_name("Recovered Plan", CLOSED_TAB, 7);
        let entry = json!({ "drawing": "Recovered Plan", "modified": now() as u64, "dxf": dxf }).to_string();
        if !(store.set)(&format!("{AUTOSAVE_PREFIX}{name}"), &entry) {
            self.record("drawing recovery", false, "browser storage refused the entry");
            return;
        }
        // As at start-up: the autosaves of tabs that stopped are offered.
        app.recovery.items = cadaddy_engine::autosave::store::recoverable_store("selftest-own");
        let Some(i) = app.recovery.items.iter().position(|r| r.drawing == "Recovered Plan") else {
            self.record(
                "drawing recovery",
                false,
                format!("not offered: {:?}", app.recovery.items.iter().map(|r| r.drawing.clone()).collect::<Vec<_>>()),
            );
            return;
        };
        let r = app.recover_item(i);
        let ok = r.is_ok() && entities(app) == 3;
        self.record("drawing recovery", ok, format!("{} with {} objects {:?}", title(app), entities(app), r.err()));
        (store.remove)(&format!("{AUTOSAVE_PREFIX}{name}"));
    }
}
