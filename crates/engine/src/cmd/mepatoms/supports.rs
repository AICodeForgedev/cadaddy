//! Raceway supports beyond straps (Q6353): no drawing shows them, the field hangs every raceway.
//! Two or more raceways running side by side (a feeder bank, a group of homeruns, a riser stack)
//! go on strut: a trapeze (strut channel, a strut strap per raceway, two threaded rods up to the
//! structure, an anchor and the nuts on each rod) overhead, a wall strut (strut, a strut strap per
//! raceway, its anchors) on a riser stack. The supports keep the raceways' own code spacing (the
//! closest of those they carry) and stand at each end of the stretch the raceways share; the
//! straps they replace go. A raceway run alone stays on its own straps. A raceway rising through
//! a floor takes a riser clamp there. Rules: `supports` in `mepatoms.json`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_geom::Vec3;
use serde::Deserialize;

use super::{Atom, support_stations};

/// Most legs looked at, pair tests made, legs in one bank, supports made (hostile models).
const MAX_LEGS: usize = 400_000;
const MAX_PAIR_TESTS: usize = 40_000_000;
const MAX_BANK_LEGS: usize = 200_000;
const MAX_SUPPORTS: usize = 200_000;
/// Most raceways one support carries.
const MAX_ON_SUPPORT: usize = 64;

/// The rules data's `supports`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SupportRules {
    /// How many raceways side by side make a bank.
    pub bank_min_runs: usize,
    /// The widest clear gap (inches) between two raceways side by side that share a support.
    pub bank_gap_in: f64,
    /// The shortest stretch (feet) they run together to share one.
    pub bank_min_ft: f64,
    /// Raceways whose centres are this much (inches) more than half the larger's diameter apart
    /// in height lie on different tiers.
    pub tier_tol_in: f64,
    pub bank_source: String,
    pub strut_item: String,
    pub strut_size: String,
    pub strut_depth_in: f64,
    pub strut_overhang_in: f64,
    pub strut_source: String,
    pub strut_strap_item: String,
    pub strut_strap_source: String,
    pub rod_item: String,
    pub rod_size: String,
    pub rod_diameter_in: f64,
    pub rods_per_trapeze: usize,
    pub rod_default_ft: f64,
    pub rod_source: String,
    pub anchor_item: String,
    pub anchors_per_rod: usize,
    pub wall_anchor_item: String,
    pub wall_anchors: usize,
    pub anchor_source: String,
    pub nut_item: String,
    pub nuts_per_rod: usize,
    pub nut_source: String,
    pub riser_clamp_item: String,
    pub riser_source: String,
}

/// One straight piece of a raceway stick (sheet units).
#[derive(Clone, Debug, Default)]
pub(crate) struct Leg {
    /// The stick's atom.
    pub atom: usize,
    /// Its run (legs of one run never share a support with each other).
    pub run: usize,
    pub a: Vec3,
    pub b: Vec3,
    /// Its outside diameter, inches (the trade size).
    pub dia_in: f64,
    /// The longest span its own code allows between supports, feet.
    pub spacing_ft: f64,
    pub size: String,
    /// Its material (`EMT`).
    pub ty: String,
    /// The underside of the structure over it (sheet units), when known.
    pub top_z: Option<f64>,
}

/// A raceway strap: its atom, the stick it holds, where it is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Strap {
    pub atom: usize,
    pub held: usize,
    pub at: Vec3,
}

/// A new piece: the atom, the existing atom whose context (level, run, circuit) it takes, and
/// whether it is shared by the bank (its run and circuit cleared) or a raceway's own.
#[derive(Clone, Debug)]
pub(crate) struct NewPiece {
    pub atom: Atom,
    pub ctx_of: usize,
    pub shared: bool,
}

/// What the supports pass makes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Planned {
    /// The new pieces; their `connects` are final indices (the list appended at `base`).
    pub pieces: Vec<NewPiece>,
    /// Existing atoms that connect to a new piece: (existing, new index).
    pub back: Vec<(usize, usize)>,
    /// The straps the supports replace.
    pub dropped: BTreeSet<usize>,
    pub trapezes: usize,
    pub wall_struts: usize,
    pub riser_clamps: usize,
    pub banks: usize,
    pub strut_ft: f64,
    pub rod_ft: f64,
}

/// A leg as the bank search sees it: its axis bucket, along coordinates and lateral position.
#[derive(Clone, Copy, Debug)]
struct Placed {
    leg: usize,
    bucket: i32,
    t0: f64,
    t1: f64,
    /// Horizontal: (across in plan, height); vertical: (x, y).
    lat: (f64, f64),
}

