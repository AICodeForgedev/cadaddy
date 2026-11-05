//! `bim.loadcalc` (Q6194): an NEC Article 220-style load calculation from the MODEL, checked
//! against the documents.
//!
//! - **Connected loads**, each with its source: a luminaire's VA from the fixture schedule (its
//!   `fixture_va`), a receptacle's by rule (180 VA a yoke, NEC 220.14(I)), equipment from its
//!   submittal (kW, else volts × full-load / nameplate / minimum circuit amps), a control device
//!   none; a circuit with no load in the model keeps its schedule's (said so).
//! - **Per circuit → panel → feeder → service**: a circuit naming another panel (`PANELBOARD DP-N`,
//!   `PANELBOARD U (4 UNITS)`) is that panel's feeder, its units' loads under it; the panel no
//!   circuit feeds is the service. A typical panel (`TYPICAL OF n`) is one unit's: its devices
//!   over the rooms that have them.
//! - **Demand factors** from rules data (`loadcalc.json`, each with its cite): the occupancy is
//!   the code sheet's (R-1 hotel, R-2 dwelling, the rest commercial) and applies to the unit rooms'
//!   general lighting (their receptacles in it); the other rooms' lighting at 100%, receptacles by
//!   NEC 220.44, kitchen equipment by 220.56, 25% of the largest motor, continuous loads at 125%.
//! - **Checks**: `E-LOAD-CIRCUIT` (a continuous load over 80% of its breaker, any load over it),
//!   `E-LOAD-RATING` (a panel's demand over its bus or main, a feeder's over its breaker or its
//!   conductors' ampacity, the service's over its size), `E-LOAD-PHASE` (phase imbalance over the
//!   rules' limit, the model's and the schedule's), `E-LOAD-SCHEDULE` (a schedule's totals that
//!   aren't its circuits', a feeder row that isn't its panel's total, the model's loads against the
//!   schedule's beyond tolerance) — each saying which side is likely wrong and why.
//! - **Report**: per panel and the service, CSV and text, a source on every line.

mod read;

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::{BuildingSpec, EquipmentElectrical, Feeder, LoadSummary, PanelCircuit, PanelSchedule};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub(crate) use read::read as read_summaries;

const CMD: &str = "bim.loadcalc";
const RULES: &str = include_str!("loadcalc.json");
/// Elements looked at, panels and circuits calculated, findings listed (hostile models).
const MAX_ELEMENTS: usize = 2_000_000;
const MAX_PANELS: usize = 500;
const MAX_CIRCUITS: usize = 400;
const MAX_FINDINGS: usize = 300;
/// How deep feeders nest (service → distribution → panel → …), at most.
const MAX_DEPTH: usize = 12;
/// Units one feeder row can carry (`(4 UNITS)`).
const MAX_UNITS: u32 = 1_000;
const SQRT3: f64 = 1.732_050_807_568_877_2;

// ---------------------------------------------------------------- rules

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct UnitLoad {
    pub kinds: Vec<String>,
    pub category: String,
    pub va: Option<f64>,
    pub cite: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Continuous {
    pub categories: Vec<String>,
    pub description_words: Vec<String>,
    pub factor: f64,
    pub cite: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Occupancy {
    pub id: String,
    pub groups: Vec<String>,
    pub name: String,
    pub unit_rooms: bool,
    pub unit_room_words: Vec<String>,
    pub unit_receptacles_in_lighting: bool,
    pub unit_receptacles_cite: String,
    pub lighting_steps: Vec<(Option<f64>, f64)>,
    pub lighting_cite: String,
    pub fixed_appliances_min: Option<u32>,
    pub fixed_appliances_factor: Option<f64>,
    pub fixed_appliances_cite: String,
    pub optional_units: Vec<(u32, f64)>,
    pub optional_cite: String,
    /// Units with cooking in them are wired as this occupancy's (`dwelling`), and why.
    pub cooking_units_as: String,
    pub cooking_cite: String,
}

/// A unit room's small-appliance branch circuits: the words that name one, its VA, the cite.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SmallAppliance {
    pub words: Vec<String>,
    pub va: f64,
    pub cite: String,
}

/// The load calculation's rules (`loadcalc.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub unit_loads: Vec<UnitLoad>,
    pub small_appliance: SmallAppliance,
    pub continuous: Continuous,
    pub circuit_limit_pct: f64,
    pub circuit_limit_cite: String,
    pub phase_imbalance_pct: f64,
    pub phase_imbalance_cite: String,
    pub tolerance_pct: f64,
    pub tolerance_va: f64,
    pub tolerance_cite: String,
    pub receptacle_words: Vec<String>,
    pub lighting_words: Vec<String>,
    pub motor_words: Vec<String>,
    pub kitchen_words: Vec<String>,
    pub cooking_words: Vec<String>,
    pub hvac_words: Vec<String>,
    pub spare_words: Vec<String>,
    pub feeder_words: Vec<String>,
    pub unit_words: Vec<String>,
    pub summary_titles: Vec<String>,
    /// A summary whose title has one of these is a feeder's (matched by its units), not the service's.
    pub feeder_summary_words: Vec<String>,
    pub service_size_words: Vec<String>,
    pub occupancies: Vec<Occupancy>,
    pub receptacle_steps: Vec<(Option<f64>, f64)>,
    pub receptacle_cite: String,
    pub kitchen_units: Vec<(u32, f64)>,
    pub kitchen_cite: String,
    pub largest_motor_factor: f64,
    pub largest_motor_cite: String,
}

pub(crate) fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

impl Rules {
    fn tolerance(&self, a: f64, b: f64) -> f64 {
        (self.tolerance_pct / 100.0 * a.abs().max(b.abs())).max(self.tolerance_va)
    }
    fn occupancy(&self, id: &str) -> Option<&Occupancy> {
        self.occupancies.iter().find(|o| o.id.eq_ignore_ascii_case(id) || o.groups.iter().any(|g| g.eq_ignore_ascii_case(id)))
    }
}

/// Upper-case words of a text (`EF-8 (FIRST FLOOR)` → `EF`, `8`, `FIRST`, `FLOOR`).
fn words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn has_word(ws: &[String], list: &[String]) -> bool {
    ws.iter().any(|w| list.iter().any(|l| l.eq_ignore_ascii_case(w)))
}

/// A stepped demand table applied to a load: `[[20000, 0.5], [100000, 0.4], [null, 0.3]]` (each
/// step's upper bound, cumulative).
fn stepped(load: f64, steps: &[(Option<f64>, f64)]) -> f64 {
    if steps.is_empty() {
        return load;
    }
    let (mut left, mut from, mut out) = (load.max(0.0), 0.0_f64, 0.0);
    for (to, f) in steps {
        if left <= 0.0 {
            break;
        }
        let span = to.map_or(left, |t| (t - from).max(0.0).min(left));
        out += span * f;
        left -= span;
        from = to.unwrap_or(from);
    }
    out + left
}

/// A table by count (`[[3, 0.9], [4, 0.8]]`): the last row whose count is at most `n`.
fn by_count(n: u32, rows: &[(u32, f64)]) -> Option<f64> {
    rows.iter().filter(|(k, _)| *k <= n).max_by_key(|(k, _)| *k).map(|(_, f)| *f)
}

// ---------------------------------------------------------------- the model's loads

/// What a load is, for its demand factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cat {
    Lighting,
    Receptacle,
    Kitchen,
    Motor,
    Equipment,
    Control,
    Spare,
    Unknown,
}

impl Cat {
    fn name(self) -> &'static str {
        match self {
            Cat::Lighting => "lighting",
            Cat::Receptacle => "receptacle",
            Cat::Kitchen => "kitchen",
            Cat::Motor => "motor",
            Cat::Equipment => "equipment",
            Cat::Control => "control",
            Cat::Spare => "spare",
            Cat::Unknown => "unknown",
        }
    }
    fn of(name: &str) -> Cat {
        match name {
            "lighting" => Cat::Lighting,
            "receptacle" => Cat::Receptacle,
            "kitchen" => Cat::Kitchen,
            "motor" => Cat::Motor,
            "equipment" => Cat::Equipment,
            "control" => Cat::Control,
            _ => Cat::Unknown,
        }
    }
}

/// One device of the model on a circuit, and its load.
#[derive(Clone, Debug)]
struct Dev {
    kind: String,
    room: String,
    va: Option<f64>,
    cat: Cat,
    /// Where its VA came from; documented (a schedule's or submittal's) or by rule.
    source: String,
    documented: bool,
}

/// A device's circuit number as its label gives it: `LP-5` → `5`, `Q-13,15` → `13`, `7` → `7`.
fn circuit_number(label: &str, panel: &str) -> String {
    let t = label.trim().to_ascii_uppercase();
    let p = panel.trim().to_ascii_uppercase();
    let rest = if !p.is_empty() && t.starts_with(&p) { t.get(p.len()..).unwrap_or("") } else { t.as_str() };
    let rest = rest.trim_start_matches(|c: char| !c.is_ascii_digit());
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().trim_start_matches('0').to_string()
}

/// A number in a property's text: `13 VA`, `13.0`, `1,200 W`.
fn first_number(t: &str) -> Option<f64> {
    let s: String = t.trim().chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    s.replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e9)
}

/// The model's devices on each panel's circuits: (panel, circuit number) → devices.
fn model_devices(d: &cadaddy_doc::Drawing, rules: &Rules) -> BTreeMap<(String, String), Vec<Dev>> {
    use cadaddy_doc::props;
    let mut out: BTreeMap<(String, String), Vec<Dev>> = BTreeMap::new();
    for (e, base) in super::elemprops::elements(d).take(MAX_ELEMENTS) {
        if !matches!(base.as_str(), "device" | "light_fixture" | "equipment") {
            continue;
        }
        let st = props::read(&e.common.xdata);
        let text = |k: &str| st.get(k).filter(|v| v.has_value()).map(|v| v.text().trim().to_string()).unwrap_or_default();
        let (panel, label) = (text("panel").to_ascii_uppercase(), text("circuit"));
        if panel.is_empty() || label.is_empty() {
            continue;
        }
        let number = circuit_number(&label, &panel);
        if number.is_empty() {
            continue;
        }
        let kind = super::elemprops::reported_kind(e, base.clone());
        let kind_l = kind.to_ascii_lowercase();
        let rule = rules.unit_loads.iter().find(|u| u.kinds.iter().any(|k| k.eq_ignore_ascii_case(&kind_l)));
        let doc_of = |k: &str| st.get(k).and_then(|p| p.source.doc.clone()).unwrap_or_default();
        // Its own rating first (the fixture schedule's VA, a nameplate the documents give it).
        let own = ["fixture_va", "load_va", "va", "wattage"].iter().find_map(|k| {
            let p = st.get(k).filter(|p| p.has_value())?;
            let v = p.value.as_f64().or_else(|| first_number(&p.text())).filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e9)?;
            Some((v, *k))
        });
        let cat = if base == "light_fixture" { Cat::Lighting } else { rule.map_or(Cat::Unknown, |r| Cat::of(&r.category)) };
        let dev = match own {
            Some((va, key)) => {
                let doc = doc_of(key);
                Dev {
                    kind: kind_l,
                    room: text("room"),
                    va: Some(va),
                    cat: if cat == Cat::Unknown { Cat::Equipment } else { cat },
                    source: if doc.is_empty() { format!("its {key}") } else { doc },
                    documented: true,
                }
            }
            None => match rule.and_then(|r| r.va.map(|v| (v, r))) {
                Some((va, r)) => Dev { kind: kind_l, room: text("room"), va: Some(va), cat, source: r.cite.clone(), documented: false },
                None => Dev { kind: kind_l, room: text("room"), va: None, cat, source: String::new(), documented: false },
            },
        };
        out.entry((panel, number)).or_default().push(dev);
    }
    out
}

