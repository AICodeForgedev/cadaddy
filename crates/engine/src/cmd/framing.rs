//! Framing from the prints (Q303 / Q5103): the job's walls and structure framed by
//! [`cadaddy_mepfix::framing`], reachable as commands (and the MCP `framing` tool):
//!
//! - `framing.build {dir? | paths?}`: read the job's DXF sheets (sheets → ceilings → structure →
//!   walls) and frame every wall: fixed members from the prints, both rule layouts, stacks. The
//!   sheets it read are kept in the job, and every field point and shop drawing entered earlier is
//!   re-applied, so a rebuild after a print revision keeps them.
//! - `framing.list`, `framing.wall`, `framing.next_stud`: compact, paged answers per wall:
//!   members by status and kind, both layouts, the "next stud" dimension, flags.
//! - `framing.field_point {wall, centre_in | tape | photo, author}`: a measured stud. Idempotent
//!   (the same entry again changes nothing), author-tagged and persisted in the job.
//! - `framing.shop_drawing {file, level, author}`: a shop drawing (DXF) replaces the rule members
//!   where it covers; persisted the same way.
//! - `framing.view {wall? | level?, walk?}`: the framing elevation of a wall, the framing plan of
//!   a level and the members as 3D boxes for the Walk view, on `FRAMING-*` layers (fixed solid,
//!   rule ghosted, dimension strings).
//!
//! The job keeps what was entered (`agent` JSON of the drawing, key `framing`); the framing itself
//! is rebuilt from the sheets and cached in the session.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::Vec3;
use cadaddy_mepfix::bore::{self, BitLine};
use cadaddy_mepfix::framing::{self as fr, End, Framing, Kind, Status, WallFrame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const BUILD: &str = "framing.build";
const LIST: &str = "framing.list";
const WALL: &str = "framing.wall";
const NEXT: &str = "framing.next_stud";
const FIELD: &str = "framing.field_point";
const SHOP: &str = "framing.shop_drawing";
const VIEW: &str = "framing.view";

/// The job's key in the drawing's agent JSON.
const KEY: &str = "framing";
/// The application name on view objects (xdata), so a view drawn again replaces the old one.
const APP: &str = "CADADDY_FRAMING";
/// Sheets read at once, at most.
const MAX_SHEETS: usize = 2_000;
/// One sheet file, at most (bytes).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const MAX_SHEET_BYTES: usize = 256 * 1024 * 1024;
/// Entries kept in the job, at most (each kind).
const MAX_ENTRIES: usize = 10_000;
/// Walls per page: by default and at most.
const PAGE: usize = 50;
const PAGE_MAX: usize = 500;
/// Members per page of one wall, at most.
const MEMBERS_MAX: usize = 1_000;
/// Two stud centres closer than this are the same entry, inches.
const SAME_IN: f64 = 1.0 / 32.0;
/// An entry's point is on a wall when this close to its centreline (beyond half its thickness), inches.
const ON_WALL_IN: f64 = 1.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(BUILD, "Frame Walls from Prints", run_build)
            .files()
            .params("{dir?: folder of DXF sheets, paths?: [DXF sheet] (none: the sheets framed last, else the open print set's DXF sheets), level?, offset?, limit?} → {sheets, walls, stud_walls, stacks, by_status: {fixed, agreed, rule, shop_drawing, field_verified}, rule_layouts: {a, b, rule_max}, flagged, reapplied: {field_points, shop_drawings}, stale: [entry], list: framing.list's answer}: frames every wall the prints show (fixed members, both rule layouts, stacks) and re-applies the field points and shop drawings entered before"),
        CommandSpec::new(LIST, "List Framed Walls", run_list)
            .params("{level?, flagged?: bool, settled?: bool, offset?, limit? (default 50, at most 500)} → {total, offset, walls: [{id, level, type, len, a_ft, b_ft, stud_in, spacing_in, settled, counts: {fixed, agreed, rule_a, rule_b, shop_drawing, field_verified}, flags}], next_offset?}")
            .noundo(),
        CommandSpec::new(WALL, "Framed Wall", run_wall)
            .params("{wall: id, members?: bool, offset?, limit?} → {id, level, type, len, a_ft, b_ft, settled, stack: [id], by_status: {status: {kind: n}}, layouts: {a: {field_studs, first}, b: {…}}, next_stud: {from_a, from_b}, flags, field_changes, members?: [{kind, status, layout?, s_in, width_in, z_in, note?}]}")
            .noundo(),
        CommandSpec::new(NEXT, "Next Stud", run_next)
            .params("{wall: id, from?: a|b} → {wall, from_a, from_b, settled, first_in: {a: [laid out from a, from b], b: […]}}: the \"next stud\" dimension from each end, under each layout")
            .noundo(),
        CommandSpec::new(FIELD, "Enter Field Stud", run_field)
            .params("{wall: id, centre_in?: inches from the a end | tape?: {from: a|b, to_near_edge_in} | photo?: {refs: [[x, y, inches from a] ×3], pick: [x, y]}, author?} → {changed, wall, centre_in, matches: a|b|null, off_in, walls: [id], settled, next_stud}: a measured stud; its run and the walls stacked with it snap to field verified. Kept in the job and re-applied on every rebuild; the same entry again changes nothing"),
        CommandSpec::new(SHOP, "Apply Shop Drawing", run_shop)
            .files()
            .params("{file: DXF, level, units_per_foot?: drawing units per foot (default 12), offset_ft?: [x, y] (added after scaling), author?} → {changed, file, level, studs, joists, tendons, walls_changed}: layers STUD, JOIST/TRUSS, TENDON/PT replace the rule members where they cover. Kept in the job and re-applied on every rebuild; the same file again changes nothing"),
        CommandSpec::new(VIEW, "Show Framing", run_view)
            .params("{wall?: id (its elevation), level? (its framing plan), walk?: bool (members as 3D boxes for the Walk view), at?: [x, y] (where the elevation goes; default left of the plan), units_per_foot? (default 12), elevation_ft?} → {layers, objects, texts: [dimension text], replaced}: on FRAMING-* layers: fixed solid, rule layouts ghosted (A dashed, B hidden), dimension strings; drawing it again replaces the earlier one"),
    ]
}

