//! BLOCKIFY: find the other copies of a group of objects — moved, turned or uniformly scaled,
//! within a tolerance — and replace them (and the group) with references to one block, new or
//! existing.
//!
//! Matching is geometric and deterministic. One template object with a direction (a line, arc,
//! polyline, ellipse, text or block reference; else two circles or points) is the anchor: each
//! object of its kind in the search set proposes the similarity that would carry the anchor onto
//! it, and the proposal holds when every other template object, carried the same way, lands on an
//! unused object of its kind with the same geometry. Groups never share objects.

use std::collections::{HashMap, HashSet};

use cadaddy_doc::{Entity, EntityKind, Handle};
use cadaddy_geom::{Bounds2, Similarity, Vec2};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most objects a template may have, and most matches.
const MAX_TEMPLATE: usize = 500;
const MAX_MATCHES: usize = 10_000;
/// Most anchor proposals tried.
const MAX_PROPOSALS: usize = 2_000_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("blockify", "Convert Matches to Block", run)
            .menu(&["Draw", "Block", "Convert Matches to Block"])
            .alias(&["bconvert"])
            .params("{handles? (the group to find; default the selection) | block?: an existing block to find (its contents), name? (the new block; default BLOCKIFY<n>), within?: [hex] (search only these; default the current space), tolerance? (drawing units; default a millionth of the group's size), replace?: bool (default true; false only reports the matches), keepTemplate?: bool (default false: the group becomes a reference too)} → {block, matches: [{handles, at, rotation, scale}], replaced}")
            .interactive(|_| Ok(Box::new(BlockifyM::default()))),
    ]
}

/// The object kinds BLOCKIFY matches.
fn supported(k: &EntityKind) -> bool {
    matches!(
        k,
        EntityKind::Line(_)
            | EntityKind::Circle(_)
            | EntityKind::Arc(_)
            | EntityKind::LwPolyline(_)
            | EntityKind::Ellipse(_)
            | EntityKind::Point(_)
            | EntityKind::Text(_)
            | EntityKind::MText(_)
            | EntityKind::Insert(_)
    )
}

fn kind_id(k: &EntityKind) -> u8 {
    match k {
        EntityKind::Line(_) => 1,
        EntityKind::Circle(_) => 2,
        EntityKind::Arc(_) => 3,
        EntityKind::LwPolyline(_) => 4,
        EntityKind::Ellipse(_) => 5,
        EntityKind::Point(_) => 6,
        EntityKind::Text(_) => 7,
        EntityKind::MText(_) => 8,
        EntityKind::Insert(_) => 9,
        _ => 0,
    }
}

/// An object's directed point pairs for proposals: (from, to). The anchor uses its first; a
/// candidate offers every way it could be the anchor (a line either way round, a closed polyline
/// from any vertex).
fn pairs(k: &EntityKind, all: bool) -> Vec<(Vec2, Vec2)> {
    match k {
        EntityKind::Line(l) => {
            let (a, b) = (l.a.xy(), l.b.xy());
            if all { vec![(a, b), (b, a)] } else { vec![(a, b)] }
        }
        EntityKind::Arc(a) => {
            let c = a.center.xy();
            vec![(c, c + Vec2::from_angle(a.start) * a.radius)]
        }
        EntityKind::Ellipse(e) => {
            let c = e.center.xy();
            let m = e.major.xy();
            if all { vec![(c, c + m), (c, c - m)] } else { vec![(c, c + m)] }
        }
        EntityKind::LwPolyline(p) => {
            let v: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
            let n = v.len();
            if n < 2 {
                return Vec::new();
            }
            if !all {
                return v.first().zip(v.get(1)).map(|(a, b)| vec![(*a, *b)]).unwrap_or_default();
            }
            let mut out = Vec::new();
            let segs = if p.closed { n } else { n - 1 };
            for i in 0..segs.min(1024) {
                if let (Some(a), Some(b)) = (v.get(i), v.get((i + 1) % n)) {
                    out.push((*a, *b));
                    out.push((*b, *a));
                }
            }
            out
        }
        EntityKind::Text(t) => {
            let p = t.justify_point();
            vec![(p, p + Vec2::from_angle(t.rotation) * t.height)]
        }
        EntityKind::MText(t) => {
            let p = t.insert.xy();
            vec![(p, p + Vec2::from_angle(t.rotation) * t.height)]
        }
        EntityKind::Insert(i) => {
            let p = i.insert.xy();
            vec![(p, p + Vec2::from_angle(i.rotation) * i.scale.x.abs().max(1e-12))]
        }
        _ => Vec::new(),
    }
}

