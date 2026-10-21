//! Wall reader: wall assemblies from the wall type legend and the wall type tags on the
//! architectural floor plans.
//!
//! - **Legend** (the sheet titled `… WALL TYPES …` / `… PARTITION …`): each tag in its symbol and
//!   the lines beside and below it — the boards per side and their count (`(2) LAYERS 5/8" TYPE X
//!   GYP. BD.`, `… EACH SIDE`, `SIDE 1:` / `SIDE A:` / `ONE SIDE:`), studs (size, gauge, spacing),
//!   cavity insulation, resilient and furring channels, CMU, sheathing, shaft liner, the height
//!   rule, fire and STC ratings and the overall thickness — into an ordered layer stack.
//! - **Walls** (`… FLOOR PLAN` sheets): parallel face lines on the wall layers a wall's thickness
//!   apart, pieces on one line joined across door openings (the gap kept as an opening).
//! - **Tags**: legend tags' text on the plan; a leader from the symbol is followed to the wall it
//!   lands on, otherwise the nearest wall takes it. A wall without a tag gets the likeliest type
//!   (same thickness; the type of the tagged walls in line with it; exterior types on the
//!   outline; the commonest) and is flagged as a guess.
//! - **Heights**: the type's rule applied to the ceilings of the rooms either side (Q264's ceiling
//!   reader; the higher one) and the underside of the deck above (Q263's slabs).
//!
//! Everything is in building feet (sheet alignment from Q255).

use crate::ceiling_reader::CeilingResult;
use crate::sheet_reader::{self, Alignment, SheetMap};
use crate::struct_reader::StructuralModel;
use crate::truth::{Truth, WallTypeLayer};
use cadaddy_doc::{Drawing, EntityKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Most wall face lines read from one sheet (hostile input).
const MAX_LINES: usize = 50_000;
/// Most texts read from one sheet.
const MAX_TEXTS: usize = 50_000;

/// A wall type read from the legend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadType {
    pub tag: String,
    pub name: String,
    /// Side 1's face to side 2's (truth's layout).
    pub layers: Vec<WallTypeLayer>,
    /// Overall as printed, inches.
    pub printed_in: Option<f64>,
    /// `ceiling`, `above_ceiling`, `deck`, `full`.
    pub height: Option<String>,
    pub fire_rating: Option<String>,
    pub stc: Option<u32>,
    pub external: bool,
    /// Lines not understood.
    pub notes: Vec<String>,
}

impl ReadType {
    /// Overall thickness: as printed, else the layers'.
    pub fn thickness_in(&self) -> f64 {
        self.printed_in.unwrap_or_else(|| self.layers.iter().filter(|l| !l.in_cavity).map(|l| l.thickness_in).sum())
    }
}

/// A wall found on a floor plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundWall {
    pub level: String,
    pub a_ft: [f64; 2],
    pub b_ft: [f64; 2],
    /// Between its faces, inches.
    pub thickness_in: f64,
    pub wall_type: Option<String>,
    /// A tag on the plan names its type (otherwise `guess` says why it's the likeliest).
    pub tagged: bool,
    pub guess: Option<String>,
    pub height_ft: Option<f64>,
    /// Where the height came from: `ceiling`, `deck`, `no ceiling beside it: deck`, `unknown`.
    pub height_source: String,
    /// Door openings: from and to along it, feet.
    pub openings: Vec<[f64; 2]>,
    /// Room numbers either side.
    pub rooms: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct WallResult {
    pub types: Vec<ReadType>,
    pub walls: Vec<FoundWall>,
}

// ---- Text --------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Txt {
    s: String,
    at: [f64; 2],
    h: f64,
    /// On a grid layer.
    grid: bool,
}

fn is_grid(e: &cadaddy_doc::Entity) -> bool {
    e.common.layer.to_ascii_uppercase().contains("GRID")
}

fn texts(d: &Drawing) -> Vec<Txt> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                Some(Txt { s: t.value.trim().to_string(), at: [p.x, p.y], h: t.height, grid: is_grid(e) })
            }
            EntityKind::MText(t) => Some(Txt { s: t.contents.trim().to_string(), at: [t.insert.x, t.insert.y], h: t.height, grid: is_grid(e) }),
            _ => None,
        })
        .filter(|t| !t.s.is_empty() && t.at.iter().all(|v| v.is_finite()) && t.h.is_finite())
        .take(MAX_TEXTS)
        .collect()
}

/// A wall type tag: `A`, `1B`, `P3`, `W-4`, `X1` (short, letters and digits, a letter in it).
fn tag_like(s: &str) -> bool {
    let n = s.chars().count();
    (1..=4).contains(&n)
        && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        && s.chars().any(|c| c.is_ascii_uppercase())
        && !s.starts_with('-')
        && !s.ends_with('-')
}

// ---- Numbers -----------------------------------------------------------------------------------

/// `5/8`, `3`, `3-5/8`, `3 5/8` → inches.
fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    let frac = |f: &str| -> Option<f64> {
        let (a, b) = f.split_once('/')?;
        let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        (b > 0.0).then(|| a / b)
    };
    if let Some(v) = frac(s) {
        return Some(v);
    }
    for sep in ['-', ' '] {
        if let Some((w, f)) = s.split_once(sep)
            && let (Ok(w), Some(f)) = (w.trim().parse::<f64>(), frac(f))
        {
            return Some(w + f);
        }
    }
    s.parse::<f64>().ok()
}

