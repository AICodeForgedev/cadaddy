//! Reading text off a scan: letter-sized blobs of ink grouped into lines (upright, or turned a
//! quarter, half or three quarters), each line cut into characters and every character matched
//! against templates drawn from CADaddy's own stroke font (the font the prints were drafted in;
//! no outside model, data or network). The match is a chamfer distance: how far the scan's ink
//! lies from the template's strokes and the template's strokes from the ink, in the line's own
//! frame (baseline and cap height), so a quote is told from a comma and an `x` from an `X`.
//!
//! Blobs that don't read as text (symbols, boxes, short lines) are left for line tracing.

use cadaddy_geom::Vec2;

/// Most ink components looked at (a dense 36" × 24" sheet at 200 dpi has ~60 000).
const MAX_COMPS: usize = 400_000;
/// Most pixels in one component kept for reading (larger ones are lines, not letters).
const MAX_LETTER_PX: usize = 20_000;
/// Most characters in a line.
const MAX_LINE_CHARS: usize = 200;

/// Grid units per cap height in the matching frame.
const UNIT: f64 = 20.0;
/// The matching grid: columns centred on the character, rows from below the baseline (descenders,
/// commas) to above the cap height (quotes).
const GRID_W: usize = 44;
const GRID_H: usize = 38;
/// The baseline's row from the grid's bottom, and the extra room above the cap height.
const BELOW: f64 = 0.45;

/// The characters the reader knows: what consultant MEP prints are lettered in.
pub const CHARSET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-'\"/.,()=@:%#&+x\u{d8}";

/// Ink: `w` × `h`, row by row from the top.
#[derive(Clone, Debug, Default)]
pub struct Ink {
    pub w: usize,
    pub h: usize,
    pub on: Vec<bool>,
}

impl Ink {
    /// Ink where the grey is at most `threshold`.
    pub fn from_gray(gray: &[u8], w: usize, h: usize, threshold: u8) -> Ink {
        Ink { w, h, on: gray.iter().take(w.saturating_mul(h)).map(|&g| g <= threshold).collect() }
    }

    /// Ink by hysteresis: at most `strong`, or at most `weak` and touching such ink (one step
    /// only, so a grey line meeting a black one isn't drawn in along with it).
    pub fn hysteresis(gray: &[u8], w: usize, h: usize, strong: u8, weak: u8) -> Ink {
        let n = w.saturating_mul(h).min(gray.len());
        let strong_at = |x: i64, y: i64| {
            x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && gray.get(y as usize * w + x as usize).is_some_and(|&g| g <= strong)
        };
        let on = (0..n)
            .map(|i| {
                let g = gray.get(i).copied().unwrap_or(255);
                if g <= strong {
                    return true;
                }
                if g > weak {
                    return false;
                }
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                strong_at(x - 1, y) || strong_at(x + 1, y) || strong_at(x, y - 1) || strong_at(x, y + 1)
            })
            .collect();
        Ink { w, h, on }
    }

    pub fn get(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        self.on.get(y as usize * self.w + x as usize).copied().unwrap_or(false)
    }
}

