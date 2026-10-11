//! Hot-grip options, typed at the command line as AutoCAD's grip modes take them: Base point,
//! Copy, Undo, Reference and eXit; Enter cycles the modes; several grips can be hot at once.
//! A whole grip edit, copies included, is one undo step.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{EntityKind, Handle};
use cadcraft_engine::geom::Vec2;
use serde_json::json;

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    let r = s.execute("line", &json!({ "points": [a, b] })).unwrap();
    Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap()
}

fn lines(s: &Session) -> Vec<(Vec2, Vec2)> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
            _ => None,
        })
        .collect()
}

fn ends(s: &Session, h: Handle) -> (Vec2, Vec2) {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Line(l) => (l.a.xy(), l.b.xy()),
        k => panic!("not a line: {k:?}"),
    }
}

fn near(p: Vec2, x: f64, y: f64) -> bool {
    p.near(Vec2::new(x, y), 1e-9)
}

fn grip_index(s: &Session, h: Handle, x: f64, y: f64) -> usize {
    s.grips_of(h).unwrap().iter().position(|g| near(*g, x, y)).unwrap()
}

fn typed(s: &mut Session, inputs: &[&str]) {
    for t in inputs {
        s.cmdline(t).unwrap();
    }
}

#[test]
fn base_point_copy_and_undo_make_one_undo_step() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    s.set_selection(vec![a]);
    let undo = s.state().unwrap().undo.len();
    s.start_grip_edit(&[(a, grip_index(&s, a, 10.0, 0.0))]).unwrap();
    assert!(s.prompt_text().contains("** STRETCH ** Specify stretch point or [Base point/Copy/Undo/eXit]"), "{}", s.prompt_text());

    // Base point 0,0, then Copy: each point stretches a new copy by (point - base).
    typed(&mut s, &["B", "0,0", "C", "5,5", "0,5", "0,9"]);
    assert_eq!(lines(&s).len(), 4, "the original and three copies");
    // Undo takes back the last copy only.
    typed(&mut s, &["U"]);
    assert_eq!(lines(&s).len(), 3);
    assert!(s.running.is_some(), "Copy mode carries on");
    // Enter after copies ends the edit.
    typed(&mut s, &[""]);
    assert!(s.running.is_none());
    let ls = lines(&s);
    assert!(near(ends(&s, a).1, 10.0, 0.0), "the original stays");
    assert!(ls.iter().any(|(p, q)| near(*p, 0.0, 0.0) && near(*q, 15.0, 5.0)), "{ls:?}");
    assert!(ls.iter().any(|(p, q)| near(*p, 0.0, 0.0) && near(*q, 10.0, 5.0)), "{ls:?}");
    assert_eq!(s.state().unwrap().undo.len(), undo + 1, "one undo step for the whole edit");
    s.undo().unwrap();
    assert_eq!(lines(&s).len(), 1);
}

#[test]
fn enter_cycles_the_modes_and_typed_values_apply() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    s.set_selection(vec![a]);
    s.start_grip_edit(&[(a, 0)]).unwrap();
    typed(&mut s, &[""]);
    assert!(s.prompt_text().contains("** MOVE **"), "{}", s.prompt_text());
    typed(&mut s, &[""]);
    assert!(s.prompt_text().contains("** ROTATE ** Specify rotation angle or [Base point/Copy/Undo/Reference/eXit]"), "{}", s.prompt_text());
    // A typed angle rotates about the grip; without Copy the edit ends.
    typed(&mut s, &["90"]);
    assert!(s.running.is_none());
    let (p, q) = ends(&s, a);
    assert!(near(p, 0.0, 0.0) && near(q, 0.0, 10.0), "{p:?} {q:?}");

    // SCALE takes a typed factor; MIRROR a second point; after MIRROR comes STRETCH again.
    s.start_grip_edit(&[(a, 0)]).unwrap();
    typed(&mut s, &["", "", "", "2"]);
    assert!(near(ends(&s, a).1, 0.0, 20.0), "{:?}", ends(&s, a));
    s.start_grip_edit(&[(a, 0)]).unwrap();
    typed(&mut s, &["", "", "", ""]);
    assert!(s.prompt_text().contains("** MIRROR ** Specify second point"), "{}", s.prompt_text());
    typed(&mut s, &[""]);
    assert!(s.prompt_text().contains("** STRETCH **"), "{}", s.prompt_text());
    // eXit leaves the objects as they are.
    typed(&mut s, &["X"]);
    assert!(s.running.is_none());
    assert!(near(ends(&s, a).1, 0.0, 20.0));
}

