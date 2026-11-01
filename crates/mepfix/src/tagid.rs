//! Tags are identities: one object per real thing, however many sheets draw it.
//!
//! A door number, a room number, a wall-type tag at a wall's centreline, a device's type and
//! circuit at its spot: each names one thing in the building, and a print set shows it on
//! several sheets (the architectural plan, every discipline's background, enlarged plans).
//! This module holds what the readers and the model share:
//! - [`tag_key`]: whether lettering reads as a door or room number, in one spelling;
//! - [`fit_by_tags`]: the transform (scale, rotation, offset) taking one sheet onto another from
//!   the tags both print, robust to tags only one of them has, with its residual;
//! - [`register_sheets`]: every plan sheet of a level put onto that level's reference plan (its
//!   architectural plan), by shared tags, else by the sheet frame when the sheets are drawn
//!   alike;
//! - [`merge_identities`]: elements read from different sheets with the same key on the same
//!   level, within the kind's tolerance in project coordinates, become one; the same key far
//!   apart is a finding, never a merge.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Most tag pairs one fit tries (each pair of them is a trial transform).
const MAX_FIT_PAIRS: usize = 300;
/// Most places one tag is looked for on the other sheet.
const MAX_PLACES_PER_TAG: usize = 4;
/// Most items one merge looks at.
const MAX_ITEMS: usize = 500_000;

/// A door or room number in one spelling (capitals, no spaces), when `s` reads as one: three or
/// four digits with at most two letters around them and at most one dash or dot (`101`, `101A`,
/// `D-101`, `1.12`, `B204`). Two-digit marks (circuits, dimensions, grid numbers) and words never
/// are.
pub fn tag_key(s: &str) -> Option<String> {
    let t: String = s.trim().chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    if t.is_empty() || t.len() > 8 {
        return None;
    }
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let letters = t.chars().filter(char::is_ascii_alphabetic).count();
    let marks = t.chars().filter(|c| matches!(c, '-' | '.')).count();
    if !(3..=4).contains(&digits) || letters > 2 || marks > 1 || digits + letters + marks != t.chars().count() {
        return None;
    }
    // Letters stand before or after the number, never inside it (`1A2B` is not a tag).
    let first_digit = t.find(|c: char| c.is_ascii_digit())?;
    let last_digit = t.rfind(|c: char| c.is_ascii_digit())?;
    let inner = t.get(first_digit..=last_digit)?;
    if inner.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if t.starts_with(['-', '.']) || t.ends_with(['-', '.']) {
        return None;
    }
    Some(t)
}

// ─── Transforms ─────────────────────────────────────────────────────────────

