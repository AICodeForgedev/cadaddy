//! MicroStation DGN V7 (the ISFF design file): read into a drawing, and a 2D drawing written as
//! one. DGN V8 (an OLE compound file whose layout isn't publicly documented) is refused with a
//! clear message.
//!
//! Written from the published element layouts of the V7 format as GDAL's MIT-licensed DGN
//! library documents them (element header and display header, 32-bit integers stored as two
//! little-endian 16-bit words high word first, VAX D-float doubles, the 1536-byte TCB with its
//! units and global origin, the colour table, Radix-50 cell names). No Bentley software, SDK or
//! file was used.
//!
//! What a file becomes:
//! - levels 1–63 → layers `Level 1` … `Level 63`;
//! - colours through the file's colour table (true colours; an ACI index when the entry is the
//!   ACI colour); line styles 1–7 → DOT, DASHED, DASHEDX2, DASHDOT, HIDDEN, DIVIDE, CENTER;
//!   weights → lineweights (0.1 mm a step);
//! - lines, line strings, shapes (closed), curves (as polylines through their points), point
//!   strings, ellipses and arcs (circles and arcs when round), texts and text nodes, B-spline
//!   curves (poles, order, knots, weights);
//! - complex chains and shapes → one polyline (round arcs as bulges);
//! - cells → blocks named by the cell (its elements placed back by the cell's transform; copies of
//!   one cell share one block, a different body under the same name gets `NAME_2`…) and a
//!   reference where it was placed;
//! - the rest (3D surfaces and solids, dimensions, shared cells, tags, raster…) is counted in the
//!   drawing's read notes.
//!
//! Everything read is capped: file size, element count, nesting, points.

use std::collections::BTreeMap;

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::{
    Arc, Block, Circle, Common, Drawing, Ellipse, Entity, EntityKind, HAlign, Line, Lineweight, LwPolyline, Point, Space, Text, VAlign,
};
use cadaddy_geom::{Bounds2, Mat3, PolyVertex, Spline, Vec2, Vec3};

use crate::{IoError, Result};

/// Largest file read.
const MAX_FILE: usize = 512 << 20;
/// Most elements read.
const MAX_ELEMENTS: usize = 4_000_000;
/// Deepest nesting of cells and text nodes (the elements that hold others).
const MAX_DEPTH: usize = 8;
/// Most earlier blocks of one cell name a cell's body is compared with before it gets a block
/// of its own (keeps a file of many different same-named cells linear).
const MAX_BODIES: usize = 64;
/// Most vertices a written line string or shape holds (MicroStation's limit).
const MAX_VERTICES: usize = 101;

/// The line styles 1–7 by linetype name (0 is solid).
const STYLES: [&str; 8] = ["CONTINUOUS", "DOT", "DASHED", "DASHEDX2", "DASHDOT", "HIDDEN", "DIVIDE", "CENTER"];

// ---------------- encoding ----------------

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from(*b.get(i)?) | u16::from(*b.get(i + 1)?) << 8)
}

/// A 32-bit integer stored as two little-endian words, the high word first.
fn i32_at(b: &[u8], i: usize) -> Option<i32> {
    let hi = u32::from(u16_at(b, i)?);
    let lo = u32::from(u16_at(b, i + 2)?);
    Some((hi << 16 | lo) as i32)
}

fn put_i32(out: &mut [u8], i: usize, v: i32) {
    let u = v as u32;
    for (k, byte) in [(u >> 16) as u8, (u >> 24) as u8, u as u8, (u >> 8) as u8].into_iter().enumerate() {
        if let Some(slot) = out.get_mut(i + k) {
            *slot = byte;
        }
    }
}

fn put_u16(out: &mut [u8], i: usize, v: u16) {
    if let Some(s) = out.get_mut(i) {
        *s = v as u8;
    }
    if let Some(s) = out.get_mut(i + 1) {
        *s = (v >> 8) as u8;
    }
}

/// A VAX D-float: sign, an 8-bit exponent (bias 128, the fraction read as 0.1fff…) and a 55-bit
/// fraction, in four little-endian words, the most significant first.
fn vax_at(b: &[u8], i: usize) -> Option<f64> {
    let hi = u32::from(u16_at(b, i)?) << 16 | u32::from(u16_at(b, i + 2)?);
    let lo = u32::from(u16_at(b, i + 4)?) << 16 | u32::from(u16_at(b, i + 6)?);
    let exp = (hi >> 23) & 0xff;
    if exp == 0 {
        return Some(0.0);
    }
    let frac = u64::from(hi & 0x7f_ffff) << 32 | u64::from(lo);
    let m = 1.0 + frac as f64 / (1u64 << 55) as f64;
    let v = m * 2f64.powi(exp as i32 - 129);
    Some(if hi >> 31 == 1 { -v } else { v })
}

fn put_vax(out: &mut [u8], i: usize, v: f64) {
    let v = if v.is_finite() { v } else { 0.0 };
    let bits = v.to_bits();
    let sign = (bits >> 63) as u32;
    let e = ((bits >> 52) & 0x7ff) as i32;
    let (hi, lo) = if v == 0.0 || e == 0 {
        (0u32, 0u32)
    } else {
        let exp = e - 1023 + 129;
        if exp <= 0 {
            (0, 0)
        } else {
            let exp = exp.min(255) as u32;
            let frac55 = (bits & 0x000f_ffff_ffff_ffff) << 3;
            (sign << 31 | exp << 23 | ((frac55 >> 32) as u32 & 0x7f_ffff), frac55 as u32)
        }
    };
    put_u16(out, i, (hi >> 16) as u16);
    put_u16(out, i + 2, hi as u16);
    put_u16(out, i + 4, (lo >> 16) as u16);
    put_u16(out, i + 6, lo as u16);
}

/// Radix-50: three characters (space, A–Z, $, ., 0–9) in a 16-bit word.
fn rad50_text(w: u16) -> String {
    let mut out = String::new();
    let mut v = w;
    for q in [1600u16, 40, 1] {
        let c = v / q;
        v -= c * q;
        out.push(match c {
            1..=26 => (b'A' + (c - 1) as u8) as char,
            27 => '$',
            28 => '.',
            30..=39 => (b'0' + (c - 30) as u8) as char,
            _ => ' ',
        });
    }
    out
}

fn rad50_word(s: &str) -> u16 {
    let mut w: u16 = 0;
    for i in 0..3 {
        let c = s.as_bytes().get(i).copied().unwrap_or(b' ').to_ascii_uppercase();
        let v = match c {
            b'A'..=b'Z' => c - b'A' + 1,
            b'$' => 27,
            b'.' => 28,
            b'0'..=b'9' => c - b'0' + 30,
            _ => 0,
        };
        w = w * 40 + u16::from(v);
    }
    w
}

/// Whether bytes are a DGN V7 design file (a TCB first, 2D or 3D) or a cell library.
pub fn is_v7(b: &[u8]) -> bool {
    matches!(b, [0x08 | 0xC8, 0x09, 0xFE, 0x02, ..] | [0x08, 0x05, 0x17, 0x00, ..])
}

/// Whether bytes are an OLE compound file, which a DGN V8 file is.
pub fn is_v8(b: &[u8]) -> bool {
    b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

// ---------------- reading ----------------

/// One element as stored: its place in the file and its bytes (header included).
struct Raw<'a> {
    at: usize,
    data: &'a [u8],
}

