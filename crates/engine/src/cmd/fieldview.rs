//! The model on the job (Q312): a field package for the tablet and phone viewer, and the notes
//! made in the field back into the drawing.
//!
//! - `fieldview.export` — the levels, rooms, model elements (with their BIM type, Q297), open
//!   clashes and RFIs, and the notes so far, as one `cadaddy-walk3d` [`FieldPackage`] file the
//!   viewer opens offline.
//! - `fieldview.sync` — merges a [`FieldNotes`] file (notes and verified points from the field)
//!   into the drawing, by note id, so syncing the same file twice adds nothing.
//! - `fieldview.notes` — the drawing's field notes, by level or room.
//!
//! Field notes are kept in the drawing's embedded files (`fieldnotes.json`), so they travel in
//! .cdy files and survive edits of the building spec.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::bimspec::entity_type_mark;
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_walk3d::field::{
    FIELD_SCHEMA, FieldElement, FieldIssue, FieldLevel, FieldNote, FieldNotes, FieldPackage, FieldRoom, FieldSpec, FieldType, IssueKind,
    MAX_ELEMENTS, MAX_NOTES, merge_notes, trade_info, trade_of,
};
use cadaddy_walk3d::{BuildingSpec, MepKind};
use serde_json::{Value, json};

use super::bspec::current;
use super::wallgen::{self, made_as};
use super::*;
use crate::{Result, Session};

const CMD_EXPORT: &str = "fieldview.export";
const CMD_SYNC: &str = "fieldview.sync";
const CMD_NOTES: &str = "fieldview.notes";
/// How thick a wall face is drawn, inches (a plan's wall is its two faces).
const WALL_FACE_IN: f64 = 1.0;
/// The embedded file the drawing keeps its field notes in.
pub const NOTES_FILE: &str = "fieldnotes.json";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_EXPORT, "Field Package", run_export)
            .params("{path?: file to write, project?, inline?: bool (return the package)} → {path?, bytes, levels, rooms, elements, trades: {trade: count}, issues, notes, package?}")
            .noundo(),
        CommandSpec::new(CMD_SYNC, "Sync Field Notes", run_sync)
            .params("{path?: notes file | notes?: {schema, project, notes: [...]}} → {added, updated, unchanged, total}"),
        CommandSpec::new(CMD_NOTES, "Field Notes", run_notes).params("{level?, room?} → {notes: [...]}").noundo(),
    ]
}

/// The drawing's field notes.
pub fn stored_notes(d: &Drawing) -> Vec<FieldNote> {
    d.embedded.get(NOTES_FILE).and_then(|b| FieldNotes::from_json(b).ok()).map(|n| n.notes).unwrap_or_default()
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Sheet (drawing units) to building feet on a level.
struct Place {
    upf: f64,
    bases: BTreeMap<String, [f64; 2]>,
}

impl Place {
    fn base(&self, level: &str) -> [f64; 2] {
        self.bases.get(&level.to_ascii_lowercase()).copied().unwrap_or([0.0, 0.0])
    }
    fn xy(&self, level: &str, x: f64, y: f64) -> [f64; 2] {
        let b = self.base(level);
        [r3((x - b[0]) / self.upf), r3((y - b[1]) / self.upf)]
    }
    fn xyz(&self, level: &str, x: f64, y: f64, z: f64) -> [f64; 3] {
        let [a, b] = self.xy(level, x, y);
        [a, b, r3(z / self.upf)]
    }
}

/// A generated entity's tags after its `CADADDY_WALLGEN` app name: part, level, source, then
/// `key:value` extras (MEP: `system:`, `kind:`, `part:`).
fn wallgen_tags(e: &Entity) -> Vec<&str> {
    let x = &e.common.xdata;
    let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP)) else { return Vec::new() };
    x.iter().skip(at + 1).take(64).take_while(|t| t.code == 1000).map(|t| t.value.as_str()).collect()
}

fn tag_value<'a>(tags: &[&'a str], key: &str) -> Option<&'a str> {
    tags.iter().find_map(|t| t.strip_prefix(key).and_then(|v| v.strip_prefix(':')))
}