/// A similarity `p ↦ R·p + t` with `R = [[a, −b], [b, a]]`: scale `√(a²+b²)`, rotation
/// `atan2(b, a)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sim {
    pub a: f64,
    pub b: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Default for Sim {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Sim {
    pub const IDENTITY: Sim = Sim { a: 1.0, b: 0.0, tx: 0.0, ty: 0.0 };

    /// Scale `s` and offset `t`, no rotation.
    pub fn scale_offset(s: f64, t: [f64; 2]) -> Sim {
        Sim { a: s, b: 0.0, tx: t[0], ty: t[1] }
    }

    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.a * p[0] - self.b * p[1] + self.tx, self.b * p[0] + self.a * p[1] + self.ty]
    }

    pub fn scale(&self) -> f64 {
        self.a.hypot(self.b)
    }

    /// Rotation in degrees, −180…180.
    pub fn rotation_deg(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }

    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.tx, self.ty].iter().all(|v| v.is_finite())
    }

    /// The fit back: `self.inverse().apply(self.apply(p)) == p`; none for a fit to a point.
    pub fn inverse(&self) -> Option<Sim> {
        let n = self.a * self.a + self.b * self.b;
        if !(n.is_finite() && n > 1e-18) {
            return None;
        }
        let (a, b) = (self.a / n, -self.b / n);
        Some(Sim { a, b, tx: -(a * self.tx - b * self.ty), ty: -(b * self.tx + a * self.ty) })
    }

    /// `self` then `next`: `p ↦ next(self(p))`.
    pub fn then(&self, next: &Sim) -> Sim {
        let t = next.apply([self.tx, self.ty]);
        Sim { a: next.a * self.a - next.b * self.b, b: next.b * self.a + next.a * self.b, tx: t[0], ty: t[1] }
    }

    /// The transform taking `p0`, `p1` onto `q0`, `q1`.
    fn from_two(p0: [f64; 2], p1: [f64; 2], q0: [f64; 2], q1: [f64; 2]) -> Option<Sim> {
        let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
        let (ex, ey) = (q1[0] - q0[0], q1[1] - q0[1]);
        let n = dx * dx + dy * dy;
        if !n.is_finite() || n <= 1e-18 {
            return None;
        }
        // (a + ib)(dx + i dy) = ex + i ey
        let a = (ex * dx + ey * dy) / n;
        let b = (ey * dx - ex * dy) / n;
        let s = Sim { a, b, tx: q0[0] - (a * p0[0] - b * p0[1]), ty: q0[1] - (b * p0[0] + a * p0[1]) };
        s.is_finite().then_some(s)
    }

    /// The least-squares similarity taking each `p` onto its `q`.
    pub fn least_squares(pairs: &[([f64; 2], [f64; 2])]) -> Option<Sim> {
        if pairs.len() < 2 {
            return None;
        }
        let n = pairs.len() as f64;
        let mf = pairs.iter().fold([0.0, 0.0], |m, (p, _)| [m[0] + p[0] / n, m[1] + p[1] / n]);
        let mt = pairs.iter().fold([0.0, 0.0], |m, (_, q)| [m[0] + q[0] / n, m[1] + q[1] / n]);
        let (mut sxx, mut sxy, mut spp) = (0.0, 0.0, 0.0);
        for (p, q) in pairs {
            let (px, py, qx, qy) = (p[0] - mf[0], p[1] - mf[1], q[0] - mt[0], q[1] - mt[1]);
            sxx += px * qx + py * qy;
            sxy += px * qy - py * qx;
            spp += px * px + py * py;
        }
        if !spp.is_finite() || spp <= 1e-18 {
            return None;
        }
        let (a, b) = (sxx / spp, sxy / spp);
        let s = Sim { a, b, tx: mt[0] - (a * mf[0] - b * mf[1]), ty: mt[1] - (b * mf[0] + a * mf[1]) };
        s.is_finite().then_some(s)
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A sheet put onto another by the tags both print.
#[derive(Clone, Debug, PartialEq)]
pub struct TagFit {
    pub sim: Sim,
    /// The tags that agree (one place each), sorted.
    pub matched: Vec<String>,
    /// Root-mean-square distance of the matched tags after the transform (the target's units).
    pub residual: f64,
}

/// The transform taking the tags `from` onto the same tags in `to`: of every transform two
/// shared tags give (scale inside `scale_range`), the one most tags agree with (within `tol`, in
/// `to`'s units), refined by least squares on those tags. None unless at least three distinct
/// tags agree. Tags only one side prints, and tags printed in several places, are fine: each tag
/// counts once, at its best place.
pub fn fit_by_tags(from: &[(String, [f64; 2])], to: &[(String, [f64; 2])], tol: f64, scale_range: (f64, f64)) -> Option<TagFit> {
    if !(tol.is_finite() && tol > 0.0) {
        return None;
    }
    let mut places: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for (t, p) in to {
        let v = places.entry(t.as_str()).or_default();
        if v.len() < MAX_PLACES_PER_TAG && p.iter().all(|c| c.is_finite()) {
            v.push(*p);
        }
    }
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (t, _) in from {
        *seen.entry(t.as_str()).or_default() += 1;
    }
    // Candidate pairs: tags printed once on both sides first (the surest), then the rest.
    let mut pairs: Vec<(&str, [f64; 2], [f64; 2], usize)> = Vec::new();
    for (t, p) in from {
        if !p.iter().all(|c| c.is_finite()) {
            continue;
        }
        let Some(qs) = places.get(t.as_str()) else { continue };
        let n = qs.len() * seen.get(t.as_str()).copied().unwrap_or(1);
        for q in qs {
            pairs.push((t.as_str(), *p, *q, n));
        }
    }
    pairs.sort_by_key(|x| x.3);
    pairs.truncate(MAX_FIT_PAIRS);
    let distinct: BTreeSet<&str> = pairs.iter().map(|x| x.0).collect();
    if distinct.len() < 3 {
        return None;
    }
    // The pairs each transform agrees with: per tag its nearest place, within `tol`.
    let agree = |s: &Sim| -> Vec<(&str, [f64; 2], [f64; 2], f64)> {
        let mut best: BTreeMap<&str, ([f64; 2], [f64; 2], f64)> = BTreeMap::new();
        for (t, p, q, _) in &pairs {
            let d = dist(s.apply(*p), *q);
            if d <= tol && best.get(t).is_none_or(|b| d < b.2) {
                best.insert(t, (*p, *q, d));
            }
        }
        best.into_iter().map(|(t, (p, q, d))| (t, p, q, d)).collect()
    };
    let in_range = |s: &Sim| {
        let k = s.scale();
        k >= scale_range.0 && k <= scale_range.1
    };
    let mut best: Option<(usize, f64, Sim)> = None;
    for (i, x) in pairs.iter().enumerate() {
        for y in pairs.iter().skip(i + 1) {
            if x.0 == y.0 || dist(x.1, y.1) < 2.0 * tol / scale_range.1.max(1e-9) {
                continue;
            }
            let Some(s) = Sim::from_two(x.1, y.1, x.2, y.2) else { continue };
            if !in_range(&s) {
                continue;
            }
            let ok = agree(&s);
            let err: f64 = ok.iter().map(|a| a.3 * a.3).sum();
            let better = match &best {
                None => true,
                Some((n, e, _)) => ok.len() > *n || (ok.len() == *n && err < *e),
            };
            if better {
                best = Some((ok.len(), err, s));
            }
        }
    }
    let (n, _, mut sim) = best?;
    if n < 3 {
        return None;
    }
    // Refine on the agreeing tags (twice: the refined transform may take in one more).
    let mut ok = agree(&sim);
    for _ in 0..2 {
        let lsq: Vec<([f64; 2], [f64; 2])> = ok.iter().map(|a| (a.1, a.2)).collect();
        let Some(s) = Sim::least_squares(&lsq).filter(in_range) else { break };
        let again = agree(&s);
        if again.len() < ok.len() {
            break;
        }
        sim = s;
        ok = again;
    }
    if ok.len() < 3 {
        return None;
    }
    let residual = (ok.iter().map(|a| dist(sim.apply(a.1), a.2).powi(2)).sum::<f64>() / ok.len() as f64).sqrt();
    let mut matched: Vec<String> = ok.iter().map(|a| a.0.to_string()).collect();
    matched.sort();
    Some(TagFit { sim, matched, residual })
}

// ─── Sheets onto their level ────────────────────────────────────────────────

/// A plan sheet as the registration sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetTags {
    pub number: String,
    pub level: String,
    /// `A`, `E`, `M`…
    pub discipline: String,
    /// Its door and room numbers ([`tag_key`] spelling) and where they are.
    pub tags: Vec<(String, [f64; 2])>,
    /// The sheet's rectangle `[x0, y0, x1, y1]` in the drawing.
    pub rect: [f64; 4],
}

