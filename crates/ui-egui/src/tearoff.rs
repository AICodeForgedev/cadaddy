//! Tear-off drawing windows: a drawing tab dragged out of the tab bar (or "Move to New Window" on
//! its menu) opens in an OS window of its own (an egui viewport; on the web, a window inside the
//! page), with its own canvas and view — several drawings side by side, across monitors. The
//! palettes and the command line stay in the main window and act on the drawing whose window was
//! last clicked or focused. "Dock", closing the window, or dragging it back onto the main window's
//! tab bar puts the drawing back among the tabs.

use std::sync::atomic::{AtomicU64, Ordering};

use egui::{Rect, vec2};

use crate::CadApp;
use crate::canvas::CanvasState;

/// Monotonic counter for unique window identifiers.
static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

fn next_window_id() -> u64 {
    NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed)
}

/// A drawing in a window of its own.
pub struct TornWindow {
    /// The drawing ([`cadaddy_engine::DocState::uid`]).
    pub uid: u64,
    /// Unique id for this window (distinct even when two windows show the same drawing).
    pub window_id: u64,
    /// Its window: outer top-left and inner size, in screen points.
    pub rect: [f32; 4],
    /// Its GPU layer set ([`crate::gpu::with_canvas_slot`]); 0 is the main window's.
    pub slot: usize,
    /// When true the drawing also stays in the main tab (NEWWINDOW): a second view, not a move.
    pub pinned: bool,
    /// This window's own view per space (independent of the drawing's view and other windows').
    views: Vec<(cadaddy_doc::Space, cadaddy_engine::View)>,
    /// The space this window is showing.
    space: cadaddy_doc::Space,
    canvas: CanvasState,
}

/// Default size of a new drawing window, points.
const NEW_SIZE: [f32; 2] = [900.0, 640.0];

fn index_of(app: &CadApp, uid: u64) -> Option<usize> {
    app.session.docs.iter().position(|d| d.uid == uid)
}

/// Whether drawing `index` has been moved to a window of its own (not just an extra view).
pub fn is_torn(app: &CadApp, index: usize) -> bool {
    app.session.docs.get(index).is_some_and(|d| app.torn.iter().any(|w| w.uid == d.uid && !w.pinned))
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
    let doc = app.session.docs.get(index).ok_or("no such drawing")?;
    let uid = doc.uid;
    let views = doc.views.clone();
    let space = doc.space.clone();
    if app.torn.iter().any(|w| w.uid == uid && !w.pinned) {
        return Ok(());
    }
    let slot = (1..crate::gpu::MAX_CANVASES).find(|s| !app.torn.iter().any(|w| w.slot == *s)).ok_or("too many drawing windows open")?;
    let mut canvas = CanvasState::default();
    canvas.gpu = app.canvas.gpu;
    canvas.gpu_device = app.canvas.gpu_device.clone();
    let n = app.torn.len() as f32;
    let at = at.unwrap_or(egui::pos2(120.0 + 30.0 * n, 120.0 + 30.0 * n));
    let window_id = next_window_id();
    app.torn.push(TornWindow { uid, window_id, rect: [at.x, at.y, NEW_SIZE[0], NEW_SIZE[1]], slot, pinned: false, views, space, canvas });
    // The main window goes on with another drawing (or the Start tab when none is left).
    if app.session.active == index {
        match main_index(app) {
            Some(i) => app.session.active = i,
            None => app.ui.start_tab = true,
        }
    }
    Ok(())
}

/// Open a second window on drawing `index`, which stays in the main tab too (NEWWINDOW). Each
/// call adds another window; edits in any of them go to the same drawing.
pub fn new_window(app: &mut CadApp, index: usize) -> Result<(), String> {
    let doc = app.session.docs.get(index).ok_or("no such drawing")?;
    let uid = doc.uid;
    let views = doc.views.clone();
    let space = doc.space.clone();
    let slot = (1..crate::gpu::MAX_CANVASES).find(|s| !app.torn.iter().any(|w| w.slot == *s)).ok_or("too many drawing windows open")?;
    let mut canvas = CanvasState::default();
    canvas.gpu = app.canvas.gpu;
    canvas.gpu_device = app.canvas.gpu_device.clone();
    let n = app.torn.len() as f32;
    let at = egui::pos2(120.0 + 30.0 * n, 120.0 + 30.0 * n);
    let window_id = next_window_id();
    app.torn.push(TornWindow { uid, window_id, rect: [at.x, at.y, NEW_SIZE[0], NEW_SIZE[1]], slot, pinned: true, views, space, canvas });
    Ok(())
}

/// Put drawing `uid` back among the main window's tabs, current (removes all its windows).
pub fn dock(app: &mut CadApp, uid: u64) {
    app.torn.retain(|w| w.uid != uid);
    if let Some(i) = index_of(app, uid) {
        app.session.active = i;
        app.ui.start_tab = false;
    }
}

