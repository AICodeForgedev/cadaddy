//! Dynamic blocks from another program: their parameters and actions are read into
//! `cadcraft_doc::dynblock`, and survive a save unchanged.
//!
//! The fixture is synthetic, written for this test: a DOOR block whose BLOCK_RECORD carries an
//! evaluation graph with a visibility parameter (Open, Closed), a linear parameter (Width, a list
//! of values), a stretch action on it, a lookup parameter and its lookup table, and a grip.

use cadcraft_doc::dynblock::{ActionKind, DynamicBlock, ParameterKind, dynamic_block};
use cadcraft_doc::{Handle, RawTag};
use cadcraft_io::{read_dxf, write_dxf};

/// DXF text from lines of `code value` (the value may be empty).
fn dxf(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (code, value) = line.split_once(' ').unwrap_or((line, ""));
        out.push_str(&format!("{code}\n{value}\n"));
    }
    out
}

const FIXTURE: &str = "
0 SECTION
2 HEADER
9 $ACADVER
1 AC1032
9 $HANDSEED
5 200
0 ENDSEC
0 SECTION
2 TABLES
0 TABLE
2 BLOCK_RECORD
5 1
330 0
100 AcDbSymbolTable
70 3
0 BLOCK_RECORD
5 1F
330 1
100 AcDbSymbolTableRecord
100 AcDbBlockTableRecord
2 *Model_Space
340 0
0 BLOCK_RECORD
5 1E
330 1
100 AcDbSymbolTableRecord
100 AcDbBlockTableRecord
2 *Paper_Space
340 0
0 BLOCK_RECORD
5 1D
102 {ACAD_XDICTIONARY
360 60
102 }
330 1
100 AcDbSymbolTableRecord
100 AcDbBlockTableRecord
2 Door
340 0
70 0
280 1
281 1
0 ENDTAB
0 ENDSEC
0 SECTION
2 BLOCKS
0 BLOCK
5 20
330 1F
100 AcDbEntity
8 0
100 AcDbBlockBegin
2 *Model_Space
70 0
10 0
20 0
30 0
3 *Model_Space
1
0 ENDBLK
5 21
330 1F
100 AcDbEntity
8 0
100 AcDbBlockEnd
0 BLOCK
5 24
330 1D
100 AcDbEntity
8 0
100 AcDbBlockBegin
2 Door
70 0
10 0
20 0
30 0
3 Door
1
0 LINE
5 30
330 1D
100 AcDbEntity
8 0
100 AcDbLine
10 0
20 0
30 0
11 36
21 0
31 0
0 LINE
5 31
330 1D
100 AcDbEntity
8 0
100 AcDbLine
10 0
20 0
30 0
11 0
21 36
31 0
0 LINE
5 32
330 1D
100 AcDbEntity
8 0
100 AcDbLine
10 0
20 0
30 0
11 25
21 25
31 0
0 ENDBLK
5 25
330 1D
100 AcDbEntity
8 0
100 AcDbBlockEnd
0 ENDSEC
0 SECTION
2 ENTITIES
0 ENDSEC
0 SECTION
2 OBJECTS
0 DICTIONARY
5 C
330 0
100 AcDbDictionary
281 1
0 DICTIONARY
5 60
330 1D
100 AcDbDictionary
280 1
281 1
3 ACAD_ENHANCEDBLOCK
360 61
0 ACAD_EVALUATION_GRAPH
5 61
102 {ACAD_REACTORS
330 60
102 }
330 60
100 AcDbEvalGraph
96 6
97 6
91 0
93 32
95 1
360 62
92 -1
92 -1
92 -1
92 -1
91 1
93 32
95 2
360 63
92 -1
92 -1
92 -1
92 -1
91 2
93 32
95 3
360 64
92 -1
92 -1
92 -1
92 -1
91 3
93 32
95 4
360 65
92 -1
92 -1
92 -1
92 -1
91 4
93 32
95 5
360 66
92 -1
92 -1
92 -1
92 -1
91 5
93 32
95 6
360 67
92 -1
92 -1
92 -1
92 -1
0 BLOCKVISIBILITYPARAMETER
5 62
330 61
100 AcDbEvalExpr
90 1
98 33
99 232
100 AcDbBlockElement
300 Visibility State
98 33
99 232
1071 0
100 AcDbBlockParameter
280 1
281 0
100 AcDbBlock1PtParameter
1010 18
1020 40
1030 0
93 0
170 0
171 0
100 AcDbBlockVisibilityParameter
281 1
301 Visibility1
302 Door swing shown or not
91 0
93 2
331 31
331 32
92 2
303 Open
94 2
332 31
332 32
95 1
333 63
303 Closed
94 0
95 0
0 BLOCKLINEARPARAMETER
5 63
330 61
100 AcDbEvalExpr
90 2
98 33
99 232
100 AcDbBlockElement
300 Linear
98 33
99 232
1071 0
100 AcDbBlockParameter
280 1
281 0
100 AcDbBlock2PtParameter
1010 0
1020 -4
1030 0
1011 36
1021 -4
1031 0
170 4
91 0
91 0
91 0
91 0
171 0
172 0
173 0
174 0
177 0
100 AcDbBlockLinearParameter
305 Width
306 Door leaf width
140 0
307
96 8
141 0
142 0
143 0
175 3
144 30
144 32
144 36
0 BLOCKSTRETCHACTION
5 64
330 61
100 AcDbEvalExpr
90 3
98 33
99 232
100 AcDbBlockElement
300 Stretch
98 33
99 232
1071 0
100 AcDbBlockAction
70 0
71 1
330 30
1010 40
1020 -8
1030 0
100 AcDbBlockStretchAction
92 2
93 2
301 EndXDelta
302 EndYDelta
72 2
1011 34
1021 -2
1011 38
1021 2
73 1
331 30
74 1
94 1
75 0
140 1
141 0
280 0
0 BLOCKLOOKUPPARAMETER
5 65
330 61
100 AcDbEvalExpr
90 4
98 33
99 232
100 AcDbBlockElement
300 Lookup
98 33
99 232
1071 0
100 AcDbBlockParameter
280 1
281 0
100 AcDbBlock1PtParameter
1010 -6
1020 40
1030 0
93 0
170 0
171 0
100 AcDbBlockLookUpParameter
94 5
303 Size
304 Standard door sizes
0 BLOCKLOOKUPACTION
5 66
330 61
100 AcDbEvalExpr
90 5
98 33
99 232
100 AcDbBlockElement
300 Lookup1
98 33
99 232
1071 0
100 AcDbBlockAction
70 0
71 0
1010 -4
1020 38
1030 0
100 AcDbBlockLookupAction
92 2
93 2
301
302 30
302 Narrow
302 36
302 Wide
303
94 2
95 1
96 0
282 0
305 Custom
281 0
304 UpdatedDistance
303
94 4
95 1
96 0
282 1
305 Custom
281 1
304 lookupString
280 1
0 BLOCKFLIPGRIP
5 67
330 61
100 AcDbEvalExpr
90 6
98 33
99 232
100 AcDbBlockElement
300 Grip
98 33
99 232
1071 0
0 ENDSEC
0 EOF
";

