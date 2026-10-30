//! DWFx and DWF 6 output, and reading both back as underlay pages.
//!
//! **DWFx** is an XPS package (Microsoft "XML Paper Specification" 1.0, 2006, with the
//! `http://schemas.microsoft.com/xps/2005/06` namespaces that DWFx uses; the same structure is
//! standardised as ECMA-388 "Open XML Paper Specification", 1st edition 2009, whose OpenXPS
//! namespaces DWFx does not use). One FixedPage per sheet (model space or a layout):
//! - **True scale.** A layout's page is its paper at 1:1 (paper units → 1/96 inch). Model space
//!   is drawn at a stated scale (`scale`: drawing units per sheet inch, default 1:1 in the
//!   drawing's units) on a page around its extents; fitted to a 24 × 18 inch sheet only when
//!   asked (`fit`). Each page's placement (the drawing point at its lower-left corner and the
//!   scale) is recorded in a CADaddy part (`CADaddySheets.xml`, related from the
//!   FixedDocument), so an underlay of the page can be put back on the source coordinates.
//! - **Layers.** One `<Canvas Name=…>` per drawing layer (XPS §5.1 Canvas; names made
//!   XML-name-safe by [`canvas_name`], reversed by [`layer_from_canvas`]).
//! - **Hyperlinks.** An object's hyperlink is its paths' `FixedPage.NavigateUri` (XPS §12.2).
//! - **Sheet names.** A DocumentStructure part (XPS §11, document outline) lists one outline
//!   entry per page with the layout's name, targeting the page's `LinkTarget`.
//!
//! **DWF 6** (DWFOUT): a `(DWF V06.00)` header followed by a zip with a manifest and one W2D
//! graphics stream per sheet, written with WHIP!'s readable opcodes ([`w2d`] cites each one). The
//! package layout (manifest, section folders) follows CADaddy's reading of the DWF 6 package
//! described in Autodesk's DWF documentation and has not been checked in another DWF viewer.
//!
//! **Reading** ([`read_page`]): a DWFx page through a real XML reader ([`xml`]) and the full
//! abbreviated path syntax with Canvas/Path render transforms ([`path`]), each subpath one
//! polyline on its Canvas's layer, in sheet inches; or a DWF 6 / bare W2D stream written with the
//! readable opcodes. Pages, sheet names and recorded placements come with it.
//!
//! No Autodesk code, viewers, DLLs or sample files were used.

pub(crate) mod path;
pub(crate) mod w2d;
pub(crate) mod xml;

use std::collections::HashMap;
use std::fmt::Write as _;

use cadaddy_color::Rgb;
use cadaddy_doc::{Common, Drawing, EntityKind, Layer, LwPolyline, Space};
use cadaddy_geom::{PolyVertex, Vec2};
use cadaddy_render::Kind;
use serde_json::{Value, json};

use crate::{IoError, Result};

// ---- XPS package constants (XPS 1.0 §2.1, §9.1, §11; OPC parts and relationships) ----

const CONTENT_TYPES: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"fpage\" ContentType=\"application/vnd.ms-package.xps-fixedpage+xml\"/>\
<Default Extension=\"fdseq\" ContentType=\"application/vnd.ms-package.xps-fixeddocumentsequence+xml\"/>\
<Default Extension=\"fdoc\" ContentType=\"application/vnd.ms-package.xps-fixeddocument+xml\"/>\
<Default Extension=\"struct\" ContentType=\"application/vnd.ms-package.xps-documentstructure+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
</Types>\n";

const XPS_NS: &str = "http://schemas.microsoft.com/xps/2005/06";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const FDSEQ_REL: &str = "http://schemas.microsoft.com/xps/2005/06/fixedrepresentation";
const STRUCT_REL: &str = "http://schemas.microsoft.com/xps/2005/06/documentstructure";
const STRUCT_NS: &str = "http://schemas.microsoft.com/xps/2005/06/documentstructure";
/// CADaddy's own relationship to the part recording each page's placement.
const SHEETS_REL: &str = "urn:cadaddy:dwfx:sheets";
const SHEETS_PART: &str = "Documents/1/CADaddySheets.xml";
const FDOC_PART: &str = "Documents/1/FixedDocument.fdoc";

/// XPS units per inch.
const XPS_PER_INCH: f64 = 96.0;
/// The margin around model space on its page, in sheet inches.
const MODEL_MARGIN_IN: f64 = 0.5;
/// The sheet model space is fitted to when asked (`fit`), in inches.
const FIT_SHEET_IN: (f64, f64) = (24.0, 18.0);
/// Largest page side written, in sheet inches (about 5 km at 1:1).
const MAX_PAGE_IN: f64 = 200_000.0;

// ---- number formatting ----

fn n(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// A layer name as an XPS `Name` (letters, digits and `_` only, not starting with a digit):
/// every other character, `_` included, is written `_xHHHH_` (UTF-16 code units). Such a name
/// never contains a bare `_`, so it can't collide with the pages' own `Sheet_N` names.
pub(crate) fn canvas_name(layer: &str) -> String {
    let mut out = String::with_capacity(layer.len() + 8);
    for (i, c) in layer.chars().enumerate() {
        if c.is_ascii_alphabetic() || (c.is_ascii_digit() && i > 0) {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                let _ = write!(out, "_x{u:04X}_");
            }
        }
    }
    if out.is_empty() { "_x0030_".into() } else { out }
}

