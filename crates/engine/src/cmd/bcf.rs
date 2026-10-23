//! BCF issues on an IFC model (BIM Collaboration Format 2.1): a topic raised from a view — its
//! camera, the selected objects (by IFC GlobalId), a comment and a snapshot — that people pass
//! back and forth with a status and an assignee, in CADaddy or another program.
//!
//! - `bcf.create {title, …}`: a topic from the current view and selection.
//! - `bcf.list`, `bcf.get {topic}`: the topics; one with its comments and viewpoints.
//! - `bcf.update {topic, status?, assignee?, priority?, type?, title?, description?, comment?}`.
//! - `bcf.show {topic, viewpoint?}`: back to its view, its objects selected.
//! - `bcf.delete {topic}`.
//! - `bcf.export {path, topics?}`, `bcf.import {path}`: `.bcf`/`.bcfzip` files (BCF 2.1), ours or
//!   another program's; an imported topic already here is updated when it was modified later, and
//!   its comments are merged.
//!
//! Topics live in the drawing ([`Drawing::bcf`], JSON), saved with it (`.cdy` and DXF). Camera
//! positions are kept as BCF keeps them: metres, in the model's coordinates.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use cadaddy_doc::Drawing;
use cadaddy_geom::Vec3;

use super::file::{base64_decode, base64_encode};
use super::ifcmodel::{self, Element};
use super::*;
use crate::{Result, Session};

/// Most topics a drawing keeps, comments a topic keeps, and objects a viewpoint selects.
const MAX_TOPICS: usize = 2_000;
const MAX_COMMENTS: usize = 500;
const MAX_SELECTED: usize = 20_000;
/// Longest text kept (a title, a comment) and largest snapshot (bytes).
const MAX_TEXT: usize = 8_000;
const MAX_SNAPSHOT: usize = 4 << 20;
/// Largest BCF file read, and XML file inside it.
const MAX_FILE: usize = 256 << 20;
const MAX_XML: usize = 16 << 20;
/// The statuses offered (others read from files are kept as they are).
pub const STATUSES: [&str; 4] = ["Open", "In Progress", "Resolved", "Closed"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Topics {
    #[serde(default)]
    pub topics: Vec<Topic>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub guid: String,
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub topic_type: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub modified: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub viewpoints: Vec<Viewpoint>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub guid: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewpoint: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Viewpoint {
    pub guid: String,
    /// Camera position, direction and up vector (metres, model coordinates).
    pub eye: [f64; 3],
    pub dir: [f64; 3],
    pub up: [f64; 3],
    /// Perspective: the field of view (degrees); orthogonal: the view's height (metres).
    #[serde(default)]
    pub fov: f64,
    #[serde(default)]
    pub ortho: bool,
    /// The selected objects' IFC GlobalIds.
    #[serde(default)]
    pub selection: Vec<String>,
    /// The snapshot: PNG or JPEG bytes, base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub snapshot_type: String,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bcf.create", "New BCF Issue", run_create)
            .params("{title, comment?, description?, type? (Issue), status? (Open), priority?, assignee?, labels?: [text], author?, selection?: [handle|GlobalId] (default the selected objects), eye?, target? (default the camera last set, else the view), snapshot?: bool (default true), width?, height? (640×480)} → {guid, index, title, status, selected, snapshot}"),
        CommandSpec::new("bcf.list", "BCF Issues", run_list)
            .params("{status?, assignee?, text?} → {topics: [{guid, index, title, status, type, priority, assignee, author, created, modified, comments, selected, snapshot}], total}")
            .noundo(),
        CommandSpec::new("bcf.get", "BCF Issue", run_get)
            .params("{topic: GlobalId or index, snapshot?: bool (include the snapshot as base64)} → the topic with its comments and viewpoints")
            .noundo(),
        CommandSpec::new("bcf.update", "Update BCF Issue", run_update)
            .params("{topic, status?, assignee?, priority?, type?, title?, description?, comment?: text (added), author?} → {guid, status, assignee, comments, changed}"),
        CommandSpec::new("bcf.show", "Show BCF Issue", run_show)
            .params("{topic, viewpoint?: index (default the first)} → {guid, selected: [handle], missing: [GlobalId], eye?, target?} — its camera and its objects selected")
            .noundo(),
        CommandSpec::new("bcf.delete", "Delete BCF Issue", run_delete).params("{topic} → {deleted}"),
        CommandSpec::new("bcf.export", "Export BCF", run_export)
            .menu(&["File", "Export", "BCF Issues..."])
            .params("{path: .bcf file, topics?: [GlobalId or index] (default all)} → {path, topics}")
            .noundo(),
        CommandSpec::new("bcf.import", "Import BCF", run_import)
            .menu(&["File", "Import", "BCF Issues..."])
            .params("{path: .bcf/.bcfzip file (BCF 2.1 or 2.0)} | {data: base64} → {added, updated, unchanged, topics, skipped: [reason]}"),
    ]
}

// ---------------- the topics in a drawing ----------------

pub fn load(d: &Drawing) -> Topics {
    d.bcf.as_deref().and_then(|t| serde_json::from_str::<Topics>(t).ok()).unwrap_or_default()
}

fn save(s: &mut Session, t: &Topics) -> Result<()> {
    let text = serde_json::to_string(t).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    s.doc_mut()?.bcf = if t.topics.is_empty() { None } else { Some(text) };
    Ok(())
}

/// The topic `q` names: its GlobalId (or the start of it, when that is unique), else its index.
fn find(cmd: &str, t: &Topics, q: Option<&str>) -> Result<usize> {
    let q = q.map(str::trim).filter(|q| !q.is_empty()).ok_or_else(|| bad(cmd, "topic required (its GlobalId or index)"))?;
    if let Some(i) = t.topics.iter().position(|x| x.guid.eq_ignore_ascii_case(q)) {
        return Ok(i);
    }
    if let Ok(n) = q.trim_start_matches('#').parse::<u32>()
        && let Some(i) = t.topics.iter().position(|x| x.index == n)
    {
        return Ok(i);
    }
    let starts: Vec<usize> =
        t.topics.iter().enumerate().filter(|(_, x)| q.len() >= 4 && x.guid.to_lowercase().starts_with(&q.to_lowercase())).map(|(i, _)| i).collect();
    match starts[..] {
        [i] => Ok(i),
        _ => Err(bad(cmd, format!("no BCF topic {}", q.chars().take(40).collect::<String>()))),
    }
}

fn topic_param(p: &Value) -> Option<&str> {
    str_param(p, "topic").or_else(|| str_param(p, "guid"))
}

fn text(p: &Value, key: &str) -> Option<String> {
    str_param(p, key).map(|t| t.trim().chars().take(MAX_TEXT).collect::<String>())
}

/// Now, as BCF dates are written (UTC).
pub(crate) fn now() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A new GUID (version 4 form) for a topic, comment or viewpoint.
pub(crate) fn new_guid(salt: &str) -> String {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |t| t.as_nanos()).hash(&mut h);
    COUNT.fetch_add(1, Ordering::Relaxed).hash(&mut h);
    salt.hash(&mut h);
    let a = h.finish();
    a.hash(&mut h);
    let b = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        a as u32,
        (a >> 32) as u16,
        (a >> 48) & 0xfff,
        8 | ((b >> 60) & 3),
        (b >> 48) & 0xfff,
        b & 0xffff_ffff_ffff
    )
}

