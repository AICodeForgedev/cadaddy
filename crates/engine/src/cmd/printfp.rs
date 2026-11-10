//! Fire protection from the documents (Q6497): sprinkler heads and pipe read off the fire
//! protection plans of the set and off the sprinkler shop drawings among the submittals.
//!
//! A building whose fire protection is a deferred submittal has no FP sheets in its design set:
//! its sprinklers are drawn on the sprinkler contractor's shop drawings. Those pages are read like
//! FP sheets:
//! - **Heads** are small circles, typed by the page's own head legend (each description of a head
//!   kind with the symbol drawn beside it: its radius and its fill, plain, marked inside (hatched,
//!   crossed) or solid); a circle no legend entry matches is no head. With no legend at all,
//!   circles of a head's usual size are heads of no type. Legend and table symbols are left out.
//! - **The head table** (`HEAD SCHEDULE`, any column order: the count under its `QTY` heading,
//!   else the row's one whole number) is checked against the heads counted on the page.
//! - **Pipe** is drawn as filled bars, one bar weight per role: the weight the heads sit on is the
//!   branch line; a bar joined end-on to the pipe found so far is pipe too (of the branch weight,
//!   or of a heavier weight when two pipes end on it, it carries a size label, or it continues a
//!   main straight on). Sizes from the labels beside the bars, else the size labelled on another
//!   bar of the same weight, else the rules data's branch/main size (`size_source` says which).
//! - **Placement**: an FP sheet of the set stays on its own sheet (the build places it by the
//!   sheet's registration, as it does the mechanical and plumbing plans); a shop-drawing page is
//!   put onto its floor's architectural plan by the room numbers both print
//!   ([`cadaddy_mepfix::tagid::fit_by_tags`]), the fit then checked and refined on the walls both
//!   draw ([`super::validate_register::fit_walls`]); its level is the plan's it fits.
//! - **Nothing to read**: no FP plan in the set and no shop drawing among the submittals is said
//!   (the trade is missing, not silently empty).
//!
//! The words are rule data (`wordrules.json`: `fp.*`), the sizes, heights and limits
//! `printfp.json`, each with its source.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_walk3d::{Made, Mep, MepKind, MepRun, MepSystem, MepTerminal};
use serde::Deserialize;
use serde_json::{Value, json};

use super::printmep::{self, SheetRef};

const RULES: &str = include_str!("printfp.json");
/// The fire protection system's name in the spec.
pub(crate) const SYSTEM: &str = "FP";

/// Most objects looked at on one page, and most circles, bars and segments kept (hostile pages).
const MAX_ENTS: usize = 2_000_000;
const MAX_CIRCLES: usize = 100_000;
const MAX_BARS: usize = 50_000;
const MAX_SEGS: usize = 400_000;
/// Most heads and pipe pieces one set reads.
const MAX_SET_ITEMS: usize = 200_000;
/// Most grid cells one bar is entered in.
const MAX_CELLS_PER_BAR: usize = 4_000;
/// The fewest points a polygon has to be read as a circle.
const CIRCLE_POINTS: usize = 8;
/// How far a polygon's points may stray from its radius and be a circle (a fraction of it).
const CIRCLE_TOL: f64 = 0.12;
/// Most rows read below a head table's title.
const MAX_TABLE_ROWS: usize = 60;
/// The longest legend description read (characters).
const MAX_LEGEND_CHARS: usize = 90;
/// The shortest wall line (feet) the wall check takes, and how many lines at most each side.
const WALL_LINE_FT: f64 = 2.0;
const MAX_WALL_LINES: usize = 4_000;

// ─── Rules ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    symbols: Symbols,
    bars: Bars,
    sizes: Sizes,
    heights: Heights,
    pages: Pages,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Symbols {
    head_radius_in: [f64; 2],
    no_legend_head_radius_in: [f64; 2],
    legend_match: f64,
    legend_reach_heights: f64,
    inner_mark_min: f64,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Bars {
    width_in: [f64; 2],
    min_aspect: f64,
    class_step: f64,
    joint_slack_in: f64,
    label_reach_heights: f64,
    head_reach_radii: f64,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Sizes {
    branch_in: f64,
    main_in: f64,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Heights {
    default_ceiling_ft: f64,
    pendent_below_ceiling_in: f64,
    upright_below_ceiling_in: f64,
    sidewall_below_ceiling_in: f64,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Pages {
    max_files: usize,
    max_pages: usize,
    max_depth: usize,
    min_tags: usize,
    tag_tol_ft: f64,
    page_scale_in_per_ft: [f64; 2],
    duplicate_head_ft: f64,
    source: String,
}

fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

// ─── Geometry ───────────────────────────────────────────────────────────────

type P = [f64; 2];

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn mid(a: P, b: P) -> P {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2.is_nan() || l2 <= 1e-18 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + dx * t, a[1] + dy * t])
}

/// A polygon's points without a repeated closing point.
fn open_ring(mut pts: Vec<P>) -> Vec<P> {
    if pts.len() > 2 && pts.first().zip(pts.last()).is_some_and(|(a, b)| dist(*a, *b) < 1e-9) {
        pts.pop();
    }
    pts
}

/// The circle a polygon draws (centre, radius), when its points lie on one.
fn as_circle(pts: &[P]) -> Option<(P, f64)> {
    if pts.len() < CIRCLE_POINTS {
        return None;
    }
    let n = pts.len() as f64;
    let c = [pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n];
    let r = pts.iter().map(|p| dist(*p, c)).sum::<f64>() / n;
    (r.is_finite() && r > 0.0 && pts.iter().all(|p| (dist(*p, c) - r).abs() <= CIRCLE_TOL * r)).then_some((c, r))
}

/// A filled bar a rectangle draws: its centreline (along its long sides) and its width.
fn as_bar(pts: &[P]) -> Option<(P, P, f64)> {
    let [p0, p1, p2, p3] = pts else { return None };
    let (p0, p1, p2, p3) = (*p0, *p1, *p2, *p3);
    // Right angles at every corner.
    let corners = [(p3, p0, p1), (p0, p1, p2), (p1, p2, p3), (p2, p3, p0)];
    for (a, b, c) in corners {
        let (u, v) = ([a[0] - b[0], a[1] - b[1]], [c[0] - b[0], c[1] - b[1]]);
        let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
        if !(lu > 1e-12 && lv > 1e-12) || ((u[0] * v[0] + u[1] * v[1]) / (lu * lv)).abs() > 0.05 {
            return None;
        }
    }
    let (e0, e1) = (dist(p0, p1), dist(p1, p2));
    if e0 >= e1 { Some((mid(p3, p0), mid(p1, p2), e1)) } else { Some((mid(p0, p1), mid(p2, p3), e0)) }
}

/// What a circle has inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fill {
    Plain,
    /// Strokes inside it: hatched or crossed.
    Marked,
    Solid,
}

impl Fill {
    fn label(self) -> &'static str {
        match self {
            Fill::Plain => "plain",
            Fill::Marked => "marked",
            Fill::Solid => "solid",
        }
    }
}

#[derive(Clone, Debug)]
struct Circ {
    c: P,
    r: f64,
    fill: Fill,
    colour: String,
}

#[derive(Clone, Debug)]
struct Bar {
    a: P,
    b: P,
    w: f64,
    colour: String,
}

impl Bar {
    fn len(&self) -> f64 {
        dist(self.a, self.b)
    }
}

/// The marks of one page, in its own units.
#[derive(Default)]
struct Prims {
    circles: Vec<Circ>,
    bars: Vec<Bar>,
    segs: Vec<(P, P)>,
    /// The colour most of the page's line length is drawn in (its background, on a shop drawing).
    background: Option<String>,
}

fn area(pts: &[P]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts.get(i).copied().unwrap_or([0.0; 2]), pts.get((i + 1) % n).copied().unwrap_or([0.0; 2]));
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

