//! The Arabic (right-to-left) interface: the palettes swap sides, the ribbon's tabs and panels and
//! the menu bar run from the right, the command line's history is read from the right, and
//! interface text is put in display order (Arabic letters joined, runs reordered); the drawing
//! itself stays as drawn.

use cadaddy_engine::Session;
use cadaddy_ui_egui::docking::Workspace;
use cadaddy_ui_egui::i18n::{self, Preference};
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

fn layout(w: &Workspace) -> Value {
    serde_json::to_value(&w.layout).unwrap()
}

#[test]
fn arabic_text_is_put_in_display_order() {
    // سطر (line): four letters, joined and read from the right.
    let shown = i18n::rtl_display("سطر");
    assert_eq!(shown.chars().count(), 3);
    assert!(shown.chars().all(|c| ('\u{FE70}'..='\u{FEFF}').contains(&c)), "joined forms: {shown:?}");
    // Mixed: the Latin run keeps its order and goes to the left of the Arabic one; brackets mirror.
    let mixed = i18n::rtl_display("حفظ CADaddy");
    assert!(mixed.starts_with("CADaddy"), "{mixed}");
    assert_eq!(i18n::rtl_display("Line (2)"), "Line (2)", "no Arabic: as it is");
    // In the Arabic interface catalog texts and formatted ones come out in display order.
    let logical = i18n::tr("ar", "Save");
    assert_ne!(logical, "Save");
    i18n::with_language("ar", || {
        assert_eq!(i18n::t("Save"), i18n::rtl_display(logical));
        assert!(i18n::rtl());
    });
    i18n::with_language("en", || assert_eq!(i18n::t("Save"), "Save"));
}

#[test]
fn the_palettes_swap_sides_and_back() {
    let w = Workspace::default();
    let m = w.mirrored();
    assert_ne!(layout(&w), layout(&m));
    assert_eq!(layout(&m.mirrored()), layout(&w), "mirroring twice is the original");
    // The default: Tool Sets on the left, Layers and Properties on the right. Mirrored: the
    // first (left) side is the Layers/Properties stack, sized as before.
    let root = layout(&m)["root"].clone();
    let split = &root["Split"];
    assert_eq!(split["size"], json!({ "FixedSecond": 220.0 }), "{split}");
    assert_eq!(split["second"]["Tabs"]["panels"], json!(["toolSets"]));
    assert_eq!(split["first"]["Split"]["size"], json!({ "FixedFirst": 300.0 }));
    assert!(split["first"]["Split"]["first"]["Stack"].is_object(), "the stack on the left");
}

fn harness(rtl: bool) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", json!({})).unwrap();
    app.ui.ribbon = true;
    app.ui.show_palettes = true;
    app.direction_override = Some(rtl);
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

fn home_tab_x(h: &Harness<'_, CadApp>) -> f64 {
    h.get_by_label("Home").accesskit_node().bounding_box().map_or(0.0, |b| b.x0)
}

#[test]
fn the_window_is_mirrored_right_to_left() {
    let ltr = harness(false);
    let default_layout = layout(&ltr.state().ui.docking);
    assert!(home_tab_x(&ltr) < 300.0, "Home tab at the left");

    let mut h = harness(true);
    assert!(h.state().ui.layout_rtl);
    assert_eq!(layout(&h.state().ui.docking), layout(&ltr.state().ui.docking.mirrored()), "the palettes on the other side");
    assert!(home_tab_x(&h) > 1000.0, "Home tab at the right: {}", home_tab_x(&h));

    // Back to left to right: the layout as it was.
    h.state_mut().direction_override = Some(false);
    h.run_steps(3);
    assert!(!h.state().ui.layout_rtl);
    assert_eq!(layout(&h.state().ui.docking), default_layout);
}

/// Screenshots of the Arabic interface for the task's record: `CADADDY_RTL_SHOTS=dir`.
#[test]
fn screenshots_of_the_arabic_interface() {
    let Ok(dir) = std::env::var("CADADDY_RTL_SHOTS") else { return };
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", json!({})).unwrap();
    app.run("line", json!({ "points": [[0, 0], [100, 50]] })).unwrap();
    app.ui.ribbon = true;
    app.ui.show_palettes = true;
    app.ui.interface_language = Preference::Arabic;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    cadaddy_ui_egui::theme::install_fonts(&h.ctx, "ar", false);
    h.run_steps(6);
    let img = h.render().unwrap();
    img.save(std::path::Path::new(&dir).join("rtl-arabic.png")).unwrap();
}
