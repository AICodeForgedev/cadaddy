//! Stairs and ramps from the plan, for the building spec's `stairs`: `stairs.detect` finds them,
//! `stairs.list` says how they measure against the code limits, `stairs.confirm` marks them
//! checked; `model.build` builds them (steps, landings, ramps and handrails, from
//! `cadaddy_walk3d::stairs`), and Walk climbs them.
//!
//! A stair run in plan is a row of parallel tread lines of one length, evenly spaced a tread
//! apart (8″–14″), usually between two stringers; the UP or DN note at its foot (or head) says
//! which way it climbs. Runs whose ends meet across a landing are one stair. A ramp is a closed
//! outline holding a RAMP note or a slope (`1:12`, `8.33%`). Heights come from the levels: a
//! stair climbs from its level to the next one up, in equal risers.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::stairs::StairRun;
use cadaddy_walk3d::{BuildingSpec, Role, Stair, level_at};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{texts, units_per_inch};
use super::*;
use crate::{Result, Session};

/// Segments looked at, at most (hostile drawings); stairs found, at most.
const MAX_SEGS: usize = 100_000;
const MAX_STAIRS: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("stairs.detect", "Detect Stairs", run_detect).params(
            "{store?: bool (put them in the building spec, replacing unconfirmed stairs; confirmed ones stay)} → {stairs: [{name, ramp, level, runs: [{from, to, treads}], width_in, risers, riser_in, tread_in, direction: text|assumed, confidence, objects}], stored}",
        ),
        CommandSpec::new("stairs.list", "List Stairs", run_list)
            .params("{} → {stairs: [{name, ramp, level, to, risers, riser_in, tread_in, length_ft, width_in, confirmed, warnings}]}: the spec's stairs measured against the code limits")
            .noundo(),
        CommandSpec::new("stairs.confirm", "Confirm Stairs", run_confirm)
            .params("{names?: [name], all?: bool, confirmed?: bool (default true)} → {confirmed: [name]}"),
    ]
}

/// A straight segment of a plan object.
#[derive(Clone, Debug)]
struct Seg {
    a: Vec2,
    b: Vec2,
    handle: Handle,
}

fn segs_of(ents: &[&Arc<Entity>]) -> Vec<Seg> {
    let mut out = Vec::new();
    for e in ents {
        for sg in super::perceive::geometry::segments(&[*e]) {
            if out.len() >= MAX_SEGS {
                return out;
            }
            out.push(Seg { a: sg.a, b: sg.b, handle: e.handle });
        }
    }
    out
}

/// A run found: its first and last tread lines' centres, the treads between, width, source.
#[derive(Clone, Debug)]
struct Found {
    from: Vec2,
    to: Vec2,
    treads: u32,
    width: f64,
    objects: Vec<Handle>,
    /// The direction is from an UP/DN note.
    known: bool,
    stringers: bool,
}