// ---- What the job keeps ------------------------------------------------------------------------

/// A measured stud, as entered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldEntry {
    pub wall: String,
    pub level: String,
    /// The stud centre in plan, building feet (finds the wall again after a rebuild).
    pub at_ft: [f64; 2],
    /// Inches from the wall's `a` end.
    pub centre_in: f64,
    /// `centre`, `tape` or `photo`, and what was given.
    pub how: String,
    pub input: Value,
    pub author: String,
}

/// A shop drawing, as applied: what it gave is kept, so a rebuild does not need the file again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShopEntry {
    pub file: String,
    pub level: String,
    pub hash: String,
    pub units_per_foot: f64,
    pub offset_ft: [f64; 2],
    pub author: String,
    pub drawing: fr::ShopDrawing,
}

/// The job's framing: the sheets framed and every entry, in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub sheets: Vec<String>,
    #[serde(default)]
    pub field_points: Vec<FieldEntry>,
    #[serde(default)]
    pub shop_drawings: Vec<ShopEntry>,
}

/// The session's framing: built from the sheets (`base`) and with the entries applied.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    sheets: Vec<String>,
    base: Framing,
    entries: String,
    applied: Framing,
    stale: Vec<Value>,
}

fn load(s: &Session, cmd: &str) -> Result<State> {
    let Some(text) = s.doc()?.agent.as_deref().filter(|t| !t.trim().is_empty()) else { return Ok(State::default()) };
    let agent: Value = serde_json::from_str(text).map_err(|e| bad(cmd, format!("the job's agent state is not JSON: {e}")))?;
    match agent.get(KEY) {
        Some(v) if !v.is_null() => serde_json::from_value(v.clone()).map_err(|e| bad(cmd, format!("the job's framing is unreadable: {e}"))),
        _ => Ok(State::default()),
    }
}

fn save(s: &mut Session, st: &State) -> Result<()> {
    let doc = s.doc_mut()?;
    let mut agent: Value = doc.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    agent[KEY] = serde_json::to_value(st).unwrap_or(Value::Null);
    doc.agent = Some(agent.to_string());
    Ok(())
}

/// FNV-1a, hex: a shop drawing's content.
fn hash(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

// ---- Reading the sheets ------------------------------------------------------------------------

fn is_dxf(p: &str) -> bool {
    p.to_ascii_lowercase().ends_with(".dxf")
}

/// The sheets to frame: `paths`, the DXF files in `dir`, the ones framed last, or the open print
/// set's DXF sheets.
fn sheet_paths(s: &Session, p: &Value, st: &State) -> Result<Vec<String>> {
    if let Some(a) = p.get("paths").filter(|v| !v.is_null()) {
        let a = a.as_array().ok_or_else(|| bad(BUILD, "`paths` is a list of DXF sheet files"))?;
        if a.len() > MAX_SHEETS {
            return Err(bad(BUILD, format!("at most {MAX_SHEETS} sheets at once")));
        }
        return a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(BUILD, "each of `paths` is a file path"))).collect();
    }
    if let Some(dir) = str_param(p, "dir").map(str::trim).filter(|d| !d.is_empty()) {
        return dxf_in(dir);
    }
    if !st.sheets.is_empty() {
        return Ok(st.sheets.clone());
    }
    if let Some(proj) = s.printset_project.as_deref() {
        let base = std::path::Path::new(&proj.source_dir);
        let mut out: Vec<String> =
            proj.sheets.iter().filter(|e| is_dxf(&e.file)).map(|e| base.join(&e.file).display().to_string()).take(MAX_SHEETS).collect();
        out.sort();
        out.dedup();
        if !out.is_empty() {
            return Ok(out);
        }
        return Err(bad(BUILD, "the open print set has no DXF sheets: pass `dir` or `paths` (DXF; convert PDF sheets first)"));
    }
    Err(bad(BUILD, "no sheets: pass `dir` (a folder of DXF sheets) or `paths`, or open the prints with printset.open first"))
}

#[cfg(not(target_arch = "wasm32"))]
fn dxf_in(dir: &str) -> Result<Vec<String>> {
    let rd = std::fs::read_dir(dir).map_err(|e| bad(BUILD, format!("{dir}: {e}")))?;
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_dxf(&p.display().to_string()))
        .map(|p| p.display().to_string())
        .collect();
    out.sort();
    if out.len() > MAX_SHEETS {
        return Err(bad(BUILD, format!("{dir} has {} DXF sheets; at most {MAX_SHEETS}", out.len())));
    }
    if out.is_empty() {
        return Err(bad(BUILD, format!("{dir} has no DXF sheets")));
    }
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
fn dxf_in(_dir: &str) -> Result<Vec<String>> {
    Err(bad(BUILD, "folders are not available on the web"))
}

