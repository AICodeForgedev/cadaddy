//! DGN V7 (Q158): a drawing written by our exporter reads back the same — lines, polylines (with
//! arcs, past 101 vertices), circles, arcs, ellipses, texts, points, splines, a block reference
//! as a cell (and back as a block reference), layers as levels, colours, linetypes, lineweights,
//! units; a V8 file is refused with a clear message; hostile bytes never panic.

use cadaddy_doc::color::Color;
use cadaddy_doc::{Block, Common, Drawing, Entity, EntityKind, HAlign, Insert, Layer, Line, Lineweight, LwPolyline, Point, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};

fn v(x: f64, y: f64) -> PolyVertex {
    PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }
}

fn sample() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 2);
    for (name, color) in [("Walls", 1), ("Doors", 3)] {
        let mut l = Layer::new(name);
        l.color = Color::Index(color);
        d.layers.push(l);
    }
    d.linetypes.extend(cadaddy_doc::library::standard_linetypes().into_iter().filter(|l| l.name == "DASHED" || l.name == "CENTER"));
    let walls = Common { layer: "Walls".into(), ..Default::default() };
    let doors =
        Common { layer: "Doors".into(), color: Color::Index(5), linetype: "DASHED".into(), lineweight: Lineweight::Mm100(50), ..Default::default() };
    let add = |d: &mut Drawing, c: &Common, k: EntityKind| {
        d.add(&Space::Model, c.clone(), k).unwrap();
    };
    add(&mut d, &walls, EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(40.0, 0.0, 0.0) }));
    add(
        &mut d,
        &walls,
        EntityKind::LwPolyline(LwPolyline {
            vertices: vec![v(0.0, 0.0), v(40.0, 0.0), v(40.0, 30.0), v(0.0, 30.0)],
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        }),
    );
    // A slot: two lines and two half circles.
    let mut slot = vec![v(10.0, 10.0), v(20.0, 10.0), v(20.0, 14.0), v(10.0, 14.0)];
    slot[1].bulge = 1.0;
    slot[3].bulge = 1.0;
    add(&mut d, &doors, EntityKind::LwPolyline(LwPolyline { vertices: slot, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
    // 250 vertices: more than one line string holds.
    let wave: Vec<PolyVertex> = (0..250).map(|i| v(f64::from(i) * 0.1, (f64::from(i) * 0.2).sin())).collect();
    add(&mut d, &walls, EntityKind::LwPolyline(LwPolyline { vertices: wave, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }));
    add(&mut d, &doors, EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(5.0, 5.0, 0.0), radius: 2.5 }));
    add(&mut d, &doors, EntityKind::Arc(cadaddy_doc::Arc { center: Vec3::new(30.0, 20.0, 0.0), radius: 3.0, start: 0.25, end: 2.0 }));
    add(
        &mut d,
        &walls,
        EntityKind::Ellipse(cadaddy_doc::Ellipse {
            center: Vec3::new(20.0, 25.0, 0.0),
            major: Vec3::new(4.0, 2.0, 0.0),
            ratio: 0.5,
            start: 0.0,
            end: std::f64::consts::TAU,
        }),
    );
    add(&mut d, &walls, EntityKind::Point(Point { p: Vec3::new(1.0, 2.0, 0.0), angle: 0.0 }));
    add(
        &mut d,
        &walls,
        EntityKind::Text(Text {
            insert: Vec3::new(2.0, 3.0, 0.0),
            align_pt: None,
            height: 0.5,
            value: "ROOM 101".into(),
            rotation: 0.3,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        }),
    );
    add(
        &mut d,
        &walls,
        EntityKind::Text(Text {
            insert: Vec3::new(20.0, 15.0, 0.0),
            align_pt: Some(Vec3::new(20.0, 15.0, 0.0)),
            height: 1.0,
            value: "Lobby Café".into(),
            rotation: 0.0,
            width_factor: 0.8,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Center,
            valign: VAlign::Middle,
        }),
    );
    // A block of a square and a diagonal, placed turned and scaled.
    let mut blk = Block::new("TABLE1");
    for k in [
        EntityKind::LwPolyline(LwPolyline {
            vertices: vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0), v(0.0, 1.0)],
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        }),
        EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(2.0, 1.0, 0.0) }),
    ] {
        let h = d.new_handle();
        blk.entities.push(Entity { handle: h, common: Common { layer: "Doors".into(), ..Default::default() }, kind: k });
    }
    d.blocks.insert("TABLE1".into(), std::sync::Arc::new(blk));
    add(
        &mut d,
        &walls,
        EntityKind::Insert(Insert {
            block: "TABLE1".into(),
            insert: Vec3::new(30.0, 5.0, 0.0),
            scale: Vec3::new(2.0, 2.0, 1.0),
            rotation: 0.5,
            attribs: vec![],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        }),
    );
    d
}

