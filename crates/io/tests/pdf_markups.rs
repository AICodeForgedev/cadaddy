//! PDF review markups read back from an annotated page: every kind, its place, comment, author
//! and colour.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_io::pdf_markups::{annotated_pdf, read};
use lopdf::{Dictionary, Document, Object, ObjectId};

pub fn sample() -> Vec<Markup> {
    let m = |kind, points: Vec<[f64; 2]>, rect: [f64; 4], text: &str| Markup {
        kind,
        points,
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: "Reviewer".into(),
        subject: String::new(),
        color: Some([255, 0, 0]),
        date: String::new(),
    };
    let mut ink = m(MarkupKind::Ink, Vec::new(), [500.0, 500.0, 560.0, 540.0], "");
    ink.strokes = vec![vec![[500.0, 500.0], [520.0, 540.0], [560.0, 510.0]]];
    let mut hl = m(MarkupKind::Highlight, Vec::new(), [100.0, 700.0, 200.0, 712.0], "check this");
    hl.strokes = vec![vec![[100.0, 700.0], [200.0, 700.0], [200.0, 712.0], [100.0, 712.0]]];
    vec![
        m(MarkupKind::Cloud, vec![[100.0, 100.0], [300.0, 100.0], [300.0, 200.0], [100.0, 200.0]], [100.0, 100.0, 300.0, 200.0], "Move this wall 2'"),
        m(MarkupKind::Rectangle, Vec::new(), [400.0, 100.0, 450.0, 150.0], ""),
        m(MarkupKind::Arrow, vec![[320.0, 150.0], [380.0, 250.0]], [320.0, 150.0, 380.0, 250.0], ""),
        m(MarkupKind::Callout, vec![[200.0, 150.0], [250.0, 300.0], [260.0, 300.0]], [260.0, 290.0, 400.0, 320.0], "Add outlet here"),
        m(MarkupKind::Note, Vec::new(), [600.0, 600.0, 700.0, 640.0], "General note: ✓ verify"),
        m(MarkupKind::Stamp, Vec::new(), [650.0, 50.0, 750.0, 90.0], "APPROVED"),
        ink,
        hl,
    ]
}

#[test]
fn every_kind_reads_back() {
    let pdf = annotated_pdf(1224.0, 792.0, &sample()).unwrap();
    let page = read(&pdf, 1).unwrap();
    assert_eq!((page.width, page.height), (1224.0, 792.0));
    let kinds: Vec<MarkupKind> = page.markups.iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            MarkupKind::Cloud,
            MarkupKind::Rectangle,
            MarkupKind::Arrow,
            MarkupKind::Callout,
            MarkupKind::Note,
            MarkupKind::Stamp,
            MarkupKind::Ink,
            MarkupKind::Highlight
        ]
    );
    let cloud = &page.markups[0];
    assert_eq!(cloud.text, "Move this wall 2'");
    assert_eq!(cloud.author, "Reviewer");
    assert_eq!(cloud.color, Some([255, 0, 0]));
    assert_eq!(cloud.points.len(), 4);
    assert_eq!(page.markups[2].points.first(), Some(&[320.0, 150.0]), "arrowhead first");
    assert_eq!(page.markups[3].points.len(), 3, "the callout's leader");
    assert_eq!(page.markups[4].text, "General note: ✓ verify", "UTF-16 text");
    assert_eq!(page.markups[5].text, "APPROVED");
    assert_eq!(page.markups[6].strokes[0].len(), 3);
    assert_eq!(page.markups[7].strokes.len(), 1);
    assert!(read(&pdf, 2).is_err());
    assert!(read(b"not a pdf", 1).is_err());
}

#[test]
fn an_icon_sized_note_is_a_sticky_note() {
    let note = |rect: [f64; 4]| Markup {
        kind: MarkupKind::Note,
        points: Vec::new(),
        strokes: Vec::new(),
        rect,
        text: "Verify the clearance in front of the panel.".into(),
        author: "Reviewer".into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    };
    let pdf = annotated_pdf(612.0, 792.0, &[note([600.0, 600.0, 620.0, 620.0])]).unwrap();
    assert!(pdf.windows(13).any(|w| w == b"/Subtype/Text") || pdf.windows(14).any(|w| w == b"/Subtype /Text"), "a Text annotation");
    let page = read(&pdf, 1).unwrap();
    let m = &page.markups[0];
    assert_eq!((m.kind, m.rect, m.text.as_str()), (MarkupKind::Note, [600.0, 600.0, 620.0, 620.0], "Verify the clearance in front of the panel."));
    assert_eq!(m.points, [[600.0, 620.0]], "anchored at the icon's top left");
}

// --- Helper to build a PDF from raw dictionaries (third-party-shaped, NOT via annotated_pdf) ---

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

fn arr(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| real(*x)).collect())
}

