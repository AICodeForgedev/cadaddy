//! MARKUPIMPORT: a synthetic annotated PDF placed onto a layout's sheet and through a pair of
//! points, as revision clouds, multileaders, text and polylines on the markup layer, listed and
//! marked done.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

fn m(kind: MarkupKind, points: Vec<[f64; 2]>, rect: [f64; 4], text: &str) -> Markup {
    Markup {
        kind,
        points,
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: "Reviewer".into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    }
}

fn pdf(dir: &std::path::Path) -> String {
    let mut ink = m(MarkupKind::Ink, Vec::new(), [500.0, 500.0, 560.0, 540.0], "");
    ink.strokes = vec![vec![[500.0, 500.0], [520.0, 540.0], [560.0, 510.0]]];
    let markups = vec![
        m(MarkupKind::Cloud, vec![[72.0, 72.0], [144.0, 72.0], [144.0, 144.0], [72.0, 144.0]], [72.0, 72.0, 144.0, 144.0], "Move this wall"),
        m(MarkupKind::Callout, vec![[200.0, 150.0], [250.0, 300.0]], [250.0, 290.0, 400.0, 320.0], "Add outlet"),
        m(MarkupKind::Note, Vec::new(), [600.0, 600.0, 700.0, 640.0], "General note"),
        ink,
    ];
    let p = dir.join("review.pdf");
    std::fs::write(&p, cadaddy_io::pdf_markups::annotated_pdf(1224.0, 792.0, &markups).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

fn hex(v: &Value) -> Handle {
    Handle::parse_hex(v.as_str().unwrap()).unwrap()
}

#[test]
fn markups_land_on_the_sheet_and_are_tracked() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let dir = std::env::temp_dir().join(format!("cadaddy-markup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = pdf(&dir);
    let mut s = Session::new();
    // In a layout: the page is the sheet (inches here: 72 points = 1).
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    // Match the layout paper to the PDF (17×11 in = 279.4×431.8 mm, landscape).
    s.execute("pagesetup", &json!({ "layout": "Layout1", "width": 279.4, "height": 431.8, "landscape": true })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path })).unwrap();
    let placed = r["placed"].as_array().unwrap().clone();
    assert_eq!(placed.len(), 4, "{r}");
    let d = s.doc().unwrap();
    assert!(d.layer("MARKUP").is_some_and(|l| !l.plot), "a markup layer that doesn't plot");
    let cloud = d.entity(hex(&placed[0]["handles"][0])).unwrap();
    let EntityKind::LwPolyline(pl) = &cloud.kind else { panic!("a revision cloud: {:?}", cloud.kind) };
    assert!(pl.closed && pl.vertices.iter().any(|v| v.bulge != 0.0), "bulged arcs");
    assert_eq!(cloud.common.layer, "MARKUP");
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    let lo = xs.iter().copied().fold(f64::MAX, f64::min);
    let origin = d.layouts.iter().find(|l| l.name == "Layout1").unwrap().origin;
    assert!((lo - (origin.x + 1.0)).abs() < 1e-6, "72 points = 1 inch from the sheet's corner: {lo}");
    let leader = d.entity(hex(&placed[1]["handles"][0])).unwrap();
    let EntityKind::MLeader(ml) = &leader.kind else { panic!("a multileader") };
    assert!(ml.text.as_ref().unwrap().contents.contains("Add outlet"));
    assert!(matches!(d.entity(hex(&placed[2]["handles"][0])).unwrap().kind, EntityKind::MText(_)));
    assert!(matches!(d.entity(hex(&placed[3]["handles"][0])).unwrap().kind, EntityKind::LwPolyline(_)));

    // Listed, then marked done.
    let list = s.execute("markups", &json!({})).unwrap();
    let ids: Vec<&str> = list["markups"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["M1", "M2", "M3", "M4"]);
    assert_eq!(list["markups"][0]["text"], "Move this wall");
    assert_eq!(list["markups"][0]["author"], "Reviewer");
    s.execute("markupstatus", &json!({ "id": "M1", "status": "done" })).unwrap();
    assert_eq!(s.execute("markups", &json!({})).unwrap()["markups"][0]["status"], "done");
    assert!(s.execute("markupstatus", &json!({ "id": "M99", "status": "done" })).is_err());

    // In model space, through a pair of points: page (72,72)→(1000,0), (144,72)→(1100,0).
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path, "from": [[72, 72], [144, 72]], "to": [[1000, 0], [1100, 0]] })).unwrap();
    assert_eq!(r["placed"][0]["id"], "M5", "ids continue");
    let EntityKind::LwPolyline(pl) = &s.doc().unwrap().entity(hex(&r["placed"][0]["handles"][0])).unwrap().kind else { panic!() };
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    assert!((xs.iter().copied().fold(f64::MAX, f64::min) - 1000.0).abs() < 1e-6);
    assert!((xs.iter().copied().fold(f64::MIN, f64::max) - 1100.0).abs() < 1e-6);
    assert!(s.execute("markupimport", &json!({ "path": dir.join("none.pdf").to_string_lossy() })).is_err());
    // One undo per import.
    let before = s.execute("markups", &json!({})).unwrap()["markups"].as_array().unwrap().len();
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.execute("markups", &json!({})).unwrap()["markups"].as_array().unwrap().len(), before - 4);
}