/// Every inch dimension in a line (`5/8"`, `3-5/8"`, `3 5/8"`, `6"`), with where it starts.
fn inch_values(s: &str) -> Vec<(usize, f64)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        if c != '"' {
            continue;
        }
        // Back over digits, '/', and one '-' or ' ' between a whole number and a fraction.
        let mut j = i;
        let mut seps = 0;
        while j > 0 {
            let p = b.get(j - 1).copied().unwrap_or(b'x');
            if p.is_ascii_digit() || p == b'/' || p == b'.' {
                j -= 1;
            } else if (p == b'-' || p == b' ')
                && seps == 0
                && j >= 2
                && b.get(j - 2).is_some_and(|q| q.is_ascii_digit())
                && s.get(j..i).is_some_and(|r| r.contains('/'))
            {
                seps += 1;
                j -= 1;
            } else {
                break;
            }
        }
        if let Some(v) = s.get(j..i).and_then(number).filter(|v| v.is_finite() && *v > 0.0 && *v < 100.0) {
            out.push((j, v));
        }
    }
    out
}

fn count_of(s: &str) -> usize {
    let u = s.to_ascii_uppercase();
    for (w, n) in [("ONE ", 1), ("TWO ", 2), ("THREE ", 3), ("FOUR ", 4)] {
        if u.contains(&format!("{w}LAYER")) {
            return n;
        }
    }
    // `(2) LAYERS`, `2 LAYERS`
    if let Some(at) = u.find("LAYER") {
        let head = u.get(..at).unwrap_or("").trim_end().trim_end_matches(')');
        let digits: String = head.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
        if let Ok(n) = digits.parse::<usize>() {
            return n.clamp(1, 8);
        }
    }
    1
}

/// A side's item → its layers, outer face first.
fn side_item(item: &str) -> Vec<WallTypeLayer> {
    let u = item.to_ascii_uppercase();
    let Some((_, th)) = inch_values(&u).first().copied() else { return Vec::new() };
    let material = if u.contains("SHAFT") {
        "shaftliner"
    } else if u.contains("SHEATHING") {
        "sheathing"
    } else if u.contains("RESILIENT") || u.contains("R.C.") {
        "resilient_channel"
    } else if u.contains("FURRING") || u.contains("HAT CHANNEL") {
        "furring"
    } else if u.contains("GYP") || u.contains("GWB") || u.contains("BOARD") {
        if u.contains("TYPE X") || u.contains("TYPE 'X'") || u.contains("TYPE \"X\"") || u.contains("FIRE-RATED") {
            "gyp_x"
        } else if u.contains("MOISTURE") || u.contains(" MR ") || u.contains("MR GYP") || u.contains("MR GWB") {
            "gyp_mr"
        } else if u.contains("ABUSE") || u.contains("IMPACT") || u.contains(" AR ") || u.contains("AR GYP") || u.contains("AR GWB") {
            "gyp_ar"
        } else {
            "gyp"
        }
    } else {
        return Vec::new();
    };
    let n = if material.starts_with("gyp") { count_of(&u) } else { 1 };
    (0..n).map(|_| WallTypeLayer { material: material.into(), thickness_in: th, side: String::new(), ..WallTypeLayer::default() }).collect()
}

/// Items of a side list: split at commas that start a new dimension.
fn side_items(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect()
}

const SIDE1: [&str; 3] = ["SIDE 1:", "SIDE A:", "ONE SIDE:"];
const SIDE2: [&str; 3] = ["SIDE 2:", "SIDE B:", "OTHER SIDE:"];

/// Parse one legend block (lines top-down; the first is the name).
pub fn parse_type(tag: &str, lines: &[String]) -> ReadType {
    let mut t = ReadType { tag: tag.into(), ..ReadType::default() };
    let (mut s1, mut core, mut s2): (Vec<WallTypeLayer>, Vec<WallTypeLayer>, Vec<WallTypeLayer>) = (Vec::new(), Vec::new(), Vec::new());
    for (i, line) in lines.iter().enumerate() {
        let u = line.to_ascii_uppercase();
        let u = u.trim();
        if i == 0 {
            t.name = line.trim().to_string();
            t.external = u.contains("EXTERIOR");
            continue;
        }
        let strip = |labels: &[&str]| labels.iter().find_map(|l| u.strip_prefix(l)).map(str::trim);
        if let Some(rest) = strip(&SIDE1) {
            s1.extend(side_items(rest).iter().flat_map(|x| side_item(x)));
        } else if let Some(rest) = strip(&SIDE2) {
            s2.extend(side_items(rest).iter().flat_map(|x| side_item(x)));
        } else if u.ends_with("EACH SIDE") || u.ends_with("BOTH SIDES") {
            let rest = u.trim_end_matches("EACH SIDE").trim_end_matches("BOTH SIDES");
            let items: Vec<WallTypeLayer> = side_items(rest).iter().flat_map(|x| side_item(x)).collect();
            s1.extend(items.iter().cloned());
            s2.extend(items);
        } else if u.contains("STUD") {
            let v = inch_values(u);
            let depth = v.first().map(|x| x.1).unwrap_or(3.625);
            let spacing = v.iter().skip(1).map(|x| x.1).find(|x| *x >= 8.0);
            let gauge = u.split(|c: char| !c.is_ascii_alphanumeric() && c != '.').collect::<Vec<_>>().windows(2).find_map(|w| match w {
                [n, g] if g.starts_with("GA") => n.parse::<u32>().ok(),
                _ => None,
            });
            core.push(WallTypeLayer {
                material: "stud".into(),
                thickness_in: depth,
                side: "core".into(),
                gauge,
                spacing_in: spacing,
                in_cavity: false,
            });
        } else if u.contains("CMU") || u.contains("MASONRY") {
            let th = inch_values(u).first().map_or(8.0, |x| x.1);
            core.push(WallTypeLayer { material: "cmu".into(), thickness_in: th, side: "core".into(), ..WallTypeLayer::default() });
        } else if u.contains("BATT") || u.contains("INSUL") {
            let th = inch_values(u).first().map_or(3.5, |x| x.1);
            core.push(WallTypeLayer {
                material: "insulation".into(),
                thickness_in: th,
                side: "core".into(),
                in_cavity: true,
                ..WallTypeLayer::default()
            });
        } else if u.contains("FULL HEIGHT") {
            t.height = Some("full".into());
        } else if u.contains("CEILING") || u.contains("CLG") {
            let above = u.contains("ABOVE") || u.contains('+');
            t.height = Some(if above { "above_ceiling" } else { "ceiling" }.into());
        } else if u.contains("DECK") || u.contains("STRUCTURE") || u.contains("U/S") {
            t.height = Some("deck".into());
        } else if u.contains("STC") {
            t.stc = u.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<u32>().ok()).find(|n| (20..=90).contains(n));
        } else if u.contains("RATING") || u.contains("RATED") || u.contains("HOUR") || u.contains(" HR") {
            if u.contains("NONE") || u.contains("NON-RATED") {
                t.fire_rating = None;
            } else if let Some(h) = u.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<u32>().ok()).find(|n| (1..=4).contains(n)) {
                t.fire_rating = Some(format!("{h} HR"));
            }
        } else if u.starts_with("OVERALL") || u.contains("THICKNESS") || u.starts_with("TOTAL") {
            t.printed_in = inch_values(u).first().map(|x| x.1);
        } else if u.contains("SHEATHING") || u.contains("GYP") || u.contains("GWB") {
            core.extend(side_item(u));
        } else {
            t.notes.push(line.trim().to_string());
        }
    }
    // Side 2 was listed outer face first: it goes in inner face first.
    s2.reverse();
    for (side, list) in [("1", &mut s1), ("2", &mut s2)] {
        for l in list.iter_mut() {
            l.side = side.into();
        }
    }
    t.layers = s1.into_iter().chain(core).chain(s2).collect();
    t
}

