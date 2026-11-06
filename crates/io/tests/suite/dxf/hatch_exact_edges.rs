//! Hatch boundaries with elliptical arcs and splines are written as exact DXF edges (edge types 3
//! and 4, DXF Reference) and read back as the same curves, so their areas stay exact. Loops of
//! lines and arcs stay polyline boundaries with bulges.

use cadaddy_doc::*;
use cadaddy_geom::{LoopEdge, Spline, Vec2, loop_area};
use cadaddy_io::{read, read_dxf, write, write_dxf};

fn hatch(loops: Vec<HatchLoop>) -> Hatch {
    Hatch {
        pattern: "SOLID".into(),
        solid: true,
        loops,
        scale: 1.0,
        angle: 0.0,
        associative: false,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::ZERO,
        background: None,
        pattern_lines: Vec::new(),
    }
}

fn hatches(d: &Drawing) -> Vec<Hatch> {
    d.model.iter().filter_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.as_ref().clone()) } else { None }).collect()
}

/// An ellipse loop, a half ellipse clockwise closed by its diameter, a spline loop (a cubic
/// Bezier closed by a line) and a plain square.
fn sample() -> Drawing {
    let mut d = Drawing::new_imperial();
    let ellipse = HatchLoop::from_curves(vec![LoopEdge::full_ellipse(Vec2::new(10.0, 0.0), Vec2::new(4.0, 3.0), 0.4)], true, Vec::new());
    let half = HatchLoop::from_curves(
        vec![
            LoopEdge::Ellipse { center: Vec2::new(30.0, 0.0), major: Vec2::X * 5.0, ratio: 0.5, start: 0.0, end: std::f64::consts::PI, ccw: false },
            LoopEdge::Line { a: Vec2::new(25.0, 0.0), b: Vec2::new(35.0, 0.0) },
        ],
        true,
        Vec::new(),
    );
    let bez = Spline {
        degree: 3,
        knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        control: vec![Vec2::new(50.0, 0.0), Vec2::new(50.0, 4.0), Vec2::new(54.0, 4.0), Vec2::new(54.0, 0.0)],
        ..Spline::default()
    };
    let spline =
        HatchLoop::from_curves(vec![LoopEdge::Spline(bez), LoopEdge::Line { a: Vec2::new(54.0, 0.0), b: Vec2::new(50.0, 0.0) }], true, Vec::new());
    let square = HatchLoop::from_curves(
        vec![
            LoopEdge::Line { a: Vec2::new(70.0, 0.0), b: Vec2::new(72.0, 0.0) },
            LoopEdge::Line { a: Vec2::new(72.0, 0.0), b: Vec2::new(72.0, 2.0) },
            LoopEdge::Line { a: Vec2::new(72.0, 2.0), b: Vec2::new(70.0, 2.0) },
            LoopEdge::Line { a: Vec2::new(70.0, 2.0), b: Vec2::new(70.0, 0.0) },
        ],
        true,
        Vec::new(),
    );
    for l in [ellipse, half, spline, square] {
        d.add(&Space::Model, Common::default(), EntityKind::Hatch(Box::new(hatch(vec![l])))).expect("add");
    }
    d
}

fn areas(d: &Drawing) -> Vec<f64> {
    hatches(d).iter().map(|h| h.loops.iter().map(HatchLoop::area).sum()).collect()
}

fn check(back: &Drawing, want: &Drawing) {
    let (a, b) = (hatches(back), hatches(want));
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        let (lx, ly) = (&x.loops[0], &y.loops[0]);
        assert_eq!(lx.curves.len(), ly.curves.len(), "{lx:?}");
        for (cx, cy) in lx.curves.iter().zip(&ly.curves) {
            assert!(cx.start_point().near(cy.start_point(), 1e-9) && cx.end_point().near(cy.end_point(), 1e-9), "{cx:?} vs {cy:?}");
        }
    }
    for (x, y) in areas(back).iter().zip(areas(want)) {
        assert!((x - y).abs() < 1e-9 * (1.0 + y.abs()), "{x} vs {y}");
    }
}