fn kinds(d: &Drawing) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = d.model.iter().map(|e| e.kind.type_name()).collect();
    v.sort_unstable();
    v
}

#[test]
fn a_drawing_written_as_dgn_reads_back_the_same() {
    let d = sample();
    let (bytes, notes) = cadaddy_io::dgn::write(&d).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert!(cadaddy_io::dgn::is_v7(&bytes));
    let back = cadaddy_io::read(&bytes, "plan.dgn").unwrap();
    assert!(back.read_notes.is_empty(), "{:?}", back.read_notes);
    assert_eq!(kinds(&back), kinds(&d), "the same kinds of objects");
    assert_eq!(back.header.i64("INSUNITS", 0), 2, "feet");
    // Levels: the layers in order, from level 1.
    let level = |name: &str| format!("Level {}", d.layers.iter().position(|l| l.name == name).unwrap() + 1);
    let layers: Vec<&str> = back.model.iter().map(|e| e.common.layer.as_str()).collect();
    assert!(layers.contains(&level("Walls").as_str()) && layers.contains(&level("Doors").as_str()), "{layers:?}");
    // The door circle: colour 5, dashed, 0.5 mm, exact centre and radius.
    let circle = back.model.iter().find(|e| matches!(e.kind, EntityKind::Circle(_))).unwrap();
    let EntityKind::Circle(c) = &circle.kind else { unreachable!() };
    assert!((c.center - Vec3::new(5.0, 5.0, 0.0)).len() < 1e-6 && (c.radius - 2.5).abs() < 1e-6, "{c:?}");
    assert_eq!((circle.common.color, circle.common.linetype.as_str(), circle.common.lineweight), (Color::Index(5), "DASHED", Lineweight::Mm100(50)));
    // A wall line by layer: its colour came from the layer (red).
    let line = back.model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).unwrap();
    assert_eq!(line.common.color, Color::Index(1));
    // The arc keeps its angles; the ellipse its axes and turn.
    let EntityKind::Arc(a) = &back.model.iter().find(|e| matches!(e.kind, EntityKind::Arc(_))).unwrap().kind else { unreachable!() };
    assert!((a.start - 0.25).abs() < 1e-6 && (a.end - 2.0).abs() < 1e-6 && (a.radius - 3.0).abs() < 1e-6, "{a:?}");
    let EntityKind::Ellipse(el) = &back.model.iter().find(|e| matches!(e.kind, EntityKind::Ellipse(_))).unwrap().kind else { unreachable!() };
    assert!((el.major - Vec3::new(4.0, 2.0, 0.0)).len() < 1e-6 && (el.ratio - 0.5).abs() < 1e-9, "{el:?}");
    // The slot is one closed polyline with its two half circles as bulges.
    let slot = back.model.iter().find_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) if p.closed && p.vertices.iter().any(|v| v.bulge.abs() > 0.9) => Some(p.clone()),
        _ => None,
    });
    let slot = slot.expect("the slot");
    assert_eq!(slot.vertices.len(), 4, "{slot:?}");
    assert_eq!(slot.vertices.iter().filter(|v| (v.bulge.abs() - 1.0).abs() < 1e-6).count(), 2);
    // The 250-vertex wave: one polyline again, every vertex in place.
    let wave = back.model.iter().find_map(|e| match &e.kind {
        EntityKind::LwPolyline(p) if p.vertices.len() > 200 => Some(p.clone()),
        _ => None,
    });
    let wave = wave.expect("the wave");
    assert_eq!(wave.vertices.len(), 250);
    assert!((wave.vertices[123].p - Vec2::new(12.3, (24.6f64).sin())).len() < 1e-3, "{:?}", wave.vertices[123]);
    // Texts: value, height, rotation, justification, width.
    let texts: Vec<&Text> = back.model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t) } else { None }).collect();
    let room = texts.iter().find(|t| t.value == "ROOM 101").unwrap();
    assert!(
        (room.height - 0.5).abs() < 1e-3 && (room.rotation - 0.3).abs() < 1e-6 && (room.insert - Vec3::new(2.0, 3.0, 0.0)).len() < 1e-3,
        "{room:?}"
    );
    let lobby = texts.iter().find(|t| t.value == "Lobby Café").unwrap();
    assert_eq!((lobby.halign, lobby.valign), (HAlign::Center, VAlign::Middle));
    assert!((lobby.width_factor - 0.8).abs() < 1e-3);
    // The cell is a block reference again: turned 0.5, scaled 2, its objects in block units.
    let ins = back.model.iter().find_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.clone()) } else { None }).expect("a block reference");
    // The cell's 2×2 matrix is fixed-point (214748 a unit): about 5e-6 of the value.
    assert!((ins.rotation - 0.5).abs() < 1e-5 && (ins.scale.x - 2.0).abs() < 1e-5 && (ins.scale.y - 2.0).abs() < 1e-5, "{ins:?}");
    assert!((ins.insert - Vec3::new(30.0, 5.0, 0.0)).len() < 1e-6);
    assert!(ins.block.starts_with("TABLE1"), "{}", ins.block);
    let blk = back.block(&ins.block).unwrap();
    let ex = blk.entities.iter().find_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.clone()) } else { None }).unwrap();
    assert!((ex.b - Vec3::new(2.0, 1.0, 0.0)).len() < 1e-4, "{ex:?}");
}