/// The legend sheet's wall types: each tag (a tag-like text inside a symbol) and the lines to its
/// right below it.
fn read_legend(d: &Drawing) -> Vec<ReadType> {
    let all = texts(d);
    let symbols: Vec<([f64; 2], f64)> = symbol_centres(d);
    let tags: Vec<&Txt> = all.iter().filter(|t| tag_like(&t.s) && symbols.iter().any(|(c, r)| dist(*c, t.at) < r.max(t.h) * 0.6)).collect();
    let mut blocks: BTreeMap<usize, Vec<&Txt>> = BTreeMap::new();
    for t in all.iter().filter(|t| !tag_like(&t.s) && !t.s.contains(" = ")) {
        let owner = tags
            .iter()
            .enumerate()
            .filter(|(_, g)| t.at[0] > g.at[0] && t.at[0] - g.at[0] < 25.0 * g.h && g.at[1] >= t.at[1] - g.h)
            .min_by(|(_, a), (_, b)| (a.at[1] - t.at[1]).total_cmp(&(b.at[1] - t.at[1])))
            .map(|(i, _)| i);
        if let Some(i) = owner {
            blocks.entry(i).or_default().push(t);
        }
    }
    let mut out: Vec<ReadType> = Vec::new();
    for (i, mut lines) in blocks {
        let Some(g) = tags.get(i) else { continue };
        lines.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]));
        // Lines run on with the block's spacing; a gap twice that ends it.
        let mut kept: Vec<String> = Vec::new();
        let mut last: Option<f64> = None;
        for l in lines {
            if let Some(y) = last
                && y - l.at[1] > 4.0 * l.h.max(1e-9)
            {
                break;
            }
            last = Some(l.at[1]);
            kept.push(l.s.clone());
        }
        let t = parse_type(&g.s, &kept);
        let has_core = t.layers.iter().any(|l| l.material == "stud" || l.material == "cmu");
        if has_core && !out.iter().any(|o| o.tag == t.tag) {
            out.push(t);
        }
    }
    out
}

/// Centres and sizes (half-width) of closed polylines and circles: tag symbols.
fn symbol_centres(d: &Drawing) -> Vec<([f64; 2], f64)> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some(([c.center.x, c.center.y], c.radius)),
            EntityKind::LwPolyline(p) if p.closed && (4..=8).contains(&p.vertices.len()) => {
                let (lo, hi) = p
                    .vertices
                    .iter()
                    .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), v| ([lo[0].min(v.p.x), lo[1].min(v.p.y)], [hi[0].max(v.p.x), hi[1].max(v.p.y)]));
                Some(([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], ((hi[0] - lo[0]).max(hi[1] - lo[1])) / 2.0))
            }
            _ => None,
        })
        .filter(|(c, r)| c.iter().all(|v| v.is_finite()) && r.is_finite())
        .take(MAX_TEXTS)
        .collect()
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

// ---- Walls on a plan ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Seg {
    a: [f64; 2],
    b: [f64; 2],
}

/// Distance from `p` to segment a–b, and how far along (feet from a).
fn to_seg(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let len = dist(a, b);
    if len <= 1e-12 {
        return (dist(p, a), 0.0);
    }
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let s = ((p[0] - a[0]) * u[0] + (p[1] - a[1]) * u[1]).clamp(0.0, len);
    (dist(p, [a[0] + u[0] * s, a[1] + u[1] * s]), s)
}

