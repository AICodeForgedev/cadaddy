//! Handles of another program's table records and objects survive a save: read → write → read
//! gives every one back, and the records still point at each other.

use cadaddy_dxf::Tag;

include!("support/foreign_dxf.rs");

/// Every record's handle in a DXF's text, by `KIND/NAME` (as `Drawing::kept_handles` keys them).
fn record_handles(text: &str) -> std::collections::BTreeMap<String, String> {
    let tags: Vec<Tag> = cadaddy_dxf::parse(text.as_bytes()).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for sec in cadaddy_dxf::sections_owned(tags) {
        if sec.name != "TABLES" {
            continue;
        }
        for (kind, tg) in cadaddy_dxf::record_slices(&sec.tags) {
            let get = |c: i32| tg.iter().find(|t| t.code == c).map(|t| t.str().trim().to_string());
            let h = if kind == "DIMSTYLE" { get(105) } else { get(5) };
            if let (Some(n), Some(h)) = (get(2), h) {
                out.insert(format!("{kind}/{}", n.to_ascii_uppercase()), h);
            }
        }
    }
    out
}

#[test]
fn table_record_and_object_handles_survive_a_save() {
    let first = cadaddy_io::read(foreign().as_bytes(), "plan.dxf").unwrap();
    let k = &first.kept_handles;
    for (key, h) in [
        ("TABLE/LAYER", 0xB1),
        ("LAYER/WALLS", 0xB3),
        ("LTYPE/DASHED", 0xA3),
        ("STYLE/NOTES", 0xC2),
        ("APPID/ACAD", 0xD2),
        ("BLOCK_RECORD/DOOR", 0xE4),
        ("BLOCK_RECORD/*MODEL_SPACE", 0xE2),
        ("BLOCK/DOOR", 0xE9),
        ("ENDBLK/DOOR", 0xEA),
        ("DICT/", 0xC),
        ("DICT/ACAD_GROUP", 0xD),
        ("GROUP/OUTLINE", 0x2F),
        ("LAYOUT/LAYOUT1", 0xF3),
    ] {
        assert_eq!(k.get(key).copied(), Some(h), "{key} read: {k:?}");
    }
    // Saved and read again: the same handles.
    let text = String::from_utf8(cadaddy_io::write(&first, "plan.dxf").unwrap()).unwrap();
    let second = cadaddy_io::read(text.as_bytes(), "plan.dxf").unwrap();
    for (key, h) in &first.kept_handles {
        // Records CADaddy writes (it writes every table and the dictionaries it models).
        if let Some(h2) = second.kept_handles.get(key) {
            assert_eq!(h2, h, "{key} changed");
        }
    }
    let written = record_handles(&text);
    for key in ["LAYER/WALLS", "LAYER/0", "LTYPE/DASHED", "LTYPE/CONTINUOUS", "STYLE/NOTES", "BLOCK_RECORD/DOOR", "BLOCK_RECORD/*MODEL_SPACE"] {
        let want = format!("{:X}", first.kept_handles.get(key).unwrap());
        assert_eq!(written.get(key), Some(&want), "{key} in the saved file");
    }
    // Links still meet: the layer records' owner is the kept LAYER table handle, the door's
    // BLOCK points at its kept block record, and entity handles are unchanged.
    let tags = cadaddy_dxf::parse(text.as_bytes()).unwrap();
    let record = |kind: &str, h: &str| {
        let at = tags.windows(2).position(|w| w[0].code == 0 && w[0].str() == kind && w[1].code == 5 && w[1].str().trim() == h).unwrap();
        tags[at..].iter().skip(1).take_while(|t| t.code != 0).cloned().collect::<Vec<Tag>>()
    };
    let owner = |r: &[Tag]| r.iter().find(|t| t.code == 330).map(|t| t.str().trim().to_string());
    assert_eq!(owner(&record("LAYER", "B3")).as_deref(), Some("B1"), "layer → its table");
    assert_eq!(owner(&record("BLOCK", "E9")).as_deref(), Some("E4"), "block → its record");
    assert!(second.model.iter().any(|e| e.handle.0 == 0x100) && second.model.iter().any(|e| e.handle.0 == 0x101));
    // No handle is written twice.
    let mut seen = std::collections::HashSet::new();
    for t in tags.iter().filter(|t| t.code == 5 || t.code == 105) {
        assert!(seen.insert(t.str().trim().to_ascii_uppercase()), "handle {} twice", t.str());
    }
}

#[test]
fn a_drawing_of_our_own_numbers_as_before() {
    let d = cadaddy_doc::Drawing::new_metric();
    assert!(d.kept_handles.is_empty());
    let text = String::from_utf8(cadaddy_io::write(&d, "x.dxf").unwrap()).unwrap();
    let back = cadaddy_io::read(text.as_bytes(), "x.dxf").unwrap();
    let again = String::from_utf8(cadaddy_io::write(&back, "x.dxf").unwrap()).unwrap();
    assert_eq!(record_handles(&text), record_handles(&again), "stable across saves");
}
