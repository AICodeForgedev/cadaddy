//! Plumbing as it is built (Q6161): drainage that falls and runs under the lowest slab, stacks
//! that are one riser from the floor they start on to the floor they end on, and each fixture
//! connected to its waste, vent and water lines.
//!
//! The numbers (slopes, the invert where the plans print none, how far a fixture's connection
//! reaches, which lines each kind of fixture takes) are rules data: `plumbing.json`, each with its
//! code section or marked a company default.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use cadaddy_doc::{Drawing, Handle};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_walk3d::{BuildingSpec, Level};
use serde::Deserialize;
use serde_json::{Value, json};

use super::propfill::WithNote;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    drainage_systems: Vec<String>,
    drainage_slope: Vec<Slope>,
    below_slab_invert_in: f64,
    below_slab_why: String,
    stack_join_ft: f64,
    connect_reach_ft: f64,
    needs: Vec<Need>,
    system_names: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct Slope {
    max_size_in: f64,
    in_per_ft: f64,
    source: String,
}

#[derive(Debug, Default, Deserialize)]
struct Need {
    words: Vec<String>,
    systems: Vec<String>,
}

const RULES: &str = include_str!("plumbing.json");

fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

/// Most runs and elements looked at.
const MAX_ITEMS: usize = 500_000;

/// The drainage fall IPC 704.1 sets for a pipe of `size_in`, and its source.
fn slope_for(r: &Rules, size_in: f64) -> Option<(f64, &str)> {
    r.drainage_slope.iter().find(|s| size_in <= s.max_size_in + 1e-9).map(|s| (s.in_per_ft, s.source.as_str()))
}

/// The building's levels lowest first (not the roof, not a level drawn in another file).
fn storeys(sp: &BuildingSpec) -> Vec<&Level> {
    let mut v: Vec<&Level> =
        sp.levels.iter().filter(|l| l.sheet.is_none() && l.elevation_ft.is_finite() && !super::wallgen::roof_level(&sp.levels, &l.name)).collect();
    v.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    v
}

/// The spec's drainage runs as they are installed: each horizontal sanitary or storm run falls at
/// the slope the code sets for its size, and those on the lowest storey with no height of their
/// own run under its slab (at the invert the rules data gives). What changed.
pub(crate) fn drainage(sp: &mut BuildingSpec) -> Value {
    let r = rules();
    let lowest = storeys(sp).first().map(|l| l.name.clone());
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let invert_ft = r.below_slab_invert_in / 12.0;
    let (mut sloped, mut below, mut below_ft) = (0usize, 0usize, 0.0);
    let mut by_slope: BTreeMap<String, usize> = BTreeMap::new();
    for run in sp.mep.runs.iter_mut().take(MAX_ITEMS) {
        if run.top_ft.is_some() || !r.drainage_systems.contains(&run.system) {
            continue;
        }
        if run.slope_in_per_ft <= 0.0
            && let Some((s, src)) = slope_for(&r, run.size_in[0])
        {
            run.slope_in_per_ft = s;
            sloped += 1;
            *by_slope.entry(src.to_string()).or_default() += 1;
        }
        let default = matches!(run.height_source.trim(), "" | "default");
        if default && lowest.as_deref() == Some(run.level.as_str()) && invert_ft > 0.0 {
            for b in &mut run.bottoms_ft {
                *b = -invert_ft;
            }
            run.height_source = format!("below slab: {}", r.below_slab_why);
            below += 1;
            below_ft += run.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>() / upf;
        }
    }
    json!({"sloped": sloped, "slopes": by_slope, "below_slab": below, "below_slab_ft": (below_ft * 10.0).round() / 10.0})
}

/// Where a level's plan sits: the point its plan's base is (the model stacks the levels by it),
/// else its region's corner.
fn anchor(l: &Level) -> Option<Vec2> {
    l.base.map(|[x, y]| Vec2::new(x, y)).or_else(|| l.region.map(|[x0, y0, x1, y1]| Vec2::new(x0.min(x1), y0.min(y1))))
}

