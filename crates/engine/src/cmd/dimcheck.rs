//! The dimming check (`bim.dimming`): what each switch leg dims.
//!
//! The submitted products say how each fixture dims (`dimming`: 0-10V, phase, DALI, non-dim;
//! `bim.submittals`). The plans give each fixture's circuit and switch leg (`A-1,a`: circuit A-1,
//! leg a). The fixtures on one leg share one control:
//!
//! - **mixed**: a phase fixture and a 0-10V fixture on one leg. One dimmer can't dim both: a
//!   phase dimmer doesn't drive the 0-10V driver, and a 0-10V control doesn't cut the phase.
//! - **0-10V pair**: every 0-10V fixture on a leg needs the 2-wire control pair from the leg's
//!   dimmer or sensor, besides its power. The takeoff (`bim.bom`) carries it per leg: the leg's
//!   0-10V fixtures chained, plus an allowance to the control, which the plans don't place.
//! - **dimmer protocol**: a dimmer dims the way its leg's fixtures do (0-10V, phase, DALI); one
//!   whose leg mixes two is a question, one whose fixtures say nothing is unknown. `bim.submittals`
//!   puts it on the dimmer (`dimming`) and the takeoff buys dimmers by it.

use std::collections::BTreeMap;

use cadaddy_doc::props::{self, SourceKind};
use cadaddy_doc::{Drawing, Handle};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.dimming";
/// Fixtures kept.
const MAX_ELEMENTS: usize = 500_000;
/// Legs listed in a report.
const MAX_LISTED: usize = 500;
/// Dimmers looked at.
const MAX_DIMMERS: usize = 50_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Dimming Check", run)
            .params("{} → {legs: [{level, room, circuit, leg, fixtures: {mark: n}, dimming: {control: [mark]}, verdict: mixed|0-10V|phase|DALI|non-dim|unknown, pair_fixtures, pair_ft?, controls}], mixed, pairs: {legs, fixtures, feet}, dimmers: {protocol: n}, dimmer_list: [{level, room, leg, mark, protocol, why}], by_level: {level: {pair_legs, pair_fixtures, pair_ft, dimmers: {protocol: n}}}, unknown: {mark: n}, no_leg: {circuits, fixtures}, findings: [text]}: the fixtures on each switch leg (circuit and leg letter printed with them, in one room) and how their submitted products dim; a leg with phase and 0-10V fixtures is a finding, each leg with 0-10V fixtures needs a 2-wire control pair to them (bim.bom carries it), and each dimmer dims the way its leg's fixtures do (0-10V, phase, DALI; mixed is a question)")
            .menu(&["Tools", "Building", "Dimming Check"])
            .noundo(),
    ]
}

/// From a leg's fixtures to its dimmer or sensor (the plans don't place the control), feet.
pub(crate) const CONTROL_ALLOWANCE_FT: f64 = 15.0;
/// Fixtures chained on one leg at most (the chain is worked out pair by pair).
const MAX_CHAIN: usize = 2_000;
/// A step longer than this between a leg's fixtures is no pair run between them: a fixture that
/// far away (a room not traced, a template's leg stamped into another room) gets its own run
/// to a control, feet.
const MAX_STEP_FT: f64 = 30.0;

/// One switch leg: the fixtures it controls.
#[derive(Default)]
struct Leg {
    fixtures: BTreeMap<String, usize>,
    /// Each control the fixtures' products use, and the marks that use it.
    dimming: BTreeMap<String, Vec<String>>,
    /// Fixtures whose product says nothing of its dimming, and their marks.
    unknown: usize,
    unknown_marks: Vec<String>,
    /// Its 0-10V fixtures: handle and where (drawing units).
    pair: Vec<(String, cadaddy_geom::Vec2)>,
}

/// The fixtures by switch leg, (level, room, circuit, leg); fixtures of unknown dimming by mark;
/// fixtures printed with a circuit and no leg, by (level, circuit); the switches and sensors by
/// the leg they control, (level, room, leg).
struct Legs {
    legs: BTreeMap<(String, String, String, String), Leg>,
    unknown: BTreeMap<String, usize>,
    no_leg: BTreeMap<(String, String), usize>,
    controls: BTreeMap<(String, String, String), Vec<Control>>,
}

/// A switch or sensor element on a leg.
struct Control {
    mark: String,
    at: cadaddy_geom::Vec2,
    /// Whether it dims (`control_dims`).
    dims: Option<bool>,
    handle: Handle,
    /// Its spec tag (`devid:`), when the build linked one.
    devid: Option<String>,
    /// How it dims by its own product or a person (`dimming`), with where that came from.
    own: Option<(String, String)>,
}

