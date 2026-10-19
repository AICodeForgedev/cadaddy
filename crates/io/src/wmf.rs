//! WMF export (Windows Metafile, vector) with a placeable header, for programs that take
//! metafiles: the display list the SVG and PDF exports draw, in 16-bit logical units (the picture
//! fitted into 0…30000 on its longer side, 1440 units to the inch at 10 inches across). Pens
//! carry colour and lineweight, brushes fill areas and wipeouts (white); images and text draw as
//! the SVG export shows them (text as its strokes; images are left out — metafiles of this kind
//! hold device-dependent bitmaps only).
//!
//! Records, from the public description of the format: META_SETWINDOWORG / META_SETWINDOWEXT,
//! META_CREATEPENINDIRECT, META_CREATEBRUSHINDIRECT, META_SELECTOBJECT, META_DELETEOBJECT,
//! META_POLYLINE, META_POLYGON, META_ELLIPSE and the end record.

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_render::Kind;

/// Logical units on the picture's longer side.
const SIZE: f64 = 30000.0;
/// Placeable header: logical units per inch (the picture is 10 inches across).
const UNITS_PER_INCH: u16 = 3000;
/// Most points one polyline or polygon record holds (longer runs are split).
const MAX_RUN: usize = 8000;

pub const PLACEABLE_KEY: u32 = 0x9AC6_CDD7;
pub const META_SETWINDOWORG: u16 = 0x020B;
pub const META_SETWINDOWEXT: u16 = 0x020C;
pub const META_SETBKMODE: u16 = 0x0102;
pub const META_SETPOLYFILLMODE: u16 = 0x0106;
pub const META_CREATEPENINDIRECT: u16 = 0x02FA;
pub const META_CREATEBRUSHINDIRECT: u16 = 0x02FC;
pub const META_SELECTOBJECT: u16 = 0x012D;
pub const META_DELETEOBJECT: u16 = 0x01F0;
pub const META_POLYLINE: u16 = 0x0325;
pub const META_POLYGON: u16 = 0x0324;
pub const META_ELLIPSE: u16 = 0x0418;
pub const META_EOF: u16 = 0x0000;

/// A space as WMF, showing `window` (or the extents with a 2% margin).
pub fn export(d: &Drawing, space: &Space, window: Option<Bounds2>) -> Vec<u8> {
    let list = cadaddy_render::build(d, space, &cadaddy_render::Options::default());
    export_list(&list, window)
}

/// Records being written: their words, the largest record, objects alive.
struct Records {
    words: Vec<u16>,
    max_record: u32,
}

impl Records {
    fn push(&mut self, func: u16, params: &[u16]) {
        let size = 3 + params.len() as u32;
        self.max_record = self.max_record.max(size);
        self.words.push(size as u16);
        self.words.push((size >> 16) as u16);
        self.words.push(func);
        self.words.extend_from_slice(params);
    }
}

fn colorref(c: Rgb) -> [u16; 2] {
    [u16::from(c.0) | (u16::from(c.1) << 8), u16::from(c.2)]
}

