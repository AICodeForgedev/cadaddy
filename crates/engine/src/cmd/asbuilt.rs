//! As-built: field changes back into the model (Q311 / Q5110), as commands and the MCP `asbuilt`
//! tool, on [`cadaddy_mepfix::asbuilt`]:
//!
//! - `asbuilt.record {kind, element | near, …}`: one field change (moved, rerouted, type, size,
//!   height, added, deleted, verified, note) with who, when, why and its status; `kind: stud`
//!   enters a measured stud through `framing.field_point` and records it on the wall.
//! - `asbuilt.import_markups {pdf, page?, sheet?}`: the review markups on a printed sheet (clouds,
//!   arrows, callouts…) mapped to the elements they mark and read as changes. A preview first;
//!   with `confirm: true` they are kept as `pending` changes (nothing pending changes the model).
//! - `asbuilt.list`, `asbuilt.set_status`, `asbuilt.compare`, `asbuilt.export`: the record paged,
//!   reviewed, compared with the design, and written as as-built sheets (DXF per level) and an
//!   as-built IFC (`CaDaddy_AsBuilt` property sets on the changed elements).
//!
//! The record is kept in the drawing (embedded file `asbuilt.json`), so it travels with the job's
//! file; the design model it is mapped against is the print set's `truth.json` (`model`: the file
//! or its folder), remembered beside it (`asbuilt-model.json`).

use std::sync::Arc;

use cadaddy_mepfix::asbuilt::{self as ab, AsBuiltChange, AsBuiltRecord, ChangeKind, ChangeStatus, MappedMarkup};
use cadaddy_mepfix::truth::Truth;
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const RECORD: &str = "asbuilt.record";
const IMPORT: &str = "asbuilt.import_markups";
const LIST: &str = "asbuilt.list";
const STATUS: &str = "asbuilt.set_status";
const COMPARE: &str = "asbuilt.compare";
const EXPORT: &str = "asbuilt.export";

/// The embedded file the drawing keeps its as-built record in.
pub const RECORD_FILE: &str = "asbuilt.json";
/// The embedded file naming the design model (`{"model": path}`).
pub const MODEL_FILE: &str = "asbuilt-model.json";
/// Changes and deviations per page: by default and at most.
const PAGE: usize = 50;
const PAGE_MAX: usize = 500;
/// The longest text a list answer shows of a change's reason or notes.
const SHOW_TEXT: usize = 200;
/// Proposed changes an import preview lists, at most (the counts cover all).
const PREVIEW_MAX: usize = 200;
/// Ids one `asbuilt.set_status` takes, at most.
const MAX_IDS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(RECORD, "Record As-Built Change", run_record)
            .params("{kind: move|reroute|type|size|height|add|delete|verify|note|stud, element?: id | near?: [x, y] with level (the nearest element, preferring `element_kind`), element_kind?, to?: [x, y, z?] (move, building feet), path?: [[x, y, z], [x, y, z]] (reroute), type?, size?, height_ft? (above its floor), at?: [x, y, z?] (verify, add, note), status?: pending|field_decision|rfi_answer|change_order (default pending), author?, date?, reason?, notes?, level?, model?: truth.json or its folder; stud: {wall, centre_in | tape: {from: a|b, to_near_edge_in} | photo} (framing.build first)} → {change, total}: one field change kept in the drawing")
            .files(),
        CommandSpec::new(IMPORT, "Import As-Built Markups", run_import)
            .params("{pdf: a printed sheet with review markups, page?: 1, sheet?: number (default: the sheet whose PDF the file is), model?, confirm?: bool (default false: a preview, nothing kept)} → {preview, sheet, level, markups, proposed: [{markup, kind, text, element, element_kind, change, needs?}], unmapped, already, added?}: clouds, arrows, callouts and notes mapped to the elements they mark and read as changes; with confirm they are kept as pending changes")
            .files(),
        CommandSpec::new(LIST, "List As-Built Changes", run_list)
            .params("{status?, level?, kind?: moved|rerouted|type_changed|size_changed|added|deleted|field_verified|height_changed|note, element?, offset?, limit? (default 50, at most 500)} → {total, offset, changes: [{id, element, element_kind, kind, status, level, at, author, date, reason, notes?, markup?, detail}], next_offset?, summary}")
            .noundo(),
        CommandSpec::new(STATUS, "Set As-Built Change Status", run_status)
            .params("{id? | ids?: [id], status: pending|field_decision|rfi_answer|change_order|rejected, reason?: appended, author?} → {updated: [id], unchanged: [id], missing: [id]}: a reviewed change (field decision, RFI answer, change order) goes into the as-built model; rejected ones never"),
        CommandSpec::new(COMPARE, "Compare Design and As-Built", run_compare)
            .params("{include_pending?: bool (default false), level?, offset?, limit?, model?} → {include_pending, total, deviations: [{element, element_kind, description, level, at, magnitude, change}], pending_not_applied, summary, next_offset?}: what the as-built model differs from the design by")
            .noundo()
            .files(),
        CommandSpec::new(EXPORT, "Export As-Built", run_export)
            .params("{dir: folder to write, ifc?: bool (default true), include_pending?: bool (default false), model?} → {sheets: [{level, path, marks}], ifc?: {path, elements, changed, not_written}}: an as-built sheet per level (DXF: the design, one mark per change on ASBUILT-CHANGES, labels, status legend) and the as-built model as IFC with CaDaddy_AsBuilt property sets")
            .noundo()
            .files(),
    ]
}

