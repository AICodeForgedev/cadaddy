//! Overrides of generated elements survive re-runs: kept where the plan around them is unchanged,
//! flagged for review where a new revision touches it, orphaned (listed, not dropped) when their
//! element is gone; re-running twice changes nothing.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::overrides::applied_ids;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, bimspec};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Room A at x 0–20', room B at x 100–120' (feet; the drawing is in inches).
const B: f64 = 100.0;

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) -> Handle {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
    h
}

/// A 20' × 10' room of 6" walls with its south-west corner at (x0, 0).
fn room(d: &mut Drawing, x0: f64) {
    let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
        line(d, "A-WALL", (ax + x0, ay), (bx + x0, by));
    }
}

fn rules(height: f64) -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": height, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    })
}

fn built() -> Session {
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    room(&mut d, B);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s.execute("wallgen", &json!({"rules": rules(9.5), "auto": true})).unwrap();
    s
}

/// The generated wall of a room (`x0`) on a side: every vertex south (y ≤ 6"), north (y ≥ 114"),
/// west (x ≤ x0 + 6") or east (x ≥ x0 + 234").
fn wall(s: &Session, x0: f64, side: char) -> Option<Handle> {
    let x0 = x0 * 12.0;
    s.doc().unwrap().model.iter().find_map(|e| {
        let EntityKind::Mesh(m) = &e.kind else { return None };
        let ok = made_as(e).as_deref() == Some("wall")
            && m.vertices.iter().all(|v| {
                v.x >= x0 - 7.0
                    && v.x <= x0 + 247.0
                    && match side {
                        's' => v.y <= 6.0 + 1e-6,
                        'n' => v.y >= 114.0 - 1e-6,
                        'w' => v.x <= x0 + 6.0 + 1e-6,
                        _ => v.x >= x0 + 234.0 - 1e-6,
                    }
            });
        ok.then_some(e.handle)
    })
}

fn min_y(s: &Session, h: Handle) -> f64 {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Mesh(m) => m.vertices.iter().map(|v| v.y).fold(f64::INFINITY, f64::min),
        _ => f64::NAN,
    }
}

fn add(s: &mut Session, p: Value) -> Value {
    s.execute("override.add", &p).unwrap()["override"].clone()
}

fn list(s: &mut Session) -> Value {
    s.execute("override.list", &json!({})).unwrap()
}

fn status_of(l: &Value, id: &Value) -> String {
    l["overrides"].as_array().unwrap().iter().find(|o| &o["id"] == id).map(|o| o["status"].as_str().unwrap().to_string()).unwrap_or_default()
}

/// Everything in model space, independent of handles: what a re-run must reproduce.
fn picture(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s.doc().unwrap().model.iter().map(|e| format!("{:?}|{}|{:?}", e.kind, e.common.layer, e.common.xdata)).collect();
    v.sort();
    v
}

/// The plan lines of room `x0`'s east side.
fn east_lines(s: &Session, x0: f64) -> Vec<String> {
    let x = (x0 + 19.0) * 12.0;
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x > x && l.b.x > x && l.a.x < x + 24.0))
        .map(|e| e.handle.hex())
        .collect()
}

/// Every kind of override on room A, and one on room B's east wall.
fn overridden() -> (Session, [Value; 6]) {
    let mut s = built();
    let south = wall(&s, 0.0, 's').unwrap();
    let north = wall(&s, 0.0, 'n').unwrap();
    let west = wall(&s, 0.0, 'w').unwrap();
    let east = wall(&s, 0.0, 'e').unwrap();
    let b_east = wall(&s, B, 'e').unwrap();
    let y0 = min_y(&s, south);
    let mv = add(&mut s, json!({"handle": south.hex(), "kind": "move", "delta": [0, -3], "note": "moved to the field line"}));
    assert!((min_y(&s, south) - (y0 - 3.0)).abs() < 1e-9);
    let ty = add(&mut s, json!({"handle": north.hex(), "kind": "type", "mark": "W2", "author": "agent"}));
    let vf = add(&mut s, json!({"handle": west.hex(), "kind": "verify", "author": "import"}));
    let dl = add(&mut s, json!({"handle": east.hex(), "kind": "delete", "note": "false detection"}));
    // A missing item: a hand-drawn line inside room A, not on a wall layer.
    let item = {
        let d = s.doc_mut().unwrap();
        line(d, "E-DEVICE", (5.0, 5.0), (6.0, 5.0))
    };
    let ad = add(&mut s, json!({"handle": item.hex(), "kind": "add"}));
    let rb = add(&mut s, json!({"handle": b_east.hex(), "kind": "verify"}));
    (s, [mv, ty, vf, dl, ad, rb])
}