fn read_bytes(cmd: &str, path: &str) -> Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let b = crate::readfile::read(path.trim()).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        if b.len() > MAX_SHEET_BYTES {
            return Err(bad(cmd, format!("{path} is larger than {} MB", MAX_SHEET_BYTES / (1024 * 1024))));
        }
        Ok(b)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(bad(cmd, "files are not available on the web"))
    }
}

fn read_sheets(paths: &[String]) -> Result<Framing> {
    let mut dxf: Vec<(String, Vec<u8>)> = Vec::new();
    for p in paths.iter().filter(|p| is_dxf(p)).take(MAX_SHEETS) {
        let name = std::path::Path::new(p.trim()).file_name().and_then(|f| f.to_str()).unwrap_or("sheet.dxf").to_string();
        dxf.push((name, read_bytes(BUILD, p)?));
    }
    if dxf.is_empty() {
        return Err(bad(BUILD, "none of the sheets is a DXF file"));
    }
    Ok(fr::read_framing(&dxf))
}

// ---- The framing with the entries applied ------------------------------------------------------

/// The wall an entry is on: the same id when its point is still on it, else the wall of its level
/// the point is on.
fn locate(f: &Framing, wall: &str, level: &str, at: [f64; 2]) -> Option<(usize, f64)> {
    let on = |w: &WallFrame| {
        let (s, off) = w.wall.local(at);
        (w.wall.stud_in.is_some() && off.abs() <= w.wall.thickness_in / 2.0 + ON_WALL_IN && s > 0.0 && s < w.wall.len_in()).then_some(s)
    };
    f.walls
        .iter()
        .enumerate()
        .find(|(_, w)| w.wall.id == wall)
        .and_then(|(i, w)| on(w).map(|s| (i, s)))
        .or_else(|| f.walls.iter().enumerate().filter(|(_, w)| w.wall.level == level).find_map(|(i, w)| on(w).map(|s| (i, s))))
}

/// Shop drawings first (in order), then field points (a measurement wins).
fn apply(base: &Framing, st: &State) -> (Framing, Vec<Value>) {
    let mut f = base.clone();
    let mut stale = Vec::new();
    for e in &st.shop_drawings {
        f.apply_shop_drawing(&e.level, &e.drawing);
    }
    for e in &st.field_points {
        match locate(&f, &e.wall, &e.level, e.at_ft) {
            Some((i, s)) => {
                if let Err(why) = f.field_point(i, s) {
                    stale.push(json!({"kind": "field_point", "wall": e.wall, "why": why}));
                }
            }
            None => stale.push(json!({"kind": "field_point", "wall": e.wall, "why": "no wall of the rebuilt framing is under the measured stud"})),
        }
    }
    (f, stale)
}

fn entries_key(st: &State) -> String {
    serde_json::to_string(&(&st.field_points, &st.shop_drawings)).unwrap_or_default()
}

/// The job's framing, built (from the cache, or the sheets framed last) and with the entries applied.
fn framing<'a>(s: &'a mut Session, cmd: &str) -> Result<&'a Cache> {
    let st = load(s, cmd)?;
    if st.sheets.is_empty() {
        return Err(bad(cmd, "no framing yet: run framing.build {dir: \"<folder of DXF sheets>\"} first"));
    }
    let fresh = s.framing.as_deref().is_some_and(|c| c.sheets == st.sheets);
    if !fresh {
        let base = read_sheets(&st.sheets)?;
        s.framing = Some(Box::new(Cache { sheets: st.sheets.clone(), base, entries: String::from("\u{0}"), ..Cache::default() }));
    }
    let key = entries_key(&st);
    let c = s.framing.as_deref_mut().ok_or_else(|| bad(cmd, "the framing cache is gone"))?;
    if c.entries != key {
        let (applied, stale) = apply(&c.base, &st);
        c.applied = applied;
        c.stale = stale;
        c.entries = key;
    }
    s.framing.as_deref().ok_or_else(|| bad(cmd, "the framing cache is gone"))
}

/// The job's framing when it has one (the BOM): `None` when nothing was framed.
pub(crate) fn job_framing(s: &mut Session) -> Option<Framing> {
    framing(s, "bim.bom").ok().map(|c| c.applied.clone())
}

// ---- Answers -----------------------------------------------------------------------------------

fn r2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

fn status_key(s: Status) -> &'static str {
    match s {
        Status::Fixed => "fixed",
        Status::Agreed => "agreed",
        Status::Rule => "rule",
        Status::ShopDrawing => "shop_drawing",
        Status::FieldVerified => "field_verified",
    }
}

fn kind_key(k: Kind) -> String {
    k.words().replace(' ', "_")
}

fn counts(w: &WallFrame) -> Value {
    let n = |pred: &dyn Fn(&fr::Member) -> bool| w.members.iter().filter(|m| pred(m)).count();
    json!({
        "fixed": n(&|m| m.status == Status::Fixed),
        "agreed": n(&|m| m.status == Status::Agreed),
        "rule_a": n(&|m| m.status == Status::Rule && m.layout == Some(End::A)),
        "rule_b": n(&|m| m.status == Status::Rule && m.layout == Some(End::B)),
        "shop_drawing": n(&|m| m.status == Status::ShopDrawing),
        "field_verified": n(&|m| m.status == Status::FieldVerified),
    })
}