#[test]
fn elliptical_and_spline_edges_roundtrip_exactly_in_dxf() {
    let d = sample();
    let text = write_dxf(&d);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let edge_types: Vec<&str> = lines.windows(2).filter(|w| w[0] == "72").map(|w| w[1]).collect();
    assert!(edge_types.contains(&"3") && edge_types.contains(&"4"), "edge types written: {edge_types:?}");
    let back = read_dxf(text.as_bytes()).unwrap();
    check(&back, &d);
    // The plain square has no curves: a polyline boundary, no edge list.
    assert!(hatches(&back)[3].loops[0].curves.is_empty());
    // Exact areas: π·5·2, half of π·5·2.5 (clockwise), the Bezier's 9.6, the square's 4.
    let a = areas(&back);
    let pi = std::f64::consts::PI;
    assert!((a[0] - pi * 10.0).abs() < 1e-9, "{}", a[0]);
    assert!((a[1].abs() - pi * 12.5 / 2.0).abs() < 1e-9, "{}", a[1]);
    assert!((a[2].abs() - 9.6).abs() < 1e-9, "{}", a[2]);
    assert!((a[3].abs() - 4.0).abs() < 1e-12);
    // And again: the same curves.
    check(&read_dxf(write_dxf(&back).as_bytes()).unwrap(), &d);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn elliptical_and_spline_edges_roundtrip_in_dwg() {
    let d = sample();
    let back = read(&write(&d, "x.dwg").unwrap(), "x.dwg").unwrap();
    check(&back, &d);
}

/// CADaddy's own DWG writer, every version: a spline edge followed by another edge (before
/// R2010 a spline edge has no fit data, and the fit point count in its groups was taken for the
/// next edge's type, so the hatch didn't read back; in an R2000 file the objects after it in
/// model space went too).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn spline_edges_roundtrip_through_the_native_writer_in_every_version() {
    let mut d = sample();
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Line(Line { a: cadaddy_geom::Vec3::new(0.0, 20.0, 0.0), b: cadaddy_geom::Vec3::new(10.0, 20.0, 0.0) }),
    )
    .expect("add");
    for v in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, v).unwrap_or_else(|e| panic!("{v}: {e}"));
        let (back, report) = cadaddy_io::dwg_read::read(&bytes).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert!(report.failed.is_empty(), "{v}: {:?}", report.failed);
        assert_eq!(back.model.iter().count(), d.model.iter().count(), "{v}");
        check(&back, &d);
    }
}

#[test]
fn a_loop_with_a_broken_curve_keeps_its_points_only() {
    // A spline edge with too few control points: the loop reads, without an exact edge list.
    let text = "0\nSECTION\n2\nENTITIES\n0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n2\nSOLID\n70\n1\n71\n0\n\
                91\n1\n92\n1\n93\n2\n72\n4\n94\n3\n73\n0\n74\n0\n95\n2\n96\n1\n40\n0\n40\n1\n10\n0\n20\n0\n97\n0\n\
                72\n1\n10\n0\n20\n0\n11\n1\n21\n1\n97\n0\n75\n0\n76\n1\n98\n0\n0\nENDSEC\n0\nEOF\n";
    let d = read_dxf(text.as_bytes()).unwrap();
    let h = hatches(&d);
    assert_eq!(h.len(), 1);
    assert!(h[0].loops[0].curves.is_empty());
    assert!(!h[0].loops[0].vertices.is_empty());
    let _ = loop_area(&h[0].loops[0].curves);
}

