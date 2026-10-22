//! Quantities from the MEP model: the takeoff of the spec's duct, pipe and conduit runs,
//! terminals and scheduled equipment (what `model.build` makes in 3D).
//!
//! - `mep.qty {level?, system?, group_by?, file?, table?}`: duct surface area and sheet-metal
//!   weight by gauge, pipe and conduit length by system and size, fittings counted by type and
//!   size (elbows, drops, tees, crosses, transitions and reducers), terminals, the equipment
//!   list and a hanger estimate, grouped by level and system. `file` exports the takeoff as CSV
//!   or XLSX; `table` places it in the drawing as a table that `model.build` refreshes, so it
//!   follows every edit of the model.
//! - `mep.qty.refresh`: rebuilds the linked takeoff tables now.
//!
//! The gauge table is ours (sizes in the manner of the published sheet-metal duct standards,
//! weights of galvanised steel per square foot); `gauges` replaces it. Hanger spacing is a rule
//! per kind and size (`hanger_spacing_ft` overrides it); the rod reaches the next floor.

use cadaddy_doc::{Common, Drawing, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, MepKind, MepRun, MepSystem};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// The xdata application of a linked takeoff table.
pub const APP: &str = "CADADDY_MEPQTY";
/// Runs, terminals and equipment counted, at most (hostile drawings).
const MAX_ITEMS: usize = 20_000;
/// Points per run, at most.
const MAX_POINTS: usize = 10_000;
/// Rows in a takeoff table, at most (the table's own limit).
const MAX_TABLE_ROWS: usize = 1_900;
/// Linked tables refreshed per build, at most.
const MAX_TABLES: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mep.qty", "MEP Quantities", run_qty).params(
            "{level?: name, system?: name, group_by?: \"level_system\" (default) | \"system\" | \"level\" | \"none\", gauges?: [{max_in, gauge, lb_per_sqft}] (rectangular by the longer side, round by the diameter), round_gauges?: [...], allowance_pct?: percent added to duct area and weight for seams (0), hanger_spacing_ft?: {duct?, round_duct?, pipe?, conduit?}, file?: path (.csv/.xlsx), format?: csv|xlsx, table?: {at: [x,y], part?: all|ducts|pipes|fittings|terminals|equipment|hangers|lug_kits} (a linked table model.build refreshes)} → {ducts: [{level, system, shape, size, gauge, length_ft, area_sqft, weight_lb, runs}], pipes: [{level, system, kind, size, length_ft, runs}], fittings: [{level, system, type, size, count}], terminals: [{level, system, kind, size, count}], equipment: [{tag, kind, level, systems, size, title}], hangers: [{level, system, kind, size, spacing_ft, count, rod_ft}], lug_kits: [{gear, point, lugs, lug, bolts, bolt, nuts, flat_washers, belleville_washers}] (lugs ordered separately from their gear: required items), labor_conditions: [{gear, conditions, factor}], totals, rows, csv, path?, table?}",
        ),
        CommandSpec::new("mep.qty.refresh", "Refresh MEP Quantities", run_refresh).params("{} → {tables}: rebuild every linked takeoff table from the model now"),
    ]
}

// ---------------- settings ----------------

/// A row of the gauge table: up to `max_in` (the longer side, or the diameter) is `gauge`, at
/// `lb_per_sqft` of galvanised sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gauge {
    pub max_in: f64,
    pub gauge: u32,
    pub lb_per_sqft: f64,
}

/// Rectangular duct, by the longer side.
const RECT_GAUGES: [Gauge; 5] = [
    Gauge { max_in: 12.0, gauge: 26, lb_per_sqft: 0.906 },
    Gauge { max_in: 30.0, gauge: 24, lb_per_sqft: 1.156 },
    Gauge { max_in: 54.0, gauge: 22, lb_per_sqft: 1.406 },
    Gauge { max_in: 84.0, gauge: 20, lb_per_sqft: 1.656 },
    Gauge { max_in: f64::INFINITY, gauge: 18, lb_per_sqft: 2.156 },
];
/// Round duct, by the diameter.
const ROUND_GAUGES: [Gauge; 5] = [
    Gauge { max_in: 12.0, gauge: 26, lb_per_sqft: 0.906 },
    Gauge { max_in: 18.0, gauge: 24, lb_per_sqft: 1.156 },
    Gauge { max_in: 30.0, gauge: 22, lb_per_sqft: 1.406 },
    Gauge { max_in: 48.0, gauge: 20, lb_per_sqft: 1.656 },
    Gauge { max_in: f64::INFINITY, gauge: 18, lb_per_sqft: 2.156 },
];

/// How the takeoff is counted and grouped.
#[derive(Clone, Debug)]
struct Settings {
    level: Option<String>,
    system: Option<String>,
    by_level: bool,
    by_system: bool,
    rect: Vec<Gauge>,
    round: Vec<Gauge>,
    allowance: f64,
    duct_ft: f64,
    round_duct_ft: f64,
    pipe_ft: Option<f64>,
    conduit_ft: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            level: None,
            system: None,
            by_level: true,
            by_system: true,
            rect: RECT_GAUGES.to_vec(),
            round: ROUND_GAUGES.to_vec(),
            allowance: 0.0,
            duct_ft: 8.0,
            round_duct_ft: 10.0,
            pipe_ft: None,
            conduit_ft: 10.0,
        }
    }
}