fn mesh_box(e: &Entity) -> Option<([f64; 3], [f64; 3])> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in m.vertices.iter().take(1_000_000) {
        for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            if c.is_finite()
                && let (Some(l), Some(h)) = (lo.get_mut(i), hi.get_mut(i))
            {
                *l = l.min(c);
                *h = h.max(c);
            }
        }
    }
    lo.iter().chain(hi.iter()).all(|v| v.is_finite()).then_some((lo, hi))
}

fn mesh_outline(e: &Entity) -> Option<(Vec<[f64; 2]>, f64)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let z = m.vertices.first()?.z;
    Some((m.vertices.iter().take(512).map(|v| [v.x, v.y]).collect(), z))
}

/// The BIM mark on an entity, else on the plan entity it was made from.
fn mark_of(d: &Drawing, e: &Entity, source: Option<&str>) -> String {
    if let Some(m) = entity_type_mark(&e.common.xdata) {
        return m.to_string();
    }
    source
        .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
        .filter(|h| *h != 0)
        .and_then(|h| d.model.get(Handle(h)))
        .and_then(|src| entity_type_mark(&src.common.xdata).map(str::to_string))
        .unwrap_or_default()
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, CMD_EXPORT)?;
    let issues = clash_issues(s);
    let d = s.doc()?;
    let project = str_param(p, "project")
        .map(str::to_string)
        .or_else(|| Some(d.properties.title.clone()).filter(|t| !t.trim().is_empty()))
        .unwrap_or_else(|| "Project".into());
    let mut pkg = package(d, &sp, &project);
    for (level, x, y, title, id, status, kind) in issues.into_iter().chain(rfi_issues(d)) {
        let z = pkg.level(&level).map_or(0.0, |l| l.elevation_ft + l.height_ft * 0.5);
        let [ax, ay] = place_of(d, &sp).xy(&level, x, y);
        pkg.issues.push(FieldIssue { id, kind, title, status, level, room: String::new(), at: [ax, ay, r3(z)], elements: Vec::new() });
    }
    pkg.notes = stored_notes(d)
        .into_iter()
        .map(|mut n| {
            n.synced = true;
            n
        })
        .collect();
    pkg.assign_rooms();
    let text = pkg.to_json().map_err(|e| bad(CMD_EXPORT, e))?;
    let mut out = json!({
        "bytes": text.len(),
        "levels": pkg.levels.len(),
        "rooms": pkg.rooms.len(),
        "elements": pkg.elements.len(),
        "trades": pkg.trade_counts(),
        "issues": pkg.issues.len(),
        "notes": pkg.notes.len(),
    });
    if pkg.elements.is_empty() {
        out["hint"] = json!("no model elements yet: run model.build first");
    }
    if let Some(path) = str_param(p, "path") {
        crate::writefile::write(path, &text).map_err(|e| bad(CMD_EXPORT, format!("{path}: {e}")))?;
        out["path"] = json!(path);
    }
    if bool_or(p, "inline", false) {
        out["package"] = serde_json::from_str(&text).unwrap_or(Value::Null);
    }
    Ok(out)
}

fn place_of(d: &Drawing, sp: &BuildingSpec) -> Place {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let bases = cadaddy_walk3d::level_bases(d, &sp.to_rules()).into_iter().map(|(n, b)| (n.to_ascii_lowercase(), [b.x, b.y])).collect();
    Place { upf, bases }
}

