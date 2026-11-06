//! Express Tools BURST (explode as drawn: attributes become text, layer 0 and ByBlock take the
//! reference's properties, nested references stay) and TXT2MTXT (single-line text to MTEXT).

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::geom::{Vec2, Vec3};
use cadaddy_engine::doc::{
    Attrib, AttribModes, AttribProps, Block, Circle, Drawing, Entity, EntityKind, HAlign, Handle, Insert, Layer, Line, Lineweight, Space, Text,
    VAlign,
};
use serde_json::{Value, json};

fn text(value: &str, at: Vec3, height: f64) -> Text {
    Text {
        insert: at,
        align_pt: None,
        height,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    }
}

fn attrib(tag: &str, value: &str, at: Vec3, invisible: bool) -> Attrib {
    Attrib {
        tag: tag.into(),
        text: text(value, at, 0.25),
        invisible,
        constant: false,
        prompt: String::new(),
        props: AttribProps::default(),
        multiline: None,
        modes: AttribModes::default(),
    }
}

fn reference(block: &str, at: Vec3, scale: Vec3, rotation: f64) -> Insert {
    Insert { block: block.into(), insert: at, scale, rotation, attribs: Vec::new(), cols: 1, rows: 1, col_spacing: 0.0, row_spacing: 0.0, clip: None }
}

fn ent(h: u64, layer: &str, kind: EntityKind) -> Entity {
    let mut e = Entity::new(Handle(h), kind);
    e.common.layer = layer.into();
    e
}

/// Block PART: on layer 0 a ByBlock line (0,0)-(1,0) and a ByLayer circle at (1,1) r 0.5; on
/// layer FIXED a red line; a reference to block NUT; attribute definitions TAG and SECRET.
/// Block NUT: a circle.
fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    let mut fixed = Layer::new("FIXED");
    fixed.color = Color::Index(1);
    d.layers.push(fixed);
    d.layers.push(Layer::new("DOORS"));
    let mut nut = Block::new("NUT");
    nut.entities.push(ent(0x40, "0", EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 0.1 })));
    d.blocks.insert("NUT".into(), Arc::new(nut));
    let mut part = Block::new("PART");
    let mut l = ent(0x50, "0", EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 0.0, 0.0) }));
    l.common.color = Color::ByBlock;
    l.common.linetype = "ByBlock".into();
    l.common.lineweight = Lineweight::ByBlock;
    part.entities.push(l);
    part.entities.push(ent(0x51, "0", EntityKind::Circle(Circle { center: Vec3::new(1.0, 1.0, 0.0), radius: 0.5 })));
    part.entities.push(ent(0x52, "FIXED", EntityKind::Line(Line { a: Vec3::new(0.0, 2.0, 0.0), b: Vec3::new(2.0, 2.0, 0.0) })));
    part.entities.push(ent(0x53, "FIXED", EntityKind::Insert(reference("NUT", Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0), 0.0))));
    part.entities.push(ent(0x54, "0", EntityKind::AttDef(Box::new(attrib("TAG", "?", Vec3::new(0.0, -0.5, 0.0), false)))));
    part.entities.push(ent(0x55, "0", EntityKind::AttDef(Box::new(attrib("SECRET", "?", Vec3::new(0.0, -1.0, 0.0), true)))));
    d.blocks.insert("PART".into(), Arc::new(part));
    d
}

fn session() -> Session {
    let mut s = Session::new();
    *s.doc_mut().unwrap() = drawing();
    s
}

/// Add a PART reference on layer DOORS, colour 3, linetype DASHED, lineweight 0.50 mm.
fn add_part(s: &mut Session, at: Vec3, scale: Vec3, rotation: f64, attribs: Vec<Attrib>) -> Handle {
    let d = s.doc_mut().unwrap();
    let h = d.new_handle();
    let mut ins = reference("PART", at, scale, rotation);
    ins.attribs = attribs;
    let mut e = ent(h.0, "DOORS", EntityKind::Insert(ins));
    e.common.color = Color::Index(3);
    e.common.linetype = "DASHED".into();
    e.common.lineweight = Lineweight::from_dxf(50);
    d.model.push(e);
    h
}

