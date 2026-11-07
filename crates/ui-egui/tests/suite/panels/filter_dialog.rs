//! Object Selection Filters: FILTER typed opens the dialog (a JSON call never does); a filter
//! typed into it selects the objects that pass on Apply; Save As keeps it for any drawing and
//! the saved list offers it again; Delete takes it away. `CADADDY_FILTER_SHOTS=<dir>` saves a
//! picture of the dialog there (to look at).

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn app() -> CadApp {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", json!({})).unwrap();
    for (c, r) in [([0, 0], 5), ([20, 0], 1), ([40, 0], 8)] {
        app.run("circle", json!({ "center": c, "radius": r })).unwrap();
    }
    app.run("line", json!({ "points": [[0, 20], [10, 20]] })).unwrap();
    app
}

fn harness(app: CadApp) -> Harness<'static, CadApp> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// Focus the text field of `role` (with that placeholder, when given) and type into it.
fn type_into(h: &mut Harness<'static, CadApp>, role: Role, placeholder: Option<&str>, text: &str) {
    {
        let field = h.get_all_by_role(role).find(|n| placeholder.is_none() || n.accesskit_node().placeholder() == placeholder).unwrap();
        field.focus();
    }
    h.run_steps(2);
    {
        let field = h.get_all_by_role(role).find(|n| placeholder.is_none() || n.accesskit_node().placeholder() == placeholder).unwrap();
        field.type_text(text);
    }
    h.run_steps(2);
}

/// Click the dialog's button `label` (the app has buttons of the same name).
fn click_in_dialog(h: &Harness<'static, CadApp>, label: &str) {
    let window = h.get_by_role_and_label(Role::Window, "Object Selection Filters");
    window.get_by_role_and_label(Role::Button, label).click();
}

#[test]
fn filter_dialog_applies_saves_and_deletes() {
    let dir = std::env::temp_dir().join(format!("cadaddy-filterdlg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Saved filters go to the settings folder: a scratch file for this test.
    cadaddy_engine::cmd::classic::set_saved_filters_file(dir.join("filters.json"));

    let mut app = app();
    app.run("filter", json!({ "filter": "type = Line" })).unwrap();
    assert!(app.ui.dialog.is_none(), "a JSON call never opens the dialog");
    app.session.set_selection(Vec::new());
    app.cmdline("FILTER");
    assert_eq!(app.ui.dialog.as_deref(), Some("filter"));

    let mut h = harness(app);
    assert!(h.query_by_label("Object Selection Filters").is_some());
    // Type a filter and apply it: the two large circles are selected and the dialog closes.
    type_into(&mut h, Role::MultilineTextInput, None, "type = Circle and radius > 2");
    if let Some(d) = std::env::var_os("CADADDY_FILTER_SHOTS") {
        std::fs::create_dir_all(&d).unwrap();
        if let Ok(img) = h.render() {
            img.save(std::path::Path::new(&d).join("filter-dialog.png")).unwrap();
        }
    }
    // Save it first.
    type_into(&mut h, Role::TextInput, Some("Name"), "Big circles");
    click_in_dialog(&h, "Save As");
    h.run_steps(3);
    let saved = h.state_mut().session.execute("filter", &json!({ "list": true })).unwrap();
    assert_eq!(saved["filters"][0]["name"], "Big circles", "{saved}");
    assert_eq!(saved["filters"][0]["filter"], "type = Circle and radius > 2");
    click_in_dialog(&h, "Apply");
    h.run_steps(3);
    assert!(h.state().ui.dialog.is_none(), "Apply closes the dialog");
    assert_eq!(h.state().session.selection().len(), 2);

    // Opened again: the saved filter is offered; Delete removes it.
    h.state_mut().session.set_selection(Vec::new());
    h.state_mut().cmdline("FI");
    h.run_steps(3);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("filter"));
    click_in_dialog(&h, "Delete");
    h.run_steps(3);
    let saved = h.state_mut().session.execute("filter", &json!({ "list": true })).unwrap();
    assert_eq!(saved["filters"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}
