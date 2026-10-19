//! EPS and WMF export: structural checks of what is written.

use cadaddy_doc::{Common, Drawing, EntityKind, Line, Space};
use cadaddy_geom::Vec3;

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
