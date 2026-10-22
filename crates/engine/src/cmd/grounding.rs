//! Grounding and bonding as real routed elements with quantities (Q329): the grounding electrode
//! system (metal water pipe, concrete-encased "Ufer", rods, ring, building steel) and each
//! electrode's conductor (GEC) from the service; the bonds (water and gas piping, other metal
//! piping and steel, the main and system bonding jumpers, separately derived systems) and the
//! telecom grounding (the bonding conductor from the service to the telecom main grounding
//! busbar and to every data room's busbar). Sizes as drawn: no sizing calculations.
//!
//! - `grounding.entries`: where cold water and gas enter (given, or found from the plumbing and
//!   gas runs), and whether their clamp can be reached (not above a gypsum ceiling).
//! - `grounding.route {all?}`: every conductor routed in the model: up to the level's routing
//!   height, across, up or down the risers, to its end (vertical runs counted).
//! - `grounding.check {review?}`: what the drawings call for that isn't modelled or routed,
//!   clamps that can't be reached, data rooms without a busbar or its bond, references to nothing,
//!   and the concrete-encased electrode against its note (length, conductor or rebar, cover,
//!   earth contact, an accessible connection) and its pour hold point; `review` makes each an RFI.
//! - `grounding.alternative {electrode, kind?}`: a missed Ufer (poured, not documented) modelled as
//!   ground rods or a ring instead, with its takeoff.
//! - `grounding.takeoff`: the required items: conductor feet by size, clamps, rods, busbars,
//!   exothermic welds or compression connectors, lugs, bonding bushings and GEC conduit, test
//!   wells, and the tools (also on `mep.qty`, part `grounding`).
//!
//! Project rules: `custom/ground_connection` (`exothermic`, `compression` or `mechanical`),
//! `custom/ground_test_wells` (`yes`/`no`), `custom/ground_makeup_ft` (per conductor end).

use std::collections::BTreeMap;

use cadaddy_doc::Drawing;
use cadaddy_doc::projrules::RuleDomain;
use cadaddy_walk3d::{BuildingSpec, Electrode, Grounding, MepKind, PointOfEntry};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most conductors routed and items counted.
const MAX_ITEMS: usize = 10_000;

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// The gear the grounding starts from.
fn service<'a>(sp: &'a BuildingSpec, g: &Grounding) -> Option<&'a cadaddy_walk3d::MepEquipment> {
    let eq = &sp.mep.equipment;
    if !g.service.trim().is_empty() {
        return eq.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(g.service.trim()));
    }
    eq.iter().find(|e| e.kind.eq_ignore_ascii_case("switchboard")).or_else(|| eq.iter().find(|e| e.tag.eq_ignore_ascii_case("MDP")))
}

fn point_in(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// Whether a clamp at `at`, `bottom_ft` above `level`'s floor, can be reached: not above a
/// gypsum (hard) ceiling.
fn reachable(sp: &BuildingSpec, level: &str, at: [f64; 2], bottom_ft: f64) -> (bool, String) {
    for c in sp.rcp.ceilings.iter().filter(|c| c.level.eq_ignore_ascii_case(level)) {
        if c.outline.len() >= 3 && point_in(&c.outline, at) && bottom_ft > c.height_ft && c.grid.is_none() {
            let room = c.room.clone().unwrap_or_else(|| "the room".into());
            return (false, format!("above the gypsum ceiling of {room} ({:.1} ft): no access panel shown", c.height_ft));
        }
    }
    (true, String::new())
}

fn system_kind(name: &str, layer: &str) -> Option<&'static str> {
    let t = format!("{} {}", name.to_ascii_uppercase(), layer.to_ascii_uppercase());
    if t.contains("GAS") {
        Some("gas")
    } else if t.contains("CW") || t.contains("COLD") || t.contains("DOMESTIC WATER") || t.contains("WATER SERVICE") {
        Some("water")
    } else {
        None
    }
}

