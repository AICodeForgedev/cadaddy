//! Redline: SEND TO REDLINE plots to one PDF (opened only when Redline is installed), IMPORT
//! REDLINE MARKUPS brings each reviewer's markups back on their own layer, matching each page to
//! the layout it came from.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_doc::{EntityKind, Space};
use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io, set_pdf_merge};
use cadaddy_engine::doc::Handle;
use serde_json::json;

fn mk(kind: MarkupKind, author: &str, rect: [f64; 4], text: &str, points: Vec<[f64; 2]>) -> Markup {
    Markup {
        kind,
        points,
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: author.into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    }
}

fn note(author: &str, rect: [f64; 4], text: &str) -> Markup {
    mk(MarkupKind::Note, author, rect, text, Vec::new())
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-redline-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    cadaddy_engine::cmd::redline::set_page_count(cadaddy_io::pdf_markups::page_count);
    cadaddy_engine::cmd::redline::set_bookmarks(cadaddy_io::pdf_markups::bookmark_titles);
}

#[test]
fn each_reviewer_gets_a_layer() {
    hooks();
    let d = dir("import");
    let markups = vec![
        note("Reviewer A", [72.0, 72.0, 144.0, 100.0], "Move this"),
        note("Reviewer B", [200.0, 200.0, 300.0, 230.0], "Check that"),
        note("", [400.0, 400.0, 500.0, 430.0], "Anonymous"),
    ];
    let p = d.join("review.pdf");
    std::fs::write(&p, cadaddy_io::pdf_markups::annotated_pdf(1224.0, 792.0, &markups).unwrap()).unwrap();
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    // Single-page: pass page explicitly to use the single-page path.
    let r = s.execute("redline.import", &json!({ "path": p.to_string_lossy(), "page": 1 })).unwrap();
    let placed = r["placed"].as_array().unwrap();
    let layers: Vec<&str> = placed.iter().map(|x| x["layer"].as_str().unwrap()).collect();
    assert_eq!(layers, ["MARKUP-Reviewer A", "MARKUP-Reviewer B", "MARKUP"], "{r}");
    let doc = s.doc().unwrap();
    for (x, name) in placed.iter().zip(&layers) {
        assert!(doc.layer(name).is_some_and(|l| !l.plot), "{name}: a layer that doesn't plot");
        let h = Handle::parse_hex(x["handles"][0].as_str().unwrap()).unwrap();
        assert_eq!(doc.entity(h).unwrap().common.layer, *name);
    }
    // Plain MARKUPIMPORT keeps the one layer.
    let r = s.execute("markupimport", &json!({ "path": p.to_string_lossy() })).unwrap();
    assert!(r["placed"].as_array().unwrap().iter().all(|x| x["layer"] == "MARKUP"), "{r}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn send_saves_the_pdf_when_redline_is_not_installed() {
    hooks();
    let d = dir("send");
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [100, 50]] })).unwrap();
    let out = d.join("sheet.pdf");
    let r = s.execute("redline", &json!({ "path": out.to_string_lossy() })).unwrap();
    assert_eq!(r["opened"], false, "no opener installed in tests: {r}");
    assert_eq!(r["path"], out.to_string_lossy().as_ref());
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "a PDF");
    // Hostile params don't panic.
    for p in [json!({ "path": 5 }), json!({ "layouts": "x" }), json!(null)] {
        let _ = s.execute("redline", &p);
        let _ = s.execute("redline.import", &p);
    }
    std::fs::remove_dir_all(&d).ok();
}

