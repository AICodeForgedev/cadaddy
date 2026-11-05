//! Branch wiring as installed (Q6206): what the routing needs beyond its circuits as the plans
//! tag them.
//!
//! - **Circuit labels with no tag on their level** ([`resolve_label`]): the label's panel and
//!   number read through the panel schedules (the panel that lists that number), a tag of the
//!   same circuit on another level's plan, and the riser (a feeder to or from the panel, the
//!   panel in the equipment), in that order; a label whose "panel" none of them names is no
//!   circuit (an equipment tag lettered beside a device) and stays unresolved with the reason.
//! - **Switch loops** ([`switch_leg_makeup`]): a leg to a switch's box carries the line, the
//!   switched leg(s) and the grounded conductor (NEC 404.2(C)), 3-way and 4-way travelers per
//!   the rules data, and the ground.
//! - **Makeup** ([`panel_end_makeup_ft`]): at a panel, each conductor runs down the can to its
//!   breaker (the rules' share of the can's height) and takes a box's makeup at the
//!   termination.
//! - **Shared raceway** ([`shared_runs`]): where homeruns of one panel run on one path (the
//!   corridor's lane), the segment is one raceway carrying all their circuits, up to the rules'
//!   current-carrying conductors per raceway (more take another raceway beside it); each such
//!   raceway is sized by fill (NEC Ch. 9) and its conductors by the 310.15(C)(1) adjustment.
//! - **Proposed circuits** ([`spare_circuits`]): a power device the prints give no circuit is
//!   joined to the nearest panel with a spare, marked not drawn and priced apart, with a review
//!   item and an RFI line.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::tags::ElementTag;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Part, RouteRules};

/// The rules data the gap work reads (in `conduitroute.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct GapRules {
    pub tag_reading: TagReading,
    pub switch_loop: SwitchLoop,
    pub shared: Shared,
    pub proposed: Proposed,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TagReading {
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SwitchLoop {
    /// Insulated conductors on a leg to a single-pole switch, a 3-way's and a 4-way's.
    pub single: u32,
    pub three_way: u32,
    pub four_way: u32,
    /// The kinds that are switches (their box takes the loop).
    pub kinds: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Shared {
    /// Most current-carrying conductors one shared raceway carries.
    pub max_ccc: u32,
    /// Two homerun segments are on one path within this (inches, across and in height).
    pub same_path_in: f64,
    /// Build each stretch as its one raceway (the homeruns cut to join it), and why it is off.
    pub merge: bool,
    pub merge_source: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Proposed {
    /// The device kinds (words in the kind) that are power branch devices.
    pub kinds: Vec<String>,
    /// Schedule descriptions that mark a spare breaker, and an empty space.
    pub spare_words: Vec<String>,
    pub space_words: Vec<String>,
    pub source: String,
}

pub(crate) fn rules() -> &'static GapRules {
    static R: std::sync::OnceLock<GapRules> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let v: Value = serde_json::from_str(super::RULES).unwrap_or_default();
        v.get("branchgap").cloned().and_then(|g| serde_json::from_value(g).ok()).unwrap_or_default()
    })
}

// ─── Circuit labels with no tag on their level ───────────────────────────────

/// How a label was resolved: where (kind of source, its words) and the circuit's conductors
/// where a document gives them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Resolved {
    /// `schedule`, `plan` (another level's tag), `riser`.
    pub by: &'static str,
    pub source: String,
    pub conductors: Option<String>,
    /// The tag on another level it is (its id), when that is how.
    pub other_tag: Option<String>,
}

