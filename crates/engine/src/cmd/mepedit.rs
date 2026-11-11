//! One MEP element at a time (Q6502): `mep.add`, `mep.move`, `mep.resize`, `mep.delete`,
//! `mep.set` and `mep.edits` add, move, resize, delete or change one duct or pipe run, air
//! terminal, sprinkler head or piece of equipment on a level, with its system, size, type mark,
//! properties and connection (to an existing run's end or to equipment).
//!
//! **Edits survive the documents being read again.** An edit is not written into the lists the
//! plan readers write (`mep.runs`, `mep.terminals`, `mep.equipment`: a re-read of a sheet
//! replaces them). It is kept beside them, in the spec's `mep.edits`, and applied to them on
//! every build ([`apply`]), in the order made. Each finds its element again by what it recorded
//! of it: its level, system and tag and where it stood (or the edit that added it). One whose
//! element the documents no longer have is listed as `orphaned`, never dropped.
//!
//! **Nothing changes silently.** What an edit changes goes on the element as a correction: the
//! value installed, the documents' value beside it (`design:<key>`), and the edit, its author and
//! reason as the rule (`correction:<key>`); an element an edit adds carries its provenance
//! (drawn, derived or inferred) and its basis. The build's report lists every edit and whether
//! it applied.
//!
//! **Joined for `bim.chain`.** The build connects the ducts and pipes it lays (runs that meet,
//! their fittings, risers), each air terminal and sprinkler head to the nearest run of its system
//! in reach, and each unit to the runs ending at it ([`join`]), so a walk from a register runs
//! through its ducts (an added segment among them) to the unit that serves it. The reaches are
//! rules data (`mepedit.json`) with their bases.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Source};
use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, Made, MepEdit, MepEquipment, MepKind, MepRun, MepTerminal, RunCorrection};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const RULES: &str = include_str!("mepedit.json");

/// Most edits kept and applied, points of an added run, properties set by one edit, characters
/// of a text kept (hostile input).
const MAX_EDITS: usize = 10_000;
const MAX_POINTS: usize = 1_000;
const MAX_PROPS: usize = 64;
const MAX_TEXT: usize = 200;
/// Largest duct or pipe and plan move an edit may give; equipment's largest side and height.
const MAX_SIZE_IN: f64 = 240.0;
const MAX_MOVE_FT: f64 = 10_000.0;
const MAX_EQUIPMENT_FT: f64 = 200.0;
const MAX_EQUIPMENT_HEIGHT_FT: f64 = 100.0;
/// How far under its floor an element may sit (under the slab), and how high over it at least.
const MAX_UNDER_FLOOR_FT: f64 = 10.0;
const MAX_STOREY_FT: f64 = 100.0;
/// Most segment tests the joining makes (hostile models), and most connections one element keeps.
const MAX_JOIN_TESTS: usize = 20_000_000;
const MAX_CONNECTS: usize = 2_000;
/// Edits listed per page at most.
const MAX_PAGE: u64 = 200;

/// The element a command changes, as its params name it (the docs of every command here).
macro_rules! target_doc {
    () => {
        "handle?: hex (a duct, pipe, fitting, terminal, sprinkler or equipment element the build made), run?: id (\"R3\"), tag?: equipment's (with level when tags repeat), edit?: id (of the mep.add that put it in: \"ME-2\"), element?: run|terminal|equipment (with level and at: the nearest within the rules' match reach), level?: name, at?: [x, y] (drawing units), system?: name (with at)"
    };
}
macro_rules! tail_doc {
    () => {
        "reason?: text (why: kept with the correction), by?: user|agent (default user), rebuild?: bool (default true when the model is built: model.build again)"
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mep.add", "Add MEP Element", run_add).params(concat!(
            "{element: run|duct|pipe|terminal|diffuser|register|grille|sprinkler|equipment, level?: name (default the level of `from`), system?: name (a spec system: a run's or terminal's; default `from`'s), ",
            "points?: [[x, y], ...] (a run's path, drawing units; one point with top_ft: a riser), size_in?: [w, h] | [d] (a run's or terminal's; default `from`'s, else the system's), bottom_ft?: feet (over the floor; default `from`'s end, else the system's, a terminal the level's ceiling), top_ft?: feet (a riser's top), ",
            "from?: object (what the run starts on, named as mep.move names an element, end?: start|end; its first point goes onto that run's nearer end or onto the unit's side: connected; a terminal: its system), to?: object (the same for the run's last point), ",
            "at?: [x, y] (a terminal's place or a unit's centre), kind?: text (a terminal's: diffuser, register, grille, sprinkler_pendent, sprinkler_upright, sprinkler_sidewall, vav_box; a unit's: ahu, fan_coil, exhaust_fan, pump, water_heater), ",
            "tag?: text (a unit's mark), size_ft?: [x, y] (a unit's plan size), height_ft?: feet (a unit's), systems?: [name] (a unit's), type_mark?: text, properties?: object (a unit's, name → value), ",
            "made?: drawn|derived|inferred (default inferred), basis?: text (why it is there; default the reason), source_sheet?: text, ", tail_doc!(),
            "} → {edit, status: applied, element, rebuilt, rebuild_error?, mepEdits?}: kept in the spec's mep.edits, applied on every build"
        )),
        CommandSpec::new("mep.move", "Move MEP Element", run_move).params(concat!(
            "{", target_doc!(), ", delta_ft?: [dx, dy] (feet), to?: [x, y] (drawing units: where its first point or centre goes), bottom_ft?: feet (over its floor), ", tail_doc!(),
            "} → {edit, status, element, rebuilt}"
        )),
        CommandSpec::new("mep.resize", "Resize MEP Element", run_resize).params(concat!(
            "{", target_doc!(), ", size_in?: [w, h] | [d] (a run or terminal, inches), size_ft?: [x, y] (equipment, about its centre), height_ft?: feet (equipment), ", tail_doc!(),
            "} → {edit, status, element, rebuilt}"
        )),
        CommandSpec::new("mep.delete", "Delete MEP Element", run_delete).params(concat!(
            "{", target_doc!(), ", ", tail_doc!(), "} → {edit, status, rebuilt}: left out of every build while the edit stands (mep.edits revert puts it back)"
        )),
        CommandSpec::new("mep.set", "Set MEP Element Properties", run_set).params(concat!(
            "{", target_doc!(), ", type_mark?: text (equipment), new_system?: name (a run's or terminal's system: a spec system), kind?: text (a terminal's or equipment's), properties?: object (name → value or null: null or empty removes one of equipment's), ", tail_doc!(),
            "} → {edit, status, element, rebuilt}"
        )),
        CommandSpec::new("mep.edits", "MEP Element Edits", run_edits).params(
            "{element?: run|terminal|equipment, status?: applied|orphaned, revert?: id (an edit, \"ME-3\": taken back, the element as the documents give it again), rebuild?: bool (with revert; default true when the model is built), cursor?: offset, limit?: count (50, at most 200)} → {edits: [{id, op, element, level, system, tag, at, reason, by, status: applied|orphaned, why?}], total, applied, orphaned, next_cursor?, reverted?, rebuilt?}",
        ),
    ]
}

// ─── Rules data ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub match_ft: f64,
    pub match_basis: String,
    pub equipment_reach_ft: f64,
    pub equipment_basis: String,
    pub terminal_rule: String,
    pub terminal_basis: String,
    pub sprinkler_reach_ft: f64,
    pub sprinkler_basis: String,
    pub unconnected_kinds: Vec<String>,
    pub unconnected_basis: String,
    pub terminal_size_in: [f64; 2],
    pub terminal_size_basis: String,
    pub sprinkler_size_in: [f64; 2],
    pub sprinkler_size_basis: String,
}

pub(crate) fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

// ─── The elements ────────────────────────────────────────────────────────────

/// Which list an element is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum El {
    Run,
    Terminal,
    Equipment,
}

impl El {
    fn as_str(self) -> &'static str {
        match self {
            El::Run => "run",
            El::Terminal => "terminal",
            El::Equipment => "equipment",
        }
    }

    /// From a word (`duct`, `register`, `sprinkler` and the like).
    fn parse(w: &str) -> Option<El> {
        match w.trim().to_ascii_lowercase().as_str() {
            "run" | "duct" | "pipe" | "segment" => Some(El::Run),
            "terminal" | "air_terminal" | "diffuser" | "register" | "grille" | "sprinkler" | "head" | "sprinkler_head" => Some(El::Terminal),
            "equipment" | "unit" => Some(El::Equipment),
            _ => None,
        }
    }
}

/// An element found in the spec: its list and place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    Run(usize),
    Terminal(usize),
    Equipment(usize),
}

/// The marker an added element carries: the id of the edit that added it.
fn marker(id: &str) -> String {
    format!("edit:{id}")
}

fn added_by_run(r: &MepRun) -> Option<&str> {
    r.objects.iter().find_map(|o| o.strip_prefix("edit:"))
}

fn added_by_terminal(t: &MepTerminal) -> Option<&str> {
    t.objects.iter().find_map(|o| o.strip_prefix("edit:"))
}

fn added_by_equipment(e: &MepEquipment) -> Option<&str> {
    e.sources.get("edit").map(String::as_str)
}