/// The package for a drawing and its spec (no issues or notes).
pub fn package(d: &Drawing, sp: &BuildingSpec, project: &str) -> FieldPackage {
    let place = place_of(d, sp);
    let mut pkg = FieldPackage::new(project);
    pkg.schema = FIELD_SCHEMA.into();
    pkg.levels = sp.levels.iter().map(|l| FieldLevel { name: l.name.clone(), elevation_ft: l.elevation_ft, height_ft: l.height_ft }).collect();

    // Rooms: the spec's, outlined by the reflected ceiling plan's areas or the built ceilings.
    let mut outlines: Vec<(String, Option<String>, Vec<[f64; 2]>)> = sp
        .rcp
        .ceilings
        .iter()
        .take(20_000)
        .map(|c| (c.level.clone(), c.room.clone(), c.outline.iter().map(|q| place.xy(&c.level, q[0], q[1])).collect()))
        .collect();
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("ceiling")).take(20_000) {
        let Some(level) = super::bspec::made_level(e) else { continue };
        if let Some((poly, _)) = mesh_outline(e) {
            outlines.push((level.clone(), None, poly.iter().map(|q| place.xy(&level, q[0], q[1])).collect()));
        }
    }
    let tags = cadaddy_walk3d::rooms(&[d], &sp.to_rules());
    let units = unit_of_rooms(d);
    for r in sp.rooms.iter().take(20_000) {
        let level = r.level.clone().or_else(|| sp.levels.first().map(|l| l.name.clone())).unwrap_or_default();
        let number = r.number.clone().unwrap_or_else(|| r.name.clone());
        let tag =
            tags.iter().find(|t| t.name.eq_ignore_ascii_case(&r.name) && sp.levels.get(t.level).is_some_and(|l| l.name.eq_ignore_ascii_case(&level)));
        let named = outlines.iter().find(|(l, room, _)| {
            l.eq_ignore_ascii_case(&level) && room.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&number) || x.eq_ignore_ascii_case(&r.name))
        });
        let at = tag.map(|t| [r3(t.at.x), r3(t.at.y)]);
        let around = at.and_then(|a| {
            outlines.iter().find(|(l, _, poly)| l.eq_ignore_ascii_case(&level) && poly.len() >= 3 && cadaddy_walk3d::field::inside(poly, a))
        });
        let outline = named.or(around).map(|(_, _, poly)| poly.clone()).unwrap_or_default();
        let at = at.unwrap_or_else(|| centre(&outline));
        let unit = units.get(&(level.to_ascii_lowercase(), number.to_ascii_lowercase())).cloned().unwrap_or_default();
        pkg.rooms.push(FieldRoom { number, name: r.name.clone(), level, outline, at, unit });
    }

    // Types (Q297), shared by mark.
    pkg.types = d
        .bim_types
        .types
        .iter()
        .map(|t| FieldType {
            mark: t.mark.clone(),
            category: t.category.clone(),
            fields: t
                .fields
                .iter()
                .map(|(k, f)| FieldSpec {
                    key: k.clone(),
                    value: f.value.clone(),
                    source: f.source.clone(),
                    confidence: f.confidence.as_str().to_string(),
                })
                .collect(),
        })
        .collect();

    // Elements: what model.build made (walls, doors, windows, devices, stairs, MEP terminals and
    // equipment) as boxes, and the MEP runs from the spec as centrelines.
    let kinds: BTreeMap<String, MepKind> = sp.mep.systems.iter().map(|sy| (sy.name.clone(), sy.kind)).collect();
    let overrides = super::overrides::load(d).unwrap_or_default();
    for e in d.model.iter() {
        if pkg.elements.len() >= MAX_ELEMENTS {
            break;
        }
        let tags = wallgen_tags(e);
        let (Some(part), Some(level)) = (tags.first().copied(), tags.get(1).copied()) else { continue };
        let source = tags.get(2).copied();
        let (kind, system) = match part {
            // Walls come from the plan's wall faces (below): a closed wall's mesh boxes the room.
            "floor" | "ceiling" | "wall" => continue,
            "mep" => {
                let p = tag_value(&tags, "part").unwrap_or_default();
                // Straight runs come from the spec as centrelines (below).
                if p == "segment" {
                    continue;
                }
                let kind = if p.starts_with("fitting") {
                    "fitting"
                } else if p.starts_with("terminal") {
                    "terminal"
                } else if p.starts_with("equipment") || tag_value(&tags, "kind") == Some("equipment") {
                    "equipment"
                } else {
                    tag_value(&tags, "kind").unwrap_or("mep")
                };
                (kind.to_string(), tag_value(&tags, "system").unwrap_or_default().to_string())
            }
            other => (other.to_string(), e.common.layer.to_string()),
        };
        let Some((lo, hi)) = mesh_box(e) else { continue };
        let trade = match kind.as_str() {
            "wall" | "door" | "window" | "stair" => "arch",
            _ => trade_of(&kind, &system),
        };
        let title = tag_value(&tags, "tag").map(|t| format!(" {t}")).unwrap_or_default();
        pkg.elements.push(FieldElement {
            id: e.handle.hex(),
            trade: trade.into(),
            kind,
            level: level.to_string(),
            room: String::new(),
            min: place.xyz(level, lo[0], lo[1], lo[2]),
            max: place.xyz(level, hi[0], hi[1], hi[2]),
            path: Vec::new(),
            size_in: None,
            mark: mark_of(d, e, source),
            source: format!("{}{title}", if system.is_empty() { e.common.layer.as_str() } else { system.as_str() }),
            status: override_status(&overrides, e),
            props: Vec::new(),
        });
    }
    // Wall faces as the plan draws them, floor to the level above (feet already).
    let rules = sp.to_rules();
    let scene = cadaddy_walk3d::derive(d, &rules);
    for (i, w) in scene.walls.iter().enumerate() {
        if pkg.elements.len() >= MAX_ELEMENTS {
            break;
        }
        let Some(level) = sp.levels.get(w.level) else { continue };
        let (z0, h) = (level.elevation_ft, level.height_ft.max(1.0));
        let zc = r3(z0 + h * 0.5);
        let t = WALL_FACE_IN / 24.0;
        pkg.elements.push(FieldElement {
            id: format!("wall:{}:{i}", level.name),
            trade: "arch".into(),
            kind: "wall".into(),
            level: level.name.clone(),
            room: String::new(),
            min: [r3(w.a.x.min(w.b.x) - t), r3(w.a.y.min(w.b.y) - t), r3(z0)],
            max: [r3(w.a.x.max(w.b.x) + t), r3(w.a.y.max(w.b.y) + t), r3(z0 + h)],
            path: vec![[r3(w.a.x), r3(w.a.y), zc], [r3(w.b.x), r3(w.b.y), zc]],
            size_in: Some([WALL_FACE_IN, r3(h * 12.0)]),
            mark: String::new(),
            source: "walls".into(),
            status: String::new(),
            props: Vec::new(),
        });
    }
    for (ri, run) in sp.mep.runs.iter().enumerate().take(MAX_ELEMENTS) {
        let Some(level) = sp.level(&run.level) else { continue };
        let kind = match kinds.get(&run.system) {
            Some(MepKind::Duct) => "duct",
            Some(MepKind::Pipe) => "pipe",
            Some(MepKind::Tray) => "tray",
            Some(MepKind::Cable) => "cable",
            Some(MepKind::Conduit) | None => "conduit",
        };
        let (w, h) = (run.size_in[0].max(0.0) / 12.0, if run.size_in[1] > 0.0 { run.size_in[1] } else { run.size_in[0] }.max(0.0) / 12.0);
        let mark = run
            .objects
            .first()
            .and_then(|h| u64::from_str_radix(h, 16).ok())
            .and_then(|h| d.model.get(Handle(h)))
            .and_then(|e| entity_type_mark(&e.common.xdata).map(str::to_string))
            .unwrap_or_default();
        for (si, pair) in run.points.windows(2).enumerate().take(512) {
            if pkg.elements.len() >= MAX_ELEMENTS {
                break;
            }
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else { continue };
            let bottom = run.bottoms_ft.get(si).or(run.bottoms_ft.last()).copied().unwrap_or(0.0);
            let z = level.elevation_ft + bottom + h * 0.5;
            let pa = place.xy(&level.name, a[0], a[1]);
            let pb = place.xy(&level.name, b[0], b[1]);
            let r = w * 0.5;
            pkg.elements.push(FieldElement {
                id: format!("run:{}:{ri}:{si}", run.system),
                trade: trade_of(kind, &run.system).into(),
                kind: kind.into(),
                level: level.name.clone(),
                room: String::new(),
                min: [r3(pa[0].min(pb[0]) - r), r3(pa[1].min(pb[1]) - r), r3(z - h * 0.5)],
                max: [r3(pa[0].max(pb[0]) + r), r3(pa[1].max(pb[1]) + r), r3(z + h * 0.5)],
                path: vec![[pa[0], pa[1], r3(z)], [pb[0], pb[1], r3(z)]],
                size_in: Some(run.size_in),
                mark: mark.clone(),
                source: run.system.clone(),
                status: String::new(),
                props: Vec::new(),
            });
        }
    }
    let counts = pkg.trade_counts();
    pkg.trades = ["arch", "struct", "mech", "plumb", "elec", "fire", "lowvolt", "other"]
        .iter()
        .filter(|k| counts.contains_key(**k))
        .map(|k| trade_info(k))
        .collect();
    pkg
}