/// A Canvas `Name` back to the layer name ([`canvas_name`] reversed; other names as they are).
pub(crate) fn layer_from_canvas(name: &str) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(name.len());
    let mut rest = name;
    while !rest.is_empty() {
        let esc =
            rest.strip_prefix("_x").and_then(|r| r.get(..5)).filter(|e| e.ends_with('_')).and_then(|e| u16::from_str_radix(e.get(..4)?, 16).ok());
        match esc {
            Some(u) => {
                units.push(u);
                rest = rest.get(7..).unwrap_or("");
            }
            None => {
                let mut chars = rest.chars();
                if let Some(c) = chars.next() {
                    let mut buf = [0u16; 2];
                    units.extend_from_slice(c.encode_utf16(&mut buf));
                }
                rest = chars.as_str();
            }
        }
    }
    String::from_utf16_lossy(&units)
}

// ---- zip writer (same pattern as 3MF in threemf.rs) ----

fn zip(entries: &[(&str, &[u8], bool)]) -> Result<Vec<u8>> {
    let too_big = || IoError::Format("the DWFx package would be larger than 4 GiB".into());
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, deflate) in entries {
        let packed = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
        let method: u16 = if *deflate { 8 } else { 0 };
        let crc = crc32fast::hash(data);
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let (size, csize) = (u32::try_from(data.len()).map_err(|_| too_big())?, u32::try_from(packed.len()).map_err(|_| too_big())?);
        let nlen = u16::try_from(name.len()).map_err(|_| too_big())?;
        let header = |sig: u32, is_central: bool| {
            let mut h = Vec::with_capacity(46 + name.len());
            h.extend(sig.to_le_bytes());
            if is_central {
                h.extend(20u16.to_le_bytes());
            }
            for v in [20u16, 0, method, 0, 0x21] {
                h.extend(v.to_le_bytes());
            }
            h.extend(crc.to_le_bytes());
            h.extend(csize.to_le_bytes());
            h.extend(size.to_le_bytes());
            h.extend(nlen.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if is_central {
                for v in [0u16, 0, 0] {
                    h.extend(v.to_le_bytes());
                }
                h.extend(0u32.to_le_bytes());
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend(&packed);
        central.extend(header(0x0201_4b50, true));
    }
    let cd_at = u32::try_from(out.len()).map_err(|_| too_big())?;
    let cd_len = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend(central);
    let cnt = u16::try_from(entries.len()).map_err(|_| too_big())?;
    out.extend(0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, cnt, cnt] {
        out.extend(v.to_le_bytes());
    }
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    Ok(out)
}

// ---- unzip reader (for DWFx underlays) ----

/// Read a zip's central directory and return entries.  Only deflate (8) and stored (0).
fn zip_entries(data: &[u8]) -> Result<Vec<(String, usize, usize, usize, u16)>> {
    let eocd = data.windows(4).rposition(|w| w == b"PK\x05\x06").ok_or_else(|| IoError::Format("not a zip file".into()))?;
    if eocd + 22 > data.len() {
        return Err(IoError::Format("truncated zip".into()));
    }
    let u32le = |off: usize| u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
    let u16le = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]);
    let cd_size = u32le(eocd + 12) as usize;
    let cd_off = u32le(eocd + 16) as usize;
    if cd_off.checked_add(cd_size).is_none_or(|end| end > data.len()) {
        return Err(IoError::Format("truncated zip central directory".into()));
    }
    let mut entries = Vec::new();
    let mut pos = cd_off;
    let limit = cd_off + cd_size;
    while pos + 46 <= limit {
        if data[pos..pos + 4] != [0x50, 0x4b, 0x01, 0x02] {
            break;
        }
        let method = u16le(pos + 10);
        let csize = u32le(pos + 20) as usize;
        let usize_ = u32le(pos + 24) as usize;
        let nlen = u16le(pos + 28) as usize;
        let extra = u16le(pos + 30) as usize;
        let comment = u16le(pos + 32) as usize;
        let local_off = u32le(pos + 42) as usize;
        let name_end = pos + 46 + nlen;
        if name_end > limit {
            break;
        }
        let name = String::from_utf8_lossy(&data[pos + 46..name_end]).to_string();
        let data_off = if local_off + 30 <= data.len() {
            let ln = u16le(local_off + 26) as usize;
            let le = u16le(local_off + 28) as usize;
            local_off + 30 + ln + le
        } else {
            0
        };
        entries.push((name, data_off, csize, usize_, method));
        pos = name_end + extra + comment;
    }
    Ok(entries)
}

/// Largest part read from a package, compressed or not.
const MAX_PART: usize = 64 << 20;

pub(crate) fn zip_read(data: &[u8], part: &str) -> Result<Vec<u8>> {
    let entries = zip_entries(data)?;
    let norm = |s: &str| s.trim_start_matches('/').to_ascii_lowercase();
    let key = norm(part);
    let (_, off, csize, usize_, method) =
        entries.iter().find(|(n, ..)| norm(n) == key).ok_or_else(|| IoError::Format(format!("part `{part}` not found in the package")))?;
    if *usize_ > MAX_PART || *csize > MAX_PART {
        return Err(IoError::Format("a part of the package is too large".into()));
    }
    let end = off.checked_add(*csize).ok_or_else(|| IoError::Format("truncated zip entry".into()))?;
    if end > data.len() {
        return Err(IoError::Format("truncated zip entry".into()));
    }
    let raw = &data[*off..end];
    match method {
        0 => Ok(raw.to_vec()),
        // The declared size may lie: inflating stops at MAX_PART whatever it says.
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, MAX_PART).map_err(|e| match e.status {
            miniz_oxide::inflate::TINFLStatus::HasMoreOutput => IoError::Format(format!("part `{part}` expands past {} MiB", MAX_PART >> 20)),
            _ => IoError::Format("failed to decompress a part".into()),
        }),
        _ => Err(IoError::Format(format!("unsupported zip compression method {method}"))),
    }
}