fn wall_row(w: &WallFrame) -> Value {
    let f = &w.wall;
    json!({
        "id": f.id, "level": f.level, "type": f.wall_type, "len": fr::inch(f.len_in()),
        "a_ft": [r2(f.a_ft[0]), r2(f.a_ft[1])], "b_ft": [r2(f.b_ft[0]), r2(f.b_ft[1])],
        "stud_in": f.stud_in, "spacing_in": f.spacing_in, "settled": w.settled,
        "counts": counts(w), "flags": w.flags.len() + w.field_changes.len(),
    })
}

fn find_wall<'a>(f: &'a Framing, cmd: &str, p: &Value) -> Result<(usize, &'a WallFrame)> {
    let id =
        str_param(p, "wall").map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| bad(cmd, "`wall` (a wall id from framing.list) is required"))?;
    f.walls
        .iter()
        .enumerate()
        .find(|(_, w)| w.wall.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| bad(cmd, format!("no framed wall `{}`: framing.list gives the ids", id.chars().take(40).collect::<String>())))
}

fn page(p: &Value) -> (usize, usize) {
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |v| usize::try_from(v).unwrap_or(PAGE_MAX)).clamp(1, PAGE_MAX);
    (offset, limit)
}

fn list(f: &Framing, p: &Value) -> Value {
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty());
    let flagged = p.get("flagged").and_then(Value::as_bool);
    let settled = p.get("settled").and_then(Value::as_bool);
    let rows: Vec<&WallFrame> = f
        .walls
        .iter()
        .filter(|w| level.is_none_or(|l| w.wall.level.eq_ignore_ascii_case(l)))
        .filter(|w| flagged.is_none_or(|x| x != (w.flags.is_empty() && w.field_changes.is_empty())))
        .filter(|w| settled.is_none_or(|x| x == w.settled.is_some()))
        .collect();
    let (offset, limit) = page(p);
    let walls: Vec<Value> = rows.iter().skip(offset).take(limit).map(|w| wall_row(w)).collect();
    let mut out = json!({"total": rows.len(), "offset": offset, "walls": walls});
    if offset.saturating_add(limit) < rows.len() {
        out["next_offset"] = json!(offset + limit);
    }
    out
}

fn summary(f: &Framing) -> Value {
    let t = f.takeoff();
    let ra: usize = t.rule_a.values().sum();
    let rb: usize = t.rule_b.values().sum();
    let mut by = serde_json::Map::new();
    for s in [Status::Fixed, Status::Agreed, Status::Rule, Status::ShopDrawing, Status::FieldVerified] {
        by.insert(status_key(s).into(), json!(t.by_status(s)));
    }
    json!({
        "walls": f.walls.len(),
        "stud_walls": f.walls.iter().filter(|w| w.wall.stud_in.is_some()).count(),
        "stacks": f.stacks.len(),
        "by_status": by,
        "rule_layouts": {"a": ra, "b": rb, "rule_max": t.rule_max()},
        "flagged": f.walls.iter().filter(|w| !(w.flags.is_empty() && w.field_changes.is_empty())).count(),
    })
}

// ---- Commands ----------------------------------------------------------------------------------

fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    let mut st = load(s, BUILD)?;
    let paths = sheet_paths(s, p, &st)?;
    let base = read_sheets(&paths)?;
    let read = paths.iter().filter(|p| is_dxf(p)).count();
    st.sheets = paths;
    save(s, &st)?;
    s.framing = Some(Box::new(Cache { sheets: st.sheets.clone(), base, entries: String::from("\u{0}"), ..Cache::default() }));
    let c = framing(s, BUILD)?;
    let mut out = summary(&c.applied);
    out["sheets"] = json!(read);
    out["reapplied"] = json!({
        "field_points": st.field_points.len().saturating_sub(c.stale.iter().filter(|x| x["kind"] == "field_point").count()),
        "shop_drawings": st.shop_drawings.len(),
    });
    out["stale"] = json!(c.stale);
    out["list"] = list(&c.applied, p);
    Ok(out)
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let c = framing(s, LIST)?;
    Ok(list(&c.applied, p))
}

fn next_stud(w: &WallFrame) -> Value {
    json!({"from_a": w.next_stud_words(End::A), "from_b": w.next_stud_words(End::B)})
}

