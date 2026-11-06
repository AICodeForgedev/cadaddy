//! Lettering a PDF draws as shapes rather than as text the reader can decode: the names of a
//! font's glyphs (a Type3 or simple font's `/Differences` without a ToUnicode map), and the
//! centre lines of filled glyph outlines (text converted to outlines, a Type3 glyph procedure
//! that fills its glyph), which the stroke lettering OCR in `pdf_read` reads like plotted
//! single-stroke lettering.

use std::collections::HashSet;

/// The text a glyph name stands for: the Adobe Glyph List names of the characters a drawing
/// prints (`zero`, `A`, `parenleft`, `uni0041`, `u1F600`), a variant suffix (`one.lf`) or a
/// ligature (`f_i`) included; `None` for a name that says nothing (`g12`, `.notdef`, `cid7`).
pub(crate) fn glyph_name_text(name: &str) -> Option<String> {
    let base = name.split('.').next().unwrap_or("");
    if base.is_empty() {
        return None;
    }
    if base.contains('_') {
        let parts: Option<Vec<String>> = base.split('_').map(glyph_name_text).collect();
        return parts.map(|p| p.concat()).filter(|s| !s.is_empty());
    }
    let mut chars = base.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphabetic()
    {
        return Some(c.to_string());
    }
    if let Some(hex) = base.strip_prefix("uni")
        && !hex.is_empty()
        && hex.len() % 4 == 0
        && hex.len() <= 32
    {
        let units: Option<String> = hex
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).ok().and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32))
            .collect();
        return units;
    }
    if let Some(hex) = base.strip_prefix('u')
        && (4..=6).contains(&hex.len())
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(String::from);
    }
    GLYPH_NAMES.iter().find(|(n, _)| *n == base).map(|(_, c)| c.to_string())
}

/// Adobe Glyph List names of the digits, punctuation and signs drawings print.
const GLYPH_NAMES: [(&str, char); 64] = [
    ("zero", '0'),
    ("one", '1'),
    ("two", '2'),
    ("three", '3'),
    ("four", '4'),
    ("five", '5'),
    ("six", '6'),
    ("seven", '7'),
    ("eight", '8'),
    ("nine", '9'),
    ("space", ' '),
    ("nbspace", ' '),
    ("exclam", '!'),
    ("quotedbl", '"'),
    ("numbersign", '#'),
    ("dollar", '$'),
    ("percent", '%'),
    ("ampersand", '&'),
    ("quotesingle", '\''),
    ("quoteright", '\u{2019}'),
    ("quoteleft", '\u{2018}'),
    ("quotedblleft", '\u{201C}'),
    ("quotedblright", '\u{201D}'),
    ("parenleft", '('),
    ("parenright", ')'),
    ("asterisk", '*'),
    ("plus", '+'),
    ("comma", ','),
    ("hyphen", '-'),
    ("minus", '\u{2212}'),
    ("period", '.'),
    ("slash", '/'),
    ("colon", ':'),
    ("semicolon", ';'),
    ("less", '<'),
    ("equal", '='),
    ("greater", '>'),
    ("question", '?'),
    ("at", '@'),
    ("bracketleft", '['),
    ("backslash", '\\'),
    ("bracketright", ']'),
    ("asciicircum", '^'),
    ("underscore", '_'),
    ("grave", '`'),
    ("braceleft", '{'),
    ("bar", '|'),
    ("braceright", '}'),
    ("asciitilde", '~'),
    ("degree", '\u{B0}'),
    ("plusminus", '\u{B1}'),
    ("multiply", '\u{D7}'),
    ("divide", '\u{F7}'),
    ("endash", '\u{2013}'),
    ("emdash", '\u{2014}'),
    ("bullet", '\u{2022}'),
    ("periodcentered", '\u{B7}'),
    ("onehalf", '\u{BD}'),
    ("onequarter", '\u{BC}'),
    ("threequarters", '\u{BE}'),
    ("section", '\u{A7}'),
    ("paragraph", '\u{B6}'),
    ("copyright", '\u{A9}'),
    ("registered", '\u{AE}'),
];

/// Pixels on a glyph's longer side when its outline is rasterised for thinning: a capital's
/// stem (an eighth of its height and more) is a few pixels wide.
const RASTER: f64 = 40.0;
/// Empty pixels round the glyph, so thinning sees its edges.
const MARGIN: usize = 2;
/// Thinning passes at most (each peels one pixel off every side).
const MAX_PASSES: usize = 64;
/// Most points a centre line keeps (the stroke OCR takes up to 64 per stroke).
const MAX_LINE_POINTS: usize = 60;

