//! Q5266 jobspeed: what a print-set job asks of the PDF reader — small fills merged, and a big
//! file parsed once for all its pages without mixing up two files.

use cadaddy_doc::*;
use cadaddy_io::pdf_read::{self, PdfOptions};

/// Build a minimal one-page 11×8.5 inch landscape PDF around a content stream.
fn pdf_with(content: &[u8]) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),
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

fn hatches(d: &Drawing) -> Vec<&Hatch> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Hatch(h) => Some(h.as_ref()),
            _ => None,
        })
        .collect()
}

/// A poché pattern: 400 dots of 2 pt in a grid, plus a wall's fill (30 × 4 pt) and a room's
/// tone (100 × 60 pt).
fn patterned_page() -> Vec<u8> {
    let mut c = String::from("q 0.4 0.4 0.4 rg ");
    for i in 0..20 {
        for j in 0..20 {
            c.push_str(&format!("{} {} 2 2 re f ", 300 + i * 6, 200 + j * 6));
        }
    }
    c.push_str("0.2 0.2 0.2 rg 100 100 30 4 re f 0.8 0.8 0.8 rg 500 400 100 60 re f 0 0 0 RG 1 w 72 72 m 200 72 l S Q");
    pdf_with(c.as_bytes())
}

#[test]
fn small_fills_merge_into_one_hatch_per_colour() {
    let pdf = patterned_page();
    let each = pdf_read::read(&pdf, &PdfOptions { recover_text: false, ..PdfOptions::default() }).expect("read");
    assert_eq!(hatches(&each.drawing).len(), 402, "one hatch per fill without merging");
    let merged = pdf_read::read(&pdf, &PdfOptions { recover_text: false, merge_small_fills: true, ..PdfOptions::default() }).expect("read");
    let hs = hatches(&merged.drawing);
    assert_eq!(hs.len(), 3, "the dots as one hatch, the wall and room fills kept: {}", hs.len());
    assert_eq!(hs.iter().map(|h| h.loops.len()).sum::<usize>(), 402, "no outline lost");
    assert_eq!(merged.stats.get("small fills merged"), Some(&400));
    // The wall's fill keeps its own hatch (the wall reader measures each fill's thickness).
    let thin = hs.iter().filter(|h| h.loops.len() == 1).count();
    assert_eq!(thin, 2);
    // Everything else on the page is the same.
    let lines = |d: &Drawing| d.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_))).count();
    assert_eq!(lines(&each.drawing), lines(&merged.drawing));
}

#[test]
fn pdfimport_options_turn_merging_on() {
    let pdf = patterned_page();
    let (d, report) = pdf_read::read_json(&pdf, &serde_json::json!({"fills": "merge", "recoverText": false})).expect("read");
    assert_eq!(hatches(&d).len(), 3, "{report}");
    let (d, _) = pdf_read::read_json(&pdf, &serde_json::json!({"recoverText": false})).expect("read");
    assert_eq!(hatches(&d).len(), 402, "off unless asked");
}

/// A page big enough to be kept parsed, with one line whose end is `x`.
fn big_page(x: u32) -> Vec<u8> {
    let mut c = String::from("q 0 0 0 RG 1 w ");
    for i in 0..18_000u32 {
        c.push_str(&format!("{} {} m {} {} l S\n", 72 + i % 600, 72 + i % 400, 73 + i % 600, 72 + i % 400));
    }
    c.push_str(&format!("10 10 m {x} 10 l S Q"));
    pdf_with(c.as_bytes())
}

#[test]
fn big_files_of_the_same_size_are_never_mixed_up() {
    let (a, b) = (big_page(500), big_page(600));
    assert!(a.len() > 256 * 1024, "big enough to be kept: {}", a.len());
    assert_eq!(a.len(), b.len(), "same size, different content");
    let ends = |pdf: &[u8]| -> Vec<f64> {
        let r = pdf_read::read(pdf, &PdfOptions { recover_text: false, join: false, ..PdfOptions::default() }).expect("read");
        r.drawing
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Line(l) if l.a.y.abs() < 0.2 && l.a.x < 0.2 => Some((l.b.x * 72.0).round()),
                _ => None,
            })
            .collect()
    };
    for _ in 0..2 {
        assert_eq!(ends(&a), vec![500.0]);
        assert_eq!(ends(&b), vec![600.0]);
    }
    assert_eq!(pdf_read::page_count(&a).ok(), Some(1));
    pdf_read::clear_cache();
    assert_eq!(ends(&b), vec![600.0], "read again after the cache is cleared");
}

#[test]
fn a_damaged_big_file_is_an_error_each_time() {
    let mut a = big_page(500);
    let cut = a.len() / 3;
    a.truncate(cut);
    a.resize(300 * 1024, b' ');
    for _ in 0..2 {
        // lopdf may recover a damaged file; either way the read never panics and is the same twice.
        let r = pdf_read::read(&a, &PdfOptions::default()).map(|r| r.drawing.model.len()).map_err(|e| e.to_string());
        let again = pdf_read::read(&a, &PdfOptions::default()).map(|r| r.drawing.model.len()).map_err(|e| e.to_string());
        assert_eq!(r, again);
    }
}
