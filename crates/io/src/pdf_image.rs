//! Raster images in PDF content (ISO 32000 §8.9): image XObjects and inline images (`BI … ID …
//! EI`) decoded to RGBA pixels for PDFIMPORT.
//!
//! Handled: the generic filters (Flate with predictors, LZW, ASCII85, ASCIIHex, RunLength, via
//! lopdf, bounded), JPEG (DCTDecode); DeviceGray/RGB/CMYK, CalGray/CalRGB, ICCBased (by its
//! component count), Indexed, Separation and DeviceN (approximated); 1, 2, 4, 8 and 16 bits per
//! component; Decode arrays; stencil masks (ImageMask, painted in the fill colour); soft masks
//! (SMask), explicit masks and colour-key masks. JPEG 2000, JBIG2 and CCITT fax images are
//! reported as not converted.
//!
//! lopdf's content parser skips inline images that carry a filter, so [`scan_inline`] reads
//! them from the content bytes itself, in order; the n-th `BI` operation is the n-th image.

use cadaddy_render::images::{MAX_PIXELS, Raster};
use lopdf::{Dictionary, Document, Object, Stream};

/// Decoded bytes one image may take (before conversion to RGBA).
const MAX_DECODED: usize = 512 << 20;

/// Why an image wasn't converted (for the import notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Unsupported {
    Jpeg2000,
    Jbig2,
    Fax,
    /// Too large, damaged, or a colour space or filter not handled.
    Other,
}

impl Unsupported {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Unsupported::Jpeg2000 => "JPEG 2000",
            Unsupported::Jbig2 => "JBIG2",
            Unsupported::Fax => "CCITT fax",
            Unsupported::Other => "damaged or unsupported",
        }
    }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

fn int(o: &Object) -> Option<i64> {
    match o {
        Object::Integer(i) => Some(*i),
        Object::Real(r) if r.is_finite() => Some(*r as i64),
        _ => None,
    }
}

