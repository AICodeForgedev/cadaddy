//! The agent port (Options ▸ agentControl, on by default) doesn't silence the updater: the daily
//! look runs and Help ▸ Check for Updates shows its card. Only a window started with
//! `--control PORT` (tests, automation) keeps them away. 0.7.7 and 0.7.8 counted the default agent
//! port as being driven, so no default install was ever offered an update.
//!
//! The frames are drawn the way the app draws its corner ([`update::corner`]): the monthly support
//! banner is due too, and the card stacks above it instead of waiting behind it.

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
    // The desktop host turns the support banner on; it hasn't shown yet, so it is due.
    app.donate.enabled = true;
    (app, checks, tx)
}

/// One frame of the bottom-right corner in `language`; the text drawn.
fn painted_in(language: &'static str, app: &mut CadApp, ctx: &egui::Context) -> String {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| cadaddy_ui_egui::i18n::with_language(language, || update::corner(app, ui.ctx())));
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

fn painted(app: &mut CadApp, ctx: &egui::Context) -> String {
    painted_in("en", app, ctx)
}

/// Where an area was laid out last frame.
fn area(ctx: &egui::Context, id: &str) -> Option<egui::Rect> {
    ctx.memory(|m| m.area_rect(egui::Id::new(id)))
}

const BANNER: &str = "CADaddy is free";

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
    assert!(text.contains(BANNER), "the support banner is up too: {text:?}");
    // Both sit bottom-right; the card is above the banner, clear of it, not behind it.
    let card = area(&ctx, "update_card").expect("the card's area");
    let banner = area(&ctx, "support_banner").expect("the banner's area");
    assert!(card.max.y <= banner.min.y, "card {card:?} above banner {banner:?}");
    assert!(card.max.x > 1500.0 && banner.max.x > 1500.0, "both at the right: {card:?} {banner:?}");
    // The banner closed: the card drops into the corner.
    app.donate.showing = false;
    app.donate.shown_ms = cadaddy_ui_egui::now_ms();
    painted(&mut app, &ctx);
    let text = painted(&mut app, &ctx);
    assert!(text.contains("99.0.0") && !text.contains(BANNER), "{text:?}");
    let alone = area(&ctx, "update_card").expect("the card's area");
    assert!((alone.max.y - banner.max.y).abs() < 1.0, "the card sits where the banner was: {alone:?} {banner:?}");
}

#[test]
fn the_card_speaks_the_interface_language() {
    let (mut app, _checks, _tx) = app(false);
    update::tick(&mut app);
    update::tick(&mut app);
    let ctx = egui::Context::default();
    painted_in("de", &mut app, &ctx);
    let text = painted_in("de", &mut app, &ctx);
    for german in ["CADaddy 99.0.0 ist verfügbar", "Neuerungen", "Später", "Diese Version überspringen"] {
        assert!(text.contains(german), "{german:?} in {text:?}");
    }
    assert!(!text.contains("Skip This Version"), "{text:?}");
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
    let text = painted(&mut app, &ctx);
    assert!(!text.contains("99.0.0"), "no card: {text:?}");
    assert!(!text.contains(BANNER), "no banner either: {text:?}");
    assert!(app.update.showing, "the offer is kept, just not drawn while driven");
}