/// What names `panel` as a panel: its schedule, a feeder to or from it (the riser), the
/// panel among the equipment.
pub(crate) fn panel_evidence(sp: &BuildingSpec, panel: &str, rl: &RouteRules) -> Option<String> {
    let p = panel.trim();
    if p.is_empty() {
        return None;
    }
    if let Some(s) = sp.mep.panel_schedules.iter().find(|s| s.panel.trim().eq_ignore_ascii_case(p)) {
        return Some(format!("panel {} schedule{}", s.panel.trim(), if s.source.is_empty() { String::new() } else { format!(" ({})", s.source) }));
    }
    if let Some(f) = sp.mep.feeders.iter().find(|f| f.to.trim().eq_ignore_ascii_case(p) || f.from.trim().eq_ignore_ascii_case(p)) {
        return Some(format!("riser: feeder {} {} → {}", f.tag, f.from, f.to));
    }
    sp.mep
        .equipment
        .iter()
        .find(|e| {
            let k = format!("{} {}", e.kind, e.title).to_ascii_lowercase();
            e.tag.trim().eq_ignore_ascii_case(p) && rl.gear.kinds.iter().any(|w| k.contains(w.as_str()))
        })
        .map(|e| format!("riser: {} {} in the equipment", e.kind, e.tag))
}

/// The conductors a schedule row's wire size gives (`12` → `2#12 + 1#12G`; a two- or
/// three-pole breaker's hots one per pole).
pub(crate) fn schedule_conductors(wire: &str, poles: u32) -> Option<String> {
    let w: String = wire.trim().trim_start_matches('#').chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
    if w.is_empty() || !w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let hots = poles.clamp(1, 3).max(2);
    Some(format!("{hots}#{w} + 1#{w}G"))
}

/// The circuit a label printed beside devices on `level` is when no circuit tag on that level
/// carries it (Q6206): its panel's schedule listing its number, else a tag of it on another
/// level (its panel named by a schedule, the riser or the equipment), else the panel the riser
/// names. `others`: the circuit tags of the label on other levels. Err: why not (no panel).
pub(crate) fn resolve_label(sp: &BuildingSpec, label: &str, others: &[&ElementTag], rl: &RouteRules) -> Result<Resolved, String> {
    let Some((panel, nums)) = super::super::printcond::parse_circuit(label) else {
        return Err(format!("`{label}` is not a circuit label (panel-number)"));
    };
    let first = nums.split(',').next().unwrap_or_default().trim().to_string();
    let src = &rules().tag_reading.source;
    let sched = sp.mep.panel_schedules.iter().find(|s| s.panel.trim().eq_ignore_ascii_case(&panel));
    if let Some(row) = sched.and_then(|s| s.circuits.iter().find(|c| c.number.trim() == first)) {
        let sheet = sched.map(|s| s.source.clone()).unwrap_or_default();
        let what = if row.description.trim().is_empty() { String::new() } else { format!(" ({})", row.description.trim()) };
        return Ok(Resolved {
            by: "schedule",
            source: format!(
                "panel {panel} schedule{}: circuit {first}{what}; {src}",
                if sheet.is_empty() { String::new() } else { format!(" on {sheet}") }
            ),
            conductors: schedule_conductors(&row.wire, row.poles),
            other_tag: others.first().map(|t| t.id.clone()),
        });
    }
    let evidence = panel_evidence(sp, &panel, rl);
    if let (Some(ev), Some(t)) = (&evidence, others.first()) {
        let sheet = t.fields.get("sheet").map(|f| f.value.clone()).unwrap_or_default();
        return Ok(Resolved {
            by: "plan",
            source: format!(
                "circuit {label} tagged on {}{} (panel: {ev}); {src}",
                t.level,
                if sheet.is_empty() { String::new() } else { format!(", sheet {sheet}") }
            ),
            conductors: t.fields.get("conductors").map(|f| f.value.clone()).filter(|c| !c.trim().is_empty()),
            other_tag: Some(t.id.clone()),
        });
    }
    match evidence {
        Some(ev) => {
            Ok(Resolved { by: "riser", source: format!("{ev}: circuit {first} not in a schedule read; {src}"), conductors: None, other_tag: None })
        }
        None => Err(format!("no panel schedule, riser or panel in the equipment names a panel {panel}: not read as a circuit")),
    }
}

