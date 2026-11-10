//! Control characters in text survive a DWG save and reopen (B4-013): a tab in MTEXT came back as
//! `^I` and a line break in dimension text as `^J`. DXF stores control characters in their caret
//! form, DWG stores them as they are; a `^` the user typed must stay a `^`.

use std::sync::Arc;

use cadaddy_doc::{Block, Common, DimKind, Dimension, Drawing, Entity, EntityKind, Insert, MLeader, MText, Space, Text};
use cadaddy_geom::Vec3;
use cadaddy_io::DwgEngine;

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

/// Text that sits on one line (TEXT, ATTRIB, ATTDEF): a tab, the caret forms of control
/// characters and a real caret before a letter.
const LINE: [&str; 10] = ["A\tB", "\u{2022}\tFirst item", "A^IB", "A^JB", "A^MB", "A^bB", "caret at end^", "^^I and ^I", "tab\tand^Jmix", "plain"];

/// Text with line breaks (MTEXT, MLEADER text, dimension text) as well.
const MULTI: [&str; 4] = ["A\nB", "4.50\n2 SLOTS", "\u{2022}\tFirst item\n\u{2022}\tSecond item", "tab\tand^Jboth\nend"];

fn all() -> impl Iterator<Item = &'static str> {
    LINE.iter().chain(MULTI.iter()).copied()
}

fn v(x: f64, y: f64) -> Vec3 {
    Vec3::new(x, y, 0.0)
}

fn text(value: &str) -> Text {
    serde_json::from_value(serde_json::json!({"insert": {"x": 1.0, "y": 2.0, "z": 0.0}, "height": 2.5, "value": value})).unwrap()
}

fn mtext(contents: &str) -> MText {
    serde_json::from_value(serde_json::json!({"insert": {"x": 3.0, "y": 4.0, "z": 0.0}, "height": 2.5, "contents": contents})).unwrap()
}

fn attrib(tag: &str, value: &str) -> cadaddy_doc::Attrib {
    serde_json::from_value(serde_json::json!({"tag": tag, "text": text(value)})).unwrap()
}

fn dimension(t: &str) -> Dimension {
    Dimension {
        insertion_point: Default::default(),
        kind: DimKind::Linear { rotation: 0.0 },
        defpt: v(5.0, 3.0),
        text_mid: v(5.0, 6.0),
        p13: v(0.0, 0.0),
        p14: v(10.0, 0.0),
        p15: Vec3::ZERO,
        p16: Vec3::ZERO,
        text: t.into(),
        style: "Standard".into(),
        measurement: 10.0,
        text_rotation: 0.0,
        user_text_pos: false,
        block: None,
        overrides: Default::default(),
        assoc: Vec::new(),
        marks: Default::default(),
    }
}

