//! Field paperwork from the model's pieces (Q6195): what the crew pulls, cuts, bends and orders,
//! read from the electrical runs the build cut into their pieces ([`super::mepatoms`]), so every
//! total is the takeoff's.
//!
//! - `pull.schedule`: the **wire pull schedule**. Per branch circuit and per feeder, each raceway
//!   path from its source to its box (or gear to gear): its conductors (count, size, insulation,
//!   colour and phase as the model's wires carry them; a feeder's from its schedule), the path's
//!   length with its vertical drops, its bends, the pull points (its ends, and a pull box in the
//!   straight before any bend that would pass the code's limit between pull points: NEC 358.26
//!   and its kin, from the raceway's rule), each pull's length and bends, and each conductor's cut
//!   length with the slack the rules data leaves at a box end, a gear end and in a pull box. Then
//!   the **reel plan**: per floor, wire size, insulation and colour, the cuts packed onto standard
//!   reels (`pulllist.json`) with the least waste. The wire it adds up to is reconciled with the
//!   takeoff's, term by term.
//! - `pull.cutlist`: the **conduit cut and bend list**. Per run its full sticks, its cut pieces,
//!   couplings, connectors, elbows by angle, offsets, field bends, straps, locknuts and bushings;
//!   then the totals by size and floor, each checked against the conduit takeoff and the model's
//!   count index (they must match exactly).
//! - `pull.release`: the **release lists**. The material per floor (or room) and phase (rough-in:
//!   raceway, boxes, rings, wire; trim: devices and their plates), each line with its price book
//!   line ([`super::pricebook`]: the Item Map's keys, the chosen quote) or why it has none.
//! - `pull.sheets`: all of it per floor as CSV files and printable paper space sheets (and PDF).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cadaddy_doc::props::{self, Prop};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Space, Text};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::BuildingSpec;
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD_PULLS: &str = "pull.schedule";
const CMD_CUTS: &str = "pull.cutlist";
const CMD_RELEASE: &str = "pull.release";
const CMD_SHEETS: &str = "pull.sheets";
/// The rules data.
const RULES: &str = include_str!("pulllist.json");
/// Elements looked at, pieces kept, pieces walked in one run (hostile drawings).
const MAX_ELEMENTS: usize = 4_000_000;
const MAX_PIECES: usize = 2_000_000;
const MAX_WALK: usize = 50_000;
/// Spec runs looked at for the feeders' routes.
const MAX_SPEC_RUNS: usize = 500_000;
/// Rows a call lists by default, and at most.
const DEFAULT_MAX: usize = 200;
const MAX_LISTED: usize = 100_000;
/// Cuts packed exactly (every way of grouping them tried); more are packed by best fit.
const EXACT_CUTS: usize = 9;
/// Cuts one reel plan group holds, at most.
const MAX_CUTS: usize = 200_000;
/// Sheets one floor's paperwork fills, at most; rows a sheet holds.
const MAX_PAGES: usize = 200;
/// Paper (inches): ARCH D, its margins, the text and the row pitch.
const PAPER: [f64; 2] = [36.0, 24.0];
const MARGIN: f64 = 0.75;
const STRIP: f64 = 2.25;
const TEXT_H: f64 = 0.1;
const ROW: f64 = 0.15;
/// The width a character of the sheets' text takes, inches.
const CHAR_W: f64 = 0.085;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_PULLS, "Wire Pull Schedule", run_schedule)
            .menu(&["Draw", "BIM", "Wire Pull Schedule"])
            .params("{level?, circuit?: part of a circuit or feeder (LP1-3, F1), max?: rows listed (200), file?: .csv|.xlsx (every pull), reels_file?: .csv|.xlsx (the reel plan), force?: bool (replace an existing file)} → {pulls: [{kind: branch|feeder, circuit | feeder, level, panel, runs, from, to, raceway, raceway_ft, vertical_ft, bends_deg, max_bends_deg, pull_boxes: [{station_ft, at, size, source}], pulls: [{from, to, ft, bends_deg}], conductors: [{size, type, color, role, count, cut_ft}], conduits, conductor_ft, takeoff_ft, slack: {box_end_ft, gear_end_ft, pull_box_ft}, estimated, status}], circuits: [{circuit, level, panel, paths, raceway_ft, conductor_ft, pull_boxes}], reels: [{level, size, type, color, kind, cuts, cut_ft, reels: [{reel, length_ft, used_ft, left_ft, cuts: [{id, ft}]}], reel_ft, waste_ft, source, method: exact|best_fit, cut_to_length}], wire: {by_size: [{size, takeoff_ft, pulled_ft, …}], takeoff_ft, pulled_ft, terms: {not_in_pulls_ft, gear_end_slack_ft, pull_box_loops_ft, feeder_conductors_ft}, matches}, totals: {pulls, circuits, feeders, raceway_ft, vertical_ft, conductor_ft, pull_boxes, reels, reel_ft, waste_ft}, unrouted_feeders, empty_raceways, rules, paths?}: from the model's pieces (model.build)")
            .noundo(),
        CommandSpec::new(CMD_CUTS, "Conduit Cut and Bend List", run_cutlist)
            .menu(&["Draw", "BIM", "Conduit Cut and Bend List"])
            .params("{level?, circuit?, max?: runs listed (200), file?: .csv|.xlsx (every run), force?: bool} → {runs: [{run, level, circuit, system, raceway, stock_ft, raceway_ft, full_sticks, cut_pieces: [ft], couplings, connectors, elbows: {angle: n}, offsets, field_bends: {angle: n}, field_offsets, straps, locknuts, bushings, bends_deg}], by_size_floor: [{level, kind, item, size, unit, qty}], takeoff_check: {conduit_takeoff: {matches, compared, diffs}, model_index: {matches, compared, diffs}}, totals: {runs, sticks, cut_pieces, raceway_ft}}: per run from the model's pieces; the totals equal the takeoff's")
            .noundo(),
        CommandSpec::new(CMD_RELEASE, "Material Release List", run_release)
            .menu(&["Draw", "BIM", "Material Release List"])
            .params("{level?, by?: level|room (default level: per floor; room: per floor and room), phase?: rough-in|trim, price_folder?: the local price database's folder (pricebook), max?: lines listed (200), file?: .csv|.xlsx, force?: bool} → {lines: [{level, area, phase, category, item, size, unit, qty, price_keys, unit_price, extended, price_date, price_source | unpriced}], totals: [{level, area?, phase, lines, priced, unpriced, extended}], unpriced: n, extended, phases_source}: the model's pieces and devices to order, per floor (or room) and phase, each with its price book line")
            .noundo(),
        CommandSpec::new(CMD_SHEETS, "Field Paperwork Sheets", run_sheets)
            .menu(&["Draw", "BIM", "Field Paperwork Sheets"])
            .params("{level?, folder?: the per-floor CSV files go here (pulls-, reels-, cuts-, release-<floor>.csv), pdf?: bool (one PDF per floor in `folder`), price_folder?, title?, force?: bool (replace existing files)} → {floors: [{level, layouts, rows, files, pdf?}]}: one or more paper space sheets per floor holding its pull schedule, reel plan, cut and bend list and release list"),
    ]
}

// ─── Rules ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct PullBoxRule {
    sized_min_awg: u32,
    straight_times: f64,
    straight_source: String,
    small: Vec<(f64, String)>,
    small_source: String,
    large_source: String,
    #[serde(rename = "where")]
    place: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ReelRule {
    words: Vec<String>,
    kind: String,
    lengths_ft: Vec<f64>,
    source: String,
}

/// The rules data (`pulllist.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    box_end_slack_source: String,
    gear_end_slack_ft: f64,
    gear_end_slack_source: String,
    pull_box_loop_ft: f64,
    pull_box_loop_source: String,
    pull_box: PullBoxRule,
    reels: Vec<ReelRule>,
    reel_default_ft: Vec<f64>,
    reel_default_source: String,
    cut_to_length_source: String,
    phases: BTreeMap<String, Vec<String>>,
    phases_source: String,
    plate_kinds: Vec<String>,
    plate_item: String,
    plate_size: String,
    plate_source: String,
}

fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A finite, non-negative number, else 0.
fn pos(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

impl Rules {
    /// The extra each of the model's conductors gets at a gear end over the panel makeup the
    /// takeoff already counts there (the atoms' rule).
    fn gear_extra(&self) -> f64 {
        (pos(self.gear_end_slack_ft) - panel_makeup()).max(0.0)
    }
    /// The extra a conductor this schedule cuts itself (a feeder's, along its route) gets at a
    /// gear end over the box makeup its length carries.
    fn computed_gear_extra(&self) -> f64 {
        (pos(self.gear_end_slack_ft) - makeup()).max(0.0)
    }
    /// The standard lengths a size (`#12`, `250 kcmil`) or a cable type (`NM 12/2 w/G`) comes
    /// in, smallest first, and their source.
    fn reels_for(&self, kind: &str, size_or_type: &str) -> (Vec<f64>, String) {
        let up = size_or_type.trim().to_ascii_uppercase();
        let first = up.split_whitespace().next().unwrap_or_default().to_string();
        let hit = self.reels.iter().filter(|r| r.kind == kind).find(|r| {
            r.words.iter().any(|w| {
                let w = w.to_ascii_uppercase();
                if kind == "cable" { first == w } else { up == w || (w == "KCMIL" && up.contains("KCMIL")) }
            })
        });
        let (mut v, src) = match hit {
            Some(r) => (r.lengths_ft.clone(), r.source.clone()),
            None => (self.reel_default_ft.clone(), self.reel_default_source.clone()),
        };
        v.retain(|x| x.is_finite() && *x > 0.0);
        v.sort_by(f64::total_cmp);
        v.dedup();
        if v.is_empty() {
            v.push(1000.0);
        }
        (v, src)
    }
    /// The phase a kind of piece goes out in.
    fn phase_of(&self, kind: &str) -> String {
        self.phases.iter().find(|(_, ks)| ks.iter().any(|k| k == kind)).map_or_else(|| "rough-in".to_string(), |(p, _)| p.clone())
    }
}

/// The takeoff's makeup at each end of a wire or cable (the atoms' rule).
fn makeup() -> f64 {
    pos(super::mepatoms::rules().makeup_ft)
}

/// The takeoff's makeup where a wire or cable ends at a panel (the atoms' rule, at least a box's).
fn panel_makeup() -> f64 {
    pos(super::mepatoms::rules().panel_makeup_ft).max(makeup())
}

// ─── The model's pieces ──────────────────────────────────────────────────────

/// One piece of a run as the model holds it.
#[derive(Clone, Debug, Default)]
struct Piece {
    handle: Handle,
    kind: String,
    item: String,
    size: String,
    ty: String,
    level: String,
    room: String,
    circuit: String,
    panel: String,
    system: String,
    run: String,
    length_ft: f64,
    angle_deg: f64,
    turn_deg: Option<f64>,
    run_ft: Option<f64>,
    cut: bool,
    start: Option<Vec3>,
    end: Option<Vec3>,
    /// Its middle (its body's), sheet units.
    at: Vec3,
    connects: Vec<Handle>,
    color: String,
    conductor: String,
    estimated: bool,
    box_of: Option<Handle>,
    junction: bool,
}

/// A device element: what the trim releases.
#[derive(Clone, Debug, Default)]
struct Device {
    level: String,
    room: String,
    kind: String,
    mark: String,
}

/// The model's electrical pieces and devices.
#[derive(Default)]
struct Scan {
    pieces: Vec<Piece>,
    at: HashMap<Handle, usize>,
    devices: Vec<Device>,
    device_kind: HashMap<Handle, String>,
}

fn point(t: &str) -> Option<Vec3> {
    let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).filter(|x| x.is_finite()).collect();
    match v[..] {
        [x, y, z] => Some(Vec3::new(x, y, z)),
        _ => None,
    }
}

/// The middle of an element's body (its mesh's vertices).
fn middle(e: &Entity) -> Vec3 {
    let EntityKind::Mesh(m) = &e.kind else { return Vec3::ZERO };
    let pts: Vec<Vec3> = m.vertices.iter().take(256).copied().filter(|p| p.is_finite()).collect();
    if pts.is_empty() {
        return Vec3::ZERO;
    }
    pts.iter().fold(Vec3::ZERO, |a, p| a + *p) * (1.0 / pts.len() as f64)
}

fn scan(d: &Drawing) -> Scan {
    let mut sc = Scan::default();
    for e in d.model.iter().filter(|e| !e.common.xdata.is_empty()).take(MAX_ELEMENTS) {
        if super::mepatoms::is_atom(e) {
            if super::tradeatoms::is_trade_piece(e) || sc.pieces.len() >= MAX_PIECES {
                continue;
            }
            let ps = props::read(&e.common.xdata);
            let text = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
            let num = |k: &str| ps.get(k).map(Prop::text).and_then(|t| t.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
            let kind = text(super::mepatoms::ATOM_KEY);
            if kind.is_empty() {
                continue;
            }
            let p = Piece {
                handle: e.handle,
                item: text("item"),
                size: text("size"),
                ty: text("type"),
                level: super::bspec::made_level(e).unwrap_or_default(),
                room: text("room"),
                circuit: text("circuit"),
                panel: text("panel"),
                system: text("system"),
                run: text("run"),
                length_ft: pos(num("length_ft").unwrap_or(0.0)),
                angle_deg: pos(num("angle_deg").unwrap_or(0.0)),
                turn_deg: num("turn_deg").map(pos),
                run_ft: num("run_ft").map(pos),
                cut: text("cut") == "true",
                start: point(&text("start")),
                end: point(&text("end")),
                at: if kind == "conduit" || kind == "wire" { Vec3::ZERO } else { middle(e) },
                connects: super::mepatoms::connects_of(e),
                color: text("color"),
                conductor: text("conductor"),
                estimated: text("estimated") == "true",
                box_of: Handle::parse_hex(&text("box_of")),
                junction: ps.get("junction").is_some(),
                kind,
            };
            sc.at.insert(p.handle, sc.pieces.len());
            sc.pieces.push(p);
        } else if super::wallgen::made_as(e).is_some_and(|m| m.eq_ignore_ascii_case("device")) {
            let ps = props::read(&e.common.xdata);
            let text = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
            let kind = Some(text("kind")).filter(|k| !k.trim().is_empty()).unwrap_or_else(|| "device".into());
            sc.device_kind.insert(e.handle, kind.replace('_', " "));
            sc.devices.push(Device {
                level: super::bspec::made_level(e).unwrap_or_default(),
                room: text("room"),
                kind: kind.replace(' ', "_").to_ascii_lowercase(),
                mark: text("type_mark"),
            });
        }
    }
    sc
}

// ─── One run ─────────────────────────────────────────────────────────────────

/// A run's end: what it lands on.
#[derive(Clone, Debug, Default)]
struct End {
    label: String,
    gear: bool,
}

/// A bend along a run: where (feet along it), how many degrees, and its place (sheet units).
#[derive(Clone, Copy, Debug, Default)]
struct Bend {
    station_ft: f64,
    deg: f64,
    at: Vec3,
}

/// A conductor as the model's wire carries it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Wire {
    size: String,
    ty: String,
    item: String,
    color: String,
    role: String,
    length_ft: f64,
}