/// Rows of parallel, equal, evenly spaced lines: stair runs (bottom → top as drawn; the notes
/// decide later).
fn runs(segs: &[Seg], k: f64) -> Vec<Found> {
    let (len_lo, len_hi) = (24.0 * k, 144.0 * k);
    let (gap_lo, gap_hi) = (8.0 * k, 14.0 * k);
    let mut groups: std::collections::BTreeMap<i64, Vec<(f64, usize)>> = std::collections::BTreeMap::new();
    for (i, s) in segs.iter().enumerate() {
        let d = s.b - s.a;
        let len = d.len();
        if !(len >= len_lo && len <= len_hi) {
            continue;
        }
        let ang = d.y.atan2(d.x).rem_euclid(std::f64::consts::PI);
        let key = (ang.to_degrees() * 2.0).round() as i64 % 360;
        let n = Vec2::new(-ang.sin(), ang.cos());
        groups.entry(key).or_default().push((n.dot(s.a), i));
    }
    let mut out = Vec::new();
    for (key, mut g) in groups {
        g.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ang = (f64::from(key as i32) / 2.0).to_radians();
        let (u, n) = (Vec2::new(ang.cos(), ang.sin()), Vec2::new(-ang.sin(), ang.cos()));
        let span = |i: usize| segs.get(i).map(|s| (u.dot(s.a).min(u.dot(s.b)), u.dot(s.a).max(u.dot(s.b))));
        let mut used = vec![false; g.len()];
        for x in 0..g.len() {
            if used.get(x).copied().unwrap_or(true) {
                continue;
            }
            let Some(&(o0, i0)) = g.get(x) else { continue };
            let Some((a0, a1)) = span(i0) else { continue };
            // Extend greedily: the next line a tread on, the same length and overlapping.
            let mut chain = vec![(o0, i0, x)];
            let mut step: Option<f64> = None;
            let mut y = x + 1;
            while let Some(&(oy, iy)) = g.get(y) {
                let last = chain.last().map_or(o0, |c| c.0);
                let d = oy - last;
                let want = step.unwrap_or(d);
                if d > gap_hi * 1.1 || (step.is_some() && d > want * 1.08) {
                    break;
                }
                let fits = d >= gap_lo
                    && (d - want).abs() <= want * 0.08
                    && span(iy).is_some_and(|(b0, b1)| {
                        let overlap = a1.min(b1) - a0.max(b0);
                        overlap >= 0.85 * (a1 - a0).min(b1 - b0) && ((b1 - b0) - (a1 - a0)).abs() <= 0.15 * (a1 - a0)
                    });
                if fits {
                    step = Some(want);
                    chain.push((oy, iy, y));
                }
                y += 1;
            }
            if chain.len() < 4 {
                continue;
            }
            for c in &chain {
                if let Some(f) = used.get_mut(c.2) {
                    *f = true;
                }
            }
            let (Some(first), Some(last)) = (chain.first(), chain.last()) else { continue };
            let lo = chain.iter().filter_map(|c| span(c.1)).map(|s| s.0).fold(f64::NEG_INFINITY, f64::max);
            let hi = chain.iter().filter_map(|c| span(c.1)).map(|s| s.1).fold(f64::INFINITY, f64::min);
            let mid = (lo + hi) / 2.0;
            let (from, to) = (u * mid + n * first.0, u * mid + n * last.0);
            let width = hi - lo;
            // Stringers: lines along the run at either side.
            let along: Vec<&Seg> = segs
                .iter()
                .filter(|s| {
                    let d = s.b - s.a;
                    d.len() > 1e-9 && (d.dot(n) / d.len()).abs() > 0.99 && ((u.dot(s.a) - lo).abs() < 3.0 * k || (u.dot(s.a) - hi).abs() < 3.0 * k)
                })
                .collect();
            let stringers = along.len() >= 2;
            let mut objects: Vec<Handle> = chain.iter().filter_map(|c| segs.get(c.1)).map(|s| s.handle).collect();
            objects.extend(along.iter().map(|s| s.handle));
            objects.sort_by_key(|h| h.0);
            objects.dedup();
            out.push(Found { from, to, treads: u32::try_from(chain.len() - 1).unwrap_or(u32::MAX), width, objects, known: false, stringers });
            if out.len() >= MAX_STAIRS {
                return out;
            }
        }
    }
    out
}

/// UP / DN notes (and slope notes for ramps).
fn up_or_down(t: &str) -> Option<bool> {
    let up = t.trim().to_ascii_uppercase();
    let first = up.split(|c: char| !c.is_ascii_alphabetic()).find(|w| !w.is_empty())?;
    match first {
        "UP" => Some(true),
        "DN" | "DOWN" => Some(false),
        _ => None,
    }
}

fn ramp_note(t: &str) -> bool {
    let up = t.to_ascii_uppercase();
    up.contains("RAMP") || up.contains("1:12") || up.contains("1:16") || up.contains("1:20") || up.contains("SLOPE") || up.contains('%')
}

