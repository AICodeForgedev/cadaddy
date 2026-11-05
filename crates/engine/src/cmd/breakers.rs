//! Breakers as model pieces (Q6166): each panel-schedule row becomes a breaker inside its panel.
//!
//! - **What it is**, from the row: its trip (amps), poles, type (standard, GFCI, AFCI,
//!   dual-function, GFEP, shunt, HACR: the row's codes read with the schedule's own legend, the
//!   rules data `breakers.json` for a code the legend leaves out), its frame and interrupting
//!   rating when the schedule gives them, its circuit number, the load it names and the row it
//!   came from.
//! - **Where it is**: in the panel element the schedule is for. A typical schedule
//!   (`TYPICAL OF n`) is every room panel of its letter's (a guest room's `G-<room>`): each built
//!   panel takes its own set, stamped like the devices.
//! - **What it connects to**: its panel, the raceway or cable its circuit lands on the panel
//!   with, and the devices on the circuit (their `breaker_handle`), so `bim.chain` reaches it.
//! - **Spares and spaces**: a spare is a breaker (bought, no load); a space is a place with no
//!   breaker. Each panel says how many of each, the poles it uses and has.
//! - **Plans against schedules**: a circuit the plans use that its panel's schedule has no row
//!   for, or a row no device, homerun or circuit tag on the plans uses, is a review item.
//!
//! The breakers are counted in the model's index (`bim.count {kind: breaker}`) and the BOM lists
//! them by type, trip and poles per panel type and level. Nothing here knows one job: every value
//! is the schedule's (with the sheet and row) or the rules data's (with its source).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, PanelCircuit, PanelSchedule};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const RULES: &str = include_str!("breakers.json");
/// The build's report among the drawing's embedded files (the review list reads it).
pub(crate) const REPORT_FILE: &str = "breakers/report.json";
/// The atom kind.
pub(crate) const KIND: &str = "breaker";
/// The system (and layer) breakers are on.
const SYSTEM: &str = "PANEL BREAKERS";
/// Most panels, rows per panel, devices, pieces and mismatches looked at (hostile specs).
const MAX_PANELS: usize = 20_000;
const MAX_ROWS: usize = 500;
const MAX_ELEMENTS: usize = 3_000_000;
const MAX_DEVICES: usize = 500_000;
const MAX_MISMATCHES: usize = 2_000;
/// Most devices one breaker lists among what it connects to.
const MAX_LINKS: usize = 200;
const CMD: &str = "bim.breakers";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Panel Breakers", run)
            .params("{panel?: tag (report one panel or panel type), rebuild?: bool (default true: the breakers built again from the schedules and linked to the devices as they are now), limit?: mismatches listed (50)} → {breakers, spares, spaces, panels: [{panel, panel_type, level, schedule, breakers, spares, spaces, poles_used, poles_total, poles_source}], by_type: [{panel_type, level, type, trip_a, poles, count}], devices_linked, devices_unlinked, mismatches_total, mismatches: [{kind: plan_no_row|row_no_plan|no_schedule|no_panel, panel, circuit, level, devices, rooms, description, sheet}], schedules_without_panel, typical: [{panel_type, typical_of, panels_built, rooms_without_panel}]}: each panel-schedule row as a breaker inside its panel (trip, poles, type, frame/AIC, circuit, load, source row), connected to its circuit and devices; spares and spaces counted; plan circuits and schedule rows that don't meet are review items")
            .noundo(),
    ]
}

// ─── Rules ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Feature {
    pub feature: String,
    pub codes: Vec<String>,
    pub words: Vec<String>,
    pub description_words: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TypeRule {
    #[serde(rename = "type")]
    pub ty: String,
    pub all: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub features: Vec<Feature>,
    pub types: Vec<TypeRule>,
    pub default_type: String,
    pub default_source: String,
    pub items: BTreeMap<String, String>,
    pub spare_words: Vec<String>,
    pub space_words: Vec<String>,
    pub pole_step: u32,
    pub pole_step_source: String,
    pub total_header_words: Vec<String>,
    pub aic_header_words: Vec<String>,
    pub frame_header_words: Vec<String>,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

impl Rules {
    fn item(&self, ty: &str) -> String {
        self.items.get(ty).cloned().unwrap_or_else(|| format!("Circuit breaker, {ty}"))
    }
    fn step(&self) -> u32 {
        self.pole_step.clamp(1, 2)
    }
}

/// Upper-case words of a text, for matching whole words and phrases.
fn norm(t: &str) -> String {
    let u: String =
        t.to_ascii_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '/' || c == '-' || c == '.' { c } else { ' ' }).collect();
    format!(" {} ", u.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn has_phrase(text: &str, phrase: &str) -> bool {
    let p = norm(phrase);
    !p.trim().is_empty() && norm(text).contains(&p)
}

/// What a breaker's codes say it is: its features (with where each was read) and its type.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Kind {
    pub ty: String,
    pub features: Vec<String>,
    /// The codes as the schedule prints them, each with what its legend says.
    pub codes: Vec<String>,
    pub source: String,
}

/// A row's type from its codes and the schedule's legend (the legend decides what its codes
/// mean; a code it leaves out is read by the rules' code lists), and its description's words
/// for the features a description can carry (`SHUNT TRIP`, `HACR`).
pub(crate) fn kind_of(c: &PanelCircuit, legend: &BTreeMap<String, String>, sheet: &str) -> Kind {
    let r = rules();
    let mut feats: Vec<String> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    for code in c.protection.iter().take(8) {
        let code = code.trim().to_ascii_uppercase();
        let meaning = legend.get(&code).map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
        let hit = match &meaning {
            Some(m) => r.features.iter().find(|f| f.words.iter().any(|w| has_phrase(m, w))),
            None => r.features.iter().find(|f| f.codes.iter().any(|k| k.eq_ignore_ascii_case(&code))),
        };
        said.push(match &meaning {
            Some(m) => format!("{code}: {m}"),
            None => code.clone(),
        });
        if let Some(f) = hit
            && !feats.contains(&f.feature)
        {
            feats.push(f.feature.clone());
        }
    }
    for f in &r.features {
        if !feats.contains(&f.feature) && f.description_words.iter().any(|w| has_phrase(&c.description, w)) {
            feats.push(f.feature.clone());
        }
    }
    let ty = r
        .types
        .iter()
        .find(|t| !t.all.is_empty() && t.all.iter().all(|f| feats.contains(f)))
        .map_or_else(|| if r.default_type.is_empty() { "standard".to_string() } else { r.default_type.clone() }, |t| t.ty.clone());
    let source = if said.is_empty() {
        format!("{sheet}: no type code on the row ({})", r.default_source)
    } else {
        format!("{sheet}: codes {} (the schedule's legend)", said.join("; "))
    };
    Kind { ty, features: feats, codes: said, source }
}

/// What a row is: a breaker (with a load), a spare breaker, or a space (no breaker).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Breaker,
    Spare,
    Space,
}

