//! TEXTALIGN with mixed TEXT, MTEXT, attribute definitions and block references' attributes (a
//! column, distributed, set spacing, along a slanted line, centre and right ends, typed); and
//! SCALETEXT, JUSTIFYTEXT and TORIENT on block references' attributes.

use cadaddy_doc::{EntityKind, HAlign, Handle, Text};
use cadaddy_engine::Session;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

fn handle(r: &Value) -> Handle {
    Handle::parse_hex(r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap()).unwrap()
}

fn add_text(s: &mut Session, at: [f64; 2], value: &str, height: f64, justify: &str) -> Handle {
    handle(&s.execute("text", &json!({"at": at, "text": value, "height": height, "justify": justify})).unwrap())
}

/// A single-line text's width as drawn.
fn width(s: &Session, t: &Text) -> f64 {
    let flat = Text { rotation: 0.0, halign: HAlign::Left, valign: cadaddy_doc::VAlign::Baseline, align_pt: None, ..t.clone() };
    cadaddy_render::place_text_entity(s.doc().unwrap(), &flat, &t.value).0.width
}

/// The left end of the baseline of an unrotated single-line text.
fn left(s: &Session, t: &Text) -> Vec2 {
    let w = width(s, t);
    match t.halign {
        HAlign::Center | HAlign::Middle => t.justify_point() - Vec2::new(w / 2.0, 0.0),
        HAlign::Right => t.justify_point() - Vec2::new(w, 0.0),
        _ => t.insert.xy(),
    }
}

fn kind(s: &Session, h: Handle) -> EntityKind {
    s.doc().unwrap().entity(h).unwrap().kind.clone()
}

fn text(s: &Session, h: Handle) -> Text {
    match kind(s, h) {
        EntityKind::Text(t) => t,
        EntityKind::AttDef(a) => a.text,
        other => panic!("not text: {other:?}"),
    }
}