// ---- What the drawing keeps --------------------------------------------------------------------

/// The drawing's as-built record (a new one named after the drawing when it has none).
fn stored(s: &Session, cmd: &str) -> Result<AsBuiltRecord> {
    let d = s.doc()?;
    match d.embedded.get(RECORD_FILE) {
        Some(b) => serde_json::from_slice(b).map_err(|e| bad(cmd, format!("the drawing's as-built record is unreadable: {e}"))),
        None => Ok(AsBuiltRecord::new("Project")),
    }
}

fn store(s: &mut Session, cmd: &str, rec: &AsBuiltRecord) -> Result<()> {
    let text = serde_json::to_vec(rec).map_err(|e| bad(cmd, e.to_string()))?;
    s.doc_mut()?.embedded.insert(RECORD_FILE.into(), Arc::new(text));
    Ok(())
}

/// The design model: `model` (a truth.json or the folder holding it), else the one named
/// before. A given model is remembered.
fn model(s: &mut Session, cmd: &str, p: &Value) -> Result<Truth> {
    let given = str_param(p, "model").map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
    let path = match &given {
        Some(m) => m.clone(),
        None => s
            .doc()?
            .embedded
            .get(MODEL_FILE)
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .and_then(|v| v.get("model").and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| bad(cmd, "no design model yet: give `model`, the print set's truth.json (or the folder holding it)"))?,
    };
    let file = if std::path::Path::new(&path).is_dir() { format!("{}/truth.json", path.trim_end_matches(['/', '\\'])) } else { path.clone() };
    let bytes =
        crate::readfile::read(&file).map_err(|e| bad(cmd, format!("{file}: {e} (give `model`: the print set's truth.json or its folder)")))?;
    let t: Truth = serde_json::from_slice(&bytes).map_err(|e| bad(cmd, format!("{file} is not a print set's truth.json: {e}")))?;
    if given.is_some() {
        let text = serde_json::to_vec(&json!({ "model": path })).map_err(|e| bad(cmd, e.to_string()))?;
        s.doc_mut()?.embedded.insert(MODEL_FILE.into(), Arc::new(text));
    }
    Ok(t)
}

// ---- Shared helpers ----------------------------------------------------------------------------

/// Parameter `key` as text, trimmed, at most `max` bytes (cut on a character boundary).
fn text(p: &Value, key: &str, max: usize) -> String {
    let t = str_param(p, key).map(str::trim).unwrap_or_default();
    let mut end = t.len().min(max);
    while end > 0 && !t.is_char_boundary(end) {
        end -= 1;
    }
    t.get(..end).unwrap_or_default().to_string()
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn pt2(v: &Value) -> Option<[f64; 2]> {
    Some([num(v.get(0)?)?, num(v.get(1)?)?])
}

fn pt3(v: &Value, z: f64) -> Option<[f64; 3]> {
    Some([num(v.get(0)?)?, num(v.get(1)?)?, v.get(2).and_then(num).unwrap_or(z)])
}

fn parse_status(cmd: &str, s: &str) -> Result<ChangeStatus> {
    match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "pending" => Ok(ChangeStatus::Pending),
        "field_decision" | "field" => Ok(ChangeStatus::FieldDecision),
        "rfi_answer" | "rfi" => Ok(ChangeStatus::RfiAnswer),
        "change_order" | "co" => Ok(ChangeStatus::ChangeOrder),
        "rejected" => Ok(ChangeStatus::Rejected),
        other => Err(bad(
            cmd,
            format!("unknown status `{}`: pending, field_decision, rfi_answer, change_order or rejected", other.chars().take(40).collect::<String>()),
        )),
    }
}