fn summary(t: &Topic) -> Value {
    json!({
        "guid": t.guid, "index": t.index, "title": t.title, "status": t.status, "type": t.topic_type, "priority": t.priority,
        "assignee": t.assignee, "author": t.author, "created": t.created, "modified": t.modified,
        "comments": t.comments.len(), "selected": t.viewpoints.first().map_or(0, |v| v.selection.len()),
        "snapshot": t.viewpoints.iter().any(|v| v.snapshot.is_some()),
    })
}

// ---------------- creating ----------------

fn vec3_param(cmd: &str, p: &Value, key: &str) -> Result<Option<Vec3>> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let n: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    match n[..] {
        [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() && x.abs() < 1e12 && y.abs() < 1e12 && z.abs() < 1e12 => {
            Ok(Some(Vec3::new(x, y, z)))
        }
        _ => Err(bad(cmd, format!("`{key}` must be [x, y, z]"))),
    }
}

fn unit(v: Vec3) -> Vec3 {
    let l = v.len();
    if l > 1e-12 { v * (1.0 / l) } else { Vec3::new(0.0, 0.0, -1.0) }
}

/// A camera's up vector: square to the view direction, as near +Z as it can be.
fn up_of(dir: Vec3) -> Vec3 {
    let d = unit(dir);
    let up = Vec3::new(0.0, 0.0, 1.0) - d * d.z;
    if up.len() > 1e-6 { unit(up) } else { Vec3::new(0.0, 1.0, 0.0) }
}

