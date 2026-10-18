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
    d.model.iter().filter_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.clone()) } else { None }).collect()
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
        d.add(&Space::Model, Common::default(), EntityKind::Hatch(hatch(vec![l]))).expect("add");
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

#[test]
fn a_loop_with_a_broken_curve_keeps_its_points_only() {
    // A spline edge with too few control points: the loop reads, without an exact edge list.
    let text = "0\nSECTION\n2\nENTITIES\n0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n2\nSOLID\n70\n1\n71\n0\n\
                91\n1\n92\n1\n93\n2\n72\n4\n94\n3\n73\n0\n74\n0\n95\n2\n96\n1\n40\n0\n40\n1\n10\n0\n20\n0\n97\n0\n\
                72\n1\n10\n0\n20\n0\n11\n1\n21\n1\n97\n0\n75\n0\n76\n1\n98\n0\n0\nENDSEC\n0\nEOF\n";
    let d = read_dxf(text.as_bytes()).unwrap();
    let h = hatches(&d);
    assert_eq!(h.len(), 1);
    assert!(h[0].loops[0].curves.iter().all(LoopEdge::is_valid));
    let _ = loop_area(&h[0].loops[0].curves);
}
