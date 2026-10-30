//! A DWG made with CADaddy's own codec from the public ODA specification, holding another
//! program's DWG-only custom records (no DXF groups): a custom class's entity in model space and a
//! custom class's object in the named object dictionary that reacts to a LINE. Shared by the tests
//! that read it through the bridge (cadaddy-io) and check what the exchange report lists
//! (cadaddy-engine).
#![allow(dead_code)]

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_dwgcodec::bits::Text;
use cadaddy_dwgcodec::classes::{Class, Classes};
use cadaddy_dwgcodec::entities::encode::{EntityHead, Handles, ObjWriter, Rec, entity_common, object_common};
use cadaddy_dwgcodec::entities::{Obj, RAW_SUBCLASS, common};
pub const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

/// Two LINEs in model space and, in the named object dictionary, one XRECORD entry.
pub fn base() -> Drawing {
    let dxf = "0\nSECTION\n2\nENTITIES\n\
        0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n\
        0\nLINE\n8\n0\n10\n2\n20\n0\n30\n0\n11\n3\n21\n1\n31\n0\n\
        0\nENDSEC\n\
        0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n281\n1\n3\nACME_WIDGETS\n350\nA10\n\
        0\nXRECORD\n5\nA10\n330\nC\n100\nAcDbXrecord\n280\n1\n1\nplaceholder\n0\nENDSEC\n0\nEOF\n";
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

fn text(v: cadaddy_dwgcodec::Version, s: &str) -> Text {
    if v >= cadaddy_dwgcodec::Version::R2007 { Text::Wide(s.encode_utf16().collect()) } else { Text::Bytes(s.as_bytes().to_vec()) }
}

/// What the kept records should be: (handle, class name, the object's data as stored).
pub type Want = Vec<(u64, String, Vec<u8>)>;

/// A DWG of `id` in which one LINE is an ACME_GADGET entity, the dictionary's XRECORD is an
/// ACME_WIDGET object that reacts to the other LINE, and the file's class table names both.
pub fn synthetic(id: &str) -> (Vec<u8>, Want) {
    let (bytes, _) = cadaddy_io::dwg_write::write(&base(), id).unwrap();
    let mut dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
    let v = dwg.version;
    let mut classes = dwg.classes().unwrap();
    let number = classes.classes.iter().map(|c| c.number).max().map_or(500, |n| n.max(499) + 1);
    for (k, (dxf, cpp, entity)) in [("ACME_GADGET", "AcmeGadget", true), ("ACME_WIDGET", "AcmeWidget", false)].into_iter().enumerate() {
        classes.classes.push(Class {
            number: number + k as i16,
            proxy_flags: 0,
            app_name: text(v, "AcmeApp"),
            cpp_name: text(v, cpp),
            dxf_name: text(v, dxf),
            was_zombie: false,
            item_class_id: if entity { 0x1F2 } else { 0x1F3 },
            instances: 1,
            dwg_version: 0,
            maintenance: 0,
            unknown: [0, 0],
        });
    }
    let classes = Classes::new(v, classes.classes);
    dwg.set_classes(&classes);

    let lines: Vec<u64> = dwg.objects.iter().filter(|o| o.type_code == 19).map(|o| o.handle).collect();
    assert_eq!(lines.len(), 2, "{id}: {lines:?}");
    let mut want = Want::new();

    // The entity: in place of the first LINE, with that line's owner, layer and links.
    let at = dwg.objects.iter().position(|o| o.handle == lines[0]).unwrap();
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
        links: Some((lines[0].checked_sub(1), c.next)),
    };
    let mut w = ObjWriter::new(v, number as u16, lines[0]);
    entity_common(&mut w, &Rec(&[]), &head, &Handles::default());
    w.d.bd(2.5);
    for b in 0..61 {
        w.d.rc((b * 7 + 3) as u8);
    }
    w.text("gadget payload");
    w.href(4, c.layer);
    let raw = w.finish();
    want.push((lines[0], "ACME_GADGET".into(), raw.data.clone()));
    dwg.objects[at] = raw;

    // The object: in place of the dictionary's XRECORD, owned by the dictionary and reacting to
    // the second LINE.
    let xrec = entry_handle(&dwg, "ACME_WIDGETS").unwrap_or_else(|| panic!("{id}: no ACME_WIDGETS entry"));
    let at = dwg.objects.iter().position(|o| o.handle == xrec).unwrap();
    let mut w = ObjWriter::new(v, number as u16 + 1, xrec);
    object_common(&mut w, Some(0xC), &[lines[1]], None, &Rec(&[]), &Handles::default());
    w.d.bd(1.5);
    for b in 0..47 {
        w.d.rc((b * 11 + 5) as u8);
    }
    w.text("widget payload");
    let raw = w.finish();
    want.push((xrec, "ACME_WIDGET".into(), raw.data.clone()));
    dwg.objects[at] = raw;
    (cadaddy_dwgcodec::write(&dwg).unwrap(), want)
}

/// The handle the named object dictionary's entry `name` points to (the writer numbers objects).
fn entry_handle(dwg: &cadaddy_dwgcodec::Dwg, name: &str) -> Option<u64> {
    use cadaddy_dwgcodec::entities::Value;
    let (recs, _) = cadaddy_dwgcodec::entities::records::decode_all(dwg).ok()?;
    for r in recs.iter().filter(|r| r.name == "DICTIONARY") {
        let mut entry: Option<&str> = None;
        for g in &r.groups {
            match (&g.value, g.code) {
                (Value::Str(n), 3) => entry = Some(n),
                (Value::Handle(h), 350 | 360) if entry.take() == Some(name) => return Some(*h),
                _ => {}
            }
        }
    }
    None
}

fn unhex(tags: &[cadaddy_doc::RawTag]) -> Vec<u8> {
    let hex: String = tags.iter().filter(|t| t.code == 310).map(|t| t.value.trim().to_string()).collect();
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect()
}

/// The raw records a drawing holds: handle, class name and the 310 groups' bytes.
pub fn kept(d: &Drawing) -> Want {
    let mut out = Want::new();
    for e in d.model.iter() {
        let EntityKind::Unknown(u) = &e.kind else { continue };
        if u.tags.iter().any(|t| t.code == 100 && t.value.trim() == RAW_SUBCLASS) {
            out.push((e.handle.0, u.dxf_type.clone(), unhex(&u.tags)));
        }
    }
    for o in &d.foreign.objects {
        if o.tags.iter().any(|t| t.code == 100 && t.value.trim() == RAW_SUBCLASS) {
            out.push((o.handle.0, o.kind.clone(), unhex(&o.tags)));
        }
    }
    out.sort();
    out
}