/// A point the object is found by (its centre for circles and points).
fn locus(k: &EntityKind) -> Option<Vec2> {
    Some(match k {
        EntityKind::Circle(c) => c.center.xy(),
        EntityKind::Point(p) => p.p.xy(),
        _ => pairs(k, false).first()?.0,
    })
}

fn near(a: Vec2, b: Vec2, tol: f64) -> bool {
    a.dist(b) <= tol
}

fn angle_near(a: f64, b: f64, tol: f64) -> bool {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    d.min(std::f64::consts::TAU - d) <= tol
}

/// Whether `a` (a template object already carried onto the candidate's place) is `b`.
fn same(a: &EntityKind, b: &EntityKind, tol: f64) -> bool {
    let at = tol.max(1e-12);
    // Angles: a tolerance's worth at the object's size.
    let ang = |size: f64| (at / size.abs().max(1e-12)).min(0.01);
    match (a, b) {
        (EntityKind::Line(x), EntityKind::Line(y)) => {
            let (xa, xb, ya, yb) = (x.a.xy(), x.b.xy(), y.a.xy(), y.b.xy());
            (near(xa, ya, at) && near(xb, yb, at)) || (near(xa, yb, at) && near(xb, ya, at))
        }
        (EntityKind::Circle(x), EntityKind::Circle(y)) => near(x.center.xy(), y.center.xy(), at) && (x.radius - y.radius).abs() <= at,
        (EntityKind::Arc(x), EntityKind::Arc(y)) => {
            let ends =
                |a: &cadaddy_doc::Arc| (a.center.xy() + Vec2::from_angle(a.start) * a.radius, a.center.xy() + Vec2::from_angle(a.end) * a.radius);
            let (xs, xe) = ends(x);
            let (ys, ye) = ends(y);
            near(x.center.xy(), y.center.xy(), at) && (x.radius - y.radius).abs() <= at && near(xs, ys, at) && near(xe, ye, at)
        }
        (EntityKind::Ellipse(x), EntityKind::Ellipse(y)) => {
            let major = near(x.major.xy(), y.major.xy(), at) || near(x.major.xy(), y.major.xy() * -1.0, at);
            let pt = |e: &cadaddy_doc::Ellipse, t: f64| {
                let m = e.major.xy();
                e.center.xy() + m * t.cos() + m.perp() * (e.ratio * t.sin())
            };
            near(x.center.xy(), y.center.xy(), at)
                && major
                && (x.ratio - y.ratio).abs() <= ang(x.major.xy().len())
                && near(pt(x, x.start), pt(y, y.start), at)
                && near(pt(x, x.end), pt(y, y.end), at)
        }
        (EntityKind::Point(x), EntityKind::Point(y)) => near(x.p.xy(), y.p.xy(), at),
        (EntityKind::LwPolyline(x), EntityKind::LwPolyline(y)) => {
            let n = x.vertices.len();
            if n != y.vertices.len() || x.closed != y.closed || n == 0 {
                return false;
            }
            let fits = |shift: usize, rev: bool| {
                (0..n).all(|i| {
                    let j = if rev { (shift + n - i) % n } else { (shift + i) % n };
                    let (Some(p), Some(q)) = (x.vertices.get(i), y.vertices.get(j)) else { return false };
                    // A reversed polyline's bulges belong to the vertex before, negated.
                    let qb = if rev { y.vertices.get((j + n - 1) % n).map_or(0.0, |v| -v.bulge) } else { q.bulge };
                    let last_open = !x.closed && i == n - 1;
                    near(p.p, q.p, at) && (last_open || (p.bulge - qb).abs() <= 1e-6)
                })
            };
            let shifts: Vec<usize> = if x.closed { (0..n.min(1024)).collect() } else { vec![0] };
            shifts.iter().any(|s| fits(*s, false)) || if x.closed { shifts.iter().any(|s| fits(*s, true)) } else { fits(n - 1, true) }
        }
        (EntityKind::Text(x), EntityKind::Text(y)) => {
            x.value == y.value
                && near(x.justify_point(), y.justify_point(), at)
                && (x.height - y.height).abs() <= at
                && angle_near(x.rotation, y.rotation, ang(x.height))
        }
        (EntityKind::MText(x), EntityKind::MText(y)) => {
            x.contents == y.contents
                && near(x.insert.xy(), y.insert.xy(), at)
                && (x.height - y.height).abs() <= at
                && angle_near(x.rotation, y.rotation, ang(x.height))
        }
        (EntityKind::Insert(x), EntityKind::Insert(y)) => {
            x.block.eq_ignore_ascii_case(&y.block)
                && near(x.insert.xy(), y.insert.xy(), at)
                && (x.scale.x - y.scale.x).abs() <= ang(1.0) * x.scale.x.abs().max(1e-9)
                && (x.scale.y - y.scale.y).abs() <= ang(1.0) * x.scale.y.abs().max(1e-9)
                && angle_near(x.rotation, y.rotation, 1e-6)
        }
        _ => false,
    }
}

