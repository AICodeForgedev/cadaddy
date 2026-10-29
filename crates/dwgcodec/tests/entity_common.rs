//! The data every entity carries besides its own (spec §20.4.1) — extended data (1001 with
//! 1000, 1040 and 1070 items), transparency (440), a colour book colour (430), reactors and an
//! extension dictionary (102 groups, 330 and 360) — decoded from a DWG written by another program
//! (acadrust, a black box) gives the groups its own DXF output gives for it, in every version
//! (Q5113). Nothing is left out of the comparison but the handle (5) and the subclass markers
//! (100), whose layout differs between writers.
mod common;

use std::collections::BTreeMap;
use std::io::Cursor;

use cadaddy_dwgcodec::entities::{Value, decode_all};
use cadaddy_dwgcodec::read;

/// A made-up drawing: two entities carrying the common data, an application of their own, and
/// the objects the extension dictionary and the reactor name.
fn fixture(id: &str) -> String {
    // Transparency and colour book colours are stored from R2004 on.
    let late = if id == "AC1015" { "" } else { "430\nPANTONE 123\n440\n33554560\n" };
    let circle_late = if id == "AC1015" { "" } else { "62\n5\n440\n33554434\n420\n255\n" };
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n9\n$HANDSEED\n5\n900\n0\nENDSEC\n\
         0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nAPPID\n70\n2\n0\nAPPID\n5\n30\n2\nACAD\n70\n0\n0\nAPPID\n5\n31\n2\nACME_APP\n70\n0\n0\nENDTAB\n\
         0\nTABLE\n2\nLAYER\n70\n1\n0\nLAYER\n5\n40\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n\
         0\nLINE\n5\n100\n102\n{{ACAD_REACTORS\n330\n300\n102\n}}\n102\n{{ACAD_XDICTIONARY\n360\n200\n102\n}}\n330\n1F\n100\nAcDbEntity\n8\n0\n{late}62\n5\n100\nAcDbLine\n10\n0\n20\n0\n30\n0\n11\n5\n21\n5\n31\n0\n\
         1001\nACME_APP\n1000\ntagged\n1040\n2.5\n1070\n42\n\
         0\nCIRCLE\n5\n101\n330\n1F\n100\nAcDbEntity\n8\n0\n{circle_late}100\nAcDbCircle\n10\n3\n20\n3\n30\n0\n40\n1.5\n\
         1001\nACME_APP\n1000\nsecond\n1070\n7\n\
         0\nENDSEC\n\
         0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n281\n1\n3\nACME\n350\n300\n\
         0\nDICTIONARY\n5\n200\n330\n100\n100\nAcDbDictionary\n281\n1\n\
         0\nDICTIONARY\n5\n300\n102\n{{ACAD_REACTORS\n330\n100\n102\n}}\n330\nC\n100\nAcDbDictionary\n281\n1\n0\nENDSEC\n0\nEOF\n"
    )
}

fn dwg(id: &str) -> Vec<u8> {
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(fixture(id).into_bytes())).and_then(|r| r.read()).expect("acadrust reads the DXF");
    doc.version = match id {
        "AC1015" => acadrust::DxfVersion::AC1015,
        "AC1018" => acadrust::DxfVersion::AC1018,
        "AC1021" => acadrust::DxfVersion::AC1021,
        "AC1024" => acadrust::DxfVersion::AC1024,
        "AC1027" => acadrust::DxfVersion::AC1027,
        _ => acadrust::DxfVersion::AC1032,
    };
    acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG")
}

/// The entity groups in the DXF acadrust writes from what it read of `dwg`, by handle.
fn theirs(dwg: &[u8]) -> BTreeMap<u64, Vec<(i32, String)>> {
    let doc = acadrust::DwgReader::from_stream(Cursor::new(dwg.to_vec())).read().expect("acadrust reads the DWG");
    let mut out = Vec::new();
    acadrust::DxfWriter::new(&doc).write_to_writer(&mut out).expect("acadrust writes DXF");
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().collect();
    let mut map = BTreeMap::new();
    let (mut section, mut want_name) = (String::new(), false);
    let mut cur: Option<Vec<(i32, String)>> = None;
    let flush = |cur: &mut Option<Vec<(i32, String)>>, map: &mut BTreeMap<u64, Vec<(i32, String)>>| {
        if let Some(groups) = cur.take()
            && let Some(h) = groups.iter().find(|g| g.0 == 5).and_then(|g| u64::from_str_radix(g.1.trim(), 16).ok())
        {
            map.insert(h, groups);
        }
    };
    let mut i = 0;
    while i + 1 < lines.len() {
        let code: i32 = lines[i].trim().parse().unwrap_or(-9999);
        let value = lines[i + 1].trim_end_matches('\r').to_string();
        i += 2;
        if code == 0 {
            flush(&mut cur, &mut map);
            if value == "SECTION" {
                want_name = true;
            } else if section == "ENTITIES" {
                cur = Some(vec![(0, value)]);
            }
            continue;
        }
        if want_name && code == 2 {
            section = value;
            want_name = false;
            continue;
        }
        if let Some(g) = cur.as_mut() {
            g.push((code, value));
        }
    }
    flush(&mut cur, &mut map);
    map
}

