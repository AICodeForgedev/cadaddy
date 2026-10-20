//! SETBYLAYER, XPLODE, VPSYNC, ATTIPEDIT and the frame settings (IMAGEFRAME, XCLIPFRAME, FRAME).

use cadaddy_color::Color;
use cadaddy_doc::{Common, EntityKind, Handle, Lineweight, Transparency, Viewport};
use cadaddy_engine::{Input, Session};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

fn handle(r: &Value) -> Handle {
    Handle::parse_hex(r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap()).unwrap()
}

fn line(s: &mut Session, y: f64) -> Handle {
    handle(&s.execute("line", &json!({"points": [[0, y], [10, y]]})).unwrap())
}

fn common(s: &Session, h: Handle) -> Common {
    s.doc().unwrap().entity(h).unwrap().common.clone()
}

/// Make block `name` from what is in model space now (the objects are removed).
fn make_block(s: &mut Session, name: &str) {
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": name, "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
}

fn block_commons(s: &Session, name: &str) -> Vec<Common> {
    s.doc().unwrap().block(name).unwrap().entities.iter().map(|e| e.common.clone()).collect()
}

fn coloured(s: &mut Session, h: Handle, props: Value) {
    let mut p = props;
    p["handles"] = json!([h.hex()]);
    s.execute("properties.set", &p).unwrap();
}

// ---------------- SETBYLAYER ----------------

#[test]
fn setbylayer_sets_properties_to_bylayer() {
    let mut s = Session::new();
    let a = line(&mut s, 0.0);
    coloured(&mut s, a, json!({"color": 1, "lineweight": 0.5, "transparency": 40}));
    let b = line(&mut s, 5.0);
    coloured(&mut s, b, json!({"color": "ByBlock"}));
    // ByBlock values stay unless asked.
    let r = s.execute("setbylayer", &json!({"handles": [a.hex(), b.hex()], "byBlock": false})).unwrap();
    assert_eq!(r["changed"], 1, "{r}");
    let c = common(&s, a);
    assert_eq!((c.color, c.lineweight, c.transparency), (Color::ByLayer, Lineweight::ByLayer, Transparency::ByLayer));
    assert_eq!(common(&s, b).color, Color::ByBlock);
    // SETBYLAYERMODE's default changes ByBlock too.
    s.execute("setbylayer", &json!({"handles": [b.hex()]})).unwrap();
    assert_eq!(common(&s, b).color, Color::ByLayer);
}

#[test]
fn setbylayermode_picks_the_properties() {
    let mut s = Session::new();
    let a = line(&mut s, 0.0);
    coloured(&mut s, a, json!({"color": 1, "lineweight": 0.5}));
    // Colour only (8).
    s.execute("setvar", &json!({"name": "SETBYLAYERMODE", "value": 8})).unwrap();
    s.execute("setbylayer", &json!({"handles": [a.hex()]})).unwrap();
    let c = common(&s, a);
    assert_eq!(c.color, Color::ByLayer);
    assert_eq!(c.lineweight, Lineweight::Mm100(50), "lineweight not in the mode");
}

#[test]
fn setbylayer_reaches_into_blocks_when_asked() {
    let mut s = Session::new();
    let inner = line(&mut s, 0.0);
    coloured(&mut s, inner, json!({"color": 5}));
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    make_block(&mut s, "INNER");
    s.execute("insert", &json!({"name": "INNER", "at": [0, 0]})).unwrap();
    let c = line(&mut s, 3.0);
    coloured(&mut s, c, json!({"color": 2}));
    make_block(&mut s, "OUTER");
    let i = handle(&s.execute("insert", &json!({"name": "OUTER", "at": [50, 0]})).unwrap());
    s.execute("setbylayer", &json!({"handles": [i.hex()], "blocks": false})).unwrap();
    assert!(block_commons(&s, "OUTER").iter().any(|c| c.color == Color::Index(2)), "blocks left alone");
    let r = s.execute("setbylayer", &json!({"handles": [i.hex()], "blocks": true})).unwrap();
    assert_eq!(r["blocks"], 2, "the nested block too: {r}");
    for b in ["OUTER", "INNER"] {
        assert!(block_commons(&s, b).iter().all(|c| c.color == Color::ByLayer), "{b}");
    }
}