/// The stacks as the building has them: a riser of one system at the same place (within the rules
/// data's `stack_join_ft`, on each storey's plan) on storeys one above the other is one riser from
/// the lowest of them to the top of the highest; the lowest storey's waste stack starts under its
/// slab, at the drain it empties into. How many stacks, and how many risers they replaced.
pub(crate) fn continuous_risers(sp: &mut BuildingSpec) -> Value {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let levels: Vec<(String, f64, f64, Vec2)> =
        storeys(sp).into_iter().filter_map(|l| Some((l.name.clone(), l.elevation_ft, l.height_ft, anchor(l)?))).collect();
    let idx = |name: &str| levels.iter().position(|(n, ..)| n == name);
    // Only piping's stacks (a conduit's riser is the electrical's).
    let pipes: Vec<String> = sp.mep.systems.iter().filter(|y| y.kind == cadaddy_walk3d::MepKind::Pipe).map(|y| y.name.clone()).collect();
    // Each riser: (run index, storey index, place on its storey's plan).
    let risers: Vec<(usize, usize, Vec2)> = sp
        .mep
        .runs
        .iter()
        .enumerate()
        .take(MAX_ITEMS)
        .filter(|(_, run)| run.top_ft.is_some() && run.points.len() == 1 && pipes.contains(&run.system))
        .filter_map(|(i, run)| {
            let li = idx(&run.level)?;
            let p = run.points.first()?;
            let (.., a) = levels.get(li)?;
            Some((i, li, Vec2::new(p[0], p[1]) - *a))
        })
        .collect();
    let reach = r.stack_join_ft * upf;
    // The riser above each one: same system, the storey above, the nearest within reach.
    let mut above: HashMap<usize, usize> = HashMap::new();
    let mut has_below: HashMap<usize, bool> = HashMap::new();
    for (i, li, p) in &risers {
        let Some(sys) = sp.mep.runs.get(*i).map(|x| x.system.as_str()) else { continue };
        let up = risers
            .iter()
            .filter(|(j, lj, _)| *lj == li + 1 && sp.mep.runs.get(*j).is_some_and(|x| x.system == sys) && !has_below.get(j).copied().unwrap_or(false))
            .map(|(j, _, q)| (*j, q.dist(*p)))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, _)) = up {
            above.insert(*i, j);
            has_below.insert(j, true);
        }
    }
    let drains: Vec<&str> = r.drainage_systems.iter().map(String::as_str).collect();
    let invert_ft = r.below_slab_invert_in / 12.0;
    let mut gone = vec![false; sp.mep.runs.len()];
    let (mut stacks, mut joined) = (0usize, 0usize);
    let mut storeys_by: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, li, _) in &risers {
        if has_below.get(i).copied().unwrap_or(false) {
            continue;
        }
        // Up the stack.
        let mut top = (*i, *li);
        let mut n = 1usize;
        while let Some(&j) = above.get(&top.0) {
            let Some(lj) = risers.iter().find(|(k, ..)| *k == j).map(|(_, l, _)| *l) else { break };
            if let Some(g) = gone.get_mut(j) {
                *g = true;
            }
            top = (j, lj);
            n += 1;
            if n > levels.len() {
                break;
            }
        }
        stacks += 1;
        *storeys_by.entry(n).or_default() += 1;
        let (Some((_, e0, ..)), Some((top_name, e1, ..))) = (levels.get(*li), levels.get(top.1)) else { continue };
        let top_rel = sp.mep.runs.get(top.0).and_then(|x| x.top_ft).unwrap_or(0.0);
        let (e0, e1, top_name) = (*e0, *e1, top_name.clone());
        let Some(run) = sp.mep.runs.get_mut(*i) else { continue };
        if n > 1 {
            joined += n - 1;
            run.top_ft = Some(e1 - e0 + top_rel);
            run.height_source = format!("stack from {} through {top_name}", run.level);
        }
        if *li == 0 && drains.contains(&run.system.as_str()) && invert_ft > 0.0 {
            if let Some(b) = run.bottoms_ft.first_mut() {
                *b = -invert_ft;
            }
            if !run.height_source.contains("below slab") {
                run.height_source = format!("{} (from below slab: {})", run.height_source, r.below_slab_why);
            }
        }
    }
    let mut k = 0;
    sp.mep.runs.retain(|_| {
        let keep = !gone.get(k).copied().unwrap_or(false);
        k += 1;
        keep
    });
    json!({"stacks": stacks, "risers_joined": joined, "stacks_by_storeys": storeys_by})
}

/// A plumbing element of the model: handle, system, level, plan bounds, and whether it is a riser.
struct PipeEl {
    handle: Handle,
    system: String,
    level: String,
    b: Bounds2,
    riser: bool,
    /// Proposed (copied work no plan draws, `status:proposed`), not drawn.
    proposed: bool,
}