/// The convex hull of `pts`, round it, without points on its sides.
fn hull(mut pts: Vec<P>) -> Vec<P> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| dist(*a, *b) < 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: P, a: P, b: P| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<P> = Vec::new();
    for p in &pts {
        while lower.len() >= 2
            && lower.get(lower.len() - 2).zip(lower.last()).is_some_and(|(a, b)| cross(*a, *b, *p) <= 1e-12 * (1.0 + dist(*a, *p).powi(2)))
        {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<P> = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2
            && upper.get(upper.len() - 2).zip(upper.last()).is_some_and(|(a, b)| cross(*a, *b, *p) <= 1e-12 * (1.0 + dist(*a, *p).powi(2)))
        {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    // Points (all but) on a side (a fill cut in slices puts its cuts there): a corner whose
    // neighbours' chord passes within a ten-thousandth of the hull's size of it isn't one.
    let size = lower.iter().flat_map(|a| lower.iter().map(move |b| dist(*a, *b))).fold(0.0, f64::max);
    let tol = 1e-4 * size;
    for _ in 0..lower.len() {
        let n = lower.len();
        if n <= 3 {
            break;
        }
        let flat = (0..n).find(|i| {
            let (a, p, b) = (lower.get((i + n - 1) % n), lower.get(*i), lower.get((i + 1) % n));
            match (a, p, b) {
                (Some(a), Some(p), Some(b)) => seg_dist(*p, *a, *b) <= tol,
                _ => false,
            }
        });
        match flat {
            Some(i) => {
                lower.remove(i);
            }
            None => break,
        }
    }
    lower
}

/// The quadrilaterals a fill written as triangles draws (any triangulation: two triangles, a fan
/// round the middle): its triangles joined where they share a corner, each group whose hull has
/// four corners and covers the triangles' own area. The triangles none of these took are left.
fn triangle_quads(rings: &[Vec<P>]) -> (Vec<Vec<P>>, Vec<bool>) {
    let tris: Vec<usize> = rings.iter().enumerate().filter(|(_, r)| r.len() == 3).map(|(i, _)| i).collect();
    let mut taken = vec![false; rings.len()];
    if tris.len() < 2 {
        return (Vec::new(), taken);
    }
    // Union-find over the triangles by their corners.
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        for _ in 0..64 {
            let p = parent.get(i).copied().unwrap_or(i);
            if p == i {
                break;
            }
            let gp = parent.get(p).copied().unwrap_or(p);
            if let Some(x) = parent.get_mut(i) {
                *x = gp;
            }
            i = gp;
        }
        i
    }
    let mut corner: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    let scale = rings.iter().flatten().map(|p| p[0].abs().max(p[1].abs())).fold(1.0, f64::max);
    let q = 1e-7 * scale;
    for (t, ri) in tris.iter().enumerate() {
        for p in rings.get(*ri).into_iter().flatten() {
            let k = ((p[0] / q).round().clamp(-9.0e15, 9.0e15) as i64, (p[1] / q).round().clamp(-9.0e15, 9.0e15) as i64);
            match corner.get(&k) {
                Some(o) => {
                    let (a, b) = (root(&mut parent, *o), root(&mut parent, t));
                    if a != b
                        && let Some(x) = parent.get_mut(a)
                    {
                        *x = b;
                    }
                }
                None => {
                    corner.insert(k, t);
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for t in 0..tris.len() {
        let r = root(&mut parent, t);
        groups.entry(r).or_default().push(t);
    }
    let mut quads = Vec::new();
    for members in groups.values().filter(|m| m.len() >= 2 && m.len() <= 16) {
        let rs: Vec<&Vec<P>> = members.iter().filter_map(|t| tris.get(*t)).filter_map(|ri| rings.get(*ri)).collect();
        let pts: Vec<P> = rs.iter().flat_map(|r| r.iter().copied()).collect();
        let h = hull(pts);
        let own: f64 = rs.iter().map(|r| area(r)).sum();
        let ha = area(&h);
        if h.len() == 4 && ha > 0.0 && (own - ha).abs() <= 0.05 * ha {
            quads.push(h);
            for t in members {
                if let Some(x) = tris.get(*t).and_then(|ri| taken.get_mut(*ri)) {
                    *x = true;
                }
            }
        }
    }
    (quads, taken)
}

/// Texts once: the same text twice within a text height of itself (a PDF's text and its
/// recovered copy) is one.
fn unique_texts(texts: Vec<cadaddy_mepfix::sheet_facts::TextItem>) -> Vec<cadaddy_mepfix::sheet_facts::TextItem> {
    let mut seen: BTreeMap<String, Vec<(P, f64)>> = BTreeMap::new();
    texts
        .into_iter()
        .filter(|t| {
            let at = [t.at.x, t.at.y];
            let h = t.height.abs();
            let v = seen.entry(t.value.trim().to_string()).or_default();
            if v.iter().take(256).any(|(p, g)| dist(*p, at) <= h.max(*g)) {
                return false;
            }
            v.push((at, h));
            true
        })
        .collect()
}

fn prims(d: &Drawing, u: f64) -> Prims {
    let rs = &rules().symbols;
    let (r_lo, r_hi) = (rs.head_radius_in[0] * u, rs.head_radius_in[1] * u);
    let mut strokes: Vec<(P, f64, String)> = Vec::new();
    let mut fills: Vec<(P, f64, String)> = Vec::new();
    let mut out = Prims::default();
    let colour = |e: &cadaddy_doc::Entity| format!("{:?}", e.common.color);
    let mut ink: BTreeMap<String, f64> = BTreeMap::new();
    for e in d.model.iter().take(MAX_ENTS) {
        match &e.kind {
            EntityKind::Circle(c) => {
                let p = [c.center.x, c.center.y];
                if c.radius.is_finite() && p.iter().all(|v| v.is_finite()) && strokes.len() < MAX_CIRCLES {
                    strokes.push((p, c.radius, colour(e)));
                }
            }
            EntityKind::Line(l) => {
                let (a, b) = ([l.a.x, l.a.y], [l.b.x, l.b.y]);
                if a.iter().chain(&b).all(|v| v.is_finite()) && out.segs.len() < MAX_SEGS {
                    *ink.entry(colour(e)).or_default() += dist(a, b);
                    out.segs.push((a, b));
                }
            }
            EntityKind::LwPolyline(pl) => {
                let pts: Vec<P> = pl.vertices.iter().map(|v| [v.p.x, v.p.y]).filter(|p| p.iter().all(|v| v.is_finite())).collect();
                let ring = open_ring(pts.clone());
                if pl.closed || ring.len() < pts.len() {
                    if let Some((c, r)) = as_circle(&ring) {
                        if strokes.len() < MAX_CIRCLES {
                            strokes.push((c, r, colour(e)));
                        }
                        continue;
                    }
                    if let Some((a, b, w)) = as_bar(&ring)
                        && out.bars.len() < MAX_BARS
                    {
                        out.bars.push(Bar { a, b, w, colour: colour(e) });
                    }
                }
                let mut all = ring.clone();
                if pl.closed
                    && let Some(f) = ring.first()
                {
                    all.push(*f);
                }
                for w in all.windows(2) {
                    if let [a, b] = w
                        && out.segs.len() < MAX_SEGS
                    {
                        *ink.entry(colour(e)).or_default() += dist(*a, *b);
                        out.segs.push((*a, *b));
                    }
                }
            }
            EntityKind::Hatch(h) => {
                let rings: Vec<Vec<P>> = h
                    .loops
                    .iter()
                    .take(MAX_BARS)
                    .map(|l| open_ring(l.vertices.iter().map(|v| [v.p.x, v.p.y]).filter(|p| p.iter().all(|v| v.is_finite())).collect()))
                    .collect();
                // A fill written as triangles: the quadrilaterals they make.
                let (joined, taken) = triangle_quads(&rings);
                let rest = rings.into_iter().zip(taken).filter(|(_, t)| !t).map(|(r, _)| r);
                for ring in joined.into_iter().chain(rest) {
                    if let Some((c, r)) = as_circle(&ring) {
                        if fills.len() < MAX_CIRCLES {
                            fills.push((c, r, colour(e)));
                        }
                    } else if let Some((a, b, w)) = as_bar(&ring) {
                        if out.bars.len() < MAX_BARS {
                            out.bars.push(Bar { a, b, w, colour: colour(e) });
                        }
                    } else if ring.len() >= 3 && fills.len() < MAX_CIRCLES {
                        let n = ring.len() as f64;
                        let c = [ring.iter().map(|p| p[0]).sum::<f64>() / n, ring.iter().map(|p| p[1]).sum::<f64>() / n];
                        let r = ring.iter().map(|p| dist(*p, c)).fold(0.0, f64::max);
                        fills.push((c, r, colour(e)));
                    }
                }
            }
            _ => {}
        }
    }
    // Each head-sized circle once (a stroke and its fill are one), with what it has inside: the
    // fills and the short strokes looked up by a grid of a head's size.
    let cell = (2.0 * r_hi).max(1e-9);
    let key = |p: P| ((p[0] / cell).floor().clamp(-1.0e9, 1.0e9) as i64, (p[1] / cell).floor().clamp(-1.0e9, 1.0e9) as i64);
    let mut fill_at: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, (c, _, _)) in fills.iter().enumerate() {
        fill_at.entry(key(*c)).or_default().push(i);
    }
    let mut seg_at: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, (a, b)) in out.segs.iter().enumerate() {
        if dist(*a, *b) <= 2.1 * r_hi {
            seg_at.entry(key(mid(*a, *b))).or_default().push(i);
        }
    }
    let around = |m: &BTreeMap<(i64, i64), Vec<usize>>, p: P| -> Vec<usize> {
        let (x, y) = key(p);
        let mut v = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ix) = m.get(&(x.saturating_add(dx), y.saturating_add(dy))) {
                    v.extend(ix.iter().copied());
                }
            }
        }
        v
    };
    let mut circles: Vec<Circ> = Vec::new();
    for (c, r, ink_of) in strokes.iter().chain(fills.iter().filter(|(_, r, _)| *r >= r_lo)) {
        if !(*r >= r_lo && *r <= r_hi) {
            continue;
        }
        if circles.len() >= MAX_CIRCLES || circles.iter().rev().take(64).any(|k| dist(k.c, *c) <= 0.1 * k.r && (k.r - r).abs() <= 0.15 * k.r) {
            continue;
        }
        let solid = around(&fill_at, *c)
            .into_iter()
            .filter_map(|i| fills.get(i))
            .any(|(fc, fr, _)| dist(*fc, *c) <= 0.4 * r && *fr <= 1.05 * r && *fr >= 0.3 * r);
        let marked = || {
            around(&seg_at, *c)
                .into_iter()
                .filter_map(|i| out.segs.get(i))
                .any(|(a, b)| dist(*a, *c) <= 1.03 * r && dist(*b, *c) <= 1.03 * r && dist(*a, *b) >= rs.inner_mark_min * r)
        };
        let fill = if solid {
            Fill::Solid
        } else if marked() {
            Fill::Marked
        } else {
            Fill::Plain
        };
        circles.push(Circ { c: *c, r: *r, fill, colour: ink_of.clone() });
    }
    out.circles = circles;
    out.background = ink.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(c, _)| c);
    out
}