impl Raw<'_> {
    fn level(&self) -> u8 {
        self.data.first().map_or(0, |b| b & 0x3f)
    }
    fn complex(&self) -> bool {
        self.data.first().is_some_and(|b| b & 0x80 != 0)
    }
    fn deleted(&self) -> bool {
        self.data.get(1).is_some_and(|b| b & 0x80 != 0)
    }
    fn ty(&self) -> u8 {
        self.data.get(1).map_or(0, |b| b & 0x7f)
    }
    /// The words after the `totlength` word of a cell or complex header: where its group ends.
    fn group_end(&self) -> usize {
        self.at + 38 + usize::from(u16_at(self.data, 36).unwrap_or(0)) * 2
    }
}

fn elements(bytes: &[u8]) -> Vec<Raw<'_>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= bytes.len() && out.len() < MAX_ELEMENTS {
        if bytes.get(at..at + 2) == Some(&[0xff, 0xff]) {
            break;
        }
        let words = usize::from(u16_at(bytes, at + 2).unwrap_or(0));
        let end = at + 4 + words * 2;
        let Some(data) = bytes.get(at..end) else { break };
        out.push(Raw { at, data });
        at = end;
    }
    out
}

/// What the TCB says and what reading collects.
struct Ctx {
    /// Master units per UOR.
    scale: f64,
    /// The global origin in master units.
    origin: Vec3,
    dim3: bool,
    palette: Option<Vec<Rgb>>,
    skipped: BTreeMap<String, usize>,
    levels: std::collections::BTreeSet<u8>,
    styles: std::collections::BTreeSet<u8>,
    /// The blocks made for cells, by cell name: each block's name and its body as compared.
    cells: BTreeMap<String, Vec<(String, Vec<serde_json::Value>)>>,
    /// The next `NAME_n` suffix to try, by cell name.
    suffix: BTreeMap<String, usize>,
}

impl Ctx {
    fn pt(&self, d: &[u8], i: usize) -> Option<Vec3> {
        let x = f64::from(i32_at(d, i)?);
        let y = f64::from(i32_at(d, i + 4)?);
        let z = if self.dim3 { f64::from(i32_at(d, i + 8)?) } else { 0.0 };
        Some(Vec3::new(x * self.scale - self.origin.x, y * self.scale - self.origin.y, z * self.scale - self.origin.z))
    }
    fn pt_vax(&self, d: &[u8], i: usize) -> Option<Vec3> {
        let x = vax_at(d, i)?;
        let y = vax_at(d, i + 8)?;
        let z = if self.dim3 { vax_at(d, i + 16)? } else { 0.0 };
        Some(Vec3::new(x * self.scale - self.origin.x, y * self.scale - self.origin.y, z * self.scale - self.origin.z))
    }
    fn psize(&self) -> usize {
        if self.dim3 { 12 } else { 8 }
    }
    fn skip(&mut self, what: &str) {
        *self.skipped.entry(what.to_string()).or_default() += 1;
    }

    /// An element's layer, colour, linetype and lineweight.
    fn common(&mut self, r: &Raw) -> Common {
        let d = r.data;
        let level = r.level().max(1);
        self.levels.insert(level);
        let mut c = Common { layer: format!("Level {level}").into(), ..Default::default() };
        if d.len() >= 36 {
            let sym = d.get(34).copied().unwrap_or(0);
            let (style, weight) = (sym & 7, sym >> 3);
            let index = d.get(35).copied().unwrap_or(0);
            c.color = match &self.palette {
                Some(p) => match p.get(usize::from(index)) {
                    Some(rgb) if index > 0 && cadaddy_color::aci_rgb(index) == *rgb => Color::Index(index),
                    Some(rgb) => Color::True(*rgb),
                    None => Color::ByLayer,
                },
                None if index == 0 => Color::Index(7),
                None => Color::Index(index),
            };
            if style > 0 {
                self.styles.insert(style);
                c.linetype = STYLES.get(usize::from(style)).copied().unwrap_or("ByLayer").into();
            }
            if weight > 0 {
                let v = u16::from(weight) * 10;
                let nearest = Lineweight::STANDARD.iter().copied().min_by_key(|s| s.abs_diff(v)).unwrap_or(v);
                c.lineweight = Lineweight::Mm100(nearest);
            }
        }
        c
    }
}

/// An arc or ellipse element's centre, axes, rotation (radians) and start/sweep (degrees).
fn arc_parts(ctx: &Ctx, d: &[u8], arc: bool) -> Option<(Vec3, f64, f64, f64, f64, f64)> {
    let off = if arc { 8 } else { 0 };
    let a = vax_at(d, 36 + off)? * ctx.scale;
    let b = vax_at(d, 44 + off)? * ctx.scale;
    let (rot, center) =
        if ctx.dim3 { (0.0, ctx.pt_vax(d, 68 + off)?) } else { (f64::from(i32_at(d, 52 + off)?) / 360_000.0, ctx.pt_vax(d, 56 + off)?) };
    let (start, sweep) = if arc {
        let start = f64::from(i32_at(d, 36)?) / 360_000.0;
        let mut raw = d.get(36 + 4..36 + 8)?.to_vec();
        let negative = raw.get(1).is_some_and(|b| b & 0x80 != 0);
        if let Some(b) = raw.get_mut(1) {
            *b &= 0x7f;
        }
        let v = f64::from(i32_at(&raw, 0)?) / 360_000.0;
        let sweep = if v == 0.0 {
            360.0
        } else if negative {
            -v
        } else {
            v
        };
        (start, sweep)
    } else {
        (0.0, 360.0)
    };
    (a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0).then_some((center, a, b, rot.to_radians(), start, sweep))
}

/// An arc/ellipse element as a circle, an arc or an ellipse.
fn arc_kind(ctx: &Ctx, d: &[u8], arc: bool) -> Option<EntityKind> {
    let (c, a, b, rot, start, sweep) = arc_parts(ctx, d, arc)?;
    let full = sweep.abs() >= 359.9999;
    let (s0, s1) = if sweep >= 0.0 { (start, start + sweep) } else { (start + sweep, start) };
    if (a - b).abs() <= 1e-9 * a.max(b) {
        if full {
            return Some(EntityKind::Circle(Circle { center: c, radius: a }));
        }
        return Some(EntityKind::Arc(Arc { center: c, radius: a, start: rot + s0.to_radians(), end: rot + s1.to_radians() }));
    }
    let (major, minor, rot) = if a >= b { (a, b, rot) } else { (b, a, rot + std::f64::consts::FRAC_PI_2) };
    let shift = if a >= b { 0.0 } else { -std::f64::consts::FRAC_PI_2 };
    let (t0, t1) = if full { (0.0, std::f64::consts::TAU) } else { (s0.to_radians() + shift, s1.to_radians() + shift) };
    Some(EntityKind::Ellipse(Ellipse {
        center: c,
        major: Vec3::new(major * rot.cos(), major * rot.sin(), 0.0),
        ratio: minor / major,
        start: t0,
        end: t1,
    }))
}

/// The points of a line string / shape / curve / pole / point string element.
fn points(ctx: &Ctx, d: &[u8]) -> Vec<Vec3> {
    let n = usize::from(u16_at(d, 36).unwrap_or(0));
    (0..n).map_while(|i| ctx.pt(d, 38 + i * ctx.psize())).collect()
}