/// MUST 3: round-trip test — drawing with Layout1+Layout2, send, add annotations on both pages
/// (two authors, a cloud and a note), import, verify markups on the correct layouts with correct
/// entity kinds, text, and positions.
#[test]
fn round_trip_two_layouts() {
    hooks();
    let d = dir("roundtrip");
    let mut s = Session::new();

    // Ensure Layout2 exists (may already be in the default document).
    let _ = s.execute("layout", &json!({ "option": "new", "name": "Layout2" }));

    // Draw something on each layout so they plot.
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "Layout2" })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [0, 10]] })).unwrap();

    // Send to Redline — plots Layout1 and Layout2 to one PDF.
    let out = d.join("roundtrip.pdf");
    let r = s.execute("redline", &json!({ "path": out.to_string_lossy(), "layouts": ["Layout1", "Layout2"] })).unwrap();
    assert_eq!(r["opened"], false);
    let pdf = std::fs::read(&out).unwrap();

    // Add synthetic annotations to the sent PDF:
    //   Page 1 (Layout1): cloud by Reviewer A, note by Reviewer B
    //   Page 2 (Layout2): note by Reviewer A, cloud by Reviewer B
    let cloud_a =
        mk(MarkupKind::Cloud, "Reviewer A", [100.0, 100.0, 200.0, 200.0], "", vec![[100.0, 100.0], [200.0, 100.0], [200.0, 200.0], [100.0, 200.0]]);
    let note_b = note("Reviewer B", [300.0, 300.0, 400.0, 330.0], "Check wiring");
    let pdf = cadaddy_io::pdf_markups::add_markups(&pdf, 1, &[cloud_a, note_b]).unwrap();

    let note_a = note("Reviewer A", [150.0, 150.0, 250.0, 180.0], "Move conduit");
    let cloud_b =
        mk(MarkupKind::Cloud, "Reviewer B", [50.0, 50.0, 150.0, 150.0], "", vec![[50.0, 50.0], [150.0, 50.0], [150.0, 150.0], [50.0, 150.0]]);
    let pdf = cadaddy_io::pdf_markups::add_markups(&pdf, 2, &[note_a, cloud_b]).unwrap();
    std::fs::write(&out, &pdf).unwrap();

    // Import with Layout1 current.
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    let r = s.execute("redline.import", &json!({ "path": out.to_string_lossy() })).unwrap();

    let pages = r["pages"].as_array().expect("pages array");
    assert_eq!(pages.len(), 2, "two pages imported: {r}");
    assert_eq!(pages[0]["layout"], "Layout1", "page 1 → Layout1");
    assert_eq!(pages[0]["page"], 1);
    assert_eq!(pages[1]["layout"], "Layout2", "page 2 → Layout2");
    assert_eq!(pages[1]["page"], 2);

    let doc = s.doc().unwrap();

    // Page 1 markups are entities of Layout1.
    for item in pages[0]["placed"].as_array().unwrap() {
        let h = Handle::parse_hex(item["handles"][0].as_str().unwrap()).unwrap();
        assert_eq!(doc.space_of(h), Some(Space::Paper("Layout1".into())), "page 1 entity in Layout1");
    }

    // Page 2 markups are entities of Layout2.
    for item in pages[1]["placed"].as_array().unwrap() {
        let h = Handle::parse_hex(item["handles"][0].as_str().unwrap()).unwrap();
        assert_eq!(doc.space_of(h), Some(Space::Paper("Layout2".into())), "page 2 entity in Layout2");
    }

    // Each markup on MARKUP-<author>.
    for page in pages {
        for item in page["placed"].as_array().unwrap() {
            let layer = item["layer"].as_str().unwrap();
            assert!(layer.starts_with("MARKUP-"), "layer {layer} starts with MARKUP-");
        }
    }

    // Page 1: cloud (Reviewer A) is an LWPOLYLINE with bulges.
    let p1 = pages[0]["placed"].as_array().unwrap();
    let cloud_item = p1.iter().find(|x| x["layer"].as_str().unwrap().contains("Reviewer A")).unwrap();
    let h = Handle::parse_hex(cloud_item["handles"][0].as_str().unwrap()).unwrap();
    let e = doc.entity(h).unwrap();
    match &e.kind {
        EntityKind::LwPolyline(lw) => {
            assert!(lw.vertices.iter().any(|v| v.bulge.abs() > 0.0), "cloud has bulges");
        }
        other => panic!("cloud should be LWPolyline, got {other:?}"),
    }

    // Page 1: note (Reviewer B) is an MTEXT with "Check wiring".
    let note_item = p1.iter().find(|x| x["layer"].as_str().unwrap().contains("Reviewer B")).unwrap();
    let h = Handle::parse_hex(note_item["handles"][0].as_str().unwrap()).unwrap();
    let e = doc.entity(h).unwrap();
    match &e.kind {
        EntityKind::MText(mt) => assert_eq!(mt.contents, "Check wiring"),
        other => panic!("note should be MText, got {other:?}"),
    }

    // Page 2: note (Reviewer A) is an MTEXT with "Move conduit".
    let p2 = pages[1]["placed"].as_array().unwrap();
    let note_item = p2.iter().find(|x| x["layer"].as_str().unwrap().contains("Reviewer A")).unwrap();
    let h = Handle::parse_hex(note_item["handles"][0].as_str().unwrap()).unwrap();
    let e = doc.entity(h).unwrap();
    match &e.kind {
        EntityKind::MText(mt) => assert_eq!(mt.contents, "Move conduit"),
        other => panic!("note should be MText, got {other:?}"),
    }

    // Page 2: cloud (Reviewer B) is an LWPOLYLINE with bulges.
    let cloud_item = p2.iter().find(|x| x["layer"].as_str().unwrap().contains("Reviewer B")).unwrap();
    let h = Handle::parse_hex(cloud_item["handles"][0].as_str().unwrap()).unwrap();
    let e = doc.entity(h).unwrap();
    match &e.kind {
        EntityKind::LwPolyline(lw) => {
            assert!(lw.vertices.iter().any(|v| v.bulge.abs() > 0.0), "cloud has bulges");
        }
        other => panic!("cloud should be LWPolyline, got {other:?}"),
    }

    // Position check: the note at rect [300,300,400,330] on page 1 → insert at the rect's
    // top-left corner (rect[0], rect[3]) scaled by the sheet placement (1/72 in per point)
    // plus Layout1's origin.
    let note_item = p1.iter().find(|x| x["layer"].as_str().unwrap().contains("Reviewer B")).unwrap();
    let h = Handle::parse_hex(note_item["handles"][0].as_str().unwrap()).unwrap();
    let e = doc.entity(h).unwrap();
    if let EntityKind::MText(mt) = &e.kind {
        let origin = doc.layouts.iter().find(|l| l.name == "Layout1").unwrap().origin;
        // Note insert comes from rect[3] (top-left y) in place_one → rect.get(3) → [rect[0], rect[3]].
        let expected_x = 300.0 / 72.0 + origin.x;
        let expected_y = 330.0 / 72.0 + origin.y;
        assert!((mt.insert.x - expected_x).abs() < 0.01, "x within 0.01: got {} expected {expected_x}", mt.insert.x);
        assert!((mt.insert.y - expected_y).abs() < 0.01, "y within 0.01: got {} expected {expected_y}", mt.insert.y);
    }

    std::fs::remove_dir_all(&d).ok();
}

