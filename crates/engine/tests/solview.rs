//! 2D drawings from solids (Q151): SOLVIEW, SOLDRAW and SOLPROF. A box with a hole drawn as a
//! top view and the front view folded off it gives the expected visible and hidden edges.

use cadaddy_engine::doc::{EntityKind, Handle, Viewport};
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

fn handle(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap_or_else(|| panic!("{v}"))).unwrap()
}

/// A 10 cube with a radius-2 hole straight down through it, standing on the origin.
fn holed_box(s: &mut Session) -> Handle {
    let b = handle(&s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10 })).unwrap());
    let c = handle(&s.execute("cylinder", &json!({ "center": [5, 5, -1], "radius": 2, "height": 12 })).unwrap());
    s.execute("subtract", &json!({ "from": [b.hex()], "remove": [c.hex()] })).unwrap();
    b
}

fn first_layout(s: &Session) -> String {
    s.doc().unwrap().layouts.first().unwrap().name.clone()
}

type Seg = ([f64; 2], [f64; 2]);

/// Lines on a model-space layer.
fn lines_on(s: &Session, layer: &str) -> Vec<Seg> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case(layer))
        .filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(([l.a.x, l.a.y], [l.b.x, l.b.y])) } else { None })
        .collect()
}

fn count_on(s: &Session, layer: &str, f: impl Fn(&EntityKind) -> bool) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(layer) && f(&e.kind)).count()
}

fn length(l: &Seg) -> f64 {
    ((l.1[0] - l.0[0]).powi(2) + (l.1[1] - l.0[1]).powi(2)).sqrt()
}

fn viewport(s: &Session, h: Handle) -> Viewport {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Viewport(v) => v.clone(),
        k => panic!("{k:?}"),
    }
}

/// Whether every line lies in what the viewport shows of model space.
fn shown_by(v: &Viewport, lines: &[Seg]) -> bool {
    let half_w = v.width / 2.0 * v.view_height / v.height + 1e-9;
    let half_h = v.view_height / 2.0 + 1e-9;
    lines.iter().flat_map(|l| [l.0, l.1]).all(|p| (p[0] - v.view_center.x).abs() <= half_w && (p[1] - v.view_center.y).abs() <= half_h)
}

/// A top view (12 × 12 viewport at (60, 150), scale 1) and a front view folded off its bottom edge.
fn top_and_front(s: &mut Session) -> (Handle, Handle) {
    let layout = first_layout(s);
    let top = s.execute("solview", &json!({ "type": "ucs", "name": "Top", "layout": layout, "scale": 1, "center": [60, 150] })).unwrap();
    let front =
        s.execute("solview", &json!({ "type": "ortho", "name": "Front", "layout": layout, "side": [60, 144.2], "center": [70, 100] })).unwrap();
    (handle(&top), handle(&front))
}