/// The overrides applied to a generated element: `OV-0003 move (active)`, joined.
fn override_status(all: &cadaddy_doc::overrides::Overrides, e: &Entity) -> String {
    cadaddy_doc::overrides::applied_ids(&e.common.xdata)
        .into_iter()
        .take(8)
        .map(|id| match all.get(id) {
            Some(o) => format!("{} {} ({})", o.label(), o.change.kind(), o.status.as_str()),
            None => cadaddy_doc::overrides::label(id),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn centre(poly: &[[f64; 2]]) -> [f64; 2] {
    if poly.is_empty() {
        return [0.0, 0.0];
    }
    let n = poly.len() as f64;
    let (x, y) = poly.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
    [r3(x / n), r3(y / n)]
}

/// Typical units by (level, room number), lower case: `King (mirrored)` (Q305 keys in the spec).
fn unit_of_rooms(d: &Drawing) -> BTreeMap<(String, String), String> {
    let Some(v) = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) else { return BTreeMap::new() };
    let names: BTreeMap<String, String> = v
        .get("unit_types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(10_000)
        .filter_map(|t| Some((t.get("id")?.as_str()?.to_string(), t.get("name").and_then(Value::as_str).unwrap_or_default().to_string())))
        .collect();
    let mut out = BTreeMap::new();
    for i in v.get("unit_instances").and_then(Value::as_array).into_iter().flatten().take(50_000) {
        let s = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let ty = Some(s("type")).filter(|t| !t.is_empty()).unwrap_or_else(|| s("unit_type"));
        let name = names.get(&ty).filter(|n| !n.is_empty()).cloned().unwrap_or(ty);
        let hand = s("hand");
        let text = if hand.is_empty() { name } else { format!("{name} ({hand})") };
        let number = Some(s("number")).filter(|n| !n.is_empty()).unwrap_or_else(|| s("room"));
        out.insert((s("level").to_ascii_lowercase(), number.to_ascii_lowercase()), text);
    }
    out
}

type RawIssue = (String, f64, f64, String, String, String, IssueKind);

/// The model's clashes (sheet units), as `model.clashes` finds them.
fn clash_issues(s: &mut Session) -> Vec<RawIssue> {
    let Ok(sp) = current(s, CMD_EXPORT).map(|(sp, _)| sp) else { return Vec::new() };
    if sp.mep.runs.is_empty() {
        return Vec::new();
    }
    let Ok(raw) = s.execute("model.clashes", &json!({})) else { return Vec::new() };
    let list = raw.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default();
    let level_of = |sys: &str| sp.mep.runs.iter().find(|r| r.system == sys).map(|r| r.level.clone()).unwrap_or_default();
    list.iter()
        .take(10_000)
        .enumerate()
        .map(|(i, c)| {
            let at = c.get("at").and_then(Value::as_array);
            let n = |k: usize| at.and_then(|a| a.get(k)).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0);
            let sys = c.get("systems").and_then(|v| v.get(0)).and_then(Value::as_str).unwrap_or_default();
            let title = c.get("message").and_then(Value::as_str).unwrap_or("clash").to_string();
            let status = c.get("status").and_then(Value::as_str).unwrap_or("new").to_string();
            (level_of(sys), n(0), n(1), title, format!("CL-{:04}", i + 1), status, IssueKind::Clash)
        })
        .collect()
}

/// The RFIs the spec carries (`rfis`, from the print RFI helper), plan units.
fn rfi_issues(d: &Drawing) -> Vec<RawIssue> {
    let Some(v) = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) else { return Vec::new() };
    v.get("rfis")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(10_000)
        .enumerate()
        .map(|(i, r)| {
            let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let n = |k: usize| r.get("at").and_then(|a| a.get(k)).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0);
            let id = Some(s("id")).filter(|x| !x.is_empty()).unwrap_or_else(|| format!("RFI-{:03}", i + 1));
            let status = Some(s("status")).filter(|x| !x.is_empty()).unwrap_or_else(|| "open".into());
            let about = s("about");
            let title = if about.is_empty() { s("issue") } else { format!("{about}: {}", s("issue")) };
            (s("level"), n(0), n(1), title, id, status, IssueKind::Rfi)
        })
        .collect()
}

