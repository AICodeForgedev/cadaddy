//! The Levels manager: the building's levels and their elevations read from the prints, the
//! sheets on each level, shafts and risers carried on through the storeys, and one storey at a
//! time in 3D.
//!
//! - `levels.read {apply?, datum_ft?}`: level notes on the sections, elevations and level markers
//!   (`LEVEL 2 = 14'-0"`, `T.O. ROOF EL. 42'-6"`, a level name with its elevation just under it),
//!   in model space and on every layout. Applied, the spec's levels take their elevations and
//!   floor to floor heights; the roof, a parapet or a penthouse with no plan here become datums
//!   (IFC storeys, the top of the floor below).
//! - `levels.list`: the levels lowest first, with their datums, the sheets on each and how many
//!   built objects each holds.
//! - `levels.set {name, rename?, elevation_ft?, height_ft?, ceiling_ft?, shift_above?}`: edit a
//!   level (or a datum's elevation); a rename follows every reference in the spec.
//! - `levels.sheets {folder?, apply?}`: each sheet (layouts, and drawing and PDF files in a
//!   folder) by number and title, attached to the level its title names.
//! - `levels.shafts {tolerance_ft?}`: shaft, chase, riser and elevator marks on each level's plan,
//!   matched through the levels; a level between a run's ends (or above a `RISER UP`) where it
//!   isn't marked is reported.
//! - `levels.isolate {level?}`: hide what `model.build` made on every other storey (no level: show
//!   them again). UNISOLATEOBJECTS shows them too.

use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::levels::{self, LevelNote, ShaftMark};
use cadaddy_walk3d::{BuildingSpec, SheetLink, level_at};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::texts;
use super::*;
use crate::{Result, Session};

/// Texts and objects looked at, at most (hostile drawings).
const MAX_TEXTS: usize = 200_000;
/// Files looked at in a folder.
const MAX_FILES: usize = 5_000;
/// The model.build tag (see `wallgen::tag`): part, level, source.
const MADE_APP: &str = "CADADDY_WALLGEN";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("levels.read", "Read Levels", run_read)
            .files().params(
            "{apply?: bool (elevations into the spec's levels; others become datums), datum_ft?: feet subtracted from every elevation (default 100 when LEVEL 1 is at 100'-0\", else 0)} → {levels: [{name, kind, elevation_ft, source, matches?, approved?}], conflicts (incl. approved values kept), datum_ft, changes?, kept?, applied}; approved levels (`levels/<name>`) keep their elevation",
        ),
        CommandSpec::new("levels.list", "List Levels", run_list)
            .params("{} → {levels: [{name, elevation_ft, height_ft, ceiling_ft, top_ft, plan, sheet?, sheets, built, source, approved}], datums: [{name, elevation_ft, kind, source?}], storeys, unattached}")
            .noundo(),
        CommandSpec::new("levels.set", "Edit Level", run_set).params(
            "{name: level or datum, rename?: new name, elevation_ft?: feet, height_ft?: floor to floor, ceiling_ft?: feet, shift_above?: bool (move the levels above by the same change)} → {level, renamed_refs?, changes}",
        ),
        CommandSpec::new("levels.sheets", "Attach Sheets to Levels", run_sheets).params(
            "{folder?: path (its .dwg/.dxf/.pdf/.cdy file names read as sheets), apply?: bool (into the spec's sheets)} → {sheets: [{number, title, discipline, level, source}], unattached, applied}",
        ),
        CommandSpec::new("levels.shafts", "Check Shafts Through Levels", run_shafts)
            .params("{tolerance_ft?: feet (4)} → {runs: [{name, at, levels, missing}], gaps}")
            .noundo(),
        CommandSpec::new("levels.isolate", "Isolate Level", run_isolate)
            .params("{level?: name (none: show every storey again)} → {level?, hidden, shown}"),
    ]
}

/// Every text of the drawing, model space then each layout, with the height it is drawn at.
fn all_texts(d: &Drawing) -> Vec<(Handle, String, Vec2, f64, Option<String>)> {
    let model: Vec<&Arc<Entity>> = plan_objects(d);
    let mut out: Vec<(Handle, String, Vec2, f64, Option<String>)> = texts(&model).into_iter().map(|(h, t, p, ht)| (h, t, p, ht, None)).collect();
    for l in &d.layouts {
        let ents: Vec<&Arc<Entity>> = l.entities.iter().take(MAX_TEXTS).collect();
        out.extend(texts(&ents).into_iter().map(|(h, t, p, ht)| (h, t, p, ht, Some(l.name.clone()))));
        if out.len() >= MAX_TEXTS {
            break;
        }
    }
    out.truncate(MAX_TEXTS);
    out
}