fn burst(s: &mut Session, h: Handle) -> Vec<Entity> {
    let r = s.execute("burst", &json!({ "handles": [h.hex()] })).unwrap();
    let d = s.doc().unwrap();
    r["handles"].as_array().unwrap().iter().map(|v| (**d.entity(Handle::parse_hex(v.as_str().unwrap()).unwrap()).unwrap()).clone()).collect()
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
}

#[test]
fn burst_keeps_geometry_for_rotated_scaled_and_mirrored_references() {
    for (scale, rot) in [
        (Vec3::new(1.0, 1.0, 1.0), 0.0),
        (Vec3::new(2.0, 2.0, 1.0), 30f64.to_radians()),
        (Vec3::new(2.0, 3.0, 1.0), 90f64.to_radians()),
        (Vec3::new(-1.0, 1.0, 1.0), 0.0),
        (Vec3::new(1.5, -0.5, 1.0), 200f64.to_radians()),
    ] {
        let mut s = session();
        // Without the attribute definitions, which the extents count but aren't drawn.
        let d = s.doc_mut().unwrap();
        let mut part = (**d.block("PART").unwrap()).clone();
        let kept: Vec<Entity> = part.entities.iter().filter(|e| !matches!(e.kind, EntityKind::AttDef(_))).map(|e| (**e).clone()).collect();
        part.entities = Default::default();
        kept.into_iter().for_each(|e| part.entities.push(e));
        d.blocks.insert("PART".into(), Arc::new(part));
        let at = Vec3::new(10.0, 5.0, 0.0);
        let h = add_part(&mut s, at, scale, rot, Vec::new());
        // EXPLODE of the same reference draws the same geometry (only properties differ).
        let mut twin = Session::new();
        *twin.doc_mut().unwrap() = s.doc().unwrap().clone();
        twin.execute("explode", &json!({ "handles": [h.hex()] })).unwrap();
        let exploded = twin.doc().unwrap().extents(&Space::Model);
        let parts = burst(&mut s, h);
        let after = s.doc().unwrap().extents(&Space::Model);
        let tol = 1e-6;
        assert!(after.min.dist(exploded.min) < tol && after.max.dist(exploded.max) < tol, "{scale:?} {rot}: {exploded:?} vs {after:?}");
        assert!(s.doc().unwrap().entity(h).is_none(), "the reference is gone");
        // The first line ends where the block's (1,0) lands.
        let end = Vec2::new(scale.x, 0.0).rotate(rot) + at.xy();
        let line = parts
            .iter()
            .find_map(|e| if let EntityKind::Line(l) = &e.kind { (e.common.layer == "DOORS").then_some(l.clone()) } else { None })
            .unwrap();
        assert!(close(line.a, at) && close(line.b, end.to3(0.0)), "{scale:?} {rot}: {line:?}");
    }
}

#[test]
fn burst_gives_layer_0_and_byblock_the_references_properties_and_keeps_nested_blocks() {
    let mut s = session();
    let h = add_part(&mut s, Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), 0.0, Vec::new());
    let parts = burst(&mut s, h);
    // Line, circle, red line, NUT reference; attribute definitions are dropped.
    assert_eq!(parts.len(), 4);
    let line = &parts[0];
    assert_eq!(
        (line.common.layer.as_str(), line.common.color, line.common.linetype.as_str(), line.common.lineweight),
        ("DOORS", Color::Index(3), "DASHED", Lineweight::from_dxf(50))
    );
    // Layer 0 ByLayer: the reference's layer, still ByLayer.
    let circle = parts.iter().find(|e| matches!(e.kind, EntityKind::Circle(_))).unwrap();
    assert_eq!((circle.common.layer.as_str(), circle.common.color), ("DOORS", Color::ByLayer));
    // Objects on other layers keep theirs.
    let fixed: Vec<&Entity> = parts.iter().filter(|e| e.common.layer == "FIXED").collect();
    assert_eq!(fixed.len(), 2);
    // The nested reference is still a reference.
    assert!(fixed.iter().any(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block == "NUT")));
    assert!(!parts.iter().any(|e| matches!(e.kind, EntityKind::AttDef(_))));

    // Undo brings the reference back.
    s.undo().unwrap();
    assert!(s.doc().unwrap().entity(h).is_some());
}