fn status_name(s: ChangeStatus) -> &'static str {
    match s {
        ChangeStatus::Pending => "pending",
        ChangeStatus::FieldDecision => "field_decision",
        ChangeStatus::RfiAnswer => "rfi_answer",
        ChangeStatus::ChangeOrder => "change_order",
        ChangeStatus::Rejected => "rejected",
    }
}

fn kind_name(k: &ChangeKind) -> &'static str {
    match k {
        ChangeKind::Moved { .. } => "moved",
        ChangeKind::Rerouted { .. } => "rerouted",
        ChangeKind::TypeChanged { .. } => "type_changed",
        ChangeKind::SizeChanged { .. } => "size_changed",
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::FieldVerified { .. } => "field_verified",
        ChangeKind::HeightChanged { .. } => "height_changed",
        ChangeKind::Note => "note",
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// What a change says, in a few words.
fn detail(k: &ChangeKind) -> String {
    match k {
        ChangeKind::Moved { from_ft, to_ft } => {
            format!(
                "moved {:.2}' from ({:.2}, {:.2}) to ({:.2}, {:.2})",
                (to_ft[0] - from_ft[0]).hypot(to_ft[1] - from_ft[1]),
                from_ft[0],
                from_ft[1],
                to_ft[0],
                to_ft[1]
            )
        }
        ChangeKind::Rerouted { asbuilt_path_ft: p, .. } => format!("rerouted ({:.2}, {:.2}) to ({:.2}, {:.2})", p[0][0], p[0][1], p[1][0], p[1][1]),
        ChangeKind::TypeChanged { design_type, asbuilt_type } => {
            format!("type {design_type} -> {}", asbuilt_type.chars().take(60).collect::<String>())
        }
        ChangeKind::SizeChanged { design_size, asbuilt_size } => format!("size {design_size} -> {asbuilt_size}"),
        ChangeKind::Added => "added in the field".into(),
        ChangeKind::Deleted => "not built".into(),
        ChangeKind::FieldVerified { at_ft } => format!("verified at ({:.2}, {:.2})", at_ft[0], at_ft[1]),
        ChangeKind::HeightChanged { design_ft, asbuilt_ft } => format!("height {design_ft:.2}' -> {asbuilt_ft:.2}'"),
        ChangeKind::Note => "note".into(),
    }
}

/// A change as list answers show it (texts shortened).
fn compact(c: &AsBuiltChange) -> Value {
    let short = |t: &str| t.chars().take(SHOW_TEXT).collect::<String>();
    let mut v = json!({
        "id": c.id, "element": c.element_id, "element_kind": c.element_kind, "kind": kind_name(&c.kind), "status": status_name(c.status),
        "level": c.level, "at": [r2(c.at_ft[0]), r2(c.at_ft[1])], "author": short(&c.author), "date": c.date, "reason": short(&c.reason),
        "detail": detail(&c.kind),
    });
    if !c.notes.is_empty() {
        v["notes"] = json!(short(&c.notes));
    }
    if let Some(m) = &c.markup_id {
        v["markup"] = json!(m);
    }
    v
}

/// The next free change id (`AB-0001`…).
fn next_id(rec: &AsBuiltRecord) -> String {
    let n = rec.changes.iter().filter_map(|c| c.id.strip_prefix("AB-").and_then(|n| n.parse::<usize>().ok())).max().unwrap_or(0);
    format!("AB-{:04}", n.saturating_add(1))
}

fn page(p: &Value) -> (usize, usize) {
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |v| usize::try_from(v).unwrap_or(PAGE_MAX)).clamp(1, PAGE_MAX);
    (offset, limit)
}

fn summary(rec: &AsBuiltRecord) -> Value {
    serde_json::to_value(ab::summarize(rec)).unwrap_or(Value::Null)
}

