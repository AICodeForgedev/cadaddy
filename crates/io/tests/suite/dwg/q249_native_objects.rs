//! Q249 object types in a DWG written by CADaddy's own codec. TABLESTYLE, SORTENTSTABLE,
//! MATERIAL, VISUALSTYLE and FIELD are not encoded natively yet: the writer keeps them in its
//! CADADDY_KEEP dictionary, and both readers (the native codec and the acadrust bridge) put them
//! back. These tests hold that path to its values, in all six versions, and check what acadrust
//! itself reads from a file our encoder wrote.

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::material::{Material, Texture};
use cadaddy_doc::*;

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

/// The drawing read back from `bytes` through the native codec and through the bridge.
fn both(bytes: &[u8], v: &str) -> [(&'static str, Drawing); 2] {
    let native = cadaddy_io::read(bytes, "q249.dwg").unwrap_or_else(|e| panic!("{v}: native read: {e}"));
    let (dxf, notes) = cadaddy_dwg::dwg_to_dxf_noted(bytes, cadaddy_dwg::Limits::DEFAULT).unwrap_or_else(|e| panic!("{v}: bridge: {e}"));
    assert!(notes.is_empty(), "{v}: bridge notes: {notes:?}");
    let bridge = cadaddy_io::read(&dxf, "q249.dxf").unwrap_or_else(|e| panic!("{v}: bridge read: {e}"));
    [("native", native), ("bridge", bridge)]
}

/// A MATERIAL comes back with its values from both readers, including those CADaddy's XDATA does
/// not carry (diffuse colour, refraction, description: from the MATERIAL groups themselves).
#[test]
fn material_survives_native_roundtrip() {
    let mut d = Drawing::new_metric();
    d.materials.push(Material {
        name: "Test Glass".into(),
        description: "Made-up glazing".into(),
        color: Rgb(30, 140, 200),
        color_map: Some(Texture::File("glass.png".into())),
        opacity: 0.85,
        refraction: 1.52,
        ..Material::default()
    });
    for v in VERSIONS {
        let (bytes, report) = cadaddy_io::dwg_write::write(&d, v).unwrap();
        assert!(report.skipped.iter().all(|(n, _)| n != "MATERIAL"), "{v}: MATERIAL kept, not left out: {report:?}");
        assert!(report.kept > 0, "{v}: stashed in CADADDY_KEEP: {report:?}");
        for (how, back) in both(&bytes, v) {
            let m = back.materials.iter().find(|m| m.name == "Test Glass").unwrap_or_else(|| panic!("{v} {how}: no material"));
            assert_eq!(m.description, "Made-up glazing", "{v} {how}");
            assert_eq!(m.color, Rgb(30, 140, 200), "{v} {how}: diffuse colour");
            assert!((m.refraction - 1.52).abs() < 1e-9, "{v} {how}: refraction {}", m.refraction);
            assert!((m.opacity - 0.85).abs() < 1e-9, "{v} {how}: opacity {}", m.opacity);
            assert_eq!(m.color_map, Some(Texture::File("glass.png".into())), "{v} {how}: diffuse map");
        }
    }
}

/// A table style that is not the default comes back with its name, margins and a cell style's
/// colour from both readers.
#[test]
fn a_table_style_keeps_its_values() {
    let mut d = Drawing::new_metric();
    let mut style = TableStyle { name: "Schedule".into(), text_height: 3.5, margin: 1.25, title: true, header: false, ..TableStyle::default() };
    style.cell_styles.push(CellStyle {
        name: "Totals".into(),
        text_height: 2.5,
        text_color: Color::Index(1),
        fill: Some(Color::Index(5)),
        margin_h: 0.75,
        margin_v: 0.5,
        ..CellStyle::default()
    });
    d.table_styles.push(style);
    for v in VERSIONS {
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, v).unwrap();
        for (how, back) in both(&bytes, v) {
            let s = back.table_styles.iter().find(|s| s.name == "Schedule").unwrap_or_else(|| panic!("{v} {how}: {:?}", back.table_styles));
            assert!((s.text_height - 3.5).abs() < 1e-9, "{v} {how}: text height {}", s.text_height);
            assert!((s.margin - 1.25).abs() < 1e-9, "{v} {how}: margin {}", s.margin);
            assert!(s.title && !s.header, "{v} {how}: title {} header {}", s.title, s.header);
            let c = s.cell_styles.iter().find(|c| c.name == "Totals").unwrap_or_else(|| panic!("{v} {how}: {:?}", s.cell_styles));
            assert_eq!(c.text_color, Color::Index(1), "{v} {how}");
            assert_eq!(c.fill, Some(Color::Index(5)), "{v} {how}");
            assert!((c.margin_h - 0.75).abs() < 1e-9 && (c.margin_v - 0.5).abs() < 1e-9, "{v} {how}: {c:?}");
        }
    }
}