#[test]
fn burst_turns_attributes_into_text_with_their_values() {
    let mut s = session();
    let a = attrib("TAG", "D-101", Vec3::new(5.0, 4.5, 0.0), false);
    let mut b = attrib("SECRET", "hidden", Vec3::new(5.0, 4.0, 0.0), true);
    b.text.value = "hidden".into();
    let mut c = attrib("LOOSE", "own layer", Vec3::new(5.0, 3.0, 0.0), false);
    c.props.layer = Some("FIXED".into());
    let h = add_part(&mut s, Vec3::new(5.0, 5.0, 0.0), Vec3::new(1.0, 1.0, 1.0), 0.0, vec![a, b, c]);
    let parts = burst(&mut s, h);
    let texts: Vec<(&str, &str, Vec3)> = parts
        .iter()
        .filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some((t.value.as_str(), e.common.layer.as_str(), t.insert)) } else { None })
        .collect();
    // The invisible attribute is dropped; the value and position are kept; layer 0 from the
    // definition becomes the reference's layer, the attribute's own layer stays.
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts.contains(&("D-101", "DOORS", Vec3::new(5.0, 4.5, 0.0))), "{texts:?}");
    assert!(texts.contains(&("own layer", "FIXED", Vec3::new(5.0, 3.0, 0.0))), "{texts:?}");
}

#[test]
fn burst_typed_uses_the_selection_and_skips_other_objects() {
    let mut s = session();
    let h = add_part(&mut s, Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), 0.0, Vec::new());
    let line = s.execute("line", &json!({ "points": [[20.0, 0.0], [30.0, 0.0]] })).unwrap();
    let line = Handle::parse_hex(line["handles"][0].as_str().or(line["handle"].as_str()).unwrap()).unwrap();
    s.set_selection(vec![h, line]);
    s.cmdline("BURST").unwrap();
    assert!(s.current_prompt().is_none());
    let d = s.doc().unwrap();
    assert!(d.entity(h).is_none() && d.entity(line).is_some());
    assert_eq!(d.model.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))).count(), 1, "only NUT is left as a reference");
    // Nothing to burst is an error in the JSON form.
    assert!(s.execute("burst", &json!({ "handles": [line.hex()] })).is_err());
}

fn add_text(s: &mut Session, t: Text, layer: &str) -> Handle {
    let d = s.doc_mut().unwrap();
    d.ensure_layer(layer);
    let h = d.new_handle();
    d.model.push(ent(h.0, layer, EntityKind::Text(t)));
    h
}

fn mtexts(s: &Session) -> Vec<(Entity, cadaddy_engine::doc::MText)> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::MText(m) = &e.kind { Some(((**e).clone(), m.clone())) } else { None }).collect()
}

#[test]
fn txt2mtxt_combines_lines_top_to_bottom() {
    let mut s = session();
    // Added out of order: the middle line first.
    let b = add_text(&mut s, text("second", Vec3::new(0.0, 9.0, 0.0), 0.5), "NOTES");
    let c = add_text(&mut s, text("third {x}", Vec3::new(0.0, 8.0, 0.0), 0.5), "OTHER");
    let a = add_text(&mut s, text("first", Vec3::new(0.0, 10.0, 0.0), 0.5), "NOTES");
    let r = s.execute("txt2mtxt", &json!({ "handles": [b.hex(), c.hex(), a.hex()] })).unwrap();
    assert_eq!(r["converted"], json!(3));
    let ms = mtexts(&s);
    assert_eq!(ms.len(), 1);
    let (e, m) = &ms[0];
    assert_eq!(m.contents, "first\\Psecond\\Pthird \\{x\\}");
    // Top line's layer, height and style; top-left corner at the top of the first line.
    assert_eq!((e.common.layer.as_str(), m.height, m.attach, m.width), ("NOTES", 0.5, 1, 0.0));
    assert!(close(m.insert, Vec3::new(0.0, 10.5, 0.0)), "{:?}", m.insert);
    // Baselines 1 apart = 1 / (5/3 · 0.5) = 1.2 × the standard spacing.
    assert!((m.line_spacing - 1.2).abs() < 1e-9, "{}", m.line_spacing);
    assert!([a, b, c].iter().all(|h| s.doc().unwrap().entity(*h).is_none()));
    // One undo step.
    s.undo().unwrap();
    assert!(mtexts(&s).is_empty());
    assert!([a, b, c].iter().all(|h| s.doc().unwrap().entity(*h).is_some()));
}