// ─── One page's reading ─────────────────────────────────────────────────────

/// A legend entry: a head kind's description and the symbol drawn beside it.
#[derive(Clone, Debug)]
struct LegendEntry {
    label: String,
    kind: &'static str,
    r: f64,
    fill: Fill,
    colour: String,
}

#[derive(Clone, Debug)]
struct PageHead {
    at: P,
    kind: &'static str,
    label: String,
}

#[derive(Clone, Debug)]
struct PagePipe {
    a: P,
    b: P,
    size_in: f64,
    size_source: String,
    branch: bool,
}

#[derive(Clone, Debug)]
struct TableRow {
    label: String,
    kind: Option<&'static str>,
    count: usize,
}

/// What one page (or sheet) draws of fire protection, in its own units.
#[derive(Default)]
struct PageRead {
    heads: Vec<PageHead>,
    pipes: Vec<PagePipe>,
    legend: Vec<LegendEntry>,
    table: Vec<TableRow>,
    bars_seen: usize,
}

fn head_kind(text: &str) -> Option<&'static str> {
    let up = text.to_ascii_uppercase();
    cadaddy_mepfix::wordrules::pairs("fp.head_kinds").iter().find(|(w, _)| up.contains(w)).map(|(_, k)| *k)
}

fn whole_number(t: &str) -> Option<usize> {
    let t = t.trim();
    (!t.is_empty() && t.len() <= 5 && t.chars().all(|c| c.is_ascii_digit())).then(|| t.parse().ok()).flatten()
}

/// A text's middle, from its insertion point (left of its baseline) and its height.
fn text_mid(t: &cadaddy_mepfix::sheet_facts::TextItem) -> P {
    let w = 0.6 * t.height * t.value.trim().chars().count() as f64;
    [t.at.x + w / 2.0, t.at.y + t.height / 2.0]
}

/// The head table under its title: each row's description and count (any column order).
fn head_table(texts: &[cadaddy_mepfix::sheet_facts::TextItem], legend: &[LegendEntry]) -> Vec<TableRow> {
    let titles = cadaddy_mepfix::wordrules::words("fp.head_table_titles");
    let qty_words = cadaddy_mepfix::wordrules::words("fp.quantity_headers");
    let Some(title) = texts.iter().find(|t| {
        let up = t.value.to_ascii_uppercase();
        titles.iter().any(|w| up.contains(w))
    }) else {
        return Vec::new();
    };
    let h = if title.height.is_finite() && title.height > 0.0 { title.height } else { return Vec::new() };
    let (tx, ty) = (title.at.x, title.at.y);
    let mut cells: Vec<&cadaddy_mepfix::sheet_facts::TextItem> = texts
        .iter()
        .filter(|t| !std::ptr::eq(*t, title) && t.at.y < ty - 0.2 * h && t.at.y > ty - 60.0 * h && t.at.x > tx - 30.0 * h && t.at.x < tx + 90.0 * h)
        .collect();
    cells.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
    let mut rows: Vec<Vec<&cadaddy_mepfix::sheet_facts::TextItem>> = Vec::new();
    for c in cells {
        let same_row = rows.last().and_then(|r| r.first()).is_some_and(|f| (f.at.y - c.at.y).abs() <= 0.6 * h.max(c.height));
        if same_row && let Some(r) = rows.last_mut() {
            r.push(c);
        } else if rows.len() < MAX_TABLE_ROWS {
            rows.push(vec![c]);
        } else {
            break;
        }
    }
    let mut qty_x: Option<f64> = None;
    let mut out = Vec::new();
    let mut misses = 0usize;
    for row in rows {
        if qty_x.is_none()
            && let Some(hd) = row.iter().find(|t| {
                let up = t.value.trim().to_ascii_uppercase();
                qty_words.iter().any(|w| up.starts_with(w))
            })
        {
            qty_x = Some(text_mid(hd)[0]);
            continue;
        }
        let numbers: Vec<(f64, usize)> = row.iter().filter_map(|t| whole_number(&t.value).map(|n| (text_mid(t)[0], n))).collect();
        let label = row.iter().filter(|t| t.value.chars().any(|c| c.is_ascii_alphabetic())).map(|t| t.value.trim()).collect::<Vec<_>>().join(" ");
        let count = match qty_x {
            Some(x) => numbers.iter().min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs())).filter(|n| (n.0 - x).abs() <= 8.0 * h).map(|n| n.1),
            None if numbers.len() == 1 => numbers.first().map(|n| n.1),
            None => numbers.iter().max_by(|a, b| a.0.total_cmp(&b.0)).map(|n| n.1),
        };
        let kind = head_kind(&label);
        let known = kind.is_some() || legend.iter().any(|l| same_label(&l.label, &label));
        match count {
            Some(n) if known && !label.to_ascii_uppercase().contains("TOTAL") => {
                misses = 0;
                out.push(TableRow { label, kind, count: n });
            }
            _ => {
                misses += 1;
                if !out.is_empty() && misses >= 2 {
                    break;
                }
            }
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase()
}