// ---------------------------------------------------------------- the panels

/// A panel's electrical system as its header prints it: line volts and phases.
#[derive(Clone, Copy, Debug)]
struct System {
    volts: f64,
    to_neutral: f64,
    phases: u32,
}

/// `208Y/120V, 3PH, 4W` → 208 V, 120 V, 3; `120/240V, 1PH, 3W` → 240, 120, 1; `480V 3PH` → 480.
fn system(text: &str) -> System {
    let u = text.to_ascii_uppercase();
    let nums: Vec<f64> = u
        .split(|c: char| !c.is_ascii_digit())
        .filter(|w| (3..=3).contains(&w.len()))
        .filter_map(|w| w.parse::<f64>().ok())
        .filter(|v| (100.0..=999.0).contains(v))
        .collect();
    let phases = if u.contains("1PH") || u.contains("1 PH") || u.contains("1\u{d8}") || u.contains("SINGLE") { 1 } else { 3 };
    let (volts, to_neutral) = match (nums.first(), nums.get(1)) {
        (Some(a), Some(b)) => (a.max(*b), a.min(*b)),
        (Some(a), None) => (*a, a / SQRT3),
        _ => (208.0, 120.0),
    };
    // A split-phase 120/240 system's line is twice its neutral's.
    let to_neutral = if phases == 1 && (volts - 2.0 * to_neutral).abs() < 1.0 { to_neutral } else { to_neutral.min(volts) };
    System { volts, to_neutral, phases }
}

/// A rating as printed: `200A`, `2000A MCB` → 2000; `MLO` → none.
fn amps_of(t: &str) -> Option<f64> {
    let u = t.trim().to_ascii_uppercase();
    let n: String = u.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == ',').collect();
    n.replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v < 100_000.0)
}

/// A circuit's current at its load: one pole to neutral, two across a line, three balanced.
fn circuit_amps(va: f64, poles: u32, sys: System) -> f64 {
    match poles {
        0 | 1 => va / sys.to_neutral.max(1.0),
        2 => va / sys.volts.max(1.0),
        _ => va / (sys.volts * SQRT3).max(1.0),
    }
}

/// A panel's current at its demand.
fn panel_amps(va: f64, sys: System) -> f64 {
    if sys.phases >= 3 { va / (sys.volts * SQRT3).max(1.0) } else { va / sys.volts.max(1.0) }
}

/// What a schedule circuit's description says it is, when the model has no load on it.
fn described(c: &PanelCircuit, rules: &Rules) -> Cat {
    let ws = words(&c.description);
    if has_word(&ws, &rules.spare_words) {
        Cat::Spare
    } else if has_word(&ws, &rules.lighting_words) {
        Cat::Lighting
    } else if has_word(&ws, &rules.receptacle_words) {
        Cat::Receptacle
    } else if has_word(&ws, &rules.kitchen_words) {
        Cat::Kitchen
    } else if has_word(&ws, &rules.motor_words) {
        Cat::Motor
    } else {
        Cat::Equipment
    }
}

/// The panel a circuit feeds and how many of it (`PANELBOARD U (4 UNITS)` → U, 4): a feeder word
/// then a panel's tag, or a feeder record from this panel naming it.
fn feeds(c: &PanelCircuit, panel: &str, tags: &BTreeSet<String>, feeders: &[Feeder], rules: &Rules) -> Option<(String, u32)> {
    let ws = words(&c.description);
    let named = ws
        .windows(2)
        .find(|w| {
            w.first().is_some_and(|f| rules.feeder_words.iter().any(|x| x.eq_ignore_ascii_case(f))) && w.get(1).is_some_and(|t| tags.contains(t))
        })
        .and_then(|w| w.get(1).cloned())
        .or_else(|| {
            feeders
                .iter()
                .filter(|f| f.from.trim().eq_ignore_ascii_case(panel))
                .map(|f| f.to.trim().to_ascii_uppercase())
                .find(|t| tags.contains(t) && ws.contains(t))
        })
        .filter(|t| !t.eq_ignore_ascii_case(panel))?;
    let units = ws
        .windows(2)
        .find(|w| w.get(1).is_some_and(|u| rules.unit_words.iter().any(|x| x.eq_ignore_ascii_case(u))))
        .and_then(|w| w.first().and_then(|n| n.parse::<u32>().ok()))
        .filter(|n| (1..=MAX_UNITS).contains(n))
        .unwrap_or(1);
    Some((named, units))
}

/// The submittal's load of the equipment a circuit names (its tag or description), VA, and how.
fn equipment_load(c: &PanelCircuit, panel: &str, eqs: &[EquipmentElectrical]) -> Option<(f64, bool, String)> {
    let ws = words(&c.description);
    let e = eqs.iter().filter(|e| e.source_panel.trim().is_empty() || e.source_panel.trim().eq_ignore_ascii_case(panel)).find(|e| {
        let tag = words(&e.tag);
        (!tag.is_empty() && ws.windows(tag.len()).any(|w| w == tag.as_slice())) || (!e.description.trim().is_empty() && words(&e.description) == ws)
    })?;
    let volts = e.volts.filter(|v| *v > 0.0)?;
    let ph = if e.phase.unwrap_or(1) >= 3 { SQRT3 } else { 1.0 };
    let motor = e.hp.is_some() || e.fla.is_some();
    let src = if e.source.trim().is_empty() { "its submittal".to_string() } else { e.source.clone() };
    if let Some(kw) = e.kw.filter(|k| *k > 0.0) {
        return Some((kw * 1000.0, motor, format!("{}: {kw} kW ({src})", e.tag)));
    }
    let (a, what) = e
        .fla
        .map(|a| (a, "full-load"))
        .or_else(|| e.nameplate_a.map(|a| (a, "nameplate")))
        .or_else(|| e.mca.map(|a| (a, "minimum circuit (with 125% of its largest motor)")))?;
    Some((
        volts * a * ph,
        motor || e.mca.is_some(),
        format!("{}: {volts:.0} V x {a} A {what}{} ({src})", e.tag, if ph > 1.0 { " x 1.732" } else { "" }),
    ))
}

/// One circuit, calculated.
#[derive(Clone, Debug, Default)]
struct Circuit {
    number: String,
    description: String,
    breaker: Option<f64>,
    poles: u32,
    schedule_va: Option<f64>,
    model_va: Option<f64>,
    /// The load used up the tree and where it came from: `model`, `schedule`, `feeder`,
    /// `submittal`, `none`.
    used_va: f64,
    used_from: &'static str,
    devices: usize,
    unknown: usize,
    documented: bool,
    rule_receptacles: usize,
    /// Devices of a kind its description doesn't name (receptacles on a lighting circuit).
    foreign: usize,
    foreign_what: String,
    units: usize,
    cats: BTreeMap<&'static str, f64>,
    continuous_va: f64,
    largest_motor_va: f64,
    kitchen_units: u32,
    /// A unit room's (its demand by the occupancy's unit rules).
    unit: bool,
    sources: Vec<String>,
    feeds: Option<(String, u32)>,
}

/// One panel, its circuits and its subtree's loads.
#[derive(Clone, Debug, Default)]
struct Panel {
    tag: String,
    typical_of: Option<u32>,
    sheet: String,
    sys_text: String,
    rating_a: Option<f64>,
    rating_from: String,
    circuits: Vec<Circuit>,
    printed: BTreeMap<String, f64>,
    units: usize,
}

/// The loads under a panel (its circuits and the panels they feed, with their units).
#[derive(Clone, Debug, Default)]
struct Tree {
    connected_va: f64,
    /// Category → (unit rooms', others') VA.
    cats: BTreeMap<&'static str, (f64, f64)>,
    continuous_va: f64,
    largest_motor_va: f64,
    kitchen_units: u32,
    units: u32,
    /// The unit rooms' appliance circuits (fixed appliances: 220.53 counts them per unit).
    appliances: u32,
    /// Units with cooking, and with heating or cooling, in them; the unit rooms' continuous load
    /// and their largest motor.
    cooking_units: u32,
    hvac_units: u32,
    continuous_unit_va: f64,
    largest_unit_motor_va: f64,
}

impl Tree {
    fn add(&mut self, o: &Tree, n: u32) {
        let k = f64::from(n);
        self.connected_va += o.connected_va * k;
        for (c, (u, x)) in &o.cats {
            let e = self.cats.entry(c).or_default();
            e.0 += u * k;
            e.1 += x * k;
        }
        self.continuous_va += o.continuous_va * k;
        self.largest_motor_va = self.largest_motor_va.max(o.largest_motor_va);
        self.kitchen_units = self.kitchen_units.saturating_add(o.kitchen_units.saturating_mul(n));
        self.units = self.units.saturating_add(o.units.saturating_mul(n));
        self.appliances = self.appliances.saturating_add(o.appliances.saturating_mul(n));
        self.cooking_units = self.cooking_units.saturating_add(o.cooking_units.saturating_mul(n));
        self.hvac_units = self.hvac_units.saturating_add(o.hvac_units.saturating_mul(n));
        self.continuous_unit_va += o.continuous_unit_va * k;
        self.largest_unit_motor_va = self.largest_unit_motor_va.max(o.largest_unit_motor_va);
    }
}

