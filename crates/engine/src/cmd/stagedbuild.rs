//! The staged PDF → BIM build (Q5258 stagedbuild): `bim.from_pdf` builds the building the way it
//! is built, stage by stage, and reports and keeps each stage.
//!
//! 1. **shell**: levels, grids, slabs, exterior walls, roof, columns, stairs, from the
//!    architectural plans only.
//! 2. **walls**: the interior partitions.
//! 3. **openings**: doors and windows in their host walls (cut from each opening's size).
//! 4. **framing**: every wall framed by [`cadaddy_mepfix::framing`]: end, corner and intersection
//!    studs, king and jack studs and a header at each opening, field studs at the code's spacing
//!    ([`cadaddy_walk3d::codebook`]), plates; one `framing_member` element per wall with its counts.
//! 5. **rooms**: from the walls with the doors closing their openings; each door between its rooms.
//! 6. **mep**: each electrical, mechanical, plumbing and fire protection plan lined up on its
//!    level's architectural plan ([`super::register`]) and its background removed before the
//!    trade readers run; then devices, ducts, pipes and conduit, placed on their levels.
//! 7. **drywall**: board, framing and ceilings by wall type and room.
//! 8. **finishes**: casework and floor, wall and ceiling finishes per room.
//!
//! The readings run first (they read the sheets, not the model), then the model is built once
//! and every element it holds belongs to exactly one stage: each stage's **checkpoint** is the
//! model with that stage's elements and the earlier stages', kept in the drawing (agent JSON,
//! key `staged_build`) so `job.stage_view` shows the model as it stood after a stage and
//! `job.rollback` takes the later stages out.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::Vec3;
use cadaddy_mepfix::framing::{self as fr, End, Kind};
use serde_json::{Value, json};

use super::{CommandSpec, bad, str_param};
use crate::{Result, Session};

/// The stages, in order.
pub const STAGES: [&str; 8] = ["shell", "walls", "openings", "framing", "rooms", "mep", "drywall", "finishes"];
/// The record's key in the drawing's agent JSON.
const KEY: &str = "staged_build";
/// The layer of the framing elements.
const FRAMING_LAYER: &str = "3D-Framing";
/// The framing element's part.
const FRAMING_PART: &str = "framing_member";
/// Walls framed at most (hostile models).
const MAX_FRAMED: usize = 20_000;
/// Elements one stage keeps in its checkpoint, at most.
const MAX_STAGE_HANDLES: usize = 400_000;
/// Questions asked per stage, at most.
const MAX_QUESTIONS: usize = 40;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("job.stage_view", "Show Build Stage", run_view).params(
            "{stage?: shell|walls|openings|framing|rooms|mep|drywall|finishes|all} → {stages: [{name, status, counts, elements}], stage?, shown, hidden}: \
             the model as it stood after that stage (later stages' elements hidden; all shows everything); no stage lists the stages bim.from_pdf built",
        ),
        CommandSpec::new("job.rollback", "Roll Back Build Stage", run_rollback).params(
            "{stage: shell|walls|openings|framing|rooms|mep|drywall|finishes} → {stage, removed, stages}: the model back to that stage's checkpoint: \
             every later stage's elements are erased and those stages marked rolled_back (undo brings them back)",
        ),
    ]
}

// ---- Which stage an element is ---------------------------------------------------------------

/// The stage index of a built part (`exterior`: a wall on the building's outside).
fn stage_of_part(part: &str, exterior: bool) -> usize {
    let p = part.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| p.contains(x));
    if p == "wall" {
        return usize::from(!exterior);
    }
    if p == FRAMING_PART || has(&["framing", "stud"]) {
        return 3;
    }
    if has(&["door", "window", "opening"]) {
        return 2;
    }
    if p == "room" || has(&["space"]) {
        return 4;
    }
    if has(&["ceiling", "soffit", "bulkhead", "drywall", "board"]) {
        return 6;
    }
    if has(&["casework", "cabinet", "counter", "vanity", "millwork", "furnish", "furniture", "finish"]) {
        return 7;
    }
    if has(&[
        "mep",
        "device",
        "equipment",
        "light",
        "diffuser",
        "terminal",
        "duct",
        "pipe",
        "conduit",
        "tray",
        "sprinkler",
        "fixture",
        "ground_rod",
        "raceway",
        "panel",
        "grounding",
        "drain",
    ]) {
        return 5;
    }
    // Slabs, roofs, parapets, columns, curtain walls, stairs, the ground, concrete: the shell.
    0
}

/// What the build made an element as, its level and the plan object it came from.
fn made(e: &Entity) -> Option<(String, String, Option<Handle>)> {
    let part = super::wallgen::made_as(e)?;
    let level = super::bspec::made_level(e).unwrap_or_default();
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    let source = x.get(at + 3).filter(|t| t.code == 1000).and_then(|t| Handle::parse_hex(&t.value)).filter(|h| h.0 != 0);
    Some((part, level, source))
}

/// A room volume (`rooms.build`).
fn is_room_volume(e: &Entity) -> bool {
    e.common.xdata.iter().any(|t| t.code == 1001 && t.value == super::rooms3d::ROOM_APP)
}

/// A wall's or opening's plan footprint: its centreline `a`..`b`, thickness, and heights.
#[derive(Clone, Copy, Debug)]
struct Foot {
    a: [f64; 2],
    b: [f64; 2],
    thick: f64,
    z: [f64; 2],
}

impl Foot {
    fn len(&self) -> f64 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }
    fn dir(&self) -> [f64; 2] {
        let l = self.len().max(1e-12);
        [(self.b[0] - self.a[0]) / l, (self.b[1] - self.a[1]) / l]
    }
    fn centre(&self) -> [f64; 2] {
        [(self.a[0] + self.b[0]) / 2.0, (self.a[1] + self.b[1]) / 2.0]
    }
    /// `p` as (along from `a`, off the centreline).
    fn local(&self, p: [f64; 2]) -> (f64, f64) {
        let u = self.dir();
        let d = [p[0] - self.a[0], p[1] - self.a[1]];
        (d[0] * u[0] + d[1] * u[1], d[0] * u[1] - d[1] * u[0])
    }
}

/// The footprint of a mesh: its main axis (the vertices' principal direction) and their spread
/// along and across it.
fn footprint(e: &Entity) -> Option<Foot> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let pts: Vec<&Vec3> = m.vertices.iter().filter(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite()).take(4_096).collect();
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.x).sum::<f64>() / n, pts.iter().map(|p| p.y).sum::<f64>() / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = [th.cos(), th.sin()];
    let (mut s0, mut s1, mut t0, mut t1, mut z0, mut z1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in &pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        let (s, t) = (dx * u[0] + dy * u[1], dx * u[1] - dy * u[0]);
        s0 = s0.min(s);
        s1 = s1.max(s);
        t0 = t0.min(t);
        t1 = t1.max(t);
        z0 = z0.min(p.z);
        z1 = z1.max(p.z);
    }
    let tm = (t0 + t1) / 2.0;
    // Off the centreline by `tm` along the normal (u.y, -u.x).
    let c = [mx + u[1] * tm, my - u[0] * tm];
    let f = Foot { a: [c[0] + u[0] * s0, c[1] + u[1] * s0], b: [c[0] + u[0] * s1, c[1] + u[1] * s1], thick: (t1 - t0).max(0.0), z: [z0, z1] };
    (f.len() > 1e-9 && f.len().is_finite()).then_some(f)
}

