//! The fill-in pass (Q6216; owner 11-05: the field built the building already: what the drawings
//! leave out it settles from the code and trade practice). After the documents' work is in the
//! spec, each object with no drawn connection is connected the way the field connects it, by the
//! trade practice rules (`tradepractice.json`), every run laid in marked inferred with its rule as
//! its basis ([`super::provenance`]), never drawn:
//! - a plumbing fixture's waste and vent (`P-BRANCH-WASTE`, `P-BRANCH-VENT`) where no drain or
//!   vent line of its floor is in reach (its water: [`super::waterfill`], `P-BRANCH-WATER`);
//! - an air terminal's branch duct (`M-BRANCH-DUCT`) where no duct of its system reaches it;
//! - a power device's circuit and homerun (`E-UNCIRCUITED`: in the conduit routing).
//!
//! Each run is laid by [`super::waterfill::infer`]: off the nearest run of its system at right
//! angles, the run carried on past its end, off a riser along one axis then the other.
//!
//! [`connections`] is the one measure of what is connected, per trade: fixtures to their lines,
//! devices to a panel, terminals to a duct.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use cadaddy_doc::props::{self, Prop};
use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::{BuildingSpec, MepKind, MepRun};
use serde_json::{Value, json};

use super::provenance::PracticeRule;
use super::waterfill::{FillRules, Need};

/// Most fixtures, terminals and runs looked at per floor (hostile models).
const MAX_ITEMS: usize = 20_000;
/// Most elements the connection measure walks.
const MAX_ELEMENTS: usize = 3_000_000;
/// Most unconnected items listed by name.
const MAX_LISTED: usize = 40;

/// A plumbing fixture as the fill-in sees it: its point, its tag, the words of its schedule row
/// and the lines it takes.
#[derive(Clone, Debug)]
pub(crate) struct FixtureAt {
    pub at: [f64; 2],
    pub what: String,
    pub words: String,
    pub lines: Vec<String>,
}

/// What one system's fill-in on one floor laid in.
#[derive(Debug, Default)]
pub(crate) struct Filled {
    pub needs: usize,
    pub served: usize,
    pub runs: Vec<MepRun>,
    pub unserved: Vec<String>,
}

impl Filled {
    pub(crate) fn feet(&self, upf: f64) -> f64 {
        let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
        self.runs.iter().map(|r| r.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>()).sum::<f64>() / upf
    }
}

/// The waterfill rules for laying `rule`'s runs of system `sys`.
fn fill_rules(rule: &PracticeRule, sys: &str) -> FillRules {
    let mut ru = super::waterfill::rules();
    ru.water = vec![sys.to_string()];
    ru.fixture_lines = vec![sys.to_string()];
    ru.branch_basis = rule.basis_for("");
    if rule.default_size_in > 0.0 {
        ru.branch_size_in = rule.default_size_in;
    }
    if rule.reach_ft > 0.0 {
        ru.reach_ft = rule.reach_ft;
    }
    ru
}

/// Whether a run of `sys` (or, `also`, a riser of one of those systems: a vent's stack vent) is
/// within `d` of `p`.
fn reached(runs: &[&MepRun], sys: &str, also: &[String], p: [f64; 2], d: f64) -> bool {
    runs.iter().filter(|r| r.system == sys || (r.top_ft.is_some() && also.contains(&r.system))).any(|r| match r.points.as_slice() {
        [q] => (q[0] - p[0]).hypot(q[1] - p[1]) <= d,
        pts => pts.windows(2).any(|w| super::waterfill::to_segment(p, w[0], w[1]).0 <= d),
    })
}

