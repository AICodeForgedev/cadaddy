//! Water where the plumbing plans draw none (Q6161; owner 11-05: the plumbers built the building
//! already, the field settles what the drawings leave out from code and trade practice). A floor
//! whose plans draw its drainage but no domestic water is piped the way the field pipes it:
//! - DERIVED: over the area where its rooms stack on the floor it is piped like (the floor below;
//!   above when no floor below has water: rules data `stack_on`), that floor's drawn mains are
//!   copied, and only there: a main under a shared space is not the rooms' over it.
//! - INFERRED: where nothing stacks, each fixture of the floor's own waste plan with no water line
//!   in reach, and each room's branches (the enlarged details') joined to no main, are run from the
//!   nearest main or riser of their system the way a plumber runs them: the main carried on along
//!   its line, the branch teed off it at right angles.
//!
//! Nothing filled in passes for drawn: each run says what it is (`height_source` [`DERIVED`] or
//! [`INFERRED`]) and its basis (`size_source`: the source floor or the rule), and the build puts
//! it on its own layer and takeoff row. The numbers and the bases are rules data (`plumbing.json`
//! `fill_in`), never a job's.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::MepRun;
use serde::Deserialize;

/// A run copied from another floor's drawn plan where this floor's rooms stack on it.
pub(crate) const DERIVED: &str = "derived";
/// A run laid in from a rule (code or trade practice) where no plan draws it.
pub(crate) const INFERRED: &str = "inferred";
/// A run a plan draws.
pub(crate) const DRAWN: &str = "drawn";

/// What a run is by its marks: [`DRAWN`], [`DERIVED`] or [`INFERRED`]: the first word of its
/// `height_source` (a later step may add to it: `derived (capped under the floor above)`).
pub(crate) fn provenance(r: &MepRun) -> &'static str {
    let first = r.height_source.trim().split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("");
    if first.eq_ignore_ascii_case(DERIVED) {
        DERIVED
    } else if first.eq_ignore_ascii_case(INFERRED) {
        INFERRED
    } else {
        DRAWN
    }
}

/// The fill-in rules (`plumbing.json` `fill_in`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FillRules {
    /// The domestic water systems a floor's water plan draws.
    pub water: Vec<String>,
    /// Of them, the ones a fixture's branch is run for (a return feeds no fixture).
    pub fixture_lines: Vec<String>,
    /// The floors a floor with no water plan is piped like, tried in this order: `below`, `above`.
    pub stack_on: Vec<String>,
    /// Why a floor is piped like the one its rooms stack on.
    pub stack_basis: String,
    /// Why a main is carried on along its line to a branch.
    pub main_basis: String,
    /// Why a fixture's or a room's branch is teed off the nearest main of its system, at its size.
    pub branch_basis: String,
    /// The size of a fixture branch nothing sizes (inches).
    pub branch_size_in: f64,
    /// How far from the nearest main a branch is still run (feet); farther stays unconnected.
    pub reach_ft: f64,
    /// A need this close to a branch already run (or a room's pipes already joined) takes it, not
    /// a line of its own back to the main (feet: one room's fixtures, back-to-back rooms).
    pub share_ft: f64,
    /// Two pipe ends this close are joined (feet).
    pub joint_ft: f64,
    /// A point this close to a room's outline is in it (feet: the walls).
    pub room_margin_ft: f64,
    /// A main is tested for stacking every this many feet along it.
    pub sample_ft: f64,
    /// A copied piece shorter than this is dropped (feet).
    pub min_piece_ft: f64,
    /// The drainage lines (waste, vent) a fixture with none in reach is run for, on every floor,
    /// from the nearest pipe of the system.
    pub drainage: Vec<String>,
    /// Why a fixture's waste or vent is run to the nearest pipe of its system.
    pub drain_basis: String,
    /// The vent line of [`Self::drainage`], and the drainage systems whose stack in reach vents a
    /// fixture (the stack vent), and why.
    pub vent: String,
    pub vent_stacks: Vec<String>,
    pub vent_stack_basis: String,
    /// The size of a fixture's drainage branch by its system and words (the first that matches;
    /// no words: any fixture), with the code section.
    pub sizes: Vec<LineSize>,
    /// A laid piece this close to a pipe of its system already there, and running along it (at
    /// most [`Self::along_deg`] off its line), is that pipe and is not laid again (feet).
    pub along_ft: f64,
    pub along_deg: f64,
}

/// A fixture branch's size by its system and the fixture's words (`plumbing.json` `fill_in.sizes`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LineSize {
    pub system: String,
    pub words: Vec<String>,
    pub size_in: f64,
    pub source: String,
}

impl FillRules {
    /// The size (inches) and its source of a fixture's `system` branch, by the fixture's words:
    /// the first rule of that system whose words it has (one with none: any). `None`: no rule.
    pub(crate) fn size_for(&self, system: &str, words: &str) -> Option<(f64, &str)> {
        let up = format!(" {} ", words.to_ascii_uppercase());
        self.sizes
            .iter()
            .filter(|z| z.system == system && z.size_in.is_finite() && z.size_in > 0.0)
            .find(|z| z.words.is_empty() || z.words.iter().any(|w| up.contains(&w.to_ascii_uppercase())))
            .map(|z| (z.size_in, z.source.as_str()))
    }

    /// The systems [`infer`] runs lines of: the water, then the drainage.
    fn filled(&self) -> impl Iterator<Item = &String> {
        self.water.iter().chain(self.drainage.iter().filter(|d| !self.water.contains(d)))
    }
}

#[derive(Default, Deserialize)]
struct File {
    #[serde(default)]
    fill_in: FillRules,
}

/// The fill-in rules data.
pub(crate) fn rules() -> FillRules {
    serde_json::from_str::<File>(include_str!("plumbing.json")).map(|f| f.fill_in).unwrap_or_default()
}

/// Rooms of a level compared at most (the stacking test is quadratic in them).
const MAX_ROOMS: usize = 1_000;
/// Needs and network pieces handled at most per level and system.
const MAX_ITEMS: usize = 20_000;
/// Rooms listed by name where they don't stack, at most.
const MAX_LISTED: usize = 40;

/// A room of a level: its type's words, its outline (drawing units) and a name to report it by.
#[derive(Clone, Debug)]
pub(crate) struct AreaRoom {
    pub words: BTreeSet<String>,
    pub outline: Vec<[f64; 2]>,
    pub label: String,
}

struct Room<'a> {
    r: &'a AreaRoom,
    b: [f64; 4],
    c: [f64; 2],
    area: f64,
}

fn rooms_of(v: &[AreaRoom]) -> Vec<Room<'_>> {
    v.iter()
        .filter_map(|r| {
            let b = r
                .outline
                .iter()
                .filter(|p| p[0].is_finite() && p[1].is_finite())
                .fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
                    [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]
                });
            let n = r.outline.len();
            let area = (0..n)
                .filter_map(|i| Some((r.outline.get(i)?, r.outline.get((i + 1) % n)?)))
                .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
                .sum::<f64>()
                .abs()
                / 2.0;
            (b[2] > b[0] && b[3] > b[1]).then(|| Room { r, b, c: [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0], area })
        })
        .take(MAX_ROOMS)
        .collect()
}

fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

/// The distance from `p` to the segment `a`-`b`, the nearest point on it and where (0 at `a`, 1 at `b`).
fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, [f64; 2], f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let u = if l <= 1e-18 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0) };
    let q = [a[0] + u * dx, a[1] + u * dy];
    ((q[0] - p[0]).hypot(q[1] - p[1]), q, u)
}

fn edge_dist(poly: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = poly.len();
    (0..n).filter_map(|i| Some(to_segment(p, *poly.get(i)?, *poly.get((i + 1) % n)?).0)).fold(f64::INFINITY, f64::min)
}

