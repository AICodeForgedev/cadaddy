use std::collections::BTreeSet;

use cadaddy_doc::foreign::Foreign;
use cadaddy_doc::*;
use cadaddy_geom::{Vec2, Vec3};

use super::container::{self, CHUNK_HEADER_LEN, HEADER_LEN, Writer};
use super::*;

/// DXF text from lines of `code value`.
fn dxf(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (code, value) = line.split_once(' ').unwrap_or((line, ""));
        out.push_str(&format!("{code}\n{value}\n"));
    }
    out
}

fn first<T>(d: &Drawing, f: impl Fn(&EntityKind) -> Option<T>) -> T {
    d.model.iter().find_map(|e| f(&e.kind)).unwrap()
}

/// A synthetic drawing with an object of every kind (in model space, a block and a layout),
/// another program's object, tables and header variables: a DXF written for the purpose, plus
/// the kinds easier to build in code.
fn every_kind() -> Drawing {
    let src = dxf(include_str!("every_kind.dxf.txt"));
    let mut d = crate::read_dxf(src.as_bytes()).unwrap();
    d.read_notes.clear();
    let mtext = first(&d, |k| if let EntityKind::MText(m) = k { Some(m.clone()) } else { None });
    let loops = first(&d, |k| if let EntityKind::Hatch(h) = k { Some(h.loops.clone()) } else { None });
    let mleader = MLeader {
        leaders: vec![vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)]],
        landing: Vec3::new(10.0, 5.0, 0.0),
        dogleg: 2.0,
        text: Some(mtext),
        style: "Standard".into(),
        arrow_size: 2.5,
        spline: false,
        block: None,
        roots: Vec::new(),
        leader_color: Some(cadaddy_color::Color::Index(1)),
        leader_linetype: Some("DASHED".into()),
        arrow: Some("_DOT".into()),
        text_frame: true,
    };
    let cell = |t: &str| TableCell { text: t.into(), ..Default::default() };
    let mut rows = vec![vec![cell("Schedule"), cell("")], vec![cell("A"), cell("B")]];
    if let Some(c) = rows.get_mut(0).and_then(|r| r.get_mut(0)) {
        c.merged = Some((1, 2));
    }
    let table = Table {
        insert: Vec3::new(5.0, 20.0, 0.0),
        col_widths: vec![1.5, 2.5],
        row_heights: vec![0.5, 0.4],
        cells: rows,
        text_height: 0.2,
        title: true,
        ..Table::default()
    };
    let image: Image = serde_json::from_value(serde_json::json!({
        "insert": {"x": 1.0, "y": 2.0, "z": 0.0},
        "u": {"x": 0.01, "y": 0.0, "z": 0.0},
        "v": {"x": 0.0, "y": 0.01, "z": 0.0},
        "size": {"x": 640.0, "y": 480.0},
        "path": "photo.png",
        "clip": [{"x": 0.0, "y": 0.0}, {"x": 100.0, "y": 100.0}],
        "clipping": true,
    }))
    .unwrap();
    for kind in [
        EntityKind::MLeader(mleader),
        EntityKind::Table(table),
        EntityKind::Region(Region { loops }),
        EntityKind::Image(image),
        EntityKind::AttDef(Attrib {
            tag: "ROOM".into(),
            prompt: "Room?".into(),
            ..first(&d, |k| if let EntityKind::Insert(i) = k { i.attribs.first().cloned() } else { None })
        }),
    ] {
        d.add(&Space::Model, Common { layer: "WALLS".into(), ..Common::default() }, kind).unwrap();
    }
    let paper = Space::Paper(d.layouts.first().map(|l| l.name.clone()).unwrap());
    let vp = Viewport {
        center: Vec3::new(5.0, 4.0, 0.0),
        width: 8.0,
        height: 6.0,
        view_center: Vec2::new(5.0, 2.5),
        view_height: 12.0,
        id: 2,
        locked: true,
        frozen_layers: vec!["WALLS".into()],
        anno_scale: Some("1:50".into()),
        layer_colors: Vec::new(),
        clip: None,
        on: true,
        twist: 0.25,
    };
    d.add(&paper, Common::default(), EntityKind::Viewport(vp)).unwrap();
    d.add(&paper, Common::default(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 1.0, 0.0) })).unwrap();
    d
}

