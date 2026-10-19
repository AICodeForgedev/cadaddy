//! Other programs' custom entities and objects go through DWG as they came: a made-up drawing with
//! a custom class's object in the named object dictionary (reacting to a line), a custom entity in
//! model space and one in a block, written as DWG by CADaddy's writer and read back — once, twice,
//! and at every DWG version — keeps every one of them, their classes, the dictionary entry, the
//! reactor and the extension dictionary, with their groups as they were.
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::*;

const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nACME_WIDGET\n2\nAcmeWidget\n3\nAcmeApp\n90\n0\n280\n0\n281\n0\n0\nCLASS\n1\nACME_GADGET\n2\nAcmeGadget\n3\nAcmeApp\n90\n0\n280\n0\n281\n1\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n5\n20\n8\n0\n2\nPART\n70\n0\n10\n0\n20\n0\n30\n0\n3\nPART\n0\nCIRCLE\n5\n121\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n0\nACME_GADGET\n5\n120\n8\n0\n100\nAcmeGadget\n10\n1\n20\n2\n30\n0\n1\nin a block\n0\nENDBLK\n5\n21\n8\n0\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n0\nLINE\n5\n100\n102\n{ACAD_XDICTIONARY\n360\n200\n102\n}\n102\n{ACAD_REACTORS\n330\n300\n102\n}\n100\nAcDbEntity\n8\n0\n100\nAcDbLine\n10\n0\n20\n0\n30\n0\n11\n10\n21\n5\n31\n0\n1001\nACME_APP\n1000\nwidget-tag\n1070\n42\n\
0\nACME_GADGET\n5\n110\n100\nAcDbEntity\n8\n0\n100\nAcmeGadget\n10\n3\n20\n4\n30\n0\n1\ngadget label\n40\n2.5\n0\nINSERT\n5\n112\n8\n0\n2\nPART\n10\n20\n20\n0\n30\n0\n0\nENDSEC\n\
0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n3\nACME_SETTINGS\n350\n201\n3\nACME_WIDGETS\n350\n300\n0\n\
DICTIONARY\n5\n200\n330\n100\n100\nAcDbDictionary\n3\nACME_DATA\n350\n202\n0\n\
XRECORD\n5\n201\n330\nC\n100\nAcDbXrecord\n1\nsettings payload\n40\n2.5\n0\n\
XRECORD\n5\n202\n330\n200\n100\nAcDbXrecord\n1\nper-line payload\n90\n7\n0\n\
ACME_WIDGET\n5\n300\n102\n{ACAD_REACTORS\n330\n100\n102\n}\n330\nC\n100\nAcmeWidget\n1\nspinner\n90\n7\n40\n1.25\n0\nENDSEC\n0\nEOF\n";

/// What other programs' data there is, without the handles a writer renumbers.
fn custom(d: &Drawing) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>, (bool, usize), Vec<String>) {
    let f = &d.foreign;
    let mut classes: Vec<String> = f.classes.iter().filter_map(|c| c.iter().find(|t| t.code == 1).map(|t| t.value.trim().to_string())).collect();
    classes.retain(|c| c.starts_with("ACME_"));
    classes.sort();
    // Other programs' objects exactly; the dictionaries and XRECORDs the DWG writer also reads
    // itself may gain its default flags (280, 281), which `kept_in` allows.
    let mut objects: Vec<String> =
        f.objects.iter().filter(|o| o.kind.starts_with("ACME_")).map(|o| format!("{} {}", o.kind, groups(&o.tags))).collect();
    objects.sort();
    let mut keys: Vec<String> = f.dictionary.iter().map(|(k, _)| k.clone()).collect();
    keys.sort();
    let line = f.entities.get(&Handle(0x100)).map(|x| (x.xdict.is_some(), x.reactors.len())).unwrap_or_default();
    let gadget = |s: &EntityStore| -> Vec<String> {
        s.iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Unknown(u) => Some(format!("{:X} {} {}", e.handle.0, u.dxf_type, groups(&u.tags))),
                _ => None,
            })
            .collect()
    };
    let in_block = d.blocks.get("PART").map(|b| gadget(&b.entities)).unwrap_or_default();
    (classes, objects, keys, gadget(&d.model), line, in_block)
}

