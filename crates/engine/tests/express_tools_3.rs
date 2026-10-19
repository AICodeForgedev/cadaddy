//! Express Tools III: TCASE (text case), TCOUNT (auto number), TXTEXP (explode text) and
//! ARCTEXT (text along an arc, as a group of characters), by JSON and at the command line.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    let t = v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap();
    Handle(u64::from_str_radix(t, 16).unwrap())
}

fn text(s: &mut Session, at: [f64; 2], t: &str) -> Handle {
    h(&s.execute("text", &json!({"at": at, "text": t, "height": 1})).unwrap())
}

fn value(s: &Session, h: Handle) -> String {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Text(t) => t.value.clone(),
        EntityKind::MText(t) => t.contents.clone(),
        k => panic!("{k:?}"),
    }
}

fn hexes(hs: &[Handle]) -> Value {
    json!(hs.iter().map(|h| h.hex()).collect::<Vec<_>>())
}

#[test]
fn tcase_changes_text_mtext_and_attributes() {
    let mut s = Session::new();
    let t = text(&mut s, [0.0, 0.0], "the PANEL. second line");
    let m = h(&s.execute("mtext", &json!({"at": [0, 10], "text": "{\\fArial|b1;mixed Case}\\Pnext para", "height": 1, "width": 20})).unwrap());
    s.execute("attdef", &json!({"tag": "NAME", "default": "x", "at": [0, 0], "height": 1})).unwrap();
    let all: Vec<Handle> = s.doc().unwrap().model.iter().map(|e| e.handle).collect();
    s.execute("block", &json!({"name": "TAGB", "base": [0, 0], "handles": [all.last().unwrap().hex()]})).unwrap();
    let ins = h(&s.execute("insert", &json!({"name": "TAGB", "at": [50, 50], "attribs": {"NAME": "rack one"}})).unwrap());

    s.execute("tcase", &json!({"handles": hexes(&[t, m, ins]), "case": "upper"})).unwrap();
    assert_eq!(value(&s, t), "THE PANEL. SECOND LINE");
    assert_eq!(value(&s, m), "{\\fArial|b1;MIXED CASE}\\PNEXT PARA", "format codes kept");
    let EntityKind::Insert(i) = &s.doc().unwrap().entity(ins).unwrap().kind else { panic!() };
    assert_eq!(i.attribs[0].text.value, "RACK ONE");

    s.execute("tcase", &json!({"handles": hexes(&[t]), "case": "sentence"})).unwrap();
    assert_eq!(value(&s, t), "The panel. Second line");
    s.execute("tcase", &json!({"handles": hexes(&[t]), "case": "title"})).unwrap();
    assert_eq!(value(&s, t), "The Panel. Second Line");
    s.execute("tcase", &json!({"handles": hexes(&[t]), "case": "toggle"})).unwrap();
    assert_eq!(value(&s, t), "tHE pANEL. sECOND lINE");
    // Undo is one step.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(value(&s, t), "The Panel. Second Line");

    // At the command line: select, then the case keyword.
    s.set_selection(vec![t]);
    s.cmdline("tcase").unwrap();
    assert!(s.prompt_text().contains("[Sentence/Lower/Upper/Title/tOggle]"), "{}", s.prompt_text());
    s.cmdline("l").unwrap();
    assert_eq!(value(&s, t), "the panel. second line");

    // Hostile and empty.
    assert!(s.execute("tcase", &json!({"handles": hexes(&[t]), "case": "shouty"})).is_err());
    let line = h(&s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap());
    assert!(s.execute("tcase", &json!({"handles": hexes(&[line])})).is_err());
}