/// The level notes of the drawing: one-text notes, and a level name with an elevation text
/// just under (or beside) it, as level markers on sections are drawn.
fn notes_of(d: &Drawing) -> Vec<LevelNote> {
    let all = all_texts(d);
    let mut out = Vec::new();
    let mut used: Vec<Handle> = Vec::new();
    for (h, t, _, _, sp) in &all {
        if let Some(mut n) = levels::parse_level_note(t) {
            n.source = source_of(sp, t);
            out.push(n);
            used.push(*h);
        }
    }
    // A name alone, and the nearest elevation alone within three text heights of it.
    let lone: Vec<(&Handle, f64, Vec2, f64, &Option<String>)> = all
        .iter()
        .filter(|(h, t, _, _, _)| !used.contains(h) && t.len() <= 24)
        .filter_map(|(h, t, p, ht, sp)| {
            let (v, a, z) = levels::parse_elevation(t)?;
            // Nothing else in it but elevation words.
            let rest = format!("{} {}", t.get(..a).unwrap_or_default(), t.get(z..).unwrap_or_default());
            let rest = rest.to_ascii_uppercase().replace(['.', ':', '='], " ");
            rest.split_whitespace().all(|w| matches!(w, "EL" | "ELEV" | "ELEVATION" | "FFE" | "TOS" | "T" | "O" | "S")).then_some((h, v, *p, *ht, sp))
        })
        .collect();
    for (h, t, p, ht, sp) in &all {
        if used.contains(h) || t.len() > 40 {
            continue;
        }
        let Some((name, kind)) = levels::canonical_level(t) else { continue };
        // Only a bare name: `LEVEL 2 MECHANICAL PLAN` is a title, not a marker.
        if t.split_whitespace().count() > 4 || levels::level_of_title(t).is_some() && t.to_ascii_uppercase().contains("PLAN") {
            continue;
        }
        let reach = 3.0 * ht.max(1e-9);
        let best = lone.iter().filter(|(_, _, q, _, s2)| *s2 == sp && q.dist(*p) <= reach).min_by(|a, b| a.2.dist(*p).total_cmp(&b.2.dist(*p)));
        if let Some((eh, v, _, _, _)) = best {
            let el = all.iter().find(|x| x.0 == **eh).map(|x| x.1.as_str()).unwrap_or_default();
            out.push(LevelNote { name, kind, elevation_ft: *v, source: source_of(sp, &format!("{t} / {el}")) });
        }
    }
    out
}

/// Where a note was read: `model: <text>` or `layout:<name>: <text>`.
fn source_of(layout: &Option<String>, text: &str) -> String {
    let text: String = text.chars().take(120).collect();
    match layout {
        Some(l) => format!("layout:{l}: {text}"),
        None => format!("model: {text}"),
    }
}

fn note_json(n: &LevelNote, sp: &BuildingSpec) -> Value {
    let kind = serde_json::to_value(n.kind).unwrap_or(Value::Null);
    let mut v = json!({ "name": n.name, "kind": kind, "elevation_ft": round(n.elevation_ft), "source": n.source });
    if let Some(l) = sp.level_matching(&n.name).and_then(|i| sp.levels.get(i)) {
        v["matches"] = json!(l.name);
        if sp.level_approved(&l.name) {
            v["approved"] = json!(true);
        }
    }
    v
}

fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn run_read(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "levels.read")?;
    let notes = notes_of(s.doc()?);
    let (mut stacked, mut conflicts) = levels::stack(&notes);
    let datum = match p.get("datum_ft") {
        None | Some(Value::Null) => levels::auto_datum(&stacked),
        Some(v) => v.as_f64().filter(|x| x.is_finite() && x.abs() <= 1e5).ok_or_else(|| bad("levels.read", "`datum_ft` must be a number of feet"))?,
    };
    for n in &mut stacked {
        n.elevation_ft -= datum;
    }
    let apply = bool_or(p, "apply", false);
    let rows: Vec<Value> = stacked.iter().map(|n| note_json(n, &sp)).collect();
    if stacked.is_empty() {
        let mut out = json!({ "levels": rows, "conflicts": conflicts, "datum_ft": datum, "applied": apply });
        out["message"] = json!("No level notes (LEVEL 2 = 14'-0\", ROOF EL. 42'-6\") found in the drawing or its layouts.");
        return Ok(out);
    }
    if !apply {
        // What applying would keep: approved values the prints disagree with.
        let mut trial = sp.clone();
        conflicts.extend(trial.apply_levels_keeping(&stacked).1);
        return Ok(json!({ "levels": rows, "conflicts": conflicts, "datum_ft": datum, "applied": false }));
    }
    let before = sp.clone();
    let (mut changes, kept) = sp.apply_levels_keeping(&stacked);
    if let Some(e) = sp.check().into_iter().find(|i| i.error) {
        return Err(bad("levels.read", format!("{}: {}", e.path, e.message)));
    }
    changes.extend(kept.iter().cloned());
    conflicts.extend(kept.iter().cloned());
    // Sources and cleared guesses are stored even when no elevation moved.
    if sp != before {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "levels": rows, "conflicts": conflicts, "datum_ft": datum, "applied": true, "changes": changes, "kept": kept }))
}

