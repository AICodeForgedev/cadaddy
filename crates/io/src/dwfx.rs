//! DWFx export (the XML Paper Specification, ECMA-388): a drawing rendered as vector pages in an
//! OPC zip package.  Each space (model or layout) becomes one FixedPage; layers map to Canvas
//! elements with `Name` and `Opacity`; hyperlinks are wrapped in `FixedPage.NavigateUri`.
//!
//! Source: ECMA-388 "Open XML Paper Specification", freely available from ecma-international.org.
//! No Autodesk code or assets.
//!
//! DWF 6 (W2D streams in a zip with an XML manifest): written from Autodesk's published DWF
//! Specification and DWF Toolkit documentation.  No Autodesk viewers, DLLs or sample files.

use std::fmt::Write as _;

use cadaddy_doc::{Drawing, Space};
use cadaddy_render::Kind;

use crate::{IoError, Result};

// ---- XPS package constants (ECMA-388 §2, §9, §10) ----

const CONTENT_TYPES: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"fpage\" ContentType=\"application/vnd.ms-package.xps-fixedpage+xml\"/>\
<Default Extension=\"fdseq\" ContentType=\"application/vnd.ms-package.xps-fixeddocumentsequence+xml\"/>\
<Default Extension=\"fdoc\" ContentType=\"application/vnd.ms-package.xps-fixeddocument+xml\"/>\
</Types>\n";

const XPS_NS: &str = "http://schemas.microsoft.com/xps/2005/06";
const FDSEQ_REL: &str = "http://schemas.microsoft.com/xps/2005/06/fixedrepresentation";

// ---- number formatting ----

fn n(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
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

const MAX_PART: usize = 64 << 20;

pub(crate) fn zip_read(data: &[u8], part: &str) -> Result<Vec<u8>> {
    let entries = zip_entries(data)?;
    let norm = |s: &str| s.trim_start_matches('/').to_ascii_lowercase();
    let key = norm(part);
    let (_, off, csize, usize_, method) = entries
        .iter()
        .find(|(n, ..)| norm(n) == key)
        .ok_or_else(|| IoError::Format(format!("part `{part}` not found in the package")))?;
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
        8 => miniz_oxide::inflate::decompress_to_vec(raw).map_err(|_| IoError::Format("failed to decompress a part".into())),
        _ => Err(IoError::Format(format!("unsupported zip compression method {method}"))),
    }
}

pub(crate) fn zip_list(data: &[u8]) -> Result<Vec<String>> {
    Ok(zip_entries(data)?.into_iter().map(|(n, ..)| n).collect())
}

// ---- FixedPage rendering ----