/// The water and gas points of entry: given, else found from the runs (a dead end of the
/// system's piping on its lowest level, the one nearest the edge of everything on that level).
pub fn entries(sp: &BuildingSpec) -> Vec<PointOfEntry> {
    let g = sp.mep.grounding.clone().unwrap_or_default();
    let mut out: Vec<PointOfEntry> = g.entries.clone();
    for kind in ["water", "gas"] {
        if out.iter().any(|e| e.kind == kind) {
            continue;
        }
        let systems: Vec<&str> = sp
            .mep
            .systems
            .iter()
            .filter(|s| s.kind == MepKind::Pipe && system_kind(&s.name, &s.layer) == Some(kind))
            .map(|s| s.name.as_str())
            .collect();
        let runs: Vec<&cadaddy_walk3d::MepRun> = sp.mep.runs.iter().filter(|r| systems.contains(&r.system.as_str())).collect();
        let Some(level) = sp
            .levels
            .iter()
            .filter(|l| runs.iter().any(|r| r.level.eq_ignore_ascii_case(&l.name)))
            .min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft))
        else {
            continue;
        };
        let on: Vec<&&cadaddy_walk3d::MepRun> = runs.iter().filter(|r| r.level.eq_ignore_ascii_case(&level.name)).collect();
        let mut ends: Vec<([f64; 2], f64)> = Vec::new();
        for r in &on {
            if let (Some(a), Some(b)) = (r.points.first(), r.points.last()) {
                ends.push((*a, r.bottoms_ft.first().copied().unwrap_or(0.0)));
                ends.push((*b, r.bottoms_ft.last().copied().unwrap_or(0.0)));
            }
        }
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() < 1.0;
        let dead: Vec<([f64; 2], f64)> = ends.iter().copied().filter(|(p, _)| ends.iter().filter(|(q, _)| near(*p, *q)).count() == 1).collect();
        // The edge: everything modelled on the level.
        let pts: Vec<[f64; 2]> = sp
            .mep
            .runs
            .iter()
            .filter(|r| r.level.eq_ignore_ascii_case(&level.name))
            .flat_map(|r| r.points.iter().copied())
            .chain(sp.mep.equipment.iter().filter(|e| e.level.eq_ignore_ascii_case(&level.name)).flat_map(|e| [e.min, e.max]))
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .collect();
        let (lo, hi) = pts.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
            ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
        });
        let edge = |p: [f64; 2]| (p[0] - lo[0]).abs().min((hi[0] - p[0]).abs()).min((p[1] - lo[1]).abs()).min((hi[1] - p[1]).abs());
        let Some((at, bottom)) = dead.into_iter().min_by(|a, b| edge(a.0).total_cmp(&edge(b.0))) else { continue };
        let (accessible, why) = reachable(sp, &level.name, at, bottom);
        out.push(PointOfEntry {
            tag: if kind == "water" { "POE-W".into() } else { "POE-G".into() },
            kind: kind.into(),
            level: level.name.clone(),
            at,
            z_ft: level.elevation_ft + bottom,
            accessible,
            why,
        });
    }
    out
}

/// Where a tag is: (level, plan point, elevation in feet).
fn place(sp: &BuildingSpec, g: &Grounding, poes: &[PointOfEntry], tag: &str) -> Option<(String, [f64; 2], f64)> {
    let t = tag.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(e) = sp.mep.equipment.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(t)) {
        let el = sp.level(&e.level).map_or(0.0, |l| l.elevation_ft);
        return Some((e.level.clone(), [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0], el + e.bottom_ft + 0.5));
    }
    if let Some(b) = g.busbars.iter().find(|b| b.tag.trim().eq_ignore_ascii_case(t)) {
        let el = sp.level(&b.level).map_or(0.0, |l| l.elevation_ft);
        return Some((b.level.clone(), b.at, if b.z_ft != 0.0 { b.z_ft } else { el + 7.0 }));
    }
    if let Some(p) = poes.iter().find(|p| p.tag.trim().eq_ignore_ascii_case(t)) {
        return Some((p.level.clone(), p.at, p.z_ft));
    }
    g.electrodes.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(t)).map(|e| (e.level.clone(), e.at, e.z_ft))
}

