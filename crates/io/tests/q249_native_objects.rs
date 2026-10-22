//! Q342 (dwgwrite3): io-level tests for the Q249 object types that are now encoded natively.
//! MATERIAL: round-trips through native write → bridge read and native write → native read,
//! with field-level asserts (opacity, refraction, name). TABLESTYLE: stays on the stash path
//! (the DWG file is valid but TABLESTYLE goes through dwg_keep).

use cadaddy_doc::material::Material;
use cadaddy_doc::*;

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

/// A MATERIAL with non-default opacity and refraction survives a native DWG write → read.
#[test]
fn material_survives_native_roundtrip() {
    let mut d = Drawing::new_metric();
    d.materials.push(Material { name: "Test Glass".into(), opacity: 0.85, refraction: 1.52, ..Material::default() });
    for v in VERSIONS {
        let (bytes, report) = cadaddy_io::dwg_write::write(&d, v).unwrap();
        assert!(!report.skipped.iter().any(|(n, _)| n == "MATERIAL"), "{v}: MATERIAL should be encoded natively, not skipped");
        // Read back through the bridge.
        let (dxf_bytes, _notes) = cadaddy_dwg::dwg_to_dxf_noted(&bytes, cadaddy_dwg::Limits::DEFAULT).unwrap();
        let bridge = cadaddy_io::read(&dxf_bytes, "material.dxf").unwrap();
        let m_bridge = bridge.materials.iter().find(|m| m.name == "Test Glass");
        assert!(m_bridge.is_some(), "{v}: MATERIAL not found in bridge read-back");
        // Read back through our native reader.
        let native = cadaddy_io::read(&bytes, "material.dwg").unwrap();
        let m_native = native.materials.iter().find(|m| m.name == "Test Glass");
        assert!(m_native.is_some(), "{v}: MATERIAL not found in native read-back");
        if let Some(m) = m_native {
            assert!((m.opacity - 0.85).abs() < 0.01, "{v}: opacity {}", m.opacity);
            assert!((m.refraction - 1.52).abs() < 0.01, "{v}: refraction {}", m.refraction);
        }
    }
}

/// TABLESTYLE is stashed (encoder returns None), so the file is valid but the style goes
/// through dwg_keep.
#[test]
fn tablestyle_is_stashed_but_file_is_valid() {
    let d = Drawing::new_metric();
    // The default drawing has a TABLESTYLE (Standard). If the encoder returns None for it,
    // it goes through the stash path and the file should still be valid.
    for v in VERSIONS {
        let (bytes, _report) = cadaddy_io::dwg_write::write(&d, v).unwrap();
        // The bridge should be able to read it without error.
        let (_, notes) = cadaddy_dwg::dwg_to_dxf_noted(&bytes, cadaddy_dwg::Limits::DEFAULT).unwrap();
        assert!(notes.is_empty(), "{v}: bridge notes: {notes:?}");
        // Our reader should be able to read it without error.
        let _native = cadaddy_io::read(&bytes, "tablestyle.dwg").unwrap();
    }
}
