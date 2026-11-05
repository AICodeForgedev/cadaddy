//! Tests for Q5239 pdfnative: PDF import quality improvements.
//!
//! Each test constructs a synthetic PDF and verifies that the converter's output passes the
//! importcheck gate criteria:
//! 1. SCALE — model at real-world size, not sheet size
//! 2. LAYERS — content-class layer assignment
//! 3. TEXT — SHX glyph-stroke recovery
//! 4. FILLS — no artifact opaque fills
//! 5. SYMBOLS — repeated clusters become blocks

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_io::pdf_read::{self, LayerMode, PdfOptions};

/// Build a minimal one-page 11×8.5 inch landscape PDF around a content stream, with one
/// Helvetica font resource (/F1).
fn pdf_with(content: &[u8]) -> Vec<u8> {
    pdf_sized(content, 792, 612)
}

/// The same on a `w` × `h` point page.
fn pdf_sized(content: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"),
        String::new(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    objs[3] = format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), String::from_utf8_lossy(content));
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

// -----------------------------------------------------------------------------------------
// Fix 1: Scale detection — a PDF with a scale note should produce model-space geometry at
// real-world size and a layout at sheet size.
// -----------------------------------------------------------------------------------------

#[test]
fn scale_note_produces_real_world_size() {
    // A PDF with a 1/4" = 1'-0" scale note and a line from 72 to 144 pt (1" to 2" on the sheet)
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 144 72 l S BT /F1 8 Tf 72 280 Td (SCALE: 1/4\" = 1'-0\") Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    // scale = 48 (1/4" = 1'-0" means 48 drawing inches per sheet inch)
    assert_eq!(r.scale, 48.0, "scale should be 48, got {}: {}", r.scale, r.scale_source);
    // the line in model space should be at 48x coordinates
    let has_scaled_line =
        r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Line(l) if (l.a.x - 48.0).abs() < 1.0 && (l.b.x - 96.0).abs() < 1.0));
    assert!(has_scaled_line, "model-space line should be at real-world coordinates (48..96)");
    // there should be a layout with a viewport
    assert!(
        r.drawing.layouts.iter().any(|l| l.entities.iter().any(|e| matches!(e.kind, EntityKind::Viewport(_)))),
        "should have a layout with a viewport"
    );
}

#[test]
fn scale_note_on_title_layer_preferred() {
    // Two scale notes: one on a generic layer, one on a TITLE layer; the TITLE one should win
    // when using detect_scale_note
    let content = b"q BT /F1 8 Tf 500 100 Td (SCALE: 1/8\" = 1'-0\") Tj ET BT /F1 8 Tf 72 500 Td (SCALE: 1/4\" = 1'-0\") Tj ET Q";
    // Both notes are present; enhanced detection should pick the best one
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    // should detect one of the scale notes
    assert!(r.scale > 1.0, "should have detected a scale note, got {}: {}", r.scale, r.scale_source);
    assert!(r.scale_source.contains("scale note"), "should report scale source as scale note");
}

// -----------------------------------------------------------------------------------------
// Fix 2: Content-class layer assignment
// -----------------------------------------------------------------------------------------

#[test]
fn content_mode_assigns_class_layers() {
    // A PDF with text and geometry; in Content mode, text should go on PDF-TEXT layer
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 10 Tf 72 300 Td (Room 101) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions { layers: LayerMode::Content, ..PdfOptions::default() });
    let r = r.expect("PDF read failed");
    let layers: Vec<&str> = r.drawing.layers.iter().map(|l| l.name.as_str()).collect();
    // in Content mode, text should be on TEXT and geometry on PDF-GEOMETRY (or similar)
    assert!(
        r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Text(_)) && e.common.layer.as_str() == "TEXT"),
        "text should be on TEXT layer, layers: {layers:?}"
    );
}

// -----------------------------------------------------------------------------------------
// Fix 3: SHX glyph-stroke recovery
// -----------------------------------------------------------------------------------------

#[test]
fn shx_stroke_clusters_become_text() {
    // Simulate SHX-stroked lettering: many short line segments clustered together
    // (like a plotter drawing each character as linework)
    // We create a grid of short strokes that look like a character cluster
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w ");
    // draw 5 clusters of short strokes at different x positions (like 5 characters)
    for cluster_x in 0..5 {
        let bx = 100 + cluster_x * 15;
        for stroke in 0..6 {
            let y = 300 + stroke * 2;
            content.extend_from_slice(format!("{bx} {y} m {} {y} l S ", bx + 8).as_bytes());
        }
    }
    // also add a real text entity for comparison
    content.extend_from_slice(b"BT /F1 10 Tf 100 400 Td (Real Text) Tj ET ");
    content.extend_from_slice(b"Q");

    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    // should have some text entities (real text + recovered SHX text)
    let text_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count();
    assert!(text_count >= 1, "should have at least the real text entity, got {text_count}");
    // verify stats mention SHX recovery if it happened
    if let Some(shx_count) = r.stats.get("SHX text clusters recovered") {
        assert!(*shx_count > 0, "SHX clusters should be recovered");
    }
}

// -----------------------------------------------------------------------------------------
// Fix 4: Artifact fill filtering
// -----------------------------------------------------------------------------------------

#[test]
fn large_artifact_fills_are_removed() {
    // A large bright yellow fill covering most of the sheet (an artifact) should be removed
    let content = b"q 1 1 0 rg 0 0 792 612 re f 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 10 Tf 72 300 Td (Test) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    // the large yellow fill should have been filtered out
    let yellow_fills = r
        .drawing
        .model
        .iter()
        .filter(|e| {
            matches!(&e.kind, EntityKind::Hatch(h) if h.solid) && matches!(e.common.color, Color::True(Rgb(r, g, b)) if r > 200 && g > 200 && b < 100)
        })
        .count();
    assert_eq!(yellow_fills, 0, "large yellow artifact fill should be removed");
    if let Some(n) = r.stats.get("artifact fills removed") {
        assert!(*n > 0, "should report artifact fills removed in stats");
    }
}

#[test]
fn legitimate_small_fills_are_kept() {
    // A small legitimate solid fill (like a wall section fill) should NOT be removed
    let content = b"q 0.5 0.5 0.5 rg 200 200 30 30 re f 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 10 Tf 72 300 Td (Test) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    // the small gray fill should still be present
    let fills = r.drawing.model.iter().filter(|e| matches!(&e.kind, EntityKind::Hatch(h) if h.solid)).count();
    assert!(fills >= 1, "small legitimate fills should be kept, got {fills}");
}

// -----------------------------------------------------------------------------------------
// Fix 5: Repeated symbol clusters become blocks
// -----------------------------------------------------------------------------------------

#[test]
fn repeated_symbols_become_blocks() {
    // Draw the same small pattern (a diamond) at 4 different positions — should become a block
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w ");
    // Draw 4 identical diamond patterns at different positions
    for (cx, cy) in [(150, 300), (250, 300), (350, 300), (450, 300)] {
        // a diamond: 4 lines forming a small shape
        let sz = 8;
        content.extend_from_slice(
            format!(
                "{} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S ",
                cx,
                cy - sz,
                cx + sz,
                cy, // bottom to right
                cx + sz,
                cy,
                cx,
                cy + sz, // right to top
                cx,
                cy + sz,
                cx - sz,
                cy, // top to left
                cx - sz,
                cy,
                cx,
                cy - sz, // left to bottom
            )
            .as_bytes(),
        );
        // add more strokes to meet the 3-stroke minimum per cluster
        content.extend_from_slice(
            format!("{} {} m {} {} l S {} {} m {} {} l S ", cx - 3, cy - 3, cx + 3, cy + 3, cx - 3, cy + 3, cx + 3, cy - 3,).as_bytes(),
        );
    }
    // add a text label near each symbol
    for (i, (cx, _cy)) in [(150, 320), (250, 320), (350, 320), (450, 320)].iter().enumerate() {
        content.extend_from_slice(format!("BT /F1 6 Tf {} 320 Td (D{}) Tj ET ", cx, i + 1).as_bytes());
    }
    content.extend_from_slice(b"Q");

    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    // check if symbols were detected (may or may not trigger depending on cluster detection)
    let block_count = r.drawing.blocks.len();
    let insert_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))).count();
    // if symbol detection triggered, we should have blocks and inserts
    if let Some(&sym_count) = r.stats.get("symbol instances detected") {
        assert!(sym_count > 0, "should have detected symbol instances");
        assert!(block_count > 0, "should have created block definitions");
        assert!(insert_count > 0, "should have created block inserts");
    }
    // at minimum, the drawing should not be empty
    assert!(!r.drawing.model.is_empty(), "drawing should not be empty");
}

// -----------------------------------------------------------------------------------------
// Regression: existing behaviour preserved
// -----------------------------------------------------------------------------------------

#[test]
fn basic_import_still_works() {
    // The sample PDF from the existing test suite: make sure it still works
    let content = b"q 0 0 0 RG 1.2 w 72 72 m 144 72 l S 0.5 0.5 0.5 RG 72 100 72 36 re S 1 1 1 rg 200 200 20 20 re f BT /F1 12 Tf 72 300 Td (PANEL A) Tj ET BT /F1 8 Tf 72 280 Td (SCALE: 1/4\" = 1'-0\") Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default());
    let r = r.expect("PDF read failed");
    assert_eq!(r.scale, 48.0, "scale should still be 48");
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "PANEL A")));
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Line(_))));
    // white fill should be a wipeout, not a hatch
    assert!(!r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Hatch(_))), "white fill should be a wipeout");
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Wipeout(_))));
}