/// One demand step: what it is, its VA in and out, the cite.
#[derive(Clone, Debug)]
struct Step {
    what: String,
    connected: f64,
    demand: f64,
    cite: String,
}

/// How a tree's demand was taken: its units as dwelling units (cooking in them), and the optional
/// method's factor when it applies.
#[derive(Clone, Copy, Debug, Default)]
struct Method {
    dwelling: bool,
    optional: Option<f64>,
}

/// A tree's demand by the rules for an occupancy.
fn demand(t: &Tree, occ: &Occupancy, commercial: &Occupancy, rules: &Rules) -> (f64, Vec<Step>, Method) {
    let mut steps = Vec::new();
    let get = |c: &str| t.cats.get(c).copied().unwrap_or_default();
    // Units with cooking, where the occupancy wires them as dwelling units: the dwelling rules.
    let as_dwelling =
        (!occ.cooking_units_as.is_empty() && t.units > 0 && t.cooking_units >= t.units).then(|| rules.occupancy(&occ.cooking_units_as)).flatten();
    let u = as_dwelling.unwrap_or(occ);
    let note = if as_dwelling.is_some() { format!("; {}", occ.cooking_cite) } else { String::new() };
    // The optional method: three or more units, each with cooking and heating or cooling.
    let unit_total: f64 = t.cats.values().map(|(x, _)| x).sum();
    let optional = (!u.optional_units.is_empty() && t.units >= 3 && t.cooking_units >= t.units && t.hvac_units >= t.units && unit_total > 0.0)
        .then(|| by_count(t.units, &u.optional_units))
        .flatten();
    let method = Method { dwelling: as_dwelling.is_some(), optional };
    let (lu, lo) = get("lighting");
    let (ru, ro) = get("receptacle");
    let (ku, ko) = get("kitchen");
    let (eu, eo) = get("equipment");
    let (mu, mo) = get("motor");
    let (uu, uo) = get("unknown");
    let mut unit_continuous = t.continuous_unit_va;
    if let Some(f) = optional {
        steps.push(Step {
            what: format!("{} units by the optional method ({:.0}%)", t.units, f * 100.0),
            connected: unit_total,
            demand: unit_total * f,
            cite: format!("{}{note}", u.optional_cite),
        });
        unit_continuous = 0.0;
    } else {
        // The unit rooms' general lighting (their general-use receptacles in it).
        let unit_general = lu + if u.unit_receptacles_in_lighting { ru } else { 0.0 };
        if unit_general > 0.0 {
            let mut cite = format!("{}{note}", u.lighting_cite);
            if u.unit_receptacles_in_lighting && ru > 0.0 {
                cite = format!("{cite}; {}", u.unit_receptacles_cite);
            }
            steps.push(Step {
                what: format!("general lighting, {}", u.name),
                connected: unit_general,
                demand: stepped(unit_general, &u.lighting_steps),
                cite,
            });
        }
        if !u.unit_receptacles_in_lighting && ru > 0.0 {
            steps.push(Step {
                what: "unit rooms' receptacles".into(),
                connected: ru,
                demand: stepped(ru, &rules.receptacle_steps),
                cite: rules.receptacle_cite.clone(),
            });
        }
        // The unit rooms' fixed appliances (dwelling rule: four or more at 75%).
        let appliances = ku + eu;
        if appliances > 0.0 {
            let four = t.units > 0 && u.fixed_appliances_min.is_some_and(|m| t.appliances >= m.saturating_mul(t.units));
            let f = u.fixed_appliances_factor.filter(|_| four).unwrap_or(1.0);
            let cite = if f < 1.0 { u.fixed_appliances_cite.clone() } else { "NEC 220.14(A): the appliances' ratings at 100%".to_string() };
            steps.push(Step { what: "unit rooms' appliances".into(), connected: appliances, demand: appliances * f, cite });
        }
        if mu > 0.0 {
            steps.push(Step {
                what: "unit rooms' motors and HVAC".into(),
                connected: mu,
                demand: mu,
                cite: "NEC 220.50, 430.24: at their ratings".into(),
            });
        }
        if uu > 0.0 {
            steps.push(Step {
                what: "unit rooms' loads the schedule gives".into(),
                connected: uu,
                demand: uu,
                cite: "the panel schedule's VA, at 100%".into(),
            });
        }
    }
    // The other rooms (house loads).
    if lo > 0.0 {
        steps.push(Step {
            what: "lighting, other areas".into(),
            connected: lo,
            demand: stepped(lo, &commercial.lighting_steps),
            cite: commercial.lighting_cite.clone(),
        });
    }
    if ro > 0.0 {
        steps.push(Step {
            what: "receptacles".into(),
            connected: ro,
            demand: stepped(ro, &rules.receptacle_steps),
            cite: rules.receptacle_cite.clone(),
        });
    }
    if ko > 0.0 {
        let f = by_count(t.kitchen_units, &rules.kitchen_units).unwrap_or(1.0);
        steps.push(Step {
            what: format!("kitchen equipment ({} units)", t.kitchen_units),
            connected: ko,
            demand: ko * f,
            cite: rules.kitchen_cite.clone(),
        });
    }
    if mo > 0.0 {
        steps.push(Step { what: "motors and HVAC".into(), connected: mo, demand: mo, cite: "NEC 220.50, 430.24: at their ratings".into() });
    }
    if eo > 0.0 {
        steps.push(Step { what: "other equipment".into(), connected: eo, demand: eo, cite: "NEC 220.14(A): at its rating".into() });
    }
    if uo > 0.0 {
        steps.push(Step {
            what: "loads the schedule gives (no model load)".into(),
            connected: uo,
            demand: uo,
            cite: "the panel schedule's VA, at 100%".into(),
        });
    }
    // Continuous loads at 125% (their share of the demand; the optional method's units have
    // their own factor), and the largest motor's extra 25%.
    let base: f64 = steps.iter().map(|s| s.demand).sum();
    let counted = if optional.is_some() { (t.connected_va - unit_total).max(0.0) } else { t.connected_va };
    let cont = (t.continuous_va - t.continuous_unit_va + unit_continuous).clamp(0.0, counted.max(0.0));
    let share = if counted > 0.0 { cont / counted } else { 0.0 };
    let opt_part = steps.first().filter(|_| optional.is_some()).map_or(0.0, |s| s.demand);
    let extra = (base - opt_part) * share * (rules.continuous.factor - 1.0).max(0.0);
    if extra > 0.0 {
        steps.push(Step { what: "continuous loads' extra 25%".into(), connected: cont, demand: extra, cite: rules.continuous.cite.clone() });
    }
    let motor = if optional.is_some() { t.largest_motor_va } else { t.largest_motor_va.max(t.largest_unit_motor_va) };
    if motor > 0.0 {
        steps.push(Step {
            what: "largest motor's extra 25%".into(),
            connected: motor,
            demand: motor * rules.largest_motor_factor,
            cite: rules.largest_motor_cite.clone(),
        });
    }
    (steps.iter().map(|s| s.demand).sum(), steps, method)
}

// ---------------------------------------------------------------- findings

#[derive(Clone, Debug)]
struct Finding {
    rule: &'static str,
    level: &'static str,
    panel: String,
    circuit: String,
    message: String,
    /// Which side is likely wrong: `schedule`, `model`, `documents`, `design`, `either`.
    likely: &'static str,
    cite: String,
}

impl Finding {
    fn json(&self) -> Value {
        json!({"rule": self.rule, "level": self.level, "panel": self.panel, "circuit": self.circuit, "message": self.message, "likely_wrong": self.likely, "cite": self.cite})
    }
}

fn r0(v: f64) -> f64 {
    v.round()
}
fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
fn va(v: f64) -> String {
    let n = v.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The phases a circuit's poles land on (standard numbering: rows of two, phases in turn).
fn phases_of(number: &str, poles: u32, phases: u32) -> Vec<usize> {
    let Ok(n) = number.parse::<u32>() else { return Vec::new() };
    if n == 0 || phases == 0 {
        return Vec::new();
    }
    let row = (n - 1) / 2;
    (0..poles.clamp(1, 3)).map(|k| ((row + k) % phases.min(3)) as usize).collect()
}

/// Imbalance: the largest phase's departure from their average, percent of the average.
fn imbalance(ph: &[f64]) -> Option<f64> {
    if ph.len() < 2 {
        return None;
    }
    let avg = ph.iter().sum::<f64>() / ph.len() as f64;
    (avg > 0.0).then(|| ph.iter().map(|p| (p - avg).abs()).fold(0.0, f64::max) / avg * 100.0)
}

/// The conductors' ampacity of a feeder: sets × the 75 C table's (coderules.json E-EQUIP-ELEC).
fn feeder_ampacity(f: &Feeder, table: &BTreeMap<String, BTreeMap<String, f64>>) -> Option<f64> {
    let size = f.size.trim().trim_start_matches('#').trim_end_matches("kcmil").trim_end_matches("KCMIL").trim().to_string();
    let a = table.get(&f.material.trim().to_ascii_lowercase()).and_then(|t| t.get(&size)).copied()?;
    Some(a * f64::from(f.sets.max(1)))
}

// ---------------------------------------------------------------- the calculation

/// The occupancy the rules use: given, else the code sheet's (a unit occupancy first), else
/// commercial.
fn pick_occupancy<'a>(rules: &'a Rules, given: Option<&str>, groups: &[(String, String)]) -> Option<(&'a Occupancy, String)> {
    if let Some(g) = given.map(str::trim).filter(|g| !g.is_empty()) {
        return rules.occupancy(g).map(|o| (o, "given".to_string()));
    }
    let found: Vec<(&Occupancy, &String)> = groups.iter().filter_map(|(g, src)| rules.occupancy(g).map(|o| (o, src))).collect();
    if let Some((o, src)) = found.iter().find(|(o, _)| o.unit_rooms).or(found.first()) {
        return Some((o, format!("the code sheet: {src}")));
    }
    rules.occupancy("commercial").map(|o| (o, "no occupancy read off a code sheet: the commercial rules".to_string()))
}

struct Calc {
    rules: Rules,
    occ: Occupancy,
    occ_from: String,
    commercial: Occupancy,
    panels: Vec<Panel>,
    by_tag: BTreeMap<String, usize>,
    feeders: Vec<Feeder>,
    summaries: Vec<LoadSummary>,
    ampacity: BTreeMap<String, BTreeMap<String, f64>>,
}