// ---- asbuilt.record ----------------------------------------------------------------------------

fn run_record(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = text(p, "kind", 40).to_ascii_lowercase();
    if kind.is_empty() {
        return Err(bad(RECORD, "`kind` is required: move, reroute, type, size, height, add, delete, verify, note or stud"));
    }
    let status = match str_param(p, "status") {
        Some(v) => parse_status(RECORD, v)?,
        None => ChangeStatus::Pending,
    };
    let rec = stored(s, RECORD)?;
    let id = next_id(&rec);
    let author = Some(text(p, "author", 120)).filter(|a| !a.is_empty()).unwrap_or_else(|| "user".into());
    let date = text(p, "date", 40);
    let reason = text(p, "reason", ab::MAX_TEXT);
    let notes = text(p, "notes", ab::MAX_TEXT);

    if kind == "stud" {
        // Through the job's framing: the wall's run snaps to field verified there too.
        let mut fp = if p.is_object() { p.clone() } else { json!({}) };
        fp["author"] = json!(author);
        let r = super::framing::run_field(s, &fp)?;
        let wall = r.get("wall").and_then(Value::as_str).unwrap_or_default().to_string();
        let at = r.get("at_ft").and_then(pt2).ok_or_else(|| bad(RECORD, "the framing gave no position for the stud"))?;
        let centre = r.get("centre_in").and_then(num).unwrap_or(0.0);
        let matches = r.get("matches").and_then(Value::as_str).map_or("neither layout".to_string(), |e| format!("laid out from the {e} end"));
        let change = AsBuiltChange {
            id,
            element_id: wall,
            element_kind: "wall".into(),
            kind: ChangeKind::FieldVerified { at_ft: [at[0], at[1], 0.0] },
            status,
            author,
            date,
            reason,
            notes: if notes.is_empty() { format!("stud centre {} from the a end: {matches}", cadaddy_mepfix::framing::inch(centre)) } else { notes },
            level: r.get("level").and_then(Value::as_str).unwrap_or_default().to_string(),
            at_ft: at,
            markup_id: None,
        };
        return keep(s, rec, change, json!({ "framing": r }));
    }

    let t = model(s, RECORD, p)?;
    let level_given = text(p, "level", 40);
    let element = match str_param(p, "element").map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => Some(e.to_string()),
        None => match p.get("near").and_then(pt2) {
            Some(at) => {
                if level_given.is_empty() {
                    return Err(bad(RECORD, "`near` needs `level` (the level code the point is on)"));
                }
                let near = ab::find_near_elements(&t, &level_given, at, 3.0);
                let want = text(p, "element_kind", 40);
                let pick = near.iter().find(|id| want.is_empty() || element_kind(&t, id) == want).or_else(|| near.first());
                Some(
                    pick.cloned()
                        .ok_or_else(|| bad(RECORD, format!("no element within 3' of ({:.2}, {:.2}) on level {level_given}", at[0], at[1])))?,
                )
            }
            None => None,
        },
    };
    if element.is_none() && kind != "add" {
        return Err(bad(RECORD, "give `element` (an element id of the design model) or `near` [x, y] with `level`"));
    }
    let eid = element.unwrap_or_default();
    let pos = if eid.is_empty() {
        None
    } else {
        Some(
            ab::element_position(&t, &eid)
                .ok_or_else(|| bad(RECORD, format!("no element `{}` in the design model", eid.chars().take(60).collect::<String>())))?,
        )
    };
    let ek = if eid.is_empty() { text(p, "element_kind", 40) } else { element_kind(&t, &eid) };
    let from = pos.unwrap_or([0.0; 3]);
    let ck = match kind.as_str() {
        "move" | "moved" => {
            let to = p
                .get("to")
                .and_then(|v| pt3(v, from[2]))
                .ok_or_else(|| bad(RECORD, "a move needs `to` [x, y, z?] (building feet; z kept when left out)"))?;
            ChangeKind::Moved { from_ft: from, to_ft: to }
        }
        "reroute" | "rerouted" => {
            let path = p
                .get("path")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 2)
                .ok_or_else(|| bad(RECORD, "a reroute needs `path` [[x, y, z], [x, y, z]] (the new run's ends)"))?;
            let a = path.first().and_then(|v| pt3(v, from[2])).ok_or_else(|| bad(RECORD, "`path` points are [x, y, z?]"))?;
            let b = path.get(1).and_then(|v| pt3(v, from[2])).ok_or_else(|| bad(RECORD, "`path` points are [x, y, z?]"))?;
            let design = design_path(&t, &eid).ok_or_else(|| bad(RECORD, "only duct, pipe and conduit runs are rerouted"))?;
            ChangeKind::Rerouted { design_path_ft: design, asbuilt_path_ft: [a, b] }
        }
        "type" => {
            let ty = text(p, "type", 120);
            if ty.is_empty() {
                return Err(bad(RECORD, "a type change needs `type` (what was installed)"));
            }
            ChangeKind::TypeChanged { design_type: design_type(&t, &eid), asbuilt_type: ty }
        }
        "size" => {
            let sz = text(p, "size", 60);
            if sz.is_empty() {
                return Err(bad(RECORD, "a size change needs `size` (what was installed)"));
            }
            ChangeKind::SizeChanged { design_size: design_size(&t, &eid), asbuilt_size: sz }
        }
        "height" => {
            let h = p.get("height_ft").and_then(num).ok_or_else(|| bad(RECORD, "a height change needs `height_ft` (above its floor)"))?;
            let design =
                design_height(&t, &eid).ok_or_else(|| bad(RECORD, "the model keeps no height for this element (duct and pipe runs, devices)"))?;
            ChangeKind::HeightChanged { design_ft: design, asbuilt_ft: h }
        }
        "add" => ChangeKind::Added,
        "delete" | "deleted" => ChangeKind::Deleted,
        "verify" | "verified" => ChangeKind::FieldVerified { at_ft: p.get("at").and_then(|v| pt3(v, from[2])).unwrap_or(from) },
        "note" => ChangeKind::Note,
        other => {
            return Err(bad(
                RECORD,
                format!(
                    "unknown kind `{}`: move, reroute, type, size, height, add, delete, verify, note or stud",
                    other.chars().take(40).collect::<String>()
                ),
            ));
        }
    };
    let at = p.get("at").and_then(pt2).or_else(|| match &ck {
        ChangeKind::Moved { to_ft, .. } => Some([to_ft[0], to_ft[1]]),
        _ => pos.map(|q| [q[0], q[1]]),
    });
    let Some(at) = at else { return Err(bad(RECORD, "an added element needs `at` [x, y] and `level`")) };
    let level = if !level_given.is_empty() { level_given } else { element_level(&t, &eid) };
    if level.is_empty() {
        return Err(bad(RECORD, "give `level` (the level code it is on)"));
    }
    let change =
        AsBuiltChange { id, element_id: eid, element_kind: ek, kind: ck, status, author, date, reason, notes, level, at_ft: at, markup_id: None };
    keep(s, rec, change, Value::Null)
}