/// A circuit tag made for a resolved label: on `level` (a typical room's own when `room`).
pub(crate) fn resolved_tag(label: &str, level: &str, room: Option<&str>, at_ft: [f64; 2], r: &Resolved, devices: usize) -> ElementTag {
    let part = |s: &str| s.trim().chars().map(|c| if matches!(c, '/' | '#' | '@') { '-' } else { c }).collect::<String>();
    let id = match room {
        Some(rm) => format!("{}{}:room-{}:{}", super::super::printcond::CIRCUIT_TAG, part(level), part(rm), part(label)),
        None => format!("{}{}{RESOLVED_PART}{}", super::super::printcond::CIRCUIT_TAG, part(level), part(label)),
    };
    let mut t = ElementTag { id, kind: "circuit".into(), level: level.to_string(), at_ft, ..ElementTag::default() };
    let panel = super::super::printcond::parse_circuit(label).map(|(p, _)| p).unwrap_or_default();
    t.set("circuit", label.trim(), "plan");
    t.set("panel", panel, r.by);
    if let Some(rm) = room {
        t.set("room", rm, "computed");
    }
    t.set("devices", devices.to_string(), "computed");
    match &r.conductors {
        Some(c) => t.set("conductors", c.clone(), r.by),
        None => t.set("conductors", super::super::printcond::DEFAULT_CONDUCTORS, "default"),
    }
    t.set(RESOLVED, r.source.clone(), r.by);
    t
}

/// What marks a circuit tag the routing made for a label with no tag on its level.
pub(crate) const RESOLVED: &str = "resolved";
const RESOLVED_PART: &str = ":resolved:";

// ─── Switch loops ────────────────────────────────────────────────────────────

/// How many travelers a switch has: 0 single-pole, 1 a 3-way, 2 a 4-way (a kind that says so,
/// else its mates: one 3-way mate, or two for a 4-way between them).
pub(crate) fn switch_ways(kind: &str, mates: usize) -> u8 {
    let k = kind.to_ascii_lowercase().replace(['_', '-', ' '], "");
    if k.contains("4way") || k.contains("fourway") || mates >= 2 {
        2
    } else if k.contains("3way") || k.contains("threeway") || mates == 1 {
        1
    } else {
        0
    }
}

/// Whether a device's kind is a switch (its box takes the switch loop).
pub(crate) fn is_switch(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase().replace('_', " ");
    rules().switch_loop.kinds.iter().any(|w| k.contains(w.as_str()))
}

/// The conductors on a leg into a switch's box: the rules' insulated count for its ways (line,
/// switched leg(s), travelers and the grounded conductor) at the circuit's hot size, and its
/// ground.
pub(crate) fn switch_leg_makeup(hot: &str, ways: u8) -> String {
    let sl = &rules().switch_loop;
    let n = match ways {
        0 => sl.single,
        1 => sl.three_way,
        _ => sl.four_way,
    }
    .clamp(2, 8);
    format!("{n}#{hot} + 1#{hot}G")
}

// ─── Makeup ──────────────────────────────────────────────────────────────────

/// Each conductor's makeup where it ends in a panel `height_ft` tall: down the can to its
/// breaker (the rules' share of the height) and a box's makeup at the termination; the rules'
/// fixed panel makeup when the height isn't known.
pub(crate) fn panel_end_makeup_ft(height_ft: Option<f64>) -> f64 {
    let ar = super::super::mepatoms::rules();
    match height_ft.filter(|h| h.is_finite() && *h > 0.0 && *h < 20.0) {
        Some(h) if ar.panel_makeup_height_share > 0.0 => h * ar.panel_makeup_height_share + ar.makeup_ft.max(0.0),
        _ => ar.panel_makeup_ft.max(ar.makeup_ft),
    }
}

// ─── Shared raceway ──────────────────────────────────────────────────────────

/// One homerun as the shared-raceway pass sees it: its run's plan points and heights (feet
/// above its floor, one per segment), its conductors and current-carrying count.
#[derive(Clone, Debug)]
pub(crate) struct HomeSeg {
    pub run: usize,
    pub key: String,
    pub pts: Vec<[f64; 2]>,
    pub bot: Vec<f64>,
    pub parts: Vec<Part>,
    pub ccc: u32,
    pub trip_a: f64,
    pub labels: Vec<String>,
}

/// A stretch of path several homeruns share: one raceway carrying their circuits.
#[derive(Clone, Debug)]
pub(crate) struct SharedRun {
    pub key: String,
    /// Plan points (two: a straight stretch) and its height above the floor, feet.
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub z_ft: f64,
    /// The homeruns (indices into the input) whose conductors it carries.
    pub members: Vec<usize>,
}