fn text_kind(ctx: &Ctx, d: &[u8]) -> Option<EntityKind> {
    let just = d.get(37).copied().unwrap_or(2);
    let k = ctx.scale * 6.0 / 1000.0;
    let length = f64::from(i32_at(d, 38)?) * k;
    let height = f64::from(i32_at(d, 42)?) * k;
    let (rot, origin, n_at) = if ctx.dim3 { (0.0, ctx.pt(d, 62)?, 74) } else { (f64::from(i32_at(d, 46)?) / 360_000.0, ctx.pt(d, 50)?, 58) };
    let n = usize::from(d.get(n_at).copied().unwrap_or(0));
    let raw = d.get(n_at + 2..(n_at + 2 + n).min(d.len())).unwrap_or(&[]);
    let value: String = if raw.starts_with(&[0xFF, 0xFD]) {
        let units: Vec<u16> = raw.get(2..).unwrap_or(&[]).as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units).trim_end_matches('\0').to_string()
    } else {
        raw.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect()
    };
    if value.trim().is_empty() || !(height.is_finite() && height > 0.0) {
        return None;
    }
    let (halign, valign) = match (just / 6, just % 6) {
        (0, 2) => (HAlign::Left, VAlign::Baseline),
        (col, row) => (
            match col {
                0 => HAlign::Left,
                1 => HAlign::Center,
                _ => HAlign::Right,
            },
            match row {
                0 => VAlign::Top,
                1 => VAlign::Middle,
                _ => VAlign::Bottom,
            },
        ),
    };
    let aligned = !(halign == HAlign::Left && valign == VAlign::Baseline);
    Some(EntityKind::Text(Text {
        insert: origin,
        align_pt: aligned.then_some(origin),
        height,
        value,
        rotation: rot.to_radians(),
        width_factor: if height > 0.0 && length > 0.0 { (length / height).clamp(0.01, 100.0) } else { 1.0 },
        oblique: 0.0,
        style: "Standard".into(),
        halign,
        valign,
    }))
}

/// A complex chain's or shape's members as one polyline (round arcs as bulges).
fn chain(ctx: &Ctx, members: &[&Raw], closed: bool) -> Option<EntityKind> {
    let mut v: Vec<PolyVertex> = Vec::new();
    let push = |v: &mut Vec<PolyVertex>, p: Vec2| {
        if v.last().is_none_or(|q| (q.p - p).len() > 1e-9) {
            v.push(PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 });
        }
    };
    for m in members {
        match m.ty() {
            3 => {
                if let (Some(a), Some(b)) = (ctx.pt(m.data, 36), ctx.pt(m.data, 36 + ctx.psize())) {
                    push(&mut v, a.xy());
                    push(&mut v, b.xy());
                }
            }
            4 | 6 | 11 => {
                for p in points(ctx, m.data) {
                    push(&mut v, p.xy());
                }
            }
            15 | 16 => {
                let Some((c, a, b, rot, start, sweep)) = arc_parts(ctx, m.data, m.ty() == 16) else { continue };
                let at = |deg: f64| {
                    let t = deg.to_radians();
                    let (x, y) = (a * t.cos(), b * t.sin());
                    c.xy() + Vec2::new(x * rot.cos() - y * rot.sin(), x * rot.sin() + y * rot.cos())
                };
                if (a - b).abs() <= 1e-9 * a {
                    push(&mut v, at(start));
                    if let Some(last) = v.last_mut() {
                        last.bulge = (sweep.to_radians() / 4.0).tan();
                    }
                    let e = at(start + sweep);
                    v.push(PolyVertex { p: e, bulge: 0.0, start_width: 0.0, end_width: 0.0 });
                } else {
                    for k in 0..=32 {
                        push(&mut v, at(start + sweep * f64::from(k) / 32.0));
                    }
                }
            }
            _ => {}
        }
    }
    if closed && v.len() > 2 && v.first().zip(v.last()).is_some_and(|(a, b)| (a.p - b.p).len() <= 1e-9) {
        v.pop();
    }
    (v.len() >= 2).then_some(EntityKind::LwPolyline(LwPolyline { vertices: v, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
}

/// A B-spline curve from its header and its pole, knot and weight elements.
fn bspline(ctx: &Ctx, header: &Raw, members: &[&Raw]) -> Option<EntityKind> {
    let d = header.data;
    let order = usize::from(d.get(40).copied().unwrap_or(0) & 0x0f) + 2;
    let closed = d.get(40).is_some_and(|b| b & 0x80 != 0);
    let poles: Vec<Vec2> = members.iter().filter(|m| m.ty() == 21).flat_map(|m| points(ctx, m.data)).map(|p| p.xy()).collect();
    if poles.len() < order {
        return None;
    }
    let array = |m: &Raw| -> Vec<f64> {
        let attr = usize::from(u16_at(m.data, 30).unwrap_or(0)) * 2 + 32;
        let end = if attr > 36 && attr <= m.data.len() { attr } else { m.data.len() };
        (0..(end.saturating_sub(36)) / 4).filter_map(|i| i32_at(m.data, 36 + i * 4)).map(|v| f64::from(v) / f64::from(i32::MAX)).collect()
    };
    let interior: Vec<f64> = members.iter().find(|m| m.ty() == 26).map(|m| array(m)).unwrap_or_default();
    let n = poles.len();
    let inner = n - order;
    // Clamped ends; the interior knots given, else evenly spaced.
    let mut knots = vec![0.0; order];
    if interior.len() == inner {
        knots.extend(interior);
    } else {
        knots.extend((1..=inner).map(|i| i as f64 / (inner + 1) as f64));
    }
    knots.extend(std::iter::repeat_n(1.0, order));
    let weights: Vec<f64> = members.iter().find(|m| m.ty() == 28).map(|m| array(m)).filter(|w| w.len() == n).unwrap_or_default();
    Some(EntityKind::Spline(Box::new(Spline { degree: order - 1, knots, control: poles, weights, closed, ..Spline::default() })))
}

/// A cell's placement: its origin and the 2×2 matrix taking its elements' local coordinates to
/// where they are.
fn cell_transform(ctx: &Ctx, d: &[u8]) -> Option<(Vec3, [f64; 4])> {
    if ctx.dim3 {
        let t = |i: usize| i32_at(d, i).map(|v| f64::from(v) / 214_748.0);
        // The 3×3 matrix's upper-left 2×2 (plan only).
        return Some((ctx.pt(d, 112)?, [t(76)?, t(80)?, t(88)?, t(92)?]));
    }
    let t = |i: usize| i32_at(d, i).map(|v| f64::from(v) / 214_748.0);
    Some((ctx.pt(d, 84)?, [t(68)?, t(72)?, t(76)?, t(80)?]))
}

/// Whether two bodies are the same: numbers within `tol`, whole numbers (colours, flags) and
/// everything else exactly.
fn same_value(a: &serde_json::Value, b: &serde_json::Value, tol: f64) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if x.is_f64() || y.is_f64() => x.as_f64().zip(y.as_f64()).is_some_and(|(x, y)| (x - y).abs() <= tol),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_value(x, y, tol)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_value(v, w, tol))),
        _ => a == b,
    }
}