fn mleader(t: &str) -> MLeader {
    serde_json::from_value(serde_json::json!({
        "leaders": [[{"x": 0.0, "y": 0.0, "z": 0.0}, {"x": 5.0, "y": 5.0, "z": 0.0}]],
        "landing": {"x": 5.0, "y": 5.0, "z": 0.0},
        "dogleg": 1.0,
        "text": mtext(t),
    }))
    .unwrap()
}

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let mut def = Block::new("TAG");
    for (i, s) in LINE.iter().enumerate() {
        let h = d.new_handle();
        def.entities.push(Entity::new(h, EntityKind::AttDef(Box::new(attrib(&format!("D{i}"), s)))));
    }
    d.blocks.insert("TAG".into(), Arc::new(def));
    for s in LINE {
        d.add(&Space::Model, Common::default(), EntityKind::Text(text(s))).unwrap();
    }
    for s in all() {
        d.add(&Space::Model, Common::default(), EntityKind::MText(mtext(s))).unwrap();
        d.add(&Space::Model, Common::default(), EntityKind::Dimension(Box::new(dimension(s)))).unwrap();
        d.add(&Space::Model, Common::default(), EntityKind::MLeader(Box::new(mleader(&format!("L:{s}"))))).unwrap();
    }
    let attribs = LINE.iter().enumerate().map(|(i, s)| attrib(&format!("A{i}"), s)).collect();
    let ins = Insert {
        block: "TAG".into(),
        insert: v(20.0, 20.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs,
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
    d
}

/// A line break as the contents write it (`\P`) or as a raw line feed: the same text.
fn canon(s: &str) -> String {
    s.replace("\\P", "\n")
}

/// Every text string in the drawing, labelled by where it sits.
fn strings(d: &Drawing) -> Vec<String> {
    let mut out = Vec::new();
    let of = |e: &Entity, out: &mut Vec<String>| match &e.kind {
        EntityKind::Text(t) => out.push(format!("TEXT {:?}", t.value)),
        // R2000 has no multileader: its text is left as MTEXT.
        EntityKind::MText(m) if m.contents.starts_with("L:") => out.push(format!("MLEADER Some({:?})", canon(&m.contents))),
        EntityKind::MText(m) => out.push(format!("MTEXT {:?}", canon(&m.contents))),
        EntityKind::Dimension(x) => out.push(format!("DIM {:?}", canon(&x.text))),
        EntityKind::MLeader(m) => out.push(format!("MLEADER {:?}", m.text.as_ref().map(|t| canon(&t.contents)))),
        EntityKind::Insert(i) => out.extend(i.attribs.iter().map(|a| format!("ATTRIB {} {:?}", a.tag, a.text.value))),
        EntityKind::AttDef(a) => out.push(format!("ATTDEF {} {:?}", a.tag, a.text.value)),
        _ => {}
    };
    for e in d.model.iter() {
        of(e, &mut out);
    }
    if let Some(b) = d.blocks.get("TAG") {
        for e in b.entities.iter() {
            of(e, &mut out);
        }
    }
    out
}

fn bridge(d: &Drawing, ver: &str) -> Drawing {
    let version = cadaddy_io::Version::for_acadver(ver).unwrap();
    let bytes = cadaddy_io::write_dwg(d, version, DwgEngine::Bridge).unwrap();
    cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap()
}

fn native(d: &Drawing, ver: &str) -> Drawing {
    let (bytes, _) = cadaddy_io::dwg_write::write(d, ver).unwrap();
    cadaddy_io::dwg_read::read(&bytes).unwrap().0
}

/// A typed `^I` or `^J` in multiline text: the DWG bridge's DXF writes it exactly as it writes a
/// tab or a line feed there, so it reads back as the control character (the codec keeps it).
fn bridge_ambiguous(s: &str) -> bool {
    (s.starts_with("MTEXT") || s.starts_with("MLEADER")) && (s.contains("^I") || s.contains("^J"))
}

fn check(label: &str, want: &[String], got: &Drawing) {
    let got = strings(got);
    let skip = label.starts_with("bridge");
    let bad: Vec<String> = want.iter().filter(|w| !(got.contains(w) || skip && bridge_ambiguous(w))).cloned().collect();
    assert!(bad.is_empty(), "{label}: lost {} of {}: {:#?}", bad.len(), want.len(), &bad[..bad.len().min(8)]);
}

#[test]
fn control_characters_survive_the_codec_in_every_version_and_generation() {
    let d = drawing();
    let want = strings(&d);
    for ver in VERSIONS {
        let once = native(&d, ver);
        check(&format!("native {ver} first"), &want, &once);
        check(&format!("native {ver} second"), &want, &native(&once, ver));
    }
}

#[test]
fn control_characters_survive_the_dwg_bridge_in_every_version_and_generation() {
    let d = drawing();
    let want = strings(&d);
    for ver in VERSIONS {
        let once = bridge(&d, ver);
        check(&format!("bridge {ver} first"), &want, &once);
        check(&format!("bridge {ver} second"), &want, &bridge(&once, ver));
    }
}

#[test]
fn control_characters_survive_dwg_dxf_dwg() {
    let d = drawing();
    let want = strings(&d);
    for ver in VERSIONS {
        let dxf = cadaddy_io::write_dxf(&native(&d, ver));
        let via = cadaddy_io::read_dxf(dxf.as_bytes()).unwrap();
        check(&format!("{ver} via DXF"), &want, &via);
        check(&format!("{ver} DWG-DXF-DWG"), &want, &native(&via, ver));
    }
}

/// The battle-test files (local only, never committed): no caret code appears in any text after a
/// save as DWG and a reopen that the file did not have to begin with.
#[test]
fn the_battle_files_show_no_caret_codes_after_the_round_trip() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = std::path::Path::new(&home).join("cadwork/tasks/fixtures/battle4");
    let carets = |d: &Drawing| strings(d).iter().filter(|s| s.contains('^')).count();
    for f in ["annot_t1.dxf", "regress_al.dxf", "orig_carriage.dwg"] {
        let Ok(bytes) = std::fs::read(dir.join(f)) else { continue };
        let d0 = cadaddy_io::read(&bytes, f).unwrap();
        let want = carets(&d0);
        for ver in VERSIONS {
            let version = cadaddy_io::Version::for_acadver(ver).unwrap();
            for (w, r) in [(DwgEngine::Bridge, DwgEngine::Bridge), (DwgEngine::Native, DwgEngine::Bridge), (DwgEngine::Bridge, DwgEngine::Native)] {
                let once = cadaddy_io::read_dwg(&cadaddy_io::write_dwg(&d0, version, w).unwrap(), r).unwrap();
                assert_eq!(carets(&once), want, "{f} {ver} {w:?}->{r:?}");
                let twice = cadaddy_io::read_dwg(&cadaddy_io::write_dwg(&once, version, w).unwrap(), r).unwrap();
                assert_eq!(carets(&twice), want, "{f} {ver} {w:?}->{r:?} second generation");
            }
        }
    }
}