/// The stretches of `a`-`b` that don't lie along `segs` (within `tol` of one, at most `sin_max`
/// (the sine of the angle) off its line, and beside it, not past its ends), as fractions from `a`
/// (0) to `b` (1), tested every `step`: a piece laid where a pipe of its system already runs is
/// that pipe, not a second one beside it. A tee or a crossing is not along.
fn open_parts(a: [f64; 2], b: [f64; 2], segs: &[[[f64; 2]; 2]], tol: f64, sin_max: f64, step: f64) -> Vec<(f64, f64)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if !(len.is_finite() && len > 0.0) {
        return Vec::new();
    }
    let lo = [a[0].min(b[0]) - tol, a[1].min(b[1]) - tol, a[0].max(b[0]) + tol, a[1].max(b[1]) + tol];
    let near: Vec<&[[f64; 2]; 2]> = segs
        .iter()
        .filter(|[p, q]| {
            p[0].min(q[0]) <= lo[2] && p[0].max(q[0]) >= lo[0] && p[1].min(q[1]) <= lo[3] && p[1].max(q[1]) >= lo[1] && {
                let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
                let l = ex.hypot(ey);
                l > 0.0 && (dx * ey - dy * ex).abs() / (len * l) <= sin_max
            }
        })
        .collect();
    if near.is_empty() {
        return vec![(0.0, 1.0)];
    }
    let along = |t: f64| {
        let m = [a[0] + dx * t, a[1] + dy * t];
        near.iter().any(|[p, q]| {
            let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
            let l2 = ex * ex + ey * ey;
            let u = ((m[0] - p[0]) * ex + (m[1] - p[1]) * ey) / l2;
            (0.0..=1.0).contains(&u) && (p[0] + u * ex - m[0]).hypot(p[1] + u * ey - m[1]) <= tol
        })
    };
    let n = ((len / step.max(1e-9)).ceil() as usize).clamp(1, 20_000);
    let mut out: Vec<(f64, f64)> = Vec::new();
    for k in 0..n {
        let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
        if along((t0 + t1) / 2.0) {
            continue;
        }
        match out.last_mut() {
            Some(last) if (last.1 - t0).abs() < 1e-12 => last.1 = t1,
            _ => out.push((t0, t1)),
        }
    }
    out
}

/// `pts` (each segment at its bottom in `bottoms`) less the stretches of it along `segs`
/// ([`open_parts`]): the pieces left, each at least `min` long.
fn clip_off(pts: &[[f64; 2]], bottoms: &[f64], segs: &[[[f64; 2]; 2]], ru: &FillRules, upf: f64, min: f64) -> Vec<(Vec<[f64; 2]>, Vec<f64>)> {
    let tol = ru.along_ft.max(0.0) * upf;
    let sin_max = ru.along_deg.clamp(0.0, 90.0).to_radians().sin();
    let step = (ru.along_ft.max(0.05) * upf / 2.0).max(1e-9);
    let mut out: Vec<(Vec<[f64; 2]>, Vec<f64>)> = Vec::new();
    let mut cur: (Vec<[f64; 2]>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut flush = |cur: &mut (Vec<[f64; 2]>, Vec<f64>)| {
        let len: f64 = cur.0.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        let piece = std::mem::take(cur);
        if piece.0.len() >= 2 && len > 0.0 && len >= min {
            out.push(piece);
        }
    };
    for (i, w) in pts.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let bottom = bottoms.get(i).or(bottoms.last()).copied().unwrap_or(0.0);
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let parts = if tol > 0.0 { open_parts(a, b, segs, tol, sin_max, step) } else { vec![(0.0, 1.0)] };
        let mut joined_end = false;
        for (t0, t1) in parts {
            // Carried on from the last segment's open end, else a new piece.
            if !(t0 <= 1e-12 && cur.0.last().is_some_and(|l| (l[0] - a[0]).abs() < 1e-9 && (l[1] - a[1]).abs() < 1e-9)) {
                flush(&mut cur);
                cur.0.push(at(t0));
            }
            cur.0.push(at(t1));
            cur.1.push(bottom);
            joined_end = t1 >= 1.0 - 1e-12;
        }
        if !joined_end {
            flush(&mut cur);
        }
    }
    flush(&mut cur);
    out
}

/// The segments of `runs`.
fn segments<'a>(runs: impl Iterator<Item = &'a MepRun>) -> Vec<[[f64; 2]; 2]> {
    runs.flat_map(|r| r.points.windows(2).map(|w| [w[0], w[1]])).take(MAX_ITEMS * 4).collect()
}

/// The type of the room `p` is in: the smallest whose outline holds it, else the nearest within
/// `margin` (on its wall). `None`: in no room.
fn room_at<'a>(rooms: &'a [Room<'_>], p: [f64; 2], margin: f64) -> Option<&'a BTreeSet<String>> {
    let near = |m: &Room<'_>| p[0] >= m.b[0] - margin && p[0] <= m.b[2] + margin && p[1] >= m.b[1] - margin && p[1] <= m.b[3] + margin;
    if let Some(m) = rooms.iter().filter(|m| near(m) && inside(&m.r.outline, p)).min_by(|a, b| a.area.total_cmp(&b.area)) {
        return Some(&m.r.words);
    }
    rooms
        .iter()
        .filter(|m| near(m))
        .map(|m| (edge_dist(&m.r.outline, p), m))
        .filter(|(d, _)| *d <= margin)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, m)| &m.r.words)
}

/// The move that lays `theirs` over `mine`: the one most pairs of like rooms (same type, sides
/// within a fifth) agree on, each pair voting for its foot-square and those round it.
fn lay_over(mine: &[Room<'_>], theirs: &[Room<'_>], upf: f64) -> Option<[f64; 2]> {
    let close = |x: f64, y: f64| (x - y).abs() <= 0.2 * x.max(y);
    let like =
        |a: &Room<'_>, b: &Room<'_>| a.r.words == b.r.words && close(a.b[2] - a.b[0], b.b[2] - b.b[0]) && close(a.b[3] - a.b[1], b.b[3] - b.b[1]);
    let mut votes: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    let mut pairs: Vec<[f64; 2]> = Vec::new();
    for a in mine {
        for b in theirs.iter().filter(|b| like(a, b)) {
            let off = [a.c[0] - b.c[0], a.c[1] - b.c[1]];
            pairs.push(off);
            let k = ((off[0] / upf).round() as i64, (off[1] / upf).round() as i64);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    *votes.entry((k.0 + dx, k.1 + dy)).or_default() += 1;
                }
            }
        }
    }
    let (k, _) = votes.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?;
    let near: Vec<&[f64; 2]> = pairs.iter().filter(|o| (o[0] / upf - k.0 as f64).abs() <= 1.5 && (o[1] / upf - k.1 as f64).abs() <= 1.5).collect();
    (!near.is_empty()).then(|| {
        let n = near.len() as f64;
        [near.iter().map(|o| o[0]).sum::<f64>() / n, near.iter().map(|o| o[1]).sum::<f64>() / n]
    })
}

/// A floor's water derived from the floor it is piped like ([`derive`]).
#[derive(Debug, Default)]
pub(crate) struct Derived {
    /// The floor copied from (as its runs name it).
    pub from: String,
    /// This floor's rooms, and how many stand on a room of their type there.
    pub rooms: usize,
    pub stacked: usize,
    /// The rooms that don't (by name, the first ones).
    pub unstacked: Vec<String>,
    /// The copies, on this floor, marked [`DERIVED`].
    pub runs: Vec<MepRun>,
}

/// A source floor for [`derive`]: its name (as its runs give it), its rooms, its drawn floor-plan
/// water runs.
pub(crate) struct Source {
    pub level: String,
    pub rooms: Vec<AreaRoom>,
    pub runs: Vec<MepRun>,
}