/// Write a display list as a placeable WMF.
pub fn export_list(list: &cadaddy_render::DisplayList, window: Option<Bounds2>) -> Vec<u8> {
    let window = window.filter(|w| !w.is_empty());
    let b = window.unwrap_or(list.bounds);
    let (w, h) = if b.is_empty() { (1.0, 1.0) } else { (b.width().max(1e-9), b.height().max(1e-9)) };
    let m = if window.is_some() { 0.0 } else { w.max(h) * 0.02 };
    let (x0, y1) = if b.is_empty() { (0.0, 1.0) } else { (b.min.x - m, b.max.y + m) };
    let (vw, vh) = (w + 2.0 * m, h + 2.0 * m);
    let k = SIZE / vw.max(vh);
    let (lw_units, lh_units) = ((vw * k).round().clamp(1.0, SIZE) as i16, (vh * k).round().clamp(1.0, SIZE) as i16);
    // Drawing units → logical units (y down).
    let px = |p: Vec2| -> [u16; 2] {
        let x = ((p.x - x0) * k).round().clamp(-32000.0, 32000.0) as i16;
        let y = ((y1 - p.y) * k).round().clamp(-32000.0, 32000.0) as i16;
        [x as u16, y as u16]
    };
    let mut r = Records { words: Vec::new(), max_record: 0 };
    r.push(META_SETWINDOWORG, &[0, 0]);
    r.push(META_SETWINDOWEXT, &[lh_units as u16, lw_units as u16]);
    r.push(META_SETBKMODE, &[1]);
    r.push(META_SETPOLYFILLMODE, &[2]);
    // Object slots: pen 0 and brush 1, made afresh when the colour or width changes.
    let white = Rgb(255, 255, 255);
    let mut current: Option<(Rgb, u16)> = None;
    let mut have = false;
    let set = |r: &mut Records, c: Rgb, width: u16, have: &mut bool| {
        if *have {
            r.push(META_DELETEOBJECT, &[0]);
            r.push(META_DELETEOBJECT, &[1]);
        }
        let cr = colorref(c);
        // Pen: style (solid), width (x, y), colour.
        r.push(META_CREATEPENINDIRECT, &[0, width, 0, cr[0], cr[1]]);
        // Brush: style (solid), colour, hatch.
        r.push(META_CREATEBRUSHINDIRECT, &[0, cr[0], cr[1], 0]);
        r.push(META_SELECTOBJECT, &[0]);
        r.push(META_SELECTOBJECT, &[1]);
        *have = true;
    };
    for p in &list.prims {
        let c = if p.kind == Kind::Mask { white } else { p.display_rgb(white) };
        // Lineweight in logical units: millimetres on a 10-inch-wide picture.
        let width = if p.lw > 0.0 { (f64::from(p.lw) / 25.4 * f64::from(UNITS_PER_INCH)).round().clamp(1.0, 2000.0) as u16 } else { 1 };
        if !matches!(p.kind, Kind::Image(_)) && current != Some((c, width)) {
            set(&mut r, c, width, &mut have);
            current = Some((c, width));
        }
        let pts = list.points(p);
        match p.kind {
            Kind::Polyline => {
                // Runs overlap by one point so the line stays joined.
                let mut start = 0;
                while start + 1 < pts.len() {
                    let end = (start + MAX_RUN).min(pts.len());
                    let run = pts.get(start..end).unwrap_or(&[]);
                    let mut params = vec![run.len() as u16];
                    params.extend(run.iter().flat_map(|q| px(*q)));
                    r.push(META_POLYLINE, &params);
                    start = end - 1;
                }
            }
            Kind::Tris | Kind::Mask => {
                for t in pts.chunks(3) {
                    if let [a, b2, cc] = t {
                        let mut params = vec![3];
                        params.extend([*a, *b2, *cc].iter().flat_map(|q| px(*q)));
                        r.push(META_POLYGON, &params);
                    }
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first() {
                    let [x, y] = px(*q);
                    let (x, y) = (x as i16, y as i16);
                    let rad = (width as i16).max(10);
                    // Bottom, right, top, left.
                    r.push(
                        META_ELLIPSE,
                        &[y.saturating_add(rad) as u16, x.saturating_add(rad) as u16, y.saturating_sub(rad) as u16, x.saturating_sub(rad) as u16],
                    );
                }
            }
            Kind::Infinite { ray } => {
                if let (Some(bp), Some(dir)) = (pts.first(), pts.get(1)) {
                    let far = (vw + vh) * 2.0 + bp.dist(b.center());
                    let a = if ray { *bp } else { *bp - *dir * far };
                    let e = *bp + *dir * far;
                    let mut params = vec![2];
                    params.extend([a, e].iter().flat_map(|q| px(*q)));
                    r.push(META_POLYLINE, &params);
                }
            }
            Kind::Image(_) => {}
        }
    }
    if have {
        r.push(META_DELETEOBJECT, &[0]);
        r.push(META_DELETEOBJECT, &[1]);
    }
    r.push(META_EOF, &[]);
    // The placeable header (22 bytes) and the metafile header (9 words).
    let mut out = Vec::with_capacity(22 + 18 + r.words.len() * 2);
    let mut head: Vec<u16> = Vec::with_capacity(11);
    head.extend([(PLACEABLE_KEY & 0xFFFF) as u16, (PLACEABLE_KEY >> 16) as u16, 0]);
    head.extend([0, 0, lw_units as u16, lh_units as u16]);
    head.extend([UNITS_PER_INCH, 0, 0]);
    let checksum = head.iter().fold(0u16, |a, w| a ^ w);
    head.push(checksum);
    for w in head {
        out.extend_from_slice(&w.to_le_bytes());
    }
    let total = 9 + r.words.len() as u32;
    // Type (on disk), header words, version 3.0, size in words, objects, largest record, unused.
    let meta: [u16; 9] = [1, 9, 0x0300, total as u16, (total >> 16) as u16, 2, r.max_record as u16, (r.max_record >> 16) as u16, 0];
    for w in meta.iter().chain(r.words.iter()) {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}
