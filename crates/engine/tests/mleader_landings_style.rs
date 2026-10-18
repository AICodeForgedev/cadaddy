//! Multileader depth (gaps F10, FF2): several landings per multileader (Add Leader on the other
//! side of the content starts one), MLEADERSTYLE's leader type and block content, block content
//! with attributes, and per-object overrides (leader colour and linetype, arrowhead, text frame).

use cadaddy_engine::Session;
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::{EntityKind, Handle, MLeader, Space};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::render;
use serde_json::{Value, json};

fn hd(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn ml(s: &Session, h: Handle) -> MLeader {
    match s.doc().unwrap().entity(h).map(|e| e.kind.clone()) {
        Some(EntityKind::MLeader(m)) => m,
        k => panic!("not a multileader: {k:?}"),
    }
}

/// A multileader with text "NOTE" right of a landing at 5,5, its arrowhead at 0,0.
fn text_ml(s: &mut Session) -> Handle {
    hd(&s.execute("mleader", &json!({ "points": [[0, 0], [5, 5]], "text": "NOTE" })).unwrap())
}

#[test]
fn a_leader_on_the_other_side_starts_a_landing_there() {
    let mut s = Session::new();
    let h = text_ml(&mut s);
    let m0 = ml(&s, h);
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "points": [[20, 0], [20, 10], [1, 9]] })).unwrap();
    let m = ml(&s, h);
    // (1, 9) joins the first landing; the two on the right share one new landing.
    assert_eq!(m.leaders.len(), 2);
    assert_eq!(m.roots.len(), 1);
    let r = &m.roots[0];
    assert_eq!(r.leaders.len(), 2);
    assert_eq!(r.dir, -1.0, "the right landing's dogleg points left, at the text");
    assert!((r.dogleg - m0.dogleg).abs() < 1e-12);
    assert!((r.landing.y - m0.landing.y).abs() < 1e-12);
    // It sits right of the text, as far out as the first landing sits left of it.
    let t = m.text.as_ref().unwrap();
    assert!(r.landing.x > t.insert.x, "{:?}", r.landing);
    assert_eq!(m.leader_count(), 4);
    // Drawn: every leader line, and the new landing's dogleg.
    let paths = m.leader_paths();
    assert_eq!(paths.len(), 4);
    assert!(paths[2].last().unwrap().near(r.landing.xy(), 1e-9));
    let dl = m.root_doglegs();
    assert_eq!(dl.len(), 1);
    assert!(dl[0][1].x < dl[0][0].x);
    let d = s.doc().unwrap();
    let l = render::build(d, &Space::Model, &render::Options::default());
    let pts: Vec<Vec2> = l.prims.iter().filter(|p| p.handle == h).flat_map(|p| l.points(p).to_vec()).collect();
    assert!(pts.iter().any(|p| p.near(Vec2::new(20.0, 10.0), 1e-6)));
    assert!(pts.iter().any(|p| p.near(dl[0][1], 1e-6)));
}

#[test]
fn removing_leaders_counts_over_every_landing() {
    let mut s = Session::new();
    let h = text_ml(&mut s);
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "points": [[20, 0], [20, 10]] })).unwrap();
    // Leaders 1 and 2 are the right landing's: removing one keeps the landing.
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "remove", "index": 2 })).unwrap();
    assert_eq!(ml(&s, h).roots.len(), 1);
    // Removing the first landing's only leader: the right landing becomes the first.
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "remove", "index": 0 })).unwrap();
    let m = ml(&s, h);
    assert!(m.roots.is_empty());
    assert_eq!(m.leaders.len(), 1);
    assert!(m.landing.x > 5.0);
    assert!(s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "remove", "index": 5 })).is_err());
}

#[test]
fn moving_and_mirroring_carry_every_landing() {
    let mut s = Session::new();
    let h = text_ml(&mut s);
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "points": [[20, 0]] })).unwrap();
    let before = ml(&s, h);
    s.execute("move", &json!({ "handles": [h.hex()], "delta": [3, 4] })).unwrap();
    let m = ml(&s, h);
    assert!(m.roots[0].landing.xy().near(before.roots[0].landing.xy() + Vec2::new(3.0, 4.0), 1e-9));
    assert!(m.roots[0].leaders[0][0].xy().near(Vec2::new(23.0, 4.0), 1e-9));
    // Mirrored about a vertical line: the landing's dogleg now points the other way.
    s.execute("mirror", &json!({ "handles": [h.hex()], "p1": [0, 0], "p2": [0, 10], "erase": true })).unwrap();
    let m = ml(&s, h);
    assert_eq!(m.roots[0].dir, 1.0);
    // Selecting near the new landing's leader picks the multileader.
    let d = s.doc().unwrap();
    assert_eq!(cadaddy_engine::select::pick(d, &Space::Model, m.roots[0].leaders[0][0].xy(), 0.2), Some(h));
}

