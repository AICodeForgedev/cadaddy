//! The dimming check (`bim.dimming`): what each switch leg dims.
//!
//! The submitted products say how each fixture dims (`dimming`: 0-10V, phase, DALI, non-dim;
//! `bim.submittals`). The plans give each fixture's circuit and switch leg (`A-1,a`: circuit A-1,
//! leg a). The fixtures on one leg share one control:
//!
//! - **mixed**: a phase fixture and a 0-10V fixture on one leg. One dimmer can't dim both: a
//!   phase dimmer doesn't drive the 0-10V driver, and a 0-10V control doesn't cut the phase.
//! - **0-10V pair**: every 0-10V fixture on a leg needs the 2-wire control pair from the leg's
//!   dimmer or sensor, besides its power. The takeoff has to carry it.

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
            .params("{} → {legs: [{level, circuit, leg, fixtures: {mark: n}, dimming: {control: [mark]}, verdict: mixed|0-10V|phase|DALI|non-dim|unknown, pair_fixtures}], mixed, pairs: {legs, fixtures}, unknown: {mark: n}, no_leg: {circuits, fixtures}, findings: [text]}: the fixtures on each switch leg (circuit and leg letter printed with them) and how their submitted products dim; a leg with phase and 0-10V fixtures is a finding, and each leg with 0-10V fixtures needs a 2-wire control pair to them")
            .menu(&["Tools", "Building", "Dimming Check"])
            .noundo(),
    ]
}

/// One switch leg: the fixtures it controls.
#[derive(Default)]
struct Leg {
    fixtures: BTreeMap<String, usize>,
    /// Each control the fixtures' products use, and the marks that use it.
    dimming: BTreeMap<String, Vec<String>>,
    /// Fixtures whose product says nothing of its dimming.
    unknown: usize,
    pair_fixtures: usize,
}

fn run(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    // (level, circuit, leg) → the leg's fixtures.
    let mut legs: BTreeMap<(String, String, String), Leg> = BTreeMap::new();
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_leg: BTreeMap<(String, String), usize> = BTreeMap::new();
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
        let l = legs.entry((level, circuit, leg)).or_default();
        *l.fixtures.entry(mark.clone()).or_default() += 1;
        match dimming {
            Some(control) => {
                if control == "0-10V" {
                    l.pair_fixtures += 1;
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

    let mut findings = Vec::new();
    let mut mixed = 0usize;
    let (mut pair_legs, mut pair_fixtures) = (0usize, 0usize);
    let mut listed = Vec::new();
    for ((level, circuit, leg), l) in &legs {
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
                "{level} circuit {circuit} switch leg {leg}: {} on one control. One dimmer can't dim both: a phase dimmer doesn't drive a 0-10V driver, and a 0-10V control doesn't cut the phase",
                what.join(" and ")
            ));
        }
        if l.pair_fixtures > 0 {
            pair_legs += 1;
            pair_fixtures += l.pair_fixtures;
        }
        if listed.len() < MAX_LISTED {
            listed.push(json!({
                "level": level,
                "circuit": circuit,
                "leg": leg,
                "fixtures": l.fixtures,
                "dimming": l.dimming,
                "verdict": verdict,
                "pair_fixtures": l.pair_fixtures,
            }));
        }
    }
    if pair_legs > 0 {
        findings.push(format!(
            "{pair_fixtures} 0-10V fixtures on {pair_legs} switch legs each need the 2-wire 0-10V control pair from their leg's dimmer or sensor, besides their power: the takeoff doesn't carry it yet"
        ));
    }
    let no_leg_fixtures: usize = no_leg.values().sum();
    Ok(json!({
        "legs": listed,
        "mixed": mixed,
        "pairs": {"legs": pair_legs, "fixtures": pair_fixtures},
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
        assert_eq!(r["pairs"], json!({"legs": 2, "fixtures": 2}));
        assert_eq!(r["no_leg"], json!({"circuits": 1, "fixtures": 1}));
        let findings: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(findings.iter().any(|f| f.contains("switch leg a") && f.contains("phase (Q7)") && f.contains("0-10V (Q7A)")), "{findings:?}");
        assert!(findings.iter().any(|f| f.starts_with("2 0-10V fixtures on 2 switch legs")), "{findings:?}");
    }
}
