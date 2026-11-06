//! `labels.read`: the panel directories' inputs read off the prints. Each `PANEL <tag> SCHEDULE`
//! (a TABLE, or text in rows and columns under a header with a CKT column) gives the panel's
//! circuits: number, description, room or area, breaker and poles. Each circuit printed beside a
//! device on the plans (`P1-5`, `P1-3,5`) gives a device and the circuit it is on. Both record
//! the sheet they came from; `apply` puts them in the building spec (`mep.equipment[].circuits`,
//! `mep.devices`), where `labels.list` makes the directories and device labels from them.

use std::collections::BTreeSet;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{ElecDevice, MepEquipment, PanelCircuit};
use serde_json::{Value, json};

use super::super::perceive::texts;
use super::super::*;
use crate::{Result, Session};

/// Most sheets read at once, objects looked at per sheet, and rows per schedule (hostile files).
const MAX_SHEETS: usize = 500;
const MAX_OBJECTS: usize = 4_000_000;
const MAX_ROWS: usize = 500;

/// What a panel schedule's header names.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Col {
    Circuit,
    Description,
    Room,
    Breaker,
    Poles,
}

fn column(h: &str) -> Option<Col> {
    let u = h.trim().to_ascii_uppercase();
    let words: Vec<&str> = u.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    Some(if has("CKT") || has("CIRCUIT") || u == "NO" || u == "NO." || u == "#" {
        Col::Circuit
    } else if has("POLES") || has("POLE") || u == "P" {
        Col::Poles
    } else if has("BKR") || has("BREAKER") || has("TRIP") || has("AMPS") || u == "A" {
        Col::Breaker
    } else if has("ROOM") || has("AREA") || has("LOCATION") {
        Col::Room
    } else if has("DESCRIPTION") || has("LOAD") || has("SERVES") {
        Col::Description
    } else {
        return None;
    })
}

/// The panel a schedule's title names: `PANEL P1 SCHEDULE`, `PANELBOARD P1 SCHEDULE`,
/// `PANEL SCHEDULE - P1`.
fn panel_of(title: &str) -> Option<String> {
    let u = title.trim().to_ascii_uppercase();
    let rest = u.strip_prefix("PANELBOARD ").or_else(|| u.strip_prefix("PANEL "))?.trim();
    let tag = match rest.strip_suffix("SCHEDULE") {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => rest.strip_prefix("SCHEDULE")?.trim_start_matches([' ', '-', ':', '\u{2013}']).trim(),
    };
    (!tag.is_empty() && tag.len() <= 16 && !tag.contains(char::is_whitespace)).then(|| tag.to_string())
}

/// A schedule read: the panel, its rows, and the texts it used.
struct Found {
    panel: String,
    rows: Vec<PanelCircuit>,
    used: Vec<Handle>,
}

fn row(cols: &[Option<Col>], cells: &[String], source: &str) -> Option<PanelCircuit> {
    let get = |c: Col| cols.iter().position(|x| *x == Some(c)).and_then(|i| cells.get(i)).map(|v| v.trim().to_string()).unwrap_or_default();
    let number = get(Col::Circuit);
    // A row is a circuit: its number is a number (a pole list's first).
    let first = number.split([',', '/', '-']).next().unwrap_or_default().trim();
    if first.is_empty() || first.len() > 4 || !first.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let poles = get(Col::Poles).trim_end_matches(['P', 'p']).trim().parse::<u32>().ok().filter(|p| (1..=3).contains(p)).unwrap_or(1);
    let dash = |v: String| if v == "-" { String::new() } else { v };
    Some(PanelCircuit {
        design_breaker: String::new(),
        number: first.to_string(),
        description: dash(get(Col::Description)),
        room: dash(get(Col::Room)),
        breaker: dash(get(Col::Breaker)),
        poles,
        protection: Vec::new(),
        wire: String::new(),
        load_va: None,
        source: source.to_string(),
    })
}

/// Panel schedules that are TABLE objects (a title row naming the panel, then the header).
fn tables(ents: &[&Arc<Entity>], source: &str) -> Vec<Found> {
    let mut out = Vec::new();
    for e in ents {
        let EntityKind::Table(t) = &e.kind else { continue };
        let rows: Vec<Vec<String>> = t.cells.iter().take(MAX_ROWS).map(|r| r.iter().map(|c| cadaddy_fonts::plain_text(&c.text)).collect()).collect();
        let Some(hi) = rows.iter().position(|r| r.iter().any(|c| column(c) == Some(Col::Circuit))) else { continue };
        let Some(panel) = rows.iter().take(hi).flatten().find_map(|c| panel_of(c)) else { continue };
        let cols: Vec<Option<Col>> = rows.get(hi).map(|r| r.iter().map(|c| column(c)).collect()).unwrap_or_default();
        let found = rows.iter().skip(hi + 1).filter_map(|r| row(&cols, r, source)).collect();
        out.push(Found { panel, rows: found, used: vec![e.handle] });
    }
    out
}

