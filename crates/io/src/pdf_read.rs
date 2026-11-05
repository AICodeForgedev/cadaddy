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
    /// Content-class layers: PDF-GRID, PDF-WALL, PDF-TEXT, PDF-SYMBOLS, PDF-TITLEBLOCK,
    /// PDF-HATCH, PDF-GEOMETRY — assigned by detecting what each element is.
    Content,
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
    /// Recover SHX-stroked text and detect repeated symbol clusters (on by default).  Turn off
    /// for downstream geometry analysis (walls, conduits, grids) that needs every stroke intact.
    pub recover_text: bool,
    /// Draw an E sheet that has a scale note at that scale (off by default: E sheets are
    /// DO NOT SCALE and stay sheet-size).  Either way the import reports `do_not_scale` and
    /// `needs_registration`.  For a sheet opened on its own, with no architectural plan to
    /// register it to, so walls and rooms still read at building size.
    pub e_sheet_at_note_scale: bool,
    /// Fills smaller than [`SMALL_FILL_IN`] on the sheet (pattern dots and dashes, lettering
    /// painted as fills) become one solid hatch per layer and colour, not one hatch each: the
    /// sheet looks the same with far fewer objects (off by default; on for print-set jobs).
    pub merge_small_fills: bool,
}

/// The largest side (sheet inches) of a fill [`PdfOptions::merge_small_fills`] merges: smaller
/// than a wall's poché is long, larger than a pattern's dots and dashes.
pub const SMALL_FILL_IN: f64 = 0.06;
/// Most outlines one merged fill holds.
const MAX_MERGED_LOOPS: usize = 4_000;

/// The layer annotations (markups) are drawn on.
pub const MARKUP_LAYER: &str = "PDF-MARKUP";

/// The layer invisible text (text rendering mode 3: the searchable copy of lettering drawn as
/// linework, as AutoCAD writes SHX text) goes on, turned off: the sheet shows the lettering
/// once, and its words can still be searched and listed.
pub const HIDDEN_TEXT_LAYER: &str = "PDF-HIDDEN-TEXT";

/// The text style of words the SHX stroke OCR read on a page without hidden text to confirm
/// them.  They are searchable text, but a guess: two ticks beside a stroke read as `8"`, a
/// corner and a dash as `EL`.  Readers that take text as evidence (duct sizes and heights) skip
/// this style.
pub const OCR_TEXT_STYLE: &str = "PDF-SHX-OCR";

/// The layer (off) that keeps the strokes recovered text was read from: the TEXT replaces them
/// on the sheet, and the plotted lettering is still there to check a reading against.
pub const SHX_SOURCE_LAYER: &str = "PDF-SHX-SOURCE";
/// Layer (off) of the OCR readings on a page with hidden text that no hidden string confirms and
/// that read like no word (a lone letter, mostly marks: a tick, a corner or a circle misread):
/// kept for review, not drawn as the sheet's lettering.
pub const OCR_REVIEW_LAYER: &str = "PDF-OCR-REVIEW";

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
            recover_text: true,
            e_sheet_at_note_scale: false,
            merge_small_fills: false,
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
    /// Whether a "DO NOT SCALE" note was detected (visible or recovered from SHX text).
    pub do_not_scale: bool,
    /// Whether the sheet needs registration (it carries a scale note but is kept at sheet size
    /// because of a DO NOT SCALE note).
    pub needs_registration: bool,
    pub pages: u32,
    pub stats: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    /// The image files the IMAGE objects name (path as they name it) and their pixels.
    pub images: Vec<(String, std::sync::Arc<cadaddy_render::images::Raster>)>,
    /// What the stroke-lettering OCR read (empty when it didn't run).
    pub ocr: OcrReport,
}

/// The number of pages.
pub fn page_count(bytes: &[u8]) -> Result<u32> {
    let doc = load_cached(bytes)?;
    Ok(u32::try_from(doc.get_pages().len()).unwrap_or(u32::MAX))
}

fn load(bytes: &[u8]) -> Result<Document> {
    match Document::load_mem(bytes) {
        Ok(doc) => Ok(doc),
        Err(first_err) => {
            // Attempt xref rebuild: find `startxref` candidates by scanning backwards for
            // earlier xref offsets (incremental saves leave multiple). The last one may be
            // damaged while an earlier one is still valid.
            if let Some(repaired) = try_repair_xref(bytes) {
                return Ok(repaired);
            }
            Err(IoError::Format(format!("PDF: {first_err}")))
        }
    }
}

/// Scan `bytes` for `startxref` markers and try loading with each candidate offset (oldest
/// first), so a PDF with a damaged final xref section can still be read from an earlier save.
fn try_repair_xref(bytes: &[u8]) -> Option<Document> {
    // Collect all `startxref` positions (there may be several in an incrementally saved PDF).
    let needle = b"startxref";
    let mut positions = Vec::new();
    for i in 0..bytes.len().saturating_sub(needle.len()) {
        if bytes.get(i..i + needle.len()) == Some(needle) {
            positions.push(i);
        }
    }
    if positions.len() < 2 {
        // Only one (or zero) startxref — nothing earlier to try.
        return None;
    }
    // Try each startxref from the oldest: build patched bytes where %%EOF points at that xref.
    for &pos in positions.iter().rev().skip(1) {
        // Read the offset number after this startxref marker.
        let after = pos + needle.len();
        let tail = std::str::from_utf8(bytes.get(after..bytes.len().min(after + 30))?).ok()?;
        let offset_str = tail.trim().lines().next()?.trim();
        if offset_str.parse::<usize>().is_err() {
            continue;
        }
        // Build a truncated copy up to (and including) this %%EOF.
        if let Some(eof_pos) = bytes.get(after..).and_then(|b| {
            let eof = b"%%EOF";
            (0..b.len().saturating_sub(eof.len())).find(|&i| b.get(i..i + eof.len()) == Some(eof))
        }) {
            let end = after + eof_pos + b"%%EOF".len();
            if let Ok(doc) = Document::load_mem(bytes.get(..end)?) {
                return Some(doc);
            }
        }
    }
    None
}

/// Files at least this big are parsed once and kept for the next pages ([`load_cached`]).
const CACHE_MIN_BYTES: usize = 256 * 1024;
/// The parsed files kept: at most this many, and at most [`CACHE_MAX_BYTES`] of PDF between them.
const CACHE_MAX_DOCS: usize = 6;
const CACHE_MAX_BYTES: usize = 512 * 1024 * 1024;

type DocSlot = std::sync::Arc<std::sync::OnceLock<std::result::Result<std::sync::Arc<Document>, String>>>;

/// The last PDFs parsed, by content: `(fingerprint, length, parsed)`.
static DOC_CACHE: std::sync::Mutex<Vec<(u64, usize, DocSlot)>> = std::sync::Mutex::new(Vec::new());

/// A fingerprint of a file's bytes (which parsed file a page read can reuse).
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15 ^ bytes.len() as u64;
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let mut w = [0u8; 8];
        w.copy_from_slice(c);
        h = (h ^ u64::from_le_bytes(w)).wrapping_mul(0x0000_0100_0000_01B3).rotate_left(29);
    }
    for &b in chunks.remainder() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// The parsed document of `bytes`: a big file (a merged print set read page by page, by several
/// threads at once) is parsed once while it stays among the last few read.
fn load_cached(bytes: &[u8]) -> Result<std::sync::Arc<Document>> {
    if bytes.len() < CACHE_MIN_BYTES {
        return load(bytes).map(std::sync::Arc::new);
    }
    let key = fingerprint(bytes);
    let slot: DocSlot = {
        let Ok(mut cache) = DOC_CACHE.lock() else { return load(bytes).map(std::sync::Arc::new) };
        if let Some((_, _, slot)) = cache.iter().find(|(k, n, _)| *k == key && *n == bytes.len()) {
            std::sync::Arc::clone(slot)
        } else {
            let slot: DocSlot = std::sync::Arc::default();
            cache.push((key, bytes.len(), std::sync::Arc::clone(&slot)));
            while cache.len() > CACHE_MAX_DOCS || (cache.len() > 1 && cache.iter().map(|(_, n, _)| *n).sum::<usize>() > CACHE_MAX_BYTES) {
                cache.remove(0);
            }
            slot
        }
    };
    // Other threads wanting the same file wait for this parse instead of parsing it again.
    match slot.get_or_init(|| load(bytes).map(std::sync::Arc::new).map_err(|e| e.to_string())) {
        Ok(doc) => Ok(std::sync::Arc::clone(doc)),
        Err(e) => {
            if let Ok(mut cache) = DOC_CACHE.lock() {
                cache.retain(|(k, n, _)| !(*k == key && *n == bytes.len()));
            }
            Err(IoError::Format(e.clone()))
        }
    }
}

/// Forget the parsed files kept for the next pages (their memory is freed once no read uses them).
pub fn clear_cache() {
    if let Ok(mut cache) = DOC_CACHE.lock() {
        cache.clear();
    }
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
    /// Drawn with four or more curves that end where they start (closed or not): CAD geometry
    /// (a circle, a symbol's round outline); plot fonts flatten their arcs into lines.
    curved_loop: bool,
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
    // A simple font's codes are one byte each, whatever code space its ToUnicode CMap declares
    // (some writers declare <0000> <FFFF> and map single bytes): only a composite font's are two.
    if subtype != b"Type0" {
        font.two_byte = false;
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
                        self.out.strokes.push(Stroke {
                            pts: piece,
                            closed: false,
                            pen,
                            layer: layer.clone(),
                            curve_loop: false,
                            curved_loop: false,
                            order,
                        });
                    }
                    continue;
                }
                let ends_meet = match (pts.first(), pts.last()) {
                    (Some(a), Some(b)) => (a.0 - b.0).hypot(a.1 - b.1) < 0.01,
                    _ => false,
                };
                let curved_loop = curves >= 4 && (closed || ends_meet);
                self.out.strokes.push(Stroke { pts, closed, pen, layer: layer.clone(), curve_loop: closed && curves == 4, curved_loop, order });
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
                cur.curved_loop |= next.curved_loop;
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
// Fix 1: Enhanced scale detection — per-viewport/drawing-title scale notes
// ---------------------------------------------------------------------------------------------

/// Check whether the sheet carries a "DO NOT SCALE" / "NTS" / "refer to architectural" note.
/// Checks both visible and hidden text.
fn has_do_not_scale_note(texts: &[TextOut]) -> bool {
    for t in texts {
        let up = t.value.trim().to_ascii_uppercase();
        if up.contains("DO NOT SCALE")
            || up.contains("NOT TO SCALE")
            || up == "NTS"
            || up == "N.T.S."
            || (up.contains("REFER") && up.contains("ARCHITECTURAL") && (up.contains("SCALE") || up.contains("DIMENSION")))
            || up.contains("DO NOT SCALE THIS DRAWING")
        {
            return true;
        }
    }
    false
}

/// Detect scale notes in text entities, with preference for those on TITLE/BORDER layers or
/// near the bottom/right of the sheet (the title block area). Returns the best scale factor and
/// the source text.
fn detect_scale_note(texts: &[TextOut], sheet: (f64, f64)) -> Option<(f64, String)> {
    struct Candidate {
        scale: f64,
        text: String,
        count: usize,
        priority: u8, // 0 = best (title block layer), 1 = bottom-right, 2 = anywhere
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for t in texts {
        let v = t.value.trim();
        let up = v.to_ascii_uppercase();
        if !(up.contains('=') || up.starts_with("SCALE")) || !(up.contains('\'') || up.contains('"') || up.contains(':')) {
            continue;
        }
        let Some(k) = parse_scale(v) else { continue };
        let key = v.trim_start_matches(|c: char| !c.is_ascii_digit()).to_string();
        let layer_up = t.layer.as_deref().unwrap_or("").to_ascii_uppercase();
        let on_title = layer_up.contains("TITLE") || layer_up.contains("BORDER") || layer_up.contains("TBLOCK");
        // sheet position: the title block is typically in the bottom-right quadrant
        let (sx, sy) = (t.at.0 / 72.0, t.at.1 / 72.0);
        let in_title_area = sx > sheet.0 * 0.6 && sy < sheet.1 * 0.3;
        let priority = if on_title {
            0
        } else if in_title_area {
            1
        } else {
            2
        };
        if let Some(idx) = seen.get(&key) {
            if let Some(c) = candidates.get_mut(*idx) {
                c.count += 1;
                if priority < c.priority {
                    c.priority = priority;
                }
            }
        } else {
            seen.insert(key.clone(), candidates.len());
            candidates.push(Candidate { scale: k, text: key, count: 1, priority });
        }
    }
    // best = lowest priority, then most occurrences
    candidates.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.count.cmp(&a.count)));
    candidates.into_iter().next().map(|c| (c.scale, c.text))
}

// ---------------------------------------------------------------------------------------------
// Fix 2: Content-class layer assignment
// ---------------------------------------------------------------------------------------------

/// Content class for a PDF entity, used to assign layers by content type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ContentClass {
    /// Gray dashed long lines — structural grid
    Grid,
    /// Wall linework (heavy black lines forming closed regions)
    Wall,
    /// Text entities
    Text,
    /// Symbols (repeated small clusters, devices, fixtures)
    Symbol,
    /// Title block elements (near sheet border, in title block area)
    TitleBlock,
    /// Solid fills / hatches
    Hatch,
    /// Annotation / dimensions / leaders
    Anno,
    /// Everything else
    Geometry,
}

/// Classify a stroke by its visual characteristics and position on the sheet.
fn classify_stroke(s: &Stroke, sheet: (f64, f64), grid_lines: &HashSet<usize>) -> ContentClass {
    if grid_lines.contains(&s.order) {
        return ContentClass::Grid;
    }
    let (sw, sh) = (sheet.0 * 72.0, sheet.1 * 72.0);
    // title block: near the border of the sheet
    if let Some(b) = bbox(std::slice::from_ref(&s.pts)) {
        let near_right = b[2] > sw * 0.95;
        let near_bottom = b[1] < sh * 0.05;
        let near_border = near_right || near_bottom || b[0] < sw * 0.02 || b[3] > sh * 0.98;
        if near_border && (b[2] - b[0] < sw * 0.4 || b[3] - b[1] < sh * 0.4) {
            return ContentClass::TitleBlock;
        }
    }
    // heavy black lines are likely walls
    let is_black = s.pen.rgb.iter().all(|c| *c <= 38);
    if is_black && s.pen.lw >= 35 {
        return ContentClass::Wall;
    }
    // thin lines with arrowhead-like short segments: annotation (dims, leaders)
    if s.pen.lw <= 15 && s.pts.len() <= 4 {
        let total_len: f64 = s.pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum();
        if total_len < sw.min(sh) * 0.15 && total_len > 1.0 {
            return ContentClass::Anno;
        }
    }
    ContentClass::Geometry
}

/// Detect grid lines: long, same-color, evenly spaced horizontal or vertical lines that span
/// most of the sheet — typically the structural grid on construction documents.
fn detect_grid_lines(strokes: &[Stroke], sheet: (f64, f64)) -> HashSet<usize> {
    let mut grid_orders = HashSet::new();
    let (sw, sh) = (sheet.0 * 72.0, sheet.1 * 72.0);
    let min_span = sw.min(sh) * 0.5;
    // collect long horizontal and vertical lines
    struct GridCandidate {
        order: usize,
        horizontal: bool,
        pos: f64, // the fixed coordinate (y for horizontal, x for vertical)
        pen: Pen,
    }
    let mut candidates: Vec<GridCandidate> = Vec::new();
    for s in strokes {
        if s.pts.len() < 2 || s.closed {
            continue;
        }
        let b = match bbox(std::slice::from_ref(&s.pts)) {
            Some(b) => b,
            None => continue,
        };
        let dx = b[2] - b[0];
        let dy = b[3] - b[1];
        // horizontal: spans most of the sheet width, thin vertically
        if dx > min_span && dy < 2.0 {
            candidates.push(GridCandidate { order: s.order, horizontal: true, pos: (b[1] + b[3]) / 2.0, pen: s.pen });
        }
        // vertical: spans most of the sheet height, thin horizontally
        if dy > min_span && dx < 2.0 {
            candidates.push(GridCandidate { order: s.order, horizontal: false, pos: (b[0] + b[2]) / 2.0, pen: s.pen });
        }
    }
    // group by direction and pen: ≥4 same-pen lines in one direction = grid
    let mut groups: HashMap<(bool, Pen), Vec<&GridCandidate>> = HashMap::new();
    for c in &candidates {
        groups.entry((c.horizontal, c.pen)).or_default().push(c);
    }
    for ((_, _), members) in &groups {
        if members.len() >= 4 {
            // check roughly even spacing
            let mut positions: Vec<f64> = members.iter().map(|c| c.pos).collect();
            positions.sort_by(f64::total_cmp);
            let spacings: Vec<f64> = positions.windows(2).map(|w| w[1] - w[0]).collect();
            if !spacings.is_empty() {
                let mean = spacings.iter().sum::<f64>() / spacings.len() as f64;
                let even = spacings.iter().all(|s| (*s - mean).abs() < mean * 0.3 + 5.0);
                if even {
                    for m in members {
                        grid_orders.insert(m.order);
                    }
                }
            }
        }
    }
    grid_orders
}

/// The content-class layer name for the given class.
fn content_class_layer(class: ContentClass) -> &'static str {
    match class {
        ContentClass::Grid => "E-GRID",
        ContentClass::Wall => "A-WALL",
        ContentClass::Text => "TEXT",
        ContentClass::Symbol => "E-DEVICE",
        ContentClass::TitleBlock => "TITLEBLOCK",
        ContentClass::Hatch => "HATCH",
        ContentClass::Geometry => "PDF-GEOMETRY",
        ContentClass::Anno => "ANNO",
    }
}

// ---------------------------------------------------------------------------------------------
// Fix 3: SHX text recovery — use invisible text (rendering mode 3) as the ground truth for
// stroked lettering, then fall back to feature-based glyph matching for any remainders.
// AutoCAD draws SHX text twice: once as linework (strokes) and once as invisible searchable
// text at the same position.  Matching hidden text to stroke clusters gives perfect accuracy.
// ---------------------------------------------------------------------------------------------

/// A page with this many real text items or more has its lettering as text: no OCR without
/// hidden text (Revit and TrueType prints; stroke OCR there only reads symbol ticks) unless
/// stroked lettering clearly outnumbers it (see [`OCR_STROKES_PER_CHAR`]).
const OCR_REAL_TEXT_LIMIT: usize = 100;

/// Loose glyph-sized strokes (outside real text) per real-text character above which a page's
/// lettering is mostly strokes and the OCR runs anyway: a plotted sheet with a few hundred
/// real labels and thousands of words of SHX lettering.
const OCR_STROKES_PER_CHAR: usize = 4;

/// The longest stroke side the OCR takes for lettering, by sheet size (points).
fn ocr_max_seg(sheet: (f64, f64)) -> f64 {
    (sheet.0.min(sheet.1) * 72.0 * 0.025).clamp(2.0, 40.0)
}

/// Most points a glyph stroke has: a plotted round letter (an 8, an S drawn in one stroke
/// with its arcs flattened) runs to a few dozen.
const OCR_MAX_STROKE_POINTS: usize = 64;

