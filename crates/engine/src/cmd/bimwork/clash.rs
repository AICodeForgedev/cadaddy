//! `bim.clash`, `bim.clash.status`, `bim.clash.fix`: the model's elements of one trade against
//! another's, mesh against mesh.
//!
//! Elements whose boxes overlap by more than the tolerance are tested face by face: a hard clash
//! is two faces crossing (or one element wholly inside the other); with a `clearance`, an element
//! whose box, grown by the clearance, reaches the other's faces is a clearance clash. Touching
//! faces are not clashes. Parts of one MEP system don't clash with each other (they join), nor do
//! hosted elements with their hosts (devices, doors and windows in walls).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::{Mat4, Vec3};
use serde_json::{Value, json};

use super::{El, elements, frame, picks, r3, selector, set_state, state, units_per_foot, viewpoint};
use crate::cmd::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

/// The model's elements that take up room: not its rooms (spaces, Q5291), which every element
/// inside them would clash with.
fn solid_elements(d: &Drawing) -> Vec<El> {
    elements(d).into_iter().filter(|e| e.category != "room").collect()
}

const CMD: &str = "bim.clash";
const CMD_STATUS: &str = "bim.clash.status";
const CMD_FIX: &str = "bim.clash.fix";
/// Clashes reported by one run, element pairs tested, and faces an element is tested by.
const MAX_CLASHES: usize = 20_000;
const MAX_PAIRS: usize = 2_000_000;
const MAX_TRIS: usize = 20_000;
/// Areas clashes are grouped in, feet a side.
const AREA_FT: f64 = 20.0;
pub const STATUSES: [&str; 3] = ["new", "active", "resolved"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Clash Check", run)
            .alias(&["clash"])
            .params("{a?: trade|category|system (or a list; e.g. electrical, duct, \"M-*\"; default every trade against the others), b?: same (default everything else), tolerance?: inches (overlaps up to it ignored, default 0.5), clearance?: inches (also report elements closer than it), group_by?: pair|area|level (default pair), hosted?: bool (devices/doors/windows in walls too; default false)} → {clashes: [{id, kind: hard|clearance, status: new|active|resolved, a, b, level, area, at, depth_in, viewpoint: {eye, target}}], groups: [{key, count, ids}], total, new, active, resolved: [ids gone since the last run], tested, run}")
            .noundo(),
        CommandSpec::new(CMD_STATUS, "Clash Status", run_status).params("{id, status: new|active|resolved, note?} → {id, status}").noundo(),
        CommandSpec::new(CMD_FIX, "Fix Clash", run_fix)
            .params("{id, strategy?: reroute|raise|offset (default reroute), clearance_in? (1), max_move_in? (24), approve?: bool (apply the move and check again)} → {id, mover, stays, why, strategy, move: [dx,dy,dz], move_in, applied, check?: {clear, new_clashes}}"),
    ]
}

// ---------------- geometry ----------------

type Tri = [Vec3; 3];

/// An element's faces as triangles (its box's when it has too many), and whether they close a
/// volume (every edge shared by two faces): only a closed mesh has an inside.
fn triangles(d: &Drawing, h: Handle, lo: Vec3, hi: Vec3) -> (Vec<Tri>, bool) {
    let mut out = Vec::new();
    let mut closed = false;
    if let Some(e) = d.entity(h)
        && let EntityKind::Mesh(m) = &e.kind
    {
        let mut edges: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for f in m.faces.iter().take(MAX_TRIS) {
            for (k, a) in f.v.iter().enumerate() {
                let b = f.v.get((k + 1) % f.v.len()).copied().unwrap_or(*a);
                if *a != b {
                    *edges.entry((*a.min(&b), *a.max(&b))).or_insert(0) += 1;
                }
            }
        }
        closed = !edges.is_empty() && edges.values().all(|n| *n == 2);
        'faces: for f in &m.faces {
            let vs: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
            let Some(&a) = vs.first() else { continue };
            for w in vs.get(1..).unwrap_or_default().windows(2) {
                if let [b, c] = w {
                    if out.len() >= MAX_TRIS {
                        out.clear();
                        break 'faces;
                    }
                    out.push([a, *b, *c]);
                }
            }
        }
    }
    if out.is_empty() { (box_tris(lo, hi), true) } else { (out, closed) }
}

fn box_tris(lo: Vec3, hi: Vec3) -> Vec<Tri> {
    let c = |i: u8| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
    let quads: [[u8; 4]; 6] = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
    quads.iter().flat_map(|q| [[c(q[0]), c(q[1]), c(q[2])], [c(q[0]), c(q[2]), c(q[3])]]).collect()
}