/// `d` as read back from its .cdy bytes, with the reading notes.
fn cycle(d: &Drawing) -> (Drawing, Vec<String>) {
    let bytes = model::write(d, None).unwrap();
    let (back, rep) = model::read(&bytes).unwrap();
    (back, rep.notes)
}

#[test]
fn every_kind_and_table_roundtrips_exactly() {
    let d = every_kind();
    let mut kinds: BTreeSet<&str> = d.model.iter().map(|e| e.kind.type_name()).collect();
    for b in d.blocks.values() {
        kinds.extend(b.entities.iter().map(|e| e.kind.type_name()));
    }
    for l in &d.layouts {
        kinds.extend(l.entities.iter().map(|e| e.kind.type_name()));
    }
    assert_eq!(kinds.len(), 30, "{kinds:?}");
    assert!(!d.foreign.is_empty(), "the fixture keeps another program's object");
    assert!(d.layers.iter().any(|l| l.name == "WALLS"));
    let (back, notes) = cycle(&d);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(back, d);
    for d in [Drawing::new_imperial(), Drawing::new_metric()] {
        assert_eq!(cycle(&d).0, d);
    }
}

#[test]
fn the_public_api_and_io_dispatch_read_what_they_write() {
    let d = every_kind();
    let bytes = crate::write(&d, "plan.cdy").unwrap();
    assert!(is_cdy(&bytes));
    let (back, rep) = read_cdy_report(&bytes).unwrap();
    assert_eq!(back, d);
    let png = rep.thumbnail.unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    let m = rep.manifest.unwrap();
    assert_eq!(m["format"], 1);
    assert_eq!(m["thumbnail"], "THMB");
    assert_eq!(m["insunits"], 4);
    assert_eq!(manifest(&bytes), Some(m));
    assert_eq!(manifest(b"not a drawing"), None);
    assert_eq!(thumbnail(&bytes).as_deref(), Some(&png[..]));
    assert_eq!(thumbnail(&model::write(&d, None).unwrap()), None, "written without one");
    // With the index gone the preview is still found, by walking.
    assert_eq!(thumbnail(&bytes[..bytes.len() - 2]).as_deref(), Some(&png[..]));
    assert_eq!(thumbnail(b"CADDY\x1a\r\n"), None);
    // Whatever the name, the content says it is a .cdy file.
    assert_eq!(crate::read(&bytes, "renamed.dxf").unwrap(), d);
    assert!(crate::write_with(&d, "a.cdy", &crate::SaveOptions { binary: true, ..Default::default() }).is_err());
}

#[test]
fn many_objects_span_chunks_in_order_and_output_is_deterministic() {
    let mut d = Drawing::new_metric();
    for i in 0..(ENTITY_CHUNK * 2 + 7) {
        let x = i as f64;
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x, 1.0, 0.0) })).unwrap();
    }
    let a = model::write(&d, None).unwrap();
    assert_eq!(a, model::write(&d.clone(), None).unwrap(), "same drawing, same bytes");
    let (list, _) = container::chunks(&a).unwrap();
    let ents = list.iter().filter(|r| matches!(r, container::Read::Ok(c) if &c.tag == b"ENTS")).count();
    assert_eq!(ents, 3);
    assert_eq!(cycle(&d).0, d);
}

/// The byte offset of the first chunk tagged `tag`.
fn offset_of(bytes: &[u8], tag: &[u8; 4]) -> usize {
    let mut at = HEADER_LEN;
    loop {
        if bytes.get(at..at + 4) == Some(&tag[..]) {
            return at;
        }
        let len = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize;
        at += CHUNK_HEADER_LEN + len;
    }
}

#[test]
fn a_damaged_part_is_left_out_and_the_rest_opens() {
    let d = every_kind();
    let mut bytes = model::write(&d, None).unwrap();
    let at = offset_of(&bytes, b"ENTS");
    bytes[at + CHUNK_HEADER_LEN + 3] ^= 0x55;
    let (back, rep) = model::read(&bytes).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("ENTS")), "{:?}", rep.notes);
    // Damage at its very start: nothing before it to recover.
    assert!(back.model.is_empty());
    assert_eq!(back.layers, d.layers);
    assert_eq!(back.blocks, d.blocks);
    assert_eq!(back.layouts, d.layouts);
    // Through the public reader the notes reach the drawing.
    assert!(read_cdy(&bytes).unwrap().read_notes.iter().any(|n| n.contains("left out")));
    // With the index gone too, the parts are found by walking.
    let n = bytes.len();
    bytes.truncate(n - 1);
    let (back, rep) = model::read(&bytes).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("index")));
    assert_eq!(back.layers, d.layers);
}

