//! Updates: CADaddy looks for a newer release at most once a day (Options ▸ Preferences turns
//! it off; Help ▸ Check for Updates looks now). A newer version shows one small card in the
//! bottom-right corner: Update downloads it (the host checks its SHA-256), and it installs when
//! CADaddy closes (Restart Now closes it at once, asking about unsaved drawings first) and opens
//! again. Where this copy can't replace itself (a .deb, a build run from source) Update opens the
//! download page instead. The host does the network and installing ([`Updater`]); without one
//! (tests, the web) nothing here runs.

use egui::{Align2, Color32, RichText, Stroke, vec2};

use crate::CadApp;
use crate::theme::Tokens;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where every release is listed.
pub const RELEASES: &str = "https://github.com/AICodeForgedev/cadaddy/releases";
/// How long after one automatic check the next one waits.
pub const CHECK_EVERY_MS: f64 = 24.0 * 3600.0 * 1000.0;

/// A release newer than this one.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.7.6".
    pub version: String,
    /// Its release notes page.
    pub page: String,
}

/// What the host's background work reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// No newer release.
    Latest,
    Available(Release),
    /// The download is in place and checked; it installs when CADaddy closes.
    Downloaded,
    Failed(String),
}

/// The host's half: looking, downloading and installing.
pub trait Updater {
    /// Look for a newer release in the background.
    fn check(&self);
    /// Download `release` in the background (only when [`Self::can_install`]).
    fn download(&self, release: &Release);
    /// The next result of [`Self::check`] or [`Self::download`], if one has come in.
    fn poll(&self) -> Option<Event>;
    /// Whether this copy can replace itself (an installed MSI, app bundle or AppImage).
    fn can_install(&self) -> bool;
    /// Open CADaddy again after the update installs (Restart Now).
    fn relaunch_after_install(&self);
    /// CADaddy is exiting: install a downloaded update.
    fn on_exit(&self) {}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    /// Looking; `manual` from Help ▸ Check for Updates (which reports "up to date" too).
    Checking {
        manual: bool,
    },
    Available(Release),
    Downloading(Release),
    /// Downloaded: installs when CADaddy closes.
    Ready(Release),
    Failed(String),
}

/// Update state and preferences (kept in the preferences: `auto`, `checked_ms`, `skip`).
#[derive(Debug)]
pub struct UpdateState {
    /// Look automatically, at most once a day.
    pub auto: bool,
    /// When the last automatic look started (ms since 1970; 0 = never).
    pub checked_ms: f64,
    /// A version the user chose to skip: it doesn't show again by itself.
    pub skip: String,
    pub phase: Phase,
    /// The card is up.
    pub showing: bool,
    /// The automatic look ran in this session.
    started: bool,
}

impl Default for UpdateState {
    fn default() -> Self {
        UpdateState { auto: true, checked_ms: 0.0, skip: String::new(), phase: Phase::Idle, showing: false, started: false }
    }
}

impl UpdateState {
    /// An automatic look is due.
    pub fn due(&self, now_ms: f64) -> bool {
        self.auto && !self.started && now_ms > 0.0 && (self.checked_ms <= 0.0 || now_ms - self.checked_ms >= CHECK_EVERY_MS)
    }
}

/// Whether version `a` ("0.7.10", "v1.2") is newer than `b`; text that isn't numbers is never newer.
pub fn newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Option<Vec<u64>> {
        let v = v.trim().trim_start_matches(['v', 'V']);
        let core = v.split(['-', '+']).next()?;
        core.split('.').map(|p| p.parse().ok()).collect()
    }
    match (parts(a), parts(b)) {
        (Some(mut x), Some(mut y)) => {
            let n = x.len().max(y.len());
            x.resize(n, 0);
            y.resize(n, 0);
            x > y
        }
        _ => false,
    }
}