/// Face pairs → wall pieces (centreline, thickness inches), then pieces on one line with the same
/// thickness joined across door-sized gaps that a door swings in (`hinges`: arc centres).
fn find_walls(lines: &[Seg], hinges: &[[f64; 2]], level: &str) -> Vec<FoundWall> {
    let mut pieces: Vec<FoundWall> = Vec::new();
    for (i, f) in lines.iter().enumerate() {
        let lf = dist(f.a, f.b);
        if lf < 1.0 {
            continue;
        }
        let u = [(f.b[0] - f.a[0]) / lf, (f.b[1] - f.a[1]) / lf];
        let n = [-u[1], u[0]];
        // The nearest parallel partner on its left, overlapping it.
        let mut best: Option<(f64, f64, f64)> = None;
        for (j, g) in lines.iter().enumerate() {
            if i == j {
                continue;
            }
            let lg = dist(g.a, g.b);
            if lg < 1.0 {
                continue;
            }
            let v = [(g.b[0] - g.a[0]) / lg, (g.b[1] - g.a[1]) / lg];
            if (u[0] * v[1] - u[1] * v[0]).abs() > 0.01 {
                continue;
            }
            let off = (g.a[0] - f.a[0]) * n[0] + (g.a[1] - f.a[1]) * n[1];
            if !(2.0 / 12.0..=16.0 / 12.0).contains(&off) {
                continue;
            }
            let along = |p: [f64; 2]| (p[0] - f.a[0]) * u[0] + (p[1] - f.a[1]) * u[1];
            let (g0, g1) = (along(g.a).min(along(g.b)), along(g.a).max(along(g.b)));
            let (s0, s1) = (g0.max(0.0), g1.min(lf));
            if s1 - s0 < 0.5 {
                continue;
            }
            if best.is_none_or(|b| off < b.0 - 1e-6) {
                best = Some((off, s0, s1));
            }
        }
        let Some((off, s0, s1)) = best else { continue };
        let at = |s: f64| [f.a[0] + u[0] * s + n[0] * off / 2.0, f.a[1] + u[1] * s + n[1] * off / 2.0];
        let w = FoundWall { level: level.into(), a_ft: at(s0), b_ft: at(s1), thickness_in: off * 12.0, ..FoundWall::default() };
        if !pieces.iter().any(|p| same_line(p, &w) && overlap(p, &w) > 0.5) {
            pieces.push(w);
        }
    }
    // Join across openings.
    let mut joined = true;
    while joined {
        joined = false;
        'outer: for i in 0..pieces.len() {
            for j in 0..pieces.len() {
                if i == j {
                    continue;
                }
                let (Some(p), Some(q)) = (pieces.get(i), pieces.get(j)) else { continue };
                if !same_line(p, q) || (p.thickness_in - q.thickness_in).abs() > 0.25 {
                    continue;
                }
                let len = dist(p.a_ft, p.b_ft);
                let u = [(p.b_ft[0] - p.a_ft[0]) / len, (p.b_ft[1] - p.a_ft[1]) / len];
                let along = |x: [f64; 2]| (x[0] - p.a_ft[0]) * u[0] + (x[1] - p.a_ft[1]) * u[1];
                let (q0, q1) = (along(q.a_ft).min(along(q.b_ft)), along(q.a_ft).max(along(q.b_ft)));
                if q0 < len - 1e-6 {
                    continue;
                }
                let gap = q0 - len;
                if !(1.5..=8.0).contains(&gap) {
                    continue;
                }
                // A door hinged at one side of the gap, on the wall.
                let n = [-u[1], u[0]];
                let door = hinges.iter().any(|h| {
                    let d = [h[0] - p.a_ft[0], h[1] - p.a_ft[1]];
                    let s = d[0] * u[0] + d[1] * u[1];
                    let off = (d[0] * n[0] + d[1] * n[1]).abs();
                    off <= p.thickness_in / 24.0 + 0.5 && s >= len - 0.5 && s <= q0 + 0.5
                });
                if !door {
                    continue;
                }
                let mut w = p.clone();
                w.b_ft = [p.a_ft[0] + u[0] * q1, p.a_ft[1] + u[1] * q1];
                w.openings.push([len, q0]);
                w.openings.extend(q.openings.iter().map(|o| [o[0] + q0, o[1] + q0]));
                let (hi, lo) = (i.max(j), i.min(j));
                pieces.remove(hi);
                pieces.remove(lo);
                pieces.push(w);
                joined = true;
                break 'outer;
            }
        }
    }
    pieces
}

/// On one line: parallel, the other's centre within half the thicker's thickness (or 2").
fn same_line(p: &FoundWall, q: &FoundWall) -> bool {
    let len = dist(p.a_ft, p.b_ft);
    let lq = dist(q.a_ft, q.b_ft);
    if len <= 1e-9 || lq <= 1e-9 {
        return false;
    }
    let u = [(p.b_ft[0] - p.a_ft[0]) / len, (p.b_ft[1] - p.a_ft[1]) / len];
    let v = [(q.b_ft[0] - q.a_ft[0]) / lq, (q.b_ft[1] - q.a_ft[1]) / lq];
    if (u[0] * v[1] - u[1] * v[0]).abs() > 0.01 {
        return false;
    }
    let off = ((q.a_ft[0] - p.a_ft[0]) * -u[1] + (q.a_ft[1] - p.a_ft[1]) * u[0]).abs();
    off < (p.thickness_in.max(q.thickness_in) / 24.0).max(2.0 / 12.0)
}