/// The level a model.build object was made on.
fn made_level(e: &Entity) -> Option<&str> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(MADE_APP))?;
    x.get(at + 2).filter(|t| t.code == 1000).map(|t| t.value.as_str())
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "levels.list")?;
    let d = s.doc()?;
    let mut built: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in d.model.iter().take(MAX_TEXTS * 10) {
        if let Some(l) = made_level(e) {
            *built.entry(l.to_ascii_uppercase()).or_default() += 1;
        }
    }
    let mut order: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().collect();
    order.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    let levels: Vec<Value> = order
        .iter()
        .map(|l| {
            let sheets: Vec<&str> =
                sp.sheets.iter().filter(|x| x.level.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&l.name))).map(|x| x.number.as_str()).collect();
            json!({
                "name": l.name,
                "elevation_ft": round(l.elevation_ft),
                "height_ft": round(l.height_ft),
                "ceiling_ft": round(l.ceiling_ft),
                "top_ft": round(l.elevation_ft + l.height_ft),
                "plan": if l.sheet.is_some() { "sheet" } else if l.region.is_some() { "region" } else { "drawing" },
                "sheet": l.sheet,
                "sheets": sheets,
                "built": built.get(&l.name.to_ascii_uppercase()).copied().unwrap_or(0),
                "source": sp.level_source(&l.name),
                "approved": sp.level_approved(&l.name),
            })
        })
        .collect();
    let datums = serde_json::to_value(&sp.datums).unwrap_or(Value::Null);
    let storeys: Vec<Value> = sp.storeys().iter().map(|(n, e)| json!({ "name": n, "elevation_ft": round(*e) })).collect();
    let unattached: Vec<&str> = sp.sheets.iter().filter(|x| x.level.is_none()).map(|x| x.number.as_str()).collect();
    Ok(json!({ "levels": levels, "datums": datums, "storeys": storeys, "unattached": unattached }))
}

fn feet_param(p: &Value, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && x.abs() <= 1e5)
            .map(Some)
            .ok_or_else(|| bad("levels.set", format!("`{key}` must be a number of feet"))),
    }
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "levels.set")?;
    let name = str_param(p, "name").ok_or_else(|| bad("levels.set", "`name` (a level) is required"))?.to_string();
    let elevation = feet_param(p, "elevation_ft")?;
    let height = feet_param(p, "height_ft")?;
    let ceiling = feet_param(p, "ceiling_ft")?;
    let mut changes: Vec<String> = Vec::new();
    let Some(i) = sp.levels.iter().position(|l| l.name.eq_ignore_ascii_case(&name)) else {
        // A datum: only its elevation (and name) can change.
        let Some(dt) = sp.datums.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)) else {
            return Err(bad("levels.set", format!("there is no level or datum named {name}")));
        };
        if let Some(e) = elevation {
            changes.push(format!("{}: elevation {} → {}", dt.name, cadaddy_walk3d::feet(dt.elevation_ft), cadaddy_walk3d::feet(e)));
            dt.elevation_ft = e;
            dt.source = "levels.set".into();
        }
        if let Some(r) = str_param(p, "rename").map(str::trim).filter(|r| !r.is_empty()) {
            dt.name = r.chars().take(64).collect();
        }
        let shown = dt.name.clone();
        sp.datums.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
        changes.extend(sp.restack_heights());
        store(s, &sp)?;
        return Ok(json!({ "level": shown, "changes": changes }));
    };
    let mut out = json!({});
    let mut current_name = sp.levels.get(i).map(|l| l.name.clone()).unwrap_or_default();
    if let Some(r) = str_param(p, "rename").filter(|r| r.trim() != current_name) {
        let n = sp.rename_level(&current_name, r).map_err(|e| bad("levels.set", e))?;
        changes.push(format!("{current_name} renamed {}", r.trim()));
        out["renamed_refs"] = json!(n);
        current_name = r.trim().to_string();
    }
    if let Some(e) = elevation {
        let old = sp.levels.get(i).map(|l| l.elevation_ft).unwrap_or(e);
        let delta = e - old;
        if bool_or(p, "shift_above", false) && delta.abs() > 1e-9 {
            for l in sp.levels.iter_mut().filter(|l| l.elevation_ft > old + 1e-6) {
                l.elevation_ft += delta;
            }
            for dt in sp.datums.iter_mut().filter(|x| x.elevation_ft > old + 1e-6) {
                dt.elevation_ft += delta;
            }
            changes.push(format!("levels above {current_name} moved {}", cadaddy_walk3d::feet(delta)));
        }
        if let Some(l) = sp.levels.get_mut(i) {
            changes.push(format!("{}: elevation {} → {}", l.name, cadaddy_walk3d::feet(old), cadaddy_walk3d::feet(e)));
            l.elevation_ft = e;
        }
        sp.set_level_source(&current_name, "levels.set");
        // The floor below now reaches this one; this one the next up (unless a height is given).
        changes.extend(sp.restack_heights());
    }
    if let Some(h) = height
        && let Some(l) = sp.levels.get_mut(i)
    {
        changes.push(format!("{}: floor to floor {} → {}", l.name, cadaddy_walk3d::feet(l.height_ft), cadaddy_walk3d::feet(h)));
        l.height_ft = h;
    }
    if let Some(c) = ceiling
        && let Some(l) = sp.levels.get_mut(i)
    {
        changes.push(format!("{}: ceiling {} → {}", l.name, cadaddy_walk3d::feet(l.ceiling_ft), cadaddy_walk3d::feet(c)));
        l.ceiling_ft = c;
    }
    if let Some(e) = sp.check().into_iter().find(|x| x.error) {
        return Err(bad("levels.set", format!("{}: {}", e.path, e.message)));
    }
    store(s, &sp)?;
    out["level"] = json!(current_name);
    out["changes"] = json!(changes);
    Ok(out)
}

