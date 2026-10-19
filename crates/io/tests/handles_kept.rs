//! Handles of another program's table records and objects survive a save: read → write → read
//! gives every one back, and the records still point at each other.

use cadaddy_dxf::Tag;

/// A small DXF as another program writes it, with its own handle numbering.
fn foreign() -> String {
    let mut t: Vec<(i32, &str)> = vec![(0, "SECTION"), (2, "HEADER"), (9, "$ACADVER"), (1, "AC1015"), (9, "$HANDSEED"), (5, "900"), (0, "ENDSEC")];
    t.extend([(0, "SECTION"), (2, "TABLES")]);
    // Linetypes, layers, text styles, app ids, block records.
    t.extend([(0, "TABLE"), (2, "LTYPE"), (5, "A1"), (330, "0"), (100, "AcDbSymbolTable"), (70, "2")]);
    t.extend([
        (0, "LTYPE"),
        (5, "A2"),
        (330, "A1"),
        (100, "AcDbSymbolTableRecord"),
        (100, "AcDbLinetypeTableRecord"),
        (2, "Continuous"),
        (70, "0"),
        (3, "Solid"),
        (72, "65"),
        (73, "0"),
        (40, "0.0"),
    ]);
    t.extend([
        (0, "LTYPE"),
        (5, "A3"),
        (330, "A1"),
        (100, "AcDbSymbolTableRecord"),
        (100, "AcDbLinetypeTableRecord"),
        (2, "DASHED"),
        (70, "0"),
        (3, "__ __"),
        (72, "65"),
        (73, "2"),
        (40, "0.75"),
        (49, "0.5"),
        (74, "0"),
        (49, "-0.25"),
        (74, "0"),
    ]);
    t.push((0, "ENDTAB"));
    t.extend([(0, "TABLE"), (2, "LAYER"), (5, "B1"), (330, "0"), (100, "AcDbSymbolTable"), (70, "2")]);
    t.extend([
        (0, "LAYER"),
        (5, "B2"),
        (330, "B1"),
        (100, "AcDbSymbolTableRecord"),
        (100, "AcDbLayerTableRecord"),
        (2, "0"),
        (70, "0"),
        (62, "7"),
        (6, "Continuous"),
    ]);
    t.extend([
        (0, "LAYER"),
        (5, "B3"),
        (330, "B1"),
        (100, "AcDbSymbolTableRecord"),
        (100, "AcDbLayerTableRecord"),
        (2, "WALLS"),
        (70, "0"),
        (62, "1"),
        (6, "DASHED"),
    ]);
    t.push((0, "ENDTAB"));
    t.extend([(0, "TABLE"), (2, "STYLE"), (5, "C1"), (330, "0"), (100, "AcDbSymbolTable"), (70, "1")]);
    t.extend([
        (0, "STYLE"),
        (5, "C2"),
        (330, "C1"),
        (100, "AcDbSymbolTableRecord"),
        (100, "AcDbTextStyleTableRecord"),
        (2, "Notes"),
        (70, "0"),
        (40, "0.0"),
        (41, "1.0"),
        (50, "0.0"),
        (71, "0"),
        (42, "2.5"),
        (3, "arial.ttf"),
        (4, ""),
    ]);
    t.push((0, "ENDTAB"));
    t.extend([(0, "TABLE"), (2, "APPID"), (5, "D1"), (330, "0"), (100, "AcDbSymbolTable"), (70, "1")]);
    t.extend([(0, "APPID"), (5, "D2"), (330, "D1"), (100, "AcDbSymbolTableRecord"), (100, "AcDbRegAppTableRecord"), (2, "ACAD"), (70, "0")]);
    t.push((0, "ENDTAB"));
    t.extend([(0, "TABLE"), (2, "BLOCK_RECORD"), (5, "E1"), (330, "0"), (100, "AcDbSymbolTable"), (70, "3")]);
    for (h, n, l) in [("E2", "*Model_Space", "F2"), ("E3", "*Paper_Space", "F3"), ("E4", "DOOR", "0")] {
        t.extend([(0, "BLOCK_RECORD"), (5, h), (330, "E1"), (100, "AcDbSymbolTableRecord"), (100, "AcDbBlockTableRecord"), (2, n), (340, l)]);
    }
    t.push((0, "ENDTAB"));
    t.push((0, "ENDSEC"));
    // Blocks.
    t.extend([(0, "SECTION"), (2, "BLOCKS")]);
    for (bh, eh, owner, n) in [("E5", "E6", "E2", "*Model_Space"), ("E7", "E8", "E3", "*Paper_Space"), ("E9", "EA", "E4", "DOOR")] {
        t.extend([
            (0, "BLOCK"),
            (5, bh),
            (330, owner),
            (100, "AcDbEntity"),
            (8, "0"),
            (100, "AcDbBlockBegin"),
            (2, n),
            (70, "0"),
            (10, "0.0"),
            (20, "0.0"),
            (30, "0.0"),
            (3, n),
            (1, ""),
        ]);
        if n == "DOOR" {
            t.extend([
                (0, "LINE"),
                (5, "EB"),
                (330, "E4"),
                (100, "AcDbEntity"),
                (8, "0"),
                (100, "AcDbLine"),
                (10, "0"),
                (20, "0"),
                (30, "0"),
                (11, "3"),
                (21, "0"),
                (31, "0"),
            ]);
        }
        t.extend([(0, "ENDBLK"), (5, eh), (330, owner), (100, "AcDbEntity"), (8, "0"), (100, "AcDbBlockEnd")]);
    }
    t.push((0, "ENDSEC"));
    t.extend([(0, "SECTION"), (2, "ENTITIES")]);
    t.extend([
        (0, "LINE"),
        (5, "100"),
        (330, "E2"),
        (100, "AcDbEntity"),
        (8, "WALLS"),
        (100, "AcDbLine"),
        (10, "0"),
        (20, "0"),
        (30, "0"),
        (11, "10"),
        (21, "0"),
        (31, "0"),
    ]);
    t.extend([
        (0, "INSERT"),
        (5, "101"),
        (330, "E2"),
        (100, "AcDbEntity"),
        (8, "0"),
        (100, "AcDbBlockReference"),
        (2, "DOOR"),
        (10, "5"),
        (20, "5"),
        (30, "0"),
    ]);
    t.push((0, "ENDSEC"));
    // Objects: the root dictionary, groups and layouts.
    t.extend([(0, "SECTION"), (2, "OBJECTS")]);
    t.extend([
        (0, "DICTIONARY"),
        (5, "C"),
        (330, "0"),
        (100, "AcDbDictionary"),
        (281, "1"),
        (3, "ACAD_GROUP"),
        (350, "D"),
        (3, "ACAD_LAYOUT"),
        (350, "1A"),
    ]);
    t.extend([(0, "DICTIONARY"), (5, "D"), (330, "C"), (100, "AcDbDictionary"), (281, "1"), (3, "OUTLINE"), (350, "2F")]);
    t.extend([(0, "GROUP"), (5, "2F"), (330, "D"), (100, "AcDbGroup"), (300, ""), (70, "0"), (71, "1"), (340, "100")]);
    t.extend([(0, "DICTIONARY"), (5, "1A"), (330, "C"), (100, "AcDbDictionary"), (281, "1"), (3, "Model"), (350, "F2"), (3, "Layout1"), (350, "F3")]);
    for (h, n, br, ord) in [("F2", "Model", "E2", "0"), ("F3", "Layout1", "E3", "1")] {
        t.extend([
            (0, "LAYOUT"),
            (5, h),
            (330, "1A"),
            (100, "AcDbPlotSettings"),
            (1, ""),
            (100, "AcDbLayout"),
            (1, n),
            (70, "1"),
            (71, ord),
            (330, br),
        ]);
    }
    t.push((0, "ENDSEC"));
    t.push((0, "EOF"));
    t.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

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
