//! 4D: the model on a schedule (`cadaddy-walk3d`'s [`Schedule4d`], kept in the drawing).
//! `fourd.import` reads a schedule (CSV/TSV or an Excel sheet: task, start, finish and an element
//! filter by level, system, class and name), `fourd.link` picks elements for a task by hand (or
//! takes them off, or changes its filter), `fourd.list` lists the tasks and what each covers,
//! `fourd.state` says where every element stands on a date (built, in progress, not yet) and
//! `fourd.export` renders the build sequence day by day to PNG frames, made into an MP4 with
//! ffmpeg as the camera-tour export does. The 3D view and Walk show a date from the 4D palette.
//!
//! The elements are model space's objects as IFC export sees them: a duct run is an `IfcDuctSegment` on
//! its storey with its system's name, a wall an `IfcWall`, plain linework an
//! `IfcBuildingElementProxy` named by its layer.

use std::collections::HashMap;

use cadaddy_doc::color::Color;
use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::fourd::{self, Counts, Element, Filter, IN_PROGRESS_RGB, Links, NOT_YET_RGB, Phase, Schedule4d, day, iso};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most handles listed per phase by `fourd.state`.
const MAX_LISTED: usize = 10_000;
/// Most frames `fourd.export` renders.
const MAX_FRAMES: usize = 3_000;
/// Largest frame side, pixels.
const MAX_SIDE: u32 = 8192;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("fourd.import", "Import 4D Schedule", run_import).menu(&["View", "4D Schedule", "Import Schedule..."]).params(
            "{path: CSV/TSV/XLSX file | rows: [[cell]] (the first row naming task, start and finish columns is the header; filter columns level, system, class, element, or one `filter` column `level=L1; system=Supply*`), sheet?: name (XLSX), merge?: bool (keep hand-picked elements of tasks with the same id; default true)} → {tasks, notes: [skipped rows], span: [start, finish], elements, linked, unscheduled, empty: [task ids covering nothing]}",
        ),
        CommandSpec::new("fourd.list", "4D Schedule Tasks", run_list)
            .params("{} → {tasks: [{id, name, start, finish, filter, add, remove, elements}], span, elements, unscheduled}"),
        CommandSpec::new("fourd.link", "Link 4D Task", run_link).params(
            "{task: id or name, handles?: [hex] (default the selection), remove?: bool (take them off the task instead), filter?: {level?, system?, class?, name?} | \"level=…; class=…\" (the task's rule, replaced), clear?: bool (drop its hand picks)} → {task, elements, added, removed}",
        ),
        CommandSpec::new("fourd.state", "4D State at a Date", run_state)
            .params("{date: e.g. 2026-03-09 (default the schedule's start), handles?: bool (list them; default true)} → {date, counts: {built, inProgress, notYet, unscheduled}, tasks: [{id, phase}], built?, inProgress?, notYet?: [hex]}"),
        CommandSpec::new("fourd.export", "Export 4D Video", run_export).params(
            "{folder: path for the frames, from?: date (schedule start), to?: date (schedule finish), step?: days per frame (1), fps?: 4, width?: 1280, height?: 720, view?: preset (\"swiso\"), style?: visual style (\"shaded\"), ghost?: bool (show not-yet work pale; default false: hidden), mp4?: path} → {frames, from, to, folder, mp4?: path | ffmpeg?: command to run}",
        ),
        CommandSpec::new("fourd.clear", "Clear 4D Schedule", run_clear).params("{} → {cleared: tasks}"),
    ]
}

/// The drawing's schedule.
pub fn load(d: &Drawing) -> std::result::Result<Schedule4d, String> {
    Schedule4d::from_json(d.schedule4d.as_deref())
}

/// Model space's objects as 4D elements.
pub fn elements(d: &Drawing) -> Vec<Element> {
    d.model.iter().map(|e| fourd::element_of(e)).collect()
}

/// Where each model element stands on `date` (days since 1970), by handle.
pub fn phases(d: &Drawing, sched: &Schedule4d, date: i32) -> (HashMap<Handle, Phase>, Counts) {
    let els = elements(d);
    let links = sched.links(&els);
    let counts = sched.counts(&links, date);
    let map = els.iter().zip(sched.phases(&links, date)).filter_map(|(e, p)| p.map(|p| (e.handle, p))).collect();
    (map, counts)
}