fn centre(e: &MepEquipment) -> [f64; 2] {
    [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Whether two level names are one level of the spec.
fn same_level(sp: &BuildingSpec, a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim()) || sp.level(a).zip(sp.level(b)).is_some_and(|(x, y)| x.name == y.name)
}

/// Where `e`'s element is in `sp` now: by the edit that added it, else by its level, system or
/// tag and where it stood (within `tol`, sheet units). `Err`: why it isn't there.
fn find(sp: &BuildingSpec, e: &MepEdit, tol: f64) -> std::result::Result<At, String> {
    let Some(el) = El::parse(&e.element) else { return Err(format!("unknown element {:?}", e.element)) };
    let sys_ok = |s: &str| e.system.is_empty() || s.eq_ignore_ascii_case(&e.system);
    let first = e.points.first().copied();
    let gone = || match first {
        Some(p) => format!(
            "no {} of {} on {} at {:.2},{:.2} any more (the documents changed there): not applied",
            el.as_str(),
            if e.system.is_empty() { &e.tag } else { &e.system },
            e.level,
            p[0],
            p[1]
        ),
        None => format!("no {} {} on {} any more: not applied", el.as_str(), e.tag, e.level),
    };
    if !e.added_by.is_empty() {
        let hit = match el {
            El::Run => sp.mep.runs.iter().position(|r| added_by_run(r) == Some(e.added_by.as_str())).map(At::Run),
            El::Terminal => sp.mep.terminals.iter().position(|t| added_by_terminal(t) == Some(e.added_by.as_str())).map(At::Terminal),
            El::Equipment => sp.mep.equipment.iter().position(|q| added_by_equipment(q) == Some(e.added_by.as_str())).map(At::Equipment),
        };
        return hit
            .ok_or_else(|| format!("the {} edit {} added is gone (reverted, or deleted by a later edit): not applied", el.as_str(), e.added_by));
    }
    match el {
        El::Run => {
            let (Some(pf), Some(pl)) = (e.points.first().copied(), e.points.last().copied()) else { return Err(gone()) };
            sp.mep
                .runs
                .iter()
                .enumerate()
                .filter(|(_, r)| same_level(sp, &r.level, &e.level) && sys_ok(&r.system))
                .filter_map(|(i, r)| {
                    let (a, b) = (r.points.first().copied()?, r.points.last().copied()?);
                    let fwd = (dist(a, pf), dist(b, pl));
                    let rev = (dist(a, pl), dist(b, pf));
                    let best = if fwd.0 + fwd.1 <= rev.0 + rev.1 { fwd } else { rev };
                    (best.0 <= tol && best.1 <= tol).then_some((i, best.0 + best.1))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(i, _)| At::Run(i))
                .ok_or_else(gone)
        }
        El::Terminal => {
            let Some(p) = first else { return Err(gone()) };
            sp.mep
                .terminals
                .iter()
                .enumerate()
                .filter(|(_, t)| same_level(sp, &t.level, &e.level) && sys_ok(&t.system))
                .map(|(i, t)| (i, dist(t.at, p)))
                .filter(|(_, d)| *d <= tol)
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(i, _)| At::Terminal(i))
                .ok_or_else(gone)
        }
        El::Equipment => {
            let level_ok = |l: &str| e.level.is_empty() || same_level(sp, l, &e.level);
            let near = |q: &MepEquipment| first.map_or(0.0, |p| dist(centre(q), p));
            let by_tag = |q: &MepEquipment| !e.tag.is_empty() && q.tag.trim().eq_ignore_ascii_case(e.tag.trim());
            let tagged =
                sp.mep.equipment.iter().enumerate().filter(|(_, q)| by_tag(q) && level_ok(&q.level)).min_by(|a, b| near(a.1).total_cmp(&near(b.1)));
            if let Some((i, _)) = tagged {
                return Ok(At::Equipment(i));
            }
            if !e.tag.is_empty() {
                return Err(gone());
            }
            sp.mep
                .equipment
                .iter()
                .enumerate()
                .filter(|(_, q)| level_ok(&q.level))
                .map(|(i, q)| (i, near(q)))
                .filter(|(_, d)| *d <= tol)
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(i, _)| At::Equipment(i))
                .ok_or_else(gone)
        }
    }
}

/// A correction on an element's list: the first design value (the documents') stays, the value
/// installed, the reason and the rule are the latest edit's.
fn push_correction(list: &mut Vec<RunCorrection>, key: &str, design: String, corrected: String, reason: &str, rule: &str) {
    if design.trim() == corrected.trim() {
        return;
    }
    match list.iter_mut().find(|c| c.key == key) {
        Some(c) => {
            c.corrected = corrected;
            c.reason = reason.to_string();
            c.rule = rule.to_string();
        }
        None => list.push(RunCorrection { key: key.to_string(), design, corrected, reason: reason.to_string(), rule: rule.to_string() }),
    }
}

