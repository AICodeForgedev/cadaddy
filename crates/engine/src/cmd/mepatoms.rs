//! MEP atoms (Q6157): the electrical runs as the pieces a crew installs, each one model element
//! with its own properties and 3D body, so a count is a lookup.
//!
//! - **Raceway**: each conduit stick (its trade size, material, length up to its stock length,
//!   a cut piece as its own length), each coupling, connector, elbow (by size and angle), offset,
//!   strap, locknut and bushing.
//! - **Cable** (NM, MC, AC, SER): each run from its source (a panel or the box upstream) to its
//!   device box, with its type, conductor size and count, length and supports.
//! - **Wire**: each conductor pulled in a raceway (size, insulation, colour, length).
//! - **Boxes**: each device's box (4" square, 4-11/16", device box, octagon, fan, floor box) with
//!   its volume, and the plaster ring on a wall box; a junction box where three runs meet.
//!
//! Every atom says what it connects to (`connects`: handles), its run, its circuit and its
//! system, so the chain gear → feeder → panel → branch → box → device is a walkable graph
//! (`bim.chain`). The model keeps a count index by kind, size, type, level, room, circuit and
//! system (`atoms/index.json` among the drawing's embedded files): `bim.count` answers from it,
//! the BOM reads it, `bim.atoms.edit` keeps it current.
//!
//! What each piece is made of comes from the rules data (`mepatoms.json`: stock lengths, NEC
//! support spacing, box volumes of NEC Table 314.16(A), conductor colours), every value with its
//! source; a material or wiring method the documents name wins over the defaults there.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_geom::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub(crate) mod build;

/// The rules data.
const RULES: &str = include_str!("mepatoms.json");
/// The count index among the drawing's embedded files.
pub(crate) const INDEX_FILE: &str = "atoms/index.json";
/// The property that marks an atom (its kind).
pub(crate) const ATOM_KEY: &str = "atom";
/// Most elements looked at, index rows kept, nodes walked (hostile drawings).
const MAX_ELEMENTS: usize = 2_000_000;
const MAX_ROWS: usize = 500_000;
const MAX_WALK: usize = 200_000;
/// Most pieces one run is cut into.
const MAX_PIECES: usize = 20_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.count", "Count Model Pieces", run_count)
            .params("{kind?: conduit|coupling|connector|elbow|bend|offset|strap|locknut|bushing|box|ring|cable|wire|breaker (a panel's, from its schedule row), a building piece (pour, rebar, mesh, tendon, anchor, chair, vapor_barrier, stud, track, plate, header, blocking, backing, sheathing, furring, joist, rim, truss, cmu, sheet, corner_bead, tape, compound, screws, batt, board, firestop, paint, flooring, base, tile, grid_main, grid_tee, wall_angle, frame, hardware_set, accessory, casework), a door or window, or a device kind (receptacle, switch, luminaire and the like), trade?: concrete|framing|drywall|insulation|firestopping|finishes|openings|specialties|masonry, item?: part of what it is bought as, size?: trade or conductor size (1\", #12, 4\" square, 5/8\", 3 5/8\"), type?: part of the type (EMT, NM 12/2, THHN, Type X), level?, room?, circuit?: panel and circuit (LP1-3), system?, group_by?: kind|item|size|type|level|room|circuit|system|trade, verify?: bool (recount every element and compare), refresh?: bool (rebuild the stored index from the elements)} → {count, length_ft, estimated (how many of them are on an estimated route), area_sf?, volume_cf?, volume_cy?, weight_lb?, pieces? (when an element stands for several: a box of screws, a run of chairs), rows?: [{value of the group_by dimension, count, length_ft, estimated, area_sf?, volume_cf?, volume_cy?, weight_lb?, pieces?}], from: index|recount, index_rows, ms, verify?: {count, length_ft, matches, recount_ms}}: counts of the built model's pieces from its stored index, no recompute")
            .noundo(),
        CommandSpec::new("bim.atoms.edit", "Edit Model Piece", run_edit)
            .params("{handle, length_ft?: feet (a conduit stick, a cable, a wire: its new length), size?: text (trade size, conductor size), type?: text (material, cable type)} → {handle, kind, before: {size, type, length_ft}, after: {size, type, length_ft}, index: {rows, count}}: change one piece of a run; its 3D body and the model's count index follow"),
        CommandSpec::new("bim.chain", "Walk the Feed", run_chain)
            .params("{handle, limit?: elements (2000)} → {from, breaker?: {handle, type, trip_a, poles, load, panel, source_row} (its circuit's breaker in the panel), path: [{handle, kind, item, size, circuit}], source: {handle, kind, tag}?, upstream: [{handle, tag}] (gear above the panel), length_ft (raceway or cable on the path)}: from a device (or any piece) back to its panel through its box, raceway or cable, then up the feeders")
            .noundo(),
    ]
}

// ─── Rules ───────────────────────────────────────────────────────────────────

/// One raceway material's pieces and supports.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Raceway {
    pub words: Vec<String>,
    pub stock_ft: f64,
    pub stock_source: String,
    pub strap_max_ft: f64,
    /// `[largest trade size (in), spacing (ft)]`, smallest first: overrides `strap_max_ft`.
    pub strap_by_size: Vec<[f64; 2]>,
    pub strap_from_end_ft: f64,
    pub strap_source: String,
    pub stick: String,
    pub coupling: String,
    pub connector: String,
    pub elbow: String,
    pub offset: String,
    pub strap: String,
    /// Trade sizes up to this (inches) are bent by hand (a field bend, no fitting); 0: always
    /// elbows.
    pub field_bend_max_in: f64,
    pub field_bend: String,
    pub field_offset: String,
    pub field_bend_source: String,
    /// The most degrees of bends between pull points.
    pub max_bends_deg: f64,
    pub max_bends_source: String,
}

impl Raceway {
    /// The longest span between supports for a trade size, feet.
    fn strap_spacing(&self, size_in: f64) -> f64 {
        self.strap_by_size.iter().find(|r| size_in <= r[0] + 1e-9).map_or(self.strap_max_ft, |r| r[1])
    }
}

/// One cable type's supports and fittings.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CableRule {
    pub words: Vec<String>,
    pub strap_max_ft: f64,
    pub strap_from_box_ft: f64,
    pub strap_source: String,
    pub item: String,
    pub strap: String,
    pub connector: String,
}

/// A box as stock: its item, size, depth, outside dimensions and volume.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BoxRule {
    pub item: String,
    pub size: String,
    pub depth_in: f64,
    /// Width, height, depth, inches.
    pub dims_in: [f64; 3],
    pub cubic_in: f64,
    pub material: String,
    pub source: String,
}

/// The box a kind of device takes, on raceway and on cable, and whether it takes a ring.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BoxChoice {
    pub raceway: String,
    pub cable: String,
    pub ring: bool,
}

/// A plaster ring as stock.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RingRule {
    pub item: String,
    pub size: String,
    pub depth_in: f64,
    pub source: String,
}

/// What conductors of one material are bought as (`feeder_conductors`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ConductorKind {
    pub insulation: String,
    pub item: String,
    pub ground_item: String,
}

/// One rule of whether a luminaire takes an outlet box: words in what its documents say of it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LuminaireBoxRule {
    pub words: Vec<String>,
    #[serde(rename = "box")]
    pub takes_box: bool,
    /// A cut sheet's words: read off the submittal (the schedule's mounting words are not).
    pub cut_sheet: bool,
    pub source: String,
}

/// Whether a luminaire takes an outlet box (`luminaire_box`): the first rule (in order) whose
/// words one of its `fields` holds decides; none: the default box.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LuminaireBox {
    pub fields: Vec<String>,
    pub rules: Vec<LuminaireBoxRule>,
    pub default_source: String,
}

/// What decided a luminaire's box: whether it takes one, the words found, the field they were in
/// and the document it came from (empty: the default), and the rule's source.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct BoxDecision {
    pub takes_box: bool,
    pub found: String,
    pub field: String,
    pub doc: String,
    pub source: String,
}

impl LuminaireBox {
    /// The decision from what a luminaire's documents say: `(field, value, document)` each.
    pub(crate) fn decide(&self, said: &[(String, String, String)]) -> BoxDecision {
        for r in &self.rules {
            for f in &self.fields {
                for (k, v, doc) in said.iter().filter(|s| &s.0 == f) {
                    let up = v.to_ascii_uppercase();
                    if let Some(w) = r.words.iter().find(|w| !w.is_empty() && up.contains(&w.to_ascii_uppercase())) {
                        return BoxDecision {
                            takes_box: r.takes_box,
                            found: w.to_ascii_lowercase(),
                            field: k.clone(),
                            doc: doc.clone(),
                            source: r.source.clone(),
                        };
                    }
                }
            }
        }
        BoxDecision { takes_box: true, source: self.default_source.clone(), ..BoxDecision::default() }
    }
    /// The first rule's words a cut sheet's lines hold (the junction box it says it has or needs).
    pub(crate) fn cut_sheet_words(&self, lines: &[String]) -> Option<String> {
        self.rules
            .iter()
            .filter(|r| r.cut_sheet)
            .find_map(|r| r.words.iter().find(|w| !w.is_empty() && lines.iter().any(|l| l.to_ascii_uppercase().contains(&w.to_ascii_uppercase()))))
            .map(|w| w.to_ascii_lowercase())
    }
}

/// The rules data (`mepatoms.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AtomRules {
    pub raceways: BTreeMap<String, Raceway>,
    pub cables: BTreeMap<String, CableRule>,
    pub boxes: BTreeMap<String, BoxRule>,
    pub box_for: BTreeMap<String, BoxChoice>,
    pub box_source: String,
    pub ring: RingRule,
    pub default_raceway: String,
    pub default_raceway_source: String,
    pub insulation: String,
    pub insulation_source: String,
    /// Each conductor's makeup at each box, feet: the code's minimum and the field allowance
    /// when the rules give them (their sum), else as given.
    pub makeup_ft: f64,
    pub makeup_min_in: f64,
    pub makeup_allowance_in: f64,
    pub makeup_source: String,
    /// Each conductor's makeup where it ends at a panel (to its breaker), feet, when its height
    /// isn't known; else the share of its height it runs down the can (and a box's makeup).
    pub panel_makeup_ft: f64,
    pub panel_makeup_height_share: f64,
    pub panel_makeup_source: String,
    pub bushing_min_awg: u32,
    pub bushing_item: String,
    pub bushing_source: String,
    pub locknut_item: String,
    pub locknut_source: String,
    pub default_voltage: String,
    pub default_voltage_source: String,
    pub colors: BTreeMap<String, BTreeMap<String, String>>,
    pub colors_source: String,
    pub phase_source: String,
    pub offset_max_ft: f64,
    pub joint_tol_in: f64,
    pub box_reach_ft: f64,
    pub panel_reach_ft: f64,
    pub feeder_makeup_ft: f64,
    pub feeder_makeup_source: String,
    pub feeder_conductors: BTreeMap<String, ConductorKind>,
    pub feeder_conductors_source: String,
    pub luminaire_box: LuminaireBox,
}

/// The rules data, read once.
pub(crate) fn rules() -> &'static AtomRules {
    static R: std::sync::OnceLock<AtomRules> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let mut r: AtomRules = serde_json::from_str(RULES).unwrap_or_default();
        let parts = r.makeup_min_in.max(0.0) + r.makeup_allowance_in.max(0.0);
        if parts.is_finite() && parts > 0.0 {
            r.makeup_ft = parts / 12.0;
        }
        r
    })
}