fn tri_box(t: &Tri) -> (Vec3, Vec3) {
    let [a, b, c] = *t;
    (
        Vec3::new(a.x.min(b.x).min(c.x), a.y.min(b.y).min(c.y), a.z.min(b.z).min(c.z)),
        Vec3::new(a.x.max(b.x).max(c.x), a.y.max(b.y).max(c.y), a.z.max(b.z).max(c.z)),
    )
}

fn boxes_meet(a: (Vec3, Vec3), b: (Vec3, Vec3), pad: f64) -> bool {
    a.0.x <= b.1.x + pad && b.0.x <= a.1.x + pad && a.0.y <= b.1.y + pad && b.0.y <= a.1.y + pad && a.0.z <= b.1.z + pad && b.0.z <= a.1.z + pad
}

/// Does the segment `p`–`q` pass through the triangle's inside (Möller–Trumbore)?
fn seg_tri(p: Vec3, q: Vec3, t: &Tri) -> bool {
    let [a, b, c] = *t;
    let dir = q - p;
    let (e1, e2) = (b - a, c - a);
    let h = dir.cross(e2);
    let det = e1.dot(h);
    let scale = e1.len() * e2.len() * dir.len();
    if !scale.is_finite() || scale <= 0.0 || det.abs() <= 1e-12 * scale {
        return false;
    }
    let f = 1.0 / det;
    let s = p - a;
    let u = f * s.dot(h);
    if !(1e-9..=1.0 - 1e-9).contains(&u) {
        return false;
    }
    let qv = s.cross(e1);
    let v = f * dir.dot(qv);
    if v < 1e-9 || u + v > 1.0 - 1e-9 {
        return false;
    }
    let k = f * e2.dot(qv);
    k > 1e-9 && k < 1.0 - 1e-9
}

/// Do two triangles cross (an edge of one through the other's inside)? Coplanar or edge-to-edge
/// contact is touching, not crossing.
fn tri_tri(a: &Tri, b: &Tri) -> bool {
    let edges = |t: &Tri| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])];
    edges(a).iter().any(|(p, q)| seg_tri(*p, *q, b)) || edges(b).iter().any(|(p, q)| seg_tri(*p, *q, a))
}

/// Is `p` inside the closed mesh `tris` (a ray's crossings, odd)?
fn inside(p: Vec3, tris: &[Tri], far: f64) -> bool {
    // A slightly skewed ray, so it doesn't run along the edges of axis-aligned faces.
    let q = p + Vec3::new(far, far * 0.000_731, far * 0.000_377);
    tris.iter().filter(|t| seg_tri(p, q, t)).count() % 2 == 1
}

/// Does the triangle reach into the box?
fn tri_in_box(t: &Tri, lo: Vec3, hi: Vec3) -> bool {
    let inb = |p: Vec3| p.x > lo.x && p.x < hi.x && p.y > lo.y && p.y < hi.y && p.z > lo.z && p.z < hi.z;
    if t.iter().any(|p| inb(*p)) {
        return true;
    }
    let bt = box_tris(lo, hi);
    let edges = [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])];
    if edges.iter().any(|(p, q)| bt.iter().any(|f| seg_tri(*p, *q, f))) {
        return true;
    }
    let c = |i: u8| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
    let box_edges: [(u8, u8); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    box_edges.iter().any(|(i, j)| seg_tri(c(*i), c(*j), t))
}

