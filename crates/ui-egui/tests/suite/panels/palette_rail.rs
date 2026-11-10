//! The palette rail: the drawing gets the window, palettes open from a slim strip of icon
//! buttons at its edge one at a time, a pin keeps one docked, the state survives a restart, and
//! commands that open a palette still do. Synthetic drawing only.

use cadaddy_engine::Session;
use cadaddy_ui_egui::docking::{Panel, Workspace};
use cadaddy_ui_egui::{CadApp, Services, UiState, rail};
use egui::{Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

const W: f32 = 1280.0;
/// The window's own edge margin plus the rail: a rail button lies within this of the edge.
const EDGE: f32 = rail::WIDTH + 20.0;

fn harness_with(state: Option<UiState>, rtl: bool) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("rectang", json!({"p1":[30,30],"p2":[170,110]})).unwrap();
    app.run("circle", json!({"center":[100,70],"radius":26})).unwrap();
    if let Some(state) = state {
        app.ui = state;
    }
    app.ui.in_window_menu = false;
    app.integrated_titlebar = true;
    app.direction_override = Some(rtl);
    let mut harness = Harness::builder().with_size(vec2(W, 760.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    harness
}

fn harness() -> Harness<'static, CadApp> {
    harness_with(None, false)
}

fn canvas(h: &Harness<'_, CadApp>) -> Rect {
    h.state().canvas.rect.expect("the drawing area was laid out")
}

/// The rail's button for `label`: the one inside the rail's strip at the window's edge.
fn rail_button(h: &Harness<'_, CadApp>, label: &str, rtl: bool) -> Rect {
    h.get_all_by_label(label)
        .map(|n| n.rect())
        .find(|r| if rtl { r.left() >= W - EDGE } else { r.right() <= EDGE })
        .unwrap_or_else(|| panic!("no rail button {label}"))
}

fn click_rail(h: &mut Harness<'_, CadApp>, label: &str) {
    let rtl = h.state().rtl();
    let r = rail_button(h, label, rtl);
    h.get_all_by_label(label).find(|n| n.rect() == r).unwrap().click();
    h.run_steps(5);
}

fn shown(h: &Harness<'_, CadApp>) -> Vec<Panel> {
    let mut v: Vec<Panel> = h.state().ui.docking.layout.panels().into_iter().copied().filter(|p| *p != Panel::Canvas).collect();
    v.sort_by_key(|p| p.id());
    v
}

/// Before: the default stood the Layers/Properties column open on the right (300 points) and
/// the drawing got what was left. Now the drawing gets the window but for the rail.
#[test]
fn default_layout_gives_the_drawing_the_window() {
    let h = harness();
    assert!(shown(&h).is_empty(), "no palette open by default: {:?}", shown(&h));
    let c = canvas(&h);
    assert!(c.width() >= W - EDGE, "drawing width {} of {W}", c.width());
    assert!(c.left() >= rail::WIDTH, "the drawing starts after the rail: {c:?}");
    // The pre-rail arrangement (kept as a layout of pinned palettes) for comparison.
    let state = UiState { docking: Workspace::docked(), show_palettes: true, show_toolsets: false, ..UiState::default() };
    let old = harness_with(Some(state), false);
    let before = canvas(&old).width();
    eprintln!("drawing width: pre-rail arrangement {before:.0} pt, rail default {:.0} pt (window {W})", c.width());
    assert!(c.width() > before + 250.0, "the drawing gains the column back ({before} -> {})", c.width());
    // Every palette has a labelled rail button.
    for p in Panel::RAIL {
        rail_button(&h, p.label(), false);
    }
}

#[test]
fn rail_button_opens_one_palette_and_again_closes_it() {
    let mut h = harness();
    let full = canvas(&h).width();
    click_rail(&mut h, "Layers");
    assert_eq!(shown(&h), [Panel::Layers]);
    let c = canvas(&h);
    assert!(c.width() < full - 200.0, "the palette stands beside the rail: {c:?}");
    // The palette sits between the rail and the drawing.
    assert!(c.left() > EDGE + 200.0, "{c:?}");
    click_rail(&mut h, "Properties");
    assert_eq!(shown(&h), [Panel::Properties], "only one is open");
    assert!(h.state().ui.show_palettes);
    click_rail(&mut h, "Coordination");
    assert_eq!(shown(&h), [Panel::Coordination]);
    assert!(!h.state().ui.show_palettes, "the View ▸ Palettes switch follows");
    click_rail(&mut h, "Coordination");
    assert!(shown(&h).is_empty());
    assert!((canvas(&h).width() - full).abs() < 1.0, "the drawing gets its width back");
    // Tool Sets is on the rail too and its switch follows.
    click_rail(&mut h, "Tool Sets");
    assert_eq!(shown(&h), [Panel::ToolSets]);
    assert!(h.state().ui.show_toolsets);
}

#[test]
fn hide_button_closes_and_pin_keeps_a_palette_open() {
    let mut h = harness();
    click_rail(&mut h, "Layers");
    h.get_by_label("Hide").click();
    h.run_steps(5);
    assert!(shown(&h).is_empty(), "the hide button puts it back on the rail");
    click_rail(&mut h, "Layers");
    h.get_by_label("Pin").click();
    h.run_steps(4);
    assert_eq!(h.state().ui.docking.pinned, [Panel::Layers]);
    click_rail(&mut h, "Symbols");
    assert_eq!(shown(&h), [Panel::Layers, Panel::Symbols], "a pinned palette stays when another opens");
    click_rail(&mut h, "External References");
    assert_eq!(shown(&h), [Panel::ExternalRefs, Panel::Layers], "the unpinned one gave way");
    // Unpin from the palette's own button: the next one opened replaces it.
    h.get_by_label("Unpin").click();
    h.run_steps(4);
    assert!(h.state().ui.docking.pinned.is_empty());
    click_rail(&mut h, "Properties");
    assert_eq!(shown(&h), [Panel::Properties]);
}

#[test]
fn open_closed_and_pinned_states_persist_across_a_restart() {
    let mut h = harness();
    click_rail(&mut h, "Properties");
    h.get_by_label("Pin").click();
    h.run_steps(4);
    click_rail(&mut h, "Coordination");
    let saved = serde_json::to_string(&h.state().ui).unwrap();
    let expected = serde_json::to_value(&h.state().ui.docking).unwrap();
    let mut again = harness_with(Some(serde_json::from_str(&saved).unwrap()), false);
    assert_eq!(serde_json::to_value(&again.state().ui.docking).unwrap(), expected);
    assert_eq!(shown(&again), [Panel::Coordination, Panel::Properties]);
    assert_eq!(again.state().ui.docking.pinned, [Panel::Properties]);
    // Closed palettes stay closed after a restart too.
    click_rail(&mut again, "Coordination");
    click_rail(&mut again, "Properties");
    let saved = serde_json::to_string(&again.state().ui).unwrap();
    let third = harness_with(Some(serde_json::from_str(&saved).unwrap()), false);
    assert!(shown(&third).is_empty());
    assert_eq!(third.state().ui.docking.pinned, [Panel::Properties], "the pin is kept for next time");
}

/// Commands, menus and the ribbon that open a palette still open and select it, the rail's way.
#[test]
fn explicit_opens_still_open_and_select_their_palette() {
    let mut h = harness();
    let opens: [(&str, Value, Panel); 7] = [
        ("properties", Value::Null, Panel::Properties),
        ("ui.panel.layers", Value::Null, Panel::Layers),
        ("ui.panel.symbols", Value::Null, Panel::Symbols),
        ("ui.dock", json!({"operation":"open", "panel":"coordination"}), Panel::Coordination),
        ("ui.panel.validate", Value::Null, Panel::Validate),
        ("ui.panel.job", Value::Null, Panel::Job),
        ("ui.toggle.toolsets", json!({"on": true}), Panel::ToolSets),
    ];
    for (id, params, panel) in opens {
        h.state_mut().run(id, params).unwrap();
        h.run_steps(4);
        assert!(h.state().ui.docking.showing(panel), "{id} shows {panel:?}");
        assert_eq!(shown(&h), [panel], "{id}: one at a time");
    }
    // Typed at the command line.
    h.state_mut().cmdline("PROPERTIES");
    h.run_steps(4);
    assert!(h.state().ui.docking.showing(Panel::Properties));
    // A palette behind another tab comes to the front.
    h.state_mut().run("ui.dock", json!({"operation":"pin", "panel":"properties", "pinned": true})).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"pin", "panel":"layers", "pinned": true})).unwrap();
    h.state_mut().run("ui.panel.layers", Value::Null).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"move", "panel":"layers", "target":"properties", "zone":"center"})).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"activate", "panel":"properties"})).unwrap();
    h.run_steps(3);
    assert!(!h.state().ui.docking.showing(Panel::Layers));
    h.state_mut().run("ui.panel.layers", Value::Null).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.docking.showing(Panel::Layers));
    assert!(h.state().ui.docking.layout.contains(&Panel::Properties), "pinned: it stays");
}