fn gauges_param(cmd: &str, v: Option<&Value>, default: &[Gauge]) -> Result<Vec<Gauge>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(default.to_vec()) };
    let a = v.as_array().ok_or_else(|| bad(cmd, "gauge tables are arrays of {max_in, gauge, lb_per_sqft}"))?;
    let mut out = Vec::new();
    for g in a.iter().take(64) {
        let max_in = match g.get("max_in") {
            None | Some(Value::Null) => f64::INFINITY,
            Some(x) => x.as_f64().filter(|x| *x > 0.0).ok_or_else(|| bad(cmd, "`max_in` must be a positive number of inches"))?,
        };
        let gauge =
            g.get("gauge").and_then(Value::as_u64).filter(|n| (1..=40).contains(n)).ok_or_else(|| bad(cmd, "`gauge` must be from 1 to 40"))?;
        let lb = g
            .get("lb_per_sqft")
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && *x > 0.0 && *x < 100.0)
            .ok_or_else(|| bad(cmd, "`lb_per_sqft` must be a positive weight"))?;
        out.push(Gauge { max_in, gauge: gauge as u32, lb_per_sqft: lb });
    }
    if out.is_empty() {
        return Err(bad(cmd, "a gauge table needs at least one row"));
    }
    out.sort_by(|a, b| a.max_in.total_cmp(&b.max_in));
    Ok(out)
}

fn settings(cmd: &str, p: &Value) -> Result<Settings> {
    let mut st =
        Settings { level: str_param(p, "level").map(str::to_string), system: str_param(p, "system").map(str::to_string), ..Settings::default() };
    match str_param(p, "group_by").unwrap_or("level_system") {
        "level_system" => {}
        "system" => st.by_level = false,
        "level" => st.by_system = false,
        "none" => (st.by_level, st.by_system) = (false, false),
        g => return Err(bad(cmd, format!("`group_by` is level_system, system, level or none (not {g})"))),
    }
    st.rect = gauges_param(cmd, p.get("gauges"), &RECT_GAUGES)?;
    st.round = gauges_param(cmd, p.get("round_gauges"), &ROUND_GAUGES)?;
    if let Some(a) = p.get("allowance_pct").filter(|v| !v.is_null()) {
        st.allowance = a.as_f64().filter(|x| (0.0..=100.0).contains(x)).ok_or_else(|| bad(cmd, "`allowance_pct` must be from 0 to 100"))? / 100.0;
    }
    if let Some(h) = p.get("hanger_spacing_ft").filter(|v| !v.is_null()) {
        let get = |k: &str| -> Result<Option<f64>> {
            match h.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_f64()
                    .filter(|x| x.is_finite() && *x >= 1.0 && *x <= 100.0)
                    .map(Some)
                    .ok_or_else(|| bad(cmd, format!("hanger spacing `{k}` must be from 1 to 100 feet"))),
            }
        };
        st.duct_ft = get("duct")?.unwrap_or(st.duct_ft);
        st.round_duct_ft = get("round_duct")?.unwrap_or(st.round_duct_ft);
        st.pipe_ft = get("pipe")?;
        st.conduit_ft = get("conduit")?.unwrap_or(st.conduit_ft);
    }
    Ok(st)
}

impl Settings {
    fn gauge(&self, size: [f64; 2]) -> Option<Gauge> {
        let (table, key) = if size[1] > 0.0 { (&self.rect, size[0].max(size[1])) } else { (&self.round, size[0]) };
        table.iter().find(|g| key <= g.max_in).or(table.last()).copied()
    }

    /// Hanger spacing, feet: ducts by shape; pipe by size (up to 1": 6', 2": 8', 4": 10', larger
    /// 12'); conduit and cable tray.
    fn spacing(&self, kind: MepKind, size: [f64; 2]) -> f64 {
        match kind {
            MepKind::Duct if size[1] > 0.0 => self.duct_ft,
            MepKind::Duct => self.round_duct_ft,
            MepKind::Conduit | MepKind::Tray | MepKind::Cable => self.conduit_ft,
            MepKind::Pipe => self.pipe_ft.unwrap_or(match size[0] {
                d if d <= 1.0 => 6.0,
                d if d <= 2.0 => 8.0,
                d if d <= 4.0 => 10.0,
                _ => 12.0,
            }),
        }
    }

    fn wants(&self, level: &str, system: &str) -> bool {
        self.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(level)) && self.system.as_deref().is_none_or(|s| s.eq_ignore_ascii_case(system))
    }

    fn key_level(&self, level: &str) -> String {
        if self.by_level { level.to_string() } else { "All".into() }
    }

    fn key_system(&self, system: &str) -> String {
        if self.by_system { system.to_string() } else { "All".into() }
    }
}

// ---------------- sizes ----------------

/// Inches as a fraction to the nearest sixteenth: `3/4"`, `1-1/2"`, `10"`.
fn inches(v: f64) -> String {
    if !v.is_finite() || v <= 0.0 {
        return "?".into();
    }
    let n = (v * 16.0).round() as i64;
    let (whole, mut num, mut den) = (n / 16, n % 16, 16);
    while num > 0 && num % 2 == 0 {
        num /= 2;
        den /= 2;
    }
    match (whole, num) {
        (w, 0) => format!("{w}\""),
        (0, f) => format!("{f}/{den}\""),
        (w, f) => format!("{w}-{f}/{den}\""),
    }
}

/// A section's size: `24x12` rectangular, `10"Ø` round duct, `2"` pipe or conduit.
fn size_label(kind: MepKind, size: [f64; 2]) -> String {
    let n = |v: f64| if (v - v.round()).abs() < 1e-6 { format!("{}", v.round() as i64) } else { format!("{:.1}", v) };
    match kind {
        MepKind::Duct if size[1] > 0.0 => format!("{}x{}", n(size[0]), n(size[1])),
        MepKind::Duct => format!("{}Ø", inches(size[0])),
        MepKind::Tray => format!("{}x{}", n(size[0]), n(size[1])),
        _ => inches(size[0]),
    }
}