type Txt = (Handle, String, Vec2, f64);

/// Panel schedules drawn as text: a `PANEL … SCHEDULE` title, the header line under it, then
/// the rows, line by line, until a gap or the next title.
fn text_grids(all: &[Txt], source: &str) -> Vec<Found> {
    let mut out = Vec::new();
    for (th, title, tat, tht) in all {
        let Some(panel) = panel_of(title) else { continue };
        // The header: the nearest line below the title (within a few lines) naming a circuit column.
        let near: Vec<&Txt> = all
            .iter()
            .filter(|(h, t, p, _)| h != th && p.y < tat.y && p.y >= tat.y - 6.0 * tht && p.x >= tat.x - 2.0 * tht && column(t) == Some(Col::Circuit))
            .collect();
        let Some(&(_, _, at, ht)) = near.iter().max_by(|a, b| a.2.y.total_cmp(&b.2.y)).copied() else { continue };
        let tol = ht.max(1e-9) * 0.6;
        let mut header: Vec<(f64, Option<Col>, Handle)> = all
            .iter()
            .filter(|(_, _, p, _)| (p.y - at.y).abs() <= tol && p.x >= tat.x - 2.0 * tht && p.x <= at.x + 60.0 * ht)
            .map(|(hh, tt, p, _)| (p.x, column(tt), *hh))
            .collect();
        header.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (x0, x1) = (header.first().map_or(at.x, |c| c.0) - 2.0 * ht, header.last().map_or(at.x, |c| c.0) + 20.0 * ht);
        let mut below: Vec<&Txt> = all.iter().filter(|(_, _, p, _)| p.y < at.y - tol && p.x >= x0 && p.x <= x1).collect();
        below.sort_by(|a, b| b.2.y.total_cmp(&a.2.y).then(a.2.x.total_cmp(&b.2.x)));
        let mut lines: Vec<Vec<&Txt>> = Vec::new();
        let mut last_y = at.y;
        let mut step: Option<f64> = None;
        for t in below {
            match lines.last_mut() {
                Some(l) if l.first().is_some_and(|f| (f.2.y - t.2.y).abs() <= tol) => l.push(t),
                _ => {
                    let gap = last_y - t.2.y;
                    // The next schedule's title, a gap of more than a few lines, or enough rows.
                    if t.1.to_ascii_uppercase().contains("SCHEDULE")
                        || step.is_some_and(|s| gap > 2.5 * s)
                        || (step.is_none() && gap > 6.0 * ht)
                        || lines.len() >= MAX_ROWS
                    {
                        break;
                    }
                    step.get_or_insert(gap);
                    last_y = t.2.y;
                    lines.push(vec![t]);
                }
            }
        }
        // Each text under the column that starts at or before it.
        let cols: Vec<Option<Col>> = header.iter().map(|c| c.1).collect();
        let cells_of = |l: &Vec<&Txt>| {
            let mut cells = vec![String::new(); header.len()];
            for t in l {
                let i = header.iter().rposition(|c| c.0 <= t.2.x + ht).unwrap_or(0);
                if let Some(c) = cells.get_mut(i) {
                    if !c.is_empty() {
                        c.push(' ');
                    }
                    c.push_str(&t.1);
                }
            }
            cells
        };
        let rows = lines.iter().filter_map(|l| row(&cols, &cells_of(l), source)).collect();
        let mut used: Vec<Handle> = header.iter().map(|c| c.2).collect();
        used.push(*th);
        used.extend(lines.iter().flatten().map(|t| t.0));
        out.push(Found { panel, rows, used });
    }
    out
}

/// A device's circuit as printed beside it: `P1-5`, `P1-3,5,7` for a known panel.
fn circuit_ref(t: &str, panels: &BTreeSet<String>) -> Option<(String, String)> {
    let t = t.trim();
    if t.len() > 24 || t.contains(char::is_whitespace) {
        return None;
    }
    let (p, c) = t.rsplit_once('-')?;
    let panel = panels.iter().find(|x| x.eq_ignore_ascii_case(p))?;
    let poles: Vec<&str> = c.split(',').map(str::trim).collect();
    poles.iter().all(|x| !x.is_empty() && x.len() <= 4 && x.chars().all(|ch| ch.is_ascii_digit())).then(|| (panel.clone(), poles.join(",")))
}

