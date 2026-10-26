//! The outside of the building, read from its drawings into the building spec's `exterior`, and
//! built by `model.build`:
//!
//! - `elev.read`: an exterior elevation (a region of the sheet): its level lines and their labels
//!   (`LEVEL 2  EL. 112'-0"`, `T.O. ROOF`, `T.O. PARAPET`), giving the levels' elevations, the
//!   roof and the parapet height; window outlines, giving the sill and head heights; storefront
//!   and curtain-wall bands (glass from the floor, or rectangles divided by mullions); material
//!   hatches with their keynotes (`BRICK`, `METAL PANEL`). The facade is the one the title names
//!   (`NORTH ELEVATION`) unless given.
//! - `roof.read`: a roof plan (a region): roof areas (outermost closed outlines), each slope from
//!   its arrow (pointing downhill) and pitch note (`1/4" / FT`, `4:12`, `2%`), parapets (a
//!   `PARAPET` note, or an inner outline a parapet's thickness inside), crickets (outlines with a
//!   `CRICKET` note) and rooftop equipment (outlines named RTU, AHU, EF, MAU, CU…). With no
//!   closed outline (a PDF's roof edge comes as loose lines) it reads the plan's lines instead,
//!   as the build does.
//!
//! `model.build` reads the roof of a roof level the spec has no roof for off that level's plan
//! (the face of the wall pen's lines its drains, slopes and parapet notes lie in, a parapet ring
//! round it, curbs and units by the notes beside them, drains at their `RD` notes), else puts a
//! flat roof over the walls of the storey under it.
//!
//! `apply: true` writes what was read into the spec. The build puts roofs on their level (sloped
//! slabs over their outlines, parapets round them, crickets as hips rising to their apex,
//! equipment as boxes on the roof), and the facades' glass bands as panels on the building's
//! outside faces, with the material bands named on the walls of that side.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::{Bounds2, Polyline, Vec2, Vec3};
use cadaddy_walk3d::levels::DatumKind;
use cadaddy_walk3d::{
    Band, BandKind, BuildingSpec, Cricket, Equipment, Exterior, Facade, LevelParapet, Role, Roof, RoofDrain, RoofSlope, Side, ValueStatus,
};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{feet_inches, texts};
use super::*;
use crate::{Result, Session};

/// Objects looked at in a region, at most (hostile drawings).
const MAX_OBJECTS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("elev.read", "Read Elevation", run_elev).params(
            "{region: [x0,y0,x1,y1] (the elevation on the sheet), facade?: north|south|east|west (else from its title), apply?: bool (into the spec: level elevations and heights, window sill and head, the facade's bands, the roof level's parapet height)} → {facade, levels: [{label, name, y, elevation_ft, matched?}], roof_ft?, parapet_ft?, parapet?: {level, height_ft, applied, roofs (given it), kept (roofs keeping their roof-plan note)}, windows: {count, sill_ft, head_ft}, bands: [...], assumed: [{item, value, assumed, why}], conflicts: [...], applied}",
        ),
        CommandSpec::new("roof.read", "Read Roof Plan", run_roof).params(
            "{region: [x0,y0,x1,y1] (the roof plan on the sheet), level?: the roof level's name (default: one named ROOF), apply?: bool} → {roofs: [{name, outline, slope?: {down, rise_per_ft, direction_status?}, parapet_ft?, parapet_status?, parapet_from?}], crickets, equipment, drains, assumed: [{item, value, assumed, why}] (each default put in), conflicts: [...] (roof-plan parapet notes the elevation disagrees with), applied}",
        ),
    ]
}

/// The objects of a region of model space (their centres inside it), the plan alone.
fn in_region<'a>(d: &'a Drawing, p: &Value, cmd: &str) -> Result<(Bounds2, Vec<&'a Arc<Entity>>)> {
    let r: Vec<f64> = p.get("region").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let [x0, y0, x1, y1] = r[..] else { return Err(bad(cmd, "`region` must be [x0, y0, x1, y1]")) };
    if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) || (x1 - x0).abs() < 1e-9 || (y1 - y0).abs() < 1e-9 {
        return Err(bad(cmd, "`region` must be a rectangle with an area"));
    }
    let b = Bounds2::new(Vec2::new(x0.min(x1), y0.min(y1)), Vec2::new(x0.max(x1), y0.max(y1)));
    Ok((b, objects_in(d, &b)))
}

/// The plan's objects whose centres lie in `b`.
fn objects_in<'a>(d: &'a Drawing, b: &Bounds2) -> Vec<&'a Arc<Entity>> {
    plan_objects(d)
        .into_iter()
        .filter(|e| {
            let eb = cadaddy_doc::entity_bounds(d, e, 0);
            !eb.is_empty() && b.contains(eb.center())
        })
        .take(MAX_OBJECTS)
        .collect()
}

/// Whether an outline is the sheet's border (or its title block's frame): it spans most of the
/// region both ways.
fn sheet_border(q: &[Vec2], region: &Bounds2) -> bool {
    let b = Bounds2::from_points(q.iter().copied());
    !b.is_empty() && b.width() >= 0.85 * region.width() && b.height() >= 0.85 * region.height()
}

fn units(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// The straight segments of the objects (line, polyline pieces).
fn segments(ents: &[&Arc<Entity>]) -> Vec<(Vec2, Vec2, Handle)> {
    let mut out = Vec::new();
    for e in ents {
        for sg in super::perceive::geometry::segments(&[*e]) {
            out.push((sg.a, sg.b, e.handle));
        }
    }
    out
}

/// Closed outlines: closed polylines (and polylines whose ends meet).
fn outlines(ents: &[&Arc<Entity>]) -> Vec<(Vec<Vec2>, Handle)> {
    ents.iter()
        .filter_map(|e| match &e.kind {
            EntityKind::LwPolyline(p) if p.vertices.len() >= 3 => {
                let closes = p.closed || p.vertices.first().zip(p.vertices.last()).is_some_and(|(a, b)| a.p.dist(b.p) < 1e-9);
                if !closes {
                    return None;
                }
                let mut q = Polyline { vertices: p.vertices.clone(), closed: true }.tessellate(1e-3);
                if q.len() > 1 && q.first().zip(q.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-9) {
                    q.pop();
                }
                (q.len() >= 3).then_some((q, e.handle))
            }
            _ => None,
        })
        .collect()
}

// ---------------- notes ----------------

/// A number at the start of `t` or after a space: `1/4`, `1 1/2`, `0.25`, `4`.
fn first_number(t: &str) -> Option<f64> {
    let words: Vec<&str> = t.split(|c: char| c.is_whitespace() || c == '"' || c == '\'').filter(|w| !w.is_empty()).collect();
    for (i, w) in words.iter().enumerate() {
        let frac = |w: &str| w.split_once('/').and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok().filter(|b| *b != 0.0)?));
        if let Some(f) = frac(w) {
            return Some(f);
        }
        if let Ok(n) = w.parse::<f64>() {
            // `1 1/2`
            return Some(n + words.get(i + 1).and_then(|x| frac(x)).filter(|f| *f < 1.0).unwrap_or(0.0));
        }
    }
    None
}

/// A roof pitch as rise per foot: `4:12`, `1/4" / FT`, `1/4":12"`, `1/4 IN/FT`, `2%`.
pub(crate) fn pitch(t: &str) -> Option<f64> {
    let up = t.to_ascii_uppercase().replace(' ', "");
    if let Some((a, _)) = up.split_once(":12")
        && !a.ends_with('"')
    {
        let n: String = a.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
        return n.parse::<f64>().ok().map(|n| n / 12.0).filter(|v| (0.0..=3.0).contains(v));
    }
    if let Some(i) = up.find('%') {
        let n: String = up[..i].chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
        return n.parse::<f64>().ok().map(|n| n / 100.0).filter(|v| (0.0..=3.0).contains(v));
    }
    let per_foot = up.contains("/FT") || up.contains("PERFT") || up.contains("/1'-0") || up.contains("\":12\"") || up.contains("IN/FT");
    if per_foot {
        return first_number(&t.to_ascii_uppercase()).map(|i| i / 12.0).filter(|v| (0.0..=3.0).contains(v));
    }
    None
}

/// A facade side named in a title.
fn side_in(t: &str) -> Option<Side> {
    let up = t.to_ascii_uppercase();
    if !up.contains("ELEVATION") {
        return None;
    }
    [("NORTH", Side::North), ("SOUTH", Side::South), ("EAST", Side::East), ("WEST", Side::West)]
        .into_iter()
        .find(|(w, _)| up.contains(w))
        .map(|(_, s)| s)
}

