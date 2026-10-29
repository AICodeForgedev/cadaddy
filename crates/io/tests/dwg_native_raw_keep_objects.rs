//! Objects (not entities) CADaddy's own reader doesn't decode — a custom class's data, a proxy,
//! SUN, FIELDLIST, an old fixed type — are no longer dropped without a word (Q5125): the reader
//! keeps them as they were stored, lists them (handle and class name) in its report and its notes,
//! and its writer gives them back: read, written and read again, the same handles, class names
//! and data bytes are there, in every DWG version.
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::Drawing;
use cadaddy_dwgcodec::bits::Text;
use cadaddy_dwgcodec::classes::{Class, Classes};
use cadaddy_dwgcodec::entities::encode::{Handles, ObjWriter, Rec, object_common};
use cadaddy_dwgcodec::entities::records::decode;
use cadaddy_dwgcodec::entities::{Names, RAW_SUBCLASS};

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

/// A drawing whose named object dictionary has five entries, each an XRECORD of another program.
fn base() -> Drawing {
    let mut dxf = String::from(
        "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n0\nENDSEC\n\
         0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n281\n1\n",
    );
    for (k, name) in ["ACME_A", "ACME_B", "ACME_C", "ACME_D", "ACME_E"].iter().enumerate() {
        dxf.push_str(&format!("3\n{name}\n350\n{:X}\n", 0xA10 + k));
    }
    for k in 0..5 {
        dxf.push_str(&format!("0\nXRECORD\n5\n{:X}\n330\nC\n100\nAcDbXrecord\n280\n1\n1\nplaceholder {k}\n", 0xA10 + k));
    }
    dxf.push_str("0\nENDSEC\n0\nEOF\n");
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

fn text(v: cadaddy_dwgcodec::Version, s: &str) -> Text {
    if v >= cadaddy_dwgcodec::Version::R2007 { Text::Wide(s.encode_utf16().collect()) } else { Text::Bytes(s.as_bytes().to_vec()) }
}

/// What the kept objects should be: (handle, DXF name, the object's data as stored).
type Want = Vec<(u64, String, Vec<u8>)>;

/// The handles of the five XRECORDs, by the names of the dictionary entries that hold them (the
/// writer numbers kept objects itself).
fn xrecord_handles(dwg: &cadaddy_dwgcodec::Dwg) -> Vec<u64> {
    use cadaddy_dwgcodec::entities::Value;
    let (recs, _) = cadaddy_dwgcodec::entities::records::decode_all(dwg).unwrap();
    let mut found: Vec<(String, u64)> = Vec::new();
    for r in recs.iter().filter(|r| r.name == "DICTIONARY") {
        let mut name: Option<String> = None;
        for g in &r.groups {
            match (&g.value, g.code) {
                (Value::Str(n), 3) => name = Some(n.clone()),
                (Value::Handle(h), 350 | 360) => {
                    if let Some(n) = name.take().filter(|n| n.starts_with("ACME_")) {
                        found.push((n, *h));
                    }
                }
                _ => {}
            }
        }
    }
    found.sort();
    found.into_iter().map(|(_, h)| h).collect()
}

/// A DWG of `id` in which the five XRECORDs are replaced by objects of classes the codec has no
/// decoder for (ACME_WIDGETDATA, ACAD_PROXY_OBJECT, SUN, FIELDLIST) and one of an old fixed type
/// (LONG_TRANSACTION), made here from the common data every object starts with and bytes of
/// their own.
fn synthetic(id: &str) -> (Vec<u8>, Want) {
    let (bytes, _) = cadaddy_io::dwg_write::write(&base(), id).unwrap();
    let mut dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
    let v = dwg.version;
    let mut classes = dwg.classes().unwrap();
    let number = classes.classes.iter().map(|c| c.number).max().map_or(500, |n| n.max(499) + 1);
    let names_of = ["ACME_WIDGETDATA", "ACAD_PROXY_OBJECT", "SUN", "FIELDLIST"];
    for (k, name) in names_of.iter().enumerate() {
        classes.classes.push(Class {
            number: number + k as i16,
            proxy_flags: 0,
            app_name: text(v, "AcmeApp"),
            cpp_name: text(v, &format!("Acme{k}")),
            dxf_name: text(v, name),
            was_zombie: false,
            item_class_id: 0x1F3,
            instances: 1,
            dwg_version: 0,
            maintenance: 0,
            unknown: [0, 0],
        });
    }
    let classes = Classes::new(v, classes.classes);
    dwg.set_classes(&classes);

    let names = Names::read(&dwg, &classes);
    let xrecords = xrecord_handles(&dwg);
    assert_eq!(xrecords.len(), 5, "{id}: {xrecords:?}");
    let types: Vec<(u16, String)> =
        names_of.iter().enumerate().map(|(k, n)| (number as u16 + k as u16, n.to_string())).chain([(76, "LONG_TRANSACTION".to_string())]).collect();
    let mut want = Want::new();
    for (k, (type_code, name)) in types.into_iter().enumerate() {
        let handle = xrecords[k];
        let at = dwg.objects.iter().position(|o| o.handle == handle).unwrap_or_else(|| panic!("{id}: no object {handle:X}"));
        let classes_now = dwg.classes().unwrap();
        let old = decode(&dwg.objects[at], v, &classes_now, &names).unwrap().expect("the XRECORD");
        let mut w = ObjWriter::new(v, type_code, handle);
        object_common(&mut w, old.owner, &[], None, &Rec(&[]), &Handles::default());
        w.d.bd(1.5 + k as f64);
        for b in 0..(30 + 70 * k) {
            w.d.rc((b * 11 + k * 5) as u8);
        }
        w.text(&format!("{name} payload"));
        let raw = w.finish();
        want.push((handle, name, raw.data.clone()));
        dwg.objects[at] = raw;
    }
    (cadaddy_dwgcodec::write(&dwg).unwrap(), want)
}

/// The raw objects a drawing holds: handle, DXF name and the 310 groups' bytes.
fn kept(d: &Drawing) -> Want {
    let mut out = Want::new();
    for o in &d.foreign.objects {
        if !o.tags.iter().any(|t| t.code == 100 && t.value.trim() == RAW_SUBCLASS) {
            continue;
        }
        let hex: String = o.tags.iter().filter(|t| t.code == 310).map(|t| t.value.trim().to_string()).collect();
        let data: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        out.push((o.handle.0, o.kind.clone(), data));
    }
    out.sort();
    out
}

#[test]
fn undecoded_objects_are_reported_kept_and_written_back_in_every_version() {
    for id in VERSIONS {
        let (bytes, mut want) = synthetic(id);
        want.sort();
        assert_eq!(want.len(), 5);
        let mut expect: Vec<(u64, String)> = want.iter().map(|(h, n, _)| (*h, n.clone())).collect();
        expect.sort();

        // The codec decodes none of the five. Nothing is dropped: the report lists each with its
        // class name, `failed` is empty and the drawing's notes say they are kept.
        let (d, report) = cadaddy_io::dwg_read::read(&bytes).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(report.failed.is_empty(), "{id}: {:?}", report.failed);
        let mut listed = report.kept_objects.clone();
        listed.sort();
        assert_eq!(listed, expect, "{id}: the report lists the kept objects");
        assert!(
            d.read_notes.iter().any(|n| n.contains("5 objects kept as-is") && n.contains("SUN") && n.contains("ACME_WIDGETDATA")),
            "{id}: {:?}",
            d.read_notes
        );
        assert!(d.read_notes.iter().all(|n| !n.contains("left out")), "{id}: {:?}", d.read_notes);
        assert_eq!(kept(&d), want, "{id}: the drawing holds each object's handle, class name and stored bytes");

        // Written by the native writer and read again: the same handles, names and bytes.
        let (out, wr) = cadaddy_io::dwg_write::write(&d, id).unwrap_or_else(|e| panic!("{id}: {e}"));
        let written = cadaddy_dwgcodec::read(&out).unwrap();
        let old_type = want.iter().find(|w| w.1 == "LONG_TRANSACTION").unwrap();
        let object = written.object(old_type.0).filter(|o| o.type_code == 76);
        if id == "AC1015" {
            // R2000 is written the way entities are: every other kept record goes to CADADDY_KEEP.
            assert!(wr.kept >= 5, "{id}: kept {}", wr.kept);
        } else {
            assert_eq!(object.map(|o| &o.data), Some(&old_type.2), "{id}: the fixed-type object is written back with the bytes it had");
            assert!(wr.kept >= 4, "{id}: the four custom classes are kept: {}", wr.kept);
        }
        let (back, report2) = cadaddy_io::dwg_read::read(&out).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(report2.failed.is_empty(), "{id}: {:?}", report2.failed);
        assert_eq!(kept(&back), want, "{id}: kept through the writer");
        let mut listed2 = report2.kept_objects.clone();
        listed2.sort();
        // What the second read lists are the objects written as objects; the rest come back from
        // the keep store, whose records read as the raw objects they were.
        for (h, n) in &expect {
            let in_keep = kept(&back).iter().any(|k| k.0 == *h && &k.1 == n);
            assert!(in_keep || listed2.contains(&(*h, n.clone())), "{id}: {n} {h:X} lost");
        }
    }
}

/// An object of a class the codec decodes whose data is damaged is kept too, and listed with why:
/// it is not dropped and not among the decoded records.
#[test]
fn a_damaged_object_of_a_decoded_class_is_kept_and_listed() {
    for id in ["AC1018", "AC1024"] {
        let (bytes, _) = cadaddy_io::dwg_write::write(&base(), id).unwrap();
        let mut dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
        let v = dwg.version;
        let mut classes = dwg.classes().unwrap();
        let number = classes.classes.iter().map(|c| c.number).max().map_or(500, |n| n.max(499) + 1);
        classes.classes.push(Class {
            number,
            proxy_flags: 0,
            app_name: text(v, "AcDbApp"),
            cpp_name: text(v, "AcDbDictionaryVar"),
            dxf_name: text(v, "DICTIONARYVAR"),
            was_zombie: false,
            item_class_id: 0x1F3,
            instances: 1,
            dwg_version: 0,
            maintenance: 0,
            unknown: [0, 0],
        });
        let classes = Classes::new(v, classes.classes);
        dwg.set_classes(&classes);
        let names = Names::read(&dwg, &classes);
        let first = xrecord_handles(&dwg)[0];
        let at = dwg.objects.iter().position(|o| o.handle == first).unwrap();
        let old = decode(&dwg.objects[at], v, &classes, &names).unwrap().expect("the XRECORD");
        // A DICTIONARYVAR without its fields: the common data and nothing more.
        let mut w = ObjWriter::new(v, number as u16, first);
        object_common(&mut w, old.owner, &[], None, &Rec(&[]), &Handles::default());
        dwg.objects[at] = w.finish();
        let (d, report) = cadaddy_io::dwg_read::read(&cadaddy_dwgcodec::write(&dwg).unwrap()).unwrap();
        assert!(report.failed.is_empty(), "{id}: {report:?}");
        let listed = report.kept_objects.iter().find(|(h, _)| *h == first);
        assert!(listed.is_some_and(|(_, n)| n.starts_with("DICTIONARYVAR: ")), "{id}: {report:?}");
        assert!(d.read_notes.iter().any(|n| n.contains("1 objects kept as-is")), "{id}: {:?}", d.read_notes);
        assert!(kept(&d).iter().any(|k| k.0 == first && k.1 == "DICTIONARYVAR"), "{id}: kept in the drawing");
    }
}