fn run_wall(s: &mut Session, p: &Value) -> Result<Value> {
    let c = framing(s, WALL)?;
    let f = &c.applied;
    let (i, w) = find_wall(f, WALL, p)?;
    let mut by: BTreeMap<&'static str, BTreeMap<String, usize>> = BTreeMap::new();
    for m in &w.members {
        *by.entry(status_key(m.status)).or_default().entry(kind_key(m.kind)).or_default() += 1;
    }
    let layout = |e: End| {
        let studs: Vec<f64> = w
            .members
            .iter()
            .filter(|m| m.layout == Some(e) && matches!(m.kind, Kind::FieldStud | Kind::Cripple))
            .map(|m| r2(m.s_in))
            .take(MEMBERS_MAX)
            .collect();
        json!({"field_studs": studs.len(), "s_in": studs, "first": w.next_stud_in(e)})
    };
    let stack: Vec<&str> = f
        .stacks
        .iter()
        .find(|g| g.contains(&i))
        .map(|g| g.iter().filter_map(|j| f.walls.get(*j)).map(|x| x.wall.id.as_str()).collect())
        .unwrap_or_default();
    let mut out = wall_row(w);
    out["len_in"] = json!(r2(w.wall.len_in()));
    out["stack"] = json!(stack);
    out["by_status"] = json!(by);
    out["layouts"] = json!({"a": layout(End::A), "b": layout(End::B)});
    out["next_stud"] = next_stud(w);
    out["flags"] = json!(w.flags);
    out["field_changes"] = json!(w.field_changes);
    if bool_or(p, "members", false) {
        let (offset, limit) = page(p);
        let limit = limit.min(MEMBERS_MAX);
        let ms: Vec<Value> = w
            .members
            .iter()
            .skip(offset)
            .take(limit)
            .map(|m| {
                let mut v = json!({"kind": kind_key(m.kind), "status": status_key(m.status), "s_in": r2(m.s_in), "width_in": r2(m.width_in), "z_in": [r2(m.z_in[0]), r2(m.z_in[1])]});
                if let Some(l) = m.layout {
                    v["layout"] = json!(l.letter());
                }
                if !m.note.is_empty() {
                    v["note"] = json!(m.note);
                }
                v
            })
            .collect();
        out["members"] = json!(ms);
        out["members_total"] = json!(w.members.len());
    }
    Ok(out)
}

fn run_next(s: &mut Session, p: &Value) -> Result<Value> {
    let from = match str_param(p, "from") {
        None => None,
        Some(v) => Some(End::parse(v.trim()).ok_or_else(|| bad(NEXT, "`from` is a or b"))?),
    };
    let c = framing(s, NEXT)?;
    let (_, w) = find_wall(&c.applied, NEXT, p)?;
    let mut out = json!({
        "wall": w.wall.id, "settled": w.settled,
        "first_in": {"a": w.next_stud_in(End::A), "b": w.next_stud_in(End::B)},
    });
    match from {
        Some(e) => out["text"] = json!(w.next_stud_words(e)),
        None => {
            out["from_a"] = json!(w.next_stud_words(End::A));
            out["from_b"] = json!(w.next_stud_words(End::B));
        }
    }
    Ok(out)
}