/// One run of the model (a raceway chain or a cable) read for the paperwork.
#[derive(Clone, Debug, Default)]
struct RunPlan {
    run: String,
    level: String,
    system: String,
    circuit: String,
    panel: String,
    estimated: bool,
    cable: bool,
    /// The raceway's material (`EMT`) or the cable's type (`NM 12/2 w/G`), and its size.
    material: String,
    size: String,
    stock_ft: f64,
    from: End,
    to: End,
    raceway_ft: f64,
    vertical_ft: f64,
    bends: Vec<Bend>,
    max_bends: f64,
    max_source: String,
    full_sticks: u32,
    cut_pieces: Vec<f64>,
    /// Fittings by kind, item and size: how many.
    fittings: BTreeMap<(String, String, String), u32>,
    wires: Vec<Wire>,
    /// A cable run: the cable's length as the takeoff counts it.
    cable_ft: f64,
}

impl RunPlan {
    fn bends_deg(&self) -> f64 {
        self.bends.iter().map(|b| b.deg).sum()
    }
    fn raceway_text(&self) -> String {
        if self.cable { self.material.clone() } else { format!("{} {}", self.size, self.material).trim().to_string() }
    }
    fn count(&self, kind: &str) -> u32 {
        self.fittings.iter().filter(|((k, _, _), _)| k == kind).map(|(_, n)| *n).sum()
    }
    /// The fittings of a kind by their angle (`90`: 3).
    fn by_angle(&self, kind: &str, labor: bool) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        for ((k, item, _), n) in &self.fittings {
            if k == kind && super::mepatoms::is_labor_item(item) == labor {
                let a: String = item.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '/' || *c == '-').collect();
                *out.entry(if a.is_empty() { item.clone() } else { a }).or_insert(0) += n;
            }
        }
        out
    }
}

/// Raceway pieces walked in their order along a run.
fn in_chain(p: &Piece) -> bool {
    matches!(p.kind.as_str(), "conduit" | "coupling" | "elbow" | "connector") || (p.kind == "offset" && !super::mepatoms::is_labor_item(&p.item))
}

/// What a run's end piece lands on (outside its run).
fn landing(d: &Drawing, sc: &Scan, p: &Piece, own: &HashSet<Handle>) -> Option<End> {
    for h in p.connects.iter().filter(|h| !own.contains(h)) {
        if let Some(q) = sc.at.get(h).and_then(|i| sc.pieces.get(*i)) {
            if q.kind != "box" {
                continue;
            }
            if q.junction {
                return Some(End { label: "junction box".into(), gear: false });
            }
            let dev = q.box_of.and_then(|b| sc.device_kind.get(&b)).cloned().unwrap_or_else(|| "device".into());
            return Some(End { label: format!("{dev} box"), gear: false });
        }
        if let Some(e) = d.model.get(*h) {
            if let Some(tag) = super::bimlook::made_tag(e, "tag").filter(|t| !t.is_empty()) {
                return Some(End { label: tag.to_string(), gear: true });
            }
            if let Some(k) = sc.device_kind.get(h) {
                return Some(End { label: k.clone(), gear: false });
            }
        }
    }
    None
}

fn dist(a: Vec3, b: Vec3) -> f64 {
    (a - b).len()
}

fn plan_run(d: &Drawing, sc: &Scan, run: &str, idx: &[usize], upf: f64) -> RunPlan {
    let ps: Vec<&Piece> = idx.iter().filter_map(|i| sc.pieces.get(*i)).collect();
    let own: HashSet<Handle> = ps.iter().map(|p| p.handle).collect();
    let first = ps.first().copied().cloned().unwrap_or_default();
    let mut rp = RunPlan {
        run: run.to_string(),
        level: first.level.clone(),
        system: first.system.clone(),
        circuit: first.circuit.clone(),
        panel: first.panel.clone(),
        estimated: ps.iter().any(|p| p.estimated),
        ..RunPlan::default()
    };
    let ar = super::mepatoms::rules();
    for p in &ps {
        match p.kind.as_str() {
            "conduit" => {
                if rp.material.is_empty() {
                    rp.material = p.ty.clone();
                    rp.size = p.size.clone();
                }
                rp.raceway_ft += p.length_ft;
                if let (Some(a), Some(b)) = (p.start, p.end) {
                    rp.vertical_ft += (b.z - a.z).abs() / upf;
                }
                if p.cut {
                    rp.cut_pieces.push(p.length_ft);
                } else {
                    rp.full_sticks += 1;
                }
            }
            "wire" => rp.wires.push(Wire {
                size: p.size.clone(),
                ty: p.ty.clone(),
                item: p.item.clone(),
                color: p.color.clone(),
                role: p.conductor.clone(),
                length_ft: p.length_ft,
            }),
            "cable" => {
                rp.cable = true;
                rp.material = p.ty.clone();
                rp.size = p.size.clone();
                rp.cable_ft += p.length_ft;
                rp.raceway_ft += p.run_ft.unwrap_or_else(|| (p.length_ft - 2.0 * makeup()).max(0.0));
            }
            "box" | "ring" => {}
            _ => {
                if p.kind == "offset" && !super::mepatoms::is_labor_item(&p.item) {
                    rp.raceway_ft += p.length_ft;
                }
                *rp.fittings.entry((p.kind.clone(), p.item.clone(), p.size.clone())).or_insert(0) += 1;
            }
        }
    }
    if let Some(r) = ar.raceways.get(&rp.material).filter(|_| !rp.cable) {
        rp.max_bends = pos(r.max_bends_deg);
        rp.max_source = r.max_bends_source.clone();
        rp.stock_ft = pos(r.stock_ft);
    }
    rp.cut_pieces.sort_by(|a, b| b.total_cmp(a));
    // The ends: the connectors (a cable without them: the cable), the one on gear first.
    let chain: Vec<&Piece> = ps.iter().copied().filter(|p| in_chain(p)).collect();
    let set: HashSet<Handle> = chain.iter().map(|p| p.handle).collect();
    let mut ends: Vec<(&Piece, Option<End>)> = ps
        .iter()
        .copied()
        .filter(|p| p.kind == "connector" || (p.kind == "cable" && !ps.iter().any(|q| q.kind == "connector")))
        .map(|p| (p, landing(d, sc, p, &own)))
        .collect();
    ends.sort_by_key(|(p, l)| (!l.as_ref().is_some_and(|l| l.gear), p.handle));
    let open = || End { label: "open end".into(), gear: false };
    if rp.cable {
        // A cable lands at both its ends through itself (or its two connectors).
        let mut ls: Vec<End> = ends.iter().filter_map(|(_, l)| l.clone()).collect();
        if let Some(c) = ps.iter().find(|p| p.kind == "cable").filter(|_| !ps.iter().any(|q| q.kind == "connector")) {
            ls = c
                .connects
                .iter()
                .filter_map(|h| {
                    let one = Piece { connects: vec![*h], ..Piece::default() };
                    landing(d, sc, &one, &own)
                })
                .collect();
            ls.sort_by_key(|l| !l.gear);
        }
        rp.from = ls.first().cloned().unwrap_or_else(open);
        rp.to = ls.get(1).cloned().unwrap_or_else(open);
        return rp;
    }
    rp.from = ends.first().and_then(|(_, l)| l.clone()).unwrap_or_else(open);
    rp.to = ends.get(1).and_then(|(_, l)| l.clone()).unwrap_or_else(open);
    // Walk the chain from its first end: sticks, couplings, elbows, offsets, in order.
    let start = ends
        .first()
        .map(|(p, _)| *p)
        .filter(|p| set.contains(&p.handle))
        .or_else(|| chain.iter().copied().find(|p| p.connects.iter().filter(|h| set.contains(h)).count() <= 1));
    let Some(start) = start else { return rp };
    let marks: Vec<&Piece> = ps.iter().copied().filter(|p| p.kind == "bend" || (p.kind == "offset" && !in_chain(p))).collect();
    let mut seen: HashSet<Handle> = HashSet::new();
    let mut cur = start;
    let mut s = 0.0;
    let mut prev = start.at;
    for _ in 0..chain.len().min(MAX_WALK) {
        seen.insert(cur.handle);
        match cur.kind.as_str() {
            "conduit" => {
                let (a, b) = match (cur.start, cur.end) {
                    (Some(a), Some(b)) if dist(b, prev) < dist(a, prev) => (b, a),
                    (Some(a), Some(b)) => (a, b),
                    _ => (prev, prev),
                };
                let mut on: Vec<(f64, f64, Vec3)> = marks
                    .iter()
                    .filter(|m| m.connects.contains(&cur.handle))
                    .map(|m| {
                        let deg = if m.kind == "offset" { 2.0 * m.angle_deg } else { m.turn_deg.unwrap_or(m.angle_deg) };
                        ((dist(m.at, a) / upf).min(cur.length_ft), deg, m.at)
                    })
                    .collect();
                on.sort_by(|x, y| x.0.total_cmp(&y.0));
                for (at_ft, deg, at) in on {
                    rp.bends.push(Bend { station_ft: s + at_ft, deg, at });
                }
                s += cur.length_ft;
                prev = b;
            }
            "elbow" => {
                rp.bends.push(Bend { station_ft: s, deg: cur.turn_deg.unwrap_or(cur.angle_deg), at: cur.at });
                prev = cur.at;
            }
            "offset" => {
                rp.bends.push(Bend { station_ft: s + cur.length_ft / 2.0, deg: 2.0 * cur.angle_deg, at: cur.at });
                s += cur.length_ft;
                prev = cur.at;
            }
            _ => prev = cur.at,
        }
        let next =
            cur.connects.iter().filter(|h| set.contains(h) && !seen.contains(h)).filter_map(|h| sc.at.get(h).and_then(|i| sc.pieces.get(*i))).next();
        match next {
            Some(n) => cur = n,
            None => break,
        }
    }
    rp
}

/// Where the pull boxes go along a path of bends: before each bend that would take the bends
/// since the last pull point past `max` degrees (indices into `bends`).
fn pull_points(bends: &[Bend], max: f64) -> Vec<usize> {
    let mut out = Vec::new();
    if !(max.is_finite() && max > 0.0) {
        return out;
    }
    let mut since = 0.0;
    for (i, b) in bends.iter().enumerate() {
        if since > 0.0 && since + b.deg > max + 1.0 {
            out.push(i);
            since = 0.0;
        }
        since += b.deg;
    }
    out
}

/// The pull box a raceway and its conductors take, and the rule.
fn pull_box_size(size: &str, wires: &[Wire]) -> (String, String) {
    let r = &rules().pull_box;
    let trade = trade_in(size);
    let large = wires.iter().any(|w| w.role != "ground" && awg_at_least(&w.size, r.sized_min_awg));
    let small = r.small.iter().find(|(max, _)| trade <= max + 1e-9).map(|(_, b)| b.clone());
    match (large, small) {
        (false, Some(b)) => (b, r.small_source.clone()),
        (big, _) => {
            let len = (pos(r.straight_times) * trade).ceil().max(1.0);
            (
                format!("{len:.0}\" long at least ({} x {size})", r.straight_times),
                if big { r.straight_source.clone() } else { r.large_source.clone() },
            )
        }
    }
}

/// A trade size's inches (`1-1/4"` → 1.25).
fn trade_in(size: &str) -> f64 {
    let t = size.trim().trim_end_matches('"').trim();
    let (whole, frac) = match t.split_once('-') {
        Some((w, f)) => (w.trim(), Some(f.trim())),
        None if t.contains('/') => ("0", Some(t)),
        None => (t, None),
    };
    let w = whole.parse::<f64>().unwrap_or(0.0);
    let f =
        frac.and_then(|f| f.split_once('/')).and_then(|(a, b)| Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?)).unwrap_or(0.0);
    let v = w + f;
    if v.is_finite() { v } else { 0.0 }
}

/// Whether a conductor size (`#4`, `#2/0`, `250 kcmil`) is #`min` or larger.
fn awg_at_least(size: &str, min: u32) -> bool {
    let t = size.trim().trim_start_matches('#').to_ascii_lowercase();
    if t.contains("kcmil") || t.contains('/') {
        return true;
    }
    t.parse::<u32>().is_ok_and(|n| n <= min.max(1))
}

// ─── Pulls ───────────────────────────────────────────────────────────────────