/// Each line of `rule`'s systems a floor's fixtures take with nothing of theirs within the rule's
/// connect reach (a vent: nor a riser of `stack_vents`, the waste stack it is vented through), laid
/// in from the floor's runs of that system (`floor_runs`) by the rule: a branch the fixture's size
/// by its schedule words, marked inferred with the rule's basis. Per system.
pub(crate) fn fixture_lines(
    level: &str,
    floor_runs: &[&MepRun],
    fixtures: &[FixtureAt],
    rule: &PracticeRule,
    stack_vents: &[String],
    upf: f64,
) -> BTreeMap<String, Filled> {
    let mut out = BTreeMap::new();
    if !(upf.is_finite() && upf > 0.0) {
        return out;
    }
    let reach = rule.connect_reach_ft.max(0.0) * upf;
    for sys in &rule.systems {
        let needs: Vec<Need> = fixtures
            .iter()
            .take(MAX_ITEMS)
            .filter(|f| f.lines.iter().any(|l| l == sys))
            .filter(|f| !reached(floor_runs, sys, stack_vents, f.at, reach))
            .map(|f| Need { system: sys.clone(), pts: vec![f.at], what: format!("fixture {}", f.what), size_in: rule.size_for(&f.words), segs: Vec::new() })
            .collect();
        if needs.is_empty() {
            continue;
        }
        let n = needs.len();
        let mains: Vec<&MepRun> = floor_runs.iter().copied().filter(|r| r.system == *sys).take(MAX_ITEMS).collect();
        let ru = fill_rules(rule, sys);
        let inf = super::waterfill::infer(level, &mains, needs, upf, &ru);
        out.insert(sys.clone(), Filled { needs: n, served: inf.served, runs: inf.runs, unserved: inf.unserved });
    }
    out
}

/// Whether a terminal is an air terminal (a sprinkler head is fire protection's).
fn air_terminal(kind: &str) -> bool {
    !kind.to_ascii_lowercase().contains("sprinkler")
}

/// The duct systems of the spec.
fn duct_systems(sp: &BuildingSpec) -> Vec<&str> {
    sp.mep.systems.iter().filter(|s| s.kind == MepKind::Duct).map(|s| s.name.as_str()).collect()
}

/// Each air terminal with no duct of its system within the rule's connect reach on its floor
/// (`same_floor`), a branch duct laid to it off the nearest duct of its system there (the rule's
/// reach): its neck size (else the rule's size for its kind), at the duct's height, marked
/// inferred with the rule's basis. Per floor and system.
pub(crate) fn terminal_ducts(sp: &BuildingSpec, rule: &PracticeRule, same_floor: &dyn Fn(&str, &str) -> bool) -> BTreeMap<(String, String), Filled> {
    let mut out = BTreeMap::new();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let ducts = duct_systems(sp);
    let reach = rule.connect_reach_ft.max(0.0) * upf;
    let mut groups: BTreeMap<(String, String), Vec<Need>> = BTreeMap::new();
    for t in sp.mep.terminals.iter().take(MAX_ITEMS).filter(|t| air_terminal(&t.terminal_kind) && ducts.contains(&t.system.as_str())) {
        let floor: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| r.system == t.system && same_floor(&r.level, &t.level)).take(MAX_ITEMS).collect();
        if reached(&floor, &t.system, &[], t.at, reach) {
            continue;
        }
        let size = if t.size_in[0].is_finite() && t.size_in[0] > 0.0 { t.size_in[0] } else { rule.size_for(&t.terminal_kind.replace('_', " ")) };
        groups.entry((t.level.clone(), t.system.clone())).or_default().push(Need {
            system: t.system.clone(),
            pts: vec![t.at],
            what: format!("{} at {:.0},{:.0} ft", t.terminal_kind.replace('_', " "), t.at[0] / upf, t.at[1] / upf),
            size_in: size,
            segs: Vec::new(),
        });
    }
    for ((level, sys), needs) in groups {
        let n = needs.len();
        let mains: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| r.system == sys && same_floor(&r.level, &level)).take(MAX_ITEMS).collect();
        let ru = fill_rules(rule, &sys);
        let inf = super::waterfill::infer(&level, &mains, needs, upf, &ru);
        out.insert((level, sys), Filled { needs: n, served: inf.served, runs: inf.runs, unserved: inf.unserved });
    }
    out
}