#[test]
fn far_away_drawings_keep_their_coordinates() {
    let mut d = Drawing::new_imperial();
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Line(Line { a: Vec3::new(2_500_000.0, 7_100_000.0, 0.0), b: Vec3::new(2_500_100.0, 7_100_050.0, 0.0) }),
    )
    .unwrap();
    let back = cadaddy_io::read(&cadaddy_io::write(&d, "far.dgn").unwrap(), "far.dgn").unwrap();
    let EntityKind::Line(l) = &back.model.iter().next().unwrap().kind else { unreachable!() };
    assert!((l.a - Vec3::new(2_500_000.0, 7_100_000.0, 0.0)).len() < 1e-3 && (l.b - Vec3::new(2_500_100.0, 7_100_050.0, 0.0)).len() < 1e-3, "{l:?}");
}

#[test]
fn dgn_v8_is_refused_clearly_and_junk_never_panics() {
    let mut v8 = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    v8.extend(vec![0u8; 600]);
    let e = cadaddy_io::read(&v8, "site.dgn").unwrap_err().to_string();
    assert!(e.contains("DGN V8 isn't supported yet") && e.contains("V7"), "{e}");
    // Truncations and corruptions of a real file, and random bytes after a TCB signature.
    let good = cadaddy_io::dgn::write(&sample()).unwrap().0;
    for cut in [4, 37, 100, 1535, 1540, good.len() / 2, good.len() - 3] {
        let _ = cadaddy_io::read(&good[..cut], "x.dgn");
    }
    let mut state = 12345u64;
    for round in 0..200 {
        let mut b = good.clone();
        for _ in 0..(1 + round % 40) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let i = (state >> 33) as usize % b.len();
            b[i] = (state >> 17) as u8;
        }
        let _ = cadaddy_io::read(&b, "x.dgn");
    }
    let mut junk = vec![0x08, 0x09, 0xFE, 0x02];
    for i in 0..5000u32 {
        junk.push((i.wrapping_mul(2654435761) >> 13) as u8);
    }
    let _ = cadaddy_io::read(&junk, "x.dgn");
    assert!(cadaddy_io::read(b"not a dgn", "x.dgn").is_err());
}