/// A conductor of a pull: size, insulation, colour and role, how many, each one's cut.
#[derive(Clone, Debug, Default)]
struct Conductor {
    size: String,
    ty: String,
    item: String,
    color: String,
    role: String,
    count: u32,
    cut_ft: f64,
    /// What the takeoff counts of each (the model's wire), 0 for a feeder's.
    takeoff_ft: f64,
}

/// One pull path: a branch run, or one conduit of a feeder (its chains end to end).
#[derive(Clone, Debug, Default)]
struct PullPath {
    kind: &'static str,
    /// The circuit (`LP1-3`), or the feeder and the gear it goes to (`F1 to LP1`).
    id: String,
    level: String,
    panel: String,
    runs: Vec<String>,
    from: End,
    to: End,
    raceway: String,
    raceway_ft: f64,
    vertical_ft: f64,
    bends: Vec<Bend>,
    max_bends: f64,
    max_source: String,
    boxes: Vec<usize>,
    box_size: (String, String),
    conductors: Vec<Conductor>,
    /// Paths alike (a feeder's parallel conduits), each one these conductors.
    conduits: u32,
    cable: bool,
    estimated: bool,
    status: String,
    /// The takeoff's own wire on this path (the model's wire or cable), all of it.
    takeoff_ft: f64,
    gear_ft: f64,
    loops_ft: f64,
    /// Read from the spec's routed runs: the model holds no pieces for it (in no takeoff).
    from_spec: bool,
}

impl PullPath {
    fn conductor_ft(&self) -> f64 {
        self.conductors.iter().map(|c| c.cut_ft * f64::from(c.count)).sum::<f64>() * f64::from(self.conduits.max(1))
    }
    fn gear_ends(&self) -> u32 {
        u32::from(self.from.gear) + u32::from(self.to.gear)
    }
    /// The pulls between its pull points: from, to, feet, degrees.
    fn pulls(&self) -> Vec<(String, String, f64, f64)> {
        let mut out = Vec::new();
        let mut from = self.from.label.clone();
        let mut s0 = 0.0;
        let mut deg = 0.0;
        let mut k = 0usize;
        for (i, b) in self.bends.iter().enumerate() {
            if self.boxes.contains(&i) {
                k += 1;
                let at = (b.station_ft - 0.5).max(s0);
                out.push((from.clone(), format!("pull box {k}"), at - s0, deg));
                from = format!("pull box {k}");
                s0 = at;
                deg = 0.0;
            }
            deg += b.deg;
        }
        out.push((from, self.to.label.clone(), (self.raceway_ft - s0).max(0.0), deg));
        out
    }
}

/// Conductors from a run's wires, grouped, each cut with its slack.
fn conductors_of(wires: &[Wire], extra: f64) -> Vec<Conductor> {
    let mut out: Vec<Conductor> = Vec::new();
    for w in wires {
        let cut = w.length_ft + extra;
        match out
            .iter_mut()
            .find(|c| c.size == w.size && c.ty == w.ty && c.item == w.item && c.color == w.color && c.role == w.role && (c.cut_ft - cut).abs() < 1e-6)
        {
            Some(c) => c.count += 1,
            None => out.push(Conductor {
                size: w.size.clone(),
                ty: w.ty.clone(),
                item: w.item.clone(),
                color: w.color.clone(),
                role: w.role.clone(),
                count: 1,
                cut_ft: cut,
                takeoff_ft: w.length_ft,
            }),
        }
    }
    out
}

/// A feeder's conductors per set as the model's wires would carry them (its schedule's makeup,
/// the phases' colours of the voltage it goes to), each `length` feet before slack.
fn feeder_wires(sp: &BuildingSpec, f: &cadaddy_walk3d::Feeder, length: f64, upf: f64) -> Vec<Wire> {
    let ar = super::mepatoms::rules();
    let makeup_text = super::mepatoms::build::feeder_makeup(f);
    let phases: Vec<u32> = [1u32, 3, 5].into_iter().take(f.phases.clamp(1, 3) as usize).collect();
    let voltage = super::mepatoms::build::gear_voltage(sp, &f.to).unwrap_or_else(|| ar.default_voltage.clone());
    let path = [Vec3::ZERO, Vec3::new(length.max(0.01) * upf, 0.0, 0.0)];
    super::mepatoms::wires(&path, upf, &makeup_text, &phases, &voltage, ar)
        .into_iter()
        .map(|a| {
            let get = |k: &str| a.extra.iter().find(|x| x.0 == k).map(|x| x.1.clone()).unwrap_or_default();
            Wire { size: a.size.clone(), ty: a.ty.clone(), item: a.item.clone(), color: get("color"), role: get("conductor"), length_ft: a.length_ft }
        })
        .collect()
}

/// Everything the paperwork is made from.
#[derive(Default)]
struct Plan {
    runs: Vec<RunPlan>,
    paths: Vec<PullPath>,
    unrouted_feeders: Vec<Value>,
    empty_raceways: (usize, f64),
    /// The takeoff's wire and cable (every piece's), by size or type.
    takeoff_wire: BTreeMap<String, f64>,
    /// The model's wire no pull holds (a feeder's entry stubs), by size.
    not_in_pulls: BTreeMap<String, f64>,
    levels: Vec<String>,
    devices: Vec<Device>,
    pieces: Vec<Piece>,
    upf: f64,
}

/// A wire's reconciliation key: its size (a cable: its type).
fn wire_key(kind: &str, p_size: &str, p_ty: &str) -> String {
    if kind == "cable" { p_ty.to_string() } else { p_size.to_string() }
}

fn plan(d: &Drawing, sp: &BuildingSpec) -> Plan {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let r = rules();
    let sc = scan(d);
    let mut by_run: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut takeoff_wire: BTreeMap<String, f64> = BTreeMap::new();
    for (i, p) in sc.pieces.iter().enumerate() {
        if matches!(p.kind.as_str(), "wire" | "cable") {
            *takeoff_wire.entry(wire_key(&p.kind, &p.size, &p.ty)).or_insert(0.0) += p.length_ft;
        }
        if !p.run.is_empty() {
            by_run.entry(p.run.clone()).or_default().push(i);
        }
    }
    let runs: Vec<RunPlan> = by_run.iter().map(|(run, idx)| plan_run(d, &sc, run, idx, upf)).collect();
    // The feeders' routed runs: each spec run's id in the pieces (`<base>#<chain>`), its leg.
    let mut feeder_base: HashMap<String, (usize, String)> = HashMap::new();
    for (i, sr) in sp.mep.runs.iter().enumerate() {
        if super::ugplan::is_feeder_run(sr) {
            feeder_base.entry(super::mep::run_base(sr)).or_insert((i, sr.source_sheet.clone()));
        }
    }
    let mut legs: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let mut pl = Plan { upf, takeoff_wire, ..Plan::default() };
    let extra_gear = r.gear_extra();
    let loop_ft = pos(r.pull_box_loop_ft);
    for (ri, rp) in runs.iter().enumerate() {
        let base = rp.run.rsplit_once('#').map_or(rp.run.as_str(), |(b, _)| b);
        if let Some((si, leg)) = feeder_base.get(base) {
            legs.entry(leg.clone()).or_default().push((*si, ri));
            // Its wire in the model is the feeder's: its pull is cut from the schedule along
            // the whole route (with the gear ends' slack).
            for w in &rp.wires {
                *pl.not_in_pulls.entry(w.size.clone()).or_insert(0.0) += w.length_ft;
            }
            continue;
        }
        if rp.run.starts_with("feeder:") {
            // A feeder's entry stub: its wire is the feeder's, pulled with the feeder.
            for w in &rp.wires {
                *pl.not_in_pulls.entry(w.size.clone()).or_insert(0.0) += w.length_ft;
            }
            continue;
        }
        if rp.circuit.is_empty() && rp.wires.is_empty() && rp.cable_ft <= 0.0 {
            pl.empty_raceways.0 += 1;
            pl.empty_raceways.1 += rp.raceway_ft;
            continue;
        }
        let boxes = pull_points(&rp.bends, rp.max_bends);
        let mut path = PullPath {
            kind: "branch",
            id: rp.circuit.clone(),
            level: rp.level.clone(),
            panel: rp.panel.clone(),
            runs: vec![rp.run.clone()],
            from: rp.from.clone(),
            to: rp.to.clone(),
            raceway: rp.raceway_text(),
            raceway_ft: rp.raceway_ft,
            vertical_ft: rp.vertical_ft,
            bends: rp.bends.clone(),
            max_bends: rp.max_bends,
            max_source: rp.max_source.clone(),
            boxes,
            conduits: 1,
            cable: rp.cable,
            estimated: rp.estimated,
            status: if rp.estimated { "estimated route".into() } else { "routed".into() },
            ..PullPath::default()
        };
        let extra = extra_gear * f64::from(path.gear_ends()) + loop_ft * path.boxes.len() as f64;
        if rp.cable {
            path.conductors = vec![Conductor {
                size: rp.size.clone(),
                ty: rp.material.clone(),
                item: rp.material.clone(),
                count: 1,
                cut_ft: rp.cable_ft + extra_gear * f64::from(path.gear_ends()),
                takeoff_ft: rp.cable_ft,
                ..Conductor::default()
            }];
            path.takeoff_ft = rp.cable_ft;
            path.gear_ft = extra_gear * f64::from(path.gear_ends());
        } else {
            path.conductors = conductors_of(&rp.wires, extra);
            path.takeoff_ft = rp.wires.iter().map(|w| w.length_ft).sum();
            let n = rp.wires.len() as f64;
            path.gear_ft = extra_gear * f64::from(path.gear_ends()) * n;
            path.loops_ft = loop_ft * path.boxes.len() as f64 * n;
        }
        path.box_size = pull_box_size(&rp.size, &rp.wires);
        pl.paths.push(path);
    }
    // Each feeder leg (`feeder <tag> <from>→<to>…`, as the routing names its runs): from its
    // pieces when the build cut it into them, else from its routed runs in the spec.
    let mut spec_legs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, sr) in sp.mep.runs.iter().enumerate().take(MAX_SPEC_RUNS) {
        if super::ugplan::is_feeder_run(sr) && sr.source_sheet.starts_with("feeder ") && !legs.contains_key(&sr.source_sheet) {
            spec_legs.entry(sr.source_sheet.clone()).or_default().push(i);
        }
    }
    let names: BTreeSet<String> = legs.keys().chain(spec_legs.keys()).cloned().collect();
    for leg in names.iter().take(MAX_LISTED) {
        let Some((f, dest)) = feeder_for(sp, leg) else { continue };
        let rows = match legs.get(leg) {
            Some(chains) => feeder_rows_from_pieces(sp, f, &dest, chains, &runs, upf),
            None => feeder_rows_from_spec(sp, f, &dest, spec_legs.get(leg).map(Vec::as_slice).unwrap_or_default(), upf),
        };
        pl.paths.extend(rows);
    }
    // The feeders no leg routes (one line for feeders alike: a typical panel's).
    let mut seen_unrouted: BTreeSet<(String, String, String)> = BTreeSet::new();
    for f in sp.mep.feeders.iter().take(MAX_LISTED) {
        let routed = names.iter().any(|leg| feeder_for(sp, leg).is_some_and(|(g, _)| g.tag == f.tag && g.from == f.from && g.to == f.to))
            || names.iter().any(|leg| leg_dest(f, leg).is_some_and(|d| first_gear(&d).starts_with(&f.to.trim().to_ascii_uppercase())));
        if !routed && seen_unrouted.insert((f.tag.clone(), f.from.clone(), f.to.clone())) {
            pl.unrouted_feeders.push(json!({"feeder": f.tag, "from": f.from, "to": f.to, "why": "no routed raceway in the model or its runs: the pull schedule has no path for it"}));
        }
    }
    // The levels, lowest first (the spec's order), then any other the pieces name.
    let mut levels: Vec<String> = sp.levels.iter().map(|l| l.name.clone()).collect();
    for rp in &runs {
        if !rp.level.is_empty() && !levels.iter().any(|l| l.eq_ignore_ascii_case(&rp.level)) {
            levels.push(rp.level.clone());
        }
    }
    pl.levels = levels;
    pl.runs = runs;
    pl.devices = sc.devices;
    pl.pieces = sc.pieces;
    pl
}

/// What a leg goes to after its feeder's prefix (`feeder 7 PA→`): `PB-2→PB-3`.
fn leg_dest(f: &cadaddy_walk3d::Feeder, leg: &str) -> Option<String> {
    leg.strip_prefix(&format!("feeder {} {}\u{2192}", f.tag, f.from)).map(str::to_string)
}

/// The first gear a leg goes to, upper case.
fn first_gear(dest: &str) -> String {
    dest.split('\u{2192}').next().unwrap_or_default().trim().to_ascii_uppercase()
}

/// The feeder a leg is of and what it goes to: the one whose `to` the leg goes to first, else
/// the first of its tag and source.
fn feeder_for<'a>(sp: &'a BuildingSpec, leg: &str) -> Option<(&'a cadaddy_walk3d::Feeder, String)> {
    let cands: Vec<(&cadaddy_walk3d::Feeder, String)> =
        sp.mep.feeders.iter().take(MAX_LISTED).filter_map(|f| leg_dest(f, leg).map(|d| (f, d))).collect();
    cands
        .iter()
        .find(|(f, d)| first_gear(d) == f.to.trim().to_ascii_uppercase())
        .or_else(|| cands.iter().find(|(f, d)| first_gear(d).starts_with(&f.to.trim().to_ascii_uppercase())))
        .or(cands.first())
        .cloned()
}

