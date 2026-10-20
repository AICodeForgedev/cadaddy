//! Sample DWG files for the tests, written by acadrust (a black box: only its public
//! DXF-in/DWG-out API is used) from a small made-up drawing.
#![allow(dead_code)]

use std::io::Cursor;

/// A small drawing: a few lines, a circle, an arc, text, a layer of our own.
pub fn dxf(entities: usize) -> String {
    let mut s = String::from(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n\
         0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n70\n2\n\
         0\nLAYER\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n\
         0\nLAYER\n2\nWALLS\n70\n0\n62\n1\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n",
    );
    for i in 0..entities {
        let x = i as f64 * 10.0;
        match i % 4 {
            0 => s.push_str(&format!("0\nLINE\n8\nWALLS\n10\n{x}\n20\n0\n30\n0\n11\n{}\n21\n5\n31\n0\n", x + 8.0)),
            1 => s.push_str(&format!("0\nCIRCLE\n8\n0\n10\n{x}\n20\n10\n30\n0\n40\n2.5\n")),
            2 => s.push_str(&format!("0\nARC\n8\n0\n10\n{x}\n20\n20\n30\n0\n40\n3\n50\n0\n51\n90\n")),
            _ => s.push_str(&format!("0\nTEXT\n8\n0\n10\n{x}\n20\n30\n30\n0\n40\n2\n1\nRoom {i}\n")),
        }
    }
    s.push_str("0\nENDSEC\n0\nEOF\n");
    s
}

pub const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

fn version(id: &str) -> acadrust::DxfVersion {
    match id {
        "AC1015" => acadrust::DxfVersion::AC1015,
        "AC1018" => acadrust::DxfVersion::AC1018,
        "AC1021" => acadrust::DxfVersion::AC1021,
        "AC1024" => acadrust::DxfVersion::AC1024,
        "AC1027" => acadrust::DxfVersion::AC1027,
        _ => acadrust::DxfVersion::AC1032,
    }
}

/// A DWG of `id` with `entities` entities, written by acadrust.
pub fn dwg(id: &str, entities: usize) -> Vec<u8> {
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(dxf(entities).into_bytes())).and_then(|r| r.read()).expect("acadrust reads the DXF");
    doc.version = version(id);
    acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG")
}

/// The handles acadrust reports reading from a DWG (black box: the entity and object handles
/// in the DXF it writes from what it read; group 5, and 105 for DIMSTYLE; the HEADER section's
/// $HANDSEED is skipped).
pub fn acadrust_handles(dwg: &[u8]) -> std::collections::BTreeSet<u64> {
    let doc = acadrust::DwgReader::from_stream(Cursor::new(dwg.to_vec())).read().expect("acadrust reads the DWG");
    let mut out = Vec::new();
    acadrust::DxfWriter::new(&doc).write_to_writer(&mut out).expect("acadrust writes DXF");
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // Past the HEADER section (its first ENDSEC).
    let start = lines.chunks(2).position(|p| p.len() == 2 && p[0] == "0" && p[1] == "ENDSEC").map_or(0, |i| i * 2);
    lines[start..].chunks(2).filter(|p| p.len() == 2 && (p[0] == "5" || p[0] == "105")).filter_map(|p| u64::from_str_radix(p[1], 16).ok()).collect()
}