fn store(s: &mut Session, sp: &BuildingSpec) -> Result<()> {
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

/// A sheet from its number and title text: its discipline and the level its title names (the
/// spec's name for it when it has that level).
fn link(number: String, discipline: String, title: String, source: String, sp: &BuildingSpec) -> SheetLink {
    let level = levels::level_of_title(&title).map(|c| sp.level_matching(&c).and_then(|i| sp.levels.get(i)).map_or(c, |l| l.name.clone()));
    SheetLink { number, title: title.chars().take(120).collect(), discipline: Some(discipline), level, source }
}

/// A layout's sheet: the sheet number written largest on it (else in the layout's name), and the
/// longest title-like text naming a plan, schedule or level.
fn layout_sheet(l: &cadaddy_doc::Layout, sp: &BuildingSpec) -> Option<SheetLink> {
    let ents: Vec<&Arc<Entity>> = l.entities.iter().take(MAX_TEXTS).collect();
    let ts = texts(&ents);
    let number = ts
        .iter()
        .filter_map(|(_, t, _, h)| levels::parse_sheet_number(t).filter(|_| t.split_whitespace().count() <= 2).map(|n| (n, *h)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, _)| n)
        .or_else(|| levels::parse_sheet_number(&l.name))?;
    let title = ts
        .iter()
        .map(|(_, t, _, _)| t)
        .filter(|t| {
            let u = t.to_ascii_uppercase();
            t.len() <= 120
                && (u.contains("PLAN") || u.contains("SCHEDULE") || u.contains("SECTION") || u.contains("ELEVATION") || u.contains("DETAIL"))
        })
        .max_by_key(|t| (levels::level_of_title(t).is_some(), t.len()))
        .cloned()
        .unwrap_or_else(|| l.name.clone());
    Some(link(number.0, number.1, title, format!("layout:{}", l.name), sp))
}

fn run_sheets(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "levels.sheets")?;
    let d = s.doc()?;
    let mut found: Vec<SheetLink> = d.layouts.iter().filter_map(|l| layout_sheet(l, &sp)).collect();
    if let Some(folder) = str_param(p, "folder") {
        let entries = std::fs::read_dir(folder).map_err(|e| bad("levels.sheets", format!("{folder}: {e}")))?;
        let mut names: Vec<String> = entries
            .take(MAX_FILES)
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                let low = n.to_ascii_lowercase();
                [".dwg", ".dxf", ".pdf", ".cdy"].iter().any(|x| low.ends_with(x))
            })
            .collect();
        names.sort();
        for n in names {
            let stem = n.rsplit_once('.').map_or(n.as_str(), |x| x.0);
            let Some((number, disc)) = levels::parse_sheet_number(stem) else { continue };
            let title = stem.replacen(&number, "", 1).trim_matches(|c: char| c == ' ' || c == '-' || c == '_').replace('_', " ");
            found.push(link(number, disc, title, format!("file:{n}"), &sp));
        }
    }
    // One per number: the first found (layouts before files).
    let mut sheets: Vec<SheetLink> = Vec::new();
    for f in found.into_iter().take(levels::MAX_NOTES) {
        if !sheets.iter().any(|x| x.number.eq_ignore_ascii_case(&f.number)) {
            sheets.push(f);
        }
    }
    let unattached: Vec<&str> = sheets.iter().filter(|x| x.level.is_none()).map(|x| x.number.as_str()).collect();
    let apply = bool_or(p, "apply", false);
    let out = json!({ "sheets": serde_json::to_value(&sheets).unwrap_or(Value::Null), "unattached": unattached, "applied": apply });
    if apply {
        for f in sheets {
            match sp.sheets.iter_mut().find(|x| x.number.eq_ignore_ascii_case(&f.number)) {
                Some(x) => *x = f,
                None => sp.sheets.push(f),
            }
        }
        store(s, &sp)?;
    }
    Ok(out)
}