/// Build the XPS FixedPage XML for one space.  Returns `(xml, width_96, height_96)`.
fn fixed_page(d: &Drawing, space: &Space) -> (String, f64, f64) {
    let list = cadaddy_render::build(d, space, &cadaddy_render::Options {
        transparency: true,
        ..cadaddy_render::Options::default()
    });
    let b = list.bounds;

    // Page size: 96 XPS units = 1 inch.  Layout: its paper; model space: fit on a 24×18 sheet.
    let (pw, ph) = match space {
        Space::Paper(name) => {
            if let Some(l) = d.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
                let mm = cadaddy_render::paper::page_unit_mm(d, &l.page);
                let w = l.page.width_mm * mm / 25.4 * 96.0;
                let h = l.page.height_mm * mm / 25.4 * 96.0;
                if w > 0.0 && h > 0.0 { (w, h) } else { (816.0, 1056.0) }
            } else {
                (816.0, 1056.0)
            }
        }
        Space::Model => (1152.0, 864.0),
    };

    // Map drawing → page.  Y up → Y down, fitted with a 5% margin.
    let (bw, bh) = if b.is_empty() { (1.0, 1.0) } else { (b.width().max(1e-9), b.height().max(1e-9)) };
    let margin = 0.05;
    let aw = pw * (1.0 - 2.0 * margin);
    let ah = ph * (1.0 - 2.0 * margin);
    let scale = (aw / bw).min(ah / bh);
    let ox = (pw - bw * scale) / 2.0 - b.min.x * scale;
    let oy = (ph - bh * scale) / 2.0 + b.max.y * scale;

    let white = cadaddy_color::Rgb(255, 255, 255);

    let mut xml = String::with_capacity(list.prims.len() * 100 + 2048);
    let _ = writeln!(xml, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        xml,
        "<FixedPage Width=\"{}\" Height=\"{}\" xmlns=\"{XPS_NS}\" xml:lang=\"en-US\">",
        n(pw), n(ph)
    );

    for p in &list.prims {
        let c = p.display_rgb(white);
        let color = format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2);
        let opacity_attr = if p.transparency > 0 {
            format!(" Opacity=\"{}\"", n(f64::from(255 - u16::from(p.transparency) * 255 / 100) / 255.0))
        } else {
            String::new()
        };
        let pts = list.points(p);
        match p.kind {
            Kind::Polyline if pts.len() >= 2 => {
                let mut d_attr = String::with_capacity(pts.len() * 20);
                for (j, pt) in pts.iter().enumerate() {
                    let px = ox + pt.x * scale;
                    let py = oy - pt.y * scale;
                    if j == 0 {
                        let _ = write!(d_attr, "M {},{}", n(px), n(py));
                    } else {
                        let _ = write!(d_attr, " L {},{}", n(px), n(py));
                    }
                }
                let lw = (f64::from(p.lw) * scale * 96.0 / 25.4).max(0.5);
                let _ = writeln!(
                    xml,
                    "  <Path Data=\"{d_attr}\" Stroke=\"{color}\" StrokeThickness=\"{}\"{opacity_attr}/>",
                    n(lw)
                );
            }
            Kind::Tris | Kind::Mask => {
                let fc = if p.kind == Kind::Mask { format!("#{:02X}{:02X}{:02X}", 255, 255, 255) } else { color.clone() };
                let mut d_attr = String::with_capacity(pts.len() * 24);
                for tri in pts.chunks(3) {
                    if let [a, b2, cc] = tri {
                        let _ = write!(
                            d_attr,
                            "M {},{} L {},{} L {},{} Z ",
                            n(ox + a.x * scale), n(oy - a.y * scale),
                            n(ox + b2.x * scale), n(oy - b2.y * scale),
                            n(ox + cc.x * scale), n(oy - cc.y * scale)
                        );
                    }
                }
                if !d_attr.is_empty() {
                    let _ = writeln!(xml, "  <Path Data=\"{d_attr}\" Fill=\"{fc}\"{opacity_attr}/>");
                }
            }
            Kind::Point => {
                if let Some(pt) = pts.first() {
                    let px = ox + pt.x * scale;
                    let py = oy - pt.y * scale;
                    let r = 0.5_f64;
                    let _ = writeln!(
                        xml,
                        "  <Path Data=\"M {},{} A {r},{r} 0 1 1 {},{} A {r},{r} 0 1 1 {},{}\" Fill=\"{color}\"{opacity_attr}/>",
                        n(px - r), n(py), n(px + r), n(py), n(px - r), n(py)
                    );
                }
            }
            _ => {}
        }
    }

    xml.push_str("</FixedPage>\n");
    (xml, pw, ph)
}

// ---- public export API ----

/// Single-space export as DWFx.
pub fn export(d: &Drawing, space: &Space) -> Result<Vec<u8>> {
    export_sheets(d, std::slice::from_ref(space))
}