fn run_sync(s: &mut Session, p: &Value) -> Result<Value> {
    let incoming = if let Some(path) = str_param(p, "path") {
        let bytes = crate::readfile::read(path).map_err(|e| bad(CMD_SYNC, format!("{path}: {e}")))?;
        FieldNotes::from_json(&bytes).map_err(|e| bad(CMD_SYNC, e))?
    } else if let Some(n) = p.get("notes").filter(|v| v.is_object()) {
        FieldNotes::from_json(n.to_string().as_bytes()).map_err(|e| bad(CMD_SYNC, e))?
    } else {
        return Err(bad(CMD_SYNC, "give `path` (a field notes file) or `notes`"));
    };
    let mut notes = stored_notes(s.doc()?);
    let m = merge_notes(&mut notes, &incoming.notes);
    if m.added + m.updated > 0 {
        let file = FieldNotes::new(&incoming.project, notes.clone());
        let text = serde_json::to_vec(&file).map_err(|e| bad(CMD_SYNC, e.to_string()))?;
        s.doc_mut()?.embedded.insert(NOTES_FILE.into(), Arc::new(text));
    }
    Ok(json!({ "added": m.added, "updated": m.updated, "unchanged": m.unchanged, "total": notes.len().min(MAX_NOTES) }))
}