/// How two elements meet: `Some(true)` hard, `Some(false)` within the clearance, `None` apart.
fn meet(d: &Drawing, a: &El, b: &El, tol: f64, clearance: f64, cache: &mut BTreeMap<Handle, (Vec<Tri>, bool)>) -> Option<bool> {
    let over = Vec3::new(a.hi.x.min(b.hi.x) - a.lo.x.max(b.lo.x), a.hi.y.min(b.hi.y) - a.lo.y.max(b.lo.y), a.hi.z.min(b.hi.z) - a.lo.z.max(b.lo.z));
    let hard_possible = over.x > tol && over.y > tol && over.z > tol;
    let soft_possible = clearance > 0.0 && over.x > -clearance && over.y > -clearance && over.z > -clearance;
    if !hard_possible && !soft_possible {
        return None;
    }
    let (ta, a_closed) = cache.entry(a.handle).or_insert_with(|| triangles(d, a.handle, a.lo, a.hi)).clone();
    let (tb, b_closed) = cache.entry(b.handle).or_insert_with(|| triangles(d, b.handle, b.lo, b.hi)).clone();
    if hard_possible {
        // Only the faces inside the overlap can cross.
        let zone = (
            Vec3::new(a.lo.x.max(b.lo.x), a.lo.y.max(b.lo.y), a.lo.z.max(b.lo.z)),
            Vec3::new(a.hi.x.min(b.hi.x), a.hi.y.min(b.hi.y), a.hi.z.min(b.hi.z)),
        );
        let fa: Vec<(&Tri, (Vec3, Vec3))> = ta.iter().map(|t| (t, tri_box(t))).filter(|(_, bx)| boxes_meet(*bx, zone, 1e-9)).collect();
        let fb: Vec<(&Tri, (Vec3, Vec3))> = tb.iter().map(|t| (t, tri_box(t))).filter(|(_, bx)| boxes_meet(*bx, zone, 1e-9)).collect();
        let mut tests = 0usize;
        for (x, bx) in &fa {
            for (y, by) in &fb {
                tests += 1;
                if tests > 4_000_000 {
                    break;
                }
                if boxes_meet(*bx, *by, 1e-9) && tri_tri(x, y) {
                    return Some(true);
                }
            }
        }
        // One inside the other.
        let far = (a.hi - a.lo).len() + (b.hi - b.lo).len() + 1.0;
        let within =
            |x: &El, y: &El| x.lo.x >= y.lo.x && x.lo.y >= y.lo.y && x.lo.z >= y.lo.z && x.hi.x <= y.hi.x && x.hi.y <= y.hi.y && x.hi.z <= y.hi.z;
        if (b_closed && within(a, b) && inside(a.centre(), &tb, far)) || (a_closed && within(b, a) && inside(b.centre(), &ta, far)) {
            return Some(true);
        }
    }
    if clearance > 0.0 {
        // The smaller element's box, grown by the clearance, against the other's faces.
        let vol = |e: &El| (e.hi.x - e.lo.x) * (e.hi.y - e.lo.y) * (e.hi.z - e.lo.z);
        let (small, other) = if vol(a) <= vol(b) { (a, &tb) } else { (b, &ta) };
        let pad = Vec3::new(clearance, clearance, clearance);
        let (lo, hi) = (small.lo - pad, small.hi + pad);
        if other.iter().take(MAX_TRIS).any(|t| boxes_meet(tri_box(t), (lo, hi), 0.0) && tri_in_box(t, lo, hi)) {
            return Some(false);
        }
    }
    None
}

/// Elements that join or are hosted: never clashes.
fn exempt(a: &El, b: &El, hosted: bool) -> bool {
    let mep = |e: &El| {
        matches!(
            e.category.as_str(),
            "duct" | "pipe" | "conduit" | "tray" | "cable" | "fitting" | "terminal" | "equipment" | "box" | "support" | "wire"
        )
    };
    if mep(a) && mep(b) && !a.system.is_empty() && a.system == b.system {
        return true;
    }
    if hosted {
        return false;
    }
    // The building's own parts meet at their joints (walls on floors, doors in walls).
    if a.trade == "architecture" && b.trade == "architecture" {
        return true;
    }
    let host = |e: &El| e.trade == "architecture" && matches!(e.category.as_str(), "wall" | "ceiling" | "floor");
    let guest = |e: &El| matches!(e.category.as_str(), "device" | "terminal") || e.in_wall;
    (host(a) && guest(b)) || (host(b) && guest(a))
}

/// A clash's id: its two elements, in order.
fn clash_id(a: Handle, b: Handle) -> String {
    let (x, y) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    format!("C-{}-{}", x.hex(), y.hex())
}

struct Found {
    id: String,
    hard: bool,
    a: El,
    b: El,
    at: Vec3,
    depth: f64,
}