impl AtomRules {
    /// The raceway a name (a system, a tag, a wiring method) says, by its words.
    pub(crate) fn raceway_named(&self, text: &str) -> Option<(&str, &Raceway)> {
        let up = text.to_ascii_uppercase();
        self.raceways.iter().find(|(_, r)| r.words.iter().any(|w| has_word(&up, w))).map(|(k, r)| (k.as_str(), r))
    }
    /// The cable type a name says, by its words.
    pub(crate) fn cable_named(&self, text: &str) -> Option<(&str, &CableRule)> {
        let up = text.to_ascii_uppercase();
        self.cables.iter().find(|(_, r)| r.words.iter().any(|w| has_word(&up, w))).map(|(k, r)| (k.as_str(), r))
    }
    /// A device kind's box and whether it takes a ring, on raceway or cable.
    pub(crate) fn box_for(&self, kind: &str, on_cable: bool) -> Option<(&BoxRule, bool)> {
        let k = kind.replace('_', " ").to_ascii_lowercase();
        let choice = self.box_for.get(&k).or_else(|| {
            let plumbing = k.contains("plumbing") || k.contains("sprinkler");
            let fallback =
                if !plumbing && (k.contains("luminaire") || k.contains("light") || k.contains("fixture")) { "luminaire" } else { return None };
            self.box_for.get(fallback)
        })?;
        let id = if on_cable { &choice.cable } else { &choice.raceway };
        self.boxes.get(id).map(|b| (b, choice.ring))
    }
    /// A conductor's colour on a system (`120/208`), for a phase `A`/`B`/`C`, `N` or `G`.
    pub(crate) fn color(&self, voltage: &str, phase: &str) -> Option<&str> {
        self.colors.get(voltage).or_else(|| self.colors.get(&self.default_voltage)).and_then(|c| c.get(phase)).map(String::as_str)
    }
}

/// `word` standing as a word in `up` (both upper case): `EMT` in `3/4" EMT`, not `MC` in `EMC`.
fn has_word(up: &str, word: &str) -> bool {
    let w = word.to_ascii_uppercase();
    if w.is_empty() {
        return false;
    }
    let bytes = up.as_bytes();
    let mut from = 0;
    while let Some(i) = up.get(from..).and_then(|s| s.find(&w)).map(|i| i + from) {
        let before = i.checked_sub(1).and_then(|j| bytes.get(j)).is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = bytes.get(i + w.len()).is_none_or(|c| !c.is_ascii_alphanumeric());
        if before && after {
            return true;
        }
        from = i + 1;
    }
    false
}

// ─── Atoms ───────────────────────────────────────────────────────────────────

