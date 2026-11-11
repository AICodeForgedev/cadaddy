//! View ▸ 3D BIM (Q6310): the built model in 3D in one click, and a drawing opened holding a
//! built model offering it. A made-up building (`sample::bim_building`).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::Space;
use cadaddy_engine::render::shade::VisualStyle;
use serde_json::json;

use crate::helper::run;

fn building() -> Session {
    let mut s = crate::helper::session();
    s.open_drawing(cadaddy_engine::sample::bim_building(3, 6), "Sample Building", None);
    s
}

/// The plan box of the built shell (floors), its storeys stacked as the BIM view draws them.
fn shell_box(s: &Session) -> cadaddy_engine::geom::Bounds2 {
    let shown = bimlook::display(s.doc().unwrap()).expect("the BIM view of a built model");
    let mut b = cadaddy_engine::geom::Bounds2::EMPTY;
    for e in shown.model.iter().filter(|e| made_as(e).as_deref() == Some("floor")) {
        b = b.union(&cadaddy_engine::doc::entity_bounds(&shown, e, 0));
    }
    b
}

#[test]
fn three_d_bim_is_one_click_from_any_state() {
    let mut s = building();
    // The owner's state: the plan, the BIM view off and solid, one level's Floor view, filled-in
    // work apart, a fixed coarse fidelity tier that leaves the MEP out.
    run(&mut s, "bim.view", json!({"level": "L2"}));
    run(&mut s, "bim.look", json!({"on": false, "ghost": false, "apart": true}));
    run(&mut s, "view.fidelity", json!({"level": "building"}));
    run(&mut s, "vscurrent", json!({"style": "2dwireframe"}));
    run(&mut s, "plan", json!({}));
    assert_eq!(s.state().unwrap().visual_style, VisualStyle::Wireframe2d);

    let r = run(&mut s, "view.bim3d", json!({}));
    assert_eq!(r["view"], "mep", "{r}");
    assert_eq!(r["level"], serde_json::Value::Null, "the whole building: {r}");
    assert_eq!(r["ghost"], true);
    assert_eq!(r["fidelity"], "auto");
    assert_eq!(r["label"], "SW Isometric");
    let st = s.state().unwrap();
    assert_eq!(st.visual_style, VisualStyle::ShadedEdges);
    assert!(st.fidelity.mode.is_none(), "a fixed tier would hold the view's MEP back");
    let look = bimlook::Look::of(&st.doc);
    assert!(!look.off && look.ghost == Some(true) && !look.apart && !look.sheet, "{look:?}");
    assert_eq!(run(&mut s, "view.fidelity", json!({}))["level"], "auto");

    // What it draws: the shell ghosted, the MEP mains in trade colours.
    let shown = bimlook::display(s.doc().unwrap()).expect("the BIM view is on");
    let mep: Vec<_> = shown.model.iter().filter(|e| made_as(e).as_deref() == Some("mep")).collect();
    assert!(!mep.is_empty(), "the 3D BIM view draws the MEP mains");
    // Trade colours: each trade's elements in its own colour, not the drawing's.
    let colours: std::collections::BTreeSet<String> = mep
        .iter()
        .map(|e| match &e.common.color {
            cadaddy_engine::doc::color::Color::ByLayer => {
                format!("{:?}", shown.layers.iter().find(|l| l.name.eq_ignore_ascii_case(&e.common.layer)).map(|l| l.color))
            }
            c => format!("{c:?}"),
        })
        .collect();
    assert!(colours.len() > 1, "the trades in their own colours: {colours:?}");
    assert!(r["drawn"].as_u64().unwrap_or(0) > 0 && r["drawn"].as_u64() < r["total"].as_u64(), "inside the view, never everything: {r}");

    // Zoomed to the building: its middle in the middle of the view, filling it.
    let st = s.state().unwrap();
    let p = st.projection().expect("a 3D view");
    let shell = shell_box(&s);
    let mid = cadaddy_engine::geom::Vec3::new((shell.min.x + shell.max.x) / 2.0, (shell.min.y + shell.max.y) / 2.0, 0.0);
    let v = st.view();
    let span = shell.width().max(shell.height());
    assert!((p.project(mid) - v.center).len() < 0.5 * span, "the view centres on the building: {:?} vs {:?}", p.project(mid), v.center);
    assert!(v.height < 2.0 * span && v.height > 0.1 * span, "the view fills with the building: height {} for a {span} building", v.height);
}

