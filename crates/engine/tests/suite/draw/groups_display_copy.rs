//! Groups: GROUPDISPLAYMODE (a selected group shown with one grip, which moves the group),
//! copying a whole group makes an unnamed group of the copies, and PICKSTYLE's associative hatch
//! bit selects a hatch's boundary with it.

use cadaddy_engine::cmd::groups::display::{grips_hidden, shown_groups};
use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    s.execute("line", &json!({ "points": [a, b] })).unwrap();
    s.doc().unwrap().model.last().unwrap().handle
}

/// Two lines grouped as "G" (selectable) and a loose third line.
fn grouped() -> (Session, Handle, Handle, Handle) {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [0.0, 5.0], [10.0, 5.0]);
    let c = line(&mut s, [30.0, 0.0], [40.0, 0.0]);
    s.execute("group", &json!({ "name": "G", "handles": [a.hex(), b.hex()] })).unwrap();
    (s, a, b, c)
}

fn groups(s: &Session) -> Vec<(String, Vec<Handle>)> {
    s.doc().unwrap().groups.iter().map(|g| (g.name.clone(), g.members.clone())).collect()
}

fn getvar(s: &mut Session, name: &str) -> Value {
    s.execute("getvar", &json!({ "name": name })).unwrap()["value"].clone()
}

fn start(s: &Session, h: Handle) -> Vec2 {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Line(l) => l.a.xy(),
        k => panic!("not a line: {k:?}"),
    }
}

#[test]
fn copying_a_whole_group_makes_an_unnamed_group_of_the_copies() {
    let (mut s, a, b, c) = grouped();
    let r = s.execute("copy", &json!({ "handles": [a.hex(), b.hex()], "delta": [0, 20] })).unwrap();
    let gs = groups(&s);
    assert_eq!(gs.len(), 2, "{gs:?}");
    let (name, members) = &gs[1];
    assert_eq!(name, "*A1");
    let copies: Vec<String> = members.iter().map(|h| h.hex()).collect();
    assert_eq!(json!(copies), r["handles"], "the copies, in the group's order");
    // The copy keeps the group's selectability and description.
    assert!(s.doc().unwrap().groups[1].selectable);
    // Copying part of a group (one member, or with other objects) makes no new group.
    s.execute("copy", &json!({ "handles": [a.hex()], "delta": [0, 40] })).unwrap();
    assert_eq!(groups(&s).len(), 2);
    // A whole group copied together with other objects still makes its copy group.
    s.execute("copy", &json!({ "handles": [a.hex(), b.hex(), c.hex()], "delta": [50, 0] })).unwrap();
    let gs = groups(&s);
    assert_eq!(gs.len(), 3);
    assert_eq!(gs[2].1.len(), 2);
}

#[test]
fn copy_count_mirror_and_array_copies_each_make_a_group() {
    let (mut s, a, b, _) = grouped();
    s.execute("copy", &json!({ "handles": [a.hex(), b.hex()], "delta": [0, 20], "count": 3 })).unwrap();
    assert_eq!(groups(&s).len(), 4, "three copies, three new groups");
    // MIRROR keeping the source: the mirrored copies are a group.
    let (mut s, a, b, _) = grouped();
    s.execute("mirror", &json!({ "handles": [a.hex(), b.hex()], "p1": [0, -1], "p2": [10, -1] })).unwrap();
    assert_eq!(groups(&s).len(), 2);
    // MIRROR erasing the source: the group's own members move, no new group.
    let (mut s, a, b, _) = grouped();
    s.execute("mirror", &json!({ "handles": [a.hex(), b.hex()], "p1": [0, -1], "p2": [10, -1], "erase": true })).unwrap();
    assert_eq!(groups(&s).len(), 1);
    // A plain (non-associative) array: a group per copy.
    let (mut s, a, b, _) = grouped();
    s.execute("arrayrect", &json!({ "handles": [a.hex(), b.hex()], "cols": 3, "rows": 1, "colSpacing": 20, "rowSpacing": 1, "associative": false }))
        .unwrap();
    assert_eq!(groups(&s).len(), 3);
}

#[test]
fn pasting_a_whole_group_makes_an_unnamed_group() {
    let (mut s, a, b, _) = grouped();
    s.execute("copyclip", &json!({ "handles": [a.hex(), b.hex()] })).unwrap();
    let r = s.execute("pasteclip", &json!({ "at": [0, 30] })).unwrap();
    let gs = groups(&s);
    assert_eq!(gs.len(), 2, "{gs:?}");
    assert_eq!(json!(gs[1].1.iter().map(|h| h.hex()).collect::<Vec<_>>()), r["handles"]);
    s.execute("pasteclip", &json!({ "at": [0, 60] })).unwrap();
    assert_eq!(groups(&s).len(), 3, "each paste makes its own group");
    assert_eq!(groups(&s)[2].0, "*A2");
    // Part of a group on the clipboard: no group.
    s.execute("copyclip", &json!({ "handles": [a.hex()] })).unwrap();
    s.execute("pasteclip", &json!({ "at": [0, 90] })).unwrap();
    assert_eq!(groups(&s).len(), 3);
    // Into another drawing.
    s.execute("copyclip", &json!({ "handles": [a.hex(), b.hex()] })).unwrap();
    s.execute("new", &json!({})).unwrap();
    s.execute("pasteclip", &json!({})).unwrap();
    let gs = groups(&s);
    assert_eq!(gs.len(), 1, "{gs:?}");
    assert_eq!(gs[0].1.len(), 2);
}

