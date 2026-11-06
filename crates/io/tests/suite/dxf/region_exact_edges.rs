//! A REGION whose loops have ellipse or spline curves keeps them through DXF and DWG files:
//! read back, the loops hold the same curves, so the region's area stays exact.

use cadaddy_doc::*;
use cadaddy_geom::{LoopEdge, Spline, Vec2};
use cadaddy_io::{read, read_dxf, write, write_dxf};

fn region(loops: Vec<HatchLoop>) -> Drawing {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Region(Region { loops })).expect("add");
    d
}

fn regions(d: &Drawing) -> Vec<Region> {
    d.model.iter().filter_map(|e| if let EntityKind::Region(r) = &e.kind { Some(r.clone()) } else { None }).collect()
}

/// A whole ellipse, a closed spline, and an ellipse with a circular hole in it (two loops).
fn samples() -> Vec<Region> {
    let ellipse = HatchLoop::from_curves(vec![LoopEdge::full_ellipse(Vec2::new(3.0, 2.0), Vec2::new(5.0, 0.0), 0.4)], true, Vec::new());
    let spline = HatchLoop::from_curves(
        vec![LoopEdge::Spline(Spline::from_fit_points_closed(&[Vec2::new(0.0, 0.0), Vec2::new(4.0, 1.0), Vec2::new(5.0, 4.0), Vec2::new(1.0, 5.0)]))],
        true,
        Vec::new(),
    );
    let outer = HatchLoop::from_curves(vec![LoopEdge::full_ellipse(Vec2::ZERO, Vec2::new(8.0, 0.0), 0.5)], true, Vec::new());
    let hole = HatchLoop::from_curves(vec![LoopEdge::full_ellipse(Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.5), 0.5)], false, Vec::new());
    vec![Region { loops: vec![ellipse] }, Region { loops: vec![spline] }, Region { loops: vec![outer, hole] }]
}

fn check(back: &Drawing, want: &[Region]) {
    let got = regions(back);
    assert_eq!(got.len(), want.len(), "regions read back");
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g.loops.len(), w.loops.len());
        for (lg, lw) in g.loops.iter().zip(&w.loops) {
            assert!(!lw.curves.is_empty());
            assert_eq!(lg.curves.len(), lw.curves.len(), "{:?}", lg.curves);
            for (cg, cw) in lg.curves.iter().zip(&lw.curves) {
                assert_eq!(std::mem::discriminant(cg), std::mem::discriminant(cw), "{cg:?} vs {cw:?}");
                assert!(cg.start_point().near(cw.start_point(), 1e-9) && cg.end_point().near(cw.end_point(), 1e-9), "{cg:?} vs {cw:?}");
            }
            assert_eq!(lg.outer, lw.outer);
        }
        assert!((g.area() - w.area()).abs() < 1e-9, "{} vs {}", g.area(), w.area());
    }
}

fn drawing(want: &[Region]) -> Drawing {
    let mut d = Drawing::new_imperial();
    for r in want {
        d.add(&Space::Model, Common::default(), EntityKind::Region(r.clone())).expect("add");
    }
    d
}

#[test]
fn region_curves_roundtrip_through_dxf() {
    let want = samples();
    let d = drawing(&want);
    let back = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    check(&back, &want);
    // Analytic truth: the ellipse is π·5·2, and the third region the big ellipse less the hole.
    let a = regions(&back);
    let pi = std::f64::consts::PI;
    assert!((a[0].area() - pi * 10.0).abs() < 1e-9, "{}", a[0].area());
    assert!((a[2].area() - (pi * 8.0 * 4.0 - pi * 1.5 * 0.75)).abs() < 1e-9, "{}", a[2].area());
    // And again: the same curves.
    check(&read_dxf(write_dxf(&back).as_bytes()).unwrap(), &want);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn region_curves_roundtrip_through_dwg() {
    let want = samples();
    let d = drawing(&want);
    let back = read(&write(&d, "x.dwg").unwrap(), "x.dwg").unwrap();
    check(&back, &want);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn region_curves_roundtrip_through_the_native_writer_in_every_version() {
    let want = samples();
    let d = drawing(&want);
    for v in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, v).unwrap_or_else(|e| panic!("{v}: {e}"));
        let (back, report) = cadaddy_io::dwg_read::read(&bytes).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert!(report.failed.is_empty(), "{v}: {:?}", report.failed);
        check(&back, &want);
    }
}

#[test]
fn a_region_of_lines_and_arcs_has_no_curve_records() {
    let sq = HatchLoop::from_curves(
        vec![
            LoopEdge::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(2.0, 0.0) },
            LoopEdge::Line { a: Vec2::new(2.0, 0.0), b: Vec2::new(2.0, 2.0) },
            LoopEdge::Line { a: Vec2::new(2.0, 2.0), b: Vec2::new(0.0, 2.0) },
            LoopEdge::Line { a: Vec2::new(0.0, 2.0), b: Vec2::new(0.0, 0.0) },
        ],
        true,
        Vec::new(),
    );
    let text = write_dxf(&region(vec![sq]));
    assert!(!text.lines().any(|l| l.trim() == "EDGE"));
    let back = read_dxf(text.as_bytes()).unwrap();
    assert!((regions(&back)[0].area() - 4.0).abs() < 1e-12);
}

/// Edge records that don't belong to the loop (a hand-edited file) are dropped: the points stay.
#[test]
fn edge_records_that_do_not_match_the_points_are_dropped() {
    let want = samples();
    let text = write_dxf(&drawing(&want[..1]));
    // Make the ellipse edge's major axis ten times too long: its area is far off the points'.
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.windows(2).position(|w| w[0].trim() == "1000" && w[1].trim() == "EDGE").unwrap();
    let mut out: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
    // After the kind record: centre x, centre y, then the major axis x.
    let major_x = (at..out.len()).filter(|&i| out[i].trim() == "1040").nth(2).unwrap() + 1;
    out[major_x] = "50".into();
    let back = read_dxf(out.join("\n").as_bytes()).unwrap();
    let r = &regions(&back)[0];
    assert!(r.loops[0].curves.is_empty(), "{:?}", r.loops[0].curves);
    assert!(!r.loops[0].vertices.is_empty());
    assert!((r.area().abs() - std::f64::consts::PI * 10.0).abs() < 0.05 * std::f64::consts::PI * 10.0);
}