fn run_notes(s: &mut Session, p: &Value) -> Result<Value> {
    let level = str_param(p, "level");
    let room = str_param(p, "room");
    let notes: Vec<Value> = stored_notes(s.doc()?)
        .into_iter()
        .filter(|n| level.is_none_or(|l| n.level.eq_ignore_ascii_case(l)))
        .filter(|n| room.is_none_or(|r| n.room.eq_ignore_ascii_case(r)))
        .filter_map(|n| serde_json::to_value(n).ok())
        .collect();
    Ok(json!({ "notes": notes }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::field::NOTES_SCHEMA;

    fn session() -> Session {
        let mut s = Session::new();
        let layer = |s: &mut Session, name: &str| {
            if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
                s.execute("layer.current", &json!({"name": name})).unwrap();
            }
        };
        let rect = |s: &mut Session, a: [f64; 2], b: [f64; 2]| {
            s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
        };
        layer(&mut s, "A-WALL");
        rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
        rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
        layer(&mut s, "A-ANNO");
        s.execute("text", &json!({"at": [200, 200], "text": "Guest Room", "height": 6})).unwrap();
        layer(&mut s, "M-DUCT-SUPP");
        s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 180]]})).unwrap();
        s.execute("text", &json!({"at": [180, 70], "text": "24x12 BOT @ 7'-0\"", "height": 4})).unwrap();
        s.execute(
            "bspec.set",
            &json!({"replace": true, "spec": {
                "levels": [{"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
                "layers": [{"layer": "A-WALL", "role": "wall"}],
                "rooms": [{"name": "Guest Room", "number": "214", "level": "L2", "ceiling_ft": 9}],
                "rcp": {"ceilings": [{"level": "L2", "room": "214", "outline": [[0, 0], [480, 0], [480, 360], [0, 360]], "height_ft": 9}]}
            }}),
        )
        .unwrap();
        s.execute("mep.read", &json!({"apply": true})).unwrap();
        s.execute("model.build", &json!({})).unwrap();
        s
    }

    #[test]
    fn export_carries_rooms_elements_types_and_clashes() {
        let mut s = session();
        // Type the duct's plan polyline (Q297): the run picks the mark up.
        let duct =
            s.doc().unwrap().model.iter().find(|e| e.common.layer == "M-DUCT-SUPP" && matches!(e.kind, EntityKind::LwPolyline(_))).unwrap().handle;
        s.execute(
            "bim.set_spec",
            &json!({"handles": [duct.hex()], "mark": "SD-1", "create": {"category": "duct", "fields": {"material": {"value": "galvanized steel", "source": "M-001 duct note 3", "confidence": "stated"}}}}),
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!("cadaddy-fieldview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Project A.field.json");
        let r = s.execute(CMD_EXPORT, &json!({"path": path.to_string_lossy(), "project": "Project A"})).unwrap();
        assert_eq!(r["levels"], 1);
        assert_eq!(r["rooms"], 1);
        assert!(r["trades"]["arch"].as_u64().unwrap() >= 4, "{r}");
        assert_eq!(r["trades"]["mech"], 3, "two duct segments and the elbow: {r}");
        let pkg = FieldPackage::from_json(&std::fs::read(&path).unwrap()).unwrap();
        let room = pkg.rooms.first().unwrap();
        assert_eq!((room.number.as_str(), room.name.as_str()), ("214", "Guest Room"));
        assert_eq!(room.outline.len(), 4);
        assert!((room.outline[2][0] - 40.0).abs() < 1e-9, "feet: {:?}", room.outline);
        let ducts: Vec<&FieldElement> = pkg.elements.iter().filter(|e| e.kind == "duct").collect();
        assert_eq!(ducts.len(), 2);
        assert_eq!(pkg.elements.iter().filter(|e| e.kind == "fitting" && e.trade == "mech").count(), 1);
        let first = ducts[0];
        assert_eq!(first.room, "214");
        assert_eq!(first.mark, "SD-1");
        assert_eq!(first.size_in, Some([24.0, 12.0]));
        // Bottom of duct at 7' above the L2 floor (12'): its centre at 19'-6".
        assert!((first.path[0][2] - 19.5).abs() < 1e-9, "{:?}", first.path);
        assert!((first.path[0][0] - 2.0).abs() < 1e-9 && (first.path[1][0] - 22.0).abs() < 1e-9);
        let ty = pkg.type_of(first).unwrap();
        let f = ty.fields.iter().find(|f| f.key == "material").unwrap();
        assert_eq!((f.value.as_str(), f.source.as_str(), f.confidence.as_str()), ("galvanized steel", "M-001 duct note 3", "stated"));
        // The duct hangs below the 9' ceiling: an open clash in room 214.
        let open: Vec<&FieldIssue> = pkg.open_issues(room).collect();
        assert!(open.iter().any(|i| i.kind == IssueKind::Clash && i.title.contains("below the ceiling")), "{:?}", pkg.issues);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rfis_and_units_from_the_spec_show_on_their_rooms() {
        let mut s = session();
        let mut spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
        spec["rfis"] = json!([
            {"id": "RFI-007", "level": "L2", "at": [120, 120], "about": "wall W3", "issue": "no wall type tag", "status": "open"},
            {"id": "RFI-008", "level": "L2", "at": [130, 120], "issue": "head of wall not given", "status": "answered"}
        ]);
        spec["unit_types"] = json!([{"id": "ut-king", "name": "King"}]);
        spec["unit_instances"] = json!([{"type": "ut-king", "room": "rm-2-1", "number": "214", "level": "L2", "hand": "mirrored"}]);
        s.doc_mut().unwrap().building_spec = Some(spec.to_string());
        let r = s.execute(CMD_EXPORT, &json!({"inline": true})).unwrap();
        let pkg: FieldPackage = serde_json::from_value(r["package"].clone()).unwrap();
        let room = pkg.rooms.first().unwrap();
        assert_eq!(room.unit, "King (mirrored)");
        let rfis: Vec<(&str, bool)> = pkg.issues.iter().filter(|i| i.kind == IssueKind::Rfi).map(|i| (i.id.as_str(), i.is_open())).collect();
        assert_eq!(rfis, vec![("RFI-007", true), ("RFI-008", false)]);
        assert!(pkg.open_issues(room).any(|i| i.id == "RFI-007" && i.title == "wall W3: no wall type tag"));
    }

    #[test]
    fn notes_sync_back_once_and_come_out_in_the_next_package() {
        let mut s = session();
        let notes = json!({"schema": NOTES_SCHEMA, "project": "Project A", "notes": [
            {"id": "FN-tab1-0001", "kind": "note", "level": "L2", "room": "214", "at": [10, 5, 12], "text": "sleeve moved 6\" east", "when": "2026-03-04T10:00"},
            {"id": "FN-tab1-0002", "kind": "verified_point", "level": "L2", "at": [12, 0, 12], "text": "stud", "when": "2026-03-04T10:05"}
        ]});
        let r = s.execute(CMD_SYNC, &json!({"notes": notes})).unwrap();
        assert_eq!((r["added"].as_u64(), r["total"].as_u64()), (Some(2), Some(2)));
        let again = s.execute(CMD_SYNC, &json!({"notes": notes})).unwrap();
        assert_eq!((again["added"].as_u64(), again["unchanged"].as_u64()), (Some(0), Some(2)));
        assert_eq!(s.execute(CMD_NOTES, &json!({"room": "214"})).unwrap()["notes"].as_array().unwrap().len(), 1);
        // The next package carries them, already synced; the verified point is placed in the room.
        let r = s.execute(CMD_EXPORT, &json!({"inline": true})).unwrap();
        let pkg: FieldPackage = serde_json::from_value(r["package"].clone()).unwrap();
        assert_eq!(pkg.notes.len(), 2);
        assert!(pkg.unsynced().is_empty());
        assert_eq!(pkg.notes.iter().find(|n| n.id == "FN-tab1-0002").map(|n| n.room.as_str()), Some(""), "kept as the device sent it");
        // Through a file, too.
        let dir = std::env::temp_dir().join(format!("cadaddy-fieldnotes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.json");
        std::fs::write(&path, r#"{"schema":"cadaddy-fieldnotes/1","notes":[{"id":"FN-ph2-0001","level":"L2","at":[1,1,12],"text":"box at 18 in"}]}"#)
            .unwrap();
        let r = s.execute(CMD_SYNC, &json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(r["added"], 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = session();
        for p in [
            json!({}),
            json!({"notes": 5}),
            json!({"notes": {"schema": "cadaddy-fieldnotes/9"}}),
            json!({"notes": {"notes": [{"id": "x", "at": [1e308, -1e308, 0], "text": "a"}]}}),
            json!({"path": "/nonexistent/dir/notes.json"}),
        ] {
            let _ = s.execute(CMD_SYNC, &p);
        }
        for p in [json!({"path": "/nonexistent/dir/x.json"}), json!({"project": 7, "inline": "yes"})] {
            let _ = s.execute(CMD_EXPORT, &p);
        }
        let _ = s.execute(CMD_NOTES, &json!({"level": [1]}));
        // An empty drawing exports an empty package with a hint.
        let mut empty = Session::new();
        let r = empty.execute(CMD_EXPORT, &json!({})).unwrap();
        assert_eq!(r["elements"], 0);
        assert!(r["hint"].as_str().is_some());
    }
}