fn run_create(s: &mut Session, p: &Value) -> Result<Value> {
    let title = text(p, "title").filter(|t| !t.is_empty()).ok_or_else(|| bad("bcf.create", "title required"))?;
    let mut topics = load(s.doc()?);
    if topics.topics.len() >= MAX_TOPICS {
        return Err(bad("bcf.create", format!("a drawing keeps at most {MAX_TOPICS} BCF topics")));
    }
    let idx = ifcmodel::cached_index(s)?;
    let upm = super::ifcref::units_per_metre(s.doc()?);
    // The objects: given (handles or GlobalIds), else the selection. Map-based lookup.
    let picked_indices: Vec<usize> = match p.get("selection").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_str)
            .take(MAX_SELECTED)
            .filter_map(|q| idx.by_guid_or_handle(q.trim()))
            .collect(),
        None => {
            let sel = s.selection();
            sel.iter().filter_map(|h| idx.by_handle(*h)).take(MAX_SELECTED).collect()
        }
    };
    let picked: Vec<&Element> = picked_indices.iter().filter_map(|i| idx.els.get(*i)).collect();
    // The camera: given, the one last set, else along the view at the selection (or everything).
    let (eye, target) = match (vec3_param("bcf.create", p, "eye")?, vec3_param("bcf.create", p, "target")?) {
        (Some(e), Some(t)) if (e - t).len() > 1e-9 => (e, t),
        (Some(_), _) | (_, Some(_)) => return Err(bad("bcf.create", "a camera needs both `eye` and `target`, apart")),
        (None, None) => match s.state()?.ifc_view.camera {
            Some(c) => c,
            None => {
                let look: Vec<Vec3> = if picked.is_empty() {
                    idx.els.iter().flat_map(|e| [e.min, e.max]).collect()
                } else {
                    picked.iter().flat_map(|e| [e.min, e.max]).collect()
                };
                let lo = look
                    .iter()
                    .fold(Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), |a, b| Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)));
                let hi = look.iter().fold(Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |a, b| {
                    Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
                });
                let (lo, hi) = if look.is_empty() { (Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)) } else { (lo, hi) };
                let target = (lo + hi) * 0.5;
                let back = unit(s.state()?.view_dir);
                let back = if back.x == 0.0 && back.y == 0.0 { unit(Vec3::new(-1.0, -1.0, 0.8)) } else { back };
                (target + back * ((hi - lo).len().max(upm) * 1.5), target)
            }
        },
    };
    let snapshot = if bool_or(p, "snapshot", true) && !idx.els.is_empty() {
        let dim = |k: &str, def: u32| p.get(k).and_then(Value::as_u64).map_or(def, |v| u32::try_from(v).unwrap_or(2048).clamp(16, 2048));
        let keep = s.selection();
        s.set_selection(picked.iter().map(|e| e.handle).collect());
        let shot = ifcmodel::render_png(
            s,
            Some(eye),
            Some(target),
            None,
            (dim("width", 640), dim("height", 480)),
            cadaddy_render::shade::VisualStyle::ShadedEdges,
            true,
        );
        s.set_selection(keep);
        shot.ok().map(|(png, _)| png).filter(|png| png.len() <= MAX_SNAPSHOT)
    } else {
        None
    };
    let author = text(p, "author").filter(|a| !a.is_empty()).unwrap_or_else(|| "CADaddy".into());
    let date = now();
    let guid = new_guid(&title);
    let vp = Viewpoint {
        guid: new_guid("viewpoint"),
        eye: [eye.x / upm, eye.y / upm, eye.z / upm],
        dir: {
            let f = unit(target - eye);
            [f.x, f.y, f.z]
        },
        up: {
            let u = up_of(target - eye);
            [u.x, u.y, u.z]
        },
        fov: 60.0,
        ortho: false,
        selection: picked.iter().map(|e| e.tag.guid.clone()).filter(|g| !g.is_empty()).collect(),
        snapshot_type: if snapshot.is_some() { "png".into() } else { String::new() },
        snapshot: snapshot.map(|b| base64_encode(&b)),
    };
    let mut t = Topic {
        guid: guid.clone(),
        title: title.clone(),
        status: text(p, "status").filter(|x| !x.is_empty()).unwrap_or_else(|| "Open".into()),
        topic_type: text(p, "type").filter(|x| !x.is_empty()).unwrap_or_else(|| "Issue".into()),
        priority: text(p, "priority").unwrap_or_default(),
        assignee: text(p, "assignee").unwrap_or_default(),
        author: author.clone(),
        created: date.clone(),
        modified: date.clone(),
        description: text(p, "description").unwrap_or_default(),
        labels: p
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).take(50).map(|l| l.chars().take(200).collect()).collect())
            .unwrap_or_default(),
        index: topics.topics.iter().map(|x| x.index).max().unwrap_or(0).saturating_add(1),
        comments: Vec::new(),
        viewpoints: vec![vp],
    };
    if let Some(c) = text(p, "comment").filter(|c| !c.is_empty()) {
        let viewpoint = t.viewpoints.first().map(|v| v.guid.clone());
        t.comments.push(Comment { guid: new_guid(&c), date, author, text: c, viewpoint });
    }
    let out = json!({
        "guid": guid, "index": t.index, "title": title, "status": t.status,
        "selected": t.viewpoints.first().map_or(0, |v| v.selection.len()), "snapshot": t.viewpoints.iter().any(|v| v.snapshot.is_some()),
    });
    topics.topics.push(t);
    save(s, &topics)?;
    Ok(out)
}

// ---------------- reading and changing ----------------

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let topics = load(s.doc()?);
    let status = str_param(p, "status").map(str::trim).filter(|x| !x.is_empty());
    let assignee = str_param(p, "assignee").map(str::trim).filter(|x| !x.is_empty());
    let q = str_param(p, "text").map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty());
    let list: Vec<Value> = topics
        .topics
        .iter()
        .filter(|t| status.is_none_or(|x| t.status.eq_ignore_ascii_case(x)))
        .filter(|t| assignee.is_none_or(|x| t.assignee.eq_ignore_ascii_case(x)))
        .filter(|t| {
            q.as_ref().is_none_or(|q| {
                t.title.to_lowercase().contains(q)
                    || t.description.to_lowercase().contains(q)
                    || t.comments.iter().any(|c| c.text.to_lowercase().contains(q))
            })
        })
        .map(summary)
        .collect();
    Ok(json!({ "total": list.len(), "topics": list }))
}

