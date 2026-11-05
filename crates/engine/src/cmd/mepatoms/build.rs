//! The atoms in the model (`model.build`): the electrical runs of the spec stitched end to end,
//! each cut into its pieces, the boxes at the devices, and every piece one element with its
//! properties, its 3D body and what it connects to; then the count index.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, MepKind, MepRun};
use serde_json::{Value, json};

use super::super::propfill::WithNote;
use super::{Atom, Pieces, WireOpts, X3, cable_pieces, large_conductors, raceway_pieces, rule_source, rules, trade_size, wires, wires_with};

/// Most runs, devices and atoms the build makes (hostile specs).
const MAX_RUNS: usize = 50_000;
const MAX_DEVICES: usize = 100_000;
const MAX_ATOMS: usize = 1_500_000;
/// Sides of a stick's or a cable's tube.
const SIDES: u32 = 8;

/// A device's box in the making: where it is, which way its face looks, and its device.
struct DevBox {
    device: Handle,
    level: String,
    circuit: String,
    /// The point a run enters it at (its top centre; a floor box's: its centre).
    entry: Vec3,
    atom: usize,
}

/// A gear element (a panel) a home run can end at.
struct Gear {
    handle: Handle,
    level: String,
    tag: String,
    plan: Bounds2,
}

/// A run's 3D path (sheet units) and what it is.
struct Path3 {
    pts: Vec<Vec3>,
    run: usize,
}

/// One stitched run: its path, its runs, and the nodes at its ends.
struct Chain {
    pts: Vec<Vec3>,
    runs: Vec<usize>,
    ends: [usize; 2],
}

/// A run's group: the runs that may join end to end (one system, level and circuit).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    system: String,
    level: String,
    circuit_tag: String,
    cable: bool,
}

/// What a group's runs are: raceway material or cable kind, circuit, conductors, and sources.
#[derive(Clone, Default)]
struct Method {
    cable: bool,
    /// `EMT`… or `NM`…
    name: String,
    source: Source,
    circuit: String,
    panel: String,
    conductors: String,
    conductors_source: Source,
    estimated: bool,
    /// Routed by [`super::super::conduitroute`]: its ends land on their boxes' entries.
    routed: bool,
    /// A feeder's run (`mep.feeders` index): its conductors are the feeder's, landing on gear.
    feeder: Option<usize>,
}

/// Whether a box on circuit `boxc` is on a run's `circuit`: the same, or one of a homerun's list
/// (`LP1-1,3,5` holds `LP1-3`).
fn on_circuit(boxc: &str, circuit: &str) -> bool {
    if boxc == circuit {
        return true;
    }
    let split = |c: &str| -> Option<(String, Vec<String>)> {
        let (p, n) = c.trim().split_once('-')?;
        let nums: Vec<String> = n.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(64).collect();
        (!p.trim().is_empty() && !nums.is_empty()).then(|| (p.trim().to_ascii_uppercase(), nums))
    };
    match (split(boxc), split(circuit)) {
        (Some((pb, nb)), Some((pc, nc))) => pb == pc && nb.iter().all(|n| nc.contains(n)),
        _ => false,
    }
}

/// The 3D path of a run: its plan points at their segments' heights (a change of height is a
/// vertical leg at the corner); a riser straight up from its bottom to its top.
pub(crate) fn run_path(r: &MepRun, base_z: f64, upf: f64) -> Vec<Vec3> {
    let z = |i: usize| base_z + r.bottoms_ft.get(i).or(r.bottoms_ft.last()).copied().unwrap_or(0.0) * upf;
    if let Some(top) = r.top_ft {
        let Some(p) = r.points.first() else { return Vec::new() };
        let bot = r.bottoms_ft.first().copied().unwrap_or(0.0);
        if !(top.is_finite() && bot.is_finite() && top > bot) {
            return Vec::new();
        }
        return vec![Vec3::new(p[0], p[1], base_z + bot * upf), Vec3::new(p[0], p[1], base_z + top * upf)];
    }
    let mut out: Vec<Vec3> = Vec::new();
    for (i, w) in r.points.windows(2).enumerate().take(super::MAX_PIECES) {
        if let [a, b] = w {
            let zi = z(i);
            let pa = Vec3::new(a[0], a[1], zi);
            if out.last().is_none_or(|q| super::dist(*q, pa) > 1e-9) {
                out.push(pa);
            }
            out.push(Vec3::new(b[0], b[1], zi));
        }
    }
    out.retain(|p| p.is_finite());
    out
}

/// End points within `tol` of each other are one node.
struct Nodes {
    tol: f64,
    grid: HashMap<(i64, i64, i64), Vec<usize>>,
    at: Vec<Vec3>,
    members: Vec<Vec<(usize, usize)>>,
}

impl Nodes {
    fn new(tol: f64) -> Self {
        Nodes { tol: tol.max(1e-9), grid: HashMap::new(), at: Vec::new(), members: Vec::new() }
    }
    fn cell(&self, p: Vec3) -> (i64, i64, i64) {
        let c = |v: f64| (v / self.tol).floor().clamp(-1e15, 1e15) as i64;
        (c(p.x), c(p.y), c(p.z))
    }
    fn node(&mut self, p: Vec3, member: (usize, usize)) -> usize {
        let (cx, cy, cz) = self.cell(p);
        let mut found = None;
        'look: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &n in self.grid.get(&(cx + dx, cy + dy, cz + dz)).into_iter().flatten() {
                        if self.at.get(n).is_some_and(|q| super::dist(*q, p) <= self.tol) {
                            found = Some(n);
                            break 'look;
                        }
                    }
                }
            }
        }
        let n = found.unwrap_or_else(|| {
            let n = self.at.len();
            self.at.push(p);
            self.members.push(Vec::new());
            self.grid.entry((cx, cy, cz)).or_default().push(n);
            n
        });
        if let Some(m) = self.members.get_mut(n) {
            m.push(member);
        }
        n
    }
    fn degree(&self, n: usize) -> usize {
        self.members.get(n).map_or(0, Vec::len)
    }
}

/// Runs joined where exactly two ends meet: each chain goes from a free end, a junction (three
/// or more ends), a box (`breaks`: a device's box at the node, which the runs end in) or itself
/// (a loop) to the next.
fn stitch(paths: &[Path3], tol: f64, breaks: &dyn Fn(Vec3, usize, bool) -> bool) -> (Vec<Chain>, Nodes, Vec<bool>) {
    let mut nodes = Nodes::new(tol);
    let mut ends: Vec<[usize; 2]> = Vec::with_capacity(paths.len());
    for (i, p) in paths.iter().enumerate() {
        let (Some(a), Some(b)) = (p.pts.first().copied(), p.pts.last().copied()) else {
            ends.push([usize::MAX, usize::MAX]);
            continue;
        };
        let na = nodes.node(a, (i, 0));
        let nb = nodes.node(b, (i, 1));
        ends.push([na, nb]);
    }
    // Whether a run leaves the node straight up or down (a drop).
    let vertical = |n: usize| {
        nodes.members.get(n).into_iter().flatten().any(|&(i, e)| {
            let Some(p) = paths.get(i) else { return false };
            let k = p.pts.len();
            let (a, b) = if e == 0 { (p.pts.first(), p.pts.get(1)) } else { (p.pts.get(k.wrapping_sub(2)), p.pts.last()) };
            a.zip(b).is_some_and(|(a, b)| Vec2::new(a.x - b.x, a.y - b.y).len() <= tol && (a.z - b.z).abs() > tol)
        })
    };
    let brk: Vec<bool> = nodes.at.iter().enumerate().map(|(n, p)| breaks(*p, nodes.degree(n), vertical(n))).collect();
    let passes = |n: usize| nodes.degree(n) == 2 && !brk.get(n).copied().unwrap_or(false);
    let mut used = vec![false; paths.len()];
    let mut chains = Vec::new();
    let walk = |start: usize, from_end: usize, used: &mut Vec<bool>| -> Option<Chain> {
        let p = paths.get(start)?;
        let e = *ends.get(start)?;
        let mut pts = p.pts.clone();
        if from_end == 1 {
            pts.reverse();
        }
        let first = if from_end == 0 { e[0] } else { e[1] };
        let mut last = if from_end == 0 { e[1] } else { e[0] };
        if let Some(u) = used.get_mut(start) {
            *u = true;
        }
        let mut runs = vec![p.run];
        let mut guard = 0;
        while passes(last) && last != first && guard < paths.len() {
            guard += 1;
            let Some(m) = nodes.members.get(last) else { break };
            let Some(&(q, qe)) = m.iter().find(|(q, _)| !used.get(*q).copied().unwrap_or(true)) else { break };
            let (Some(qp), Some(qends)) = (paths.get(q), ends.get(q)) else { break };
            let mut more = qp.pts.clone();
            if qe == 1 {
                more.reverse();
            }
            pts.extend(more.into_iter().skip(1));
            last = if qe == 0 { qends[1] } else { qends[0] };
            runs.push(qp.run);
            if let Some(u) = used.get_mut(q) {
                *u = true;
            }
        }
        Some(Chain { pts, runs, ends: [first, last] })
    };
    // From every end that doesn't pass straight on, then what is left (loops).
    for i in 0..paths.len() {
        for end in 0..2 {
            if used.get(i).copied().unwrap_or(true) {
                break;
            }
            let n = ends.get(i).map_or(usize::MAX, |e| if end == 0 { e[0] } else { e[1] });
            if n != usize::MAX && !passes(n) {
                chains.extend(walk(i, end, &mut used));
            }
        }
    }
    for i in 0..paths.len() {
        if !used.get(i).copied().unwrap_or(true) && ends.get(i).is_some_and(|e| e[0] != usize::MAX) {
            chains.extend(walk(i, 0, &mut used));
        }
    }
    (chains, nodes, brk)
}