fn real(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)).filter(|v| v.is_finite()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Inline images
// ---------------------------------------------------------------------------------------------

/// Inline images' full key names for the abbreviations (§8.9.7, table 92).
fn full_key(k: &[u8]) -> &[u8] {
    match k {
        b"BPC" => b"BitsPerComponent",
        b"CS" => b"ColorSpace",
        b"D" => b"Decode",
        b"DP" => b"DecodeParms",
        b"F" => b"Filter",
        b"H" => b"Height",
        b"IM" => b"ImageMask",
        b"I" => b"Interpolate",
        b"W" => b"Width",
        b"L" => b"Length",
        k => k,
    }
}

/// Full names for abbreviated filter and colour space names (table 93).
fn full_name(n: &[u8]) -> &[u8] {
    match n {
        b"AHx" => b"ASCIIHexDecode",
        b"A85" => b"ASCII85Decode",
        b"LZW" => b"LZWDecode",
        b"Fl" => b"FlateDecode",
        b"RL" => b"RunLengthDecode",
        b"CCF" => b"CCITTFaxDecode",
        b"DCT" => b"DCTDecode",
        b"G" => b"DeviceGray",
        b"RGB" => b"DeviceRGB",
        b"CMYK" => b"DeviceCMYK",
        b"I" => b"Indexed",
        n => n,
    }
}

fn expand_names(o: Object) -> Object {
    match o {
        Object::Name(n) => Object::Name(full_name(&n).to_vec()),
        Object::Array(a) => Object::Array(a.into_iter().map(expand_names).collect()),
        o => o,
    }
}

/// A tiny tokenizer over content bytes: enough for inline image dictionaries and for finding
/// `BI` operators outside strings and comments.
struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

fn is_white(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | 0)
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn skip_space(&mut self) {
        while let Some(c) = self.peek() {
            if is_white(c) {
                self.i += 1;
            } else if c == b'%' {
                while self.peek().is_some_and(|c| c != b'\n' && c != b'\r') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }
    /// A literal string `( … )` (escapes and nesting), returned raw-unescaped enough for lookups.
    fn string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        self.i += 1;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\\' => {
                    let Some(e) = self.peek() else { break };
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xff) as u8);
                        }
                        b'\r' | b'\n' => {}
                        e => out.push(e),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' if depth == 0 => break,
                b')' => {
                    depth -= 1;
                    out.push(c);
                }
                c => out.push(c),
            }
        }
        out
    }
    /// A regular token (name body, number, keyword).
    fn word(&mut self) -> &[u8] {
        let s = self.i;
        while self.peek().is_some_and(|c| !is_white(c) && !is_delim(c)) {
            self.i += 1;
        }
        self.b.get(s..self.i).unwrap_or(&[])
    }
    /// One object, `depth` levels deep at most.
    fn object(&mut self, depth: usize) -> Option<Object> {
        if depth > 16 {
            return None;
        }
        self.skip_space();
        match self.peek()? {
            b'/' => {
                self.i += 1;
                Some(Object::Name(self.word().to_vec()))
            }
            b'(' => Some(Object::String(self.string(), lopdf::StringFormat::Literal)),
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.skip_space();
                    match self.peek()? {
                        b']' => {
                            self.i += 1;
                            return Some(Object::Array(a));
                        }
                        _ => a.push(self.object(depth + 1)?),
                    }
                    if a.len() > 100_000 {
                        return None;
                    }
                }
            }
            b'<' if self.b.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut d = Dictionary::new();
                loop {
                    self.skip_space();
                    if self.b.get(self.i..self.i + 2) == Some(b">>") {
                        self.i += 2;
                        return Some(Object::Dictionary(d));
                    }
                    let Object::Name(k) = self.object(depth + 1)? else { return None };
                    let v = self.object(depth + 1)?;
                    d.set(k, v);
                }
            }
            b'<' => {
                self.i += 1;
                let mut hex = Vec::new();
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == b'>' {
                        break;
                    }
                    if c.is_ascii_hexdigit() {
                        hex.push(c);
                    }
                }
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                let bytes = hex.chunks(2).filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect();
                Some(Object::String(bytes, lopdf::StringFormat::Hexadecimal))
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.i += 1;
                    return Some(Object::Null);
                }
                let s = std::str::from_utf8(w).ok()?;
                Some(match s {
                    "true" => Object::Boolean(true),
                    "false" => Object::Boolean(false),
                    "null" => Object::Null,
                    s => match s.parse::<i64>() {
                        Ok(i) => Object::Integer(i),
                        Err(_) => Object::Real(s.parse::<f32>().ok()?),
                    },
                })
            }
        }
    }
}

/// The inline images of a content stream, in order: (dictionary with full key and name
/// spellings, the bytes between `ID` and `EI`). An image whose dictionary can't be read is
/// `None`, so later images keep their position.
pub(crate) fn scan_inline(data: &[u8], resources: Option<&Dictionary>, doc: &Document) -> Vec<Option<(Dictionary, Vec<u8>)>> {
    let mut out = Vec::new();
    // fast exit: most content streams have none
    if !data.windows(2).any(|w| w == b"BI") {
        return out;
    }
    let mut lx = Lexer { b: data, i: 0 };
    while lx.i < data.len() && out.len() < 1_000_000 {
        lx.skip_space();
        let Some(c) = lx.peek() else { break };
        match c {
            b'(' => {
                lx.string();
            }
            b'<' | b'[' | b'/' => {
                if lx.object(0).is_none() {
                    lx.i += 1;
                }
            }
            b']' | b'>' | b')' | b'{' | b'}' => lx.i += 1,
            _ => {
                if lx.word() == b"BI" {
                    out.push(inline_image(&mut lx, resources, doc));
                }
            }
        }
    }
    out
}

