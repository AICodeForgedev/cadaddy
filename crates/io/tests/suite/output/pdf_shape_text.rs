//! Lettering a PDF draws as shapes, read back as text: a plan's room tags can reach the
//! importer as text converted to filled outlines, or in a Type3 font, either with glyph names
//! that say what each glyph is (no ToUnicode map) or with glyphs only their procedures draw.
//!
//! The glyphs are made up here: digits of thick strokes and ellipse rings with real holes, at
//! several sizes, turned, and filled by either fill rule.

use cadaddy_doc::*;
use cadaddy_io::pdf_read::{self, OCR_TEXT_STYLE, PdfOptions, SHX_SOURCE_LAYER};

// ---------- made-up glyphs: a 44 × 70 cell, cap height 70 ----------

/// A closed ellipse, `n` points, counter-clockwise (or clockwise: a hole).
fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64, n: usize, clockwise: bool) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect();
    if clockwise {
        pts.reverse();
    }
    pts
}

/// A stroke of width `w` along a polyline, as one quad per segment (lengthened by half the
/// width at both ends, so the joints fill): pieces of one glyph that overlap.
fn thick(line: &[(f64, f64)], w: f64) -> Vec<Vec<(f64, f64)>> {
    line.windows(2)
        .map(|s| {
            let ((x0, y0), (x1, y1)) = (s[0], s[1]);
            let len = (x1 - x0).hypot(y1 - y0);
            let (ux, uy) = ((x1 - x0) / len * w / 2.0, (y1 - y0) / len * w / 2.0);
            let (nx, ny) = (-uy, ux);
            vec![(x0 - ux - nx, y0 - uy - ny), (x1 + ux - nx, y1 + uy - ny), (x1 + ux + nx, y1 + uy + ny), (x0 - ux + nx, y0 - uy + ny)]
        })
        .collect()
}

/// A digit's filled outline in the glyph cell, its strokes 9 wide: one contour (a 1), an
/// outline round its holes (a 0, a 4 with its counter; for the even-odd rule the holes run the
/// same way round as the outline), or overlapping pieces (a 2, for the non-zero rule).
fn glyph(ch: char, even_odd: bool) -> Vec<Vec<(f64, f64)>> {
    let w = 9.0;
    match ch {
        '0' => vec![ellipse(22.0, 35.0, 22.0, 35.0, 32, false), ellipse(22.0, 35.0, 22.0 - w, 35.0 - w, 32, !even_odd)],
        '1' => vec![vec![(24.0, 0.0), (33.0, 0.0), (33.0, 70.0), (26.0, 70.0), (10.0, 58.0), (14.0, 51.0), (24.0, 58.0)]],
        '4' => {
            let mut counter = vec![(28.0, 26.0), (9.0, 26.0), (28.0, 55.0)];
            if even_odd {
                counter.reverse();
            }
            vec![
                vec![
                    (28.0, 0.0),
                    (37.0, 0.0),
                    (37.0, 17.0),
                    (46.0, 17.0),
                    (46.0, 26.0),
                    (37.0, 26.0),
                    (37.0, 70.0),
                    (28.0, 70.0),
                    (0.0, 26.0),
                    (0.0, 17.0),
                    (28.0, 17.0),
                ],
                counter,
            ]
        }
        '2' => {
            let mut line: Vec<(f64, f64)> = (0..=10)
                .map(|i| {
                    let a = (160.0 - 200.0 * i as f64 / 10.0_f64).to_radians();
                    (22.0 + 18.0 * a.cos(), 50.0 + 17.0 * a.sin())
                })
                .collect();
            line.extend([(4.0, 3.0), (44.0, 3.0)]);
            thick(&line, w)
        }
        _ => Vec::new(),
    }
}

/// Path operators for loops mapped by `m` (`[a b c d e f]`, as a PDF matrix).
fn path(loops: &[Vec<(f64, f64)>], m: [f64; 6]) -> String {
    let at = |(x, y): (f64, f64)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let mut s = String::new();
    for l in loops {
        for (i, &p) in l.iter().enumerate() {
            let (x, y) = at(p);
            s.push_str(&format!("{x:.3} {y:.3} {}\n", if i == 0 { "m" } else { "l" }));
        }
        s.push_str("h\n");
    }
    s
}