#[test]
fn reference_rotate_and_scale() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    s.set_selection(vec![a]);
    // ROTATE Reference: from 45° to 90° turns by 45°.
    s.start_grip_edit(&[(a, 0)]).unwrap();
    typed(&mut s, &["", "", "R", "45", "90"]);
    let q = ends(&s, a).1;
    let r = std::f64::consts::FRAC_1_SQRT_2 * 10.0;
    assert!(near(q, r, r), "{q:?}");

    // SCALE Reference with two points: reference length 5 (two points), new length 10.
    let b = line(&mut s, [0.0, 0.0], [4.0, 0.0]);
    s.set_selection(vec![b]);
    s.start_grip_edit(&[(b, 0)]).unwrap();
    typed(&mut s, &["", "", "", "R", "0,0", "5,0", "10"]);
    assert!(near(ends(&s, b).1, 8.0, 0.0), "{:?}", ends(&s, b));
    assert!(s.running.is_none());
}

#[test]
fn move_with_base_point_and_copy_on_the_selection() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [0.0, 5.0], [10.0, 5.0]);
    s.set_selection(vec![a, b]);
    s.start_grip_edit(&[(a, 0)]).unwrap();
    // MOVE, Copy: both selected lines are copied by each displacement.
    typed(&mut s, &["", "C", "0,20", "0,40", "X"]);
    assert!(s.running.is_none());
    assert_eq!(lines(&s).len(), 6);
    assert!(near(ends(&s, a).0, 0.0, 0.0), "the originals stay");
}

#[test]
fn several_hot_grips_stretch_together() {
    let mut s = Session::new();
    s.execute("pline", &json!({ "vertices": [[0, 0], [10, 0], [10, 10], [0, 10]], "closed": true })).unwrap();
    let h = s.doc().unwrap().model.iter().last().unwrap().handle;
    s.set_selection(vec![h]);
    let (i, j) = (grip_index(&s, h, 10.0, 0.0), grip_index(&s, h, 10.0, 10.0));
    s.start_grip_edit(&[(h, i), (h, j)]).unwrap();
    typed(&mut s, &["15,0"]);
    assert!(s.running.is_none());
    let g = s.grips_of(h).unwrap();
    assert!(g.iter().any(|p| near(*p, 15.0, 0.0)) && g.iter().any(|p| near(*p, 15.0, 10.0)), "{g:?}");

    // Typed: GRIP, pick the hot grips, Enter, then the stretch point.
    typed(&mut s, &["GRIP", "0,0", "0,10", "", "-5,0"]);
    assert!(s.running.is_none());
    let g = s.grips_of(h).unwrap();
    assert!(g.iter().any(|p| near(*p, -5.0, 0.0)) && g.iter().any(|p| near(*p, -5.0, 10.0)), "{g:?}");

    // JSON: grip.stretch with several grips.
    let (i, j) = (grip_index(&s, h, 15.0, 0.0), grip_index(&s, h, 15.0, 10.0));
    s.execute("grip.stretch", &json!({ "grips": [[h.hex(), i], [h.hex(), j]], "delta": [5, 0] })).unwrap();
    let g = s.grips_of(h).unwrap();
    assert!(g.iter().any(|p| near(*p, 20.0, 0.0)) && g.iter().any(|p| near(*p, 20.0, 10.0)), "{g:?}");
}

#[test]
fn bad_grips_are_errors_not_panics() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    s.set_selection(vec![a]);
    assert!(s.start_grip_edit(&[]).is_err());
    assert!(s.start_grip_edit(&[(a, 99)]).is_err());
    assert!(s.start_grip_edit(&[(Handle(0xFFFF), 0)]).is_err());
    for p in [
        json!({ "grips": "x" }),
        json!({ "grips": [] , "delta": [1, 0] }),
        json!({ "grips": [[a.hex()]], "delta": [1, 0] }),
        json!({ "grips": [[a.hex(), -1]], "delta": [1, 0] }),
        json!({ "grips": [[a.hex(), 0]], "delta": [f64::MAX, 0] }),
        json!({ "grips": [[a.hex(), 0]] }),
    ] {
        let _ = s.execute("grip.stretch", &p);
    }
    assert!(s.execute("grip", &json!({})).is_err());
    // Typed GRIP away from any grip re-prompts; nonsense at the mode prompt too.
    typed(&mut s, &["GRIP", "50,50"]);
    assert!(s.running.is_some());
    typed(&mut s, &["10,0", "", "foo", "1e400,0"]);
    s.cancel();
    assert!(s.running.is_none());
    let (p, q) = ends(&s, a);
    assert!(p.is_finite() && q.is_finite());
}
