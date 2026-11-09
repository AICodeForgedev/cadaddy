//! The files a drawing carries inside itself (`Drawing::embedded`: the project data commands keep
//! — object forms, estimate revisions, clash status, reports — and texture images, the baked
//! lightmap) go through every format CADaddy writes: DXF (ASCII and binary, every version but
//! Release 12, which has no objects) and DWG with both writers, saved once and twice.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use cadaddy_doc::*;
use cadaddy_io::{DwgEngine, SaveOptions, Version};

/// A made-up drawing with a line and embedded files of every kind: JSON stores, binary data
/// (every byte value), an empty file, a name with a backslash and non-ASCII characters, and a
/// store bigger than one XRECORD string many times over.
fn drawing() -> Drawing {
    let mut d = Drawing::default();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: cadaddy_geom::Vec3::ZERO, b: cadaddy_geom::Vec3::new(10.0, 5.0, 0.0) }))
        .unwrap();
    let put = |d: &mut Drawing, name: &str, bytes: Vec<u8>| {
        d.embedded.insert(name.into(), Arc::new(bytes));
    };
    put(&mut d, "object-forms.json", br#"{"version":1,"forms":{"door":{"fields":[{"key":"lock_type","type":"text"}]}}}"#.to_vec());
    put(&mut d, "bimwork.json", br#"{"revisions":{"bid1":{"total":1234.5}},"clashes":{"c1":"active"}}"#.to_vec());
    put(&mut d, "breakers.json", (0..40_000u32).map(|i| b'a' + (i % 26) as u8).collect());
    put(&mut d, "lightmap.png", (0..=255u8).cycle().take(5_000).collect());
    put(&mut d, "empty.bin", Vec::new());
    put(&mut d, "texture:maps\\Façade 1.png", b"\x89PNG\r\n\x1a\n\0\0pixels\\u005c".to_vec());
    d
}

fn assert_kept(d: &Drawing, back: &Drawing, what: &str) {
    let names: Vec<&String> = back.embedded.keys().collect();
    assert_eq!(names, d.embedded.keys().collect::<Vec<_>>(), "{what}: the embedded files read back");
    for (n, b) in &d.embedded {
        assert_eq!(back.embedded.get(n).map(|x| x.as_slice()), Some(b.as_slice()), "{what}: `{n}` read back as it was");
    }
}

#[test]
fn dxf_keeps_the_embedded_project_data() {
    let d = drawing();
    for version in [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
        for binary in [false, true] {
            let opts = SaveOptions { version: Some(version), binary };
            let bytes = cadaddy_io::write_with(&d, "p.dxf", &opts).unwrap();
            let back = cadaddy_io::read(&bytes, "p.dxf").unwrap();
            assert_kept(&d, &back, &format!("DXF {version:?} binary {binary}"));
            // Saved again from what was read: still all there, once.
            let again = cadaddy_io::read(&cadaddy_io::write_with(&back, "p.dxf", &opts).unwrap(), "p.dxf").unwrap();
            assert_kept(&d, &again, &format!("DXF {version:?} binary {binary}, saved twice"));
        }
    }
}

#[test]
fn dwg_keeps_the_embedded_project_data_with_either_writer() {
    let d = drawing();
    for engine in [DwgEngine::Bridge, DwgEngine::Native] {
        for version in [Version::R2004, Version::R2010, Version::R2018] {
            let bytes = cadaddy_io::write_dwg(&d, version, engine).unwrap();
            let back = cadaddy_io::read_dwg(&bytes, engine).unwrap();
            assert_kept(&d, &back, &format!("DWG {version:?} {engine:?}"));
            let again = cadaddy_io::read_dwg(&cadaddy_io::write_dwg(&back, version, engine).unwrap(), engine).unwrap();
            assert_kept(&d, &again, &format!("DWG {version:?} {engine:?}, saved twice"));
        }
    }
}

#[test]
fn a_drawing_without_embedded_files_writes_no_record() {
    let mut d = drawing();
    d.embedded.clear();
    let text = cadaddy_io::write_dxf(&d);
    assert!(!text.contains("CADADDY_EMBEDDED"), "no embedded files, no record");
    assert!(cadaddy_io::read_dxf(text.as_bytes()).unwrap().embedded.is_empty());
}

/// Hostile records: damaged JSON, a newer version, bad base64, names with control characters or
/// empty ones are left out without a panic; the good files beside them are read.
#[test]
fn damaged_embedded_records_are_left_out() {
    let d = drawing();
    let text = cadaddy_io::write_dxf(&d);
    let good = "{\"version\":1,\"files\":[{\"name\":\"ok.json\",\"data\":\"e30=\"},{\"name\":\"\",\"data\":\"e30=\"},{\"name\":\"bad\\u0001name\",\"data\":\"e30=\"},{\"name\":\"bad.b64\",\"data\":\"*not base64*\"}]}";
    for (payload, expect) in [
        (good, vec!["ok.json"]),
        ("{\"version\":2,\"files\":[{\"name\":\"ok.json\",\"data\":\"e30=\"}]}", vec![]),
        ("{\"version\":1,\"files\":[{\"name\":", vec![]),
        ("[1,2,3]", vec![]),
    ] {
        // The record itself (the XRECORD holding the files): its strings replaced by the payload.
        let mut doc = String::new();
        let mut replaced = false;
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let is_xrec = lines[i].trim() == "XRECORD" && i > 0 && lines[i - 1].trim() == "0";
            if is_xrec && !replaced {
                // Find the record's strings: the first group 1 after it that begins the payload.
                let mut j = i + 1;
                while j + 1 < lines.len() && lines[j].trim() != "0" {
                    j += 2;
                }
                let block = &lines[i..j];
                if block.iter().any(|l| l.contains("\"files\"")) {
                    doc.push_str("XRECORD\n");
                    let mut k = 1;
                    while k + 1 < block.len() {
                        if block[k].trim() == "1" {
                            k += 2;
                            continue;
                        }
                        doc.push_str(block[k]);
                        doc.push('\n');
                        doc.push_str(block[k + 1]);
                        doc.push('\n');
                        k += 2;
                    }
                    doc.push_str("  1\n");
                    doc.push_str(payload);
                    doc.push('\n');
                    replaced = true;
                    i = j;
                    continue;
                }
            }
            doc.push_str(lines[i]);
            doc.push('\n');
            i += 1;
        }
        assert!(replaced, "the embedded record was found");
        let back = cadaddy_io::read_dxf(doc.as_bytes()).unwrap();
        let names: Vec<&str> = back.embedded.keys().map(String::as_str).collect();
        assert_eq!(names, expect, "payload {payload}");
    }
}
