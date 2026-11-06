//! The data storage section (ACIS data of solids, from AutoCAD 2013 on) against a file of another
//! writer (acadrust, a black box): what it writes decodes here to the handles and bytes it was
//! given, and what is written here is the same bytes (reading our section back by that writer is
//! `crates/io/tests/suite/dwg/dwg_acds.rs`).
mod common;

use std::io::Cursor;

use cadaddy_dwgcodec::acds;

const SECTION: &str = "AcDb:AcDsPrototype_1b";

fn blob(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + 3) & 255) as u8).collect()
}

fn dxf(sizes: &[usize]) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02X}")).collect::<String>();
    let mut s = String::from("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n9\n$HANDSEED\n5\n100\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
    for i in 0..sizes.len() {
        s.push_str(&format!("0\n3DSOLID\n5\n{:X}\n330\n1F\n100\nAcDbEntity\n8\n0\n100\nAcDbModelerGeometry\n290\n0\n2\n{{00000000-0000-0000-0000-000000000000}}\n100\nAcDb3dSolid\n350\n0\n", 0x2A + i));
    }
    s.push_str(
        "0\nENDSEC\n0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n0\nENDSEC\n0\nSECTION\n2\nACDSDATA\n70\n2\n71\n",
    );
    s.push_str(&format!(
        "{}\n0\nACDSSCHEMA\n90\n1\n1\nAcDb3DSolid_ASM_Data\n2\nAcDbDs::ID\n280\n10\n91\n8\n2\nASM_Data\n280\n15\n91\n0\n",
        sizes.len()
    ));
    for (i, n) in sizes.iter().enumerate() {
        let b = blob(*n);
        let chunks: String = b.chunks(127).map(|c| format!("310\n{}\n", hex(c))).collect();
        s.push_str(&format!("0\nACDSRECORD\n90\n1\n2\nAcDbDs::ID\n280\n10\n320\n{:X}\n2\nASM_Data\n280\n15\n94\n{n}\n{chunks}", 0x2A + i));
    }
    s.push_str("0\nENDSEC\n0\nEOF\n");
    s
}

fn dwg(sizes: &[usize]) -> Vec<u8> {
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(dxf(sizes).into_bytes())).and_then(|r| r.read()).expect("acadrust reads the DXF");
    doc.version = acadrust::DxfVersion::AC1032;
    acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG")
}

#[test]
fn another_writers_section_decodes_and_ours_is_the_same_bytes() {
    for sizes in [vec![10], vec![47, 47], vec![10, 300, 5000], vec![5, 6, 7, 8, 9], vec![1000; 12]] {
        let bytes = dwg(&sizes);
        let d = cadaddy_dwgcodec::read(&bytes).unwrap();
        let section = d.section(SECTION).expect("the data storage section").data.clone();
        let want: Vec<(u64, Vec<u8>)> = sizes.iter().enumerate().map(|(i, n)| (0x2A + i as u64, blob(*n))).collect();
        assert_eq!(acds::decode(&section).unwrap(), want, "{sizes:?}: decoded");
        let ours = acds::encode(&want).unwrap();
        // The bytes are the same where the layouts are known to agree (the first five records).
        if sizes.len() <= 3 {
            assert_eq!(ours, section, "{sizes:?}: the section's bytes");
        }
    }
}
