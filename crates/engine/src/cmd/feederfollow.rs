//! Feeders follow their gear (Q6523), as branch circuits follow their devices.
//!
//! A feeder's conduit is routed gear to gear by [`super::gearbuild::feeder_runs`] and kept as
//! runs in the building spec. Whenever the gear may have moved (the equipment set again, the model
//! built again, the circuits routed again) the stored routes are checked against the gear as it
//! stands now: a route is kept while its ends still land on each piece of gear its feeder lands
//! on (an end in the gear's outline, or a riser just beside it); a route with an end no longer at
//! its gear (the gear moved, went to another floor, a feed-through chain takes other copies now)
//! is never kept: the feeder is routed again from its source to where its gear is now, every
//! conduit of it, as the job's feeder routes step routes it.
//! - a gear moved within its own outline keeps its route (its conduits still enter it; the usual
//!   field call). The alternative, routing again on any move, would also replace the routes a
//!   person drew or edited;
//! - the routes judged are those CADaddy's feeder routing made (a person's edit on one too); a
//!   route drawn on the prints or entered by hand is kept as given, and the check counts it when
//!   it doesn't land on its gear;
//! - a feeder with placed gear and no route gets one from `conduit.reroute` ("gear placed since
//!   the build"), once the model's feeders were routed at all (the job's feeder routes step), or
//!   from `conduit.reroute {feeders: "all"}` on demand; bspec.set and model.build only follow the
//!   routes there are (a model may leave a feeder unrouted on purpose: the build gives it its
//!   raceway into its gear);
//! - a proposed route whose gear is gone is not kept;
//! - a route follows the model's feed data (Q6664): a proposed route whose chains differ from
//!   those the gear's feeds give now (a stack's copies recorded fed from another breaker, or
//!   through other copies' lugs, or the other way along the stack) is routed again as the feed
//!   data says; a route drawn or entered by hand that differs is kept as given and said;
//! - never both: a feeder is not routed where its typical panel's copies already take a route
//!   that is kept as given (drawn, entered, a feeder of no listed tag); the two disagree, said,
//!   and the copies keep their one raceway in.
//!
//! Nothing is replaced silently: the report lists each feeder routed again and why.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::{BuildingSpec, MepEquipment, MepRun};
use serde_json::{Value, json};

/// A route still lands on its gear when one of its ends on the gear's level is within this of the
/// gear's outline, feet (the conduit's entry in it, or the riser just beside it).
const AT_GEAR_REACH_FT: f64 = 1.5;
/// An end of a run is at its gear's height when within this of the gear's bottom or top (or
/// between them), feet.
const AT_GEAR_HEIGHT_FT: f64 = 0.5;
/// The most feeder runs looked at (a bound on hostile input).
const MAX_RUNS: usize = 200_000;
/// The most feeders named in the report.
const MAX_LISTED: usize = 50;

/// What following the gear did.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Followed {
    /// Feeders (`tag from→to`) routed again, each with why.
    pub rerouted: Vec<(String, String)>,
    /// Feeders whose routes were kept as they are.
    pub kept: usize,
    /// Proposed routes dropped: their feeder can't be routed now (its gear is gone).
    pub dropped: Vec<(String, String)>,
    /// Feeders whose route didn't land on its gear before / after.
    pub off_gear_before: usize,
    pub off_gear_after: usize,
    /// Feeders with placed gear and no route before / after.
    pub unrouted_before: usize,
    pub unrouted_after: usize,
    /// Feeders whose stored route doesn't follow the feed data (other chains) before / after.
    pub off_feed_before: usize,
    pub off_feed_after: usize,
    /// Routes kept as given that the feed data disagrees with, and feeders not routed because
    /// their copies already take a route kept as given (route and feed data disagree).
    pub disagree: Vec<(String, String)>,
}

impl Followed {
    pub(crate) fn changed(&self) -> bool {
        !self.rerouted.is_empty() || !self.dropped.is_empty()
    }

    /// Whether there is anything to say: a change, or a route and the feed data disagreeing.
    pub(crate) fn notable(&self) -> bool {
        self.changed() || !self.disagree.is_empty()
    }

    pub(crate) fn json(&self) -> Value {
        let list = |v: &[(String, String)]| v.iter().take(MAX_LISTED).map(|(f, why)| json!({"feeder": f, "why": why})).collect::<Vec<_>>();
        json!({
            "rerouted": self.rerouted.len(), "kept": self.kept, "dropped": self.dropped.len(),
            "off_gear_before": self.off_gear_before, "off_gear_after": self.off_gear_after,
            "unrouted_before": self.unrouted_before, "unrouted_after": self.unrouted_after,
            "off_feed_before": self.off_feed_before, "off_feed_after": self.off_feed_after,
            "disagree": self.disagree.len(), "disagree_list": list(&self.disagree),
            "rerouted_list": list(&self.rerouted), "dropped_list": list(&self.dropped),
        })
    }

