//! Buried branch and site circuits (Q6348): the conductors and the raceway of each branch or
//! site circuit run below the slab or in the earth (a pole light's, a sign's, a charger's, a
//! site load's out of its panel).
//!
//! - [`tag_id`] / [`run_id`]: a buried circuit's runs carry its circuit in their id
//!   (`circuit:<level>:buried:<label>#<n>`), so the atoms pull its conductors and the takeoff names
//!   it.
//! - [`size`]: what the circuit pulls (the panel schedule's wire, never under its breaker's
//!   minimum, upsized for the voltage drop over its buried length; a one-pole circuit its
//!   neutral; one equipment ground for the raceway) and the smallest PVC its fill allows, never
//!   under the specification's minimum (else NEC 352.20(A)).
//! - [`assign`]: each buried circuit's runs sized so, a run a note sizes kept at the note's size
//!   unless its conductors don't fit (then corrected, the note's size beside it), and the
//!   circuit's tag (its circuit, panel, conductors, route) in the spec.
//! - [`kind_of`]: what a buried conduit run is for (feeder, branch, site, service, telecom), for
//!   the underground takeoff.
//!
//! Every value comes from the documents or from `ugcircuit.json` and the routing rules
//! (`conduitroute.json`), each with its source.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::tags::ElementTag;
use cadaddy_walk3d::{BuildingSpec, MepRun, RunCorrection};
use serde::Deserialize;
use serde_json::{Value, json};

use super::conduitroute::{self, size};

const RULES: &str = include_str!("ugcircuit.json");

/// What a buried circuit's tag id holds between its level and its label.
pub(crate) const TAG_PART: &str = ":buried:";
/// The `route` field of a buried circuit's tag.
pub(crate) const ROUTE: &str = "buried";
/// Circuits one buried label names, at most (hostile labels).
const MAX_CIRCUITS: usize = 12;
/// Runs looked at, at most.
const MAX_RUNS: usize = 200_000;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub material: String,
    pub min_in: f64,
    pub min_source: String,
    pub conductors_source: String,
    pub raceway_source: String,
    pub length_source: String,
    pub kinds: BTreeMap<String, String>,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A label as one id (`A-28, A-30` → `A-28,A-30`).
fn norm(label: &str) -> String {
    label.split([',', ' ', ';']).map(str::trim).filter(|v| !v.is_empty()).collect::<Vec<_>>().join(",").to_ascii_uppercase()
}

/// The tag id of the buried circuit(s) `label` on `level`.
pub(crate) fn tag_id(level: &str, label: &str) -> String {
    format!("{}{level}{TAG_PART}{}", super::printcond::CIRCUIT_TAG, norm(label))
}

/// The id of run `n` (its sheet and number: `E-8.3`) of the buried circuit(s) `label` on `level`.
pub(crate) fn run_id(level: &str, label: &str, n: impl std::fmt::Display) -> String {
    format!("{}#{}", tag_id(level, label), n.to_string().replace('#', ""))
}

/// Whether a tag (or run) id is a buried circuit's.
pub(crate) fn is_buried(id: &str) -> bool {
    id.starts_with(super::printcond::CIRCUIT_TAG) && id.contains(TAG_PART)
}

/// The label a buried circuit's tag or run id holds.
fn label_of(id: &str) -> Option<&str> {
    let (_, rest) = id.split_once(TAG_PART)?;
    Some(rest.split('#').next().unwrap_or(rest)).filter(|v| !v.is_empty())
}

/// One breaker a buried circuit's label names, as its panel's schedule gives it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Breaker {
    pub panel: String,
    /// Its numbers (`78`, `80` for a two-pole breaker).
    pub numbers: Vec<String>,
    pub trip: Option<f64>,
    pub poles: u32,
    /// The schedule's wire size (`10`), when it prints one.
    pub wire: Option<String>,
    pub load_va: Option<f64>,
    pub description: String,
    /// The panel's `volts/phase` as its schedule prints it.
    pub volts: Option<String>,
    /// Where it was read (`panel A schedule (E-2): circuit 78 EV CHARGER, 40/2`), or why not.
    pub source: String,
}