/// Whether the ray from `o` along `d` crosses the segment `a`..`b` within `max`.
fn ray_hits(o: [f64; 2], d: [f64; 2], a: [f64; 2], b: [f64; 2], max: f64) -> bool {
    let e = [b[0] - a[0], b[1] - a[1]];
    let den = d[0] * e[1] - d[1] * e[0];
    if den.abs() < 1e-12 {
        return false;
    }
    let w = [a[0] - o[0], a[1] - o[1]];
    let t = (w[0] * e[1] - w[1] * e[0]) / den;
    let s = (w[0] * d[1] - w[1] * d[0]) / den;
    t > 0.0 && t <= max && (-1e-9..=1.0 + 1e-9).contains(&s)
}

/// The walls on the building's outside: from points along each wall, one side sees no other
/// wall of its level (two of three points agree).
fn exterior_walls(walls: &[(Handle, String, Foot)]) -> BTreeSet<Handle> {
    let mut by_level: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, (_, level, _)) in walls.iter().enumerate() {
        by_level.entry(level.as_str()).or_default().push(i);
    }
    let mut out = BTreeSet::new();
    for idx in by_level.values() {
        let max = f64::INFINITY;
        for &i in idx {
            let Some((h, _, f)) = walls.get(i) else { continue };
            let u = f.dir();
            let nrm = [u[1], -u[0]];
            let mut open = [0usize; 2];
            for k in [0.25, 0.5, 0.75] {
                let p = [f.a[0] + (f.b[0] - f.a[0]) * k, f.a[1] + (f.b[1] - f.a[1]) * k];
                for (side, sgn) in [(0usize, 1.0), (1, -1.0)] {
                    let d = [nrm[0] * sgn, nrm[1] * sgn];
                    let o = [p[0] + d[0] * (f.thick / 2.0 + 1e-6), p[1] + d[1] * (f.thick / 2.0 + 1e-6)];
                    let blocked = idx.iter().any(|&j| j != i && walls.get(j).is_some_and(|(_, _, g)| ray_hits(o, d, g.a, g.b, max)));
                    if !blocked && let Some(x) = open.get_mut(side) {
                        *x += 1;
                    }
                }
            }
            if open.iter().any(|n| *n >= 2) {
                out.insert(*h);
            }
        }
    }
    out
}

// ---- Framing ---------------------------------------------------------------------------------

/// A wall's framing rules from the code book: (wall kind, stud, spacing in, rule id, header rows
/// or none when the code gives no header table for it).
fn framing_rules(
    book: &cadaddy_walk3d::codebook::CodeBook,
    exterior: bool,
    thick_in: f64,
    height_ft: f64,
) -> (String, String, f64, String, Option<Vec<fr::HeaderRow>>) {
    use cadaddy_walk3d::codebook::{RuleContext, Topic};
    let wall = if exterior { "bearing" } else { "non-bearing" };
    let stud = if thick_in >= 6.5 { "2x6" } else { "2x4" };
    let ctx = RuleContext { wall: Some(wall.into()), stud: Some(stud.into()), height_ft: Some(height_ft.min(10.0)), ..RuleContext::default() };
    let mut spacing: Option<(f64, String)> = None;
    let mut headers: Option<Vec<fr::HeaderRow>> = None;
    for rule in book.query(Some(Topic::Framing), &ctx) {
        let v = book.resolve(rule);
        if let Some(m) = v.max_spacing_in.filter(|m| m.is_finite() && *m >= 8.0 && *m <= 48.0)
            && spacing.as_ref().is_none_or(|(s, _)| m < *s)
        {
            spacing = Some((m, rule.id.clone()));
        }
        if headers.is_none() && !v.table.is_empty() {
            let mut rows: Vec<fr::HeaderRow> = v
                .table
                .iter()
                .filter_map(|(k, size)| {
                    let ft: f64 = k.strip_prefix("<=")?.trim().parse().ok()?;
                    let nominal: f64 = size.rsplit(['x', 'X']).next()?.trim().parse().ok()?;
                    (ft.is_finite() && ft > 0.0).then(|| fr::HeaderRow {
                        max_span_in: ft * 12.0,
                        size: format!("2-{size}"),
                        depth_in: (nominal - 0.75).max(1.0),
                    })
                })
                .collect();
            rows.sort_by(|a, b| a.max_span_in.total_cmp(&b.max_span_in));
            if let Some(last) = rows.last().cloned() {
                rows.push(fr::HeaderRow { max_span_in: f64::MAX, ..last });
            }
            headers = (!rows.is_empty()).then_some(rows);
        }
    }
    let (sp, id) = spacing.unwrap_or((16.0, "default 16\" o.c.".into()));
    (wall.into(), stud.into(), sp, id, headers)
}

/// What framing every wall gave.
#[derive(Default)]
struct Framed {
    walls: usize,
    studs: usize,
    headers: usize,
    placeholder_headers: usize,
    members_lf: f64,
    plates_lf: f64,
    by_level: BTreeMap<String, (usize, usize)>,
    elements: Vec<Handle>,
}

