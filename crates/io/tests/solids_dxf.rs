//! 3D solids in files (Q51): saved as the mesh of their surfaces, which other programs read
//! (MESH from 2010 on, a polyface mesh before), with CADaddy's definition in the mesh's xdata so
//! CADaddy reads the solid back exactly.

use cadaddy_doc::*;
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_io::{SaveOptions, Version, read_dxf, write_with};

fn solids() -> Vec<Solid3d> {
    let mut cyl = Solid3d::new(SolidShape::Frustum { base_radius: 2.0, top_radius: 2.0, height: 5.0 });
    cyl.transform(&placement(Vec3::new(10.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
    let sq = |a: f64, b: f64| {
        vec![PolyVertex::new(Vec2::new(a, a)), PolyVertex::new(Vec2::new(b, a)), PolyVertex::new(Vec2::new(b, b)), PolyVertex::new(Vec2::new(a, b))]
    };
    vec![
        Solid3d::new(SolidShape::Box { length: 3.0, width: 2.0, height: 1.0 }),
        cyl,
        Solid3d::new(SolidShape::Sphere { radius: 1.5 }),
        Solid3d::new(SolidShape::Torus { radius: 4.0, tube: 1.0 }),
        // A plate with a square hole: its caps need triangles.
        Solid3d::new(SolidShape::Extrusion { loops: vec![sq(0.0, 6.0), sq(2.0, 4.0)], height: 1.0, taper: 0.0 }),
    ]
}

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    for s in solids() {
        d.add(&Space::Model, Common::default(), EntityKind::Solid3d(Box::new(s))).unwrap();
    }
    d
}

fn back(d: &Drawing) -> Vec<Solid3d> {
    d.model.iter().filter_map(|e| if let EntityKind::Solid3d(s) = &e.kind { Some((**s).clone()) } else { None }).collect()
}

/// The volume a closed mesh encloses (signed tetrahedra from the origin).
fn mesh_volume(m: &Mesh) -> f64 {
    m.triangles().iter().map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0).sum()
}

#[test]
fn solids_round_trip_exactly_in_new_and_old_files() {
    for (version, marker) in [(Version::R2018, "AcDbSubDMesh"), (Version::R2000, "AcDbPolyFaceMesh")] {
        let bytes = write_with(&drawing(), "x.dxf", &SaveOptions { version: Some(version), binary: false }).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(marker) && text.contains("SOLID3D"), "{version:?}");
        assert_eq!(back(&read_dxf(&bytes).unwrap()), solids(), "{version:?}");
    }
}

#[test]
fn other_programs_see_a_closed_mesh_of_the_same_volume() {
    // Without our xdata (as a program that ignores it reads the file) each solid is a mesh
    // enclosing the solid within its tolerance (a 4000th of its size): within 1% of the volume.
    let bytes = write_with(&drawing(), "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap();
    let text = String::from_utf8_lossy(&bytes).replace("SOLID3D", "SOMETHING");
    let d = read_dxf(text.as_bytes()).unwrap();
    assert!(back(&d).is_empty());
    let meshes: Vec<Mesh> = d.model.iter().filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None }).collect();
    assert_eq!(meshes.len(), solids().len());
    for (m, s) in meshes.iter().zip(solids()) {
        let exact = s.props(1e-4).volume;
        let v = mesh_volume(m);
        assert!((v - exact).abs() <= 1e-2 * exact, "{}: mesh {v} vs {exact}", s.shape_name());
    }
}

#[test]
fn solids_survive_dwg() {
    let dwg = cadaddy_io::write(&drawing(), "x.dwg").unwrap();
    let d = cadaddy_io::read(&dwg, "x.dwg").unwrap();
    let kinds: Vec<&str> = d.model.iter().map(|e| e.kind.type_name()).collect();
    assert_eq!(back(&d), solids(), "{kinds:?}");
}

#[test]
fn a_damaged_definition_falls_back_to_the_mesh() {
    let bytes = write_with(&drawing(), "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap();
    // Break every definition's JSON: the meshes are still read.
    let text = String::from_utf8_lossy(&bytes).replace("\"type\"", "\"tyqe\"");
    let d = read_dxf(text.as_bytes()).unwrap();
    assert!(back(&d).is_empty());
    assert_eq!(d.model.iter().filter(|e| matches!(e.kind, EntityKind::Mesh(_))).count(), solids().len());
    // A definition that parses but isn't a solid is refused too.
    let text = String::from_utf8_lossy(&bytes).replace("\"radius\":1.5", "\"radius\":-1.5");
    let d = read_dxf(text.as_bytes()).unwrap();
    assert_eq!(back(&d).len(), solids().len() - 1);
}

#[test]
fn composites_round_trip_and_save_their_result_as_the_mesh() {
    let mut hole = Solid3d::new(SolidShape::Frustum { base_radius: 1.0, top_radius: 1.0, height: 4.0 });
    hole.transform(&placement(Vec3::new(1.5, 1.0, -1.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
    let holed = combine(Solid3d::new(SolidShape::Box { length: 3.0, width: 2.0, height: 1.0 }), hole, BoolOp::Subtract);
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Solid3d(Box::new(holed.clone()))).unwrap();
    for version in [Version::R2018, Version::R2000] {
        let bytes = write_with(&d, "x.dxf", &SaveOptions { version: Some(version), binary: false }).unwrap();
        assert_eq!(back(&read_dxf(&bytes).unwrap()), vec![holed.clone()], "{version:?}");
        // Other programs see the box with its hole.
        let text = String::from_utf8_lossy(&bytes).replace("SOLID3D", "SOMETHING");
        let m = read_dxf(text.as_bytes())
            .unwrap()
            .model
            .iter()
            .find_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
            .unwrap();
        let exact = 6.0 - std::f64::consts::PI;
        assert!((mesh_volume(&m) - exact).abs() < 1e-2 * exact, "{version:?}: {}", mesh_volume(&m));
    }
    let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
    assert_eq!(back(&cadaddy_io::read(&dwg, "x.dwg").unwrap()), vec![holed]);
}

#[test]
fn section_planes_round_trip_and_others_see_a_line() {
    let sp =
        SectionPlane { origin: Vec3::new(5.0, 2.0, 3.0), normal: Vec3::new(1.0, 0.0, 0.0), width: 12.0, height: 8.0, live: true, name: "A".into() };
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::SectionPlane(sp.clone())).unwrap();
    let bytes = write_with(&d, "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap();
    let back: Vec<SectionPlane> = read_dxf(&bytes)
        .unwrap()
        .model
        .iter()
        .filter_map(|e| if let EntityKind::SectionPlane(p) = &e.kind { Some(p.clone()) } else { None })
        .collect();
    assert_eq!(back, vec![sp]);
    let text = String::from_utf8_lossy(&bytes).replace("SECTIONPLANE", "SOMETHING");
    let d = read_dxf(text.as_bytes()).unwrap();
    assert!(d.model.iter().any(|e| matches!(e.kind, EntityKind::Line(_))));
}
