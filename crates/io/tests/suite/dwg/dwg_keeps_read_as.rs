//! DWG keeps what it was given as it was read (test-coord-battle8: save notices that grew):
//! another program's visual styles in the layout of AutoCAD 2013 and later in a drawing saved as
//! an older DWG, justified text that has no alignment point, and objects drawn in a tilted plane.
//! All of it made-up drawings.
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_doc::*;
use cadaddy_dxf::Version;
use cadaddy_geom::Vec3;

fn text_of(groups: &[(i32, String)]) -> String {
    groups.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

fn g(c: i32, v: impl ToString) -> (i32, String) {
    (c, v.to_string())
}

/// Each property's type in the property-list layout of a visual style: L a whole number (90),
/// D a real (40), C a colour (62), B a flag (290), T a text (1).
const PROPERTY_TYPES: &str = "LLLLDDCLLCCLLDLCDLLLCLLLBLDLBBBBBBBBBLDDLCLLCBLLLBLCDLTBDD";

/// A made-up visual style `name` (handle `h`, owned by dictionary 2A) in that layout: its type, the
/// lighting model and the internal flag, the count, then each property's value and its 176 flag.
fn visual_style(h: u64, name: &str, seed: usize) -> Vec<(i32, String)> {
    let mut out =
        vec![g(0, "VISUALSTYLE"), g(5, format!("{h:X}")), g(330, "2A"), g(100, "AcDbVisualStyle"), g(2, name), g(70, 2), g(177, 3), g(291, 0)];
    out.push(g(70, PROPERTY_TYPES.len()));
    for (i, t) in PROPERTY_TYPES.chars().enumerate() {
        let k = i + seed;
        out.push(match t {
            'L' => g(90, k % 7),
            'D' => g(40, format!("{}.25", k % 5)),
            'C' => g(62, 1 + k % 200),
            'B' => g(290, k % 2),
            _ => g(1, format!("pattern{seed}.tif")),
        });
        out.push(g(176, 1));
    }
    out
}

const STYLES: [&str; 3] = ["Style A", "Style B", "Style C"];

/// A 2010 drawing (AC1024) with three such visual styles in the named object dictionary's
/// ACAD_VISUALSTYLE dictionary.
fn styled_drawing() -> String {
    let mut p = vec![g(0, "SECTION"), g(2, "HEADER"), g(9, "$ACADVER"), g(1, "AC1024"), g(9, "$HANDSEED"), g(5, "400"), g(0, "ENDSEC")];
    p.extend([g(0, "SECTION"), g(2, "CLASSES")]);
    p.extend([g(0, "CLASS"), g(1, "VISUALSTYLE"), g(2, "AcDbVisualStyle"), g(3, "ObjectDBX Classes"), g(90, 4095), g(91, 3), g(280, 0), g(281, 0)]);
    p.extend([g(0, "ENDSEC"), g(0, "SECTION"), g(2, "ENTITIES")]);
    p.extend([
        g(0, "LINE"),
        g(5, "100"),
        g(100, "AcDbEntity"),
        g(8, "0"),
        g(100, "AcDbLine"),
        g(10, 0),
        g(20, 0),
        g(30, 0),
        g(11, 10),
        g(21, 5),
        g(31, 0),
    ]);
    p.extend([g(0, "ENDSEC"), g(0, "SECTION"), g(2, "OBJECTS")]);
    p.extend([g(0, "DICTIONARY"), g(5, "C"), g(330, 0), g(100, "AcDbDictionary"), g(281, 1), g(3, "ACAD_VISUALSTYLE"), g(350, "2A")]);
    p.extend([g(0, "DICTIONARY"), g(5, "2A"), g(330, "C"), g(100, "AcDbDictionary"), g(281, 1)]);
    for (i, n) in STYLES.iter().enumerate() {
        p.extend([g(3, n), g(350, format!("{:X}", 0x2B + i))]);
    }
    for (i, n) in STYLES.iter().enumerate() {
        p.extend(visual_style(0x2B + i as u64, n, i * 3));
    }
    p.extend([g(0, "ENDSEC"), g(0, "EOF")]);
    text_of(&p)
}

/// The visual styles of `d` by name: their groups without the handles a writer renumbers.
fn styles(d: &Drawing) -> Vec<(String, Vec<(i32, String)>)> {
    let mut out: Vec<(String, Vec<(i32, String)>)> = d
        .foreign
        .objects
        .iter()
        .filter(|o| o.kind.eq_ignore_ascii_case("VISUALSTYLE"))
        .map(|o| {
            let name = o.tags.iter().find(|t| t.code == 2).map(|t| t.value.trim().to_string()).unwrap_or_default();
            (name, o.tags.iter().filter(|t| !matches!(t.code, 5 | 102 | 330 | 360)).map(|t| (t.code, t.value.trim().to_string())).collect())
        })
        .collect();
    out.sort();
    out
}

fn as_dwg(d: &Drawing, v: Version) -> Vec<u8> {
    cadaddy_io::write_with(d, "x.dwg", &cadaddy_io::SaveOptions { version: Some(v), binary: false }).unwrap()
}

/// Read as the bridge reads a DWG, without CADaddy putting back what it kept: what other
/// programs see, as groups.
fn as_others_see(dwg: &[u8]) -> Vec<(i32, String)> {
    let text = String::from_utf8_lossy(&cadaddy_dwg::dwg_to_dxf(dwg).unwrap()).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    lines.chunks(2).filter_map(|p| Some((p.first()?.trim().parse().ok()?, p.get(1)?.trim().to_string()))).collect()
}

#[test]
fn visual_styles_of_a_newer_layout_come_back_from_an_older_dwg_as_they_were_read() {
    let d = cadaddy_io::read(styled_drawing().as_bytes(), "styles.dxf").unwrap();
    let want = styles(&d);
    assert_eq!(want.len(), 3, "{want:?}");
    assert!(want.iter().all(|(_, g)| g.len() > 2 * PROPERTY_TYPES.len()), "all 58 properties read");
    for v in [Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
        let dwg = as_dwg(&d, v);
        let back = cadaddy_io::read(&dwg, "x.dwg").unwrap();
        // 2007 and 2010 hold 24 or 28 of the 58 properties: the bridge gave those back alone.
        assert_eq!(styles(&back), want, "{v:?}");
        // Other programs still find the visual styles in the file.
        let seen = as_others_see(&dwg);
        assert_eq!(seen.iter().filter(|(c, v)| *c == 0 && v == "VISUALSTYLE").count(), 3, "{v:?}");
        // Saved again from what was read back: the same.
        let again = cadaddy_io::read(&as_dwg(&back, v), "x.dwg").unwrap();
        assert_eq!(styles(&again), want, "{v:?} second save");
    }
}

#[test]
fn a_visual_style_another_program_changed_stays_as_it_wrote_it() {
    let d = cadaddy_io::read(styled_drawing().as_bytes(), "styles.dxf").unwrap();
    let dwg = as_dwg(&d, Version::R2010);
    // Another program renames one of the styles in the DWG (and writes everything else as it read it).
    let mut seen = as_others_see(&dwg);
    let name = seen.iter_mut().find(|(c, v)| *c == 2 && v == "Style B").expect("the style's name is in the file");
    name.1 = "Style Z".into();
    let renamed: String = seen.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect();
    let theirs = cadaddy_dwg::dxf_to_dwg_as(renamed.as_bytes(), "AC1024").unwrap();
    let back = cadaddy_io::read(&theirs, "x.dwg").unwrap();
    let names: Vec<String> = styles(&back).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["Style A", "Style C", "Style Z"]);
    // The two it left alone come back as they were read.
    let want = styles(&d);
    for (n, groups) in styles(&back).iter().filter(|(n, _)| n != "Style Z") {
        assert_eq!(Some(groups), want.iter().find(|(m, _)| m == n).map(|(_, g)| g), "{n}");
    }
}