/// Whether a stroke could be (part of) a stroked glyph: a few points, glyph-sized, open or a
/// loop (the importer closes a D's stem and bowl, or an O, into one).
fn ocr_candidate(s: &Stroke, max_seg: f64) -> Option<[f64; 4]> {
    if s.pts.len() > OCR_MAX_STROKE_POINTS || (s.closed && s.pts.len() < 4) {
        return None;
    }
    // one stroke can draw a whole letter (a G, an 8): several times the letter's size
    let seg_len: f64 = s.pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum();
    if seg_len > max_seg * 8.0 || seg_len < 0.05 {
        return None;
    }
    let b = bbox(std::slice::from_ref(&s.pts))?;
    ((b[2] - b[0]) <= max_seg * 2.0 && (b[3] - b[1]) <= max_seg * 2.0).then_some(b)
}

/// The confidence (0..1, its weakest character's) under which an OCR reading isn't inserted: at
/// the template threshold it is as likely a tick, a corner or a symbol's bars as a letter.
const OCR_MIN_CONFIDENCE: f64 = 0.2;

/// The OCR's readings settled against the page's hidden text. A reading inside a hidden
/// string's box (where the PDF says that string is written: its start to its end, a letter
/// high) is that string's lettering: when the string was recovered already its strokes join
/// that text; else the reading becomes the string, one per string (the PDF says what is written
/// there; the OCR read `H`, `6`, `O` for `Hello`). Of the rest, a lone `O` drawn as one round
/// closed loop is a circle, and on a page with hidden text (the OCR fills in round real text
/// there) a reading under [`OCR_MIN_CONFIDENCE`] is too weak: neither is inserted (their
/// strokes stay linework). A page lettered in strokes alone keeps its weak readings (in the
/// unconfirmed OCR style): its room numbers are read there, and the sheet's registration needs
/// them. The readings kept, and how many took a hidden text, were circles, were too weak.
fn settle_ocr(
    ocr: Vec<RecoveredText>,
    texts: &[TextOut],
    strokes: &[Stroke],
    recovered: &mut [RecoveredText],
) -> (Vec<RecoveredText>, usize, usize, usize) {
    // Each hidden string's box, a third of a letter round.
    let hidden: Vec<(&TextOut, [f64; 4])> = texts
        .iter()
        .filter(|t| t.layer.as_deref() == Some(HIDDEN_TEXT_LAYER) && !t.value.trim().is_empty())
        .map(|t| {
            let b = text_box(t);
            let m = t.height.max(0.5) / 3.0;
            (t, [b[0] - m, b[1] - m, b[2] + m, b[3] + m])
        })
        .collect();
    let mut taken: Vec<Option<RecoveredText>> = (0..hidden.len()).map(|_| None).collect();
    let mut kept = Vec::new();
    let (mut from_hidden, mut circles, mut weak) = (0usize, 0usize, 0usize);
    for rt in ocr {
        let on = hidden.iter().position(|(_, b)| rt.at.0 >= b[0] && rt.at.0 <= b[2] && rt.at.1 >= b[1] && rt.at.1 <= b[3]);
        match on.and_then(|i| Some((i, hidden.get(i)?.0))) {
            Some((i, h)) => {
                // Recovered already: the reading's strokes are more of that text's lettering.
                let near = h.height.max(0.5);
                if let Some(done) =
                    recovered.iter_mut().find(|r| r.value.trim() == h.value.trim() && (r.at.0 - h.at.0).hypot(r.at.1 - h.at.1) <= near)
                {
                    from_hidden += 1;
                    done.stroke_indices.extend(rt.stroke_indices);
                    continue;
                }
                // Not recovered: a reading of the string's own words stays as it was read; one
                // that isn't (a misread: `6` for `e`) takes the string.
                if reads_like(&rt.value, &h.value) {
                    kept.push(rt);
                    continue;
                }
                from_hidden += 1;
                let Some(slot) = taken.get_mut(i) else { continue };
                match slot {
                    Some(t) => t.stroke_indices.extend(rt.stroke_indices),
                    None => {
                        *slot = Some(RecoveredText {
                            at: h.at,
                            height: h.height,
                            value: h.value.trim().to_string(),
                            stroke_indices: rt.stroke_indices,
                            pen: rt.pen,
                            layer: rt.layer,
                            unconfirmed: false,
                            confidence: None,
                            review: false,
                            rotation: h.rotation,
                        })
                    }
                }
            }
            None if matches!(rt.value.trim(), "O" | "o" | "0") && rt.stroke_indices.iter().any(|i| strokes.get(*i).is_some_and(round_loop)) => {
                circles += 1
            }
            None if !hidden.is_empty() && rt.confidence.is_some_and(|c| c < OCR_MIN_CONFIDENCE) => weak += 1,
            // On a page whose lettering has hidden copies, a reading where none is that reads like
            // no word is kept for review, not as the sheet's text.
            None if !hidden.is_empty() && !reads_as_word(&rt.value) => kept.push(RecoveredText { review: true, ..rt }),
            None => kept.push(rt),
        }
    }
    kept.extend(taken.into_iter().flatten());
    (kept, from_hidden, circles, weak)
}

/// Whether an OCR reading reads like a word, a number or a mark a drawing prints (`151`, `A-1`,
/// `XA`, `#12`): two letters or digits at least, making up most of it (not `U`, not `M L#.'`).
fn reads_as_word(reading: &str) -> bool {
    let t = reading.trim();
    let alnum = t.chars().filter(|c| c.is_alphanumeric()).count();
    let other = t.chars().filter(|c| !c.is_alphanumeric() && !c.is_whitespace()).count();
    alnum >= 2 && alnum >= 2 * other
}

/// Whether an OCR reading reads a string: its letters and digits run in the string's order and
/// make up at least 60% of the string's (a whole word misread a little; not a fragment like
/// `H L` read off `Hello`), any case.
fn reads_like(reading: &str, string: &str) -> bool {
    let h: Vec<char> = string.chars().filter(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    let r: Vec<char> = reading.chars().filter(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if r.is_empty() || h.is_empty() {
        return false;
    }
    let mut it = h.iter();
    let in_order = r.iter().all(|c| it.any(|x| x == c));
    in_order && 5 * r.len() >= 3 * h.len()
}

/// A stroke that is a circle: closed (or ending where it starts), every point the same distance
/// from its middle to within 12%. A plot font's `O` is taller than wide; a circle is round.
fn round_loop(s: &Stroke) -> bool {
    let (Some(first), Some(last)) = (s.pts.first(), s.pts.last()) else { return false };
    let n = s.pts.len();
    if n < 8 {
        return false;
    }
    let c = s.pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let c = (c.0 / n as f64, c.1 / n as f64);
    let radii: Vec<f64> = s.pts.iter().map(|p| (p.0 - c.0).hypot(p.1 - c.1)).collect();
    let (lo, hi) = radii.iter().fold((f64::INFINITY, 0.0f64), |a, r| (a.0.min(*r), a.1.max(*r)));
    let closes = s.closed || (first.0 - last.0).hypot(first.1 - last.1) <= 0.1 * hi;
    closes && hi > 0.0 && lo / hi >= 0.88
}

/// The strokes that lie on a visible real text item: a plotted copy of the text, which the
/// text already is.  The OCR leaves them alone.  (Strokes under hidden text are the lettering
/// the hidden text stands for; what its own recovery didn't claim, the OCR may read.)
fn strokes_under_text(strokes: &[Stroke], texts: &[TextOut]) -> HashSet<usize> {
    const CELL: f64 = 36.0;
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for t in texts.iter().filter(|t| !t.value.trim().is_empty() && t.layer.as_deref() != Some(HIDDEN_TEXT_LAYER)) {
        let h = t.height.max(0.5);
        let (dx, dy) = (t.end.0 - t.at.0, t.end.1 - t.at.1);
        let len = dx.hypot(dy);
        // the text's up direction (perpendicular to its baseline)
        let (ux, uy) = if len > 1e-9 { (-dy / len, dx / len) } else { (0.0, 1.0) };
        let corners = [t.at, t.end, (t.at.0 + ux * h, t.at.1 + uy * h), (t.end.0 + ux * h, t.end.1 + uy * h)];
        let pad = h * 0.25;
        let b = corners.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
        let b = [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad];
        let (gx0, gy0, gx1, gy1) =
            ((b[0] / CELL).floor() as i64, (b[1] / CELL).floor() as i64, (b[2] / CELL).floor() as i64, (b[3] / CELL).floor() as i64);
        // a long note spans many cells; a hostile one is capped
        if (gx1 - gx0 + 1).saturating_mul(gy1 - gy0 + 1) > 2_000 {
            continue;
        }
        let bi = boxes.len();
        boxes.push(b);
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                grid.entry((gx, gy)).or_default().push(bi);
            }
        }
    }
    let mut under = HashSet::new();
    if boxes.is_empty() {
        return under;
    }
    for (i, s) in strokes.iter().enumerate() {
        let Some(b) = bbox(std::slice::from_ref(&s.pts)) else { continue };
        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let Some(cands) = grid.get(&((cx / CELL).floor() as i64, (cy / CELL).floor() as i64)) else { continue };
        if cands.iter().filter_map(|&bi| boxes.get(bi)).any(|t| cx >= t[0] && cx <= t[2] && cy >= t[1] && cy <= t[3]) {
            under.insert(i);
        }
    }
    under
}

/// A text entity recovered from SHX stroke clusters.
struct RecoveredText {
    at: (f64, f64),
    height: f64,
    value: String,
    stroke_indices: HashSet<usize>,
    pen: Pen,
    layer: Option<String>,
    /// Read by the OCR with no hidden text to confirm it (drawn in [`OCR_TEXT_STYLE`]).
    unconfirmed: bool,
    /// The OCR's confidence in the reading (0..1); `None` when the value came from real text.
    confidence: Option<f64>,
    /// An OCR reading for review only ([`OCR_REVIEW_LAYER`]).
    review: bool,
    /// The lettering's direction in degrees, counter-clockwise (90: reading up the sheet).
    rotation: f64,
}

/// Recover SHX-stroked text by matching invisible (hidden) text entities to nearby stroke
/// clusters.  Each hidden text entity that sits near a group of short strokes at the same
/// position and height is adopted: the strokes are consumed and the hidden text's value is
/// used.  Returns the recovered texts and the set of consumed stroke indices.
fn recover_shx_from_hidden(strokes: &[Stroke], texts: &[TextOut], sheet: (f64, f64)) -> Vec<RecoveredText> {
    // separate hidden from visible texts
    let hidden: Vec<&TextOut> = texts.iter().filter(|t| t.layer.as_deref() == Some(HIDDEN_TEXT_LAYER)).collect();
    if hidden.is_empty() {
        return Vec::new();
    }

    let (sw, sh) = (sheet.0 * 72.0, sheet.1 * 72.0);
    // SHX strokes are short segments (2% of sheet) around each hidden text position
    let max_seg = (sw.min(sh) * 0.025).clamp(2.0, 40.0);

    // build a spatial grid of short strokes
    let cell = max_seg * 4.0;
    struct SI {
        idx: usize,
        cx: f64,
        cy: f64,
    }
    let mut short: Vec<SI> = Vec::new();
    for (i, s) in strokes.iter().enumerate() {
        if s.pts.len() > 8 {
            continue;
        }
        let seg_len: f64 = s.pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum();
        if seg_len > max_seg || seg_len < 0.05 {
            continue;
        }
        if let Some(b) = bbox(std::slice::from_ref(&s.pts)) {
            short.push(SI { idx: i, cx: (b[0] + b[2]) / 2.0, cy: (b[1] + b[3]) / 2.0 });
        }
    }
    let mut sgrid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (si, s) in short.iter().enumerate() {
        let gx = (s.cx / cell).floor() as i64;
        let gy = (s.cy / cell).floor() as i64;
        sgrid.entry((gx, gy)).or_default().push(si);
    }

    let mut used: HashSet<usize> = HashSet::new();
    let mut result: Vec<RecoveredText> = Vec::new();

    for ht in &hidden {
        if ht.value.trim().is_empty() {
            continue;
        }
        let h = ht.height.max(1.0);
        // approximate the text's bounding box from its position, height and character count
        let est_w = h * 0.6 * ht.value.len().max(1) as f64;
        let (tx0, ty0) = (ht.at.0 - h * 0.2, ht.at.1 - h * 0.3);
        let (tx1, ty1) = (ht.at.0 + est_w + h * 0.2, ht.at.1 + h * 1.3);
        // find short strokes inside this box
        let gx0 = ((tx0 / cell).floor() as i64) - 1;
        let gx1 = ((tx1 / cell).floor() as i64) + 1;
        let gy0 = ((ty0 / cell).floor() as i64) - 1;
        let gy1 = ((ty1 / cell).floor() as i64) + 1;
        let mut matched: Vec<usize> = Vec::new();
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                if let Some(members) = sgrid.get(&(gx, gy)) {
                    for &si in members {
                        let s = &short[si];
                        if used.contains(&s.idx) {
                            continue;
                        }
                        if s.cx >= tx0 && s.cx <= tx1 && s.cy >= ty0 && s.cy <= ty1 {
                            matched.push(s.idx);
                        }
                    }
                }
            }
        }
        // need at least 1 stroke per character (SHX glyphs typically have 2-6 strokes each)
        if matched.len() < ht.value.len().max(1) {
            continue;
        }
        for &si in &matched {
            used.insert(si);
        }
        let pen = strokes.get(matched[0]).map(|s| s.pen).unwrap_or(Pen { rgb: [0, 0, 0], lw: 0, transp: 0 });
        let layer = strokes.get(matched[0]).and_then(|s| s.layer.clone());
        result.push(RecoveredText {
            at: ht.at,
            height: ht.height,
            value: ht.value.clone(),
            stroke_indices: matched.into_iter().collect(),
            pen,
            layer,
            unconfirmed: false,
            confidence: None,
            review: false,
            rotation: 0.0,
        });
    }
    result
}

/// Fallback SHX text recovery: when no hidden text is available, detect clusters of short
/// strokes that form glyph-like patterns and match them to nearby visible text.  This is less
/// accurate than hidden-text matching but recovers more text than leaving it as linework.
fn recover_shx_fallback(strokes: &[Stroke], texts: &[TextOut], sheet: (f64, f64)) -> Vec<RecoveredText> {
    let (sw, sh) = (sheet.0 * 72.0, sheet.1 * 72.0);
    let max_seg = (sw.min(sh) * 0.02).clamp(2.0, 25.0);
    let cell = max_seg * 3.0;

    // find short strokes (SHX candidates)
    struct SI {
        idx: usize,
        cx: f64,
        cy: f64,
        pen: Pen,
    }
    let mut short: Vec<SI> = Vec::new();
    for (i, s) in strokes.iter().enumerate() {
        if s.closed || s.pts.len() > 6 {
            continue;
        }
        let seg_len: f64 = s.pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum();
        if seg_len > max_seg || seg_len < 0.1 {
            continue;
        }
        if let Some(b) = bbox(std::slice::from_ref(&s.pts)) {
            short.push(SI { idx: i, cx: (b[0] + b[2]) / 2.0, cy: (b[1] + b[3]) / 2.0, pen: s.pen });
        }
    }
    if short.is_empty() {
        return Vec::new();
    }

    // cluster short strokes by proximity and same pen
    let mut grid: HashMap<(i64, i64, Pen), Vec<usize>> = HashMap::new();
    for (si, s) in short.iter().enumerate() {
        let gx = (s.cx / cell).floor() as i64;
        let gy = (s.cy / cell).floor() as i64;
        for dx in -1..=1 {
            for dy in -1..=1 {
                grid.entry((gx + dx, gy + dy, s.pen)).or_default().push(si);
            }
        }
    }
    let mut visited = vec![false; short.len()];
    let mut clusters: Vec<([f64; 4], Vec<usize>, f64, Pen)> = Vec::new(); // bounds, indices, height, pen
    for si in 0..short.len() {
        if visited[si] {
            continue;
        }
        visited[si] = true;
        let s = &short[si];
        let mut members = vec![si];
        let mut queue = vec![si];
        let mut bounds = [s.cx, s.cy, s.cx, s.cy];
        while let Some(cur) = queue.pop() {
            let c = &short[cur];
            let gx = (c.cx / cell).floor() as i64;
            let gy = (c.cy / cell).floor() as i64;
            if let Some(neighbors) = grid.get(&(gx, gy, c.pen)) {
                for &ni in neighbors {
                    if visited[ni] {
                        continue;
                    }
                    let n = &short[ni];
                    if (n.cx - c.cx).abs() < cell && (n.cy - c.cy).abs() < cell {
                        visited[ni] = true;
                        members.push(ni);
                        queue.push(ni);
                        bounds = [bounds[0].min(n.cx), bounds[1].min(n.cy), bounds[2].max(n.cx), bounds[3].max(n.cy)];
                    }
                }
            }
        }
        let h = bounds[3] - bounds[1];
        let w = bounds[2] - bounds[0];
        if members.len() >= 2 && h > 0.5 && h < max_seg * 12.0 && w > 0.5 && w < max_seg * 100.0 {
            clusters.push((bounds, members.iter().map(|&mi| short[mi].idx).collect(), h, s.pen));
        }
    }

    // for each cluster, try to find a visible text entity that overlaps it
    let mut result: Vec<RecoveredText> = Vec::new();
    let mut used_texts: HashSet<usize> = HashSet::new();
    for (bounds, indices, height, pen) in &clusters {
        let pad = height * 0.5;
        let cb = [bounds[0] - pad, bounds[1] - pad, bounds[2] + pad, bounds[3] + pad];
        // find visible text that overlaps this cluster
        let mut best: Option<(usize, f64)> = None;
        for (ti, t) in texts.iter().enumerate() {
            if used_texts.contains(&ti) {
                continue;
            }
            if t.layer.as_deref() == Some(HIDDEN_TEXT_LAYER) {
                continue;
            }
            let th = t.height.max(1.0);
            let tw = th * 0.6 * t.value.len().max(1) as f64;
            let tb = [t.at.0 - 1.0, t.at.1 - 1.0, t.at.0 + tw + 1.0, t.at.1 + th + 1.0];
            if tb[0] < cb[2] && tb[2] > cb[0] && tb[1] < cb[3] && tb[3] > cb[1] {
                let dist = ((t.at.0 - bounds[0]).hypot(t.at.1 - bounds[1])).abs();
                if !best.is_some_and(|(_, d)| dist >= d) {
                    best = Some((ti, dist));
                }
            }
        }
        if let Some((ti, _)) = best {
            used_texts.insert(ti);
            let t = &texts[ti];
            result.push(RecoveredText {
                at: (bounds[0], bounds[1]),
                height: *height,
                value: t.value.clone(),
                stroke_indices: indices.iter().copied().collect(),
                pen: *pen,
                layer: t.layer.clone(),
                unconfirmed: false,
                confidence: None,
                review: false,
                rotation: 0.0,
            });
        }
    }
    result
}