fn keep(s: &mut Session, mut rec: AsBuiltRecord, change: AsBuiltChange, extra: Value) -> Result<Value> {
    if !rec.add_change(change.clone()) {
        return Err(bad(RECORD, "the record holds 10000 changes; that is the most it keeps"));
    }
    store(s, RECORD, &rec)?;
    let mut out = json!({ "change": compact(&change), "total": rec.changes.len() });
    if !extra.is_null() {
        out["framing"] = extra.get("framing").cloned().unwrap_or(Value::Null);
    }
    Ok(out)
}

fn element_kind(t: &Truth, id: &str) -> String {
    if t.segments.iter().any(|x| x.id == id) {
        "segment"
    } else if t.elec_devices.iter().any(|x| x.id == id) {
        "elec_device"
    } else if t.equipment.iter().any(|x| x.id == id) {
        "equipment"
    } else if t.elec_conduits.iter().any(|x| x.id == id) {
        "elec_conduit"
    } else if t.terminals.iter().any(|x| x.id == id) {
        "terminal"
    } else if t.fixtures.iter().any(|x| x.id == id) {
        "fixture"
    } else if t.plan_walls.iter().any(|x| x.id == id) {
        "wall"
    } else {
        "node"
    }
    .into()
}

fn element_level(t: &Truth, id: &str) -> String {
    t.segments
        .iter()
        .find(|x| x.id == id)
        .map(|x| &x.level)
        .or_else(|| t.elec_devices.iter().find(|x| x.id == id).map(|x| &x.level))
        .or_else(|| t.equipment.iter().find(|x| x.id == id).map(|x| &x.level))
        .or_else(|| t.elec_conduits.iter().find(|x| x.id == id).map(|x| &x.level))
        .or_else(|| t.terminals.iter().find(|x| x.id == id).map(|x| &x.level))
        .or_else(|| t.fixtures.iter().find(|x| x.id == id).map(|x| &x.level))
        .or_else(|| t.plan_walls.iter().find(|x| x.id == id).map(|x| &x.level))
        .or_else(|| t.nodes.iter().find(|x| x.id == id).map(|x| &x.level))
        .cloned()
        .unwrap_or_default()
}