fn same(ours: &Value, theirs: &str) -> bool {
    let t = theirs.trim();
    match ours {
        Value::Str(s) => s.eq_ignore_ascii_case(t),
        Value::Int(i) => t.parse::<i64>().is_ok_and(|x| x == *i),
        Value::Real(f) => t.parse::<f64>().is_ok_and(|x| (x - f).abs() <= 1e-9 * f.abs().max(1.0)),
        Value::Handle(h) => u64::from_str_radix(t, 16).is_ok_and(|x| x == *h),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Handle(h) => format!("{h:X}"),
    }
}

#[test]
fn common_entity_data_matches_acadrust_in_every_version() {
    let mut report = Vec::new();
    for id in common::VERSIONS {
        let bytes = dwg(id);
        let d = read(&bytes).unwrap_or_else(|e| panic!("{id}: {e}"));
        let (ents, failed) = decode_all(&d).unwrap();
        assert!(failed.is_empty(), "{id}: undecoded {failed:?}");
        let theirs = theirs(&bytes);
        for handle in [0x100u64, 0x101] {
            let e = ents.iter().find(|e| e.handle == handle).unwrap_or_else(|| panic!("{id}: no entity {handle:x}"));
            let tg = theirs.get(&handle).unwrap_or_else(|| panic!("{id}: acadrust has no entity {handle:x}"));
            let mut codes: Vec<i32> = e.groups.iter().map(|g| g.code).chain(tg.iter().map(|g| g.0)).filter(|c| !matches!(c, 0 | 5 | 100)).collect();
            codes.sort_unstable();
            codes.dedup();
            for code in codes {
                // A true colour's colour index (62) is only a hint at the nearest ACI colour: the
                // file stores whatever its writer gave, and acadrust works its own out from the
                // value when it reads (the colour itself, 420, is compared).
                if code == 62 && e.groups.iter().any(|g| g.code == 420) {
                    continue;
                }
                let mut a: Vec<&Value> = e.groups.iter().filter(|g| g.code == code).map(|g| &g.value).collect();
                let mut b: Vec<&str> = tg.iter().filter(|g| g.0 == code).map(|g| g.1.as_str()).collect();
                // The two groups of braces (reactors, extension dictionary) are told in either order.
                if code == 102 {
                    a.sort_by_key(|v| text_of(v));
                    b.sort_unstable();
                }
                // Compared as the groups they are: the same number of each code and the same
                // values in order (handles by what they name: both read the one file).
                if a.len() != b.len() || a.iter().zip(&b).any(|(x, y)| !same(x, y)) {
                    report.push(format!(
                        "{id} {} {handle:x} group {code}: ours {:?} theirs {b:?}",
                        e.name,
                        a.iter().map(|v| text_of(v)).collect::<Vec<_>>()
                    ));
                }
            }
        }

        // The fixture's data is really there (not a comparison of two empty sides).
        let line = ents.iter().find(|e| e.handle == 0x100).unwrap();
        let get = |code: i32| -> Vec<String> { line.groups.iter().filter(|g| g.code == code).map(|g| text_of(&g.value)).collect() };
        assert_eq!(get(1001), ["ACME_APP"], "{id}");
        assert_eq!(get(1000), ["tagged"], "{id}");
        assert_eq!(get(1040), ["2.5"], "{id}");
        assert_eq!(get(1070), ["42"], "{id}");
        assert_eq!(get(102), ["{ACAD_REACTORS", "}", "{ACAD_XDICTIONARY", "}"], "{id}");
        assert_eq!(get(360), ["200"], "{id}: the extension dictionary");
        assert!(get(330).contains(&"300".to_string()), "{id}: the reactor {:?}", get(330));
        if id == "AC1015" {
            assert!(get(430).is_empty() && get(440).is_empty(), "{id}: only R2004+ stores them");
        } else {
            assert_eq!(get(430), ["PANTONE 123"], "{id}: the colour book colour");
            assert_eq!(get(440), ["33554560"], "{id}: the transparency");
            let circle = ents.iter().find(|e| e.handle == 0x101).unwrap();
            let c = |code: i32| -> Vec<String> { circle.groups.iter().filter(|g| g.code == code).map(|g| text_of(&g.value)).collect() };
            assert_eq!((c(440), c(420)), (vec!["33554434".to_string()], vec!["255".to_string()]), "{id}: a true colour with its transparency");
        }
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}
