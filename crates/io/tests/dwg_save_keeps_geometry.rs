//! What a save as DWG used to change in drawings read from other programs' files (Q6038): the
//! extrusion of circles, arcs, solids and traces (a tilted circle lost its plane), MTEXT tabs
//! and line breaks (stored as the caret codes `^I` and `^J`), and the text middle of a dimension
//! whose text is not placed by hand (calculated again, differently from the file's).
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::Vec3;
use cadaddy_io::{DwgEngine, Version};

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

fn groups(g: &[(i32, &str)]) -> String {
    g.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

fn read(entities: &str) -> Drawing {
    let dxf = format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n");
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

/// `d` written as DWG `id` and read back, then written again from that: both generations.
fn generations(d: &Drawing, id: &str) -> Vec<Drawing> {
    let version = Version::for_acadver(id).unwrap();
    let first = cadaddy_io::read(&cadaddy_io::write_dwg(d, version, DwgEngine::Bridge).unwrap(), "a.dwg").unwrap();
    let second = cadaddy_io::read(&cadaddy_io::write_dwg(&first, version, DwgEngine::Bridge).unwrap(), "b.dwg").unwrap();
    vec![first, second]
}

fn get(d: &Drawing, h: u64) -> std::sync::Arc<cadaddy_doc::Entity> {
    d.model.iter().find(|e| e.handle == Handle(h)).unwrap_or_else(|| panic!("{h:X} is gone")).clone()
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.z - b.z).abs() < 1e-9
}

#[test]
fn extrusions_come_back_as_they_were() {
    let tilt = 0.5f64.sqrt().to_string();
    let n = ["210", "0", "220", &tilt, "230", &tilt];
    let down = ["210", "0", "220", "0", "230", "-1"];
    let ext = |e: &[&str; 6]| format!("{}\n{}\n{}\n{}\n{}\n{}\n", e[0], e[1], e[2], e[3], e[4], e[5]);
    let mut s = String::new();
    // A circle in a tilted plane, an arc and a trace under a mirrored one, a line under a mirrored one.
    s += &format!("0\nCIRCLE\n5\n100\n8\n0\n10\n1\n20\n2\n30\n3\n40\n2\n{}", ext(&n));
    s += &format!("0\nARC\n5\n101\n8\n0\n10\n5\n20\n5\n30\n0\n40\n2\n50\n10\n51\n80\n{}", ext(&down));
    s += &format!("0\nTRACE\n5\n102\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n0\n31\n0\n12\n0\n22\n1\n32\n0\n13\n1\n23\n1\n33\n0\n{}", ext(&down));
    s += &format!("0\nLINE\n5\n103\n8\n0\n10\n0\n20\n0\n30\n0\n11\n5\n21\n5\n31\n0\n{}", ext(&down));
    s += &format!("0\nTEXT\n5\n104\n8\n0\n10\n-10\n20\n5\n30\n0\n40\n1\n1\nNote\n{}", ext(&down));
    let d = read(&s);
    let before: Vec<_> = (0x100..=0x104).map(|h| get(&d, h)).collect();
    for id in VERSIONS {
        for (g, back) in generations(&d, id).iter().enumerate() {
            for (i, was) in before.iter().enumerate() {
                let now = get(back, 0x100 + i as u64);
                let what = format!("{id} generation {g} entity {:X}", was.handle.0);
                assert!(near(was.common.extrusion, now.common.extrusion), "{what}: {:?} became {:?}", was.common.extrusion, now.common.extrusion);
                match (&was.kind, &now.kind) {
                    (EntityKind::Circle(a), EntityKind::Circle(b)) => {
                        assert!(near(a.center, b.center) && (a.radius - b.radius).abs() < 1e-9, "{what}");
                    }
                    (EntityKind::Arc(a), EntityKind::Arc(b)) => {
                        assert!(near(a.center, b.center), "{what}");
                        assert!((a.start - b.start).abs() < 1e-9 && (a.end - b.end).abs() < 1e-9, "{what}");
                    }
                    (EntityKind::Trace(a), EntityKind::Trace(b)) => {
                        assert!(a.corners.iter().zip(&b.corners).all(|(p, q)| near(*p, *q)), "{what}: {:?} {:?}", a.corners, b.corners);
                    }
                    _ => {}
                }
            }
        }
    }
    // The tilted circle keeps its plane; a line keeps its extrusion (its points are world ones and
    // the writer writes 210, #2241), so the generations above kept (0, 0, -1).
    assert!(before[0].common.extrusion.y > 0.5 && before[0].common.extrusion.z > 0.5);
    assert!(near(before[3].common.extrusion, Vec3::new(0.0, 0.0, -1.0)), "{:?}", before[3].common.extrusion);
}

#[test]
fn mtext_tabs_and_line_breaks_are_not_stored_as_caret_codes() {
    let s = groups(&[(0, "MTEXT"), (5, "50"), (8, "0"), (10, "0"), (20, "0"), (30, "0"), (40, "1"), (71, "1"), (1, "A^IB^JC^ D")]);
    let d = read(&s);
    let text = |d: &Drawing| match &get(d, 0x50).kind {
        EntityKind::MText(m) => m.contents.clone(),
        k => panic!("{k:?}"),
    };
    assert_eq!(text(&d), "A\tB\nC^D");
    for id in VERSIONS {
        for back in generations(&d, id) {
            let t = text(&back);
            assert!(t.starts_with("A\tB") && t.ends_with("C^D"), "{id}: {t:?}");
            assert!(!t.contains("^I") && !t.contains("^J"), "{id}: {t:?}");
        }
    }
}

#[test]
fn the_text_middle_of_a_dimension_is_the_one_the_file_gave() {
    // A linear dimension whose text the file puts at (4.5, 2.3) while it is not placed by hand.
    let s = groups(&[
        (0, "DIMENSION"),
        (5, "60"),
        (8, "0"),
        (100, "AcDbDimension"),
        (2, "*D1"),
        (10, "5"),
        (20, "2"),
        (30, "0"),
        (11, "4.5"),
        (21, "2.3"),
        (31, "0"),
        (70, "32"),
        (1, "<>"),
        (100, "AcDbAlignedDimension"),
        (13, "0"),
        (23, "0"),
        (33, "0"),
        (14, "10"),
        (24, "0"),
        (34, "0"),
        (100, "AcDbRotatedDimension"),
    ]);
    let d = read(&s);
    let mid = |d: &Drawing| match &get(d, 0x60).kind {
        EntityKind::Dimension(dm) => (dm.text_mid, dm.user_text_pos),
        k => panic!("{k:?}"),
    };
    let (was, user) = mid(&d);
    assert!(!user && near(was, Vec3::new(4.5, 2.3, 0.0)), "{was:?} {user}");
    for id in VERSIONS {
        for back in generations(&d, id) {
            let (now, _) = mid(&back);
            assert!(near(was, now), "{id}: {was:?} became {now:?}");
        }
    }
}

#[test]
fn the_measurement_of_a_dimension_is_the_stored_one() {
    // A radius of 3 that the file measures as 5 (a linear factor of 5/3 in its style): the
    // value is what properties and takeoffs read, so it must not come back worked out again
    // (doubled, for the radial dimensions of a real drawing).
    let radial = groups(&[
        (0, "DIMENSION"),
        (5, "61"),
        (8, "0"),
        (100, "AcDbDimension"),
        (2, "*D2"),
        (10, "0"),
        (20, "0"),
        (30, "0"),
        (11, "2"),
        (21, "1"),
        (31, "0"),
        (70, "36"),
        (42, "5"),
        (100, "AcDbRadialDimension"),
        (15, "3"),
        (25, "0"),
        (35, "0"),
        (40, "0"),
    ]);
    let d = read(&radial);
    let measurement = |d: &Drawing| match &get(d, 0x61).kind {
        EntityKind::Dimension(dm) => dm.measurement,
        k => panic!("{k:?}"),
    };
    assert!((measurement(&d) - 5.0).abs() < 1e-12);
    for id in VERSIONS {
        for back in generations(&d, id) {
            assert!((measurement(&back) - 5.0).abs() < 1e-9, "{id}: {}", measurement(&back));
        }
    }
}
