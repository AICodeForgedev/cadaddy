//! Foreign pipe and duct out of electrical gear's dedicated space (NEC 110.26(E)(1), the rule as
//! data in `gearspace.json`).
//!
//! The space over a piece of electrical gear (its footprint, from the floor to 6 ft above it or
//! the storey's height) is the gear's own; no pipe or duct foreign to it may run there. Plans
//! draw them through it all the same (a vent laid on an enlarged plan over the wall a panel is
//! set in), and the field offsets them past the gear's side. So does the build, before the model
//! is made: each pipe or duct run whose stretch crosses a gear's footprint below the space's top
//! is run around it, the clearance off its side (an end that sits over the gear is moved past
//! it, and every run joined there moves with it). The run keeps its design route beside the
//! corrected one ([`cadaddy_walk3d::MepRun::corrections`], key `route`: the drawn path, what was installed, the
//! reason and the rule), so the built piece shows the difference and nothing changes silently.
//! A crossing the offset can't clear (a riser through the space, no side free of other gear) is
//! listed, still open.

use cadaddy_walk3d::{BuildingSpec, MepKind, RunCorrection};
use serde::Deserialize;
use serde_json::{Value, json};

/// The most runs, gear and crossings looked at (hostile specs).
const MAX_RUNS: usize = 200_000;
const MAX_GEAR: usize = 20_000;
const MAX_POINTS: usize = 10_000;
/// The most offsets laid in one run (each clears one crossing).
const MAX_ROUNDS: usize = 12;
const MAX_LISTED: usize = 50;
/// The most of a run's drawn points written into its design value.
const MAX_DESIGN_POINTS: usize = 40;

/// The rule as data (`gearspace.json` `dedicated_space`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    /// How far over the gear's top the space goes (feet).
    pub above_ft: f64,
    /// How far past the gear's side an offset pipe or duct runs (feet, to its edge).
    pub clear_ft: f64,
    pub rule: String,
    pub basis: String,
    /// Systems the rule lets run there (sprinkler protection of the space).
    pub exempt_systems: Vec<String>,
    pub exempt_basis: String,
}

#[derive(Default, Deserialize)]
struct File {
    #[serde(default)]
    dedicated_space: Rules,
}

/// The rules data.
pub(crate) fn rules() -> Rules {
    serde_json::from_str::<File>(include_str!("gearspace.json")).map(|f| f.dedicated_space).unwrap_or_default()
}

impl Rules {
    fn exempt(&self, system: &str) -> bool {
        let s = system.trim().to_ascii_uppercase();
        self.exempt_systems.iter().map(|w| w.trim().to_ascii_uppercase()).any(|w| !w.is_empty() && (s == w || (w.len() >= 5 && s.contains(&w))))
    }
}

/// A gear's dedicated space: its footprint (drawing units, a box turned as the gear is: centre,
/// its axes and half sizes) from the floor to `top_ft`.
#[derive(Clone, Debug)]
struct Space {
    level: Option<usize>,
    level_name: String,
    c: [f64; 2],
    u: [f64; 2],
    v: [f64; 2],
    hu: f64,
    hv: f64,
    top_ft: f64,
    what: String,
}

/// Where a gear stands in plan: its box's centre, its long axis (a unit vector) and half sizes
/// along and across it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Footprint {
    pub c: [f64; 2],
    pub u: [f64; 2],
    pub hu: f64,
    pub hv: f64,
}

impl Footprint {
    /// The spec's box (drawing units), square to the plan.
    #[cfg(test)]
    pub(crate) fn of_box(min: [f64; 2], max: [f64; 2]) -> Self {
        let (lo, hi) = ([min[0].min(max[0]), min[1].min(max[1])], [min[0].max(max[0]), min[1].max(max[1])]);
        Footprint { c: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], u: [1.0, 0.0], hu: (hi[0] - lo[0]) / 2.0, hv: (hi[1] - lo[1]) / 2.0 }
    }
}