/// Every clash between the elements `a` picks and those `b` picks.
fn detect(d: &Drawing, els: &[El], sa: &[String], sb: Option<&[String]>, tol: f64, clearance: f64, hosted: bool) -> (Vec<Found>, usize) {
    // Everything with nothing named against it: each trade against the others.
    let cross = sb.is_none() && sa.iter().any(|x| matches!(x.trim(), "*" | "all" | "any"));
    let in_a: Vec<bool> = els.iter().map(|e| picks(sa, e)).collect();
    let in_b: Vec<bool> = els.iter().zip(&in_a).map(|(e, ina)| if cross { true } else { sb.map_or(!ina, |sb| picks(sb, e)) }).collect();
    // Sweep along x: each element against those whose x-range it meets.
    let pad = clearance.max(0.0);
    let mut order: Vec<usize> = (0..els.len()).filter(|i| in_a.get(*i) == Some(&true) || in_b.get(*i) == Some(&true)).collect();
    order.sort_by(|i, j| {
        let (x, y) = (els.get(*i).map_or(0.0, |e| e.lo.x), els.get(*j).map_or(0.0, |e| e.lo.x));
        x.total_cmp(&y)
    });
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut run_pairs: BTreeMap<String, usize> = BTreeMap::new();
    let mut cache: BTreeMap<Handle, (Vec<Tri>, bool)> = BTreeMap::new();
    let mut tested = 0usize;
    let mut active: Vec<usize> = Vec::new();
    for &i in &order {
        let Some(e) = els.get(i) else { continue };
        active.retain(|j| els.get(*j).is_some_and(|o| o.hi.x + pad >= e.lo.x));
        for &j in &active {
            let Some(o) = els.get(j) else { continue };
            let ab = in_a.get(i) == Some(&true) && in_b.get(j) == Some(&true);
            let ba = in_b.get(i) == Some(&true) && in_a.get(j) == Some(&true);
            if !(ab || ba) || (cross && e.trade == o.trade) || !boxes_meet((e.lo, e.hi), (o.lo, o.hi), pad) || exempt(e, o, hosted) {
                continue;
            }
            tested += 1;
            if tested > MAX_PAIRS || out.len() >= MAX_CLASHES {
                break;
            }
            let id = clash_id(e.handle, o.handle);
            if seen.contains(&id) {
                continue;
            }
            // A run's pieces against one element: one clash, named by a stick of the run where
            // one meets it (else the piece that does).
            let side = |x: &El| if x.run.is_empty() { x.handle.hex() } else { format!("run:{}", x.run) };
            let pair = if side(e) <= side(o) { format!("{}|{}", side(e), side(o)) } else { format!("{}|{}", side(o), side(e)) };
            let by_run = !e.run.is_empty() || !o.run.is_empty();
            let had = by_run.then(|| run_pairs.get(&pair).copied()).flatten();
            let stick = |x: &El| !x.run.is_empty() && x.category == "conduit";
            if let Some(k) = had
                && (out.get(k).is_some_and(|f: &Found| stick(&f.a) || stick(&f.b)) || !(stick(e) || stick(o)))
            {
                continue;
            }
            // The `a` side first.
            let (x, y) = if ab { (e, o) } else { (o, e) };
            if let Some(hard) = meet(d, x, y, tol, clearance, &mut cache) {
                let lo = Vec3::new(x.lo.x.max(y.lo.x), x.lo.y.max(y.lo.y), x.lo.z.max(y.lo.z));
                let hi = Vec3::new(x.hi.x.min(y.hi.x), x.hi.y.min(y.hi.y), x.hi.z.min(y.hi.z));
                let depth = (hi.x - lo.x).min(hi.y - lo.y).min(hi.z - lo.z);
                seen.insert(id.clone());
                let f = Found { id, hard, a: x.clone(), b: y.clone(), at: (lo + hi) * 0.5, depth };
                match had.and_then(|k| out.get_mut(k)) {
                    Some(old) => *old = f,
                    None => {
                        if by_run {
                            run_pairs.insert(pair, out.len());
                        }
                        out.push(f);
                    }
                }
            }
        }
        active.push(i);
    }
    (out, tested)
}

/// `a` and `b`; no `a`: every trade against the others.
fn selectors(p: &Value) -> Result<(Vec<String>, Option<Vec<String>>)> {
    let what =
        "a trade (electrical, mechanical, plumbing, architecture, structure, fire), a category (duct, conduit, wall…) or a system, or a list of them";
    let a = match p.get("a").filter(|v| !v.is_null()) {
        None => vec!["*".to_string()],
        Some(_) => selector(p, "a").ok_or_else(|| bad(CMD, format!("`a` is {what}")))?,
    };
    let b = match p.get("b").filter(|v| !v.is_null()) {
        None => None,
        Some(_) => Some(selector(p, "b").ok_or_else(|| bad(CMD, format!("`b` is {what}")))?),
    };
    Ok((a, b))
}