/// Frame every wall of the model: its openings are the doors and windows hosted in it.
fn frame_walls(
    d: &mut Drawing,
    walls: &[(Handle, String, Foot)],
    openings: &[(Handle, String, Foot)],
    exterior: &BTreeSet<Handle>,
    upf: f64,
) -> Framed {
    let book = d
        .building_spec
        .as_deref()
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| v.get("codebook").cloned())
        .and_then(|v| serde_json::from_value::<cadaddy_walk3d::codebook::CodeBook>(v).ok())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(cadaddy_walk3d::codebook::starter_book);
    let ft = |v: f64| v / upf;
    let mut out = Framed::default();
    let mut groups: BTreeMap<(String, String, bool), (Vec<fr::FrameWall>, Option<Vec<fr::HeaderRow>>, String, String)> = BTreeMap::new();
    for (h, level, f) in walls.iter().take(MAX_FRAMED) {
        let (thick_in, height_ft) = (ft(f.thick) * 12.0, ft(f.z[1] - f.z[0]));
        let ext = exterior.contains(h);
        let (kind, stud, spacing, rule, headers) = framing_rules(&book, ext, thick_in, height_ft);
        // The openings in this wall: along it, within its thickness and a little.
        let near = f.thick / 2.0 + upf * 0.5;
        let mut ops: Vec<[f64; 2]> = Vec::new();
        for (_, ol, of) in openings.iter().filter(|(_, ol, _)| ol == level) {
            let (s, off) = f.local(of.centre());
            if off.abs() > near || s < -upf || s > f.len() + upf {
                continue;
            }
            let (sa, _) = f.local(of.a);
            let (sb, _) = f.local(of.b);
            let (lo, hi) = (sa.min(sb).max(0.0), sa.max(sb).min(f.len()));
            if hi - lo > upf * 0.5 {
                ops.push([ft(lo), ft(hi)]);
            }
            let _ = ol;
        }
        ops.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let wall = fr::FrameWall {
            id: h.hex(),
            level: level.clone(),
            a_ft: [ft(f.a[0]), ft(f.a[1])],
            b_ft: [ft(f.b[0]), ft(f.b[1])],
            thickness_in: thick_in.clamp(0.5, 48.0),
            wall_type: kind.clone(),
            stud_in: (thick_in <= 12.0).then_some(if stud == "2x6" { 5.5 } else { 3.5 }),
            flange_in: 1.5,
            spacing_in: spacing,
            height_ft: height_ft.clamp(1.0, 100.0),
            openings: ops,
            plumbing: false,
            rooms: Vec::new(),
            type_guess: Some(format!("{kind} {stud} wall (no wall type read): studs at {spacing}\" o.c. by {rule}")),
        };
        let key = (kind, stud, headers.is_some());
        let g = groups.entry(key).or_insert_with(|| (Vec::new(), headers.clone(), rule.clone(), String::new()));
        g.0.push(wall);
    }
    if d.layer(FRAMING_LAYER).is_none() {
        d.layers.push(cadaddy_doc::Layer {
            name: FRAMING_LAYER.into(),
            color: cadaddy_doc::color::Color::Index(42),
            ..cadaddy_doc::Layer::default()
        });
    }
    for ((kind, stud, has_headers), (fws, headers, rule, _)) in groups {
        let framing = fr::frame(&fws, &[], &[], headers.as_deref().unwrap_or(&[]));
        for wf in &framing.walls {
            let mine: Vec<&fr::Member> = wf.resolved(End::A).collect();
            let studs = mine
                .iter()
                .filter(|m| {
                    matches!(
                        m.kind,
                        Kind::EndStud
                            | Kind::CornerStud
                            | Kind::IntersectionStud
                            | Kind::PostStud
                            | Kind::KingStud
                            | Kind::JackStud
                            | Kind::FieldStud
                            | Kind::Cripple
                    )
                })
                .count();
            let heads = mine.iter().filter(|m| m.kind == Kind::Header).count();
            let vertical_lf: f64 = mine.iter().filter(|m| m.kind != Kind::Header).map(|m| (m.z_in[1] - m.z_in[0]).max(0.0) / 12.0).sum();
            let header_lf: f64 = mine.iter().filter(|m| m.kind == Kind::Header).map(|m| m.width_in.max(0.0) / 12.0).sum();
            let len_ft = wf.wall.len_in() / 12.0;
            let plates = 2.0 * len_ft;
            out.walls += 1;
            out.studs += studs;
            out.headers += heads;
            if !has_headers {
                out.placeholder_headers += heads;
            }
            out.plates_lf += plates;
            out.members_lf += vertical_lf + header_lf + plates;
            let e = out.by_level.entry(wf.wall.level.clone()).or_default();
            e.0 += 1;
            e.1 += studs;
            // The wall's framing, one element: its counts as properties.
            let Some(src) = Handle::parse_hex(&wf.wall.id) else { continue };
            let Some(f) = walls.iter().find(|(h, _, _)| *h == src).map(|(_, _, f)| *f) else { continue };
            let z = f.z[0];
            let t = |code: i32, v: String| RawTag { code, value: v };
            let mut xdata = vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, FRAMING_PART.into()),
                t(1000, wf.wall.level.chars().take(255).collect()),
                t(1000, src.hex()),
            ];
            let mut props = cadaddy_doc::props::Props::default();
            let rule_src = || cadaddy_doc::props::Source::from_doc(cadaddy_doc::props::SourceKind::Rule, &rule);
            let num = |v: f64| (v * 10.0).round() / 10.0;
            for (k, v) in [
                ("member_kind", json!("wall framing")),
                ("material", json!("wood")),
                ("size", json!(stud)),
                ("wall_kind", json!(kind)),
                ("spacing", json!(wf.wall.spacing_in)),
                ("studs", json!(studs)),
                ("headers", json!(heads)),
                ("length", json!(num(len_ft))),
                ("members_lf", json!(num(vertical_lf + header_lf + plates))),
                ("plates_lf", json!(num(plates))),
                ("wall_host", json!(src.hex())),
                ("source", json!(if has_headers { "rule" } else { "rule; placeholder headers (no header rule for this wall)" })),
            ] {
                let mut p = cadaddy_doc::props::Prop::new(v, rule_src());
                if matches!(k, "spacing") {
                    p = p.with_unit("in");
                } else if matches!(k, "length" | "members_lf" | "plates_lf") {
                    p = p.with_unit("ft");
                }
                props.set(k, p);
            }
            xdata = cadaddy_doc::props::write(&xdata, &props);
            let line = cadaddy_doc::Line { a: Vec3::new(f.a[0], f.a[1], z), b: Vec3::new(f.b[0], f.b[1], z) };
            let common = Common { layer: FRAMING_LAYER.into(), visible: false, xdata: xdata.into(), ..Common::default() };
            if let Ok(h) = d.add(&Space::Model, common, EntityKind::Line(line)) {
                out.elements.push(h);
            }
        }
    }
    out
}

// ---- The staged record -----------------------------------------------------------------------

/// One stage as reported and kept.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stage {
    pub name: &'static str,
    pub status: &'static str,
    pub counts: serde_json::Map<String, Value>,
    pub by_level: BTreeMap<String, serde_json::Map<String, Value>>,
    pub notes: Vec<String>,
    pub elements: Vec<Handle>,
}

impl Stage {
    fn new(name: &'static str) -> Stage {
        Stage { name, status: "done", ..Stage::default() }
    }
    fn count(&mut self, k: &str, v: impl Into<Value>) {
        self.counts.insert(k.into(), v.into());
    }
    fn level(&mut self, level: &str, k: &str, add: f64) {
        let e = self.by_level.entry(level.to_string()).or_default();
        let now = e.get(k).and_then(Value::as_f64).unwrap_or(0.0) + add;
        e.insert(k.into(), json!(if now.fract() == 0.0 && now.abs() < 1e15 { json!(now as i64) } else { json!((now * 10.0).round() / 10.0) }));
    }
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "status": self.status,
            "counts": self.counts,
            "by_level": self.by_level,
            "elements": self.elements.len(),
            "notes": self.notes,
        })
    }
}

/// The record kept in the drawing: each stage with its elements' handles.
fn store(d: &mut Drawing, stages: &[Stage]) {
    let mut root: Value = d.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let rec: Vec<Value> = stages
        .iter()
        .map(|st| {
            let mut v = st.to_json();
            v["handles"] = json!(st.elements.iter().take(MAX_STAGE_HANDLES).map(|h| h.hex()).collect::<Vec<_>>());
            v
        })
        .collect();
    root[KEY] = json!({"stages": rec});
    d.agent = serde_json::to_string(&root).ok();
}

fn load(d: &Drawing) -> Option<Vec<Value>> {
    let root: Value = serde_json::from_str(d.agent.as_deref()?).ok()?;
    root.get(KEY)?.get("stages")?.as_array().cloned()
}

fn stage_handles(st: &Value) -> Vec<Handle> {
    st.get("handles")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter_map(Handle::parse_hex).collect())
        .unwrap_or_default()
}

fn stage_index(cmd: &str, name: &str) -> Result<usize> {
    STAGES.iter().position(|s| s.eq_ignore_ascii_case(name.trim())).ok_or_else(|| bad(cmd, format!("`stage` must be one of {}", STAGES.join(", "))))
}

