//! Sections and drawing views (Q59): SECTIONPLANE, LIVESECTION, SECTIONPLANETOBLOCK, FLATSHOT,
//! VIEWBASE and VIEWSECTION; a box with a hole gives the expected 2D section lines.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, Space};
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

fn handle(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap_or_else(|| panic!("{v}"))).unwrap()
}

/// A 10 cube with a radius-2 hole through it, standing on the origin.
fn holed_box(s: &mut Session) -> Handle {
    let b = handle(&s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10 })).unwrap());
    let c = handle(&s.execute("cylinder", &json!({ "center": [5, 5, -1], "radius": 2, "height": 12 })).unwrap());
    s.execute("subtract", &json!({ "from": [b.hex()], "remove": [c.hex()] })).unwrap();
    b
}

/// The entities of the block an insert shows.
fn block_kinds(s: &Session, ins: Handle) -> Vec<EntityKind> {
    let d = s.doc().unwrap();
    let EntityKind::Insert(i) = &d.entity(ins).unwrap().kind else { panic!("not an insert") };
    d.block(&i.block).unwrap().entities.iter().map(|e| e.kind.clone()).collect()
}

#[test]
fn a_box_with_a_hole_sections_into_two_hatched_rectangles() {
    let mut s = session();
    holed_box(&mut s);
    // A vertical plane through x = 5, looking along +X.
    let sp = handle(&s.execute("sectionplane", &json!({ "from": [5, -1], "to": [5, 11] })).unwrap());
    assert!(matches!(&s.doc().unwrap().entity(sp).unwrap().kind, EntityKind::SectionPlane(p) if p.live && (p.normal.x - 1.0).abs() < 1e-12));
    let r = s.execute("sectionplanetoblock", &json!({ "handle": sp.hex(), "at": [30, 0], "background": false })).unwrap();
    let kinds = block_kinds(&s, handle(&r));
    let rects: Vec<_> = kinds.iter().filter_map(|k| if let EntityKind::LwPolyline(p) = k { Some(p.clone()) } else { None }).collect();
    assert_eq!(rects.len(), 2, "{kinds:?}");
    let mut sizes: Vec<(f64, f64)> = rects
        .iter()
        .map(|p| {
            assert!(p.closed && p.vertices.len() == 4, "{p:?}");
            let xs: Vec<f64> = p.vertices.iter().map(|v| v.p.x).collect();
            let ys: Vec<f64> = p.vertices.iter().map(|v| v.p.y).collect();
            let span = |v: &[f64]| v.iter().copied().fold(f64::MIN, f64::max) - v.iter().copied().fold(f64::MAX, f64::min);
            (span(&xs), span(&ys))
        })
        .collect();
    sizes.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Each side of the hole: 3 wide, 10 high.
    for (w, h) in sizes {
        assert!((w - 3.0).abs() < 1e-6 && (h - 10.0).abs() < 1e-6, "{w} × {h}");
    }
    assert!(kinds.iter().any(|k| matches!(k, EntityKind::Hatch(h) if h.loops.len() == 2)));
    // With the model beyond: lines too, among them the hole's far wall outline.
    let r = s.execute("sectionplanetoblock", &json!({ "handle": sp.hex(), "at": [60, 0] })).unwrap();
    assert!(block_kinds(&s, handle(&r)).iter().filter(|k| matches!(k, EntityKind::Line(_))).count() >= 4);
    // LIVESECTION toggles.
    let r = s.execute("livesection", &json!({ "handle": sp.hex() })).unwrap();
    assert_eq!(r["live"], false);
    // A plane that misses everything has nothing to draw.
    let miss = handle(&s.execute("sectionplane", &json!({ "from": [50, 0], "to": [50, 10] })).unwrap());
    assert!(s.execute("sectionplanetoblock", &json!({ "handle": miss.hex() })).is_err());
}

#[test]
fn flatshot_draws_the_visible_edges() {
    let mut s = session();
    s.execute("box", &json!({ "corner": [0, 0, 0], "length": 4, "width": 2, "height": 3 })).unwrap();
    // From above: the box's outline, 4 lines (12 long).
    let r = s.execute("flatshot", &json!({ "view": "top", "at": [20, 0] })).unwrap();
    let lines: Vec<_> = block_kinds(&s, handle(&r)).into_iter().filter_map(|k| if let EntityKind::Line(l) = k { Some(l) } else { None }).collect();
    let total: f64 = lines.iter().map(|l| l.a.xy().dist(l.b.xy())).sum();
    assert!((total - 12.0).abs() < 1e-6, "{lines:?}");
    // South-east, with hidden lines: 9 visible and 3 hidden edges.
    let r = s.execute("flatshot", &json!({ "view": "seiso", "at": [40, 0], "hidden": true })).unwrap();
    let kinds = block_kinds(&s, handle(&r));
    assert_eq!(kinds.iter().filter(|k| matches!(k, EntityKind::Line(_))).count(), 12, "{kinds:?}");
    assert!(s.execute("flatshot", &json!({ "view": "sideways" })).is_err());
}

#[test]
fn drawing_views_on_a_layout() {
    let mut s = session();
    holed_box(&mut s);
    let r = s.execute("viewbase", &json!({ "view": "top", "at": [50, 100], "scale": 2 })).unwrap();
    let base = handle(&r);
    let d = s.doc().unwrap();
    assert!(matches!(d.space_of(base), Some(Space::Paper(_))), "on a layout");
    // The top view: the square and the hole's circle.
    assert!(block_kinds(&s, base).len() > 4);
    // A section through the middle (paper coordinates: the view's block is 10 × 10 at scale 2,
    // its lower left at (50, 100)), placed to the right.
    let r = s.execute("viewsection", &json!({ "base": base.hex(), "from": [40, 110], "to": [80, 110], "at": [120, 100] })).unwrap();
    let sec = handle(&r);
    let kinds = block_kinds(&s, sec);
    assert!(kinds.iter().any(|k| matches!(k, EntityKind::Hatch(_))), "{kinds:?}");
    let d = s.doc().unwrap();
    assert_eq!(d.space_of(sec), d.space_of(base));
    // Lettered.
    let paper = d.space(&d.space_of(base).unwrap()).unwrap();
    assert!(paper.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "SECTION A-A")));
    assert!(s.execute("viewsection", &json!({ "base": "FFFF", "from": [0, 0], "to": [1, 0], "at": [5, 5] })).is_err());
}

#[test]
fn section_planes_move_and_refuse_bad_input() {
    let mut s = session();
    let sp = handle(&s.execute("sectionplane", &json!({ "point": [0, 0, 5], "normal": [0, 0, 1] })).unwrap());
    s.execute("move", &json!({ "handles": [sp.hex()], "delta": [3, 4] })).unwrap();
    assert!(
        matches!(&s.doc().unwrap().entity(sp).unwrap().kind, EntityKind::SectionPlane(p) if (p.origin.x - 3.0).abs() < 1e-12 && (p.origin.z - 5.0).abs() < 1e-12)
    );
    for p in [
        json!({ "from": [0, 0], "to": [0, 0] }),
        json!({ "point": [0, 0, 0], "normal": [0, 0, 0] }),
        json!({ "orthographic": "sideways" }),
        json!({}),
    ] {
        assert!(s.execute("sectionplane", &p).is_err(), "{p}");
    }
}
