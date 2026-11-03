//! What happens when the interface panics. Commands already run under their own guard
//! (`cadaddy_engine::guard`); this one covers the rest of the frame, so a panic in a panel keeps
//! the user's work: every drawing with unsaved changes is autosaved at once and the app says so.
//! A panic that keeps coming back leaves a plain window that only closes, with the recovery copies
//! kept for the next start. Each one gets a crash report (`cadaddy_engine::crash_report`), which
//! stays on this machine until Help > Report a Problem offers it. None of this helps on the web,
//! where a panic aborts.

use std::panic::{AssertUnwindSafe, catch_unwind};

use egui::{RichText, vec2};

use crate::theme::Tokens;
use crate::{CadApp, now_ms};

/// This many panics, each within [`QUIET_MS`] of the last, and the interface is given up on.
const MAX_STRIKES: u32 = 3;
/// A stretch this long without a panic means the app has recovered.
const QUIET_MS: f64 = 5_000.0;

/// Panics caught around the frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFault {
    /// The last panic's message, until the user dismisses the notice.
    pub message: Option<String>,
    /// Drawings autosaved because of panics so far.
    pub autosaved: usize,
    /// Panics in the current run of them.
    pub strikes: u32,
    /// When the last one happened.
    pub last_ms: f64,
    /// The interface keeps panicking: it no longer runs, and recovery copies are kept on exit.
    pub broken: bool,
}

impl CadApp {
    /// [`CadApp::logic`] under the panic guard.
    pub fn guarded_logic(&mut self, ctx: &egui::Context) {
        self.guarded(ctx, |app| app.logic(ctx));
    }

    /// [`CadApp::ui`] under the panic guard, then the notice about any panic. Once the interface
    /// has been given up on, only the window that closes the app is drawn.
    pub fn guarded_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.guarded(&ctx, |app| app.ui(ui));
        self.show_fault(&ctx);
    }

    /// The notice about a caught panic, or the window that closes the app.
    pub fn show_fault(&mut self, ctx: &egui::Context) {
        if self.fault.broken {
            broken_window(self, ctx);
        } else {
            notice(self, ctx);
        }
    }

    /// Run `frame`; a panic in it autosaves unsaved drawings instead of ending the app.
    pub fn guarded(&mut self, ctx: &egui::Context, frame: impl FnOnce(&mut CadApp)) {
        if self.fault.broken {
            return;
        }
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| frame(self))) {
            let message = p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned());
            self.on_panic(ctx, message.unwrap_or_else(|| "panic".into()));
        }
    }

    fn on_panic(&mut self, ctx: &egui::Context, message: String) {
        let now = now_ms();
        let fault = &mut self.fault;
        fault.strikes = if now - fault.last_ms < QUIET_MS { fault.strikes + 1 } else { 1 };
        fault.last_ms = now;
        fault.broken = fault.strikes >= MAX_STRIKES;
        log::error!("the interface panicked: {message}");
        cadaddy_engine::crash_report::record(&self.session, "interface", &message);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(saver) = &mut self.autosave {
            let session = &self.session;
            // The save is the point of being here: a panic inside it must not end the app either.
            self.fault.autosaved += catch_unwind(AssertUnwindSafe(|| saver.emergency_save(session))).unwrap_or(0);
        }
        self.fault.message = Some(message);
        ctx.request_repaint();
    }
}

/// The notice after a panic. It doesn't block the drawing: the user needs to be able to save.
fn notice(app: &mut CadApp, ctx: &egui::Context) {
    let Some(message) = app.fault.message.clone() else { return };
    let t = Tokens::get();
    let mut dismiss = false;
    egui::Window::new(crate::tl!("Internal error"))
        .id(egui::Id::unique("cc_fault_notice"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 48.0))
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(crate::tl!("Part of CADaddy failed, and the app kept running."));
            ui.label(saved_text(app.fault.autosaved));
            ui.label(RichText::new(crate::tl!("Save your work and restart CADaddy.")).strong());
            ui.add_space(4.0);
            details(ui, t, &message);
            // A row just as tall as the button (a plain right-to-left layout takes the rest).
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(crate::tl!("Dismiss")).clicked() {
                    dismiss = true;
                }
            });
        });
    if dismiss {
        app.fault.message = None;
    }
}

/// All that's left once the interface keeps panicking.
fn broken_window(app: &CadApp, ctx: &egui::Context) {
    let t = Tokens::get();
    egui::Window::new(crate::tl!("CADaddy has to close"))
        .id(egui::Id::unique("cc_fault_broken"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(crate::tl!("Part of CADaddy keeps failing, so it can't continue."));
            ui.label(saved_text(app.fault.autosaved));
            ui.label(RichText::new(crate::tl!("Drawing Recovery offers them at the next start.")).strong());
            ui.add_space(4.0);
            if let Some(message) = &app.fault.message {
                details(ui, t, message);
            }
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(crate::tl!("Close CADaddy")).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
}

fn saved_text(autosaved: usize) -> String {
    match autosaved {
        0 => crate::tl!("No drawing needed a new recovery copy.").to_string(),
        1 => crate::tl!("A recovery copy of the drawing with unsaved changes was autosaved.").to_string(),
        k => crate::tf!("Recovery copies of {count} drawings with unsaved changes were autosaved.", count = k),
    }
}

fn details(ui: &mut egui::Ui, t: Tokens, message: &str) {
    ui.collapsing(crate::tl!("Details"), |ui| {
        ui.label(RichText::new(message).color(t.text_dim).monospace());
        ui.label(RichText::new(crate::tl!("A crash report was kept on this computer: Help > Report a Problem offers it.")).color(t.text_dim));
    });
}