    /// One line for the command line, or none when nothing changed.
    pub(crate) fn line(&self) -> Option<String> {
        if !self.notable() {
            return None;
        }
        let names = self.rerouted.iter().take(6).map(|(f, why)| format!("{f} ({why})")).collect::<Vec<_>>().join("; ");
        let disagree = self.disagree.iter().take(3).map(|(f, why)| format!("{f} ({why})")).collect::<Vec<_>>().join("; ");
        Some(format!(
            "feeders followed their gear and feeds: {} routed again{}{}; {} kept{}",
            self.rerouted.len(),
            if names.is_empty() { String::new() } else { format!(": {names}") },
            if self.dropped.is_empty() { String::new() } else { format!("; {} routes dropped (their gear is gone)", self.dropped.len()) },
            self.kept,
            if disagree.is_empty() { String::new() } else { format!("; route and feed data disagree for {}: {disagree}", self.disagree.len()) }
        ))
    }
}

/// Which feeder a run is routed for: its `mep.feeders` index, else its source text.
fn group_of(sp: &BuildingSpec, r: &MepRun) -> String {
    match super::mepatoms::build::feeder_of_run(sp, r) {
        Some(i) => format!("#{i}"),
        None => format!("={}", r.source_sheet),
    }
}

/// A run CADaddy's feeder routing made (a person may have edited it since).
fn proposed(r: &MepRun) -> bool {
    r.size_source == "feeder schedule" && r.height_source == "proposed"
}

/// What following the gear may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Route again the routes whose gear moved (bspec.set, model.build).
    Moved,
    /// And route the feeders with placed gear and no route, once the model's feeders were routed
    /// (conduit.reroute: gear placed since the build).
    Placed,
    /// And route every feeder with placed gear and no route, routed or not before (on demand).
    All,
}

/// A run's ends: a riser's one point, a run's first and last (not the corners between).
fn ends(r: &MepRun) -> impl Iterator<Item = [f64; 2]> + '_ {
    let last = if r.points.len() >= 2 { r.points.last().copied() } else { None };
    r.points.first().copied().into_iter().chain(last)
}

/// A level's floor elevation, feet, and its plan's corner (each level's plan has its own place).
fn level_at(sp: &BuildingSpec, name: &str) -> (f64, [f64; 2]) {
    sp.level(name).map_or((0.0, [0.0, 0.0]), |l| {
        let z = if l.elevation_ft.is_finite() { l.elevation_ft } else { 0.0 };
        (z, l.region.filter(|r| r.iter().all(|v| v.is_finite())).map_or([0.0, 0.0], |r| [r[0], r[1]]))
    })
}

/// Whether the run has an end at the gear, within `reach` of its outline on plan: a run across
/// on the gear's level with an end over (or under) it; a riser on any level (moved onto the gear
/// level's plan) with its foot or head at the gear's bottom or top: one passing the gear's floor
/// beside it, floor to ceiling, isn't.
fn end_at(sp: &BuildingSpec, r: &MepRun, g: &MepEquipment, reach: f64) -> bool {
    if r.points.len() != 1 {
        return r.level.eq_ignore_ascii_case(&g.level) && ends(r).any(|p| near(g, p, reach));
    }
    let ((zr, cr), (zg, cg)) = (level_at(sp, &r.level), level_at(sp, &g.level));
    let bottom = r.bottoms_ft.first().copied().filter(|b| b.is_finite()).unwrap_or(0.0);
    let top = r.top_ft.filter(|t| t.is_finite()).unwrap_or(bottom);
    let (gb, gh) = (if g.bottom_ft.is_finite() { g.bottom_ft } else { 0.0 }, if g.height_ft.is_finite() { g.height_ft.max(0.0) } else { 0.0 });
    let (lo, hi) = (zg + gb - AT_GEAR_HEIGHT_FT, zg + gb + gh + AT_GEAR_HEIGHT_FT);
    if ![zr + bottom, zr + top].iter().any(|z| *z >= lo && *z <= hi) {
        return false;
    }
    ends(r).any(|p| near(g, [p[0] - cr[0] + cg[0], p[1] - cr[1] + cg[1]], reach))
}

/// Whether `p` is within `reach` of the gear's outline on plan.
fn near(g: &MepEquipment, p: [f64; 2], reach: f64) -> bool {
    let (x0, x1) = (g.min[0].min(g.max[0]), g.min[0].max(g.max[0]));
    let (y0, y1) = (g.min[1].min(g.max[1]), g.min[1].max(g.max[1]));
    p[0] >= x0 - reach && p[0] <= x1 + reach && p[1] >= y0 - reach && p[1] <= y1 + reach
}

/// The gear a feeder's fresh route lands on: each piece the feeder lands on (its source, its end,
/// a copy of its end) that the route has a point in.
fn landed_gear<'a>(sp: &'a BuildingSpec, feeder: Option<usize>, fresh: &[&MepRun]) -> Vec<&'a MepEquipment> {
    let Some(f) = feeder.and_then(|i| sp.mep.feeders.get(i)) else { return Vec::new() };
    let reach = AT_GEAR_REACH_FT * upf(sp);
    sp.mep
        .equipment
        .iter()
        .filter(|e| super::mepatoms::build::feeder_lands_on(f, &e.tag))
        .filter(|e| fresh.iter().any(|r| end_at(sp, r, e, reach)))
        .collect()
}