/// One sheet to read: its objects by space, its name and its level.
struct Sheet {
    name: String,
    level: String,
    drawing: Drawing,
}

fn spaces(d: &Drawing) -> Vec<Vec<&Arc<Entity>>> {
    let mut v = vec![d.model.iter().take(MAX_OBJECTS).collect::<Vec<_>>()];
    v.extend(d.layouts.iter().map(|l| l.entities.iter().take(MAX_OBJECTS).collect::<Vec<_>>()).filter(|x| !x.is_empty()));
    v
}

fn sheets(s: &Session, p: &Value) -> Result<Vec<Sheet>> {
    const CMD: &str = "labels.read";
    let mut wanted: Vec<(String, Option<String>, String)> = Vec::new();
    let entry = |v: &Value| -> Option<(String, Option<String>, String)> {
        match v {
            Value::String(path) => Some((path.clone(), None, String::new())),
            Value::Object(o) => Some((
                o.get("path")?.as_str()?.to_string(),
                o.get("sheet").and_then(Value::as_str).map(str::to_string),
                o.get("level").and_then(Value::as_str).unwrap_or_default().to_string(),
            )),
            _ => None,
        }
    };
    if let Some(a) = p.get("paths").filter(|v| !v.is_null()) {
        let a = a.as_array().ok_or_else(|| bad(CMD, "`paths` is a list of sheets"))?;
        for v in a.iter().take(MAX_SHEETS + 1) {
            wanted.push(entry(v).ok_or_else(|| bad(CMD, "each of `paths` is a path or {path, sheet?, level?}"))?);
        }
        if wanted.len() > MAX_SHEETS {
            return Err(bad(CMD, format!("at most {MAX_SHEETS} sheets at once")));
        }
    } else if let Some(v) = p.get("path") {
        wanted.push(entry(v).ok_or_else(|| bad(CMD, "`path` is a path or {path, sheet?, level?}"))?);
    }
    if wanted.is_empty() {
        // The open drawing.
        let name = str_param(p, "sheet").map(str::trim).filter(|v| !v.is_empty()).unwrap_or("current drawing").to_string();
        let level = str_param(p, "level").unwrap_or_default().to_string();
        return Ok(vec![Sheet { name, level, drawing: s.doc()?.clone() }]);
    }
    let hooks = super::super::file::io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let mut out = Vec::new();
    for (path, sheet, level) in wanted {
        let bytes = crate::readfile::read(path.trim()).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let file = std::path::Path::new(path.trim()).file_name().and_then(|f| f.to_str()).unwrap_or("sheet.dxf").to_string();
        let drawing = (hooks.read)(&bytes, &file).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let stem = std::path::Path::new(&file).file_stem().and_then(|f| f.to_str()).unwrap_or_default().to_string();
        out.push(Sheet { name: sheet.filter(|v| !v.trim().is_empty()).unwrap_or(stem), level, drawing });
    }
    Ok(out)
}

