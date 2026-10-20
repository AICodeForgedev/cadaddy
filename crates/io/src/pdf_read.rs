//! PDF → Drawing: PDFIMPORT and "Convert PDF to DWG".
//!
//! Reads one page's vector content — paths (lines, Bézier curves, rectangles) from the page and
//! its form XObjects, with the current transformation matrix, colours and line widths — and its
//! text (positioned, sized and rotated, decoded through the fonts' ToUnicode maps or their simple
//! encodings). Images, shadings and clipping are not read (clipped-away content shows).
//!
//! What it does with them comes from converting real plotted sheets:
//! - Plotters draw curves as hundreds of separate short segments: touching pieces of one style
//!   are joined into polylines, and closed polylines that are round become circles.
//! - Some plotters draw a few objects twice, exactly on top: exact duplicates are dropped.
//! - White fills are masks behind tags and text; they hide what they mask on a dark drawing
//!   area, so they are left out unless asked for.
//! - Without PDF layers (optional content groups), the useful split is by pen: one layer per
//!   colour and line weight (the grey architectural background, the black electrical work), so
//!   a background can be frozen in one go. With PDF layers, those are the layers.
//! - Pure black becomes colour 7 (white on a dark drawing area, black on paper), as in CAD.
//! - The sheet's own scale note ("SCALE: 1/8" = 1'-0"") is found and offered as the scale.
//! - With a scale, the sheet is drawn full size in model space and a layout the size of the
//!   page shows it through one viewport at that scale, so it plots like the original.
//!
//! Lettering a plotter drew as strokes (SHX fonts) stays linework; real text stays text.

use std::collections::{BTreeMap, HashMap, HashSet};

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::{IoError, Result};

/// Content-stream operations read per page (all forms included): a guard against hostile files.
const MAX_OPS: usize = 20_000_000;
/// Most bytes one content stream or form inflates to (a small "flate bomb" can't take all memory).
const MAX_STREAM_BYTES: usize = 256 << 20;
/// Nesting of form XObjects.
const MAX_DEPTH: usize = 12;
/// Objects created.
const MAX_OBJECTS: usize = 4_000_000;
/// Points in one path.
const MAX_PATH_POINTS: usize = 2_000_000;
/// Segments a Bézier curve is flattened into.
const CURVE_STEPS: usize = 8;

/// How objects are put on layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayerMode {
    /// The PDF's layers (optional content groups); by pen when it has none.
    #[default]
    Pdf,
    /// One layer per colour and line weight.
    Pen,
    /// AutoCAD's PDFIMPORT object layers: PDF_Geometry, PDF_Solid Fills, PDF_Text.
    Object,
}

#[derive(Clone, Debug)]
pub struct PdfOptions {
    /// 1-based page number.
    pub page: u32,
    /// Drawing units per inch of the sheet; `None` = the sheet's own scale note, else 1 (the
    /// sheet in inches).
    pub scale: Option<f64>,
    pub layers: LayerMode,
    /// Keep white fills as solid white fills (off by default).
    pub white_fills: bool,
    /// White fills that aren't kept become wipeouts (masks in the drawing's background colour,
    /// as AutoCAD masks), in painting order: masks behind tags, cut-outs in logos (on by default).
    pub masks: bool,
    pub text: bool,
    /// Join touching pieces into polylines and turn round closed ones into circles.
    pub join: bool,
    /// When the scale isn't 1: a layout the size of the page, showing model space at the scale.
    pub layout: bool,
    /// Draw the page's annotations (markups: highlights, clouds, stamps, text boxes) as they
    /// appear, on their own layer PDF-MARKUP (on by default).
    pub markups: bool,
    /// Bring raster images (logos, photos, scans) in as IMAGE objects (on by default). Their
    /// pixels are returned in [`PdfImport::images`] for the caller to save as PNG files.
    pub images: bool,
    /// The folder the image files go in: IMAGE objects name `<folder>/<image_prefix>-<n>.png`.
    /// `None`: they name `pdf-image:<image_prefix>-<n>.png`, which only this session can show
    /// until the images are saved somewhere.
    pub image_folder: Option<String>,
    /// File name start of the images (the PDF's name and page).
    pub image_prefix: String,
}

/// The layer annotations (markups) are drawn on.
pub const MARKUP_LAYER: &str = "PDF-MARKUP";

/// The layer invisible text (text rendering mode 3: the searchable copy of lettering drawn as
/// linework, as AutoCAD writes SHX text) goes on, turned off: the sheet shows the lettering
/// once, and its words can still be searched and listed.
pub const HIDDEN_TEXT_LAYER: &str = "PDF-HIDDEN-TEXT";

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions {
            page: 1,
            scale: None,
            layers: LayerMode::Pdf,
            white_fills: false,
            masks: true,
            text: true,
            join: true,
            layout: true,
            markups: true,
            images: true,
            image_folder: None,
            image_prefix: "pdf-image".into(),
        }
    }
}

/// A converted page and what happened on the way.
pub struct PdfImport {
    pub drawing: Drawing,
    /// The sheet in inches (as displayed, rotation applied).
    pub sheet: (f64, f64),
    /// The scale used, and where it came from ("scale note 1/8" = 1'-0"", "option", "none").
    pub scale: f64,
    pub scale_source: String,
    pub pages: u32,
    pub stats: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    /// The image files the IMAGE objects name (path as they name it) and their pixels.
    pub images: Vec<(String, std::sync::Arc<cadaddy_render::images::Raster>)>,
}

/// The number of pages.
pub fn page_count(bytes: &[u8]) -> Result<u32> {
    let doc = load(bytes)?;
    Ok(u32::try_from(doc.get_pages().len()).unwrap_or(u32::MAX))
}

fn load(bytes: &[u8]) -> Result<Document> {
    Document::load_mem(bytes).map_err(|e| IoError::Format(format!("PDF: {e}")))
}

fn err(m: impl Into<String>) -> IoError {
    IoError::Format(format!("PDF: {}", m.into()))
}

// ---------------------------------------------------------------------------------------------
// Graphics state and painted output
// ---------------------------------------------------------------------------------------------

