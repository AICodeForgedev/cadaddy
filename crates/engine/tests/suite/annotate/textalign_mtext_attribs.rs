//! SCALETEXT, JUSTIFYTEXT and TORIENT on MTEXT and on block references' attribute values, proven by
//! position: the text stays where it reads (exact values, 1e-6), not just "a property changed".

use cadaddy_doc::{EntityKind, HAlign, Handle, MText, Text, VAlign};
use cadaddy_engine::Session;
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use serde_json::{Value, json};
use std::f64::consts::{FRAC_PI_2, PI};

const TOL: f64 = 1e-6;

fn handle(r: &Value) -> Handle {
    Handle::parse_hex(r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap()).unwrap()
}

fn close(a: Vec2, b: Vec2) -> bool {
    a.dist(b) < TOL
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < TOL
}

// ---------- MTEXT ----------

fn add_mtext(s: &mut Session, at: [f64; 2], text: &str, height: f64, width: f64, rotation_deg: f64, justify: &str) -> Handle {
    handle(
        &s.execute("mtext", &json!({"at": at, "text": text, "height": height, "width": width, "rotation": rotation_deg, "justify": justify}))
            .unwrap(),
    )
}

fn mtext(s: &Session, h: Handle) -> MText {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::MText(m) => m.clone(),
        other => panic!("not mtext: {other:?}"),
    }
}

/// The MTEXT's laid-out box, counterclockwise from its bottom left, in the world: the reference
/// width (the glyph width when it doesn't wrap) by the laid-out height, hung from the attachment point.
fn mbox(s: &Session, m: &MText) -> [Vec2; 4] {
    let flat = MText { rotation: 0.0, attach: 1, insert: Vec3::ZERO, ..m.clone() };
    let b = cadaddy_render::layout_mtext_entity(s.doc().unwrap(), &flat).bounds;
    let (w, h) = (if m.width > 0.0 { m.width } else { b.width() }, b.height());
    let i = f64::from(m.attach - 1);
    let (dir, up) = (Vec2::from_angle(m.rotation), Vec2::from_angle(m.rotation).perp());
    let tl = m.insert.xy() - (dir * ((i % 3.0) * w / 2.0) + up * (-(i / 3.0).floor() * h / 2.0));
    [tl - up * h, tl - up * h + dir * w, tl + dir * w, tl]
}

fn top_left(b: &[Vec2; 4]) -> Vec2 {
    b[3]
}
fn bottom_right(b: &[Vec2; 4]) -> Vec2 {
    b[1]
}
fn centre(b: &[Vec2; 4]) -> Vec2 {
    (b[0] + b[2]) * 0.5
}

#[test]
fn scaletext_on_mtext_scales_height_and_width_and_keeps_the_base() {
    let mut s = Session::new();
    for rot in [0.0, 30.0, 200.0] {
        // Existing: the attachment point (m.insert) stays; height and width scale by k.
        let h = add_mtext(&mut s, [10.0, 5.0], "Panel LP-1\nFed from MDP", 2.0, 30.0, rot, "MC");
        let before = mtext(&s, h);
        let box0 = mbox(&s, &before);
        s.execute("scaletext", &json!({"handles": [h.hex()], "factor": 2.5})).unwrap();
        let after = mtext(&s, h);
        assert!(near(after.height, before.height * 2.5), "{rot}° height {}", after.height);
        assert!(near(after.width, before.width * 2.5), "{rot}° width {}", after.width);
        assert_eq!(after.attach, before.attach);
        assert!(near(after.rotation, before.rotation));
        assert!(close(after.insert.xy(), before.insert.xy()), "{rot}° existing moved insert: {:?}", after.insert);
        let box1 = mbox(&s, &after);
        assert!(near(box1[0].dist(box1[1]), box0[0].dist(box0[1]) * 2.5), "{rot}° box width");
        assert!(near(box1[1].dist(box1[2]), box0[1].dist(box0[2]) * 2.5), "{rot}° box height");
        // `base: existing` spelled out is the same.
        s.execute("scaletext", &json!({"handles": [h.hex()], "factor": 0.4, "base": "existing"})).unwrap();
        let back = mtext(&s, h);
        assert!(near(back.height, before.height) && near(back.width, before.width));
        assert!(close(back.insert.xy(), before.insert.xy()));

        // A height target scales by height/old height.
        s.execute("scaletext", &json!({"handles": [h.hex()], "height": 3.0})).unwrap();
        let by_height = mtext(&s, h);
        assert!(near(by_height.height, 3.0) && near(by_height.width, before.width * 1.5));
        assert!(close(by_height.insert.xy(), before.insert.xy()));

        // Top left / bottom right: that corner of the box stays put.
        for (base, corner) in [("tl", top_left as fn(&[Vec2; 4]) -> Vec2), ("br", bottom_right as fn(&[Vec2; 4]) -> Vec2)] {
            let m0 = mtext(&s, h);
            let b0 = mbox(&s, &m0);
            s.execute("scaletext", &json!({"handles": [h.hex()], "factor": 2.0, "base": base})).unwrap();
            let m1 = mtext(&s, h);
            let b1 = mbox(&s, &m1);
            assert!(near(m1.height, m0.height * 2.0) && near(m1.width, m0.width * 2.0), "{rot}° {base}");
            assert!(close(corner(&b0), corner(&b1)), "{rot}° {base}: {:?} → {:?}", corner(&b0), corner(&b1));
            // The box really grew about that corner: the centre moved away from it by 2x.
            assert!(close(centre(&b1) - corner(&b1), (centre(&b0) - corner(&b0)) * 2.0), "{rot}° {base} centre");
        }
    }
}

