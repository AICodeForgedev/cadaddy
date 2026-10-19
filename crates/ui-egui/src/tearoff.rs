//! Tear-off drawing windows: a drawing tab dragged out of the tab bar (or "Move to New Window" on
//! its menu) opens in an OS window of its own (an egui viewport; on the web, a window inside the
//! page), with its own canvas and view — several drawings side by side, across monitors. The
//! palettes and the command line stay in the main window and act on the drawing whose window was
//! last clicked or focused. "Dock", closing the window, or dragging it back onto the main window's
//! tab bar puts the drawing back among the tabs.

use egui::{Rect, vec2};

use crate::CadApp;
use crate::canvas::CanvasState;

/// A drawing in a window of its own.
pub struct TornWindow {
    /// The drawing ([`cadaddy_engine::DocState::uid`]).
    pub uid: u64,
    /// Its window: outer top-left and inner size, in screen points.
    pub rect: [f32; 4],
    /// Its GPU layer set ([`crate::gpu::with_canvas_slot`]); 0 is the main window's.
    pub slot: usize,
    canvas: CanvasState,
}

/// Default size of a new drawing window, points.
const NEW_SIZE: [f32; 2] = [900.0, 640.0];

fn index_of(app: &CadApp, uid: u64) -> Option<usize> {
    app.session.docs.iter().position(|d| d.uid == uid)
}

/// Whether drawing `index` is in a window of its own.
pub fn is_torn(app: &CadApp, index: usize) -> bool {
    app.session.docs.get(index).is_some_and(|d| app.torn.iter().any(|w| w.uid == d.uid))
}

/// The drawing the main window shows: the active one unless it is torn off, else the last one
/// the main window showed, else the first one still in a tab.
pub fn main_index(app: &CadApp) -> Option<usize> {
    let active = app.session.active;
    if active < app.session.docs.len() && !is_torn(app, active) {
        return Some(active);
    }
    app.main_uid.and_then(|u| index_of(app, u)).filter(|i| !is_torn(app, *i)).or_else(|| (0..app.session.docs.len()).find(|i| !is_torn(app, *i)))
}

/// Move drawing `index` into a window of its own, its top-left at `at` (screen points) if given.
pub fn tear_off(app: &mut CadApp, index: usize, at: Option<egui::Pos2>) -> Result<(), String> {
    let uid = app.session.docs.get(index).map(|d| d.uid).ok_or("no such drawing")?;
    if app.torn.iter().any(|w| w.uid == uid) {
        return Ok(());
    }
    let slot = (1..crate::gpu::MAX_CANVASES).find(|s| !app.torn.iter().any(|w| w.slot == *s)).ok_or("too many drawing windows open")?;
    let mut canvas = CanvasState::default();
    canvas.gpu = app.canvas.gpu;
    canvas.gpu_device = app.canvas.gpu_device.clone();
    let n = app.torn.len() as f32;
    let at = at.unwrap_or(egui::pos2(120.0 + 30.0 * n, 120.0 + 30.0 * n));
    app.torn.push(TornWindow { uid, rect: [at.x, at.y, NEW_SIZE[0], NEW_SIZE[1]], slot, canvas });
    // The main window goes on with another drawing (or the Start tab when none is left).
    if app.session.active == index {
        match main_index(app) {
            Some(i) => app.session.active = i,
            None => app.ui.start_tab = true,
        }
    }
    Ok(())
}

/// Put drawing `uid` back among the main window's tabs, current.
pub fn dock(app: &mut CadApp, uid: u64) {
    app.torn.retain(|w| w.uid != uid);
    if let Some(i) = index_of(app, uid) {
        app.session.active = i;
        app.ui.start_tab = false;
    }
}

/// Run `f` (the main window's canvas) with the main window's drawing current; a drawing that
/// becomes current meanwhile (a click switched it) stays current.
pub fn with_main_doc(app: &mut CadApp, ui: &mut egui::Ui, f: impl FnOnce(&mut CadApp, &mut egui::Ui)) {
    let Some(main) = main_index(app) else {
        ui.centered_and_justified(|ui| ui.label(crate::tl!("Every drawing is in a window of its own.")));
        return;
    };
    app.main_uid = app.session.docs.get(main).map(|d| d.uid);
    let before = app.session.active;
    app.session.active = main;
    f(app, ui);
    if app.session.active == main && before != main {
        app.session.active = before;
    }
}

/// Where the main window's tab bar is, in its own points (set as it draws), for docking by drag.
pub fn note_tab_bar(app: &mut CadApp, r: Rect) {
    app.tab_bar = Some(r);
}

