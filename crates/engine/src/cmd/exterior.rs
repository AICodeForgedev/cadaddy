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
//!   `CRICKET` note) and rooftop equipment (outlines named RTU, AHU, EF, MAU, CU…).
//!
//! `apply: true` writes what was read into the spec. The build puts roofs on their level (sloped
//! slabs over their outlines, parapets round them, crickets as hips rising to their apex,
//! equipment as boxes on the roof), and the facades' glass bands as panels on the building's
//! outside faces, with the material bands named on the walls of that side.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::{Bounds2, Polyline, Vec2, Vec3};
use cadaddy_walk3d::{Band, BandKind, BuildingSpec, Cricket, Equipment, Facade, Roof, RoofSlope, Side};
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
            "{region: [x0,y0,x1,y1] (the elevation on the sheet), facade?: north|south|east|west (else from its title), apply?: bool (into the spec: level elevations and heights, window sill and head, the facade's bands, parapets)} → {facade, levels: [{label, name, y, elevation_ft, matched?}], roof_ft?, parapet_ft?, windows: {count, sill_ft, head_ft}, bands: [...], applied}",
        ),
        CommandSpec::new("roof.read", "Read Roof Plan", run_roof).params(
            "{region: [x0,y0,x1,y1] (the roof plan on the sheet), level?: the roof level's name (default: one named ROOF), apply?: bool} → {roofs: [{name, outline, slope?: {down, rise_per_ft}, parapet_ft?}], crickets, equipment, applied}",
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
    let ents = plan_objects(d)
        .into_iter()
        .filter(|e| {
            let eb = cadaddy_doc::entity_bounds(d, e, 0);
            !eb.is_empty() && b.contains(eb.center())
        })
        .take(MAX_OBJECTS)
        .collect();
    Ok((b, ents))
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
    let base_ft = sp.levels.iter().find(|l| names_level(&label_name(&ground.label), &l.name)).map_or(0.0, |l| l.elevation_ft);
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
        if let Some(pp) = parapet {
            for r in &mut sp.exterior.roofs {
                r.parapet_ft.get_or_insert(round(pp));
            }
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({
        "facade": side.map(|s| s.name()),
        "levels": out_levels,
        "roof_ft": roof.map(|r| round(base_ft + r)),
        "parapet_ft": parapet.map(round),
        "windows": { "count": sills.len(), "sill_ft": sill, "head_ft": head },
        "bands": serde_json::to_value(&bands).unwrap_or(Value::Null),
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
    let (_, ents) = in_region(d, p, "roof.read")?;
    let notes = texts(&ents);
    let segs = segments(&ents);
    let all = outlines(&ents);
    let area = |q: &[Vec2]| cadaddy_geom::shoelace(q).abs();
    let inside = |inner: &[Vec2], outer: &[Vec2]| inner.iter().all(|p| cadaddy_geom::point_in_polygon(outer, *p));
    let up = |t: &str| t.to_ascii_uppercase();
    let word = |t: &str, ws: &[&str]| {
        let u = up(t);
        u.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| ws.iter().any(|x| w == *x || (w.starts_with(x) && w[x.len()..].chars().all(|c| c.is_ascii_digit()))))
    };
    let named = |q: &[Vec2], ws: &[&str]| {
        notes.iter().find(|(_, t, at, _)| cadaddy_geom::point_in_polygon(q, *at) && word(t, ws)).map(|(_, t, _, _)| t.clone())
    };
    let equipment_words = ["RTU", "AHU", "EF", "MAU", "CU", "ACCU", "UNIT", "EQUIP", "EQUIPMENT", "HVAC"];
    // Rooftop equipment and crickets: each note names the smallest outline holding it.
    let mut equipment = Vec::new();
    let mut crickets = Vec::new();
    let mut taken: Vec<usize> = Vec::new();
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
        if cricket {
            let h = feet_inches(&up(t)).map_or(0.5, |i| i / 12.0);
            crickets.push(Cricket { level: level.clone(), outline, height_ft: h });
        } else {
            let h = feet_inches(&up(t)).map_or(4.0, |i| i / 12.0);
            let name = t.split_whitespace().next().unwrap_or("EQUIP").to_string();
            equipment.push(Equipment { name, level: level.clone(), outline, height_ft: h });
        }
        taken.push(i);
    }
    // Roof areas: the outermost remaining outlines of 100 sq ft or more.
    let candidates: Vec<usize> =
        (0..all.len()).filter(|i| !taken.contains(i) && all.get(*i).is_some_and(|(q, _)| area(q) >= 100.0 * upf * upf)).collect();
    let mut roofs = Vec::new();
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
        let parapet = match (&parapet_note, inner) {
            (Some(t), _) => Some(feet_inches(&up(t)).map_or(3.0, |i| i / 12.0)),
            (None, Some(_)) => Some(3.0),
            _ => None,
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
            let (tail, head) = if strokes(a) > strokes(b) { (b, a) } else { (a, b) };
            let dir = head - tail;
            let len = dir.len().max(1e-12);
            Some(RoofSlope { down: [dir.x / len, dir.y / len], rise_per_ft: rise })
        });
        let name = format!("Roof {}", roofs.len() + 1);
        roofs.push(Roof { name, level: level.clone(), outline: q.iter().map(|p| [p.x, p.y]).collect(), slope, parapet_ft: parapet });
    }
    if roofs.is_empty() {
        return Err(bad("roof.read", "no roof outline found in the region (a closed outline of 100 sq ft or more)"));
    }
    let apply = bool_or(p, "apply", false);
    let result = json!({
        "level": level,
        "roofs": serde_json::to_value(&roofs).unwrap_or(Value::Null),
        "crickets": serde_json::to_value(&crickets).unwrap_or(Value::Null),
        "equipment": serde_json::to_value(&equipment).unwrap_or(Value::Null),
        "applied": apply,
    });
    if apply {
        sp.exterior.roofs.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.crickets.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.equipment.retain(|r| !r.level.eq_ignore_ascii_case(&level));
        sp.exterior.roofs.extend(roofs);
        sp.exterior.crickets.extend(crickets);
        sp.exterior.equipment.extend(equipment);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(result)
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
    let (mut roofs, mut parapets, mut crickets, mut equipment, mut glass) = (0, 0, 0, 0, 0);
    let add = |d: &mut Drawing, layer: &str, xdata: Vec<RawTag>, m: Mesh| {
        !m.faces.is_empty() && d.add(&Space::Model, Common { layer: layer.into(), xdata, ..Common::default() }, EntityKind::Mesh(m)).is_ok()
    };
    for r in sp.exterior.roofs.iter().take(10_000) {
        let Some(level) = sp.level(&r.level) else { continue };
        let z0 = level.elevation_ft * upf;
        let poly: Vec<Vec2> = r.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let rise = surface(r);
        let top = |p: Vec2| z0 + rise(p);
        if let Some(m) = slab(&poly, &top, upf)
            && add(d, "3D-Roofs", tag("roof", &level.name, &r.name), m)
        {
            roofs += 1;
        }
        if let Some(h) = r.parapet_ft {
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
            if add(d, "3D-Parapets", tag("parapet", &level.name, &r.name), m) {
                parapets += 1;
            }
        }
    }
    // The roof surface under a point of a level (for what stands on it).
    let roof_at = |level: &str, p: Vec2| {
        sp.exterior.roofs.iter().filter(|r| r.level.eq_ignore_ascii_case(level)).find_map(|r| {
            let poly: Vec<Vec2> = r.outline.iter().map(|q| Vec2::new(q[0], q[1])).collect();
            cadaddy_geom::point_in_polygon(&poly, p).then(|| surface(r)(p))
        })
    };
    for c in sp.exterior.crickets.iter().take(10_000) {
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
    for e in sp.exterior.equipment.iter().take(10_000) {
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
    json!({ "roofs": roofs, "parapets": parapets, "crickets": crickets, "equipment": equipment, "facadeGlass": glass })
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