/// How much of the shorter of two walls on one line the other covers (0–1).
fn overlap(p: &FoundWall, q: &FoundWall) -> f64 {
    let len = dist(p.a_ft, p.b_ft);
    if len <= 1e-9 {
        return 0.0;
    }
    let u = [(p.b_ft[0] - p.a_ft[0]) / len, (p.b_ft[1] - p.a_ft[1]) / len];
    let along = |x: [f64; 2]| (x[0] - p.a_ft[0]) * u[0] + (x[1] - p.a_ft[1]) * u[1];
    let (q0, q1) = (along(q.a_ft).min(along(q.b_ft)), along(q.a_ft).max(along(q.b_ft)));
    let o = q1.min(len) - q0.max(0.0);
    (o / len.min(q1 - q0).max(1e-9)).max(0.0)
}

/// Tags on a plan: each legend tag's text, landing where its leader ends (or at the text).
fn plan_tags(d: &Drawing, al: &Alignment, types: &[ReadType]) -> Vec<(String, [f64; 2], bool)> {
    let leaders: Vec<Seg> = d
        .model
        .iter()
        .filter(|e| !e.common.layer.to_ascii_uppercase().contains("WALL") || e.common.layer.to_ascii_uppercase().contains("TAG"))
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some(Seg { a: [l.a.x, l.a.y], b: [l.b.x, l.b.y] }),
            _ => None,
        })
        .take(MAX_LINES)
        .collect();
    let mut out = Vec::new();
    for t in texts(d) {
        // Grid bubbles' labels can look like tags (`A`, `B`…): not those.
        if t.grid || !types.iter().any(|x| x.tag == t.s) {
            continue;
        }
        let h = t.h.max(1e-9);
        // A leader starts just outside the symbol (about 1.5 text heights out) and runs away.
        let leader = leaders
            .iter()
            .filter_map(|l| {
                let (da, db) = (dist(l.a, t.at), dist(l.b, t.at));
                let (near, far) = if da < db { (da, l.b) } else { (db, l.a) };
                (near < 2.8 * h && dist(far, t.at) > near + 0.3 * h && dist(far, t.at) > 2.0 * h).then_some((near, far))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, far)| far);
        let p = leader.unwrap_or(t.at);
        out.push((t.s.clone(), sheet_reader::building_from_model(al, p), leader.is_some()));
    }
    out
}

/// Rooms either side of a wall (numbers), from the ceiling reader's rooms on its level.
fn rooms_beside(w: &FoundWall, cl: &CeilingResult) -> Vec<String> {
    let len = dist(w.a_ft, w.b_ft);
    let mut out: Vec<String> = Vec::new();
    if len <= 1e-9 {
        return out;
    }
    let u = [(w.b_ft[0] - w.a_ft[0]) / len, (w.b_ft[1] - w.a_ft[1]) / len];
    let n = [-u[1], u[0]];
    let off = w.thickness_in / 24.0 + 0.75;
    let steps = ((len / 2.0).ceil() as usize).clamp(1, 1000);
    for i in 0..=steps {
        let s = (0.5 + (len - 1.0) * i as f64 / steps as f64).min(len);
        for side in [-1.0, 1.0] {
            let p = [w.a_ft[0] + u[0] * s + n[0] * off * side, w.a_ft[1] + u[1] * s + n[1] * off * side];
            // The smallest room holding the point (an outline round the whole floor holds them all).
            let area = |r: &&crate::ceiling_reader::FoundRoom| (r.max_ft[0] - r.min_ft[0]) * (r.max_ft[1] - r.min_ft[1]);
            if let Some(r) = cl
                .rooms
                .iter()
                .filter(|r| r.level == w.level && p[0] > r.min_ft[0] && p[0] < r.max_ft[0] && p[1] > r.min_ft[1] && p[1] < r.max_ft[1])
                .min_by(|a, b| area(a).total_cmp(&area(b)))
                && !out.contains(&r.number)
            {
                out.push(r.number.clone());
            }
        }
    }
    out
}

/// `13'-6 1/2"` → feet.
fn feet_inches(s: &str) -> Option<f64> {
    let (f, rest) = s.split_once('\'')?;
    let f: f64 = f.trim().parse().ok()?;
    let rest = rest.trim_start_matches('-').trim();
    let i = rest.split('"').next().map(str::trim).filter(|x| !x.is_empty()).map_or(Some(0.0), number)?;
    let v = if f < 0.0 { f - i / 12.0 } else { f + i / 12.0 };
    v.is_finite().then_some(v)
}

/// What a framing plan's notes say: its level's top of steel and top of slab (elevations, feet).
fn framing_notes(d: &Drawing) -> (Option<f64>, Option<f64>) {
    let mut tos = None;
    let mut top = None;
    for t in texts(d) {
        let u = t.s.to_ascii_uppercase();
        let after = |key: &str| {
            u.find(key)
                .and_then(|i| u.get(i + key.len()..))
                .and_then(|r| feet_inches(r.trim_start_matches([' ', '=']).trim_start_matches("EL").trim()))
        };
        if let Some(v) = after("T.O.S.").or_else(|| after("TOP OF STEEL")) {
            tos = Some(v);
        }
        if let Some(v) = after("TOP OF SLAB") {
            top = Some(v);
        }
    }
    (tos, top)
}