/// A conductor's route: up to the start level's routing height, across (x then y), along the
/// riser to the end level's routing height, then to the end.
fn route(sp: &BuildingSpec, a: (&str, [f64; 2], f64), b: (&str, [f64; 2], f64)) -> Vec<[f64; 3]> {
    let height =
        |lv: &str| sp.level(lv).map_or(9.0, |l| l.elevation_ft + if l.ceiling_ft > 0.0 { l.ceiling_ft + 0.5 } else { (l.height_ft - 1.5).max(7.0) });
    let (za, zb) = (height(a.0), height(b.0));
    let mut p =
        vec![[a.1[0], a.1[1], a.2], [a.1[0], a.1[1], za], [b.1[0], a.1[1], za], [b.1[0], b.1[1], za], [b.1[0], b.1[1], zb], [b.1[0], b.1[1], b.2]];
    p.dedup_by(|x, y| (x[0] - y[0]).abs() < 1e-9 && (x[1] - y[1]).abs() < 1e-9 && (x[2] - y[2]).abs() < 1e-9);
    p
}

/// A route's length, feet (plan in drawing units, elevations in feet).
pub fn length_ft(path: &[[f64; 3]], upf: f64) -> f64 {
    path.windows(2)
        .map(|w| match w {
            [a, b] => (((b[0] - a[0]) / upf).powi(2) + ((b[1] - a[1]) / upf).powi(2) + (b[2] - a[2]).powi(2)).sqrt(),
            _ => 0.0,
        })
        .filter(|v| v.is_finite())
        .sum()
}

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn grounding(cmd: &str, sp: &BuildingSpec) -> Result<Grounding> {
    sp.mep.grounding.clone().ok_or_else(|| bad(cmd, "the building spec has no grounding (mep.grounding)"))
}

fn run_entries(s: &mut Session, _p: &Value) -> Result<Value> {
    let sp = spec("grounding.entries", s)?;
    let e = entries(&sp);
    Ok(
        json!({"entries": e.iter().map(|x| json!({"tag": x.tag, "kind": x.kind, "level": x.level, "at": x.at, "z_ft": x.z_ft, "accessible": x.accessible, "why": x.why})).collect::<Vec<_>>()}),
    )
}

fn run_route(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("grounding.route", s)?;
    let mut g = grounding("grounding.route", &sp)?;
    let all = bool_or(p, "all", false);
    let poes = entries(&sp);
    let svc =
        service(&sp, &g).map(|e| e.tag.clone()).ok_or_else(|| bad("grounding.route", "no service gear to start from (set grounding.service)"))?;
    let (mut routed, mut failed) = (Vec::new(), Vec::new());
    let u = upf(&sp);
    let snapshot = g.clone();
    for e in g.electrodes.iter_mut().take(MAX_ITEMS) {
        if !all && !e.path.is_empty() {
            continue;
        }
        let end = if e.entry.trim().is_empty() { Some((e.level.clone(), e.at, e.z_ft)) } else { place(&sp, &snapshot, &poes, &e.entry) };
        match (place(&sp, &snapshot, &poes, &svc), end) {
            (Some(a), Some(b)) => {
                e.path = route(&sp, (&a.0, a.1, a.2), (&b.0, b.1, b.2));
                routed.push(json!({"tag": e.tag, "length_ft": (length_ft(&e.path, u) * 10.0).round() / 10.0}));
            }
            _ => failed.push(e.tag.clone()),
        }
    }
    for b in g.bonds.iter_mut().take(MAX_ITEMS) {
        if !all && !b.path.is_empty() {
            continue;
        }
        let from = if b.from.trim().is_empty() { svc.clone() } else { b.from.clone() };
        let to = if b.to.trim().is_empty() { Some((b.level.clone(), b.at, b.z_ft)) } else { place(&sp, &snapshot, &poes, &b.to) };
        match (place(&sp, &snapshot, &poes, &from), to) {
            (Some(a), Some(t)) => {
                b.path = route(&sp, (&a.0, a.1, a.2), (&t.0, t.1, t.2));
                routed.push(json!({"tag": b.tag, "length_ft": (length_ft(&b.path, u) * 10.0).round() / 10.0}));
            }
            _ => failed.push(b.tag.clone()),
        }
    }
    if !routed.is_empty() {
        sp.mep.grounding = Some(g);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({"routed": routed, "failed": failed}))
}

/// One thing the check found.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub about: String,
    pub check: &'static str,
    pub error: bool,
    pub message: String,
    pub fix: String,
}

fn is_data_room(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["DATA", "IDF", "MDF", "TELECOM", "COMM ROOM"].iter().any(|k| n.contains(k))
}