fn summary(stages: &[Value]) -> Vec<Value> {
    stages
        .iter()
        .map(|st| {
            let mut v = st.clone();
            if let Some(o) = v.as_object_mut() {
                o.remove("handles");
            }
            v
        })
        .collect()
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "job.stage_view";
    let Some(stages) = load(s.doc()?) else {
        return Err(bad(CMD, "no staged build in this drawing: bim.from_pdf {path} builds one"));
    };
    let Some(name) = str_param(p, "stage").filter(|n| !n.trim().is_empty()) else {
        return Ok(json!({"stages": summary(&stages)}));
    };
    let upto = if name.trim().eq_ignore_ascii_case("all") { STAGES.len() - 1 } else { stage_index(CMD, name)? };
    let d = s.doc_mut()?;
    let (mut shown, mut hidden) = (0usize, 0usize);
    for (i, st) in stages.iter().enumerate() {
        for h in stage_handles(st) {
            if d.entity(h).is_none() {
                continue;
            }
            if i <= upto {
                d.hidden_objects.remove(&h);
                shown += 1;
            } else {
                d.hidden_objects.insert(h);
                hidden += 1;
            }
        }
    }
    Ok(json!({"stage": STAGES.get(upto), "shown": shown, "hidden": hidden, "stages": summary(&stages)}))
}

fn run_rollback(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "job.rollback";
    let name = str_param(p, "stage").ok_or_else(|| bad(CMD, format!("`stage` is required: one of {}", STAGES.join(", "))))?;
    let upto = stage_index(CMD, name)?;
    let Some(mut stages) = load(s.doc()?) else {
        return Err(bad(CMD, "no staged build in this drawing: bim.from_pdf {path} builds one"));
    };
    let d = s.doc_mut()?;
    let mut removed = 0usize;
    for (i, st) in stages.iter_mut().enumerate().skip(upto + 1) {
        for h in stage_handles(st) {
            if d.entity(h).is_some() {
                d.model.remove(h);
                d.hidden_objects.remove(&h);
                removed += 1;
            }
        }
        st["status"] = json!("rolled_back");
        st["handles"] = json!([]);
        st["elements"] = json!(0);
        let _ = i;
    }
    // What is left shows.
    for st in stages.iter().take(upto + 1) {
        for h in stage_handles(st) {
            d.hidden_objects.remove(&h);
        }
    }
    let mut root: Value = d.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    root[KEY] = json!({"stages": stages});
    d.agent = serde_json::to_string(&root).ok();
    Ok(json!({"stage": STAGES.get(upto), "removed": removed, "stages": summary(&stages)}))
}

// ---- bim.from_pdf, staged ----------------------------------------------------------------------

/// What the stages need from the readings made before the build.
#[derive(Default)]
pub(crate) struct Readings {
    /// The MEP plans read and lined up ([`super::printmep::SheetRead::to_json`]).
    pub mep: Option<Value>,
    /// Every trade plan's registration ([`super::register::Registration::to_json`]).
    pub sheets: Vec<Value>,
    pub devices: usize,
    pub warnings: Vec<String>,
}

/// The sheets of a job as the trade readers see them, the architectural ones' discipline first.
fn arch_spans(refs: &[super::printmep::SheetRef]) -> Vec<(u64, u64, [f64; 4], bool)> {
    refs.iter()
        .map(|r| {
            let (lo, hi) = r.span.unwrap_or((0, 0));
            (lo, hi, r.rect, r.discipline.eq_ignore_ascii_case("A"))
        })
        .collect()
}

/// Whether a plan object (by handle and where it is) lies on an architectural sheet.
fn on_arch(d: &Drawing, h: Handle, spans: &[(u64, u64, [f64; 4], bool)]) -> Option<bool> {
    if let Some((_, _, _, a)) = spans.iter().find(|(lo, hi, _, _)| hi > lo && h.0 >= *lo && h.0 < *hi) {
        return Some(*a);
    }
    let e = d.entity(h)?;
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    let (cx, cy) = ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
    spans.iter().find(|(_, _, r, _)| cx >= r[0] && cx <= r[2] && cy >= r[1] && cy <= r[3]).map(|(_, _, _, a)| *a)
}

