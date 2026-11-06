//! The Blocks palette's Parts Library: CADaddy's parts by discipline and category, searched,
//! previewed, and placed with PART (defined with their dynamic parameters, at the drawing's units,
//! on their discipline's layer) by a click, a drop or Repeat Placement.

use cadaddy_engine::doc::EntityKind;
use cadaddy_engine::{Input, Session};
use cadaddy_ui_egui::blockspalette::{self, BlockRef};
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Rect, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn inserts(app: &CadApp) -> Vec<(String, f64, f64, String)> {
    app.session
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| {
            if let EntityKind::Insert(i) = &e.kind { Some((i.block.clone(), i.scale.x, i.rotation, e.common.layer.to_string())) } else { None }
        })
        .collect()
}

fn part(name: &str) -> BlockRef {
    BlockRef { name: name.into(), file: None }
}

#[test]
fn dropping_a_part_places_it_with_part() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.blocks_palette.rotation = 90.0;
    // The palette's scale is not a part's: parts come at the drawing's units.
    app.ui.blocks_palette.scale = 5.0;
    let ipe = part("S_IPE300");
    blockspalette::insert_at(&mut app, &ipe, [10.0, 0.0]).unwrap();
    let i = inserts(&app);
    assert_eq!(i.len(), 1);
    let (block, scale, rot, layer) = &i[0];
    assert_eq!((block.as_str(), layer.as_str()), ("S_IPE300", "S-MEMB"));
    assert!((scale - 1.0 / 25.4).abs() < 1e-12 && (rot - 90f64.to_radians()).abs() < 1e-9, "{scale} {rot}");
    assert_eq!(app.ui.blocks_palette.recent, std::slice::from_ref(&ipe));
    // A block of the drawing is still an INSERT with the palette's scale.
    let c = app.session.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let h = c["handle"].as_str().unwrap().to_string();
    app.session.execute("block", &json!({ "name": "TAG", "base": [0, 0], "handles": [h], "keep": "delete" })).unwrap();
    blockspalette::insert_at(&mut app, &part("TAG"), [0.0, 0.0]).unwrap();
    assert!(inserts(&app).iter().any(|(b, s, _, _)| b == "TAG" && (*s - 5.0).abs() < 1e-9));
}

#[test]
fn clicking_a_part_starts_part_and_repeats() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.session.execute("setvar", &json!({"name": "ATTREQ", "value": 0})).unwrap();
    app.ui.blocks_palette.repeat = true;
    let door = part("A_DOOR_SGL_36_RH");
    blockspalette::start_insert(&mut app, &door);
    assert!(app.session.running.as_ref().is_some_and(|r| r.id == "part"));
    assert_eq!(app.session.current_prompt().map(|p| p.message), Some("Specify insertion point".to_string()));
    app.session.input(Input::Point(cadaddy_engine::geom::Vec2::new(10.0, 0.0))).unwrap();
    while app.session.running.is_some() && app.session.current_prompt().is_some_and(|p| p.message != "Specify insertion point") {
        app.session.input(Input::Enter).unwrap();
    }
    blockspalette::follow(&mut app);
    assert_eq!(inserts(&app).len(), 1);
    assert_eq!(inserts(&app)[0].3, "A-DOOR");
    // The block is a dynamic one: its swing is a property.
    assert!(cadaddy_engine::doc::dynblock::dynamic_block(app.session.doc().unwrap(), "A_DOOR_SGL_36_RH").is_some());
    // Repeat Placement starts PART again with the same part; Escape ends it.
    assert!(app.session.running.as_ref().is_some_and(|r| r.id == "part"));
    app.session.input(Input::Cancel).unwrap();
    blockspalette::follow(&mut app);
    assert!(app.session.running.is_none() && app.placing.is_none());
}

fn texts(shape: &Shape, clip: Rect, out: &mut Vec<String>) {
    match shape {
        Shape::Text(t) if t.visual_bounding_rect().intersect(clip).is_positive() => out.push(t.galley.text().to_string()),
        Shape::Vec(v) => v.iter().for_each(|s| texts(s, clip, out)),
        _ => {}
    }
}

fn painted(h: &Harness<'static, CadApp>) -> Vec<String> {
    let mut out = Vec::new();
    for s in &h.output().shapes {
        texts(&s.shape, s.clip_rect, &mut out);
    }
    out
}

/// The tab lists parts with previews, says how many there are, and the filter finds them by
/// designation. `CADADDY_PARTS_SHOT=1` saves renders to `target/tmp` to look at.
#[test]
fn the_parts_library_tab_lists_and_filters() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.start_tab = false;
    app.ui.dialog = Some("blocks".into());
    let shots = std::env::var_os("CADADDY_PARTS_SHOT").is_some();
    let builder = Harness::builder().with_size(vec2(1280.0, 800.0));
    let builder = if shots { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h.get_by_label("Parts Library").click();
    h.run_steps(4);
    let shown = painted(&h);
    let total = cadaddy_engine::doc::library::parts::parts().len();
    assert!(shown.iter().any(|t| t.contains(&format!("of {total} parts"))), "{shown:?}");
    assert!(shown.iter().any(|t| t.starts_with("DOOR_SGL")), "{shown:?}");
    let save = |h: &mut Harness<'static, CadApp>, name: &str| {
        if shots && let Ok(img) = h.render() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
            let _ = std::fs::create_dir_all(&dir);
            let _ = img.save(dir.join(name));
        }
    };
    save(&mut h, "parts-library.png");
    // The label and the field both carry the name: the field comes second.
    h.query_all_by_label("Filter").last().unwrap().click();
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text("w8x31".into()));
    h.run_steps(4);
    let shown = painted(&h);
    assert!(shown.iter().any(|t| t == "W8X31"), "{shown:?}");
    assert!(!shown.iter().any(|t| t.starts_with("DOOR")), "{shown:?}");
    save(&mut h, "parts-library-w8x31.png");
}