/// MUST 2: Model page markup is refused with a message naming the page and `from`/`to`.
#[test]
fn model_page_refused() {
    hooks();
    let d = dir("model");
    let mut s = Session::new();

    // Draw in model space so Model is included in the send.
    s.execute("line", &json!({ "points": [[0, 0], [100, 50]] })).unwrap();

    // Send: Model + Layout1.
    let out = d.join("model.pdf");
    s.execute("redline", &json!({ "path": out.to_string_lossy() })).unwrap();
    let pdf = std::fs::read(&out).unwrap();

    // Add annotations to page 1 (Model).
    let n = note("Reviewer A", [100.0, 100.0, 200.0, 130.0], "Fix here");
    let pdf = cadaddy_io::pdf_markups::add_markups(&pdf, 1, &[n]).unwrap();
    std::fs::write(&out, &pdf).unwrap();

    // Import — Model page should be refused.
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    let r = s.execute("redline.import", &json!({ "path": out.to_string_lossy() })).unwrap();

    let unmatched = r["unmatched"].as_array().expect("unmatched array");
    assert!(!unmatched.is_empty(), "Model page is unmatched: {r}");
    let reason = unmatched[0]["reason"].as_str().expect("reason string");
    assert!(reason.contains("Model"), "reason mentions Model: {reason}");
    assert!(reason.contains("from") && reason.contains("to"), "reason mentions from/to: {reason}");

    std::fs::remove_dir_all(&d).ok();
}

/// MUST 4: the sent PDF has the right page count, bookmark titles, and hyperlinks.
#[test]
fn send_pdf_page_count_bookmarks_links() {
    hooks();
    let d = dir("sendcheck");
    let mut s = Session::new();

    // Ensure Layout2 exists.
    let _ = s.execute("layout", &json!({ "option": "new", "name": "Layout2" }));

    // Draw on each layout.
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    // Add a hyperlink (PE_URL extended data) to a line within the printable area.
    let line_r = s.execute("line", &json!({ "points": [[3, 3], [5, 3]] })).unwrap();
    let line_h = line_r["handles"][0].as_str().unwrap();
    let hr = s.execute("hyperlink", &json!({ "handles": [line_h], "url": "https://aicodeforge.dev/test" })).unwrap();
    assert_eq!(hr["changed"], 1, "hyperlink attached: {hr}");

    s.execute("layout", &json!({ "option": "set", "name": "Layout2" })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [0, 10]] })).unwrap();

    // Send.
    let out = d.join("check.pdf");
    s.execute("redline", &json!({ "path": out.to_string_lossy(), "layouts": ["Layout1", "Layout2"] })).unwrap();
    let bytes = std::fs::read(&out).unwrap();

    // Page count == 2 (layouts sent).
    let count = cadaddy_io::pdf_markups::page_count(&bytes).unwrap();
    assert_eq!(count, 2, "page count == layouts sent");

    // Bookmark titles == layout names.
    let bmarks = cadaddy_io::pdf_merge::bookmarks(&bytes).unwrap();
    let titles: Vec<&str> = bmarks.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(titles, ["Layout1", "Layout2"], "bookmark titles == layout names");

    // The hyperlink produces a /Link annotation in the merged PDF.
    let pdf_text = String::from_utf8_lossy(&bytes);
    let link_count = pdf_text.matches("/Subtype /Link").count() + pdf_text.matches("/Subtype/Link").count();
    assert_eq!(link_count, 1, "exactly one /Subtype /Link in the PDF");
    assert!(pdf_text.contains("aicodeforge.dev/test"), "the URI is in the PDF");

    std::fs::remove_dir_all(&d).ok();
}