// ---------------- hand-built files (Q5014) ----------------
//
// Bytes laid out per the published V7 element layouts (not through our writer): a 1536-byte TCB,
// then elements (header, display header, own bytes), then the 0xFFFF end word. 1000 UORs per
// master unit (metres).

/// A 32-bit integer as two little-endian words, the high word first.
fn put_i32(b: &mut [u8], i: usize, v: i32) {
    let u = v as u32;
    b[i..i + 4].copy_from_slice(&[(u >> 16) as u8, (u >> 24) as u8, u as u8, (u >> 8) as u8]);
}

fn put_u16(b: &mut [u8], i: usize, v: u16) {
    b[i..i + 2].copy_from_slice(&v.to_le_bytes());
}

/// Master units to UORs.
fn uor(v: f64) -> i32 {
    (v * 1000.0).round() as i32
}

fn tcb(dim3: bool) -> Vec<u8> {
    let mut t = vec![0u8; 1536];
    t[0] = if dim3 { 0xC8 } else { 0x08 };
    t[1] = 9;
    put_u16(&mut t, 2, 766);
    if dim3 {
        t[1214] |= 0x40;
    }
    put_i32(&mut t, 1112, 1);
    put_i32(&mut t, 1116, 1000);
    t[1120] = b'm';
    t[1121] = b' ';
    t
}

/// An element of type `ty`, `len` bytes, on level 1; `complex` marks a group member.
fn elem(ty: u8, len: usize, complex: bool) -> Vec<u8> {
    let len = len.max(36) + len % 2;
    let mut e = vec![0u8; len];
    e[0] = 1 | if complex { 0x80 } else { 0 };
    e[1] = ty;
    put_u16(&mut e, 2, (len / 2 - 2) as u16);
    put_u16(&mut e, 30, ((len - 32) / 2) as u16);
    e
}

/// A group header's `totlength`: the words after it up to the end of its last member.
fn set_totlength(header: &mut [u8], members: &[&Vec<u8>]) {
    let words = (header.len() - 38) / 2 + members.iter().map(|m| m.len() / 2).sum::<usize>();
    put_u16(header, 36, words as u16);
}

fn file(dim3: bool, elements: &[Vec<u8>]) -> Vec<u8> {
    let mut b = tcb(dim3);
    for e in elements {
        b.extend_from_slice(e);
    }
    b.extend_from_slice(&[0xff, 0xff]);
    b
}

/// Points as a line string / curve / pole / point string element (count at 36, points from 38).
fn point_list(ty: u8, pts: &[(f64, f64)], complex: bool) -> Vec<u8> {
    let mut e = elem(ty, 38 + pts.len() * 8, complex);
    put_u16(&mut e, 36, pts.len() as u16);
    for (k, (x, y)) in pts.iter().enumerate() {
        put_i32(&mut e, 38 + k * 8, uor(*x));
        put_i32(&mut e, 42 + k * 8, uor(*y));
    }
    e
}

/// A 2D line (type 3), its two ends at 36 and 44.
fn line2(a: (f64, f64), b: (f64, f64), complex: bool) -> Vec<u8> {
    let mut e = elem(3, 52, complex);
    for (k, v) in [a.0, a.1, b.0, b.1].into_iter().enumerate() {
        put_i32(&mut e, 36 + k * 4, uor(v));
    }
    e
}

fn rad50(s: &str) -> u16 {
    s.bytes().chain(std::iter::repeat(b' ')).take(3).fold(0u16, |w, c| {
        w * 40
            + match c {
                b'A'..=b'Z' => u16::from(c - b'A' + 1),
                b'0'..=b'9' => u16::from(c - b'0' + 30),
                _ => 0,
            }
    })
}