#[test]
fn tcount_numbers_in_order_with_each_placement() {
    let mut s = Session::new();
    // Three labels in a column, top to bottom c, b, a; created bottom first.
    let a = text(&mut s, [5.0, 0.0], "LT");
    let b = text(&mut s, [0.0, 5.0], "LT");
    let c = text(&mut s, [10.0, 10.0], "LT");
    let r = s.execute("tcount", &json!({"handles": hexes(&[a, b, c])})).unwrap();
    assert_eq!(r["handles"], hexes(&[c, b, a]), "sorted top to bottom");
    assert_eq!((value(&s, c), value(&s, b), value(&s, a)), ("1 LT".into(), "2 LT".into(), "3 LT".into()));

    // Left to right from 10 by 5, as a suffix.
    s.execute("tcount", &json!({"handles": hexes(&[a, b, c]), "sort": "x", "start": 10, "increment": 5, "placement": "suffix"})).unwrap();
    assert_eq!(value(&s, b), "2 LT 10");
    assert_eq!(value(&s, a), "3 LT 15");
    assert_eq!(value(&s, c), "1 LT 20");
    // Overwrite in selection order.
    s.execute("tcount", &json!({"handles": hexes(&[a, c, b]), "sort": "select", "placement": "overwrite", "start": 7})).unwrap();
    assert_eq!((value(&s, a), value(&s, c), value(&s, b)), ("7".into(), "8".into(), "9".into()));
    // Find & replace.
    let d1 = text(&mut s, [0.0, 30.0], "CKT-#");
    let d2 = text(&mut s, [0.0, 20.0], "CKT-#");
    s.execute("tcount", &json!({"handles": hexes(&[d1, d2]), "placement": "find", "find": "#", "start": 2, "increment": 2})).unwrap();
    assert_eq!((value(&s, d1), value(&s, d2)), ("CKT-2".into(), "CKT-4".into()));

    // At the command line: X order, 100,10, Prefix.
    s.set_selection(vec![a, b, c]);
    for t in ["tcount", "x", "100,10", "p"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.current_prompt().is_none(), "{}", s.prompt_text());
    assert_eq!(value(&s, b), "100 9");

    for bad in [
        json!({"handles": hexes(&[a]), "sort": "z"}),
        json!({"handles": hexes(&[a]), "placement": "find"}),
        json!({"handles": hexes(&[a]), "start": 1e300}),
        json!({"handles": hexes(&[a]), "placement": "sideways"}),
    ] {
        assert!(s.execute("tcount", &bad).is_err(), "{bad}");
    }
}

#[test]
fn txtexp_turns_text_into_polylines_on_its_layer() {
    let mut s = Session::new();
    s.execute("layer.make", &json!({"name": "NOTES"})).ok();
    let t = text(&mut s, [0.0, 0.0], "HELLO");
    s.execute("chprop", &json!({"handles": [t.hex()], "layer": "NOTES"})).ok();
    let layer = s.doc().unwrap().entity(t).unwrap().common.layer.clone();
    let r = s.execute("txtexp", &json!({"handles": [t.hex()]})).unwrap();
    let made = r["handles"].as_array().unwrap();
    assert!(made.len() >= 5, "{r}");
    let d = s.doc().unwrap();
    assert!(d.entity(t).is_none(), "the text is gone");
    let mut bounds = cadaddy_engine::geom::Bounds2::EMPTY;
    for v in made {
        let e = d.entity(Handle(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap())).unwrap();
        assert_eq!(e.common.layer, layer);
        let EntityKind::LwPolyline(p) = &e.kind else { panic!("{:?}", e.kind) };
        for v in &p.vertices {
            bounds = bounds.union(&cadaddy_engine::geom::Bounds2::from_points([v.p]));
        }
    }
    // Five letters one unit tall, from the insertion point rightwards.
    assert!(bounds.min.x >= -0.1 && bounds.max.x > 3.0 && (bounds.height() - 1.0).abs() < 0.2, "{bounds:?}");
    // MTEXT too; a line is refused.
    let m = h(&s.execute("mtext", &json!({"at": [0, 10], "text": "AB\\PCD", "height": 1, "width": 20})).unwrap());
    assert!(s.execute("txtexp", &json!({"handles": [m.hex()]})).unwrap()["handles"].as_array().unwrap().len() >= 4);
    let line = h(&s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap());
    assert!(s.execute("txtexp", &json!({"handles": [line.hex()]})).is_err());
    // Undo brings the text back.
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert!(s.doc().unwrap().entity(m).is_some());
}

