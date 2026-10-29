//! An OLE2FRAME (an embedded object's frame and data) read from a file survives a save as DWG:
//! reopened it has the same handle and data bytes, in every DWG version the writer makes (Q6038).
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_io::{DwgEngine, Version};

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

fn data() -> Vec<String> {
    vec!["D0CF11E0A1B11AE1000000FFFF".into(), "00112233445566778899AABBCCDDEEFF".into(), "0A0B0C".into()]
}

fn drawing() -> Drawing {
    let mut dxf = String::from(
        "0\nSECTION\n2\nENTITIES\n\
         0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n\
         0\nOLE2FRAME\n5\n4F2\n8\n0\n100\nAcDbOle2Frame\n70\n2\n10\n6.5\n20\n1.25\n30\n0\n11\n8.5\n21\n0.75\n31\n0\n71\n2\n72\n0\n73\n1\n",
    );
    let chunks = data();
    let len: usize = chunks.iter().map(|c| c.len() / 2).sum();
    dxf.push_str(&format!("90\n{len}\n"));
    for c in &chunks {
        dxf.push_str(&format!("310\n{c}\n"));
    }
    dxf.push_str("1\nOLE_END\n0\nENDSEC\n0\nEOF\n");
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

fn ole(d: &Drawing) -> Option<(u64, String)> {
    d.model.iter().find_map(|e| match &e.kind {
        EntityKind::Unknown(u) if u.dxf_type == "OLE2FRAME" => {
            let hex: String = u.tags.iter().filter(|t| t.code == 310).map(|t| t.value.trim().to_string()).collect();
            Some((e.handle.0, hex))
        }
        _ => None,
    })
}

#[test]
fn an_ole_frame_survives_a_dwg_save_in_every_version() {
    let d = drawing();
    let (handle, hex) = ole(&d).expect("the frame is read");
    assert_eq!(hex, data().concat());
    for id in VERSIONS {
        let version = Version::for_acadver(id).unwrap();
        for engine in [DwgEngine::Bridge, DwgEngine::Native] {
            let bytes = cadaddy_io::write_dwg(&d, version, engine).unwrap();
            let back = cadaddy_io::read(&bytes, "x.dwg").unwrap();
            assert_eq!(ole(&back), Some((handle, hex.clone())), "{id} {engine:?}");
            // And a second generation.
            let again = cadaddy_io::read(&cadaddy_io::write_dwg(&back, version, engine).unwrap(), "y.dwg").unwrap();
            assert_eq!(ole(&again), Some((handle, hex.clone())), "{id} {engine:?} second generation");
        }
    }
}