/// A 2D cell header (type 2) named `name`: the 2×2 matrix (×214748) at 68, the origin at 84.
fn cell2(name: &str, origin: (f64, f64), rot: f64, members: &[&Vec<u8>]) -> Vec<u8> {
    let mut e = elem(2, 92, false);
    put_u16(&mut e, 38, rad50(name.get(0..3).unwrap_or(name)));
    put_u16(&mut e, 40, rad50(name.get(3..).unwrap_or("")));
    let (s, c) = rot.sin_cos();
    for (k, v) in [c, -s, s, c].into_iter().enumerate() {
        put_i32(&mut e, 68 + k * 4, (v * 214_748.0).round() as i32);
    }
    put_i32(&mut e, 84, uor(origin.0));
    put_i32(&mut e, 88, uor(origin.1));
    set_totlength(&mut e, members);
    e
}

/// A cell of one line from (0,0) to `end` (cell units), placed at `origin` turned `rot`.
fn line_cell(name: &str, origin: (f64, f64), rot: f64, end: (f64, f64)) -> Vec<Vec<u8>> {
    let (s, c) = rot.sin_cos();
    let placed = (origin.0 + c * end.0 - s * end.1, origin.1 + s * end.0 + c * end.1);
    let member = line2(origin, placed, true);
    vec![cell2(name, origin, rot, &[&member]), member]
}

fn inserts(d: &Drawing) -> Vec<Insert> {
    d.model.iter().filter_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.clone()) } else { None }).collect()
}

#[test]
fn deeply_nested_text_nodes_are_capped_not_a_stack_overflow() {
    // 100,000 text-node headers, each a member of the one before (complex bit set, totlength
    // reaching as far as a header's word can: past thousands of the nodes after it).
    let mut els = Vec::new();
    for _ in 0..100_000 {
        let mut e = elem(7, 40, true);
        put_u16(&mut e, 36, u16::MAX);
        els.push(e);
    }
    let d = cadaddy_io::read(&file(false, &els), "nested.dgn").unwrap();
    assert!(d.read_notes.iter().any(|n| n.contains("text nodes nested too deep")), "{:?}", d.read_notes);
    assert_eq!(d.model.iter().count(), 0);
}

#[test]
fn copies_of_one_cell_are_one_block() {
    // 5,000 placements of one cell, turned four ways and spread far apart.
    let mut els = Vec::new();
    for k in 0..5000 {
        let at = (f64::from(k % 100) * 37.5, f64::from(k / 100) * 41.25);
        els.extend(line_cell("POLE", at, f64::from(k % 4) * 0.7, (1.5, 0.25)));
    }
    let started = std::time::Instant::now();
    let d = cadaddy_io::read(&file(false, &els), "site.dgn").unwrap();
    assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
    let refs = inserts(&d);
    assert_eq!(refs.len(), 5000);
    assert!(refs.iter().all(|i| i.block == "POLE"), "{:?}", refs.iter().map(|i| &i.block).collect::<std::collections::BTreeSet<_>>());
    assert_eq!(d.blocks.keys().filter(|k| k.starts_with("POLE")).count(), 1, "{:?}", d.blocks.keys().collect::<Vec<_>>());
    let EntityKind::Line(l) = &d.block("POLE").unwrap().entities.iter().next().unwrap().kind else { panic!("not a line") };
    assert!(l.a.len() < 1e-3 && (l.b - Vec3::new(1.5, 0.25, 0.0)).len() < 1e-3, "{l:?}");
}