#[test]
fn each_kind_applies_and_is_listed_by_author() {
    let (mut s, ovs) = overridden();
    assert_eq!(ovs.iter().map(|o| o["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["move", "type", "verify", "delete", "add", "verify"]);
    assert_eq!(ovs[0]["id"], "OV-0001");
    let north = wall(&s, 0.0, 'n').unwrap();
    assert_eq!(bimspec::entity_type_mark(&s.doc().unwrap().entity(north).unwrap().common.xdata), Some("W2"));
    assert!(wall(&s, 0.0, 'e').is_none(), "the false detection is deleted");
    let l = list(&mut s);
    assert_eq!(l["counts"], json!({"active": 6, "review": 0, "orphaned": 0}));
    assert_eq!(l["byAuthor"], json!({"user": 4, "agent": 1, "import": 1}));
    let agent = s.execute("override.list", &json!({"author": "agent"})).unwrap();
    assert_eq!(agent["overrides"].as_array().unwrap().len(), 1);
    assert_eq!(agent["overrides"][0]["mark"], "W2");
    let r = s.execute("override.list", &json!({"kind": "verify", "select": true})).unwrap();
    assert_eq!(r["overrides"].as_array().unwrap().len(), 2);
    assert_eq!(s.selection().len(), 2);
    // Refused: a plan object that wasn't generated (except as an added item), bad params.
    let plan_line = s.doc().unwrap().model.iter().find(|e| made_as(e).is_none() && e.common.layer == "A-WALL").unwrap().handle;
    assert!(s.execute("override.add", &json!({"handle": plan_line.hex(), "kind": "move", "delta": [1, 0]})).is_err());
    let south = wall(&s, 0.0, 's').unwrap();
    for p in [
        json!({"handle": south.hex(), "kind": "move", "delta": ["far", 0]}),
        json!({"handle": south.hex(), "kind": "move"}),
        json!({"handle": south.hex(), "kind": "type"}),
        json!({"handle": south.hex(), "kind": "add"}),
        json!({"handle": south.hex(), "kind": "paint"}),
        json!({"handle": "zz", "kind": "verify"}),
        json!({"handle": south.hex(), "kind": "verify", "author": "someone"}),
    ] {
        assert!(s.execute("override.add", &p).is_err(), "{p}");
    }
    assert!(s.execute("override.add", &json!({"handle": wall(&s, 0.0, 'w').unwrap().hex(), "kind": "verify"})).is_err(), "verified twice");
}

#[test]
fn a_new_revision_keeps_untouched_overrides_and_flags_the_changed_area() {
    let (mut s, ovs) = overridden();
    // The new revision moves room B's east wall 2' east; the model rebuilds by itself.
    s.execute("move", &json!({"handles": east_lines(&s, B), "from": [0, 0], "to": [24, 0]})).unwrap();
    let l = list(&mut s);
    for o in ovs.iter().take(5) {
        assert_eq!(status_of(&l, &o["id"]), "active", "{l}");
    }
    assert_eq!(status_of(&l, &ovs[5]["id"]), "review", "{l}");
    let flagged = l["overrides"].as_array().unwrap().iter().find(|o| o["id"] == ovs[5]["id"]).unwrap().clone();
    assert_eq!(flagged["message"], "review: the drawing changed here");
    // Every override is in place exactly once on the regenerated model.
    let south = wall(&s, 0.0, 's').unwrap();
    assert!((min_y(&s, south) - (-3.0)).abs() < 1e-9, "moved once, not twice: {}", min_y(&s, south));
    assert_eq!(applied_ids(&s.doc().unwrap().entity(south).unwrap().common.xdata), [1]);
    let north = wall(&s, 0.0, 'n').unwrap();
    assert_eq!(bimspec::entity_type_mark(&s.doc().unwrap().entity(north).unwrap().common.xdata), Some("W2"));
    assert!(wall(&s, 0.0, 'e').is_none(), "still deleted");
    let added = s.doc().unwrap().model.iter().filter(|e| e.common.layer == "E-DEVICE").count();
    assert_eq!(added, 1, "the added item once");
    // The regenerated east wall of room B carries the (flagged) verification.
    let b_east = wall(&s, B + 2.0, 'e').unwrap();
    assert_eq!(applied_ids(&s.doc().unwrap().entity(b_east).unwrap().common.xdata), [6]);
    // Accepted after a look: active again, and stays so.
    s.execute("override.accept", &json!({"id": ovs[5]["id"]})).unwrap();
    assert_eq!(status_of(&list(&mut s), &ovs[5]["id"]), "active");
    s.execute("wallgen", &json!({"rules": rules(9.5)})).unwrap();
    assert_eq!(list(&mut s)["counts"], json!({"active": 6, "review": 0, "orphaned": 0}));
}

#[test]
fn rerunning_twice_changes_nothing() {
    let (mut s, _) = overridden();
    let before = picture(&s);
    let r = s.execute("override.reapply", &json!({})).unwrap();
    assert_eq!(r["applied"], json!([]), "{r}");
    assert_eq!(picture(&s), before);
    // A rule re-run (changed rules: taller walls), twice: the same model both times.
    s.execute("wallgen", &json!({"rules": rules(10.0)})).unwrap();
    let once = picture(&s);
    s.execute("wallgen", &json!({"rules": rules(10.0)})).unwrap();
    assert_eq!(picture(&s), once);
    s.execute("override.reapply", &json!({})).unwrap();
    assert_eq!(picture(&s), once);
    assert_eq!(list(&mut s)["counts"], json!({"active": 6, "review": 0, "orphaned": 0}));
    let south = wall(&s, 0.0, 's').unwrap();
    assert!((min_y(&s, south) - (-3.0)).abs() < 1e-9);
}

#[test]
fn an_element_gone_from_the_revision_is_orphaned_listed_and_comes_back_with_it() {
    let (mut s, ovs) = overridden();
    // Room B's east wall is taken out of the plan.
    let gone = east_lines(&s, B);
    s.execute("erase", &json!({"handles": gone})).unwrap();
    let l = list(&mut s);
    assert_eq!(status_of(&l, &ovs[5]["id"]), "orphaned", "{l}");
    assert_eq!(l["counts"]["orphaned"], 1);
    assert_eq!(l["overrides"].as_array().unwrap().len(), 6, "never dropped");
    let o = s.execute("override.list", &json!({"status": "orphaned"})).unwrap();
    assert_eq!(o["overrides"][0]["message"], "orphaned: its element is no longer generated");
    assert!(s.execute("override.accept", &json!({"id": ovs[5]["id"]})).is_err());
    // Undo the erase: the wall is generated again, and the override finds it.
    s.execute("undo", &json!({})).unwrap();
    s.execute("wallgen", &json!({"rules": rules(9.5)})).unwrap();
    assert_eq!(status_of(&list(&mut s), &ovs[5]["id"]), "active");
}

#[test]
fn a_revision_redrawn_with_new_handles_finds_the_same_elements() {
    let (mut s, ovs) = overridden();
    // Redraw every plan wall line (new handles, same geometry), as a new revision's file would.
    let old: Vec<(Handle, Line)> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) if e.common.layer == "A-WALL" => Some((e.handle, l.clone())),
            _ => None,
        })
        .collect();
    {
        let d = s.doc_mut().unwrap();
        for (h, l) in &old {
            d.remove_entity(*h);
            let nh = d.new_handle();
            let mut e = Entity::new(nh, EntityKind::Line(l.clone()));
            e.common.layer = "A-WALL".into();
            d.model.push(e);
        }
    }
    s.execute("wallgen", &json!({"rules": rules(9.5)})).unwrap();
    let l = list(&mut s);
    assert_eq!(l["counts"], json!({"active": 6, "review": 0, "orphaned": 0}), "{l}");
    assert!(wall(&s, 0.0, 'e').is_none());
    let south = wall(&s, 0.0, 's').unwrap();
    assert!((min_y(&s, south) - (-3.0)).abs() < 1e-9);
    assert_eq!(status_of(&l, &ovs[0]["id"]), "active");
}