/// Each level's underside of the deck above (it bears on the top of the steel), above the
/// level's floor: from the framing plans' notes, else the structure reader's slabs.
fn deck_heights(notes: &BTreeMap<String, (Option<f64>, Option<f64>)>, st: &StructuralModel) -> BTreeMap<String, f64> {
    let mut levels: Vec<(String, f64, Option<f64>)> = Vec::new();
    for (lv, (tos, top)) in notes {
        let slab = st.slabs.iter().find(|s| &s.level == lv);
        let tos = tos.or_else(|| slab.map(|s| s.top_ft - s.concrete_in / 12.0));
        if let Some(tos) = tos.filter(|v| v.is_finite()) {
            let floor = top.or_else(|| slab.map(|s| s.top_ft)).filter(|v| v.is_finite());
            levels.push((lv.clone(), tos, floor));
        }
    }
    for s in &st.slabs {
        if !levels.iter().any(|l| l.0 == s.level) && s.top_ft.is_finite() {
            levels.push((s.level.clone(), s.top_ft - s.concrete_in / 12.0, Some(s.top_ft)));
        }
    }
    levels.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out = BTreeMap::new();
    // The lowest level's floor is at 0 (slab on grade, not on the framing plans).
    let mut floor = ("1".to_string(), 0.0);
    for (lv, tos, top) in levels {
        let deck = tos - floor.1;
        if deck > 0.0 && deck < 100.0 {
            out.insert(floor.0.clone(), deck);
        }
        floor = (lv, top.unwrap_or(tos));
    }
    out
}

/// The likeliest type of an untagged wall, and why.
fn guess(
    w: &FoundWall,
    tagged: &[FoundWall],
    types: &[ReadType],
    outline: &([f64; 2], [f64; 2]),
    room_names: &BTreeMap<String, String>,
) -> Option<(String, String)> {
    let near = |t: &ReadType| (t.thickness_in() - w.thickness_in).abs() <= 0.25;
    let cands: Vec<&ReadType> = types.iter().filter(|t| near(t)).collect();
    if cands.len() == 1 {
        return cands.first().map(|t| (t.tag.clone(), format!("no tag: the only type {} thick", crate::walls::frac(t.thickness_in(), true))));
    }
    let pool: Vec<&ReadType> = if cands.is_empty() { types.iter().collect() } else { cands };
    // What the rooms either side are: corridors get the corridor type, toilets the plumbing wall,
    // conference rooms the acoustic one.
    for (rooms, words) in [
        (&["CORRIDOR"][..], &["CORRIDOR"][..]),
        (&["MEN", "WOMEN", "TOILET", "RESTROOM"][..], &["PLUMBING", "CHASE", "WET"][..]),
        (&["CONFERENCE", "TRAINING"][..], &["ACOUSTIC", "SOUND"][..]),
    ] {
        if w.rooms.iter().any(|n| room_names.get(n).is_some_and(|name| rooms.iter().any(|r| name.contains(r))))
            && let Some(t) = pool.iter().find(|t| words.iter().any(|x| t.name.to_ascii_uppercase().contains(x)))
        {
            return Some((t.tag.clone(), format!("no tag: beside a {} room, the {} type", rooms.first().copied().unwrap_or(""), t.name)));
        }
    }
    // In line with tagged walls of a candidate type.
    if let Some(t) =
        tagged.iter().filter(|o| o.level == w.level && same_line(o, w)).find_map(|o| pool.iter().find(|t| Some(&t.tag) == o.wall_type.as_ref()))
    {
        return Some((t.tag.clone(), "no tag: the type of the tagged wall in line with it".into()));
    }
    // On the building's outline: an exterior type.
    let (lo, hi) = outline;
    let on_edge = |p: [f64; 2]| (p[0] - lo[0]).abs() < 1.0 || (p[0] - hi[0]).abs() < 1.0 || (p[1] - lo[1]).abs() < 1.0 || (p[1] - hi[1]).abs() < 1.0;
    let mid = [(w.a_ft[0] + w.b_ft[0]) / 2.0, (w.a_ft[1] + w.b_ft[1]) / 2.0];
    let ext: Vec<&&ReadType> = pool.iter().filter(|t| t.external).collect();
    if on_edge(mid) && on_edge(w.a_ft) && on_edge(w.b_ft) {
        if let Some(t) = ext.first() {
            return Some((t.tag.clone(), "no tag: on the building's outline, the exterior type".into()));
        }
    } else if let Some(t) = pool
        .iter()
        // Not beside a corridor (that was tried above): not the corridor's type either.
        .filter(|t| !t.external && !t.name.to_ascii_uppercase().contains("CORRIDOR"))
        .max_by_key(|t| tagged.iter().filter(|o| o.wall_type.as_ref() == Some(&t.tag)).count())
    {
        return Some((t.tag.clone(), "no tag: the commonest interior type that thick".into()));
    }
    pool.first().map(|t| (t.tag.clone(), "no tag: a type that thick".into()))
}

