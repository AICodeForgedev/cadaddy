//! Entities decoded from DWG files written by another program (acadrust, a black box) give the
//! same DXF groups as that program's own DXF output for the same file, group by group.
mod common;

use std::collections::BTreeMap;
use std::io::Cursor;

use cadaddy_dwgcodec::entities::{Value, decode_all};
use cadaddy_dwgcodec::read;

/// A made-up drawing with every entity type decoded here.
fn rich_dxf() -> String {
    let mut e = String::new();
    let mut add = |s: &str| e.push_str(s);
    add("0\nLINE\n8\nWALLS\n62\n3\n10\n1\n20\n2\n30\n0\n11\n9\n21\n7\n31\n0\n");
    add("0\nLINE\n8\n0\n10\n1\n20\n2\n30\n3\n11\n4\n21\n5\n31\n6\n39\n2.5\n");
    add("0\nCIRCLE\n8\n0\n10\n5\n20\n5\n30\n0\n40\n2.5\n");
    add("0\nARC\n8\n0\n10\n15\n20\n5\n30\n0\n40\n3\n50\n30\n51\n200\n");
    add("0\nPOINT\n8\n0\n10\n7\n20\n8\n30\n0\n");
    add("0\nELLIPSE\n8\n0\n10\n20\n20\n20\n30\n0\n11\n6\n21\n2\n31\n0\n40\n0.4\n41\n0\n42\n3.14159\n");
    add("0\nTEXT\n8\n0\n10\n1\n20\n30\n30\n0\n40\n2\n1\nRoom 101\n50\n15\n");
    add("0\nTEXT\n8\n0\n10\n1\n20\n35\n30\n0\n40\n2\n1\nCentred\n72\n1\n11\n10\n21\n35\n31\n0\n");
    add("0\nMTEXT\n8\n0\n10\n30\n20\n30\n30\n0\n40\n2.5\n41\n20\n71\n1\n72\n1\n1\nTwo\\Plines of text\n");
    add("0\nLWPOLYLINE\n8\nWALLS\n90\n4\n70\n1\n10\n0\n20\n50\n10\n10\n20\n50\n42\n0.5\n10\n10\n20\n60\n10\n0\n20\n60\n");
    add("0\nLWPOLYLINE\n8\n0\n90\n2\n70\n0\n43\n0.5\n10\n0\n20\n70\n10\n20\n20\n70\n");
    add("0\nSOLID\n8\n0\n10\n40\n20\n0\n30\n0\n11\n45\n21\n0\n31\n0\n12\n40\n22\n5\n32\n0\n13\n45\n23\n5\n33\n0\n");
    add("0\n3DFACE\n8\n0\n10\n50\n20\n0\n30\n0\n11\n55\n21\n0\n31\n1\n12\n55\n22\n5\n32\n2\n13\n50\n23\n5\n33\n3\n");
    add("0\nRAY\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n0\n31\n0\n");
    add("0\nXLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n0\n21\n1\n31\n0\n");
    add(
        "0\nSPLINE\n8\n0\n70\n8\n71\n3\n72\n8\n73\n4\n74\n0\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n10\n0\n20\n80\n30\n0\n10\n5\n20\n90\n30\n0\n10\n10\n20\n80\n30\n0\n10\n15\n20\n90\n30\n0\n",
    );
    add(
        "0\nPOLYLINE\n8\n0\n66\n1\n10\n0\n20\n0\n30\n0\n70\n1\n0\nVERTEX\n8\n0\n10\n60\n20\n0\n30\n0\n0\nVERTEX\n8\n0\n10\n70\n20\n0\n30\n0\n42\n0.3\n0\nVERTEX\n8\n0\n10\n70\n20\n10\n30\n0\n0\nSEQEND\n8\n0\n",
    );
    add(
        "0\nPOLYLINE\n8\n0\n66\n1\n10\n0\n20\n0\n30\n0\n70\n8\n0\nVERTEX\n8\n0\n10\n80\n20\n0\n30\n1\n70\n32\n0\nVERTEX\n8\n0\n10\n90\n20\n0\n30\n2\n70\n32\n0\nSEQEND\n8\n0\n",
    );
    add(
        "0\nINSERT\n8\n0\n66\n1\n2\nTAG\n10\n100\n20\n0\n30\n0\n41\n2\n42\n2\n43\n2\n50\n45\n0\nATTRIB\n8\n0\n10\n100\n20\n0\n30\n0\n40\n1\n1\nA-12\n2\nNUMBER\n70\n0\n0\nSEQEND\n8\n0\n",
    );
    add("0\nINSERT\n8\n0\n2\nTAG\n10\n110\n20\n0\n30\n0\n");
    add(
        "0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n2\nSOLID\n70\n1\n71\n0\n91\n1\n92\n2\n72\n0\n73\n1\n93\n4\n10\n0\n20\n100\n10\n10\n20\n100\n10\n10\n20\n110\n10\n0\n20\n110\n97\n0\n75\n0\n76\n1\n98\n0\n",
    );
    add(
        "0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n2\nANSI31\n70\n0\n71\n0\n91\n1\n92\n1\n93\n3\n72\n1\n10\n20\n20\n100\n11\n30\n21\n100\n72\n2\n10\n25\n20\n100\n40\n5\n50\n0\n51\n180\n73\n1\n72\n1\n10\n20\n20\n100\n11\n30\n21\n100\n97\n0\n75\n0\n76\n1\n52\n0\n41\n1\n77\n0\n78\n1\n53\n45\n43\n0\n44\n0\n45\n-2.245\n46\n2.245\n79\n0\n98\n1\n10\n25\n20\n102\n",
    );
    add(
        "0\nDIMENSION\n8\n0\n2\n*D1\n10\n10\n20\n130\n30\n0\n11\n5\n21\n131\n31\n0\n70\n32\n3\nStandard\n100\nAcDbAlignedDimension\n13\n0\n23\n120\n33\n0\n14\n10\n24\n120\n34\n0\n50\n0\n100\nAcDbRotatedDimension\n",
    );
    add(
        "0\nDIMENSION\n8\n0\n2\n*D2\n10\n20\n20\n130\n30\n0\n11\n25\n21\n131\n31\n0\n70\n33\n3\nStandard\n100\nAcDbAlignedDimension\n13\n20\n23\n120\n33\n0\n14\n30\n24\n125\n34\n0\n",
    );
    add(
        "0\nDIMENSION\n8\n0\n2\n*D3\n10\n40\n20\n120\n30\n0\n11\n42\n21\n122\n31\n0\n70\n36\n3\nStandard\n100\nAcDbRadialDimension\n15\n44\n25\n120\n35\n0\n40\n0\n",
    );
    add(
        "0\nDIMENSION\n8\n0\n2\n*D4\n10\n50\n20\n120\n30\n0\n11\n52\n21\n122\n31\n0\n70\n35\n3\nStandard\n100\nAcDbDiametricDimension\n15\n46\n25\n120\n35\n0\n40\n0\n",
    );
    add(
        "0\nDIMENSION\n8\n0\n2\n*D5\n10\n60\n20\n120\n30\n0\n11\n62\n21\n125\n31\n0\n70\n37\n3\nStandard\n100\nAcDb3PointAngularDimension\n13\n60\n23\n110\n33\n0\n14\n70\n24\n120\n34\n0\n15\n60\n25\n120\n35\n0\n",
    );
    add(
        "0\nDIMENSION\n8\n0\n2\n*D6\n10\n80\n20\n120\n30\n0\n11\n82\n21\n125\n31\n0\n70\n102\n3\nStandard\n100\nAcDbOrdinateDimension\n13\n80\n23\n110\n33\n0\n14\n85\n24\n130\n34\n0\n",
    );
    add("0\nLEADER\n8\n0\n3\nStandard\n71\n1\n72\n0\n73\n3\n76\n3\n10\n0\n20\n140\n30\n0\n10\n5\n20\n145\n30\n0\n10\n10\n20\n145\n30\n0\n");
    add(
        "0\nMLINE\n8\n0\n2\nSTANDARD\n40\n1\n70\n0\n71\n1\n72\n3\n73\n2\n10\n0\n20\n200\n30\n0\n11\n0\n21\n200\n31\n0\n12\n1\n22\n0\n32\n0\n13\n0\n23\n1\n33\n0\n74\n2\n41\n0.5\n41\n0\n75\n0\n74\n2\n41\n-0.5\n41\n0\n75\n0\n11\n10\n21\n200\n31\n0\n12\n1\n22\n0\n32\n0\n13\n0\n23\n1\n33\n0\n74\n2\n41\n0.5\n41\n0\n75\n0\n74\n2\n41\n-0.5\n41\n0\n75\n0\n11\n20\n21\n210\n31\n0\n12\n0\n22\n1\n32\n0\n13\n-1\n23\n1\n33\n0\n74\n2\n41\n0.5\n41\n0\n75\n0\n74\n2\n41\n-0.5\n41\n0\n75\n0\n",
    );
    add(
        "0\nWIPEOUT\n8\n0\n90\n0\n10\n0\n20\n220\n30\n0\n11\n10\n21\n0\n31\n0\n12\n0\n22\n10\n32\n0\n13\n1\n23\n1\n70\n7\n280\n1\n281\n50\n282\n50\n283\n0\n71\n2\n91\n5\n14\n-0.5\n24\n0.5\n14\n0.5\n24\n0.5\n14\n0.5\n24\n-0.5\n14\n-0.5\n24\n-0.5\n14\n-0.5\n24\n0.5\n",
    );
    add(
        "0\nVIEWPORT\n8\n0\n67\n1\n10\n5\n20\n5\n30\n0\n40\n8\n41\n6\n68\n2\n69\n2\n12\n50\n22\n50\n13\n0\n23\n0\n14\n1\n24\n1\n15\n1\n25\n1\n16\n0\n26\n0\n36\n1\n17\n0\n27\n0\n37\n0\n42\n50\n43\n0\n44\n0\n45\n40\n50\n0\n51\n0\n72\n100\n90\n32864\n1\n\n281\n0\n71\n1\n74\n0\n110\n0\n120\n0\n130\n0\n111\n1\n121\n0\n131\n0\n112\n0\n122\n1\n132\n0\n79\n0\n146\n0\n170\n0\n",
    );
    add("0\nTOLERANCE\n8\n0\n3\nStandard\n10\n20\n20\n140\n30\n0\n1\n{\\Fgdt;j}%%v0.5\n11\n1\n21\n0\n31\n0\n");
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n\
         0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n70\n2\n\
         0\nLAYER\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n\
         0\nLAYER\n2\nWALLS\n70\n0\n62\n1\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
         0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n8\n0\n2\nTAG\n70\n2\n10\n0\n20\n0\n30\n0\n3\nTAG\n\
         0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n\
         0\nATTDEF\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n1\n?\n3\nNumber?\n2\nNUMBER\n70\n0\n0\nENDBLK\n8\n0\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n{e}0\nENDSEC\n0\nEOF\n"
    )
}