/// The stretches where homeruns of one key (a panel on a level) run on one path: collinear
/// horizontal pieces at one height (within the rules' tolerance) overlapping along the path.
/// Each stretch's homeruns are packed into raceways of at most the rules' current-carrying
/// conductors, in the order given (first fit, each homerun keeping its raceway along a path);
/// a raceway that carries two or more homeruns is a shared run. `upf`: drawing units per foot.
pub(crate) fn shared_runs(homes: &[HomeSeg], upf: f64) -> Vec<SharedRun> {
    let sh = &rules().shared;
    let tol = (sh.same_path_in.max(0.0) / 12.0 * upf).max(1e-6);
    let cap = sh.max_ccc.max(1);
    // Each horizontal straight piece: (key, axis, across, z, from, to, homerun).
    struct Piece {
        across: f64,
        z: f64,
        lo: f64,
        hi: f64,
        home: usize,
    }
    let mut lines: BTreeMap<(String, u8), Vec<Piece>> = BTreeMap::new();
    for (hi, h) in homes.iter().enumerate().take(super::MAX_RUNS) {
        for (i, w) in h.pts.windows(2).enumerate().take(4_000) {
            let [a, b] = w else { continue };
            let z = h.bot.get(i).copied().unwrap_or(f64::NAN);
            if !z.is_finite() {
                continue;
            }
            let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
            let (axis, across, lo, hi_) = if dy <= tol && dx > tol {
                (0u8, (a[1] + b[1]) / 2.0, a[0].min(b[0]), a[0].max(b[0]))
            } else if dx <= tol && dy > tol {
                (1u8, (a[0] + b[0]) / 2.0, a[1].min(b[1]), a[1].max(b[1]))
            } else {
                continue;
            };
            lines.entry((h.key.clone(), axis)).or_default().push(Piece { across, z, lo, hi: hi_, home: hi });
        }
    }
    let mut out = Vec::new();
    for ((key, axis), mut ps) in lines {
        // Pieces on one path: the same line across and the same height.
        ps.sort_by(|a, b| (a.across, a.z, a.lo).partial_cmp(&(b.across, b.z, b.lo)).unwrap_or(std::cmp::Ordering::Equal));
        let mut start = 0;
        while start < ps.len() {
            let Some(p0) = ps.get(start) else { break };
            let (c0, z0) = (p0.across, p0.z);
            let mut end = start + 1;
            while ps.get(end).is_some_and(|p| (p.across - c0).abs() <= tol && (p.z - z0).abs() * upf <= tol) {
                end += 1;
            }
            let group: Vec<&Piece> = ps.get(start..end).map(|g| g.iter().collect()).unwrap_or_default();
            start = end;
            if group.iter().map(|p| p.home).collect::<BTreeSet<_>>().len() < 2 {
                continue;
            }
            // The elementary stretches between every end along the path.
            let mut cuts: Vec<f64> = group.iter().flat_map(|p| [p.lo, p.hi]).collect();
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() <= tol);
            // Each homerun's raceway on this path (first fit by its current-carrying conductors
            // over its whole stretch).
            let mut lanes: Vec<Vec<(f64, f64, u32)>> = Vec::new();
            let mut lane_of: BTreeMap<usize, usize> = BTreeMap::new();
            let mut by_home: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
            for p in &group {
                let e = by_home.entry(p.home).or_insert((p.lo, p.hi));
                e.0 = e.0.min(p.lo);
                e.1 = e.1.max(p.hi);
            }
            for (&hm, &(lo, hi)) in &by_home {
                let n = homes.get(hm).map_or(0, |h| h.ccc);
                let fits = |lane: &Vec<(f64, f64, u32)>| {
                    cuts.windows(2).filter(|w| matches!(w, [a, b] if *a < hi - tol && *b > lo + tol)).all(|w| {
                        let mid = (w.first().copied().unwrap_or(0.0) + w.get(1).copied().unwrap_or(0.0)) / 2.0;
                        lane.iter().filter(|(l, h, _)| *l <= mid && mid <= *h).map(|x| x.2).sum::<u32>() + n <= cap
                    })
                };
                let li = match lanes.iter().position(fits) {
                    Some(i) => i,
                    None => {
                        lanes.push(Vec::new());
                        lanes.len() - 1
                    }
                };
                if let Some(l) = lanes.get_mut(li) {
                    l.push((lo, hi, n));
                }
                lane_of.insert(hm, li);
            }
            // Each lane's stretches carried by two or more homeruns, joined where the same set
            // carries on.
            for li in 0..lanes.len() {
                let mut cur: Option<(f64, f64, Vec<usize>)> = None;
                for w in cuts.windows(2) {
                    let [a, b] = w else { continue };
                    let mid = (a + b) / 2.0;
                    let mut on: Vec<usize> = group
                        .iter()
                        .filter(|p| lane_of.get(&p.home) == Some(&li) && p.lo <= mid && mid <= p.hi)
                        .map(|p| p.home)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    on.sort_unstable();
                    match cur.as_mut() {
                        Some(c) if c.2 == on && (c.1 - a).abs() <= tol => c.1 = *b,
                        _ => {
                            if let Some(c) = cur.take().filter(|c| c.2.len() >= 2) {
                                out.push(stretch(&key, axis, c0, z0, c));
                            }
                            cur = Some((*a, *b, on));
                        }
                    }
                }
                if let Some(c) = cur.take().filter(|c| c.2.len() >= 2) {
                    out.push(stretch(&key, axis, c0, z0, c));
                }
            }
        }
    }
    out
}