fn run_get(s: &mut Session, p: &Value) -> Result<Value> {
    let topics = load(s.doc()?);
    let i = find("bcf.get", &topics, topic_param(p))?;
    let mut t = topics.topics.get(i).cloned().ok_or_else(|| bad("bcf.get", "no such topic"))?;
    if !bool_or(p, "snapshot", false) {
        for v in &mut t.viewpoints {
            v.snapshot = v.snapshot.as_ref().map(|_| "(left out: snapshot: true includes it)".into());
        }
    }
    serde_json::to_value(&t).map_err(|e| crate::EngineError::Other(e.to_string()))
}

fn run_update(s: &mut Session, p: &Value) -> Result<Value> {
    let mut topics = load(s.doc()?);
    let i = find("bcf.update", &topics, topic_param(p))?;
    let Some(t) = topics.topics.get_mut(i) else { return Err(bad("bcf.update", "no such topic")) };
    let mut changed = false;
    for (key, field) in [
        ("status", &mut t.status),
        ("assignee", &mut t.assignee),
        ("priority", &mut t.priority),
        ("type", &mut t.topic_type),
        ("title", &mut t.title),
        ("description", &mut t.description),
    ] {
        if let Some(v) = text(p, key)
            && *field != v
        {
            if key == "title" && v.is_empty() {
                return Err(bad("bcf.update", "a topic's title can't be empty"));
            }
            *field = v;
            changed = true;
        }
    }
    let author = text(p, "author").filter(|a| !a.is_empty()).unwrap_or_else(|| "CADaddy".into());
    if let Some(c) = text(p, "comment").filter(|c| !c.is_empty()) {
        if t.comments.len() >= MAX_COMMENTS {
            return Err(bad("bcf.update", format!("a topic keeps at most {MAX_COMMENTS} comments")));
        }
        t.comments.push(Comment { guid: new_guid(&c), date: now(), author, text: c, viewpoint: None });
        changed = true;
    }
    if changed {
        t.modified = now();
    }
    let out = json!({ "guid": t.guid, "status": t.status, "assignee": t.assignee, "comments": t.comments.len(), "changed": changed });
    if changed {
        save(s, &topics)?;
    }
    Ok(out)
}

fn run_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let mut topics = load(s.doc()?);
    let i = find("bcf.delete", &topics, topic_param(p))?;
    let t = topics.topics.remove(i);
    save(s, &topics)?;
    Ok(json!({ "deleted": t.guid }))
}

fn run_show(s: &mut Session, p: &Value) -> Result<Value> {
    let topics = load(s.doc()?);
    let i = find("bcf.show", &topics, topic_param(p))?;
    let t = topics.topics.get(i).ok_or_else(|| bad("bcf.show", "no such topic"))?;
    let k = p.get("viewpoint").and_then(Value::as_u64).map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let Some(vp) = t.viewpoints.get(k) else {
        return Ok(json!({ "guid": t.guid, "selected": [], "missing": [], "message": "the topic has no viewpoint" }));
    };
    let idx = ifcmodel::cached_index(s)?;
    let upm = super::ifcref::units_per_metre(s.doc()?);
    let (mut selected, mut missing) = (Vec::new(), Vec::new());
    for g in &vp.selection {
        match idx.by_guid(g) {
            Some(i) => {
                if let Some(e) = idx.els.get(i) {
                    selected.push(e.handle);
                }
            }
            None => missing.push(g.clone()),
        }
    }
    let mut out = json!({ "guid": t.guid, "selected": selected.iter().map(|h| h.hex()).collect::<Vec<_>>(), "missing": missing });
    let eye = Vec3::new(vp.eye[0] * upm, vp.eye[1] * upm, vp.eye[2] * upm);
    let dir = unit(Vec3::new(vp.dir[0], vp.dir[1], vp.dir[2]));
    if eye.x.is_finite() && eye.y.is_finite() && eye.z.is_finite() && dir.len() > 0.5 {
        // Fit to the selection's bounds (padded) along eye→target, not the whole model.
        let sel_els: Vec<&Element> = selected.iter().filter_map(|h| idx.by_handle(*h).and_then(|i| idx.els.get(i))).collect();
        let target = if sel_els.is_empty() {
            // No selection: look as far as the model centre.
            let centre = if idx.els.is_empty() {
                eye + dir * upm
            } else {
                idx.els.iter().fold(Vec3::ZERO, |a, e| a + (e.min + e.max) * 0.5) * (1.0 / idx.els.len() as f64)
            };
            let reach = (centre - eye).dot(dir).max(upm);
            eye + dir * reach
        } else {
            // Selection: target = centre of selected objects' bounds.
            let pts: Vec<Vec3> = sel_els.iter().flat_map(|e| [e.min, e.max]).collect();
            let lo = pts.iter().fold(Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), |a, b| {
                Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z))
            });
            let hi = pts.iter().fold(Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |a, b| {
                Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
            });
            let centre = (lo + hi) * 0.5;
            let reach = (centre - eye).dot(dir).max(upm);
            eye + dir * reach
        };
        ifcmodel::set_camera(s, eye, target)?;
        out["eye"] = json!([eye.x, eye.y, eye.z]);
        out["target"] = json!([target.x, target.y, target.z]);
    }
    s.set_selection(selected);
    Ok(out)
}

// ---------------- BCF files ----------------