/// Close a single window by its window id. If this was the last non-pinned window for that
/// drawing, the drawing returns to the main window's tabs.
pub fn dock_one(app: &mut CadApp, window_id: u64) {
    let info = app.torn.iter().find(|w| w.window_id == window_id).map(|w| (w.uid, w.pinned));
    app.torn.retain(|w| w.window_id != window_id);
    if let Some((uid, was_pinned)) = info
        && !was_pinned
        && !app.torn.iter().any(|w| w.uid == uid && !w.pinned)
        && let Some(i) = index_of(app, uid)
    {
        app.session.active = i;
        app.ui.start_tab = false;
    }
}

/// The [`egui::ViewportId`] for a torn window.
pub(crate) fn viewport_id(w: &TornWindow) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("cadaddy-window", w.window_id))
}

/// Install window `k`'s view and space on its drawing, run `f`, then restore the drawing's
/// own view and space. The window's (possibly changed) view is written back. Used by
/// [`show_windows`] for each frame.
pub(crate) fn with_window_view<R>(app: &mut CadApp, k: usize, f: impl FnOnce(&mut CadApp, usize) -> R) -> Option<R> {
    let w = app.torn.get_mut(k)?;
    let uid = w.uid;
    let wid = w.window_id;
    let mut win_views = std::mem::take(&mut w.views);
    let mut win_space = std::mem::take(&mut w.space);
    let Some(index) = index_of(app, uid) else {
        // Doc gone — put views/space back.
        if let Some(w) = app.torn.get_mut(k) {
            w.views = win_views;
            w.space = win_space;
        }
        return None;
    };
    if let Some(doc) = app.session.docs.get_mut(index) {
        std::mem::swap(&mut doc.views, &mut win_views);
        std::mem::swap(&mut doc.space, &mut win_space);
    }
    let result = f(app, index);
    if let Some(idx) = index_of(app, uid)
        && let Some(doc) = app.session.docs.get_mut(idx)
    {
        std::mem::swap(&mut doc.views, &mut win_views);
        std::mem::swap(&mut doc.space, &mut win_space);
    }
    if let Some(w) = app.torn.iter_mut().find(|w| w.window_id == wid) {
        w.views = win_views;
        w.space = win_space;
    }
    Some(result)
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
        let id = viewport_id(w);
        let (uid, wid, rect, slot) = (w.uid, w.window_id, w.rect, w.slot);
        let mut canvas = std::mem::take(&mut w.canvas);
        let Some(index) = index_of(app, uid) else { continue };
        let title = app.session.docs.get(index).map(|d| format!("{}{} - CADaddy", d.title, if d.is_dirty() { "*" } else { "" })).unwrap_or_default();
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
                // Install this window's view and space, draw, then restore.
                with_window_view(app, k, |app, _| {
                    std::mem::swap(&mut app.canvas, &mut canvas);
                    crate::gpu::with_canvas_slot(slot, || crate::canvas::show(app, ui));
                    std::mem::swap(&mut app.canvas, &mut canvas);
                });
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
        if let Some(w) = app.torn.iter_mut().find(|w| w.window_id == wid) {
            w.canvas = canvas;
            if let Some(r) = moved {
                w.rect = r;
            }
        }
        if dock_it {
            dock_one(app, wid);
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
    fn newwindow_opens_a_second_view_while_the_drawing_stays_in_the_main_tab() {
        let mut app = app_with_two();
        // NEWWINDOW on the active drawing: a window opens, but the drawing stays in the main tab.
        new_window(&mut app, 1).unwrap();
        assert!(!is_torn(&app, 1), "pinned: not torn — stays in tabs");
        assert_eq!(app.session.active, 1, "the drawing stays active");
        assert_eq!(main_index(&app), Some(1), "still the main window's drawing");
        assert_eq!(app.torn.len(), 1);
        assert!(app.torn[0].pinned);
        // A second NEWWINDOW: another window on the same drawing.
        new_window(&mut app, 1).unwrap();
        assert_eq!(app.torn.len(), 2);
        // Edits go to the drawing (both windows share it).
        app.run("line", json!({"points": [[0, 0], [5, 5]]})).unwrap();
        assert_eq!(app.session.docs[1].doc.model.len(), 2);
        // "Dock All" closes all windows for that drawing at once.
        let uid = app.session.docs[1].uid;
        dock(&mut app, uid);
        assert!(app.torn.is_empty());
    }

    /// MUST 1: each window keeps its own view; zoom/pan in one does not affect the others or
    /// the main tab. Goes through the same install/restore path that [`show_windows`] uses.
    #[test]
    fn newwindow_views_are_independent() {
        let mut app = app_with_two();
        // Two NEWWINDOW on docs[1].
        new_window(&mut app, 1).unwrap();
        new_window(&mut app, 1).unwrap();
        assert_eq!(app.torn.len(), 2);

        // Snapshot before any zoom.
        let main_view_before = app.session.docs[1].view();
        let b_views_before = app.torn[1].views.clone();

        // With window A's view installed (the path show_windows takes), zoom.
        with_window_view(&mut app, 0, |app, index| {
            app.session.active = index;
            app.run("zoom", json!({"factor": 4})).unwrap();
        })
        .expect("window A install/restore");

        // Window A's stored view changed.
        assert_ne!(app.torn[0].views, b_views_before, "window A's view changed after zoom");
        // Window B's stored views are exactly the same as before.
        assert_eq!(app.torn[1].views, b_views_before, "window B's view unchanged");
        // The main tab's view is exactly the same as before.
        assert_eq!(app.session.docs[1].view(), main_view_before, "main tab view unchanged");

        // With window B installed, add a line; the edit lands in the model.
        with_window_view(&mut app, 1, |app, index| {
            app.session.active = index;
            app.run("line", json!({"points": [[1, 1], [2, 2]]})).unwrap();
        })
        .expect("window B install/restore");
        assert_eq!(app.session.docs[1].doc.model.len(), 2, "edit through window B visible in model");
    }

    /// MUST 2: every TornWindow has a unique window_id used for ViewportId and canvas handback;
    /// two new_window on one drawing plus a tear_off on another all coexist with distinct state.
    #[test]
    fn window_ids_are_unique_across_windows() {
        let mut app = app_with_two();
        new_window(&mut app, 1).unwrap();
        new_window(&mut app, 1).unwrap();
        tear_off(&mut app, 0, None).unwrap();
        assert_eq!(app.torn.len(), 3);

        // Give each window a distinct rect, views entry and canvas marker.
        app.torn[0].rect = [100.0, 100.0, 800.0, 600.0];
        app.torn[1].rect = [200.0, 200.0, 700.0, 500.0];
        app.torn[2].rect = [300.0, 300.0, 600.0, 400.0];
        app.torn[0].views[0].1.height = 10.0;
        app.torn[1].views[0].1.height = 20.0;
        app.torn[2].views[0].1.height = 30.0;
        app.torn[0].canvas.mesh_ms = 1.0;
        app.torn[1].canvas.mesh_ms = 2.0;
        app.torn[2].canvas.mesh_ms = 3.0;

        let ids: Vec<u64> = app.torn.iter().map(|w| w.window_id).collect();
        assert_ne!(ids[0], ids[1], "two NEWWINDOW on the same drawing have distinct ids");
        assert_ne!(ids[1], ids[2]);
        assert_ne!(ids[0], ids[2]);

        // Run show_windows headless for 2 frames.
        let ctx = egui::Context::default();
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| show_windows(&mut app, ui.ctx())).textures_delta.clear();
        }
        assert_eq!(app.torn.len(), 3, "all three windows survive show_windows");

        // Each window (looked up by window_id) still has exactly its own rect, views and
        // canvas marker — proves hand-back is keyed by window_id, not uid.
        for (i, &wid) in ids.iter().enumerate() {
            let w = app.torn.iter().find(|w| w.window_id == wid).unwrap_or_else(|| panic!("window {i} missing"));
            let expected_height = [10.0, 20.0, 30.0][i];
            assert_eq!(w.views[0].1.height, expected_height, "window {i} views preserved");
            let expected_rect = [[100.0_f32, 100.0, 800.0, 600.0], [200.0, 200.0, 700.0, 500.0], [300.0, 300.0, 600.0, 400.0]][i];
            assert_eq!(w.rect, expected_rect, "window {i} rect preserved");
            assert_eq!(w.canvas.mesh_ms, (i + 1) as f64, "window {i} canvas preserved");
        }

        // ViewportIds taken from the function show_windows uses (not rebuilt in the test).
        let vids: Vec<egui::ViewportId> = app.torn.iter().map(viewport_id).collect();
        assert_ne!(vids[0], vids[1], "viewports 0 and 1 distinct");
        assert_ne!(vids[1], vids[2], "viewports 1 and 2 distinct");
        assert_ne!(vids[0], vids[2], "viewports 0 and 2 distinct");
    }

    /// MUST 3: closing one window removes only that window; the other remains.
    #[test]
    fn dock_one_removes_only_the_target_window() {
        let mut app = app_with_two();
        new_window(&mut app, 1).unwrap();
        new_window(&mut app, 1).unwrap();
        assert_eq!(app.torn.len(), 2);
        let wid_a = app.torn[0].window_id;
        let wid_b = app.torn[1].window_id;
        // Close window A.
        dock_one(&mut app, wid_a);
        assert_eq!(app.torn.len(), 1, "only one window removed");
        assert_eq!(app.torn[0].window_id, wid_b, "window B survives");
        // The drawing is still in the main tab (pinned windows).
        assert!(!is_torn(&app, 1));
        assert_eq!(main_index(&app), Some(1));
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
