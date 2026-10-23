//! Reading scanned prints: a grey scan of a sheet becomes a drawing the vector readers
//! ([`crate::duct_reader`], [`crate::pipe_reader`]) can read, so the same pipeline runs on paper.
//!
//! 1. **Deskew**: the sheet's turn on the glass is found from its ink (the angle at which the
//!    rows of ink line up sharpest) and undone.
//! 2. **Text** ([`crate::ocr`]): tags, notes, legends and title blocks are read in the sheet's
//!    own stroke font, upright and turned, and taken out of the ink.
//! 3. **Linework**: what is left is traced (IMAGETRACE's vectoriser, with the Q209 accuracy
//!    work) to lines, arcs and circles; dashes are joined back into dashed lines.
//! 4. **Alignment**: the structural grid's bubbles (circles round a read label) are matched to
//!    the building grid, which gives the scan's scale, quarter turn and offset — no sheet
//!    coordinates are taken from anywhere else.
//! 5. **Layers**: a scan has no layers, so they are inferred the way a reader of the print would:
//!    the title strip and border, lineweight (runs are drawn heavier than leaders), dashes,
//!    symbols (small boxes are diffusers and grilles, larger ones equipment), and the read text.
//!
//! Every inferred item carries its source sheet, inference status and confidence in
//! [`ScanRead::sources`]; scores against the truth are in the tests.

use crate::layout;
use crate::ocr::{self, Comp, Ink, LetterSize, TextLine};
use crate::scan::{self, Scan};
use crate::truth::{Grid, Sheet};
use cadaddy_doc::{Arc, Circle, Common, Drawing, EntityKind, HAlign, Line, LwPolyline, Space, Text, VAlign};
use cadaddy_geom::vectorize::{self, Shape};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};

/// Ink is at most this grey: black ink, not the screened background (colour 8 prints ~128).
pub const INK: u8 = 90;
/// Fainter pixels joined to ink are ink too (a thin stroke softened by the scanner or JPEG).
pub const INK_WEAK: u8 = 150;
/// How [`crate::duct_reader`] reads a scan's drawing: walls come in pieces, so every pair is
/// chained.
pub const DUCT_OPTIONS: crate::duct_reader::DuctOptions = crate::duct_reader::DuctOptions { all_pairs: true };
/// Most traced lines handled after tracing.
const MAX_LINES: usize = 200_000;
/// Largest skew searched, degrees.
const MAX_SKEW_DEG: f64 = 4.0;

/// The scan → building map: building feet = `scale` × R(`turn_deg`) × (x, y up) + `offset`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PxToBuilding {
    pub turn_deg: f64,
    pub scale: f64,
    pub offset: [f64; 2],
    /// Root-mean-square misfit of the grid bubbles, feet.
    pub rms_ft: f64,
}

impl PxToBuilding {
    pub fn apply(&self, p: Vec2) -> [f64; 2] {
        let (s, c) = self.turn_deg.to_radians().sin_cos();
        [self.scale * (p.x * c - p.y * s) + self.offset[0], self.scale * (p.x * s + p.y * c) + self.offset[1]]
    }
    /// Pixel lengths to building feet.
    pub fn len(&self, px: f64) -> f64 {
        px * self.scale
    }
}

/// Where an item read from a scan came from, and how confident the reader is.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanSource {
    /// The scan sheet this item was read from (e.g. `"M-101"`).
    pub sheet: String,
    /// How the item was inferred: `"rule"` (layer / lineweight / pattern), `"ocr"` (text read),
    /// `"grid"` (structural grid alignment).
    pub status: String,
    /// 0.0–1.0: how sure the reader is. Scans are always lower than vector reads.
    pub confidence: f64,
}

/// A scan read back.
#[derive(Clone, Debug, Default)]
pub struct ScanRead {
    /// The sheet as a drawing, in the sheet's model space (so readers given the sheet work
    /// unchanged), with inferred layers.
    pub drawing: Drawing,
    /// The text read, image pixels (y up) of the deskewed scan.
    pub text: Vec<TextLine>,
    /// The skew found and undone, degrees counter-clockwise as seen.
    pub skew_deg: f64,
    /// Grid bubbles found: label and centre (pixels, y up).
    pub bubbles: Vec<(String, Vec2)>,
    pub map: PxToBuilding,
    /// Traced lines by inferred layer, for reports.
    pub layer_counts: std::collections::BTreeMap<String, usize>,
    /// Every inferred item's source, status and confidence, keyed by its layer.
    pub sources: Vec<ScanSource>,
    /// The sheet number read from the title block (if any).
    pub title_block_number: Option<String>,
    /// The discipline read from the title block (if any).
    pub title_block_discipline: Option<String>,
}

/// The skew of a scan: the angle (degrees, counter-clockwise as seen) at which the ink's rows
/// are sharpest. Long horizontal lines (borders, runs, the title strip) dominate.
pub fn skew_deg(s: &Scan) -> f64 {
    let (w, h) = (s.w, s.h);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let n = s.gray.len().min(w.saturating_mul(h));
    let ink = s.gray.iter().take(n).filter(|&&g| g <= INK).count();
    let step = (ink / 300_000).max(1);
    let mut k = 0usize;
    for (i, &g) in s.gray.iter().take(n).enumerate() {
        if g <= INK {
            k += 1;
            if k.is_multiple_of(step) {
                pts.push(((i % w) as f64, (i / w) as f64));
            }
        }
    }
    if pts.len() < 10 {
        return 0.0;
    }
    let rows = h + w;
    let sharp = |deg: f64| -> f64 {
        let t = deg.to_radians().tan();
        let mut hist = vec![0u32; rows * 8 + 8];
        for (x, y) in &pts {
            let v = (y + x * t + w as f64) * 4.0;
            if v >= 0.0
                && let Some(c) = hist.get_mut(v as usize)
            {
                *c += 1;
            }
        }
        hist.iter().map(|&c| f64::from(c) * f64::from(c)).sum()
    };
    let mut best = (0.0, sharp(0.0));
    let mut a = -MAX_SKEW_DEG;
    while a <= MAX_SKEW_DEG + 1e-9 {
        let v = sharp(a);
        if v > best.1 {
            best = (a, v);
        }
        a += 0.1;
    }
    let centre = best.0;
    let mut a = centre - 0.1;
    while a <= centre + 0.1 + 1e-9 {
        let v = sharp(a);
        if v > best.1 {
            best = (a, v);
        }
        a += 0.01;
    }
    best.0
}

