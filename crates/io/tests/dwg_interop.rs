//! DWG interop with independent readers (Q230): what the ODA's File Converter, opened black-box
//! on CADaddy's DWG files, rejected or changed, held here without it.
//!
//! - R2000: an entity at either end of its block stores its null links (the ODA's reader took
//!   "no links" as handle ± 1 and rejected a lone dimension); the viewport entity header control
//!   exists; viewports carry its (null) handle.
//! - R2018: the auxiliary header's maintenance versions are longs.
//! - R2013+: fit splines say they use the knot parameterization that follows.
//! - The bridge's R2000/R2004 image definitions list their reactors (LibreCAD's reader couldn't
//!   read a drawing with an image without them).
//! - Underlays the ODA's converter named PDFREFERENCE read back, from DXF and through the bridge;
//!   records CADaddy keeps in its CADADDY_KEEP dictionary come back from another program's DXF.
//!
//! The compression fixes (LZ77 far copies, the R2007 header's first opcode) are tested in
//! `cadaddy-dwgcodec`.

use cadaddy_doc::{Common, Drawing, EntityKind, Helix, HelixConstrain, Image, Space, Underlay};
use cadaddy_dwgcodec::entities::{self, Obj};
use cadaddy_dwgcodec::header::Value as HV;
use cadaddy_geom::{Vec2, Vec3};

fn dxf(entities: &str) -> Drawing {
    let text = format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n");
    cadaddy_io::read(text.as_bytes(), "made-up.dxf").unwrap()
}

fn native(d: &Drawing, code: &str) -> cadaddy_dwgcodec::Dwg {
    let (bytes, _) = cadaddy_io::dwg_write::write(d, code).unwrap();
    cadaddy_dwgcodec::read(&bytes).unwrap()
}

/// Handles left in an object's handle stream once its common entity data is read.
fn handles_after_common(raw: &cadaddy_dwgcodec::objects::RawObject, v: cadaddy_dwgcodec::Version) -> usize {
    let mut o = Obj::open(raw, v).unwrap();
    entities::common(&mut o).unwrap();
    let mut n = 0;
    while o.href().is_ok() && n < 100 {
        n += 1;
    }
    n
}

#[test]
fn r2000_entities_at_the_ends_of_a_block_store_their_null_links() {
    let lines = "0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n0\n31\n0\n";
    for n in [1, 3] {
        let d = dxf(&lines.repeat(n));
        let dwg = native(&d, "AC1015");
        let (ents, failed) = entities::decode_all(&dwg).unwrap();
        assert!(failed.is_empty(), "{failed:?}");
        let model: Vec<_> = ents.iter().filter(|e| e.name == "LINE").collect();
        assert_eq!(model.len(), n);
        for (k, e) in model.iter().enumerate() {
            // The last entity's next is null, not the handle after it.
            assert_eq!(e.next, model.get(k + 1).map(|m| m.handle), "{n} lines, entity {k}");
        }
    }
}

#[test]
fn r2000_has_a_viewport_entity_header_control() {
    let dwg = native(&dxf(""), "AC1015");
    let header = dwg.header_vars().unwrap();
    let Some(HV::Handle(h)) = header.get("VIEWPORT_ENTITY_HEADER_CONTROL") else { panic!("no VX control handle") };
    let vx = dwg.object(h.value).expect("the VX control object");
    assert_eq!(vx.type_code, 70);
    // Later versions don't have one.
    let dwg = native(&dxf(""), "AC1018");
    assert!(dwg.objects.iter().all(|o| o.type_code != 70));
}

#[test]
fn r2000_viewports_carry_the_viewport_entity_header_handle() {
    let vp = "0\nVIEWPORT\n8\n0\n67\n1\n10\n5\n20\n4\n30\n0\n40\n8\n41\n6\n68\n2\n69\n2\n12\n5\n22\n4\n45\n6\n";
    let mut counts = Vec::new();
    for (code, v) in [("AC1015", cadaddy_dwgcodec::Version::R2000), ("AC1018", cadaddy_dwgcodec::Version::R2004)] {
        let dwg = native(&dxf(vp), code);
        let vps: Vec<_> = dwg.objects.iter().filter(|o| o.type_code == 34).collect();
        assert!(!vps.is_empty(), "{code}: no viewport written");
        counts.push(vps.iter().map(|o| handles_after_common(o, v)).max().unwrap_or(0));
    }
    // Clip boundary, (R2000) viewport entity header, named and base UCS.
    assert_eq!(counts, [4, 3]);
}