/// A feeder path's conductors: its schedule's sets per raceway (`sets` over `conduits`), each
/// cut with the slack at its gear ends and pull boxes.
fn feeder_conductors(sp: &BuildingSpec, f: &cadaddy_walk3d::Feeder, path: &mut PullPath, upf: f64) {
    let r = rules();
    let wires = feeder_wires(sp, f, path.raceway_ft, upf);
    path.box_size = pull_box_size(path.raceway.split_whitespace().next().unwrap_or_default(), &wires);
    let extra = r.computed_gear_extra() * f64::from(path.gear_ends()) + pos(r.pull_box_loop_ft) * path.boxes.len() as f64;
    let per = (f.sets / f.conduits.max(1)).clamp(1, 12);
    path.conductors = conductors_of(&wires, extra);
    for c in &mut path.conductors {
        c.count = c.count.saturating_mul(per);
        c.takeoff_ft = 0.0;
    }
}

/// Paths alike (the same ends, length and bends) as one row of `conduits` paths.
fn add_alike(rows: &mut Vec<PullPath>, path: PullPath) {
    match rows.iter_mut().find(|r| {
        r.from.label == path.from.label
            && r.to.label == path.to.label
            && (r.raceway_ft - path.raceway_ft).abs() < 0.5
            && r.bends.len() == path.bends.len()
            && r.boxes.len() == path.boxes.len()
            && r.cable == path.cable
    }) {
        Some(r) => {
            r.conduits += 1;
            r.runs.extend(path.runs);
            r.takeoff_ft += path.takeoff_ft;
            r.gear_ft += path.gear_ft;
        }
        None => rows.push(path),
    }
}

/// A feeder leg the build cut into pieces: its conduits, each its chains end to end (a chain on
/// the leg's first level starts the next conduit; one on another level continues it: the riser
/// goes on).
fn feeder_rows_from_pieces(
    sp: &BuildingSpec,
    f: &cadaddy_walk3d::Feeder,
    dest: &str,
    chains: &[(usize, usize)],
    runs: &[RunPlan],
    upf: f64,
) -> Vec<PullPath> {
    let mut chains = chains.to_vec();
    chains.sort();
    let first_level = chains.first().and_then(|(_, ri)| runs.get(*ri)).map(|r| r.level.clone()).unwrap_or_default();
    let mut conduits: Vec<Vec<&RunPlan>> = Vec::new();
    for (_, ri) in &chains {
        let Some(rp) = runs.get(*ri) else { continue };
        let goes_on = !rp.level.eq_ignore_ascii_case(&first_level);
        match conduits.last_mut() {
            Some(c) if goes_on => c.push(rp),
            _ => conduits.push(vec![rp]),
        }
    }
    let to_gear = dest.rsplit('\u{2192}').next().unwrap_or(dest).to_string();
    let mut rows: Vec<PullPath> = Vec::new();
    for c in conduits {
        let mut bends = Vec::new();
        let mut s0 = 0.0;
        for rp in &c {
            bends.extend(rp.bends.iter().map(|b| Bend { station_ft: b.station_ft + s0, ..*b }));
            s0 += rp.raceway_ft;
        }
        let (Some(a), Some(z)) = (c.first(), c.last()) else { continue };
        let cable = c.iter().any(|rp| rp.cable);
        let boxes = if cable { Vec::new() } else { pull_points(&bends, a.max_bends) };
        let mut path = PullPath {
            kind: "feeder",
            id: format!("{} to {dest}", f.tag),
            level: a.level.clone(),
            panel: f.from.clone(),
            runs: c.iter().map(|rp| rp.run.clone()).collect(),
            from: if a.from.label == "open end" { End { label: f.from.clone(), gear: true } } else { a.from.clone() },
            to: if z.to.label == "open end" { End { label: to_gear.clone(), gear: true } } else { z.to.clone() },
            raceway: a.raceway_text(),
            raceway_ft: s0,
            vertical_ft: c.iter().map(|rp| rp.vertical_ft).sum(),
            bends,
            max_bends: a.max_bends,
            max_source: a.max_source.clone(),
            boxes,
            conduits: 1,
            cable,
            status: "proposed feeder route".into(),
            ..PullPath::default()
        };
        if cable {
            let gear_extra = rules().gear_extra() * f64::from(path.gear_ends());
            let ft: f64 = c.iter().map(|rp| rp.cable_ft).sum();
            path.conductors = vec![Conductor {
                size: a.size.clone(),
                ty: a.material.clone(),
                item: a.material.clone(),
                count: 1,
                cut_ft: ft + gear_extra,
                takeoff_ft: ft,
                ..Conductor::default()
            }];
            path.takeoff_ft = ft;
            path.gear_ft = gear_extra;
        } else {
            feeder_conductors(sp, f, &mut path, upf);
        }
        add_alike(&mut rows, path);
    }
    rows
}

/// A path's length, its vertical part (feet) and its bends: the turn at each corner.
fn geometry(pts: &[Vec3], upf: f64) -> (f64, f64, Vec<Bend>) {
    let (mut len, mut vert) = (0.0, 0.0);
    let mut bends = Vec::new();
    for (i, w) in pts.windows(2).enumerate() {
        let [a, b] = w else { continue };
        if i > 0
            && let Some(p) = pts.get(i - 1)
        {
            let (u, v) = (*a - *p, *b - *a);
            let (lu, lv) = (u.len(), v.len());
            if lu > 1e-9 && lv > 1e-9 {
                let deg = (u.dot(v) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees();
                if deg >= 1.0 {
                    bends.push(Bend { station_ft: len, deg, at: *a });
                }
            }
        }
        len += dist(*a, *b) / upf;
        vert += (b.z - a.z).abs() / upf;
    }
    (len, vert, bends)
}

/// A feeder leg the model holds only as routed runs (not cut into pieces: in no takeoff): its
/// runs stitched end to end (each level's plan from its own corner, so a riser meets the floor
/// above), each path's ends named by the leg's gear they are at (a feed-through chain is a path
/// per panel; parallel conduits are one row).
fn feeder_rows_from_spec(sp: &BuildingSpec, f: &cadaddy_walk3d::Feeder, dest: &str, idx: &[usize], upf: f64) -> Vec<PullPath> {
    let ar = super::mepatoms::rules();
    let base = |level: &str| sp.level(level).map(|l| (l.elevation_ft * upf, l.region.map_or([0.0, 0.0], |r| [r[0], r[1]])));
    let tol = pos(ar.joint_tol_in).max(0.01) / 12.0 * upf;
    let mut polys: Vec<(Vec<Vec3>, Vec<String>)> = Vec::new();
    let mut first: Option<&cadaddy_walk3d::MepRun> = None;
    for r in idx.iter().filter_map(|i| sp.mep.runs.get(*i)) {
        let Some((z0, c)) = base(&r.level) else { continue };
        let pts: Vec<Vec3> = super::mepatoms::build::run_path(r, z0, upf).into_iter().map(|p| Vec3::new(p.x - c[0], p.y - c[1], p.z)).collect();
        let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied()) else { continue };
        if pts.len() < 2 {
            continue;
        }
        first = first.or(Some(r));
        let id = super::mep::run_base(r);
        let near = |p: Vec3, q: Vec3| dist(p, q) <= tol;
        let mut placed = false;
        for (pl, ids) in polys.iter_mut() {
            let (Some(s), Some(e)) = (pl.first().copied(), pl.last().copied()) else { continue };
            if pl.len() > 4_000 {
                continue;
            }
            if near(e, a) {
                pl.extend(pts.iter().skip(1).copied());
            } else if near(e, b) {
                pl.extend(pts.iter().rev().skip(1).copied());
            } else if near(s, b) {
                let mut v = pts.clone();
                v.pop();
                v.extend(pl.iter().copied());
                *pl = v;
            } else if near(s, a) {
                let mut v: Vec<Vec3> = pts.iter().rev().copied().collect();
                v.pop();
                v.extend(pl.iter().copied());
                *pl = v;
            } else {
                continue;
            }
            ids.push(id.clone());
            placed = true;
            break;
        }
        if !placed {
            polys.push((pts, vec![id]));
        }
    }
    let Some(first) = first else { return Vec::new() };
    // The gear along the leg, in its order, and which of them a point is at.
    let gears: Vec<String> =
        std::iter::once(f.from.clone()).chain(dest.split('\u{2192}').map(|g| g.trim().to_string())).filter(|g| !g.is_empty()).collect();
    let reach = pos(ar.panel_reach_ft) * upf;
    let gear_at = |p: Vec3| -> Option<usize> {
        gears.iter().enumerate().find_map(|(k, g)| {
            let e = sp.mep.equipment.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(g))?;
            let (z0, c) = base(&e.level)?;
            let (x0, x1) = (e.min[0].min(e.max[0]) - c[0] - reach, e.min[0].max(e.max[0]) - c[0] + reach);
            let (y0, y1) = (e.min[1].min(e.max[1]) - c[1] - reach, e.min[1].max(e.max[1]) - c[1] + reach);
            let (zb, zt) = (z0 + (e.bottom_ft - 1.0) * upf, z0 + (e.bottom_ft + e.height_ft + 1.0) * upf);
            (p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1 && p.z >= zb && p.z <= zt).then_some(k)
        })
    };
    let sy = sp.mep.systems.iter().find(|y| y.name == first.system);
    let words = sy.map_or_else(|| first.system.clone(), |y| format!("{} {}", y.name, y.layer));
    let (material, rule) = ar
        .raceway_named(&words)
        .or_else(|| ar.raceways.get_key_value(ar.default_raceway.as_str()).map(|(k, v)| (k.as_str(), v)))
        .map_or(("", None), |(k, v)| (k, Some(v)));
    let size = super::mepatoms::trade_size(first.size_in[0]);
    let level_of = |run: &str| {
        idx.iter().filter_map(|i| sp.mep.runs.get(*i)).find(|r| super::mep::run_base(r) == run).map(|r| r.level.clone()).unwrap_or_default()
    };
    let mut rows = Vec::new();
    for (mut pts, ids) in polys {
        let (mut ka, mut kb) = (pts.first().and_then(|p| gear_at(*p)), pts.last().and_then(|p| gear_at(*p)));
        if ka.unwrap_or(usize::MAX) > kb.unwrap_or(usize::MAX) {
            pts.reverse();
            std::mem::swap(&mut ka, &mut kb);
        }
        let label = |k: Option<usize>| {
            k.and_then(|k| gears.get(k)).map_or_else(|| End { label: "open end".into(), gear: false }, |g| End { label: g.clone(), gear: true })
        };
        let (len, vert, bends) = geometry(&pts, upf);
        let max_bends = rule.map_or(0.0, |r| pos(r.max_bends_deg));
        let mut path = PullPath {
            kind: "feeder",
            id: format!("{} to {dest}", f.tag),
            level: ids.first().map(|r| level_of(r)).unwrap_or_default(),
            panel: f.from.clone(),
            from: label(ka),
            to: label(kb),
            raceway: format!("{size} {material}").trim().to_string(),
            raceway_ft: len,
            vertical_ft: vert,
            boxes: pull_points(&bends, max_bends),
            bends,
            max_bends,
            max_source: rule.map(|r| r.max_bends_source.clone()).unwrap_or_default(),
            runs: ids,
            conduits: 1,
            from_spec: true,
            status: "proposed feeder route, held as runs: not cut into pieces, in no takeoff".into(),
            ..PullPath::default()
        };
        feeder_conductors(sp, f, &mut path, upf);
        add_alike(&mut rows, path);
    }
    rows
}

fn r2(v: f64) -> f64 {
    let r = if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 };
    // No `-0`: a straight riser turns 0 degrees.
    if r == 0.0 { 0.0 } else { r }
}

/// Feet as a schedule shows them: `12.5`.
fn ft(v: f64) -> String {
    let r = r2(v);
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
}

fn path_json(p: &PullPath, upf: f64) -> Value {
    let r = rules();
    let boxes: Vec<Value> = p
        .boxes
        .iter()
        .enumerate()
        .filter_map(|(k, i)| {
            let b = p.bends.get(*i)?;
            Some(json!({
                "box": k + 1,
                "station_ft": r2((b.station_ft - 0.5).max(0.0)),
                "at": [r2(b.at.x / upf), r2(b.at.y / upf), r2(b.at.z / upf)],
                "before_bend_deg": r2(b.deg),
                "size": p.box_size.0,
                "source": p.box_size.1,
                "where": r.pull_box.place,
            }))
        })
        .collect();
    let mut j = json!({
        "kind": p.kind,
        "level": p.level,
        "panel": p.panel,
        "runs": p.runs,
        "from": p.from.label,
        "to": p.to.label,
        "raceway": p.raceway,
        "raceway_ft": r2(p.raceway_ft),
        "vertical_ft": r2(p.vertical_ft),
        "bends_deg": r2(p.bends.iter().map(|b| b.deg).sum()),
        "max_bends_deg": p.max_bends,
        "max_bends_source": p.max_source,
        "pull_boxes": boxes,
        "pulls": p.pulls().into_iter().map(|(a, b, f, d)| json!({"from": a, "to": b, "ft": r2(f), "bends_deg": r2(d)})).collect::<Vec<_>>(),
        "conductors": p.conductors.iter().map(|c| json!({"size": c.size, "type": c.ty, "item": c.item, "color": c.color, "role": c.role, "count": c.count, "cut_ft": r2(c.cut_ft)})).collect::<Vec<_>>(),
        "conduits": p.conduits,
        "conductor_ft": r2(p.conductor_ft()),
        "takeoff_ft": r2(p.takeoff_ft),
        "slack": {
            "box_end_ft": makeup(), "box_end_source": r.box_end_slack_source,
            "gear_end_ft": pos(r.gear_end_slack_ft), "gear_end_source": r.gear_end_slack_source,
            "pull_box_ft": pos(r.pull_box_loop_ft), "pull_box_source": r.pull_box_loop_source,
        },
        "estimated": p.estimated,
        "in_takeoff": !p.from_spec,
        "status": p.status,
    });
    j[if p.kind == "feeder" { "feeder" } else { "circuit" }] = json!(p.id);
    j
}