/// A made-up drawing of a centred text and a top-left one, neither with an alignment point
/// (group 11), and a left-baseline one.
const LONE_TEXTS: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1024\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n\
0\nTEXT\n5\n101\n100\nAcDbEntity\n8\n0\n100\nAcDbText\n10\n40\n20\n30\n30\n0\n40\n2.5\n1\nCENTRED\n72\n1\n100\nAcDbText\n\
0\nTEXT\n5\n102\n100\nAcDbEntity\n8\n0\n100\nAcDbText\n10\n-15\n20\n8\n30\n0\n40\n1.5\n1\nTOP LEFT\n50\n30\n100\nAcDbText\n73\n3\n\
0\nTEXT\n5\n103\n100\nAcDbEntity\n8\n0\n100\nAcDbText\n10\n5\n20\n6\n30\n0\n40\n1\n1\nPLAIN\n100\nAcDbText\n\
0\nENDSEC\n0\nEOF\n";

fn texts(d: &Drawing) -> Vec<(Handle, Text)> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some((e.handle, t.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn justified_text_without_an_alignment_point_stays_where_it_is_drawn() {
    let d = cadaddy_io::read(LONE_TEXTS.as_bytes(), "texts.dxf").unwrap();
    let want: Vec<(Handle, cadaddy_geom::Vec2)> = texts(&d).iter().map(|(h, t)| (*h, t.justify_point())).collect();
    assert_eq!(want.len(), 3);
    assert!(texts(&d).iter().filter(|(h, _)| h.0 != 0x103).all(|(_, t)| t.align_pt.is_none()), "read without one");
    // A DXF keeps them as they were read; a DWG, which always stores an alignment point, gets the
    // point each is drawn by.
    let dxf = String::from_utf8(cadaddy_io::write(&d, "x.dxf").unwrap()).unwrap();
    let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
    let groups: Vec<(&str, &str)> = lines.chunks(2).filter_map(|p| Some((*p.first()?, *p.get(1)?))).collect();
    let at = groups.iter().position(|g| *g == ("1", "CENTRED")).unwrap();
    let start = groups[..at].iter().rposition(|(c, _)| *c == "0").unwrap();
    let end = at + groups[at..].iter().position(|(c, _)| *c == "0").unwrap();
    let x11 = groups[start..end].iter().find(|(c, _)| *c == "11").map(|(_, v)| v.parse::<f64>().unwrap());
    assert_eq!(x11, None, "{:?}", &groups[start..end]);
    for (name, bytes) in [("x.dxf", dxf.into_bytes()), ("x.dwg", as_dwg(&d, Version::R2010)), ("x.dwg", as_dwg(&d, Version::R2018))] {
        let back = cadaddy_io::read(&bytes, name).unwrap();
        let got: Vec<(Handle, cadaddy_geom::Vec2)> = texts(&back).iter().map(|(h, t)| (*h, t.justify_point())).collect();
        assert_eq!(got.len(), 3, "{name}");
        for ((h, a), (k, b)) in want.iter().zip(&got) {
            assert_eq!(h, k);
            // In a DWG the text went to the origin (the bridge's default alignment point).
            assert!(a.dist(*b) < 1e-9, "{name} {h:?}: {a:?} came back at {b:?}");
        }
    }
}

/// A made-up drawing of objects in a plane tilted about the Y axis (extrusion 0.6, 0, 0.8):
/// a text, a polyline, a block reference, an MTEXT, a point, an ellipse and a hatch.
fn tilted() -> String {
    let n = "210\n0.6\n220\n0\n230\n0.8\n";
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1024\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n5\n20\n100\nAcDbEntity\n8\n0\n100\nAcDbBlockBegin\n2\nB1\n70\n0\n10\n0\n20\n0\n30\n0\n3\nB1\n\
0\nLINE\n5\n22\n100\nAcDbEntity\n8\n0\n100\nAcDbLine\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n0\nENDBLK\n5\n23\n100\nAcDbEntity\n8\n0\n100\nAcDbBlockEnd\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n\
0\nTEXT\n5\n101\n100\nAcDbEntity\n8\n0\n100\nAcDbText\n10\n1\n20\n2\n30\n3\n40\n2\n1\nT\n{n}100\nAcDbText\n\
0\nLWPOLYLINE\n5\n102\n100\nAcDbEntity\n8\n0\n100\nAcDbPolyline\n90\n3\n70\n0\n38\n2\n10\n0\n20\n0\n10\n5\n20\n0\n10\n5\n20\n5\n{n}\
0\nINSERT\n5\n103\n100\nAcDbEntity\n8\n0\n100\nAcDbBlockReference\n2\nB1\n10\n4\n20\n5\n30\n1\n{n}\
0\nMTEXT\n5\n104\n100\nAcDbEntity\n8\n0\n100\nAcDbMText\n10\n1\n20\n2\n30\n0\n40\n2\n1\nMT\n{n}11\n0.8\n21\n0\n31\n-0.6\n\
0\nPOINT\n5\n105\n100\nAcDbEntity\n8\n0\n100\nAcDbPoint\n10\n1\n20\n2\n30\n3\n{n}\
0\nELLIPSE\n5\n106\n100\nAcDbEntity\n8\n0\n100\nAcDbEllipse\n10\n0\n20\n0\n30\n0\n11\n0.8\n21\n0\n31\n-0.6\n{n}40\n0.5\n41\n0\n42\n6.283185307179586\n\
0\nHATCH\n5\n107\n100\nAcDbEntity\n8\n0\n100\nAcDbHatch\n10\n0\n20\n0\n30\n1.5\n{n}2\nSOLID\n70\n1\n71\n0\n91\n1\n92\n2\n72\n0\n73\n1\n93\n3\n10\n0\n20\n0\n10\n4\n20\n0\n10\n4\n20\n4\n97\n0\n75\n0\n76\n1\n98\n0\n\
0\nENDSEC\n0\nEOF\n"
    )
}

#[test]
fn objects_in_a_tilted_plane_keep_it() {
    let d = cadaddy_io::read(tilted().as_bytes(), "tilted.dxf").unwrap();
    let want: Vec<(Handle, Vec3, serde_json::Value)> =
        d.model.iter().map(|e| (e.handle, e.common.extrusion, serde_json::to_value(&e.kind).unwrap())).collect();
    assert_eq!(want.len(), 7);
    assert!(want.iter().all(|(_, n, _)| (n.x - 0.6).abs() < 1e-12 && (n.z - 0.8).abs() < 1e-12), "read in the tilted plane");
    let dxf = cadaddy_io::write(&d, "x.dxf").unwrap();
    for (name, bytes) in [("x.dxf", dxf), ("x.dwg", as_dwg(&d, Version::R2010)), ("x.dwg", as_dwg(&d, Version::R2018))] {
        let back = cadaddy_io::read(&bytes, name).unwrap();
        for (h, n, kind) in &want {
            let e = back.model.get(*h).unwrap_or_else(|| panic!("{name} {h:?} missing"));
            // They came back flat: in the XY plane, with the same coordinates.
            assert!((e.common.extrusion - *n).len() < 1e-9, "{name} {h:?}: extrusion {:?}", e.common.extrusion);
            assert_eq!(serde_json::to_value(&e.kind).unwrap()["type"], kind["type"], "{name} {h:?}");
        }
        let text = |dr: &Drawing| texts(dr).first().map(|(_, t)| t.insert);
        assert_eq!(text(&back), text(&d), "{name}");
    }
}
