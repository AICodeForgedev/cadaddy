//! DWF and DWFx underlays survive DXF, DWG and .cdy as DWF underlays: DWFUNDERLAY entities and
//! DWFDEFINITION objects in `ACAD_DWFDEFINITIONS` (DXF Reference), not as PDF underlays.

use cadaddy_doc::{Common, Drawing, EntityKind, Space, Underlay};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_io::{DwgEngine, SaveOptions, Version, read, write_dwg, write_with};

fn underlay(path: &str, page: u32) -> Underlay {
    Underlay {
        insert: Vec3::new(10.0, 20.0, 0.0),
        u: Vec3::new(2.0, 0.0, 0.0),
        v: Vec3::new(0.0, 2.0, 0.0),
        size: Vec2::new(17.0, 11.0),
        path: path.into(),
        page,
        name: String::new(),
        clip: vec![Vec2::new(1.0, 1.0), Vec2::new(8.0, 6.0)],
        clipping: true,
        fade: 40,
        contrast: 70,
        monochrome: false,
        layers_off: vec!["E-POWER".into()],
    }
}

/// A DWFx page 2, a DWF page 1 and, for contrast, a PDF.
fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (p, page) in [("sheets/Sample Plan.dwfx", 2), ("sheets/Sample Plan.dwf", 1), ("sheets/Sample Plan.pdf", 1)] {
        d.add(&Space::Model, Common::default(), EntityKind::Underlay(Box::new(underlay(p, page)))).unwrap();
    }
    d
}

fn underlays(d: &Drawing) -> Vec<Underlay> {
    d.model.iter().filter_map(|e| if let EntityKind::Underlay(u) = &e.kind { Some(u.as_ref().clone()) } else { None }).collect()
}

fn same(a: &[Underlay], b: &[Underlay], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}");
    for (x, y) in a.iter().zip(b) {
        assert_eq!(Underlay { name: String::new(), ..y.clone() }, Underlay { name: String::new(), ..x.clone() }, "{what}");
    }
}

#[test]
fn dxf_writes_dwf_underlays_as_dwf_in_every_version() {
    let d = drawing();
    let u = underlays(&d);
    assert!(u[0].is_dwf() && u[1].is_dwf() && !u[2].is_dwf());
    assert_eq!(d.model.iter().map(|e| e.kind.dxf_name()).collect::<Vec<_>>(), ["DWFUNDERLAY", "DWFUNDERLAY", "PDFUNDERLAY"]);
    for v in Version::ALL.into_iter().filter(|v| *v != Version::R12) {
        let bytes = write_with(&d, "t.dxf", &SaveOptions { version: Some(v), binary: false }).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let count = |s: &str| lines.iter().filter(|l| **l == s).count();
        assert_eq!((count("DWFUNDERLAY"), count("PDFUNDERLAY")), (3, 2), "{v:?}: two entities and the class each");
        assert_eq!((count("DWFDEFINITION"), count("ACAD_DWFDEFINITIONS")), (3, 1), "{v:?}: two definitions and the class");
        assert_eq!((count("AcDbDwfReference"), count("AcDbDwfDefinition")), (1, 1), "{v:?}");
        // The definition holds the file and the page.
        assert!(lines.windows(4).any(|w| w == ["1", "sheets/Sample Plan.dwfx", "2", "2"]), "{v:?}");
        let back = read(&bytes, "t.dxf").unwrap();
        same(&u, &underlays(&back), &format!("{v:?}"));
        // Read and written again, nothing doubles.
        let again = cadaddy_io::write_dxf(&back);
        assert_eq!(again.lines().filter(|l| l.trim() == "ACAD_DWFDEFINITIONS").count(), 1);
    }
}

#[test]
fn dwg_and_cdy_keep_dwf_underlays() {
    let d = drawing();
    let u = underlays(&d);
    for engine in [DwgEngine::Bridge, DwgEngine::Native] {
        for v in [Version::R2004, Version::R2010, Version::R2018] {
            let bytes = write_dwg(&d, v, engine).unwrap();
            // The DWG's own classes say DWF underlay (as the DXF does), next to the PDF one. (The
            // native writer, not the default yet, writes no underlay classes of any kind.)
            if engine == DwgEngine::Native {
                let back = read(&bytes, "t.dwg").unwrap();
                same(&u, &underlays(&back), &format!("{engine:?} {v:?}"));
                continue;
            }
            let classes = cadaddy_dwgcodec::read(&bytes).unwrap().classes().unwrap();
            let names: Vec<String> = classes.classes.iter().map(|c| c.dxf_name.to_string_lossy()).collect();
            for want in ["DWFUNDERLAY", "DWFDEFINITION", "PDFUNDERLAY", "PDFDEFINITION"] {
                assert!(names.iter().any(|n| n == want), "{engine:?} {v:?}: {want} in {names:?}");
            }
            let back = read(&bytes, "t.dwg").unwrap();
            same(&u, &underlays(&back), &format!("{engine:?} {v:?}"));
            assert_eq!(back.model.iter().map(|e| e.kind.dxf_name()).collect::<Vec<_>>(), ["DWFUNDERLAY", "DWFUNDERLAY", "PDFUNDERLAY"]);
        }
    }
    let bytes = write_with(&d, "t.cdy", &SaveOptions::default()).unwrap();
    same(&u, &underlays(&read(&bytes, "t.cdy").unwrap()), "cdy");
}