// ---------------------------------------------------------------------------------------------
// SHX OCR: recognise stroked (SHX-style) lettering by matching individual glyph clusters
// against stroke templates.  Used when no hidden text covers the strokes.
//
// Source: CADaddy's own single-stroke font definitions (crates/fonts/src/stroke.rs), which are
// original (CC0).  Each glyph is normalised to its own box, so matching tolerates size and
// width factor; a slanted (oblique) glyph is also tried straightened.
//
// Algorithm:
//   1. Find all short open strokes that could be SHX glyph strokes.
//   2. Join strokes that touch (same pen, within a tenth of their size) into glyph clusters:
//      single-stroke lettering draws each character as connected strokes, with a gap to the
//      next character.
//   3. Match each glyph cluster against the templates by chamfer distance (upright, and with
//      its slant removed); the distance gives the match's confidence.
//   4. Assemble matched glyphs into lines by baseline and height, then into texts by spacing.
//   5. Drop lone glyphs that plain linework also makes (a single straight stroke, punctuation)
//      and emit TEXT entities for the rest.
// ---------------------------------------------------------------------------------------------

/// Normalised stroke template for one character: sampled points along the template strokes,
/// each in the [0,1]×[0,1] unit cell (bottom-left origin).
struct ShxTemplate {
    ch: char,
    /// Evenly sampled points along all strokes, normalised to [0,1]².
    samples: Vec<(f64, f64)>,
    /// Width/height aspect ratio of this character's bounding box in the source font.
    aspect: f64,
    /// Expected number of strokes.
    n_strokes: usize,
}

/// The gap (as a share of the larger stroke's size, up to twice the smaller's) under which two
/// strokes join one glyph.
const OCR_JOIN_TOLERANCE: f64 = 0.05;

/// Sample points per glyph, spread along its strokes by length (so a glyph the PDF drew as
/// one joined polyline samples like the same glyph drawn as separate strokes).
const GLYPH_SAMPLES: usize = 32;

/// The characters the OCR reads.
const OCR_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-./'\"()#&,";

/// Other common shapes of single-stroke plot lettering, in the stroke font's notation
/// (`crates/fonts/src/stroke.rs`: x digit, y letter with `c` the baseline, `i` the cap
/// line, strokes split by spaces).  Plot fonts differ from ours mostly here: an M whose
/// middle reaches the baseline, a W whose middle reaches the cap line, a 1 without a foot,
/// a plain I and a 5 with a round bowl.
const OCR_VARIANTS: [(char, &str); 5] = [('M', "0c0i2c4i4c"), ('W', "0i1c2i3c4i"), ('1', "1h2i2c"), ('I', "0i0c"), ('5', "4i1i0f1g3g4f4d3c1c0d")];

/// Build the template library from our stroke font for the characters the OCR reads, plus
/// the [`OCR_VARIANTS`].
fn shx_templates() -> Vec<ShxTemplate> {
    let font = OCR_CHARS.chars().filter_map(|ch| cadaddy_fonts::stroke::glyph(ch).map(|(_, spec)| (ch, spec)));
    let mut templates = Vec::new();
    for (ch, spec) in font.chain(OCR_VARIANTS) {
        let raw_strokes = cadaddy_fonts::stroke::strokes(spec);
        let refs: Vec<&[(f64, f64)]> = raw_strokes.iter().map(Vec::as_slice).filter(|s| !s.is_empty()).collect();
        let Some((samples, aspect)) = glyph_samples(&refs, 0.0) else { continue };
        templates.push(ShxTemplate { ch, samples, aspect, n_strokes: refs.len() });
    }
    templates
}

/// The frame a glyph is normalised to: its bounding box, except that a side much thinner than
/// the other (an I, a 1, a dash) is widened to the other's size around its centre, so pen
/// jitter across a thin stroke doesn't fill the whole unit cell.  Returns `(x0, y0, w, h)`.
fn norm_frame(b: [f64; 4]) -> (f64, f64, f64, f64) {
    let (mut x0, mut y0) = (b[0], b[1]);
    let (mut w, mut h) = (b[2] - b[0], b[3] - b[1]);
    let big = w.max(h).max(1e-6);
    if w < big * 0.15 {
        x0 = (b[0] + b[2]) / 2.0 - big / 2.0;
        w = big;
    }
    if h < big * 0.15 {
        y0 = (b[1] + b[3]) / 2.0 - big / 2.0;
        h = big;
    }
    (x0, y0, w, h)
}

/// Sample a glyph's strokes, sheared by `slant` (x -= slant·y: 0 keeps it as drawn) and
/// normalised to its frame.  Returns the samples and the box's aspect (width / height, kept
/// within 0.1..10 so thin glyphs compare).
fn glyph_samples(strokes: &[&[(f64, f64)]], slant: f64) -> Option<(Vec<(f64, f64)>, f64)> {
    let sheared: Vec<Vec<(f64, f64)>> = strokes.iter().map(|s| s.iter().map(|&(x, y)| (x - slant * y, y)).collect()).collect();
    let b = bbox(&sheared)?;
    let (x0, y0, w, h) = norm_frame(b);
    let length = |s: &[(f64, f64)]| s.windows(2).map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1)).sum::<f64>();
    let total: f64 = sheared.iter().map(|s| length(s)).sum::<f64>().max(1e-9);
    let mut samples = Vec::new();
    for s in &sheared {
        let n = ((GLYPH_SAMPLES as f64 * length(s) / total).round() as usize).max(2);
        sample_polyline_norm(s, x0, y0, w, h, n, &mut samples);
    }
    let aspect = ((b[2] - b[0]) / (b[3] - b[1]).max(1e-6)).clamp(0.1, 10.0);
    Some((samples, aspect))
}

/// Sample a polyline into `n` evenly spaced points, normalised by the given bbox.
fn sample_polyline_norm(pts: &[(f64, f64)], x0: f64, y0: f64, w: f64, h: f64, n: usize, out: &mut Vec<(f64, f64)>) {
    if pts.len() < 2 {
        if let Some(&(x, y)) = pts.first() {
            out.push(((x - x0) / w, (y - y0) / h));
        }
        return;
    }
    let total: f64 = pts.windows(2).map(|s| (s[1].0 - s[0].0).hypot(s[1].1 - s[0].1)).sum();
    if total < 1e-9 {
        if let Some(&(x, y)) = pts.first() {
            out.push(((x - x0) / w, (y - y0) / h));
        }
        return;
    }
    let n = n.max(2);
    for si in 0..n {
        let target = total * si as f64 / (n - 1) as f64;
        let mut acc = 0.0;
        let mut found = false;
        for seg in pts.windows(2) {
            let sl = (seg[1].0 - seg[0].0).hypot(seg[1].1 - seg[0].1);
            if acc + sl >= target - 1e-12 {
                let t = if sl > 1e-12 { (target - acc) / sl } else { 0.0 };
                let px = seg[0].0 + t * (seg[1].0 - seg[0].0);
                let py = seg[0].1 + t * (seg[1].1 - seg[0].1);
                out.push(((px - x0) / w, (py - y0) / h));
                found = true;
                break;
            }
            acc += sl;
        }
        if !found && let Some(&(x, y)) = pts.last() {
            out.push(((x - x0) / w, (y - y0) / h));
        }
    }
}

/// The directed chamfer distance from `a` to `b` (mean distance of `a`'s points to the nearest
/// of `b`'s), or `None` as soon as it must exceed `bound`.
fn chamfer(a: &[(f64, f64)], b: &[(f64, f64)], bound: f64) -> Option<f64> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let limit = bound * a.len() as f64;
    let mut sum = 0.0;
    for &(ax, ay) in a {
        let d2 = b.iter().map(|&(bx, by)| (ax - bx) * (ax - bx) + (ay - by) * (ay - by)).fold(f64::MAX, f64::min);
        sum += d2.sqrt();
        if sum > limit {
            return None;
        }
    }
    Some(sum / a.len() as f64)
}

/// The symmetric chamfer distance (the larger of both directions), `None` above `bound`.
fn chamfer_sym(a: &[(f64, f64)], b: &[(f64, f64)], bound: f64) -> Option<f64> {
    let ab = chamfer(a, b, bound)?;
    let ba = chamfer(b, a, bound)?;
    Some(ab.max(ba))
}

/// Max chamfer distance for a template match to be accepted.
const MATCH_THRESHOLD: f64 = 0.25;

/// A match's confidence from its chamfer distance: 1 for an exact fit, 0 at the threshold.
fn match_confidence(dist: f64) -> f64 {
    (1.0 - dist / MATCH_THRESHOLD).clamp(0.0, 1.0)
}

/// A glyph's reading: the best character, its chamfer distance, and the runner-up character
/// when it is a look-alike that fits about as well (an O for a 0, a B for an 8).
#[derive(Clone, Copy, Debug, PartialEq)]
struct GlyphRead {
    ch: char,
    dist: f64,
    alt: Option<char>,
}

/// Letter/digit look-alikes the stroke shapes can't tell apart; the word around them decides.
const LOOK_ALIKES: [(char, char); 6] = [('O', '0'), ('B', '8'), ('S', '5'), ('I', '1'), ('Z', '2'), ('G', '6')];

fn look_alike(a: char, b: char) -> bool {
    LOOK_ALIKES.iter().any(|&(l, d)| (a, b) == (l, d) || (a, b) == (d, l))
}

/// Match a candidate glyph against all templates.
fn match_glyph(candidate: &[(f64, f64)], candidate_aspect: f64, candidate_n_strokes: usize, templates: &[ShxTemplate]) -> Option<GlyphRead> {
    if candidate.is_empty() {
        return None;
    }
    let mut fits: Vec<(char, f64)> = Vec::new();
    for tpl in templates {
        // loose aspect ratio filter (width factors from about 0.4 to 2.5 pass)
        let ar = candidate_aspect / tpl.aspect;
        if !(0.25..=4.0).contains(&ar) {
            continue;
        }
        // loose stroke count filter: PDF output joins and splits strokes
        let (lo, hi) = (tpl.n_strokes / 2, tpl.n_strokes * 2 + 2);
        if candidate_n_strokes < lo || candidate_n_strokes > hi {
            continue;
        }
        // only fits that could still be the best or its look-alike runner-up matter
        let best = fits.iter().map(|f| f.1).fold(MATCH_THRESHOLD, f64::min);
        if let Some(dist) = chamfer_sym(candidate, &tpl.samples, (best * 1.5 + 0.02).min(MATCH_THRESHOLD)) {
            fits.push((tpl.ch, dist));
        }
    }
    // the closest fit; equal fits keep the template order (letters before digits)
    fits.sort_by(|a, b| a.1.total_cmp(&b.1));
    let &(ch, dist) = fits.first()?;
    let alt = fits.iter().skip(1).find(|&&(c, d)| look_alike(ch, c) && d <= dist * 1.2 + 0.01).map(|&(c, _)| c);
    Some(GlyphRead { ch, dist, alt })
}

/// A row slant from which the row counts as oblique lettering (about 8.5°).
const OBLIQUE_ROW: f64 = 0.15;

/// Largest slant the OCR straightens (horizontal shift per unit of height, about 31°).
const MAX_SLANT: f64 = 0.6;

/// The slant of a glyph: the length-weighted mean lean (dx per dy) of its near-vertical
/// full-height segments (within 35° of vertical).  About 0 for upright lettering (the legs of an A or a V
/// lean both ways and cancel); an oblique style leans all its verticals the same way.  One
/// glyph's lean is rough (a 7, a slanted A); the row's median is what the OCR uses.  `None`
/// for a glyph without long near-vertical segments (an O, a dash): it says nothing of the slant.
fn glyph_slant(strokes: &[&[(f64, f64)]]) -> Option<f64> {
    // only the strokes about the glyph's full height (a stem, a side): a curve's short corner
    // segments lean anyhow, and so does an R's or a K's leg
    let (lo, hi) = strokes.iter().flat_map(|s| s.iter()).fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let min_dy = (hi - lo) * 0.7;
    let (mut sum, mut weight) = (0.0, 0.0);
    for s in strokes {
        for w in s.windows(2) {
            let (mut dx, mut dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            if dy < 0.0 {
                dx = -dx;
                dy = -dy;
            }
            let len = dx.hypot(dy);
            if len < 1e-9 || dy < min_dy || dx.abs() > dy * 0.7 {
                continue;
            }
            sum += dx / dy * len;
            weight += len;
        }
    }
    (weight >= 1e-9).then(|| (sum / weight).clamp(-MAX_SLANT, MAX_SLANT))
}

/// Best match for a glyph's strokes, given the slant of its row (straightened at it and a
/// tenth either side, as the estimate is rough).  On a row that leans clearly (oblique
/// lettering) the straightened reading wins unless the upright one fits clearly better (by a
/// fifth); on one that barely leans it is the other way round, so an upright 7 or / stays
/// itself.
fn read_glyph(strokes: &[&[(f64, f64)]], slant: f64, templates: &[ShxTemplate]) -> Option<GlyphRead> {
    let n = strokes.len();
    let upright = glyph_samples(strokes, 0.0).and_then(|(s, a)| match_glyph(&s, a, n, templates));
    if slant.abs() < 0.05 {
        return upright;
    }
    let straightened = [slant - 0.1, slant, slant + 0.1]
        .into_iter()
        .filter(|s| s.abs() >= 0.05 && s.abs() <= MAX_SLANT)
        .filter_map(|s| glyph_samples(strokes, s).and_then(|(smp, a)| match_glyph(&smp, a, n, templates)))
        .min_by(|a, b| a.dist.total_cmp(&b.dist));
    match (upright, straightened) {
        (Some(u), Some(s)) if slant.abs() >= OBLIQUE_ROW => Some(if u.dist < s.dist * 0.8 { u } else { s }),
        (Some(u), Some(s)) if s.dist < u.dist * 0.8 => Some(s),
        (None, s) => s,
        (u, _) => u,
    }
}

/// Distance between two polylines (0 when they cross or touch).
fn polyline_gap(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    let seg = |s: &[(f64, f64)]| -> Vec<((f64, f64), (f64, f64))> {
        if s.len() == 1 { s.first().map(|&p| vec![(p, p)]).unwrap_or_default() } else { s.windows(2).map(|w| (w[0], w[1])).collect() }
    };
    let mut best = f64::MAX;
    for (p0, p1) in seg(a) {
        for (q0, q1) in seg(b) {
            best = best.min(segment_gap(p0, p1, q0, q1));
            if best <= 0.0 {
                return 0.0;
            }
        }
    }
    best
}

fn point_segment_gap(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-18 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

fn segment_gap(p0: (f64, f64), p1: (f64, f64), q0: (f64, f64), q1: (f64, f64)) -> f64 {
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let (d1, d2) = (cross(q0, q1, p0), cross(q0, q1, p1));
    let (d3, d4) = (cross(p0, p1, q0), cross(p0, p1, q1));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return 0.0;
    }
    point_segment_gap(p0, q0, q1).min(point_segment_gap(p1, q0, q1)).min(point_segment_gap(q0, p0, p1)).min(point_segment_gap(q1, p0, p1))
}

/// What the OCR pass found on a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrReport {
    /// Glyph-sized stroke clusters the OCR tried to read.
    pub clusters: usize,
    /// Clusters that matched a character above the threshold.
    pub glyphs: usize,
    /// Lone matches dropped as linework noise (a single straight stroke, punctuation alone).
    pub noise_dropped: usize,
    /// Texts read in lettering that runs up or down the sheet.
    pub turned: usize,
    /// The texts it recovered, with their confidence.
    pub words: Vec<OcrWord>,
}

impl OcrReport {
    /// The mean confidence of the recovered texts (0 when there are none).
    pub fn mean_confidence(&self) -> f64 {
        if self.words.is_empty() { 0.0 } else { self.words.iter().map(|w| w.confidence).sum::<f64>() / self.words.len() as f64 }
    }
}

/// One text the OCR recovered.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub value: String,
    /// Insertion point in drawing units.
    pub at: (f64, f64),
    /// Its weakest character's confidence, 0..1 (1: an exact template fit; 0: at the threshold).
    pub confidence: f64,
}

/// A matched glyph.
struct MatchedGlyph {
    ch: char,
    /// A look-alike the shapes fit as well (see [`LOOK_ALIKES`]).
    alt: Option<char>,
    /// Its box, the x extent upright (an oblique row's slant taken out).
    bb: [f64; 4],
    indices: Vec<usize>,
    pen: Pen,
    confidence: f64,
    /// One straight stroke (an I, a 1, a dash or a slash): plain linework looks the same.
    straight: bool,
    /// A single closed loop (an O, a D): alone it is as likely a small circle or box.  Also a
    /// glyph with a curve-drawn loop in it (`Stroke::curved_loop`): alone it is a symbol (a
    /// circle with a bar through it reads as a 6).
    ring: bool,
    /// The slant of its row.
    slant: f64,
}

/// Characters that don't set a line's height or baseline: dashes, dots, commas and quotes sit
/// inside the letter band, and parentheses, slashes and number signs often reach past it.
fn small_glyph(ch: char) -> bool {
    matches!(ch, '-' | '.' | ',' | '\'' | '"') || tall_glyph(ch)
}

/// Characters plot fonts often draw taller than a capital (see [`small_glyph`]).
fn tall_glyph(ch: char) -> bool {
    matches!(ch, '(' | ')' | '/' | '#')
}

/// Settle the look-alikes of a word (O/0, B/8, I/1, ...): each takes the kind (letter or
/// digit) most of the sure characters of its part of the word are (the parts split at
/// punctuation: "LP" and "1A" in "LP-1A"); a part that doesn't say takes its nearest
/// neighbour part's kind (the 6 in "2,4,6"), else the whole word's.  A word of look-alikes
/// only is a number if any of them reads as a digit first (a 5 and a ring: "50").
fn settle_look_alikes(word: &mut [(&MatchedGlyph, char)]) {
    // the kind most sure characters of `part` are: Some(true) for digits
    let kind = |part: &[(&MatchedGlyph, char)]| -> Option<bool> {
        let (letters, digits) = part
            .iter()
            .filter(|(g, _)| g.alt.is_none())
            .fold((0, 0), |(l, d), (_, c)| (l + usize::from(c.is_ascii_alphabetic()), d + usize::from(c.is_ascii_digit())));
        (letters != digits).then_some(digits > letters)
    };
    let all_alike = word.iter().all(|(g, c)| g.alt.is_some() || !c.is_ascii_alphanumeric());
    let whole = kind(word).or_else(|| (all_alike && word.iter().any(|(_, c)| c.is_ascii_digit())).then_some(true));
    // the parts: runs of letters and digits
    let mut parts: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, (_, c)) in word.iter().enumerate() {
        match (c.is_ascii_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                parts.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        parts.push((s, word.len()));
    }
    let kinds: Vec<Option<bool>> = parts.iter().map(|&(a, b)| word.get(a..b).and_then(kind)).collect();
    for (pi, &(a, b)) in parts.iter().enumerate() {
        let near = || {
            let before = kinds.get(..pi).and_then(|k| k.iter().rev().flatten().next().copied());
            before.or_else(|| kinds.get(pi + 1..).and_then(|k| k.iter().flatten().next().copied()))
        };
        let Some(want_digit) = kinds.get(pi).copied().flatten().or_else(near).or(whole) else { continue };
        for (g, ch) in word.get_mut(a..b).into_iter().flatten() {
            if let Some(alt) = g.alt
                && alt.is_ascii_digit() == want_digit
            {
                *ch = alt;
            }
        }
    }
}

