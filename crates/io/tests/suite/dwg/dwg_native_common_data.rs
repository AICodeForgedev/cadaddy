//! What every entity carries besides its own data — extended data, transparency, a colour book
//! colour, reactors, an extension dictionary — is in the drawing CADaddy's own reader makes from a
//! DWG, as in the drawing the DWG bridge makes, in every version (Q5113).
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::Handle;

fn dxf(late: bool) -> String {
    let (line, circle) = if late { ("430\nPANTONE 123\n440\n33554560\n", "62\n5\n440\n33554434\n420\n255\n") } else { ("", "") };
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n9\n$HANDSEED\n5\n900\n0\nENDSEC\n\
         0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nAPPID\n70\n2\n0\nAPPID\n5\n30\n2\nACAD\n70\n0\n0\nAPPID\n5\n31\n2\nACME_APP\n70\n0\n0\nENDTAB\n\
         0\nTABLE\n2\nLAYER\n70\n1\n0\nLAYER\n5\n40\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n\
         0\nLINE\n5\n100\n102\n{{ACAD_REACTORS\n330\n300\n102\n}}\n102\n{{ACAD_XDICTIONARY\n360\n200\n102\n}}\n330\n1F\n100\nAcDbEntity\n8\n0\n{line}62\n5\n100\nAcDbLine\n10\n0\n20\n0\n30\n0\n11\n5\n21\n5\n31\n0\n\
         1001\nACME_APP\n1000\ntagged\n1040\n2.5\n1070\n42\n\
         0\nCIRCLE\n5\n101\n330\n1F\n100\nAcDbEntity\n8\n0\n{circle}100\nAcDbCircle\n10\n3\n20\n3\n30\n0\n40\n1.5\n\
         1001\nACME_APP\n1000\nsecond\n1070\n7\n\
         0\nENDSEC\n\
         0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n281\n1\n3\nACME\n350\n300\n\
         0\nDICTIONARY\n5\n200\n330\n100\n100\nAcDbDictionary\n281\n1\n3\nACME_DATA\n350\n202\n\
         0\nXRECORD\n5\n202\n330\n200\n100\nAcDbXrecord\n1\nper-line payload\n90\n7\n\
         0\nDICTIONARY\n5\n300\n102\n{{ACAD_REACTORS\n330\n100\n102\n}}\n330\nC\n100\nAcDbDictionary\n281\n1\n0\nENDSEC\n0\nEOF\n"
    )
}

#[test]
fn common_data_is_in_the_native_drawing_as_in_the_bridges() {
    for id in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        // Transparency and colour book colours are stored from R2004 on.
        let mut d = cadaddy_io::read(dxf(id != "AC1015").as_bytes(), "made-up.dxf").unwrap();
        d.header.set_str("ACADVER", id);
        let bytes = cadaddy_io::write(&d, "made-up.dwg").unwrap();
        let bridge = cadaddy_io::read_dwg(&bytes, cadaddy_io::DwgEngine::Bridge).unwrap();
        let native = cadaddy_io::read_dwg(&bytes, cadaddy_io::DwgEngine::Native).unwrap();
        assert!(native.read_notes.iter().all(|n| !n.contains("bridge")), "{id}: {:?}", native.read_notes);
        for h in [0x100u64, 0x101] {
            let find = |dr: &cadaddy_doc::Drawing| dr.model.iter().find(|e| e.handle.0 == h).map(|e| serde_json::to_value(e.as_ref()).unwrap());
            let (b, n) = (find(&bridge), find(&native));
            assert!(b.is_some(), "{id}: the bridge has entity {h:x}");
            assert_eq!(b, n, "{id}: entity {h:x}");
            let extra = |dr: &cadaddy_doc::Drawing| dr.foreign.entities.get(&Handle(h)).cloned();
            assert_eq!(extra(&bridge), extra(&native), "{id}: entity {h:x}'s extended data, reactors and extension dictionary");
        }
        // The line's: really there.
        let line = native.foreign.entities.get(&Handle(0x100)).expect("the line's foreign data");
        assert!(line.xdict.is_some(), "{id}: extension dictionary");
        assert_eq!(line.reactors.len(), 1, "{id}: reactors");
        let first = native.model.iter().find(|e| e.handle.0 == 0x100).unwrap();
        let json = serde_json::to_string(first.as_ref()).unwrap();
        assert!(json.contains("ACME_APP") && json.contains("tagged"), "{id}: {json}");
        if id != "AC1015" {
            assert!(json.contains("33554560") || json.contains("transparen"), "{id}: transparency {json}");
        }
    }
}