/// The circuits of one schedule, with the model's devices on them.
fn circuits_of(
    sch: &PanelSchedule,
    devs: &BTreeMap<(String, String), Vec<Dev>>,
    tags: &BTreeSet<String>,
    sp: &BuildingSpec,
    rules: &Rules,
    unit_rooms: &dyn Fn(&str) -> bool,
) -> (Vec<Circuit>, usize) {
    let key = sch.panel.trim().to_ascii_uppercase();
    // A typical panel's units: the rooms its devices are in.
    let rooms: BTreeSet<String> =
        devs.iter().filter(|((p, _), _)| *p == key).flat_map(|(_, v)| v.iter().map(|d| d.room.clone())).filter(|r| !r.is_empty()).collect();
    let typical = sch.typical_of.is_some();
    let mut out = Vec::new();
    for c in sch.circuits.iter().take(MAX_CIRCUITS) {
        let num = c.number.trim().trim_start_matches('0').to_string();
        let mut cc = Circuit {
            number: c.number.clone(),
            description: c.description.clone(),
            breaker: c.breaker.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0),
            poles: c.poles.max(1),
            schedule_va: c.load_va.filter(|v| v.is_finite() && *v >= 0.0),
            used_from: "none",
            ..Circuit::default()
        };
        let ws = words(&c.description);
        let cont_words = has_word(&ws, &rules.continuous.description_words);
        if let Some((child, n)) = feeds(c, &key, tags, &sp.mep.feeders, rules) {
            cc.feeds = Some((child, n));
            cc.used_from = "feeder";
            out.push(cc);
            continue;
        }
        let on = devs.get(&(key.clone(), num)).cloned().unwrap_or_default();
        let desc = described(c, rules);
        let (has_l, has_r) = (has_word(&ws, &rules.lighting_words), has_word(&ws, &rules.receptacle_words));
        if !on.is_empty() {
            // A typical panel's circuit: one unit's devices (the rooms that have it).
            let units = if typical { on.iter().map(|d| d.room.clone()).filter(|r| !r.is_empty()).collect::<BTreeSet<_>>().len().max(1) } else { 1 };
            let per = 1.0 / units as f64;
            cc.units = units;
            cc.devices = on.len();
            cc.unit = typical || on.iter().filter(|d| unit_rooms(&d.room)).count() * 2 > on.len();
            // A unit room's kitchen counter circuit: a small-appliance branch circuit (its VA by rule).
            let small =
                cc.unit && !rules.small_appliance.words.is_empty() && has_word(&ws, &rules.small_appliance.words) && rules.small_appliance.va > 0.0;
            // A circuit naming a piece of equipment (no lighting or receptacle words): its
            // receptacles and connections are that equipment's; its load the equipment's.
            let names_equipment = !small && matches!(desc, Cat::Kitchen | Cat::Motor | Cat::Equipment) && !has_l && !has_r;
            let connection = |d: &Dev| {
                matches!(d.cat, Cat::Receptacle | Cat::Equipment | Cat::Unknown | Cat::Motor | Cat::Kitchen)
                    && !(d.documented && d.cat == Cat::Lighting)
            };
            let replaced = small || names_equipment;
            let mut sum = 0.0;
            let mut srcs: BTreeMap<String, (usize, f64)> = BTreeMap::new();
            for d in on.iter().filter(|d| !(replaced && connection(d))) {
                let Some(v) = d.va else { continue };
                sum += v * per;
                *cc.cats.entry(d.cat.name()).or_default() += v * per;
                if rules.continuous.categories.iter().any(|k| k == d.cat.name()) || cont_words {
                    cc.continuous_va += v * per;
                }
                let e = srcs.entry(format!("{} ({})", d.kind, d.source)).or_default();
                e.0 += 1;
                e.1 += v;
            }
            let rest: Vec<&Dev> = on.iter().filter(|d| !(replaced && connection(d))).collect();
            cc.unknown = rest.iter().filter(|d| d.va.is_none() && d.cat != Cat::Control).count();
            cc.rule_receptacles = rest.iter().filter(|d| d.cat == Cat::Receptacle && !d.documented).count();
            cc.documented = rest.iter().filter(|d| d.cat != Cat::Control).all(|d| d.documented);
            // Devices the description doesn't name: receptacles on a lighting circuit, luminaires
            // on a receptacle circuit.
            let recs = on.iter().filter(|d| d.cat == Cat::Receptacle).count();
            let lums = on.iter().filter(|d| d.cat == Cat::Lighting).count();
            if desc == Cat::Lighting && !has_r && recs > 0 {
                cc.foreign = recs;
                cc.foreign_what = format!("the model puts {recs} receptacle(s) on this lighting circuit");
            } else if desc == Cat::Receptacle && !has_l && lums > 0 {
                cc.foreign = lums;
                cc.foreign_what = format!("the model puts {lums} luminaire(s) on this receptacle circuit");
            }
            let mut extra: Option<(f64, String, bool)> = None;
            if small {
                extra = Some((rules.small_appliance.va, rules.small_appliance.cite.clone(), true));
                *cc.cats.entry("receptacle").or_default() += rules.small_appliance.va;
            } else if names_equipment {
                let from = equipment_load(c, &key, &sp.mep.equipment_electrical).map(|(v, m, src)| (v, src, m)).or_else(|| {
                    cc.schedule_va.filter(|v| *v > 0.0).map(|v| {
                        (
                            v,
                            format!(
                                "the panel schedule's {} VA for the {} it names ({})",
                                va(v),
                                c.description.trim(),
                                if sch.source.is_empty() { "schedule" } else { sch.source.as_str() }
                            ),
                            desc == Cat::Motor,
                        )
                    })
                });
                if let Some((v, src, motor)) = from {
                    let cat = if motor {
                        "motor"
                    } else if desc == Cat::Kitchen {
                        "kitchen"
                    } else {
                        "equipment"
                    };
                    *cc.cats.entry(cat).or_default() += v;
                    if motor {
                        cc.largest_motor_va = v;
                    }
                    if cont_words {
                        cc.continuous_va += v;
                    }
                    extra = Some((v, src, true));
                    cc.documented = cc.documented || rest.iter().all(|d| d.cat == Cat::Control);
                } else {
                    // Nothing documents the equipment: its connections' rule values.
                    for d in on.iter().filter(|d| connection(d)) {
                        let Some(v) = d.va else { continue };
                        sum += v * per;
                        *cc.cats.entry(d.cat.name()).or_default() += v * per;
                        let e = srcs.entry(format!("{} ({})", d.kind, d.source)).or_default();
                        e.0 += 1;
                        e.1 += v;
                    }
                    cc.unknown += on.iter().filter(|d| connection(d) && d.va.is_none()).count();
                }
            }
            let has_load = extra.is_some() || on.iter().any(|d| d.va.is_some_and(|v| v > 0.0));
            if has_load {
                let total = sum + extra.as_ref().map_or(0.0, |e| e.0);
                cc.model_va = Some(total);
                cc.used_va = total;
                cc.used_from = "model";
                if let Some((v, src, _)) = &extra {
                    cc.sources.push(format!("{} VA: {src}", va(*v)));
                }
                for (s, (n, v)) in srcs.iter().take(8) {
                    let n = if typical { format!("{:.1}", *n as f64 / units as f64) } else { n.to_string() };
                    cc.sources.push(format!("{n} x {s} = {} VA", va(v / units as f64)));
                }
                if typical {
                    cc.sources.push(format!("one unit's: the devices over the {units} rooms that have them"));
                }
                if cc.unknown > 0 {
                    cc.sources.push(format!("{} device(s) with no load data", cc.unknown));
                }
            }
        }
        if cc.used_from == "none" {
            if let Some((v, motor, src)) = equipment_load(c, &key, &sp.mep.equipment_electrical) {
                cc.model_va = Some(v);
                cc.used_va = v;
                cc.used_from = "submittal";
                cc.documented = true;
                let cat = if motor { "motor" } else { "equipment" };
                *cc.cats.entry(cat).or_default() += v;
                if motor {
                    cc.largest_motor_va = v;
                }
                cc.sources.push(src);
            } else if let Some(v) = cc.schedule_va {
                cc.used_va = v;
                cc.used_from = "schedule";
                let cat = described(c, rules);
                let name = match cat {
                    Cat::Spare => "spare",
                    Cat::Lighting => "lighting",
                    Cat::Receptacle => "receptacle",
                    Cat::Kitchen => "kitchen",
                    Cat::Motor => "motor",
                    _ => "unknown",
                };
                if cat != Cat::Spare {
                    *cc.cats.entry(name).or_default() += v;
                }
                if cat == Cat::Motor {
                    cc.largest_motor_va = v;
                }
                if cat == Cat::Lighting || cont_words {
                    cc.continuous_va = v;
                }
                cc.sources.push(format!(
                    "the panel schedule's {} VA ({}): no load in the model on it",
                    va(v),
                    if sch.source.is_empty() { "schedule" } else { sch.source.as_str() }
                ));
            }
            cc.unit = typical;
        }
        if cc.cats.contains_key("kitchen") && !cc.unit {
            cc.kitchen_units = 1;
        }
        out.push(cc);
    }
    (out, if typical { rooms.len() } else { 0 })
}

impl Calc {
    fn new(
        sp: &BuildingSpec,
        devs: &BTreeMap<(String, String), Vec<Dev>>,
        rules: Rules,
        occ: Occupancy,
        occ_from: String,
        unit_rooms: &dyn Fn(&str) -> bool,
    ) -> Calc {
        let commercial = rules.occupancy("commercial").cloned().unwrap_or_default();
        let tags: BTreeSet<String> = sp.mep.panel_schedules.iter().map(|p| p.panel.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()).collect();
        let mut panels = Vec::new();
        let mut by_tag = BTreeMap::new();
        for sch in sp.mep.panel_schedules.iter().take(MAX_PANELS) {
            let tag = sch.panel.trim().to_ascii_uppercase();
            if tag.is_empty() || by_tag.contains_key(&tag) {
                continue;
            }
            let (circuits, units) = circuits_of(sch, devs, &tags, sp, &rules, unit_rooms);
            let h = |k: &str| sch.header.get(k).cloned().unwrap_or_default();
            let main = amps_of(&h("main size / type")).filter(|_| !h("main size / type").to_ascii_uppercase().starts_with("MLO"));
            let bus = amps_of(&h("bus amps"));
            let (rating_a, rating_from) = match (main, bus) {
                (Some(m), _) => (Some(m), format!("main {}", h("main size / type"))),
                (None, Some(b)) => (Some(b), format!("bus {}", h("bus amps"))),
                _ => (None, String::new()),
            };
            by_tag.insert(tag.clone(), panels.len());
            panels.push(Panel {
                tag,
                typical_of: sch.typical_of,
                sheet: sch.source.clone(),
                sys_text: h("volts/phase"),
                rating_a,
                rating_from,
                circuits,
                printed: sch.totals.clone(),
                units,
            });
        }
        let ampacity = super::coderules::rules().into_iter().find(|r| !r.ampacity_75c.is_empty()).map(|r| r.ampacity_75c).unwrap_or_default();
        Calc { rules, occ, occ_from, commercial, panels, by_tag, feeders: sp.mep.feeders.clone(), summaries: sp.mep.load_summaries.clone(), ampacity }
    }

