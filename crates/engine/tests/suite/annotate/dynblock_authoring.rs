//! The Block Authoring Palettes (BAUTHORPALETTE) and the test window (BTESTBLOCK): the palette
//! lists every parameter and action type and its tools run BPARAMETER and BACTIONTOOL with picks
//! at the prompt; the test window works on a temporary copy of the block, and closing it leaves
//! the drawing as it was.

use cadaddy_engine::doc::{Drawing, EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Value {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].clone()
}

/// Block "Box": a 10 × 5 rectangle of lines whose base point is (2, 0).
fn boxed() -> Session {
    let mut s = Session::new();
    let ls = vec![
        line(&mut s, [0.0, 0.0], [10.0, 0.0]),
        line(&mut s, [10.0, 0.0], [10.0, 5.0]),
        line(&mut s, [10.0, 5.0], [0.0, 5.0]),
        line(&mut s, [0.0, 5.0], [0.0, 0.0]),
    ];
    s.execute("block", &json!({ "name": "Box", "base": [2, 0], "handles": ls })).unwrap();
    s
}

fn lines_in(d: &Drawing, block: &str) -> Vec<[f64; 4]> {
    d.block(block)
        .unwrap()
        .entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some([l.a.x, l.a.y, l.b.x, l.b.y]),
            _ => None,
        })
        .collect()
}

fn assert_lines(got: &[[f64; 4]], want: &[[f64; 4]]) {
    assert_eq!(got.len(), want.len(), "{got:?} != {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(g.iter().zip(w).all(|(a, b)| (a - b).abs() < 1e-9), "{got:?} != {want:?}");
    }
}

fn p(x: f64, y: f64) -> Input {
    Input::Point(Vec2::new(x, y))
}