/// One piece as the build makes it (sheet units; `connects` are indices among its run's atoms
/// until the build gives them handles).
#[derive(Clone, Debug, Default)]
pub(crate) struct Atom {
    pub kind: &'static str,
    /// What it is bought as (`EMT coupling`).
    pub item: String,
    /// Its material or type (`EMT`, `NM 12/2 w/G`, `THHN`, `4" square`).
    pub ty: String,
    /// Its size as text (`1"`, `#12`, `4" square`).
    pub size: String,
    /// The diameter its body is drawn with, inches.
    pub body_in: f64,
    /// Feet (sticks, cables, wires; 0 for fittings).
    pub length_ft: f64,
    /// Its two ends (a fitting: its centre twice, `b` along its direction).
    pub a: Vec3,
    pub b: Vec3,
    /// Its centreline when it bends (a cable, a wire).
    pub path: Vec<Vec3>,
    pub angle_deg: f64,
    /// A stick cut from stock.
    pub cut: bool,
    pub connects: Vec<usize>,
    /// More properties (`cubic_in`, `color`, `conductors`…) with their sources.
    pub extra: Vec<(&'static str, String, String)>,
    /// Where what it is came from (a rule's citation).
    pub source: String,
}

impl Atom {
    pub(crate) fn fitting(kind: &'static str, item: String, ty: &str, size: &str, at: Vec3, dir: Vec3, body_in: f64, source: &str) -> Self {
        Atom { kind, item, ty: ty.into(), size: size.into(), body_in, a: at, b: at + dir, source: source.into(), ..Atom::default() }
    }
}

/// The pieces of one raceway run and its two end connectors (indices).
#[derive(Clone, Debug, Default)]
pub(crate) struct Pieces {
    pub atoms: Vec<Atom>,
    pub ends: [Option<usize>; 2],
    pub length_ft: f64,
    /// The degrees it turns between its ends (NEC 358.26 and its kin: 360 at most between pull
    /// points).
    pub bend_deg: f64,
}

/// A trade size as printed: `3/4"`, `1"`, `1-1/4"`.
pub(crate) fn trade_size(v: f64) -> String {
    if !(v.is_finite() && v > 0.0) {
        return String::new();
    }
    let e = (v * 8.0).round() as i64;
    let (w, r) = (e / 8, e % 8);
    let frac = match r {
        0 => String::new(),
        x if x % 4 == 0 => format!("{}/2", x / 4),
        x if x % 2 == 0 => format!("{}/4", x / 2),
        x => format!("{x}/8"),
    };
    let t = match (w, frac.is_empty()) {
        (0, false) => frac,
        (w, true) => w.to_string(),
        (w, false) => format!("{w}-{frac}"),
    };
    format!("{t}\"")
}

/// A path without repeated points or straight-through corners (sheet units).
fn clean_path(path: &[Vec3], upf: f64) -> Vec<Vec3> {
    let tiny = upf / 192.0;
    let mut out: Vec<Vec3> = Vec::new();
    for p in path.iter().take(MAX_PIECES).copied().filter(|p| p.is_finite()) {
        if out.last().is_none_or(|q| dist(*q, p) > tiny) {
            out.push(p);
        }
    }
    let mut i = 1;
    while i + 1 < out.len() {
        let (Some(a), Some(b), Some(c)) = (out.get(i - 1).copied(), out.get(i).copied(), out.get(i + 1).copied()) else { break };
        if bend_deg(b - a, c - b) < 1.0 {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    out
}

/// The x axis.
pub(crate) const X3: Vec3 = Vec3::new(1.0, 0.0, 0.0);

/// The distance between two points.
pub(crate) fn dist(a: Vec3, b: Vec3) -> f64 {
    (a - b).len()
}

/// The angle (degrees) a path turns between two legs.
fn bend_deg(u: Vec3, v: Vec3) -> f64 {
    let (lu, lv) = (u.len(), v.len());
    if lu <= 1e-12 || lv <= 1e-12 {
        return 0.0;
    }
    (u.dot(v) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// An elbow's angle as stocked: 90, 45, 30, 22-1/2 or 11-1/4 degrees, the nearest.
fn stock_angle(deg: f64) -> f64 {
    [90.0, 45.0, 30.0, 22.5, 11.25].into_iter().min_by(|a, b| (a - deg).abs().total_cmp(&(b - deg).abs())).unwrap_or(90.0)
}

fn angle_text(a: f64) -> String {
    if (a - a.round()).abs() < 1e-9 { format!("{a:.0}") } else { format!("{a}") }
}

/// The length of a path, feet.
pub(crate) fn path_ft(path: &[Vec3], upf: f64) -> f64 {
    path.windows(2).map(|w| if let [a, b] = w { dist(*a, *b) } else { 0.0 }).sum::<f64>() / upf
}

/// The point at `s` feet along a path, and the leg it is on.
fn along(path: &[Vec3], upf: f64, s: f64) -> (Vec3, usize) {
    let mut left = s * upf;
    for (i, w) in path.windows(2).enumerate() {
        if let [a, b] = w {
            let l = dist(*a, *b);
            if left <= l || i + 2 == path.len() {
                let t = if l > 1e-12 { (left / l).clamp(0.0, 1.0) } else { 0.0 };
                return (*a + (*b - *a) * t, i);
            }
            left -= l;
        }
    }
    (path.first().copied().unwrap_or(Vec3::ZERO), 0)
}

/// The part of a path from `s0` to `s1` feet along it: its two points and the corners between.
fn sub_path(path: &[Vec3], upf: f64, s0: f64, s1: f64) -> Vec<Vec3> {
    let (a, i) = along(path, upf, s0);
    let (b, j) = along(path, upf, s1);
    let mut out = vec![a];
    for k in i + 1..=j {
        if let Some(p) = path.get(k).copied().filter(|p| dist(*p, a) > 1e-9 && dist(*p, b) > 1e-9) {
            out.push(p);
        }
    }
    out.push(b);
    out
}

/// Where supports go along a run `total` feet long: within `from_end` of each end and at most
/// `spacing` apart (feet along it).
pub(crate) fn support_stations(total: f64, from_end: f64, spacing: f64) -> Vec<f64> {
    if !(total.is_finite() && total > 1e-6 && spacing.is_finite() && spacing > 1e-6) {
        return Vec::new();
    }
    let from_end = from_end.max(0.0);
    let first = from_end.min(total / 2.0);
    let last = (total - from_end).max(first);
    let mut out = vec![first];
    if last - first > 1e-6 {
        let n = ((last - first) / spacing - 1e-9).ceil().max(1.0) as usize;
        for k in 1..=n.min(MAX_PIECES) {
            out.push(first + (last - first) * k as f64 / n as f64);
        }
    }
    out
}

/// A raceway run as its pieces: along each straight leg its sticks (full stock lengths, then the
/// cut piece), an elbow at each turn (an offset where a short leg steps between two parallel
/// ones), a coupling at every joint, a connector (with its locknut, and a bushing for large
/// conductors) at each end, and the straps the code asks for. `path`: sheet units.
pub(crate) fn raceway_pieces(
    path: &[Vec3],
    upf: f64,
    size_in: f64,
    material: &str,
    rw: &Raceway,
    large_conductors: bool,
    rules: &AtomRules,
) -> Pieces {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let pts = clean_path(path, upf);
    let mut out = Pieces::default();
    if pts.len() < 2 {
        return out;
    }
    let size = trade_size(size_in);
    let stock = if rw.stock_ft.is_finite() && rw.stock_ft > 0.0 { rw.stock_ft } else { 10.0 };
    let tiny_ft = 1.0 / 192.0;
    let legs: Vec<(Vec3, Vec3)> = pts.windows(2).filter_map(|w| if let [a, b] = w { Some((*a, *b)) } else { None }).collect();
    // A short leg stepping between two parallel legs is one offset, not two elbows and a stick.
    let offset_max = rules.offset_max_ft.max(0.0) * upf;
    let mut offset_leg = vec![false; legs.len()];
    for i in 1..legs.len().saturating_sub(1) {
        let (Some(p), Some(l), Some(n)) = (legs.get(i - 1), legs.get(i), legs.get(i + 1)) else { continue };
        let parallel = bend_deg(p.1 - p.0, n.1 - n.0) < 1.0;
        if parallel
            && dist(l.0, l.1) <= offset_max
            && bend_deg(p.1 - p.0, l.1 - l.0) >= 1.0
            && let Some(o) = offset_leg.get_mut(i)
        {
            *o = true;
        }
    }
    // A trade size bent by hand turns without a fitting: its sticks run on through the bend.
    let field = rw.field_bend_max_in > 0.0 && size_in <= rw.field_bend_max_in + 1e-9;
    // The pieces in order along the run (sticks, elbows, factory offsets), with the stretch of
    // the run (feet) each stick covers; a field bend or offset is marked on the stick it is in.
    let mut chain: Vec<usize> = Vec::new();
    let mut spans: Vec<(f64, f64, usize)> = Vec::new();
    let mut marks: Vec<(f64, Atom)> = Vec::new();
    let mut s0 = 0.0;
    // The stretch of straight and field-bent legs since the last fitting: its points and where
    // it starts along the run.
    let mut group: Vec<Vec3> = pts.first().copied().into_iter().collect();
    let mut group_s0 = 0.0;
    let flush = |group: &mut Vec<Vec3>, group_s0: f64, out: &mut Pieces, chain: &mut Vec<usize>, spans: &mut Vec<(f64, f64, usize)>| {
        let total = path_ft(group, upf);
        let mut done = 0.0;
        while total - done > tiny_ft && out.atoms.len() < MAX_PIECES {
            let piece = (total - done).min(stock);
            let cut = piece < stock - tiny_ft;
            let sub = sub_path(group, upf, done, done + piece);
            let (Some(pa), Some(pb)) = (sub.first().copied(), sub.last().copied()) else { break };
            let st = Atom {
                kind: "conduit",
                item: rw.stick.clone(),
                ty: material.into(),
                size: size.clone(),
                body_in: size_in,
                length_ft: piece,
                a: pa,
                b: pb,
                path: if sub.len() > 2 { sub } else { Vec::new() },
                cut,
                source: rw.stock_source.clone(),
                ..Atom::default()
            };
            spans.push((group_s0 + done, group_s0 + done + piece, out.atoms.len()));
            chain.push(out.atoms.len());
            out.atoms.push(st);
            done += piece;
        }
        let last = group.last().copied();
        group.clear();
        group.extend(last);
    };
    for (i, (a, b)) in legs.iter().enumerate() {
        let len_ft = dist(*a, *b) / upf;
        let prev = i.checked_sub(1).and_then(|j| legs.get(j)).map(|p| p.1 - p.0);
        if offset_leg.get(i).copied().unwrap_or(false) {
            let mid = (*a + *b) * 0.5;
            let mut at = Atom::fitting("offset", rw.offset.clone(), material, &size, mid, *b - *a, size_in, &rw.stock_source);
            at.angle_deg = bend_deg(prev.unwrap_or(Vec3::Z), *b - *a);
            at.extra.push(("offset_in", format!("{:.2}", dist(*a, *b) / upf * 12.0), "computed".into()));
            out.bend_deg += 2.0 * at.angle_deg;
            if field {
                at.item = rw.field_offset.clone();
                at.source = rw.field_bend_source.clone();
                marks.push((s0 + len_ft / 2.0, at));
                group.push(*b);
            } else {
                flush(&mut group, group_s0, &mut out, &mut chain, &mut spans);
                at.length_ft = len_ft;
                chain.push(out.atoms.len());
                out.atoms.push(at);
                group = vec![*b];
                group_s0 = s0 + len_ft;
            }
            s0 += len_ft;
            continue;
        }
        if let Some(prev) = prev.filter(|_| i > 0 && !offset_leg.get(i - 1).copied().unwrap_or(false)) {
            let deg = bend_deg(prev, *b - *a);
            let ang = stock_angle(deg);
            out.bend_deg += deg;
            if field {
                let mut at = Atom::fitting(
                    "bend",
                    rw.field_bend.replace("{angle}", &angle_text(ang)),
                    material,
                    &size,
                    *a,
                    *b - *a,
                    size_in,
                    &rw.field_bend_source,
                );
                at.angle_deg = ang;
                at.extra.push(("turn_deg", format!("{deg:.1}"), "computed".into()));
                marks.push((s0, at));
            } else {
                flush(&mut group, group_s0, &mut out, &mut chain, &mut spans);
                let mut at =
                    Atom::fitting("elbow", rw.elbow.replace("{angle}", &angle_text(ang)), material, &size, *a, *b - *a, size_in, &rw.stock_source);
                at.angle_deg = ang;
                at.extra.push(("turn_deg", format!("{deg:.1}"), "computed".into()));
                chain.push(out.atoms.len());
                out.atoms.push(at);
                group = vec![*a];
                group_s0 = s0;
            }
        }
        group.push(*b);
        s0 += len_ft;
    }
    flush(&mut group, group_s0, &mut out, &mut chain, &mut spans);
    // The field bends and offsets, each on the stick it is bent into.
    for (st, mut at) in marks {
        if out.atoms.len() >= MAX_PIECES {
            break;
        }
        let held = spans.iter().find(|(a, b, _)| st >= *a - 1e-9 && st <= *b + 1e-9).map(|(_, _, i)| *i);
        let mi = out.atoms.len();
        if let Some(h) = held {
            at.connects = vec![h];
        }
        out.atoms.push(at);
        if let Some(a) = held.and_then(|h| out.atoms.get_mut(h)) {
            a.connects.push(mi);
        }
    }
    out.length_ft = s0;
    // A coupling at every joint between two pieces.
    let joints: Vec<(usize, usize)> = chain.windows(2).filter_map(|w| if let [x, y] = w { Some((*x, *y)) } else { None }).collect();
    for (x, y) in joints {
        let (Some(ax), Some(ay)) = (out.atoms.get(x), out.atoms.get(y)) else { continue };
        let at = if ax.kind == "conduit" { ax.b } else { ay.a };
        let dir = ay.b - ay.a;
        let mut c = Atom::fitting("coupling", rw.coupling.clone(), material, &size, at, dir, size_in * 1.25, &rw.stock_source);
        c.connects = vec![x, y];
        let ci = out.atoms.len();
        out.atoms.push(c);
        if let Some(a) = out.atoms.get_mut(x) {
            a.connects.push(ci);
        }
        if let Some(a) = out.atoms.get_mut(y) {
            a.connects.push(ci);
        }
    }
    // A connector at each end, its locknut, and a bushing over large conductors.
    for (end, piece) in [(0usize, chain.first().copied()), (1usize, chain.last().copied())] {
        let Some(p) = piece else { continue };
        let (at, dir) =
            if end == 0 { (pts.first().copied(), legs.first().map(|l| l.0 - l.1)) } else { (pts.last().copied(), legs.last().map(|l| l.1 - l.0)) };
        let (Some(at), Some(dir)) = (at, dir) else { continue };
        let mut c = Atom::fitting("connector", rw.connector.clone(), material, &size, at, dir, size_in * 1.3, &rw.stock_source);
        c.connects = vec![p];
        let ci = out.atoms.len();
        out.atoms.push(c);
        if let Some(a) = out.atoms.get_mut(p) {
            a.connects.push(ci);
        }
        if let Some(x) = out.ends.get_mut(end) {
            *x = Some(ci);
        }
        let mut nut = Atom::fitting("locknut", rules.locknut_item.clone(), material, &size, at, dir, size_in * 1.6, &rules.locknut_source);
        nut.connects = vec![ci];
        let ni = out.atoms.len();
        out.atoms.push(nut);
        if let Some(a) = out.atoms.get_mut(ci) {
            a.connects.push(ni);
        }
        if large_conductors {
            let mut bu = Atom::fitting("bushing", rules.bushing_item.clone(), material, &size, at, dir, size_in * 1.2, &rules.bushing_source);
            bu.connects = vec![ci];
            let bi = out.atoms.len();
            out.atoms.push(bu);
            if let Some(a) = out.atoms.get_mut(ci) {
                a.connects.push(bi);
            }
        }
    }
    // Straps.
    for s in support_stations(out.length_ft, rw.strap_from_end_ft, rw.strap_spacing(size_in)) {
        if out.atoms.len() >= MAX_PIECES {
            break;
        }
        let (at, leg) = along(&pts, upf, s);
        let dir = legs.get(leg).map_or(X3, |l| l.1 - l.0);
        let held = spans.iter().find(|(a, b, _)| s >= *a - 1e-9 && s <= *b + 1e-9).map(|(_, _, i)| *i);
        let mut st = Atom::fitting("strap", rw.strap.clone(), material, &size, at, dir, size_in * 1.4, &rw.strap_source);
        st.extra.push(("station_ft", format!("{s:.2}"), "computed".into()));
        let si = out.atoms.len();
        if let Some(h) = held {
            st.connects = vec![h];
        }
        out.atoms.push(st);
        if let Some(a) = held.and_then(|h| out.atoms.get_mut(h)) {
            a.connects.push(si);
        }
    }
    out
}

/// A branch circuit's conductors (`2#12 + 1#12G`, `3#10, 1#10G`): count, size, ground.
pub(crate) fn conductor_parts(makeup: &str) -> Vec<(u32, String, bool)> {
    makeup
        .split(" in ")
        .next()
        .unwrap_or_default()
        .split(['+', ','])
        .filter_map(|p| {
            let (n, rest) = p.trim().split_once('#')?;
            let n = n.trim().parse::<u32>().ok().unwrap_or(1).clamp(1, 12);
            let size: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
            let after = rest.get(size.len()..).unwrap_or_default().trim_start().to_ascii_uppercase();
            let ground = after.starts_with('G') || after.starts_with("EG") || after.starts_with("GND");
            (!size.is_empty()).then_some((n, size, ground))
        })
        .collect()
}

/// A conductor size as printed: `#12`, `3/0`, `500 kcmil`.
pub(crate) fn wire_size(size: &str) -> String {
    match size.parse::<u32>() {
        Ok(n) if n >= 250 => format!("{n} kcmil"),
        _ if size.contains('/') => format!("#{size}"),
        _ => format!("#{size}"),
    }
}

/// Whether a conductor size is #`min` or larger (`4`, `2`, `1/0`, `250`).
fn large_awg(size: &str, min: u32) -> bool {
    size.contains('/') || size.parse::<u32>().is_ok_and(|n| n <= min || n >= 250)
}

/// Whether the conductors include a large one (an insulating bushing at each end).
pub(crate) fn large_conductors(makeup: &str, min_awg: u32) -> bool {
    conductor_parts(makeup).iter().any(|(_, s, g)| !g && large_awg(s, min_awg))
}

/// The phase of a panel circuit by its number (standard bussing: 1-2 A, 3-4 B, 5-6 C).
pub(crate) fn phase_of(circuit: u32) -> &'static str {
    match (circuit.saturating_sub(1) / 2) % 3 {
        0 => "A",
        1 => "B",
        _ => "C",
    }
}

/// A cable's type in words: `NM 12/2 w/G` from its kind and conductors.
pub(crate) fn cable_type(kind: &str, makeup: &str) -> (String, String, u32) {
    let parts = conductor_parts(makeup);
    let insulated: u32 = parts.iter().filter(|p| !p.2).map(|p| p.0).sum();
    let ground = parts.iter().any(|p| p.2);
    let size = parts.iter().find(|p| !p.2).map(|p| p.1.clone()).unwrap_or_else(|| "12".into());
    let n = insulated.max(1);
    (format!("{kind} {size}/{n}{}", if ground { " w/G" } else { "" }), format!("#{size}"), n + u32::from(ground))
}

/// A cable run as its pieces: the cable (its length plus makeup at each end) and its supports.
pub(crate) fn cable_pieces(path: &[Vec3], upf: f64, kind: &str, cr: &CableRule, makeup: &str, rules: &AtomRules) -> Pieces {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let pts = clean_path(path, upf);
    let mut out = Pieces::default();
    let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied()) else { return out };
    if pts.len() < 2 {
        return out;
    }
    let run_ft = path_ft(&pts, upf);
    let (ty, size, count) = cable_type(kind, makeup);
    let makeup_ft = rules.makeup_ft.max(0.0);
    let mut cable = Atom {
        kind: "cable",
        item: cr.item.clone(),
        ty: ty.clone(),
        size: size.clone(),
        body_in: 0.5,
        length_ft: run_ft + 2.0 * makeup_ft,
        a,
        b,
        path: pts.clone(),
        source: cr.strap_source.clone(),
        ..Atom::default()
    };
    cable.extra.push(("conductors", count.to_string(), "computed".into()));
    cable.extra.push(("run_ft", format!("{run_ft:.2}"), "computed".into()));
    cable.extra.push(("makeup_ft", format!("{makeup_ft}"), rules.makeup_source.clone()));
    out.length_ft = run_ft;
    out.atoms.push(cable);
    let legs: Vec<Vec3> = pts.windows(2).filter_map(|w| if let [p, q] = w { Some(*q - *p) } else { None }).collect();
    for s in support_stations(run_ft, cr.strap_from_box_ft, cr.strap_max_ft) {
        let (at, leg) = along(&pts, upf, s);
        let mut st = Atom::fitting("strap", cr.strap.clone(), &ty, &size, at, legs.get(leg).copied().unwrap_or(X3), 0.75, &cr.strap_source);
        st.connects = vec![0];
        st.extra.push(("station_ft", format!("{s:.2}"), "computed".into()));
        let i = out.atoms.len();
        out.atoms.push(st);
        if let Some(c) = out.atoms.first_mut() {
            c.connects.push(i);
        }
    }
    if !cr.connector.is_empty() {
        for (end, at, dir) in [(0usize, a, legs.first().map_or(X3, |v| *v * -1.0)), (1usize, b, legs.last().copied().unwrap_or(X3))] {
            let mut c = Atom::fitting("connector", cr.connector.clone(), &ty, &size, at, dir, 0.75, &cr.strap_source);
            c.connects = vec![0];
            let i = out.atoms.len();
            out.atoms.push(c);
            if let Some(c) = out.atoms.first_mut() {
                c.connects.push(i);
            }
            if let Some(x) = out.ends.get_mut(end) {
                *x = Some(i);
            }
        }
    } else {
        out.ends = [Some(0), Some(0)];
    }
    out
}

/// The conductors pulled in a raceway: one wire each (size, insulation, colour, length: the
/// run's plus makeup at each end). `circuits`: the panel circuit numbers (`1,3,5`), for the
/// phases; `voltage`: `120/208`.
pub(crate) fn wires(path: &[Vec3], upf: f64, makeup: &str, circuits: &[u32], voltage: &str, rules: &AtomRules) -> Vec<Atom> {
    let m = rules.makeup_ft.max(0.0);
    wires_with(path, upf, makeup, circuits, voltage, rules, &WireOpts { kind: None, makeup_ft: [m, m], makeup_source: &rules.makeup_source })
}

/// How a run's conductors differ from a branch circuit's: what they are bought as (`None`: THHN
/// copper, the rules' insulation) and the makeup at each end, feet.
pub(crate) struct WireOpts<'a> {
    pub kind: Option<(&'a ConductorKind, &'a str)>,
    pub makeup_ft: [f64; 2],
    pub makeup_source: &'a str,
}

/// [`wires`], the conductors' kind and each end's makeup as `opts` say.
pub(crate) fn wires_with(
    path: &[Vec3],
    upf: f64,
    makeup: &str,
    circuits: &[u32],
    voltage: &str,
    rules: &AtomRules,
    opts: &WireOpts<'_>,
) -> Vec<Atom> {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let pts = clean_path(path, upf);
    let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied()) else { return Vec::new() };
    if pts.len() < 2 {
        return Vec::new();
    }
    let ends = opts.makeup_ft.map(|m| if m.is_finite() { m.max(0.0) } else { 0.0 });
    let len = path_ft(&pts, upf) + ends[0] + ends[1];
    let makeup_text = if (ends[0] - ends[1]).abs() < 1e-9 { format!("{}", ends[0]) } else { format!("{} + {}", ends[0], ends[1]) };
    let mut phases: Vec<&str> = circuits.iter().map(|c| phase_of(*c)).collect();
    if phases.is_empty() {
        phases.push("A");
    }
    let mut out = Vec::new();
    let mut hot = 0usize;
    for (n, size, ground) in conductor_parts(makeup) {
        for _ in 0..n.min(12) {
            let role = if ground {
                "G"
            } else if hot < phases.len() {
                hot += 1;
                phases.get(hot - 1).copied().unwrap_or("A")
            } else {
                "N"
            };
            let color = rules.color(voltage, role).unwrap_or_default();
            let (item, ty, source) = match opts.kind {
                Some((k, src)) => (if ground { k.ground_item.clone() } else { k.item.clone() }, k.insulation.clone(), src.to_string()),
                None => (
                    if ground { "Copper ground, green".into() } else { "THHN copper".into() },
                    rules.insulation.clone(),
                    rules.insulation_source.clone(),
                ),
            };
            let mut w = Atom {
                kind: "wire",
                item,
                ty,
                size: wire_size(&size),
                body_in: 0.15,
                length_ft: len,
                a,
                b,
                path: pts.clone(),
                source,
                ..Atom::default()
            };
            w.extra.push(("color", color.to_string(), rules.colors_source.clone()));
            w.extra.push((
                "conductor",
                if ground {
                    "ground".into()
                } else if role == "N" {
                    "neutral".into()
                } else {
                    format!("phase {role}")
                },
                rules.phase_source.clone(),
            ));
            w.extra.push(("makeup_ft", makeup_text.clone(), opts.makeup_source.to_string()));
            out.push(w);
        }
    }
    out
}

// ─── The count index ─────────────────────────────────────────────────────────

/// What a piece is counted by.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Key {
    pub kind: String,
    /// What it is bought as (`EMT 90° elbow`): the BOM's line.
    #[serde(default)]
    pub item: String,
    pub size: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub level: String,
    pub room: String,
    pub circuit: String,
    pub system: String,
    /// The trade a building piece belongs to (`concrete`, `framing`, `drywall`…, Q6162); empty
    /// for the electrical pieces and devices.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trade: String,
}

