//! Room tags a plan prints but carries no text for: one room's number is text converted to
//! filled outlines, the other's is set in a Type3 font whose glyphs nothing names (no ToUnicode
//! map, glyph names like `/g1`). The import reads both as lettering, and the rooms are numbered
//! from them as from any tag.
//!
//! The plan, its glyphs and its numbers are made up.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A closed ellipse, counter-clockwise (clockwise: a hole).
fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64, clockwise: bool) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = (0..32)
        .map(|i| {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect();
    if clockwise {
        pts.reverse();
    }
    pts
}

/// Made-up digit outlines in a 44 × 70 cell (cap height 70, strokes 9 wide).
fn glyph(ch: char) -> Vec<Vec<(f64, f64)>> {
    match ch {
        '0' => vec![ellipse(22.0, 35.0, 22.0, 35.0, false), ellipse(22.0, 35.0, 13.0, 26.0, true)],
        '1' => vec![vec![(24.0, 0.0), (33.0, 0.0), (33.0, 70.0), (26.0, 70.0), (10.0, 58.0), (14.0, 51.0), (24.0, 58.0)]],
        '2' => {
            // the bowl, the diagonal and the foot as overlapping quads (non-zero fill)
            let mut line: Vec<(f64, f64)> = (0..=10)
                .map(|i| {
                    let a = (160.0 - 200.0 * i as f64 / 10.0_f64).to_radians();
                    (22.0 + 18.0 * a.cos(), 50.0 + 17.0 * a.sin())
                })
                .collect();
            line.extend([(4.0, 3.0), (44.0, 3.0)]);
            line.windows(2)
                .map(|s| {
                    let ((x0, y0), (x1, y1)) = (s[0], s[1]);
                    let len = (x1 - x0).hypot(y1 - y0);
                    let (ux, uy) = ((x1 - x0) / len * 4.5, (y1 - y0) / len * 4.5);
                    vec![(x0 - ux + uy, y0 - uy - ux), (x1 + ux + uy, y1 + uy - ux), (x1 + ux - uy, y1 + uy + ux), (x0 - ux - uy, y0 - uy + ux)]
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn path(loops: &[Vec<(f64, f64)>], k: f64, at: (f64, f64)) -> String {
    let mut s = String::new();
    for l in loops {
        for (i, &(x, y)) in l.iter().enumerate() {
            s.push_str(&format!("{:.3} {:.3} {}\n", at.0 + x * k, at.1 + y * k, if i == 0 { "m" } else { "l" }));
        }
        s.push_str("h\n");
    }
    s
}

/// A one-page plan: two rooms side by side (walls as double lines, 9 pt apart), each named in
/// Helvetica; "1012" drawn as filled outlines in the first, "2010" in the Type3 font /U (codes
/// 1, 2, 3 = 0, 1, 2; FontMatrix 0.01) in the second.
fn plan_pdf() -> Vec<u8> {
    let mut c = String::from("0 0 0 RG 1 w\n");
    for (x0, y0, x1, y1) in [(36.0, 36.0, 756.0, 540.0), (45.0, 45.0, 391.5, 531.0), (400.5, 45.0, 747.0, 531.0)] {
        c.push_str(&format!("{x0} {y0} m {x1} {y0} l {x1} {y1} l {x0} {y1} l h S\n"));
    }
    c.push_str("BT /F1 10 Tf 190 300 Td (OFFICE) Tj ET\nBT /F1 10 Tf 540 300 Td (STORAGE) Tj ET\n0 0 0 rg\n");
    // cap height 7.2 pt, like the names' (10 pt Helvetica)
    let k = 7.2 / 70.0;
    for (i, ch) in "1012".chars().enumerate() {
        c.push_str(&path(&glyph(ch), k, (200.0 + i as f64 * 56.0 * k, 285.0)));
        c.push_str("f\n");
    }
    // 10.3 pt × 70/100 = 7.2 pt tall
    c.push_str("BT /U 10.3 Tf 555 285 Td <03010201> Tj ET\n");
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] /Resources << /Font << /F1 5 0 R /U 6 0 R >> >> /Contents 4 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 44 70] /FontMatrix [0.01 0 0 0.01 0 0] /CharProcs << /g1 7 0 R /g2 8 0 R /g3 9 0 R >> \
         /Encoding << /Type /Encoding /Differences [1 /g1 /g2 /g3] >> /FirstChar 1 /LastChar 3 /Widths [60 60 60] /Resources << >> >>"
            .into(),
    ];
    for ch in ['0', '1', '2'] {
        let body = format!("60 0 0 0 44 70 d1\n{}f\n", path(&glyph(ch), 1.0, (0.0, 0.0)));
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The real PDF reader behind `pdfimport`.
fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

#[test]
fn rooms_are_numbered_from_outlined_and_type3_tags() {
    real_pdf_reader();
    let dir = std::env::temp_dir().join(format!("cadaddy-roomtagtext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Floor Plan.pdf");
    std::fs::write(&path, plan_pdf()).unwrap();
    let mut s = Session::new();
    // 1/4" = 1'-0": the rooms are about 20 by 27 ft
    let r = s.execute("pdfimport", &json!({"path": path.to_string_lossy(), "scale": 48, "layout": false})).unwrap();
    let tags: Vec<Value> = s.execute("texts", &json!({"limit": 50})).unwrap()["texts"].as_array().unwrap().clone();
    let values: Vec<&str> = tags.iter().filter_map(|t| t["text"].as_str()).collect();
    assert!(values.contains(&"1012") && values.contains(&"2010"), "the tags are lettering: {values:?} ({r})");
    let rooms = s.execute("rooms.list", &json!({})).unwrap();
    let list = rooms["rooms"].as_array().unwrap();
    let number = |name: &str| list.iter().find(|r| r["name"] == name).map(|r| r["number"].clone()).unwrap_or_else(|| panic!("no {name} in {rooms}"));
    assert_eq!(number("OFFICE"), "1012", "{rooms}");
    assert_eq!(number("STORAGE"), "2010", "{rooms}");
    let _ = std::fs::remove_dir_all(&dir);
}