/// A filled glyph outline's centre lines in the outline's own units.
pub(crate) struct Skeleton {
    /// The centre lines, and whether each is a closed ring (the bowl of an O, a D, a 0).
    pub lines: Vec<(Vec<(f64, f64)>, bool)>,
    /// The share of the glyph's box the outline fills (0..1): a solid square fills all of it,
    /// lettering a fifth to a half.
    pub ink: f64,
    /// The holes in the shape (an O has one, a B two, an L none).
    pub holes: usize,
    /// How thick its strokes are (its ink over its centre lines' length), as a share of the
    /// glyph's longer side: lettering a tenth or so, a solid triangle or square a half.
    pub width: f64,
}

/// How many holes a character's shape has in the sans faces drawings use (`None`: no rule).
/// A `4` is open or closed by face, an `&` has one or two.
pub(crate) fn holes_fit(ch: char, holes: usize) -> bool {
    match ch {
        'B' | '8' => holes == 2,
        'A' | 'D' | 'O' | 'P' | 'Q' | 'R' | '0' | '6' | '9' | '#' => holes == 1,
        '4' => holes <= 1,
        '&' => (1..=2).contains(&holes),
        _ => holes == 0,
    }
}

/// The background's pieces the shape encloses (4-connected): its holes.
fn count_holes(g: &Grid) -> usize {
    let mut seen = vec![false; g.w * g.h];
    let mut pieces = 0usize;
    for start in 0..g.w * g.h {
        if seen.get(start).copied().unwrap_or(true) || g.px.get(start).copied().unwrap_or(true) {
            continue;
        }
        pieces += 1;
        let mut stack = vec![start];
        if let Some(v) = seen.get_mut(start) {
            *v = true;
        }
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % g.w) as i64, (i / g.w) as i64);
            for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx as usize >= g.w || ny as usize >= g.h {
                    continue;
                }
                let j = ny as usize * g.w + nx as usize;
                if !g.at(nx, ny) && seen.get(j) == Some(&false) {
                    if let Some(v) = seen.get_mut(j) {
                        *v = true;
                    }
                    stack.push(j);
                }
            }
        }
    }
    // the margin keeps the outside one piece
    pieces.saturating_sub(1)
}

struct Grid {
    w: usize,
    h: usize,
    px: Vec<bool>,
}

impl Grid {
    fn at(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        let (x, y) = (x as usize, y as usize);
        x < self.w && y < self.h && self.px.get(y * self.w + x).copied().unwrap_or(false)
    }
    fn set(&mut self, x: usize, y: usize, v: bool) {
        if x < self.w
            && let Some(p) = self.px.get_mut(y * self.w + x)
        {
            *p = v;
        }
    }
}

/// The 8 neighbours from north round clockwise (y up): P2..P9 of the Zhang–Suen thinning.
const RING: [(i64, i64); 8] = [(0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1)];