/// The axis of a bucket: horizontal ones by whole degrees 0..180, vertical 180.
fn axis(bucket: i32) -> (Vec3, Vec3) {
    if bucket >= 180 {
        return (Vec3::Z, Vec3::new(1.0, 0.0, 0.0));
    }
    let r = f64::from(bucket).to_radians();
    let u = Vec3::new(r.cos(), r.sin(), 0.0);
    (u, Vec3::new(-u.y, u.x, 0.0))
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn place(i: usize, l: &Leg) -> Option<Placed> {
    let d = l.b - l.a;
    let len = d.len();
    if !(len > 1e-9 && l.a.is_finite() && l.b.is_finite()) {
        return None;
    }
    let uz = d.z / len;
    let bucket = if uz.abs() > 0.9 {
        180
    } else if uz.abs() < 0.1 {
        let deg = d.y.atan2(d.x).to_degrees().rem_euclid(180.0);
        (deg.round() as i32).rem_euclid(180)
    } else {
        return None;
    };
    let (u, n) = axis(bucket);
    let (ta, tb) = (dot(l.a, u), dot(l.b, u));
    let mid = (l.a + l.b) * 0.5;
    let lat = if bucket >= 180 { (mid.x, mid.y) } else { (dot(mid, n), mid.z) };
    Some(Placed { leg: i, bucket, t0: ta.min(tb), t1: ta.max(tb), lat })
}

/// Whether two legs of one axis lie side by side close enough to share a support (not looking at
/// their lengths). `upf`: sheet units per foot.
fn beside(p: &Placed, q: &Placed, lp: &Leg, lq: &Leg, r: &SupportRules, upf: f64) -> bool {
    let inch = upf / 12.0;
    let half = (lp.dia_in + lq.dia_in) / 2.0 * inch;
    let gap = r.bank_gap_in.max(0.0) * inch;
    if p.bucket >= 180 {
        let d = ((p.lat.0 - q.lat.0).powi(2) + (p.lat.1 - q.lat.1).powi(2)).sqrt();
        return d - half <= gap;
    }
    let tier = (lp.dia_in.max(lq.dia_in) / 2.0 + r.tier_tol_in.max(0.0)) * inch;
    (p.lat.1 - q.lat.1).abs() <= tier && (p.lat.0 - q.lat.0).abs() - half <= gap
}

fn root(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    let mut n = 0;
    while let Some(&p) = parent.get(r) {
        if p == r || n > parent.len() {
            break;
        }
        r = p;
        n += 1;
    }
    // Path compression: each node on the way points at the root.
    let mut j = i;
    for _ in 0..=n {
        if j == r {
            break;
        }
        let Some(x) = parent.get_mut(j) else { break };
        let up = *x;
        *x = r;
        j = up;
    }
    r
}

/// The point of leg `l` at along-coordinate `t` of axis `u`.
fn point_at(l: &Leg, u: Vec3, t: f64) -> Vec3 {
    let (ta, tb) = (dot(l.a, u), dot(l.b, u));
    let k = if (tb - ta).abs() > 1e-12 { ((t - ta) / (tb - ta)).clamp(0.0, 1.0) } else { 0.0 };
    l.a + (l.b - l.a) * k
}

/// The supports of a model's raceways: banks of `legs` running side by side get strut supports
/// in place of their `straps`; risers through `floors` (sheet-unit elevations, not the lowest
/// level's) get riser clamps. New pieces are numbered from `base`, at most `room` of them.
pub(crate) fn plan(legs: &[Leg], straps: &[Strap], floors: &[f64], r: &SupportRules, upf: f64, base: usize, room: usize) -> Planned {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut out = Planned::default();
    let legs = legs.get(..legs.len().min(MAX_LEGS)).unwrap_or_default();
    let placed: Vec<Placed> = legs.iter().enumerate().filter_map(|(i, l)| place(i, l)).collect();
    if r.bank_min_runs >= 2 {
        // A vertical bank is a riser stack (its raceways reach or pass a floor), never drops
        // side by side in a wall's cavity: those are strapped to the framing.
        let eps = 0.5 * upf / 12.0;
        let riser = |p: &Placed| floors.iter().any(|e| p.t0 <= e + eps && p.t1 >= e - eps);
        let bankable: Vec<Placed> = placed.iter().copied().filter(|p| p.bucket < 180 || riser(p)).collect();
        banks(legs, &bankable, straps, r, upf, base, room, &mut out);
    }
    risers(legs, &placed, floors, r, upf, base, room, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn banks(legs: &[Leg], placed: &[Placed], straps: &[Strap], r: &SupportRules, upf: f64, base: usize, room: usize, out: &mut Planned) {
    let inch = upf / 12.0;
    let widest = legs.iter().map(|l| l.dia_in).filter(|d| d.is_finite()).fold(1.0_f64, f64::max).min(8.0);
    let cell = ((r.bank_gap_in.max(0.0) + widest) * inch).max(1e-6);
    let key = |p: &Placed| -> (i32, i64, i64) {
        let c = |v: f64| {
            let k = (v / cell).floor();
            if k.is_finite() { k.clamp(-1e15, 1e15) as i64 } else { 0 }
        };
        (p.bucket, c(p.lat.0), c(p.lat.1))
    };
    let mut grid: HashMap<(i32, i64, i64), Vec<usize>> = HashMap::new();
    for (k, p) in placed.iter().enumerate() {
        grid.entry(key(p)).or_default().push(k);
    }
    // Pairs side by side over a stretch long enough, joined into banks.
    let min_overlap = r.bank_min_ft.max(0.0) * upf;
    let eps = 0.25 * inch;
    let mut parent: Vec<usize> = (0..placed.len()).collect();
    let mut tests = 0usize;
    let mut paired = vec![false; placed.len()];
    'outer: for (k, p) in placed.iter().enumerate() {
        let (b, cx, cy) = key(p);
        let Some(lp) = legs.get(p.leg) else { continue };
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(cands) = grid.get(&(b, cx + dx, cy + dy)) else { continue };
                for &j in cands {
                    if j <= k {
                        continue;
                    }
                    tests += 1;
                    if tests > MAX_PAIR_TESTS {
                        break 'outer;
                    }
                    let (Some(q), Some(lq)) = (placed.get(j), placed.get(j).and_then(|q| legs.get(q.leg))) else { continue };
                    let overlap = p.t1.min(q.t1) - p.t0.max(q.t0);
                    // One run's sticks end to end are one raceway along the bank: joined, not
                    // a pair.
                    let chained = lp.run == lq.run && overlap >= -eps && overlap <= eps;
                    if !chained && (lp.run == lq.run || overlap < min_overlap) || !beside(p, q, lp, lq, r, upf) {
                        continue;
                    }
                    let (ra, rb) = (root(&mut parent, k), root(&mut parent, j));
                    if let Some(x) = parent.get_mut(ra.max(rb)) {
                        *x = ra.min(rb);
                    }
                    if chained {
                        continue;
                    }
                    if let Some(x) = paired.get_mut(k) {
                        *x = true;
                    }
                    if let Some(x) = paired.get_mut(j) {
                        *x = true;
                    }
                }
            }
        }
    }
    // The banks: each set of legs joined, where at least two of them run side by side.
    let mut comps: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut with_pair: BTreeSet<usize> = BTreeSet::new();
    for k in 0..placed.len() {
        let rt = root(&mut parent, k);
        comps.entry(rt).or_default().push(k);
        if paired.get(k).copied().unwrap_or(false) {
            with_pair.insert(rt);
        }
    }
    comps.retain(|rt, _| with_pair.contains(rt));
    // Straps by the stick they hold.
    let mut straps_of: HashMap<usize, Vec<Strap>> = HashMap::new();
    for s in straps {
        straps_of.entry(s.held).or_default().push(*s);
    }
    for members in comps.values() {
        let mut members: Vec<&Placed> = members.iter().take(MAX_BANK_LEGS).filter_map(|k| placed.get(*k)).collect();
        members.sort_by(|a, b| a.t0.total_cmp(&b.t0));
        let Some(bucket) = members.first().map(|p| p.bucket) else { continue };
        let (u, _) = axis(bucket);
        out.banks += 1;
        // The stretches where the same raceways run side by side.
        let mut cuts: Vec<f64> = members.iter().flat_map(|p| [p.t0, p.t1]).collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() <= eps);
        // (runs) → (start, end, legs by run)
        let mut open: BTreeMap<Vec<usize>, (f64, f64, BTreeMap<usize, Vec<usize>>)> = BTreeMap::new();
        let mut done: Vec<(f64, f64, BTreeMap<usize, Vec<usize>>)> = Vec::new();
        // A sweep along the bank: the legs that started, less those that ended.
        let mut next = 0usize;
        let mut live: Vec<&Placed> = Vec::new();
        for w in cuts.windows(2) {
            let [e0, e1] = w else { continue };
            while let Some(p) = members.get(next).filter(|p| p.t0 <= e0 + eps) {
                live.push(p);
                next += 1;
            }
            live.retain(|p| p.t1 >= e1 - eps);
            if e1 - e0 <= eps {
                continue;
            }
            let active: Vec<&Placed> = live.iter().copied().take(MAX_ON_SUPPORT * 4).collect();
            // The active legs side by side, by single linkage.
            let mut par: Vec<usize> = (0..active.len()).collect();
            for i in 0..active.len() {
                for j in i + 1..active.len() {
                    let (Some(p), Some(q)) = (active.get(i), active.get(j)) else { continue };
                    let (Some(lp), Some(lq)) = (legs.get(p.leg), legs.get(q.leg)) else { continue };
                    if lp.run != lq.run && beside(p, q, lp, lq, r, upf) {
                        let (a, b) = (root(&mut par, i), root(&mut par, j));
                        if let Some(x) = par.get_mut(a.max(b)) {
                            *x = a.min(b);
                        }
                    }
                }
            }
            let mut clusters: BTreeMap<usize, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
            for (i, p) in active.iter().enumerate() {
                let Some(l) = legs.get(p.leg) else { continue };
                clusters.entry(root(&mut par, i)).or_default().entry(l.run).or_default().push(p.leg);
            }
            let mut seen: BTreeSet<Vec<usize>> = BTreeSet::new();
            for by_run in clusters.into_values().filter(|c| c.len() >= r.bank_min_runs) {
                let runs: Vec<usize> = by_run.keys().copied().collect();
                seen.insert(runs.clone());
                match open.get_mut(&runs) {
                    Some(s) if (s.1 - e0).abs() <= eps => {
                        s.1 = *e1;
                        for (run, ls) in by_run {
                            s.2.entry(run).or_default().extend(ls);
                        }
                    }
                    _ => {
                        if let Some(old) = open.insert(runs, (*e0, *e1, by_run)) {
                            done.push(old);
                        }
                    }
                }
            }
            let gone: Vec<Vec<usize>> = open.keys().filter(|k| !seen.contains(*k)).cloned().collect();
            for k in gone {
                if let Some(s) = open.remove(&k) {
                    done.push(s);
                }
            }
        }
        done.extend(open.into_values());
        // Supports along each stretch, merged where two stretches meet at one station.
        let mut stations: BTreeMap<i64, BTreeMap<usize, usize>> = BTreeMap::new();
        for (s0, s1, by_run) in &done {
            let len_ft = (s1 - s0) / upf;
            if len_ft + 1e-9 < r.bank_min_ft.max(0.0) {
                continue;
            }
            let spacing =
                by_run.values().flatten().filter_map(|i| legs.get(*i)).map(|l| l.spacing_ft).filter(|s| *s > 0.0).fold(f64::INFINITY, f64::min);
            if !spacing.is_finite() {
                continue;
            }
            for st in support_stations(len_ft, 0.0, spacing) {
                let t = s0 + st * upf;
                let q = (t / eps).round();
                let q = if q.is_finite() { q.clamp(-1e15, 1e15) as i64 } else { 0 };
                let at = stations.entry(q).or_default();
                for (run, ls) in by_run {
                    let on = ls.iter().copied().find(|i| {
                        legs.get(*i).is_some_and(|l| {
                            let (ta, tb) = (dot(l.a, u), dot(l.b, u));
                            t >= ta.min(tb) - eps && t <= ta.max(tb) + eps
                        })
                    });
                    if let Some(i) = on {
                        at.entry(*run).or_insert(i);
                    }
                }
            }
            // The straps of its raceways along the stretch go: the supports hold them there.
            for i in by_run.values().flatten() {
                let Some(l) = legs.get(*i) else { continue };
                for s in straps_of.get(&l.atom).into_iter().flatten() {
                    let t = dot(s.at, u);
                    let near = (point_at(l, u, t) - s.at).len() <= (l.dia_in + 1.0) * inch;
                    if near && t >= s0 - eps && t <= s1 + eps {
                        out.dropped.insert(s.atom);
                    }
                }
            }
        }
        for (q, on) in stations {
            if on.len() < r.bank_min_runs || out.trapezes + out.wall_struts >= MAX_SUPPORTS || out.pieces.len() >= room {
                continue;
            }
            let t = q as f64 * eps;
            let held: Vec<(usize, Vec3)> = on.values().take(MAX_ON_SUPPORT).filter_map(|i| legs.get(*i).map(|l| (*i, point_at(l, u, t)))).collect();
            support(legs, &held, bucket, r, upf, base, out);
        }
    }
}