#[test]
fn the_palette_offers_every_parameter_and_action_type() {
    let mut s = boxed();
    assert!(s.execute("bauthorpalette", &json!({})).is_err(), "only in the Block Editor");
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    let r = s.execute("bauthorpalette", &json!({})).unwrap();
    assert_eq!(r["visible"], true, "the editor opens with its palettes");
    assert_eq!(r["block"], "Box");
    let tabs = r["tabs"].as_array().unwrap();
    let ids = |tab: usize| -> Vec<String> { tabs[tab]["tools"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect() };
    assert_eq!((tabs[0]["label"].as_str(), tabs[1]["label"].as_str()), (Some("Parameters"), Some("Actions")));
    assert_eq!(ids(0), ["point", "linear", "polar", "xy", "rotation", "alignment", "flip", "visibility", "lookup", "basepoint"]);
    assert_eq!(ids(1), ["move", "scale", "stretch", "polarstretch", "rotate", "flip", "array", "lookup"]);
    // Every parameter tool is a type BPARAMETER takes; every action tool one BACTIONTOOL takes.
    for id in ids(0) {
        let two = ["linear", "polar", "xy", "rotation", "alignment", "flip"].contains(&id.as_str());
        let params = if two {
            json!({ "type": id, "base": [0, 0], "end": [3, 0], "label": format!("T{id}") })
        } else if id == "visibility" {
            json!({ "type": id, "at": [0, 1] })
        } else {
            json!({ "type": id, "at": [0, 1], "label": format!("T{id}") })
        };
        s.execute("bparameter", &params).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    // Hide, show and toggle.
    assert_eq!(s.execute("bauthorpalette", &json!({ "action": "hide" })).unwrap()["visible"], false);
    assert_eq!(s.execute("bauthorpalette", &json!({})).unwrap()["visible"], false);
    assert_eq!(s.execute("bauthorpalette", &json!({ "action": "toggle" })).unwrap()["visible"], true);
    assert!(s.execute("bauthorpalette", &json!({ "action": "wobble" })).is_err());
}

#[test]
fn a_parameter_tool_asks_for_its_points_in_the_editor() {
    let mut s = boxed();
    // Outside the editor the block is named first, and the points are the block's own.
    s.start("bparameter").unwrap();
    assert_eq!(s.current_prompt().map(|p| p.message), Some("Enter block name".to_string()));
    let _ = s.input(Input::Text("Nope".into()));
    assert_eq!(s.current_prompt().map(|p| p.message), Some("Enter block name".to_string()), "no such block: it asks again");
    s.input(Input::Text("box".into())).unwrap();
    s.input(Input::Keyword("Point".into())).unwrap();
    s.input(p(3.0, 4.0)).unwrap();
    assert!(s.running.is_none());
    let d = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    assert_eq!((d["parameters"][0]["kind"]["at"]["x"].as_f64(), d["parameters"][0]["kind"]["at"]["y"].as_f64()), (Some(3.0), Some(4.0)));
    s.execute("bremove", &json!({ "block": "Box", "element": "Position1" })).unwrap();
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    s.start("bparameter").unwrap();
    s.input(Input::Keyword("Linear".into())).unwrap();
    s.input(p(0.0, 0.0)).unwrap();
    assert!(s.running.is_some(), "the end point is still to come");
    s.input(p(10.0, 0.0)).unwrap();
    assert!(s.running.is_none());
    // The editor has the base point (2, 0) at its origin: the block's coordinates add it back.
    let d = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    let k = &d["parameters"][0]["kind"];
    assert_eq!((k["type"].as_str(), k["base"]["x"].as_f64(), k["end"]["x"].as_f64()), (Some("linear"), Some(2.0), Some(12.0)), "{d}");
    // A one-point parameter, typed as the palette's tool would.
    s.start("bparameter").unwrap();
    s.input(Input::Text("base point".into())).unwrap();
    s.input(p(1.0, 3.0)).unwrap();
    let d = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    assert_eq!(d["parameters"][1]["kind"]["type"], "basePoint");
    assert_eq!(d["parameters"][1]["kind"]["at"]["x"], 3.0);
    // Not a type.
    s.start("bparameter").unwrap();
    let _ = s.input(Input::Keyword("Wobble".into()));
    assert!(s.running.is_some(), "not a type: it asks again");
    s.cancel();
}

#[test]
fn an_action_tool_picks_the_parameter_frame_and_objects() {
    let mut s = boxed();
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    s.execute("bparameter", &json!({ "type": "linear", "base": [2, 0], "end": [12, 0], "label": "Width" })).unwrap();
    let picked: Vec<Handle> = s.doc().unwrap().model.handles();
    s.start("bactiontool").unwrap();
    s.input(Input::Keyword("Stretch".into())).unwrap();
    // The one distance is the parameter the stretch follows; then the frame and the objects.
    s.input(p(6.0, -1.0)).unwrap();
    s.input(p(10.0, 6.0)).unwrap();
    s.input(Input::Pick(picked)).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.running.is_none());
    let d = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    let a = &d["actions"][0];
    assert_eq!(a["name"], "Stretch1");
    assert_eq!(a["kind"]["frame"][0], json!({ "x": 8.0, "y": -1.0 }), "the frame is in block coordinates: {a}");
    // The right side (and the half of the top and bottom in the frame) is what moves.
    s.execute("bsave", &json!({})).unwrap();
    s.execute("bclose", &json!({})).unwrap();
    s.execute("insert", &json!({ "name": "Box", "at": [0, 0] })).unwrap();
    let h = s.doc().unwrap().model.handles().last().unwrap().hex();
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Width", "value": "20" })).unwrap();
    let Some(EntityKind::Insert(i)) = s.doc().unwrap().entity(Handle::parse_hex(&h).unwrap()).map(|e| e.kind.clone()) else { panic!() };
    assert_lines(
        &lines_in(s.doc().unwrap(), &i.block),
        &[[0.0, 0.0, 20.0, 0.0], [20.0, 0.0, 20.0, 5.0], [20.0, 5.0, 0.0, 5.0], [0.0, 5.0, 0.0, 0.0]],
    );
}

#[test]
fn array_and_polar_stretch_tools_ask_for_their_extra_picks() {
    let mut s = boxed();
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    s.execute("bparameter", &json!({ "type": "linear", "base": [2, 0], "end": [4, 0], "label": "Run" })).unwrap();
    let first = s.doc().unwrap().model.handles()[0];
    s.start("bactiontool").unwrap();
    s.input(Input::Text("array".into())).unwrap();
    s.input(Input::Pick(vec![first])).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.running.is_some(), "the column offset is still to come");
    // A bad distance is refused at the prompt, which stays.
    let _ = s.input(Input::Text("-1".into()));
    assert!(s.running.is_some(), "still asking for the column offset");
    s.input(Input::Text("2".into())).unwrap();
    assert!(s.running.is_none());
    let d = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    assert_eq!(d["actions"][0]["kind"], json!({ "type": "array", "columnOffset": 2.0, "rowOffset": 0.0 }));
    // A polar stretch needs a polar parameter: none yet.
    s.start("bactiontool").unwrap();
    let _ = s.input(Input::Text("polar stretch".into()));
    assert!(s.running.as_ref().is_some_and(|r| r.id == "bactiontool"), "refused: it stays at the type prompt");
    // A lookup table is not entered at the prompt.
    let _ = s.input(Input::Text("lookup".into()));
    assert!(s.running.is_some());
    s.cancel();
    let d = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    assert_eq!(d["actions"].as_array().unwrap().len(), 1, "nothing was added");
}