/// The breakers a label names: `A-40`, `A-78,80` (one two-pole breaker where the schedule says
/// so), `A-28,A-30,A-32` (three). A number the schedule doesn't list is a breaker of the label's
/// numbers in its group (`78,80`: two poles), the rules' breaker.
pub(crate) fn breakers(sp: &BuildingSpec, label: &str) -> Vec<Breaker> {
    // Each `panel-numbers` group of the label, its numbers in order.
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for t in label.split([',', ' ', ';']).map(str::trim).filter(|v| !v.is_empty()).take(4 * MAX_CIRCUITS) {
        let digits = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
        if let Some((p, n)) = t.rsplit_once('-')
            && digits(n)
            && !p.trim().is_empty()
        {
            groups.push((p.trim().to_ascii_uppercase(), vec![n.to_string()]));
        } else if digits(t)
            && let Some(g) = groups.last_mut()
        {
            g.1.push(t.to_string());
        }
    }
    let mut out: Vec<Breaker> = Vec::new();
    // The numbers each panel's breakers so far take (a label naming one twice names it once).
    let mut covered: BTreeSet<(String, u32)> = BTreeSet::new();
    for (panel, nums) in &groups {
        let sched = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(panel));
        let volts = sched.and_then(|s| s.header.iter().find(|(k, _)| k.to_ascii_lowercase().contains("volt")).map(|(_, v)| v.clone()));
        for (i, n) in nums.iter().enumerate() {
            let Ok(num) = n.parse::<u32>() else { continue };
            if covered.contains(&(panel.clone(), num)) || out.len() >= MAX_CIRCUITS {
                continue;
            }
            let row = sched.and_then(|s| s.circuits.iter().find(|c| c.number.trim() == n.as_str()));
            let printed_poles = row.and_then(|r| {
                let after = r.breaker.split_once('/').map(|(_, p)| p.trim().chars().take_while(char::is_ascii_digit).collect::<String>());
                after.and_then(|p| p.parse::<u32>().ok())
            });
            let poles = match row {
                Some(r) if r.poles > 0 => r.poles,
                Some(_) => printed_poles.unwrap_or(1),
                // Not on a schedule: the label's numbers of its group from here are one breaker.
                None => (nums.len() - i) as u32,
            }
            .clamp(1, 3);
            let mut numbers: Vec<String> = Vec::new();
            for k in 0..poles {
                let v = num.saturating_add(2 * k);
                covered.insert((panel.clone(), v));
                numbers.push(v.to_string());
            }
            let trip = row.and_then(|r| {
                let digits: String = r.breaker.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
                digits.parse::<f64>().ok().filter(|a| (5.0..=4000.0).contains(a))
            });
            let wire = row.and_then(|r| {
                let w: String = r.wire.trim().trim_start_matches('#').chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
                conduitroute::rules().vdrop.sizes.iter().any(|s| *s == w).then_some(w)
            });
            let source = match (row, sched) {
                (Some(r), Some(s)) => format!(
                    "panel {} schedule{}: circuit {} {}{}",
                    s.panel.trim(),
                    if s.source.is_empty() { String::new() } else { format!(" ({})", s.source) },
                    r.number.trim(),
                    r.description.trim(),
                    if r.breaker.trim().is_empty() { String::new() } else { format!(", {}", r.breaker.trim()) }
                ),
                (None, Some(s)) => format!("panel {} schedule lists no circuit {n}: the label's numbers taken as one breaker", s.panel.trim()),
                _ => format!("no schedule for panel {panel}: the label's numbers taken as one breaker"),
            };
            out.push(Breaker {
                panel: panel.clone(),
                numbers,
                trip,
                poles,
                wire,
                load_va: row.and_then(|r| r.load_va).filter(|v| v.is_finite() && *v > 0.0 && *v < 1.0e6),
                description: row.map(|r| r.description.trim().to_string()).unwrap_or_default(),
                volts: volts.clone(),
                source,
            });
        }
    }
    out
}