/// A hatch stored mirrored (extrusion 0,0,-1, what MIRROR leaves behind) with a whole ellipse
/// and an elliptical arc closed by a line: the exact edges mirror with the points, so the
/// ellipse centre lands at the negated-x WCS position and the file round trip keeps the WCS
/// geometry (it used to jump to the mirror image).
const MIRRORED: &str = "0\nSECTION\n2\nENTITIES\n0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n-1\n2\nSOLID\n70\n1\n71\n0\n\
    91\n2\n\
    92\n1\n93\n1\n72\n3\n10\n5\n20\n2\n11\n3\n21\n0\n40\n0.5\n50\n0\n51\n360\n73\n1\n97\n0\n\
    92\n1\n93\n2\n72\n3\n10\n20\n20\n0\n11\n4\n21\n0\n40\n0.5\n50\n0\n51\n180\n73\n1\n97\n0\n\
    72\n1\n10\n16\n20\n0\n11\n24\n21\n0\n97\n0\n75\n0\n76\n1\n98\n0\n0\nENDSEC\n0\nEOF\n";

fn mirrored_hatch() -> Hatch {
    hatches(&read_dxf(MIRRORED.as_bytes()).unwrap()).remove(0)
}

#[test]
fn a_mirrored_ocs_hatch_mirrors_its_exact_edges() {
    let h = mirrored_hatch();
    assert_eq!(h.loops.len(), 2);
    let pi = std::f64::consts::PI;
    // Whole ellipse: centre (5,2) in the OCS is (-5,2) in the WCS; area π·3·1.5.
    let full = &h.loops[0];
    match full.curves.as_slice() {
        [LoopEdge::Ellipse { center, major, ratio, .. }] => {
            assert!(center.near(Vec2::new(-5.0, 2.0), 1e-9), "{center:?}");
            assert!(major.near(Vec2::new(-3.0, 0.0), 1e-9), "{major:?}");
            assert!((ratio - 0.5).abs() < 1e-12);
        }
        other => panic!("{other:?}"),
    }
    assert!((full.area().abs() - pi * 4.5).abs() < 1e-9, "{}", full.area());
    // Half ellipse + diameter: the ellipse centre is at (-20,0), the arc runs from the mirrored
    // OCS (24,0) to (16,0), i.e. WCS (-24,0) to (-16,0).
    let half = &h.loops[1];
    assert!(
        half.curves.iter().any(|c| matches!(c, LoopEdge::Ellipse { center, .. } if center.near(Vec2::new(-20.0, 0.0), 1e-9))),
        "{:?}",
        half.curves
    );
    let ends: Vec<Vec2> = half.curves.iter().flat_map(|c| [c.start_point(), c.end_point()]).collect();
    for want in [Vec2::new(-24.0, 0.0), Vec2::new(-16.0, 0.0)] {
        assert!(ends.iter().any(|p| p.near(want, 1e-9)), "{want:?} not in {ends:?}");
    }
    assert!((half.area().abs() - pi * 4.0 * 2.0 / 2.0).abs() < 1e-9, "{}", half.area());
    // The tessellated points follow the curves (same box).
    for l in &h.loops {
        assert!(l.vertices.iter().all(|v| v.p.x < 0.0), "{:?}", l.vertices);
    }
}

fn same_wcs(a: &Hatch, b: &Hatch) {
    assert_eq!(a.loops.len(), b.loops.len());
    for (x, y) in a.loops.iter().zip(&b.loops) {
        assert_eq!(x.curves.len(), y.curves.len());
        for (cx, cy) in x.curves.iter().zip(&y.curves) {
            assert!(cx.start_point().near(cy.start_point(), 1e-9) && cx.end_point().near(cy.end_point(), 1e-9), "{cx:?} vs {cy:?}");
        }
        assert!((x.area() - y.area()).abs() < 1e-9, "{} vs {}", x.area(), y.area());
    }
}

#[test]
fn a_mirrored_ocs_hatch_keeps_its_wcs_geometry_through_dxf() {
    let h = mirrored_hatch();
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(Box::new(h.clone()))).expect("add");
    let back = hatches(&read_dxf(write_dxf(&d).as_bytes()).unwrap());
    same_wcs(&back[0], &h);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_mirrored_ocs_hatch_keeps_its_wcs_geometry_through_dwg() {
    let h = mirrored_hatch();
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(Box::new(h.clone()))).expect("add");
    let back = hatches(&read(&write(&d, "x.dwg").unwrap(), "x.dwg").unwrap());
    same_wcs(&back[0], &h);
}