    fn panel(&self, tag: &str) -> Option<&Panel> {
        self.by_tag.get(&tag.trim().to_ascii_uppercase()).and_then(|i| self.panels.get(*i))
    }

    /// The loads under a panel, its feeders' panels included (each once per unit it feeds).
    fn tree(&self, tag: &str, depth: usize, seen: &mut Vec<String>) -> Tree {
        let mut t = Tree::default();
        let Some(p) = self.panel(tag) else { return t };
        if depth > MAX_DEPTH || seen.iter().any(|s| s == &p.tag) {
            return t;
        }
        seen.push(p.tag.clone());
        if p.typical_of.is_some() {
            t.units = 1;
            let any = |list: &[String]| p.circuits.iter().any(|c| c.feeds.is_none() && has_word(&words(&c.description), list));
            t.cooking_units = u32::from(any(&self.rules.cooking_words));
            t.hvac_units = u32::from(any(&self.rules.hvac_words));
        }
        for c in &p.circuits {
            if let Some((child, n)) = &c.feeds {
                let sub = self.tree(child, depth + 1, seen);
                t.add(&sub, *n);
                continue;
            }
            t.connected_va += c.used_va;
            for (k, v) in &c.cats {
                let e = t.cats.entry(k).or_default();
                if c.unit { e.0 += v } else { e.1 += v }
            }
            t.continuous_va += c.continuous_va;
            if c.unit {
                t.continuous_unit_va += c.continuous_va;
                t.largest_unit_motor_va = t.largest_unit_motor_va.max(c.largest_motor_va);
            } else {
                t.largest_motor_va = t.largest_motor_va.max(c.largest_motor_va);
            }
            t.kitchen_units = t.kitchen_units.saturating_add(c.kitchen_units);
            if c.unit && (c.cats.contains_key("kitchen") || c.cats.contains_key("equipment")) {
                t.appliances = t.appliances.saturating_add(1);
            }
        }
        seen.pop();
        t
    }

    /// The model's load of a circuit up the tree (a feeder's: its panel's connected × units).
    fn circuit_va(&self, c: &Circuit) -> f64 {
        match &c.feeds {
            Some((child, n)) => self.tree(child, 1, &mut Vec::new()).connected_va * f64::from(*n),
            None => c.used_va,
        }
    }

    /// Who feeds a panel: (panel, circuit index).
    fn fed_by(&self, tag: &str) -> Option<(usize, usize)> {
        self.panels
            .iter()
            .enumerate()
            .find_map(|(pi, p)| p.circuits.iter().position(|c| c.feeds.as_ref().is_some_and(|(t, _)| t.eq_ignore_ascii_case(tag))).map(|ci| (pi, ci)))
    }

    /// The service: the panel nothing feeds with the most under it.
    fn service(&self) -> Option<&Panel> {
        self.panels
            .iter()
            .filter(|p| self.fed_by(&p.tag).is_none() && p.typical_of.is_none())
            .max_by(|a, b| self.tree(&a.tag, 0, &mut Vec::new()).connected_va.total_cmp(&self.tree(&b.tag, 0, &mut Vec::new()).connected_va))
    }
}

/// The summary lines' facts: (service size A, demand VA, demand A, connected VA) as printed (a
/// line in kVA read as VA).
fn summary_facts(s: &LoadSummary, rules: &Rules) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let up = |l: &str| l.to_ascii_uppercase();
    let service = s
        .lines
        .iter()
        .find(|l| rules.service_size_words.iter().any(|w| up(&l.label).contains(&w.to_ascii_uppercase())))
        .and_then(|l| l.values.last().copied());
    let is_amps = |l: &cadaddy_walk3d::LoadSummaryLine| up(&l.label).contains("AMP");
    let scale = |l: &cadaddy_walk3d::LoadSummaryLine| if up(&l.label).contains("KVA") || up(&l.text).contains("KVA") { 1000.0 } else { 1.0 };
    let totals: Vec<&cadaddy_walk3d::LoadSummaryLine> = s.lines.iter().filter(|l| up(&l.label).contains("TOTAL") && !l.values.is_empty()).collect();
    let demand_va = s
        .lines
        .iter()
        .rev()
        .find(|l| up(&l.label).contains("DEMAND") && !is_amps(l) && !l.values.is_empty())
        .or_else(|| totals.iter().rev().find(|l| !is_amps(l)).copied())
        .and_then(|l| l.values.last().map(|v| v * scale(l)));
    let demand_a = s
        .lines
        .iter()
        .rev()
        .find(|l| (up(&l.label).contains("TOTAL") || up(&l.label).contains("DEMAND")) && is_amps(l) && !l.values.is_empty())
        .and_then(|l| l.values.last().copied());
    let connected_va = totals.iter().find(|l| !is_amps(l) && l.values.len() >= 2).and_then(|l| l.values.first().map(|v| v * scale(l)));
    (service, demand_va, demand_a, connected_va)
}

/// Is a summary one feeder's (`UNIT FEEDER DEMAND CALCULATION`)?
fn feeder_summary(s: &LoadSummary, rules: &Rules) -> bool {
    s.panel.is_empty() && has_word(&words(&s.title), &rules.feeder_summary_words)
}

/// The units a summary is for: `FOR 4 SUITES` in a label, or a line led by a unit word
/// (`12 UNITS WITH A TOTAL SF OF: 9000`) and its first number.
fn summary_units(s: &LoadSummary, rules: &Rules) -> Option<u32> {
    let unit = |w: &str| rules.unit_words.iter().any(|u| u.eq_ignore_ascii_case(w));
    for l in &s.lines {
        let ws = words(&l.label);
        if let Some(n) = ws.windows(2).find(|w| w.get(1).is_some_and(|x| unit(x))).and_then(|w| w.first()?.parse::<u32>().ok()) {
            return Some(n).filter(|n| (1..=MAX_UNITS).contains(n));
        }
        if ws.first().is_some_and(|w| unit(w))
            && let Some(v) = l.values.first().filter(|v| v.fract() == 0.0 && **v >= 1.0 && **v <= f64::from(MAX_UNITS))
        {
            return Some(*v as u32);
        }
    }
    None
}

