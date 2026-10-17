//! Feedback and donations: Help ▸ Report a Bug / Request a Feature, the heart button in the
//! ribbon's title row, and a donation prompt (like Wikipedia's) shown once per version after an
//! install or update while donations are on.
//!
//! The links go to aicodeforge.dev/cadaddy (`pdfapp/cadaddy/site.mjs` on S1): the feedback form
//! gets the version and OS in its address, and /donate redirects to the fundraiser, so the
//! fundraiser can change without a release.

use egui::{Align2, Color32, Pos2, RichText, Shape, Stroke, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

/// Whether CADaddy asks for donations (the Donate entries and the prompt). Off until the
/// fundraiser is live.
pub const DONATIONS: bool = false;
pub const SITE: &str = "https://aicodeforge.dev/cadaddy";
pub const FEEDBACK_EMAIL: &str = "feedback@aicodeforge.dev";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The feedback form, preset to `kind` ("bug" or "idea").
pub fn feedback_url(kind: &str) -> String {
    format!("{SITE}/feedback?kind={kind}&v={VERSION}&os={}", std::env::consts::OS)
}

pub fn donate_url() -> String {
    format!("{SITE}/donate")
}

/// What the donation prompt remembers (kept in the preferences).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DonatePrefs {
    /// The version the prompt last showed in; it shows again after the next update.
    pub asked_version: String,
    /// "Don't ask again".
    pub never: bool,
}

impl DonatePrefs {
    pub fn wants_prompt(&self) -> bool {
        DONATIONS && !self.never && self.asked_version != VERSION
    }
}

/// The heart button's entries: (command, label).
pub fn entries() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("ui.feedback", "Report a Bug..."), ("ui.feature", "Request a Feature...")];
    if DONATIONS {
        v.push(("ui.donate", "Donate..."));
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

/// The donation prompt, at the bottom of the window. Agents driving the control channel never
/// see it, so their screenshots stay clean.
pub fn prompt(app: &mut CadApp, ctx: &egui::Context) {
    if !app.donate.wants_prompt() || app.has_control_channel() {
        return;
    }
    let t = Tokens::get();
    let mut answer = None;
    egui::Window::new(crate::tl!("Support CADaddy"))
        .id(egui::Id::new("donate_prompt"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -56.0))
        .fixed_size(vec2(520.0, 0.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), egui::Sense::hover());
                paint_heart(ui.painter(), r.center(), 26.0, Color32::from_rgb(0x1f, 0x6b, 0xff));
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::tl!("CADaddy is free, and it always will be.")).strong().size(16.0));
                    ui.label(crate::tl!(
                        "No ads, no subscription. If CADaddy saves you time, a small donation pays for the AI time that builds the next features."
                    ));
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(crate::tl!("Donate")).strong().color(Color32::WHITE)).fill(t.accent)).clicked() {
                    answer = Some("donate");
                }
                if ui.button(crate::tl!("Maybe later")).clicked() {
                    answer = Some("later");
                }
                if ui.button(crate::tl!("Don't ask again")).clicked() {
                    answer = Some("never");
                }
            });
        });
    if let Some(a) = answer {
        app.donate.asked_version = VERSION.to_string();
        app.donate.never = a == "never";
        if a == "donate" {
            app.pending_url = Some(donate_url());
        }
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
    fn the_prompt_shows_once_per_version_and_never_after_dont_ask() {
        let fresh = DonatePrefs::default();
        assert_eq!(fresh.wants_prompt(), DONATIONS);
        assert!(!DonatePrefs { asked_version: VERSION.into(), never: false }.wants_prompt());
        assert!(!DonatePrefs { asked_version: String::new(), never: true }.wants_prompt());
        assert_eq!(DonatePrefs { asked_version: "0.0.1".into(), never: false }.wants_prompt(), DONATIONS);
    }

    #[test]
    fn donate_appears_only_while_donations_are_on() {
        assert_eq!(entries().iter().any(|(c, _)| *c == "ui.donate"), DONATIONS);
    }
}
