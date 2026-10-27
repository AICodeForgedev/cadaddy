//! Raster plans to vectors: a scanned or rendered plan image traced into lines, arcs, circles,
//! wall pairs (thick bands such as solid or hatched walls, as their two faces) and text boxes.
//!
//! Published methods only:
//! - Threshold: N. Otsu, "A Threshold Selection Method from Gray-Level Histograms", IEEE Trans.
//!   Systems, Man, and Cybernetics 9(1), 1979.
//! - Stroke width: a 3-4 chamfer distance transform, G. Borgefors, "Distance transformations in
//!   digital images", Computer Vision, Graphics, and Image Processing 34(3), 1986.
//! - Centre lines: T. Y. Zhang and C. Y. Suen, "A fast parallel algorithm for thinning digital
//!   patterns", Communications of the ACM 27(3), 1984.
//! - Polylines: U. Ramer, "An iterative procedure for the polygonal approximation of plane
//!   curves", Computer Graphics and Image Processing 1(3), 1972; D. Douglas and T. Peucker,
//!   Cartographica 10(2), 1973.
//! - Circles: I. Kåsa, "A circle fitting procedure and its error analysis", IEEE Trans.
//!   Instrumentation and Measurement 25(1), 1976 (algebraic least squares).
//!
//! Coordinates of the result are image units with y up: the origin at the image's lower-left
//! corner, one unit per pixel, pixel centres at half units.

/// Most pixels traced (a 8000 × 6000 scan).
pub const MAX_PIXELS: usize = 48_000_000;
/// Most chains and shapes produced.
const MAX_SHAPES: usize = 200_000;
/// Most thinning passes (each removes a layer of border pixels).
const MAX_THIN_PASSES: usize = 2_000;
/// Most stroke checks made while pairing hatched walls (bounds a dense sheet).
const MAX_PAIR_WORK: usize = 50_000_000;

/// How to trace.
#[derive(Clone, Debug)]
pub struct Options {
    /// Ink is at most this grey (0..255); `None`: chosen by Otsu's method.
    pub threshold: Option<u8>,
    /// Largest distance from a traced shape to the stroke's centre, pixels.
    pub tolerance: f64,
    /// Lines within this many degrees of the drawing's two main directions are made exactly so:
    /// horizontal and vertical, or the directions a slightly turned scan's walls run in.
    pub ortho_deg: f64,
    /// Thick strokes become wall pairs.
    pub walls: bool,
    /// Strokes at least this wide (pixels) are walls; `None`: three times the typical stroke.
    pub wall_min_px: Option<f64>,
    /// Two parallel thin lines a wall's thickness apart with nothing between them pair as a wall
    /// too (a hatched strip always does, when `walls`).
    pub wall_pairs: bool,
    /// Small blobs in rows become text boxes.
    pub text: bool,
    /// Text is at most this tall (pixels); `None`: 2.5% of the image's shorter side, or up to 6%
    /// for letters with another letter beside them on their row.
    pub text_max_px: Option<f64>,
    /// Shorter chains are dropped (specks), pixels.
    pub min_length: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threshold: None,
            tolerance: 1.5,
            ortho_deg: 3.0,
            walls: true,
            wall_min_px: None,
            wall_pairs: false,
            text: true,
            text_max_px: None,
            min_length: 4.0,
        }
    }
}

/// A traced shape (image units, y up).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Line {
        a: [f64; 2],
        b: [f64; 2],
        width: f64,
    },
    /// Counter-clockwise from `start` to `end` (radians).
    Arc {
        center: [f64; 2],
        radius: f64,
        start: f64,
        end: f64,
        width: f64,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
        width: f64,
    },
    /// A thick band along `a`–`b` (its centre line), `thickness` wide: drawn as its two faces.
    Wall {
        a: [f64; 2],
        b: [f64; 2],
        thickness: f64,
    },
    TextBox {
        min: [f64; 2],
        max: [f64; 2],
    },
}

/// What tracing found, with the threshold used.
#[derive(Clone, Debug, Default)]
pub struct Traced {
    pub shapes: Vec<Shape>,
    pub threshold: u8,
    /// The typical stroke width, pixels.
    pub stroke: f64,
}

/// Otsu's threshold of a grey image: the level that best splits it into two classes.
pub fn otsu(gray: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for &g in gray {
        if let Some(h) = hist.get_mut(usize::from(g)) {
            *h += 1;
        }
    }
    let total = gray.len() as f64;
    if total == 0.0 {
        return 128;
    }
    let sum: f64 = hist.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum();
    let (mut w_b, mut sum_b, mut best, mut level) = (0.0, 0.0, -1.0, 128u8);
    for (t, &n) in hist.iter().enumerate() {
        w_b += n as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += t as f64 * n as f64;
        let (m_b, m_f) = (sum_b / w_b, (sum - sum_b) / w_f);
        let between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if between > best {
            best = between;
            level = u8::try_from(t).unwrap_or(128);
        }
    }
    level
}

/// A binary image, row by row, y down.
struct Bits {
    w: usize,
    h: usize,
    on: Vec<bool>,
}

impl Bits {
    fn get(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return false;
        }
        self.on.get(y as usize * self.w + x as usize).copied().unwrap_or(false)
    }
}

/// The 8 neighbours P2..P9 of Zhang–Suen, clockwise from north.
const NB: [(i64, i64); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];

/// Zhang–Suen thinning in place.
fn thin(b: &mut Bits) {
    let mut del: Vec<usize> = Vec::new();
    for pass in 0..MAX_THIN_PASSES {
        let mut changed = false;
        for sub in 0..2 {
            del.clear();
            for y in 0..b.h {
                for x in 0..b.w {
                    if !b.get(x as i64, y as i64) {
                        continue;
                    }
                    let p: [bool; 8] = NB.map(|(dx, dy)| b.get(x as i64 + dx, y as i64 + dy));
                    let n = p.iter().filter(|v| **v).count();
                    if !(2..=6).contains(&n) {
                        continue;
                    }
                    let a = (0..8).filter(|&i| !p[i] && p[(i + 1) % 8]).count();
                    if a != 1 {
                        continue;
                    }
                    let (p2, p4, p6, p8) = (p[0], p[2], p[4], p[6]);
                    // Zhang–Suen: pass 1 needs not (p2·p4·p6) and not (p4·p6·p8); pass 2 not (p2·p4·p8)
                    // and not (p2·p6·p8) — written minimized.
                    let ok = if sub == 0 { !p4 || !p6 || (!p2 && !p8) } else { !p2 || !p8 || (!p4 && !p6) };
                    if ok {
                        del.push(y * b.w + x);
                    }
                }
            }
            for &i in &del {
                if let Some(v) = b.on.get_mut(i) {
                    *v = false;
                }
            }
            changed |= !del.is_empty();
        }
        if !changed || pass + 1 == MAX_THIN_PASSES {
            break;
        }
    }
}

/// A closed outline round a hole across most of it (a symbol's circle or box, a letter O): from
/// the middle of its box, every way out meets the outline only after a third of the way.
fn ring(comp: &[usize], w: usize) -> bool {
    if w == 0 || comp.is_empty() {
        return false;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for &i in comp {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let Some(cells) = bw.checked_mul(bh) else { return false };
    let mut ink = vec![false; cells];
    for &i in comp {
        if let Some(c) = ink.get_mut((i / w - y0) * bw + (i % w - x0)) {
            *c = true;
        }
    }
    let at = |x: i64, y: i64| {
        x >= 0 && y >= 0 && (x as usize) < bw && (y as usize) < bh && ink.get(y as usize * bw + x as usize).copied().unwrap_or(false)
    };
    let (cx, cy) = ((bw / 2) as i64, (bh / 2) as i64);
    [(1, 0, bw), (-1, 0, bw), (0, 1, bh), (0, -1, bh)].iter().all(|&(dx, dy, span)| {
        let half = (span / 2) as i64;
        // The first ink on the way out: past a third of the half-span, and before the edge.
        (0..=half + 1).find(|k| at(cx + dx * k, cy + dy * k)).is_some_and(|k| k * 3 >= half)
    })
}

/// The most common length of the ink runs across rows and columns: a fine line's width, pixels.
fn typical_run(b: &Bits) -> f64 {
    const LONGEST: usize = 64;
    let mut count = [0usize; LONGEST + 1];
    let mut tally = |run: usize| {
        if let Some(c) = count.get_mut(run.min(LONGEST)) {
            *c += 1;
        }
    };
    let on = |x: usize, y: usize| b.on.get(y * b.w + x).copied().unwrap_or(false);
    for y in 0..b.h {
        let mut run = 0;
        for x in 0..b.w {
            if on(x, y) {
                run += 1;
            } else if run > 0 {
                tally(run);
                run = 0;
            }
        }
        tally(run);
    }
    for x in 0..b.w {
        let mut run = 0;
        for y in 0..b.h {
            if on(x, y) {
                run += 1;
            } else if run > 0 {
                tally(run);
                run = 0;
            }
        }
        tally(run);
    }
    // Runs along a stroke are long and spread out; across one they pile up at its width.
    count.iter().enumerate().skip(1).take(LONGEST - 1).max_by_key(|(_, c)| **c).map_or(1.0, |(i, _)| i as f64)
}

/// 3-4 chamfer distance to the nearest background pixel, in pixels (÷3).
fn distance(b: &Bits) -> Vec<f32> {
    let (w, h) = (b.w, b.h);
    let big = u32::MAX / 4;
    let mut d: Vec<u32> = b.on.iter().map(|&v| if v { big } else { 0 }).collect();
    let at = |d: &Vec<u32>, x: i64, y: i64| -> u32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 { 0 } else { d.get(y as usize * w + x as usize).copied().unwrap_or(0) }
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let i = y as usize * w + x as usize;
            if d.get(i).copied().unwrap_or(0) == 0 {
                continue;
            }
            let m = [at(&d, x - 1, y) + 3, at(&d, x, y - 1) + 3, at(&d, x - 1, y - 1) + 4, at(&d, x + 1, y - 1) + 4].into_iter().min().unwrap_or(big);
            if let Some(v) = d.get_mut(i) {
                *v = (*v).min(m);
            }
        }
    }
    for y in (0..h as i64).rev() {
        for x in (0..w as i64).rev() {
            let i = y as usize * w + x as usize;
            if d.get(i).copied().unwrap_or(0) == 0 {
                continue;
            }
            let m = [at(&d, x + 1, y) + 3, at(&d, x, y + 1) + 3, at(&d, x + 1, y + 1) + 4, at(&d, x - 1, y + 1) + 4].into_iter().min().unwrap_or(big);
            if let Some(v) = d.get_mut(i) {
                *v = (*v).min(m);
            }
        }
    }
    d.into_iter().map(|v| v as f32 / 3.0).collect()
}