pub(crate) fn zip_list(data: &[u8]) -> Result<Vec<String>> {
    Ok(zip_entries(data)?.into_iter().map(|(n, ..)| n).collect())
}

fn part_xml(data: &[u8], part: &str) -> Result<xml::Element> {
    let bytes = zip_read(data, part)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| IoError::Format(format!("`{part}` is not UTF-8")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    xml::parse(text).map_err(|e| IoError::Format(format!("`{part}`: {e}")))
}

// ---- sheets: what a page shows, in sheet inches ----

/// How DWFXOUT/DWFOUT draw model space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExportOptions {
    /// Model space: drawing units per sheet inch (default: 1:1 in the drawing's units, 25.4 for
    /// a metric drawing and 1 for an imperial one). Layouts are always at 1:1 paper size.
    pub scale: Option<f64>,
    /// Model space fitted to a 24 × 18 inch sheet instead (the scale is then recorded).
    pub fit: bool,
}

impl ExportOptions {
    /// `{scale?, fit?}` of a command or plot.
    pub fn from_json(v: &Value) -> ExportOptions {
        ExportOptions {
            scale: v.get("scale").and_then(Value::as_f64).filter(|s| s.is_finite() && *s > 0.0),
            fit: v.get("fit").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Shape {
    Line(Vec<Vec2>),
    /// Filled triangles (three points each).
    Tris(Vec<Vec2>),
    Dot(Vec2),
}

#[derive(Clone, Debug)]
struct Item {
    layer: String,
    rgb: Rgb,
    /// 0..1 when not opaque.
    opacity: Option<f64>,
    /// Stroke width in sheet inches.
    lw: f64,
    mask: bool,
    shape: Shape,
    link: Option<String>,
}

/// One sheet: its name, page size and placement, and what it draws (sheet inches, y up).
#[derive(Clone, Debug)]
struct SheetGeom {
    name: String,
    size: Vec2,
    /// The drawing point at the page's lower-left corner.
    origin: Vec2,
    /// Drawing units per sheet inch.
    scale: f64,
    items: Vec<Item>,
}

/// Hyperlinks of the drawing's objects (`PE_URL`), by handle, resolved against HYPERLINKBASE.
fn links(d: &Drawing) -> HashMap<cadaddy_doc::Handle, String> {
    let base = d.header.str("HYPERLINKBASE", "");
    std::iter::once(&d.model)
        .chain(d.layouts.iter().map(|l| &l.entities))
        .flat_map(|s| s.iter())
        .filter_map(|e| {
            let h = cadaddy_doc::hyperlink::read(&e.common.xdata)?;
            let url = cadaddy_doc::hyperlink::resolve(&h, &base, None)?;
            (!url.is_empty() && url.len() <= cadaddy_doc::hyperlink::MAX_LEN).then_some((e.handle, url))
        })
        .collect()
}

fn sheet_geom(d: &Drawing, space: &Space, o: &ExportOptions, links: &HashMap<cadaddy_doc::Handle, String>) -> Result<SheetGeom> {
    let list = cadaddy_render::build_plot(d, space, &cadaddy_render::Options { transparency: true, ..cadaddy_render::Options::default() });
    let (name, size, origin, scale) = match space {
        Space::Paper(name) => {
            let l = d.layout(name).ok_or_else(|| IoError::Format(format!("no layout `{name}`")))?;
            let unit = cadaddy_render::paper::page_unit_mm(d, &l.page);
            let sheet = cadaddy_render::Sheet::from_page(&l.page, unit);
            // Paper units per sheet inch.
            let scale = 25.4 / unit;
            (l.name.clone(), sheet.size / scale, l.origin, scale)
        }
        Space::Model => {
            let b = if list.bounds.is_empty() { cadaddy_geom::Bounds2::new(Vec2::ZERO, Vec2::new(1.0, 1.0)) } else { list.bounds };
            let (bw, bh) = (b.width().max(1e-9), b.height().max(1e-9));
            let scale = if o.fit {
                let (w, h) = (FIT_SHEET_IN.0 - 2.0 * MODEL_MARGIN_IN, FIT_SHEET_IN.1 - 2.0 * MODEL_MARGIN_IN);
                (bw / w).max(bh / h)
            } else {
                o.scale.unwrap_or(25.4 / cadaddy_render::paper::paper_unit_mm(d))
            };
            let size = Vec2::new(bw / scale + 2.0 * MODEL_MARGIN_IN, bh / scale + 2.0 * MODEL_MARGIN_IN);
            let origin = b.min - Vec2::new(MODEL_MARGIN_IN, MODEL_MARGIN_IN) * scale;
            ("Model".to_string(), size, origin, scale)
        }
    };
    if !(scale.is_finite() && scale > 0.0) {
        return Err(IoError::Format("the sheet's scale is not a positive number".into()));
    }
    if !(size.x.is_finite() && size.y.is_finite()) || size.x > MAX_PAGE_IN || size.y > MAX_PAGE_IN {
        return Err(IoError::Format(format!(
            "at {} drawing units per inch the sheet would be {} × {} inches: give a larger `scale` or `fit`",
            n(scale),
            n(size.x),
            n(size.y)
        )));
    }
    let to_sheet = |p: Vec2| (p - origin) / scale;
    let white = Rgb(255, 255, 255);
    let mut items = Vec::with_capacity(list.prims.len());
    for (i, p) in list.prims.iter().enumerate() {
        let layer = list
            .prim_layers
            .get(i)
            .and_then(|li| d.layers.get(usize::try_from(*li).ok()?))
            .map(|l| l.name.to_string())
            .unwrap_or_else(|| "0".to_string());
        let pts = list.points(p);
        let shape = match p.kind {
            Kind::Polyline if pts.len() >= 2 => Shape::Line(pts.iter().map(|q| to_sheet(*q)).collect()),
            Kind::Tris | Kind::InkMask | Kind::Mask if pts.len() >= 3 => Shape::Tris(pts.iter().map(|q| to_sheet(*q)).collect()),
            Kind::Point => match pts.first() {
                Some(q) => Shape::Dot(to_sheet(*q)),
                None => continue,
            },
            _ => continue,
        };
        items.push(Item {
            layer,
            rgb: p.display_rgb(white),
            opacity: (p.transparency > 0).then(|| f64::from(p.alpha()) / 255.0),
            lw: f64::from(p.lw) / 25.4,
            mask: p.kind == Kind::Mask,
            shape,
            link: links.get(&p.handle).cloned(),
        });
    }
    Ok(SheetGeom { name, size, origin, scale, items })
}

/// The XPS FixedPage of a sheet: one Canvas per layer, in the order layers first draw.
fn fixed_page(s: &SheetGeom, index: usize) -> String {
    let h = s.size.y * XPS_PER_INCH;
    let at = |p: &Vec2| format!("{},{}", n(p.x * XPS_PER_INCH), n(h - p.y * XPS_PER_INCH));
    let mut order: Vec<&str> = Vec::new();
    let mut by_layer: HashMap<&str, Vec<&Item>> = HashMap::new();
    for it in &s.items {
        let e = by_layer.entry(it.layer.as_str()).or_default();
        if e.is_empty() {
            order.push(it.layer.as_str());
        }
        e.push(it);
    }
    let mut out = String::with_capacity(s.items.len() * 100 + 1024);
    let _ = writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        out,
        "<FixedPage Width=\"{}\" Height=\"{}\" xmlns=\"{XPS_NS}\" xml:lang=\"en-US\" Name=\"Sheet_{index}\">",
        n(s.size.x * XPS_PER_INCH),
        n(h)
    );
    for layer in order {
        let _ = writeln!(out, " <Canvas Name=\"{}\">", canvas_name(layer));
        for it in by_layer.get(layer).into_iter().flatten() {
            let color = format!("#{:02X}{:02X}{:02X}", it.rgb.0, it.rgb.1, it.rgb.2);
            let mut extra = String::new();
            if let Some(o) = it.opacity {
                let _ = write!(extra, " Opacity=\"{}\"", n(o));
            }
            if let Some(url) = &it.link {
                let _ = write!(extra, " FixedPage.NavigateUri=\"{}\"", xml::escape(url));
            }
            match &it.shape {
                Shape::Line(pts) => {
                    let mut data = String::with_capacity(pts.len() * 20);
                    for (j, p) in pts.iter().enumerate() {
                        let _ = write!(data, "{}{}", if j == 0 { "M " } else { " L " }, at(p));
                    }
                    let lw = (it.lw * XPS_PER_INCH).max(0.5);
                    let _ = writeln!(out, "  <Path Data=\"{data}\" Stroke=\"{color}\" StrokeThickness=\"{}\"{extra}/>", n(lw));
                }
                Shape::Tris(pts) => {
                    let mut data = String::with_capacity(pts.len() * 24);
                    for t in pts.chunks_exact(3) {
                        if let [a, b, c] = t {
                            let _ = write!(data, "M {} L {} L {} Z ", at(a), at(b), at(c));
                        }
                    }
                    let fill = if it.mask { "#FFFFFF".to_string() } else { color };
                    let _ = writeln!(out, "  <Path Data=\"{}\" Fill=\"{fill}\"{extra}/>", data.trim_end());
                }
                Shape::Dot(p) => {
                    let (x, y) = (p.x * XPS_PER_INCH, h - p.y * XPS_PER_INCH);
                    let r = 0.5;
                    let _ = writeln!(
                        out,
                        "  <Path Data=\"M {},{} A {r},{r} 0 1 1 {},{} A {r},{r} 0 1 1 {},{} Z\" Fill=\"{color}\"{extra}/>",
                        n(x - r),
                        n(y),
                        n(x + r),
                        n(y),
                        n(x - r),
                        n(y)
                    );
                }
            }
        }
        out.push_str(" </Canvas>\n");
    }
    out.push_str("</FixedPage>\n");
    out
}

// ---- public export API ----

/// Single-space export as DWFx.
pub fn export(d: &Drawing, space: &Space) -> Result<Vec<u8>> {
    export_sheets_with(d, std::slice::from_ref(space), &ExportOptions::default())
}

/// Multi-sheet DWFx at the default scale: one page per space.
pub fn export_sheets(d: &Drawing, spaces: &[Space]) -> Result<Vec<u8>> {
    export_sheets_with(d, spaces, &ExportOptions::default())
}

/// Multi-sheet DWFx: one page per space, named, layered and at true scale.
pub fn export_sheets_with(d: &Drawing, spaces: &[Space], o: &ExportOptions) -> Result<Vec<u8>> {
    if spaces.is_empty() {
        return Err(IoError::Format("no sheets to export as DWFx".into()));
    }
    let links = links(d);
    let sheets = spaces.iter().map(|s| sheet_geom(d, s, o, &links)).collect::<Result<Vec<_>>>()?;
    let mut parts: Vec<(String, Vec<u8>, bool)> = Vec::new();
    parts.push(("[Content_Types].xml".into(), CONTENT_TYPES.as_bytes().to_vec(), false));
    parts.push((
        "_rels/.rels".into(),
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"{RELS_NS}\"><Relationship Target=\"/FixedDocumentSequence.fdseq\" Id=\"R0\" Type=\"{FDSEQ_REL}\"/></Relationships>\n")
            .into_bytes(),
        false,
    ));
    parts.push((
        "FixedDocumentSequence.fdseq".into(),
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FixedDocumentSequence xmlns=\"{XPS_NS}\"><DocumentReference Source=\"/{FDOC_PART}\"/></FixedDocumentSequence>\n").into_bytes(),
        true,
    ));
    let mut fdoc = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FixedDocument xmlns=\"{XPS_NS}\">\n");
    let mut outline = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DocumentStructure xmlns=\"{STRUCT_NS}\">\n <DocumentStructure.Outline>\n  <DocumentOutline xml:lang=\"en-US\">\n"
    );
    let mut record = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Sheets xmlns=\"urn:cadaddy:dwfx:sheets\">\n");
    for (i, s) in sheets.iter().enumerate() {
        let k = i + 1;
        let page = format!("Documents/1/Pages/{k}.fpage");
        parts.push((page.clone(), fixed_page(s, k).into_bytes(), true));
        let _ = writeln!(
            fdoc,
            " <PageContent Source=\"/{page}\" Width=\"{}\" Height=\"{}\"><PageContent.LinkTargets><LinkTarget Name=\"Sheet_{k}\"/></PageContent.LinkTargets></PageContent>",
            n(s.size.x * XPS_PER_INCH),
            n(s.size.y * XPS_PER_INCH)
        );
        let name = xml::escape(&s.name);
        let _ = writeln!(outline, "   <OutlineEntry OutlineLevel=\"1\" OutlineTarget=\"/{FDOC_PART}#Sheet_{k}\" Description=\"{name}\"/>");
        let _ =
            writeln!(record, " <Sheet Page=\"{k}\" Name=\"{name}\" OriginX=\"{}\" OriginY=\"{}\" Scale=\"{}\"/>", s.origin.x, s.origin.y, s.scale);
    }
    fdoc.push_str("</FixedDocument>\n");
    outline.push_str("  </DocumentOutline>\n </DocumentStructure.Outline>\n</DocumentStructure>\n");
    record.push_str("</Sheets>\n");
    parts.push((FDOC_PART.into(), fdoc.into_bytes(), true));
    parts.push((
        "Documents/1/_rels/FixedDocument.fdoc.rels".into(),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"{RELS_NS}\">\
             <Relationship Target=\"/Documents/1/Structure/DocStructure.struct\" Id=\"R1\" Type=\"{STRUCT_REL}\"/>\
             <Relationship Target=\"/{SHEETS_PART}\" Id=\"R2\" Type=\"{SHEETS_REL}\"/></Relationships>\n"
        )
        .into_bytes(),
        false,
    ));
    parts.push(("Documents/1/Structure/DocStructure.struct".into(), outline.into_bytes(), true));
    parts.push((SHEETS_PART.into(), record.into_bytes(), true));
    let refs: Vec<(&str, &[u8], bool)> = parts.iter().map(|(n, d, c)| (n.as_str(), d.as_slice(), *c)).collect();
    zip(&refs)
}