/// Sort the built model into the eight stages, frame its walls, take off its board and
/// finishes, and keep each stage's checkpoint in the drawing. `built`: model.build's answer.
pub(crate) fn stage_model(s: &mut Session, built: &Value, readings: &Readings, refs: &[super::printmep::SheetRef]) -> Result<(Vec<Stage>, Value)> {
    let upf = super::bimwork::units_per_foot(s);
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut st: Vec<Stage> = STAGES.iter().map(|n| Stage::new(n)).collect();
    // The elements by part.
    let (mut walls, mut openings): (Vec<(Handle, String, Foot)>, Vec<(Handle, String, Foot)>) = (Vec::new(), Vec::new());
    let mut parts: Vec<(Handle, String, String)> = Vec::new();
    let mut volumes: Vec<Handle> = Vec::new();
    let spans = arch_spans(refs);
    let (mut arch_walls, mut other_walls) = (0usize, 0usize);
    {
        let d = s.doc()?;
        for e in d.model.iter() {
            if let Some((part, level, source)) = made(e) {
                if part == FRAMING_PART {
                    continue;
                }
                if part == "wall" {
                    if let Some(f) = footprint(e) {
                        walls.push((e.handle, level.clone(), f));
                    }
                    match source.and_then(|h| on_arch(d, h, &spans)) {
                        Some(false) => other_walls += 1,
                        _ => arch_walls += 1,
                    }
                } else if (part == "door" || part == "window")
                    && let Some(f) = footprint(e)
                {
                    openings.push((e.handle, level.clone(), f));
                }
                parts.push((e.handle, part, level));
            } else if is_room_volume(e) {
                volumes.push(e.handle);
            }
        }
    }
    let exterior = exterior_walls(&walls);
    // Earlier framing elements go (a rebuild frames again).
    {
        let d = s.doc_mut()?;
        let old: Vec<Handle> = d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some(FRAMING_PART)).map(|e| e.handle).collect();
        for h in old {
            d.model.remove(h);
        }
    }
    let mut unsorted: BTreeMap<String, usize> = BTreeMap::new();
    for (h, part, level) in &parts {
        let i = stage_of_part(part, exterior.contains(h));
        if let Some(stage) = st.get_mut(i) {
            stage.elements.push(*h);
            stage.level(if level.is_empty() { "(no level)" } else { level }, part, 1.0);
        }
        if i == 0
            && !matches!(
                part.as_str(),
                "wall" | "floor" | "roof" | "parapet" | "column" | "curtainwall" | "ground" | "stair" | "ramp" | "railing" | "guard" | "concrete"
            )
        {
            *unsorted.entry(part.clone()).or_default() += 1;
        }
    }
    if let Some(rooms) = st.get_mut(4) {
        rooms.elements.extend(volumes.iter().copied());
    }
    let n = |k: &str| built.get(k).and_then(Value::as_u64).unwrap_or(0);
    let levels = n("levels");
    // 1. shell
    if let Some(x) = st.get_mut(0) {
        x.count("levels", levels);
        x.count("exterior_walls", exterior.len());
        for k in ["floors", "columns", "gridColumns", "curtainWalls", "roofs", "parapets", "stairs", "ramps", "concrete"] {
            x.count(k, n(k));
        }
        if !unsorted.is_empty() {
            x.notes.push(format!(
                "other built parts kept with the shell: {}",
                unsorted.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
            ));
        }
        for (h, level, _) in &walls {
            if exterior.contains(h) {
                x.level(level, "exterior_walls", 1.0);
            }
        }
    }
    // 2. walls
    let interior = walls.len().saturating_sub(exterior.len());
    let wall_ratio = if arch_walls > 0 {
        (arch_walls + other_walls) as f64 / arch_walls as f64
    } else if other_walls > 0 {
        f64::INFINITY
    } else {
        1.0
    };
    if let Some(x) = st.get_mut(1) {
        x.count("walls", walls.len());
        x.count("interior_walls", interior);
        x.count("arch_walls", arch_walls);
        x.count("walls_from_other_sheets", other_walls);
        x.count("wall_ratio", (wall_ratio * 1000.0).round() / 1000.0);
        for (h, level, _) in &walls {
            if !exterior.contains(h) {
                x.level(level, "walls", 1.0);
            }
        }
    }
    // 3. openings: each in its host wall.
    let mut hosted = 0usize;
    for (_, level, o) in &openings {
        let c = o.centre();
        let ok = walls.iter().any(|(_, wl, f)| {
            if wl != level {
                return false;
            }
            let (s0, off) = f.local(c);
            off.abs() <= f.thick / 2.0 + upf * 0.5 && s0 >= -upf * 0.5 && s0 <= f.len() + upf * 0.5
        });
        hosted += usize::from(ok);
    }
    if let Some(x) = st.get_mut(2) {
        x.count("doors", n("doors"));
        x.count("windows", n("windows"));
        x.count("in_walls", hosted);
        x.count("not_in_a_wall", openings.len().saturating_sub(hosted));
    }
    // 4. framing
    let framed = {
        let d = s.doc_mut()?;
        frame_walls(d, &walls, &openings, &exterior, upf)
    };
    if let Some(x) = st.get_mut(3) {
        x.count("walls_framed", framed.walls);
        x.count("studs", framed.studs);
        x.count("headers", framed.headers);
        x.count("placeholder_headers", framed.placeholder_headers);
        x.count("members_lf", (framed.members_lf * 10.0).round() / 10.0);
        x.count("plates_lf", (framed.plates_lf * 10.0).round() / 10.0);
        for (level, (w, studs)) in &framed.by_level {
            x.level(level, "walls_framed", *w as f64);
            x.level(level, "studs", *studs as f64);
        }
        if framed.placeholder_headers > 0 {
            x.notes.push(format!(
                "{} header(s) in non-bearing walls are placeholder box headers: the code book gives no header table for them and the prints' header schedule was not read",
                framed.placeholder_headers
            ));
        }
        x.elements.clone_from(&framed.elements);
        if framed.walls == 0 && !walls.is_empty() {
            x.status = "failed";
        }
    }
    // 5. rooms
    if let Some(x) = st.get_mut(4) {
        // The model's rooms: the build's traced ones, else the volumes rooms.build made from the
        // plans' labels.
        let traced = n("rooms");
        x.count("rooms", traced.max(volumes.len() as u64));
        x.count("rooms_traced", traced);
        x.count("room_volumes", volumes.len());
        if traced == 0 && !volumes.is_empty() {
            x.notes.push(format!("the build traced no rooms; the {} room volumes are from the plans' labels", volumes.len()));
        }
        if let Some(by) = built.get("roomsByLevel").and_then(Value::as_object) {
            for (level, v) in by {
                x.level(level, "rooms", v.as_f64().unwrap_or(0.0));
            }
        }
    }
    // 6. mep
    let mep = readings.mep.clone().unwrap_or_else(|| json!({}));
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let registered = readings.sheets.iter().filter(|r| r["registered"] == json!(true)).count();
    if let Some(x) = st.get_mut(5) {
        x.count("mep_sheets", readings.sheets.len());
        x.count("registered", registered);
        x.count("duct_ft", num(&mep, "duct_ft"));
        x.count("pipe_ft", num(&mep, "pipe_ft"));
        x.count("duct_runs", num(&mep, "duct_runs"));
        x.count("pipe_runs", num(&mep, "pipe_runs"));
        x.count("conduit_ft", mep.get("conduit").map_or(0.0, |c| num(c, "drawn_ft") + num(c, "estimated_ft")));
        x.count("devices", readings.devices);
        x.count("mep_elements_built", n("mepRuns"));
        x.count("terminals", n("terminals"));
        x.count("equipment", n("mepEquipment"));
        let dropped: u64 = mep["sheets"].as_array().map_or(0, |a| a.iter().filter_map(|r| r["dropped_on_walls"].as_u64()).sum());
        x.count("runs_on_walls_dropped", dropped);
        for r in mep["sheets"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let level = r["level"].as_str().unwrap_or_default();
            let key = if r["trade"] == "duct" { "duct_ft" } else { "pipe_ft" };
            x.level(level, key, r["length_ft"].as_f64().unwrap_or(0.0));
        }
        x.notes.extend(readings.warnings.iter().take(20).cloned());
    }
    // 7. drywall
    match s.execute("drywall.takeoff", &json!({"groupBy": "level"})) {
        Ok(t) => {
            let tot = &t["totals"];
            let walls_sf = num(tot, "boardAreaSqFt");
            let ceil_sf = num(tot, "ceilingBoardAreaSqFt") + num(tot, "soffitBoardAreaSqFt");
            if let Some(x) = st.get_mut(6) {
                x.count("drywall_sf", ((walls_sf + ceil_sf) * 10.0).round() / 10.0);
                x.count("wall_board_sf", (walls_sf * 10.0).round() / 10.0);
                x.count("ceiling_board_sf", (ceil_sf * 10.0).round() / 10.0);
                x.count("ceilings", n("ceilings"));
                if walls_sf + ceil_sf <= 0.0 && !walls.is_empty() {
                    x.status = "failed";
                    x.notes.push(format!("drywall.takeoff found no board on the {} walls built", walls.len()));
                }
                for row in t["rows"].as_array().map(Vec::as_slice).unwrap_or_default() {
                    let level = row["group"].as_str().unwrap_or_default();
                    let sf: f64 = row["boards"].as_array().map_or(0.0, |b| b.iter().map(|x| num(x, "areaSqFt")).sum());
                    x.level(level, "drywall_sf", sf);
                }
            }
        }
        Err(e) => {
            if let Some(x) = st.get_mut(6) {
                x.status = "failed";
                x.notes.push(format!("drywall.takeoff: {e}"));
            }
        }
    }
    // 8. finishes
    let summary = s.execute("bim.props.summary", &json!({"kind": "room"})).ok();
    let filled = |k: &str| summary.as_ref().and_then(|v| v["kinds"]["room"]["filled"][k].as_u64()).unwrap_or(0);
    let casework = parts.iter().filter(|(_, p, _)| stage_of_part(p, false) == 7).count();
    if let Some(x) = st.get_mut(7) {
        x.count("rooms_with_floor_finish", filled("floor_finish"));
        x.count("rooms_with_wall_finish", filled("wall_finish"));
        x.count("rooms_with_ceiling_finish", filled("ceiling_finish"));
        x.count("casework", casework);
        if summary.is_none() {
            x.notes.push("the room properties could not be read".into());
        }
    }
    // Every element in one stage only: the earliest stage that lists it keeps it.
    let mut seen = BTreeSet::new();
    for x in &mut st {
        x.elements.retain(|h| seen.insert(*h));
        x.count("elements", x.elements.len());
    }
    store(s.doc_mut()?, &st);
    let questions = questions(&st, readings);
    let staged = json!({
        "wall_ratio": if wall_ratio.is_finite() { json!((wall_ratio * 1000.0).round() / 1000.0) } else { json!(null) },
        "arch_walls": arch_walls,
        "levels": levels,
        "questions": questions,
    });
    Ok((st, staged))
}