/// Help ▸ Check for Updates.
pub fn check_now(app: &mut CadApp) {
    let Some(u) = &app.updater else {
        app.pending_url = Some(format!("{RELEASES}/latest"));
        return;
    };
    match &app.update.phase {
        Phase::Checking { .. } | Phase::Downloading(_) => {}
        Phase::Ready(_) | Phase::Available(_) => app.update.showing = true,
        Phase::Idle | Phase::Failed(_) => {
            app.update.phase = Phase::Checking { manual: true };
            u.check();
            app.set_status(crate::tl!("Checking for updates..."));
        }
    }
}

/// Start the daily look and take in what the host has found; called every frame.
pub fn tick(app: &mut CadApp) {
    let Some(u) = &app.updater else { return };
    let now = crate::now_ms();
    if app.update.due(now) && !app.has_control_channel() && app.update.phase == Phase::Idle {
        app.update.started = true;
        app.update.checked_ms = now;
        app.update.phase = Phase::Checking { manual: false };
        u.check();
    }
    let Some(event) = u.poll() else { return };
    let manual = matches!(app.update.phase, Phase::Checking { manual: true });
    match event {
        Event::Latest => {
            app.update.phase = Phase::Idle;
            if manual {
                app.set_status(crate::tf!("CADaddy {v} is the latest version.", v = VERSION));
            }
        }
        Event::Available(r) => {
            // A skipped version stays quiet unless asked for.
            app.update.showing = manual || r.version != app.update.skip;
            app.update.phase = Phase::Available(r);
        }
        Event::Downloaded => {
            if let Phase::Downloading(r) = &app.update.phase {
                app.update.phase = Phase::Ready(r.clone());
                app.update.showing = true;
            }
        }
        Event::Failed(e) => {
            log::warn!("update: {e}");
            // An automatic look that fails (offline) says nothing; anything asked for does.
            if manual || matches!(app.update.phase, Phase::Downloading(_)) {
                app.update.phase = Phase::Failed(e);
                app.update.showing = true;
            } else {
                app.update.phase = Phase::Idle;
            }
        }
    }
}

/// What a card button asked for.
enum Act {
    Update,
    Notes,
    Later,
    Skip,
    Restart,
    Page,
}

/// Space between the support banner and the update card stacked on it.
const STACK_GAP: f32 = 8.0;

/// The bottom-right corner: the support banner, with the update card above it when both are up
/// (an update to act on never waits behind the banner).
pub fn corner(app: &mut CadApp, ctx: &egui::Context) {
    let banner = crate::support::banner(app, ctx);
    card_above(app, ctx, banner.map_or(0.0, |r| r.height() + STACK_GAP));
}

/// The update card alone, in the corner.
pub fn card(app: &mut CadApp, ctx: &egui::Context) {
    card_above(app, ctx, 0.0);
}