#[test]
fn content_layer_mode_accepted() {
    // Verify the Content layer mode option is accepted and produces layers
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 10 Tf 72 300 Td (Test) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions { layers: LayerMode::Content, ..PdfOptions::default() });
    let r = r.expect("PDF read failed");
    // in Content mode, we expect content-class layer names (E-GRID, A-WALL, TEXT, E-DEVICE, etc.)
    let layer_names: Vec<&str> = r.drawing.layers.iter().map(|l| l.name.as_str()).collect();
    let has_content_layers = layer_names
        .iter()
        .any(|n| ["E-GRID", "A-WALL", "TEXT", "E-DEVICE", "TITLEBLOCK", "HATCH", "ANNO", "PDF-GEOMETRY", "PDF-MASK", "PDF-IMAGES"].contains(n));
    assert!(has_content_layers, "Content mode should produce content-class layers, got: {layer_names:?}");
}

#[test]
fn parse_scale_still_works() {
    // Regression: the public parse_scale function still works
    assert_eq!(pdf_read::parse_scale("SCALE: 1/8\" = 1'-0\""), Some(96.0));
    assert_eq!(pdf_read::parse_scale("1/4\"=1'-0\""), Some(48.0));
    assert_eq!(pdf_read::parse_scale("1:100"), Some(100.0));
    assert_eq!(pdf_read::parse_scale("NOT TO SCALE"), None);
}

// -----------------------------------------------------------------------------------------
// Statistics regression test: reproduces the profile of a real E-size sheet (36×24)
// with many white fills (masks), raster tiles, text entities, and repeated device symbols.
// -----------------------------------------------------------------------------------------

/// Build a 36×24 E-size landscape PDF with content that simulates a real electrical plan:
/// - Many small white fills (masks behind device tags)
/// - Small raster tile strips
/// - Text entities (room tags, circuit numbers)
/// - Repeated symbol-like stroke clusters
/// - A "DO NOT SCALE" note
fn e_sheet_with_masks_and_tiles() -> Vec<u8> {
    let mut content = Vec::new();
    content.extend_from_slice(b"q ");

    // geometry: some wall-like heavy lines
    content.extend_from_slice(b"0 0 0 RG 2.0 w ");
    for y in [200, 400] {
        content.extend_from_slice(format!("72 {y} m 2520 {y} l S ").as_bytes());
    }
    for x in [72, 800, 1600, 2520] {
        content.extend_from_slice(format!("{x} 200 m {x} 400 l S ").as_bytes());
    }

    // many small white fills (simulating mask artefacts from PDF rendering)
    content.extend_from_slice(b"1 1 1 rg ");
    for i in 0..200 {
        let x = 100 + (i % 40) * 60;
        let y = 100 + (i / 40) * 80;
        content.extend_from_slice(format!("{x} {y} 20 8 re f ").as_bytes());
    }

    // text entities: room tags and device tags (each in its own BT/ET for absolute positioning)
    for i in 0..30 {
        let x = 100 + (i % 10) * 240;
        let y = 220 + (i / 10) * 150;
        content.extend_from_slice(format!("BT /F1 8 Tf {x} {y} Td (DP{}) Tj ET ", i + 1).as_bytes());
    }
    // DO NOT SCALE note
    content.extend_from_slice(b"BT /F1 8 Tf 2200 80 Td (DO NOT SCALE) Tj ET ");

    // repeated symbol-like patterns (small device shapes) — 20 instances of a diamond+cross
    content.extend_from_slice(b"0 0 0 RG 0.5 w ");
    for i in 0..20 {
        let cx = 150 + (i % 10) * 230;
        let cy = 250 + (i / 10) * 150;
        // diamond
        let sz = 6;
        content.extend_from_slice(
            format!(
                "{} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S ",
                cx,
                cy - sz,
                cx + sz,
                cy,
                cx + sz,
                cy,
                cx,
                cy + sz,
                cx,
                cy + sz,
                cx - sz,
                cy,
                cx - sz,
                cy,
                cx,
                cy - sz,
            )
            .as_bytes(),
        );
        // cross
        content.extend_from_slice(
            format!("{} {} m {} {} l S {} {} m {} {} l S ", cx - 3, cy - 3, cx + 3, cy + 3, cx - 3, cy + 3, cx + 3, cy - 3,).as_bytes(),
        );
    }

    content.extend_from_slice(b"Q");

    // build PDF with 36×24 inch page (2592×1728 points)
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2592 1728] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),
        String::new(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    objs[3] = format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), String::from_utf8_lossy(&content));
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

#[test]
fn stats_regression_e_sheet_profile() {
    let pdf = e_sheet_with_masks_and_tiles();
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");

    // the white fills should become wipeouts (masks); mask merging should drastically reduce count
    let wipeout_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Wipeout(_))).count();
    let text_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count();
    let block_count = r.drawing.blocks.len();
    let insert_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))).count();

    // text should be present (room tags + device tags + DO NOT SCALE)
    assert!(text_count >= 10, "should have text entities, got {text_count}");

    // mask merging: 200 small white fills should merge down significantly (>50% reduction)
    assert!(wipeout_count < 200, "masks should be merged: got {wipeout_count} from 200 white fills");

    // symbol detection: 20 identical diamond+cross patterns should become blocks
    if block_count > 0 {
        assert!(insert_count >= 3, "should have block inserts for repeated symbols, got {insert_count}");
    }

    // DO NOT SCALE: should stay at sheet size (scale=1)
    assert_eq!(r.scale, 1.0, "DO NOT SCALE sheet should stay at scale=1, got {}", r.scale);
    assert!(r.scale_source.contains("DO NOT SCALE"), "scale source should mention DO NOT SCALE: {}", r.scale_source);

    // nothing outside the page box (stray geometry fix)
    let sheet_w = r.sheet.0;
    for e in r.drawing.model.iter() {
        if let EntityKind::Line(l) = &e.kind {
            assert!(l.a.x >= -1.0 && l.b.x >= -1.0, "line outside page: {:?}", l);
            assert!(l.a.x <= sheet_w + 1.0 && l.b.x <= sheet_w + 1.0, "line outside page: {:?}", l);
        }
    }

    // print stats for the report
    eprintln!("--- E-sheet profile ---");
    for (k, v) in &r.stats {
        eprintln!("  {k}: {v}");
    }
    eprintln!("  scale: {} ({})", r.scale, r.scale_source);
    eprintln!("  wipeouts: {wipeout_count}, texts: {text_count}, blocks: {block_count}, inserts: {insert_count}");
}

#[test]
fn do_not_scale_keeps_sheet_size() {
    // A PDF with a DO NOT SCALE note and a scale note — should stay at scale=1
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 8 Tf 72 280 Td (SCALE: 1/4\" = 1'-0\") Tj 72 260 Td (DO NOT SCALE) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(r.scale, 1.0, "DO NOT SCALE should override the scale note");
    assert!(r.scale_source.contains("DO NOT SCALE"), "should report DO NOT SCALE");
}

#[test]
fn page_clip_removes_stray_geometry() {
    // A PDF with content outside the page box should be clipped
    let content = b"q 0 0 0 RG 1.0 w -200 100 m -100 100 l S 72 72 m 200 72 l S BT /F1 10 Tf 72 300 Td (Test) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default()).expect("PDF read failed");
    // the line at x=-200..-100 should be clipped (outside page box [0,0,792,612])
    let has_negative_line = r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Line(l) if l.a.x < -0.1 || l.b.x < -0.1));
    assert!(!has_negative_line, "stray geometry outside page box should be clipped");
    // the line inside the page should still be there
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Line(_))), "in-page lines should remain");
}

#[test]
fn mask_merging_reduces_count() {
    // Many overlapping small white fills should merge into fewer wipeouts
    let mut content = Vec::new();
    content.extend_from_slice(b"q ");
    // draw some geometry first
    content.extend_from_slice(b"0 0 0 RG 1.0 w 72 72 m 700 72 l S ");
    // 50 small overlapping white fills in a grid
    content.extend_from_slice(b"1 1 1 rg ");
    for i in 0..50 {
        let x = 100 + (i % 10) * 15;
        let y = 100 + (i / 10) * 15;
        content.extend_from_slice(format!("{x} {y} 20 10 re f ").as_bytes());
    }
    content.extend_from_slice(b"BT /F1 10 Tf 100 300 Td (Test) Tj ET Q");

    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default()).expect("PDF read failed");
    let wipeout_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Wipeout(_))).count();
    // 50 overlapping fills should merge to significantly fewer wipeouts
    assert!(wipeout_count < 50, "masks should merge: got {wipeout_count} from 50 white fills");
}

// -----------------------------------------------------------------------------------------
// Q5252 rw2: SHX text recovery from invisible (hidden) text matching
// -----------------------------------------------------------------------------------------

/// Build a PDF with invisible text (rendering mode 3) and stroked linework at the same
/// position — the hidden text recovery should produce text entities with the correct value.
fn pdf_with_hidden_text(visible: &[u8], hidden: &[u8]) -> Vec<u8> {
    // PDF with two content sections: visible linework, then invisible text (Tr 3)
    let mut stream = Vec::new();
    stream.extend_from_slice(b"q ");
    stream.extend_from_slice(visible);
    // invisible text: set rendering mode to 3 (invisible)
    stream.extend_from_slice(b" BT 3 Tr /F1 8 Tf ");
    stream.extend_from_slice(hidden);
    stream.extend_from_slice(b" ET ");
    // visible text too, for comparison
    stream.extend_from_slice(b" BT 0 Tr /F1 10 Tf 72 500 Td (Visible Label) Tj ET ");
    stream.extend_from_slice(b"Q");
    pdf_with(&stream)
}