/// A PDF written to a temp folder, returning the path.
fn write_pdf(tag: &str, bytes: &[u8]) -> String {
    let dir = std::env::temp_dir().join(format!("cadaddy-markup-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("review.pdf");
    std::fs::write(&p, bytes).unwrap();
    p.to_string_lossy().into_owned()
}

/// A PDF of `markups` on an 11×17 sheet, written to a fresh folder.
fn pdf_of(tag: &str, markups: &[Markup]) -> String {
    let dir = std::env::temp_dir().join(format!("cadaddy-markup-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("review.pdf");
    std::fs::write(&p, cadaddy_io::pdf_markups::annotated_pdf(1224.0, 792.0, markups).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn markup_arrows_on_a_sheet_ignore_dimscale() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let path = pdf_of(
        "pspace",
        &[
            m(MarkupKind::Arrow, vec![[320.0, 150.0], [380.0, 250.0]], [320.0, 150.0, 380.0, 250.0], ""),
            m(MarkupKind::Callout, vec![[200.0, 150.0], [250.0, 300.0]], [250.0, 290.0, 400.0, 320.0], "Add outlet"),
        ],
    );
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "DIMSCALE", "value": 48 })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    s.execute("pagesetup", &json!({ "layout": "Layout1", "width": 279.4, "height": 431.8, "landscape": true })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path })).unwrap();
    let d = s.doc().unwrap();
    for p in r["placed"].as_array().unwrap() {
        let EntityKind::MLeader(ml) = &d.entity(hex(&p["handles"][0])).unwrap().kind else { panic!("a multileader: {p}") };
        assert!((ml.arrow_size - 0.18).abs() < 1e-9, "the style's size on the sheet, not ×48: {}", ml.arrow_size);
        if let Some(t) = &ml.text {
            assert!((t.height - 0.18).abs() < 1e-9, "text {}", t.height);
            assert!((ml.dogleg - 0.36).abs() < 1e-9);
        }
    }
    // In model space at 48 drawing units per page inch: the page's size there.
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path, "scale": 48 })).unwrap();
    let EntityKind::MLeader(ml) = &s.doc().unwrap().entity(hex(&r["placed"][0]["handles"][0])).unwrap().kind else { panic!() };
    assert!((ml.arrow_size - 0.18 * 48.0).abs() < 1e-9, "{}", ml.arrow_size);
}

#[test]
fn sticky_note_text_is_as_wide_as_its_text() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let text = "Verify panel P-9 clearance (36 in. working space).";
    // A 20 pt sticky-note icon, and a text box the reviewer sized.
    let path = pdf_of(
        "sticky",
        &[
            m(MarkupKind::Note, Vec::new(), [600.0, 600.0, 620.0, 620.0], text),
            m(MarkupKind::Note, Vec::new(), [100.0, 600.0, 172.0, 640.0], "A boxed note"),
        ],
    );
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    s.execute("pagesetup", &json!({ "layout": "Layout1", "width": 279.4, "height": 431.8, "landscape": true })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path })).unwrap();
    let d = s.doc().unwrap();
    let EntityKind::MText(mt) = &d.entity(hex(&r["placed"][0]["handles"][0])).unwrap().kind else { panic!("a note") };
    assert_eq!(mt.contents, text);
    // 10 pt text (0.139 in.): every word fits and the 50 characters run on at most two lines.
    let char_w = 6.0 / 72.0;
    assert!(mt.width >= 40.0 * char_w - 1e-9, "wide enough for its text, not the 20 pt icon: {}", mt.width);
    let origin = d.layouts.iter().find(|l| l.name == "Layout1").unwrap().origin;
    assert!((mt.insert.x - (origin.x + 600.0 / 72.0)).abs() < 1e-9 && (mt.insert.y - (origin.y + 620.0 / 72.0)).abs() < 1e-9, "at the icon");
    let EntityKind::MText(boxed) = &d.entity(hex(&r["placed"][1]["handles"][0])).unwrap().kind else { panic!() };
    assert!((boxed.width - 1.0).abs() < 1e-9, "a text box keeps its width: {}", boxed.width);
}

// --- MUST 1: rotated PDF imported onto Layout1, bounding box x AND y ---

