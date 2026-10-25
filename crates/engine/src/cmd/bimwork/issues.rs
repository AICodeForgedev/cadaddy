//! `bim.issue.*`: issues, RFIs and punch items on the model's elements, and `bim.field.link`.
//!
//! An issue is a BCF topic ([`crate::cmd::bcf`]) pinned to elements (their handles, and their
//! GlobalIds when they have one) and to a spot in the drawing, with an assignee, a status, a due
//! date, a viewpoint looking at the spot and a photo as its snapshot. So the same issues export
//! and import as BCF 2.1 or 3.0, show in 3D (`bcf.show`, `bim.issue.show`), and show on the plan:
//! `bim.issue.marks` draws each as a numbered mark at its spot on the sheet of its level.
//!
//! Built elements sit on the sheet's plan (the plan's coordinates, the level's elevation), so a
//! sheet point and a 3D point share x and y; `bim.field.link` turns one into the other with the
//! level and a camera.

use cadaddy_doc::{Circle, Common, EntityKind, Handle, RawTag, Space, Text};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::{El, elements, frame, handles_of, r3, units_per_foot, vec3_of, viewpoint};
use crate::cmd::bcf::{self, Topic, Viewpoint};
use crate::cmd::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const CMD_CREATE: &str = "bim.issue.create";
const CMD_LIST: &str = "bim.issue.list";
const CMD_SET: &str = "bim.issue.set";
const CMD_SHOW: &str = "bim.issue.show";
const CMD_MARKS: &str = "bim.issue.marks";
const CMD_LINK: &str = "bim.field.link";
/// The layer issue marks go on, and the xdata app that tags them.
pub const MARK_LAYER: &str = "ISSUE-MARKS";
const MARK_APP: &str = "CADADDY_ISSUE";
/// Largest photo kept (bytes) and longest text.
const MAX_PHOTO: usize = 4 << 20;
const MAX_TEXT: usize = 8_000;
/// Eye height standing on a level, feet.
const EYE_FT: f64 = 5.5;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_CREATE, "New Issue", run_create)
            .params("{title, kind?: issue|rfi|punch (default issue), description?, elements?: [handle] (default the selection), at?: [x,y,z] (default the elements' centre), level?, assignee?, status? (Open), priority?, due?: date, photo?: .png/.jpg path | photo_data?: base64, author?} → {guid, index, kind, title, status, elements, at, level, viewpoint, photo}"),
        CommandSpec::new(CMD_LIST, "Issues", run_list)
            .params("{kind?, status?, assignee?, level?, element?: handle, text?} → {issues: [{guid, index, kind, title, status, assignee, priority, due, elements, at, level, sheet: {level, at}, viewpoint, photo, comments}], total}")
            .noundo(),
        CommandSpec::new(CMD_SET, "Update Issue", run_set)
            .params("{issue: GlobalId or index, status?, assignee?, priority?, due?, title?, description?, comment?, elements?: [handle], at?: [x,y,z], photo?: path, author?} → {guid, status, assignee, changed}"),
        CommandSpec::new(CMD_SHOW, "Show Issue", run_show)
            .params("{issue} → {guid, selected, handles, viewpoint} — its elements selected and the 3D camera on its spot")
            .noundo(),
        CommandSpec::new(CMD_MARKS, "Issue Marks", run_marks)
            .params("{level?, status?: (default every status but Closed), clear?: bool (just remove the marks)} → {marks, removed, layer} — each issue as a numbered circle at its spot on the plan (layer ISSUE-MARKS), redrawn each time"),
        CommandSpec::new(CMD_LINK, "Sheet and 3D Link", run_link)
            .params("{at: [x,y] (a sheet point) | [x,y,z] (a 3D point), from?: sheet|model (default sheet for [x,y], model for [x,y,z]), level?} → {level, sheet: [x,y], model: [x,y,z], building_ft: [x,y,z], viewpoint: {eye, target}, window: {min, max}, near: [elements]} — the 3D camera goes to the spot")
            .noundo(),
    ]
}

fn text(p: &Value, key: &str) -> Option<String> {
    str_param(p, key).map(|t| t.trim().chars().take(MAX_TEXT).collect::<String>())
}

