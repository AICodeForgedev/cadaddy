//! Review markups in PDFs: the annotations of a page (ISO 32000-1 §12.5) read as
//! [`Markup`]s — Square and Circle (clouds when their border effect is cloudy), Polygon (a
//! cloud when its intent is PolygonCloud), PolyLine and Line (arrows from their line endings),
//! Ink, FreeText (a callout when it has a callout line), Text notes, Stamps, and the text
//! markups (Highlight, Underline, StrikeOut, Squiggly). Links, popups, widgets and the rest are
//! left out. And the other way: a page with markups written as annotations (a review copy, and
//! what the tests read).

use cadaddy_doc::markup::{Markup, MarkupKind, MarkupPage};
use lopdf::{Dictionary, Document, Object, ObjectId};

/// Most annotations read from one page.
const MAX_ANNOTS: usize = 10_000;
/// Most points in one path.
const MAX_POINTS: usize = 100_000;

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
    .filter(|v| v.is_finite())
}

fn nums(doc: &Document, d: &Dictionary, key: &[u8]) -> Vec<f64> {
    get(doc, d, key)
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().filter_map(|o| num(deref(doc, o))).take(2 * MAX_POINTS).collect())
        .unwrap_or_default()
}

fn pairs(v: &[f64]) -> Vec<[f64; 2]> {
    v.as_chunks::<2>().0.to_vec()
}

fn name(doc: &Document, d: &Dictionary, key: &[u8]) -> String {
    get(doc, d, key).and_then(|o| o.as_name().ok()).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()
}

/// A PDF text string: UTF-16BE with its byte order mark, else PDFDocEncoding (Latin-1 here).
fn text(doc: &Document, d: &Dictionary, key: &[u8]) -> String {
    let Some(Object::String(b, _)) = get(doc, d, key) else { return String::new() };
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        b.iter().map(|c| char::from(*c)).collect()
    }
}

fn color(doc: &Document, d: &Dictionary) -> Option<[u8; 3]> {
    let c = nums(doc, d, b"C");
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c.as_slice() {
        [g] => Some([b(*g); 3]),
        [r, g, bl] => Some([b(*r), b(*g), b(*bl)]),
        [c, m, y, k] => Some([b((1.0 - c) * (1.0 - k)), b((1.0 - m) * (1.0 - k)), b((1.0 - y) * (1.0 - k))]),
        _ => None,
    }
}

/// Whether the annotation's border effect is cloudy (BE /S /C).
fn cloudy(doc: &Document, d: &Dictionary) -> bool {
    get(doc, d, b"BE").and_then(|o| o.as_dict().ok()).is_some_and(|be| name(doc, be, b"S") == "C")
}

fn rect_points(r: [f64; 4]) -> Vec<[f64; 2]> {
    let [x0, y0, x1, y1] = r;
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// An ellipse inscribed in `r`, as a polygon.
fn ellipse_points(r: [f64; 4]) -> Vec<[f64; 2]> {
    let [x0, y0, x1, y1] = r;
    let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0).abs() / 2.0, (y1 - y0).abs() / 2.0);
    (0..32)
        .map(|i| {
            let a = std::f64::consts::TAU * f64::from(i) / 32.0;
            [cx + rx * a.cos(), cy + ry * a.sin()]
        })
        .collect()
}