#[test]
fn rotated_pdf_cloud_lands_correctly_on_layout() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    // A 792×612 page (landscape letter) with /Rotate 90 → viewed as 612×792 (portrait).
    // Cloud annotation at Rect [72, 72, 144, 144] in unrotated coords.
    // Under Rotate 90: (x,y) → (y, 792−x). So:
    //   (72,72) → (72, 720); (144,144) → (144, 648).
    // Bounding box in viewed coords: x [72,144], y [648,720].
    // On Layout1 (inches, k=1/72): x [1.0, 2.0], y [9.0, 10.0] + layout origin.
    let cloud = m(MarkupKind::Cloud, vec![[72.0, 72.0], [144.0, 72.0], [144.0, 144.0], [72.0, 144.0]], [72.0, 72.0, 144.0, 144.0], "Rotated cloud");
    let bytes = cadaddy_io::pdf_markups::annotated_pdf_ex(792.0, 612.0, &[cloud], Some(90), None).unwrap();
    let path = write_pdf("rot90", &bytes);
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    // Set Layout1 paper to portrait (8.5×11 = 612×792 pt) to match the viewed rotated page.
    s.execute("pagesetup", &json!({ "layout": "Layout1", "landscape": false })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path })).unwrap();
    let placed = r["placed"].as_array().unwrap();
    assert_eq!(placed.len(), 1, "{r}");
    let d = s.doc().unwrap();
    let EntityKind::LwPolyline(pl) = &d.entity(hex(&placed[0]["handles"][0])).unwrap().kind else { panic!("expected cloud") };
    let origin = d.layouts.iter().find(|l| l.name == "Layout1").unwrap().origin;
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    let ys: Vec<f64> = pl.vertices.iter().map(|v| v.p.y).collect();
    let (xmin, xmax) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
    let (ymin, ymax) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
    assert!((xmin - (origin.x + 1.0)).abs() < 1e-6, "xmin: {xmin} vs {}", origin.x + 1.0);
    assert!((xmax - (origin.x + 2.0)).abs() < 1e-6, "xmax: {xmax} vs {}", origin.x + 2.0);
    assert!((ymin - (origin.y + 9.0)).abs() < 1e-6, "ymin: {ymin} vs {}", origin.y + 9.0);
    assert!((ymax - (origin.y + 10.0)).abs() < 1e-6, "ymax: {ymax} vs {}", origin.y + 10.0);
}

// --- MUST 2: sheet mode scaling ---

#[test]
fn half_size_page_scales_to_fit_layout() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    // An 11×17 PDF page (1224×792 pt, but it is 792×1224 since 11" wide × 17" tall).
    // Actually the spec says: "11×17 page (1224×792 pt)" so width=1224 height=792.
    // Layout paper: 36×24 in = 914.4×609.6 mm (landscape) → 609.6 wide × 914.4 tall in portrait,
    // but landscape: effective = 914.4 × 609.6 mm = 36×24 in = 2592×1728 pt.
    // Same aspect (17/11 ≈ 1.545, 36/24 = 1.5)? No — 1224/792 ≈ 1.545, 2592/1728 = 1.5.
    // Those aren't within 1%. Let me use the correct aspect.
    // 11×17: 1224×792 pt. Layout 22×34 in (the same 11:17 aspect ratio scaled 2×).
    // 22×34 in = 558.8×863.6 mm. Landscape: effective_w=863.6 mm, effective_h=558.8 mm.
    // paper_w_pt = 863.6 * 72/25.4 = 2448, paper_h_pt = 558.8 * 72/25.4 = 1584.
    // aspect_page = 1224/792 ≈ 1.5455. aspect_paper = 2448/1584 ≈ 1.5455. Match!
    // scale_factor = min(2448/1224, 1584/792) = min(2.0, 2.0) = 2.0.
    // Cloud at page centre (612, 396). After scaling: (612 * 2.0 / 72, 396 * 2.0 / 72) + origin.
    //   = (17.0, 11.0) + origin. But we need to use the scaled k: k_scaled = k * 2.0 = 2.0/72.
    //   Cloud at (612, 396) → (612 * 2/72, 396 * 2/72) = (17.0, 11.0) + origin.
    let cloud = m(MarkupKind::Cloud, vec![[562.0, 346.0], [662.0, 346.0], [662.0, 446.0], [562.0, 446.0]], [562.0, 346.0, 662.0, 446.0], "Centre cloud");
    let path = pdf_of("halfsize", &[cloud]);
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    // Set the layout paper to 22×34 in (558.8×863.6 mm, landscape).
    s.execute("pagesetup", &json!({ "layout": "Layout1", "width": 558.8, "height": 863.6, "landscape": true })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path })).unwrap();
    assert!(r.get("scaled").is_some(), "result reports scaled: {r}");
    let scale = r["scaled"].as_f64().unwrap();
    assert!((scale - 2.0).abs() < 0.01, "scale ≈ 2.0: {scale}");

    // Verify cloud placement.
    let d = s.doc().unwrap();
    let placed = r["placed"].as_array().unwrap();
    let EntityKind::LwPolyline(pl) = &d.entity(hex(&placed[0]["handles"][0])).unwrap().kind else { panic!() };
    let origin = d.layouts.iter().find(|l| l.name == "Layout1").unwrap().origin;
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    let ys: Vec<f64> = pl.vertices.iter().map(|v| v.p.y).collect();
    let (xmin, xmax) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
    let (ymin, ymax) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
    // Expected: 562 * 2/72 = 15.611, 662 * 2/72 = 18.389, 346 * 2/72 = 9.611, 446 * 2/72 = 12.389
    assert!((xmin - (origin.x + 562.0 * 2.0 / 72.0)).abs() < 1e-6, "xmin scaled: {xmin}");
    assert!((xmax - (origin.x + 662.0 * 2.0 / 72.0)).abs() < 1e-6, "xmax scaled: {xmax}");
    assert!((ymin - (origin.y + 346.0 * 2.0 / 72.0)).abs() < 1e-6, "ymin scaled: {ymin}");
    assert!((ymax - (origin.y + 446.0 * 2.0 / 72.0)).abs() < 1e-6, "ymax scaled: {ymax}");
}