/// A size as text (`12x8`, a round `6`).
fn size_text(s: [f64; 2]) -> String {
    let n = |v: f64| {
        let t = format!("{v:.3}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if s[1] > 0.0 { format!("{}x{}", n(s[0]), n(s[1])) } else { n(s[0]) }
}

fn feet_text(v: f64) -> String {
    let t = format!("{v:.3}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The rule an edit's corrections name: the edit and who made it.
fn rule_of(e: &MepEdit) -> String {
    format!("mep edit {} ({})", e.id, if e.by.is_empty() { "user" } else { e.by.as_str() })
}

fn reason_of(e: &MepEdit) -> String {
    if e.reason.trim().is_empty() {
        format!("{} {} by {}", e.op, e.element, if e.by.is_empty() { "user" } else { e.by.as_str() })
    } else {
        e.reason.clone()
    }
}

/// What applying the spec's edits did: per edit, `applied` or `orphaned` and why.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Outcome {
    pub id: String,
    pub applied: bool,
    pub why: String,
}

/// Apply the spec's edits to its lists, in the order made (each finds its element as the edits
/// before it left the lists). The edits stay in the spec.
pub(crate) fn apply(sp: &mut BuildingSpec) -> Vec<Outcome> {
    if sp.mep.edits.is_empty() {
        return Vec::new();
    }
    let edits = std::mem::take(&mut sp.mep.edits);
    let tol = rules().match_ft.max(0.0) * upf(sp);
    let out = edits
        .iter()
        .take(MAX_EDITS)
        .map(|e| match apply_one(sp, e, tol) {
            Ok(()) => Outcome { id: e.id.clone(), applied: true, why: String::new() },
            Err(why) => Outcome { id: e.id.clone(), applied: false, why },
        })
        .collect();
    sp.mep.edits = edits;
    out
}

/// The build's report of the edits it applied.
pub(crate) fn report(out: &[Outcome]) -> Value {
    let orphaned: Vec<Value> = out.iter().filter(|o| !o.applied).take(50).map(|o| json!({"id": o.id, "why": o.why})).collect();
    json!({"edits": out.len(), "applied": out.iter().filter(|o| o.applied).count(), "orphaned": out.iter().filter(|o| !o.applied).count(), "orphaned_first": orphaned})
}

fn apply_one(sp: &mut BuildingSpec, e: &MepEdit, tol: f64) -> std::result::Result<(), String> {
    let (rule, reason) = (rule_of(e), reason_of(e));
    let upf = upf(sp);
    let pt = |p: [f64; 2]| format!("{:.2},{:.2} ft", p[0] / upf, p[1] / upf);
    if e.op == "add" {
        let mark = marker(&e.id);
        match (&e.run, &e.terminal, &e.equipment) {
            (Some(r), None, None) => {
                if !sp.mep.runs.iter().any(|x| added_by_run(x) == Some(e.id.as_str())) {
                    let mut r = r.clone();
                    if !r.objects.contains(&mark) {
                        r.objects.insert(0, mark);
                    }
                    if r.id.is_empty() {
                        r.id.clone_from(&e.id);
                    }
                    sp.mep.runs.push(r);
                }
            }
            (None, Some(t), None) => {
                if !sp.mep.terminals.iter().any(|x| added_by_terminal(x) == Some(e.id.as_str())) {
                    let mut t = t.clone();
                    if !t.objects.contains(&mark) {
                        t.objects.insert(0, mark);
                    }
                    sp.mep.terminals.push(t);
                }
            }
            (None, None, Some(q)) => {
                if !sp.mep.equipment.iter().any(|x| added_by_equipment(x) == Some(e.id.as_str())) {
                    let mut q = q.clone();
                    q.sources.insert("edit".into(), e.id.clone());
                    sp.mep.equipment.push(q);
                }
            }
            _ => return Err("an add edit carries exactly one run, terminal or piece of equipment".into()),
        }
        return Ok(());
    }
    let at = find(sp, e, tol)?;
    match e.op.as_str() {
        "delete" => {
            match at {
                At::Run(i) if i < sp.mep.runs.len() => {
                    sp.mep.runs.remove(i);
                }
                At::Terminal(i) if i < sp.mep.terminals.len() => {
                    sp.mep.terminals.remove(i);
                }
                At::Equipment(i) if i < sp.mep.equipment.len() => {
                    sp.mep.equipment.remove(i);
                }
                _ => return Err("the element is gone".into()),
            }
            Ok(())
        }
        "move" => {
            let d = e.delta.filter(|d| d.iter().all(|v| v.is_finite()));
            let b = e.bottom_ft.filter(|b| b.is_finite());
            match at {
                At::Run(i) => {
                    let r = sp.mep.runs.get_mut(i).ok_or("the run is gone")?;
                    if let Some(d) = d {
                        let was = r.points.first().copied().unwrap_or_default();
                        for p in &mut r.points {
                            *p = [p[0] + d[0], p[1] + d[1]];
                        }
                        let now = r.points.first().copied().unwrap_or_default();
                        push_correction(&mut r.corrections, "position", pt(was), pt(now), &reason, &rule);
                    }
                    if let Some(b) = b {
                        let was = r.bottoms_ft.first().copied().unwrap_or_default();
                        if r.bottoms_ft.is_empty() {
                            r.bottoms_ft.push(b);
                        }
                        r.bottoms_ft.fill(b);
                        r.height_source = "override".into();
                        push_correction(&mut r.corrections, "bottom_ft", feet_text(was), feet_text(b), &reason, &rule);
                    }
                    r.edited_by = if e.by.is_empty() { "user".into() } else { e.by.clone() };
                }
                At::Terminal(i) => {
                    let t = sp.mep.terminals.get_mut(i).ok_or("the terminal is gone")?;
                    if let Some(d) = d {
                        let was = t.at;
                        t.at = [t.at[0] + d[0], t.at[1] + d[1]];
                        push_correction(&mut t.corrections, "position", pt(was), pt(t.at), &reason, &rule);
                    }
                    if let Some(b) = b {
                        push_correction(&mut t.corrections, "bottom_ft", feet_text(t.bottom_ft), feet_text(b), &reason, &rule);
                        t.bottom_ft = b;
                    }
                }
                At::Equipment(i) => {
                    let q = sp.mep.equipment.get_mut(i).ok_or("the equipment is gone")?;
                    if let Some(d) = d {
                        let was = centre(q);
                        q.min = [q.min[0] + d[0], q.min[1] + d[1]];
                        q.max = [q.max[0] + d[0], q.max[1] + d[1]];
                        let now = centre(q);
                        push_correction(&mut q.corrections, "position", pt(was), pt(now), &reason, &rule);
                    }
                    if let Some(b) = b {
                        push_correction(&mut q.corrections, "bottom_ft", feet_text(q.bottom_ft), feet_text(b), &reason, &rule);
                        q.bottom_ft = b;
                    }
                }
            }
            Ok(())
        }
        "resize" => {
            let size = e.size_in.filter(|s| s[0].is_finite() && s[0] > 0.0 && s[1].is_finite() && s[1] >= 0.0);
            match at {
                At::Run(i) => {
                    let r = sp.mep.runs.get_mut(i).ok_or("the run is gone")?;
                    let s = size.ok_or("a run is resized by its size_in")?;
                    push_correction(&mut r.corrections, "size", size_text(r.size_in), size_text(s), &reason, &rule);
                    r.size_in = s;
                    r.size_source = "override".into();
                    r.edited_by = if e.by.is_empty() { "user".into() } else { e.by.clone() };
                }
                At::Terminal(i) => {
                    let t = sp.mep.terminals.get_mut(i).ok_or("the terminal is gone")?;
                    let s = size.ok_or("a terminal is resized by its size_in")?;
                    push_correction(&mut t.corrections, "size", size_text(t.size_in), size_text(s), &reason, &rule);
                    t.size_in = s;
                }
                At::Equipment(i) => {
                    let q = sp.mep.equipment.get_mut(i).ok_or("the equipment is gone")?;
                    if let Some(f) = e.footprint.filter(|f| f.iter().all(|v| v.is_finite() && *v > 0.0)) {
                        let c = centre(q);
                        let was = [(q.max[0] - q.min[0]).abs(), (q.max[1] - q.min[1]).abs()];
                        q.min = [c[0] - f[0] / 2.0, c[1] - f[1] / 2.0];
                        q.max = [c[0] + f[0] / 2.0, c[1] + f[1] / 2.0];
                        let ft = |s: [f64; 2]| format!("{} x {} ft", feet_text(s[0] / upf), feet_text(s[1] / upf));
                        push_correction(&mut q.corrections, "footprint", ft(was), ft(f), &reason, &rule);
                    }
                    if let Some(h) = e.height_ft.filter(|h| h.is_finite() && *h > 0.0) {
                        push_correction(&mut q.corrections, "height_ft", feet_text(q.height_ft), feet_text(h), &reason, &rule);
                        q.height_ft = h;
                    }
                }
            }
            Ok(())
        }
        "set" => {
            match at {
                At::Run(i) => {
                    let r = sp.mep.runs.get_mut(i).ok_or("the run is gone")?;
                    if let Some(sy) = &e.new_system {
                        push_correction(&mut r.corrections, "system", r.system.clone(), sy.clone(), &reason, &rule);
                        r.system.clone_from(sy);
                    }
                    for (k, v) in e.properties.iter().take(MAX_PROPS).filter(|(_, v)| !v.is_empty()) {
                        push_correction(&mut r.corrections, k, "(none)".into(), v.clone(), &reason, &rule);
                    }
                }
                At::Terminal(i) => {
                    let t = sp.mep.terminals.get_mut(i).ok_or("the terminal is gone")?;
                    if let Some(sy) = &e.new_system {
                        push_correction(&mut t.corrections, "system", t.system.clone(), sy.clone(), &reason, &rule);
                        t.system.clone_from(sy);
                    }
                    if let Some(k) = &e.kind {
                        push_correction(&mut t.corrections, "kind", t.terminal_kind.clone(), k.clone(), &reason, &rule);
                        t.terminal_kind.clone_from(k);
                    }
                    for (k, v) in e.properties.iter().take(MAX_PROPS).filter(|(_, v)| !v.is_empty()) {
                        push_correction(&mut t.corrections, k, "(none)".into(), v.clone(), &reason, &rule);
                    }
                }
                At::Equipment(i) => {
                    let q = sp.mep.equipment.get_mut(i).ok_or("the equipment is gone")?;
                    if let Some(m) = &e.type_mark {
                        let was = if q.type_mark.is_empty() { q.tag.clone() } else { q.type_mark.clone() };
                        push_correction(&mut q.corrections, "type_mark", was, m.clone(), &reason, &rule);
                        q.type_mark.clone_from(m);
                    }
                    if let Some(k) = &e.kind {
                        push_correction(&mut q.corrections, "kind", q.kind.clone(), k.clone(), &reason, &rule);
                        q.kind.clone_from(k);
                    }
                    for (k, v) in e.properties.iter().take(MAX_PROPS) {
                        let was = q.properties.get(k).cloned().unwrap_or_else(|| "(none)".into());
                        if v.is_empty() {
                            if q.properties.remove(k).is_some() {
                                push_correction(&mut q.corrections, k, was, "(removed)".into(), &reason, &rule);
                            }
                        } else {
                            push_correction(&mut q.corrections, k, was, v.clone(), &reason, &rule);
                            q.properties.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            Ok(())
        }
        other => Err(format!("unknown edit {other:?}")),
    }
}

// ─── Joining what the build lays (for bim.chain) ─────────────────────────────

/// One run the MEP build laid, for joining: its run, the kind of its system, its level and that
/// level's floor (feet), and the elements built of it (its segment, its own fittings).
pub(crate) struct NetRun<'a> {
    pub run: &'a MepRun,
    pub kind: MepKind,
    pub level: String,
    pub floor_ft: f64,
    pub seg: Option<Handle>,
    pub fittings: Vec<Handle>,
}

/// What the MEP build laid: its runs (as it numbered them), the fittings where runs meet (each
/// with the runs it joins), the runs meeting end to end with no fitting, its terminals and its
/// equipment.
#[derive(Default)]
pub(crate) struct Net<'a> {
    pub runs: Vec<NetRun<'a>>,
    pub joints: Vec<(Handle, Vec<usize>)>,
    pub meets: Vec<(usize, usize)>,
    pub terminals: Vec<(Handle, &'a MepTerminal)>,
    pub equipment: Vec<(Handle, &'a MepEquipment)>,
}

/// The distance from `p` to a run's plan path.
fn path_dist(pts: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let p = Vec2::new(p[0], p[1]);
    match pts {
        [] => f64::INFINITY,
        [a] => p.dist(Vec2::new(a[0], a[1])),
        _ => pts
            .windows(2)
            .map(|w| {
                let (a, b) = (Vec2::new(w[0][0], w[0][1]), Vec2::new(w[1][0], w[1][1]));
                let ab = b - a;
                let l2 = ab.dot(ab);
                let t = if l2 > 1e-18 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
                p.dist(a + ab * t)
            })
            .fold(f64::INFINITY, f64::min),
    }
}

/// Connect what the MEP build laid ([`Net`]), both ways, in the elements' `connects`: a run's
/// segment with its fittings and the fittings where it meets others; runs meeting end to end; a
/// riser with the runs of its system it passes through; each air terminal and sprinkler head
/// with the nearest run of its system on its level in the rules' reach; each unit (not
/// electrical gear) with the runs of its systems ending at it. The connections written.
pub(crate) fn join(d: &mut Drawing, net: &Net, upf: f64) -> usize {
    let r = rules();
    let inch = upf / 12.0;
    let mut adj: HashMap<Handle, BTreeSet<Handle>> = HashMap::new();
    let mut link = |a: Handle, b: Handle| {
        if a != b {
            adj.entry(a).or_default().insert(b);
            adj.entry(b).or_default().insert(a);
        }
    };
    let ducts_pipes = |k: MepKind| matches!(k, MepKind::Duct | MepKind::Pipe);
    let seg = |i: usize| net.runs.get(i).filter(|n| ducts_pipes(n.kind)).and_then(|n| n.seg);
    for n in net.runs.iter().filter(|n| ducts_pipes(n.kind)) {
        if let Some(s) = n.seg {
            for f in &n.fittings {
                link(s, *f);
            }
        }
    }
    for (h, ends) in &net.joints {
        for i in ends {
            if let Some(s) = seg(*i) {
                link(*h, s);
            }
        }
    }
    for (a, b) in &net.meets {
        if let (Some(x), Some(y)) = (seg(*a), seg(*b)) {
            link(x, y);
        }
    }
    // By system: the runs of each (risers apart), for the lookups below.
    let mut by_system: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, n) in net.runs.iter().enumerate().filter(|(_, n)| ducts_pipes(n.kind) && n.seg.is_some()) {
        by_system.entry(n.run.system.to_ascii_uppercase()).or_default().push(i);
    }
    let mut tests = 0usize;
    let half = |s: [f64; 2]| s[0].max(0.0) / 2.0 * inch;
    // Risers: through the runs of their system whose height they span, on any level.
    for (i, n) in net.runs.iter().enumerate() {
        let (Some(top), Some(s), Some(p)) = (n.run.top_ft, seg(i), n.run.points.first().copied()) else { continue };
        let z0 = n.floor_ft + n.run.bottoms_ft.first().copied().unwrap_or(0.0);
        let z1 = n.floor_ft + top;
        for j in by_system.get(&n.run.system.to_ascii_uppercase()).into_iter().flatten() {
            let Some(m) = net.runs.get(*j).filter(|m| *j != i && m.run.top_ft.is_none()) else { continue };
            tests += m.run.points.len();
            if tests > MAX_JOIN_TESTS {
                break;
            }
            let z = m.floor_ft + m.run.bottoms_ft.first().copied().unwrap_or(0.0);
            let slack = (m.run.size_in[1].max(m.run.size_in[0]) / 12.0).max(1.0);
            if z < z0 - slack || z > z1 + slack {
                continue;
            }
            if path_dist(&m.run.points, p) <= half(n.run.size_in) + half(m.run.size_in) + inch
                && let Some(t) = m.seg
            {
                link(s, t);
            }
        }
    }
    // Terminals and heads: the nearest run of their system on their level in reach.
    let air_reach = super::provenance::rule(&r.terminal_rule).connect_reach_ft.max(0.0) * upf;
    let head_reach = r.sprinkler_reach_ft.max(0.0) * upf;
    for (h, t) in &net.terminals {
        let reach = if t.terminal_kind.to_ascii_lowercase().contains("sprinkler") { head_reach } else { air_reach };
        let mut best: Option<(f64, Handle)> = None;
        for j in by_system.get(&t.system.to_ascii_uppercase()).into_iter().flatten() {
            let Some(m) = net.runs.get(*j).filter(|m| m.level.eq_ignore_ascii_case(&t.level) || m.run.level.eq_ignore_ascii_case(&t.level)) else {
                continue;
            };
            tests += m.run.points.len();
            if tests > MAX_JOIN_TESTS {
                break;
            }
            let dd = path_dist(&m.run.points, t.at);
            if dd <= reach && best.is_none_or(|(b, _)| dd < b) {
                best = m.seg.map(|s| (dd, s));
            }
        }
        if let Some((_, s)) = best {
            link(*h, s);
        }
    }
    // Equipment: the runs of its systems (any duct or pipe when it names none) ending at it.
    let reach = r.equipment_reach_ft.max(0.0) * upf;
    let electrical = |k: &str| {
        let k = k.trim().to_ascii_lowercase();
        r.unconnected_kinds.iter().any(|u| u.eq_ignore_ascii_case(&k))
    };
    for (h, q) in &net.equipment {
        if electrical(&q.kind) {
            continue;
        }
        let (lo, hi) =
            ([q.min[0].min(q.max[0]) - reach, q.min[1].min(q.max[1]) - reach], [q.min[0].max(q.max[0]) + reach, q.min[1].max(q.max[1]) + reach]);
        let inside = |p: &[f64; 2]| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1];
        let systems: Vec<String> = q.systems.iter().map(|s| s.to_ascii_uppercase()).collect();
        for (sys, list) in &by_system {
            if !systems.is_empty() && !systems.contains(sys) {
                continue;
            }
            for j in list {
                let Some(m) = net.runs.get(*j).filter(|m| m.level.eq_ignore_ascii_case(&q.level) || m.run.level.eq_ignore_ascii_case(&q.level))
                else {
                    continue;
                };
                tests += 2;
                if tests > MAX_JOIN_TESTS {
                    break;
                }
                let ends = [m.run.points.first(), m.run.points.last()];
                if ends.into_iter().flatten().any(inside)
                    && let Some(s) = m.seg
                {
                    link(*h, s);
                }
            }
        }
    }
    let mut written = 0usize;
    for (h, set) in adj {
        let _ = d.modify_entity(h, |e: &mut Entity| {
            let mut ps = props::read(&e.common.xdata);
            let mut all = super::mepatoms::connects_of(e);
            all.extend(set.iter().copied());
            all.sort_unstable();
            all.dedup();
            ps.set("connects", Prop::new(all.iter().take(MAX_CONNECTS).map(|x| x.hex()).collect::<Vec<_>>().join(","), Source::computed()));
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
            written += set.len();
        });
    }
    written / 2
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// The stored spec, and the spec as every build sees it (its edits applied) with what applying
/// them did.
fn views(s: &Session, cmd: &str) -> Result<(BuildingSpec, BuildingSpec, Vec<Outcome>)> {
    let (sp, _) = super::bspec::current(s, cmd)?;
    let mut v = sp.clone();
    let out = apply(&mut v);
    Ok((sp, v, out))
}

/// The next edit id (`ME-<n>`).
fn next_id(sp: &BuildingSpec) -> String {
    let n = sp.mep.edits.iter().filter_map(|e| e.id.strip_prefix("ME-").and_then(|x| x.parse::<u64>().ok())).max().unwrap_or(0);
    format!("ME-{}", n.saturating_add(1))
}

fn text(cmd: &str, p: &Value, key: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) if t.chars().count() <= MAX_TEXT => Ok(Some(t.trim().to_string())),
        Some(Value::String(_)) => Err(bad(cmd, format!("`{key}` is at most {MAX_TEXT} characters"))),
        Some(_) => Err(bad(cmd, format!("`{key}` must be text"))),
    }
}

/// Who: `user` (default) or `agent`.
fn by_param(cmd: &str, p: &Value) -> Result<String> {
    match text(cmd, p, "by")?.as_deref() {
        None | Some("") | Some("user") => Ok("user".into()),
        Some("agent") => Ok("agent".into()),
        Some(o) => Err(bad(cmd, format!("`by` is user or agent, not {o:?}"))),
    }
}

/// `[x, y]` of finite numbers within the drawing's reach.
fn pair(cmd: &str, p: &Value, key: &str, max: f64) -> Result<Option<[f64; 2]>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, format!("`{key}` is [x, y]")))?;
            let n = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite() && x.abs() <= max);
            match (n(0), n(1)) {
                (Some(x), Some(y)) => Ok(Some([x, y])),
                _ => Err(bad(cmd, format!("`{key}` is two numbers, each at most {max} across"))),
            }
        }
    }
}

