//! Multifunctional grips: the options of dimension text and dimension line grips, multileader
//! arrowhead and landing grips, text insertion grips and hatch grips; Ctrl (the cycle keyword)
//! stepping through a hot grip's options.

use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::grips::{CYCLE_OPTIONS, GripAction, grip_actions};
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn handle_of(v: &Value) -> Handle {
    Handle::parse_hex(v.as_str().unwrap()).unwrap()
}

fn kind(s: &Session, h: Handle) -> EntityKind {
    s.doc().unwrap().entity(h).unwrap().kind.clone()
}

fn option(s: &mut Session, h: Handle, index: usize, opt: &str, extra: Value) -> cadaddy_engine::Result<Value> {
    let mut p = json!({ "handle": h.hex(), "index": index, "option": opt });
    if let (Some(m), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        m.extend(e.clone());
    }
    s.execute("grip.option", &p)
}

/// A horizontal linear dimension from (0,0) to (10,0), its line at y = 5.
fn dim() -> (Session, Handle) {
    let mut s = Session::new();
    let r = s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, 5] })).unwrap();
    let h = handle_of(&r["handle"]);
    (s, h)
}

fn dimension(s: &Session, h: Handle) -> cadaddy_engine::doc::Dimension {
    match kind(s, h) {
        EntityKind::Dimension(d) => *d,
        k => panic!("not a dimension: {k:?}"),
    }
}

#[test]
fn dimension_grips_offer_autocads_options() {
    let (s, h) = dim();
    let k = kind(&s, h);
    assert_eq!(
        grip_actions(&k, 3),
        vec![GripAction::DimMoveWithLine, GripAction::DimMoveText, GripAction::DimAboveLine, GripAction::DimCenter, GripAction::DimResetText]
    );
    assert_eq!(grip_actions(&k, 2), vec![GripAction::DimContinue, GripAction::DimBaseline, GripAction::DimFlipArrow]);
    assert!(grip_actions(&k, 0).is_empty());
    assert_eq!(GripAction::DimMoveText.label(), "Move Text Only");
}

#[test]
fn dimension_text_options() {
    let (mut s, h) = dim();
    option(&mut s, h, 3, "dimMoveText", json!({ "to": [5, 9] })).unwrap();
    let d = dimension(&s, h);
    assert!(d.user_text_pos && d.text_mid.xy().near(Vec2::new(5.0, 9.0), 1e-9));
    assert_eq!(d.overrides.get("textMove"), Some(&json!(2)));
    assert!((d.defpt.y - 5.0).abs() < 1e-9, "the dimension line stays");
    // Move with Dim Line: the dimension line follows the text.
    option(&mut s, h, 3, "dimMoveWithLine", json!({ "to": [5, 12] })).unwrap();
    let d = dimension(&s, h);
    assert_eq!(d.overrides.get("textMove"), Some(&json!(0)));
    assert!(d.defpt.y > 9.0, "the dimension line moved toward the text: {:?}", d.defpt);
    // Above / centred / reset put the text back in its place.
    option(&mut s, h, 3, "dimAboveLine", json!({})).unwrap();
    let d = dimension(&s, h);
    assert!(!d.user_text_pos);
    assert_eq!(d.overrides.get("textAbove"), Some(&json!(1)));
    option(&mut s, h, 3, "dimCenter", json!({})).unwrap();
    assert_eq!(dimension(&s, h).overrides.get("textAbove"), Some(&json!(0)));
    option(&mut s, h, 3, "dimMoveText", json!({ "to": [2, 9] })).unwrap();
    option(&mut s, h, 3, "dimResetText", json!({})).unwrap();
    assert!(!dimension(&s, h).user_text_pos);
}

