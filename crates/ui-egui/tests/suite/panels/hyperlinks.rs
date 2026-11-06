//! Hyperlinks in the app: HYPERLINK opens the Insert Hyperlink dialog (selecting first when
//! nothing is selected), a linked object's tooltip, and Ctrl+click (⌘-click) following its link.

use cadaddy_engine::Session;
use cadaddy_engine::geom::Vec2;
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};

fn harness(app: CadApp) -> Harness<'static, CadApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn drawn() -> CadApp {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.run("line", json!({ "points": [[0, 0], [20, 0]] })).unwrap();
    app.run("circle", json!({ "center": [10, 10], "radius": 3 })).unwrap();
    app.session.execute("view.set", &json!({ "center": [10, 5], "height": 30 })).unwrap();
    app.canvas.zoom_pending = false;
    app
}

fn handles(app: &CadApp) -> Vec<String> {
    app.session.doc().unwrap().model.handles().iter().map(|h| h.hex()).collect()
}

#[test]
fn hyperlink_opens_the_dialog_selecting_first_when_needed() {
    let mut app = drawn();
    // Nothing selected: HYPERLINK selects, then the dialog opens for what was picked.
    app.run("hyperlink", Value::Null).unwrap();
    assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("hyperlink.pick"));
    assert_eq!(app.ui.dialog, None);
    // "Last" picks the circle; Enter ends the selection.
    app.session.input(cadaddy_engine::Input::Keyword("Last".into())).unwrap();
    app.session.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(app.session.running.is_none());
    assert_eq!(app.session.selection().len(), 1);
    let mut h = harness(app);
    h.run_steps(2);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("hyperlink"));
    h.run_steps(2);
    // The dialog shows (Insert Hyperlink: the line has no link yet).
    let text = format!("{:?}", h.output().shapes);
    assert!(text.contains("Insert Hyperlink"), "the dialog's title");
    // Selected already: straight to the dialog; a linked object makes it the edit dialog.
    let mut app = drawn();
    let hs = handles(&app);
    app.run("hyperlink", json!({ "handles": [hs[1]], "url": "https://example.com", "description": "Example" })).unwrap();
    app.session.set_selection(vec![app.session.doc().unwrap().model.handles()[1]]);
    app.run("hyperlink", Value::Null).unwrap();
    assert_eq!(app.ui.dialog.as_deref(), Some("hyperlink"));
    let mut h = harness(app);
    h.run_steps(2);
    let text = format!("{:?}", h.output().shapes);
    assert!(text.contains("Edit Hyperlink") && text.contains("Remove Link"), "editing a link");
}

fn open_urls(h: &Harness<'_, CadApp>) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn ctrl_click_follows_the_link_and_a_plain_click_selects() {
    let mut app = drawn();
    let hs = handles(&app);
    app.run("hyperlink", json!({ "handles": [hs[0]], "url": "https://example.com/spec" })).unwrap();
    app.run("hyperlink", json!({ "handles": [hs[1]], "target": "Layout1" })).unwrap();
    let mut h = harness(app);
    let at = |h: &Harness<'_, CadApp>, p: Vec2| -> Pos2 { h.state().canvas.xf.unwrap().to_screen(p) };
    let click = |h: &mut Harness<'_, CadApp>, pos: Pos2, modifiers: Modifiers| -> Vec<String> {
        // The modifiers held, as the window reports them (not only on the click events).
        h.event(Event::ModifiersChanged(modifiers));
        h.event(Event::PointerMoved(pos));
        h.run_steps(3);
        let mut urls = Vec::new();
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
            h.run_steps(1);
            urls.extend(open_urls(h));
        }
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(1);
        urls
    };
    // A plain click on the linked line selects it.
    let line = at(&h, Vec2::new(5.0, 0.0));
    assert!(click(&mut h, line, Modifiers::NONE).is_empty());
    assert_eq!(h.state().session.selection().len(), 1);
    h.state_mut().session.set_selection(Vec::new());
    // Ctrl+click (⌘-click) opens its URL and selects nothing.
    let urls = click(&mut h, line, Modifiers::COMMAND);
    assert_eq!(urls, ["https://example.com/spec"]);
    assert!(h.state().session.selection().is_empty());
    // A link to a layout of this drawing goes there.
    let circle = at(&h, Vec2::new(13.0, 10.0));
    let urls = click(&mut h, circle, Modifiers::COMMAND);
    assert!(urls.is_empty());
    assert_eq!(h.state_mut().session.execute("getvar", &json!({ "name": "CTAB" })).map(|v| v["value"].clone()).ok(), Some(json!("Layout1")));
}