fn same_label(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

/// A uniform grid over the bars, for the bars near a point.
struct BarGrid {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl BarGrid {
    fn new(bars: &[Bar], cell: f64) -> BarGrid {
        let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        let key = |v: f64| (v / cell).floor().clamp(-1.0e9, 1.0e9) as i64;
        for (i, b) in bars.iter().enumerate() {
            let pad = b.w + cell * 0.01;
            let (x0, x1) = (key(b.a[0].min(b.b[0]) - pad), key(b.a[0].max(b.b[0]) + pad));
            let (y0, y1) = (key(b.a[1].min(b.b[1]) - pad), key(b.a[1].max(b.b[1]) + pad));
            let n = (x1.saturating_sub(x0).saturating_add(1)).saturating_mul(y1.saturating_sub(y0).saturating_add(1));
            if !(0..=MAX_CELLS_PER_BAR as i64).contains(&n) {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        BarGrid { cell, cells }
    }
    fn near(&self, p: P) -> &[usize] {
        let k = ((p[0] / self.cell).floor().clamp(-1.0e9, 1.0e9) as i64, (p[1] / self.cell).floor().clamp(-1.0e9, 1.0e9) as i64);
        self.cells.get(&k).map_or(&[], Vec::as_slice)
    }
}

/// Read one page's fire protection; `u`: the page's units per inch of paper.
fn read_page(d: &Drawing, u: f64) -> PageRead {
    let rl = rules();
    let rs = &rl.symbols;
    let rb = &rl.bars;
    let pr = prims(d, u);
    let texts = unique_texts(cadaddy_mepfix::sheet_facts::texts(d));
    let mut out = PageRead::default();

    // The legend: each head description with the symbol on its line, left of it.
    let mut used = vec![false; pr.circles.len()];
    for t in texts.iter().filter(|t| t.value.trim().chars().count() <= MAX_LEGEND_CHARS && t.rotation.abs() < 0.05) {
        let Some(kind) = head_kind(&t.value) else { continue };
        let h = if t.height.is_finite() && t.height > 0.0 { t.height } else { continue };
        let y = t.at.y + h / 2.0;
        let best = pr
            .circles
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                !used.get(*i).copied().unwrap_or(true)
                    && (c.c[1] - y).abs() <= 1.5 * h
                    && c.c[0] < t.at.x
                    && c.c[0] > t.at.x - rs.legend_reach_heights * h
            })
            .min_by(|a, b| (t.at.x - a.1.c[0]).total_cmp(&(t.at.x - b.1.c[0])));
        if let Some((i, c)) = best {
            if let Some(f) = used.get_mut(i) {
                *f = true;
            }
            if !out.legend.iter().any(|l| same_label(&l.label, &t.value) && l.fill == c.fill && (l.r - c.r).abs() <= rs.legend_match * l.r) {
                out.legend.push(LegendEntry { label: t.value.trim().to_string(), kind, r: c.r, fill: c.fill, colour: c.colour.clone() });
            }
        }
    }
    // A background's own legend (an architectural or ceiling plan drawn under the sprinklers, in
    // its own pen) is not the page's: when the page has a legend in another pen than its
    // background, the background's entries are dropped.
    if let Some(bg) = &pr.background
        && out.legend.iter().any(|l| l.colour != *bg)
    {
        out.legend.retain(|l| l.colour != *bg);
    }
    // The heads: every other circle a legend entry matches (else, no legend, of a head's size).
    let (n_lo, n_hi) = (rs.no_legend_head_radius_in[0] * u, rs.no_legend_head_radius_in[1] * u);
    let mut head_r: Vec<f64> = Vec::new();
    for (i, c) in pr.circles.iter().enumerate() {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        if out.legend.is_empty() {
            if c.r >= n_lo && c.r <= n_hi {
                out.heads.push(PageHead { at: c.c, kind: "sprinkler_pendent", label: String::new() });
                head_r.push(c.r);
            }
            continue;
        }
        let entry = out
            .legend
            .iter()
            .filter(|l| l.fill == c.fill && l.colour == c.colour && (l.r - c.r).abs() <= rs.legend_match * l.r)
            .min_by(|a, b| (a.r - c.r).abs().total_cmp(&(b.r - c.r).abs()));
        if let Some(l) = entry {
            out.heads.push(PageHead { at: c.c, kind: l.kind, label: l.label.clone() });
            head_r.push(c.r);
        }
    }
    out.table = head_table(&texts, &out.legend);

    // Pipe: filled bars of a pipe's weight.
    let bars: Vec<Bar> =
        pr.bars.into_iter().filter(|b| b.w >= rb.width_in[0] * u && b.w <= rb.width_in[1] * u && b.len() >= rb.min_aspect * b.w).collect();
    out.bars_seen = bars.len();
    if bars.is_empty() || out.heads.is_empty() {
        return out;
    }
    // Bar weights: widths more than `class_step` apart are two.
    let mut widths: Vec<f64> = bars.iter().map(|b| b.w).collect();
    widths.sort_by(f64::total_cmp);
    let mut classes: Vec<f64> = Vec::new();
    for w in widths {
        if classes.last().is_none_or(|c| w > c * rb.class_step) {
            classes.push(w);
        }
    }
    let class_of = |w: f64| classes.iter().rposition(|c| w >= *c * 0.999).unwrap_or(0);
    let cls: Vec<usize> = bars.iter().map(|b| class_of(b.w)).collect();
    // The weight the heads sit on is the branch line's.
    let mut on_heads: BTreeMap<usize, usize> = BTreeMap::new();
    let mut seeds: Vec<usize> = Vec::new();
    for (i, b) in bars.iter().enumerate() {
        let touches = out.heads.iter().zip(&head_r).filter(|(hd, r)| seg_dist(hd.at, b.a, b.b) <= (rb.head_reach_radii * **r).max(b.w)).count();
        if touches > 0 {
            *on_heads.entry(cls.get(i).copied().unwrap_or(0)).or_default() += touches;
            seeds.push(i);
        }
    }
    let Some(branch) = on_heads.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(c, _)| *c) else { return out };
    seeds.retain(|i| cls.get(*i) == Some(&branch));
    let colours: BTreeSet<&str> = seeds.iter().filter_map(|i| bars.get(*i)).map(|b| b.colour.as_str()).collect();

    // Size labels, each on the bar nearest it.
    let mut label_of: Vec<Option<f64>> = vec![None; bars.len()];
    let longest = bars.iter().map(Bar::len).fold(0.0, f64::max);
    let grid = BarGrid::new(&bars, (longest / 4.0).max(1.0 * u));
    for t in &texts {
        let v = t.value.trim();
        if v.chars().count() > 14 || !v.contains('"') {
            continue;
        }
        let Some(size) = cadaddy_mepfix::sprinkler_reader::parse_pipe_size(v) else { continue };
        let m = text_mid(t);
        let reach = rb.label_reach_heights * t.height.max(1e-9);
        let nearest = |ix: &mut dyn Iterator<Item = usize>| -> Option<(f64, usize)> {
            ix.filter_map(|i| bars.get(i).map(|b| (seg_dist(m, b.a, b.b) - b.w / 2.0, i)))
                .filter(|(dd, _)| *dd <= reach)
                .min_by(|a, b| a.0.total_cmp(&b.0))
        };
        let best = nearest(&mut grid.near(m).iter().copied()).or_else(|| nearest(&mut (0..bars.len())));
        if let Some((_, i)) = best
            && let Some(l) = label_of.get_mut(i)
        {
            *l = Some(size);
        }
    }

    // Grow the pipe from the branch lines the heads sit on: what ends on it is pipe too.
    let slack = rb.joint_slack_in * u;
    let joins = |i: usize, j: usize| -> bool {
        let (Some(x), Some(y)) = (bars.get(i), bars.get(j)) else { return false };
        let tol = (x.w + y.w) / 2.0 + slack;
        seg_dist(x.a, y.a, y.b) <= tol || seg_dist(x.b, y.a, y.b) <= tol || seg_dist(y.a, x.a, x.b) <= tol || seg_dist(y.b, x.a, x.b) <= tol
    };
    let straight_on = |i: usize, j: usize| -> bool {
        let (Some(x), Some(y)) = (bars.get(i), bars.get(j)) else { return false };
        let ux = [(x.b[0] - x.a[0]) / x.len().max(1e-12), (x.b[1] - x.a[1]) / x.len().max(1e-12)];
        let uy = [(y.b[0] - y.a[0]) / y.len().max(1e-12), (y.b[1] - y.a[1]) / y.len().max(1e-12)];
        let tol = (x.w + y.w) / 2.0 + slack;
        (ux[0] * uy[1] - ux[1] * uy[0]).abs() < 0.09 && [x.a, x.b].iter().any(|p| dist(*p, y.a) <= tol || dist(*p, y.b) <= tol)
    };
    let mut pipe = vec![false; bars.len()];
    for i in &seeds {
        if let Some(f) = pipe.get_mut(*i) {
            *f = true;
        }
    }
    for _ in 0..bars.len().min(10_000) {
        let mut changed = false;
        for j in 0..bars.len() {
            if pipe.get(j).copied().unwrap_or(true) {
                continue;
            }
            let Some(bj) = bars.get(j) else { continue };
            let cj = cls.get(j).copied().unwrap_or(0);
            if cj < branch || !colours.contains(bj.colour.as_str()) {
                continue;
            }
            let mut near: BTreeSet<usize> = BTreeSet::new();
            for p in [bj.a, bj.b, mid(bj.a, bj.b)] {
                near.extend(grid.near(p).iter().copied().filter(|i| *i != j && pipe.get(*i).copied().unwrap_or(false)));
            }
            let joined: Vec<usize> = near.into_iter().filter(|i| joins(*i, j)).collect();
            if joined.is_empty() {
                continue;
            }
            let ok = cj == branch
                || joined.len() >= 2
                || label_of.get(j).copied().flatten().is_some()
                || joined.iter().any(|i| cls.get(*i).is_some_and(|c| *c > branch) && straight_on(*i, j));
            if ok && let Some(f) = pipe.get_mut(j) {
                *f = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Sizes: its label, else the label its weight carries most, else the rules' size.
    let mut by_class: BTreeMap<usize, BTreeMap<u64, usize>> = BTreeMap::new();
    for (i, l) in label_of.iter().enumerate() {
        if let (Some(s), true) = (l, pipe.get(i).copied().unwrap_or(false)) {
            *by_class.entry(cls.get(i).copied().unwrap_or(0)).or_default().entry(s.to_bits()).or_default() += 1;
        }
    }
    for (i, b) in bars.iter().enumerate() {
        if !pipe.get(i).copied().unwrap_or(false) {
            continue;
        }
        let c = cls.get(i).copied().unwrap_or(0);
        let (size_in, size_source) = match label_of.get(i).copied().flatten() {
            Some(s) => (s, "tag".to_string()),
            None => match by_class.get(&c).and_then(|m| m.iter().max_by_key(|(_, n)| **n)).map(|(s, _)| f64::from_bits(*s)) {
                Some(s) => (s, "the size labelled on pipe of the same bar weight".to_string()),
                None if c == branch => (rl.sizes.branch_in, format!("rule: branch line size ({})", rl.sizes.source)),
                None => (rl.sizes.main_in, format!("rule: main size ({})", rl.sizes.source)),
            },
        };
        out.pipes.push(PagePipe { a: b.a, b: b.b, size_in, size_source, branch: c == branch });
    }
    out
}

// ─── The set ────────────────────────────────────────────────────────────────

/// One page or sheet read for fire protection.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceRead {
    /// `FP-101`, or a shop drawing's `<file> p.<n>`.
    pub source: String,
    /// `sheet` (an FP plan of the set) or `shop_drawing`.
    pub kind: &'static str,
    pub level: String,
    pub heads: usize,
    pub heads_by_type: BTreeMap<String, usize>,
    pub pipe_ft: f64,
    pub pipe_by_size: BTreeMap<String, f64>,
    /// The head table's rows (description, count) and how the heads counted compare.
    pub table: Vec<(String, usize, usize)>,
    pub table_differs: bool,
    /// The head legend read: description, head kind, symbol fill.
    pub legend: Vec<(String, String, String)>,
    /// How a shop drawing was put onto its floor.
    pub placed: Value,
}

impl SourceRead {
    fn to_json(&self) -> Value {
        json!({
            "source": self.source, "kind": self.kind, "level": self.level, "heads": self.heads, "heads_by_type": self.heads_by_type,
            "pipe_ft": printmep::r1(self.pipe_ft),
            "pipe_by_size": self.pipe_by_size.iter().map(|(k, v)| (k.clone(), json!(printmep::r1(*v)))).collect::<serde_json::Map<String, Value>>(),
            "table": self.table.iter().map(|(l, n, c)| json!({"row": l, "table": n, "counted": c})).collect::<Vec<_>>(),
            "table_differs": self.table_differs, "placed": self.placed,
            "legend": self.legend.iter().map(|(l, k, f)| json!({"label": l, "kind": k, "symbol": f})).collect::<Vec<_>>(),
        })
    }
}

/// What the set's fire protection read gave.
#[derive(Clone, Debug, Default)]
pub(crate) struct FpRead {
    pub mep: Mep,
    pub sources: Vec<SourceRead>,
    /// Shop-drawing pages with heads that could not be put onto any floor, and why.
    pub unplaced: Vec<(String, String)>,
    /// The shop-drawing files looked at.
    pub files: Vec<String>,
    /// FP plans of the set.
    pub fp_sheets: usize,
    pub duplicates: usize,
    pub warnings: Vec<String>,
}

impl FpRead {
    pub fn heads(&self) -> usize {
        self.mep.terminals.len()
    }
    pub fn pipe_ft(&self) -> f64 {
        self.sources.iter().map(|s| s.pipe_ft).sum()
    }
    /// No FP plan in the set and no shop drawing read: the trade has no source.
    pub fn missing(&self) -> bool {
        self.sources.is_empty() && self.unplaced.is_empty()
    }
    /// The `source_sheet` names of what was read (for a re-read to replace).
    pub fn source_names(&self) -> Vec<String> {
        self.sources.iter().map(|s| s.source.clone()).collect()
    }
    pub fn to_json(&self) -> Value {
        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        for s in &self.sources {
            for (k, n) in &s.heads_by_type {
                *by_type.entry(k.clone()).or_default() += n;
            }
        }
        json!({
            "heads": self.heads(), "heads_by_type": by_type, "pipe_ft": printmep::r1(self.pipe_ft()), "pipe_runs": self.mep.runs.len(),
            "fp_sheets": self.fp_sheets, "shop_drawing_files": self.files, "duplicate_heads_dropped": self.duplicates,
            "sources": self.sources.iter().map(SourceRead::to_json).collect::<Vec<_>>(),
            "unplaced": self.unplaced.iter().map(|(p, w)| json!({"page": p, "why": w})).collect::<Vec<_>>(),
            "missing": self.missing(), "warnings": self.warnings,
            "rules": {"symbols": rules().symbols.source, "bars": rules().bars.source, "heights": rules().heights.source, "pages": rules().pages.source},
        })
    }
}

/// A head's deflector height above its floor for its kind, under a ceiling at `ceiling_ft`.
fn head_height(kind: &str, ceiling_ft: f64) -> f64 {
    let h = &rules().heights;
    let below = match kind {
        "sprinkler_sidewall" => h.sidewall_below_ceiling_in,
        "sprinkler_upright" => h.upright_below_ceiling_in,
        _ => h.pendent_below_ceiling_in,
    };
    ceiling_ft - below / 12.0
}

fn size_label(inches: f64) -> String {
    let whole = inches.trunc();
    let frac = inches - whole;
    let f = [(0.25, "1/4"), (0.5, "1/2"), (0.75, "3/4")].iter().find(|(v, _)| (frac - v).abs() < 0.02).map(|(_, t)| *t);
    match (whole as u64, f) {
        (0, Some(t)) => format!("{t}\""),
        (w, Some(t)) => format!("{w}-{t}\""),
        (w, None) => format!("{w}\""),
    }
}

/// Add one source's reading to the set: points through `to` (page units → the drawing's), feet
/// at `upf` drawing units, on `levels` (the first its own, the others its typical floors').
#[allow(clippy::too_many_arguments)]
fn add_source(
    out: &mut FpRead,
    page: &PageRead,
    to: &dyn Fn(P) -> P,
    upf: f64,
    levels: &[String],
    source: &str,
    kind: &'static str,
    placed: Value,
    ceiling_of: &dyn Fn(&str) -> f64,
) {
    let dup = rules().pages.duplicate_head_ft * upf;
    for (n, level) in levels.iter().enumerate() {
        let ceiling = ceiling_of(level);
        let mut read = SourceRead { source: source.to_string(), kind, level: level.clone(), placed: placed.clone(), ..SourceRead::default() };
        read.legend = page.legend.iter().map(|l| (l.label.clone(), l.kind.to_string(), l.fill.label().to_string())).collect();
        let mut counted: BTreeMap<String, usize> = BTreeMap::new();
        for h in &page.heads {
            if out.mep.terminals.len() >= MAX_SET_ITEMS {
                break;
            }
            let at = to(h.at);
            if !at.iter().all(|v| v.is_finite()) {
                continue;
            }
            let label = if h.label.is_empty() { h.kind.to_string() } else { h.label.clone() };
            *counted.entry(label.clone()).or_default() += 1;
            // The same head another page draws (two pages showing one area) is one.
            if out.mep.terminals.iter().any(|t| t.level == *level && t.source_sheet != source && dist(t.at, at) <= dup) {
                out.duplicates += 1;
                continue;
            }
            *read.heads_by_type.entry(label).or_default() += 1;
            read.heads += 1;
            out.mep.terminals.push(MepTerminal {
                system: SYSTEM.into(),
                level: level.clone(),
                at,
                size_in: [2.0, 0.0],
                bottom_ft: head_height(h.kind, ceiling),
                terminal_kind: h.kind.to_string(),
                objects: Vec::new(),
                source_sheet: source.to_string(),
                made: Made::Drawn,
                basis: String::new(),
            });
        }
        for p in &page.pipes {
            if out.mep.runs.len() >= MAX_SET_ITEMS {
                break;
            }
            let (a, b) = (to(p.a), to(p.b));
            if !a.iter().chain(&b).all(|v| v.is_finite()) {
                continue;
            }
            // The same piece another page draws is one.
            let again = |r: &MepRun| {
                r.level == *level
                    && r.source_sheet != source
                    && matches!(r.points.as_slice(), [x, y] if (dist(*x, a) <= dup && dist(*y, b) <= dup) || (dist(*x, b) <= dup && dist(*y, a) <= dup))
            };
            if out.mep.runs.iter().any(again) {
                continue;
            }
            let ft = dist(a, b) / upf;
            read.pipe_ft += ft;
            *read.pipe_by_size.entry(size_label(p.size_in)).or_default() += ft;
            out.mep.runs.push(MepRun {
                system: SYSTEM.into(),
                level: level.clone(),
                points: vec![a, b],
                size_in: [p.size_in, 0.0],
                bottoms_ft: vec![ceiling + if p.branch { 0.5 } else { 1.0 }],
                objects: Vec::new(),
                id: String::new(),
                encased: false,
                size_source: p.size_source.clone(),
                height_source: "default".into(),
                reviewed: false,
                edited_by: String::new(),
                top_ft: None,
                source_sheet: source.to_string(),
                slope_in_per_ft: 0.0,
                made: Made::Drawn,
                basis: String::new(),
                corrections: Vec::new(),
            });
        }
        // The head table against the heads counted (the page's own count, before any duplicate
        // another page drew is taken out).
        if n == 0 {
            for row in &page.table {
                let have = counted
                    .iter()
                    .filter(|(l, _)| {
                        same_label(l, &row.label) || (!page.legend.iter().any(|e| same_label(&e.label, &row.label)) && row.kind == head_kind(l))
                    })
                    .map(|(_, c)| *c)
                    .sum::<usize>();
                read.table_differs |= have != row.count;
                read.table.push((row.label.clone(), row.count, have));
            }
        }
        out.sources.push(read);
    }
}

fn levels_of(sh: &SheetRef) -> Vec<String> {
    let mut v = vec![printmep::level_name(&sh.level)];
    for l in printmep::extra_levels_for(sh) {
        let n = printmep::level_name(&l);
        if !v.contains(&n) {
            v.push(n);
        }
    }
    v
}

fn ensure_system(mep: &mut Mep) {
    if !mep.systems.iter().any(|s| s.name == SYSTEM) {
        mep.systems.push(MepSystem {
            name: SYSTEM.into(),
            layer: "FP".into(),
            kind: MepKind::Pipe,
            size_in: [rules().sizes.branch_in, 0.0],
            bottom_ft: rules().heights.default_ceiling_ft + 0.5,
            show: true,
            duct_type: None,
            color_index: 0,
        });
    }
}

/// The FP plans of the set: each read on its own sheet (the build places it by the sheet's
/// registration, as the mechanical and plumbing plans).
fn read_set_sheets(out: &mut FpRead, d: &Drawing, sheets: &[SheetRef], upf: f64, ceiling_of: &dyn Fn(&str) -> f64) {
    for sh in sheets.iter().filter(|s| s.discipline.eq_ignore_ascii_case("FP") && s.builds()) {
        out.fp_sheets += 1;
        let sub = printmep::sheet_drawing(d, sh);
        let paper = printmep::paper_units_per_foot(&sub, sh);
        let sheet_upf = paper.unwrap_or(upf);
        // Units per inch of paper: a sheet at paper size is in inches; one at its scale, its
        // units per foot over the paper inches a foot is printed at.
        let printed = sh.scale_text.as_deref().and_then(cadaddy_mepfix::sheet_reader::parse_scale).filter(|k| k.is_finite() && *k > 0.0);
        let u = if paper.is_some() { 1.0 } else { upf / printed.unwrap_or(0.125) };
        let page = read_page(&sub, u);
        let k = if paper.is_some() { upf / sheet_upf } else { 1.0 };
        let at = printmep::placement(sh, k);
        if page.heads.is_empty() && page.pipes.is_empty() {
            out.warnings.push(format!("{}: no sprinkler heads or pipe read on this fire protection plan", sh.number));
        }
        add_source(out, &page, &|p| at.apply(p), upf, &levels_of(sh), &sh.number, "sheet", json!({"method": "its sheet's registration"}), ceiling_of);
    }
}

/// An architectural floor plan a shop drawing may be put onto.
struct Floor<'a> {
    sh: &'a SheetRef,
    sheet_upf: f64,
    k: f64,
    tags: Vec<(String, P)>,
    lines: Vec<(P, P)>,
}

fn long_lines(d: &Drawing, min: f64) -> Vec<(P, P)> {
    let mut v: Vec<(P, P)> = prims(d, 1.0).segs.into_iter().filter(|(a, b)| dist(*a, *b) >= min).collect();
    v.sort_by(|x, y| dist(y.0, y.1).total_cmp(&dist(x.0, x.1)));
    v.truncate(MAX_WALL_LINES);
    v
}

/// Put a shop-drawing page onto the floor plan it shares the most room numbers with; then the
/// walls both draw check the fit and refine it. The page's point → the floor sheet's point, the
/// floor, and how.
fn place_page<'a>(
    page: &Drawing,
    floors: &'a [Floor<'a>],
    title_level: Option<String>,
) -> Result<(Box<dyn Fn(P) -> P + 'a>, &'a Floor<'a>, Value), String> {
    use cadaddy_mepfix::tagid;
    let rp = &rules().pages;
    let tags = super::identity::number_texts(page);
    if tags.len() < rp.min_tags {
        return Err(format!("{} room number(s) printed on the page: at least {} are needed to put it onto a floor plan", tags.len(), rp.min_tags));
    }
    let [lo, hi] = rp.page_scale_in_per_ft;
    let mut best: Option<(usize, bool, f64, tagid::TagFit, &Floor)> = None;
    for f in floors {
        let tol = rp.tag_tol_ft * f.sheet_upf;
        let Some(fit) = tagid::fit_by_tags(&tags, &f.tags, tol, (f.sheet_upf / hi.max(1e-9), f.sheet_upf / lo.max(1e-9))) else { continue };
        if fit.matched.len() < rp.min_tags || tagid::few_tags_off_square(fit.matched.len(), fit.sim.rotation_deg()) {
            continue;
        }
        let on_level = title_level.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&f.sh.level));
        let res = fit.residual / f.sheet_upf;
        let better = best
            .as_ref()
            .is_none_or(|(n, lv, r, _, _)| (fit.matched.len(), on_level) > (*n, *lv) || ((fit.matched.len(), on_level) == (*n, *lv) && res < *r));
        if better {
            best = Some((fit.matched.len(), on_level, res, fit, f));
        }
    }
    let Some((n, _, res, fit, floor)) = best else {
        return Err("its room numbers fit no architectural floor plan of the set (three or more agreeing, at one scale)".into());
    };
    let sim = fit.sim;
    // The walls: the page's long lines carried onto the plan, fitted to the plan's lines.
    let unit = floor.sheet_upf;
    let page_upf = unit / sim.scale().max(1e-12);
    let model: Vec<(P, P)> = long_lines(page, WALL_LINE_FT * page_upf).into_iter().map(|(a, b)| (sim.apply(a), sim.apply(b))).collect();
    let centre = {
        let pts: Vec<P> = tags.iter().filter(|(t, _)| fit.matched.contains(t)).map(|(_, p)| sim.apply(*p)).collect();
        let m = pts.len().max(1) as f64;
        [pts.iter().map(|p| p[0]).sum::<f64>() / m, pts.iter().map(|p| p[1]).sum::<f64>() / m]
    };
    let walls = super::validate_register::fit_walls(&model, &floor.lines, unit, centre);
    let (correction, walls_json) = match walls {
        Ok(w) if w.good => {
            let c = w.correction;
            (
                Some(c),
                json!({"checked": true, "coverage": (w.coverage * 100.0).round() / 100.0, "moved_ft": (w.residual_ft * 100.0).round() / 100.0, "fit_ft": (w.fit_ft * 100.0).round() / 100.0}),
            )
        }
        Ok(w) => {
            (None, json!({"checked": true, "agree": false, "why": w.weak.unwrap_or_default(), "coverage": (w.coverage * 100.0).round() / 100.0}))
        }
        Err(why) => (None, json!({"checked": false, "why": why})),
    };
    let how = json!({
        "method": "room numbers", "onto": floor.sh.number, "level": floor.sh.level, "matched": n, "residual_ft": (res * 100.0).round() / 100.0,
        "scale": (sim.scale() * 1.0e4).round() / 1.0e4, "rotation_deg": (sim.rotation_deg() * 100.0).round() / 100.0,
        "page_in_per_ft": (page_upf * 1.0e4).round() / 1.0e4, "walls": walls_json,
    });
    let place = printmep::placement(floor.sh, floor.k);
    let to: Box<dyn Fn(P) -> P + 'a> = Box::new(move |p| {
        let q = sim.apply(p);
        let q = correction.map_or(q, |c| c.apply(q));
        place.apply(q)
    });
    Ok((to, floor, how))
}