/// A connected blob of ink (8-connected).
#[derive(Clone, Debug, Default)]
pub struct Comp {
    /// Pixel indices (`y * w + x`).
    pub px: Vec<usize>,
    /// Bounding box, inclusive, image pixels (y down).
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Comp {
    pub fn width(&self) -> usize {
        self.x1 + 1 - self.x0
    }
    pub fn height(&self) -> usize {
        self.y1 + 1 - self.y0
    }
}

/// The ink's connected blobs (at most [`MAX_COMPS`]; pixel lists of blobs over
/// [`MAX_LETTER_PX`] are dropped, only their boxes kept).
pub fn components(ink: &Ink) -> Vec<Comp> {
    let (w, h) = (ink.w, ink.h);
    let mut seen = vec![false; ink.on.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..ink.on.len().min(w.saturating_mul(h)) {
        if seen.get(start).copied().unwrap_or(true) || !ink.on.get(start).copied().unwrap_or(false) {
            continue;
        }
        if out.len() >= MAX_COMPS {
            break;
        }
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        stack.clear();
        stack.push(start);
        let mut c = Comp { x0: usize::MAX, y0: usize::MAX, ..Comp::default() };
        let mut keep = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (c.x0, c.y0, c.x1, c.y1) = (c.x0.min(x), c.y0.min(y), c.x1.max(x), c.y1.max(y));
            if keep {
                c.px.push(i);
                if c.px.len() > MAX_LETTER_PX {
                    keep = false;
                    c.px = Vec::new();
                }
            }
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if (dx == 0 && dy == 0) || !ink.get(nx, ny) {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if let Some(s) = seen.get_mut(j)
                        && !*s
                    {
                        *s = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(c);
    }
    out
}

/// A line of text read off the scan.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub text: String,
    /// Reading direction, degrees counter-clockwise from the image's x axis: 0, 90, 180 or 270.
    pub angle_deg: f64,
    /// The start of the baseline, image pixels with y up from the bottom edge.
    pub origin: Vec2,
    /// Cap height, pixels.
    pub height: f64,
    /// Mean match distance (grid units; a cap height is 20): lower is surer.
    pub score: f64,
    /// The components it is made of (indices into the list given to [`read_lines`]).
    pub comps: Vec<usize>,
    /// Where each character of `text` lies along the line (its frame's along axis, pixels).
    pub spans: Vec<[f64; 2]>,
    /// The baseline in the line's frame (its up axis, pixels).
    pub base: f64,
}

impl TextLine {
    /// How sure the reading is, 0–1.
    pub fn confidence(&self) -> f64 {
        (1.0 - self.score / 4.0).clamp(0.0, 1.0)
    }

    /// Characters `from..to` of the line as a line of their own (trimmed of spaces).
    pub fn part(&self, from: usize, to: usize) -> Option<TextLine> {
        let chars: Vec<char> = self.text.chars().collect();
        let (mut a, mut b) = (from.min(chars.len()), to.min(chars.len()));
        while a < b && chars.get(a) == Some(&' ') {
            a += 1;
        }
        while b > a && chars.get(b - 1) == Some(&' ') {
            b -= 1;
        }
        if a >= b {
            return None;
        }
        let spans: Vec<[f64; 2]> = self.spans.get(a..b)?.to_vec();
        let lo = spans.first()?[0];
        let o = from_frame(self.angle_deg, [lo, self.base]);
        Some(TextLine { text: chars.get(a..b)?.iter().collect(), origin: Vec2::new(o[0], o[1]), spans, comps: Vec::new(), ..self.clone() })
    }

    /// The middle of the line, image pixels (y up).
    pub fn centre(&self) -> Vec2 {
        let (lo, hi) = (self.spans.first().map_or(0.0, |s| s[0]), self.spans.last().map_or(0.0, |s| s[1]));
        let m = from_frame(self.angle_deg, [(lo + hi) / 2.0, self.base + self.height / 2.0]);
        Vec2::new(m[0], m[1])
    }

    /// Its length along the line, pixels.
    pub fn length(&self) -> f64 {
        self.spans.last().map_or(0.0, |s| s[1]) - self.spans.first().map_or(0.0, |s| s[0])
    }
}

/// What may be a letter.
#[derive(Clone, Copy, Debug)]
pub struct LetterSize {
    /// Tallest letter (cap height plus stroke), pixels.
    pub max_px: f64,
    /// Smallest blob kept (specks are smaller), pixels of ink.
    pub min_ink: usize,
    /// Shortest line of text (its tallest letter), pixels: specks in a row aren't words.
    pub min_px: f64,
}

/// A template: the glyph's strokes in the matching grid and their distance field.
struct Template {
    ch: char,
    /// Sample points along the strokes, grid units.
    pts: Vec<[f64; 2]>,
    /// Distance from each grid cell to the nearest stroke, grid units.
    dist: Vec<f32>,
}

fn templates() -> &'static [Template] {
    static T: std::sync::OnceLock<Vec<Template>> = std::sync::OnceLock::new();
    T.get_or_init(|| CHARSET.chars().filter_map(template).collect())
}

/// The glyph of `c` in the stroke font, centred in the grid, cap height [`UNIT`].
fn template(c: char) -> Option<Template> {
    let sh = cadaddy_fonts::shape_stroke(&c.to_string(), UNIT, 1.0, 0.0);
    let strokes = sh.outlines();
    let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in strokes.iter().flatten() {
        (x0, x1) = (x0.min(p.x), x1.max(p.x));
    }
    if !x0.is_finite() {
        return None;
    }
    let cx = (x0 + x1) / 2.0;
    let to_grid = |p: Vec2| [p.x - cx + GRID_W as f64 / 2.0, p.y + BELOW * UNIT];
    let mut pts = Vec::new();
    for s in &strokes {
        for seg in s.windows(2) {
            let (Some(a), Some(b)) = (seg.first(), seg.get(1)) else { continue };
            let n = ((a.dist(*b) * 3.0).ceil() as usize).clamp(1, 400);
            for k in 0..=n {
                let t = k as f64 / n as f64;
                pts.push(to_grid(*a + (*b - *a) * t));
            }
        }
        if s.len() == 1
            && let Some(a) = s.first()
        {
            pts.push(to_grid(*a));
        }
    }
    let mut on = vec![false; GRID_W * GRID_H];
    for p in &pts {
        let (x, y) = (p[0].floor(), p[1].floor());
        if x >= 0.0
            && y >= 0.0
            && (x as usize) < GRID_W
            && (y as usize) < GRID_H
            && let Some(o) = on.get_mut(y as usize * GRID_W + x as usize)
        {
            *o = true;
        }
    }
    Some(Template { ch: c, pts, dist: distance(&on, GRID_W, GRID_H) })
}

/// Chamfer distance field (3-4 weights): distance from each cell to the nearest set cell.
fn distance(on: &[bool], w: usize, h: usize) -> Vec<f32> {
    let big = 1e6f32;
    let mut d: Vec<f32> = on.iter().map(|&o| if o { 0.0 } else { big }).collect();
    let get = |d: &[f32], x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h { big } else { d.get(y as usize * w + x as usize).copied().unwrap_or(big) }
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let v = get(&d, x, y)
                .min(get(&d, x - 1, y) + 1.0)
                .min(get(&d, x, y - 1) + 1.0)
                .min(get(&d, x - 1, y - 1) + 1.4)
                .min(get(&d, x + 1, y - 1) + 1.4);
            if let Some(c) = d.get_mut(y as usize * w + x as usize) {
                *c = v;
            }
        }
    }
    for y in (0..h as i64).rev() {
        for x in (0..w as i64).rev() {
            let v = get(&d, x, y)
                .min(get(&d, x + 1, y) + 1.0)
                .min(get(&d, x, y + 1) + 1.0)
                .min(get(&d, x + 1, y + 1) + 1.4)
                .min(get(&d, x - 1, y + 1) + 1.4);
            if let Some(c) = d.get_mut(y as usize * w + x as usize) {
                *c = v;
            }
        }
    }
    d
}