/// `text` as filled outlines, its cap `height` points tall at `at`, turned `deg` degrees,
/// painted with `fill` (`f` non-zero, `f*` even-odd): one fill per glyph.
fn outlined(text: &str, at: (f64, f64), height: f64, deg: f64, fill: &str) -> String {
    let k = height / 70.0;
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let mut out = String::from("0 0 0 rg\n");
    for (i, ch) in text.chars().enumerate() {
        let along = i as f64 * 56.0 * k;
        let m = [k * c, k * s, -k * s, k * c, at.0 + along * c, at.1 + along * s];
        out.push_str(&path(&glyph(ch, fill == "f*"), m));
        out.push_str(fill);
        out.push('\n');
    }
    out
}

/// A one-page 11 × 8.5 in PDF around `content`, with Helvetica as /F1 and two Type3 fonts
/// whose glyphs are the digits above: /N names its glyphs (`/two` …, codes 1–5, no ToUnicode),
/// /U doesn't (`/g1` …, codes 1–5). Both have FontMatrix 0.01 (glyph space in hundredths).
fn pdf(content: &str) -> Vec<u8> {
    let digits = ['0', '1', '2', '4'];
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] /Resources << /Font << /F1 5 0 R /N 6 0 R /U 7 0 R >> >> /Contents 4 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    // the glyph procedures: objects 8.. (one per digit)
    let first_proc = 8;
    let procs: Vec<String> = digits
        .iter()
        .map(|&ch| {
            let body = format!("60 0 0 0 44 70 d1\n{}f\n", path(&glyph(ch, false), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
        })
        .collect();
    let font = |names: [&str; 4]| {
        let char_procs: String = names.iter().enumerate().map(|(i, n)| format!("/{n} {} 0 R ", first_proc + i)).collect();
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 44 70] /FontMatrix [0.01 0 0 0.01 0 0] /CharProcs << {char_procs}>> \
             /Encoding << /Type /Encoding /Differences [1 /{} /{} /{} /{}] >> /FirstChar 1 /LastChar 4 /Widths [60 60 60 60] /Resources << >> >>",
            names[0], names[1], names[2], names[3]
        )
    };
    objs.push(font(["zero", "one", "two", "four"]));
    objs.push(font(["g1", "g2", "g3", "g4"]));
    objs.extend(procs);
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

/// The digits as codes of the Type3 fonts (0 → 1, 1 → 2, 2 → 3, 4 → 4), a hex string.
fn codes(text: &str) -> String {
    let hex: String = text
        .chars()
        .map(|c| match c {
            '0' => "01",
            '1' => "02",
            '2' => "03",
            _ => "04",
        })
        .collect();
    format!("<{hex}>")
}

struct Read {
    value: String,
    style: String,
    layer: String,
    /// insertion point and height, sheet points (the import is at sheet size, inches)
    at: (f64, f64),
    height: f64,
    rotation: f64,
}

fn texts(r: &pdf_read::PdfImport) -> Vec<Read> {
    r.drawing
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(Read {
                value: t.value.clone(),
                style: t.style.to_string(),
                layer: e.common.layer.to_string(),
                at: (t.insert.x * 72.0, t.insert.y * 72.0),
                height: t.height * 72.0,
                rotation: t.rotation.to_degrees(),
            }),
            _ => None,
        })
        .collect()
}

fn read(content: &str) -> pdf_read::PdfImport {
    let opts = PdfOptions { scale: Some(1.0), ..PdfOptions::default() };
    pdf_read::read(&pdf(content), &opts).expect("read")
}

fn find<'a>(all: &'a [Read], value: &str) -> &'a Read {
    all.iter()
        .find(|t| t.value == value)
        .unwrap_or_else(|| panic!("no {value:?} among {:?}", all.iter().map(|t| (&t.value, &t.style, t.at)).collect::<Vec<_>>()))
}

