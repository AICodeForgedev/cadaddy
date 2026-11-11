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
fn minimize_button_closes_and_pin_keeps_a_palette_open() {
    let mut h = harness();
    click_rail(&mut h, "Layers");
    h.get_by_label(cadaddy_ui_egui::docking::MINIMIZE).click();
    h.run_steps(5);
    assert!(shown(&h).is_empty(), "the minimize button puts it back on the rail");
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

/// The settings file's workspace object as a release before this one wrote it: `docking` as the
/// palettes stood (`rail` 1: the rail's first release, which kept them pinned open; `None`:
/// before the rail, no `rail`, `pinned` or `mirrored` field), and both switches on.
fn old_settings(docking: &Workspace, rail: Option<u32>) -> Value {
    let mut d = serde_json::to_value(docking).unwrap();
    let o = d.as_object_mut().unwrap();
    for k in ["rail", "pinned", "mirrored"] {
        o.remove(k);
    }
    if let Some(rail) = rail {
        let pins: Vec<Value> = docking.layout.panels().into_iter().filter(|p| **p != Panel::Canvas).map(|p| json!(p.id())).collect();
        o.insert("rail".into(), json!(rail));
        o.insert("pinned".into(), json!(pins));
    }
    json!({ "docking": d, "showToolsets": true, "showPalettes": true, "toolsetTab": "Drafting" })
}

/// A settings file's workspace read back the way the app reads it at start: the dock layout
/// recovered on its own, the rest strictly.
fn read_settings(workspace: &Value) -> UiState {
    let mut fields = workspace.clone();
    let docking = fields.as_object_mut().unwrap().remove("docking").unwrap();
    let mut ui: UiState = serde_json::from_value(fields).unwrap();
    let docking: Workspace = serde_json::from_value(docking).unwrap();
    docking.validate().unwrap();
    ui.docking = docking;
    ui
}

/// Coordination, Layers, Properties and the Sheet Set Manager docked open, as the releases
/// before this one saved them (made up here: each release's layout shape).
fn saved_by_older_releases() -> Vec<(&'static str, Value)> {
    let mut v = Vec::new();
    // Before the rail: the palette column on the right, Coordination and the Sheet Set Manager
    // docked beside the drawing.
    let mut w = Workspace::docked();
    w.set_visible(Panel::Coordination, true).unwrap();
    w.set_visible(Panel::SheetSets, true).unwrap();
    v.push(("0.7.8", old_settings(&w, None)));
    // Coordination a tab behind Properties, the Sheet Set Manager floating.
    let mut w = Workspace::docked();
    w.set_visible(Panel::Coordination, true).unwrap();
    w.apply(craft_ui::docking::Action::Move {
        panel: Panel::Coordination,
        anchor: Panel::Properties,
        placement: craft_ui::docking::Placement::Tab { before: None },
    })
    .unwrap();
    w.set_visible(Panel::SheetSets, true).unwrap();
    w.apply(craft_ui::docking::Action::Float { panel: Panel::SheetSets, rect: [700.0, 90.0, 380.0, 520.0] }).unwrap();
    v.push(("0.7.9", old_settings(&w, None)));
    // A pre-rail 0.7.10: External References and Symbols out too, Tool Sets closed.
    let mut w = Workspace::docked();
    for p in [Panel::Coordination, Panel::SheetSets, Panel::ExternalRefs] {
        w.set_visible(p, true).unwrap();
    }
    w.set_visible(Panel::ToolSets, false).unwrap();
    v.push(("0.7.10 before the rail", old_settings(&w, None)));
    // The rail's first release, after its upgrade had pinned that layout open.
    v.push(("0.7.10 first rail", old_settings(&w, Some(1))));
    v
}

/// A made-up building model with print sheets, saved as .cdy.
fn bim_cdy_with_sheets(dir: &std::path::Path) -> String {
    let mut d = cadaddy_engine::sample::job_print_set(3, 40);
    let bim = cadaddy_engine::sample::bim_building(2, 3);
    for l in &bim.layers {
        if !d.layers.iter().any(|m| m.name.eq_ignore_ascii_case(&l.name)) {
            d.layers.push(l.clone());
        }
    }
    for e in bim.model.iter() {
        let mut e = (**e).clone();
        e.handle = d.new_handle();
        d.model.push(e);
    }
    let path = dir.join("Sample Building.cdy");
    std::fs::write(&path, cadaddy_io::cdy::write_cdy(&d).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

/// Open `path` the way a double-clicked file or File ▸ Open does (read in the background), and
/// wait for it to open on its first sheet.
fn open_file(h: &mut Harness<'_, CadApp>, path: &str) {
    h.state_mut().open_path(path);
    let t = std::time::Instant::now();
    while h.state().session.active_sheet().is_none() && t.elapsed() < std::time::Duration::from_secs(60) {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().session.active_sheet().is_some(), "{path} opened on a sheet: {:?}", h.state().status);
    h.run_steps(8);
}

/// The test that failed before the fix (owner, Q6524): settings saved by an older release with
/// Coordination, Layers, Properties and the Sheet Set Manager docked open came back pinned open
/// on the right at every start. Now: only the drawing and the rail; opening a building model
/// with sheets opens no palette; Coordination opened from the rail minimizes back onto it; after
/// a restart everything is still closed.
#[test]
fn every_palette_starts_on_the_rail_whatever_an_older_release_saved() {
    let dir = std::env::temp_dir().join(format!("cadaddy-rail-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = bim_cdy_with_sheets(&dir);
    for (release, saved) in saved_by_older_releases() {
        let mut h = harness_with(Some(read_settings(&saved)), false);
        assert!(shown(&h).is_empty(), "{release}: palettes open at start: {:?}", shown(&h));
        assert!(h.state().ui.docking.pinned.is_empty(), "{release}: {:?}", h.state().ui.docking.pinned);
        let c = canvas(&h);
        assert!(c.left() >= rail::WIDTH && c.left() < EDGE && c.right() > W - 20.0, "{release}: the drawing gets the window: {c:?}");
        for label in ["Coordination", "Layers", "Properties", "Sheet Set Manager"] {
            rail_button(&h, label, false);
        }
        // A building model with sheets opens on its first sheet; no palette opens with it.
        open_file(&mut h, &model);
        assert!(shown(&h).is_empty(), "{release}: opening the model opened {:?}", shown(&h));
        // Opened from the rail and minimized again: back on the rail.
        click_rail(&mut h, "Coordination");
        assert_eq!(shown(&h), [Panel::Coordination], "{release}");
        h.get_by_label(cadaddy_ui_egui::docking::MINIMIZE).click();
        h.run_steps(5);
        assert!(shown(&h).is_empty(), "{release}: minimized onto the rail");
        // Restart: the rail state was saved, so still nothing open.
        let saved = serde_json::to_string(&h.state().ui).unwrap();
        let again = harness_with(Some(serde_json::from_str(&saved).unwrap()), false);
        assert!(shown(&again).is_empty(), "{release}: after a restart {:?}", shown(&again));
        assert_eq!(again.state().ui.docking.rail, cadaddy_ui_egui::docking::RAIL_LAYOUT);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// In a right-to-left interface an older layout goes to the rail on the right, and the minimize
/// arrow points to it.
#[test]
fn right_to_left_older_layouts_go_to_the_rail_on_the_right() {
    let (_, saved) = saved_by_older_releases().remove(0);
    let mut state = read_settings(&saved);
    state.layout_rtl = true;
    let mut h = harness_with(Some(state), true);
    assert!(shown(&h).is_empty(), "{:?}", shown(&h));
    click_rail(&mut h, "Layers");
    let c = canvas(&h);
    assert!(c.right() < W - EDGE - 200.0, "Layers opens beside the rail on the right: {c:?}");
    let arrow = h.get_by_label(cadaddy_ui_egui::docking::MINIMIZE).rect();
    assert!(arrow.left() > c.right(), "the minimize arrow is in the palette by the rail: {arrow:?}");
    h.get_by_label(cadaddy_ui_egui::docking::MINIMIZE).click();
    h.run_steps(5);
    assert!(shown(&h).is_empty());
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
    // Settings an older release saved with Coordination, Layers, Properties and the Sheet Set
    // Manager docked open: the start, then a building model with sheets opened.
    let older = saved_by_older_releases().pop().map(|(_, v)| read_settings(&v));
    let mut o = build(older, false);
    save(&mut o, "5-older-settings-start.png");
    let scratch = std::env::temp_dir().join(format!("cadaddy-rail-shots-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    open_file(&mut o, &bim_cdy_with_sheets(&scratch));
    save(&mut o, "6-older-settings-model-opened.png");
    let _ = std::fs::remove_dir_all(&scratch);
}