#[test]
fn the_style_holds_the_leader_type_and_block_content() {
    let mut s = Session::new();
    // A block with an attribute.
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 0.5 })).unwrap();
    let a = s.execute("attdef", &json!({ "tag": "NO", "default": "1", "at": [0, 0] })).unwrap();
    s.execute("block", &json!({ "name": "TAGB", "base": [0, 0], "handles": [c["handle"], a["handle"]], "keep": "delete" })).unwrap();
    s.execute("mleaderstyle", &json!({ "name": "Tags", "leaderType": "spline", "block": "TAGB", "blockAttach": "insertion" })).unwrap();
    let st = s.doc().unwrap().mleader_styles.iter().find(|x| x.name == "Tags").cloned().unwrap();
    assert_eq!((st.leader_type.as_str(), st.block.as_str(), st.block_center), ("spline", "TAGB", false));
    // A new multileader takes them; the block's attribute gets its default, or the value given.
    let h = hd(&s.execute("mleader", &json!({ "points": [[0, 0], [2, 1], [4, 2]] })).unwrap());
    let m = ml(&s, h);
    assert!(m.spline);
    let b = m.block.as_ref().expect("block content");
    assert_eq!(b.block, "TAGB");
    assert_eq!(b.attribs.iter().map(|x| (x.tag.as_str(), x.text.value.as_str())).collect::<Vec<_>>(), [("NO", "1")]);
    let h2 = hd(&s.execute("mleader", &json!({ "points": [[0, 5], [4, 7]], "attribs": { "NO": "42" } })).unwrap());
    assert_eq!(ml(&s, h2).block.unwrap().attribs[0].text.value, "42");
    // Text given explicitly still makes text content; leaderType given wins.
    let h3 = hd(&s.execute("mleader", &json!({ "points": [[0, 9], [4, 9]], "text": "x", "leaderType": "straight" })).unwrap());
    let m3 = ml(&s, h3);
    assert!(m3.block.is_none() && m3.text.is_some() && !m3.spline);
    // Typed MLEADER starts from the style too.
    s.cmdline("mleader").unwrap();
    s.cmdline("10,0").unwrap();
    s.cmdline("14,2").unwrap();
    let last = s.doc().unwrap().model.iter().last().map(|e| e.handle).unwrap();
    if let Some(EntityKind::MLeader(m)) = s.doc().unwrap().entity(last).map(|e| e.kind.clone()) {
        assert!(m.spline);
        assert_eq!(m.block.map(|b| b.block), Some("TAGB".to_string()));
    }
    s.cancel();
    // Refusals change nothing.
    for bad in
        [json!({ "name": "Tags", "leaderType": "wavy" }), json!({ "name": "Tags", "block": "NOPE" }), json!({ "name": "Tags", "blockAttach": "x" })]
    {
        assert!(s.execute("mleaderstyle", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.doc().unwrap().mleader_styles.iter().find(|x| x.name == "Tags").unwrap().block, "TAGB");
    // "" clears the block.
    s.execute("mleaderstyle", &json!({ "name": "Tags", "block": "" })).unwrap();
    assert!(s.doc().unwrap().mleader_styles.iter().find(|x| x.name == "Tags").unwrap().block.is_empty());
}

#[test]
fn per_object_overrides() {
    let mut s = Session::new();
    s.execute("linetype", &json!({ "option": "load", "name": "DASHED" })).ok();
    let h = hd(&s
        .execute("mleader", &json!({ "points": [[0, 0], [5, 5]], "text": "A", "leaderColor": "1", "arrow": "_DOT", "textFrame": true }))
        .unwrap());
    let m = ml(&s, h);
    assert_eq!(m.leader_color, Some(Color::Index(1)));
    assert_eq!(m.arrow.as_deref(), Some("_DOT"));
    assert!(m.text_frame);
    // Changed afterwards; null drops an override.
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "set", "leaderColor": null, "arrow": "_OPEN", "textFrame": false })).unwrap();
    let m = ml(&s, h);
    assert_eq!((m.leader_color, m.arrow.as_deref(), m.text_frame), (None, Some("_OPEN"), false));
    if s.doc().unwrap().linetypes.iter().any(|l| l.name.eq_ignore_ascii_case("DASHED")) {
        s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "set", "leaderLinetype": "dashed" })).unwrap();
        assert_eq!(ml(&s, h).leader_linetype.as_deref(), Some("DASHED"));
    }
    for bad in [
        json!({ "handle": h.hex(), "mode": "set", "leaderColor": "plaid" }),
        json!({ "handle": h.hex(), "mode": "set", "arrow": "_BANANA" }),
        json!({ "handle": h.hex(), "mode": "set", "leaderLinetype": "NOPE" }),
        json!({ "handle": h.hex(), "mode": "set", "textFrame": "yes" }),
        json!({ "handle": h.hex(), "mode": "set" }),
    ] {
        assert!(s.execute("mleaderedit", &bad).is_err(), "{bad}");
    }
    // Drawn: the frame around the text when on.
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "set", "textFrame": true })).unwrap();
    let d = s.doc().unwrap();
    let with = render::build(d, &Space::Model, &render::Options::default()).prims.iter().filter(|p| p.handle == h).count();
    s.execute("mleaderedit", &json!({ "handle": h.hex(), "mode": "set", "textFrame": false })).unwrap();
    let d = s.doc().unwrap();
    let without = render::build(d, &Space::Model, &render::Options::default()).prims.iter().filter(|p| p.handle == h).count();
    assert!(with > without);
}