fn sample(d: &[f32], p: [f64; 2]) -> f64 {
    let (x, y) = (p[0].floor(), p[1].floor());
    if x < 0.0 || y < 0.0 || x as usize >= GRID_W || y as usize >= GRID_H {
        return 8.0;
    }
    d.get(y as usize * GRID_W + x as usize).map_or(8.0, |&v| f64::from(v).min(8.0))
}

/// The frame of a line read at `angle_deg`: image pixel (x right, y up) → (along, up).
fn to_frame(angle_deg: f64, p: [f64; 2]) -> [f64; 2] {
    let (s, c) = angle_deg.to_radians().sin_cos();
    [p[0] * c + p[1] * s, -p[0] * s + p[1] * c]
}

fn from_frame(angle_deg: f64, q: [f64; 2]) -> [f64; 2] {
    let (s, c) = angle_deg.to_radians().sin_cos();
    [q[0] * c - q[1] * s, q[0] * s + q[1] * c]
}

/// A component's box in a frame (min along, min up, max along, max up), pixel edges.
fn frame_box(c: &Comp, ih: usize, angle: f64) -> [f64; 4] {
    let corners = [
        [c.x0 as f64, (ih - c.y1 - 1) as f64],
        [(c.x1 + 1) as f64, (ih - c.y1 - 1) as f64],
        [c.x0 as f64, (ih - c.y0) as f64],
        [(c.x1 + 1) as f64, (ih - c.y0) as f64],
    ];
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in corners {
        let q = to_frame(angle, p);
        b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
    }
    b
}

/// Typical stroke width of these components: the commonest short run of ink across rows and
/// columns.
pub fn stroke_width(ink: &Ink, comps: &[&Comp]) -> f64 {
    let mut hist = [0usize; 16];
    for c in comps {
        for y in c.y0..=c.y1 {
            let mut run = 0usize;
            for x in c.x0..=c.x1 + 1 {
                if x <= c.x1 && ink.get(x as i64, y as i64) {
                    run += 1;
                } else if run > 0 {
                    if let Some(hc) = hist.get_mut(run) {
                        *hc += 1;
                    }
                    run = 0;
                }
            }
        }
        for x in c.x0..=c.x1 {
            let mut run = 0usize;
            for y in c.y0..=c.y1 + 1 {
                if y <= c.y1 && ink.get(x as i64, y as i64) {
                    run += 1;
                } else if run > 0 {
                    if let Some(hc) = hist.get_mut(run) {
                        *hc += 1;
                    }
                    run = 0;
                }
            }
        }
    }
    hist.iter().enumerate().skip(1).max_by_key(|(_, n)| **n).map_or(1.0, |(i, _)| i as f64)
}