fn h(hex: &str) -> Handle {
    Handle::parse_hex(hex).unwrap()
}

#[test]
fn a_dynamic_block_reads_its_parameters_and_actions() {
    let d = read_dxf(dxf(FIXTURE).as_bytes()).unwrap();
    assert_eq!(d.block("DOOR").unwrap().entities.len(), 3);
    let m = dynamic_block(&d, "door").expect("Door is a dynamic block");
    assert!(dynamic_block(&d, "*Model_Space").is_none(), "model space has no graph here");

    let labels: Vec<_> = m.parameters.iter().map(|p| p.kind.label().unwrap_or("-")).collect();
    assert_eq!(labels, ["Visibility1", "Width", "Size"]);
    let ParameterKind::Visibility { states, controlled, description, .. } = &m.parameters[0].kind else { panic!("{:?}", m.parameters[0]) };
    assert_eq!(description, "Door swing shown or not");
    assert_eq!(controlled, &[h("31"), h("32")]);
    let names: Vec<_> = states.iter().map(|s| (s.name.as_str(), s.visible.len(), s.active.len())).collect();
    assert_eq!(names, [("Open", 2, 1), ("Closed", 0, 0)]);

    let p = &m.parameters[1];
    assert_eq!((p.node, p.element.as_str(), p.show_properties), (2, "Linear", true));
    let ParameterKind::Linear { base, end, values, description, .. } = &p.kind else { panic!("{p:?}") };
    assert_eq!((base.x, base.y, end.x, end.y), (0.0, -4.0, 36.0, -4.0));
    assert_eq!(description, "Door leaf width");
    assert!(values.is_list() && !values.has_minimum());
    assert_eq!(values.list, [30.0, 32.0, 36.0]);

    let names: Vec<_> = m.actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Stretch", "Lookup1"]);
    let a = &m.actions[0];
    assert_eq!(a.selection, [h("30")]);
    assert_eq!(a.connections, [(2, "EndXDelta".to_string()), (2, "EndYDelta".to_string())]);
    let ActionKind::Stretch { frame, vertices, multiplier, .. } = &a.kind else { panic!("{a:?}") };
    assert_eq!(frame.len(), 2);
    assert_eq!((frame[0].x, frame[1].y), (34.0, 2.0));
    assert_eq!(vertices, &[(h("30"), vec![1])]);
    assert_eq!(*multiplier, 1.0);

    let ActionKind::Lookup { columns, rows } = &m.actions[1].kind else { panic!("{:?}", m.actions[1]) };
    assert_eq!(rows, &[vec!["30".to_string(), "Narrow".to_string()], vec!["36".to_string(), "Wide".to_string()]]);
    let cols: Vec<_> = columns.iter().map(|c| (c.node, c.property.as_str(), c.lookup, c.unmatched.as_str())).collect();
    assert_eq!(cols, [(2, "UpdatedDistance", false, "Custom"), (4, "lookupString", true, "Custom")]);

    assert_eq!(m.other, ["BLOCKFLIPGRIP"]);
}