/// One support across the raceways `held` (leg, point on it).
fn support(legs: &[Leg], held: &[(usize, Vec3)], bucket: i32, r: &SupportRules, upf: f64, base: usize, out: &mut Planned) {
    let inch = upf / 12.0;
    let Some(&(first, p0)) = held.first() else { return };
    let vertical = bucket >= 180;
    // The strut's way: across a horizontal bank in plan; along a riser stack's spread.
    let across = if vertical {
        let far = held.iter().map(|h| h.1).max_by(|a, b| {
            let da = (a.x - p0.x).hypot(a.y - p0.y);
            let db = (b.x - p0.x).hypot(b.y - p0.y);
            da.total_cmp(&db)
        });
        let d = far.map_or(Vec3::new(1.0, 0.0, 0.0), |f| Vec3::new(f.x - p0.x, f.y - p0.y, 0.0));
        if d.len() > 1e-9 { d * (1.0 / d.len()) } else { Vec3::new(1.0, 0.0, 0.0) }
    } else {
        axis(bucket).1
    };
    let dia = |i: usize| legs.get(i).map_or(1.0, |l| l.dia_in.max(0.1));
    let over = r.strut_overhang_in.max(0.0) * inch;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut bottom = f64::INFINITY;
    for (i, p) in held {
        let s = dot(*p - p0, across);
        let half = dia(*i) / 2.0 * inch;
        lo = lo.min(s - half);
        hi = hi.max(s + half);
        bottom = bottom.min(p.z - half);
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return;
    }
    let (lo, hi) = (lo - over, hi + over);
    let depth = r.strut_depth_in.max(0.5) * inch;
    // A trapeze under the bank; a wall strut at the riser stack's height, behind it.
    let z = if vertical { p0.z } else { bottom - depth / 2.0 };
    let at = |s: f64| {
        let q = p0 + across * s;
        Vec3::new(q.x, q.y, z)
    };
    let (sa, sb) = (at(lo), at(hi));
    let strut_ft = (hi - lo) / upf;
    let si = base + out.pieces.len();
    let mut strut = Atom::fitting("strut", r.strut_item.clone(), "strut", &r.strut_size, sa, sb - sa, r.strut_depth_in.max(0.5), &r.strut_source);
    strut.b = sb;
    strut.length_ft = strut_ft;
    strut.extra.push(("support", (if vertical { "wall strut" } else { "trapeze" }).into(), r.bank_source.clone()));
    strut.extra.push(("raceways", held.len().to_string(), "computed".into()));
    out.strut_ft += strut_ft;
    out.pieces.push(NewPiece { atom: strut, ctx_of: legs.get(first).map_or(0, |l| l.atom), shared: true });
    // A strut strap on each raceway.
    for (i, p) in held {
        let Some(l) = legs.get(*i) else { continue };
        let k = base + out.pieces.len();
        let mut a =
            Atom::fitting("strut_strap", r.strut_strap_item.clone(), &l.ty, &l.size, *p, across, l.dia_in.max(0.1) * 1.3, &r.strut_strap_source);
        a.connects = vec![l.atom, si];
        out.back.push((l.atom, k));
        if let Some(s) = out.pieces.get_mut(si - base) {
            s.atom.connects.push(k);
        }
        out.pieces.push(NewPiece { atom: a, ctx_of: l.atom, shared: false });
    }
    let shared = legs.get(first).map_or(0, |l| l.atom);
    if vertical {
        out.wall_struts += 1;
        let n = r.wall_anchors.min(8);
        for k in 0..n {
            let s = if n <= 1 { (lo + hi) / 2.0 } else { lo + over / 2.0 + (hi - lo - over) * k as f64 / (n - 1) as f64 };
            let ai = base + out.pieces.len();
            let mut a = Atom::fitting("support_anchor", r.wall_anchor_item.clone(), "anchor", &r.rod_size, at(s), across, 0.5, &r.anchor_source);
            a.connects = vec![si];
            if let Some(x) = out.pieces.get_mut(si - base) {
                x.atom.connects.push(ai);
            }
            out.pieces.push(NewPiece { atom: a, ctx_of: shared, shared: true });
        }
        return;
    }
    out.trapezes += 1;
    let top = legs.get(first).and_then(|l| l.top_z).filter(|t| t.is_finite() && *t > z + inch);
    let top = top.unwrap_or(z + r.rod_default_ft.max(0.0) * upf);
    let rods = r.rods_per_trapeze.min(8);
    for k in 0..rods {
        let s = if rods <= 1 { (lo + hi) / 2.0 } else { lo + over / 2.0 + (hi - lo - over) * k as f64 / (rods - 1) as f64 };
        let foot = at(s);
        let head = Vec3::new(foot.x, foot.y, top);
        let rod_ft = (top - z) / upf;
        let ri = base + out.pieces.len();
        let mut rod =
            Atom::fitting("rod", r.rod_item.clone(), "threaded rod", &r.rod_size, foot, head - foot, r.rod_diameter_in.max(0.1), &r.rod_source);
        rod.b = head;
        rod.length_ft = rod_ft;
        rod.connects = vec![si];
        if let Some(x) = out.pieces.get_mut(si - base) {
            x.atom.connects.push(ri);
        }
        out.rod_ft += rod_ft;
        out.pieces.push(NewPiece { atom: rod, ctx_of: shared, shared: true });
        for _ in 0..r.anchors_per_rod.min(4) {
            let ai = base + out.pieces.len();
            let mut a = Atom::fitting("support_anchor", r.anchor_item.clone(), "anchor", &r.rod_size, head, Vec3::Z, 0.6, &r.anchor_source);
            a.connects = vec![ri];
            if let Some(x) = out.pieces.get_mut(ri - base) {
                x.atom.connects.push(ai);
            }
            out.pieces.push(NewPiece { atom: a, ctx_of: shared, shared: true });
        }
        for n in 0..r.nuts_per_rod.min(8) {
            let ni = base + out.pieces.len();
            let dz = if n % 2 == 0 { -depth / 2.0 } else { depth / 2.0 };
            let mut a =
                Atom::fitting("strut_nut", r.nut_item.clone(), "nut", &r.rod_size, foot + Vec3::new(0.0, 0.0, dz), Vec3::Z, 0.6, &r.nut_source);
            a.connects = vec![ri];
            if let Some(x) = out.pieces.get_mut(ri - base) {
                x.atom.connects.push(ni);
            }
            out.pieces.push(NewPiece { atom: a, ctx_of: shared, shared: true });
        }
    }
}