/// Feet and inches for a plan dimension: `4'-6"`.
fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{}'-{}\"", total / 12, total % 12)
}

fn kind_str(kind: MepKind) -> &'static str {
    match kind {
        MepKind::Duct => "duct",
        MepKind::Pipe => "pipe",
        MepKind::Conduit => "conduit",
        MepKind::Tray => "tray",
        MepKind::Cable => "cable",
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ---------------- the takeoff ----------------

#[derive(Default)]
struct DuctRow {
    shape: &'static str,
    gauge: u32,
    length_ft: f64,
    area_sqft: f64,
    weight_lb: f64,
    runs: Vec<usize>,
}

#[derive(Default)]
struct LenRow {
    length_ft: f64,
    runs: Vec<usize>,
}

#[derive(Default)]
struct HangerRow {
    spacing_ft: f64,
    count: usize,
    rod_ft: f64,
}

/// Group key: level, system, then what is counted (size, type…).
type Key = (String, String, String);

/// The takeoff, grouped.
#[derive(Default)]
pub struct Takeoff {
    ducts: BTreeMap<Key, DuctRow>,
    pipes: BTreeMap<(String, String, &'static str, String), LenRow>,
    fittings: BTreeMap<(String, String, String, String), usize>,
    terminals: BTreeMap<(String, String, String, String), usize>,
    equipment: Vec<Value>,
    hangers: BTreeMap<(String, String, &'static str, String), HangerRow>,
    /// Lugs ordered separately from the gear they land on, with their bolt kits (required items).
    lug_kits: Vec<Value>,
    /// Gear accepted with review failures, and the factor on its installation.
    labor: Vec<Value>,
}

/// A run's corner: an elbow (a turn in plan) or a drop (a change of height), with the plan turn
/// in degrees. The same test `model.build` makes its fittings with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Corner {
    Elbow(f64),
    Drop(f64),
}

fn corners(pts: &[Vec2], bottoms: &[f64]) -> Vec<Corner> {
    let mut out = Vec::new();
    for (i, w) in pts.windows(3).enumerate() {
        let [a, b, c] = [w[0], w[1], w[2]];
        let (u, v) = (b - a, c - b);
        let turn = u.len() > 1e-9 && v.len() > 1e-9 && (u.cross(v) / (u.len() * v.len())).abs() > 0.09;
        let z = bottoms.get(i).copied().unwrap_or(0.0);
        let z2 = bottoms.get(i + 1).copied().unwrap_or(0.0);
        if (z2 - z).abs() > 1e-9 {
            out.push(Corner::Drop((z2 - z).abs()));
        } else if turn {
            let cos = (u.dot(v) / (u.len() * v.len())).clamp(-1.0, 1.0);
            out.push(Corner::Elbow(cos.acos().to_degrees()));
        }
    }
    out
}

/// An elbow's name by its turn: 90°, 45°, else the angle to the nearest 5°.
fn elbow_name(deg: f64) -> String {
    if (deg - 90.0).abs() <= 10.0 {
        "elbow 90°".into()
    } else if (deg - 45.0).abs() <= 10.0 {
        "elbow 45°".into()
    } else {
        format!("elbow {}°", ((deg / 5.0).round() * 5.0) as i64)
    }
}

/// Where runs meet: a tee where a run ends on another's side; at a shared end, a tee (three
/// runs), a cross (four or more), or a transition (rectangular) / reducer (round) between two
/// sizes.
fn joints(runs: &[(usize, &MepRun, &MepSystem)], upf: f64, st: &Settings, t: &mut Takeoff) {
    // Per system and level: the run ends.
    let mut groups: BTreeMap<(&str, &str), Vec<(usize, &MepRun, &MepSystem)>> = BTreeMap::new();
    for r in runs {
        groups.entry((r.1.system.as_str(), r.1.level.as_str())).or_default().push(*r);
    }
    for ((system, level), rs) in groups {
        let tol_of = |size: [f64; 2]| (size[0].max(size[1]) / 24.0 + 0.1) * upf;
        let key = |what: &str, size: String| (st.key_level(level), st.key_system(system), what.to_string(), size);
        // Ends: (point, run index in rs, size).
        let ends: Vec<(Vec2, usize)> = rs
            .iter()
            .enumerate()
            .flat_map(|(i, (_, r, _))| {
                let f = r.points.first().map(|p| Vec2::new(p[0], p[1]));
                let l = if r.points.len() > 1 { r.points.last().map(|p| Vec2::new(p[0], p[1])) } else { None };
                f.into_iter().chain(l).map(move |p| (p, i))
            })
            .collect();
        let mut used = vec![false; ends.len()];
        for (ei, (p, ri)) in ends.iter().enumerate() {
            let Some((_, run, sy)) = rs.get(*ri) else { continue };
            // On another run's side?
            let mut on_side = None;
            for (rj, (_, other, _)) in rs.iter().enumerate() {
                if rj == *ri {
                    continue;
                }
                let tol = tol_of(other.size_in);
                let pts: Vec<Vec2> = other.points.iter().take(MAX_POINTS).map(|q| Vec2::new(q[0], q[1])).collect();
                let near_end = pts.first().is_some_and(|q| q.dist(*p) <= tol) || pts.last().is_some_and(|q| q.dist(*p) <= tol);
                if !near_end && pts.windows(2).any(|w| seg_dist(*p, w[0], w[1]) <= tol) {
                    on_side = Some(other);
                    break;
                }
            }
            if let Some(main) = on_side {
                let size = format!("{} x {}", size_label(sy.kind, main.size_in), size_label(sy.kind, run.size_in));
                *t.fittings.entry(key("tee", size)).or_default() += 1;
                if let Some(u) = used.get_mut(ei) {
                    *u = true;
                }
                continue;
            }
            if used.get(ei).copied().unwrap_or(true) {
                continue;
            }
            // Shared ends.
            let tol = tol_of(run.size_in);
            let mut at: Vec<usize> = vec![ei];
            for (ej, (q, rj)) in ends.iter().enumerate().skip(ei + 1) {
                if rj != ri && !used.get(ej).copied().unwrap_or(true) && q.dist(*p) <= tol {
                    at.push(ej);
                }
            }
            if at.len() < 2 {
                continue;
            }
            for e in &at {
                if let Some(u) = used.get_mut(*e) {
                    *u = true;
                }
            }
            let mut sizes: Vec<[f64; 2]> =
                at.iter().filter_map(|e| ends.get(*e)).filter_map(|(_, r)| rs.get(*r)).map(|(_, r, _)| r.size_in).collect();
            sizes.sort_by(|a, b| (b[0] * b[1].max(1.0)).total_cmp(&(a[0] * a[1].max(1.0))));
            let label = |s: &[f64; 2]| size_label(sy.kind, *s);
            match sizes.as_slice() {
                [a, b] if (a[0] - b[0]).abs() > 1e-6 || (a[1] - b[1]).abs() > 1e-6 => {
                    let what = if sy.kind == MepKind::Duct && (a[1] > 0.0 || b[1] > 0.0) { "transition" } else { "reducer" };
                    *t.fittings.entry(key(what, format!("{} to {}", label(a), label(b)))).or_default() += 1;
                }
                [_, _] => {}
                [a, _, c] => {
                    // Sorted largest first: the main and the smallest branch.
                    *t.fittings.entry(key("tee", format!("{} x {}", label(a), label(c)))).or_default() += 1;
                }
                [a, ..] => {
                    *t.fittings.entry(key("cross", label(a))).or_default() += 1;
                }
                [] => {}
            }
        }
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 1e-18 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// The takeoff of the spec's MEP model.
fn takeoff(sp: &BuildingSpec, st: &Settings) -> Takeoff {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut t = Takeoff::default();
    let mut counted: Vec<(usize, &MepRun, &MepSystem)> = Vec::new();
    for (id, r) in sp.mep.runs.iter().enumerate().take(MAX_ITEMS) {
        let Some(sy) = sp.mep.systems.iter().find(|s| s.name == r.system) else { continue };
        if !st.wants(&r.level, &r.system) || !r.size_in.iter().all(|v| v.is_finite()) || r.size_in[0] <= 0.0 {
            continue;
        }
        let level = sp.level(&r.level);
        let pts: Vec<Vec2> = r.points.iter().take(MAX_POINTS).map(|p| Vec2::new(p[0], p[1])).filter(|p| p.is_finite()).collect();
        let bottoms: Vec<f64> = r.bottoms_ft.iter().map(|b| if b.is_finite() { *b } else { 0.0 }).collect();
        let plan_ft: Vec<f64> = pts.windows(2).map(|w| w[0].dist(w[1]) / upf).collect();
        let cs = corners(&pts, &bottoms);
        let rise_ft: f64 = cs.iter().map(|c| if let Corner::Drop(d) = c { *d } else { 0.0 }).sum();
        let length = plan_ft.iter().sum::<f64>() + rise_ft;
        let (lk, sk) = (st.key_level(&r.level), st.key_system(&r.system));
        let size = size_label(sy.kind, r.size_in);
        counted.push((id, r, sy));

        match sy.kind {
            MepKind::Duct => {
                let round = r.size_in[1] <= 0.0;
                let perim_ft = if round { std::f64::consts::PI * r.size_in[0] / 12.0 } else { 2.0 * (r.size_in[0] + r.size_in[1]) / 12.0 };
                let area = perim_ft * length * (1.0 + st.allowance);
                let g = st.gauge(r.size_in);
                let row = t.ducts.entry((lk.clone(), sk.clone(), size.clone())).or_default();
                row.shape = if round { "round" } else { "rectangular" };
                row.gauge = g.map_or(0, |g| g.gauge);
                row.length_ft += length;
                row.area_sqft += area;
                row.weight_lb += area * g.map_or(0.0, |g| g.lb_per_sqft);
                row.runs.push(id);
            }
            MepKind::Pipe | MepKind::Conduit | MepKind::Tray | MepKind::Cable => {
                let row = t.pipes.entry((lk.clone(), sk.clone(), kind_str(sy.kind), size.clone())).or_default();
                row.length_ft += length;
                row.runs.push(id);
            }
        }

        for c in &cs {
            let what = match c {
                Corner::Elbow(deg) => elbow_name(*deg),
                Corner::Drop(_) => if sy.kind == MepKind::Duct { "drop" } else { "offset" }.to_string(),
            };
            *t.fittings.entry((lk.clone(), sk.clone(), what, size.clone())).or_default() += 1;
        }

        // Hangers along each straight plan segment; the rod from the top of the run to the
        // floor above.
        let spacing = st.spacing(sy.kind, r.size_in);
        let section_ft = if r.size_in[1] > 0.0 { r.size_in[1] } else { r.size_in[0] } / 12.0;
        let height = level.map_or(0.0, |l| l.height_ft).max(0.0);
        let row = t.hangers.entry((lk, sk, kind_str(sy.kind), size)).or_default();
        row.spacing_ft = spacing;
        for (i, l) in plan_ft.iter().enumerate() {
            if *l <= 1e-9 {
                continue;
            }
            let n = (l / spacing).ceil().max(1.0) as usize;
            let top = bottoms.get(i).copied().unwrap_or(0.0) + section_ft;
            row.count += n;
            row.rod_ft += n as f64 * (height - top).max(0.0);
        }
    }
    joints(&counted, upf, st, &mut t);

    for term in sp.mep.terminals.iter().take(MAX_ITEMS) {
        if !st.wants(&term.level, &term.system) {
            continue;
        }
        let kind = sp.mep.systems.iter().find(|s| s.name == term.system).map_or(MepKind::Duct, |s| s.kind);
        let size = if term.size_in[1] > 0.0 { size_label(MepKind::Duct, term.size_in) } else { size_label(kind, term.size_in) };
        *t.terminals.entry((st.key_level(&term.level), st.key_system(&term.system), term.terminal_kind.clone(), size)).or_default() += 1;
    }

    for e in sp.mep.equipment.iter().take(MAX_ITEMS) {
        let level_ok = st.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&e.level));
        let system_ok = st.system.as_deref().is_none_or(|s| e.systems.iter().any(|x| x.eq_ignore_ascii_case(s)));
        if !level_ok || !system_ok {
            continue;
        }
        let w = (e.max[0] - e.min[0]).abs() / upf;
        let dp = (e.max[1] - e.min[1]).abs() / upf;
        let size = if e.round {
            format!("{}Ø x {} H", ft_in(w.min(dp)), ft_in(e.height_ft))
        } else {
            format!("{} x {} x {} H", ft_in(w), ft_in(dp), ft_in(e.height_ft))
        };
        t.equipment.push(json!({
            "tag": e.tag, "kind": e.kind, "level": e.level, "systems": e.systems, "size": size,
            "title": e.title, "properties": e.properties,
        }));
    }
    t.equipment.sort_by(|a, b| (a["level"].as_str(), a["tag"].as_str()).cmp(&(b["level"].as_str(), b["tag"].as_str())));
    let gear_level = |tag: &str| sp.mep.equipment.iter().find(|e| e.tag == tag).map(|e| e.level.clone()).unwrap_or_default();
    let on_level = |tag: &str| st.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&gear_level(tag)));
    if st.system.is_none() {
        t.lug_kits = super::gear::lug_kits(sp).into_iter().filter(|k| on_level(k["gear"].as_str().unwrap_or_default())).collect();
        t.labor = super::gear::labor(sp).into_iter().filter(|k| on_level(k["gear"].as_str().unwrap_or_default())).collect();
    }
    t
}