/// A traced straight line, pixels (y up), with what the scan says about it.
#[derive(Clone, Debug)]
struct Seg {
    a: Vec2,
    b: Vec2,
    /// Ink width across it, pixels.
    width: f64,
    /// Pieces it was joined from (dashes) and their mean length, pixels.
    pieces: usize,
    dash: f64,
}

impl Seg {
    fn len(&self) -> f64 {
        self.a.dist(self.b)
    }
    fn dir(&self) -> Vec2 {
        let d = self.b - self.a;
        let l = d.len();
        if l > 1e-9 { d * (1.0 / l) } else { Vec2::new(1.0, 0.0) }
    }
}

/// The ink's width across a line: the median run of ink perpendicular to it at a few places.
fn ink_width(ink: &Ink, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l = d.len();
    if l < 1e-9 {
        return 0.0;
    }
    let (u, n) = (d * (1.0 / l), Vec2::new(-d.y / l, d.x / l));
    let mut runs = Vec::new();
    let ih = ink.h as f64;
    let at = |p: Vec2| ink.get(p.x.floor() as i64, (ih - p.y).floor() as i64);
    for k in 1..8 {
        let c = a + u * (l * f64::from(k) / 8.0);
        if !at(c) {
            continue;
        }
        let mut run = 1.0;
        for sgn in [-1.0, 1.0] {
            let mut t = 0.5;
            while t < 12.0 && at(c + n * (sgn * t)) {
                run += 0.5;
                t += 0.5;
            }
        }
        runs.push(run);
    }
    runs.sort_by(f64::total_cmp);
    runs.get(runs.len() / 2).copied().unwrap_or(0.0)
}

/// Join collinear pieces with short gaps (dashes, and lines a symbol or a removed letter cut).
fn join(mut segs: Vec<Seg>, max_gap: f64) -> Vec<Seg> {
    // Group by direction and offset; within a group sort along and merge.
    segs.retain(|s| s.len() > 1e-6);
    let key = |s: &Seg| {
        let d = s.dir();
        d.y.atan2(d.x).rem_euclid(std::f64::consts::PI)
    };
    segs.sort_by(|a, b| key(a).total_cmp(&key(b)));
    let mut out: Vec<Seg> = Vec::new();
    let mut used = vec![false; segs.len()];
    for i in 0..segs.len() {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(s0) = segs.get(i).cloned() else { continue };
        let ang0 = key(&s0);
        let u = s0.dir();
        let n = Vec2::new(-u.y, u.x);
        let off = n.dot(s0.a);
        let mut members = vec![i];
        for j in i + 1..segs.len() {
            let Some(sj) = segs.get(j) else { continue };
            let da = key(sj) - ang0;
            if da > 0.02 {
                break;
            }
            if used.get(j).copied().unwrap_or(true) {
                continue;
            }
            if (n.dot(sj.a) - off).abs() <= 1.6 && (n.dot(sj.b) - off).abs() <= 1.6 {
                members.push(j);
            }
        }
        // Lines near 0 and near 180 degrees are the same direction: also look at the far end.
        if ang0 < 0.02 {
            for j in (i + 1..segs.len()).rev() {
                let Some(sj) = segs.get(j) else { continue };
                if key(sj) < std::f64::consts::PI - 0.02 {
                    break;
                }
                if !used.get(j).copied().unwrap_or(true) && (n.dot(sj.a) - off).abs() <= 1.6 && (n.dot(sj.b) - off).abs() <= 1.6 {
                    members.push(j);
                }
            }
        }
        // Intervals along u.
        let mut iv: Vec<(f64, f64, usize)> =
            members.iter().filter_map(|&m| segs.get(m).map(|s| (u.dot(s.a).min(u.dot(s.b)), u.dot(s.a).max(u.dot(s.b)), m))).collect();
        iv.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut cur: Option<(f64, f64, Vec<usize>)> = None;
        let flush = |c: (f64, f64, Vec<usize>), out: &mut Vec<Seg>, used: &mut Vec<bool>| {
            let (lo, hi, ms) = c;
            let mut width = 0.0;
            let mut len = 0.0;
            let mut offs = 0.0;
            for &m in &ms {
                if let Some(s) = segs.get(m) {
                    width += s.width * s.len();
                    len += s.len();
                    offs += n.dot((s.a + s.b) * 0.5) * s.len();
                }
                if let Some(x) = used.get_mut(m) {
                    *x = true;
                }
            }
            let o = if len > 0.0 { offs / len } else { off };
            let pieces = ms.iter().filter_map(|&m| segs.get(m)).map(|s| s.pieces).sum::<usize>().max(1);
            out.push(Seg {
                a: u * lo + n * o,
                b: u * hi + n * o,
                width: if len > 0.0 { width / len } else { 0.0 },
                pieces,
                dash: len / pieces as f64,
            });
        };
        for (lo, hi, m) in iv {
            match cur.as_mut() {
                Some(c) if lo - c.1 <= max_gap => {
                    c.1 = c.1.max(hi);
                    c.2.push(m);
                }
                _ => {
                    if let Some(c) = cur.take() {
                        flush(c, &mut out, &mut used);
                    }
                    cur = Some((lo, hi, vec![m]));
                }
            }
        }
        if let Some(c) = cur.take() {
            flush(c, &mut out, &mut used);
        }
    }
    out
}

