//! EMF (Enhanced Metafile, vector) for the Windows clipboard's `CF_ENHMETAFILE`: the display list
//! the WMF, SVG and PDF exports draw, in 32-bit logical units of 0.01 mm (the picture fitted into
//! 0…20000 on its longer side, 200 mm). Pens carry colour and lineweight, brushes fill areas and
//! wipeouts (white); text draws as its strokes; images are left out.
//!
//! Records, from the public description of the format: EMR_HEADER, EMR_SETBKMODE,
//! EMR_SETPOLYFILLMODE, EMR_CREATEPEN, EMR_CREATEBRUSHINDIRECT, EMR_SELECTOBJECT,
//! EMR_DELETEOBJECT, EMR_POLYLINE, EMR_POLYGON, EMR_ELLIPSE and EMR_EOF.

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::{Bounds2, Vec2};

use crate::Kind;
use crate::clip::{clip_polygon, clip_segment};

/// Logical units (0.01 mm) on the picture's longer side.
const SIZE: f64 = 20000.0;
/// Most points one polyline or polygon record holds (longer runs are split).
const MAX_RUN: usize = 8000;
/// The thinnest pen, in logical units (0.1 mm), so hairlines show in documents.
const MIN_PEN: f64 = 10.0;

pub const EMR_HEADER: u32 = 1;
pub const EMR_POLYGON: u32 = 3;
pub const EMR_POLYLINE: u32 = 4;
pub const EMR_EOF: u32 = 14;
pub const EMR_SETBKMODE: u32 = 18;
pub const EMR_SETPOLYFILLMODE: u32 = 19;
pub const EMR_SELECTOBJECT: u32 = 37;
pub const EMR_CREATEPEN: u32 = 38;
pub const EMR_CREATEBRUSHINDIRECT: u32 = 39;
pub const EMR_DELETEOBJECT: u32 = 40;
pub const EMR_ELLIPSE: u32 = 42;
/// The header's signature, " EMF" read as a little-endian number.
pub const SIGNATURE: u32 = 0x464D_4520;
/// Bytes in the header record (the original version, without extension fields).
pub const HEADER_BYTES: u32 = 88;
/// Stock objects that take the pen's and brush's place before they are deleted (NULL_PEN, NULL_BRUSH).
const STOCK_NULL_PEN: u32 = 0x8000_0008;
const STOCK_NULL_BRUSH: u32 = 0x8000_0005;

/// A space as EMF (the extents with a 1% margin).
pub fn export(d: &Drawing, space: &Space) -> Vec<u8> {
    let list = crate::build(d, space, &crate::Options::default());
    export_list(&list)
}

/// Records written so far: their bytes and how many.
struct Records {
    bytes: Vec<u8>,
    count: u32,
}

impl Records {
    /// One record: type, size, then the parameter words (the size covers the 8-byte prefix).
    fn push(&mut self, kind: u32, params: &[u32]) {
        self.bytes.extend_from_slice(&kind.to_le_bytes());
        self.bytes.extend_from_slice(&(8 + 4 * params.len() as u32).to_le_bytes());
        for p in params {
            self.bytes.extend_from_slice(&p.to_le_bytes());
        }
        self.count += 1;
    }

    /// A polyline or polygon record: bounds, point count, 32-bit points.
    fn push_points(&mut self, kind: u32, pts: &[[u32; 2]]) {
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        for p in pts {
            for (i, v) in p.iter().enumerate() {
                let v = *v as i32;
                lo[i] = lo[i].min(v);
                hi[i] = hi[i].max(v);
            }
        }
        let mut params = vec![lo[0] as u32, lo[1] as u32, hi[0] as u32, hi[1] as u32, pts.len() as u32];
        params.extend(pts.iter().flatten());
        self.push(kind, &params);
    }

    /// Swap the pen and brush for stock ones and delete ours.
    fn release(&mut self) {
        self.push(EMR_SELECTOBJECT, &[STOCK_NULL_PEN]);
        self.push(EMR_SELECTOBJECT, &[STOCK_NULL_BRUSH]);
        self.push(EMR_DELETEOBJECT, &[1]);
        self.push(EMR_DELETEOBJECT, &[2]);
    }
}

fn colorref(c: Rgb) -> u32 {
    u32::from(c.0) | (u32::from(c.1) << 8) | (u32::from(c.2) << 16)
}

