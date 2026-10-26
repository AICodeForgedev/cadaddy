//! `arch.read`: the architecture of the open job's prints ([`cadaddy_mepfix::arch_reader`]):
//! doors (mark, hand, the rooms either side, hinge and latch, schedule row, card reader),
//! windows, the stair and elevator, the roof, and rooms with their finishes and device plate
//! colours, read from the A floor plans, the roof plan and the door, hardware, window and finish
//! schedules. Every item says where it was read (its plan sheet; the schedule sheet of its row)
//! and how sure that is (`drawing`, `schedule`, or `guess` with why).
//!
//! The sheets come from, in order: the guided job's drawing (its A sheets side by side), the
//! open print set's DXF sheets (`printset.open`), DXF sheets kept in the drawing, else the
//! drawing itself as one sheet. Preview by default; `apply` merges the reading into the building
//! spec (rooms by level and number, schedule rows by mark, stairs and hoistways by name), as one
//! undo step, with who applied it in the spec's sources.

use cadaddy_mepfix::arch_reader::{self, ArchResult};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};
use std::collections::HashSet;

use super::*;
use crate::{Result, Session};

const CMD: &str = "arch.read";
/// Doors and rooms listed per answer by default, and at most.
const PAGE: usize = 50;
const PAGE_MAX: usize = 200;
/// Most sheets read, and the largest sheet file (hostile input).
const MAX_SHEETS: usize = 400;
const MAX_FILE: u64 = 256 * 1024 * 1024;
/// Misses listed per answer.
const MAX_MISSES: usize = 50;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Read Architecture from Prints", run)
            .params("{level?: level code or name (only that level), apply?: bool (default false: preview; true merges the reading into the building spec — rooms with finishes, base and plates; door/window rows; door tags with hand, swing rooms, hinge, latch and card reader; stairs; elevator hoistways — as one undo step), author?: who applies it (default \"user\"), offset?: first door and room listed (0), limit?: doors and rooms listed (50, max 200)} → {source: job drawing|print set|embedded sheets|current drawing, plans: [{sheet, level}], schedules: {doors, hardware, windows, finishes, legend, roof}, counts: {doors, marked, windows, stairs, elevators, rooms, rooms_with_finishes}, doors: [{mark, level, hand, swings_into, from_room, card_reader, width_in, source: {sheet, schedule?, status: drawing|schedule|guess, reason?}}], rooms: [{number, name, level, floor, base, wall, ceiling, plates, source}], stairs, elevators, misses: [text], misses_total, ignored, offset, next?, applied, warnings, message?}: reads the open job's A floor plans, roof plan and door/hardware/window/finish schedules (DXF sheets); with none, says what to load"),
    ]
}

/// The sheets to read: (`<file stem>.dxf`, DXF bytes), and where they came from.
fn gather(s: &Session) -> Result<(Vec<(String, Vec<u8>)>, &'static str)> {
    let d = s.doc()?;
    if let Some(job) = super::guided::load_job(s).ok().flatten().filter(|j| !j.sheet_regions.is_empty()) {
        let files: Vec<(String, Vec<u8>)> = super::guided::job_sheet_refs(&job)
            .iter()
            .filter(|r| r.discipline.eq_ignore_ascii_case("A"))
            .take(MAX_SHEETS)
            .map(|r| (format!("{}.dxf", r.number), arch_reader::dxf_bytes(&super::printmep::sheet_drawing(d, r))))
            .collect();
        if !files.is_empty() {
            return Ok((files, "job drawing"));
        }
    }
    if let Ok(pr) = super::printset::get_project_handle(s) {
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        for sh in pr.sheets.iter().filter(|x| x.superseded_by.is_empty()) {
            if files.len() >= MAX_SHEETS || !sh.file.to_ascii_lowercase().ends_with(".dxf") || !seen.insert(sh.file.clone()) {
                continue;
            }
            let path = std::path::Path::new(&sh.file);
            let path = if path.is_absolute() { path.to_path_buf() } else { std::path::Path::new(&pr.source_dir).join(path) };
            if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("sheet");
            files.push((format!("{stem}.dxf"), bytes));
        }
        return Ok((files, "print set"));
    }
    let kept: Vec<(String, Vec<u8>)> = d
        .embedded
        .iter()
        .filter(|(n, _)| n.to_ascii_lowercase().ends_with(".dxf"))
        .take(MAX_SHEETS)
        .map(|(n, b)| (n.rsplit('/').next().unwrap_or(n).to_string(), (**b).clone()))
        .collect();
    if !kept.is_empty() {
        return Ok((kept, "embedded sheets"));
    }
    let name = s.state().map(|st| st.title.trim_end_matches(".dxf").trim_end_matches(".dwg").to_string()).unwrap_or_default();
    Ok((vec![(format!("{}.dxf", if name.is_empty() { "sheet" } else { &name }), arch_reader::dxf_bytes(d))], "current drawing"))
}