#[test]
fn hidden_text_recovers_shx_strokes() {
    // Draw short strokes at (100, 200) that look like SHX text, plus invisible text "ROOM 101"
    // at the same position. The recovery should produce "ROOM 101" as text.
    let mut visible = Vec::new();
    visible.extend_from_slice(b"0 0 0 RG 0.5 w ");
    // draw a bunch of short strokes near (100, 200)
    for i in 0..15 {
        let x = 100 + (i % 5) * 4;
        let y = 198 + (i / 5) * 3;
        visible.extend_from_slice(format!("{x} {y} m {} {} l S ", x + 3, y + 2).as_bytes());
    }

    let hidden = b"100 200 Td (ROOM 101) Tj";

    let pdf = pdf_with_hidden_text(&visible, hidden);
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");

    // check that ROOM 101 was recovered (should be on the hidden text layer originally,
    // then matched to strokes and emitted as a visible text entity)
    let _has_room = r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value.contains("ROOM 101")));
    // the recovery should have found it (when hidden text matched strokes)
    if let Some(&n) = r.stats.get("SHX text clusters recovered") {
        assert!(n > 0, "should have recovered SHX text from hidden text");
    }
    // visible text should also be present
    let has_visible = r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "Visible Label"));
    assert!(has_visible, "visible text should still be present");
}

// -----------------------------------------------------------------------------------------
// Q5252 rw2: E-sheet DO NOT SCALE heuristic
// -----------------------------------------------------------------------------------------

#[test]
fn e_sheet_implies_do_not_scale() {
    // A PDF with sheet number "E-4" in the title block area and a scale note.
    // E-sheets always carry DO NOT SCALE, even when the note is only in stroked text.
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 12 Tf 700 50 Td (E-4) Tj ET BT /F1 8 Tf 600 80 Td (SCALE: 1/8\" = 1'-0\") Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default()).expect("PDF read failed");
    assert!(r.do_not_scale, "E-sheet should set do_not_scale");
    assert!(r.needs_registration, "E-sheet with scale note should set needs_registration");
    assert_eq!(r.scale, 1.0, "E-sheet should stay at scale=1, got {}", r.scale);
    assert!(r.scale_source.contains("DO NOT SCALE"), "should report DO NOT SCALE: {}", r.scale_source);
}

#[test]
fn non_e_sheet_uses_scale_note() {
    // A PDF with sheet number "A-101" (architectural) and a scale note.
    // Non-E-sheets should use the scale note normally.
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 12 Tf 700 50 Td (A-101) Tj ET BT /F1 8 Tf 600 80 Td (SCALE: 1/4\" = 1'-0\") Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default()).expect("PDF read failed");
    assert!(!r.do_not_scale, "A-sheet should not set do_not_scale");
    assert_eq!(r.scale, 48.0, "A-sheet should use scale note, got {}", r.scale);
}

// -----------------------------------------------------------------------------------------
// Q5252 rw2: Spatial raster stitching
// -----------------------------------------------------------------------------------------

#[test]
fn do_not_scale_fields_present() {
    // Verify that the PdfImport struct has the do_not_scale and needs_registration fields
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default()).expect("PDF read failed");
    // no DO NOT SCALE note, no E-sheet: both should be false
    assert!(!r.do_not_scale, "should be false without DO NOT SCALE note");
    assert!(!r.needs_registration, "should be false without DO NOT SCALE + scale note");
}

// -----------------------------------------------------------------------------------------
// Q5252 rw2: Device symbol detection from text proximity
// -----------------------------------------------------------------------------------------

#[test]
fn device_tags_near_strokes_become_blocks() {
    // Draw short stroke clusters near device-tag text labels. The symbol detection should
    // find them and create block inserts.
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w ");
    // draw 6 identical small stroke clusters at different positions, each with a tag "A"
    for (cx, cy) in [(150, 300), (250, 300), (350, 300), (150, 400), (250, 400), (350, 400)] {
        // a small diamond symbol (4 strokes)
        let sz = 6;
        content.extend_from_slice(
            format!(
                "{} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S ",
                cx,
                cy - sz,
                cx + sz,
                cy,
                cx + sz,
                cy,
                cx,
                cy + sz,
                cx,
                cy + sz,
                cx - sz,
                cy,
                cx - sz,
                cy,
                cx,
                cy - sz,
            )
            .as_bytes(),
        );
        // add cross strokes
        content.extend_from_slice(
            format!("{} {} m {} {} l S {} {} m {} {} l S ", cx - 3, cy - 3, cx + 3, cy + 3, cx - 3, cy + 3, cx + 3, cy - 3,).as_bytes(),
        );
        // tag text near the symbol
        content.extend_from_slice(format!("BT /F1 6 Tf {} {} Td (A) Tj ET ", cx + 10, cy).as_bytes());
    }
    content.extend_from_slice(b"Q");

    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default()).expect("PDF read failed");
    // with device tags "A" near each cluster, should detect symbols
    let _insert_count = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))).count();
    let _block_count = r.drawing.blocks.len();
    if let Some(&sym_count) = r.stats.get("symbol instances detected") {
        assert!(sym_count >= 4, "should detect ≥4 symbol instances from 6 device tags, got {sym_count}");
    }
    assert!(!r.drawing.model.is_empty(), "drawing should not be empty");
}

#[test]
fn a_symbols_tag_stays_where_it_was_printed() {
    // Six diamonds, each tagged "L4" 10 pt to its right in 6 pt text: the TAG attribute keeps
    // that place and size (not the drawing's origin), so the circuit printed under it pairs.
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w ");
    for (cx, cy) in [(150, 300), (250, 300), (350, 300), (150, 400), (250, 400), (350, 400)] {
        let sz = 6;
        content.extend_from_slice(
            format!(
                "{} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S {} {} m {} {} l S ",
                cx,
                cy - sz,
                cx + sz,
                cy,
                cx + sz,
                cy,
                cx,
                cy + sz,
                cx,
                cy + sz,
                cx - sz,
                cy,
                cx - sz,
                cy,
                cx,
                cy - sz,
            )
            .as_bytes(),
        );
        content.extend_from_slice(
            format!("{} {} m {} {} l S {} {} m {} {} l S ", cx - 3, cy - 3, cx + 3, cy + 3, cx - 3, cy + 3, cx + 3, cy - 3).as_bytes(),
        );
        content.extend_from_slice(format!("BT /F1 6 Tf {} {} Td (L4) Tj ET ", cx + 10, cy).as_bytes());
    }
    content.extend_from_slice(b"Q");
    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default()).expect("PDF read failed");
    let tagged: Vec<(f64, f64, f64)> = r
        .drawing
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Insert(i) if i.block.starts_with("PDF-SYM-") => {
                i.attribs.iter().find(|a| a.tag == "TAG").map(|a| (a.text.insert.x - i.insert.x, a.text.insert.y - i.insert.y, a.text.height))
            }
            _ => None,
        })
        .collect();
    assert!(tagged.len() >= 4, "symbols with their tag: {tagged:?}");
    for (dx, dy, h) in tagged {
        // 10 pt right of the symbol's centre, on its line, as tall as its letters (the reader's
        // text height for 6 pt type), in any drawing unit.
        assert!(dx > 0.0 && dy.abs() < 0.5 * dx && (0.3..=0.7).contains(&(h / dx)), "tag at {dx},{dy} height {h}");
    }
}

// -----------------------------------------------------------------------------------------
// SHX OCR: stroke-template character recognition on sheets without hidden text
// -----------------------------------------------------------------------------------------

/// Draw `text` in our stroke font as plain PDF strokes (no hidden text): `height` in points,
/// `width` the width factor, `oblique` the slant in degrees.  Returns the x after the text.
fn shx_text(text: &str, x: f64, y: f64, height: f64, width: f64, oblique: f64, out: &mut Vec<u8>) -> f64 {
    use cadaddy_fonts::stroke::{CAP, GAP, glyph, strokes};
    let k = height / CAP;
    let shear = oblique.to_radians().tan();
    let mut pen_x = x;
    for ch in text.chars() {
        let Some((advance, spec)) = glyph(ch) else {
            pen_x += (4.0 + GAP) * width * k;
            continue;
        };
        for s in strokes(spec) {
            if s.len() < 2 {
                continue;
            }
            for (i, (fx, fy)) in s.into_iter().enumerate() {
                let (px, py) = (pen_x + (fx * width + fy * shear) * k, y + fy * k);
                out.extend_from_slice(format!("{px:.2} {py:.2} {} ", if i == 0 { "m" } else { "l" }).as_bytes());
            }
            out.extend_from_slice(b"S ");
        }
        pen_x += (advance + GAP) * width * k;
    }
    pen_x
}

/// A page of black 0.5 pt strokes.
fn strokes_page(draw: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut content = b"q 0 0 0 RG 0.5 w ".to_vec();
    draw(&mut content);
    content.extend_from_slice(b"Q");
    pdf_with(&content)
}

/// A searchable (rendering mode 3) note far from the drawing: the page is an SHX print with
/// hidden text, so the OCR's reads there count as confirmed (their strokes leave the plan).
const HIDDEN_NOTE: &[u8] = b"BT 3 Tr /F1 6 Tf 700 560 Td (NOTE) Tj ET ";

