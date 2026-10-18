//! Match Properties and draw order leftovers (gaps F19): MATCHPROP's settings include material,
//! shadow display and centre objects and its settings prompt offers the Property Settings dialog;
//! TEXTTOFRONT and HATCHTOBACK act on every space and block; OVERKILL deletes lines and arcs that
//! lie on a polyline's segments.

use cadaddy_engine::doc::{EntityKind, Handle, Space};
use cadaddy_engine::{Input, Picker, Session};
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    let x = v.get("handle").or_else(|| v.get("handles").and_then(|a| a.get(0))).and_then(Value::as_str).unwrap();
    Handle::parse_hex(x).unwrap()
}

fn exists(s: &Session, x: Handle) -> bool {
    s.doc().unwrap().entity(x).is_some()
}

// ---------- MATCHPROP ----------

#[test]
fn matchprop_settings_include_the_new_properties_and_offer_the_dialog() {
    let mut s = Session::new();
    let a = h(&s.execute("line", &json!({"points": [[0, 0], [1, 0]]})).unwrap());
    s.cmdline("matchprop").unwrap();
    s.input(Input::Pick(vec![a])).unwrap();
    s.input(Input::Keyword("Settings".into())).unwrap();
    let p = s.current_prompt().unwrap();
    assert_eq!(p.picker, Some(Picker::MatchProps));
    for k in ["MAterial", "SHadow", "CEnter"] {
        assert!(p.keywords.iter().any(|x| x == k), "{k}");
    }
    // Typed keywords still toggle them.
    s.input(Input::Keyword("SHadow".into())).unwrap();
    assert!(!s.last_used.matchprop.shadow);
    s.cmdline("ce").unwrap();
    assert!(!s.last_used.matchprop.center);
    s.input(Input::Enter).unwrap();
    assert!(s.prompt_text().contains("Select destination object"));
    // JSON names.
    let b = h(&s.execute("line", &json!({"points": [[0, 1], [1, 1]]})).unwrap());
    s.cancel();
    s.execute("matchprop", &json!({"source": a.hex(), "targets": [b.hex()], "properties": ["material", "shadowDisplay", "centerObject"]})).unwrap();
    assert!(s.execute("matchprop", &json!({"source": a.hex(), "targets": [b.hex()], "properties": ["shine"]})).is_err());
}

// ---------- TEXTTOFRONT / HATCHTOBACK ----------

fn order(s: &Session, space: &Space) -> Vec<Handle> {
    s.doc().unwrap().space(space).unwrap().handles()
}

fn block_order(s: &Session, name: &str) -> Vec<(Handle, &'static str)> {
    let d = s.doc().unwrap();
    d.block(name)
        .unwrap()
        .entities
        .iter()
        .map(|e| {
            (
                e.handle,
                match &e.kind {
                    EntityKind::Hatch(_) => "hatch",
                    EntityKind::Text(_) | EntityKind::MText(_) => "text",
                    _ => "other",
                },
            )
        })
        .collect()
}

#[test]
fn texttofront_and_hatchtoback_act_on_every_space_and_block() {
    let mut s = Session::new();
    // Model: text, then a rectangle, then a hatch over it.
    let t = h(&s.execute("text", &json!({"at": [1, 1], "height": 1, "text": "A"})).unwrap());
    let r = h(&s.execute("rectang", &json!({"p1": [0, 0], "p2": [10, 10]})).unwrap());
    let hat = h(&s.execute("hatch", &json!({"handles": [r.hex()]})).unwrap());
    // The hatch went to the back when made; bring it forward to test.
    s.execute("draworder", &json!({"handles": [hat.hex()], "order": "front"})).unwrap();
    // A block holding text under a line and a hatch on top.
    let bt = h(&s.execute("text", &json!({"at": [50, 50], "height": 1, "text": "B"})).unwrap());
    let br = h(&s.execute("rectang", &json!({"p1": [40, 40], "p2": [60, 60]})).unwrap());
    let bh = h(&s.execute("hatch", &json!({"handles": [br.hex()]})).unwrap());
    s.execute("draworder", &json!({"handles": [bh.hex()], "order": "front"})).unwrap();
    s.execute("block", &json!({"name": "B", "base": [40, 40], "handles": [bt.hex(), br.hex(), bh.hex()], "keep": "delete"})).unwrap();
    // A layout with text under a line.
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    let lt = h(&s.execute("text", &json!({"at": [1, 1], "height": 1, "text": "L"})).unwrap());
    s.execute("line", &json!({"points": [[0, 0], [5, 5]]})).unwrap();
    s.execute("layout.set", &json!({"name": "Model"})).unwrap();

    let r1 = s.execute("hatchtoback", &json!({})).unwrap();
    assert!(r1["moved"].as_u64().unwrap() >= 2);
    assert_eq!(order(&s, &Space::Model).first(), Some(&hat));
    assert_eq!(block_order(&s, "B").first().map(|x| x.1), Some("hatch"));

    let r2 = s.execute("texttofront", &json!({"objects": "text"})).unwrap();
    assert!(r2["moved"].as_u64().unwrap() >= 3);
    assert_eq!(order(&s, &Space::Model).last(), Some(&t));
    assert_eq!(block_order(&s, "B").last().map(|x| x.1), Some("text"));
    assert_eq!(order(&s, &Space::Paper("Layout1".into())).last(), Some(&lt));
    // One undo step each.
    s.undo().unwrap();
    assert_ne!(order(&s, &Space::Paper("Layout1".into())).last(), Some(&lt));
}