#[test]
fn type3_glyph_names_are_the_text_at_the_font_matrix_size() {
    // codes 4 1 2 1 are "four zero one zero" by the font's encoding; then a jump of 3000/1000 em
    let r = read(&format!("BT /N 12 Tf 100 300 Td {} Tj [{} -3000 {}] TJ ET", codes("40"), codes("10"), codes("2")));
    let all = texts(&r);
    let tag = find(&all, "4010");
    assert!((tag.at.0 - 100.0).abs() < 0.01 && (tag.at.1 - 300.0).abs() < 0.01, "at {:?}", tag.at);
    // the glyphs' d1 box: 70 of 100 units per em, at 12 pt
    assert!((tag.height - 8.4).abs() < 0.01, "height {}", tag.height);
    assert_ne!(tag.style, OCR_TEXT_STYLE, "read from the font, not guessed");
    // advances through FontMatrix: four glyphs of 60 units are 28.8 pt, then 36 pt of jump
    let two = find(&all, "2");
    assert!((two.at.0 - (100.0 + 28.8 + 36.0)).abs() < 0.01, "at {:?}", two.at);
    // named glyphs are text, not drawings as well
    assert!(!r.drawing.model.iter().any(|e| matches!(e.kind, EntityKind::Hatch(_))), "no glyph shapes drawn");
    assert!(all.iter().all(|t| !t.value.chars().any(char::is_control)), "no undecoded codes");
}

#[test]
fn type3_glyphs_nothing_names_are_drawn_and_read() {
    let content = format!(
        "BT /U 14 Tf 100 300 Td {} Tj ET\nBT /U 10 Tf 0 1 -1 0 400 200 Tm {} Tj ET\nBT /F1 8 Tf 100 100 Td (OFFICE) Tj ET",
        codes("4012"),
        codes("2140")
    );
    let r = read(&content);
    let all = texts(&r);
    // no junk from the codes themselves (they decode to nothing)
    assert!(all.iter().all(|t| !t.value.chars().any(char::is_control)), "{:?}", all.iter().map(|t| &t.value).collect::<Vec<_>>());
    // the glyphs are drawn where they print, and the lettering reader reads them
    assert!(r.drawing.model.iter().any(|e| matches!(e.kind, EntityKind::Hatch(_))), "glyph shapes drawn");
    let tag = find(&all, "4012");
    assert_eq!(tag.style, OCR_TEXT_STYLE, "a reading: unconfirmed");
    assert!((tag.at.0 - 100.0).abs() < 1.5 && (tag.at.1 - 300.0).abs() < 1.5, "at {:?}", tag.at);
    assert!((tag.height - 9.8).abs() < 1.0, "cap height {}", tag.height);
    let turned = find(&all, "2140");
    assert!((turned.rotation - 90.0).abs() < 1.0, "reads up the sheet: {}", turned.rotation);
    assert_ne!(find(&all, "OFFICE").style, OCR_TEXT_STYLE, "real text stays as it was");
    assert!(r.stats.get("outlined lettering texts recovered").is_some_and(|&n| n >= 2), "{:?}", r.stats);
}

#[test]
fn text_converted_to_outlines_reads_as_tags_in_either_fill_rule_and_turned() {
    let content = [
        outlined("1010", (100.0, 400.0), 9.0, 0.0, "f"),
        outlined("1004", (300.0, 400.0), 14.0, 0.0, "f*"),
        outlined("2041", (500.0, 300.0), 9.0, 90.0, "f"),
        outlined("2104", (100.0, 200.0), 6.0, 0.0, "f"),
        "BT /F1 8 Tf 100 100 Td (OFFICE) Tj ET".into(),
    ]
    .concat();
    let r = read(&content);
    let all = texts(&r);
    for (value, at, h) in [("1010", (100.0, 400.0), 9.0), ("1004", (300.0, 400.0), 14.0), ("2104", (100.0, 200.0), 6.0)] {
        let t = find(&all, value);
        // where the ink starts (a 1's flag reaches out a seventh of the height)
        assert!((t.at.0 - at.0).abs() < h * 0.2 && (t.at.1 - at.1).abs() < 1.0, "{value} at {:?}", t.at);
        assert!((t.height - h).abs() < h * 0.15, "{value} height {}", t.height);
        assert_eq!(t.style, OCR_TEXT_STYLE);
        assert!(t.rotation.abs() < 1.0);
    }
    assert!((find(&all, "2041").rotation - 90.0).abs() < 1.0);
    // without hidden text to confirm them, the outlines stay drawn where they are
    assert!(r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Hatch(_))).all(|e| e.common.layer != SHX_SOURCE_LAYER));
}