fn run_key(a: &[String], b: Option<&[String]>) -> String {
    let norm = |v: &[String]| {
        let mut v: Vec<String> = v.iter().map(|x| x.trim().to_ascii_lowercase()).collect();
        v.sort();
        v.join(",")
    };
    format!("{}|{}", norm(a), b.map_or_else(|| "*".to_string(), norm))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (sa, sb) = selectors(p)?;
    let upf = units_per_foot(s);
    let inch = upf / 12.0;
    let tol = p.get("tolerance").and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(0.5).min(1_000.0) * inch;
    let clearance = p.get("clearance").and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(0.0).min(1_000.0) * inch;
    let group_by = str_param(p, "group_by").unwrap_or("pair").to_ascii_lowercase();
    if !matches!(group_by.as_str(), "pair" | "area" | "level") {
        return Err(bad(CMD, "`group_by` is pair, area or level"));
    }
    let hosted = bool_or(p, "hosted", false);
    let d = s.doc()?;
    let els = solid_elements(d);
    if els.is_empty() {
        return Err(crate::EngineError::Other(
            "bim.clash: the drawing has no built model to check: build it first (model.build or bim.build)".into(),
        ));
    }
    let (found, tested) = detect(d, &els, &sa, sb.as_deref(), tol, clearance, hosted);
    // Against the last run of the same check.
    let key = run_key(&sa, sb.as_deref());
    let st = state(d);
    let prev = st["clash"]["runs"][&key].clone();
    let prev_items = prev["items"].as_object().cloned().unwrap_or_default();
    let run_no = prev["run"].as_u64().unwrap_or(0) + 1;
    let mut items = serde_json::Map::new();
    let mut clashes = Vec::new();
    let (mut n_new, mut n_active) = (0usize, 0usize);
    for f in &found {
        let before = prev_items.get(&f.id);
        let status = match before.and_then(|b| b["status"].as_str()) {
            None => "new",
            // Still there: active (a clash marked resolved that is back is active again).
            Some(_) => "active",
        };
        if status == "new" {
            n_new += 1;
        } else {
            n_active += 1;
        }
        let first = before.and_then(|b| b["first_run"].as_u64()).unwrap_or(run_no);
        let note = before.and_then(|b| b["note"].as_str()).unwrap_or_default();
        let cell = |v: f64| (v / (AREA_FT * upf)).floor() as i64;
        let area = format!("{}:{}", cell(f.at.x), cell(f.at.y));
        let level = if f.a.level.is_empty() { f.b.level.clone() } else { f.a.level.clone() };
        let size = ((f.a.hi - f.a.lo).len().min((f.b.hi - f.b.lo).len()) * 2.0).max(8.0 * upf);
        let mut c = json!({
            "id": f.id, "kind": if f.hard { "hard" } else { "clearance" }, "status": status,
            "a": f.a.json(), "b": f.b.json(), "level": level, "area": area,
            "at": [r3(f.at.x), r3(f.at.y), r3(f.at.z)], "depth_in": r3((f.depth / inch).max(0.0)),
            "message": format!("{} {} {} {}", f.a.category, if f.hard { "runs into" } else { "is within the clearance of" }, f.b.category, f.b.system),
            "viewpoint": viewpoint(f.at, size), "first_run": first,
        });
        if !note.is_empty() {
            c["note"] = json!(note);
        }
        items.insert(
            f.id.clone(),
            json!({"status": status, "first_run": first, "seen_run": run_no, "note": note, "a": f.a.handle.hex(), "b": f.b.handle.hex(), "hard": f.hard, "level": level}),
        );
        clashes.push(c);
    }
    // There last run, gone now: resolved.
    let resolved: Vec<Value> = prev_items
        .iter()
        .filter(|(id, v)| !items.contains_key(*id) && v["seen_run"].as_u64() == Some(run_no.saturating_sub(1)))
        .map(|(id, _)| json!(id))
        .collect();
    for (id, v) in &prev_items {
        if !items.contains_key(id) && items.len() < MAX_CLASHES * 2 {
            let mut v = v.clone();
            v["status"] = json!("resolved");
            items.insert(id.clone(), v);
        }
    }
    // Groups.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in &clashes {
        let k = match group_by.as_str() {
            "level" => format!("level {}", c["level"].as_str().unwrap_or_default()),
            "area" => format!("{} area {}", c["level"].as_str().unwrap_or_default(), c["area"].as_str().unwrap_or_default()),
            _ => format!("{} × {}", c["a"]["category"].as_str().unwrap_or_default(), c["b"]["category"].as_str().unwrap_or_default()),
        };
        groups.entry(k).or_default().push(c["id"].as_str().unwrap_or_default().to_string());
    }
    let groups: Vec<Value> = groups.into_iter().map(|(k, ids)| json!({"key": k, "count": ids.len(), "ids": ids})).collect();
    let mut runs = st["clash"]["runs"].as_object().cloned().unwrap_or_default();
    runs.insert(key.clone(), json!({"run": run_no, "a": sa, "b": sb, "items": items}));
    set_state(s, "clash", json!({"runs": runs, "last": key}))?;
    Ok(json!({
        "clashes": clashes, "groups": groups, "total": found.len(), "new": n_new, "active": n_active, "resolved": resolved,
        "tested": tested, "elements": els.len(), "run": run_no, "a": sa, "b": sb.unwrap_or_else(|| vec!["everything else".into()]),
    }))
}