fn parse_side(t: &str) -> Option<Side> {
    match t.to_ascii_lowercase().as_str() {
        "north" | "n" => Some(Side::North),
        "south" | "s" => Some(Side::South),
        "east" | "e" => Some(Side::East),
        "west" | "w" => Some(Side::West),
        _ => None,
    }
}

/// A level label's name: its words without the elevation (`LEVEL 2 EL. 112'-0"` → `LEVEL 2`).
fn label_name(t: &str) -> String {
    t.split_whitespace()
        .filter(|w| {
            let up = w.to_ascii_uppercase();
            !(w.contains('\'') || w.contains('"') || matches!(up.as_str(), "EL" | "EL." | "ELEV" | "ELEV." | "@" | "=" | "+/-" | "±"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a label names a spec level: the same letters and digits, or the same number with
/// the same first letter (`LEVEL 2` = `Level 2` = `L2`).
fn names_level(label: &str, level: &str) -> bool {
    let norm = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
    let (a, b) = (norm(label), norm(level));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    a == b || (!digits(&a).is_empty() && digits(&a) == digits(&b) && a.chars().next() == b.chars().next())
}

/// Where a roof's parapet height came from, as stored.
const FROM_ROOF_PLAN: &str = "roof plan";
const FROM_ELEVATION: &str = "elevation";
const FROM_DEFAULT: &str = "default";

/// The parapet height a roof plan gives with no height note and no elevation, feet.
const DEFAULT_PARAPET_FT: f64 = 3.0;
const DEFAULT_CRICKET_FT: f64 = 0.5;
const DEFAULT_EQUIPMENT_FT: f64 = 4.0;

/// One value put in because the drawings don't give it.
fn assumed(item: &str, value: &str, v: Value, why: &str) -> Value {
    json!({ "item": item, "value": value, "assumed": v, "why": why })
}

/// The level an elevation's parapet belongs to: the one named ROOF, else the highest.
fn roof_level(sp: &BuildingSpec) -> Option<String> {
    sp.levels
        .iter()
        .find(|l| l.name.to_ascii_uppercase().contains("ROOF"))
        .or_else(|| sp.levels.iter().max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)))
        .map(|l| l.name.clone())
}

/// Gives a roof an elevation's parapet height unless the roof has its own documented one (a
/// roof-plan note, or a value put in by hand). The documented one stays; a disagreement comes
/// back as a conflict. Whether the roof took the elevation's value.
fn settle_parapet(r: &mut Roof, elevation_ft: f64, conflicts: &mut Vec<Value>) -> bool {
    let documented = r.parapet_ft.is_some() && !ValueStatus::is_default(&r.parapet_status) && r.parapet_from.as_deref() != Some(FROM_ELEVATION);
    if documented {
        if let Some(own) = r.parapet_ft.filter(|own| (own - elevation_ft).abs() > 1e-6) {
            conflicts.push(json!({
                "roof": r.name,
                "level": r.level,
                "roof_plan_ft": own,
                "elevation_ft": elevation_ft,
                "kept_ft": own,
                "message": format!("{}: the roof plan gives a {} parapet, the elevation {}; the roof plan's is kept", r.name, cadaddy_walk3d::feet(own), cadaddy_walk3d::feet(elevation_ft)),
            }));
        }
        return false;
    }
    r.parapet_ft = Some(elevation_ft);
    r.parapet_status = Some(ValueStatus::Read);
    r.parapet_from = Some(FROM_ELEVATION.into());
    true
}

// ---------------- elev.read ----------------

struct LevelLine {
    y: f64,
    label: String,
    elevation: Option<f64>,
}

fn run_elev(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "elev.read")?;
    let upf = units(&sp);
    let d = s.doc()?;
    let (region, ents) = in_region(d, p, "elev.read")?;
    let notes = texts(&ents);
    let side = match str_param(p, "facade") {
        Some(f) => Some(parse_side(f).ok_or_else(|| bad("elev.read", "`facade` must be north, south, east or west"))?),
        None => notes.iter().find_map(|(_, t, _, _)| side_in(t)),
    };
    let segs = segments(&ents);
    // Level lines: long horizontal LINEs (outlines are polylines), one per height.
    let mut lines: Vec<(f64, f64, f64)> = Vec::new();
    let plain: Vec<(Vec2, Vec2)> =
        ents.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some((l.a.xy(), l.b.xy())) } else { None }).collect();
    for (a, b) in &plain {
        let len = a.dist(*b);
        if (a.y - b.y).abs() <= 1e-6 * len.max(1.0) && len >= 0.5 * region.width() {
            let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
            match lines.iter_mut().find(|l| (l.0 - a.y).abs() < 1e-6 * len.max(1.0)) {
                Some(l) => {
                    l.1 = l.1.min(x0);
                    l.2 = l.2.max(x1);
                }
                None => lines.push((a.y, x0, x1)),
            }
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Their labels: the texts beside an end, on or just above or below the line.
    let mut levels: Vec<LevelLine> = Vec::new();
    for (y, x0, x1) in &lines {
        let mut near: Vec<&(Handle, String, Vec2, f64)> = notes
            .iter()
            .filter(|(_, _, at, h)| (at.y - y).abs() <= 2.5 * h.max(1e-9) && (at.x <= x0 + 0.05 * (x1 - x0) || at.x >= x1 - 0.05 * (x1 - x0)))
            .collect();
        near.sort_by(|a, b| b.2.y.total_cmp(&a.2.y));
        let label = near.iter().map(|n| n.1.as_str()).collect::<Vec<_>>().join(" ");
        let elevation = near.iter().find_map(|n| feet_inches(&n.1.to_ascii_uppercase())).map(|i| i / 12.0);
        levels.push(LevelLine { y: *y, label, elevation });
    }
    let has = |l: &LevelLine, w: &str| l.label.to_ascii_uppercase().contains(w);
    let floors: Vec<&LevelLine> = levels.iter().filter(|l| !has(l, "GRADE") && !has(l, "PARAPET") && !has(l, "ROOF")).collect();
    let Some(ground) = floors.first().copied() else {
        return Err(bad("elev.read", "no level lines found in the region (long horizontal lines with labels)"));
    };
    // Heights above the ground floor: from the labels' elevations when they have them.
    let rel = |l: &LevelLine| match (l.elevation, ground.elevation) {
        (Some(e), Some(g)) => e - g,
        _ => (l.y - ground.y) / upf,
    };
    let ground_level = sp.levels.iter().find(|l| names_level(&label_name(&ground.label), &l.name));
    let base_ft = ground_level.map_or(0.0, |l| l.elevation_ft);
    let mut assumptions = Vec::new();
    if ground_level.is_none() {
        assumptions.push(assumed(
            &label_name(&ground.label),
            "elevation_ft",
            json!(0.0),
            "the ground line's label names no level of the spec, so heights are taken from 0'-0\"",
        ));
    }
    let roof = levels.iter().find(|l| has(l, "ROOF")).map(rel);
    let parapet = levels.iter().find(|l| has(l, "PARAPET")).map(rel).zip(roof).map(|(p, r)| p - r).filter(|p| *p > 0.0);
    let mut out_levels = Vec::new();
    for l in &levels {
        let name = label_name(&l.label);
        let matched = sp.levels.iter().find(|x| names_level(&name, &x.name)).map(|x| x.name.clone());
        out_levels.push(json!({ "label": l.label, "name": name, "y": l.y, "elevation_ft": base_ft + rel(l), "matched": matched }));
    }
    // Windows and glass bands: rectangles above the ground line.
    let (mut sills, mut heads) = (Vec::new(), Vec::new());
    let mut bands: Vec<Band> = Vec::new();
    let level_ys: Vec<f64> = floors.iter().map(|l| l.y).collect();
    let rects: Vec<(Bounds2, Handle)> = outlines(&ents)
        .into_iter()
        .filter(|(q, _)| q.len() == 4)
        .map(|(q, h)| (Bounds2::from_points(q.iter().copied()), h))
        .filter(|(b, _)| b.min.y >= ground.y - 1e-6 && b.height() >= upf && b.width() <= 60.0 * upf)
        .collect();
    // The building's ends in the elevation: its tall vertical edges (else the level lines).
    let tall: Vec<f64> = segs.iter().filter(|(a, b, _)| (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() >= 4.0 * upf).map(|(a, _, _)| a.x).collect();
    let left = tall.iter().copied().fold(f64::INFINITY, f64::min);
    let left = if left.is_finite() { left } else { lines.first().map_or(region.min.x, |l| l.1) };
    let hatched: Vec<Bounds2> = ents.iter().filter(|e| matches!(e.kind, EntityKind::Hatch(_))).map(|e| cadaddy_doc::entity_bounds(d, e, 0)).collect();
    for (b, _) in &rects {
        // A building silhouette (holding other outlines) or a hatch's boundary is not an opening.
        if rects.iter().any(|(o, _)| o != b && b.contains_box(o)) || hatched.iter().any(|h| h.min.dist(b.min) < 1e-6 && h.max.dist(b.max) < 1e-6) {
            continue;
        }
        let storey = level_ys.iter().copied().filter(|y| *y <= b.min.y + 1e-6).fold(f64::NEG_INFINITY, f64::max);
        if !storey.is_finite() {
            continue;
        }
        let mullions = segs
            .iter()
            .filter(|(a, c, _)| (a.x - c.x).abs() < 1e-6 && a.x > b.min.x + 1e-6 && a.x < b.max.x - 1e-6 && (a.y - c.y).abs() >= 0.8 * b.height())
            .count();
        let sill = (b.min.y - storey) / upf;
        let head = (b.max.y - storey) / upf;
        if sill < 0.5 || mullions >= 2 {
            bands.push(Band {
                kind: BandKind::Glass,
                from_ft: (b.min.y - ground.y) / upf,
                to_ft: (b.max.y - ground.y) / upf,
                start_ft: Some((b.min.x - left) / upf),
                end_ft: Some((b.max.x - left) / upf),
                material: None,
                assembly: None,
            });
        } else {
            sills.push(sill);
            heads.push(head);
        }
    }
    // Material bands: hatches, named by the note inside them (else their pattern).
    for e in &ents {
        let EntityKind::Hatch(h) = &e.kind else { continue };
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        let note = notes.iter().find(|(_, t, at, _)| b.contains(*at) && t.chars().any(char::is_alphabetic)).map(|(_, t, _, _)| t.clone());
        let material = note.or_else(|| (!h.pattern.is_empty()).then(|| h.pattern.clone()));
        bands.push(Band {
            kind: BandKind::Material,
            from_ft: ((b.min.y - ground.y) / upf).max(0.0),
            to_ft: (b.max.y - ground.y) / upf,
            start_ft: Some((b.min.x - left) / upf),
            end_ft: Some((b.max.x - left) / upf),
            material,
            assembly: None,
        });
    }
    let round = |v: f64| (v * 96.0).round() / 96.0;
    for b in &mut bands {
        b.from_ft = round(b.from_ft);
        b.to_ft = round(b.to_ft);
        b.start_ft = b.start_ft.map(round);
        b.end_ft = b.end_ft.map(round);
    }
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().map(round)
    };
    let (sill, head) = (median(sills.clone()), median(heads));
    let apply = bool_or(p, "apply", false);
    if apply {
        // Levels: elevations from the lines, floor to floor to the next one (or the roof).
        let mut tops: Vec<(String, f64)> = Vec::new();
        for l in &floors {
            if let Some(x) = sp.levels.iter().find(|x| names_level(&label_name(&l.label), &x.name)) {
                tops.push((x.name.clone(), base_ft + rel(l)));
            }
        }
        let roof_abs = roof.map(|r| base_ft + r);
        // The roof level: one named ROOF takes the roof line's elevation.
        if let (Some(r), Some(x)) = (roof_abs, sp.levels.iter_mut().find(|x| x.name.to_ascii_uppercase().contains("ROOF"))) {
            x.elevation_ft = round(r);
        }
        for (i, (name, e)) in tops.iter().enumerate() {
            let next = tops.get(i + 1).map(|t| t.1).or(roof_abs);
            if let Some(x) = sp.levels.iter_mut().find(|x| &x.name == name) {
                x.elevation_ft = round(*e);
                if let Some(n) = next.filter(|n| *n > *e) {
                    x.height_ft = round(n - e);
                }
            }
        }
        if let (Some(si), Some(he)) = (sill, head) {
            sp.openings.window_sill_ft = si;
            sp.openings.window_head_ft = he;
        }
        if let Some(side) = side {
            sp.exterior.facades.retain(|f| f.side != side);
            sp.exterior.facades.push(Facade { side, bands: bands.clone() });
        }
    }
    // The parapet: kept by the roof level, so a roof plan read later finds it too, and given to
    // the level's roofs now unless they have a documented height of their own.
    let mut conflicts = Vec::new();
    let parapet_out = match (parapet.map(round), roof_level(&sp)) {
        (None, _) => Value::Null,
        (Some(pp), None) => json!({ "height_ft": pp, "applied": false, "why": "the spec has no levels to put it on" }),
        (Some(pp), Some(level)) => {
            let (mut given, mut kept) = (Vec::new(), Vec::new());
            let mut roofs = sp.exterior.roofs.clone();
            for r in roofs.iter_mut().filter(|r| r.level.eq_ignore_ascii_case(&level)) {
                if settle_parapet(r, pp, &mut conflicts) { given.push(r.name.clone()) } else { kept.push(r.name.clone()) }
            }
            if apply {
                sp.exterior.roofs = roofs;
                sp.exterior.parapets.retain(|x| !x.level.eq_ignore_ascii_case(&level));
                sp.exterior.parapets.push(LevelParapet { level: level.clone(), height_ft: pp, facade: side });
            }
            json!({ "level": level, "height_ft": pp, "applied": apply, "roofs": given, "kept": kept })
        }
    };
    if apply {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({
        "facade": side.map(|s| s.name()),
        "levels": out_levels,
        "roof_ft": roof.map(|r| round(base_ft + r)),
        "parapet_ft": parapet.map(round),
        "parapet": parapet_out,
        "windows": { "count": sills.len(), "sill_ft": sill, "head_ft": head },
        "bands": serde_json::to_value(&bands).unwrap_or(Value::Null),
        "assumed": assumptions,
        "conflicts": conflicts,
        "applied": apply,
    }))
}

// ---------------- roof.read ----------------

fn run_roof(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "roof.read")?;
    let upf = units(&sp);
    let level = match str_param(p, "level") {
        Some(n) => sp.level(n).map(|l| l.name.clone()).ok_or_else(|| bad("roof.read", format!("there is no level named {n}")))?,
        None => sp
            .levels
            .iter()
            .find(|l| l.name.to_ascii_uppercase().contains("ROOF"))
            .map(|l| l.name.clone())
            .ok_or_else(|| bad("roof.read", "give `level` (the roof level); none is named ROOF"))?,
    };
    let d = s.doc()?;
    let (region, ents) = in_region(d, p, "roof.read")?;
    let notes = texts(&ents);
    let segs = segments(&ents);
    let all = outlines(&ents);
    let area = |q: &[Vec2]| cadaddy_geom::shoelace(q).abs();
    let inside = |inner: &[Vec2], outer: &[Vec2]| inner.iter().all(|p| cadaddy_geom::point_in_polygon(outer, *p));
    let up = |t: &str| t.to_ascii_uppercase();
    let word = has_word;
    let named = |q: &[Vec2], ws: &[&str]| {
        notes.iter().find(|(_, t, at, _)| cadaddy_geom::point_in_polygon(q, *at) && word(t, ws)).map(|(_, t, _, _)| t.clone())
    };
    let equipment_words = EQUIPMENT_WORDS;
    // Rooftop equipment and crickets: each note names the smallest outline holding it.
    let mut equipment = Vec::new();
    let mut crickets = Vec::new();
    let mut taken: Vec<usize> = Vec::new();
    let mut assumptions = Vec::new();
    for (_, t, at, _) in &notes {
        let cricket = word(t, &["CRICKET"]);
        if !cricket && !word(t, &equipment_words) {
            continue;
        }
        let Some(i) = (0..all.len())
            .filter(|i| all.get(*i).is_some_and(|(q, _)| cadaddy_geom::point_in_polygon(q, *at)))
            .min_by(|a, b| area(&all[*a].0).total_cmp(&area(&all[*b].0)))
        else {
            continue;
        };
        let Some((q, _)) = all.get(i) else { continue };
        if taken.contains(&i) || area(q) >= 2_000.0 * upf * upf {
            continue;
        }
        let outline: Vec<[f64; 2]> = q.iter().map(|p| [p.x, p.y]).collect();
        let height = feet_inches(&up(t)).map(|i| i / 12.0);
        let status = Some(if height.is_some() { ValueStatus::Read } else { ValueStatus::Default });
        if cricket {
            let h = height.unwrap_or(DEFAULT_CRICKET_FT);
            let item = format!("Cricket {}", crickets.len() + 1);
            if height.is_none() {
                assumptions.push(assumed(&item, "height_ft", json!(h), "its note gives no height; 6\" assumed"));
            }
            crickets.push(Cricket { level: level.clone(), outline, height_ft: h, height_status: status });
        } else {
            let h = height.unwrap_or(DEFAULT_EQUIPMENT_FT);
            let name = t.split_whitespace().next().unwrap_or("EQUIP").to_string();
            if height.is_none() {
                assumptions.push(assumed(&name, "height_ft", json!(h), "its note gives no height (H=); 4'-0\" assumed"));
            }
            equipment.push(Equipment { name, level: level.clone(), outline, height_ft: h, height_status: status });
        }
        taken.push(i);
    }
    // Roof areas: the outermost remaining outlines of 100 sq ft or more.
    let candidates: Vec<usize> = (0..all.len())
        .filter(|i| !taken.contains(i) && all.get(*i).is_some_and(|(q, _)| area(q) >= 100.0 * upf * upf && !sheet_border(q, &region)))
        .collect();
    let mut roofs = Vec::new();
    let mut conflicts = Vec::new();
    let elevation_parapet = sp.exterior.level_parapet(&level);
    for &i in &candidates {
        let Some((q, _)) = all.get(i) else { continue };
        let outer = candidates.iter().any(|&j| j != i && all.get(j).is_some_and(|(o, _)| area(o) > area(q) && inside(q, o)));
        if outer {
            continue;
        }
        // A parapet: a note, or an inner outline a parapet's thickness (4"–24") inside.
        let inner = candidates.iter().filter_map(|&j| all.get(j)).find(|(o, _)| {
            area(o) < area(q) && inside(o, q) && {
                let gap = o
                    .iter()
                    .map(|p| q.iter().zip(q.iter().cycle().skip(1)).map(|(a, b)| seg_dist(*p, *a, *b)).fold(f64::INFINITY, f64::min))
                    .fold(f64::INFINITY, f64::min);
                gap >= 4.0 / 12.0 * upf && gap <= 2.0 * upf
            }
        });
        let parapet_note = named(q, &["PARAPET"]);
        let noted = parapet_note.as_deref().and_then(|t| feet_inches(&up(t))).map(|i| i / 12.0);
        let name = format!("Roof {}", roofs.len() + 1);
        // A height noted on the roof plan; else the elevation's; else a default when the plan
        // draws one (a note without a height, or the inner line).
        let (parapet, parapet_status, parapet_from) = match (noted, parapet_note.is_some() || inner.is_some()) {
            (Some(h), _) => (Some(h), Some(ValueStatus::Read), Some(FROM_ROOF_PLAN.to_string())),
            (None, drawn) if drawn || elevation_parapet.is_some() => match elevation_parapet {
                Some(h) => (Some(h), Some(ValueStatus::Read), Some(FROM_ELEVATION.to_string())),
                None => {
                    assumptions.push(assumed(
                        &name,
                        "parapet_ft",
                        json!(DEFAULT_PARAPET_FT),
                        "the roof plan draws a parapet with no height note and no elevation gives one; 3'-0\" assumed",
                    ));
                    (Some(DEFAULT_PARAPET_FT), Some(ValueStatus::Default), Some(FROM_DEFAULT.to_string()))
                }
            },
            _ => (None, None, None),
        };
        // Its slope: the pitch note inside, and the arrow nearest it (downhill to its head).
        let note = notes.iter().find(|(_, t, at, _)| cadaddy_geom::point_in_polygon(q, *at) && pitch(t).is_some());
        let slope = note.and_then(|(_, t, at, _)| {
            let rise = pitch(t)?;
            let arrow = segs
                .iter()
                .filter(|(a, b, _)| a.dist(*b) >= 3.0 * upf && cadaddy_geom::point_in_polygon(q, (*a + *b) * 0.5))
                .filter(|(a, b, _)| {
                    !q.iter().zip(q.iter().cycle().skip(1)).any(|(e0, e1)| seg_dist(*a, *e0, *e1) < 1e-6 && seg_dist(*b, *e0, *e1) < 1e-6)
                })
                .min_by(|x, y| seg_dist(*at, x.0, x.1).total_cmp(&seg_dist(*at, y.0, y.1)))?;
            let (a, b) = (arrow.0, arrow.1);
            // The head: the end other short strokes meet.
            let strokes = |p: Vec2| segs.iter().filter(|(c, e, _)| c.dist(*e) < a.dist(b) * 0.3 && (c.dist(p) < 1e-6 || e.dist(p) < 1e-6)).count();
            let (sa, sb) = (strokes(a), strokes(b));
            let (tail, head) = if sa > sb { (b, a) } else { (a, b) };
            let dir = head - tail;
            let len = dir.len().max(1e-12);
            let down = [dir.x / len, dir.y / len];
            // No head to tell the ends apart: downhill to the end drawn last.
            let direction_status = Some(if sa == sb {
                assumptions.push(assumed(
                    &name,
                    "slope.down",
                    json!(down),
                    "the slope arrow's head can't be told from its tail; downhill taken to the end drawn last",
                ));
                ValueStatus::Default
            } else {
                ValueStatus::Read
            });
            Some(RoofSlope { down, rise_per_ft: rise, direction_status })
        });
        let mut roof = Roof {
            name,
            level: level.clone(),
            outline: q.iter().map(|p| [p.x, p.y]).collect(),
            slope,
            parapet_ft: parapet,
            parapet_status,
            parapet_from,
            assembly: None,
        };
        if let Some(h) = elevation_parapet {
            settle_parapet(&mut roof, h, &mut conflicts);
        }
        roofs.push(roof);
    }
    let mut drains = Vec::new();
    if roofs.is_empty() {
        // No closed outline: the roof plan's lines (a PDF's edges come as loose lines).
        let read = read_plan(d, &sp, &region, &level)
            .ok_or_else(|| bad("roof.read", "no roof outline found in the region (a closed outline of 100 sq ft or more)"))?;
        roofs = read.roofs;
        crickets.extend(read.crickets);
        equipment.extend(read.equipment);
        drains = read.drains;
    }
    let apply = bool_or(p, "apply", false);
    let result = json!({
        "level": level,
        "roofs": serde_json::to_value(&roofs).unwrap_or(Value::Null),
        "crickets": serde_json::to_value(&crickets).unwrap_or(Value::Null),
        "equipment": serde_json::to_value(&equipment).unwrap_or(Value::Null),
        "drains": serde_json::to_value(&drains).unwrap_or(Value::Null),
        "assumed": assumptions,
        "conflicts": conflicts,
        "applied": apply,
    });
    if apply {
        sp.exterior.roofs.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.crickets.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.equipment.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.drains.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.roofs.extend(roofs);
        sp.exterior.crickets.extend(crickets);
        sp.exterior.equipment.extend(equipment);
        sp.exterior.drains.extend(drains);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(result)
}

/// Whether `t` has one of the words `ws` (or one with a number after it: `RTU2`).
fn has_word(t: &str, ws: &[&str]) -> bool {
    let u = t.to_ascii_uppercase();
    u.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| ws.iter().any(|x| w == *x || (w.starts_with(x) && w.get(x.len()..).is_some_and(|r| r.chars().all(|c| c.is_ascii_digit())))))
}

/// Words naming what stands on a roof.
const EQUIPMENT_WORDS: [&str; 10] = ["RTU", "AHU", "EF", "MAU", "CU", "ACCU", "UNIT", "EQUIP", "EQUIPMENT", "HVAC"];

/// Curbs and openings on a roof that aren't units: their notes give a curb or a height.
const CURB_WORDS: [&str; 6] = ["HATCH", "OVERRUN", "SKYLIGHT", "CURB", "PENTHOUSE", "SMOKE"];

/// Notes a roof plan writes on its roof (the seeds for finding it; not its title's `ROOF`).
const ROOF_WORDS: [&str; 5] = ["RD", "DRAIN", "PARAPET", "CRICKET", "SCUPPER"];

/// Roof faces searched for at most (each is a boundary search over the plan's lines).
const MAX_SEARCHES: usize = 40;

/// Pieces on a roof plan, at most.
const MAX_ROOF_ITEMS: usize = 2_000;

/// What a roof plan's lines say: its roofs, crickets, curbs and units, and drains.
struct PlanRoof {
    roofs: Vec<Roof>,
    crickets: Vec<Cricket>,
    equipment: Vec<Equipment>,
    drains: Vec<RoofDrain>,
}

/// The roof read off a roof plan's lines in `region` (a PDF's roof edge, parapet and curbs come
/// as loose lines, not closed outlines): the face most of its notes lie in (drains, slopes, the
/// parapet, curbs), never the sheet's border; a parapet when a ring a parapet's thickness
/// (4"–24") wraps that face, or a note says so; the curbs inside it, named by the notes beside
/// them; drains at their `RD` notes. Arrows pointing different ways (tapered insulation to
/// drains in the field) leave the roof flat. None when no face of 100 sq ft or more holds them.
fn read_plan(d: &Drawing, sp: &BuildingSpec, region: &Bounds2, level: &str) -> Option<PlanRoof> {
    use cadaddy_geom::region::boundary_at;
    let upf = units(sp);
    let ents = objects_in(d, region);
    let notes = texts(&ents);
    let segs = segments(&ents);
    // The edge and curbs are drawn with the wall pen: the wall layers' lines when the spec names
    // any (not the grid, dimensions or lettering), else every line.
    let walled: Vec<Vec<Vec2>> = segs
        .iter()
        .filter(|(_, _, h)| d.entity(*h).is_some_and(|e| sp.role(&e.common.layer).is_some_and(|r| r.role == Role::Wall)))
        .map(|(a, b, _)| vec![*a, *b])
        .collect();
    let pen: Vec<Vec<Vec2>> = if walled.is_empty() { segs.iter().map(|(a, b, _)| vec![*a, *b]).collect() } else { walled };
    if pen.is_empty() {
        return None;
    }
    let tol = upf / 48.0;
    let area = |q: &[Vec2]| cadaddy_geom::shoelace(q).abs();
    let inside = |q: &[Vec2], p: Vec2| cadaddy_geom::point_in_polygon(q, p);
    let seeds: Vec<Vec2> = notes
        .iter()
        .filter(|(_, t, _, _)| pitch(t).is_some() || has_word(t, &ROOF_WORDS) || has_word(t, &EQUIPMENT_WORDS) || has_word(t, &CURB_WORDS))
        .map(|(_, _, at, _)| *at)
        .take(MAX_ROOF_ITEMS)
        .collect();
    // Each face found, and how many notes lie in it (on it, not on a curb inside it).
    let mut faces: Vec<(cadaddy_geom::region::Boundary, usize)> = Vec::new();
    let in_face = |b: &cadaddy_geom::region::Boundary, p: Vec2| inside(&b.outer, p) && !b.islands.iter().any(|i| inside(&i.outline, p));
    let mut searches = 0;
    for at in &seeds {
        if let Some(f) = faces.iter_mut().find(|(b, _)| in_face(b, *at)) {
            f.1 += 1;
            continue;
        }
        if searches >= MAX_SEARCHES {
            continue;
        }
        searches += 1;
        let Some(b) = boundary_at(&pen, *at, tol) else { continue };
        let a = area(&b.outer);
        let bb = Bounds2::from_points(b.outer.iter().copied());
        // Not the border, nor a strip of the title block.
        if a >= 100.0 * upf * upf
            && a <= 0.7 * region.width() * region.height()
            && bb.width().min(bb.height()) >= 8.0 * upf
            && !sheet_border(&b.outer, region)
        {
            faces.push((b, 1));
        }
    }
    // The roof: the face most notes are on (the larger of equals).
    let best = faces.iter().max_by(|a, b| a.1.cmp(&b.1).then(area(&a.0.outer).total_cmp(&area(&b.0.outer))))?;
    let face = &best.0;
    let mut outline = face.outer.clone();
    // A parapet: a ring a parapet's thickness wide round the face (its outer edge is the roof's).
    let ring = outline.iter().zip(outline.iter().cycle().skip(1)).max_by(|a, b| a.0.dist(*a.1).total_cmp(&b.0.dist(*b.1))).and_then(|(a, b)| {
        let len = a.dist(*b);
        // Counter-clockwise: outside is on the right.
        let out = Vec2::new(b.y - a.y, a.x - b.x) * (1.0 / len.max(1e-12));
        let probe = (*a + *b) * 0.5 + out * (upf / 12.0);
        let o = boundary_at(&pen, probe, tol)?.outer;
        let gap = outline
            .iter()
            .map(|p| o.iter().zip(o.iter().cycle().skip(1)).map(|(e0, e1)| seg_dist(*p, *e0, *e1)).fold(f64::INFINITY, f64::min))
            .fold(f64::INFINITY, f64::min);
        (area(&o) > area(&outline)
            && outline.iter().all(|p| inside(&o, *p))
            && gap >= 4.0 / 12.0 * upf
            && gap <= 2.0 * upf
            && !sheet_border(&o, region))
        .then_some(o)
    });
    // Its height: the roof plan's note, else the elevation's, else a default for one drawn.
    let parapet_note = notes.iter().find(|(_, t, at, _)| has_word(t, &["PARAPET"]) && inside(&face.outer, *at));
    let noted = parapet_note.and_then(|(_, t, _, _)| feet_inches(&t.to_ascii_uppercase())).map(|i| i / 12.0);
    let elevation_parapet = sp.exterior.level_parapet(level);
    let (parapet, parapet_status, parapet_from) = match (noted, elevation_parapet) {
        (Some(h), _) => (Some(h), Some(ValueStatus::Read), Some(FROM_ROOF_PLAN)),
        (None, Some(h)) => (Some(h), Some(ValueStatus::Read), Some(FROM_ELEVATION)),
        (None, None) if parapet_note.is_some() || ring.is_some() => (Some(DEFAULT_PARAPET_FT), Some(ValueStatus::Default), Some(FROM_DEFAULT)),
        _ => (None, None, None),
    };
    if let Some(o) = ring {
        outline = o;
    }
    // One slope when its arrows agree; arrows every way are tapered insulation to drains.
    let pitched: Vec<&(Handle, String, Vec2, f64)> = notes.iter().filter(|(_, t, at, _)| pitch(t).is_some() && inside(&outline, *at)).collect();
    let slopes: Vec<RoofSlope> = pitched.iter().filter_map(|n| note_slope(&outline, std::slice::from_ref(*n), &segs, upf)).collect();
    let slope = slopes
        .first()
        .copied()
        .filter(|s0| slopes.iter().all(|s| s.down[0] * s0.down[0] + s.down[1] * s0.down[1] > 0.9 && (s.rise_per_ft - s0.rise_per_ft).abs() < 1e-9));
    // The curbs on it: each note names the nearest one within 4 feet.
    let near = 4.0 * upf;
    let box_dist = |q: &[Vec2], p: Vec2| {
        let b = Bounds2::from_points(q.iter().copied());
        let dx = (b.min.x - p.x).max(p.x - b.max.x).max(0.0);
        let dy = (b.min.y - p.y).max(p.y - b.max.y).max(0.0);
        (dx * dx + dy * dy).sqrt()
    };
    // The curbs: closed groups of the roof's lines inside it, and closed outlines on any layer
    // too big for lettering (an importer can take a plain box for a letter).
    let mut curbs: Vec<Vec<Vec2>> = face
        .islands
        .iter()
        .map(|i| i.outline.clone())
        .filter(|q| area(q) >= upf * upf && area(q) < 2_000.0 * upf * upf)
        .take(MAX_ROOF_ITEMS)
        .collect();
    for (q, _) in outlines(&ents).into_iter().take(MAX_ROOF_ITEMS) {
        let b = Bounds2::from_points(q.iter().copied());
        let a = area(&q);
        let same = |c: &Vec<Vec2>| inside(c, b.center()) && (area(c) - a).abs() <= 0.2 * a;
        if b.width().min(b.height()) >= 1.5 * upf && a < 2_000.0 * upf * upf && in_face(face, b.center()) && !curbs.iter().any(same) {
            curbs.push(q);
        }
    }
    let mut named: Vec<Vec<&str>> = vec![Vec::new(); curbs.len()];
    for (_, t, at, _) in notes.iter().filter(|(_, _, at, _)| inside(&outline, *at)) {
        let best = (0..curbs.len())
            .filter_map(|i| curbs.get(i).map(|c| (i, box_dist(c, *at))))
            .filter(|(_, dist)| *dist <= near)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some(words) = best.and_then(|(i, _)| named.get_mut(i)) {
            words.push(t.as_str());
        }
    }
    let (mut crickets, mut equipment) = (Vec::new(), Vec::new());
    for (c, words) in curbs.iter().zip(&named) {
        let ring: Vec<[f64; 2]> = c.iter().map(|p| [p.x, p.y]).collect();
        if let Some(t) = words.iter().find(|t| has_word(t, &["CRICKET"])) {
            let read = feet_inches(&t.to_ascii_uppercase()).map(|i| i / 12.0);
            let height_status = Some(if read.is_some() { ValueStatus::Read } else { ValueStatus::Default });
            crickets.push(Cricket { level: level.into(), outline: ring, height_ft: read.unwrap_or(DEFAULT_CRICKET_FT), height_status });
            continue;
        }
        let unit = words.iter().find(|t| has_word(t, &EQUIPMENT_WORDS));
        let Some(name_note) = unit.or_else(|| words.iter().find(|t| has_word(t, &CURB_WORDS))) else { continue };
        let name: String = if unit.is_some() {
            name_note.split_whitespace().next().unwrap_or("EQUIP").chars().take(40).collect()
        } else {
            name_note.trim().chars().take(40).collect()
        };
        // Its height: one a note gives above the roof, else a unit's 4', else its curb's.
        let above = words.iter().filter(|t| t.to_ascii_uppercase().contains("ABOVE")).find_map(|t| feet_inches(&t.to_ascii_uppercase()));
        let curb = words.iter().filter(|t| has_word(t, &["CURB"])).find_map(|t| feet_inches(&t.to_ascii_uppercase()));
        let read = above.or(if unit.is_some() { None } else { curb }).map(|i| i / 12.0);
        let height_status = Some(if read.is_some() { ValueStatus::Read } else { ValueStatus::Default });
        let height_ft = read.unwrap_or(if unit.is_some() { DEFAULT_EQUIPMENT_FT } else { 1.0 }).clamp(0.1, 60.0);
        if !equipment.iter().any(|e: &Equipment| e.outline == ring) {
            equipment.push(Equipment { name, level: level.into(), outline: ring, height_ft, height_status });
        }
    }
    // Drains at their notes on the roof (the legend's are off it), at the circle beside one.
    let centres: Vec<Vec2> = ents
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some(c.center.xy()),
            EntityKind::Insert(i) => Some(i.insert.xy()),
            _ => None,
        })
        .collect();
    let mut drains: Vec<RoofDrain> = Vec::new();
    for (_, _, at, _) in notes.iter().filter(|(_, t, at, _)| has_word(t, &["RD", "DRAIN"]) && in_face(face, *at)).take(MAX_ROOF_ITEMS) {
        let c = centres.iter().copied().filter(|c| c.dist(*at) <= 3.0 * upf).min_by(|a, b| a.dist(*at).total_cmp(&b.dist(*at))).unwrap_or(*at);
        if !drains.iter().any(|x| Vec2::new(x.at[0], x.at[1]).dist(c) < 2.0 * upf) {
            drains.push(RoofDrain { level: level.into(), at: [c.x, c.y] });
        }
    }
    let roof = Roof {
        name: "Roof 1".into(),
        level: level.into(),
        outline: outline.iter().map(|p| [p.x, p.y]).collect(),
        slope,
        parapet_ft: parapet,
        parapet_status,
        parapet_from: parapet_from.map(str::to_string),
        assembly: None,
    };
    Some(PlanRoof { roofs: vec![roof], crickets, equipment, drains })
}