/// Read the walls of a set: legend, plans, tags, guesses, heights.
pub fn read_walls(dxf: &[(String, Vec<u8>)], map: &SheetMap, ceilings: &CeilingResult, structure: &StructuralModel) -> WallResult {
    let mut res = WallResult::default();
    let mut plans: Vec<(String, Alignment, Drawing)> = Vec::new();
    let mut framing: BTreeMap<String, (Option<f64>, Option<f64>)> = BTreeMap::new();
    for (name, bytes) in dxf {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "A" && sh.discipline != "S" {
            continue;
        }
        if sh.discipline == "S"
            && let Some(level) = &sh.level
            && let Ok(d) = cadaddy_io::read_dxf(bytes)
        {
            framing.insert(level.clone(), framing_notes(&d));
            continue;
        }
        let title = sh.title.to_ascii_uppercase();
        let legend = title.contains("WALL TYPE") || title.contains("PARTITION");
        let plan = title.contains("FLOOR PLAN");
        if !legend && !plan {
            continue;
        }
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        if legend {
            for t in read_legend(&d) {
                if !res.types.iter().any(|o| o.tag == t.tag) {
                    res.types.push(t);
                }
            }
        } else if let (Some(level), Some(al)) = (&sh.level, &sh.alignment) {
            plans.push((level.clone(), al.clone(), d));
        }
    }
    let decks = deck_heights(&framing, structure);
    for (level, al, d) in &plans {
        let lines: Vec<Seg> = d
            .model
            .iter()
            .filter(|e| {
                let l = e.common.layer.to_ascii_uppercase();
                l.contains("WALL") && !l.contains("TAG")
            })
            .filter_map(|e| match &e.kind {
                EntityKind::Line(l) => {
                    Some(Seg { a: sheet_reader::building_from_model(al, [l.a.x, l.a.y]), b: sheet_reader::building_from_model(al, [l.b.x, l.b.y]) })
                }
                _ => None,
            })
            .filter(|s| s.a.iter().chain(&s.b).all(|v| v.is_finite()))
            .take(MAX_LINES)
            .collect();
        let hinges: Vec<[f64; 2]> = d
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Arc(a) => Some(sheet_reader::building_from_model(al, [a.center.x, a.center.y])),
                _ => None,
            })
            .filter(|p| p.iter().all(|v| v.is_finite()))
            .take(MAX_LINES)
            .collect();
        let mut walls = find_walls(&lines, &hinges, level);
        for (tag, p, leader) in plan_tags(d, al, &res.types) {
            let reach = if leader { 1.0 } else { 3.0 };
            let best = walls
                .iter_mut()
                .map(|w| (to_seg(p, w.a_ft, w.b_ft).0 - w.thickness_in / 24.0, w))
                .filter(|(dd, _)| *dd < reach)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, w)) = best {
                w.wall_type = Some(tag);
                w.tagged = true;
            }
        }
        let tagged: Vec<FoundWall> = walls.iter().filter(|w| w.tagged).cloned().collect();
        let (lo, hi) = walls
            .iter()
            .flat_map(|w| [w.a_ft, w.b_ft])
            .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
        // The outline through the outer walls' centrelines.
        let inset = |v: f64, d: f64| v + d;
        let outline = ([inset(lo[0], 0.3), inset(lo[1], 0.0)], [inset(hi[0], -0.3), inset(hi[1], 0.0)]);
        let room_names: BTreeMap<String, String> =
            ceilings.rooms.iter().filter(|r| &r.level == level).map(|r| (r.number.clone(), r.name.to_ascii_uppercase())).collect();
        for w in walls.iter_mut() {
            w.rooms = rooms_beside(w, ceilings);
        }
        for w in walls.iter_mut().filter(|w| !w.tagged) {
            if let Some((t, why)) = guess(w, &tagged, &res.types, &outline, &room_names) {
                w.wall_type = Some(t);
                w.guess = Some(why);
            }
        }
        let deck = decks.get(level).copied();
        for w in walls.iter_mut() {
            let ceiling = ceilings
                .ceilings
                .iter()
                .filter(|c| c.level == w.level && w.rooms.contains(&c.room_number))
                .filter_map(|c| c.height_ft)
                .reduce(f64::max);
            let rule = w.wall_type.as_ref().and_then(|t| res.types.iter().find(|x| &x.tag == t)).and_then(|t| t.height.clone());
            (w.height_ft, w.height_source) = match (rule.as_deref(), ceiling, deck) {
                (Some("ceiling"), Some(c), d) => (Some(d.map_or(c, |d| c.min(d))), "ceiling".into()),
                (Some("above_ceiling"), Some(c), d) => (Some(d.map_or(c + 0.5, |d| (c + 0.5).min(d))), "ceiling".into()),
                (Some("ceiling" | "above_ceiling"), None, Some(d)) => (Some(d), "no ceiling beside it: deck".into()),
                (_, _, Some(d)) => (Some(d), "deck".into()),
                _ => (None, "unknown".into()),
            };
        }
        res.walls.extend(walls);
    }
    res
}

/// Generate a set, recognise its sheets, read ceilings and structure, then walls.
pub fn read_fixture_walls(seed: u64) -> Result<(Truth, WallResult), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(&dxf, &map);
    let drawings: Vec<(String, Drawing)> =
        dxf.iter().filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&map, &refs);
    Ok((set.truth, read_walls(&dxf, &map, &ceilings, &structure)))
}

// ---- Scoring -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct WallScore {
    pub types_total: usize,
    pub types_found: usize,
    /// Legend types whose layer stack (material, thickness, side, in order) is exactly right.
    pub stacks_correct: usize,
    pub heights_rule_correct: usize,
    pub ratings_correct: usize,
    pub thickness_correct: usize,
    pub walls_total: usize,
    pub walls_found: usize,
    /// Tagged walls found with the right type.
    pub tagged_total: usize,
    pub tagged_correct: usize,
    /// Untagged walls: flagged as guesses, and guessed right.
    pub untagged_total: usize,
    pub untagged_flagged: usize,
    pub untagged_correct: usize,
    /// Found walls with a height within 1".
    pub height_correct: usize,
    pub height_mae_ft: f64,
    pub openings_total: usize,
    pub openings_found: usize,
}

impl WallScore {
    pub fn type_accuracy(&self) -> f64 {
        self.tagged_correct as f64 / self.tagged_total.max(1) as f64
    }
}

fn same_stack(a: &[WallTypeLayer], b: &[WallTypeLayer]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.material == y.material && (x.thickness_in - y.thickness_in).abs() < 1e-3 && x.side == y.side && x.in_cavity == y.in_cavity
        })
}