#[test]
fn hidden_text_confirms_outlined_lettering() {
    // the searchable copy (rendering mode 3) where the outlines are drawn
    let content = [
        outlined("1010", (100.0, 400.0), 9.0, 0.0, "f"),
        "BT 3 Tr /F1 12.6 Tf 100 400 Td (1010) Tj ET\n".into(),
        outlined("2104", (300.0, 400.0), 9.0, 0.0, "f"),
    ]
    .concat();
    let r = read(&content);
    let all = texts(&r);
    let shown: Vec<&Read> = all.iter().filter(|t| t.value == "1010" && t.layer != pdf_read::HIDDEN_TEXT_LAYER).collect();
    assert_eq!(shown.len(), 1, "{:?}", all.iter().map(|t| (&t.value, &t.layer)).collect::<Vec<_>>());
    assert_eq!(shown[0].style, "Standard", "confirmed by the PDF's own string");
    // the outlines it was read from are kept on the source layer (off), as stroked lettering's are
    let hatches: Vec<&std::sync::Arc<Entity>> = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Hatch(_))).collect();
    // a hatch's first vertex, sheet points
    let near = |e: &Entity, x: f64| match &e.kind {
        EntityKind::Hatch(h) => h.loops.first().and_then(|l| l.vertices.first()).is_some_and(|v| (v.p.x * 72.0 - x).abs() < 30.0),
        _ => false,
    };
    assert!(hatches.iter().filter(|e| near(e, 100.0)).all(|e| e.common.layer == SHX_SOURCE_LAYER), "the confirmed tag's outlines");
    // the page gives its lettering hidden copies: outlines with none are read for review only
    // (layer off), and stay drawn
    assert_eq!(find(&all, "2104").layer, pdf_read::OCR_REVIEW_LAYER);
    assert!(hatches.iter().filter(|e| near(e, 300.0)).all(|e| e.common.layer != SHX_SOURCE_LAYER));
}

#[test]
fn shapes_that_are_not_lettering_stay_shapes() {
    let mut content = String::from("0 0 0 rg\n");
    // column marks and a row of them, arrowheads, a dot pattern, a big filled region
    for i in 0..6 {
        let x = 100.0 + i as f64 * 14.0;
        content.push_str(&format!("{x} 400 8 8 re f\n"));
        content.push_str(&format!("{x} 300 m {} 304 l {x} 308 l h f\n", x + 9.0));
    }
    for i in 0..40 {
        content.push_str(&format!("{} 200 0.8 0.8 re f\n", 100.0 + i as f64 * 2.0));
    }
    content.push_str("300 100 200 120 re f\n");
    // wall poché in pieces: corners, bars and a hollow box side by side (they'd read L J I O)
    for i in 0..5 {
        let x = 100.0 + i as f64 * 12.0;
        content.push_str(&format!("{x} 500 m {} 500 l {} 502 l {} 502 l {} 512 l {x} 512 l h f\n", x + 8.0, x + 8.0, x + 2.0, x + 2.0));
        content.push_str(&format!("{} 520 2 12 re f\n", x + 3.0));
        content.push_str(&format!(
            "{x} 540 m {} 540 l {} 552 l {x} 552 l h {} 542 m {} 550 l {} 550 l {} 542 l h f\n",
            x + 8.0,
            x + 8.0,
            x + 2.0,
            x + 6.0,
            x + 6.0,
            x + 2.0
        ));
    }
    // a lone glyph no hidden string confirms is a mark as often as a letter, and so are glyphs
    // spaced like letters
    content.push_str(&outlined("2", (600.0, 300.0), 9.0, 0.0, "f"));
    for i in 0..4 {
        content.push_str(&outlined("0", (600.0 + i as f64 * 20.0, 200.0), 9.0, 0.0, "f"));
    }
    // outlines under real text are that text drawn twice
    content.push_str(&outlined("4012", (400.0, 400.0), 9.0, 0.0, "f"));
    content.push_str("BT /F1 12.6 Tf 400 400 Td (4012) Tj ET");
    let r = read(&content);
    let all = texts(&r);
    let values: Vec<&str> = all.iter().map(|t| t.value.as_str()).collect();
    assert_eq!(values, ["4012"], "only the real text: {values:?}");
}