/// Recover SHX-stroked text by OCR: cluster short strokes into individual glyph-sized groups,
/// match each against stroke templates, and assemble matched glyphs into TEXT entities.
/// Strokes in `excluded` (already read or part of a symbol) are left alone.
/// Read stroked lettering in the three directions a plan uses: across the sheet, and up or down
/// along a vertical wall.  The upright reading goes first; its words (two or more letters or
/// digits) stand.  Each turned direction then reads a turned copy of the strokes those words
/// left, mapped back.  A turned word read upright falls apart into stray single glyphs, so a
/// turned reading (of two or more letters or digits: a lone turned glyph is as often a tick)
/// replaces the stray glyphs it shares strokes with.  Where the two turns read the same
/// strokes (a word read upside down too), the reading with more letters wins, then the surer.
fn recover_shx_ocr(strokes: &[Stroke], sheet: (f64, f64), excluded: &HashSet<usize>) -> (Vec<RecoveredText>, OcrReport) {
    let (upright, mut report) = recover_shx_ocr_upright(strokes, sheet, excluded);
    let alnum = |rt: &RecoveredText| rt.value.chars().filter(char::is_ascii_alphanumeric).count();
    let (mut result, stray): (Vec<RecoveredText>, Vec<RecoveredText>) = upright.into_iter().partition(|rt| alnum(rt) >= 2);
    let mut taken: HashSet<usize> = excluded.clone();
    for rt in &result {
        taken.extend(&rt.stroke_indices);
    }
    let mut turned_readings: Vec<RecoveredText> = Vec::new();
    // (the turn that lays the lettering flat, the turn back, the lettering's direction)
    let turns: [(fn((f64, f64)) -> (f64, f64), fn((f64, f64)) -> (f64, f64), f64); 2] =
        [(|(x, y)| (y, -x), |(u, v)| (-v, u), 90.0), (|(x, y)| (-y, x), |(u, v)| (v, -u), 270.0)];
    for (flat, back, rotation) in turns {
        let turned: Vec<Stroke> = strokes
            .iter()
            .map(|s| Stroke {
                pts: s.pts.iter().map(|&p| flat(p)).collect(),
                closed: s.closed,
                pen: s.pen,
                layer: s.layer.clone(),
                curve_loop: s.curve_loop,
                curved_loop: s.curved_loop,
                order: s.order,
            })
            .collect();
        let (texts, _) = recover_shx_ocr_upright(&turned, (sheet.1, sheet.0), &taken);
        for mut rt in texts.into_iter().filter(|rt| alnum(rt) >= 2) {
            rt.at = back(rt.at);
            rt.rotation = rotation;
            turned_readings.push(rt);
        }
    }
    // most letters first, then the surer reading (a word read upside down fits its letters worse)
    turned_readings.sort_by(|a, b| alnum(b).cmp(&alnum(a)).then(b.confidence.unwrap_or(0.0).total_cmp(&a.confidence.unwrap_or(0.0))));
    for rt in turned_readings {
        if rt.stroke_indices.iter().any(|s| taken.contains(s)) {
            continue;
        }
        taken.extend(&rt.stroke_indices);
        report.turned += 1;
        result.push(rt);
    }
    result.extend(stray.into_iter().filter(|rt| !rt.stroke_indices.iter().any(|s| taken.contains(s))));
    (result, report)
}

/// Read the lettering that runs across the sheet (left to right, upright or slanted).
fn recover_shx_ocr_upright(strokes: &[Stroke], sheet: (f64, f64), excluded: &HashSet<usize>) -> (Vec<RecoveredText>, OcrReport) {
    let mut report = OcrReport::default();
    let templates = shx_templates();
    if templates.is_empty() {
        return (Vec::new(), report);
    }

    let max_seg = ocr_max_seg(sheet);

    // ---- phase 1: find candidate SHX strokes ----
    struct Cand {
        idx: usize,
        bb: [f64; 4],
        pen: Pen,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for (i, s) in strokes.iter().enumerate() {
        if excluded.contains(&i) {
            continue;
        }
        if let Some(b) = ocr_candidate(s, max_seg) {
            cands.push(Cand { idx: i, bb: b, pen: s.pen });
        }
    }
    if cands.is_empty() {
        return (Vec::new(), report);
    }

    // ---- phase 2: join touching strokes into glyphs ----
    // Two strokes of the same pen belong to one glyph when they touch: their gap is under a
    // twentieth of the larger one's size (at least 0.3 pt, for rounded coordinates).  Plot
    // fonts set their letters close (two letters' feet can be a tenth of the height apart), so
    // the tolerance stays well under that.  The larger size counts up to twice the smaller: a
    // box drawn round a tag (one big loop) must not reach the lettering inside it.
    let cell = max_seg;
    let mut grid: HashMap<(i64, i64, Pen), Vec<usize>> = HashMap::new();
    for (ci, c) in cands.iter().enumerate() {
        let gx = (((c.bb[0] + c.bb[2]) / 2.0) / cell).floor() as i64;
        let gy = (((c.bb[1] + c.bb[3]) / 2.0) / cell).floor() as i64;
        grid.entry((gx, gy, c.pen)).or_default().push(ci);
    }
    let size = |b: &[f64; 4]| (b[2] - b[0]).max(b[3] - b[1]);
    let mut visited = vec![false; cands.len()];
    // each glyph: its strokes (indices into `strokes`), box and pen
    let mut glyphs: Vec<(Vec<usize>, [f64; 4], Pen)> = Vec::new();
    for (ci, first) in cands.iter().enumerate() {
        match visited.get_mut(ci) {
            Some(v) if !*v => *v = true,
            _ => continue,
        }
        let mut members = vec![first.idx];
        let mut queue = vec![ci];
        let mut gbb = first.bb;
        while let Some(cur) = queue.pop() {
            let Some(cc) = cands.get(cur) else { continue };
            let gx = (((cc.bb[0] + cc.bb[2]) / 2.0) / cell).floor() as i64;
            let gy = (((cc.bb[1] + cc.bb[3]) / 2.0) / cell).floor() as i64;
            for dx in -1_i64..=1 {
                for dy in -1_i64..=1 {
                    let Some(ns) = grid.get(&(gx + dx, gy + dy, cc.pen)) else { continue };
                    for &ni in ns {
                        if visited.get(ni).is_none_or(|&v| v) || members.len() > 16 {
                            continue;
                        }
                        let Some(n) = cands.get(ni) else { continue };
                        let (small, large) = (size(&cc.bb).min(size(&n.bb)), size(&cc.bb).max(size(&n.bb)));
                        let tol = (large.min(small * 2.0) * OCR_JOIN_TOLERANCE).max(0.3);
                        if n.bb[0] > cc.bb[2] + tol || n.bb[2] < cc.bb[0] - tol || n.bb[1] > cc.bb[3] + tol || n.bb[3] < cc.bb[1] - tol {
                            continue;
                        }
                        let (Some(a), Some(b)) = (strokes.get(cc.idx), strokes.get(n.idx)) else { continue };
                        if polyline_gap(&a.pts, &b.pts) <= tol {
                            if let Some(v) = visited.get_mut(ni) {
                                *v = true;
                            }
                            members.push(n.idx);
                            queue.push(ni);
                            gbb = [gbb[0].min(n.bb[0]), gbb[1].min(n.bb[1]), gbb[2].max(n.bb[2]), gbb[3].max(n.bb[3])];
                        }
                    }
                }
            }
        }
        let (w, h) = (gbb[2] - gbb[0], gbb[3] - gbb[1]);
        // glyph-sized: not a speck, not taller than lettering gets, a handful of strokes
        if w.max(h) > 0.25 && h < max_seg * 4.0 && w < max_seg * 4.0 && members.len() <= 12 {
            glyphs.push((members, gbb, first.pen));
        }
    }
    report.clusters = glyphs.len();

    // ---- phase 3: match each glyph cluster against templates ----
    // a loop is sampled all the way round (the importer drops its repeated closing point)
    let loops: HashMap<usize, Vec<(f64, f64)>> = glyphs
        .iter()
        .flat_map(|(members, _, _)| members.iter())
        .filter_map(|&i| strokes.get(i).filter(|s| s.closed).map(|s| (i, s.pts.iter().chain(s.pts.first()).copied().collect())))
        .collect();
    let glyph_pts: Vec<Vec<&[(f64, f64)]>> = glyphs
        .iter()
        .map(|(members, _, _)| {
            members.iter().filter_map(|&i| loops.get(&i).map(Vec::as_slice).or_else(|| strokes.get(i).map(|s| s.pts.as_slice()))).collect()
        })
        .collect();
    // the slant of each glyph's row: the median lean of the glyphs of its height beside it
    let leans: Vec<Option<f64>> = glyph_pts.iter().map(|p| glyph_slant(p)).collect();
    let mut by_cell: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (gi, (_, bb, _)) in glyphs.iter().enumerate() {
        by_cell.entry((((bb[0] + bb[2]) / 2.0 / cell).floor() as i64, ((bb[1] + bb[3]) / 2.0 / cell).floor() as i64)).or_default().push(gi);
    }
    let row_slant = |gi: usize| -> f64 {
        let Some((_, bb, pen)) = glyphs.get(gi) else { return 0.0 };
        let (cx, cy, h) = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0, (bb[3] - bb[1]).max(0.5));
        let reach = ((h * 4.0 / cell).ceil() as i64).clamp(1, 4);
        let (gx, gy) = ((cx / cell).floor() as i64, (cy / cell).floor() as i64);
        // (height, lean) of the glyphs beside it
        let mut near: Vec<(f64, Option<f64>)> = Vec::new();
        for dx in -reach..=reach {
            for dy in -1..=1 {
                for &gj in by_cell.get(&(gx + dx, gy + dy)).map(Vec::as_slice).unwrap_or_default() {
                    let Some((_, b2, p2)) = glyphs.get(gj) else { continue };
                    let h2 = b2[3] - b2[1];
                    if p2 == pen
                        && ((b2[1] + b2[3]) / 2.0 - cy).abs() <= h * 0.5
                        && ((b2[0] + b2[2]) / 2.0 - cx).abs() <= h * 4.0
                        && (0.6..=1.6).contains(&(h2 / h))
                    {
                        near.push((h2, leans.get(gj).copied().flatten()));
                    }
                }
            }
        }
        // only the glyphs of the row's letter height vote: a slash or a number sign drawn
        // taller than the letters leans by design
        let mut heights: Vec<f64> = near.iter().map(|n| n.0).collect();
        heights.sort_by(f64::total_cmp);
        let Some(&hm) = heights.get(heights.len() / 2) else { return 0.0 };
        let mut votes: Vec<f64> = near.iter().filter(|n| (hm * 0.85..=hm * 1.15).contains(&n.0)).filter_map(|n| n.1).collect();
        votes.sort_by(f64::total_cmp);
        votes.get(votes.len() / 2).copied().unwrap_or(0.0)
    };
    // the letter height of each glyph's row: the median height of the taller glyphs beside it
    // whose letter band holds its centre (none for a glyph standing alone)
    let row_height = |gi: usize| -> Option<f64> {
        let (_, bb, pen) = glyphs.get(gi)?;
        let (cx, cy, h) = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0, bb[3] - bb[1]);
        let (gx, gy) = ((cx / cell).floor() as i64, (cy / cell).floor() as i64);
        let mut near: Vec<f64> = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &gj in by_cell.get(&(gx + dx, gy + dy)).map(Vec::as_slice).unwrap_or_default() {
                    let Some((_, b2, p2)) = glyphs.get(gj) else { continue };
                    let h2 = b2[3] - b2[1];
                    // (a lone straight stroke beside it may be a leader or a wall end, not a letter)
                    let line = glyph_pts.get(gj).is_some_and(|p| p.len() == 1 && p.first().is_some_and(|s| s.len() <= 2));
                    if gj != gi
                        && !line
                        && p2 == pen
                        && h2 >= h * 0.6
                        && h2 > 0.5
                        && cy >= b2[1] - h2 * 0.25
                        && cy <= b2[3] + h2 * 0.25
                        && ((b2[0] + b2[2]) / 2.0 - cx).abs() <= h2 * 3.0
                    {
                        near.push(h2);
                    }
                }
            }
        }
        near.sort_by(f64::total_cmp);
        near.get(near.len() / 2).copied()
    };
    let mut matched: Vec<MatchedGlyph> = Vec::new();
    for (gi, ((members, bb, pen), pts)) in glyphs.iter().zip(&glyph_pts).enumerate() {
        let slant = row_slant(gi);
        if let Some(GlyphRead { mut ch, dist, mut alt }) = read_glyph(pts, slant, &templates) {
            let straight = pts.len() == 1 && pts.first().is_some_and(|p| p.len() <= 2);
            let ring = (members.len() == 1 && members.first().and_then(|&i| strokes.get(i)).is_some_and(|s| s.closed))
                || members.iter().any(|&i| strokes.get(i).is_some_and(|s| s.curved_loop));
            // a lone upright stroke is an I (or a 1) at full letter height and a dot or an
            // apostrophe below it; a speck a fraction of the letters' height is a dot whatever
            // its shape (plot fonts draw a period as a tiny ring or diamond)
            let (w, h) = (bb[2] - bb[0], bb[3] - bb[1]);
            // upright, or leaning with its row (an oblique I reads as a slash)
            let lean = pts.first().and_then(|p| Some((p.first()?, p.last()?))).map_or(f64::MAX, |(a, b)| {
                let dy = b.1 - a.1;
                if dy.abs() < 1e-9 { f64::MAX } else { (b.0 - a.0) / dy }
            });
            let along_row = straight && (lean - slant).abs() <= 0.1;
            let row = row_height(gi);
            if row.is_none() && straight && lean.abs() <= 0.1 && matches!(ch, '.' | '\'' | '/' | 'I') {
                // alone, an upright stroke is a 1 (a circuit number) if it is anything (a mark's
                // letter height and clear space decide, see `keep_lone_marks`); a lone stroke's
                // "row" slant is its own lean, so a leader would pass as an oblique 1
                (ch, alt) = ('1', Some('I'));
            }
            if let Some(row_h) = row {
                if along_row && matches!(ch, '.' | '\'' | 'I' | '1' | '/') {
                    (ch, alt) = if h >= row_h * 0.75 { ('I', Some('1')) } else { ('.', None) };
                } else if w.max(h) <= row_h * 0.3 && ch != '-' {
                    // a speck with a tail (taller than wide, not one straight tick) is a comma
                    (ch, alt) = (if !straight && h > w * 1.8 { ',' } else { '.' }, None);
                }
            }
            // spacing is measured upright: x with the row's slant taken out about the glyph's
            // bottom (where an oblique letter stands)
            let mut bb = *bb;
            if slant.abs() >= 0.05 {
                let xs = pts.iter().flat_map(|p| p.iter()).map(|&(x, y)| x - slant * (y - bb[1]));
                let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
                if x0 <= x1 {
                    bb[0] = x0;
                    bb[2] = x1;
                }
            }
            matched.push(MatchedGlyph {
                ch,
                alt,
                bb,
                indices: members.clone(),
                pen: *pen,
                confidence: match_confidence(dist),
                straight,
                ring,
                slant,
            });
        }
    }
    report.glyphs = matched.len();
    if matched.is_empty() {
        return (Vec::new(), report);
    }

    // ---- phase 4: lines by baseline and height, then texts by spacing ----
    // Letters and digits (full-height glyphs) build the lines, left to right; dashes, dots and
    // quotes join the line whose letter band holds them.
    matched.sort_by(|a, b| a.bb[0].total_cmp(&b.bb[0]));
    struct TextLine {
        pen: Pen,
        bottom: f64,
        height: f64,
        x1: f64,
        slant: f64,
        members: Vec<usize>,
    }
    let mut lines: Vec<TextLine> = Vec::new();
    let mut small: Vec<usize> = Vec::new();
    for (gi, g) in matched.iter().enumerate() {
        if small_glyph(g.ch) {
            small.push(gi);
            continue;
        }
        let h = g.bb[3] - g.bb[1];
        let best = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                let gap = g.bb[0] - l.x1;
                l.pen == g.pen
                    && (g.bb[1] - l.bottom).abs() <= l.height * 0.3
                    && (0.7..=1.45).contains(&(h / l.height.max(1e-6)))
                    // room for a dash or a dot between (they join the line afterwards)
                    && gap <= l.height * 2.0
                    && gap >= -l.height * 0.3
            })
            .min_by(|a, b| (g.bb[0] - a.1.x1).total_cmp(&(g.bb[0] - b.1.x1)))
            .map(|(li, _)| li);
        match best.and_then(|li| lines.get_mut(li)) {
            Some(l) => {
                l.x1 = l.x1.max(g.bb[2]);
                l.members.push(gi);
            }
            None => lines.push(TextLine { pen: g.pen, bottom: g.bb[1], height: h, x1: g.bb[2], slant: g.slant, members: vec![gi] }),
        }
    }
    for gi in small {
        let Some(g) = matched.get(gi) else { continue };
        // a dash or a quote stands above the baseline: on an oblique row it shifts with the slant
        let on_oblique_line = |l: &TextLine| {
            let x0 = l.members.first().and_then(|&m| matched.get(m)).map_or(f64::MAX, |m| m.bb[0]);
            let rise = g.bb[1] - l.bottom;
            l.pen == g.pen
                && l.slant.abs() >= 0.05
                && (0.0..=l.height * 1.25).contains(&rise)
                && g.bb[0] - l.slant * rise >= x0 - l.height * 1.2
                && g.bb[2] - l.slant * rise <= l.x1 + l.height * 1.2
        };
        let shift = lines.iter().find(|l| on_oblique_line(l)).map(|l| l.slant * (g.bb[1] - l.bottom));
        if let Some(shift) = shift
            && let Some(g) = matched.get_mut(gi)
        {
            g.bb[0] -= shift;
            g.bb[2] -= shift;
        }
        let Some(g) = matched.get(gi) else { continue };
        let cy = (g.bb[1] + g.bb[3]) / 2.0;
        let line = lines.iter_mut().find(|l| {
            let x0 = l.members.first().and_then(|&m| matched.get(m)).map_or(f64::MAX, |m| m.bb[0]);
            l.pen == g.pen
                && cy >= l.bottom - l.height * 0.25
                && cy <= l.bottom + l.height * 1.25
                && (g.bb[3] - g.bb[1]) <= l.height * if tall_glyph(g.ch) { 1.7 } else { 1.1 }
                && g.bb[0] >= x0 - l.height * 1.2
                && g.bb[2] <= l.x1 + l.height * 1.2
        });
        match line {
            Some(l) => {
                l.x1 = l.x1.max(g.bb[2]);
                l.members.push(gi);
            }
            None => {
                lines.push(TextLine { pen: g.pen, bottom: g.bb[1], height: (g.bb[3] - g.bb[1]).max(0.5), x1: g.bb[2], slant: 0.0, members: vec![gi] })
            }
        }
    }

    // lines the punctuation now bridges join: in "(A) #12" the parenthesis and the number sign
    // left a gap between the A and the 12 that only they fill
    let line_x0 = |l: &TextLine| l.members.iter().filter_map(|&m| matched.get(m)).map(|g| g.bb[0]).fold(f64::MAX, f64::min);
    lines.sort_by(|a, b| line_x0(a).total_cmp(&line_x0(b)));
    let mut joined: Vec<TextLine> = Vec::with_capacity(lines.len());
    for l in lines {
        let x0 = line_x0(&l);
        let into = joined.iter_mut().rev().find(|m| {
            let gap = x0 - m.x1;
            m.pen == l.pen
                && (l.bottom - m.bottom).abs() <= m.height * 0.3
                && (0.7..=1.45).contains(&(l.height / m.height.max(1e-6)))
                && gap <= m.height * 2.0
                && gap >= -m.height * 0.3
        });
        match into {
            Some(m) => {
                m.x1 = m.x1.max(l.x1);
                m.members.extend(l.members);
            }
            None => joined.push(l),
        }
    }
    let lines = joined;

    let mut result: Vec<RecoveredText> = Vec::new();
    let mut marks: Vec<RecoveredText> = Vec::new();
    for mut line in lines {
        line.members.sort_by(|&a, &b| {
            let (ax, bx) = (matched.get(a).map_or(0.0, |g| g.bb[0]), matched.get(b).map_or(0.0, |g| g.bb[0]));
            ax.total_cmp(&bx)
        });
        let h = line.height;
        // the line's character width (its median letter), for a width factor's wider spacing
        let mut widths: Vec<f64> = line.members.iter().filter_map(|&m| matched.get(m)).map(|g| g.bb[2] - g.bb[0]).filter(|&w| w >= h * 0.4).collect();
        widths.sort_by(f64::total_cmp);
        let cw = widths.get(widths.len() / 2).copied().unwrap_or(h * 0.67);
        // spacing is measured on nominal boxes: a narrow glyph (an I, a 1, a dot) is given
        // a normal letter's width around its centre
        let span = |g: &MatchedGlyph| -> (f64, f64) {
            // a dot, a comma or a quote sits in a narrow cell of its own
            let nominal = if matches!(g.ch, '.' | ',' | '\'') { cw * 0.3 } else { cw * 0.9 };
            let (cx, w) = ((g.bb[0] + g.bb[2]) / 2.0, (g.bb[2] - g.bb[0]).max(nominal));
            (cx - w / 2.0, cx + w / 2.0)
        };
        // split the line into texts at wide gaps (about two spaces); a narrower gap is a space
        let mut runs: Vec<Vec<&MatchedGlyph>> = Vec::new();
        let mut prev_end = f64::MIN;
        for g in line.members.iter().filter_map(|&m| matched.get(m)) {
            let (x0, x1) = span(g);
            match runs.last_mut() {
                Some(run) if x0 - prev_end <= cw * 1.8 => run.push(g),
                _ => runs.push(vec![g]),
            }
            prev_end = prev_end.max(x1);
        }
        for run in runs {
            // words (split at spaces), each glyph with the character it reads as
            let mut words: Vec<Vec<(&MatchedGlyph, char)>> = Vec::new();
            let mut prev_end = f64::MIN;
            for &g in &run {
                let (x0, x1) = span(g);
                // a tick in the upper half of the letter band is an apostrophe, low a dot (or
                // a comma, read by its tail)
                let ch = match g.ch {
                    '.' | '\'' | ',' if (g.bb[1] + g.bb[3]) / 2.0 > line.bottom + h * 0.5 => '\'',
                    '.' | '\'' => '.',
                    c => c,
                };
                match words.last_mut() {
                    Some(word) if x0 - prev_end <= cw * 0.6 => word.push((g, ch)),
                    _ => words.push(vec![(g, ch)]),
                }
                prev_end = prev_end.max(x1);
            }
            let mut value = String::new();
            for word in &mut words {
                settle_look_alikes(word);
                if !value.is_empty() {
                    value.push(' ');
                }
                value.extend(word.iter().map(|(_, c)| *c));
            }
            // two ticks side by side are a double quote
            let value = value.replace("''", "\"");
            let alnum = value.chars().filter(char::is_ascii_alphanumeric).count();
            let lone = run.len() == 1;
            let confidence = run.iter().map(|g| g.confidence).fold(1.0, f64::min);
            // linework reads as lone glyphs and runs of straight strokes (hatching: "////"); a
            // lone loop is a small circle or box as often as an O, and a row of nothing but
            // loops is a row of circles or boxes ("OOO", or "DDD" for boxes read turned)
            let noise =
                alnum == 0 || run.iter().all(|g| g.straight) || run.iter().all(|g| g.ring) || (lone && run.iter().any(|g| g.confidence < 0.7));
            // a lone letter or digit can still be a mark (a circuit number, a fixture type):
            // it is judged once the page's words give its letter heights
            let mark = noise && lone && alnum == 1 && run.first().is_some_and(|g| lone_mark_shape(g, strokes));
            if noise && !mark {
                report.noise_dropped += 1;
                continue;
            }
            let Some(first) = run.first() else { continue };
            let x0 = run.iter().map(|g| g.bb[0]).fold(f64::MAX, f64::min);
            let y0 = run.iter().filter(|g| !small_glyph(g.ch)).map(|g| g.bb[1]).fold(f64::MAX, f64::min);
            let y0 = if y0 == f64::MAX { first.bb[1] } else { y0 };
            let stroke_indices: HashSet<usize> = run.iter().flat_map(|g| g.indices.iter().copied()).collect();
            let layer = first.indices.first().and_then(|&i| strokes.get(i)).and_then(|s| s.layer.clone());
            let text = RecoveredText {
                at: (x0, y0),
                height: h,
                value,
                stroke_indices,
                pen: line.pen,
                layer,
                unconfirmed: false,
                confidence: Some(confidence),
                review: false,
                rotation: 0.0,
            };
            if mark { marks.push(text) } else { result.push(text) }
        }
    }
    let kept = keep_lone_marks(marks, &result, strokes, max_seg);
    report.noise_dropped += kept.1;
    result.extend(kept.0);
    (result, report)
}