pub(crate) fn row_of(c: &PanelCircuit) -> Row {
    let r = rules();
    let d = c.description.trim().trim_end_matches(['.', ':']).trim().to_ascii_uppercase();
    let amps = trip(&c.breaker);
    if r.space_words.iter().any(|w| d == w.trim().to_ascii_uppercase()) || (d.is_empty() && amps.is_none()) {
        return Row::Space;
    }
    let first = d.split(|ch: char| !ch.is_ascii_alphanumeric()).find(|w| !w.is_empty()).unwrap_or_default();
    if r.spare_words.iter().any(|w| first == w.trim().to_ascii_uppercase()) {
        return Row::Spare;
    }
    Row::Breaker
}

/// A breaker's trip as the schedule prints it: `20`, `20A`, `60AT/100AF`, `3P-20A`.
pub(crate) fn trip(t: &str) -> Option<f64> {
    let u = t.trim().to_ascii_uppercase();
    // An explicit `AT` (amps trip) first.
    let toks: Vec<&str> = u.split(['/', ' ', ',', '-']).filter(|x| !x.is_empty()).collect();
    if let Some(at) = toks.iter().find_map(|x| x.strip_suffix("AT")).and_then(|n| n.trim().parse::<f64>().ok()) {
        return Some(at).filter(|v| v.is_finite() && *v > 0.0 && *v <= 6000.0);
    }
    toks.iter()
        .filter(|x| !x.ends_with("AF") && !x.ends_with('P'))
        .find_map(|x| x.trim_end_matches('A').parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 6000.0)
}

/// A breaker's frame when its text gives one (`60AT/100AF`, `100AF`).
fn frame(t: &str) -> Option<f64> {
    let u = t.trim().to_ascii_uppercase();
    u.split(['/', ' ', ',', '-'])
        .find_map(|x| x.strip_suffix("AF"))
        .and_then(|n| n.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 6000.0)
}

/// The first number of a text (`22,000 SERIES RATED` → 22000; `42` → 42).
fn first_number(t: &str) -> Option<f64> {
    let s: String = t.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    s.replace(',', "").trim_end_matches('.').parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

/// A schedule header's fact whose key names one of `words`.
fn header_fact<'a>(h: &'a BTreeMap<String, String>, words: &[String]) -> Option<(&'a str, &'a str)> {
    h.iter().find(|(k, v)| !v.trim().is_empty() && words.iter().any(|w| has_phrase(k, w))).map(|(k, v)| (k.as_str(), v.as_str()))
}

/// A device's or tag's circuit numbers on its panel: `G-13,15` → [13, 15]; `LP1-3` → [3];
/// `7` → [7]; `D-1A` → [1].
pub(crate) fn circuit_numbers(label: &str, panel: &str) -> Vec<u32> {
    let t = label.trim().to_ascii_uppercase();
    let p = panel.trim().to_ascii_uppercase();
    let rest = if !p.is_empty() && t.starts_with(&p) {
        t.get(p.len()..).unwrap_or("")
    } else {
        t.rsplit_once(['-', '/', '.']).map_or(t.as_str(), |(_, n)| n)
    };
    rest.split([',', '&', '+', ' ', '/'])
        .filter_map(|x| {
            let n: String = x.trim_start_matches(|c: char| !c.is_ascii_digit()).chars().take_while(char::is_ascii_digit).collect();
            n.parse::<u32>().ok().filter(|v| (1..=1000).contains(v))
        })
        .take(6)
        .collect()
}

/// The circuit numbers a breaker takes: its number and, for each further pole, the next on its
/// bus side; only its number on a schedule that numbers its breakers, not their poles.
fn takes(c: &PanelCircuit, by_breaker: bool) -> Vec<u32> {
    let Ok(n) = c.number.trim().parse::<u32>() else { return Vec::new() };
    if by_breaker {
        return vec![n];
    }
    let step = rules().step();
    (0..c.poles.clamp(1, 3)).map(|k| n + k * step).collect()
}

/// Whether a schedule numbers its breakers (a switchboard's `1, 2, 3`, each of three poles)
/// rather than their poles: a multi-pole row's further poles would be numbers other rows have.
pub(crate) fn numbered_by_breaker(s: &PanelSchedule) -> bool {
    let step = rules().step();
    let nums: BTreeSet<u32> = s.circuits.iter().take(MAX_ROWS).filter_map(|c| c.number.trim().parse::<u32>().ok()).collect();
    s.circuits
        .iter()
        .take(MAX_ROWS)
        .any(|c| c.poles > 1 && c.number.trim().parse::<u32>().is_ok_and(|n| (1..c.poles.min(3)).any(|k| nums.contains(&(n + k * step)))))
}

// ─── The model's facts ───────────────────────────────────────────────────────

/// A value of a build tag `prop:KEY=value` (equipment's schedule properties).
fn made_prop(e: &Entity, key: &str) -> Option<String> {
    super::bimlook::made_tags(e)?.iter().filter(|t| t.code == 1000).find_map(|t| {
        let (k, v) = t.value.strip_prefix("prop:")?.split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim().to_string())
    })
}

/// Whether an element is a breaker this build made.
pub(crate) fn is_breaker(e: &Entity) -> bool {
    super::bimlook::made_tag(e, "part").is_some_and(|p| p.eq_ignore_ascii_case("atom:breaker"))
}

/// A panel element: its handle, tag, level, the schedule letter it is a room's panel of (`PANEL`)
/// and its room (`ROOM`), and its box (model units).
#[derive(Clone, Debug)]
struct PanelEl {
    handle: Handle,
    tag: String,
    level: String,
    typical: Option<String>,
    room: String,
    lo: Vec3,
    hi: Vec3,
    stacked: Vec2,
    /// A panelboard or load center (a room's own panel can only be one of those).
    panel_kind: bool,
}

/// The kinds of gear a room's own panel is.
const ROOM_PANEL_KINDS: [&str; 5] = ["panel", "panelboard", "load_center", "loadcenter", "panel_board"];

fn panels_of(d: &Drawing) -> Vec<PanelEl> {
    d.model
        .iter()
        .filter(|e| super::wallgen::made_as(e).is_some_and(|p| p.eq_ignore_ascii_case("mep")))
        .filter(|e| super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")))
        .filter_map(|e| {
            let tag = super::bimlook::made_tag(e, "tag").map(str::trim).filter(|t| !t.is_empty())?.to_string();
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
            let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for v in m.vertices.iter().take(100_000) {
                lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
                hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
            }
            let ok = [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z].iter().all(|v| v.is_finite()) && hi.z > lo.z;
            ok.then(|| PanelEl {
                handle: e.handle,
                level: super::bspec::made_level(e).unwrap_or_default(),
                typical: made_prop(e, "PANEL").filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(&tag)),
                room: made_prop(e, "ROOM").unwrap_or_default(),
                tag,
                lo,
                hi,
                stacked: super::bspec::stacked_by(e),
                panel_kind: super::bimlook::made_tag(e, "part")
                    .and_then(|p| p.split_once(':'))
                    .is_some_and(|(_, k)| ROOM_PANEL_KINDS.iter().any(|x| k.trim().eq_ignore_ascii_case(x))),
            })
        })
        .take(MAX_PANELS)
        .collect()
}