fn dwg(id: &str) -> Vec<u8> {
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(rich_dxf().into_bytes())).and_then(|r| r.read()).expect("acadrust reads the DXF");
    doc.version = match id {
        "AC1015" => acadrust::DxfVersion::AC1015,
        "AC1018" => acadrust::DxfVersion::AC1018,
        "AC1024" => acadrust::DxfVersion::AC1024,
        "AC1027" => acadrust::DxfVersion::AC1027,
        _ => acadrust::DxfVersion::AC1032,
    };
    acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG")
}

/// The entity groups in the DXF acadrust writes from what it read of `dwg`, by handle.
fn theirs(dwg: &[u8]) -> BTreeMap<u64, (String, Vec<(i32, String)>)> {
    let doc = acadrust::DwgReader::from_stream(Cursor::new(dwg.to_vec())).read().expect("acadrust reads the DWG");
    let mut out = Vec::new();
    acadrust::DxfWriter::new(&doc).write_to_writer(&mut out).expect("acadrust writes DXF");
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().collect();
    let mut map = BTreeMap::new();
    let mut section = String::new();
    let mut cur: Option<(String, Vec<(i32, String)>)> = None;
    let flush = |cur: &mut Option<(String, Vec<(i32, String)>)>, map: &mut BTreeMap<u64, (String, Vec<(i32, String)>)>| {
        if let Some((name, groups)) = cur.take()
            && let Some(h) = groups.iter().find(|g| g.0 == 5).and_then(|g| u64::from_str_radix(g.1.trim(), 16).ok())
        {
            map.insert(h, (name, groups));
        }
    };
    let mut i = 0;
    let mut want_name = false;
    while i + 1 < lines.len() {
        let code: i32 = lines[i].trim().parse().unwrap_or(-9999);
        let value = lines[i + 1].trim_end_matches('\r').to_string();
        i += 2;
        if code == 0 {
            flush(&mut cur, &mut map);
            if value == "SECTION" {
                want_name = true;
                continue;
            }
            if section == "ENTITIES" || section == "BLOCKS" {
                cur = Some((value.clone(), Vec::new()));
            }
            continue;
        }
        if want_name && code == 2 {
            section = value;
            want_name = false;
            continue;
        }
        if let Some((_, g)) = cur.as_mut() {
            g.push((code, value));
        }
    }
    flush(&mut cur, &mut map);
    map
}

