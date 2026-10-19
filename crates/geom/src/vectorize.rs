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

/// How to trace.
#[derive(Clone, Debug)]
pub struct Options {
    /// Ink is at most this grey (0..255); `None`: chosen by Otsu's method.
    pub threshold: Option<u8>,
    /// Largest distance from a traced shape to the stroke's centre, pixels.
    pub tolerance: f64,
    /// Lines within this many degrees of horizontal or vertical are made exactly so.
    pub ortho_deg: f64,
    /// Thick strokes become wall pairs.
    pub walls: bool,
    /// Strokes at least this wide (pixels) are walls; `None`: three times the typical stroke.
    pub wall_min_px: Option<f64>,
    /// Small blobs in rows become text boxes.
    pub text: bool,
    /// Text is at most this tall (pixels); `None`: 2.5% of the image's shorter side.
    pub text_max_px: Option<f64>,
    /// Shorter chains are dropped (specks), pixels.
    pub min_length: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { threshold: None, tolerance: 1.5, ortho_deg: 3.0, walls: true, wall_min_px: None, text: true, text_max_px: None, min_length: 4.0 }
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
    let is_node = |x: i64, y: i64| deg(x, y) <= 1 || crossings(x, y) != 2;
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
    let tol = if o.tolerance.is_finite() { o.tolerance.clamp(0.25, 50.0) } else { 1.5 };
    let flip = |x: f64, y: f64| [x + 0.5, h as f64 - y - 0.5];
    let mut shapes = Vec::new();
    // Text: small blobs, grouped in rows, boxed and taken out before tracing lines.
    if o.text {
        let tmax = o.text_max_px.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.025 * w.min(h) as f64).max(4.0);
        let mut boxes: Vec<[f64; 4]> = Vec::new();
        for comp in components(&bits) {
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for &i in &comp {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let (bw, bh) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
            // Letter-sized and not a thin rule (a short line would look like an "l" alone).
            if bh <= tmax && bw <= tmax * 1.5 && comp.len() >= 3 && !(bw.max(bh) > 4.0 * bw.min(bh).max(1.0) && bw.max(bh) > tmax * 0.6) {
                boxes.push([x0 as f64, y0 as f64, x1 as f64 + 1.0, y1 as f64 + 1.0]);
                for &i in &comp {
                    if let Some(p) = bits.on.get_mut(i) {
                        *p = false;
                    }
                }
            }
        }
        // Letters within about a letter's height of each other on a row are one box.
        let mut merged = true;
        while merged {
            merged = false;
            'outer: for i in 0..boxes.len() {
                for j in i + 1..boxes.len() {
                    let (a, b) = (boxes[i], boxes[j]);
                    let gap = 0.9 * (a[3] - a[1]).max(b[3] - b[1]);
                    let overlap_y = a[1].max(b[1]) < a[3].min(b[3]);
                    let near_x = a[0].max(b[0]) - a[2].min(b[2]) <= gap;
                    if overlap_y && near_x {
                        boxes[i] = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                        boxes.swap_remove(j);
                        merged = true;
                        break 'outer;
                    }
                }
            }
        }
        for b in boxes {
            // Lone specks are noise, not text.
            if (b[2] - b[0]) * (b[3] - b[1]) >= 9.0 {
                shapes.push(Shape::TextBox { min: [b[0], h as f64 - b[3]], max: [b[2], h as f64 - b[1]] });
            }
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
        // Curved (several RDP pieces) and on a circle: an arc, or a whole circle when closed.
        if keep.len() >= 4
            && let Some((c, r, rms)) = fit_circle(&pts)
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
    for (a, b, width) in edges {
        let (Some(pa), Some(pb)) = (nodes.get(a), nodes.get(b)) else { continue };
        if (pb[0] - pa[0]).hypot(pb[1] - pa[1]) < o.min_length.max(1.0) {
            continue;
        }
        if o.walls && width >= wall_min {
            shapes.push(Shape::Wall { a: *pa, b: *pb, thickness: width });
        } else {
            shapes.push(Shape::Line { a: *pa, b: *pb, width });
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