/// 8-connected components: each one's pixel indices.
fn components(b: &Bits) -> Vec<Vec<usize>> {
    let mut seen = vec![false; b.on.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..b.on.len() {
        if !b.on.get(start).copied().unwrap_or(false) || seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut comp = Vec::new();
        stack.push(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = ((i % b.w) as i64, (i / b.w) as i64);
            for (dx, dy) in NB {
                let (nx, ny) = (x + dx, y + dy);
                if b.get(nx, ny) {
                    let j = ny as usize * b.w + nx as usize;
                    if let Some(s) = seen.get_mut(j)
                        && !*s
                    {
                        *s = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(comp);
        if out.len() >= MAX_SHAPES {
            break;
        }
    }
    out
}

/// Scanner specks out: ink blobs of at most `max` pixels removed, and holes of at most `max`
/// pixels in the ink filled (a speck inside a solid wall would make it a ring). Lines a pixel wide
/// are long, so they stay.
fn despeckle(b: &mut Bits, max: usize) {
    for comp in components(b) {
        if comp.len() <= max {
            for i in comp {
                if let Some(p) = b.on.get_mut(i) {
                    *p = false;
                }
            }
        }
    }
    let paper = Bits { w: b.w, h: b.h, on: b.on.iter().map(|on| !on).collect() };
    for comp in components(&paper) {
        if comp.len() <= max {
            for i in comp {
                if let Some(p) = b.on.get_mut(i) {
                    *p = true;
                }
            }
        }
    }
}

/// The drawing's main direction (radians, within ±10°): the length-weighted median of how far
/// the edges near horizontal or vertical (within 10°, folded to a quarter turn) are turned, so a
/// diagonal doesn't pull it. 0 for too few such edges, or under a quarter of a degree (pixel
/// rounding alone turns an edge that much, and axis-aligned plans stay exactly so).
fn main_direction(edges: &[([f64; 2], [f64; 2])], min_len: f64) -> f64 {
    let lim = 10f64.to_radians();
    let quarter = std::f64::consts::FRAC_PI_2;
    let mut turned: Vec<(f64, f64)> = edges
        .iter()
        .filter_map(|(a, b)| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx.hypot(dy);
            // Folded into (-45°, 45°]: how far from the nearest axis.
            let f = (dy.atan2(dx) + quarter / 2.0).rem_euclid(quarter) - quarter / 2.0;
            (len >= min_len && f.abs() <= lim).then_some((f, len))
        })
        .collect();
    let total: f64 = turned.iter().map(|t| t.1).sum();
    if turned.len() < 3 || total <= 0.0 {
        return 0.0;
    }
    turned.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut acc = 0.0;
    let median = turned.iter().find(|t| {
        acc += t.1;
        acc >= total / 2.0
    });
    let d = median.map_or(0.0, |t| t.0);
    if d.abs() < 0.25f64.to_radians() { 0.0 } else { d }
}

/// Pixel chains of a one-pixel-wide skeleton between its ends and junctions (and closed loops).
fn chains(b: &Bits) -> Vec<(Vec<(i64, i64)>, bool)> {
    let deg = |x: i64, y: i64| NB.iter().filter(|(dx, dy)| b.get(x + dx, y + dy)).count();
    let idx = |x: i64, y: i64| y as usize * b.w + x as usize;
    let mut used = vec![false; b.on.len()];
    let mut out = Vec::new();
    // The crossing number (0 → 1 steps around the pixel): 1 at an end, 2 along a curve (a
    // staircase pixel with three neighbours too), 3 or more where curves meet.
    let crossings = |x: i64, y: i64| {
        let p: [bool; 8] = NB.map(|(dx, dy)| b.get(x + dx, y + dy));
        (0..8).filter(|&i| !p[i] && p[(i + 1) % 8]).count()
    };
    // Thinning can leave a 2 × 2 block where strokes cross at a shallow angle; each of its pixels
    // looks like a curve's on its own, so the block is taken for the junction it is.
    let in_block = |x: i64, y: i64| {
        [(-1i64, -1i64), (0, -1), (-1, 0), (0, 0)].iter().any(|(ox, oy)| (0..2).all(|i| (0..2).all(|j| b.get(x + ox + i, y + oy + j))))
    };
    let is_node = |x: i64, y: i64| deg(x, y) <= 1 || crossings(x, y) != 2 || in_block(x, y);
    // From each node, walk every unused direction.
    let walk = |sx: i64, sy: i64, used: &mut Vec<bool>, out: &mut Vec<(Vec<(i64, i64)>, bool)>| {
        for (dx, dy) in NB {
            let (mut x, mut y) = (sx + dx, sy + dy);
            if !b.get(x, y) || used.get(idx(x, y)).copied().unwrap_or(true) && !is_node(x, y) {
                continue;
            }
            let mut chain = vec![(sx, sy)];
            let (mut px, mut py) = (sx, sy);
            loop {
                chain.push((x, y));
                if is_node(x, y) || chain.len() > b.on.len() {
                    break;
                }
                if let Some(u) = used.get_mut(idx(x, y)) {
                    *u = true;
                }
                // The next pixel: a neighbour that isn't where we came from (4-neighbours first).
                let next = [(0i64, -1i64), (1, 0), (0, 1), (-1, 0), (1, -1), (1, 1), (-1, 1), (-1, -1)]
                    .into_iter()
                    .map(|(ex, ey)| (x + ex, y + ey))
                    .find(|&(nx, ny)| {
                        b.get(nx, ny)
                            && (nx, ny) != (px, py)
                            && !chain.contains(&(nx, ny))
                            && (is_node(nx, ny) || !used.get(idx(nx, ny)).copied().unwrap_or(true))
                    });
                match next {
                    Some((nx, ny)) => {
                        (px, py) = (x, y);
                        (x, y) = (nx, ny);
                    }
                    None => break,
                }
            }
            // A chain between two adjacent nodes is walked from both: keep one.
            if chain.len() == 2 && (chain[1].1, chain[1].0) < (sy, sx) {
                continue;
            }
            out.push((chain, false));
            if out.len() >= MAX_SHAPES {
                return;
            }
        }
    };
    for y in 0..b.h as i64 {
        for x in 0..b.w as i64 {
            if b.get(x, y) && is_node(x, y) && deg(x, y) > 0 {
                walk(x, y, &mut used, &mut out);
            }
        }
    }
    // What is left is closed loops (every pixel has two neighbours).
    for y in 0..b.h as i64 {
        for x in 0..b.w as i64 {
            if !b.get(x, y) || used.get(idx(x, y)).copied().unwrap_or(true) || is_node(x, y) {
                continue;
            }
            let mut chain = vec![(x, y)];
            if let Some(u) = used.get_mut(idx(x, y)) {
                *u = true;
            }
            let (mut cx, mut cy) = (x, y);
            while let Some((nx, ny)) =
                NB.iter().map(|(dx, dy)| (cx + dx, cy + dy)).find(|&(nx, ny)| b.get(nx, ny) && !used.get(idx(nx, ny)).copied().unwrap_or(true))
            {
                if let Some(u) = used.get_mut(idx(nx, ny)) {
                    *u = true;
                }
                chain.push((nx, ny));
                (cx, cy) = (nx, ny);
            }
            if chain.len() >= 3 {
                out.push((chain, true));
            }
            if out.len() >= MAX_SHAPES {
                return out;
            }
        }
    }
    out
}

/// Ramer–Douglas–Peucker: the indices of the points kept.
fn rdp(p: &[[f64; 2]], eps: f64) -> Vec<usize> {
    if p.len() < 3 {
        return (0..p.len()).collect();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (p[a], p[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = dx.hypot(dy);
        let (mut best, mut at) = (0.0, a);
        for (i, q) in p.iter().enumerate().take(b).skip(a + 1) {
            let d = if len < 1e-12 { (q[0] - pa[0]).hypot(q[1] - pa[1]) } else { ((q[0] - pa[0]) * dy - (q[1] - pa[1]) * dx).abs() / len };
            if d > best {
                best = d;
                at = i;
            }
        }
        if best > eps {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect()
}

/// Kåsa's algebraic circle fit: centre, radius and the RMS distance of the points from it.
fn fit_circle(p: &[[f64; 2]]) -> Option<([f64; 2], f64, f64)> {
    if p.len() < 5 {
        return None;
    }
    let n = p.len() as f64;
    let (mx, my) = (p.iter().map(|q| q[0]).sum::<f64>() / n, p.iter().map(|q| q[1]).sum::<f64>() / n);
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for q in p {
        let (u, v) = (q[0] - mx, q[1] - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-9 {
        return None;
    }
    let (b1, b2) = (0.5 * (suuu + suvv), 0.5 * (svvv + svuu));
    let uc = (b1 * svv - b2 * suv) / det;
    let vc = (suu * b2 - suv * b1) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    let c = [uc + mx, vc + my];
    let rms = (p.iter().map(|q| ((q[0] - c[0]).hypot(q[1] - c[1]) - r).powi(2)).sum::<f64>() / n).sqrt();
    (r.is_finite() && rms.is_finite()).then_some((c, r, rms))
}

/// Arcs of one circle that meet end to start become one arc, or the circle when they close
/// (a thinned circle is cut where its stroke touches itself at the pixel staircase).
fn merge_arcs(shapes: &mut Vec<Shape>, tol: f64) {
    use std::f64::consts::TAU;
    let close = |a: f64, b: f64| ((a - b + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0).abs() < 20f64.to_radians();
    let mut changed = true;
    while changed {
        changed = false;
        let arcs: Vec<usize> = shapes.iter().enumerate().filter(|(_, s)| matches!(s, Shape::Arc { .. })).map(|(i, _)| i).collect();
        'pairs: for &i in &arcs {
            for &j in &arcs {
                if i == j {
                    continue;
                }
                let (
                    Some(Shape::Arc { center: c1, radius: r1, start: s1, end: e1, width: w1 }),
                    Some(Shape::Arc { center: c2, radius: r2, start: s2, end: e2, width: w2 }),
                ) = (shapes.get(i).cloned(), shapes.get(j).cloned())
                else {
                    continue;
                };
                let same = (c1[0] - c2[0]).hypot(c1[1] - c2[1]) <= 3.0 * tol + 0.05 * r1.max(r2) && (r1 - r2).abs() <= 2.0 * tol + 0.05 * r1.max(r2);
                if !same || !close(e1, s2) {
                    continue;
                }
                let sweep = |s: f64, e: f64| (e - s).rem_euclid(TAU);
                let (cx, cy, r, w) = ((c1[0] + c2[0]) / 2.0, (c1[1] + c2[1]) / 2.0, (r1 + r2) / 2.0, (w1 + w2) / 2.0);
                let total = sweep(s1, e1) + sweep(s2, e2);
                let merged = if total > TAU - 0.35 || close(e2, s1) && total > TAU * 0.75 {
                    Shape::Circle { center: [cx, cy], radius: r, width: w }
                } else {
                    Shape::Arc { center: [cx, cy], radius: r, start: s1, end: e2, width: w }
                };
                let (hi, lo) = if i > j { (i, j) } else { (j, i) };
                shapes.swap_remove(hi);
                shapes.swap_remove(lo);
                shapes.push(merged);
                changed = true;
                break 'pairs;
            }
        }
    }
}

/// A traced segment: its ends and stroke width (image units).
type Seg = ([f64; 2], [f64; 2], f64);

/// Thin pieces along one face line: the direction group `k`, the offset `c` along its normal
/// (length-weighted), their total length, and their extents along it.
struct FaceLine {
    k: usize,
    c: f64,
    len: f64,
    spans: Vec<(f64, f64)>,
}

/// Hatched walls as wall pairs. A wall drawn the usual way is two parallel face lines a wall's
/// thickness apart with diagonal hatch strokes between them; thinning leaves the faces broken at
/// every stroke. Collinear face pieces are gathered into face lines, parallel lines at a wall's spacing
/// (overlapping along at least half the shorter one) are paired when the strip between them is
/// crossed by diagonal strokes spread along it (or always, with `pairs`), and each pair becomes one
/// `Shape::Wall` on the strip's centre line, as thick as the gap. The faces, the hatch strokes and
/// the junction pieces inside the strip are taken out of `segs` (and arcs and circles lying
/// wholly inside it, bent junction pieces, out of `curves`).
fn hatched_walls(segs: &mut Vec<Seg>, curves: &mut Vec<Shape>, thin_max: f64, stroke: f64, tol: f64, min_len: f64, pairs: bool) -> Vec<Shape> {
    use std::f64::consts::PI;
    let ang_tol = 1.5f64.to_radians();
    let slack = tol + stroke;
    let dot = |p: [f64; 2], q: [f64; 2]| p[0] * q[0] + p[1] * q[1];
    // Thin segments and their directions, in [-ang_tol/2, PI - ang_tol/2) so near-horizontal
    // ones from either side fall together.
    let thin: Vec<(usize, f64, f64)> = segs
        .iter()
        .enumerate()
        .filter(|(_, s)| s.2 < thin_max)
        .map(|(i, s)| {
            let th = (s.1[1] - s.0[1]).atan2(s.1[0] - s.0[0]).rem_euclid(PI);
            (i, if th >= PI - ang_tol / 2.0 { th - PI } else { th }, (s.1[0] - s.0[0]).hypot(s.1[1] - s.0[1]))
        })
        .collect();
    // Direction groups of the face candidates (greedy from the smallest angle).
    let mut by_angle: Vec<&(usize, f64, f64)> = thin.iter().filter(|t| t.2 >= min_len).collect();
    by_angle.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut groups: Vec<Vec<(usize, f64, f64)>> = Vec::new();
    let mut first = f64::NEG_INFINITY;
    for t in by_angle {
        match groups.last_mut() {
            Some(g) if t.1 - first <= ang_tol => g.push(*t),
            _ => {
                first = t.1;
                groups.push(vec![*t]);
            }
        }
    }
    // Each group's direction (length-weighted). A group can hold a wall only when two of its
    // lines lie a wall's thickness apart.
    let (gap_lo, gap_hi) = ((3.0 * stroke).max(2.0 * slack + 1.0), 60.0 * stroke.max(1.0));
    let mut dirs: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut maybe: Vec<bool> = Vec::new();
    for g in &groups {
        let total: f64 = g.iter().map(|t| t.2).sum();
        let th = g.iter().map(|t| t.1 * t.2).sum::<f64>() / total.max(1e-9);
        let (d, n) = ([th.cos(), th.sin()], [-th.sin(), th.cos()]);
        dirs.push((d, n));
        let mut cs: Vec<f64> = g.iter().filter_map(|&(i, _, _)| segs.get(i).map(|s| (dot(n, s.0) + dot(n, s.1)) / 2.0)).collect();
        cs.sort_by(|a, b| a.total_cmp(b));
        let mut hi = 0;
        let mut any = false;
        for (lo, c) in cs.iter().enumerate() {
            hi = hi.max(lo);
            while cs.get(hi + 1).is_some_and(|x| x - c <= gap_hi) {
                hi += 1;
            }
            if cs.get(lo..=hi).is_some_and(|w| w.iter().any(|x| x - c >= gap_lo)) {
                any = true;
                break;
            }
        }
        maybe.push(any);
    }
    // A group within 8° of a longer one is that one's (the bent pieces by the hatch junctions),
    // not a direction of its own.
    let totals: Vec<f64> = groups.iter().map(|g| g.iter().map(|t| t.2).sum()).collect();
    let mut by_total: Vec<usize> = (0..groups.len()).collect();
    by_total.sort_by(|&a, &b| totals.get(b).unwrap_or(&0.0).total_cmp(totals.get(a).unwrap_or(&0.0)));
    let mut kept_dirs: Vec<f64> = Vec::new();
    for k in by_total {
        let Some(&(d, _)) = dirs.get(k) else { continue };
        let th = d[1].atan2(d[0]);
        let near = kept_dirs.iter().any(|&o| {
            let x = (th - o).rem_euclid(PI);
            x.min(PI - x) <= 8f64.to_radians()
        });
        if near {
            if let Some(m) = maybe.get_mut(k) {
                *m = false;
            }
        } else {
            kept_dirs.push(th);
        }
    }
    // Per such group, every thin segment in its frame: (u from, u to, t from, t to, angle to the
    // group's direction folded into 0..90°, segment), sorted by `u from`.
    let mut work = 0usize;
    let mut frames: Vec<Vec<(f64, f64, f64, f64, f64, usize)>> = vec![Vec::new(); dirs.len()];
    for ((&(d, n), frame), _) in dirs.iter().zip(frames.iter_mut()).zip(&maybe).filter(|(_, m)| **m) {
        work += thin.len();
        if work > MAX_PAIR_WORK {
            break;
        }
        let dir = d[1].atan2(d[0]);
        *frame = thin
            .iter()
            .filter_map(|&(i, th, _)| {
                let s = segs.get(i)?;
                let (ua, ub) = (dot(n, s.0), dot(n, s.1));
                let fold = (th - dir).rem_euclid(PI);
                Some((ua.min(ub), ua.max(ub), dot(d, s.0), dot(d, s.1), fold.min(PI - fold), i))
            })
            .collect();
        frame.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    // Face lines: the thin pieces within 8° of a group's direction and no wider than a stroke,
    // by their offset. Thinning bends a face toward each hatch stroke it meets (a few degrees, and
    // up to a few pixels into the strip on a narrow wall), so the pieces of one face wander: they
    // chain into a line where each is within the tolerance of the last, and short off-line pieces
    // join the nearest long line within two strokes' width.
    let join_gap = (3.0 * stroke).max(2.0 * tol) + 2.0;
    let mut lines: Vec<FaceLine> = Vec::new();
    for (k, frame) in frames.iter().enumerate() {
        // (offset, t0, t1, length)
        let mut on: Vec<(f64, f64, f64, f64)> = frame
            .iter()
            .filter(|q| q.4 <= 8f64.to_radians() && q.1 - q.0 <= slack)
            .map(|q| ((q.0 + q.1) / 2.0, q.2.min(q.3), q.2.max(q.3), (q.3 - q.2).abs().max(1e-6)))
            .collect();
        on.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut found: Vec<FaceLine> = Vec::new();
        let mut start = 0;
        while start < on.len() {
            let c0 = on.get(start).map_or(0.0, |q| q.0);
            let mut end = start + 1;
            while let (Some(prev), Some(q)) = (on.get(end - 1), on.get(end)) {
                if q.0 - prev.0 > tol || q.0 - c0 > slack {
                    break;
                }
                end += 1;
            }
            let part = on.get(start..end).unwrap_or_default();
            let len: f64 = part.iter().map(|q| q.3).sum();
            let c = part.iter().map(|q| q.0 * q.3).sum::<f64>() / len.max(1e-9);
            found.push(FaceLine { k, c, len, spans: part.iter().map(|q| (q.1, q.2)).collect() });
            start = end;
        }
        // Longest first, each line takes in the shorter ones within two strokes' width (no
        // wall is that narrow) and keeps its own offset.
        found.sort_by(|a, b| b.len.total_cmp(&a.len));
        let mut kept: Vec<FaceLine> = Vec::new();
        for m in found {
            let near = kept.iter_mut().filter(|f| (f.c - m.c).abs() <= 2.0 * slack).min_by(|a, b| (a.c - m.c).abs().total_cmp(&(b.c - m.c).abs()));
            match near {
                Some(f) => {
                    f.spans.extend(m.spans);
                    f.len += m.len;
                }
                None => kept.push(m),
            }
        }
        // Long enough to face a wall: an overlap of 1.5 of the narrowest wall.
        kept.retain(|f| f.len >= 1.5 * gap_lo);
        for f in kept.iter_mut() {
            f.spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        lines.extend(kept);
    }
    // Runs along a line: its pieces joined across breaks shorter than `join`.
    let runs_of = |f: &FaceLine, join: f64| -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for &(a, b) in &f.spans {
            match out.last_mut() {
                Some(r) if a <= r.1 + join => r.1 = r.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    };
    // Candidate pairs: (hatched, gap, line, line, run on the first, run on the second, overlap).
    // A face broken for less than the wall's thickness (at the hatch junctions) is one face.
    lines.sort_by(|a, b| a.k.cmp(&b.k).then(a.c.total_cmp(&b.c)));
    type Cand = (bool, f64, usize, usize, (f64, f64), (f64, f64), (f64, f64));
    let mut cands: Vec<Cand> = Vec::new();
    for (ia, la) in lines.iter().enumerate() {
        for (ib, lb) in lines.iter().enumerate().skip(ia + 1) {
            let gap = lb.c - la.c;
            if lb.k != la.k || gap > gap_hi {
                break;
            }
            if gap < gap_lo {
                continue;
            }
            let Some(frame) = frames.get(la.k) else { continue };
            // Hatch strokes along the strip: diagonal, inside it, across most of it; their
            // extents and middles along the direction.
            let mid = (la.c + lb.c) / 2.0;
            let (lo, hi) = (mid - gap / 2.0 - slack, mid + gap / 2.0 + slack);
            let from = frame.partition_point(|q| q.0 < lo);
            let mut strokes: Vec<(f64, f64, usize)> = Vec::new();
            for q in frame.iter().skip(from) {
                work += 1;
                if q.0 > hi || work > MAX_PAIR_WORK {
                    break;
                }
                if q.1 <= hi && (20f64.to_radians()..=70f64.to_radians()).contains(&q.4) && q.1 - q.0 >= 0.45 * gap {
                    strokes.push((q.2.min(q.3), q.2.max(q.3), q.5));
                }
            }
            if strokes.is_empty() && !pairs {
                continue;
            }
            strokes.sort_by(|a, b| a.0.total_cmp(&b.0));
            // A face broken for less than the wall's thickness, or where the hatching carries on
            // across the break (thinning pulls a face off its line by the strokes), is one face;
            // a door opening has neither face nor hatch.
            let join = join_gap.max(gap);
            let bridge = |runs: Vec<(f64, f64)>| -> Vec<(f64, f64)> {
                let mut out: Vec<(f64, f64)> = Vec::new();
                for r in runs {
                    let Some(last) = out.last_mut() else {
                        out.push(r);
                        continue;
                    };
                    let (x, y) = (last.1, r.0);
                    let (mut cur, mut hole) = (x, 0.0f64);
                    for &(a, b, _) in strokes.iter().filter(|s| s.1 > x && s.0 < y) {
                        hole = hole.max(a - cur);
                        cur = cur.max(b);
                    }
                    hole = hole.max(y - cur);
                    if hole <= join {
                        last.1 = last.1.max(r.1);
                    } else {
                        out.push(r);
                    }
                }
                out
            };
            let (runs_a, runs_b) = (bridge(runs_of(la, join)), bridge(runs_of(lb, join)));
            for &ra in &runs_a {
                for &rb in &runs_b {
                    work += 1;
                    let (ov0, ov1) = (ra.0.max(rb.0), ra.1.min(rb.1));
                    let ov = ov1 - ov0;
                    if ov < 0.5 * (ra.1 - ra.0).min(rb.1 - rb.0) || ov < 1.5 * gap {
                        continue;
                    }
                    let inside: Vec<&(f64, f64, usize)> = strokes.iter().filter(|s| s.0 >= ov0 - slack && s.1 <= ov1 + slack).collect();
                    let at: Vec<f64> = inside.iter().map(|s| (s.0 + s.1) / 2.0).collect();
                    // Hatching is a ladder: at least three of its strokes lie on parallel lines of
                    // their own. (Between two hatch strokes, the pieces of the faces they cut lie on
                    // two lines of each direction.)
                    let mut keys: Vec<(i64, f64)> = inside
                        .iter()
                        .filter_map(|q| {
                            let s = segs.get(q.2)?;
                            let mut phi = (s.1[1] - s.0[1]).atan2(s.1[0] - s.0[0]).rem_euclid(PI);
                            if phi >= PI - 2.5f64.to_radians() {
                                phi -= PI;
                            }
                            Some(((phi.to_degrees() / 5.0).round() as i64, dot([-phi.sin(), phi.cos()], s.0)))
                        })
                        .collect();
                    keys.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                    let mut ladder = 0usize;
                    let mut run = 0usize;
                    for (j, key) in keys.iter().enumerate() {
                        match j.checked_sub(1).and_then(|p| keys.get(p)) {
                            Some(prev) if prev.0 == key.0 => {
                                if key.1 - prev.1 > slack {
                                    run += 1;
                                }
                            }
                            _ => run = 1,
                        }
                        ladder = ladder.max(run);
                    }
                    // Spread along the strip: no stretch without a stroke longer than three wall
                    // thicknesses or a third of the strip (`at` is in order of the strokes' starts,
                    // the same as their middles for strokes of one slope).
                    let mut stops = vec![ov0];
                    stops.extend(at.iter().copied());
                    stops.push(ov1);
                    stops.sort_by(|a, b| a.total_cmp(b));
                    let widest = stops.windows(2).map(|p| p[1] - p[0]).fold(0.0, f64::max);
                    let hatched = ladder >= 3 && widest <= (3.0 * gap).max(ov / 3.0);
                    if hatched || pairs {
                        cands.push((hatched, gap, ia, ib, ra, rb, (ov0, ov1)));
                    }
                }
            }
        }
    }
    // Hatched pairs first, then the narrowest; a stretch of a face line is one wall's face only.
    cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
    let mut used: Vec<Vec<(f64, f64)>> = vec![Vec::new(); lines.len()];
    let free =
        |used: &[Vec<(f64, f64)>], r: usize, a: f64, b: f64| used.get(r).is_none_or(|u| u.iter().all(|&(x, y)| b <= x + slack || a >= y - slack));
    // Walls found: (direction group, centre offset, thickness, from, to).
    let mut found: Vec<(usize, f64, f64, f64, f64)> = Vec::new();
    for (_, gap, ia, ib, ra, rb, (ov0, ov1)) in cands {
        if !free(&used, ia, ov0, ov1) || !free(&used, ib, ov0, ov1) {
            continue;
        }
        let (Some(la), Some(lb)) = (lines.get(ia), lines.get(ib)) else { continue };
        let Some(_) = dirs.get(la.k) else { continue };
        // Ends: where the faces end together (or a wall's thickness apart, at a corner) the
        // centre line ends half-way; else where they stop overlapping.
        let s0 = if (ra.0 - rb.0).abs() <= gap + slack { (ra.0 + rb.0) / 2.0 } else { ov0 };
        let s1 = if (ra.1 - rb.1).abs() <= gap + slack { (ra.1 + rb.1) / 2.0 } else { ov1 };
        found.push((la.k, (la.c + lb.c) / 2.0, gap, s0, s1));
        for r in [ia, ib] {
            if let Some(u) = used.get_mut(r) {
                u.push((ov0, ov1));
            }
        }
    }
    // A wall lying inside walls already found (longest first) repeats them: between two of a
    // wall's own hatch strokes the faces' pieces can pass for hatching.
    let line_of = |w: &(usize, f64, f64, f64, f64)| dirs.get(w.0).map(|&(d, n)| (d, n, w.1));
    found.sort_by(|a, b| (b.4 - b.3).total_cmp(&(a.4 - a.3)));
    let mut kept_walls: Vec<(usize, f64, f64, f64, f64)> = Vec::new();
    for w in found {
        let Some((d, n, c)) = line_of(&w) else { continue };
        let inside = (0..=4).all(|i| {
            let t = w.3 + (w.4 - w.3) * f64::from(i) / 4.0;
            let p = [d[0] * t + n[0] * c, d[1] * t + n[1] * c];
            kept_walls.iter().any(|o| {
                line_of(o).is_some_and(|(od, on, oc)| {
                    let reach = o.2 / 2.0 + slack;
                    (dot(on, p) - oc).abs() <= reach && dot(od, p) >= o.3 - reach && dot(od, p) <= o.4 + reach
                })
            })
        });
        if !inside {
            kept_walls.push(w);
        }
    }
    let found = kept_walls;
    // Corners and tees: an end within the two walls' thicknesses of another wall's centre line
    // (one that crosses it at 30° or more) is carried to that line, as a solid band's centre
    // line meets the next one's.
    let mut ends: Vec<(f64, f64)> = found.iter().map(|w| (w.3, w.4)).collect();
    for (i, wi) in found.iter().enumerate() {
        let Some((di, ni, ci)) = line_of(wi) else { continue };
        for (end, s) in [(0usize, wi.3), (1, wi.4)] {
            let mut best: Option<(f64, f64)> = None;
            for (j, wj) in found.iter().enumerate() {
                let Some((dj, nj, cj)) = line_of(wj) else { continue };
                let cross = di[0] * dj[1] - di[1] * dj[0];
                if j == i || cross.abs() < 0.5 {
                    continue;
                }
                // Where wall i's centre line meets wall j's: dot(nj, ci·ni + t·di) = cj.
                let t = (cj - ci * dot(nj, ni)) / dot(nj, di);
                let x = [ni[0] * ci + di[0] * t, ni[1] * ci + di[1] * t];
                let along_j = dot(dj, x);
                let reach = (wi.2 + wj.2) / 2.0 + slack;
                if (t - s).abs() <= reach && along_j >= wj.3 - reach && along_j <= wj.4 + reach && best.is_none_or(|b| (t - s).abs() < b.0) {
                    best = Some(((t - s).abs(), t));
                }
            }
            if let (Some((_, t)), Some(e)) = (best, ends.get_mut(i)) {
                if end == 0 {
                    e.0 = t;
                } else {
                    e.1 = t;
                }
            }
        }
    }
    let mut walls = Vec::new();
    // Strips taken out: (direction group, centre offset, half width, from, to).
    let mut strips: Vec<(usize, f64, f64, f64, f64)> = Vec::new();
    for (w, &(s0, s1)) in found.iter().zip(&ends) {
        let Some((d, n, mid)) = line_of(w) else { continue };
        let (s0, s1) = (s0.min(s1), s0.max(s1));
        let at = |t: f64| [d[0] * t + n[0] * mid, d[1] * t + n[1] * mid];
        walls.push(Shape::Wall { a: at(s0), b: at(s1), thickness: w.2 });
        strips.push((w.0, mid, w.2 / 2.0 + slack, s0 - w.2 / 2.0 - slack, s1 + w.2 / 2.0 + slack));
    }
    if strips.is_empty() {
        return walls;
    }
    // The strips by grid cell (their bounding boxes), so each piece is checked against the strips
    // near it only.
    const CELL: f64 = 256.0;
    let cell = |v: f64| (v / CELL).floor() as i64;
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (j, &(k, mid, half, t0, t1)) in strips.iter().enumerate() {
        let Some(&(d, n)) = dirs.get(k) else { continue };
        let pt = |t: f64, u: f64| [d[0] * t + n[0] * u, d[1] * t + n[1] * u];
        let cs = [pt(t0, mid - half), pt(t0, mid + half), pt(t1, mid - half), pt(t1, mid + half)];
        let (x0, x1) = (cs.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), cs.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max));
        let (y0, y1) = (cs.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min), cs.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max));
        for cx in cell(x0)..=cell(x1) {
            for cy in cell(y0)..=cell(y1) {
                grid.entry((cx, cy)).or_default().push(j);
            }
        }
    }
    let near = |lo: [f64; 2], hi: [f64; 2]| -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for cx in cell(lo[0])..=cell(hi[0]) {
            for cy in cell(lo[1])..=cell(hi[1]) {
                if let Some(v) = grid.get(&(cx, cy)) {
                    out.extend(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    };
    let in_strip = |p: [f64; 2], among: &[usize]| {
        among
            .iter()
            .filter_map(|&j| strips.get(j))
            .any(|&(k, mid, half, t0, t1)| dirs.get(k).is_some_and(|&(d, n)| (dot(n, p) - mid).abs() <= half && (t0..=t1).contains(&dot(d, p))))
    };
    curves.retain(|c| {
        let on = |center: [f64; 2], radius: f64, a0: f64, sweep: f64| {
            let among = near([center[0] - radius, center[1] - radius], [center[0] + radius, center[1] + radius]);
            !among.is_empty()
                && (0..=8).all(|i| {
                    let a = a0 + sweep * f64::from(i) / 8.0;
                    in_strip([center[0] + radius * a.cos(), center[1] + radius * a.sin()], &among)
                })
        };
        match *c {
            Shape::Arc { center, radius, start, end, .. } => !on(center, radius, start, (end - start).rem_euclid(std::f64::consts::TAU)),
            Shape::Circle { center, radius, .. } => !on(center, radius, 0.0, std::f64::consts::TAU),
            _ => true,
        }
    });
    // Thin segments inside a strip go; one crossing its end keeps the part outside.
    let mut kept: Vec<Seg> = Vec::with_capacity(segs.len());
    'seg: for s in segs.drain(..) {
        if s.2 >= thin_max {
            kept.push(s);
            continue;
        }
        let among = near([s.0[0].min(s.1[0]), s.0[1].min(s.1[1])], [s.0[0].max(s.1[0]), s.0[1].max(s.1[1])]);
        if among.is_empty() {
            kept.push(s);
            continue;
        }
        let mut pieces = vec![(0.0f64, 1.0f64)];
        for &(k, mid, half, t0, t1) in among.iter().filter_map(|&j| strips.get(j)) {
            let Some(&(d, n)) = dirs.get(k) else { continue };
            let (ua, ub) = (dot(n, s.0) - mid, dot(n, s.1) - mid);
            if ua.abs() > half || ub.abs() > half {
                continue;
            }
            let (ta, tb) = (dot(d, s.0), dot(d, s.1));
            if (tb - ta).abs() < 1e-9 {
                if (t0..=t1).contains(&ta) {
                    continue 'seg;
                }
                continue;
            }
            // The part of the segment (0..1 along it) inside t0..t1.
            let (p0, p1) = ((t0 - ta) / (tb - ta), (t1 - ta) / (tb - ta));
            let (cut0, cut1) = (p0.min(p1), p0.max(p1));
            pieces = pieces.into_iter().flat_map(|(a, b)| [(a, b.min(cut0)), (a.max(cut1), b)]).filter(|(a, b)| b - a > 1e-9).collect();
        }
        let len = (s.1[0] - s.0[0]).hypot(s.1[1] - s.0[1]);
        let at = |f: f64| [s.0[0] + (s.1[0] - s.0[0]) * f, s.0[1] + (s.1[1] - s.0[1]) * f];
        // A hatch stroke across a corner has its ends in two walls' strips: it goes when it lies
        // inside their union all along.
        for (a, b) in pieces {
            let inside = (0..=4).all(|i| in_strip(at(a + (b - a) * f64::from(i) / 4.0), &among));
            if !inside && ((b - a) * len >= min_len || (a == 0.0 && b == 1.0)) {
                kept.push((at(a), at(b), s.2));
            }
        }
    }
    *segs = kept;
    walls
}

/// Trace a grey image (`w` × `h`, row by row from the top, 0 black … 255 white).
pub fn trace(gray: &[u8], w: usize, h: usize, o: &Options) -> Result<Traced, String> {
    let n = w.checked_mul(h).ok_or("the image is too large")?;
    if n == 0 || gray.len() < n {
        return Err("the image has no pixels".into());
    }
    if n > MAX_PIXELS {
        return Err(format!("the image is larger than {} megapixels; scale it down first", MAX_PIXELS / 1_000_000));
    }
    let threshold = o.threshold.unwrap_or_else(|| otsu(gray.get(..n).unwrap_or_default()));
    let mut bits = Bits { w, h, on: gray.iter().take(n).map(|&g| g <= threshold).collect() };
    despeckle(&mut bits, 4);
    let tol = if o.tolerance.is_finite() { o.tolerance.clamp(0.25, 50.0) } else { 1.5 };
    let flip = |x: f64, y: f64| [x + 0.5, h as f64 - y - 0.5];
    let mut shapes = Vec::new();
    // Text: small blobs, grouped in rows, boxed and taken out before tracing lines.
    if o.text {
        let given = o.text_max_px.filter(|v| v.is_finite() && *v > 0.0);
        // The scan's scale from its pen: the typical stroke is a fine line (about 0.25 mm), so
        // ordinary letters (at most about 3.5 mm) are under 14 strokes tall and titles (about
        // 12 mm) under 48, however many pixels the sheet has; never more than a share of the
        // image (2.5%, 6%). A full sheet's device symbols (4–5 mm) are then not letters.
        let pen = typical_run(&bits);
        let tmax = given.unwrap_or((0.025 * w.min(h) as f64).min(14.0 * pen)).max(4.0);
        // Larger letters (titles, room names on a fine scan) count when they have company on
        // their row, so a lone column or symbol of that size isn't taken for a letter.
        let big = given.unwrap_or((0.06 * w.min(h) as f64).min(48.0 * pen)).max(tmax);
        // Each group: its box, its blobs, and whether it has a letter of the ordinary size.
        let mut groups: Vec<([f64; 4], Vec<Vec<usize>>, bool, usize)> = Vec::new();
        for comp in components(&bits) {
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for &i in &comp {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let (bw, bh) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
            // Letter-sized and not a thin rule (a short line would look like an "l" alone).
            let letter = |m: f64| bh <= m && bw <= m * 1.5 && comp.len() >= 3 && !(bw.max(bh) > 4.0 * bw.min(bh).max(1.0) && bw.max(bh) > m * 0.6);
            let small = letter(tmax);
            if small || letter(big) {
                groups.push(([x0 as f64, y0 as f64, x1 as f64 + 1.0, y1 as f64 + 1.0], vec![comp], small, 1));
            }
        }
        // Letters within about a letter's height of each other on a row are one box.
        let mut merged = true;
        while merged {
            merged = false;
            'outer: for i in 0..groups.len() {
                for j in i + 1..groups.len() {
                    let (a, b) = (groups[i].0, groups[j].0);
                    let gap = 0.9 * (a[3] - a[1]).max(b[3] - b[1]);
                    let overlap_y = a[1].max(b[1]) < a[3].min(b[3]);
                    let near_x = a[0].max(b[0]) - a[2].min(b[2]) <= gap;
                    if overlap_y && near_x {
                        let (_, comps, small, count) = groups.swap_remove(j);
                        let g = &mut groups[i];
                        g.0 = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                        g.1.extend(comps);
                        g.2 |= small;
                        g.3 += count;
                        merged = true;
                        break 'outer;
                    }
                }
            }
        }
        for (b, comps, small, count) in &groups {
            // Lone specks are noise, not text; a lone large blob is a symbol, not a letter.
            if (b[2] - b[0]) * (b[3] - b[1]) < 9.0 {
                continue;
            }
            // Groups of only "big" blobs (no confirmed ordinary-sized letter) need at least 2
            // members, not all of them rings (a row of symbols, circles or outlined boxes, is
            // closed outlines round a hole; a word has open letters), and the ink of letters
            // drawn with the pen: strokes across each letter's height fill at least about
            // twice the pen ÷ the height of the box, up to a tenth (scattered specks fill less).
            if !small
                && (*count < 2 || comps.iter().all(|c| ring(c, w)) || {
                    let area = (b[2] - b[0]) * (b[3] - b[1]);
                    let ink: usize = comps.iter().map(|c| c.len()).sum();
                    ink as f64 / area < (2.0 * pen / (b[3] - b[1]).max(1.0)).min(0.10)
                })
            {
                continue;
            }
            for i in comps.iter().flatten() {
                if let Some(p) = bits.on.get_mut(*i) {
                    *p = false;
                }
            }
            shapes.push(Shape::TextBox { min: [b[0], h as f64 - b[3]], max: [b[2], h as f64 - b[1]] });
        }
    }
    let dist = distance(&bits);
    thin(&mut bits);
    let found = chains(&bits);
    // Stroke width along each chain: twice the distance at its pixels.
    let width_of = |c: &[(i64, i64)]| -> f64 {
        let mut ws: Vec<f32> = c.iter().filter_map(|&(x, y)| dist.get(y as usize * w + x as usize)).map(|d| 2.0 * d).collect();
        ws.sort_by(|a, b| a.total_cmp(b));
        ws.get(ws.len() / 2).map_or(1.0, |v| f64::from(*v))
    };
    let widths: Vec<f64> = found.iter().map(|(c, _)| width_of(c)).collect();
    let stroke = {
        let mut ws: Vec<f64> = found.iter().zip(&widths).filter(|((c, _), _)| c.len() as f64 >= o.min_length).map(|(_, w)| *w).collect();
        ws.sort_by(|a, b| a.total_cmp(b));
        ws.get(ws.len() / 4).copied().unwrap_or(1.0).max(1.0)
    };
    let wall_min = o.wall_min_px.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(stroke * 3.0).max(3.0);
    // Segments as (a, b, width), joined and snapped below.
    let mut segs: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
    for ((chain, closed), width) in found.iter().zip(widths) {
        let pts: Vec<[f64; 2]> = chain.iter().map(|&(x, y)| flip(x as f64, y as f64)).collect();
        let len: f64 = pts.windows(2).map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1])).sum();
        if len < o.min_length.max(1.0) {
            continue;
        }
        let keep = rdp(&pts, tol);
        // The circle is fitted away from the chain's ends: where another line crosses (a grid
        // line through a column) thinning bends the last few pixels toward the junction.
        // A shallow crossing merges the two strokes for longer: a sixth of the chain at each end.
        let trim = if *closed { 0 } else { (width.ceil() as usize + 1).max(pts.len() / 6).min(pts.len() / 4) };
        let core = pts.get(trim..pts.len() - trim).filter(|c| c.len() >= 8).unwrap_or(&pts);
        // Curved (several RDP pieces) and on a circle: an arc, or a whole circle when closed.
        if keep.len() >= 4
            && let Some((c, r, rms)) = fit_circle(core)
            && rms <= tol * 0.75
            && r > 2.0 * tol
            && r < 4.0 * (w.max(h) as f64)
        {
            let ang: Vec<f64> = pts.iter().map(|q| (q[1] - c[1]).atan2(q[0] - c[0])).collect();
            let mut sweep = 0.0;
            for a in ang.windows(2) {
                let mut d = a[1] - a[0];
                d = (d + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                sweep += d;
            }
            let full = *closed || sweep.abs() > std::f64::consts::TAU - 0.35;
            if full {
                shapes.push(Shape::Circle { center: c, radius: r, width });
                continue;
            }
            if sweep.abs() >= 30f64.to_radians() {
                let (a0, a1) = (ang[0], ang[ang.len() - 1]);
                let (start, end) = if sweep > 0.0 { (a0, a1) } else { (a1, a0) };
                shapes.push(Shape::Arc {
                    center: c,
                    radius: r,
                    start: start.rem_euclid(std::f64::consts::TAU),
                    end: end.rem_euclid(std::f64::consts::TAU),
                    width,
                });
                continue;
            }
        }
        for k in keep.windows(2) {
            if let (Some(a), Some(b)) = (pts.get(k[0]), pts.get(k[1])) {
                segs.push((*a, *b, width));
            }
        }
        if *closed && let (Some(a), Some(b)) = (keep.last().and_then(|i| pts.get(*i)), pts.first()) {
            segs.push((*a, *b, width));
        }
    }
    merge_arcs(&mut shapes, tol);
    // Join end points that meet (within the tolerance and the stroke) to their average.
    let join = (tol * 2.0).max(stroke);
    let mut nodes: Vec<[f64; 2]> = Vec::new();
    let node_of = |p: [f64; 2], nodes: &mut Vec<[f64; 2]>| -> usize {
        match nodes.iter().position(|q| (q[0] - p[0]).hypot(q[1] - p[1]) <= join) {
            Some(i) => i,
            None => {
                nodes.push(p);
                nodes.len() - 1
            }
        }
    };
    let mut edges: Vec<(usize, usize, f64)> =
        segs.iter().map(|(a, b, w)| (node_of(*a, &mut nodes), node_of(*b, &mut nodes), *w)).filter(|(a, b, _)| a != b).collect();
    edges.truncate(MAX_SHAPES);
    // Orthogonal snap: a near-horizontal edge ties its ends' y, a near-vertical one their x; each
    // tied group takes its mean (union-find), so corners stay joined.
    let snap = o.ortho_deg.is_finite() && o.ortho_deg > 0.0;
    let lim = o.ortho_deg.clamp(0.0, 20.0).to_radians();
    let mut px: Vec<usize> = (0..nodes.len()).collect();
    let mut py: Vec<usize> = (0..nodes.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while let Some(&up) = p.get(i) {
            if up == i {
                break;
            }
            let grand = p.get(up).copied().unwrap_or(up);
            if let Some(v) = p.get_mut(i) {
                *v = grand;
            }
            i = up;
        }
        i
    }
    // A slightly turned scan: snap to its own main directions, not the image's axes (that would
    // bend the walls that are snapped against the lines that aren't).
    let theta = if snap {
        let list: Vec<([f64; 2], [f64; 2])> = edges.iter().filter_map(|&(a, b, _)| Some((*nodes.get(a)?, *nodes.get(b)?))).collect();
        main_direction(&list, 4.0 * tol)
    } else {
        0.0
    };
    let turn = |q: [f64; 2], t: f64| [q[0] * t.cos() - q[1] * t.sin(), q[0] * t.sin() + q[1] * t.cos()];
    if theta != 0.0 {
        for q in nodes.iter_mut() {
            *q = turn(*q, -theta);
        }
    }
    if snap {
        for &(a, b, _) in &edges {
            let (pa, pb) = (nodes.get(a).copied().unwrap_or_default(), nodes.get(b).copied().unwrap_or_default());
            let ang = (pb[1] - pa[1]).atan2(pb[0] - pa[0]).rem_euclid(std::f64::consts::PI);
            let off_h = ang.min(std::f64::consts::PI - ang);
            let off_v = (ang - std::f64::consts::FRAC_PI_2).abs();
            if off_h <= lim {
                let (ra, rb) = (root(&mut py, a), root(&mut py, b));
                if let Some(v) = py.get_mut(ra) {
                    *v = rb;
                }
            } else if off_v <= lim {
                let (ra, rb) = (root(&mut px, a), root(&mut px, b));
                if let Some(v) = px.get_mut(ra) {
                    *v = rb;
                }
            }
        }
        let mean = |nodes: &[[f64; 2]], p: &mut Vec<usize>, axis: usize| -> Vec<f64> {
            let mut sum = vec![(0.0, 0usize); nodes.len()];
            for i in 0..nodes.len() {
                let r = root(p, i);
                if let (Some(s), Some(q)) = (sum.get_mut(r), nodes.get(i)) {
                    s.0 += q[axis];
                    s.1 += 1;
                }
            }
            (0..nodes.len()).map(|i| sum.get(root(p, i)).map_or(0.0, |(s, n)| s / (*n).max(1) as f64)).collect()
        };
        let xs = mean(&nodes, &mut px, 0);
        let ys = mean(&nodes, &mut py, 1);
        for (i, q) in nodes.iter_mut().enumerate() {
            *q = [xs.get(i).copied().unwrap_or(q[0]), ys.get(i).copied().unwrap_or(q[1])];
        }
    }
    if theta != 0.0 {
        for q in nodes.iter_mut() {
            *q = turn(*q, theta);
        }
    }
    // Collinear runs through two-edge nodes become one line (a thinned corner leaves extra nodes).
    let mut degree = vec![0usize; nodes.len()];
    for &(a, b, _) in &edges {
        for i in [a, b] {
            if let Some(d) = degree.get_mut(i) {
                *d += 1;
            }
        }
    }
    let mut merged = true;
    while merged {
        merged = false;
        'scan: for n in 0..nodes.len() {
            if degree.get(n) != Some(&2) {
                continue;
            }
            let at: Vec<usize> = edges.iter().enumerate().filter(|(_, e)| e.0 == n || e.1 == n).map(|(i, _)| i).take(3).collect();
            let [i, j] = at.as_slice() else { continue };
            let (ei, ej) = (edges[*i], edges[*j]);
            let other = |e: (usize, usize, f64)| if e.0 == n { e.1 } else { e.0 };
            let (oa, ob) = (other(ei), other(ej));
            let (Some(pa), Some(pn), Some(pb)) = (nodes.get(oa), nodes.get(n), nodes.get(ob)) else { continue };
            let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
            let len = dx.hypot(dy);
            if len < 1e-9 || oa == ob {
                continue;
            }
            let off = ((pn[0] - pa[0]) * dy - (pn[1] - pa[1]) * dx).abs() / len;
            let similar = (ei.2 - ej.2).abs() <= wall_min.max(ei.2.max(ej.2)) * 0.5;
            if off <= tol && similar {
                let (hi, lo) = if i > j { (*i, *j) } else { (*j, *i) };
                edges.swap_remove(hi);
                edges.swap_remove(lo);
                edges.push((oa, ob, (ei.2 + ej.2) / 2.0));
                if let Some(d) = degree.get_mut(n) {
                    *d = 0;
                }
                merged = true;
                break 'scan;
            }
        }
    }
    let mut segs: Vec<Seg> = edges.iter().filter_map(|&(a, b, width)| Some((*nodes.get(a)?, *nodes.get(b)?, width))).collect();
    if o.walls {
        let walls = hatched_walls(&mut segs, &mut shapes, wall_min, stroke, tol, o.min_length.max(1.0), o.wall_pairs);
        shapes.extend(walls);
    }
    for (pa, pb, width) in segs {
        if (pb[0] - pa[0]).hypot(pb[1] - pa[1]) < o.min_length.max(1.0) {
            continue;
        }
        if o.walls && width >= wall_min {
            shapes.push(Shape::Wall { a: pa, b: pb, thickness: width });
        } else {
            shapes.push(Shape::Line { a: pa, b: pb, width });
        }
    }
    shapes.truncate(MAX_SHAPES);
    Ok(Traced { shapes, threshold, stroke })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white canvas to draw synthetic plans on (y down, like an image).
    struct Canvas {
        w: usize,
        h: usize,
        px: Vec<u8>,
    }

    impl Canvas {
        fn new(w: usize, h: usize) -> Canvas {
            Canvas { w, h, px: vec![255; w * h] }
        }
        fn dot(&mut self, cx: f64, cy: f64, r: f64) {
            let r = r.max(0.5);
            for y in (cy - r).floor() as i64..=(cy + r).ceil() as i64 {
                for x in (cx - r).floor() as i64..=(cx + r).ceil() as i64 {
                    if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) <= r {
                        self.px[y as usize * self.w + x as usize] = 0;
                    }
                }
            }
        }
        /// A stroke `width` pixels wide (y down).
        fn line(&mut self, a: (f64, f64), b: (f64, f64), width: f64) {
            let n = ((b.0 - a.0).hypot(b.1 - a.1) * 2.0).ceil().max(1.0) as usize;
            for i in 0..=n {
                let t = i as f64 / n as f64;
                self.dot(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, width / 2.0);
            }
        }
        fn circle(&mut self, c: (f64, f64), r: f64, width: f64, from: f64, to: f64) {
            let n = (r * (to - from).abs() * 2.0).ceil().max(8.0) as usize;
            for i in 0..=n {
                let a = from + (to - from) * i as f64 / n as f64;
                self.dot(c.0 + r * a.cos(), c.1 - r * a.sin(), width / 2.0);
            }
        }
        fn trace(&self, o: &Options) -> Traced {
            trace(&self.px, self.w, self.h, o).unwrap()
        }
    }

    fn near(p: [f64; 2], q: [f64; 2], tol: f64) -> bool {
        (p[0] - q[0]).hypot(p[1] - q[1]) <= tol
    }

    fn lines(t: &Traced) -> Vec<([f64; 2], [f64; 2])> {
        t.shapes.iter().filter_map(|s| if let Shape::Line { a, b, .. } = s { Some((*a, *b)) } else { None }).collect()
    }

    #[test]
    fn a_turned_room_snaps_to_its_own_directions() {
        // A room turned 2° with a diagonal brace across it: the walls are made exactly parallel
        // and square at 2° (not snapped to the image's axes), the brace stays where it is.
        let mut c = Canvas::new(400, 300);
        let t = 2f64.to_radians();
        let turn = |x: f64, y: f64| (200.0 + (x - 200.0) * t.cos() - (y - 150.0) * t.sin(), 150.0 + (x - 200.0) * t.sin() + (y - 150.0) * t.cos());
        let corners = [turn(60.0, 60.0), turn(340.0, 60.0), turn(340.0, 240.0), turn(60.0, 240.0)];
        for i in 0..4 {
            c.line(corners[i], corners[(i + 1) % 4], 2.0);
        }
        c.line(turn(100.0, 220.0), turn(300.0, 80.0), 2.0);
        let tr = c.trace(&Options { text: false, ..Options::default() });
        let ls = lines(&tr);
        assert_eq!(ls.len(), 5, "{ls:?}");
        // Each line's turn from the nearest axis; image y is up in the result and down on the
        // canvas, so the room is at -2° there.
        let q = std::f64::consts::FRAC_PI_2;
        let fold = |(a, b): &([f64; 2], [f64; 2])| ((b[1] - a[1]).atan2(b[0] - a[0]) + q / 2.0).rem_euclid(q) - q / 2.0;
        let mut turns: Vec<f64> = ls.iter().map(fold).collect();
        turns.sort_by(|a, b| a.total_cmp(b));
        // Four walls exactly square to each other at about -2°; the brace (about 35°) apart.
        let walls: Vec<f64> = turns.iter().copied().filter(|d| (d + t).abs() < 0.2f64.to_radians()).collect();
        assert_eq!(walls.len(), 4, "{turns:?}");
        assert!(walls.iter().all(|d| (d - walls[0]).abs() < 1e-9), "exactly square: {walls:?}");
    }

    #[test]
    fn specks_in_a_solid_wall_leave_it_a_wall() {
        // A 14 px band, and thin lines that make it thick by comparison.
        let mut c = Canvas::new(300, 100);
        c.line((20.0, 50.0), (280.0, 50.0), 14.0);
        c.line((20.0, 85.0), (280.0, 85.0), 2.0);
        c.line((20.0, 92.0), (280.0, 92.0), 2.0);
        // Paper-white specks inside the band, and ink specks on the paper.
        for (x, y) in [(60, 48), (61, 52), (130, 50), (200, 47), (201, 47), (250, 53)] {
            c.px[y * 300 + x] = 255;
        }
        for (x, y) in [(40, 20), (150, 70), (151, 70), (220, 15)] {
            c.px[y * 300 + x] = 0;
        }
        let t = c.trace(&Options { text: false, ..Options::default() });
        let walls = t.shapes.iter().filter(|s| matches!(s, Shape::Wall { .. })).count();
        assert_eq!((walls, t.shapes.len()), (1, 3), "one wall and the two lines, no rings or specks: {:?}", t.shapes);
    }

    #[test]
    fn large_letters_in_a_row_are_text_and_a_lone_blob_of_that_size_is_not() {
        // 600 × 400: ordinary letters are up to 10 px (2.5%), large ones up to 24 px (6%) with
        // company. Three 20 px "letters" in a row, and a lone ring of the same size.
        let mut c = Canvas::new(600, 400);
        for k in 0..3 {
            let x = 100.0 + 18.0 * f64::from(k);
            c.line((x, 100.0), (x, 120.0), 2.0);
            c.line((x, 100.0), (x + 10.0, 100.0), 2.0);
            c.line((x, 110.0), (x + 8.0, 110.0), 2.0);
        }
        c.circle((400.0, 300.0), 9.0, 2.0, 0.0, std::f64::consts::TAU);
        let t = c.trace(&Options::default());
        let boxes: Vec<_> = t.shapes.iter().filter(|s| matches!(s, Shape::TextBox { .. })).collect();
        assert_eq!(boxes.len(), 1, "{:?}", t.shapes);
        assert!(t.shapes.iter().any(|s| matches!(s, Shape::Circle { .. })), "the ring is a circle: {:?}", t.shapes);
    }

    #[test]
    fn otsu_splits_ink_from_paper() {
        let mut g = vec![240u8; 1000];
        g.extend(vec![20u8; 200]);
        let t = otsu(&g);
        assert!((20..240).contains(&t), "{t}");
        assert_eq!(otsu(&[]), 128);
    }

    #[test]
    fn a_room_outline_traces_to_four_orthogonal_joined_lines() {
        let mut c = Canvas::new(220, 160);
        // y down: corners (20,20) (200,20) (200,140) (20,140) → y up: 140 / 20.
        c.line((20.0, 20.0), (200.0, 20.0), 2.0);
        c.line((200.0, 20.0), (200.0, 140.0), 2.0);
        c.line((200.0, 140.0), (20.0, 140.0), 2.0);
        c.line((20.0, 140.0), (20.0, 20.0), 2.0);
        let t = c.trace(&Options::default());
        let ls = lines(&t);
        assert_eq!(ls.len(), 4, "{:?}", t.shapes);
        for (a, b) in &ls {
            assert!(a[0] == b[0] || a[1] == b[1], "orthogonal: {a:?} {b:?}");
        }
        let corners = [[20.0, 140.0], [200.0, 140.0], [200.0, 20.0], [20.0, 20.0]];
        for k in corners {
            assert!(ls.iter().any(|(a, b)| near(*a, k, 2.0) || near(*b, k, 2.0)), "corner {k:?} in {ls:?}");
        }
        assert!(t.shapes.iter().all(|s| !matches!(s, Shape::Wall { .. } | Shape::TextBox { .. })));
    }

    #[test]
    fn circles_and_arcs_trace_back_within_tolerance() {
        let mut c = Canvas::new(240, 200);
        c.circle((70.0, 100.0), 40.0, 2.0, 0.0, std::f64::consts::TAU);
        // A door swing: a quarter arc, counter-clockwise from 0° to 90° (y up).
        c.circle((150.0, 150.0), 50.0, 2.0, 0.0, std::f64::consts::FRAC_PI_2);
        let t = c.trace(&Options { text: false, ..Options::default() });
        let circle = t.shapes.iter().find_map(|s| if let Shape::Circle { center, radius, .. } = s { Some((*center, *radius)) } else { None });
        let (cc, r) = circle.unwrap_or_else(|| panic!("no circle in {:?}", t.shapes));
        assert!(near(cc, [70.0, 100.0], 1.5) && (r - 40.0).abs() < 1.5, "{cc:?} {r}");
        let arc = t
            .shapes
            .iter()
            .find_map(|s| if let Shape::Arc { center, radius, start, end, .. } = s { Some((*center, *radius, *start, *end)) } else { None });
        let (ac, ar, s, e) = arc.unwrap_or_else(|| panic!("no arc in {:?}", t.shapes));
        assert!(near(ac, [150.0, 50.0], 2.0) && (ar - 50.0).abs() < 2.0, "{ac:?} {ar}");
        assert!(s.to_degrees() < 6.0 || s.to_degrees() > 354.0, "start {}", s.to_degrees());
        assert!((e.to_degrees() - 90.0).abs() < 6.0, "end {}", e.to_degrees());
    }

    #[test]
    fn thick_bands_are_walls_and_letters_are_text_boxes() {
        let mut c = Canvas::new(300, 200);
        c.line((20.0, 40.0), (280.0, 40.0), 10.0);
        c.line((20.0, 150.0), (280.0, 150.0), 2.0);
        // A word: five letter-sized blobs in a row.
        for i in 0..5 {
            let x = 60.0 + i as f64 * 8.0;
            c.line((x, 90.0), (x, 98.0), 2.0);
            c.line((x, 90.0), (x + 4.0, 90.0), 2.0);
        }
        let t = c.trace(&Options { text_max_px: Some(12.0), ..Options::default() });
        let wall = t.shapes.iter().find_map(|s| if let Shape::Wall { a, b, thickness } = s { Some((*a, *b, *thickness)) } else { None });
        let (a, b, th) = wall.unwrap_or_else(|| panic!("no wall in {:?}", t.shapes));
        assert!((th - 10.0).abs() <= 2.5, "thickness {th}");
        assert!((a[1] - 160.0).abs() < 1.5 && (b[1] - 160.0).abs() < 1.5 && (a[0] - b[0]).abs() > 230.0, "{a:?} {b:?}");
        assert_eq!(lines(&t).len(), 1, "the thin line stays a line: {:?}", t.shapes);
        let boxes: Vec<_> = t.shapes.iter().filter_map(|s| if let Shape::TextBox { min, max } = s { Some((*min, *max)) } else { None }).collect();
        assert_eq!(boxes.len(), 1, "one word: {boxes:?}");
        let (mn, mx) = boxes[0];
        assert!(mn[0] <= 60.0 && mx[0] >= 92.0 && mn[1] <= 102.0 && mx[1] >= 109.0, "{mn:?} {mx:?}");
        // Text off: no boxes, and the letters are traced as strokes instead.
        let t2 = c.trace(&Options { text: false, text_max_px: Some(12.0), ..Options::default() });
        assert!(t2.shapes.iter().all(|s| !matches!(s, Shape::TextBox { .. })));
    }

    /// Two 2 px face lines 16 px apart (y down 40 and 56, x 20–280) and, apart, a thin line.
    fn double_line(hatch: bool) -> Canvas {
        let mut c = Canvas::new(300, 140);
        c.line((20.0, 40.0), (280.0, 40.0), 2.0);
        c.line((20.0, 56.0), (280.0, 56.0), 2.0);
        if hatch {
            // 45° strokes from face to face, 12 px apart along the band (8.5 px square to the
            // strokes: four times the stroke).
            let mut x = 24.0;
            while x + 16.0 <= 276.0 {
                c.line((x, 56.0), (x + 16.0, 40.0), 2.0);
                x += 12.0;
            }
        }
        c.line((20.0, 110.0), (280.0, 110.0), 2.0);
        c
    }

    #[test]
    fn a_hatched_band_is_one_wall_and_a_bare_double_line_is_not() {
        let walls = |t: &Traced| -> Vec<([f64; 2], [f64; 2], f64)> {
            t.shapes.iter().filter_map(|s| if let Shape::Wall { a, b, thickness } = s { Some((*a, *b, *thickness)) } else { None }).collect()
        };
        // The band's centre line: y down 48 → y up 140 − 48 = 92.
        let check_wall = |t: &Traced| {
            let w = walls(t);
            assert_eq!(w.len(), 1, "one wall: {:?}", t.shapes);
            let (a, b, th) = w[0];
            assert!((th - 16.0).abs() <= 2.5, "thickness {th}");
            assert!((a[1] - 92.0).abs() < 1.5 && (b[1] - 92.0).abs() < 1.5 && (a[0] - b[0]).abs() > 240.0, "{a:?} {b:?}");
            // Only the thin line apart is a line: no faces, no hatch strokes.
            let ls = lines(t);
            assert_eq!(ls.len(), 1, "no lines from the band: {ls:?}");
            assert!((ls[0].0[1] - 30.0).abs() < 1.5 && (ls[0].1[1] - 30.0).abs() < 1.5, "{ls:?}");
        };
        let o = Options { text: false, ..Options::default() };
        check_wall(&double_line(true).trace(&o));
        // Text finding on (the default) leaves the hatched band alone too.
        check_wall(&double_line(true).trace(&Options::default()));
        // No hatch between: two lines, not a wall, unless asked.
        let bare = double_line(false).trace(&o);
        assert!(walls(&bare).is_empty(), "{:?}", bare.shapes);
        assert_eq!(lines(&bare).len(), 3, "{:?}", bare.shapes);
        check_wall(&double_line(false).trace(&Options { wall_pairs: true, ..o.clone() }));
        // Walls off: nothing is paired.
        let off = double_line(true).trace(&Options { walls: false, ..o });
        assert!(walls(&off).is_empty(), "{:?}", off.shapes);
    }

    #[test]
    fn hostile_images_are_refused_or_traced_to_nothing() {
        assert!(trace(&[], 0, 0, &Options::default()).is_err());
        assert!(trace(&[0; 10], 5, 5, &Options::default()).is_err(), "short data");
        assert!(trace(&[0; 4], usize::MAX, 2, &Options::default()).is_err());
        let blank = trace(&vec![255; 400], 20, 20, &Options::default()).unwrap();
        assert!(blank.shapes.is_empty());
        let black = trace(&vec![0; 400], 20, 20, &Options { text: false, ..Options::default() }).unwrap();
        assert!(black.shapes.len() <= 8);
        let noise: Vec<u8> = (0..10_000u32).map(|i| if i.wrapping_mul(2_654_435_761u32) >> 28 < 3 { 0 } else { 255 }).collect();
        let t = trace(&noise, 100, 100, &Options { tolerance: f64::NAN, ortho_deg: f64::INFINITY, ..Options::default() }).unwrap();
        assert!(t.shapes.len() < 10_000);
    }
}