/// The calculation and its checks: the report as JSON, CSV and text.
fn calculate(c: &Calc) -> (Value, String, String) {
    let rules = &c.rules;
    let mut findings: Vec<Finding> = Vec::new();
    let mut csv = String::from(
        "level,panel,circuit,description,breaker_a,poles,schedule_va,model_va,used_va,used_from,category,continuous_va,amps,rating_a,percent_of_rating,demand_va,demand_a,findings,sources\n",
    );
    let cell = |v: &str| if v.contains([',', '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() };
    let opt = |v: Option<f64>| v.map(|x| format!("{:.0}", x)).unwrap_or_default();
    let mut text = String::new();
    text.push_str(&format!("LOAD CALCULATION (model) - occupancy: {} [{}] ({})\n", c.occ.name, c.occ.groups.join(", "), c.occ_from));
    text.push_str("Connected loads: luminaires at the fixture schedule's VA, receptacles by rule, equipment from its submittal, else the panel schedule's VA (said so on the line).\n\n");
    let mut panels_json = Vec::new();
    let service = c.service().map(|p| p.tag.clone());
    // Feeders first in the report: the service, then down its tree; then panels nothing feeds.
    let mut order: Vec<String> = Vec::new();
    fn walk(c: &Calc, tag: &str, order: &mut Vec<String>, depth: usize) {
        if depth > MAX_DEPTH || order.iter().any(|t| t == tag) {
            return;
        }
        order.push(tag.to_string());
        if let Some(p) = c.panel(tag) {
            for cc in &p.circuits {
                if let Some((child, _)) = &cc.feeds {
                    walk(c, child, order, depth + 1);
                }
            }
        }
    }
    if let Some(s) = &service {
        walk(c, s, &mut order, 0);
    }
    for p in &c.panels {
        if !order.contains(&p.tag) {
            walk(c, &p.tag, &mut order, 0);
        }
    }
    let mut service_json = Value::Null;
    // Feeder summaries compared once per feeder kind (its panel and units).
    let mut feeder_seen: BTreeSet<(String, u32)> = BTreeSet::new();
    for tag in &order {
        let Some(p) = c.panel(tag) else { continue };
        let sys = system(&p.sys_text);
        let tree = c.tree(&p.tag, 0, &mut Vec::new());
        let (dem, steps, method) = demand(&tree, &c.occ, &c.commercial, rules);
        let dem_a = panel_amps(dem, sys);
        let is_service = service.as_deref() == Some(p.tag.as_str());
        let level = if is_service { "service" } else { "panel" };
        let fed = c.fed_by(&p.tag);
        let fed_text = fed.and_then(|(pi, ci)| {
            let up = c.panels.get(pi)?;
            let cc = up.circuits.get(ci)?;
            Some(format!("fed from {} circuit {} ({} A)", up.tag, cc.number, cc.breaker.map_or("?".into(), |b| format!("{b:.0}"))))
        });
        text.push_str(&format!(
            "{} {}{} ({}) - {}{}{}\n",
            if is_service { "SERVICE / PANEL" } else { "PANEL" },
            p.tag,
            p.typical_of.map(|n| format!(" (typical of {n}: one unit's loads, over {} rooms in the model)", p.units)).unwrap_or_default(),
            if p.sheet.is_empty() { "schedule" } else { p.sheet.as_str() },
            if p.sys_text.is_empty() { "208Y/120V 3PH assumed".to_string() } else { p.sys_text.clone() },
            p.rating_a.map(|r| format!(", rated {r:.0} A ({})", p.rating_from)).unwrap_or_default(),
            fed_text.map(|f| format!(", {f}")).unwrap_or_default()
        ));
        text.push_str("  ckt  description                        breaker  schedule VA  model VA  used VA  from       source\n");
        let mut sched_sum = 0.0;
        let mut model_sum = 0.0;
        let mut phase = [0.0_f64; 3];
        for cc in &p.circuits {
            let used = c.circuit_va(cc);
            model_sum += used;
            sched_sum += cc.schedule_va.unwrap_or(0.0);
            let ph = phases_of(&cc.number, cc.poles, sys.phases.clamp(2, 3));
            for k in &ph {
                if let Some(slot) = phase.get_mut(*k) {
                    *slot += used / ph.len() as f64;
                }
            }
            let mut notes: Vec<String> = Vec::new();
            // E-LOAD-CIRCUIT: continuous at 125% plus the rest against the breaker.
            if let (Some(b), None) = (cc.breaker, &cc.feeds) {
                let cont = cc.continuous_va.min(used);
                let need = circuit_amps(cont * rules.continuous.factor + (used - cont), cc.poles, sys);
                let load_a = circuit_amps(used, cc.poles, sys);
                let pct = circuit_amps(cont, cc.poles, sys) / b * 100.0;
                let side = if cc.used_from == "model" || cc.used_from == "submittal" { "the model's" } else { "the schedule's" };
                // The schedule's own load over the limit too: the design; else the model's
                // devices the circuit doesn't name, else either.
                let sched_over = cc.schedule_va.is_some_and(|sv| {
                    let sc = if cc.continuous_va > 0.0 { sv } else { 0.0 };
                    circuit_amps(sc * rules.continuous.factor + (sv - sc), cc.poles, sys) > b + 1e-6
                });
                let (likely, why) = if sched_over || side == "the schedule's" {
                    ("design", String::new())
                } else if cc.foreign > 0 {
                    ("model", format!("; {}", cc.foreign_what))
                } else {
                    ("either", cc.schedule_va.map_or(String::new(), |sv| format!("; the schedule gives {} VA", va(sv))))
                };
                if load_a > b + 1e-6 {
                    notes.push(format!("over its breaker: {load_a:.1} A on {b:.0} A"));
                    findings.push(Finding {
                        rule: "E-LOAD-CIRCUIT",
                        level: "circuit",
                        panel: p.tag.clone(),
                        circuit: cc.number.clone(),
                        message: format!(
                            "{} {} ({}): {side} {} VA is {load_a:.1} A on a {b:.0} A breaker{why}",
                            p.tag,
                            cc.number,
                            cc.description,
                            va(used)
                        ),
                        likely,
                        cite: rules.circuit_limit_cite.clone(),
                    });
                } else if cont > 0.0 && need > b + 1e-6 {
                    notes.push(format!("continuous {pct:.0}% of its breaker"));
                    findings.push(Finding {
                        rule: "E-LOAD-CIRCUIT",
                        level: "circuit",
                        panel: p.tag.clone(),
                        circuit: cc.number.clone(),
                        message: format!(
                            "{} {} ({}): {side} continuous load {} VA is {pct:.0}% of its {b:.0} A breaker (over {:.0}%): {need:.1} A with 125% of it{why}",
                            p.tag,
                            cc.number,
                            cc.description,
                            va(cont),
                            rules.circuit_limit_pct
                        ),
                        likely,
                        cite: rules.circuit_limit_cite.clone(),
                    });
                }
            }
            // E-LOAD-SCHEDULE: the model's load against the schedule's.
            if let (Some(m), Some(s)) = (cc.model_va, cc.schedule_va)
                && cc.feeds.is_none()
                && (m - s).abs() > rules.tolerance(m, s)
            {
                let (likely, why) = if cc.foreign > 0 {
                    ("model", cc.foreign_what.clone())
                } else if cc.unknown > 0 && m < s {
                    ("model", format!("{} device(s) on it have no load in the model", cc.unknown))
                } else if cc.documented && cc.unknown == 0 {
                    ("schedule", format!("the model's loads are all documented ({})", cc.sources.first().cloned().unwrap_or_default()))
                } else if cc.rule_receptacles > 0 {
                    // A unit's receptacles: the count over its rooms.
                    let n = (cc.rule_receptacles as f64 / cc.units.max(1) as f64).max(1e-9);
                    (
                        "either",
                        format!(
                            "{} receptacle(s) at 180 VA by rule; the schedule's {} VA is {} VA a receptacle if they are all its load",
                            if cc.units > 1 { format!("{n:.1}") } else { format!("{n:.0}") },
                            va(s),
                            va(s / n)
                        ),
                    )
                } else {
                    ("either", "the model counts the devices the plans put on it".to_string())
                };
                notes.push(format!("schedule {} VA vs model {} VA", va(s), va(m)));
                findings.push(Finding {
                    rule: "E-LOAD-SCHEDULE",
                    level: "circuit",
                    panel: p.tag.clone(),
                    circuit: cc.number.clone(),
                    message: format!(
                        "{} {} ({}): the schedule gives {} VA, the model {} VA{}: {why}",
                        p.tag,
                        cc.number,
                        cc.description,
                        va(s),
                        va(m),
                        if cc.units > 1 { format!(" (one unit's, over {} rooms)", cc.units) } else { String::new() }
                    ),
                    likely,
                    cite: rules.tolerance_cite.clone(),
                });
            }
            // E-LOAD-SCHEDULE: a feeder row against its panel's own total.
            if let Some((child, n)) = &cc.feeds
                && let (Some(row), Some(cp)) = (cc.schedule_va, c.panel(child))
            {
                let child_total = cp.printed.get("connected_va").copied().or_else(|| {
                    let s: f64 = cp.circuits.iter().filter_map(|x| x.schedule_va).sum();
                    (s > 0.0).then_some(s)
                });
                if let Some(ct) = child_total.map(|t| t * f64::from(*n))
                    && (row - ct).abs() > rules.tolerance(row, ct)
                {
                    // Another feeder row of this panel that carries this panel's total: swapped.
                    let swapped = p.circuits.iter().filter(|o| o.number != cc.number).find_map(|o| {
                        let (oc, on) = o.feeds.as_ref()?;
                        let ot = c.panel(oc)?.printed.get("connected_va").copied()? * f64::from(*on);
                        ((row - ot).abs() <= rules.tolerance(row, ot))
                            .then(|| format!(" - it is {oc}'s total: rows {} and {} look swapped", cc.number, o.number))
                    });
                    notes.push(format!("row {} VA vs {child}'s {} VA", va(row), va(ct)));
                    findings.push(Finding {
                        rule: "E-LOAD-SCHEDULE",
                        level: "feeder",
                        panel: p.tag.clone(),
                        circuit: cc.number.clone(),
                        message: format!(
                            "{} {} feeds {child}{}: its row gives {} VA, {child}'s schedule totals {} VA{}",
                            p.tag,
                            cc.number,
                            if *n > 1 { format!(" ({n} units)") } else { String::new() },
                            va(row),
                            va(ct),
                            swapped.unwrap_or_default()
                        ),
                        likely: "schedule",
                        cite: format!("{}'s row against {child}'s own schedule ({})", p.tag, cp.sheet),
                    });
                }
            }
            // E-LOAD-RATING: the feeder's demand against its breaker and conductors.
            let mut amps_cell = String::new();
            let mut demand_cells = (String::new(), String::new());
            if let Some((child, n)) = &cc.feeds {
                let mut sub = Tree::default();
                sub.add(&c.tree(child, 1, &mut Vec::new()), *n);
                let (fd, _, _) = demand(&sub, &c.occ, &c.commercial, rules);
                let child_sys = c.panel(child).map_or(sys, |x| system(&x.sys_text));
                let fa = panel_amps(fd, if cc.poles >= 3 { sys } else { System { phases: 1, ..child_sys } });
                demand_cells = (format!("{fd:.0}"), format!("{fa:.1}"));
                amps_cell = format!("{fa:.1}");
                // The documents' demand calculation for a feeder of this many units.
                for s in c.summaries.iter().filter(|s| feeder_summary(s, rules) && summary_units(s, rules) == Some(*n)) {
                    let (_, _, da, _) = summary_facts(s, rules);
                    let Some(da) = da else { continue };
                    notes.push(format!("{} ({}) gives {da:.0} A", s.title, s.source));
                    if feeder_seen.insert((child.clone(), *n)) && (da - fa).abs() > (rules.tolerance_pct / 100.0 * da.max(fa)).max(5.0) {
                        findings.push(Finding {
                            rule: "E-LOAD-SCHEDULE",
                            level: "feeder",
                            panel: p.tag.clone(),
                            circuit: cc.number.clone(),
                            message: format!(
                                "{} {} feeds {child} ({n} units): the documents' {} ({}) gives {da:.0} A demand{}; the model's {fa:.0} A",
                                p.tag,
                                cc.number,
                                s.title,
                                s.source,
                                if s.basis.is_empty() { String::new() } else { format!(" by NEC {}", s.basis) }
                            ),
                            likely: "either",
                            cite: rules.tolerance_cite.clone(),
                        });
                    }
                }
                if let Some(b) = cc.breaker
                    && fa > b + 1e-6
                {
                    notes.push(format!("feeder demand {fa:.0} A over its {b:.0} A breaker"));
                    findings.push(Finding {
                        rule: "E-LOAD-RATING",
                        level: "feeder",
                        panel: p.tag.clone(),
                        circuit: cc.number.clone(),
                        message: format!(
                            "{} {} feeds {child}{}: its demand {} VA is {fa:.0} A on a {b:.0} A breaker",
                            p.tag,
                            cc.number,
                            if *n > 1 { format!(" ({n} units)") } else { String::new() },
                            va(fd)
                        ),
                        likely: "design",
                        cite: "NEC 215.3, 240.4: a feeder's protection at least its load (continuous at 125%)".into(),
                    });
                }
                let f = c.feeders.iter().find(|f| f.to.trim().eq_ignore_ascii_case(child) && f.from.trim().eq_ignore_ascii_case(&p.tag));
                if let Some(f) = f
                    && let Some(amp) = feeder_ampacity(f, &c.ampacity)
                    && fa > amp + 1e-6
                {
                    notes.push(format!("feeder demand {fa:.0} A over its conductors' {amp:.0} A"));
                    findings.push(Finding {
                        rule: "E-LOAD-RATING",
                        level: "feeder",
                        panel: p.tag.clone(),
                        circuit: cc.number.clone(),
                        message: format!(
                            "feeder {} from {} to {child}: {} set(s) of {} {} carry {amp:.0} A (75 C); its demand is {fa:.0} A",
                            f.tag,
                            p.tag,
                            f.sets.max(1),
                            f.size,
                            f.material,
                        ),
                        likely: "design",
                        cite: "NEC 215.2(A)(1), Table 310.16 (310.15(B)(16)) 75 C column".into(),
                    });
                }
            } else if cc.used_va > 0.0 {
                amps_cell = format!("{:.1}", circuit_amps(cc.used_va, cc.poles, sys));
            }
            let pct = match (cc.breaker, amps_cell.parse::<f64>().ok()) {
                (Some(b), Some(a)) if b > 0.0 => format!("{:.0}", a / b * 100.0),
                _ => String::new(),
            };
            let cat = cc.feeds.as_ref().map_or_else(
                || cc.cats.iter().max_by(|a, b| a.1.total_cmp(b.1)).map_or_else(|| "none".to_string(), |(k, _)| (*k).to_string()),
                |(t, n)| format!("feeder to {t} x{n}"),
            );
            let sources = if cc.feeds.is_some() {
                format!("{}'s calculated load below", cc.feeds.as_ref().map(|f| f.0.as_str()).unwrap_or(""))
            } else {
                cc.sources.join("; ")
            };
            csv.push_str(&format!(
                "circuit,{},{},{},{},{},{},{},{:.0},{},{},{:.0},{},{},{},{},{},{},{}\n",
                cell(&p.tag),
                cell(&cc.number),
                cell(&cc.description),
                opt(cc.breaker),
                cc.poles,
                opt(cc.schedule_va),
                opt(cc.feeds.as_ref().map_or(cc.model_va, |_| Some(used))),
                used,
                cc.used_from,
                cell(&cat),
                cc.continuous_va,
                amps_cell,
                opt(cc.breaker),
                pct,
                demand_cells.0,
                demand_cells.1,
                cell(&notes.join("; ")),
                cell(&sources)
            ));
            text.push_str(&format!(
                "  {:<4} {:<34} {:>7}  {:>11}  {:>8}  {:>7}  {:<9}  {}\n",
                cc.number,
                cc.description.chars().take(34).collect::<String>(),
                cc.breaker.map_or(String::new(), |b| format!("{b:.0}/{}", cc.poles)),
                cc.schedule_va.map(va).unwrap_or_default(),
                cc.feeds.as_ref().map_or(cc.model_va.map(va).unwrap_or_default(), |_| va(used)),
                va(used),
                cc.used_from,
                if notes.is_empty() {
                    sources.chars().take(160).collect::<String>()
                } else {
                    format!("{} | {}", notes.join("; "), sources.chars().take(120).collect::<String>())
                }
            ));
        }
        // E-LOAD-SCHEDULE: the schedule's own totals against its circuits.
        if let Some(pt) = p.printed.get("connected_va").copied()
            && (pt - sched_sum).abs() > rules.tolerance(pt, sched_sum)
        {
            findings.push(Finding {
                rule: "E-LOAD-SCHEDULE",
                level,
                panel: p.tag.clone(),
                circuit: String::new(),
                message: format!(
                    "{}: the schedule prints {} VA connected; its circuits read add to {} VA (its arithmetic, or a circuit's load not read)",
                    p.tag,
                    va(pt),
                    va(sched_sum)
                ),
                likely: "schedule",
                cite: format!("the schedule's own lines ({})", p.sheet),
            });
        }
        if let Some(pt) = p.printed.get("connected_va").copied()
            && (pt - model_sum).abs() > rules.tolerance(pt, model_sum)
        {
            let differ = findings.iter().filter(|f| f.panel == p.tag && f.rule == "E-LOAD-SCHEDULE" && f.level == "circuit").count();
            let fed: BTreeSet<String> = p.circuits.iter().filter_map(|x| x.feeds.as_ref().map(|f| f.0.clone())).collect();
            let below = if differ == 0 && !fed.is_empty() {
                format!(", the difference is in the panels it feeds: {}", fed.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                String::new()
            };
            let model_side = p.circuits.iter().filter(|x| x.unknown > 0).count();
            findings.push(Finding {
                rule: "E-LOAD-SCHEDULE",
                level,
                panel: p.tag.clone(),
                circuit: String::new(),
                message: format!(
                    "{}: the schedule prints {} VA connected, the model's loads {} VA ({differ} circuit(s) differ beyond tolerance{below}{})",
                    p.tag,
                    va(pt),
                    va(model_sum),
                    if model_side > 0 { format!("; {model_side} circuit(s) have devices with no load data") } else { String::new() }
                ),
                likely: if model_side > 0 && model_sum < pt { "model" } else { "either" },
                cite: rules.tolerance_cite.clone(),
            });
        }
        // E-LOAD-RATING: the panel's demand against its rating.
        if let Some(r) = p.rating_a
            && dem_a > r + 1e-6
        {
            findings.push(Finding {
                rule: "E-LOAD-RATING",
                level,
                panel: p.tag.clone(),
                circuit: String::new(),
                message: format!("{}: its demand {} VA is {dem_a:.0} A, over its {r:.0} A rating ({})", p.tag, va(dem), p.rating_from),
                likely: "design",
                cite: "NEC 408.30, 215.2, 230.42: equipment and feeders rated for the calculated load".into(),
            });
        }
        if let (Some(r), Some(pa)) = (p.rating_a, p.printed.get("demand_a").copied())
            && pa > r + 1e-6
        {
            findings.push(Finding {
                rule: "E-LOAD-RATING",
                level,
                panel: p.tag.clone(),
                circuit: String::new(),
                message: format!("{}: the schedule's own demand {pa:.0} A is over its {r:.0} A rating ({})", p.tag, p.rating_from),
                likely: "schedule",
                cite: "NEC 408.30".into(),
            });
        }
        // E-LOAD-PHASE: the model's phases and the schedule's.
        let n_ph = if sys.phases >= 3 { 3 } else { 2 };
        let model_ph: Vec<f64> = phase.iter().take(n_ph).copied().collect();
        let printed_ph: Vec<f64> = ["phase_a_va", "phase_b_va", "phase_c_va"].iter().take(n_ph).filter_map(|k| p.printed.get(*k).copied()).collect();
        for (who, ph, likely) in [("the model's", &model_ph, "design"), ("the schedule's", &printed_ph, "schedule")] {
            if let Some(im) = imbalance(ph)
                && im > rules.phase_imbalance_pct
                && ph.iter().sum::<f64>() > rules.tolerance_va * 10.0
            {
                findings.push(Finding {
                    rule: "E-LOAD-PHASE",
                    level,
                    panel: p.tag.clone(),
                    circuit: String::new(),
                    message: format!(
                        "{}: {who} phase loads {} VA are {im:.0}% out of balance (limit {:.0}%)",
                        p.tag,
                        ph.iter().map(|v| va(*v)).collect::<Vec<_>>().join(" / "),
                        rules.phase_imbalance_pct
                    ),
                    likely,
                    cite: rules.phase_imbalance_cite.clone(),
                });
            }
        }
        // The documents' demand calculation for this panel (a summary naming it, or the service's).
        let docs: Vec<&LoadSummary> = c
            .summaries
            .iter()
            .filter(|s| if s.panel.is_empty() { is_service && !feeder_summary(s, rules) } else { s.panel.eq_ignore_ascii_case(&p.tag) })
            .collect();
        let mut doc_json = Vec::new();
        for s in &docs {
            let (size, dva, da, cva) = summary_facts(s, rules);
            doc_json.push(json!({"title": s.title, "sheet": s.source, "basis": s.basis, "service_size_a": size, "demand_va": dva, "demand_a": da, "connected_va": cva}));
            // The documents' own method on the model's loads: the optional multifamily method's
            // factor by the units under it.
            let optional = (s.basis == "220.84")
                .then(|| rules.occupancies.iter().find(|o| !o.optional_units.is_empty()))
                .flatten()
                .and_then(|o| by_count(tree.units, &o.optional_units).map(|f| (f, o.optional_cite.clone())));
            let optional_text = optional.as_ref().map_or(String::new(), |(f, _)| {
                let ov = tree.connected_va * f;
                format!(
                    "; by NEC 220.84 on the model's loads ({} units at {:.0}%): {} VA = {:.0} A",
                    tree.units,
                    f * 100.0,
                    va(ov),
                    panel_amps(ov, sys)
                )
            });
            if s.basis == "220.84" && method.optional.is_none() {
                findings.push(Finding {
                    rule: "E-LOAD-SCHEDULE",
                    level,
                    panel: p.tag.clone(),
                    circuit: String::new(),
                    message: format!(
                        "{}: the documents' {} ({}) uses NEC 220.84, the optional method for three or more dwelling units each with electric cooking and heating or cooling; under {} the model has {} unit(s), {} with cooking, {} with heating or cooling ({} rules){optional_text}",
                        p.tag,
                        s.title,
                        s.source,
                        p.tag,
                        tree.units,
                        tree.cooking_units,
                        tree.hvac_units,
                        if method.dwelling { "dwelling".to_string() } else { c.occ.name.clone() }
                    ),
                    likely: "documents",
                    cite: optional.as_ref().map_or_else(|| "NEC 220.84".to_string(), |(_, c)| c.clone()),
                });
            }
            if let Some(da) = da
                && (da - dem_a).abs() > (rules.tolerance_pct / 100.0 * da.max(dem_a)).max(5.0)
            {
                findings.push(Finding {
                    rule: "E-LOAD-SCHEDULE",
                    level,
                    panel: p.tag.clone(),
                    circuit: String::new(),
                    message: format!(
                        "{}: the documents' {} ({}) gives {da:.0} A demand{}; the model's calculation {dem_a:.0} A ({}){optional_text}",
                        p.tag,
                        s.title,
                        s.source,
                        if s.basis.is_empty() { String::new() } else { format!(" by NEC {}", s.basis) },
                        match (method.optional, method.dwelling) {
                            (Some(f), _) => format!("the optional method, {} units at {:.0}%", tree.units, f * 100.0),
                            (None, true) => "dwelling rules, its units having cooking: NEC 210.18".to_string(),
                            (None, false) => format!("{} rules", c.occ.name),
                        }
                    ),
                    likely: "either",
                    cite: optional.as_ref().map_or_else(|| c.occ.lighting_cite.clone(), |(_, cite)| cite.clone()),
                });
            }
            if let Some(size) = size
                && dem_a > size + 1e-6
            {
                findings.push(Finding {
                    rule: "E-LOAD-RATING",
                    level: "service",
                    panel: p.tag.clone(),
                    circuit: String::new(),
                    message: format!("the service: the model's demand {dem_a:.0} A is over the {size:.0} A service size {} gives", s.title),
                    likely: "design",
                    cite: "NEC 230.42, 230.79".into(),
                });
            }
        }
        // A typical panel's units: the feeders' unit counts against its `typical of`.
        if let Some(t) = p.typical_of {
            let fed_units: u32 =
                c.panels.iter().flat_map(|x| &x.circuits).filter_map(|cc| cc.feeds.as_ref()).filter(|(ch, _)| ch == &p.tag).map(|(_, n)| *n).sum();
            if fed_units > 0 && fed_units != t {
                findings.push(Finding {
                    rule: "E-LOAD-SCHEDULE",
                    level,
                    panel: p.tag.clone(),
                    circuit: String::new(),
                    message: format!("{}: its schedule is typical of {t}; the feeder rows feed {fed_units} units", p.tag),
                    likely: "schedule",
                    cite: "the schedules' own counts".into(),
                });
            }
        }
        let printed = |k: &str| p.printed.get(k).copied();
        text.push_str(&format!(
            "  connected: model {} VA; schedule circuits {} VA{}\n",
            va(model_sum),
            va(sched_sum),
            printed("connected_va").map(|v| format!("; schedule prints {} VA", va(v))).unwrap_or_default()
        ));
        text.push_str(&format!(
            "  demand ({} rules{}):\n",
            c.occ.name,
            if tree.units > 1 { format!(", {} units under it", tree.units) } else { String::new() }
        ));
        for st in &steps {
            text.push_str(&format!("    {:<44} {:>12} VA -> {:>12} VA   [{}]\n", st.what, va(st.connected), va(st.demand), st.cite));
        }
        text.push_str(&format!(
            "  demand: model {} VA = {dem_a:.1} A{}{}\n",
            va(dem),
            p.rating_a.map(|r| format!(" ({:.0}% of {r:.0} A)", dem_a / r * 100.0)).unwrap_or_default(),
            printed("demand_va")
                .map(|v| format!("; schedule prints {} VA{}", va(v), printed("demand_a").map(|a| format!(" / {a:.0} A")).unwrap_or_default()))
                .unwrap_or_default()
        ));
        text.push_str(&format!(
            "  phases (VA): model {}{}\n",
            model_ph.iter().map(|v| va(*v)).collect::<Vec<_>>().join(" / "),
            if printed_ph.is_empty() {
                String::new()
            } else {
                format!("; schedule {}", printed_ph.iter().map(|v| va(*v)).collect::<Vec<_>>().join(" / "))
            }
        ));
        for s in &docs {
            text.push_str(&format!(
                "  documents: {} ({}){}\n",
                s.title,
                s.source,
                if s.basis.is_empty() { String::new() } else { format!(", based on NEC {}", s.basis) }
            ));
            for l in s.lines.iter().filter(|l| !l.values.is_empty()).take(60) {
                text.push_str(&format!("    {}\n", l.text));
            }
        }
        for f in findings.iter().filter(|f| f.panel == p.tag) {
            text.push_str(&format!("  ! {} [{}] {} (likely wrong: {})\n", f.rule, f.level, f.message, f.likely));
        }
        text.push('\n');
        csv.push_str(&format!(
            "{level},{},,,{},,{},,{:.0},model,,{:.0},{:.1},{},{},{:.0},{:.1},,{}\n",
            cell(&p.tag),
            opt(p.rating_a),
            opt(printed("connected_va")),
            model_sum,
            tree.continuous_va,
            dem_a,
            opt(p.rating_a),
            p.rating_a.map(|r| format!("{:.0}", dem_a / r * 100.0)).unwrap_or_default(),
            dem,
            dem_a,
            cell(&steps.iter().map(|s| format!("{}: {:.0} -> {:.0} VA ({})", s.what, s.connected, s.demand, s.cite)).collect::<Vec<_>>().join("; "))
        ));
        let pj = json!({
            "panel": p.tag, "typical_of": p.typical_of, "sheet": p.sheet, "system": p.sys_text, "level": level,
            "rating_a": p.rating_a, "rating_from": p.rating_from,
            "connected_va": {"model": r0(model_sum), "schedule_circuits": r0(sched_sum), "printed": printed("connected_va")},
            "demand_va": r0(dem), "demand_a": r1(dem_a), "printed_demand": {"va": printed("demand_va"), "a": printed("demand_a")},
            "phases_va": {"model": model_ph.iter().map(|v| r0(*v)).collect::<Vec<_>>(), "printed": printed_ph},
            "units_in_model": p.units, "units_under": tree.units,
            "circuits": p.circuits.len(),
            "from_model": p.circuits.iter().filter(|x| x.used_from == "model" || x.used_from == "submittal").count(),
            "from_schedule": p.circuits.iter().filter(|x| x.used_from == "schedule").count(),
            "documents": doc_json,
            "demand_steps": steps.iter().map(|s| json!({"what": s.what, "connected_va": r0(s.connected), "demand_va": r0(s.demand), "cite": s.cite})).collect::<Vec<_>>(),
        });
        if is_service {
            service_json = pj.clone();
        }
        panels_json.push(pj);
    }
    let mut by_rule: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &findings {
        *by_rule.entry(f.rule).or_default() += 1;
    }
    let total = findings.len();
    text.push_str(&format!("FINDINGS: {total} ({})\n", by_rule.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")));
    for f in &findings {
        csv.push_str(&format!(
            "finding,{},{},{},,,,,,,{},,,,,,,{},{}\n",
            cell(&f.panel),
            cell(&f.circuit),
            cell(&f.message),
            f.rule,
            cell(&format!("likely wrong: {}", f.likely)),
            cell(&f.cite)
        ));
    }
    let out = json!({
        "occupancy": {"id": c.occ.id, "name": c.occ.name, "groups": c.occ.groups, "from": c.occ_from},
        "service": service_json,
        "panels": panels_json,
        "findings": findings.iter().take(MAX_FINDINGS).map(Finding::json).collect::<Vec<_>>(),
        "findings_total": total,
        "by_rule": by_rule,
        "summaries_read": c.summaries.len(),
    });
    (out, csv, text)
}

// ---------------------------------------------------------------- the command

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Load Calculation", run)
        .params("{occupancy?: \"hotel\" | \"dwelling\" | \"commercial\" | a group (\"R-1\") (default the code sheet's: code_texts), code_texts?: [string] (the code-analysis sheet's texts in reading order), code_sheet?: sheet number (the code texts' sheet, named in occupancy.from), csv?: path, text?: path, force?: bool (replace existing files), panel?: tag (report one panel)} → {occupancy: {id, name, groups, from}, service: {panel, connected_va: {model, schedule_circuits, printed}, demand_va, demand_a, rating_a, phases_va, documents: [{title, sheet, basis, service_size_a, demand_va, demand_a}], demand_steps: [{what, connected_va, demand_va, cite}]}, panels: [same], findings: [{rule: E-LOAD-CIRCUIT | E-LOAD-RATING | E-LOAD-PHASE | E-LOAD-SCHEDULE, level: circuit | feeder | panel | service, panel, circuit, message, likely_wrong: schedule | model | design | either, cite}], findings_total, by_rule, csv?, text?}: an NEC 220-style load calculation from the model (luminaires at the fixture schedule's VA, receptacles by rule, equipment from its submittal, else the schedule's VA), per circuit -> panel -> feeder -> service with demand factors from rules data by occupancy, checked against the panel schedules' totals and phases, breaker, feeder and service sizes and the documents' load summaries")
        .menu(&["Tools", "Building", "Load Calculation"])
        .files()]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    for k in ["csv", "text"] {
        if let Some(path) = str_param(p, k).map(str::trim).filter(|x| !x.is_empty()) {
            super::exchange::guard_output(s, CMD, p, path)?;
        }
    }
    let (sp, stored) = super::bspec::current(s, CMD)?;
    if !stored || sp.mep.panel_schedules.is_empty() {
        return Ok(
            json!({"panels": [], "findings": [], "note": "no panel schedules in this drawing: a job reads them off the electrical sheets (job.start)"}),
        );
    }
    let rules = rules();
    let code_texts: Vec<String> = p
        .get("code_texts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|t| t.chars().take(2_000).collect())
        .take(20_000)
        .collect();
    // The code sheet's occupancies: given, else the ones the job kept with the spec.
    let groups = if !code_texts.is_empty() {
        super::valueeng::occupancies(&code_texts)
    } else {
        sp.mep
            .occupancy
            .split(',')
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .take(20)
            .map(|g| (g.to_string(), sp.mep.occupancy_source.clone()))
            .collect()
    };
    let given = str_param(p, "occupancy");
    let (occ, occ_from) = pick_occupancy(&rules, given, &groups)
        .map(|(o, f)| {
            (
                o.clone(),
                str_param(p, "code_sheet")
                    .map_or(f.clone(), |sh| f.replacen("the code sheet", &format!("code sheet {}", sh.chars().take(40).collect::<String>()), 1)),
            )
        })
        .ok_or_else(|| bad(CMD, format!("unknown occupancy {}: hotel, dwelling, commercial or a group (R-1, R-2, B…)", given.unwrap_or_default())))?;
    let d = s.doc()?;
    let devs = model_devices(d, &rules);
    // The unit rooms: rooms whose name has the occupancy's unit words.
    let names: BTreeMap<String, String> = super::bspec::room_elements(d)
        .into_iter()
        .filter_map(|r| r.number.clone().filter(|n| !n.trim().is_empty()).map(|n| (n.trim().to_ascii_uppercase(), r.name.to_ascii_uppercase())))
        .collect();
    let unit_words = occ.unit_room_words.clone();
    let unit_rooms = move |room: &str| {
        let r = room.trim().to_ascii_uppercase();
        let name = names.get(&r).cloned().unwrap_or(r);
        !unit_words.is_empty() && has_word(&words(&name), &unit_words)
    };
    let calc = Calc::new(&sp, &devs, rules, occ, occ_from, &unit_rooms);
    let (mut out, csv, text) = calculate(&calc);
    if let Some(want) = str_param(p, "panel").map(str::trim).filter(|x| !x.is_empty()) {
        let keep: Vec<Value> = out["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["panel"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(want)))
            .cloned()
            .collect();
        if keep.is_empty() {
            return Err(bad(CMD, format!("no schedule for panel {want}")));
        }
        out["panels"] = json!(keep);
        let f: Vec<Value> = out["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["panel"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(want)))
            .cloned()
            .collect();
        out["findings"] = json!(f);
    }
    if let Some(path) = str_param(p, "csv").map(str::trim).filter(|x| !x.is_empty()) {
        crate::writefile::write(path, &csv).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        out["csv"] = json!(path);
    }
    if let Some(path) = str_param(p, "text").map(str::trim).filter(|x| !x.is_empty()) {
        crate::writefile::write(path, &text).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        out["text"] = json!(path);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