// ─── The measure ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct Tally {
    total: usize,
    connected: usize,
    by_level: BTreeMap<String, usize>,
    listed: Vec<String>,
}

impl Tally {
    fn add(&mut self, ok: bool, level: &str, what: impl FnOnce() -> String) {
        self.total += 1;
        if ok {
            self.connected += 1;
        } else {
            *self.by_level.entry(level.to_string()).or_default() += 1;
            if self.listed.len() < MAX_LISTED {
                self.listed.push(what());
            }
        }
    }
    fn json(&self, of: &str) -> Value {
        json!({of: self.total, "connected": self.connected, "unconnected": self.total - self.connected, "unconnected_by_level": self.by_level, "unconnected_first": self.listed})
    }
}

/// Whether a device kind takes power: a power device of the routing's rules, a luminaire or sign.
fn takes_power(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase().replace('_', " ");
    super::conduitroute::gap::is_power_device(&k) || ["luminaire", "light", "fixture", "exit", "fan"].iter().any(|w| k.contains(w)) && !k.contains("plumbing")
}

/// What is connected, per trade (Q6216): each plumbing fixture to every line it takes (as the
/// build's connection step found it), each power-taking device to a panel (its box, raceway or
/// cable walked to gear), each air terminal to a duct of its system within the rule's reach.
pub(crate) fn connections(d: &Drawing, sp: Option<&BuildingSpec>) -> Value {
    let mut fixtures = Tally::default();
    let mut devices = Tally::default();
    // The connects graph, both ways; gear its roots.
    let mut adj: HashMap<Handle, Vec<Handle>> = HashMap::new();
    let mut gear: Vec<Handle> = Vec::new();
    let mut power: Vec<(Handle, String, String)> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        let Some(part) = super::wallgen::made_as(e) else { continue };
        if super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")) {
            gear.push(e.handle);
        }
        let ps = props::read(&e.common.xdata);
        let level = super::bspec::made_level(e).unwrap_or_default();
        let kind = ps.get("kind").map(Prop::text).unwrap_or_default();
        if kind == "plumbing_fixture" && (ps.get("connects").is_some() || ps.get("unconnected").is_some()) {
            let ok = ps.get("unconnected").is_none_or(|p| !p.has_value());
            fixtures.add(ok, &level, || format!("{} {}", ps.get("tag").map(Prop::text).unwrap_or_else(|| e.handle.hex()), level));
        }
        if part.eq_ignore_ascii_case("device") && kind != "plumbing_fixture" && takes_power(&kind) {
            power.push((e.handle, level.clone(), kind.clone()));
        }
        let cs = super::mepatoms::connects_of(e);
        for c in cs {
            adj.entry(e.handle).or_default().push(c);
            adj.entry(c).or_default().push(e.handle);
        }
    }
    let mut seen: HashSet<Handle> = gear.iter().copied().collect();
    let mut queue: VecDeque<Handle> = gear.iter().copied().collect();
    while let Some(x) = queue.pop_front() {
        if seen.len() > MAX_ELEMENTS {
            break;
        }
        for n in adj.get(&x).into_iter().flatten() {
            if seen.insert(*n) {
                queue.push_back(*n);
            }
        }
    }
    let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (h, level, kind) in &power {
        let ok = seen.contains(h);
        let e = by_kind.entry(kind.clone()).or_default();
        e.0 += 1;
        e.1 += usize::from(ok);
        devices.add(ok, level, || format!("{kind} {} {level}", h.hex()));
    }
    let mut terminals = Tally::default();
    let mut filled_terminals = 0usize;
    if let Some(sp) = sp {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let rule = super::provenance::rule("M-BRANCH-DUCT");
        let reach = rule.connect_reach_ft.max(0.0) * upf;
        let ducts = duct_systems(sp);
        for t in sp.mep.terminals.iter().take(MAX_ITEMS).filter(|t| air_terminal(&t.terminal_kind)) {
            let floor: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| ducts.contains(&r.system.as_str()) && r.level.eq_ignore_ascii_case(&t.level)).collect();
            let own = reached(&floor, &t.system, &[], t.at, reach);
            filled_terminals += usize::from(
                own && floor.iter().filter(|r| r.system == t.system && !super::provenance::made_of_run(r).is_drawn()).any(|r| reached(&[*r], &t.system, &[], t.at, reach)),
            );
            terminals.add(own, &t.level, || format!("{} {} {}", t.terminal_kind, t.system, t.level));
        }
    }
    let mut e = devices.json("devices");
    e["by_kind"] = by_kind.iter().map(|(k, (n, c))| (k.clone(), json!({"devices": n, "connected": c}))).collect::<serde_json::Map<_, _>>().into();
    let mut h = terminals.json("terminals");
    h["connected_by_inferred_duct"] = json!(filled_terminals);
    json!({"plumbing": fixtures.json("fixtures"), "electrical": e, "hvac": h})
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::Made;

    fn run(system: &str, pts: &[[f64; 2]]) -> MepRun {
        serde_json::from_value(json!({
            "system": system, "level": "2", "points": pts.iter().map(|p| [p[0] * 12.0, p[1] * 12.0]).collect::<Vec<_>>(),
            "size_in": [4.0, 0.0], "bottoms_ft": vec![1.0; pts.len().saturating_sub(1).max(1)], "size_source": "tag", "height_source": "tag",
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn fixture(x: f64, y: f64, what: &str, words: &str) -> FixtureAt {
        FixtureAt { at: [x * 12.0, y * 12.0], what: what.into(), words: words.into(), lines: vec!["SAN".into(), "V".into(), "CW".into()] }
    }

    #[test]
    fn a_fixture_no_drain_reaches_gets_its_waste_arm_and_vent_inferred_by_the_rule() {
        let main = run("SAN", &[[0.0, 0.0], [100.0, 0.0]]);
        let vent = run("V", &[[0.0, 30.0], [100.0, 30.0]]);
        let near = fixture(20.0, 5.0, "P-91", "P-91 LAVATORY");
        let far = fixture(60.0, 40.0, "P-92", "P-92 WATER CLOSET");
        let runs = [&main, &vent];
        let fixtures = [near, far];
        let w = super::super::provenance::rule("P-BRANCH-WASTE");
        let got = fixture_lines("2", &runs, &fixtures, &w, &[], 12.0);
        let san = got.get("SAN").unwrap_or_else(|| panic!("waste laid: {got:?}"));
        // The fixture 5 ft off the main is connected already; the one 40 ft off is not.
        assert_eq!((san.needs, san.served), (1, 1));
        assert!(!san.runs.is_empty());
        for r in &san.runs {
            assert_eq!(r.made, Made::Inferred, "never drawn");
            assert!(r.basis.contains("P-BRANCH-WASTE"), "{}", r.basis);
            assert_eq!(r.system, "SAN");
        }
        // The water closet's arm is 3 in., its trap's size.
        assert!(san.runs.iter().any(|r| (r.size_in[0] - 3.0).abs() < 1e-9), "{:?}", san.runs.iter().map(|r| r.size_in).collect::<Vec<_>>());
        // The branch ends at the fixture: it is reached now.
        let mut all: Vec<&MepRun> = runs.to_vec();
        all.extend(san.runs.iter());
        assert!(reached(&all, "SAN", &[], fixtures[1].at, w.connect_reach_ft * 12.0));
        // Its vent: the vent line is 10 ft from it, the near one's 25 ft.
        let v = super::super::provenance::rule("P-BRANCH-VENT");
        let got = fixture_lines("2", &runs, &fixtures, &v, &[], 12.0);
        let vv = got.get("V").unwrap_or_else(|| panic!("vent laid"));
        assert_eq!((vv.needs, vv.served), (1, 1), "the lavatory 25 ft from the vent line needs one");
        assert!(vv.runs.iter().all(|r| r.made == Made::Inferred && r.basis.contains("P-BRANCH-VENT")));
    }

    #[test]
    fn a_vent_through_its_waste_stack_needs_no_vent_line_and_out_of_reach_stays_listed() {
        let mut stack = run("SAN", &[[50.0, 50.0]]);
        stack.top_ft = Some(30.0);
        let v = super::super::provenance::rule("P-BRANCH-VENT");
        let by_stack = fixture(52.0, 50.0, "P-93", "P-93 SHOWER");
        let got = fixture_lines("2", &[&stack], std::slice::from_ref(&by_stack), &v, &["SAN".into()], 12.0);
        assert!(got.is_empty(), "a stack vent: {got:?}");
        // No vent of the floor within the rule's reach: listed, nothing laid.
        let far_vent = run("V", &[[500.0, 500.0], [600.0, 500.0]]);
        let got = fixture_lines("2", &[&far_vent], std::slice::from_ref(&by_stack), &v, &[], 12.0);
        let vv = got.get("V").unwrap_or_else(|| panic!("a need"));
        assert_eq!((vv.needs, vv.served, vv.runs.len()), (1, 0, 0));
        assert_eq!(vv.unserved.len(), 1);
    }

    #[test]
    fn an_air_terminal_no_duct_reaches_gets_a_branch_duct_at_its_neck_size() {
        let mut sp = BuildingSpec::default();
        sp.units_per_foot = 12.0;
        sp.mep.systems = vec![serde_json::from_value(json!({"name": "SA", "layer": "M-SUPPLY", "kind": "duct", "size_in": [12, 10], "bottom_ft": 9.0}))
            .unwrap_or_else(|e| panic!("{e}"))];
        let mut duct = run("SA", &[[0.0, 0.0], [80.0, 0.0]]);
        duct.size_in = [14.0, 10.0];
        sp.mep.runs = vec![duct];
        let t = |x: f64, y: f64, neck: f64, kind: &str| -> cadaddy_walk3d::MepTerminal {
            serde_json::from_value(json!({"system": "SA", "level": "2", "at": [x * 12.0, y * 12.0], "size_in": [neck, 0], "bottom_ft": 9.0, "terminal_kind": kind}))
                .unwrap_or_else(|e| panic!("{e}"))
        };
        sp.mep.terminals = vec![t(10.0, 4.0, 10.0, "supply_diffuser"), t(40.0, 25.0, 0.0, "supply_diffuser"), t(30.0, 30.0, 0.0, "sprinkler_pendent")];
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let rule = super::super::provenance::rule("M-BRANCH-DUCT");
        let got = terminal_ducts(&sp, &rule, &same);
        let f = got.get(&("2".to_string(), "SA".to_string())).unwrap_or_else(|| panic!("{got:?}"));
        assert_eq!((f.needs, f.served), (1, 1), "the diffuser 4 ft off the duct is on it; the sprinkler is no air terminal");
        assert!(f.runs.iter().all(|r| r.made == Made::Inferred && r.basis.contains("M-BRANCH-DUCT") && r.system == "SA"));
        // No neck size: the rule's for its kind.
        assert!(f.runs.iter().any(|r| (r.size_in[0] - rule.default_size_in).abs() < 1e-9));
        // Measured: one of two air terminals connected before, both after.
        let before = connections(&Drawing::default(), Some(&sp));
        assert_eq!((before["hvac"]["terminals"].as_u64(), before["hvac"]["connected"].as_u64()), (Some(2), Some(1)));
        sp.mep.runs.extend(f.runs.iter().cloned());
        let after = connections(&Drawing::default(), Some(&sp));
        assert_eq!((after["hvac"]["connected"].as_u64(), after["hvac"]["connected_by_inferred_duct"].as_u64()), (Some(2), Some(1)));
    }
}