#[test]
fn groupdisplaymode_shows_a_selected_group_with_one_grip() {
    let (mut s, a, b, c) = grouped();
    assert_eq!(getvar(&mut s, "GROUPDISPLAYMODE"), json!(2));
    // A pick on a member selects the whole group (PICKSTYLE 1): it shows with one grip.
    s.idle_click(Vec2::new(5.0, 0.0), false).unwrap();
    assert_eq!(s.selection().len(), 2);
    let shown = shown_groups(&s);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].name, "G");
    assert!(shown[0].center.near(Vec2::new(5.0, 2.5), 1e-9), "{:?}", shown[0].center);
    let hidden = grips_hidden(&s);
    assert!(hidden.contains(&a) && hidden.contains(&b) && !hidden.contains(&c));
    // Mode 0: every member shows its grips.
    s.execute("setvar", &json!({ "name": "GROUPDISPLAYMODE", "value": 0 })).unwrap();
    assert!(shown_groups(&s).is_empty() && grips_hidden(&s).is_empty());
    s.execute("setvar", &json!({ "name": "GROUPDISPLAYMODE", "value": 1 })).unwrap();
    assert_eq!(shown_groups(&s).len(), 1);
    assert!(s.execute("setvar", &json!({ "name": "GROUPDISPLAYMODE", "value": 3 })).is_err());
    // Only part of the group selected: its members show their own grips.
    s.set_selection(vec![a, c]);
    assert!(shown_groups(&s).is_empty());
    // Group selection off: no single grip.
    s.set_selection(vec![a, b]);
    s.execute("pickstyle", &json!({ "value": 0 })).unwrap();
    assert!(shown_groups(&s).is_empty());
    // A user profile setting.
    let profile = s.execute("options", &json!({})).unwrap();
    assert_eq!(profile["GROUPDISPLAYMODE"], json!(1));
}

#[test]
fn an_unselectable_group_shows_no_single_grip() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [0.0, 5.0], [10.0, 5.0]);
    s.execute("group", &json!({ "name": "U", "handles": [a.hex(), b.hex()], "selectable": false })).unwrap();
    s.set_selection(vec![a, b]);
    assert!(shown_groups(&s).is_empty());
}

#[test]
fn the_group_grip_moves_the_group_in_one_undo_step() {
    let (mut s, a, b, c) = grouped();
    s.idle_click(Vec2::new(5.0, 0.0), false).unwrap();
    let index = shown_groups(&s)[0].index;
    s.start_group_grip(index).unwrap();
    assert!(s.current_prompt().unwrap().message.contains("MOVE"));
    assert!(s.drag(Vec2::new(15.0, 12.5)).is_some(), "the group is dragged");
    s.input(Input::Point(Vec2::new(15.0, 12.5))).unwrap();
    assert!(s.running.is_none());
    assert!(start(&s, a).near(Vec2::new(10.0, 10.0), 1e-9));
    assert!(start(&s, b).near(Vec2::new(10.0, 15.0), 1e-9));
    assert!(start(&s, c).near(Vec2::new(30.0, 0.0), 1e-9), "not a member");
    s.execute("undo", &json!({})).unwrap();
    assert!(start(&s, a).near(Vec2::ZERO, 1e-9), "one undo step");
    // Esc leaves the group where it was.
    s.set_selection(vec![a, b]);
    s.start_group_grip(index).unwrap();
    s.input(Input::Cancel).unwrap();
    assert!(start(&s, a).near(Vec2::ZERO, 1e-9));
    // A group that isn't selected has no grip to drag.
    s.set_selection(vec![c]);
    assert!(s.start_group_grip(index).is_err());
    assert!(s.start_group_grip(999).is_err());
}

#[test]
fn pickstyle_2_selects_an_associative_hatchs_boundary() {
    let mut s = Session::new();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 5 })).unwrap();
    let circle = s.doc().unwrap().model.last().unwrap().handle;
    s.execute("hatch", &json!({ "handles": [circle.hex()], "pattern": "SOLID", "associative": true })).unwrap();
    let hatch = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Hatch(_))).expect("a hatch").handle;
    let EntityKind::Hatch(h) = &s.doc().unwrap().entity(hatch).unwrap().kind else { panic!("no hatch") };
    assert!(h.associative && h.loops.iter().any(|l| l.sources.contains(&circle)), "{h:?}");
    // PICKSTYLE 1 (the default): the hatch alone.
    s.execute("select", &json!({ "handles": [] })).unwrap();
    s.idle_click(Vec2::new(1.0, 1.0), false).unwrap();
    assert_eq!(s.selection(), vec![hatch]);
    // PICKSTYLE 2 and 3: its boundary comes with it.
    for v in [2, 3] {
        s.set_selection(Vec::new());
        s.execute("pickstyle", &json!({ "value": v })).unwrap();
        s.idle_click(Vec2::new(1.0, 1.0), false).unwrap();
        assert_eq!(s.selection(), vec![hatch, circle], "PICKSTYLE {v}");
    }
    // At "Select objects" too: ERASE takes both.
    s.set_selection(Vec::new());
    s.start("erase").unwrap();
    s.input(Input::Point(Vec2::new(1.0, 1.0))).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.doc().unwrap().model.is_empty());
}