/// Whether a control dims: one its legend calls a dimmer (`switch, dimmer`) does; else by its
/// designation: a dimmer (`SD`) does, a plain switch (`S`, `S3`…) doesn't; a sensor or anything
/// else: its product says (unknown here).
fn control_dims(mark: &str, item: &str) -> Option<bool> {
    let item = item.to_ascii_lowercase();
    if item.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "dimmer" || w == "dimmers") {
        return Some(true);
    }
    match mark.trim().to_ascii_uppercase().as_str() {
        "SD" | "DS" => Some(true),
        "S" | "S2" | "S3" | "S4" | "SP" | "SM" | "SH" | "SK" | "SOR" | "SWT" => Some(false),
        _ => None,
    }
}

/// A switch leg's 0-10V fixtures and the control pair they need.
pub(crate) struct PairLeg {
    pub level: String,
    pub room: String,
    pub circuit: String,
    pub leg: String,
    pub handles: Vec<String>,
    /// The pair's length: the fixtures chained to the leg's dimming control (where the plans
    /// place one) plus `CONTROL_ALLOWANCE_FT` a run that reaches none, feet.
    pub feet: f64,
}

/// Each switch leg's 0-10V fixtures and the pair they need. A leg whose switches all just switch
/// (no dimmer) needs none.
pub(crate) fn zero_ten_pairs(s: &Session) -> Vec<PairLeg> {
    let Ok(d) = s.doc() else { return Vec::new() };
    let upf = units_per_foot(s, d);
    let Legs { legs, controls, .. } = read_legs(d);
    legs.into_iter()
        .filter(|(_, l)| !l.pair.is_empty())
        .filter_map(|((level, room, circuit, leg), l)| {
            // A leg's controls are its room's: fixtures in no traced room have none known.
            let ctl =
                if room.is_empty() { &[][..] } else { controls.get(&(level.clone(), room.clone(), leg.clone())).map_or(&[][..], Vec::as_slice) };
            if !ctl.is_empty() && ctl.iter().all(|c| c.dims == Some(false)) {
                return None;
            }
            let mut pts: Vec<cadaddy_geom::Vec2> = l.pair.iter().map(|(_, p)| *p).collect();
            // From the dimming control the plans place (the first run reaches it).
            let placed = ctl.iter().find(|c| c.dims != Some(false)).map(|c| c.at);
            pts.extend(placed);
            let (chained, runs) = chain_length(&pts, MAX_STEP_FT * upf);
            let unreached = if placed.is_some() { runs.saturating_sub(1) } else { runs };
            let feet = chained / upf + CONTROL_ALLOWANCE_FT * unreached as f64;
            Some(PairLeg { level, room, circuit, leg, handles: l.pair.into_iter().map(|(h, _)| h).collect(), feet: (feet * 10.0).round() / 10.0 })
        })
        .collect()
}

fn units_per_foot(s: &Session, d: &cadaddy_doc::Drawing) -> f64 {
    super::bspec::current(s, CMD)
        .ok()
        .map(|(sp, _)| sp.units_per_foot)
        .filter(|u| u.is_finite() && *u > 0.0)
        .unwrap_or_else(|| 12.0 * super::perceive::units_per_inch(d))
}

/// The shortest way to chain `pts` together (each joined to its nearest one already chained),
/// drawing units, with no step longer than `max_step`; and how many runs that makes (a longer
/// step starts another).
fn chain_length(pts: &[cadaddy_geom::Vec2], max_step: f64) -> (f64, usize) {
    let pts = &pts[..pts.len().min(MAX_CHAIN)];
    let Some(first) = pts.first() else { return (0.0, 0) };
    let mut runs = 1usize;
    let mut best: Vec<f64> = pts.iter().map(|p| p.dist(*first)).collect();
    let mut joined = vec![false; pts.len()];
    joined[0] = true;
    let mut total = 0.0;
    for _ in 1..pts.len() {
        let Some((i, d)) = best.iter().enumerate().filter(|(i, _)| !joined[*i]).min_by(|a, b| a.1.total_cmp(b.1)).map(|(i, d)| (i, *d)) else {
            break;
        };
        joined[i] = true;
        if d <= max_step {
            total += d;
        } else {
            runs += 1;
        }
        for (j, p) in pts.iter().enumerate() {
            if !joined[j] {
                best[j] = best[j].min(p.dist(pts[i]));
            }
        }
    }
    (total, runs)
}

