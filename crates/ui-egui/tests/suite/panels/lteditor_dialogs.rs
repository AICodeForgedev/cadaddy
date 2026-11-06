//! The Linetype Editor and Hatch Pattern Editor (Q152) open from the Format menu, preview what
//! they define and apply it. `CADADDY_LTEDIT_SHOTS=<dir>` saves the renders (to look at).

use cadaddy_ui_egui::{CadApp, Services, theme};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn harness(dialog: &'static str, dark: bool) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(cadaddy_engine::Session::new(), Services::default());
    app.run(dialog, serde_json::Value::Null).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(640.0, 560.0)).wgpu().build_ui_state(
        move |ui, app: &mut CadApp| {
            theme::apply(ui.ctx(), if dark { egui::Theme::Dark } else { egui::Theme::Light }, false);
            cadaddy_ui_egui::dialogs::show(app, ui.ctx());
        },
        app,
    );
    h.run_steps(4);
    h
}

#[test]
fn both_editors_render_light_and_dark() {
    for (dialog, id) in [("ltypeeditor", cadaddy_ui_egui::lteditor::LINETYPE), ("pateditor", cadaddy_ui_egui::lteditor::PATTERN)] {
        for dark in [false, true] {
            let mut h = harness(dialog, dark);
            assert_eq!(h.state().ui.dialog.as_deref(), Some(id), "{dialog} stays open");
            if let Some(dir) = std::env::var_os("CADADDY_LTEDIT_SHOTS") {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                h.render().unwrap().save(dir.join(format!("{dialog}-{}.png", if dark { "dark" } else { "light" }))).unwrap();
            }
        }
    }
}

#[test]
fn apply_defines_the_linetype_and_the_pattern() {
    let mut h = harness("ltypeeditor", false);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let d = h.state().session.doc().unwrap();
    let lt = d.linetype("MY_LINETYPE").expect("defined");
    assert_eq!(lt.pattern[1].shape, Some(2), "the circle from cadaddy.shx: {:?}", lt.pattern);
    let mut h = harness("pateditor", false);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(cadaddy_engine::pattern_choices().iter().any(|c| c.name == "MY_PATTERN" && c.lines.len() == 2));
}