/// The plot hook's DWFx/DWF output: the sheets of `layouts` (when the plot names several) or
/// `space`; model space fitted unless `fit: false` (then at `scale` paper units per drawing
/// unit, as PLOT takes it).
pub fn plot(d: &Drawing, space: &Space, opts: &Value, dwf6: bool) -> Result<Vec<u8>> {
    let spaces: Vec<Space> = match opts.get("layouts").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a
            .iter()
            .filter_map(Value::as_str)
            .map(|n| if n.eq_ignore_ascii_case("model") { Space::Model } else { Space::Paper(n.to_string()) })
            .collect(),
        _ => vec![space.clone()],
    };
    let fit = opts.get("fit").and_then(Value::as_bool).unwrap_or(true);
    let paper_per_unit = opts.get("scale").and_then(Value::as_f64).filter(|k| k.is_finite() && *k > 0.0);
    let o =
        ExportOptions { fit: fit && paper_per_unit.is_none(), scale: paper_per_unit.map(|k| 25.4 / (cadaddy_render::paper::paper_unit_mm(d) * k)) };
    if dwf6 { export_dwf6_sheets(d, &spaces, &o) } else { export_sheets_with(d, &spaces, &o) }
}

// ---- DWF 6 export ----

/// DWF 6 of model space at the default scale.
pub fn export_dwf6(d: &Drawing, space: &Space) -> Result<Vec<u8>> {
    export_dwf6_sheets(d, std::slice::from_ref(space), &ExportOptions::default())
}