fn kind_type(kind: &str) -> Option<&'static str> {
    Some(match kind.trim().to_ascii_lowercase().as_str() {
        "issue" | "" => "Issue",
        "rfi" | "request" => "Request",
        "punch" | "punchlist" | "punch list" => "Punch",
        _ => return None,
    })
}

fn kind_of(t: &Topic) -> &'static str {
    match t.topic_type.to_ascii_lowercase().as_str() {
        "request" | "rfi" => "rfi",
        "punch" | "punch list" => "punch",
        _ => "issue",
    }
}

/// A photo: PNG or JPEG bytes and their type.
fn photo(cmd: &str, p: &Value) -> Result<Option<(Vec<u8>, &'static str)>> {
    let bytes = match (str_param(p, "photo"), str_param(p, "photo_data")) {
        (Some(path), _) => crate::readfile::read(path.trim()).map_err(|e| bad(cmd, format!("{path}: {e}")))?,
        (None, Some(b)) => crate::cmd::file::base64_decode(b).ok_or_else(|| bad(cmd, "photo_data isn't base64"))?,
        _ => return Ok(None),
    };
    let kind = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else {
        return Err(bad(cmd, "the photo must be a PNG or JPEG"));
    };
    if bytes.len() > MAX_PHOTO {
        return Err(bad(cmd, "the photo must be under 4 MB"));
    }
    Ok(Some((bytes, kind)))
}

/// The elements' centre and level.
fn spot(els: &[El], hs: &[Handle]) -> Option<(Vec3, String)> {
    let picked: Vec<&El> = els.iter().filter(|e| hs.contains(&e.handle)).collect();
    let first = picked.first()?;
    let lo = picked.iter().fold(first.lo, |a, e| Vec3::new(a.x.min(e.lo.x), a.y.min(e.lo.y), a.z.min(e.lo.z)));
    let hi = picked.iter().fold(first.hi, |a, e| Vec3::new(a.x.max(e.hi.x), a.y.max(e.hi.y), a.z.max(e.hi.z)));
    Some(((lo + hi) * 0.5, first.level.clone()))
}

/// A BCF viewpoint looking at `at` (BCF keeps metres).
fn bcf_view(s: &Session, at: Vec3, guids: Vec<String>, handles: Vec<String>, shot: Option<(Vec<u8>, &str)>) -> Result<Viewpoint> {
    let upm = crate::cmd::ifcref::units_per_metre(s.doc()?);
    let upm = if upm.is_finite() && upm > 0.0 { upm } else { 39.37 };
    let vp = viewpoint(at, 20.0 * units_per_foot(s));
    let eye = vec3_of(&vp["eye"]).unwrap_or(at);
    let dir = at - eye;
    let l = dir.len().max(1e-9);
    let d = dir * (1.0 / l);
    let up = Vec3::new(0.0, 0.0, 1.0) - d * d.z;
    let ul = up.len().max(1e-9);
    Ok(Viewpoint {
        guid: bcf::new_guid("viewpoint"),
        eye: [eye.x / upm, eye.y / upm, eye.z / upm],
        dir: [d.x, d.y, d.z],
        up: [up.x / ul, up.y / ul, up.z / ul],
        fov: 60.0,
        ortho: false,
        selection: guids,
        snapshot_type: shot.as_ref().map(|(_, k)| (*k).to_string()).unwrap_or_default(),
        snapshot: shot.map(|(b, _)| crate::cmd::file::base64_encode(&b)),
        handles,
    })
}

