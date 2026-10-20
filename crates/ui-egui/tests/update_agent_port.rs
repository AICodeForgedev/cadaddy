//! The agent port (Options ▸ agentControl, on by default) doesn't silence the updater: the daily
//! look runs and Help ▸ Check for Updates shows its card. Only a window started with
//! `--control PORT` (tests, automation) keeps them away. 0.7.7 and 0.7.8 counted the default agent
//! port as being driven, so no default install was ever offered an update.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use cadaddy_engine::Session;
use cadaddy_ui_egui::update::{self, Event, Release, Updater};
use cadaddy_ui_egui::{CadApp, Services};

/// Counts looks; the first poll after one reports `next`.
struct Fake {
    checks: Arc<AtomicUsize>,
    next: std::sync::Mutex<Option<Event>>,
}

impl Updater for Fake {
    fn check(&self) {
        self.checks.fetch_add(1, Ordering::SeqCst);
    }
    fn download(&self, _release: &Release) {}
    fn poll(&self) -> Option<Event> {
        if self.checks.load(Ordering::SeqCst) == 0 {
            return None;
        }
        self.next.lock().ok()?.take()
    }
    fn can_install(&self) -> bool {
        true
    }
    fn relaunch_after_install(&self) {}
}

fn app(scripted: bool) -> (CadApp, Arc<AtomicUsize>, mpsc::Sender<cadaddy_ui_egui::ControlRequest>) {
    let (tx, rx) = mpsc::channel();
    let mut app = CadApp::new(Session::new(), Services::default()).with_control(rx, scripted);
    let checks = Arc::new(AtomicUsize::new(0));
    let next = Event::Available(Release { version: "99.0.0".into(), page: "https://example.invalid/notes".into() });
    app.updater = Some(Box::new(Fake { checks: checks.clone(), next: std::sync::Mutex::new(Some(next)) }));
    (app, checks, tx)
}

fn painted(app: &mut CadApp, ctx: &egui::Context) -> String {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| update::card(app, ui.ctx()));
    output.textures_delta.clear();
    let mut text = String::new();
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(s) => {
                text.push_str(s.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, text)),
            _ => {}
        }
    }
    for s in &output.shapes {
        collect(&s.shape, &mut text);
    }
    text
}

#[test]
fn the_default_agent_port_still_gets_updates() {
    let (mut app, checks, _tx) = app(false);
    assert!(!app.has_control_channel(), "the agent port alone is not a driven window");
    // The automatic look runs, finds 99.0.0 and shows the card.
    update::tick(&mut app);
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    update::tick(&mut app);
    assert!(app.update.showing, "{:?}", app.update.phase);
    let ctx = egui::Context::default();
    // An area is measured on its first frame and drawn from the second.
    painted(&mut app, &ctx);
    let text = painted(&mut app, &ctx);
    assert!(text.contains("99.0.0"), "the card is drawn: {text:?}");
}

#[test]
fn a_window_started_with_control_stays_quiet() {
    let (mut app, checks, _tx) = app(true);
    assert!(app.has_control_channel());
    update::tick(&mut app);
    assert_eq!(checks.load(Ordering::SeqCst), 0, "no automatic look while driven");
    // Asked for (Help ▸ Check for Updates over the channel): it looks, but draws no card.
    update::check_now(&mut app);
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    update::tick(&mut app);
    let ctx = egui::Context::default();
    painted(&mut app, &ctx);
    assert!(!painted(&mut app, &ctx).contains("99.0.0"));
}