/// In a right-to-left interface the rail is at the right edge and its palettes open beside it.
#[test]
fn right_to_left_mirrors_the_rail() {
    let mut h = harness_with(None, true);
    h.run_steps(3);
    assert!(h.state().ui.docking.mirrored, "the layout follows the interface's direction");
    let c = canvas(&h);
    assert!(c.right() <= W - rail::WIDTH && c.left() < 20.0, "the rail on the right: {c:?}");
    let r = rail_button(&h, "Layers", true);
    assert!(r.left() >= c.right(), "{r:?}");
    click_rail(&mut h, "Layers");
    assert_eq!(shown(&h), [Panel::Layers]);
    let c = canvas(&h);
    assert!(c.right() < W - EDGE - 200.0 && c.left() < 20.0, "the palette opens between the drawing and the rail: {c:?}");
}

/// Pictures of the rail for review: `RAIL_SHOTS=<folder>` and `--ignored` (needs a wgpu adapter).
#[test]
#[ignore = "writes screenshots on request"]
fn rail_screenshots() {
    let Some(dir) = std::env::var_os("RAIL_SHOTS").map(std::path::PathBuf::from) else { return };
    let build = |state: Option<UiState>, rtl: bool| {
        let mut app = CadApp::new(Session::default(), Services::default());
        let _ = app.run("ui.sample", json!({}));
        if let Some(state) = state {
            app.ui = state;
        }
        app.ui.in_window_menu = false;
        app.integrated_titlebar = true;
        app.direction_override = Some(rtl);
        let mut h = Harness::builder().with_size(vec2(W, 760.0)).wgpu().build_ui_state(
            |ui, app: &mut CadApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(6);
        h
    };
    let save = |h: &mut Harness<'_, CadApp>, name: &str| {
        h.run_steps(4);
        match h.render() {
            Ok(img) => img.save(dir.join(name)).unwrap(),
            Err(e) => eprintln!("rail_screenshots: not rendered ({e})"),
        }
    };
    let state = UiState { docking: Workspace::docked(), show_palettes: true, show_toolsets: false, ..UiState::default() };
    save(&mut build(Some(state), false), "0-before-palette-column.png");
    let mut h = build(None, false);
    save(&mut h, "1-default-rail.png");
    click_rail(&mut h, "Properties");
    save(&mut h, "2-properties-from-rail.png");
    h.get_by_label("Pin").click();
    h.run_steps(4);
    click_rail(&mut h, "Layers");
    save(&mut h, "3-properties-pinned-layers-open.png");
    let mut r = build(None, true);
    click_rail(&mut r, "Layers");
    save(&mut r, "4-right-to-left.png");
}
