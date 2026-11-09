//! A dimension whose block handle is null in a DWG (it has no block and draws itself, as AutoCAD
//! stores dimensional constraints) is read as a dimension with no block by both DWG readers,
//! never as a reference to a block the drawing doesn't have. A dimension with a block keeps it.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_io::DwgEngine;

/// A made-up drawing: one rotated dimension with no block, and one drawn by an anonymous block.
const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1024\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n8\n0\n2\n*D1\n70\n1\n10\n0\n20\n0\n30\n0\n3\n*D1\n0\nLINE\n8\n0\n10\n0\n20\n20\n30\n0\n11\n50\n21\n20\n31\n0\n0\nENDBLK\n8\n0\n\
0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
0\nDIMENSION\n5\nA1\n8\n0\n100\nAcDbEntity\n100\nAcDbDimension\n10\n10\n20\n10\n30\n0\n11\n5\n21\n12\n31\n0\n70\n32\n1\nwidth=10\n3\nStandard\n\
100\nAcDbAlignedDimension\n13\n0\n23\n0\n33\n0\n14\n10\n24\n0\n34\n0\n50\n0\n100\nAcDbRotatedDimension\n\
0\nDIMENSION\n5\nA2\n8\n0\n100\nAcDbEntity\n100\nAcDbDimension\n2\n*D1\n10\n50\n20\n20\n30\n0\n11\n25\n21\n22\n31\n0\n70\n32\n3\nStandard\n\
100\nAcDbAlignedDimension\n13\n0\n23\n15\n33\n0\n14\n50\n24\n15\n34\n0\n50\n0\n100\nAcDbRotatedDimension\n\
0\nENDSEC\n0\nEOF\n";

/// Each dimension's block by handle, in model space.
fn dim_blocks(d: &Drawing) -> Vec<(u64, Option<String>)> {
    let mut out: Vec<(u64, Option<String>)> =
        d.model.iter().filter_map(|e| if let EntityKind::Dimension(dim) = &e.kind { Some((e.handle.0, dim.block.clone())) } else { None }).collect();
    out.sort();
    out
}

/// `data` with the last `from` stored at any bit offset replaced by `to` (as long).
fn replace_last_bits(data: &mut [u8], from: &[u8], to: &[u8]) -> bool {
    let bit = |d: &[u8], i: usize| d[i / 8] >> (7 - i % 8) & 1;
    let n = from.len() * 8;
    for at in (0..(data.len() * 8).saturating_sub(n)).rev() {
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

/// The DWG of [`DXF`], with the first dimension's block handle made null as AutoCAD stores a
/// constraint dimension's: CADaddy's writer gives every dimension a block, so the reference (code
/// 5, the handle's bytes) is rewritten in place to the same length with the value 0.
fn dwg() -> Vec<u8> {
    let d = cadaddy_io::read_dxf(DXF.as_bytes()).unwrap();
    let (bytes, _) = cadaddy_io::dwg_write::write(&d, "AC1024").unwrap();
    let mut file = cadaddy_dwgcodec::read(&bytes).unwrap();
    let both = [0xA1, 0xA2].into_iter().collect();
    let written = cadaddy_dwgcodec::entities::dimension_blocks(&file, &both);
    let Some(Some(block)) = written.get(&0xA1).copied() else { panic!("the first dimension's block as written: {written:?}") };
    let value: Vec<u8> = block.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
    let mut from = vec![0x50 | value.len() as u8];
    from.extend(&value);
    let mut to = vec![0x50 | value.len() as u8];
    to.extend(std::iter::repeat_n(0, value.len()));
    let dim = file.objects.iter_mut().find(|o| o.handle == 0xA1).unwrap();
    assert!(replace_last_bits(&mut dim.data, &from, &to), "the first dimension's block reference");
    let bytes = cadaddy_dwgcodec::write(&file).unwrap();

    let file = cadaddy_dwgcodec::read(&bytes).unwrap();
    let stored = cadaddy_dwgcodec::entities::dimension_blocks(&file, &both);
    assert_eq!(stored.get(&0xA1), Some(&None), "the first dimension's block handle is null in the file");
    assert!(matches!(stored.get(&0xA2), Some(Some(_))), "the second dimension points at its block");
    bytes
}

#[test]
fn null_dimension_block_is_no_block_through_the_bridge() {
    let d = cadaddy_io::read_dwg(&dwg(), DwgEngine::Bridge).unwrap();
    let blocks = dim_blocks(&d);
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0], (0xA1, None), "a null block handle is no block, not a made-up name");
    let named = blocks[1].1.clone().unwrap_or_default();
    assert!(d.block(&named).is_some(), "the second dimension keeps a block the drawing has: {named:?}");
    for (h, b) in &blocks {
        if let Some(b) = b {
            assert!(d.block(b).is_some(), "dimension {h:X} names block {b:?}, which the drawing doesn't have");
        }
    }
}

#[test]
fn null_dimension_block_is_no_block_through_the_codec() {
    let d = cadaddy_io::read_dwg(&dwg(), DwgEngine::Native).unwrap();
    let blocks = dim_blocks(&d);
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0], (0xA1, None));
    assert!(blocks[1].1.as_deref().is_some_and(|b| d.block(b).is_some()), "{blocks:?}");
}