#[test]
fn setbylayer_leaves_locked_layers_and_prompts() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "LOCKED"})).unwrap();
    let a = line(&mut s, 0.0);
    coloured(&mut s, a, json!({"color": 1, "layer": "LOCKED"}));
    s.execute("layer.set", &json!({"name": "LOCKED", "locked": true})).unwrap();
    let b = line(&mut s, 5.0);
    coloured(&mut s, b, json!({"color": "ByBlock", "linetype": "ByBlock"}));
    // Typed: the objects, then "Change ByBlock to ByLayer?" No and "Include blocks?" Yes.
    s.cmdline("setbylayer").unwrap();
    assert!(s.log.iter().any(|l| l.starts_with("Current active settings: Color")), "{:?}", s.log);
    s.input(Input::Pick(vec![a, b])).unwrap();
    s.input(Input::Enter).unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Change ByBlock to ByLayer?");
    s.cmdline("n").unwrap();
    s.cmdline("y").unwrap();
    assert!(s.running.is_none());
    assert_eq!(common(&s, a).color, Color::Index(1), "locked layer");
    assert_eq!(common(&s, b).color, Color::ByBlock, "ByBlock kept (No)");
    // Settings: a new SETBYLAYERMODE.
    s.set_selection(Vec::new());
    s.cmdline("setbylayer").unwrap();
    s.cmdline("s").unwrap();
    s.cmdline("2").unwrap();
    assert_eq!(s.doc().unwrap().header.i64("SETBYLAYERMODE", 255), 2);
    assert_eq!(s.log.last().map(String::as_str), Some("Current active settings: Lineweight"));
}

// ---------------- XPLODE ----------------

/// Block B: a line on layer 0 drawn ByBlock and a circle on layer KEEP in green; a reference on
/// layer A in red.
fn xplode_drawing() -> (Session, Handle) {
    let mut s = Session::new();
    for l in ["A", "KEEP", "PIECES"] {
        s.execute("layer.new", &json!({"name": l})).unwrap();
    }
    let l = line(&mut s, 0.0);
    coloured(&mut s, l, json!({"color": "ByBlock", "linetype": "ByBlock", "lineweight": "ByBlock", "layer": "0"}));
    let c = handle(&s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap());
    coloured(&mut s, c, json!({"color": 3, "layer": "KEEP"}));
    make_block(&mut s, "B");
    let i = handle(&s.execute("insert", &json!({"name": "B", "at": [0, 0]})).unwrap());
    coloured(&mut s, i, json!({"color": 1, "layer": "A", "lineweight": 0.3}));
    (s, i)
}

fn pieces(s: &Session, r: &Value) -> Vec<(EntityKind, Common)> {
    r["handles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            let e = s.doc().unwrap().entity(Handle::parse_hex(h.as_str().unwrap()).unwrap()).unwrap();
            (e.kind.clone(), e.common.clone())
        })
        .collect()
}

#[test]
fn xplode_inherits_from_the_parent_block() {
    let (mut s, i) = xplode_drawing();
    let r = s.execute("xplode", &json!({"handles": [i.hex()], "inherit": true})).unwrap();
    let ps = pieces(&s, &r);
    assert_eq!(ps.len(), 2);
    for (k, c) in ps {
        match k {
            // On layer 0 and ByBlock: takes the reference's.
            EntityKind::Line(_) => assert_eq!((c.layer.as_str(), c.color, c.lineweight), ("A", Color::Index(1), Lineweight::Mm100(30))),
            // On its own layer with its own colour: stays.
            _ => assert_eq!((c.layer.as_str(), c.color), ("KEEP", Color::Index(3))),
        }
    }
}

#[test]
fn xplode_sets_the_pieces_properties() {
    let (mut s, i) = xplode_drawing();
    let r = s.execute("xplode", &json!({"handles": [i.hex()], "color": 4, "layer": "pieces", "lineweight": "ByLayer"})).unwrap();
    for (_, c) in pieces(&s, &r) {
        assert_eq!((c.layer.as_str(), c.color, c.lineweight), ("PIECES", Color::Index(4), Lineweight::ByLayer));
    }
    let (mut s, i) = xplode_drawing();
    assert!(s.execute("xplode", &json!({"handles": [i.hex()], "layer": "NOPE"})).is_err());
    assert!(s.execute("xplode", &json!({"handles": [i.hex()], "linetype": "NOPE"})).is_err());
    assert!(s.doc().unwrap().entity(i).is_some(), "nothing exploded on a bad option");
}

