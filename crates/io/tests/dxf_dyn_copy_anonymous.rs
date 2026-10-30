//! An anonymous copy of a dynamic block (`*U7`, its own name in `AcDbDynamicBlockTrueName`
//! xdata) is not listed as a block, whether the drawing came from a DXF or back from a DWG.

use cadaddy_io::{DwgEngine, Version};

/// A made-up drawing: block `*U7`, a copy of dynamic block "Sample Title", inserted once.
const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1027\n0\nENDSEC\n\
0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nBLOCK_RECORD\n70\n3\n\
0\nBLOCK_RECORD\n5\n1F\n100\nAcDbSymbolTableRecord\n100\nAcDbBlockTableRecord\n2\n*U7\n70\n0\n280\n1\n\
1001\nAcDbDynamicBlockTrueName\n1000\nSample Title\n\
0\nBLOCK_RECORD\n5\n20\n100\nAcDbSymbolTableRecord\n100\nAcDbBlockTableRecord\n2\nPLAIN\n70\n0\n280\n1\n\
0\nENDTAB\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n8\n0\n2\n*U7\n70\n1\n10\n0\n20\n0\n30\n0\n3\n*U7\n0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n5\n21\n0\n31\n0\n0\nENDBLK\n8\n0\n\
0\nBLOCK\n8\n0\n2\nPLAIN\n70\n0\n10\n0\n20\n0\n30\n0\n3\nPLAIN\n0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n0\nENDBLK\n8\n0\n\
0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
0\nINSERT\n8\n0\n2\n*U7\n10\n0\n20\n0\n30\n0\n\
0\nINSERT\n8\n0\n2\nPLAIN\n10\n9\n20\n0\n30\n0\n\
0\nENDSEC\n0\nEOF\n";

/// The names a block list shows: no anonymous block.
fn listed(d: &cadaddy_doc::Drawing) -> Vec<String> {
    let mut v: Vec<String> = d.blocks.iter().filter(|(k, b)| !k.starts_with('*') && !b.anonymous).map(|(k, _)| k.clone()).collect();
    v.sort();
    v
}

#[test]
fn block_lists_agree_between_dxf_and_dwg() {
    let d = cadaddy_io::read_dxf(DXF.as_bytes()).unwrap();
    assert_eq!(listed(&d), ["PLAIN"]);
    for engine in [DwgEngine::Bridge, DwgEngine::Native] {
        let bytes = cadaddy_io::write_dwg(&d, Version::R2018, engine).unwrap();
        for read in [DwgEngine::Bridge, DwgEngine::Native] {
            let back = cadaddy_io::read_dwg(&bytes, read).unwrap();
            assert_eq!(listed(&back), listed(&d), "written by {engine:?}, read by {read:?}");
        }
    }
}