/// Where a sheet sits in its level's project coordinates (the reference plan's).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SheetTransform {
    pub sheet: String,
    pub level: String,
    /// The level's reference plan (its architectural plan with the most tags).
    pub reference: String,
    pub registered: bool,
    /// `reference` (the reference plan itself), `tags` (shared door/room numbers) or `frame`
    /// (a sheet drawn alike: same size, and the numbers it shares where the frame puts them).
    pub method: String,
    /// Sheet to project coordinates.
    pub transform: Sim,
    /// The tags the fit used.
    pub matched: usize,
    /// Root-mean-square error of the matched tags, drawing units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub residual: Option<f64>,
}

impl SheetTransform {
    /// Where a point of the sheet lands in project coordinates (None when not registered).
    pub fn place(&self, p: [f64; 2]) -> Option<[f64; 2]> {
        self.registered.then(|| self.transform.apply(p))
    }
}

/// Every sheet put onto its level's reference plan: the architectural sheet with the most tags
/// is the reference (else the sheet with the most tags); every other sheet of the level is
/// fitted onto it by the tags both print (`tol`: how far one tag may sit from its copy, drawing
/// units), else, when it is drawn on the same size of sheet and the one or two numbers it shares
/// sit where its frame puts them, by its frame. A sheet with neither
/// is left unregistered (its elements are never merged with another sheet's).
pub fn register_sheets(sheets: &[SheetTags], tol: f64) -> Vec<SheetTransform> {
    let mut by_level: BTreeMap<&str, Vec<&SheetTags>> = BTreeMap::new();
    for s in sheets {
        by_level.entry(s.level.as_str()).or_default().push(s);
    }
    let mut out = Vec::new();
    for (level, list) in by_level {
        let distinct = |s: &SheetTags| s.tags.iter().map(|t| t.0.as_str()).collect::<BTreeSet<_>>().len();
        let Some(reference) =
            list.iter().copied().max_by_key(|s| (s.discipline.eq_ignore_ascii_case("A"), distinct(s), std::cmp::Reverse(s.number.clone())))
        else {
            continue;
        };
        for s in list {
            let mut t = SheetTransform {
                sheet: s.number.clone(),
                level: level.to_string(),
                reference: reference.number.clone(),
                registered: false,
                method: String::new(),
                transform: Sim::IDENTITY,
                matched: 0,
                residual: None,
            };
            if std::ptr::eq(s, reference) {
                t.registered = true;
                t.method = "reference".into();
                out.push(t);
                continue;
            }
            // The frame: the same size of sheet, lower-left corner onto lower-left corner.
            let (w0, h0) = (s.rect[2] - s.rect[0], s.rect[3] - s.rect[1]);
            let (w1, h1) = (reference.rect[2] - reference.rect[0], reference.rect[3] - reference.rect[1]);
            let alike = w0 > 0.0 && h0 > 0.0 && ((w0 / w1) - 1.0).abs() < 0.02 && ((h0 / h1) - 1.0).abs() < 0.02;
            let frame = Sim::scale_offset(1.0, [reference.rect[0] - s.rect[0], reference.rect[1] - s.rect[1]]);
            if let Some(f) = fit_by_tags(&s.tags, &reference.tags, tol, (0.25, 4.0)) {
                t.registered = true;
                t.method = "tags".into();
                t.transform = f.sim;
                t.matched = f.matched.len();
                t.residual = Some(f.residual);
            } else if alike && frame.is_finite() {
                // The tags it does print must agree with the frame (a plan shifted on its sheet
                // would put them elsewhere).
                let on_ref: BTreeMap<&str, [f64; 2]> = reference.tags.iter().map(|(k, p)| (k.as_str(), *p)).collect();
                let shared: Vec<f64> = s.tags.iter().filter_map(|(k, p)| on_ref.get(k.as_str()).map(|q| dist(frame.apply(*p), *q))).collect();
                let agree = shared.iter().filter(|d| **d <= tol).count();
                // At least one must agree: two halves of a floor split by a match line are
                // drawn on the same size of sheet too, and share no number.
                if agree >= 1 && agree * 2 > shared.len() {
                    t.registered = true;
                    t.method = "frame".into();
                    t.transform = frame;
                    t.matched = agree;
                    let ok: Vec<f64> = shared.iter().copied().filter(|d| *d <= tol).collect();
                    t.residual = (!ok.is_empty()).then(|| (ok.iter().map(|d| d * d).sum::<f64>() / ok.len() as f64).sqrt());
                }
            }
            out.push(t);
        }
    }
    out
}