/// What one counted element measures: its length, area, volume, weight and pieces (a bag of
/// screws or a run of chairs is one element of many pieces), and whether it is estimated.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Measure {
    pub length_ft: f64,
    pub area_sf: f64,
    pub volume_cf: f64,
    pub weight_lb: f64,
    /// Pieces it stands for (1 when it is one piece).
    pub qty: f64,
    pub estimated: bool,
}

impl Measure {
    /// One piece of `length_ft` (0 for a fitting).
    pub(crate) fn length(length_ft: f64, estimated: bool) -> Measure {
        Measure { length_ft, qty: 1.0, estimated, ..Measure::default() }
    }
}

/// How many pieces of a key, their length, area, volume, weight and pieces, and how many are
/// estimated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tally {
    pub count: u64,
    pub length_ft: f64,
    pub estimated: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub area_sf: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub volume_cf: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub weight_lb: f64,
    /// Pieces (an element counts its `qty`); 0 in an index stored before trades: then `count`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub qty: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Tally {
    fn add(&mut self, m: &Measure, sign: i64) {
        if sign >= 0 {
            self.count += 1;
            self.length_ft += m.length_ft;
            self.area_sf += m.area_sf;
            self.volume_cf += m.volume_cf;
            self.weight_lb += m.weight_lb;
            self.qty += m.qty;
            self.estimated += u64::from(m.estimated);
        } else {
            self.count = self.count.saturating_sub(1);
            self.length_ft = (self.length_ft - m.length_ft).max(0.0);
            self.area_sf = (self.area_sf - m.area_sf).max(0.0);
            self.volume_cf = (self.volume_cf - m.volume_cf).max(0.0);
            self.weight_lb = (self.weight_lb - m.weight_lb).max(0.0);
            self.qty = (self.qty - m.qty).max(0.0);
            self.estimated = self.estimated.saturating_sub(u64::from(m.estimated));
        }
    }
    /// Another tally's totals added in.
    pub(crate) fn merge(&mut self, t: &Tally) {
        self.count += t.count;
        self.length_ft += t.length_ft;
        self.area_sf += t.area_sf;
        self.volume_cf += t.volume_cf;
        self.weight_lb += t.weight_lb;
        self.qty += t.qty;
        self.estimated += t.estimated;
    }
    /// The pieces it stands for: its `qty`, else (an older index) its count.
    pub(crate) fn pieces(&self) -> f64 {
        if self.qty > 0.0 { self.qty } else { self.count as f64 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Row {
    #[serde(flatten)]
    key: Key,
    #[serde(flatten)]
    tally: Tally,
}

/// The model's count index: every piece (atom) and device by its key.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Index {
    pub rows: BTreeMap<Key, Tally>,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    rows: Vec<Row>,
}

impl Index {
    pub(crate) fn add(&mut self, key: Key, m: &Measure) {
        if self.rows.len() < MAX_ROWS || self.rows.contains_key(&key) {
            self.rows.entry(key).or_default().add(m, 1);
        }
    }
    pub(crate) fn remove(&mut self, key: &Key, m: &Measure) {
        if let Some(t) = self.rows.get_mut(key) {
            t.add(m, -1);
            if t.count == 0 {
                self.rows.remove(key);
            }
        }
    }
    /// The index of every counted element of a drawing (a full recount). The built elements
    /// come after the imported sheets (hundreds of thousands of objects): the cap applies to the
    /// built ones.
    pub(crate) fn of(d: &Drawing) -> Index {
        let mut ix = Index::default();
        for (k, m) in d.model.iter().filter_map(|e| counted(e)).take(MAX_ELEMENTS) {
            ix.add(k, &m);
        }
        ix
    }
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let f = IndexFile { version: 2, rows: self.rows.iter().map(|(k, t)| Row { key: k.clone(), tally: *t }).collect() };
        serde_json::to_vec(&f).unwrap_or_default()
    }
    pub(crate) fn from_bytes(b: &[u8]) -> Option<Index> {
        let f: IndexFile = serde_json::from_slice(b).ok()?;
        Some(Index { rows: f.rows.into_iter().take(MAX_ROWS).map(|r| (r.key, r.tally)).collect() })
    }
    /// The pieces matching a filter: their tally, and by the group's dimension.
    pub(crate) fn query(&self, f: &Filter, group_by: Option<&str>) -> (Tally, BTreeMap<String, Tally>) {
        let mut total = Tally::default();
        let mut groups: BTreeMap<String, Tally> = BTreeMap::new();
        for (k, t) in self.rows.iter().filter(|(k, _)| f.matches(k)) {
            total.merge(t);
            if let Some(g) = group_by {
                let v = match g {
                    "kind" => &k.kind,
                    "item" => &k.item,
                    "size" => &k.size,
                    "type" => &k.ty,
                    "level" => &k.level,
                    "room" => &k.room,
                    "circuit" => &k.circuit,
                    "trade" => &k.trade,
                    _ => &k.system,
                };
                groups.entry(v.clone()).or_default().merge(t);
            }
        }
        (total, groups)
    }
}