/// The centre lines of a filled outline (`loops`, filled by the non-zero rule or `even_odd`):
/// rasterised, thinned to one pixel (Zhang–Suen) and traced from end to end and junction to
/// junction, with the short spurs thinning leaves at corners cut off. `None` for an empty or
/// degenerate outline.
pub(crate) fn skeleton(loops: &[&[(f64, f64)]], even_odd: bool) -> Option<Skeleton> {
    if loops.iter().flat_map(|l| l.iter()).any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return None;
    }
    let pts = loops.iter().flat_map(|l| l.iter());
    let (x0, y0, x1, y1) = pts.fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, &(x, y)| (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y)));
    let long = (x1 - x0).max(y1 - y0);
    if !long.is_finite() || long <= 1e-9 {
        return None;
    }
    let scale = RASTER / long;
    let gw = ((x1 - x0) * scale).ceil() as usize + 2 * MARGIN;
    let gh = ((y1 - y0) * scale).ceil() as usize + 2 * MARGIN;
    let mut g = Grid { w: gw, h: gh, px: vec![false; gw * gh] };
    // pixel centre ↔ outline units
    let ux = |px: f64| x0 + (px + 0.5 - MARGIN as f64) / scale;
    let uy = |py: f64| y0 + (py + 0.5 - MARGIN as f64) / scale;
    let col = |x: f64| (x - x0) * scale + MARGIN as f64 - 0.5;
    let mut ink = 0usize;
    for row in 0..gh {
        let yc = uy(row as f64);
        let mut cross: Vec<(f64, i32)> = Vec::new();
        for l in loops {
            let n = l.len();
            for i in 0..n {
                let (Some(&p), Some(&q)) = (l.get(i), l.get((i + 1) % n)) else { continue };
                if (p.1 <= yc) != (q.1 <= yc) {
                    let x = p.0 + (yc - p.1) * (q.0 - p.0) / (q.1 - p.1);
                    cross.push((x, if q.1 > p.1 { 1 } else { -1 }));
                }
            }
        }
        cross.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut wind = 0i32;
        for (i, &(x, dir)) in cross.iter().enumerate() {
            wind += if even_odd { 1 } else { dir };
            let inside = if even_odd { wind % 2 != 0 } else { wind != 0 };
            let Some(&(next, _)) = cross.get(i + 1) else { break };
            if !inside {
                continue;
            }
            let (c0, c1) = (col(x).ceil().max(0.0) as usize, col(next).floor().min(gw as f64 - 1.0));
            if c1 < 0.0 {
                continue;
            }
            for c in c0..=(c1 as usize) {
                if !g.at(c as i64, row as i64) {
                    ink += 1;
                }
                g.set(c, row, true);
            }
        }
    }
    let inner = (gw - 2 * MARGIN).max(1) * (gh - 2 * MARGIN).max(1);
    if ink == 0 {
        return None;
    }
    let holes = count_holes(&g);
    thin(&mut g);
    // the stroke width w with ink = w × (centre length + w): thinning eats half a width off
    // each end of a stroke, which matters for a short thick one (a bold I)
    let centre = g.px.iter().filter(|p| **p).count() as f64;
    let width = ((centre * centre + 4.0 * ink as f64).sqrt() - centre) / 2.0 / RASTER;
    let lines = trace(&g, ink)
        .into_iter()
        .map(|(path, closed)| {
            let pts: Vec<(f64, f64)> = path.iter().map(|&(x, y)| (ux(x as f64), uy(y as f64))).collect();
            (simplify(&pts, 0.6 / scale), closed)
        })
        .collect();
    Some(Skeleton { lines, ink: (ink as f64 / inner as f64).min(1.0), holes, width })
}

/// Zhang–Suen thinning: peel boundary pixels that don't break the shape until one pixel is left.
fn thin(g: &mut Grid) {
    for _ in 0..MAX_PASSES {
        let mut changed = false;
        for step in 0..2 {
            let mut gone: Vec<(usize, usize)> = Vec::new();
            for y in 0..g.h {
                for x in 0..g.w {
                    let (xi, yi) = (x as i64, y as i64);
                    if !g.at(xi, yi) {
                        continue;
                    }
                    let p: Vec<bool> = RING.iter().map(|&(dx, dy)| g.at(xi + dx, yi + dy)).collect();
                    let on = |i: usize| p.get(i).copied().unwrap_or(false);
                    let b = p.iter().filter(|v| **v).count();
                    let a = (0..8).filter(|&i| !on(i) && on((i + 1) % 8)).count();
                    // P2 N, P4 E, P6 S, P8 W: the first step needs P2·P4·P6 = P4·P6·P8 = 0, the
                    // second P2·P4·P8 = P2·P6·P8 = 0
                    let (n, e, s, w) = (on(0), on(2), on(4), on(6));
                    let ok = if step == 0 { !e || !s || (!n && !w) } else { !n || !w || (!e && !s) };
                    if (2..=6).contains(&b) && a == 1 && ok {
                        gone.push((x, y));
                    }
                }
            }
            changed |= !gone.is_empty();
            for (x, y) in gone {
                g.set(x, y, false);
            }
        }
        if !changed {
            break;
        }
    }
}

/// A skeleton pixel's neighbours, mixed adjacency: the 4 beside it, and a diagonal one only
/// when no side neighbour already links the two (a staircase is a line, not a row of triangles).
fn neighbours(g: &Grid, x: i64, y: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
        if g.at(x + dx, y + dy) {
            out.push((x + dx, y + dy));
        }
    }
    for (dx, dy) in [(1, 1), (1, -1), (-1, -1), (-1, 1)] {
        if g.at(x + dx, y + dy) && !g.at(x + dx, y) && !g.at(x, y + dy) {
            out.push((x + dx, y + dy));
        }
    }
    out
}