/// A riser clamp at each floor a vertical raceway passes: where it rises through the floor's
/// elevation, or reaches it from below (cut there, it runs on in the next level's piece).
#[allow(clippy::too_many_arguments)]
fn risers(legs: &[Leg], placed: &[Placed], floors: &[f64], r: &SupportRules, upf: f64, base: usize, room: usize, out: &mut Planned) {
    if r.riser_clamp_item.is_empty() {
        return;
    }
    let inch = upf / 12.0;
    let eps = 0.5 * inch;
    let mut done: BTreeSet<(i64, i64, usize)> = BTreeSet::new();
    for p in placed.iter().filter(|p| p.bucket >= 180) {
        let Some(l) = legs.get(p.leg) else { continue };
        for (fi, e) in floors.iter().enumerate() {
            if !(p.t0 < e - eps && p.t1 >= e - eps) {
                continue;
            }
            let q = |v: f64| {
                let k = (v / eps).round();
                if k.is_finite() { k.clamp(-1e15, 1e15) as i64 } else { 0 }
            };
            if !done.insert((q(p.lat.0), q(p.lat.1), fi)) || out.pieces.len() >= room {
                continue;
            }
            let at = Vec3::new(p.lat.0, p.lat.1, *e);
            let k = base + out.pieces.len();
            let mut a = Atom::fitting(
                "riser_clamp",
                r.riser_clamp_item.clone(),
                &l.ty,
                &l.size,
                at,
                Vec3::new(1.0, 0.0, 0.0),
                l.dia_in.max(0.1) * 2.0,
                &r.riser_source,
            );
            a.connects = vec![l.atom];
            out.back.push((l.atom, k));
            out.pieces.push(NewPiece { atom: a, ctx_of: l.atom, shared: false });
            out.riser_clamps += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> SupportRules {
        super::super::rules().supports.clone()
    }

    fn leg(atom: usize, run: usize, a: [f64; 3], b: [f64; 3], dia: f64) -> Leg {
        Leg {
            atom,
            run,
            a: Vec3::new(a[0], a[1], a[2]),
            b: Vec3::new(b[0], b[1], b[2]),
            dia_in: dia,
            spacing_ft: 10.0,
            size: "1\"".into(),
            ty: "EMT".into(),
            top_z: Some(12.0 * 12.0),
        }
    }

    fn kinds(p: &Planned) -> BTreeMap<&'static str, usize> {
        let mut m = BTreeMap::new();
        for x in &p.pieces {
            *m.entry(x.atom.kind).or_default() += 1;
        }
        m
    }

    #[test]
    fn the_rules_are_data_with_their_sources() {
        let r = rules();
        assert!(r.bank_min_runs >= 2 && r.bank_gap_in > 0.0 && r.bank_min_ft > 0.0, "{r:?}");
        for s in [&r.bank_source, &r.strut_source, &r.rod_source, &r.anchor_source, &r.riser_source, &r.strut_strap_source, &r.nut_source] {
            assert!(s.len() > 40, "each rule says why: {s}");
        }
        assert!(!r.strut_item.is_empty() && !r.rod_item.is_empty() && !r.riser_clamp_item.is_empty());
    }

    #[test]
    fn three_parallel_raceways_go_on_rod_hung_trapezes() {
        // Three 1" EMT runs, 4" apart centre to centre, 40 ft along x at 9 ft (inches as sheet
        // units), each with its own straps at its own stations.
        let upf = 12.0;
        let legs: Vec<Leg> = (0..3).map(|k| leg(k, k, [0.0, 4.0 * k as f64, 108.0], [480.0, 4.0 * k as f64, 108.0], 1.0)).collect();
        let mut straps = Vec::new();
        for (k, l) in legs.iter().enumerate() {
            for (n, s) in support_stations(40.0, 3.0, 10.0).into_iter().enumerate() {
                straps.push(Strap { atom: 100 + k * 10 + n, held: l.atom, at: Vec3::new(s * upf + k as f64, l.a.y, l.a.z) });
            }
        }
        let p = plan(&legs, &straps, &[], &rules(), upf, 1000, usize::MAX);
        // 40 ft at most 10 ft apart, one at each end: 5 trapezes.
        assert_eq!(p.trapezes, 5, "{p:?}");
        assert_eq!(p.wall_struts, 0);
        let k = kinds(&p);
        assert_eq!(k.get("strut"), Some(&5));
        assert_eq!(k.get("strut_strap"), Some(&15), "one per raceway per support");
        assert_eq!(k.get("rod"), Some(&10), "two rods per trapeze");
        assert_eq!(k.get("support_anchor"), Some(&10), "an anchor per rod");
        assert_eq!(k.get("strut_nut"), Some(&20), "two nuts per rod");
        assert_eq!(p.dropped.len(), straps.len(), "every strap along the shared stretch is replaced");
        // Each rod from the strut up to the structure at 12 ft.
        for x in p.pieces.iter().filter(|x| x.atom.kind == "rod") {
            assert!((x.atom.b.z - 144.0).abs() < 1e-6 && x.atom.a.z < 108.0, "{:?}", x.atom);
            assert!((x.atom.length_ft - (144.0 - x.atom.a.z) / upf).abs() < 1e-9);
        }
        // The strut spans the bank: 8" between the outer centres + 1" of conduit + 3" each side.
        let s = p.pieces.iter().find(|x| x.atom.kind == "strut").map(|x| x.atom.length_ft).unwrap_or(0.0);
        assert!((s - 15.0 / 12.0).abs() < 1e-6, "{s}");
        // Each strut strap holds its own stick and the strut; the sticks connect back.
        for x in p.pieces.iter().filter(|x| x.atom.kind == "strut_strap") {
            assert_eq!(x.atom.connects.len(), 2);
            assert!(x.atom.connects.first().is_some_and(|c| *c < 3));
            assert!(!x.shared);
        }
        assert_eq!(p.back.len(), 15);
    }

    #[test]
    fn a_bank_of_runs_cut_into_sticks_is_one_bank() {
        // The same three runs, each four 10 ft sticks end to end: one bank, five trapezes, not
        // two at every joint.
        let mut legs = Vec::new();
        for k in 0..3 {
            for n in 0..4 {
                let (x0, x1) = (120.0 * n as f64, 120.0 * (n + 1) as f64);
                legs.push(leg(k * 4 + n, k, [x0, 4.0 * k as f64, 108.0], [x1, 4.0 * k as f64, 108.0], 1.0));
            }
        }
        let p = plan(&legs, &[], &[], &rules(), 12.0, 0, usize::MAX);
        assert_eq!((p.banks, p.trapezes), (1, 5), "{p:?}");
        assert_eq!(kinds(&p).get("strut_strap"), Some(&15));
    }

    #[test]
    fn a_raceway_run_alone_keeps_its_straps() {
        let legs = vec![leg(0, 0, [0.0, 0.0, 108.0], [480.0, 0.0, 108.0], 1.0), leg(1, 1, [0.0, 60.0, 108.0], [480.0, 60.0, 108.0], 1.0)];
        let straps = vec![Strap { atom: 9, held: 0, at: Vec3::new(36.0, 0.0, 108.0) }];
        let p = plan(&legs, &straps, &[], &rules(), 12.0, 10, usize::MAX);
        assert_eq!(p.trapezes, 0, "5 ft apart is not a bank");
        assert!(p.pieces.is_empty() && p.dropped.is_empty());
        // Nor one run's own sticks side by side, nor a short stretch together.
        let legs = vec![leg(0, 0, [0.0, 0.0, 108.0], [480.0, 0.0, 108.0], 1.0), leg(1, 0, [0.0, 3.0, 108.0], [480.0, 3.0, 108.0], 1.0)];
        assert_eq!(plan(&legs, &[], &[], &rules(), 12.0, 10, usize::MAX).trapezes, 0);
        let legs = vec![leg(0, 0, [0.0, 0.0, 108.0], [480.0, 0.0, 108.0], 1.0), leg(1, 1, [440.0, 3.0, 108.0], [900.0, 3.0, 108.0], 1.0)];
        assert_eq!(plan(&legs, &[], &[], &rules(), 12.0, 10, usize::MAX).trapezes, 0, "3 ft together is under the shortest stretch");
        // Nor two raceways one over the other on different tiers.
        let legs = vec![leg(0, 0, [0.0, 0.0, 108.0], [480.0, 0.0, 108.0], 1.0), leg(1, 1, [0.0, 0.0, 96.0], [480.0, 0.0, 96.0], 1.0)];
        assert_eq!(plan(&legs, &[], &[], &rules(), 12.0, 10, usize::MAX).trapezes, 0);
    }

    #[test]
    fn supports_stand_where_a_raceway_joins_or_leaves_the_bank() {
        // Run 0 along 0..40 ft; run 1 between it and run 2 from 10 to 30 ft; run 2 beside run 0
        // (5" clear) along 0..40 ft.
        let upf = 12.0;
        let legs = vec![
            leg(0, 0, [0.0, 0.0, 108.0], [480.0, 0.0, 108.0], 1.0),
            leg(1, 1, [120.0, 3.0, 108.0], [360.0, 3.0, 108.0], 1.0),
            leg(2, 2, [0.0, 6.0, 108.0], [480.0, 6.0, 108.0], 1.0),
        ];
        let p = plan(&legs, &[], &[], &rules(), upf, 0, usize::MAX);
        let mut at: Vec<(f64, usize)> = p
            .pieces
            .iter()
            .filter(|x| x.atom.kind == "strut")
            .map(|x| {
                (
                    ((x.atom.a.x / upf) * 100.0).round() / 100.0,
                    x.atom.extra.iter().find(|e| e.0 == "raceways").map_or(0, |e| e.1.parse().unwrap_or(0)),
                )
            })
            .collect();
        at.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Supports at 0, 10 (run 1 joins), 20, 30 (it leaves), 40: never more than 10 ft apart.
        let xs: Vec<f64> = at.iter().map(|a| a.0).collect();
        assert_eq!(xs, vec![0.0, 10.0, 20.0, 30.0, 40.0], "{at:?}");
        assert!(xs.windows(2).all(|w| matches!(w, [a, b] if b - a <= 10.0 + 1e-9)));
        // At the stations where run 1 is there it is held too.
        assert_eq!(at.iter().map(|a| a.1).collect::<Vec<_>>(), vec![2, 3, 3, 3, 2], "{at:?}");
    }

    #[test]
    fn a_riser_stack_gets_wall_strut_and_a_clamp_at_each_floor() {
        // Two 2" risers 4" apart from 0 to 30 ft through floors at 10 and 20 ft.
        let upf = 12.0;
        let mut a = leg(0, 0, [0.0, 0.0, 0.0], [0.0, 0.0, 360.0], 2.0);
        let mut b = leg(1, 1, [4.0, 0.0, 0.0], [4.0, 0.0, 360.0], 2.0);
        a.size = "2\"".into();
        b.size = "2\"".into();
        let p = plan(&[a, b], &[], &[120.0, 240.0], &rules(), upf, 0, usize::MAX);
        assert_eq!(p.trapezes, 0);
        assert_eq!(p.wall_struts, 4, "30 ft at most 10 ft apart: {p:?}");
        let k = kinds(&p);
        assert_eq!(k.get("support_anchor"), Some(&8), "two anchors per wall strut");
        assert_eq!(k.get("rod"), None);
        assert_eq!(p.riser_clamps, 4, "each riser at each of two floors");
        // A riser cut at the floor (it reaches it from below, the next piece runs on above): one
        // clamp there, not two.
        let lo = leg(0, 0, [0.0, 0.0, 0.0], [0.0, 0.0, 120.0], 2.0);
        let hi = leg(1, 0, [0.0, 0.0, 120.0], [0.0, 0.0, 240.0], 2.0);
        let p = plan(&[lo, hi], &[], &[120.0], &rules(), upf, 0, usize::MAX);
        assert_eq!(p.riser_clamps, 1);
        // A drop that stops under the floor above gets none.
        let drop = leg(0, 0, [0.0, 0.0, 60.0], [0.0, 0.0, 110.0], 1.0);
        assert_eq!(plan(&[drop], &[], &[120.0], &rules(), upf, 0, usize::MAX).riser_clamps, 0);
    }

    #[test]
    fn drops_side_by_side_in_a_wall_are_no_riser_stack() {
        let a = leg(0, 0, [0.0, 0.0, 18.0], [0.0, 0.0, 110.0], 0.75);
        let b = leg(1, 1, [4.0, 0.0, 18.0], [4.0, 0.0, 110.0], 0.75);
        let p = plan(&[a, b], &[], &[120.0], &rules(), 12.0, 0, usize::MAX);
        assert_eq!((p.wall_struts, p.riser_clamps), (0, 0), "{p:?}");
    }

    #[test]
    fn hostile_input_is_bounded() {
        let mut l = leg(0, 0, [f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0);
        l.dia_in = f64::INFINITY;
        let p = plan(&[l.clone(), l], &[], &[f64::NAN], &rules(), f64::NAN, 0, usize::MAX);
        assert!(p.pieces.is_empty());
        let legs: Vec<Leg> = (0..50).map(|k| leg(k, k, [0.0, k as f64, 108.0], [480.0, k as f64, 108.0], 1.0)).collect();
        let p = plan(&legs, &[], &[], &rules(), 12.0, 0, 7);
        assert!(p.pieces.len() <= 7 + MAX_ON_SUPPORT * 3, "{}", p.pieces.len());
    }
}
