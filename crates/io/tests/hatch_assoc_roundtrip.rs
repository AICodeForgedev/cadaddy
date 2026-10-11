//! The source boundary objects of an associative HATCH (group 97 count, 330 handles per loop,
//! DXF Reference) survive a DXF save and read (#451).

use cadcraft_doc::*;
use cadcraft_geom::{PolyVertex, Vec2};
use cadcraft_io::{read_dxf, write_dxf};

fn square(x0: f64, s: f64) -> Vec<PolyVertex> {
    [(x0, x0), (x0 + s, x0), (x0 + s, x0 + s), (x0, x0 + s)].into_iter().map(|(x, y)| PolyVertex::new(Vec2::new(x, y))).collect()
}

fn hatch(loops: Vec<HatchLoop>, associative: bool) -> Hatch {
    Hatch {
        pattern: "ANSI31".into(),
        solid: false,
        loops,
        scale: 1.0,
        angle: 0.0,
        associative,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::ZERO,
        background: None,
        pattern_lines: Vec::new(),
    }
}

fn poly(d: &mut Drawing, vs: Vec<PolyVertex>) -> Handle {
    let k = EntityKind::LwPolyline(LwPolyline { vertices: vs, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false });
    d.add(&Space::Model, Common::default(), k).expect("add")
}

fn hatches(d: &Drawing) -> Vec<Hatch> {
    d.model.iter().filter_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.clone()) } else { None }).collect()
}

#[test]
fn source_boundary_objects_roundtrip() {
    let mut d = Drawing::new_imperial();
    let outer = poly(&mut d, square(0.0, 20.0));
    let island = poly(&mut d, square(5.0, 5.0));
    let loops = vec![
        HatchLoop { vertices: square(0.0, 20.0), outer: true, sources: vec![outer] },
        HatchLoop { vertices: square(5.0, 5.0), outer: false, sources: vec![island] },
    ];
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(hatch(loops.clone(), true))).expect("add");
    // A non-associative hatch writes no sources; a source that is gone isn't written.
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(hatch(vec![HatchLoop { sources: vec![outer], ..loops[0].clone() }], false)))
        .expect("add");
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(hatch(vec![HatchLoop { sources: vec![Handle(0xFFFF)], ..loops[0].clone() }], true)))
        .expect("add");
    let text = write_dxf(&d);
    let back = read_dxf(text.as_bytes()).expect("read");
    let hs = hatches(&back);
    assert_eq!(hs.len(), 3);
    assert!(hs[0].associative);
    assert_eq!(hs[0].loops.iter().map(|l| l.sources.clone()).collect::<Vec<_>>(), vec![vec![outer], vec![island]]);
    assert!(hs[1].loops[0].sources.is_empty());
    assert!(hs[2].loops[0].sources.is_empty());
    // Saved again, the links stay.
    let again = read_dxf(write_dxf(&back).as_bytes()).expect("read");
    assert_eq!(hatches(&again)[0].loops, hs[0].loops);
}

/// (group code, value) pairs of a DXF text.
fn pairs(text: &str) -> Vec<(i32, String)> {
    let lines: Vec<&str> = text.lines().collect();
    lines.chunks(2).filter_map(|c| Some((c.first()?.trim().parse().ok()?, c.get(1)?.trim().to_string()))).collect()
}

fn join(p: &[(i32, String)]) -> String {
    p.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

/// A HATCH whose edge boundary has a spline edge with fit data (its own 97 count) followed by
/// the source objects, and hostile counts and handles.
#[test]
fn sources_after_spline_fit_data_and_hostile_counts() {
    let mut d = Drawing::new_imperial();
    let outer = poly(&mut d, square(0.0, 20.0));
    let loops = vec![HatchLoop { vertices: square(0.0, 20.0), outer: true, sources: vec![outer] }];
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(hatch(loops, true))).expect("add");
    let p = pairs(&write_dxf(&d));
    let start = p.iter().position(|(c, v)| *c == 0 && v == "HATCH").expect("HATCH");
    let at = start + p.iter().skip(start).position(|(c, _)| *c == 92).expect("92");
    let end = at + p.iter().skip(at).position(|(c, _)| *c == 97).expect("97");
    assert_eq!(p.get(end + 1), Some(&(330, outer.hex())), "sources written after the boundary");
    let with = |boundary: &str, sources: &str| {
        let mut q: Vec<(i32, String)> = p[..at].to_vec();
        q.extend(pairs(boundary));
        q.extend(pairs(sources));
        q.extend(p[end + 2..].iter().cloned());
        read_dxf(join(&q).as_bytes()).expect("read")
    };
    let src = format!("97\n1\n330\n{}\n", outer.hex());
    // An edge boundary: one spline edge with two fit points.
    let spline = "92\n1\n93\n1\n72\n4\n94\n3\n73\n0\n74\n0\n95\n8\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n96\n4\n10\n0\n20\n0\n10\n20\n20\n0\n10\n20\n20\n20\n10\n0\n20\n20\n97\n2\n11\n0\n21\n0\n11\n0\n21\n20\n";
    assert_eq!(hatches(&with(spline, &src))[0].loops[0].sources, vec![outer]);
    let poly_loop = join(&p[at..end]);
    // Hostile: a huge count, unparsable and zero handles; a negative count.
    let back = with(&poly_loop, "97\n999999999999\n330\nZZZ\n330\n0\n330\n1F\n");
    assert_eq!(hatches(&back)[0].loops[0].sources, vec![Handle(0x1F)]);
    assert!(hatches(&with(&poly_loop, "97\n-5\n"))[0].loops[0].sources.is_empty());
}