#[test]
fn hide_show_and_revert() {
    let (mut s, ovs) = overridden();
    let with = picture(&s);
    let r = s.execute("override.show", &json!({"on": false})).unwrap();
    assert_eq!((r["hidden"].clone(), r["changed"].clone()), (json!(true), json!(6)));
    // Hidden: the model as the rules generate it.
    let south = wall(&s, 0.0, 's').unwrap();
    assert!(min_y(&s, south).abs() < 1e-9);
    assert!(wall(&s, 0.0, 'e').is_some());
    assert_eq!(s.doc().unwrap().model.iter().filter(|e| e.common.layer == "E-DEVICE").count(), 0);
    assert!(s.doc().unwrap().model.iter().all(|e| applied_ids(&e.common.xdata).is_empty()));
    // A rebuild while hidden applies nothing; showing again puts every override back.
    s.execute("wallgen", &json!({"rules": rules(9.5)})).unwrap();
    assert!(wall(&s, 0.0, 'e').is_some());
    let r = s.execute("override.show", &json!({})).unwrap();
    assert_eq!((r["hidden"].clone(), r["changed"].clone()), (json!(false), json!(6)));
    assert_eq!(picture(&s), with);
    // Revert one: the moved wall goes back; the rest stay.
    let r = s.execute("override.revert", &json!({"id": ovs[0]["id"]})).unwrap();
    assert_eq!(r["reverted"], "OV-0001");
    let south = wall(&s, 0.0, 's').unwrap();
    assert!(min_y(&s, south).abs() < 1e-9);
    // Revert the deletion and the added item.
    s.execute("override.revert", &json!({"id": "OV-0004"})).unwrap();
    assert!(wall(&s, 0.0, 'e').is_some());
    s.execute("override.revert", &json!({"id": 5})).unwrap();
    assert_eq!(s.doc().unwrap().model.iter().filter(|e| e.common.layer == "E-DEVICE").count(), 0);
    s.execute("override.revert", &json!({"id": "OV-0002"})).unwrap();
    let north = wall(&s, 0.0, 'n').unwrap();
    assert_eq!(bimspec::entity_type_mark(&s.doc().unwrap().entity(north).unwrap().common.xdata), None);
    // Reverted stays reverted through a rebuild; ids aren't reused.
    s.execute("wallgen", &json!({"rules": rules(9.5)})).unwrap();
    assert!(min_y(&s, wall(&s, 0.0, 's').unwrap()).abs() < 1e-9);
    assert!(wall(&s, 0.0, 'e').is_some());
    assert_eq!(list(&mut s)["overrides"].as_array().unwrap().len(), 2);
    let south = wall(&s, 0.0, 's').unwrap();
    let next = add(&mut s, json!({"handle": south.hex(), "kind": "verify"}));
    assert_eq!(next["id"], "OV-0007");
    assert!(s.execute("override.revert", &json!({"id": "OV-0001"})).is_err());
    assert!(s.execute("override.revert", &json!({})).is_err());
}