/// The submittal files that may hold sprinkler shop drawings: the given files, and the files below
/// the given folders (or, none given, below the folders beside the prints named like submittals)
/// whose path names fire protection.
pub(crate) fn shop_drawing_files(given: &[String], prints: &str) -> Vec<String> {
    let rp = &rules().pages;
    let fp_words = cadaddy_mepfix::wordrules::words("fp.shop_drawing_path_words");
    let is_pdf = |p: &std::path::Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf"));
    let names_fp = |p: &std::path::Path| {
        let up = p.to_string_lossy().to_ascii_uppercase();
        fp_words.iter().any(|w| up.contains(w))
    };
    let mut out: Vec<String> = Vec::new();
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for g in given {
        let p = std::path::Path::new(g.trim());
        if p.is_file() && is_pdf(p) {
            out.push(p.to_string_lossy().to_string());
        } else if p.is_dir() {
            roots.push(p.to_path_buf());
        }
    }
    if given.is_empty() {
        let prints = std::path::Path::new(prints);
        let folder = if prints.is_file() { prints.parent().map(std::path::Path::to_path_buf) } else { Some(prints.to_path_buf()) };
        let words = cadaddy_mepfix::wordrules::words("fp.submittal_folder_words");
        let named = |p: &std::path::Path| {
            let up = p.file_name().map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
            words.iter().any(|w| up.contains(w))
        };
        // Folders inside the prints folder and beside it named like submittals.
        for dir in folder.iter().cloned().chain(folder.as_deref().and_then(std::path::Path::parent).map(std::path::Path::to_path_buf)) {
            let mut kids: Vec<std::path::PathBuf> =
                std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir() && named(p)).collect();
            kids.sort();
            roots.extend(kids.into_iter().take(16));
        }
    }
    fn walk(dir: &std::path::Path, depth: usize, max_depth: usize, found: &mut Vec<std::path::PathBuf>, cap: usize) {
        if depth > max_depth || found.len() >= cap {
            return;
        }
        let mut items: Vec<std::path::PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        items.sort();
        for p in items.into_iter().take(10_000) {
            if p.is_dir() {
                walk(&p, depth + 1, max_depth, found, cap);
            } else if found.len() < cap {
                found.push(p);
            }
        }
    }
    for r in roots {
        let mut found = Vec::new();
        walk(&r, 0, rp.max_depth, &mut found, 20_000);
        let all_fp = names_fp(&r);
        out.extend(
            found.into_iter().filter(|p| is_pdf(p) && (all_fp || names_fp(p.strip_prefix(&r).unwrap_or(p)))).map(|p| p.to_string_lossy().to_string()),
        );
    }
    let mut seen = BTreeSet::new();
    out.retain(|f| seen.insert(f.clone()));
    out.truncate(rp.max_files);
    out
}