/// One character: its components and its extent along the line.
struct Glyph {
    comps: Vec<usize>,
    lo: f64,
    hi: f64,
}

/// Read one character: the best template and its distance.
fn match_glyph(ink: &Ink, comps: &[Comp], g: &Glyph, angle: f64, base: f64, cap: f64, stroke: f64) -> (char, f64) {
    let ih = ink.h;
    // Which pixels belong to this character.
    let mut mine: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &i in &g.comps {
        if let Some(c) = comps.get(i) {
            mine.extend(c.px.iter().copied());
        }
    }
    let k0 = cap / UNIT; // pixels per grid unit
    let mid = (g.lo + g.hi) / 2.0;
    let mut best = ('?', f64::INFINITY);
    // The line's cap height and baseline are estimates: try the character a little larger,
    // smaller, higher and lower, and keep each template's best fit.
    for sc in [0.93, 1.0, 1.07] {
        for dy in [-0.06, 0.0, 0.06] {
            let k = k0 * sc;
            let mut on = vec![false; GRID_W * GRID_H];
            let mut cells = Vec::new();
            for gy in 0..GRID_H {
                for gx in 0..GRID_W {
                    let along = mid + (gx as f64 + 0.5 - GRID_W as f64 / 2.0) * k;
                    let up = base + dy * cap + (gy as f64 + 0.5 - BELOW * UNIT) * k;
                    let p = from_frame(angle, [along, up]);
                    let (x, y) = (p[0].floor(), ih as f64 - p[1].floor() - 1.0);
                    if x < 0.0 || y < 0.0 || x >= ink.w as f64 || y >= ih as f64 {
                        continue;
                    }
                    if mine.contains(&(y as usize * ink.w + x as usize)) {
                        if let Some(o) = on.get_mut(gy * GRID_W + gx) {
                            *o = true;
                        }
                        cells.push([gx as f64 + 0.5, gy as f64 + 0.5]);
                    }
                }
            }
            if cells.is_empty() {
                continue;
            }
            let dist = distance(&on, GRID_W, GRID_H);
            // Thick ink lies up to half a stroke from the template's centre line.
            let half = (stroke / k / 2.0 - 0.5).max(0.0);
            for t in templates() {
                if t.pts.is_empty() {
                    continue;
                }
                let a = t.pts.iter().map(|p| sample(&dist, *p)).sum::<f64>() / t.pts.len() as f64;
                let b = cells.iter().map(|p| (sample(&t.dist, *p) - half).max(0.0)).sum::<f64>() / cells.len() as f64;
                // A larger or smaller fit than the line's own is a little less likely.
                let s = 0.5 * (a + b) + 0.5 * a.max(b) + 0.3 * (sc - 1.0).abs() / 0.07 * 0.1 + dy.abs();
                if s < best.1 {
                    best = (t.ch, s);
                }
            }
        }
    }
    best
}

