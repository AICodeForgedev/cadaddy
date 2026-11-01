//! EPS and WMF export: structural checks of what is written.

use cadaddy_doc::{Common, Drawing, EntityKind, Line, RayLine, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let red = Common { color: cadaddy_doc::color::Color::Index(1), ..Common::default() };
    d.add(&Space::Model, red, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(200.0, 100.0, 0.0) })).unwrap();
    d.add(&Space::Model, Common::default(), EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(50.0, 50.0, 0.0), radius: 20.0 })).unwrap();
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Solid(cadaddy_doc::Solid {
            corners: [Vec3::new(120.0, 10.0, 0.0), Vec3::new(160.0, 10.0, 0.0), Vec3::new(120.0, 40.0, 0.0), Vec3::new(160.0, 40.0, 0.0)],
        }),
    )
    .unwrap();
    d
}

#[test]
fn eps_has_its_header_box_and_drawing() {
    let d = drawing();
    let bytes = cadaddy_io::write(&d, "plan.eps").unwrap();
    let s = String::from_utf8(bytes).unwrap();
    assert!(s.starts_with("%!PS-Adobe-3.0 EPSF-3.0\n"));
    assert!(s.trim_end().ends_with("%%EOF"));
    // The box keeps the drawing's proportions: 200 × 100 plus a 2% margin all round (208 : 108).
    let bb = s.lines().find_map(|l| l.strip_prefix("%%BoundingBox: ")).unwrap();
    let n: Vec<f64> = bb.split_whitespace().map(|x| x.parse().unwrap()).collect();
    assert_eq!((n[0], n[1]), (0.0, 0.0));
    assert!((n[2] / n[3] - 208.0 / 108.0).abs() < 0.01 && n[2] <= 721.0, "{bb}");
    assert!(s.contains("1.0000 0.0000 0.0000 setrgbcolor"), "the red line");
    assert!(s.contains(" stroke") && s.contains(" t\n"), "lines and fills");
    // Balanced save/restore.
    assert_eq!(s.matches("gsave").count(), s.matches("grestore").count());
    // A window frames it.
    let w = cadaddy_geom::Bounds2::new(cadaddy_geom::Vec2::new(0.0, 0.0), cadaddy_geom::Vec2::new(50.0, 50.0));
    let s = String::from_utf8(cadaddy_io::write_framed(&d, "x.eps", Some(w)).unwrap()).unwrap();
    assert!(s.contains("%%BoundingBox: 0 0 720 720"), "{}", s.lines().nth(1).unwrap());
}

/// The WMF's records: (function, parameter words), after checking the headers.
fn records(b: &[u8]) -> Vec<(u16, Vec<u16>)> {
    let w = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    // Placeable header: key, checksum over its first ten words.
    assert_eq!(u32::from(w(0)) | (u32::from(w(2)) << 16), cadaddy_io::wmf::PLACEABLE_KEY);
    let xor = (0..10).fold(0u16, |a, k| a ^ w(k * 2));
    assert_eq!(xor, w(20), "placeable checksum");
    // Metafile header at 22: type 1, 9 words, version 3.0, size in words.
    assert_eq!((w(22), w(24), w(26)), (1, 9, 0x0300));
    let size = u32::from(w(28)) | (u32::from(w(30)) << 16);
    assert_eq!(size as usize * 2, b.len() - 22, "the header's size is the file's");
    let max = u32::from(w(34)) | (u32::from(w(36)) << 16);
    let mut out = Vec::new();
    let mut at = 22 + 18;
    loop {
        let rs = (u32::from(w(at)) | (u32::from(w(at + 2)) << 16)) as usize;
        let f = w(at + 4);
        assert!(rs >= 3 && rs as u32 <= max, "record size {rs}");
        let params = (0..rs - 3).map(|k| w(at + 6 + k * 2)).collect();
        out.push((f, params));
        at += rs * 2;
        if f == cadaddy_io::wmf::META_EOF {
            break;
        }
    }
    assert_eq!(at, b.len(), "the end record ends the file");
    out
}

#[test]
fn wmf_is_a_placeable_metafile_with_pens_lines_and_fills() {
    use cadaddy_io::wmf::*;
    let d = drawing();
    let b = cadaddy_io::write(&d, "plan.wmf").unwrap();
    let r = records(&b);
    let count = |f: u16| r.iter().filter(|x| x.0 == f).count();
    assert!(count(META_POLYLINE) >= 2, "the line and the circle");
    assert!(count(META_POLYGON) >= 2, "the solid's triangles");
    // Every polyline's point count matches its words.
    for (f, p) in &r {
        if *f == META_POLYLINE || *f == META_POLYGON {
            assert_eq!(p.len(), 1 + 2 * p[0] as usize);
        }
    }
    // A red pen (COLORREF 0x000000FF) is made, and every object made is deleted.
    assert!(r.iter().any(|(f, p)| *f == META_CREATEPENINDIRECT && p[3] == 0x00FF && p[4] == 0));
    assert_eq!(count(META_CREATEPENINDIRECT) + count(META_CREATEBRUSHINDIRECT), count(META_DELETEOBJECT));
    // The window's extent is the picture's: 30000 on the long side.
    let ext = r.iter().find(|x| x.0 == META_SETWINDOWEXT).unwrap();
    assert_eq!(ext.1[1], 30000);
    // An empty drawing still writes a valid file.
    let empty = cadaddy_io::write(&Drawing::new_metric(), "e.wmf").unwrap();
    assert_eq!(records(&empty).last().unwrap().0, META_EOF);
}