/// Lines in the screened grey ink (lighter than [`INK`], not the soft edge of black ink),
/// dashes joined.
fn grey_lines(s: &Scan, dpi: f64) -> Result<Vec<Seg>, String> {
    let (w, h) = (s.w, s.h);
    // Black ink grown by two pixels: grey next to it is antialiasing, not a grey line.
    let black: Vec<bool> = s.gray.iter().map(|&g| g <= INK).collect();
    let grow = |src: &[bool], horiz: bool| -> Vec<bool> {
        let mut out = vec![false; src.len()];
        for (i, o) in out.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            *o = (-2i64..=2).any(|d| {
                let (nx, ny) = if horiz { (x as i64 + d, y as i64) } else { (x as i64, y as i64 + d) };
                nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && src.get(ny as usize * w + nx as usize).copied().unwrap_or(false)
            });
        }
        out
    };
    let near = grow(&grow(&black, true), false);
    let gray: Vec<u8> = s.gray.iter().zip(&near).map(|(&g, &n)| if g > INK && g <= 200 && !n { 0 } else { 255 }).collect();
    let opts = vectorize::Options { threshold: Some(128), walls: false, text: false, min_length: (0.03 * dpi).max(3.0), ..Default::default() };
    let traced = vectorize::trace(&gray, w, h, &opts)?;
    let segs: Vec<Seg> = traced
        .shapes
        .iter()
        .take(MAX_LINES)
        .filter_map(|shp| match shp {
            Shape::Line { a, b, width } => {
                let (a, b) = (Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]));
                Some(Seg { a, b, width: *width, pieces: 1, dash: a.dist(b) })
            }
            _ => None,
        })
        .collect();
    Ok(join(segs, 0.1 * dpi))
}