#[test]
fn overrides_are_saved_with_the_drawing_and_apply_once_after_reopening() {
    let (s, _) = overridden();
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    assert!(text.contains("CADADDY_OVERRIDES"));
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    assert_eq!(cadaddy_engine::cmd::overrides::load(&back).unwrap(), cadaddy_engine::cmd::overrides::load(s.doc().unwrap()).unwrap());
    let cdy = cadaddy_io::cdy::write_cdy(s.doc().unwrap()).unwrap();
    let from_cdy = cadaddy_io::cdy::read_cdy(&cdy).unwrap();
    assert!(from_cdy.overrides.is_some());
    assert_eq!(cadaddy_engine::cmd::overrides::load(&from_cdy).unwrap().items.len(), 6);
    // Reopened and built again: each override applied once.
    let mut s2 = Session::empty();
    s2.open_drawing(back, "Sample Floor Plan", None);
    s2.execute("wallgen", &json!({"rules": rules(9.5)})).unwrap();
    assert_eq!(s2.execute("override.list", &json!({})).unwrap()["counts"], json!({"active": 6, "review": 0, "orphaned": 0}));
    let south = wall(&s2, 0.0, 's').unwrap();
    assert!((min_y(&s2, south) - (-3.0)).abs() < 1e-9);
    assert_eq!(s2.doc().unwrap().model.iter().filter(|e| e.common.layer == "E-DEVICE").count(), 1);
    assert!(wall(&s2, 0.0, 'e').is_none());
}