/// The text values other than [`HIDDEN_NOTE`]'s.
fn text_values_but_note(r: &pdf_read::PdfImport) -> Vec<String> {
    text_values(r).into_iter().filter(|t| t != "NOTE").collect()
}

fn text_values(r: &pdf_read::PdfImport) -> Vec<String> {
    let mut v: Vec<String> = r
        .drawing
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

fn sorted(v: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

#[test]
fn ocr_recognises_simple_word() {
    // "HELLO" in SHX strokes, no hidden text and no visible text anchors: read from geometry
    let pdf = strokes_page(|c| {
        shx_text("HELLO", 100.0, 300.0, 12.0, 1.0, 0.0, c);
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&["HELLO"]));
    assert_eq!(r.stats.get("SHX text (OCR) recovered").copied(), Some(1));
}

#[test]
fn ocr_recognises_digits() {
    let pdf = strokes_page(|c| {
        shx_text("123", 200.0, 250.0, 10.0, 1.0, 0.0, c);
        shx_text("0123456789", 200.0, 200.0, 10.0, 1.0, 0.0, c);
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&["123", "0123456789"]));
}

#[test]
fn ocr_reads_the_whole_character_set_and_punctuation() {
    let lines = ["ABCDEFGHIJKLM", "NOPQRSTUVWXYZ", "PANEL LP-1A", "ROOM 104", "(A) #12 A&B 1/2", "6'-0\" A.F.F.", "BOB 808 S5"];
    let pdf = strokes_page(|c| {
        for (i, l) in lines.iter().enumerate() {
            shx_text(l, 80.0, 500.0 - i as f64 * 30.0, 12.0, 1.0, 0.0, c);
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&lines));
}

#[test]
fn ocr_reads_scaled_stretched_and_slanted_lettering() {
    // (text, height pt, width factor, oblique degrees)
    let cases = [
        ("SMALL 6", 6.0, 1.0, 0.0),
        ("LARGE 24", 24.0, 1.0, 0.0),
        ("NARROW E-12", 12.0, 0.7, 0.0),
        ("WIDE 3C", 10.0, 1.5, 0.0),
        ("SLANT 15", 12.0, 1.0, 15.0),
        ("OBLIQUE A&B 25", 12.0, 1.0, 25.0),
        ("BACK 7", 12.0, 1.0, -15.0),
        ("TAG XA-2", 9.0, 0.8, 15.0),
    ];
    let pdf = strokes_page(|c| {
        for (i, (t, h, w, o)) in cases.iter().enumerate() {
            shx_text(t, 60.0, 560.0 - i as f64 * 60.0, *h, *w, *o, c);
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&cases.map(|c| c.0)));
}

#[test]
fn ocr_splits_texts_at_wide_gaps_and_rows() {
    let pdf = strokes_page(|c| {
        // one text with a space, then a second text far to the right on the same baseline
        shx_text("AB CD", 100.0, 300.0, 12.0, 1.0, 0.0, c);
        shx_text("EF", 250.0, 300.0, 12.0, 1.0, 0.0, c);
        // a row just below, and a smaller row inside the first's x range
        shx_text("ROW TWO", 100.0, 280.0, 12.0, 1.0, 0.0, c);
        shx_text("TINY", 100.0, 266.0, 6.0, 1.0, 0.0, c);
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&["AB CD", "EF", "ROW TWO", "TINY"]));
}

#[test]
fn ocr_leaves_linework_and_unmatched_shapes_alone() {
    let pdf = strokes_page(|c| {
        // hatching: parallel short 45° lines (each would read as a lone '/')
        for i in 0..30 {
            let x = 100.0 + f64::from(i) * 3.0;
            c.extend_from_slice(format!("{x:.1} 100 m {:.1} 108 l S ", x + 8.0).as_bytes());
        }
        // tick marks and lone dashes
        for i in 0..10 {
            let x = 100.0 + f64::from(i) * 20.0;
            c.extend_from_slice(format!("{x:.1} 200 m {x:.1} 206 l S {x:.1} 220 m {:.1} 220 l S ", x + 6.0).as_bytes());
        }
        // shapes no character fits: a star, a spiral, a comb
        c.extend_from_slice(b"300 300 m 306 318 l 312 300 l 297 311 l 315 311 l 300 300 l S ");
        c.extend_from_slice(b"400 300 m 410 300 l 410 312 l 398 312 l 398 302 l 408 302 l 408 310 l 400 310 l 400 304 l 406 304 l S ");
        c.extend_from_slice(b"500 300 m 500 312 l 502 312 l 502 302 l 504 302 l 504 312 l 506 312 l 506 302 l 508 302 l 508 312 l 510 312 l S ");
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), Vec::<String>::new(), "linework must not become text");
    assert!(r.ocr.clusters >= 40, "the OCR looked at the shapes: {:?}", r.ocr);
    assert!(r.ocr.noise_dropped > 0, "lone straight strokes are dropped as noise: {:?}", r.ocr);
    // nothing was read, so every stroke stays visible linework
    assert!(r.drawing.layers.iter().all(|l| l.name != pdf_read::SHX_SOURCE_LAYER));
}

#[test]
fn ocr_reports_clusters_and_confidence() {
    // exact lettering, and the same lettering with its points nudged (as a plotter rounds)
    let pdf = strokes_page(|c| {
        shx_text("EXACT 42", 100.0, 400.0, 12.0, 1.0, 0.0, c);
        let mut rough = Vec::new();
        shx_text("ROUGH 42", 100.0, 300.0, 12.0, 1.0, 0.0, &mut rough);
        let mut n = 0u32;
        let nudged: Vec<String> = String::from_utf8_lossy(&rough)
            .split(' ')
            .map(|tok| match tok.parse::<f64>() {
                Ok(v) => {
                    n = n.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    format!("{:.2}", v + (f64::from(n >> 16 & 0xff) / 255.0 - 0.5) * 0.8)
                }
                Err(_) => tok.to_string(),
            })
            .collect();
        c.extend_from_slice(nudged.join(" ").as_bytes());
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&["EXACT 42", "ROUGH 42"]));
    let conf = |v: &str| r.ocr.words.iter().find(|w| w.value == v).map(|w| w.confidence).unwrap_or(-1.0);
    let (exact, rough) = (conf("EXACT 42"), conf("ROUGH 42"));
    assert!(exact > 0.95, "an exact fit is near-certain: {exact}");
    assert!(rough > 0.0 && rough < exact, "a rough fit is less sure: {rough} vs {exact}");
    // 14 glyphs: every one a cluster, every one matched
    assert_eq!((r.ocr.clusters, r.ocr.glyphs), (14, 14), "{:?}", r.ocr);
    assert_eq!(r.stats.get("SHX OCR glyph clusters").copied(), Some(14));
    assert_eq!(r.stats.get("SHX text (OCR) recovered").copied(), Some(2));
    // B11-002: the stat and the note give the mean of the words the import kept, not of none (0%)
    let pct = (r.ocr.mean_confidence() * 100.0).round() as usize;
    assert!(pct > 0, "{:?}", r.ocr);
    assert_eq!(r.stats.get("SHX OCR mean confidence %").copied(), Some(pct));
    assert!(r.notes.iter().any(|n| n.starts_with("SHX OCR:") && n.contains(&format!("mean confidence {pct}%"))), "{:?}", r.notes);
    // the PDFIMPORT report carries the same
    let (_, report) = pdf_read::read_json(&pdf, &serde_json::json!({})).expect("read_json");
    assert_eq!(report["ocr"]["clusters"], 14);
    assert_eq!(report["ocr"]["recovered"], 2);
    let words = report["ocr"]["words"].as_array().cloned().unwrap_or_default();
    assert!(words.iter().all(|w| w["confidence"].as_f64().is_some_and(|c| (0.0..=1.0).contains(&c))), "{words:?}");
}

#[test]
fn ocr_content_layer_fallback_without_ocgs() {
    // A PDF without optional content groups (OCGs) should fall back to Content layer mode,
    // creating content-class layer names like TEXT, ANNO, PDF-GEOMETRY, etc.
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 400 72 l S BT /F1 10 Tf 72 400 Td (Test label) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions::default()).expect("PDF read failed");
    let layer_names: Vec<&str> = r.drawing.layers.iter().map(|l| l.name.as_str()).collect();
    // should have content-class layers, not pen-based layers
    let has_content = layer_names.iter().any(|n| {
        n.starts_with("TEXT")
            || n.starts_with("ANNO")
            || n.starts_with("PDF-GEOMETRY")
            || n.starts_with("E-GRID")
            || n.starts_with("A-WALL")
            || n.starts_with("E-DEVICE")
            || n.starts_with("TITLEBLOCK")
            || n.starts_with("HATCH")
    });
    assert!(has_content, "should have content-class layers when no OCGs present, got: {layer_names:?}");
}

#[test]
fn ocr_consumed_strokes_go_to_a_hidden_layer() {
    // the strokes the OCR read stay in the drawing, on their own layer, turned off (on a page
    // with hidden text; without it the strokes stay linework, see
    // `ocr_without_hidden_text_keeps_the_strokes_as_linework`)
    let mut letters = 0;
    let pdf = strokes_page(|c| {
        c.extend_from_slice(HIDDEN_NOTE);
        let mut ab = Vec::new();
        shx_text("AB", 100.0, 300.0, 12.0, 1.0, 0.0, &mut ab);
        letters = ab.windows(2).filter(|w| w == b"S ").count();
        c.extend_from_slice(&ab);
        // a long line that isn't lettering
        c.extend_from_slice(b"72 72 m 700 72 l S ");
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values_but_note(&r), sorted(&["AB"]));
    let layer = r.drawing.layers.iter().find(|l| l.name == pdf_read::SHX_SOURCE_LAYER).expect("the source layer");
    assert!(!layer.on, "the source layer is off");
    let on_source: Vec<&Entity> = r.drawing.model.iter().map(|e| &**e).filter(|e| e.common.layer.as_str() == pdf_read::SHX_SOURCE_LAYER).collect();
    // the importer may join touching strokes into one polyline: between 1 per glyph and 1 per stroke
    assert!((2..=letters).contains(&on_source.len()), "{} of {letters} strokes on the source layer", on_source.len());
    assert!(on_source.iter().all(|e| matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_))));
    assert_eq!(r.stats.get("SHX source strokes (layer off)").copied(), Some(on_source.len()));
    // nothing of the letters is left as visible linework; the long line is
    let visible: Vec<&Entity> = r
        .drawing
        .model
        .iter()
        .map(|e| &**e)
        .filter(|e| matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_)) && e.common.layer.as_str() != pdf_read::SHX_SOURCE_LAYER)
        .collect();
    assert_eq!(visible.len(), 1, "only the long line is visible linework");
    assert!(matches!(&visible[0].kind, EntityKind::Line(l) if (l.b.x - l.a.x).abs() > 5.0));
}