/// A block B with attributes T and U (left-justified), and a reference to it at `at`.
fn block_ref(s: &mut Session, at: [f64; 2]) -> Handle {
    if s.doc().unwrap().block("B").is_none() {
        s.execute("attdef", &json!({"tag": "T", "default": "tee", "at": [0, 1]})).unwrap();
        s.execute("attdef", &json!({"tag": "U", "default": "you", "at": [3, 4]})).unwrap();
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        s.execute("block", &json!({"name": "B", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    handle(&s.execute("insert", &json!({"name": "B", "at": at})).unwrap())
}

fn attribs(s: &Session, h: Handle) -> Vec<Text> {
    match kind(s, h) {
        EntityKind::Insert(i) => i.attribs.into_iter().map(|a| a.text).collect(),
        other => panic!("not an insert: {other:?}"),
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_column_of_mixed_texts_lines_up_on_the_first() {
    let mut s = Session::new();
    // The block first (it is made from everything drawn so far).
    let i = block_ref(&mut s, [60.0, 40.0]);
    let r = s.execute("text", &json!({"at": [10, 0], "text": "Reference", "height": 2.5})).unwrap();
    let a = handle(&r);
    let b = add_text(&mut s, [30.0, 5.0], "centred", 2.5, "C");
    let m = handle(&s.execute("mtext", &json!({"at": [5, 20], "text": "Two\\Plines", "height": 2, "width": 30})).unwrap());
    let ad = handle(&s.execute("attdef", &json!({"tag": "X", "default": "def", "at": [40, -5]})).unwrap());
    let before: Vec<f64> = attribs(&s, i).iter().map(|t| t.insert.y).collect();
    let r = s.execute("textalign", &json!({"handles": [a.hex(), b.hex(), m.hex(), ad.hex(), i.hex()]})).unwrap();
    assert_eq!(r["changed"].as_array().unwrap().len(), 4, "the first stays: {r}");
    // Every left end on x = 10; each kept its height on the page.
    assert!(near(left(&s, &text(&s, a)).x, 10.0) && near(text(&s, a).insert.y, 0.0));
    let tb = text(&s, b);
    assert!(near(left(&s, &tb).x, 10.0) && near(tb.justify_point().y, 5.0), "{:?}", left(&s, &tb));
    let EntityKind::MText(mt) = kind(&s, m) else { panic!() };
    assert!(near(mt.insert.x, 10.0) && near(mt.insert.y, 20.0), "{:?}", mt.insert);
    assert!(near(text(&s, ad).insert.x, 10.0) && near(text(&s, ad).insert.y, -5.0));
    let at = attribs(&s, i);
    assert!(at.iter().all(|t| near(t.insert.x, 10.0)), "{:?}", at.iter().map(|t| t.insert).collect::<Vec<_>>());
    assert_eq!(at.iter().map(|t| t.insert.y).collect::<Vec<_>>(), before, "attributes keep their heights");
}

#[test]
fn distribute_and_set_spacing_place_texts_along_the_line() {
    let mut s = Session::new();
    let hs: Vec<Handle> = [(0.0, 0.0), (7.0, 3.0), (-4.0, 11.0)].iter().map(|(x, y)| add_text(&mut s, [*x, *y], "abc", 1.0, "L")).collect();
    let hex: Vec<String> = hs.iter().map(|h| h.hex()).collect();
    s.execute("textalign", &json!({"handles": hex, "mode": "distribute", "end": [0, 30]})).unwrap();
    // Half way up each text: from the first's (0.5) evenly to the end (30).
    let ys: Vec<f64> = hs.iter().map(|h| text(&s, *h).insert.y + 0.5).collect();
    assert!(near(ys[0], 0.5) && near(ys[1], 15.25) && near(ys[2], 30.0), "{ys:?}");
    assert!(hs.iter().all(|h| near(text(&s, *h).insert.x, 0.0)));
    // Set spacing: 4 apart from the first, each staying on its side of it and in its order (here
    // above it, though the line points down).
    s.execute("textalign", &json!({"handles": hex, "spacing": 4, "angle": 270})).unwrap();
    let ys: Vec<f64> = hs.iter().map(|h| text(&s, *h).insert.y).collect();
    assert!(near(ys[0], 0.0) && near(ys[1], 4.0) && near(ys[2], 8.0), "{ys:?}");
    // The first in the middle: one each side.
    s.execute("textalign", &json!({"handles": [hex[1], hex[0], hex[2]], "spacing": 2.5})).unwrap();
    let ys: Vec<f64> = hs.iter().map(|h| text(&s, *h).insert.y).collect();
    assert!(near(ys[1], 4.0) && near(ys[0], 1.5) && near(ys[2], 6.5), "{ys:?}");
}

#[test]
fn a_slanted_line_and_centre_and_right_ends() {
    let mut s = Session::new();
    let a = add_text(&mut s, [0.0, 0.0], "a", 1.0, "L");
    let b = add_text(&mut s, [20.0, 3.0], "a much longer one", 1.0, "L");
    let c = add_text(&mut s, [-9.0, 14.0], "mid", 1.0, "L");
    let hex = [a.hex(), b.hex(), c.hex()];
    // The line runs from the first's left end half way up, (0, 0.5), at 45°.
    s.execute("textalign", &json!({"handles": hex, "through": [10, 10.5]})).unwrap();
    // All left ends (half way up) on it.
    let o = left(&s, &text(&s, a)) + Vec2::new(0.0, 0.5);
    for h in [b, c] {
        let p = left(&s, &text(&s, h)) + Vec2::new(0.0, 0.5) - o;
        assert!(near(p.x, p.y), "{h:?}: {p:?}");
    }
    for (alignment, at) in [("right", 1.0), ("center", 0.5)] {
        s.execute("textalign", &json!({"handles": hex, "alignment": alignment})).unwrap();
        let end = |h: Handle| {
            let t = text(&s, h);
            left(&s, &t).x + width(&s, &t) * at
        };
        assert!(near(end(b), end(a)) && near(end(c), end(a)), "{alignment}");
    }
}

#[test]
fn typed_and_bad_input() {
    let mut s = Session::new();
    let a = add_text(&mut s, [0.0, 0.0], "one", 1.0, "L");
    let b = add_text(&mut s, [5.0, 9.0], "three", 2.0, "L");
    s.set_selection(vec![a, b]);
    // Centre, along a horizontal line, keeping the spacing.
    s.cmdline("textalign").unwrap();
    for answer in ["C", "0", ""] {
        s.cmdline(answer).unwrap();
    }
    let mid = |h: Handle| {
        let t = text(&s, h);
        t.insert.y + t.height / 2.0
    };
    assert!(near(mid(a), mid(b)), "{} {}", mid(a), mid(b));
    let hex = [a.hex(), b.hex()];
    for bad in [
        json!({"handles": hex, "alignment": "justified"}),
        json!({"handles": hex, "mode": "random"}),
        json!({"handles": hex, "mode": "distribute"}),
        json!({"handles": hex, "mode": "spacing"}),
        json!({"handles": hex, "spacing": "wide"}),
        json!({"handles": hex, "angle": "up"}),
        json!({"handles": hex, "through": [-100000.0, -100000.0], "to": "ZZZZ"}),
        json!({"handles": []}),
    ] {
        assert!(s.execute("textalign", &bad).is_err(), "{bad}");
    }
    // A point on the text aligned to can't give a direction.
    let at = left(&s, &text(&s, a)) + Vec2::new(0.0, 0.5);
    assert!(s.execute("textalign", &json!({"handles": hex, "through": [at.x, at.y]})).is_err());
}

#[test]
fn scale_justify_and_orient_change_attribute_values() {
    let mut s = Session::new();
    let i = block_ref(&mut s, [0.0, 0.0]);
    let t = add_text(&mut s, [50.0, 0.0], "plain", 1.0, "L");
    let hex = [i.hex(), t.hex()];
    let r = s.execute("scaletext", &json!({"handles": hex, "height": 3})).unwrap();
    assert_eq!(r["changed"].as_array().unwrap().len(), 2);
    assert!(attribs(&s, i).iter().all(|a| near(a.height, 3.0)) && near(text(&s, t).height, 3.0));
    s.execute("justifytext", &json!({"handles": hex, "justify": "mc"})).unwrap();
    assert!(attribs(&s, i).iter().all(|a| a.halign == HAlign::Middle || a.halign == HAlign::Center));
    // Upside-down attribute values turn readable.
    s.execute("torient", &json!({"handles": [i.hex()], "angle": 180})).unwrap();
    assert!(attribs(&s, i).iter().all(|a| near(a.rotation, std::f64::consts::PI)));
    s.execute("torient", &json!({"handles": [i.hex()]})).unwrap();
    assert!(attribs(&s, i).iter().all(|a| a.rotation.abs() < 1e-9 || near(a.rotation, std::f64::consts::TAU)));
    // A block reference without attributes isn't text.
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    let hs: Vec<String> = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": "NOATT", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    let plain = handle(&s.execute("insert", &json!({"name": "NOATT", "at": [9, 9]})).unwrap());
    assert!(s.execute("scaletext", &json!({"handles": [plain.hex()], "height": 2})).is_err());
    // TEXTMASK still takes text only.
    assert!(s.execute("textmask", &json!({"handles": [i.hex()]})).is_err());
}