#[test]
fn mismatched_aspect_ratio_errors() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    // 11×17 page (1224×792 pt) onto a layout with different aspect (e.g., 24×36 in).
    // 24×36 in = 609.6×914.4 mm, landscape → effective 914.4×609.6 mm.
    // paper_w_pt = 914.4 * 72/25.4 = 2592, paper_h_pt = 609.6 * 72/25.4 = 1728.
    // aspect_page = 1224/792 ≈ 1.545, aspect_paper = 2592/1728 = 1.5 → doesn't match (>1%).
    let cloud = m(MarkupKind::Cloud, vec![[100.0, 100.0], [200.0, 200.0]], [100.0, 100.0, 200.0, 200.0], "");
    let path = pdf_of("mismatch", &[cloud]);
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    s.execute("pagesetup", &json!({ "layout": "Layout1", "width": 609.6, "height": 914.4, "landscape": true })).unwrap();
    let err = s.execute("markupimport", &json!({ "path": path })).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("from"), "error mentions from/to: {msg}");
}

// --- MUST 3: Model space without placement errors ---

#[test]
fn model_space_without_placement_returns_error() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let cloud = m(MarkupKind::Cloud, vec![[72.0, 72.0], [144.0, 144.0]], [72.0, 72.0, 144.0, 144.0], "test");
    let path = pdf_of("model-err", &[cloud]);
    let mut s = Session::new();
    // Stay in Model space (the default).
    let err = s.execute("markupimport", &json!({ "path": path })).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("from"), "error guides to from/to: {msg}");
    assert!(msg.contains("scale"), "error guides to scale: {msg}");
}

#[test]
fn model_space_with_from_to_places_cloud() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let cloud = m(MarkupKind::Cloud, vec![[72.0, 72.0], [144.0, 72.0], [144.0, 144.0], [72.0, 144.0]], [72.0, 72.0, 144.0, 144.0], "placed");
    let path = pdf_of("model-ok", &[cloud]);
    let mut s = Session::new();
    // Map page (72,72)→(500,500) and (144,72)→(600,500): scale = 100/72 ≈ 1.389.
    let r = s.execute("markupimport", &json!({
        "path": path,
        "from": [[72, 72], [144, 72]],
        "to": [[500, 500], [600, 500]]
    })).unwrap();
    let placed = r["placed"].as_array().unwrap();
    assert_eq!(placed.len(), 1);
    let d = s.doc().unwrap();
    let EntityKind::LwPolyline(pl) = &d.entity(hex(&placed[0]["handles"][0])).unwrap().kind else { panic!() };
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    let ys: Vec<f64> = pl.vertices.iter().map(|v| v.p.y).collect();
    let (xmin, xmax) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
    let (ymin, ymax) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
    // page (72,72) → (500,500), page (144,144) → (600, 600) (uniform scale, no rotation).
    assert!((xmin - 500.0).abs() < 1e-6, "xmin at picked location: {xmin}");
    assert!((xmax - 600.0).abs() < 1e-6, "xmax: {xmax}");
    assert!((ymin - 500.0).abs() < 1e-6, "ymin: {ymin}");
    assert!((ymax - 600.0).abs() < 1e-6, "ymax: {ymax}");
}
