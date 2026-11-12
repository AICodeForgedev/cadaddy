//! A highlight painted over lettering (a fill with the Multiply blend mode, in the page or in a
//! markup's appearance) is drawn under the lettering, so the text it marks stays legible.

use cadaddy_doc::*;
use cadaddy_io::pdf_read::{self, PdfOptions};
use lopdf::{Dictionary, Document, Object, Stream};

fn name(n: &str) -> Object {
    Object::Name(n.as_bytes().to_vec())
}

fn nums(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x as f32)).collect())
}

/// An ExtGState with blend mode `bm`.
fn blend(bm: &str) -> Dictionary {
    let mut g = Dictionary::new();
    g.set("Type", name("ExtGState"));
    g.set("BM", name(bm));
    g
}

/// A one-page PDF (letter, landscape) of `content`, with font /F1 and ExtGState /GS1 of blend
/// mode `bm`, and optionally one Square markup whose appearance is `appearance` (over `rect`,
/// its ExtGState /GS0 of blend mode `bm`).
fn page(content: &str, bm: &str, markup: Option<(&str, [f64; 4])>) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut font = Dictionary::new();
    font.set("Type", name("Font"));
    font.set("Subtype", name("Type1"));
    font.set("BaseFont", name("Helvetica"));
    let font_id = doc.add_object(font);
    let mut fonts = Dictionary::new();
    fonts.set("F1", Object::Reference(font_id));
    let mut states = Dictionary::new();
    states.set("GS1", Object::Dictionary(blend(bm)));
    let mut res = Dictionary::new();
    res.set("Font", Object::Dictionary(fonts));
    res.set("ExtGState", Object::Dictionary(states));
    let contents = doc.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
    let mut pg = Dictionary::new();
    pg.set("Type", name("Page"));
    pg.set("Parent", Object::Reference(pages_id));
    pg.set("MediaBox", nums(&[0.0, 0.0, 792.0, 612.0]));
    pg.set("Resources", Object::Dictionary(res));
    pg.set("Contents", Object::Reference(contents));
    if let Some((ap, rect)) = markup {
        let mut sd = Dictionary::new();
        sd.set("Type", name("XObject"));
        sd.set("Subtype", name("Form"));
        sd.set("BBox", nums(&[0.0, 0.0, rect[2] - rect[0], rect[3] - rect[1]]));
        let mut gs = Dictionary::new();
        gs.set("GS0", Object::Dictionary(blend(bm)));
        let mut r = Dictionary::new();
        r.set("ExtGState", Object::Dictionary(gs));
        sd.set("Resources", Object::Dictionary(r));
        let ap_id = doc.add_object(Stream::new(sd, ap.as_bytes().to_vec()));
        let mut n = Dictionary::new();
        n.set("N", Object::Reference(ap_id));
        let mut a = Dictionary::new();
        a.set("Type", name("Annot"));
        a.set("Subtype", name("Square"));
        a.set("Rect", nums(&rect));
        a.set("F", Object::Integer(4));
        a.set("AP", Object::Dictionary(n));
        let a_id = doc.add_object(a);
        pg.set("Annots", Object::Array(vec![Object::Reference(a_id)]));
    }
    let page_id = doc.add_object(pg);
    let mut pages = Dictionary::new();
    pages.set("Type", name("Pages"));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut catalog = Dictionary::new();
    catalog.set("Type", name("Catalog"));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// Where in the drawing's painting order the yellow fill and the note's text are.
fn fill_and_text(bytes: &[u8]) -> (usize, usize) {
    let r = pdf_read::read(bytes, &PdfOptions::default()).unwrap();
    let ents: Vec<&Entity> = r.drawing.model.iter().map(|e| e.as_ref()).collect();
    let fill = ents.iter().position(|e| matches!(&e.kind, EntityKind::Hatch(h) if h.solid)).expect("the highlight is a solid fill");
    let text = ents.iter().position(|e| matches!(&e.kind, EntityKind::Text(t) if t.value.contains("PROVIDE DAMPER"))).expect("the note is text");
    (fill, text)
}

const NOTE: &str = "BT /F1 10 Tf 100 300 Td (2. PROVIDE DAMPER AT WALL) Tj ET";

#[test]
fn a_multiply_highlight_in_the_page_goes_under_the_lettering() {
    let content = format!("{NOTE} q /GS1 gs 1 0.933 0.345 rg 95 295 220 18 re f Q");
    let (fill, text) = fill_and_text(&page(&content, "Multiply", None));
    assert!(fill < text, "the highlight is painted after the note in the PDF, but drawn under it: fill {fill}, text {text}");
    // An opaque fill painted over the note (blend mode Normal) still covers it, as it prints.
    let (fill, text) = fill_and_text(&page(&content, "Normal", None));
    assert!(fill > text, "a Normal fill keeps its place: fill {fill}, text {text}");
}

#[test]
fn a_highlight_markup_goes_under_the_lettering() {
    // A reviewer's "Highlight Area" markup: a Square annotation whose appearance fills its box
    // with a Multiply blend (drawn after the page's content).
    let ap = "/GS0 gs 1 0.933 0.345 rg 0 0 220 18 re f";
    let (fill, text) = fill_and_text(&page(NOTE, "Multiply", Some((ap, [95.0, 295.0, 315.0, 313.0]))));
    assert!(fill < text, "the highlight markup is drawn under the note: fill {fill}, text {text}");
    let (fill, text) = fill_and_text(&page(NOTE, "Normal", Some((ap, [95.0, 295.0, 315.0, 313.0]))));
    assert!(fill > text, "an opaque markup keeps its place over the page: fill {fill}, text {text}");
}