/// What the grounding is missing or gets wrong.
pub fn check(sp: &BuildingSpec) -> Vec<Finding> {
    let Some(g) = sp.mep.grounding.as_ref() else { return Vec::new() };
    let poes = entries(sp);
    let mut out = Vec::new();
    let mut add = |about: &str, check: &'static str, error: bool, message: String, fix: &str| {
        if out.len() < MAX_ITEMS {
            out.push(Finding { about: about.into(), check, error, message, fix: fix.into() });
        }
    };
    if service(sp, g).is_none() {
        add("service", "service", true, "no service gear to start the grounding from".into(), "Set grounding.service to the switchboard's tag");
    }
    // What the drawings call for: modelled, and routed.
    for r in &g.required {
        let to = r.to.trim();
        let matches_to = |t: &str| to.is_empty() || t.trim().eq_ignore_ascii_case(to);
        let electrode = g.electrodes.iter().find(|e| e.kind.eq_ignore_ascii_case(&r.kind) && (matches_to(&e.entry) || matches_to(&e.tag)));
        let bond = g.bonds.iter().find(|b| b.kind.eq_ignore_ascii_case(&r.kind) && matches_to(&b.to));
        let what = if to.is_empty() { r.kind.replace('_', " ") } else { format!("{} to {to}", r.kind.replace('_', " ")) };
        match (electrode.map(|e| e.path.is_empty()), bond.map(|b| b.path.is_empty())) {
            (None, None) => add(
                &what,
                "not modelled",
                true,
                format!("the drawings call for {what} (\"{}\") but it isn't in the model", r.note),
                "Model it (and route it)",
            ),
            (Some(true), _) | (_, Some(true)) => {
                add(&what, "not routed", true, format!("{what} (\"{}\") is modelled but not routed", r.note), "grounding.route")
            }
            _ => {}
        }
    }
    // Clamps that can't be reached, references to nothing.
    for (tag, target) in g
        .electrodes
        .iter()
        .filter(|e| !e.entry.trim().is_empty())
        .map(|e| (e.tag.as_str(), e.entry.as_str()))
        .chain(g.bonds.iter().map(|b| (b.tag.as_str(), b.to.as_str())))
    {
        if target.trim().is_empty() {
            continue;
        }
        match poes.iter().find(|p| p.tag.trim().eq_ignore_ascii_case(target.trim())) {
            Some(p) if !p.accessible => add(
                tag,
                "inaccessible clamp",
                true,
                format!("the {} clamp for {tag} at {} isn't accessible: {}", p.kind, p.tag, p.why),
                "Move the clamp to an accessible spot on the pipe, or add an access panel",
            ),
            Some(_) => {}
            None if place(sp, g, &poes, target).is_none() => add(
                tag,
                "unknown",
                true,
                format!("{tag} goes to {target}, which isn't in the model"),
                "Give the tag of a gear, busbar, point of entry or electrode",
            ),
            None => {}
        }
    }
    for b in &g.bonds {
        if !b.from.trim().is_empty() && place(sp, g, &poes, &b.from).is_none() {
            add(&b.tag, "unknown", true, format!("{} starts at {}, which isn't in the model", b.tag, b.from), "Give the tag of a gear or busbar");
        }
    }
    // Every data room: a busbar, and its bonding conductor.
    for room in sp.rooms.iter().filter(|r| is_data_room(&r.name)) {
        let names: Vec<&str> = std::iter::once(room.name.as_str()).chain(room.number.as_deref()).collect();
        let bar = g.busbars.iter().find(|b| names.iter().any(|n| b.room.trim().eq_ignore_ascii_case(n.trim())));
        match bar {
            None => add(
                &room.name,
                "data room",
                true,
                format!("data room {} has no grounding busbar", room.name),
                "Add its TGB and the bonding conductor to it",
            ),
            Some(bar) if !g.bonds.iter().any(|b| b.to.trim().eq_ignore_ascii_case(bar.tag.trim())) => add(
                &room.name,
                "data room",
                true,
                format!("data room {}'s busbar {} has no bonding conductor", room.name, bar.tag),
                "Bond it to the TMGB or the service ground",
            ),
            Some(_) => {}
        }
    }
    // The concrete-encased electrode against its note, and its pour hold point.
    let note = &g.ufer_note;
    for e in g.electrodes.iter().filter(|e| e.kind.eq_ignore_ascii_case("concrete_encased")) {
        let replaced = g.electrodes.iter().any(|x| x.alternative_of.eq_ignore_ascii_case(&e.tag));
        if e.length_ft + 1e-9 < note.length_ft {
            add(
                &e.tag,
                "ufer",
                true,
                format!("{}: {} ft in the footing, the note calls for {} ft", e.tag, e.length_ft, note.length_ft),
                "Lengthen it to the note's length",
            );
        }
        if e.material.eq_ignore_ascii_case("rebar") {
            if e.rebar_in + 1e-9 < note.min_rebar_in {
                add(
                    &e.tag,
                    "ufer",
                    true,
                    format!("{}: {}\" rebar, at least {}\" called for", e.tag, e.rebar_in, note.min_rebar_in),
                    "Bond rebar at least the note's size",
                );
            }
        } else if let (Some(have), Some(need)) = (super::gear::cmil(&e.conductor), super::gear::cmil(&note.min_conductor))
            && have < need
        {
            add(
                &e.tag,
                "ufer",
                true,
                format!("{}: {} bare copper, at least {} called for", e.tag, e.conductor, note.min_conductor),
                "Use the note's conductor size",
            );
        }
        if e.cover_in + 1e-9 < note.cover_in {
            add(
                &e.tag,
                "ufer",
                true,
                format!("{}: {}\" of concrete cover, at least {}\" called for", e.tag, e.cover_in, note.cover_in),
                "Set it deeper in the footing",
            );
        }
        if !e.earth_contact {
            add(
                &e.tag,
                "ufer",
                true,
                format!("{} isn't in a part of the footing in direct contact with earth", e.tag),
                "Move it to the part of the footing on earth (not over a vapour barrier)",
            );
        }
        if !e.connection_accessible {
            add(&e.tag, "ufer", true, format!("{}'s stub-up connection isn't accessible", e.tag), "Bring the stub-up to an accessible location");
        }
        if e.pour.eq_ignore_ascii_case("poured") && !e.documented {
            add(
                &e.tag,
                "pour",
                !replaced,
                if replaced {
                    format!("{}: the footing is poured and the electrode isn't documented; the alternative electrode is modelled", e.tag)
                } else {
                    format!("{}: the footing is poured and the electrode isn't documented as installed", e.tag)
                },
                "Document it (photos, inspection), or model the alternative electrode (grounding.alternative)",
            );
        } else if !e.pour.eq_ignore_ascii_case("poured") {
            add(&e.tag, "pour", false, format!("{}: a pour hold point: sign it off in 3D before the concrete", e.tag), "");
        }
    }
    // Unrouted conductors: a warning (an error when a note calls for them, above).
    let called_out = out.iter().any(|f| f.check == "not routed");
    for (tag, empty) in g.electrodes.iter().map(|e| (&e.tag, e.path.is_empty())).chain(g.bonds.iter().map(|b| (&b.tag, b.path.is_empty()))) {
        if empty && !called_out && out.len() < MAX_ITEMS {
            out.push(Finding {
                about: tag.clone(),
                check: "unrouted",
                error: false,
                message: format!("{tag} isn't routed"),
                fix: "grounding.route".into(),
            });
        }
    }
    out
}