#[test]
fn dimension_line_options_continue_baseline_and_flip() {
    let (mut s, h) = dim();
    let r = option(&mut s, h, 2, "dimContinue", json!({ "to": [20, 0] })).unwrap();
    let c = handle_of(&r["created"]);
    let cd = dimension(&s, c);
    assert!(cd.p13.xy().near(Vec2::new(10.0, 0.0), 1e-9) && cd.p14.xy().near(Vec2::new(20.0, 0.0), 1e-9), "{cd:?}");
    let r = option(&mut s, h, 2, "dimBaseline", json!({ "to": [30, 0] })).unwrap();
    let b = dimension(&s, handle_of(&r["created"]));
    assert!(b.p13.xy().near(Vec2::ZERO, 1e-9) && b.p14.xy().near(Vec2::new(30.0, 0.0), 1e-9), "{b:?}");
    assert!(b.defpt.y > 5.0 + 1e-9, "a baseline dimension sits further out");
    assert_eq!(s.doc().unwrap().model.len(), 3);
    // Flip Arrow toggles the arrowhead at that end.
    option(&mut s, h, 2, "dimFlipArrow", json!({})).unwrap();
    assert_eq!(dimension(&s, h).marks.flip_arrows, [false, true]);
    option(&mut s, h, 2, "dimFlipArrow", json!({})).unwrap();
    assert_eq!(dimension(&s, h).marks.flip_arrows, [false, false]);
    // Options of another grip are refused.
    assert!(option(&mut s, h, 0, "dimFlipArrow", json!({})).is_err());
    assert!(option(&mut s, h, 2, "dimContinue", json!({})).is_err(), "needs a point");
}

/// A multileader from an arrowhead at (0,0) to a landing at (10,5).
fn mleader() -> (Session, Handle) {
    let mut s = Session::new();
    let r = s.execute("mleader", &json!({ "points": [[0, 0], [10, 5]], "text": "Note" })).unwrap();
    let h = handle_of(&r["handle"]);
    (s, h)
}

fn ml(s: &Session, h: Handle) -> cadaddy_engine::doc::MLeader {
    match kind(s, h) {
        EntityKind::MLeader(m) => *m,
        k => panic!("not a multileader: {k:?}"),
    }
}

#[test]
fn multileader_add_remove_leader_and_lengthen_landing() {
    let (mut s, h) = mleader();
    let k = kind(&s, h);
    // Grips: the arrowhead (0) and the landing (1).
    assert_eq!(grip_actions(&k, 0), vec![GripAction::AddLeader]);
    assert_eq!(grip_actions(&k, 1), vec![GripAction::LengthenLanding, GripAction::AddLeader]);
    option(&mut s, h, 1, "addLeader", json!({ "to": [0, 10] })).unwrap();
    assert_eq!(ml(&s, h).leaders.len(), 2);
    let k = kind(&s, h);
    assert_eq!(grip_actions(&k, 0), vec![GripAction::AddLeader, GripAction::RemoveLeader], "with two leaders, one can go");
    option(&mut s, h, 0, "removeLeader", json!({})).unwrap();
    let m = ml(&s, h);
    assert_eq!(m.leaders.len(), 1);
    assert!(m.leaders[0][0].xy().near(Vec2::new(0.0, 10.0), 1e-9), "the first leader went");
    assert!(option(&mut s, h, 0, "removeLeader", json!({})).is_err(), "the last leader stays");
    // Lengthen Landing: the dogleg grows and the text follows.
    let before = ml(&s, h);
    option(&mut s, h, 1, "lengthenLanding", json!({ "value": 2 })).unwrap();
    let after = ml(&s, h);
    assert!((after.dogleg - before.dogleg - 2.0).abs() < 1e-9);
    let (t0, t1) = (before.text.unwrap().insert, after.text.unwrap().insert);
    assert!((t1.x - t0.x - 2.0).abs() < 1e-9, "{t0:?} → {t1:?}");
    assert!(option(&mut s, h, 1, "lengthenLanding", json!({ "value": -100 })).is_err(), "no negative landing");
}