/// The skeleton's lines in pixels: from each end or junction to the next, then the rings left
/// (an O has no end). Spurs (an end to a junction, shorter than half the stroke's width: the
/// corners thinning rounds off) are cut.
fn trace(g: &Grid, ink: usize) -> Vec<(Vec<(i64, i64)>, bool)> {
    let mut on: Vec<(i64, i64)> = Vec::new();
    for y in 0..g.h as i64 {
        for x in 0..g.w as i64 {
            if g.at(x, y) {
                on.push((x, y));
            }
        }
    }
    if on.is_empty() {
        return Vec::new();
    }
    let degree = |p: (i64, i64)| neighbours(g, p.0, p.1).len();
    let key = |a: (i64, i64), b: (i64, i64)| if a <= b { (a, b) } else { (b, a) };
    let mut walked: HashSet<((i64, i64), (i64, i64))> = HashSet::new();
    let mut paths: Vec<(Vec<(i64, i64)>, bool)> = Vec::new();
    let cap = on.len() + 2;
    let walk = |start: (i64, i64), first: (i64, i64), walked: &mut HashSet<((i64, i64), (i64, i64))>| -> (Vec<(i64, i64)>, bool) {
        let mut path = vec![start, first];
        walked.insert(key(start, first));
        let (mut prev, mut cur) = (start, first);
        while degree(cur) == 2 && path.len() <= cap {
            let Some(next) = neighbours(g, cur.0, cur.1).into_iter().find(|&q| q != prev) else { break };
            if !walked.insert(key(cur, next)) {
                break;
            }
            path.push(next);
            (prev, cur) = (cur, next);
            if cur == start {
                return (path, true);
            }
        }
        (path, false)
    };
    for &p in &on {
        if degree(p) == 2 {
            continue;
        }
        for q in neighbours(g, p.0, p.1) {
            if walked.contains(&key(p, q)) {
                continue;
            }
            paths.push(walk(p, q, &mut walked));
        }
    }
    for &p in &on {
        if degree(p) != 2 {
            continue;
        }
        let Some(q) = neighbours(g, p.0, p.1).into_iter().find(|&q| !walked.contains(&key(p, q))) else { continue };
        paths.push(walk(p, q, &mut walked));
    }
    // the stroke's width in pixels: its ink over its centre line
    let width = ink as f64 / on.len() as f64;
    let spur = |path: &[(i64, i64)]| -> bool {
        let (Some(&a), Some(&b)) = (path.first(), path.last()) else { return true };
        let (da, db) = (degree(a), degree(b));
        let end_to_junction = (da == 1 && db >= 3) || (db == 1 && da >= 3);
        (end_to_junction && (path.len() as f64) < (width * 0.5).max(2.0)) || path.len() < 2
    };
    paths.retain(|(path, closed)| if *closed { path.len() >= 4 } else { !spur(path) });
    // a lone dot (a speck the thinning left)
    if paths.is_empty()
        && let Some(&p) = on.first()
    {
        paths.push((vec![p], false));
    }
    paths
}