/// The kinds of piece that make up a run (not devices).
pub(crate) const PIECES: &[&str] =
    &["conduit", "coupling", "connector", "elbow", "offset", "strap", "locknut", "bushing", "box", "ring", "cable", "wire"];

/// Whether a piece is the trade's work, not something bought (a field bend or offset).
pub(crate) fn is_labor_item(item: &str) -> bool {
    rules().raceways.values().any(|r| {
        let bend = r.field_bend.split("{angle}").next().unwrap_or_default();
        (!bend.is_empty() && item.starts_with(bend)) || (!r.field_offset.is_empty() && item == r.field_offset)
    })
}

/// The drawing's stored index, if it has one.
pub(crate) fn stored(d: &Drawing) -> Option<Index> {
    d.embedded.get(INDEX_FILE).and_then(|b| Index::from_bytes(b))
}

/// FNV-1a of bytes (which stored index the cache holds).
fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ u64::from(*x)).wrapping_mul(0x0100_0000_01b3))
}

thread_local! {
    /// The last stored index read, by its bytes' hash and length: a count is a lookup, not a
    /// parse of the index each time.
    static CACHE: std::cell::RefCell<Option<(u64, usize, std::rc::Rc<Index>)>> = const { std::cell::RefCell::new(None) };
}

/// The drawing's stored index, read once per version of it.
fn stored_cached(d: &Drawing) -> Option<std::rc::Rc<Index>> {
    let b = d.embedded.get(INDEX_FILE)?;
    let key = (fnv(b), b.len());
    if let Some(ix) = CACHE.with(|c| c.borrow().as_ref().filter(|(h, n, _)| (*h, *n) == key).map(|(_, _, ix)| std::rc::Rc::clone(ix))) {
        return Some(ix);
    }
    let ix = std::rc::Rc::new(Index::from_bytes(b)?);
    CACHE.with(|c| *c.borrow_mut() = Some((key.0, key.1, std::rc::Rc::clone(&ix))));
    Some(ix)
}

/// Store an index in the drawing.
pub(crate) fn store(d: &mut Drawing, ix: &Index) {
    d.embedded.insert(INDEX_FILE.into(), std::sync::Arc::new(ix.to_bytes()));
}

/// Rebuild and store the drawing's index from its elements; its row count.
pub(crate) fn reindex(d: &mut Drawing) -> usize {
    let ix = Index::of(d);
    let n = ix.rows.len();
    if n == 0 && !d.embedded.contains_key(INDEX_FILE) {
        return 0;
    }
    store(d, &ix);
    n
}

/// After a command: a command that changed counted elements without keeping the stored index
/// (a plain ERASE, COPY, EXPLODE or MOVE of model pieces, the live rebuild) updates it by what
/// changed, so `bim.count`, the BOM, the takeoff, the estimate and a saved .cdy agree with a
/// recount. A command that wrote the index itself (a build, `bim.atoms.edit`) is left alone.
/// The cost follows the change, not the model's size.
pub(crate) fn after_command(s: &mut Session, before: Option<&std::sync::Arc<Drawing>>, uid: Option<u64>) {
    let Some(before) = before else { return };
    let Ok(st) = s.state() else { return };
    if uid.is_some_and(|u| u != st.uid) || std::sync::Arc::ptr_eq(before, &st.doc) || st.doc.model.unchanged_from(&before.model) {
        return;
    }
    let (Some(old), Some(now)) = (before.embedded.get(INDEX_FILE), st.doc.embedded.get(INDEX_FILE)) else { return };
    if !std::sync::Arc::ptr_eq(old, now) {
        return;
    }
    let (added, modified, erased) = before.model.changes(&st.doc.model);
    let was = |h: &Handle| before.model.get(*h).and_then(|e| counted(e));
    let is = |h: &Handle| st.doc.model.get(*h).and_then(|e| counted(e));
    let mut out = Vec::new();
    let mut new = Vec::new();
    for h in &modified {
        let (a, b) = (was(h), is(h));
        if a != b {
            out.extend(a);
            new.extend(b);
        }
    }
    out.extend(erased.iter().filter_map(was));
    new.extend(added.iter().filter_map(is));
    if out.is_empty() && new.is_empty() {
        return;
    }
    let Some(mut ix) = stored(before) else { return };
    for (k, m) in &out {
        ix.remove(k, m);
    }
    for (k, m) in new {
        ix.add(k, &m);
    }
    if let Ok(d) = s.doc_mut() {
        store(d, &ix);
    }
}

/// A filter of `bim.count`: each given dimension must match.
#[derive(Clone, Debug, Default)]
pub(crate) struct Filter {
    pub kind: Option<String>,
    pub size: Option<String>,
    pub ty: Option<String>,
    pub level: Option<String>,
    pub room: Option<String>,
    pub circuit: Option<String>,
    pub system: Option<String>,
    pub trade: Option<String>,
    pub item: Option<String>,
}

/// Size text compared without quotes, spaces or `in`: `1"` = `1` = `1 in`.
fn norm_size(s: &str) -> String {
    let t = s.trim().to_ascii_lowercase().replace("in.", "").replace("inch", "").replace(" in", "");
    t.chars().filter(|c| !matches!(c, '"' | '\'' | ' ')).collect()
}

/// A kind as asked: `stick` and `conduit` are a conduit stick; `ell`, `bend` an elbow.
fn norm_kind(s: &str) -> String {
    let k = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    match k.as_str() {
        "stick" | "sticks" | "conduit_stick" | "raceway" => "conduit".into(),
        "ell" | "ells" | "elbows" => "elbow".into(),
        "bends" | "field_bend" | "field_bends" => "bend".into(),
        "couplings" => "coupling".into(),
        "connectors" => "connector".into(),
        "straps" | "support" | "supports" | "staple" => "strap".into(),
        "boxes" | "outlet_box" | "junction_box" => "box".into(),
        "mud_ring" | "plaster_ring" | "rings" => "ring".into(),
        "wires" | "conductor" | "conductors" => "wire".into(),
        "cables" | "romex" => "cable".into(),
        "breakers" | "circuit_breaker" | "circuit_breakers" | "cb" => "breaker".into(),
        "outlet" | "receptacles" => "receptacle".into(),
        "light" | "light_fixture" | "luminaires" => "luminaire".into(),
        _ => k,
    }
}

impl Filter {
    pub(crate) fn from_params(p: &Value) -> Filter {
        let g = |k: &str| {
            p.get(k).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_f64().map(|n| n.to_string()))).filter(|s| !s.trim().is_empty())
        };
        Filter {
            kind: g("kind"),
            size: g("size"),
            ty: g("type"),
            level: g("level"),
            room: g("room"),
            circuit: g("circuit"),
            system: g("system"),
            trade: g("trade"),
            item: g("item"),
        }
    }
    pub(crate) fn matches(&self, k: &Key) -> bool {
        let eq = |want: &Option<String>, have: &str| want.as_deref().is_none_or(|w| w.trim().eq_ignore_ascii_case(have.trim()));
        let kind_eq = |w: &str| {
            let (want, have) = (norm_kind(w), k.kind.replace(' ', "_").to_ascii_lowercase());
            // `studs`, `sheets`, `trusses`: the plural of a kind is the kind.
            want == have || want.strip_suffix("es").is_some_and(|x| x == have) || want.strip_suffix('s').is_some_and(|x| x == have)
        };
        self.kind.as_deref().is_none_or(kind_eq)
            && self.trade.as_deref().is_none_or(|w| super::tradeatoms::trade_eq(w, &k.trade))
            && self.item.as_deref().is_none_or(|w| k.item.to_ascii_lowercase().contains(&w.trim().to_ascii_lowercase()))
            && self.size.as_deref().is_none_or(|w| norm_size(w) == norm_size(&k.size))
            && self.ty.as_deref().is_none_or(|w| k.ty.to_ascii_lowercase().contains(&w.trim().to_ascii_lowercase()))
            && eq(&self.level, &k.level)
            && eq(&self.room, &k.room)
            && eq(&self.circuit, &k.circuit)
            && eq(&self.system, &k.system)
    }
}

/// What an element is counted as in the index: an atom by its kind, a device by its own kind;
/// with its length (feet) and whether it is on an estimated route. Nothing for other elements.
pub(crate) fn counted(e: &Entity) -> Option<(Key, Measure)> {
    let part = super::wallgen::made_as(e)?;
    let level = super::bspec::made_level(e).unwrap_or_default();
    let ps = props::read(&e.common.xdata);
    let text = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
    let num = |k: &str| ps.get(k).map(Prop::text).and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0);
    if let Some(kind) = ps.get(ATOM_KEY).map(Prop::text).filter(|k| !k.is_empty()) {
        let m = Measure {
            length_ft: num("length_ft").unwrap_or(0.0),
            area_sf: num("area_sf").unwrap_or(0.0),
            volume_cf: num("volume_cf").unwrap_or(0.0),
            weight_lb: num("weight_lb").unwrap_or(0.0),
            qty: num("qty").unwrap_or(1.0),
            estimated: text("estimated") == "true",
        };
        return Some((
            Key {
                kind,
                item: text("item"),
                size: text("size"),
                ty: text("type"),
                level,
                room: text("room"),
                circuit: text("circuit"),
                system: text("system"),
                trade: text("trade"),
            },
            m,
        ));
    }
    if part.eq_ignore_ascii_case("device") {
        let kind = ps.get("kind").map(Prop::text).filter(|k| !k.trim().is_empty()).unwrap_or_else(|| "device".into());
        return Some((
            Key {
                kind: kind.replace(' ', "_").to_ascii_lowercase(),
                item: String::new(),
                size: String::new(),
                ty: text("type_mark"),
                level,
                room: text("room"),
                circuit: text("circuit"),
                system: String::new(),
                trade: String::new(),
            },
            Measure::length(0.0, false),
        ));
    }
    // Casework (Q6162): counted as itself, the specialties'.
    if part.eq_ignore_ascii_case("casework") {
        return Some((
            Key {
                kind: "casework".into(),
                item: text("item"),
                size: text("size"),
                ty: text("type_mark"),
                level,
                room: text("room"),
                circuit: String::new(),
                system: String::new(),
                trade: "specialties".into(),
            },
            Measure::length(0.0, false),
        ));
    }
    // Doors and windows (Q6162): counted as themselves, with their opening's size.
    if part.eq_ignore_ascii_case("door") || part.eq_ignore_ascii_case("window") {
        return Some((
            Key {
                kind: part.to_ascii_lowercase(),
                item: text("item"),
                size: text("size"),
                ty: text("type_mark"),
                level,
                room: text("room"),
                circuit: String::new(),
                system: String::new(),
                trade: "openings".into(),
            },
            Measure { area_sf: num("area_sf").unwrap_or(0.0), ..Measure::length(0.0, false) },
        ));
    }
    None
}

// ─── Commands ────────────────────────────────────────────────────────────────