/// Read a group of components as one line at `angle`: the text, the mean distance, the origin
/// and the cap height.
fn read_as(ink: &Ink, comps: &[Comp], members: &[usize], angle: f64) -> Option<Read> {
    let ih = ink.h;
    let mut boxes: Vec<(usize, [f64; 4])> = members.iter().filter_map(|&i| comps.get(i).map(|c| (i, frame_box(c, ih, angle)))).collect();
    if boxes.is_empty() || boxes.len() > MAX_LINE_CHARS * 2 {
        return None;
    }
    boxes.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
    let refs: Vec<&Comp> = members.iter().filter_map(|&i| comps.get(i)).collect();
    let stroke = stroke_width(ink, &refs);
    // Cap height and baseline from the tall characters.
    let tallest = boxes.iter().map(|(_, b)| b[3] - b[1]).fold(0.0, f64::max);
    let mut tall: Vec<&[f64; 4]> = boxes.iter().map(|(_, b)| b).filter(|b| b[3] - b[1] >= 0.7 * tallest).collect();
    tall.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let bottom = tall.get(tall.len() / 2).map_or(0.0, |b| b[1]);
    tall.sort_by(|a, b| a[3].total_cmp(&b[3]));
    let top = tall.get(tall.len() / 2).map_or(0.0, |b| b[3]);
    let cap = (top - bottom - stroke).max(2.0);
    let base = bottom + stroke / 2.0;
    // Characters: components overlapping along the line belong together.
    let mut glyphs: Vec<Glyph> = Vec::new();
    for (i, b) in &boxes {
        if let Some(g) = glyphs.last_mut() {
            let ov = g.hi.min(b[2]) - g.lo.max(b[0]);
            let narrow = (g.hi - g.lo).min(b[2] - b[0]).max(1.0);
            if ov > 0.3 * narrow || (b[0] + b[2]) / 2.0 < g.hi {
                g.comps.push(*i);
                g.hi = g.hi.max(b[2]);
                g.lo = g.lo.min(b[0]);
                continue;
            }
        }
        glyphs.push(Glyph { comps: vec![*i], lo: b[0], hi: b[2] });
    }
    if glyphs.len() > MAX_LINE_CHARS {
        return None;
    }
    let mut chars: Vec<(char, [f64; 2])> = Vec::new();
    let mut total = 0.0;
    for g in &glyphs {
        let (c, sc) = match_glyph(ink, comps, g, angle, base, cap, stroke);
        total += sc;
        if let Some(&(p, ps)) = chars.last() {
            let gap = g.lo - ps[1];
            if gap > 0.55 * cap {
                chars.push((' ', [ps[1], g.lo]));
            } else if p == '\'' && c == '\'' {
                // Two ticks side by side: a double quote.
                if let Some(last) = chars.last_mut() {
                    *last = ('"', [ps[0], g.hi]);
                }
                continue;
            }
        }
        chars.push((c, [g.lo, g.hi]));
    }
    let text: String = chars.iter().map(|c| c.0).collect();
    let text = lexicon(&letters_and_digits(&text));
    let spans: Vec<[f64; 2]> = chars.iter().map(|c| c.1).collect();
    let first = glyphs.first().map_or(0.0, |g| g.lo + stroke / 2.0);
    let o = from_frame(angle, [first, base]);
    Some(Read { text, score: total / glyphs.len().max(1) as f64, origin: Vec2::new(o[0], o[1]), cap, spans, base })
}

/// One reading of a line.
struct Read {
    text: String,
    score: f64,
    origin: Vec2,
    cap: f64,
    spans: Vec<[f64; 2]>,
    base: f64,
}

/// The stroke font draws `O` and `0` alike: a zero among digits and marks (`10'-0"`), a letter
/// among letters (`TOD`).
fn letters_and_digits(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let numeric = |c: Option<&char>| c.is_some_and(|c| c.is_ascii_digit() || "'\"-./x@\u{d8}".contains(*c));
    let letter = |c: Option<&char>| c.is_some_and(|c| c.is_ascii_uppercase() && *c != 'O');
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if c != 'O' {
                return c;
            }
            let (p, n) = (i.checked_sub(1).and_then(|j| chars.get(j)), chars.get(i + 1));
            if (numeric(p) || numeric(n)) && !letter(p) && !letter(n) { '0' } else { c }
        })
        .collect()
}

/// Words drafted on MEP prints, for telling apart letters the stroke font draws nearly alike.
const WORDS: &[&str] = &[
    "ABOVE",
    "AFF",
    "AHU",
    "AIR",
    "BELOW",
    "BOD",
    "BOP",
    "BOTTOM",
    "CEILING",
    "CENTERLINE",
    "CL",
    "COLD",
    "CW",
    "DEPTH",
    "DIA",
    "DIFFUSER",
    "DN",
    "DOMESTIC",
    "DOWN",
    "DROP",
    "DUCT",
    "EXHAUST",
    "FINISHED",
    "FIRE",
    "FLOOR",
    "FROM",
    "GAS",
    "GRILLE",
    "HANDLING",
    "HEIGHT",
    "HOT",
    "HW",
    "HWR",
    "LEVEL",
    "MECHANICAL",
    "OF",
    "OUTSIDE",
    "PIPE",
    "PIPING",
    "PLAN",
    "PLUMBING",
    "PROTECTION",
    "RETURN",
    "RISE",
    "RISER",
    "ROUND",
    "SAN",
    "SANITARY",
    "SCALE",
    "SEWER",
    "SITE",
    "SIZE",
    "SUP",
    "SUPPLY",
    "TERMINAL",
    "TO",
    "TOD",
    "TOP",
    "TYPE",
    "UNIT",
    "UP",
    "VARIABLE",
    "VAV",
    "VENT",
    "VOLUME",
    "WATER",
    "WIDTH",
];

