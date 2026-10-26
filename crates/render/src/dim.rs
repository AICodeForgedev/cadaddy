//! Dimension geometry generation from definition points and a dimension style: extension
//! lines (DIMEXO/DIMEXE, DIMSE1/2), arrowheads (DIMBLK/DIMBLK1/DIMBLK2, drawn in code) or ticks
//! (DIMTSZ, DIMDLE), fit (DIMATFIT, DIMTIX, DIMSOXD, DIMTOFL), suppressed halves (DIMSD1/2),
//! fixed-length extension lines (DIMFXL/DIMFXLON), text placement (DIMTAD, DIMTVP, DIMJUST,
//! DIMTIH/DIMTOH, DIMGAP, DIMTMOVE) and text formatting (DIMLUNIT, DIMDEC, DIMZIN, DIMRND,
//! DIMPOST, DIMTOL/DIMLIM with DIMTZIN and DIMTOLJ, DIMALT with DIMALTU/DIMALTZ/DIMALTRND,
//! DIMAZIN, DIMARCSYM) with its background (DIMTFILL).

use cadcraft_doc::{DimKind, DimStyle, Dimension};
use cadcraft_fonts::{MTextParams, TextFont};
use cadcraft_geom::{Arc, Vec2, ccw_sweep};

use crate::units::{NumFormat, format_angle_with_separator, format_decimal, format_linear};

mod arrowheads;
mod text;

use arrowheads::tick;
pub use arrowheads::{ArrowGeom, Arrowhead, arrow, arrowhead};
pub use text::{DimText, format_linear_value};
use text::{apply_override, inspected, linear_text};

/// Most DIMBREAK gaps drawn per dimension.
const MAX_BREAKS: usize = 1000;

/// What a dimension line primitive is (for DIMCLRD / DIMCLRE colours).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineRole {
    /// Dimension line, arrowheads, ticks, leaders (DIMCLRD).
    Dim,
    /// Extension lines and centre marks (DIMCLRE).
    Ext,
}

#[derive(Clone, Debug, Default)]
pub struct DimGeometry {
    /// Every line (dimension and extension lines, open arrowheads, ticks).
    pub lines: Vec<Vec<Vec2>>,
    /// Role of each entry of `lines`.
    pub line_roles: Vec<LineRole>,
    /// Filled triangles (three points each) in the dimension line colour.
    pub fills: Vec<Vec<Vec2>>,
    /// DIMTFILL 1 or 2: the corners of the text's background, the text box widened by DIMGAP
    /// on every side (empty without a fill or text).
    pub text_fill: Vec<Vec2>,
    /// Indices into `lines` of the marks drawn with the text, over `text_fill`: the arc length
    /// symbol above the text (DIMARCSYM 1) and the DIMINSPECT frame.
    pub text_marks: Vec<usize>,
    /// Text strokes (stroke font, fraction bars, underlines).
    pub text: Vec<Vec<Vec2>>,
    /// Text glyph outlines (TrueType), grouped per glyph.
    pub text_glyphs: Vec<Vec<Vec<Vec2>>>,
    /// The font glyphs of `text_glyphs` (outline indices into it).
    pub text_runs: Vec<cadcraft_fonts::TextGlyph>,
    pub text_pos: Vec2,
    /// The measured value (DXF group 42): a length, or the angle in radians.
    pub measurement: f64,
    pub text_angle: f64,
    pub text_height: f64,
    /// Plain text of the dimension.
    pub value: String,
    /// The text as MTEXT contents (stacked fractions and tolerances).
    pub mtext: String,
    /// Indices into `lines` of the second extension line (DIMLTEX2); the other extension
    /// lines take DIMLTEX1.
    pub ext2_lines: Vec<usize>,
}