/// The update card: bottom-right, `lift` points above the corner, non-blocking, never shown to
/// agents on the control channel.
fn card_above(app: &mut CadApp, ctx: &egui::Context, lift: f32) {
    if !app.update.showing || app.has_control_channel() {
        return;
    }
    let can_install = app.updater.as_ref().is_some_and(|u| u.can_install());
    let t = Tokens::get();
    let blue = Color32::from_rgb(0x1f, 0x6b, 0xff);
    let mut act = None;
    let (title, body): (String, String) = match &app.update.phase {
        Phase::Available(r) => (
            crate::tf!("CADaddy {v} is available", v = r.version),
            crate::tf!("You have {v}. Updating keeps your drawings and settings.", v = VERSION),
        ),
        Phase::Downloading(r) => (crate::tf!("Downloading CADaddy {v}...", v = r.version), String::new()),
        Phase::Ready(r) => (
            crate::tf!("CADaddy {v} is ready to install", v = r.version),
            crate::tl!("It installs when you close CADaddy. Restart now to update right away.").to_string(),
        ),
        Phase::Failed(e) => (crate::tl!("Couldn't update CADaddy").to_string(), e.clone()),
        Phase::Idle | Phase::Checking { .. } => return,
    };
    let area = egui::Area::new(egui::Id::new("update_card")).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0 - lift.max(0.0)));
    area.order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::new()
            .fill(t.panel)
            .stroke(Stroke::new(1.0, blue.gamma_multiply(0.7)))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(80) })
            .show(ui, |ui| {
                ui.set_max_width(420.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).strong());
                    if matches!(app.update.phase, Phase::Downloading(_)) {
                        ui.spinner();
                    }
                });
                if !body.is_empty() {
                    ui.label(RichText::new(body).small().color(t.text_dim));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let primary = |ui: &mut egui::Ui, text: &str| {
                        ui.add(egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(blue)).clicked()
                    };
                    match &app.update.phase {
                        Phase::Available(_) => {
                            let label = if can_install { crate::tl!("Update") } else { crate::tl!("Download") };
                            if primary(ui, label) {
                                act = Some(Act::Update);
                            }
                            if ui.button(crate::tl!("What's New")).clicked() {
                                act = Some(Act::Notes);
                            }
                            if ui.button(crate::tl!("Later")).clicked() {
                                act = Some(Act::Later);
                            }
                            if ui.button(crate::tl!("Skip This Version")).clicked() {
                                act = Some(Act::Skip);
                            }
                        }
                        Phase::Ready(_) => {
                            if primary(ui, crate::tl!("Restart Now")) {
                                act = Some(Act::Restart);
                            }
                            if ui.button(crate::tl!("When I Close CADaddy")).clicked() {
                                act = Some(Act::Later);
                            }
                        }
                        Phase::Failed(_) => {
                            if primary(ui, crate::tl!("Download Page")) {
                                act = Some(Act::Page);
                            }
                            if ui.button(crate::tl!("Close")).clicked() {
                                act = Some(Act::Later);
                            }
                        }
                        _ => {
                            if ui.button(crate::tl!("Hide")).clicked() {
                                act = Some(Act::Later);
                            }
                        }
                    }
                });
            });
    });
    let Some(act) = act else { return };
    let release = match &app.update.phase {
        Phase::Available(r) | Phase::Downloading(r) | Phase::Ready(r) => Some(r.clone()),
        _ => None,
    };
    match act {
        Act::Update => {
            if let Some(r) = release {
                match &app.updater {
                    Some(u) if can_install => {
                        u.download(&r);
                        app.update.phase = Phase::Downloading(r);
                    }
                    _ => {
                        app.pending_url = Some(r.page.clone());
                        app.update.showing = false;
                    }
                }
            }
        }
        Act::Notes => {
            if let Some(r) = release {
                app.pending_url = Some(r.page);
            }
        }
        Act::Later => {
            app.update.showing = false;
            if matches!(app.update.phase, Phase::Failed(_)) {
                app.update.phase = Phase::Idle;
            }
        }
        Act::Skip => {
            if let Some(r) = release {
                app.update.skip = r.version;
            }
            app.update.showing = false;
        }
        Act::Restart => {
            if let Some(u) = &app.updater {
                u.relaunch_after_install();
            }
            app.quit_requested = true;
        }
        Act::Page => {
            app.pending_url = Some(format!("{RELEASES}/latest"));
            app.update.showing = false;
            app.update.phase = Phase::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.7.10", "0.7.9"));
        assert!(newer("v0.8.0", "0.7.5"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.7.5", "0.7.5"));
        assert!(!newer("0.7.5", "0.7.5.0"));
        assert!(!newer("0.7.4", "0.7.5"));
        assert!(!newer("latest", "0.7.5"));
        assert!(newer("0.7.6-beta", "0.7.5"));
    }

    #[test]
    fn the_automatic_look_comes_at_most_once_a_day() {
        let now = 1.8e12;
        assert!(UpdateState::default().due(now));
        assert!(!UpdateState { checked_ms: now - 3_600_000.0, ..Default::default() }.due(now));
        assert!(UpdateState { checked_ms: now - 25.0 * 3_600_000.0, ..Default::default() }.due(now));
        assert!(!UpdateState { auto: false, ..Default::default() }.due(now));
        assert!(!UpdateState { started: true, ..Default::default() }.due(now));
    }
}