fn w2d_items(s: &SheetGeom) -> Vec<w2d::Drawn> {
    let mut out = Vec::with_capacity(s.items.len());
    for it in &s.items {
        let rgb = if it.mask { Rgb(255, 255, 255) } else { it.rgb };
        match &it.shape {
            Shape::Line(p) => out.push(w2d::Drawn { layer: it.layer.clone(), rgb, shape: w2d::Shape::Line(p.clone()) }),
            Shape::Tris(p) => {
                out.extend(p.chunks_exact(3).map(|t| w2d::Drawn { layer: it.layer.clone(), rgb, shape: w2d::Shape::Fill(t.to_vec()) }));
            }
            Shape::Dot(_) => {}
        }
    }
    out
}

/// DWF 6: the `(DWF V06.00)` header, then a zip of `manifest.xml` and one W2D stream per sheet
/// (`ePlot_<n>/graphics.w2d`). The manifest names each sheet and records its paper size and
/// placement.
pub fn export_dwf6_sheets(d: &Drawing, spaces: &[Space], o: &ExportOptions) -> Result<Vec<u8>> {
    if spaces.is_empty() {
        return Err(IoError::Format("no sheets to export as DWF".into()));
    }
    let links = links(d);
    let mut manifest = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Manifest xmlns=\"http://www.autodesk.com/dwf/schema\">\n");
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, space) in spaces.iter().enumerate() {
        let s = sheet_geom(d, space, o, &links)?;
        let k = i + 1;
        let href = format!("ePlot_{k}/graphics.w2d");
        let _ = writeln!(
            manifest,
            " <Section Name=\"ePlot_{k}\" Type=\"com.autodesk.dwf.ePlot\" Title=\"{}\">\n  <Resource Href=\"{href}\" Role=\"2d streaming graphics\" MimeType=\"application/x-w2d\">\n   <Properties>\n    <Property Name=\"PaperSize\" Value=\"{} {}\"/>\n    <Property Name=\"CADaddy.Origin\" Value=\"{} {}\"/>\n    <Property Name=\"CADaddy.Scale\" Value=\"{}\"/>\n   </Properties>\n  </Resource>\n </Section>",
            xml::escape(&s.name),
            s.size.x,
            s.size.y,
            s.origin.x,
            s.origin.y,
            s.scale
        );
        parts.push((href, w2d::write(&w2d_items(&s))));
    }
    manifest.push_str("</Manifest>\n");
    let mut entries: Vec<(&str, &[u8], bool)> = vec![("manifest.xml", manifest.as_bytes(), true)];
    entries.extend(parts.iter().map(|(n, b)| (n.as_str(), b.as_slice(), true)));
    let mut out = w2d::HEADER.to_vec();
    out.extend(zip(&entries)?);
    Ok(out)
}