/// The stored record of clash `id` (the last run that has it) and its run key.
fn stored(st: &Value, id: &str) -> Option<(String, Value)> {
    let runs = st["clash"]["runs"].as_object()?;
    let last = st["clash"]["last"].as_str().unwrap_or_default();
    runs.get(last)
        .and_then(|r| r["items"].get(id))
        .map(|v| (last.to_string(), v.clone()))
        .or_else(|| runs.iter().find_map(|(k, r)| r["items"].get(id).map(|v| (k.clone(), v.clone()))))
}

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_param(p, "id").ok_or_else(|| bad(CMD_STATUS, "`id` is required"))?;
    let status = str_param(p, "status").map(str::to_ascii_lowercase).ok_or_else(|| bad(CMD_STATUS, "`status` is required"))?;
    if !STATUSES.contains(&status.as_str()) {
        return Err(bad(CMD_STATUS, "`status` is new, active or resolved"));
    }
    set_item(s, CMD_STATUS, id, |v| {
        v["status"] = json!(status);
        if let Some(n) = str_param(p, "note") {
            v["note"] = json!(n.chars().take(2_000).collect::<String>());
        }
    })?;
    Ok(json!({"id": id, "status": status}))
}

fn set_item(s: &mut Session, cmd: &str, id: &str, f: impl FnOnce(&mut Value)) -> Result<()> {
    let mut st = state(s.doc()?);
    let (key, _) = stored(&st, id).ok_or_else(|| bad(cmd, format!("no clash {id}: run bim.clash first")))?;
    let mut clash = st["clash"].take();
    if let Some(v) = clash.get_mut("runs").and_then(|r| r.get_mut(&key)).and_then(|r| r.get_mut("items")).and_then(|i| i.get_mut(id)) {
        f(v);
    }
    if let Some(o) = st.as_object_mut() {
        o.insert("clash".into(), clash.clone());
    }
    set_state(s, "clash", clash)
}

/// Which element yields: higher numbers stay put. Building elements never move; gravity piping
/// beats duct, duct beats pressure piping, then sprinklers, tray, conduit and devices.
fn priority(e: &El) -> u32 {
    let sys = e.system.to_ascii_uppercase();
    let gravity =
        ["SAN", "SW", "STORM", "WASTE", "VENT", "DRAIN", "GRAV", "SS", "RWL", "WV"].iter().any(|w| sys.split(['-', '_', ' ']).any(|p| p == *w));
    match (e.trade, e.category.as_str()) {
        ("architecture" | "structure", _) => 1_000,
        ("plumbing", "pipe" | "fitting") if gravity => 900,
        ("mechanical", "duct" | "fitting" | "terminal") => 700,
        ("mechanical" | "plumbing", "equipment") => 650,
        ("plumbing", _) => 600,
        ("fire", _) => 550,
        (_, "tray") => 500,
        ("electrical", "conduit" | "cable" | "fitting") => 400,
        _ => 300,
    }
}

