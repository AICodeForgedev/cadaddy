//! Annotative objects keep a position per supported scale: moving one at the current scale moves
//! only that representation, ANNORESET brings the others to it. A viewport can have its own
//! annotation scale apart from its zoom scale. Both survive DXF.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec2;
use cadaddy_engine::doc::{AnnoScale, Drawing, Entity, EntityKind, Handle, Space, anno_scaled};
use serde_json::{Value, json};

fn setvar(s: &mut Session, name: &str, value: Value) {
    s.execute("setvar", &json!({ "name": name, "value": value })).unwrap();
}

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn entity(s: &Session, hd: Handle) -> Entity {
    (**s.doc().unwrap().entity(hd).unwrap()).clone()
}

/// Where the text's representation at scale factor `f` is drawn.
fn text_at(s: &Session, hd: Handle, f: f64) -> Vec2 {
    match anno_scaled(&entity(s, hd), f).kind {
        EntityKind::Text(t) => t.insert.xy(),
        k => panic!("not text: {k:?}"),
    }
}

/// An annotative text at 0,0 supporting 1:1 and 1:2, with 1:1 current.
fn two_scale_text() -> (Session, Handle) {
    let mut s = Session::new();
    s.execute("style", &json!({ "name": "Notes", "annotative": true, "current": true })).unwrap();
    let t = h(&s.execute("text", &json!({ "at": [0, 0], "text": "NOTE", "height": 0.25 })).unwrap());
    s.execute("objectscale", &json!({ "handles": [t.hex()], "add": "1:2" })).unwrap();
    assert_eq!(entity(&s, t).common.anno_scales.len(), 2);
    (s, t)
}

#[test]
fn moving_at_the_current_scale_moves_only_that_representation() {
    let (mut s, t) = two_scale_text();
    s.execute("move", &json!({ "handles": [t.hex()], "from": [0, 0], "to": [5, 1] })).unwrap();
    // The stored position stays; the 1:1 representation moved, the 1:2 one didn't.
    let EntityKind::Text(base) = entity(&s, t).kind else { panic!() };
    assert_eq!(base.insert.xy(), Vec2::ZERO);
    assert_eq!(text_at(&s, t, 1.0), Vec2::new(5.0, 1.0));
    assert_eq!(text_at(&s, t, 2.0), Vec2::ZERO);

    // At 1:2 a move moves the 1:2 representation.
    setvar(&mut s, "CANNOSCALE", json!("1:2"));
    s.execute("move", &json!({ "handles": [t.hex()], "from": [0, 0], "to": [0, -3] })).unwrap();
    assert_eq!(text_at(&s, t, 2.0), Vec2::new(0.0, -3.0));
    assert_eq!(text_at(&s, t, 1.0), Vec2::new(5.0, 1.0));

    // Undo takes back the last move only.
    s.undo().unwrap();
    assert_eq!(text_at(&s, t, 2.0), Vec2::ZERO);
    assert_eq!(text_at(&s, t, 1.0), Vec2::new(5.0, 1.0));
}