// ---- reading (for underlays) ----

/// A page read for an underlay.
#[derive(Clone, Debug)]
pub struct Page {
    /// The page's geometry in sheet inches (origin at its lower-left corner), on its layers.
    pub drawing: Drawing,
    /// The page size in sheet inches.
    pub size: Vec2,
    /// Pages in the file.
    pub pages: usize,
    /// The sheet's name (a layout's), when the file names it.
    pub name: Option<String>,
    /// Where CADaddy drew the page from, when it did: the drawing point at the page's
    /// lower-left corner and the drawing units per sheet inch.
    pub placement: Option<(Vec2, f64)>,
}

impl Page {
    /// The reader's report for DWFATTACH: `{sheet: [w, h], pages, name?, origin?, scale?}`.
    pub fn report(&self) -> Value {
        let mut r = json!({ "sheet": [self.size.x, self.size.y], "pages": self.pages });
        if let Some(n) = &self.name {
            r["name"] = json!(n);
        }
        if let Some((o, s)) = self.placement {
            r["origin"] = json!([o.x, o.y]);
            r["scale"] = json!(s);
        }
        r
    }
}

/// Whether `data` is a DWF 6 file or a bare W2D stream (as opposed to a DWFx).
pub fn is_dwf6(data: &[u8]) -> bool {
    data.starts_with(b"(DWF V") || data.starts_with(b"(W2D V")
}