/// A roof's slope: the pitch note inside its outline, and the arrow nearest it (downhill to
/// its head).
fn note_slope(q: &[Vec2], notes: &[(Handle, String, Vec2, f64)], segs: &[(Vec2, Vec2, Handle)], upf: f64) -> Option<RoofSlope> {
    let (_, t, at, _) = notes.iter().find(|(_, t, at, _)| cadaddy_geom::point_in_polygon(q, *at) && pitch(t).is_some())?;
    let rise = pitch(t)?;
    let arrow = segs
        .iter()
        .filter(|(a, b, _)| a.dist(*b) >= 3.0 * upf && cadaddy_geom::point_in_polygon(q, (*a + *b) * 0.5))
        .filter(|(a, b, _)| !q.iter().zip(q.iter().cycle().skip(1)).any(|(e0, e1)| seg_dist(*a, *e0, *e1) < 1e-6 && seg_dist(*b, *e0, *e1) < 1e-6))
        .min_by(|x, y| seg_dist(*at, x.0, x.1).total_cmp(&seg_dist(*at, y.0, y.1)))?;
    let (a, b) = (arrow.0, arrow.1);
    // The head: the end other short strokes meet.
    let strokes = |p: Vec2| segs.iter().filter(|(c, e, _)| c.dist(*e) < a.dist(b) * 0.3 && (c.dist(p) < 1e-6 || e.dist(p) < 1e-6)).count();
    let (sa, sb) = (strokes(a), strokes(b));
    let (tail, head) = if sa > sb { (b, a) } else { (a, b) };
    let dir = head - tail;
    let len = dir.len().max(1e-12);
    // No head to tell the ends apart: downhill to the end drawn last.
    let direction_status = Some(if sa == sb { ValueStatus::Default } else { ValueStatus::Read });
    Some(RoofSlope { down: [dir.x / len, dir.y / len], rise_per_ft: rise, direction_status })
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

// ---------------- building ----------------

/// The roof surface's height above its level at a plan point, drawing units: 0 at its lowest
/// corner, rising against the slope.
fn surface(r: &Roof) -> impl Fn(Vec2) -> f64 + '_ {
    let low = r.slope.map_or(0.0, |s| r.outline.iter().map(|p| p[0] * s.down[0] + p[1] * s.down[1]).fold(f64::NEG_INFINITY, f64::max));
    move |p: Vec2| match r.slope {
        Some(s) => {
            let along = p.x * s.down[0] + p.y * s.down[1];
            let n = (s.down[0] * s.down[0] + s.down[1] * s.down[1]).sqrt().max(1e-12);
            (low - along) / n * s.rise_per_ft
        }
        None => 0.0,
    }
}