fn run_fix(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_param(p, "id").ok_or_else(|| bad(CMD_FIX, "`id` is required"))?.to_string();
    let strategy = str_param(p, "strategy").unwrap_or("reroute").to_ascii_lowercase();
    if !matches!(strategy.as_str(), "reroute" | "raise" | "offset") {
        return Err(bad(CMD_FIX, "`strategy` is reroute, raise or offset"));
    }
    let upf = units_per_foot(s);
    let inch = upf / 12.0;
    let clr = p.get("clearance_in").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(1.0).min(120.0) * inch;
    let max_move = p.get("max_move_in").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(24.0).min(1_200.0) * inch;
    let d = s.doc()?;
    let st = state(d);
    let (_, item) = stored(&st, &id).ok_or_else(|| bad(CMD_FIX, format!("no clash {id}: run bim.clash first")))?;
    let els = solid_elements(d);
    let find = |k: &str| item[k].as_str().and_then(Handle::parse_hex).and_then(|h| els.iter().find(|e| e.handle == h)).cloned();
    let (Some(a), Some(b)) = (find("a"), find("b")) else {
        return Err(bad(CMD_FIX, format!("the elements of {id} are gone: run bim.clash again")));
    };
    let (mover, stays) = if priority(&a) <= priority(&b) { (a, b) } else { (b, a) };
    if priority(&mover) >= 1_000 {
        return Err(bad(
            CMD_FIX,
            format!("{id} is between two building elements ({} and {}): it needs a design change, not a move", mover.category, stays.category),
        ));
    }
    let why = if priority(&mover) == priority(&stays) {
        format!("{} {} moves (same priority as {} {}; the first picked moves)", mover.system, mover.category, stays.system, stays.category)
    } else {
        format!(
            "{} yields to {}",
            mover.category,
            if stays.trade == "architecture" || stays.trade == "structure" {
                stays.category.clone()
            } else {
                format!("{} ({})", stays.category, stays.system)
            }
        )
    };
    // Candidate moves.
    let up = stays.hi.z - mover.lo.z + clr;
    let down = -(mover.hi.z - stays.lo.z + clr);
    let along_x = (mover.hi.x - mover.lo.x) >= (mover.hi.y - mover.lo.y);
    let (side_pos, side_neg) = if along_x {
        (stays.hi.y - mover.lo.y + clr, -(mover.hi.y - stays.lo.y + clr))
    } else {
        (stays.hi.x - mover.lo.x + clr, -(mover.hi.x - stays.lo.x + clr))
    };
    let side = |v: f64| if along_x { Vec3::new(0.0, v, 0.0) } else { Vec3::new(v, 0.0, 0.0) };
    let mut cands: Vec<(&str, Vec3)> = match strategy.as_str() {
        "raise" => vec![("raise", Vec3::new(0.0, 0.0, up))],
        "offset" => vec![("offset", side(side_pos)), ("offset", side(side_neg))],
        _ => vec![("raise", Vec3::new(0.0, 0.0, up)), ("drop", Vec3::new(0.0, 0.0, down)), ("offset", side(side_pos)), ("offset", side(side_neg))],
    };
    cands.sort_by(|x, y| x.1.len().total_cmp(&y.1.len()));
    let others: Vec<&El> =
        els.iter().filter(|e| e.handle != mover.handle && !(e.system == mover.system && !e.system.is_empty() && e.trade == mover.trade)).collect();
    let mut cache: BTreeMap<Handle, (Vec<Tri>, bool)> = BTreeMap::new();
    let mut pick: Option<(&str, Vec3, usize)> = None;
    for (name, mv) in &cands {
        if mv.len() > max_move {
            continue;
        }
        let moved = El { lo: mover.lo + *mv, hi: mover.hi + *mv, ..mover.clone() };
        let (mut moved_tris, moved_closed) = triangles(d, mover.handle, mover.lo, mover.hi);
        for t in &mut moved_tris {
            for v in t.iter_mut() {
                *v = *v + *mv;
            }
        }
        cache.insert(mover.handle, (moved_tris, moved_closed));
        let new_clashes = others
            .iter()
            .filter(|o| boxes_meet((moved.lo, moved.hi), (o.lo, o.hi), 0.0) && !exempt(&moved, o, false))
            .filter(|o| meet(d, &moved, o, 0.5 * inch, 0.0, &mut cache) == Some(true))
            .count();
        cache.remove(&mover.handle);
        if new_clashes == 0 || strategy != "reroute" {
            pick = Some((name, *mv, new_clashes));
            break;
        }
        if pick.is_none_or(|(_, _, n)| new_clashes < n) {
            pick = Some((name, *mv, new_clashes));
        }
    }
    let Some((how, mv, expect_new)) = pick else {
        let sleeve = if stays.trade == "architecture" || stays.trade == "structure" {
            format!(", or a sleeve through the {}", stays.category)
        } else {
            String::new()
        };
        return Err(bad(CMD_FIX, format!("no {strategy} within {} clears {id}: it needs rerouting by hand{sleeve}", feet_in(max_move / inch))));
    };
    let mut out = json!({
        "id": id, "mover": mover.json(), "stays": stays.json(), "why": why, "strategy": strategy, "how": how,
        "move": [r3(mv.x), r3(mv.y), r3(mv.z)], "move_in": r3(mv.len() / inch), "new_clashes_expected": expect_new, "applied": false,
    });
    // A piece of a run moves with its run (its sticks, fittings, straps and wire).
    let movers: Vec<Handle> =
        if mover.run.is_empty() { vec![mover.handle] } else { els.iter().filter(|e| e.run == mover.run).map(|e| e.handle).take(100_000).collect() };
    out["pieces_moved"] = json!(movers.len());
    if bool_or(p, "approve", false) {
        let m = Mat4 { m: [[1.0, 0.0, 0.0, mv.x], [0.0, 1.0, 0.0, mv.y], [0.0, 0.0, 1.0, mv.z], [0.0, 0.0, 0.0, 1.0]] };
        for h in &movers {
            s.doc_mut()?.modify_entity(*h, |e| {
                e.kind.transform3(&m);
            })?;
        }
        // Check again: the pair, and the moved element against the rest.
        let d = s.doc()?;
        let els = solid_elements(d);
        let (Some(moved), Some(st2)) =
            (els.iter().find(|e| e.handle == mover.handle).cloned(), els.iter().find(|e| e.handle == stays.handle).cloned())
        else {
            return Err(bad(CMD_FIX, "the moved element is gone"));
        };
        let mut cache = BTreeMap::new();
        let clear = meet(d, &moved, &st2, 0.5 * inch, 0.0, &mut cache).is_none();
        let new_clashes = els
            .iter()
            .filter(|o| o.handle != moved.handle && !(o.system == moved.system && !o.system.is_empty() && o.trade == moved.trade))
            .filter(|o| boxes_meet((moved.lo, moved.hi), (o.lo, o.hi), 0.0) && !exempt(&moved, o, false))
            .filter(|o| meet(d, &moved, o, 0.5 * inch, 0.0, &mut cache) == Some(true))
            .map(|o| o.handle.hex())
            .collect::<Vec<_>>();
        if clear {
            set_item(s, CMD_FIX, &id, |v| {
                v["status"] = json!("resolved");
                v["fix"] = json!({"how": how, "move": [r3(mv.x), r3(mv.y), r3(mv.z)]});
            })?;
        }
        frame(s, &[mover.handle])?;
        out["applied"] = json!(true);
        out["check"] = json!({"clear": clear, "new_clashes": new_clashes.len(), "with": new_clashes});
    }
    Ok(out)
}