// ---------- OVERKILL ----------

#[test]
fn overkill_deletes_lines_and_arcs_on_polyline_segments() {
    let mut s = Session::new();
    // A polyline: straight 0,0 → 10,0, then a half-circle arc to 10,10 (bulge 1), then to 0,10.
    let pl = h(&s.execute("pline", &json!({"vertices": [[0, 0], {"p": [10, 0], "bulge": 1}, [10, 10], [0, 10]]})).unwrap());
    // On the first segment (part of it): deleted.
    let on = h(&s.execute("line", &json!({"points": [[2, 0], [7, 0]]})).unwrap());
    // Reversed and touching the segment's ends: deleted.
    let rev = h(&s.execute("line", &json!({"points": [[10, 10], [0, 10]]})).unwrap());
    // Running past the segment: kept.
    let past = h(&s.execute("line", &json!({"points": [[8, 0], [15, 0]]})).unwrap());
    // Parallel, off by more than the tolerance: kept.
    let off = h(&s.execute("line", &json!({"points": [[2, 0.5], [7, 0.5]]})).unwrap());
    // Part of the bulge's arc (centre 10,5, radius 5, from -90° to 90° counterclockwise): deleted.
    let arc_on = h(&s.execute("arc", &json!({"center": [10, 5], "radius": 5, "start": -60, "end": 30})).unwrap());
    // The other half of that circle: kept.
    let arc_off = h(&s.execute("arc", &json!({"center": [10, 5], "radius": 5, "start": 100, "end": 200})).unwrap());
    let r = s.execute("overkill", &json!({})).unwrap();
    assert!(r["overlapping"].as_u64().unwrap() >= 3, "{r}");
    assert!(exists(&s, pl));
    assert_eq!(
        (exists(&s, on), exists(&s, rev), exists(&s, arc_on)),
        (false, false, false),
        "{:?}",
        s.doc().unwrap().entity(pl).map(|e| e.kind.clone())
    );
    assert!(exists(&s, past) && exists(&s, off) && exists(&s, arc_off));
}

#[test]
fn overkill_polyline_comparison_follows_its_options() {
    let mut s = Session::new();
    s.execute("pline", &json!({"vertices": [[0, 0], [10, 0]]})).unwrap();
    let on = h(&s.execute("line", &json!({"points": [[2, 0], [7, 0]]})).unwrap());
    // Off with the polyline option.
    s.execute("overkill", &json!({"plines": false})).unwrap();
    assert!(exists(&s, on));
    // A different colour is a different object unless colour is ignored.
    s.execute("change", &json!({"handles": [on.hex()], "color": "1"})).ok();
    s.execute("chprop", &json!({"handles": [on.hex()], "color": "1"})).ok();
    let red = s.doc().unwrap().entity(on).unwrap().common.color != cadaddy_engine::doc::color::Color::ByLayer;
    if red {
        s.execute("overkill", &json!({})).unwrap();
        assert!(exists(&s, on));
        s.execute("overkill", &json!({"ignore": ["color"]})).unwrap();
        assert!(!exists(&s, on));
    } else {
        s.execute("overkill", &json!({})).unwrap();
        assert!(!exists(&s, on));
    }
    // Within the tolerance.
    let near = h(&s.execute("line", &json!({"points": [[2, 0.01], [7, 0.01]]})).unwrap());
    s.execute("overkill", &json!({})).unwrap();
    assert!(exists(&s, near));
    s.execute("overkill", &json!({"tolerance": 0.05})).unwrap();
    assert!(!exists(&s, near));
}