#[test]
fn objects_before_damage_in_a_part_are_recovered_exactly() {
    let d = every_kind();
    let bytes = model::write(&d, None).unwrap();
    let at = offset_of(&bytes, b"ENTS");
    let len = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize;
    let originals: Vec<_> = d.model.iter().map(|e| e.kind.clone()).collect();
    let mut best = 0;
    // Damage at several places in the part: what comes back is the objects before it, unchanged.
    for frac in [0.1, 0.3, 0.5, 0.7, 0.9, 0.98] {
        let mut b = bytes.clone();
        let spot = at + CHUNK_HEADER_LEN + ((len as f64 * frac) as usize).min(len - 1);
        for x in &mut b[spot..(spot + 4).min(at + CHUNK_HEADER_LEN + len)] {
            *x ^= 0x5a;
        }
        let (back, rep) = model::read(&b).unwrap();
        // Damage in the part's key table (its start) leaves nothing to salvage.
        assert!(rep.notes.iter().any(|n| n.contains("model space part was damaged") || n.contains("ENTS part was left out")), "{:?}", rep.notes);
        let got: Vec<_> = back.model.iter().map(|e| e.kind.clone()).collect();
        assert!(got.len() < originals.len());
        assert_eq!(got[..], originals[..got.len()], "a recovered object changed (damage at {frac})");
        assert_eq!(back.layers, d.layers);
        best = best.max(got.len());
    }
    // Late damage keeps most of them (it lost every object before).
    assert!(best * 2 > originals.len(), "only {best} of {} recovered", originals.len());
}

#[test]
fn unknown_parts_are_skipped_and_newer_known_parts_refused() {
    let d = every_kind();
    // A file with a part a later version might add.
    let mut w = Writer::new();
    w.chunk(*b"ZZZZ", 7, b"from the future", true);
    let tail = model::write(&d, None).unwrap();
    let mut with_future = Vec::new();
    let mut head = w.finish();
    head.truncate(head.len() - container::TRAILER_LEN);
    // Keep only the header and the ZZZZ chunk, then the drawing's chunks (walked: no index).
    let zzzz_end = offset_of(&head, b"INDX");
    with_future.extend_from_slice(&head[..zzzz_end]);
    with_future.extend_from_slice(&tail[HEADER_LEN..offset_of(&tail, b"INDX")]);
    let (back, rep) = model::read(&with_future).unwrap();
    assert_eq!(back, d, "{:?}", rep.notes);
    // A known part at a newer schema version.
    let mut newer = model::write(&d, None).unwrap();
    let at = offset_of(&newer, b"ENTS");
    newer[at + 4] = 2;
    let err = model::read(&newer).unwrap_err();
    assert!(err.contains("newer CADaddy"), "{err}");
}

#[test]
fn hostile_files_never_panic() {
    let d = every_kind();
    let bytes = model::write(&d, None).unwrap();
    // Every prefix.
    for n in (0..bytes.len()).step_by(7) {
        let _ = model::read(&bytes[..n]);
    }
    // Byte flips across the file (lengths, counts, CRCs, payloads).
    let mut seed = 0x9E37_79B9u32;
    for _ in 0..3000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let mut b = bytes.clone();
        let i = seed as usize % b.len();
        b[i] = b[i].wrapping_add((seed >> 24) as u8 | 1);
        let _ = model::read(&b);
    }
    // Valid chunks with hostile payloads: deep nesting, huge counts, wrong shapes.
    let deep: Vec<tokens::Tok> = std::iter::repeat_n(tokens::Tok::List(1), 100_000).chain([tokens::Tok::Null]).collect();
    let keys: Vec<String> = Vec::new();
    for payload in [
        postcard::to_allocvec(&(&keys, &deep)).unwrap(),
        postcard::to_allocvec(&(&keys, vec![tokens::Tok::List(u32::MAX)])).unwrap(),
        postcard::to_allocvec(&(&keys, vec![tokens::Tok::Record(1), tokens::Tok::Key(7), tokens::Tok::Null])).unwrap(),
        tokens::encode(&serde_json::json!({"layers": 5})),
        tokens::encode(&serde_json::json!([1, 2, 3])),
        vec![0xFF; 64],
    ] {
        for tag in [*b"HEAD", *b"TABL", *b"BLKS", *b"ENTS", *b"LAYO", *b"FRGN", *b"MANI"] {
            let mut w = Writer::new();
            w.chunk(tag, 1, &payload, true);
            let (_, rep) = model::read(&w.finish()).unwrap();
            assert!(tag == *b"MANI" || !rep.notes.is_empty());
        }
    }
}