/// A slab over `poly` whose top follows `top` and whose underside is `t` below its lowest point.
fn slab(poly: &[Vec2], top: &dyn Fn(Vec2) -> f64, t: f64) -> Option<Mesh> {
    let tris = super::bspec::ear_clip(poly);
    if tris.is_empty() {
        return None;
    }
    let ccw = cadaddy_geom::shoelace(poly) > 0.0;
    let ring: Vec<Vec2> = if ccw { poly.to_vec() } else { poly.iter().rev().copied().collect() };
    let tris = super::bspec::ear_clip(&ring);
    let n = u32::try_from(ring.len()).ok()?;
    let low = ring.iter().map(|p| top(*p)).fold(f64::INFINITY, f64::min) - t;
    let mut vertices: Vec<Vec3> = ring.iter().map(|q| Vec3::new(q.x, q.y, low)).collect();
    vertices.extend(ring.iter().map(|q| Vec3::new(q.x, q.y, top(*q))));
    let id = |i: usize| u32::try_from(i).unwrap_or(0);
    let mut faces = Vec::new();
    for tr in &tris {
        faces.push(MeshFace { v: vec![id(tr[0]), id(tr[2]), id(tr[1])], hidden: 0 });
        faces.push(MeshFace { v: vec![id(tr[0]) + n, id(tr[1]) + n, id(tr[2]) + n], hidden: 0 });
    }
    for i in 0..n {
        let j = (i + 1) % n;
        faces.push(MeshFace { v: vec![i, j, j + n, i + n], hidden: 0 });
    }
    Some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

/// The exterior into model space (sheet units): roofs, parapets, crickets, equipment, facade
/// glass and materials. The counts.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let upf = units(sp);
    let (ext, notes) = with_plan_roofs(d, sp);
    for (layer, color) in [("3D-Roofs", 33), ("3D-Parapets", 8), ("3D-Equipment", 252)] {
        if d.layer(layer).is_none() {
            d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(color), ..cadaddy_doc::Layer::default() });
        }
    }
    let tag = |part: &str, level: &str, name: &str| {
        let t = |code: i32, v: String| RawTag { code, value: v };
        vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, part.into()),
            t(1000, level.chars().take(255).collect()),
            t(1000, Handle(0).hex()),
            t(1000, name.chars().take(240).collect()),
        ]
    };
    // What the takeoff measures, on the element: `area:` sq ft, `lf:` feet.
    let measured = |mut x: Vec<RawTag>, key: &str, v: f64| {
        if v.is_finite() && v > 0.0 {
            x.push(RawTag { code: 1000, value: format!("{key}:{}", (v * 100.0).round() / 100.0) });
        }
        x
    };
    let perimeter = |poly: &[Vec2]| poly.iter().zip(poly.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum::<f64>() / upf;
    let (mut roofs, mut parapets, mut crickets, mut equipment, mut glass, mut drains) = (0, 0, 0, 0, 0, 0);
    let add = |d: &mut Drawing, layer: &str, xdata: Vec<RawTag>, m: Mesh| {
        !m.faces.is_empty()
            && d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok()
    };
    for r in ext.roofs.iter().take(10_000) {
        let Some(level) = sp.level(&r.level) else { continue };
        let z0 = level.elevation_ft * upf;
        let poly: Vec<Vec2> = r.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let rise = surface(r);
        let top = |p: Vec2| z0 + rise(p);
        let plan_area = cadaddy_geom::shoelace(&poly).abs() / (upf * upf);
        if let Some(m) = slab(&poly, &top, upf)
            && add(d, "3D-Roofs", measured(tag("roof", &level.name, &r.name), "area", plan_area), m)
        {
            roofs += 1;
        }
        if let Some(h) = r.parapet_ft.or_else(|| sp.exterior.level_parapet(&r.level)) {
            // Round the outline, 8" thick inside it, up to the parapet over the high point.
            let high = poly.iter().map(|p| top(*p)).fold(f64::NEG_INFINITY, f64::max) + h * upf;
            let t = 8.0 / 12.0 * upf;
            let ccw = cadaddy_geom::shoelace(&poly) > 0.0;
            let mut m = super::wallgen::empty_mesh();
            for (a, b) in poly.iter().zip(poly.iter().cycle().skip(1)) {
                let len = a.dist(*b);
                if len <= 1e-9 {
                    continue;
                }
                let u = (*b - *a) * (1.0 / len);
                let inward = if ccw { Vec2::new(-u.y, u.x) } else { Vec2::new(u.y, -u.x) };
                let (pa, pb) = (*a + inward * (t / 2.0), *b + inward * (t / 2.0));
                super::wallgen::add_box(&mut m, pa, pb, t, z0, high);
            }
            if add(d, "3D-Parapets", measured(tag("parapet", &level.name, &r.name), "lf", perimeter(&poly)), m) {
                parapets += 1;
            }
        }
    }
    // No explicit roof in the spec: if there is a ROOF datum, build a flat roof from the top
    // level's building outline (the bounding rectangle of its walls).
    if roofs == 0
        && let Some(roof_datum) = sp.datums.iter().find(|dt| dt.kind == DatumKind::Roof)
        && let Some((_, bb)) = top_walls(d, sp, f64::INFINITY)
    {
        let z0 = roof_datum.elevation_ft * upf;
        if !bb.is_empty() {
            let poly =
                vec![Vec2::new(bb.min.x, bb.min.y), Vec2::new(bb.max.x, bb.min.y), Vec2::new(bb.max.x, bb.max.y), Vec2::new(bb.min.x, bb.max.y)];
            let top_fn = |_: Vec2| z0;
            if let Some(m) = slab(&poly, &top_fn, upf)
                && add(d, "3D-Roofs", measured(tag("roof", &roof_datum.name, "auto"), "area", bb.width() * bb.height() / (upf * upf)), m)
            {
                roofs += 1;
            }
            // A PARAPET datum above the roof: add parapets round the outline.
            if let Some(parapet_datum) = sp.datums.iter().find(|dt| dt.kind == DatumKind::Parapet) {
                let parapet_z = parapet_datum.elevation_ft * upf;
                if parapet_z > z0 {
                    let t = 8.0 / 12.0 * upf;
                    let mut m = super::wallgen::empty_mesh();
                    for (a, b) in poly.iter().zip(poly.iter().cycle().skip(1)) {
                        let len = a.dist(*b);
                        if len <= 1e-9 {
                            continue;
                        }
                        let u = (*b - *a) * (1.0 / len);
                        // Inward for CCW (our rectangle is CCW).
                        let inward = Vec2::new(-u.y, u.x);
                        let (pa, pb) = (*a + inward * (t / 2.0), *b + inward * (t / 2.0));
                        super::wallgen::add_box(&mut m, pa, pb, t, z0, parapet_z);
                    }
                    if add(d, "3D-Parapets", measured(tag("parapet", &roof_datum.name, "auto"), "lf", perimeter(&poly)), m) {
                        parapets += 1;
                    }
                }
            }
        }
    }
    // The roof surface under a point of a level (for what stands on it).
    let roof_at = |level: &str, p: Vec2| {
        ext.roofs.iter().filter(|r| r.level.eq_ignore_ascii_case(level)).find_map(|r| {
            let poly: Vec<Vec2> = r.outline.iter().map(|q| Vec2::new(q[0], q[1])).collect();
            cadaddy_geom::point_in_polygon(&poly, p).then(|| surface(r)(p))
        })
    };
    for c in ext.crickets.iter().take(10_000) {
        let Some(level) = sp.level(&c.level) else { continue };
        let poly: Vec<Vec2> = c.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let n = poly.len().max(1) as f64;
        let apex = poly.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / n);
        let z0 = level.elevation_ft * upf;
        let base = |p: Vec2| z0 + roof_at(&level.name, p).unwrap_or(0.0);
        let mut vertices: Vec<Vec3> = poly.iter().map(|p| Vec3::new(p.x, p.y, base(*p))).collect();
        vertices.push(Vec3::new(apex.x, apex.y, base(apex) + c.height_ft * upf));
        let k = u32::try_from(poly.len()).unwrap_or(0);
        let faces = (0..k).map(|i| MeshFace { v: vec![i, (i + 1) % k, k], hidden: 0 }).collect();
        if k >= 3 && add(d, "3D-Roofs", tag("cricket", &level.name, ""), Mesh { vertices, faces, kind: MeshKind::Polyface }) {
            crickets += 1;
        }
    }
    for e in ext.equipment.iter().take(10_000) {
        let Some(level) = sp.level(&e.level) else { continue };
        let poly: Vec<Vec2> = e.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let n = poly.len().max(1) as f64;
        let c = poly.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / n);
        let z = level.elevation_ft * upf + roof_at(&level.name, c).unwrap_or(0.0);
        if let Some(m) = super::bspec::prism(&poly, z, z + e.height_ft * upf)
            && add(d, "3D-Equipment", tag("equipment", &level.name, &e.name), m)
        {
            equipment += 1;
        }
    }
    // Roof drains: a 1'-6" bowl standing 4" proud of the roof.
    for r in ext.drains.iter().take(10_000) {
        let Some(level) = sp.level(&r.level) else { continue };
        let c = Vec2::new(r.at[0], r.at[1]);
        let z = level.elevation_ft * upf + roof_at(&level.name, c).unwrap_or(0.0);
        let ring: Vec<Vec2> = (0..8).map(|i| c + Vec2::new(0.75 * upf, 0.0).rotate(f64::from(i) * std::f64::consts::FRAC_PI_4)).collect();
        if let Some(m) = super::bspec::prism(&ring, z, z + upf / 3.0)
            && add(d, "3D-Roofs", tag("roof-drain", &level.name, "RD"), m)
        {
            drains += 1;
        }
    }
    // Facades: glass panels on the building's outside face, materials named on its walls.
    if !sp.exterior.facades.is_empty() {
        let ground = sp.levels.iter().min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).map(|l| (l.name.clone(), l.elevation_ft));
        let walls: Vec<(Handle, Bounds2)> = d
            .model
            .iter()
            .filter(|e| super::wallgen::made_as(e).as_deref() == Some("wall"))
            .filter(|e| ground.as_ref().is_some_and(|(g, _)| super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(g))))
            .map(|e| (e.handle, cadaddy_doc::entity_bounds(d, e, 0)))
            .collect();
        let bb = walls.iter().fold(Bounds2::EMPTY, |a, (_, b)| a.union(b));
        if let (Some((gname, gz)), false) = (ground, bb.is_empty()) {
            let z0 = gz * upf;
            let t = 2.0 / 12.0 * upf;
            for f in &sp.exterior.facades {
                // The face, from its left end as seen from outside, and outward.
                let (start, dir, out) = match f.side {
                    Side::South => (Vec2::new(bb.min.x, bb.min.y), Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0)),
                    Side::North => (Vec2::new(bb.max.x, bb.max.y), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0)),
                    Side::East => (Vec2::new(bb.max.x, bb.min.y), Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)),
                    Side::West => (Vec2::new(bb.min.x, bb.max.y), Vec2::new(0.0, -1.0), Vec2::new(-1.0, 0.0)),
                };
                let length = if matches!(f.side, Side::South | Side::North) { bb.width() } else { bb.height() } / upf;
                let mut m = super::wallgen::empty_mesh();
                for b in f.bands.iter().filter(|b| b.kind == BandKind::Glass) {
                    let (s0, s1) = (b.start_ft.unwrap_or(0.0).max(0.0), b.end_ft.unwrap_or(length).min(length));
                    if s1 > s0 {
                        let a = start + dir * (s0 * upf) + out * (t / 2.0);
                        let c = start + dir * (s1 * upf) + out * (t / 2.0);
                        super::wallgen::add_box(&mut m, a, c, t, z0 + b.from_ft * upf, z0 + b.to_ft * upf);
                    }
                }
                if add(d, "3D-Windows", tag("facade-glass", &gname, f.side.name()), m) {
                    glass += 1;
                }
                // The material covering most of the facade's height names its walls.
                let material = f
                    .bands
                    .iter()
                    .filter(|b| b.kind == BandKind::Material && b.material.is_some())
                    .max_by(|a, b| (a.to_ft - a.from_ft).total_cmp(&(b.to_ft - b.from_ft)))
                    .and_then(|b| b.material.clone());
                if let Some(mat) = material {
                    let face = |w: &Bounds2| match f.side {
                        Side::South => (w.min.y - bb.min.y).abs() < 1e-6,
                        Side::North => (w.max.y - bb.max.y).abs() < 1e-6,
                        Side::East => (w.max.x - bb.max.x).abs() < 1e-6,
                        Side::West => (w.min.x - bb.min.x).abs() < 1e-6,
                    };
                    for (h, _) in walls.iter().filter(|(_, w)| face(w)) {
                        let v = format!("facade:{}:{}", f.side.name(), mat.chars().take(200).collect::<String>());
                        let _ = d.modify_entity(*h, |e| e.common.xdata.push(RawTag { code: 1000, value: v }));
                    }
                }
            }
        }
    }
    json!({ "roofs": roofs, "parapets": parapets, "crickets": crickets, "equipment": equipment, "roofDrains": drains, "facadeGlass": glass, "roofNotes": notes })
}