pub(crate) fn tally_json(t: &Tally) -> Value {
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let mut j = json!({ "count": t.count, "length_ft": r2(t.length_ft), "estimated": t.estimated });
    // A building piece's measures (Q6162): only what the counted pieces have.
    if t.area_sf > 0.0 {
        j["area_sf"] = json!(r2(t.area_sf));
    }
    if t.volume_cf > 0.0 {
        j["volume_cf"] = json!(r2(t.volume_cf));
        j["volume_cy"] = json!(r2(t.volume_cf / 27.0));
    }
    if t.weight_lb > 0.0 {
        j["weight_lb"] = json!(r2(t.weight_lb));
    }
    if (t.pieces() - t.count as f64).abs() > 1e-9 {
        j["pieces"] = json!(r2(t.pieces()));
    }
    j
}

fn run_count(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.count";
    let started = web_time::Instant::now();
    let group_by = p.get("group_by").and_then(Value::as_str).map(str::to_ascii_lowercase);
    if let Some(g) = group_by.as_deref()
        && !matches!(g, "kind" | "item" | "size" | "type" | "level" | "room" | "circuit" | "system" | "trade")
    {
        return Err(bad(CMD, format!("`group_by` must be kind, item, size, type, level, room, circuit, system or trade, not `{g}`")));
    }
    let f = Filter::from_params(p);
    if bool_or(p, "refresh", false) {
        let d = s.doc_mut()?;
        let ix = Index::of(d);
        store(d, &ix);
        s.touch();
    }
    let d = s.doc()?;
    let (ix, from) = match stored_cached(d) {
        Some(ix) => (ix, "index"),
        None => (std::rc::Rc::new(Index::of(d)), "recount"),
    };
    let (total, groups) = ix.query(&f, group_by.as_deref());
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut out = tally_json(&total);
    out["from"] = json!(from);
    out["index_rows"] = json!(ix.rows.len());
    out["ms"] = json!((ms * 1000.0).round() / 1000.0);
    if let Some(g) = group_by.as_deref() {
        out["rows"] = Value::Array(
            groups
                .iter()
                .take(10_000)
                .map(|(v, t)| {
                    let mut r = tally_json(t);
                    r[g] = json!(v);
                    r
                })
                .collect(),
        );
    }
    if bool_or(p, "verify", false) {
        let t0 = web_time::Instant::now();
        let full = Index::of(s.doc()?);
        let (t, _) = full.query(&f, None);
        let recount_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let close = |a: f64, b: f64| (a - b).abs() <= 0.01 + 1e-9 * a.abs().max(b.abs());
        let mut v = tally_json(&t);
        v["matches"] = json!(
            t.count == total.count
                && close(t.length_ft, total.length_ft)
                && close(t.area_sf, total.area_sf)
                && close(t.volume_cf, total.volume_cf)
                && close(t.weight_lb, total.weight_lb)
                && close(t.qty, total.qty)
                && full.rows.len() == ix.rows.len()
                && full.rows.iter().zip(ix.rows.iter()).all(|((ka, ta), (kb, tb))| ka == kb && ta.count == tb.count)
        );
        v["recount_ms"] = json!((recount_ms * 10.0).round() / 10.0);
        out["verify"] = v;
    }
    if from == "recount" && total.count == 0 && ix.rows.is_empty() {
        out["note"] = json!("the model has no pieces to count: build it (model.build) first");
    }
    Ok(out)
}

/// The atom property values an edit can change, before and after.
fn atom_view(ps: &cadaddy_doc::props::Props) -> Value {
    let t = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
    json!({ "size": t("size"), "type": t("type"), "length_ft": ps.get("length_ft").map(Prop::text).and_then(|v| v.parse::<f64>().ok()) })
}

fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.atoms.edit";
    let h = p.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex).ok_or_else(|| bad(CMD, "`handle` (hex) is required"))?;
    let length = match p.get("length_ft") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 10_000.0)
                .ok_or_else(|| bad(CMD, "`length_ft` must be a length in feet over 0 and up to 10000"))?,
        ),
    };
    let text = |k: &str| -> Result<Option<String>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(t)) if !t.trim().is_empty() && t.len() <= 200 => Ok(Some(t.trim().to_string())),
            Some(_) => Err(bad(CMD, format!("`{k}` must be text (up to 200 characters)"))),
        }
    };
    let (size, ty) = (text("size")?, text("type")?);
    if length.is_none() && size.is_none() && ty.is_none() {
        return Err(bad(CMD, "give what to change: `length_ft`, `size` or `type`"));
    }
    let upf = super::bspec::current(s, CMD).map(|(sp, _)| sp.units_per_foot).ok().filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0);
    let d = s.doc()?;
    let e = d.model.get(h).cloned().ok_or_else(|| bad(CMD, format!("there is no object {} in the model", h.hex())))?;
    let (old_key, old_m) = counted(&e)
        .filter(|(k, _)| !k.kind.is_empty())
        .ok_or_else(|| bad(CMD, format!("{} is not a piece of a run (build the model's runs first)", h.hex())))?;
    let ps = props::read(&e.common.xdata);
    if ps.get(ATOM_KEY).is_none() {
        return Err(bad(CMD, format!("{} is a device, not a piece of a run: edit it with bim.props.set", h.hex())));
    }
    let before = atom_view(&ps);
    let mut ps2 = ps.clone();
    let user = || Source::user();
    if let Some(l) = length {
        if ps.get("length_ft").is_none() {
            return Err(bad(CMD, format!("a {} has no length to change", old_key.kind)));
        }
        ps2.set("length_ft", Prop::new(format!("{:.4}", l).trim_end_matches('0').trim_end_matches('.').to_string(), user()).with_unit("ft"));
        if l > 0.0 {
            ps2.set("cut", Prop::new("true", user()));
        }
    }
    if let Some(z) = &size {
        ps2.set("size", Prop::new(z.clone(), user()));
    }
    if let Some(t) = &ty {
        ps2.set("type", Prop::new(t.clone(), user()));
    }
    // A stick's body follows its new length (from its start, along it).
    let new_mesh = match (&e.kind, length, old_key.kind.as_str()) {
        (cadaddy_doc::EntityKind::Mesh(_), Some(l), "conduit") => build::stick_ends(&ps).map(|(a, b)| {
            let dir = b - a;
            let len = dir.len();
            let b2 = if len > 1e-12 { a + dir * (l * upf / len) } else { b };
            let dia = build::body_of(&ps);
            let mut m = super::wallgen::empty_mesh();
            build::tube(&mut m, a, b2, dia / 24.0 * upf, 8);
            (m, b2)
        }),
        _ => None,
    };
    if let Some((_, b2)) = &new_mesh {
        ps2.set("end", Prop::new(format!("{:.4},{:.4},{:.4}", b2.x, b2.y, b2.z), Source::computed()));
    }
    let xdata = props::write(&e.common.xdata, &ps2);
    let d = s.doc_mut()?;
    d.modify_entity(h, |e| {
        e.common.xdata = xdata.into();
        if let Some((m, _)) = new_mesh {
            e.kind = cadaddy_doc::EntityKind::Mesh(m);
        }
    })
    .map_err(|err| bad(CMD, err.to_string()))?;
    // The index: out with the old key, in with the new.
    let e2 = d.model.get(h).cloned();
    let mut ix = stored(d).unwrap_or_else(|| Index::of(d));
    if stored(d).is_some() {
        ix.remove(&old_key, &old_m);
        if let Some((k, m)) = e2.as_deref().and_then(counted) {
            ix.add(k, &m);
        }
    }
    let rows = ix.rows.len();
    let total: u64 = ix.rows.values().map(|t| t.count).sum();
    store(d, &ix);
    s.touch();
    Ok(json!({
        "handle": h.hex(),
        "kind": old_key.kind,
        "before": before,
        "after": atom_view(&ps2),
        "index": { "rows": rows, "count": total },
    }))
}

/// Whether an element is a piece of a run (an atom).
pub(crate) fn is_atom(e: &Entity) -> bool {
    super::bimlook::made_tag(e, "part").is_some_and(|p| p.starts_with("atom:"))
}

/// The handles an element's `connects` property names.
pub(crate) fn connects_of(e: &Entity) -> Vec<Handle> {
    props::read(&e.common.xdata)
        .get("connects")
        .map(Prop::text)
        .map(|t| t.split([',', ' ']).filter_map(Handle::parse_hex).take(64).collect())
        .unwrap_or_default()
}

fn node_json(e: &Entity) -> Value {
    let ps = props::read(&e.common.xdata);
    let t = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
    let kind =
        ps.get(ATOM_KEY).map(Prop::text).or_else(|| ps.get("kind").map(Prop::text)).unwrap_or_else(|| super::wallgen::made_as(e).unwrap_or_default());
    let mut j = json!({ "handle": e.handle.hex(), "kind": kind, "item": t("item"), "size": t("size"), "circuit": t("circuit") });
    if let Some(tag) = super::bimlook::made_tag(e, "tag") {
        j["tag"] = json!(tag);
    }
    j
}

fn is_gear(e: &Entity) -> bool {
    super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment"))
}

fn run_chain(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.chain";
    let h = p.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex).ok_or_else(|| bad(CMD, "`handle` (hex) is required"))?;
    let limit = p.get("limit").and_then(Value::as_u64).map_or(2000, |n| n.clamp(1, MAX_WALK as u64) as usize);
    let d = s.doc()?;
    let start = d.model.get(h).cloned().ok_or_else(|| bad(CMD, format!("there is no object {} in the model", h.hex())))?;
    // Breadth first over `connects` to the nearest gear (a panel): the shortest feed.
    let mut prev: HashMap<Handle, Handle> = HashMap::new();
    let mut seen: BTreeSet<Handle> = BTreeSet::from([h]);
    let mut queue: VecDeque<Handle> = VecDeque::from([h]);
    let mut found: Option<Handle> = None;
    while let Some(x) = queue.pop_front() {
        if seen.len() > limit {
            break;
        }
        let Some(e) = d.model.get(x) else { continue };
        if x != h && is_gear(e) {
            found = Some(x);
            break;
        }
        for n in connects_of(e) {
            if seen.insert(n) {
                prev.insert(n, x);
                queue.push_back(n);
            }
        }
    }
    let mut path: Vec<Handle> = Vec::new();
    if let Some(mut x) = found {
        path.push(x);
        while let Some(&y) = prev.get(&x) {
            path.push(y);
            x = y;
            if path.len() > limit {
                break;
            }
        }
        path.reverse();
    }
    let length_ft: f64 = path
        .iter()
        .filter_map(|x| d.model.get(*x))
        .filter_map(|e| {
            let ps = props::read(&e.common.xdata);
            let k = ps.get(ATOM_KEY).map(Prop::text)?;
            matches!(k.as_str(), "conduit" | "cable").then(|| ps.get("length_ft").map(Prop::text).and_then(|t| t.parse::<f64>().ok()))?
        })
        .sum();
    // Up the feeders from the panel.
    let mut upstream = Vec::new();
    let mut at = found;
    let mut hops = 0;
    while let Some(x) = at {
        let Some(e) = d.model.get(x) else { break };
        let up = props::read(&e.common.xdata).get("fed_from").map(Prop::text).and_then(|t| Handle::parse_hex(t.trim()));
        match up.and_then(|u| d.model.get(u)) {
            Some(g) if hops < 32 && !upstream.iter().any(|v: &Value| v["handle"] == json!(g.handle.hex())) => {
                upstream.push(node_json(g));
                at = Some(g.handle);
                hops += 1;
            }
            _ => break,
        }
    }
    // The breaker its circuit leaves the panel on (Q6166): the device's, else a piece's on the way.
    let breaker = std::iter::once(h)
        .chain(path.iter().copied())
        .filter_map(|x| d.model.get(x))
        .find_map(|e| props::read(&e.common.xdata).get("breaker_handle").and_then(|p| Handle::parse_hex(p.text().trim())))
        .and_then(|b| d.model.get(b))
        .map(|e| {
            let ps = props::read(&e.common.xdata);
            let mut j = node_json(e);
            for k in ["type", "trip_a", "poles", "load", "panel", "source_row"] {
                if let Some(v) = ps.get(k).filter(|v| v.has_value()) {
                    j[k] = v.value.clone();
                }
            }
            j
        });
    Ok(json!({
        "from": node_json(&start),
        "breaker": breaker,
        "path": path.iter().filter_map(|x| d.model.get(*x)).map(|e| node_json(e)).collect::<Vec<_>>(),
        "source": found.and_then(|x| d.model.get(x)).map(|e| node_json(e)),
        "upstream": upstream,
        "length_ft": (length_ft * 100.0).round() / 100.0,
        "note": if found.is_none() { json!("no panel reached through what it connects to (an open end, or a run of no circuit)") } else { Value::Null },
    }))
}