#[test]
fn arctext_places_characters_along_the_arc_as_a_group() {
    let mut s = Session::new();
    let arc = h(&s.execute("arc", &json!({"center": [0, 0], "radius": 10, "start": 0, "end": 180})).unwrap());
    let r = s.execute("arctext", &json!({"arc": arc.hex(), "text": "ABC", "height": 1})).unwrap();
    let hs: Vec<Handle> = r["handles"].as_array().unwrap().iter().map(|v| Handle(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap())).collect();
    assert_eq!(hs.len(), 3);
    let d = s.doc().unwrap();
    let chars: Vec<(String, Vec2, f64)> = hs
        .iter()
        .map(|h| match &d.entity(*h).unwrap().kind {
            EntityKind::Text(t) => (t.value.clone(), t.justify_point(), t.rotation),
            k => panic!("{k:?}"),
        })
        .collect();
    // Fit, convex: A at the left end, B at the top (upright), C at the right end; reading clockwise.
    assert_eq!(chars.iter().map(|c| c.0.as_str()).collect::<String>(), "ABC");
    assert!(chars[1].1.near(Vec2::new(0.0, 10.0), 1e-6) && chars[1].2.abs() < 1e-9, "{chars:?}");
    assert!(chars[0].1.x < -5.0 && chars[2].1.x > 5.0);
    for c in &chars {
        assert!((c.1.len() - 10.0).abs() < 1e-6, "on the arc: {c:?}");
    }
    let g = r["group"].as_str().unwrap();
    assert!(d.groups.iter().any(|gr| gr.name == g && gr.members == hs), "kept together as a group");

    // Concave, centred, inside by 1: reads counterclockwise along the bottom of a bowl.
    let bowl = h(&s.execute("arc", &json!({"center": [0, 0], "radius": 10, "start": 180, "end": 360})).unwrap());
    let r = s.execute("arctext", &json!({"arc": bowl.hex(), "text": "X Y", "height": 1, "side": "concave", "align": "center", "offset": 1})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2, "spaces leave a gap, not an object");

    // Too long, hostile, not an arc.
    let line = h(&s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap());
    for bad in [
        json!({"arc": arc.hex(), "text": "A".repeat(200), "height": 1, "align": "left"}),
        json!({"arc": arc.hex(), "text": "A".repeat(200), "height": 1}),
        json!({"arc": arc.hex(), "text": "", "height": 1}),
        json!({"arc": arc.hex(), "text": "AB", "height": -1}),
        json!({"arc": arc.hex(), "text": "AB", "side": "concave", "offset": 50}),
        json!({"arc": arc.hex(), "text": "AB", "align": "justified"}),
        json!({"arc": line.hex(), "text": "AB"}),
    ] {
        assert!(s.execute("arctext", &bad).is_err(), "{bad}");
    }

    // At the command line: pick the arc, type the text.
    s.execute("arc", &json!({"center": [100, 0], "radius": 5, "start": 0, "end": 180})).unwrap();
    s.cmdline("arctext").unwrap();
    s.cmdline("100,5").unwrap();
    s.cmdline("HI").unwrap();
    assert!(s.current_prompt().is_none(), "{}", s.prompt_text());
}

#[test]
fn express_text_tools_are_on_the_express_tools_menu() {
    for (id, label) in [("tcase", "Change Text Case"), ("tcount", "Auto Number"), ("txtexp", "Explode Text"), ("arctext", "Arc Aligned Text")] {
        let spec = cadaddy_engine::cmd::find_command(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(spec.label, label);
        assert_eq!(spec.menu, ["Tools", "Express Tools", label]);
    }
}