#[test]
fn kept_file_data_roundtrips_through_its_mirror() {
    let d = every_kind();
    let f: &Foreign = &d.foreign;
    assert!(!f.objects.is_empty() || !f.entities.is_empty() || !f.classes.is_empty());
    assert_eq!(&model::foreign_from(model::foreign_v1(f)), f);
    let (cycled, _) = cycle(&d);
    assert!(cycled.foreign == d.foreign);
}

/// A file written by the first .cdy writer, kept so later versions prove they still read it.
const V1_SAMPLE: &[u8] = include_bytes!("v1_sample.cdy");

fn v1_drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    d.layers.push(Layer { name: "Notes".into(), ..Layer::default() });
    let c = Common { layer: "Notes".into(), ..Common::default() };
    d.add(&Space::Model, c.clone(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(100.0, 50.0, 0.0) })).unwrap();
    d.add(&Space::Model, c.clone(), EntityKind::Circle(Circle { center: Vec3::new(30.0, 20.0, 0.0), radius: 12.5 })).unwrap();
    d.add(&Space::Model, c, EntityKind::Arc(cadaddy_doc::Arc { center: Vec3::ZERO, radius: 10.0, start: 0.0, end: 1.5 })).unwrap();
    d
}

#[test]
fn the_frozen_v1_sample_still_reads() {
    let (d, rep) = model::read(V1_SAMPLE).unwrap();
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    let want = v1_drawing();
    assert_eq!(d.layers, want.layers);
    let got: Vec<&EntityKind> = d.model.iter().map(|e| &e.kind).collect();
    let exp: Vec<&EntityKind> = want.model.iter().map(|e| &e.kind).collect();
    assert_eq!(got, exp);
    assert_eq!(rep.manifest.unwrap()["format"], 1);
}

/// Writes the frozen sample (run once, by hand, when the v1 writer was finished).
#[test]
#[ignore]
fn write_v1_sample() {
    let bytes = model::write(&v1_drawing(), None).unwrap();
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cdy/v1_sample.cdy"), bytes).unwrap();
}

/// Sizes and times next to DXF and DWG (printed with `--nocapture`); .cdy must be the smallest.
#[test]
fn size_and_speed_against_dxf_and_dwg() {
    let mut d = every_kind();
    for i in 0..20_000 {
        let x = f64::from(i) * 0.37;
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(x, x.sin(), 0.0), b: Vec3::new(x + 1.0, x.cos(), 0.0) }))
            .unwrap();
    }
    let t = std::time::Instant::now();
    let cdy = model::write(&d, None).unwrap();
    let cdy_w = t.elapsed();
    let t = std::time::Instant::now();
    let _ = model::read(&cdy).unwrap();
    let cdy_r = t.elapsed();
    let t = std::time::Instant::now();
    let dxf = crate::write(&d, "x.dxf").unwrap();
    let dxf_w = t.elapsed();
    let t = std::time::Instant::now();
    let _ = crate::read(&dxf, "x.dxf").unwrap();
    let dxf_r = t.elapsed();
    let dwg = crate::write(&d, "x.dwg").map(|b| b.len()).unwrap_or(0);
    println!("cdy {} B (write {cdy_w:?}, read {cdy_r:?}); dxf {} B (write {dxf_w:?}, read {dxf_r:?}); dwg {dwg} B", cdy.len(), dxf.len());
    assert!(cdy.len() < dxf.len());
}