/// The checkpoints' questions: a level with no rooms, openings not in a wall, a trade plan not
/// lined up.
fn questions(st: &[Stage], readings: &Readings) -> Vec<Value> {
    let mut q = Vec::new();
    if let Some(rooms) = st.get(4)
        && let Some(walls) = st.get(1)
    {
        for level in walls.by_level.keys() {
            if rooms.by_level.get(level).and_then(|m| m.get("rooms")).and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
                q.push(json!({"stage": "rooms", "level": level, "question": format!("{level} has walls but no rooms — are its room tags on another sheet?")}));
            }
        }
    }
    if let Some(o) = st.get(2)
        && let Some(n) = o.counts.get("not_in_a_wall").and_then(Value::as_u64).filter(|n| *n > 0)
    {
        q.push(json!({"stage": "openings", "question": format!("{n} door(s) or window(s) are not in a wall — check them on the plans")}));
    }
    for r in readings.sheets.iter().filter(|r| r["registered"] != json!(true)) {
        q.push(json!({
            "stage": "mep",
            "sheet": r["sheet"],
            "level": r["level"],
            "matched_pct": r["matched_pct"],
            "question": format!(
                "{} did not line up on the architectural plan of {} ({}% of its lines match): wrong level, another revision, or no background? Its trade was placed by its sheet position, background and all — check it",
                r["sheet"].as_str().unwrap_or("?"),
                r["level"].as_str().unwrap_or("?"),
                (r["matched_pct"].as_f64().unwrap_or(0.0) * 100.0).round()
            ),
        }));
    }
    q.truncate(MAX_QUESTIONS * STAGES.len());
    q
}

/// The eight stages as the answer lists them; `skipped` when nothing was built.
pub(crate) fn skipped(why: &str) -> Vec<Value> {
    STAGES.iter().map(|n| json!({"name": n, "status": "skipped", "counts": {}, "by_level": {}, "elements": 0, "notes": [why]})).collect()
}

/// The flat numbers of a staged build, for agents and gates.
pub(crate) fn flat(stages: &[Value], staged: &Value, readings: &Readings, built: bool) -> Value {
    let names: Vec<&str> = stages.iter().filter_map(|s| s["name"].as_str()).collect();
    let done = stages.iter().filter(|s| s["status"] == "done").count();
    let c = |stage: &str, k: &str| stages.iter().find(|s| s["name"] == stage).and_then(|s| s["counts"].get(k).cloned()).unwrap_or(json!(0));
    json!({
        "ok": built && done == STAGES.len(),
        "stages_8ok": names == STAGES,
        "stages_done": done,
        "levels": staged["levels"],
        "arch_walls": staged["arch_walls"],
        "wall_ratio": staged["wall_ratio"],
        "mep_sheets": readings.sheets.len(),
        "registered": readings.sheets.iter().filter(|r| r["registered"] == json!(true)).count(),
        "stage_rooms": c("rooms", "rooms"),
        "duct_ft": c("mep", "duct_ft"),
        "pipe_ft": c("mep", "pipe_ft"),
        "stage_drywall_sf": c("drywall", "drywall_sf"),
        "stage_studs": c("framing", "studs"),
    })
}

/// Read the trade plans before the build: devices on the electrical plans, then each mechanical
/// and plumbing plan lined up on its level's architectural plan and read on what is left of it
/// (into the stored spec), and every trade plan's registration.
pub(crate) fn read_trades(s: &mut Session, job: &mut super::guided::GuidedJob, refs: &[super::printmep::SheetRef]) -> Readings {
    let mut r = Readings::default();
    match super::guided::run_devices(s, job) {
        Ok(_) => r.devices = job.devices.len(),
        Err(e) => r.warnings.push(format!("devices: {e}")),
    }
    match super::guided::run_mep_read_stage(s, job) {
        Ok(v) => r.mep = Some(v),
        Err(e) => r.warnings.push(format!("mep read: {e}")),
    }
    r.sheets = registrations(s, r.mep.as_ref(), refs, &mut r.warnings);
    if !job.devices.is_empty() {
        match super::readers::place_devices(s, &job.devices, "bim.from_pdf") {
            Ok(_) => {}
            Err(e) => r.warnings.push(format!("the devices could not be put in the model: {e}")),
        }
    }
    r
}

/// Every trade plan's registration: the M and P plans' from their reading (`mep`), the others'
/// (electrical, fire protection) lined up here.
fn registrations(s: &Session, mep: Option<&Value>, refs: &[super::printmep::SheetRef], warnings: &mut Vec<String>) -> Vec<Value> {
    let mut by_sheet: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(m) = mep {
        for sh in m["sheets"].as_array().map(Vec::as_slice).unwrap_or_default() {
            if let (Some(n), Some(reg)) = (sh["sheet"].as_str(), sh.get("registration").filter(|v| !v.is_null())) {
                by_sheet.entry(n.to_string()).or_insert_with(|| reg.clone());
            }
            if let Some(w) = sh["warnings"].as_array() {
                warnings.extend(w.iter().filter_map(Value::as_str).map(str::to_string).take(4));
            }
        }
    }
    let upf = super::bimwork::units_per_foot(s);
    if let Ok(d) = s.doc() {
        let done: BTreeSet<String> = by_sheet.keys().cloned().collect();
        for reg in super::register::register_all(d, refs, upf, &|sh| !done.contains(&sh.number)) {
            by_sheet.entry(reg.sheet.clone()).or_insert_with(|| reg.to_json());
        }
    }
    by_sheet.into_values().collect()
}

/// The guided job's BIM build in stages: the readings its devices and MEP stages made, the model
/// its build made sorted into the eight stages (kept in the drawing). Adds `build_stages` and
/// `staged` to the build stage's `result`.
pub(crate) fn job_stages(s: &mut Session, job: &super::guided::GuidedJob, mep: Option<Value>, result: &mut Value) {
    let refs = super::guided::job_sheet_refs(job);
    let mut r = Readings { devices: job.devices.len(), mep, ..Readings::default() };
    r.sheets = registrations(s, r.mep.as_ref(), &refs, &mut r.warnings);
    match stage_model(s, result, &r, &refs) {
        Ok((st, sum)) => {
            let stages: Vec<Value> = st.iter().map(Stage::to_json).collect();
            result["staged"] = flat(&stages, &sum, &r, true);
            result["build_stages"] = json!(stages);
            result["build_questions"] = sum["questions"].clone();
            result["sheets_registered"] = json!(r.sheets);
        }
        Err(e) => result["build_stages_error"] = json!(e.to_string()),
    }
}