/// The water of a floor whose plans draw none, derived where its rooms stack: the first of
/// `sources` (the floors it may be piped like, in the rules' order) that has rooms; its plan laid
/// over this one by the move like rooms agree on; each of its drawn mains copied onto `level`
/// over the stretches where the room under the pipe there and the room over it here are of one
/// type (a corridor over a corridor, a guest room over its type; in no room on both: a chase).
/// A run wholly in one room of a type that repeats on its floor is that room's branch, not a main
/// (the rooms of a type take their type's branches). `None`: no source floor with rooms, or no
/// like rooms to lay the plans over each other by.
pub(crate) fn derive(mine: &[AreaRoom], level: &str, sources: &[Source], upf: f64, ru: &FillRules) -> Option<Derived> {
    let here = rooms_of(mine);
    if here.is_empty() || !(upf.is_finite() && upf > 0.0) {
        return None;
    }
    let src = sources.iter().find(|s| !s.rooms.is_empty())?;
    let there = rooms_of(&src.rooms);
    let off = lay_over(&here, &there, upf)?;
    let margin = ru.room_margin_ft.max(0.0) * upf;
    let mut out = Derived { from: src.level.clone(), rooms: here.len(), ..Derived::default() };
    // Room by room: one of its type there at the same place (each centre in the other's box).
    let mut used = vec![false; there.len()];
    for a in &here {
        let hit = there.iter().enumerate().position(|(j, b)| {
            let moved = [b.b[0] + off[0], b.b[1] + off[1], b.b[2] + off[0], b.b[3] + off[1]];
            let c = [b.c[0] + off[0], b.c[1] + off[1]];
            !used.get(j).copied().unwrap_or(true)
                && a.r.words == b.r.words
                && a.c[0] >= moved[0]
                && a.c[0] <= moved[2]
                && a.c[1] >= moved[1]
                && a.c[1] <= moved[3]
                && c[0] >= a.b[0]
                && c[0] <= a.b[2]
                && c[1] >= a.b[1]
                && c[1] <= a.b[3]
        });
        match hit.and_then(|j| used.get_mut(j)) {
            Some(u) => {
                *u = true;
                out.stacked += 1;
            }
            None if out.unstacked.len() < MAX_LISTED => out.unstacked.push(a.r.label.clone()),
            None => {}
        }
    }
    // The source floor's room branches: wholly in one room of a repeating type.
    let mut repeats: BTreeMap<&BTreeSet<String>, usize> = BTreeMap::new();
    for t in &there {
        *repeats.entry(&t.r.words).or_default() += 1;
    }
    let branch = |r: &MepRun| {
        there.iter().filter(|t| repeats.get(&t.r.words).is_some_and(|n| *n >= 2)).any(|t| {
            r.points.iter().all(|p| p[0] >= t.b[0] - margin && p[0] <= t.b[2] + margin && p[1] >= t.b[1] - margin && p[1] <= t.b[3] + margin)
        })
    };
    let stacks = |p: [f64; 2]| room_at(&there, p, margin) == room_at(&here, [p[0] + off[0], p[1] + off[1]], margin);
    let step = ru.sample_ft.max(0.25) * upf;
    let min_piece = ru.min_piece_ft.max(0.0) * upf;
    let basis = format!("derived from {}'s drawn main where this floor's rooms stack on its: {}", src.level, ru.stack_basis);
    // The longest first: a stretch the source floor draws twice (its plan and a second view) is
    // copied once, the next copy clipped where it runs along one already made.
    let mut mains: Vec<&MepRun> =
        src.runs.iter().filter(|r| r.top_ft.is_none() && r.points.len() >= 2 && provenance(r) == DRAWN && !branch(r)).collect();
    let run_len = |r: &MepRun| r.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>();
    mains.sort_by(|a, b| run_len(b).total_cmp(&run_len(a)));
    let mut copied: BTreeMap<&str, Vec<[[f64; 2]; 2]>> = BTreeMap::new();
    for r in mains {
        // Each stretch that stacks, with each segment's bottom.
        let mut pieces: Vec<(Vec<[f64; 2]>, Vec<f64>)> = Vec::new();
        let mut cur: (Vec<[f64; 2]>, Vec<f64>) = (Vec::new(), Vec::new());
        let flush = |cur: &mut (Vec<[f64; 2]>, Vec<f64>), pieces: &mut Vec<(Vec<[f64; 2]>, Vec<f64>)>| {
            let len: f64 = cur.0.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
            if cur.0.len() >= 2 && len >= min_piece && len > 0.0 {
                pieces.push(std::mem::take(cur));
            }
            cur.0.clear();
            cur.1.clear();
        };
        for (i, w) in r.points.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            let bottom = r.bottoms_ft.get(i).or(r.bottoms_ft.last()).copied().unwrap_or(0.0);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            let n = ((len / step).ceil() as usize).clamp(1, 10_000);
            let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            for k in 0..n {
                let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
                if stacks(at((t0 + t1) / 2.0)) {
                    if cur.0.is_empty() {
                        cur.0.push(at(t0));
                    }
                    // One point a straight stretch: extend its last segment when it is this one's.
                    let same = cur.0.len() >= 2 && cur.1.last().is_some_and(|l| (l - bottom).abs() < 1e-9) && k > 0;
                    if same {
                        if let Some(last) = cur.0.last_mut() {
                            *last = at(t1);
                        }
                    } else {
                        cur.0.push(at(t1));
                        cur.1.push(bottom);
                    }
                } else {
                    flush(&mut cur, &mut pieces);
                }
            }
        }
        flush(&mut cur, &mut pieces);
        let done = copied.entry(r.system.as_str()).or_default();
        let pieces: Vec<(Vec<[f64; 2]>, Vec<f64>)> =
            pieces.iter().flat_map(|(pts, bottoms)| clip_off(pts, bottoms, done, ru, upf, min_piece.max(1e-9))).collect();
        for (pts, _) in &pieces {
            done.extend(pts.windows(2).map(|w| [w[0], w[1]]));
        }
        for (pts, bottoms) in pieces {
            let mut c = r.clone();
            c.points = pts.iter().map(|p| [p[0] + off[0], p[1] + off[1]]).collect();
            c.bottoms_ft = bottoms;
            c.level = level.to_string();
            c.objects.clear();
            c.id.clear();
            c.reviewed = false;
            c.edited_by.clear();
            c.height_source = DERIVED.into();
            c.size_source.clone_from(&basis);
            out.runs.push(c);
        }
    }
    Some(out)
}

/// Each [`DERIVED`] run of `runs` less its stretches along a drawn pipe of its system on its
/// level (laid there after it was derived: a room's own pipes): the drawn pipe is that pipe. The
/// derived feet taken out.
pub(crate) fn clip_derived(runs: &mut Vec<MepRun>, upf: f64, ru: &FillRules) -> f64 {
    if !(upf.is_finite() && upf > 0.0) || !runs.iter().any(|r| provenance(r) == DERIVED) {
        return 0.0;
    }
    let mut drawn: BTreeMap<(&str, &str), Vec<[[f64; 2]; 2]>> = BTreeMap::new();
    for r in runs.iter().filter(|r| provenance(r) == DRAWN && r.top_ft.is_none()).take(MAX_ITEMS) {
        drawn.entry((r.level.as_str(), r.system.as_str())).or_default().extend(r.points.windows(2).map(|w| [w[0], w[1]]));
    }
    let len = |p: &[[f64; 2]]| p.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>();
    let min = ru.min_piece_ft.max(0.0) * upf;
    let mut out: Vec<MepRun> = Vec::with_capacity(runs.len());
    let mut cut = 0.0;
    for r in runs.iter() {
        let segs = drawn.get(&(r.level.as_str(), r.system.as_str()));
        match segs {
            Some(segs) if provenance(r) == DERIVED && r.top_ft.is_none() => {
                let before = len(&r.points);
                let mut after = 0.0;
                for (pts, bottoms) in clip_off(&r.points, &r.bottoms_ft, segs, ru, upf, min.max(1e-9)) {
                    let mut c = r.clone();
                    after += len(&pts);
                    c.points = pts;
                    c.bottoms_ft = bottoms;
                    out.push(c);
                }
                cut += (before - after).max(0.0) / upf;
            }
            _ => out.push(r.clone()),
        }
    }
    *runs = out;
    cut
}