/// The conductors as one line: `3 #12 THHN (black A, red B, white N) + 1 #12 green G`.
fn conductors_text(p: &PullPath) -> String {
    p.conductors
        .iter()
        .map(|c| {
            let role = c.role.strip_prefix("phase ").unwrap_or(&c.role);
            if p.cable { c.ty.clone() } else { format!("{} {} {} {}", c.count, c.size, c.color, role).trim().to_string() }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ─── Reels ───────────────────────────────────────────────────────────────────

/// One reel of a plan: its length, the cuts on it (indices), and what they use.
#[derive(Clone, Debug, Default, PartialEq)]
struct Reel {
    length: f64,
    cuts: Vec<usize>,
    used: f64,
    cut_to_length: bool,
}

/// Cuts packed onto reels of the standard lengths with the least waste (then the fewest reels):
/// every grouping tried for a few cuts, best fit decreasing (each standard length as the reel
/// opened, every reel shrunk to the shortest that holds it) for more. A cut longer than every
/// standard reel is a cut-to-length reel of its own.
fn pack(cuts: &[f64], lengths: &[f64]) -> (Vec<Reel>, &'static str) {
    let mut sizes: Vec<f64> = lengths.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
    sizes.sort_by(f64::total_cmp);
    sizes.dedup();
    let largest = sizes.last().copied().unwrap_or(1000.0);
    if sizes.is_empty() {
        sizes.push(largest);
    }
    let fit = |used: f64| sizes.iter().copied().find(|s| used <= s + 1e-9);
    let mut reels: Vec<Reel> = Vec::new();
    let mut rest: Vec<usize> = Vec::new();
    for (i, c) in cuts.iter().enumerate().take(MAX_CUTS) {
        let c = pos(*c);
        if c > largest + 1e-9 {
            reels.push(Reel { length: c.ceil(), cuts: vec![i], used: c, cut_to_length: true });
        } else {
            rest.push(i);
        }
    }
    let len = |i: usize| pos(cuts.get(i).copied().unwrap_or(0.0));
    let waste = |rs: &[Reel]| rs.iter().map(|r| r.length - r.used).sum::<f64>();
    let score = |rs: &[Reel]| (r2(waste(rs) * 100.0), rs.len());
    let method;
    let best: Vec<Reel> = if rest.len() <= EXACT_CUTS {
        method = "exact";
        // Every grouping (restricted growth strings), each group on its shortest reel.
        let n = rest.len();
        let mut best: Option<(Vec<Reel>, (f64, usize))> = None;
        let mut groups: Vec<f64> = Vec::new();
        let mut assign: Vec<usize> = vec![0; n];
        fn go(
            k: usize,
            rest: &[usize],
            len: &dyn Fn(usize) -> f64,
            largest: f64,
            groups: &mut Vec<f64>,
            assign: &mut [usize],
            visit: &mut dyn FnMut(&[f64], &[usize]),
        ) {
            if k == rest.len() {
                visit(groups, assign);
                return;
            }
            let c = rest.get(k).map_or(0.0, |i| len(*i));
            for g in 0..=groups.len() {
                let opened = g == groups.len();
                if opened {
                    groups.push(c);
                } else if let Some(u) = groups.get_mut(g) {
                    if *u + c > largest + 1e-9 {
                        continue;
                    }
                    *u += c;
                }
                if let Some(a) = assign.get_mut(k) {
                    *a = g;
                }
                go(k + 1, rest, len, largest, groups, assign, visit);
                if opened {
                    groups.pop();
                } else if let Some(u) = groups.get_mut(g) {
                    *u -= c;
                }
            }
        }
        let mut visit = |gs: &[f64], asg: &[usize]| {
            let mut rs: Vec<Reel> = gs.iter().map(|u| Reel { length: fit(*u).unwrap_or(largest), used: *u, ..Reel::default() }).collect();
            for (k, g) in asg.iter().enumerate() {
                if let (Some(r), Some(i)) = (rs.get_mut(*g), rest.get(k)) {
                    r.cuts.push(*i);
                }
            }
            let sc = score(&rs);
            if best.as_ref().is_none_or(|(_, b)| sc < *b) {
                best = Some((rs, sc));
            }
        };
        go(0, &rest, &len, largest, &mut groups, &mut assign, &mut visit);
        best.map(|b| b.0).unwrap_or_default()
    } else {
        method = "best_fit";
        let mut order = rest.clone();
        order.sort_by(|a, b| len(*b).total_cmp(&len(*a)).then(a.cmp(b)));
        let mut best: Option<Vec<Reel>> = None;
        let total: f64 = order.iter().map(|i| len(*i)).sum();
        for open in sizes.iter().copied().map(Some).chain(std::iter::once(None)) {
            let mut rs: Vec<Reel> = Vec::new();
            let mut left = total;
            for &i in &order {
                let c = len(i);
                let slot = rs.iter_mut().filter(|r| r.length - r.used >= c - 1e-9).min_by(|a, b| (a.length - a.used).total_cmp(&(b.length - b.used)));
                match slot {
                    Some(r) => {
                        r.used += c;
                        r.cuts.push(i);
                    }
                    None => {
                        // The reel opened: the one asked for, else (`None`) the shortest that
                        // holds all that is left, else the longest.
                        let size = match open {
                            Some(s) if s >= c - 1e-9 => s,
                            Some(_) => fit(c).unwrap_or(largest),
                            None => fit(left).unwrap_or(largest),
                        };
                        rs.push(Reel { length: size, cuts: vec![i], used: c, cut_to_length: false });
                    }
                }
                left -= c;
            }
            for r in &mut rs {
                r.length = fit(r.used).unwrap_or(r.length);
            }
            if best.as_ref().is_none_or(|b| score(&rs) < score(b)) {
                best = Some(rs);
            }
        }
        best.unwrap_or_default()
    };
    let mut out = best;
    out.extend(reels);
    (out, method)
}

/// One reel plan group: a floor, a size (or a cable type), insulation and colour; its cuts.
#[derive(Clone, Debug, Default)]
struct ReelGroup {
    level: String,
    kind: &'static str,
    size: String,
    ty: String,
    color: String,
    cuts: Vec<(String, f64)>,
    reels: Vec<Reel>,
    method: &'static str,
    source: String,
}

fn reel_plan(paths: &[&PullPath]) -> Vec<ReelGroup> {
    let r = rules();
    let mut groups: BTreeMap<(String, String, String, String, bool), Vec<(String, f64)>> = BTreeMap::new();
    for p in paths {
        for (ci, c) in p.conductors.iter().enumerate() {
            let key = (
                p.level.clone(),
                if p.cable { c.ty.clone() } else { c.size.clone() },
                if p.cable { String::new() } else { c.ty.clone() },
                c.color.clone(),
                p.cable,
            );
            let v = groups.entry(key).or_default();
            for k in 0..(c.count.min(64) * p.conduits.clamp(1, 400)) {
                if v.len() < MAX_CUTS {
                    v.push((format!("{} {}/{}#{}", p.id, p.runs.first().map_or("", String::as_str), ci + 1, k + 1), c.cut_ft));
                }
            }
        }
    }
    groups
        .into_iter()
        .map(|((level, size, ty, color, cable), cuts)| {
            let kind = if cable { "cable" } else { "wire" };
            let (lengths, source) = r.reels_for(kind, &size);
            let lens: Vec<f64> = cuts.iter().map(|c| c.1).collect();
            let (reels, method) = pack(&lens, &lengths);
            ReelGroup { level, kind, size, ty, color, cuts, reels, method, source }
        })
        .collect()
}

fn reel_json(g: &ReelGroup) -> Value {
    let reel_ft: f64 = g.reels.iter().map(|r| r.length).sum();
    let used: f64 = g.reels.iter().map(|r| r.used).sum();
    json!({
        "level": g.level, "kind": g.kind, "size": g.size, "type": g.ty, "color": g.color,
        "cuts": g.cuts.len(), "cut_ft": r2(g.cuts.iter().map(|c| c.1).sum()),
        "reels": g.reels.iter().enumerate().map(|(k, r)| json!({
            "reel": k + 1, "length_ft": r2(r.length), "used_ft": r2(r.used), "left_ft": r2(r.length - r.used),
            "cut_to_length": r.cut_to_length,
            "cuts": r.cuts.iter().take(500).filter_map(|i| g.cuts.get(*i)).map(|(id, f)| json!({"id": id, "ft": r2(*f)})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "reel_ft": r2(reel_ft), "waste_ft": r2(reel_ft - used),
        "method": g.method, "source": g.source,
        "cut_to_length": g.reels.iter().filter(|r| r.cut_to_length).count(),
        "cut_to_length_source": rules().cut_to_length_source,
    })
}

// ─── Filters ─────────────────────────────────────────────────────────────────

fn level_ok(p: &Value, level: &str) -> bool {
    str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).is_none_or(|l| l.eq_ignore_ascii_case(level))
}

fn circuit_ok(p: &Value, id: &str) -> bool {
    str_param(p, "circuit").map(str::trim).filter(|c| !c.is_empty()).is_none_or(|c| id.to_ascii_uppercase().contains(&c.to_ascii_uppercase()))
}

fn max_of(cmd: &str, p: &Value) -> Result<usize> {
    Ok(count_param(cmd, p, "max", 0, MAX_LISTED as u64)?.map_or(DEFAULT_MAX, |n| n as usize))
}

/// The model's plan, or why there is none.
fn plan_of(s: &Session, cmd: &str) -> Result<Plan> {
    let (sp, _) = super::bspec::current(s, cmd)?;
    let d = s.doc()?;
    let pl = plan(d, &sp);
    if pl.runs.is_empty() {
        return Err(crate::EngineError::Disabled(
            cmd.into(),
            "the model has no electrical runs cut into their pieces: build the model first (model.build)".into(),
        ));
    }
    Ok(pl)
}

// ─── pull.schedule ───────────────────────────────────────────────────────────

const PULL_COLUMNS: [&str; 17] = [
    "level",
    "kind",
    "circuit or feeder",
    "panel",
    "from",
    "to",
    "raceway",
    "conduits",
    "raceway ft",
    "vertical ft",
    "bends deg",
    "pull boxes",
    "pulls (ft/deg)",
    "conductors",
    "cut ft each",
    "conductor ft",
    "runs",
];

fn pull_row(p: &PullPath) -> Vec<String> {
    let cut = p.conductors.iter().map(|c| ft(c.cut_ft)).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join("/");
    vec![
        p.level.clone(),
        p.kind.to_string(),
        p.id.clone(),
        p.panel.clone(),
        p.from.label.clone(),
        p.to.label.clone(),
        p.raceway.clone(),
        p.conduits.to_string(),
        ft(p.raceway_ft),
        ft(p.vertical_ft),
        ft(p.bends.iter().map(|b| b.deg).sum()),
        p.boxes.len().to_string(),
        p.pulls().iter().map(|(_, _, f, d)| format!("{}/{}", ft(*f), ft(*d))).collect::<Vec<_>>().join(" "),
        conductors_text(p),
        cut,
        ft(p.conductor_ft()),
        p.runs.join(" "),
    ]
}

const REEL_COLUMNS: [&str; 11] = ["level", "kind", "size", "type", "color", "reel", "reel ft", "used ft", "left ft", "cut to length", "cuts (ft)"];

fn reel_rows(g: &ReelGroup) -> Vec<Vec<String>> {
    g.reels
        .iter()
        .enumerate()
        .map(|(k, r)| {
            vec![
                g.level.clone(),
                g.kind.to_string(),
                g.size.clone(),
                g.ty.clone(),
                g.color.clone(),
                (k + 1).to_string(),
                ft(r.length),
                ft(r.used),
                ft(r.length - r.used),
                if r.cut_to_length { "yes".into() } else { String::new() },
                r.cuts.iter().take(400).filter_map(|i| g.cuts.get(*i)).map(|(_, f)| ft(*f)).collect::<Vec<_>>().join(" "),
            ]
        })
        .collect()
}

fn with_header(cols: &[&str], rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let mut out = vec![cols.iter().map(|c| c.to_string()).collect::<Vec<_>>()];
    out.extend(rows);
    out
}

fn run_schedule(s: &mut Session, p: &Value) -> Result<Value> {
    super::exchange::guard_params(s, CMD_PULLS, p, &["file", "reels_file"])?;
    let max = max_of(CMD_PULLS, p)?;
    let pl = plan_of(s, CMD_PULLS)?;
    let r = rules();
    let paths: Vec<&PullPath> = pl.paths.iter().filter(|x| level_ok(p, &x.level) && circuit_ok(p, &x.id)).collect();
    let reels = reel_plan(&paths);
    // Per circuit (a feeder: per leg).
    let mut circuits: BTreeMap<(String, String, String), (usize, f64, f64, usize)> = BTreeMap::new();
    for x in &paths {
        let c = circuits.entry((x.level.clone(), x.panel.clone(), x.id.clone())).or_default();
        c.0 += x.conduits.max(1) as usize;
        c.1 += x.raceway_ft * f64::from(x.conduits.max(1));
        c.2 += x.conductor_ft();
        c.3 += x.boxes.len() * x.conduits.max(1) as usize;
    }
    // The wire reconciled with the takeoff, by size (a filter narrows the pulls, not the takeoff:
    // the reconciliation is the whole model's).
    let all: Vec<&PullPath> = pl.paths.iter().collect();
    let wire = reconcile(&pl, &all);
    let reel_ft: f64 = reels.iter().flat_map(|g| g.reels.iter()).map(|x| x.length).sum();
    let used: f64 = reels.iter().flat_map(|g| g.reels.iter()).map(|x| x.used).sum();
    let mut out = json!({
        "pulls": paths.iter().take(max).map(|x| path_json(x, pl.upf)).collect::<Vec<_>>(),
        "circuits": circuits.iter().take(max).map(|((level, panel, id), (n, rf, cf, b))| json!({
            "circuit": id, "level": level, "panel": panel, "paths": n, "raceway_ft": r2(*rf), "conductor_ft": r2(*cf), "pull_boxes": b,
        })).collect::<Vec<_>>(),
        "reels": reels.iter().take(max).map(reel_json).collect::<Vec<_>>(),
        "wire": wire,
        "totals": {
            "pulls": paths.len(),
            "circuits": circuits.keys().filter(|k| !paths.iter().any(|x| x.kind == "feeder" && x.id == k.2)).count(),
            "feeders": paths.iter().filter(|x| x.kind == "feeder").map(|x| x.id.split(" to ").next().unwrap_or_default().to_string()).collect::<BTreeSet<_>>().len(),
            "raceway_ft": r2(paths.iter().map(|x| x.raceway_ft * f64::from(x.conduits.max(1))).sum()),
            "vertical_ft": r2(paths.iter().map(|x| x.vertical_ft * f64::from(x.conduits.max(1))).sum()),
            "conductor_ft": r2(paths.iter().map(|x| x.conductor_ft()).sum()),
            "pull_boxes": paths.iter().map(|x| x.boxes.len() * x.conduits.max(1) as usize).sum::<usize>(),
            "estimated_paths": paths.iter().filter(|x| x.estimated).count(),
            "reel_groups": reels.len(),
            "reels": reels.iter().map(|g| g.reels.len()).sum::<usize>(),
            "reel_ft": r2(reel_ft),
            "waste_ft": r2(reel_ft - used),
        },
        "feeders_not_in_takeoff": {
            "paths": paths.iter().filter(|x| x.from_spec).map(|x| x.conduits.max(1) as usize).sum::<usize>(),
            "raceway_ft": r2(paths.iter().filter(|x| x.from_spec).map(|x| x.raceway_ft * f64::from(x.conduits.max(1))).sum()),
            "conductor_ft": r2(paths.iter().filter(|x| x.from_spec).map(|x| x.conductor_ft()).sum()),
            "why": "routed feeders the model holds as runs, not cut into pieces: their raceway and conductors are in no takeoff yet",
        },
        "unrouted_feeders": pl.unrouted_feeders,
        "empty_raceways": {"runs": pl.empty_raceways.0, "raceway_ft": r2(pl.empty_raceways.1), "why": "raceway on no circuit and no feeder: nothing to pull"},
        "rules": {
            "box_end_slack_ft": makeup(), "box_end_slack_source": r.box_end_slack_source,
            "gear_end_slack_ft": pos(r.gear_end_slack_ft), "gear_end_slack_source": r.gear_end_slack_source,
            "pull_box_loop_ft": pos(r.pull_box_loop_ft), "pull_box_loop_source": r.pull_box_loop_source,
            "pull_box_where": r.pull_box.place,
        },
    });
    if paths.len() > max {
        out["more"] = json!(paths.len() - max);
    }
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        let rows = with_header(&PULL_COLUMNS, paths.iter().map(|x| pull_row(x)).collect());
        out["file"] = super::propuse::write_table(CMD_PULLS, path, &rows, "Pull Schedule")?;
    }
    if let Some(path) = str_param(p, "reels_file").map(str::trim).filter(|x| !x.is_empty()) {
        let rows = with_header(&REEL_COLUMNS, reels.iter().flat_map(reel_rows).collect());
        out["reels_file"] = super::propuse::write_table(CMD_PULLS, path, &rows, "Reel Plan")?;
    }
    Ok(out)
}

/// The pulls' wire against the takeoff's, by size: the takeoff's, less what no branch pull holds
/// (a feeder's wire in the model), plus the slack the takeoff doesn't count (the gear ends' over
/// the box makeup, the pull boxes' loops) and the feeders' conductors along their routes (cut
/// from their schedules), is what the pulls cut.
fn reconcile(pl: &Plan, paths: &[&PullPath]) -> Value {
    #[derive(Default)]
    struct T {
        takeoff: f64,
        pulled: f64,
        not_in: f64,
        gear: f64,
        loops: f64,
        feeder: f64,
        from_model: f64,
    }
    let mut by: BTreeMap<String, T> = BTreeMap::new();
    for (k, v) in &pl.takeoff_wire {
        by.entry(k.clone()).or_default().takeoff += v;
    }
    for (k, v) in &pl.not_in_pulls {
        by.entry(k.clone()).or_default().not_in += v;
    }
    let extra_gear = rules().gear_extra();
    let loop_ft = pos(rules().pull_box_loop_ft);
    for p in paths {
        let n = f64::from(p.conduits.max(1));
        for c in &p.conductors {
            let key = if p.cable { c.ty.clone() } else { c.size.clone() };
            let t = by.entry(key).or_default();
            let count = f64::from(c.count) * n;
            t.pulled += c.cut_ft * count;
            if p.kind == "feeder" && !p.cable {
                t.feeder += c.cut_ft * count;
            } else {
                t.from_model += c.takeoff_ft * count;
                t.gear += extra_gear * f64::from(p.gear_ends()) * count;
                if !p.cable {
                    t.loops += loop_ft * p.boxes.len() as f64 * count;
                }
            }
        }
    }
    // Model wire neither in a pull nor set apart (a run on no circuit): listed as not pulled.
    let mut rows = Vec::new();
    let mut all_ok = true;
    let (mut tt, mut tp) = (0.0, 0.0);
    let (mut tn, mut tg, mut tl, mut tf) = (0.0, 0.0, 0.0, 0.0);
    for (k, t) in &by {
        let other = t.takeoff - t.not_in - t.from_model;
        let expect = t.takeoff - t.not_in - other + t.gear + t.loops + t.feeder;
        let ok = (expect - t.pulled).abs() < 0.05 && other.abs() < 0.05;
        all_ok &= ok;
        tt += t.takeoff;
        tp += t.pulled;
        tn += t.not_in + other;
        tg += t.gear;
        tl += t.loops;
        tf += t.feeder;
        rows.push(json!({
            "size": k,
            "takeoff_ft": r2(t.takeoff),
            "pulled_ft": r2(t.pulled),
            "not_in_pulls_ft": r2(t.not_in),
            "other_not_pulled_ft": r2(other),
            "gear_end_slack_ft": r2(t.gear),
            "pull_box_loops_ft": r2(t.loops),
            "feeder_conductors_ft": r2(t.feeder),
            "matches": ok,
        }));
    }
    json!({
        "by_size": rows,
        "takeoff_ft": r2(tt),
        "pulled_ft": r2(tp),
        "terms": {
            "not_in_pulls_ft": r2(tn), "not_in_pulls": "the model's wire no branch pull holds: a feeder's (its entry stubs, its routed raceways' wire: its pull is cut from its schedule gear to gear), a run on no circuit",
            "gear_end_slack_ft": r2(tg), "gear_end_slack": rules().gear_end_slack_source,
            "pull_box_loops_ft": r2(tl), "pull_box_loops": rules().pull_box_loop_source,
            "feeder_conductors_ft": r2(tf), "feeder_conductors": "each routed feeder's conductors (its schedule's) along its route, cut with its slack",
        },
        "identity": "pulled = takeoff - not_in_pulls + gear_end_slack + pull_box_loops + feeder_conductors",
        "matches": all_ok,
    })
}

// ─── pull.cutlist ────────────────────────────────────────────────────────────

/// The raceway kinds the cut list counts (field bends are the crew's: listed, nothing bought).
const CUT_KINDS: [&str; 9] = ["conduit", "coupling", "connector", "elbow", "offset", "bend", "strap", "locknut", "bushing"];

fn angles_text(m: &BTreeMap<String, u32>) -> String {
    m.iter().map(|(a, n)| format!("{n}x{a}")).collect::<Vec<_>>().join(" ")
}

fn cut_json(rp: &RunPlan) -> Value {
    json!({
        "run": rp.run, "level": rp.level, "circuit": rp.circuit, "system": rp.system,
        "raceway": rp.raceway_text(), "stock_ft": rp.stock_ft, "raceway_ft": r2(rp.raceway_ft), "vertical_ft": r2(rp.vertical_ft),
        "full_sticks": rp.full_sticks, "cut_pieces": rp.cut_pieces.iter().map(|c| r2(*c)).collect::<Vec<_>>(),
        "couplings": rp.count("coupling"), "connectors": rp.count("connector"),
        "elbows": rp.by_angle("elbow", false), "offsets": rp.by_angle("offset", false).values().sum::<u32>(),
        "field_bends": rp.by_angle("bend", true), "field_offsets": rp.by_angle("offset", true).values().sum::<u32>(),
        "straps": rp.count("strap"), "locknuts": rp.count("locknut"), "bushings": rp.count("bushing"),
        "bends_deg": r2(rp.bends_deg()), "estimated": rp.estimated,
    })
}

const CUT_COLUMNS: [&str; 17] = [
    "level",
    "run",
    "circuit",
    "raceway",
    "raceway ft",
    "vertical ft",
    "full sticks",
    "cut pieces ft",
    "couplings",
    "connectors",
    "elbows",
    "offsets",
    "field bends",
    "field offsets",
    "straps",
    "locknuts",
    "bushings",
];

fn cut_row(rp: &RunPlan) -> Vec<String> {
    vec![
        rp.level.clone(),
        rp.run.clone(),
        rp.circuit.clone(),
        rp.raceway_text(),
        ft(rp.raceway_ft),
        ft(rp.vertical_ft),
        rp.full_sticks.to_string(),
        rp.cut_pieces.iter().map(|c| ft(*c)).collect::<Vec<_>>().join(" "),
        rp.count("coupling").to_string(),
        rp.count("connector").to_string(),
        angles_text(&rp.by_angle("elbow", false)),
        rp.by_angle("offset", false).values().sum::<u32>().to_string(),
        angles_text(&rp.by_angle("bend", true)),
        rp.by_angle("offset", true).values().sum::<u32>().to_string(),
        rp.count("strap").to_string(),
        rp.count("locknut").to_string(),
        rp.count("bushing").to_string(),
    ]
}

/// The cut list's totals by floor, kind, item and size: feet of conduit, a count of the rest.
fn cut_totals(pl: &Plan, p: &Value) -> BTreeMap<(String, String, String, String), f64> {
    let mut out = BTreeMap::new();
    for x in pl.pieces.iter().filter(|x| CUT_KINDS.contains(&x.kind.as_str()) && level_ok(p, &x.level) && circuit_ok(p, &x.circuit)) {
        *out.entry((x.level.clone(), x.kind.clone(), x.item.clone(), x.size.clone())).or_insert(0.0) +=
            if x.kind == "conduit" { x.length_ft } else { 1.0 };
    }
    out
}

/// Each total against another count of the same pieces: matches, how many compared, the ones
/// that differ (to the cent of a foot; `lines`: the lines of theirs a total adds, each rounded
/// to it).
fn compare(
    mine: &BTreeMap<(String, String, String, String), f64>,
    theirs: &BTreeMap<(String, String, String, String), f64>,
    lines: &BTreeMap<(String, String, String, String), usize>,
) -> Value {
    let keys: BTreeSet<&(String, String, String, String)> = mine.keys().chain(theirs.keys()).collect();
    let mut diffs = Vec::new();
    for k in &keys {
        let (a, b) = (mine.get(*k).copied().unwrap_or(0.0), theirs.get(*k).copied().unwrap_or(0.0));
        let tol = 0.005 * lines.get(*k).copied().unwrap_or(0).max(1) as f64 + 1e-6;
        if (a - b).abs() > tol && diffs.len() < 50 {
            diffs.push(json!({"level": k.0, "kind": k.1, "item": k.2, "size": k.3, "cut_list": r2(a), "takeoff": r2(b)}));
        }
    }
    json!({"matches": diffs.is_empty(), "compared": keys.len(), "diffs": diffs})
}

fn run_cutlist(s: &mut Session, p: &Value) -> Result<Value> {
    super::exchange::guard_params(s, CMD_CUTS, p, &["file"])?;
    let max = max_of(CMD_CUTS, p)?;
    let pl = plan_of(s, CMD_CUTS)?;
    let runs: Vec<&RunPlan> = pl.runs.iter().filter(|r| !r.cable && level_ok(p, &r.level) && circuit_ok(p, &r.circuit)).collect();
    let mine = cut_totals(&pl, p);
    // The conduit takeoff's lines (the same pieces counted by the takeoff) and the model's count
    // index (what bim.takeoff and the bill of materials read).
    let mut theirs: BTreeMap<(String, String, String, String), f64> = BTreeMap::new();
    let mut lines_of: BTreeMap<(String, String, String, String), usize> = BTreeMap::new();
    let by_circuit = str_param(p, "circuit").is_some_and(|c| !c.trim().is_empty());
    let groups = if by_circuit { json!(["level", "circuit"]) } else { json!(["level"]) };
    let ct = s.execute("conduit.takeoff", &json!({"group_by": groups, "price": false}))?;
    for l in ct["lines"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let g = |k: &str| l[k].as_str().unwrap_or_default().to_string();
        let kind = g("kind");
        if !CUT_KINDS.contains(&kind.as_str()) || !level_ok(p, &g("level")) || !circuit_ok(p, &g("circuit")) {
            continue;
        }
        let key = (g("level"), kind, g("item"), g("size"));
        *lines_of.entry(key.clone()).or_insert(0) += 1;
        *theirs.entry(key).or_insert(0.0) += l["qty"].as_f64().unwrap_or(0.0);
    }
    let mut index: BTreeMap<(String, String, String, String), f64> = BTreeMap::new();
    let stored = super::mepatoms::stored(s.doc()?);
    if let Some(ix) = &stored {
        for (k, t) in ix.rows.iter().filter(|(k, _)| CUT_KINDS.contains(&k.kind.as_str()) && k.trade.is_empty()) {
            if level_ok(p, &k.level) && circuit_ok(p, &k.circuit) {
                *index.entry((k.level.clone(), k.kind.clone(), k.item.clone(), k.size.clone())).or_insert(0.0) +=
                    if k.kind == "conduit" { t.length_ft } else { t.count as f64 };
            }
        }
    }
    let r_conduit = compare(&mine, &theirs, &lines_of);
    let r_index =
        if stored.is_some() { compare(&mine, &index, &BTreeMap::new()) } else { json!({"matches": false, "why": "the model keeps no count index"}) };
    let by_size: Vec<Value> = mine
        .iter()
        .map(|((level, kind, item, size), q)| {
            json!({"level": level, "kind": kind, "item": item, "size": size, "unit": if kind == "conduit" { "ft" } else { "each" }, "qty": r2(*q),
                   "labor_only": super::mepatoms::is_labor_item(item)})
        })
        .collect();
    let mut out = json!({
        "runs": runs.iter().take(max).map(|r| cut_json(r)).collect::<Vec<_>>(),
        "by_size_floor": by_size,
        "takeoff_check": {"conduit_takeoff": r_conduit, "model_index": r_index},
        "feeders_not_cut": {
            "paths": pl.paths.iter().filter(|x| x.from_spec && level_ok(p, &x.level)).map(|x| x.conduits.max(1) as usize).sum::<usize>(),
            "raceway_ft": r2(pl.paths.iter().filter(|x| x.from_spec && level_ok(p, &x.level)).map(|x| x.raceway_ft * f64::from(x.conduits.max(1))).sum()),
            "why": "routed feeders the model holds as runs, not cut into pieces: neither this list nor the takeoff has their sticks and fittings",
        },
        "totals": {
            "runs": runs.len(),
            "sticks": runs.iter().map(|r| r.full_sticks as usize + r.cut_pieces.len()).sum::<usize>(),
            "full_sticks": runs.iter().map(|r| r.full_sticks as usize).sum::<usize>(),
            "cut_pieces": runs.iter().map(|r| r.cut_pieces.len()).sum::<usize>(),
            "raceway_ft": r2(runs.iter().map(|r| r.raceway_ft).sum()),
            "vertical_ft": r2(runs.iter().map(|r| r.vertical_ft).sum()),
        },
    });
    if runs.len() > max {
        out["more"] = json!(runs.len() - max);
    }
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        let rows = with_header(&CUT_COLUMNS, runs.iter().map(|r| cut_row(r)).collect());
        out["file"] = super::propuse::write_table(CMD_CUTS, path, &rows, "Cut and Bend List")?;
    }
    Ok(out)
}

// ─── pull.release ────────────────────────────────────────────────────────────

/// One release line, priced or not.
#[derive(Clone, Debug, Default)]
struct Release {
    level: String,
    area: String,
    phase: String,
    category: String,
    item: String,
    size: String,
    unit: &'static str,
    qty: f64,
    price: Option<(f64, Vec<String>, String, String)>,
    unpriced: String,
}

fn category_of(kind: &str) -> &'static str {
    match kind {
        "conduit" => "conduit",
        "box" | "ring" => "boxes",
        "cable" | "wire" => "wire",
        "device" => "devices",
        "plate" => "plates",
        _ => "conduit fittings",
    }
}

fn release_lines(pl: &Plan, p: &Value, by_room: bool, book: &super::pricebook::Book) -> Vec<Release> {
    let r = rules();
    let mut groups: BTreeMap<(String, String, String, String, String, String, &'static str), f64> = BTreeMap::new();
    let mut add = |level: &str, room: &str, kind: &str, item: String, size: String, unit: &'static str, q: f64| {
        let phase = r.phase_of(kind);
        let key = (level.to_string(), if by_room { room.to_string() } else { String::new() }, phase, category_of(kind).to_string(), item, size, unit);
        *groups.entry(key).or_insert(0.0) += q;
    };
    for x in pl.pieces.iter().filter(|x| level_ok(p, &x.level) && super::mepatoms::PIECES.contains(&x.kind.as_str())) {
        if super::mepatoms::is_labor_item(&x.item) {
            continue;
        }
        let by_ft = matches!(x.kind.as_str(), "conduit" | "cable" | "wire");
        // A cable is bought by its type (`NM-B cable`, `12/2 w/G`), as the bill of materials does.
        let size = if x.kind == "cable" { x.ty.split_once(' ').map_or_else(|| x.size.clone(), |(_, rest)| rest.to_string()) } else { x.size.clone() };
        let item = if x.item.is_empty() { x.kind.clone() } else { x.item.clone() };
        add(&x.level, &x.room, &x.kind, item, size, if by_ft { "ft" } else { "each" }, if by_ft { x.length_ft } else { 1.0 });
    }
    for dv in pl.devices.iter().filter(|dv| level_ok(p, &dv.level)) {
        let item = format!("{} {}", dv.kind.replace('_', " "), dv.mark).trim().to_string();
        add(&dv.level, &dv.room, "device", item, String::new(), "each", 1.0);
        if r.plate_kinds.iter().any(|k| k == &dv.kind) {
            add(&dv.level, &dv.room, "plate", r.plate_item.clone(), r.plate_size.clone(), "each", 1.0);
        }
    }
    let phase_want = str_param(p, "phase").map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty());
    groups
        .into_iter()
        .filter(|(k, _)| phase_want.as_deref().is_none_or(|w| k.2.eq_ignore_ascii_case(w)))
        .map(|((level, area, phase, category, item, size, unit), qty)| {
            let mut l = Release { level, area, phase, category, item, size, unit, qty, ..Release::default() };
            match super::pricebook::lookup(book, &l.item, &l.size, unit) {
                Some(super::pricebook::Found::Priced { material, parts, date }) => {
                    let keys = parts.iter().map(|c| c.quote.key.clone()).collect();
                    l.price = Some((material, keys, date, super::pricebook::source_text(&parts)));
                }
                Some(super::pricebook::Found::Excluded(why)) => l.unpriced = format!("excluded in the price book: {why}"),
                None => {
                    let mapped = book.map.iter().any(|m| m.item.eq_ignore_ascii_case(&l.item));
                    l.unpriced = if mapped {
                        format!(
                            "the Item Map has {} but not size {} (or its key has no quote in this unit)",
                            l.item,
                            if l.size.is_empty() { "(none)" } else { &l.size }
                        )
                    } else {
                        format!("no Item Map row for {} (pricebook.map)", l.item)
                    };
                }
            }
            l
        })
        .collect()
}

fn release_json(l: &Release) -> Value {
    let mut j = json!({
        "level": l.level, "area": l.area, "phase": l.phase, "category": l.category, "item": l.item, "size": l.size,
        "unit": l.unit, "qty": r2(l.qty),
    });
    match &l.price {
        Some((u, keys, date, src)) => {
            j["unit_price"] = json!(u);
            j["extended"] = json!(r2(u * l.qty));
            j["price_keys"] = json!(keys);
            j["price_date"] = json!(date);
            j["price_source"] = json!(src);
        }
        None => j["unpriced"] = json!(l.unpriced),
    }
    j
}

const RELEASE_COLUMNS: [&str; 12] =
    ["level", "area", "phase", "category", "item", "size", "unit", "qty", "price keys", "unit price", "extended", "price source / why unpriced"];

fn release_row(l: &Release) -> Vec<String> {
    let (keys, unit, ext, src) = match &l.price {
        Some((u, k, _, s)) => (k.join("+"), format!("{u:.4}"), format!("{:.2}", u * l.qty), s.clone()),
        None => (String::new(), String::new(), String::new(), l.unpriced.clone()),
    };
    vec![
        l.level.clone(),
        l.area.clone(),
        l.phase.clone(),
        l.category.clone(),
        l.item.clone(),
        l.size.clone(),
        l.unit.to_string(),
        ft(l.qty),
        keys,
        unit,
        ext,
        src,
    ]
}

fn run_release(s: &mut Session, p: &Value) -> Result<Value> {
    super::exchange::guard_params(s, CMD_RELEASE, p, &["file"])?;
    let max = max_of(CMD_RELEASE, p)?;
    let by_room = match str_param(p, "by").map(|x| x.trim().to_ascii_lowercase()) {
        None => false,
        Some(b) if b.is_empty() || b == "level" || b == "floor" => false,
        Some(b) if b == "room" || b == "area" => true,
        Some(_) => return Err(bad(CMD_RELEASE, "`by` is level (per floor) or room (per floor and room)")),
    };
    if let Some(ph) = str_param(p, "phase").map(str::trim).filter(|x| !x.is_empty())
        && !rules().phases.keys().any(|k| k.eq_ignore_ascii_case(ph))
    {
        return Err(bad(CMD_RELEASE, format!("`phase` is one of {}", rules().phases.keys().cloned().collect::<Vec<_>>().join(", "))));
    }
    let pl = plan_of(s, CMD_RELEASE)?;
    let book = super::pricebook::load(str_param(p, "price_folder"));
    let lines = release_lines(&pl, p, by_room, &book);
    let mut totals: BTreeMap<(String, String, String), (usize, usize, f64)> = BTreeMap::new();
    for l in &lines {
        let t = totals.entry((l.level.clone(), l.area.clone(), l.phase.clone())).or_default();
        t.0 += 1;
        if let Some((u, _, _, _)) = &l.price {
            t.1 += 1;
            t.2 += u * l.qty;
        }
    }
    let mut out = json!({
        "lines": lines.iter().take(max).map(release_json).collect::<Vec<_>>(),
        "totals": totals.iter().map(|((level, area, phase), (n, priced, ext))| {
            let mut j = json!({"level": level, "phase": phase, "lines": n, "priced": priced, "unpriced": n - priced, "extended": r2(*ext)});
            if by_room {
                j["area"] = json!(area);
            }
            j
        }).collect::<Vec<_>>(),
        "unpriced": lines.iter().filter(|l| l.price.is_none()).count(),
        "extended": r2(lines.iter().filter_map(|l| l.price.as_ref().map(|x| x.0 * l.qty)).sum()),
        "phases_source": rules().phases_source,
        "plate_source": rules().plate_source,
        "price_book": book.folder.as_ref().map(|f| f.to_string_lossy().into_owned()),
    });
    if lines.len() > max {
        out["more"] = json!(lines.len() - max);
    }
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        let rows = with_header(&RELEASE_COLUMNS, lines.iter().map(release_row).collect());
        out["file"] = super::propuse::write_table(CMD_RELEASE, path, &rows, "Release List")?;
    }
    Ok(out)
}

// ─── pull.sheets ─────────────────────────────────────────────────────────────

/// A table on a sheet: its title, columns (name, width in inches) and rows.
struct Table {
    title: String,
    columns: Vec<(&'static str, f64)>,
    rows: Vec<Vec<String>>,
}

/// Column widths (inches) for a table's columns: shared out by the longest text in each, within
/// the sheet's width.
fn widths(cols: &[&'static str], rows: &[Vec<String>]) -> Vec<(&'static str, f64)> {
    let avail = PAPER[0] - 2.0 * MARGIN;
    let want: Vec<f64> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let longest = rows.iter().take(2000).filter_map(|r| r.get(i)).map(|t| t.chars().count()).max().unwrap_or(0).max(c.chars().count());
            (longest.min(60) as f64 + 1.0) * CHAR_W
        })
        .collect();
    let total: f64 = want.iter().sum();
    let k = if total > avail { avail / total } else { 1.0 };
    cols.iter().zip(want).map(|(c, w)| (*c, w * k)).collect()
}

/// Text cut to fit a width.
fn fit(t: &str, w: f64) -> String {
    let n = ((w / CHAR_W).floor() as usize).saturating_sub(1).max(1);
    if t.chars().count() <= n { t.to_string() } else { t.chars().take(n.saturating_sub(1)).collect::<String>() + "~" }
}

fn text_at(at: [f64; 2], t: &str, h: f64) -> EntityKind {
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
    })
}