#[test]
fn a_box_with_a_hole_draws_its_top_and_front_views() {
    let mut s = session();
    holed_box(&mut s);
    let (top, front) = top_and_front(&mut s);
    let (tv, fv) = (viewport(&s, top), viewport(&s, front));
    assert!((fv.center.x - tv.center.x).abs() < 1e-9, "lined up under the top view: {} vs {}", fv.center.x, tv.center.x);
    assert!((fv.height / fv.view_height - 1.0).abs() < 1e-9, "the parent's scale");
    let d = s.doc().unwrap();
    assert_eq!(d.entity(front).unwrap().common.layer, "VPORTS");
    // Each view's layers, frozen in the other view.
    for n in ["Top-VIS", "Top-HID", "Top-DIM", "Top-HAT", "Front-VIS", "Front-HID", "Front-DIM", "Front-HAT"] {
        assert!(d.layer(n).is_some(), "{n}");
    }
    assert_eq!(d.layer("Front-HID").unwrap().linetype.to_ascii_uppercase(), "HIDDEN");
    assert!(tv.frozen_layers.iter().any(|l| l == "Front-VIS") && !tv.frozen_layers.iter().any(|l| l == "Top-VIS"));
    assert!(fv.frozen_layers.iter().any(|l| l == "Top-HID") && !fv.frozen_layers.iter().any(|l| l == "Front-HID"));

    let r = s.execute("soldraw", &json!({ "handles": [top.hex(), front.hex()] })).unwrap();
    assert_eq!(r["views"].as_array().map(Vec::len), Some(2), "{r}");
    // The front view: the square outline visible, the hole's two sides hidden, 4 apart.
    let (fvis, fhid) = (lines_on(&s, "Front-VIS"), lines_on(&s, "Front-HID"));
    assert_eq!(fvis.len(), 4, "{fvis:?}");
    assert!(fvis.iter().all(|l| (length(l) - 10.0).abs() < 1e-6), "{fvis:?}");
    assert_eq!(fhid.len(), 2, "{fhid:?}");
    assert!(fhid.iter().all(|l| (length(l) - 10.0).abs() < 1e-6 && (l.0[0] - l.1[0]).abs() < 1e-9));
    assert!(((fhid[0].0[0] - fhid[1].0[0]).abs() - 4.0).abs() < 1e-6, "{fhid:?}");
    // The top view: the square and the hole's rim, nothing hidden.
    let (tvis, thid) = (lines_on(&s, "Top-VIS"), lines_on(&s, "Top-HID"));
    assert!(thid.is_empty(), "{thid:?}");
    assert_eq!(tvis.iter().filter(|l| (length(l) - 10.0).abs() < 1e-6).count(), 4);
    let rim: f64 = tvis.iter().filter(|l| length(l) < 9.0).map(length).sum();
    assert!((rim - 4.0 * std::f64::consts::PI).abs() < 0.1, "rim {rim}");
    // Each viewport shows its own drawing.
    assert!(shown_by(&viewport(&s, front), &fvis) && shown_by(&viewport(&s, front), &fhid));
    assert!(shown_by(&viewport(&s, top), &tvis));
    // Drawing again replaces the lines rather than adding to them.
    s.execute("soldraw", &json!({ "handles": [front.hex()] })).unwrap();
    assert_eq!((lines_on(&s, "Front-VIS").len(), lines_on(&s, "Front-HID").len()), (4, 2));
    assert_eq!(lines_on(&s, "Top-VIS").len(), tvis.len());
    // Undo takes a drawing back as one step.
    s.undo().unwrap();
    assert_eq!(lines_on(&s, "Front-VIS").len(), 4);
    s.undo().unwrap();
    assert!(lines_on(&s, "Front-VIS").is_empty() && lines_on(&s, "Top-VIS").is_empty());
}

#[test]
fn a_section_view_hatches_the_cut() {
    let mut s = session();
    holed_box(&mut s);
    let (top, _) = top_and_front(&mut s);
    let layout = first_layout(&s);
    // Across the top view's middle (model y = 5), viewed from below (the front).
    let r = s
        .execute(
            "solview",
            &json!({ "type": "section", "name": "Cut", "layout": layout, "parent": top.hex(), "from": [52, 150], "to": [68, 150], "side": [60, 140], "scale": 2, "center": [120, 100] }),
        )
        .unwrap();
    let cut = handle(&r);
    assert!((viewport(&s, cut).height / viewport(&s, cut).view_height - 2.0).abs() < 1e-9);
    let r = s.execute("soldraw", &json!({ "handles": [cut.hex()] })).unwrap();
    assert_eq!(r["views"][0]["hatches"], 1, "{r}");
    let hatches: Vec<_> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| e.common.layer == "Cut-HAT")
        .filter_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.clone()) } else { None })
        .collect();
    assert_eq!(hatches.len(), 1);
    // Two cut faces, one each side of the hole.
    assert_eq!(hatches[0].loops.len(), 2);
    assert_eq!(count_on(&s, "Cut-VIS", |k| matches!(k, EntityKind::LwPolyline(p) if p.closed && p.vertices.len() == 4)), 2);
    // The cut faces sit in what the viewport shows.
    let v = viewport(&s, cut);
    let pts: Vec<Seg> = hatches[0].loops.iter().flat_map(|l| l.vertices.iter().map(|q| ([q.p.x, q.p.y], [q.p.x, q.p.y]))).collect();
    assert!(shown_by(&v, &pts));
    // A cutting plane missing the model (left of it): nothing to draw.
    let r = s
        .execute(
            "solview",
            &json!({ "type": "section", "name": "Miss", "layout": layout, "parent": top.hex(), "from": [53.5, 145], "to": [53.5, 155], "side": [50, 150], "center": [180, 100] }),
        )
        .unwrap();
    assert!(s.execute("soldraw", &json!({ "handles": [handle(&r).hex()] })).is_err());
}