// ─── Bodies ──────────────────────────────────────────────────────────────────

/// A tube from `a` to `b` of radius `r` (any direction), with end caps.
pub(crate) fn tube(m: &mut Mesh, a: Vec3, b: Vec3, r: f64, sides: u32) {
    let d = b - a;
    let len = d.len();
    if !(len > 1e-9 && r > 1e-9 && a.is_finite() && b.is_finite()) {
        return;
    }
    let u = d * (1.0 / len);
    let helper = if u.z.abs() < 0.9 { Vec3::Z } else { X3 };
    let n1 = u.cross(helper);
    let n1 = n1 * (1.0 / n1.len().max(1e-12));
    let n2 = u.cross(n1);
    let sides = sides.clamp(3, 32);
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    for end in [a, b] {
        for k in 0..sides {
            let t = f64::from(k) * std::f64::consts::TAU / f64::from(sides);
            m.vertices.push(end + n1 * (r * t.cos()) + n2 * (r * t.sin()));
        }
    }
    for k in 0..sides {
        let j = (k + 1) % sides;
        m.faces.push(MeshFace { v: vec![base + k, base + j, base + sides + j, base + sides + k], hidden: 0 });
    }
    m.faces.push(MeshFace { v: (0..sides).rev().map(|k| base + k).collect(), hidden: 0 });
    m.faces.push(MeshFace { v: (0..sides).map(|k| base + sides + k).collect(), hidden: 0 });
}

/// A box centred at `c`: `w` along `along` (horizontal), `t` across it, `h` high.
fn block(m: &mut Mesh, c: Vec3, along: Vec2, w: f64, t: f64, h: f64) {
    let along = if along.len() > 1e-9 { along * (1.0 / along.len()) } else { Vec2::new(1.0, 0.0) };
    let half = along * (w / 2.0);
    let p = Vec2::new(c.x, c.y);
    super::super::wallgen::add_box(m, p - half, p + half, t, c.z - h / 2.0, c.z + h / 2.0);
}

/// A piece's body (sheet units).
pub(crate) fn body(a: &Atom, upf: f64) -> Mesh {
    let mut m = super::super::wallgen::empty_mesh();
    let r = (a.body_in.max(0.1) / 24.0) * upf;
    let dir3 = a.b - a.a;
    let dir = if dir3.len() > 1e-12 { dir3 * (1.0 / dir3.len()) } else { X3 };
    let plan = Vec2::new(dir.x, dir.y);
    match a.kind {
        "conduit" if a.path.len() < 2 => tube(&mut m, a.a, a.b, r, SIDES),
        "conduit" | "cable" | "wire" => {
            for w in a.path.windows(2).take(super::MAX_PIECES) {
                if let [p, q] = w {
                    tube(
                        &mut m,
                        *p,
                        *q,
                        r,
                        match a.kind {
                            "wire" => 3,
                            "cable" => 6,
                            _ => SIDES,
                        },
                    );
                }
            }
        }
        "coupling" | "connector" | "locknut" | "bushing" => {
            let half = (if a.kind == "coupling" { 1.0 } else { 0.4 }) / 12.0 * upf;
            tube(&mut m, a.a - dir * half, a.a + dir * half, r, SIDES);
        }
        "strap" => block(&mut m, a.a, plan, 0.75 / 12.0 * upf, r * 2.4, r * 2.4),
        "elbow" | "offset" => tube(&mut m, a.a - dir * (r * 1.5), a.a + dir * (r * 1.5), r * 1.15, SIDES),
        // A field bend is part of its stick: a thin band marks it.
        "bend" => tube(&mut m, a.a - dir * (r * 0.3), a.a + dir * (r * 0.3), r * 1.05, SIDES),
        // A value engineering scenario's own boxes, brackets, plates and fittings (Q6202).
        "box" | "bracket" | "nail_plate" | "fitting" => {
            let s = a.body_in.max(0.1) / 12.0 * upf;
            block(&mut m, a.a, plan, s, s * 0.5, s);
        }
        _ => {}
    }
    m
}

// ─── The build ───────────────────────────────────────────────────────────────

/// A stick's two ends as stored (`start`, `end` properties).
pub(crate) fn stick_ends(ps: &Props) -> Option<(Vec3, Vec3)> {
    let p = |k: &str| {
        let t = ps.get(k)?.text();
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
        match v[..] {
            [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() => Some(Vec3::new(x, y, z)),
            _ => None,
        }
    };
    Some((p("start")?, p("end")?))
}

/// The diameter a piece is drawn with (`body_in`), inches.
pub(crate) fn body_of(ps: &Props) -> f64 {
    ps.get("body_in").map(Prop::text).and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0 && *v < 100.0).unwrap_or(1.0)
}

/// What kind of box a device takes (none: a plumbing fixture, equipment).
pub(crate) fn box_kind(kind: &str, layer: &str) -> Option<&'static str> {
    let k = kind.replace('_', " ").to_ascii_lowercase();
    let layer = layer.to_ascii_uppercase();
    if layer.contains("PLUMB") || k.contains("plumbing") || k.contains("sprinkler") {
        return None;
    }
    Some(if k.contains("receptacle") || k.contains("outlet") && !k.contains("data") && !k.contains("voltage") {
        "receptacle"
    } else if k.contains("switch") || k.contains("dimmer") || k.contains("occupancy") {
        "switch"
    } else if k.contains("fan") {
        "fan"
    } else if k.contains("luminaire") || k.contains("light") || k.contains("fixture") || layer.contains("LIGHT") || layer.contains("LTG") {
        "luminaire"
    } else if k.contains("fire alarm") || k.contains("smoke") || k.contains("horn") || k.contains("strobe") || k.contains("pull station") {
        "fire alarm device"
    } else if k.contains("thermostat") {
        "thermostat"
    } else if k.contains("data") || k.contains("voltage") || k.contains("tel") || k.contains("tv") || k.contains("catv") {
        "low voltage outlet"
    } else if k.contains("connection") || k.contains("junction") || k.contains("disconnect") {
        "equipment connection"
    } else if k == "device" {
        "device"
    } else {
        return None;
    })
}

/// The room holding a plan point on a level.
struct Rooms {
    by_level: HashMap<String, Vec<(Bounds2, Vec<Vec2>, String)>>,
}

impl Rooms {
    fn of(d: &Drawing) -> Self {
        let mut by_level: HashMap<String, Vec<(Bounds2, Vec<Vec2>, String)>> = HashMap::new();
        for r in super::super::bspec::room_elements(d).into_iter().filter(|r| r.outline.len() >= 3) {
            let label = r.number.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.name.trim().to_string());
            by_level.entry(r.level.to_ascii_lowercase()).or_default().push((Bounds2::from_points(r.outline.iter().copied()), r.outline, label));
        }
        Rooms { by_level }
    }
    fn at(&self, level: &str, p: Vec2) -> String {
        self.by_level
            .get(&level.to_ascii_lowercase())
            .and_then(|rs| rs.iter().find(|(b, o, _)| b.contains(p) && cadaddy_geom::point_in_polygon(o, p)))
            .map(|(_, _, l)| l.clone())
            .unwrap_or_default()
    }
}

/// The project's wiring method for branch circuits, when one is set (not the built-in).
fn wiring_method(d: &Drawing) -> Option<(String, Source)> {
    let r = d.project_rules.get(cadaddy_doc::projrules::RuleDomain::WiringMethod, "general")?;
    let v = r.active.value.trim().to_string();
    (!v.is_empty()).then(|| (v.clone(), Source::from_doc(SourceKind::Rule, &format!("project rule wiring_method/general: {v}"))))
}

/// The tags that make an atom a built MEP element (its level, source, system, kind and part).
pub(crate) fn mep_tags(level: &str, source: Handle, system: &str, kind: &str, size: &str, item: &str, mepid: &str) -> Vec<RawTag> {
    let t = |code: i32, v: String| RawTag { code, value: v.chars().take(250).collect() };
    let mk = if kind == "cable" { "cable" } else { "conduit" };
    vec![
        t(1001, super::super::wallgen::APP.into()),
        t(1000, "mep".into()),
        t(1000, level.to_string()),
        t(1000, source.hex()),
        t(1000, format!("system:{system}")),
        t(1000, format!("kind:{mk}")),
        t(1000, format!("part:atom:{kind}")),
        t(1000, format!("size:{size}")),
        t(1000, format!("type:{item}")),
        t(1000, format!("mepid:{mepid}")),
    ]
}