fn run_shafts(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "levels.shafts")?;
    let tol = match p.get("tolerance_ft") {
        None | Some(Value::Null) => 4.0,
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && *x > 0.0 && *x <= 100.0)
            .ok_or_else(|| bad("levels.shafts", "`tolerance_ft` must be 0 to 100 feet"))?,
    };
    let d = s.doc()?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let bases = cadaddy_walk3d::level_bases(d, &sp.to_rules());
    let ents = plan_objects(d);
    let mut marks = Vec::new();
    for (_, t, at, _) in texts(&ents).into_iter().take(MAX_TEXTS) {
        let Some((_, dir)) = levels::shaft_mark(&t) else { continue };
        let Some(level) = level_at(&sp.levels, at.x, at.y) else { continue };
        let base = bases.iter().find(|(n, _)| *n == level.name).map_or(Vec2::new(0.0, 0.0), |b| b.1);
        let b = (at - base) * (1.0 / upf);
        marks.push(ShaftMark { level: level.name.clone(), name: t, at: [b.x, b.y], dir });
    }
    let mut order: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    order.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    let order: Vec<String> = order.iter().map(|l| l.name.clone()).collect();
    let runs = levels::shaft_runs(&marks, &order, tol);
    let gaps: usize = runs.iter().map(|r| r.missing.len()).sum();
    let rows: Vec<Value> = runs.iter().map(|r| json!({ "name": r.name, "at": r.at, "levels": r.levels, "missing": r.missing })).collect();
    Ok(json!({ "runs": rows, "gaps": gaps }))
}

