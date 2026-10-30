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
        // Everything reaching here was clipped to `view` (within the picture plus a margin), so
        // the values fit 16 bits without bending any line.
        let x = ((p.x - x0) * k).round() as i16;
        let y = ((y1 - p.y) * k).round() as i16;
        [x as u16, y as u16]
    };
    // The picture plus a small margin, in drawing coordinates: far points are clipped to this
    // along the line, never clamped per axis.
    let e = vw.max(vh) * 0.02;
    let view = Bounds2::new(Vec2::new(x0 - e, y1 - vh - e), Vec2::new(x0 + vw + e, y1 + e));
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
                // Each segment is clipped to the view; pieces that stay joined form one run.
                let mut run: Vec<Vec2> = Vec::new();
                for seg in pts.windows(2) {
                    let (Some(a), Some(b2)) = (seg.first(), seg.get(1)) else { continue };
                    match clip_segment(*a, *b2, &view) {
                        Some((ca, cb)) => {
                            if run.last().is_some_and(|l| l.dist(ca) > 1e-9) {
                                polyline(&mut r, &run, &px);
                                run.clear();
                            }
                            if run.is_empty() {
                                run.push(ca);
                            }
                            run.push(cb);
                        }
                        None => {
                            polyline(&mut r, &run, &px);
                            run.clear();
                        }
                    }
                }
                polyline(&mut r, &run, &px);
            }
            Kind::Tris | Kind::Mask => {
                for t in pts.chunks(3) {
                    if let [a, b2, cc] = t {
                        let poly = clip_polygon(&[*a, *b2, *cc], &view);
                        if poly.len() >= 3 {
                            let mut params = vec![poly.len() as u16];
                            params.extend(poly.iter().flat_map(|q| px(*q)));
                            r.push(META_POLYGON, &params);
                        }
                    }
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first().filter(|q| view.contains(**q)) {
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
                    let en = *bp + *dir * far;
                    if let Some((ca, ce)) = clip_segment(a, en, &view) {
                        polyline(&mut r, &[ca, ce], &px);
                    }
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

/// Write a run of points as POLYLINE records (longer runs are split, overlapping by one point so
/// the line stays joined).
fn polyline(r: &mut Records, run: &[Vec2], px: &dyn Fn(Vec2) -> [u16; 2]) {
    let mut start = 0;
    while start + 1 < run.len() {
        let end = (start + MAX_RUN).min(run.len());
        let part = run.get(start..end).unwrap_or(&[]);
        let mut params = vec![part.len() as u16];
        params.extend(part.iter().flat_map(|q| px(*q)));
        r.push(META_POLYLINE, &params);
        start = end - 1;
    }
}

/// The part of segment `a`–`b` inside `r` (Liang–Barsky), or `None`; the line keeps its slope.
fn clip_segment(a: Vec2, b: Vec2, r: &Bounds2) -> Option<(Vec2, Vec2)> {
    if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
        return None;
    }
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a.x - r.min.x), (dx, r.max.x - a.x), (-dy, a.y - r.min.y), (dy, r.max.y - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
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
    (t0 <= t1).then(|| (Vec2::new(a.x + t0 * dx, a.y + t0 * dy), Vec2::new(a.x + t1 * dx, a.y + t1 * dy)))
}

/// A polygon clipped to the rectangle `r` (Sutherland–Hodgman).
fn clip_polygon(poly: &[Vec2], r: &Bounds2) -> Vec<Vec2> {
    if poly.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return Vec::new();
    }
    let mut pts = poly.to_vec();
    // Edges: (axis is x, keep greater-or-equal, bound).
    for (is_x, ge, bound) in [(true, true, r.min.x), (true, false, r.max.x), (false, true, r.min.y), (false, false, r.max.y)] {
        let val = |p: Vec2| if is_x { p.x } else { p.y };
        let inside = |p: Vec2| if ge { val(p) >= bound } else { val(p) <= bound };
        let mut out = Vec::with_capacity(pts.len() + 2);
        for (i, cur) in pts.iter().enumerate() {
            let Some(prev) = pts.get((i + pts.len() - 1) % pts.len()) else { continue };
            if inside(*cur) != inside(*prev) {
                let d = val(*cur) - val(*prev);
                let t = if d == 0.0 { 0.0 } else { (bound - val(*prev)) / d };
                out.push(*prev + (*cur - *prev) * t);
            }
            if inside(*cur) {
                out.push(*cur);
            }
        }
        pts = out;
        if pts.is_empty() {
            break;
        }
    }
    pts
}