/// After `BI`: the dictionary up to `ID`, then the data up to `EI`.
fn inline_image(lx: &mut Lexer, resources: Option<&Dictionary>, doc: &Document) -> Option<(Dictionary, Vec<u8>)> {
    let mut d = Dictionary::new();
    loop {
        lx.skip_space();
        if lx.b.get(lx.i..lx.i + 2) == Some(b"ID") && lx.b.get(lx.i + 2).is_none_or(|c| is_white(*c)) {
            lx.i += 2;
            // one white-space byte separates ID from the data
            if lx.peek().is_some_and(is_white) {
                lx.i += 1;
            }
            break;
        }
        let Some(Object::Name(k)) = lx.object(0) else {
            skip_to_ei(lx);
            return None;
        };
        let v = lx.object(0)?;
        d.set(full_key(&k).to_vec(), expand_names(v));
    }
    let start = lx.i;
    // unfiltered data has a known length (as lopdf reads it); filtered data ends at EI
    let len = if d.get(b"Filter").is_err() { raw_length(&d, resources, doc) } else { None };
    let end = match len.and_then(|n| start.checked_add(n)).filter(|e| *e <= lx.b.len()) {
        Some(e) => {
            lx.i = e;
            skip_to_ei(lx);
            e
        }
        None => skip_to_ei(lx),
    };
    Some((d, lx.b.get(start..end).unwrap_or(&[]).to_vec()))
}

/// Move past the next `EI` that stands between white space; returns where its white space
/// starts (the end of the data).
fn skip_to_ei(lx: &mut Lexer) -> usize {
    let b = lx.b;
    let mut i = lx.i;
    while i + 2 < b.len() {
        if b.get(i).is_some_and(|c| is_white(*c))
            && b.get(i + 1) == Some(&b'E')
            && b.get(i + 2) == Some(&b'I')
            && b.get(i + 3).is_none_or(|c| is_white(*c))
        {
            lx.i = i + 3;
            return i;
        }
        i += 1;
    }
    lx.i = b.len();
    b.len()
}

/// The byte length of an unfiltered inline image's data.
fn raw_length(d: &Dictionary, resources: Option<&Dictionary>, doc: &Document) -> Option<usize> {
    let w = usize::try_from(d.get(b"Width").ok().and_then(int)?).ok()?;
    let h = usize::try_from(d.get(b"Height").ok().and_then(int)?).ok()?;
    let mask = matches!(d.get(b"ImageMask"), Ok(Object::Boolean(true)));
    let bpc = if mask { 1 } else { usize::try_from(d.get(b"BitsPerComponent").ok().and_then(int)?).ok()? };
    let n = if mask { 1 } else { ColorSpace::of(doc, d.get(b"ColorSpace").ok()?, resources, 0)?.components() };
    w.checked_mul(n)?.checked_mul(bpc)?.div_ceil(8).checked_mul(h)
}

// ---------------------------------------------------------------------------------------------
// Colour spaces
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    /// A palette of `base` colours, `hival + 1` entries.
    Indexed {
        base: Box<ColorSpace>,
        hival: usize,
        lookup: Vec<u8>,
    },
    /// One tint component (a spot colour): drawn as grey ink, 1 = full.
    Separation,
    /// `n` colourants: four are read as CMYK, others as grey ink of the first.
    DeviceN(usize),
    /// CIE Lab: L as grey.
    Lab,
}

impl ColorSpace {
    fn components(&self) -> usize {
        match self {
            ColorSpace::Gray | ColorSpace::Separation | ColorSpace::Indexed { .. } => 1,
            ColorSpace::Rgb | ColorSpace::Lab => 3,
            ColorSpace::Cmyk => 4,
            ColorSpace::DeviceN(n) => *n,
        }
    }