/// Letters the reader mixes up, both ways.
const CONFUSABLE: &[(char, char)] = &[('O', 'D'), ('8', 'B'), ('1', 'I'), ('5', 'S'), ('2', 'Z'), ('0', 'D'), ('E', 'F'), ('U', 'D'), ('Y', 'V')];

/// A word one or two confusable letters away from a known drafting word is that word.
fn lexicon(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let core: String = w.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            if core.len() < 2 || WORDS.contains(&core.as_str()) || !core.chars().any(|c| c.is_ascii_uppercase()) {
                return w.to_string();
            }
            let alts = |c: char| -> Vec<char> {
                CONFUSABLE
                    .iter()
                    .filter_map(|&(a, b)| {
                        if a == c {
                            Some(b)
                        } else if b == c {
                            Some(a)
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            let chars: Vec<char> = w.chars().collect();
            let fits = |cand: &[char]| {
                let core: String = cand.iter().filter(|c| c.is_ascii_alphanumeric()).collect();
                WORDS.contains(&core.as_str())
            };
            for i in 0..chars.len() {
                for a in alts(chars.get(i).copied().unwrap_or(' ')) {
                    let mut c1 = chars.clone();
                    if let Some(x) = c1.get_mut(i) {
                        *x = a;
                    }
                    if fits(&c1) {
                        return c1.iter().collect();
                    }
                    for j in i + 1..chars.len() {
                        for b in alts(chars.get(j).copied().unwrap_or(' ')) {
                            let mut c2 = c1.clone();
                            if let Some(x) = c2.get_mut(j) {
                                *x = b;
                            }
                            if fits(&c2) {
                                return c2.iter().collect();
                            }
                        }
                    }
                }
            }
            w.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Union-find root.
fn root(p: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while let Some(&q) = p.get(r) {
        if q == r {
            break;
        }
        r = q;
    }
    let mut j = i;
    while let Some(&q) = p.get(j) {
        if q == r {
            break;
        }
        if let Some(pj) = p.get_mut(j) {
            *pj = r;
        }
        j = q;
    }
    r
}

/// Groups of letter-sized components that sit side by side along x (`vertical` false) or y.
fn group(comps: &[Comp], cand: &[usize], vertical: bool) -> Vec<Vec<usize>> {
    let n = cand.len();
    let mut parent: Vec<usize> = (0..n).collect();
    // (along lo, along hi, across lo, across hi)
    let span = |c: &Comp| -> [f64; 4] {
        if vertical {
            [c.y0 as f64, c.y1 as f64 + 1.0, c.x0 as f64, c.x1 as f64 + 1.0]
        } else {
            [c.x0 as f64, c.x1 as f64 + 1.0, c.y0 as f64, c.y1 as f64 + 1.0]
        }
    };
    let spans: Vec<[f64; 4]> = cand.iter().map(|&i| comps.get(i).map_or([0.0; 4], span)).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| spans.get(a).map_or(0.0, |s| s[0]).total_cmp(&spans.get(b).map_or(0.0, |s| s[0])));
    let tallest = spans.iter().map(|s| s[3] - s[2]).fold(0.0, f64::max);
    for (k, &a) in order.iter().enumerate() {
        let Some(sa) = spans.get(a).copied() else { continue };
        for &b in order.iter().skip(k + 1) {
            let Some(sb) = spans.get(b).copied() else { continue };
            if sb[0] - sa[1] > tallest {
                break;
            }
            let (ha, hb) = (sa[3] - sa[2], sb[3] - sb[2]);
            let h = ha.max(hb);
            let gap = sb[0] - sa[1];
            if gap > 1.15 * h {
                continue;
            }
            let ov = sa[3].min(sb[3]) - sa[2].max(sb[2]);
            let small = ha.min(hb);
            // Side by side on one line: across-extents overlap well, or a small mark (a dash,
            // a quote, a dot) lies within the other's extent.
            let inside = (ha < hb && sa[2] >= sb[2] - 1.0 && sa[3] <= sb[3] + 1.0) || (hb < ha && sb[2] >= sa[2] - 1.0 && sb[3] <= sa[3] + 1.0);
            let similar = ov >= 0.6 * small && small >= 0.5 * h;
            if similar || (inside && small < 0.6 * h) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if let Some(p) = parent.get_mut(ra) {
                    *p = rb;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for k in 0..n {
        let r = root(&mut parent, k);
        if let Some(&ci) = cand.get(k) {
            groups.entry(r).or_default().push(ci);
        }
    }
    groups.into_values().collect()
}

fn skew_deg_pair(skew: f64) -> [f64; 2] {
    [skew, 180.0 + skew]
}

/// Whether a component may be (part of) a letter.
pub fn letter_like(c: &Comp, size: &LetterSize) -> bool {
    let (w, h) = (c.width() as f64, c.height() as f64);
    !c.px.is_empty() && c.px.len() >= size.min_ink && w <= size.max_px * 1.3 && h <= size.max_px * 1.3
}

/// Read every line of text among `comps`: the letter-sized ones that group into lines and match
/// the font well. Components enclosing others (bubbles, boxes) are never letters.
pub fn read_lines(ink: &Ink, comps: &[Comp], size: &LetterSize, skew_deg: f64) -> Vec<TextLine> {
    let mut cand: Vec<usize> = (0..comps.len()).filter(|&i| comps.get(i).is_some_and(|c| letter_like(c, size))).collect();
    // A frame around another letter-sized blob (a grid bubble round its label) isn't a letter.
    let enclosing: std::collections::HashSet<usize> = {
        let mut by_x: Vec<usize> = cand.clone();
        by_x.sort_by_key(|&i| comps.get(i).map_or(0, |c| c.x0));
        let mut out = std::collections::HashSet::new();
        for (k, &a) in by_x.iter().enumerate() {
            let Some(ca) = comps.get(a) else { continue };
            for &b in by_x.iter().skip(k + 1) {
                let Some(cb) = comps.get(b) else { continue };
                if cb.x0 > ca.x1 {
                    break;
                }
                if cb.x0 > ca.x0 && cb.x1 < ca.x1 && cb.y0 > ca.y0 && cb.y1 < ca.y1 && cb.width() * 2 < ca.width() {
                    out.insert(a);
                }
            }
        }
        out
    };
    cand.retain(|i| !enclosing.contains(i));
    // Lines both ways; the larger groups claim their components first.
    let mut groups: Vec<(bool, Vec<usize>)> = group(comps, &cand, false).into_iter().map(|g| (false, g)).collect();
    groups.extend(group(comps, &cand, true).into_iter().map(|g| (true, g)));
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut claimed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (vertical, g) in groups {
        let members: Vec<usize> = g.into_iter().filter(|i| !claimed.contains(i)).collect();
        if members.is_empty() {
            continue;
        }
        let angles: [f64; 2] = if vertical { [90.0 + skew_deg, 270.0 + skew_deg] } else { skew_deg_pair(skew_deg) };
        let mut best: Option<(Read, f64, f64)> = None;
        for a in angles {
            // Prints are lettered to read from the bottom or the right: upside-down readings must
            // be clearly better (many letters look alike either way up).
            let bias = if (a - skew_deg).rem_euclid(360.0) >= 135.0 { 0.15 } else { 0.0 };
            if let Some(r) = read_as(ink, comps, &members, a)
                && best.as_ref().is_none_or(|b| r.score + bias < b.0.score + b.2)
            {
                best = Some((r, a, bias));
            }
        }
        let Some((r, angle_deg, _)) = best else { continue };
        if r.cap < size.min_px {
            continue;
        }
        let known = r.text.chars().filter(|c| *c != '?' && *c != ' ').count();
        let all = r.text.chars().filter(|c| *c != ' ').count().max(1);
        // A single blob must read very well to be taken for a letter (symbols are blobs too).
        let limit = if all == 1 { 1.6 } else { 2.6 };
        if r.score > limit || known * 4 < all * 3 {
            continue;
        }
        // Single upright marks that are everywhere in linework aren't text on their own.
        if all == 1 && r.text.chars().any(|c| "-'\".,/()=:".contains(c)) {
            continue;
        }
        claimed.extend(members.iter().copied());
        out.push(TextLine { text: r.text, angle_deg, origin: r.origin, height: r.cap, score: r.score, comps: members, spans: r.spans, base: r.base });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s` drawn in the stroke font at `cap` pixels, turned `angle` degrees, stroke `pen` px.
    fn ink_of(s: &str, cap: f64, angle: f64, pen: f64) -> Ink {
        let sh = cadaddy_fonts::shape_stroke(s, cap, 1.0, 0.0);
        let (w, h) = (((sh.width + 4.0 * cap) * 1.5) as usize, ((sh.width + 4.0 * cap) * 1.5) as usize);
        let mut ink = Ink { w, h, on: vec![false; w * h] };
        let c = [w as f64 / 2.0 - sh.width / 2.0, h as f64 / 2.0 - cap / 2.0];
        let (sn, cs) = angle.to_radians().sin_cos();
        let place = |p: Vec2| {
            let (x, y) = (p.x - sh.width / 2.0, p.y - cap / 2.0);
            [x * cs - y * sn + c[0] + sh.width / 2.0, x * sn + y * cs + c[1] + cap / 2.0]
        };
        for st in sh.outlines() {
            for seg in st.windows(2) {
                let (a, b) = (place(seg[0]), place(seg[1]));
                let n = ((a[0] - b[0]).hypot(a[1] - b[1]) * 4.0).ceil().max(1.0) as usize;
                for k in 0..=n {
                    let t = k as f64 / n as f64;
                    let (px, py) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
                    let r = pen / 2.0;
                    let mut yy = (py - r).floor();
                    while yy <= py + r {
                        let mut xx = (px - r).floor();
                        while xx <= px + r {
                            if (xx + 0.5 - px).hypot(yy + 0.5 - py) <= r.max(0.5) && xx >= 0.0 && yy >= 0.0 && (xx as usize) < w && (yy as usize) < h
                            {
                                ink.on[(h - 1 - yy as usize) * w + xx as usize] = true;
                            }
                            xx += 1.0;
                        }
                        yy += 1.0;
                    }
                }
            }
        }
        ink
    }

    fn read(s: &str, cap: f64, angle: f64, pen: f64) -> Vec<TextLine> {
        let ink = ink_of(s, cap, angle, pen);
        let comps = components(&ink);
        read_lines(&ink, &comps, &LetterSize { max_px: cap * 1.6, min_ink: 2, min_px: 4.0 }, 0.0)
    }

    #[test]
    fn reads_tags_upright() {
        for s in ["24x14", "TOD @ 10'-10\"", "12\" DIA", "VAV-1-03", "CD-3", "SUP = SUPPLY AIR", "4\" SAN", "1-1/4\" CW UP"] {
            let lines = read(s, 16.0, 0.0, 2.0);
            assert_eq!(lines.len(), 1, "{s}: {lines:?}");
            assert_eq!(lines[0].text, s, "{lines:?}");
            assert_eq!(lines[0].angle_deg, 0.0);
        }
    }

    #[test]
    fn reads_turned_text() {
        for (s, a) in [("14\" DIA", 90.0), ("TOD @ 10'-9\"", 90.0), ("RISER UP", 270.0), ("48x14", 180.0)] {
            let lines = read(s, 16.0, a, 2.0);
            assert_eq!(lines.len(), 1, "{s}: {lines:?}");
            assert_eq!(lines[0].text, s);
            assert_eq!(lines[0].angle_deg, a);
        }
    }

    #[test]
    fn small_text_still_reads() {
        let lines = read("12\"\u{d8} 36x18", 11.0, 0.0, 1.5);
        assert_eq!(lines.first().map(|l| l.text.as_str()), Some("12\"\u{d8} 36x18"), "{lines:?}");
    }

    #[test]
    fn a_box_is_not_a_letter() {
        let (w, h) = (60, 60);
        let mut ink = Ink { w, h, on: vec![false; w * h] };
        for i in 10..40 {
            for (x, y) in [(i, 10), (i, 39), (10, i), (39, i), (i, i), (i, 49 - i)] {
                ink.on[y * w + x] = true;
            }
        }
        let comps = components(&ink);
        assert!(read_lines(&ink, &comps, &LetterSize { max_px: 40.0, min_ink: 2, min_px: 4.0 }, 0.0).is_empty());
    }

    #[test]
    fn empty_and_huge_inputs_do_not_panic() {
        let ink = Ink::default();
        assert!(components(&ink).is_empty());
        let ink = Ink { w: 10, h: 10, on: vec![true; 3] };
        let comps = components(&ink);
        let _ = read_lines(&ink, &comps, &LetterSize { max_px: 5.0, min_ink: 1, min_px: 1.0 }, 0.0);
    }
}
