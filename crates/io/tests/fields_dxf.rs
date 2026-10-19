//! Fields read from DXF as AutoCAD lays them out (DXF Reference, FIELD): the text's extension
//! dictionary → ACAD_FIELD dictionary → TEXT → a `_text` FIELD owning one FIELD per field, each
//! with its code and cached value. The codes go back into the text, the values are kept, and
//! saving writes one set of field objects again (none kept twice).

use cadaddy_doc::{EntityKind, Handle};
use cadaddy_io::{read_dxf, write_dxf};

fn dxf(pairs: &[(i32, &str)]) -> String {
    pairs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

/// An MTEXT whose text holds a file name and a plot date field, and an INSERT whose attribute
/// holds a sheet number field.
fn sample() -> String {
    let mut p: Vec<(i32, &str)> = vec![(0, "SECTION"), (2, "BLOCKS"), (0, "BLOCK"), (2, "TB"), (70, "2"), (10, "0"), (20, "0"), (30, "0"), (3, "TB")];
    p.extend([
        (0, "ATTDEF"),
        (5, "60"),
        (8, "0"),
        (10, "0"),
        (20, "0"),
        (30, "0"),
        (40, "1"),
        (1, "x"),
        (100, "AcDbAttributeDefinition"),
        (3, "Sheet"),
        (2, "NO"),
        (70, "0"),
    ]);
    p.extend([(0, "ENDBLK"), (0, "ENDSEC")]);
    p.extend([(0, "SECTION"), (2, "ENTITIES")]);
    p.extend([
        (0, "MTEXT"),
        (5, "2A"),
        (102, "{ACAD_XDICTIONARY"),
        (360, "E0"),
        (102, "}"),
        (100, "AcDbEntity"),
        (8, "0"),
        (100, "AcDbMText"),
        (10, "0"),
        (20, "0"),
        (30, "0"),
        (40, "1"),
        (71, "1"),
        (1, "File: Old.dwg  Plotted: 12/5/2006 11:15:39 AM"),
    ]);
    p.extend([
        (0, "INSERT"),
        (5, "2B"),
        (8, "0"),
        (66, "1"),
        (2, "TB"),
        (10, "5"),
        (20, "5"),
        (30, "0"),
        (0, "ATTRIB"),
        (5, "2C"),
        (102, "{ACAD_XDICTIONARY"),
        (360, "F0"),
        (102, "}"),
        (330, "2B"),
        (100, "AcDbEntity"),
        (8, "0"),
        (100, "AcDbText"),
        (10, "5"),
        (20, "5"),
        (30, "0"),
        (40, "1"),
        (1, "A-1"),
        (100, "AcDbAttribute"),
        (2, "NO"),
        (70, "0"),
        (0, "SEQEND"),
        (5, "2D"),
        (8, "0"),
    ]);
    p.extend([(0, "ENDSEC"), (0, "SECTION"), (2, "OBJECTS")]);
    p.extend([(0, "DICTIONARY"), (5, "C"), (330, "0"), (100, "AcDbDictionary"), (281, "1")]);
    // The MTEXT's fields.
    p.extend([(0, "DICTIONARY"), (5, "E0"), (330, "2A"), (100, "AcDbDictionary"), (280, "1"), (281, "1"), (3, "ACAD_FIELD"), (360, "E1")]);
    p.extend([(0, "DICTIONARY"), (5, "E1"), (330, "E0"), (100, "AcDbDictionary"), (280, "1"), (281, "1"), (3, "TEXT"), (360, "E2")]);
    p.extend([
        (0, "FIELD"),
        (5, "E2"),
        (330, "E1"),
        (100, "AcDbField"),
        (1, "_text"),
        (2, "File: %<\\_FldIdx 0>%  Plotted: %<\\_FldIdx 1>%"),
        (90, "2"),
        (360, "E3"),
        (360, "E4"),
        (97, "0"),
        (91, "63"),
        (92, "0"),
        (94, "13"),
        (95, "2"),
        (96, "0"),
        (300, ""),
        (93, "1"),
        (6, "ACFD_FIELDTEXT_CHECKSUM"),
        (93, "2"),
        (90, "2"),
        (140, "1132241.0"),
        (94, "0"),
        (300, ""),
        (302, ""),
        (304, "ACVALUE_END"),
        (7, "ACFD_FIELD_VALUE"),
        (93, "3"),
        (90, "0"),
        (94, "0"),
        (300, ""),
        (302, ""),
        (304, "ACVALUE_END"),
        (301, ""),
        (98, "0"),
    ]);
    p.extend([
        (0, "FIELD"),
        (5, "E3"),
        (330, "E2"),
        (100, "AcDbField"),
        (1, "AcVar"),
        (2, "\\AcVar Filename \\f \"%fn6\""),
        (90, "0"),
        (97, "0"),
        (91, "63"),
        (92, "0"),
        (94, "59"),
        (95, "2"),
        (96, "0"),
        (300, ""),
        (93, "1"),
        (6, "Variable"),
        (93, "0"),
        (90, "4"),
        (1, "Filename"),
        (94, "0"),
        (300, ""),
        (302, ""),
        (304, "ACVALUE_END"),
        (7, "ACFD_FIELD_VALUE"),
        (93, "4"),
        (90, "4"),
        (1, "C:\\jobs\\Old.dwg"),
        (94, "0"),
        (300, "%fn6"),
        (302, "Old.dwg"),
        (304, "ACVALUE_END"),
        (301, "Old.dwg"),
        (98, "7"),
    ]);
    p.extend([
        (0, "FIELD"),
        (5, "E4"),
        (330, "E2"),
        (100, "AcDbField"),
        (1, "AcVar"),
        (2, "\\AcVar PlotDate \\f \"M/d/yyyy h:mm:ss tt\""),
        (90, "0"),
        (97, "0"),
        (91, "63"),
        (92, "0"),
        (94, "59"),
        (95, "32"),
        (96, "0"),
        (300, ""),
        (93, "0"),
        (7, "ACFD_FIELD_VALUE"),
        (93, "4"),
        (90, "8"),
        (92, "16"),
        (310, "D6070C00020005000B000F0027000000"),
        (94, "0"),
        (300, "M/d/yyyy h:mm:ss tt"),
        (302, "12/5/2006 11:15:39 AM"),
        (304, "ACVALUE_END"),
        (301, "12/5/2006 11:15:39 AM"),
        (98, "21"),
    ]);
    // The attribute's field.
    p.extend([(0, "DICTIONARY"), (5, "F0"), (330, "2C"), (100, "AcDbDictionary"), (281, "1"), (3, "ACAD_FIELD"), (360, "F1")]);
    p.extend([(0, "DICTIONARY"), (5, "F1"), (330, "F0"), (100, "AcDbDictionary"), (281, "1"), (3, "TEXT"), (360, "F2")]);
    p.extend([
        (0, "FIELD"),
        (5, "F2"),
        (330, "F1"),
        (100, "AcDbField"),
        (1, "_text"),
        (2, "%<\\_FldIdx 0>%"),
        (90, "1"),
        (360, "F3"),
        (97, "0"),
        (301, ""),
    ]);
    p.extend([
        (0, "FIELD"),
        (5, "F3"),
        (330, "F2"),
        (100, "AcDbField"),
        (1, "AcSm"),
        (2, "\\AcSm Sheet.Number \\f \"%tc1\""),
        (90, "0"),
        (97, "0"),
        (7, "ACFD_FIELD_VALUE"),
        (93, "0"),
        (90, "4"),
        (1, "a-1"),
        (304, "ACVALUE_END"),
        (301, "A-1"),
        (98, "3"),
    ]);
    p.extend([(0, "ENDSEC"), (0, "EOF")]);
    dxf(&p)
}

#[test]
fn autocad_fields_are_read() {
    let d = read_dxf(sample().as_bytes()).unwrap();
    let m = d.entity(Handle(0x2A)).unwrap();
    let EntityKind::MText(t) = &m.kind else { panic!("{:?}", m.kind) };
    assert_eq!(t.contents, "File: %<\\AcVar Filename \\f \"%fn6\">%  Plotted: %<\\AcVar PlotDate \\f \"M/d/yyyy h:mm:ss tt\">%");
    assert_eq!(d.field_values.get(&Handle(0x2A)).unwrap(), &["Old.dwg", "12/5/2006 11:15:39 AM"]);
    let i = d.entity(Handle(0x2B)).unwrap();
    let EntityKind::Insert(ins) = &i.kind else { panic!() };
    assert_eq!(ins.attribs[0].text.value, "%<\\AcSm Sheet.Number \\f \"%tc1\">%");
    assert_eq!(d.field_values.get(&Handle(0x2B)).unwrap(), &["A-1"]);
}

#[test]
fn saving_writes_one_set_of_field_objects() {
    let d = read_dxf(sample().as_bytes()).unwrap();
    let text = write_dxf(&d);
    let count = |t: &str| t.lines().filter(|l| l.trim() == "FIELD").count();
    let fields = count(&text);
    assert_eq!(fields, 5, "two texts' _text fields and their three fields, none kept twice");
    assert!(text.contains("File: Old.dwg  Plotted: 12/5/2006 11:15:39 AM"));
    let back = read_dxf(text.as_bytes()).unwrap();
    assert_eq!(back.field_values, d.field_values);
    assert_eq!(count(&write_dxf(&back)), 5);
}

#[test]
fn broken_field_objects_leave_the_text_as_it_was() {
    // The dictionary points at a missing FIELD; a cycle of children; a child that isn't a FIELD.
    let bad = sample().replace("(360\nE2\n", "").replace("360\nE2\n", "360\nZZ\n").replace("360\nF3\n", "360\nF2\n");
    let d = read_dxf(bad.as_bytes()).unwrap();
    let EntityKind::MText(t) = &d.entity(Handle(0x2A)).unwrap().kind else { panic!() };
    assert_eq!(t.contents, "File: Old.dwg  Plotted: 12/5/2006 11:15:39 AM");
    assert!(!d.field_values.contains_key(&Handle(0x2A)));
    let EntityKind::Insert(ins) = &d.entity(Handle(0x2B)).unwrap().kind else { panic!() };
    assert_eq!(ins.attribs[0].text.value, "A-1", "a field owning itself is left out");
}

/// Fields come back from a DWG file (tester 0.7.7f: after a DWG round trip they were static
/// text: the bridge's DWG writer dropped every FIELD object whose class the drawing didn't
/// declare). Both ways the drawing goes to the bridge: binary DXF, and text when it has materials.
#[test]
fn fields_survive_a_dwg_round_trip() {
    let d = read_dxf(sample().as_bytes()).unwrap();
    let mut with_material = d.clone();
    with_material.materials.push(cadaddy_doc::material::recipe("concrete").unwrap().material(0.0254));
    for d in [d, with_material] {
        let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
        let back = cadaddy_io::read(&dwg, "x.dwg").unwrap();
        let EntityKind::MText(t) = &back.entity(Handle(0x2A)).unwrap().kind else { panic!("the MTEXT") };
        assert_eq!(t.contents, "File: %<\\AcVar Filename \\f \"%fn6\">%  Plotted: %<\\AcVar PlotDate \\f \"M/d/yyyy h:mm:ss tt\">%");
        assert_eq!(back.field_values, d.field_values, "the cached values too");
        let EntityKind::Insert(ins) = &back.entity(Handle(0x2B)).unwrap().kind else { panic!("the INSERT") };
        assert_eq!(ins.attribs[0].text.value, "%<\\AcSm Sheet.Number \\f \"%tc1\">%");
        // And again: one set of fields, still fields.
        let again = cadaddy_io::read(&cadaddy_io::write(&back, "x.dwg").unwrap(), "x.dwg").unwrap();
        assert_eq!(again.field_values, d.field_values);
    }
}