#[test]
fn r2018_auxiliary_header_has_long_maintenance_versions() {
    let aux = |v| cadaddy_dwgcodec::Dwg::new_file_sections(v, 10, 0x100, 0).into_iter().find(|s| s.name == "AcDb:AuxHeader").unwrap().data;
    let a = aux(cadaddy_dwgcodec::Version::R2018);
    assert_eq!(a.len(), 129);
    assert_eq!(a.get(3..9), Some(&[0x21, 0, 4, 0, 0, 0][..]), "version 0x21, maintenance RL 4");
    let a = aux(cadaddy_dwgcodec::Version::R2013);
    assert_eq!(a.len(), 123);
    assert_eq!(a.get(3..7), Some(&[0x1F, 0, 105, 0][..]), "version 0x1F, maintenance RS 105");
}

#[test]
fn fit_splines_say_their_knot_parameterization_follows() {
    let spline = "0\nSPLINE\n8\n0\n70\n8\n71\n3\n74\n4\n11\n0\n21\n0\n31\n0\n11\n5\n21\n4\n31\n0\n11\n10\n21\n0\n31\n0\n11\n15\n21\n4\n31\n0\n";
    for (code, v) in [("AC1027", cadaddy_dwgcodec::Version::R2013), ("AC1032", cadaddy_dwgcodec::Version::R2018)] {
        let dwg = native(&dxf(spline), code);
        let raw = dwg.objects.iter().find(|o| o.type_code == 36).expect("a spline");
        let mut o = Obj::open(raw, v).unwrap();
        entities::common(&mut o).unwrap();
        let scenario = o.d.bl().unwrap();
        let flags = o.d.bl().unwrap();
        assert_eq!((scenario, flags), (2, 1 | 8), "{code}");
    }
}

fn underlay() -> Underlay {
    Underlay {
        insert: Vec3::new(10.0, 20.0, 0.0),
        u: Vec3::new(2.0, 0.0, 0.0),
        v: Vec3::new(0.0, 2.0, 0.0),
        size: Vec2::new(17.0, 11.0),
        path: "C:/jobs/site-plan.pdf".into(),
        page: 1,
        name: String::new(),
        clip: Vec::new(),
        clipping: false,
        fade: 25,
        contrast: 50,
        monochrome: false,
        layers_off: Vec::new(),
    }
}

fn underlays(d: &Drawing) -> usize {
    d.model.iter().filter(|e| matches!(e.kind, EntityKind::Underlay(_))).count()
}