/// Generated drawings (seeded, so failures repeat): random kinds, coordinates across magnitudes,
/// text with any characters, layers, blocks and inserts; each must come back exactly.
#[test]
fn generated_drawings_roundtrip_exactly() {
    let seed = std::cell::Cell::new(0x2545_F491_4F6C_DD1Du64);
    let next = || {
        let mut s = seed.get();
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        seed.set(s);
        s
    };
    for round in 0..40 {
        let mut d = if round % 2 == 0 { Drawing::new_imperial() } else { Drawing::new_metric() };
        let f = || {
            let r = next();
            let mag = 10f64.powi((r % 13) as i32 - 6);
            (((r >> 8) as f64 / (1u64 << 56) as f64) - 0.5) * mag
        };
        for l in 0..(round % 5) {
            d.layers.push(Layer { name: format!("L{l}-ü"), ..Layer::default() });
        }
        let mut block = Block::new("B");
        for _ in 0..(round % 4) {
            block.entities.push(Entity::new(Handle(0), EntityKind::Line(Line { a: Vec3::new(f(), f(), 0.0), b: Vec3::new(f(), f(), f()) })));
        }
        d.blocks.insert("B".into(), std::sync::Arc::new(block));
        for i in 0..(round * 7) {
            let kind = match next() % 6 {
                0 => EntityKind::Line(Line { a: Vec3::new(f(), f(), f()), b: Vec3::new(f(), f(), f()) }),
                1 => EntityKind::Circle(Circle { center: Vec3::new(f(), f(), 0.0), radius: f().abs() + 1e-6 }),
                2 => EntityKind::Arc(cadaddy_doc::Arc { center: Vec3::new(f(), f(), 0.0), radius: f().abs() + 1e-6, start: f(), end: f() }),
                3 => EntityKind::Point(Point { p: Vec3::new(f(), f(), f()), angle: f() }),
                4 => EntityKind::Insert(Insert { block: "B".into(), insert: Vec3::new(f(), f(), 0.0), ..first_insert() }),
                _ => {
                    let mut t = first(&every_kind(), |k| if let EntityKind::Text(t) = k { Some(t.clone()) } else { None });
                    t.value = (0..(next() % 12)).filter_map(|_| char::from_u32((next() % 0x2FFF) as u32)).collect();
                    t.rotation = f();
                    EntityKind::Text(t)
                }
            };
            let layer = format!("L{}-ü", i % 3);
            let layer = if d.layers.iter().any(|l| l.name == layer) { layer } else { "0".into() };
            d.add(&Space::Model, Common { layer, ..Common::default() }, kind).unwrap();
        }
        let (back, notes) = cycle(&d);
        assert!(notes.is_empty(), "round {round}: {notes:?}");
        assert_eq!(back, d, "round {round}");
    }
}

fn first_insert() -> Insert {
    first(&every_kind(), |k| if let EntityKind::Insert(i) = k { Some(i.clone()) } else { None })
}

// ---- Incremental saves (Q99) ----

/// The file after storing `out` over `file`.
fn stored(file: &[u8], out: &Output) -> Vec<u8> {
    match out {
        Output::Full(b) => b.clone(),
        Output::Append { from, bytes, .. } => {
            assert_eq!(*from as usize, file.len(), "appended to the file as it is");
            [file, bytes.as_slice()].concat()
        }
    }
}

fn lines(n: usize) -> Drawing {
    let mut d = Drawing::new_metric();
    for i in 0..n {
        let x = i as f64;
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x, 1.0, 0.0) })).unwrap();
    }
    d
}

fn no_thumb() -> Option<Vec<u8>> {
    None
}

#[test]
fn an_edit_appends_only_its_chunk_and_reads_back_exactly() {
    let mut d = lines(5000);
    let (out, state) = save(&d, None, &no_thumb).unwrap();
    assert!(matches!(out, Output::Full(_)));
    let file = stored(&[], &out);
    // Nothing changed: only a new index and trailer (and nothing else) are appended.
    let (out, state) = save(&d, Some(&state), &no_thumb).unwrap();
    let Output::Append { bytes, .. } = &out else { panic!("expected an append") };
    assert!(bytes.len() < 400, "{} bytes appended for no change", bytes.len());
    let file = stored(&file, &out);
    // One object edited: its chunk (of 1024) is appended.
    let h = d.model.iter().nth(2500).map(|e| e.handle).unwrap();
    d.model.modify(h, |e| e.common.layer = "Edited".into());
    let (out, state) = save(&d, Some(&state), &no_thumb).unwrap();
    let Output::Append { bytes, index_at, .. } = &out else { panic!("expected an append") };
    assert!(*index_at > 0 && bytes.len() < file.len() / 3, "{} of {}", bytes.len(), file.len());
    let file = stored(&file, &out);
    let (back, rep) = model::read(&file).unwrap();
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!(back, d);
    assert_eq!(state.len as usize, file.len());
}