/// Whether a stored route has an end at each piece of gear ([`end_at`], within [`AT_GEAR_REACH_FT`]).
fn lands_on(sp: &BuildingSpec, stored: &[&MepRun], gear: &[&MepEquipment]) -> bool {
    let reach = AT_GEAR_REACH_FT * upf(sp);
    gear.iter().all(|g| stored.iter().any(|r| end_at(sp, r, g, reach)))
}

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// A feeder as the report names it.
fn feeder_name(sp: &BuildingSpec, group: &str, runs: &[&MepRun]) -> String {
    let listed = group.strip_prefix('#').and_then(|i| i.parse::<usize>().ok()).and_then(|i| sp.mep.feeders.get(i));
    match listed {
        Some(f) => format!("{} {}→{}", f.tag, f.from, f.to).trim().to_string(),
        None => runs.first().map(|r| r.source_sheet.trim_start_matches("feeder ").to_string()).unwrap_or_default(),
    }
}

/// Route the spec's feeders again where their gear moved (module doc), and by `mode` route those
/// with no route. The spec's runs are changed in place.
pub(crate) fn follow(sp: &mut BuildingSpec, mode: Mode) -> Followed {
    let mut out = Followed::default();
    if sp.mep.feeders.is_empty() {
        return out;
    }
    // Routed at all: a listed feeder has a route (a raceway drawn on a feeder layer alone is not
    // a route; the build makes each unrouted feeder's raceway into its gear).
    let any_routed =
        sp.mep.runs.iter().take(MAX_RUNS).any(|r| super::ugplan::is_feeder_run(r) && super::mepatoms::build::feeder_of_run(sp, r).is_some());
    if !any_routed && mode != Mode::All {
        return out;
    }
    let route_missing = mode != Mode::Moved;
    let (fresh, _left) = super::gearbuild::feeder_runs(sp);
    let mut fresh_by: BTreeMap<String, Vec<&MepRun>> = BTreeMap::new();
    for r in fresh.iter().take(MAX_RUNS) {
        fresh_by.entry(group_of(sp, r)).or_default().push(r);
    }
    let mut stored_by: BTreeMap<String, Vec<&MepRun>> = BTreeMap::new();
    for r in sp.mep.runs.iter().filter(|r| super::ugplan::is_feeder_run(r)).take(MAX_RUNS) {
        stored_by.entry(group_of(sp, r)).or_default().push(r);
    }
    // Per feeder: route it again (with why), drop its route, or keep it.
    let mut replace: BTreeSet<String> = BTreeSet::new();
    let mut drop: BTreeSet<String> = BTreeSet::new();
    // Feeders to route (again), with why: laid unless their copies take a route kept as given.
    let mut wanted: Vec<(String, String, String)> = Vec::new();
    let groups: BTreeSet<&String> = fresh_by.keys().chain(stored_by.keys()).collect();
    for g in groups {
        let feeder = g.strip_prefix('#').and_then(|i| i.parse::<usize>().ok());
        let new = fresh_by.get(g).map(Vec::as_slice).unwrap_or_default();
        let old = stored_by.get(g).map(Vec::as_slice).unwrap_or_default();
        let name = feeder_name(sp, g, if old.is_empty() { new } else { old });
        let gear = landed_gear(sp, feeder, new);
        let at_gear = lands_on(sp, old, &gear);
        if old.is_empty() {
            if !new.is_empty() {
                out.unrouted_before += 1;
                if route_missing {
                    wanted.push((g.clone(), name, "not routed: its gear is placed".into()));
                }
            }
            continue;
        }
        if new.is_empty() {
            // It can't be routed as the gear stands (its gear is gone, or not one piece): a route
            // CADaddy proposed to it goes; another (drawn, read, edited) stays as given.
            if feeder.is_some() && old.iter().all(|r| proposed(r)) {
                out.dropped.push((name, "its feeder can't be routed now: not one placed piece of gear at an end".into()));
                drop.insert(g.clone());
            } else {
                out.kept += 1;
            }
            continue;
        }
        if !at_gear {
            out.off_gear_before += 1;
        }
        // The chains it lays against those the feed data gives now (Q6664).
        let off_feed = feeder.is_some() && off_feed(old, new);
        if off_feed {
            out.off_feed_before += 1;
        }
        // Drawn or entered by hand: kept as given (the check counts it).
        if !old.iter().all(|r| proposed(r)) {
            out.kept += 1;
            if off_feed && out.disagree.len() < MAX_LISTED {
                out.disagree.push((name, "a route drawn or entered by hand, kept as given: it doesn't follow the panels' recorded feed".into()));
            }
            continue;
        }
        if at_gear && !off_feed {
            out.kept += 1;
            continue;
        }
        // Other chains than the feed's say more than an end off its gear.
        let why = if off_feed {
            format!("its route doesn't follow the panels' feed: {}", feed_difference(old, new))
        } else if old.iter().any(|r| !r.edited_by.is_empty()) {
            "its gear moved: the route (edited) ended where the gear was".to_string()
        } else {
            "its gear moved: the route ended where the gear was".to_string()
        };
        wanted.push((g.clone(), name, why));
    }
    // Never both (Q6664): a typical panel's copy that takes a route kept as given (another
    // feeder's, drawn or entered) gets no second raceway in; the two disagree, said.
    let reach = AT_GEAR_REACH_FT * upf(sp);
    let changing: BTreeSet<&String> = wanted.iter().map(|w| &w.0).chain(drop.iter()).collect();
    let kept_runs: Vec<&MepRun> = stored_by.iter().filter(|(g, _)| !changing.contains(g)).flat_map(|(_, v)| v.iter().copied()).collect();
    for (g, name, why) in wanted {
        let feeder = g.strip_prefix('#').and_then(|i| i.parse::<usize>().ok());
        let copies = feeder.and_then(|i| sp.mep.feeders.get(i)).map_or_else(Vec::new, |f| {
            landed_gear(sp, feeder, fresh_by.get(&g).map(Vec::as_slice).unwrap_or_default())
                .into_iter()
                .filter(|e| !e.tag.trim().eq_ignore_ascii_case(f.from.trim()) && !e.tag.trim().eq_ignore_ascii_case(f.to.trim()))
                .collect()
        });
        let taken = copies.iter().find(|e| kept_runs.iter().any(|r| end_at(sp, r, e, reach)));
        if let Some(e) = taken {
            if out.disagree.len() < MAX_LISTED {
                out.disagree.push((name, format!("not laid ({why}): {} already takes a route kept as given", e.tag)));
            }
            continue;
        }
        out.rerouted.push((name, why));
        replace.insert(g);
    }
    if replace.is_empty() && drop.is_empty() {
        out.off_gear_after = out.off_gear_before;
        out.unrouted_after = out.unrouted_before;
        out.off_feed_after = out.off_feed_before;
        return out;
    }
    // The fresh routes keep the ids of the runs they replace, run for run where as many.
    let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (g, old) in &stored_by {
        if replace.contains(g) && fresh_by.get(g).is_some_and(|n| n.len() == old.len()) {
            ids.insert(g.clone(), old.iter().map(|r| r.id.clone()).collect());
        }
    }
    let gone: BTreeSet<String> = replace.union(&drop).cloned().collect();
    let mut add: Vec<MepRun> = Vec::new();
    for g in &replace {
        let given = ids.get(g);
        for (k, r) in fresh_by.get(g).map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
            let mut r = (*r).clone();
            if let Some(id) = given.and_then(|v| v.get(k)) {
                r.id.clone_from(id);
            }
            add.push(r);
        }
    }
    let keep: Vec<MepRun> =
        std::mem::take(&mut sp.mep.runs).into_iter().filter(|r| !super::ugplan::is_feeder_run(r) || !gone.contains(&group_of(sp, r))).collect();
    sp.mep.runs = keep;
    sp.mep.runs.extend(add);
    // The check again on what the spec holds now.
    let after = check(sp);
    out.off_gear_after = after.0;
    out.unrouted_after = after.1;
    out.off_feed_after = off_feed_count(sp);
    out
}