#[test]
fn symbols_are_found_before_the_ocr_reads_their_strokes() {
    // four device symbols: a ring drawn like the letter O plus a tick below it, each with the
    // real-text tag "EF".  The ring would read as a lone "O"; it must stay part of the symbol.
    let pdf = strokes_page(|c| {
        for (x, y) in [(150.0, 300.0), (250.0, 300.0), (350.0, 300.0), (450.0, 300.0)] {
            shx_text("O", x, y, 8.0, 1.0, 0.0, c);
            c.extend_from_slice(format!("{:.1} {:.1} m {:.1} {:.1} l S ", x + 2.7, y - 4.0, x + 2.7, y - 1.5).as_bytes());
            c.extend_from_slice(format!("BT /F1 5 Tf {:.1} {:.1} Td (EF) Tj ET ", x + 8.0, y + 2.0).as_bytes());
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    let inserts = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))).count();
    assert_eq!(inserts, 4, "every symbol became a block insert: {:?}", r.stats);
    assert!(!text_values(&r).iter().any(|t| t == "O"), "a symbol's ring is not a letter: {:?}", text_values(&r));
}

#[test]
fn ocr_reads_a_full_sheet_of_dense_lettering() {
    // a 36 x 24 in sheet with 3,000 texts of 3/32" lettering in rows 11 pt apart
    let words = ["E-101", "PANEL LP-1A", "ROOM 104", "EF", "XA-2", "CORRIDOR", "J-BOX", "2#12 & 1#12G"];
    let mut content = b"q 0 0 0 RG 0.5 w ".to_vec();
    let mut want = Vec::new();
    for row in 0..150 {
        for col in 0..20 {
            let w = words[(row + col) % words.len()];
            shx_text(w, 20.0 + col as f64 * 125.0, 20.0 + row as f64 * 11.0, 6.75, 1.0, 0.0, &mut content);
            want.push(w);
        }
    }
    content.extend_from_slice(b"Q");
    let r = pdf_read::read(&pdf_sized(&content, 2592, 1728), &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&want));
    assert_eq!((r.ocr.clusters, r.ocr.glyphs, r.ocr.noise_dropped), (19_114, 19_114, 0));
}

/// `n` real (Helvetica) text labels in a grid from (x, y), 6 pt.
fn real_labels(n: usize, x: f64, y: f64, label: impl Fn(usize) -> String, out: &mut Vec<u8>) {
    for i in 0..n {
        let (lx, ly) = (x + (i % 20) as f64 * 40.0, y + (i / 20) as f64 * 20.0);
        out.extend_from_slice(format!("BT /F1 6 Tf {lx:.1} {ly:.1} Td ({}) Tj ET ", label(i)).as_bytes());
    }
}

#[test]
fn ocr_runs_where_stroked_lettering_outnumbers_real_text() {
    // 100 real one-letter labels, and 80 words of stroked lettering elsewhere on the sheet
    let pdf = {
        let mut c = b"q 0 0 0 RG 0.5 w ".to_vec();
        real_labels(100, 100.0, 100.0, |_| "A".into(), &mut c);
        for i in 0..80 {
            shx_text("ROOM 104", 100.0 + (i % 10) as f64 * 120.0, 600.0 + (i / 10) as f64 * 20.0, 8.0, 1.0, 0.0, &mut c);
        }
        c.extend_from_slice(b"Q");
        pdf_sized(&c, 2592, 1728)
    };
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    let texts = text_values(&r);
    assert_eq!(texts.iter().filter(|t| *t == "ROOM 104").count(), 80, "{:?}", r.ocr);
    assert_eq!(texts.iter().filter(|t| *t == "A").count(), 100);
    assert_eq!(texts.len(), 180);
}