fn same(ours: &Value, theirs: &str) -> bool {
    let t = theirs.trim();
    match ours {
        Value::Str(s) => s == t || (s.eq_ignore_ascii_case(t)),
        Value::Int(i) => t.parse::<i64>().is_ok_and(|x| x == *i),
        Value::Real(f) => t.parse::<f64>().is_ok_and(|x| (x - f).abs() <= 1e-9 * f.abs().max(1.0)),
        Value::Handle(h) => u64::from_str_radix(t, 16).is_ok_and(|x| x == *h),
    }
}

/// Group codes compared: the entity's own data and common properties (not subclass markers,
/// owners or handles, whose layout differs between writers).
fn compared(code: i32) -> bool {
    !matches!(code, 5 | 100 | 102 | 330 | 360 | 347 | 390)
}

/// The values a group defaults to when left out (DXF Reference).
fn default(name: &str, code: i32) -> Vec<&'static str> {
    match (name, code) {
        (_, 230) => vec!["1"],
        ("TEXT" | "ATTRIB" | "ATTDEF", 41) => vec!["1"],
        ("INSERT" | "MINSERT", 41..=43) => vec!["1"],
        ("DIMENSION", 41) => vec!["1"],
        ("LEADER", 77) => vec!["256"],
        ("VIEWPORT", 61) => vec!["5"],
        ("VIEWPORT", 282 | 292) => vec!["0", "1"],
        ("LEADER", 40) => vec!["2.5", "0"],
        (_, 6) => vec!["ByLayer", "BYLAYER"],
        (_, 62) => vec!["256"],
        (_, 370) => vec!["-1"],
        (_, 48) => vec!["1"],
        _ => vec!["0"],
    }
}