fn read_legs(d: &Drawing) -> Legs {
    // (level, room, circuit, leg) → the leg's fixtures. A leg is a room's: a typical room's
    // fixtures all carry the template's circuit and leg, each room its own.
    let mut legs: BTreeMap<(String, String, String, String), Leg> = BTreeMap::new();
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_leg: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut controls: BTreeMap<(String, String, String), Vec<Control>> = BTreeMap::new();
    // Every entity is looked at (a print set's linework comes first), the fixtures kept capped.
    let mut kept = 0usize;
    for e in d.model.iter() {
        if kept >= MAX_ELEMENTS {
            break;
        }
        let Some(kind) = super::elemprops::element_kind(e) else { continue };
        if kind != "device" && kind != "light_fixture" {
            continue;
        }
        let store = props::read(&e.common.xdata);
        let text = |k: &str| store.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).filter(|v| !v.is_empty());
        let Some(mark) = text("type_mark") else { continue };
        // A switch or sensor (or a device its legend calls a dimmer, not a luminaire): the leg it
        // controls.
        let item = text("type").unwrap_or_default();
        if matches!(super::propfill::kind_of(&mark), "switch" | "occupancy sensor") || (kind == "device" && control_dims(&mark, &item) == Some(true))
        {
            if let (Some(leg), Some(at)) = (text("switch_leg"), super::bspec::centre(d, e)) {
                kept += 1;
                let key = (super::bspec::made_level(e).unwrap_or_default(), text("room").unwrap_or_default(), leg);
                let dims = control_dims(&mark, &item);
                // Its own product's or a person's dimming (not one worked out from its leg).
                let own =
                    store.get("dimming").filter(|p| p.has_value() && matches!(p.source.kind, SourceKind::User | SourceKind::Submittal)).map(|p| {
                        let from = if p.source.kind == SourceKind::User {
                            "a person".to_string()
                        } else {
                            format!("its submittal {}", p.source.doc.as_deref().unwrap_or("")).trim().to_string()
                        };
                        (p.text().trim().to_string(), from)
                    });
                let devid = super::bimlook::made_tag(e, "devid").map(str::to_string);
                controls.entry(key).or_default().push(Control { mark, at, dims, handle: e.handle, devid, own });
            }
            continue;
        }
        let dimming = text("dimming");
        // A fixture is what its submittal says dims (or doesn't); a switch or a box is neither.
        if dimming.is_none() && text("submittal").is_none() {
            continue;
        }
        kept += 1;
        let level = super::bspec::made_level(e).unwrap_or_default();
        let circuit = text("circuit").unwrap_or_default();
        let Some(leg) = text("switch_leg") else {
            if !circuit.is_empty() {
                *no_leg.entry((level, circuit)).or_default() += 1;
            }
            continue;
        };
        let room = text("room").unwrap_or_default();
        let l = legs.entry((level, room, circuit, leg)).or_default();
        *l.fixtures.entry(mark.clone()).or_default() += 1;
        match dimming {
            Some(control) => {
                if control == "0-10V"
                    && let Some(c) = super::bspec::centre(d, e)
                {
                    l.pair.push((e.handle.hex(), c));
                }
                let marks = l.dimming.entry(control).or_default();
                if !marks.contains(&mark) {
                    marks.push(mark);
                }
            }
            None => {
                l.unknown += 1;
                if !l.unknown_marks.contains(&mark) {
                    l.unknown_marks.push(mark.clone());
                }
                *unknown.entry(mark).or_default() += 1;
            }
        }
    }
    Legs { legs, unknown, no_leg, controls }
}

/// What a dimmer dims, from the luminaires on its leg (their submittals).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DimmerProtocol {
    pub handle: Handle,
    /// Its spec tag (`devid:`), when the build linked one.
    pub devid: Option<String>,
    pub level: String,
    pub room: String,
    pub leg: String,
    pub mark: String,
    /// `0-10V`, `phase`, `DALI`… (the way its leg's fixtures dim, or its own product or a person
    /// says), `mixed` (its leg's fixtures dim two ways: a question), `non-dim` (none of them
    /// dims) or `unknown` (nothing says).
    pub protocol: String,
    /// Why, in words (`its leg's fixtures: 0-10V Q7A; unknown Q9`).
    pub why: String,
}

impl DimmerProtocol {
    /// A protocol to buy the dimmer by (not a question).
    pub(crate) fn settled(&self) -> bool {
        !matches!(self.protocol.as_str(), "mixed" | "non-dim" | "unknown")
    }
}

/// Each dimmer and the way it dims: its own product's or a person's `dimming`, else what the
/// fixtures on its leg in its room dim by (non-dim ones just switch).
pub(crate) fn dimmer_protocols(d: &Drawing) -> Vec<DimmerProtocol> {
    let Legs { legs, controls, .. } = read_legs(d);
    // (level, room, leg) → that leg's fixtures on any circuit.
    let mut by_leg: BTreeMap<(&str, &str, &str), Vec<&Leg>> = BTreeMap::new();
    for ((level, room, _, leg), l) in &legs {
        by_leg.entry((level.as_str(), room.as_str(), leg.as_str())).or_default().push(l);
    }
    let mut out = Vec::new();
    for ((level, room, leg), ctl) in &controls {
        for c in ctl.iter().filter(|c| c.dims == Some(true)) {
            if out.len() >= MAX_DIMMERS {
                return out;
            }
            let (protocol, why) = match &c.own {
                Some((p, from)) => (p.clone(), format!("{from} says {p}")),
                None if room.is_empty() => ("unknown".to_string(), "in no traced room: the fixtures on its leg are not known".to_string()),
                None => leg_protocol(by_leg.get(&(level.as_str(), room.as_str(), leg.as_str())).map_or(&[][..], Vec::as_slice)),
            };
            out.push(DimmerProtocol {
                handle: c.handle,
                devid: c.devid.clone(),
                level: level.clone(),
                room: room.clone(),
                leg: leg.clone(),
                mark: c.mark.clone(),
                protocol,
                why,
            });
        }
    }
    out
}