/// The drawing as it stands on `date`: not-yet elements left out (or pale with `ghost`),
/// in-progress ones orange, the rest as they are. What the 3D view and the export draw.
pub fn staged(d: &Drawing, date: i32, ghost: bool) -> std::result::Result<(Drawing, Counts), String> {
    let sched = load(d)?;
    let (map, counts) = phases(d, &sched, date);
    let mut out = d.clone();
    let rgb = |c: [u8; 3]| Color::True(cadaddy_color::Rgb(c[0], c[1], c[2]));
    for (h, p) in map {
        match p {
            Phase::Built => {}
            Phase::NotYet if !ghost => {
                out.remove_entity(h);
            }
            Phase::NotYet | Phase::InProgress => {
                let c = if p == Phase::InProgress { rgb(IN_PROGRESS_RGB) } else { rgb(NOT_YET_RGB) };
                let _ = out.modify_entity(h, |e| {
                    e.common.color = c;
                    e.common.material = None.into();
                });
            }
        }
    }
    Ok((out, counts))
}

fn sched(s: &Session, cmd: &str) -> Result<Schedule4d> {
    load(s.doc()?).map_err(|e| bad(cmd, e))
}

fn store(s: &mut Session, sc: &Schedule4d) -> Result<()> {
    s.doc_mut()?.schedule4d = sc.to_json();
    Ok(())
}

fn date_param(cmd: &str, p: &Value, key: &str) -> Result<Option<i32>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) => day(t).map(Some).ok_or_else(|| bad(cmd, format!("`{key}` `{t}` is not a date (2026-03-09)"))),
        Some(v) => day(&v.to_string()).map(Some).ok_or_else(|| bad(cmd, format!("`{key}` is not a date (2026-03-09)"))),
    }
}

fn summary(sc: &Schedule4d, els: &[Element], links: &Links) -> Value {
    json!({
        "span": sc.span().map(|(a, b)| json!([iso(a), iso(b)])),
        "elements": els.len(),
        "linked": els.len() - links.unlinked(),
        "unscheduled": links.unlinked(),
    })
}