/// A lone glyph that may be a mark rather than linework: a letter or digit of a letter's shape
/// (not a drawn circle, a box as wide as it is tall, or a stroke leaning off the upright).
fn lone_mark_shape(g: &MatchedGlyph, strokes: &[Stroke]) -> bool {
    let (w, h) = (g.bb[2] - g.bb[0], g.bb[3] - g.bb[1]);
    if !g.ch.is_ascii_alphanumeric() || h <= 0.5 || g.confidence < 0.45 {
        return false;
    }
    if g.indices.iter().any(|&i| strokes.get(i).is_some_and(|s| s.curved_loop)) {
        return false;
    }
    // a lettering loop (O, D, 0) is narrower than it is tall; a circle or a box is not
    if g.ring && w > h * 0.8 {
        return false;
    }
    // a lone straight stroke must stand upright (an I or a 1)
    if g.straight && w > h * 0.12 {
        return false;
    }
    true
}

/// Which lone marks are lettering (see [`lone_mark_shape`]): one standing at the letter height
/// of the page's words (in its pen), clear of every other stroke.  A door jamb, a duct tick
/// or a wall end reads like an I but touches the lines it ends; a circuit number or a fixture
/// type stands free.  Returns the kept marks and how many were dropped.
fn keep_lone_marks(marks: Vec<RecoveredText>, words: &[RecoveredText], strokes: &[Stroke], cell: f64) -> (Vec<RecoveredText>, usize) {
    if marks.is_empty() {
        return (marks, 0);
    }
    let heights: Vec<(Pen, f64)> =
        words.iter().filter(|w| w.value.chars().filter(char::is_ascii_alphanumeric).count() >= 2).map(|w| (w.pen, w.height)).collect();
    // the segments of the page near a mark, by grid cell
    let cell = cell.max(1.0);
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<(usize, (f64, f64), (f64, f64))>> = HashMap::new();
    for m in &marks {
        let (k0, k1) = (key(m.at.0 - m.height, m.at.1 - m.height), key(m.at.0 + m.height * 2.0, m.at.1 + m.height * 2.0));
        for gx in k0.0..=k1.0.min(k0.0 + 8) {
            for gy in k0.1..=k1.1.min(k0.1 + 8) {
                grid.entry((gx, gy)).or_default();
            }
        }
    }
    for (si, s) in strokes.iter().enumerate() {
        let closing = if s.closed && s.pts.len() > 2 { s.pts.last().zip(s.pts.first()) } else { None };
        for (&a, &b) in s.pts.iter().zip(s.pts.iter().skip(1)).chain(closing) {
            let (k0, k1) = (key(a.0.min(b.0), a.1.min(b.1)), key(a.0.max(b.0), a.1.max(b.1)));
            if (k1.0 - k0.0 + 1).saturating_mul(k1.1 - k0.1 + 1) > 400 {
                // a long diagonal: only the cells it passes near a mark
                for (&(gx, gy), segs) in grid.iter_mut() {
                    let (x0, y0) = (gx as f64 * cell, gy as f64 * cell);
                    let centre = (x0 + cell / 2.0, y0 + cell / 2.0);
                    if point_segment_gap(centre, a, b) <= cell {
                        segs.push((si, a, b));
                    }
                }
                continue;
            }
            for gx in k0.0..=k1.0 {
                for gy in k0.1..=k1.1 {
                    if let Some(segs) = grid.get_mut(&(gx, gy)) {
                        segs.push((si, a, b));
                    }
                }
            }
        }
    }
    let mut kept = Vec::new();
    let mut dropped = 0;
    for m in marks {
        let at_letter_height = heights.iter().any(|&(pen, h)| pen == m.pen && (m.height / h.max(1e-6) - 1.0).abs() <= 0.15);
        let clear = (m.height * 0.15).max(0.3);
        let free = at_letter_height
            && m.stroke_indices.iter().filter_map(|&i| strokes.get(i)).all(|s| {
                s.pts.iter().zip(s.pts.iter().skip(1)).all(|(&p, &q)| {
                    let (k0, k1) = (key(p.0.min(q.0) - clear, p.1.min(q.1) - clear), key(p.0.max(q.0) + clear, p.1.max(q.1) + clear));
                    (k0.0..=k1.0).all(|gx| {
                        (k0.1..=k1.1).all(|gy| {
                            grid.get(&(gx, gy)).is_none_or(|segs| {
                                segs.iter().all(|&(si, a, b)| m.stroke_indices.contains(&si) || polyline_gap(&[p, q], &[a, b]) > clear)
                            })
                        })
                    })
                })
            });
        if free {
            kept.push(m);
        } else {
            dropped += 1;
        }
    }
    (kept, dropped)
}

// ---------------------------------------------------------------------------------------------
// Fix 4: Filter opaque fills that the PDF render doesn't show
// ---------------------------------------------------------------------------------------------

/// Check if a fill is likely a false opaque fill (artifact) rather than real content visible
/// in the PDF. False fills are typically:
/// - Large solid fills covering significant portions of the sheet (yellow/white boxes)
/// - Fills whose color doesn't match nearby content (e.g., bright yellow on a line drawing)
/// - Fills behind text that are not wipeouts
fn is_artifact_fill(f: &Fill, sheet: (f64, f64), texts: &[TextOut], strokes: &[Stroke]) -> bool {
    let b = match bbox(&f.loops) {
        Some(b) => b,
        None => return false,
    };
    let (sw, sh) = (sheet.0 * 72.0, sheet.1 * 72.0);
    let fill_area = (b[2] - b[0]) * (b[3] - b[1]);
    let sheet_area = sw * sh;
    if sheet_area < 1.0 {
        return false;
    }
    // fills covering >50% of the sheet are almost certainly artifacts
    if fill_area > sheet_area * 0.5 {
        return true;
    }
    // bright yellow, orange, or other non-standard fills on construction docs that cover large areas
    // are typically "highlighted" artifacts from PDF redline tools, not original drawing content
    let [r, g, _b] = f.pen.rgb;
    let is_bright_yellow = r > 200 && g > 200 && _b < 100;
    let is_bright_orange = r > 200 && g > 100 && g < 200 && _b < 100;
    let is_artifact_color = is_bright_yellow || is_bright_orange;
    if is_artifact_color && fill_area > sheet_area * 0.01 {
        // large bright colored fills without nearby matching-color content are artifacts
        let has_matching_content = strokes.iter().any(|s| s.pen.rgb == f.pen.rgb) || texts.iter().any(|t| t.pen.rgb == f.pen.rgb);
        if !has_matching_content {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Mask reduction: merge overlapping/adjacent wipeouts, drop those that mask nothing visible
// ---------------------------------------------------------------------------------------------

/// Reduce the mask count by merging spatially overlapping masks into combined bounding-box
/// masks, and by dropping masks that do not overlap any visible non-white content.
fn reduce_masks(masks: Vec<Fill>, strokes: &[Stroke], fills: &[Fill], texts: &[TextOut], sheet: (f64, f64)) -> Vec<Fill> {
    if masks.is_empty() {
        return masks;
    }
    let (sw, sh) = (sheet.0 * 72.0, sheet.1 * 72.0);

    // compute a bounding box for each mask
    struct MaskInfo {
        bb: [f64; 4],
        order: usize,
    }
    let mut infos: Vec<MaskInfo> = Vec::new();
    for m in &masks {
        if let Some(bb) = bbox(&m.loops) {
            infos.push(MaskInfo { bb, order: m.order });
        }
    }
    if infos.is_empty() {
        return Vec::new();
    }

    // grid-based spatial grouping: merge masks whose bounding boxes overlap or are close
    let cell_size = sw.min(sh) * 0.05; // 5% of sheet for grouping
    let cell_size = cell_size.max(20.0);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, info) in infos.iter().enumerate() {
        let gx0 = (info.bb[0] / cell_size).floor() as i64;
        let gx1 = (info.bb[2] / cell_size).floor() as i64;
        let gy0 = (info.bb[1] / cell_size).floor() as i64;
        let gy1 = (info.bb[3] / cell_size).floor() as i64;
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                grid.entry((gx, gy)).or_default().push(i);
            }
        }
    }

    // union-find to group overlapping masks
    let mut parent: Vec<usize> = (0..infos.len()).collect();
    let find = |parent: &mut [usize], mut x: usize| -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    };
    let pad = cell_size * 0.5; // generous tolerance for adjacency (masks behind nearby tags)
    for members in grid.values() {
        for (mi, &a) in members.iter().enumerate() {
            for &b in &members[mi + 1..] {
                let (ba, bb_) = (&infos[a].bb, &infos[b].bb);
                // overlapping or adjacent (with padding)
                if ba[0] <= bb_[2] + pad && ba[2] >= bb_[0] - pad && ba[1] <= bb_[3] + pad && ba[3] >= bb_[1] - pad {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    if ra != rb {
                        parent[rb] = ra;
                    }
                }
            }
        }
    }

    // collect groups and merge each into a single bounding-box mask
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..infos.len() {
        groups.entry(find(&mut parent, i)).or_default().push(i);
    }

    // determine which areas have visible non-white content (strokes, fills, texts)
    // to decide whether a merged mask is worth keeping
    let mut content_bbs: Vec<[f64; 4]> = Vec::new();
    for s in strokes {
        if let Some(b) = bbox(std::slice::from_ref(&s.pts)) {
            content_bbs.push(b);
        }
    }
    for f in fills {
        if let Some(b) = bbox(&f.loops) {
            content_bbs.push(b);
        }
    }
    for t in texts {
        let h = t.height.max(1.0);
        let w = h * t.value.len().max(1) as f64 * 0.6;
        content_bbs.push([t.at.0, t.at.1, t.at.0 + w, t.at.1 + h]);
    }

    // generous proximity check: a mask near visible content is worth keeping (it may mask
    // behind text or tags that are adjacent, not exactly overlapping)
    let content_pad = sw.min(sh) * 0.05; // 5% of sheet size
    let overlaps_content = |bb: &[f64; 4]| -> bool {
        let expanded = [bb[0] - content_pad, bb[1] - content_pad, bb[2] + content_pad, bb[3] + content_pad];
        content_bbs.iter().any(|cb| cb[0] < expanded[2] && cb[2] > expanded[0] && cb[1] < expanded[3] && cb[3] > expanded[1])
    };

    let mut result: Vec<Fill> = Vec::new();
    for (_, members) in groups {
        let mut merged_bb = infos[members[0]].bb;
        let mut max_order = infos[members[0]].order;
        for &mi in &members[1..] {
            let b = &infos[mi].bb;
            merged_bb = [merged_bb[0].min(b[0]), merged_bb[1].min(b[1]), merged_bb[2].max(b[2]), merged_bb[3].max(b[3])];
            max_order = max_order.max(infos[mi].order);
        }

        // drop only large merged groups of masks that don't overlap any visible content:
        // a cluster of 10+ tiny masks far from anything useful is an artefact.
        // Singleton or small groups are kept (a single white fill behind a tag is legitimate).
        if members.len() >= 10 && !overlaps_content(&merged_bb) {
            continue;
        }

        // emit one merged mask for the group
        let loop_pts = vec![(merged_bb[0], merged_bb[1]), (merged_bb[2], merged_bb[1]), (merged_bb[2], merged_bb[3]), (merged_bb[0], merged_bb[3])];
        result.push(Fill { loops: vec![loop_pts], pen: Pen { rgb: [255, 255, 255], lw: 0, transp: 0 }, order: max_order, layer: None });
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Fix 5: Detect repeated symbol clusters and create blocks with attributes
// ---------------------------------------------------------------------------------------------

/// A group of entities at one location that appears multiple times on the sheet.
struct SymbolInstance {
    /// Center point.
    center: (f64, f64),
    /// Stroke indices that form this symbol.
    stroke_indices: Vec<usize>,
    /// Fill indices.
    fill_indices: Vec<usize>,
    /// Nearby text (potential attribute tag).
    nearby_text: Option<String>,
    nearby_text_idx: Option<usize>,
    /// Where the tag is printed (page coordinates) and its height (points): the TAG attribute
    /// goes there, so the tag stays where it was read and the circuit printed under it pairs.
    nearby_text_at: Option<((f64, f64), f64)>,
    /// The tag is lettering the OCR read from strokes: its index in the recovered texts.
    ocr_idx: Option<usize>,
    /// The device-type tag from nearby hidden text (e.g. "A", "G", "OS").
    device_type: Option<String>,
}

/// A short mark that may name the device beside it: real PDF text, or lettering the OCR read
/// from strokes.
struct TagAnchor<'a> {
    value: &'a str,
    at: (f64, f64),
    height: f64,
    /// Index in the page's texts (real text).
    text_idx: Option<usize>,
    /// Index in the recovered texts (OCR lettering).
    ocr_idx: Option<usize>,
}

/// Device-tag candidates among real texts: short marks (common on E-sheets: A, C, DH, E, EF,
/// G, J, L4, OS, REC, XA, etc.), hidden or visible.
fn is_device_tag(v: &str) -> bool {
    let t = v.trim();
    (1..=6).contains(&t.len())
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && t.chars().any(|c| c.is_ascii_alphabetic())
        // not a scale note or sheet number
        && !t.contains('=') && !t.contains('"') && !t.contains('\'')
}

/// Device marks the OCR read from strokes.  Stricter than real text, since a misread glyph
/// costs nothing to make: 1-3 capitals and up to 2 digits, never a lone I, O or X (a short
/// line, a circle or a crossing read as a letter).
fn is_ocr_device_tag(v: &str) -> bool {
    let t = v.trim();
    let letters = t.bytes().take_while(u8::is_ascii_uppercase).count();
    (1..=3).contains(&letters) && t.len() - letters <= 2 && t.bytes().skip(letters).all(|b| b.is_ascii_digit()) && !matches!(t, "I" | "O" | "X")
}