/// One copy found: its objects (in template order) and the similarity carrying the template
/// onto it.
#[derive(Clone, Debug)]
pub struct Match {
    pub handles: Vec<Handle>,
    pub sim: Similarity,
}

/// The copies of `template` among `pool` (template objects are never part of a copy).
pub fn find(template: &[Entity], pool: &[std::sync::Arc<Entity>], tol: Option<f64>) -> Vec<Match> {
    let tb = Bounds2::from_points(template.iter().filter_map(|e| locus(&e.kind)));
    let size = if tb.is_empty() { 1.0 } else { tb.width().max(tb.height()).max(1e-9) };
    let tol = tol.unwrap_or(size * 1e-6).max(size * 1e-9);
    // The anchor: an object with a direction, the rarest kind first, then the longest.
    let count = |k: u8| pool.iter().filter(|e| kind_id(&e.kind) == k).count();
    let directed: Vec<usize> = (0..template.len()).filter(|i| template.get(*i).is_some_and(|e| !pairs(&e.kind, false).is_empty())).collect();
    let len = |i: usize| template.get(i).and_then(|e| pairs(&e.kind, false).first().map(|(p, q)| p.dist(*q))).unwrap_or(0.0);
    let kind_count = |i: usize| template.get(i).map_or(usize::MAX, |e| count(kind_id(&e.kind)));
    let anchor = directed.iter().copied().min_by(|a, b| kind_count(*a).cmp(&kind_count(*b)).then(len(*b).total_cmp(&len(*a))));
    // Without one (circles and points): two of them, as far apart as can be.
    type Proposal = (Vec2, Vec2, u8, Option<u8>, Option<(usize, usize)>);
    let reference: Option<Proposal> = match anchor {
        Some(i) => template.get(i).and_then(|e| pairs(&e.kind, false).first().map(|(p, q)| (*p, *q, kind_id(&e.kind), None, None))),
        None => {
            let mut best: Option<(f64, usize, usize)> = None;
            for (i, a) in template.iter().enumerate() {
                for (j, b) in template.iter().enumerate().skip(i + 1) {
                    if let (Some(p), Some(q)) = (locus(&a.kind), locus(&b.kind)) {
                        let d = p.dist(q);
                        if best.is_none_or(|x| d > x.0) {
                            best = Some((d, i, j));
                        }
                    }
                }
            }
            best.and_then(|(_, i, j)| {
                Some((
                    locus(&template.get(i)?.kind)?,
                    locus(&template.get(j)?.kind)?,
                    kind_id(&template.get(i)?.kind),
                    Some(kind_id(&template.get(j)?.kind)),
                    Some((i, j)),
                ))
            })
        }
    };
    let Some((ra, rb, rk, second, pair_ix)) = reference else {
        // A single circle or point: every one of its kind (any size) is a copy.
        let Some(t) = template.first() else { return Vec::new() };
        let Some(tp) = locus(&t.kind) else { return Vec::new() };
        let tr = match &t.kind {
            EntityKind::Circle(c) => c.radius,
            _ => 1.0,
        };
        return pool
            .iter()
            .filter(|e| kind_id(&e.kind) == kind_id(&t.kind))
            .filter_map(|e| {
                let p = locus(&e.kind)?;
                let s = match &e.kind {
                    EntityKind::Circle(c) => c.radius / tr.max(1e-300),
                    _ => 1.0,
                };
                Some(Match { handles: vec![e.handle], sim: Similarity { scale: s, angle: 0.0, offset: p - tp * s } })
            })
            .take(MAX_MATCHES)
            .collect();
    };
    // Every pool object by kind, and a grid of them by locus.
    let cell = size.max(1e-9);
    let key = |p: Vec2, s: f64| ((p.x / (cell * s)).floor() as i64, (p.y / (cell * s)).floor() as i64);
    let by_kind: HashMap<u8, Vec<usize>> = pool.iter().enumerate().fold(HashMap::new(), |mut m, (i, e)| {
        m.entry(kind_id(&e.kind)).or_default().push(i);
        m
    });
    let mut used: HashSet<Handle> = template.iter().map(|e| e.handle).collect();
    let mut out: Vec<Match> = Vec::new();
    let mut proposals = 0usize;
    let empty = Vec::new();
    let candidates = by_kind.get(&rk).unwrap_or(&empty);
    for &ci in candidates {
        let Some(c) = pool.get(ci) else { continue };
        if used.contains(&c.handle) {
            continue;
        }
        let offers: Vec<(Vec2, Vec2)> = match second {
            None => pairs(&c.kind, true),
            // Two circles/points: this one and each of the other kind at a plausible distance.
            Some(k2) => {
                let Some(p) = locus(&c.kind) else { continue };
                by_kind
                    .get(&k2)
                    .map(|v| {
                        v.iter().filter_map(|j| pool.get(*j)).filter(|o| o.handle != c.handle).filter_map(|o| Some((p, locus(&o.kind)?))).collect()
                    })
                    .unwrap_or_default()
            }
        };
        for (pa, pb) in offers {
            proposals += 1;
            if proposals > MAX_PROPOSALS || out.len() >= MAX_MATCHES {
                return out;
            }
            let Some(sim) = Similarity::from_pairs(ra, rb, pa, pb) else { continue };
            if !(sim.scale > 1e-9 && sim.scale < 1e9) {
                continue;
            }
            let m = sim.mat();
            let t = tol * sim.scale;
            // Every template object carried over must land on an unused object like it.
            let mut taken: Vec<Handle> = Vec::with_capacity(template.len());
            let mut ok = true;
            for (ti, te) in template.iter().enumerate() {
                let mut moved = te.kind.clone();
                moved.transform(&m);
                let want = locus(&moved);
                let pick = if anchor == Some(ti) || pair_ix.is_some_and(|(i, _)| i == ti) {
                    same(&moved, &c.kind, t).then_some(c.handle)
                } else {
                    by_kind.get(&kind_id(&te.kind)).and_then(|v| {
                        v.iter()
                            .filter_map(|j| pool.get(*j))
                            .filter(|o| !used.contains(&o.handle) && !taken.contains(&o.handle))
                            .filter(|o| match (want, locus(&o.kind)) {
                                (Some(w), Some(l)) => key(w, sim.scale) == key(l, sim.scale) || w.dist(l) <= cell * sim.scale,
                                _ => true,
                            })
                            .find(|o| same(&moved, &o.kind, t))
                            .map(|o| o.handle)
                    })
                };
                match pick {
                    Some(h) => taken.push(h),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                used.extend(taken.iter().copied());
                out.push(Match { handles: taken, sim });
                break;
            }
        }
    }
    out
}

/// A new block name: BLOCKIFY1, BLOCKIFY2…
fn next_name(d: &cadaddy_doc::Drawing) -> String {
    (1..).map(|i| format!("BLOCKIFY{i}")).find(|n| d.block(n).is_none()).unwrap_or_else(|| "BLOCKIFY".into())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "blockify";
    let space = s.space();
    let d = s.doc()?;
    let pool_all: Vec<std::sync::Arc<Entity>> = d.space(&space).map(|st| st.iter().cloned().collect()).unwrap_or_default();
    let tol = match p.get("tolerance") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|t| t.is_finite() && *t >= 0.0).ok_or_else(|| bad(cmd, "`tolerance` must be a distance of 0 or more"))?),
    };
    // The template: the selection (or handles), or an existing block's contents.
    let existing = str_param(p, "block").map(str::to_string);
    let (template, base): (Vec<Entity>, Vec2) = match &existing {
        Some(name) => {
            let b = d.block(name).ok_or_else(|| bad(cmd, format!("no block `{name}`")))?;
            (b.entities.iter().map(|e| (**e).clone()).collect(), b.base.xy())
        }
        None => {
            let hs = targets(s, p)?;
            let t: Vec<Entity> = hs.iter().filter_map(|h| d.entity(*h).map(|e| (**e).clone())).collect();
            let b =
                Bounds2::from_points(t.iter().map(|e| cadaddy_doc::entity_bounds(d, e, 0)).filter(|b| !b.is_empty()).flat_map(|b| [b.min, b.max]));
            (t, if b.is_empty() { Vec2::ZERO } else { b.min })
        }
    };
    if template.is_empty() {
        return Err(bad(cmd, "select the objects to look for (or name a `block`)"));
    }
    if template.len() > MAX_TEMPLATE {
        return Err(bad(cmd, format!("at most {MAX_TEMPLATE} objects in the group")));
    }
    if let Some(e) = template.iter().find(|e| !supported(&e.kind)) {
        return Err(bad(
            cmd,
            format!("BLOCKIFY matches lines, arcs, circles, polylines, ellipses, points, text and block references, not {}", e.kind.dxf_name()),
        ));
    }
    let within: Option<HashSet<Handle>> = match p.get("within") {
        None | Some(Value::Null) => None,
        Some(_) => Some(targets(s, &json!({ "handles": p.get("within") }))?.into_iter().collect()),
    };
    let pool: Vec<std::sync::Arc<Entity>> = pool_all
        .into_iter()
        .filter(|e| within.as_ref().is_none_or(|w| w.contains(&e.handle)))
        .filter(|e| supported(&e.kind) && !super::curves::is_locked(s, e.handle))
        .collect();
    let matches = find(&template, &pool, tol);
    let report: Vec<Value> = matches
        .iter()
        .map(|m| {
            let at = m.sim.apply(base);
            json!({ "handles": m.handles.iter().map(|h| h.hex()).collect::<Vec<_>>(), "at": [at.x, at.y], "rotation": m.sim.angle.to_degrees().rem_euclid(360.0), "scale": m.sim.scale })
        })
        .collect();
    if p.get("replace").and_then(Value::as_bool) == Some(false) {
        return Ok(json!({ "matches": report, "replaced": 0 }));
    }
    let keep_template = p.get("keepTemplate").and_then(Value::as_bool) == Some(true);
    // The block: the existing one, or the group made into one (it becomes a reference).
    let name = match &existing {
        Some(n) => n.clone(),
        None => {
            let name = str_param(p, "name").map_or_else(|| next_name(d), str::to_string);
            if d.block(&name).is_some() {
                return Err(bad(cmd, format!("block `{name}` already exists (pass `block` to use it)")));
            }
            let spec = find_command("block").ok_or_else(|| EngineError::UnknownCommand("block".into()))?;
            let hs: Vec<String> = template.iter().map(|e| e.handle.hex()).collect();
            (spec.run)(
                s,
                &json!({ "name": name, "base": [base.x, base.y], "handles": hs, "keep": if keep_template { "retain" } else { "convert" } }),
            )?;
            name
        }
    };
    let insert = find_command("insert").ok_or_else(|| EngineError::UnknownCommand("insert".into()))?;
    let mut replaced = 0;
    for m in &matches {
        // On the layer of the copy's first object.
        let layer = m.handles.first().and_then(|h| s.doc().ok()?.entity(*h).map(|e| e.common.layer.clone()));
        let at = m.sim.apply(base);
        let r = (insert.run)(s, &json!({ "name": name, "at": [at.x, at.y], "scale": m.sim.scale, "rotation": m.sim.angle.to_degrees() }))?;
        let doc = s.doc_mut()?;
        for h in &m.handles {
            doc.remove_entity(*h);
        }
        if let (Some(layer), Some(h)) = (layer, r.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex)) {
            doc.modify_entity(h, |e| e.common.layer = layer)?;
        }
        replaced += 1;
    }
    s.set_selection(Vec::new());
    s.echo(format!("{} cop{} of the group replaced with block {name}.", replaced, if replaced == 1 { "y" } else { "ies" }));
    Ok(json!({ "block": name, "matches": report, "replaced": replaced }))
}