#[test]
fn a_different_body_under_the_same_name_gets_its_own_block() {
    let mut els = line_cell("TREE", (0.0, 0.0), 0.0, (2.0, 0.0));
    els.extend(line_cell("TREE", (10.0, 0.0), 0.0, (0.0, 3.0)));
    els.extend(line_cell("TREE", (20.0, 0.0), 1.0, (2.0, 0.0)));
    let d = cadaddy_io::read(&file(false, &els), "trees.dgn").unwrap();
    let names: Vec<String> = inserts(&d).into_iter().map(|i| i.block).collect();
    assert_eq!(names, ["TREE", "TREE_2", "TREE"]);
    let end = |name: &str| match &d.block(name).unwrap().entities.iter().next().unwrap().kind {
        EntityKind::Line(l) => l.b,
        k => panic!("{k:?}"),
    };
    assert!((end("TREE") - Vec3::new(2.0, 0.0, 0.0)).len() < 1e-3 && (end("TREE_2") - Vec3::new(0.0, 3.0, 0.0)).len() < 1e-3);
}

#[test]
fn a_bspline_curve_reads_its_order_knots_poles_and_closed_flag() {
    // The header (type 27): order − 2 in the low nibble of byte 40, closed = 0x80; then a pole
    // element (type 21) and a knot element (type 26, interior knots × (2³¹ − 1)).
    let mut header = elem(27, 48, false);
    header[40] = (3 - 2) | 0x80;
    let poles = [(0.0, 0.0), (1.0, 2.0), (3.0, 3.0), (5.0, 1.0), (6.0, 0.0)];
    let pole = point_list(21, &poles, true);
    let mut knot = elem(26, 44, true);
    put_i32(&mut knot, 36, (0.3 * f64::from(i32::MAX)).round() as i32);
    put_i32(&mut knot, 40, (0.6 * f64::from(i32::MAX)).round() as i32);
    let d = cadaddy_io::read(&file(false, &[header, pole, knot]), "curve.dgn").unwrap();
    let s = d.model.iter().find_map(|e| if let EntityKind::Spline(s) = &e.kind { Some(s.clone()) } else { None }).expect("a spline");
    assert_eq!(s.degree, 2);
    assert!(s.closed);
    let want = [0.0, 0.0, 0.0, 0.3, 0.6, 1.0, 1.0, 1.0];
    assert_eq!(s.knots.len(), want.len(), "{:?}", s.knots);
    assert!(s.knots.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{:?}", s.knots);
    assert_eq!(s.control.len(), 5);
    assert!(s.control.iter().zip(poles).all(|(p, (x, y))| (*p - Vec2::new(x, y)).len() < 1e-9), "{:?}", s.control);
}

#[test]
fn a_curve_drops_its_tangent_points() {
    // Type 11: the first two and last two points only set the end tangents.
    let pts = [(-5.0, -5.0), (-1.0, -1.0), (0.0, 0.0), (2.0, 1.0), (4.0, 0.0), (5.0, 1.0), (9.0, 9.0)];
    let d = cadaddy_io::read(&file(false, &[point_list(11, &pts, false)]), "curve.dgn").unwrap();
    let p = d.model.iter().find_map(|e| if let EntityKind::LwPolyline(p) = &e.kind { Some(p.clone()) } else { None }).expect("a polyline");
    let got: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
    assert_eq!(got, [Vec2::new(0.0, 0.0), Vec2::new(2.0, 1.0), Vec2::new(4.0, 0.0)]);
    assert!(!p.closed);
}

#[test]
fn a_text_node_reads_both_its_texts() {
    // 2D text (type 17): justification 37, length and height multipliers 38 and 42 (× 6/1000 of
    // a UOR), rotation 46, origin 50, character count 58, characters from 60.
    let text = |value: &str, at: (f64, f64)| {
        let mut e = elem(17, 60 + value.len(), true);
        e[37] = 2;
        put_i32(&mut e, 38, 100_000);
        put_i32(&mut e, 42, 100_000);
        put_i32(&mut e, 50, uor(at.0));
        put_i32(&mut e, 54, uor(at.1));
        e[58] = value.len() as u8;
        e[60..60 + value.len()].copy_from_slice(value.as_bytes());
        e
    };
    let (a, b) = (text("NORTH", (2.0, 3.0)), text("WING", (2.0, 2.25)));
    let mut node = elem(7, 72, false);
    set_totlength(&mut node, &[&a, &b]);
    put_u16(&mut node, 38, 2);
    let d = cadaddy_io::read(&file(false, &[node, a, b]), "label.dgn").unwrap();
    let texts: Vec<Text> = d.model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).collect();
    assert_eq!(texts.iter().map(|t| t.value.as_str()).collect::<Vec<_>>(), ["NORTH", "WING"]);
    assert!((texts[0].insert - Vec3::new(2.0, 3.0, 0.0)).len() < 1e-9 && (texts[1].insert - Vec3::new(2.0, 2.25, 0.0)).len() < 1e-9);
    // 100,000 × 6/1000 of a UOR (1/1000 m): 0.6 m.
    assert!((texts[0].height - 0.6).abs() < 1e-9, "{}", texts[0].height);
}