/// Group code/value pairs of a DXF text.
fn pairs(dxf: &[u8]) -> Vec<(i32, String)> {
    let text = String::from_utf8_lossy(dxf);
    let lines: Vec<&str> = text.lines().collect();
    lines.chunks(2).filter_map(|p| Some((p.first()?.trim().parse().ok()?, p.get(1)?.trim().to_string()))).collect()
}

/// The records of type `name` as their groups.
fn records(pairs: &[(i32, String)], name: &str) -> Vec<Vec<(i32, String)>> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<(i32, String)>> = None;
    for (c, v) in pairs {
        if *c == 0 {
            out.extend(cur.take());
            if v == name {
                cur = Some(Vec::new());
            }
        } else if let Some(r) = cur.as_mut() {
            r.push((*c, v.clone()));
        }
    }
    out.extend(cur);
    out
}

fn get(r: &[(i32, String)], code: i32) -> Option<&str> {
    r.iter().find(|(c, _)| *c == code).map(|(_, v)| v.as_str())
}

fn num(r: &[(i32, String)], code: i32) -> f64 {
    get(r, code).and_then(|v| v.parse().ok()).unwrap_or(f64::NAN)
}

/// A DWG written by our encoder, read by acadrust (the bridge, a black box): the values it reads
/// are the ones written, in every version; the stashed MATERIAL rides in CADADDY_KEEP.
#[test]
fn ours_read_in_acadrust_with_their_values() {
    let dxf = "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n0\nLAYER\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n\
               0\nLAYER\n2\nWALLS\n70\n0\n62\n3\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
               0\nSECTION\n2\nENTITIES\n\
               0\nLINE\n8\nWALLS\n10\n1.5\n20\n2\n30\n0\n11\n9\n21\n7.25\n31\n0\n\
               0\nCIRCLE\n8\n0\n10\n5\n20\n6\n30\n0\n40\n2.5\n\
               0\nTEXT\n8\n0\n10\n1\n20\n30\n30\n0\n40\n2\n1\nRoom 101\n\
               0\nENDSEC\n0\nEOF\n";
    let mut d = cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap();
    d.materials.push(Material { name: "Test Glass".into(), refraction: 1.52, ..Material::default() });
    for v in VERSIONS {
        let (bytes, _) = cadaddy_io::dwg_write::write(&d, v).unwrap();
        let (theirs, notes) = cadaddy_dwg::dwg_to_dxf_noted(&bytes, cadaddy_dwg::Limits::DEFAULT).unwrap_or_else(|e| panic!("{v}: acadrust: {e}"));
        assert!(notes.is_empty(), "{v}: {notes:?}");
        let p = pairs(&theirs);
        let walls = records(&p, "LAYER").into_iter().find(|r| get(r, 2) == Some("WALLS")).unwrap_or_else(|| panic!("{v}: no WALLS layer"));
        assert_eq!(get(&walls, 62), Some("3"), "{v}: layer colour");
        let lines = records(&p, "LINE");
        assert_eq!(lines.len(), 1, "{v}");
        assert_eq!(get(&lines[0], 8), Some("WALLS"), "{v}");
        assert_eq!([10, 20, 11, 21].map(|c| num(&lines[0], c)), [1.5, 2.0, 9.0, 7.25], "{v}: line ends");
        let circles = records(&p, "CIRCLE");
        assert_eq!(circles.len(), 1, "{v}");
        assert_eq!([10, 20, 40].map(|c| num(&circles[0], c)), [5.0, 6.0, 2.5], "{v}: circle");
        let texts = records(&p, "TEXT");
        assert_eq!(texts.len(), 1, "{v}");
        assert_eq!(get(&texts[0], 1), Some("Room 101"), "{v}");
        assert_eq!(num(&texts[0], 40), 2.0, "{v}: text height");
        // Not a MATERIAL record of its own (not encoded natively): kept, its groups intact.
        assert!(p.iter().any(|(c, s)| *c == 3 && s == "CADADDY_KEEP"), "{v}: CADADDY_KEEP dictionary");
        let theirs_text = String::from_utf8_lossy(&theirs);
        assert!(theirs_text.contains("Test Glass") && theirs_text.contains("1.52"), "{v}: the kept MATERIAL's groups");
    }
}