/// A duct, pipe or terminal size: `[w, h]` or `[d]` (round), inches.
fn size_in(cmd: &str, p: &Value) -> Result<Option<[f64; 2]>> {
    let Some(v) = p.get("size_in").filter(|v| !v.is_null()) else { return Ok(None) };
    let inches = |x: &Value| x.as_f64().filter(|x| x.is_finite() && (0.0..=MAX_SIZE_IN).contains(x));
    let a = v.as_array().filter(|a| (1..=2).contains(&a.len()) && a.iter().all(|x| inches(x).is_some()));
    match a.map(|a| (a.first().and_then(inches).unwrap_or(0.0), a.get(1).and_then(inches).unwrap_or(0.0))) {
        Some((w, h)) if w > 0.0 => Ok(Some([w, h])),
        _ => Err(bad(cmd, format!("`size_in` is [width, height] or [diameter] in inches: the first above 0, each at most {MAX_SIZE_IN}"))),
    }
}

/// A height over the level's floor, feet: from under the slab to the building's top (or 100').
fn bottom_param(cmd: &str, p: &Value, key: &str, sp: &BuildingSpec, level: &str) -> Result<Option<f64>> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let b = v.as_f64().filter(|b| b.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` must be a number of feet")))?;
    let floor = sp.level(level).map(|l| l.elevation_ft).filter(|e| e.is_finite());
    let top = sp.levels.iter().map(|l| l.elevation_ft + l.height_ft).filter(|t| t.is_finite()).fold(f64::NEG_INFINITY, f64::max);
    let storey = floor.filter(|_| top.is_finite()).map_or(MAX_STOREY_FT, |f| (top - f).max(MAX_STOREY_FT));
    if !(-MAX_UNDER_FLOOR_FT..=storey).contains(&b) {
        return Err(bad(cmd, format!("`{key}` is feet above {level}'s floor: from -{MAX_UNDER_FLOOR_FT} (under the slab) to {storey}, not {b}")));
    }
    Ok(Some(b))
}

/// A spec system by name (any case), as the spec names it.
fn system_named(cmd: &str, sp: &BuildingSpec, name: &str, runs_only: bool) -> Result<String> {
    let sy = sp.mep.systems.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
        let known: Vec<&str> = sp.mep.systems.iter().take(30).map(|s| s.name.as_str()).collect();
        bad(
            cmd,
            format!(
                "no system {name:?} in the spec (it has {}); bspec.set adds one",
                if known.is_empty() { "none".to_string() } else { known.join(", ") }
            ),
        )
    })?;
    if runs_only && !matches!(sy.kind, MepKind::Duct | MepKind::Pipe) {
        return Err(bad(cmd, format!("{} is not a duct or pipe system (conduit and cable go with their circuits)", sy.name)));
    }
    Ok(sy.name.clone())
}

/// The key an edit records of an element of `v` (the spec as the builds see it).
fn key_of(v: &BuildingSpec, at: At) -> Option<MepEdit> {
    let mut e = MepEdit::default();
    match at {
        At::Run(i) => {
            let r = v.mep.runs.get(i)?;
            e.element = "run".into();
            e.level.clone_from(&r.level);
            e.system.clone_from(&r.system);
            match added_by_run(r) {
                Some(id) => e.added_by = id.to_string(),
                None => e.points = r.points.iter().take(MAX_POINTS).copied().collect(),
            }
        }
        At::Terminal(i) => {
            let t = v.mep.terminals.get(i)?;
            e.element = "terminal".into();
            e.level.clone_from(&t.level);
            e.system.clone_from(&t.system);
            match added_by_terminal(t) {
                Some(id) => e.added_by = id.to_string(),
                None => e.points = vec![t.at],
            }
        }
        At::Equipment(i) => {
            let q = v.mep.equipment.get(i)?;
            e.element = "equipment".into();
            e.level.clone_from(&q.level);
            e.tag.clone_from(&q.tag);
            match added_by_equipment(q) {
                Some(id) => e.added_by = id.to_string(),
                None => e.points = vec![centre(q)],
            }
        }
    }
    Some(e)
}

/// The element `p` names, in `v`: by `handle`, `run`, `tag`, `edit`, or `element` + `level` + `at`.
fn target(cmd: &str, s: &Session, v: &BuildingSpec, p: &Value) -> Result<At> {
    let tol = rules().match_ft.max(0.0) * upf(v);
    if let Some(hv) = p.get("handle").filter(|h| !h.is_null()) {
        let h = hv.as_str().and_then(Handle::parse_hex).ok_or_else(|| bad(cmd, "`handle` is a hex handle (\"1A2\")"))?;
        let d = s.doc()?;
        let e = d.model.get(h).ok_or_else(|| bad(cmd, format!("there is no object {} in the model", h.hex())))?;
        return from_element(cmd, v, e, tol);
    }
    if let Some(id) = text(cmd, p, "run")?.filter(|t| !t.is_empty()) {
        return v
            .mep
            .runs
            .iter()
            .position(|r| r.id.eq_ignore_ascii_case(&id))
            .map(At::Run)
            .ok_or_else(|| bad(cmd, format!("no run with id {id} (mep.list lists them)")));
    }
    let level = text(cmd, p, "level")?.unwrap_or_default();
    if let Some(tag) = text(cmd, p, "tag")?.filter(|t| !t.is_empty()) {
        let probe = MepEdit { element: "equipment".into(), tag: tag.clone(), level: level.clone(), ..MepEdit::default() };
        return find(v, &probe, tol)
            .map_err(|_| bad(cmd, format!("no equipment tagged {tag}{}", if level.is_empty() { String::new() } else { format!(" on {level}") })));
    }
    if let Some(id) = text(cmd, p, "edit")?.filter(|t| !t.is_empty()) {
        let hit = v
            .mep
            .runs
            .iter()
            .position(|r| added_by_run(r) == Some(id.as_str()))
            .map(At::Run)
            .or_else(|| v.mep.terminals.iter().position(|t| added_by_terminal(t) == Some(id.as_str())).map(At::Terminal))
            .or_else(|| v.mep.equipment.iter().position(|q| added_by_equipment(q) == Some(id.as_str())).map(At::Equipment));
        return hit.ok_or_else(|| bad(cmd, format!("no element added by edit {id} (mep.edits lists them)")));
    }
    if let Some(at) = pair(cmd, p, "at", 1e12)? {
        let el = text(cmd, p, "element")?
            .as_deref()
            .and_then(El::parse)
            .ok_or_else(|| bad(cmd, "with `at`, `element` (run, terminal or equipment) is required"))?;
        if level.is_empty() {
            return Err(bad(cmd, "with `at`, `level` is required"));
        }
        let system = text(cmd, p, "system")?.unwrap_or_default();
        return match el {
            El::Run => v
                .mep
                .runs
                .iter()
                .enumerate()
                .filter(|(_, r)| same_level(v, &r.level, &level) && (system.is_empty() || r.system.eq_ignore_ascii_case(&system)))
                .map(|(i, r)| (i, path_dist(&r.points, at)))
                .filter(|(_, d)| *d <= tol)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| At::Run(i))
                .ok_or_else(|| bad(cmd, format!("no run on {level} within {} ft of {:.2},{:.2}", rules().match_ft, at[0], at[1]))),
            _ => {
                let probe = MepEdit { element: el.as_str().into(), level: level.clone(), system, points: vec![at], ..MepEdit::default() };
                find(v, &probe, tol)
                    .map_err(|_| bad(cmd, format!("no {} on {level} within {} ft of {:.2},{:.2}", el.as_str(), rules().match_ft, at[0], at[1])))
            }
        };
    }
    Err(bad(cmd, "say which element: handle, run, tag, edit, or element + level + at"))
}

/// A built element back to the spec element it was built from.
fn from_element(cmd: &str, v: &BuildingSpec, e: &Entity, tol: f64) -> Result<At> {
    let part = super::bimlook::made_tag(e, "part").unwrap_or_default().to_ascii_lowercase();
    let level = super::bspec::made_level(e).unwrap_or_default();
    let system = super::bimlook::made_tag(e, "system").unwrap_or_default().to_string();
    let id = super::bimlook::made_tag(e, "mepid").unwrap_or_default().to_string();
    let not_mep = || bad(cmd, format!("{} is not a duct, pipe, terminal or piece of equipment the build made", e.handle.hex()));
    if part.starts_with("equipment") {
        let tag = super::bimlook::made_tag(e, "tag").unwrap_or_default().to_string();
        let probe = MepEdit { element: "equipment".into(), tag: tag.clone(), level, ..MepEdit::default() };
        return find(v, &probe, tol)
            .map_err(|_| bad(cmd, format!("equipment {tag} is in the model but not in the spec any more: model.build again")));
    }
    if part.starts_with("terminal") {
        let at = id
            .rsplit_once("@")
            .and_then(|(_, xy)| xy.split_once(','))
            .and_then(|(x, y)| Some([x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?]));
        let at = at.ok_or_else(not_mep)?;
        let probe = MepEdit { element: "terminal".into(), level, system, points: vec![at], ..MepEdit::default() };
        return find(v, &probe, tol).map_err(|_| bad(cmd, "that terminal is in the model but not in the spec any more: model.build again"));
    }
    if part == "segment" || part.starts_with("fitting:elbow") || part.starts_with("fitting:drop") {
        let base = id.rsplit_once('/').map(|(b, _)| b).ok_or_else(not_mep)?;
        return v
            .mep
            .runs
            .iter()
            .position(|r| super::mep::run_base(r) == base && r.system == system && same_level(v, &r.level, &level))
            .map(At::Run)
            .ok_or_else(|| bad(cmd, "that run is in the model but not in the spec any more: model.build again"));
    }
    if part.starts_with("fitting:") {
        return Err(bad(cmd, format!("{} is a fitting where several runs meet: pick one of the runs", e.handle.hex())));
    }
    Err(not_mep())
}

/// An element of `v` as JSON.
fn element_json(v: &BuildingSpec, at: At) -> Value {
    let j = match at {
        At::Run(i) => v.mep.runs.get(i).and_then(|x| serde_json::to_value(x).ok()),
        At::Terminal(i) => v.mep.terminals.get(i).and_then(|x| serde_json::to_value(x).ok()),
        At::Equipment(i) => v.mep.equipment.get(i).and_then(|x| serde_json::to_value(x).ok()),
    };
    j.unwrap_or(Value::Null)
}

/// Where the element an edit made or changed is in `v` (none: deleted).
fn locate(v: &BuildingSpec, e: &MepEdit) -> Option<At> {
    let id = if e.op == "add" { e.id.as_str() } else { e.added_by.as_str() };
    if !id.is_empty() {
        let probe = MepEdit { element: e.element.clone(), added_by: id.to_string(), ..MepEdit::default() };
        return find(v, &probe, 0.0).ok();
    }
    if e.op == "delete" {
        return None;
    }
    // Where the edit put it.
    let mut probe = e.clone();
    if let Some(d) = e.delta {
        for p in &mut probe.points {
            *p = [p[0] + d[0], p[1] + d[1]];
        }
    }
    if let Some(sy) = &e.new_system {
        probe.system.clone_from(sy);
    }
    find(v, &probe, rules().match_ft.max(0.0) * upf(v)).ok()
}

/// Store `e` in the drawing's spec (after checking it applies), rebuild when asked, and answer.
fn commit(cmd: &str, s: &mut Session, p: &Value, mut sp: BuildingSpec, mut e: MepEdit) -> Result<Value> {
    if sp.mep.edits.len() >= MAX_EDITS {
        return Err(bad(cmd, format!("the spec holds {MAX_EDITS} edits already: mep.edits revert takes some back")));
    }
    bool_param(cmd, p, "rebuild")?;
    e.id = next_id(&sp);
    e.by = by_param(cmd, p)?;
    e.reason = text(cmd, p, "reason")?.unwrap_or_default();
    sp.mep.edits.push(e.clone());
    let mut v = sp.clone();
    let out = apply(&mut v);
    if let Some(o) = out.iter().find(|o| o.id == e.id && !o.applied) {
        return Err(bad(cmd, o.why.clone()));
    }
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    let element = locate(&v, &e).map_or(Value::Null, |at| element_json(&v, at));
    let mut r = json!({"edit": serde_json::to_value(&e).unwrap_or(Value::Null), "status": "applied", "element": element});
    rebuild(cmd, s, p, &mut r)?;
    s.echo(format!(
        "{} {}: {} {} — kept in the building spec's edits; every build applies it.",
        e.id,
        e.op,
        e.element,
        if e.tag.is_empty() { e.system.as_str() } else { e.tag.as_str() }
    ));
    Ok(r)
}

/// `model.build` again when asked (by default when the model is built).
fn rebuild(cmd: &str, s: &mut Session, p: &Value, r: &mut Value) -> Result<()> {
    let built = super::buildrec::read(s.doc()?).is_some() || s.state().is_ok_and(|st| st.wallgen.is_some());
    if bool_param(cmd, p, "rebuild")?.unwrap_or(built) {
        match s.execute("model.build", &json!({"echo": false})) {
            Ok(b) => {
                r["rebuilt"] = json!(true);
                r["mepEdits"] = b.get("mepEdits").cloned().unwrap_or(Value::Null);
            }
            Err(err) => {
                r["rebuilt"] = json!(false);
                r["rebuild_error"] = json!(err.to_string());
            }
        }
    } else {
        r["rebuilt"] = json!(false);
    }
    Ok(())
}

/// The provenance of an added element: `made` (default inferred) and its basis (default the reason).
fn made_param(cmd: &str, p: &Value, reason: &str, by: &str) -> Result<(Made, String)> {
    let made = match text(cmd, p, "made")? {
        None => Made::Inferred,
        Some(t) => Made::parse(&t).ok_or_else(|| bad(cmd, "`made` is drawn, derived or inferred"))?,
    };
    let basis = text(cmd, p, "basis")?
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| if reason.is_empty() { format!("added by {by} (mep.add)") } else { format!("added by {by}: {reason}") });
    Ok((made, if made.is_drawn() { String::new() } else { basis }))
}

/// A run end or a unit's side to connect a new run's end to: the point, and the run's system,
/// size and bottom there (none for equipment).
struct Joint {
    at: [f64; 2],
    level: String,
    run: Option<(String, [f64; 2], f64)>,
}

fn joint(cmd: &str, s: &Session, v: &BuildingSpec, p: &Value, key: &str, toward: Option<[f64; 2]>) -> Result<Option<Joint>> {
    let Some(t) = p.get(key).filter(|t| !t.is_null()) else { return Ok(None) };
    if !t.is_object() {
        return Err(bad(cmd, format!("`{key}` names an element: {{run}}, {{handle}}, {{tag}}, {{edit}} or {{element, level, at}}")));
    }
    let at = target(cmd, s, v, t)?;
    match at {
        At::Run(i) => {
            let r = v.mep.runs.get(i).ok_or_else(|| bad(cmd, "the run is gone"))?;
            let (Some(a), Some(b)) = (r.points.first().copied(), r.points.last().copied()) else { return Err(bad(cmd, "that run has no points")) };
            let end = text(cmd, t, "end")?;
            let first = match end.as_deref() {
                Some("start") => true,
                Some("end") => false,
                None | Some("") => toward.is_none_or(|q| dist(a, q) <= dist(b, q)),
                Some(o) => return Err(bad(cmd, format!("`end` is start or end, not {o:?}"))),
            };
            let bottom = if first { r.bottoms_ft.first() } else { r.bottoms_ft.last() }.copied().unwrap_or(0.0);
            Ok(Some(Joint { at: if first { a } else { b }, level: r.level.clone(), run: Some((r.system.clone(), r.size_in, bottom)) }))
        }
        At::Equipment(i) => {
            let q = v.mep.equipment.get(i).ok_or_else(|| bad(cmd, "the equipment is gone"))?;
            // Onto its side nearest the run's next point (its centre without one).
            let c = centre(q);
            let q2 = toward.unwrap_or(c);
            let (lo, hi) = ([q.min[0].min(q.max[0]), q.min[1].min(q.max[1])], [q.min[0].max(q.max[0]), q.min[1].max(q.max[1])]);
            let inside = [q2[0].clamp(lo[0], hi[0]), q2[1].clamp(lo[1], hi[1])];
            let to_side = [
                (inside[0] - lo[0], [lo[0], inside[1]]),
                (hi[0] - inside[0], [hi[0], inside[1]]),
                (inside[1] - lo[1], [inside[0], lo[1]]),
                (hi[1] - inside[1], [inside[0], hi[1]]),
            ];
            let side = if q2 == inside { to_side.iter().min_by(|a, b| a.0.total_cmp(&b.0)).map_or(inside, |x| x.1) } else { inside };
            Ok(Some(Joint { at: side, level: q.level.clone(), run: None }))
        }
        At::Terminal(_) => Err(bad(cmd, format!("`{key}`: a run connects to a run's end or to equipment, not to a terminal"))),
    }
}

fn run_add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mep.add";
    let (sp, v, _) = views(s, CMD)?;
    let el = text(CMD, p, "element")?
        .ok_or_else(|| bad(CMD, "`element` is required: run, terminal or equipment (or duct, pipe, diffuser, register, grille, sprinkler)"))?;
    let word = el.to_ascii_lowercase();
    let kind_el = El::parse(&word).ok_or_else(|| bad(CMD, format!("`element` is run, terminal or equipment, not {el:?}")))?;
    let by = by_param(CMD, p)?;
    let reason = text(CMD, p, "reason")?.unwrap_or_default();
    let (made, basis) = made_param(CMD, p, &reason, &by)?;
    let source_sheet = text(CMD, p, "source_sheet")?.unwrap_or_default();
    let upf = upf(&sp);
    let level_of = |l: &str| -> Result<String> { sp.level(l).map(|x| x.name.clone()).ok_or_else(|| bad(CMD, format!("no level {l:?} in the spec"))) };
    let mut e = MepEdit { op: "add".into(), element: kind_el.as_str().into(), ..MepEdit::default() };
    match kind_el {
        El::Run => {
            let raw = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`points` ([[x, y], ...], drawing units) is required"))?;
            if raw.is_empty() || raw.len() > MAX_POINTS {
                return Err(bad(CMD, format!("`points` holds 1 to {MAX_POINTS} points")));
            }
            let mut pts: Vec<[f64; 2]> = Vec::with_capacity(raw.len());
            for q in raw {
                let a = point_value(q)
                    .filter(|a| a.x.abs() <= 1e12 && a.y.abs() <= 1e12)
                    .ok_or_else(|| bad(CMD, "each point is [x, y] of finite numbers"))?;
                pts.push([a.x, a.y]);
            }
            let top = match p.get("top_ft").filter(|t| !t.is_null()) {
                Some(t) => Some(t.as_f64().filter(|t| t.is_finite()).ok_or_else(|| bad(CMD, "`top_ft` must be feet"))?),
                None => None,
            };
            let from = joint(CMD, s, &v, p, "from", pts.get(1).copied())?;
            let to = joint(CMD, s, &v, p, "to", pts.len().checked_sub(2).and_then(|i| pts.get(i)).copied())?;
            if let (Some(j), Some(f)) = (&from, pts.first_mut()) {
                *f = j.at;
            }
            if let (Some(j), Some(l)) = (&to, pts.last_mut()) {
                *l = j.at;
            }
            pts.dedup_by(|a, b| dist(*a, *b) <= 1e-9);
            if pts.len() < 2 && top.is_none() {
                return Err(bad(CMD, "a run needs two distinct points (one point with top_ft is a riser)"));
            }
            let level = match text(CMD, p, "level")?.filter(|l| !l.is_empty()) {
                Some(l) => level_of(&l)?,
                None => level_of(&from.as_ref().or(to.as_ref()).map(|j| j.level.clone()).ok_or_else(|| bad(CMD, "`level` is required"))?)?,
            };
            let inherited = from.as_ref().and_then(|j| j.run.clone()).or_else(|| to.as_ref().and_then(|j| j.run.clone()));
            let system = match text(CMD, p, "system")?.filter(|x| !x.is_empty()) {
                Some(x) => system_named(CMD, &sp, &x, true)?,
                None => system_named(
                    CMD,
                    &sp,
                    &inherited.as_ref().map(|x| x.0.clone()).ok_or_else(|| bad(CMD, "`system` is required (or `from` a run)"))?,
                    true,
                )?,
            };
            let sy = sp.mep.systems.iter().find(|x| x.name == system);
            let size = match size_in(CMD, p)? {
                Some(x) => x,
                None => inherited.as_ref().map(|x| x.1).or_else(|| sy.map(|y| y.size_in)).unwrap_or([0.0, 0.0]),
            };
            if !(size[0].is_finite() && size[0] > 0.0) {
                return Err(bad(CMD, "`size_in` is required: the system gives no size"));
            }
            let bottom = match bottom_param(CMD, p, "bottom_ft", &sp, &level)? {
                Some(b) => b,
                None => inherited.as_ref().map(|x| x.2).or_else(|| sy.map(|y| y.bottom_ft)).unwrap_or(0.0),
            };
            if let Some(t) = top
                && t <= bottom
            {
                return Err(bad(CMD, "`top_ft` must be above the bottom"));
            }
            let segs = pts.len().saturating_sub(1).max(1);
            let run = MepRun {
                system: system.clone(),
                level: level.clone(),
                points: pts.clone(),
                size_in: size,
                bottoms_ft: vec![bottom; segs],
                objects: Vec::new(),
                id: String::new(),
                encased: false,
                size_source: "override".into(),
                height_source: "override".into(),
                reviewed: true,
                edited_by: by.clone(),
                top_ft: top,
                source_sheet,
                slope_in_per_ft: 0.0,
                made,
                basis,
                corrections: Vec::new(),
            };
            e.level = level;
            e.system = system;
            e.points = pts;
            e.run = Some(run);
        }
        El::Terminal => {
            let at = pair(CMD, p, "at", 1e12)?.ok_or_else(|| bad(CMD, "`at` ([x, y], drawing units) is required"))?;
            let from = joint(CMD, s, &v, p, "from", Some(at))?;
            let level = match text(CMD, p, "level")?.filter(|l| !l.is_empty()) {
                Some(l) => level_of(&l)?,
                None => level_of(&from.as_ref().map(|j| j.level.clone()).ok_or_else(|| bad(CMD, "`level` is required"))?)?,
            };
            let system = match text(CMD, p, "system")?.filter(|x| !x.is_empty()) {
                Some(x) => system_named(CMD, &sp, &x, false)?,
                None => from
                    .as_ref()
                    .and_then(|j| j.run.as_ref().map(|r| r.0.clone()))
                    .ok_or_else(|| bad(CMD, "`system` is required (or `from` a run)"))?,
            };
            let sprinkler = word.contains("sprinkler") || word == "head";
            let kind = match text(CMD, p, "kind")?.filter(|k| !k.is_empty()) {
                Some(k) => k.to_ascii_lowercase().replace(' ', "_"),
                None if sprinkler => "sprinkler_pendent".into(),
                None if matches!(word.as_str(), "register" | "grille" | "diffuser") => word.clone(),
                None => "diffuser".into(),
            };
            let r = rules();
            let size = size_in(CMD, p)?.unwrap_or(if kind.contains("sprinkler") { r.sprinkler_size_in } else { r.terminal_size_in });
            let ceiling = sp.level(&level).map(|l| l.ceiling_ft).filter(|c| c.is_finite()).unwrap_or(0.0);
            let bottom = bottom_param(CMD, p, "bottom_ft", &sp, &level)?.unwrap_or(ceiling);
            e.level.clone_from(&level);
            e.system.clone_from(&system);
            e.points = vec![at];
            e.terminal = Some(MepTerminal {
                system,
                level,
                at,
                size_in: size,
                bottom_ft: bottom,
                terminal_kind: kind,
                objects: Vec::new(),
                source_sheet,
                made,
                basis,
                corrections: Vec::new(),
                tag: String::new(),
                cfm: None,
            });
        }
        El::Equipment => {
            let tag = text(CMD, p, "tag")?.filter(|t| !t.is_empty()).ok_or_else(|| bad(CMD, "`tag` is required (the equipment's mark: FCU-1)"))?;
            let kind = text(CMD, p, "kind")?
                .filter(|t| !t.is_empty())
                .ok_or_else(|| bad(CMD, "`kind` is required (ahu, fan_coil, exhaust_fan, pump, water_heater…)"))?;
            let level = level_of(&text(CMD, p, "level")?.ok_or_else(|| bad(CMD, "`level` is required"))?)?;
            if v.mep.equipment.iter().any(|q| q.tag.eq_ignore_ascii_case(&tag) && same_level(&v, &q.level, &level)) {
                return Err(bad(CMD, format!("{level} has equipment {tag} already: mep.move, mep.resize or mep.set change it")));
            }
            let at = pair(CMD, p, "at", 1e12)?.ok_or_else(|| bad(CMD, "`at` ([x, y], its centre, drawing units) is required"))?;
            let size = pair(CMD, p, "size_ft", MAX_EQUIPMENT_FT)?
                .filter(|s| s[0] > 0.0 && s[1] > 0.0)
                .ok_or_else(|| bad(CMD, format!("`size_ft` ([along x, along y], feet, each above 0 and at most {MAX_EQUIPMENT_FT}) is required")))?;
            let height = super::size_param(CMD, p, "height_ft", false)?
                .filter(|h| *h <= MAX_EQUIPMENT_HEIGHT_FT)
                .ok_or_else(|| bad(CMD, format!("`height_ft` (above 0, at most {MAX_EQUIPMENT_HEIGHT_FT}) is required")))?;
            let bottom = bottom_param(CMD, p, "bottom_ft", &sp, &level)?.unwrap_or(0.0);
            let systems = match p.get("systems").filter(|x| !x.is_null()) {
                None => Vec::new(),
                Some(x) => x
                    .as_array()
                    .filter(|a| a.len() <= 32)
                    .ok_or_else(|| bad(CMD, "`systems` is a list of at most 32 system names"))?
                    .iter()
                    .map(|n| n.as_str().map(|n| system_named(CMD, &sp, n, false)).unwrap_or_else(|| Err(bad(CMD, "`systems` holds names"))))
                    .collect::<Result<Vec<_>>>()?,
            };
            let (hx, hy) = (size[0] * upf / 2.0, size[1] * upf / 2.0);
            let mut q = MepEquipment {
                tag: tag.clone(),
                kind: kind.to_ascii_lowercase().replace(' ', "_"),
                level: level.clone(),
                min: [at[0] - hx, at[1] - hy],
                max: [at[0] + hx, at[1] + hy],
                bottom_ft: bottom,
                height_ft: height,
                systems,
                type_mark: text(CMD, p, "type_mark")?.unwrap_or_default(),
                properties: props_param(CMD, p)?.into_iter().filter(|(_, v)| !v.is_empty()).collect(),
                confidence: 1.0,
                ..MepEquipment::default()
            };
            q.sources.insert("position".into(), if made.is_drawn() { "drawn (mep.add)".into() } else { format!("{}: {basis}", made.as_str()) });
            if !source_sheet.is_empty() {
                q.sources.insert("sheet".into(), source_sheet);
            }
            if !made.is_drawn() {
                q.sources.insert("made".into(), made.as_str().into());
                q.sources.insert("basis".into(), basis);
            }
            e.level = level;
            e.tag = tag;
            e.points = vec![at];
            e.equipment = Some(q);
        }
    }
    commit(CMD, s, p, sp, e)
}

/// `properties`: an object of text, numbers or null (an empty text removes one).
fn props_param(cmd: &str, p: &Value) -> Result<BTreeMap<String, String>> {
    let Some(o) = p.get("properties").filter(|x| !x.is_null()) else { return Ok(BTreeMap::new()) };
    let o =
        o.as_object().filter(|o| o.len() <= MAX_PROPS).ok_or_else(|| bad(cmd, format!("`properties` is an object of at most {MAX_PROPS} names")))?;
    let mut out = BTreeMap::new();
    for (k, v) in o {
        let k = k.trim();
        if k.is_empty() || k.chars().count() > 64 || k.contains('=') {
            return Err(bad(cmd, "a property's name is 1 to 64 characters, without `=`"));
        }
        let t = match v {
            Value::Null => String::new(),
            Value::String(t) if t.chars().count() <= MAX_TEXT => t.trim().to_string(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(bad(cmd, format!("property {k}: text of at most {MAX_TEXT} characters, a number or null"))),
        };
        out.insert(k.to_string(), t);
    }
    Ok(out)
}

/// The edit of `op` on the element `p` names, its key filled in.
fn keyed(cmd: &str, s: &Session, p: &Value, op: &str) -> Result<(BuildingSpec, BuildingSpec, At, MepEdit)> {
    let (sp, v, _) = views(s, cmd)?;
    let at = target(cmd, s, &v, p)?;
    let mut e = key_of(&v, at).ok_or_else(|| bad(cmd, "the element is gone"))?;
    e.op = op.into();
    Ok((sp, v, at, e))
}

fn run_move(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mep.move";
    let (sp, v, at, mut e) = keyed(CMD, s, p, "move")?;
    let upf = upf(&sp);
    let delta = match (pair(CMD, p, "delta_ft", MAX_MOVE_FT)?, pair(CMD, p, "to", 1e12)?) {
        (Some(_), Some(_)) => return Err(bad(CMD, "give `delta_ft` or `to`, not both")),
        (Some(d), None) => Some([d[0] * upf, d[1] * upf]),
        (None, Some(t)) => {
            let now = match at {
                At::Run(i) => v.mep.runs.get(i).and_then(|r| r.points.first().copied()),
                At::Terminal(i) => v.mep.terminals.get(i).map(|t| t.at),
                At::Equipment(i) => v.mep.equipment.get(i).map(centre),
            };
            now.map(|n| [t[0] - n[0], t[1] - n[1]])
        }
        (None, None) => None,
    };
    e.delta = delta.filter(|d| d[0] != 0.0 || d[1] != 0.0);
    e.bottom_ft = bottom_param(CMD, p, "bottom_ft", &sp, &e.level)?;
    if e.delta.is_none() && e.bottom_ft.is_none() {
        return Err(bad(CMD, "give `delta_ft` (feet), `to` ([x, y]) or `bottom_ft`"));
    }
    commit(CMD, s, p, sp, e)
}

fn run_resize(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mep.resize";
    let (sp, _, at, mut e) = keyed(CMD, s, p, "resize")?;
    let upf = upf(&sp);
    match at {
        At::Equipment(_) => {
            if p.get("size_in").is_some_and(|x| !x.is_null()) {
                return Err(bad(CMD, "equipment is resized by `size_ft` ([along x, along y]) and `height_ft`"));
            }
            e.footprint = pair(CMD, p, "size_ft", MAX_EQUIPMENT_FT)?
                .map(|f| if f[0] > 0.0 && f[1] > 0.0 { Ok([f[0] * upf, f[1] * upf]) } else { Err(bad(CMD, "`size_ft` sides are above 0")) })
                .transpose()?;
            e.height_ft = super::size_param(CMD, p, "height_ft", false)?
                .map(
                    |h| if h <= MAX_EQUIPMENT_HEIGHT_FT { Ok(h) } else { Err(bad(CMD, format!("`height_ft` is at most {MAX_EQUIPMENT_HEIGHT_FT}"))) },
                )
                .transpose()?;
            if e.footprint.is_none() && e.height_ft.is_none() {
                return Err(bad(CMD, "give `size_ft` or `height_ft`"));
            }
        }
        _ => {
            e.size_in = Some(size_in(CMD, p)?.ok_or_else(|| bad(CMD, "`size_in` ([w, h] or [d], inches) is required"))?);
        }
    }
    commit(CMD, s, p, sp, e)
}

fn run_delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mep.delete";
    let (sp, _, _, e) = keyed(CMD, s, p, "delete")?;
    commit(CMD, s, p, sp, e)
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mep.set";
    let (sp, _, at, mut e) = keyed(CMD, s, p, "set")?;
    let is_eq = matches!(at, At::Equipment(_));
    e.type_mark = text(CMD, p, "type_mark")?.filter(|m| !m.is_empty());
    if e.type_mark.is_some() && !is_eq {
        return Err(bad(CMD, "`type_mark` is set on equipment (a run's or terminal's type is its system and size)"));
    }
    if let Some(sy) = text(CMD, p, "new_system")?.filter(|x| !x.is_empty()) {
        if is_eq {
            return Err(bad(CMD, "equipment's systems are not changed here: `new_system` is a run's or terminal's"));
        }
        e.new_system = Some(system_named(CMD, &sp, &sy, matches!(at, At::Run(_)))?);
    }
    e.kind = text(CMD, p, "kind")?.filter(|k| !k.is_empty()).map(|k| k.to_ascii_lowercase().replace(' ', "_"));
    if e.kind.is_some() && matches!(at, At::Run(_)) {
        return Err(bad(CMD, "a run's kind is its system's: mep.set `new_system`"));
    }
    e.properties = props_param(CMD, p)?;
    if e.type_mark.is_none() && e.new_system.is_none() && e.kind.is_none() && e.properties.is_empty() {
        return Err(bad(CMD, "give `type_mark`, `new_system`, `kind` or `properties`"));
    }
    commit(CMD, s, p, sp, e)
}

fn run_edits(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mep.edits";
    bool_param(CMD, p, "rebuild")?;
    let (mut sp, _, _) = views(s, CMD)?;
    let mut reverted = Value::Null;
    if let Some(id) = text(CMD, p, "revert")?.filter(|x| !x.is_empty()) {
        let i = sp.mep.edits.iter().position(|e| e.id.eq_ignore_ascii_case(&id)).ok_or_else(|| bad(CMD, format!("no edit {id}")))?;
        let e = sp.mep.edits.remove(i);
        s.doc_mut()?.building_spec = Some(sp.to_json()).into();
        reverted = serde_json::to_value(&e).unwrap_or(Value::Null);
        s.echo(format!("{} taken back: the {} is as the documents give it again.", e.id, e.element));
    }
    let (_, _, out) = views(s, CMD)?;
    let only_el = text(CMD, p, "element")?
        .filter(|x| !x.is_empty())
        .map(|x| El::parse(&x).map(El::as_str).ok_or_else(|| bad(CMD, "`element` is run, terminal or equipment")))
        .transpose()?;
    let only_status = text(CMD, p, "status")?.filter(|x| !x.is_empty());
    if only_status.as_deref().is_some_and(|x| x != "applied" && x != "orphaned") {
        return Err(bad(CMD, "`status` is applied or orphaned"));
    }
    let status = |id: &str| out.iter().find(|o| o.id == id);
    let rows: Vec<Value> = sp
        .mep
        .edits
        .iter()
        .filter(|e| only_el.is_none_or(|x| e.element == x))
        .filter_map(|e| {
            let o = status(&e.id);
            let st = if o.is_some_and(|o| o.applied) { "applied" } else { "orphaned" };
            if only_status.as_deref().is_some_and(|x| x != st) {
                return None;
            }
            let mut j = json!({"id": e.id, "op": e.op, "element": e.element, "level": e.level, "system": e.system, "tag": e.tag, "at": e.points.first(), "reason": e.reason, "by": e.by, "status": st});
            if let Some(o) = o.filter(|o| !o.applied) {
                j["why"] = json!(o.why);
            }
            if !e.added_by.is_empty() {
                j["added_by"] = json!(e.added_by);
            }
            Some(j)
        })
        .collect();
    let cursor = count_param(CMD, p, "cursor", 0, u64::MAX)?.unwrap_or(0) as usize;
    let limit = count_param(CMD, p, "limit", 1, MAX_PAGE)?.unwrap_or(50) as usize;
    let total = rows.len();
    let page: Vec<Value> = rows.into_iter().skip(cursor).take(limit).collect();
    let mut r = json!({
        "edits": page, "total": total,
        "applied": out.iter().filter(|o| o.applied).count(), "orphaned": out.iter().filter(|o| !o.applied).count(),
    });
    if cursor.saturating_add(limit) < total {
        r["next_cursor"] = json!(cursor + limit);
    }
    if !reverted.is_null() {
        r["reverted"] = reverted;
        rebuild(CMD, s, p, &mut r)?;
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> BuildingSpec {
        serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "ceiling_ft": 9.0}],
            "mep": {
                "systems": [{"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 10.0}],
                "runs": [{"system": "SA", "level": "Level 1", "points": [[0, 0], [240, 0]], "size_in": [12, 8], "bottoms_ft": [10.0], "id": "R1"}],
                "terminals": [{"system": "SA", "level": "Level 1", "at": [120, 24], "size_in": [12, 12], "bottom_ft": 9.0, "terminal_kind": "register"}],
                "equipment": [{"tag": "AHU-1", "kind": "ahu", "level": "Level 1", "min": [-60, -24], "max": [-12, 24], "bottom_ft": 0.0, "height_ft": 5.0, "systems": ["SA"], "confidence": 1.0}]
            }
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_rules_are_data_with_their_bases() {
        let r = rules();
        assert!(r.match_ft > 0.0 && !r.match_basis.is_empty());
        assert!(r.equipment_reach_ft > 0.0 && !r.equipment_basis.is_empty());
        assert!(r.sprinkler_reach_ft > 0.0 && !r.sprinkler_basis.is_empty());
        assert!(super::super::provenance::rule(&r.terminal_rule).connect_reach_ft > 0.0, "the terminal reach rule is in tradepractice.json");
        assert!(!r.unconnected_kinds.is_empty() && r.terminal_size_in[0] > 0.0);
    }

    #[test]
    fn edits_apply_to_the_lists_as_read_again_and_keep_the_design_value() {
        let mut sp = spec();
        sp.mep.edits = vec![
            MepEdit {
                id: "ME-1".into(),
                op: "move".into(),
                element: "terminal".into(),
                level: "Level 1".into(),
                system: "SA".into(),
                points: vec![[120.0, 24.0]],
                delta: Some([24.0, 0.0]),
                by: "user".into(),
                ..MepEdit::default()
            },
            MepEdit {
                id: "ME-2".into(),
                op: "set".into(),
                element: "equipment".into(),
                level: "Level 1".into(),
                tag: "AHU-1".into(),
                type_mark: Some("AHU-B".into()),
                ..MepEdit::default()
            },
            MepEdit {
                id: "ME-3".into(),
                op: "delete".into(),
                element: "run".into(),
                level: "Level 1".into(),
                system: "SA".into(),
                points: vec![[0.0, 0.0], [240.0, 0.0]],
                ..MepEdit::default()
            },
        ];
        // As the plan readers write the lists again: the run read a little off, in the other direction.
        sp.mep.runs[0].points = vec![[240.0, 0.5], [0.5, 0.0]];
        let out = apply(&mut sp);
        assert!(out.iter().all(|o| o.applied), "{out:?}");
        assert_eq!(sp.mep.terminals[0].at, [144.0, 24.0]);
        assert_eq!(sp.mep.terminals[0].corrections[0].design, "10.00,2.00 ft");
        assert_eq!(sp.mep.terminals[0].corrections[0].corrected, "12.00,2.00 ft");
        assert_eq!(sp.mep.equipment[0].type_mark, "AHU-B");
        assert_eq!(sp.mep.equipment[0].corrections[0].design, "AHU-1");
        assert!(sp.mep.runs.is_empty());
        assert_eq!(sp.mep.edits.len(), 3, "the edits stay");
    }

    #[test]
    fn an_edit_whose_element_the_documents_dropped_is_orphaned_not_misapplied() {
        let mut sp = spec();
        sp.mep.edits = vec![MepEdit {
            id: "ME-1".into(),
            op: "delete".into(),
            element: "terminal".into(),
            level: "Level 1".into(),
            system: "SA".into(),
            points: vec![[600.0, 600.0]],
            ..MepEdit::default()
        }];
        let out = apply(&mut sp);
        assert!(!out[0].applied && out[0].why.contains("any more"), "{out:?}");
        assert_eq!(sp.mep.terminals.len(), 1);
    }

    #[test]
    fn an_added_element_carries_its_edit_and_later_edits_find_it_by_that() {
        let mut sp = spec();
        let mut run = sp.mep.runs[0].clone();
        run.id = String::new();
        run.points = vec![[240.0, 0.0], [240.0, 120.0]];
        run.made = Made::Inferred;
        run.basis = "added by user".into();
        sp.mep.edits = vec![
            MepEdit { id: "ME-1".into(), op: "add".into(), element: "run".into(), run: Some(run), ..MepEdit::default() },
            MepEdit {
                id: "ME-2".into(),
                op: "resize".into(),
                element: "run".into(),
                added_by: "ME-1".into(),
                size_in: Some([10.0, 6.0]),
                ..MepEdit::default()
            },
        ];
        let out = apply(&mut sp);
        assert!(out.iter().all(|o| o.applied), "{out:?}");
        let added = sp.mep.runs.iter().find(|r| added_by_run(r) == Some("ME-1")).unwrap_or_else(|| panic!("added"));
        assert_eq!((added.id.as_str(), added.size_in, added.made), ("ME-1", [10.0, 6.0], Made::Inferred));
        assert_eq!(added.corrections[0].rule, "mep edit ME-2 (user)");
        // Applying again adds nothing twice.
        let mut again = sp.clone();
        apply(&mut again);
        assert_eq!(again.mep.runs.len(), sp.mep.runs.len());
    }

    #[test]
    fn a_path_distance_is_to_the_nearest_segment() {
        assert!((path_dist(&[[0.0, 0.0], [10.0, 0.0]], [5.0, 3.0]) - 3.0).abs() < 1e-9);
        assert!((path_dist(&[[0.0, 0.0]], [3.0, 4.0]) - 5.0).abs() < 1e-9);
        assert!(path_dist(&[], [0.0, 0.0]).is_infinite());
    }
}