#[test]
fn every_decoded_entity_matches_acadrusts_dxf_for_it() {
    let mut report = Vec::new();
    for v in common::VERSIONS {
        let bytes = dwg(v);
        let d = read(&bytes).unwrap_or_else(|e| panic!("{v}: {e}"));
        let (ents, failed) = decode_all(&d).unwrap();
        assert!(failed.is_empty(), "{v}: undecoded {failed:?}");
        let theirs = theirs(&bytes);
        let mut checked: BTreeMap<String, usize> = BTreeMap::new();
        for e in &ents {
            let Some((tname, tg)) = theirs.get(&e.handle) else {
                // The vertices and end of a polyline acadrust writes as an LWPOLYLINE.
                if matches!(e.name.as_str(), "VERTEX" | "SEQEND") {
                    continue;
                }
                report.push(format!("{v} {} {:x}: not in acadrust's DXF", e.name, e.handle));
                continue;
            };
            // acadrust's DXF writes a 2D POLYLINE as an LWPOLYLINE: compared at the drawing level.
            if e.name == "POLYLINE" && tname == "LWPOLYLINE" {
                continue;
            }
            if *tname != e.name {
                report.push(format!("{v} {:x}: {} vs {tname}", e.handle, e.name));
                continue;
            }
            *checked.entry(e.name.clone()).or_default() += 1;
            // Each code's values in order.
            let mut codes: Vec<i32> = e.groups.iter().map(|g| g.code).chain(tg.iter().map(|g| g.0)).filter(|c| compared(*c)).collect();
            codes.sort_unstable();
            codes.dedup();
            for code in codes {
                // A dimension without a drawn block: acadrust names it *U0.
                if e.name == "DIMENSION" && code == 2 && tg.iter().any(|g| g.0 == 2 && g.1.trim() == "*U0") {
                    continue;
                }
                // A viewport's status (68) and id (69) aren't stored in a DWG: they come from the
                // layout's list of viewports.
                if e.name == "VIEWPORT" && matches!(code, 68 | 69) {
                    continue;
                }
                // Vertices: acadrust writes no layer for them.
                if e.name == "VERTEX" && code == 8 {
                    continue;
                }
                // Values equal to the group's default are left out by one writer, written by the
                // other: compare what isn't a default.
                let dflt = default(&e.name, code);
                let a: Vec<&Value> =
                    e.groups.iter().filter(|g| g.code == code).map(|g| &g.value).filter(|v| !dflt.iter().any(|d| same(v, d))).collect();
                let b: Vec<&str> = tg
                    .iter()
                    .filter(|g| g.0 == code)
                    .map(|g| g.1.as_str())
                    .filter(|t| {
                        !dflt.iter().any(|d| {
                            same(&Value::Str(d.to_string()), t) || t.trim().parse::<f64>().is_ok_and(|x| d.parse::<f64>().is_ok_and(|y| x == y))
                        })
                    })
                    .collect();
                if a.len() != b.len() || a.iter().zip(&b).any(|(x, y)| !same(x, y)) {
                    report.push(format!("{v} {} {:x} group {code}: ours {a:?} theirs {b:?}", e.name, e.handle));
                }
            }
        }
        eprintln!("{v}: checked {checked:?}");
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

/// Damaged object data never makes the entity decoder panic or run away: every object of a real
/// file, with bytes flipped, cut short or zeroed, decodes or is refused.
#[test]
fn hostile_entity_data_never_panics() {
    use cadaddy_dwgcodec::entities::{Names, decode};
    for v in ["AC1015", "AC1024"] {
        let bytes = dwg(v);
        let d = read(&bytes).unwrap();
        let classes = d.classes().unwrap();
        let names = Names::read(&d, &classes);
        let mut x = 0x2545_f491u32;
        for raw in &d.objects {
            for round in 0..24 {
                let mut o = raw.clone();
                match round % 3 {
                    0 => {
                        for _ in 0..4 {
                            x ^= x << 13;
                            x ^= x >> 17;
                            x ^= x << 5;
                            if !o.data.is_empty() {
                                let i = x as usize % o.data.len();
                                o.data[i] ^= (x >> 8) as u8 | 1;
                            }
                        }
                    }
                    1 => o.data.truncate(o.data.len() * round / 24),
                    _ => o.data.iter_mut().skip(round).for_each(|b| *b = 0xff),
                }
                let _ = decode(&o, d.version, &classes, &names);
            }
        }
    }
}