fn esc(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn xyz(tag: &str, v: [f64; 3]) -> String {
    let n = |x: f64| if x.is_finite() { x } else { 0.0 };
    format!("<{tag}><X>{}</X><Y>{}</Y><Z>{}</Z></{tag}>", n(v[0]), n(v[1]), n(v[2]))
}

fn markup(t: &Topic) -> String {
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Markup>\n");
    x += &format!("  <Topic Guid=\"{}\" TopicType=\"{}\" TopicStatus=\"{}\">\n", esc(&t.guid), esc(&t.topic_type), esc(&t.status));
    x += &format!("    <Title>{}</Title>\n", esc(&t.title));
    if !t.priority.is_empty() {
        x += &format!("    <Priority>{}</Priority>\n", esc(&t.priority));
    }
    if t.index > 0 {
        x += &format!("    <Index>{}</Index>\n", t.index);
    }
    for l in &t.labels {
        x += &format!("    <Labels>{}</Labels>\n", esc(l));
    }
    x += &format!("    <CreationDate>{}</CreationDate>\n    <CreationAuthor>{}</CreationAuthor>\n", esc(&t.created), esc(&t.author));
    if !t.modified.is_empty() {
        x += &format!("    <ModifiedDate>{}</ModifiedDate>\n", esc(&t.modified));
    }
    if !t.assignee.is_empty() {
        x += &format!("    <AssignedTo>{}</AssignedTo>\n", esc(&t.assignee));
    }
    if !t.description.is_empty() {
        x += &format!("    <Description>{}</Description>\n", esc(&t.description));
    }
    x += "  </Topic>\n";
    for c in &t.comments {
        x += &format!(
            "  <Comment Guid=\"{}\">\n    <Date>{}</Date>\n    <Author>{}</Author>\n    <Comment>{}</Comment>\n",
            esc(&c.guid),
            esc(&c.date),
            esc(&c.author),
            esc(&c.text)
        );
        if let Some(v) = &c.viewpoint {
            x += &format!("    <Viewpoint Guid=\"{}\"/>\n", esc(v));
        }
        x += "  </Comment>\n";
    }
    for (k, v) in t.viewpoints.iter().enumerate() {
        let name = if k == 0 { "viewpoint".to_string() } else { format!("viewpoint{k}") };
        x += &format!("  <Viewpoints Guid=\"{}\">\n    <Viewpoint>{name}.bcfv</Viewpoint>\n", esc(&v.guid));
        if v.snapshot.is_some() {
            x += &format!(
                "    <Snapshot>snapshot{}.{}</Snapshot>\n",
                if k == 0 { String::new() } else { k.to_string() },
                if v.snapshot_type == "jpg" { "jpg" } else { "png" }
            );
        }
        x += "  </Viewpoints>\n";
    }
    x + "</Markup>\n"
}

fn visinfo(v: &Viewpoint) -> String {
    let mut x = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<VisualizationInfo Guid=\"{}\">\n  <Components>\n", esc(&v.guid));
    if !v.selection.is_empty() {
        x += "    <Selection>\n";
        for g in &v.selection {
            x += &format!("      <Component IfcGuid=\"{}\"/>\n", esc(g));
        }
        x += "    </Selection>\n";
    }
    x += "    <Visibility DefaultVisibility=\"true\"/>\n  </Components>\n";
    let cam = format!("{}{}{}", xyz("CameraViewPoint", v.eye), xyz("CameraDirection", v.dir), xyz("CameraUpVector", v.up));
    if v.ortho {
        x += &format!(
            "  <OrthogonalCamera>{cam}<ViewToWorldScale>{}</ViewToWorldScale></OrthogonalCamera>\n",
            if v.fov.is_finite() && v.fov > 0.0 { v.fov } else { 10.0 }
        );
    } else {
        x += &format!(
            "  <PerspectiveCamera>{cam}<FieldOfView>{}</FieldOfView></PerspectiveCamera>\n",
            if v.fov.is_finite() && v.fov > 0.0 { v.fov } else { 60.0 }
        );
    }
    x + "</VisualizationInfo>\n"
}

/// The BCF 2.1 archive of `topics`.
pub fn write_bcf(topics: &[&Topic]) -> std::result::Result<Vec<u8>, String> {
    let mut files: Vec<(String, Vec<u8>)> = vec![(
        "bcf.version".into(),
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Version VersionId=\"2.1\">\n  <DetailedVersion>2.1</DetailedVersion>\n</Version>\n".to_vec(),
    )];
    for t in topics {
        let dir = t.guid.replace(['/', '\\', '.'], "_");
        files.push((format!("{dir}/markup.bcf"), markup(t).into_bytes()));
        for (k, v) in t.viewpoints.iter().enumerate() {
            let name = if k == 0 { "viewpoint".to_string() } else { format!("viewpoint{k}") };
            files.push((format!("{dir}/{name}.bcfv"), visinfo(v).into_bytes()));
            if let Some(b) = v.snapshot.as_deref().and_then(base64_decode) {
                let ext = if v.snapshot_type == "jpg" { "jpg" } else { "png" };
                files.push((format!("{dir}/snapshot{}.{ext}", if k == 0 { String::new() } else { k.to_string() }), b));
            }
        }
    }
    super::file::zip::write(&files)
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("bcf.export", "path required"))?;
    let topics = load(s.doc()?);
    let pick: Vec<&Topic> = match p.get("topics").and_then(Value::as_array) {
        Some(a) => {
            let mut v = Vec::new();
            for q in a.iter().filter_map(|q| q.as_str().map(str::to_string).or_else(|| q.as_u64().map(|n| n.to_string()))) {
                let i = find("bcf.export", &topics, Some(&q))?;
                if let Some(t) = topics.topics.get(i) {
                    v.push(t);
                }
            }
            v
        }
        None => topics.topics.iter().collect(),
    };
    if pick.is_empty() {
        return Err(bad("bcf.export", "there are no BCF topics to export"));
    }
    let bytes = write_bcf(&pick).map_err(|e| bad("bcf.export", e))?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
        Ok(json!({ "path": path, "topics": pick.len(), "bytes": bytes.len() }))
    }
    #[cfg(target_arch = "wasm32")]
    Ok(json!({ "path": path, "topics": pick.len(), "data": base64_encode(&bytes) }))
}