#[test]
fn torient_on_mtext_turns_it_readable_about_the_box_centre() {
    let mut s = Session::new();
    for (rot, readable) in [(180.0_f64, 0.0_f64), (260.0, 80.0)] {
        for attach in ["TL", "MC", "BR"] {
            let h = add_mtext(&mut s, [7.0, -3.0], "Feeder schedule\nNote 4", 1.5, 0.0, rot, attach);
            let before = mtext(&s, h);
            let c0 = centre(&mbox(&s, &before));
            s.execute("torient", &json!({"handles": [h.hex()]})).unwrap();
            let after = mtext(&s, h);
            assert!(near(after.rotation.rem_euclid(2.0 * PI), readable.to_radians()), "{rot}° {attach}: rotation {}", after.rotation.to_degrees());
            assert!(near(after.height, before.height) && after.attach == before.attach);
            let c1 = centre(&mbox(&s, &after));
            assert!(close(c0, c1), "{rot}° {attach}: centre {c0:?} → {c1:?}");
        }
    }
    // Already readable MTEXT (0°, 80°) doesn't turn.
    for rot in [0.0_f64, 80.0] {
        let h = add_mtext(&mut s, [1.0, 1.0], "ok", 1.0, 0.0, rot, "TL");
        let before = mtext(&s, h);
        s.execute("torient", &json!({"handles": [h.hex()]})).unwrap();
        let after = mtext(&s, h);
        assert!(near(after.rotation, rot.to_radians()) && close(after.insert.xy(), before.insert.xy()), "{rot}°");
    }
    // An explicit angle sets exactly that rotation, about the same centre.
    let h = add_mtext(&mut s, [20.0, 20.0], "Fixed\nangle", 2.0, 12.0, 33.0, "TR");
    let c0 = centre(&mbox(&s, &mtext(&s, h)));
    s.execute("torient", &json!({"handles": [h.hex()], "angle": 90})).unwrap();
    let m = mtext(&s, h);
    assert!(near(m.rotation, FRAC_PI_2), "rotation {}", m.rotation);
    assert!(close(c0, centre(&mbox(&s, &m))));
}

// ---------- block references' attribute values ----------