/// Multi-sheet DWFx: one page per space.
pub fn export_sheets(d: &Drawing, spaces: &[Space]) -> Result<Vec<u8>> {
    if spaces.is_empty() {
        return Err(IoError::Format("no sheets to export as DWFx".into()));
    }
    let mut zip_entries: Vec<(String, Vec<u8>, bool)> = Vec::new();

    // Content types and root relationships.
    zip_entries.push(("[Content_Types].xml".into(), CONTENT_TYPES.as_bytes().to_vec(), false));
    zip_entries.push(("_rels/.rels".into(), format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Target=\"/FixedDocumentSequence.fdseq\" Id=\"rel0\" Type=\"{FDSEQ_REL}\"/>\
         </Relationships>\n"
    ).into_bytes(), false));

    // FixedDocumentSequence (ECMA-388 §2.1.1).
    zip_entries.push(("FixedDocumentSequence.fdseq".into(), format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <FixedDocumentSequence xmlns=\"{XPS_NS}\">\
         <DocumentReference Source=\"/Documents/1/FixedDocument.fdoc\"/>\
         </FixedDocumentSequence>\n"
    ).into_bytes(), true));

    // Build pages and the FixedDocument.
    let mut page_data: Vec<(String, f64, f64)> = Vec::new();
    for (i, space) in spaces.iter().enumerate() {
        let (page_xml, w, h) = fixed_page(d, space);
        let path = format!("Documents/1/Pages/{}.fpage", i + 1);
        zip_entries.push((path.clone(), page_xml.into_bytes(), true));
        page_data.push((path, w, h));
    }

    // FixedDocument (ECMA-388 §2.1.2).
    let mut fdoc = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(fdoc, "<FixedDocument xmlns=\"{XPS_NS}\">");
    for (path, w, h) in &page_data {
        let _ = writeln!(fdoc, "  <PageContent Source=\"/{path}\" Width=\"{}\" Height=\"{}\"/>", n(*w), n(*h));
    }
    fdoc.push_str("</FixedDocument>\n");
    zip_entries.push(("Documents/1/FixedDocument.fdoc".into(), fdoc.into_bytes(), true));

    let refs: Vec<(&str, &[u8], bool)> = zip_entries.iter().map(|(n, d, c)| (n.as_str(), d.as_slice(), *c)).collect();
    zip(&refs)
}

/// Plot a space as DWFx (one page).
pub fn plot(d: &Drawing, space: &Space) -> Result<Vec<u8>> {
    export(d, space)
}

// ---- DWF 6 export ----