/// `bim.from_pdf`: one-call PDF → BIM, built in stages. The print set (one PDF of any number of
/// pages, or a folder) is classified and its plan pages imported, the import checked, the spec
/// drafted with one stacked level per architectural floor plan; the trade plans are lined up on
/// their levels' architectural plans and read on what their backgrounds leave; then the model is
/// built and sorted into the eight stages ([`STAGES`]), each kept as a checkpoint, and saved as
/// .cdy at `out` (default: a temp folder). Nothing is written next to the PDFs.
pub(crate) fn run_bim_from_pdf(s: &mut Session, p: &Value) -> Result<Value> {
    use super::bspec::{bim_temp_out, build_check, check_blocks_build, current, detect_floor_levels};
    let path = str_param(p, "path").ok_or_else(|| bad("bim.from_pdf", "`path` is required (a PDF file or folder)"))?;
    let force = super::bool_or(p, "force", false);
    let path_obj = std::path::Path::new(path);
    if !path_obj.exists() {
        return Err(bad("bim.from_pdf", format!("{path}: file or folder not found")));
    }
    let is_pdf = |p: &std::path::Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    let has_pdfs = if path_obj.is_dir() {
        std::fs::read_dir(path_obj).is_ok_and(|entries| entries.flatten().any(|e| e.path().is_file() && is_pdf(&e.path())))
    } else {
        is_pdf(path_obj)
    };
    if !has_pdfs {
        return Err(bad("bim.from_pdf", format!("{path}: no PDF files found. Next: provide a path to a PDF file or folder.")));
    }
    let stem = path_obj.file_stem().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Building".into());
    let out = match str_param(p, "out").filter(|o| !o.trim().is_empty()) {
        Some(o) => std::path::PathBuf::from(o),
        None => bim_temp_out(&stem).ok_or_else(|| bad("bim.from_pdf", "`out` is required here (the .cdy file to save)"))?,
    };

    let mut cdy_paths: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut built_counts = json!({});
    let mut guesses: Vec<Value> = Vec::new();

    // Classify the set and import its plan pages, as the guided job does.
    s.echo(format!("bim.from_pdf: reading the sheet index of {stem}..."));
    s.new_drawing_with_viewports(false);
    let mut plan = super::guided::load_plan_pages(s, path_obj)?;
    let refs = super::guided::job_sheet_refs(&plan.job);
    let names: Vec<String> = plan
        .pages
        .iter()
        .map(|pg| {
            let sheet = pg.get("sheet").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("(no number)");
            match pg.get("level").and_then(Value::as_str).filter(|l| !l.is_empty()) {
                Some(l) => format!("{sheet} (level {l})"),
                None => sheet.to_string(),
            }
        })
        .collect();
    let pages_note = if plan.indexed == 0 {
        format!("no sheet index read; imported every page ({})", names.len())
    } else {
        format!("imported {} of {} sheets: {}", names.len(), plan.indexed, names.join(", "))
    };
    s.echo(format!("bim.from_pdf: {pages_note}"));

    // The import check of everything imported. A hard fail is never a pass.
    let check = super::import_check::check_drawing(s.doc()?);
    let check_results = vec![build_check(path, &check)];
    let hard = check.hard_fails.join(", ");
    let blocked = !check.hard_fails.is_empty() && check_blocks_build(&check) && !force;
    if names.is_empty() {
        warnings.push(format!("{stem}: no page could be imported"));
    } else if blocked {
        warnings.push(format!(
            "{stem}: import check failed ({hard}; score {}/100); skipping build. Use force:true to build anyway, or fix the import first with: import_check {{}}",
            check.score
        ));
    } else if !check.hard_fails.is_empty() {
        let why = if force { "force=true, building anyway" } else { "SCALE mismatch with a detected scale note — proceeding" };
        warnings.push(format!("{stem}: import check failed ({hard}; score {}/100); {why}", check.score));
    }

    let mut stages: Vec<Value> = skipped("nothing was built");
    let mut staged = json!({"levels": 0, "arch_walls": 0, "wall_ratio": null, "questions": []});
    let mut readings = Readings::default();
    let mut built_ok = false;
    if !names.is_empty() && !blocked {
        // The spec: one stacked level per architectural floor plan (one sheet showing several
        // floor titles: those are the levels).
        s.echo(format!("bim.from_pdf: drafting spec for {stem}..."));
        let floors = if plan.levels.len() <= 1 { detect_floor_levels(s).ok().filter(|f| f.len() >= 2) } else { None };
        if let Some(levels_found) = floors {
            s.execute("bspec.draft", &json!({"store": true}))?;
            let levels_json: Vec<Value> = levels_found
                .iter()
                .enumerate()
                .map(|(i, fl)| {
                    let elevation = i as f64 * 10.0; // 10' floor-to-floor default
                    json!({"name": fl.name, "elevation_ft": elevation, "height_ft": 10.0, "ceiling_ft": 8.0, "region": fl.region})
                })
                .collect();
            let _ = s.execute("bspec.set", &json!({"spec": {"levels": levels_json}}));
            s.echo(format!(
                "bim.from_pdf: detected {} levels: {}",
                levels_found.len(),
                levels_found.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        } else {
            let draft = if plan.levels.is_empty() { json!({"store": true}) } else { json!({"store": true, "levels": plan.levels, "stack": true}) };
            let spec = s.execute("bspec.draft", &draft)?;
            guesses = spec.get("guesses").and_then(Value::as_array).map(|g| g.iter().take(200).cloned().collect()).unwrap_or_default();
        }
        // Where each imported sheet sits: what lies on the other sheets is theirs.
        if let Ok((mut sp, true)) = current(s, "bim.from_pdf") {
            super::guided::add_sheet_links(&mut sp, &plan.regions);
            s.doc_mut()?.building_spec = Some(sp.to_json());
        }

        // The trade plans: lined up on the architectural plans, backgrounds removed, read.
        s.echo(format!("bim.from_pdf: lining up the trade plans of {stem} and reading them..."));
        readings = read_trades(s, &mut plan.job, &refs);

        // The rooms read on each architectural plan go in the spec, so the build traces them in
        // their walls (a print set has no room tag layers to find them by).
        match super::guided::rooms_into_spec(s, &mut plan.job) {
            Ok((read, added)) => s.echo(format!("bim.from_pdf: {read} rooms read on the plans, {added} added to the spec")),
            Err(e) => warnings.push(format!("{stem}: the rooms could not be read: {e}")),
        }

        // Build the model once, then sort it into its stages.
        s.echo(format!("bim.from_pdf: building 3D model for {stem}..."));
        match s.execute("model.build", &json!({})) {
            Ok(r) => built_counts = r,
            Err(e) => warnings.push(format!("{stem}: model.build failed: {e}")),
        }
        if built_counts.as_object().is_some_and(|o| !o.is_empty()) {
            // The rooms as elements, what the readers found on every element, and each door
            // between its two rooms (a failure is a warning, never the end of the build).
            for (cmd, params) in [("rooms.build", json!({})), ("bim.props.fill", json!({}))] {
                if let Err(e) = s.execute(cmd, &params) {
                    warnings.push(format!("{cmd}: {e}"));
                }
            }
            if let Err(e) = super::identity::identity(s, true, None) {
                warnings.push(format!("element identities: {e}"));
            }
            match stage_model(s, &built_counts, &readings, &refs) {
                Ok((st, sum)) => {
                    stages = st.iter().map(Stage::to_json).collect();
                    staged = sum;
                    built_ok = true;
                }
                Err(e) => warnings.push(format!("{stem}: the stages could not be sorted: {e}")),
            }
            // Save at `out` (its images go next to it).
            let cdy_str = out.to_string_lossy().to_string();
            let made = out.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all);
            match made.map_err(|e| e.to_string()).and_then(|()| s.execute("saveas", &json!({ "path": cdy_str })).map_err(|e| e.to_string())) {
                Ok(_) => {
                    s.echo(format!("bim.from_pdf: saved {cdy_str}"));
                    cdy_paths.push(cdy_str);
                }
                Err(e) => warnings.push(format!("{stem}: could not save .cdy: {e}")),
            }
        }
    }
    warnings.extend(readings.warnings.iter().take(50).cloned());

    let mut next_steps: Vec<String> = Vec::new();
    if cdy_paths.is_empty() {
        next_steps.push("import_check {} — review import quality and fix issues before building".into());
        next_steps.push("bim.from_pdf {path: \"...\", force: true} — force build even with import check failures".into());
    } else {
        next_steps.push("job.stage_view {stage: \"walls\"} — the model as it stood after a stage (\"all\" shows everything)".into());
        next_steps.push("render {} — render the 3D view to see the building".into());
        next_steps.push("bspec.set {spec: {...}} — refine the building spec (room heights, materials, etc.)".into());
        next_steps.push("model.build {} — rebuild after spec changes".into());
    }

    // A per-level breakdown from the notes (e.g. "Level 3: 1303 walls ...").
    let mut built: serde_json::Map<String, Value> = serde_json::Map::new();
    let levels_count = built_counts.get("levels").and_then(Value::as_u64).unwrap_or(0);
    if let Some(notes) = built_counts.get("notes").and_then(Value::as_array) {
        for text in notes.iter().filter_map(Value::as_str) {
            let Some(rest) = text.strip_prefix("Level ") else { continue };
            let Some((n, desc)) = rest.split_once(':') else { continue };
            let mut walls = 0u64;
            for w in desc.split_whitespace().collect::<Vec<&str>>().windows(2) {
                if let [a, b] = w
                    && b.starts_with("wall")
                    && let Ok(v) = a.parse::<u64>()
                {
                    walls = v;
                }
            }
            built.insert(format!("Level {}", n.trim()), json!({"walls": walls}));
        }
    }
    let flat = flat(&stages, &staged, &readings, built_ok);
    Ok(json!({
        "cdy_paths": cdy_paths,
        "pages": plan.pages,
        "links": plan.links,
        "pages_note": pages_note,
        "spec_guesses": guesses,
        "check_results": check_results,
        "built_counts": built_counts,
        "levels": if levels_count > 1 { json!(built.keys().collect::<Vec<_>>()) } else { json!(levels_count) },
        "built": built,
        "stages": stages,
        "sheets": readings.sheets,
        "questions": staged["questions"],
        "staged": flat,
        "wall_ratio": staged["wall_ratio"],
        "warnings": warnings,
        "next_steps": next_steps,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_sort_into_their_stages() {
        assert_eq!(stage_of_part("wall", true), 0);
        assert_eq!(stage_of_part("wall", false), 1);
        assert_eq!(stage_of_part("door", false), 2);
        assert_eq!(stage_of_part("window", false), 2);
        assert_eq!(stage_of_part(FRAMING_PART, false), 3);
        assert_eq!(stage_of_part("room", false), 4);
        assert_eq!(stage_of_part("mep", false), 5);
        assert_eq!(stage_of_part("device", false), 5);
        assert_eq!(stage_of_part("ceiling", false), 6);
        assert_eq!(stage_of_part("casework", false), 7);
        for p in ["floor", "roof", "column", "stair", "concrete", "parapet"] {
            assert_eq!(stage_of_part(p, false), 0, "{p}");
        }
    }

    #[test]
    fn the_outside_walls_of_a_box_of_rooms_are_exterior() {
        let f = |a: [f64; 2], b: [f64; 2]| Foot { a, b, thick: 6.0, z: [0.0, 120.0] };
        let walls: Vec<(Handle, String, Foot)> = [
            f([0.0, 0.0], [480.0, 0.0]),
            f([480.0, 0.0], [480.0, 240.0]),
            f([480.0, 240.0], [0.0, 240.0]),
            f([0.0, 240.0], [0.0, 0.0]),
            // A partition across the middle.
            f([240.0, 0.0], [240.0, 240.0]),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, x)| (Handle(i as u64 + 1), "Level 1".to_string(), x))
        .collect();
        let ext = exterior_walls(&walls);
        assert_eq!(ext.len(), 4, "{ext:?}");
        assert!(!ext.contains(&Handle(5)));
    }

    #[test]
    fn a_wall_with_a_door_gets_king_jack_studs_and_a_header() {
        let mut d = Drawing::default();
        let wall = (Handle(10), "Level 1".to_string(), Foot { a: [0.0, 0.0], b: [240.0, 0.0], thick: 4.5, z: [0.0, 108.0] });
        let door = (Handle(11), "Level 1".to_string(), Foot { a: [100.0, 0.0], b: [136.0, 0.0], thick: 4.5, z: [0.0, 84.0] });
        let framed = frame_walls(&mut d, &[wall], &[door], &BTreeSet::new(), 12.0);
        assert_eq!(framed.walls, 1);
        assert_eq!(framed.headers, 1, "one header over the door");
        // Non-bearing 2x4 at the code's 24" o.c.: ends, king + jack each side, field studs.
        assert!(framed.studs >= 2 + 4 + 6, "{}", framed.studs);
        assert_eq!(framed.placeholder_headers, 1, "no header rule for a non-bearing wall");
        assert_eq!(framed.elements.len(), 1);
        let e = d.entity(framed.elements[0]).expect("framing element");
        let props = cadaddy_doc::props::read(&e.common.xdata);
        assert_eq!(props.get("studs").map(|p| p.text()), Some(framed.studs.to_string()));
        assert_eq!(props.get("spacing").and_then(|p| p.text().parse::<f64>().ok()), Some(24.0));
        assert_eq!(super::super::wallgen::made_as(e).as_deref(), Some(FRAMING_PART));
    }

    #[test]
    fn ray_hits_a_segment_ahead_only() {
        assert!(ray_hits([0.0, 0.0], [1.0, 0.0], [5.0, -1.0], [5.0, 1.0], 10.0));
        assert!(!ray_hits([0.0, 0.0], [-1.0, 0.0], [5.0, -1.0], [5.0, 1.0], 10.0));
        assert!(!ray_hits([0.0, 0.0], [1.0, 0.0], [5.0, 1.0], [5.0, 2.0], 10.0));
        assert!(!ray_hits([0.0, 0.0], [1.0, 0.0], [50.0, -1.0], [50.0, 1.0], 10.0));
    }
}