/// The POLYLINE records' points as (x, y) logical units.
fn polylines(b: &[u8]) -> Vec<Vec<(i16, i16)>> {
    records(b)
        .into_iter()
        .filter(|(f, _)| *f == cadaddy_io::wmf::META_POLYLINE)
        .map(|(_, p)| p[1..].chunks(2).map(|c| (c[0] as i16, c[1] as i16)).collect())
        .collect()
}

/// Angle (degrees, 0…180, y up) of the WMF segment `a`→`b` (logical y runs down), and the
/// distance of `through` from the line.
fn angle_and_miss(a: (i16, i16), b: (i16, i16), through: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (f64::from(b.0) - f64::from(a.0), -(f64::from(b.1) - f64::from(a.1)));
    let ang = dy.atan2(dx).to_degrees().rem_euclid(180.0);
    let miss = ((through.0 - f64::from(a.0)) * dy - (-(through.1 - f64::from(a.1))) * dx).abs() / dx.hypot(dy);
    (ang, miss)
}

fn line_drawing(a: Vec3, b: Vec3) -> Drawing {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a, b })).unwrap();
    d
}

#[test]
fn wmf_xline_keeps_its_angle_and_place() {
    // A 30° construction line through (40, 30), framed to (0,0)–(100,100): 300 units per drawing unit.
    let mut d = Drawing::new_metric();
    let dir = Vec3::new(30f64.to_radians().cos(), 30f64.to_radians().sin(), 0.0);
    d.add(&Space::Model, Common::default(), EntityKind::XLine(RayLine { base: Vec3::new(40.0, 30.0, 0.0), dir })).unwrap();
    let w = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
    let b = cadaddy_io::write_framed(&d, "x.wmf", Some(w)).unwrap();
    let lines = polylines(&b);
    assert_eq!(lines.len(), 1);
    let pts = &lines[0];
    assert_eq!(pts.len(), 2);
    let (ang, miss) = angle_and_miss(pts[0], pts[1], (40.0 * 300.0, (100.0 - 30.0) * 300.0));
    assert!((ang - 30.0).abs() < 0.5, "angle {ang}");
    assert!(miss < 2.0, "off the point by {miss}");
}

#[test]
fn wmf_framed_diagonal_keeps_its_slope() {
    // A line from (-300,-100) to (700,400) (slope 0.5, through (40, 70)) shown in a 100 × 100 window.
    let d = line_drawing(Vec3::new(-300.0, -100.0, 0.0), Vec3::new(700.0, 400.0, 0.0));
    let w = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
    let b = cadaddy_io::write_framed(&d, "x.wmf", Some(w)).unwrap();
    let lines = polylines(&b);
    assert_eq!(lines.len(), 1);
    let pts = &lines[0];
    let (ang, miss) = angle_and_miss(pts[0], *pts.last().unwrap(), (40.0 * 300.0, (100.0 - 70.0) * 300.0));
    assert!((ang - 0.5f64.atan().to_degrees()).abs() < 0.5, "angle {ang}");
    assert!(miss < 2.0, "off the point by {miss}");
}

#[test]
fn eps_and_wmf_put_the_red_line_where_the_transform_says() {
    // The red line (0,0)–(200,100); extents 200 × 100 plus 4 units all round: 208 × 108.
    let d = drawing();
    let eps = String::from_utf8(cadaddy_io::write(&d, "plan.eps").unwrap()).unwrap();
    // 720 pt over 208 units: (0,0) → (13.846, 13.846), (200,100) → (706.154, 360.000), y up.
    assert!(eps.contains("13.846 13.846 m 706.154 360.000 l"), "{eps}");
    // WMF: 30000 over 208 units, y down from the top edge (104).
    let lines = polylines(&cadaddy_io::write(&d, "plan.wmf").unwrap());
    let near = |p: (i16, i16), x: f64, y: f64| (f64::from(p.0) - x).abs() <= 2.0 && (f64::from(p.1) - y).abs() <= 2.0;
    let k = 30000.0 / 208.0;
    assert!(
        lines.iter().any(|l| l.len() == 2 && near(l[0], 4.0 * k, 104.0 * k) && near(l[1], 204.0 * k, 4.0 * k)),
        "red line in logical units: {lines:?}"
    );
}

#[test]
fn eps_writes_text_as_text() {
    let mut d = Drawing::new_metric();
    let text = cadaddy_doc::Text {
        insert: Vec3::new(10.0, 20.0, 0.0),
        align_pt: None,
        height: 5.0,
        value: "PANEL A".into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: cadaddy_doc::HAlign::Left,
        valign: cadaddy_doc::VAlign::Baseline,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Text(text)).unwrap();
    // Framed to (0,0)–(100,100): 7.2 pt per unit, so the insert is at (72, 144) and the size 36.
    let w = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
    let s = String::from_utf8(cadaddy_io::write_framed(&d, "x.eps", Some(w)).unwrap()).unwrap();
    let line = s.lines().find(|l| l.contains("(PANEL A) show")).expect("the word as text");
    let size: f64 = line.split(" scalefont").next().unwrap().rsplit(' ').next().unwrap().parse().unwrap();
    assert!((size - 36.0).abs() <= 1.8, "size {size}");
    let m: Vec<f64> = line.split('[').nth(1).unwrap().split(']').next().unwrap().split_whitespace().map(|x| x.parse().unwrap()).collect();
    assert_eq!(m.len(), 6);
    assert!((m[0] - 1.0).abs() < 1e-3 && m[1].abs() < 1e-3, "along the baseline");
    assert!((m[4] - 72.0).abs() <= 1.0 && (m[5] - 144.0).abs() <= 1.0, "origin {:?}", &m[4..]);
}