impl Takeoff {
    fn to_json(&self, st: &Settings) -> Value {
        let ducts: Vec<Value> = self
            .ducts
            .iter()
            .map(|((l, s, size), r)| {
                json!({"level": l, "system": s, "shape": r.shape, "size": size, "gauge": r.gauge, "length_ft": r2(r.length_ft),
                    "area_sqft": r2(r.area_sqft), "weight_lb": r2(r.weight_lb), "runs": r.runs})
            })
            .collect();
        let pipes: Vec<Value> = self
            .pipes
            .iter()
            .map(|((l, s, k, size), r)| json!({"level": l, "system": s, "kind": k, "size": size, "length_ft": r2(r.length_ft), "runs": r.runs}))
            .collect();
        let fittings: Vec<Value> =
            self.fittings.iter().map(|((l, s, ty, size), n)| json!({"level": l, "system": s, "type": ty, "size": size, "count": n})).collect();
        let terminals: Vec<Value> =
            self.terminals.iter().map(|((l, s, k, size), n)| json!({"level": l, "system": s, "kind": k, "size": size, "count": n})).collect();
        let hangers: Vec<Value> = self
            .hangers
            .iter()
            .filter(|(_, r)| r.count > 0)
            .map(|((l, s, k, size), r)| json!({"level": l, "system": s, "kind": k, "size": size, "spacing_ft": r.spacing_ft, "count": r.count, "rod_ft": r2(r.rod_ft)}))
            .collect();
        let sum_len = |k: &str| r2(self.pipes.iter().filter(|((_, _, x, _), _)| *x == k).map(|(_, r)| r.length_ft).sum());
        let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
        for e in &self.equipment {
            *by_kind.entry(e["kind"].as_str().unwrap_or("").to_string()).or_default() += 1;
        }
        let gauge_json = |g: &[Gauge]| -> Vec<Value> {
            g.iter().map(|g| json!({"max_in": if g.max_in.is_finite() { json!(g.max_in) } else { Value::Null }, "gauge": g.gauge, "lb_per_sqft": g.lb_per_sqft})).collect()
        };
        json!({
            "ducts": ducts, "pipes": pipes, "fittings": fittings, "terminals": terminals,
            "equipment": self.equipment, "hangers": hangers, "lug_kits": self.lug_kits, "labor_conditions": self.labor,
            "totals": {
                "duct_length_ft": r2(self.ducts.values().map(|r| r.length_ft).sum()),
                "duct_area_sqft": r2(self.ducts.values().map(|r| r.area_sqft).sum()),
                "duct_weight_lb": r2(self.ducts.values().map(|r| r.weight_lb).sum()),
                "pipe_length_ft": sum_len("pipe"),
                "conduit_length_ft": sum_len("conduit"),
                "fittings": self.fittings.values().sum::<usize>(),
                "terminals": self.terminals.values().sum::<usize>(),
                "equipment": self.equipment.len(),
                "equipment_by_kind": by_kind,
                "hangers": self.hangers.values().map(|r| r.count).sum::<usize>(),
                "hanger_rod_ft": r2(self.hangers.values().map(|r| r.rod_ft).sum()),
            },
            "gauge_table": {"rectangular": gauge_json(&st.rect), "round": gauge_json(&st.round), "allowance_pct": st.allowance * 100.0},
        })
    }