/// The sprinkler shop-drawing pages among `files`, each put onto its floor's architectural plan.
fn read_shop_drawings(out: &mut FpRead, d: &Drawing, sheets: &[SheetRef], upf: f64, files: &[String], ceiling_of: &dyn Fn(&str) -> f64) {
    let Some(read_pdf) = super::file::io().and_then(|h| h.read_pdf) else {
        if !files.is_empty() {
            out.warnings.push("no PDF reader to read the shop drawings with".into());
        }
        return;
    };
    let page_words = cadaddy_mepfix::wordrules::words("fp.page_words");
    let floors: Vec<Floor> = sheets
        .iter()
        .filter(|s| s.discipline.eq_ignore_ascii_case("A") && s.builds() && s.kind == "plan")
        .map(|sh| {
            let sub = printmep::sheet_drawing(d, sh);
            let paper = printmep::paper_units_per_foot(&sub, sh);
            let sheet_upf = paper.unwrap_or(upf);
            let k = if paper.is_some() { upf / sheet_upf } else { 1.0 };
            Floor { sh, sheet_upf, k, tags: super::identity::number_texts(&sub), lines: long_lines(&sub, WALL_LINE_FT * sheet_upf) }
        })
        .collect();
    for f in files {
        out.files.push(f.clone());
        let Ok(bytes) = crate::readfile::read(f) else {
            out.warnings.push(format!("{f}: could not be read"));
            continue;
        };
        let name = std::path::Path::new(f).file_stem().map_or_else(|| f.clone(), |s| s.to_string_lossy().to_string());
        let opts = |n: usize| json!({"page": n, "scale": 1, "layout": false, "images": false, "text": true, "markups": false});
        let mut pages = 1usize;
        let mut n = 1usize;
        while n <= pages.min(rules().pages.max_pages) {
            let Ok((page, rep)) = read_pdf(&bytes, &opts(n)) else {
                out.warnings.push(format!("{name} p.{n}: could not be read"));
                break;
            };
            if n == 1 {
                pages = rep.get("pages").and_then(Value::as_u64).map_or(1, |p| usize::try_from(p).unwrap_or(1));
            }
            let source = format!("{} p.{n}", name.chars().take(60).collect::<String>());
            n += 1;
            let texts = cadaddy_mepfix::sheet_facts::texts(&page);
            if !texts.iter().any(|t| {
                let up = t.value.to_ascii_uppercase();
                page_words.iter().any(|w| up.contains(w))
            }) {
                continue;
            }
            let fp = read_page(&page, 1.0);
            if fp.heads.is_empty() {
                continue;
            }
            let title_level = cadaddy_mepfix::sheet_facts::identify_sheet(&texts)
                .map(|id| cadaddy_mepfix::printset::level_of_title(&id.title))
                .or_else(|| texts.iter().map(|t| cadaddy_mepfix::printset::level_of_title(&t.value)).find(|l| !l.is_empty()))
                .filter(|l| !l.is_empty());
            match place_page(&page, &floors, title_level) {
                Ok((to, floor, how)) => add_source(out, &fp, &*to, upf, &levels_of(floor.sh), &source, "shop_drawing", how, ceiling_of),
                Err(why) => out.unplaced.push((source, why)),
            }
        }
    }
}