/// The atoms of the spec's electrical runs and its devices' boxes, into the model; the counts.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let started = web_time::Instant::now();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let rl = rules();
    let rooms = Rooms::of(d);
    let wiring = wiring_method(d);
    let circuits: HashMap<&str, &cadaddy_walk3d::tags::ElementTag> =
        sp.tags.iter().filter(|t| t.kind == "circuit").map(|t| (t.id.as_str(), t)).collect();
    let field = |t: &cadaddy_walk3d::tags::ElementTag, k: &str| {
        t.fields.get(k).map(|f| (f.value.trim().to_string(), f.source.clone())).filter(|(v, _)| !v.is_empty())
    };
    let src_of = |kind: &str, what: &str| {
        let k = match kind {
            "plan" | "schedule" | "note" | "legend" | "spec" => SourceKind::Print,
            "user" => SourceKind::User,
            "default" => SourceKind::Default,
            _ => SourceKind::Rule,
        };
        Source::from_doc(k, what)
    };

    // -- What each run is: raceway or cable, its material, circuit and conductors. --
    let ve_feeders = super::super::valueeng::approved_feeders(d);
    let mut paths: BTreeMap<GroupKey, (Method, Vec<Path3>)> = BTreeMap::new();
    let mut cable_circuits: BTreeSet<String> = BTreeSet::new();
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_RUNS) {
        let Some(sy) = sp.mep.systems.iter().find(|s| s.name == r.system).filter(|s| s.show) else { continue };
        if !matches!(sy.kind, MepKind::Conduit | MepKind::Cable) {
            continue;
        }
        let Some(level) = sp.level(&r.level) else { continue };
        let pts = run_path(r, level.elevation_ft * upf, upf);
        if pts.len() < 2 {
            continue;
        }
        // A feeder's run: its feeder's conductors (its schedule's), grouped by its route.
        let feeder = super::super::ugplan::is_feeder_run(r).then(|| feeder_of_run(sp, r)).flatten();
        let ctag = match feeder {
            Some(_) => r.source_sheet.clone(),
            None => super::super::printcond::circuit_of_run(r).unwrap_or_default().to_string(),
        };
        let tag = circuits.get(ctag.as_str()).copied();
        let estimated = r.height_source == super::super::printcond::ESTIMATED || r.system == super::super::printcond::CIRCUIT_SYSTEM;
        // A routed run (Q6152) ends at its box's top: moved onto the box's entry like an estimate.
        let routed = r.height_source == super::super::conduitroute::ROUTED;
        let (circuit, panel) = tag.map_or_else(Default::default, |t| {
            (field(t, "circuit").map(|v| v.0).unwrap_or_default(), field(t, "panel").map(|v| v.0).unwrap_or_default())
        });
        let (conductors, csrc) = tag.and_then(|t| field(t, "conductors")).map_or_else(
            || {
                (
                    super::super::printcond::DEFAULT_CONDUCTORS.to_string(),
                    Source::from_doc(SourceKind::Default, "a branch circuit's conductors where the plan prints none"),
                )
            },
            |(v, s)| {
                let what = format!("circuit {circuit}: {v}");
                (v, src_of(&s, &what))
            },
        );
        // The raceway the documents name (the circuit's, the system's), else the project's wiring
        // method, else the default; a cable system or a cable wiring method is cable.
        let named = tag.and_then(|t| field(t, "conduit")).filter(|(_, s)| s != "default");
        let from_named = named.as_ref().and_then(|(v, s)| {
            let src = || src_of(s, &format!("circuit {circuit}: {v}"));
            rl.raceway_named(v).map(|(k, _)| (false, k.to_string(), src())).or_else(|| rl.cable_named(v).map(|(k, _)| (true, k.to_string(), src())))
        });
        let from_system = || {
            let words = format!("{} {}", sy.name, sy.layer);
            match sy.kind {
                MepKind::Cable => Some(rl.cable_named(&words).map_or_else(
                    || (true, "MC".to_string(), Source::from_doc(SourceKind::Default, "a homerun's cable where the documents name no other: MC")),
                    |(k, _)| (true, k.to_string(), Source::from_doc(SourceKind::Rule, &format!("system {}", sy.name))),
                )),
                _ => {
                    rl.raceway_named(&words).map(|(k, _)| (false, k.to_string(), Source::from_doc(SourceKind::Rule, &format!("system {}", sy.name))))
                }
            }
        };
        // The project's wiring method decides only what the drawings leave to it: an estimated
        // route (a drawn conduit run is conduit).
        let from_rule = || {
            let (v, s) = wiring.as_ref().filter(|_| estimated)?;
            if let Some((k, _)) = rl.cable_named(v).filter(|_| rl.raceway_named(v).is_none()) {
                return Some((true, k.to_string(), s.clone()));
            }
            rl.raceway_named(v).map(|(k, _)| (false, k.to_string(), s.clone()))
        };
        let (cable, name, source) = from_named
            .or_else(from_system)
            .or_else(from_rule)
            .unwrap_or_else(|| (false, rl.default_raceway.clone(), Source::from_doc(SourceKind::Default, &rl.default_raceway_source)));
        if cable && !circuit.is_empty() {
            cable_circuits.insert(circuit.clone());
        }
        // A feeder an approved value engineering alternative changed: its cable, its own chain.
        let ve = (r.system == super::super::gearbuild::FEEDER_SYSTEM).then(|| ve_feeders.get(&r.source_sheet)).flatten().and_then(|(cable, why)| {
            let f = sp.mep.feeders.iter().find(|f| r.source_sheet.starts_with(&format!("feeder {} {}→", f.tag, f.from)))?;
            rl.cables.contains_key(cable).then(|| (cable.clone(), why.clone(), super::super::valueeng::feeder_makeup(f)))
        });
        let (cable, name, source, conductors, csrc, ctag) = match ve {
            Some((c, why, makeup)) => {
                let src = Source::from_doc(SourceKind::Rule, &why);
                (true, c, src.clone(), makeup, src, r.source_sheet.clone())
            }
            None => match feeder.and_then(|fi| sp.mep.feeders.get(fi)) {
                Some(f) => {
                    let doc = if f.source.is_empty() { format!("feeder {}", f.tag) } else { format!("feeder {}: {}", f.tag, f.source) };
                    (cable, name, source, feeder_makeup(f), Source::from_doc(SourceKind::Print, &doc), ctag)
                }
                None => (cable, name, source, conductors, csrc, ctag),
            },
        };
        let panel = match feeder.and_then(|fi| sp.mep.feeders.get(fi)) {
            Some(f) => f.from.clone(),
            None => panel,
        };
        let key = GroupKey { system: r.system.clone(), level: level.name.clone(), circuit_tag: ctag, cable };
        let m = Method { cable, name, source, circuit, panel, conductors, conductors_source: csrc, estimated, routed, feeder };
        paths.entry(key).or_insert_with(|| (m, Vec::new())).1.push(Path3 { pts, run: i });
    }

    // -- The gear (panels) home runs end at. --
    let gear: Vec<Gear> = d
        .model
        .iter()
        .filter(|e| super::super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")))
        .filter_map(|e| {
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            (!b.is_empty()).then(|| Gear {
                handle: e.handle,
                level: super::super::bspec::made_level(e).unwrap_or_default(),
                tag: super::super::bimlook::made_tag(e, "tag").unwrap_or_default().to_string(),
                plan: Bounds2::from_points([b.min, b.max]),
            })
        })
        .take(MAX_DEVICES)
        .collect();

    // -- The atoms: one list for the whole build, their connections by index. --
    let mut atoms: Vec<(Atom, AtomCtx)> = Vec::new();
    let mut external: Vec<(usize, Handle)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // -- A box at each device, a ring on a wall box. --
    let mut boxes: Vec<DevBox> = Vec::new();
    let devices: Vec<(Handle, String, Props, String, Bounds2, f64, f64)> = d
        .model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("device"))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let (z0, z1) = m
                .vertices
                .iter()
                .take(100_000)
                .filter(|p| p.z.is_finite())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.z), b.max(p.z)));
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            (z0 <= z1 && !b.is_empty()).then(|| {
                (
                    e.handle,
                    super::super::bspec::made_level(e).unwrap_or_default(),
                    props::read(&e.common.xdata),
                    e.common.layer.to_string(),
                    Bounds2::from_points([b.min, b.max]),
                    z0,
                    z1,
                )
            })
        })
        .take(MAX_DEVICES)
        .collect();
    // The built walls' faces, for a wall box: its front on the face of its wall.
    // The face's direction is `face_normal` (older models kept it in `host_face`, now the face's name).
    let walls = if devices.iter().any(|d| d.2.get("face_normal").or_else(|| d.2.get("host_face")).is_some()) {
        super::super::propfill::wall_footprints(d)
    } else {
        Vec::new()
    };
    // Another trade's devices (Q6167): a box and ring only where the documents make them ours.
    let scope = super::super::scope::decisions(d);
    let sr = super::super::scope::rules();
    // Luminaires by type: (with a box, without, why).
    let mut lum_boxes: BTreeMap<String, (usize, usize, BTreeSet<String>)> = BTreeMap::new();
    // A switch's or sensor's box with no circuit of its own is on the circuit of what it
    // controls (the router joins it to that circuit: its switch leg lands in it).
    let circuit_of: HashMap<Handle, String> =
        devices.iter().filter_map(|(h, _, ps, ..)| Some((*h, ps.get("circuit").map(Prop::text).filter(|c| !c.trim().is_empty())?))).collect();
    for (h, level, ps, layer, plan, z0, z1) in &devices {
        let kind = ps.get("kind").map(Prop::text).unwrap_or_default();
        let Some(bk) = box_kind(&kind, layer) else { continue };
        let theirs = sr.class_of(&kind, "").and_then(|c| scope.iter().find(|x| x.class == c.class)).filter(|x| x.by_others()).map(|x| {
            let fa = ps.get("fa_kind").map(Prop::text).unwrap_or_default();
            let ours = x.ours_for(&kind, &fa);
            (ours.iter().any(|i| i.item == "box"), ours.iter().any(|i| i.item == "ring"))
        });
        if theirs.is_some_and(|(b, _)| !b) {
            continue;
        }
        // A device placed outside its level's footprint is flagged for review: no box with it.
        if ps.get("misplaced").is_some() {
            continue;
        }
        // A luminaire takes an outlet box only when its type needs one: what its schedule and
        // its submittal say of it (rules `luminaire_box`).
        let lum_box = (bk == "luminaire").then(|| luminaire_box(ps, &sp.mep.fixture_schedule));
        if let Some(dec) = &lum_box {
            let mark = ps.get("type_mark").map(Prop::text).filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "(no type)".into());
            let row = lum_boxes.entry(mark).or_insert_with(|| (0, 0, BTreeSet::new()));
            if dec.takes_box {
                row.0 += 1;
            } else {
                row.1 += 1;
            }
            row.2.insert(why_box(dec));
            if !dec.takes_box {
                continue;
            }
        }
        // A device of no known kind at the ceiling takes a ceiling box.
        let ceiling = sp.level(level).map(|l| (l.elevation_ft + if l.ceiling_ft > 0.0 { l.ceiling_ft } else { l.height_ft }) * upf);
        let bk = if matches!(bk, "device" | "low voltage outlet") && ceiling.is_some_and(|c| *z1 >= c - upf) { "luminaire" } else { bk };
        let circuit = ps
            .get("circuit")
            .map(Prop::text)
            .filter(|c| !c.trim().is_empty())
            .or_else(|| super::super::conduitroute::controlled_circuit(&super::super::conduitroute::controls_of(ps), &circuit_of))
            .unwrap_or_default();
        let on_cable = cable_circuits.contains(&circuit);
        let Some((br, ring)) = rl.box_for(bk, on_cable) else { continue };
        let ring = ring && theirs.is_none_or(|(_, r)| r);
        let c = plan.center();
        let out = ["face_normal", "host_face"].iter().find_map(|k| {
            let t = ps.get(k).map(Prop::text)?;
            let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            match v[..] {
                [x, y] if x.is_finite() && y.is_finite() && x.hypot(y) > 1e-9 => Some(Vec2::new(x, y) * (1.0 / x.hypot(y))),
                _ => None,
            }
        });
        let [bw, bh, bd] = br.dims_in.map(|v| v.max(0.5) / 12.0 * upf);
        let floor = sp.level(level).map_or(0.0, |l| l.elevation_ft * upf);
        let (centre, entry, along, across) = match (bk, out) {
            ("luminaire" | "fan", _) => {
                let ctr = Vec3::new(c.x, c.y, z1 + bd / 2.0);
                (ctr, Vec3::new(c.x, c.y, z1 + bd), Vec2::new(1.0, 0.0), bw)
            }
            ("floor", _) => (Vec3::new(c.x, c.y, floor - bd / 2.0), Vec3::new(c.x, c.y, floor - bd), Vec2::new(1.0, 0.0), bw),
            (_, Some(o)) => {
                // In its wall, its front on the wall's face (where the device's is: on the face,
                // or flush in it); with no face found, behind the device.
                let depth_plan = (plan.max - plan.min).dot(Vec2::new(o.x.abs(), o.y.abs()));
                let face = super::super::propfill::wall_face(&walls, level, c, super::super::propfill::HOST_SNAP_FT * upf, None)
                    .filter(|f| f.out.dot(o) > 0.5)
                    .map_or_else(|| c - o * (depth_plan / 2.0), |f| c + o * ((f.at - c).dot(o)));
                let mid = face - o * (bd / 2.0);
                let zc = (z0 + z1) / 2.0;
                (Vec3::new(mid.x, mid.y, zc), Vec3::new(mid.x, mid.y, zc + bh / 2.0), o.perp(), bd)
            }
            _ => {
                let zc = (z0 + z1) / 2.0;
                (Vec3::new(c.x, c.y, zc), Vec3::new(c.x, c.y, zc + bh / 2.0), Vec2::new(1.0, 0.0), bd)
            }
        };
        let ctx = AtomCtx {
            level: level.clone(),
            system: String::new(),
            circuit: circuit.clone(),
            panel: ps.get("panel").map(Prop::text).unwrap_or_default(),
            run: String::new(),
            estimated: false,
            source: *h,
            body: Some((centre, along, bw, across, bh)),
            src: rule_source(&format!("{} — {}", rl.box_source, br.source)),
        };
        let mut a = Atom { kind: "box", item: br.item.clone(), ty: br.size.clone(), size: br.size.clone(), a: centre, b: centre, ..Atom::default() };
        a.extra.push(("cubic_in", format!("{}", br.cubic_in), br.source.clone()));
        a.extra.push(("depth_in", format!("{}", br.depth_in), br.source.clone()));
        a.extra.push(("material", br.material.clone(), br.source.clone()));
        a.extra.push(("box_of", h.hex(), "computed".into()));
        if let Some(dec) = &lum_box {
            a.extra.push(("box_needed", why_box(dec), dec.source.clone()));
        }
        let bi = atoms.len();
        atoms.push((a, ctx.clone()));
        external.push((bi, *h));
        if ring
            && !on_cable
            && let Some(o) = out
        {
            let depth = ps.get("mud_ring").map(Prop::text).and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0 && *v < 6.0);
            let (depth, src) = depth.map_or((rl.ring.depth_in, rl.ring.source.clone()), |v| {
                (v, "the box's wall type: the board on its face (partition legend)".to_string())
            });
            let face = Vec3::new(centre.x, centre.y, centre.z) + Vec3::new(o.x, o.y, 0.0) * (bd / 2.0 + depth / 24.0 * upf);
            let mut rg = Atom {
                kind: "ring",
                item: rl.ring.item.clone(),
                ty: rl.ring.size.clone(),
                size: format!("{}, {} deep", rl.ring.size, super::super::walltypes::frac(depth)),
                a: face,
                b: face,
                connects: vec![bi],
                ..Atom::default()
            };
            rg.extra.push(("depth_in", format!("{depth}"), src.clone()));
            let mut rctx = ctx.clone();
            rctx.body = Some((face, o.perp(), bw * 0.9, depth / 12.0 * upf, bh * 0.9));
            rctx.src = rule_source(&src);
            let ri = atoms.len();
            atoms.push((rg, rctx));
            if let Some(b) = atoms.get_mut(bi) {
                b.0.connects.push(ri);
            }
        }
        boxes.push(DevBox { device: *h, level: level.clone(), circuit, entry, atom: bi });
    }

    // -- Each group's runs stitched end to end, then cut into their pieces. --
    let tol = rl.joint_tol_in.max(0.01) / 12.0 * upf;
    let reach = rl.box_reach_ft.max(0.0) * upf;
    let panel_reach = rl.panel_reach_ft.max(0.0) * upf;
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let (mut chains_n, mut junctions, mut to_boxes, mut to_gear, mut open_ends, mut entry_gaps) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut over_bends = 0usize;
    let (mut feeder_chains, mut feeder_wire_ft) = (0usize, 0.0_f64);
    let mut landing_ft = 0.0_f64;
    let mut bend_notes: Vec<(usize, [Option<usize>; 2], String, String)> = Vec::new();
    // The box a run end at `p` lands in: on its level, within reach of the box's entry; a box
    // of its own circuit first, else the nearest one no other circuit took.
    // `own_only`: a box of its own circuit at any height under or over it (a route the readers
    // gave no drop to it).
    let box_near = |boxes: &[DevBox], taken: &BTreeSet<usize>, level: &str, circuit: &str, p: Vec3, own_only: bool| -> Option<usize> {
        let pp = Vec2::new(p.x, p.y);
        boxes
            .iter()
            .enumerate()
            .filter(|(bi, b)| b.level.eq_ignore_ascii_case(level) && (on_circuit(&b.circuit, circuit) || !own_only && !taken.contains(bi)))
            .map(|(bi, b)| (bi, on_circuit(&b.circuit, circuit), Vec2::new(b.entry.x, b.entry.y).dist(pp), (b.entry.z - p.z).abs()))
            .filter(|(_, own, dp, dz)| *dp <= reach && (*dz <= 3.0 * upf || own_only && *own))
            .min_by(|a, b| (!a.1, a.2 + a.3).partial_cmp(&(!b.1, b.2 + b.3)).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(bi, _, _, _)| bi)
    };
    for (key, (m, ps)) in &paths {
        // A run passing a box of its own circuit ends in it (and the next leaves it).
        // A run passing a box of its own circuit ends in it (and the next leaves it): one at its
        // height, or one under or over it when no drop leaves the node for it.
        let breaks = |p: Vec3, degree: usize, drop_here: bool| {
            degree >= 2
                && !m.circuit.is_empty()
                && box_near(&boxes, &taken, &key.level, &m.circuit, p, true)
                    .and_then(|bi| boxes.get(bi))
                    .is_some_and(|b| (b.entry.z - p.z).abs() <= 3.0 * upf || !drop_here)
        };
        let (mut chains, nodes, brk) = stitch(ps, tol, &breaks);
        // Junction boxes where three or more runs meet.
        let mut junction_at: HashMap<usize, usize> = HashMap::new();
        for c in &chains {
            for n in c.ends {
                if m.feeder.is_none()
                    && nodes.degree(n) >= 3
                    && !brk.get(n).copied().unwrap_or(false)
                    && !junction_at.contains_key(&n)
                    && atoms.len() < MAX_ATOMS
                {
                    let Some(at) = nodes.at.get(n).copied() else { continue };
                    let Some(br) = rl.boxes.get("junction") else { continue };
                    let [bw, bh, bd] = br.dims_in.map(|v| v.max(0.5) / 12.0 * upf);
                    let mut a =
                        Atom { kind: "box", item: br.item.clone(), ty: br.size.clone(), size: br.size.clone(), a: at, b: at, ..Atom::default() };
                    a.extra.push(("cubic_in", format!("{}", br.cubic_in), br.source.clone()));
                    a.extra.push(("junction", nodes.degree(n).to_string(), "computed: the runs that meet there".into()));
                    let ctx = AtomCtx {
                        level: key.level.clone(),
                        system: key.system.clone(),
                        circuit: m.circuit.clone(),
                        panel: m.panel.clone(),
                        run: String::new(),
                        estimated: m.estimated,
                        source: Handle(0),
                        body: Some((at, Vec2::new(1.0, 0.0), bw, bh, bd)),
                        src: rule_source(&br.source),
                    };
                    junction_at.insert(n, atoms.len());
                    atoms.push((a, ctx));
                    junctions += 1;
                }
            }
        }
        let mut gaps: Vec<(usize, usize, f64)> = Vec::new();
        for (ci, c) in chains.iter_mut().enumerate() {
            if atoms.len() >= MAX_ATOMS {
                notes.push("the model holds the most pieces it can: the rest of the runs were not cut into pieces".into());
                break;
            }
            chains_n += 1;
            // What each free end lands in: a device's box (an estimated route moves onto it), a
            // panel, else nothing (an open end).
            let mut lands: [Option<Land>; 2] = [None, None];
            for (end, land) in lands.iter_mut().enumerate() {
                let n = c.ends.get(end).copied().unwrap_or(usize::MAX);
                if let Some(&j) = junction_at.get(&n) {
                    *land = Some(Land::Atom(j));
                    continue;
                }
                let at_box = brk.get(n).copied().unwrap_or(false);
                if nodes.degree(n) != 1 && !at_box {
                    continue;
                }
                let Some(p) = (if end == 0 { c.pts.first() } else { c.pts.last() }).copied() else { continue };
                let pp = Vec2::new(p.x, p.y);
                // A feeder lands on gear, never in a device's box.
                let near_box = box_near(&boxes, &taken, &key.level, &m.circuit, p, at_box)
                    .filter(|_| m.feeder.is_none())
                    .and_then(|bi| boxes.get(bi).map(|b| (bi, b.entry, b.atom)));
                if let Some((bi, entry, atom)) = near_box {
                    taken.insert(bi);
                    let gap = (entry.z - p.z).abs();
                    if gap > 3.0 * upf {
                        // Not moved: the route gives no drop to it; the gap is said for review.
                        gaps.push((ci, end, gap / upf));
                    } else if m.estimated || m.routed {
                        let before = super::path_ft(&c.pts, upf);
                        land_on(&mut c.pts, end, entry, upf);
                        landing_ft += super::path_ft(&c.pts, upf) - before;
                    }
                    *land = Some(Land::Atom(atom));
                    continue;
                }
                let feeder_gear = |g: &Gear| m.feeder.and_then(|fi| sp.mep.feeders.get(fi)).is_none_or(|f| feeder_lands_on(f, &g.tag));
                let g = gear
                    .iter()
                    .filter(|g| g.level.eq_ignore_ascii_case(&key.level) && g.plan.expand(panel_reach).contains(pp) && feeder_gear(g))
                    .min_by(|a, b| {
                        let pa = if !m.panel.is_empty() && a.tag.eq_ignore_ascii_case(&m.panel) { 0.0 } else { 1.0 };
                        let pb = if !m.panel.is_empty() && b.tag.eq_ignore_ascii_case(&m.panel) { 0.0 } else { 1.0 };
                        (pa, a.plan.center().dist(pp)).partial_cmp(&(pb, b.plan.center().dist(pp))).unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some(g) = g {
                    *land = Some(Land::Gear(g.handle));
                }
            }
            let run_id = format!("{}#{}", c.runs.first().and_then(|i| sp.mep.runs.get(*i)).map_or_else(String::new, super::super::mep::run_base), ci);
            let source = c
                .runs
                .first()
                .and_then(|i| sp.mep.runs.get(*i))
                .and_then(|r| r.objects.iter().find_map(|o| u64::from_str_radix(o, 16).ok()))
                .map_or(Handle(0), Handle);
            let size_in = c.runs.first().and_then(|i| sp.mep.runs.get(*i)).map_or(0.75, |r| r.size_in[0]);
            let pieces: Pieces = if m.cable {
                let Some(cr) = rl.cables.get(&m.name) else { continue };
                cable_pieces(&c.pts, upf, &m.name, cr, &m.conductors, rl)
            } else {
                let Some(rw) = rl.raceways.get(&m.name) else { continue };
                raceway_pieces(&c.pts, upf, size_in, &m.name, rw, large_conductors(&m.conductors, rl.bushing_min_awg), rl)
            };
            // More bends between its ends than the code allows between pull points.
            let max_bends = if m.cable { 0.0 } else { rl.raceways.get(&m.name).map_or(0.0, |r| r.max_bends_deg) };
            let over = max_bends > 0.0 && pieces.bend_deg > max_bends + 1.0;
            let base = atoms.len();
            if over {
                over_bends += 1;
                let cite = rl.raceways.get(&m.name).map(|r| r.max_bends_source.clone()).unwrap_or_default();
                bend_notes.push((base, pieces.ends, format!("{:.0}", pieces.bend_deg), cite));
            }
            let ctx = AtomCtx {
                level: key.level.clone(),
                system: key.system.clone(),
                circuit: m.circuit.clone(),
                panel: m.panel.clone(),
                run: run_id.clone(),
                estimated: m.estimated,
                source,
                body: None,
                src: m.source.clone(),
            };
            // Its conductors' makeup at a panel it ends in: to the breaker, not a box's.
            let at_panel = lands.iter().filter(|l| matches!(l, Some(Land::Gear(_)))).count();
            for mut a in pieces.atoms {
                for x in &mut a.connects {
                    *x += base;
                }
                if a.kind == "cable" {
                    panel_makeup(&mut a, at_panel, rl);
                }
                atoms.push((a, ctx.clone()));
            }
            // Wires in a raceway with a circuit.
            if !m.cable && !m.circuit.is_empty() {
                let nums: Vec<u32> = super::super::printcond::parse_circuit(&m.circuit)
                    .map(|(_, n)| n.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                    .unwrap_or_default();
                let voltage = gear_voltage(sp, &m.panel).unwrap_or_else(|| rl.default_voltage.clone());
                let ends: Vec<usize> = pieces.ends.iter().flatten().map(|e| e + base).collect();
                for mut w in wires(&c.pts, upf, &m.conductors, &nums, &voltage, rl) {
                    w.connects = ends.clone();
                    panel_makeup(&mut w, at_panel, rl);
                    w.extra.push(("conductors_of_circuit", m.conductors.clone(), m.conductors_source.doc.clone().unwrap_or_default()));
                    let mut wctx = ctx.clone();
                    wctx.src = Source::from_doc(
                        m.conductors_source.kind,
                        &format!("{} — {}", m.conductors_source.doc.clone().unwrap_or_default(), rl.insulation_source),
                    );
                    atoms.push((w, wctx));
                }
            }
            // A feeder's conductors in each of its raceways (its sets shared among them), with
            // the makeup at each end that lands on gear.
            if let Some(f) = m.feeder.filter(|_| !m.cable).and_then(|fi| sp.mep.feeders.get(fi)) {
                let per = f.sets.max(1).div_ceil(f.conduits.max(1)).clamp(1, 12);
                let phases: Vec<u32> = [1u32, 3, 5].into_iter().take(f.phases.clamp(1, 3) as usize).collect();
                let voltage = gear_voltage(sp, &f.to).or_else(|| gear_voltage(sp, &f.from)).unwrap_or_else(|| rl.default_voltage.clone());
                let mk = if rl.feeder_makeup_ft.is_finite() { rl.feeder_makeup_ft.max(0.0) } else { 0.0 };
                let mut ends_mk = [0usize, 1].map(|end| if matches!(lands.get(end), Some(Some(Land::Gear(_)))) { mk } else { 0.0 });
                // Both ends by one piece of gear: one termination, at the end nearer its middle;
                // a riser on into the floor above (a feed-through) ends in the gear up there too.
                if let [Some(Land::Gear(g0)), Some(Land::Gear(g1))] = &lands
                    && g0 == g1
                {
                    let (a, b) = (c.pts.first().copied().unwrap_or(Vec3::ZERO), c.pts.last().copied().unwrap_or(Vec3::ZERO));
                    let top = sp.level(&key.level).map_or(f64::INFINITY, |l| (l.elevation_ft + l.height_ft) * upf);
                    let riser_up = Vec2::new(a.x, a.y).dist(Vec2::new(b.x, b.y)) < tol && a.z.max(b.z) >= top - tol;
                    if !riser_up {
                        let mid = gear.iter().find(|g| g.handle == *g0).map_or(Vec2::new(a.x, a.y), |g| g.plan.center());
                        let far = if Vec2::new(a.x, a.y).dist(mid) <= Vec2::new(b.x, b.y).dist(mid) { 1 } else { 0 };
                        if let Some(x) = ends_mk.get_mut(far) {
                            *x = 0.0;
                        }
                    }
                }
                let material = f.material.trim().to_ascii_lowercase();
                let kind = rl.feeder_conductors.get(&material).or_else(|| rl.feeder_conductors.get("cu"));
                let opts = WireOpts {
                    kind: kind.map(|k| (k, rl.feeder_conductors_source.as_str())),
                    makeup_ft: ends_mk,
                    makeup_source: &rl.feeder_makeup_source,
                };
                let ends: Vec<usize> = pieces.ends.iter().flatten().map(|e| e + base).collect();
                let doc = m.conductors_source.doc.clone().unwrap_or_default();
                for _ in 0..per {
                    for mut w in wires_with(&c.pts, upf, &m.conductors, &phases, &voltage, rl, &opts) {
                        w.connects = ends.clone();
                        w.extra.push(("conductors_of_feeder", format!("{} per set", m.conductors), doc.clone()));
                        feeder_wire_ft += w.length_ft;
                        let mut wctx = ctx.clone();
                        wctx.src = Source::from_doc(m.conductors_source.kind, &format!("{doc} — {}", rl.feeder_conductors_source));
                        atoms.push((w, wctx));
                    }
                }
                feeder_chains += 1;
            }
            for &(_, end, ft) in gaps.iter().filter(|g| g.0 == ci) {
                if let Some(a) = pieces.ends.get(end).copied().flatten().and_then(|e| atoms.get_mut(e + base)) {
                    a.0.extra.push((
                        "entry_gap_ft",
                        format!("{ft:.2}"),
                        "computed: the route ends this far above or below the box it serves (no drop was routed to it)".into(),
                    ));
                }
                entry_gaps += 1;
            }
            for (end, land) in lands.iter().enumerate() {
                let Some(e) = pieces.ends.get(end).copied().flatten().map(|e| e + base) else { continue };
                match land {
                    Some(Land::Atom(j)) => {
                        if let Some(a) = atoms.get_mut(e) {
                            a.0.connects.push(*j);
                        }
                        if let Some(b) = atoms.get_mut(*j) {
                            b.0.connects.push(e);
                        }
                        if boxes.iter().any(|b| b.atom == *j) {
                            to_boxes += 1;
                        }
                    }
                    Some(Land::Gear(g)) => {
                        external.push((e, *g));
                        to_gear += 1;
                    }
                    None => open_ends += 1,
                }
            }
        }
    }

    for (base, ends, deg, cite) in bend_notes {
        for e in ends.iter().flatten() {
            if let Some(a) = atoms.get_mut(base + e) {
                a.0.extra.push(("bends_deg", deg.clone(), cite.clone()));
                a.0.extra.push(("review", format!("{deg}° of bends between this run's ends: a pull point is needed"), cite.clone()));
            }
        }
    }
    // -- Feeders where they enter their gear: each raceway's pieces and one set's conductors,
    // landing on the gear. --
    // A feeder routed gear to gear has its raceway into each gear in its route: no stub of its own.
    let routed_feeders: BTreeSet<usize> =
        sp.mep.runs.iter().take(MAX_RUNS).filter(|r| super::super::ugplan::is_feeder_run(r)).filter_map(|r| feeder_of_run(sp, r)).collect();
    let mut feeder_stubs = 0usize;
    for st in super::super::mep::feeder_stubs(sp, &d.bim_types, upf) {
        if atoms.len() >= MAX_ATOMS {
            break;
        }
        if routed_feeders.contains(&st.index) {
            continue;
        }
        let Some(f) = sp.mep.feeders.get(st.index) else { continue };
        let Some((rname, rw)) = rl.raceways.get_key_value(rl.default_raceway.as_str()) else { break };
        let (lo, hi) = (Vec3::new(st.at.x, st.at.y, st.z0), Vec3::new(st.at.x, st.at.y, st.z1));
        let path = if st.gear_end_low { vec![lo, hi] } else { vec![hi, lo] };
        let makeup = feeder_makeup(f);
        let pieces = raceway_pieces(&path, upf, st.trade_in, rname, rw, large_conductors(&makeup, rl.bushing_min_awg), rl);
        let base = atoms.len();
        let ctx = AtomCtx {
            level: st.level.clone(),
            system: st.feeder.clone(),
            circuit: String::new(),
            panel: f.from.clone(),
            run: format!("feeder:{}@{}", st.feeder, st.gear),
            estimated: false,
            source: Handle(0),
            body: None,
            src: Source::from_doc(
                SourceKind::Default,
                &format!(
                    "feeder {}: the schedule's {} raceway, {} where it names no material",
                    f.tag,
                    trade_size(f.conduit_in),
                    rl.default_raceway_source
                ),
            ),
        };
        for mut a in pieces.atoms {
            for x in &mut a.connects {
                *x += base;
            }
            atoms.push((a, ctx.clone()));
        }
        let phases: Vec<u32> = [1u32, 3, 5].into_iter().take(f.phases.clamp(1, 3) as usize).collect();
        let voltage = gear_voltage(sp, &f.to).unwrap_or_else(|| rl.default_voltage.clone());
        let ends: Vec<usize> = pieces.ends.iter().flatten().map(|e| e + base).collect();
        let material = f.material.trim().to_ascii_lowercase();
        let kind = rl.feeder_conductors.get(&material).or_else(|| rl.feeder_conductors.get("cu"));
        let m = rl.makeup_ft.max(0.0);
        let opts = WireOpts { kind: kind.map(|k| (k, rl.feeder_conductors_source.as_str())), makeup_ft: [m, m], makeup_source: &rl.makeup_source };
        for mut w in wires_with(&path, upf, &makeup, &phases, &voltage, rl, &opts) {
            w.connects = ends.clone();
            let mut wctx = ctx.clone();
            wctx.src = Source::from_doc(SourceKind::Print, &format!("feeder schedule {}: {makeup} per set", f.tag));
            atoms.push((w, wctx));
        }
        // The end at the gear lands on it.
        if let (Some(e), Some(g)) = (pieces.ends.first().copied().flatten(), gear.iter().find(|g| g.tag.eq_ignore_ascii_case(&st.gear))) {
            external.push((e + base, g.handle));
        }
        feeder_stubs += 1;
    }

    // -- Into the model: each atom an element; then what it connects to, by handle. --
    let mut handles: Vec<Option<Handle>> = Vec::with_capacity(atoms.len());
    let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (a, ctx) in &atoms {
        let mesh = match ctx.body {
            Some((c, along, w, t, h)) if !matches!(a.kind, "conduit" | "cable" | "wire") => {
                let mut m = super::super::wallgen::empty_mesh();
                block(&mut m, c, along, w, t, h);
                m
            }
            _ => body(a, upf),
        };
        if mesh.faces.is_empty() {
            handles.push(None);
            continue;
        }
        let system = if ctx.system.is_empty() { "ELECTRICAL BOXES".to_string() } else { ctx.system.clone() };
        let layer = super::super::mep::mep_layer_name(&system, &ctx.level);
        if d.layer(&layer).is_none() {
            d.layers.push(cadaddy_doc::Layer {
                name: layer.as_str().into(),
                color: cadaddy_doc::color::Color::Index(if a.kind == "box" || a.kind == "ring" { 40 } else { 6 }),
                ..cadaddy_doc::Layer::default()
            });
        }
        let xdata = mep_tags(&ctx.level, ctx.source, &system, a.kind, &a.size, &a.item, &format!("{}/{}{}", ctx.run, a.kind, handles.len()));
        match d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(mesh)) {
            Ok(h) => {
                *by_kind.entry(a.kind).or_default() += 1;
                handles.push(Some(h));
            }
            Err(_) => handles.push(None),
        }
    }
    let mut outside: HashMap<usize, Vec<Handle>> = HashMap::new();
    for (i, h) in &external {
        outside.entry(*i).or_default().push(*h);
    }
    // The devices' and the gear's side of each connection.
    let mut back: HashMap<Handle, Vec<Handle>> = HashMap::new();
    for (i, (a, ctx)) in atoms.iter().enumerate() {
        let Some(h) = handles.get(i).copied().flatten() else { continue };
        let mut cn: Vec<Handle> = a.connects.iter().filter_map(|j| handles.get(*j).copied().flatten()).collect();
        for x in outside.get(&i).into_iter().flatten() {
            cn.push(*x);
            back.entry(*x).or_default().push(h);
        }
        cn.sort_unstable();
        cn.dedup();
        let mid = (a.a + a.b) * 0.5;
        let room = rooms.at(&ctx.level, Vec2::new(mid.x, mid.y));
        let mut ps = Props::default();
        let computed = Source::computed;
        ps.set(super::ATOM_KEY, Prop::new(a.kind, computed()));
        ps.set("item", Prop::new(a.item.clone(), ctx.src.clone()));
        ps.set("type", Prop::new(a.ty.clone(), ctx.src.clone()));
        if !a.size.is_empty() {
            ps.set("size", Prop::new(a.size.clone(), ctx.src.clone()));
        }
        if matches!(a.kind, "conduit" | "cable" | "wire" | "offset") {
            ps.set(
                "length_ft",
                Prop::new(format!("{:.4}", a.length_ft).trim_end_matches('0').trim_end_matches('.').to_string(), computed()).with_unit("ft"),
            );
        }
        if a.kind == "conduit" {
            ps.set("cut", Prop::new(if a.cut { "true" } else { "false" }, computed()));
            ps.set("start", Prop::new(format!("{:.4},{:.4},{:.4}", a.a.x, a.a.y, a.a.z), computed()));
            ps.set("end", Prop::new(format!("{:.4},{:.4},{:.4}", a.b.x, a.b.y, a.b.z), computed()));
            ps.set("stock_ft", Prop::new(rl.raceways.get(&a.ty).map_or(0.0, |r| r.stock_ft), rule_source(&a.source)).with_unit("ft"));
        }
        if a.body_in > 0.0 {
            ps.set("body_in", Prop::new(format!("{}", a.body_in), computed()).with_unit("in"));
        }
        // Straight up or down (a drop in a wall's cavity, its fittings).
        let dir = a.b - a.a;
        if a.path.is_empty() && dir.len() > 1e-9 && dir.z.abs() > 0.9 * dir.len() {
            ps.set("vertical", Prop::new("true", computed()));
        }
        if a.angle_deg > 0.0 {
            ps.set("angle_deg", Prop::new(format!("{}", a.angle_deg), computed()).with_unit("deg"));
        }
        if !ctx.run.is_empty() {
            ps.set("run", Prop::new(ctx.run.clone(), computed()));
        }
        if !ctx.circuit.is_empty() {
            ps.set("circuit", Prop::new(ctx.circuit.clone(), computed()));
        }
        if !ctx.panel.is_empty() {
            ps.set("panel", Prop::new(ctx.panel.clone(), computed()));
        }
        ps.set("system", Prop::new(ctx.system.clone(), computed()));
        if !room.is_empty() {
            ps.set("room", Prop::new(room, computed()));
        }
        if ctx.estimated {
            let mut p = Prop::new("true", Source::from_doc(SourceKind::Default, super::super::printcond::ESTIMATED));
            p.note = Some("on a circuit's estimated route: placed by rule until a router lays it out".into());
            ps.set("estimated", p);
        }
        if !a.source.is_empty() {
            ps.set("source_rule", Prop::new(a.source.clone(), rule_source(&a.source)));
        }
        for (k, v, s) in &a.extra {
            ps.set(k, Prop::new(v.clone(), if s == "computed" || s.starts_with("computed") { computed() } else { rule_source(s) }));
        }
        if !cn.is_empty() {
            ps.set("connects", Prop::new(cn.iter().map(|h| h.hex()).collect::<Vec<_>>().join(","), computed()));
        }
        let _ = d.modify_entity(h, |e| {
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    // Devices: their box and what feeds it; gear: what lands on it and the feeders between.
    let box_of_device: HashMap<Handle, Handle> = boxes.iter().filter_map(|b| Some((b.device, handles.get(b.atom).copied().flatten()?))).collect();
    let feeds_box = |bx: Handle| -> Vec<Handle> {
        let Some(e) = d.model.get(bx) else { return Vec::new() };
        super::connects_of(e)
            .into_iter()
            .filter(|h| {
                d.model.get(*h).is_some_and(|n| {
                    let k = props::read(&n.common.xdata).get(super::ATOM_KEY).map(Prop::text).unwrap_or_default();
                    matches!(k.as_str(), "connector" | "cable")
                })
            })
            .collect()
    };
    let mut device_links: Vec<(Handle, Handle, Vec<Handle>)> = Vec::new();
    for (dev, bx) in &box_of_device {
        device_links.push((*dev, *bx, feeds_box(*bx)));
    }
    let mut fed_devices = 0usize;
    for (dev, bx, fed) in device_links {
        fed_devices += usize::from(!fed.is_empty());
        let _ = d.modify_entity(dev, |e| {
            let mut ps = props::read(&e.common.xdata);
            ps.set("box", Prop::new(bx.hex(), Source::computed()).with_note("the box it is set in"));
            ps.set("connects", Prop::new(bx.hex(), Source::computed()));
            if fed.is_empty() {
                ps.remove("fed_by");
            } else {
                ps.set(
                    "fed_by",
                    Prop::new(fed.iter().map(|h| h.hex()).collect::<Vec<_>>().join(","), Source::computed())
                        .with_note("the raceway or cable ends at its box"),
                );
            }
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    // Feeders between gear: the upstream end is what feeds it.
    let tag_of: HashMap<String, Handle> = gear.iter().filter(|g| !g.tag.is_empty()).map(|g| (g.tag.to_ascii_uppercase(), g.handle)).collect();
    let mut feeders = 0usize;
    let mut fed_from: HashMap<Handle, Handle> = HashMap::new();
    for f in sp.mep.feeders.iter().take(MAX_DEVICES) {
        let (Some(a), Some(b)) = (tag_of.get(&f.from.trim().to_ascii_uppercase()), tag_of.get(&f.to.trim().to_ascii_uppercase())) else { continue };
        if a == b {
            continue;
        }
        fed_from.insert(*b, *a);
        back.entry(*a).or_default().push(*b);
        back.entry(*b).or_default().push(*a);
        feeders += 1;
    }
    for (g, hs) in &back {
        let up = fed_from.get(g).copied();
        let feeder = up.and_then(|u| {
            sp.mep
                .feeders
                .iter()
                .find(|f| tag_of.get(&f.to.trim().to_ascii_uppercase()) == Some(g) && tag_of.get(&f.from.trim().to_ascii_uppercase()) == Some(&u))
        });
        let _ = d.modify_entity(*g, |e| {
            let mut ps = props::read(&e.common.xdata);
            let mut all: Vec<Handle> = hs.clone();
            all.sort_unstable();
            all.dedup();
            ps.set("connects", Prop::new(all.iter().take(2000).map(|h| h.hex()).collect::<Vec<_>>().join(","), Source::computed()));
            if let Some(u) = up {
                let mut p = Prop::new(u.hex(), Source::from_doc(SourceKind::Print, "the feeder schedule"));
                if let Some(f) = feeder {
                    p.note = Some(format!("feeder {}: {} to {}", f.tag, f.from, f.to));
                }
                ps.set("fed_from", p);
            }
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    let rows = super::reindex(d);
    let total: usize = by_kind.values().sum();
    if open_ends > 0 {
        notes.push(format!("{open_ends} run end(s) land in no box or panel (an open end): see bim.chain"));
    }
    json!({
        "atoms": total,
        "atomsByKind": by_kind,
        "atomChains": chains_n,
        "junctionBoxes": junctions,
        "deviceBoxes": boxes.len(),
        "devicesFed": fed_devices,
        "endsAtBoxes": to_boxes,
        "endsAtPanels": to_gear,
        "openEnds": open_ends,
        "endsWithNoDrop": entry_gaps,
        "runsOverBendLimit": over_bends,
        "landingFt": (landing_ft * 10.0).round() / 10.0,
        "feedersLinked": feeders,
        "feederRaceways": feeder_stubs,
        "feederRunsWired": feeder_chains,
        "luminaireBoxes": lum_boxes
            .iter()
            .take(500)
            .map(|(m, (with, without, why))| json!({"type": m, "boxes": with, "no_box": without, "why": why.iter().take(4).collect::<Vec<_>>()}))
            .collect::<Vec<_>>(),
        "feederWireFt": (feeder_wire_ft * 10.0).round() / 10.0,
        "atomIndexRows": rows,
        "atomsMs": (started.elapsed().as_secs_f64() * 1000.0).round(),
        "atomNotes": notes,
    })
}

/// The plaster rings as their devices' walls need them (`mud_ring`, which `bim.props.fill`
/// reads from the partition legend after the build, or a person sets): each ring's depth and
/// size from its box's device. How many changed.
pub(crate) fn sync_rings(d: &mut Drawing) -> usize {
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    for e in d.model.iter().take(super::MAX_ELEMENTS) {
        let ps = props::read(&e.common.xdata);
        if ps.get(super::ATOM_KEY).map(Prop::text).as_deref() != Some("ring") {
            continue;
        }
        let device = super::connects_of(e)
            .into_iter()
            .filter_map(|b| d.model.get(b))
            .find_map(|b| props::read(&b.common.xdata).get("box_of").map(Prop::text).and_then(|h| Handle::parse_hex(h.trim())))
            .and_then(|h| d.model.get(h));
        let Some(dev) = device else { continue };
        let dps = props::read(&dev.common.xdata);
        let Some(ring) = dps.get("mud_ring") else { continue };
        let Some(depth) = ring.text().trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v < 6.0) else { continue };
        let now = ps.get("depth_in").map(Prop::text).and_then(|t| t.parse::<f64>().ok());
        if now.is_some_and(|v| (v - depth).abs() < 1e-6) {
            continue;
        }
        let rl = rules();
        let mut ps2 = ps.clone();
        ps2.set("depth_in", Prop::new(format!("{depth}"), ring.source.clone()).with_unit("in"));
        ps2.set("size", Prop::new(format!("{}, {} deep", rl.ring.size, super::super::walltypes::frac(depth)), ring.source.clone()));
        changes.push((e.handle, ps2));
    }
    let n = changes.len();
    for (h, ps) in changes {
        let _ = d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &ps).into());
    }
    n
}

/// What a run end lands in.
enum Land {
    /// An atom of this build (a box), by index.
    Atom(usize),
    /// A gear element.
    Gear(Handle),
}

/// Where an atom is in the model and what it is part of.
#[derive(Clone)]
struct AtomCtx {
    level: String,
    system: String,
    circuit: String,
    panel: String,
    run: String,
    estimated: bool,
    /// The plan object (or device) it was made for.
    source: Handle,
    /// A box's body: centre, the way along its face, width, depth across, height.
    body: Option<(Vec3, Vec2, f64, f64, f64)>,
    /// Where what it is came from.
    src: Source,
}

/// Move a run's end onto a box's entry: a short jog at the run's height, then into the box.
fn land_on(pts: &mut Vec<Vec3>, end: usize, entry: Vec3, upf: f64) {
    if end == 0 {
        pts.reverse();
    }
    let tiny = upf / 192.0;
    let n = pts.len();
    if let (Some(&last), Some(&prev)) = (pts.get(n.wrapping_sub(1)), pts.get(n.wrapping_sub(2))) {
        let vertical = Vec2::new(last.x - prev.x, last.y - prev.y).len() <= tiny;
        let plan_off = Vec2::new(entry.x - last.x, entry.y - last.y).len();
        if vertical {
            pts.pop();
            if plan_off > tiny {
                pts.push(Vec3::new(entry.x, entry.y, prev.z));
            }
            pts.push(entry);
        } else {
            if plan_off > tiny {
                pts.push(Vec3::new(entry.x, entry.y, last.z));
            }
            if (entry.z - last.z).abs() > tiny {
                pts.push(entry);
            }
        }
    }
    if end == 0 {
        pts.reverse();
    }
}

/// Whether a luminaire takes an outlet box, from what its properties say (its schedule's and its
/// submittal's words; rules `luminaire_box`), and its type's fixture schedule row where its
/// properties don't carry the row yet.
pub(crate) fn luminaire_box(ps: &Props, schedule: &[cadaddy_walk3d::FixtureType]) -> super::BoxDecision {
    let lb = &rules().luminaire_box;
    let mut said: Vec<(String, String, String)> =
        lb.fields.iter().filter_map(|k| ps.get(k).map(|p| (k.clone(), p.text(), p.source.doc.clone().unwrap_or_default()))).collect();
    let mark = ps.get("type_mark").map(Prop::text).unwrap_or_default();
    if let Some(row) = schedule.iter().find(|r| !mark.trim().is_empty() && r.tag.trim().eq_ignore_ascii_case(mark.trim())) {
        let doc = format!("{} type {}", row.source, row.tag);
        for (k, v) in [("installation", &row.installation), ("fixture_description", &row.description)] {
            if !v.trim().is_empty() && !said.iter().any(|s| s.0 == k) {
                said.push((k.to_string(), v.clone(), doc.clone()));
            }
        }
    }
    lb.decide(&said)
}

/// The luminaires' boxes as their types need them now (a submittal applied since the build may
/// say a type has its own junction box): a box its luminaire no longer needs goes, and what ran
/// into it lands on the luminaire. How many went, and how many luminaires need a box they have
/// not got (the next build or `conduit.reroute` makes them).
pub(crate) fn sync_luminaire_boxes(d: &mut Drawing) -> (usize, usize) {
    let schedule = d.building_spec.as_deref().and_then(|j| BuildingSpec::from_json(j).ok()).map(|sp| sp.mep.fixture_schedule).unwrap_or_default();
    let mut gone: HashMap<Handle, Handle> = HashMap::new();
    let mut boxed: BTreeSet<Handle> = BTreeSet::new();
    for e in d.model.iter().take(super::MAX_ELEMENTS) {
        let ps = props::read(&e.common.xdata);
        if ps.get(super::ATOM_KEY).map(Prop::text).as_deref() != Some("box") {
            continue;
        }
        let Some(dev) = ps.get("box_of").and_then(|p| Handle::parse_hex(p.text().trim())) else { continue };
        let Some(de) = d.model.get(dev) else { continue };
        let dps = props::read(&de.common.xdata);
        let kind = dps.get("kind").map(Prop::text).unwrap_or_default();
        if box_kind(&kind, &de.common.layer) != Some("luminaire") {
            continue;
        }
        boxed.insert(dev);
        if !luminaire_box(&dps, &schedule).takes_box {
            gone.insert(e.handle, dev);
        }
    }
    // Luminaires that need a box and have none (one a person or the scope left out aside).
    let missing = d
        .model
        .iter()
        .take(super::MAX_ELEMENTS)
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("device") && !boxed.contains(&e.handle))
        .filter(|e| {
            let ps = props::read(&e.common.xdata);
            let kind = ps.get("kind").map(Prop::text).unwrap_or_default();
            box_kind(&kind, &e.common.layer) == Some("luminaire")
                && ps.get("misplaced").is_none()
                && ps.get("junction_box").is_some()
                && luminaire_box(&ps, &schedule).takes_box
        })
        .count();
    if gone.is_empty() {
        return (0, missing);
    }
    // Whatever names a box that goes names its luminaire instead; the luminaire loses its box.
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    for e in d.model.iter().take(super::MAX_ELEMENTS) {
        if gone.contains_key(&e.handle) {
            continue;
        }
        let ps = props::read(&e.common.xdata);
        let own_box = ps.get("box").and_then(|p| Handle::parse_hex(p.text().trim())).filter(|b| gone.contains_key(b));
        let cn = super::connects_of(e);
        if own_box.is_none() && !cn.iter().any(|h| gone.contains_key(h)) {
            continue;
        }
        let mut ps2 = ps.clone();
        if own_box.is_some() {
            ps2.remove("box");
            let fed = ps.get("fed_by").map(Prop::text).unwrap_or_default();
            if fed.trim().is_empty() {
                ps2.remove("connects");
            } else {
                ps2.set("connects", Prop::new(fed, Source::computed()));
            }
        } else {
            let mut now: Vec<Handle> = cn.iter().map(|h| gone.get(h).copied().unwrap_or(*h)).collect();
            now.dedup();
            ps2.set("connects", Prop::new(now.iter().map(|h| h.hex()).collect::<Vec<_>>().join(","), Source::computed()));
        }
        changes.push((e.handle, ps2));
    }
    for (h, ps) in changes {
        let _ = d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &ps).into());
    }
    let n = gone.len();
    for b in gone.into_keys() {
        d.remove_entity(b);
    }
    super::reindex(d);
    (n, missing)
}

/// Why a luminaire takes a box or none, in words: the words found and where.
fn why_box(dec: &super::BoxDecision) -> String {
    let yes = if dec.takes_box { "box" } else { "no box" };
    if dec.found.is_empty() {
        return format!("{yes}: {}", dec.source);
    }
    let doc = if dec.doc.is_empty() { String::new() } else { format!(", {}", dec.doc) };
    format!("{yes}: {} \"{}\"{doc}", dec.field, dec.found)
}

/// The feeder a feeder's run is routed for (`mep.feeders` index): its source names the feeder's
/// tag and source (`feeder F7 PA→U-101→U-201`), and the gear it goes to first is the feeder's
/// end, or a copy of it (`U-101` of a typical `U`). Feeders alike in all three are one type: the
/// first.
pub(crate) fn feeder_of_run(sp: &BuildingSpec, r: &MepRun) -> Option<usize> {
    let rest = r.source_sheet.strip_prefix("feeder ")?;
    sp.mep.feeders.iter().take(MAX_RUNS).position(|f| {
        rest.strip_prefix(&format!("{} {}→", f.tag, f.from)).is_some_and(|to| {
            let first = to.split('→').next().unwrap_or_default();
            !f.to.trim().is_empty() && feeder_lands_on(f, first)
        })
    })
}

/// Whether a piece of gear is one a feeder lands on: its source, its end, or a copy of its end.
pub(crate) fn feeder_lands_on(f: &cadaddy_walk3d::Feeder, tag: &str) -> bool {
    let (t, to) = (tag.trim().to_ascii_uppercase(), f.to.trim().to_ascii_uppercase());
    t == f.from.trim().to_ascii_uppercase() || t == to || (!to.is_empty() && t.strip_prefix(&to).is_some_and(|x| x.starts_with('-')))
}

/// A cable's or wire's makeup at the panels its run ends in (`ends`: none, one or both): each
/// end a panel's makeup in place of a box's.
fn panel_makeup(a: &mut Atom, ends: usize, rl: &super::AtomRules) {
    let more = (rl.panel_makeup_ft - rl.makeup_ft.max(0.0)).max(0.0) * ends.min(2) as f64;
    if more > 0.0 && more.is_finite() {
        a.length_ft += more;
        a.extra.push(("panel_makeup_ft", format!("{}", rl.panel_makeup_ft), rl.panel_makeup_source.clone()));
    }
}

/// One set of a feeder's conductors as a makeup (`3#1 + 1#1 + 1#6G`).
pub(crate) fn feeder_makeup(f: &cadaddy_walk3d::Feeder) -> String {
    let size = |s: &str| -> String {
        let t = s.trim().trim_start_matches('#').to_ascii_lowercase();
        t.split_whitespace().next().unwrap_or_default().to_string()
    };
    let phase = size(&f.size);
    let mut parts = vec![format!("{}#{}", f.phases.clamp(1, 4), if phase.is_empty() { "12".into() } else { phase.clone() })];
    if f.neutrals > 0 {
        let n = size(&f.neutral_size);
        parts.push(format!("{}#{}", f.neutrals.min(4), if n.is_empty() { phase.clone() } else { n }));
    }
    if f.grounds > 0 {
        let g = size(&f.ground_size);
        parts.push(format!("{}#{}G", f.grounds.min(4), if g.is_empty() { phase } else { g }));
    }
    parts.join(" + ")
}

/// A panel's voltage from its schedule row's properties (`VOLTS`, `VOLTAGE`), as the colour
/// table names it.
pub(crate) fn gear_voltage(sp: &BuildingSpec, panel: &str) -> Option<String> {
    let e = sp.mep.equipment.iter().find(|e| !panel.is_empty() && e.tag.trim().eq_ignore_ascii_case(panel.trim()))?;
    let v = e.properties.iter().find(|(k, _)| k.to_ascii_uppercase().contains("VOLT")).map(|(_, v)| v.to_ascii_uppercase())?;
    if v.contains("480") || v.contains("277") {
        Some("277/480".into())
    } else if v.contains("240") {
        Some("120/240".into())
    } else if v.contains("208") {
        Some("120/208".into())
    } else {
        None
    }
}