pub fn score_walls(t: &Truth, r: &WallResult) -> WallScore {
    let mut sc = WallScore { types_total: t.wall_types.len(), walls_total: t.plan_walls.len(), ..WallScore::default() };
    for tw in &t.wall_types {
        let Some(f) = r.types.iter().find(|x| x.tag == tw.tag) else { continue };
        sc.types_found += 1;
        sc.stacks_correct += usize::from(same_stack(&tw.layers, &f.layers));
        sc.heights_rule_correct += usize::from(f.height.as_deref() == Some(tw.height.as_str()));
        sc.ratings_correct += usize::from(f.fire_rating == tw.fire_rating && f.stc == tw.stc);
        sc.thickness_correct += usize::from((f.thickness_in() - tw.thickness_in).abs() < 1e-3);
    }
    let mut err = Vec::new();
    for tw in &t.plan_walls {
        let probe = FoundWall { a_ft: tw.a_ft, b_ft: tw.b_ft, thickness_in: 6.0, ..FoundWall::default() };
        let found = r
            .walls
            .iter()
            .filter(|f| f.level == tw.level && same_line(&probe, f) && overlap(&probe, f) > 0.5)
            .max_by(|a, b| overlap(&probe, a).total_cmp(&overlap(&probe, b)));
        if tw.tagged {
            sc.tagged_total += 1;
        } else {
            sc.untagged_total += 1;
        }
        sc.openings_total += tw.openings.len();
        let Some(f) = found else { continue };
        sc.walls_found += 1;
        sc.openings_found += tw.openings.len().min(f.openings.len());
        let right = f.wall_type.as_deref() == Some(tw.wall_type.as_str());
        if tw.tagged {
            sc.tagged_correct += usize::from(right && f.tagged);
        } else {
            sc.untagged_flagged += usize::from(!f.tagged && f.guess.is_some());
            sc.untagged_correct += usize::from(right);
        }
        if let Some(h) = f.height_ft {
            let e = (h - tw.height_ft).abs();
            err.push(e);
            sc.height_correct += usize::from(e <= 1.0 / 12.0 + 1e-6);
        }
    }
    sc.height_mae_ft = if err.is_empty() { f64::NAN } else { err.iter().sum::<f64>() / err.len() as f64 };
    sc
}

// ---- Into the building spec --------------------------------------------------------------------

fn category(m: &str) -> &'static str {
    match m {
        "stud" => "framing",
        "insulation" => "insulation",
        "cmu" => "masonry",
        "sheathing" => "sheathing",
        "resilient_channel" | "furring" => "furring",
        _ => "board",
    }
}

/// A layer in plain words: `5/8" Type X gypsum board`, `3 5/8" 20 ga metal studs @ 16" o.c.`.
pub fn layer_words(l: &WallTypeLayer) -> String {
    let d = crate::walls::frac(l.thickness_in, false);
    match l.material.as_str() {
        "gyp" => format!("{d} gypsum board"),
        "gyp_x" => format!("{d} Type X gypsum board"),
        "gyp_mr" => format!("{d} moisture-resistant gypsum board"),
        "gyp_ar" => format!("{d} abuse-resistant gypsum board"),
        "shaftliner" => format!("{d} shaft liner panel"),
        "sheathing" => format!("{d} exterior sheathing"),
        "resilient_channel" => format!("{d} resilient channel"),
        "furring" => format!("{d} furring channel"),
        "cmu" => format!("{d} CMU"),
        "insulation" => format!("{d} batt insulation"),
        "stud" => {
            let g = l.gauge.map(|g| format!(" {g} ga")).unwrap_or_default();
            let o = l.spacing_in.map(|s| format!(" @ {} o.c.", crate::walls::frac(s, false))).unwrap_or_default();
            format!("{d}{g} metal studs{o}")
        }
        m => format!("{d} {m}"),
    }
}

/// The building spec patch for what was read: a wall type per legend type (named by its tag)
/// with its assembly, and a wall mark per wall. `to_plan` maps a level's building feet to its
/// plan drawing's coordinates (none: the level isn't built).
pub fn spec_patch(r: &WallResult, to_plan: &dyn Fn(&str, [f64; 2]) -> Option<[f64; 2]>) -> Value {
    let types: Vec<Value> = r
        .types
        .iter()
        .map(|t| {
            let layers: Vec<Value> = t
                .layers
                .iter()
                .map(|l| {
                    let side = match l.side.as_str() {
                        "1" => "side1",
                        "2" => "side2",
                        _ => "core",
                    };
                    json!({"material": layer_words(l), "thickness_in": l.thickness_in, "side": side, "category": category(&l.material), "in_cavity": l.in_cavity})
                })
                .collect();
            let mut a = json!({"tag": t.tag, "description": t.name, "layers": layers, "external": t.external});
            if let Some(h) = &t.height {
                a["height"] = json!(h);
            }
            if let Some(f) = &t.fire_rating {
                a["fire_rating"] = json!(f);
            }
            if let Some(s) = t.stc {
                a["acoustic_rating"] = json!(format!("STC {s}"));
            }
            if !t.notes.is_empty() {
                a["notes"] = json!(t.notes);
            }
            json!({"name": t.tag, "thickness_in": t.thickness_in(), "from": "pairs", "assembly": a})
        })
        .collect();
    let walls: Vec<Value> = r
        .walls
        .iter()
        .filter_map(|w| {
            let t = w.wall_type.as_ref()?;
            let (a, b) = (to_plan(&w.level, w.a_ft)?, to_plan(&w.level, w.b_ft)?);
            let mut m = json!({"level": w.level, "a": a, "b": b, "wall_type": t});
            if let Some(h) = w.height_ft {
                m["height_ft"] = json!(h);
            }
            if let Some(g) = &w.guess {
                m["guess"] = json!(g);
            }
            Some(m)
        })
        .collect();
    json!({"wall_types": types, "walls": walls})
}