fn design_path(t: &Truth, id: &str) -> Option<[[f64; 3]; 2]> {
    if let Some(sg) = t.segments.iter().find(|x| x.id == id) {
        return Some(sg.path_ft);
    }
    let c = t.elec_conduits.iter().find(|x| x.id == id)?;
    let z = ab::element_position(t, id).map_or(0.0, |q| q[2]);
    Some([[c.from_ft[0], c.from_ft[1], z], [c.to_ft[0], c.to_ft[1], z]])
}

fn design_type(t: &Truth, id: &str) -> String {
    t.elec_devices
        .iter()
        .find(|x| x.id == id)
        .map(|x| x.type_tag.clone())
        .or_else(|| t.terminals.iter().find(|x| x.id == id).map(|x| x.type_tag.clone()))
        .or_else(|| t.elec_conduits.iter().find(|x| x.id == id).map(|x| x.conduit_type.clone()))
        .or_else(|| t.plan_walls.iter().find(|x| x.id == id).map(|x| x.wall_type.clone()))
        .or_else(|| t.fixtures.iter().find(|x| x.id == id).map(|x| x.tag.clone()))
        .or_else(|| t.equipment.iter().find(|x| x.id == id).map(|x| x.tag.clone()))
        .unwrap_or_default()
}

fn design_size(t: &Truth, id: &str) -> String {
    if let Some(sg) = t.segments.iter().find(|x| x.id == id) {
        return if sg.size_in[0] == sg.size_in[1] { format!("{}", sg.size_in[0]) } else { format!("{}x{}", sg.size_in[0], sg.size_in[1]) };
    }
    t.elec_conduits.iter().find(|x| x.id == id).map(|x| x.size.clone()).unwrap_or_default()
}

fn design_height(t: &Truth, id: &str) -> Option<f64> {
    if let Some(sg) = t.segments.iter().find(|x| x.id == id) {
        return sg.height.as_ref().map(|h| h.centre_ft);
    }
    t.elec_devices.iter().find(|x| x.id == id).map(|d| d.mounting_in / 12.0)
}

// ---- asbuilt.import_markups --------------------------------------------------------------------