/// A buried circuit's conductors and raceway.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sized {
    pub breakers: Vec<Breaker>,
    /// `3#10 + 1#10G`.
    pub makeup: String,
    pub conductors_why: Vec<String>,
    pub size_in: f64,
    pub size_source: String,
}

/// The conductor sizes in order, smallest first.
fn order(s: &str) -> usize {
    conduitroute::rules().vdrop.sizes.iter().position(|x| x == s).unwrap_or(0)
}

/// What the buried circuit(s) `label` pull over `length_ft` of buried run, and the raceway they
/// fill, at least `min` (the specification's minimum and where it says so; none: the rules').
/// None: the label names no breaker.
pub(crate) fn size(sp: &BuildingSpec, label: &str, length_ft: f64, min: Option<(f64, String)>) -> Option<Sized> {
    let r = rules();
    let rl = conduitroute::rules();
    let brks = breakers(sp, label);
    if brks.is_empty() {
        return None;
    }
    let sizes = &rl.vdrop.sizes;
    let ft = if length_ft.is_finite() { length_ft.max(0.0) } else { 0.0 };
    // Current-carrying conductors in the raceway: each breaker's poles and a one-pole's neutral.
    let ccc: u32 = brks.iter().map(|b| b.poles + u32::from(b.poles == 1)).sum();
    let mut why: Vec<String> = Vec::new();
    let mut parts: BTreeMap<usize, u32> = BTreeMap::new();
    let mut ground: Option<String> = None;
    for b in &brks {
        let label = format!("{}-{}", b.panel, b.numbers.join(","));
        let trip = b.trip.unwrap_or(rl.vdrop.breaker_a);
        // The smallest size its breaker allows; the schedule's wire where it prints a larger.
        let smallest = sizes.first().cloned().unwrap_or_else(|| "12".into());
        let least = size::breaker_minimum(&smallest, ccc, Some(trip), rl).map_or(smallest, |(s, _)| s);
        let mut hot = least.clone();
        match &b.wire {
            Some(w) if order(w) >= order(&least) => {
                hot.clone_from(w);
                why.push(format!("{label}: #{hot}, {}", b.source));
            }
            Some(w) => why.push(format!("{label}: #{hot} for its {trip:.0} A breaker, not the schedule's #{w} ({})", b.source)),
            None => why.push(format!("{label}: #{hot} for its {trip:.0} A breaker ({})", b.source)),
        }
        if b.wire.is_none() && b.trip.is_none() {
            why.push(format!("{label}: its breaker not read: {} A taken ({})", rl.vdrop.breaker_a, rl.vdrop.source));
        }
        // The voltage drop over the buried run at the circuit's own voltage and load.
        let sup = size::supply(b.poles, b.volts.as_deref(), &b.panel, rl);
        let (amps, load) = match b.load_va {
            Some(va) => (sup.amps(va), format!("{va:.0} VA on the schedule")),
            None => (trip * rl.vdrop.load_pct / 100.0, format!("{}% of the breaker", rl.vdrop.load_pct)),
        };
        if size::drop_pct(&hot, ft, amps, &sup, rl).is_some_and(|v| v > rl.vdrop.max_pct) {
            let up = sizes.iter().skip(order(&hot) + 1).find(|s| size::drop_pct(s, ft, amps, &sup, rl).is_some_and(|v| v <= rl.vdrop.max_pct));
            let to = up.or(sizes.last()).cloned().unwrap_or_else(|| hot.clone());
            let pct = size::drop_pct(&to, ft, amps, &sup, rl).unwrap_or(0.0);
            why.push(format!(
                "{label}: #{to} for the voltage drop: {ft:.0} ft at {amps:.1} A ({load}), {}: {pct:.1}%{} ({}; {})",
                sup.source,
                if up.is_none() { ", the largest the table holds: the engineer confirms" } else { "" },
                rl.vdrop.source,
                r.length_source
            ));
            hot = to;
        }
        let n = parts.entry(order(&hot)).or_default();
        *n = n.saturating_add(b.poles + u32::from(b.poles == 1));
        // Its ground: by its breaker (250.122(A)), upsized as its phase conductors were for the
        // drop (250.122(B)), never over them.
        let egc_table = &super::onelinegear::rules().egc_table;
        let mut g = egc_table.iter().find(|(max, _)| trip <= *max + 1e-9).map(|(_, g)| g.trim_start_matches('#').to_string()).unwrap_or_else(|| hot.clone());
        let cmil = |s: &str| rl.vdrop.cmil.get(s).copied().unwrap_or(0.0);
        if cmil(&least) > 0.0 && cmil(&hot) > cmil(&least) {
            let need = cmil(&g) * cmil(&hot) / cmil(&least);
            if let Some(s) = sizes.iter().find(|s| cmil(s) + 1e-9 >= need) {
                g.clone_from(s);
            }
        }
        if order(&g) > order(&hot) {
            g.clone_from(&hot);
        }
        if ground.as_ref().is_none_or(|x| order(&g) > order(x)) {
            ground = Some(g);
        }
    }
    let mut list: Vec<(u32, String, bool)> =
        parts.iter().rev().filter_map(|(o, n)| sizes.get(*o).map(|s| (*n, s.clone(), false))).collect();
    if let Some(g) = ground {
        list.push((1, g, true));
    }
    let makeup = conduitroute::makeup_text(&list);
    let (min_in, min_src) = match min.filter(|(v, _)| v.is_finite() && *v > 0.0) {
        Some((v, s)) => (v.max(r.min_in), s),
        None => (r.min_in, r.min_source.clone()),
    };
    let material = if r.material.trim().is_empty() { "PVC" } else { r.material.trim() };
    let (size_in, size_source) = match conduitroute::fill_size(&list, material, min_in, rl) {
        Some((t, area, allowed)) => (
            t,
            format!(
                "fill: {makeup}: {:.3} sq in of conductors in {:.3} sq in allowed of {} {material} ({}; {}); not under the minimum: {min_src}",
                area,
                allowed,
                super::ugsheets::trade_size(t),
                r.raceway_source,
                rl.fill.source
            ),
        ),
        None => {
            let biggest = rl.fill.area_sq_in.get(material).and_then(|t| t.last()).map_or(4.0, |x| x[0]);
            (biggest, format!("fill: {makeup} over the largest {material} the table holds: {biggest}\"; the engineer confirms the raceway"))
        }
    };
    Some(Sized { breakers: brks, makeup, conductors_why: why, size_in, size_source })
}