/// A device on a circuit: its panel letter or tag, circuit numbers, room, level and the panel
/// element it is fed from when the fill said (`fed_from_handle`).
#[derive(Clone, Debug)]
struct Dev {
    handle: Handle,
    panel: String,
    label: String,
    numbers: Vec<u32>,
    room: String,
    level: String,
    fed_from: Option<Handle>,
}

fn devices_of(d: &Drawing) -> Vec<Dev> {
    d.model
        .iter()
        .filter(|e| super::wallgen::made_as(e).is_some_and(|p| p.eq_ignore_ascii_case("device") || p.eq_ignore_ascii_case("light")))
        .filter_map(|e| {
            let ps = props::read(&e.common.xdata);
            let text = |k: &str| ps.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).unwrap_or_default();
            let (panel, label) = (text("panel").to_ascii_uppercase(), text("circuit"));
            if panel.is_empty() || label.is_empty() {
                return None;
            }
            let numbers = circuit_numbers(&label, &panel);
            (!numbers.is_empty()).then(|| Dev {
                handle: e.handle,
                fed_from: Handle::parse_hex(&text("fed_from_handle")),
                room: text("room"),
                level: super::bspec::made_level(e).unwrap_or_default(),
                panel,
                label,
                numbers,
            })
        })
        .take(MAX_DEVICES)
        .collect()
}

/// The schedules by panel tag (upper case): the job's printed ones, then the equipment's own
/// circuit lists for a panel none is printed for.
fn schedules(sp: &BuildingSpec) -> BTreeMap<String, PanelSchedule> {
    let mut out: BTreeMap<String, PanelSchedule> = BTreeMap::new();
    for p in sp.mep.panel_schedules.iter().take(MAX_PANELS) {
        let k = p.panel.trim().to_ascii_uppercase();
        if !k.is_empty() && !out.contains_key(&k) {
            out.insert(k, p.clone());
        }
    }
    for e in sp.mep.equipment.iter().filter(|e| !e.circuits.is_empty()).take(MAX_PANELS) {
        let k = e.tag.trim().to_ascii_uppercase();
        if !k.is_empty() && !out.contains_key(&k) {
            let source = e.circuits.iter().map(|c| c.source.clone()).find(|s| !s.is_empty()).unwrap_or_else(|| "the equipment's schedule row".into());
            out.insert(k, PanelSchedule { panel: e.tag.clone(), source, circuits: e.circuits.clone(), ..PanelSchedule::default() });
        }
    }
    out
}

/// Whether a row feeds another panel (a panelboard row, or a feeder the spec has from this one
/// to a panel its description names): its load is downstream gear, not a plan circuit.
fn feeds_gear(c: &PanelCircuit, panel: &str, tags: &BTreeSet<String>, sp: &BuildingSpec) -> bool {
    let words: Vec<String> = c
        .description
        .to_ascii_uppercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    let names_tag = words.iter().any(|w| tags.contains(w) && w != panel);
    let gear_word = words.iter().any(|w| matches!(w.as_str(), "PANEL" | "PANELBOARD" | "PNL" | "PNLBD" | "SWITCHBOARD" | "TRANSFORMER" | "XFMR"));
    let feeder =
        sp.mep.feeders.iter().any(|f| f.from.trim().eq_ignore_ascii_case(panel) && words.iter().any(|w| w.eq_ignore_ascii_case(f.to.trim())));
    (names_tag && gear_word) || feeder || c.wire.to_ascii_lowercase().starts_with("feeder")
}

/// Whether a row serves equipment the model has (its description names an equipment tag).
fn serves_equipment(c: &PanelCircuit, equipment: &BTreeSet<String>) -> bool {
    c.description.to_ascii_uppercase().split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-').any(|w| !w.is_empty() && equipment.contains(w))
}

// ─── The build ───────────────────────────────────────────────────────────────

/// One breaker in the making.
struct Made {
    panel: usize,
    row: usize,
    numbers: Vec<u32>,
    label: String,
    kind: Kind,
    spare: bool,
}

/// A mismatch between the plans and a schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Mismatch {
    /// `plan_no_row`, `row_no_plan`, `no_schedule`, `no_panel`.
    pub kind: String,
    pub panel: String,
    pub circuit: String,
    pub level: String,
    pub devices: usize,
    pub rooms: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sheet: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<String>,
}