#[test]
fn the_test_window_works_on_a_copy_and_closing_it_changes_nothing() {
    let mut s = boxed();
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    s.execute("bparameter", &json!({ "type": "linear", "base": [2, 0], "end": [12, 0], "label": "Width", "values": { "list": [10, 20, 30] } }))
        .unwrap();
    let hs: Vec<String> = s.doc().unwrap().model.handles().iter().map(|h| h.hex()).collect();
    s.execute("bactiontool", &json!({ "type": "stretch", "parameter": "Width", "handles": hs, "frame": [[8, -1], [12, -1], [12, 6], [8, 6]] }))
        .unwrap();
    assert!(s.execute("btestblock", &json!({ "action": "close" })).is_err(), "no test window yet");
    // An unsaved change in the editor is in the test: a mark line drawn there.
    line(&mut s, [-2.0, 2.0], [-1.0, 2.0]);
    let before = (lines_in(s.doc().unwrap(), "Box"), s.doc().unwrap().blocks.len(), s.state().unwrap().doc.model.len());

    let r = s.execute("btestblock", &json!({})).unwrap();
    assert_eq!(r["block"], "Box");
    let h = r["reference"].as_str().unwrap().to_string();
    assert!(s.state().unwrap().block_testing());
    assert_eq!(s.state().unwrap().doc.model.len(), 1, "one reference to try");
    // Properties and grips work on it; the editor's commands are not available.
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Width", "value": "30" })).unwrap();
    let Some(EntityKind::Insert(i)) = s.doc().unwrap().entity(Handle::parse_hex(&h).unwrap()).map(|e| e.kind.clone()) else { panic!() };
    let shown = lines_in(s.doc().unwrap(), &i.block);
    assert_eq!(shown.len(), 5, "the unsaved mark is in the copy: {shown:?}");
    assert!(shown.iter().any(|l| (l[2] - 30.0).abs() < 1e-9 || (l[0] - 30.0).abs() < 1e-9), "stretched to 30: {shown:?}");
    for c in ["bsave", "bclose", "bedit", "bparameter", "bauthorpalette"] {
        assert!(s.execute(c, &json!({})).is_err(), "{c} is refused in the test window");
    }
    assert!(s.execute("btestblock", &json!({})).is_err(), "one test window at a time");
    let g = s.execute("blocks.reference.grips", &json!({ "handle": h })).unwrap();
    assert!(!g.as_array().unwrap().is_empty(), "{g}");
    s.execute("line", &json!({ "points": [[0, 0], [5, 5]] })).unwrap();

    // Closing leaves the editor and the block as they were.
    let r = s.execute("btestblock", &json!({ "action": "close" })).unwrap();
    assert_eq!(r["block"], "Box");
    assert!(!s.state().unwrap().block_testing());
    assert_eq!(s.state().unwrap().block_editor(), Some("Box"));
    let after = (lines_in(s.doc().unwrap(), "Box"), s.doc().unwrap().blocks.len(), s.state().unwrap().doc.model.len());
    assert_eq!(before, after, "nothing of the test persists");
    // And the drawing behind the editor never saw it.
    s.execute("bclose", &json!({ "save": false })).unwrap();
    assert_lines(&lines_in(s.doc().unwrap(), "Box"), &[[0.0, 0.0, 10.0, 0.0], [10.0, 0.0, 10.0, 5.0], [10.0, 5.0, 0.0, 5.0], [0.0, 5.0, 0.0, 0.0]]);
    assert!(
        s.doc().unwrap().blocks.keys().all(|k| !k.starts_with("*U")),
        "no anonymous copies left: {:?}",
        s.doc().unwrap().blocks.keys().collect::<Vec<_>>()
    );
    assert_eq!(s.doc().unwrap().model.len(), 1, "the one reference BLOCK left was all that was here");
}

#[test]
fn a_file_save_during_the_test_writes_the_drawing_not_the_copy() {
    let mut s = boxed();
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    s.execute("btestblock", &json!({})).unwrap();
    let saved = s.state().unwrap().drawing().clone();
    assert_eq!(saved.model.len(), 1, "the drawing, with the one reference BLOCK left");
    assert!(saved.blocks.keys().all(|k| !k.starts_with("*U")));
    assert_eq!(saved.block("Box").unwrap().entities.len(), 4);
    s.execute("btestblock", &json!({ "action": "close" })).unwrap();
}
