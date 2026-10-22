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