/// PDF matrix [a b c d e f]: x' = a x + c y + e, y' = b x + d y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
struct M([f64; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    /// `self` then `o` (PDF's `self × o`).
    fn then(self, o: M) -> M {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        M([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }
    /// The mean scale (line widths, text heights).
    fn scale(&self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }
    fn finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
}

#[derive(Clone)]
struct Gs {
    ctm: M,
    stroke: [f64; 3],
    fill: [f64; 3],
    width: f64,
    /// Components the current non-stroking / stroking colour space takes (1 gray, 3 RGB, 4 CMYK).
    fill_n: usize,
    stroke_n: usize,
    /// The clipping outline set with W/W* (sheet points); fills are clipped to its box, and a
    /// fill covering it draws the outline itself (logos and lettering exported as clip + fill).
    clip: Option<std::rc::Rc<Vec<Vec<(f64, f64)>>>>,
    /// Constant alpha (ExtGState ca / CA).
    fill_alpha: f64,
    stroke_alpha: f64,
}

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: M::ID, stroke: [0.0; 3], fill: [0.0; 3], width: 1.0, fill_n: 1, stroke_n: 1, clip: None, fill_alpha: 1.0, stroke_alpha: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Pen {
    rgb: [u8; 3],
    /// Line weight in hundredths of a millimetre on the sheet (0 for fills and text).
    lw: u16,
    /// Transparency percent (0 = opaque), from the PDF's constant alpha.
    transp: u8,
}

struct Stroke {
    pts: Vec<(f64, f64)>,
    closed: bool,
    pen: Pen,
    layer: Option<String>,
    /// From a curve (Bézier) of 4 segments forming a closed loop: a circle candidate.
    curve_loop: bool,
    /// Painting order on the page.
    order: usize,
}

struct Fill {
    loops: Vec<Vec<(f64, f64)>>,
    pen: Pen,
    layer: Option<String>,
    order: usize,
}

/// A raster image painted on the page: its pixels and where the unit square of image space
/// lands (`ctm`, sheet points; row 0 of the pixels at the top, y = 1).
struct ImageOut {
    ctm: M,
    raster: std::sync::Arc<cadaddy_render::images::Raster>,
    layer: Option<String>,
    clip: Option<std::rc::Rc<Vec<Vec<(f64, f64)>>>>,
    /// Transparency percent from the constant fill alpha.
    transp: u8,
    order: usize,
}

/// Pixel memory images of one page may take.
const MAX_IMAGE_BYTES: usize = 1 << 30;

struct TextOut {
    at: (f64, f64),
    /// Where the text's advance ends (the next character would start), sheet points.
    end: (f64, f64),
    height: f64,
    rotation: f64,
    width_factor: f64,
    value: String,
    pen: Pen,
    layer: Option<String>,
    /// (text style, TrueType file) when the PDF font has one.
    style: Option<(String, String)>,
    order: usize,
}

#[derive(Default)]
struct Out {
    strokes: Vec<Stroke>,
    fills: Vec<Fill>,
    texts: Vec<TextOut>,
    images: Vec<ImageOut>,
    /// Images left out, by why.
    image_failures: BTreeMap<&'static str, usize>,
    /// Pixel bytes held by `images` (bounded: [`MAX_IMAGE_BYTES`]).
    image_bytes: usize,
    /// The next painting-order number.
    seq: usize,
    ops: usize,
    clipped_ops: usize,
}

fn to_rgb(c: &[f64]) -> [f64; 3] {
    let k = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    match c {
        [g] => [k(*g); 3],
        [r, g, b] => [k(*r), k(*g), k(*b)],
        [cy, m, y, bk] => [(1.0 - k(*cy)) * (1.0 - k(*bk)), (1.0 - k(*m)) * (1.0 - k(*bk)), (1.0 - k(*y)) * (1.0 - k(*bk))],
        _ => [0.0; 3],
    }
}

/// Transparency percent from an alpha (1 = opaque); CAD transparency stops at 90 %.
fn transp(alpha: f64) -> u8 {
    if !alpha.is_finite() {
        return 0;
    }
    ((1.0 - alpha.clamp(0.0, 1.0)) * 100.0).round().clamp(0.0, 90.0) as u8
}

fn rgb8(c: [f64; 3]) -> [u8; 3] {
    c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Standard lineweights, hundredths of a millimetre.
const LINEWEIGHTS: [u16; 24] = [0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158, 200, 211];

fn lineweight(width_pt: f64) -> u16 {
    let mm100 = width_pt.max(0.0) * 25.4 / 72.0 * 100.0;
    LINEWEIGHTS.iter().copied().min_by(|a, b| (f64::from(*a) - mm100).abs().total_cmp(&(f64::from(*b) - mm100).abs())).unwrap_or(0)
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
    .filter(|v| v.is_finite())
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().filter_map(num).collect()
}

// ---------------------------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Font {
    /// Character code → text, from the ToUnicode CMap.
    map: HashMap<u32, String>,
    /// Codes are two bytes (composite fonts, or a 2-byte codespace).
    two_byte: bool,
    /// Glyph widths in text space × 1000.
    widths: HashMap<u32, f64>,
    default_width: f64,
    /// The CAD text style for the font: (style name, TrueType font file); `None` keeps Standard.
    style: Option<(String, String)>,
}

/// The CAD text style for a PDF font (its BaseFont, subset prefix and all): a style named after
/// the font, using the TrueType file AutoCAD and CADaddy find it by (arial.ttf, arialbd.ttf, …).
fn ttf_style(base: &str) -> Option<(String, String)> {
    let name = base.split_once('+').map_or(base, |(_, n)| n);
    let lower = name.to_ascii_lowercase();
    let bold = lower.contains("bold") || lower.contains("black") || lower.ends_with(",b");
    let italic = lower.contains("italic") || lower.contains("oblique");
    let fam: String = lower.split([',', '-']).next().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let fam = fam.trim_end_matches("mt").trim_end_matches("psmt").to_string();
    let (label, file) = match fam.as_str() {
        "arialblack" => ("Arial Black", "ariblk.ttf".to_string()),
        "arialnarrow" => (
            "Arial Narrow",
            match (bold, italic) {
                (true, true) => "arialnbi.ttf",
                (true, false) => "arialnb.ttf",
                (false, true) => "arialni.ttf",
                _ => "arialn.ttf",
            }
            .to_string(),
        ),
        // Swiss 721 is Bitstream's Helvetica, with Arial's widths; Arial is on every system
        "arial" | "helvetica" | "arialmt" | "swiss721bt" | "swiss721" | "swis721bt" => (
            "Arial",
            match (bold, italic) {
                (true, true) => "arialbi.ttf",
                (true, false) => "arialbd.ttf",
                (false, true) => "ariali.ttf",
                _ => "arial.ttf",
            }
            .to_string(),
        ),
        "twcen" => (
            "Tw Cen MT",
            match (bold, italic) {
                (true, true) => "tcbi____.ttf",
                (true, false) => "tcb_____.ttf",
                (false, true) => "tcmi____.ttf",
                _ => "tcm_____.ttf",
            }
            .to_string(),
        ),
        "bradleyhanditc" => ("Bradley Hand ITC", "bradhitc.ttf".to_string()),
        "centurygothic" => ("Century Gothic", if bold { "GOTHICB.TTF" } else { "GOTHIC.TTF" }.to_string()),
        "timesnewroman" | "times" | "timesroman" => ("Times New Roman", if bold { "timesbd.ttf" } else { "times.ttf" }.to_string()),
        "calibri" => ("Calibri", if bold { "calibrib.ttf" } else { "calibri.ttf" }.to_string()),
        "couriernew" | "courier" => ("Courier New", if bold { "courbd.ttf" } else { "cour.ttf" }.to_string()),
        "verdana" => ("Verdana", if bold { "verdanab.ttf" } else { "verdana.ttf" }.to_string()),
        "tahoma" => ("Tahoma", if bold { "tahomabd.ttf" } else { "tahoma.ttf" }.to_string()),
        "" => return None,
        // AutoCAD's own SHX-as-TrueType stand-ins and symbol fonts: keep the default style
        f if f.starts_with("symbol") || f.starts_with("zapf") || f.starts_with("wingding") || f.contains("shx") => return None,
        _ => {
            let fam_name = name.split([',', '-']).next().unwrap_or(name).to_string();
            let styled = if bold { format!("{fam_name} Bold") } else { fam_name.clone() };
            return Some((format!("PDF-{}", styled.replace(' ', "")), styled));
        }
    };
    let style =
        format!("PDF-{}{}{}", label.replace(' ', ""), if bold && label != "Arial Black" { "-Bold" } else { "" }, if italic { "-Italic" } else { "" });
    Some((style, file))
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn dict_get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

fn stream_bytes(doc: &Document, o: &Object) -> Option<Vec<u8>> {
    match deref(doc, o) {
        Object::Stream(s) => s.decompressed_content_with_limit(MAX_STREAM_BYTES).ok().or_else(|| Some(s.content.clone())),
        _ => None,
    }
}

fn hex_code(s: &str) -> Option<(u32, usize)> {
    let s = s.trim_start_matches('<').trim_end_matches('>');
    if s.is_empty() || s.len() > 8 {
        return None;
    }
    u32::from_str_radix(s, 16).ok().map(|v| (v, s.len().div_ceil(2)))
}

fn utf16_hex(s: &str) -> String {
    let s = s.trim_start_matches('<').trim_end_matches('>');
    let units: Vec<u16> = s.as_bytes().chunks(4).filter_map(|c| std::str::from_utf8(c).ok().and_then(|h| u16::from_str_radix(h, 16).ok())).collect();
    String::from_utf16_lossy(&units)
}

/// Parse a ToUnicode CMap (bfchar, bfrange, codespacerange).
fn parse_cmap(data: &[u8], font: &mut Font) {
    let text = String::from_utf8_lossy(data);
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace())
        .flat_map(|t| {
            // "<0041><0041>" written together
            let mut parts = Vec::new();
            let mut rest = t;
            while let Some(i) = rest[1.min(rest.len())..].find('<').map(|i| i + 1) {
                if rest.starts_with('<') && i > 0 {
                    parts.push(&rest[..i]);
                    rest = &rest[i..];
                } else {
                    break;
                }
            }
            parts.push(rest);
            parts
        })
        .filter(|t| !t.is_empty())
        .collect();
    let mut i = 0;
    let mut mode = "";
    let mut guard = 0usize;
    while i < tokens.len() && guard < 2_000_000 {
        guard += 1;
        let t = tokens.get(i).copied().unwrap_or("");
        match t {
            "begincodespacerange" | "beginbfchar" | "beginbfrange" => {
                mode = t;
                i += 1;
                continue;
            }
            "endcodespacerange" | "endbfchar" | "endbfrange" => {
                mode = "";
                i += 1;
                continue;
            }
            _ => {}
        }
        match mode {
            "begincodespacerange" => {
                if let Some((_, n)) = hex_code(t) {
                    font.two_byte |= n >= 2;
                }
                i += 1;
            }
            "beginbfchar" => {
                if let (Some((code, n)), Some(dst)) = (hex_code(t), tokens.get(i + 1)) {
                    font.two_byte |= n >= 2;
                    font.map.insert(code, utf16_hex(dst));
                }
                i += 2;
            }
            "beginbfrange" => {
                let (Some((lo, n)), Some((hi, _))) = (hex_code(t), tokens.get(i + 1).and_then(|t| hex_code(t))) else {
                    i += 1;
                    continue;
                };
                font.two_byte |= n >= 2;
                let dst = tokens.get(i + 2).copied().unwrap_or("");
                if dst.starts_with('[') {
                    // [<dst1> <dst2> ...]
                    let mut k = i + 2;
                    let mut code = lo;
                    let mut first = true;
                    while let Some(tok) = tokens.get(k) {
                        let tk = if first { tok.trim_start_matches('[') } else { tok };
                        first = false;
                        let end = tk.ends_with(']');
                        let tk = tk.trim_end_matches(']');
                        if !tk.is_empty() && code <= hi {
                            font.map.insert(code, utf16_hex(tk));
                            code += 1;
                        }
                        k += 1;
                        if end {
                            break;
                        }
                    }
                    i = k;
                } else {
                    let base: Vec<u16> = {
                        let s = utf16_hex(dst);
                        s.encode_utf16().collect()
                    };
                    if hi >= lo && hi - lo < 65_536 {
                        for (n, code) in (lo..=hi).enumerate() {
                            let mut u = base.clone();
                            if let Some(last) = u.last_mut() {
                                *last = last.wrapping_add(u16::try_from(n).unwrap_or(0));
                            }
                            font.map.insert(code, String::from_utf16_lossy(&u));
                        }
                    }
                    i += 3;
                }
            }
            _ => i += 1,
        }
    }
}

fn load_font(doc: &Document, fd: &Dictionary) -> Font {
    let mut font = Font { default_width: 500.0, ..Font::default() };
    if let Some(Object::Name(base)) = dict_get(doc, fd, b"BaseFont") {
        font.style = ttf_style(&String::from_utf8_lossy(base));
    }
    let subtype = dict_get(doc, fd, b"Subtype").and_then(|o| o.as_name().ok()).unwrap_or(b"");
    if subtype == b"Type0" {
        font.two_byte = true;
        font.default_width = 1000.0;
        if let Some(Object::Array(desc)) = dict_get(doc, fd, b"DescendantFonts")
            && let Some(Object::Dictionary(cid)) = desc.first().map(|o| deref(doc, o))
        {
            if let Some(dw) = dict_get(doc, cid, b"DW").and_then(num) {
                font.default_width = dw;
            }
            if let Some(Object::Array(w)) = dict_get(doc, cid, b"W") {
                // c [w1 w2 ...] | c_first c_last w
                let mut k = 0;
                while k < w.len() && k < 200_000 {
                    let Some(c) = w.get(k).and_then(num) else { break };
                    match w.get(k + 1).map(|o| deref(doc, o)) {
                        Some(Object::Array(ws)) => {
                            for (n, x) in ws.iter().filter_map(num).enumerate() {
                                font.widths.insert(c as u32 + n as u32, x);
                            }
                            k += 2;
                        }
                        Some(o) => {
                            let (Some(last), Some(x)) = (num(o), w.get(k + 2).and_then(num)) else { break };
                            let (c0, c1) = (c as u32, (last as u32).min(c as u32 + 65_535));
                            for code in c0..=c1 {
                                font.widths.insert(code, x);
                            }
                            k += 3;
                        }
                        None => break,
                    }
                }
            }
        }
    } else {
        let first = dict_get(doc, fd, b"FirstChar").and_then(num).unwrap_or(0.0) as u32;
        if let Some(Object::Array(ws)) = dict_get(doc, fd, b"Widths") {
            for (n, x) in ws.iter().map(|o| deref(doc, o)).filter_map(num).enumerate() {
                font.widths.insert(first + n as u32, x);
            }
        }
        if let Some(Object::Dictionary(desc)) = dict_get(doc, fd, b"FontDescriptor")
            && let Some(mw) = dict_get(doc, desc, b"MissingWidth").and_then(num)
            && mw > 0.0
        {
            font.default_width = mw;
        }
    }
    if let Some(o) = fd.get(b"ToUnicode").ok()
        && let Some(data) = stream_bytes(doc, o)
    {
        parse_cmap(&data, &mut font);
    }
    font
}

/// Character codes of a shown string.
fn codes(font: &Font, bytes: &[u8]) -> Vec<u32> {
    if font.two_byte {
        bytes.chunks(2).map(|c| c.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b))).collect()
    } else {
        bytes.iter().map(|b| u32::from(*b)).collect()
    }
}

/// WinAnsi codes 0x80–0x9F that differ from Latin-1.
fn win_ansi(c: u32) -> char {
    const HI: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C,
        0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&c) {
        let u = HI.get((c - 0x80) as usize).copied().unwrap_or(0);
        return char::from_u32(u32::from(u)).filter(|_| u != 0).unwrap_or('?');
    }
    char::from_u32(c).unwrap_or('?')
}