pub(super) fn run_read(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = super::spec("labels.read", s)?;
    let read = sheets(s, p)?;
    // The schedules first: their panels name the circuits printed beside devices.
    let mut schedules: Vec<(String, Vec<PanelCircuit>)> = Vec::new();
    // Per sheet and space: its texts and the ones a schedule used (handles are per drawing).
    let mut by_sheet: Vec<Vec<(Vec<Txt>, BTreeSet<Handle>)>> = Vec::new();
    for sh in &read {
        let mut pools = Vec::new();
        for ents in spaces(&sh.drawing) {
            let mut used = BTreeSet::new();
            let all = texts(&ents);
            let mut found = tables(&ents, &sh.name);
            found.extend(text_grids(&all, &sh.name));
            for f in found {
                used.extend(f.used.iter().copied());
                match schedules.iter_mut().find(|(p, _)| p.eq_ignore_ascii_case(&f.panel)) {
                    // A panel scheduled twice: the later sheet's rows win circuit by circuit.
                    Some((_, rows)) => {
                        for r in f.rows {
                            rows.retain(|x| x.number != r.number);
                            rows.push(r);
                        }
                    }
                    None => schedules.push((f.panel, f.rows)),
                }
            }
            pools.push((all, used));
        }
        by_sheet.push(pools);
    }
    for (_, rows) in &mut schedules {
        rows.sort_by_key(|r| r.number.parse::<u32>().unwrap_or(u32::MAX));
    }
    let mut panels: BTreeSet<String> = schedules.iter().map(|(p, _)| p.clone()).collect();
    panels.extend(sp.mep.equipment.iter().filter(|e| super::super::gear::is_gear(e)).map(|e| e.tag.trim().to_string()));
    // Devices: each circuit printed on a plan (not in a schedule) is one.
    let mut devices: Vec<ElecDevice> = Vec::new();
    for (sh, pools) in read.iter().zip(&by_sheet) {
        let mut on_sheet: Vec<(Vec2, String, String)> = Vec::new();
        for (all, used) in pools {
            for (h, t, at, _) in all {
                if used.contains(h) {
                    continue;
                }
                if let Some((panel, circuit)) = circuit_ref(t, &panels) {
                    on_sheet.push((*at, panel, circuit));
                }
            }
        }
        on_sheet.sort_by(|a, b| b.0.y.total_cmp(&a.0.y).then(a.0.x.total_cmp(&b.0.x)));
        for (i, (_, panel, circuit)) in on_sheet.into_iter().enumerate() {
            devices.push(ElecDevice {
                tag: format!("{}-D{:03}", sh.name, i + 1),
                kind: "device".into(),
                level: sh.level.clone(),
                room: String::new(),
                panel,
                circuit,
                source: sh.name.clone(),
                ..ElecDevice::default()
            });
            if devices.len() >= super::MAX_LABELS {
                break;
            }
        }
    }
    let names: BTreeSet<&str> = read.iter().map(|x| x.name.as_str()).collect();
    let mut added = Vec::new();
    if bool_or(p, "apply", false) {
        for (panel, rows) in &schedules {
            match sp.mep.equipment.iter_mut().find(|e| e.tag.trim().eq_ignore_ascii_case(panel)) {
                Some(e) => e.circuits = rows.clone(),
                None => {
                    // A panel scheduled but not placed yet: its directory and labels still come.
                    sp.mep.equipment.push(MepEquipment {
                        tag: panel.clone(),
                        kind: "panel".into(),
                        title: format!("PANEL {panel} SCHEDULE"),
                        circuits: rows.clone(),
                        ..MepEquipment::default()
                    });
                    added.push(panel.clone());
                }
            }
        }
        // Reading a sheet again replaces what it gave; devices entered by hand stay.
        sp.mep.devices.retain(|d| !names.contains(d.source.as_str()));
        sp.mep.devices.extend(devices.iter().cloned());
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let sched: Vec<Value> = schedules
        .iter()
        .map(|(panel, rows)| {
            json!({"panel": panel, "circuits": rows.iter().map(|r| json!({"number": r.number, "description": r.description, "room": r.room, "breaker": r.breaker, "poles": r.poles, "source": r.source})).collect::<Vec<_>>()})
        })
        .collect();
    let dev: Vec<Value> =
        devices.iter().map(|d| json!({"tag": d.tag, "panel": d.panel, "circuit": d.circuit, "level": d.level, "source": d.source})).collect();
    Ok(json!({
        "sheets": names.into_iter().collect::<Vec<_>>(),
        "schedules": sched,
        "devices": dev,
        "added_panels": added,
        "applied": bool_or(p, "apply", false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_columns_and_refs() {
        assert_eq!(panel_of("PANEL P1 SCHEDULE").as_deref(), Some("P1"));
        assert_eq!(panel_of("Panelboard LA2 Schedule").as_deref(), Some("LA2"));
        assert_eq!(panel_of("PANEL SCHEDULE - H1").as_deref(), Some("H1"));
        assert_eq!(panel_of("PANEL SCHEDULE"), None);
        assert_eq!(panel_of("LIGHT FIXTURE SCHEDULE"), None);
        assert_eq!(column("CKT"), Some(Col::Circuit));
        assert_eq!(column("BKR"), Some(Col::Breaker));
        assert_eq!(column("POLES"), Some(Col::Poles));
        assert_eq!(column("ROOM/AREA"), Some(Col::Room));
        assert_eq!(column("LOAD DESCRIPTION"), Some(Col::Description));
        assert_eq!(column("TYPE"), None);
        let panels: BTreeSet<String> = ["P1".to_string(), "L-2".to_string()].into();
        assert_eq!(circuit_ref("P1-5", &panels), Some(("P1".into(), "5".into())));
        assert_eq!(circuit_ref("p1-3,5", &panels), Some(("P1".into(), "3,5".into())));
        assert_eq!(circuit_ref("L-2-12", &panels), Some(("L-2".into(), "12".into())));
        assert_eq!(circuit_ref("B-2", &panels), None);
        assert_eq!(circuit_ref("P1-X", &panels), None);
        assert_eq!(circuit_ref("P1-", &panels), None);
    }
}