#[test]
fn text_grip_options_are_move_rotate_scale_modes() {
    let mut s = Session::new();
    s.execute("text", &json!({ "at": [0, 0], "text": "Hi", "height": 1 })).unwrap();
    let h = s.doc().unwrap().model.last().unwrap().handle;
    assert_eq!(grip_actions(&kind(&s, h), 0), vec![GripAction::TextMove, GripAction::TextRotate, GripAction::TextScale]);
    s.set_selection(vec![h]);
    s.start_grip_edit(&[(h, 0)]).unwrap();
    s.input(Input::Keyword("Rotate".into())).unwrap();
    assert!(s.current_prompt().unwrap().message.contains("ROTATE"));
    s.cmdline("90").unwrap();
    let EntityKind::Text(t) = kind(&s, h) else { panic!("no text") };
    assert!((t.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "{}", t.rotation);
    assert!(option(&mut s, h, 0, "textRotate", json!({ "value": 10 })).is_err(), "modes go through grip.rotate");
}

#[test]
fn hatch_grip_options_origin_angle_scale() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [0, 0], "p2": [10, 10] })).unwrap();
    let rect = s.doc().unwrap().model.last().unwrap().handle;
    s.execute("hatch", &json!({ "handles": [rect.hex()], "pattern": "ANSI31" })).unwrap();
    let h = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Hatch(_))).unwrap().handle;
    assert_eq!(grip_actions(&kind(&s, h), 0), vec![GripAction::HatchOrigin, GripAction::HatchAngle, GripAction::HatchScale]);
    option(&mut s, h, 0, "hatchAngle", json!({ "value": 45 })).unwrap();
    option(&mut s, h, 0, "hatchScale", json!({ "value": 2.5 })).unwrap();
    option(&mut s, h, 0, "hatchOrigin", json!({ "to": [3, 4] })).unwrap();
    let EntityKind::Hatch(hh) = kind(&s, h) else { panic!("no hatch") };
    assert!((hh.angle - 45f64.to_radians()).abs() < 1e-9);
    assert!((hh.scale - 2.5).abs() < 1e-9);
    assert!(hh.origin.near(Vec2::new(3.0, 4.0), 1e-9));
    // Dragged: the angle toward the point, the scale the distance to it.
    option(&mut s, h, 0, "hatchAngle", json!({ "to": [5, 15] })).unwrap();
    let EntityKind::Hatch(hh) = kind(&s, h) else { panic!("no hatch") };
    assert!((hh.angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "{}", hh.angle);
    assert!(option(&mut s, h, 0, "hatchScale", json!({ "value": 0 })).is_err());
    assert!(option(&mut s, h, 0, "hatchScale", json!({ "value": -1 })).is_err());
}

#[test]
fn ctrl_cycles_a_hot_grips_options_then_back_to_stretch() {
    let (mut s, h) = dim();
    s.set_selection(vec![h]);
    s.start_grip_edit(&[(h, 3)]).unwrap();
    let prompt = |s: &Session| s.current_prompt().unwrap().message;
    assert!(prompt(&s).contains("STRETCH"));
    s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
    assert!(prompt(&s).contains("MOVE WITH DIM LINE"), "{}", prompt(&s));
    s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
    assert!(prompt(&s).contains("MOVE TEXT ONLY"), "{}", prompt(&s));
    // Above / centre / reset act at once: Ctrl doesn't stop on them.
    s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
    assert!(prompt(&s).contains("STRETCH"), "{}", prompt(&s));
    // The option picked by Ctrl takes the next point; one undo step.
    s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
    s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
    s.input(Input::Point(Vec2::new(4.0, 8.0))).unwrap();
    assert!(s.running.is_none());
    assert_eq!(dimension(&s, h).overrides.get("textMove"), Some(&json!(2)));
    s.execute("undo", &json!({})).unwrap();
    assert!(!dimension(&s, h).user_text_pos);
    // Text: Ctrl steps through MOVE, ROTATE, SCALE.
    s.execute("text", &json!({ "at": [0, 20], "text": "Hi", "height": 1 })).unwrap();
    let t = s.doc().unwrap().model.last().unwrap().handle;
    s.set_selection(vec![t]);
    s.start_grip_edit(&[(t, 0)]).unwrap();
    for want in ["MOVE", "ROTATE", "SCALE", "STRETCH"] {
        s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
        assert!(prompt(&s).starts_with(&format!("** {want} **")), "{want}: {}", prompt(&s));
    }
    // A grip without options: Ctrl changes nothing.
    s.input(Input::Cancel).unwrap();
    s.execute("line", &json!({ "points": [[0, -5], [5, -5]] })).unwrap();
    let l = s.doc().unwrap().model.last().unwrap().handle;
    s.start_grip_edit(&[(l, 1)]).unwrap();
    s.input(Input::Keyword(CYCLE_OPTIONS.into())).unwrap();
    assert!(prompt(&s).contains("STRETCH"));
}