/// Page `page` (1-based) of a DWFx, DWF 6 or bare W2D file, in sheet inches.
pub fn read_page(data: &[u8], page: u32) -> Result<Page> {
    if is_dwf6(data) {
        return read_dwf6_page(data, page);
    }
    let parts = zip_list(data)?;
    if !parts.iter().any(|p| p.to_ascii_lowercase().ends_with(".fpage")) {
        if parts.iter().any(|p| p.to_ascii_lowercase().ends_with(".w2d")) {
            return read_dwf6_page(data, page);
        }
        return Err(IoError::Format("not a DWFx or DWF file (no pages in the package)".into()));
    }
    let order = page_order(data, &parts);
    let pages = order.len();
    let idx = usize::try_from(page).unwrap_or(usize::MAX).saturating_sub(1);
    let (part, fdoc, link_target) =
        order.get(idx).cloned().ok_or_else(|| IoError::Format(format!("page {page} not found (the file has {pages} page(s))")))?;
    let root = part_xml(data, &part)?;
    let (drawing, size) = parse_fixed_page(&root)?;
    let mut name = None;
    let mut placement = None;
    if let Some(fdoc) = &fdoc {
        for (ty, target) in rels(data, fdoc) {
            if ty == STRUCT_REL {
                name = name.or_else(|| outline_name(data, &target, fdoc, link_target.as_deref(), idx));
            } else if ty == SHEETS_REL
                && let Ok(s) = part_xml(data, &target)
                && let Some(sheet) = s.named("Sheet").find(|e| e.attr("Page").and_then(|p| p.trim().parse::<usize>().ok()) == Some(idx + 1))
            {
                let f = |k: &str| sheet.attr(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
                if let (Some(x), Some(y), Some(s)) = (f("OriginX"), f("OriginY"), f("Scale").filter(|s| *s > 0.0)) {
                    placement = Some((Vec2::new(x, y), s));
                }
                name = sheet.attr("Name").map(str::to_string).or(name);
            }
        }
    }
    Ok(Page { drawing, size, pages, name, placement })
}

/// A part name from a reference in `from`: absolute (`/a/b`) or relative to `from`'s folder.
fn resolve_part(from: &str, target: &str) -> String {
    let target = target.split('#').next().unwrap_or("");
    let mut segs: Vec<&str> = if target.starts_with('/') { Vec::new() } else { from.trim_start_matches('/').split('/').collect() };
    if !target.starts_with('/') {
        segs.pop();
    }
    for s in target.trim_start_matches('/').split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    segs.join("/")
}

/// The relationships of `part`: (type, target part).
fn rels(data: &[u8], part: &str) -> Vec<(String, String)> {
    let p = part.trim_start_matches('/');
    let (dir, file) = p.rsplit_once('/').unwrap_or(("", p));
    let rels_part = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
    let Ok(root) = part_xml(data, &rels_part) else { return Vec::new() };
    root.named("Relationship")
        .filter(|r| r.attr("TargetMode").is_none_or(|m| m != "External"))
        .filter_map(|r| Some((r.attr("Type")?.to_string(), resolve_part(p, r.attr("Target")?))))
        .collect()
}

/// The pages in reading order: (page part, its FixedDocument, its first link target). From
/// the package's FixedDocumentSequence; without one, the `.fpage` parts by number.
fn page_order(data: &[u8], parts: &[String]) -> Vec<(String, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    if let Some((_, seq)) = rels(data, "/").into_iter().find(|(t, _)| t == FDSEQ_REL)
        && let Ok(seq_xml) = part_xml(data, &seq)
    {
        for doc in seq_xml.named("DocumentReference").filter_map(|r| r.attr("Source")).map(|s| resolve_part(&seq, s)) {
            let Ok(fdoc) = part_xml(data, &doc) else { continue };
            for pc in fdoc.named("PageContent") {
                let Some(src) = pc.attr("Source") else { continue };
                let target = pc.child("PageContent.LinkTargets").and_then(|l| l.child("LinkTarget")).and_then(|t| t.attr("Name")).map(str::to_string);
                out.push((resolve_part(&doc, src), Some(doc.clone()), target));
            }
        }
    }
    if out.is_empty() {
        let mut pages: Vec<&String> = parts.iter().filter(|p| p.to_ascii_lowercase().ends_with(".fpage")).collect();
        let num = |p: &str| p.rsplit('/').next().and_then(|f| f.split('.').next()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(u64::MAX);
        pages.sort_by(|a, b| num(a).cmp(&num(b)).then(a.cmp(b)));
        out = pages.into_iter().map(|p| (p.trim_start_matches('/').to_string(), None, None)).collect();
    }
    out
}

/// The outline entry naming page `idx`: the one targeting its link target, else the
/// `idx`-th first-level entry when every entry targets a page by position.
fn outline_name(data: &[u8], part: &str, _fdoc: &str, link: Option<&str>, idx: usize) -> Option<String> {
    let root = part_xml(data, part).ok()?;
    let mut entries = Vec::new();
    root.descendants("OutlineEntry", &mut entries);
    if let Some(link) = link
        && let Some(e) = entries.iter().find(|e| e.attr("OutlineTarget").and_then(|t| t.rsplit_once('#')).is_some_and(|(_, f)| f == link))
    {
        return e.attr("Description").map(str::to_string);
    }
    entries.get(idx).and_then(|e| e.attr("Description")).map(str::to_string)
}

/// `#RRGGBB` or `#AARRGGBB` (XPS sRGB colours).
fn color(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    let h = match h.len() {
        6 => h,
        8 => h.get(2..)?,
        _ => return None,
    };
    let c = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some(Rgb(c(0)?, c(2)?, c(4)?))
}

/// What a page walk carries down: the transform so far, the layer and the link.
#[derive(Clone)]
struct Scope {
    m: path::Matrix,
    layer: String,
    link: Option<String>,
}

/// The page's layers are the sheet's: a new drawing's own (`0`, `Defpoints`) go unless the
/// sheet draws on them.
fn only_used_layers(d: &mut Drawing) {
    let used: std::collections::HashSet<String> = d.model.iter().map(|e| e.common.layer.to_ascii_lowercase()).collect();
    d.layers.retain(|l| used.contains(&l.name.to_ascii_lowercase()));
}

/// A FixedPage as a drawing in sheet inches (y up) and its size in inches.
fn parse_fixed_page(root: &xml::Element) -> Result<(Drawing, Vec2)> {
    if root.name != "FixedPage" {
        return Err(IoError::Format(format!("a page part holds `<{}>`, not a FixedPage", root.name)));
    }
    let dim = |k: &str| root.attr(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
    let (w, h) = (dim("Width").unwrap_or(816.0), dim("Height").unwrap_or(1056.0));
    let mut d = Drawing::new_imperial();
    let scope = Scope { m: path::Matrix::IDENTITY, layer: "0".into(), link: root.attr("FixedPage.NavigateUri").map(str::to_string) };
    let mut budget = path::MAX_POINTS;
    walk(root, &scope, h, &mut d, &mut budget)?;
    only_used_layers(&mut d);
    Ok((d, Vec2::new(w / XPS_PER_INCH, h / XPS_PER_INCH)))
}

fn walk(el: &xml::Element, scope: &Scope, page_h: f64, d: &mut Drawing, budget: &mut usize) -> Result<()> {
    for c in &el.children {
        // Property elements (`X.RenderTransform`, `X.Resources`, `Path.Data`…) are read by
        // their owners.
        if c.name.contains('.') {
            continue;
        }
        let own = path::render_transform(c).unwrap_or(path::Matrix::IDENTITY);
        let mut sc = Scope { m: own.then(&scope.m), layer: scope.layer.clone(), link: scope.link.clone() };
        if let Some(url) = c.attr("FixedPage.NavigateUri") {
            sc.link = Some(url.to_string());
        }
        match c.name.as_str() {
            "Canvas" => {
                if let Some(name) = c.attr("Name").filter(|n| !n.trim().is_empty()) {
                    sc.layer = layer_from_canvas(name);
                }
                walk(c, &sc, page_h, d, budget)?;
            }
            "Path" => {
                let figures = match c.attr("Data") {
                    Some(data) if !data.trim_start().starts_with('{') => path::parse_data(data),
                    _ => match c.child("Path.Data").and_then(|p| p.child("PathGeometry")) {
                        Some(g) => path::parse_geometry(g),
                        None => Ok(Vec::new()),
                    },
                }
                .map_err(IoError::Format)?;
                let rgb = c.attr("Stroke").and_then(color).or_else(|| c.attr("Fill").and_then(color));
                add_figures(d, &sc, rgb, figures, page_h, budget)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn add_figures(d: &mut Drawing, sc: &Scope, rgb: Option<Rgb>, figures: Vec<path::Figure>, page_h: f64, budget: &mut usize) -> Result<()> {
    if figures.is_empty() {
        return Ok(());
    }
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&sc.layer)) {
        d.layers.push(Layer::new(&sc.layer));
    }
    let mut common = Common { layer: sc.layer.as_str().into(), ..Common::default() };
    if let Some(c) = rgb {
        common.color = cadaddy_color::Color::True(c);
    }
    if let Some(url) = &sc.link {
        let link = cadaddy_doc::hyperlink::Hyperlink { url: url.clone(), ..Default::default() };
        cadaddy_doc::hyperlink::write(&mut common.xdata, Some(&link));
    }
    for f in figures {
        *budget = budget.checked_sub(f.points.len()).ok_or_else(|| IoError::Format("the page has too many points".into()))?;
        let vertices = f
            .points
            .iter()
            .map(|p| {
                let q = sc.m.apply(*p);
                PolyVertex { p: Vec2::new(q.x / XPS_PER_INCH, (page_h - q.y) / XPS_PER_INCH), bulge: 0.0, start_width: 0.0, end_width: 0.0 }
            })
            .collect();
        let pl = LwPolyline { vertices, closed: f.closed, const_width: 0.0, elevation: 0.0, plinegen: false };
        d.add(&Space::Model, common.clone(), EntityKind::LwPolyline(pl)).map_err(|e| IoError::Format(e.to_string()))?;
    }
    Ok(())
}

/// A DWF 6 page (or a bare W2D stream as page 1).
fn read_dwf6_page(data: &[u8], page: u32) -> Result<Page> {
    let zip_at = data.windows(4).position(|w| w == b"PK\x03\x04");
    let Some(zip_at) = zip_at.filter(|_| !data.starts_with(b"(W2D V")) else {
        // A whole-file stream (older single-stream DWF, or a bare W2D).
        if page != 1 {
            return Err(IoError::Format(format!("page {page} not found (the file has 1 page(s))")));
        }
        return dwf6_page(data, None, None, 1);
    };
    let zipped = data.get(zip_at..).unwrap_or(&[]);
    let parts = zip_list(zipped)?;
    let mut streams: Vec<(String, Option<String>, Option<(Vec2, f64, Vec2)>)> = Vec::new();
    if let Ok(m) = part_xml(zipped, "manifest.xml") {
        let mut secs = Vec::new();
        m.descendants("Section", &mut secs);
        for s in secs {
            let mut res = Vec::new();
            s.descendants("Resource", &mut res);
            let Some(href) = res.iter().find_map(|r| r.attr("Href").filter(|h| h.to_ascii_lowercase().ends_with(".w2d"))) else { continue };
            let mut props = Vec::new();
            s.descendants("Property", &mut props);
            let prop = |k: &str| -> Option<Vec<f64>> {
                let v = props.iter().find(|p| p.attr("Name") == Some(k))?.attr("Value")?;
                v.split_whitespace().map(|t| t.parse::<f64>().ok().filter(|x| x.is_finite())).collect()
            };
            let placed = match (prop("CADaddy.Origin").as_deref(), prop("CADaddy.Scale").as_deref(), prop("PaperSize").as_deref()) {
                (Some([x, y]), Some([s]), Some([w, h])) if *s > 0.0 => Some((Vec2::new(*x, *y), *s, Vec2::new(*w, *h))),
                _ => None,
            };
            streams.push((href.trim_start_matches('/').to_string(), s.attr("Title").map(str::to_string), placed));
        }
    }
    if streams.is_empty() {
        let mut w: Vec<&String> = parts.iter().filter(|p| p.to_ascii_lowercase().ends_with(".w2d")).collect();
        w.sort();
        streams = w.into_iter().map(|p| (p.clone(), None, None)).collect();
    }
    let pages = streams.len();
    let idx = usize::try_from(page).unwrap_or(usize::MAX).saturating_sub(1);
    let (href, name, placed) =
        streams.get(idx).cloned().ok_or_else(|| IoError::Format(format!("page {page} not found (the file has {pages} page(s))")))?;
    let stream = zip_read(zipped, &href)?;
    let mut p = dwf6_page(&stream, name, placed, pages)?;
    p.pages = pages;
    Ok(p)
}

fn dwf6_page(stream: &[u8], name: Option<String>, placed: Option<(Vec2, f64, Vec2)>, pages: usize) -> Result<Page> {
    let drawn = w2d::read(stream).map_err(IoError::Format)?;
    let mut d = Drawing::new_imperial();
    for it in drawn {
        if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&it.layer)) {
            d.layers.push(Layer::new(&it.layer));
        }
        let common = Common { layer: it.layer.as_str().into(), color: cadaddy_color::Color::True(it.rgb), ..Common::default() };
        let (pts, closed) = match it.shape {
            w2d::Shape::Line(p) => (p, false),
            w2d::Shape::Fill(p) => (p, true),
        };
        let vertices = pts.into_iter().map(|p| PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
        d.add(&Space::Model, common, EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
            .map_err(|e| IoError::Format(e.to_string()))?;
    }
    only_used_layers(&mut d);
    let ext = d.extents(&Space::Model);
    let size = match placed {
        Some((_, _, s)) if s.x > 0.0 && s.y > 0.0 => s,
        _ if !ext.is_empty() => ext.max,
        _ => Vec2::new(8.5, 11.0),
    };
    Ok(Page { drawing: d, size, pages, name, placement: placed.map(|(o, s, _)| (o, s)) })
}

#[cfg(test)]
mod tests;
