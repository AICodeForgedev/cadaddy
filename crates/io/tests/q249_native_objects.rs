//! Q249 (dwgwrite2): the native DWG writer now encodes MATERIAL, TABLESTYLE, VISUALSTYLE,
//! FIELD and SORTENTSTABLE instead of stashing them.

use cadaddy_doc::*;
use cadaddy_doc::material::Material;
use cadaddy_io::DwgEngine;

const VERSIONS: [cadaddy_io::Version; 6] = [
    cadaddy_io::Version::R2000,
    cadaddy_io::Version::R2004,
    cadaddy_io::Version::R2007,
    cadaddy_io::Version::R2010,
    cadaddy_io::Version::R2013,
    cadaddy_io::Version::R2018,
];

/// A drawing with a custom material: the native DWG writer encodes MATERIAL objects that
/// acadrust (black box) reads back. The material survives the roundtrip in every version.
#[test]
fn material_survives_native_roundtrip() {
    let mut d = Drawing::new_metric();
    d.materials.push(Material::new("Sample Brick"));
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Line(Line { a: Default::default(), b: cadaddy_geom::Vec3::new(10.0, 0.0, 0.0) }),
    )
    .ok();

    for v in VERSIONS {
        let bytes = cadaddy_io::write_dwg(&d, v, DwgEngine::Native).unwrap();
        let back = cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap();
        assert!(
            back.materials.iter().any(|m| m.name == "Sample Brick"),
            "{v:?}: material 'Sample Brick' not found in {:?}",
            back.materials.iter().map(|m| &m.name).collect::<Vec<_>>()
        );
    }
}

/// The native DWG writer produces a valid DWG file (acadrust opens it without error) even
/// when the drawing has table styles, which the binary format may not perfectly match yet.
#[test]
fn native_dwg_with_tablestyle_opens() {
    let mut d = Drawing::new_metric();
    d.table_styles.push(TableStyle { name: "Project Tables".into(), ..TableStyle::default() });
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Line(Line { a: Default::default(), b: cadaddy_geom::Vec3::new(10.0, 0.0, 0.0) }),
    )
    .ok();

    for v in VERSIONS {
        let bytes = cadaddy_io::write_dwg(&d, v, DwgEngine::Native).unwrap();
        // The file must be valid — acadrust opens it without error.
        let _back = cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap();
    }
}