fn stretch(key: &str, axis: u8, across: f64, z: f64, (lo, hi, members): (f64, f64, Vec<usize>)) -> SharedRun {
    let (a, b) = if axis == 0 { ([lo, across], [hi, across]) } else { ([across, lo], [across, hi]) };
    SharedRun { key: key.to_string(), a, b, z_ft: z, members }
}

// ─── Proposed circuits ───────────────────────────────────────────────────────

/// Whether a device kind is a power branch device a circuit should feed.
pub(crate) fn is_power_device(kind: &str) -> bool {
    let k = kind.trim().to_ascii_lowercase().replace('_', " ");
    rules().proposed.kinds.contains(&k)
}

/// A panel's spare breakers by its schedule (`SPARE` rows), then its spaces: their numbers.
pub(crate) fn spare_circuits(sp: &BuildingSpec, panel: &str) -> (Vec<String>, Vec<String>) {
    let pr = &rules().proposed;
    let has = |d: &str, ws: &[String]| {
        let up = d.to_ascii_uppercase();
        ws.iter().any(|w| up.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w.as_str()))
    };
    let Some(s) = sp.mep.panel_schedules.iter().find(|s| s.panel.trim().eq_ignore_ascii_case(panel.trim())) else { return (Vec::new(), Vec::new()) };
    let spare = s.circuits.iter().filter(|c| has(&c.description, &pr.spare_words)).map(|c| c.number.trim().to_string()).take(84).collect();
    let space = s.circuits.iter().filter(|c| has(&c.description, &pr.space_words)).map(|c| c.number.trim().to_string()).take(84).collect();
    (spare, space)
}

/// One proposed circuit's report row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn proposed_row(device: &str, kind: &str, level: &str, room: &str, panel: &str, spare: Option<&str>, est_ft: f64, at: Vec2) -> Value {
    let pr = &rules().proposed;
    let breaker = match spare {
        Some(n) => format!("spare {n}"),
        None => "no spare listed: a new breaker".to_string(),
    };
    json!({
        "device": device, "kind": kind, "level": level, "room": room, "panel": panel, "breaker": breaker,
        "estimated_ft": super::r1(est_ft), "at": [super::r2(at.x), super::r2(at.y)], "drawn": false, "priced": "apart (not in the takeoff)",
        "review": format!("the {kind} in {} on {level} has no circuit on the prints: proposed on panel {panel} ({breaker}), not drawn", if room.is_empty() { "no room" } else { room }),
        "rfi": format!("Confirm the circuit for the {kind} {}on {level}: proposed panel {panel}, {breaker}.", if room.is_empty() { String::new() } else { format!("in room {room} ") }),
        "source": pr.source,
    })
}