#[test]
fn a_point_string_is_its_points() {
    let pts = [(1.0, 1.0), (2.5, 1.0), (4.0, -2.0)];
    let d = cadaddy_io::read(&file(false, &[point_list(22, &pts, false)]), "survey.dgn").unwrap();
    let got: Vec<Vec3> = d.model.iter().filter_map(|e| if let EntityKind::Point(p) = &e.kind { Some(p.p) } else { None }).collect();
    assert_eq!(got.len(), 3);
    assert!(got.iter().zip(pts).all(|(p, (x, y))| (*p - Vec3::new(x, y, 0.0)).len() < 1e-9), "{got:?}");
}

#[test]
fn a_3d_design_file_reads_a_line_and_a_turned_cell_in_plan() {
    // 3D: points are three integers (12 bytes); the cell's 3×3 matrix is at 76 (row by row) and
    // its origin at 112.
    let line3 = |a: [f64; 3], b: [f64; 3], complex: bool| {
        let mut e = elem(3, 60, complex);
        for (k, v) in a.into_iter().chain(b).enumerate() {
            put_i32(&mut e, 36 + k * 4, uor(v));
        }
        e
    };
    let line = line3([1.0, 2.0, 3.0], [4.0, 6.0, 3.0], false);
    let rot = 30f64.to_radians();
    let (s, c) = rot.sin_cos();
    let member = line3([10.0, 20.0, 0.0], [10.0 + 2.0 * c, 20.0 + 2.0 * s, 0.0], true);
    let mut cell = elem(2, 124, false);
    put_u16(&mut cell, 38, rad50("VAL"));
    put_u16(&mut cell, 40, rad50("VE"));
    for (k, v) in [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        put_i32(&mut cell, 76 + k * 4, (v * 214_748.0).round() as i32);
    }
    for (k, v) in [10.0, 20.0, 0.0].into_iter().enumerate() {
        put_i32(&mut cell, 112 + k * 4, uor(v));
    }
    set_totlength(&mut cell, &[&member]);
    let d = cadaddy_io::read(&file(true, &[line, cell, member]), "plant.dgn").unwrap();
    assert!(d.read_notes.iter().any(|n| n.contains("3D design file")), "{:?}", d.read_notes);
    let l = d.model.iter().find_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.clone()) } else { None }).expect("the line");
    assert!((l.a - Vec3::new(1.0, 2.0, 3.0)).len() < 1e-9 && (l.b - Vec3::new(4.0, 6.0, 3.0)).len() < 1e-9, "{l:?}");
    let refs = inserts(&d);
    assert_eq!(refs.len(), 1);
    let ins = &refs[0];
    assert_eq!(ins.block, "VALVE");
    assert!((ins.rotation - rot).abs() < 1e-5 && (ins.insert - Vec3::new(10.0, 20.0, 0.0)).len() < 1e-9, "{ins:?}");
    assert!((ins.scale.x - 1.0).abs() < 1e-5 && (ins.scale.y - 1.0).abs() < 1e-5, "{ins:?}");
    let EntityKind::Line(m) = &d.block("VALVE").unwrap().entities.iter().next().unwrap().kind else { panic!("not a line") };
    assert!(m.a.len() < 1e-4 && (m.b - Vec3::new(2.0, 0.0, 0.0)).len() < 1e-4, "{m:?}");
}