impl Space {
    fn local(&self, p: [f64; 2]) -> [f64; 2] {
        let d = [p[0] - self.c[0], p[1] - self.c[1]];
        [d[0] * self.u[0] + d[1] * self.u[1], d[0] * self.v[0] + d[1] * self.v[1]]
    }
    /// Whether `p` is inside the footprint grown by `grow`.
    fn inside(&self, p: [f64; 2], grow: f64) -> bool {
        let l = self.local(p);
        l[0].abs() < self.hu + grow && l[1].abs() < self.hv + grow
    }
    /// Whether the segment a-b passes through the footprint grown by `grow`.
    fn crossed(&self, a: [f64; 2], b: [f64; 2], grow: f64) -> bool {
        let (h0, h1) = (self.hu + grow, self.hv + grow);
        crosses(self.local(a), self.local(b), [-h0, -h1], [h0, h1])
    }
    fn corners(&self) -> [[f64; 2]; 4] {
        let at = |su: f64, sv: f64| {
            [self.c[0] + self.u[0] * su * self.hu + self.v[0] * sv * self.hv, self.c[1] + self.u[1] * su * self.hu + self.v[1] * sv * self.hv]
        };
        [at(-1.0, -1.0), at(-1.0, 1.0), at(1.0, 1.0), at(1.0, -1.0)]
    }
}

/// What the pass did.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cleared {
    /// The gear whose space was checked.
    pub gear: usize,
    /// The runs found through a gear's space.
    pub found: usize,
    /// Of those, the runs run around it (their design route kept).
    pub corrected: usize,
    /// Other runs moved because an end joined to a moved one moved.
    pub joined: usize,
    /// Every run changed (corrected or joined): its index in the spec's runs and what its built
    /// pieces were known by before ([`super::mep::run_base`]).
    pub changed: Vec<(usize, String)>,
    /// Longer by (feet), all offsets.
    pub extra_ft: f64,
    /// What was moved: level, system, gear.
    pub moved: Vec<String>,
    /// The crossings left open, and why.
    pub left: Vec<String>,
}

impl Cleared {
    pub(crate) fn json(&self) -> Value {
        json!({
            "gear": self.gear, "found": self.found, "corrected": self.corrected, "joined_moved": self.joined,
            "extra_ft": (self.extra_ft * 10.0).round() / 10.0, "moved": self.moved, "left": self.left,
        })
    }
}