/// A run's plan length plus its rise, feet.
fn run_ft(r: &MepRun, upf: f64) -> f64 {
    let flat: f64 = r
        .points
        .windows(2)
        .filter_map(|w| match w {
            [a, b] => Some((a[0] - b[0]).hypot(a[1] - b[1])),
            _ => None,
        })
        .sum::<f64>()
        / upf;
    let rise = match (r.top_ft, r.bottoms_ft.first()) {
        (Some(t), Some(b)) if t.is_finite() && b.is_finite() => (t - b).abs(),
        _ => 0.0,
    };
    if flat.is_finite() { flat + rise } else { rise }
}

/// Each buried circuit of `runs` (those whose id is a buried circuit's, [`run_id`]) sized by its
/// conductors ([`size`]), at least `min` (the specification's minimum, where it says so); its
/// tag in the spec. A run a note sizes keeps the note's size where its conductors fit; where they
/// don't it is corrected, the note's size beside it. The buried circuits' tags no run of the spec
/// or of `runs` holds any more go. One line per circuit sized.
pub(crate) fn assign(sp: &mut BuildingSpec, runs: &mut [MepRun], min: Option<(f64, String)>) -> Vec<Value> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, r) in runs.iter().enumerate().take(MAX_RUNS) {
        if is_buried(&r.id) {
            let tag = r.id.split('#').next().unwrap_or(&r.id).to_string();
            groups.entry(tag).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    let mut tags: Vec<ElementTag> = Vec::new();
    for (id, members) in &groups {
        let Some(label) = label_of(id) else { continue };
        let length: f64 = members.iter().filter_map(|i| runs.get(*i)).map(|r| run_ft(r, upf)).sum();
        let Some(sz) = size(sp, label, length, min.clone()) else { continue };
        let mut corrected = 0usize;
        for i in members {
            let Some(r) = runs.get_mut(*i) else { continue };
            let old = r.size_in.first().copied().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(sz.size_in);
            let noted = r.size_source.starts_with("note");
            if noted && old + 1e-9 >= sz.size_in {
                continue;
            }
            if noted {
                r.corrections.push(RunCorrection {
                    key: "size_in".into(),
                    design: format!("{} ({})", super::ugsheets::trade_size(old), r.size_source),
                    corrected: super::ugsheets::trade_size(sz.size_in),
                    reason: format!("the note's raceway is too small for its circuit's conductors ({})", sz.makeup),
                    rule: sz.size_source.clone(),
                });
                corrected += 1;
            }
            // The run's top stays where its cover puts it: its bottom moves by the change.
            let shift = (old - sz.size_in) / 12.0;
            r.bottoms_ft.iter_mut().filter(|b| b.is_finite()).for_each(|b| *b += shift);
            if let Some(s) = r.size_in.first_mut() {
                *s = sz.size_in;
            }
            r.size_source.clone_from(&sz.size_source);
        }
        let Some(first) = members.first().and_then(|i| runs.get(*i)) else { continue };
        let panels: BTreeSet<&str> = sz.breakers.iter().map(|b| b.panel.as_str()).collect();
        let circuit = match panels.iter().next() {
            Some(p) if panels.len() == 1 => format!("{p}-{}", sz.breakers.iter().flat_map(|b| b.numbers.iter().cloned()).collect::<Vec<_>>().join(",")),
            _ => norm(label),
        };
        let mut t = ElementTag { id: id.clone(), kind: "circuit".into(), level: first.level.clone(), ..ElementTag::default() };
        if let Some(p) = first.points.first() {
            t.at_ft = [p[0] / upf, p[1] / upf];
        }
        t.set("circuit", circuit.clone(), "computed");
        t.set("circuits", sz.breakers.iter().map(|b| format!("{}-{}", b.panel, b.numbers.join(","))).collect::<Vec<_>>().join(", "), "computed");
        t.set("panel", panels.into_iter().collect::<Vec<_>>().join(", "), "plan");
        t.set("conductors", sz.makeup.clone(), "computed");
        t.set("conductors_basis", format!("{}; {}", sz.conductors_why.join("; "), rules().conductors_source), "rule");
        t.set("raceway_in", super::ugsheets::trade_size(sz.size_in), "computed");
        t.set("raceway_basis", sz.size_source.clone(), "rule");
        t.set("route", ROUTE, "computed");
        t.set("buried_ft", format!("{length:.1}"), "computed");
        let loads: Vec<&str> = sz.breakers.iter().map(|b| b.description.as_str()).filter(|d| !d.is_empty()).collect();
        if !loads.is_empty() {
            t.set("load", loads.join(", "), "schedule");
        }
        out.push(json!({
            "circuit": circuit,
            "level": first.level,
            "runs": members.len(),
            "buried_ft": (length * 10.0).round() / 10.0,
            "conductors": sz.makeup,
            "size_in": sz.size_in,
            "size_source": sz.size_source,
            "corrected": corrected,
        }));
        tags.push(t);
    }
    // The spec's tags: each circuit's own again, those no run holds any more gone.
    let held: BTreeSet<String> = sp
        .mep
        .runs
        .iter()
        .chain(runs.iter())
        .take(2 * MAX_RUNS)
        .filter(|r| is_buried(&r.id))
        .map(|r| r.id.split('#').next().unwrap_or(&r.id).to_string())
        .collect();
    let fresh: BTreeSet<String> = tags.iter().map(|t| t.id.clone()).collect();
    sp.tags.retain(|t| !is_buried(&t.id) || (held.contains(&t.id) && !fresh.contains(&t.id)));
    sp.tags.extend(tags);
    out
}

/// Whether site work's buried runs already carry circuit `label` (`A-78`: a breaker of it).
pub(crate) fn site_carries(sp: &BuildingSpec, label: &str) -> bool {
    let want = breakers(sp, label);
    let Some(w) = want.first() else { return false };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    sp.mep.runs.iter().take(MAX_RUNS).filter(|r| r.source_sheet.starts_with(super::sitework::SOURCE_PREFIX) && is_buried(&r.id)).any(|r| {
        let Some(l) = label_of(&r.id) else { return false };
        seen.insert(l) && breakers(sp, l).iter().any(|b| b.panel.eq_ignore_ascii_case(&w.panel) && b.numbers.iter().any(|n| w.numbers.contains(n)))
    })
}

/// The specification's minimum raceway size as the job read it (`spec 26 05 33: Minimum Raceway
/// Size: 1/2-inch`), when it gives one.
pub(crate) fn spec_minimum(d: &cadaddy_doc::Drawing) -> Option<(f64, String)> {
    let m = conduitroute::Methods::stored(d);
    m.min_size_in.filter(|v| v.is_finite() && *v > 0.0).map(|v| (v, m.min_size_source))
}

/// What a buried conduit run is for: `feeder`, `site`, `service`, `telecom` or `branch`.
pub(crate) fn kind_of(r: &MepRun) -> &'static str {
    let hs = r.height_source.as_str();
    if super::ugplan::is_feeder_run(r) || r.source_sheet.starts_with(&format!("{}feeder", super::ugplan::PROPOSAL_PREFIX)) {
        "feeder"
    } else if hs.contains("(service_") {
        "service"
    } else if hs.contains("(telecom") {
        "telecom"
    } else if r.source_sheet.starts_with(super::sitework::SOURCE_PREFIX) || hs.contains("(ev_charger") || hs.contains("(site_power") {
        "site"
    } else {
        "branch"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::{PanelCircuit, PanelSchedule};

    fn schedule(panel: &str, volts: &str, rows: &[(&str, &str, u32, &str, Option<f64>, &str)]) -> PanelSchedule {
        PanelSchedule {
            panel: panel.into(),
            header: [("volts/phase".to_string(), volts.to_string())].into_iter().collect(),
            circuits: rows
                .iter()
                .map(|(n, brk, poles, wire, va, what)| PanelCircuit {
                    number: (*n).into(),
                    breaker: (*brk).into(),
                    poles: *poles,
                    wire: (*wire).into(),
                    load_va: *va,
                    description: (*what).into(),
                    source: "E-9".into(),
                    ..PanelCircuit::default()
                })
                .collect(),
            source: "E-9".into(),
            ..PanelSchedule::default()
        }
    }

    fn spec() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        sp.mep.panel_schedules.push(schedule(
            "LP2",
            "208Y/120V 3PH 4W",
            &[
                ("5", "20/1", 1, "", Some(400.0), "PARKING LIGHTS"),
                ("7", "20/1", 1, "", None, "SIGN"),
                ("9", "20/1", 1, "", None, "SIGN"),
                ("11", "50/2", 2, "", Some(7200.0), "CAR CHARGER"),
                ("12", "20/1", 1, "10", None, "GATE"),
            ],
        ));
        sp
    }

    #[test]
    fn a_label_names_its_breakers_by_the_schedule() {
        let sp = spec();
        let b = breakers(&sp, "LP2-11,13");
        assert_eq!(b.len(), 1, "one two-pole breaker: {b:?}");
        assert_eq!((b[0].poles, b[0].trip, b[0].numbers.clone()), (2, Some(50.0), vec!["11".to_string(), "13".to_string()]));
        let b = breakers(&sp, "LP2-7, LP2-9");
        assert_eq!(b.iter().map(|x| x.numbers.join(",")).collect::<Vec<_>>(), vec!["7", "9"]);
        // A number no schedule lists: the label's numbers of its group are one breaker.
        let b = breakers(&sp, "Q-4,6");
        assert_eq!((b.len(), b[0].poles, b[0].trip), (1, 2, None));
        assert!(breakers(&sp, "SITE").is_empty());
    }

    #[test]
    fn a_buried_circuit_is_sized_by_its_conductors_not_one_size() {
        let sp = spec();
        // A short run of one 20 A circuit: #12 with its neutral and ground, 1/2" (NEC 352.20).
        let s = size(&sp, "LP2-5", 40.0, None).expect("sized");
        assert_eq!(s.makeup, "2#12 + 1#12G");
        assert_eq!(s.size_in, 0.5, "{s:?}");
        // The specification's minimum governs over the code's.
        let s = size(&sp, "LP2-5", 40.0, Some((0.75, "spec 26 05 33: Minimum Raceway Size: 3/4-inch".into()))).expect("sized");
        assert_eq!(s.size_in, 0.75);
        assert!(s.size_source.contains("26 05 33"), "{}", s.size_source);
        // A long run: upsized for the drop, its ground with it (250.122(B)).
        let s = size(&sp, "LP2-5", 400.0, None).expect("sized");
        assert_ne!(s.makeup, "2#12 + 1#12G", "{s:?}");
        assert!(s.conductors_why.iter().any(|w| w.contains("voltage drop")), "{s:?}");
        // A two-pole charger: no neutral, #8 for 50 A, 3/4" by fill.
        let s = size(&sp, "LP2-11,13", 30.0, None).expect("sized");
        assert_eq!(s.makeup, "2#8 + 1#10G", "{s:?}");
        assert_eq!(s.size_in, 0.5, "three conductors, #8 the largest, fill 1/2\" PVC (Chapter 9): {s:?}");
        // The schedule's wire is kept.
        let s = size(&sp, "LP2-12", 20.0, None).expect("sized");
        assert_eq!(s.makeup, "2#10 + 1#10G");
        // Two chargers in one raceway: more conductors, a larger size.
        let s = size(&sp, "LP2-11,13,LP2-11,13", 30.0, None).expect("sized");
        assert_eq!(s.breakers.len(), 1, "a breaker listed twice is one");
        let mut two = sp.clone();
        if let Some(p) = two.mep.panel_schedules.first_mut() {
            p.circuits.push(PanelCircuit { number: "15".into(), breaker: "50/2".into(), poles: 2, ..PanelCircuit::default() });
        }
        let s = size(&two, "LP2-11,13,LP2-15,17", 30.0, None).expect("sized");
        assert_eq!(s.breakers.len(), 2);
        assert!(s.size_in >= 1.0, "four #8 or larger and a ground: {s:?}");
    }

    #[test]
    fn assign_sizes_a_circuits_runs_and_tags_it() {
        let mut sp = spec();
        let mut home = super::super::printcond::run(super::super::ugplan::E_UG, "L1", "site work E-9", vec![[0.0, 0.0], [600.0, 0.0]], 1.0, -26.0 / 12.0);
        home.id = run_id("L1", "LP2-11,13", 0);
        let mut up = super::super::printcond::run(super::super::ugplan::E_UG, "L1", "site work E-9", vec![[600.0, 0.0]], 1.0, -26.0 / 12.0);
        up.top_ft = Some(0.5);
        up.id = run_id("L1", "LP2-11,13", 1);
        // A note's run that holds its conductors keeps the note's size.
        let mut noted = super::super::printcond::run(super::super::ugplan::E_UG, "L1", "E-9", vec![[0.0, 0.0], [0.0, 120.0]], 1.5, -2.0);
        noted.id = run_id("L1", "LP2-7", 0);
        noted.size_source = "note: E-9 note".into();
        // A note's run too small for its conductors is corrected, the note's size beside it.
        let mut small = super::super::printcond::run(super::super::ugplan::E_UG, "L1", "E-9", vec![[0.0, 0.0], [0.0, 120.0]], 0.5, -2.0);
        small.id = run_id("L1", "LP2-11,13,LP2-15,17", 0);
        small.size_source = "note: E-9 note".into();
        if let Some(p) = sp.mep.panel_schedules.first_mut() {
            p.circuits.push(PanelCircuit { number: "15".into(), breaker: "50/2".into(), poles: 2, ..PanelCircuit::default() });
        }
        // A stale tag of a circuit no run holds goes.
        let stale = ElementTag { id: tag_id("L1", "LP2-99"), kind: "circuit".into(), ..ElementTag::default() };
        sp.tags.push(stale);
        let mut runs = vec![home, up, noted, small];
        let lines = assign(&mut sp, &mut runs, None);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(runs[0].size_in[0], 0.5, "{:?}", runs[0]);
        assert_eq!(runs[1].size_in[0], 0.5, "its stub-up with it");
        // The top stays at its cover: the bottom rises by the smaller size.
        assert!((runs[0].bottoms_ft[0] - (-26.0 / 12.0 + 0.5 / 12.0)).abs() < 1e-9, "{:?}", runs[0].bottoms_ft);
        assert!(runs[0].size_source.starts_with("fill: 2#8 + 1#10G"), "{}", runs[0].size_source);
        assert_eq!(runs[2].size_in[0], 1.5, "the note's size holds");
        assert!(runs[2].corrections.is_empty());
        assert!(runs[3].size_in[0] > 0.5);
        assert_eq!(runs[3].corrections.first().map(|c| c.key.as_str()), Some("size_in"));
        assert!(runs[3].corrections.first().is_some_and(|c| c.design.starts_with("1/2\"")), "{:?}", runs[3].corrections);
        let t = sp.tags.iter().find(|t| t.id == tag_id("L1", "LP2-11,13")).expect("tag");
        assert_eq!(t.kind, "circuit");
        assert_eq!(t.fields.get("circuit").map(|f| f.value.as_str()), Some("LP2-11,13"));
        assert_eq!(t.fields.get("conductors").map(|f| f.value.as_str()), Some("2#8 + 1#10G"));
        assert_eq!(t.fields.get("route").map(|f| f.value.as_str()), Some(ROUTE));
        assert_eq!(super::super::printcond::circuit_of_run(&runs[0]), Some(t.id.as_str()));
        assert!(!sp.tags.iter().any(|t| t.id == tag_id("L1", "LP2-99")), "the stale tag went");
        // Again: one tag each, not two.
        let _ = assign(&mut sp, &mut runs, None);
        assert_eq!(sp.tags.iter().filter(|t| is_buried(&t.id)).count(), 3);
    }

    #[test]
    fn a_buried_runs_kind_is_its_purpose() {
        let mut r = super::super::printcond::run(super::super::ugplan::E_UG, "L1", "site work E-9", vec![[0.0, 0.0], [1.0, 0.0]], 1.0, -2.0);
        r.height_source = "proposed".into();
        assert_eq!(kind_of(&r), "site");
        r.height_source = "called: E-9 note (service_primary, steel elbows and risers)".into();
        assert_eq!(kind_of(&r), "service");
        r.source_sheet = "feeder 7 SB1→P2".into();
        r.height_source = "proposed".into();
        assert_eq!(kind_of(&r), "feeder");
        r.source_sheet = "ug feed 74 MDP→ELEVATOR (circuit 8)".into();
        assert_eq!(kind_of(&r), "branch");
        r.source_sheet = "E-9".into();
        r.height_source = "called: E-9 keynote 1 (ev_charger)".into();
        assert_eq!(kind_of(&r), "site");
    }
}