fn findings_json(f: &[Finding]) -> Vec<Value> {
    f.iter().map(|x| json!({"about": x.about, "check": x.check, "severity": if x.error { "error" } else { "warning" }, "message": x.message, "fix": x.fix})).collect()
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("grounding.check", s)?;
    grounding("grounding.check", &sp)?;
    let f = check(&sp);
    let errors = f.iter().filter(|x| x.error).count();
    let mut rfis = Vec::new();
    if bool_or(p, "review", false) {
        let svc_at = sp
            .mep
            .grounding
            .as_ref()
            .and_then(|g| service(&sp, g))
            .map(|e| (e.level.clone(), [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]));
        for x in f.iter().filter(|x| x.error) {
            let id = format!("RFI-E{}", hash(&format!("{}|{}", x.about, x.message)));
            if sp.rfis.iter().any(|r| r.id == id) || sp.rfis.len() >= cadaddy_walk3d::rfi::MAX_RFIS {
                continue;
            }
            let kind = cadaddy_walk3d::RfiKind::Grounding;
            sp.rfis.push(cadaddy_walk3d::Rfi {
                id: id.clone(),
                kind: Some(kind),
                severity: kind.severity(),
                level: svc_at.as_ref().map(|s| s.0.clone()).unwrap_or_default(),
                at: svc_at.as_ref().map_or([0.0, 0.0], |s| s.1),
                about: format!("{} (grounding)", x.about),
                issue: x.message.clone(),
                assumed: "Modelled as drawn meanwhile.".into(),
                proposed: (!x.fix.is_empty()).then(|| x.fix.clone()),
                ..cadaddy_walk3d::Rfi::default()
            });
            rfis.push(id);
        }
        if !rfis.is_empty() {
            s.doc_mut()?.building_spec = Some(sp.to_json());
        }
    }
    Ok(json!({"ok": errors == 0, "errors": errors, "findings": findings_json(&f), "rfis": rfis}))
}