/// Rules across the sheet (border, title strip): rows (`horiz`) or columns where axis-aligned
/// lines add up to at least `min_len`, even in pieces. Sorted.
fn rules(segs: &[Seg], horiz: bool, min_len: f64, dpi: f64) -> Vec<f64> {
    let mut at: Vec<(f64, f64)> = segs
        .iter()
        .filter(|s| if horiz { s.dir().x.abs() > 0.999 } else { s.dir().y.abs() > 0.999 })
        .map(|s| (if horiz { (s.a.y + s.b.y) / 2.0 } else { (s.a.x + s.b.x) / 2.0 }, s.len()))
        .collect();
    at.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tol = 0.02 * dpi;
    let mut out = Vec::new();
    let mut i = 0;
    while i < at.len() {
        let Some(&(p0, _)) = at.get(i) else { break };
        let mut j = i;
        let (mut sum, mut wsum) = (0.0, 0.0);
        while let Some(&(p, l)) = at.get(j) {
            if p - p0 > tol {
                break;
            }
            sum += l;
            wsum += p * l;
            j += 1;
        }
        if sum >= min_len && sum > 0.0 {
            let p = wsum / sum;
            if out.last().is_none_or(|q: &f64| p - q > 0.05 * dpi) {
                out.push(p);
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Fit the grid: building feet from pixels, by the bubbles' labels. Tries the four quarter
/// turns and keeps the one the bubbles fit best.
fn fit_grid(bubbles: &[(String, Vec2)], grid: &Grid) -> Option<PxToBuilding> {
    let mut best: Option<PxToBuilding> = None;
    for turn in [0.0, 90.0, 180.0, 270.0] {
        let (s, c) = f64::to_radians(turn).sin_cos();
        // Equations: X_L = k qx + tx (x-grid labels), Y_M = k qy + ty (y-grid labels).
        // Unknowns (k, tx, ty): normal equations.
        let mut ata = [[0.0f64; 3]; 3];
        let mut atb = [0.0f64; 3];
        let mut eqs: Vec<([f64; 3], f64)> = Vec::new();
        for (label, p) in bubbles {
            let q = [p.x * c - p.y * s, p.x * s + p.y * c];
            if let Some(g) = grid.x.iter().find(|g| g.label == *label) {
                eqs.push(([q[0], 1.0, 0.0], g.at_ft));
            }
            if let Some(g) = grid.y.iter().find(|g| g.label == *label) {
                eqs.push(([q[1], 0.0, 1.0], g.at_ft));
            }
        }
        let nx = eqs.iter().filter(|e| e.0[1] > 0.5).count();
        let ny = eqs.iter().filter(|e| e.0[2] > 0.5).count();
        if nx < 2 || ny < 1 {
            continue;
        }
        for (r, y) in &eqs {
            for i in 0..3 {
                for j in 0..3 {
                    ata[i][j] += r[i] * r[j];
                }
                atb[i] += r[i] * y;
            }
        }
        let Some(x) = solve3(ata, atb) else { continue };
        if x[0] <= 0.0 || !x.iter().all(|v| v.is_finite()) {
            continue;
        }
        let rms = (eqs.iter().map(|(r, y)| (r[0] * x[0] + r[1] * x[1] + r[2] * x[2] - y).powi(2)).sum::<f64>() / eqs.len() as f64).sqrt();
        let m = PxToBuilding { turn_deg: turn, scale: x[0], offset: [x[1], x[2]], rms_ft: rms };
        if best.as_ref().is_none_or(|b| m.rms_ft < b.rms_ft) {
            best = Some(m);
        }
    }
    best
}

/// Solve a 3 × 3 system (Cramer's rule); `None` when singular.
fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(a);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut x = [0.0; 3];
    for (k, xk) in x.iter_mut().enumerate() {
        let mut m = a;
        for (r, row) in m.iter_mut().enumerate() {
            row[k] = b[r];
        }
        *xk = det(m) / d;
    }
    Some(x)
}

/// Bubbles robust to misreads and stray labels: drop the worst-fitting bubble until the rest
/// fit to within a foot.
fn fit_grid_robust(bubbles: &[(String, Vec2)], grid: &Grid) -> Option<PxToBuilding> {
    let mut keep: Vec<(String, Vec2)> = bubbles.to_vec();
    let mut m = fit_grid(&keep, grid)?;
    for _ in 0..bubbles.len().min(200) {
        if m.rms_ft < 1.0 {
            return Some(m);
        }
        let resid = |(l, p): &(String, Vec2)| -> f64 {
            let b = m.apply(*p);
            let ex = grid.x.iter().find(|g| g.label == *l).map_or(0.0, |g| (b[0] - g.at_ft).abs());
            let ey = grid.y.iter().find(|g| g.label == *l).map_or(0.0, |g| (b[1] - g.at_ft).abs());
            ex.max(ey)
        };
        // Labels not on the grid at all go first; then the worst fit.
        let worst = keep
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let on_grid = grid.x.iter().chain(grid.y.iter()).any(|g| g.label == b.0);
                (i, if on_grid { resid(b) } else { f64::INFINITY })
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)?;
        keep.remove(worst);
        m = fit_grid(&keep, grid)?;
    }
    (m.rms_ft < 1.0).then_some(m)
}

/// Read a scanned sheet. `sh` is the sheet the scan is of (its discipline and the model space
/// the readers work in); `grid` the building grid the bubbles are matched to.
pub fn read_scan(s: &Scan, sh: &Sheet, grid: &Grid) -> Result<ScanRead, String> {
    if s.w == 0 || s.h == 0 || s.gray.len() < s.w.saturating_mul(s.h) {
        return Err("the scan has no pixels".into());
    }
    if s.w.saturating_mul(s.h) > scan::MAX_SCAN_PIXELS {
        return Err("the scan is too large".into());
    }
    if !(s.dpi.is_finite() && s.dpi > 10.0) {
        return Err("the scan's resolution is unknown".into());
    }
    // 1. Skew: found here, undone on the vectors (turning the pixels would blur thin lines).
    let skew = skew_deg(s);
    let (w, h, dpi) = (s.w, s.h, s.dpi);
    // Turning about the image's corner keeps every text line's own frame coordinates.
    let (ssn, scs) = (-skew).to_radians().sin_cos();
    let unskew = |p: Vec2| Vec2::new(p.x * scs - p.y * ssn, p.x * ssn + p.y * scs);
    let mut ink = Ink::hysteresis(&s.gray, w, h, INK, INK_WEAK);
    // 2. Text.
    let comps = ocr::components(&ink);
    let size = LetterSize { max_px: 0.25 * dpi, min_ink: ((0.013 * dpi).powi(2)).max(3.0) as usize, min_px: 0.035 * dpi };
    let mut text = ocr::read_lines(&ink, &comps, &size, skew);
    // A lone character is text only inside a frame (a grid bubble, a tag): elsewhere it is a
    // tick, a dash or a symbol.
    text.retain(|t| t.text.trim().chars().count() > 1 || enclosed(&comps, t, h, dpi));
    let text_comps: Vec<&Comp> = text.iter().flat_map(|t| t.comps.iter().filter_map(|&i| comps.get(i))).collect();
    let text_stroke = ocr::stroke_width(&ink, &text_comps);
    for t in &text {
        for &i in &t.comps {
            if let Some(c) = comps.get(i) {
                for &p in &c.px {
                    if let Some(o) = ink.on.get_mut(p) {
                        *o = false;
                    }
                }
            }
        }
    }
    drop(comps);
    for t in &mut text {
        t.origin = unskew(t.origin);
        t.angle_deg -= skew;
    }
    let text: Vec<TextLine> = text.iter().flat_map(split_tags).collect();
    // 3. Linework.
    let gray: Vec<u8> = ink.on.iter().map(|&o| if o { 0 } else { 255 }).collect();
    let opts = vectorize::Options { threshold: Some(128), walls: false, text: false, min_length: (0.02 * dpi).max(3.0), ..Default::default() };
    let traced = vectorize::trace(&gray, w, h, &opts)?;
    drop(gray);
    let mut segs = Vec::new();
    let mut circles = Vec::new();
    let mut arcs = Vec::new();
    for shp in traced.shapes.iter().take(MAX_LINES) {
        match shp {
            Shape::Line { a, b, .. } => {
                let (a, b) = (Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]));
                let width = ink_width(&ink, a, b);
                let (a, b) = (unskew(a), unskew(b));
                segs.push(Seg { a, b, width, pieces: 1, dash: a.dist(b) });
            }
            Shape::Circle { center, radius, .. } => circles.push((unskew(Vec2::new(center[0], center[1])), *radius)),
            Shape::Arc { center, radius, start, end, .. } => {
                let t = skew.to_radians();
                arcs.push((unskew(Vec2::new(center[0], center[1])), *radius, start - t, end - t));
            }
            _ => {}
        }
    }
    let segs = join(segs, 0.06 * dpi);
    // 4. Alignment by the grid bubbles: a circle round a short label.
    let mut bubbles = Vec::new();
    for t in &text {
        let label = t.text.trim();
        if label.is_empty() || label.len() > 3 || !label.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let mid = t.centre();
        if let Some((c, _)) = circles
            .iter()
            .filter(|(c, r)| c.dist(mid) < r * 0.6 && *r > 0.05 * dpi && *r < 0.5 * dpi)
            .min_by(|a, b| a.0.dist(mid).total_cmp(&b.0.dist(mid)))
        {
            bubbles.push((label.to_string(), *c));
        }
    }
    let map = fit_grid_robust(&bubbles, grid).ok_or("no structural grid found on the scan to align it by")?;
    // 5. Layers.
    let mut out = ScanRead { skew_deg: skew, map: map.clone(), ..ScanRead::default() };
    let to_model = |p: Vec2| -> Vec3 {
        let m = layout::to_model(sh, map.apply(p));
        Vec3::new(m[0], m[1], 0.0)
    };
    let model_per_px = map.len(1.0) * 12.0;
    let mut d = Drawing::new_imperial();
    let add = |d: &mut Drawing, layer: &str, kind: EntityKind| {
        if !d.layers.iter().any(|l| l.name == layer) {
            d.layers.push(cadaddy_doc::Layer::new(layer));
        }
        let _ = d.add(&Space::Model, Common { layer: layer.into(), ..Common::default() }, kind);
    };
    // The plan area: inside the border, above the title strip (long rules across the sheet).
    let long_h = rules(&segs, true, 0.75 * w as f64, dpi);
    let long_v = rules(&segs, false, 0.75 * h as f64, dpi);
    let top = long_h.last().copied().unwrap_or(h as f64);
    let strip = if long_h.len() >= 3 { long_h.get(1).copied().unwrap_or(0.0) } else { long_h.first().copied().unwrap_or(0.0) };
    let (left, right) = (long_v.first().copied().unwrap_or(0.0), long_v.last().copied().unwrap_or(w as f64));
    let margin = 0.03 * dpi;
    let in_plan = |p: Vec2| p.y > strip + margin && p.y < top - margin && p.x > left + margin && p.x < right - margin;
    let disc = sh.discipline.as_str();
    let mech = disc == "M";
    // Lineweights: leaders and symbols are lettered with the text pen; runs are heavier.
    let run_width = {
        let mut ws: Vec<f64> = segs.iter().filter(|s| in_plan(s.a) && in_plan(s.b) && s.len() > 0.5 * dpi).map(|s| s.width).collect();
        ws.sort_by(f64::total_cmp);
        ws.get(ws.len() * 3 / 4).copied().unwrap_or(text_stroke + 1.0)
    };
    let thin_max = (text_stroke + run_width) / 2.0;
    // Symbols on mechanical sheets: small boxes (diffusers, grilles) and boxes on runs (VAV).
    let mut taken = vec![false; segs.len()];
    if mech {
        for (min, max, kind, members) in boxes(&segs, dpi, &in_plan) {
            for m in &members {
                if let Some(t) = taken.get_mut(*m) {
                    *t = true;
                }
            }
            let vs: Vec<PolyVertex> = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
                .iter()
                .map(|p| {
                    let q = to_model(*p);
                    PolyVertex { p: Vec2::new(q.x, q.y), ..Default::default() }
                })
                .collect();
            add(&mut d, kind, EntityKind::LwPolyline(LwPolyline { vertices: vs, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
            for m in &members {
                if let Some(s) = segs.get(*m) {
                    let inside = |p: Vec2| p.x > min.x + 1.5 && p.x < max.x - 1.5 && p.y > min.y + 1.5 && p.y < max.y - 1.5;
                    if inside((s.a + s.b) * 0.5) {
                        add(&mut d, kind, EntityKind::Line(Line { a: to_model(s.a), b: to_model(s.b) }));
                    }
                }
            }
        }
    }
    let reach = 0.08 * dpi;
    let near_text = |p: Vec2| {
        text.iter().any(|t| {
            let (sn, c) = t.angle_deg.to_radians().sin_cos();
            let q = p - t.centre();
            let (along, up) = (q.x * c + q.y * sn, -q.x * sn + q.y * c);
            along.abs() < t.length() / 2.0 + reach && up.abs() < t.height / 2.0 + reach
        })
    };
    for (i, s) in segs.iter().enumerate() {
        if taken.get(i).copied().unwrap_or(false) {
            continue;
        }
        let layer = if !in_plan(s.a) || !in_plan(s.b) {
            "BORDER"
        } else if s.pieces >= 3 && s.dash < 0.14 * dpi && s.pieces as f64 * s.dash < 0.85 * s.len() {
            // Mechanical sheets dash what is hidden; plumbing tells hidden (short dashes) from
            // vent (long dashes).
            if mech || s.dash < 0.11 * dpi { "HIDDEN" } else { "DASHED" }
        } else if s.width <= thin_max && s.len() < 1.2 * dpi && (near_text(s.a) || near_text(s.b)) {
            // A leader: a light line from a tag.
            "TEXT"
        } else if mech {
            "DUCT"
        } else if s.width > run_width + 0.4 * (run_width - text_stroke).max(1.0) {
            "HEAVY"
        } else {
            "PIPE"
        };
        *out.layer_counts.entry(layer.to_string()).or_default() += 1;
        add(&mut d, layer, EntityKind::Line(Line { a: to_model(s.a), b: to_model(s.b) }));
    }
    // The background, screened grey: duct centrelines (dashed), the grid and the architecture.
    for g in grey_lines(s, dpi)? {
        let g = Seg { a: unskew(g.a), b: unskew(g.b), ..g };
        if !in_plan(g.a) || !in_plan(g.b) {
            continue;
        }
        let (fa, fb) = (map.apply(g.a), map.apply(g.b));
        let on_grid = |v: f64, lines: &[crate::truth::GridLine]| lines.iter().any(|l| (l.at_ft - v).abs() < 0.5);
        let grid_line = ((fa[0] - fb[0]).abs() < 0.3 && on_grid((fa[0] + fb[0]) / 2.0, &grid.x))
            || ((fa[1] - fb[1]).abs() < 0.3 && on_grid((fa[1] + fb[1]) / 2.0, &grid.y));
        let dashed = g.pieces >= 3 && g.pieces as f64 * g.dash < 0.92 * g.len();
        let layer = if grid_line {
            "GRID"
        } else if dashed && mech {
            "CENTRE"
        } else {
            "ARCH"
        };
        *out.layer_counts.entry(layer.to_string()).or_default() += 1;
        add(&mut d, layer, EntityKind::Line(Line { a: to_model(g.a), b: to_model(g.b) }));
    }
    for (c, r, a0, a1) in &arcs {
        if !in_plan(*c) {
            continue;
        }
        let layer = if mech { "DUCT" } else { "PIPE" };
        // Pixels (y up) to model: the angles turn with the map.
        let turn = map.turn_deg.to_radians() + (sh.model_from_building.rotation_deg).to_radians();
        add(&mut d, layer, EntityKind::Arc(Arc { center: to_model(*c), radius: r * model_per_px, start: a0 + turn, end: a1 + turn }));
    }
    let bubble_at: Vec<Vec2> = bubbles.iter().map(|(_, c)| *c).collect();
    for (c, r) in &circles {
        let layer = if bubble_at.iter().any(|b| b.dist(*c) < 1.0) {
            "GRID-BUBBLE"
        } else if !in_plan(*c) {
            "BORDER"
        } else if mech {
            "DUCT"
        } else {
            "PIPE"
        };
        add(&mut d, layer, EntityKind::Circle(Circle { center: to_model(*c), radius: r * model_per_px }));
    }
    // Text: tags and notes in the plan, the legend and title block below it.
    let rot = map.turn_deg + sh.model_from_building.rotation_deg;
    for t in &text {
        let mid = t.centre();
        let label = t.text.trim();
        let layer = if bubble_at.iter().any(|b| b.dist(mid) < 0.3 * dpi) {
            "GRID-BUBBLE"
        } else if in_plan(mid) {
            "TEXT"
        } else if mid.x > left + 0.62 * (right - left) {
            "TITLE"
        } else if label.contains('=') {
            "TEXT"
        } else {
            "LEGEND"
        };
        add(
            &mut d,
            layer,
            EntityKind::Text(Text {
                insert: to_model(t.origin),
                align_pt: Some(to_model(mid)),
                height: t.height * model_per_px,
                value: label.to_string(),
                rotation: (t.angle_deg + rot).to_radians(),
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: HAlign::Center,
                valign: VAlign::Middle,
            }),
        );
    }
    // Title-block identification: read the sheet number and discipline from the title strip.
    let (tb_number, tb_discipline) = identify_title_block(&text, strip, top, left, right, dpi);
    let sheet_label = tb_number.as_deref().unwrap_or(&sh.number);
    // Populate source/status/confidence for every inferred entity.
    let base_conf = 0.4 + 0.1 * (1.0 - map.rms_ft.min(1.0)); // 0.4–0.5 based on alignment quality
    for (layer, &count) in &out.layer_counts {
        let (status, conf) = match layer.as_str() {
            "TEXT" | "TITLE" | "LEGEND" => ("ocr", base_conf + 0.1),
            "GRID" | "GRID-BUBBLE" => ("grid", base_conf + 0.2),
            _ => ("rule", base_conf),
        };
        for _ in 0..count {
            out.sources.push(ScanSource { sheet: sheet_label.to_string(), status: status.to_string(), confidence: conf.min(1.0) });
        }
    }
    out.title_block_number = tb_number;
    out.title_block_discipline = tb_discipline;
    out.drawing = d;
    out.text = text;
    out.bubbles = bubbles;
    Ok(out)
}

/// Identify the sheet number and discipline from the title-block text in the title strip.
///
/// The title strip is between `strip_y` and the bottom of the sheet; the sheet number is
/// typically the largest text in the lower-right area, matching patterns like `M-101`, `P-201`,
/// `FP-101`, `E-301`. The discipline prefix (`M`, `P`, `FP`, `E`, `A`, `S`, `G`) is extracted
/// from the number.
fn identify_title_block(text: &[TextLine], strip_y: f64, _top_y: f64, left: f64, right: f64, dpi: f64) -> (Option<String>, Option<String>) {
    // Title-strip text: below the plan area, in the right portion (where the sheet number is).
    let title_right = left + 0.62 * (right - left);
    let mut candidates: Vec<(&str, f64)> = Vec::new();
    for t in text {
        let mid = t.centre();
        if mid.y > strip_y || mid.x < title_right {
            continue;
        }
        let label = t.text.trim();
        if label.is_empty() {
            continue;
        }
        candidates.push((label, t.height));
    }
    // A sheet number matches: 1-3 letter discipline prefix, a dash, and a digit sequence.
    // E.g. M-101, P-201, FP-101, E-301, A-101, S-100, G-001, FA-101, T-101.
    let sheet_re = |s: &str| -> bool {
        let Some((prefix, suffix)) = s.split_once('-') else { return false };
        !prefix.is_empty()
            && prefix.len() <= 3
            && prefix.chars().all(|c| c.is_ascii_uppercase())
            && !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_digit())
    };
    // Among matches, prefer the largest text (most prominent in the title block).
    let min_height = 0.08 * dpi; // ~0.08" = typical sheet number text
    let best = candidates.iter().filter(|(label, h)| sheet_re(label) && *h >= min_height).max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some(&(number, _)) = best {
        let disc = number.split('-').next().unwrap_or_default();
        (Some(number.to_string()), Some(disc.to_string()))
    } else {
        (None, None)
    }
}

/// Run the downstream MEP pipeline readers on a scan's drawing: heights, equipment/schedules,
/// and clashes. Each result is scored against the truth with a threshold relative to the vector
/// reader, or deferred with a reason.
///
/// Returns a summary of what ran and what each found.
pub struct ScanPipelineResult {
    /// Heights resolved from the scan's runs (empty if no runs have height notes).
    pub heights: Vec<crate::heights::ResolvedHeight>,
    /// Equipment read from the scan's schedules and plans.
    pub equipment: crate::equip_reader::EquipmentRead,
    /// Clashes among the scan's resolved 3D elements.
    pub clashes: Vec<crate::truth::Clash>,
    /// Items that could not be run, with the reason.
    pub deferred: Vec<(String, String)>,
}

/// Run heights, equipment and clashes on a scan's drawing, using the truth for context.
///
/// This wires the scan reader's output into the same pipeline the vector sheets use:
/// - Heights: the runs from ducts/pipes on the scan are fed to `resolve_heights`.
/// - Equipment: the scan drawing is passed to `read_equipment` as a `SheetIn`.
/// - Clashes: deferred — requires full 3D placement which needs duct3d (Q258).
pub fn run_pipeline_on_scan(scan: &ScanRead, sh: &Sheet, truth: &crate::truth::Truth) -> ScanPipelineResult {
    let mut result = ScanPipelineResult {
        heights: Vec::new(),
        equipment: crate::equip_reader::EquipmentRead::default(),
        clashes: Vec::new(),
        deferred: Vec::new(),
    };

    let level_code = sh.level.as_deref().unwrap_or_default();

    // --- Heights (Q257) ---
    // Build SegmentInputs from the duct and pipe runs found on the scan.
    let duct_net = crate::duct_reader::read_ducts_with(&scan.drawing, sh, &DUCT_OPTIONS);
    let duct_runs = duct_net.to_found_runs(sh);
    let (pipe_runs, _risers) = crate::pipe_reader::read_pipes(&scan.drawing, sh);

    let mut seg_inputs: Vec<crate::heights::SegmentInput> = Vec::new();
    for (i, r) in duct_runs.iter().chain(pipe_runs.iter()).enumerate() {
        let mid = [(r.from_ft[0] + r.to_ft[0]) / 2.0, (r.from_ft[1] + r.to_ft[1]) / 2.0];
        seg_inputs.push(crate::heights::SegmentInput {
            id: format!("scan-{i}"),
            system: r.system.clone(),
            kind: r.kind.clone(),
            shape: r.shape.clone(),
            size_in: r.size_in,
            level: level_code.to_string(),
            midpoint_ft: mid,
            from_ft: r.from_ft,
            to_ft: r.to_ft,
            height_note: r.bottom_ft.map(|h| format!("BOD @ {h}")),
            vertical: false,
        });
    }

    if let Some(level) = truth.level(level_code) {
        let rooms: Vec<crate::truth::Room> = truth.building.rooms.iter().filter(|rm| rm.level == level_code).cloned().collect();
        let beams: Vec<crate::truth::Beam> = truth.building.beams.iter().filter(|b| b.level == level_code).cloned().collect();
        // Floor-to-floor: next level's elevation minus this one's, or a default of 13 ft.
        let ftf = truth
            .building
            .levels
            .iter()
            .filter(|l| l.elevation_ft > level.elevation_ft)
            .map(|l| l.elevation_ft - level.elevation_ft)
            .fold(f64::INFINITY, f64::min);
        let ftf = if ftf.is_finite() { ftf } else { 13.0 };
        let ctx = crate::heights::LevelContext {
            level_code: level_code.to_string(),
            elevation_ft: level.elevation_ft,
            structure_underside_ft: beams.iter().map(|b| b.tos_ft - level.elevation_ft).fold(f64::INFINITY, f64::min).min(ftf),
            rooms,
            beams,
        };
        let rules = crate::heights::TradeRules::standard();
        result.heights = crate::heights::resolve_heights(&seg_inputs, &ctx, &rules);
    } else {
        result.deferred.push(("heights".into(), format!("level '{level_code}' not in truth; cannot resolve heights without level context")));
    }

    // --- Equipment/schedules (Q262) ---
    let sheet_in = crate::equip_reader::SheetIn { sheet: sh, drawing: &scan.drawing };
    let levels: Vec<crate::truth::Level> = truth.building.levels.clone();
    result.equipment = crate::equip_reader::read_equipment(&[sheet_in], &levels);

    // --- Clashes (Q261) ---
    // Deferred: clashes require full 3D resolved segments in the Truth structure, which needs
    // duct3d (Q258) to place the scan's runs in 3D. The clash detector works on Truth.segments,
    // not on FoundRuns. Wiring this requires duct3d to accept scan-derived runs and produce
    // Truth segments, which is a follow-up task.
    result.deferred.push((
        "clashes".into(),
        "clash detection requires 3D-placed segments (Truth.segments) from duct3d (Q258); \
         scan runs are 2D FoundRuns without elevation. Follow-up: wire duct3d to accept scan \
         heights and produce Truth segments, then run clash::clashes on the result."
            .into(),
    ));

    result
}

/// Whether a ring of ink (a component not part of the text) encloses a text line's middle: about
/// as wide as tall, larger than the letters and at most half an inch across.
fn enclosed(comps: &[Comp], t: &TextLine, ih: usize, dpi: f64) -> bool {
    let mid = t.centre();
    let row = ih as f64 - mid.y;
    comps.iter().enumerate().any(|(i, c)| {
        let (w, h) = (c.width() as f64, c.height() as f64);
        !t.comps.contains(&i)
            && w > t.height * 1.3
            && w < 0.6 * dpi
            && (w / h - 1.0).abs() < 0.25
            && mid.x > c.x0 as f64
            && mid.x < (c.x1 + 1) as f64
            && row > c.y0 as f64
            && row < (c.y1 + 1) as f64
    })
}

/// Tags written end to end along a run read as one line (`12" DIA 14" DIA`): split where a
/// complete tag (a size, a height) is followed by the start of another.
fn split_tags(t: &TextLine) -> Vec<TextLine> {
    let chars: Vec<char> = t.text.chars().collect();
    // Word boundaries: (start, end) char indices.
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars.get(i) == Some(&' ') {
            i += 1;
            continue;
        }
        let st = i;
        while i < chars.len() && chars.get(i) != Some(&' ') {
            i += 1;
        }
        words.push((st, i));
    }
    let word = |w: (usize, usize)| -> String { chars.get(w.0..w.1).map(|c| c.iter().collect()).unwrap_or_default() };
    let size = |s: &str| {
        let s = s.trim_end_matches('"');
        s.split_once('x')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()))
    };
    let mut cuts = Vec::new();
    let mut piece: Vec<String> = Vec::new();
    for (k, &w) in words.iter().enumerate() {
        let ws = word(w);
        if !piece.is_empty() {
            let last = piece.last().cloned().unwrap_or_default();
            let complete = last == "DIA" || last.ends_with('\u{d8}') || size(&last) || (last.ends_with('"') && piece.iter().any(|p| p == "@"));
            let starts = ws.chars().next().is_some_and(|c| c.is_ascii_digit()) || ["TOD", "BOD", "CL", "BOP", "TOP", "BOT"].contains(&ws.as_str());
            if complete && starts {
                cuts.push(k);
                piece.clear();
            }
        }
        piece.push(ws);
    }
    if cuts.is_empty() {
        return vec![t.clone()];
    }
    let mut out = Vec::new();
    let mut from = 0;
    for &k in &cuts {
        let to = words.get(k).map_or(chars.len(), |w| w.0);
        out.extend(t.part(from, to));
        from = to;
    }
    out.extend(t.part(from, chars.len()));
    out
}