/// Whether the segment a-b passes through the open box (its inside, not along its edge).
fn crosses(a: [f64; 2], b: [f64; 2], min: [f64; 2], max: [f64; 2]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..2 {
        let (p, d, lo, hi) = (a[k], b[k] - a[k], min[k], max[k]);
        if d.abs() <= 1e-12 {
            if p <= lo || p >= hi {
                return false;
            }
            continue;
        }
        let (mut u0, mut u1) = ((lo - p) / d, (hi - p) / d);
        if u0 > u1 {
            std::mem::swap(&mut u0, &mut u1);
        }
        t0 = t0.max(u0);
        t1 = t1.min(u1);
        if t0 >= t1 {
            return false;
        }
    }
    t1 - t0 > 1e-9
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// The way round a space for the segment a-b (`half`: the run's half width; `clear`: from the
/// box to the run's centre line): along the segment to before the box, across past its nearer
/// side, along, and back onto the line after it. An end over the box moves across with the
/// offset. The path from a to b (its ends moved where they sat over the box), or None when
/// neither side clears every space.
fn detour(a: [f64; 2], b: [f64; 2], sp: &Space, all: &[&Space], bottom_ft: f64, half: f64, clear: f64) -> Option<Vec<[f64; 2]>> {
    let l = dist(a, b);
    if !(l.is_finite() && l > 1e-9 && clear.is_finite()) {
        return None;
    }
    let d = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
    let n = [-d[1], d[0]];
    let corners = sp.corners();
    let along = |c: &[f64; 2]| (c[0] - a[0]) * d[0] + (c[1] - a[1]) * d[1];
    let across = |c: &[f64; 2]| (c[0] - a[0]) * n[0] + (c[1] - a[1]) * n[1];
    let (s0, s1) = corners.iter().map(along).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let (o0, o1) = corners.iter().map(across).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let (enter, leave) = ((s0 - clear).max(0.0), (s1 + clear).min(l));
    let at = |s: f64, o: f64| [a[0] + d[0] * s + n[0] * o, a[1] + d[1] * s + n[1] * o];
    let mut sides = [o1 + clear, o0 - clear];
    sides.sort_by(|x, y| x.abs().total_cmp(&y.abs()));
    let blocked = |p: [f64; 2], q: [f64; 2]| all.iter().any(|k| bottom_ft < k.top_ft && k.crossed(p, q, half));
    sides.into_iter().find_map(|o| {
        // An end within the box's reach along the line moves across with the offset.
        let start = if enter <= 1e-9 && sp.inside(a, half) { at(0.0, o) } else { a };
        let end = if leave >= l - 1e-9 && sp.inside(b, half) { at(l, o) } else { b };
        let mut path = vec![start, at(enter, 0.0), at(enter, o), at(leave, o), at(leave, 0.0), end];
        if start != a {
            path.remove(1);
            path.remove(1);
        }
        if end != b {
            path.pop();
            path.pop();
        }
        path.dedup_by(|x, y| dist(*x, *y) <= 1e-6);
        let over = |p: &[f64; 2]| all.iter().any(|k| bottom_ft < k.top_ft && k.inside(*p, half));
        (path.len() >= 2 && path.windows(2).all(|w| !blocked(w[0], w[1])) && !path.iter().any(over)).then_some(path)
    })
}

/// Where a run's drawn path goes, in the drawing's units (its design value).
fn path_text(pts: &[[f64; 2]]) -> String {
    let mut s: Vec<String> = pts.iter().take(MAX_DESIGN_POINTS).map(|p| format!("{:.2},{:.2}", p[0], p[1])).collect();
    if pts.len() > MAX_DESIGN_POINTS {
        s.push(format!("… {} points", pts.len()));
    }
    s.join(" → ")
}

fn length(pts: &[[f64; 2]]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Each pipe and duct run of `sp` through a gear's dedicated space run around it, the design
/// route kept on the run beside the corrected one (see the module doc); the gear where its spec
/// box is.
#[cfg(test)]
pub(crate) fn clear(sp: &mut BuildingSpec) -> Cleared {
    clear_with(sp, &rules(), &|_, e| Some(Footprint::of_box(e.min, e.max)))
}

/// [`clear`] with the gear where the build stands it (`d`'s built walls): a panel on its wall's
/// face ([`super::mep::equipment_plan`]).
pub(crate) fn clear_built(d: &cadaddy_doc::Drawing, sp: &mut BuildingSpec) -> Cleared {
    clear_built_of(d, sp, None)
}

/// [`clear_built`] for only the gear `tags` names (upper case; None: all of it), as a step that
/// lays out or moves gear after the build calls it.
pub(crate) fn clear_built_of(d: &cadaddy_doc::Drawing, sp: &mut BuildingSpec, tags: Option<&std::collections::BTreeSet<String>>) -> Cleared {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let has_gear = sp.mep.equipment.iter().any(super::validate_gear::is_gear);
    // The walls where the plans draw them, as the gear is built on them.
    let walls = if has_gear { super::propfill::wall_footprints_on_plans(d) } else { Vec::new() };
    let levels = sp.levels.clone();
    let level_of = |name: &str| sp.level_matching(name).and_then(|i| levels.get(i)).cloned();
    let places: Vec<Option<Footprint>> = sp
        .mep
        .equipment
        .iter()
        .map(|e| {
            if tags.is_some_and(|t| !t.contains(&e.tag.trim().to_ascii_uppercase())) {
                return None;
            }
            let l = level_of(&e.level)?;
            let z0 = (l.elevation_ft + e.bottom_ft) * upf;
            let at = super::mep::equipment_plan(e, &l.name, &walls, upf, z0, z0 + e.height_ft * upf);
            let (dx, dy) = (at.b.x - at.a.x, at.b.y - at.a.y);
            let len = dx.hypot(dy);
            let u = if len > 1e-9 { [dx / len, dy / len] } else { [1.0, 0.0] };
            Some(Footprint { c: [(at.a.x + at.b.x) / 2.0, (at.a.y + at.b.y) / 2.0], u, hu: len / 2.0, hv: at.depth.abs() / 2.0 })
        })
        .collect();
    clear_with(sp, &rules(), &|i, _| places.get(i).copied().flatten())
}

/// [`clear`] by the rules `ru`, each gear where `place` puts it (by its index in the spec's
/// equipment; None: left out).
pub(crate) fn clear_with(sp: &mut BuildingSpec, ru: &Rules, place: &dyn Fn(usize, &cadaddy_walk3d::MepEquipment) -> Option<Footprint>) -> Cleared {
    let mut out = Cleared::default();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    if !(ru.above_ft.is_finite() && ru.above_ft > 0.0) {
        return out;
    }
    let level_of = |sp: &BuildingSpec, name: &str| sp.level_matching(name);
    let spaces: Vec<Space> = sp
        .mep
        .equipment
        .iter()
        .enumerate()
        .filter(|(_, e)| super::validate_gear::is_gear(e))
        .filter(|(_, e)| e.min.iter().chain(&e.max).all(|v| v.is_finite()) && e.bottom_ft.is_finite() && e.height_ft.is_finite())
        .take(MAX_GEAR)
        .filter_map(|(i, e)| {
            let f = place(i, e)?;
            let top = e.bottom_ft + e.height_ft.max(0.0) + ru.above_ft;
            let storey = sp.level(&e.level).map(|l| l.height_ft).filter(|h| h.is_finite() && *h > 0.0);
            let kind = e.kind.replace('_', " ");
            let ok = f.c.iter().chain(&f.u).chain([&f.hu, &f.hv]).all(|v| v.is_finite()) && f.hu > 1e-9 && f.hv > 1e-9;
            ok.then(|| Space {
                level: level_of(sp, &e.level),
                level_name: e.level.clone(),
                c: f.c,
                u: f.u,
                v: [-f.u[1], f.u[0]],
                hu: f.hu,
                hv: f.hv,
                top_ft: storey.map_or(top, |h| h.min(top)),
                what: if e.tag.trim().is_empty() { kind } else { format!("{kind} {}", e.tag.trim()) },
            })
        })
        .collect();
    out.gear = spaces.len();
    if spaces.is_empty() {
        return out;
    }
    let kind_of = |sp: &BuildingSpec, system: &str| sp.mep.systems.iter().find(|y| y.name.eq_ignore_ascii_case(system)).map(|y| y.kind);
    let same_level = |s: &Space, lvl: Option<usize>, name: &str| match (s.level, lvl) {
        (Some(a), Some(b)) => a == b,
        _ => s.level_name.eq_ignore_ascii_case(name),
    };
    let tol = (0.05 * upf).max(1e-6);
    let n_runs = sp.mep.runs.len().min(MAX_RUNS);
    for i in 0..n_runs {
        let Some(r) = sp.mep.runs.get(i) else { break };
        if !matches!(kind_of(sp, &r.system), Some(MepKind::Pipe | MepKind::Duct)) || ru.exempt(&r.system) || r.points.len() > MAX_POINTS {
            continue;
        }
        let lvl = level_of(sp, &r.level);
        let here: Vec<&Space> = spaces.iter().filter(|s| same_level(s, lvl, &r.level)).collect();
        if here.is_empty() {
            continue;
        }
        // The run's depth (feet) and its half width (drawing units).
        let depth_ft = if r.size_in[1] > 0.0 { r.size_in[1] } else { r.size_in[0] }.max(0.0) / 12.0;
        let half = r.size_in[0].max(0.0) / 24.0 * upf;
        let clear = ru.clear_ft.max(0.0) * upf + half;
        let in_space = |bottom: f64, s: &Space| bottom < s.top_ft && bottom + depth_ft > 0.0;
        let over = |s: &Space, a: [f64; 2], b: [f64; 2]| s.crossed(a, b, half) || s.inside(a, half) || s.inside(b, half);
        // A riser standing in the space: its plan point can't be offset here.
        if let (Some(top), [p]) = (r.top_ft, r.points.as_slice()) {
            let bottom = r.bottoms_ft.first().copied().unwrap_or(0.0);
            if let Some(s) = here.iter().find(|s| s.inside(*p, half) && bottom < s.top_ft && top > 0.0) {
                out.found += 1;
                if out.left.len() < MAX_LISTED {
                    out.left.push(format!("{} {} riser on {} stands in {}'s dedicated space", r.system, size_words(r.size_in), r.level, s.what));
                }
            }
            continue;
        }
        if r.top_ft.is_some() || r.points.len() < 2 {
            continue;
        }
        let drawn = r.points.clone();
        let mut pts = r.points.clone();
        let mut bottoms = r.bottoms_ft.clone();
        let mut notes: Vec<String> = Vec::new();
        let mut reasons: Vec<String> = Vec::new();
        let mut moved_ends: Vec<([f64; 2], [f64; 2])> = Vec::new();
        let mut open: Option<String> = None;
        let mut hit = false;
        for _ in 0..MAX_ROUNDS {
            // The first stretch through a space.
            let found = pts.windows(2).enumerate().find_map(|(k, w)| {
                let bottom = bottoms.get(k).or(bottoms.last()).copied().unwrap_or(0.0);
                let (a, b) = (*w.first()?, *w.get(1)?);
                here.iter().find(|s| in_space(bottom, s) && over(s, a, b)).map(|s| (k, a, b, bottom, *s))
            });
            let Some((k, a, b, bottom, s)) = found else { break };
            hit = true;
            let Some(path) = detour(a, b, s, &here, bottom, half, clear) else {
                open = Some(format!(
                    "{} {} on {} through {}'s dedicated space at {:.2} ft AFF: no side of it clear of other gear",
                    r.system,
                    size_words(r.size_in),
                    r.level,
                    s.what,
                    bottom
                ));
                break;
            };
            let first = k == 0;
            let last = k + 2 == pts.len();
            // A moved end is a joint: what meets the run there moves with it.
            if first
                && let Some(p0) = path.first()
                && dist(*p0, a) > 1e-6
            {
                moved_ends.push((a, *p0));
            }
            if last
                && let Some(p1) = path.last()
                && dist(*p1, b) > 1e-6
            {
                moved_ends.push((b, *p1));
            }
            let added = length(&path) - dist(a, b);
            let off = path.iter().map(|p| point_to_line(*p, a, b)).fold(0.0f64, f64::max);
            notes.push(format!("offset {:.0} in past {}'s side ({:.1} ft longer)", off / upf * 12.0, s.what, added.max(0.0) / upf));
            reasons.push(format!(
                "the {} {} at {:.2} ft AFF runs over {}'s footprint below {:.2} ft (6 ft over its top or the storey's height)",
                r.system,
                size_words(r.size_in),
                bottom,
                s.what,
                s.top_ft
            ));
            out.extra_ft += added.max(0.0) / upf;
            let n_new = path.len().saturating_sub(1);
            // The stretch k..k+1 becomes the detour's pieces, each at the stretch's bottom.
            let mut next: Vec<[f64; 2]> = pts.iter().take(k).copied().collect();
            next.extend(path.iter().copied());
            next.extend(pts.iter().skip(k + 2).copied());
            let mut nb: Vec<f64> = (0..k).map(|j| bottoms.get(j).or(bottoms.last()).copied().unwrap_or(bottom)).collect();
            nb.extend(std::iter::repeat_n(bottom, n_new));
            nb.extend((k + 1..pts.len().saturating_sub(1)).map(|j| bottoms.get(j).or(bottoms.last()).copied().unwrap_or(bottom)));
            pts = next;
            bottoms = nb;
            if pts.len() > MAX_POINTS {
                break;
            }
        }
        if !hit {
            continue;
        }
        out.found += 1;
        if let Some(why) = open {
            if out.left.len() < MAX_LISTED {
                out.left.push(why);
            }
            continue;
        }
        if notes.is_empty() {
            continue;
        }
        let what = format!("{} {} on {}", r.system, size_words(r.size_in), r.level);
        let correction = RunCorrection {
            key: "route".into(),
            design: format!("as drawn, through the gear's dedicated space: {}", path_text(&drawn)),
            corrected: notes.join("; "),
            reason: format!("{}; {}", reasons.join("; "), ru.basis),
            rule: ru.rule.clone(),
        };
        let (system, level) = (r.system.clone(), r.level.clone());
        out.changed.push((i, super::mep::run_base(r)));
        if let Some(r) = sp.mep.runs.get_mut(i) {
            r.points = pts;
            r.bottoms_ft = bottoms;
            // Offset again (gear moved after the build): the drawn route stays the design value,
            // the installed one says each offset.
            match r.corrections.iter_mut().find(|c| c.key == "route") {
                Some(c) => {
                    c.corrected = format!("{}; then {}", c.corrected, correction.corrected);
                    c.reason = format!("{}; then {}", c.reason, correction.reason);
                }
                None => r.corrections.push(correction),
            }
        }
        out.corrected += 1;
        if out.moved.len() < MAX_LISTED {
            out.moved.push(format!("{what}: {}", notes.join("; ")));
        }
        // The runs joined at a moved end: their point there moves too (their own route kept).
        for (from, to) in moved_ends {
            for (j, o) in sp.mep.runs.iter_mut().enumerate().take(MAX_RUNS) {
                if j == i || !o.system.eq_ignore_ascii_case(&system) || !o.level.eq_ignore_ascii_case(&level) {
                    continue;
                }
                let before = o.points.clone();
                let base = super::mep::run_base(o);
                let mut changed = false;
                for p in o.points.iter_mut().take(MAX_POINTS) {
                    if dist(*p, from) <= tol {
                        *p = to;
                        changed = true;
                    }
                }
                if changed {
                    out.joined += 1;
                    out.changed.push((j, base));
                    if !o.corrections.iter().any(|c| c.key == "route") {
                        o.corrections.push(RunCorrection {
                            key: "route".into(),
                            design: format!("as drawn: {}", path_text(&before)),
                            corrected: format!("its end at {:.2},{:.2} moved to {:.2},{:.2} with the run it joins", from[0], from[1], to[0], to[1]),
                            reason: format!("the {system} it joins there is offset out of electrical gear's dedicated space; {}", ru.basis),
                            rule: ru.rule.clone(),
                        });
                    }
                }
            }
        }
    }
    out
}

fn point_to_line(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let l = dist(a, b);
    if l <= 1e-12 {
        return dist(p, a);
    }
    ((b[0] - a[0]) * (a[1] - p[1]) - (a[0] - p[0]) * (b[1] - a[1])).abs() / l
}

fn size_words(s: [f64; 2]) -> String {
    let n = |v: f64| if (v - v.round()).abs() < 1e-6 { format!("{v:.0}") } else { format!("{v}") };
    if s[1] > 0.0 { format!("{}x{} in", n(s[0]), n(s[1])) } else { format!("{} in", n(s[0])) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::MepEquipment;

    fn spec(runs: Vec<Value>, gear: Vec<MepEquipment>) -> BuildingSpec {
        let mut sp: BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [{"name": "Level 2", "elevation_ft": 0.0, "height_ft": 12.0}],
            "mep": {
                "systems": [
                    {"name": "V", "layer": "P-VENT", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 9.0},
                    {"name": "EA", "layer": "M-EXH", "kind": "duct", "size_in": [8, 8], "bottom_ft": 9.0},
                    {"name": "SPK", "layer": "F-SPK", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9.0},
                    {"name": "BR", "layer": "E-BR", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9.0}
                ],
                "runs": runs
            }
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        sp.mep.equipment = gear;
        sp
    }

    fn panel(tag: &str, min: [f64; 2], max: [f64; 2], bottom_ft: f64, height_ft: f64) -> MepEquipment {
        MepEquipment { tag: tag.into(), kind: "panel".into(), level: "Level 2".into(), min, max, bottom_ft, height_ft, ..Default::default() }
    }

    fn run(system: &str, pts: &[[f64; 2]], bottom: f64, size: f64) -> Value {
        json!({"system": system, "level": "Level 2", "points": pts, "size_in": [size, 0], "bottoms_ft": vec![bottom; pts.len().saturating_sub(1)]})
    }

    fn through(sp: &BuildingSpec, i: usize, g: &MepEquipment) -> bool {
        let r = &sp.mep.runs[i];
        r.points.windows(2).any(|w| crosses(w[0], w[1], g.min, g.max))
    }

    #[test]
    fn a_drawn_vent_over_a_panel_is_offset_past_its_side_and_keeps_its_design_route() {
        // A 30 in wide panel, 4.5 to 6.5 ft: its space goes to 12.5 ft, capped at the storey's 12.
        let g = panel("PX", [120.0, 100.0], [150.0, 104.0], 4.5, 2.0);
        let mut sp = spec(vec![run("V", &[[60.0, 102.0], [240.0, 102.0]], 8.4, 3.0)], vec![g.clone()]);
        assert!(through(&sp, 0, &g), "the drawn vent crosses the panel's footprint");
        let c = clear(&mut sp);
        assert_eq!((c.gear, c.found, c.corrected), (1, 1, 1), "{c:?}");
        let r = &sp.mep.runs[0];
        assert!(!through(&sp, 0, &g), "{:?}", r.points);
        // Its ends stay where they were drawn; it runs past the panel's nearer side, clear by
        // 6 in to the pipe's edge (1.5 in half width).
        assert_eq!(r.points.first(), Some(&[60.0, 102.0]));
        assert_eq!(r.points.last(), Some(&[240.0, 102.0]));
        assert!(r.points.iter().any(|p| (p[1] - 111.5).abs() < 1e-6 || (p[1] - 92.5).abs() < 1e-6), "{:?}", r.points);
        assert_eq!(r.bottoms_ft.len(), r.points.len() - 1);
        assert!(r.bottoms_ft.iter().all(|b| (*b - 8.4).abs() < 1e-9));
        let k = &r.corrections[0];
        assert_eq!(k.key, "route");
        assert!(k.design.contains("60.00,102.00 → 240.00,102.00"), "{}", k.design);
        assert!(k.corrected.contains("panel PX"), "{}", k.corrected);
        assert_eq!(k.rule, "NEC 110.26(E)(1)(a)");
        assert!(k.reason.contains("12.00 ft"), "{}", k.reason);
        // Run again: nothing more to do, the design value stays the drawn one.
        let again = clear(&mut sp);
        assert_eq!((again.found, again.corrected), (0, 0));
        assert_eq!(sp.mep.runs[0].corrections.len(), 1);
        // The gear moves onto the offset (as a step after the build may move it): offset again,
        // the drawn route still the design value, both offsets said.
        let y = sp.mep.runs[0].points.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
        sp.mep.equipment[0].min = [125.0, y - 2.0];
        sp.mep.equipment[0].max = [145.0, y + 2.0];
        let moved = clear(&mut sp);
        assert_eq!(moved.corrected, 1, "{moved:?}");
        let k = &sp.mep.runs[0].corrections;
        assert_eq!(k.len(), 1);
        assert!(k[0].design.contains("60.00,102.00 → 240.00,102.00") && k[0].corrected.contains("; then offset"), "{k:?}");
    }

    #[test]
    fn a_pipe_whose_body_reaches_over_the_gear_is_offset_one_clear_of_it_stays() {
        let g = panel("PB", [120.0, 100.0], [150.0, 104.0], 4.5, 2.0);
        // A 3 in pipe 1 in off the panel's edge: its centre line misses, its body is over it.
        let mut sp = spec(vec![run("V", &[[60.0, 105.0], [240.0, 105.0]], 8.4, 3.0), run("V", &[[60.0, 106.0], [240.0, 106.0]], 8.4, 3.0)], vec![g]);
        let c = clear(&mut sp);
        assert_eq!((c.found, c.corrected), (1, 1), "{c:?}");
        assert!(!sp.mep.runs[0].corrections.is_empty() && sp.mep.runs[0].points.len() > 2, "{:?}", sp.mep.runs[0]);
        assert!(sp.mep.runs[0].points.iter().all(|p| p[1] >= 105.0), "the nearer side: {:?}", sp.mep.runs[0].points);
        // 2 in off: its body clear (1-1/2 in half width): as drawn.
        assert!(sp.mep.runs[1].corrections.is_empty() && sp.mep.runs[1].points.len() == 2);
    }

    #[test]
    fn a_duct_is_offset_too_and_above_the_space_or_not_over_the_gear_stays() {
        let g = panel("PY", [0.0, 0.0], [24.0, 6.0], 3.0, 3.0);
        let mut sp = spec(
            vec![
                // Through the space (top 12 ft, the storey caps 3+3+6).
                run("EA", &[[-50.0, 3.0], [80.0, 3.0]], 9.0, 8.0),
                // Clear of its footprint.
                run("V", &[[-50.0, 40.0], [80.0, 40.0]], 9.0, 2.0),
                // A branch circuit's conduit serves gear: not foreign piping.
                run("BR", &[[-50.0, 3.0], [80.0, 3.0]], 9.0, 0.75),
                // Sprinkler protection of the space is permitted.
                run("SPK", &[[-50.0, 3.0], [80.0, 3.0]], 9.0, 1.0),
            ],
            vec![g.clone()],
        );
        let c = clear(&mut sp);
        assert_eq!((c.found, c.corrected), (1, 1), "{c:?}");
        assert!(!through(&sp, 0, &g));
        assert!(through(&sp, 2, &g) && through(&sp, 3, &g));
        assert!(sp.mep.runs[1].corrections.is_empty() && sp.mep.runs[1].points.len() == 2);
        // A lower panel: its space ends at 3 ft, under the duct at 9 ft.
        let low = panel("PZ", [0.0, 0.0], [24.0, 6.0], 0.5, 0.5);
        let mut sp = spec(vec![run("EA", &[[-50.0, 3.0], [80.0, 3.0]], 9.0, 8.0)], vec![low]);
        let mut ru = rules();
        ru.above_ft = 2.0;
        let c = clear_with(&mut sp, &ru, &|_, e| Some(Footprint::of_box(e.min, e.max)));
        assert_eq!((c.found, c.corrected), (0, 0), "{c:?}");
    }

    #[test]
    fn an_end_over_the_gear_moves_past_it_and_the_run_joined_there_moves_with_it() {
        let g = panel("PQ", [100.0, 100.0], [130.0, 104.0], 4.5, 2.0);
        let mut sp = spec(
            vec![
                // Ends over the panel.
                run("V", &[[40.0, 102.0], [115.0, 102.0]], 9.0, 2.0),
                // The arm joined at that end, running away from the panel.
                run("V", &[[115.0, 102.0], [115.0, 160.0]], 9.0, 2.0),
            ],
            vec![g.clone()],
        );
        let c = clear(&mut sp);
        assert_eq!((c.found, c.corrected, c.joined), (1, 1, 1), "{c:?}");
        let (a, b) = (&sp.mep.runs[0], &sp.mep.runs[1]);
        let end = *a.points.last().unwrap_or_else(|| panic!("points"));
        assert!(!g_inside(&g, end), "{:?}", a.points);
        // Still joined: the arm's end moved with it, its drawn route kept beside.
        assert_eq!(b.points.first(), Some(&end), "{:?} {:?}", a.points, b.points);
        assert!(b.corrections.iter().any(|k| k.key == "route" && k.design.contains("115.00,102.00")), "{:?}", b.corrections);
        assert!(!through(&sp, 0, &g) && !through(&sp, 1, &g), "{:?} {:?}", a.points, b.points);
    }

    fn g_inside(g: &MepEquipment, p: [f64; 2]) -> bool {
        p[0] > g.min[0] && p[0] < g.max[0] && p[1] > g.min[1] && p[1] < g.max[1]
    }

    #[test]
    fn gear_laid_out_after_the_build_clears_only_its_own_space_and_says_what_changed() {
        let gear = vec![panel("PA", [100.0, 100.0], [130.0, 104.0], 4.5, 2.0), panel("PN", [300.0, 100.0], [330.0, 104.0], 4.5, 2.0)];
        let mut sp = spec(vec![run("V", &[[40.0, 102.0], [200.0, 102.0]], 9.0, 2.0), run("V", &[[240.0, 102.0], [400.0, 102.0]], 9.0, 2.0)], gear);
        let base = super::super::mep::run_base(&sp.mep.runs[1]);
        let only: std::collections::BTreeSet<String> = ["PN".to_string()].into_iter().collect();
        let c = clear_built_of(&cadaddy_doc::Drawing::new_imperial(), &mut sp, Some(&only));
        assert_eq!((c.found, c.corrected), (1, 1), "{c:?}");
        assert_eq!(c.changed, vec![(1, base)]);
        assert!(sp.mep.runs[0].corrections.is_empty() && !sp.mep.runs[1].corrections.is_empty());
    }

    #[test]
    fn a_crossing_no_side_clears_is_left_open_and_listed() {
        // Two panels side by side with the run's way round each blocked by the other's space
        // and a third: the run stays as drawn, listed.
        let gear = vec![
            panel("P1", [100.0, 90.0], [130.0, 114.0], 4.0, 2.0),
            panel("P2", [100.0, 114.5], [130.0, 200.0], 4.0, 2.0),
            panel("P3", [100.0, 0.0], [130.0, 89.5], 4.0, 2.0),
        ];
        let mut sp = spec(vec![run("V", &[[40.0, 102.0], [200.0, 102.0]], 9.0, 2.0)], gear);
        let c = clear(&mut sp);
        assert_eq!((c.found, c.corrected), (1, 0), "{c:?}");
        assert_eq!(c.left.len(), 1, "{c:?}");
        assert!(sp.mep.runs[0].corrections.is_empty() && sp.mep.runs[0].points.len() == 2);
    }

    #[test]
    fn a_riser_standing_in_the_space_is_listed_open() {
        let g = panel("PR", [0.0, 0.0], [24.0, 6.0], 4.0, 2.0);
        let mut sp = spec(
            vec![json!({"system": "V", "level": "Level 2", "points": [[12.0, 3.0]], "size_in": [2, 0], "bottoms_ft": [0.0], "top_ft": 12.0})],
            vec![g],
        );
        let c = clear(&mut sp);
        assert_eq!((c.found, c.corrected, c.left.len()), (1, 0, 1), "{c:?}");
    }
}