/// Draw every drawing window.
pub fn show_windows(app: &mut CadApp, ctx: &egui::Context) {
    // Windows of drawings that were closed go too.
    let open: Vec<u64> = app.session.docs.iter().map(|d| d.uid).collect();
    app.torn.retain(|w| open.contains(&w.uid));
    // The main window's tab bar on the screen: a drawing window dragged onto it docks.
    let main_inner = ctx.input(|i| i.viewport().inner_rect);
    let tab_bar_screen = app.tab_bar.zip(main_inner).map(|(t, inner)| t.translate(inner.min.to_vec2()).expand2(vec2(0.0, 12.0)));
    for k in 0..app.torn.len() {
        let Some(w) = app.torn.get_mut(k) else { break };
        let (uid, rect, slot) = (w.uid, w.rect, w.slot);
        let mut canvas = std::mem::take(&mut w.canvas);
        let Some(index) = index_of(app, uid) else { continue };
        let title = app.session.docs.get(index).map(|d| format!("{}{} - CADaddy", d.title, if d.is_dirty() { "*" } else { "" })).unwrap_or_default();
        let id = egui::ViewportId::from_hash_of(("cadaddy-drawing", uid));
        let builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_position(egui::pos2(rect[0], rect[1]))
            .with_inner_size(vec2(rect[2], rect[3]))
            .with_min_inner_size(vec2(320.0, 240.0));
        let mut dock_it = false;
        let mut moved: Option<[f32; 4]> = None;
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            let embedded = class == egui::ViewportClass::EmbeddedWindow;
            let mut body = |ui: &mut egui::Ui| {
                ui.horizontal(|ui| {
                    if embedded {
                        ui.strong(&title);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        dock_it = ui.small_button(crate::tl!("Dock")).on_hover_text(crate::tl!("Back among the drawing tabs")).clicked();
                    });
                });
                // The drawing current while its window draws (picks, grips and typing act on it);
                // it stays current when this window was clicked or has the keyboard.
                let before = app.session.active;
                app.session.active = index;
                std::mem::swap(&mut app.canvas, &mut canvas);
                crate::gpu::with_canvas_slot(slot, || crate::canvas::show(app, ui));
                std::mem::swap(&mut app.canvas, &mut canvas);
                let engaged = ui.ctx().input(|i| i.pointer.any_pressed() || (i.viewport().focused == Some(true) && !i.events.is_empty()));
                if engaged {
                    app.ui.start_tab = false;
                } else {
                    app.session.active = before;
                }
            };
            if embedded {
                body(ui);
                return;
            }
            let t = crate::theme::Tokens::get();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(t.chrome_dark).inner_margin(egui::Margin::symmetric(4, 2)))
                .show(ui, |ui| body(ui));
            let info = ui.ctx().input(|i| i.viewport().clone());
            if info.close_requested() {
                dock_it = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            if let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) {
                let r = [outer.min.x, outer.min.y, inner.width(), inner.height()];
                if r.iter().zip(rect.iter()).any(|(a, b)| (a - b).abs() > 0.5) {
                    moved = Some(r);
                    // Dragged by its title bar onto the main window's tabs: back in.
                    if tab_bar_screen.is_some_and(|t| t.contains(egui::pos2(outer.center().x, outer.min.y + 10.0))) {
                        dock_it = true;
                    }
                }
            }
        });
        if let Some(w) = app.torn.iter_mut().find(|w| w.uid == uid) {
            w.canvas = canvas;
            if let Some(r) = moved {
                w.rect = r;
            }
        }
        if dock_it {
            dock(app, uid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use serde_json::json;

    fn app_with_two() -> CadApp {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        app.run("line", json!({"points": [[0, 0], [10, 0]]})).unwrap();
        app.run("new", json!({})).unwrap();
        app.run("circle", json!({"center": [0, 0], "radius": 50})).unwrap();
        assert_eq!(app.session.docs.len(), 2);
        app
    }

    #[test]
    fn a_torn_off_drawing_has_its_own_window_view_and_edits() {
        let mut app = app_with_two();
        // Drawing 2 (current) goes to a window: the main window goes back to drawing 1.
        tear_off(&mut app, 1, None).unwrap();
        assert!(is_torn(&app, 1) && !is_torn(&app, 0));
        assert_eq!(app.session.active, 0);
        assert_eq!(main_index(&app), Some(0));
        assert_eq!(app.torn.first().map(|w| w.slot), Some(1));
        tear_off(&mut app, 1, None).unwrap();
        assert_eq!(app.torn.len(), 1, "once");
        // Its view is its own: zooming the window's drawing leaves the main one's view alone.
        let main_view = app.session.docs[0].view();
        app.session.active = 1;
        app.run("zoom", json!({"factor": 4})).unwrap();
        assert_eq!(app.session.docs[0].view(), main_view);
        assert_ne!(app.session.docs[1].view(), main_view);
        // Edits go to the drawing whose window is current.
        app.run("line", json!({"points": [[0, 0], [0, 10]]})).unwrap();
        assert_eq!((app.session.docs[0].doc.model.len(), app.session.docs[1].doc.model.len()), (1, 2));
        // The main window still shows drawing 1 while drawing 2 is current.
        assert_eq!(main_index(&app), Some(0));
        let ctx = egui::Context::default();
        let mut drawn_with = None;
        ctx.run_ui(egui::RawInput::default(), |ui| with_main_doc(&mut app, ui, |app, _| drawn_with = Some(app.session.active)))
            .textures_delta
            .clear();
        assert_eq!(drawn_with, Some(0));
        assert_eq!(app.session.active, 1, "and drawing 2 stays current for the command line");
        // Docked: back among the tabs, current.
        let uid = app.session.docs[1].uid;
        dock(&mut app, uid);
        assert!(app.torn.is_empty() && app.session.active == 1);
    }

    #[test]
    fn windows_are_drawn_and_follow_their_drawings() {
        let mut app = app_with_two();
        tear_off(&mut app, 0, Some(egui::pos2(300.0, 200.0))).unwrap();
        assert_eq!(app.torn.first().map(|w| w.rect), Some([300.0, 200.0, NEW_SIZE[0], NEW_SIZE[1]]));
        // Drawn headless (no OS windows here: egui shows it inside the main one) without a crash.
        let ctx = egui::Context::default();
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| show_windows(&mut app, ui.ctx())).textures_delta.clear();
        }
        assert_eq!(app.torn.len(), 1);
        // Every drawing torn off: the main window says so instead of drawing one.
        tear_off(&mut app, 1, None).unwrap();
        assert_eq!(main_index(&app), None);
        assert!(app.ui.start_tab);
        // A closed drawing's window goes.
        app.session.docs.remove(0);
        app.session.active = 0;
        ctx.run_ui(egui::RawInput::default(), |ui| show_windows(&mut app, ui.ctx())).textures_delta.clear();
        assert_eq!(app.torn.len(), 1);
    }
}