impl DimGeometry {
    /// `value` as the contents of a plain MTEXT (the dimension's `*D` block, its EXPLODE
    /// pieces): the diameter sign as `%%c`, not the decoded ⌀ (U+2300), which no SHX font has,
    /// so other programs plot nothing for it, while every font draws `%%c` (#1645). A `%`
    /// that would run into a code is written `%%%`.
    pub fn mtext_contents(&self) -> String {
        let mut out = String::with_capacity(self.value.len() + 2);
        let mut chars = self.value.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '⌀' => out.push_str("%%c"),
                '%' if matches!(chars.peek(), Some('%' | '⌀')) => out.push_str("%%%"),
                c => out.push(c),
            }
        }
        out
    }
    fn dim(&mut self, l: Vec<Vec2>) {
        if l.len() >= 2 {
            self.lines.push(l);
            self.line_roles.push(LineRole::Dim);
        }
    }
    /// A dimension line that marks the text (see `text_marks`).
    fn text_mark(&mut self, l: Vec<Vec2>) {
        if l.len() >= 2 {
            self.text_marks.push(self.lines.len());
            self.dim(l);
        }
    }
    fn ext(&mut self, l: Vec<Vec2>) {
        if l.len() >= 2 {
            self.lines.push(l);
            self.line_roles.push(LineRole::Ext);
        }
    }
    /// Leave out the parts of the lines (of `role`, or all) within `r` of `c`.
    fn cut(&mut self, c: Vec2, r: f64, role: Option<LineRole>) {
        if !(r > 0.0 && r.is_finite() && c.is_finite()) {
            return;
        }
        let mut lines = Vec::with_capacity(self.lines.len());
        let mut roles = Vec::with_capacity(self.lines.len());
        let (mut ext2, mut marks) = (Vec::new(), Vec::new());
        for (i, (l, rl)) in self.lines.iter().zip(&self.line_roles).enumerate() {
            let pieces = if role.is_none_or(|x| x == *rl) { cut_polyline(l, c, r) } else { vec![l.clone()] };
            for p in pieces {
                if self.ext2_lines.contains(&i) {
                    ext2.push(lines.len());
                }
                if self.text_marks.contains(&i) {
                    marks.push(lines.len());
                }
                lines.push(p);
                roles.push(*rl);
            }
        }
        (self.lines, self.line_roles, self.ext2_lines, self.text_marks) = (lines, roles, ext2, marks);
    }
    /// Extension line 1 or 2 (`second`).
    fn ext_n(&mut self, l: Option<Vec<Vec2>>, second: bool) {
        if let Some(l) = l.filter(|l| l.len() >= 2) {
            if second {
                self.ext2_lines.push(self.lines.len());
            }
            self.ext(l);
        }
    }
    fn fill(&mut self, tris: Vec<Vec2>) {
        for t in tris.chunks(3) {
            if t.len() == 3 {
                self.fills.push(t.to_vec());
            }
        }
    }
    /// The lines with a role.
    pub fn lines_of(&self, role: LineRole) -> impl Iterator<Item = &Vec<Vec2>> {
        self.lines.iter().zip(&self.line_roles).filter(move |(_, r)| **r == role).map(|(l, _)| l)
    }
    fn add_arrow(&mut self, a: ArrowGeom) {
        for l in a.lines {
            self.dim(l);
        }
        self.fill(a.tris);
    }
}

/// The pieces of a polyline outside the circle (`c`, `r`).
fn cut_polyline(l: &[Vec2], c: Vec2, r: f64) -> Vec<Vec<Vec2>> {
    let mut out: Vec<Vec<Vec2>> = Vec::new();
    let mut keep = |a: Vec2, b: Vec2| {
        if a.dist(b) <= 1e-12 {
            return;
        }
        match out.last_mut() {
            Some(last) if last.last() == Some(&a) => last.push(b),
            _ => out.push(vec![a, b]),
        }
    };
    for w in l.windows(2) {
        let (p, q) = (w[0], w[1]);
        let dv = q - p;
        let (qa, f) = (dv.dot(dv), p - c);
        let (qb, qc) = (2.0 * f.dot(dv), f.dot(f) - r * r);
        let disc = qb * qb - 4.0 * qa * qc;
        if qa < 1e-24 || disc.is_nan() || disc <= 0.0 {
            keep(p, q);
            continue;
        }
        let sq = disc.sqrt();
        let (t0, t1) = ((-qb - sq) / (2.0 * qa), (-qb + sq) / (2.0 * qa));
        if t1 <= 0.0 || t0 >= 1.0 {
            keep(p, q);
            continue;
        }
        if t0 > 0.0 {
            keep(p, p + dv * t0);
        }
        if t1 < 1.0 {
            keep(p + dv * t1, q);
        }
    }
    out
}

fn ext_line(origin: Vec2, foot: Vec2, st: &DimStyle, k: f64) -> Option<Vec<Vec2>> {
    let v = foot - origin;
    let len = v.len();
    if len < 1e-12 {
        return None;
    }
    let u = v / len;
    let mut start = origin + u * (st.ext_offset * k);
    // DIMFXLON: a fixed length from the dimension line toward the origin (never past the offset).
    let fixed = st.fixed_ext_length * k;
    if st.fixed_ext && fixed.is_finite() && fixed >= 0.0 && fixed < len - st.ext_offset * k {
        start = foot - u * fixed;
    }
    Some(vec![start, foot + u * (st.ext_extend * k)])
}

/// An angle reduced to the readable range (text never upside down).
fn readable(a: f64) -> f64 {
    let mut a = cadcraft_geom::norm_angle(a);
    if a > std::f64::consts::FRAC_PI_2 + 1e-9 && a <= 3.0 * std::f64::consts::FRAC_PI_2 + 1e-9 {
        a -= std::f64::consts::PI;
    }
    a
}

