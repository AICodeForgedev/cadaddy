//! Table records and objects decoded from DWG files written by another program (acadrust, a
//! black box) give the same DXF groups as that program's own DXF output for the same file.
mod common;

use std::collections::BTreeMap;
use std::io::Cursor;

use cadaddy_dwgcodec::entities::Value;
use cadaddy_dwgcodec::entities::records::decode_all;
use cadaddy_dwgcodec::read;

/// A made-up drawing with tables of every kind and common objects.
fn dxf() -> String {
    "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n\
     0\nSECTION\n2\nTABLES\n\
     0\nTABLE\n2\nLTYPE\n70\n2\n\
     0\nLTYPE\n2\nDASHED\n70\n0\n3\n__ __ __\n72\n65\n73\n2\n40\n0.75\n49\n0.5\n74\n0\n49\n-0.25\n74\n0\n\
     0\nLTYPE\n2\nCENTER\n70\n0\n3\n____ _ ____\n72\n65\n73\n4\n40\n2.0\n49\n1.25\n74\n0\n49\n-0.25\n74\n0\n49\n0.25\n74\n0\n49\n-0.25\n74\n0\n0\nENDTAB\n\
     0\nTABLE\n2\nLAYER\n70\n3\n\
     0\nLAYER\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n\
     0\nLAYER\n2\nWALLS\n70\n0\n62\n1\n6\nDASHED\n370\n50\n\
     0\nLAYER\n2\nHIDDEN-OFF\n70\n4\n62\n-3\n6\nCENTER\n290\n0\n\
     0\nLAYER\n2\nFROZEN\n70\n1\n62\n5\n6\nCONTINUOUS\n420\n16744448\n0\nENDTAB\n\
     0\nTABLE\n2\nSTYLE\n70\n2\n\
     0\nSTYLE\n2\nStandard\n70\n0\n40\n0\n41\n1\n50\n0\n71\n0\n42\n2.5\n3\ntxt\n4\n\n\
     0\nSTYLE\n2\nNotes\n70\n0\n40\n3.5\n41\n0.8\n50\n15\n71\n0\n42\n3.5\n3\narial.ttf\n4\n\n0\nENDTAB\n\
     0\nTABLE\n2\nDIMSTYLE\n70\n1\n0\nDIMSTYLE\n105\n0\n2\nStandard\n70\n0\n40\n1\n41\n2.5\n42\n0.625\n44\n1.25\n140\n2.5\n147\n0.625\n77\n1\n271\n2\n0\nDIMSTYLE\n105\n0\n2\nARCH\n70\n0\n40\n48\n41\n0.1875\n140\n0.125\n173\n1\n176\n2\n178\n3\n277\n4\n0\nENDTAB\n\
     0\nTABLE\n2\nUCS\n70\n1\n0\nUCS\n2\nTILTED\n70\n0\n10\n5\n20\n6\n30\n0\n11\n0.707106781186548\n21\n0.707106781186548\n31\n0\n12\n-0.707106781186548\n22\n0.707106781186548\n32\n0\n0\nENDTAB\n\
     0\nTABLE\n2\nVIEW\n70\n1\n0\nVIEW\n2\nPLAN\n70\n0\n40\n50\n10\n25\n20\n20\n41\n80\n11\n0\n21\n0\n31\n1\n12\n0\n22\n0\n32\n0\n42\n50\n43\n0\n44\n0\n50\n0\n71\n0\n0\nENDTAB\n\
     0\nTABLE\n2\nAPPID\n70\n1\n0\nAPPID\n2\nACAD\n70\n0\n0\nAPPID\n2\nMY_APP\n70\n0\n0\nENDTAB\n\
     0\nENDSEC\n\
     0\nSECTION\n2\nENTITIES\n0\nLINE\n8\nWALLS\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n0\nENDSEC\n0\nEOF\n"
        .to_string()
}

fn dwg(id: &str) -> Vec<u8> {
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(dxf().into_bytes())).and_then(|r| r.read()).expect("acadrust reads the DXF");
    doc.version = match id {
        "AC1015" => acadrust::DxfVersion::AC1015,
        "AC1018" => acadrust::DxfVersion::AC1018,
        "AC1024" => acadrust::DxfVersion::AC1024,
        "AC1027" => acadrust::DxfVersion::AC1027,
        _ => acadrust::DxfVersion::AC1032,
    };
    acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG")
}