/// Whether a level code the prints use is the level asked for (its code or its spec name).
fn level_is(code: &str, want: Option<&str>) -> bool {
    want.is_none_or(|w| code.eq_ignore_ascii_case(w) || super::printmep::level_name(code).eq_ignore_ascii_case(w))
}

/// Only what is on `level`.
fn on_level(mut r: ArchResult, level: Option<&str>) -> ArchResult {
    if level.is_none() {
        return r;
    }
    r.doors.retain(|d| level_is(&d.level, level));
    r.windows.retain(|w| level_is(&w.level, level));
    r.rooms.retain(|rm| level_is(&rm.level, level));
    r.stairs.retain(|st| st.levels.first().is_some_and(|l| level_is(l, level)));
    r.elevators.retain(|e| e.levels.first().is_some_and(|l| level_is(l, level)));
    r.plans.retain(|(_, l)| level_is(l, level));
    if !level.is_some_and(|l| level_is("R", Some(l))) {
        r.roof = None;
    }
    r
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Put `changes` into `items`: each replaces (merged into) the first item `same` says it is,
/// else is added.
fn merge_into(items: &mut Vec<Value>, changes: Vec<Value>, same: &dyn Fn(&Value, &Value) -> bool) {
    for c in changes {
        match items.iter_mut().find(|x| same(x, &c)) {
            Some(x) => cadaddy_walk3d::merge(x, &c),
            None => items.push(c),
        }
    }
}

fn text_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Merge the reading into the building spec. The new spec, and the warnings.
fn applied_spec(s: &Session, r: &ArchResult, author: &str) -> Result<(BuildingSpec, Vec<String>)> {
    let (sp, _) = super::bspec::current(s, CMD)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let name_of = |code: &str| sp.level(code).or_else(|| sp.level(&super::printmep::level_name(code))).map(|l| l.name.clone());
    let to_plan = |code: &str, p: [f64; 2]| {
        let l = sp.level(&name_of(code)?)?;
        let b = l.base.unwrap_or([0.0, 0.0]);
        Some([b[0] + p[0] * upf, b[1] + p[1] * upf])
    };
    let mut warnings = Vec::new();
    let mut missing: Vec<&str> = r.plans.iter().map(|(_, l)| l.as_str()).filter(|l| name_of(l).is_none()).collect();
    missing.dedup();
    for l in missing {
        warnings.push(format!(
            "level {l} isn't in the building spec, so its rooms, door and window tags, stairs and hoistways weren't applied: add it (levels.read or bspec.set)"
        ));
    }
    let mut patch = arch_reader::spec_patch_named(r, &to_plan, &|c| name_of(c).unwrap_or_else(|| c.to_string()));
    // A stair to a level the spec hasn't got climbs to the next one up instead.
    if let Some(stairs) = patch.get_mut("stairs").and_then(Value::as_array_mut) {
        for st in stairs.iter_mut() {
            if st.get("to_level").and_then(Value::as_str).is_some_and(|l| sp.level(l).is_none())
                && let Some(o) = st.as_object_mut()
            {
                o.remove("to_level");
            }
        }
    }
    let mut v = serde_json::to_value(&sp).map_err(|e| bad(CMD, e.to_string()))?;
    let take = |patch: &mut Value, k: &str| patch.get_mut(k).map(Value::take).and_then(|x| if let Value::Array(a) = x { Some(a) } else { None });
    let rooms = take(&mut patch, "rooms").unwrap_or_default();
    let (rows, tags) = match patch.get_mut("schedule") {
        Some(sc) => (take(sc, "rows").unwrap_or_default(), take(sc, "tags").unwrap_or_default()),
        None => (Vec::new(), Vec::new()),
    };
    let hoistways = take(&mut patch, "elevators").unwrap_or_default();
    if let Some(o) = patch.as_object_mut() {
        o.remove("rooms");
        o.remove("schedule");
        o.remove("elevators");
    }
    let same_room = |a: &Value, b: &Value| {
        text_of(a, "number").eq_ignore_ascii_case(text_of(b, "number"))
            && !text_of(b, "number").is_empty()
            && text_of(a, "level").eq_ignore_ascii_case(text_of(b, "level"))
    };
    let same_row =
        |a: &Value, b: &Value| text_of(a, "kind") == text_of(b, "kind") && text_of(a, "mark").trim().eq_ignore_ascii_case(text_of(b, "mark").trim());
    let marks: HashSet<String> = tags.iter().map(|t| text_of(t, "mark").trim().to_ascii_uppercase()).collect();
    {
        let Some(o) = v.as_object_mut() else { return Err(bad(CMD, "the building spec isn't an object")) };
        if let Some(Value::Array(items)) = o.get_mut("rooms") {
            merge_into(items, rooms, &same_room);
        } else {
            o.insert("rooms".into(), Value::Array(rooms));
        }
        let sc = o.entry("schedule").or_insert_with(|| json!({}));
        if !sc.is_object() {
            *sc = json!({});
        }
        let mut old_rows = sc.get_mut("rows").map(Value::take).and_then(|x| if let Value::Array(a) = x { Some(a) } else { None }).unwrap_or_default();
        merge_into(&mut old_rows, rows, &same_row);
        let mut old_tags = sc.get_mut("tags").map(Value::take).and_then(|x| if let Value::Array(a) = x { Some(a) } else { None }).unwrap_or_default();
        old_tags.retain(|t| !marks.contains(&text_of(t, "mark").trim().to_ascii_uppercase()));
        old_tags.extend(tags);
        sc["rows"] = Value::Array(old_rows);
        sc["tags"] = Value::Array(old_tags);
        // A hoistway read again replaces the one of its name (a keyed merge would add its
        // levels to the old ones).
        let mut old = o.get_mut("elevators").map(Value::take).and_then(|x| if let Value::Array(a) = x { Some(a) } else { None }).unwrap_or_default();
        old.retain(|e| !hoistways.iter().any(|h| text_of(h, "name").eq_ignore_ascii_case(text_of(e, "name"))));
        old.extend(hoistways);
        o.insert("elevators".into(), Value::Array(old));
    }
    cadaddy_walk3d::merge(&mut v, &patch);
    let plans: Vec<String> = r.plans.iter().map(|(sh, l)| format!("{sh} (level {l})")).collect();
    let sch = &r.sheets;
    let schedules: Vec<String> = [("doors", &sch.doors), ("hardware", &sch.hardware), ("windows", &sch.windows), ("finishes", &sch.finishes)]
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|s| format!("{k} {s}")))
        .collect();
    v["sources"]["arch.read"] = json!(format!("applied by {author}: plans {}; schedules {}", plans.join(", "), schedules.join(", ")));
    let new = BuildingSpec::from_json(&v.to_string()).map_err(|e| bad(CMD, e))?;
    Ok((new, warnings))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let apply = bool_or(p, "apply", false);
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).map(|l| l.chars().take(40).collect::<String>());
    let author: String = str_param(p, "author").map(str::trim).filter(|a| !a.is_empty()).unwrap_or("user").chars().take(80).collect();
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let limit = match p.get("limit") {
        None | Some(Value::Null) => PAGE,
        Some(v) => v
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(PAGE_MAX).clamp(1, PAGE_MAX))
            .ok_or_else(|| bad(CMD, "`limit` is a count of doors and rooms, 1 to 200"))?,
    };
    let (files, from) = gather(s)?;
    let (all, a_sheets) = arch_reader::read_sheets(&files);
    let r = on_level(all, level.as_deref());
    let sch = &r.sheets;
    let nothing = r.plans.is_empty() && sch.doors.is_none() && sch.windows.is_none() && sch.finishes.is_none() && r.roof.is_none();
    let mut out = json!({
        "source": from,
        "sheets_read": files.len(),
        "plans": r.plans.iter().map(|(sh, l)| json!({"sheet": sh, "level": l})).collect::<Vec<_>>(),
        "schedules": {"doors": sch.doors, "hardware": sch.hardware, "windows": sch.windows, "finishes": sch.finishes, "legend": sch.legend, "roof": sch.roof},
        "counts": {
            "doors": r.doors.len(),
            "marked": r.doors.iter().filter(|d| d.mark.is_some()).count(),
            "windows": r.windows.len(),
            "stairs": r.stairs.len(),
            "elevators": r.elevators.len(),
            "rooms": r.rooms.len(),
            "rooms_with_finishes": r.rooms.iter().filter(|rm| rm.floor.is_some()).count(),
        },
        "applied": false,
        "offset": offset,
    });
    if nothing {
        let seen = if a_sheets.is_empty() {
            "no architectural (A) sheet".to_string()
        } else {
            format!("A sheets {}, but none a floor plan, roof plan or schedule", a_sheets.join("; "))
        };
        out["message"] = json!(format!(
            "nothing to read in the {from} ({} sheet{}: {seen}). Load the job's architectural floor plans and door/window/finish schedules as DXF sheets: printset.open {{path: the folder}}, or start the guided job (job.start)",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ));
        out["doors"] = json!([]);
        out["rooms"] = json!([]);
        out["misses"] = json!([]);
        out["warnings"] = json!([]);
        return Ok(out);
    }
    let doors: Vec<Value> = r
        .doors
        .iter()
        .skip(offset)
        .take(limit)
        .map(|d| {
            json!({
                "mark": d.mark, "level": d.level, "hand": d.hand, "swings_into": d.swings_into, "from_room": d.from_room,
                "card_reader": d.card_reader, "width_in": r2(d.width_in), "source": d.source.to_json(),
            })
        })
        .collect();
    let rooms: Vec<Value> = r
        .rooms
        .iter()
        .skip(offset)
        .take(limit)
        .map(|rm| {
            json!({
                "number": rm.number, "name": rm.name, "level": rm.level, "floor": rm.floor, "base": rm.base, "wall": rm.wall,
                "ceiling": rm.ceiling, "plates": rm.plates, "source": rm.source.to_json(),
            })
        })
        .collect();
    let stairs: Vec<Value> = r
        .stairs
        .iter()
        .take(20)
        .map(|st| {
            json!({
                "tag": st.tag, "levels": st.levels, "width_in": r2(st.width_in), "risers": st.risers, "riser_in": st.riser_in.map(r2),
                "tread_in": r2(st.tread_in), "runs": st.runs.len(), "source": st.source.to_json(),
            })
        })
        .collect();
    let elevators: Vec<Value> = r
        .elevators
        .iter()
        .take(20)
        .map(|e| json!({"tag": e.tag, "levels": e.levels, "pit_ft": e.pit_ft.map(r2), "capacity_lb": e.capacity_lb, "source": e.source.to_json()}))
        .collect();
    let misses = r.misses();
    out["doors"] = json!(doors);
    out["rooms"] = json!(rooms);
    out["stairs"] = json!(stairs);
    out["elevators"] = json!(elevators);
    out["misses_total"] = json!(misses.len());
    out["misses"] = json!(misses.into_iter().take(MAX_MISSES).collect::<Vec<_>>());
    out["ignored"] = json!(r.ignored.iter().take(MAX_MISSES).collect::<Vec<_>>());
    if offset.saturating_add(limit) < r.doors.len().max(r.rooms.len()) {
        out["next"] = json!(offset + limit);
    }
    let mut warnings: Vec<String> = Vec::new();
    if apply {
        let (sp, w) = applied_spec(s, &r, &author)?;
        warnings.extend(w);
        let errors: Vec<String> =
            super::bspec::validate(s.doc()?, &sp).into_iter().filter(|i| i.error).map(|i| format!("{}: {}", i.path, i.message)).take(10).collect();
        if errors.is_empty() {
            let text = sp.to_json();
            // The same reading again leaves the drawing alone (no empty undo step).
            if s.doc()?.building_spec.as_deref() == Some(text.as_str()) {
                out["unchanged"] = json!(true);
            } else {
                s.doc_mut()?.building_spec = Some(text);
            }
            out["applied"] = json!(true);
            s.echo(format!(
                "Architecture applied to the building spec: {} rooms, {} doors, {} windows.",
                r.rooms.len(),
                r.doors.len(),
                r.windows.len()
            ));
        } else {
            out["errors"] = json!(errors);
            warnings.push("not applied: the merged building spec has errors (see errors); fix the spec (bspec.set) and apply again".into());
        }
    }
    out["warnings"] = json!(warnings);
    Ok(out)
}