fn run_create(s: &mut Session, p: &Value) -> Result<Value> {
    let title = text(p, "title").filter(|t| !t.is_empty()).ok_or_else(|| bad(CMD_CREATE, "`title` is required"))?;
    let kind = str_param(p, "kind").unwrap_or("issue");
    let ty = kind_type(kind).ok_or_else(|| bad(CMD_CREATE, "`kind` is issue, rfi or punch"))?;
    let hs: Vec<Handle> = match p.get("elements") {
        Some(v) => {
            let hs = handles_of(v);
            if hs.len() != v.as_array().map_or(0, Vec::len) {
                return Err(bad(CMD_CREATE, "`elements` are handles (hex strings)"));
            }
            hs
        }
        None => s.selection(),
    };
    let d = s.doc()?;
    if let Some(h) = hs.iter().find(|h| d.entity(**h).is_none()) {
        return Err(bad(CMD_CREATE, format!("no object {}", h.hex())));
    }
    let els = elements(d);
    let at = match p.get("at") {
        Some(v) => Some(vec3_of(v).ok_or_else(|| bad(CMD_CREATE, "`at` is [x, y] or [x, y, z]"))?),
        None => None,
    };
    let (at, level) = match (at, spot(&els, &hs)) {
        (Some(a), sp) => (a, text(p, "level").or(sp.map(|x| x.1)).unwrap_or_default()),
        (None, Some((c, lv))) => (c, text(p, "level").unwrap_or(lv)),
        (None, None) => return Err(bad(CMD_CREATE, "pin the issue: `elements` (or select them) or `at`")),
    };
    let shot = photo(CMD_CREATE, p)?;
    let has_photo = shot.is_some();
    // Elements with a GlobalId go by it, the others by handle.
    let mut guids = Vec::new();
    let mut handles = Vec::new();
    for h in &hs {
        match d.entity(*h).and_then(|e| crate::cmd::ifcref::tag(e)).map(|t| t.guid).filter(|g| !g.is_empty()) {
            Some(g) => guids.push(g),
            None => handles.push(h.hex()),
        }
    }
    let vp = bcf_view(s, at, guids, handles, shot)?;
    let mut topics = bcf::load(s.doc()?);
    let date = bcf::now();
    let author = text(p, "author").filter(|a| !a.is_empty()).unwrap_or_else(|| "CADaddy".into());
    let t = Topic {
        guid: bcf::new_guid(&title),
        title: title.clone(),
        status: text(p, "status").filter(|x| !x.is_empty()).unwrap_or_else(|| "Open".into()),
        topic_type: ty.into(),
        priority: text(p, "priority").unwrap_or_default(),
        assignee: text(p, "assignee").unwrap_or_default(),
        author,
        created: date.clone(),
        modified: date,
        description: text(p, "description").unwrap_or_default(),
        labels: vec![ty.to_string()],
        index: topics.topics.iter().map(|x| x.index).max().unwrap_or(0).saturating_add(1),
        viewpoints: vec![vp],
        due: text(p, "due").unwrap_or_default(),
        at: Some([r3(at.x), r3(at.y), r3(at.z)]),
        level: level.clone(),
        ..Topic::default()
    };
    let out = json!({
        "guid": t.guid, "index": t.index, "kind": kind_of(&t), "title": title, "status": t.status,
        "elements": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(), "at": t.at, "level": level,
        "viewpoint": viewpoint(at, 20.0 * units_per_foot(s)), "photo": has_photo,
    });
    topics.topics.push(t);
    save(s, &topics)?;
    Ok(out)
}

fn save(s: &mut Session, t: &bcf::Topics) -> Result<()> {
    let text = serde_json::to_string(t).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    s.doc_mut()?.bcf = if t.topics.is_empty() { None } else { Some(text) };
    Ok(())
}

/// A topic's elements: its handles, and the drawing's objects with its GlobalIds.
fn topic_handles(s: &mut Session, t: &Topic) -> Result<Vec<Handle>> {
    let Some(v) = t.viewpoints.first() else { return Ok(Vec::new()) };
    let mut out: Vec<Handle> = v.handles.iter().filter_map(|h| Handle::parse_hex(h)).collect();
    if !v.selection.is_empty() {
        let idx = crate::cmd::ifcmodel::cached_index(s)?;
        out.extend(v.selection.iter().filter_map(|g| idx.by_guid(g)).filter_map(|i| idx.els.get(i)).map(|e| e.handle));
    }
    Ok(out)
}

/// Where a topic is: its pin, else its elements' centre (an imported topic), and its level.
fn where_of(s: &mut Session, t: &Topic, els: &[El]) -> Result<(Option<Vec3>, String)> {
    if let Some([x, y, z]) = t.at {
        return Ok((Some(Vec3::new(x, y, z)), t.level.clone()));
    }
    let hs = topic_handles(s, t)?;
    Ok(match spot(els, &hs) {
        Some((c, lv)) => (Some(c), if t.level.is_empty() { lv } else { t.level.clone() }),
        None => (None, t.level.clone()),
    })
}