/// The chains a feeder's runs lay: each run names its feeder's chain, source to last copy
/// (`feeder 604 DP1→G-101→G-201`).
fn chains<'a>(runs: &[&'a MepRun]) -> BTreeSet<&'a str> {
    runs.iter().map(|r| r.source_sheet.trim()).collect()
}

/// Whether a stored route lays other chains than the fresh one (the feed data's).
fn off_feed(old: &[&MepRun], new: &[&MepRun]) -> bool {
    chains(old) != chains(new)
}

/// How a stored route's chains differ from the feed data's, briefly.
fn feed_difference(old: &[&MepRun], new: &[&MepRun]) -> String {
    let (o, n) = (chains(old), chains(new));
    let short = |c: &str| c.trim_start_matches("feeder ").to_string();
    let laid: Vec<String> = o.difference(&n).take(2).map(|c| short(c)).collect();
    let fed: Vec<String> = n.difference(&o).take(2).map(|c| short(c)).collect();
    format!(
        "{} of its {} chains differ (laid {}; the feed {})",
        n.difference(&o).count().max(o.difference(&n).count()),
        n.len(),
        if laid.is_empty() { "-".to_string() } else { laid.join(", ") },
        if fed.is_empty() { "-".to_string() } else { fed.join(", ") }
    )
}