/// The lines (systems) a fixture takes, by the words of its schedule name (rules data `needs`).
fn needs<'a>(r: &'a Rules, words: &str) -> &'a [String] {
    let up = format!(" {} ", words.to_ascii_uppercase());
    r.needs.iter().find(|n| n.words.iter().any(|w| up.contains(&w.to_ascii_uppercase()))).map_or(&[], |n| n.systems.as_slice())
}

fn bounds_dist(b: &Bounds2, p: Vec2) -> f64 {
    let dx = (b.min.x - p.x).max(0.0).max(p.x - b.max.x);
    let dy = (b.min.y - p.y).max(0.0).max(p.y - b.max.y);
    dx.hypot(dy)
}

/// Each plumbing fixture of the built model connected to the lines it takes (rules data `needs`:
/// a water closet its waste, vent and cold water): the nearest pipe or riser of each system on its
/// storey within the rules data's reach, its vent else the waste stack it drains to (a stack
/// vent). The fixture records each (`waste`, `vent`, `cold_water`, `hot_water`) and all of them in
/// `connects`, and each pipe the fixtures on it, so the plumbing is walked like the wiring
/// (`bim.chain`); a line it has none of within reach is listed in `unconnected`. The counts.
pub(crate) fn connect(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let pipes_sys: Vec<&str> = sp.mep.systems.iter().filter(|s| s.kind == cadaddy_walk3d::MepKind::Pipe).map(|s| s.name.as_str()).collect();
    let mut pipes: Vec<PipeEl> = Vec::new();
    let mut fixtures: Vec<(Handle, String, Vec2, String)> = Vec::new();
    // The built elements (the drawing also holds the prints' objects: many more).
    for e in d.model.iter().filter(|e| super::bimlook::made_tags(e).is_some()).take(MAX_ITEMS) {
        if let Some(sys) = super::bimlook::made_tag(e, "system").filter(|s| pipes_sys.contains(s))
            && super::bimlook::made_tag(e, "kind") == Some("pipe")
        {
            let Some(level) = super::bspec::made_level(e) else { continue };
            let Some((b, ..)) = super::propfill::extent(d, e) else { continue };
            let riser = super::bimlook::made_tag(e, "part").is_some_and(|p| p.contains("riser")) || {
                let w = b.max - b.min;
                w.x.max(w.y) <= 1.5 * upf
            };
            let proposed = super::bimlook::made_tag(e, "status").is_some_and(|v| v.eq_ignore_ascii_case(super::mep::PROPOSED));
            pipes.push(PipeEl { handle: e.handle, system: sys.to_string(), level, b, riser, proposed });
            continue;
        }
        let ps = props::read(&e.common.xdata);
        if ps.get("kind").map(Prop::text).as_deref() != Some("plumbing_fixture") {
            continue;
        }
        let Some(level) = super::bspec::made_level(e) else { continue };
        let Some((b, ..)) = super::propfill::extent(d, e) else { continue };
        let words = ["fixture", "type", "device_type", "type_mark"].iter().filter_map(|k| ps.get(k).map(Prop::text)).collect::<Vec<_>>().join(" ");
        fixtures.push((e.handle, level, b.center(), words));
    }
    let reach = r.connect_reach_ft * upf;
    let cell = reach.max(1e-9);
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(String, i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in pipes.iter().enumerate() {
        let (x0, y0) = key(p.b.min);
        let (x1, y1) = key(p.b.max);
        for x in x0..=x1.min(x0.saturating_add(64)) {
            for y in y0..=y1.min(y0.saturating_add(64)) {
                grid.entry((p.level.clone(), x, y)).or_default().push(i);
            }
        }
    }
    let nearest = |level: &str, at: Vec2, ok: &dyn Fn(&PipeEl) -> bool| -> Option<usize> {
        let (kx, ky) = key(at);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in grid.get(&(level.to_string(), kx.saturating_add(dx), ky.saturating_add(dy))).into_iter().flatten() {
                    let Some(p) = pipes.get(i) else { continue };
                    if !ok(p) {
                        continue;
                    }
                    let d = bounds_dist(&p.b, at);
                    if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i));
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    };
    let mut on_pipe: HashMap<usize, Vec<Handle>> = HashMap::new();
    let mut whole = 0usize;
    // Of the whole, those with a line on a proposed pipe (copied, not drawn).
    let mut whole_proposed = 0usize;
    let mut by_system: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut proposed_by_system: BTreeMap<String, usize> = BTreeMap::new();
    let mut writes: Vec<(Handle, Vec<(String, Handle, String)>, Vec<String>)> = Vec::new();
    for (h, level, at, words) in &fixtures {
        let want = needs(&r, words);
        if want.is_empty() {
            continue;
        }
        let mut got: Vec<(String, Handle, String)> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let mut on_proposed = false;
        for sys in want {
            let name = r.system_names.get(sys).cloned().unwrap_or_else(|| sys.to_ascii_lowercase());
            // A drawn pipe in reach first; a proposed one (copied, not drawn) only where none is.
            let mut hit = nearest(level, *at, &|p| p.system == *sys && !p.proposed)
                .or_else(|| nearest(level, *at, &|p| p.system == *sys && p.proposed))
                .map(|i| (i, format!("the nearest {sys} pipe")));
            // A vent: else the waste stack it drains to, carried up as the stack vent.
            if hit.is_none() && sys == "V" {
                hit = nearest(level, *at, &|p| p.riser && r.drainage_systems.contains(&p.system))
                    .map(|i| (i, "its waste stack's vent (a stack vent)".to_string()));
            }
            let e = by_system.entry(sys.clone()).or_default();
            e.1 += 1;
            match hit {
                Some((i, why)) => {
                    e.0 += 1;
                    if let Some(p) = pipes.get(i) {
                        if p.proposed {
                            on_proposed = true;
                            *proposed_by_system.entry(sys.clone()).or_default() += 1;
                        }
                        let why = if p.proposed { format!("{why} (proposed: copied, not drawn)") } else { why };
                        got.push((name, p.handle, why));
                        on_pipe.entry(i).or_default().push(*h);
                    }
                }
                None => missing.push(format!("{sys}: none within {} ft on its storey", r.connect_reach_ft)),
            }
        }
        whole += usize::from(missing.is_empty());
        whole_proposed += usize::from(missing.is_empty() && on_proposed);
        writes.push((*h, got, missing));
    }
    let computed = || Source::computed();
    for (h, got, missing) in &writes {
        let _ = d.modify_entity(*h, |e| {
            let mut ps = props::read(&e.common.xdata);
            for (name, ph, why) in got {
                ps.set(name, Prop::new(ph.hex(), computed()).with_note(why));
            }
            let all: Vec<String> = got.iter().map(|(_, ph, _)| ph.hex()).collect();
            if !all.is_empty() {
                ps.set("connects", Prop::new(all.join(","), computed()));
            }
            if missing.is_empty() {
                ps.remove("unconnected");
            } else {
                ps.set("unconnected", Prop::new(missing.join("; "), Source::from_doc(SourceKind::Rule, "plumbing.json needs")));
            }
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    for (i, fx) in &on_pipe {
        let Some(p) = pipes.get(*i) else { continue };
        let _ = d.modify_entity(p.handle, |e| {
            let mut ps = props::read(&e.common.xdata);
            let mut all = super::mepatoms::connects_of(e);
            all.extend(fx.iter().copied());
            all.sort_unstable();
            all.dedup();
            ps.set("connects", Prop::new(all.iter().take(2000).map(|h| h.hex()).collect::<Vec<_>>().join(","), computed()));
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
        });
    }
    let by: BTreeMap<&String, Value> = by_system
        .iter()
        .map(|(k, (c, n))| (k, json!({"connected": c, "of": n, "to_proposed": proposed_by_system.get(k).copied().unwrap_or(0)})))
        .collect();
    json!({
        "fixtures": writes.len(), "fully_connected": whole,
        "fully_connected_drawn": whole - whole_proposed, "fully_connected_with_proposed": whole_proposed,
        "by_system": by, "fixture_elements": fixtures.len(), "pipe_elements": pipes.len(),
        "proposed_pipe_elements": pipes.iter().filter(|p| p.proposed).count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::MepRun;

    fn run(system: &str, level: &str, points: Vec<[f64; 2]>, size: f64, top: Option<f64>) -> MepRun {
        serde_json::from_value(json!({
            "system": system, "level": level, "points": points, "size_in": [size, 0.0],
            "bottoms_ft": vec![0.0; points.len().saturating_sub(1).max(1)], "top_ft": top,
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn spec() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        for (i, name) in ["Level 1", "Level 2", "Level 3"].iter().enumerate() {
            let x = 10_000.0 * i as f64;
            sp.levels.push(Level {
                name: (*name).into(),
                elevation_ft: 10.0 * i as f64,
                height_ft: 10.0,
                ceiling_ft: 9.0,
                region: Some([x, 0.0, x + 5000.0, 5000.0]),
                base: Some([x + 100.0, 100.0]),
                attr: None,
                sheet: None,
            });
        }
        sp
    }

    #[test]
    fn the_lowest_floors_drainage_runs_under_its_slab_and_falls_by_its_size() {
        let mut sp = spec();
        sp.mep.runs = vec![
            run("SAN", "Level 1", vec![[0.0, 0.0], [240.0, 0.0]], 4.0, None),
            run("SAN", "Level 2", vec![[0.0, 0.0], [240.0, 0.0]], 2.0, None),
            run("CW", "Level 1", vec![[0.0, 0.0], [240.0, 0.0]], 1.0, None),
        ];
        let out = drainage(&mut sp);
        assert_eq!(out["below_slab"], 1, "{out}");
        let r = &sp.mep.runs;
        assert_eq!((r[0].slope_in_per_ft, r[1].slope_in_per_ft, r[2].slope_in_per_ft), (0.125, 0.25, 0.0));
        assert_eq!(r[0].bottoms_ft, vec![-1.5]);
        assert!(r[0].height_source.starts_with("below slab"), "{}", r[0].height_source);
        // Upstairs and pressure piping keep their heights.
        assert_eq!(r[1].bottoms_ft, vec![0.0]);
        assert_eq!(r[2].bottoms_ft, vec![0.0]);
    }

    #[test]
    fn a_stack_on_three_floors_is_one_riser_from_under_the_slab() {
        let mut sp = spec();
        // The same stack on each floor's plan (an inch off on Level 3), and a vent on Level 2 only.
        sp.mep.runs = vec![
            run("SAN", "Level 1", vec![[600.0, 600.0]], 3.0, Some(10.0)),
            run("SAN", "Level 2", vec![[10_600.0, 600.0]], 3.0, Some(10.0)),
            run("SAN", "Level 3", vec![[20_601.0, 600.0]], 3.0, Some(10.0)),
            run("V", "Level 2", vec![[10_600.0, 600.0]], 2.0, Some(10.0)),
            // Another stack 10 ft away on Level 2 only.
            run("SAN", "Level 2", vec![[10_720.0, 600.0]], 3.0, Some(10.0)),
            // A conduit's riser on two floors is the electrical's: left alone.
            run("EC", "Level 1", vec![[900.0, 600.0]], 1.0, Some(10.0)),
            run("EC", "Level 2", vec![[10_900.0, 600.0]], 1.0, Some(10.0)),
        ];
        for (name, kind) in [("SAN", "pipe"), ("V", "pipe"), ("EC", "conduit")] {
            sp.mep.systems.push(
                serde_json::from_value(json!({"name": name, "kind": kind, "layer": name, "size_in": [1.0, 0.0], "bottom_ft": 0.0}))
                    .unwrap_or_else(|e| panic!("{e}")),
            );
        }
        let out = continuous_risers(&mut sp);
        assert_eq!(out["stacks"], 3, "{out}");
        assert_eq!(out["risers_joined"], 2);
        let san: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| r.system == "SAN").collect();
        assert_eq!(san.len(), 2);
        assert_eq!(san[0].level, "Level 1");
        assert_eq!(san[0].top_ft, Some(30.0));
        assert_eq!(san[0].bottoms_ft[0], -1.5);
        assert_eq!(sp.mep.runs.iter().filter(|r| r.system == "V").count(), 1);
        assert_eq!(sp.mep.runs.iter().filter(|r| r.system == "EC" && r.top_ft == Some(10.0)).count(), 2);
    }

    #[test]
    fn a_fixture_takes_the_lines_its_kind_needs() {
        let r = rules();
        assert_eq!(needs(&r, "WATER CLOSET water closet WC-1 WC-1"), ["SAN", "V", "CW"]);
        assert_eq!(needs(&r, "LAVATORY lavatory L-1"), ["SAN", "V", "CW", "HW"]);
        assert_eq!(needs(&r, "FLOOR DRAIN FD"), ["SAN"]);
        assert!(needs(&r, "BOOSTER PUMP BP-9").contains(&"CW".to_string()));
        assert!(needs(&r, "SOMETHING ELSE").is_empty());
        assert_eq!(slope_for(&r, 3.0).map(|s| s.0), Some(0.125));
    }
}