/// Build a one-page PDF whose page dictionary has `media_box`, optional `crop_box`, optional
/// `/Rotate`, and the given annotation dictionaries.
fn build_pdf(
    media_box: [f64; 4],
    crop_box: Option<[f64; 4]>,
    rotate: Option<i64>,
    annots: Vec<Dictionary>,
    inherit_media_from_parent: bool,
) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id: ObjectId = doc.new_object_id();
    let mut annot_refs = Vec::new();
    for a in annots {
        annot_refs.push(Object::Reference(doc.add_object(a)));
    }
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    if !inherit_media_from_parent {
        page.set("MediaBox", arr(&media_box));
    }
    if let Some(cb) = crop_box {
        page.set("CropBox", arr(&cb));
    }
    if let Some(r) = rotate {
        page.set("Rotate", Object::Integer(r));
    }
    page.set("Annots", Object::Array(annot_refs));
    let page_id = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    if inherit_media_from_parent {
        pages.set("MediaBox", arr(&media_box));
    }
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// A simple square annotation dict at the given Rect.
fn square_annot(rect: [f64; 4], cloudy: bool) -> Dictionary {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(b"Square".to_vec()));
    d.set("Rect", arr(&rect));
    d.set("Contents", Object::string_literal("cloud test"));
    d.set("T", Object::string_literal("Tester"));
    if cloudy {
        let mut be = Dictionary::new();
        be.set("S", Object::Name(b"C".to_vec()));
        d.set("BE", Object::Dictionary(be));
    }
    d
}

// --- MUST 1: CropBox and /Rotate ---

#[test]
fn rotate_90_transforms_cloud_coordinates() {
    // A 792×612 page (landscape letter) rotated 90° so it's viewed as 612×792 (portrait).
    // A cloud annotation at Rect [100,200,300,400] in unrotated coordinates.
    // Under /Rotate 90: (x, y) → (y, W−x) where W=792.
    // So point (100,200) → (200, 692), point (300,400) → (400, 492).
    // The viewed page is 612 wide × 792 tall.
    let cloud = square_annot([100.0, 200.0, 300.0, 400.0], true);
    let pdf = build_pdf([0.0, 0.0, 792.0, 612.0], None, Some(90), vec![cloud], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.width, 612.0, "viewed width after 90° rotation");
    assert_eq!(page.height, 792.0, "viewed height after 90° rotation");
    assert_eq!(page.markups.len(), 1);
    let m = &page.markups[0];
    assert_eq!(m.kind, MarkupKind::Cloud);
    // Rect corners: (100,200) and (300,400) → rotated to (200,692) and (400,492).
    // rect = [min_x, min_y, max_x, max_y] = [200, 492, 400, 692].
    assert!((m.rect[0] - 200.0).abs() < 0.01, "rect x0: {}", m.rect[0]);
    assert!((m.rect[1] - 492.0).abs() < 0.01, "rect y0: {}", m.rect[1]);
    assert!((m.rect[2] - 400.0).abs() < 0.01, "rect x1: {}", m.rect[2]);
    assert!((m.rect[3] - 692.0).abs() < 0.01, "rect y1: {}", m.rect[3]);
    // Points should be the 4 rect corners, all transformed.
    for p in &m.points {
        assert!(p[0] >= 200.0 - 0.01 && p[0] <= 400.0 + 0.01, "x in rotated range: {:?}", p);
        assert!(p[1] >= 492.0 - 0.01 && p[1] <= 692.0 + 0.01, "y in rotated range: {:?}", p);
    }
}

#[test]
fn cropbox_offset_shifts_coordinates() {
    // A letter page (612×792) with a CropBox offset: [50, 100, 562, 692].
    // A cloud at Rect [100, 200, 300, 400]. After shifting to the CropBox corner:
    // (100−50, 200−100) = (50, 100) and (300−50, 400−100) = (250, 300).
    // The viewed page is 512×592.
    let cloud = square_annot([100.0, 200.0, 300.0, 400.0], true);
    let pdf = build_pdf([0.0, 0.0, 612.0, 792.0], Some([50.0, 100.0, 562.0, 692.0]), None, vec![cloud], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.width, 512.0);
    assert_eq!(page.height, 592.0);
    let m = &page.markups[0];
    assert!((m.rect[0] - 50.0).abs() < 0.01, "shifted x0: {}", m.rect[0]);
    assert!((m.rect[1] - 100.0).abs() < 0.01, "shifted y0: {}", m.rect[1]);
    assert!((m.rect[2] - 250.0).abs() < 0.01, "shifted x1: {}", m.rect[2]);
    assert!((m.rect[3] - 300.0).abs() < 0.01, "shifted y1: {}", m.rect[3]);
}

// --- MUST 4: Third-party dictionary reader tests ---

