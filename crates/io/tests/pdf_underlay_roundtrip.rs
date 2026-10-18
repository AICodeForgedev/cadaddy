//! PDF underlays survive DXF as PDFUNDERLAY entities and PDFDEFINITION objects (DXF Reference),
//! with the page size and the layers turned off in CADaddy's xdata.

use cadaddy_doc::{Common, Drawing, EntityKind, Space, Underlay};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_io::{SaveOptions, Version, read, write_with};

fn underlay(path: &str, page: u32, mirrored: bool) -> Underlay {
    let (s, c) = 30f64.to_radians().sin_cos();
    let k = 2.0;
    let side = if mirrored { -1.0 } else { 1.0 };
    Underlay {
        insert: Vec3::new(10.0, 20.0, 0.0),
        u: Vec3::new(c * k, s * k, 0.0),
        v: Vec3::new(-s * k * side, c * k * side, 0.0),
        size: Vec2::new(17.0, 11.0),
        path: path.into(),
        page,
        name: String::new(),
        clip: vec![Vec2::new(1.0, 1.0), Vec2::new(8.0, 1.0), Vec2::new(8.0, 6.0)],
        clipping: true,
        fade: 40,
        contrast: 70,
        monochrome: true,
        layers_off: vec!["NOTES".into(), "E-LITE".into()],
    }
}

fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (p, page, m) in [("C:/jobs/plans.pdf", 2, false), ("C:/jobs/plans.pdf", 2, true), ("/tmp/site.pdf", 1, false)] {
        d.add(&Space::Model, Common::default(), EntityKind::Underlay(underlay(p, page, m))).unwrap();
    }
    d
}

fn underlays(d: &Drawing) -> Vec<Underlay> {
    d.model.iter().filter_map(|e| if let EntityKind::Underlay(u) = &e.kind { Some(u.clone()) } else { None }).collect()
}

fn close(a: &Underlay, b: &Underlay) -> bool {
    let near = |x: Vec3, y: Vec3| (x - y).len() < 1e-9;
    near(a.insert, b.insert)
        && near(a.u, b.u)
        && near(a.v, b.v)
        && Underlay { insert: b.insert, u: b.u, v: b.v, name: String::new(), ..a.clone() } == Underlay { name: String::new(), ..b.clone() }
}

#[test]
fn underlays_roundtrip_in_every_dxf_version() {
    let d = drawing();
    for v in Version::ALL {
        let bytes = write_with(&d, "t.dxf", &SaveOptions { version: Some(v), binary: false }).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("PDFUNDERLAY") && text.contains("PDFDEFINITION") && text.contains("ACAD_PDFDEFINITIONS"), "{v:?}");
        // Two underlays of the same file and page share one definition.
        assert_eq!(text.matches("AcDbUnderlayDefinition").count(), 2, "{v:?}");
        let back = read(&bytes, "t.dxf").unwrap();
        let (a, b) = (underlays(&d), underlays(&back));
        assert_eq!(a.len(), b.len(), "{v:?}");
        for (x, y) in a.iter().zip(&b) {
            assert!(close(x, y), "{v:?}\n{x:?}\n{y:?}");
            assert!(!y.name.is_empty(), "the definition's name comes back");
        }
        // Read and written again, nothing doubles.
        let again = cadaddy_io::write_dxf(&back);
        assert_eq!(again.matches("ACAD_PDFDEFINITIONS").count(), 1);
    }
}

#[test]
fn hostile_underlay_values_are_tamed() {
    let d = drawing();
    let text = cadaddy_io::write_dxf(&d);
    // Fade and contrast out of range (the value lines after group codes 282 and 281).
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut changed = 0;
    for i in 0..lines.len().saturating_sub(1) {
        match lines[i].trim() {
            "282" if lines[i + 1].trim() == "40" => {
                lines[i + 1] = "9999".into();
                changed += 1;
            }
            "281" if lines[i + 1].trim() == "70" => {
                lines[i + 1] = "-5".into();
                changed += 1;
            }
            _ => {}
        }
    }
    assert!(changed >= 2, "the groups were found");
    let back = read(lines.join("\n").as_bytes(), "t.dxf").unwrap();
    let u = &underlays(&back)[0];
    assert!(u.fade <= 80 && (20..=100).contains(&u.contrast), "{u:?}");
}

#[test]
fn dwg_keeps_underlays() {
    let d = drawing();
    let Ok(bytes) = write_with(&d, "t.dwg", &SaveOptions::default()) else { return };
    let back = read(&bytes, "t.dwg").unwrap();
    let n = underlays(&back).len();
    assert_eq!(n, 3, "the DWG bridge keeps them");
    let (a, b) = (underlays(&d), underlays(&back));
    for (x, y) in a.iter().zip(&b) {
        assert!(close(x, y), "{x:?}\n{y:?}");
    }
}