/// A run's plan path (`pts`, one height per segment in `bot`) with the shared stretches `cuts`
/// (straight, at a height) taken out of it: the pieces left, each a path and its heights. A
/// stretch takes the part of a segment on its line and at its height (within `tol` across and
/// `tol_z_ft` in height); a piece too short to keep (one point) goes.
pub(crate) fn cut_run(pts: &[[f64; 2]], bot: &[f64], cuts: &[([f64; 2], [f64; 2], f64)], tol: f64, tol_z_ft: f64) -> Vec<(Vec<[f64; 2]>, Vec<f64>)> {
    let mut out: Vec<(Vec<[f64; 2]>, Vec<f64>)> = Vec::new();
    let Some(first) = pts.first() else { return out };
    let mut cur_p: Vec<[f64; 2]> = vec![*first];
    let mut cur_b: Vec<f64> = Vec::new();
    let lerp = |a: [f64; 2], b: [f64; 2], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    for (i, w) in pts.windows(2).enumerate().take(100_000) {
        let [a, b] = w else { continue };
        let z = bot.get(i).or(bot.last()).copied().unwrap_or(0.0);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let along_x = dy.abs() <= tol && dx.abs() > tol;
        let along_y = dx.abs() <= tol && dy.abs() > tol;
        // The stretches on this segment, as parameters along it.
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for (ca, cb, cz) in cuts {
            if (cz - z).abs() > tol_z_ft {
                continue;
            }
            let (on, t0, t1) = if along_x && (ca[1] - cb[1]).abs() <= tol && (ca[1] - a[1]).abs() <= tol {
                (true, (ca[0] - a[0]) / dx, (cb[0] - a[0]) / dx)
            } else if along_y && (ca[0] - cb[0]).abs() <= tol && (ca[0] - a[0]).abs() <= tol {
                (true, (ca[1] - a[1]) / dy, (cb[1] - a[1]) / dy)
            } else {
                (false, 0.0, 0.0)
            };
            if !on || !t0.is_finite() || !t1.is_finite() {
                continue;
            }
            let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(1.0));
            if hi > lo + 1e-9 {
                spans.push((lo, hi));
            }
        }
        spans.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for s in spans {
            match merged.last_mut() {
                Some(m) if s.0 <= m.1 + 1e-9 => m.1 = m.1.max(s.1),
                _ => merged.push(s),
            }
        }
        let seg_len = dx.hypot(dy).max(1e-12);
        let eps = tol / seg_len;
        let mut t_at = 0.0;
        for (lo, hi) in merged {
            let x = lerp(*a, *b, lo);
            if lo > t_at + eps {
                cur_p.push(x);
                cur_b.push(z);
            }
            // The piece ends at the stretch's height (its rise or drop onto it kept).
            if cur_b.last().is_none_or(|l| (l - z).abs() > 1e-9) {
                cur_p.push(x);
                cur_b.push(z);
            }
            if cur_p.len() >= 2 {
                out.push((std::mem::take(&mut cur_p), std::mem::take(&mut cur_b)));
            }
            // The next piece leaves the stretch at its height.
            let y = lerp(*a, *b, hi);
            cur_p = vec![y, y];
            cur_b = vec![z];
            t_at = hi;
        }
        if t_at < 1.0 - eps || seg_len <= tol {
            // The rest of the segment (all of it when no stretch is on it; a vertical one, no
            // plan length, keeps its height).
            cur_p.push(*b);
            cur_b.push(z);
        } else if let Some(p) = cur_p.last_mut() {
            *p = *b;
        }
    }
    if cur_p.len() >= 2 {
        out.push((cur_p, cur_b));
    }
    // A piece with no length at all (a stretch to the run's very end) goes.
    out.retain(|(p, b)| {
        let plan: f64 = p.windows(2).map(|w| if let [a, c] = w { (c[0] - a[0]).hypot(c[1] - a[1]) } else { 0.0 }).sum();
        let vert: f64 = b.windows(2).map(|w| if let [a, c] = w { (c - a).abs() } else { 0.0 }).sum();
        plan > tol || vert > tol_z_ft
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan length plus the changes of height at the corners (the router's own measure).
    fn ft(p: &[[f64; 2]], b: &[f64]) -> f64 {
        let plan: f64 = p.windows(2).map(|w| if let [a, c] = w { (c[0] - a[0]).hypot(c[1] - a[1]) } else { 0.0 }).sum();
        plan + b.windows(2).map(|w| if let [a, c] = w { (c - a).abs() } else { 0.0 }).sum::<f64>()
    }

    #[test]
    fn a_shared_stretch_is_cut_out_of_a_run_and_its_rise_kept() {
        // Up 7 ft from a box at 2 ft, along 20 ft at 9 ft, down 1 ft onto a panel at 8 ft.
        let pts = [[0.0, 0.0], [0.0, 0.0], [20.0, 0.0], [20.0, 0.0]];
        let bot = [2.0, 9.0, 8.0];
        assert!((ft(&pts, &bot) - 28.0).abs() < 1e-9);
        // The middle 10 ft shared: two pieces, 18 ft left (the rise and the drop kept).
        let cut = [([5.0, 0.0], [15.0, 0.0], 9.0)];
        let pieces = cut_run(&pts, &bot, &cut, 0.01, 0.01);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        let left: f64 = pieces.iter().map(|(p, b)| ft(p, b)).sum();
        assert!((left - 18.0).abs() < 1e-9, "{left}: {pieces:?}");
        // From the rise on: the rise stays with the box's piece.
        let cut = [([0.0, 0.0], [12.0, 0.0], 9.0)];
        let pieces = cut_run(&pts, &bot, &cut, 0.01, 0.01);
        let left: f64 = pieces.iter().map(|(p, b)| ft(p, b)).sum();
        assert!((left - 16.0).abs() < 1e-9, "the 7 ft rise, 8 ft along and the 1 ft drop: {left}: {pieces:?}");
        // A stretch to the corner where it rises (a riser through the floor): the riser kept.
        let pts = [[0.0, 0.0], [0.0, 0.0], [20.0, 0.0], [20.0, 0.0]];
        let bot = [2.0, 9.0, -3.0];
        let cut = [([15.0, 0.0], [20.0, 0.0], 9.0)];
        let left: f64 = cut_run(&pts, &bot, &cut, 0.01, 0.01).iter().map(|(p, b)| ft(p, b)).sum();
        assert!((left - (7.0 + 15.0 + 12.0)).abs() < 1e-9, "{left}");
        let pts = [[0.0, 0.0], [0.0, 0.0], [20.0, 0.0], [20.0, 0.0]];
        let bot = [2.0, 9.0, 8.0];
        // At another height, or another line: nothing cut.
        for cut in [[([5.0, 0.0], [15.0, 0.0], 9.5)], [([5.0, 1.0], [15.0, 1.0], 9.0)]] {
            let pieces = cut_run(&pts, &bot, &cut, 0.01, 0.01);
            assert_eq!(pieces.len(), 1);
            assert!((pieces.iter().map(|(p, b)| ft(p, b)).sum::<f64>() - 28.0).abs() < 1e-9);
        }
    }

    fn spec(v: Value) -> BuildingSpec {
        serde_json::from_value(v).unwrap_or_default()
    }

    #[test]
    fn a_label_resolves_by_the_schedule_then_another_levels_tag_then_the_riser() {
        let rl = super::super::rules();
        let sp = spec(json!({"version": 1, "mep": {
            "panel_schedules": [{"panel": "LP1", "source": "E-601", "circuits": [{"number": "11", "description": "RECEPTACLES", "room": "", "breaker": "20A", "poles": 1, "wire": "10"}]}],
            "feeders": [{"tag": "F2", "from": "MDP", "to": "LP2"}]
        }}));
        let r = resolve_label(&sp, "LP1-11", &[], rl).expect("schedule");
        assert_eq!((r.by, r.conductors.as_deref()), ("schedule", Some("2#10 + 1#10G")));
        assert!(r.source.contains("E-601") && r.source.contains("circuit 11"), "{}", r.source);
        // Not in the schedule, tagged on another level: that tag (its panel's schedule names it).
        let mut other = ElementTag { id: "circuit:L1:E-101:LP1-13".into(), kind: "circuit".into(), level: "L1".into(), ..ElementTag::default() };
        other.set("conductors", "2#8 + 1#8G", "plan");
        other.set("sheet", "E-101", "plan");
        let r = resolve_label(&sp, "LP1-13", &[&other], rl).expect("plan");
        assert_eq!((r.by, r.conductors.as_deref(), r.other_tag.as_deref()), ("plan", Some("2#8 + 1#8G"), Some("circuit:L1:E-101:LP1-13")));
        // Only the riser names LP2.
        assert_eq!(resolve_label(&sp, "LP2-4", &[], rl).map(|r| r.by), Ok("riser"));
        // Nothing names a panel XQ, even with a tag of the label elsewhere: no circuit.
        assert!(resolve_label(&sp, "XQ-2", &[&other], rl).is_err());
        assert!(resolve_label(&sp, "not a label", &[], rl).is_err());
    }

    #[test]
    fn switch_loops_and_makeup_come_from_the_rules() {
        assert_eq!(switch_leg_makeup("12", switch_ways("switch", 0)), "3#12 + 1#12G");
        assert_eq!(switch_leg_makeup("12", switch_ways("switch", 1)), "4#12 + 1#12G");
        assert_eq!(switch_leg_makeup("10", switch_ways("4way_switch", 0)), "5#10 + 1#10G");
        assert!(is_switch("occupancy_sensor") && is_switch("dimmer") && !is_switch("receptacle"));
        // A 4' panel: half of it down the can and a box's makeup (300.14's 6" + the 12" allowance).
        let ar = super::super::super::mepatoms::rules();
        assert!((ar.makeup_ft - 1.5).abs() < 1e-9, "{}", ar.makeup_ft);
        assert!((panel_end_makeup_ft(Some(4.0)) - 3.5).abs() < 1e-9);
        assert!((panel_end_makeup_ft(None) - ar.panel_makeup_ft).abs() < 1e-9);
        assert!(
            is_power_device("receptacle")
                && is_power_device("junction_box")
                && !is_power_device("fire_alarm_device")
                && !is_power_device("smoke_detector")
        );
        assert_eq!(schedule_conductors("#12", 1).as_deref(), Some("2#12 + 1#12G"));
        assert_eq!(schedule_conductors("8", 3).as_deref(), Some("3#8 + 1#8G"));
        assert_eq!(schedule_conductors("", 1), None);
    }

    #[test]
    fn homeruns_on_one_path_share_a_raceway_up_to_the_rules_conductors() {
        let cap = rules().shared.max_ccc;
        assert!(cap >= 6, "rules data: {cap}");
        let h = |run: usize, x0: f64, ccc: u32| HomeSeg {
            run,
            key: "l1|p".into(),
            pts: vec![[x0, 0.0], [100.0, 0.0]],
            bot: vec![9.0],
            parts: vec![(ccc, "12".into(), false)],
            ccc,
            trip_a: 20.0,
            labels: vec![format!("P-{run}")],
        };
        // Three homeruns of 2 current-carrying conductors each, joining at 0, 40 and 70 ft.
        let homes = vec![h(0, 0.0, 2), h(1, 40.0, 2), h(2, 70.0, 2)];
        let s = shared_runs(&homes, 1.0);
        let mut got: Vec<(f64, f64, Vec<usize>)> = s.iter().map(|x| (x.a[0], x.b[0], x.members.clone())).collect();
        got.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(got, vec![(40.0, 70.0, vec![0, 1]), (70.0, 100.0, vec![0, 1, 2])]);
        // More conductors than one raceway takes: the last homerun runs beside them.
        let homes = vec![h(0, 0.0, cap - 2), h(1, 40.0, 2), h(2, 70.0, 2)];
        let s = shared_runs(&homes, 1.0);
        assert!(s.iter().all(|x| x.members.iter().map(|m| homes[*m].ccc).sum::<u32>() <= cap), "{s:?}");
        assert!(s.iter().all(|x| !x.members.contains(&2)), "the third doesn't fit: {s:?}");
        // Another panel's homerun on the same line is its own.
        let mut other = h(1, 40.0, 2);
        other.key = "l1|q".into();
        assert!(shared_runs(&[h(0, 0.0, 2), other], 1.0).is_empty());
    }
}