/// Read the set's fire protection: the FP plans among `sheets` of `d` and the sprinkler shop
/// drawings among `submittals` (none given: the submittals folders beside `prints`).
pub(crate) fn read(d: &Drawing, sheets: &[SheetRef], upf: f64, submittals: &[String], prints: &str, ceiling_of: &dyn Fn(&str) -> f64) -> FpRead {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut out = FpRead::default();
    read_set_sheets(&mut out, d, sheets, upf, ceiling_of);
    let files = shop_drawing_files(submittals, prints);
    read_shop_drawings(&mut out, d, sheets, upf, &files, ceiling_of);
    if !out.mep.runs.is_empty() || !out.mep.terminals.is_empty() {
        ensure_system(&mut out.mep);
    }
    out
}

/// The ceiling height (feet) of each level the spec knows, else the rules' default.
pub(crate) fn ceiling_lookup(sp: Option<&cadaddy_walk3d::BuildingSpec>) -> impl Fn(&str) -> f64 + '_ {
    let default = rules().heights.default_ceiling_ft;
    move |level: &str| {
        sp.and_then(|s| s.levels.iter().find(|l| l.name.eq_ignore_ascii_case(level)))
            .map(|l| l.ceiling_ft)
            .filter(|c| c.is_finite() && *c > 0.0)
            .unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Hatch, HatchLoop, LwPolyline, Space, Text};
    use cadaddy_geom::{PolyVertex, Vec2, Vec3};

    fn add(d: &mut Drawing, kind: EntityKind) {
        let _ = d.add(&Space::Model, Common::default(), kind);
    }
    fn circle(d: &mut Drawing, c: P, r: f64) {
        add(d, EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(c[0], c[1], 0.0), radius: r }));
    }
    fn line(d: &mut Drawing, a: P, b: P) {
        add(d, EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
    }
    fn text(d: &mut Drawing, at: P, t: &str, h: f64) {
        add(
            d,
            EntityKind::Text(Text {
                insert: Vec3::new(at[0], at[1], 0.0),
                align_pt: None,
                height: h,
                value: t.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }),
        );
    }
    fn ring(pts: &[P]) -> Vec<PolyVertex> {
        pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect()
    }
    fn solid(pts: &[P]) -> EntityKind {
        EntityKind::Hatch(Box::new(Hatch {
            pattern: "SOLID".into(),
            solid: true,
            loops: vec![HatchLoop { vertices: ring(pts), outer: true, sources: Vec::new(), curves: Vec::new() }],
            scale: 1.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        }))
    }
    /// A filled bar from `a` to `b`, `w` wide (axis-aligned), as a solid fill.
    fn bar(d: &mut Drawing, a: P, b: P, w: f64) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        let (nx, ny) = (-dy / l * w / 2.0, dx / l * w / 2.0);
        let pts = [[a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny], [b[0] - nx, b[1] - ny], [a[0] - nx, a[1] - ny]];
        add(d, solid(&pts));
    }
    /// A head symbol: a circle, crossed when `marked`, filled when `solid`.
    fn head(d: &mut Drawing, c: P, r: f64, fill: Fill) {
        circle(d, c, r);
        match fill {
            Fill::Marked => {
                line(d, [c[0] - 0.7 * r, c[1] - 0.7 * r], [c[0] + 0.7 * r, c[1] + 0.7 * r]);
                line(d, [c[0] - 0.7 * r, c[1] + 0.7 * r], [c[0] + 0.7 * r, c[1] - 0.7 * r]);
            }
            Fill::Solid => {
                let pts: Vec<P> =
                    (0..16).map(|i| f64::from(i) * std::f64::consts::TAU / 16.0).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect();
                add(d, solid(&pts));
            }
            Fill::Plain => {}
        }
    }

    /// A page in paper inches: a legend (marked = residential pendent, plain = sidewall), five
    /// heads on a branch line, a heavier main the branch tees off, a wall bar of the main's
    /// weight that one branch end stops at, size labels, and a head table in the given order.
    fn page(qty_first: bool, table_pendents: usize) -> Drawing {
        let mut d = Drawing::new_imperial();
        let r = 0.06;
        // Legend, top left.
        text(&mut d, [1.0, 20.0], "SPRINKLER LEGEND", 0.12);
        head(&mut d, [1.1, 19.55], r, Fill::Marked);
        text(&mut d, [1.4, 19.5], "QUICK RESPONSE RESIDENTIAL PENDENT", 0.1);
        head(&mut d, [1.1, 19.25], r, Fill::Plain);
        text(&mut d, [1.4, 19.2], "HORIZONTAL SIDEWALL", 0.1);
        // Heads: four pendents and a sidewall on one branch along y = 10.
        let xs = [5.0, 7.0, 9.0, 11.0];
        for x in xs {
            head(&mut d, [x, 10.0], r, Fill::Marked);
        }
        head(&mut d, [13.0, 10.0], r, Fill::Plain);
        // An unrelated small circle (a different symbol) not in the legend: solid, no head.
        head(&mut d, [20.0, 15.0], r, Fill::Solid);
        // The branch, head to head, and its riser nipple from the main at y = 8.
        for w in [5.0, 7.0, 9.0, 11.0, 13.0].windows(2) {
            bar(&mut d, [w[0], 10.0], [w[1], 10.0], 0.02);
        }
        bar(&mut d, [5.0, 8.0], [5.0, 10.0], 0.02);
        // The main, heavier: from x = 2 to x = 5 along y = 8, then on to x = 5... a second piece
        // straight on to x = 3 (continuing), and a wall bar of the same weight the branch's far end
        // runs into (x = 13.1 ... ) that nothing else joins.
        bar(&mut d, [2.0, 8.0], [5.0, 8.0], 0.05);
        bar(&mut d, [0.5, 8.0], [2.0, 8.0], 0.05);
        bar(&mut d, [13.06, 6.0], [13.06, 14.0], 0.05);
        // A bar far away with nothing on it.
        bar(&mut d, [25.0, 2.0], [30.0, 2.0], 0.02);
        text(&mut d, [3.0, 8.1], "2-1/2\"", 0.08);
        text(&mut d, [6.0, 10.08], "1\"", 0.08);
        // The head table, bottom right.
        text(&mut d, [20.0, 6.0], "SPRINKLER HEAD SCHEDULE", 0.12);
        let rows: [(&str, String); 2] = [("QUICK RESPONSE RESIDENTIAL PENDENT", table_pendents.to_string()), ("HORIZONTAL SIDEWALL", "1".into())];
        if qty_first {
            text(&mut d, [20.0, 5.6], "QTY", 0.1);
            text(&mut d, [21.0, 5.6], "TYPE", 0.1);
            text(&mut d, [27.0, 5.6], "K", 0.1);
            for (i, (label, n)) in rows.iter().enumerate() {
                let y = 5.3 - 0.3 * i as f64;
                text(&mut d, [20.0, y], n, 0.1);
                text(&mut d, [21.0, y], label, 0.1);
                text(&mut d, [27.0, y], "4.9", 0.1);
            }
        } else {
            for (i, (label, n)) in rows.iter().enumerate() {
                let y = 5.6 - 0.3 * i as f64;
                text(&mut d, [20.0, y], label, 0.1);
                text(&mut d, [25.0, y], "155", 0.1);
                text(&mut d, [26.0, y], n, 0.1);
            }
        }
        d
    }

    fn by_kind(p: &PageRead) -> BTreeMap<&str, usize> {
        let mut m = BTreeMap::new();
        for h in &p.heads {
            *m.entry(h.kind).or_default() += 1;
        }
        m
    }

    #[test]
    fn the_shipped_rules_read() {
        let r: Rules = serde_json::from_str(RULES).expect("printfp.json");
        assert!(r.symbols.head_radius_in[0] > 0.0 && r.symbols.head_radius_in[1] > r.symbols.head_radius_in[0]);
        assert!(r.bars.class_step > 1.0 && r.bars.min_aspect > 1.0 && r.sizes.branch_in > 0.0 && r.sizes.main_in > r.sizes.branch_in);
        assert!(r.pages.min_tags >= 3 && r.pages.max_files > 0 && r.heights.default_ceiling_ft > 0.0);
        for s in [&r.symbols.source, &r.bars.source, &r.sizes.source, &r.heights.source, &r.pages.source] {
            assert!(!s.trim().is_empty());
        }
    }

    #[test]
    fn heads_are_typed_by_the_legend_and_counted_against_the_head_table_in_either_column_order() {
        for qty_first in [false, true] {
            let p = read_page(&page(qty_first, 4), 1.0);
            assert_eq!(p.legend.len(), 2, "{:?}", p.legend);
            assert_eq!(
                by_kind(&p),
                BTreeMap::from([("sprinkler_pendent", 4), ("sprinkler_sidewall", 1)]),
                "legend symbols and the solid circle are no heads"
            );
            let t: Vec<(&str, usize)> = p.table.iter().map(|r| (r.label.as_str(), r.count)).collect();
            assert_eq!(t, vec![("QUICK RESPONSE RESIDENTIAL PENDENT", 4), ("HORIZONTAL SIDEWALL", 1)], "qty first: {qty_first}");
        }
    }

    #[test]
    fn pipe_is_the_bars_the_heads_sit_on_and_what_ends_on_them() {
        let p = read_page(&page(false, 4), 1.0);
        let branch: Vec<&PagePipe> = p.pipes.iter().filter(|x| x.branch).collect();
        let mains: Vec<&PagePipe> = p.pipes.iter().filter(|x| !x.branch).collect();
        // Four head-to-head pieces and the nipple from the main; the lone far bar is no pipe.
        assert_eq!(branch.len(), 5, "{:?}", p.pipes);
        // The main's labelled piece and the piece straight on from it; the wall bar one branch end
        // stops at is no pipe.
        assert_eq!(mains.len(), 2, "{:?}", p.pipes);
        assert!(mains.iter().all(|m| (m.a[1] - 8.0).abs() < 1e-6 && (m.size_in - 2.5).abs() < 1e-9), "{mains:?}");
        assert!(mains.iter().any(|m| m.size_source == "tag") && mains.iter().any(|m| m.size_source.contains("same bar weight")));
        assert!(branch.iter().all(|b| (b.size_in - 1.0).abs() < 1e-9), "{branch:?}");
    }

    #[test]
    fn with_no_legend_circles_of_a_heads_size_are_heads_and_hostile_pages_read_nothing() {
        let mut d = Drawing::new_imperial();
        for x in [1.0, 2.0, 3.0] {
            circle(&mut d, [x, 1.0], 0.05);
        }
        circle(&mut d, [5.0, 5.0], 0.25); // a grid bubble: too big
        let p = read_page(&d, 1.0);
        assert_eq!(p.heads.len(), 3);
        assert!(p.heads.iter().all(|h| h.label.is_empty()));
        let mut bad = Drawing::new_imperial();
        circle(&mut bad, [f64::NAN, 1.0], 0.05);
        circle(&mut bad, [1.0, 1.0], f64::INFINITY);
        bar(&mut bad, [0.0, 0.0], [0.0, 0.0], 0.02);
        add(
            &mut bad,
            EntityKind::LwPolyline(LwPolyline {
                vertices: ring(&[[0.0, 0.0], [f64::NAN, 1.0], [1.0, 1.0], [1.0, 0.0]]),
                closed: true,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
        );
        let p = read_page(&bad, 1.0);
        assert!(p.heads.is_empty() && p.pipes.is_empty());
    }

    #[test]
    fn a_page_with_too_few_room_numbers_is_not_placed() {
        let d = page(false, 4);
        let Err(why) = place_page(&d, &[], None) else { panic!("placed") };
        assert!(why.contains("room number"), "{why}");
    }

    #[test]
    fn shop_drawing_files_are_the_fire_protection_ones() {
        let base = std::env::temp_dir().join(format!("cadaddy_printfp_files_{}", std::process::id()));
        let prints = base.join("Drawings");
        let subs = base.join("Project Submittals");
        let fp = subs.join("21 13 13 Fire Sprinklers");
        for d in [&prints, &fp, &subs.join("26 Electrical")] {
            std::fs::create_dir_all(d).expect("dir");
        }
        for f in [
            fp.join("Shop Drawings.pdf"),
            subs.join("26 Electrical").join("Panels.pdf"),
            subs.join("Wet Sprinkler Heads.pdf"),
            prints.join("Set.pdf"),
        ] {
            std::fs::write(f, b"%PDF").expect("file");
        }
        let found = shop_drawing_files(&[], &prints.to_string_lossy());
        let names: Vec<String> =
            found.iter().map(|f| std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).collect();
        assert_eq!(names, vec!["Shop Drawings.pdf", "Wet Sprinkler Heads.pdf"], "{found:?}");
        // Given: a file is taken as it is; a folder's fire protection files only.
        let given = shop_drawing_files(&[subs.join("26 Electrical").join("Panels.pdf").to_string_lossy().to_string()], "");
        assert_eq!(given.len(), 1);
        let none = shop_drawing_files(&[subs.join("26 Electrical").to_string_lossy().to_string()], &prints.to_string_lossy());
        assert!(none.is_empty(), "{none:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_bar_written_as_triangles_is_one_bar_however_it_is_cut() {
        let (a, b, c, d) = ([0.0, 0.0], [4.0, 0.0], [4.0, 0.05], [0.0, 0.05]);
        let m = [2.0, 0.025];
        // Two triangles, and a fan round the middle; a lone triangle stays a triangle.
        for rings in [vec![vec![a, b, c], vec![a, c, d]], vec![vec![m, a, b], vec![m, b, c], vec![m, c, d], vec![m, d, a]]] {
            let (quads, taken) = triangle_quads(&rings);
            assert_eq!(quads.len(), 1, "{rings:?}");
            assert!(taken.iter().all(|t| *t));
            let bar = quads.first().and_then(|q| as_bar(q)).expect("a bar");
            assert!((bar.2 - 0.05).abs() < 1e-9 && (dist(bar.0, bar.1) - 4.0).abs() < 1e-9, "{bar:?}");
        }
        // A slanted bar cut in horizontal slices (degenerate triangles at its ends, cut points on its sides).
        let (u, n) = ([0.6, 0.8], [-0.8 * 0.01, 0.6 * 0.01]);
        let at = |t: f64, s: f64| [u[0] * t + n[0] * s, u[1] * t + n[1] * s];
        let (p0, p1, p2, p3) = (at(0.0, -1.0), at(0.0, 1.0), at(3.0, 1.0), at(3.0, -1.0));
        let y1 = p1[1].max(p0[1]);
        let y2 = p3[1].min(p2[1]);
        let on = |p: P, q: P, y: f64| [p[0] + (q[0] - p[0]) * (y - p[1]) / (q[1] - p[1]), y];
        let (l1, r1) = (on(p1, p2, y1.max(p0[1])), on(p0, p3, y1));
        let (l2, r2) = (on(p1, p2, y2), on(p0, p3, y2.min(p2[1])));
        let lo = if p0[1] < p1[1] { p0 } else { p1 };
        let hi = if p2[1] > p3[1] { p2 } else { p3 };
        let sliced = vec![vec![lo, lo, r1], vec![lo, r1, l1], vec![l1, r1, r2], vec![l1, r2, l2], vec![l2, r2, hi], vec![l2, hi, hi]];
        let (quads, _) = triangle_quads(&sliced);
        let bar = quads.first().and_then(|q| as_bar(q)).expect("the sliced bar");
        assert!((bar.2 - 0.02).abs() < 1e-6 && (dist(bar.0, bar.1) - 3.0).abs() < 1e-6, "{bar:?}");
        let (quads, taken) = triangle_quads(&[vec![a, b, c], vec![[9.0, 9.0], [10.0, 9.0], [9.0, 10.0]]]);
        assert!(quads.is_empty() && taken.iter().all(|t| !t));
        // Two texts at one place are one; the same text elsewhere is another.
        let t = |v: &str, x: f64| cadaddy_mepfix::sheet_facts::TextItem { value: v.into(), at: Vec2::new(x, 0.0), height: 0.1, rotation: 0.0 };
        assert_eq!(unique_texts(vec![t("101", 0.0), t("101", 0.02), t("101", 3.0), t("102", 0.0)]).len(), 3);
    }

    #[test]
    fn sizes_and_heights_read_as_printed_and_by_the_rules() {
        assert_eq!(size_label(1.0), "1\"");
        assert_eq!(size_label(2.5), "2-1/2\"");
        assert_eq!(size_label(0.75), "3/4\"");
        assert!((head_height("sprinkler_sidewall", 9.0) - (9.0 - 4.0 / 12.0)).abs() < 1e-9);
        assert!((head_height("sprinkler_pendent", 9.0) - 9.0).abs() < 1e-9);
        assert!((head_height("sprinkler_upright", 10.0) - 9.75).abs() < 1e-9);
    }
}