// A small XML reader for BCF's markup and viewpoint files: elements by name (nested ones of the
// same name matched), attributes, text with the entities decoded. Bounded; never panics.

/// The elements named `name` at the outermost level of `xml`: (attributes text, inner text).
fn elements<'a>(xml: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let starts_tag = |i: usize| xml.get(i + open.len()..).and_then(|r| r.chars().next()).is_some_and(|c| c == '>' || c == '/' || c.is_whitespace());
    while out.len() < 100_000 {
        let Some(i) = xml.get(at..).and_then(|r| r.find(&open)).map(|i| i + at) else { break };
        if !starts_tag(i) {
            at = i + open.len();
            continue;
        }
        let Some(gt) = xml.get(i..).and_then(|r| r.find('>')).map(|g| g + i) else { break };
        let head = xml.get(i + open.len()..gt).unwrap_or_default();
        if head.ends_with('/') {
            out.push((head.trim_end_matches('/'), ""));
            at = gt + 1;
            continue;
        }
        // The matching close: same-name elements inside count.
        let mut depth = 1usize;
        let mut k = gt + 1;
        let mut end = None;
        for _ in 0..100_000 {
            let next_open = xml.get(k..).and_then(|r| r.find(&open)).map(|j| j + k);
            let next_close = xml.get(k..).and_then(|r| r.find(&close)).map(|j| j + k);
            match (next_open, next_close) {
                (Some(o), Some(c)) if o < c => {
                    let self_closing =
                        xml.get(o..).and_then(|r| r.find('>')).map(|g| g + o).is_some_and(|g| xml.get(g.saturating_sub(1)..g) == Some("/"));
                    if starts_tag(o) && !self_closing {
                        depth += 1;
                    }
                    k = o + open.len();
                }
                (_, Some(c)) => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(c);
                        break;
                    }
                    k = c + close.len();
                }
                _ => break,
            }
        }
        let Some(c) = end else { break };
        out.push((head, xml.get(gt + 1..c).unwrap_or_default()));
        at = c + close.len();
    }
    out
}

fn first<'a>(xml: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    elements(xml, name).into_iter().next()
}

fn attr(head: &str, key: &str) -> Option<String> {
    let mut rest = head;
    for _ in 0..64 {
        let i = rest.find(key)?;
        let before_ok = i == 0 || rest.get(i - 1..i).is_some_and(|c| c.chars().all(char::is_whitespace));
        let after = rest.get(i + key.len()..)?.trim_start();
        if before_ok && let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let body = v.get(1..)?;
                let end = body.find(q)?;
                return Some(unescape(body.get(..end)?));
            }
        }
        rest = rest.get(i + key.len()..)?;
    }
    None
}

fn unescape(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").and_then(|x| x.strip_suffix("]]>")).map_or_else(|| s.to_string(), str::to_string);
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let tail = rest.get(i..).unwrap_or_default();
        let Some(semi) = tail.find(';').filter(|j| *j <= 10) else {
            out.push('&');
            rest = tail.get(1..).unwrap_or_default();
            continue;
        };
        let ent = tail.get(1..semi).unwrap_or_default();
        let ch = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(tail.get(..=semi).unwrap_or_default()),
        }
        rest = tail.get(semi + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

fn child_text(xml: &str, name: &str) -> String {
    first(xml, name).map(|(_, inner)| unescape(inner).chars().take(MAX_TEXT).collect()).unwrap_or_default()
}

fn xyz_of(xml: &str, name: &str) -> Option<[f64; 3]> {
    let (_, inner) = first(xml, name)?;
    let n = |k: &str| child_text(inner, k).trim().parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() < 1e9);
    Some([n("X")?, n("Y")?, n("Z")?])
}