#[test]
fn xplode_at_the_command_line() {
    let (mut s, i) = xplode_drawing();
    let j = handle(&s.execute("insert", &json!({"name": "B", "at": [20, 0]})).unwrap());
    // Two objects, one by one: the first gets a colour, the second inherits.
    s.set_selection(Vec::new());
    s.cmdline("xplode").unwrap();
    s.input(Input::Pick(vec![i, j])).unwrap();
    s.input(Input::Enter).unwrap();
    s.cmdline("i").unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Enter an option");
    s.cmdline("c").unwrap();
    s.cmdline("6").unwrap();
    s.cmdline("i").unwrap();
    assert!(s.running.is_none());
    let d = s.doc().unwrap();
    assert!(d.entity(i).is_none() && d.entity(j).is_none());
    let colours: Vec<Color> = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).map(|e| e.common.color).collect();
    // The second reference has the default properties, so its line inherits ByLayer.
    assert!(colours.contains(&Color::Index(6)) && colours.contains(&Color::ByLayer), "{colours:?}");
    // All: colour, layer, linetype, lineweight in turn; a missing layer asks again.
    let (mut s, i) = xplode_drawing();
    s.set_selection(vec![i]);
    s.cmdline("xplode").unwrap();
    s.cmdline("a").unwrap();
    s.cmdline("2").unwrap();
    s.cmdline("NOPE").unwrap();
    assert_eq!(s.log.last().map(String::as_str), Some("Cannot find layer \"NOPE\"."));
    s.cmdline("PIECES").unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.running.is_none());
    assert!(s.doc().unwrap().model.iter().all(|e| e.common.layer == "PIECES" && e.common.color == Color::Index(2)));
}

// ---------------- VPSYNC ----------------

fn vp(s: &Session, h: Handle) -> Viewport {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Viewport(v) => v.clone(),
        other => panic!("not a viewport: {other:?}"),
    }
}

fn two_viewports() -> (Session, Handle, Handle) {
    let mut s = Session::new();
    line(&mut s, 0.0);
    let mk = |s: &mut Session, p1: [f64; 2], p2: [f64; 2]| {
        let r = s.execute("mview", &json!({ "layout": "Layout1", "p1": p1, "p2": p2 })).unwrap();
        Handle::parse_hex(r["viewports"][0].as_str().unwrap()).unwrap()
    };
    let a = mk(&mut s, [0.0, 0.0], [8.0, 6.0]);
    let b = mk(&mut s, [10.0, 0.0], [14.0, 6.0]);
    s.execute("viewport.set", &json!({"handle": a.hex(), "scale": 0.5, "center": [4, 3]})).unwrap();
    s.execute("viewport.set", &json!({"handle": b.hex(), "scale": 2, "center": [100, 100]})).unwrap();
    (s, a, b)
}

#[test]
fn vpsync_lines_viewports_up_with_the_master() {
    let (mut s, a, b) = two_viewports();
    let r = s.execute("vpsync", &json!({"master": a.hex(), "handles": [b.hex()]})).unwrap();
    assert_eq!(r["synced"], 1);
    let (ma, vb) = (vp(&s, a), vp(&s, b));
    // The master's scale, and model space continues: a paper point shows the same model point
    // through either viewport.
    assert!((vb.view_height / vb.height - ma.view_height / ma.height).abs() < 1e-9);
    let p = Vec2::new(9.0, 2.0);
    let (ma_m, vb_m) = (ma.to_model(p).unwrap(), vb.to_model(p).unwrap());
    assert!(ma_m.dist(vb_m) < 1e-9, "{ma_m:?} {vb_m:?}");
    // A locked viewport stays.
    let (mut s, a, b) = two_viewports();
    s.execute("viewport.set", &json!({"handle": b.hex(), "locked": true})).unwrap();
    let before = vp(&s, b);
    let r = s.execute("vpsync", &json!({"master": a.hex(), "handles": [b.hex()]})).unwrap();
    assert_eq!((r["synced"].as_u64(), r["locked"].as_u64()), (Some(0), Some(1)));
    assert_eq!(vp(&s, b), before);
    assert!(s.execute("vpsync", &json!({"master": "1", "handles": [b.hex()]})).is_err());
}