/// The way one leg's fixtures dim, and why in words: the one control their products use
/// (non-dim fixtures just switch), `mixed` for two, `non-dim` when none dims, `unknown` when
/// none says.
fn leg_protocol(legs: &[&Leg]) -> (String, String) {
    let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut unknown: Vec<&str> = Vec::new();
    for l in legs {
        for (control, marks) in &l.dimming {
            let e = by.entry(control.as_str()).or_default();
            for m in marks {
                if !e.contains(&m.as_str()) {
                    e.push(m);
                }
            }
        }
        for m in &l.unknown_marks {
            if !unknown.contains(&m.as_str()) {
                unknown.push(m);
            }
        }
    }
    let mut said: Vec<String> = by.iter().map(|(c, marks)| format!("{c} {}", marks.join(", "))).collect();
    if !unknown.is_empty() {
        said.push(format!("unknown {}", unknown.join(", ")));
    }
    let why = if said.is_empty() { "no fixture on its leg".to_string() } else { format!("its leg's fixtures: {}", said.join("; ")) };
    let dims: Vec<&str> = by.keys().copied().filter(|c| *c != "non-dim").collect();
    let protocol = match dims.as_slice() {
        [one] => (*one).to_string(),
        [] if by.contains_key("non-dim") => "non-dim".to_string(),
        [] => "unknown".to_string(),
        _ => "mixed".to_string(),
    };
    (protocol, why)
}

