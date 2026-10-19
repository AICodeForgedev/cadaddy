//! Meshes in files (Q50): MESH (AcDbSubDMesh) read and written from AutoCAD 2010 on, a
//! polyface mesh before that; POLYLINE polyface and polygon meshes kept as what they are; DWG
//! through the bridge; hostile counts refused without panicking.

use cadaddy_doc::*;
use cadaddy_geom::Vec3;
use cadaddy_io::{SaveOptions, Version, read_dxf, write_dxf, write_with};

/// A pyramid: a square base and four triangles, one face of five vertices for good measure.
fn pyramid() -> Mesh {
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0), v(2.0, 2.0, 3.0), v(2.0, -1.0, 0.0)];
    let f = |a: &[u32]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f(&[0, 5, 1, 2, 3]), f(&[0, 1, 4]), f(&[1, 2, 4]), f(&[2, 3, 4]), f(&[3, 0, 4])];
    Mesh { vertices, faces, kind: MeshKind::Subd }
}

fn drawing_with(m: Mesh) -> Drawing {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Mesh(m)).unwrap();
    d
}

fn meshes(d: &Drawing) -> Vec<Mesh> {
    d.model.iter().filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None }).collect()
}

fn read(bytes: &[u8]) -> Drawing {
    read_dxf(bytes).unwrap()
}

#[test]
fn mesh_entities_round_trip_from_2010() {
    let d = drawing_with(pyramid());
    let bytes = write_with(&d, "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("AcDbSubDMesh"));
    let back = meshes(&read(&bytes));
    assert_eq!(back, vec![pyramid()]);
}

#[test]
fn older_files_get_a_polyface_with_the_same_surface() {
    let d = drawing_with(pyramid());
    let bytes = write_with(&d, "x.dxf", &SaveOptions { version: Some(Version::R2000), binary: false }).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("AcDbSubDMesh") && text.contains("AcDbPolyFaceMesh"));
    let back = meshes(&read(&bytes));
    assert_eq!(back.len(), 1);
    let m = &back[0];
    assert_eq!(m.kind, MeshKind::Polyface);
    assert_eq!(m.vertices, pyramid().vertices);
    // The five-vertex face became three triangles; its inner edges are hidden, so the visible
    // edges are the same.
    assert_eq!(m.faces.len(), 3 + 4);
    assert_eq!(m.edges(), pyramid().edges());
    assert_eq!(m.triangles().len(), pyramid().triangles().len());
}

#[test]
fn polygon_meshes_stay_polygon_meshes() {
    let v = |x: f64, y: f64| Vec3::new(x, y, x * y);
    let vertices = vec![v(0.0, 0.0), v(0.0, 1.0), v(0.0, 2.0), v(1.0, 0.0), v(1.0, 1.0), v(1.0, 2.0)];
    let faces = vec![MeshFace { v: vec![0, 1, 4, 3], hidden: 0 }, MeshFace { v: vec![1, 2, 5, 4], hidden: 0 }];
    let m = Mesh { vertices, faces, kind: MeshKind::Polygon { m: 2, n: 3, closed_m: false, closed_n: false } };
    let back = meshes(&read(write_dxf(&drawing_with(m.clone())).as_bytes()));
    assert_eq!(back, vec![m]);
}

#[test]
fn meshes_survive_dwg() {
    let d = drawing_with(pyramid());
    let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
    let back = cadaddy_io::read(&dwg, "x.dwg").unwrap();
    let ms = meshes(&back);
    assert_eq!(ms.len(), 1, "{:?}", back.model.iter().map(|e| e.kind.type_name()).collect::<Vec<_>>());
    assert_eq!(ms[0].vertices, pyramid().vertices);
    assert_eq!(ms[0].edges(), pyramid().edges());
}

#[test]
fn hostile_meshes_are_refused_quietly() {
    let tags = |pairs: &[(i32, &str)]| pairs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect::<String>();
    let wrap = |body: String| format!("0\nSECTION\n2\nENTITIES\n{body}0\nENDSEC\n0\nEOF\n");
    for body in [
        // Counts far beyond what follows.
        tags(&[
            (0, "MESH"),
            (8, "0"),
            (100, "AcDbSubDMesh"),
            (92, "999999999"),
            (10, "0"),
            (20, "0"),
            (30, "0"),
            (93, "99999999"),
            (90, "3"),
            (90, "0"),
            (90, "0"),
            (90, "0"),
        ]),
        // Negative counts, an index past the vertices, a face of 5000 vertices.
        tags(&[(0, "MESH"), (8, "0"), (100, "AcDbSubDMesh"), (92, "-4"), (93, "-1")]),
        tags(&[
            (0, "MESH"),
            (8, "0"),
            (100, "AcDbSubDMesh"),
            (92, "1"),
            (10, "0"),
            (20, "0"),
            (30, "0"),
            (93, "4"),
            (90, "3"),
            (90, "0"),
            (90, "7"),
            (90, "9"),
        ]),
        tags(&[(0, "MESH"), (8, "0"), (100, "AcDbSubDMesh"), (92, "1"), (10, "0"), (20, "0"), (30, "0"), (93, "2"), (90, "5000"), (90, "0")]),
        // A polyface whose faces name vertices it doesn't have.
        tags(&[
            (0, "POLYLINE"),
            (8, "0"),
            (66, "1"),
            (70, "64"),
            (0, "VERTEX"),
            (8, "0"),
            (10, "0"),
            (20, "0"),
            (30, "0"),
            (70, "192"),
            (0, "VERTEX"),
            (8, "0"),
            (70, "128"),
            (71, "1"),
            (72, "-9"),
            (73, "3"),
            (0, "SEQEND"),
        ]),
    ] {
        let d = read_dxf(wrap(body).as_bytes()).unwrap();
        assert!(meshes(&d).is_empty());
    }
}