#[test]
fn vpsync_at_the_command_line() {
    let (mut s, a, b) = two_viewports();
    // Not on the Model tab.
    s.cmdline("vpsync").unwrap();
    assert!(s.running.is_none() && s.log.last().is_some_and(|l| l.contains("layout tab")), "{:?}", s.log.last());
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    s.cmdline("vpsync").unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Select master viewport");
    s.input(Input::Pick(vec![a])).unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Select viewports to be synchronized");
    s.input(Input::Pick(vec![b])).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.running.is_none());
    let (ma, vb) = (vp(&s, a), vp(&s, b));
    assert!((vb.view_height / vb.height - ma.view_height / ma.height).abs() < 1e-9);
}

// ---------------- ATTIPEDIT ----------------

fn attributed() -> (Session, Handle) {
    let mut s = Session::new();
    s.execute("attdef", &json!({"tag": "ROOM", "default": "101", "at": [0, 0], "height": 1})).unwrap();
    s.execute("attdef", &json!({"tag": "NAME", "default": "Office", "at": [0, 5], "height": 1})).unwrap();
    make_block(&mut s, "TAG");
    let i = handle(&s.execute("insert", &json!({"name": "TAG", "at": [10, 10]})).unwrap());
    (s, i)
}

fn value(s: &Session, h: Handle, tag: &str) -> String {
    let EntityKind::Insert(i) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
    i.attribs.iter().find(|a| a.tag == tag).unwrap().text.value.clone()
}

#[test]
fn attipedit_changes_one_attribute() {
    let (mut s, i) = attributed();
    s.execute("attipedit", &json!({"handle": i.hex(), "tag": "room", "value": "102"})).unwrap();
    assert_eq!((value(&s, i, "ROOM"), value(&s, i, "NAME")), ("102".into(), "Office".into()));
    // By a point on its text (the NAME attribute at 10,15).
    let r = s.execute("attipedit", &json!({"handle": i.hex(), "at": [11, 15.5], "value": "Storage Room"})).unwrap();
    assert_eq!(r["tag"], "NAME");
    assert_eq!(value(&s, i, "NAME"), "Storage Room");
    assert!(s.execute("attipedit", &json!({"handle": i.hex(), "at": [500, 500], "value": "x"})).is_err());
    assert!(s.execute("attipedit", &json!({"handle": i.hex(), "tag": "NOPE", "value": "x"})).is_err());
}

#[test]
fn attipedit_at_the_command_line() {
    let (mut s, i) = attributed();
    s.cmdline("attipedit").unwrap();
    s.input(Input::Point(Vec2::new(10.5, 10.5))).unwrap();
    assert_eq!(s.current_prompt().unwrap().default.as_deref(), Some("101"));
    s.input(Input::Text("B-12".into())).unwrap();
    assert!(s.running.is_none());
    assert_eq!(value(&s, i, "ROOM"), "B-12");
    // Picking nothing that is an attribute says so and asks again.
    s.cmdline("attipedit").unwrap();
    s.input(Input::Point(Vec2::new(-50.0, -50.0))).unwrap();
    assert_eq!(s.log.last().map(String::as_str), Some("Not an attribute."));
    assert!(s.running.is_some());
    s.cancel();
}

// ---------------- frames ----------------

#[test]
fn frame_settings() {
    let mut s = Session::new();
    let get = |s: &mut Session, c: &str| s.execute(c, &json!({})).unwrap()["value"].as_i64().unwrap();
    assert_eq!((get(&mut s, "imageframe"), get(&mut s, "xclipframe")), (1, 2));
    assert_eq!(get(&mut s, "frame"), 3, "they differ");
    s.execute("frame", &json!({"value": 0})).unwrap();
    assert_eq!((get(&mut s, "imageframe"), get(&mut s, "xclipframe"), get(&mut s, "frame")), (0, 0, 0));
    s.execute("imageframe", &json!({"value": 2})).unwrap();
    assert_eq!(s.doc().unwrap().header.i64("IMAGEFRAME", 1), 2);
    assert!(s.execute("xclipframe", &json!({"value": 7})).is_err());
    // Typed.
    s.cmdline("xclipframe").unwrap();
    s.cmdline("1").unwrap();
    assert_eq!(s.doc().unwrap().header.i64("XCLIPFRAME", 2), 1);
}