#[test]
fn pdfreference_reads_as_an_underlay_from_dxf() {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Underlay(underlay())).unwrap();
    let text = cadaddy_io::write_dxf(&d);
    // As the ODA's converter writes it: the entity (and its class) named PDFREFERENCE.
    let renamed = text.replace("PDFUNDERLAY", "PDFREFERENCE");
    assert!(renamed.contains("PDFREFERENCE") && !renamed.contains("PDFUNDERLAY"));
    let back = cadaddy_io::read(renamed.as_bytes(), "other.dxf").unwrap();
    let EntityKind::Underlay(u) = &back.model.iter().find(|e| matches!(e.kind, EntityKind::Underlay(_))).expect("the underlay").kind else {
        unreachable!()
    };
    assert_eq!(u.path, "C:/jobs/site-plan.pdf", "its definition found");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pdfreference_classes_read_as_underlays_through_the_bridge() {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Underlay(underlay())).unwrap();
    for v in [cadaddy_io::Version::R2000, cadaddy_io::Version::R2004, cadaddy_io::Version::R2007, cadaddy_io::Version::R2018] {
        let bytes = cadaddy_io::write_dwg(&d, v, cadaddy_io::DwgEngine::Bridge).unwrap();
        // As the ODA's converter saves it: the underlay's class named PDFREFERENCE.
        let mut dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
        let mut classes = dwg.classes().unwrap();
        let unicode = dwg.version.unicode();
        for c in classes.classes.iter_mut().filter(|c| c.dxf_name.to_string_lossy() == "PDFUNDERLAY") {
            c.dxf_name = if unicode {
                cadaddy_dwgcodec::bits::Text::Wide("PDFREFERENCE".encode_utf16().collect())
            } else {
                cadaddy_dwgcodec::bits::Text::Bytes(b"PDFREFERENCE".to_vec())
            };
        }
        dwg.set_classes(&classes);
        let other = cadaddy_dwgcodec::write(&dwg).unwrap();
        let back = cadaddy_io::read_dwg(&other, cadaddy_io::DwgEngine::Bridge).unwrap();
        assert_eq!(underlays(&back), 1, "{v:?}");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn records_cadaddy_keeps_come_back_from_another_programs_dxf() {
    // A helix the native writer keeps in its CADADDY_KEEP dictionary (it doesn't encode helices).
    let mut d = Drawing::new_imperial();
    let helix = Helix {
        center: Vec3::ZERO,
        base_radius: 3.0,
        top_radius: 1.0,
        height: 8.0,
        turns: 4.0,
        start_angle: 0.0,
        ccw: true,
        constrain: HelixConstrain::Turns,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Helix(helix)).unwrap();
    let (dwg, report) = cadaddy_io::dwg_write::write(&d, "AC1032").unwrap();
    assert!(report.kept > 0, "the helix is kept, not encoded");
    // Another program reads the DWG and saves DXF: the kept records are an XRECORD's data.
    let other = cadaddy_dwg::dwg_to_dxf(&dwg).unwrap();
    assert!(String::from_utf8_lossy(&other).contains("CADADDY_KEEP"));
    let back = cadaddy_io::read(&other, "other.dxf").unwrap();
    assert_eq!(back.model.iter().filter(|e| matches!(e.kind, EntityKind::Helix(_))).count(), 1);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn bridge_image_definitions_list_their_reactors() {
    let mut d = Drawing::new_imperial();
    let img = Image {
        insert: Vec3::new(1.0, 1.0, 0.0),
        u: Vec3::new(2.0, 0.0, 0.0),
        v: Vec3::new(0.0, 2.0, 0.0),
        size: Vec2::new(2.0, 1.0),
        path: "pictures/site plan.png".into(),
        ..Image::default()
    };
    d.add(&Space::Model, Common::default(), EntityKind::Image(img)).unwrap();
    for v in [cadaddy_io::Version::R2000, cadaddy_io::Version::R2004] {
        let bytes = cadaddy_io::write_dwg(&d, v, cadaddy_io::DwgEngine::Bridge).unwrap();
        let dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
        let classes = dwg.classes().unwrap();
        let class = |n: &str| classes.classes.iter().find(|c| c.dxf_name.to_string_lossy() == n).map(|c| c.number as u16).unwrap();
        let (def, reactor) = (class("IMAGEDEF"), class("IMAGEDEF_REACTOR"));
        let def = dwg.objects.iter().find(|o| o.type_code == def).expect("an image definition");
        let reactor = dwg.objects.iter().find(|o| o.type_code == reactor).expect("its reactor").handle;
        let mut o = Obj::open(def, dwg.version).unwrap();
        let c = entities::object_common(&mut o).unwrap();
        assert!(c.reactors.contains(&reactor), "{v:?}: {:X?}", c.reactors);
        assert!(c.owner.is_some_and(|owner| c.reactors.contains(&owner)), "{v:?}: the image dictionary");
        // Still the same image.
        let back = cadaddy_io::read_dwg(&bytes, cadaddy_io::DwgEngine::Bridge).unwrap();
        let paths: Vec<String> =
            back.model.iter().filter_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.path.clone()) } else { None }).collect();
        assert_eq!(paths, ["pictures/site plan.png"], "{v:?}");
    }
}