#[test]
fn an_auxiliary_view_looks_square_to_an_inclined_plane() {
    let mut s = session();
    holed_box(&mut s);
    let (_, front) = top_and_front(&mut s);
    let layout = first_layout(&s);
    let c = viewport(&s, front).center;
    // A 45° line across the front view, viewed from its lower right.
    let r = s
        .execute(
            "solview",
            &json!({ "type": "auxiliary", "name": "Aux", "layout": layout, "from": [c.x - 3.0, c.y - 3.0], "to": [c.x + 3.0, c.y + 3.0], "side": [c.x + 3.0, c.y - 3.0], "center": [140, 60] }),
        )
        .unwrap();
    let aux = handle(&r);
    let r = s.execute("soldraw", &json!({ "handles": [aux.hex()] })).unwrap();
    let vis = lines_on(&s, "Aux-VIS");
    assert!(vis.len() > 4, "{r}");
    assert!(shown_by(&viewport(&s, aux), &vis));
    // The box's edges square to the front view keep their length (10) in the auxiliary view.
    assert!(vis.iter().any(|l| (length(l) - 10.0).abs() < 1e-6), "{vis:?}");
}

#[test]
fn soldraw_with_nothing_picked_draws_every_view() {
    let mut s = session();
    holed_box(&mut s);
    top_and_front(&mut s);
    let r = s.execute("soldraw", &json!({})).unwrap();
    assert_eq!(r["views"].as_array().map(Vec::len), Some(2), "{r}");
}

#[test]
fn a_view_survives_saving_to_dxf() {
    let mut s = session();
    holed_box(&mut s);
    let (_, front) = top_and_front(&mut s);
    let back = cadaddy_io::read(cadaddy_io::write_dxf(s.doc().unwrap()).as_bytes(), "t.dxf").unwrap();
    let mut s2 = session();
    s2.open_drawing(back, "t", None);
    let r = s2.execute("soldraw", &json!({ "handles": [front.hex()] })).unwrap();
    assert_eq!((r["views"][0]["visible"].as_u64(), r["views"][0]["hidden"].as_u64()), (Some(4), Some(2)), "{r}");
}

#[test]
fn solprof_makes_profile_blocks() {
    let mut s = session();
    let b = holed_box(&mut s);
    let r = s.execute("solprof", &json!({ "handles": [b.hex()], "view": "front" })).unwrap();
    assert_eq!((r["visible"].as_u64(), r["hidden"].as_u64()), (Some(4), Some(2)), "{r}");
    assert_eq!(r["layers"], json!(["PV-MODEL", "PH-MODEL"]));
    let d = s.doc().unwrap();
    assert_eq!(d.layer("PH-MODEL").unwrap().linetype.to_ascii_uppercase(), "HIDDEN");
    let inserts: Vec<_> = d
        .model
        .iter()
        .filter_map(|e| if let EntityKind::Insert(i) = &e.kind { Some((e.common.layer.clone(), i.block.clone())) } else { None })
        .collect();
    assert_eq!(inserts.len(), 2, "{inserts:?}");
    // The profile lies in the plane through the origin square to the view (y = 0 seen from the front).
    for (_, blk) in &inserts {
        for e in d.block(blk).unwrap().entities.iter() {
            let EntityKind::Line(l) = &e.kind else { panic!("{:?}", e.kind) };
            assert!(l.a.y.abs() < 1e-9 && l.b.y.abs() < 1e-9, "{l:?}");
        }
    }
    // Hidden lines with the visible ones in one block; in the plane through the solid instead.
    let r = s.execute("solprof", &json!({ "handles": [b.hex()], "view": "front", "hidden": false, "project": false })).unwrap();
    assert_eq!(r["blocks"].as_array().map(Vec::len), Some(1), "{r}");
    let d = s.doc().unwrap();
    let ins = Handle::parse_hex(r["blocks"][0].as_str().unwrap()).unwrap();
    let EntityKind::Insert(i) = &d.entity(ins).unwrap().kind else { panic!() };
    let blk = d.block(&i.block).unwrap();
    assert_eq!(blk.entities.len(), 6);
    assert!(blk.entities.iter().all(|e| matches!(&e.kind, EntityKind::Line(l) if (l.a.y - 5.0).abs() < 1e-9)));
}