/// The block for a cell named `base` with `members` (in the cell's own coordinates): an earlier
/// block of that name with the same body (within `tol`), else a new block `base`, `base_2`, …
/// under a name no block has yet.
fn cell_block(ctx: &mut Ctx, d: &mut Drawing, base: &str, members: Vec<(Common, EntityKind)>, tol: f64) -> String {
    let body: Vec<serde_json::Value> = members.iter().filter_map(|m| serde_json::to_value(m).ok()).collect();
    let earlier = ctx.cells.get(base).map(Vec::as_slice).unwrap_or(&[]);
    let start = earlier.len().saturating_sub(MAX_BODIES);
    if let Some((name, _)) = earlier
        .get(start..)
        .unwrap_or(&[])
        .iter()
        .find(|(_, b)| b.len() == body.len() && body.len() == members.len() && b.iter().zip(&body).all(|(x, y)| same_value(x, y, tol)))
    {
        return name.clone();
    }
    let mut n = ctx.suffix.get(base).copied().unwrap_or(1);
    let mut name = if n == 1 { base.to_string() } else { format!("{base}_{n}") };
    while d.blocks.contains_key(&name) {
        n += 1;
        name = format!("{base}_{n}");
    }
    ctx.suffix.insert(base.to_string(), n + 1);
    let mut blk = Block::new(&name);
    for (c, k) in members {
        let h = d.new_handle();
        blk.entities.push(Entity { handle: h, common: c, kind: k });
    }
    d.blocks.insert(name.clone(), std::sync::Arc::new(blk));
    ctx.cells.entry(base.to_string()).or_default().push((name.clone(), body));
    name
}