fn run_isolate(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "levels.isolate")?;
    let want = match str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => {
            let i = sp.level_matching(l).ok_or_else(|| bad("levels.isolate", format!("there is no level named {l}")))?;
            sp.levels.get(i).map(|x| x.name.clone())
        }
        None => None,
    };
    let d = s.doc()?;
    let mut hide: Vec<Handle> = Vec::new();
    let mut show: Vec<Handle> = Vec::new();
    for e in d.model.iter().take(MAX_TEXTS * 10) {
        let Some(l) = made_level(e) else { continue };
        match &want {
            Some(w) if !l.eq_ignore_ascii_case(w) => hide.push(e.handle),
            _ => {
                if d.is_hidden_object(e.handle) {
                    show.push(e.handle);
                }
            }
        }
    }
    let shown = show.len();
    if !show.is_empty() {
        let dm = s.doc_mut()?;
        for h in &show {
            dm.hidden_objects.remove(h);
        }
    }
    let hidden = super::selecttools::hide(s, hide)?;
    let mut out = json!({ "hidden": hidden, "shown": shown });
    if let Some(w) = want {
        out["level"] = json!(w);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &mut Session, t: &str, at: [f64; 2]) {
        s.execute("text", &json!({ "text": t, "at": at, "height": 6.0 })).unwrap();
    }

    fn two_levels(s: &mut Session) {
        two_levels_with(s, json!({}));
    }

    /// The three-level spec with `extra` top-level keys (`approved`, `guess`) merged in.
    fn two_levels_with(s: &mut Session, extra: Value) {
        let mut spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 10, "region": [0, 0, 1000, 1000]},
                {"name": "Level 2", "elevation_ft": 10, "height_ft": 10, "region": [2000, 0, 3000, 1000]},
                {"name": "Level 3", "elevation_ft": 20, "height_ft": 10, "region": [4000, 0, 5000, 1000]}
            ]
        });
        for (k, v) in extra.as_object().unwrap() {
            spec[k] = v.clone();
        }
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    fn stored_spec(s: &Session) -> BuildingSpec {
        current(s, "test").unwrap().0
    }

    #[test]
    fn levels_are_read_from_section_notes_and_markers() {
        let mut s = Session::new();
        two_levels_with(&mut s, json!({ "guess": { "levels/Level 2": "drafted: no level note found" } }));
        assert_eq!(stored_spec(&s).guess.get("levels/Level 2").map(String::as_str), Some("drafted: no level note found"));
        // A section: one-text notes, and a marker drawn as a name over its elevation.
        text(&mut s, "LEVEL 1 = 0'-0\"", [-500.0, 0.0]);
        text(&mut s, "LEVEL 2 = 14'-0\"", [-500.0, 168.0]);
        text(&mut s, "LEVEL 3", [-500.0, 336.0]);
        text(&mut s, "28'-0\"", [-500.0, 326.0]);
        text(&mut s, "T.O. ROOF EL. 42'-6\"", [-500.0, 510.0]);
        text(&mut s, "TOP OF PARAPET 46'-0\"", [-500.0, 552.0]);
        // A plan title is no marker.
        text(&mut s, "LEVEL 2 MECHANICAL PLAN", [2100.0, -50.0]);
        let r = s.execute("levels.read", &json!({ "apply": true })).unwrap();
        let names: Vec<&str> = r["levels"].as_array().unwrap().iter().filter_map(|l| l["name"].as_str()).collect();
        assert_eq!(names, ["LEVEL 1", "LEVEL 2", "LEVEL 3", "ROOF", "PARAPET"], "{r}");
        let l = s.execute("levels.list", &json!({})).unwrap();
        let rows: Vec<(String, f64, f64)> = l["levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["name"].as_str().unwrap().to_string(), x["elevation_ft"].as_f64().unwrap(), x["height_ft"].as_f64().unwrap()))
            .collect();
        assert_eq!(rows, [("L1".into(), 0.0, 14.0), ("Level 2".into(), 14.0, 14.0), ("Level 3".into(), 28.0, 14.5)]);
        let storeys: Vec<&str> = l["storeys"].as_array().unwrap().iter().filter_map(|x| x["name"].as_str()).collect();
        assert_eq!(storeys, ["L1", "Level 2", "Level 3", "ROOF"]);
        // Every applied value names the note it came from, in levels.read and levels.list.
        let read_src: Vec<&str> = r["levels"].as_array().unwrap().iter().filter_map(|x| x["source"].as_str()).collect();
        assert_eq!(
            read_src,
            [
                "model: LEVEL 1 = 0'-0\"",
                "model: LEVEL 2 = 14'-0\"",
                "model: LEVEL 3 / 28'-0\"",
                "model: T.O. ROOF EL. 42'-6\"",
                "model: TOP OF PARAPET 46'-0\""
            ],
            "{r}"
        );
        let list_src: Vec<&str> = l["levels"].as_array().unwrap().iter().map(|x| x["source"].as_str().unwrap()).collect();
        assert_eq!(list_src, ["model: LEVEL 1 = 0'-0\"", "model: LEVEL 2 = 14'-0\"", "model: LEVEL 3 / 28'-0\""], "{l}");
        assert!(list_src[1].contains("LEVEL 2 = 14'-0\""));
        let roof = l["datums"].as_array().unwrap().iter().find(|d| d["name"] == json!("ROOF")).unwrap();
        assert_eq!(roof["source"], json!("model: T.O. ROOF EL. 42'-6\""), "{l}");
        // The level read from the prints is no longer a guess.
        let sp = stored_spec(&s);
        assert_eq!(sp.guess.get("levels/Level 2"), None);
        assert_eq!(sp.level_source("Level 2"), Some("model: LEVEL 2 = 14'-0\""));
        // Undo takes it all back.
        s.execute("undo", &json!({})).unwrap();
        let l = s.execute("levels.list", &json!({})).unwrap();
        assert_eq!(l["levels"][1]["elevation_ft"], json!(10.0));
    }

    #[test]
    fn an_approved_level_keeps_its_elevation_when_the_prints_are_read() {
        let mut s = Session::new();
        two_levels_with(&mut s, json!({ "approved": ["levels/level 2"] }));
        text(&mut s, "LEVEL 2 = 14'-0\"", [-500.0, 168.0]);
        text(&mut s, "LEVEL 3 = 28'-0\"", [-500.0, 336.0]);
        // The preview already says what applying keeps.
        let r = s.execute("levels.read", &json!({})).unwrap();
        assert_eq!(r["conflicts"], json!(["Level 2: kept 10'-0\" (approved); prints say 14'-0\""]), "{r}");
        assert_eq!(r["levels"][0]["approved"], json!(true));
        let r = s.execute("levels.read", &json!({ "apply": true })).unwrap();
        let kept = "Level 2: kept 10'-0\" (approved); prints say 14'-0\"";
        assert_eq!(r["kept"], json!([kept]), "{r}");
        assert_eq!(r["conflicts"], json!([kept]), "{r}");
        assert!(r["changes"].as_array().unwrap().iter().any(|c| c == kept), "{r}");
        let l = s.execute("levels.list", &json!({})).unwrap();
        assert_eq!(l["levels"][1]["name"], json!("Level 2"));
        assert_eq!(l["levels"][1]["elevation_ft"], json!(10.0), "{l}");
        assert_eq!(l["levels"][1]["approved"], json!(true));
        assert_eq!(l["levels"][1]["source"], Value::Null);
        // The unapproved level took the print's value.
        assert_eq!(l["levels"][2]["elevation_ft"], json!(28.0));
        assert_eq!(l["levels"][2]["source"], json!("model: LEVEL 3 = 28'-0\""));
        let sp = stored_spec(&s);
        assert_eq!(sp.levels[1].elevation_ft, 10.0);
        // Re-reading again keeps it again and moves nothing.
        let r = s.execute("levels.read", &json!({ "apply": true })).unwrap();
        assert_eq!(r["kept"], json!([kept]));
        assert_eq!(stored_spec(&s).levels[1].elevation_ft, 10.0);
    }

    #[test]
    fn a_100_foot_datum_is_taken_off_and_conflicts_reported() {
        let mut s = Session::new();
        two_levels(&mut s);
        text(&mut s, "LEVEL 1 EL. 100'-0\"", [-500.0, 0.0]);
        text(&mut s, "LEVEL 2 EL. 113'-4\"", [-500.0, 160.0]);
        text(&mut s, "LEVEL 2 EL. 114'-0\"", [-900.0, 160.0]);
        text(&mut s, "LEVEL 2 EL. 113'-4\"", [-1300.0, 160.0]);
        let r = s.execute("levels.read", &json!({})).unwrap();
        assert_eq!(r["datum_ft"], json!(100.0));
        assert_eq!(r["levels"][1]["elevation_ft"].as_f64().map(|v| (v * 1000.0).round()), Some(13333.0));
        assert_eq!(r["levels"][1]["matches"], json!("Level 2"));
        assert_eq!(r["conflicts"].as_array().unwrap().len(), 1, "{r}");
        assert_eq!(r["applied"], json!(false));
    }

    #[test]
    fn levels_are_edited_and_renamed() {
        let mut s = Session::new();
        two_levels(&mut s);
        let r = s.execute("levels.set", &json!({ "name": "Level 2", "elevation_ft": 12, "shift_above": true, "rename": "LEVEL 2" })).unwrap();
        assert_eq!(r["level"], json!("LEVEL 2"));
        let l = s.execute("levels.list", &json!({})).unwrap();
        let e: Vec<f64> = l["levels"].as_array().unwrap().iter().map(|x| x["elevation_ft"].as_f64().unwrap()).collect();
        assert_eq!(e, [0.0, 12.0, 22.0]);
        assert_eq!(l["levels"][0]["height_ft"], json!(12.0));
        assert!(s.execute("levels.set", &json!({ "name": "nowhere", "elevation_ft": 1 })).is_err());
        assert!(s.execute("levels.set", &json!({ "name": "L1", "height_ft": -3 })).is_err());
        assert!(s.execute("levels.set", &json!({ "name": "L1", "elevation_ft": "x" })).is_err());
        assert!(s.execute("levels.set", &json!({})).is_err());
    }

    #[test]
    fn sheets_attach_to_their_levels() {
        let mut s = Session::new();
        two_levels(&mut s);
        let dir = std::env::temp_dir().join(format!("cadaddy-levels-sheets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "M-201 LEVEL 2 MECHANICAL PLAN.pdf",
            "P-101 FIRST FLOOR PLUMBING PLAN.dwg",
            "M-601 MECHANICAL SCHEDULES.pdf",
            "notes.txt",
            "Sample Floor Plan.dwg",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let r = s.execute("levels.sheets", &json!({ "folder": dir.to_string_lossy(), "apply": true })).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let got: Vec<(String, Value)> =
            r["sheets"].as_array().unwrap().iter().map(|x| (x["number"].as_str().unwrap().to_string(), x["level"].clone())).collect();
        assert_eq!(got, [("M-201".into(), json!("Level 2")), ("M-601".into(), Value::Null), ("P-101".into(), json!("L1"))], "{r}");
        let l = s.execute("levels.list", &json!({})).unwrap();
        assert_eq!(l["levels"][1]["sheets"], json!(["M-201"]));
        assert_eq!(l["unattached"], json!(["M-601"]));
        assert!(s.execute("levels.sheets", &json!({ "folder": "/no/such/folder/anywhere" })).is_err());
    }

    #[test]
    fn a_shaft_missing_on_a_level_between_its_ends_is_found() {
        let mut s = Session::new();
        two_levels(&mut s);
        // The same shaft at 20',10' on L1 and Level 3 (each level's base at its region corner
        // via its walls: none here, so base = 0 for all; the plans sit 2000 apart).
        for (x0, t) in [(0.0, "SHAFT"), (4000.0, "SHAFT"), (0.0, "ELEV. 1"), (2000.0, "ELEVATOR")] {
            let x = if t.contains("ELEV") { 600.0 } else { 240.0 };
            text(&mut s, t, [x0 + x, 120.0]);
        }
        let spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
        let mut spec = spec;
        spec["levels"][1]["base"] = json!([2000, 0]);
        spec["levels"][2]["base"] = json!([4000, 0]);
        spec["levels"][0]["base"] = json!([0, 0]);
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        let r = s.execute("levels.shafts", &json!({})).unwrap();
        assert_eq!(r["gaps"], json!(1), "{r}");
        let shaft = r["runs"].as_array().unwrap().iter().find(|x| x["name"] == json!("SHAFT")).unwrap();
        assert_eq!(shaft["missing"], json!(["Level 2"]));
        assert!(s.execute("levels.shafts", &json!({ "tolerance_ft": -1 })).is_err());
    }

    #[test]
    fn isolating_a_level_hides_the_other_storeys() {
        let mut s = Session::new();
        two_levels(&mut s);
        let mut made = Vec::new();
        for (i, lvl) in ["L1", "Level 2", "Level 3"].iter().enumerate() {
            let x = 100.0 * i as f64;
            s.execute("line", &json!({ "points": [[x, 0], [x, 10]] })).unwrap();
            let h = s.doc().unwrap().model.last().unwrap().handle;
            s.doc_mut()
                .unwrap()
                .modify_entity(h, |e| e.common.xdata = crate::cmd::wallgen::tag(cadaddy_walk3d::Part::Wall, lvl, Handle(0)).into())
                .unwrap();
            made.push(h);
        }
        let r = s.execute("levels.isolate", &json!({ "level": "l2" })).unwrap();
        assert_eq!((r["hidden"].as_u64(), r["level"].as_str()), (Some(2), Some("Level 2")), "{r}");
        let d = s.doc().unwrap();
        assert!(d.is_hidden_object(made[0]) && !d.is_hidden_object(made[1]) && d.is_hidden_object(made[2]));
        let r = s.execute("levels.isolate", &json!({})).unwrap();
        assert_eq!(r["shown"], json!(2));
        assert!(s.doc().unwrap().hidden_objects.is_empty());
        assert!(s.execute("levels.isolate", &json!({ "level": "Level 9" })).is_err());
    }

    #[test]
    fn ifc_gets_a_storey_for_every_level_and_the_roof() {
        let mut s = Session::new();
        two_levels(&mut s);
        text(&mut s, "T.O. ROOF EL. 30'-0\"", [-500.0, 360.0]);
        s.execute("levels.read", &json!({ "apply": true })).unwrap();
        // One floor slab built on L1.
        let mesh = cadaddy_doc::Mesh {
            vertices: vec![
                cadaddy_geom::Vec3::new(0.0, 0.0, 0.0),
                cadaddy_geom::Vec3::new(120.0, 0.0, 0.0),
                cadaddy_geom::Vec3::new(0.0, 120.0, 0.0),
            ],
            faces: vec![cadaddy_doc::MeshFace { v: vec![0, 1, 2], hidden: 0 }],
            ..Default::default()
        };
        let common =
            cadaddy_doc::Common { xdata: crate::cmd::wallgen::tag(cadaddy_walk3d::Part::Floor, "L1", Handle(0)).into(), ..Default::default() };
        s.doc_mut().unwrap().add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Mesh(mesh)).unwrap();
        let text = String::from_utf8(cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "levels.ifc").unwrap()).unwrap();
        let storeys: Vec<&str> = text.lines().filter(|l| l.contains("IFCBUILDINGSTOREY(")).collect();
        assert_eq!(storeys.len(), 4, "{storeys:?}");
        for n in ["'L1'", "'Level 2'", "'Level 3'", "'ROOF'"] {
            assert!(storeys.iter().any(|l| l.contains(n)), "{n}: {storeys:?}");
        }
    }
}
