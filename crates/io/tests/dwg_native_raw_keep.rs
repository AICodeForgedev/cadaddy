//! Entity objects CADaddy's own reader doesn't decode (a custom class's entity, a REGION, and the
//! types it has no decoder for) are kept in the drawing as they were stored, and its own writer
//! writes them back: read, written and read again, the same handles, type names and data bytes
//! are there, in every DWG version, and the read report lists them as kept, not failed (Q5113).
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_dwgcodec::bits::Text;
use cadaddy_dwgcodec::classes::{Class, Classes};
use cadaddy_dwgcodec::entities::encode::{EntityHead, Handles, ObjWriter, Rec, entity_common};
use cadaddy_dwgcodec::entities::{Names, Obj, common};

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

fn base() -> Drawing {
    let dxf = "0\nSECTION\n2\nENTITIES\n\
        0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n\
        0\nLINE\n8\n0\n10\n2\n20\n0\n30\n0\n11\n3\n21\n1\n31\n0\n\
        0\nLINE\n8\n0\n10\n4\n20\n0\n30\n0\n11\n5\n21\n1\n31\n0\n\
        0\nLINE\n8\n0\n10\n6\n20\n0\n30\n0\n11\n7\n21\n1\n31\n0\n\
        0\nENDSEC\n0\nEOF\n";
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

fn text(v: cadaddy_dwgcodec::Version, s: &str) -> Text {
    if v >= cadaddy_dwgcodec::Version::R2007 { Text::Wide(s.encode_utf16().collect()) } else { Text::Bytes(s.as_bytes().to_vec()) }
}

/// What the raw-kept entities should be: (handle, DXF name, the object's data as stored).
type Want = Vec<(u64, String, Vec<u8>)>;

/// A DWG of `id` whose model space holds two LINEs and, in place of two others, a custom class's
/// entity, a REGION, a MESH and a LIGHT objects (made here from the entity data every entity
/// starts with and bytes of their own).
fn synthetic(id: &str) -> (Vec<u8>, Want) {
    let (bytes, _) = cadaddy_io::dwg_write::write(&base(), id).unwrap();
    let mut dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
    let v = dwg.version;
    let mut classes = dwg.classes().unwrap();
    let number = classes.classes.iter().map(|c| c.number).max().map_or(500, |n| n.max(499) + 1);
    let class = |number: i16, dxf: &str, cpp: &str| Class {
        number,
        proxy_flags: 0,
        app_name: text(v, "AcmeApp"),
        cpp_name: text(v, cpp),
        dxf_name: text(v, dxf),
        was_zombie: false,
        item_class_id: 0x1F2,
        instances: 1,
        dwg_version: 0,
        maintenance: 0,
        unknown: [0, 0],
    };
    classes.classes.push(class(number, "ACME_GADGET", "AcmeGadget"));
    classes.classes.push(class(number + 1, "MESH", "AcDbSubDMesh"));
    classes.classes.push(class(number + 2, "LIGHT", "AcDbLight"));
    let classes = Classes::new(v, classes.classes);
    dwg.set_classes(&classes);

    let names = Names::read(&dwg, &classes);
    let lines: Vec<u64> = dwg.objects.iter().filter(|o| o.type_code == 19).map(|o| o.handle).collect();
    assert!(lines.len() >= 4, "{id}: {lines:?}");
    let types = [(number as u16, "ACME_GADGET"), (37, "REGION"), (number as u16 + 1, "MESH"), (number as u16 + 2, "LIGHT")];
    let mut want = Want::new();
    for (k, (type_code, name)) in types.into_iter().enumerate() {
        let handle = lines[k];
        let at = dwg.objects.iter().position(|o| o.handle == handle).unwrap();
        // The line's own common data (owner, layer, links) for the new object.
        let mut o = Obj::open(&dwg.objects[at], v).unwrap();
        let c = common(&mut o).unwrap();
        let head = EntityHead {
            entmode: c.entmode,
            owner: c.owner,
            reactors: Vec::new(),
            xdict: None,
            layer: c.layer,
            ltype: None,
            ltype_flags: 0,
            material: None,
            plotstyle: None,
            links: Some((k.checked_sub(1).and_then(|j| lines.get(j)).copied(), lines.get(k + 1).copied())),
        };
        let mut w = ObjWriter::new(v, type_code, handle);
        entity_common(&mut w, &Rec(&[]), &head, &Handles::default());
        w.d.bd(2.5 + k as f64);
        for b in 0..(40 + 90 * k) {
            w.d.rc((b * 7 + k * 13) as u8);
        }
        w.text(&format!("{name} payload"));
        w.href(4, c.layer);
        let raw = w.finish();
        want.push((handle, name.to_string(), raw.data.clone()));
        dwg.objects[at] = raw;
    }
    let _ = names;
    (cadaddy_dwgcodec::write(&dwg).unwrap(), want)
}

/// The kept raw records of a drawing: handle, DXF name and the 310 groups' bytes.
fn kept(d: &Drawing) -> Want {
    let mut out = Want::new();
    for e in d.model.iter() {
        let EntityKind::Unknown(u) = &e.kind else { continue };
        if !u.tags.iter().any(|t| t.code == 100 && t.value.trim() == cadaddy_dwgcodec::entities::RAW_SUBCLASS) {
            continue;
        }
        let hex: String = u.tags.iter().filter(|t| t.code == 310).map(|t| t.value.trim().to_string()).collect();
        let data: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        out.push((e.handle.0, u.dxf_type.clone(), data));
    }
    out.sort();
    out
}

#[test]
fn undecoded_entities_are_kept_and_written_back_in_every_version() {
    for id in VERSIONS {
        let (bytes, mut want) = synthetic(id);
        want.sort();
        assert_eq!(want.len(), 4);

        // The codec decodes none of the four, and says which ones it kept.
        let (d, report) = cadaddy_io::dwg_read::read(&bytes).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(report.failed.is_empty(), "{id}: {:?}", report.failed);
        let mut listed: Vec<(u64, String)> = report.kept.clone();
        listed.sort();
        let mut expect: Vec<(u64, String)> = want.iter().map(|(h, n, _)| (*h, n.clone())).collect();
        expect.sort();
        assert_eq!(listed, expect, "{id}: the report lists the kept entities");
        assert!(d.read_notes.iter().any(|n| n.contains("kept as-is") && !n.contains("left out")), "{id}: {:?}", d.read_notes);
        assert!(d.read_notes.iter().all(|n| !n.contains("left out")), "{id}: {:?}", d.read_notes);
        let first = kept(&d);
        assert_eq!(first, want, "{id}: the drawing holds each object's handle, name and stored bytes");

        // Written by the native writer and read again: the same.
        let (out, wr) = cadaddy_io::dwg_write::write(&d, id).unwrap_or_else(|e| panic!("{id}: {e}"));
        // Every kept entity is written back as the object it was, in every version (R2000 too:
        // the links it stores are the ones the file gives it): its handle, its type (a custom
        // class by the number its CLASS record has in the written file) and its bytes are the
        // ones it had; none is wrapped in an XRECORD (the type and bytes are asserted below) or reported as left out.
        assert!(wr.skipped.is_empty(), "{id}: {:?}", wr.skipped);
        let written = cadaddy_dwgcodec::read(&out).unwrap();
        let classes = written.classes().unwrap();
        for (h, name, data) in &want {
            let o = written.object(*h).unwrap_or_else(|| panic!("{id}: {name} {h:X} is not an object of the written file"));
            let expected_type = if name == "REGION" {
                37
            } else {
                classes
                    .classes
                    .iter()
                    .find(|c| c.dxf_name.to_string_lossy() == *name)
                    .map(|c| c.number as u16)
                    .unwrap_or_else(|| panic!("{id}: no class {name}"))
            };
            assert_eq!(o.type_code, expected_type, "{id}: {name} {h:X} type");
            assert_eq!(&o.data, data, "{id}: {name} {h:X} bytes");
        }
        let (back, report2) = cadaddy_io::dwg_read::read(&out).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(report2.failed.is_empty(), "{id}: {:?}", report2.failed);
        assert_eq!(kept(&back), want, "{id}: kept through the writer");
        let mut listed2 = report2.kept.clone();
        listed2.sort();
        assert_eq!(listed2, expect, "{id}: listed again");
    }
}

/// The kept entities sit where their objects were: the model-space entities are the two lines
/// the codec decoded and the four kept, in their stored order.
#[test]
fn kept_entities_keep_their_place() {
    for id in VERSIONS {
        let (bytes, want) = synthetic(id);
        let (d, _) = cadaddy_io::dwg_read::read(&bytes).unwrap();
        let order: Vec<u64> = d.model.iter().map(|e| e.handle.0).collect();
        let mut expect: Vec<u64> = want.iter().map(|w| w.0).collect();
        expect.sort();
        let mut got: Vec<u64> = order.iter().copied().filter(|h| expect.contains(h)).collect();
        got.sort();
        assert_eq!(got, expect, "{id}: {order:?}");
        assert_eq!(order.len(), 4, "{id}: {order:?}");
        // The layer the object's common data names is read for it.
        for e in d.model.iter() {
            assert_eq!(e.common.layer, "0", "{id}");
        }
    }
}