#[test]
fn three_d_bim_frames_the_building_not_a_stray_object() {
    let mut s = building();
    run(&mut s, "view.bim3d", json!({}));
    // A reader's stray built object far off the building (model.issues: outside_building), of a
    // kind the view draws.
    let stray = {
        let shown = bimlook::display(s.doc().unwrap()).expect("the BIM view is on");
        let h = shown.model.iter().find(|e| made_as(e).as_deref() == Some("mep")).expect("a drawn MEP element").handle;
        let mut c = (**s.doc().unwrap().model.get(h).expect("the drawn element is the drawing's")).clone();
        c.handle = s.doc_mut().unwrap().new_handle();
        c.kind.transform(&cadaddy_engine::geom::Mat3::translate(cadaddy_engine::geom::Vec2::new(200_000.0, 150_000.0)));
        c
    };
    s.doc_mut().unwrap().model.push(stray);
    let span = {
        let shell = shell_box(&s);
        shell.width().max(shell.height())
    };
    // Zoom Extents takes it in: the building a speck.
    run(&mut s, "zoom.extents", json!({}));
    let all = s.state().unwrap().view().height;
    assert!(all > 4.0 * span, "the stray object is drawn and far off: extents height {all} for a {span} building");
    run(&mut s, "view.bim3d", json!({}));
    let h = s.state().unwrap().view().height;
    assert!(h < 2.0 * span, "the stray object shrank the building to a speck: view height {h} for a {span} building");
}

#[test]
fn three_d_bim_needs_a_built_model_and_leaves_a_layout_for_the_model() {
    let mut s = crate::helper::session();
    s.open_drawing(cadaddy_engine::doc::Drawing::new_imperial(), "Plan", None);
    let e = s.execute("view.bim3d", &json!({})).unwrap_err().to_string();
    assert!(e.contains("model.build"), "{e}");

    let mut s = building();
    run(&mut s, "layout.set", json!({"name": "Layout1"}));
    assert_ne!(s.state().unwrap().space, Space::Model);
    run(&mut s, "view.bim3d", json!({}));
    assert_eq!(s.state().unwrap().space, Space::Model);
    assert_eq!(s.state().unwrap().visual_style, VisualStyle::ShadedEdges);
}

#[test]
fn a_drawing_opened_holding_a_built_model_offers_its_3d_bim_view() {
    crate::helper::hooks();
    let built = cadaddy_io::write(&cadaddy_engine::sample::bim_building(2, 4), "Sample Building.dxf").expect("write");
    let plain = cadaddy_io::write(&cadaddy_engine::doc::Drawing::new_imperial(), "Plan.dxf").expect("write");
    let mut s = crate::helper::session();
    let data = |b: &[u8]| cadaddy_engine::cmd::file::base64_encode(b);
    let r = run(&mut s, "open", json!({"data": data(&built), "name": "Sample Building.dxf"}));
    assert_eq!(r["builtModel"], true, "{r}");
    assert_eq!(r["offer"], "view.bim3d", "{r}");
    // It opens on the plan: the offer is the app's to make.
    assert_eq!(s.state().unwrap().visual_style, VisualStyle::Wireframe2d);
    let r = run(&mut s, "open", json!({"data": data(&plain), "name": "Plan.dxf"}));
    assert!(r.get("builtModel").is_none() && r.get("offer").is_none(), "{r}");
}