/// The same markup imported again: same sheet, kind and text, at the same place.
fn same_markup(a: &MappedMarkup, b: &MappedMarkup) -> bool {
    a.sheet == b.sheet
        && a.kind == b.kind
        && a.text == b.text
        && (a.building_ft[0] - b.building_ft[0]).abs() < 0.01
        && (a.building_ft[1] - b.building_ft[1]).abs() < 0.01
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "pdf")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad(IMPORT, "`pdf` (a printed sheet with review markups) is required"))?
        .to_string();
    let page_no = p.get("page").and_then(Value::as_u64).map_or(1, |v| u32::try_from(v).unwrap_or(u32::MAX)).max(1);
    let confirm = bool_or(p, "confirm", false);
    let t = model(s, IMPORT, p)?;
    let file_name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let sheet = match str_param(p, "sheet").map(str::trim).filter(|v| !v.is_empty()) {
        Some(n) => t.sheets.iter().find(|sh| sh.number.eq_ignore_ascii_case(n)),
        None => t.sheets.iter().find(|sh| sh.pdf.eq_ignore_ascii_case(&file_name) || sh.number.eq_ignore_ascii_case(&stem)),
    };
    let plans: Vec<&str> = t.sheets.iter().filter(|sh| sh.level.is_some()).map(|sh| sh.number.as_str()).take(40).collect();
    let sheet = sheet.ok_or_else(|| bad(IMPORT, format!("which sheet is it? give `sheet`, one of: {}", plans.join(", "))))?;
    if sheet.level.is_none() {
        return Err(bad(IMPORT, format!("{} is not a plan of a level; markups are mapped on plan sheets ({})", sheet.number, plans.join(", "))));
    }
    let bytes = crate::readfile::read(&path).map_err(|e| bad(IMPORT, format!("{path}: {e}")))?;
    let mut rec = stored(s, IMPORT)?;
    let first = rec.markups.iter().filter(|m| m.sheet == sheet.number).count().saturating_add(1);
    let mapped = ab::read_pdf_markups(&t, sheet, &bytes, page_no, first).map_err(|e| bad(IMPORT, e))?;
    let (already, new): (Vec<MappedMarkup>, Vec<MappedMarkup>) = mapped.into_iter().partition(|m| rec.markups.iter().any(|o| same_markup(o, m)));
    let mut proposed = Vec::new();
    let mut changes = Vec::new();
    let mut unmapped = Vec::new();
    let mut probe = rec.clone();
    for m in &new {
        let id = next_id(&probe);
        match ab::markup_to_change(m, &t, &id) {
            Some(c) => {
                let needs = c.notes.strip_prefix("needs value: ").map(str::to_string);
                if proposed.len() < PREVIEW_MAX {
                    let mut v = json!({
                        "markup": m.id, "kind": m.kind, "text": m.text.chars().take(SHOW_TEXT).collect::<String>(), "element": c.element_id,
                        "element_kind": c.element_kind, "change": kind_name(&c.kind), "detail": detail(&c.kind), "id": c.id,
                    });
                    if let Some(n) = needs {
                        v["needs"] = json!(n);
                    }
                    proposed.push(v);
                }
                probe.changes.push(c.clone());
                changes.push(c);
            }
            None => unmapped.push(json!({ "markup": m.id, "kind": m.kind, "text": m.text.chars().take(SHOW_TEXT).collect::<String>(), "at": [r2(m.building_ft[0]), r2(m.building_ft[1])] })),
        }
    }
    let mut out = json!({
        "preview": !confirm, "sheet": sheet.number, "level": sheet.level, "markups": new.len() + already.len(), "already": already.len(),
        "proposed": proposed, "changes": changes.len(), "unmapped": unmapped,
    });
    if confirm {
        let mut added = 0usize;
        for c in changes {
            if rec.add_change(c) {
                added += 1;
            }
        }
        rec.markups.extend(new);
        store(s, IMPORT, &rec)?;
        out["added"] = json!(added);
        out["message"] = json!("kept as pending changes: asbuilt.set_status puts reviewed ones into the as-built model");
    } else {
        out["message"] = json!("preview: nothing kept; run again with confirm: true to keep these as pending changes");
    }
    Ok(out)
}

