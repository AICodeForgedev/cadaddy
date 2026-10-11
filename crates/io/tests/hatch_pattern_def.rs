//! HATCH pattern definition lines (groups 78, 53, 43–46, 79, 49) of a pattern the library
//! doesn't define survive a DXF read and save (issue #290).

use cadcraft_doc::library::PatternLine;
use cadcraft_doc::*;
use cadcraft_geom::{PolyVertex, Vec2};
use cadcraft_io::{read_dxf, write_dxf};

fn square() -> HatchLoop {
    let v = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)].into_iter().map(|(x, y)| PolyVertex::new(Vec2::new(x, y))).collect();
    HatchLoop { vertices: v, outer: true }
}

fn hatch(pattern: &str, scale: f64, angle: f64, origin: Vec2, lines: Vec<PatternLine>) -> Hatch {
    Hatch {
        pattern: pattern.into(),
        solid: false,
        loops: vec![square()],
        scale,
        angle,
        associative: false,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin,
        background: None,
        pattern_lines: lines,
    }
}

fn custom_lines() -> Vec<PatternLine> {
    vec![
        PatternLine { angle: 0.0, origin: (0.0, 0.0), delta: (0.5, 1.0), dashes: vec![0.75, -0.25, 0.0, -0.25] },
        PatternLine { angle: 90.0, origin: (0.25, 0.0), delta: (0.0, 1.0), dashes: vec![] },
    ]
}

fn save(h: Hatch) -> String {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(h)).expect("add");
    write_dxf(&d)
}

fn first_hatch(text: &str) -> Hatch {
    let d = read_dxf(text.as_bytes()).expect("read");
    d.model.iter().find_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.clone()) } else { None }).expect("hatch")
}

/// (group code, value) pairs of a DXF text.
fn pairs(text: &str) -> Vec<(i32, String)> {
    let lines: Vec<&str> = text.lines().collect();
    lines.chunks(2).filter_map(|c| Some((c.first()?.trim().parse().ok()?, c.get(1)?.trim().to_string()))).collect()
}

/// Index of the HATCH's 78 group.
fn pattern_data(p: &[(i32, String)]) -> usize {
    let hatch = p.iter().position(|(c, v)| *c == 0 && v == "HATCH").expect("HATCH");
    p.iter().skip(hatch).position(|(c, _)| *c == 78).map(|k| hatch + k).expect("78")
}

fn join(p: &[(i32, String)]) -> String {
    p.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

fn near(a: &[PatternLine], b: &[PatternLine]) -> bool {
    let close = |x: f64, y: f64| (x - y).abs() < 1e-9;
    a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| {
            close(p.angle, q.angle)
                && close(p.origin.0, q.origin.0)
                && close(p.origin.1, q.origin.1)
                && close(p.delta.0, q.delta.0)
                && close(p.delta.1, q.delta.1)
                && p.dashes.len() == q.dashes.len()
                && p.dashes.iter().zip(&q.dashes).all(|(x, y)| close(*x, *y))
        })
}

#[test]
fn custom_pattern_lines_roundtrip_scaled_and_rotated() {
    let angle = 30f64.to_radians();
    let text = save(hatch("MYPAT", 2.0, angle, Vec2::new(1.0, 2.0), custom_lines()));
    // In the file the lines are as drawn: rotated with the hatch, scaled, offset as a world vector.
    let p = pairs(&text);
    let at = pattern_data(&p);
    assert_eq!(p[at].1, "2");
    let num = |k: usize| p[at + k].1.parse::<f64>().expect("number");
    assert!((num(1) - 30.0).abs() < 1e-9, "53 angle");
    let off = Vec2::new(0.5, 1.0).rotate(angle) * 2.0;
    assert!((num(2) - 1.0).abs() < 1e-9 && (num(3) - 2.0).abs() < 1e-9, "43/44 base point at the origin");
    assert!((num(4) - off.x).abs() < 1e-9 && (num(5) - off.y).abs() < 1e-9, "45/46 offset");
    assert_eq!(p[at + 6], (79, "4".into()));
    assert!((num(7) - 1.5).abs() < 1e-9, "49 dash scaled");
    // Read back to the same definition, and saved again unchanged.
    let h = first_hatch(&text);
    assert!(near(&h.pattern_lines, &custom_lines()), "{:?}", h.pattern_lines);
    assert!(near(&first_hatch(&save(h)).pattern_lines, &custom_lines()));
}

#[test]
fn library_patterns_keep_no_definition() {
    let h = first_hatch(&save(hatch("ANSI31", 1.0, 0.0, Vec2::ZERO, vec![])));
    assert!(h.pattern_lines.is_empty());
}

#[test]
fn hostile_pattern_definitions_are_capped() {
    let text = save(hatch("MYPAT", 1.0, 0.0, Vec2::ZERO, custom_lines()));
    let p = pairs(&text);
    let at = pattern_data(&p);
    let end = p.iter().skip(at).position(|(c, _)| *c == 98).map(|k| at + k).expect("98");
    let mut hostile = vec![(78, "2000000000".to_string())];
    // A line with a NaN offset, a line with a million dashes, then far too many lines.
    hostile.extend([(53, "0"), (43, "0"), (44, "0"), (45, "nan"), (46, "1"), (79, "1"), (49, "1")].map(|(c, v)| (c, v.to_string())));
    hostile.extend([(53, "0"), (43, "0"), (44, "0"), (45, "0"), (46, "1"), (79, "1000000")].map(|(c, v)| (c, v.to_string())));
    hostile.extend((0..1_000).map(|_| (49, "0.5".to_string())));
    for _ in 0..3_000 {
        hostile.extend([(53, "45"), (43, "0"), (44, "0"), (45, "0"), (46, "1"), (79, "0")].map(|(c, v)| (c, v.to_string())));
    }
    let mut q = p[..at].to_vec();
    q.extend(hostile);
    q.extend_from_slice(&p[end..]);
    let h = first_hatch(&join(&q));
    assert!(h.pattern_lines.len() <= 1024 && h.pattern_lines.len() > 100, "{}", h.pattern_lines.len());
    assert!(h.pattern_lines.iter().all(|l| l.delta.0.is_finite() && l.dashes.len() <= 256));
    // Drawing the capped definition stays bounded.
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(h)).expect("add");
    let _ = cadcraft_render::build(&d, &Space::Model, &cadcraft_render::Options::default());
}