/// The rows of a schedule file.
fn file_rows(cmd: &str, path: &str, sheet: Option<&str>) -> Result<Vec<Vec<String>>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        const MAX_BYTES: u64 = 64 << 20;
        let meta = std::fs::metadata(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        if meta.len() > MAX_BYTES {
            return Err(bad(cmd, format!("{path}: larger than {} MB", MAX_BYTES >> 20)));
        }
        let bytes = crate::readfile::read(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext == "xlsx" || ext == "xlsm" {
            let wb = super::table::xlsx::open(&bytes).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            return wb.cells(sheet, None).map_err(|e| bad(cmd, format!("{path}: {e}")));
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{feff}');
        Ok(super::table::breaks::parse_csv(text, super::table::breaks::separator(path, text)))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, sheet);
        Err(bad(cmd, "reading files is not available in the browser: pass `rows`"))
    }
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "fourd.import";
    let rows: Vec<Vec<String>> = match (p.get("rows").and_then(Value::as_array), str_param(p, "path")) {
        (Some(rows), _) => rows
            .iter()
            .take(fourd::MAX_TASKS + 50)
            .map(|r| {
                r.as_array()
                    .map(|cs| {
                        cs.iter()
                            .take(64)
                            .map(|c| c.as_str().map_or_else(|| if c.is_null() { String::new() } else { c.to_string() }, str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect(),
        (None, Some(path)) => file_rows(CMD, path, str_param(p, "sheet"))?,
        (None, None) => return Err(bad(CMD, "`path` or `rows` is required")),
    };
    let (mut new, notes) = Schedule4d::from_rows(&rows).map_err(|e| bad(CMD, e))?;
    if bool_or(p, "merge", true) {
        let old = sched(s, CMD).unwrap_or_default();
        for t in &mut new.tasks {
            if let Some(o) = old.tasks.iter().find(|o| o.id.eq_ignore_ascii_case(&t.id)) {
                t.add.clone_from(&o.add);
                t.remove.clone_from(&o.remove);
            }
        }
    }
    let els = elements(s.doc()?);
    let links = new.links(&els);
    let empty: Vec<&str> = new.tasks.iter().zip(&links.tasks).filter(|(_, l)| l.is_empty()).map(|(t, _)| t.id.as_str()).take(200).collect();
    let mut out = summary(&new, &els, &links);
    out["tasks"] = json!(new.tasks.len());
    out["notes"] = json!(notes.iter().take(200).collect::<Vec<_>>());
    out["empty"] = json!(empty);
    store(s, &new)?;
    Ok(out)
}

fn task_json(t: &fourd::Task, linked: usize) -> Value {
    json!({"id": t.id, "name": t.name, "start": t.start, "finish": t.finish, "filter": t.filter.text(), "add": t.add.len(), "remove": t.remove.len(), "elements": linked})
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let sc = sched(s, "fourd.list")?;
    let els = elements(s.doc()?);
    let links = sc.links(&els);
    let mut out = summary(&sc, &els, &links);
    out["tasks"] = sc.tasks.iter().zip(&links.tasks).map(|(t, l)| task_json(t, l.len())).collect();
    Ok(out)
}

fn run_link(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "fourd.link";
    let mut sc = sched(s, CMD)?;
    let key = str_param(p, "task").ok_or_else(|| bad(CMD, "`task` is required"))?;
    let ti = sc.find(key).ok_or_else(|| bad(CMD, format!("no task {key}")))?;
    let handles: Vec<Handle> = if p.get("handles").is_some() || p.get("filter").is_some() || bool_or(p, "clear", false) {
        p.get("handles")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter().filter_map(Value::as_str).filter_map(|h| u64::from_str_radix(h.trim(), 16).ok()).map(Handle).take(fourd::MAX_PICKS).collect()
            })
            .unwrap_or_default()
    } else {
        targets(s, p)?
    };
    let remove = bool_or(p, "remove", false);
    let Some(t) = sc.tasks.get_mut(ti) else { return Err(bad(CMD, format!("no task {key}"))) };
    match p.get("filter") {
        Some(Value::String(text)) => t.filter = Filter::parse(text).map_err(|e| bad(CMD, e))?,
        Some(v @ Value::Object(_)) => t.filter = serde_json::from_value(v.clone()).map_err(|e| bad(CMD, format!("`filter`: {e}")))?,
        _ => {}
    }
    if bool_or(p, "clear", false) {
        t.add.clear();
        t.remove.clear();
    }
    let (mut added, mut removed) = (0usize, 0usize);
    for h in handles {
        let hex = h.hex();
        let same = |x: &String| x.eq_ignore_ascii_case(&hex);
        if remove {
            t.add.retain(|x| !same(x));
            if !t.remove.iter().any(same) && t.remove.len() < fourd::MAX_PICKS {
                t.remove.push(hex);
                removed += 1;
            }
        } else {
            t.remove.retain(|x| !same(x));
            if !t.add.iter().any(same) && t.add.len() < fourd::MAX_PICKS {
                t.add.push(hex);
                added += 1;
            }
        }
    }
    let id = t.id.clone();
    let els = elements(s.doc()?);
    let n = sc.links(&els).tasks.get(ti).map_or(0, Vec::len);
    store(s, &sc)?;
    Ok(json!({"task": id, "elements": n, "added": added, "removed": removed}))
}

fn run_state(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "fourd.state";
    let sc = sched(s, CMD)?;
    let Some((first, _)) = sc.span() else { return Err(bad(CMD, "the drawing has no 4D schedule (fourd.import)")) };
    let d = date_param(CMD, p, "date")?.unwrap_or(first);
    let doc = s.doc()?;
    let els = elements(doc);
    let links = sc.links(&els);
    let ph = sc.phases(&links, d);
    let mut out = json!({
        "date": iso(d),
        "counts": sc.counts(&links, d),
        "tasks": sc.tasks.iter().map(|t| json!({"id": t.id, "phase": t.phase(d).name()})).collect::<Vec<_>>(),
    });
    if bool_or(p, "handles", true) {
        for (key, want) in [("built", Phase::Built), ("inProgress", Phase::InProgress), ("notYet", Phase::NotYet)] {
            out[key] = els.iter().zip(&ph).filter(|(_, p)| **p == Some(want)).map(|(e, _)| json!(e.handle.hex())).take(MAX_LISTED).collect();
        }
    }
    Ok(out)
}

fn run_clear(s: &mut Session, _p: &Value) -> Result<Value> {
    let n = sched(s, "fourd.clear").map(|sc| sc.tasks.len()).unwrap_or(0);
    s.doc_mut()?.schedule4d = None;
    Ok(json!({"cleared": n}))
}

/// The frame dates of an export: `from` to `to` every `step` days, the last day always included.
pub fn frame_days(from: i32, to: i32, step: i32) -> Vec<i32> {
    let step = step.max(1);
    let mut out: Vec<i32> = (0..MAX_FRAMES)
        .map_while(|i| i32::try_from(i).ok().and_then(|i| i.checked_mul(step)).and_then(|o| from.checked_add(o)).filter(|d| *d <= to))
        .collect();
    if out.last().is_some_and(|l| *l < to) && out.len() < MAX_FRAMES {
        out.push(to);
    }
    out
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "fourd.export";
    let sc = sched(s, CMD)?;
    let Some((first, last)) = sc.span() else { return Err(bad(CMD, "the drawing has no 4D schedule (fourd.import)")) };
    let from = date_param(CMD, p, "from")?.unwrap_or(first);
    let to = date_param(CMD, p, "to")?.unwrap_or(last);
    if to < from {
        return Err(bad(CMD, "`to` is before `from`"));
    }
    let step = p.get("step").and_then(Value::as_i64).unwrap_or(1);
    if !(1..=3650).contains(&step) {
        return Err(bad(CMD, "`step` is 1 to 3650 days"));
    }
    let fps = f64_or(p, "fps", 4.0);
    if !(0.1..=240.0).contains(&fps) {
        return Err(bad(CMD, "`fps` is between 0.1 and 240"));
    }
    let side = |k: &str, dflt: u32| -> Result<u32> {
        let v = p.get(k).and_then(Value::as_u64).unwrap_or(u64::from(dflt));
        u32::try_from(v).ok().filter(|v| (16..=MAX_SIDE).contains(v)).ok_or_else(|| bad(CMD, format!("`{k}` is 16 to {MAX_SIDE} pixels")))
    };
    let (w, h) = (side("width", 1280)?, side("height", 720)?);
    let view = str_param(p, "view").unwrap_or("swiso");
    let dir = cadaddy_geom::view3::preset(view).ok_or_else(|| bad(CMD, format!("unknown view {view} (swiso, seiso, front…)")))?;
    let proj = cadaddy_geom::view3::Projection::new(dir);
    let style_name = str_param(p, "style").unwrap_or("shaded");
    let style = cadaddy_render::shade::VisualStyle::parse(style_name).ok_or_else(|| bad(CMD, format!("unknown visual style {style_name}")))?;
    let ghost = bool_or(p, "ghost", false);
    let folder = str_param(p, "folder").ok_or_else(|| bad(CMD, "`folder` is required"))?;
    let days = frame_days(from, to, i32::try_from(step).unwrap_or(1));
    let doc = s.doc()?.clone();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = std::path::Path::new(folder);
        std::fs::create_dir_all(dir).map_err(|e| bad(CMD, format!("{folder}: {e}")))?;
        let bg = cadaddy_color::Rgb(255, 255, 255);
        for (i, d) in days.iter().enumerate() {
            let (st, _) = staged(&doc, *d, ghost).map_err(|e| bad(CMD, e))?;
            let png = cadaddy_render::shade::render_fitted(&st, proj, style, w, h, bg)
                .or_else(|| cadaddy_render::images::Raster::new(w, h, vec![255; (w as usize) * (h as usize) * 4]))
                .and_then(|r| r.encode_png())
                .ok_or_else(|| bad(CMD, "the frame could not be drawn"))?;
            let file = dir.join(cadaddy_walk3d::video::frame_name(i));
            std::fs::write(&file, png).map_err(|e| bad(CMD, format!("{}: {e}", file.display())))?;
        }
        let mut out = json!({"frames": days.len(), "from": iso(from), "to": iso(to), "folder": folder, "fps": fps, "width": w, "height": h});
        if let Some(mp4) = str_param(p, "mp4") {
            let mp4 = std::path::Path::new(mp4);
            match cadaddy_walk3d::video::encode_mp4(dir, fps, mp4) {
                Ok(true) => out["mp4"] = json!(mp4.display().to_string()),
                Ok(false) => out["ffmpeg"] = json!(cadaddy_walk3d::video::ffmpeg_command(dir, fps, mp4)),
                Err(e) => return Err(bad(CMD, e)),
            }
        }
        Ok(out)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (doc, days, proj, style, ghost, w, h);
        Err(bad(CMD, format!("{folder}: writing files is not available in the browser")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_days_cover_the_span() {
        assert_eq!(frame_days(10, 13, 1), vec![10, 11, 12, 13]);
        assert_eq!(frame_days(10, 15, 2), vec![10, 12, 14, 15], "the last day is always shown");
        assert_eq!(frame_days(10, 10, 7), vec![10]);
        assert_eq!(frame_days(0, i32::MAX, 1).len(), MAX_FRAMES);
        assert!(frame_days(5, 4, 1).is_empty());
    }

    #[test]
    fn a_spreadsheet_schedule_imports() {
        // Dates as day numbers, as Excel keeps them.
        let cells: Vec<Vec<String>> = [["Activity", "Begin", "End", "Element"], ["Cold water", "46092", "46093", "P-*"]]
            .iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect();
        let dir = std::env::temp_dir().join(format!("cadaddy_fourd_xlsx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let xlsx = dir.join("Sample Schedule.xlsx");
        std::fs::write(&xlsx, super::super::table::xlsx::write(&cells, "Schedule").unwrap()).unwrap();
        let mut s = Session::new();
        s.execute("layer.new", &json!({"name": "P-DOMW-CW", "current": true})).unwrap();
        s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
        let r = s.execute("fourd.import", &json!({"path": xlsx.display().to_string(), "sheet": "Schedule"})).unwrap();
        assert_eq!(r["span"], json!(["2026-03-11", "2026-03-12"]), "{r}");
        assert_eq!(r["linked"], 1, "the line, by its layer: {r}");
        assert!(s.execute("fourd.import", &json!({"path": xlsx.display().to_string(), "sheet": "Nope"})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
