//! A dimension's stored measurement and defining points survive a DWG save and reopen, in every
//! DWG version and through DWG → DXF → DWG (B4-012): radial dimensions read back at 2x and
//! angular ones changed because the saved value was recalculated from the geometry.

use cadaddy_doc::{Common, DimKind, Dimension, Drawing, EntityKind, Space};
use cadaddy_geom::Vec3;

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

fn v(x: f64, y: f64) -> Vec3 {
    Vec3::new(x, y, 0.0)
}

fn dim(kind: DimKind, defpt: Vec3, p13: Vec3, p14: Vec3, p15: Vec3, p16: Vec3, measurement: f64) -> Dimension {
    Dimension {
        insertion_point: Default::default(),
        kind,
        defpt,
        text_mid: v(5.0, 6.0),
        p13,
        p14,
        p15,
        p16,
        text: String::new(),
        style: "Standard".into(),
        measurement,
        text_rotation: 0.0,
        user_text_pos: false,
        block: None,
        overrides: Default::default(),
        assoc: Vec::new(),
        marks: Default::default(),
    }
}

/// One dimension per kind. Radial, diameter and arc-length carry a stored value that differs from
/// the geometry's (a linear factor), and the angular ones a stored value the geometry would not
/// give back either.
fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let all = [
        dim(DimKind::Radius, v(10.0, 10.0), Vec3::ZERO, Vec3::ZERO, v(24.79, 10.0), Vec3::ZERO, 7.395),
        dim(DimKind::Diameter, v(10.0, 20.0), Vec3::ZERO, Vec3::ZERO, v(30.0, 20.0), Vec3::ZERO, 10.0),
        dim(DimKind::Angular, v(15.0, 15.0), v(0.0, 0.0), v(10.0, 0.0), v(0.0, 0.0), v(0.0, 10.0), 0.04),
        dim(DimKind::Angular3P, v(12.0, 12.0), v(10.0, 0.0), v(0.0, 10.0), v(0.0, 0.0), Vec3::ZERO, 1.3),
        dim(DimKind::Linear { rotation: 0.0 }, v(5.0, 3.0), v(0.0, 0.0), v(10.0, 0.0), Vec3::ZERO, Vec3::ZERO, 4.5),
        dim(DimKind::Aligned, v(5.0, 8.0), v(0.0, 5.0), v(10.0, 5.0), Vec3::ZERO, Vec3::ZERO, 9.75),
        dim(DimKind::Ordinate { x_type: true }, v(40.0, 0.0), v(40.0, 5.0), v(42.0, 8.0), Vec3::ZERO, Vec3::ZERO, 40.0),
        dim(DimKind::ArcLength, v(12.0, 12.0), v(10.0, 0.0), v(0.0, 10.0), Vec3::ZERO, Vec3::ZERO, 15.7),
    ];
    for x in all {
        d.add(&Space::Model, Common::default(), EntityKind::Dimension(Box::new(x))).unwrap();
    }
    d
}

fn dims(d: &Drawing) -> Vec<Dimension> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Dimension(dm) => Some((**dm).clone()),
            _ => None,
        })
        .collect()
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6
}

fn same(label: &str, a: &[Dimension], b: &[Dimension]) {
    assert_eq!(a.len(), b.len(), "{label}: count");
    for (x, y) in a.iter().zip(b) {
        let k = format!("{label} {:?}", x.kind);
        assert!((x.measurement - y.measurement).abs() < 1e-9, "{k}: measurement {} -> {}", x.measurement, y.measurement);
        assert!(close(x.defpt, y.defpt), "{k}: defpt {:?} -> {:?}", x.defpt, y.defpt);
        let (p13, p14, p15, p16) = match x.kind {
            DimKind::Radius | DimKind::Diameter => (false, false, true, false),
            DimKind::Angular => (true, true, true, true),
            DimKind::Angular3P => (true, true, true, false),
            _ => (true, true, false, false),
        };
        assert!(!p13 || close(x.p13, y.p13), "{k}: p13 {:?} -> {:?}", x.p13, y.p13);
        assert!(!p14 || close(x.p14, y.p14), "{k}: p14 {:?} -> {:?}", x.p14, y.p14);
        assert!(!p15 || close(x.p15, y.p15), "{k}: p15 {:?} -> {:?}", x.p15, y.p15);
        assert!(!p16 || close(x.p16, y.p16), "{k}: p16 {:?} -> {:?}", x.p16, y.p16);
    }
}

fn reopened(d: &Drawing, ver: &str) -> Drawing {
    let (bytes, _) = cadaddy_io::dwg_write::write(d, ver).unwrap();
    cadaddy_io::dwg_read::read(&bytes).unwrap().0
}

#[test]
fn every_kind_keeps_its_measurement_and_points_through_dwg_in_every_version() {
    let d = drawing();
    let before = dims(&d);
    assert_eq!(before.len(), 8);
    for ver in VERSIONS {
        let once = reopened(&d, ver);
        same(&format!("{ver} first"), &before, &dims(&once));
        // A second generation of the same file.
        let twice = reopened(&once, ver);
        same(&format!("{ver} second"), &before, &dims(&twice));
    }
}

#[test]
fn every_kind_keeps_its_measurement_and_points_through_dwg_dxf_dwg() {
    let d = drawing();
    let before = dims(&d);
    for ver in VERSIONS {
        let dwg = reopened(&d, ver);
        let dxf = cadaddy_io::write_dxf(&dwg);
        let via_dxf = cadaddy_io::read_dxf(dxf.as_bytes()).unwrap();
        same(&format!("{ver} via DXF"), &before, &dims(&via_dxf));
        let back = reopened(&via_dxf, ver);
        same(&format!("{ver} DWG-DXF-DWG"), &before, &dims(&back));
    }
}

/// The battle-test file (local only, never committed): its seven dimensions keep their values.
#[test]
fn the_carriage_drawings_dimensions_keep_their_values() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let path = std::path::Path::new(&home).join("cadwork/tasks/fixtures/battle4/orig_carriage.dwg");
    let Ok(bytes) = std::fs::read(path) else { return };
    let (d, _) = cadaddy_io::dwg_read::read(&bytes).unwrap();
    let before = dims(&d);
    assert!(!before.is_empty());
    for ver in VERSIONS {
        same(&format!("carriage {ver}"), &before, &dims(&reopened(&d, ver)));
    }
}