#[test]
fn a_file_read_back_saves_incrementally() {
    let d = every_kind();
    let (out, _) = save(&d, None, &|| Some(b"\x89PNG fake".to_vec())).unwrap();
    let file = stored(&[], &out);
    let (mut opened, _) = model::read(&file).unwrap();
    let state = seed(&opened, &file).expect("a file this writer wrote matches the drawing read from it");
    opened.layers.push(Layer { name: "Added".into(), ..Layer::default() });
    let h = opened.model.iter().next().map(|e| e.handle).unwrap();
    opened.model.modify(h, |e| e.common.layer = "Added".into());
    let (out, _) = save(&opened, Some(&state), &no_thumb).unwrap();
    let Output::Append { bytes, .. } = &out else { panic!("expected an append") };
    assert!(bytes.len() < file.len());
    let file2 = stored(&file, &out);
    let (back, rep) = read_cdy_report(&file2).unwrap();
    assert_eq!(back, opened);
    assert_eq!(rep.thumbnail.as_deref(), Some(&b"\x89PNG fake"[..]), "an appended save keeps the preview");
    // A file a different writer laid out (or damaged) isn't matched: the next save is full.
    assert!(seed(&opened, &file[..file.len() - 1]).is_none());
    assert!(seed(&lines(3), &file).is_none());
}

#[test]
fn waste_past_the_limit_compacts_the_file() {
    let mut d = lines(3000);
    let (out, mut state) = save(&d, None, &no_thumb).unwrap();
    let mut file = stored(&[], &out);
    let h = d.model.iter().next().map(|e| e.handle).unwrap();
    let mut appends = 0;
    for i in 0..20 {
        d.model.modify(h, |e| e.common.layer = format!("L{i}"));
        let (out, s) = save(&d, Some(&state), &no_thumb).unwrap();
        state = s;
        match &out {
            Output::Append { .. } => appends += 1,
            Output::Full(_) => {
                assert!(appends > 0, "appends first");
                file = stored(&file, &out);
                let (back, _) = model::read(&file).unwrap();
                assert_eq!(back, d);
                return;
            }
        }
        file = stored(&file, &out);
        let waste = file.len() as f64 - state.len as f64;
        assert!(waste.abs() < 1.0);
    }
    panic!("never compacted after {appends} appends");
}

#[test]
fn a_save_cut_short_anywhere_leaves_the_previous_save() {
    let mut d = lines(1500);
    let (out, state) = save(&d, None, &no_thumb).unwrap();
    let old = stored(&[], &out);
    let before = d.clone();
    let h = d.model.iter().last().map(|e| e.handle).unwrap();
    d.model.modify(h, |e| e.common.layer = "New".into());
    d.layers.push(Layer { name: "New".into(), ..Layer::default() });
    let (out, _) = save(&d, Some(&state), &no_thumb).unwrap();
    let Output::Append { index_at, .. } = &out else { panic!("expected an append") };
    let new = stored(&old, &out);
    // Until the new index is complete the file reads as the previous save; once it is (only the
    // trailer short), as the new one.
    let index_end = new.len() - container::TRAILER_LEN;
    for cut in old.len()..new.len() {
        let (back, _) = model::read(&new[..cut]).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
        let want = if cut < index_end { &before } else { &d };
        assert_eq!(&back, want, "cut at {cut} of {} (index from {})", new.len(), old.len() + index_at);
    }
    assert_eq!(model::read(&new).unwrap().0, d);
}

#[test]
fn parts_from_a_newer_version_survive_an_incremental_save() {
    let d = lines(10);
    let (out, _) = save(&d, None, &no_thumb).unwrap();
    let Output::Full(file) = out else { panic!() };
    let (opened, _) = model::read(&file).unwrap();
    // The file as a newer CADaddy might leave it: one more chunk, of a type this one doesn't know.
    let combined = {
        let mut w = Writer::append(file.len() as u64);
        w.chunk(*b"ZZZZ", 3, b"future data", false);
        for e in container::index(&file).unwrap() {
            w.reuse(&e);
        }
        [file.clone(), w.finish()].concat()
    };
    let state = seed(&opened, &combined).expect("a part this version doesn't know doesn't stop the match");
    let mut edited = opened.clone();
    edited.layers.push(Layer { name: "X".into(), ..Layer::default() });
    let (out, _) = save(&edited, Some(&state), &no_thumb).unwrap();
    let after = stored(&combined, &out);
    let tags: Vec<[u8; 4]> = container::index(&after).unwrap().iter().map(|e| e.tag).collect();
    assert!(tags.contains(b"ZZZZ"), "{tags:?}");
    assert_eq!(model::read(&after).unwrap().0, edited);
}