fn author(p: &Value) -> String {
    str_param(p, "author").map(str::trim).filter(|a| !a.is_empty()).unwrap_or("user").chars().take(80).collect()
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn pt(v: &Value) -> Option<[f64; 2]> {
    Some([num(v.get(0)?)?, num(v.get(1)?)?])
}

fn run_field(s: &mut Session, p: &Value) -> Result<Value> {
    let who = author(p);
    let c = framing(s, FIELD)?;
    let f = &c.applied;
    let (i, w) = find_wall(f, FIELD, p)?;
    let len = w.wall.len_in();
    let flange = w.wall.flange_in;
    let (how, input, centre) = if let Some(v) = p.get("centre_in").filter(|v| !v.is_null()) {
        let c = num(v).ok_or_else(|| bad(FIELD, "`centre_in` is a number (inches from the a end)"))?;
        ("centre", json!({"centre_in": c}), c)
    } else if let Some(t) = p.get("tape").filter(|v| !v.is_null()) {
        let from = t.get("from").and_then(Value::as_str).and_then(End::parse).ok_or_else(|| bad(FIELD, "`tape.from` is a or b"))?;
        let d = t.get("to_near_edge_in").and_then(num).ok_or_else(|| bad(FIELD, "`tape.to_near_edge_in` is a number (inches)"))?;
        let c = match from {
            End::A => d + flange / 2.0,
            End::B => len - d - flange / 2.0,
        };
        ("tape", json!({"from": from.letter(), "to_near_edge_in": d}), c)
    } else if let Some(ph) = p.get("photo").filter(|v| !v.is_null()) {
        let refs = ph
            .get("refs")
            .and_then(Value::as_array)
            .filter(|r| r.len() == 3)
            .ok_or_else(|| bad(FIELD, "`photo.refs` is three [x, y, inches from a]"))?;
        let mut rs = [([0.0, 0.0], 0.0); 3];
        for (slot, r) in rs.iter_mut().zip(refs) {
            let xy = pt(r).ok_or_else(|| bad(FIELD, "each of `photo.refs` is [x, y, inches from a]"))?;
            let sv = r.get(2).and_then(num).ok_or_else(|| bad(FIELD, "each of `photo.refs` is [x, y, inches from a]"))?;
            *slot = (xy, sv);
        }
        let pick = ph.get("pick").and_then(pt).ok_or_else(|| bad(FIELD, "`photo.pick` is [x, y] on the photo"))?;
        let c = fr::photo_to_wall(rs, pick)
            .ok_or_else(|| bad(FIELD, "the photo's reference points don't fix a position along the wall (pick three apart)"))?;
        ("photo", ph.clone(), c)
    } else {
        return Err(bad(FIELD, "give the stud as `centre_in`, `tape` {from, to_near_edge_in} or `photo` {refs, pick}"));
    };
    // Checked on a copy first: an entry that can't apply is refused, not kept.
    let mut trial = f.clone();
    let snap = trial.field_point(i, centre).map_err(|e| bad(FIELD, e))?;
    let entry = FieldEntry {
        wall: w.wall.id.clone(),
        level: w.wall.level.clone(),
        at_ft: w.wall.at(centre),
        centre_in: centre,
        how: how.into(),
        input,
        author: who,
    };
    let snapped: Vec<String> = snap.walls.iter().filter_map(|j| trial.walls.get(*j)).map(|x| x.wall.id.clone()).collect();
    let settled = trial.walls.get(i).and_then(|x| x.settled.clone());
    let next = trial.walls.get(i).map(next_stud).unwrap_or(Value::Null);
    let mut st = load(s, FIELD)?;
    let same = st.field_points.iter().position(|e| e.wall == entry.wall);
    let changed = match same {
        Some(k) if st.field_points.get(k).is_some_and(|e| (e.centre_in - centre).abs() < SAME_IN) => false,
        Some(k) => {
            st.field_points.remove(k);
            st.field_points.push(entry.clone());
            true
        }
        None => {
            if st.field_points.len() >= MAX_ENTRIES {
                return Err(bad(FIELD, format!("the job has {MAX_ENTRIES} field points; that is the most it keeps")));
            }
            st.field_points.push(entry.clone());
            true
        }
    };
    if changed {
        save(s, &st)?;
    }
    Ok(json!({
        "changed": changed, "wall": entry.wall, "centre_in": r2(centre), "how": entry.how, "author": entry.author,
        "matches": snap.matches.map(End::letter), "off_in": r2(snap.off_in), "walls": snapped, "settled": settled, "next_stud": next,
        "message": if changed { "entered: kept in the job and re-applied on every rebuild" } else { "already entered: nothing changed" },
    }))
}

fn run_shop(s: &mut Session, p: &Value) -> Result<Value> {
    let file = str_param(p, "file")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad(SHOP, "`file` (a DXF shop drawing) is required"))?
        .to_string();
    let level = str_param(p, "level")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad(SHOP, "`level` (the level it frames) is required"))?
        .to_string();
    let upf = match p.get("units_per_foot").filter(|v| !v.is_null()) {
        None => 12.0,
        Some(v) => num(v).filter(|u| *u > 0.0 && *u < 1e6).ok_or_else(|| bad(SHOP, "`units_per_foot` is a positive number"))?,
    };
    let off = match p.get("offset_ft").filter(|v| !v.is_null()) {
        None => [0.0, 0.0],
        Some(v) => pt(v).ok_or_else(|| bad(SHOP, "`offset_ft` is [x, y] in feet"))?,
    };
    let who = author(p);
    // The framing first: a shop drawing needs walls to land on.
    let before = framing(s, SHOP)?.applied.clone();
    if !before.walls.iter().any(|w| w.wall.level.eq_ignore_ascii_case(&level)) {
        return Err(bad(SHOP, format!("no framed wall on level `{}`: framing.list gives the levels", level.chars().take(40).collect::<String>())));
    }
    let level = before.walls.iter().find(|w| w.wall.level.eq_ignore_ascii_case(&level)).map_or(level, |w| w.wall.level.clone());
    let bytes = read_bytes(SHOP, &file)?;
    let h = hash(&bytes);
    let hooks = super::file::io().ok_or_else(|| bad(SHOP, "file formats are not available in this build"))?;
    let name = std::path::Path::new(&file).file_name().and_then(|f| f.to_str()).unwrap_or("shop.dxf").to_string();
    let d: Drawing = (hooks.read)(&bytes, &name).map_err(|e| bad(SHOP, format!("{file}: {e}")))?;
    let sd = fr::read_shop_drawing(&d, &|q| [q[0] / upf + off[0], q[1] / upf + off[1]]);
    if sd.studs.is_empty() && sd.joists.is_empty() && sd.tendons.is_empty() {
        return Err(bad(SHOP, format!("{file} has nothing on STUD, JOIST/TRUSS or TENDON/PT layers")));
    }
    let entry = ShopEntry { file: file.clone(), level: level.clone(), hash: h, units_per_foot: upf, offset_ft: off, author: who, drawing: sd };
    let mut st = load(s, SHOP)?;
    let same = st.shop_drawings.iter().position(|e| e.file == entry.file && e.level == entry.level);
    let changed = match same {
        Some(k) if st.shop_drawings.get(k).is_some_and(|e| e.hash == entry.hash && e.units_per_foot == upf && e.offset_ft == off) => false,
        Some(k) => {
            st.shop_drawings.remove(k);
            st.shop_drawings.push(entry.clone());
            true
        }
        None => {
            if st.shop_drawings.len() >= MAX_ENTRIES {
                return Err(bad(SHOP, format!("the job has {MAX_ENTRIES} shop drawings; that is the most it keeps")));
            }
            st.shop_drawings.push(entry.clone());
            true
        }
    };
    if changed {
        save(s, &st)?;
    }
    let after = framing(s, SHOP)?;
    let walls_changed = after.applied.walls.iter().zip(&before.walls).filter(|(a, b)| a.members != b.members).count();
    Ok(json!({
        "changed": changed, "file": file, "level": level, "author": entry.author,
        "studs": entry.drawing.studs.len(), "joists": entry.drawing.joists.len(), "tendons": entry.drawing.tendons.len(),
        "walls_changed": walls_changed,
        "message": if changed { "applied: kept in the job and re-applied on every rebuild" } else { "already applied: nothing changed" },
    }))
}