fn read_viewpoint(xml: &str, guid: &str) -> Option<Viewpoint> {
    let (head, body) = first(xml, "VisualizationInfo")?;
    let mut v = Viewpoint { guid: attr(head, "Guid").filter(|g| !g.is_empty()).unwrap_or_else(|| guid.to_string()), ..Viewpoint::default() };
    if let Some((_, comps)) = first(body, "Components")
        && let Some((_, sel)) = first(comps, "Selection")
    {
        v.selection =
            elements(sel, "Component").into_iter().filter_map(|(h, _)| attr(h, "IfcGuid")).filter(|g| !g.is_empty()).take(MAX_SELECTED).collect();
    }
    let (cam, ortho) = match first(body, "PerspectiveCamera") {
        Some((_, c)) => (Some(c), false),
        None => (first(body, "OrthogonalCamera").map(|(_, c)| c), true),
    };
    if let Some(c) = cam {
        v.eye = xyz_of(c, "CameraViewPoint")?;
        v.dir = xyz_of(c, "CameraDirection")?;
        v.up = xyz_of(c, "CameraUpVector").unwrap_or([0.0, 0.0, 1.0]);
        v.ortho = ortho;
        let f =
            child_text(c, if ortho { "ViewToWorldScale" } else { "FieldOfView" }).trim().parse::<f64>().ok().filter(|f| f.is_finite() && *f > 0.0);
        v.fov = f.unwrap_or(if ortho { 10.0 } else { 60.0 });
    }
    Some(v)
}