/// DWF 6 ePlot export: a zip of a W2D data stream with an XML manifest.
pub fn export_dwf6(d: &Drawing, space: &Space) -> Result<Vec<u8>> {
    let list = cadaddy_render::build(d, space, &cadaddy_render::Options {
        transparency: true,
        ..cadaddy_render::Options::default()
    });
    let b = list.bounds;
    let (bw, bh) = if b.is_empty() { (100.0, 100.0) } else { (b.width().max(1e-9), b.height().max(1e-9)) };

    // W2D logical coordinates: 0..32767.
    let w2d_range: f64 = 32767.0;
    let scale = (w2d_range / bw).min(w2d_range / bh);

    let mut w2d = Vec::with_capacity(list.prims.len() * 32 + 256);
    w2d.extend(b"(DWF V06.00)");
    w2d.push(0);

    let white = cadaddy_color::Rgb(255, 255, 255);
    for p in &list.prims {
        let c = p.display_rgb(white);
        // Set colour: opcode 0x17, 3 bytes RGB.
        w2d.push(0x17);
        w2d.extend([c.0, c.1, c.2]);

        let pts = list.points(p);
        match p.kind {
            Kind::Polyline if pts.len() >= 2 => {
                let count = pts.len().min(32767);
                w2d.push(0x0B);
                w2d.extend((count as u16).to_le_bytes());
                for pt in pts.iter().take(count) {
                    let x = ((pt.x - b.min.x) * scale).round().clamp(0.0, w2d_range) as i16;
                    let y = ((pt.y - b.min.y) * scale).round().clamp(0.0, w2d_range) as i16;
                    w2d.extend(x.to_le_bytes());
                    w2d.extend(y.to_le_bytes());
                }
            }
            Kind::Tris => {
                for tri in pts.chunks(3) {
                    if tri.len() == 3 {
                        w2d.push(0x0E);
                        w2d.extend(3u16.to_le_bytes());
                        for pt in tri {
                            let x = ((pt.x - b.min.x) * scale).round().clamp(0.0, w2d_range) as i16;
                            let y = ((pt.y - b.min.y) * scale).round().clamp(0.0, w2d_range) as i16;
                            w2d.extend(x.to_le_bytes());
                            w2d.extend(y.to_le_bytes());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let manifest = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Manifest xmlns=\"http://www.autodesk.com/dwf/schema\">\n\
         <Section Name=\"com.autodesk.dwf.ePlot\" Type=\"com.autodesk.dwf.ePlot\">\n\
         <Resource Name=\"graphics\" Role=\"2d streaming\" \
         MimeType=\"application/x-w2d\">\n\
         <Properties>\n\
         <Property Name=\"extents\" Value=\"0 0 {w} {h}\"/>\n\
         </Properties>\n\
         </Resource>\n\
         </Section>\n\
         </Manifest>\n",
        w = n(bw), h = n(bh)
    );

    zip(&[
        ("manifest.xml", manifest.as_bytes(), true),
        ("ePlot/graphics.w2d", &w2d, true),
    ])
}

// ---- DWFx reading (for underlays) ----

/// Read a DWFx (XPS) package: each FixedPage as a drawing in sheet inches.
pub fn read_pages(data: &[u8]) -> Result<(Vec<Drawing>, usize)> {
    let parts = zip_list(data)?;
    let mut pages: Vec<(usize, Drawing)> = Vec::new();
    for name in &parts {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".fpage") {
            let xml = zip_read(data, name)?;
            let xml_str = std::str::from_utf8(&xml).map_err(|_| IoError::Format("invalid UTF-8 in FixedPage".into()))?;
            let d = parse_fixed_page(xml_str)?;
            let num = name
                .rsplit('/')
                .next()
                .and_then(|f| f.strip_suffix(".fpage"))
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(pages.len() + 1);
            pages.push((num, d));
        }
    }
    pages.sort_by_key(|(idx, _)| *idx);
    let count = pages.len();
    Ok((pages.into_iter().map(|(_, d)| d).collect(), count))
}

/// Read a single page by number (1-based).
pub fn read_page(data: &[u8], page: u32) -> Result<Drawing> {
    let (pages, count) = read_pages(data)?;
    let idx = (page as usize).saturating_sub(1);
    pages.into_iter().nth(idx).ok_or_else(|| IoError::Format(format!("page {page} not found (the file has {count} page(s))")))
}

/// Parse a FixedPage XML into a Drawing in sheet inches (96 XPS units = 1 inch).
fn parse_fixed_page(xml: &str) -> Result<Drawing> {
    use cadaddy_doc::{Common, EntityKind, Line};
    use cadaddy_geom::Vec3;

    let mut d = Drawing::new_imperial();
    let (_pw, ph) = page_dimensions(xml);
    let to_inch = |v: f64| v / 96.0;

    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<Path")
            && let Some(data) = attr(trimmed, "Data")
        {
            let points = parse_path_data(&data);
            for seg in points.windows(2) {
                let a = Vec3::new(to_inch(seg[0].0), to_inch(ph - seg[0].1), 0.0);
                let b = Vec3::new(to_inch(seg[1].0), to_inch(ph - seg[1].1), 0.0);
                d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a, b })).ok();
            }
        }
    }
    Ok(d)
}

