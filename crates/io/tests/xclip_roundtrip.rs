//! XCLIP: a block reference's clip boundary goes into the file as AutoCAD's SPATIAL_FILTER and
//! comes back; a file from elsewhere with a SPATIAL_FILTER is read; DWG keeps it too.

use std::sync::Arc;

use cadaddy_doc::*;
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_io::{read, read_dxf, write, write_dxf};

fn drawing(clip: Option<XClip>) -> Drawing {
    let mut d = Drawing::new_imperial();
    let mut b = Block::new("GRID");
    b.entities.push(Entity {
        handle: Handle(0x500),
        common: Common::default(),
        kind: EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(100.0, 100.0, 0.0) }),
    });
    d.blocks.insert("GRID".into(), Arc::new(b));
    let ins = Insert {
        block: "GRID".into(),
        insert: Vec3::new(150.0, 0.0, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
    d
}

fn clip_of(d: &Drawing) -> Option<XClip> {
    d.model.iter().find_map(|e| if let EntityKind::Insert(i) = &e.kind { i.clip.clone() } else { None })
}

fn square() -> XClip {
    XClip {
        boundary: vec![Vec2::new(20.0, 20.0), Vec2::new(60.0, 20.0), Vec2::new(60.0, 50.0), Vec2::new(20.0, 50.0)],
        enabled: true,
        inverted: false,
        front: None,
        back: None,
    }
}

#[test]
fn clip_survives_dxf_and_dwg() {
    let d = drawing(Some(square()));
    let text = write_dxf(&d);
    assert!(text.contains("SPATIAL_FILTER") && text.contains("ACAD_FILTER"));
    assert_eq!(clip_of(&read_dxf(text.as_bytes()).unwrap()), Some(square()));
    let dwg = write(&d, "x.dwg").unwrap();
    assert_eq!(clip_of(&read(&dwg, "x.dwg").unwrap()), Some(square()));
    // Turned off, it stays off.
    let off = drawing(Some(XClip { enabled: false, ..square() }));
    assert_eq!(clip_of(&read_dxf(write_dxf(&off).as_bytes()).unwrap()).map(|c| c.enabled), Some(false));
    // No clip: no filter written.
    assert!(!write_dxf(&drawing(None)).contains("SPATIAL_FILTER"));
}

/// A two-point (rectangular) boundary written by another program, with the inverse of the
/// reference's transform (it sits at 150,0): read as the same block-coordinate rectangle.
#[test]
fn rectangular_filter_from_another_program() {
    let mut text = write_dxf(&drawing(Some(square()))).replace("\r\n", "\n");
    let start = text.find("AcDbSpatialFilter").unwrap();
    let end = text[start..].find("\n  0\n").or_else(|| text[start..].find("\n0\n")).unwrap() + start;
    let other = "AcDbSpatialFilter\n 70\n2\n 10\n170.0\n 20\n20.0\n 10\n210.0\n 20\n50.0\n210\n0.0\n220\n0.0\n230\n1.0\n 11\n0.0\n 21\n0.0\n 31\n0.0\n 71\n1\n 72\n0\n 73\n0\n 40\n1.0\n 40\n0.0\n 40\n0.0\n 40\n-150.0\n 40\n0.0\n 40\n1.0\n 40\n0.0\n 40\n0.0\n 40\n0.0\n 40\n0.0\n 40\n1.0\n 40\n0.0\n 40\n1.0\n 40\n0.0\n 40\n0.0\n 40\n0.0\n 40\n0.0\n 40\n1.0\n 40\n0.0\n 40\n0.0\n 40\n0.0\n 40\n0.0\n 40\n1.0\n 40\n0.0";
    text.replace_range(start..end, other);
    let c = clip_of(&read_dxf(text.as_bytes()).unwrap()).expect("clip read");
    let xs: Vec<f64> = c.boundary.iter().map(|p| p.x).collect();
    assert!(xs.iter().all(|x| (*x - 20.0).abs() < 1e-9 || (*x - 60.0).abs() < 1e-9), "{:?}", c.boundary);
    assert_eq!(c.boundary.len(), 4);
}

/// ZOOM Extents frames what a clipped reference shows, not the whole block.
#[test]
fn extents_follow_the_clip() {
    let d = drawing(Some(square()));
    let e = d.model.iter().next().unwrap();
    let b = entity_bounds(&d, e, 0);
    assert!((b.min.x - 170.0).abs() < 1e-9 && (b.max.x - 210.0).abs() < 1e-9, "{b:?}");
    assert!((b.min.y - 20.0).abs() < 1e-9 && (b.max.y - 50.0).abs() < 1e-9, "{b:?}");
    // Turned off, the whole block counts again.
    let d = drawing(Some(XClip { enabled: false, ..square() }));
    let b = entity_bounds(&d, d.model.iter().next().unwrap(), 0);
    assert!((b.max.x - 250.0).abs() < 1e-9, "{b:?}");
}

/// An inverted clip stays inverted through DXF and DWG.
#[test]
fn inverted_clip_survives_dxf_and_dwg() {
    let inv = XClip { inverted: true, ..square() };
    let d = drawing(Some(inv.clone()));
    assert_eq!(clip_of(&read_dxf(write_dxf(&d).as_bytes()).unwrap()), Some(inv.clone()), "dxf");
    let dwg = write(&d, "x.dwg").unwrap();
    assert_eq!(clip_of(&read(&dwg, "x.dwg").unwrap()), Some(inv), "dwg");
}