/// The stairs of the plan, as spec entries with their measures and confidence.
fn detect(s: &Session, sp: &BuildingSpec) -> Result<Vec<(Stair, Value)>> {
    let plan = super::verify::plan_session(s)?;
    let d: &Drawing = plan.doc()?;
    let k = units_per_inch(d);
    let all = plan_objects(d);
    let stair_layer = |l: &str| {
        let up = l.to_ascii_uppercase();
        up.contains("STAIR") || up.contains("STRS") || up.contains("RAMP")
    };
    let has_roles = sp.layers.iter().any(|r| r.role == Role::Stair);
    let ents: Vec<&Arc<Entity>> = all
        .iter()
        .copied()
        .filter(|e| if has_roles { sp.role(&e.common.layer).is_some_and(|r| r.role == Role::Stair) } else { stair_layer(&e.common.layer) })
        .collect();
    let notes: Vec<(String, Vec2, Handle)> = texts(&all).into_iter().map(|(h, t, at, _)| (t, at, h)).collect();
    let mut found = runs(&segs_of(&ents), k);
    // Which way each run climbs: an UP note at its foot, or DN at its head.
    for f in &mut found {
        let len = f.from.dist(f.to).max(1e-9);
        let u = (f.to - f.from) * (1.0 / len);
        let n = Vec2::new(-u.y, u.x);
        let near = |p: Vec2| {
            let rel = p - f.from;
            let along = rel.dot(u);
            along >= -f.width && along <= len + f.width && rel.dot(n).abs() <= f.width
        };
        if let Some((t, at, h)) = notes.iter().find(|(t, at, _)| up_or_down(t).is_some() && near(*at)) {
            let up = up_or_down(t).unwrap_or(true);
            let foot_is_from = at.dist(f.from) <= at.dist(f.to);
            if up != foot_is_from {
                std::mem::swap(&mut f.from, &mut f.to);
            }
            f.known = true;
            f.objects.push(*h);
        }
    }
    // Runs that meet across a landing are one stair: the head of one near the foot of the next.
    let mut stairs: Vec<Vec<Found>> = Vec::new();
    let mut left: Vec<Found> = found;
    while let Some(first) = (!left.is_empty()).then(|| left.remove(0)) {
        let mut chain = vec![first];
        while let Some(last) = chain.last() {
            let reach = 1.6 * last.width;
            let next = left.iter().position(|r| r.from.dist(last.to) <= reach || (!r.known && r.to.dist(last.to) <= reach));
            let Some(i) = next else { break };
            let mut r = left.remove(i);
            if r.from.dist(last.to) > reach {
                std::mem::swap(&mut r.from, &mut r.to);
            }
            chain.push(r);
        }
        // And runs that lead into its foot.
        while let Some(head) = chain.first() {
            let reach = 1.6 * head.width;
            let Some(i) = left.iter().position(|r| r.to.dist(head.from) <= reach || (!r.known && r.from.dist(head.from) <= reach)) else { break };
            let mut r = left.remove(i);
            if r.to.dist(head.from) > reach {
                std::mem::swap(&mut r.from, &mut r.to);
            }
            chain.insert(0, r);
        }
        stairs.push(chain);
    }
    let upi = 1.0 / k;
    // Drawing coordinates to a millionth (the arithmetic leaves 22.000000000000007).
    let r6 = |p: Vec2| [(p.x * 1e6).round() / 1e6, (p.y * 1e6).round() / 1e6];
    let mut out = Vec::new();
    for (i, chain) in stairs.iter().enumerate() {
        let Some(first) = chain.first() else { continue };
        let Some(level) = level_at(&sp.levels, first.from.x, first.from.y) else { continue };
        let width = chain.iter().map(|r| r.width).fold(f64::INFINITY, f64::min);
        let known = chain.iter().any(|r| r.known);
        let stringers = chain.iter().all(|r| r.stringers);
        let st = Stair {
            name: format!("S{}", i + 1),
            ramp: false,
            level: level.name.clone(),
            to_level: None,
            runs: chain.iter().map(|r| StairRun { from: r6(r.from), to: r6(r.to), treads: r.treads }).collect(),
            width_in: (width * upi * 8.0).round() / 8.0,
            confirmed: false,
            objects: chain.iter().flat_map(|r| r.objects.iter().map(|h| h.hex())).collect(),
        };
        let confidence = 0.5 + if known { 0.25 } else { 0.0 } + if stringers { 0.15 } else { 0.0 } + if has_roles { 0.1 } else { 0.0 };
        out.push((st, json!({ "direction": if known { "text" } else { "assumed" }, "confidence": confidence })));
    }
    // Ramps: a closed outline holding a ramp note, its long side the run.
    let mut r = 0;
    for (_, at, h) in notes.iter().filter(|(t, _, _)| ramp_note(t)) {
        let outline = all
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::LwPolyline(p) if p.closed && p.vertices.len() == 4 => {
                    let q: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                    cadaddy_geom::point_in_polygon(&q, *at).then_some((q, e.handle))
                }
                _ => None,
            })
            .min_by(|a, b| cadaddy_geom::shoelace(&a.0).abs().total_cmp(&cadaddy_geom::shoelace(&b.0).abs()));
        let Some((q, oh)) = outline else { continue };
        if out.iter().any(|(st, _)| st.ramp && st.objects.contains(&oh.hex())) {
            continue;
        }
        let [p0, p1, p2, p3] = [q[0], q[1], q[2], q[3]];
        let (long, short) = if p0.dist(p1) >= p1.dist(p2) { ((p0, p1, p3), p1.dist(p2)) } else { ((p1, p2, p0), p0.dist(p1)) };
        let half = (long.2 - long.0) * 0.5;
        let (mut a, mut b) = (long.0 + half, long.1 + half);
        // Which way: an UP/DN note inside, else the ramp note's arrow end is unknown: assumed.
        let note = notes.iter().find(|(t2, at2, _)| up_or_down(t2).is_some() && cadaddy_geom::point_in_polygon(&q, *at2));
        let known = note.is_some();
        if let Some((t2, at2, _)) = note {
            let up = up_or_down(t2).unwrap_or(true);
            if up != (at2.dist(a) <= at2.dist(b)) {
                std::mem::swap(&mut a, &mut b);
            }
        }
        let Some(level) = level_at(&sp.levels, a.x, a.y) else { continue };
        r += 1;
        let st = Stair {
            name: format!("R{r}"),
            ramp: true,
            level: level.name.clone(),
            to_level: None,
            runs: vec![StairRun { from: r6(a), to: r6(b), treads: 0 }],
            width_in: (short * upi * 8.0).round() / 8.0,
            confirmed: false,
            objects: vec![oh.hex(), h.hex()],
        };
        out.push((st, json!({ "direction": if known { "text" } else { "assumed" }, "confidence": if known { 0.8 } else { 0.6 } })));
        if out.len() >= MAX_STAIRS {
            break;
        }
    }
    Ok(out)
}