// ---- asbuilt.list / set_status -----------------------------------------------------------------

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let rec = stored(s, LIST)?;
    let status = match str_param(p, "status") {
        Some(v) => Some(parse_status(LIST, v)?),
        None => None,
    };
    let level = str_param(p, "level").map(str::trim);
    let kind = str_param(p, "kind").map(str::trim);
    let element = str_param(p, "element").map(str::trim);
    let sel: Vec<&AsBuiltChange> = rec
        .changes
        .iter()
        .filter(|c| status.is_none_or(|st| c.status == st))
        .filter(|c| level.is_none_or(|l| c.level.eq_ignore_ascii_case(l)))
        .filter(|c| kind.is_none_or(|k| kind_name(&c.kind) == k))
        .filter(|c| element.is_none_or(|e| c.element_id == e))
        .collect();
    let (offset, limit) = page(p);
    let changes: Vec<Value> = sel.iter().skip(offset).take(limit).map(|c| compact(c)).collect();
    let mut out = json!({ "total": sel.len(), "offset": offset, "changes": changes, "summary": summary(&rec) });
    if offset.saturating_add(limit) < sel.len() {
        out["next_offset"] = json!(offset + limit);
    }
    Ok(out)
}

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    let status = parse_status(
        STATUS,
        str_param(p, "status").ok_or_else(|| bad(STATUS, "`status` is required: pending, field_decision, rfi_answer, change_order or rejected"))?,
    )?;
    let mut ids: Vec<String> = p
        .get("ids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).take(MAX_IDS).collect())
        .unwrap_or_default();
    if let Some(id) = str_param(p, "id") {
        ids.push(id.to_string());
    }
    if ids.is_empty() {
        return Err(bad(STATUS, "give `id` or `ids` (change ids from asbuilt.list)"));
    }
    let reason = text(p, "reason", ab::MAX_TEXT);
    let mut rec = stored(s, STATUS)?;
    let (mut updated, mut unchanged, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for id in &ids {
        match rec.changes.iter_mut().find(|c| c.id == *id) {
            Some(c) if c.status == status && reason.is_empty() => unchanged.push(id.clone()),
            Some(c) => {
                c.status = status;
                if !reason.is_empty() {
                    let joined = if c.reason.is_empty() { reason.clone() } else { format!("{}; {reason}", c.reason) };
                    c.reason = text(&json!({ "r": joined }), "r", ab::MAX_TEXT);
                }
                updated.push(id.clone());
            }
            None => missing.push(id.clone()),
        }
    }
    if !updated.is_empty() {
        store(s, STATUS, &rec)?;
    }
    Ok(json!({ "status": status_name(status), "updated": updated, "unchanged": unchanged, "missing": missing }))
}

// ---- asbuilt.compare / export ------------------------------------------------------------------

fn run_compare(s: &mut Session, p: &Value) -> Result<Value> {
    let include = bool_or(p, "include_pending", false);
    let rec = stored(s, COMPARE)?;
    let t = model(s, COMPARE, p)?;
    let level = str_param(p, "level").map(str::trim);
    let applied = AsBuiltRecord { changes: rec.changes.iter().filter(|c| ab::applies(c.status, include)).cloned().collect(), ..rec.clone() };
    let devs: Vec<ab::Deviation> =
        ab::compare_design_vs_asbuilt(&t, &applied).into_iter().filter(|d| level.is_none_or(|l| d.level.eq_ignore_ascii_case(l))).collect();
    let (offset, limit) = page(p);
    let list: Vec<Value> = devs
        .iter()
        .skip(offset)
        .take(limit)
        .map(|d| {
            json!({
                "element": d.element_id, "element_kind": d.element_kind, "description": d.description.chars().take(SHOW_TEXT).collect::<String>(),
                "level": d.level, "at": [r2(d.at_ft[0]), r2(d.at_ft[1])], "magnitude": r2(d.magnitude), "change": d.change_id,
            })
        })
        .collect();
    let pending = rec.changes.iter().filter(|c| c.status == ChangeStatus::Pending).count();
    let mut out = json!({
        "include_pending": include, "total": devs.len(), "offset": offset, "deviations": list,
        "pending_not_applied": if include { 0 } else { pending }, "summary": summary(&rec),
    });
    if offset.saturating_add(limit) < devs.len() {
        out["next_offset"] = json!(offset + limit);
    }
    Ok(out)
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = str_param(p, "dir")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad(EXPORT, "`dir` (the folder to write the sheets and IFC into) is required"))?
        .to_string();
    let include = bool_or(p, "include_pending", false);
    let rec = stored(s, EXPORT)?;
    let t = model(s, EXPORT, p)?;
    std::fs::create_dir_all(&dir).map_err(|e| bad(EXPORT, format!("{dir}: {e}")))?;
    let dir = dir.trim_end_matches(['/', '\\']).to_string();
    let mut sheets = Vec::new();
    for f in ab::asbuilt_sheet_files(&t, &rec) {
        let path = format!("{dir}/{}", f.name);
        crate::writefile::write(&path, &f.dxf).map_err(|e| bad(EXPORT, format!("{path}: {e}")))?;
        sheets.push(json!({ "level": f.level, "path": path, "marks": f.marks }));
    }
    let mut out = json!({ "sheets": sheets, "include_pending": include });
    if bool_or(p, "ifc", true) {
        let path = format!("{dir}/asbuilt.ifc");
        let (bytes, r) = ab::asbuilt_ifc(&t, &rec, include, &path).map_err(|e| bad(EXPORT, e))?;
        crate::writefile::write(&path, &bytes).map_err(|e| bad(EXPORT, format!("{path}: {e}")))?;
        out["ifc"] = json!({ "path": path, "elements": r.elements, "changed": r.changed.len(), "not_written": r.not_written });
    }
    Ok(out)
}