/// A water line some fixture or room needs on a floor with no water plan ([`infer`]).
#[derive(Clone, Debug)]
pub(crate) struct Need {
    pub system: String,
    /// Where it may be joined (a fixture: its point; a room's branches: their points).
    pub pts: Vec<[f64; 2]>,
    /// What it serves (`fixture P-91`, `the branches of 2 run(s)`).
    pub what: String,
    /// The branch's size (inches); 0: the rules' fixture branch size.
    pub size_in: f64,
    /// A room's branches: their segments (once joined, later needs may take them).
    pub segs: Vec<[[f64; 2]; 2]>,
    /// Where its size comes from (a drainage branch: the code's fixture drain or vent size).
    pub size_basis: String,
}

/// A fixture of a floor: its point, what to call it and the lines it takes.
pub(crate) struct Fixture {
    pub at: [f64; 2],
    pub what: String,
    pub lines: Vec<String>,
    /// Its tag and schedule row's words (what sizes its drainage branches).
    pub words: String,
}

/// A main or riser (what a branch is run from): a riser, or a run no plan draws on the floor.
pub(crate) fn is_main(r: &MepRun) -> bool {
    r.top_ft.is_some() || r.points.len() < 2 || provenance(r) != DRAWN
}

/// The water and drainage lines a floor still needs, from its runs (`runs`: the floor's, derived
/// mains among them) and its fixtures: each fixture's line (water or drainage) with no pipe of its
/// system within `reach_ft` (as a fixture is connected; a vent, nor a stack of the rules'
/// `vent_stacks`: the stack vent), and on a floor with no water plan (`branches`) each group of
/// joined water branches (a room's, from the enlarged details) that touches no main or riser.
/// A pipe at the edge of the reach (within the rules' joint of it) is not counted on: the fixture
/// is connected from its built body, whose centre may lie that much off its symbol's point.
pub(crate) fn needs(runs: &[&MepRun], fixtures: &[Fixture], reach_ft: f64, upf: f64, ru: &FillRules, branches: bool) -> Vec<Need> {
    let mut out = Vec::new();
    let reach = (reach_ft.max(0.0) - ru.joint_ft.max(0.0)).max(0.0) * upf;
    let joint = ru.joint_ft.max(0.0) * upf;
    let near_pipe = |sys: &str, p: [f64; 2], d: f64, main_only: bool| {
        runs.iter().filter(|r| r.system == sys && (!main_only || is_main(r))).any(|r| match r.points.as_slice() {
            [q] => (q[0] - p[0]).hypot(q[1] - p[1]) <= d,
            pts => pts.windows(2).any(|w| to_segment(p, w[0], w[1]).0 <= d),
        })
    };
    let stack_near = |p: [f64; 2]| {
        runs.iter()
            .filter(|r| ru.vent_stacks.contains(&r.system) && (r.top_ft.is_some() || r.points.len() < 2))
            .any(|r| r.points.first().is_some_and(|q| (q[0] - p[0]).hypot(q[1] - p[1]) <= reach))
    };
    for f in fixtures.iter().take(MAX_ITEMS) {
        for sys in f.lines.iter().filter(|s| ru.fixture_lines.contains(s) || ru.drainage.contains(s)) {
            if near_pipe(sys, f.at, reach, false) || (*sys == ru.vent && stack_near(f.at)) {
                continue;
            }
            let (size_in, size_basis) = if ru.drainage.contains(sys) {
                ru.size_for(sys, &f.words).map_or((0.0, String::new()), |(z, b)| (z, b.to_string()))
            } else {
                (0.0, String::new())
            };
            out.push(Need { system: sys.clone(), pts: vec![f.at], what: format!("fixture {}", f.what), size_in, segs: Vec::new(), size_basis });
        }
    }
    // The branches, grouped where their pieces touch.
    for sys in ru.water.iter().filter(|_| branches) {
        let br: Vec<&MepRun> = runs.iter().filter(|r| &r.system == sys && !is_main(r)).take(MAX_ITEMS).copied().collect();
        let mut group: Vec<usize> = (0..br.len()).collect();
        fn root(g: &[usize], mut i: usize) -> usize {
            // Each parent is a lower index: the walk ends.
            while let Some(&p) = g.get(i) {
                if p >= i {
                    break;
                }
                i = p;
            }
            i
        }
        let touch = |a: &MepRun, b: &MepRun| {
            a.points.iter().any(|p| b.points.windows(2).any(|w| to_segment(*p, w[0], w[1]).0 <= joint))
                || b.points.iter().any(|p| a.points.windows(2).any(|w| to_segment(*p, w[0], w[1]).0 <= joint))
        };
        for i in 0..br.len() {
            for j in i + 1..br.len() {
                let (Some(a), Some(b)) = (br.get(i), br.get(j)) else { continue };
                if touch(a, b) {
                    let (ri, rj) = (root(&group, i), root(&group, j));
                    if let Some(g) = group.get_mut(ri.max(rj)) {
                        *g = ri.min(rj);
                    }
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<&MepRun>> = BTreeMap::new();
        for (i, r) in br.iter().enumerate() {
            groups.entry(root(&group, i)).or_default().push(r);
        }
        for g in groups.values() {
            let pts: Vec<[f64; 2]> = g.iter().flat_map(|r| r.points.iter().copied()).collect();
            if pts.iter().any(|p| near_pipe(sys, *p, joint, true)) {
                continue;
            }
            let size = g.iter().map(|r| r.size_in[0]).fold(0.0, f64::max);
            let segs = g.iter().flat_map(|r| r.points.windows(2).map(|w| [w[0], w[1]])).collect();
            out.push(Need {
                system: sys.clone(),
                pts,
                what: format!("the branches of {} run(s)", g.len()),
                size_in: size,
                segs,
                size_basis: String::new(),
            });
        }
    }
    out
}

/// What [`infer`] laid in.
#[derive(Debug, Default)]
pub(crate) struct Inferred {
    /// The runs, marked [`INFERRED`].
    pub runs: Vec<MepRun>,
    /// The needs met, and those with no main of their system within the rules' reach.
    pub served: usize,
    pub unserved: Vec<String>,
}

/// One piece of a floor's water network: its ends, the run it is of (the template a run laid off
/// it copies), its bottom, and whether it is a main (joined from the rules' reach) or a branch
/// (joined only from the rules' share distance).
struct Piece {
    a: [f64; 2],
    b: [f64; 2],
    run: usize,
    bottom: f64,
    main: bool,
}

/// Each need met from the nearest main or riser of its system on `level` (`mains`), the nearest
/// first, each run laid in joining the network for the next: a main carried on as a main, a branch
/// (and a room's branches once joined) as a branch a need within the rules' share distance takes
/// instead of a line of its own back to the main (one room's fixtures, back-to-back rooms): a need beside a main is teed off it
/// at right angles; one past a main's end has the main carried on along its line to it first
/// (sized as the main), then the branch (its own size, else the rules' fixture branch). Off a
/// riser: along one axis, then the other. Laid at the main's height. Each run marked
/// [`INFERRED`] with its basis; a need farther than the rules' reach is left, listed. A run is
/// not laid where it would run along a pipe of its system already on the floor (`along`: all its
/// pipe, drawn or filled in) or laid before it: that stretch is that pipe.
pub(crate) fn infer(level: &str, mains: &[&MepRun], along: &[&MepRun], needs: Vec<Need>, upf: f64, ru: &FillRules) -> Inferred {
    let mut out = Inferred::default();
    if !(upf.is_finite() && upf > 0.0) {
        return out;
    }
    let reach = ru.reach_ft.max(0.0) * upf;
    let joint = ru.joint_ft.max(0.0) * upf;
    let share = ru.share_ft.max(0.0) * upf;
    let mut by_sys: BTreeMap<&str, Vec<Need>> = BTreeMap::new();
    for n in needs {
        if let Some(sys) = ru.filled().find(|s| **s == n.system) {
            by_sys.entry(sys.as_str()).or_default().push(n);
        }
    }
    for (sys, mut left) in by_sys {
        let mut templates: Vec<MepRun> = mains.iter().filter(|r| r.system == sys).take(MAX_ITEMS).map(|r| (*r).clone()).collect();
        // A horizontal off a riser sits at the level's mains' height.
        let main_bottom = templates
            .iter()
            .filter(|r| r.top_ft.is_none() && r.points.len() >= 2)
            .find_map(|r| r.bottoms_ft.first().copied())
            .or_else(|| templates.iter().find_map(|r| r.bottoms_ft.first().copied()))
            .unwrap_or(0.0);
        let mut there: Vec<[[f64; 2]; 2]> = segments(along.iter().copied().filter(|r| r.system == sys));
        let mut net: Vec<Piece> = Vec::new();
        for (i, r) in templates.iter().enumerate() {
            match r.points.as_slice() {
                [] => {}
                [p] => net.push(Piece { a: *p, b: *p, run: i, bottom: main_bottom, main: true }),
                pts => {
                    for (k, w) in pts.windows(2).enumerate() {
                        let bottom = r.bottoms_ft.get(k).or(r.bottoms_ft.last()).copied().unwrap_or(main_bottom);
                        net.push(Piece { a: w[0], b: w[1], run: i, bottom, main: true });
                    }
                }
            }
        }
        while !left.is_empty() && net.len() < MAX_ITEMS {
            // The need nearest the network, and where it joins.
            let mut best: Option<(f64, usize, [f64; 2], [f64; 2], usize, f64)> = None;
            for (ni, n) in left.iter().enumerate() {
                for p in &n.pts {
                    for (pi, s) in net.iter().enumerate() {
                        let (d, q, u) = to_segment(*p, s.a, s.b);
                        if d <= if s.main { reach } else { share } && best.as_ref().is_none_or(|b| d < b.0) {
                            best = Some((d, ni, *p, q, pi, u));
                        }
                    }
                }
            }
            let Some((_, ni, p, q, pi, u)) = best else { break };
            let need = left.swap_remove(ni);
            let Some(piece) = net.get(pi) else { break };
            let (run, bottom, off_main) = (piece.run, piece.bottom, piece.main);
            let (dx, dy) = (piece.b[0] - piece.a[0], piece.b[1] - piece.a[1]);
            let l = dx.hypot(dy);
            // Past a main's end: carried on along its line to opposite the need. Off a riser:
            // along the longer axis first.
            let corner = if l <= 1e-9 {
                if (p[0] - q[0]).abs() >= (p[1] - q[1]).abs() { [p[0], q[1]] } else { [q[0], p[1]] }
            } else if u <= 1e-9 || u >= 1.0 - 1e-9 {
                let d = if u >= 0.5 { [dx / l, dy / l] } else { [-dx / l, -dy / l] };
                let e = (p[0] - q[0]) * d[0] + (p[1] - q[1]) * d[1];
                if e > joint { [q[0] + d[0] * e, q[1] + d[1] * e] } else { q }
            } else {
                q
            };
            let Some(t) = templates.get(run).cloned() else { break };
            let lay = |pts: Vec<[f64; 2]>, size: [f64; 2], basis: String| {
                let mut c = t.clone();
                c.points = pts;
                c.bottoms_ft = vec![bottom];
                c.size_in = size;
                c.level = level.to_string();
                c.top_ft = None;
                c.objects.clear();
                c.id.clear();
                c.reviewed = false;
                c.edited_by.clear();
                c.slope_in_per_ft = 0.0;
                c.height_source = INFERRED.into();
                c.size_source = basis;
                c
            };
            let gap = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]).hypot(b[1] - a[1]) > joint;
            let size = if need.size_in > 0.0 { need.size_in } else { ru.branch_size_in };
            let branch = |what: &str| {
                if ru.drainage.iter().any(|d| d == sys) {
                    let size = if need.size_basis.is_empty() { String::new() } else { format!("; size: {}", need.size_basis) };
                    format!("inferred: {} ({sys} branch to {what}{size})", ru.drain_basis)
                } else {
                    format!("inferred: {} ({sys} branch to {what})", ru.branch_basis)
                }
            };
            // (Each run, and whether it is a main: off a branch, both legs are the branch.)
            let mut laid: Vec<(MepRun, bool)> = Vec::new();
            if gap(q, corner) {
                laid.push(if off_main {
                    (lay(vec![q, corner], t.size_in, format!("inferred: {} ({sys} main carried on to {})", ru.main_basis, need.what)), true)
                } else {
                    (lay(vec![q, corner], [size, 0.0], branch(&need.what)), false)
                });
            }
            if gap(corner, p) {
                laid.push((lay(vec![corner, p], [size, 0.0], branch(&need.what)), false));
            }
            for (r, main) in laid {
                if let [a, b] = r.points.as_slice() {
                    net.push(Piece { a: *a, b: *b, run: templates.len(), bottom, main });
                    templates.push(r.clone());
                }
                // Only the stretches no pipe of its system already runs along.
                for (pts, bottoms) in clip_off(&r.points, &r.bottoms_ft, &there, ru, upf, joint.max(1e-9)) {
                    let mut c = r.clone();
                    c.points = pts;
                    c.bottoms_ft = bottoms;
                    there.extend(c.points.windows(2).map(|w| [w[0], w[1]]));
                    out.runs.push(c);
                }
            }
            // A room's branches, joined now: a branch the next need may take.
            for [a, b] in need.segs.iter().take(MAX_ITEMS) {
                net.push(Piece { a: *a, b: *b, run, bottom, main: false });
            }
            out.served += 1;
        }
        out.unserved.extend(left.iter().take(MAX_LISTED).map(|n| format!("{sys} for {}", n.what)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ru() -> FillRules {
        rules()
    }

    /// A made-up run (feet, upf 12).
    fn run(system: &str, level: &str, pts: &[[f64; 2]]) -> MepRun {
        MepRun {
            system: system.into(),
            level: level.into(),
            points: pts.iter().map(|p| [p[0] * 12.0, p[1] * 12.0]).collect(),
            size_in: [2.0, 0.0],
            bottoms_ft: vec![9.0; pts.len().saturating_sub(1).max(1)],
            objects: vec!["1A".into()],
            id: String::new(),
            encased: false,
            size_source: "tag".into(),
            height_source: "default".into(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: "P-91".into(),
            slope_in_per_ft: 0.0,
        }
    }

    fn rect(words: &str, x: f64, y: f64, w: f64, h: f64, label: &str) -> AreaRoom {
        AreaRoom {
            words: words.split_whitespace().map(str::to_string).collect(),
            outline: [(x, y), (x + w, y), (x + w, y + h), (x, y + h)].iter().map(|(a, b)| [a * 12.0, b * 12.0]).collect(),
            label: label.into(),
        }
    }

    /// A floor at `(x0, y0)`: `n` 12' x 15' rooms along a 5' corridor; `commons`: the rooms of
    /// the row replaced by one atrium on this floor.
    fn floor(x0: f64, y0: f64, n: usize, typ: &str, commons: std::ops::Range<usize>) -> Vec<AreaRoom> {
        let mut v: Vec<AreaRoom> =
            (0..n).filter(|i| !commons.contains(i)).map(|i| rect(typ, x0 + 12.0 * i as f64, y0 + 5.0, 12.0, 15.0, &format!("{typ} {i}"))).collect();
        if !commons.is_empty() {
            let (a, b) = (commons.start as f64, commons.end as f64);
            v.push(rect("ATRIUM", x0 + 12.0 * a, y0, 12.0 * (b - a), 20.0, "ATRIUM"));
            // The corridor stops at the atrium.
            v.push(rect("CORRIDOR", x0, y0, 12.0 * a, 5.0, "CORRIDOR W"));
            v.push(rect("CORRIDOR", x0 + 12.0 * b, y0, 12.0 * (n as f64 - b), 5.0, "CORRIDOR E"));
        } else {
            v.push(rect("CORRIDOR", x0, y0, 12.0 * n as f64, 5.0, "CORRIDOR"));
        }
        v
    }

    fn ft(r: &MepRun) -> f64 {
        r.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>() / 12.0
    }

    #[test]
    fn a_floor_takes_the_mains_of_the_floor_below_only_over_the_area_its_rooms_stack_on() {
        // Floor 1 (drawn 400' east): guest rooms 0-3 and 7-9, an atrium under rooms 4-6 of floor 2.
        let below = floor(400.0, 0.0, 10, "STUDIO", 4..7);
        let mine = floor(0.0, 0.0, 10, "STUDIO", 0..0);
        // Floor 1's corridor main the whole length (through the atrium), and a room branch.
        let runs = vec![run("CW", "Level 1", &[[401.0, 2.5], [519.0, 2.5]]), run("CW", "Level 1", &[[403.0, 8.0], [403.0, 12.0]])];
        let src = Source { level: "Level 1".into(), rooms: below, runs };
        let d = derive(&mine, "Level 2", &[src], 12.0, &ru()).expect("derived");
        assert_eq!(d.from, "Level 1");
        // 10 guest rooms + the corridor here; 7 guest rooms stack (the corridor here is one room,
        // floor 1's are two: neither of those is the same place).
        assert_eq!(d.rooms, 11);
        assert_eq!(d.stacked, 7, "{:?}", d.unstacked);
        assert!(d.unstacked.iter().any(|l| l == "STUDIO 5"), "{:?}", d.unstacked);
        // The main copied where the corridor stacks on the corridor: x 1..48 and 84..119, not over the atrium.
        let mut spans: Vec<(f64, f64)> = d.runs.iter().map(|r| (r.points[0][0] / 12.0, r.points.last().unwrap()[0] / 12.0)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!((spans[0].0 - 1.0).abs() < 0.01 && (spans[0].1 - 48.0).abs() <= 1.0, "{spans:?}");
        assert!((spans[1].0 - 84.0).abs() <= 1.0 && (spans[1].1 - 119.0).abs() < 0.01, "{spans:?}");
        for r in &d.runs {
            assert_eq!(provenance(r), DERIVED);
            assert_eq!(r.level, "Level 2");
            assert!(r.size_source.contains("derived from Level 1"), "{}", r.size_source);
            assert!(r.objects.is_empty(), "a copy was read from no plan object");
            assert_eq!(r.bottoms_ft.len(), r.points.len() - 1);
            assert_eq!(r.points.len(), 2, "a straight stretch is one segment: {:?}", r.points);
        }
        // The room's own branch is no main.
        assert!(d.runs.iter().all(|r| ft(r) > 10.0));
    }

    #[test]
    fn a_floor_is_derived_from_the_first_source_with_rooms_and_none_without_like_rooms() {
        let mine = floor(0.0, 0.0, 6, "SUITE", 0..0);
        let empty = Source { level: "Level 9".into(), rooms: Vec::new(), runs: vec![run("CW", "Level 9", &[[0.0, 0.0], [10.0, 0.0]])] };
        let other = Source {
            level: "Level 3".into(),
            rooms: floor(300.0, 0.0, 6, "STUDIO", 0..0),
            runs: vec![run("CW", "Level 3", &[[301.0, 2.5], [371.0, 2.5]]), run("CW", "Level 3", &[[301.0, 10.0], [371.0, 10.0]])],
        };
        // The first has no rooms: the next is used. Its guest rooms are of another type: only the
        // corridor stacks, and only the corridor's main is copied, not the one through the rooms.
        let d = derive(&mine, "Level 2", &[empty, other], 12.0, &ru()).expect("the corridors lay over");
        assert_eq!(d.from, "Level 3");
        assert_eq!((d.stacked, d.rooms), (1, 7));
        assert_eq!(d.runs.len(), 1, "{:?}", d.runs.iter().map(|r| &r.points).collect::<Vec<_>>());
        assert!((d.runs[0].points[0][1] / 12.0 - 2.5).abs() < 1e-6);
        // No like rooms at all: nothing lays the plans over each other.
        let lone = Source { level: "Level 3".into(), rooms: vec![rect("STAIR", 0.0, 0.0, 10.0, 20.0, "S")], runs: Vec::new() };
        assert!(derive(&mine, "Level 2", &[lone], 12.0, &ru()).is_none());
        assert!(derive(&[], "Level 2", &[], 12.0, &ru()).is_none());
    }

    #[test]
    fn a_fixture_with_no_water_in_reach_is_teed_off_the_nearest_main_and_a_main_carried_on_past_its_end() {
        let r = ru();
        let main = {
            let mut m = run("CW", "Level 2", &[[0.0, 2.5], [48.0, 2.5]]);
            m.height_source = DERIVED.into();
            m
        };
        let mains = vec![&main];
        // Beside the main (x 20): straight across. Past its end (x 70): the main on to x 70, then
        // the branch. Out of reach (x 400): left, listed.
        let fixtures = vec![
            Fixture { at: [20.0 * 12.0, 30.0 * 12.0], what: "P-91".into(), lines: vec!["CW".into(), "SAN".into()], words: String::new() },
            Fixture { at: [70.0 * 12.0, 30.0 * 12.0], what: "P-92".into(), lines: vec!["CW".into(), "HW".into()], words: String::new() },
            Fixture { at: [400.0 * 12.0, 30.0 * 12.0], what: "P-93".into(), lines: vec!["CW".into()], words: String::new() },
        ];
        let n = needs(&mains, &fixtures, 15.0, 12.0, &r, true);
        // CW for all three (none within 15 ft), HW for the second, the first's waste.
        assert_eq!(n.len(), 5, "{n:?}");
        let inf = infer("Level 2", &mains, &mains, n, 12.0, &r);
        assert_eq!(inf.served, 2);
        assert_eq!(inf.unserved.len(), 3, "{:?}", inf.unserved);
        assert!(inf.unserved.iter().any(|u| u.contains("SAN")), "no waste pipe on the floor: {:?}", inf.unserved);
        assert!(inf.unserved.iter().any(|u| u.contains("HW")), "no HW main on the floor: {:?}", inf.unserved);
        assert!(inf.unserved.iter().any(|u| u.contains("P-93")), "{:?}", inf.unserved);
        for x in &inf.runs {
            assert_eq!(provenance(x), INFERRED);
            assert!(x.size_source.starts_with("inferred: "), "{}", x.size_source);
            assert_eq!(x.bottoms_ft, vec![9.0], "at the main's height");
            assert!(x.objects.is_empty());
        }
        // The tee: (20, 2.5) to (20, 30), the rules' branch size.
        let tee = inf.runs.iter().find(|x| (x.points[0][0] / 12.0 - 20.0).abs() < 1e-6).expect("the tee");
        assert!((tee.points[0][1] / 12.0 - 2.5).abs() < 1e-6 && (tee.points[1][1] / 12.0 - 30.0).abs() < 1e-6, "{:?}", tee.points);
        assert!((tee.size_in[0] - r.branch_size_in).abs() < 1e-9);
        // The main carried on from its end (48) to 70 at the main's size, then the branch.
        let on = inf.runs.iter().find(|x| x.size_source.contains("main carried on")).expect("the main carried on");
        assert!((on.points[0][0] / 12.0 - 48.0).abs() < 1e-6 && (on.points[1][0] / 12.0 - 70.0).abs() < 1e-6, "{:?}", on.points);
        assert!((on.size_in[0] - 2.0).abs() < 1e-9);
        assert_eq!(inf.runs.len(), 3);
        // A fixture with water in reach needs nothing.
        let near = vec![Fixture { at: [20.0 * 12.0, 10.0 * 12.0], what: "P-94".into(), lines: vec!["CW".into()], words: String::new() }];
        assert!(needs(&mains, &near, 15.0, 12.0, &r, true).is_empty());
    }

    #[test]
    fn a_rooms_branches_joined_to_no_main_are_run_to_it_once_and_the_next_room_tees_off_that() {
        let r = ru();
        let mut main = run("CW", "Level 2", &[[0.0, 2.5], [24.0, 2.5]]);
        main.height_source = DERIVED.into();
        // Two rooms' branches past the main's end, each two joined runs; a third touching the main.
        let b1 = run("CW", "Level 2", &[[40.0, 6.0], [40.0, 12.0]]);
        let b1b = run("CW", "Level 2", &[[40.0, 12.0], [44.0, 12.0]]);
        let b2 = run("CW", "Level 2", &[[60.0, 6.0], [60.0, 12.0]]);
        let mut b3 = run("CW", "Level 2", &[[10.0, 2.5], [10.0, 9.0]]);
        b3.size_in = [0.75, 0.0];
        let all = vec![&main, &b1, &b1b, &b2, &b3];
        // A floor with a water plan: its rooms' branches are its own to join.
        assert!(needs(&all, &[], 15.0, 12.0, &r, false).is_empty());
        let n = needs(&all, &[], 15.0, 12.0, &r, true);
        assert_eq!(n.len(), 2, "{n:?}");
        assert!(n.iter().any(|x| x.what == "the branches of 2 run(s)"));
        let inf = infer("Level 2", &[&main], &all, n, 12.0, &r);
        assert_eq!(inf.served, 2);
        // Main on 24 → 40, tee to (40, 6); then 40 → 60 on, tee to (60, 6): the second carries the
        // first's extension on, not a second line beside it.
        let on: Vec<(f64, f64)> =
            inf.runs.iter().filter(|x| x.size_source.contains("main carried on")).map(|x| (x.points[0][0] / 12.0, x.points[1][0] / 12.0)).collect();
        assert_eq!(on, vec![(24.0, 40.0), (40.0, 60.0)], "{:?}", inf.runs.iter().map(|x| &x.points).collect::<Vec<_>>());
        let total: f64 = inf.runs.iter().map(ft).sum();
        assert!((total - (16.0 + 3.5 + 20.0 + 3.5)).abs() < 1e-6, "{total}");
    }

    #[test]
    fn fixtures_of_one_room_share_one_branch_and_a_room_beside_a_joined_one_takes_its_pipes() {
        let r = ru();
        let mut main = run("CW", "Level 2", &[[0.0, 2.5], [100.0, 2.5]]);
        main.height_source = DERIVED.into();
        // A lavatory and a tub 3' apart, 20' from the main: one line from the main to the first,
        // the second teed off it (not a second line beside it).
        let fixtures = vec![
            Fixture { at: [40.0 * 12.0, 22.5 * 12.0], what: "P-91".into(), lines: vec!["CW".into()], words: String::new() },
            Fixture { at: [43.0 * 12.0, 22.5 * 12.0], what: "P-92".into(), lines: vec!["CW".into()], words: String::new() },
        ];
        let n = needs(&[&main], &fixtures, 15.0, 12.0, &r, true);
        assert_eq!(n.len(), 2);
        let inf = infer("Level 2", &[&main], &[&main], n, 12.0, &r);
        assert_eq!(inf.served, 2);
        let total: f64 = inf.runs.iter().map(ft).sum();
        assert!(
            (total - (20.0 + 3.0)).abs() < 1e-6,
            "one 20' line and a 3' stub: {total} {:?}",
            inf.runs.iter().map(|x| &x.points).collect::<Vec<_>>()
        );
        assert!(inf.runs.iter().all(|x| !x.size_source.contains("main carried on")), "a stub off a branch is the branch");
        // Two rooms' branches back to back 2' apart, 30' from the main: the second takes the first's pipes.
        let a = run("HW", "Level 2", &[[60.0, 32.5], [60.0, 40.0]]);
        let b = run("HW", "Level 2", &[[62.0, 33.0], [62.0, 40.0]]);
        let mut hmain = run("HW", "Level 2", &[[0.0, 2.5], [100.0, 2.5]]);
        hmain.height_source = DERIVED.into();
        let n = needs(&[&hmain, &a, &b], &[], 15.0, 12.0, &r, true);
        assert_eq!(n.len(), 2, "{n:?}");
        let inf = infer("Level 2", &[&hmain], &[&hmain, &a, &b], n, 12.0, &r);
        assert_eq!(inf.served, 2);
        let total: f64 = inf.runs.iter().map(ft).sum();
        assert!((total - (30.0 + 2.0)).abs() < 1e-6, "{total} {:?}", inf.runs.iter().map(|x| &x.points).collect::<Vec<_>>());
    }

    #[test]
    fn a_runs_provenance_is_the_first_word_of_its_height_source() {
        let mut r = run("CW", "Level 2", &[[0.0, 0.0], [1.0, 0.0]]);
        for (h, p) in [
            ("derived", DERIVED),
            ("derived (capped under the floor above)", DERIVED),
            ("Inferred", INFERRED),
            ("inferred (capped under the floor above)", INFERRED),
            ("default", DRAWN),
            ("tag", DRAWN),
            ("", DRAWN),
            ("derivedish", DRAWN),
            ("below slab: company default", DRAWN),
        ] {
            r.height_source = h.into();
            assert_eq!(provenance(&r), p, "{h}");
        }
    }

    #[test]
    fn a_piece_is_not_laid_where_a_pipe_of_its_system_already_runs_and_a_tee_or_crossing_is_not_along() {
        let r = ru();
        let mut main = run("CW", "Level 2", &[[0.0, 2.5], [48.0, 2.5]]);
        main.height_source = DERIVED.into();
        // A drawn pipe up from the main to 25 ft; the fixture's need came from the main alone.
        let up = run("CW", "Level 2", &[[20.0, 2.5], [20.0, 25.0]]);
        let fixtures = vec![Fixture { at: [20.0 * 12.0, 30.0 * 12.0], what: "P-91".into(), lines: vec!["CW".into()], words: String::new() }];
        let n = needs(&[&main], &fixtures, 15.0, 12.0, &r, true);
        assert_eq!(n.len(), 1);
        let inf = infer("Level 2", &[&main], &[&main, &up], n, 12.0, &r);
        assert_eq!(inf.served, 1);
        // Only the 5 ft past the drawn pipe is laid.
        let total: f64 = inf.runs.iter().map(ft).sum();
        assert!((total - 5.0).abs() < 0.3, "{total} {:?}", inf.runs.iter().map(|x| &x.points).collect::<Vec<_>>());
        assert!(inf.runs.iter().all(|x| provenance(x) == INFERRED));
        // Beside it 3 in off: along. A tee off it and a crossing: not.
        let seg = [[[0.0, 0.0], [120.0, 0.0]]];
        assert!(open_parts([12.0, 3.0], [96.0, 3.0], &seg, 6.0, 0.26, 3.0).is_empty());
        assert_eq!(open_parts([60.0, 0.0], [60.0, 120.0], &seg, 6.0, 0.26, 3.0), vec![(0.0, 1.0)]);
        assert_eq!(open_parts([60.0, -60.0], [60.0, 60.0], &seg, 6.0, 0.26, 3.0), vec![(0.0, 1.0)]);
        // Carried on past its end: not along; half over it: the other half is laid.
        assert_eq!(open_parts([120.0, 0.0], [240.0, 0.0], &seg, 6.0, 0.26, 3.0), vec![(0.0, 1.0)]);
        let half = open_parts([60.0, 0.0], [180.0, 0.0], &seg, 6.0, 0.26, 3.0);
        assert_eq!(half.len(), 1);
        assert!((half[0].0 - 0.5).abs() < 0.03 && (half[0].1 - 1.0).abs() < 1e-12, "{half:?}");
        // A polyline clipped in its middle is two pieces, each at its segment's bottom.
        let pts = [[0.0, 24.0], [60.0, 24.0], [60.0, 180.0]];
        let over = [[[-6.0, 24.0], [30.0, 24.0]], [[60.0, 60.0], [60.0, 120.0]]];
        let parts = clip_off(&pts, &[8.0, 9.0], &over, &r, 12.0, 1.0);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert!((parts[0].0[0][0] - 30.0).abs() < 3.1 && parts[0].0.len() == 3 && parts[0].1 == vec![8.0, 9.0], "{parts:?}");
        assert!((parts[1].0[0][1] - 120.0).abs() < 3.1 && parts[1].1 == vec![9.0], "{parts:?}");
    }

    #[test]
    fn a_derived_main_is_cut_where_a_drawn_pipe_of_its_system_runs_along_it_on_its_floor() {
        let r = ru();
        let mut d = run("CW", "Level 2", &[[0.0, 2.5], [40.0, 2.5]]);
        d.height_source = DERIVED.into();
        let drawn = run("CW", "Level 2", &[[10.0, 2.6], [20.0, 2.6]]);
        let other_floor = run("CW", "Level 3", &[[0.0, 2.5], [40.0, 2.5]]);
        let other_sys = run("HW", "Level 2", &[[0.0, 2.5], [40.0, 2.5]]);
        let mut runs = vec![d, drawn, other_floor, other_sys];
        let cut = clip_derived(&mut runs, 12.0, &r);
        assert!((cut - 10.0).abs() < 0.6, "{cut}");
        let derived: Vec<&MepRun> = runs.iter().filter(|x| provenance(x) == DERIVED).collect();
        assert_eq!(derived.len(), 2, "{:?}", derived.iter().map(|x| &x.points).collect::<Vec<_>>());
        assert!(derived.iter().all(|x| x.level == "Level 2" && x.system == "CW"));
        assert_eq!(runs.len(), 5, "the drawn ones stay as they are");
        // Nothing derived: nothing changes.
        let mut plain = vec![run("CW", "Level 2", &[[0.0, 0.0], [5.0, 0.0]])];
        assert_eq!(clip_derived(&mut plain, 12.0, &r), 0.0);
        assert_eq!(plain.len(), 1);
    }

    #[test]
    fn a_pipe_at_the_edge_of_reach_is_not_counted_on() {
        let r = ru();
        let main = run("HW", "Level 2", &[[0.0, 0.0], [40.0, 0.0]]);
        let at = |y: f64| vec![Fixture { at: [20.0 * 12.0, y * 12.0], what: "P-95".into(), lines: vec!["HW".into()], words: String::new() }];
        assert_eq!(needs(&[&main], &at(14.8), 15.0, 12.0, &r, false).len(), 1);
        assert!(needs(&[&main], &at(13.5), 15.0, 12.0, &r, false).is_empty());
    }

    #[test]
    fn a_main_the_source_floor_draws_twice_is_copied_once() {
        let below = floor(400.0, 0.0, 6, "STUDIO", 0..0);
        let mine = floor(0.0, 0.0, 6, "STUDIO", 0..0);
        // The corridor main, and the same main again 3 in off along half of it (a second view).
        let runs = vec![run("CW", "Level 1", &[[401.0, 2.5], [471.0, 2.5]]), run("CW", "Level 1", &[[431.0, 2.75], [466.0, 2.75]])];
        let d = derive(&mine, "Level 2", &[Source { level: "Level 1".into(), rooms: below, runs }], 12.0, &ru()).expect("derived");
        let total: f64 = d.runs.iter().map(ft).sum();
        assert!((total - 70.0).abs() < 0.6, "one main, not one and a half: {total} {:?}", d.runs.iter().map(|r| &r.points).collect::<Vec<_>>());
    }

    #[test]
    fn a_fixtures_waste_and_vent_are_run_to_the_nearest_pipe_at_the_codes_size_and_a_stack_vents_it() {
        let r = ru();
        // A drawn floor: its waste main along y 2.5, a vent main along y 0; a water closet 25 ft off.
        let san = run("SAN", "Level 3", &[[0.0, 2.5], [60.0, 2.5]]);
        let vent = run("V", "Level 3", &[[0.0, 0.0], [60.0, 0.0]]);
        let wc = Fixture {
            at: [30.0 * 12.0, 27.5 * 12.0],
            what: "P-91".into(),
            lines: vec!["SAN".into(), "V".into()],
            words: "P-91 WATER CLOSET floor mounted".into(),
        };
        let all = vec![&san, &vent];
        let n = needs(&all, std::slice::from_ref(&wc), 15.0, 12.0, &r, false);
        assert_eq!(n.len(), 2, "{n:?}");
        let inf = infer("Level 3", &all, &all, n, 12.0, &r);
        assert_eq!(inf.served, 2, "{:?}", inf.unserved);
        let s = inf.runs.iter().find(|x| x.system == "SAN").expect("its waste");
        assert!((s.size_in[0] - 3.0).abs() < 1e-9, "{:?}", s.size_in);
        assert!(s.size_source.contains(&r.drain_basis) && s.size_source.contains("water closet"), "{}", s.size_source);
        assert!((ft(s) - 25.0).abs() < 1e-6);
        let v = inf.runs.iter().find(|x| x.system == "V").expect("its vent");
        assert!((v.size_in[0] - 1.5).abs() < 1e-9, "{:?}", v.size_in);
        assert!(inf.runs.iter().all(|x| provenance(x) == INFERRED && x.slope_in_per_ft == 0.0), "the drainage step slopes them");
        // A waste stack beside it vents it (the stack vent): only its waste is run.
        let mut stack = run("SAN", "Level 3", &[[31.0, 25.0]]);
        stack.top_ft = Some(40.0);
        let n = needs(&[&san, &stack], std::slice::from_ref(&wc), 15.0, 12.0, &r, false);
        assert!(n.iter().all(|x| x.system != "V"), "{n:?}");
        // The code's sizes by the fixture's words, the first rule that matches.
        for (sys, words, size) in [
            ("SAN", "P-93 TUB SHOWER", 1.5),
            ("SAN", "P-92 ADA LAVATORY wall hung", 1.25),
            ("SAN", "P-94 ROLL IN SHOWER", 2.0),
            ("SAN", "P-95 KITCHEN SINK", 1.5),
            ("SAN", "FCO CLEANOUT", 3.0),
            ("SAN", "X-1 SOMETHING", 2.0),
            ("V", "P-91 WATER CLOSET", 1.5),
            ("V", "P-92 LAVATORY", 1.25),
        ] {
            assert_eq!(r.size_for(sys, words).map(|z| z.0), Some(size), "{sys} {words}");
        }
        assert!(r.size_for("CW", "P-91 WATER CLOSET").is_none());
    }

    #[test]
    fn the_fill_in_rules_are_data_with_their_bases() {
        let r = ru();
        assert_eq!(r.water, ["CW", "HW", "HWR"]);
        assert_eq!(r.stack_on, ["below", "above"]);
        for b in [&r.stack_basis, &r.main_basis, &r.branch_basis] {
            assert!(b.len() > 20, "{b}");
        }
        assert!(r.branch_size_in > 0.0 && r.reach_ft > 0.0 && r.joint_ft > 0.0 && r.sample_ft > 0.0);
        assert!(r.share_ft > r.joint_ft && r.share_ft < r.reach_ft);
        assert_eq!(r.drainage, ["SAN", "V"]);
        assert!(r.drainage.contains(&r.vent) && !r.vent_stacks.is_empty());
        assert!(r.drain_basis.len() > 20 && r.vent_stack_basis.len() > 20);
        assert!(r.sizes.iter().all(|z| z.size_in > 0.0 && !z.source.is_empty()), "every size has its source");
        assert!(r.along_ft > 0.0 && r.along_ft < r.joint_ft && r.along_deg > 0.0 && r.along_deg < 45.0);
    }
}
