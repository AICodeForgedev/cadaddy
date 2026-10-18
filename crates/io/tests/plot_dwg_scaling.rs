//! Plotting and DWG saving of large drawings (perf1): a large page's content is written as
//! several streams compressed side by side, which together are exactly the page's content; the
//! DWG bridge hands the converter binary DXF and keeps linetype names as the drawing spells them.

use cadaddy_doc::*;
use cadaddy_geom::Vec3;
use cadaddy_io::{PdfOptions, pdf};

fn many_lines(n: usize) -> Drawing {
    let mut d = Drawing::new_metric();
    for i in 0..n {
        let (x, y) = ((i % 300) as f64 * 3.0, (i / 300) as f64 * 3.0);
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 2.0, y + 1.7, 0.0) })).unwrap();
    }
    d
}

/// The content streams of a page, in order, decompressed.
fn page_content(pdf: &[u8]) -> Vec<u8> {
    let doc = lopdf::Document::load_mem(pdf).unwrap();
    let page = *doc.get_pages().values().next().unwrap();
    doc.get_page_content(page)
}

#[test]
fn a_large_page_is_several_streams_with_the_same_content() {
    let d = many_lines(60_000);
    let t = std::time::Instant::now();
    let packed = pdf(&d, &Space::Model, &PdfOptions { compress: true, ..PdfOptions::default() }).unwrap();
    let took = t.elapsed();
    let plain = pdf(&d, &Space::Model, &PdfOptions { compress: false, ..PdfOptions::default() }).unwrap();
    let text = String::from_utf8_lossy(&packed);
    assert!(text.contains("/Contents ["), "split into several streams");
    // PDF joins a page's streams between tokens: compare the token sequences.
    let tokens = |b: Vec<u8>| String::from_utf8_lossy(&b).split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let (a, b) = (tokens(page_content(&packed)), tokens(page_content(&plain)));
    let first = a.iter().zip(&b).position(|(x, y)| x != y);
    assert!(a.len() == b.len() && first.is_none(), "{} vs {} tokens, first difference at {first:?}", a.len(), b.len());
    assert!(packed.len() < plain.len() / 2);
    // CADaddy's own PDF import reads the page the same either way.
    let import = |b: &[u8]| cadaddy_io::pdf_read::read(b, &cadaddy_io::pdf_read::PdfOptions::default()).unwrap().drawing.entity_count();
    assert_eq!(import(&packed), import(&plain));
    assert!(took.as_secs_f64() < 30.0, "plot took {took:?}");
    // A small page stays one stream.
    let small = pdf(&many_lines(10), &Space::Model, &PdfOptions { compress: true, ..PdfOptions::default() }).unwrap();
    assert!(!String::from_utf8_lossy(&small).contains("/Contents ["));
}

#[test]
fn dwg_saves_of_large_drawings_read_back_the_same() {
    let mut d = many_lines(30_000);
    // A linetype spelled as an older file spells it: the DWG keeps the spelling.
    let mut lt = d.linetypes.iter().find(|l| l.name.eq_ignore_ascii_case("continuous")).cloned().unwrap();
    lt.name = "CONTINUOUS".into();
    d.linetypes.retain(|l| !l.name.eq_ignore_ascii_case("continuous"));
    d.linetypes.push(lt);
    let t = std::time::Instant::now();
    let dwg = cadaddy_io::write(&d, "big.dwg").unwrap();
    let took = t.elapsed();
    let back = cadaddy_io::read(&dwg, "big.dwg").unwrap();
    assert_eq!(back.model.len(), 30_000);
    assert!(d.model.iter().zip(back.model.iter()).all(|(a, b)| a.kind == b.kind));
    assert!(back.linetypes.iter().any(|l| l.name == "CONTINUOUS"), "{:?}", back.linetypes.iter().map(|l| &l.name).collect::<Vec<_>>());
    assert!(took.as_secs_f64() < 30.0, "DWG save took {took:?}");
}