/// Generate the geometry of a dimension with the stroke font.
pub fn dimension_geometry(d: &Dimension, st: &DimStyle, dimscale: f64) -> DimGeometry {
    dimension_geometry_with(d, st, dimscale, &DimText { width_factor: 1.0, ..Default::default() })
}

/// Generate the geometry of a dimension. The dimension's own overrides are applied to `st`.
pub fn dimension_geometry_with(d: &Dimension, st: &DimStyle, dimscale: f64, font: &DimText) -> DimGeometry {
    let st = &st.with_overrides(&d.overrides);
    let k = st.effective_scale(dimscale);
    let asz = st.arrow_size * k;
    let th = if font.fixed_height > 0.0 { font.fixed_height } else { st.text_height * k };
    let gap = st.text_gap.abs() * k;
    let tsz = st.tick_size * k;
    let ticks = tsz > 0.0;
    let blk1 = Arrowhead::parse(if st.arrow_block1.is_empty() { &st.arrow_block } else { &st.arrow_block1 });
    let blk2 = Arrowhead::parse(if st.arrow_block2.is_empty() { &st.arrow_block } else { &st.arrow_block2 });
    let mut g = DimGeometry { text_height: th, ..Default::default() };
    // Measure the text: lay it out once at the origin, centred.
    let wf = if font.width_factor > 0.0 && font.width_factor.is_finite() { font.width_factor } else { 1.0 };
    let params = |mt: Vec2, rot: f64| MTextParams {
        attach: 5,
        rotation: rot,
        font: font.font.clone(),
        width_factor: wf,
        oblique: font.oblique,
        ..MTextParams::new(mt, th)
    };
    let measure = |text: &str| {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        let l = cadcraft_fonts::layout_mtext_with(text, &params(Vec2::ZERO, 0.0));
        (l.bounds.width(), l.bounds.height())
    };
    let mut text_angle = 0.0;
    let mut text_mid = d.text_mid.xy();
    // Place an end marker: tick or arrowhead. Returns how far the dimension line stops short.
    let end = |g: &mut DimGeometry, kind: Arrowhead, tip: Vec2, dir: Vec2| -> f64 {
        if ticks {
            g.dim(tick(tip, dir, tsz));
            0.0
        } else {
            g.add_arrow(arrowhead(kind, tip, dir, asz));
            kind.inset(asz)
        }
    };
    match d.kind {
        DimKind::Linear { .. } | DimKind::Aligned => {
            let p1 = d.p13.xy();
            let p2 = d.p14.xy();
            let on = d.defpt.xy();
            let dir = match d.kind {
                DimKind::Linear { rotation } => Vec2::from_angle(rotation),
                _ => (p2 - p1).normalized(),
            };
            let dir = if dir == Vec2::ZERO || !dir.is_finite() { Vec2::X } else { dir };
            // DIMEDIT Oblique: extension lines at their own angle (not along the dimension line).
            let oblique = d.marks.oblique.map(Vec2::from_angle).filter(|e| e.is_finite() && e.cross(dir).abs() > 1e-6);
            let foot = |p: Vec2| match oblique {
                Some(e) => p + e * ((on - p).cross(dir) / e.cross(dir)),
                None => on + dir * (p - on).dot(dir),
            };
            let f1 = foot(p1);
            let f2 = foot(p2);
            // The measurement is along the dimension direction whatever the extension lines do.
            let meas = (p2 - p1).dot(dir).abs();
            g.mtext = linear_text(meas, d, st, "");
            g.measurement = meas;
            // From here on: the length drawn between the extension lines.
            let meas = f1.dist(f2);
            if !st.suppress_ext1 {
                g.ext_n(ext_line(p1, f1, st, k), false);
            }
            if !st.suppress_ext2 {
                g.ext_n(ext_line(p2, f2, st, k), true);
            }
            let u = (f2 - f1).normalized();
            let u = if u == Vec2::ZERO || !u.is_finite() { dir } else { u };
            let n = u.perp();
            let (tw, tht) = measure(&g.mtext);
            let has_text = tw > 0.0;
            let tick_like = ticks || (blk1.is_tick() && blk2.is_tick());
            let aw = if tick_like { 0.0 } else { asz };
            let arrows_fit = tick_like || meas > 2.0 * asz + 1e-9;
            // Text orientation: inside / outside horizontal.
            let along_ang = readable(u.angle());
            let extent = |ang: f64| {
                // Half-extent of the text box projected on the dimension line.
                let r = Vec2::from_angle(ang);
                (tw * r.dot(u).abs() + tht * r.perp().dot(u).abs()) / 2.0
            };
            let inside_ang = if st.text_inside_horizontal { 0.0 } else { along_ang };
            // DIMTVP (with DIMTAD 0) lifts the text off the line once it is 0.7 text heights or more.
            let tvp = if st.text_above == 0 && st.text_vertical_pos.is_finite() { st.text_vertical_pos } else { 0.0 };
            let centred = st.text_above == 0 && tvp.abs() < 0.7;
            let text_need = if centred { 2.0 * (extent(inside_ang) + gap) } else { 2.0 * extent(inside_ang) };
            let fits = !has_text || meas >= text_need + 2.0 * aw;
            let text_fits = !has_text || meas >= text_need;
            let just = st.text_just.min(4);
            // DIMATFIT / DIMTIX: what goes outside the extension lines when text and arrows
            // don't both fit between them. Text the user placed, or over an extension line, is
            // not part of the fit.
            let (text_in, arrows_inside) = if d.user_text_pos || !has_text || just >= 3 {
                (true, arrows_fit)
            } else if fits {
                (true, true)
            } else if st.text_inside {
                (true, false)
            } else {
                match st.fit {
                    0 => (false, false),
                    1 => (text_fits, false),
                    2 => (false, arrows_fit),
                    _ if arrows_fit => (false, true),
                    _ => (text_fits, false),
                }
            };
            let text_outside = !text_in;
            text_angle = if text_outside { if st.text_outside_horizontal { 0.0 } else { along_ang } } else { inside_ang };
            let half = extent(text_angle);
            // DIMTMOVE 1 / 2: text that doesn't fit is moved off the dimension line, which stays put.
            let moved = text_outside && st.text_move != 0;
            // DIMSOXD: arrowheads that don't fit inside are left out.
            let outside_arrows = !arrows_inside && !st.suppress_outside_arrows;
            // Text centre along the dimension line.
            let along_c = if text_outside {
                f2 + u * (2.0 * asz.max(tsz) + gap + half)
            } else {
                match just {
                    1 => f1 + u * (aw + gap + half + asz),
                    2 => f2 - u * (aw + gap + half + asz),
                    _ => f1.mid(f2),
                }
            };
            // Perpendicular offset per DIMTAD.
            let side_out = if (on - p1).dot(n) >= 0.0 { n } else { -n };
            let tr = Vec2::from_angle(text_angle);
            let off_along = |m: Vec2| {
                let glyph_extent = tw * tr.dot(m).abs() / 2.0 + tht * tr.perp().dot(m).abs() * if st.text_above == 1 { 0.6 } else { 0.5 };
                // Empirical #2004 match: 0.595× height with SHX and 0.601× with TrueType.
                gap + glyph_extent
            };
            let above = if text_angle.abs() < 1e-12 && n.y.abs() > 1e-9 {
                if n.y > 0.0 { n } else { -n }
            } else if text_angle.abs() < 1e-12 {
                if n.x < 0.0 { n } else { -n }
            } else if tr.perp().dot(n) >= 0.0 {
                n
            } else {
                -n
            };
            let offset = match st.text_above {
                0 => above * (tvp * th),
                2 => side_out * off_along(side_out),
                4 => -above * off_along(-above),
                _ => above * off_along(above),
            };
            if just >= 3 && !d.user_text_pos && has_text {
                // Over an extension line, aligned with it.
                let (p, f) = if just == 3 { (p1, f1) } else { (p2, f2) };
                let e = (f - p).normalized();
                let e = if e == Vec2::ZERO { side_out } else { e };
                text_angle = readable(e.angle());
                let outward = if just == 3 { -u } else { u };
                text_mid = f + e * (st.ext_extend * k + gap + tw / 2.0) + outward * (gap + tht / 2.0);
            } else if moved {
                // Over the middle of the line, clear of the arrowheads, on the side away from the
                // measured points; with DIMTMOVE 1 it starts there and a leader joins it.
                let mid = f1.mid(f2);
                let lift = off_along(side_out) + 2.0 * asz.max(tsz);
                text_mid = if st.text_move == 1 { mid + side_out * lift + u * half } else { mid + side_out * (lift + off_along(side_out)) };
            } else if !d.user_text_pos {
                text_mid = along_c + offset;
            }
            // Dimension line. DIMSD1 / DIMSD2 leave out its first / second half (with the arrow).
            let dle = if tick_like { st.dim_line_extend * k } else { 0.0 };
            let lo = if st.suppress_dim1 { meas / 2.0 } else { f64::NEG_INFINITY };
            let hi = if st.suppress_dim2 { meas / 2.0 } else { f64::INFINITY };
            let seg = |g: &mut DimGeometry, a: Vec2, b: Vec2| {
                let (ta, tb) = ((a - f1).dot(u), (b - f1).dot(u));
                let (t0, t1) = (ta.min(tb).max(lo), ta.max(tb).min(hi));
                if t1 > t0 + 1e-12 {
                    g.dim(vec![f1 + u * t0, f1 + u * t1]);
                }
            };
            // The line between `a` and `b`, broken around centred text between the extension lines.
            let inner = |g: &mut DimGeometry, a: Vec2, b: Vec2| {
                if centred && has_text && !text_outside && just <= 2 && !d.user_text_pos {
                    let h = extent(text_angle) + gap;
                    let (t0, t1) = ((along_c - u * h - f1).dot(u), (along_c + u * h - f1).dot(u));
                    if t0 > (a - f1).dot(u) + 1e-9 {
                        seg(g, a, f1 + u * t0);
                    }
                    if t1 < (b - f1).dot(u) - 1e-9 {
                        seg(g, f1 + u * t1, b);
                    }
                } else {
                    seg(g, a, b);
                }
            };
            // Out to text outside the extension lines, from `from` along the line.
            let to_text = |g: &mut DimGeometry, from: Vec2| {
                let to = text_mid - offset - u * (half + gap);
                if text_outside && (to - from).dot(u) > 0.0 {
                    seg(g, from, from + u * (to - from).dot(u));
                }
            };
            if arrows_inside {
                let i1 = if st.suppress_dim1 { 0.0 } else { end(&mut g, blk1, f1, -u) };
                let i2 = if st.suppress_dim2 { 0.0 } else { end(&mut g, blk2, f2, u) };
                inner(&mut g, f1 - u * dle + u * i1, f2 + u * dle - u * i2);
                to_text(&mut g, f2);
            } else {
                let stub = asz * 2.0;
                if outside_arrows {
                    if !st.suppress_dim1 {
                        end(&mut g, blk1, f1, u);
                    }
                    if !st.suppress_dim2 {
                        end(&mut g, blk2, f2, -u);
                    }
                    seg(&mut g, f1 - u * stub, f1);
                    seg(&mut g, f2, f2 + u * stub);
                }
                // DIMTOFL: the line between the extension lines as well.
                if st.force_line_inside {
                    inner(&mut g, f1, f2);
                }
                to_text(&mut g, if outside_arrows { f2 + u * stub } else { f2 });
            }
            // DIMTMOVE 1: a leader from the dimension line to text moved off it.
            if (d.user_text_pos || moved) && st.text_move == 1 && has_text {
                let base = f1.mid(f2);
                let off = (text_mid - base).dot(n).abs();
                if off > tht / 2.0 + gap + 1e-9 {
                    let side = if (base - text_mid).dot(tr) >= 0.0 { tr } else { -tr };
                    g.dim(vec![base, text_mid + side * (tw / 2.0 + gap)]);
                }
            }
            // DIMJOGLINE: a zigzag in the dimension line, 1.5 text heights tall.
            if let Some(j) = d.marks.jog_line {
                let at = f1 + u * (j.xy() - f1).dot(u);
                let h = 1.5 * th;
                if at.is_finite() && h > 0.0 {
                    let r = h / 2.0;
                    g.cut(at, r, Some(LineRole::Dim));
                    g.dim(vec![at - u * r, at - u * (r / 4.0) + n * (h / 2.0), at + u * (r / 4.0) - n * (h / 2.0), at + u * r]);
                }
            }
        }
        DimKind::Radius if d.marks.jogged.is_some() => 'jogged: {
            // DIMJOGGED: from the arrowhead on the arc toward the centre, a jog at DIMJOGANG,
            // then on to the overriding centre (parallel to the first part).
            let center = d.defpt.xy();
            let tip = d.p15.xy();
            let r = center.dist(tip);
            let j = d.marks.jogged.clone().unwrap_or_default();
            let (o, jp) = (j.center.xy(), j.jog.xy());
            g.mtext = linear_text(r, d, st, "R");
            // A NaN or infinite radius leaves no line to draw (and NaN is no bound for `clamp`).
            if !r.is_finite() {
                break 'jogged;
            }
            let (tw, _) = measure(&g.mtext);
            let u = (tip - center).normalized();
            let u = if u == Vec2::ZERO || !u.is_finite() { Vec2::X } else { u };
            let n = u.perp();
            let h = (o - center).dot(n);
            let ang = if st.jog_angle.is_finite() { st.jog_angle.clamp(5.0, 90.0) } else { 45.0 }.to_radians();
            let run = h.abs() / ang.tan().max(1e-6);
            let tj = (jp - center).dot(u).clamp(0.0, r);
            let (j1, j2) = (center + u * (tj + run / 2.0), center + u * (tj - run / 2.0) + n * h);
            g.dim(vec![tip, j1, j2, o]);
            end(&mut g, blk2, tip, u);
            let tpos = if d.user_text_pos { text_mid } else { tip + u * (asz.max(tsz) * 3.0) };
            if tpos.dist(center) > r {
                g.dim(vec![tip, tpos]);
            }
            text_angle = if st.text_outside_horizontal { 0.0 } else { readable(u.angle()) };
            if !d.user_text_pos {
                let x = Vec2::from_angle(text_angle);
                let x = if x.dot(u) >= 0.0 { x } else { -x };
                text_mid = tpos + x * (tw / 2.0 + gap);
            }
        }
        DimKind::Radius | DimKind::Diameter => {
            let a = d.defpt.xy();
            let b = d.p15.xy();
            let radius = matches!(d.kind, DimKind::Radius);
            let (center, r) = if radius { (a, a.dist(b)) } else { (a.mid(b), a.dist(b) / 2.0) };
            let meas = if radius { r } else { 2.0 * r };
            g.mtext = linear_text(meas, d, st, if radius { "R" } else { "%%c" });
            g.measurement = meas;
            let (tw, tht) = measure(&g.mtext);
            let u = (b - center).normalized();
            let u = if u == Vec2::ZERO || !u.is_finite() { Vec2::X } else { u };
            let tip = center + u * r;
            let tpos = if d.user_text_pos { text_mid } else { tip + u * (asz.max(tsz) * 3.0) };
            // Text placed inside the circle (a dimension line location picked inside): the text
            // sits on the dimension line, which runs from the text out to the arrowhead (for a
            // diameter it spans the circle, broken around the text). DIMTIH turns it horizontal.
            let inside = d.user_text_pos && text_mid.dist(center) < r;
            let horizontal = if inside { st.text_inside_horizontal } else { st.text_outside_horizontal };
            if inside {
                let tr = Vec2::from_angle(if horizontal { 0.0 } else { readable(u.angle()) });
                let half = if tw > 0.0 { (tw * tr.dot(u).abs() + tht * tr.perp().dot(u).abs()) / 2.0 + gap } else { 0.0 };
                let start = if radius { center } else { center - u * r };
                let (t0, t1) = ((text_mid - start).dot(u) - half, (text_mid - start).dot(u) + half);
                if !radius {
                    if t0 > 0.0 {
                        g.dim(vec![start, start + u * t0]);
                    }
                    end(&mut g, blk1, start, -u);
                }
                if t1 < tip.dist(start) {
                    g.dim(vec![start + u * t1.max(0.0), tip]);
                }
            } else {
                // Text outside: a diameter line spans the circle and a radius line runs in to the
                // centre only with DIMTOFL; otherwise just the leader to the text.
                if st.force_line_inside {
                    let start = if radius { center } else { center - u * r };
                    g.dim(vec![start, tip]);
                    if !radius {
                        end(&mut g, blk1, start, -u);
                    }
                }
                // Text the user placed: the leader stops short of it by the offset the default
                // placement leaves between leader end and text.
                let lead = if !d.user_text_pos {
                    tpos
                } else if horizontal {
                    text_mid - Vec2::new(tw / 2.0 + gap, 0.0) * if u.x >= 0.0 { 1.0 } else { -1.0 }
                } else {
                    text_mid - u * (tw / 2.0 + gap)
                };
                g.dim(vec![tip, lead]);
            }
            end(&mut g, blk2, tip, u);
            if horizontal {
                text_angle = 0.0;
                let sx = if u.x >= 0.0 { 1.0 } else { -1.0 };
                if !d.user_text_pos {
                    text_mid = tpos + Vec2::new(tw / 2.0 + gap, 0.0) * sx;
                }
            } else {
                text_angle = readable(u.angle());
                if !d.user_text_pos {
                    text_mid = tpos + u * (tw / 2.0 + gap);
                }
            }
            // The DIMCEN mark is drawn only when no dimension line runs inside the circle (text
            // outside without DIMTOFL), never with a line through the centre.
            let line_outside = !st.force_line_inside && tpos.dist(center) >= r;
            if line_outside && st.center_mark != 0.0 && st.center_mark.is_finite() {
                let c = st.center_mark.abs() * k;
                g.ext(vec![center - Vec2::X * c, center + Vec2::X * c]);
                g.ext(vec![center - Vec2::Y * c, center + Vec2::Y * c]);
                if st.center_mark < 0.0 && r > 2.0 * c {
                    // Centre lines out past the circle.
                    for dv in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                        g.ext(vec![center + dv * (2.0 * c), center + dv * (r + c)]);
                    }
                }
            }
        }
        DimKind::Angular | DimKind::Angular3P => {
            // Angular3P: p15 vertex, p13/p14 on the legs, defpt on the arc.
            // Angular (2 lines): lines p13-p14 and defpt-p15; p16 on the arc.
            let (vertex, a1, a2, arc_pt) = if matches!(d.kind, DimKind::Angular3P) {
                (d.p15.xy(), d.p13.xy(), d.p14.xy(), d.defpt.xy())
            } else {
                let v = cadcraft_geom::line_line_infinite(d.p13.xy(), d.p14.xy(), d.defpt.xy(), d.p15.xy()).map(|x| x.0).unwrap_or(d.p14.xy());
                (v, d.p14.xy(), d.p15.xy(), d.p16.xy())
            };
            let r = vertex.dist(arc_pt).max(1e-9);
            let s = vertex.angle_to(a1);
            let e = vertex.angle_to(a2);
            let arc =
                if cadcraft_geom::angle_in_sweep(vertex.angle_to(arc_pt), s, e) { Arc::new(vertex, r, s, e) } else { Arc::new(vertex, r, e, s) };
            let sweep = ccw_sweep(arc.start, arc.end);
            g.measurement = sweep;
            // DIMAZIN 1 / 2 suppress leading / trailing zeros (DIMZIN's bits 4 / 8).
            let azin = (st.angular_zero_suppression & 3) * 4;
            let txt = format_angle_with_separator(sweep, st.angular_unit, st.angular_decimals, azin, &st.decimal_separator);
            // DIMPOST is a linear-dimension prefix/suffix; angles have their own format only.
            g.mtext = inspected(apply_override(&txt, d), d);
            let mut pts = Vec::new();
            arc.tessellate(r * 1e-3, &mut pts);
            g.dim(pts);
            let sp = arc.start_point();
            let ep = arc.end_point();
            // The arc's ends with the outward tangents there, paired with leg 1 and leg 2: when
            // the arc location is outside the leg 1 → leg 2 sweep the arc runs from leg 2 to
            // leg 1, and each leg keeps its own extension line, DIMSE1/2 and DIMBLK1/2.
            let (start, finish) = ((sp, -(sp - vertex).perp().normalized()), (ep, (ep - vertex).perp().normalized()));
            let ((p1, dir1), (p2, dir2)) =
                if cadcraft_geom::angle_in_sweep(vertex.angle_to(arc_pt), s, e) { (start, finish) } else { (finish, start) };
            if !st.suppress_ext1 {
                g.ext_n(ext_line(vertex + (p1 - vertex).normalized() * vertex.dist(a1).min(r), p1, st, k), false);
            }
            if !st.suppress_ext2 {
                g.ext_n(ext_line(vertex + (p2 - vertex).normalized() * vertex.dist(a2).min(r), p2, st, k), true);
            }
            end(&mut g, blk1, p1, dir1);
            end(&mut g, blk2, p2, dir2);
            let (tw, tht) = measure(&g.mtext);
            let mid = arc.mid_point();
            let radial = (mid - vertex).normalized();
            text_angle = if st.text_inside_horizontal { 0.0 } else { readable(radial.perp().angle()) };
            if !d.user_text_pos {
                let tr = Vec2::from_angle(text_angle);
                let off = gap + (tw * tr.dot(radial).abs() + tht * tr.perp().dot(radial).abs()) / 2.0;
                text_mid = mid + radial * off;
            }
        }
        DimKind::Ordinate { x_type } => {
            let feature = d.p13.xy();
            let leader_end = d.p14.xy();
            let origin = d.defpt.xy();
            let meas = if x_type { (feature.x - origin.x).abs() } else { (feature.y - origin.y).abs() };
            g.mtext = linear_text(meas, d, st, "");
            g.measurement = meas;
            let (tw, _) = measure(&g.mtext);
            g.dim(vec![feature, leader_end]);
            if x_type {
                text_angle = std::f64::consts::FRAC_PI_2;
            }
            if !d.user_text_pos {
                let dirv = (leader_end - feature).normalized();
                let dirv = if dirv == Vec2::ZERO { if x_type { Vec2::Y } else { Vec2::X } } else { dirv };
                text_mid = leader_end + dirv * (tw / 2.0 + gap);
            }
        }
        DimKind::ArcLength => {
            let c = d.p15.xy();
            let r = c.dist(d.p13.xy());
            let arc = Arc::new(c, c.dist(d.defpt.xy()).max(1e-9), c.angle_to(d.p13.xy()), c.angle_to(d.p14.xy()));
            let meas = r * arc.sweep();
            // DIMARCSYM: the arc symbol before the text (0), over it (1) or none (2).
            g.mtext = linear_text(meas, d, st, if st.arc_symbol == 0 { "⌒" } else { "" });
            g.measurement = meas;
            let mut pts = Vec::new();
            arc.tessellate(arc.radius * 1e-3, &mut pts);
            g.dim(pts);
            for (i, (p, q)) in [(d.p13.xy(), arc.start_point()), (d.p14.xy(), arc.end_point())].into_iter().enumerate() {
                g.ext_n(ext_line(p, q, st, k), i == 1);
            }
            end(&mut g, blk1, arc.start_point(), -(arc.start_point() - c).perp().normalized());
            end(&mut g, blk2, arc.end_point(), (arc.end_point() - c).perp().normalized());
            let m = arc.mid_point();
            let radial = (m - c).normalized();
            text_angle = if st.text_inside_horizontal { 0.0 } else { readable(radial.perp().angle()) };
            if !d.user_text_pos {
                text_mid = m + radial * (gap + th / 2.0);
            }
            if st.arc_symbol == 1 {
                // A small arc over the text.
                let (tw, tht) = measure(&g.mtext);
                let (x, y) = (Vec2::from_angle(text_angle), Vec2::from_angle(text_angle).perp());
                let half = (tw / 2.0).max(th / 2.0);
                let base = text_mid + y * (tht / 2.0 + gap / 2.0);
                let sym: Vec<Vec2> = (0..=12)
                    .map(|i| {
                        let t = f64::from(i) / 6.0 - 1.0;
                        base + x * (t * half) + y * ((1.0 - t * t).max(0.0).sqrt() * th * 0.35)
                    })
                    .collect();
                g.text_mark(sym);
            }
        }
    }
    // DIMTEDIT Angle: an explicit text rotation wins.
    if d.text_rotation != 0.0 && d.text_rotation.is_finite() {
        text_angle = d.text_rotation;
    }
    // DIMBREAK: gaps in the dimension and extension lines.
    for b in d.marks.breaks.iter().take(MAX_BREAKS) {
        g.cut(b.at.xy(), b.size / 2.0, None);
    }
    // DIMINSPECT: a frame round the text, with separators between label, value and rate.
    if let Some(ins) = &d.marks.inspect
        && ins.shape < 2
        && !g.mtext.is_empty()
    {
        let (w, h) = measure(&g.mtext);
        let (x, y) = (Vec2::from_angle(text_angle), Vec2::from_angle(text_angle).perp());
        let at = |px: f64, py: f64| text_mid + x * px + y * py;
        let (hw, hh) = (w / 2.0 + gap, h / 2.0 + gap);
        // Widths of the label, value and rate as laid out: the two spaces between parts measured
        // in "x  x", the value is what the rest leaves.
        let gap2 = (measure("x  x").0 - 2.0 * measure("x").0).max(0.0);
        let (wl, wr) = (measure(&ins.label).0, measure(&ins.rate).0);
        let seps = usize::from(!ins.label.is_empty()) + usize::from(!ins.rate.is_empty());
        let wv = (w - wl - wr - gap2 * seps as f64).max(0.0);
        let mut left = -w / 2.0;
        if !ins.label.is_empty() {
            left += wl;
            g.text_mark(vec![at(left + gap2 / 2.0, -hh), at(left + gap2 / 2.0, hh)]);
            left += gap2;
        }
        if !ins.rate.is_empty() {
            left += wv;
            g.text_mark(vec![at(left + gap2 / 2.0, -hh), at(left + gap2 / 2.0, hh)]);
        }
        g.text_mark(vec![at(-hw, hh), at(hw, hh)]);
        g.text_mark(vec![at(-hw, -hh), at(hw, -hh)]);
        for side in [-1.0, 1.0] {
            let ends: Vec<Vec2> = if ins.shape == 0 {
                (0..=12).map(|i| f64::from(i) * std::f64::consts::PI / 12.0).map(|a| at(side * (hw + hh * a.sin()), hh * a.cos())).collect()
            } else {
                vec![at(side * hw, hh), at(side * (hw + hh), 0.0), at(side * hw, -hh)]
            };
            g.text_mark(ends);
        }
    }
    // DIMTFILL: a background behind the text, the text box plus DIMGAP on every side.
    if st.text_fill != 0 && !g.mtext.is_empty() {
        let (w, h) = measure(&g.mtext);
        let (x, y) = (Vec2::from_angle(text_angle), Vec2::from_angle(text_angle).perp());
        let (hw, hh) = (w / 2.0 + gap, h / 2.0 + gap);
        g.text_fill = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].into_iter().map(|(px, py)| text_mid + x * px + y * py).collect();
    }
    g.value = decode_value(&cadcraft_fonts::plain_mtext(&g.mtext));
    if !g.mtext.is_empty() {
        let l = cadcraft_fonts::layout_mtext_with(&g.mtext, &params(text_mid, text_angle));
        for p in l.pieces {
            let base = g.text_glyphs.len();
            g.text_runs.extend(p.shaped.text.into_iter().map(|t| cadcraft_fonts::TextGlyph { outline: t.outline + base, ..t }));
            g.text.extend(p.shaped.strokes);
            g.text_glyphs.extend(p.shaped.glyphs);
        }
    }
    g.text_pos = text_mid;
    g.text_angle = text_angle;
    g
}

/// `%%` codes in the plain value become their characters (`%%c` → ⌀, `%%p` → ±, `%%d` → °).
fn decode_value(s: &str) -> String {
    cadcraft_fonts::decode_controls(s).into_iter().map(|(c, _, _)| c).collect()
}

#[cfg(test)]
mod tests;