#[test]
fn single_scale_objects_and_other_transforms_move_the_object() {
    let mut s = Session::new();
    s.execute("style", &json!({ "name": "Notes", "annotative": true, "current": true })).unwrap();
    let t = h(&s.execute("text", &json!({ "at": [0, 0], "text": "ONE", "height": 0.25 })).unwrap());
    s.execute("move", &json!({ "handles": [t.hex()], "from": [0, 0], "to": [2, 0] })).unwrap();
    let EntityKind::Text(base) = entity(&s, t).kind else { panic!() };
    assert_eq!(base.insert.xy(), Vec2::new(2.0, 0.0));

    // Rotating a two-scale object rotates the object itself.
    let (mut s, t) = two_scale_text();
    s.execute("rotate", &json!({ "handles": [t.hex()], "base": [0, 0], "angle": 90 })).unwrap();
    let e = entity(&s, t);
    assert!(e.common.anno_scales.iter().all(|sc| sc.offset == Vec2::ZERO));
    let EntityKind::Text(base) = e.kind else { panic!() };
    assert!((base.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
}

#[test]
fn annoreset_brings_every_representation_to_the_current_one() {
    let (mut s, t) = two_scale_text();
    s.execute("move", &json!({ "handles": [t.hex()], "from": [0, 0], "to": [5, 1] })).unwrap();
    let r = s.execute("annoreset", &json!({ "handles": [t.hex()] })).unwrap();
    assert_eq!(r["changed"], 1);
    assert_eq!(text_at(&s, t, 2.0), Vec2::new(5.0, 1.0), "the 1:2 representation came to the 1:1 one");
    // Already in step: nothing changes.
    assert_eq!(s.execute("annoreset", &json!({ "handles": [t.hex()] })).unwrap()["changed"], 0);

    // From 1:2, the others go where 1:2's is.
    s.execute("move", &json!({ "handles": [t.hex()], "from": [0, 0], "to": [1, 0] })).unwrap();
    setvar(&mut s, "CANNOSCALE", json!("1:2"));
    s.execute("annoreset", &json!({ "handles": [t.hex()] })).unwrap();
    assert_eq!(text_at(&s, t, 1.0), Vec2::new(5.0, 1.0));
}

#[test]
fn multileaders_and_dimensions_move_their_content() {
    let mut e = Entity::new(
        Handle(0x50),
        EntityKind::Dimension(Box::new(cadaddy_engine::doc::Dimension {
            kind: cadaddy_engine::doc::DimKind::Linear { rotation: 0.0 },
            defpt: cadaddy_engine::doc::geom::Vec3::new(0.0, 2.0, 0.0),
            text_mid: cadaddy_engine::doc::geom::Vec3::ZERO,
            p13: cadaddy_engine::doc::geom::Vec3::ZERO,
            p14: cadaddy_engine::doc::geom::Vec3::new(10.0, 0.0, 0.0),
            p15: cadaddy_engine::doc::geom::Vec3::ZERO,
            p16: cadaddy_engine::doc::geom::Vec3::ZERO,
            text: String::new(),
            style: "Standard".into(),
            measurement: 0.0,
            text_rotation: 0.0,
            user_text_pos: false,
            block: None,
            overrides: Default::default(),
            assoc: Vec::new(),
            marks: Default::default(),
        })),
    );
    let mut sc = AnnoScale::new("1:2", 1.0, 2.0);
    sc.offset = Vec2::new(0.0, 3.0);
    e.common.anno_scales = vec![AnnoScale::new("1:1", 1.0, 1.0), sc];
    // The dimension line moves; the points it measures stay.
    let EntityKind::Dimension(d) = anno_scaled(&e, 2.0).kind else { panic!() };
    assert_eq!((d.defpt.y, d.p13.y, d.p14.x), (5.0, 0.0, 10.0));
    let EntityKind::Dimension(d) = anno_scaled(&e, 1.0).kind else { panic!() };
    assert_eq!(d.defpt.y, 2.0);

    // A multileader's content moves with its landing; the arrowheads stay.
    let mut s = Session::new();
    let m = h(&s.execute("mleader", &json!({ "points": [[0, 0], [4, 4]], "text": "NOTE" })).unwrap());
    let mut e = entity(&s, m);
    let mut sc = AnnoScale::new("1:2", 1.0, 2.0);
    sc.offset = Vec2::new(1.0, 0.0);
    e.common.anno_scales = vec![AnnoScale::new("1:1", 1.0, 1.0), sc];
    let (EntityKind::MLeader(before), EntityKind::MLeader(after)) = (e.kind.clone(), anno_scaled(&e, 2.0).kind) else { panic!() };
    assert!((after.landing.x - before.landing.x - 1.0).abs() < 1e-9);
    assert_eq!(after.leaders, before.leaders);
}

/// A drawing with an annotative text supporting only 1:2, and Layout1's viewport at 1:1 zoom.
fn layout_with_text() -> (Session, Handle) {
    let mut s = Session::new();
    s.execute("style", &json!({ "name": "Notes", "annotative": true, "current": true })).unwrap();
    setvar(&mut s, "CANNOSCALE", json!("1:2"));
    s.execute("text", &json!({ "at": [1, 1], "text": "HALF", "height": 0.25 })).unwrap();
    setvar(&mut s, "ANNOALLVISIBLE", json!(0));
    let vp = s.execute("mview", &json!({ "layout": "Layout1", "p1": [0, 0], "p2": [8, 6] })).unwrap()["viewports"][0].as_str().unwrap().to_string();
    let vp = Handle::parse_hex(&vp).unwrap();
    s.execute("viewport.set", &json!({ "handle": vp.hex(), "scale": 1, "center": [1, 1] })).unwrap();
    (s, vp)
}

fn content_prims(d: &Drawing) -> usize {
    let l = cadaddy_render::build(d, &Space::Paper("Layout1".into()), &cadaddy_render::Options::default());
    l.prims.iter().filter(|p| p.handle == cadaddy_render::VIEWPORT_CONTENT).count()
}

#[test]
fn a_viewport_has_its_own_annotation_scale() {
    let (mut s, vp) = layout_with_text();
    // At 1:1 the 1:2-only text is hidden (ANNOALLVISIBLE 0).
    let hidden = content_prims(s.doc().unwrap());
    let r = s.execute("viewport.set", &json!({ "handle": vp.hex(), "annoScale": "1:2" })).unwrap();
    assert_eq!(r[0]["annoScale"], "1:2");
    assert_eq!(r[0]["scale"], 1.0, "the zoom scale is unchanged");
    let shown = content_prims(s.doc().unwrap());
    assert!(shown > hidden, "{hidden} → {shown}");
    // Following the viewport's scale again.
    s.execute("viewport.set", &json!({ "handle": vp.hex(), "annoScale": null })).unwrap();
    assert_eq!(content_prims(s.doc().unwrap()), hidden);
    assert!(s.execute("viewport.set", &json!({ "handle": vp.hex(), "annoScale": "1:7.5" })).is_err());
}

#[test]
fn positions_and_viewport_scales_survive_dxf() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let (mut s, vp) = layout_with_text();
    s.execute("viewport.set", &json!({ "handle": vp.hex(), "annoScale": "1:2" })).unwrap();
    let (s2, t) = {
        let (mut s2, t) = two_scale_text();
        s2.execute("move", &json!({ "handles": [t.hex()], "from": [0, 0], "to": [5, 1] })).unwrap();
        (s2, t)
    };
    let back = cadaddy_io::read(cadaddy_io::write_dxf(s2.doc().unwrap()).as_bytes(), "t.dxf").unwrap();
    let e = back.entity(t).unwrap();
    let offsets: Vec<(String, Vec2)> = e.common.anno_scales.iter().map(|sc| (sc.name.clone(), sc.offset)).collect();
    assert_eq!(offsets, vec![("1:1".into(), Vec2::new(5.0, 1.0)), ("1:2".into(), Vec2::ZERO)]);

    let back = cadaddy_io::read(cadaddy_io::write_dxf(s.doc().unwrap()).as_bytes(), "t.dxf").unwrap();
    let vps: Vec<Option<String>> = back
        .layout("Layout1")
        .unwrap()
        .entities
        .iter()
        .filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id != 1).then(|| v.anno_scale.clone()) } else { None })
        .collect();
    assert!(vps.contains(&Some("1:2".into())), "{vps:?}");
}
