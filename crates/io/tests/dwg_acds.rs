//! The data storage section (the ACIS data of solids, from AutoCAD 2013 on) CADaddy's codec
//! writes is read by the DWG bridge (another writer's reader, a black box) as the records it
//! was given, and the bridge's own section is read by the codec (Q5113).
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_dwgcodec::acds;

const SECTION: &str = "AcDb:AcDsPrototype_1b";

/// Stand-in modeler data of `n` bytes (at least 32): the binary ACIS magic, filler, and the
/// end-of-data marker DWG readers delimit each body with.
fn blob(n: usize) -> Vec<u8> {
    seeded(n, 7)
}

fn seeded(n: usize, seed: usize) -> Vec<u8> {
    let (head, tail) = (b"ACIS BinaryFile\0", b"End-of-ACIS-data");
    let mut v = head.to_vec();
    v.extend((0..n.saturating_sub(head.len() + tail.len())).map(|i| ((i * seed + 3) & 255) as u8));
    v.extend_from_slice(tail);
    v
}

fn dxf(sizes: &[usize]) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02X}")).collect::<String>();
    let mut s = String::from("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n9\n$HANDSEED\n5\n100\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
    for i in 0..sizes.len() {
        s.push_str(&format!("0\n3DSOLID\n5\n{:X}\n330\n1F\n100\nAcDbEntity\n8\n0\n100\nAcDbModelerGeometry\n290\n0\n2\n{{00000000-0000-0000-0000-000000000000}}\n100\nAcDb3dSolid\n350\n0\n", 0x2A + i));
    }
    s.push_str("0\nENDSEC\n0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n0\nENDSEC\n0\nSECTION\n2\nACDSDATA\n70\n2\n71\n");
    s.push_str(&format!("{}\n0\nACDSSCHEMA\n90\n1\n1\nAcDb3DSolid_ASM_Data\n2\nAcDbDs::ID\n280\n10\n91\n8\n2\nASM_Data\n280\n15\n91\n0\n", sizes.len()));
    for (i, n) in sizes.iter().enumerate() {
        let b = blob(*n);
        let chunks: String = b.chunks(127).map(|c| format!("310\n{}\n", hex(c))).collect();
        s.push_str(&format!("0\nACDSRECORD\n90\n1\n2\nAcDbDs::ID\n280\n10\n320\n{:X}\n2\nASM_Data\n280\n15\n94\n{n}\n{chunks}", 0x2A + i));
    }
    s.push_str("0\nENDSEC\n0\nEOF\n");
    s
}

/// The ASM data in the ACDSDATA section of DXF text, by handle.
fn asm_data(text: &str) -> Vec<(u64, Vec<u8>)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let Some(start) = lines.windows(2).position(|w| w == ["2", "ACDSDATA"]) else { return Vec::new() };
    let mut out: Vec<(u64, Vec<u8>)> = Vec::new();
    let (mut handle, mut asm, mut hex) = (None::<u64>, false, String::new());
    for pair in lines[start..].chunks(2) {
        let [code, value] = pair else { break };
        if *code == "0" {
            if let (Some(h), true) = (handle.take(), asm) {
                out.push((h, (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap_or(0)).collect()));
            }
            asm = false;
            hex.clear();
            if *value == "ENDSEC" {
                break;
            }
        }
        match *code {
            "320" => handle = u64::from_str_radix(value, 16).ok(),
            "2" => asm = *value == "ASM_Data",
            "310" if asm => hex.push_str(value),
            _ => {}
        }
    }
    out.sort();
    out
}

#[test]
fn the_bridge_reads_the_section_the_codec_writes() {
    for sizes in [vec![47], vec![47, 47], vec![60, 300, 5000], vec![40, 41, 42, 43, 44], vec![1000; 12]] {
        let d = cadaddy_io::read_dxf(dxf(&sizes).as_bytes()).unwrap();
        let bridge = cadaddy_io::write_dwg(&d, cadaddy_io::Version::R2018, cadaddy_io::DwgEngine::Bridge).unwrap();
        let want: Vec<(u64, Vec<u8>)> = sizes.iter().enumerate().map(|(i, n)| (0x2A + i as u64, blob(*n))).collect();
        // The bridge's own file: the codec reads its section.
        // The bridge reads its own file's section (so this checks reading ours, not a blind spot).
        let own = String::from_utf8(cadaddy_dwg::dwg_to_dxf(&bridge).unwrap()).unwrap();
        let mut own = asm_data(&own);
        own.sort();
        assert_eq!(own, want, "{sizes:?}: the bridge reads its own section");
        let mut file = cadaddy_dwgcodec::read(&bridge).unwrap();
        let section = file.sections.iter_mut().find(|s| s.name == SECTION).expect("the data storage section");
        let mut decoded = acds::decode(&section.data).unwrap();
        decoded.sort();
        assert_eq!(decoded, want, "{sizes:?}: the codec reads the bridge's section");
        // The codec's section, with other bytes in it, in the bridge's file: the bridge reads ours.
        let other: Vec<(u64, Vec<u8>)> = want.iter().map(|(h, b)| (*h, seeded(b.len(), 11))).collect();
        section.data = acds::encode(&other).unwrap();
        let rewritten = cadaddy_dwgcodec::write(&file).unwrap();
        let text = String::from_utf8(cadaddy_dwg::dwg_to_dxf(&rewritten).unwrap()).unwrap();
        assert_eq!(asm_data(&text), other, "{sizes:?}: the bridge reads the codec's section");
    }
}