/// A record's groups without the handles and owner a writer renumbers or adds.
fn groups(tags: &[RawTag]) -> String {
    tags.iter().filter(|t| !matches!(t.code, 5 | 102 | 320..=369)).map(|t| format!("{}={}", t.code, t.value.trim())).collect::<Vec<_>>().join(",")
}

/// Every group of each of `a`'s other objects (dictionaries, XRECORDs) is in one of `b`'s.
fn kept_in(a: &Drawing, b: &Drawing) -> bool {
    let set = |o: &cadaddy_doc::foreign::ForeignObject| -> Vec<String> { groups(&o.tags).split(',').map(str::to_string).collect() };
    a.foreign.objects.iter().filter(|o| !o.kind.starts_with("ACME_")).all(|o| {
        let want = set(o);
        b.foreign.objects.iter().any(|p| p.kind == o.kind && want.iter().all(|g| set(p).contains(g)))
    })
}

fn through_dwg(d: &Drawing, version: Option<cadaddy_dxf::Version>) -> Drawing {
    let dwg = cadaddy_io::write_with(d, "x.dwg", &cadaddy_io::SaveOptions { version, binary: false }).unwrap();
    let back = cadaddy_io::read(&dwg, "x.dwg").unwrap();
    assert!(back.read_notes.is_empty(), "{:?}", back.read_notes);
    back
}

#[test]
fn custom_records_go_through_dwg_as_they_came() {
    let d = cadaddy_io::read(DXF.as_bytes(), "acme.dxf").unwrap();
    let want = custom(&d);
    // The made-up drawing has them all, read from the DXF.
    assert_eq!(want.0, ["ACME_GADGET", "ACME_WIDGET"]);
    assert_eq!(want.1, ["ACME_WIDGET 100=AcmeWidget,1=spinner,90=7,40=1.25"]);
    assert_eq!(want.2, ["ACME_SETTINGS", "ACME_WIDGETS"]);
    assert_eq!(want.3.len(), 1, "{:?}", want.3);
    assert_eq!(want.4, (true, 1));
    assert_eq!(want.5.len(), 1, "{:?}", want.5);

    let once = through_dwg(&d, None);
    assert_eq!(custom(&once), want, "through DWG once");
    assert!(kept_in(&d, &once));
    let twice = through_dwg(&once, None);
    assert_eq!(custom(&twice), want, "through DWG twice");
    // CADaddy's own bookkeeping never shows in the drawing.
    assert!(!once.foreign.dictionary.iter().any(|(k, _)| k.contains("CADADDY_KEEP")));
    // And back out as DXF: the records as the DXF had them.
    let dxf = String::from_utf8(cadaddy_io::write(&twice, "x.dxf").unwrap()).unwrap();
    for g in ["ACME_WIDGET", "AcmeWidget", "spinner", "ACME_GADGET", "gadget label", "in a block", "ACME_WIDGETS"] {
        assert!(dxf.contains(g), "{g} missing from the DXF written after two DWG trips");
    }
}

#[test]
fn every_dwg_version_keeps_them() {
    let d = cadaddy_io::read(DXF.as_bytes(), "acme.dxf").unwrap();
    let want = custom(&d);
    for v in [
        cadaddy_dxf::Version::R2000,
        cadaddy_dxf::Version::R2004,
        cadaddy_dxf::Version::R2007,
        cadaddy_dxf::Version::R2010,
        cadaddy_dxf::Version::R2013,
        cadaddy_dxf::Version::R2018,
    ] {
        let back = through_dwg(&d, Some(v));
        assert_eq!(custom(&back), want, "{v:?}");
        assert!(kept_in(&d, &back), "{v:?}");
    }
}

#[test]
fn a_drawing_without_custom_data_is_written_as_before() {
    // No kept dictionary in an ordinary drawing's DWG.
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: cadaddy_geom::Vec3::ZERO, b: cadaddy_geom::Vec3::new(1.0, 1.0, 0.0) }))
        .unwrap();
    let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
    let dxf = cadaddy_dwg::dwg_to_dxf(&dwg).unwrap();
    assert!(!String::from_utf8_lossy(&dxf).contains("CADADDY_KEEP"));
}