#[test]
fn a_saved_drawing_keeps_the_dynamic_block_as_read() {
    let d = read_dxf(dxf(FIXTURE).as_bytes()).unwrap();
    let before = dynamic_block(&d, "Door").unwrap();
    let again = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    // The objects may get new handles on save; the block's objects keep theirs, and the graph's
    // references between its members follow the new ones.
    assert_eq!(by_node(dynamic_block(&again, "Door").unwrap()), by_node(before.clone()));
    // And through DWG: the lookup parameter (AutoCAD writes its marker AcDbBlockLookUpParameter)
    // keeps its name and lookup action.
    let dwg = cadcraft_io::write(&d, "x.dwg").unwrap();
    let back = cadcraft_io::read(&dwg, "x.dwg").unwrap();
    assert_eq!(by_node(dynamic_block(&back, "Door").unwrap()), by_node(before));
}

/// The model with each parameter and action object named by its graph id instead of its handle.
fn by_node(mut m: DynamicBlock) -> DynamicBlock {
    let ids: std::collections::HashMap<Handle, i32> =
        m.parameters.iter().map(|p| (p.handle, p.node)).chain(m.actions.iter().map(|a| (a.handle, a.node))).collect();
    let id = |h: &Handle| Handle(ids.get(h).copied().unwrap_or(-1) as u64);
    for p in &mut m.parameters {
        p.handle = id(&p.handle);
        if let ParameterKind::Visibility { states, .. } = &mut p.kind {
            for s in states {
                s.active = s.active.iter().map(id).collect();
            }
        }
    }
    for a in &mut m.actions {
        a.handle = id(&a.handle);
    }
    m
}