// ─── One element per identity ───────────────────────────────────────────────

/// An element read off one sheet, as the merge sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdItem {
    /// `door`, `room`, `wall`, `device`, `equipment`, `fixture`…
    pub kind: String,
    pub level: String,
    /// Its identity key (a door's number, a device's type + circuit). Empty: none was read;
    /// such elements merge only with an untagged element of the same `kind` and `sort` at the
    /// same spot.
    pub key: String,
    /// What it is when it has no key (a device's type): untagged elements of different sorts at
    /// one spot (a switch and its sensor) stay two.
    pub sort: String,
    /// Where it is in project coordinates (None: its sheet is not registered, so it is compared
    /// with nothing).
    pub at: Option<[f64; 2]>,
    pub sheet: String,
    /// Its source's priority (lower wins: schedule/spec 0, architectural plan 1, discipline
    /// background 2, default 3). The kept element of a merge is the best one.
    pub rank: u8,
}

/// Elements that are one thing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct IdGroup {
    pub kind: String,
    pub level: String,
    pub key: String,
    /// The index (into the items) of the element kept.
    pub keep: usize,
    /// The others, merged into it.
    pub merged: Vec<usize>,
    /// Every sheet it was seen on, the kept one's first.
    pub sheets: Vec<String>,
}

/// The same key on one level at places too far apart to be one thing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct IdFinding {
    pub kind: String,
    pub level: String,
    pub key: String,
    /// The places (project coordinates) and their sheets.
    pub places: Vec<([f64; 2], String)>,
}