/// Select the group, see the copies found (selected), then replace them with a block.
#[derive(Default)]
struct BlockifyM {
    sel: SelectPhase,
    group: Vec<Handle>,
    found: usize,
    stage: u8,
}

impl Interactive for BlockifyM {
    fn name(&self) -> &'static str {
        "BLOCKIFY"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return self.preview(s, hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            0 => self.sel.prompt(),
            1 => Prompt::new(format!("{} copies found. Replace them with a block?", self.found), Accept::TEXT).kw(&["Yes", "No"]).default("Yes"),
            _ => Prompt::new("Enter block name", Accept::TEXT).default(s.doc().map(next_name).unwrap_or_default()),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            s.set_selection(Vec::new());
            return Ok(Step::Cancel);
        }
        let t = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            _ => String::new(),
        };
        match self.stage {
            0 => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => self.preview(s, hs),
            },
            1 => {
                if t.to_ascii_lowercase().starts_with('n') {
                    s.set_selection(Vec::new());
                    return Ok(Step::Done);
                }
                self.stage = 2;
                Ok(Step::Continue)
            }
            _ => {
                let mut p = json!({ "handles": self.group.iter().map(|h| h.hex()).collect::<Vec<_>>() });
                if !t.is_empty()
                    && let Some(o) = p.as_object_mut()
                {
                    o.insert("name".into(), json!(t));
                }
                if let Err(e) = run(s, &p) {
                    s.echo(e.to_string());
                    return Ok(Step::Continue);
                }
                Ok(Step::Done)
            }
        }
    }
}

impl BlockifyM {
    /// Find the copies and select them, so they show.
    fn preview(&mut self, s: &mut Session, hs: Vec<Handle>) -> Result<Step> {
        self.group = hs;
        let r = run(s, &json!({ "handles": self.group.iter().map(|h| h.hex()).collect::<Vec<_>>(), "replace": false }));
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                s.echo(e.to_string());
                return Ok(Step::Done);
            }
        };
        let found: Vec<Handle> = r["matches"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|m| m["handles"].as_array().into_iter().flatten().filter_map(|h| h.as_str().and_then(Handle::parse_hex)))
            .collect();
        self.found = r["matches"].as_array().map_or(0, Vec::len);
        if self.found == 0 {
            s.echo("No copies of the group found.");
            return Ok(Step::Done);
        }
        s.set_selection(found);
        self.stage = 1;
        Ok(Step::Continue)
    }
}