fn find(cmd: &str, topics: &bcf::Topics, q: Option<&str>) -> Result<usize> {
    let q = q.map(str::trim).filter(|q| !q.is_empty()).ok_or_else(|| bad(cmd, "`issue` is required (its GlobalId or index)"))?;
    topics
        .topics
        .iter()
        .position(|t| t.guid.eq_ignore_ascii_case(q))
        .or_else(|| q.trim_start_matches('#').parse::<u32>().ok().and_then(|n| topics.topics.iter().position(|t| t.index == n)))
        .ok_or_else(|| bad(cmd, format!("no issue {}", q.chars().take(40).collect::<String>())))
}

fn issue_param(p: &Value) -> Option<String> {
    p.get("issue").or_else(|| p.get("topic")).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())))
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let topics = bcf::load(s.doc()?);
    let els = elements(s.doc()?);
    let f = |k: &str| str_param(p, k).map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty());
    let (kind, status, assignee, level, text) = (f("kind"), f("status"), f("assignee"), f("level"), f("text"));
    let element = str_param(p, "element").and_then(Handle::parse_hex);
    let mut out = Vec::new();
    for t in &topics.topics {
        if kind.as_deref().is_some_and(|k| k != kind_of(t))
            || status.as_deref().is_some_and(|x| !t.status.eq_ignore_ascii_case(x))
            || assignee.as_deref().is_some_and(|x| !t.assignee.eq_ignore_ascii_case(x))
            || text.as_deref().is_some_and(|x| !(t.title.to_ascii_lowercase().contains(x) || t.description.to_ascii_lowercase().contains(x)))
        {
            continue;
        }
        let hs = topic_handles(s, t)?;
        if element.is_some_and(|h| !hs.contains(&h)) {
            continue;
        }
        let (at, lv) = where_of(s, t, &els)?;
        if level.as_deref().is_some_and(|x| !lv.eq_ignore_ascii_case(x)) {
            continue;
        }
        let upf = units_per_foot(s);
        out.push(json!({
            "guid": t.guid, "index": t.index, "kind": kind_of(t), "title": t.title, "status": t.status, "assignee": t.assignee,
            "priority": t.priority, "due": t.due, "description": t.description, "author": t.author, "created": t.created, "modified": t.modified,
            "elements": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(),
            "at": at.map(|a| [r3(a.x), r3(a.y), r3(a.z)]), "level": lv,
            "sheet": at.map(|a| json!({"level": lv, "at": [r3(a.x), r3(a.y)]})),
            "viewpoint": at.map(|a| viewpoint(a, 20.0 * upf)),
            "photo": t.viewpoints.iter().any(|v| v.snapshot.is_some()), "comments": t.comments.len(),
        }));
    }
    Ok(json!({"issues": out, "total": out.len()}))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let mut topics = bcf::load(s.doc()?);
    let i = find(CMD_SET, &topics, issue_param(p).as_deref())?;
    let shot = photo(CMD_SET, p)?;
    let new_handles = p.get("elements").map(handles_of);
    let new_at = match p.get("at") {
        Some(v) => Some(vec3_of(v).ok_or_else(|| bad(CMD_SET, "`at` is [x, y] or [x, y, z]"))?),
        None => None,
    };
    let els = if new_handles.is_some() { elements(s.doc()?) } else { Vec::new() };
    let Some(t) = topics.topics.get_mut(i) else { return Err(bad(CMD_SET, "no such issue")) };
    let mut changed = false;
    for (key, field) in [
        ("status", &mut t.status),
        ("assignee", &mut t.assignee),
        ("priority", &mut t.priority),
        ("due", &mut t.due),
        ("title", &mut t.title),
        ("description", &mut t.description),
    ] {
        if let Some(v) = text(p, key)
            && *field != v
        {
            if key == "title" && v.is_empty() {
                return Err(bad(CMD_SET, "an issue's title can't be empty"));
            }
            *field = v;
            changed = true;
        }
    }
    if let Some(hs) = new_handles {
        if let Some(v) = t.viewpoints.first_mut() {
            v.handles = hs.iter().map(|h| h.hex()).collect();
        }
        if new_at.is_none()
            && let Some((c, lv)) = spot(&els, &hs)
        {
            t.at = Some([r3(c.x), r3(c.y), r3(c.z)]);
            t.level = lv;
        }
        changed = true;
    }
    if let Some(a) = new_at {
        t.at = Some([r3(a.x), r3(a.y), r3(a.z)]);
        changed = true;
    }
    if let Some((b, k)) = shot
        && let Some(v) = t.viewpoints.first_mut()
    {
        v.snapshot = Some(crate::cmd::file::base64_encode(&b));
        v.snapshot_type = k.into();
        changed = true;
    }
    if let Some(c) = text(p, "comment").filter(|c| !c.is_empty()) {
        let author = text(p, "author").filter(|a| !a.is_empty()).unwrap_or_else(|| "CADaddy".into());
        if t.comments.len() >= 500 {
            return Err(bad(CMD_SET, "an issue keeps at most 500 comments"));
        }
        t.comments.push(bcf::Comment { guid: bcf::new_guid(&c), date: bcf::now(), author, text: c, viewpoint: None });
        changed = true;
    }
    if changed {
        t.modified = bcf::now();
    }
    let out = json!({"guid": t.guid, "status": t.status, "assignee": t.assignee, "changed": changed});
    if changed {
        save(s, &topics)?;
    }
    Ok(out)
}