/// Boxes among the plan's short axis-aligned lines: (min, max, layer, the lines making it up and
/// lying inside it). Small square boxes with something inside are terminals (`DEVICE`), other
/// small boxes equipment (`EQUIP`).
fn boxes(segs: &[Seg], dpi: f64, in_plan: &dyn Fn(Vec2) -> bool) -> Vec<(Vec2, Vec2, &'static str, Vec<usize>)> {
    let (lo, hi) = (0.06 * dpi, 0.6 * dpi);
    let tol = 0.025 * dpi;
    let hs: Vec<usize> = (0..segs.len())
        .filter(|&i| segs.get(i).is_some_and(|s| s.dir().x.abs() > 0.998 && s.len() >= lo * 0.7 && in_plan(s.a) && in_plan(s.b)))
        .collect();
    let vs: Vec<usize> = (0..segs.len())
        .filter(|&i| segs.get(i).is_some_and(|s| s.dir().y.abs() > 0.998 && s.len() >= lo * 0.7 && in_plan(s.a) && in_plan(s.b)))
        .collect();
    let span = |s: &Seg, horiz: bool| {
        if horiz { (s.a.x.min(s.b.x), s.a.x.max(s.b.x), (s.a.y + s.b.y) / 2.0) } else { (s.a.y.min(s.b.y), s.a.y.max(s.b.y), (s.a.x + s.b.x) / 2.0) }
    };
    // Index vertical lines by x for lookups.
    let mut vx: Vec<(f64, usize)> = vs.iter().filter_map(|&i| segs.get(i).map(|s| (span(s, false).2, i))).collect();
    vx.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut hy: Vec<(f64, usize)> = hs.iter().filter_map(|&i| segs.get(i).map(|s| (span(s, true).2, i))).collect();
    hy.sort_by(|a, b| a.0.total_cmp(&b.0));
    let near = |list: &[(f64, usize)], v: f64| -> Vec<usize> {
        let start = list.partition_point(|(x, _)| *x < v - tol);
        list.iter().skip(start).take_while(|(x, _)| *x <= v + tol).map(|(_, i)| *i).collect()
    };
    let covers = |s: &Seg, horiz: bool, a: f64, b: f64| {
        let (s0, s1, _) = span(s, horiz);
        let ov = s1.min(b) - s0.max(a);
        ov >= 0.8 * (b - a)
    };
    let mut out: Vec<(Vec2, Vec2, &'static str, Vec<usize>)> = Vec::new();
    for &(y0, i) in &hy {
        let Some(bot) = segs.get(i) else { continue };
        let (x0, x1, _) = span(bot, true);
        // Corners: vertical lines at both ends of this bottom line, going up.
        for &l in &near(&vx, x0) {
            let Some(ls) = segs.get(l) else { continue };
            let (ly0, ly1, _) = span(ls, false);
            if (ly0 - y0).abs() > tol && !(ly0 < y0 && ly1 > y0) {
                continue;
            }
            for &r in &near(&vx, x1) {
                if r == l {
                    continue;
                }
                let Some(rs) = segs.get(r) else { continue };
                let (ry0, ry1, _) = span(rs, false);
                let top = ly1.min(ry1);
                let side = top - y0;
                let width = x1 - x0;
                if side < lo || side > hi || width < lo || width > hi || (ry0 - y0).abs() > tol && !(ry0 < y0 && ry1 > y0) {
                    continue;
                }
                // A top line across.
                let Some(&t) = near(&hy, top).iter().find(|&&t| t != i && segs.get(t).is_some_and(|s| covers(s, true, x0, x1))) else { continue };
                if !(covers(ls, false, y0, top) && covers(rs, false, y0, top)) {
                    continue;
                }
                let (min, max) = (Vec2::new(x0, y0), Vec2::new(x1, top));
                if out.iter().any(|(a, b, _, _)| a.dist(min) < tol * 2.0 && b.dist(max) < tol * 2.0) {
                    continue;
                }
                // Lines inside it.
                let inside: Vec<usize> = (0..segs.len())
                    .filter(|&k| {
                        segs.get(k).is_some_and(|s| {
                            let m = (s.a + s.b) * 0.5;
                            m.x > x0 + tol && m.x < x1 - tol && m.y > y0 + tol && m.y < top - tol && s.len() < 1.6 * width.max(side)
                        })
                    })
                    .collect();
                let square = (side / width - 1.0).abs() < 0.2;
                let kind = if square && !inside.is_empty() && width < 0.35 * dpi { "DEVICE" } else { "EQUIP" };
                let mut members = vec![i, t];
                // The sides belong to the box unless they run on past it (a duct wall).
                for (s, k) in [(ls, l), (rs, r)] {
                    let (a, b, _) = span(s, false);
                    if b - a < 1.3 * side {
                        members.push(k);
                    }
                }
                members.retain(|&m| segs.get(m).is_some_and(|s| s.len() < 1.3 * width.max(side)));
                members.extend(inside);
                out.push((min, max, kind, members));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::truth::GridLine;

    #[test]
    fn grid_fit_recovers_scale_turn_and_offset() {
        let grid = Grid {
            x: vec![GridLine { label: "A".into(), at_ft: 0.0 }, GridLine { label: "B".into(), at_ft: 30.0 }],
            y: vec![GridLine { label: "1".into(), at_ft: 0.0 }, GridLine { label: "2".into(), at_ft: 25.0 }],
        };
        // 2 px per foot, the plan turned a quarter on the sheet, offset (100, 50) px.
        let m = PxToBuilding { turn_deg: 90.0, scale: 0.5, offset: [-25.0, 50.0], rms_ft: 0.0 };
        // Invert: pixel of a building point.
        let px = |b: [f64; 2]| {
            let q = [(b[0] - m.offset[0]) / m.scale, (b[1] - m.offset[1]) / m.scale];
            Vec2::new(q[1], -q[0])
        };
        let bubbles = vec![
            ("A".to_string(), px([0.0, -5.0])),
            ("A".to_string(), px([0.0, 30.0])),
            ("B".to_string(), px([30.0, -5.0])),
            ("1".to_string(), px([-5.0, 0.0])),
            ("2".to_string(), px([-5.0, 25.0])),
            ("7".to_string(), px([12.0, 12.0])),
        ];
        let f = fit_grid_robust(&bubbles, &grid).unwrap();
        assert_eq!(f.turn_deg, 90.0);
        assert!((f.scale - 0.5).abs() < 1e-9 && (f.offset[0] + 25.0).abs() < 1e-6 && (f.offset[1] - 50.0).abs() < 1e-6, "{f:?}");
    }

    #[test]
    fn dashes_join_into_one_dashed_line() {
        let segs: Vec<Seg> = (0..6)
            .map(|i| {
                let x = f64::from(i) * 15.0;
                Seg { a: Vec2::new(x, 10.0), b: Vec2::new(x + 10.0, 10.2), width: 2.0, pieces: 1, dash: 10.0 }
            })
            .collect();
        let j = join(segs, 8.0);
        assert_eq!(j.len(), 1, "{j:?}");
        assert_eq!(j[0].pieces, 6);
        assert!((j[0].len() - 85.0).abs() < 0.5);
    }

    #[test]
    fn skew_is_found() {
        let mut s = Scan { w: 400, h: 300, gray: vec![255; 400 * 300], dpi: 100.0 };
        for y in [50usize, 120, 200, 260] {
            for x in 20..380 {
                s.gray[y * 400 + x] = 0;
            }
        }
        let t = scan::turn(&s, 1.2, [0.0, 0.0]);
        assert!((skew_deg(&t) - 1.2).abs() < 0.06, "{}", skew_deg(&t));
        assert!(skew_deg(&s).abs() < 0.02);
    }

    #[test]
    fn hostile_scans_are_refused_not_panicked_on() {
        let sh = Sheet::default();
        let g = Grid::default();
        assert!(read_scan(&Scan { w: 0, h: 0, gray: vec![], dpi: 200.0 }, &sh, &g).is_err());
        assert!(read_scan(&Scan { w: 10, h: 10, gray: vec![0; 5], dpi: 200.0 }, &sh, &g).is_err());
        assert!(read_scan(&Scan { w: 10, h: 10, gray: vec![255; 100], dpi: f64::NAN }, &sh, &g).is_err());
        // A blank page has no grid to align by.
        assert!(read_scan(&Scan { w: 50, h: 40, gray: vec![255; 2000], dpi: 100.0 }, &sh, &g).is_err());
    }
}