/// A DWG written by one codec and read by the other keeps every string: both hold text in the DWG
/// as its characters (a tab as a tab, a typed `^I` as the caret and the letter), so neither
/// reads the other's caret codes as text or its typed carets as control characters.
#[test]
fn control_characters_survive_a_dwg_written_by_one_codec_and_read_by_the_other() {
    let d = drawing();
    let want = strings(&d);
    for ver in VERSIONS {
        let version = cadaddy_io::Version::for_acadver(ver).unwrap();
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, ver).unwrap();
        check(&format!("native {ver} -> bridge"), &want, &cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap());
        // The bridge's own writing of a typed `^I` in multiline text is ambiguous (see
        // `bridge_ambiguous`): the label starting "bridge" skips those strings.
        let bytes = cadaddy_io::write_dwg(&d, version, DwgEngine::Bridge).unwrap();
        check(&format!("bridge {ver} -> native"), &want, &cadaddy_io::dwg_read::read(&bytes).unwrap().0);
    }
}

/// What CADaddy's own codec stores in the DWG: the characters, as an AutoCAD DWG has them (a tab
/// in MTEXT where its DXF has `^I`), never DXF's caret codes.
#[test]
fn the_native_dwg_holds_text_as_its_characters() {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::MText(mtext("\u{2022}\tFirst item ^I typed"))).unwrap();
    d.add(&Space::Model, Common::default(), EntityKind::Text(text("10^2 and\ta tab"))).unwrap();
    for ver in VERSIONS {
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, ver).unwrap();
        let doc = acadrust::DwgReader::from_stream(std::io::Cursor::new(bytes)).read().unwrap();
        let mut seen = Vec::new();
        for e in doc.entities() {
            match e {
                acadrust::EntityType::MText(t) => seen.push(t.value.clone()),
                acadrust::EntityType::Text(t) => seen.push(t.value.clone()),
                _ => {}
            }
        }
        seen.sort();
        assert_eq!(seen, ["10^2 and\ta tab", "\u{2022}\tFirst item ^I typed"], "{ver}");
    }
}

/// A DWG from another writer holds text as its characters: a typed `^I` or `^ ` stays the
/// characters through CADaddy's own codec, and a tab stays a tab. The bridge reads the same
/// except a TEXT's caret before a space: its DXF writer leaves that caret unescaped (its rule for
/// SAT data), so the space reads as the caret's escape.
#[test]
fn typed_carets_in_another_writers_dwg_stay_text() {
    for ver in [acadrust::DxfVersion::AC1015, acadrust::DxfVersion::AC1018, acadrust::DxfVersion::AC1032] {
        let mut doc = acadrust::CadDocument::new();
        doc.version = ver;
        for (i, value) in ["A^IB", "caret ^ space", "real\ttab"].into_iter().enumerate() {
            let at = acadrust::Vector3::new(0.0, 10.0 * i as f64, 0.0);
            doc.add_entity(acadrust::EntityType::MText(Box::new(acadrust::MText::with_value(value, at)))).unwrap();
            doc.add_entity(acadrust::EntityType::Text(acadrust::Text::with_value(value, at))).unwrap();
        }
        let bytes = acadrust::DwgWriter::write_to_vec(&doc).unwrap();
        for (label, back) in [("native", cadaddy_io::dwg_read::read(&bytes).unwrap().0), ("bridge", cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap())]
        {
            let mut got: Vec<String> = back
                .model
                .iter()
                .filter_map(|e| match &e.kind {
                    EntityKind::MText(m) => Some(format!("MTEXT {:?}", m.contents)),
                    EntityKind::Text(t) => Some(format!("TEXT {:?}", t.value)),
                    _ => None,
                })
                .collect();
            got.sort();
            let want = [
                "MTEXT \"A^IB\"",
                "MTEXT \"caret ^ space\"",
                "MTEXT \"real\\ttab\"",
                "TEXT \"A^IB\"",
                "TEXT \"caret ^ space\"",
                "TEXT \"real\\ttab\"",
            ];
            let want: Vec<&str> = want.into_iter().filter(|w| label == "native" || *w != "TEXT \"caret ^ space\"").collect();
            got.retain(|g| label == "native" || !g.starts_with("TEXT \"caret ^"));
            assert_eq!(got, want, "{label} {ver:?}");
        }
    }
}
