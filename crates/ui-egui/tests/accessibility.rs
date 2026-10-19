//! Screen readers: every interactive widget of the app window (tool bar or ribbon, palettes,
//! status bar, command line, layout tabs, Start tab) has a name in the AccessKit tree.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::accesskit::Action;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT;

fn harness(ribbon: bool, start: bool) -> Harness<'static, CadApp> {
    harness_with(ribbon, start, None)
}

fn harness_with(ribbon: bool, start: bool, dialog: Option<&str>) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", serde_json::json!({})).unwrap();
    app.ui.dialog = dialog.map(str::to_string);
    app.ui.start_tab = start;
    app.ui.ribbon = ribbon;
    app.ui.show_palettes = true;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// The interactive nodes without a name: (role, bounds, text near it).
fn unnamed(h: &Harness<'_, CadApp>) -> Vec<String> {
    let root = h.root();
    let mut out = Vec::new();
    for n in root.children_recursive() {
        let a = n.accesskit_node();
        let data = a.data();
        let interactive = data.supports_action(Action::Click) || data.supports_action(Action::Focus);
        // Scroll bars are egui's own (screen readers name them by role); nodes without on-screen
        // bounds aren't shown.
        let shown = a.bounding_box().is_some_and(|b| [b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite()));
        if !interactive || a.is_hidden() || !shown || a.role() == egui::accesskit::Role::ScrollBar {
            continue;
        }
        // egui's own window title bar (dragged to move the window, named by its window): an
        // unnamed area at the very top of its window.
        let window = std::iter::successors(a.parent(), |p| p.parent()).find(|p| p.role() == egui::accesskit::Role::Window);
        let title_bar = a.role() == egui::accesskit::Role::Unknown
            && match (window.and_then(|w| w.bounding_box()), a.bounding_box()) {
                (Some(w), Some(b)) => (b.y0 - w.y0).abs() < 1.0 && b.y1 - b.y0 < 45.0,
                _ => false,
            };
        // Nothing to point at: empty selectable text.
        let empty = a.bounding_box().is_some_and(|b| b.x1 - b.x0 < 0.5 || b.y1 - b.y0 < 0.5);
        if title_bar || empty {
            continue;
        }
        // A text field is announced by its label, its value or its placeholder (hint text).
        let named = a.label().is_some_and(|l| !l.trim().is_empty())
            || a.value().is_some_and(|v| !v.trim().is_empty())
            || a.placeholder().is_some_and(|v| !v.trim().is_empty());
        if !named {
            let near = a.parent().and_then(|p| p.label()).unwrap_or_default();
            out.push(format!("{:?} at {:?} (parent: {near:?} {:?})", a.role(), a.bounding_box(), a.parent().map(|p| p.role())));
        }
    }
    out
}

#[test]
fn every_interactive_widget_has_a_name() {
    let mut problems = Vec::new();
    for (ribbon, start) in [(false, false), (true, false), (false, true)] {
        let h = harness(ribbon, start);
        for p in unnamed(&h) {
            problems.push(format!("ribbon={ribbon} start={start}: {p}"));
        }
    }
    assert!(problems.is_empty(), "{} unnamed:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn dialogs_name_their_controls_too() {
    let mut problems = Vec::new();
    for dialog in ["layers", "dsettings", "units", "options", "qselect", "parameters", "csettings", "plot", "stylesmanager", "about", "language"] {
        let h = harness_with(false, false, Some(dialog));
        for p in unnamed(&h) {
            problems.push(format!("{dialog}: {p}"));
        }
    }
    assert!(problems.is_empty(), "{} unnamed:\n{}", problems.len(), problems.join("\n"));
}

fn press_alt(h: &mut Harness<'static, CadApp>) {
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    h.step();
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
}

fn type_text(h: &mut Harness<'static, CadApp>, t: &str) {
    h.input_mut().events.push(egui::Event::Text(t.into()));
    h.step();
    h.step();
}

#[test]
fn ribbon_key_tips_open_a_tab_and_run_a_button() {
    use cadaddy_ui_egui::keytips::{self, Level};
    let mut h = harness(true, false);
    assert_eq!(keytips::level(&h.ctx), Level::Off);
    // Alt on its own shows the tab letters; I opens the Insert tab.
    press_alt(&mut h);
    assert_eq!(keytips::level(&h.ctx), Level::Tabs);
    type_text(&mut h, "i");
    assert_eq!(h.state().ui.ribbon_tab, "Insert");
    assert_eq!(keytips::level(&h.ctx), Level::Buttons);
    // Esc goes back to the tabs, Alt hides the tips.
    h.key_press(egui::Key::Escape);
    h.step();
    assert_eq!(keytips::level(&h.ctx), Level::Tabs);
    press_alt(&mut h);
    assert_eq!(keytips::level(&h.ctx), Level::Off);
    // Home, then L: the Line button, without the letters reaching the command line.
    press_alt(&mut h);
    type_text(&mut h, "h");
    type_text(&mut h, "l");
    h.run_steps(2);
    let running = h.state().session.running.as_ref().map(|r| r.id.clone());
    assert_eq!(running.as_deref(), Some("line"), "{:?}", h.state().session.log);
    assert_eq!(keytips::level(&h.ctx), Level::Off);
}

#[test]
fn high_contrast_and_interface_size() {
    let mut h = harness(false, false);
    h.state_mut().run("ui.theme", serde_json::json!({ "theme": "high-contrast" })).unwrap();
    h.state_mut().run("ui.scale.150", serde_json::Value::Null).unwrap();
    h.run_steps(3);
    assert!(cadaddy_ui_egui::theme::high_contrast());
    assert_eq!(cadaddy_ui_egui::theme::Tokens::get().text, egui::Color32::WHITE);
    assert!((h.ctx.zoom_factor() - 1.5).abs() < 1e-6);
    // Everything stays named in high contrast too.
    assert!(unnamed(&h).is_empty(), "{:?}", unnamed(&h));
    assert!(h.state_mut().run("ui.scale", serde_json::json!({ "scale": 9 })).is_err());
    h.state_mut().run("ui.theme", serde_json::json!({ "theme": "dark" })).unwrap();
    h.run_steps(2);
    assert!(!cadaddy_ui_egui::theme::high_contrast());
}