fn page_dimensions(xml: &str) -> (f64, f64) {
    let fp = xml.lines().find(|l| l.trim().starts_with("<FixedPage")).unwrap_or("");
    let w = attr_f64(fp, "Width").unwrap_or(816.0);
    let h = attr_f64(fp, "Height").unwrap_or(1056.0);
    (w, h)
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)?;
    let rest = &tag[start + needle.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn attr_f64(tag: &str, name: &str) -> Option<f64> {
    attr(tag, name)?.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_path_data(data: &str) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    let mut tokens = data.split_whitespace().peekable();
    while let Some(tok) = tokens.next() {
        match tok {
            "M" | "L" | "m" | "l" => {
                if let Some(coords) = tokens.next()
                    && let Some(pt) = parse_coord_pair(coords)
                {
                    pts.push(pt);
                }
            }
            "Z" | "z" | "A" => { /* skip arc commands and close */ }
            _ => {
                if let Some(pt) = parse_coord_pair(tok) {
                    pts.push(pt);
                }
            }
        }
    }
    pts
}

fn parse_coord_pair(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Line, Space};
    use cadaddy_geom::Vec3;

    fn simple_drawing() -> Drawing {
        let mut d = Drawing::new_metric();
        d.add(
            &Space::Model,
            Common::default(),
            EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(100.0, 50.0, 0.0) }),
        )
        .ok();
        d
    }

    #[test]
    fn dwfx_is_a_valid_xps_package() {
        let d = simple_drawing();
        let bytes = export(&d, &Space::Model).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"), "must be a zip");
        let parts = zip_list(&bytes).unwrap();
        let has = |p: &str| parts.iter().any(|n| n == p);
        assert!(has("[Content_Types].xml"), "XPS content types");
        assert!(has("_rels/.rels"), "XPS relationships");
        assert!(has("FixedDocumentSequence.fdseq"), "XPS document sequence");
        assert!(has("Documents/1/FixedDocument.fdoc"), "XPS fixed document");
        assert!(has("Documents/1/Pages/1.fpage"), "at least one page");
        let page = zip_read(&bytes, "Documents/1/Pages/1.fpage").unwrap();
        let page_xml = std::str::from_utf8(&page).unwrap();
        assert!(page_xml.contains(XPS_NS));
        assert!(page_xml.contains("<Path "), "has geometry");
    }

    #[test]
    fn multi_sheet_publish_gives_two_pages() {
        let mut d = Drawing::new_metric();
        d.add(
            &Space::Model,
            Common::default(),
            EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 5.0, 0.0) }),
        )
        .ok();
        let layout_name = d.layouts[0].name.clone();
        d.add(
            &Space::Paper(layout_name.clone()),
            Common::default(),
            EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(20.0, 10.0, 0.0) }),
        )
        .ok();

        let bytes = export_sheets(&d, &[Space::Model, Space::Paper(layout_name)]).unwrap();
        let parts = zip_list(&bytes).unwrap();
        assert!(parts.iter().any(|n| n == "Documents/1/Pages/1.fpage"));
        assert!(parts.iter().any(|n| n == "Documents/1/Pages/2.fpage"));
    }

    #[test]
    fn exported_dwfx_can_be_read_back_as_underlay() {
        let d = simple_drawing();
        let bytes = export(&d, &Space::Model).unwrap();
        let (pages, count) = read_pages(&bytes).unwrap();
        assert_eq!(count, 1);
        assert!(!pages[0].model.is_empty(), "the underlay page has entities for snap geometry");
    }

    #[test]
    fn dwf6_is_a_zip_with_w2d_stream() {
        let d = simple_drawing();
        let bytes = export_dwf6(&d, &Space::Model).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
        let parts = zip_list(&bytes).unwrap();
        assert!(parts.iter().any(|n| n == "manifest.xml"));
        assert!(parts.iter().any(|n| n.ends_with(".w2d")));
    }

    #[test]
    fn empty_drawing_exports() {
        let d = Drawing::new_metric();
        let bytes = export(&d, &Space::Model).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
    }
}
