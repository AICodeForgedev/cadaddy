//! EMF export (the Windows clipboard's enhanced metafile): structural checks of what is written.

use cadaddy_doc::{Common, Drawing, EntityKind, Line, Space};
use cadaddy_geom::Vec3;
use cadaddy_render::emf;

fn line_drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let red = Common { color: cadaddy_doc::color::Color::Index(1), ..Common::default() };
    d.add(&Space::Model, red, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(200.0, 100.0, 0.0) })).unwrap();
    d
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The records after the header: (type, parameter words), checking each size chains to the end.
fn records(b: &[u8]) -> Vec<(u32, Vec<u32>)> {
    let mut at = 0;
    let mut out = Vec::new();
    while at < b.len() {
        let (kind, size) = (u32_at(b, at), u32_at(b, at + 4) as usize);
        assert!(size >= 8 && size % 4 == 0 && at + size <= b.len(), "record {kind} size {size} at {at}");
        out.push((kind, (8..size).step_by(4).map(|i| u32_at(b, at + i)).collect()));
        at += size;
    }
    assert_eq!(at, b.len(), "records end exactly at the end of the file");
    out
}

#[test]
fn emf_has_its_header_bounds_and_end_record() {
    let b = emf::export(&line_drawing(), &Space::Model);
    // EMR_HEADER: type, size, then the " EMF" signature at byte 40.
    assert_eq!(u32_at(&b, 0), emf::EMR_HEADER);
    assert_eq!(u32_at(&b, 4), emf::HEADER_BYTES);
    assert_eq!(&b[40..44], b" EMF");
    assert_eq!(u32_at(&b, 40), emf::SIGNATURE);
    assert_eq!(u32_at(&b, 44), 0x0001_0000, "version");
    assert_eq!(u32_at(&b, 48) as usize, b.len(), "nBytes is the file size");
    let recs = records(&b[emf::HEADER_BYTES as usize..]);
    assert_eq!(u32_at(&b, 52) as usize, recs.len() + 1, "nRecords counts the header");
    // The device bounds keep the selection's 2:1 proportions (200 × 100 plus a 1% margin all round).
    let (bw, bh) = (f64::from(u32_at(&b, 16)) + 1.0, f64::from(u32_at(&b, 20)) + 1.0);
    assert!((bw / bh - 204.0 / 104.0).abs() < 0.01, "{bw} x {bh}");
    assert_eq!((u32_at(&b, 24), u32_at(&b, 28)), (0, 0), "frame starts at the origin");
    assert_eq!((f64::from(u32_at(&b, 32)), f64::from(u32_at(&b, 36))), (bw, bh), "frame is the picture in 0.01 mm");
    // EMR_EOF is last, and its trailing size matches its own record.
    let (kind, params) = recs.last().unwrap();
    assert_eq!(*kind, emf::EMR_EOF);
    assert_eq!(params.last(), Some(&20));
}

#[test]
fn emf_draws_a_copied_line_as_a_red_polyline_across_the_picture() {
    let b = emf::export(&line_drawing(), &Space::Model);
    let recs = records(&b[emf::HEADER_BYTES as usize..]);
    let lines: Vec<_> = recs.iter().filter(|(k, _)| *k == emf::EMR_POLYLINE).collect();
    assert_eq!(lines.len(), 1, "one polyline record for one line");
    let p = &lines[0].1;
    // Bounds (4 words), count, then two points: lower left to upper right, y down.
    assert_eq!(p[4], 2);
    let (x0, y0, x1, y1) = (p[5] as i32, p[6] as i32, p[7] as i32, p[8] as i32);
    assert!(x0 < x1 && y0 > y1, "({x0},{y0}) to ({x1},{y1})");
    assert_eq!((p[0] as i32, p[1] as i32, p[2] as i32, p[3] as i32), (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)));
    // The pen is red (COLORREF 0x0000FF) and was created and selected before the line.
    let pen = recs.iter().position(|(k, _)| *k == emf::EMR_CREATEPEN).unwrap();
    assert_eq!(recs[pen].1[4], 0x0000_00FF);
    assert!(pen < recs.iter().position(|(k, _)| *k == emf::EMR_POLYLINE).unwrap());
    // Every created object is deleted again.
    let made = recs.iter().filter(|(k, _)| matches!(*k, emf::EMR_CREATEPEN | emf::EMR_CREATEBRUSHINDIRECT)).count();
    assert_eq!(recs.iter().filter(|(k, _)| *k == emf::EMR_DELETEOBJECT).count(), made);
}

#[test]
fn an_empty_drawing_gives_a_valid_empty_metafile() {
    let b = emf::export(&Drawing::new_metric(), &Space::Model);
    assert_eq!(&b[40..44], b" EMF");
    let recs = records(&b[emf::HEADER_BYTES as usize..]);
    assert_eq!(recs.last().map(|r| r.0), Some(emf::EMR_EOF));
    assert!(recs.iter().all(|(k, _)| *k != emf::EMR_POLYLINE));
}
