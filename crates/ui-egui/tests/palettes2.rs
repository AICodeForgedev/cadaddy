//! The Blocks palette (insertion options, repeat placement, recent, favourites, a library
//! drawing inserted as a block, drag-insert) and tool palettes (tools from the drawing's objects,
//! using them, kept with the interface state, export and import).

use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Circle, Drawing, Entity, EntityKind, Handle};
use cadaddy_engine::{Input, Session, sysvars};
use cadaddy_ui_egui::blockspalette::{self, BlockRef};
use cadaddy_ui_egui::toolpalettes::{self, Tool};
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::{Value, json};

/// Every library "drawing" reads as one circle of radius 2.
fn hooks() {
    set_io(IoHooks {
        read: |_, _| {
            let mut d = Drawing::new_imperial();
            d.model.push(Entity::new(Handle(0x30), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 2.0 })));
            Ok(d)
        },
        write: |_, _| Err("no writer".into()),
        plot: None,
        read_pdf: None,
    });
}

fn app() -> CadApp {
    let mut app = CadApp::new(Session::new(), Services::default());
    let c = app.session.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let h = c["handle"].as_str().unwrap().to_string();
    app.session.execute("block", &json!({ "name": "TAG", "base": [0, 0], "handles": [h], "keep": "delete" })).unwrap();
    app
}

fn inserts(app: &CadApp) -> Vec<cadaddy_engine::doc::Insert> {
    app.session.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.clone()) } else { None }).collect()
}

#[test]
fn drag_insert_creates_the_reference_with_the_options() {
    hooks();
    let mut app = app();
    app.ui.blocks_palette.scale = 2.0;
    app.ui.blocks_palette.rotation = 30.0;
    let tag = BlockRef { name: "TAG".into(), file: None };
    blockspalette::insert_at(&mut app, &tag, [5.0, 6.0]).unwrap();
    let i = inserts(&app);
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].block, "TAG");
    assert!((i[0].insert.x - 5.0).abs() < 1e-9 && (i[0].scale.x - 2.0).abs() < 1e-9 && (i[0].rotation - 30f64.to_radians()).abs() < 1e-9);
    assert_eq!(app.ui.blocks_palette.recent, std::slice::from_ref(&tag));

    // A library drawing comes in as a block named after the file.
    let lib = BlockRef { name: "Valve".into(), file: Some(std::env::temp_dir().join("Valve.dxf").to_string_lossy().into_owned()) };
    std::fs::write(lib.file.as_ref().unwrap(), "x").unwrap();
    blockspalette::insert_at(&mut app, &lib, [0.0, 0.0]).unwrap();
    assert!(app.session.doc().unwrap().block("Valve").is_some());
    assert_eq!(app.ui.blocks_palette.recent.first(), Some(&lib), "most recent first");

    // Favourites toggle.
    blockspalette::toggle_favourite(&mut app, &tag);
    assert_eq!(app.ui.blocks_palette.favourites, std::slice::from_ref(&tag));
    blockspalette::toggle_favourite(&mut app, &tag);
    assert!(app.ui.blocks_palette.favourites.is_empty());
}

#[test]
fn clicking_a_block_places_it_with_the_options_and_repeats() {
    hooks();
    let mut app = app();
    app.ui.blocks_palette.scale = 3.0;
    app.ui.blocks_palette.rotation = 90.0;
    app.ui.blocks_palette.repeat = true;
    let tag = BlockRef { name: "TAG".into(), file: None };
    blockspalette::start_insert(&mut app, &tag);
    assert_eq!(app.session.current_prompt().map(|p| p.message), Some("Specify insertion point".to_string()));
    app.session.input(Input::Point(cadaddy_engine::geom::Vec2::new(10.0, 0.0))).unwrap();
    // Attribute-free block: placed at once with the preset scale and rotation.
    while app.session.running.is_some() && app.session.current_prompt().is_some_and(|p| p.message != "Specify insertion point") {
        app.session.input(Input::Enter).unwrap();
    }
    blockspalette::follow(&mut app);
    let i = inserts(&app);
    assert_eq!(i.len(), 1);
    assert!((i[0].scale.x - 3.0).abs() < 1e-9 && (i[0].rotation - 90f64.to_radians()).abs() < 1e-9);
    // Repeat Placement: INSERT asks again; Escape ends it.
    assert!(app.session.running.as_ref().is_some_and(|r| r.id == "insert"));
    app.session.input(Input::Cancel).unwrap();
    blockspalette::follow(&mut app);
    assert!(app.session.running.is_none() && app.placing.is_none());
}