fn run(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let dimmers = dimmer_protocols(d);
    let Legs { legs, unknown, no_leg, controls } = read_legs(d);
    let pairs = zero_ten_pairs(s);
    let pair_ft: BTreeMap<(&str, &str, &str, &str), f64> =
        pairs.iter().map(|p| ((p.level.as_str(), p.room.as_str(), p.circuit.as_str(), p.leg.as_str()), p.feet)).collect();
    let mut findings = Vec::new();
    let mut mixed = 0usize;
    let mut listed = Vec::new();
    // Legs whose dimmer controls 0-10V fixtures (by what it controls), and 0-10V fixtures on legs
    // that are only switched.
    let mut dimmer_legs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut switched: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ((level, room, circuit, leg), l) in &legs {
        // Phase and 0-10V on one leg: no one dimmer dims both (non-dim fixtures just switch).
        let dims: Vec<&String> = l.dimming.keys().filter(|c| c.as_str() != "non-dim").collect();
        let verdict = match (dims.len(), l.dimming.len(), l.unknown) {
            (n, _, _) if n > 1 => "mixed",
            (1, _, _) => dims[0].as_str(),
            (0, 0, _) => "unknown",
            _ => "non-dim",
        };
        if verdict == "mixed" {
            mixed += 1;
            let what: Vec<String> = l.dimming.iter().map(|(c, marks)| format!("{c} ({})", marks.join(", "))).collect();
            findings.push(format!(
                "{level}{} circuit {circuit} switch leg {leg}: {} on one control. One dimmer can't dim both: a phase dimmer doesn't drive a 0-10V driver, and a 0-10V control doesn't cut the phase",
                if room.is_empty() { String::new() } else { format!(" room {room}") },
                what.join(" and ")
            ));
        }
        let ctl: &[Control] =
            if room.is_empty() { &[] } else { controls.get(&(level.clone(), room.clone(), leg.clone())).map_or(&[][..], Vec::as_slice) };
        let zero_ten: Vec<String> = l.dimming.get("0-10V").cloned().unwrap_or_default();
        if !zero_ten.is_empty() {
            let place = format!("{level}{} leg {leg}", if room.is_empty() { String::new() } else { format!(" room {room}") });
            let fixtures = zero_ten.join(", ");
            if ctl.iter().any(|c| c.dims == Some(true)) {
                // The dimmer's kind isn't on the plans (a designation, "D"): it has to be a
                // 0-10V one for these fixtures.
                let dimmers: Vec<&str> = ctl.iter().filter(|c| c.dims == Some(true)).map(|c| c.mark.as_str()).collect();
                let key = format!("{} controls 0-10V {fixtures}", dimmers.join("/"));
                dimmer_legs.entry(key).or_default().push(place);
            } else if !ctl.is_empty() && ctl.iter().all(|c| c.dims == Some(false)) {
                switched.entry(fixtures).or_default().push(place);
            }
        }
        if listed.len() < MAX_LISTED {
            let mut row = json!({
                "level": level,
                "room": room,
                "circuit": circuit,
                "leg": leg,
                "fixtures": l.fixtures,
                "dimming": l.dimming,
                "verdict": verdict,
                "pair_fixtures": l.pair.len(),
                "controls": ctl.iter().map(|c| c.mark.as_str()).collect::<Vec<_>>(),
            });
            if let Some(ft) = pair_ft.get(&(level.as_str(), room.as_str(), circuit.as_str(), leg.as_str())) {
                row["pair_ft"] = json!(ft);
            }
            listed.push(row);
        }
    }
    let listed_places = |places: &[String]| {
        let shown: Vec<&str> = places.iter().take(6).map(String::as_str).collect();
        if places.len() > shown.len() { format!("{} and {} more", shown.join(", "), places.len() - shown.len()) } else { shown.join(", ") }
    };
    for (what, places) in &dimmer_legs {
        findings.push(format!(
            "{} switch legs: dimmer {what} fixtures ({}). It has to be a 0-10V dimmer with the 2-wire pair to them, not a phase (LE/TE) dimmer: check the lighting controls schedule",
            places.len(),
            listed_places(places)
        ));
    }
    for (fixtures, places) in &switched {
        findings.push(format!(
            "0-10V fixtures {fixtures} on {} legs with no dimmer, only switches ({}): they run at full output and need no control pair",
            places.len(),
            listed_places(places)
        ));
    }
    let pair_legs = pairs.len();
    let pair_fixtures: usize = pairs.iter().map(|p| p.handles.len()).sum();
    let pair_feet = (pairs.iter().map(|p| p.feet).sum::<f64>() * 10.0).round() / 10.0;
    if pair_legs > 0 {
        findings.push(format!(
            "{pair_fixtures} 0-10V fixtures on {pair_legs} switch legs each need the 2-wire 0-10V control pair from their leg's dimmer or sensor, besides their power: the takeoff carries {pair_feet} ft (each leg's fixtures chained to its dimmer or sensor where the plans place one, one over {MAX_STEP_FT} ft from the rest on a run of its own, plus {CONTROL_ALLOWANCE_FT} ft a run that reaches no control)"
        ));
    }
    // The dimmers by how they dim, and those whose leg doesn't settle it.
    let mut by_protocol: BTreeMap<&str, usize> = BTreeMap::new();
    let mut open: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
    for x in &dimmers {
        *by_protocol.entry(x.protocol.as_str()).or_default() += 1;
        if !x.settled() {
            let place = format!("{}{} leg {}", x.level, if x.room.is_empty() { String::new() } else { format!(" room {}", x.room) }, x.leg);
            open.entry((x.protocol.as_str(), x.mark.as_str())).or_default().push(place);
        }
    }
    for ((protocol, mark), places) in &open {
        let what = match *protocol {
            "mixed" => "their legs' fixtures dim two ways, so no one dimmer fits: which dimmer, or which fixtures?",
            "non-dim" => "none of their legs' fixtures dims: is it a dimmer?",
            _ => "no fixture on their legs says how it dims (its submittal gives no dimming): which dimmer?",
        };
        findings.push(format!("{} dimmers {mark} {protocol}: {what} ({})", places.len(), listed_places(places)));
    }
    // Each level: its pair legs, fixtures and feet, and its dimmers by protocol.
    let mut by_level: BTreeMap<&str, Value> = BTreeMap::new();
    for p in &pairs {
        let v = by_level.entry(p.level.as_str()).or_insert_with(|| json!({"pair_legs": 0, "pair_fixtures": 0, "pair_ft": 0.0, "dimmers": {}}));
        v["pair_legs"] = json!(v["pair_legs"].as_u64().unwrap_or(0) + 1);
        v["pair_fixtures"] = json!(v["pair_fixtures"].as_u64().unwrap_or(0) + p.handles.len() as u64);
        v["pair_ft"] = json!(((v["pair_ft"].as_f64().unwrap_or(0.0) + p.feet) * 10.0).round() / 10.0);
    }
    for x in &dimmers {
        let v = by_level.entry(x.level.as_str()).or_insert_with(|| json!({"pair_legs": 0, "pair_fixtures": 0, "pair_ft": 0.0, "dimmers": {}}));
        let n = v["dimmers"][x.protocol.as_str()].as_u64().unwrap_or(0);
        v["dimmers"][x.protocol.as_str()] = json!(n + 1);
    }
    let dimmer_list: Vec<Value> = dimmers
        .iter()
        .take(MAX_LISTED)
        .map(|x| json!({"level": x.level, "room": x.room, "leg": x.leg, "mark": x.mark, "protocol": x.protocol, "why": x.why}))
        .collect();
    let no_leg_fixtures: usize = no_leg.values().sum();
    Ok(json!({
        "legs": listed,
        "mixed": mixed,
        "pairs": {"legs": pair_legs, "fixtures": pair_fixtures, "feet": pair_feet},
        "dimmers": by_protocol,
        "dimmer_list": dimmer_list,
        "by_level": by_level,
        "unknown": unknown,
        "no_leg": {"circuits": no_leg.len(), "fixtures": no_leg_fixtures},
        "findings": findings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::props::{Prop, Props, Source, SourceKind};
    use cadaddy_doc::{Common, EntityKind, Handle, Space};
    use cadaddy_geom::Vec2;

    /// A fixture element as the build and `bim.submittals` leave it.
    fn fixture(s: &mut Session, mark: &str, circuit: &str, leg: &str, dimming: Option<&str>, at: f64) {
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(at, 0.0), Vec2::new(at + 4.0, 0.0), 2.0, 100.0, 104.0);
        let tags = super::super::wallgen::tag(cadaddy_walk3d::Part::Device, "Level 1", Handle(0));
        let mut store = Props::default();
        let print = Source::print("E-1", None);
        store.set("type_mark", Prop::new(mark, print.clone()));
        store.set("room", Prop::new("101", Source::computed()));
        store.set("circuit", Prop::new(circuit, print.clone()));
        if !leg.is_empty() {
            store.set("switch_leg", Prop::new(leg, print.clone()));
        }
        let sub = Source::from_doc(SourceKind::Submittal, "lighting p.3");
        store.set("submittal", Prop::new("lighting p.3", sub.clone()));
        if let Some(c) = dimming {
            store.set("dimming", Prop::new(c, sub));
        }
        let xdata = props::write(&tags, &store);
        let d = s.doc_mut().unwrap();
        d.add(&Space::Model, Common { layer: "3D-Devices".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap();
    }

    #[test]
    fn a_phase_and_a_0_10v_fixture_on_one_leg_is_a_finding() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        fixture(&mut s, "Q7", "A-1", "a", Some("phase"), 0.0);
        fixture(&mut s, "Q7", "A-1", "a", Some("phase"), 10.0);
        fixture(&mut s, "Q7A", "A-1", "a", Some("0-10V"), 20.0);
        // Another leg, all 0-10V: no mix, but its fixtures need the pair.
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 30.0);
        // An exit sign that doesn't dim shares a leg with phase fixtures: fine.
        fixture(&mut s, "Q8", "A-3", "c", Some("non-dim"), 40.0);
        fixture(&mut s, "Q7", "A-3", "c", Some("phase"), 50.0);
        // No leg printed: counted, not judged.
        fixture(&mut s, "Q7", "A-5", "", Some("phase"), 60.0);
        let r = s.execute(CMD, &json!({})).unwrap();
        assert_eq!(r["mixed"], 1, "{r}");
        let legs = r["legs"].as_array().unwrap();
        let leg = |c: &str, l: &str| legs.iter().find(|x| x["circuit"] == c && x["leg"] == l).cloned().unwrap_or_default();
        assert_eq!(leg("A-1", "a")["verdict"], "mixed");
        assert_eq!(leg("A-1", "b")["verdict"], "0-10V");
        assert_eq!(leg("A-3", "c")["verdict"], "phase", "a non-dim fixture just switches");
        assert_eq!(r["pairs"]["legs"], 2);
        assert_eq!(r["pairs"]["fixtures"], 2);
        // One fixture a leg: no chain, the allowance to the control each.
        assert_eq!(r["pairs"]["feet"], 2.0 * CONTROL_ALLOWANCE_FT);
        assert_eq!(r["no_leg"], json!({"circuits": 1, "fixtures": 1}));
        let findings: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(findings.iter().any(|f| f.contains("switch leg a") && f.contains("phase (Q7)") && f.contains("0-10V (Q7A)")), "{findings:?}");
        assert!(findings.iter().any(|f| f.starts_with("2 0-10V fixtures on 2 switch legs")), "{findings:?}");
    }

    /// A switch or sensor element (`mark`) on leg `leg`, as the build makes them.
    fn control(s: &mut Session, mark: &str, leg: &str, at: f64) {
        control_as(s, mark, "", leg, at, "");
    }

    /// A switch or sensor element (`mark`, its legend item `item`) on leg `leg`, linked to spec
    /// tag `devid` when one is given.
    fn control_as(s: &mut Session, mark: &str, item: &str, leg: &str, at: f64, devid: &str) -> Handle {
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(at, 0.0), Vec2::new(at + 4.0, 0.0), 2.0, 46.0, 50.0);
        let mut tags = super::super::wallgen::tag(cadaddy_walk3d::Part::Device, "Level 1", Handle(0));
        if !devid.is_empty() {
            tags.push(cadaddy_doc::RawTag { code: 1000, value: format!("devid:{devid}") });
        }
        let mut store = Props::default();
        let print = Source::print("E-1", None);
        store.set("type_mark", Prop::new(mark, print.clone()));
        if !item.is_empty() {
            store.set("type", Prop::new(item, print.clone()));
        }
        store.set("room", Prop::new("101", Source::computed()));
        store.set("switch_leg", Prop::new(leg, print));
        let xdata = props::write(&tags, &store);
        let d = s.doc_mut().unwrap();
        d.add(&Space::Model, Common { layer: "3D-Devices".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap()
    }

    fn protocol_of(s: &Session, leg: &str) -> Vec<String> {
        let mut v: Vec<String> = dimmer_protocols(s.doc().unwrap()).into_iter().filter(|x| x.leg == leg).map(|x| x.protocol).collect();
        v.sort();
        v
    }

    #[test]
    fn a_dimmer_dims_the_way_its_legs_fixtures_do() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        // Leg a: phase fixtures and a non-dim one, a dimmer by its designation.
        fixture(&mut s, "Q7", "A-1", "a", Some("phase"), 0.0);
        fixture(&mut s, "Q8", "A-1", "a", Some("non-dim"), 20.0);
        control(&mut s, "SD", "a", 40.0);
        // Leg b: a 0-10V fixture on one circuit and another on a second, plus one whose product
        // says nothing; a dimmer known by its legend's words only, beside a 3-way switch.
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 100.0);
        fixture(&mut s, "Q7A", "A-3", "b", Some("0-10V"), 120.0);
        fixture(&mut s, "Q9", "A-1", "b", None, 140.0);
        control_as(&mut s, "XD", "switch, dimmer", "b", 160.0, "");
        control(&mut s, "S3", "b", 170.0);
        // Leg c: phase and 0-10V together: a question.
        fixture(&mut s, "Q7", "A-1", "c", Some("phase"), 200.0);
        fixture(&mut s, "Q7A", "A-1", "c", Some("0-10V"), 220.0);
        control(&mut s, "SD", "c", 240.0);
        // Leg d: no fixture on it; leg e: only non-dim ones.
        control(&mut s, "SD", "d", 300.0);
        fixture(&mut s, "Q8", "A-1", "e", Some("non-dim"), 400.0);
        control(&mut s, "SD", "e", 420.0);
        assert_eq!(protocol_of(&s, "a"), ["phase"], "non-dim fixtures just switch");
        assert_eq!(protocol_of(&s, "b"), ["0-10V"], "both circuits' fixtures; the 3-way switch is no dimmer");
        assert_eq!(protocol_of(&s, "c"), ["mixed"]);
        assert_eq!(protocol_of(&s, "d"), ["unknown"]);
        assert_eq!(protocol_of(&s, "e"), ["non-dim"]);
        let b = dimmer_protocols(s.doc().unwrap()).into_iter().find(|x| x.leg == "b").unwrap();
        assert_eq!(b.why, "its leg's fixtures: 0-10V Q7A; unknown Q9");
        let r = s.execute(CMD, &json!({})).unwrap();
        assert_eq!(r["dimmers"], json!({"0-10V": 1, "mixed": 1, "non-dim": 1, "phase": 1, "unknown": 1}), "{r}");
        assert_eq!(r["by_level"]["Level 1"]["dimmers"]["0-10V"], 1, "{r}");
        let findings: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(findings.iter().any(|f| f.starts_with("1 dimmers SD mixed: their legs' fixtures dim two ways")), "{findings:?}");
        assert!(findings.iter().any(|f| f.starts_with("1 dimmers SD unknown:") && f.contains("leg d")), "{findings:?}");
        assert!(!findings.iter().any(|f| f.contains("dimmers XD")), "a settled dimmer is no question: {findings:?}");
    }

    #[test]
    fn a_dimmers_own_product_or_a_persons_word_stands() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        fixture(&mut s, "Q7", "A-1", "a", Some("phase"), 0.0);
        let dimmer = control_as(&mut s, "SD", "", "a", 40.0, "");
        let d = s.doc_mut().unwrap();
        let mut store = props::read(&d.model.get(dimmer).unwrap().common.xdata);
        store.set("dimming", Prop::new("ELV", Source::user()));
        d.modify_entity(dimmer, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        let x = dimmer_protocols(s.doc().unwrap());
        assert_eq!((x[0].protocol.as_str(), x[0].why.as_str()), ("ELV", "a person says ELV"));
        // One worked out from its leg before is worked out again.
        let d = s.doc_mut().unwrap();
        store.set("dimming", Prop::new("0-10V", Source::computed()));
        d.modify_entity(dimmer, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        assert_eq!(protocol_of(&s, "a"), ["phase"]);
    }

    #[test]
    fn the_takeoff_buys_dimmers_by_protocol_and_reports_each_level() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let tag = |id: &str| json!({"id": id, "kind": "switch", "level": "Level 1", "at_ft": [1, 1], "fields": {"device_type": {"value": "switch, dimmer", "source": "legend"}}});
        let spec = json!({
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12}],
            "tags": [tag("D1"), tag("D2"), tag("D3"), tag("D4"),
                {"id": "S1", "kind": "switch", "level": "Level 1", "at_ft": [2, 1], "fields": {"device_type": {"value": "switch, 3-way", "source": "legend"}}}]
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        // Two 0-10V legs, a phase leg, a mixed one.
        fixture(&mut s, "Q7A", "A-1", "a", Some("0-10V"), 0.0);
        control_as(&mut s, "SD", "switch, dimmer", "a", 40.0, "D1");
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 100.0);
        control_as(&mut s, "SD", "switch, dimmer", "b", 140.0, "D2");
        fixture(&mut s, "Q7", "A-1", "c", Some("phase"), 200.0);
        control_as(&mut s, "SD", "switch, dimmer", "c", 240.0, "D3");
        fixture(&mut s, "Q7", "A-1", "e", Some("phase"), 300.0);
        fixture(&mut s, "Q7A", "A-1", "e", Some("0-10V"), 320.0);
        control_as(&mut s, "SD", "switch, dimmer", "e", 340.0, "D4");
        control_as(&mut s, "S3", "switch, 3-way", "e", 350.0, "S1");
        let bom = s.execute("bim.bom", &json!({})).unwrap();
        let net =
            |item: &str| bom["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!(item)).map(|l| l["net"].clone()).unwrap_or_default();
        assert_eq!(net("switch, dimmer, 0-10V"), json!(2.0), "{bom}");
        assert_eq!(net("switch, dimmer, phase"), json!(1.0));
        assert_eq!(net("switch, dimmer, protocol a question (its leg's fixtures dim two ways)"), json!(1.0));
        assert_eq!(net("switch, 3-way"), json!(1.0), "a switch that doesn't dim keeps its item");
        assert_eq!(net("switch, dimmer"), Value::Null, "no dimmer left unsplit");
        let r = s.execute(CMD, &json!({})).unwrap();
        let l1 = &r["by_level"]["Level 1"];
        assert_eq!((l1["pair_legs"].clone(), l1["pair_fixtures"].clone()), (json!(3), json!(3)), "{r}");
        assert_eq!(l1["dimmers"], json!({"0-10V": 2, "mixed": 1, "phase": 1}));
        let a = r["legs"].as_array().unwrap().iter().find(|l| l["leg"] == "a").cloned().unwrap_or_default();
        // The fixture to its dimmer 40" on (the spec's 12 units a foot).
        assert_eq!(a["pair_ft"], json!(3.3), "{a}");
    }

    #[test]
    fn a_dimmer_on_a_0_10v_leg_is_a_question_and_a_switched_leg_needs_no_pair() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        // Leg b: two 0-10V fixtures 10' apart and a dimmer 10' on.
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 0.0);
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 120.0);
        control(&mut s, "SD", "b", 240.0);
        // Leg c: a 0-10V fixture on a plain switch.
        fixture(&mut s, "Q7A", "A-1", "c", Some("0-10V"), 600.0);
        control(&mut s, "S", "c", 650.0);
        let pairs = zero_ten_pairs(&s);
        assert_eq!(pairs.len(), 1, "a switched leg needs no pair");
        assert_eq!(pairs[0].feet, 20.0, "chained to the dimmer the plans place, no allowance");
        let r = s.execute(CMD, &json!({})).unwrap();
        let findings: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(findings.iter().any(|f| f.starts_with("1 switch legs: dimmer SD controls 0-10V Q7A")), "{findings:?}");
        assert!(findings.iter().any(|f| f.starts_with("0-10V fixtures Q7A on 1 legs with no dimmer")), "{findings:?}");
        let legs = r["legs"].as_array().unwrap();
        assert!(legs.iter().any(|l| l["leg"] == "b" && l["controls"] == json!(["SD"])), "{r}");
    }

    #[test]
    fn a_legs_0_10v_fixtures_are_chained_to_their_control() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        // Three 0-10V fixtures 10' apart in a row (drawing inches) and a phase one on the leg.
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 0.0);
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 120.0);
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 240.0);
        fixture(&mut s, "Q7", "A-1", "b", Some("phase"), 360.0);
        let pairs = zero_ten_pairs(&s);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].handles.len(), 3, "the phase fixture needs no pair");
        assert_eq!(pairs[0].feet, 20.0 + CONTROL_ALLOWANCE_FT);
        // A fixture of the leg 50' on (another room's, stamped from the same template): its own
        // run to a control, not a pair run between the rooms.
        fixture(&mut s, "Q7A", "A-1", "b", Some("0-10V"), 840.0);
        let pairs = zero_ten_pairs(&s);
        assert_eq!(pairs[0].feet, 20.0 + 2.0 * CONTROL_ALLOWANCE_FT);
        let bom = s.execute("bim.bom", &json!({})).unwrap();
        let line =
            bom["lines"].as_array().unwrap().iter().find(|l| l["item"].as_str().is_some_and(|i| i.starts_with("0-10V"))).cloned().unwrap_or_default();
        assert_eq!(line["net"], 20.0 + 2.0 * CONTROL_ALLOWANCE_FT, "the takeoff carries the pair: {line}");
    }
}
