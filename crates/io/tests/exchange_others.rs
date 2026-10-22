//! Cases found by exchanging DXF with other programs (Q229), each rebuilt from a made-up file in
//! the shape those programs write (never their files).

use cadaddy_doc::{DimKind, EntityKind};
use cadaddy_io::read;

/// A minimal ENTITIES-only DXF around `entities` (group/value lines).
fn dxf(entities: &str) -> Vec<u8> {
    format!("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
}

/// Some programs write a dimension with no block and `42` (its measurement) as -1, or leave
/// `42` out: CADaddy drew it right but kept -1 as its measurement (what LIST, properties and
/// agents read). It is worked out from the definition points.
#[test]
fn a_dimension_without_its_measurement_gets_it_from_its_points() {
    let linear = "0\nDIMENSION\n8\n0\n10\n0.0\n20\n-5.0\n30\n0.0\n11\n20.0\n21\n-5.0\n31\n0.0\n70\n32\n42\n-1.0\n3\nStandard\n13\n0.0\n23\n0.0\n33\n0.0\n14\n40.0\n24\n0.0\n34\n0.0\n";
    let radius =
        "0\nDIMENSION\n8\n0\n10\n60.0\n20\n10.0\n30\n0.0\n11\n64.0\n21\n10.0\n31\n0.0\n70\n36\n3\nStandard\n15\n68.0\n25\n10.0\n35\n0.0\n40\n0.0\n";
    let d = read(&dxf(&format!("{linear}{radius}")), "theirs.dxf").unwrap();
    let dims: Vec<(String, f64)> = d
        .model
        .iter()
        .filter_map(|e| if let EntityKind::Dimension(dm) = &e.kind { Some((format!("{:?}", dm.kind), dm.measurement)) } else { None })
        .collect();
    assert_eq!(dims.len(), 2, "{dims:?}");
    assert!((dims[0].1 - 40.0).abs() < 1e-9, "{dims:?}");
    assert!((dims[1].1 - 8.0).abs() < 1e-9, "{dims:?}");
    assert!(matches!(&d.model.iter().next().unwrap().kind, EntityKind::Dimension(dm) if matches!(dm.kind, DimKind::Linear { .. })));
}

/// LibreCAD (dxfrw 0.6.3) writes INSERT z-scale as 0; a zero scale in any axis makes the
/// block invisible. We treat it as 1.0 (the default when the group code is missing) (Q253).
#[test]
fn insert_with_zero_z_scale_reads_as_one() {
    let insert = "0\nINSERT\n8\n0\n2\nTAG\n10\n100.0\n20\n60.0\n30\n0.0\n41\n1.0\n42\n1.0\n43\n0.0\n50\n0.0\n";
    let d = read(&dxf(insert), "librecad.dxf").unwrap();
    let ins = d.model.iter().find_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i) } else { None });
    let ins = ins.expect("INSERT entity");
    assert!((ins.scale.x - 1.0).abs() < 1e-9);
    assert!((ins.scale.y - 1.0).abs() < 1e-9);
    assert!((ins.scale.z - 1.0).abs() < 1e-9, "z-scale 0 should read as 1.0, got {}", ins.scale.z);
}