#[test]
fn square_with_cloudy_border_reads_as_cloud() {
    // Square with /BE <</S /C>> → Cloud.
    let cloud = square_annot([100.0, 100.0, 300.0, 200.0], true);
    let pdf = build_pdf([0.0, 0.0, 612.0, 792.0], None, None, vec![cloud], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.markups.len(), 1);
    assert_eq!(page.markups[0].kind, MarkupKind::Cloud);
    assert_eq!(page.markups[0].points.len(), 4, "rect corners as points");
    assert_eq!(page.markups[0].text, "cloud test");
}

#[test]
fn line_with_closed_arrow_reads_as_arrow_head_first() {
    // Line with /LE [/None /ClosedArrow] → Arrow with head (end point) first.
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(b"Line".to_vec()));
    d.set("Rect", arr(&[100.0, 100.0, 400.0, 300.0]));
    d.set("L", arr(&[100.0, 100.0, 400.0, 300.0]));
    d.set("LE", Object::Array(vec![Object::Name(b"None".to_vec()), Object::Name(b"ClosedArrow".to_vec())]));
    d.set("Contents", Object::string_literal("arrow test"));
    let pdf = build_pdf([0.0, 0.0, 612.0, 792.0], None, None, vec![d], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.markups.len(), 1);
    let m = &page.markups[0];
    assert_eq!(m.kind, MarkupKind::Arrow);
    // Head first: /LE [/None /ClosedArrow] means end has the arrow → reversed.
    assert!((m.points[0][0] - 400.0).abs() < 0.01, "head (end) first x: {}", m.points[0][0]);
    assert!((m.points[0][1] - 300.0).abs() < 0.01, "head (end) first y: {}", m.points[0][1]);
    assert!((m.points[1][0] - 100.0).abs() < 0.01, "tail x: {}", m.points[1][0]);
}

#[test]
fn polyline_without_le_reads_as_polyline() {
    // PolyLine without LE → Polyline.
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(b"PolyLine".to_vec()));
    d.set("Rect", arr(&[10.0, 10.0, 500.0, 500.0]));
    d.set("Vertices", arr(&[10.0, 10.0, 250.0, 500.0, 500.0, 10.0]));
    d.set("Contents", Object::string_literal("polyline test"));
    let pdf = build_pdf([0.0, 0.0, 612.0, 792.0], None, None, vec![d], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.markups.len(), 1);
    let m = &page.markups[0];
    assert_eq!(m.kind, MarkupKind::Polyline);
    assert_eq!(m.points.len(), 3);
    assert!((m.points[0][0] - 10.0).abs() < 0.01);
    assert!((m.points[1][0] - 250.0).abs() < 0.01);
    assert!((m.points[2][0] - 500.0).abs() < 0.01);
}

#[test]
fn cmyk_color_converts_to_rgb() {
    // CMYK C [0 1 1 0] → pure red [255, 0, 0].
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(b"Square".to_vec()));
    d.set("Rect", arr(&[100.0, 100.0, 200.0, 200.0]));
    d.set("C", arr(&[0.0, 1.0, 1.0, 0.0]));
    d.set("Contents", Object::string_literal("cmyk test"));
    let pdf = build_pdf([0.0, 0.0, 612.0, 792.0], None, None, vec![d], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.markups.len(), 1);
    assert_eq!(page.markups[0].color, Some([255, 0, 0]));
}

#[test]
fn mediabox_inherited_from_pages_with_nonzero_origin() {
    // MediaBox inherited from the Pages node with a non-zero origin [50, 100, 662, 892].
    // A cloud at [200, 300, 400, 500] → shifted by (−50, −100) → [150, 200, 350, 400].
    let cloud = square_annot([200.0, 300.0, 400.0, 500.0], true);
    let pdf = build_pdf([50.0, 100.0, 662.0, 892.0], None, None, vec![cloud], true);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.width, 612.0);
    assert_eq!(page.height, 792.0);
    let m = &page.markups[0];
    assert!((m.rect[0] - 150.0).abs() < 0.01, "shifted x0: {}", m.rect[0]);
    assert!((m.rect[1] - 200.0).abs() < 0.01, "shifted y0: {}", m.rect[1]);
    assert!((m.rect[2] - 350.0).abs() < 0.01, "shifted x1: {}", m.rect[2]);
    assert!((m.rect[3] - 400.0).abs() < 0.01, "shifted y1: {}", m.rect[3]);
}

#[test]
fn hidden_annotation_is_skipped() {
    // F=2 (Hidden) → skipped.
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(b"Square".to_vec()));
    d.set("Rect", arr(&[100.0, 100.0, 200.0, 200.0]));
    d.set("F", Object::Integer(2));
    d.set("Contents", Object::string_literal("hidden"));
    let visible = square_annot([300.0, 300.0, 400.0, 400.0], false);
    let pdf = build_pdf([0.0, 0.0, 612.0, 792.0], None, None, vec![d, visible], false);
    let page = read(&pdf, 1).unwrap();
    assert_eq!(page.markups.len(), 1);
    assert_eq!(page.markups[0].text, "cloud test");
}
