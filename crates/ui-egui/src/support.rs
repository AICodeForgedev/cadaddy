//! Feedback and donations: Help ▸ Report a Bug / Request a Feature / Support CADaddy, the heart
//! button in the ribbon's title row, and a small support banner (like Wikipedia's) at most once a
//! month. Never a nag dialog, never a locked feature.
//!
//! The links go to aicodeforge.dev/cadaddy and open in the
//! system browser: the feedback form gets the version and OS in its address, and /donate
//! redirects to the GoFundMe page named in the server's `donate-url` file, so the fundraiser can
//! change without a release.

use egui::{Align2, Color32, Pos2, RichText, Shape, Stroke, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

/// Whether CADaddy asks for support (Help ▸ Support CADaddy, the About box, the heart button and
/// the monthly banner).
pub const DONATIONS: bool = true;
pub const SITE: &str = "https://aicodeforge.dev/cadaddy";
pub const FEEDBACK_EMAIL: &str = "feedback@aicodeforge.dev";
/// How to report a security problem (SECURITY.md): About links to it.
pub const SECURITY_URL: &str = "https://github.com/AICodeForgedev/cadaddy/blob/main/SECURITY.md";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The feedback form, preset to `kind` ("bug" or "idea").
pub fn feedback_url(kind: &str) -> String {
    format!("{SITE}/feedback?kind={kind}&v={VERSION}&os={}", std::env::consts::OS)
}

pub fn donate_url() -> String {
    format!("{SITE}/donate")
}

/// How long the support banner stays away once it has shown.
pub const BANNER_EVERY_MS: f64 = 30.0 * 24.0 * 3600.0 * 1000.0;

/// What the support banner remembers (kept in the preferences).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DonatePrefs {
    /// When the banner last showed (ms since 1970; 0 = never).
    pub shown_ms: f64,
    /// The banner is up in this session (until closed).
    pub showing: bool,
    /// The host shows the banner (the desktop app turns it on; tests and other hosts don't).
    pub enabled: bool,
}

impl DonatePrefs {
    /// At most once a month.
    pub fn due(&self, now_ms: f64) -> bool {
        DONATIONS && (self.shown_ms <= 0.0 || now_ms - self.shown_ms >= BANNER_EVERY_MS)
    }
}

/// The heart button's entries: (command, label).
pub fn entries() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("ui.feedback", "Report a Bug..."), ("ui.feature", "Request a Feature...")];
    if DONATIONS {
        v.push(("ui.donate", "Support CADaddy..."));
    }
    v
}

/// The heart button's caption.
pub fn button_label() -> &'static str {
    if DONATIONS { "Donate / Improve" } else { "Improve CADaddy" }
}

/// A filled heart centred on `c`, `size` points wide.
pub fn paint_heart(painter: &egui::Painter, c: Pos2, size: f32, color: Color32) {
    let r = size * 0.27;
    painter.circle_filled(pos2(c.x - r * 0.95, c.y - r * 0.55), r, color);
    painter.circle_filled(pos2(c.x + r * 0.95, c.y - r * 0.55), r, color);
    let w = r * 1.95;
    painter.add(Shape::convex_polygon(
        vec![pos2(c.x - w, c.y - r * 0.25), pos2(c.x + w, c.y - r * 0.25), pos2(c.x, c.y + size * 0.48)],
        color,
        Stroke::NONE,
    ));
}

/// The support banner: one small, non-blocking card in the bottom-right corner (clear of the
/// command line), at most once a month. Agents driving the control channel never see it, so their screenshots stay clean.
pub fn banner(app: &mut CadApp, ctx: &egui::Context) {
    if !app.donate.showing {
        let now = crate::now_ms();
        if !app.donate.enabled || app.has_control_channel() || now <= 0.0 || !app.donate.due(now) {
            return;
        }
        app.donate.showing = true;
        app.donate.shown_ms = now;
    }
    let t = Tokens::get();
    let blue = Color32::from_rgb(0x1f, 0x6b, 0xff);
    let mut close = false;
    egui::Area::new(egui::Id::new("support_banner")).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0)).order(egui::Order::Foreground).show(
        ctx,
        |ui| {
            egui::Frame::new()
                .fill(t.panel)
                .stroke(Stroke::new(1.0, blue.gamma_multiply(0.7)))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(80) })
                .show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::hover());
                        paint_heart(ui.painter(), r.center(), 20.0, blue);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(crate::tl!("CADaddy is free, and it always will be.")).strong());
                            ui.label(RichText::new(crate::tl!(
                            "No ads, no subscription. If CADaddy saves you time, a small donation pays for the AI time that builds the next features."
                        ))
                        .small()
                        .color(t.text_dim));
                        });
                        if ui.add(egui::Button::new(RichText::new(crate::tl!("Support CADaddy")).strong().color(Color32::WHITE)).fill(blue)).clicked()
                        {
                            app.pending_url = Some(donate_url());
                            close = true;
                        }
                        if ui.add(egui::Button::new("×").frame(false)).on_hover_text(crate::tl!("Close")).clicked() {
                            close = true;
                        }
                    });
                });
        },
    );
    if close {
        app.donate.showing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_links_carry_version_and_os() {
        let u = feedback_url("idea");
        assert!(u.starts_with("https://aicodeforge.dev/cadaddy/feedback?kind=idea&v="));
        assert!(u.contains(VERSION) && u.ends_with(std::env::consts::OS));
    }

    #[test]
    fn the_banner_comes_at_most_once_a_month() {
        let now = 1.8e12;
        assert_eq!(DonatePrefs::default().due(now), DONATIONS);
        assert!(!DonatePrefs { shown_ms: now - 29.0 * 86_400_000.0, ..Default::default() }.due(now));
        assert_eq!(DonatePrefs { shown_ms: now - 31.0 * 86_400_000.0, ..Default::default() }.due(now), DONATIONS);
    }

    #[test]
    fn donate_appears_only_while_donations_are_on() {
        assert_eq!(entries().iter().any(|(c, _)| *c == "ui.donate"), DONATIONS);
    }
}
