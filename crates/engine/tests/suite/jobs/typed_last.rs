//! "Last" (typed `L` at Select objects, STRETCH's `L`, LISP's `(ssget "L")` and `(entlast)`) is the
//! user's most recently created visible, selectable object: what the program generates (the live
//! model's rebuild output, appended after the line the user drew), objects on hidden or locked
//! layers never become Last, whatever their place in the drawing order (soak S6-002).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::{made_as, model_behind};
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Feet → a line in inches on A-WALL.
fn wall_line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    })
}

/// A 20' × 10' room of 6" walls with an associative (live) model built from it.
fn live() -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        wall_line(&mut d, a, b);
    }
    for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        wall_line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    s
}

fn handle_of(r: &Value) -> Handle {
    let hex = r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap_or_else(|| panic!("no handle in {r}"));
    Handle::parse_hex(hex).unwrap()
}

fn draw_line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    handle_of(&s.execute("line", &json!({"points": [a, b]})).unwrap())
}

/// A plan edit (the east wall lines 2' east) leaves the model behind; the user then draws a line
/// far away, and a read rebuilds the model: its new objects come after the line in draw order.
fn line_then_rebuild(s: &mut Session, at: f64) -> Handle {
    let east: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x > 200.0 && l.b.x > 200.0))
        .map(|e| e.handle.hex())
        .collect();
    assert!(!east.is_empty());
    s.execute("move", &json!({"handles": east, "from": [0, 0], "to": [24.0, 0]})).unwrap();
    assert!(model_behind(s), "the plan edit leaves the model behind");
    let line = draw_line(s, [at, -5000.0], [at + 96.0, -4964.0]);
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], json!(true), "{r}");
    // Nothing selected beforehand, so the typed command asks for objects and gets `L`.
    s.set_selection(Vec::new());
    // The setup the soak found: the rebuild's output is now last in draw order, after the line.
    let last = s.doc().unwrap().model.iter().last().map(|e| (e.handle, made_as(e).is_some()));
    assert!(last.is_some_and(|(h, made)| made && h != line), "a generated object ends the drawing order: {last:?}");
    line
}

/// Counts of (generated objects, objects on the line's layer that are not generated).
fn counts(s: &Session) -> (usize, usize) {
    let d = s.doc().unwrap();
    let made = d.model.iter().filter(|e| made_as(e).is_some()).count();
    (made, d.model.iter().filter(|e| made_as(e).is_none() && e.common.layer == "0").count())
}

fn generated_fingerprint(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_some())
        .map(|e| format!("{}|{}|{:?}|{:?}", e.handle.hex(), e.common.layer, e.common.color, e.kind))
        .collect();
    v.sort();
    v
}

#[test]
fn typed_last_after_a_live_rebuild_is_the_line_the_user_drew() {
    let mut s = live();

    // CHPROP L: the line turns blue; no generated object changes.
    let line = line_then_rebuild(&mut s, -100000.0);
    let model = generated_fingerprint(&s);
    s.cmdline("chprop l  c blue ").unwrap();
    assert_eq!(s.doc().unwrap().entity(line).map(|e| e.common.color), Some(Color::Index(5)), "CHPROP L changed the line");
    assert_eq!(generated_fingerprint(&s), model, "CHPROP L left the model alone");

    // CHANGE L, Properties, LAyer: the line goes to the new layer; no generated object moves.
    let line = line_then_rebuild(&mut s, -99000.0);
    s.doc_mut().unwrap().ensure_layer("SKETCH");
    let model = generated_fingerprint(&s);
    s.cmdline("change l  p la SKETCH ").unwrap();
    assert_eq!(s.doc().unwrap().entity(line).map(|e| e.common.layer.clone()).as_deref(), Some("SKETCH"), "CHANGE L moved the line");
    assert_eq!(generated_fingerprint(&s), model, "CHANGE L left the model alone");

    // -ARRAY L: five copies of the line, none of a generated object.
    let line = line_then_rebuild(&mut s, -98000.0);
    let layer = s.doc().unwrap().entity(line).map(|e| e.common.layer.clone()).unwrap();
    let on_layer = |s: &Session| s.doc().unwrap().model.iter().filter(|e| made_as(e).is_none() && e.common.layer == layer).count();
    let (made, before) = (counts(&s).0, on_layer(&s));
    s.cmdline("-array l  r 3 2 3 3").unwrap();
    assert_eq!((counts(&s).0, on_layer(&s)), (made, before + 5), "-ARRAY L copied the line, not a model object");

    // ERASE L takes the line away and leaves every generated object.
    let line = line_then_rebuild(&mut s, -97000.0);
    let made = counts(&s).0;
    s.cmdline("erase l ").unwrap();
    assert!(s.doc().unwrap().entity(line).is_none(), "ERASE L erased the line");
    assert_eq!(counts(&s).0, made);
}

#[test]
fn stretch_and_lisp_last_skip_generated_objects() {
    let mut s = live();
    let line = line_then_rebuild(&mut s, -100000.0);
    let lisp = |s: &mut Session, code: String| s.execute("lisp", &json!({"code": code})).unwrap()["value"].clone();
    let is_line = |what: &str| format!("(eq {what} (handent \"{}\"))", line.hex());
    assert_eq!(lisp(&mut s, is_line("(ssname (ssget \"L\") 0)")), json!("T"), "(ssget \"L\")");
    assert_eq!(lisp(&mut s, is_line("(entlast)")), json!("T"), "(entlast)");

    // STRETCH's own selection: L picks the line (moved whole: a crossing never caught it).
    let before = s.doc().unwrap().entity(line).map(|e| e.kind.clone());
    let model = generated_fingerprint(&s);
    for t in ["stretch", "l", "", "0,0", "12,0"] {
        s.cmdline(t).unwrap();
    }
    assert_ne!(s.doc().unwrap().entity(line).map(|e| e.kind.clone()), before, "STRETCH L moved the line");
    assert_eq!(generated_fingerprint(&s), model, "STRETCH L left the model alone");
}

#[test]
fn last_skips_objects_on_locked_and_hidden_layers() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Plan", None);
    let first = draw_line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let d = s.doc_mut().unwrap();
    d.ensure_layer("LOCKED");
    d.ensure_layer("OFF");
    let locked = draw_line(&mut s, [0.0, 5.0], [10.0, 5.0]);
    let off = draw_line(&mut s, [0.0, 9.0], [10.0, 9.0]);
    let d = s.doc_mut().unwrap();
    d.modify_entity(locked, |e| e.common.layer = "LOCKED".into()).unwrap();
    d.modify_entity(off, |e| e.common.layer = "OFF".into()).unwrap();
    d.layer_mut("LOCKED").unwrap().locked = true;
    d.layer_mut("OFF").unwrap().on = false;
    s.set_selection(Vec::new());
    s.cmdline("erase l ").unwrap();
    let d = s.doc().unwrap();
    assert!(d.entity(first).is_none(), "Last is the newest object that can be selected");
    assert!(d.entity(locked).is_some() && d.entity(off).is_some(), "locked and hidden objects are never Last");
}