/// A floor's tables laid out on as many sheets as they fill: each sheet `(rows, page)` as the
/// text items to place (paper inches) and its row count.
fn lay_out(tables: &[Table]) -> Vec<Vec<([f64; 2], String, f64)>> {
    let top = PAPER[1] - MARGIN;
    let mut pages: Vec<Vec<([f64; 2], String, f64)>> = vec![Vec::new()];
    let mut y = top;
    for t in tables {
        let header = |page: &mut Vec<([f64; 2], String, f64)>, y: &mut f64, cont: bool| {
            let title = if cont { format!("{} (continued)", t.title) } else { t.title.clone() };
            page.push(([MARGIN, *y - 0.18], title, 0.16));
            *y -= 0.3;
            let mut x = MARGIN;
            for (c, w) in &t.columns {
                page.push(([x, *y - TEXT_H], fit(&c.to_ascii_uppercase(), *w), TEXT_H));
                x += w;
            }
            *y -= ROW + 0.05;
        };
        if y - 0.5 - 2.0 * ROW < STRIP {
            if pages.len() >= MAX_PAGES {
                break;
            }
            pages.push(Vec::new());
            y = top;
        }
        if let Some(page) = pages.last_mut() {
            header(page, &mut y, false);
        }
        if t.rows.is_empty()
            && let Some(page) = pages.last_mut()
        {
            page.push(([MARGIN, y - TEXT_H], "(none)".into(), TEXT_H));
            y -= ROW;
        }
        for r in &t.rows {
            if y - ROW < STRIP {
                if pages.len() >= MAX_PAGES {
                    return pages;
                }
                pages.push(Vec::new());
                y = top;
                if let Some(page) = pages.last_mut() {
                    header(page, &mut y, true);
                }
            }
            let Some(page) = pages.last_mut() else { break };
            let mut x = MARGIN;
            for ((_, w), cell) in t.columns.iter().zip(r.iter()) {
                if !cell.trim().is_empty() {
                    page.push(([x, y - TEXT_H], fit(cell, *w), TEXT_H));
                }
                x += w;
            }
            y -= ROW;
        }
        y -= 0.35;
    }
    pages
}