/// A block B with left-justified attributes T and U, and a reference to it at `at`.
fn block_ref(s: &mut Session, at: [f64; 2]) -> Handle {
    if s.doc().unwrap().block("B").is_none() {
        s.execute("attdef", &json!({"tag": "T", "default": "tee", "at": [0, 1]})).unwrap();
        s.execute("attdef", &json!({"tag": "U", "default": "you longer", "at": [3, 4]})).unwrap();
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        s.execute("block", &json!({"name": "B", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    handle(&s.execute("insert", &json!({"name": "B", "at": at})).unwrap())
}

fn attribs(s: &Session, h: Handle) -> Vec<Text> {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Insert(i) => i.attribs.iter().map(|a| a.text.clone()).collect(),
        other => panic!("not an insert: {other:?}"),
    }
}

/// Where an attribute value's glyphs are, as the renderer draws them.
fn glyph_box(s: &Session, t: &Text) -> Bounds2 {
    let sh = cadaddy_render::place_text_entity(s.doc().unwrap(), t, &t.value).0;
    Bounds2::from_points(sh.strokes.iter().flatten().copied().chain(sh.glyphs.iter().flatten().flatten().copied()))
}

fn same_box(a: Bounds2, b: Bounds2) -> bool {
    a.min.dist(b.min) < TOL && a.max.dist(b.max) < TOL
}

/// The middle of an attribute value's frame (its MC point), whatever its justification.
fn frame_centre(s: &Session, t: &Text) -> Vec2 {
    let flat = Text { rotation: 0.0, halign: HAlign::Left, valign: VAlign::Baseline, align_pt: None, ..t.clone() };
    let w = cadaddy_render::place_text_entity(s.doc().unwrap(), &flat, &t.value).0.width;
    let (dir, up) = (Vec2::from_angle(t.rotation), Vec2::from_angle(t.rotation).perp());
    let x = match t.halign {
        HAlign::Center | HAlign::Middle => w / 2.0,
        HAlign::Right => w,
        _ => 0.0,
    };
    let y = match (t.halign, t.valign) {
        (HAlign::Middle, _) | (_, VAlign::Middle) => t.height / 2.0,
        (_, VAlign::Top) => t.height,
        (_, VAlign::Bottom) => -t.height / 3.0,
        (_, VAlign::Baseline) => 0.0,
    };
    // `justify_point` is the point the justification pins; step from it to the middle.
    t.justify_point() + dir * (w / 2.0 - x) + up * (t.height / 2.0 - y)
}

#[test]
fn scaletext_keeps_each_attributes_justification_point() {
    let mut s = Session::new();
    let i = block_ref(&mut s, [10.0, 20.0]);
    let seed = attribs(&s, i);
    assert_eq!(seed.len(), 2);
    for (just, h, v) in [("left", HAlign::Left, VAlign::Baseline), ("mc", HAlign::Center, VAlign::Middle), ("br", HAlign::Right, VAlign::Bottom)] {
        s.execute("justifytext", &json!({"handles": [i.hex()], "justify": just})).unwrap();
        let before = attribs(&s, i);
        s.execute("scaletext", &json!({"handles": [i.hex()], "factor": 2.0})).unwrap();
        let after = attribs(&s, i);
        for (b, a) in before.iter().zip(&after) {
            assert!(close(a.justify_point(), b.justify_point()), "{just}: {:?} → {:?}", b.justify_point(), a.justify_point());
            assert_eq!((a.halign, a.valign), (h, v), "{just}");
            assert!(near(a.height, b.height * 2.0), "{just}: height {}", a.height);
            assert!(near(a.rotation, b.rotation));
            // And the glyphs grew about that point: every box corner is twice as far from it.
            let (g0, g1) = (glyph_box(&s, b), glyph_box(&s, a));
            let p = b.justify_point();
            assert!(close(g1.min - p, (g0.min - p) * 2.0) && close(g1.max - p, (g0.max - p) * 2.0), "{just}: {g0:?} → {g1:?}");
        }
        // Back to the starting height for the next justification.
        s.execute("scaletext", &json!({"handles": [i.hex()], "factor": 0.5})).unwrap();
        for (a, b) in attribs(&s, i).iter().zip(&before) {
            assert!(close(a.justify_point(), b.justify_point()) && near(a.height, b.height), "{just}: restore");
        }
    }
}

#[test]
fn justifytext_mc_on_attributes_is_one_exact_pair_and_the_glyphs_stay() {
    let mut s = Session::new();
    let i = block_ref(&mut s, [10.0, 20.0]);
    // Rotate the values first so the box isn't axis-aligned.
    s.execute("torient", &json!({"handles": [i.hex()], "angle": 30})).unwrap();
    let before: Vec<Bounds2> = attribs(&s, i).iter().map(|t| glyph_box(&s, t)).collect();
    s.execute("justifytext", &json!({"handles": [i.hex()], "justify": "mc"})).unwrap();
    let after = attribs(&s, i);
    assert_eq!(after.len(), 2);
    for (t, b) in after.iter().zip(&before) {
        assert_eq!((t.halign, t.valign), (HAlign::Center, VAlign::Middle));
        assert!(same_box(*b, glyph_box(&s, t)), "{b:?} → {:?}", glyph_box(&s, t));
        assert!(near(t.rotation, 30.0_f64.to_radians()));
    }
    // The pinned point is the middle of the frame, at the same place as before.
    let mid: Vec<Vec2> = after.iter().map(|t| frame_centre(&s, t)).collect();
    for (t, m) in after.iter().zip(&mid) {
        assert!(close(t.justify_point(), *m));
    }
    // Other justifications keep the glyphs too.
    for j in ["tl", "br", "right", "left", "bc"] {
        s.execute("justifytext", &json!({"handles": [i.hex()], "justify": j})).unwrap();
        for (t, b) in attribs(&s, i).iter().zip(&before) {
            assert!(same_box(*b, glyph_box(&s, t)), "{j}");
        }
    }
}

#[test]
fn torient_keeps_each_attributes_frame_centre() {
    let mut s = Session::new();
    let i = block_ref(&mut s, [10.0, 20.0]);
    for just in ["left", "mc", "br"] {
        s.execute("justifytext", &json!({"handles": [i.hex()], "justify": just})).unwrap();
        let centres: Vec<Vec2> = attribs(&s, i).iter().map(|t| frame_centre(&s, t)).collect();
        for (angle, expect) in [(Some(180.0_f64), PI), (None, 0.0), (Some(37.0), 37.0_f64.to_radians()), (Some(90.0), FRAC_PI_2)] {
            let p = match angle {
                Some(a) => json!({"handles": [i.hex()], "angle": a}),
                None => json!({"handles": [i.hex()]}),
            };
            s.execute("torient", &p).unwrap();
            for (t, c) in attribs(&s, i).iter().zip(&centres) {
                assert!(near(t.rotation.rem_euclid(2.0 * PI), expect), "{just} {angle:?}: rotation {}", t.rotation);
                assert!(close(frame_centre(&s, t), *c), "{just} {angle:?}: centre {c:?} → {:?}", frame_centre(&s, t));
            }
        }
    }
}