/// How a device stands against the face of its wall: its front's distance from the face along
/// the face's outward direction (inches; positive: proud of the wall, negative: recessed).
fn wall_face_of(d: &Drawing, e: &Entity, upf: f64) -> Option<Value> {
    let ps = props::read(&e.common.xdata);
    // The face's direction is `face_normal` (older models kept it in `host_face`, now the face's name).
    let out = ["face_normal", "host_face"].iter().find_map(|k| {
        let t = ps.get(k)?.text();
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        match v[..] {
            [x, y] if x.is_finite() && y.is_finite() && x.hypot(y) > 1e-9 => Some(cadaddy_geom::Vec2::new(x, y) * (1.0 / x.hypot(y))),
            _ => None,
        }
    })?;
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    if b.is_empty() {
        return None;
    }
    let level = super::bspec::made_level(e).unwrap_or_default();
    let walls = super::propfill::wall_footprints(d);
    let centre = b.center();
    let f = super::propfill::wall_face(&walls, &level, centre, super::propfill::HOST_SNAP_FT * upf, None)?;
    // Its depth across the face, and its front: the side its room sees.
    let depth = (b.max - b.min).dot(cadaddy_geom::Vec2::new(out.x.abs(), out.y.abs()));
    let front = centre + out * (depth / 2.0);
    let off = (front - f.at).dot(f.out) / upf * 12.0;
    let r = (off * 100.0).round() / 100.0;
    let as_ = if off.abs() < 0.125 {
        "flush"
    } else if off > 0.0 {
        "proud"
    } else {
        "recessed"
    };
    Some(json!({ "inches": r, "as": as_, "wall": f.wall.hex() }))
}

/// What `bim.element` adds for an atom or a device: what it is as a piece, and its
/// neighbours; a device's box and what feeds it, and how it stands on its wall's face.
pub(crate) fn element_detail(d: &Drawing, e: &Entity, upf: f64) -> Option<Value> {
    let ps = props::read(&e.common.xdata);
    let t = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
    let neighbours = |hs: &[Handle]| -> Vec<Value> { hs.iter().take(64).filter_map(|h| d.model.get(*h)).map(|n| node_json(n)).collect() };
    if let Some(kind) = ps.get(ATOM_KEY).map(Prop::text) {
        let mut j = json!({
            "kind": kind,
            "item": t("item"),
            "type": t("type"),
            "size": t("size"),
            "run": t("run"),
            "circuit": t("circuit"),
            "panel": t("panel"),
            "system": t("system"),
            "room": t("room"),
            "estimated": t("estimated") == "true",
            "connects": neighbours(&connects_of(e)),
        });
        if let Some(l) = ps.get("length_ft").map(Prop::text).and_then(|v| v.parse::<f64>().ok()) {
            j["length_ft"] = json!(l);
        }
        for k in [
            "angle_deg",
            "cubic_in",
            "color",
            "conductor",
            "conductors",
            "cut",
            "depth_in",
            "box_of",
            "source_rule",
            "bends_deg",
            "review",
            "entry_gap_ft",
            "junction",
        ] {
            if let Some(v) = ps.get(k) {
                j[k] = json!(v.text());
            }
        }
        return Some(j);
    }
    let boxed = ps.get("box").map(Prop::text).and_then(|b| Handle::parse_hex(b.trim()));
    let face = wall_face_of(d, e, upf);
    if boxed.is_none() && face.is_none() {
        return None;
    }
    let fed: Vec<Handle> =
        ps.get("fed_by").map(Prop::text).map(|t| t.split([',', ' ']).filter_map(Handle::parse_hex).take(16).collect()).unwrap_or_default();
    let mut j = json!({ "box": boxed.and_then(|b| d.model.get(b)).map(|n| node_json(n)), "fed_by": neighbours(&fed) });
    if let Some(f) = face {
        j["wall_face"] = f;
    }
    Some(j)
}