fn markup(doc: &Document, d: &Dictionary) -> Option<Markup> {
    let sub = name(doc, d, b"Subtype");
    let r = nums(doc, d, b"Rect");
    let rect = match r.as_slice() {
        [a, b, c, e] => [a.min(*c), b.min(*e), a.max(*c), b.max(*e)],
        _ => return None,
    };
    let flags = get(doc, d, b"F").and_then(num).unwrap_or(0.0) as i64;
    if flags & 2 != 0 {
        return None;
    }
    let mut m = Markup {
        kind: MarkupKind::Note,
        points: Vec::new(),
        strokes: Vec::new(),
        rect,
        text: text(doc, d, b"Contents"),
        author: text(doc, d, b"T"),
        subject: text(doc, d, b"Subj"),
        color: color(doc, d),
        date: {
            let m = text(doc, d, b"M");
            if m.is_empty() { text(doc, d, b"CreationDate") } else { m }
        },
    };
    let arrow_ends = |doc: &Document| {
        get(doc, d, b"LE")
            .and_then(|o| o.as_array().ok())
            .map(|a| {
                a.iter().map(|o| deref(doc, o).as_name().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()).collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let is_arrow = |s: &str| s.contains("Arrow");
    match sub.as_str() {
        "Square" => {
            m.kind = if cloudy(doc, d) { MarkupKind::Cloud } else { MarkupKind::Rectangle };
            m.points = rect_points(rect);
        }
        "Circle" => {
            m.kind = if cloudy(doc, d) { MarkupKind::Cloud } else { MarkupKind::Ellipse };
            m.points = ellipse_points(rect);
        }
        "Polygon" => {
            m.kind = if name(doc, d, b"IT") == "PolygonCloud" || cloudy(doc, d) { MarkupKind::Cloud } else { MarkupKind::Polygon };
            m.points = pairs(&nums(doc, d, b"Vertices"));
        }
        "PolyLine" | "Line" => {
            let pts = if sub == "Line" { pairs(&nums(doc, d, b"L")) } else { pairs(&nums(doc, d, b"Vertices")) };
            let ends = arrow_ends(doc);
            let (start, end) = (ends.first().is_some_and(|e| is_arrow(e)), ends.get(1).is_some_and(|e| is_arrow(e)));
            m.kind = if start || end {
                MarkupKind::Arrow
            } else if sub == "Line" {
                MarkupKind::Line
            } else {
                MarkupKind::Polyline
            };
            // An arrow's head first.
            m.points = if end && !start { pts.into_iter().rev().collect() } else { pts };
        }
        "Ink" => {
            m.kind = MarkupKind::Ink;
            m.strokes = get(doc, d, b"InkList")
                .and_then(|o| o.as_array().ok())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| deref(doc, s).as_array().ok())
                        .map(|s| pairs(&s.iter().filter_map(|o| num(deref(doc, o))).collect::<Vec<_>>()))
                        .collect()
                })
                .unwrap_or_default();
        }
        "FreeText" => {
            let cl = pairs(&nums(doc, d, b"CL"));
            if cl.len() >= 2 || name(doc, d, b"IT") == "FreeTextCallout" {
                m.kind = MarkupKind::Callout;
                m.points = cl;
            } else {
                m.kind = MarkupKind::Note;
                m.points = vec![[rect[0], rect[3]]];
            }
        }
        "Text" => {
            m.kind = MarkupKind::Note;
            m.points = vec![[rect[0], rect[3]]];
        }
        "Stamp" => {
            m.kind = MarkupKind::Stamp;
            m.points = rect_points(rect);
            if m.text.is_empty() {
                m.text = name(doc, d, b"Name");
            }
        }
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            m.kind = MarkupKind::Highlight;
            let q = nums(doc, d, b"QuadPoints");
            m.strokes = q
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| {
                    let xs = [c[0], c[2], c[4], c[6]];
                    let ys = [c[1], c[3], c[5], c[7]];
                    let (x0, x1) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
                    let (y0, y1) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
                    rect_points([x0, y0, x1, y1])
                })
                .collect();
            if m.strokes.is_empty() {
                m.strokes.push(rect_points(rect));
            }
        }
        _ => return None,
    }
    m.points.truncate(MAX_POINTS);
    Some(m)
}

/// The markups of page `page` (1-based) of a PDF.
pub fn read(bytes: &[u8], page: u32) -> Result<MarkupPage, String> {
    let doc = Document::load_mem(bytes).map_err(|e| format!("PDF: {e}"))?;
    let pages = doc.get_pages();
    let id = *pages.get(&page.max(1)).ok_or_else(|| format!("the PDF has {} page(s)", pages.len()))?;
    let pd = doc.get_dictionary(id).map_err(|e| format!("PDF page: {e}"))?;
    // MediaBox may be inherited from the page tree.
    let mut mb = nums(&doc, pd, b"MediaBox");
    let mut parent = pd.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    let mut guard = 0;
    while mb.len() != 4 && guard < 32 {
        guard += 1;
        let Some(p) = parent.and_then(|p| doc.get_dictionary(p).ok()) else { break };
        mb = nums(&doc, p, b"MediaBox");
        parent = p.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    }
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(mb.as_slice()).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let annots: Vec<&Dictionary> = get(&doc, pd, b"Annots")
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().take(MAX_ANNOTS).filter_map(|o| deref(&doc, o).as_dict().ok()).collect())
        .unwrap_or_default();
    let mut markups: Vec<Markup> = annots.iter().filter_map(|d| markup(&doc, d)).collect();
    // Page coordinates from the media box's corner.
    for m in &mut markups {
        let shift = |p: &mut [f64; 2]| {
            p[0] -= x0.min(x1);
            p[1] -= y0.min(y1);
        };
        m.points.iter_mut().for_each(shift);
        m.strokes.iter_mut().flatten().for_each(shift);
        m.rect = [m.rect[0] - x0.min(x1), m.rect[1] - y0.min(y1), m.rect[2] - x0.min(x1), m.rect[3] - y0.min(y1)];
    }
    Ok(MarkupPage { width: (x1 - x0).abs(), height: (y1 - y0).abs(), markups })
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