fn run_alternative(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("grounding.alternative", s)?;
    let mut g = grounding("grounding.alternative", &sp)?;
    let tag = str_param(p, "electrode")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("grounding.alternative", "electrode required (the missed one's tag)"))?;
    let missed = g
        .electrodes
        .iter()
        .find(|e| e.tag.eq_ignore_ascii_case(tag))
        .cloned()
        .ok_or_else(|| bad("grounding.alternative", format!("no electrode tagged {}", tag.chars().take(40).collect::<String>())))?;
    if g.electrodes.iter().any(|e| e.alternative_of.eq_ignore_ascii_case(tag)) {
        return Err(bad("grounding.alternative", format!("{tag}'s alternative is already modelled")));
    }
    let kind = str_param(p, "kind").unwrap_or("rods").trim().to_ascii_lowercase();
    let level_el = sp.level(&missed.level).map_or(0.0, |l| l.elevation_ft);
    let alt = match kind.as_str() {
        "rods" => Electrode {
            tag: format!("{}-ALT", missed.tag),
            kind: "ground_rod".into(),
            level: missed.level.clone(),
            at: missed.at,
            z_ft: level_el - 1.0,
            gec_size: if missed.gec_size.is_empty() { "#4".into() } else { missed.gec_size.clone() },
            count: 2,
            rod_length_ft: 10.0,
            alternative_of: missed.tag.clone(),
            ..Electrode::default()
        },
        "ring" => Electrode {
            tag: format!("{}-ALT", missed.tag),
            kind: "ground_ring".into(),
            level: missed.level.clone(),
            at: missed.at,
            z_ft: level_el - 2.5,
            gec_size: if missed.gec_size.is_empty() { "#2".into() } else { missed.gec_size.clone() },
            conductor: "#2".into(),
            ring_length_ft: str_param(p, "ring_length_ft")
                .and_then(|v| v.parse().ok())
                .or_else(|| p.get("ring_length_ft").and_then(Value::as_f64))
                .filter(|v: &f64| v.is_finite() && *v > 0.0 && *v < 1e5)
                .unwrap_or(200.0),
            alternative_of: missed.tag.clone(),
            ..Electrode::default()
        },
        other => return Err(bad("grounding.alternative", format!("kind is rods or ring, not {}", other.chars().take(20).collect::<String>()))),
    };
    let out = json!({"tag": alt.tag, "kind": alt.kind, "alternative_of": alt.alternative_of});
    g.electrodes.push(alt);
    sp.mep.grounding = Some(g);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(out)
}

// ---------------- the takeoff ----------------