fn safe(t: &str) -> String {
    t.chars().map(|c| if super::layout::BAD_CHARS.contains(&c) || c.is_control() || c == ' ' { '-' } else { c }).collect()
}

fn run_sheets(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| run_sheets_inner(s, p))
}

fn run_sheets_inner(s: &mut Session, p: &Value) -> Result<Value> {
    let folder = str_param(p, "folder").map(str::trim).filter(|x| !x.is_empty()).map(std::path::PathBuf::from);
    let pdf = bool_or(p, "pdf", false);
    if pdf && folder.is_none() {
        return Err(bad(CMD_SHEETS, "`pdf: true` writes into `folder`: name the folder"));
    }
    let pl = plan_of(s, CMD_SHEETS)?;
    let book = super::pricebook::load(str_param(p, "price_folder"));
    let title = str_param(p, "title").map(str::trim).filter(|t| !t.is_empty()).map(|t| t.chars().take(80).collect::<String>());
    let mut floors = Vec::new();
    let levels: Vec<String> = pl.levels.iter().filter(|l| level_ok(p, l)).cloned().collect();
    for level in levels.iter().take(200) {
        let at = json!({ "level": level });
        let paths: Vec<&PullPath> = pl.paths.iter().filter(|x| x.level.eq_ignore_ascii_case(level)).collect();
        let runs: Vec<&RunPlan> = pl.runs.iter().filter(|r| !r.cable && r.level.eq_ignore_ascii_case(level)).collect();
        let release = release_lines(&pl, &at, false, &book);
        if paths.is_empty() && runs.is_empty() && release.is_empty() {
            continue;
        }
        let reels = reel_plan(&paths);
        let totals = cut_totals(&pl, &at);
        let pull_rows: Vec<Vec<String>> = paths.iter().map(|x| pull_row(x)).collect();
        let reel_rows_all: Vec<Vec<String>> = reels.iter().flat_map(reel_rows).collect();
        let cut_rows: Vec<Vec<String>> = runs.iter().map(|r| cut_row(r)).collect();
        let total_rows: Vec<Vec<String>> = totals
            .iter()
            .map(|((_, kind, item, size), q)| {
                vec![kind.clone(), item.clone(), size.clone(), if kind == "conduit" { "ft" } else { "each" }.into(), ft(*q)]
            })
            .collect();
        let release_rows: Vec<Vec<String>> = release.iter().map(release_row).collect();
        // The CSV files: the same tables.
        let mut files = Vec::new();
        if let Some(dir) = &folder {
            for (name, cols, rows) in [
                ("pulls", &PULL_COLUMNS[..], &pull_rows),
                ("reels", &REEL_COLUMNS[..], &reel_rows_all),
                ("cuts", &CUT_COLUMNS[..], &cut_rows),
                ("release", &RELEASE_COLUMNS[..], &release_rows),
            ] {
                let path = dir.join(format!("{name}-{}.csv", safe(level))).to_string_lossy().into_owned();
                super::exchange::guard_output(s, CMD_SHEETS, p, &path)?;
                files.push(super::propuse::write_table(CMD_SHEETS, &path, &with_header(cols, rows.clone()), name)?);
            }
        }
        let tables = vec![
            Table { title: format!("WIRE PULL SCHEDULE - {level}"), columns: widths(&PULL_COLUMNS[..16], &pull_rows), rows: pull_rows.clone() },
            Table { title: format!("REEL PLAN - {level}"), columns: widths(&REEL_COLUMNS, &reel_rows_all), rows: reel_rows_all.clone() },
            Table { title: format!("CONDUIT CUT AND BEND LIST - {level}"), columns: widths(&CUT_COLUMNS, &cut_rows), rows: cut_rows.clone() },
            Table {
                title: format!("CONDUIT TOTALS BY SIZE - {level} (EQUAL TO THE TAKEOFF)"),
                columns: widths(&["kind", "item", "size", "unit", "qty"], &total_rows),
                rows: total_rows,
            },
            Table { title: format!("MATERIAL RELEASE - {level}"), columns: widths(&RELEASE_COLUMNS, &release_rows), rows: release_rows.clone() },
        ];
        let pages = lay_out(&tables);
        let n = pages.len();
        let mut layouts = Vec::new();
        for (k, page) in pages.into_iter().enumerate() {
            let name = format!("Field Paperwork {} - {} of {n}", safe(level), k + 1);
            let existing = s.doc()?.layout(&name).map(|l| l.name.clone());
            let layout = match existing {
                Some(old) => {
                    if let Some(l) = s.doc_mut()?.layouts.iter_mut().find(|l| l.name == old) {
                        l.entities = cadaddy_doc::EntityStore::default();
                    }
                    s.state_mut()?.selection.clear();
                    old
                }
                None => {
                    let made = s.execute("layout.new", &json!({ "name": name, "viewport": false }))?;
                    made["name"].as_str().unwrap_or(&name).to_string()
                }
            };
            s.execute("layout.set", &json!({ "name": layout }))?;
            s.execute(
                "pagesetup",
                &json!({ "layout": layout, "width": PAPER[0] * 25.4, "height": PAPER[1] * 25.4, "margins": [0, 0, 0, 0], "scale": 1, "center": false }),
            )?;
            let d = s.doc_mut()?;
            let space = Space::Paper(layout.clone());
            let common = || Common { layer: "E-PAPERWORK".into(), ..Common::default() };
            let frame = super::helpers::rect_vertices(Vec2::new(0.5, 0.5), Vec2::new(PAPER[0] - 0.5, PAPER[1] - 0.5));
            let _ = d.add(&space, common(), super::helpers::lwpoly(frame, true));
            let _ = d.add(&space, common(), super::helpers::line(Vec2::new(0.5, 2.0), Vec2::new(PAPER[0] - 0.5, 2.0)));
            let head = title.clone().unwrap_or_else(|| "FIELD PAPERWORK".into());
            for (at, t, h) in [
                ([1.0, 1.25], format!("{head} - {level}"), 0.3),
                ([1.0, 0.8], "GENERATED BY CADADDY FROM THE MODEL'S PIECES - CHECK IN THE FIELD BEFORE ORDERING OR CUTTING".to_string(), 0.14),
                ([PAPER[0] - 6.0, 1.0], format!("SHEET {} OF {n}", k + 1), 0.3),
            ] {
                let _ = d.add(&space, common(), text_at(at, &t, h));
            }
            for (at, t, h) in &page {
                let _ = d.add(&space, common(), text_at(*at, t, *h));
            }
            layouts.push(layout);
        }
        let mut f = json!({"level": level, "layouts": layouts, "rows": {"pulls": paths.len(), "reels": reel_rows_all.len(), "cuts": cut_rows.len(), "release": release_rows.len()}, "files": files});
        if pdf && let Some(dir) = &folder {
            let path = dir.join(format!("field-paperwork-{}.pdf", safe(level))).to_string_lossy().into_owned();
            super::exchange::guard_output(s, CMD_SHEETS, p, &path)?;
            s.execute("exportpdf", &json!({ "path": path, "layouts": f["layouts"], "force": true }))?;
            f["pdf"] = json!(path);
        }
        floors.push(f);
    }
    if floors.is_empty() {
        return Err(bad(CMD_SHEETS, "no floor has electrical pieces to put on a sheet"));
    }
    Ok(json!({ "floors": floors }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bend(s: f64, deg: f64) -> Bend {
        Bend { station_ft: s, deg, at: Vec3::ZERO }
    }

    #[test]
    fn five_quarter_bends_need_one_pull_box_before_the_fifth() {
        let bends: Vec<Bend> = (0..5).map(|i| bend(f64::from(i) * 10.0, 90.0)).collect();
        assert_eq!(pull_points(&bends, 360.0), vec![4]);
        // Three of 200 degrees can't share a pull: a box before the second and the third.
        let big: Vec<Bend> = (0..3).map(|i| bend(f64::from(i), 200.0)).collect();
        assert_eq!(pull_points(&big, 360.0), vec![1, 2]);
        // Four 90s are the limit, not over it; no limit, no boxes.
        assert!(pull_points(&bends[..4], 360.0).is_empty());
        assert!(pull_points(&bends, 0.0).is_empty());
    }

    #[test]
    fn reels_pack_with_the_least_waste() {
        // 2,000 ft in four cuts: two 1,000 ft reels exactly, not a 2,500 ft reel 500 short.
        let (r, m) = pack(&[600.0, 400.0, 450.0, 550.0], &[500.0, 1000.0, 2500.0]);
        assert_eq!(m, "exact");
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|x| (x.length - 1000.0).abs() < 1e-9 && (x.used - 1000.0).abs() < 1e-9), "{r:?}");
        // Many cuts by best fit: 30 of 100 ft on 500 / 2,500 ft reels waste nothing.
        let cuts = vec![100.0; 30];
        let (r, m) = pack(&cuts, &[500.0, 2500.0]);
        assert_eq!(m, "best_fit");
        let waste: f64 = r.iter().map(|x| x.length - x.used).sum();
        assert!(waste.abs() < 1e-6, "{r:?}");
        assert_eq!(r.iter().map(|x| x.cuts.len()).sum::<usize>(), 30);
        // A cut longer than every reel is ordered cut to length.
        let (r, _) = pack(&[1200.0, 100.0], &[500.0, 1000.0]);
        assert!(r.iter().any(|x| x.cut_to_length && (x.length - 1200.0).abs() < 1e-9), "{r:?}");
        // Hostile lengths never panic and every cut is placed.
        let (r, _) = pack(&[f64::NAN, -5.0, 10.0], &[f64::INFINITY, 0.0, -1.0]);
        assert_eq!(r.iter().map(|x| x.cuts.len()).sum::<usize>(), 3);
    }

    #[test]
    fn few_cuts_are_packed_exactly() {
        // 7 cuts, 2,000 ft, on 1,000 ft reels: two reels hold them, none left over.
        let cuts = [500.0, 400.0, 300.0, 300.0, 200.0, 200.0, 100.0];
        let (r, _) = pack(&cuts, &[1000.0]);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|x| (x.used - 1000.0).abs() < 1e-9), "{r:?}");
    }

    #[test]
    fn trade_sizes_and_conductor_sizes_read() {
        assert!((trade_in("1-1/4\"") - 1.25).abs() < 1e-9);
        assert!((trade_in("3/4\"") - 0.75).abs() < 1e-9);
        assert!((trade_in("2\"") - 2.0).abs() < 1e-9);
        assert_eq!(trade_in("junk"), 0.0);
        assert!(awg_at_least("#4", 4) && awg_at_least("#2/0", 4) && awg_at_least("250 kcmil", 4));
        assert!(!awg_at_least("#6", 4) && !awg_at_least("#12", 4));
    }

    #[test]
    fn pull_boxes_are_sized_by_the_conductors_and_the_raceway() {
        let w = |size: &str, role: &str| Wire { size: size.into(), role: role.into(), ..Wire::default() };
        // #4 and larger: 8 times the trade size (NEC 314.28(A)(1)).
        let (b, src) = pull_box_size("1-1/2\"", &[w("#1", "phase A"), w("#6", "ground")]);
        assert!(b.starts_with("12\" long"), "{b}");
        assert!(src.contains("314.28"), "{src}");
        // Small conductors in a small raceway: the trade's box; a large raceway: sized all the same.
        assert!(pull_box_size("3/4\"", &[w("#12", "phase A")]).0.contains("4\" square"));
        assert!(pull_box_size("1-1/4\"", &[w("#10", "phase A")]).0.contains("4-11/16"));
        assert!(pull_box_size("2\"", &[w("#8", "phase A")]).0.starts_with("16\" long"));
        // The ground's size doesn't make the box large.
        assert!(pull_box_size("3/4\"", &[w("#12", "phase A"), w("#4", "ground")]).0.contains("square"));
    }

    #[test]
    fn the_rules_name_their_sources() {
        let r = rules();
        assert!(r.gear_end_slack_ft > 0.0 && !r.gear_end_slack_source.is_empty());
        assert!(r.pull_box_loop_ft > 0.0 && !r.pull_box_loop_source.is_empty());
        assert!(!r.pull_box.straight_source.is_empty() && r.pull_box.straight_times > 0.0);
        assert!(r.reels.iter().all(|x| !x.source.is_empty() && !x.lengths_ft.is_empty()));
        assert_eq!(r.reels_for("wire", "#12").0, vec![500.0, 2500.0]);
        assert_eq!(r.reels_for("wire", "250 kcmil").0, vec![500.0, 1000.0]);
        assert_eq!(r.reels_for("cable", "NM 12/2 w/G").0, vec![250.0, 1000.0]);
        assert_eq!(r.phase_of("wire"), "rough-in");
        assert_eq!(r.phase_of("device"), "trim");
    }

    #[test]
    fn numbers_read_plainly() {
        assert_eq!(ft(-0.0), "0");
        assert_eq!(ft(-1e-9), "0");
        assert_eq!(ft(12.5), "12.5");
        assert_eq!(ft(f64::NAN), "0");
        assert_eq!(r2(-0.004), 0.0);
        assert!(r2(-0.004).is_sign_positive());
    }

    #[test]
    fn sheets_flow_onto_more_pages() {
        let rows: Vec<Vec<String>> = (0..400).map(|i| vec![i.to_string(), "x".into()]).collect();
        let t = Table { title: "T".into(), columns: widths(&["a", "b"], &rows), rows };
        let pages = lay_out(&[t]);
        assert!(pages.len() >= 3, "{}", pages.len());
        assert!(pages.iter().flatten().all(|(at, _, _)| at[1] >= STRIP - 1e-9 && at[1] <= PAPER[1]));
    }
}
