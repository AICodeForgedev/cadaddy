//! AutoCAD stores every anonymous block of a kind under one name (`*U`, `*D`, `*T`…) and numbers
//! them when it loads the file. CADaddy's DWG reader tells them apart the same way for the block
//! table and for the references to them, so each block reference draws its own block.

use cadaddy_doc::EntityKind;

/// A made-up drawing: two anonymous blocks (a line, a circle), each inserted once.
const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n8\n0\n2\n*U1\n70\n1\n10\n0\n20\n0\n30\n0\n3\n*U1\n0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n5\n21\n0\n31\n0\n0\nENDBLK\n8\n0\n\
0\nBLOCK\n8\n0\n2\n*U2\n70\n1\n10\n0\n20\n0\n30\n0\n3\n*U2\n0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n2\n0\nENDBLK\n8\n0\n\
0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
0\nINSERT\n8\n0\n2\n*U1\n10\n0\n20\n0\n30\n0\n\
0\nINSERT\n8\n0\n2\n*U2\n10\n20\n20\n0\n30\n0\n\
0\nENDSEC\n0\nEOF\n";

/// `data` with the first `from` stored at any bit offset (strings in R2000 objects are bytes
/// in the bit stream, most significant bit first) replaced by `to` (as long).
fn replace_bits(data: &mut [u8], from: &[u8], to: &[u8]) -> bool {
    let bit = |d: &[u8], i: usize| d[i / 8] >> (7 - i % 8) & 1;
    let n = from.len() * 8;
    for at in 0..(data.len() * 8).saturating_sub(n) {
        if (0..n).all(|k| bit(data, at + k) == bit(from, k)) {
            for k in 0..n {
                let (i, m) = ((at + k) / 8, 1u8 << (7 - (at + k) % 8));
                if bit(to, k) == 1 { data[i] |= m } else { data[i] &= !m }
            }
            return true;
        }
    }
    false
}

#[test]
fn anonymous_blocks_stored_under_one_name_stay_apart() {
    let d = cadaddy_io::read_dxf(DXF.as_bytes()).unwrap();
    let (bytes, _) = cadaddy_io::dwg_write::write(&d, "AC1015").unwrap();
    // Store both block records as `*U1`, as AutoCAD would store them both as `*U`.
    let mut dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
    let patched = dwg.objects.iter_mut().filter(|o| o.type_code == 49).any(|o| replace_bits(&mut o.data, b"*U2", b"*U1"));
    assert!(patched, "the second block record's name");
    let bytes = cadaddy_dwgcodec::write(&dwg).unwrap();

    let (back, report) = cadaddy_io::dwg_read::read(&bytes).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let refs: Vec<String> =
        back.model.iter().filter_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.block.clone()) } else { None }).collect();
    assert_eq!(refs.len(), 2);
    assert_ne!(refs[0], refs[1], "each reference names its own block");
    let drawn: Vec<&str> = refs.iter().map(|n| back.blocks[n].entities.iter().next().map_or("", |e| e.kind.dxf_name())).collect();
    assert_eq!(drawn, ["LINE", "CIRCLE"]);
}