/// The required items: `[{item, unit, qty, note}]`.
pub fn takeoff(sp: &BuildingSpec, d: &Drawing) -> Vec<Value> {
    let Some(g) = sp.mep.grounding.as_ref() else { return Vec::new() };
    let rule = |key: &str| d.project_rules.resolve(RuleDomain::Custom, key).map(|v| v.value.trim().to_ascii_lowercase());
    let connection = rule("ground_connection").unwrap_or_else(|| "exothermic".into());
    let test_wells = rule("ground_test_wells").is_none_or(|v| !matches!(v.as_str(), "no" | "false" | "0"));
    let makeup = rule("ground_makeup_ft").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && (0.0..100.0).contains(v)).unwrap_or(1.0);
    let u = upf(sp);
    let poes = entries(sp);
    let mut items: Vec<Value> = Vec::new();
    let mut put = |item: String, unit: &str, qty: f64, note: String| {
        if qty > 0.0 {
            items.push(json!({"item": item, "unit": unit, "qty": (qty * 10.0).round() / 10.0, "note": note}));
        }
    };
    // Conductor by size: the routes (risers included), with makeup at each end; a bare Ufer
    // conductor and a ring by their lengths.
    let mut ft: BTreeMap<String, f64> = BTreeMap::new();
    let mut in_conduit = 0.0f64;
    let mut conduit_runs = 0u32;
    for e in &g.electrodes {
        if !e.path.is_empty() {
            let l = length_ft(&e.path, u) + 2.0 * makeup;
            *ft.entry(e.gec_size.trim().to_string()).or_default() += l;
            if e.in_conduit {
                in_conduit += length_ft(&e.path, u);
                conduit_runs += 1;
            }
        }
        if e.kind.eq_ignore_ascii_case("concrete_encased") && !e.material.eq_ignore_ascii_case("rebar") {
            *ft.entry(format!("{} bare", e.conductor.trim())).or_default() += e.length_ft;
        }
        if e.kind.eq_ignore_ascii_case("ground_ring") {
            *ft.entry(format!("{} bare", e.conductor.trim())).or_default() += e.ring_length_ft;
        }
    }
    for b in &g.bonds {
        if !b.path.is_empty() {
            *ft.entry(b.size.trim().to_string()).or_default() += length_ft(&b.path, u) + 2.0 * makeup;
            if b.in_conduit {
                in_conduit += length_ft(&b.path, u);
                conduit_runs += 1;
            }
        }
    }
    for (size, l) in &ft {
        let size = if size.is_empty() { "(size not drawn)" } else { size.as_str() };
        put(format!("Grounding conductor, {size} copper"), "ft", *l, format!("routed, risers included, {makeup} ft makeup a end"));
    }
    // Clamps at the piping.
    let water = g.electrodes.iter().filter(|e| e.kind.eq_ignore_ascii_case("water_pipe")).count()
        + g.bonds.iter().filter(|b| b.kind.eq_ignore_ascii_case("water")).count();
    let gas = g.bonds.iter().filter(|b| b.kind.eq_ignore_ascii_case("gas")).count();
    let other_pipe = g.bonds.iter().filter(|b| b.kind.eq_ignore_ascii_case("metal_piping")).count();
    put("Water pipe ground clamp".into(), "ea", water as f64, String::new());
    put("Gas pipe bonding clamp".into(), "ea", gas as f64, String::new());
    put("Pipe bonding clamp".into(), "ea", other_pipe as f64, String::new());
    // Rods, ring, Ufer, steel: their connections.
    let alt_note = |e: &Electrode| if e.alternative_of.is_empty() { String::new() } else { format!("alternative to missed {}", e.alternative_of) };
    let mut joints: BTreeMap<&'static str, (f64, String)> = BTreeMap::new();
    for e in &g.electrodes {
        match e.kind.to_ascii_lowercase().as_str() {
            "ground_rod" => {
                let n = f64::from(e.count.clamp(1, 1000));
                put(format!("Ground rod, 5/8\" x {}' copper-clad", e.rod_length_ft), "ea", n, alt_note(e));
                if test_wells {
                    put("Ground test well".into(), "ea", n, alt_note(e));
                }
                let j = joints.entry("cable to rod").or_default();
                j.0 += n;
            }
            "ground_ring" => {
                let j = joints.entry("cable to cable").or_default();
                j.0 += 2.0;
                if test_wells {
                    put("Ground test well".into(), "ea", 1.0, alt_note(e));
                }
            }
            "concrete_encased" => {
                joints.entry("cable to cable").or_default().0 += 1.0;
            }
            "building_steel" => {
                joints.entry("cable to steel").or_default().0 += 1.0;
            }
            _ => {}
        }
    }
    for b in &g.bonds {
        if b.kind.eq_ignore_ascii_case("structural_steel") {
            joints.entry("cable to steel").or_default().0 += 1.0;
        }
    }
    let total: f64 = joints.values().map(|v| v.0).sum();
    match connection.as_str() {
        "compression" => {
            for (k, (n, _)) in &joints {
                put(format!("Irreversible compression connector, {k}"), "ea", *n, String::new());
            }
            put("Compression crimper with dies (tool)".into(), "ea", if total > 0.0 { 1.0 } else { 0.0 }, String::new());
        }
        "mechanical" => {
            for (k, (n, _)) in &joints {
                put(format!("Mechanical ground clamp, {k}"), "ea", *n, String::new());
            }
        }
        _ => {
            for (k, (n, _)) in &joints {
                put(format!("Exothermic weld, {k}"), "ea", *n, String::new());
                put(format!("Exothermic weld mould, {k} (tool)"), "ea", 1.0, String::new());
            }
            put("Weld metal cartridge".into(), "ea", total, String::new());
            put("Mould handle clamp and flint igniter (tool)".into(), "ea", if total > 0.0 { 1.0 } else { 0.0 }, String::new());
        }
    }
    // Busbars, and the lugs landing on them.
    let mut bars: BTreeMap<String, f64> = BTreeMap::new();
    for b in &g.busbars {
        *bars.entry(format!("Grounding busbar ({}), {}", b.kind.to_uppercase(), b.size.trim())).or_default() += 1.0;
    }
    for (k, n) in bars {
        put(k, "ea", n, String::new());
    }
    let lugs = g
        .bonds
        .iter()
        .map(|b| [&b.from, &b.to].iter().filter(|t| g.busbars.iter().any(|x| x.tag.trim().eq_ignore_ascii_case(t.trim()))).count())
        .sum::<usize>();
    put("Two-hole compression lug on busbar".into(), "ea", lugs as f64, String::new());
    // Jumpers, bushings and GEC conduit.
    let jumpers = g
        .bonds
        .iter()
        .filter(|b| matches!(b.kind.to_ascii_lowercase().as_str(), "main_bonding_jumper" | "system_bonding_jumper" | "separately_derived"))
        .count();
    put("Bonding jumper connection (bonding jumper lugs)".into(), "ea", jumpers as f64 * 2.0, String::new());
    put("Bonding bushing (GEC in metal raceway)".into(), "ea", f64::from(conduit_runs) * 2.0, String::new());
    put("Conduit for grounding conductors".into(), "ft", in_conduit, String::new());
    let _ = poes;
    items
}