/// 190k objects: the full save, then a save after one edit (printed with `--nocapture`).
#[test]
fn saving_a_large_drawing_after_a_small_edit_is_fast() {
    let mut d = lines(190_000);
    let t = std::time::Instant::now();
    let (out, state) = save(&d, None, &no_thumb).unwrap();
    let full = t.elapsed();
    let file = stored(&[], &out);
    let h = d.model.iter().nth(100_000).map(|e| e.handle).unwrap();
    d.model.modify(h, |e| e.common.layer = "Edited".into());
    let t = std::time::Instant::now();
    let (out, _) = save(&d, Some(&state), &no_thumb).unwrap();
    let small = t.elapsed();
    let Output::Append { bytes, .. } = &out else { panic!("expected an append") };
    println!("190k objects: full save {full:?} ({} B); after one edit {small:?} ({} B appended)", file.len(), bytes.len());
    assert!(small * 10 < full, "{small:?} vs {full:?}");
    assert_eq!(model::read(&stored(&file, &out)).unwrap().0, d);
}

/// Damage to a chunk's own header (its tag and length) still finds it through the index and
/// salvages it; the note names it, not the damaged bytes (UI review 0.7.7d: the part after a
/// damaged boundary was lost, and a note printed "a ���� part").
#[test]
fn a_damaged_chunk_header_is_salvaged_through_the_index() {
    let d = every_kind();
    let mut bytes = model::write(&d, None).unwrap();
    let at = offset_of(&bytes, b"ENTS");
    for x in &mut bytes[at..at + 12] {
        *x ^= 0xa5;
    }
    let (back, rep) = model::read(&bytes).unwrap();
    assert!(rep.notes.iter().all(|n| n.is_ascii()), "{:?}", rep.notes);
    assert!(!back.model.is_empty(), "the objects were lost: {:?}", rep.notes);
    let originals: Vec<_> = d.model.iter().map(|e| e.kind.clone()).collect();
    let got: Vec<_> = back.model.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(got[..], originals[..got.len()]);
    assert_eq!(back.layers, d.layers);
    assert_eq!(back.layouts, d.layouts);
}

/// Materials and their assignments, layer filters and field values come back from a .cdy file
/// (tester 0.7.7e: reopened, it had no materials).
#[test]
fn materials_filters_and_field_values_round_trip() {
    let mut d = every_kind();
    let mut m = cadaddy_doc::material::recipe("concrete").unwrap().material(0.0254);
    m.name = "MyMat".into();
    d.materials.push(m);
    if let Some(l) = d.layers.first_mut() {
        l.material = Some("MyMat".into());
    }
    let h = d.model.iter().next().unwrap().handle;
    d.field_values.insert(h, vec!["2026-10-18".into()]);
    let bytes = model::write(&d, None).unwrap();
    let (back, rep) = model::read(&bytes).unwrap();
    assert_eq!(back.materials, d.materials, "{:?}", rep.notes);
    assert_eq!(back.layers, d.layers);
    assert_eq!(back.layer_filters, d.layer_filters);
    assert_eq!(back.field_values, d.field_values);
}

/// The sun and render exposure come back from a .cdy file (tester 0.7.7g: SUNPROPERTIES and
/// RENDEREXPOSURE reopened as defaults; .dxf and .dwg kept them).
#[test]
fn sun_and_exposure_round_trip() {
    let mut d = every_kind();
    d.sun.year = 2026;
    d.sun.month = 6;
    d.sun.day = 21;
    d.sun.latitude = 40.0;
    d.sun.longitude = -99.0;
    d.sun.exposure = 1.5;
    let bytes = model::write(&d, None).unwrap();
    let (back, rep) = model::read(&bytes).unwrap();
    assert_eq!(back.sun, d.sun, "{:?}", rep.notes);
}