/// One element (and the members of a group it heads) as objects, appended to `out`; returns the
/// index of the next element to read.
fn take(ctx: &mut Ctx, d: &mut Drawing, raws: &[Raw], i: usize, depth: usize, out: &mut Vec<(Common, EntityKind)>) -> usize {
    let Some(r) = raws.get(i) else { return i + 1 };
    if r.deleted() {
        return i + 1;
    }
    let members_end = |raws: &[Raw], i: usize, end: usize| -> usize {
        let mut j = i + 1;
        while raws.get(j).is_some_and(|m| m.at < end && m.complex()) {
            j += 1;
        }
        j
    };
    let ty = r.ty();
    // Not drawn (the TCB, the colour table and other group data, digitizer setup, level
    // symbology, a cell library header): no layer of their own.
    if matches!(ty, 1 | 5 | 8 | 9 | 10) {
        return i + 1;
    }
    // Cells and text nodes are the elements that hold others (and so recurse): their nesting is
    // capped here, once, for both.
    if depth >= MAX_DEPTH && matches!(ty, 2 | 7) {
        ctx.skip(if ty == 2 { "cells nested too deep" } else { "text nodes nested too deep" });
        return members_end(raws, i, r.group_end()).max(i + 1);
    }
    let common = ctx.common(r);
    match ty {
        3 => {
            if let (Some(a), Some(b)) = (ctx.pt(r.data, 36), ctx.pt(r.data, 36 + ctx.psize())) {
                if (a - b).len() <= 1e-12 {
                    out.push((common, EntityKind::Point(Point { p: a, angle: 0.0 })));
                } else {
                    out.push((common, EntityKind::Line(Line { a, b })));
                }
            }
        }
        4 | 6 | 11 => {
            let mut pts = points(ctx, r.data);
            // A curve's first and last two points set its end tangents.
            if ty == 11 && pts.len() > 4 {
                pts = pts.get(2..pts.len() - 2).map(<[Vec3]>::to_vec).unwrap_or_default();
            }
            let closed = ty == 6;
            if closed && pts.len() > 2 && pts.first().zip(pts.last()).is_some_and(|(a, b)| (*a - *b).len() <= 1e-9) {
                pts.pop();
            }
            if pts.len() >= 2 {
                let vertices = pts.iter().map(|p| PolyVertex { p: p.xy(), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
                out.push((
                    common,
                    EntityKind::LwPolyline(LwPolyline {
                        vertices,
                        closed,
                        const_width: 0.0,
                        elevation: pts.first().map_or(0.0, |p| p.z),
                        plinegen: false,
                    }),
                ));
            }
        }
        22 => {
            for p in points(ctx, r.data) {
                out.push((common.clone(), EntityKind::Point(Point { p, angle: 0.0 })));
            }
        }
        15 | 16 => {
            if let Some(k) = arc_kind(ctx, r.data, ty == 16) {
                out.push((common, k));
            }
        }
        17 => {
            if let Some(k) = text_kind(ctx, r.data) {
                out.push((common, k));
            }
        }
        7 => {
            // A text node: its texts follow it.
            let end = members_end(raws, i, r.group_end());
            let mut j = i + 1;
            while j < end {
                j = take(ctx, d, raws, j, depth + 1, out);
            }
            return end.max(i + 1);
        }
        12 | 14 => {
            let end = members_end(raws, i, r.group_end());
            let members: Vec<&Raw> = raws.get(i + 1..end).map(|s| s.iter().collect()).unwrap_or_default();
            if let Some(k) = chain(ctx, &members, ty == 14) {
                out.push((common, k));
            }
            return end.max(i + 1);
        }
        27 => {
            let mut j = i + 1;
            while raws.get(j).is_some_and(|m| m.complex() && matches!(m.ty(), 21 | 26 | 28)) {
                j += 1;
            }
            let members: Vec<&Raw> = raws.get(i + 1..j).map(|s| s.iter().collect()).unwrap_or_default();
            match bspline(ctx, r, &members) {
                Some(k) => out.push((common, k)),
                None => ctx.skip("B-spline curves that couldn't be read"),
            }
            return j;
        }
        2 => {
            let end = members_end(raws, i, r.group_end());
            let mut members = Vec::new();
            let mut j = i + 1;
            while j < end {
                j = take(ctx, d, raws, j, depth + 1, &mut members);
            }
            let name = format!("{}{}", rad50_text(u16_at(r.data, 38).unwrap_or(0)), rad50_text(u16_at(r.data, 40).unwrap_or(0))).trim().to_string();
            let Some((origin, t)) = cell_transform(ctx, r.data) else { return end.max(i + 1) };
            let det = t[0] * t[3] - t[1] * t[2];
            if det.abs() < 1e-12 || members.is_empty() {
                out.extend(members);
                return end.max(i + 1);
            }
            // The members are stored placed: back into the cell's own coordinates.
            // placed = origin + T·local, T = [[t0, t1], [t2, t3]].
            let to_local = Mat3::translate(-origin.xy());
            let inv = [t[3] / det, -t[1] / det, -t[2] / det, t[0] / det];
            let m_inv = Mat3 { a: inv[0], b: inv[2], c: inv[1], d: inv[3], e: 0.0, f: 0.0 }.then_before(to_local);
            let base = if name.is_empty() { "CELL".to_string() } else { name };
            let members: Vec<(Common, EntityKind)> = members
                .into_iter()
                .map(|(c, mut k)| {
                    k.transform(&m_inv);
                    (c, k)
                })
                .collect();
            let sx = (t[0] * t[0] + t[2] * t[2]).sqrt();
            let sy = (t[1] * t[1] + t[3] * t[3]).sqrt() * det.signum();
            // Copies of one cell are one block: the same body within what the file can tell apart
            // (a UOR, back in the cell's own units, and the fixed-point matrix's precision).
            let size = members
                .iter()
                .map(|(c, k)| cadaddy_doc::entity_bounds(d, &Entity { handle: Default::default(), common: c.clone(), kind: k.clone() }, 1))
                .fold(Bounds2::EMPTY, |a, b| a.union(&b));
            let size = if size.is_empty() { 0.0 } else { (size.max - size.min).len() };
            let tol = (1e-5 * size).max(2.0 * ctx.scale / sx.min(sy.abs()).max(1e-9)).max(1e-9);
            let tol = if tol.is_finite() { tol } else { 1e-9 };
            let block_name = cell_block(ctx, d, &base, members, tol);
            let ins = cadaddy_doc::Insert {
                block: block_name,
                insert: origin,
                scale: Vec3::new(sx, sy, 1.0),
                rotation: t[2].atan2(t[0]),
                attribs: Vec::new(),
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            };
            out.push((common, EntityKind::Insert(ins)));
            return end.max(i + 1);
        }
        18 | 19 | 24 => {
            ctx.skip(match ty {
                18 => "3D surfaces",
                19 => "3D solids",
                _ => "B-spline surfaces",
            });
            let end = members_end(raws, i, r.group_end());
            return end.max(i + 1);
        }
        33 => ctx.skip("dimensions"),
        34 | 35 => ctx.skip("shared cells"),
        36 => ctx.skip("multilines"),
        37 => ctx.skip("tags"),
        23 => ctx.skip("cones"),
        21 | 25 | 26 | 28 => ctx.skip("B-spline parts outside a curve"),
        87..=94 => ctx.skip("raster data"),
        66 => ctx.skip("application elements"),
        other => ctx.skip(&format!("elements of type {other}")),
    }
    i + 1
}

/// Read a DGN V7 file. A DGN V8 file is refused with a clear message.
pub fn read(bytes: &[u8]) -> Result<Drawing> {
    if is_v8(bytes) {
        return Err(IoError::Format("DGN V8 isn't supported yet; save it as DGN V7 in MicroStation (or export DWG/DXF)".into()));
    }
    if !is_v7(bytes) {
        return Err(IoError::Format("not a DGN V7 design file".into()));
    }
    if bytes.len() > MAX_FILE {
        return Err(IoError::Format(format!("DGN files over {} MB aren't read", MAX_FILE >> 20)));
    }
    let raws = elements(bytes);
    let mut ctx = Ctx {
        scale: 1.0,
        origin: Vec3::ZERO,
        dim3: false,
        palette: None,
        skipped: BTreeMap::new(),
        levels: Default::default(),
        styles: Default::default(),
        cells: BTreeMap::new(),
        suffix: BTreeMap::new(),
    };
    let mut d = Drawing::new_imperial();
    // The TCB: dimension, units, global origin.
    if let Some(tcb) = raws.first().filter(|r| r.ty() == 9 && r.data.len() >= 1264) {
        ctx.dim3 = tcb.data.get(1214).is_some_and(|b| b & 0x40 != 0) || tcb.data.first() == Some(&0xC8);
        let sub_per_master = i32_at(tcb.data, 1112).unwrap_or(0);
        let uor_per_sub = i32_at(tcb.data, 1116).unwrap_or(0);
        let per = f64::from(sub_per_master) * f64::from(uor_per_sub);
        if per.is_finite() && per > 0.0 {
            ctx.scale = 1.0 / per;
        }
        let o = |i: usize| vax_at(tcb.data, i).unwrap_or(0.0) * ctx.scale;
        ctx.origin = Vec3::new(o(1240), o(1248), o(1256));
        if !ctx.origin.is_finite() {
            ctx.origin = Vec3::ZERO;
        }
        let units: String = tcb
            .data
            .get(1120..1122)
            .unwrap_or(&[])
            .iter()
            .map(|b| char::from(*b))
            .collect::<String>()
            .trim()
            .trim_end_matches('\0')
            .to_ascii_lowercase();
        let (insunits, metric) = match units.as_str() {
            "in" | "\"" => (1, false),
            "ft" | "'" => (2, false),
            "mi" => (3, false),
            "mm" => (4, true),
            "cm" => (5, true),
            "m" => (6, true),
            "km" => (7, true),
            _ => (0, false),
        };
        if insunits != 0 {
            d.header.set_i64("INSUNITS", insunits);
            d.header.set_i64("MEASUREMENT", i64::from(metric));
        }
    }
    // The colour table (group data on level 1).
    if let Some(ct) = raws.iter().find(|r| r.ty() == 5 && r.level() == 1 && r.data.len() >= 806) {
        let rgb =
            |i: usize| Rgb(ct.data.get(i).copied().unwrap_or(0), ct.data.get(i + 1).copied().unwrap_or(0), ct.data.get(i + 2).copied().unwrap_or(0));
        let mut p: Vec<Rgb> = (0..255).map(|k| rgb(41 + k * 3)).collect();
        p.push(rgb(38));
        ctx.palette = Some(p);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < raws.len() {
        i = take(&mut ctx, &mut d, &raws, i, 0, &mut out);
    }
    for level in &ctx.levels {
        d.ensure_layer(&format!("Level {level}"));
    }
    let library = cadaddy_doc::library::standard_linetypes_for(&d);
    for s in &ctx.styles {
        if let Some(name) = STYLES.get(usize::from(*s)).filter(|n| **n != "CONTINUOUS")
            && d.linetype(name).is_none()
            && let Some(lt) = library.iter().find(|l| l.name == *name)
        {
            d.linetypes.push(lt.clone());
        }
    }
    for (c, k) in out {
        let _ = d.add(&Space::Model, c, k);
    }
    if ctx.dim3 {
        d.read_notes.push("DGN: a 3D design file, read in plan (rotations about other axes are left out)".into());
    }
    for (what, n) in &ctx.skipped {
        d.read_notes.push(format!("DGN: {n} {what} not read"));
    }
    Ok(d)
}

// ---------------- writing ----------------

/// The element being built: its header and display header, then its own bytes.
struct Writer {
    /// UORs per master unit.
    per: f64,
    /// Added to master coordinates before they become UORs (centres the drawing).
    shift: Vec2,
    out: Vec<u8>,
    levels: BTreeMap<String, u8>,
    notes: BTreeMap<String, usize>,
}

/// What an element is drawn with.
#[derive(Clone, Copy)]
struct Sym {
    level: u8,
    color: u8,
    style: u8,
    weight: u8,
}

impl Writer {
    fn uor(&self, v: f64) -> i32 {
        (v * self.per).round().clamp(-2_147_483_647.0, 2_147_483_647.0) as i32
    }
    fn put_pt(&self, e: &mut [u8], i: usize, p: Vec2) {
        put_i32(e, i, self.uor(p.x + self.shift.x));
        put_i32(e, i + 4, self.uor(p.y + self.shift.y));
    }
    fn put_pt_vax(&self, e: &mut [u8], i: usize, p: Vec2) {
        put_vax(e, i, (p.x + self.shift.x) * self.per);
        put_vax(e, i + 8, (p.y + self.shift.y) * self.per);
    }
    /// A new element of type `ty`, `len` bytes, with its header, range and symbology.
    fn element(&self, ty: u8, len: usize, sym: Sym, range: Bounds2) -> Vec<u8> {
        let len = len.max(36) + len % 2;
        let mut e = vec![0u8; len];
        e[0] = sym.level.clamp(1, 63);
        e[1] = ty;
        put_u16(&mut e, 2, (len / 2 - 2) as u16);
        // The range: low x, y, z then high, as unsigned ("binary offset") integers.
        let r = if range.is_empty() { Bounds2::new(Vec2::ZERO, Vec2::ZERO) } else { range };
        for (k, v) in
            [r.min.x + self.shift.x, r.min.y + self.shift.y, 0.0, r.max.x + self.shift.x, r.max.y + self.shift.y, 0.0].into_iter().enumerate()
        {
            let u = (self.uor(v) as u32) ^ 0x8000_0000;
            put_i32(&mut e, 4 + k * 4, u as i32);
        }
        put_u16(&mut e, 30, ((len - 32) / 2) as u16);
        e[34] = (sym.style & 7) | (sym.weight.min(31) << 3);
        e[35] = sym.color;
        e
    }
}

/// Mark an element as a member of a complex group.
fn member(mut e: Vec<u8>) -> Vec<u8> {
    if let Some(b) = e.first_mut() {
        *b |= 0x80;
    }
    e
}

/// An entity's symbology in the file: its layer's level, its colour (an ACI index), style and
/// weight.
fn sym(d: &Drawing, w: &Writer, c: &Common) -> Sym {
    let layer = d.layer(&c.layer);
    let level = w.levels.get(&c.layer.to_ascii_uppercase()).copied().unwrap_or(1);
    let color = match c.color {
        Color::Index(i) => i,
        Color::True(rgb) => cadaddy_color::nearest_aci(rgb),
        Color::ByLayer => match layer.map(|l| l.color) {
            Some(Color::Index(i)) => i,
            Some(Color::True(rgb)) => cadaddy_color::nearest_aci(rgb),
            _ => 7,
        },
        Color::ByBlock => 7,
    };
    let lt = if c.linetype.eq_ignore_ascii_case("ByLayer") { layer.map(|l| l.linetype.clone()).unwrap_or_default() } else { c.linetype.to_string() };
    let style = STYLES.iter().position(|s| s.eq_ignore_ascii_case(&lt)).unwrap_or(0) as u8;
    let lw = match c.lineweight {
        Lineweight::ByLayer => layer.map(|l| l.lineweight).unwrap_or_default(),
        other => other,
    };
    let weight = match lw {
        Lineweight::Mm100(v) => ((f64::from(v) / 10.0).round() as u8).min(31),
        _ => 0,
    };
    Sym { level, color, style, weight }
}

fn bounds(pts: impl IntoIterator<Item = Vec2>) -> Bounds2 {
    Bounds2::from_points(pts)
}

/// Line strings (or shapes) of at most [`MAX_VERTICES`] points; more as a complex chain/shape.
fn polyline_elements(w: &Writer, pts: &[Vec2], closed: bool, s: Sym) -> Vec<Vec<u8>> {
    let mut pts = pts.to_vec();
    if closed
        && pts.first() != pts.last()
        && let Some(p) = pts.first().copied()
    {
        pts.push(p);
    }
    if pts.len() < 2 {
        return Vec::new();
    }
    let one = |w: &Writer, chunk: &[Vec2], ty: u8| -> Vec<u8> {
        let mut e = w.element(ty, 38 + 8 * chunk.len(), s, bounds(chunk.iter().copied()));
        put_u16(&mut e, 36, chunk.len() as u16);
        for (i, p) in chunk.iter().enumerate() {
            w.put_pt(&mut e, 38 + i * 8, *p);
        }
        e
    };
    if pts.len() == 2 && !closed {
        let mut e = w.element(3, 52, s, bounds(pts.iter().copied()));
        w.put_pt(&mut e, 36, pts[0]);
        w.put_pt(&mut e, 44, pts[1]);
        return vec![e];
    }
    if pts.len() <= MAX_VERTICES {
        return vec![one(w, &pts, if closed { 6 } else { 4 })];
    }
    // Chunks sharing their end points, under a complex header.
    let mut members = Vec::new();
    let mut start = 0;
    while start + 1 < pts.len() {
        let end = (start + MAX_VERTICES).min(pts.len());
        if let Some(chunk) = pts.get(start..end) {
            members.push(member(one(w, chunk, 4)));
        }
        start = end - 1;
    }
    complex(w, if closed { 14 } else { 12 }, s, bounds(pts.iter().copied()), members)
}

/// A complex header (chain 12, shape 14) and its members.
fn complex(w: &Writer, ty: u8, s: Sym, range: Bounds2, members: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let words: usize = members.iter().map(|m| m.len() / 2).sum();
    let mut h = w.element(ty, 40, s, range);
    put_u16(&mut h, 36, (words + 1).min(u16::MAX as usize) as u16);
    put_u16(&mut h, 38, members.len() as u16);
    std::iter::once(h).chain(members).collect()
}

/// An arc element (2D): centre, axes, rotation and start/sweep in degrees.
fn arc_element(w: &Writer, s: Sym, c: Vec2, a: f64, b: f64, rot: f64, start: f64, sweep: f64) -> Vec<u8> {
    let r = a.max(b);
    let range = Bounds2::new(c - Vec2::new(r, r), c + Vec2::new(r, r));
    let mut e = w.element(16, 80, s, range);
    put_i32(&mut e, 36, (start * 360_000.0).round() as i32);
    let mut sw = ((sweep.abs() * 360_000.0).round() as i64).clamp(0, 0x7fff_ffff) as i32;
    if sweep.abs() >= 359.9999 {
        sw = 0;
    }
    put_i32(&mut e, 40, sw);
    if sweep < 0.0
        && let Some(byte) = e.get_mut(41)
    {
        *byte |= 0x80;
    }
    put_vax(&mut e, 44, a * w.per);
    put_vax(&mut e, 52, b * w.per);
    put_i32(&mut e, 60, (rot * 360_000.0).round() as i32);
    w.put_pt_vax(&mut e, 64, c);
    e
}

fn ellipse_element(w: &Writer, s: Sym, c: Vec2, a: f64, b: f64, rot: f64) -> Vec<u8> {
    let r = a.max(b);
    let mut e = w.element(15, 72, s, Bounds2::new(c - Vec2::new(r, r), c + Vec2::new(r, r)));
    put_vax(&mut e, 36, a * w.per);
    put_vax(&mut e, 44, b * w.per);
    put_i32(&mut e, 52, (rot * 360_000.0).round() as i32);
    w.put_pt_vax(&mut e, 56, c);
    e
}

fn text_element(w: &Writer, s: Sym, t: &Text) -> Option<Vec<u8>> {
    let value: String = t.value.chars().take(250).collect();
    if value.is_empty() {
        return None;
    }
    let latin: Option<Vec<u8>> = value.chars().map(|c| u8::try_from(u32::from(c)).ok().filter(|b| *b >= 0x20)).collect();
    let bytes = match latin {
        Some(b) if b.len() <= 255 => b,
        _ => {
            let mut b = vec![0xFF, 0xFD];
            for u in value.encode_utf16().take(126) {
                b.extend(u.to_le_bytes());
            }
            b
        }
    };
    let just = match (t.halign, t.valign) {
        (HAlign::Left, VAlign::Baseline) | (HAlign::Left, VAlign::Bottom) => 2,
        (h, v) => {
            let col = match h {
                HAlign::Left => 0,
                HAlign::Center | HAlign::Middle | HAlign::Aligned | HAlign::Fit => 1,
                HAlign::Right => 2,
            };
            let row = match v {
                VAlign::Top => 0,
                VAlign::Middle => 1,
                _ => 2,
            };
            col * 6 + row
        }
    };
    let origin = if just == 2 { t.insert.xy() } else { t.align_pt.unwrap_or(t.insert).xy() };
    let length = t.height * t.width_factor;
    let width = length * value.chars().count() as f64;
    let range = Bounds2::new(origin - Vec2::new(width, t.height), origin + Vec2::new(width, t.height));
    let mut e = w.element(17, 60 + bytes.len(), s, range);
    e[36] = 1;
    e[37] = just;
    let k = 1000.0 / (6.0 / w.per);
    put_i32(&mut e, 38, (length * k).round() as i32);
    put_i32(&mut e, 42, (t.height * k).round() as i32);
    put_i32(&mut e, 46, (t.rotation.to_degrees() * 360_000.0).round() as i32);
    w.put_pt(&mut e, 50, origin);
    e[58] = bytes.len() as u8;
    if let Some(dst) = e.get_mut(60..60 + bytes.len()) {
        dst.copy_from_slice(&bytes);
    }
    Some(e)
}

/// The elements one object becomes (`m` places it: a block's objects inside a cell); nothing for
/// what V7 can't hold (counted in `notes`).
fn object(d: &Drawing, w: &mut Writer, e: &Entity, m: &Mat3, depth: usize) -> Vec<Vec<u8>> {
    let s = sym(d, w, &e.common);
    let mut k = e.kind.clone();
    if !matches!(k, EntityKind::Insert(_)) {
        k.transform(m);
    }
    let note = |w: &mut Writer, what: &str| {
        *w.notes.entry(what.to_string()).or_default() += 1;
        Vec::new()
    };
    match &k {
        EntityKind::Line(l) => polyline_elements(w, &[l.a.xy(), l.b.xy()], false, s),
        EntityKind::Point(p) => {
            let mut el = w.element(3, 52, s, bounds([p.p.xy()]));
            w.put_pt(&mut el, 36, p.p.xy());
            w.put_pt(&mut el, 44, p.p.xy());
            vec![el]
        }
        EntityKind::LwPolyline(pl) => lwpolyline(w, pl, s),
        EntityKind::Circle(c) => vec![ellipse_element(w, s, c.center.xy(), c.radius, c.radius, 0.0)],
        EntityKind::Arc(a) => {
            let mut sweep = (a.end - a.start).to_degrees();
            while sweep <= 0.0 {
                sweep += 360.0;
            }
            vec![arc_element(w, s, a.center.xy(), a.radius, a.radius, 0.0, a.start.to_degrees(), sweep)]
        }
        EntityKind::Ellipse(el) => {
            let a = el.major.xy().len();
            let b = a * el.ratio;
            let rot = el.major.xy().angle().to_degrees();
            let full = (el.end - el.start).abs() >= std::f64::consts::TAU - 1e-9;
            if full {
                vec![ellipse_element(w, s, el.center.xy(), a, b, rot)]
            } else {
                let mut sweep = (el.end - el.start).to_degrees();
                while sweep <= 0.0 {
                    sweep += 360.0;
                }
                vec![arc_element(w, s, el.center.xy(), a, b, rot, el.start.to_degrees(), sweep)]
            }
        }
        EntityKind::Text(t) => text_element(w, s, t).into_iter().collect(),
        EntityKind::Spline(sp) => {
            let pts = sp.tessellate(1e-3 * sp.control.iter().fold(1.0f64, |m, p| m.max(p.len())));
            note(w, "splines (as line strings through their curve)");
            polyline_elements(w, &pts, sp.closed, s)
        }
        EntityKind::Insert(ins) => {
            let Some(blk) = d.block(&ins.block) else { return Vec::new() };
            if depth >= MAX_DEPTH {
                return note(w, "block references nested too deep");
            }
            let place = m.then_before(ins.transform(blk.base.xy()));
            let mut members = Vec::new();
            for be in blk.entities.iter() {
                for el in object(d, w, be, &place, depth + 1) {
                    members.push(member(el));
                }
            }
            for a in &ins.attribs {
                let mut t = a.text.clone();
                let mut tk = EntityKind::Text(t.clone());
                tk.transform(m);
                if let EntityKind::Text(tt) = tk {
                    t = tt;
                }
                if let Some(el) = text_element(w, s, &t) {
                    members.push(member(el));
                }
            }
            if members.is_empty() {
                return Vec::new();
            }
            cell(w, s, &ins.block, place, members)
        }
        EntityKind::Hatch(h) => {
            note(w, "hatches (as their boundaries)");
            let mut out = Vec::new();
            for l in &h.loops {
                let pl = LwPolyline { vertices: l.vertices.clone(), closed: true, const_width: 0.0, elevation: 0.0, plinegen: false };
                out.extend(lwpolyline(w, &pl, s));
            }
            out
        }
        other => {
            let name = other.type_name().to_string();
            note(w, &format!("{name} objects"))
        }
    }
}

/// A polyline: line strings (a shape when closed); with arcs, a complex chain or shape of
/// line strings and arcs.
fn lwpolyline(w: &Writer, pl: &LwPolyline, s: Sym) -> Vec<Vec<u8>> {
    let has_arcs = pl.vertices.iter().any(|v| v.bulge.abs() > 1e-12);
    if !has_arcs {
        return polyline_elements(w, &pl.vertices.iter().map(|v| v.p).collect::<Vec<_>>(), pl.closed, s);
    }
    // Straight runs as line strings, bulged segments as arcs, in a complex chain/shape.
    let n = pl.vertices.len();
    let segs = if pl.closed { n } else { n.saturating_sub(1) };
    let mut members = Vec::new();
    let mut run: Vec<Vec2> = Vec::new();
    let flush = |w: &Writer, run: &mut Vec<Vec2>, members: &mut Vec<Vec<u8>>| {
        if run.len() >= 2 {
            for el in polyline_elements(w, run, false, s) {
                members.push(member(el));
            }
        }
        run.clear();
    };
    for i in 0..segs {
        let (Some(a), Some(b)) = (pl.vertices.get(i), pl.vertices.get((i + 1) % n)) else { continue };
        if a.bulge.abs() <= 1e-12 {
            if run.is_empty() {
                run.push(a.p);
            }
            run.push(b.p);
            continue;
        }
        flush(w, &mut run, &mut members);
        let theta = 4.0 * a.bulge.atan();
        let chord = b.p - a.p;
        let len = chord.len();
        if len <= 1e-12 {
            continue;
        }
        let r = len / (2.0 * (theta / 2.0).sin()).abs();
        let mid = (a.p + b.p) * 0.5;
        let h = r * (theta / 2.0).cos() * theta.signum();
        let c = mid + Vec2::new(-chord.y, chord.x) * (h / len);
        let start = (a.p - c).angle().to_degrees();
        members.push(member(arc_element(w, s, c, r, r, 0.0, start, theta.to_degrees())));
    }
    flush(w, &mut run, &mut members);
    let range = bounds(pl.vertices.iter().map(|v| v.p));
    complex(w, if pl.closed { 14 } else { 12 }, s, range, members)
}

/// A cell header for `members` placed by `place` (rotation and scale from its 2×2 part).
fn cell(w: &Writer, s: Sym, name: &str, place: Mat3, members: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let origin = place.apply(Vec2::ZERO);
    let ux = place.apply(Vec2::new(1.0, 0.0)) - origin;
    let uy = place.apply(Vec2::new(0.0, 1.0)) - origin;
    let words: usize = members.iter().map(|m| m.len() / 2).sum();
    let mut h = w.element(2, 92, s, Bounds2::new(origin, origin));
    put_u16(&mut h, 36, ((92 - 38) / 2 + words).min(u16::MAX as usize) as u16);
    let clean: String = name.to_ascii_uppercase().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '$' | '.')).take(6).collect();
    put_u16(&mut h, 38, rad50_word(clean.get(..clean.len().min(3)).unwrap_or("")));
    put_u16(&mut h, 40, rad50_word(clean.get(3.min(clean.len())..).unwrap_or("")));
    // Levels used: bit (level − 1).
    let mut mask = [0u8; 8];
    for m in &members {
        let l = usize::from(m.first().map_or(1, |b| (b & 0x3f).max(1)) - 1);
        if let Some(b) = mask.get_mut(l / 8) {
            *b |= 1 << (l % 8);
        }
    }
    if let Some(dst) = h.get_mut(44..52) {
        dst.copy_from_slice(&mask);
    }
    for (i, v) in [ux.x, uy.x, ux.y, uy.y].into_iter().enumerate() {
        put_i32(&mut h, 68 + i * 4, (v * 214_748.0).round().clamp(-2_147_483_647.0, 2_147_483_647.0) as i32);
    }
    w.put_pt(&mut h, 84, origin);
    std::iter::once(h).chain(members).collect()
}

/// A drawing's model space as a 2D DGN V7 file, with what couldn't be written (counts by kind).
pub fn write(d: &Drawing) -> Result<(Vec<u8>, Vec<String>)> {
    let ext = d.extents(&Space::Model);
    let center = if ext.is_empty() { Vec2::ZERO } else { ext.center() };
    let half = if ext.is_empty() { 0.0 } else { ext.width().max(ext.height()) / 2.0 };
    // 10,000 UORs a master unit (a ten-thousandth of a foot or a metre), fewer for a drawing
    // too big for 32-bit coordinates.
    let mut per = 10_000.0;
    while half * per > 1.9e9 && per > 1e-6 {
        per /= 10.0;
    }
    let mut w = Writer { per, shift: -center, out: Vec::new(), levels: BTreeMap::new(), notes: BTreeMap::new() };
    for (i, l) in d.layers.iter().enumerate() {
        w.levels.insert(l.name.to_ascii_uppercase(), (i + 1).min(63) as u8);
    }
    if d.layers.len() > 63 {
        *w.notes.entry("layers past the 63rd (on level 63)".into()).or_default() += d.layers.len() - 63;
    }
    // The TCB.
    let mut tcb = vec![0u8; 1536];
    tcb[0] = 0x08;
    tcb[1] = 0x09;
    put_u16(&mut tcb, 2, 766);
    let unit = match d.header.i64("INSUNITS", 0) {
        1 => "in",
        2 => "ft",
        3 => "mi",
        4 => "mm",
        5 => "cm",
        6 => "m ",
        7 => "km",
        _ => "mu",
    };
    put_i32(&mut tcb, 1112, 1);
    put_i32(&mut tcb, 1116, per.round().max(1.0) as i32);
    if per < 1.0 {
        // Fewer than one UOR a unit: a sub unit of 10ⁿ masters.
        put_i32(&mut tcb, 1112, 1);
        put_i32(&mut tcb, 1116, 1);
        w.per = 1.0;
    }
    if let Some(dst) = tcb.get_mut(1120..1122) {
        dst.copy_from_slice(unit.as_bytes());
    }
    if let Some(dst) = tcb.get_mut(1122..1124) {
        dst.copy_from_slice(unit.as_bytes());
    }
    // The global origin (UORs): master = UOR / per − origin, with UOR = (master + shift) · per;
    // the shift a whole number of UORs, so points on the grid stay exactly on it.
    w.shift = Vec2::new((w.shift.x * w.per).round() / w.per, (w.shift.y * w.per).round() / w.per);
    put_vax(&mut tcb, 1240, w.shift.x * w.per);
    put_vax(&mut tcb, 1248, w.shift.y * w.per);
    put_vax(&mut tcb, 1256, 0.0);
    w.out.extend(tcb);
    // The colour table: the ACI colours, so colour numbers mean the same.
    let s = Sym { level: 1, color: 0, style: 0, weight: 0 };
    let mut ct = w.element(5, 806, s, Bounds2::EMPTY);
    ct[0] = 1;
    let rgb = |i: u8| cadaddy_color::aci_rgb(i);
    let c255 = rgb(255);
    ct[38..41].copy_from_slice(&[c255.0, c255.1, c255.2]);
    for k in 0..255u8 {
        let c = if k == 0 { Rgb(0, 0, 0) } else { rgb(k) };
        let at = 41 + usize::from(k) * 3;
        if let Some(dst) = ct.get_mut(at..at + 3) {
            dst.copy_from_slice(&[c.0, c.1, c.2]);
        }
    }
    w.out.extend(ct);
    let identity = Mat3::IDENTITY;
    let mut els = Vec::new();
    for e in d.model.iter() {
        els.extend(object(d, &mut w, e, &identity, 0));
    }
    for el in els {
        w.out.extend(el);
    }
    w.out.extend([0xff, 0xff]);
    let notes = w.notes.iter().map(|(what, n)| format!("{n} {what} not written as such")).collect();
    Ok((w.out, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip_through_the_file_encodings() {
        for v in [0.0, 1.0, -1.0, 0.1, 123456.789, -9.5e-4, 3.0e15, 1000.0, f64::MIN_POSITIVE * 1e300] {
            let mut b = [0u8; 8];
            put_vax(&mut b, 0, v);
            let back = vax_at(&b, 0).unwrap();
            assert!((back - v).abs() <= 1e-15 * v.abs().max(1e-300), "{v} → {back}");
        }
        // 1.0 is VAX exponent 129, fraction 0: the first word 0x4080.
        let mut b = [0u8; 8];
        put_vax(&mut b, 0, 1.0);
        assert_eq!(&b[..2], &[0x80, 0x40]);
        for v in [0, 1, -1, 123_456_789, -2_000_000_000, i32::MAX, i32::MIN] {
            let mut b = [0u8; 4];
            put_i32(&mut b, 0, v);
            assert_eq!(i32_at(&b, 0), Some(v));
        }
        // 0x00010002 stored high word first, each word little-endian.
        let mut b = [0u8; 4];
        put_i32(&mut b, 0, 0x0001_0002);
        assert_eq!(b, [1, 0, 2, 0]);
        assert_eq!(rad50_text(rad50_word("AB1")), "AB1");
        assert_eq!(rad50_text(rad50_word("Z$.")), "Z$.");
        assert_eq!(rad50_text(rad50_word("X")), "X  ");
    }
}