/// The spec's exterior with the roofs of roof levels it has none for: read off the level's
/// roof plan ([`read_plan`]), else a flat roof over the walls of the storey under it, moved onto
/// the roof plan's sheet (so stacking puts it over them). What was done, as notes.
fn with_plan_roofs(d: &Drawing, sp: &BuildingSpec) -> (Exterior, Vec<String>) {
    let upf = units(sp);
    let mut ext = sp.exterior.clone();
    let mut notes = Vec::new();
    let rect = |r: [f64; 4]| Bounds2::new(Vec2::new(r[0].min(r[2]), r[1].min(r[3])), Vec2::new(r[0].max(r[2]), r[1].max(r[3])));
    for level in sp.levels.iter().filter(|l| l.sheet.is_none() && super::wallgen::roof_level(&sp.levels, &l.name)).take(100) {
        if ext.roofs.iter().any(|r| r.level.eq_ignore_ascii_case(&level.name)) {
            continue;
        }
        if let Some(read) = level.region.and_then(|r| read_plan(d, sp, &rect(r), &level.name)) {
            let sq_ft: f64 = read
                .roofs
                .iter()
                .map(|r| cadaddy_geom::shoelace(&r.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect::<Vec<_>>()).abs())
                .sum::<f64>()
                / (upf * upf);
            notes.push(format!(
                "{}: roof read from its plan: {} sq ft{}, {} drain(s), {} curb(s) and unit(s).",
                level.name,
                sq_ft.round(),
                read.roofs.iter().find_map(|r| r.parapet_ft).map_or(String::new(), |p| format!(", parapet {}", cadaddy_walk3d::feet(p))),
                read.drains.len(),
                read.equipment.len()
            ));
            ext.roofs.extend(read.roofs);
            ext.crickets.extend(read.crickets);
            ext.equipment.extend(read.equipment);
            ext.drains.extend(read.drains);
            continue;
        }
        let Some((under, bb)) = top_walls(d, sp, level.elevation_ft) else {
            notes.push(format!("{}: no roof outline on its plan and no walls under it, so no roof is built.", level.name));
            continue;
        };
        // As far from the roof plan's corner as the walls are from their plan's.
        let shift = match (level.region, under.region) {
            (Some(a), Some(b)) => rect(a).min - rect(b).min,
            _ => Vec2::ZERO,
        };
        let (lo, hi) = (bb.min + shift, bb.max + shift);
        ext.roofs.push(Roof {
            name: "Roof 1".into(),
            level: level.name.clone(),
            outline: vec![[lo.x, lo.y], [hi.x, lo.y], [hi.x, hi.y], [lo.x, hi.y]],
            slope: None,
            parapet_ft: sp.exterior.level_parapet(&level.name),
            parapet_status: sp.exterior.level_parapet(&level.name).map(|_| ValueStatus::Read),
            parapet_from: sp.exterior.level_parapet(&level.name).map(|_| FROM_ELEVATION.to_string()),
            assembly: None,
        });
        notes.push(format!("{}: no roof outline read on its plan; a flat roof over {}'s walls.", level.name, under.name));
    }
    (ext, notes)
}