#[test]
fn bad_input_is_refused() {
    let mut s = session();
    let b = holed_box(&mut s);
    let layout = first_layout(&s);
    let (top, _) = top_and_front(&mut s);
    for p in [
        json!({ "type": "ucs", "name": "Top", "layout": layout, "center": [10, 10] }),
        json!({ "type": "ucs", "name": "", "layout": layout, "center": [10, 10] }),
        json!({ "type": "ucs", "name": "a/b", "layout": layout, "center": [10, 10] }),
        json!({ "type": "ucs", "name": "X", "layout": layout }),
        json!({ "type": "ucs", "name": "X", "layout": layout, "center": [10, 10], "scale": 0 }),
        json!({ "type": "ucs", "name": "X", "layout": layout, "center": [10, 10], "ucs": "sideways" }),
        json!({ "type": "sideways", "name": "X", "layout": layout, "center": [10, 10] }),
        json!({ "type": "ortho", "name": "X", "layout": layout, "side": [500, 500], "center": [10, 10] }),
        json!({ "type": "ortho", "name": "X", "layout": layout, "center": [10, 10] }),
        json!({ "type": "auxiliary", "name": "X", "layout": layout, "parent": top.hex(), "from": [60, 150], "to": [60, 150], "side": [70, 150], "center": [10, 10] }),
        json!({ "type": "section", "name": "X", "layout": layout, "parent": b.hex(), "from": [55, 150], "to": [65, 150], "side": [60, 140], "center": [10, 10] }),
        json!({ "type": "ucs", "name": "X", "layout": "Nowhere", "center": [10, 10] }),
        json!({ "type": "ucs", "name": "X", "layout": layout, "center": [10, 10], "p1": [0, 0], "p2": [0, 5] }),
        json!({ "type": "ucs", "name": "X", "layout": layout, "center": [1e300, 1e300], "scale": 1e8 }),
    ] {
        assert!(s.execute("solview", &p).is_err(), "{p}");
    }
    assert!(s.execute("soldraw", &json!({ "handles": [b.hex()] })).is_err());
    assert!(s.execute("solprof", &json!({ "handles": [top.hex()] })).is_err());
    assert!(s.execute("solprof", &json!({ "handles": ["FFFFF"] })).is_err());
    // Nothing broke on the way.
    assert!(s.execute("soldraw", &json!({ "handles": [top.hex()] })).is_ok());
}

#[test]
fn soldraw_without_views_says_so() {
    let mut s = session();
    holed_box(&mut s);
    assert!(s.execute("soldraw", &json!({})).is_err());
}

#[test]
fn the_prompts_set_up_a_view_and_a_profile() {
    let mut s = session();
    let b = holed_box(&mut s);
    s.start("solview").unwrap();
    for t in ["U", "", "1", "60,150", "50,140", "70,160", "Plan"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.doc().unwrap().layer("Plan-VIS").is_some(), "SOLVIEW by prompts: {}", s.prompt_text());
    // Ortho off the plan's bottom edge.
    s.start("solview").unwrap();
    for t in ["O", "60,140", "60,110", "50,100", "70,120", "Elevation"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.doc().unwrap().layer("Elevation-HID").is_some(), "{}", s.prompt_text());
    let r = s.execute("soldraw", &json!({})).unwrap();
    assert_eq!(r["views"][1]["hidden"], 2, "{r}");
    // SOLPROF: the solid, then Yes, No, and the default.
    s.start("solprof").unwrap();
    s.input(Input::Pick(vec![b])).unwrap();
    s.input(Input::Enter).unwrap();
    s.cmdline("Y").unwrap();
    s.cmdline("N").unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.doc().unwrap().layer("PV-MODEL").is_some(), "{}", s.prompt_text());
    // Cancelled half-way: nothing is added.
    let n = s.doc().unwrap().layers.len();
    s.start("solview").unwrap();
    s.cmdline("S").unwrap();
    s.input(Input::Cancel).unwrap();
    assert_eq!(s.doc().unwrap().layers.len(), n);
}