    /// The takeoff as one flat table (header first), the part asked for: what a spreadsheet,
    /// the CSV and the drawing's table show.
    fn rows(&self, part: &str) -> Vec<Vec<String>> {
        let n = |v: f64| format!("{}", r2(v));
        let mut out = vec![
            ["Category", "Level", "System", "Item", "Size", "Gauge", "Quantity", "Unit", "Area (sq ft)", "Weight (lb)"].map(String::from).to_vec(),
        ];
        let all = part == "all";
        if all || part == "ducts" {
            for ((l, s, size), r) in &self.ducts {
                out.push(vec![
                    "Duct".into(),
                    l.clone(),
                    s.clone(),
                    format!("{} duct", r.shape),
                    size.clone(),
                    r.gauge.to_string(),
                    n(r.length_ft),
                    "ft".into(),
                    n(r.area_sqft),
                    n(r.weight_lb),
                ]);
            }
        }
        if all || part == "pipes" {
            for ((l, s, k, size), r) in &self.pipes {
                let cat = if *k == "conduit" { "Conduit" } else { "Pipe" };
                out.push(vec![
                    cat.into(),
                    l.clone(),
                    s.clone(),
                    k.to_string(),
                    size.clone(),
                    String::new(),
                    n(r.length_ft),
                    "ft".into(),
                    String::new(),
                    String::new(),
                ]);
            }
        }
        if all || part == "fittings" {
            for ((l, s, ty, size), c) in &self.fittings {
                out.push(vec![
                    "Fitting".into(),
                    l.clone(),
                    s.clone(),
                    ty.clone(),
                    size.clone(),
                    String::new(),
                    c.to_string(),
                    "ea".into(),
                    String::new(),
                    String::new(),
                ]);
            }
        }
        if all || part == "terminals" {
            for ((l, s, k, size), c) in &self.terminals {
                out.push(vec![
                    "Terminal".into(),
                    l.clone(),
                    s.clone(),
                    k.clone(),
                    size.clone(),
                    String::new(),
                    c.to_string(),
                    "ea".into(),
                    String::new(),
                    String::new(),
                ]);
            }
        }
        if all || part == "equipment" {
            for e in &self.equipment {
                let sys = e["systems"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                let item = format!("{} {}", e["tag"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or("")).trim().to_string();
                out.push(vec![
                    "Equipment".into(),
                    e["level"].as_str().unwrap_or("").into(),
                    sys,
                    item,
                    e["size"].as_str().unwrap_or("").into(),
                    String::new(),
                    "1".into(),
                    "ea".into(),
                    String::new(),
                    String::new(),
                ]);
            }
        }
        if all || part == "lug_kits" {
            // Required items: lugs ordered separately from their gear, each with its bolt kit.
            for k in &self.lug_kits {
                let gear = k["gear"].as_str().unwrap_or("");
                let level = sp_level(&self.equipment, gear);
                let at = format!("{gear} {}", k["point"].as_str().unwrap_or(""));
                let mut line = |item: String, qty: &Value| {
                    out.push(vec![
                        "Lug kit".into(),
                        level.clone(),
                        at.clone(),
                        item,
                        String::new(),
                        String::new(),
                        qty.to_string(),
                        "ea".into(),
                        String::new(),
                        String::new(),
                    ]);
                };
                line(format!("lug, {}", k["lug"].as_str().unwrap_or("")), &k["lugs"]);
                line(format!("bolt, {}", k["bolt"].as_str().unwrap_or("")), &k["bolts"]);
                line("nut".into(), &k["nuts"]);
                line("flat washer".into(), &k["flat_washers"]);
                line("Belleville washer".into(), &k["belleville_washers"]);
            }
        }
        if all || part == "hangers" {
            for ((l, s, k, size), r) in self.hangers.iter().filter(|(_, r)| r.count > 0) {
                let item = format!("{k} hanger @ {}' (rod {} ft)", n(r.spacing_ft), n(r.rod_ft));
                out.push(vec![
                    "Hanger".into(),
                    l.clone(),
                    s.clone(),
                    item,
                    size.clone(),
                    String::new(),
                    r.count.to_string(),
                    "ea".into(),
                    String::new(),
                    String::new(),
                ]);
            }
        }
        out
    }
}

/// The level of the equipment tagged `tag` in the takeoff's equipment list.
fn sp_level(equipment: &[Value], tag: &str) -> String {
    equipment.iter().find(|e| e["tag"].as_str() == Some(tag)).and_then(|e| e["level"].as_str()).unwrap_or_default().to_string()
}

/// A CSV field: quoted when it holds a comma, quote or line break; text a spreadsheet would run
/// as a formula (`=`, `+`, `-`, `@` and not a number) starts with an apostrophe.
fn csv_field(text: &str) -> String {
    let formula = text.starts_with(['=', '+', '-', '@', '\t', '\r']) && text.parse::<f64>().is_err();
    let t = if formula { format!("'{text}") } else { text.to_string() };
    if t.contains([',', '"', '\n', '\r']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t }
}

fn csv(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for r in rows {
        out.push_str(&r.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

const PARTS: [&str; 8] = ["all", "ducts", "pipes", "fittings", "terminals", "equipment", "hangers", "lug_kits"];

// ---------------- linked tables ----------------

fn tag(v: String) -> RawTag {
    RawTag { code: 1000, value: v.chars().take(250).collect() }
}

/// The settings a linked table keeps (in its xdata), to count again with.
fn link_xdata(p: &Value, part: &str) -> Vec<RawTag> {
    let mut x = vec![RawTag { code: 1001, value: APP.into() }, tag(format!("part:{part}"))];
    for k in ["level", "system", "group_by"] {
        if let Some(v) = str_param(p, k) {
            x.push(tag(format!("{k}:{v}")));
        }
    }
    x.push(tag(format!("allowance_pct:{}", p.get("allowance_pct").and_then(Value::as_f64).unwrap_or(0.0))));
    x
}

/// A linked table's settings back from its xdata (None: not a takeoff table).
fn link_params(xdata: &[RawTag]) -> Option<(Value, String)> {
    let at = xdata.iter().position(|t| t.code == 1001 && t.value == APP)?;
    let mut p = json!({});
    let mut part = "all".to_string();
    for t in xdata.iter().skip(at + 1).take_while(|t| t.code != 1001) {
        let Some((k, v)) = t.value.split_once(':') else { continue };
        match k {
            "part" => part = v.to_string(),
            "level" | "system" | "group_by" => p[k] = json!(v),
            "allowance_pct" => p[k] = json!(v.parse::<f64>().unwrap_or(0.0)),
            _ => {}
        }
    }
    Some((p, part))
}

fn table_title(p: &Value) -> String {
    let mut t = "MEP Quantities".to_string();
    for k in ["level", "system"] {
        if let Some(v) = str_param(p, k) {
            t.push_str(&format!(" — {v}"));
        }
    }
    t
}

fn table_style(d: &Drawing) -> cadaddy_doc::TableStyle {
    let cur = d.header.str("CTABLESTYLE", "Standard");
    d.table_styles.iter().find(|t| t.name.eq_ignore_ascii_case(&cur)).or(d.table_styles.first()).cloned().unwrap_or_default()
}

fn make(d: &Drawing, at: Vec2, rows: &[Vec<String>], p: &Value) -> cadaddy_doc::Table {
    let st = table_style(d);
    let header = rows.first().cloned();
    let body: Vec<Vec<String>> = rows.iter().skip(1).take(MAX_TABLE_ROWS).cloned().collect();
    let cols = header.as_ref().map_or(1, Vec::len);
    let mut t = super::table::make_table(&st, at, body.len(), cols, None, None, Some(&table_title(p)), header, &body);
    // Item and size columns are wider.
    for (i, w) in t.col_widths.iter_mut().enumerate() {
        *w = st.text_height
            * if i == 3 {
                22.0
            } else if i == 4 || i == 2 {
                10.0
            } else {
                7.0
            };
    }
    t
}

/// Rebuild every linked takeoff table in model space from `sp`. The number refreshed.
pub(crate) fn refresh(d: &mut Drawing, sp: &BuildingSpec) -> usize {
    let linked: Vec<(Handle, Value, String, Vec2)> = d
        .model
        .iter_kinds(cadaddy_doc::KIND_TABLE)
        .filter_map(|e| {
            let EntityKind::Table(t) = &e.kind else { return None };
            let (p, part) = link_params(&e.common.xdata)?;
            Some((e.handle, p, part, Vec2::new(t.insert.x, t.insert.y)))
        })
        .take(MAX_TABLES)
        .collect();
    let mut n = 0;
    for (h, p, part, at) in linked {
        let Ok(st) = settings("mep.qty", &p) else { continue };
        let rows = takeoff(sp, &st).rows(&part);
        let mut t = make(d, at, &rows, &p);
        if let Some(EntityKind::Table(old)) = d.entity(h).map(|e| &e.kind) {
            t.style = old.style.clone();
            t.text_height = old.text_height;
            // Keep the column widths the user set.
            if old.col_widths.len() == t.col_widths.len() {
                t.col_widths = old.col_widths.clone();
            }
        }
        if d.modify_entity(h, |e| e.kind = EntityKind::Table(Box::new(t))).is_ok() {
            n += 1;
        }
    }
    n
}

// ---------------- commands ----------------

fn run_qty(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "mep.qty")?;
    let st = settings("mep.qty", p)?;
    if let Some(l) = &st.level
        && sp.level(l).is_none()
    {
        return Err(bad("mep.qty", format!("there is no level named {l}")));
    }
    let t = takeoff(&sp, &st);
    let rows = t.rows("all");
    let mut out = t.to_json(&st);
    out["rows"] = json!(rows.len().saturating_sub(1));
    out["csv"] = json!(csv(&rows));

    if let Some(path) = str_param(p, "file").filter(|x| !x.trim().is_empty()) {
        let format = match str_param(p, "format") {
            Some(f) => f.to_ascii_lowercase(),
            None => std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("csv").to_ascii_lowercase(),
        };
        let bytes = match format.as_str() {
            "csv" => csv(&rows).into_bytes(),
            "xlsx" => super::table::xlsx::write(&rows, "MEP Quantities").map_err(|e| bad("mep.qty", e))?,
            _ => return Err(bad("mep.qty", "`format` is csv or xlsx")),
        };
        out["format"] = json!(format);
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
            out["path"] = json!(path);
        }
        #[cfg(target_arch = "wasm32")]
        {
            out["data"] = json!(super::file::base64_encode(&bytes));
        }
    }

    if let Some(tp) = p.get("table").filter(|v| !v.is_null()) {
        let at = point_req("mep.qty", tp, "at")?;
        let part = str_param(tp, "part").unwrap_or("all");
        if !PARTS.contains(&part) {
            return Err(bad("mep.qty", format!("table `part` is one of {}", PARTS.join(", "))));
        }
        let rows = t.rows(part);
        let table = make(s.doc()?, at, &rows, p);
        let layer = s.doc()?.header.str("CLAYER", "0");
        let common = Common { layer, xdata: link_xdata(p, part), ..Common::default() };
        let h = s.doc_mut()?.add(&Space::Model, common, EntityKind::Table(Box::new(table)))?;
        out["table"] = json!(h.hex());
    }
    s.echo(format!(
        "MEP quantities: {} ft of duct ({} sq ft, {} lb), {} ft of pipe, {} ft of conduit, {} fittings, {} terminals, {} equipment, {} hangers.",
        out["totals"]["duct_length_ft"],
        out["totals"]["duct_area_sqft"],
        out["totals"]["duct_weight_lb"],
        out["totals"]["pipe_length_ft"],
        out["totals"]["conduit_length_ft"],
        out["totals"]["fittings"],
        out["totals"]["terminals"],
        out["totals"]["equipment"],
        out["totals"]["hangers"]
    ));
    Ok(out)
}

fn run_refresh(s: &mut Session, _: &Value) -> Result<Value> {
    let (sp, _) = current(s, "mep.qty.refresh")?;
    let n = refresh(s.doc_mut()?, &sp);
    Ok(json!({ "tables": n }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_printed() {
        assert_eq!(inches(0.75), "3/4\"");
        assert_eq!(inches(1.5), "1-1/2\"");
        assert_eq!(inches(10.0), "10\"");
        assert_eq!(inches(f64::NAN), "?");
        assert_eq!(size_label(MepKind::Duct, [24.0, 12.0]), "24x12");
        assert_eq!(size_label(MepKind::Duct, [10.0, 0.0]), "10\"Ø");
        assert_eq!(size_label(MepKind::Pipe, [2.0, 0.0]), "2\"");
        assert_eq!(ft_in(4.5), "4'-6\"");
    }

    #[test]
    fn gauges_by_longer_side_or_diameter() {
        let st = Settings::default();
        assert_eq!(st.gauge([12.0, 8.0]).map(|g| g.gauge), Some(26));
        assert_eq!(st.gauge([8.0, 24.0]).map(|g| g.gauge), Some(24), "the longer side decides");
        assert_eq!(st.gauge([60.0, 20.0]).map(|g| g.gauge), Some(20));
        assert_eq!(st.gauge([120.0, 20.0]).map(|g| g.gauge), Some(18));
        assert_eq!(st.gauge([20.0, 0.0]).map(|g| g.gauge), Some(22), "round: 19–30\" is 22 ga");
    }

    #[test]
    fn hanger_spacing_by_size() {
        let st = Settings::default();
        assert_eq!(st.spacing(MepKind::Pipe, [0.75, 0.0]), 6.0);
        assert_eq!(st.spacing(MepKind::Pipe, [2.0, 0.0]), 8.0);
        assert_eq!(st.spacing(MepKind::Pipe, [4.0, 0.0]), 10.0);
        assert_eq!(st.spacing(MepKind::Pipe, [6.0, 0.0]), 12.0);
        assert_eq!(st.spacing(MepKind::Duct, [24.0, 12.0]), 8.0);
        assert_eq!(st.spacing(MepKind::Duct, [12.0, 0.0]), 10.0);
    }

    #[test]
    fn corners_are_elbows_and_drops() {
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0), Vec2::new(30.0, 20.0)];
        let c = corners(&pts, &[9.0, 9.0, 8.0, 8.0]);
        assert!(matches!(c.first(), Some(Corner::Elbow(d)) if (d - 90.0).abs() < 1e-6), "{c:?}");
        assert!(matches!(c.get(1), Some(Corner::Drop(d)) if (d - 1.0).abs() < 1e-9));
        assert!(matches!(c.get(2), Some(Corner::Elbow(d)) if (d - 45.0).abs() < 1e-6));
        assert_eq!(elbow_name(88.0), "elbow 90°");
        assert_eq!(elbow_name(30.0), "elbow 30°");
    }

    #[test]
    fn csv_quotes_and_defuses_formulas() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(csv_field("-2.5"), "-2.5");
        assert_eq!(csv_field("@SUM"), "'@SUM");
    }

    #[test]
    fn link_settings_round_trip() {
        let x = link_xdata(&json!({"level": "L2", "system": "SA", "group_by": "system", "allowance_pct": 15.0}), "ducts");
        let (p, part) = link_params(&x).unwrap();
        assert_eq!(part, "ducts");
        assert_eq!(p["level"], "L2");
        assert_eq!(p["system"], "SA");
        assert_eq!(p["group_by"], "system");
        assert_eq!(p["allowance_pct"], 15.0);
        assert!(link_params(&[RawTag { code: 1001, value: "OTHER".into() }]).is_none());
    }

    #[test]
    fn bad_settings_are_refused() {
        assert!(settings("t", &json!({"group_by": "room"})).is_err());
        assert!(settings("t", &json!({"gauges": []})).is_err());
        assert!(settings("t", &json!({"gauges": [{"max_in": 10, "gauge": 99, "lb_per_sqft": 1}]})).is_err());
        assert!(settings("t", &json!({"hanger_spacing_ft": {"pipe": 0}})).is_err());
        assert!(settings("t", &json!({"allowance_pct": 300})).is_err());
        let st = settings("t", &json!({"gauges": [{"gauge": 20, "lb_per_sqft": 1.656}, {"max_in": 20, "gauge": 24, "lb_per_sqft": 1.156}]})).unwrap();
        assert_eq!(st.gauge([10.0, 10.0]).map(|g| g.gauge), Some(24), "sorted by size, no max = any size");
        assert_eq!(st.gauge([40.0, 10.0]).map(|g| g.gauge), Some(20));
    }
}