#[test]
fn tool_palettes_are_made_from_objects_used_kept_and_exchanged() {
    hooks();
    let mut app = app();
    app.session.execute("layer", &json!({ "action": "new", "name": "WALLS" })).ok();
    if app.session.doc().unwrap().layer("WALLS").is_none() {
        app.session.doc_mut().unwrap().layers.push(cadaddy_engine::doc::Layer::new("WALLS"));
    }
    let line = app.session.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    let lh = line["handle"].as_str().or_else(|| line["handles"][0].as_str()).unwrap().to_string();
    let h = cadaddy_engine::doc::Handle::parse_hex(&lh).unwrap();
    app.session.doc_mut().unwrap().model.modify(h, |e| e.common.layer = "WALLS".into());
    blockspalette::insert_at(&mut app, &BlockRef { name: "TAG".into(), file: None }, [20.0, 0.0]).unwrap();
    let all: Vec<Handle> = app.session.doc().unwrap().model.iter().map(|e| e.handle).collect();
    app.session.state_mut().unwrap().selection = all;

    let r = app.run("toolpalettes", json!({ "action": "add", "palette": "Site", "selected": true })).unwrap();
    assert_eq!(r["added"], 2);
    let site = app.ui.tool_palettes.iter().find(|p| p.name == "Site").unwrap().clone();
    assert!(site.tools.contains(&Tool::Block { name: "TAG".into(), file: None }));
    assert!(site.tools.contains(&Tool::Command { command: "line".into(), layer: Some("WALLS".into()), linetype: None }));
    // A hatch tool added by description.
    app.run(
        "toolpalettes",
        json!({ "action": "add", "palette": "Site", "tools": [{ "type": "hatch", "pattern": "ANSI37", "scale": 2.0, "angle": 45.0 }] }),
    )
    .unwrap();

    // Using tools.
    toolpalettes::use_tool(&mut app, &Tool::Hatch { pattern: "ANSI37".into(), scale: 2.0, angle: 45.0 });
    assert_eq!(sysvars::get(&app.session, "HPNAME"), Some(json!("ANSI37")));
    app.session.input(Input::Cancel).ok();
    toolpalettes::use_tool(&mut app, &Tool::Command { command: "line".into(), layer: Some("WALLS".into()), linetype: None });
    assert_eq!(sysvars::get(&app.session, "CLAYER"), Some(json!("WALLS")));
    assert!(app.session.running.as_ref().is_some_and(|r| r.id == "line"));
    app.session.input(Input::Cancel).ok();

    // Kept with the interface state.
    let ui: cadaddy_ui_egui::UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
    assert_eq!(ui.tool_palettes, app.ui.tool_palettes);

    // Exported and imported into another app.
    let file = std::env::temp_dir().join(format!("cadaddy-tools-{}.json", std::process::id()));
    app.run("toolpalettes", json!({ "action": "export", "path": file.to_string_lossy() })).unwrap();
    let mut other = CadApp::new(Session::new(), Services::default());
    other.run("toolpalettes", json!({ "action": "import", "path": file.to_string_lossy() })).unwrap();
    assert_eq!(other.ui.tool_palettes.iter().find(|p| p.name == "Site").map(|p| p.tools.len()), Some(3));
    std::fs::write(&file, r#"{"palettes": []}"#).unwrap();
    assert!(other.run("toolpalettes", json!({ "action": "import", "path": file.to_string_lossy() })).is_err());

    // Removing a tool, then the palette.
    app.run("toolpalettes", json!({ "action": "remove", "palette": "Site", "index": 0 })).unwrap();
    assert_eq!(app.ui.tool_palettes.iter().find(|p| p.name == "Site").map(|p| p.tools.len()), Some(2));
    app.run("toolpalettes", json!({ "action": "remove", "palette": "Site" })).unwrap();
    assert!(app.run("toolpalettes", json!({ "action": "remove", "palette": "Site" })).is_err());
    let r: Value = app.run("toolpalettes", json!({})).unwrap();
    assert!(r["palettes"].is_array());
}

#[test]
fn the_palettes_open() {
    let mut app = app();
    app.run("blockspalette", Value::Null).unwrap();
    assert_eq!(app.ui.dialog.as_deref(), Some("blocks"));
    app.run("toolpalettes", Value::Null).unwrap();
    assert_eq!(app.ui.dialog.as_deref(), Some("toolpalettes"));
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| cadaddy_ui_egui::dialogs::show(&mut app, ui.ctx()));
    out.textures_delta.clear();
}