/// A closed loop: a symbol's circle, box or triangle.
fn is_outline(s: &Stroke) -> bool {
    s.closed
        || s.pts.len() >= 4
            && match (s.pts.first(), s.pts.last()) {
                (Some(a), Some(b)) => (a.0 - b.0).hypot(a.1 - b.1) < 0.05,
                _ => false,
            }
}

/// Where a real text sits, sheet points (rotation ignored: tags and their neighbours read
/// left to right).
fn text_box(t: &TextOut) -> [f64; 4] {
    [t.at.0.min(t.end.0), t.at.1.min(t.end.1), t.at.0.max(t.end.0), t.at.1.max(t.end.1) + t.height]
}

/// Lettering the OCR read that stands alone, as a device mark does: no other text box within
/// one letter height of `boxes[i]`.  A letter the OCR split off a word sits right beside the
/// rest of it.
fn stands_alone(i: usize, height: f64, boxes: &[[f64; 4]]) -> bool {
    let Some(b) = boxes.get(i) else { return false };
    let gap = height.max(1.0);
    boxes.iter().enumerate().all(|(k, o)| k == i || !(o[0] <= b[2] + gap && o[2] >= b[0] - gap && o[1] <= b[3] + gap && o[3] >= b[1] - gap))
}

/// Smallest and largest symbol on a sheet, in points.
fn symbol_size_range(sheet: (f64, f64)) -> (f64, f64) {
    let short = (sheet.0 * 72.0).min(sheet.1 * 72.0);
    (short * 0.002, short * 0.06)
}

/// Group symbol instances by device type (in tag order); each type with ≥2 instances becomes
/// a block.  Within a group, instances keep their order (the first one draws the block).
fn group_symbols(instances: Vec<SymbolInstance>) -> Vec<Vec<SymbolInstance>> {
    let mut groups: BTreeMap<String, Vec<SymbolInstance>> = BTreeMap::new();
    for inst in instances {
        let key = inst.device_type.clone().unwrap_or_else(|| "UNKNOWN".to_string());
        groups.entry(key).or_default().push(inst);
    }
    groups.into_values().filter(|g| g.len() >= 2).collect()
}

/// Find device symbols: for each tag anchor, the compact cluster of short strokes around it
/// (none of them in `excluded`: lettering, or symbols already found) is the symbol.
fn find_symbol_instances(
    strokes: &[Stroke],
    fills: &[Fill],
    anchors: &[TagAnchor],
    sheet: (f64, f64),
    excluded: &HashSet<usize>,
    need_outline: bool,
) -> Vec<SymbolInstance> {
    let (min_size, max_size) = symbol_size_range(sheet);

    // Build a spatial grid of short strokes not excluded
    let cell = max_size * 1.5;
    let mut stroke_grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in strokes.iter().enumerate() {
        if excluded.contains(&i) {
            continue;
        }
        let seg_len: f64 = s.pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum();
        if seg_len > max_size * 3.0 {
            continue;
        }
        if let Some(b) = bbox(std::slice::from_ref(&s.pts)) {
            let gx = ((b[0] + b[2]) / 2.0 / cell).floor() as i64;
            let gy = ((b[1] + b[3]) / 2.0 / cell).floor() as i64;
            stroke_grid.entry((gx, gy)).or_default().push(i);
        }
    }
    // Fill grid
    let mut fill_grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, f) in fills.iter().enumerate() {
        if let Some(b) = bbox(&f.loops) {
            let gx = ((b[0] + b[2]) / 2.0 / cell).floor() as i64;
            let gy = ((b[1] + b[3]) / 2.0 / cell).floor() as i64;
            fill_grid.entry((gx, gy)).or_default().push(i);
        }
    }

    let mut used_strokes: HashSet<usize> = HashSet::new();
    let mut instances: Vec<SymbolInstance> = Vec::new();

    // For each device tag, find the compact stroke cluster around it
    for t in anchors {
        let v = t.value.trim();
        let tx = t.at.0;
        let ty = t.at.1;
        let search_r = t.height.max(5.0) * 4.0; // search radius: ~4× the text height

        // find short strokes near this tag
        let gx0 = (((tx - search_r) / cell).floor() as i64) - 1;
        let gx1 = (((tx + search_r) / cell).floor() as i64) + 1;
        let gy0 = (((ty - search_r) / cell).floor() as i64) - 1;
        let gy1 = (((ty + search_r) / cell).floor() as i64) + 1;

        let mut nearby: Vec<usize> = Vec::new();
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                if let Some(members) = stroke_grid.get(&(gx, gy)) {
                    for &si in members {
                        if used_strokes.contains(&si) {
                            continue;
                        }
                        let s = &strokes[si];
                        if let Some(b) = bbox(std::slice::from_ref(&s.pts)) {
                            let sc = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                            let dist = (sc.0 - tx).hypot(sc.1 - ty);
                            if dist < search_r {
                                nearby.push(si);
                            }
                        }
                    }
                }
            }
        }
        if nearby.len() < 2 {
            continue;
        }

        // flood-fill from the closest stroke to the tag to build a compact cluster
        nearby.sort_by(|&a, &b| {
            let ab = bbox(std::slice::from_ref(&strokes[a].pts)).unwrap_or([0.0; 4]);
            let bb = bbox(std::slice::from_ref(&strokes[b].pts)).unwrap_or([0.0; 4]);
            let da = ((ab[0] + ab[2]) / 2.0 - tx).hypot((ab[1] + ab[3]) / 2.0 - ty);
            let db = ((bb[0] + bb[2]) / 2.0 - tx).hypot((bb[1] + bb[3]) / 2.0 - ty);
            da.total_cmp(&db)
        });

        let seed = nearby[0];
        let seed_b = match bbox(std::slice::from_ref(&strokes[seed].pts)) {
            Some(b) => b,
            None => continue,
        };
        let mut cluster = vec![seed];
        let mut bounds = seed_b;
        let mut visited = HashSet::from([seed]);

        // greedily add nearby strokes that keep the cluster compact
        for &si in &nearby {
            if visited.contains(&si) {
                continue;
            }
            let Some(sb) = bbox(std::slice::from_ref(&strokes[si].pts)) else { continue };
            let trial = [bounds[0].min(sb[0]), bounds[1].min(sb[1]), bounds[2].max(sb[2]), bounds[3].max(sb[3])];
            let tw = trial[2] - trial[0];
            let th = trial[3] - trial[1];
            if tw <= max_size && th <= max_size {
                // check the stroke is close to the cluster
                let sc = ((sb[0] + sb[2]) / 2.0, (sb[1] + sb[3]) / 2.0);
                let cc = ((bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0);
                if (sc.0 - cc.0).hypot(sc.1 - cc.1) < max_size {
                    visited.insert(si);
                    cluster.push(si);
                    bounds = trial;
                }
            }
        }

        let w = bounds[2] - bounds[0];
        let h = bounds[3] - bounds[1];
        if cluster.len() < 2 || w < min_size || h < min_size {
            continue;
        }
        // Leftover lettering is open strokes; a symbol drawn round a closed outline is not.
        if need_outline && !cluster.iter().filter_map(|&si| strokes.get(si)).any(is_outline) {
            continue;
        }

        // collect fills in the cluster area
        let center = ((bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0);
        let cgx = (center.0 / cell).floor() as i64;
        let cgy = (center.1 / cell).floor() as i64;
        let mut cluster_fills = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(neighbors) = fill_grid.get(&(cgx + dx, cgy + dy)) {
                    for &fi in neighbors {
                        if let Some(fb) = bbox(&fills[fi].loops)
                            && fb[0] >= bounds[0] - 2.0
                            && fb[1] >= bounds[1] - 2.0
                            && fb[2] <= bounds[2] + 2.0
                            && fb[3] <= bounds[3] + 2.0
                        {
                            cluster_fills.push(fi);
                        }
                    }
                }
            }
        }

        // extract the device type from the tag text (first alphabetic chars)
        let dev_type: String = v.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();

        for &si in &cluster {
            used_strokes.insert(si);
        }

        instances.push(SymbolInstance {
            center,
            stroke_indices: cluster,
            fill_indices: cluster_fills,
            nearby_text: Some(v.to_string()),
            nearby_text_idx: t.text_idx,
            nearby_text_at: (t.at.0.is_finite() && t.at.1.is_finite() && t.height.is_finite() && t.height > 0.0).then_some((t.at, t.height)),
            ocr_idx: t.ocr_idx,
            device_type: if dev_type.is_empty() { None } else { Some(dev_type) },
        });
    }
    instances
}

// ---------------------------------------------------------------------------------------------
// The whole page
// ---------------------------------------------------------------------------------------------