#[test]
fn txt2mtxt_orders_rotated_text_along_its_rotation_and_can_wrap() {
    let mut s = session();
    // Rotated 90°: "up" is -x, so the line at smaller x is on top.
    let mut t1 = text("lower", Vec3::new(1.0, 0.0, 0.0), 0.5);
    t1.rotation = std::f64::consts::FRAC_PI_2;
    let mut t2 = text("upper", Vec3::new(0.0, 0.0, 0.0), 0.5);
    t2.rotation = std::f64::consts::FRAC_PI_2;
    let h1 = add_text(&mut s, t1, "0");
    let h2 = add_text(&mut s, t2, "0");
    s.execute("txt2mtxt", &json!({ "handles": [h1.hex(), h2.hex()], "wordWrap": true })).unwrap();
    let (_, m) = mtexts(&s).pop().unwrap();
    assert_eq!(m.contents, "upper lower");
    assert!(m.width > 0.0);
    assert!((m.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
}

#[test]
fn txt2mtxt_not_combined_keeps_each_justification() {
    let mut s = session();
    let cases = [
        (HAlign::Left, VAlign::Baseline, 7u8),
        (HAlign::Center, VAlign::Top, 2),
        (HAlign::Right, VAlign::Middle, 6),
        (HAlign::Middle, VAlign::Baseline, 5),
        (HAlign::Right, VAlign::Bottom, 9),
    ];
    let mut hs = Vec::new();
    for (i, (h, v, _)) in cases.iter().enumerate() {
        let mut t = text(&format!("t{i}"), Vec3::new(0.0, i as f64 * 2.0, 0.0), 0.25);
        t.halign = *h;
        t.valign = *v;
        t.align_pt = Some(Vec3::new(5.0, i as f64 * 2.0, 0.0));
        let layer = format!("L{i}");
        hs.push(add_text(&mut s, t, &layer));
    }
    let r = s.execute("txt2mtxt", &json!({ "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(), "combine": false })).unwrap();
    assert_eq!(r["handles"].as_array().map(Vec::len), Some(cases.len()));
    for (i, (_, _, attach)) in cases.iter().enumerate() {
        let (e, m) = mtexts(&s).into_iter().find(|(_, m)| m.contents == format!("t{i}")).unwrap();
        assert_eq!(m.attach, *attach, "case {i}");
        assert_eq!(e.common.layer, format!("L{i}"));
        // Left-baseline text is placed by its start point, the others by their alignment point.
        let x = if i == 0 { 0.0 } else { 5.0 };
        assert!(close(m.insert, Vec3::new(x, i as f64 * 2.0, 0.0)), "case {i}: {:?}", m.insert);
    }
}

#[test]
fn txt2mtxt_typed_settings_and_preselection() {
    let mut s = session();
    let a = add_text(&mut s, text("one", Vec3::new(0.0, 1.0, 0.0), 0.25), "0");
    let b = add_text(&mut s, text("two", Vec3::new(0.0, 0.0, 0.0), 0.25), "0");
    s.cmdline("TXT2MTXT").unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Select objects or [SEttings]:");
    s.cmdline("SE").unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Combine text into one Mtext? [Yes/No] <Yes>:");
    s.cmdline("N").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![a, b])).unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(s.current_prompt().is_none());
    assert_eq!(mtexts(&s).len(), 2);

    // Pickfirst: runs at once with the defaults (combined).
    s.undo().unwrap();
    s.set_selection(vec![a, b]);
    s.cmdline("TXT2MTXT").unwrap();
    assert!(s.current_prompt().is_none());
    assert_eq!(mtexts(&s).len(), 1);
    // Nothing but non-text selected is an error in the JSON form.
    let v: Value = s.execute("line", &json!({ "points": [[0.0, 0.0], [1.0, 0.0]] })).unwrap();
    let line = v["handles"][0].as_str().or(v["handle"].as_str()).unwrap().to_string();
    assert!(s.execute("txt2mtxt", &json!({ "handles": [line] })).is_err());
}