fn run_takeoff(s: &mut Session, _p: &Value) -> Result<Value> {
    let sp = spec("grounding.takeoff", s)?;
    grounding("grounding.takeoff", &sp)?;
    Ok(json!({"items": takeoff(&sp, s.doc()?)}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("grounding.entries", "Water and Gas Points of Entry", run_entries)
            .params("{} → {entries: [{tag, kind: water|gas, level, at, z_ft, accessible, why}]} — given in mep.grounding.entries, else found from the cold water and gas runs")
            .noundo(),
        CommandSpec::new("grounding.route", "Route Grounding", run_route).params("{all?: bool (route again what has a route)} → {routed: [{tag, length_ft}], failed: [tag]}"),
        CommandSpec::new("grounding.check", "Check Grounding", run_check)
            .params("{review?: bool (each error an RFI)} → {ok, errors, findings: [{about, check, severity, message, fix}], rfis}"),
        CommandSpec::new("grounding.alternative", "Alternative Grounding Electrode", run_alternative)
            .params("{electrode: the missed one's tag, kind?: rods (default: two 10 ft rods) | ring, ring_length_ft?} → {tag, kind, alternative_of}"),
        CommandSpec::new("grounding.takeoff", "Grounding Takeoff", run_takeoff).params("{} → {items: [{item, unit, qty, note}]}").noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_go_up_across_and_down_and_measure_their_risers() {
        let sp = BuildingSpec::from_json(r#"{"units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}, {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9}]}"#).unwrap();
        let p = route(&sp, ("L1", [0.0, 0.0], 0.5), ("L2", [120.0, 240.0], 19.0));
        assert_eq!(p, vec![[0.0, 0.0, 0.5], [0.0, 0.0, 9.5], [120.0, 0.0, 9.5], [120.0, 240.0, 9.5], [120.0, 240.0, 21.5], [120.0, 240.0, 19.0]]);
        assert!((length_ft(&p, 12.0) - (9.0 + 10.0 + 20.0 + 12.0 + 2.5)).abs() < 1e-9);
        assert!(point_in(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], [5.0, 5.0]));
        assert!(!point_in(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], [15.0, 5.0]));
    }
}