// ---- The view ----------------------------------------------------------------------------------

fn view_tag(key: &str) -> Vec<RawTag> {
    vec![RawTag { code: 1001, value: APP.into() }, RawTag { code: 1000, value: key.chars().take(255).collect() }]
}

fn view_key(e: &Entity) -> Option<&str> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value == APP)?;
    x.get(at + 1).filter(|t| t.code == 1000).map(|t| t.value.as_str())
}

/// A box as a mesh: 8 corners, 6 faces.
fn box_mesh(lo: [f64; 3], hi: [f64; 3]) -> Mesh {
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: u32, b: u32, c: u32, d: u32| MeshFace { v: vec![a, b, c, d], hidden: 0 };
    Mesh { vertices, faces: vec![f(0, 3, 2, 1), f(4, 5, 6, 7), f(0, 1, 5, 4), f(1, 2, 6, 5), f(2, 3, 7, 6), f(3, 0, 4, 7)], ..Mesh::default() }
}

fn walk_layer(st: Status, layout: Option<End>) -> &'static str {
    match (st, layout) {
        (Status::Fixed, _) => "FRAMING-3D-FIXED",
        (Status::Agreed, _) => "FRAMING-3D-AGREED",
        (Status::Rule, Some(End::B)) => "FRAMING-3D-RULE-B",
        (Status::Rule, _) => "FRAMING-3D-RULE-A",
        (Status::ShopDrawing, _) => "FRAMING-3D-SHOP",
        (Status::FieldVerified, _) => "FRAMING-3D-VERIFIED",
    }
}