/// Read page `opts.page` as a drawing.
pub fn read(bytes: &[u8], opts: &PdfOptions) -> Result<PdfImport> {
    let doc = load_cached(bytes)?;
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
    // The page's content streams in order. One that is missing or whose data doesn't decode is
    // counted: its drawing is not read (the raw bytes go through as the library's own
    // page-content reader would pass them).
    let content_ids = doc.get_page_contents(page_id);
    let mut unreadable_streams = 0usize;
    let mut content = Vec::new();
    for id in &content_ids {
        let Some(st) = doc.get_object(*id).ok().and_then(|o| o.as_stream().ok()) else {
            unreadable_streams += 1;
            continue;
        };
        // The library's decoder hands back what it got from damaged data as if it were whole; a
        // plain Flate stream is decoded strictly here so damage shows.
        let plain_flate = st.filters().is_ok_and(|f| f == [b"FlateDecode".as_slice()]) && st.dict.get(b"DecodeParms").is_err();
        let strict = plain_flate.then(|| miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&st.content, MAX_STREAM_BYTES));
        match strict {
            Some(Ok(data)) => content.extend_from_slice(&data),
            Some(Err(_)) | None => match st.decompressed_content_with_limit(MAX_STREAM_BYTES) {
                Ok(data) => {
                    unreadable_streams += usize::from(strict.is_some());
                    content.extend_from_slice(&data);
                }
                Err(_) => {
                    unreadable_streams += 1;
                    content.extend_from_slice(&st.content);
                }
            },
        }
        content.push(b'\n');
    }
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

    // scale: the option, else the sheet's scale note (enhanced detection with title block
    // preference — Fix 1: detect per viewport/drawing title)
    // "DO NOT SCALE" sheets stay at sheet size (scale=1) but carry the scale and note in the
    // manifest for registration purposes (MUST 6)
    // Initial scale detection — may be overridden after SHX text recovery if a DO NOT SCALE
    // note only exists in stroked (hidden) text.
    let do_not_scale_visible = if opts.recover_text { has_do_not_scale_note(&out.texts) } else { false };
    let best_note = if opts.scale.is_some() {
        None
    } else if opts.recover_text {
        detect_scale_note(&out.texts, sheet)
    } else {
        // old algorithm: most common scale note (no title-block priority)
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
        notes_count.into_iter().max_by(|a, b| a.1.1.cmp(&b.1.1).then(b.0.cmp(&a.0))).map(|(t, (k, _))| (k, t))
    };
    // MUST 5: E-sheets (electrical) always carry a "do not scale / refer to architectural"
    // general note, even when it's only in stroked text we can't read.  The sheet number in
    // the title block tells us: detect it early from visible text.
    let early_sheet_num = {
        let dummy_to_sheet = |x: f64, y: f64| -> (f64, f64) {
            let (x, y) = (x - x0, y - y0);
            let (x, y) = match rotate {
                90 => (y, pw - x),
                180 => (pw - x, ph - y),
                270 => (ph - y, x),
                _ => (x, y),
            };
            (x / 72.0, y / 72.0)
        };
        sheet_number(&out.texts, &dummy_to_sheet, sheet)
    };
    let is_e_sheet = if opts.recover_text {
        early_sheet_num.as_ref().is_some_and(|n| {
            let first = n.chars().next().unwrap_or(' ');
            first == 'E' || first == 'e'
        })
    } else {
        false
    };
    // E-sheets are DO NOT SCALE (reported either way); they stay sheet-size unless the caller
    // asked for an E sheet's scale note to be used (a sheet opened on its own).
    let e_sheet_drawn_to_note = is_e_sheet && !do_not_scale_visible && opts.e_sheet_at_note_scale && best_note.is_some();
    let do_not_scale = do_not_scale_visible || (is_e_sheet && !e_sheet_drawn_to_note);
    let (mut scale, mut scale_source) = match (opts.scale, &best_note) {
        (Some(k), _) if k.is_finite() && k > 0.0 => (k, "option".to_string()),
        (_, Some((k, note))) if e_sheet_drawn_to_note => {
            (*k, format!("scale note {note} (E sheet: DO NOT SCALE, locations approximate; register it to the architectural plans)"))
        }
        (_, Some((k, note))) if !do_not_scale => (*k, format!("scale note {note}")),
        (_, Some((k, note))) => {
            // DO NOT SCALE: keep scale=1 but record the note for registration
            (1.0, format!("DO NOT SCALE (detected {note}, scale {k}x — sheet kept at sheet size)"))
        }
        _ if do_not_scale => (1.0, "DO NOT SCALE (the sheet in inches)".to_string()),
        _ => (1.0, "none (the sheet in inches)".to_string()),
    };

    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    let mut notes = Vec::new();
    if unreadable_streams > 0 {
        notes.push(format!(
            "PDF: {unreadable_streams} of {} content streams on page {} could not be decoded (the file is damaged); the page may be empty or incomplete",
            content_ids.len(),
            opts.page.max(1)
        ));
    }

    // page-box clip: discard anything entirely outside the page (stray content that should not
    // contribute to the drawing extents, e.g. block defs with bad base points in the source PDF)
    let page_box = [x0, y0, x0 + pw, y0 + ph];
    let inside_page = |pts: &[(f64, f64)]| -> bool {
        let mut it = pts.iter();
        let Some(&(x, y)) = it.next() else { return false };
        let b = it.fold([x, y, x, y], |b, &(x, y)| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]);
        b[2] >= page_box[0] && b[0] <= page_box[2] && b[3] >= page_box[1] && b[1] <= page_box[3]
    };

    // duplicates + page clip
    let mut seen: HashSet<(Vec<(i64, i64)>, Pen, Option<String>)> = HashSet::new();
    let mut strokes = Vec::new();
    for s in out.strokes {
        if opts.recover_text && !inside_page(&s.pts) {
            continue;
        }
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
        // page clip: skip fills entirely outside the page box
        if opts.recover_text && !f.loops.iter().any(|l| inside_page(l)) {
            continue;
        }
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
    let merged = if opts.recover_text {
        let before = out.texts.len();
        let m = merge_texts(out.texts);
        if m.len() < before {
            stats.insert("text pieces joined".into(), before - m.len());
        }
        m
    } else {
        out.texts
    };
    let mut texts = Vec::new();
    let mut seen_text: HashSet<(String, i64, i64)> = HashSet::new();
    for t in merged {
        // page clip: skip text outside the page box (only when recover_text is on)
        if opts.recover_text
            && (t.at.0 < page_box[0] - 72.0 || t.at.0 > page_box[2] + 72.0 || t.at.1 < page_box[1] - 72.0 || t.at.1 > page_box[3] + 72.0)
        {
            continue;
        }
        if seen_text.insert((t.value.clone(), (t.at.0 * 10.0).round() as i64, (t.at.1 * 10.0).round() as i64)) {
            texts.push(t);
        } else {
            *stats.entry("duplicates removed".into()).or_default() += 1;
        }
    }

    // save original mask orders for the stitch function before reducing (the reduced masks have
    // different orders which would break the "between" check in stitching)
    let original_white: HashSet<usize> = masks.iter().map(|f| f.order).collect();

    // Reduce masks: merge overlapping/adjacent wipeouts into larger ones, and drop masks that
    // do not overlap any visible non-white content (they mask nothing the viewer can see).
    // Skip when recover_text is off — keep raw masks for pre-merge compatibility.
    if opts.recover_text {
        let masks_before = masks.len();
        masks = reduce_masks(masks, &strokes, &fills, &texts, sheet);
        let masks_merged = masks_before.saturating_sub(masks.len());
        if masks_merged > 0 {
            stats.insert("masks merged/dropped".into(), masks_merged);
        }
    }

    // page clip images: discard raster pieces entirely outside the page box
    let clipped_images: Vec<ImageOut> = out
        .images
        .into_iter()
        .filter(|im| {
            let b = image_box(&im.ctm);
            b[2] >= page_box[0] && b[0] <= page_box[2] && b[3] >= page_box[1] && b[1] <= page_box[3]
        })
        .collect();
    let (images, stitched) = stitch(clipped_images, &original_white);
    if stitched > 0 {
        stats.insert("image pieces stitched".into(), stitched);
    }
    // MUST 4: second pass — spatial grouping of remaining raster tiles into composite images.
    // Many PDF producers tile an image into hundreds/thousands of small strips; the sequential
    // stitch catches runs but not spatially adjacent tiles that had other content between them.
    let images_before_spatial = images.len();
    let images = spatial_stitch(images);
    let spatial_stitched = images_before_spatial.saturating_sub(images.len());
    if spatial_stitched > 0 {
        stats.insert("raster tiles spatially merged".into(), spatial_stitched);
    }

    // Fix 2: detect grid lines for content-class layer assignment
    let grid_lines = if opts.recover_text { detect_grid_lines(&strokes, sheet) } else { HashSet::new() };
    if !grid_lines.is_empty() {
        stats.insert("grid lines detected".into(), grid_lines.len());
    }

    // Fix 3: recover SHX-stroked text, artifact fills, and repeated symbol clusters.
    // All three are text/symbol-oriented post-processing; skip when recover_text is off
    // (downstream geometry analysis that needs every stroke intact).
    let mut ocr_report = OcrReport::default();
    let (
        recovered_texts,
        shx_stroke_indices,
        symbol_groups,
        symbol_stroke_indices,
        symbol_fill_indices,
        symbol_text_indices,
        symbol_ocr_indices,
        recovered_do_not_scale,
    ) = if opts.recover_text {
        // Primary: match invisible text (rendering mode 3) to nearby stroke clusters — 100%
        // accuracy.  Fallback: cluster short strokes and match to overlapping visible text —
        // lower accuracy but catches sheets without hidden text.
        let mut recovered_texts = recover_shx_from_hidden(&strokes, &texts, sheet);
        if recovered_texts.is_empty() {
            recovered_texts = recover_shx_fallback(&strokes, &texts, sheet);
            if !recovered_texts.is_empty() {
                stats.insert("SHX text (fallback) recovered".into(), recovered_texts.len());
            }
        }
        let mut shx_stroke_indices: HashSet<usize> = HashSet::new();
        for rt in &recovered_texts {
            shx_stroke_indices.extend(&rt.stroke_indices);
        }

        // Fix 4: filter artifact fills (large opaque fills not visible in the PDF render)
        let artifact_count = fills.len();
        fills.retain(|f| !is_artifact_fill(f, sheet, &texts, &strokes));
        let artifacts_removed = artifact_count - fills.len();
        if artifacts_removed > 0 {
            stats.insert("artifact fills removed".into(), artifacts_removed);
        }

        // Fix 5: detect repeated symbol clusters, first beside the page's real tag texts.
        // This runs before the OCR: the OCR reads the short lines inside a symbol (the bars of
        // a receptacle, the letter in a circle) as glyphs, and a symbol robbed of them is lost.
        let anchors: Vec<TagAnchor> = texts
            .iter()
            .enumerate()
            .filter(|(_, t)| is_device_tag(&t.value))
            .map(|(i, t)| TagAnchor { value: &t.value, at: t.at, height: t.height, text_idx: Some(i), ocr_idx: None })
            .collect();
        let found = group_symbols(find_symbol_instances(&strokes, &fills, &anchors, sheet, &shx_stroke_indices, false));
        let mut instances: Vec<SymbolInstance> = found.into_iter().flatten().collect();
        let mut taken: HashSet<usize> = shx_stroke_indices.clone();
        for inst in &instances {
            taken.extend(&inst.stroke_indices);
        }

        // OCR pass: template-based glyph recognition for the strokes not yet claimed (by text
        // or symbols) and not lying on real text: on a sheet with hidden text, for the strokes
        // it doesn't cover; without hidden text, where the lettering is drawn as strokes (the
        // fallback stage recovered some of it, little real text on the page, or far more loose
        // glyph-sized strokes than real characters).  A page lettered in real text has nothing
        // to OCR: its short strokes are symbols and ticks, which the OCR would read as stray
        // letters.
        let mut excluded = taken.clone();
        excluded.extend(strokes_under_text(&strokes, &texts));
        let has_hidden_text = texts.iter().any(|t| t.layer.as_deref() == Some(HIDDEN_TEXT_LAYER));
        let has_shx_evidence = has_hidden_text || !recovered_texts.is_empty();
        let real_chars: usize = texts.iter().map(|t| t.value.trim().chars().count()).sum();
        let max_seg = ocr_max_seg(sheet);
        let loose = strokes.iter().enumerate().filter(|(i, s)| !excluded.contains(i) && ocr_candidate(s, max_seg).is_some()).count();
        let lettering_is_strokes = texts.len() < OCR_REAL_TEXT_LIMIT || loose >= real_chars.saturating_mul(OCR_STROKES_PER_CHAR);
        if has_shx_evidence || lettering_is_strokes {
            let (ocr_texts, report) = recover_shx_ocr(&strokes, sheet, &excluded);
            // What the PDF's hidden text says is written where the OCR read takes that string;
            // a circle and a reading under the confidence floor aren't inserted (their strokes
            // stay linework).
            let (mut ocr_texts, from_hidden, circles, weak) = settle_ocr(ocr_texts, &texts, &strokes, &mut recovered_texts);
            if from_hidden > 0 {
                stats.insert("SHX OCR readings replaced by the hidden text there".into(), from_hidden);
            }
            if circles > 0 {
                stats.insert("SHX OCR circles not read as O".into(), circles);
            }
            if weak > 0 {
                stats.insert("SHX OCR readings under the confidence floor dropped".into(), weak);
            }
            if report.clusters > 0 {
                stats.insert("SHX OCR glyph clusters".into(), report.clusters);
                stats.insert("SHX OCR glyphs matched".into(), report.glyphs);
                stats.insert("SHX OCR lone glyphs dropped".into(), report.noise_dropped);
                if report.turned > 0 {
                    stats.insert("SHX OCR turned texts".into(), report.turned);
                }
            }
            if !ocr_texts.is_empty() {
                stats.insert("SHX text (OCR) recovered".into(), ocr_texts.len());
                let pct = (report.mean_confidence() * 100.0).round().clamp(0.0, 100.0) as usize;
                stats.insert("SHX OCR mean confidence %".into(), pct);
                let strokes_went = if has_hidden_text {
                    format!("their strokes are kept on layer {SHX_SOURCE_LAYER} (off)")
                } else {
                    format!("no hidden text confirms them, so their strokes stay linework and the texts use style {OCR_TEXT_STYLE}")
                };
                notes.push(format!(
                    "SHX OCR: {} text(s) read from {} glyph cluster(s) ({} matched a character, mean confidence {pct}%); {strokes_went}",
                    ocr_texts.len(),
                    report.clusters,
                    report.glyphs
                ));
                for rt in &ocr_texts {
                    taken.extend(&rt.stroke_indices);
                }
                // Then beside the device marks that exist only as stroked lettering (no text
                // object at all): standing alone, away from the symbols already found, and
                // only round a closed outline.
                let (_, max_size) = symbol_size_range(sheet);
                let base = recovered_texts.len();
                // Every text's box: the page's texts, then the recovered ones, then the OCR's.
                let mut boxes: Vec<[f64; 4]> = texts.iter().map(text_box).collect();
                for rt in recovered_texts.iter().chain(&ocr_texts) {
                    let own: Vec<Vec<(f64, f64)>> = rt.stroke_indices.iter().filter_map(|&si| strokes.get(si)).map(|s| s.pts.clone()).collect();
                    boxes.push(bbox(&own).unwrap_or([f64::NAN; 4]));
                }
                let first_ocr = texts.len() + base;
                let anchors: Vec<TagAnchor> = ocr_texts
                    .iter()
                    .enumerate()
                    .filter(|(i, rt)| is_ocr_device_tag(&rt.value) && stands_alone(first_ocr + i, rt.height, &boxes))
                    .filter(|(_, rt)| !instances.iter().any(|inst| (inst.center.0 - rt.at.0).hypot(inst.center.1 - rt.at.1) < max_size))
                    .map(|(i, rt)| TagAnchor { value: &rt.value, at: rt.at, height: rt.height, text_idx: None, ocr_idx: Some(base + i) })
                    .collect();
                let more = find_symbol_instances(&strokes, &fills, &anchors, sheet, &taken, true);
                if !more.is_empty() {
                    stats.insert("symbols tagged by stroked lettering".into(), more.len());
                }
                instances.extend(more);
                // Without hidden text to confirm them, the OCR's words are added but their
                // strokes stay linework: a misread wall corner, jamb or duct tick must not leave
                // the plan (walls, duct and pipe chains read those strokes).  The symbols found
                // above already kept their own lines away from the OCR.  Their style marks them
                // as unconfirmed, so duct tags don't come from them.
                if !has_hidden_text {
                    for t in &mut ocr_texts {
                        t.stroke_indices.clear();
                        t.unconfirmed = true;
                    }
                }
                recovered_texts.extend(ocr_texts);
            }
            ocr_report = report;
        }
        for rt in &recovered_texts {
            shx_stroke_indices.extend(&rt.stroke_indices);
        }
        if !recovered_texts.is_empty() {
            stats.insert("SHX text clusters recovered".into(), recovered_texts.len());
        }
        // Also check recovered text for DO NOT SCALE (MUST 5): the note may only exist in
        // SHX-stroked form, not as real PDF text.
        let recovered_do_not_scale = recovered_texts.iter().any(|rt| {
            let up = rt.value.trim().to_ascii_uppercase();
            up.contains("DO NOT SCALE") || up.contains("NOT TO SCALE")
        });

        // MUST 5: if DO NOT SCALE was found in recovered SHX text but not in visible text, and
        // the scale was set from a scale note, override back to 1.0 (sheet size).
        if recovered_do_not_scale && !do_not_scale_visible && scale > 1.0 {
            let old_scale = scale;
            scale = 1.0;
            scale_source = format!("DO NOT SCALE (recovered from SHX text; detected scale {old_scale}x from note — sheet kept at sheet size)");
            notes.push(format!("DO NOT SCALE note found in recovered SHX text — overriding scale {old_scale}x → 1.0"));
        }

        let symbol_groups = group_symbols(instances);
        let mut symbol_stroke_indices: HashSet<usize> = HashSet::new();
        let mut symbol_fill_indices: HashSet<usize> = HashSet::new();
        let mut symbol_text_indices: HashSet<usize> = HashSet::new();
        let mut symbol_ocr_indices: HashSet<usize> = HashSet::new();
        let total_symbols: usize = symbol_groups.iter().map(|g| g.len()).sum();
        for group in &symbol_groups {
            for inst in group {
                symbol_stroke_indices.extend(&inst.stroke_indices);
                symbol_fill_indices.extend(&inst.fill_indices);
                symbol_text_indices.extend(inst.nearby_text_idx);
                symbol_ocr_indices.extend(inst.ocr_idx);
            }
        }
        if total_symbols > 0 {
            stats.insert("symbol instances detected".into(), total_symbols);
            stats.insert("symbol types".into(), symbol_groups.len());
        }
        (
            recovered_texts,
            shx_stroke_indices,
            symbol_groups,
            symbol_stroke_indices,
            symbol_fill_indices,
            symbol_text_indices,
            symbol_ocr_indices,
            recovered_do_not_scale,
        )
    } else {
        (Vec::new(), HashSet::new(), Vec::new(), HashSet::new(), HashSet::new(), HashSet::new(), HashSet::new(), false)
    };

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
    let mode = if opts.layers == LayerMode::Content {
        LayerMode::Content
    } else if use_ocg {
        LayerMode::Pdf
    } else if opts.layers == LayerMode::Object {
        LayerMode::Object
    } else if opts.layers == LayerMode::Pen {
        LayerMode::Pen
    } else {
        // Pdf mode without OCGs: fall back to Content (classifies strokes into E-GRID,
        // A-WALL, TEXT, E-DEVICE, TITLEBLOCK, HATCH, ANNO, PDF-GEOMETRY) — more useful
        // than pen-based layers for construction-drawing PDFs.
        LayerMode::Content
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
        if kind == "shx_source" {
            layer_set.entry(SHX_SOURCE_LAYER.into()).or_insert((Color::Index(8), Lineweight::Default));
            return (SHX_SOURCE_LAYER.into(), color, lw);
        }
        if kind == "ocr_review" {
            layer_set.entry(OCR_REVIEW_LAYER.into()).or_insert((Color::Index(8), Lineweight::Default));
            return (OCR_REVIEW_LAYER.into(), color, lw);
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
            LayerMode::Content => {
                let name = match kind {
                    "mask" => "PDF-MASK",
                    "image" => "PDF-IMAGES",
                    "text" | "shx_text" => content_class_layer(ContentClass::Text),
                    "fill" => content_class_layer(ContentClass::Hatch),
                    "grid" => content_class_layer(ContentClass::Grid),
                    "wall" => content_class_layer(ContentClass::Wall),
                    "symbol" => content_class_layer(ContentClass::Symbol),
                    "titleblock" => content_class_layer(ContentClass::TitleBlock),
                    "anno" => content_class_layer(ContentClass::Anno),
                    _ => content_class_layer(ContentClass::Geometry),
                }
                .to_string();
                layer_set.entry(name.clone()).or_insert((color, if kind == "line" { lw } else { Lineweight::Default }));
                (name, color, if kind == "line" { lw } else { Lineweight::ByLayer })
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
        let common = Common { layer: layer.into(), color, lineweight: lw, transparency, ..Common::default() };
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
    // small fills merged: (layer, colour, transparency) → (placed layer, colour, weight, outlines)
    let mut small_fills: BTreeMap<(String, [u8; 3], u8), (String, Color, Lineweight, Vec<HatchLoop>)> = BTreeMap::new();
    let small_pt = SMALL_FILL_IN * 72.0;
    let is_small = |l: &[(f64, f64)]| {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in l {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        x1 - x0 <= small_pt && y1 - y0 <= small_pt
    };
    // image files by their pixels (a logo on every sheet is one file)
    let mut image_files: HashMap<u64, String> = HashMap::new();
    let mut image_out: Vec<(String, std::sync::Arc<cadaddy_render::images::Raster>)> = Vec::new();
    // Pre-compute which SHX-consumed wall-weight strokes should be protected.
    // A stroke is protected only when it forms a wall face pair with another stroke
    // AND has no non-consumed counterpart already covering the same line (i.e. it is
    // isolated geometry that would be lost without protection).  This keeps real wall
    // faces on prints where SHX recovery consumes wall linework near lettering, while
    // avoiding duplicate geometry on synthetic tests where the wall strokes survive
    // independently of OCR.
    let protected_shx_indices: std::collections::HashSet<usize> = {
        let min_dim_pts = (sheet.0 * 72.0_f64).min(sheet.1 * 72.0);
        let face_min_len = min_dim_pts * 0.01;
        let wall_min_pts = 1.0_f64;
        let wall_max_pts = (min_dim_pts * 0.025).max(40.0);
        // Represent a stroke as a directed line segment if it is straight enough.
        let seg_of = |st: &Stroke| -> Option<((f64, f64), (f64, f64), f64, f64, f64)> {
            if st.pts.len() < 2 {
                return None;
            }
            let a = st.pts[0];
            let b = *st.pts.last()?;
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            let len = dx.hypot(dy);
            if len < face_min_len {
                return None;
            }
            // Reject non-straight strokes (multi-point with deviation).
            if st.pts.len() > 3 {
                let chord = len;
                for p in &st.pts[1..st.pts.len() - 1] {
                    let cross = ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs();
                    if cross / chord > 2.0 {
                        return None;
                    }
                }
            }
            Some((a, b, dx / len, dy / len, len))
        };
        // Collect SHX-consumed wall-weight candidate segments.
        let mut cands: Vec<(usize, (f64, f64), (f64, f64), f64, f64, f64)> = Vec::new();
        for &idx in &shx_stroke_indices {
            if symbol_stroke_indices.contains(&idx) {
                continue;
            }
            let Some(st) = strokes.get(idx) else { continue };
            if !(st.pen.lw >= 24 && st.pen.rgb.iter().all(|&c| c <= 60)) {
                continue;
            }
            if let Some((a, b, ux, uy, len)) = seg_of(st) {
                cands.push((idx, a, b, ux, uy, len));
            }
        }
        // Collect non-consumed straight segments (the surviving geometry).
        let mut survivors: Vec<((f64, f64), (f64, f64), f64, f64, f64)> = Vec::new();
        for (si, st) in strokes.iter().enumerate() {
            if shx_stroke_indices.contains(&si) || symbol_stroke_indices.contains(&si) {
                continue;
            }
            if let Some(s) = seg_of(st) {
                survivors.push(s);
            }
        }
        // Step 1: find which candidates form wall face pairs with each other.
        let mut in_pair: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for ci in 0..cands.len() {
            let (_, ai, _, uxi, uyi, leni) = cands[ci];
            let nxi = -uyi;
            let nyi = uxi;
            for (cj, &(_, aj, bj, uxj, uyj, _)) in cands.iter().enumerate().skip(ci + 1) {
                if (uxi * uxj + uyi * uyj).abs() < 0.996 {
                    continue;
                }
                let d0 = (aj.0 - ai.0) * nxi + (aj.1 - ai.1) * nyi;
                let d1 = (bj.0 - ai.0) * nxi + (bj.1 - ai.1) * nyi;
                let dist = ((d0 + d1) / 2.0).abs();
                if dist < wall_min_pts || dist > wall_max_pts {
                    continue;
                }
                let proj = |p: (f64, f64)| (p.0 - ai.0) * uxi + (p.1 - ai.1) * uyi;
                let (pb0, pb1) = {
                    let p0 = proj(aj);
                    let p1 = proj(bj);
                    (p0.min(p1), p0.max(p1))
                };
                let overlap = leni.min(pb1) - 0.0_f64.max(pb0);
                if overlap > dist.max(face_min_len) {
                    in_pair.insert(ci);
                    in_pair.insert(cj);
                }
            }
        }
        // Step 2: for each paired candidate, check if a non-consumed survivor already
        // covers it (parallel, collinear within tolerance).  If so it's redundant.
        let collinear_tol = 3.0_f64; // pts: lateral offset tolerance
        let overlap_frac = 0.5_f64; // at least half the length overlaps
        let mut result: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &ci in &in_pair {
            let (idx, ai, _bi, uxi, uyi, leni) = cands[ci];
            let nxi = -uyi;
            let nyi = uxi;
            let has_survivor = survivors.iter().any(|(sa, sb, sux, suy, slen)| {
                if (uxi * sux + uyi * suy).abs() < 0.996 {
                    return false;
                }
                // Lateral distance: must be nearly collinear.
                let d = ((sa.0 - ai.0) * nxi + (sa.1 - ai.1) * nyi).abs();
                if d > collinear_tol {
                    return false;
                }
                // Overlap along direction.
                let proj = |p: (f64, f64)| (p.0 - ai.0) * uxi + (p.1 - ai.1) * uyi;
                let (pb0, pb1) = {
                    let p0 = proj(*sa);
                    let p1 = proj(*sb);
                    (p0.min(p1), p0.max(p1))
                };
                let ov = leni.min(pb1) - 0.0_f64.max(pb0);
                ov > leni * overlap_frac || ov > slen * overlap_frac
            });
            if !has_survivor {
                result.insert(idx);
            }
        }
        result
    };
    for (_, kind, i) in paint {
        match kind {
            0 => {
                let Some(s) = strokes.get(i) else { continue };
                // Fix 5: strokes that became symbol blocks are drawn by the blocks
                if symbol_stroke_indices.contains(&i) {
                    continue;
                }
                // Fix 3: strokes recovered text was read from stay, on their own layer (off),
                // but isolated wall face pairs the recovery took stay on the sheet.
                let kind_hint = if shx_stroke_indices.contains(&i) && !protected_shx_indices.contains(&i) {
                    "shx_source"
                } else if mode == LayerMode::Content {
                    let class = classify_stroke(s, sheet, &grid_lines);
                    match class {
                        ContentClass::Grid => "grid",
                        ContentClass::Wall => "wall",
                        ContentClass::TitleBlock => "titleblock",
                        ContentClass::Anno => "anno",
                        _ => "line",
                    }
                } else {
                    "line"
                };
                let (layer, color, lw) = place(kind_hint, s.pen, &s.layer);
                if kind_hint == "shx_source" {
                    let vertices: Vec<PolyVertex> =
                        s.pts.iter().map(|(x, y)| PolyVertex { p: sp(*x, *y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
                    let kind = match (vertices.as_slice(), s.closed) {
                        ([va, vb], false) => EntityKind::Line(Line { a: Vec3::new(va.p.x, va.p.y, 0.0), b: Vec3::new(vb.p.x, vb.p.y, 0.0) }),
                        _ => EntityKind::LwPolyline(LwPolyline { vertices, closed: s.closed, const_width: 0.0, elevation: 0.0, plinegen: false }),
                    };
                    add(&mut d, layer, color, lw, s.pen.transp, kind);
                    *stats.entry("SHX source strokes (layer off)".into()).or_default() += 1;
                    continue;
                }
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
                // Fix 5: skip fills consumed by symbol block detection
                if symbol_fill_indices.contains(&i) {
                    continue;
                }
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
                if opts.merge_small_fills && f.loops.iter().filter(|l| l.len() >= 3).all(|l| is_small(l)) {
                    let e = small_fills
                        .entry((f.layer.clone().unwrap_or_default(), f.pen.rgb, f.pen.transp))
                        .or_insert_with(|| (layer, color, lw, Vec::new()));
                    e.3.extend(loops.into_iter().map(|mut l| {
                        l.outer = true;
                        l
                    }));
                    *stats.entry("small fills merged".into()).or_default() += 1;
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
                add(&mut d, layer, color, lw, f.pen.transp, EntityKind::Hatch(Box::new(hatch)));
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
                add(&mut d, layer, color, lw, im.transp, EntityKind::Image(Box::new(image)));
                *stats.entry("images".into()).or_default() += 1;
            }
            _ => {
                let Some(t) = texts.get(i) else { continue };
                // Fix 5: skip texts consumed as symbol attributes (but keep hidden text —
                // it is the searchable copy and must not be swallowed by symbol detection).
                // Also keep single-char alphanumeric text: these are grid bubble labels
                // ("A", "B", "1", "2" etc.) that symbol detection may claim erroneously.
                if symbol_text_indices.contains(&i)
                    && t.layer.as_deref() != Some(HIDDEN_TEXT_LAYER)
                    && !(t.value.trim().len() == 1 && t.value.trim().chars().next().is_some_and(|c| c.is_ascii_alphanumeric()))
                {
                    continue;
                }
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
    for ((_, _, transp), (layer, color, lw, loops)) in small_fills {
        for chunk in loops.chunks(MAX_MERGED_LOOPS) {
            let hatch = Hatch {
                pattern: "SOLID".into(),
                solid: true,
                loops: chunk.to_vec(),
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
            add(&mut d, layer.clone(), color, lw, transp, EntityKind::Hatch(Box::new(hatch)));
            *stats.entry("solid fills".into()).or_default() += 1;
        }
    }
    // Fix 3: emit recovered SHX text entities (replacing the strokes that formed them)
    // using glyph-matched characters instead of '?' placeholders
    let mut recovered_count = 0usize;
    for (i, rt) in recovered_texts.iter().enumerate() {
        // skip empty or all-unrecognised clusters (they stay as linework), and device marks
        // that became a symbol's TAG
        if rt.value.is_empty() || symbol_ocr_indices.contains(&i) {
            continue;
        }
        let (layer, color, lw) = if rt.review {
            place("ocr_review", rt.pen, &rt.layer)
        } else {
            place(if mode == LayerMode::Content { "shx_text" } else { "text" }, rt.pen, &rt.layer)
        };
        let p = sp(rt.at.0, rt.at.1);
        let text = Text {
            insert: Vec3::new(p.x, p.y, 0.0),
            align_pt: None,
            height: rt.height / 72.0 * k,
            value: rt.value.clone(),
            rotation: rt.rotation.to_radians(),
            width_factor: 1.0,
            oblique: 0.0,
            style: if rt.unconfirmed { OCR_TEXT_STYLE.into() } else { "Standard".into() },
            halign: HAlign::default(),
            valign: VAlign::default(),
        };
        if rt.unconfirmed && !d.text_styles.iter().any(|st| st.name.eq_ignore_ascii_case(OCR_TEXT_STYLE)) {
            d.text_styles.push(TextStyle { name: OCR_TEXT_STYLE.into(), ..TextStyle::default() });
        }
        add(&mut d, layer, color, lw, rt.pen.transp, EntityKind::Text(text));
        recovered_count += 1;
        if let Some(confidence) = rt.confidence {
            ocr_report.words.push(OcrWord { value: rt.value.clone(), at: (p.x, p.y), confidence });
        }
    }
    if recovered_count > 0 {
        stats.insert("SHX recovered texts".into(), recovered_count);
    }

    // Fix 5: create blocks for repeated symbol clusters
    for (gi, group) in symbol_groups.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let block_name = format!("PDF-SYM-{}", gi + 1);
        // use the first instance as the block definition
        let Some(first) = group.first() else { continue };
        let base_x = first.center.0;
        let base_y = first.center.1;
        let base = sp(base_x, base_y);
        let mut block = Block { name: block_name.clone(), base: Vec3::ZERO, explodable: true, ..Block::new(&block_name) };
        // add the first instance's strokes to the block definition, relative to center
        for &si in &first.stroke_indices {
            if let Some(s) = strokes.get(si) {
                let vertices: Vec<PolyVertex> = s
                    .pts
                    .iter()
                    .map(|(x, y)| {
                        let p = sp(*x, *y);
                        PolyVertex { p: Vec2::new(p.x - base.x, p.y - base.y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }
                    })
                    .collect();
                let color = color_of(s.pen.rgb);
                let lw = Lineweight::Mm100(s.pen.lw);
                let common = Common { layer: "0".into(), color, lineweight: lw, ..Common::default() };
                let h = d.new_handle();
                if let ([va, vb], false) = (vertices.as_slice(), s.closed) {
                    let (a, b) = (va.p, vb.p);
                    block.entities.push(Entity {
                        handle: h,
                        common,
                        kind: EntityKind::Line(Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) }),
                    });
                } else {
                    block.entities.push(Entity {
                        handle: h,
                        common,
                        kind: EntityKind::LwPolyline(LwPolyline { vertices, closed: s.closed, const_width: 0.0, elevation: 0.0, plinegen: false }),
                    });
                }
            }
        }
        d.blocks.insert(block_name.clone(), std::sync::Arc::new(block));
        // insert all instances of this symbol
        for inst in group {
            let c = sp(inst.center.0, inst.center.1);
            let (layer, color, lw) =
                place(if mode == LayerMode::Content { "symbol" } else { "line" }, Pen { rgb: [0, 0, 0], lw: 0, transp: 0 }, &None);
            let mut attribs = Vec::new();
            if let Some(tag) = &inst.nearby_text {
                // Where the tag was printed (an attribute's place is in the drawing's coordinates).
                let (at, height) = match inst.nearby_text_at {
                    Some(((x, y), h)) => (sp(x, y), h / 72.0 * k),
                    None => (Vec2::new(0.0, 0.0), 2.5 / 72.0 * k),
                };
                attribs.push(Attrib {
                    tag: "TAG".into(),
                    text: Text {
                        insert: Vec3::new(at.x, at.y, 0.0),
                        align_pt: None,
                        height,
                        value: tag.clone(),
                        rotation: 0.0,
                        width_factor: 1.0,
                        oblique: 0.0,
                        style: "Standard".into(),
                        halign: HAlign::default(),
                        valign: VAlign::default(),
                    },
                    invisible: false,
                    constant: false,
                    prompt: String::new(),
                    props: AttribProps::default(),
                    multiline: None,
                    modes: AttribModes::default(),
                });
            }
            let insert = Insert {
                block: block_name.clone(),
                insert: Vec3::new(c.x, c.y, 0.0),
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: 0.0,
                attribs,
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            };
            add(&mut d, layer, color, lw, 0, EntityKind::Insert(insert));
            *stats.entry("symbol inserts".into()).or_default() += 1;
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
    for l in d.layers.iter_mut().filter(|l| l.name == HIDDEN_TEXT_LAYER || l.name == SHX_SOURCE_LAYER || l.name == OCR_REVIEW_LAYER) {
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
    // Update the SHX text note: if we recovered some, the note changes
    if !recovered_texts.is_empty() {
        notes.push(format!("{} SHX text cluster(s) recovered from stroked lettering", recovered_texts.len()));
    } else if texts.is_empty() && strokes.len() > 1000 {
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
        if d.add(&Space::Paper(name.clone()), Common { layer: "0".into(), ..Common::default() }, EntityKind::Viewport(Box::new(vp))).is_ok() {
            stats.insert("layout".into(), 1);
        }
        notes.push(format!("layout \"{name}\" shows model space at 1:{k} on the {:.0} x {:.0} in sheet", sheet.0, sheet.1));
    }
    let do_not_scale = do_not_scale_visible || recovered_do_not_scale || is_e_sheet;
    let needs_registration = do_not_scale && best_note.is_some();
    Ok(PdfImport {
        drawing: d,
        sheet,
        scale: k,
        scale_source,
        do_not_scale,
        needs_registration,
        pages: page_total,
        stats,
        notes,
        images: image_out,
        ocr: ocr_report,
    })
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

/// Spatial stitching: group images by proximity and orientation, merge each group into one
/// composite image.  This catches tiles that the sequential stitch missed because non-image
/// content (white fills, strokes) was interleaved between them.
fn spatial_stitch(images: Vec<ImageOut>) -> Vec<ImageOut> {
    if images.len() <= 50 {
        return images; // already within the target — nothing to do
    }

    let unit = |x: f64, y: f64| -> Option<(f64, f64)> {
        let l = x.hypot(y);
        (l > 1e-9).then(|| (x / l, y / l))
    };
    let frame_dir = |m: &M| -> Option<((f64, f64), (f64, f64))> {
        let [a, b, c, d, ..] = m.0;
        let u = unit(a, b)?;
        let v = unit(c, d)?;
        // orthogonal only
        ((u.0 * v.0 + u.1 * v.1).abs() < 1e-4).then_some((u, v))
    };

    // quantize frame direction to group by orientation (8 bits each component)
    let dir_key = |m: &M| -> Option<(i16, i16, i16, i16)> {
        let (u, v) = frame_dir(m)?;
        Some(((u.0 * 100.0).round() as i16, (u.1 * 100.0).round() as i16, (v.0 * 100.0).round() as i16, (v.1 * 100.0).round() as i16))
    };

    // group by orientation
    let mut orient_groups: HashMap<(i16, i16, i16, i16), Vec<usize>> = HashMap::new();
    for (i, im) in images.iter().enumerate() {
        if let Some(dk) = dir_key(&im.ctm) {
            orient_groups.entry(dk).or_default().push(i);
        }
    }

    // within each orientation group, spatial-cluster by proximity using union-find
    let mut parent: Vec<usize> = (0..images.len()).collect();
    let find = |parent: &mut [usize], mut x: usize| -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    };

    for members in orient_groups.values() {
        if members.len() < 2 {
            continue;
        }
        // bounding boxes
        let boxes: Vec<[f64; 4]> = members.iter().map(|&i| image_box(&images[i].ctm)).collect();
        // pad: images that nearly touch belong together
        let avg_size: f64 = boxes.iter().map(|b| (b[2] - b[0]).max(b[3] - b[1])).sum::<f64>() / boxes.len() as f64;
        let pad = avg_size * 0.5;

        // simple O(n²) within each orientation group — usually small per group
        let n = members.len();
        if n > 10000 {
            continue;
        } // safety cap
        for mi in 0..n {
            for mj in (mi + 1)..n {
                let (ba, bb) = (&boxes[mi], &boxes[mj]);
                if ba[0] <= bb[2] + pad && ba[2] >= bb[0] - pad && ba[1] <= bb[3] + pad && ba[3] >= bb[1] - pad {
                    let (ra, rb) = (find(&mut parent, members[mi]), find(&mut parent, members[mj]));
                    if ra != rb {
                        parent[rb] = ra;
                    }
                }
            }
        }
    }

    // collect groups
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..images.len() {
        groups.entry(find(&mut parent, i)).or_default().push(i);
    }

    let mut result: Vec<ImageOut> = Vec::with_capacity(groups.len());
    for (_, members) in groups {
        if members.len() == 1 {
            // avoid moving out of images (we'll reconstruct later)
            result.push(ImageOut {
                ctm: images[members[0]].ctm,
                raster: std::sync::Arc::clone(&images[members[0]].raster),
                layer: images[members[0]].layer.clone(),
                clip: images[members[0]].clip.clone(),
                transp: images[members[0]].transp,
                order: images[members[0]].order,
            });
            continue;
        }
        // merge group: composite into one image
        let run: Vec<&ImageOut> = members.iter().map(|&i| &images[i]).collect();
        if let Some(merged) = spatial_stitch_group(&run) {
            result.push(merged);
        } else {
            for &i in &members {
                result.push(ImageOut {
                    ctm: images[i].ctm,
                    raster: std::sync::Arc::clone(&images[i].raster),
                    layer: images[i].layer.clone(),
                    clip: images[i].clip.clone(),
                    transp: images[i].transp,
                    order: images[i].order,
                });
            }
        }
    }
    result
}

/// Merge a group of images that share orientation and spatial proximity into one composite.
fn spatial_stitch_group(run: &[&ImageOut]) -> Option<ImageOut> {
    if run.len() < 2 {
        return None;
    }

    let unit = |x: f64, y: f64| -> Option<(f64, f64, f64)> {
        let l = x.hypot(y);
        (l > 1e-9).then(|| (x / l, y / l, l))
    };
    let frame = |m: &M| -> Option<((f64, f64), (f64, f64), f64, f64)> {
        let [a, b, c, d, ..] = m.0;
        let (ux, uy, lu) = unit(a, b)?;
        let (vx, vy, lv) = unit(c, d)?;
        Some(((ux, uy), (vx, vy), lu, lv))
    };
    let first = run[0];
    let (u, v, _, _) = frame(&first.ctm)?;
    let dot = |p: (f64, f64), q: (f64, f64)| p.0 * q.0 + p.1 * q.1;

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
    if cw * ch > 16_000_000 {
        return None;
    } // safety: max ~16M pixels per composite

    let mut rgba = vec![0u8; cw * ch * 4];
    for (im, &(pu, pv, lu, lv)) in run.iter().zip(&parts) {
        let (w, h) = (im.raster.width as usize, im.raster.height as usize);
        let cx0 = (((pu - u0) / (u1 - u0)) * cw as f64).floor().max(0.0) as usize;
        let cx1 = ((((pu + lu - u0) / (u1 - u0)) * cw as f64).ceil() as usize).min(cw);
        let cy0 = (((v1 - pv - lv) / (v1 - v0)) * ch as f64).floor().max(0.0) as usize;
        let cy1 = ((((v1 - pv) / (v1 - v0)) * ch as f64).ceil() as usize).min(ch);
        for cy in cy0..cy1 {
            let vv = v1 - (cy as f64 + 0.5) / ch as f64 * (v1 - v0);
            let t = (vv - pv) / lv;
            if !(0.0..1.0).contains(&t) {
                continue;
            }
            let sy = (((1.0 - t) * h as f64) as usize).min(h.saturating_sub(1));
            for cx in cx0..cx1 {
                let uu = u0 + (cx as f64 + 0.5) / cw as f64 * (u1 - u0);
                let sx_f = (uu - pu) / lu;
                if !(0.0..1.0).contains(&sx_f) {
                    continue;
                }
                let sx = ((sx_f * w as f64) as usize).min(w.saturating_sub(1));
                let Some(px) = im.raster.pixel(sx as u32, sy as u32) else { continue };
                if px[3] == 0 {
                    continue;
                }
                let i = (cy * cw + cx) * 4;
                if let Some(dst) = rgba.get_mut(i..i + 4) {
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
    let order = run.iter().map(|im| im.order).max().unwrap_or(first.order);
    Some(ImageOut { ctm, raster: std::sync::Arc::new(raster), layer: first.layer.clone(), clip: first.clip.clone(), transp: first.transp, order })
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
/// imageFolder?, imagePrefix?, fills?: "each" | "merge"}` (`merge`: small fills as one hatch per
/// layer and colour, [`PdfOptions::merge_small_fills`]). Images are written as PNG files in `imageFolder` (created
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
        Some("content") => LayerMode::Content,
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
    o.recover_text = flag("recoverText", true);
    // PDFIMPORT brings one sheet in: an E sheet with a scale note comes in at that scale
    // (still reported do_not_scale + needs_registration) so it reads at building size.
    o.e_sheet_at_note_scale = true;
    o.merge_small_fills = p.get("fills").and_then(Value::as_str).is_some_and(|f| f.eq_ignore_ascii_case("merge"));
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
        "do_not_scale": r.do_not_scale,
        "needs_registration": r.needs_registration,
        "stats": r.stats,
        "ocr": {
            "clusters": r.ocr.clusters,
            "glyphs": r.ocr.glyphs,
            "recovered": r.ocr.words.len(),
            "noise_dropped": r.ocr.noise_dropped,
            "mean_confidence": (r.ocr.mean_confidence() * 1000.0).round() / 1000.0,
            "low_confidence": r.ocr.words.iter().filter(|w| w.confidence < 0.5).count(),
            "words": r.ocr.words.iter().take(500).map(|w| json!({"value": w.value, "at": [w.at.0, w.at.1], "confidence": (w.confidence * 1000.0).round() / 1000.0})).collect::<Vec<_>>(),
        },
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
    fn readings_no_hidden_string_confirms_and_no_word_are_kept_for_review() {
        let pen = Pen { rgb: [0, 0, 0], lw: 0, transp: 0 };
        let hidden = TextOut {
            at: (100.0, 100.0),
            end: (130.0, 100.0),
            height: 6.0,
            rotation: 0.0,
            width_factor: 1.0,
            value: "Hello".into(),
            pen,
            layer: Some(HIDDEN_TEXT_LAYER.into()),
            style: None,
            order: 0,
        };
        let read = |v: &str, at: (f64, f64)| RecoveredText {
            at,
            height: 6.0,
            value: v.into(),
            stroke_indices: HashSet::new(),
            pen,
            layer: None,
            unconfirmed: false,
            confidence: Some(0.9),
            review: false,
            rotation: 0.0,
        };
        let ocr = vec![read("U", (300.0, 300.0)), read("M L#.'", (320.0, 300.0)), read("151", (400.0, 300.0)), read("A-1", (420.0, 300.0))];
        let (kept, _, _, _) = settle_ocr(ocr, &[hidden], &[], &mut []);
        let review: Vec<(&str, bool)> = kept.iter().map(|r| (r.value.as_str(), r.review)).collect();
        assert_eq!(review, [("U", true), ("M L#.'", true), ("151", false), ("A-1", false)]);
        // A page with no hidden text keeps its readings as lettering (its sheets register by them).
        let (kept, _, _, _) = settle_ocr(vec![read("U", (300.0, 300.0))], &[], &[], &mut []);
        assert!(kept.iter().all(|r| !r.review));
    }

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
        let strokes: Vec<Stroke> = pts
            .windows(2)
            .map(|w| Stroke { pts: w.to_vec(), closed: false, pen, layer: None, curve_loop: false, curved_loop: false, order: 0 })
            .collect();
        // a faceplate: two arcs of one circle and two short sides is not a circle
        let lens: Vec<(f64, f64)> = (0..=8)
            .map(|i| (f64::from(i) / 8.0 * 1.2 - 0.6 + std::f64::consts::FRAC_PI_2).sin_cos())
            .map(|(s, c)| (5.0 * c, 5.0 * s))
            .chain((0..=8).map(|i| (f64::from(i) / 8.0 * 1.2 - 0.6 - std::f64::consts::FRAC_PI_2).sin_cos()).map(|(s, c)| (5.0 * c, 5.0 * s)))
            .collect();
        assert!(as_circle(&Stroke { pts: lens, closed: true, pen, layer: None, curve_loop: false, curved_loop: false, order: 0 }).is_none());
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
        // Pen mode: the default Pdf-mode now falls back to Content for non-OCG PDFs, but this
        // test checks pen-based layer names like PDF-BLACK-0.40.
        let r = read(&sample_pdf(), &PdfOptions { layers: LayerMode::Pen, ..PdfOptions::default() });
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

    #[test]
    fn stroked_marks_that_can_tag_a_symbol() {
        for t in ["A", "F", "OS", "EF", "L4", "WAP", "XA12"] {
            assert!(is_ocr_device_tag(t), "{t}");
        }
        for t in ["I", "O", "X", "s", "Ef", "4", "4A", "ABCD", "A123", "A-1", "NO TE", ""] {
            assert!(!is_ocr_device_tag(t), "{t}");
        }
        // a mark alone; a letter split off a word (the rest one gap away); its own box ignored
        let boxes = [[100.0, 100.0, 106.0, 108.0], [300.0, 100.0, 330.0, 108.0], [331.0, 100.0, 337.0, 108.0], [f64::NAN; 4]];
        assert!(stands_alone(0, 8.0, &boxes));
        assert!(!stands_alone(2, 8.0, &boxes));
        assert!(!stands_alone(9, 8.0, &boxes));
    }
}