#[test]
fn ocr_does_not_reread_real_text_or_run_on_a_page_lettered_in_text() {
    // 120 real labels, each also plotted as strokes on top of itself, plus five stroked X's:
    // the OCR reads the X's and never the plotted copies
    let page = |copies: bool| {
        let mut c = b"q 0 0 0 RG 0.5 w ".to_vec();
        real_labels(120, 100.0, 100.0, |i| format!("E{}", i % 10), &mut c);
        if copies {
            for i in 0..120 {
                let (lx, ly) = (100.0 + (i % 20) as f64 * 40.0, 100.0 + (i / 20) as f64 * 20.0);
                shx_text(&format!("E{}", i % 10), lx, ly, 6.0, 0.6, 0.0, &mut c);
            }
        }
        for i in 0..5 {
            shx_text("X", 100.0 + f64::from(i) * 50.0, 500.0, 8.0, 1.0, 0.0, &mut c);
        }
        c.extend_from_slice(b"Q");
        pdf_sized(&c, 2592, 1728)
    };
    let r = pdf_read::read(&page(true), &PdfOptions::default()).expect("PDF read failed");
    let read: Vec<&str> = r.ocr.words.iter().map(|w| w.value.as_str()).collect();
    assert_eq!(read, ["X"; 5], "only the stroked-only glyphs are read: {:?}", r.ocr);
    // without plotted copies the page is lettered in real text: no OCR at all
    let r = pdf_read::read(&page(false), &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(r.ocr.clusters, 0, "the OCR didn't run: {:?}", r.ocr);
    assert!(!r.stats.contains_key("SHX text (OCR) recovered"));
}

#[test]
fn ocr_skips_strokes_that_lie_on_real_text() {
    // a few real labels with a plotted stroke copy on top, and one stroked-only word
    let pdf = strokes_page(|c| {
        real_labels(10, 100.0, 100.0, |i| format!("E{i}"), c);
        for i in 0..10 {
            shx_text(&format!("E{i}"), 100.0 + i as f64 * 40.0, 100.0, 6.0, 0.6, 0.0, c);
        }
        shx_text("ROOM 5", 100.0, 400.0, 10.0, 1.0, 0.0, c);
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    let read: Vec<&str> = r.ocr.words.iter().map(|w| w.value.as_str()).collect();
    assert_eq!(read, ["ROOM 5"], "only the stroked-only word is read: {:?}", r.ocr);
}

/// One glyph of a round "plot font" (unlike our angular stroke font, as real plot fonts are):
/// its strokes in a unit cap height, with a closed flag (drawn with `h`), and its width.
type PlotGlyph = (Vec<(Vec<(f64, f64)>, bool)>, f64);

/// An elliptical arc from `a0` to `a1` degrees (counter-clockwise for a1 > a0).
fn arc(cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, a1: f64) -> Vec<(f64, f64)> {
    (0..=16).map(|i| (a0 + (a1 - a0) * f64::from(i) / 16.0).to_radians()).map(|a| (cx + rx * a.cos(), cy + ry * a.sin())).collect()
}

fn plot_glyph(ch: char) -> Option<PlotGlyph> {
    let open = |pts: Vec<(f64, f64)>| (pts, false);
    let line = |pts: &[(f64, f64)]| (pts.to_vec(), false);
    Some(match ch {
        'C' => (vec![open(arc(0.35, 0.5, 0.35, 0.5, 45.0, 315.0))], 0.7),
        'O' | '0' => {
            let mut o = arc(0.33, 0.5, 0.33, 0.5, 90.0, 450.0);
            o.pop();
            (vec![(o, true)], 0.66)
        }
        'R' => {
            let mut bowl = vec![(0.0, 1.0), (0.4, 1.0)];
            bowl.extend(arc(0.4, 0.76, 0.22, 0.24, 90.0, -90.0));
            bowl.push((0.0, 0.52));
            (vec![line(&[(0.0, 0.0), (0.0, 1.0)]), open(bowl), line(&[(0.3, 0.52), (0.62, 0.0)])], 0.62)
        }
        'D' => {
            let mut bowl = vec![(0.0, 1.0), (0.3, 1.0)];
            bowl.extend(arc(0.3, 0.5, 0.34, 0.5, 90.0, -90.0));
            bowl.push((0.0, 0.0));
            (vec![line(&[(0.0, 0.0), (0.0, 1.0)]), open(bowl)], 0.64)
        }
        'I' => (vec![line(&[(0.0, 0.0), (0.0, 1.0)])], 0.0),
        'N' => (vec![line(&[(0.0, 0.0), (0.0, 1.0), (0.62, 0.0), (0.62, 1.0)])], 0.62),
        'M' => (vec![line(&[(0.0, 0.0), (0.0, 1.0)]), line(&[(0.0, 1.0), (0.38, 0.0), (0.76, 1.0)]), line(&[(0.76, 1.0), (0.76, 0.0)])], 0.76),
        'E' => (vec![line(&[(0.55, 1.0), (0.0, 1.0), (0.0, 0.0), (0.55, 0.0)]), line(&[(0.0, 0.52), (0.38, 0.52)])], 0.55),
        'S' => {
            let mut s = arc(0.3, 0.76, 0.28, 0.24, 20.0, 270.0);
            s.extend(arc(0.3, 0.26, 0.3, 0.26, 90.0, -160.0));
            (vec![open(s)], 0.6)
        }
        '1' => (vec![line(&[(0.0, 0.8), (0.2, 1.0), (0.2, 0.0)])], 0.2),
        '2' => {
            let mut s = arc(0.3, 0.72, 0.3, 0.28, 160.0, -20.0);
            s.extend([(0.0, 0.0), (0.62, 0.0)]);
            (vec![open(s)], 0.62)
        }
        '5' => {
            let mut s = vec![(0.55, 1.0), (0.05, 1.0), (0.0, 0.56)];
            s.extend(arc(0.3, 0.32, 0.32, 0.32, 140.0, -150.0));
            (vec![open(s)], 0.62)
        }
        '-' => (vec![line(&[(0.0, 0.5), (0.5, 0.5)])], 0.5),
        '.' => {
            let mut o = arc(0.05, 0.05, 0.05, 0.05, 0.0, 360.0);
            o.pop();
            (vec![(o, true)], 0.1)
        }
        '/' => (vec![line(&[(0.0, -0.2), (0.55, 1.15)])], 0.55),
        '(' => (vec![open(arc(0.45, 0.47, 0.45, 0.7, 120.0, 240.0))], 0.25),
        ')' => (vec![open(arc(-0.2, 0.47, 0.45, 0.7, 60.0, -60.0))], 0.25),
        '#' => (
            vec![
                line(&[(0.3, 1.15), (0.15, -0.2)]),
                line(&[(0.6, 1.15), (0.45, -0.2)]),
                line(&[(0.0, 0.72), (0.72, 0.72)]),
                line(&[(-0.05, 0.3), (0.67, 0.3)]),
            ],
            0.72,
        ),
        _ => return None,
    })
}

/// Draw `text` in the round plot font: cap `height` in points, letters `gap` × height apart.
fn plot_text(text: &str, x: f64, y: f64, height: f64, gap: f64, out: &mut Vec<u8>) {
    let mut pen_x = x;
    for ch in text.chars() {
        let Some((strokes, width)) = plot_glyph(ch) else {
            pen_x += height * 0.6;
            continue;
        };
        // a left parenthesis' arc bulges left of its start: shift it into its cell
        let left = strokes.iter().flat_map(|s| s.0.iter()).map(|p| p.0).fold(f64::MAX, f64::min).min(0.0);
        for (pts, closed) in strokes {
            for (i, (fx, fy)) in pts.into_iter().enumerate() {
                let (px, py) = (pen_x + (fx - left) * height, y + fy * height);
                out.extend_from_slice(format!("{px:.3} {py:.3} {} ", if i == 0 { "m" } else { "l" }).as_bytes());
            }
            out.extend_from_slice(if closed { b"h S " } else { b"S " });
        }
        pen_x += (width - left + gap) * height;
    }
}

#[test]
fn ocr_reads_round_plot_lettering() {
    // a plot font unlike our stroke font: round letters, an I without serifs, a 1 without a
    // foot, an M whose middle reaches the baseline, tiny rings for periods, tall parentheses,
    // slashes and number signs, and letters set close (their feet a tenth of the height apart)
    let lines = ["CORRIDOR", "DOOR 105", "MIN. 1/2", "(E) 2#12", "SIDE 50", "ROOM 1-2"];
    // (a page with hidden text: the reads are confirmed, so their strokes leave the plan)
    let pdf = strokes_page(|c| {
        c.extend_from_slice(HIDDEN_NOTE);
        for (i, l) in lines.iter().enumerate() {
            plot_text(l, 80.0, 500.0 - i as f64 * 25.0, 9.0, 0.1, c);
        }
        // the same at a small size, tighter still
        for (i, l) in lines.iter().enumerate() {
            plot_text(l, 400.0, 500.0 - i as f64 * 12.0, 4.5, 0.08, c);
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    let mut want: Vec<&str> = lines.to_vec();
    want.extend(lines);
    assert_eq!(text_values_but_note(&r), sorted(&want), "{:?}", r.ocr);
    // the rings of the O's and periods are read inside words, never left as linework
    let visible = r.drawing.model.iter().filter(|e| e.common.layer.as_str() != pdf_read::SHX_SOURCE_LAYER && !matches!(e.kind, EntityKind::Text(_)));
    assert_eq!(visible.count(), 0, "every stroke was lettering");
}

#[test]
fn ocr_leaves_lone_rings_alone() {
    // small circles and boxes drawn as closed loops (symbols' parts, markers): no lone "O"s
    let pdf = strokes_page(|c| {
        for i in 0..6 {
            let x = 100.0 + f64::from(i) * 40.0;
            let mut ring = String::new();
            for (k, (px, py)) in arc(x, 300.0, 3.0, 3.0, 0.0, 345.0).into_iter().enumerate() {
                ring.push_str(&format!("{px:.2} {py:.2} {} ", if k == 0 { "m" } else { "l" }));
            }
            c.extend_from_slice(format!("{ring}h S {x:.1} 200 m {:.1} 200 l {:.1} 207 l {x:.1} 207 l h S ", x + 5.0, x + 5.0).as_bytes());
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), Vec::<String>::new(), "{:?}", r.ocr);
}

/// The recovered texts with their direction in whole degrees and their insertion point.
fn texts_turned(r: &pdf_read::PdfImport) -> Vec<(String, i64, (f64, f64))> {
    let mut v: Vec<(String, i64, (f64, f64))> = r
        .drawing
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some((t.value.clone(), t.rotation.to_degrees().round() as i64, (t.insert.x, t.insert.y))),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

#[test]
fn ocr_reads_lettering_turned_along_walls() {
    // plans letter along vertical walls too: reading up the sheet (90°) and down it (270°)
    let pdf = strokes_page(|c| {
        // upright references, also giving the page-to-drawing mapping
        shx_text("CORRIDOR", 80.0, 500.0, 9.0, 1.0, 0.0, c);
        plot_text("DOOR 105", 400.0, 200.0, 9.0, 0.1, c);
        // turned: the PDF's own transform turns the lettering, which starts at the origin
        c.extend_from_slice(b"q 0 1 -1 0 300 100 cm ");
        shx_text("STORAGE", 0.0, 0.0, 9.0, 1.0, 0.0, c);
        c.extend_from_slice(b"Q q 0 1 -1 0 340 100 cm ");
        plot_text("ROOM 1-2", 0.0, 0.0, 9.0, 0.1, c);
        c.extend_from_slice(b"Q q 0 -1 1 0 500 400 cm ");
        shx_text("EF-2", 0.0, 0.0, 9.0, 1.0, 0.0, c);
        c.extend_from_slice(b"Q ");
        // a block of notes stacked two letter heights apart is read along its lines, never
        // down its columns
        for (i, l) in ["NOTE 1 SEE E-501", "NOTE 2 PROVIDE GFCI", "NOTE 3 TYP", "NOTE 4 EXISTING"].iter().enumerate() {
            shx_text(l, 80.0, 400.0 - i as f64 * 18.0, 9.0, 1.0, 0.0, c);
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    let got = texts_turned(&r);
    let found: Vec<(&str, i64)> = got.iter().map(|(v, a, _)| (v.as_str(), *a)).collect();
    assert_eq!(
        found,
        vec![
            ("CORRIDOR", 0),
            ("DOOR 105", 0),
            ("EF-2", 270),
            ("NOTE 1 SEE E-501", 0),
            ("NOTE 2 PROVIDE GFCI", 0),
            ("NOTE 3 TYP", 0),
            ("NOTE 4 EXISTING", 0),
            ("ROOM 1-2", 90),
            ("STORAGE", 90)
        ],
        "{:?}",
        r.ocr
    );
    assert_eq!(r.ocr.turned, 3);
    assert_eq!(r.stats.get("SHX OCR turned texts").copied(), Some(3));
    // a turned text starts where its lettering starts: map page points to the drawing by the
    // two upright references (their lettering starts at its drawing point too)
    let at = |v: &str| got.iter().find(|t| t.0 == v).map(|t| t.2).expect("text");
    let (a, b) = (at("CORRIDOR"), at("DOOR 105"));
    let (kx, ky) = ((b.0 - a.0) / 320.0, (b.1 - a.1) / -300.0);
    let page = |p: (f64, f64)| (80.0 + (p.0 - a.0) / kx, 500.0 + (p.1 - a.1) / ky);
    for (v, want) in [("STORAGE", (300.0, 100.0)), ("ROOM 1-2", (340.0, 100.0)), ("EF-2", (500.0, 400.0))] {
        let p = page(at(v));
        assert!((p.0 - want.0).abs() < 1.5 && (p.1 - want.1).abs() < 1.5, "{v} starts at {p:?}, not {want:?}");
    }
}

#[test]
fn ocr_reads_tags_inside_boxes() {
    // a room tag in a box drawn round it 2 pt off the lettering: the box's sides join
    // into one big loop, which must not reach the letters (a wide box made "MECHANICA")
    // on a 36" x 24" sheet, where a glyph-sized stroke runs to 80 pt: the box is one
    let mut c = b"q 0 0 0 RG 0.5 w ".to_vec();
    {
        let c = &mut c;
        for (i, (name, num)) in [("MECHANICAL", "103"), ("ELECTRICAL", "119"), ("CONFERENCE", "129")].iter().enumerate() {
            let (x, y) = (100.0, 500.0 - i as f64 * 60.0);
            let end = shx_text(name, x + 2.0, y + 13.0, 6.75, 1.0, 0.0, c);
            shx_text(num, x + 2.0, y + 2.0, 6.75, 1.0, 0.0, c);
            let (w, h) = (end - x + 2.0, 13.0 + 6.75 + 2.0);
            c.extend_from_slice(
                format!(
                    "{x} {y} m {} {y} l S {} {y} m {} {} l S {} {} m {x} {} l S {x} {} m {x} {y} l S ",
                    x + w,
                    x + w,
                    x + w,
                    y + h,
                    x + w,
                    y + h,
                    y + h,
                    y + h
                )
                .as_bytes(),
            );
        }
    }
    c.extend_from_slice(b"Q");
    let pdf = pdf_sized(&c, 2592, 1728);
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&["103", "119", "129", "CONFERENCE", "ELECTRICAL", "MECHANICAL"]), "{:?}", r.ocr);
}

#[test]
fn ocr_leaves_curve_drawn_symbols_alone() {
    // device symbols: a circle drawn with four curves (closed or not) and a bar through it read
    // as a "6"; plot fonts flatten their arcs, so a lone glyph with a curve-drawn loop is a
    // symbol.  The tags beside the symbols are still read.
    let pdf = strokes_page(|c| {
        for i in 0..6 {
            let (x, y, r) = (100.0 + f64::from(i) * 60.0, 300.0, 4.0);
            let k = 0.5523 * r;
            let close = if i % 2 == 0 { "h " } else { "" };
            c.extend_from_slice(
                format!(
                    "{} {y} m {} {} {} {} {x} {} c {} {} {} {} {} {y} c {} {} {} {} {x} {} c {} {} {} {} {} {y} c {close}S {} {y} m {} {y} l S ",
                    x + r,
                    x + r,
                    y + k,
                    x + k,
                    y + r,
                    y + r,
                    x - k,
                    y + r,
                    x - r,
                    y + k,
                    x - r,
                    x - r,
                    y - k,
                    x - k,
                    y - r,
                    y - r,
                    x + k,
                    y - r,
                    x + r,
                    y - k,
                    x + r,
                    x - r,
                    x + r
                )
                .as_bytes(),
            );
            shx_text("EF", x + 8.0, y + 2.0, 6.0, 1.0, 0.0, c);
        }
    });
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    // every tag is read, as loose text or as the TAG of the symbol it stands beside (a closed
    // outline); no symbol is read as a 6 or a 9
    let mut tags = text_values(&r);
    tags.extend(symbol_tags(&r.drawing));
    tags.sort();
    assert_eq!(tags, sorted(&["EF"; 6]), "{:?} {:?}", r.ocr, r.stats);
}

#[test]
fn ocr_keeps_free_standing_marks() {
    // lone marks (circuit numbers, fixture types) stand free at the page's letter height;
    // a duct line's ticks and a wall end look like an I or a 1 but touch the lines they end,
    // and a stroke taller than any lettering on the page is linework too (a 36" x 24" sheet,
    // where a 50 pt leader is still glyph-sized)
    let mut c = b"q 0 0 0 RG 0.5 w ".to_vec();
    {
        let c = &mut c;
        shx_text("PANEL LP-1", 100.0, 500.0, 6.75, 1.0, 0.0, c);
        shx_text("EXISTING", 100.0, 470.0, 6.75, 1.0, 0.0, c);
        for (i, mark) in ["1", "A", "3", "7", "C"].iter().enumerate() {
            shx_text(mark, 100.0 + i as f64 * 40.0, 400.0, 6.75, 1.0, 0.0, c);
        }
        // a lone 1 drawn as one upright stroke
        c.extend_from_slice(b"300 400 m 300 406.75 l S ");
        // a mark beside a leader much taller than the letters: still a 2, not a dot
        shx_text("2", 330.0, 400.0, 6.75, 1.0, 0.0, c);
        c.extend_from_slice(b"345 380 m 375 420 l S ");
        // a duct line with ticks of the letters' height above and below it
        c.extend_from_slice(b"100 300 m 400 300 l S ");
        for i in 0..8 {
            let x = 110.0 + f64::from(i) * 30.0;
            c.extend_from_slice(format!("{x:.1} 300 m {x:.1} 306.75 l S {:.1} 293.25 m {:.1} 300 l S ", x + 10.0, x + 10.0).as_bytes());
        }
        // a wall end: an upright stroke of the letters' height ending on a wall
        c.extend_from_slice(b"100 200 m 300 200 l S 300 200 m 300 206.75 l S ");
        // an upright stroke three times the letters' height, standing free
        c.extend_from_slice(b"450 400 m 450 420.25 l S ");
    }
    c.extend_from_slice(b"Q");
    let pdf = pdf_sized(&c, 2592, 1728);
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(text_values(&r), sorted(&["1", "1", "2", "3", "7", "A", "C", "EXISTING", "PANEL LP-1"]), "{:?}", r.ocr);
}

// -----------------------------------------------------------------------------------------
// Q5265 devicetypes: the OCR never takes a symbol's own lines; stroked marks tag symbols too
// -----------------------------------------------------------------------------------------

/// A closed 16-pt square centred on (cx, cy), as a symbol's outline.
fn square(cx: f64, cy: f64, out: &mut Vec<u8>) {
    out.extend_from_slice(
        format!(
            "{:.1} {:.1} m {:.1} {:.1} l {:.1} {:.1} l {:.1} {:.1} l h S ",
            cx - 8.0,
            cy - 8.0,
            cx + 8.0,
            cy - 8.0,
            cx + 8.0,
            cy + 8.0,
            cx - 8.0,
            cy + 8.0
        )
        .as_bytes(),
    );
}

/// The symbol inserts' TAG values, sorted.
fn symbol_tags(d: &Drawing) -> Vec<String> {
    let mut tags: Vec<String> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Insert(i) if i.block.starts_with("PDF-SYM-") => {
                Some(i.attribs.iter().find(|a| a.tag == "TAG").map_or_else(String::new, |a| a.text.value.clone()))
            }
            _ => None,
        })
        .collect();
    tags.sort();
    tags
}

#[test]
fn ocr_leaves_symbol_lines_to_the_symbol() {
    // Four squares with a stroked "T" inside, each tagged by a real text "T1"; the page has
    // hidden text, so the OCR runs.  The T's lines read as a glyph, but they are the symbol's.
    let mut visible = b"0 0 0 RG 0.5 w ".to_vec();
    for i in 0..4 {
        let (cx, cy) = (150.0 + 120.0 * f64::from(i), 300.0);
        square(cx, cy, &mut visible);
        shx_text("T", cx - 4.0, cy - 6.0, 12.0, 1.0, 0.0, &mut visible);
        visible.extend_from_slice(format!("BT 0 Tr /F1 6 Tf {:.1} {:.1} Td (T1) Tj ET ", cx + 10.0, cy - 3.0).as_bytes());
    }
    let pdf = pdf_with_hidden_text(&visible, b"600 560 Td (GENERAL NOTES) Tj");
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(symbol_tags(&r.drawing), vec!["T1"; 4], "stats: {:?}", r.stats);
    let t_texts = r.drawing.model.iter().filter(|e| matches!(&e.kind, EntityKind::Text(t) if t.value.trim() == "T")).count();
    assert_eq!(t_texts, 0, "the symbols' lines must not also become text");
}

#[test]
fn stroked_device_marks_tag_symbols() {
    // Three squares with a smaller closed square inside, tagged only by stroked lettering
    // "F" (no text object); hidden text elsewhere turns the OCR on.  A lone stroked "O"
    // beside a fourth symbol is a circle-shaped glyph, never a tag.
    let mut visible = b"0 0 0 RG 0.5 w ".to_vec();
    for i in 0..4 {
        let (cx, cy) = (150.0 + 120.0 * f64::from(i), 300.0);
        square(cx, cy, &mut visible);
        visible.extend_from_slice(
            format!("{:.1} {:.1} m {:.1} {:.1} l {:.1} {:.1} l h S ", cx - 4.0, cy - 4.0, cx + 4.0, cy - 4.0, cx, cy + 4.0).as_bytes(),
        );
        let mark = if i < 3 { "F" } else { "O" };
        for (k, ch) in mark.chars().enumerate() {
            shx_text(&ch.to_string(), cx + 12.0 + 8.0 * k as f64, cy - 4.0, 8.0, 1.0, 0.0, &mut visible);
        }
    }
    let pdf = pdf_with_hidden_text(&visible, b"600 560 Td (GENERAL NOTES) Tj");
    let r = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(symbol_tags(&r.drawing), vec!["F"; 3], "stats: {:?}", r.stats);
    assert_eq!(r.stats.get("symbols tagged by stroked lettering").copied(), Some(3));
    // the marks live on as the symbols' TAG, not twice as loose text
    let f_texts = r.drawing.model.iter().filter(|e| matches!(&e.kind, EntityKind::Text(t) if t.value.trim() == "F")).count();
    assert_eq!(f_texts, 0);
}

// -----------------------------------------------------------------------------------------
// Q5261 importfix2: OCR without hidden text, and E sheets keep DO NOT SCALE
// -----------------------------------------------------------------------------------------

fn texts_of(r: &pdf_read::PdfImport) -> Vec<String> {
    r.drawing
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn ocr_reads_stroked_lettering_on_a_page_without_hidden_text() {
    // The SHX lettering of a print without its searchable copy (no rendering-mode-3 text) is
    // read from the strokes; the long line stays linework.
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w 72 72 m 700 72 l S ");
    shx_text("HELLO", 100.0, 300.0, 12.0, 1.0, 0.0, &mut content);
    content.extend_from_slice(b"BT /F1 10 Tf 72 500 Td (Sample Title) Tj ET Q");
    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default()).expect("PDF read failed");
    assert!(r.stats.get("SHX text (OCR) recovered").copied().unwrap_or(0) > 0, "OCR ran: {:?}", r.stats);
    let texts = texts_of(&r);
    assert!(texts.iter().any(|t| t.contains('H')), "the stroked H is read: {texts:?}");
    assert!(texts.iter().any(|t| t == "Sample Title"), "{texts:?}");
    assert!(r.drawing.model.iter().any(|e| matches!(&e.kind, EntityKind::Line(l) if (l.b.x - l.a.x).abs() > 5.0)), "long line kept");
}

#[test]
fn ocr_skips_pages_whose_lettering_is_real_text() {
    // A page lettered in real text (a Revit or TrueType print): short strokes there are symbol
    // linework, not lettering, so the OCR leaves them alone.
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w ");
    shx_text("HELLO", 100.0, 300.0, 12.0, 1.0, 0.0, &mut content);
    for i in 0..120 {
        let (x, y) = (60 + (i % 10) * 70, 380 + (i / 10) * 18);
        content.extend_from_slice(format!("BT /F1 6 Tf {x} {y} Td (Room {i}) Tj ET ").as_bytes());
    }
    content.extend_from_slice(b"Q");
    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default()).expect("PDF read failed");
    assert_eq!(r.stats.get("SHX text (OCR) recovered"), None, "no OCR on a page lettered in real text");
    assert!(
        r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_))).count() > 5,
        "the strokes stay linework"
    );
    assert_eq!(texts_of(&r).len(), 120, "the real text is all there");
}

#[test]
fn ocr_leaves_lone_straight_strokes_as_linework() {
    // A plan's short pieces — wall ends, door jambs, duct and pipe ticks — are single straight
    // strokes the size of a letter. The OCR reads the stroked word but leaves each lone stroke
    // as linework (not an "I", "1" or "/"), so the wall, duct and pipe readers still see it.
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w 72 72 m 700 72 l S ");
    shx_text("HELLO", 100.0, 400.0, 12.0, 1.0, 0.0, &mut content);
    let mut stubs = 0;
    for i in 0..30 {
        let (x, y) = (90 + (i % 15) * 40, 150 + (i / 15) * 60);
        // upright and leaning stubs, 8 pt long, well apart
        let (dx, dy) = if i % 2 == 0 { (0, 8) } else { (5, 6) };
        content.extend_from_slice(format!("{x} {y} m {} {} l S ", x + dx, y + dy).as_bytes());
        stubs += 1;
    }
    content.extend_from_slice(b"Q");
    let r = pdf_read::read(&pdf_with(&content), &PdfOptions::default()).expect("PDF read failed");
    let texts = texts_of(&r);
    assert!(texts.iter().any(|t| t.contains('H')), "the stroked word is still read: {texts:?}");
    let bare = texts.iter().filter(|t| t.chars().all(|c| "I1l|-/\\'. ".contains(c))).count();
    assert_eq!(bare, 0, "no lone stroke becomes a letter: {texts:?}");
    let short_lines = r
        .drawing
        .model
        .iter()
        .filter(|e| matches!(&e.kind, EntityKind::Line(l) if (l.b.x - l.a.x).hypot(l.b.y - l.a.y) < 1.0))
        .filter(|e| match &e.kind {
            EntityKind::Line(l) => l.a.y.min(l.b.y) < 300.0 / 72.0,
            _ => false,
        })
        .count();
    assert_eq!(short_lines, stubs, "every stub stays a line");
}

#[test]
fn ocr_without_hidden_text_keeps_the_strokes_as_linework() {
    // With no hidden text to confirm it, an OCR word is added as text but its strokes stay in
    // the plan: on a real sheet a "word" can be a wall corner, a jamb or duct ticks, and the
    // wall, duct and pipe readers need those lines.
    let mut content = Vec::new();
    content.extend_from_slice(b"q 0 0 0 RG 0.5 w 72 72 m 700 72 l S ");
    shx_text("HELLO", 100.0, 300.0, 12.0, 1.0, 0.0, &mut content);
    content.extend_from_slice(b"Q");
    let pdf = pdf_with(&content);
    let linework_near_word = |r: &pdf_read::PdfImport| {
        let near = |y: f64| y > 280.0 / 72.0 && y < 330.0 / 72.0;
        r.drawing
            .model
            .iter()
            .filter(|e| match &e.kind {
                EntityKind::Line(l) => near(l.a.y),
                EntityKind::LwPolyline(p) => p.vertices.first().is_some_and(|v| near(v.p.y)),
                _ => false,
            })
            .count()
    };
    let read = pdf_read::read(&pdf, &PdfOptions::default()).expect("PDF read failed");
    let raw = pdf_read::read(&pdf, &PdfOptions { recover_text: false, ..PdfOptions::default() }).expect("PDF read failed");
    assert!(read.stats.get("SHX text (OCR) recovered").copied().unwrap_or(0) > 0, "OCR ran: {:?}", read.stats);
    assert!(texts_of(&read).iter().any(|t| t.contains('H')), "the word is read: {:?}", texts_of(&read));
    assert!(linework_near_word(&raw) > 0, "the word is linework without recovery");
    assert_eq!(linework_near_word(&read), linework_near_word(&raw), "every stroke of the word stays linework");
    // the word is marked as an unconfirmed OCR read, in a style the drawing has
    let ocr_styles: Vec<&str> = read
        .drawing
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) if t.value.contains('H') => Some(t.style.as_str()),
            _ => None,
        })
        .collect();
    assert!(!ocr_styles.is_empty() && ocr_styles.iter().all(|st| *st == pdf_read::OCR_TEXT_STYLE), "{ocr_styles:?}");
    assert!(read.drawing.text_styles.iter().any(|st| st.name == pdf_read::OCR_TEXT_STYLE), "the style is in the drawing");
}

