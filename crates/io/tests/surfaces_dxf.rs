//! NURBS surfaces in files (Q160): saved as the mesh of their kept part, which other programs read
//! (MESH from 2010 on, a polyface mesh before), with CADaddy's definition (control vertices,
//! knots, weights, trims, CV display) in the mesh's xdata, so CADaddy reads the surface back
//! exactly from DXF, DWG and .cdy.

use cadaddy_doc::*;
use cadaddy_geom::{NurbsSurface, Vec3};
use cadaddy_io::{SaveOptions, Version, read_dxf, write_with};

fn surfaces() -> Vec<Surface3d> {
    let flat = NurbsSurface::bilinear(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 6.0, 0.0), Vec3::new(10.0, 6.0, 3.0)).unwrap();
    let rows: Vec<Vec<Vec3>> =
        (0..6).map(|i| (0..5).map(|j| Vec3::new(i as f64 * 2.0, j as f64 * 2.0, ((i * j) as f64 * 0.3).sin())).collect()).collect();
    let mut wavy = Surface3d::new(NurbsSurface::interpolate(&rows, 3, 3).unwrap());
    wavy.holes.push(vec![[0.3, 0.3], [0.6, 0.3], [0.6, 0.6], [0.3, 0.6]]);
    wavy.cvs_shown = true;
    let mut rational = Surface3d::new(NurbsSurface::interpolate(&rows, 2, 3).unwrap());
    rational.nurbs.weights = (0..rational.nurbs.control.len()).map(|i| 1.0 + (i % 3) as f64 * 0.25).collect();
    vec![Surface3d::new(flat), wavy, rational]
}

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    for s in surfaces() {
        d.add(&Space::Model, Common::default(), EntityKind::Surface(Box::new(s))).unwrap();
    }
    d
}

fn back(d: &Drawing) -> Vec<Surface3d> {
    d.model.iter().filter_map(|e| if let EntityKind::Surface(s) = &e.kind { Some((**s).clone()) } else { None }).collect()
}

#[test]
fn surfaces_round_trip_exactly_in_new_and_old_files() {
    for (version, marker) in [(Version::R2018, "AcDbSubDMesh"), (Version::R2000, "AcDbPolyFaceMesh")] {
        let bytes = write_with(&drawing(), "x.dxf", &SaveOptions { version: Some(version), binary: false }).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(marker) && text.contains("SURFACE3D"), "{version:?}");
        assert_eq!(back(&read_dxf(&bytes).unwrap()), surfaces(), "{version:?}");
    }
}

#[test]
fn other_programs_see_the_mesh_of_the_kept_part() {
    let bytes = write_with(&drawing(), "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap();
    let text = String::from_utf8_lossy(&bytes).replace("SURFACE3D", "SOMETHING");
    let d = read_dxf(text.as_bytes()).unwrap();
    assert!(back(&d).is_empty());
    let meshes: Vec<Mesh> = d.model.iter().filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None }).collect();
    assert_eq!(meshes.len(), surfaces().len());
    for (m, s) in meshes.iter().zip(surfaces()) {
        let area: f64 = m.triangles().iter().map(|[a, b, c]| (*b - *a).cross(*c - *a).len() / 2.0).sum();
        let want = s.area(s.size() / 4000.0);
        assert!((area - want).abs() <= 1e-6 * want.max(1.0), "mesh {area} vs {want}");
    }
}

#[test]
fn surfaces_survive_dwg_and_cdy() {
    for name in ["x.dwg", "x.cdy"] {
        let bytes = cadaddy_io::write(&drawing(), name).unwrap();
        let d = cadaddy_io::read(&bytes, name).unwrap();
        let kinds: Vec<&str> = d.model.iter().map(|e| e.kind.type_name()).collect();
        assert_eq!(back(&d), surfaces(), "{name}: {kinds:?}");
    }
}

#[test]
fn a_damaged_definition_falls_back_to_the_mesh() {
    let bytes = write_with(&drawing(), "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap();
    let text = String::from_utf8_lossy(&bytes).replace("\"knots_u\"", "\"knots_q\"");
    let d = read_dxf(text.as_bytes()).unwrap();
    assert!(back(&d).is_empty());
    assert_eq!(d.model.iter().filter(|e| matches!(e.kind, EntityKind::Mesh(_))).count(), surfaces().len());
}