/// The listed feeders whose stored route lays other chains than the feed data gives.
pub(crate) fn off_feed_count(sp: &BuildingSpec) -> usize {
    if sp.mep.feeders.is_empty() {
        return 0;
    }
    let (fresh, _) = super::gearbuild::feeder_runs(sp);
    let mut fresh_by: BTreeMap<String, Vec<&MepRun>> = BTreeMap::new();
    for r in fresh.iter().take(MAX_RUNS) {
        fresh_by.entry(group_of(sp, r)).or_default().push(r);
    }
    let mut stored_by: BTreeMap<String, Vec<&MepRun>> = BTreeMap::new();
    for r in sp.mep.runs.iter().filter(|r| super::ugplan::is_feeder_run(r)).take(MAX_RUNS) {
        stored_by.entry(group_of(sp, r)).or_default().push(r);
    }
    fresh_by.iter().filter(|(g, new)| g.starts_with('#') && stored_by.get(*g).is_some_and(|old| off_feed(old, new))).count()
}

/// The feeders whose stored route doesn't land on its gear, and those with placed gear and no
/// route: (off gear, unrouted).
pub(crate) fn check(sp: &BuildingSpec) -> (usize, usize) {
    if sp.mep.feeders.is_empty() {
        return (0, 0);
    }
    let (fresh, _) = super::gearbuild::feeder_runs(sp);
    let mut fresh_by: BTreeMap<String, Vec<&MepRun>> = BTreeMap::new();
    for r in fresh.iter().take(MAX_RUNS) {
        fresh_by.entry(group_of(sp, r)).or_default().push(r);
    }
    let mut stored_by: BTreeMap<String, Vec<&MepRun>> = BTreeMap::new();
    for r in sp.mep.runs.iter().filter(|r| super::ugplan::is_feeder_run(r)).take(MAX_RUNS) {
        stored_by.entry(group_of(sp, r)).or_default().push(r);
    }
    let (mut off, mut unrouted) = (0, 0);
    for (g, new) in &fresh_by {
        let feeder = g.strip_prefix('#').and_then(|i| i.parse::<usize>().ok());
        match stored_by.get(g) {
            None => unrouted += 1,
            Some(old) if !lands_on(sp, old, &landed_gear(sp, feeder, new)) => off += 1,
            Some(_) => {}
        }
    }
    (off, unrouted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gear(tag: &str, level: &str, at: [f64; 2]) -> MepEquipment {
        MepEquipment {
            tag: tag.into(),
            kind: "panel".into(),
            level: level.into(),
            min: [at[0] - 6.0, at[1] - 3.0],
            max: [at[0] + 6.0, at[1] + 3.0],
            bottom_ft: 2.0,
            height_ft: 4.0,
            ..Default::default()
        }
    }

    /// Two floors: a main panel on the first, panel B on the second, a feeder between them.
    fn two_floors() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        for (n, e, r) in
            [("One", 0.0, [0.0, 0.0, 1200.0, 1200.0]), ("Two", 12.0, [2000.0, 0.0, 3200.0, 1200.0]), ("Three", 24.0, [4000.0, 0.0, 5200.0, 1200.0])]
        {
            sp.levels.push(
                serde_json::from_value(json!({"name": n, "elevation_ft": e, "height_ft": 12.0, "ceiling_ft": 9.0, "region": r})).expect("level"),
            );
        }
        sp.mep.equipment = vec![gear("MAIN", "One", [100.0, 100.0]), gear("B", "Two", [2300.0, 400.0])];
        sp.mep.feeders = vec![cadaddy_walk3d::Feeder {
            tag: "F1".into(),
            from: "MAIN".into(),
            to: "B".into(),
            conduits: 2,
            conduit_in: 2.0,
            entry: "top".into(),
            from_entry: "top".into(),
            ..Default::default()
        }];
        let (runs, left) = super::super::gearbuild::feeder_runs(&sp);
        assert!(left.is_empty(), "{left:?}");
        sp.mep.runs = runs;
        sp
    }

    fn ends_on(sp: &BuildingSpec, tag: &str) -> bool {
        let g = sp.mep.equipment.iter().find(|e| e.tag == tag).expect("gear");
        sp.mep
            .runs
            .iter()
            .filter(|r| super::super::ugplan::is_feeder_run(r))
            .any(|r| r.level == g.level && r.points.iter().any(|p| near(g, *p, 1e-6)))
    }

    #[test]
    fn routed_feeders_are_kept_while_their_gear_stays() {
        let mut sp = two_floors();
        let before = sp.mep.runs.clone();
        let f = follow(&mut sp, Mode::Moved);
        assert!(!f.changed(), "{f:?}");
        assert_eq!(f.kept, 1);
        assert_eq!(sp.mep.runs, before);
        assert_eq!(check(&sp), (0, 0));
    }

    #[test]
    fn a_panel_moved_across_its_room_gets_its_feeder_routed_to_where_it_is_now() {
        let mut sp = two_floors();
        sp.mep.runs.iter_mut().for_each(|r| r.id = format!("R{}", r.points.len()));
        let n = sp.mep.runs.len();
        // B goes to the far side of its room.
        sp.mep.equipment[1] = gear("B", "Two", [3000.0, 1000.0]);
        assert!(!ends_on(&sp, "B"));
        assert_eq!(check(&sp), (1, 0), "the stored route ends where B was");
        let f = follow(&mut sp, Mode::Moved);
        assert_eq!(f.rerouted.len(), 1, "{f:?}");
        assert!(f.rerouted[0].0.contains("F1 MAIN→B") && f.rerouted[0].1.contains("moved"), "{f:?}");
        assert_eq!((f.off_gear_before, f.off_gear_after), (1, 0));
        assert!(ends_on(&sp, "B") && ends_on(&sp, "MAIN"));
        assert_eq!(sp.mep.runs.len(), n);
        assert!(sp.mep.runs.iter().all(|r| !r.id.is_empty()), "the routed-again runs keep the ids");
        // As routing it from scratch would.
        let (fresh, _) = super::super::gearbuild::feeder_runs(&sp);
        let shape =
            |v: &[MepRun]| v.iter().map(|r| (r.level.clone(), format!("{:?} {:?} {:?}", r.points, r.bottoms_ft, r.top_ft))).collect::<BTreeSet<_>>();
        assert_eq!(shape(&sp.mep.runs), shape(&fresh));
        // A second pass finds nothing to do.
        assert!(!follow(&mut sp, Mode::Moved).changed());
    }

    #[test]
    fn a_panel_moved_within_its_own_outline_keeps_its_route() {
        let mut sp = two_floors();
        let before = sp.mep.runs.clone();
        sp.mep.equipment[1] = gear("B", "Two", [2302.0, 401.0]);
        assert!(!follow(&mut sp, Mode::Moved).changed());
        assert_eq!(sp.mep.runs, before);
    }

    #[test]
    fn a_route_into_its_gear_made_by_hand_is_kept_as_given() {
        let mut sp = two_floors();
        // Fewer, simpler runs than the router makes, each end in or just beside the gear.
        let made = |level: &str, pts: Vec<[f64; 2]>, z: f64, top: Option<f64>| MepRun {
            top_ft: top,
            size_source: "feeder schedule".into(),
            height_source: "proposed".into(),
            ..super::super::printcond::run(super::super::gearbuild::FEEDER_SYSTEM, level, "feeder F1 MAIN→B", pts, 2.0, z)
        };
        sp.mep.runs = vec![
            made("One", vec![[100.0, 100.0]], 6.0, Some(10.0)),
            made("One", vec![[100.0, 100.0], [318.0, 100.0], [318.0, 400.0]], 10.0, None),
            made("One", vec![[318.0, 400.0]], 10.0, Some(22.0)),
            made("Two", vec![[2318.0, 400.0], [2300.0, 400.0]], 10.0, None),
            made("Two", vec![[2300.0, 400.0]], 6.0, Some(10.0)),
        ];
        let given = sp.mep.runs.clone();
        let f = follow(&mut sp, Mode::Moved);
        assert!(!f.changed(), "{f:?}");
        assert_eq!(sp.mep.runs, given);
    }

    #[test]
    fn a_panel_moved_to_another_floor_gets_a_riser_to_that_floor() {
        let mut sp = two_floors();
        sp.mep.equipment[1] = gear("B", "Three", [4300.0, 400.0]);
        let f = follow(&mut sp, Mode::Moved);
        assert_eq!(f.rerouted.len(), 1, "{f:?}");
        assert!(ends_on(&sp, "B"));
        let feeder = |r: &&MepRun| super::super::ugplan::is_feeder_run(r);
        assert!(sp.mep.runs.iter().filter(feeder).any(|r| r.level == "Three"), "on the new floor");
        // The riser now passes Two's floor and goes on up; nothing stops at Two's ceiling route.
        assert!(!sp.mep.runs.iter().filter(feeder).any(|r| r.level == "Two" && r.points.len() >= 2), "no route across Two any more");
    }

    #[test]
    fn a_panel_moved_a_floor_under_its_old_place_isnt_taken_for_the_riser_passing_it() {
        let mut sp = two_floors();
        // B on Three, routed there; then moved down to Two at the same place on plan: the riser
        // up to Three passes Two's floor right beside it, but no end of the route is at B.
        sp.mep.equipment[1] = gear("B", "Three", [4300.0, 400.0]);
        assert!(follow(&mut sp, Mode::Moved).changed());
        sp.mep.equipment[1] = gear("B", "Two", [2300.0, 400.0]);
        assert_eq!(check(&sp), (1, 0));
        let f = follow(&mut sp, Mode::Moved);
        assert_eq!(f.rerouted.len(), 1, "{f:?}");
        assert!(!sp.mep.runs.iter().any(|r| r.level == "Three"), "nothing goes up to Three any more");
        assert!(ends_on(&sp, "B"));
    }

    #[test]
    fn a_feeder_with_no_route_left_is_routed_once_the_models_feeders_were() {
        let mut sp = two_floors();
        sp.mep.equipment.push(gear("C", "Two", [2900.0, 200.0]));
        sp.mep.feeders.push(cadaddy_walk3d::Feeder {
            tag: "F2".into(),
            from: "MAIN".into(),
            to: "C".into(),
            conduits: 1,
            conduit_in: 1.0,
            ..Default::default()
        });
        assert_eq!(check(&sp).1, 1);
        // bspec.set and model.build follow the routes there are; they don't add one.
        let f = follow(&mut sp.clone(), Mode::Moved);
        assert!(!f.changed() && f.unrouted_after == 1, "{f:?}");
        let f = follow(&mut sp, Mode::Placed);
        assert!(f.rerouted.iter().any(|(n, w)| n.contains("F2") && w.contains("not routed")), "{f:?}");
        assert!(ends_on(&sp, "C"));
        assert_eq!(f.unrouted_after, 0);
        // A raceway on the feeder system that is no listed feeder's route doesn't count as routed.
        let mut drawn = two_floors();
        drawn.mep.runs =
            vec![super::super::printcond::run(super::super::gearbuild::FEEDER_SYSTEM, "One", "E-1", vec![[0.0, 0.0], [10.0, 0.0]], 1.0, 9.0)];
        assert!(!follow(&mut drawn, Mode::Placed).changed());
        // Never routed at all: left for the job's feeder routes, unless asked for.
        let mut none = two_floors();
        none.mep.runs.clear();
        assert!(!follow(&mut none, Mode::Placed).changed());
        assert!(none.mep.runs.is_empty());
        assert!(follow(&mut none, Mode::All).changed());
        assert!(ends_on(&none, "B"));
    }

    #[test]
    fn a_drawn_or_edited_route_is_kept_while_it_lands_on_its_gear() {
        let mut sp = two_floors();
        // A person's edit of the routed feeder: other points, but out of and into each piece of gear.
        for r in &mut sp.mep.runs {
            r.edited_by = "user".into();
            if r.points.len() >= 2 {
                r.points.insert(1, [r.points[0][0], r.points[0][1] + 24.0]);
                r.bottoms_ft.push(r.bottoms_ft.first().copied().unwrap_or(10.0));
            }
        }
        let edited = sp.mep.runs.clone();
        assert!(!follow(&mut sp, Mode::Moved).changed());
        assert_eq!(sp.mep.runs, edited);
        // The gear moved away from it: routed again.
        sp.mep.equipment[0] = gear("MAIN", "One", [900.0, 900.0]);
        let f = follow(&mut sp, Mode::Moved);
        assert!(f.rerouted.iter().any(|(_, w)| w.contains("(edited)")), "{f:?}");
        assert!(ends_on(&sp, "MAIN"));
    }

    #[test]
    fn a_route_drawn_or_entered_by_hand_is_kept_as_given_and_counted_off_its_gear() {
        let mut sp = two_floors();
        sp.mep.runs = vec![super::super::printcond::run(
            super::super::gearbuild::FEEDER_SYSTEM,
            "One",
            "feeder F1 MAIN→B",
            vec![[100.0, 100.0], [700.0, 100.0]],
            2.0,
            10.0,
        )];
        let given = sp.mep.runs.clone();
        let f = follow(&mut sp, Mode::Placed);
        assert!(!f.changed(), "{f:?}");
        assert_eq!((f.kept, f.off_gear_before, f.off_gear_after), (1, 1, 1));
        assert_eq!(sp.mep.runs, given);
        assert_eq!(check(&sp), (1, 0));
    }

    /// Q6664: P1 on the first floor beside stack 2, P2 on the third over stack 1; two stacks of
    /// three copies of typical panel U, a feeder from each source's schedule row; no route yet.
    fn two_stacks() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        for (k, n) in ["Level 1", "Level 2", "Level 3"].iter().enumerate() {
            sp.levels.push(
                serde_json::from_value(json!({"name": n, "elevation_ft": 10.0 * k as f64, "height_ft": 10.0, "ceiling_ft": 9.0})).expect("level"),
            );
        }
        sp.mep.equipment = vec![gear("P1", "Level 1", [3100.0, 100.0]), gear("P2", "Level 3", [900.0, 100.0])];
        for level in 1..=3 {
            for (stack, x) in [(1, 1000.0), (2, 3000.0)] {
                sp.mep.equipment.push(gear(&format!("U-{level}0{stack}"), &format!("Level {level}"), [x, 600.0]));
            }
        }
        sp.mep.panel_schedules = vec![cadaddy_walk3d::PanelSchedule { panel: "U".into(), typical_of: Some(6), ..Default::default() }];
        sp.mep.feeders = ["P1", "P2"]
            .iter()
            .map(|p| cadaddy_walk3d::Feeder {
                tag: "604".into(),
                from: (*p).into(),
                to: "U".into(),
                conduits: 1,
                conduit_in: 1.25,
                source: format!("made-up feeder schedule; panel {p} schedule circuit 1"),
                ..Default::default()
            })
            .collect();
        sp
    }

    /// The panels' feed as entered: U1a <- P1, U2a <- U1a, U3a <- U2a; U3b <- P2, U2b <- U3b, U1b <- U2b.
    fn record_feeds(sp: &mut BuildingSpec) {
        for (tag, from) in [("U-101", "P1"), ("U-201", "U-101"), ("U-301", "U-201"), ("U-302", "P2"), ("U-202", "U-302"), ("U-102", "U-202")] {
            if let Some(e) = sp.mep.equipment.iter_mut().find(|e| e.tag == tag) {
                e.properties.insert("fed_from".into(), from.into());
            }
        }
    }

    fn laid(sp: &BuildingSpec) -> BTreeSet<String> {
        sp.mep.runs.iter().filter(|r| super::super::ugplan::is_feeder_run(r)).map(|r| r.source_sheet.clone()).collect()
    }

    const AS_FED: [&str; 2] = ["feeder 604 P1→U-101→U-201→U-301", "feeder 604 P2→U-302→U-202→U-102"];

    #[test]
    fn a_proposed_route_against_the_panels_recorded_feed_is_routed_again_as_the_feed_says() {
        let mut sp = two_stacks();
        // Routed before the feed was recorded: the nearest stacks (P1 feeding stack 2 bottom up,
        // P2 stack 1 top down), each stack fed from the wrong source and the wrong end.
        sp.mep.runs = super::super::gearbuild::feeder_runs(&sp).0;
        let before = laid(&sp);
        assert!(before.contains("feeder 604 P1→U-102→U-202→U-302"), "{before:#?}");
        record_feeds(&mut sp);
        // Each source's route lands on the other stack than its feed's.
        assert_eq!(check(&sp), (2, 0));
        assert_eq!(off_feed_count(&sp), 2);
        let f = follow(&mut sp, Mode::Moved);
        assert_eq!(f.rerouted.len(), 2, "{f:?}");
        assert!(f.rerouted.iter().all(|(_, why)| why.starts_with("its route doesn't follow the panels' feed")), "{f:?}");
        assert_eq!((f.off_feed_before, f.off_feed_after), (2, 0), "{f:?}");
        assert_eq!(laid(&sp), AS_FED.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
        assert!(f.line().is_some_and(|l| l.contains("2 routed again")), "{:?}", f.line());
        // Followed: a second pass keeps it.
        let again = follow(&mut sp, Mode::Moved);
        assert!(!again.changed() && again.disagree.is_empty(), "{again:?}");
    }

    #[test]
    fn a_feeder_is_never_laid_beside_a_route_kept_as_given_into_its_copies() {
        let mut sp = two_stacks();
        record_feeds(&mut sp);
        // Stack 1's raceway drawn on the prints (no listed feeder's route): kept as given.
        let drawn: Vec<MepRun> = super::super::gearbuild::feeder_runs(&sp)
            .0
            .into_iter()
            .filter(|r| r.source_sheet == AS_FED[0])
            .map(|r| MepRun { source_sheet: "E-2".into(), size_source: "drawn".into(), height_source: "drawn".into(), ..r })
            .collect();
        assert!(!drawn.is_empty());
        sp.mep.runs = drawn.clone();
        let f = follow(&mut sp, Mode::All);
        // Stack 2 is routed; P1's feeder into stack 1 is not laid beside the drawn raceway.
        assert_eq!(f.rerouted.len(), 1, "{f:?}");
        assert!(f.rerouted[0].0.starts_with("604 P2→U"), "{f:?}");
        assert_eq!(f.disagree.len(), 1, "{f:?}");
        assert!(f.disagree[0].0.starts_with("604 P1→U") && f.disagree[0].1.contains("already takes a route kept as given"), "{f:?}");
        assert!(f.line().is_some_and(|l| l.contains("route and feed data disagree")), "{:?}", f.line());
        assert_eq!(laid(&sp), BTreeSet::from(["E-2".to_string(), AS_FED[1].to_string()]));
        assert!(drawn.iter().all(|r| sp.mep.runs.contains(r)), "the drawn raceway is kept as given");

        // A listed feeder's route entered by hand along other chains: kept as given, and said.
        let mut sp = two_stacks();
        sp.mep.runs = super::super::gearbuild::feeder_runs(&sp).0.into_iter().map(|r| MepRun { height_source: "entered".into(), ..r }).collect();
        let given = sp.mep.runs.clone();
        record_feeds(&mut sp);
        let f = follow(&mut sp, Mode::All);
        assert!(!f.changed(), "{f:?}");
        assert_eq!(sp.mep.runs, given);
        assert_eq!((f.off_feed_before, f.off_feed_after, f.disagree.len()), (2, 2, 2), "{f:?}");
    }

    #[test]
    fn a_proposed_route_whose_gear_is_gone_is_not_kept() {
        let mut sp = two_floors();
        sp.mep.equipment.retain(|e| e.tag != "B");
        let f = follow(&mut sp, Mode::Moved);
        assert_eq!(f.dropped.len(), 1, "{f:?}");
        assert!(!sp.mep.runs.iter().any(super::super::ugplan::is_feeder_run));
    }
}
