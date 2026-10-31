//! Excel data links: a workbook written by the test (shared and rich strings, inline strings,
//! number formats, booleans, a formula's value, an error, two sheets) filling a table through
//! DATALINK with a sheet and a range, kept up to date when the file changes, and TABLEEXPORT
//! to .xlsx read back.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::zip;
use cadaddy_engine::cmd::xlsx;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::json;

const NS: &str = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn workbook(cost: &str) -> Vec<u8> {
    let parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), b"<Types/>".to_vec()),
        ("xl/workbook.xml".into(), format!(r#"<?xml version="1.0"?><workbook {NS}><sheets><sheet name="Costs" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId2"/></sheets></workbook>"#).into_bytes()),
        (
            "xl/_rels/workbook.xml.rels".into(),
            br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="worksheets/other.xml"/></Relationships>"#.to_vec(),
        ),
        ("xl/sharedStrings.xml".into(), br#"<sst><si><t>Item</t></si><si><r><t>Co</t></r><r><rPr><b/></rPr><t>st</t></r></si><si><t xml:space="preserve">A &amp; B</t></si></sst>"#.to_vec()),
        (
            "xl/styles.xml".into(),
            br##"<styleSheet><numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0.00"/></numFmts><cellXfs count="4"><xf numFmtId="0"/><xf numFmtId="164"/><xf numFmtId="9"/><xf numFmtId="14"/></cellXfs></styleSheet>"##.to_vec(),
        ),
        (
            "xl/worksheets/sheet1.xml".into(),
            format!(
                r#"<worksheet {NS}><sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>Share</t></is></c><c r="D1" t="inlineStr"><is><t>Date</t></is></c></row>
<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" s="1"><v>{cost}</v></c><c r="C2" s="2"><v>0.25</v></c><c r="D2" s="3"><v>45000</v></c></row>
<row r="3"><c r="A3" t="b"><v>1</v></c><c r="B3" s="1"><f>B2*2</f><v>2469</v></c><c r="C3" t="e"><v>#DIV/0!</v></c></row>
</sheetData></worksheet>"#
            )
            .into_bytes(),
        ),
        ("xl/worksheets/other.xml".into(), format!(r#"<worksheet {NS}><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>other</t></is></c><c r="B1"><v>7</v></c><c r="C1"><v>8</v></c></row></sheetData></worksheet>"#).into_bytes()),
    ];
    zip::write(&parts).unwrap()
}

fn cells(s: &Session, h: &str) -> Vec<Vec<String>> {
    let Some(EntityKind::Table(t)) = s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).map(|e| e.kind.clone()) else { panic!() };
    t.cells.iter().map(|r| r.iter().map(|c| c.text.clone()).collect()).collect()
}

#[test]
fn a_workbook_fills_a_table_and_follows_its_changes() {
    let dir = std::env::temp_dir().join(format!("cadaddy-xlsx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("costs.xlsx");
    std::fs::write(&file, workbook("1234.5")).unwrap();
    let mut s = Session::new();
    let h = s.execute("table", &json!({ "at": [0, 0], "rows": 2, "cols": 2 })).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "startRow": 0 })).unwrap();
    assert_eq!(
        cells(&s, &h),
        [vec!["Item", "Cost", "Share", "Date"], vec!["A & B", "1,234.50", "25%", "03-15-23"], vec!["TRUE", "2,469.00", "#DIV/0!", ""],]
    );

    // A sheet and a range.
    s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "sheet": "Other", "range": "B1:C1", "startRow": 0 })).unwrap();
    assert_eq!(cells(&s, &h), [vec!["7", "8"]]);
    assert!(s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "sheet": "Nope", "startRow": 0 })).is_err());
    assert!(s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "range": "nonsense" })).is_err());

    // Back to the first sheet; then the file changes: only a changed file is read again.
    s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "startRow": 0 })).unwrap();
    assert_eq!(s.execute("datalinkupdate", &json!({ "changed": true })).unwrap()["updated"], 0);
    std::fs::write(&file, workbook("99")).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(later).unwrap();
    assert_eq!(s.execute("datalinkupdate", &json!({ "changed": true })).unwrap()["updated"], 1);
    assert_eq!(cells(&s, &h)[1][1], "99.00");
    assert_eq!(s.execute("datalinkupdate", &json!({ "changed": true })).unwrap()["updated"], 0, "read: no change since");

    // Exported to a workbook and read back.
    let out = dir.join("out.xlsx");
    s.execute("tableexport", &json!({ "handle": h, "path": out.to_string_lossy() })).unwrap();
    let wb = xlsx::open(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(wb.sheets[0].0, "Table");
    let back = wb.cells(None, None).unwrap();
    assert_eq!(back, cells(&s, &h).into_iter().map(|r| r.into_iter().collect::<Vec<_>>()).collect::<Vec<_>>());
    // Not a workbook.
    std::fs::write(dir.join("bad.xlsx"), b"not a zip").unwrap();
    assert!(s.execute("datalink", &json!({ "handle": h, "path": dir.join("bad.xlsx").to_string_lossy() })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A workbook whose one sheet part is `sheet` (the other parts are minimal).
fn workbook_with_sheet(sheet: Vec<u8>) -> Vec<u8> {
    let parts: Vec<(String, Vec<u8>)> = vec![
        ("xl/workbook.xml".into(), format!(r#"<workbook {NS}><sheets><sheet name="Big" sheetId="1" r:id="rId1"/></sheets></workbook>"#).into_bytes()),
        ("xl/_rels/workbook.xml.rels".into(), br#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#.to_vec()),
        ("xl/worksheets/sheet1.xml".into(), sheet),
    ];
    zip::write(&parts).unwrap()
}

/// A table linked to `file` through a first, good workbook, so that the file can then be swapped
/// for a hostile one.
fn linked(dir: &std::path::Path, name: &str) -> (Session, String, std::path::PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    let file = dir.join(name);
    std::fs::write(
        &file,
        workbook_with_sheet(format!(r#"<worksheet {NS}><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#).into_bytes()),
    )
    .unwrap();
    let mut s = Session::new();
    let h = s.execute("table", &json!({ "at": [0, 0], "rows": 1, "cols": 1 })).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "startRow": 0 })).unwrap();
    assert_eq!(cells(&s, &h), [vec!["1"]]);
    (s, h, file)
}

fn touch_later(file: &std::path::Path) {
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(file).unwrap().set_modified(later).unwrap();
}

#[test]
fn a_sheet_that_unpacks_to_hundreds_of_megabytes_is_refused_before_inflating() {
    let dir = std::env::temp_dir().join(format!("cadaddy-xlsx-bomb-{}", std::process::id()));
    let (mut s, h, file) = linked(&dir, "bomb.xlsx");
    // A few KB on disk: one run of spaces inside the sheet part, 300 MB (over the workbook's
    // 256 MB) and 100 MB (over one part's 64 MB).
    for (megabytes, limit) in [(300usize, "256 MB"), (100, "64 MB")] {
        let mut sheet = format!(r#"<worksheet {NS}><sheetData><row r="1"><c r="A1"><v>2</v></c></row></sheetData></worksheet>"#).into_bytes();
        sheet.extend(std::iter::repeat_n(b' ', megabytes << 20));
        let bomb = workbook_with_sheet(sheet);
        assert!(bomb.len() < 1 << 20, "the file itself is small: {} bytes", bomb.len());
        std::fs::write(&file, &bomb).unwrap();
        touch_later(&file);
        // DATALINK refuses it, naming the limit, and leaves the table as it was.
        let e = s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "startRow": 0 })).unwrap_err().to_string();
        assert!(e.contains("size limit") && e.contains(limit), "{e}");
        assert_eq!(cells(&s, &h), [vec!["1"]]);
        // So does DATALINKUPDATE for that table, and nothing is updated.
        let r = s.execute("datalinkupdate", &json!({ "changed": true })).unwrap();
        let e = r["errors"][0].as_str().unwrap();
        assert!(r["updated"] == 0 && e.contains("size limit") && e.contains(limit), "{r}");
        assert_eq!(cells(&s, &h), [vec!["1"]]);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_sheet_of_too_many_empty_cells_is_refused() {
    let dir = std::env::temp_dir().join(format!("cadaddy-xlsx-cells-{}", std::process::id()));
    let (mut s, h, file) = linked(&dir, "cells.xlsx");
    // 1 000 001 self-closing <c/>: more than the 1 000 000 cells read from one sheet.
    let mut sheet = format!(r#"<worksheet {NS}><sheetData><row r="1">"#);
    sheet.push_str(&"<c/>".repeat(1_000_001));
    sheet.push_str("</row></sheetData></worksheet>");
    std::fs::write(&file, workbook_with_sheet(sheet.into_bytes())).unwrap();
    touch_later(&file);
    let e = s.execute("datalink", &json!({ "handle": h, "path": file.to_string_lossy(), "startRow": 0 })).unwrap_err().to_string();
    assert!(e.contains("more than 1000000 cells") && e.contains("size limit"), "{e}");
    let r = s.execute("datalinkupdate", &json!({ "changed": true })).unwrap();
    assert!(r["updated"] == 0 && r["errors"][0].as_str().unwrap().contains("more than 1000000 cells"), "{r}");
    assert_eq!(cells(&s, &h), [vec!["1"]]);
    std::fs::remove_dir_all(&dir).ok();
}