impl Mismatch {
    pub(crate) fn id(&self) -> String {
        format!("{}|{}|{}", self.kind, self.panel, self.circuit)
    }
    pub(crate) fn title(&self) -> String {
        match self.kind.as_str() {
            "plan_no_row" => format!(
                "Circuit {} is on the plans ({} device(s){}) but panel {}'s schedule ({}) has no breaker for it",
                self.circuit,
                self.devices,
                if self.rooms > 1 { format!(", {} rooms", self.rooms) } else { String::new() },
                self.panel,
                if self.sheet.is_empty() { "its schedule" } else { &self.sheet }
            ),
            "row_no_plan" => format!(
                "Panel {} circuit {} ({}) is on the schedule ({}) but nothing on the plans is on it",
                self.panel,
                self.circuit,
                if self.description.is_empty() { "no load named" } else { &self.description },
                if self.sheet.is_empty() { "its schedule" } else { &self.sheet }
            ),
            "no_panel" => format!("Panel {}'s schedule ({}) has no panel in the model to hold its breakers", self.panel, self.sheet),
            _ => format!("Panel {}: {} device(s) on its circuits but no schedule for it was read", self.panel, self.devices),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PanelUse {
    pub panel: String,
    pub panel_type: String,
    pub level: String,
    pub schedule: String,
    pub breakers: usize,
    pub spares: usize,
    pub spaces: usize,
    pub poles_used: u32,
    pub poles_total: u32,
    pub poles_source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ByType {
    pub panel_type: String,
    pub level: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub trip_a: Option<f64>,
    pub poles: u32,
    pub count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Typical {
    pub panel_type: String,
    pub typical_of: Option<u32>,
    pub panels_built: usize,
    pub rooms_without_panel: Vec<String>,
}

/// What the build made and found, stored in the drawing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Report {
    pub breakers: usize,
    pub spares: usize,
    pub spaces: usize,
    pub panels: Vec<PanelUse>,
    pub by_type: Vec<ByType>,
    pub devices_linked: usize,
    pub devices_unlinked: usize,
    pub mismatches: Vec<Mismatch>,
    pub mismatches_total: usize,
    pub schedules_without_panel: Vec<String>,
    pub typical: Vec<Typical>,
}

pub(crate) fn stored(d: &Drawing) -> Option<Report> {
    d.embedded.get(REPORT_FILE).and_then(|b| serde_json::from_slice(b).ok())
}

/// The breakers of the spec's panel schedules, built again inside the model's panels and linked
/// to its devices and runs; the model's count index kept current. The report.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Report {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // -- The breakers of an earlier build go (the index with them). --
    let old: Vec<Handle> = d.model.iter().filter(|e| is_breaker(e)).map(|e| e.handle).take(MAX_ELEMENTS).collect();
    let mut ix = super::mepatoms::stored(d);
    for h in &old {
        if let (Some(ix), Some((k, m))) = (ix.as_mut(), d.model.get(*h).and_then(|e| super::mepatoms::counted(e))) {
            ix.remove(&k, &m);
        }
        d.remove_entity(*h);
    }
    let gone: BTreeSet<Handle> = old.iter().copied().collect();
    let scheds = schedules(sp);
    let tags: BTreeSet<String> = scheds.keys().cloned().chain(sp.mep.equipment.iter().map(|e| e.tag.trim().to_ascii_uppercase())).collect();
    let equipment: BTreeSet<String> = sp.mep.equipment.iter().map(|e| e.tag.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()).collect();
    let mut panels = panels_of(d);
    // A room's own panel: a panelboard whose `PANEL` names a typical schedule (or whose tag is
    // `<letter>-<room>`). Other gear's `PANEL` is the panel feeding it (a damper's power box).
    for p in &mut panels {
        let keep = p.typical.as_ref().is_some_and(|t| {
            let t = t.to_ascii_uppercase();
            p.panel_kind && scheds.get(&t).is_some_and(|s| s.typical_of.is_some() || p.tag.to_ascii_uppercase().starts_with(&format!("{t}-")))
        });
        if !keep {
            p.typical = None;
        }
    }
    let panels = panels;
    let devices = devices_of(d);
    // Whether each schedule numbers its breakers rather than their poles.
    let by_breaker: BTreeMap<String, bool> = scheds.iter().map(|(k, s)| (k.clone(), numbered_by_breaker(s))).collect();
    let bb = |k: &str| by_breaker.get(k).copied().unwrap_or(false);
    // -- Each panel's schedule: its own, else the typical one its letter names. --
    let sched_of = |p: &PanelEl| -> Option<(&String, &PanelSchedule)> {
        scheds.get_key_value(&p.tag.to_ascii_uppercase()).or_else(|| p.typical.as_ref().and_then(|t| scheds.get_key_value(&t.to_ascii_uppercase())))
    };
    // Panel elements by handle, and the room panels of each letter by room.
    let by_handle: HashMap<Handle, usize> = panels.iter().enumerate().map(|(i, p)| (p.handle, i)).collect();
    let mut by_tag: HashMap<String, usize> = HashMap::new();
    let mut by_room: HashMap<(String, String), usize> = HashMap::new();
    for (i, p) in panels.iter().enumerate() {
        by_tag.entry(p.tag.to_ascii_uppercase()).or_insert(i);
        if let Some(t) = &p.typical {
            if !p.room.is_empty() {
                by_room.entry((t.to_ascii_uppercase(), p.room.trim().to_ascii_uppercase())).or_insert(i);
            }
            // `G-101` names its room too when the build gave it no `ROOM`.
            if let Some(room) = p.tag.to_ascii_uppercase().strip_prefix(&format!("{}-", t.to_ascii_uppercase())) {
                by_room.entry((t.to_ascii_uppercase(), room.to_string())).or_insert(i);
            }
        }
    }
    // -- Each device's panel element: the fill's, else its room's panel of its letter, else the
    // panel of its tag. --
    let mut unlinked_rooms: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let typical_letters: BTreeSet<String> = scheds
        .iter()
        .filter(|(_, s)| s.typical_of.is_some())
        .map(|(k, _)| k.clone())
        .chain(panels.iter().filter_map(|p| p.typical.as_ref().map(|t| t.to_ascii_uppercase())))
        .collect();
    let dev_panel: Vec<Option<usize>> = devices
        .iter()
        .map(|dv| {
            if let Some(i) = dv.fed_from.and_then(|h| by_handle.get(&h)) {
                return Some(*i);
            }
            let room = by_room.get(&(dv.panel.clone(), dv.room.trim().to_ascii_uppercase())).copied();
            let typical = typical_letters.contains(&dv.panel);
            match room {
                Some(i) => Some(i),
                None if typical => {
                    if !dv.room.is_empty() {
                        unlinked_rooms.entry(dv.panel.clone()).or_default().insert(dv.room.clone());
                    }
                    None
                }
                None => by_tag.get(&dv.panel).copied(),
            }
        })
        .collect();
    // Devices by (panel element, circuit number).
    let mut on: HashMap<(usize, u32), Vec<usize>> = HashMap::new();
    for (di, (dv, pi)) in devices.iter().zip(&dev_panel).enumerate() {
        if let (Some(pi), Some(n)) = (pi, dv.numbers.first()) {
            on.entry((*pi, *n)).or_default().push(di);
        }
    }
    // -- The runs that land on each panel, by circuit number: the raceway or cable a breaker feeds. --
    let panel_handles: HashMap<Handle, usize> = by_handle.clone();
    let mut lands: HashMap<(usize, u32), Vec<Handle>> = HashMap::new();
    for e in d.model.iter().filter(|e| super::mepatoms::is_atom(e) && !is_breaker(e) && !super::tradeatoms::is_trade_piece(e)).take(MAX_ELEMENTS) {
        let cn = super::mepatoms::connects_of(e);
        let Some(pi) = cn.iter().find_map(|h| panel_handles.get(h)) else { continue };
        let ps = props::read(&e.common.xdata);
        let circuit = ps.get("circuit").map(Prop::text).unwrap_or_default();
        let panel = ps.get("panel").map(Prop::text).unwrap_or_default();
        for n in circuit_numbers(&circuit, &panel).into_iter().take(1) {
            lands.entry((*pi, n)).or_default().push(e.handle);
        }
    }
    // The plan's circuit tags (homeruns): panel letter or tag → numbers.
    let mut tagged: BTreeMap<String, BTreeMap<u32, (String, String)>> = BTreeMap::new();
    for t in sp.tags.iter().filter(|t| t.kind == "circuit").take(MAX_DEVICES) {
        let f = |k: &str| t.fields.get(k).map(|v| v.value.trim().to_string()).unwrap_or_default();
        let (panel, label) = (f("panel").to_ascii_uppercase(), f("circuit"));
        for n in circuit_numbers(&label, &panel) {
            tagged.entry(panel.clone()).or_default().entry(n).or_insert_with(|| (label.clone(), t.level.clone()));
        }
    }

    // -- The breakers: each panel's schedule rows. --
    let mut made: Vec<Made> = Vec::new();
    let mut report = Report::default();
    let mut spaces_of: Vec<(usize, usize)> = Vec::new();
    for (pi, p) in panels.iter().enumerate() {
        let Some((skey, s)) = sched_of(p) else { continue };
        let panel_type = s.panel.trim().to_string();
        let by_breaker = bb(skey);
        let mut u = PanelUse {
            panel: p.tag.clone(),
            panel_type: panel_type.clone(),
            level: p.level.clone(),
            schedule: s.source.clone(),
            ..PanelUse::default()
        };
        let mut taken: BTreeSet<u32> = BTreeSet::new();
        for (ri, c) in s.circuits.iter().enumerate().take(MAX_ROWS) {
            let t = takes(c, by_breaker);
            if t.is_empty() {
                continue;
            }
            let row = row_of(c);
            taken.extend(t.iter().copied());
            if row == Row::Space {
                u.spaces += c.poles.clamp(1, 3) as usize;
                continue;
            }
            u.poles_used += c.poles.clamp(1, 3);
            if row == Row::Spare {
                u.spares += 1;
            } else {
                u.breakers += 1;
            }
            let label = format!("{}-{}", panel_type, t.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
            made.push(Made { panel: pi, row: ri, numbers: t, label, kind: kind_of(c, &s.legend, &s.source), spare: row == Row::Spare });
        }
        let highest = taken.iter().max().copied().unwrap_or(0);
        match header_fact(&s.header, &r.total_header_words).and_then(|(k, v)| first_number(v).map(|n| (k, v, n))) {
            Some((k, v, n)) if (1.0..=1000.0).contains(&n) => {
                u.poles_total = n as u32;
                u.poles_source = format!("{} header {k}: {v}", s.source);
            }
            _ if by_breaker => {
                u.poles_total = u.poles_used + u.spaces as u32;
                u.poles_source = format!("the poles of its rows ({}): it numbers its breakers, not their poles", s.source);
            }
            _ => {
                u.poles_total = highest + highest % 2;
                u.poles_source = format!("the highest circuit number on its schedule ({}), to an even count", s.source);
            }
        }
        spaces_of.push((pi, u.spaces));
        report.panels.push(u);
    }

    // -- Into the model: each breaker an element in its panel's box. --
    let mut handles: Vec<Option<Handle>> = Vec::with_capacity(made.len());
    let layer_of = |level: &str| super::mep::mep_layer_name(SYSTEM, level);
    for m in &made {
        let Some(p) = panels.get(m.panel) else {
            handles.push(None);
            continue;
        };
        let Some((skey, s)) = sched_of(p) else {
            handles.push(None);
            continue;
        };
        let Some(c) = s.circuits.get(m.row) else {
            handles.push(None);
            continue;
        };
        let mesh = body(p, m, s, bb(skey), upf);
        if mesh.faces.is_empty() {
            handles.push(None);
            continue;
        }
        let layer = layer_of(&p.level);
        if d.layer(&layer).is_none() {
            d.layers.push(cadaddy_doc::Layer {
                name: layer.as_str().into(),
                color: cadaddy_doc::color::Color::Index(140),
                on: true,
                ..cadaddy_doc::Layer::default()
            });
        }
        let tr = trip(&c.breaker);
        let size = size_text(tr, c.poles);
        let item = format!("{}, panel {}", r.item(&m.kind.ty), s.panel.trim());
        let tt = |code: i32, v: String| RawTag { code, value: v.chars().take(250).collect() };
        let mut tags = vec![
            tt(1001, super::wallgen::APP.into()),
            tt(1000, "mep".into()),
            tt(1000, p.level.clone()),
            tt(1000, p.handle.hex()),
            tt(1000, format!("system:{SYSTEM}")),
            tt(1000, format!("kind:{KIND}")),
            tt(1000, format!("part:atom:{KIND}")),
            tt(1000, format!("size:{size}")),
            tt(1000, format!("type:{item}")),
            tt(1000, format!("mepid:{}/breaker{}", p.tag, m.label)),
        ];
        if p.stacked.len() > 1e-9 {
            tags = super::bspec::with_stacked(&tags, p.stacked);
        }
        match d.add(&Space::Model, Common { layer: layer.into(), xdata: tags.into(), ..Common::default() }, EntityKind::Mesh(mesh)) {
            Ok(h) => handles.push(Some(h)),
            Err(_) => handles.push(None),
        }
    }

    // -- What each breaker is and connects to; the devices and runs their breaker. --
    let mut device_breaker: HashMap<Handle, (Handle, String)> = HashMap::new();
    let mut run_breaker: HashMap<Handle, Handle> = HashMap::new();
    let mut panel_breakers: BTreeMap<usize, Vec<Handle>> = BTreeMap::new();
    let mut typed: BTreeMap<(String, String, String, Option<i64>, u32), usize> = BTreeMap::new();
    for (m, h) in made.iter().zip(&handles) {
        let Some(h) = *h else { continue };
        let (Some(p), Some((_, s))) = (panels.get(m.panel), panels.get(m.panel).and_then(&sched_of)) else { continue };
        let Some(c) = s.circuits.get(m.row) else { continue };
        let row_src = Source::from_doc(SourceKind::Schedule, &format!("{} panel {} schedule, circuit {}", s.source, s.panel.trim(), c.number.trim()));
        let tr = trip(&c.breaker);
        let mut cn: Vec<Handle> = vec![p.handle];
        let first = m.numbers.first().copied().unwrap_or(0);
        let runs = lands.get(&(m.panel, first)).cloned().unwrap_or_default();
        cn.extend(runs.iter().take(MAX_LINKS));
        for rh in &runs {
            run_breaker.insert(*rh, h);
        }
        let devs = on.get(&(m.panel, first)).cloned().unwrap_or_default();
        for di in &devs {
            if let Some(dv) = devices.get(*di) {
                if cn.len() < MAX_LINKS * 2 {
                    cn.push(dv.handle);
                }
                device_breaker.insert(dv.handle, (h, format!("panel {} circuit {}", p.tag, m.label)));
            }
        }
        cn.sort_unstable();
        cn.dedup();
        let mut ps = Props::default();
        let computed = Source::computed;
        ps.set(super::mepatoms::ATOM_KEY, Prop::new(KIND, computed()));
        ps.set("item", Prop::new(format!("{}, panel {}", r.item(&m.kind.ty), s.panel.trim()), row_src.clone()));
        let mut tp = Prop::new(m.kind.ty.clone(), Source::from_doc(SourceKind::Schedule, &m.kind.source));
        tp.note = (!m.kind.codes.is_empty()).then(|| format!("codes {}", m.kind.codes.join("; ")));
        ps.set("type", tp);
        ps.set("size", Prop::new(size_text(tr, c.poles), row_src.clone()));
        match tr {
            Some(a) => {
                ps.set("trip_a", Prop::new(a, row_src.clone()).with_unit("A"));
            }
            None => {
                let mut x = Prop::new("", row_src.clone());
                x.note =
                    Some(format!("the schedule prints no trip for it ({})", if c.breaker.trim().is_empty() { "blank" } else { c.breaker.trim() }));
                ps.set("trip_a", x);
            }
        }
        ps.set("poles", Prop::new(c.poles.clamp(1, 3), row_src.clone()));
        if let Some(f) = frame(&c.breaker) {
            ps.set("frame_a", Prop::new(f, row_src.clone()).with_unit("A"));
        } else if let Some((k, v)) = header_fact(&s.header, &r.frame_header_words) {
            ps.set(
                "frame",
                Prop::new(v.trim().to_string(), Source::from_doc(SourceKind::Schedule, &format!("{} panel {} header {k}", s.source, s.panel.trim()))),
            );
        }
        if let Some((k, v)) = header_fact(&s.header, &r.aic_header_words) {
            let mut x =
                Prop::new(v.trim().to_string(), Source::from_doc(SourceKind::Schedule, &format!("{} panel {} header {k}", s.source, s.panel.trim())));
            x.note = Some("the panel's rating, its breakers' unless a row says otherwise".into());
            ps.set("aic", x);
        }
        if !m.kind.features.is_empty() {
            ps.set("features", Prop::new(m.kind.features.join(","), Source::from_doc(SourceKind::Schedule, &m.kind.source)));
        }
        ps.set("circuit", Prop::new(m.label.clone(), row_src.clone()));
        ps.set("circuit_number", Prop::new(c.number.trim().to_string(), row_src.clone()));
        ps.set("load", Prop::new(c.description.trim().to_string(), row_src.clone()));
        if m.spare {
            ps.set("spare", Prop::new("true", row_src.clone()));
        }
        ps.set("panel", Prop::new(p.tag.clone(), computed()));
        ps.set("panel_type", Prop::new(s.panel.trim().to_string(), row_src.clone()));
        ps.set(
            "source_row",
            Prop::new(
                format!("{} panel {} row {}: circuit {} {}", s.source, s.panel.trim(), m.row + 1, c.number.trim(), c.description.trim()),
                row_src.clone(),
            ),
        );
        if !c.wire.trim().is_empty() {
            ps.set("wire", Prop::new(c.wire.trim().to_string(), row_src.clone()));
        }
        if let Some(va) = c.load_va.filter(|v| v.is_finite() && *v >= 0.0) {
            ps.set("load_va", Prop::new(va, row_src.clone()).with_unit("VA"));
        }
        ps.set("system", Prop::new(SYSTEM, computed()));
        if !p.room.is_empty() {
            ps.set("room", Prop::new(p.room.clone(), computed()));
        }
        ps.set("devices", Prop::new(devs.len() as u64, computed()));
        ps.set("connects", Prop::new(cn.iter().map(|x| x.hex()).collect::<Vec<_>>().join(","), computed()));
        let _ = d.modify_entity(h, |e| {
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
        panel_breakers.entry(m.panel).or_default().push(h);
        *typed
            .entry((s.panel.trim().to_string(), p.level.clone(), m.kind.ty.clone(), tr.map(|a| (a * 10.0).round() as i64), c.poles.clamp(1, 3)))
            .or_default() += 1;
        if m.spare {
            report.spares += 1;
        } else {
            report.breakers += 1;
        }
        if let Some(ix) = ix.as_mut()
            && let Some((k, mm)) = d.model.get(h).and_then(|e| super::mepatoms::counted(e))
        {
            ix.add(k, &mm);
        }
    }
    report.spaces = spaces_of.iter().map(|(_, n)| n).sum();
    report.by_type = typed
        .into_iter()
        .map(|((panel_type, level, ty, trip_a, poles), count)| ByType {
            panel_type,
            level,
            ty,
            trip_a: trip_a.map(|t| t as f64 / 10.0),
            poles,
            count,
        })
        .collect();

    // -- The panels: their breakers among what they connect to, and their space use. --
    for (pi, hs) in &panel_breakers {
        let Some(p) = panels.get(*pi) else { continue };
        let Some(u) = report.panels.iter().find(|u| u.panel == p.tag).cloned() else { continue };
        let _ = d.modify_entity(p.handle, |e| {
            let mut ps = props::read(&e.common.xdata);
            let mut all: Vec<Handle> = ps
                .get("connects")
                .map(Prop::text)
                .map(|t| t.split([',', ' ']).filter_map(Handle::parse_hex).filter(|h| !gone.contains(h)).collect())
                .unwrap_or_default();
            all.extend(hs.iter().copied());
            all.sort_unstable();
            all.dedup();
            let src = Source::from_doc(SourceKind::Schedule, &u.schedule);
            ps.set("connects", Prop::new(all.iter().take(4000).map(|h| h.hex()).collect::<Vec<_>>().join(","), Source::computed()));
            ps.set("breakers", Prop::new(u.breakers as u64, src.clone()));
            ps.set("spares", Prop::new(u.spares as u64, src.clone()));
            ps.set("spaces", Prop::new(u.spaces as u64, src.clone()));
            ps.set("poles_used", Prop::new(u.poles_used, src.clone()));
            let mut t = Prop::new(u.poles_total, src);
            t.note = Some(u.poles_source.clone());
            ps.set("poles_total", t);
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    // Devices and the runs landing on the panel: their breaker.
    for (h, (b, what)) in &device_breaker {
        let _ = d.modify_entity(*h, |e| {
            let mut ps = props::read(&e.common.xdata);
            let mut p = Prop::new(b.hex(), Source::computed());
            p.note = Some(format!("its circuit's breaker: {what}"));
            ps.set("breaker_handle", p);
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    for (h, b) in &run_breaker {
        let _ = d.modify_entity(*h, |e| {
            let mut ps = props::read(&e.common.xdata);
            ps.set("breaker_handle", Prop::new(b.hex(), Source::computed()));
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    // Devices whose breaker an earlier build made and this one didn't: no stale link.
    if !gone.is_empty() {
        let stale: Vec<Handle> = devices
            .iter()
            .filter(|dv| !device_breaker.contains_key(&dv.handle))
            .filter(|dv| {
                d.model.get(dv.handle).is_some_and(|e| {
                    props::read(&e.common.xdata).get("breaker_handle").and_then(|p| Handle::parse_hex(&p.text())).is_some_and(|h| gone.contains(&h))
                })
            })
            .map(|dv| dv.handle)
            .collect();
        for h in stale {
            let _ = d.modify_entity(h, |e| {
                let mut ps = props::read(&e.common.xdata);
                ps.remove("breaker_handle");
                let x = props::write(&e.common.xdata, &ps);
                e.common.xdata = x.into();
            });
        }
    }
    report.devices_linked = device_breaker.len();
    report.devices_unlinked = devices.len().saturating_sub(device_breaker.len());

    // -- The plans against the schedules. --
    let mut mism: Vec<Mismatch> = Vec::new();
    // A plan circuit by (schedule key, number): its devices, rooms, levels and a few handles.
    let mut plan: BTreeMap<(String, u32), (usize, BTreeSet<String>, String, Vec<String>, String)> = BTreeMap::new();
    for dv in &devices {
        let Some(n) = dv.numbers.first() else { continue };
        let slot = plan.entry((dv.panel.clone(), *n)).or_insert_with(|| (0, BTreeSet::new(), dv.level.clone(), Vec::new(), dv.label.clone()));
        slot.0 += 1;
        if !dv.room.is_empty() {
            slot.1.insert(dv.room.clone());
        }
        if slot.3.len() < 8 {
            slot.3.push(dv.handle.hex());
        }
    }
    for (panel, nums) in &tagged {
        for (n, (label, level)) in nums {
            plan.entry((panel.clone(), *n)).or_insert_with(|| (0, BTreeSet::new(), level.clone(), Vec::new(), label.clone()));
        }
    }
    // The runs landing on panels count as plan circuits of their schedule.
    let mut landed: BTreeSet<(String, u32)> = BTreeSet::new();
    for (pi, n) in lands.keys() {
        if let Some((k, _)) = panels.get(*pi).and_then(&sched_of) {
            landed.insert((k.clone(), *n));
        }
    }
    let mut no_schedule: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for ((panel, n), (count, rooms, level, els, label)) in &plan {
        let Some(s) = scheds.get(panel) else {
            if *count > 0 {
                let slot = no_schedule.entry(panel.clone()).or_insert((0, level.clone()));
                slot.0 += count;
            }
            continue;
        };
        let row = s.circuits.iter().take(MAX_ROWS).find(|c| takes(c, bb(panel)).contains(n));
        if row.is_none_or(|c| row_of(c) == Row::Space) {
            mism.push(Mismatch {
                kind: "plan_no_row".into(),
                panel: s.panel.trim().to_string(),
                circuit: label.clone(),
                level: level.clone(),
                devices: *count,
                rooms: rooms.len(),
                description: row.map(|c| c.description.trim().to_string()).unwrap_or_default(),
                sheet: s.source.clone(),
                elements: els.clone(),
            });
        }
    }
    for (panel, (count, level)) in &no_schedule {
        // Only a panel the model has or the plans name with a schedule elsewhere: a stray letter
        // is the reader's (its devices say which).
        mism.push(Mismatch { kind: "no_schedule".into(), panel: panel.clone(), level: level.clone(), devices: *count, ..Mismatch::default() });
    }
    for (key, s) in &scheds {
        for c in s.circuits.iter().take(MAX_ROWS) {
            if row_of(c) != Row::Breaker {
                continue;
            }
            let t = takes(c, bb(key));
            let used = t.iter().any(|n| plan.contains_key(&(key.clone(), *n)) || landed.contains(&(key.clone(), *n)));
            if used || feeds_gear(c, key, &tags, sp) || serves_equipment(c, &equipment) {
                continue;
            }
            mism.push(Mismatch {
                kind: "row_no_plan".into(),
                panel: s.panel.trim().to_string(),
                circuit: format!("{}-{}", s.panel.trim(), c.number.trim()),
                description: c.description.trim().to_string(),
                sheet: s.source.clone(),
                ..Mismatch::default()
            });
        }
        let placed = panels.iter().any(|p| p.tag.eq_ignore_ascii_case(key) || p.typical.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(key)));
        if !placed {
            report.schedules_without_panel.push(s.panel.trim().to_string());
            mism.push(Mismatch { kind: "no_panel".into(), panel: s.panel.trim().to_string(), sheet: s.source.clone(), ..Mismatch::default() });
        }
    }
    report.mismatches_total = mism.len();
    mism.truncate(MAX_MISMATCHES);
    report.mismatches = mism;
    // The typical schedules: how many of their panels the model has, rooms with none.
    for (key, s) in scheds.iter().filter(|(_, s)| s.typical_of.is_some()) {
        let built = panels.iter().filter(|p| p.typical.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(key))).count();
        report.typical.push(Typical {
            panel_type: s.panel.trim().to_string(),
            typical_of: s.typical_of,
            panels_built: built,
            rooms_without_panel: unlinked_rooms.get(key).map(|r| r.iter().take(500).cloned().collect()).unwrap_or_default(),
        });
    }
    if let Some(ix) = ix.as_ref() {
        super::mepatoms::store(d, ix);
    } else if !made.is_empty() {
        super::mepatoms::reindex(d);
    }
    if let Ok(b) = serde_json::to_vec(&report) {
        d.embedded.insert(REPORT_FILE.into(), std::sync::Arc::new(b));
    }
    report
}

/// `20A 1P`; `2P` when the trip isn't printed.
fn size_text(trip: Option<f64>, poles: u32) -> String {
    let p = poles.clamp(1, 3);
    match trip {
        Some(a) => format!("{}A {p}P", (a * 10.0).round() / 10.0),
        None => format!("{p}P"),
    }
}

/// A breaker's body inside its panel's box: odd circuits on the left, even on the right, top
/// down, each pole a row's height.
fn body(p: &PanelEl, m: &Made, s: &PanelSchedule, by_breaker: bool, upf: f64) -> cadaddy_doc::Mesh {
    let mut mesh = super::wallgen::empty_mesh();
    let (dx, dy, dz) = (p.hi.x - p.lo.x, p.hi.y - p.lo.y, p.hi.z - p.lo.z);
    let along_x = dx >= dy;
    let (w, depth) = if along_x { (dx, dy) } else { (dy, dx) };
    if !(w > 1e-9 && dz > 1e-9) {
        return mesh;
    }
    let highest = s.circuits.iter().take(MAX_ROWS).flat_map(|c| takes(c, by_breaker)).max().unwrap_or(2).max(2);
    let rows = f64::from(highest.div_ceil(2).max(1));
    let n = m.numbers.first().copied().unwrap_or(1).max(1);
    let row = f64::from((n - 1) / 2);
    let pitch = dz * 0.8 / rows;
    let poles = m.numbers.len().max(1) as f64;
    let z1 = p.hi.z - dz * 0.1 - row * pitch - pitch * 0.05;
    let z0 = z1 - (pitch * poles - pitch * 0.1).max(pitch * 0.5);
    let side = if n % 2 == 1 { -0.25 } else { 0.25 };
    let half = (w * 0.2).min(1.5 / 12.0 * upf).max(1e-6);
    let thick = (depth * 0.6).max(1e-6).min(3.0 / 12.0 * upf);
    let c = Vec2::new((p.lo.x + p.hi.x) * 0.5, (p.lo.y + p.hi.y) * 0.5);
    let (a, b) = if along_x {
        let x = c.x + side * w;
        (Vec2::new(x - half, c.y), Vec2::new(x + half, c.y))
    } else {
        let y = c.y + side * w;
        (Vec2::new(c.x, y - half), Vec2::new(c.x, y + half))
    };
    super::wallgen::add_box(&mut mesh, a, b, thick, z0.max(p.lo.z), z1.min(p.hi.z));
    mesh
}

// ─── Command ─────────────────────────────────────────────────────────────────

fn report_json(r: &Report, panel: Option<&str>, limit: usize) -> Value {
    let want = |t: &str, ty: &str| panel.is_none_or(|w| t.eq_ignore_ascii_case(w) || ty.eq_ignore_ascii_case(w));
    let panels: Vec<&PanelUse> = r.panels.iter().filter(|u| want(&u.panel, &u.panel_type)).collect();
    let mism: Vec<&Mismatch> = r.mismatches.iter().filter(|m| panel.is_none_or(|w| m.panel.eq_ignore_ascii_case(w))).collect();
    // Room panels of one type are many alike: listed once each, the first 200.
    json!({
        "breakers": r.breakers, "spares": r.spares, "spaces": r.spaces,
        "panels_total": panels.len(),
        "panels": panels.iter().take(200).collect::<Vec<_>>(),
        "by_type": r.by_type.iter().filter(|b| panel.is_none_or(|w| b.panel_type.eq_ignore_ascii_case(w))).take(500).collect::<Vec<_>>(),
        "devices_linked": r.devices_linked, "devices_unlinked": r.devices_unlinked,
        "mismatches_total": if panel.is_some() { mism.len() } else { r.mismatches_total },
        "mismatches_by_kind": mism.iter().fold(BTreeMap::<String, usize>::new(), |mut m, x| { *m.entry(x.kind.clone()).or_default() += 1; m }),
        "mismatches": mism.iter().take(limit).map(|m| json!({"kind": m.kind, "panel": m.panel, "circuit": m.circuit, "level": m.level, "devices": m.devices, "rooms": m.rooms, "description": m.description, "sheet": m.sheet, "title": m.title()})).collect::<Vec<_>>(),
        "schedules_without_panel": r.schedules_without_panel,
        "typical": r.typical.iter().map(|t| json!({"panel_type": t.panel_type, "typical_of": t.typical_of, "panels_built": t.panels_built, "rooms_without_panel": t.rooms_without_panel.len(), "rooms": t.rooms_without_panel.iter().take(50).collect::<Vec<_>>()})).collect::<Vec<_>>(),
    })
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, stored_spec) = super::bspec::current(s, CMD)?;
    let panel = str_param(p, "panel").map(str::trim).filter(|t| !t.is_empty());
    let limit = p.get("limit").and_then(Value::as_u64).map_or(50, |n| n.clamp(1, 500) as usize);
    let rebuild = bool_or(p, "rebuild", true);
    if !stored_spec || sp.mep.panel_schedules.is_empty() && sp.mep.equipment.iter().all(|e| e.circuits.is_empty()) {
        return Ok(
            json!({"breakers": 0, "note": "no panel schedules in this drawing: a job reads them off the electrical sheets (job.start), panels.schedule shows them"}),
        );
    }
    let r = if rebuild {
        let r = build(s.doc_mut()?, &sp);
        s.touch();
        r
    } else {
        stored(s.doc()?).ok_or_else(|| bad(CMD, "no breakers built yet: model.build makes them, or bim.breakers {rebuild: true}"))?
    };
    Ok(report_json(&r, panel, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ckt(n: &str, desc: &str, amps: &str, poles: u32, codes: &[&str]) -> PanelCircuit {
        PanelCircuit {
            number: n.into(),
            description: desc.into(),
            breaker: amps.into(),
            poles,
            protection: codes.iter().map(|c| c.to_string()).collect(),
            ..PanelCircuit::default()
        }
    }

    #[test]
    fn a_rows_codes_read_with_its_legend_give_its_type() {
        let legend: BTreeMap<String, String> = [
            ("AF", "COMBINATION TYPE ARC FAULT CIRCUIT INTERRUPTER"),
            ("GF", "GROUND FAULT INTERRUPTER"),
            ("GFEP", "GROUND FAULT EQUIPMENT PROTECTION"),
            ("ST", "SHUNT TRIP"),
            ("LT", "LOCKING TAB"),
            ("X", "DUAL FUNCTION AFCI/GFCI"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let ty = |codes: &[&str], desc: &str| kind_of(&ckt("1", desc, "20", 1, codes), &legend, "S-1").ty;
        assert_eq!(ty(&["AF"], "BEDROOM"), "AFCI");
        assert_eq!(ty(&["GF"], "BATH"), "GFCI");
        assert_eq!(ty(&["AF", "GF"], "KITCHEN"), "dual-function");
        // The legend's word wins over the code: its own code letters for dual function.
        assert_eq!(ty(&["X"], "LAUNDRY"), "dual-function");
        // Equipment ground fault is not personnel GFCI though its meaning says ground fault.
        assert_eq!(ty(&["GFEP"], "HEAT TRACE"), "GFEP");
        assert_eq!(ty(&["ST"], "ELEVATOR"), "shunt");
        // A locking tab is an accessory, no protection: a standard breaker.
        assert_eq!(ty(&["LT"], "EXIT SIGNS"), "standard");
        assert_eq!(ty(&[], "LIGHTS"), "standard");
        // No legend for a code: the rules' code list reads it; the description can say HACR.
        let none = BTreeMap::new();
        assert_eq!(kind_of(&ckt("3", "RTU-1", "40", 2, &["AFCI"]), &none, "S-1").ty, "AFCI");
        assert_eq!(kind_of(&ckt("3", "CONDENSER (HACR)", "40", 2, &[]), &none, "S-1").ty, "HACR");
        let k = kind_of(&ckt("5", "RCPT", "20", 1, &["AF", "GF"]), &legend, "S-1");
        assert!(k.source.contains("S-1") && k.source.contains("GF: GROUND FAULT INTERRUPTER"), "{k:?}");
    }

    #[test]
    fn spares_spaces_trips_and_circuit_labels() {
        assert_eq!(row_of(&ckt("10", "SPACE ONLY", "", 1, &[])), Row::Space);
        assert_eq!(row_of(&ckt("12", "", "", 1, &[])), Row::Space);
        assert_eq!(row_of(&ckt("14", "SPARE", "20", 1, &[])), Row::Spare);
        assert_eq!(row_of(&ckt("16", "Spare 20A", "20", 1, &[])), Row::Spare);
        // A space heater is a load, not a space.
        assert_eq!(row_of(&ckt("18", "SPACE HEATER", "20", 1, &[])), Row::Breaker);
        assert_eq!(trip("20"), Some(20.0));
        assert_eq!(trip("20A"), Some(20.0));
        assert_eq!(trip("60AT/100AF"), Some(60.0));
        assert_eq!(frame("60AT/100AF"), Some(100.0));
        assert_eq!(trip("OL"), None);
        assert_eq!(circuit_numbers("G-13,15", "G"), vec![13, 15]);
        assert_eq!(circuit_numbers("LP1-3", "LP1"), vec![3]);
        assert_eq!(circuit_numbers("LP1-3", ""), vec![3]);
        assert_eq!(circuit_numbers("7", "A"), vec![7]);
        assert_eq!(takes(&ckt("13", "PTAC", "20", 2, &[]), false), vec![13, 15]);
        assert_eq!(takes(&ckt("1", "MAIN", "100", 3, &[]), false), vec![1, 3, 5]);
        assert_eq!(takes(&ckt("1", "MAIN", "100", 3, &[]), true), vec![1]);
    }

    #[test]
    fn a_schedule_numbers_its_poles_or_its_breakers() {
        let sched = |rows: Vec<PanelCircuit>| PanelSchedule { panel: "S".into(), circuits: rows, ..PanelSchedule::default() };
        // Two-sided by poles: 13 is two poles (13, 15), and 15 is no row of its own.
        let poles = sched(vec![ckt("1", "A", "20", 1, &[]), ckt("2", "B", "20", 1, &[]), ckt("13", "C", "20", 2, &[]), ckt("14", "D", "20", 1, &[])]);
        assert!(!numbered_by_breaker(&poles));
        // A switchboard's rows 1, 2, 3, each of three poles: by breaker.
        let rows = sched(vec![ckt("1", "PANEL A", "200", 3, &[]), ckt("2", "PANEL B", "200", 3, &[]), ckt("3", "SPACE", "", 3, &[])]);
        assert!(numbered_by_breaker(&rows));
        assert_eq!(size_text(Some(20.0), 1), "20A 1P");
        assert_eq!(size_text(None, 2), "2P");
    }
}