/// A stair as `stairs.detect` and `stairs.list` report it.
fn stair_json(sp: &BuildingSpec, st: &Stair) -> Value {
    let mut v = serde_json::to_value(st).unwrap_or(Value::Null);
    if let Some((risers, riser, tread, len)) = sp.stair_measures(st) {
        let r = |x: f64| if x.is_finite() { (x * 1000.0).round() / 1000.0 } else { 0.0 };
        v["risers"] = json!(risers);
        v["riser_in"] = json!(r(riser));
        v["tread_in"] = json!(r(tread));
        v["length_ft"] = json!(r(len));
    }
    v
}

fn run_detect(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "stairs.detect")?;
    let found = detect(s, &sp)?;
    let list: Vec<Value> = found
        .iter()
        .map(|(st, extra)| {
            let mut v = stair_json(&sp, st);
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            v
        })
        .collect();
    let store = bool_or(p, "store", false);
    if store {
        // Confirmed stairs stay; unconfirmed ones are replaced by what was found (skipping any
        // that a confirmed stair already covers).
        sp.stairs.retain(|x| x.confirmed);
        for (mut st, _) in found {
            let covered = sp.stairs.iter().any(|c| {
                c.runs.first().zip(st.runs.first()).is_some_and(|(a, b)| {
                    let d = |x: [f64; 2], y: [f64; 2]| ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2)).sqrt();
                    d(a.from, b.from).min(d(a.from, b.to)) < 1e-6 * (1.0 + d(a.from, a.to))
                })
            });
            if covered {
                continue;
            }
            let mut n = 1;
            let stem = if st.ramp { "R" } else { "S" };
            while sp.stairs.iter().any(|x| x.name.eq_ignore_ascii_case(&st.name)) {
                n += 1;
                st.name = format!("{stem}{n}");
            }
            sp.stairs.push(st);
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "stairs": list, "stored": store }))
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "stairs.list")?;
    let issues = sp.check();
    let list: Vec<Value> = sp
        .stairs
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let mut v = stair_json(&sp, st);
            let at = format!("stairs[{i}]");
            v["warnings"] =
                json!(issues.iter().filter(|x| x.path == at || x.path.starts_with(&format!("{at}."))).map(|x| x.message.clone()).collect::<Vec<_>>());
            if let Some((_, top)) = sp.stair_heights(st) {
                v["to"] = json!(sp.levels.iter().find(|l| (l.elevation_ft - top).abs() < 1e-6).map_or(Value::Null, |l| json!(l.name)));
            }
            v
        })
        .collect();
    Ok(json!({ "stairs": list }))
}

fn run_confirm(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "stairs.confirm")?;
    let all = bool_or(p, "all", false);
    let names: Vec<String> =
        p.get("names").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if !all && names.is_empty() {
        return Err(bad("stairs.confirm", "give `names` or `all: true`"));
    }
    if let Some(n) = names.iter().find(|n| !sp.stairs.iter().any(|x| x.name.eq_ignore_ascii_case(n))) {
        return Err(bad("stairs.confirm", format!("there is no stair named {n}")));
    }
    let on = bool_or(p, "confirmed", true);
    let mut done = Vec::new();
    for st in &mut sp.stairs {
        if all || names.iter().any(|n| n.eq_ignore_ascii_case(&st.name)) {
            st.confirmed = on;
            done.push(st.name.clone());
        }
    }
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(json!({ "confirmed": done }))
}

// ---------------- building them ----------------