fn run_show(s: &mut Session, p: &Value) -> Result<Value> {
    let topics = bcf::load(s.doc()?);
    let i = find(CMD_SHOW, &topics, issue_param(p).as_deref())?;
    let t = topics.topics.get(i).cloned().ok_or_else(|| bad(CMD_SHOW, "no such issue"))?;
    let hs = topic_handles(s, &t)?;
    let mut out = frame(s, &hs)?;
    let els = elements(s.doc()?);
    if let (Some(a), _) = where_of(s, &t, &els)? {
        let vp = viewpoint(a, 20.0 * units_per_foot(s));
        if let (Some(e), Some(tg)) = (vec3_of(&vp["eye"]), vec3_of(&vp["target"])) {
            s.state_mut()?.ifc_view.camera = Some((e, tg));
        }
        out["viewpoint"] = vp;
    }
    out["guid"] = json!(t.guid);
    Ok(out)
}

fn is_mark(e: &cadaddy_doc::Entity) -> bool {
    e.common.xdata.iter().any(|t| t.code == 1001 && t.value == MARK_APP)
}

fn run_marks(s: &mut Session, p: &Value) -> Result<Value> {
    let old: Vec<Handle> = s.doc()?.model.iter().filter(|e| is_mark(e)).map(|e| e.handle).collect();
    let removed = old.len();
    {
        let d = s.doc_mut()?;
        for h in &old {
            d.remove_entity(*h);
        }
    }
    if bool_or(p, "clear", false) {
        return Ok(json!({"marks": 0, "removed": removed, "layer": MARK_LAYER}));
    }
    let level = str_param(p, "level").map(str::trim).filter(|x| !x.is_empty());
    let status = str_param(p, "status").map(str::trim).filter(|x| !x.is_empty());
    let topics = bcf::load(s.doc()?);
    let els = elements(s.doc()?);
    let upf = units_per_foot(s);
    let mut marks = Vec::new();
    for t in &topics.topics {
        if status.map_or(t.status.eq_ignore_ascii_case("closed"), |x| !t.status.eq_ignore_ascii_case(x)) {
            continue;
        }
        let (Some(at), lv) = where_of(s, t, &els)? else { continue };
        if level.is_some_and(|x| !lv.eq_ignore_ascii_case(x)) {
            continue;
        }
        marks.push((t.guid.clone(), t.index, kind_of(t), at));
    }
    let d = s.doc_mut()?;
    d.ensure_layer(MARK_LAYER);
    let mut out = Vec::new();
    for (guid, index, kind, at) in &marks {
        let x = vec![RawTag { code: 1001, value: MARK_APP.into() }, RawTag { code: 1000, value: guid.clone() }];
        let common = Common { layer: MARK_LAYER.into(), xdata: x.into(), ..Common::default() };
        let c = Vec3::new(at.x, at.y, 0.0);
        let r = 1.0 * upf;
        let prefix = match *kind {
            "rfi" => "R",
            "punch" => "P",
            _ => "I",
        };
        let circle = d.add(&Space::Model, common.clone(), EntityKind::Circle(Circle { center: c, radius: r }))?;
        let label = Text {
            insert: c,
            align_pt: Some(c),
            height: 0.8 * upf,
            value: format!("{prefix}{index}"),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Center,
            valign: cadaddy_doc::VAlign::Middle,
        };
        let text = d.add(&Space::Model, common, EntityKind::Text(label))?;
        out.push(json!({"guid": guid, "label": format!("{prefix}{index}"), "at": [r3(at.x), r3(at.y)], "handles": [circle.hex(), text.hex()]}));
    }
    Ok(json!({"marks": out.len(), "issues": out, "removed": removed, "layer": MARK_LAYER}))
}