fn feet_in(inches: f64) -> String {
    format!("{}\"", r3(inches))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Tri {
        [Vec3::new(a[0], a[1], a[2]), Vec3::new(b[0], b[1], b[2]), Vec3::new(c[0], c[1], c[2])]
    }

    #[test]
    fn triangles_cross_but_touching_is_not_crossing() {
        let floor = t([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]);
        let through = t([2.0, 2.0, -1.0], [2.0, 2.0, 1.0], [3.0, 2.0, 1.0]);
        let above = t([2.0, 2.0, 0.5], [2.0, 2.0, 1.0], [3.0, 2.0, 1.0]);
        let resting = t([2.0, 2.0, 0.0], [2.0, 3.0, 1.0], [3.0, 2.0, 1.0]);
        assert!(tri_tri(&floor, &through));
        assert!(!tri_tri(&floor, &above));
        assert!(!tri_tri(&floor, &resting), "a vertex on the face is touching");
        // Degenerate and hostile triangles never cross anything.
        let flat = t([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        let nan = t([f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(!tri_tri(&floor, &flat) && !tri_tri(&nan, &through));
    }

    #[test]
    fn a_point_inside_a_box_mesh() {
        let b = box_tris(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0));
        assert!(inside(Vec3::new(1.0, 2.0, 3.0), &b, 20.0));
        assert!(!inside(Vec3::new(5.0, 2.0, 3.0), &b, 20.0));
        assert!(tri_in_box(&t([-1.0, 2.0, 2.0], [5.0, 2.0, 2.0], [2.0, 2.0, 9.0]), Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0)));
        assert!(!tri_in_box(&t([5.0, 5.0, 5.0], [6.0, 5.0, 5.0], [5.0, 6.0, 5.0]), Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0)));
        // A big triangle that the box pokes through (no vertex or edge of it inside the box).
        assert!(tri_in_box(&t([-50.0, -50.0, 2.0], [50.0, -50.0, 2.0], [0.0, 50.0, 2.0]), Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0)));
    }

    #[test]
    fn who_yields() {
        let el = |trade: &'static str, cat: &str, sys: &str| El {
            handle: Handle(1),
            trade,
            category: cat.into(),
            system: sys.into(),
            level: String::new(),
            lo: Vec3::ZERO,
            hi: Vec3::ZERO,
            run: String::new(),
            in_wall: false,
        };
        let conduit = el("electrical", "conduit", "E-COND");
        let duct = el("mechanical", "duct", "M-SUPPLY");
        let sanitary = el("plumbing", "pipe", "P-SAN");
        let water = el("plumbing", "pipe", "P-DCW");
        let wall = el("architecture", "wall", "A-WALL");
        assert!(priority(&conduit) < priority(&duct));
        assert!(priority(&duct) < priority(&sanitary));
        assert!(priority(&water) < priority(&duct));
        assert!(priority(&duct) < priority(&wall));
    }
}