/// Records of the TABLES and OBJECTS sections of acadrust's DXF for `dwg`, by handle.
fn theirs(dwg: &[u8]) -> BTreeMap<u64, (String, Vec<(i32, String)>)> {
    let doc = acadrust::DwgReader::from_stream(Cursor::new(dwg.to_vec())).read().expect("acadrust reads the DWG");
    let mut out = Vec::new();
    acadrust::DxfWriter::new(&doc).write_to_writer(&mut out).expect("acadrust writes DXF");
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().collect();
    let mut map = BTreeMap::new();
    let (mut section, mut want) = (String::new(), false);
    let mut cur: Option<(String, Vec<(i32, String)>)> = None;
    let flush = |cur: &mut Option<(String, Vec<(i32, String)>)>, map: &mut BTreeMap<u64, (String, Vec<(i32, String)>)>| {
        if let Some((n, g)) = cur.take()
            && let Some(h) = g.iter().find(|x| x.0 == 5 || x.0 == 105).and_then(|x| u64::from_str_radix(x.1.trim(), 16).ok())
        {
            map.insert(h, (n, g));
        }
    };
    for p in lines.chunks(2) {
        let [c, v] = p else { break };
        let code: i32 = c.trim().parse().unwrap_or(-1);
        let v = v.trim_end_matches('\r').to_string();
        if code == 0 {
            flush(&mut cur, &mut map);
            want = v == "SECTION";
            if (section == "TABLES" && v != "TABLE" && v != "ENDTAB") || section == "OBJECTS" {
                cur = Some((v, Vec::new()));
            }
            continue;
        }
        if want && code == 2 {
            section = v.clone();
            want = false;
        }
        if let Some((_, g)) = cur.as_mut() {
            g.push((code, v));
        }
    }
    flush(&mut cur, &mut map);
    map
}

fn same(ours: &Value, theirs: &str) -> bool {
    let t = theirs.trim();
    match ours {
        Value::Str(s) => s.trim().eq_ignore_ascii_case(t),
        Value::Int(i) => t.parse::<i64>().is_ok_and(|x| x == *i),
        Value::Real(f) => t.parse::<f64>().is_ok_and(|x| (x - f).abs() <= 1e-9 * f.abs().max(1.0)),
        Value::Handle(h) => u64::from_str_radix(t, 16).is_ok_and(|x| x == *h),
    }
}

fn default(name: &str, code: i32) -> Vec<&'static str> {
    match (name, code) {
        ("LAYER", 370) => vec!["-3"],
        ("STYLE", 41) => vec!["1"],
        ("DIMSTYLE", 3) => vec!["<>", ""],
        ("DIMSTYLE", 49) => vec!["1", "0"],
        ("DIMSTYLE", 290) => vec!["0"],
        // DIMJOGANG: 45° when stored (R2007+); before, the other reader writes its own default.
        ("DIMSTYLE", 50) => vec!["45", "5"],
        (_, 290) => vec!["1"],
        _ => vec!["0", ""],
    }
}

/// Codes compared: not markers, owners or handles (their layout differs between writers).
fn compared(code: i32) -> bool {
    !matches!(code, 5 | 105 | 100 | 102 | 330 | 360 | 340 | 347 | 390 | 310)
}

#[test]
fn every_decoded_record_matches_acadrusts_dxf_for_it() {
    let mut report = Vec::new();
    for v in common::VERSIONS {
        let bytes = dwg(v);
        let d = read(&bytes).unwrap();
        let (recs, failed) = decode_all(&d).unwrap();
        assert!(failed.is_empty(), "{v}: undecoded {failed:?}");
        let theirs = theirs(&bytes);
        let mut checked: BTreeMap<String, usize> = BTreeMap::new();
        for r in &recs {
            let Some((tname, tg)) = theirs.get(&r.handle) else {
                report.push(format!("{v} {} {:x}: not in acadrust's DXF", r.name, r.handle));
                continue;
            };
            if *tname != r.name {
                report.push(format!("{v} {:x}: {} vs {tname}", r.handle, r.name));
                continue;
            }
            *checked.entry(r.name.clone()).or_default() += 1;
            let mut codes: Vec<i32> = r.groups.iter().map(|g| g.code).chain(tg.iter().map(|g| g.0)).filter(|c| compared(*c)).collect();
            codes.sort_unstable();
            codes.dedup();
            for code in codes {
                // acadrust's DXF has no version (179) or bottom attachment (272) for a
                // multileader style.
                if r.name == "MLEADERSTYLE" && matches!(code, 179 | 272) {
                    continue;
                }
                // A true colour's nearest index (62) needs the palette: the reader uses 420.
                if code == 62 && r.groups.iter().any(|g| g.code == 420) {
                    continue;
                }
                let dflt = default(&r.name, code);
                let a: Vec<&Value> =
                    r.groups.iter().filter(|g| g.code == code).map(|g| &g.value).filter(|x| !dflt.iter().any(|d| same(x, d))).collect();
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
                    report.push(format!(
                        "{v} {} {:x} {:?} group {code}: ours {a:?} theirs {b:?}",
                        r.name,
                        r.handle,
                        tg.iter().find(|g| g.0 == 2).map(|g| g.1.clone())
                    ));
                }
            }
        }
        eprintln!("{v}: checked {checked:?}");
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

/// Damaged record data never makes the decoder panic: every object of a real file, with bytes
/// flipped, cut short or overwritten, decodes or is refused.
#[test]
fn hostile_record_data_never_panics() {
    use cadaddy_dwgcodec::entities::Names;
    use cadaddy_dwgcodec::entities::records::decode;
    for v in ["AC1015", "AC1024"] {
        let bytes = dwg(v);
        let d = read(&bytes).unwrap();
        let classes = d.classes().unwrap();
        let names = Names::read(&d, &classes);
        let mut x = 0x9e37_79b9u32;
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