fn run_link(s: &mut Session, p: &Value) -> Result<Value> {
    let a = p.get("at").and_then(Value::as_array).ok_or_else(|| bad(CMD_LINK, "`at` is required: [x, y] on a sheet or [x, y, z] in the model"))?;
    let n: Vec<f64> = a.iter().take(3).filter_map(Value::as_f64).filter(|v| v.is_finite() && v.abs() < 1e12).collect();
    if n.len() < 2 || n.len() != a.len().min(3) {
        return Err(bad(CMD_LINK, "`at` is [x, y] or [x, y, z] (numbers)"));
    }
    let from = str_param(p, "from").map(str::to_ascii_lowercase).unwrap_or_else(|| if n.len() == 3 { "model".into() } else { "sheet".into() });
    if !matches!(from.as_str(), "sheet" | "model") {
        return Err(bad(CMD_LINK, "`from` is sheet or model"));
    }
    let (x, y) = (n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0));
    let (sp, _) = crate::cmd::bspec::current(s, CMD_LINK)?;
    let upf = units_per_foot(s);
    // The level: named, else the sheet region holding the point (a sheet point) or the storey
    // the height is in (a 3D point), else the first.
    let named = str_param(p, "level").and_then(|l| sp.levels.iter().find(|x| x.name.eq_ignore_ascii_case(l.trim())));
    let by_region = || cadaddy_walk3d::level_at(&sp.levels, x, y);
    let by_height =
        |z: f64| sp.levels.iter().filter(|l| l.elevation_ft * upf <= z + 0.05 * upf).max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    let level = match (&named, from.as_str(), n.get(2)) {
        (Some(l), _, _) => Some(*l),
        (None, "model", Some(z)) => {
            // Side-by-side sheets: the region decides; stacked: the height.
            let r = by_region().filter(|l| l.region.is_some());
            r.or_else(|| by_height(*z))
        }
        _ => by_region(),
    }
    .or_else(|| sp.levels.first());
    let (lname, elev) = level.map_or((String::new(), 0.0), |l| (l.name.clone(), l.elevation_ft));
    let z = match (from.as_str(), n.get(2)) {
        ("model", Some(z)) => *z,
        _ => (elev + EYE_FT) * upf,
    };
    let target = Vec3::new(x, y, z);
    let vp = viewpoint(target, 25.0 * upf);
    if let (Some(e), Some(t)) = (vec3_of(&vp["eye"]), vec3_of(&vp["target"])) {
        s.state_mut()?.ifc_view.camera = Some((e, t));
    }
    let base = level.and_then(|l| l.base).unwrap_or([0.0, 0.0]);
    let half = 15.0 * upf;
    // The elements near the spot.
    let d = s.doc()?;
    let mut near: Vec<(f64, Value)> = elements(d)
        .into_iter()
        .filter(|e| lname.is_empty() || e.level.is_empty() || e.level.eq_ignore_ascii_case(&lname))
        .filter_map(|e| {
            let dx = (e.lo.x - x).max(x - e.hi.x).max(0.0);
            let dy = (e.lo.y - y).max(y - e.hi.y).max(0.0);
            let dist = dx.hypot(dy);
            (dist <= half).then(|| (dist, e.json()))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(json!({
        "from": from, "level": lname, "sheet": [r3(x), r3(y)], "model": [r3(x), r3(y), r3(z)],
        "building_ft": [r3((x - base[0]) / upf), r3((y - base[1]) / upf), r3(z / upf)],
        "viewpoint": vp, "window": {"min": [r3(x - half), r3(y - half)], "max": [r3(x + half), r3(y + half)]},
        "near": near.into_iter().take(10).map(|(_, v)| v).collect::<Vec<_>>(),
    }))
}
