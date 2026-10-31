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

use std::collections::BTreeMap;

use cadaddy_doc::props;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.dimming";
/// Fixtures kept.
const MAX_ELEMENTS: usize = 500_000;
/// Legs listed in a report.
const MAX_LISTED: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Dimming Check", run)
            .params("{} → {legs: [{level, room, circuit, leg, fixtures: {mark: n}, dimming: {control: [mark]}, verdict: mixed|0-10V|phase|DALI|non-dim|unknown, pair_fixtures}], mixed, pairs: {legs, fixtures, feet}, unknown: {mark: n}, no_leg: {circuits, fixtures}, findings: [text]}: the fixtures on each switch leg (circuit and leg letter printed with them, in one room) and how their submitted products dim; a leg with phase and 0-10V fixtures is a finding, and each leg with 0-10V fixtures needs a 2-wire control pair to them (bim.bom carries it)")
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
    /// Fixtures whose product says nothing of its dimming.
    unknown: usize,
    /// Its 0-10V fixtures: handle and where (drawing units).
    pair: Vec<(String, cadaddy_geom::Vec2)>,
}

/// The fixtures by switch leg, (level, room, circuit, leg); fixtures of unknown dimming by mark;
/// fixtures printed with a circuit and no leg, by (level, circuit); the switches and sensors by
/// the leg they control, (level, room, leg): their marks and where they are.
struct Legs {
    legs: BTreeMap<(String, String, String, String), Leg>,
    unknown: BTreeMap<String, usize>,
    no_leg: BTreeMap<(String, String), usize>,
    controls: BTreeMap<(String, String, String), Vec<(String, cadaddy_geom::Vec2)>>,
}

/// Whether a control dims: a dimmer (`SD`) does, a plain switch (`S`, `S3`…) doesn't; a sensor
/// or anything else: its product says (unknown here).
fn control_dims(mark: &str) -> Option<bool> {
    match mark.trim().to_ascii_uppercase().as_str() {
        "SD" | "DS" => Some(true),
        "S" | "S2" | "S3" | "S4" | "SP" | "SM" | "SH" | "SK" | "SOR" | "SWT" => Some(false),
        _ => None,
    }
}

/// A switch leg's 0-10V fixtures and the control pair they need.
pub(crate) struct PairLeg {
    pub level: String,
    pub circuit: String,
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
            let ctl = if room.is_empty() { &[][..] } else { controls.get(&(level.clone(), room, leg)).map_or(&[][..], Vec::as_slice) };
            if !ctl.is_empty() && ctl.iter().all(|(m, _)| control_dims(m) == Some(false)) {
                return None;
            }
            let mut pts: Vec<cadaddy_geom::Vec2> = l.pair.iter().map(|(_, p)| *p).collect();
            // From the dimming control the plans place (the first run reaches it).
            let placed = ctl.iter().find(|(m, _)| control_dims(m) != Some(false)).map(|(_, p)| *p);
            pts.extend(placed);
            let (chained, runs) = chain_length(&pts, MAX_STEP_FT * upf);
            let unreached = if placed.is_some() { runs.saturating_sub(1) } else { runs };
            let feet = chained / upf + CONTROL_ALLOWANCE_FT * unreached as f64;
            Some(PairLeg { level, circuit, handles: l.pair.into_iter().map(|(h, _)| h).collect(), feet: (feet * 10.0).round() / 10.0 })
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

fn read_legs(d: &cadaddy_doc::Drawing) -> Legs {
    // (level, room, circuit, leg) → the leg's fixtures. A leg is a room's: a typical room's
    // fixtures all carry the template's circuit and leg, each room its own.
    let mut legs: BTreeMap<(String, String, String, String), Leg> = BTreeMap::new();
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_leg: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut controls: BTreeMap<(String, String, String), Vec<(String, cadaddy_geom::Vec2)>> = BTreeMap::new();
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
        // A switch or sensor: the leg it controls.
        if matches!(super::propfill::kind_of(&mark), "switch" | "occupancy sensor") {
            if let (Some(leg), Some(c)) = (text("switch_leg"), super::bspec::centre(d, e)) {
                kept += 1;
                let key = (super::bspec::made_level(e).unwrap_or_default(), text("room").unwrap_or_default(), leg);
                controls.entry(key).or_default().push((mark, c));
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
                *unknown.entry(mark).or_default() += 1;
            }
        }
    }
    Legs { legs, unknown, no_leg, controls }
}

fn run(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let Legs { legs, unknown, no_leg, controls } = read_legs(d);
    let pairs = zero_ten_pairs(s);
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
        let ctl: Vec<String> = if room.is_empty() {
            Vec::new()
        } else {
            controls.get(&(level.clone(), room.clone(), leg.clone())).into_iter().flatten().map(|(m, _)| m.clone()).collect()
        };
        let zero_ten: Vec<String> = l.dimming.get("0-10V").cloned().unwrap_or_default();
        if !zero_ten.is_empty() {
            let place = format!("{level}{} leg {leg}", if room.is_empty() { String::new() } else { format!(" room {room}") });
            let fixtures = zero_ten.join(", ");
            if ctl.iter().any(|m| control_dims(m) == Some(true)) {
                // The dimmer's kind isn't on the plans (a designation, "D"): it has to be a
                // 0-10V one for these fixtures.
                let dimmers: Vec<&String> = ctl.iter().filter(|m| control_dims(m) == Some(true)).collect();
                let key = format!("{} controls 0-10V {fixtures}", dimmers.iter().map(|m| m.as_str()).collect::<Vec<_>>().join("/"));
                dimmer_legs.entry(key).or_default().push(place);
            } else if !ctl.is_empty() && ctl.iter().all(|m| control_dims(m) == Some(false)) {
                switched.entry(fixtures).or_default().push(place);
            }
        }
        if listed.len() < MAX_LISTED {
            listed.push(json!({
                "level": level,
                "room": room,
                "circuit": circuit,
                "leg": leg,
                "fixtures": l.fixtures,
                "dimming": l.dimming,
                "verdict": verdict,
                "pair_fixtures": l.pair.len(),
                "controls": ctl,
            }));
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
    let no_leg_fixtures: usize = no_leg.values().sum();
    Ok(json!({
        "legs": listed,
        "mixed": mixed,
        "pairs": {"legs": pair_legs, "fixtures": pair_fixtures, "feet": pair_feet},
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
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(at, 0.0), Vec2::new(at + 4.0, 0.0), 2.0, 46.0, 50.0);
        let tags = super::super::wallgen::tag(cadaddy_walk3d::Part::Device, "Level 1", Handle(0));
        let mut store = Props::default();
        let print = Source::print("E-1", None);
        store.set("type_mark", Prop::new(mark, print.clone()));
        store.set("room", Prop::new("101", Source::computed()));
        store.set("switch_leg", Prop::new(leg, print));
        let xdata = props::write(&tags, &store);
        let d = s.doc_mut().unwrap();
        d.add(&Space::Model, Common { layer: "3D-Devices".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap();
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