/// Group `items` into one element per real thing: same kind, level and key, within `tol(kind)`
/// in project coordinates (untagged: same kind and sort as well). A key at two places on one
/// level is a finding (two groups stay). Only groups with something merged are returned.
pub fn merge_identities(items: &[IdItem], tol: &dyn Fn(&str) -> f64) -> (Vec<IdGroup>, Vec<IdFinding>) {
    let mut by_key: BTreeMap<(&str, &str, &str, &str), Vec<usize>> = BTreeMap::new();
    for (i, it) in items.iter().enumerate().take(MAX_ITEMS) {
        if it.at.is_none() || it.kind.is_empty() {
            continue;
        }
        let sort = if it.key.is_empty() { it.sort.as_str() } else { "" };
        by_key.entry((it.kind.as_str(), it.level.as_str(), it.key.as_str(), sort)).or_default().push(i);
    }
    let mut groups = Vec::new();
    let mut findings = Vec::new();
    for ((kind, level, key, _), mut list) in by_key {
        let r = tol(kind);
        // The best source first; then sheet and drawing order.
        let order = |i: &usize| items.get(*i).map(|x| (x.rank, x.sheet.as_str()));
        list.sort_by(|a, b| order(a).cmp(&order(b)).then(a.cmp(b)));
        let mut clusters: Vec<(usize, [f64; 2], Vec<usize>)> = Vec::new();
        for i in list {
            let Some(at) = items.get(i).and_then(|x| x.at) else { continue };
            match clusters.iter_mut().find(|c| dist(c.1, at) <= r) {
                Some(c) => c.2.push(i),
                None => clusters.push((i, at, Vec::new())),
            }
        }
        if !key.is_empty() && clusters.len() > 1 {
            findings.push(IdFinding {
                kind: kind.into(),
                level: level.into(),
                key: key.into(),
                places: clusters.iter().map(|c| (c.1, items.get(c.0).map(|x| x.sheet.clone()).unwrap_or_default())).collect(),
            });
        }
        for (keep, _, merged) in clusters {
            if merged.is_empty() {
                continue;
            }
            let mut sheets: Vec<String> = Vec::new();
            for i in std::iter::once(keep).chain(merged.iter().copied()) {
                if let Some(s) = items.get(i).map(|x| x.sheet.clone())
                    && !sheets.contains(&s)
                {
                    sheets.push(s);
                }
            }
            groups.push(IdGroup { kind: kind.into(), level: level.into(), key: key.into(), keep, merged, sheets });
        }
    }
    (groups, findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_fit_then_another_is_both_in_turn() {
        let f = Sim { a: 0.0, b: 2.0, tx: 1.0, ty: -3.0 };
        let g = Sim::scale_offset(0.5, [10.0, 20.0]);
        let p = [3.0, 4.0];
        let (both, step) = (f.then(&g).apply(p), g.apply(f.apply(p)));
        assert!((both[0] - step[0]).abs() < 1e-12 && (both[1] - step[1]).abs() < 1e-12, "{both:?} vs {step:?}");
        let back = f.inverse().map(|i| i.apply(f.apply(p))).unwrap_or([f64::NAN; 2]);
        assert!((back[0] - p[0]).abs() < 1e-12 && (back[1] - p[1]).abs() < 1e-12, "{back:?}");
        assert!(Sim { a: 0.0, b: 0.0, tx: 1.0, ty: 1.0 }.inverse().is_none());
    }

    #[test]
    fn door_and_room_numbers_read_as_tags_and_other_marks_do_not() {
        for (s, k) in [("101", "101"), (" 101a ", "101A"), ("D-101", "D-101"), ("B204", "B204"), ("1.12", "1.12"), ("1201", "1201")] {
            assert_eq!(tag_key(s).as_deref(), Some(k), "{s}");
        }
        for s in ["12", "A", "LP-1", "1A2B", "12345", "ROOM", "-101", "101-", "1'-6\"", "ABC123", "1-0-1"] {
            assert_eq!(tag_key(s), None, "{s}");
        }
    }

    fn moved(tags: &[(&str, [f64; 2])], s: &Sim) -> Vec<(String, [f64; 2])> {
        tags.iter().map(|(t, p)| (t.to_string(), s.apply(*p))).collect()
    }

    #[test]
    fn an_offset_rotated_sheet_sharing_four_door_numbers_fits_within_an_inch() {
        // An enlarged plan at twice the scale, turned 30°, shifted; it shares 4 door numbers
        // with the floor plan and prints a fifth the floor plan doesn't, and the floor plan
        // prints a second "104" far away (a typo on the print).
        let plan = [("101", [100.0, 100.0]), ("102", [400.0, 120.0]), ("103", [420.0, 380.0]), ("104", [90.0, 360.0]), ("200", [2000.0, 50.0])];
        let truth = Sim { a: 2.0 * 30f64.to_radians().cos(), b: 2.0 * 30f64.to_radians().sin(), tx: -300.0, ty: 5000.0 };
        let inv = |p: [f64; 2]| {
            // Enlarged-sheet coordinates of a floor-plan point.
            let (x, y) = (p[0] - truth.tx, p[1] - truth.ty);
            let k = truth.a * truth.a + truth.b * truth.b;
            [(truth.a * x + truth.b * y) / k, (-truth.b * x + truth.a * y) / k]
        };
        let mut enlarged: Vec<(String, [f64; 2])> = plan.iter().take(4).map(|(t, p)| (t.to_string(), inv(*p))).collect();
        enlarged.push(("105".into(), [7.0, 7.0]));
        let mut on_plan: Vec<(String, [f64; 2])> = plan.iter().map(|(t, p)| (t.to_string(), *p)).collect();
        on_plan.push(("104".into(), [3000.0, 3000.0]));
        let f = fit_by_tags(&enlarged, &on_plan, 12.0, (0.1, 10.0)).expect("registered");
        assert_eq!(f.matched, vec!["101", "102", "103", "104"]);
        assert!(f.residual < 1.0, "residual {}", f.residual);
        assert!((f.sim.scale() - 2.0).abs() < 1e-6 && (f.sim.rotation_deg() - 30.0).abs() < 1e-6, "{:?}", f.sim);
        // Two shared tags are not enough.
        assert!(fit_by_tags(&enlarged[..2], &on_plan, 12.0, (0.1, 10.0)).is_none());
        // Positions an inch or two off still fit, and the residual says how far.
        let noisy: Vec<(String, [f64; 2])> =
            moved(&[("101", [100.0, 101.5]), ("102", [401.0, 120.0]), ("103", [420.0, 379.0]), ("104", [90.0, 360.0])], &Sim::IDENTITY);
        let f = fit_by_tags(&noisy, &on_plan, 12.0, (0.5, 2.0)).expect("noisy fit");
        assert!(f.residual > 0.1 && f.residual < 2.0, "{}", f.residual);
    }

    #[test]
    fn discipline_sheets_register_on_the_architectural_plan() {
        let a_tags =
            vec![("101".to_string(), [10.0, 10.0]), ("102".into(), [50.0, 10.0]), ("103".into(), [50.0, 40.0]), ("104".into(), [10.0, 40.0])];
        let shift = Sim::scale_offset(1.0, [1000.0, 0.0]);
        let back = Sim::scale_offset(1.0, [-1000.0, 0.0]);
        let sheets = vec![
            SheetTags { number: "A-101".into(), level: "1".into(), discipline: "A".into(), tags: a_tags.clone(), rect: [0.0, 0.0, 900.0, 600.0] },
            SheetTags {
                number: "E-101".into(),
                level: "1".into(),
                discipline: "E".into(),
                tags: a_tags.iter().map(|(t, p)| (t.clone(), shift.apply(*p))).collect(),
                rect: [1000.0, 0.0, 1900.0, 600.0],
            },
            // One shared number, same sheet size, the number where the frame puts it: by its
            // frame.
            SheetTags {
                number: "M-101".into(),
                level: "1".into(),
                discipline: "M".into(),
                tags: vec![("103".into(), [2050.0, 40.0])],
                rect: [2000.0, 0.0, 2900.0, 600.0],
            },
            // No number at all, same sheet size (another part of the floor, maybe): unregistered.
            SheetTags { number: "M-102".into(), level: "1".into(), discipline: "M".into(), tags: vec![], rect: [5000.0, 0.0, 5900.0, 600.0] },
            // No tags and another size: unregistered.
            SheetTags { number: "P-101".into(), level: "1".into(), discipline: "P".into(), tags: vec![], rect: [3000.0, 0.0, 3500.0, 300.0] },
        ];
        let t = register_sheets(&sheets, 6.0);
        let get = |n: &str| t.iter().find(|x| x.sheet == n).expect(n);
        assert_eq!(get("A-101").method, "reference");
        assert_eq!(get("E-101").method, "tags");
        assert_eq!(get("E-101").matched, 4);
        let p = get("E-101").place([1010.0, 10.0]).expect("placed");
        assert!(dist(p, back.apply([1010.0, 10.0])) < 1e-6, "{p:?}");
        assert_eq!(get("M-101").method, "frame");
        assert!(dist(get("M-101").place([2050.0, 40.0]).expect("frame"), [50.0, 40.0]) < 1e-9);
        assert!(!get("P-101").registered);
        assert!(!get("M-102").registered);
    }

    #[test]
    fn one_door_drawn_on_three_sheets_is_one_and_a_number_twice_apart_is_a_finding() {
        let door = |sheet: &str, rank: u8, at: [f64; 2], key: &str| IdItem {
            kind: "door".into(),
            level: "1".into(),
            key: key.into(),
            at: Some(at),
            sheet: sheet.into(),
            rank,
            ..IdItem::default()
        };
        let items = vec![
            door("E-101", 2, [100.0, 100.0], "101"),
            door("A-101", 1, [101.0, 99.0], "101"),
            door("M-101", 2, [99.0, 102.0], "101"),
            // The same number far away on the same level.
            door("A-101", 1, [600.0, 100.0], "101"),
            // Untagged copies at one spot merge too; two sorts stay two.
            IdItem {
                kind: "device".into(),
                level: "1".into(),
                sort: "S".into(),
                at: Some([5.0, 5.0]),
                sheet: "E-101".into(),
                rank: 1,
                ..IdItem::default()
            },
            IdItem {
                kind: "device".into(),
                level: "1".into(),
                sort: "S".into(),
                at: Some([5.5, 5.0]),
                sheet: "E-102".into(),
                rank: 2,
                ..IdItem::default()
            },
            IdItem {
                kind: "device".into(),
                level: "1".into(),
                sort: "OS".into(),
                at: Some([5.0, 5.0]),
                sheet: "E-101".into(),
                rank: 1,
                ..IdItem::default()
            },
            // Another level: never merged.
            IdItem { level: "2".into(), ..door("A-201", 1, [100.0, 100.0], "101") },
        ];
        let (groups, findings) = merge_identities(&items, &|k| if k == "door" { 6.0 } else { 12.0 });
        let doors: Vec<&IdGroup> = groups.iter().filter(|g| g.kind == "door").collect();
        assert_eq!(doors.len(), 1, "{groups:?}");
        assert_eq!(doors[0].keep, 1, "the architectural plan's copy is kept");
        assert_eq!(doors[0].merged, vec![0, 2]);
        assert_eq!(doors[0].sheets, vec!["A-101", "E-101", "M-101"]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].places.len(), 2);
        let devices: Vec<&IdGroup> = groups.iter().filter(|g| g.kind == "device").collect();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].merged, vec![5]);
    }
}
