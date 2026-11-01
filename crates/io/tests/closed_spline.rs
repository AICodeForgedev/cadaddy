//! A closed fit-point spline is periodic: DXF flags it closed + periodic (group 70 bits 1 and 2)
//! and it reads back as the same smooth closed curve.

use cadaddy_doc::{Common, Drawing, EntityKind, Space};
use cadaddy_geom::{Spline, Vec2};

#[test]
fn closed_fit_spline_roundtrips_as_periodic() {
    let fit = [Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0), Vec2::new(0.0, 3.0)];
    let sp = Spline::from_fit_points_closed(&fit);
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Spline(Box::new(sp.clone()))).unwrap();
    let bytes = cadaddy_io::write(&d, "a.dxf").unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let spline = &text[text.find("SPLINE").unwrap()..];
    assert!(spline.contains("70\r\n11\r\n"), "planar + closed + periodic");
    let back = cadaddy_io::read(&bytes, "a.dxf").unwrap();
    let Some(EntityKind::Spline(r)) = back.model.iter().map(|e| e.kind.clone()).next() else { panic!("no spline") };
    assert!(r.closed && r.is_periodic());
    assert_eq!(r.fit.len(), 4);
    for i in 0..=20 {
        let t = i as f64 / 20.0;
        assert!(r.eval(t).near(sp.eval(t), 1e-9));
    }
}

/// A closed fit spline (what OFFSET of an ellipse makes) saved as DWG in every version reopens
/// closed with the same control points and knots; before R2013 it reopened open.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn closed_fit_spline_stays_closed_in_every_dwg_version() {
    let fit: Vec<Vec2> = (0..12)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / 12.0;
            Vec2::new(5.0 + 3.6 * a.cos(), 21.0 + 2.7 * a.sin())
        })
        .collect();
    let sp = Spline::from_fit_points_closed(&fit);
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Spline(Box::new(sp.clone()))).unwrap();
    for v in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, v).unwrap_or_else(|e| panic!("{v}: {e}"));
        let (back, report) = cadaddy_io::dwg_read::read(&bytes).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert!(report.failed.is_empty(), "{v}: {:?}", report.failed);
        let Some(EntityKind::Spline(r)) = back.model.iter().map(|e| e.kind.clone()).next() else { panic!("{v}: no spline") };
        assert!(r.closed && r.is_periodic(), "{v}: closed + periodic");
        assert_eq!(r.control.len(), sp.control.len(), "{v}: control points");
        assert_eq!(r.knots.len(), sp.knots.len(), "{v}: knots");
        for i in 0..=20 {
            let t = i as f64 / 20.0;
            assert!(r.eval(t).near(sp.eval(t), 1e-9), "{v}: curve at {t}");
        }
    }
}

/// The same through the public DWG save (`write_dwg`) with each engine: the default writer is the
/// bridge, which used to write a closed fit spline before R2013 in the fit-point form, reopened open.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn closed_fit_spline_stays_closed_through_the_public_save_with_each_engine() {
    use cadaddy_io::{DEFAULT_DWG_WRITE, DwgEngine, Version};
    let fit: Vec<Vec2> = (0..12)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / 12.0;
            Vec2::new(5.0 + 3.6 * a.cos(), 21.0 + 2.7 * a.sin())
        })
        .collect();
    let sp = Spline::from_fit_points_closed(&fit);
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Spline(Box::new(sp.clone()))).unwrap();
    assert_eq!(DEFAULT_DWG_WRITE, DwgEngine::Bridge);
    for engine in [DEFAULT_DWG_WRITE, DwgEngine::Native] {
        for v in [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
            let bytes = cadaddy_io::write_dwg(&d, v, engine).unwrap_or_else(|e| panic!("{engine:?} {v:?}: {e}"));
            let back = cadaddy_io::read_dwg(&bytes, engine).unwrap_or_else(|e| panic!("{engine:?} {v:?}: {e}"));
            let splines: Vec<_> =
                back.model.iter().filter_map(|e| if let EntityKind::Spline(s) = &e.kind { Some(s.clone()) } else { None }).collect();
            assert_eq!(splines.len(), 1, "{engine:?} {v:?}: one spline");
            let r = &splines[0];
            assert!(r.closed && r.is_periodic(), "{engine:?} {v:?}: closed + periodic");
            assert_eq!(r.control.len(), sp.control.len(), "{engine:?} {v:?}: control points");
            assert_eq!(r.knots.len(), sp.knots.len(), "{engine:?} {v:?}: knots");
            for i in 0..=20 {
                let t = i as f64 / 20.0;
                assert!(r.eval(t).near(sp.eval(t), 1e-6), "{engine:?} {v:?}: curve at {t}");
            }
        }
    }
}