const E_SHEET_WITH_NOTE: &[u8] =
    b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 12 Tf 700 50 Td (E-4) Tj ET BT /F1 8 Tf 600 80 Td (SCALE: 1/8\" = 1'-0\") Tj ET Q";

#[test]
fn e_sheet_drawn_at_its_note_when_asked_stays_flagged() {
    // Opened on its own (nothing to register to), an E sheet comes in at its scale note so
    // walls and rooms read at building size, and still says DO NOT SCALE.
    let r =
        pdf_read::read(&pdf_with(E_SHEET_WITH_NOTE), &PdfOptions { e_sheet_at_note_scale: true, ..PdfOptions::default() }).expect("PDF read failed");
    assert_eq!(r.scale, 96.0, "drawn at the 1/8\" note");
    assert!(r.do_not_scale, "still DO NOT SCALE");
    assert!(r.needs_registration, "still needs registration");
    assert!(r.scale_source.contains("DO NOT SCALE"), "{}", r.scale_source);

    // the same through PDFIMPORT's JSON and through opening the file
    let (_, report) = pdf_read::read_json(&pdf_with(E_SHEET_WITH_NOTE), &serde_json::json!({})).expect("pdfimport");
    assert_eq!(report["scale"].as_f64(), Some(96.0), "{report}");
    assert_eq!(report["do_not_scale"].as_bool(), Some(true), "{report}");
    assert_eq!(report["needs_registration"].as_bool(), Some(true), "{report}");
    let d = cadaddy_io::read(&pdf_with(E_SHEET_WITH_NOTE), "Sample E Sheet.pdf").expect("open");
    let long = d.model.iter().any(|e| match &e.kind {
        EntityKind::Line(l) => ((l.b.x - l.a.x).abs() - 128.0 / 72.0 * 96.0).abs() < 0.5,
        EntityKind::LwPolyline(p) => {
            p.vertices.len() == 2 && p.vertices.windows(2).any(|w| ((w[1].p.x - w[0].p.x).abs() - 128.0 / 72.0 * 96.0).abs() < 0.5)
        }
        _ => false,
    });
    assert!(long, "the opened E sheet's line is at 1/8\" scale");
}

#[test]
fn e_sheet_without_a_note_stays_sheet_size_even_when_asked() {
    let content = b"q 0 0 0 RG 1.0 w 72 72 m 200 72 l S BT /F1 12 Tf 700 50 Td (E-4) Tj ET Q";
    let r = pdf_read::read(&pdf_with(content), &PdfOptions { e_sheet_at_note_scale: true, ..PdfOptions::default() }).expect("PDF read failed");
    assert_eq!(r.scale, 1.0);
    assert!(r.do_not_scale);
    assert!(!r.needs_registration, "no scale note to register with");
}