fn arr(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| real(*x)).collect())
}

fn pdf_text(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend(u.to_be_bytes());
        }
        Object::String(b, lopdf::StringFormat::Hexadecimal)
    }
}

/// A one-page PDF of `width` × `height` points carrying `markups` as annotations (no
/// appearance streams: viewers draw them from their dictionaries).
pub fn annotated_pdf(width: f64, height: f64, markups: &[Markup]) -> Result<Vec<u8>, String> {
    let mut doc = Document::with_version("1.7");
    let pages_id: ObjectId = doc.new_object_id();
    let mut annots = Vec::new();
    for m in markups {
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"Annot".to_vec()));
        let flat = |pts: &[[f64; 2]]| pts.iter().flat_map(|p| [p[0], p[1]]).collect::<Vec<f64>>();
        let sub: &[u8] = match m.kind {
            MarkupKind::Cloud => {
                d.set("IT", Object::Name(b"PolygonCloud".to_vec()));
                d.set("Vertices", arr(&flat(&m.points)));
                b"Polygon"
            }
            MarkupKind::Rectangle => b"Square",
            MarkupKind::Ellipse => b"Circle",
            MarkupKind::Polygon => {
                d.set("Vertices", arr(&flat(&m.points)));
                b"Polygon"
            }
            MarkupKind::Polyline => {
                d.set("Vertices", arr(&flat(&m.points)));
                b"PolyLine"
            }
            MarkupKind::Line | MarkupKind::Arrow => {
                d.set("L", arr(&flat(&m.points.iter().take(2).copied().collect::<Vec<_>>())));
                if m.kind == MarkupKind::Arrow {
                    d.set("LE", Object::Array(vec![Object::Name(b"OpenArrow".to_vec()), Object::Name(b"None".to_vec())]));
                }
                b"Line"
            }
            MarkupKind::Ink => {
                d.set("InkList", Object::Array(m.strokes.iter().map(|s| arr(&flat(s))).collect()));
                b"Ink"
            }
            // A note no bigger than an icon is a sticky note; others are text boxes.
            MarkupKind::Note if (m.rect[2] - m.rect[0]).abs() <= 24.0 && (m.rect[3] - m.rect[1]).abs() <= 24.0 => b"Text",
            MarkupKind::Note => b"FreeText",
            MarkupKind::Callout => {
                d.set("CL", arr(&flat(&m.points)));
                d.set("IT", Object::Name(b"FreeTextCallout".to_vec()));
                b"FreeText"
            }
            MarkupKind::Stamp => b"Stamp",
            MarkupKind::Highlight => {
                d.set(
                    "QuadPoints",
                    arr(&m
                        .strokes
                        .iter()
                        .flat_map(|q| {
                            let xs: Vec<f64> = q.iter().map(|p| p[0]).collect();
                            let ys: Vec<f64> = q.iter().map(|p| p[1]).collect();
                            let (x0, x1) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
                            let (y0, y1) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
                            [x0, y1, x1, y1, x0, y0, x1, y0]
                        })
                        .collect::<Vec<_>>()),
                );
                b"Highlight"
            }
        };
        d.set("Subtype", Object::Name(sub.to_vec()));
        d.set("Rect", arr(&m.rect));
        d.set("Contents", pdf_text(&m.text));
        if !m.author.is_empty() {
            d.set("T", pdf_text(&m.author));
        }
        if let Some(c) = m.color {
            d.set("C", arr(&c.map(|v| f64::from(v) / 255.0)));
        }
        annots.push(Object::Reference(doc.add_object(d)));
    }
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set("MediaBox", arr(&[0.0, 0.0, width, height]));
    page.set("Annots", Object::Array(annots));
    let page_id = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(|e| format!("writing the PDF: {e}"))?;
    Ok(bytes)
}