#[test]
fn damaged_dynamic_block_data_is_skipped_not_trusted() {
    // Counts far beyond the groups present, values that aren't numbers, a graph member that
    // doesn't exist and a dictionary entry with no graph: reading must not panic or hang.
    let src = FIXTURE
        .replace("93 2\n331 31", "93 999999999\n331 31")
        .replace("92 2\n303 Open", "92 -7\n303 Open")
        .replace("175 3", "175 2147483647")
        .replace("144 32", "144 nan")
        .replace("72 2\n1011 34", "72 99999\n1011 34")
        .replace("92 2\n93 2\n301\n", "92 1000000\n93 1000000\n301\n")
        .replace("360 67", "360 FFFF");
    let d = read_dxf(dxf(&src).as_bytes()).unwrap();
    let m = dynamic_block(&d, "Door").unwrap();
    assert_eq!(m.parameters.len(), 3);
    let ParameterKind::Linear { values, .. } = &m.parameters[1].kind else { panic!() };
    assert!(values.list.len() <= 3 && values.list.iter().all(|v| v.is_finite()), "{:?}", values.list);
    assert!(m.other.is_empty(), "a member that doesn't exist is skipped");

    let tags = |v: &[(i32, &str)]| v.iter().map(|(c, s)| RawTag { code: *c, value: (*s).into() }).collect::<Vec<_>>();
    let mut d = read_dxf(dxf(FIXTURE).as_bytes()).unwrap();
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    if let Some(dict) = f.objects.iter_mut().find(|o| o.handle == h("60")) {
        dict.tags = tags(&[(100, "AcDbDictionary"), (3, "ACAD_ENHANCEDBLOCK")]);
    }
    assert!(dynamic_block(&d, "Door").is_none(), "no graph handle");
}

/// The fixture with references to Door: one with Door's own values (handle 80), and two to
/// anonymous blocks with values of their own, as another program leaves them: 81 (scale 2,
/// Closed, Width 30, so Size is Narrow) and 82 (Width 33, which no Size row has).
fn with_references() -> String {
    let record =
        |h: &str, name: &str| format!("0 BLOCK_RECORD\n5 {h}\n330 1\n100 AcDbSymbolTableRecord\n100 AcDbBlockTableRecord\n2 {name}\n340 0\n70 0\n");
    let block = |h: &str, rec: &str, name: &str| {
        format!(
            "0 BLOCK\n5 {h}\n330 {rec}\n100 AcDbEntity\n8 0\n100 AcDbBlockBegin\n2 {name}\n70 1\n10 0\n20 0\n30 0\n3 {name}\n1\n0 LINE\n5 {h}1\n330 {rec}\n100 AcDbEntity\n8 0\n100 AcDbLine\n10 0\n20 0\n30 0\n11 30\n21 0\n31 0\n0 ENDBLK\n5 {h}2\n330 {rec}\n100 AcDbEntity\n8 0\n100 AcDbBlockEnd\n"
        )
    };
    let insert = |h: &str, block: &str, xdict: Option<&str>, scale: f64| {
        let x = xdict.map(|x| format!("102 {{ACAD_XDICTIONARY\n360 {x}\n102 }}\n")).unwrap_or_default();
        format!(
            "0 INSERT\n5 {h}\n{x}330 1F\n100 AcDbEntity\n8 0\n100 AcDbBlockReference\n2 {block}\n10 100\n20 0\n30 0\n41 {scale}\n42 {scale}\n43 {scale}\n"
        )
    };
    let dict = |h: &str, owner: &str, entries: &[(&str, &str)]| {
        let e: String = entries.iter().map(|(k, v)| format!("3 {k}\n360 {v}\n")).collect();
        format!("0 DICTIONARY\n5 {h}\n330 {owner}\n100 AcDbDictionary\n280 1\n281 1\n{e}")
    };
    let xrecord =
        |h: &str, owner: &str, body: &str| format!("0 XRECORD\n5 {h}\n330 {owner}\n100 AcDbXrecord\n280 1\n1071 1\n1071 2\n70 25\n70 104\n{body}");
    // The values of a reference: representation data pointing at Door's record (1D) and one
    // XRECORD per parameter graph id.
    let values = |p: &str, owner: &str, vis: &str, width: f64, size: &str| {
        [
            dict(&format!("{p}0"), owner, &[("AcDbBlockRepresentation", &format!("{p}1"))]),
            dict(&format!("{p}1"), &format!("{p}0"), &[("AcDbRepData", &format!("{p}2")), ("AppDataCache", &format!("{p}3"))]),
            format!("0 ACDB_BLOCKREPRESENTATION_DATA\n5 {p}2\n330 {p}1\n100 AcDbBlockRepresentationData\n70 1\n340 1D\n"),
            dict(&format!("{p}3"), &format!("{p}1"), &[("ACAD_ENHANCEDBLOCKDATA", &format!("{p}4"))]),
            dict(
                &format!("{p}4"),
                &format!("{p}3"),
                &[("1", &format!("{p}5")), ("2", &format!("{p}6")), ("3", &format!("{p}7")), ("4", &format!("{p}8"))],
            ),
            xrecord(&format!("{p}5"), &format!("{p}4"), &format!("10 18\n20 40\n30 0\n1 {vis}\n")),
            xrecord(&format!("{p}6"), &format!("{p}4"), &format!("10 0\n20 -4\n30 0\n10 {width}\n20 -4\n30 0\n10 0\n20 0\n30 -1\n")),
            xrecord(&format!("{p}7"), &format!("{p}4"), "40 0\n"),
            xrecord(&format!("{p}8"), &format!("{p}4"), &format!("10 -6\n20 40\n30 0\n1 {size}\n")),
        ]
        .concat()
    };
    let src = dxf(FIXTURE);
    let lines = |s: String| dxf(&s);
    src.replace("0\nENDTAB\n", &format!("{}0\nENDTAB\n", lines(record("A1", "*U1") + &record("B1", "*U2"))))
        .replace(
            "0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nENDSEC\n",
            &lines(
                block("A2", "A1", "*U1")
                    + &block("B2", "B1", "*U2")
                    + "0 ENDSEC\n0 SECTION\n2 ENTITIES\n"
                    + &insert("80", "Door", None, 1.0)
                    + &insert("81", "*U1", Some("C0"), 2.0)
                    + &insert("82", "*U2", Some("D0"), 1.0)
                    + "0 ENDSEC\n",
            ),
        )
        .replace(
            "0\nENDSEC\n0\nEOF\n",
            &format!("{}0\nENDSEC\n0\nEOF\n", lines(values("C", "81", "Closed", 30.0, "Narrow") + &values("D", "82", "Open", 33.0, "Narrow"))),
        )
}