#[test]
fn hostile_overrides_never_panic() {
    let mut s = built();
    for text in ["", "{}", "[]", "null", r#"{"version":1,"next_id":4294967295,"items":[]}"#, r#"{"version":1,"next_id":1,"items":[{"id":1}]}"#] {
        s.doc_mut().unwrap().overrides = Some(text.into());
        let _ = s.execute("override.list", &json!({}));
        let _ = s.execute("override.reapply", &json!({}));
        let _ = s.execute("override.show", &json!({}));
        let _ = s.execute("wallgen", &json!({"rules": rules(9.5)}));
    }
    // A full id counter refuses new overrides instead of wrapping.
    s.doc_mut().unwrap().overrides = Some(r#"{"version":1,"next_id":4294967295,"items":[]}"#.into());
    let w = wall(&s, 0.0, 's').unwrap();
    assert!(s.execute("override.add", &json!({"handle": w.hex(), "kind": "verify"})).is_err());
    for p in [json!({"id": -1}), json!({"id": "OV-"}), json!({"id": [1]}), json!({"all": "yes"})] {
        let _ = s.execute("override.revert", &p);
        let _ = s.execute("override.accept", &p);
    }
}

#[test]
fn typed_at_the_command_line() {
    let mut s = built();
    let south = wall(&s, 0.0, 's').unwrap();
    // OVERRIDE with the wall picked first: Move, from a base point to a second point.
    s.set_selection(vec![south]);
    s.cmdline("override").unwrap();
    assert!(s.prompt_text().contains("Move/Type/Verify/Delete/Add"), "{}", s.prompt_text());
    for t in ["m", "0,0", "0,-3"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.running.is_none());
    assert!((min_y(&s, south) - (-3.0)).abs() < 1e-9);
    // Type, with its mark.
    let north = wall(&s, 0.0, 'n').unwrap();
    s.set_selection(vec![north]);
    for t in ["override", "t", "W3"] {
        s.cmdline(t).unwrap();
    }
    assert_eq!(bimspec::entity_type_mark(&s.doc().unwrap().entity(north).unwrap().common.xdata), Some("W3"));
    assert_eq!(list(&mut s)["overrides"].as_array().unwrap().len(), 2);
    // OVERRIDEREVERT: Enter takes the newest.
    s.cmdline("overriderevert").unwrap();
    assert!(s.prompt_text().contains("<OV-0002>"), "{}", s.prompt_text());
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert_eq!(bimspec::entity_type_mark(&s.doc().unwrap().entity(north).unwrap().common.xdata), None);
    for t in ["overriderevert", "OV-0001"] {
        s.cmdline(t).unwrap();
    }
    assert!(min_y(&s, south).abs() < 1e-9);
    assert_eq!(list(&mut s)["overrides"].as_array().unwrap().len(), 0);
}