/// The highest storey under `below_ft` with walls built (not a roof), and their plan extents.
fn top_walls<'a>(d: &Drawing, sp: &'a BuildingSpec, below_ft: f64) -> Option<(&'a cadaddy_walk3d::Level, Bounds2)> {
    let mut storeys: Vec<&cadaddy_walk3d::Level> =
        sp.levels.iter().filter(|l| l.elevation_ft < below_ft && !super::wallgen::roof_level(&sp.levels, &l.name)).collect();
    storeys.sort_by(|a, b| b.elevation_ft.total_cmp(&a.elevation_ft));
    storeys.into_iter().find_map(|l| Some(built_bounds(d, &l.name, &["wall"])).filter(|b| !b.is_empty()).map(|b| (l, b)))
}

/// The plan extents of the objects built on `level` as one of `parts` (sheet coordinates).
pub(crate) fn built_bounds(d: &Drawing, level: &str, parts: &[&str]) -> Bounds2 {
    let mut bb = Bounds2::EMPTY;
    for e in d.model.iter() {
        let Some(part) = super::wallgen::made_as(e) else { continue };
        if !parts.iter().any(|p| p.eq_ignore_ascii_case(&part)) || !super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(level)) {
            continue;
        }
        bb = bb.union(&cadaddy_doc::entity_bounds(d, e, 0));
    }
    bb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes() {
        assert!((pitch("1/4\" / FT").unwrap_or(0.0) - 0.25 / 12.0).abs() < 1e-12);
        assert!((pitch("SLOPE 1/4\":12\"").unwrap_or(0.0) - 0.25 / 12.0).abs() < 1e-12);
        assert!((pitch("4:12").unwrap_or(0.0) - 4.0 / 12.0).abs() < 1e-12);
        assert!((pitch("2%").unwrap_or(0.0) - 0.02).abs() < 1e-12);
        assert!((pitch("1 1/2 IN/FT").unwrap_or(0.0) - 1.5 / 12.0).abs() < 1e-12);
        assert_eq!(pitch("RTU-1"), None);
        assert_eq!(label_name("LEVEL 2 EL. 112'-0\""), "LEVEL 2");
        assert!(names_level("LEVEL 2", "Level 2") && names_level("LEVEL 2", "L2") && !names_level("LEVEL 2", "L3"));
        assert_eq!(side_in("NORTH ELEVATION"), Some(Side::North));
        assert_eq!(side_in("NORTH"), None);
    }
}