/// Ramer–Douglas–Peucker: the points of `pts` that keep it within `tol`, at most
/// [`MAX_LINE_POINTS`] (the tolerance grows until it fits).
fn simplify(pts: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    let mut tol = tol.max(1e-9);
    for _ in 0..16 {
        let mut keep = vec![false; pts.len()];
        if let Some(k) = keep.first_mut() {
            *k = true;
        }
        if let Some(k) = keep.last_mut() {
            *k = true;
        }
        let mut stack = vec![(0usize, pts.len().saturating_sub(1))];
        while let Some((a, b)) = stack.pop() {
            if b <= a + 1 {
                continue;
            }
            let (Some(&pa), Some(&pb)) = (pts.get(a), pts.get(b)) else { continue };
            let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
            let len = dx.hypot(dy);
            let dist = |p: (f64, f64)| if len < 1e-12 { (p.0 - pa.0).hypot(p.1 - pa.1) } else { ((p.0 - pa.0) * dy - (p.1 - pa.1) * dx).abs() / len };
            let far = (a + 1..b).filter_map(|i| pts.get(i).map(|&p| (i, dist(p)))).max_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((i, d)) = far
                && d > tol
            {
                if let Some(k) = keep.get_mut(i) {
                    *k = true;
                }
                stack.push((a, i));
                stack.push((i, b));
            }
        }
        let out: Vec<(f64, f64)> = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
        if out.len() <= MAX_LINE_POINTS {
            return out;
        }
        tol *= 1.5;
    }
    pts.iter().step_by(pts.len().div_ceil(MAX_LINE_POINTS).max(1)).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    #[test]
    fn glyph_names_read_as_their_characters() {
        let read = |n: &str| glyph_name_text(n);
        assert_eq!(read("two").as_deref(), Some("2"));
        assert_eq!(read("A").as_deref(), Some("A"));
        assert_eq!(read("parenleft").as_deref(), Some("("));
        assert_eq!(read("uni0042").as_deref(), Some("B"));
        assert_eq!(read("uni00410042").as_deref(), Some("AB"));
        assert_eq!(read("u0043").as_deref(), Some("C"));
        assert_eq!(read("one.lf").as_deref(), Some("1"));
        assert_eq!(read("f_i").as_deref(), Some("fi"));
        for none in ["g12", ".notdef", "cid7", "a3", "uniZZZZ", ""] {
            assert_eq!(read(none), None, "{none}");
        }
    }

    #[test]
    fn a_bar_thins_to_its_centre_line() {
        let bar = rect(0.0, 0.0, 10.0, 100.0);
        let sk = skeleton(&[&bar], false).expect("skeleton");
        assert_eq!(sk.lines.len(), 1, "{:?}", sk.lines);
        let (line, closed) = sk.lines.first().expect("line");
        assert!(!closed);
        for &(x, _) in line {
            assert!((x - 5.0).abs() < 2.5, "off centre: {line:?}");
        }
        let ys: Vec<f64> = line.iter().map(|p| p.1).collect();
        let span = ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
        assert!(span > 80.0, "most of the bar's length: {line:?}");
        assert!(sk.ink > 0.9);
        assert!((sk.width - 0.1).abs() < 0.03, "a tenth as wide as long: {}", sk.width);
        // a solid triangle is about as thick as it is big
        let tri = [(0.0, 0.0), (90.0, 40.0), (0.0, 80.0)];
        assert!(skeleton(&[&tri], false).expect("skeleton").width > 0.3);
    }

    #[test]
    fn a_ring_with_its_hole_thins_to_a_closed_line_by_either_rule() {
        let outer = rect(0.0, 0.0, 60.0, 100.0);
        // the hole runs the other way round (non-zero) — even-odd needs no direction
        let hole: Vec<(f64, f64)> = rect(12.0, 12.0, 48.0, 88.0).into_iter().rev().collect();
        for even_odd in [false, true] {
            let sk = skeleton(&[&outer, &hole], even_odd).expect("skeleton");
            assert!(sk.lines.iter().any(|(l, closed)| *closed && l.len() >= 4), "{even_odd}: {:?}", sk.lines);
            assert!(sk.ink < 0.6, "the hole is empty: {}", sk.ink);
            assert_eq!(sk.holes, 1);
        }
        // overlapping pieces of one glyph (a T drawn as two bars) fill as one by non-zero
        let (stem, top) = (rect(25.0, 0.0, 35.0, 90.0), rect(0.0, 80.0, 60.0, 90.0));
        let sk = skeleton(&[&stem, &top], false).expect("skeleton");
        assert!((2..=4).contains(&sk.lines.len()), "a junction, three arms: {:?}", sk.lines);
        assert_eq!(sk.holes, 0);
        // two holes: an 8 drawn as a box with two windows
        let (w1, w2): (Vec<(f64, f64)>, Vec<(f64, f64)>) =
            (rect(12.0, 12.0, 48.0, 44.0).into_iter().rev().collect(), rect(12.0, 56.0, 48.0, 88.0).into_iter().rev().collect());
        assert_eq!(skeleton(&[&outer, &w1, &w2], false).expect("skeleton").holes, 2);
        assert!(holes_fit('8', 2) && holes_fit('B', 2) && !holes_fit('6', 2) && holes_fit('4', 0) && holes_fit('4', 1) && !holes_fit('T', 1));
    }

    #[test]
    fn degenerate_outlines_have_no_skeleton() {
        assert!(skeleton(&[], false).is_none());
        assert!(skeleton(&[&[(1.0, 1.0), (1.0, 1.0), (1.0, 1.0)]], false).is_none());
        let nan = [(f64::NAN, 0.0), (1.0, 0.0), (1.0, 1.0)];
        assert!(skeleton(&[&nan], false).is_none());
    }
}