/// A beam along a 3D segment, `t` square (a handrail).
fn beam(m: &mut Mesh, a: Vec3, b: Vec3, t: f64) {
    let d = b - a;
    let flat = Vec2::new(d.x, d.y);
    if flat.len() <= 1e-9 {
        return;
    }
    let side = Vec2::new(-flat.y, flat.x) * (t / 2.0 / flat.len());
    let up = Vec3::new(0.0, 0.0, t / 2.0);
    let s3 = Vec3::new(side.x, side.y, 0.0);
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    for p in [a, b] {
        for (i, j) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            m.vertices.push(p + s3 * i + up * j);
        }
    }
    let f = |v: [u32; 4]| MeshFace { v: v.iter().map(|i| base + i).collect(), hidden: 0 };
    m.faces.extend([f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])]);
}

/// A ramp's solid: its sloped top, sides and ends.
fn wedge(m: &mut Mesh, w: &cadaddy_walk3d::stairs::Wedge, upf: f64) {
    let len = w.a.dist(w.b);
    if len <= 1e-9 {
        return;
    }
    let n = Vec2::new(-(w.b - w.a).y, (w.b - w.a).x) * (w.width / 2.0 / len);
    let p = |q: Vec2, z: f64| Vec3::new(q.x * upf, q.y * upf, z * upf);
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    m.vertices.extend([
        p(w.a - n, w.z0),
        p(w.b - n, w.z0),
        p(w.b + n, w.z0),
        p(w.a + n, w.z0),
        p(w.a - n, w.za),
        p(w.b - n, w.zb),
        p(w.b + n, w.zb),
        p(w.a + n, w.za),
    ]);
    let f = |v: [u32; 4]| MeshFace { v: v.iter().map(|i| base + i).collect(), hidden: 0 };
    m.faces.extend([f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])]);
}

/// Build the spec's stairs into model space (sheet units): one mesh per stair or ramp, one
/// for its handrails. The counts: (stairs, ramps, railings).
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> (usize, usize, usize) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let (mut stairs, mut ramps, mut rails) = (0, 0, 0);
    for layer in ["3D-Stairs", "3D-Railings"] {
        if d.layer(layer).is_none() {
            let color = if layer == "3D-Stairs" { 9 } else { 8 };
            d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(color), ..cadaddy_doc::Layer::default() });
        }
    }
    for st in sp.stairs.iter().take(MAX_STAIRS) {
        let Some(rule) = sp.stair_rule(st) else { continue };
        let g = rule.geometry(Vec2::new(0.0, 0.0), upf);
        let mut m = super::wallgen::empty_mesh();
        for b in &g.blocks {
            super::wallgen::add_box(&mut m, b.a * upf, b.b * upf, b.width * upf, b.z0 * upf, b.z1 * upf);
        }
        for w in &g.wedges {
            wedge(&mut m, w, upf);
        }
        let mut r = super::wallgen::empty_mesh();
        for (a, b) in &g.rails {
            let v = |x: [f64; 3]| Vec3::new(x[0] * upf, x[1] * upf, x[2] * upf);
            beam(&mut r, v(*a), v(*b), 0.15 * upf);
        }
        let source = st.objects.first().and_then(|h| u64::from_str_radix(h, 16).ok()).map_or(Handle(0), Handle);
        let tag = |part: &str| {
            let t = |code: i32, v: String| RawTag { code, value: v };
            vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, part.into()),
                t(1000, st.level.chars().take(255).collect()),
                t(1000, source.hex()),
                t(1000, format!("stair:{}", st.name)),
            ]
        };
        let part = if st.ramp { "ramp" } else { "stair" };
        if !m.faces.is_empty()
            && d.add(&Space::Model, Common { layer: "3D-Stairs".into(), xdata: tag(part), ..Common::default() }, EntityKind::Mesh(m)).is_ok()
        {
            if st.ramp {
                ramps += 1;
            } else {
                stairs += 1;
            }
        }
        if !r.faces.is_empty()
            && d.add(&Space::Model, Common { layer: "3D-Railings".into(), xdata: tag("railing"), ..Common::default() }, EntityKind::Mesh(r)).is_ok()
        {
            rails += 1;
        }
    }
    (stairs, ramps, rails)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes() {
        assert_eq!(up_or_down("UP"), Some(true));
        assert_eq!(up_or_down("up 18R"), Some(true));
        assert_eq!(up_or_down("DN"), Some(false));
        assert_eq!(up_or_down("DOWN TO LOBBY"), Some(false));
        assert_eq!(up_or_down("UPPER"), None);
        assert!(ramp_note("RAMP 1:12") && ramp_note("8.33% SLOPE") && !ramp_note("STAIR"));
    }
}