fn decode(font: &Font, code: u32) -> String {
    match font.map.get(&code) {
        Some(s) => s.clone(),
        None if font.two_byte => String::new(),
        None => win_ansi(code).to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// The content-stream interpreter
// ---------------------------------------------------------------------------------------------

struct Interp<'a> {
    doc: &'a Document,
    out: Out,
    fonts: HashMap<ObjectId, Font>,
    /// Optional content group names by object id.
    ocg_names: HashMap<ObjectId, String>,
    want_text: bool,
    want_images: bool,
    /// Everything drawn goes on this layer (annotations).
    force_layer: Option<String>,
}

struct TextState {
    font: Option<ObjectId>,
    size: f64,
    char_space: f64,
    word_space: f64,
    hscale: f64,
    leading: f64,
    rise: f64,
    /// Text rendering mode (Tr): 3 and 7 are invisible (the searchable copy of lettering the
    /// PDF draws as linework).
    render: i64,
    tm: M,
    tlm: M,
}

impl Default for TextState {
    fn default() -> Self {
        TextState { font: None, size: 0.0, char_space: 0.0, word_space: 0.0, hscale: 1.0, leading: 0.0, rise: 0.0, render: 0, tm: M::ID, tlm: M::ID }
    }
}

#[derive(Clone, Copy)]
enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

impl<'a> Interp<'a> {
    fn font(&mut self, id: ObjectId) -> &Font {
        if !self.fonts.contains_key(&id) {
            let f = match self.doc.get_object(id) {
                Ok(Object::Dictionary(d)) => load_font(self.doc, d),
                _ => Font { default_width: 500.0, ..Font::default() },
            };
            self.fonts.insert(id, f);
        }
        static EMPTY: std::sync::OnceLock<Font> = std::sync::OnceLock::new();
        self.fonts.get(&id).unwrap_or_else(|| EMPTY.get_or_init(Font::default))
    }

    fn run(&mut self, data: &[u8], resources: Option<&Dictionary>, gs0: Gs, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(content) = Content::decode(data) else { return };
        // lopdf drops filtered inline images: read them from the bytes (the n-th BI is the n-th)
        let inline = if self.want_images { crate::pdf_image::scan_inline(data, resources, self.doc) } else { Vec::new() };
        let mut inline_at = 0usize;
        let mut gs = gs0;
        let mut stack: Vec<Gs> = Vec::new();
        let mut path: Vec<Seg> = Vec::new();
        let mut ts = TextState::default();
        let mut ocg: Vec<Option<String>> = Vec::new();
        let mut clip_pending = false;
        for op in &content.operations {
            self.out.ops += 1;
            if self.out.ops > MAX_OPS {
                return;
            }
            let a = &op.operands;
            let n = nums(a);
            match op.operator.as_str() {
                "q" => {
                    if stack.len() < 256 {
                        stack.push(gs.clone());
                    }
                }
                "Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                "cm" => {
                    if let [a, b, c, d, e, f] = n[..] {
                        let m = M([a, b, c, d, e, f]).then(gs.ctm);
                        if m.finite() {
                            gs.ctm = m;
                        }
                    }
                }
                "w" => {
                    if let Some(w) = n.first() {
                        gs.width = *w;
                    }
                }
                // graphics state parameters: line width and constant alpha
                "gs" => {
                    if let Some(Object::Name(nm)) = a.first()
                        && let Some(Object::Dictionary(eg)) = resources
                            .and_then(|r| dict_get(self.doc, r, b"ExtGState"))
                            .and_then(|e| e.as_dict().ok())
                            .and_then(|e| e.get(nm).ok())
                            .map(|o| deref(self.doc, o))
                    {
                        if let Some(v) = dict_get(self.doc, eg, b"ca").and_then(num) {
                            gs.fill_alpha = v;
                        }
                        if let Some(v) = dict_get(self.doc, eg, b"CA").and_then(num) {
                            gs.stroke_alpha = v;
                        }
                        if let Some(v) = dict_get(self.doc, eg, b"LW").and_then(num) {
                            gs.width = v;
                        }
                    }
                }
                "G" => gs.stroke = to_rgb(&n),
                "g" => gs.fill = to_rgb(&n),
                "RG" => gs.stroke = to_rgb(&n),
                "rg" => gs.fill = to_rgb(&n),
                "K" => gs.stroke = to_rgb(&n),
                "k" => gs.fill = to_rgb(&n),
                "CS" | "cs" => {
                    let comps = a.first().and_then(|o| o.as_name().ok()).map_or(1, |name| match name {
                        b"DeviceRGB" | b"CalRGB" => 3,
                        b"DeviceCMYK" => 4,
                        b"DeviceGray" | b"CalGray" => 1,
                        _ => 3,
                    });
                    if op.operator == "CS" {
                        gs.stroke_n = comps;
                        gs.stroke = [0.0; 3];
                    } else {
                        gs.fill_n = comps;
                        gs.fill = [0.0; 3];
                    }
                }
                "SC" | "SCN" => {
                    if n.len() >= gs.stroke_n.min(4) && !n.is_empty() {
                        gs.stroke = to_rgb(&n[..n.len().min(4)]);
                    }
                }
                "sc" | "scn" => {
                    if n.len() >= gs.fill_n.min(4) && !n.is_empty() {
                        gs.fill = to_rgb(&n[..n.len().min(4)]);
                    }
                }
                // path construction
                "m" => {
                    if let [x, y] = n[..] {
                        path.push(Seg::Move(x, y));
                    }
                }
                "l" => {
                    if let [x, y] = n[..] {
                        path.push(Seg::Line(x, y));
                    }
                }
                "c" => {
                    if let [x1, y1, x2, y2, x3, y3] = n[..] {
                        path.push(Seg::Curve(x1, y1, x2, y2, x3, y3));
                    }
                }
                "v" => {
                    if let [x2, y2, x3, y3] = n[..] {
                        let (x0, y0) = current(&path);
                        path.push(Seg::Curve(x0, y0, x2, y2, x3, y3));
                    }
                }
                "y" => {
                    if let [x1, y1, x3, y3] = n[..] {
                        path.push(Seg::Curve(x1, y1, x3, y3, x3, y3));
                    }
                }
                "h" => path.push(Seg::Close),
                "re" => {
                    if let [x, y, w, h] = n[..] {
                        path.extend([Seg::Move(x, y), Seg::Line(x + w, y), Seg::Line(x + w, y + h), Seg::Line(x, y + h), Seg::Close]);
                    }
                }
                "W" | "W*" => clip_pending = true,
                // painting
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                    let o = op.operator.as_str();
                    if matches!(o, "s" | "b" | "b*") {
                        path.push(Seg::Close);
                    }
                    let layer = self.force_layer.clone().or_else(|| ocg.iter().rev().find_map(|l| l.clone()));
                    if clip_pending {
                        // the new clip applies to what follows (and is popped with Q)
                        let loops: Vec<Vec<(f64, f64)>> = flatten(&path, &gs.ctm).into_iter().map(|s| s.0).filter(|l| l.len() >= 3).collect();
                        if !loops.is_empty() {
                            // nested rectangular clips (viewport inside viewport): their intersection
                            let new = match (gs.clip.as_deref().and_then(|c| clip_rect(c)), clip_rect(&loops)) {
                                (Some(a), Some(b)) => {
                                    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
                                    if r[0] < r[2] && r[1] < r[3] {
                                        vec![vec![(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])]]
                                    } else {
                                        loops
                                    }
                                }
                                _ => loops,
                            };
                            gs.clip = Some(std::rc::Rc::new(new));
                        }
                    }
                    if o != "n" {
                        let fill = matches!(o, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                        let stroke = matches!(o, "S" | "s" | "B" | "B*" | "b" | "b*");
                        self.paint(&path, &gs, fill, stroke, layer);
                    }
                    path.clear();
                    clip_pending = false;
                }
                // optional content (PDF layers)
                "BDC" => {
                    let mut name = None;
                    if a.first().and_then(|o| o.as_name().ok()) == Some(b"OC".as_slice()) {
                        let prop = a.get(1);
                        let ocg_obj = match prop {
                            Some(Object::Name(nm)) => resources
                                .and_then(|r| dict_get(self.doc, r, b"Properties"))
                                .and_then(|p| p.as_dict().ok())
                                .and_then(|p| p.get(nm).ok()),
                            Some(o) => Some(o),
                            None => None,
                        };
                        if let Some(Object::Reference(id)) = ocg_obj {
                            name = self.ocg_names.get(id).cloned();
                        }
                    }
                    ocg.push(name);
                }
                "BMC" => ocg.push(None),
                "EMC" => {
                    ocg.pop();
                }
                // text
                "BT" => {
                    ts.tm = M::ID;
                    ts.tlm = M::ID;
                }
                "Tf" => {
                    if let (Some(Object::Name(nm)), Some(size)) = (a.first(), a.get(1).and_then(num)) {
                        ts.size = size;
                        ts.font = resources
                            .and_then(|r| dict_get(self.doc, r, b"Font"))
                            .and_then(|f| f.as_dict().ok())
                            .and_then(|f| f.get(nm).ok())
                            .and_then(|o| o.as_reference().ok());
                    }
                }
                "Tc" => ts.char_space = n.first().copied().unwrap_or(0.0),
                "Tw" => ts.word_space = n.first().copied().unwrap_or(0.0),
                "Tz" => ts.hscale = n.first().copied().unwrap_or(100.0) / 100.0,
                "TL" => ts.leading = n.first().copied().unwrap_or(0.0),
                "Ts" => ts.rise = n.first().copied().unwrap_or(0.0),
                "Tr" => ts.render = n.first().map_or(0, |v| *v as i64),
                "Td" | "TD" => {
                    if let [tx, ty] = n[..] {
                        if op.operator == "TD" {
                            ts.leading = -ty;
                        }
                        ts.tlm = M([1.0, 0.0, 0.0, 1.0, tx, ty]).then(ts.tlm);
                        ts.tm = ts.tlm;
                    }
                }
                "Tm" => {
                    if let [a, b, c, d, e, f] = n[..] {
                        let m = M([a, b, c, d, e, f]);
                        if m.finite() {
                            ts.tlm = m;
                            ts.tm = m;
                        }
                    }
                }
                "T*" => {
                    ts.tlm = M([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(ts.tlm);
                    ts.tm = ts.tlm;
                }
                "Tj" | "'" | "\"" | "TJ" => {
                    if op.operator == "'" || op.operator == "\"" {
                        if op.operator == "\"" {
                            ts.word_space = n.first().copied().unwrap_or(ts.word_space);
                            ts.char_space = n.get(1).copied().unwrap_or(ts.char_space);
                        }
                        ts.tlm = M([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(ts.tlm);
                        ts.tm = ts.tlm;
                    }
                    let mut parts: Vec<(Vec<u8>, f64)> = Vec::new();
                    match op.operator.as_str() {
                        "TJ" => {
                            if let Some(Object::Array(items)) = a.first() {
                                let mut adj = 0.0;
                                for it in items {
                                    match it {
                                        Object::String(s, _) => {
                                            parts.push((s.clone(), adj));
                                            adj = 0.0;
                                        }
                                        o => adj += num(o).unwrap_or(0.0),
                                    }
                                }
                            }
                        }
                        _ => {
                            if let Some(Object::String(s, _)) = a.last() {
                                parts.push((s.clone(), 0.0));
                            }
                        }
                    }
                    // invisible text goes on its own layer, off: the sheet shows the lettering once
                    let layer = if matches!(ts.render, 3 | 7) {
                        Some(HIDDEN_TEXT_LAYER.to_string())
                    } else {
                        self.force_layer.clone().or_else(|| ocg.iter().rev().find_map(|l| l.clone()))
                    };
                    self.show_text(&mut ts, &gs, &parts, layer);
                }
                // form XObjects
                "Do" => {
                    let Some(Object::Name(nm)) = a.first() else { continue };
                    let Some(xo) =
                        resources.and_then(|r| dict_get(self.doc, r, b"XObject")).and_then(|x| x.as_dict().ok()).and_then(|x| x.get(nm).ok())
                    else {
                        continue;
                    };
                    let Object::Stream(st) = deref(self.doc, xo) else { continue };
                    match st.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) {
                        Some(b"Form") => {
                            let m = st
                                .dict
                                .get(b"Matrix")
                                .ok()
                                .and_then(|o| deref(self.doc, o).as_array().ok())
                                .map(|arr| nums(arr))
                                .and_then(|v| <[f64; 6]>::try_from(v).ok())
                                .map(M)
                                .unwrap_or(M::ID);
                            let mut g = gs.clone();
                            g.ctm = m.then(gs.ctm);
                            let res = st.dict.get(b"Resources").ok().and_then(|o| deref(self.doc, o).as_dict().ok()).or(resources);
                            let data = st.decompressed_content_with_limit(MAX_STREAM_BYTES).unwrap_or_else(|_| st.content.clone());
                            self.run(&data, res, g, depth + 1);
                        }
                        Some(b"Image") => {
                            let layer = self.force_layer.clone().or_else(|| ocg.iter().rev().find_map(|l| l.clone()));
                            self.image(&st.dict, &st.content, resources, &gs, layer);
                        }
                        _ => {}
                    }
                }
                "BI" => {
                    let found = inline.get(inline_at).cloned().flatten();
                    inline_at += 1;
                    match found {
                        Some((dict, bytes)) => {
                            let layer = self.force_layer.clone().or_else(|| ocg.iter().rev().find_map(|l| l.clone()));
                            self.image(&dict, &bytes, resources, &gs, layer);
                        }
                        None if self.want_images => *self.out.image_failures.entry("damaged or unsupported").or_default() += 1,
                        None => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Decode an image (XObject or inline) and keep it with where it lands.
    fn image(&mut self, dict: &Dictionary, raw: &[u8], resources: Option<&Dictionary>, gs: &Gs, layer: Option<String>) {
        if !self.want_images || !gs.ctm.finite() {
            return;
        }
        match crate::pdf_image::decode(self.doc, dict, raw, resources, rgb8(gs.fill)) {
            Ok(raster) if self.out.image_bytes.saturating_add(raster.rgba.len()) <= MAX_IMAGE_BYTES => {
                self.out.image_bytes += raster.rgba.len();
                self.out.images.push(ImageOut {
                    ctm: gs.ctm,
                    raster: std::sync::Arc::new(raster),
                    layer,
                    clip: gs.clip.clone(),
                    transp: transp(gs.fill_alpha),
                    order: self.out.seq,
                });
                self.out.seq += 1;
            }
            Ok(_) => *self.out.image_failures.entry("too much image data").or_default() += 1,
            Err(e) => *self.out.image_failures.entry(e.label()).or_default() += 1,
        }
    }

    /// Draw the page's annotations from their normal appearance streams, on [`MARKUP_LAYER`].
    /// The appearance's box (BBox through its Matrix) is fitted to the annotation's Rect.
    fn annotations(&mut self, page: &Dictionary, page_box: [f64; 4]) -> usize {
        let Some(Object::Array(annots)) = dict_get(self.doc, page, b"Annots") else { return 0 };
        let mut drawn = 0;
        for a in annots.iter().take(10_000) {
            let Object::Dictionary(ad) = deref(self.doc, a) else { continue };
            let sub = dict_get(self.doc, ad, b"Subtype").and_then(|o| o.as_name().ok()).unwrap_or(b"");
            if matches!(sub, b"Link" | b"Popup") {
                continue;
            }
            // hidden (2) or not shown on screen (32)
            let flags = dict_get(self.doc, ad, b"F").and_then(num).unwrap_or(0.0) as i64;
            if flags & (2 | 32) != 0 {
                continue;
            }
            let Some(rect) =
                dict_get(self.doc, ad, b"Rect").and_then(|o| o.as_array().ok()).map(|v| nums(v)).and_then(|v| <[f64; 4]>::try_from(v).ok())
            else {
                continue;
            };
            let Some(Object::Dictionary(ap)) = dict_get(self.doc, ad, b"AP") else { continue };
            let n = match dict_get(self.doc, ap, b"N") {
                Some(Object::Stream(st)) => Some(st),
                Some(Object::Dictionary(states)) => {
                    let st_name = dict_get(self.doc, ad, b"AS").and_then(|o| o.as_name().ok()).map(<[u8]>::to_vec);
                    st_name.and_then(|k| dict_get(self.doc, states, &k)).and_then(|o| o.as_stream().ok())
                }
                _ => None,
            };
            let Some(st) = n else { continue };
            let bbox = st
                .dict
                .get(b"BBox")
                .ok()
                .and_then(|o| deref(self.doc, o).as_array().ok())
                .map(|v| nums(v))
                .and_then(|v| <[f64; 4]>::try_from(v).ok());
            let Some([bx0, by0, bx1, by1]) = bbox else { continue };
            let mat = st
                .dict
                .get(b"Matrix")
                .ok()
                .and_then(|o| deref(self.doc, o).as_array().ok())
                .map(|v| nums(v))
                .and_then(|v| <[f64; 6]>::try_from(v).ok())
                .map(M)
                .unwrap_or(M::ID);
            // the appearance box after its matrix, then onto Rect
            let corners = [(bx0, by0), (bx1, by0), (bx1, by1), (bx0, by1)].map(|(x, y)| mat.apply(x, y));
            let (tx0, tx1) = corners.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
            let (ty0, ty1) = corners.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
            let [rx0, ry0, rx1, ry1] = rect;
            let (rx0, rx1, ry0, ry1) = (rx0.min(rx1), rx0.max(rx1), ry0.min(ry1), ry0.max(ry1));
            if tx1 - tx0 <= 1e-9 || ty1 - ty0 <= 1e-9 {
                continue;
            }
            let (sx, sy) = ((rx1 - rx0) / (tx1 - tx0), (ry1 - ry0) / (ty1 - ty0));
            let fit = M([sx, 0.0, 0.0, sy, rx0 - tx0 * sx, ry0 - ty0 * sy]);
            // Clipped to the annotation's Rect and the page, as viewers show it: a markup drawn
            // off the sheet (a PDF-XChange dimension with a bogus /LL leader length) stays off it.
            let [px0, py0, px1, py1] = page_box;
            let (cx0, cx1, cy0, cy1) = (rx0.max(px0.min(px1)), rx1.min(px0.max(px1)), ry0.max(py0.min(py1)), ry1.min(py0.max(py1)));
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            let frame = vec![vec![(cx0, cy0), (cx1, cy0), (cx1, cy1), (cx0, cy1)]];
            let g = Gs { ctm: mat.then(fit), clip: Some(std::rc::Rc::new(frame)), ..Gs::default() };
            if !g.ctm.finite() {
                continue;
            }
            let res = st.dict.get(b"Resources").ok().and_then(|o| deref(self.doc, o).as_dict().ok());
            let data = st.decompressed_content_with_limit(MAX_STREAM_BYTES).unwrap_or_else(|_| st.content.clone());
            self.force_layer = Some(MARKUP_LAYER.to_string());
            let texts_before = self.out.texts.len();
            self.run(&data, res, g, 1);
            self.force_layer = None;
            let slack = ((cx1 - cx0) + (cy1 - cy0)) * 0.05 + 1.0;
            let inside = |p: (f64, f64)| p.0 >= cx0 - slack && p.0 <= cx1 + slack && p.1 >= cy0 - slack && p.1 <= cy1 + slack;
            let mut i = 0usize;
            self.out.texts.retain(|t| {
                i += 1;
                i <= texts_before || inside(t.at)
            });
            drawn += 1;
        }
        drawn
    }

    fn show_text(&mut self, ts: &mut TextState, gs: &Gs, parts: &[(Vec<u8>, f64)], layer: Option<String>) {
        let Some(fid) = ts.font else { return };
        let (size, cs, ws, hs, rise) = (ts.size, ts.char_space, ts.word_space, ts.hscale, ts.rise);
        let font = self.font(fid);
        let style = font.style.clone();
        let mut glyphs: Vec<(String, f64, f64)> = Vec::new(); // (text, adjustment before, advance)
        for (bytes, adj) in parts {
            let mut first = true;
            for code in codes(font, bytes) {
                let w = font.widths.get(&code).copied().unwrap_or(font.default_width) / 1000.0;
                let s = decode(font, code);
                let space = if !font.two_byte && code == 32 { ws } else { 0.0 };
                glyphs.push((s, if first { *adj } else { 0.0 }, (w * size + cs + space) * hs));
                first = false;
            }
        }
        // Some writers pack separate labels into one TJ with large jumps between them: a jump of
        // more than a character width starts a new text where it lands; a smaller gap is a space.
        let mut runs: Vec<(M, String, M)> = Vec::new();
        let mut value = String::new();
        let mut start: Option<M> = None;
        for (s, adj, advance) in glyphs {
            if adj != 0.0 {
                if adj.abs() > 1000.0 && start.is_some() {
                    if let Some(m) = start.take() {
                        runs.push((m, std::mem::take(&mut value), ts.tm));
                    }
                } else if adj < -200.0 && !value.is_empty() && !value.ends_with(' ') {
                    value.push(' ');
                }
                ts.tm = M([1.0, 0.0, 0.0, 1.0, -adj / 1000.0 * size * hs, 0.0]).then(ts.tm);
            }
            if start.is_none() {
                start = Some(ts.tm);
            }
            value.push_str(&s);
            ts.tm = M([1.0, 0.0, 0.0, 1.0, advance, 0.0]).then(ts.tm);
        }
        if let Some(m) = start {
            runs.push((m, value, ts.tm));
        }
        if !self.want_text {
            return;
        }
        for (tm, value, end_tm) in runs {
            let value = value.replace('\u{0}', "");
            if value.trim().is_empty() {
                continue;
            }
            // rendering matrix: [size·Th 0 0 size 0 rise] × Tm × CTM
            let trm = M([size * hs, 0.0, 0.0, size, 0.0, rise]).then(tm).then(gs.ctm);
            if !trm.finite() {
                continue;
            }
            let [a, b, c, d, e, f] = trm.0;
            let (xlen, ylen) = (a.hypot(b), c.hypot(d));
            if ylen < 1e-9 {
                continue;
            }
            // leading spaces move the start, not the text
            let lead = value.len() - value.trim_start().len();
            let shift = lead as f64 * 0.278 * xlen / hs.max(1e-9) * hs;
            let (ux, uy) = (a / xlen.max(1e-12), b / xlen.max(1e-12));
            let em = M([size * hs, 0.0, 0.0, size, 0.0, rise]).then(end_tm).then(gs.ctm);
            self.out.texts.push(TextOut {
                at: (e + ux * shift, f + uy * shift),
                end: (em.0[4], em.0[5]),
                // cap height of the common sans fonts, as CAD text heights are
                height: ylen * 0.716,
                rotation: b.atan2(a).to_degrees(),
                width_factor: if hs > 0.0 { (xlen / ylen).clamp(0.05, 20.0) } else { 1.0 },
                value: value.trim().to_string(),
                pen: Pen { rgb: rgb8(gs.fill), lw: 0, transp: transp(gs.fill_alpha) },
                layer: layer.clone(),
                style: style.clone(),
                order: self.out.seq,
            });
            self.out.seq += 1;
        }
    }

    fn paint(&mut self, path: &[Seg], gs: &Gs, fill: bool, stroke: bool, layer: Option<String>) {
        let subs = flatten(path, &gs.ctm);
        if subs.is_empty() {
            return;
        }
        let order = self.out.seq;
        self.out.seq += 1;
        let clip_box = gs.clip.as_deref().and_then(|c| bbox(c));
        let outside = |b: [f64; 4]| clip_box.is_some_and(|c| b[2] < c[0] - 0.01 || b[0] > c[2] + 0.01 || b[3] < c[1] - 0.01 || b[1] > c[3] + 0.01);
        if fill {
            let pen = Pen { rgb: rgb8(gs.fill), lw: 0, transp: transp(gs.fill_alpha) };
            let mut loops: Vec<Vec<(f64, f64)>> = subs.iter().map(|s| s.0.clone()).collect();
            if let (Some(c), Some(fb), Some(clip)) = (clip_box, bbox(&loops), gs.clip.as_deref()) {
                if outside(fb) {
                    loops.clear();
                } else if let Some(r) =
                    clip_rect(clip).filter(|r| fb[0] < r[0] - 0.01 || fb[1] < r[1] - 0.01 || fb[2] > r[2] + 0.01 || fb[3] > r[3] + 0.01)
                {
                    // a fill crossing a rectangular clip: cut at it, unless it covers it (then it is the clip)
                    if fb[0] <= r[0] + 0.01
                        && fb[1] <= r[1] + 0.01
                        && fb[2] >= r[2] - 0.01
                        && fb[3] >= r[3] - 0.01
                        && loops.len() == 1
                        && clip_rect(&loops).is_some()
                    {
                        loops = clip.clone();
                    } else {
                        loops = loops.iter().map(|l| clip_polygon(l, r)).filter(|l| l.len() >= 3).collect();
                    }
                } else if fb[0] <= c[0] + 0.01 && fb[1] <= c[1] + 0.01 && fb[2] >= c[2] - 0.01 && fb[3] >= c[3] - 0.01 {
                    // a fill over the whole clip: the clip outline is the shape
                    loops = clip.clone();
                    self.out.clipped_ops += 1;
                }
            }
            if !loops.is_empty() {
                self.out.fills.push(Fill { loops, pen, layer: layer.clone(), order });
            }
        }
        if stroke {
            let pen = Pen { rgb: rgb8(gs.stroke), lw: lineweight(gs.width * gs.ctm.scale()), transp: transp(gs.stroke_alpha) };
            let rect = gs.clip.as_deref().and_then(|c| clip_rect(c));
            for (pts, closed, curves) in subs {
                if bbox(std::slice::from_ref(&pts)).is_some_and(outside) {
                    continue;
                }
                if let Some(r) = rect
                    && bbox(std::slice::from_ref(&pts))
                        .is_some_and(|b| b[0] < r[0] - 0.01 || b[1] < r[1] - 0.01 || b[2] > r[2] + 0.01 || b[3] > r[3] + 0.01)
                {
                    let mut ring = pts.clone();
                    if closed && let Some(f) = ring.first().copied() {
                        ring.push(f);
                    }
                    for piece in clip_polyline(&ring, r) {
                        self.out.strokes.push(Stroke { pts: piece, closed: false, pen, layer: layer.clone(), curve_loop: false, order });
                    }
                    continue;
                }
                self.out.strokes.push(Stroke { pts, closed, pen, layer: layer.clone(), curve_loop: closed && curves == 4, order });
            }
        }
    }
}

/// A path's subpaths in sheet points: (points, closed, Bézier segments).
fn flatten(path: &[Seg], ctm: &M) -> Vec<(Vec<(f64, f64)>, bool, usize)> {
    let ctm = *ctm;
    // subpaths in sheet points
    let mut subs: Vec<(Vec<(f64, f64)>, bool, usize)> = Vec::new(); // points, closed, curve count
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut closed = false;
    let mut curves = 0usize;
    let mut total = 0usize;
    let mut last = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for seg in path {
        match *seg {
            Seg::Move(x, y) => {
                if cur.len() > 1 {
                    subs.push((std::mem::take(&mut cur), closed, curves));
                }
                cur.clear();
                closed = false;
                curves = 0;
                last = (x, y);
                start = last;
                cur.push(ctm.apply(x, y));
            }
            Seg::Line(x, y) => {
                if cur.is_empty() {
                    cur.push(ctm.apply(last.0, last.1));
                }
                last = (x, y);
                cur.push(ctm.apply(x, y));
            }
            Seg::Curve(x1, y1, x2, y2, x3, y3) => {
                if cur.is_empty() {
                    cur.push(ctm.apply(last.0, last.1));
                }
                let (x0, y0) = last;
                for k in 1..=CURVE_STEPS {
                    let t = k as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    let x = u * u * u * x0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x3;
                    let y = u * u * u * y0 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y3;
                    cur.push(ctm.apply(x, y));
                }
                curves += 1;
                last = (x3, y3);
            }
            Seg::Close => {
                closed = true;
                last = start;
            }
        }
        total += 1;
        if total > MAX_PATH_POINTS {
            break;
        }
    }
    if cur.len() > 1 {
        subs.push((cur, closed, curves));
    }
    subs
}

/// The clip as an axis-aligned rectangle [x0, y0, x1, y1], when it is one (a viewport's clip).
fn clip_rect(loops: &[Vec<(f64, f64)>]) -> Option<[f64; 4]> {
    let [l] = loops else { return None };
    let pts: Vec<(f64, f64)> = if l.len() == 5 && l.first() == l.last() { l[..4].to_vec() } else { l.clone() };
    if pts.len() != 4 {
        return None;
    }
    let b = bbox(std::slice::from_ref(&pts))?;
    let on_edge = |(x, y): (f64, f64)| ((x - b[0]).abs() < 0.01 || (x - b[2]).abs() < 0.01) && ((y - b[1]).abs() < 0.01 || (y - b[3]).abs() < 0.01);
    pts.iter().all(|p| on_edge(*p)).then_some(b)
}

/// Cut a polyline at a rectangle: the pieces inside (Liang–Barsky per segment).
fn clip_polyline(pts: &[(f64, f64)], r: [f64; 4]) -> Vec<Vec<(f64, f64)>> {
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    for w in pts.windows(2) {
        let (&(x0, y0), &(x1, y1)) = match w {
            [a, b] => (a, b),
            _ => continue,
        };
        let (dx, dy) = (x1 - x0, y1 - y0);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut keep = true;
        for (p, q) in [(-dx, x0 - r[0]), (dx, r[2] - x0), (-dy, y0 - r[1]), (dy, r[3] - y0)] {
            if p.abs() < 1e-12 {
                if q < 0.0 {
                    keep = false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if !keep || t0 > t1 {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        let a = (x0 + t0 * dx, y0 + t0 * dy);
        let b = (x0 + t1 * dx, y0 + t1 * dy);
        if cur.last().is_none_or(|l| (l.0 - a.0).abs() > 1e-9 || (l.1 - a.1).abs() > 1e-9) {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur = vec![a];
        }
        cur.push(b);
        if t1 < 1.0 {
            out.push(std::mem::take(&mut cur));
        }
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// Cut a polygon at a rectangle (Sutherland–Hodgman).
fn clip_polygon(pts: &[(f64, f64)], r: [f64; 4]) -> Vec<(f64, f64)> {
    let mut poly = pts.to_vec();
    for edge in 0..4 {
        let inside = |p: (f64, f64)| match edge {
            0 => p.0 >= r[0],
            1 => p.0 <= r[2],
            2 => p.1 >= r[1],
            _ => p.1 <= r[3],
        };
        let cut = |a: (f64, f64), b: (f64, f64)| {
            let t = match edge {
                0 => (r[0] - a.0) / (b.0 - a.0),
                1 => (r[2] - a.0) / (b.0 - a.0),
                2 => (r[1] - a.1) / (b.1 - a.1),
                _ => (r[3] - a.1) / (b.1 - a.1),
            };
            (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))
        };
        let n = poly.len();
        let mut next = Vec::with_capacity(n + 4);
        for i in 0..n {
            let (Some(&cur), Some(&prev)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
            match (inside(prev), inside(cur)) {
                (true, true) => next.push(cur),
                (true, false) => next.push(cut(prev, cur)),
                (false, true) => {
                    next.push(cut(prev, cur));
                    next.push(cur);
                }
                (false, false) => {}
            }
        }
        poly = next;
        if poly.is_empty() {
            break;
        }
    }
    poly
}

fn bbox(loops: &[Vec<(f64, f64)>]) -> Option<[f64; 4]> {
    let mut it = loops.iter().flatten();
    let (x, y) = *it.next()?;
    Some(it.fold([x, y, x, y], |b, (x, y)| [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)]))
}

fn current(path: &[Seg]) -> (f64, f64) {
    for s in path.iter().rev() {
        match *s {
            Seg::Move(x, y) | Seg::Line(x, y) | Seg::Curve(_, _, _, _, x, y) => return (x, y),
            Seg::Close => continue,
        }
    }
    (0.0, 0.0)
}

// ---------------------------------------------------------------------------------------------
// Clean-up: duplicates, joining, circles
// ---------------------------------------------------------------------------------------------

fn key_pts(pts: &[(f64, f64)]) -> Vec<(i64, i64)> {
    pts.iter().map(|(x, y)| ((x * 100.0).round() as i64, (y * 100.0).round() as i64)).collect()
}

/// Join open strokes of one pen and layer whose ends touch, end to end.
fn join(strokes: Vec<Stroke>) -> Vec<Stroke> {
    const EPS: f64 = 0.05; // points
    let q = |p: (f64, f64)| ((p.0 / EPS).round() as i64, (p.1 / EPS).round() as i64);
    let mut out: Vec<Stroke> = Vec::new();
    let mut open: Vec<Option<Stroke>> = Vec::new();
    for s in strokes {
        if s.closed || s.pts.len() < 2 {
            out.push(s);
        } else {
            open.push(Some(s));
        }
    }
    // endpoint index per (pen, layer)
    let mut ends: HashMap<((i64, i64), Pen, Option<String>), Vec<usize>> = HashMap::new();
    for (i, s) in open.iter().enumerate() {
        if let Some(s) = s {
            for p in [s.pts.first(), s.pts.last()].into_iter().flatten() {
                ends.entry((q(*p), s.pen, s.layer.clone())).or_default().push(i);
            }
        }
    }
    for i in 0..open.len() {
        let Some(mut cur) = open.get_mut(i).and_then(Option::take) else { continue };
        let mut guard = 0;
        // grow at the end, then at the start
        for at_end in [true, false] {
            loop {
                guard += 1;
                if guard > 100_000 {
                    break;
                }
                let tip = if at_end { cur.pts.last() } else { cur.pts.first() };
                let Some(tip) = tip.copied() else { break };
                let key = (q(tip), cur.pen, cur.layer.clone());
                let cand = ends.get(&key).and_then(|v| v.iter().copied().find(|j| *j != i && open.get(*j).is_some_and(Option::is_some)));
                let Some(j) = cand else { break };
                let Some(mut next) = open.get_mut(j).and_then(Option::take) else { break };
                cur.order = cur.order.min(next.order);
                // orient `next` to continue from `tip`
                let starts_here = next.pts.first().is_some_and(|p| q(*p) == q(tip));
                if at_end {
                    if !starts_here {
                        next.pts.reverse();
                    }
                    cur.pts.extend(next.pts.into_iter().skip(1));
                } else {
                    if starts_here {
                        next.pts.reverse();
                    }
                    let mut pts = next.pts;
                    pts.pop();
                    pts.extend(cur.pts);
                    cur.pts = pts;
                }
                if cur.pts.len() > 2 && cur.pts.first().map(|p| q(*p)) == cur.pts.last().map(|p| q(*p)) {
                    cur.pts.pop();
                    cur.closed = true;
                    break;
                }
            }
            if cur.closed {
                break;
            }
        }
        out.push(cur);
    }
    out
}

/// Join text pieces that continue each other on one baseline (writers that place every
/// character, or every word, on its own): same pen, layer, style, height and angle, the next
/// starting where the previous ends; a gap of about a space's width becomes a space.
fn merge_texts(texts: Vec<TextOut>) -> Vec<TextOut> {
    let mut out: Vec<TextOut> = Vec::with_capacity(texts.len());
    for t in texts {
        if let Some(prev) = out.last_mut() {
            let same = prev.pen == t.pen
                && prev.layer == t.layer
                && prev.style == t.style
                && (prev.height - t.height).abs() <= prev.height * 0.02
                && (prev.rotation - t.rotation).abs() < 0.5
                && (prev.width_factor - t.width_factor).abs() < 0.02;
            if same {
                let r = prev.rotation.to_radians();
                let (ux, uy) = (r.cos(), r.sin());
                let (dx, dy) = (t.at.0 - prev.end.0, t.at.1 - prev.end.1);
                let along = dx * ux + dy * uy;
                let across = -dx * uy + dy * ux;
                let em = prev.height / 0.716;
                // a wider gap is a tab or a column: the pieces stay where they are
                if across.abs() < prev.height * 0.2 && along > -0.15 * em && along < 0.6 * em {
                    if along > 0.2 * em && !prev.value.ends_with(' ') {
                        prev.value.push(' ');
                    }
                    prev.value.push_str(&t.value);
                    prev.end = t.end;
                    // drawn when its last piece was: a mask painted between the pieces (behind
                    // the later one) must not cover the joined text
                    prev.order = prev.order.max(t.order);
                    continue;
                }
            }
        }
        out.push(t);
    }
    out
}

/// A closed loop that is round: its centre and radius (sheet points).
fn as_circle(s: &Stroke) -> Option<((f64, f64), f64)> {
    if !s.closed || s.pts.len() < 12 && !s.curve_loop {
        return None;
    }
    let n = s.pts.len() as f64;
    let cx = s.pts.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = s.pts.iter().map(|p| p.1).sum::<f64>() / n;
    let ds: Vec<f64> = s.pts.iter().map(|p| (p.0 - cx).hypot(p.1 - cy)).collect();
    let r = ds.iter().sum::<f64>() / n;
    if r.is_nan() || r <= 0.05 {
        return None;
    }
    let worst = ds.iter().map(|d| (d - r).abs()).fold(0.0, f64::max);
    if worst > r * 0.015 + 0.02 {
        return None;
    }
    // Every corner on a circle isn't enough (a faceplate of two arcs and two short sides has
    // that): the outline must go all the way round — its length is the circumference and no
    // direction is skipped.
    let len: f64 = s
        .pts
        .windows(2)
        .map(|w| match w {
            [a, b] => (b.0 - a.0).hypot(b.1 - a.1),
            _ => 0.0,
        })
        .sum::<f64>()
        + s.pts.first().zip(s.pts.last()).map_or(0.0, |(a, b)| (b.0 - a.0).hypot(b.1 - a.1));
    let circumference = std::f64::consts::TAU * r;
    if (len - circumference).abs() > circumference * 0.03 {
        return None;
    }
    let mut angles: Vec<f64> = s.pts.iter().map(|p| (p.1 - cy).atan2(p.0 - cx)).collect();
    angles.sort_by(f64::total_cmp);
    let wrap = angles.first().zip(angles.last()).map_or(std::f64::consts::TAU, |(a, b)| a + std::f64::consts::TAU - b);
    let gap = angles
        .windows(2)
        .map(|w| match w {
            [a, b] => b - a,
            _ => 0.0,
        })
        .fold(wrap, f64::max);
    (gap <= std::f64::consts::FRAC_PI_4).then_some(((cx, cy), r))
}

// ---------------------------------------------------------------------------------------------
// Scale note
// ---------------------------------------------------------------------------------------------

/// Drawing units (inches) per sheet inch from a note like `1/8" = 1'-0"`, `3/16"=1'-0"`,
/// `1" = 20'`, `1:100`.
pub fn parse_scale(text: &str) -> Option<f64> {
    let t = text.replace(['\u{2019}', '\u{2032}'], "'").replace(['\u{201D}', '\u{2033}'], "\"");
    let t = t.to_ascii_uppercase();
    let t = t.trim_start_matches("SCALE").trim_start_matches(':').trim();
    // architectural / engineering: <paper>" = <feet>'-<inches>"
    if let Some((lhs, rhs)) = t.split_once('=') {
        let lhs = lhs.trim().trim_end_matches('"').trim();
        let paper = if let Some((a, b)) = lhs.split_once('/') {
            let (whole, num) = match a.trim().split_once(' ') {
                Some((w, n)) => (w.trim().parse::<f64>().ok()?, n.trim().parse::<f64>().ok()?),
                None => (0.0, a.trim().parse::<f64>().ok()?),
            };
            whole + num / b.trim().parse::<f64>().ok()?
        } else {
            lhs.parse::<f64>().ok()?
        };
        let rhs = rhs.trim();
        let (ft, inch) = match rhs.split_once('\'') {
            Some((f, rest)) => {
                let i = rest.trim().trim_start_matches('-').trim().trim_end_matches('"').trim();
                (f.trim().parse::<f64>().ok()?, if i.is_empty() { 0.0 } else { i.parse::<f64>().ok()? })
            }
            None => (0.0, rhs.trim_end_matches('"').trim().parse::<f64>().ok()?),
        };
        let real = ft * 12.0 + inch;
        return (paper > 0.0 && real > 0.0).then(|| real / paper).filter(|k| k.is_finite() && (0.001..=1e6).contains(k));
    }
    // metric ratio 1:100 (drawing in the sheet's units: inches)
    let (a, b) = t.split_once(':')?;
    let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
    (a > 0.0 && b > 0.0).then(|| b / a)
}

// ---------------------------------------------------------------------------------------------
// The whole page
// ---------------------------------------------------------------------------------------------

/// Read page `opts.page` as a drawing.
pub fn read(bytes: &[u8], opts: &PdfOptions) -> Result<PdfImport> {
    let doc = load(bytes)?;
    let pages = doc.get_pages();
    let page_total = u32::try_from(pages.len()).unwrap_or(u32::MAX);
    let page_id = *pages.get(&opts.page.max(1)).ok_or_else(|| err(format!("page {} doesn't exist (the file has {page_total})", opts.page)))?;
    let page = doc.get_dictionary(page_id).map_err(|e| err(e.to_string()))?;

    // the page box (inherited from parents if need be)
    let inherited = |key: &[u8]| -> Option<&Object> {
        let mut d = page;
        for _ in 0..32 {
            if let Some(o) = dict_get(&doc, d, key) {
                return Some(o);
            }
            d = dict_get(&doc, d, b"Parent")?.as_dict().ok()?;
        }
        None
    };
    let mbox = inherited(b"CropBox")
        .or_else(|| inherited(b"MediaBox"))
        .and_then(|o| o.as_array().ok())
        .map(|a| nums(a))
        .and_then(|v| <[f64; 4]>::try_from(v).ok())
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let [bx0, by0, bx1, by1] = mbox;
    let (x0, y0) = (bx0.min(bx1), by0.min(by1));
    let (pw, ph) = ((bx1 - bx0).abs(), (by1 - by0).abs());
    let rotate = inherited(b"Rotate").and_then(num).map_or(0, |r| ((r as i64 % 360) + 360) % 360);

    // optional content group names
    let mut ocg_names = HashMap::new();
    if let Ok(cat) = doc.catalog()
        && let Some(Object::Dictionary(ocp)) = dict_get(&doc, cat, b"OCProperties")
        && let Some(Object::Array(list)) = dict_get(&doc, ocp, b"OCGs")
    {
        for o in list {
            if let Object::Reference(id) = o
                && let Ok(Object::Dictionary(g)) = doc.get_object(*id)
                && let Some(Object::String(name, _)) = dict_get(&doc, g, b"Name")
            {
                let name = decode_pdf_string(name);
                let name: String =
                    name.chars().filter(|c| !matches!(c, '<' | '>' | '/' | '\\' | '"' | ':' | ';' | '?' | '*' | '|' | ',' | '=' | '`')).collect();
                if !name.trim().is_empty() {
                    ocg_names.insert(*id, name.trim().to_string());
                }
            }
        }
    }

    let resources =
        dict_get(&doc, page, b"Resources").and_then(|o| o.as_dict().ok()).or_else(|| inherited(b"Resources").and_then(|o| o.as_dict().ok()));
    let content = doc.get_page_content(page_id);
    let mut it = Interp {
        doc: &doc,
        out: Out::default(),
        fonts: HashMap::new(),
        ocg_names: ocg_names.clone(),
        want_text: opts.text,
        want_images: opts.images,
        force_layer: None,
    };
    it.run(&content, resources, Gs::default(), 0);
    let mut markups = 0usize;
    if opts.markups {
        markups = it.annotations(page, mbox);
    }
    let out = it.out;
    if out.ops > MAX_OPS {
        return Err(err("the page is too complex (more than 20 million drawing operations)"));
    }

    // sheet points → displayed sheet inches (page rotation applied)
    let to_sheet = |x: f64, y: f64| -> (f64, f64) {
        let (x, y) = (x - x0, y - y0);
        let (x, y) = match rotate {
            90 => (y, pw - x),
            180 => (pw - x, ph - y),
            270 => (ph - y, x),
            _ => (x, y),
        };
        (x / 72.0, y / 72.0)
    };
    let sheet = if rotate == 90 || rotate == 270 { (ph / 72.0, pw / 72.0) } else { (pw / 72.0, ph / 72.0) };
    let rot_deg = match rotate {
        90 => -90.0,
        180 => 180.0,
        270 => 90.0,
        _ => 0.0,
    };

    // scale: the option, else the sheet's scale note (the most common one)
    let mut notes_count: HashMap<String, (f64, usize)> = HashMap::new();
    for t in &out.texts {
        let v = t.value.trim();
        let up = v.to_ascii_uppercase();
        if (up.contains('=') || up.starts_with("SCALE"))
            && (up.contains('\'') || up.contains('"') || up.contains(':'))
            && let Some(k) = parse_scale(v)
        {
            let e = notes_count.entry(v.trim_start_matches(|c: char| !c.is_ascii_digit()).to_string()).or_insert((k, 0));
            e.1 += 1;
        }
    }
    let best_note = notes_count.into_iter().max_by(|a, b| a.1.1.cmp(&b.1.1).then(b.0.cmp(&a.0)));
    let (scale, scale_source) = match (opts.scale, &best_note) {
        (Some(k), _) if k.is_finite() && k > 0.0 => (k, "option".to_string()),
        (_, Some((note, (k, _)))) => (*k, format!("scale note {note}")),
        _ => (1.0, "none (the sheet in inches)".to_string()),
    };

    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    let mut notes = Vec::new();

    // duplicates
    let mut seen: HashSet<(Vec<(i64, i64)>, Pen, Option<String>)> = HashSet::new();
    let mut strokes = Vec::new();
    for s in out.strokes {
        if seen.insert((key_pts(&s.pts), s.pen, s.layer.clone())) {
            strokes.push(s);
        } else {
            *stats.entry("duplicates removed".into()).or_default() += 1;
        }
    }
    let raw_strokes = strokes.len();
    let strokes = if opts.join { join(strokes) } else { strokes };
    if opts.join && strokes.len() < raw_strokes {
        stats.insert("pieces joined".into(), raw_strokes - strokes.len());
    }
    let mut fills = Vec::new();
    let mut masks: Vec<Fill> = Vec::new();
    for f in out.fills {
        if !opts.white_fills && f.pen.rgb.iter().all(|c| *c >= 250) {
            if opts.masks {
                masks.push(f);
            } else {
                *stats.entry("white fills left out".into()).or_default() += 1;
            }
            continue;
        }
        let key = (f.loops.iter().flat_map(|l| key_pts(l)).collect::<Vec<_>>(), f.pen, f.layer.clone());
        if seen.insert(key) {
            fills.push(f);
        } else {
            *stats.entry("duplicates removed".into()).or_default() += 1;
        }
    }
    let before = out.texts.len();
    let merged = merge_texts(out.texts);
    if merged.len() < before {
        stats.insert("text pieces joined".into(), before - merged.len());
    }
    let mut texts = Vec::new();
    let mut seen_text: HashSet<(String, i64, i64)> = HashSet::new();
    for t in merged {
        if seen_text.insert((t.value.clone(), (t.at.0 * 10.0).round() as i64, (t.at.1 * 10.0).round() as i64)) {
            texts.push(t);
        } else {
            *stats.entry("duplicates removed".into()).or_default() += 1;
        }
    }

    let white: HashSet<usize> = masks.iter().map(|f| f.order).collect();
    let (images, stitched) = stitch(out.images, &white);
    if stitched > 0 {
        stats.insert("image pieces stitched".into(), stitched);
    }

    // the drawing
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1);
    // masks and images show without a frame, as on the sheet
    d.header.set_i64("WIPEOUTFRAME", 0);
    d.header.set_i64("IMAGEFRAME", 0);
    if scale >= 12.0 {
        d.header.set_i64("LUNITS", 4); // architectural
    }
    // What the user adds is sized for the sheet's scale: dimensions (DIMSCALE), annotative
    // objects and symbols (CANNOSCALE, when the list has the scale), text (3/32" on the sheet).
    if scale.is_finite() && scale > 1.0 {
        d.header.set_f64("DIMSCALE", scale);
        d.header.set_f64("TEXTSIZE", 0.09375 * scale);
        if let Some(name) = d.anno_scale_or_ratio(&format!("1:{scale}")).map(|sc| sc.name.clone()) {
            d.header.set_str("CANNOSCALE", &name);
        }
    }
    let use_ocg = opts.layers == LayerMode::Pdf && !ocg_names.is_empty();
    let mode = if use_ocg {
        LayerMode::Pdf
    } else if opts.layers == LayerMode::Object {
        LayerMode::Object
    } else {
        LayerMode::Pen
    };
    let mut layer_set: BTreeMap<String, (Color, Lineweight)> = BTreeMap::new();
    // black and near-black greys (up to about 15%) are colour 7: white on a dark screen, black on
    // paper. As true colours they vanish on the dark model space (10% grey hatch lines did).
    let color_of = |rgb: [u8; 3]| -> Color {
        let (hi, lo) = (rgb.iter().copied().max().unwrap_or(0), rgb.iter().copied().min().unwrap_or(0));
        if hi <= 38 && hi - lo <= 8 { Color::Index(7) } else { Color::True(Rgb(rgb[0], rgb[1], rgb[2])) }
    };
    let pen_name = |rgb: [u8; 3]| -> String {
        if rgb[0] == rgb[1] && rgb[1] == rgb[2] {
            match rgb[0] {
                0 => "BLACK".into(),
                255 => "WHITE".into(),
                g => format!("GRAY{}", (u32::from(g) * 100 + 127) / 255),
            }
        } else {
            match rgb {
                [255, 0, 0] => "RED".into(),
                [0, 255, 0] => "GREEN".into(),
                [0, 0, 255] => "BLUE".into(),
                [255, 255, 0] => "YELLOW".into(),
                [0, 255, 255] => "CYAN".into(),
                [255, 0, 255] => "MAGENTA".into(),
                [r, g, b] => format!("{r:02X}{g:02X}{b:02X}"),
            }
        }
    };
    let mut place = |kind: &str, pen: Pen, layer: &Option<String>| -> (String, Color, Lineweight) {
        let lw = Lineweight::Mm100(pen.lw);
        let color = color_of(pen.rgb);
        if layer.as_deref() == Some(MARKUP_LAYER) {
            layer_set.entry(MARKUP_LAYER.into()).or_insert((Color::Index(2), Lineweight::Default));
            return (MARKUP_LAYER.into(), color, if kind == "line" { lw } else { Lineweight::ByLayer });
        }
        if layer.as_deref() == Some(HIDDEN_TEXT_LAYER) {
            layer_set.entry(HIDDEN_TEXT_LAYER.into()).or_insert((Color::Index(8), Lineweight::Default));
            return (HIDDEN_TEXT_LAYER.into(), Color::ByLayer, Lineweight::ByLayer);
        }
        match mode {
            LayerMode::Pdf => {
                let name = layer.clone().unwrap_or_else(|| "0".into());
                layer_set.entry(name.clone()).or_insert((Color::Index(7), Lineweight::Default));
                (name, color, if kind == "line" { lw } else { Lineweight::ByLayer })
            }
            LayerMode::Object => {
                let name = match kind {
                    "fill" | "mask" => "PDF_Solid Fills",
                    "text" => "PDF_Text",
                    "image" => "PDF_Images",
                    _ => "PDF_Geometry",
                }
                .to_string();
                layer_set.entry(name.clone()).or_insert((Color::Index(7), Lineweight::Default));
                (name, color, if kind == "line" { lw } else { Lineweight::ByLayer })
            }
            LayerMode::Pen => {
                if kind == "mask" {
                    layer_set.entry("PDF-MASK".into()).or_insert((Color::Index(7), Lineweight::Default));
                    return ("PDF-MASK".into(), Color::ByLayer, Lineweight::ByLayer);
                }
                if kind == "image" {
                    layer_set.entry("PDF-IMAGES".into()).or_insert((Color::Index(7), Lineweight::Default));
                    return ("PDF-IMAGES".into(), Color::ByLayer, Lineweight::ByLayer);
                }
                let name = match kind {
                    "fill" => format!("PDF-FILL-{}", pen_name(pen.rgb)),
                    "text" => format!("PDF-TEXT-{}", pen_name(pen.rgb)),
                    _ => format!("PDF-{}-{:.2}", pen_name(pen.rgb), f64::from(pen.lw) / 100.0),
                };
                layer_set.entry(name.clone()).or_insert((color, if kind == "line" { lw } else { Lineweight::Default }));
                (name, Color::ByLayer, Lineweight::ByLayer)
            }
        }
    };
    let k = scale;
    let sp = |x: f64, y: f64| -> Vec2 {
        let (sx, sy) = to_sheet(x, y);
        Vec2::new(sx * k, sy * k)
    };
    let mut added = 0usize;
    let mut add = |d: &mut Drawing, layer: String, color: Color, lw: Lineweight, transp: u8, kind: EntityKind| {
        if added >= MAX_OBJECTS {
            return;
        }
        let transparency = if transp > 0 { Transparency::Percent(transp) } else { Transparency::ByLayer };
        let common = Common { layer, color, lineweight: lw, transparency, ..Common::default() };
        if d.add(&Space::Model, common, kind).is_ok() {
            added += 1;
        }
    };
    // everything in the PDF's painting order: a mask covers what was drawn before it, not after
    let mut paint: Vec<(usize, u8, usize)> = Vec::with_capacity(strokes.len() + fills.len() + masks.len() + texts.len());
    paint.extend(strokes.iter().enumerate().map(|(i, s)| (s.order, 0, i)));
    paint.extend(fills.iter().enumerate().map(|(i, f)| (f.order, 1, i)));
    paint.extend(masks.iter().enumerate().map(|(i, f)| (f.order, 2, i)));
    paint.extend(texts.iter().enumerate().map(|(i, t)| (t.order, 3, i)));
    paint.extend(images.iter().enumerate().map(|(i, im)| (im.order, 4, i)));
    paint.sort_unstable();
    // image files by their pixels (a logo on every sheet is one file)
    let mut image_files: HashMap<u64, String> = HashMap::new();
    let mut image_out: Vec<(String, std::sync::Arc<cadaddy_render::images::Raster>)> = Vec::new();
    for (_, kind, i) in paint {
        match kind {
            0 => {
                let Some(s) = strokes.get(i) else { continue };
                let (layer, color, lw) = place("line", s.pen, &s.layer);
                if opts.join
                    && let Some(((cx, cy), r)) = as_circle(s)
                {
                    let c = sp(cx, cy);
                    add(
                        &mut d,
                        layer,
                        color,
                        lw,
                        s.pen.transp,
                        EntityKind::Circle(Circle { center: Vec3::new(c.x, c.y, 0.0), radius: r / 72.0 * k }),
                    );
                    *stats.entry("circles".into()).or_default() += 1;
                    continue;
                }
                let vertices: Vec<PolyVertex> =
                    s.pts.iter().map(|(x, y)| PolyVertex { p: sp(*x, *y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
                if let ([va, vb], false) = (vertices.as_slice(), s.closed) {
                    let (a, b) = (va.p, vb.p);
                    add(&mut d, layer, color, lw, s.pen.transp, EntityKind::Line(Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) }));
                    *stats.entry("lines".into()).or_default() += 1;
                } else {
                    add(
                        &mut d,
                        layer,
                        color,
                        lw,
                        s.pen.transp,
                        EntityKind::LwPolyline(LwPolyline { vertices, closed: s.closed, const_width: 0.0, elevation: 0.0, plinegen: false }),
                    );
                    *stats.entry("polylines".into()).or_default() += 1;
                }
            }
            1 => {
                let Some(f) = fills.get(i) else { continue };
                let (layer, color, lw) = place("fill", f.pen, &f.layer);
                let loops: Vec<HatchLoop> = f
                    .loops
                    .iter()
                    .filter(|l| l.len() >= 3)
                    .enumerate()
                    .map(|(i, l)| HatchLoop {
                        curves: Vec::new(),
                        vertices: l.iter().map(|(x, y)| PolyVertex { p: sp(*x, *y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect(),
                        outer: i == 0,
                        sources: Vec::new(),
                    })
                    .collect();
                if loops.is_empty() {
                    continue;
                }
                let hatch = Hatch {
                    pattern: "SOLID".into(),
                    solid: true,
                    loops,
                    scale: 1.0,
                    angle: 0.0,
                    associative: false,
                    style: 0,
                    elevation: 0.0,
                    gradient: None,
                    origin: Vec2::ZERO,
                    background: None,
                    pattern_lines: Vec::new(),
                };
                add(&mut d, layer, color, lw, f.pen.transp, EntityKind::Hatch(hatch));
                *stats.entry("solid fills".into()).or_default() += 1;
            }
            2 => {
                let Some(f) = masks.get(i) else { continue };
                let (layer, _, _) = place("mask", f.pen, &f.layer);
                for l in f.loops.iter().filter(|l| l.len() >= 3) {
                    let boundary = l.iter().map(|(x, y)| sp(*x, *y)).collect();
                    add(&mut d, layer.clone(), Color::ByLayer, Lineweight::ByLayer, 0, EntityKind::Wipeout(Wipeout { boundary }));
                    *stats.entry("masks (wipeouts)".into()).or_default() += 1;
                }
            }
            4 => {
                let Some(im) = images.get(i) else { continue };
                let (w, h) = (f64::from(im.raster.width), f64::from(im.raster.height));
                let at = |x: f64, y: f64| {
                    let (px, py) = im.ctm.apply(x, y);
                    sp(px, py)
                };
                let p00 = at(0.0, 0.0);
                let (u, v) = ((at(1.0, 0.0) - p00) / w, (at(0.0, 1.0) - p00) / h);
                if !(u.is_finite() && v.is_finite()) || u.len() < 1e-12 || v.len() < 1e-12 {
                    continue;
                }
                let (clip, clipping) = image_clip(im, &sp, p00, u, v, h);
                // a picture of nothing but white paper (Revit tiles whole sheets with them) is a
                // mask like a white fill, or it shows as a white sheet behind white lines on screen
                if !clipping && blank_paper(&im.raster.rgba) {
                    let (layer, _, _) = place("mask", Pen { rgb: [255, 255, 255], lw: 0, transp: 0 }, &im.layer);
                    let boundary = vec![p00, at(1.0, 0.0), at(1.0, 1.0), at(0.0, 1.0)];
                    add(&mut d, layer, Color::ByLayer, Lineweight::ByLayer, 0, EntityKind::Wipeout(Wipeout { boundary }));
                    *stats.entry("masks (wipeouts)".into()).or_default() += 1;
                    continue;
                }
                let key = {
                    use std::hash::{Hash, Hasher};
                    let mut hs = std::collections::hash_map::DefaultHasher::new();
                    (im.raster.width, im.raster.height, &im.raster.rgba).hash(&mut hs);
                    hs.finish()
                };
                let path = match image_files.get(&key) {
                    Some(p) => p.clone(),
                    None => {
                        let file = format!("{}-{}.png", opts.image_prefix, image_files.len() + 1);
                        let path = match &opts.image_folder {
                            Some(f) => std::path::Path::new(f).join(&file).to_string_lossy().to_string(),
                            None => format!("pdf-image:{file}"),
                        };
                        image_files.insert(key, path.clone());
                        image_out.push((path.clone(), std::sync::Arc::clone(&im.raster)));
                        path
                    }
                };
                let transparent = im.raster.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
                let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let image = Image {
                    insert: Vec3::new(p00.x, p00.y, 0.0),
                    u: Vec3::new(u.x, u.y, 0.0),
                    v: Vec3::new(v.x, v.y, 0.0),
                    size: Vec2::new(w, h),
                    path,
                    name,
                    clip,
                    clipping,
                    display: 1 | 2 | if clipping { 4 } else { 0 } | if transparent { 8 } else { 0 },
                    ..Image::default()
                };
                let (layer, color, lw) = place("image", Pen { rgb: [0, 0, 0], lw: 0, transp: im.transp }, &im.layer);
                add(&mut d, layer, color, lw, im.transp, EntityKind::Image(image));
                *stats.entry("images".into()).or_default() += 1;
            }
            _ => {
                let Some(t) = texts.get(i) else { continue };
                let (layer, color, lw) = place("text", t.pen, &t.layer);
                let p = sp(t.at.0, t.at.1);
                let text = Text {
                    insert: Vec3::new(p.x, p.y, 0.0),
                    align_pt: None,
                    height: t.height / 72.0 * k,
                    value: t.value.clone(),
                    rotation: (t.rotation + rot_deg).to_radians(),
                    width_factor: t.width_factor,
                    oblique: 0.0,
                    style: match &t.style {
                        Some((name, file)) => {
                            if !d.text_styles.iter().any(|st| st.name.eq_ignore_ascii_case(name)) {
                                d.text_styles.push(TextStyle { name: name.clone(), font: file.clone(), ..TextStyle::default() });
                            }
                            name.clone()
                        }
                        None => "Standard".into(),
                    },
                    halign: HAlign::default(),
                    valign: VAlign::default(),
                };
                add(&mut d, layer, color, lw, t.pen.transp, EntityKind::Text(text));
                *stats.entry("texts".into()).or_default() += 1;
            }
        }
    }
    // Adding an object on a new layer created it plain: give each its pen.
    for (name, (color, lw)) in &layer_set {
        if name == "0" {
            continue;
        }
        match d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) {
            Some(l) => {
                l.color = *color;
                l.lineweight = *lw;
            }
            None => d.layers.push(Layer { name: name.clone(), color: *color, lineweight: *lw, ..Layer::default() }),
        }
    }
    if let Some(l) = d.layers.iter_mut().find(|l| l.name == HIDDEN_TEXT_LAYER) {
        l.on = false;
    }
    if added >= MAX_OBJECTS {
        notes.push(format!("stopped at {MAX_OBJECTS} objects"));
    }
    if !image_out.is_empty() {
        stats.insert("image files".into(), image_out.len());
    }
    for (why, n) in &out.image_failures {
        notes.push(format!("{n} raster image(s) not converted ({why})"));
    }
    if markups > 0 {
        stats.insert("markups".into(), markups);
    }
    if out.clipped_ops > 0 {
        stats.insert("clip-shaped fills".into(), out.clipped_ops);
    }
    if texts.is_empty() && strokes.len() > 1000 {
        notes.push("no text in the PDF: its lettering was plotted as linework (SHX fonts) and stays linework".into());
    }

    // the sheet on a layout, through one viewport at the scale
    if opts.layout && (k - 1.0).abs() > 1e-9 {
        let name = sheet_number(&texts, &to_sheet, sheet).unwrap_or_else(|| "Sheet".into());
        let mut lay = Layout::new(&name, 1);
        lay.page.paper = format!("PDF sheet ({:.2} x {:.2} Inches)", sheet.0, sheet.1);
        lay.page.width_mm = sheet.0.min(sheet.1) * 25.4;
        lay.page.height_mm = sheet.0.max(sheet.1) * 25.4;
        lay.page.landscape = sheet.0 > sheet.1;
        lay.page.margins_mm = [0.0; 4];
        // the sheet's layout replaces the empty default ones
        d.layouts.retain(|l| !l.entities.is_empty() && !l.name.eq_ignore_ascii_case(&name));
        let order = d.layouts.len() as u32 + 1;
        lay.tab_order = order;
        d.layouts.insert(0, lay);
        let vp = Viewport {
            center: Vec3::new(sheet.0 / 2.0, sheet.1 / 2.0, 0.0),
            width: sheet.0,
            height: sheet.1,
            view_center: Vec2::new(sheet.0 / 2.0 * k, sheet.1 / 2.0 * k),
            view_height: sheet.1 * k,
            id: 2,
            locked: true,
            ..Viewport::default()
        };
        if d.add(&Space::Paper(name.clone()), Common { layer: "0".into(), ..Common::default() }, EntityKind::Viewport(vp)).is_ok() {
            stats.insert("layout".into(), 1);
        }
        notes.push(format!("layout \"{name}\" shows model space at 1:{k} on the {:.0} x {:.0} in sheet", sheet.0, sheet.1));
    }
    Ok(PdfImport { drawing: d, sheet, scale: k, scale_source, pages: page_total, stats, notes, images: image_out })
}

/// Images painted one after another in the same orientation that touch, each small (the rows
/// of a run-length-coded logo, tiles of a picture): stitched into one image at the finest
/// resolution among them. Only white fills (the white background some writers paint under each
/// piece) may come between two pieces; the stitched image is painted where the last piece was,
/// over those. Returns the images and how many pieces were stitched away.
fn stitch(images: Vec<ImageOut>, white_fills: &HashSet<usize>) -> (Vec<ImageOut>, usize) {
    let unit = |x: f64, y: f64| {
        let l = x.hypot(y);
        (l > 1e-9).then(|| (x / l, y / l, l))
    };
    // the image's frame: unit vectors along its width (U) and height (V) and their lengths
    let frame = |m: &M| -> Option<((f64, f64), (f64, f64), f64, f64)> {
        let [a, b, c, d, _, _] = m.0;
        let (ux, uy, lu) = unit(a, b)?;
        let (vx, vy, lv) = unit(c, d)?;
        // orthogonal frames only (no skew)
        ((ux * vx + uy * vy).abs() < 1e-6).then_some(((ux, uy), (vx, vy), lu, lv))
    };
    let small = |r: &cadaddy_render::images::Raster| r.width.min(r.height) <= 4 || u64::from(r.width) * u64::from(r.height) <= 4096;
    let mut out: Vec<ImageOut> = Vec::with_capacity(images.len());
    let mut run: Vec<ImageOut> = Vec::new();
    // the run's sheet-point box
    let mut run_box = [0.0f64; 4];
    let mut stitched = 0usize;
    let flush = |run: &mut Vec<ImageOut>, out: &mut Vec<ImageOut>, stitched: &mut usize| {
        if run.len() < 2 {
            out.append(run);
            return;
        }
        match stitch_run(run, &frame) {
            Some(im) => {
                *stitched += run.len() - 1;
                run.clear();
                out.push(im);
            }
            None => out.append(run),
        }
    };
    for im in images {
        let joins = run.last().is_some_and(|prev| {
            let (Some((pu, pv, _, _)), Some((u, v, _, _))) = (frame(&prev.ctm), frame(&im.ctm)) else { return false };
            let same = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6;
            // within the run's box, padded (a logo's runs are separated by its white gaps)
            let near = {
                let (b0, b1) = (run_box, image_box(&im.ctm));
                let pad = (0.25 * (b0[2] - b0[0]).max(b0[3] - b0[1])).max(10.0);
                b1[0] <= b0[2] + pad && b1[2] >= b0[0] - pad && b1[1] <= b0[3] + pad && b1[3] >= b0[1] - pad
            };
            let between = (prev.order + 1..im.order).all(|o| white_fills.contains(&o));
            im.order > prev.order
                && between
                && same(pu, u)
                && same(pv, v)
                && small(&im.raster)
                && small(&prev.raster)
                && near
                && prev.layer == im.layer
                && prev.transp == im.transp
        });
        let b = image_box(&im.ctm);
        if joins {
            run_box = [run_box[0].min(b[0]), run_box[1].min(b[1]), run_box[2].max(b[2]), run_box[3].max(b[3])];
        } else {
            flush(&mut run, &mut out, &mut stitched);
            run_box = b;
        }
        run.push(im);
    }
    flush(&mut run, &mut out, &mut stitched);
    (out, stitched)
}

/// The sheet-point box of an image's unit square.
fn image_box(m: &M) -> [f64; 4] {
    let pts = [m.apply(0.0, 0.0), m.apply(1.0, 0.0), m.apply(0.0, 1.0), m.apply(1.0, 1.0)];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in pts {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    [x0, y0, x1, y1]
}

/// An image's clip boundary in pixel coordinates (corner at -0.5, -0.5, y down), when the
/// clipping path in force cuts it: the path's outline when it is one loop, else its box.
fn image_clip(im: &ImageOut, sp: &dyn Fn(f64, f64) -> Vec2, p00: Vec2, u: Vec2, v: Vec2, h: f64) -> (Vec<Vec2>, bool) {
    let Some(loops) = &im.clip else { return (Vec::new(), false) };
    let Some([cx0, cy0, cx1, cy1]) = bbox(loops) else { return (Vec::new(), false) };
    let [ix0, iy0, ix1, iy1] = image_box(&im.ctm);
    let tol = 0.5;
    if cx0 <= ix0 + tol && cy0 <= iy0 + tol && cx1 >= ix1 - tol && cy1 >= iy1 - tol {
        return (Vec::new(), false);
    }
    // a model point in pixel coordinates: solve q - p00 = u·s + v·t
    let det = u.x * v.y - u.y * v.x;
    if det.abs() < 1e-300 {
        return (Vec::new(), false);
    }
    let to_px = |x: f64, y: f64| {
        let q = sp(x, y) - p00;
        let s = (q.x * v.y - q.y * v.x) / det;
        let t = (u.x * q.y - u.y * q.x) / det;
        Vec2::new(s - 0.5, h - t - 0.5)
    };
    let pts: Vec<Vec2> = match loops.as_slice() {
        [one] if (3..=4096).contains(&one.len()) => one.iter().map(|(x, y)| to_px(*x, *y)).collect(),
        _ => [(cx0, cy0), (cx1, cy0), (cx1, cy1), (cx0, cy1)].iter().map(|(x, y)| to_px(*x, *y)).collect(),
    };
    (pts, true)
}

/// Every pixel opaque and white (to JPEG noise): the image only paints paper.
fn blank_paper(rgba: &[u8]) -> bool {
    !rgba.is_empty() && rgba.as_chunks::<4>().0.iter().all(|p| p.iter().all(|&c| c >= 250))
}

/// One image painted from a run of images sharing a frame (see [`stitch`]).
#[allow(clippy::type_complexity)]
fn stitch_run(run: &[ImageOut], frame: &dyn Fn(&M) -> Option<((f64, f64), (f64, f64), f64, f64)>) -> Option<ImageOut> {
    let first = run.first()?;
    let (u, v, _, _) = frame(&first.ctm)?;
    let dot = |p: (f64, f64), q: (f64, f64)| p.0 * q.0 + p.1 * q.1;
    // each piece in frame coordinates: (u0, v0, length along U, along V)
    let mut parts = Vec::with_capacity(run.len());
    let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let (mut ru, mut rv) = (0.0f64, 0.0f64);
    for im in run {
        let (_, _, lu, lv) = frame(&im.ctm)?;
        let o = (im.ctm.0[4], im.ctm.0[5]);
        let (pu, pv) = (dot(o, u), dot(o, v));
        parts.push((pu, pv, lu, lv));
        (u0, v0, u1, v1) = (u0.min(pu), v0.min(pv), u1.max(pu + lu), v1.max(pv + lv));
        ru = ru.max(f64::from(im.raster.width) / lu);
        rv = rv.max(f64::from(im.raster.height) / lv);
    }
    let (mut cw, mut ch) = (((u1 - u0) * ru).ceil(), ((v1 - v0) * rv).ceil());
    const MAX_SIDE: f64 = 4096.0;
    if cw > MAX_SIDE || ch > MAX_SIDE {
        let k = MAX_SIDE / cw.max(ch);
        (cw, ch) = ((cw * k).ceil(), (ch * k).ceil());
    }
    if !(cw >= 1.0 && ch >= 1.0) {
        return None;
    }
    let (cw, ch) = (cw as usize, ch as usize);
    let mut rgba = vec![0u8; cw * ch * 4];
    for (im, (pu, pv, lu, lv)) in run.iter().zip(parts) {
        let (w, h) = (im.raster.width as usize, im.raster.height as usize);
        // canvas pixels whose centre falls inside this piece
        let cx0 = (((pu - u0) / (u1 - u0)) * cw as f64).floor().max(0.0) as usize;
        let cx1 = ((((pu + lu - u0) / (u1 - u0)) * cw as f64).ceil() as usize).min(cw);
        // canvas row 0 is the far end of V (the top of the image)
        let cy0 = (((v1 - pv - lv) / (v1 - v0)) * ch as f64).floor().max(0.0) as usize;
        let cy1 = ((((v1 - pv) / (v1 - v0)) * ch as f64).ceil() as usize).min(ch);
        for cy in cy0..cy1 {
            let vv = v1 - (cy as f64 + 0.5) / ch as f64 * (v1 - v0);
            let t = (vv - pv) / lv;
            if !(0.0..1.0).contains(&t) {
                continue;
            }
            let sy = (((1.0 - t) * h as f64) as usize).min(h - 1);
            for cx in cx0..cx1 {
                let uu = u0 + (cx as f64 + 0.5) / cw as f64 * (u1 - u0);
                let sx_f = (uu - pu) / lu;
                if !(0.0..1.0).contains(&sx_f) {
                    continue;
                }
                let sx = ((sx_f * w as f64) as usize).min(w - 1);
                let Some(px) = im.raster.pixel(sx as u32, sy as u32) else { continue };
                if px[3] == 0 {
                    continue;
                }
                let i = (cy * cw + cx) * 4;
                if let Some(dst) = rgba.get_mut(i..i + 4) {
                    // source over what is there
                    let a = u32::from(px[3]);
                    for k in 0..3 {
                        dst[k] = ((u32::from(px[k]) * a + u32::from(dst[k]) * (255 - a)) / 255) as u8;
                    }
                    dst[3] = (a + u32::from(dst[3]) * (255 - a) / 255) as u8;
                }
            }
        }
    }
    let raster = cadaddy_render::images::Raster::new(cw as u32, ch as u32, rgba)?;
    let origin = (u.0 * u0 + v.0 * v0, u.1 * u0 + v.1 * v0);
    let ctm = M([u.0 * (u1 - u0), u.1 * (u1 - u0), v.0 * (v1 - v0), v.1 * (v1 - v0), origin.0, origin.1]);
    let order = run.last().map_or(first.order, |l| l.order);
    Some(ImageOut { ctm, raster: std::sync::Arc::new(raster), layer: first.layer.clone(), clip: first.clip.clone(), transp: first.transp, order })
}

/// The sheet number from the title block: the largest short text near the right or bottom edge
/// that looks like one (E-4, A-101, M2.1).
fn sheet_number(texts: &[TextOut], to_sheet: &dyn Fn(f64, f64) -> (f64, f64), sheet: (f64, f64)) -> Option<String> {
    texts
        .iter()
        .filter(|t| {
            let v = t.value.trim();
            let (x, y) = to_sheet(t.at.0, t.at.1);
            v.len() <= 8
                && v.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && v.chars().any(|c| c.is_ascii_digit())
                && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
                && (x > sheet.0 * 0.8 || y < sheet.1 * 0.2)
        })
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .map(|t| t.value.trim().to_string())
}

fn decode_pdf_string(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> =
            b[2..].chunks(2).map(|c| u16::from_be_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)])).collect();
        String::from_utf16_lossy(&units)
    } else {
        b.iter().map(|c| win_ansi(u32::from(*c))).collect()
    }
}

/// Write each image as a PNG file at its path (in `folder`, created when missing).
#[cfg(not(target_arch = "wasm32"))]
fn write_images(folder: &str, images: &[(String, std::sync::Arc<cadaddy_render::images::Raster>)]) -> std::io::Result<usize> {
    std::fs::create_dir_all(folder)?;
    let mut n = 0;
    for (path, raster) in images {
        let png = raster.encode_png().ok_or_else(|| std::io::Error::other("PNG encoding failed"))?;
        std::fs::write(path, png)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(target_arch = "wasm32")]
fn write_images(_folder: &str, _images: &[(String, std::sync::Arc<cadaddy_render::images::Raster>)]) -> std::io::Result<usize> {
    Err(std::io::Error::other("files can't be written on the web"))
}

/// PDFIMPORT's JSON options → the converted page and a report: `{page?, scale?: number | "auto",
/// layers?: "pdf" | "pen" | "object", whiteFills?, text?, join?, layout?, markups?, images?,
/// imageFolder?, imagePrefix?}`. Images are written as PNG files in `imageFolder` (created
/// when missing) and their pixels handed to the image cache, so they show at once.
pub fn read_json(bytes: &[u8], p: &serde_json::Value) -> std::result::Result<(Drawing, serde_json::Value), String> {
    use serde_json::{Value, json};
    let mut o = PdfOptions::default();
    if let Some(n) = p.get("page").and_then(Value::as_u64) {
        o.page = u32::try_from(n.max(1)).unwrap_or(1);
    }
    o.scale = match p.get("scale") {
        Some(Value::Number(n)) => n.as_f64().filter(|k| k.is_finite() && *k > 0.0),
        Some(Value::String(s)) if !s.eq_ignore_ascii_case("auto") => Some(
            s.trim()
                .parse::<f64>()
                .ok()
                .or_else(|| parse_scale(s))
                .ok_or_else(|| format!("`scale` \"{s}\" isn't a number or a scale like 1/8\" = 1'-0\""))?,
        ),
        _ => None,
    };
    o.layers = match p.get("layers").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        Some("pen") => LayerMode::Pen,
        Some("object") => LayerMode::Object,
        _ => LayerMode::Pdf,
    };
    let flag = |k: &str, d: bool| p.get(k).and_then(Value::as_bool).unwrap_or(d);
    o.white_fills = flag("whiteFills", false);
    o.masks = flag("masks", true);
    o.text = flag("text", true);
    o.join = flag("join", true);
    o.layout = flag("layout", true);
    o.markups = flag("markups", true);
    o.images = flag("images", true);
    o.image_folder = p.get("imageFolder").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty()).map(str::to_string);
    if let Some(prefix) = p.get("imagePrefix").and_then(Value::as_str) {
        let clean: String = prefix.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
        if !clean.trim().is_empty() {
            o.image_prefix = clean.trim().to_string();
        }
    }
    let mut r = read(bytes, &o).map_err(|e| e.to_string())?;
    let mut written = 0usize;
    if let Some(folder) = &o.image_folder
        && !r.images.is_empty()
    {
        match write_images(folder, &r.images) {
            Ok(n) => written = n,
            Err(e) => r.notes.push(format!("the image files couldn't be written to {folder}: {e}; the images show in this session only")),
        }
    }
    for (path, raster) in &r.images {
        cadaddy_render::images::insert(path, std::sync::Arc::clone(raster));
    }
    let report = json!({
        "pages": r.pages,
        "page": o.page,
        "sheet": [r.sheet.0, r.sheet.1],
        "scale": r.scale,
        "scaleSource": r.scale_source,
        "stats": r.stats,
        "notes": r.notes,
        "layers": r.drawing.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
        "imageFiles": written,
        "imageFolder": o.image_folder,
    });
    Ok((r.drawing, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_notes() {
        assert_eq!(parse_scale("SCALE: 1/8\" = 1'-0\""), Some(96.0));
        assert_eq!(parse_scale("1/4\"=1'-0\""), Some(48.0));
        assert_eq!(parse_scale("3/16\" = 1'-0\""), Some(64.0));
        assert_eq!(parse_scale("1 1/2\" = 1'-0\""), Some(8.0));
        assert_eq!(parse_scale("1\" = 20'"), Some(240.0));
        assert_eq!(parse_scale("1:100"), Some(100.0));
        assert_eq!(parse_scale("NOT TO SCALE"), None);
    }

    #[test]
    fn rectangular_clips_cut_lines_and_fills() {
        let r = [0.0, 0.0, 10.0, 10.0];
        let pieces = clip_polyline(&[(-5.0, 5.0), (5.0, 5.0), (5.0, 15.0)], r);
        assert_eq!(pieces, vec![vec![(0.0, 5.0), (5.0, 5.0), (5.0, 10.0)]]);
        assert!(clip_polyline(&[(20.0, 20.0), (30.0, 30.0)], r).is_empty());
        let poly = clip_polygon(&[(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)], r);
        assert_eq!(bbox(&[poly]), Some([0.0, 0.0, 5.0, 5.0]));
        assert_eq!(clip_rect(&[vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]]), Some(r));
        assert_eq!(clip_rect(&[vec![(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)]]), None);
    }

    #[test]
    fn letters_placed_one_by_one_join_into_words() {
        let pen = Pen { rgb: [0, 0, 0], lw: 0, transp: 0 };
        let t = |x: f64, v: &str| TextOut {
            at: (x, 0.0),
            end: (x + 6.0, 0.0),
            height: 7.16,
            rotation: 0.0,
            width_factor: 1.0,
            value: v.into(),
            pen,
            layer: None,
            style: None,
            order: 0,
        };
        // "Ti" + "lt" touching, then a space-wide gap, then "=0"; a far one stays apart
        let m = merge_texts(vec![t(0.0, "T"), t(6.0, "i"), t(12.0, "lt"), t(21.0, "=0"), t(200.0, "X")]);
        let v: Vec<&str> = m.iter().map(|t| t.value.as_str()).collect();
        assert_eq!(v, ["Tilt =0", "X"]);
    }

    #[test]
    fn lineweights_snap_to_standard_values() {
        assert_eq!(lineweight(0.0), 0);
        assert_eq!(lineweight(0.24), 9);
        assert_eq!(lineweight(1.2), 40);
    }

    #[test]
    fn touching_pieces_join_and_round_loops_become_circles() {
        let pen = Pen { rgb: [0, 0, 0], lw: 25, transp: 0 };
        // a circle drawn as 24 separate chords
        let pts: Vec<(f64, f64)> =
            (0..=24).map(|i| (f64::from(i) * std::f64::consts::TAU / 24.0).sin_cos()).map(|(s, c)| (10.0 + 5.0 * c, 10.0 + 5.0 * s)).collect();
        let strokes: Vec<Stroke> =
            pts.windows(2).map(|w| Stroke { pts: w.to_vec(), closed: false, pen, layer: None, curve_loop: false, order: 0 }).collect();
        // a faceplate: two arcs of one circle and two short sides is not a circle
        let lens: Vec<(f64, f64)> = (0..=8)
            .map(|i| (f64::from(i) / 8.0 * 1.2 - 0.6 + std::f64::consts::FRAC_PI_2).sin_cos())
            .map(|(s, c)| (5.0 * c, 5.0 * s))
            .chain((0..=8).map(|i| (f64::from(i) / 8.0 * 1.2 - 0.6 - std::f64::consts::FRAC_PI_2).sin_cos()).map(|(s, c)| (5.0 * c, 5.0 * s)))
            .collect();
        assert!(as_circle(&Stroke { pts: lens, closed: true, pen, layer: None, curve_loop: false, order: 0 }).is_none());
        let joined = join(strokes);
        assert_eq!(joined.len(), 1);
        let c = joined.first().and_then(as_circle);
        assert!(c.is_some_and(|((x, y), r)| (x - 10.0).abs() < 0.01 && (y - 10.0).abs() < 0.01 && (r - 5.0).abs() < 0.05), "{c:?}");
    }

    /// A hand-written one-page PDF: a stroked line, a rectangle, a white mask, text and a scale note.
    fn sample_pdf() -> Vec<u8> {
        pdf_with(b"q 0 0 0 RG 1.2 w 72 72 m 144 72 l S 0.5 0.5 0.5 RG 72 100 72 36 re S 0.1 0.1 0.1 RG 72 400 m 144 400 l S 1 1 1 rg 200 200 20 20 re f BT /F1 12 Tf 72 300 Td (PANEL A) Tj ET BT 3 Tr /F1 12 Tf 72 500 Td (SEARCH ONLY) Tj ET BT /F1 8 Tf 72 280 Td (SCALE: 1/4\" = 1'-0\") Tj ET Q")
    }

    /// A one-page 11×8.5 PDF around a content stream.
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

    #[test]
    fn reads_lines_text_scale_and_layout() {
        let r = read(&sample_pdf(), &PdfOptions::default());
        let r = match r {
            Ok(r) => r,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(r.scale, 48.0, "{}", r.scale_source);
        // What the user adds next is sized for the sheet's scale.
        let h = &r.drawing.header;
        assert_eq!((h.f64("DIMSCALE", 1.0), h.f64("TEXTSIZE", 0.0)), (48.0, 4.5));
        assert!(h.str("CANNOSCALE", "").starts_with("1/4\""), "{}", h.str("CANNOSCALE", ""));
        assert_eq!(r.sheet, (11.0, 8.5));
        let ents: Vec<_> = r.drawing.model.iter().collect();
        // the black line: 1"..2" at 1" up on the sheet, × 48
        assert!(ents.iter().any(
            |e| matches!(&e.kind, EntityKind::Line(l) if (l.a.x - 48.0).abs() < 1e-6 && (l.b.x - 96.0).abs() < 1e-6 && (l.a.y - 48.0).abs() < 1e-6)
        ));
        assert!(ents.iter().any(|e| matches!(&e.kind, EntityKind::LwPolyline(p) if p.closed && p.vertices.len() == 4)));
        assert!(ents.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "PANEL A" && t.style == "PDF-Arial")));
        assert!(r.drawing.text_styles.iter().any(|st| st.name == "PDF-Arial" && st.font == "arial.ttf"));
        // invisible text is kept, on its own layer that is off
        assert!(
            r.drawing.model.iter().any(|e| e.common.layer == HIDDEN_TEXT_LAYER && matches!(&e.kind, EntityKind::Text(t) if t.value == "SEARCH ONLY"))
        );
        assert!(r.drawing.layers.iter().any(|l| l.name == HIDDEN_TEXT_LAYER && !l.on));
        // condensed and other families keep their own files, so the text keeps its width
        assert_eq!(ttf_style("KLEEBE+ArialNarrow,Bold"), Some(("PDF-ArialNarrow-Bold".into(), "arialnb.ttf".into())));
        assert_eq!(ttf_style("ArialNarrow"), Some(("PDF-ArialNarrow".into(), "arialn.ttf".into())));
        assert_eq!(ttf_style("Swiss721BT-Bold"), Some(("PDF-Arial-Bold".into(), "arialbd.ttf".into())));
        assert_eq!(ttf_style("TKHEYN+TwCenMT"), Some(("PDF-TwCenMT".into(), "tcm_____.ttf".into())));
        assert_eq!(ttf_style("BradleyHandITC"), Some(("PDF-BradleyHandITC".into(), "bradhitc.ttf".into())));
        assert!(!ents.iter().any(|e| matches!(e.kind, EntityKind::Hatch(_))), "the white mask is no solid fill");
        assert!(ents.iter().any(|e| matches!(&e.kind, EntityKind::Wipeout(w) if w.boundary.len() == 4)), "it is a wipeout");
        // black lines on a pen layer shown in colour 7
        assert!(r.drawing.layers.iter().any(|l| l.name == "PDF-BLACK-0.40" && l.color == Color::Index(7)));
        // near-black greys too, or they vanish on the dark screen
        assert!(r.drawing.layers.iter().any(|l| l.name.starts_with("PDF-GRAY10") && l.color == Color::Index(7)), "{:?}", r.drawing.layers);
        assert!(
            r.drawing.layers.iter().any(|l| l.name.starts_with("PDF-GRAY50") && l.color == Color::True(Rgb(128, 128, 128))),
            "{:?}",
            r.drawing.layers
        );
        assert!(r.drawing.layouts.first().is_some_and(|l| l.entities.iter().any(|e| matches!(e.kind, EntityKind::Viewport(_)))));
    }

    #[test]
    fn white_paper_images_become_wipeouts() {
        // two 2×2 RGB images: one near-white (JPEG-white 254), one with a black pixel
        let content = b"q 100 0 0 100 300 300 cm BI /W 2 /H 2 /CS /RGB /BPC 8 /F /AHx ID fefefefefefefefefefefefe> EI Q \
            q 100 0 0 100 500 300 cm BI /W 2 /H 2 /CS /RGB /BPC 8 /F /AHx ID 000000fefefefefefefefefe> EI Q";
        let r = match read(&pdf_with(content), &PdfOptions::default()) {
            Ok(r) => r,
            Err(e) => panic!("{e}"),
        };
        let images = r.drawing.model.iter().filter(|e| matches!(e.kind, EntityKind::Image(_))).count();
        let masks = r.drawing.model.iter().filter(|e| matches!(&e.kind, EntityKind::Wipeout(w) if w.boundary.len() == 4)).count();
        assert_eq!((images, masks), (1, 1));
        assert!(blank_paper(&[254, 254, 254, 255]) && !blank_paper(&[254, 254, 254, 0]) && !blank_paper(&[]));
    }

    #[test]
    fn hostile_input_is_an_error_not_a_crash() {
        assert!(read(b"%PDF-1.4 garbage", &PdfOptions::default()).is_err());
        assert!(read(&sample_pdf(), &PdfOptions { page: 7, ..PdfOptions::default() }).is_err());
    }
}