/// Write a display list as an EMF; an empty list gives a valid empty metafile.
pub fn export_list(list: &crate::DisplayList) -> Vec<u8> {
    let b = list.bounds;
    let ok = !b.is_empty() && b.width().is_finite() && b.height().is_finite();
    let (w, h) = if ok { (b.width().max(1e-9), b.height().max(1e-9)) } else { (1.0, 1.0) };
    let m = w.max(h) * 0.01;
    let (x0, y1) = if ok { (b.min.x - m, b.max.y + m) } else { (0.0, 1.0) };
    let (vw, vh) = (w + 2.0 * m, h + 2.0 * m);
    let k = SIZE / vw.max(vh);
    let (lw, lh) = ((vw * k).round().clamp(1.0, SIZE) as i32, (vh * k).round().clamp(1.0, SIZE) as i32);
    // Drawing units → logical units (y down).
    let px = |p: Vec2| -> [u32; 2] { [((p.x - x0) * k).round() as i32 as u32, ((y1 - p.y) * k).round() as i32 as u32] };
    // Far points are clipped to the picture plus a margin along the line, never clamped per axis.
    let e = vw.max(vh) * 0.02;
    let view = Bounds2::new(Vec2::new(x0 - e, y1 - vh - e), Vec2::new(x0 + vw + e, y1 + e));
    let mut r = Records { bytes: Vec::new(), count: 1 };
    r.push(EMR_SETBKMODE, &[1]);
    r.push(EMR_SETPOLYFILLMODE, &[2]);
    // Object handles: pen 1 and brush 2 (0 is the metafile itself), made afresh when the colour or width changes.
    let white = Rgb(255, 255, 255);
    let mut current: Option<(Rgb, u32)> = None;
    let mut have = false;
    for p in &list.prims {
        let c = if p.kind == Kind::Mask { white } else { p.display_rgb(white) };
        let width = (f64::from(p.lw) * 100.0).round().clamp(MIN_PEN, 2000.0) as u32;
        if !matches!(p.kind, Kind::Image(_)) && current != Some((c, width)) {
            if have {
                r.release();
            }
            // Pen: handle, style (solid), width (x, y), colour. Brush: handle, style (solid), colour, hatch.
            r.push(EMR_CREATEPEN, &[1, 0, width, 0, colorref(c)]);
            r.push(EMR_CREATEBRUSHINDIRECT, &[2, 0, colorref(c), 0]);
            r.push(EMR_SELECTOBJECT, &[1]);
            r.push(EMR_SELECTOBJECT, &[2]);
            have = true;
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
            Kind::Tris | Kind::InkMask | Kind::Mask => {
                for t in pts.chunks(3) {
                    if let [a, b2, cc] = t {
                        let poly = clip_polygon(&[*a, *b2, *cc], &view);
                        if poly.len() >= 3 {
                            let q: Vec<[u32; 2]> = poly.iter().map(|v| px(*v)).collect();
                            r.push_points(EMR_POLYGON, &q);
                        }
                    }
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first().filter(|q| view.contains(**q)) {
                    let [x, y] = px(*q);
                    let (x, y) = (x as i32, y as i32);
                    let rad = (width as i32).max(10);
                    // The ellipse's box: left, top, right, bottom.
                    r.push(EMR_ELLIPSE, &[x - rad, y - rad, x + rad, y + rad].map(|v| v as u32));
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
        r.release();
    }
    // The end record: no palette entries, offset to the (absent) palette, its own size repeated.
    r.push(EMR_EOF, &[0, 16, 20]);
    let total = HEADER_BYTES + r.bytes.len() as u32;
    let mut out = Vec::with_capacity(total as usize);
    let word = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    word(&mut out, EMR_HEADER);
    word(&mut out, HEADER_BYTES);
    // Bounds in device units (inclusive), then the frame in 0.01 mm: the whole picture.
    for v in [0, 0, lw - 1, lh - 1, 0, 0, lw, lh] {
        word(&mut out, v as u32);
    }
    word(&mut out, SIGNATURE);
    word(&mut out, 0x0001_0000);
    word(&mut out, total);
    word(&mut out, r.count);
    // Handles (pen, brush and the metafile) and a reserved word.
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    // Description (none: size, offset), palette entries, the device in pixels (0.01 mm each) and in millimetres.
    for v in [0, 0, 0, 30000, 30000, 300, 300] {
        word(&mut out, v);
    }
    debug_assert_eq!(out.len() as u32, HEADER_BYTES);
    out.extend_from_slice(&r.bytes);
    out
}

/// Write a run of points as POLYLINE records (longer runs are split, overlapping by one point so
/// the line stays joined).
fn polyline(r: &mut Records, run: &[Vec2], px: &dyn Fn(Vec2) -> [u32; 2]) {
    let mut start = 0;
    while start + 1 < run.len() {
        let end = (start + MAX_RUN).min(run.len());
        let part: Vec<[u32; 2]> = run.get(start..end).unwrap_or(&[]).iter().map(|q| px(*q)).collect();
        r.push_points(EMR_POLYLINE, &part);
        start = end - 1;
    }
}