    fn of(doc: &Document, o: &Object, resources: Option<&Dictionary>, depth: usize) -> Option<ColorSpace> {
        if depth > 8 {
            return None;
        }
        let o = deref(doc, o);
        match o {
            Object::Name(n) => match full_name(n) {
                b"DeviceGray" | b"CalGray" => Some(ColorSpace::Gray),
                b"DeviceRGB" | b"CalRGB" => Some(ColorSpace::Rgb),
                b"DeviceCMYK" => Some(ColorSpace::Cmyk),
                // a named colour space of the resources
                name => {
                    let cs = resources.and_then(|r| get(doc, r, b"ColorSpace")).and_then(|c| c.as_dict().ok()).and_then(|c| c.get(name).ok())?;
                    ColorSpace::of(doc, cs, resources, depth + 1)
                }
            },
            Object::Array(a) => {
                let kind = a.first().map(|o| deref(doc, o)).and_then(|o| o.as_name().ok())?;
                match full_name(kind) {
                    b"DeviceGray" | b"CalGray" => Some(ColorSpace::Gray),
                    b"DeviceRGB" | b"CalRGB" => Some(ColorSpace::Rgb),
                    b"DeviceCMYK" => Some(ColorSpace::Cmyk),
                    b"Lab" => Some(ColorSpace::Lab),
                    b"ICCBased" => {
                        let Object::Stream(st) = deref(doc, a.get(1)?) else { return None };
                        match get(doc, &st.dict, b"N").and_then(int) {
                            Some(1) => Some(ColorSpace::Gray),
                            Some(3) => Some(ColorSpace::Rgb),
                            Some(4) => Some(ColorSpace::Cmyk),
                            _ => ColorSpace::of(doc, get(doc, &st.dict, b"Alternate")?, resources, depth + 1),
                        }
                    }
                    b"Indexed" => {
                        let base = ColorSpace::of(doc, a.get(1)?, resources, depth + 1)?;
                        if matches!(base, ColorSpace::Indexed { .. }) {
                            return None;
                        }
                        let hival = usize::try_from(int(deref(doc, a.get(2)?))?).ok()?.min(255);
                        let lookup = match deref(doc, a.get(3)?) {
                            Object::String(s, _) => s.clone(),
                            Object::Stream(st) => st.decompressed_content_with_limit(1 << 20).unwrap_or_else(|_| st.content.clone()),
                            _ => return None,
                        };
                        Some(ColorSpace::Indexed { base: Box::new(base), hival, lookup })
                    }
                    b"Separation" => Some(ColorSpace::Separation),
                    b"DeviceN" => {
                        let names = deref(doc, a.get(1)?).as_array().ok()?;
                        Some(ColorSpace::DeviceN(names.len().clamp(1, 32)))
                    }
                    b"Pattern" => None,
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// RGB of decoded component values (each 0..=1; for Indexed, the index).
    fn rgb(&self, c: &[f64]) -> [u8; 3] {
        let k = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
        match self {
            ColorSpace::Gray => [k(at(0)); 3],
            ColorSpace::Rgb => [k(at(0)), k(at(1)), k(at(2))],
            ColorSpace::Cmyk => cmyk(at(0), at(1), at(2), at(3)),
            ColorSpace::Lab => [k(at(0) / 100.0); 3],
            ColorSpace::Separation => [k(1.0 - at(0)); 3],
            ColorSpace::DeviceN(4) => cmyk(at(0), at(1), at(2), at(3)),
            ColorSpace::DeviceN(_) => [k(1.0 - at(0)); 3],
            ColorSpace::Indexed { base, hival, lookup } => {
                let i = (at(0).round().max(0.0) as usize).min(*hival);
                let n = base.components();
                let entry: Vec<f64> = (0..n).map(|j| f64::from(lookup.get(i * n + j).copied().unwrap_or(0)) / 255.0).collect();
                // a Lab palette holds L in 0..255 for 0..100
                if matches!(**base, ColorSpace::Lab) {
                    return [k(entry.first().copied().unwrap_or(0.0)); 3];
                }
                base.rgb(&entry)
            }
        }
    }

    /// The default Decode range of each component for `bpc`-bit samples.
    fn default_decode(&self, bpc: u32) -> Vec<(f64, f64)> {
        match self {
            ColorSpace::Indexed { .. } => vec![(0.0, f64::from((1u32 << bpc.min(16)) - 1))],
            ColorSpace::Lab => vec![(0.0, 100.0), (-100.0, 100.0), (-100.0, 100.0)],
            cs => vec![(0.0, 1.0); cs.components()],
        }
    }
}

fn cmyk(c: f64, m: f64, y: f64, k: f64) -> [u8; 3] {
    let f = |v: f64| ((1.0 - v.clamp(0.0, 1.0)) * (1.0 - k.clamp(0.0, 1.0)) * 255.0).round() as u8;
    [f(c), f(m), f(y)]
}

// ---------------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------------

/// The filters of an image dictionary, in decoding order.
fn filters(doc: &Document, d: &Dictionary) -> Vec<Vec<u8>> {
    match get(doc, d, b"Filter") {
        Some(Object::Name(n)) => vec![full_name(n).to_vec()],
        Some(Object::Array(a)) => a.iter().filter_map(|o| deref(doc, o).as_name().ok()).map(|n| full_name(n).to_vec()).collect(),
        _ => Vec::new(),
    }
}

/// What the generic filters leave: the samples, or JPEG file bytes when the last filter is
/// DCTDecode (`true`).
fn unfilter(doc: &Document, d: &Dictionary, raw: &[u8]) -> Result<(Vec<u8>, bool), Unsupported> {
    let fs = filters(doc, d);
    let (generic, last) = match fs.last().map(Vec::as_slice) {
        Some(b"DCTDecode") => (&fs[..fs.len() - 1], Some(true)),
        Some(b"JPXDecode") => return Err(Unsupported::Jpeg2000),
        Some(b"JBIG2Decode") => return Err(Unsupported::Jbig2),
        Some(b"CCITTFaxDecode") => return Err(Unsupported::Fax),
        _ => (fs.as_slice(), None),
    };
    let data = if generic.is_empty() {
        raw.to_vec()
    } else {
        let mut sd = Dictionary::new();
        sd.set("Filter", Object::Array(generic.iter().map(|f| Object::Name(f.clone())).collect()));
        if let Some(p) = get(doc, d, b"DecodeParms") {
            let p = match p {
                // one parameter dictionary per filter: the one for the (single) generic filter
                Object::Array(a) => a.first().map(|o| deref(doc, o).clone()).unwrap_or(Object::Null),
                p => p.clone(),
            };
            if matches!(p, Object::Dictionary(_)) {
                sd.set("DecodeParms", p);
            }
        }
        Stream::new(sd, raw.to_vec()).decompressed_content_with_limit(MAX_DECODED).map_err(|_| Unsupported::Other)?
    };
    Ok((data, last.is_some()))
}

/// Unpack `w × h` samples of `n` components at `bpc` bits (rows padded to bytes), as values
/// 0..=2^bpc-1 (16-bit samples keep their high byte's range, 0..=65535).
fn samples(data: &[u8], w: usize, h: usize, n: usize, bpc: u32) -> Option<Vec<u32>> {
    let row = w.checked_mul(n)?.checked_mul(bpc as usize)?.div_ceil(8);
    let mut out = Vec::with_capacity(w.checked_mul(h)?.checked_mul(n)?);
    for y in 0..h {
        let r = data.get(y * row..(y + 1) * row);
        for i in 0..w * n {
            let v = match r {
                None => 0,
                Some(r) => match bpc {
                    8 => u32::from(r.get(i).copied().unwrap_or(0)),
                    16 => u32::from(r.get(2 * i).copied().unwrap_or(0)) << 8 | u32::from(r.get(2 * i + 1).copied().unwrap_or(0)),
                    1 | 2 | 4 => {
                        let bit = i * bpc as usize;
                        let byte = r.get(bit / 8).copied().unwrap_or(0);
                        let shift = 8 - bpc as usize - bit % 8;
                        (u32::from(byte) >> shift) & ((1 << bpc) - 1)
                    }
                    _ => return None,
                },
            };
            out.push(v);
        }
    }
    Some(out)
}

/// The image's size, if sane.
fn size(doc: &Document, d: &Dictionary) -> Option<(usize, usize)> {
    let w = usize::try_from(get(doc, d, b"Width").and_then(int)?).ok()?;
    let h = usize::try_from(get(doc, d, b"Height").and_then(int)?).ok()?;
    (w > 0 && h > 0 && (w as u64) * (h as u64) <= MAX_PIXELS).then_some((w, h))
}

/// A 0/1 coverage mask (`true` = painted) from an image mask's samples and Decode array.
fn stencil(doc: &Document, d: &Dictionary, data: &[u8], w: usize, h: usize) -> Option<Vec<bool>> {
    let s = samples(data, w, h, 1, 1)?;
    // Decode [0 1] (default): 0 paints; [1 0]: 1 paints
    let inverted = get(doc, d, b"Decode").and_then(|o| o.as_array().ok()).and_then(|a| a.first().and_then(real)).is_some_and(|v| v > 0.5);
    Some(s.into_iter().map(|v| (v == 0) != inverted).collect())
}

/// Decode an image (`dict` with full key names, `raw` its stream bytes) to RGBA. `fill` is the
/// current fill colour, which a stencil mask paints with.
pub(crate) fn decode(doc: &Document, dict: &Dictionary, raw: &[u8], resources: Option<&Dictionary>, fill: [u8; 3]) -> Result<Raster, Unsupported> {
    let (w, h) = size(doc, dict).ok_or(Unsupported::Other)?;
    let (data, jpeg) = unfilter(doc, dict, raw)?;
    let image_mask = matches!(get(doc, dict, b"ImageMask"), Some(Object::Boolean(true)));
    let mut rgba = if image_mask {
        let m = stencil(doc, dict, &data, w, h).ok_or(Unsupported::Other)?;
        m.iter().flat_map(|on| if *on { [fill[0], fill[1], fill[2], 255] } else { [0; 4] }).collect::<Vec<u8>>()
    } else if jpeg {
        let r = cadaddy_render::images::decode(&data).ok_or(Unsupported::Other)?;
        if (r.width as usize, r.height as usize) != (w, h) {
            return Err(Unsupported::Other);
        }
        // Adobe CMYK JPEGs are stored inverted; a Decode of [1 0 …] says so too
        r.rgba
    } else {
        let cs = match get(doc, dict, b"ColorSpace") {
            Some(o) => ColorSpace::of(doc, o, resources, 0).ok_or(Unsupported::Other)?,
            None => ColorSpace::Gray,
        };
        let bpc = u32::try_from(get(doc, dict, b"BitsPerComponent").and_then(int).unwrap_or(8)).unwrap_or(8);
        if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
            return Err(Unsupported::Other);
        }
        let n = cs.components();
        let s = samples(&data, w, h, n, bpc).ok_or(Unsupported::Other)?;
        let max = f64::from((1u32 << bpc) - 1);
        let decode: Vec<(f64, f64)> = match get(doc, dict, b"Decode").and_then(|o| o.as_array().ok()) {
            Some(a) if a.len() >= 2 * n => {
                a.chunks(2).take(n).map(|p| (p.first().and_then(real).unwrap_or(0.0), p.get(1).and_then(real).unwrap_or(1.0))).collect()
            }
            _ => cs.default_decode(bpc),
        };
        // colour-key masking (§8.9.6.4): samples inside every component's range are left out
        let key: Option<Vec<(u32, u32)>> = match get(doc, dict, b"Mask") {
            Some(Object::Array(a)) if a.len() >= 2 * n => Some(
                a.chunks(2)
                    .take(n)
                    .map(|p| (p.first().and_then(int).unwrap_or(0).max(0) as u32, p.get(1).and_then(int).unwrap_or(0).max(0) as u32))
                    .collect(),
            ),
            _ => None,
        };
        let mut out = Vec::with_capacity(w * h * 4);
        let mut comps = vec![0.0; n];
        for px in s.chunks(n) {
            for (j, v) in px.iter().enumerate() {
                let (d0, d1) = decode.get(j).copied().unwrap_or((0.0, 1.0));
                if let Some(c) = comps.get_mut(j) {
                    *c = d0 + f64::from(*v) * (d1 - d0) / max;
                }
            }
            let [r, g, b] = cs.rgb(&comps);
            let masked = key.as_ref().is_some_and(|k| px.iter().zip(k).all(|(v, (lo, hi))| v >= lo && v <= hi));
            out.extend_from_slice(&[r, g, b, if masked { 0 } else { 255 }]);
        }
        out
    };
    // a soft mask (alpha) or an explicit stencil mask, resampled to the image when sizes differ
    let alpha_of = |st: &Stream, soft: bool| -> Option<Vec<u8>> {
        let (mw, mh) = size(doc, &st.dict)?;
        let (data, jpeg) = unfilter(doc, &st.dict, &st.content).ok()?;
        let a: Vec<u8> = if soft {
            if jpeg {
                cadaddy_render::images::decode(&data)?.rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
            } else {
                let bpc = u32::try_from(get(doc, &st.dict, b"BitsPerComponent").and_then(int).unwrap_or(8)).ok()?;
                let max = (1u32 << bpc.min(16)) - 1;
                samples(&data, mw, mh, 1, bpc)?.into_iter().map(|v| (v * 255 / max.max(1)) as u8).collect()
            }
        } else {
            stencil(doc, &st.dict, &data, mw, mh)?.into_iter().map(|on| if on { 255 } else { 0 }).collect()
        };
        Some((0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| a.get((y * mh / h) * mw + x * mw / w).copied().unwrap_or(255)).collect())
    };
    let mask = match get(doc, dict, b"SMask") {
        Some(Object::Stream(st)) => alpha_of(st, true),
        _ => match get(doc, dict, b"Mask") {
            Some(Object::Stream(st)) => alpha_of(st, false),
            _ => None,
        },
    };
    if let Some(m) = mask {
        for (px, a) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(m) {
            if let Some(p) = px.get_mut(3) {
                *p = ((u32::from(*p) * u32::from(a)) / 255) as u8;
            }
        }
    }
    Raster::new(u32::try_from(w).map_err(|_| Unsupported::Other)?, u32::try_from(h).map_err(|_| Unsupported::Other)?, rgba).ok_or(Unsupported::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::with_version("1.5")
    }

    #[test]
    fn inline_images_are_found_in_order_with_their_data() {
        let content =
            b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /G /BPC 8 ID \x00\xff EI Q (BI in a string) Tj BI /W 1 /H 1 /CS /RGB /BPC 8 /F /AHx ID ff0000> EI";
        let d = doc();
        let found = scan_inline(content, None, &d);
        assert_eq!(found.len(), 2, "the BI inside the string is not an image");
        let (d0, data0) = found[0].as_ref().unwrap();
        assert_eq!(data0, &vec![0x00, 0xff]);
        assert_eq!(d0.get(b"ColorSpace").unwrap().as_name().unwrap(), b"DeviceGray");
        let r = decode(&d, d0, data0, None, [0, 0, 0]).unwrap();
        assert_eq!(r.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(r.pixel(1, 0), Some([255, 255, 255, 255]));
        let (d1, data1) = found[1].as_ref().unwrap();
        assert_eq!(decode(&d, d1, data1, None, [0, 0, 0]).unwrap().pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn flate_inline_images_with_predictors() {
        // one RGB row of 2 pixels, PNG predictor "None" (tag 0) per row, deflated
        let row = [0u8, 10, 20, 30, 40, 50, 60];
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&row, 6);
        let mut content = b"BI /W 2 /H 1 /CS /RGB /BPC 8 /F /Fl /DP << /Predictor 15 /Columns 2 /Colors 3 >> ID ".to_vec();
        content.extend_from_slice(&z);
        content.extend_from_slice(b"\nEI Q");
        let d = doc();
        let found = scan_inline(&content, None, &d);
        let (dict, data) = found[0].as_ref().unwrap();
        let r = decode(&d, dict, data, None, [0, 0, 0]).unwrap();
        assert_eq!(r.pixel(1, 0), Some([40, 50, 60, 255]));
    }

    #[test]
    fn stencil_masks_paint_the_fill_colour() {
        let mut dict = Dictionary::new();
        dict.set("Width", 8);
        dict.set("Height", 1);
        dict.set("ImageMask", true);
        // 0 paints
        let r = decode(&doc(), &dict, &[0b0000_1111], None, [200, 0, 0]).unwrap();
        assert_eq!(r.pixel(0, 0), Some([200, 0, 0, 255]));
        assert_eq!(r.pixel(7, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn indexed_cmyk_and_decode_arrays() {
        let d = doc();
        let mut dict = Dictionary::new();
        dict.set("Width", 2);
        dict.set("Height", 1);
        dict.set("BitsPerComponent", 1);
        dict.set(
            "ColorSpace",
            Object::Array(vec![
                Object::Name(b"Indexed".to_vec()),
                Object::Name(b"DeviceRGB".to_vec()),
                Object::Integer(1),
                Object::String(vec![0, 0, 255, 255, 255, 0], lopdf::StringFormat::Literal),
            ]),
        );
        let r = decode(&d, &dict, &[0b0100_0000], None, [0, 0, 0]).unwrap();
        assert_eq!((r.pixel(0, 0), r.pixel(1, 0)), (Some([0, 0, 255, 255]), Some([255, 255, 0, 255])));
        let mut cmyk = Dictionary::new();
        cmyk.set("Width", 1);
        cmyk.set("Height", 1);
        cmyk.set("BitsPerComponent", 8);
        cmyk.set("ColorSpace", Object::Name(b"DeviceCMYK".to_vec()));
        assert_eq!(decode(&d, &cmyk, &[0, 0, 0, 255], None, [0; 3]).unwrap().pixel(0, 0), Some([0, 0, 0, 255]));
        cmyk.set("Decode", Object::Array([1, 0, 1, 0, 1, 0, 1, 0].iter().map(|v| Object::Integer(*v)).collect()));
        assert_eq!(decode(&d, &cmyk, &[255, 255, 255, 255], None, [0; 3]).unwrap().pixel(0, 0), Some([255, 255, 255, 255]), "inverted");
    }

    #[test]
    fn unsupported_and_hostile_images_are_errors() {
        let d = doc();
        let mut dict = Dictionary::new();
        dict.set("Width", 1);
        dict.set("Height", 1);
        dict.set("Filter", Object::Name(b"JBIG2Decode".to_vec()));
        assert_eq!(decode(&d, &dict, &[], None, [0; 3]).unwrap_err(), Unsupported::Jbig2);
        dict.set("Width", 1_000_000);
        dict.set("Height", 1_000_000);
        assert_eq!(decode(&d, &dict, &[], None, [0; 3]).unwrap_err(), Unsupported::Other);
        assert!(scan_inline(b"BI /W", None, &d).iter().all(Option::is_none));
        assert!(scan_inline(b"BI /W 1 /H 1 /BPC 8 /CS /G ID", None, &d).len() <= 1);
    }
}