/// Mark a source kind a rule (`company default`) as `default`, a code citation as `code`.
pub(crate) fn rule_source(text: &str) -> Source {
    let kind = if text.contains("company default") || text.starts_with("trade stock") || text.starts_with("the trade") {
        SourceKind::Default
    } else {
        SourceKind::Rule
    };
    Source::from_doc(kind, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ft: f64) -> Vec<Vec3> {
        vec![Vec3::new(0.0, 0.0, 120.0), Vec3::new(ft * 12.0, 0.0, 120.0)]
    }

    fn count(p: &Pieces, kind: &str) -> usize {
        p.atoms.iter().filter(|a| a.kind == kind).count()
    }

    fn said(v: &[(&str, &str)]) -> Vec<(String, String, String)> {
        v.iter().map(|(k, x)| (k.to_string(), x.to_string(), format!("doc of {k}"))).collect()
    }

    #[test]
    fn a_luminaires_box_is_decided_by_the_strongest_words_its_documents_say() {
        let lb = &rules().luminaire_box;
        // The cut sheet's canless wafer beats the schedule's "surface".
        let d = lb.decide(&said(&[("installation", "SURFACE/CEILING"), ("junction_box", "canless")]));
        assert!(!d.takes_box, "{d:?}");
        assert_eq!((d.found.as_str(), d.field.as_str(), d.doc.as_str()), ("canless", "junction_box", "doc of junction_box"));
        // A recessed row whose cut sheet mounts it on a junction box: a box.
        assert!(lb.decide(&said(&[("installation", "Recessed"), ("junction_box", "compatible junction box")])).takes_box);
        // The schedule's words alone, in any case and column.
        assert!(!lb.decide(&said(&[("fixture_description", "2x4 lay-in troffer")])).takes_box);
        assert!(!lb.decide(&said(&[("mounting_type", "recessed")])).takes_box);
        assert!(lb.decide(&said(&[("installation", "Pendant, 12' AFF")])).takes_box);
        // Nothing said: the usual box, said so.
        let none = lb.decide(&[]);
        assert!(none.takes_box && none.found.is_empty() && !none.source.is_empty(), "{none:?}");
        // A field the rules don't read decides nothing.
        assert!(lb.decide(&said(&[("finish", "recessed bronze")])).found.is_empty());
    }

    #[test]
    fn a_cut_sheet_is_read_for_its_junction_box_words_only() {
        let lb = &rules().luminaire_box;
        let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(lb.cut_sheet_words(&lines(&["ML9 4in CANLESS downlight", "Remote driver / junction box"])).as_deref(), Some("canless"));
        assert_eq!(lb.cut_sheet_words(&lines(&["Housing with integral junction box", "canless"])).as_deref(), Some("integral junction box"));
        assert_eq!(lb.cut_sheet_words(&lines(&["Universal J-box pattern on back plate"])).as_deref(), Some("j-box pattern"));
        // Mounting words on a cut sheet (recessed, surface, ceiling) are not its junction box.
        assert_eq!(lb.cut_sheet_words(&lines(&["Recessed or surface ceiling mounting", "LED 90 CRI"])), None);
    }

    #[test]
    fn a_feeders_wires_are_of_its_material_with_the_makeup_each_end_takes() {
        let rl = rules();
        let al = rl.feeder_conductors.get("al").expect("aluminum");
        let opts = WireOpts { kind: Some((al, "rule")), makeup_ft: [3.0, 0.0], makeup_source: "gear" };
        let w = wires_with(&line(20.0), 12.0, "3#4/0 + 1#4/0 + 1#4G", &[1, 3, 5], "120/208", rl, &opts);
        assert_eq!(w.len(), 5);
        assert!(w.iter().all(|a| (a.length_ft - 23.0).abs() < 1e-9 && a.item == "XHHW aluminum" && a.ty == "XHHW-2"), "{w:?}");
        assert_eq!(w.iter().filter(|a| a.size == "#4/0").count(), 4);
        // A branch circuit's wires stay THHN copper with the box makeup at both ends.
        let b = wires(&line(20.0), 12.0, "2#12 + 1#12G", &[1], "120/208", rl);
        assert!(b.iter().all(|a| (a.length_ft - (20.0 + 2.0 * rl.makeup_ft)).abs() < 1e-9), "{b:?}");
        assert_eq!(b.iter().filter(|a| a.item == "Copper ground, green").count(), 1);
    }

    #[test]
    fn a_37_ft_run_of_1_inch_emt_is_four_sticks_three_couplings_two_connectors() {
        let r = rules();
        let (name, emt) = r.raceway_named("1\" EMT").unwrap();
        assert_eq!(name, "EMT");
        let p = raceway_pieces(&line(37.0), 12.0, 1.0, name, emt, false, r);
        let sticks: Vec<f64> = p.atoms.iter().filter(|a| a.kind == "conduit").map(|a| (a.length_ft * 1000.0).round() / 1000.0).collect();
        assert_eq!(sticks, vec![10.0, 10.0, 10.0, 7.0]);
        assert_eq!(p.atoms.iter().filter(|a| a.kind == "conduit" && a.cut).count(), 1);
        assert_eq!(count(&p, "coupling"), 3);
        assert_eq!(count(&p, "connector"), 2);
        assert_eq!(count(&p, "locknut"), 2);
        assert_eq!(count(&p, "bushing"), 0);
        assert_eq!(count(&p, "elbow"), 0);
        assert!(p.atoms.iter().all(|a| a.size == "1\""));
        assert!(p.atoms.iter().filter(|a| a.kind == "coupling").all(|a| a.item == "EMT coupling"));
        // Straps within 3' of each end and no more than 10' apart: 3, ~13.3, ~23.7, 34.
        let st: Vec<f64> = p
            .atoms
            .iter()
            .filter(|a| a.kind == "strap")
            .filter_map(|a| a.extra.iter().find(|x| x.0 == "station_ft"))
            .map(|x| x.1.parse().unwrap())
            .collect();
        assert_eq!(st.first().copied(), Some(3.0));
        assert_eq!(st.last().copied(), Some(34.0));
        assert!(st.windows(2).all(|w| w[1] - w[0] <= 10.0 + 1e-9), "{st:?}");
        // Every coupling joins two sticks, both ways.
        for (i, c) in p.atoms.iter().enumerate().filter(|(_, a)| a.kind == "coupling") {
            assert_eq!(c.connects.len(), 2);
            for j in &c.connects {
                assert!(p.atoms[*j].connects.contains(&i));
            }
        }
        assert_eq!(p.ends.iter().flatten().count(), 2);
        assert!((p.length_ft - 37.0).abs() < 1e-9);
    }

    #[test]
    fn a_turn_is_an_elbow_by_its_angle_and_a_short_step_is_an_offset() {
        let r = rules();
        let (name, emt) = r.raceway_named("EMT").unwrap();
        // 12' east, then 8' north in 1-1/4" EMT: a factory elbow (90°), sticks 10 + 2 and 8.
        let path = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(144.0, 0.0, 0.0), Vec3::new(144.0, 96.0, 0.0)];
        let p = raceway_pieces(&path, 12.0, 1.25, name, emt, false, r);
        assert_eq!(count(&p, "elbow"), 1);
        let el = p.atoms.iter().find(|a| a.kind == "elbow").unwrap();
        assert_eq!((el.angle_deg, el.item.as_str(), el.size.as_str()), (90.0, "EMT 90° elbow", "1-1/4\""));
        assert_eq!(count(&p, "conduit"), 3);
        // couplings: stick-stick, stick-elbow, elbow-stick.
        assert_eq!(count(&p, "coupling"), 3);
        assert!((p.bend_deg - 90.0).abs() < 1e-6);
        // The same turn in 3/4": bent by hand, the sticks run on through it (10' and 10').
        let p = raceway_pieces(&path, 12.0, 0.75, name, emt, false, r);
        assert_eq!((count(&p, "elbow"), count(&p, "bend"), count(&p, "conduit"), count(&p, "coupling")), (0, 1, 2, 1));
        let bend = p.atoms.iter().find(|a| a.kind == "bend").unwrap();
        assert_eq!(bend.item, "EMT field bend 90°");
        assert!(is_labor_item(&bend.item) && !is_labor_item("EMT 90° elbow"));
        let bent = p.atoms.iter().find(|a| a.kind == "conduit" && !a.path.is_empty()).unwrap();
        assert!((bent.length_ft - 10.0).abs() < 1e-9 && bent.path.len() == 3, "the first stick turns the corner: {bent:?}");
        assert_eq!(bend.connects.len(), 1);
        assert!(std::ptr::eq(&p.atoms[bend.connects[0]], bent));
        // 10' along, a 6" step up, 10' on: an offset, no elbows (1-1/4": a factory offset).
        let path = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(120.0, 0.0, 0.0), Vec3::new(120.0, 0.0, 6.0), Vec3::new(240.0, 0.0, 6.0)];
        let p = raceway_pieces(&path, 12.0, 1.25, name, emt, false, r);
        assert_eq!((count(&p, "offset"), count(&p, "elbow"), count(&p, "conduit")), (1, 0, 2));
        let p = raceway_pieces(&path, 12.0, 0.75, name, emt, false, r);
        assert_eq!((count(&p, "offset"), count(&p, "bend"), count(&p, "conduit")), (1, 0, 3));
        assert_eq!(p.atoms.iter().find(|a| a.kind == "offset").map(|a| a.item.as_str()), Some("EMT field offset"));
        // Four 90s and a fifth: over the 360° between pull points.
        let zig: Vec<Vec3> = (0..7).map(|i| Vec3::new(f64::from(i / 2) * 60.0, f64::from((i + 1) / 2 % 2) * 60.0, 0.0)).collect();
        let p = raceway_pieces(&zig, 12.0, 0.75, name, emt, false, r);
        assert!(p.bend_deg > emt.max_bends_deg, "{}", p.bend_deg);
        // Large conductors: a bushing at each end.
        let p = raceway_pieces(&line(20.0), 12.0, 2.0, name, emt, large_conductors("4#2 + 1#6G", r.bushing_min_awg), r);
        assert_eq!(count(&p, "bushing"), 2);
        assert!(!large_conductors("2#12 + 1#12G", r.bushing_min_awg));
    }

    #[test]
    fn pvc_straps_closer_by_its_size_table() {
        let r = rules();
        let (name, pvc) = r.raceway_named("1\" PVC SCH 40").unwrap();
        assert_eq!(name, "PVC");
        assert_eq!(pvc.strap_spacing(1.0), 3.0);
        assert_eq!(pvc.strap_spacing(2.0), 5.0);
        assert_eq!(pvc.strap_spacing(6.0), 8.0);
        let p = raceway_pieces(&line(30.0), 12.0, 1.0, name, pvc, false, r);
        let n = count(&p, "strap");
        assert!(n >= 9, "{n}");
    }

    #[test]
    fn a_25_ft_nm_12_2_from_a_panel_to_a_receptacle_box() {
        let r = rules();
        let (kind, nm) = r.cable_named("ROMEX").unwrap();
        assert_eq!(kind, "NM");
        // 20' along the ceiling then a 5' drop in the wall: 25' of run.
        let path = vec![Vec3::new(0.0, 0.0, 108.0), Vec3::new(240.0, 0.0, 108.0), Vec3::new(240.0, 0.0, 48.0)];
        let p = cable_pieces(&path, 12.0, kind, nm, "2#12 + 1#12G", r);
        let c = &p.atoms[0];
        assert_eq!((c.kind, c.ty.as_str(), c.size.as_str(), c.item.as_str()), ("cable", "NM 12/2 w/G", "#12", "NM-B cable"));
        assert!((p.length_ft - 25.0).abs() < 1e-9);
        assert!((c.length_ft - 28.0).abs() < 1e-9, "25' of run plus 1.5' makeup at each end (300.14's 6\" and a 12\" allowance): {}", c.length_ft);
        assert_eq!(c.extra.iter().find(|x| x.0 == "conductors").map(|x| x.1.as_str()), Some("3"));
        // Staples within 12" of each box, at most 4-1/2' apart.
        let st: Vec<f64> = p
            .atoms
            .iter()
            .filter(|a| a.kind == "strap")
            .filter_map(|a| a.extra.iter().find(|x| x.0 == "station_ft"))
            .map(|x| x.1.parse().unwrap())
            .collect();
        assert_eq!(st.first().copied(), Some(1.0));
        assert_eq!(st.last().copied(), Some(24.0));
        assert!(st.windows(2).all(|w| w[1] - w[0] <= 4.5 + 1e-9));
        // NM into a nonmetallic box: no connector, the cable itself ends at both boxes.
        assert_eq!(p.ends, [Some(0), Some(0)]);
        // MC: a connector at each end.
        let (k, mc) = r.cable_named("MC CABLE").unwrap();
        let p = cable_pieces(&path, 12.0, k, mc, "3#10 + 1#10G", r);
        assert_eq!(p.atoms[0].ty, "MC 10/3 w/G");
        assert_eq!(p.atoms.iter().filter(|a| a.kind == "connector").count(), 2);
    }

    #[test]
    fn wires_take_their_phases_and_colours() {
        let r = rules();
        let w = wires(&line(30.0), 12.0, "2#12 + 1#12G", &[3], "120/208", r);
        let colors: Vec<&str> = w.iter().map(|a| a.extra.iter().find(|x| x.0 == "color").map_or("", |x| x.1.as_str())).collect();
        assert_eq!(colors, vec!["red", "white", "green"], "circuit 3 is phase B");
        assert!(w.iter().all(|a| (a.length_ft - 33.0).abs() < 1e-9), "30' and 1.5' at each end");
        let w = wires(&line(10.0), 12.0, "3#10 + 1#10G", &[1, 3, 5], "277/480", r);
        let colors: Vec<&str> = w.iter().map(|a| a.extra.iter().find(|x| x.0 == "color").map_or("", |x| x.1.as_str())).collect();
        assert_eq!(colors, vec!["brown", "orange", "yellow", "green"]);
        assert_eq!(phase_of(1), "A");
        assert_eq!(phase_of(4), "B");
        assert_eq!(phase_of(6), "C");
        assert_eq!(phase_of(7), "A");
    }

    #[test]
    fn the_rules_name_their_sources() {
        let r = rules();
        assert!(r.raceways.len() >= 4 && r.cables.len() >= 4);
        for (k, b) in &r.boxes {
            assert!(!b.source.is_empty() && !b.item.is_empty(), "box {k}");
        }
        assert_eq!(r.boxes.get("sq4-2.125").map(|b| b.cubic_in), Some(30.3));
        let (b, ring) = r.box_for("receptacle", false).unwrap();
        assert_eq!((b.item.as_str(), ring), ("Box, 4\" square", true));
        let (b, _) = r.box_for("receptacle", true).unwrap();
        assert!(b.item.contains("nonmetallic"));
        assert_eq!(r.box_for("luminaire", false).map(|b| b.0.item.as_str()), Some("Box, 4\" octagon"));
        assert!(r.box_for("plumbing fixture", false).is_none());
        assert!(has_word("3/4\" EMT", "EMT") && !has_word("EMC", "MC") && has_word("MC CABLE", "MC"));
    }

    #[test]
    fn the_index_adds_removes_and_filters() {
        let k = |kind: &str, size: &str, level: &str| Key {
            kind: kind.into(),
            size: size.into(),
            ty: "EMT".into(),
            level: level.into(),
            ..Key::default()
        };
        let mut ix = Index::default();
        for _ in 0..3 {
            ix.add(k("conduit", "1\"", "L1"), &Measure::length(10.0, false));
        }
        ix.add(k("conduit", "1\"", "L1"), &Measure::length(7.0, true));
        ix.add(k("conduit", "3/4\"", "L2"), &Measure::length(5.0, false));
        let f = Filter { kind: Some("stick".into()), size: Some("1".into()), ..Filter::default() };
        let (t, _) = ix.query(&f, None);
        assert_eq!((t.count, t.length_ft, t.estimated), (4, 37.0, 1));
        let (_, g) = ix.query(&Filter { kind: Some("conduit".into()), ..Filter::default() }, Some("level"));
        assert_eq!(g.get("L2").map(|t| t.count), Some(1));
        ix.remove(&k("conduit", "3/4\"", "L2"), &Measure::length(5.0, false));
        assert!(!ix.rows.contains_key(&k("conduit", "3/4\"", "L2")));
        let back = Index::from_bytes(&ix.to_bytes()).unwrap();
        assert_eq!(back, ix);
        assert!(Index::from_bytes(b"{not json").is_none());
    }

    #[test]
    fn hostile_paths_never_panic() {
        let r = rules();
        let (name, emt) = r.raceway_named("EMT").unwrap();
        for path in [
            vec![],
            vec![Vec3::ZERO],
            vec![Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
            vec![Vec3::ZERO, Vec3::ZERO],
            vec![Vec3::ZERO, Vec3::new(1e9, 0.0, 0.0)],
        ] {
            let _ = raceway_pieces(&path, 12.0, 1.0, name, emt, true, r);
            let _ = raceway_pieces(&path, f64::NAN, f64::NAN, name, emt, true, r);
            let (k, nm) = r.cable_named("NM").unwrap();
            let _ = cable_pieces(&path, 0.0, k, nm, "##", r);
            let _ = wires(&path, 12.0, "99#12", &[], "", r);
        }
        assert!(support_stations(f64::INFINITY, 3.0, 10.0).is_empty());
        assert!(support_stations(10.0, 3.0, 0.0).is_empty());
    }
}
