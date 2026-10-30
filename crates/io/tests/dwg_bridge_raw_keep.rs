//! Another program's DWG-only custom records (a custom class's object and entity, which have no
//! DXF groups) read from a DWG on the default bridge path are kept in the drawing as they were
//! stored, not just noted, and a save writes them back with their handles: read, written and read
//! again, the same class names, handles and data bytes are there, in every DWG version, with no
//! "left out" in the read notes (Q5225 MUST 1). The DWGs are made here with CADaddy's own codec
//! from the public ODA specification: the custom records are objects it builds from the common data
//! every object starts with and bytes of its own.
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::Drawing;
use cadaddy_io::{DwgEngine, Version};

#[path = "support/custom_dwg.rs"]
mod custom_dwg;
use custom_dwg::{VERSIONS, Want, kept, synthetic};

fn check_read(id: &str, what: &str, d: &Drawing, want: &Want) {
    assert!(d.read_notes.iter().all(|n| !n.contains("left out")), "{id} {what}: {:?}", d.read_notes);
    assert_eq!(&kept(d), want, "{id} {what}: the drawing holds each record's handle, class name and stored bytes");
}

#[test]
fn dwg_only_custom_records_are_kept_on_the_bridge_read_and_written_back() {
    for id in VERSIONS {
        let (bytes, mut want) = synthetic(id);
        want.sort();
        let version = Version::for_acadver(id).unwrap();

        let d = cadaddy_io::read(&bytes, "made-up.dwg").unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(cadaddy_io::dwg_engine(true), DwgEngine::Bridge, "the default read path is the bridge");
        assert!(d.read_notes.iter().any(|n| n.contains("kept as stored") && n.contains("ACME_WIDGET")), "{id}: {:?}", d.read_notes);
        check_read(id, "read", &d, &want);

        for engine in [DwgEngine::Bridge, DwgEngine::Native] {
            let out = cadaddy_io::write_dwg(&d, version, engine).unwrap_or_else(|e| panic!("{id} {engine:?}: {e}"));
            let back = cadaddy_io::read(&out, "made-up.dwg").unwrap_or_else(|e| panic!("{id} {engine:?}: {e}"));
            check_read(id, &format!("{engine:?} write, read"), &back, &want);
            // A second trip.
            let out2 = cadaddy_io::write_dwg(&back, version, engine).unwrap();
            let back2 = cadaddy_io::read(&out2, "made-up.dwg").unwrap();
            check_read(id, &format!("{engine:?} write, second trip"), &back2, &want);
        }
    }
}