/// `src`'s objects into the drawing (scaled, moved), tagged `key`; its layers and linetypes too.
fn put(d: &mut Drawing, src: &Drawing, scale: f64, at: [f64; 2], key: &str, texts: &mut Vec<String>) -> usize {
    super::exchange::merge_tables(d, src);
    let m = cadaddy_geom::Mat3::scale(scale, scale).then(cadaddy_geom::Mat3::translate(cadaddy_geom::Vec2::new(at[0], at[1])));
    let mut n = 0;
    for e in src.model.iter() {
        let mut kind = e.kind.clone();
        kind.transform(&m);
        if let EntityKind::Text(t) = &kind
            && e.common.layer == "FRAMING-DIMS"
            && texts.len() < 200
        {
            texts.push(t.value.clone());
        }
        let common = Common { xdata: view_tag(key).into(), ..e.common.clone() };
        if d.add(&Space::Model, common, kind).is_ok() {
            n += 1;
        }
    }
    n
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    let upf = match p.get("units_per_foot").filter(|v| !v.is_null()) {
        None => 12.0,
        Some(v) => num(v).filter(|u| *u > 0.0 && *u < 1e6).ok_or_else(|| bad(VIEW, "`units_per_foot` is a positive number"))?,
    };
    let walk = bool_or(p, "walk", false);
    let c = framing(s, VIEW)?;
    let f = c.applied.clone();
    let wall = match str_param(p, "wall").map(str::trim).filter(|v| !v.is_empty()) {
        Some(_) => Some(find_wall(&f, VIEW, p)?.1.clone()),
        None => None,
    };
    let level = match (str_param(p, "level").map(str::trim).filter(|v| !v.is_empty()), &wall) {
        (Some(l), _) => {
            let found = f.walls.iter().find(|w| w.wall.level.eq_ignore_ascii_case(l)).map(|w| w.wall.level.clone());
            Some(found.ok_or_else(|| bad(VIEW, format!("no framed wall on level `{}`", l.chars().take(40).collect::<String>())))?)
        }
        (None, Some(w)) if walk => Some(w.wall.level.clone()),
        _ => None,
    };
    if wall.is_none() && level.is_none() {
        return Err(bad(VIEW, "give a `wall` (its elevation) or a `level` (its framing plan); `walk: true` adds the 3D members"));
    }
    let elev = match p.get("elevation_ft").filter(|v| !v.is_null()) {
        Some(v) => num(v).ok_or_else(|| bad(VIEW, "`elevation_ft` is a number"))?,
        None => match (&level, super::bspec::current(s, VIEW)) {
            (Some(l), Ok((sp, true))) => sp.level(l).map_or(0.0, |x| x.elevation_ft),
            _ => 0.0,
        },
    };
    let scale = upf / 12.0;
    let mut keys: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut objects = 0usize;
    let d = s.doc_mut()?;
    let want: Vec<String> = [
        level.as_ref().map(|l| format!("plan:{l}")),
        wall.as_ref().map(|w| format!("elevation:{}", w.wall.id)),
        level.as_ref().filter(|_| walk).map(|l| format!("walk:{l}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let before = d.model.len();
    let gone: Vec<cadaddy_doc::Handle> =
        d.model.iter().filter(|e| view_key(e).is_some_and(|k| want.iter().any(|w| w == k))).map(|e| e.handle).collect();
    for h in &gone {
        d.model.remove(*h);
    }
    let replaced = before.saturating_sub(d.model.len());
    if let Some(l) = &level {
        let key = format!("plan:{l}");
        objects += put(d, &fr::draw_plan(&f, l), scale, [0.0, 0.0], &key, &mut texts);
        keys.push(key);
    }
    if let Some(w) = &wall {
        let key = format!("elevation:{}", w.wall.id);
        // Default: left of the wall's plan position, clear of the plan.
        let at = match p.get("at").filter(|v| !v.is_null()) {
            Some(v) => pt(v).ok_or_else(|| bad(VIEW, "`at` is [x, y]"))?,
            None => [w.wall.a_ft[0] * upf, w.wall.a_ft[1] * upf - (w.wall.height_ft + 10.0) * upf],
        };
        objects += put(d, &fr::draw_elevation(w), scale, at, &key, &mut texts);
        keys.push(key);
    }
    if let Some(l) = level.as_ref().filter(|_| walk) {
        let key = format!("walk:{l}");
        for (lo, hi, st, layout) in fr::boxes(&f, l, elev).into_iter().take(500_000) {
            let lo = [lo[0] * upf, lo[1] * upf, lo[2] * upf];
            let hi = [hi[0] * upf, hi[1] * upf, hi[2] * upf];
            if !(lo.iter().chain(&hi).all(|v| v.is_finite())) {
                continue;
            }
            let common = Common { layer: walk_layer(st, layout).into(), xdata: view_tag(&key).into(), ..Common::default() };
            if d.add(&Space::Model, common, EntityKind::Mesh(box_mesh(lo, hi))).is_ok() {
                objects += 1;
            }
        }
        keys.push(key);
    }
    let mut layers: Vec<String> = d.model.iter().filter(|e| view_key(e).is_some()).map(|e| e.common.layer.to_string()).collect();
    layers.sort();
    layers.dedup();
    Ok(json!({"views": keys, "objects": objects, "replaced": replaced, "layers": layers, "texts": texts}))
}

// ---- The BOM -----------------------------------------------------------------------------------

/// One framing BOM line: (level, kind, status, count, from a rule layout).
pub(crate) type FramingLine = (String, Kind, Status, usize, bool);

/// The framing's BOM lines by level: settled members by status, rule members of the larger layout
/// (so the rule lines add up to [`fr::Takeoff::rule_max`]).
pub(crate) fn bom_lines(f: &Framing) -> (Vec<FramingLine>, Option<End>) {
    let t = f.takeoff();
    let ra: usize = t.rule_a.values().sum();
    let rb: usize = t.rule_b.values().sum();
    let larger = if ra == 0 && rb == 0 {
        None
    } else if ra >= rb {
        Some(End::A)
    } else {
        Some(End::B)
    };
    let mut by: BTreeMap<(String, Kind, Status, bool), usize> = BTreeMap::new();
    let members = f
        .walls
        .iter()
        .flat_map(|w| w.members.iter().map(move |m| (&w.wall.level, m)))
        .chain(f.bays.iter().flat_map(|b| b.members.iter().map(move |m| (&b.bay.level, m))));
    for (level, m) in members {
        match m.layout {
            None => *by.entry((level.clone(), m.kind, m.status, false)).or_default() += 1,
            Some(e) if Some(e) == larger => *by.entry((level.clone(), m.kind, m.status, true)).or_default() += 1,
            Some(_) => {}
        }
    }
    (by.into_iter().map(|((l, k, s, r), n)| (l, k, s, n, r)).collect(), larger)
}

/// Inches as the prints write them (`2-1/8"`).
pub(crate) fn inch_words(v: f64) -> String {
    fr::inch(v)
}

/// Holes through framing members per bit, from the job's raceways crossing the framed walls
/// (`runs`: level, plan points in feet, trade size inches, cable).
pub(crate) fn bits(f: &Framing, runs: &[(String, Vec<[f64; 2]>, f64, bool)]) -> Vec<BitLine> {
    let mut checks: Vec<bore::BoreCheck> = Vec::new();
    for (level, pts, size, cable) in runs.iter().take(100_000) {
        let r = if *cable { bore::Raceway::Mc } else { bore::Raceway::Emt(*size) };
        for seg in pts.windows(2).take(10_000) {
            let (Some(a), Some(b)) = (seg.first(), seg.get(1)) else { continue };
            for x in fr::wall_crossings(f, level, *a, *b, r) {
                let Some(c) = x.check.filter(|_| x.along) else { continue };
                let holes = x.studs.iter().copied().max().unwrap_or(0).min(10_000);
                checks.extend(std::iter::repeat_n(c, holes));
            }
        }
    }
    bore::bit_takeoff(checks.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_params_are_refused_not_panics() {
        let mut s = Session::new();
        for (cmd, p) in [
            (LIST, json!({})),
            (WALL, json!({"wall": "X"})),
            (NEXT, json!({"wall": 7})),
            (FIELD, json!({"wall": "X", "centre_in": "a lot"})),
            (SHOP, json!({"file": ""})),
            (VIEW, json!({"walk": "yes"})),
            (BUILD, json!({"paths": "not a list"})),
            (BUILD, json!({"paths": [1, 2]})),
            (BUILD, json!({})),
        ] {
            assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
        }
    }

    #[test]
    fn a_box_mesh_has_six_faces_round_eight_corners() {
        let m = box_mesh([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.faces.len(), 6);
        assert!(m.faces.iter().all(|f| f.v.iter().all(|i| (*i as usize) < 8)));
    }
}