/// The topics of a BCF archive (2.1 or 2.0), and why any were left out.
pub fn read_bcf(bytes: &[u8]) -> std::result::Result<(Vec<Topic>, Vec<String>), String> {
    if bytes.len() > MAX_FILE {
        return Err("the BCF file is too large".into());
    }
    let files = super::file::zip::read(bytes).map_err(|e| format!("not a BCF file: {e}"))?;
    let get = |name: &str| files.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, b)| b);
    let xml = |b: &Vec<u8>| -> Option<String> {
        (b.len() <= MAX_XML).then(|| String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(b)).into_owned())
    };
    if let Some(v) = get("bcf.version").and_then(xml)
        && let Some((head, _)) = first(&v, "Version")
        && let Some(id) = attr(head, "VersionId")
        && !(id.starts_with("2.0") || id.starts_with("2.1"))
    {
        return Err(format!("BCF {id} isn't read yet (2.0 and 2.1 are)"));
    }
    let mut topics = Vec::new();
    let mut skipped = Vec::new();
    for (name, b) in &files {
        let Some(dir) = name.strip_suffix("/markup.bcf").or_else(|| name.strip_suffix("\\markup.bcf")) else { continue };
        if topics.len() >= MAX_TOPICS {
            skipped.push(format!("more than {MAX_TOPICS} topics: the rest were left out"));
            break;
        }
        let Some(m) = xml(b) else {
            skipped.push(format!("{dir}: the markup is too large"));
            continue;
        };
        let Some((th, tb)) = first(&m, "Topic") else {
            skipped.push(format!("{dir}: no Topic in its markup"));
            continue;
        };
        let guid = attr(th, "Guid").filter(|g| !g.is_empty()).unwrap_or_else(|| dir.to_string());
        let mut t = Topic {
            guid: guid.chars().take(64).collect(),
            title: child_text(tb, "Title"),
            status: attr(th, "TopicStatus").unwrap_or_default(),
            topic_type: attr(th, "TopicType").unwrap_or_default(),
            priority: child_text(tb, "Priority"),
            assignee: child_text(tb, "AssignedTo"),
            author: child_text(tb, "CreationAuthor"),
            created: child_text(tb, "CreationDate"),
            modified: child_text(tb, "ModifiedDate"),
            description: child_text(tb, "Description"),
            labels: elements(tb, "Labels").into_iter().take(50).map(|(_, l)| unescape(l)).collect(),
            index: child_text(tb, "Index").trim().parse().unwrap_or(0),
            comments: Vec::new(),
            viewpoints: Vec::new(),
        };
        if t.title.is_empty() {
            t.title = "(untitled)".into();
        }
        for (ch, cb) in elements(&m, "Comment").into_iter().take(MAX_COMMENTS) {
            t.comments.push(Comment {
                guid: attr(ch, "Guid").unwrap_or_else(|| new_guid("comment")),
                date: child_text(cb, "Date"),
                author: child_text(cb, "Author"),
                text: child_text(cb, "Comment"),
                viewpoint: first(cb, "Viewpoint").and_then(|(h, _)| attr(h, "Guid")),
            });
        }
        // Viewpoints: listed in the markup, else the topic folder's viewpoint.bcfv (BCF 2.0).
        let mut listed: Vec<(String, String, String)> = elements(&m, "Viewpoints")
            .into_iter()
            .take(100)
            .map(|(h, b)| (attr(h, "Guid").unwrap_or_default(), child_text(b, "Viewpoint"), child_text(b, "Snapshot")))
            .collect();
        if listed.is_empty() && get(&format!("{dir}/viewpoint.bcfv")).is_some() {
            listed.push((
                String::new(),
                "viewpoint.bcfv".into(),
                if get(&format!("{dir}/snapshot.png")).is_some() { "snapshot.png".into() } else { String::new() },
            ));
        }
        for (vg, file, snap) in listed {
            let file = file.trim().trim_start_matches("./");
            let Some(mut v) = get(&format!("{dir}/{file}")).and_then(xml).and_then(|x| read_viewpoint(&x, &vg)) else {
                skipped.push(format!("{}: viewpoint {file} couldn't be read", t.title));
                continue;
            };
            if !vg.is_empty() {
                v.guid = vg;
            }
            let snap = snap.trim().trim_start_matches("./");
            if !snap.is_empty()
                && let Some(img) = get(&format!("{dir}/{snap}"))
            {
                let kind = if img.starts_with(&[0x89, b'P', b'N', b'G']) {
                    "png"
                } else if img.starts_with(&[0xFF, 0xD8]) {
                    "jpg"
                } else {
                    ""
                };
                if kind.is_empty() || img.len() > MAX_SNAPSHOT {
                    skipped.push(format!("{}: its snapshot isn't a PNG or JPEG under 4 MB", t.title));
                } else {
                    v.snapshot = Some(base64_encode(img));
                    v.snapshot_type = kind.into();
                }
            }
            t.viewpoints.push(v);
        }
        topics.push(t);
    }
    if topics.is_empty() && skipped.is_empty() {
        return Err("the file holds no BCF topics".into());
    }
    Ok((topics, skipped))
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let bytes = match (str_param(p, "path"), str_param(p, "data")) {
        (Some(path), _) => crate::readfile::read(path.trim()).map_err(|e| bad("bcf.import", format!("{path}: {e}")))?,
        (None, Some(data)) => base64_decode(data).ok_or_else(|| bad("bcf.import", "data isn't base64"))?,
        _ => return Err(bad("bcf.import", "path or data required")),
    };
    let (incoming, skipped) = read_bcf(&bytes).map_err(|e| bad("bcf.import", e))?;
    let mut topics = load(s.doc()?);
    let (mut added, mut updated, mut unchanged) = (0usize, 0usize, 0usize);
    for mut t in incoming {
        let at = topics.topics.iter().position(|x| x.guid.eq_ignore_ascii_case(&t.guid));
        let n = topics.topics.len();
        let next_index = topics.topics.iter().map(|x| x.index).max().unwrap_or(0).saturating_add(1);
        let index_taken = topics.topics.iter().any(|x| x.index == t.index);
        match at.and_then(|i| topics.topics.get_mut(i)) {
            Some(have) => {
                // Comments merge (by GlobalId); the topic's fields come from whichever was modified last.
                let before = have.clone();
                for c in t.comments.drain(..) {
                    if have.comments.len() < MAX_COMMENTS && !have.comments.iter().any(|x| x.guid == c.guid) {
                        have.comments.push(c);
                    }
                }
                for v in t.viewpoints.drain(..) {
                    if !have.viewpoints.iter().any(|x| x.guid == v.guid) {
                        have.viewpoints.push(v);
                    }
                }
                let theirs = if t.modified.is_empty() { &t.created } else { &t.modified };
                let ours = if have.modified.is_empty() { &have.created } else { &have.modified };
                // A tie goes to the file: dates are to the second, and an answer can come back
                // within it.
                if theirs >= ours {
                    (have.title, have.status, have.topic_type, have.priority, have.assignee, have.description, have.modified) =
                        (t.title, t.status, t.topic_type, t.priority, t.assignee, t.description, t.modified);
                    if !t.labels.is_empty() {
                        have.labels = t.labels;
                    }
                }
                if *have == before {
                    unchanged += 1;
                } else {
                    updated += 1;
                }
            }
            None if n < MAX_TOPICS => {
                if t.index == 0 || index_taken {
                    t.index = next_index;
                }
                topics.topics.push(t);
                added += 1;
            }
            None => {}
        }
    }
    if added + updated > 0 {
        save(s, &topics)?;
    }
    Ok(json!({ "added": added, "updated": updated, "unchanged": unchanged, "topics": topics.topics.len(), "skipped": skipped }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xml_reader_matches_nested_names_and_decodes_text() {
        let x = r#"<Markup><Comment Guid="a"><Date>d</Date><Comment>one &amp; &lt;two&gt; &#233;</Comment><Viewpoint Guid='v'/></Comment><Comment Guid="b"><Comment><![CDATA[x < y]]></Comment></Comment></Markup>"#;
        let cs = elements(x, "Comment");
        assert_eq!(cs.len(), 2);
        assert_eq!(attr(cs[0].0, "Guid").as_deref(), Some("a"));
        assert_eq!(child_text(cs[0].1, "Comment"), "one & <two> é");
        assert_eq!(first(cs[0].1, "Viewpoint").and_then(|(h, _)| attr(h, "Guid")).as_deref(), Some("v"));
        assert_eq!(child_text(cs[1].1, "Comment"), "x < y");
        // Hostile: unclosed, junk entities, prefixes of other names.
        assert!(elements("<Comment><Comment>", "Comment").is_empty());
        assert_eq!(unescape("&bogus; & &#xFFFFFFFF;"), "&bogus; & &#xFFFFFFFF;");
        assert!(elements("<Comments/><CommentX>", "Comment").is_empty());
        assert_eq!(attr(r#"NotGuid="x" Guid="y""#, "Guid").as_deref(), Some("y"));
    }

    #[test]
    fn dates_and_guids() {
        let d = now();
        assert_eq!(d.len(), 20, "{d}");
        assert!(d.starts_with("20") && d.ends_with('Z'));
        let (a, b) = (new_guid("x"), new_guid("x"));
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
    }
}
