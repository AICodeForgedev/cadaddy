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

/// The plan edit (the east wall lines 2' east) and a line the user draws, with nothing selected
/// afterwards; the model is behind and not rebuilt yet.
fn plan_edit_then_line(s: &mut Session, at: f64) -> Handle {
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
    let line = draw_line(s, [at, -5000.0], [at + 96.0, -4964.0]);
    s.set_selection(Vec::new());
    assert!(model_behind(s), "the plan edit leaves the model behind");
    line
}

fn pickfirst_lisp(s: &mut Session) -> Value {
    s.execute("lisp", &json!({"code": "(cadr (ssgetfirst))"})).unwrap()["value"].clone()
}

/// Soak finding Q6601: an automatic rebuild (model.update, the one before a typed command, the
/// idle one) selected the rebuilt objects of the changed plan objects, so the next typed command
/// took a model object as its implied selection: ERASE + Enter deleted a model wall, CHANGE and
/// -ARRAY skipped Select objects and read `L` as a bad option. An automatic rebuild never
/// changes the user's selection; only a model.revise the user runs selects what it changed.
#[test]
fn an_automatic_rebuild_leaves_the_selection_as_it_was() {
    let mut s = live();

    // model.update with nothing selected: nothing selected after, for the engine and for LISP.
    let _ = plan_edit_then_line(&mut s, -100000.0);
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], json!(true), "{r}");
    let sel: Vec<String> = s.selection().iter().map(|h| h.hex()).collect();
    assert!(sel.is_empty(), "the automatic rebuild selected {sel:?}");
    assert_eq!(pickfirst_lisp(&mut s), json!("nil"), "(cadr (ssgetfirst)) after model.update");
    let last = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert_eq!(last["auto"], json!(true), "{last}");
    assert!(!last["changed"].as_array().unwrap().is_empty(), "the rebuild changed model objects: {last}");
    assert_eq!(last["selected"], json!([]), "an automatic rebuild selects nothing: {last}");

    // Typed ERASE + Enter: no implied selection, nothing erased.
    let (made, mine) = counts(&s);
    let all = s.doc().unwrap().model.len();
    s.cmdline("erase ").unwrap();
    assert_eq!((counts(&s), s.doc().unwrap().model.len()), ((made, mine), all), "ERASE + Enter erased nothing");

    // The rebuild that runs before a typed command (no model.update): CHANGE L asks for objects,
    // takes the line and moves it to the new layer; no model object changes.
    let line = plan_edit_then_line(&mut s, -99000.0);
    s.doc_mut().unwrap().ensure_layer("SKETCH");
    s.cmdline("change l  p la SKETCH ").unwrap();
    assert!(!model_behind(&s), "the typed command rebuilt the model first");
    assert_eq!(s.doc().unwrap().entity(line).map(|e| e.common.layer.clone()).as_deref(), Some("SKETCH"), "CHANGE L moved the line");
    assert!(s.doc().unwrap().model.iter().filter(|e| made_as(e).is_some()).all(|e| e.common.layer != "SKETCH"), "CHANGE L left the model alone");

    // -ARRAY L after model.update, selection never cleared by hand: five copies of the line.
    let line = plan_edit_then_line(&mut s, -98000.0);
    assert_eq!(s.execute("model.update", &json!({})).unwrap()["rebuilt"], json!(true));
    let layer = s.doc().unwrap().entity(line).map(|e| e.common.layer.clone()).unwrap();
    let on_layer = |s: &Session| s.doc().unwrap().model.iter().filter(|e| made_as(e).is_none() && e.common.layer == layer).count();
    let (made, before) = (counts(&s).0, on_layer(&s));
    s.cmdline("-array l  r 3 2 3 3").unwrap();
    assert_eq!((counts(&s).0, on_layer(&s)), (made, before + 5), "-ARRAY L copied the line, not a model object");

    // What the user had selected stays selected through the rebuild.
    let line = plan_edit_then_line(&mut s, -97000.0);
    s.set_selection(vec![line]);
    assert_eq!(s.execute("model.update", &json!({})).unwrap()["rebuilt"], json!(true));
    assert_eq!(s.selection(), vec![line], "the user's selection is kept");

    // A model.revise the user runs still selects what it rebuilt, and says so.
    let _ = plan_edit_then_line(&mut s, -96000.0);
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!((r["rebuilt"].clone(), r["auto"].clone()), (json!(true), json!(false)), "{r}");
    let mut sel: Vec<String> = s.selection().iter().map(|h| h.hex()).collect();
    sel.sort();
    let mut shown: Vec<String> = r["selected"].as_array().unwrap().iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    shown.sort();
    assert!(!sel.is_empty(), "{r}");
    assert_eq!(sel, shown, "the reply lists the selection model.revise made");
    assert_eq!(r["selected"], r["changed"], "{r}");
}

/// The soak's check (Q6601), printed for a report: what an automatic rebuild leaves selected and
/// what typed ERASE + Enter then erases. `cargo test ... -- --ignored --nocapture`.
#[test]
#[ignore = "a probe: prints the numbers, asserts nothing"]
fn probe_soak_check_after_an_automatic_rebuild() {
    let mut s = live();
    let _ = plan_edit_then_line(&mut s, -100000.0);
    let r = s.execute("model.update", &json!({})).unwrap();
    let picked: Vec<(String, bool)> =
        s.selection().iter().map(|h| (h.hex(), s.doc().unwrap().entity(*h).is_some_and(|e| made_as(e).is_some()))).collect();
    let lisp = pickfirst_lisp(&mut s);
    let (made, mine) = counts(&s);
    s.cmdline("erase ").unwrap();
    let (made2, mine2) = counts(&s);
    println!(
        "PROBE rebuilt={} selected={picked:?} (handle, is_model) lisp_pickfirst={lisp} erase+enter: model {made}->{made2} ({:+}), user objects {mine}->{mine2}",
        r["rebuilt"],
        made2 as i64 - made as i64
    );
}