fn props(d: &cadcraft_doc::Drawing, h: &str) -> (String, Vec<(String, String, Vec<String>)>) {
    let (name, ps) = cadcraft_doc::dynblock::custom_properties(d, Handle::parse_hex(h).unwrap()).unwrap();
    (name, ps.into_iter().map(|p| (p.name, p.value, p.choices)).collect())
}

#[test]
fn dynamic_block_references_show_their_values() {
    let d = read_dxf(with_references().as_bytes()).unwrap();
    assert_eq!(d.model.len(), 3, "the three references were read");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // Door's own values.
    let (name, p) = props(&d, "80");
    assert_eq!(name, "Door");
    assert_eq!(
        p,
        [
            ("Visibility1".into(), "Open".into(), s(&["Open", "Closed"])),
            ("Width".into(), "36.0000".into(), s(&["30.0000", "32.0000", "36.0000"])),
            ("Size".into(), "Wide".into(), s(&["Narrow", "Wide"])),
        ]
    );

    // An anonymous block with values of its own, scaled 2: lengths in drawing units.
    let r = cadcraft_doc::dynblock::reference(&d, h("81")).unwrap();
    assert_eq!(r.definition, "Door");
    assert_eq!(r.values.len(), 3, "the action's record has no value");
    let (name, p) = props(&d, "81");
    assert_eq!(name, "Door");
    assert_eq!(
        p,
        [
            ("Visibility1".into(), "Closed".into(), s(&["Open", "Closed"])),
            ("Width".into(), "60.0000".into(), s(&["60.0000", "64.0000", "72.0000"])),
            ("Size".into(), "Narrow".into(), s(&["Narrow", "Wide"])),
        ]
    );

    // A width no Size row has: the lookup shows its "no match" name, whatever was stored.
    let (_, p) = props(&d, "82");
    assert_eq!(p[2], ("Size".into(), "Custom".into(), s(&["Narrow", "Wide"])));

    // Other references and objects aren't dynamic block references.
    assert!(cadcraft_doc::dynblock::reference(&d, h("30")).is_none(), "a line in the block");
    assert!(cadcraft_doc::dynblock::reference(&d, h("FFFF")).is_none());
}

#[test]
fn dynamic_block_references_survive_a_save() {
    let d = read_dxf(with_references().as_bytes()).unwrap();
    let again = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    for hx in ["80", "81", "82"] {
        assert_eq!(props(&again, hx), props(&d, hx), "{hx}");
    }
}
