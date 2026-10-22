//! Parametric parts (Q277): a history-based part. Constrained 2D sketches on a plane (or a plane
//! taken from a solid's face) feed features — Pad, Pocket, Revolve, Fillet, Chamfer — kept in a
//! tree with named parameters (`width = 2*thick`). Change a sketch or a parameter and the part
//! rebuilds from the top: each sketch is solved by the constraint solver, its closed loops become
//! profiles, and each feature adds to or cuts the body. The evaluated body is an ordinary 3D solid
//! in the drawing (so it saves, exports and plots like any other); the tree is kept in the
//! drawing (`Drawing::parts`, `.cdy` PRTS, DXF `CADADDY_PARTS`).
//!
//! Fillets and chamfers name their edges by the two faces either side (the faces' tags), not by a
//! position, so a resized part keeps them on the same edges.

use std::collections::BTreeMap;

use cadaddy_constraints::{Env, SolveOptions};
use cadaddy_doc::{
    Arc, BoolOp, Circle, Common, Constraint, ConstraintKind, DistAxis, Drawing, EntityKind, GeomRef, Handle, Line, Parameter, Solid3d, SolidShape,
    Space, Sub, combine, placement,
};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const MAX_PARTS: usize = 1_000;
const MAX_FEATURES: usize = 2_000;
const MAX_GEOMETRY: usize = 2_000;
const MAX_PARAMETERS: usize = 1_000;
const MAX_EDGES: usize = 100;
/// The largest size a feature may have (drawing units).
const MAX_SIZE: f64 = 1e7;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parts {
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Part {
    pub name: String,
    pub parameters: Vec<Param>,
    pub features: Vec<Feature>,
    /// The evaluated body's 3D solid in the drawing (hex handle).
    pub solid: Option<String>,
    pub next_id: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub expr: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub suppressed: bool,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Kind {
    Sketch {
        plane: Plane,
        geometry: Vec<Geom>,
        constraints: Vec<Constraint>,
    },
    Pad {
        sketch: u32,
        length: String,
        #[serde(default)]
        reversed: bool,
        #[serde(default)]
        symmetric: bool,
    },
    Pocket {
        sketch: u32,
        depth: String,
        #[serde(default)]
        through_all: bool,
        #[serde(default)]
        reversed: bool,
    },
    Revolve {
        sketch: u32,
        angle: String,
        /// `v`: the sketch's vertical axis (its Y), `h`: its horizontal axis (its X).
        axis: String,
        #[serde(default)]
        cut: bool,
    },
    Fillet {
        edges: Vec<[u64; 2]>,
        radius: String,
    },
    Chamfer {
        edges: Vec<[u64; 2]>,
        distance: String,
    },
}

impl Kind {
    fn type_name(&self) -> &'static str {
        match self {
            Kind::Sketch { .. } => "Sketch",
            Kind::Pad { .. } => "Pad",
            Kind::Pocket { .. } => "Pocket",
            Kind::Revolve { .. } => "Revolve",
            Kind::Fillet { .. } => "Fillet",
            Kind::Chamfer { .. } => "Chamfer",
        }
    }
    fn sketch(&self) -> Option<u32> {
        match self {
            Kind::Pad { sketch, .. } | Kind::Pocket { sketch, .. } | Kind::Revolve { sketch, .. } => Some(*sketch),
            _ => None,
        }
    }
}

/// A sketch's plane: a standard plane moved along its normal by `offset`, or a plane in space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Plane {
    Standard { name: String, offset: String },
    Custom { origin: [f64; 3], x: [f64; 3], normal: [f64; 3] },
}

/// Sketch geometry in the sketch's own 2D coordinates. Construction geometry is constrained but
/// never part of a profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Geom {
    Line {
        a: [f64; 2],
        b: [f64; 2],
        #[serde(default)]
        construction: bool,
    },
    Arc {
        center: [f64; 2],
        radius: f64,
        start: f64,
        end: f64,
        #[serde(default)]
        construction: bool,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
        #[serde(default)]
        construction: bool,
    },
}

impl Geom {
    fn construction(&self) -> bool {
        match self {
            Geom::Line { construction, .. } | Geom::Arc { construction, .. } | Geom::Circle { construction, .. } => *construction,
        }
    }
}

fn v3(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

fn arr3(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

// ---------------- state ----------------

fn load(d: &Drawing) -> Parts {
    d.parts.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
}

fn store(s: &mut Session, p: &Parts) -> Result<()> {
    s.doc_mut()?.parts = if p.parts.is_empty() { None } else { serde_json::to_string(p).ok() };
    Ok(())
}

/// The part a command names, or the only/last one.
fn part_index(cmd: &str, ps: &Parts, p: &Value) -> Result<usize> {
    match str_param(p, "part").map(str::trim).filter(|x| !x.is_empty()) {
        Some(n) => ps
            .parts
            .iter()
            .position(|x| x.name.eq_ignore_ascii_case(n))
            .ok_or_else(|| bad(cmd, format!("no part named {}", n.chars().take(60).collect::<String>()))),
        None => ps.parts.len().checked_sub(1).ok_or_else(|| bad(cmd, "no part yet (part.new)")),
    }
}

/// A feature by id or name.
fn feature_index(cmd: &str, part: &Part, v: Option<&Value>) -> Result<usize> {
    let v = v.ok_or_else(|| bad(cmd, "feature required (its id or name)"))?;
    let found = match v {
        Value::Number(n) => n.as_u64().and_then(|id| part.features.iter().position(|f| u64::from(f.id) == id)),
        Value::String(t) => part
            .features
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(t.trim()))
            .or_else(|| t.trim().parse::<u32>().ok().and_then(|id| part.features.iter().position(|f| f.id == id))),
        _ => None,
    };
    found.ok_or_else(|| bad(cmd, format!("no feature {v} in {}", part.name)))
}

fn env(part: &Part) -> Env {
    let mut e = Env::new();
    for x in &part.parameters {
        e.insert(&x.name, &x.expr);
    }
    e
}

/// A length, angle or radius: an expression in the part's parameters.
fn eval(e: &mut Env, what: &str, text: &str) -> std::result::Result<f64, String> {
    let v = e.eval(text).map_err(|x| format!("{what} `{}`: {x}", text.chars().take(80).collect::<String>()))?;
    if v.is_finite() && v.abs() <= MAX_SIZE { Ok(v) } else { Err(format!("{what} `{text}` is out of range")) }
}

// ---------------- sketches ----------------

/// A sketch plane's frame: origin, X, Y and normal.
fn frame(e: &mut Env, plane: &Plane) -> std::result::Result<(Vec3, Vec3, Vec3, Vec3), String> {
    match plane {
        Plane::Standard { name, offset } => {
            let off = if offset.trim().is_empty() { 0.0 } else { eval(e, "offset", offset)? };
            let (x, y, n) = match name.to_ascii_lowercase().as_str() {
                "xy" => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
                "xz" => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, -1.0, 0.0)),
                "yz" => (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0)),
                _ => return Err(format!("no plane {name}: xy, xz or yz")),
            };
            Ok((n * off, x, y, n))
        }
        Plane::Custom { origin, x, normal } => {
            let n = v3(*normal).normalized();
            let x0 = v3(*x);
            let x = (x0 - n * x0.dot(n)).normalized();
            if !(n.len() > 0.5 && x.len() > 0.5 && origin.iter().all(|c| c.is_finite() && c.abs() <= MAX_SIZE)) {
                return Err("the sketch plane is degenerate".into());
            }
            Ok((v3(*origin), x, n.cross(x), n))
        }
    }
}

/// Solve a sketch's constraints (in a scratch drawing, the part's parameters available to its
/// dimensions) and return the solved geometry.
fn solve_sketch(part: &Part, geometry: &[Geom], constraints: &[Constraint]) -> std::result::Result<Vec<Geom>, String> {
    if constraints.is_empty() {
        return Ok(geometry.to_vec());
    }
    let mut d = Drawing::default();
    d.parametric.parameters =
        part.parameters.iter().map(|x| Parameter { name: x.name.clone(), expr: x.expr.clone(), description: String::new() }).collect();
    let mut handles = Vec::new();
    for g in geometry {
        let kind = match g {
            Geom::Line { a, b, .. } => EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }),
            Geom::Arc { center, radius, start, end, .. } => {
                EntityKind::Arc(Arc { center: Vec3::new(center[0], center[1], 0.0), radius: *radius, start: *start, end: *end })
            }
            Geom::Circle { center, radius, .. } => EntityKind::Circle(Circle { center: Vec3::new(center[0], center[1], 0.0), radius: *radius }),
        };
        handles.push(d.add(&Space::Model, Common::default(), kind).map_err(|e| e.to_string())?);
    }
    // The sketch's constraints refer to geometry by number (handle = index + 1).
    for c in constraints {
        let refs = c
            .refs
            .iter()
            .map(|r| {
                let i = usize::try_from(r.handle.0)
                    .ok()
                    .and_then(|h| h.checked_sub(1))
                    .and_then(|i| handles.get(i))
                    .ok_or_else(|| format!("constraint {}: no geometry {}", c.id, r.handle.0))?;
                Ok(GeomRef { handle: *i, sub: r.sub })
            })
            .collect::<std::result::Result<Vec<_>, String>>()?;
        d.constraints.push(Constraint { refs, ..c.clone() });
    }
    let report = cadaddy_constraints::solve(&mut d, &SolveOptions::default()).map_err(|e| e.to_string())?;
    if let Some((id, why)) = report.skipped.first() {
        return Err(format!("constraint {id}: {why}"));
    }
    geometry
        .iter()
        .zip(&handles)
        .map(|(g, h)| {
            let e = d.entity(*h).ok_or("the solver lost a curve")?;
            Ok(match (&e.kind, g) {
                (EntityKind::Line(l), Geom::Line { construction, .. }) => {
                    Geom::Line { a: [l.a.x, l.a.y], b: [l.b.x, l.b.y], construction: *construction }
                }
                (EntityKind::Arc(a), Geom::Arc { construction, .. }) => {
                    Geom::Arc { center: [a.center.x, a.center.y], radius: a.radius, start: a.start, end: a.end, construction: *construction }
                }
                (EntityKind::Circle(c), Geom::Circle { construction, .. }) => {
                    Geom::Circle { center: [c.center.x, c.center.y], radius: c.radius, construction: *construction }
                }
                _ => g.clone(),
            })
        })
        .collect()
}

/// A curve of a profile: its two ends and the bulge from the first to the second.
struct Piece {
    a: Vec2,
    b: Vec2,
    bulge: f64,
}

/// The sketch's closed loops as profiles (bulge polylines); open chains are an error.
fn profiles(geometry: &[Geom]) -> std::result::Result<Vec<Vec<PolyVertex>>, String> {
    let mut loops: Vec<Vec<PolyVertex>> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut scale = 1.0f64;
    for g in geometry.iter().filter(|g| !g.construction()) {
        match g {
            Geom::Line { a, b, .. } => pieces.push(Piece { a: Vec2::new(a[0], a[1]), b: Vec2::new(b[0], b[1]), bulge: 0.0 }),
            Geom::Arc { center, radius, start, end, .. } => {
                let c = Vec2::new(center[0], center[1]);
                let mut sweep = (end - start).rem_euclid(std::f64::consts::TAU);
                if sweep <= 1e-12 {
                    sweep = std::f64::consts::TAU;
                }
                let at = |t: f64| c + Vec2::new(t.cos(), t.sin()) * *radius;
                pieces.push(Piece { a: at(*start), b: at(*end), bulge: (sweep / 4.0).tan() });
            }
            Geom::Circle { center, radius, .. } => {
                let c = Vec2::new(center[0], center[1]);
                let v = |p: Vec2| PolyVertex { p, bulge: 1.0, start_width: 0.0, end_width: 0.0 };
                loops.push(vec![v(c + Vec2::new(*radius, 0.0)), v(c - Vec2::new(*radius, 0.0))]);
            }
        }
    }
    for p in &pieces {
        scale = scale.max(p.a.x.abs()).max(p.a.y.abs()).max(p.b.x.abs()).max(p.b.y.abs());
    }
    let eps = 1e-6 * scale;
    let mut used = vec![false; pieces.len()];
    for start in 0..pieces.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Some(first) = pieces.get(start) else { continue };
        if let Some(u) = used.get_mut(start) {
            *u = true;
        }
        let origin = first.a;
        let mut verts = vec![PolyVertex { p: first.a, bulge: first.bulge, start_width: 0.0, end_width: 0.0 }];
        let mut at = first.b;
        let mut closed = (at - origin).len() <= eps;
        while !closed {
            // The next unused piece starting or ending here (reversed if it ends here).
            let next = pieces
                .iter()
                .enumerate()
                .find(|(i, q)| !used.get(*i).copied().unwrap_or(true) && ((q.a - at).len() <= eps || (q.b - at).len() <= eps));
            let Some((i, q)) = next else {
                return Err(format!("the sketch's profile isn't closed (an open end at {:.4}, {:.4})", at.x, at.y));
            };
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            let (from, to, bulge) = if (q.a - at).len() <= eps { (q.a, q.b, q.bulge) } else { (q.b, q.a, -q.bulge) };
            verts.push(PolyVertex { p: from, bulge, start_width: 0.0, end_width: 0.0 });
            at = to;
            closed = (at - origin).len() <= eps;
        }
        if verts.len() >= 2 || verts.first().is_some_and(|v| v.bulge != 0.0) {
            loops.push(verts);
        }
    }
    if loops.is_empty() {
        return Err("the sketch has no closed profile".into());
    }
    Ok(loops)
}

// ---------------- rebuild ----------------

/// What a rebuild made: the body (none when every solid feature is suppressed) and each
/// feature's state.
pub struct Built {
    pub body: Option<Solid3d>,
    pub status: Vec<Value>,
    /// Sketches with their solved geometry.
    pub solved: BTreeMap<u32, Vec<Geom>>,
}

fn bounds_diag(so: &Solid3d) -> f64 {
    let pr = so.props(so.mesh_tol());
    let d = (pr.max - pr.min).len();
    if d.is_finite() && d > 0.0 { d } else { 1.0 }
}

/// Evaluate the feature tree from the top.
pub fn rebuild(part: &Part) -> Built {
    let mut e = env(part);
    let mut body: Option<Solid3d> = None;
    let mut sketches: BTreeMap<u32, (Vec<Vec<PolyVertex>>, (Vec3, Vec3, Vec3, Vec3))> = BTreeMap::new();
    let mut solved = BTreeMap::new();
    let mut status = Vec::new();
    for f in part.features.iter().take(MAX_FEATURES) {
        if f.suppressed {
            status.push(json!({"id": f.id, "name": f.name, "type": f.kind.type_name(), "state": "suppressed"}));
            continue;
        }
        let r: std::result::Result<(), String> = (|| {
            match &f.kind {
                Kind::Sketch { plane, geometry, constraints } => {
                    let fr = frame(&mut e, plane)?;
                    let g = solve_sketch(part, geometry, constraints)?;
                    // A sketch without a closed profile is fine until a feature uses it.
                    let loops = profiles(&g).unwrap_or_default();
                    solved.insert(f.id, g);
                    sketches.insert(f.id, (loops, fr));
                }
                Kind::Pad { sketch, length, reversed, symmetric } => {
                    let (loops, (o, x, y, n)) = profile_of(part, &sketches, *sketch)?;
                    let l = eval(&mut e, "length", length)?;
                    if l <= 0.0 {
                        return Err("the length must be more than zero".into());
                    }
                    let base = if *symmetric {
                        o - n * (l / 2.0)
                    } else if *reversed {
                        o - n * l
                    } else {
                        o
                    };
                    let so = Solid3d { shape: SolidShape::Extrusion { loops, height: l, taper: 0.0 }, xf: placement(base, x, y, n) };
                    body = Some(match body.take() {
                        Some(b) => combine(b, so, BoolOp::Union),
                        None => so,
                    });
                }
                Kind::Pocket { sketch, depth, through_all, reversed } => {
                    // Everything that can fail before the body is taken: a failed feature leaves
                    // the body as it was.
                    let (loops, (o, x, y, n)) = profile_of(part, &sketches, *sketch)?;
                    let given = if *through_all { None } else { Some(eval(&mut e, "depth", depth)?) };
                    if given.is_some_and(|l| l <= 0.0) {
                        return Err("the depth must be more than zero".into());
                    }
                    let b = body.take().ok_or("a pocket needs a body to cut")?;
                    let l = given.unwrap_or_else(|| 2.0 * bounds_diag(&b) + (o - Vec3::ZERO).len());
                    // Into the material: against the sketch's normal unless reversed.
                    let base = if *reversed { o } else { o - n * l };
                    let cutter = Solid3d { shape: SolidShape::Extrusion { loops, height: l, taper: 0.0 }, xf: placement(base, x, y, n) };
                    body = Some(combine(b, cutter, BoolOp::Subtract));
                }
                Kind::Revolve { sketch, angle, axis, cut } => {
                    let (loops, (o, x, y, n)) = profile_of(part, &sketches, *sketch)?;
                    let a = eval(&mut e, "angle", angle)?;
                    if !(a > 0.0 && a <= 360.0) {
                        return Err("the angle is more than 0 and at most 360 degrees".into());
                    }
                    // Revolution turns a profile in its XY about its Y, with the profile at x ≥ 0.
                    let (loops, xf) = if axis.eq_ignore_ascii_case("h") {
                        let swapped = loops
                            .into_iter()
                            .map(|l| l.into_iter().map(|v| PolyVertex { p: Vec2::new(v.p.y, v.p.x), bulge: -v.bulge, ..v }).collect())
                            .collect();
                        (swapped, placement(o, y, x, n * -1.0))
                    } else {
                        (loops, placement(o, x, y, n))
                    };
                    let tol = 1e-9 * loops.iter().flatten().fold(1.0f64, |m: f64, v: &PolyVertex| m.max(v.p.x.abs()).max(v.p.y.abs()));
                    if loops.iter().flatten().any(|v| v.p.x < -tol) {
                        return Err("the profile crosses the axis: keep it on one side".into());
                    }
                    let so = Solid3d { shape: SolidShape::Revolution { loops, angle: a.to_radians() }, xf };
                    body = Some(match (body.take(), cut) {
                        (Some(b), true) => combine(b, so, BoolOp::Subtract),
                        (None, true) => return Err("a cut needs a body".into()),
                        (Some(b), false) => combine(b, so, BoolOp::Union),
                        (None, false) => so,
                    });
                }
                Kind::Fillet { edges, radius } | Kind::Chamfer { edges, distance: radius } => {
                    let fillet = matches!(f.kind, Kind::Fillet { .. });
                    let size = eval(&mut e, if fillet { "radius" } else { "distance" }, radius)?;
                    let mut b = body.take().ok_or("an edge feature needs a body")?;
                    let start = b.clone();
                    let tol = start.mesh_tol();
                    // Every edge found on the body as it was, then cut one after another.
                    let found: Vec<cadaddy_doc::Edge> = match edges
                        .iter()
                        .map(|t| start.edge_between(*t, tol).ok_or_else(|| format!("edge {}/{} isn't on the body any more", t[0], t[1])))
                        .collect()
                    {
                        Ok(v) => v,
                        Err(x) => {
                            body = Some(start);
                            return Err(x);
                        }
                    };
                    for edge in &found {
                        b = match if fillet { b.fillet_edge(edge, size) } else { b.chamfer_edge(edge, size) } {
                            Ok(x) => x,
                            Err(x) => {
                                body = Some(start);
                                return Err(x.into());
                            }
                        };
                    }
                    if !b.is_valid() {
                        body = Some(start);
                        return Err("the result isn't a valid solid".into());
                    }
                    body = Some(b);
                }
            }
            Ok(())
        })();
        match r {
            Ok(()) => status.push(json!({"id": f.id, "name": f.name, "type": f.kind.type_name(), "state": "ok"})),
            Err(x) => status.push(json!({"id": f.id, "name": f.name, "type": f.kind.type_name(), "state": "error", "error": x})),
        }
    }
    Built { body, status, solved }
}

type Profile = (Vec<Vec<PolyVertex>>, (Vec3, Vec3, Vec3, Vec3));

fn profile_of(part: &Part, sketches: &BTreeMap<u32, Profile>, id: u32) -> std::result::Result<Profile, String> {
    let name = part.features.iter().find(|f| f.id == id).map_or_else(|| id.to_string(), |f| f.name.clone());
    let (loops, fr) = sketches.get(&id).ok_or_else(|| format!("sketch {name} isn't built (suppressed, failed, or later in the tree)"))?;
    if loops.is_empty() {
        return Err(format!("sketch {name} has no closed profile"));
    }
    Ok((loops.clone(), *fr))
}

/// Rebuild a part, write its solved sketches back and put its body in the drawing.
fn apply(s: &mut Session, ps: &mut Parts, i: usize) -> Result<Value> {
    let part = ps.parts.get_mut(i).ok_or_else(|| bad("part", "no such part"))?;
    let built = rebuild(part);
    for f in &mut part.features {
        if let (Kind::Sketch { geometry, .. }, Some(g)) = (&mut f.kind, built.solved.get(&f.id)) {
            *geometry = g.clone();
        }
    }
    let old = part.solid.as_deref().and_then(Handle::parse_hex).filter(|h| s.doc().is_ok_and(|d| d.entity(*h).is_some()));
    let mut volume = Value::Null;
    match (&built.body, old) {
        (Some(b), Some(h)) => {
            let kind = EntityKind::Solid3d(Box::new(b.clone()));
            s.doc_mut()?.modify_entity(h, |e| e.kind = kind)?;
        }
        (Some(b), None) => {
            let h = s.add_entity(EntityKind::Solid3d(Box::new(b.clone())))?;
            part.solid = Some(h.hex());
        }
        (None, Some(h)) => {
            s.doc_mut()?.remove_entity(h);
            part.solid = None;
        }
        (None, None) => part.solid = None,
    }
    if let Some(b) = &built.body {
        let pr = b.props(b.mesh_tol());
        volume = json!(pr.volume);
    }
    let out = json!({"part": part.name, "solid": part.solid, "volume": volume, "features": built.status});
    store(s, ps)?;
    Ok(out)
}

// ---------------- commands ----------------

fn new_id(part: &mut Part) -> u32 {
    part.next_id = part.next_id.max(part.features.iter().map(|f| f.id).max().unwrap_or(0)) + 1;
    part.next_id
}

fn add_feature(s: &mut Session, cmd: &str, p: &Value, kind: Kind) -> Result<Value> {
    let mut ps = load(s.doc()?);
    let i = part_index(cmd, &ps, p)?;
    let part = ps.parts.get_mut(i).ok_or_else(|| bad(cmd, "no such part"))?;
    if part.features.len() >= MAX_FEATURES {
        return Err(bad(cmd, format!("a part holds at most {MAX_FEATURES} features")));
    }
    if let Some(sk) = kind.sketch()
        && !part.features.iter().any(|f| f.id == sk && matches!(f.kind, Kind::Sketch { .. }))
    {
        return Err(bad(cmd, format!("no sketch {sk} in {}", part.name)));
    }
    let id = new_id(part);
    let count = part.features.iter().filter(|f| f.kind.type_name() == kind.type_name()).count() + 1;
    let name = str_param(p, "name")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| x.chars().take(80).collect())
        .unwrap_or_else(|| format!("{}{count:03}", kind.type_name()));
    part.features.push(Feature { id, name, suppressed: false, kind });
    let mut out = apply(s, &mut ps, i)?;
    out["id"] = json!(id);
    Ok(out)
}

fn sketch_ref(cmd: &str, s: &Session, p: &Value) -> Result<u32> {
    let ps = load(s.doc()?);
    let part = ps.parts.get(part_index(cmd, &ps, p)?).ok_or_else(|| bad(cmd, "no such part"))?;
    let i = feature_index(cmd, part, p.get("sketch"))?;
    part.features.get(i).filter(|f| matches!(f.kind, Kind::Sketch { .. })).map(|f| f.id).ok_or_else(|| bad(cmd, "that feature isn't a sketch"))
}

/// An expression parameter: a number or text in the part's parameters.
fn expr_param(cmd: &str, p: &Value, k: &str, default: Option<&str>) -> Result<String> {
    match p.get(k) {
        Some(Value::Number(n)) => n
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= MAX_SIZE)
            .map(|v| format!("{v}"))
            .ok_or_else(|| bad(cmd, format!("{k} is out of range"))),
        Some(Value::String(t)) if !t.trim().is_empty() => {
            cadaddy_constraints::expr::parse(t).map_err(|e| bad(cmd, format!("{k}: {e}")))?;
            Ok(t.trim().chars().take(400).collect())
        }
        _ => default.map(str::to_string).ok_or_else(|| bad(cmd, format!("{k} required (a number or an expression)"))),
    }
}

fn pt2(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    let x = a.first()?.as_f64()?;
    let y = a.get(1)?.as_f64()?;
    (x.is_finite() && y.is_finite() && x.abs() <= MAX_SIZE && y.abs() <= MAX_SIZE).then_some([x, y])
}

fn pt3(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite() && x.abs() <= MAX_SIZE);
    Some(Vec3::new(c(0)?, c(1)?, c(2).unwrap_or(0.0)))
}

fn geometry(cmd: &str, v: Option<&Value>) -> Result<Vec<Geom>> {
    let a = v.and_then(Value::as_array).ok_or_else(|| bad(cmd, "geometry required: [{type: line|arc|circle, …}]"))?;
    if a.len() > MAX_GEOMETRY {
        return Err(bad(cmd, format!("a sketch holds at most {MAX_GEOMETRY} curves")));
    }
    a.iter()
        .enumerate()
        .map(|(i, g)| {
            let construction = g["construction"].as_bool().unwrap_or(false);
            let num = |k: &str| g[k].as_f64().filter(|x| x.is_finite() && x.abs() <= MAX_SIZE);
            let radius = || num("radius").filter(|r| *r > 0.0).ok_or_else(|| bad(cmd, format!("geometry {i}: radius must be more than zero")));
            match g["type"].as_str().unwrap_or("") {
                "line" => match (pt2(&g["a"]), pt2(&g["b"])) {
                    (Some(a), Some(b)) if a != b => Ok(Geom::Line { a, b, construction }),
                    _ => Err(bad(cmd, format!("geometry {i}: a line needs two different points a and b"))),
                },
                "arc" => {
                    let center = pt2(&g["center"]).ok_or_else(|| bad(cmd, format!("geometry {i}: center required")))?;
                    let (st, en) = (
                        num("start").ok_or_else(|| bad(cmd, format!("geometry {i}: start angle (degrees)")))?,
                        num("end").ok_or_else(|| bad(cmd, format!("geometry {i}: end angle (degrees)")))?,
                    );
                    Ok(Geom::Arc { center, radius: radius()?, start: st.to_radians(), end: en.to_radians(), construction })
                }
                "circle" => Ok(Geom::Circle {
                    center: pt2(&g["center"]).ok_or_else(|| bad(cmd, format!("geometry {i}: center required")))?,
                    radius: radius()?,
                    construction,
                }),
                t => Err(bad(cmd, format!("geometry {i}: type is line, arc or circle, not {}", t.chars().take(20).collect::<String>()))),
            }
        })
        .collect()
}

fn constraint_kind(t: &str) -> Option<ConstraintKind> {
    use ConstraintKind::*;
    Some(match t.trim().to_ascii_lowercase().replace(['_', ' '], "").as_str() {
        "coincident" => Coincident,
        "collinear" => Collinear,
        "concentric" => Concentric,
        "fix" | "fixed" => Fix,
        "parallel" => Parallel,
        "perpendicular" => Perpendicular,
        "horizontal" => Horizontal,
        "vertical" => Vertical,
        "tangent" => Tangent,
        "symmetric" => Symmetric,
        "equal" => Equal,
        "distance" | "aligned" => Distance(DistAxis::Aligned),
        "horizontaldistance" => Distance(DistAxis::Horizontal),
        "verticaldistance" => Distance(DistAxis::Vertical),
        "angle" | "angular" => Angular,
        "radius" => Radius,
        "diameter" => Diameter,
        _ => return None,
    })
}

fn constraints(cmd: &str, v: Option<&Value>, n_geom: usize) -> Result<Vec<Constraint>> {
    let Some(a) = v.and_then(Value::as_array) else { return Ok(Vec::new()) };
    if a.len() > cadaddy_constraints::MAX_CONSTRAINTS {
        return Err(bad(cmd, "too many constraints"));
    }
    a.iter()
        .enumerate()
        .map(|(i, c)| {
            let kind = c["kind"].as_str().and_then(constraint_kind).ok_or_else(|| bad(cmd, format!("constraint {i}: no such kind {}", c["kind"])))?;
            let refs = c["refs"]
                .as_array()
                .ok_or_else(|| bad(cmd, format!("constraint {i}: refs required: [{{geom, sub?}}]")))?
                .iter()
                .take(8)
                .map(|r| {
                    let g = r["geom"]
                        .as_u64()
                        .filter(|g| (*g as usize) < n_geom)
                        .ok_or_else(|| bad(cmd, format!("constraint {i}: geom is a curve number below {n_geom}")))?;
                    let sub =
                        Sub::parse(r["sub"].as_str().unwrap_or("")).ok_or_else(|| bad(cmd, format!("constraint {i}: no such sub {}", r["sub"])))?;
                    Ok(GeomRef { handle: Handle(g + 1), sub })
                })
                .collect::<Result<Vec<_>>>()?;
            let expr = if kind.is_dimensional() {
                expr_param(cmd, c, "value", None)
                    .map_err(|_| bad(cmd, format!("constraint {i}: a dimension needs value (a number or an expression)")))?
            } else {
                String::new()
            };
            Ok(Constraint { id: i as u32 + 1, kind, refs, name: String::new(), expr })
        })
        .collect()
}

fn plane_param(cmd: &str, s: &Session, p: &Value) -> Result<Plane> {
    if let Some(face) = p.get("face") {
        // A plane on a solid's face, taken once (phase 1: a face reference doesn't follow the face).
        let pt = pt3(&face["point"]).ok_or_else(|| bad(cmd, "face.point required: [x, y, z] on a face of the part"))?;
        let ps = load(s.doc()?);
        let part = ps.parts.get(part_index(cmd, &ps, p)?).ok_or_else(|| bad(cmd, "no such part"))?;
        let body = rebuild(part).body.ok_or_else(|| bad(cmd, "the part has no body to sketch on"))?;
        let (_, n) = body.face_at(pt, false, body.mesh_tol()).ok_or_else(|| bad(cmd, "no face of the part there"))?;
        let (x, _) = cadaddy_doc::frame_for(n);
        return Ok(Plane::Custom { origin: arr3(pt), x: arr3(x), normal: arr3(n) });
    }
    match p.get("plane") {
        None => Ok(Plane::Standard { name: "xy".into(), offset: String::new() }),
        Some(Value::String(n)) if ["xy", "xz", "yz"].contains(&n.trim().to_ascii_lowercase().as_str()) => Ok(Plane::Standard {
            name: n.trim().to_ascii_lowercase(),
            offset: if p.get("offset").is_some() { expr_param(cmd, p, "offset", None)? } else { String::new() },
        }),
        Some(v) if v.is_object() => {
            let origin = pt3(&v["origin"]).ok_or_else(|| bad(cmd, "plane.origin required"))?;
            let normal = pt3(&v["normal"]).filter(|n| n.len() > 1e-9).ok_or_else(|| bad(cmd, "plane.normal required"))?;
            let x = pt3(&v["x"]).filter(|x| x.cross(normal).len() > 1e-9).unwrap_or_else(|| cadaddy_doc::frame_for(normal.normalized()).0);
            Ok(Plane::Custom { origin: arr3(origin), x: arr3(x), normal: arr3(normal) })
        }
        Some(v) => Err(bad(cmd, format!("plane is xy, xz, yz or {{origin, normal, x?}}, not {v}"))),
    }
}

fn run_new(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.new";
    let mut ps = load(s.doc()?);
    if ps.parts.len() >= MAX_PARTS {
        return Err(bad(cmd, "too many parts"));
    }
    let name = str_param(p, "name")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| x.chars().take(80).collect::<String>())
        .unwrap_or_else(|| format!("Part{:03}", ps.parts.len() + 1));
    if ps.parts.iter().any(|x| x.name.eq_ignore_ascii_case(&name)) {
        return Err(bad(cmd, format!("there is already a part named {name}")));
    }
    // Its parameters all together, in any order (`depth: "2*thick"` beside `thick`), and all or
    // nothing: every one must evaluate before the part is stored, so a refused set leaves no
    // half-made part in the way of a retry.
    let mut part = Part { name: name.clone(), ..Part::default() };
    if let Some(params) = p.get("parameters").filter(|v| !v.is_null()) {
        let params = params.as_object().ok_or_else(|| bad(cmd, "`parameters` is {name: number or expression}"))?;
        if params.len() > MAX_PARAMETERS {
            return Err(bad(cmd, "too many parameters"));
        }
        for (k, v) in params {
            let k = k.trim();
            if !cadaddy_constraints::expr::valid_name(k) {
                return Err(bad(cmd, format!("parameter `{k}`: a name is a letter, then letters, digits or _")));
            }
            let expr = expr_param(cmd, &json!({ "expr": v }), "expr", None)?;
            part.parameters.push(Param { name: k.to_string(), expr });
        }
        let mut e = env(&part);
        if let Some(err) = part.parameters.iter().find_map(|x| e.value(&x.name).err().map(|er| format!("{}: {er}", x.name))) {
            return Err(bad(cmd, err));
        }
    }
    ps.parts.push(part);
    store(s, &ps)?;
    Ok(json!({"part": name}))
}

fn run_parameter(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.parameter";
    let mut ps = load(s.doc()?);
    let i = part_index(cmd, &ps, p)?;
    let part = ps.parts.get_mut(i).ok_or_else(|| bad(cmd, "no such part"))?;
    let name = str_param(p, "name")
        .map(str::trim)
        .filter(|n| cadaddy_constraints::expr::valid_name(n))
        .ok_or_else(|| bad(cmd, "name required: a letter, then letters, digits or _"))?
        .to_string();
    let before = part.parameters.clone();
    match p.get("expr") {
        None | Some(Value::Null) => {
            let n = part.parameters.len();
            part.parameters.retain(|x| x.name != name);
            if part.parameters.len() == n {
                return Err(bad(cmd, format!("no parameter {name}")));
            }
        }
        Some(_) => {
            let expr = expr_param(cmd, p, "expr", None)?;
            let n = part.parameters.len();
            match part.parameters.iter_mut().find(|x| x.name == name) {
                Some(x) => x.expr = expr,
                None if n < MAX_PARAMETERS => part.parameters.push(Param { name: name.clone(), expr }),
                None => return Err(bad(cmd, "too many parameters")),
            }
        }
    }
    // Every parameter must still evaluate (no cycles, no unknown names).
    let mut e = env(part);
    if let Some(err) = part.parameters.iter().find_map(|x| e.value(&x.name).err().map(|er| format!("{}: {er}", x.name))) {
        part.parameters = before;
        return Err(bad(cmd, err));
    }
    let mut out = apply(s, &mut ps, i)?;
    let part = ps.parts.get(i).ok_or_else(|| bad(cmd, "no such part"))?;
    let mut e = env(part);
    out["parameters"] = part
        .parameters
        .iter()
        .map(|x| (x.name.clone(), json!({"expr": x.expr, "value": e.value(&x.name).ok()})))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    Ok(out)
}

fn run_sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.sketch";
    let plane = plane_param(cmd, s, p)?;
    let geometry = geometry(cmd, p.get("geometry"))?;
    let constraints = constraints(cmd, p.get("constraints"), geometry.len())?;
    add_feature(s, cmd, p, Kind::Sketch { plane, geometry, constraints })
}

fn run_pad(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.pad";
    let sketch = sketch_ref(cmd, s, p)?;
    let length = expr_param(cmd, p, "length", None)?;
    add_feature(s, cmd, p, Kind::Pad { sketch, length, reversed: bool_or(p, "reversed", false), symmetric: bool_or(p, "symmetric", false) })
}

fn run_pocket(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.pocket";
    let sketch = sketch_ref(cmd, s, p)?;
    let through_all = bool_or(p, "through_all", false);
    let depth = if through_all { String::new() } else { expr_param(cmd, p, "depth", None)? };
    add_feature(s, cmd, p, Kind::Pocket { sketch, depth, through_all, reversed: bool_or(p, "reversed", false) })
}

fn run_revolve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.revolve";
    let sketch = sketch_ref(cmd, s, p)?;
    let angle = expr_param(cmd, p, "angle", Some("360"))?;
    let axis = match str_param(p, "axis").unwrap_or("v").trim().to_ascii_lowercase().as_str() {
        "v" | "y" | "vertical" => "v",
        "h" | "x" | "horizontal" => "h",
        _ => return Err(bad(cmd, "axis is v (the sketch's Y) or h (its X)")),
    };
    add_feature(s, cmd, p, Kind::Revolve { sketch, angle, axis: axis.into(), cut: bool_or(p, "cut", false) })
}

/// Edges picked by points on the part's current body, named by their faces.
fn edge_refs(cmd: &str, s: &Session, p: &Value) -> Result<Vec<[u64; 2]>> {
    let pts: Vec<Vec3> = p.get("edges").and_then(Value::as_array).map(|a| a.iter().take(MAX_EDGES).filter_map(pt3).collect()).unwrap_or_default();
    if pts.is_empty() {
        return Err(bad(cmd, "edges required: [[x, y, z], …] points on the edges"));
    }
    let ps = load(s.doc()?);
    let part = ps.parts.get(part_index(cmd, &ps, p)?).ok_or_else(|| bad(cmd, "no such part"))?;
    let body = rebuild(part).body.ok_or_else(|| bad(cmd, "the part has no body yet"))?;
    let tol = body.mesh_tol();
    pts.iter()
        .map(|q| {
            body.edge_near_tagged(*q, false, tol)
                .map(|(_, t)| t)
                .ok_or_else(|| bad(cmd, format!("no straight edge between two flat faces at {:.4}, {:.4}, {:.4}", q.x, q.y, q.z)))
        })
        .collect()
}

fn run_fillet(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.fillet";
    let radius = expr_param(cmd, p, "radius", None)?;
    let edges = edge_refs(cmd, s, p)?;
    add_feature(s, cmd, p, Kind::Fillet { edges, radius })
}

fn run_chamfer(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.chamfer";
    let distance = expr_param(cmd, p, "distance", None)?;
    let edges = edge_refs(cmd, s, p)?;
    add_feature(s, cmd, p, Kind::Chamfer { edges, distance })
}

/// Change a feature: its sizes, flags, a sketch's geometry and constraints, its name, whether
/// it's suppressed.
fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.edit";
    let mut ps = load(s.doc()?);
    let i = part_index(cmd, &ps, p)?;
    let part = ps.parts.get_mut(i).ok_or_else(|| bad(cmd, "no such part"))?;
    let fi = feature_index(cmd, part, p.get("feature"))?;
    let f = part.features.get_mut(fi).ok_or_else(|| bad(cmd, "no such feature"))?;
    if let Some(n) = str_param(p, "name").map(str::trim).filter(|x| !x.is_empty()) {
        f.name = n.chars().take(80).collect();
    }
    if let Some(b) = p.get("suppressed").and_then(Value::as_bool) {
        f.suppressed = b;
    }
    let set = |k: &str, slot: &mut String| -> Result<()> {
        if p.get(k).is_some() {
            *slot = expr_param(cmd, p, k, None)?;
        }
        Ok(())
    };
    let flag = |k: &str, slot: &mut bool| {
        if let Some(b) = p.get(k).and_then(Value::as_bool) {
            *slot = b;
        }
    };
    match &mut f.kind {
        Kind::Sketch { geometry: g, constraints: c, .. } => {
            if p.get("geometry").is_some() {
                *g = geometry(cmd, p.get("geometry"))?;
            }
            if p.get("constraints").is_some() || p.get("geometry").is_some() {
                let src = p.get("constraints").cloned();
                if let Some(v) = src {
                    *c = constraints(cmd, Some(&v), g.len())?;
                } else if c.iter().flat_map(|x| &x.refs).any(|r| r.handle.0 as usize > g.len()) {
                    return Err(bad(cmd, "the constraints refer to curves the new geometry doesn't have: give constraints too"));
                }
            }
            if p.get("plane").is_some() || p.get("offset").is_some() {
                let plane = match (p.get("plane"), &f.kind) {
                    (None, Kind::Sketch { plane: Plane::Standard { name, .. }, .. }) => json!(name),
                    (v, _) => v.cloned().unwrap_or(json!("xy")),
                };
                let mut q = p.clone();
                q["plane"] = plane;
                let new = plane_param(cmd, s, &q)?;
                if let Some(Feature { kind: Kind::Sketch { plane, .. }, .. }) = part_feature_mut(&mut ps, i, fi) {
                    *plane = new;
                }
            }
        }
        Kind::Pad { length, reversed, symmetric, .. } => {
            set("length", length)?;
            flag("reversed", reversed);
            flag("symmetric", symmetric);
        }
        Kind::Pocket { depth, through_all, reversed, .. } => {
            set("depth", depth)?;
            flag("through_all", through_all);
            flag("reversed", reversed);
        }
        Kind::Revolve { angle, axis, cut, .. } => {
            set("angle", angle)?;
            if let Some(a) = str_param(p, "axis") {
                *axis = if a.trim().eq_ignore_ascii_case("h") { "h".into() } else { "v".into() };
            }
            flag("cut", cut);
        }
        Kind::Fillet { radius, .. } => set("radius", radius)?,
        Kind::Chamfer { distance, .. } => set("distance", distance)?,
    }
    apply(s, &mut ps, i)
}

fn part_feature_mut(ps: &mut Parts, i: usize, fi: usize) -> Option<&mut Feature> {
    ps.parts.get_mut(i)?.features.get_mut(fi)
}

/// Move a feature in the tree. A feature can't move above its sketch, nor a sketch below the
/// features that use it.
fn run_reorder(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.reorder";
    let mut ps = load(s.doc()?);
    let i = part_index(cmd, &ps, p)?;
    let part = ps.parts.get_mut(i).ok_or_else(|| bad(cmd, "no such part"))?;
    let from = feature_index(cmd, part, p.get("feature"))?;
    let to = p
        .get("to")
        .and_then(Value::as_u64)
        .map(|t| (t as usize).min(part.features.len().saturating_sub(1)))
        .ok_or_else(|| bad(cmd, "to required: the new position (0 = top)"))?;
    let f = part.features.remove(from);
    part.features.insert(to, f);
    let pos = |id: u32, fs: &[Feature]| fs.iter().position(|f| f.id == id);
    for (k, f) in part.features.iter().enumerate() {
        if let Some(sk) = f.kind.sketch()
            && pos(sk, &part.features).is_some_and(|sp| sp > k)
        {
            return Err(bad(cmd, format!("{} uses a sketch that would come after it", f.name)));
        }
    }
    apply(s, &mut ps, i)
}

fn run_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "part.delete";
    let mut ps = load(s.doc()?);
    let i = part_index(cmd, &ps, p)?;
    let part = ps.parts.get_mut(i).ok_or_else(|| bad(cmd, "no such part"))?;
    let fi = feature_index(cmd, part, p.get("feature"))?;
    let id = part.features.get(fi).map(|f| f.id).unwrap_or(0);
    if let Some(user) = part.features.iter().find(|f| f.kind.sketch() == Some(id)) {
        return Err(bad(cmd, format!("{} uses it: delete that first", user.name)));
    }
    part.features.remove(fi);
    apply(s, &mut ps, i)
}

fn run_rebuild(s: &mut Session, p: &Value) -> Result<Value> {
    let mut ps = load(s.doc()?);
    let i = part_index("part.rebuild", &ps, p)?;
    apply(s, &mut ps, i)
}

fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let ps = load(s.doc()?);
    let i = part_index("part.tree", &ps, p)?;
    let part = ps.parts.get(i).ok_or_else(|| bad("part.tree", "no such part"))?;
    let built = rebuild(part);
    let mut e = env(part);
    let features: Vec<Value> = part
        .features
        .iter()
        .zip(&built.status)
        .map(|(f, st)| {
            let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
            v["state"] = st["state"].clone();
            v["type"] = st["type"].clone();
            if let Some(er) = st.get("error") {
                v["error"] = er.clone();
            }
            v
        })
        .collect();
    let volume = built.body.as_ref().map(|b| b.props(b.mesh_tol()).volume);
    Ok(json!({
        "part": part.name, "solid": part.solid, "volume": volume,
        "parameters": part.parameters.iter().map(|x| json!({"name": x.name, "expr": x.expr, "value": e.value(&x.name).ok()})).collect::<Vec<_>>(),
        "features": features,
    }))
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let ps = load(s.doc()?);
    Ok(json!({"parts": ps.parts.iter().map(|x| json!({"name": x.name, "features": x.features.len(), "solid": x.solid})).collect::<Vec<_>>()}))
}

fn has_part(s: &Session) -> std::result::Result<(), String> {
    if s.doc().is_ok_and(|d| d.parts.is_some()) { Ok(()) } else { Err("there is no part yet (part.new)".into()) }
}

pub fn specs() -> Vec<CommandSpec> {
    let needs = |c: CommandSpec| c.enabled(has_part);
    vec![
        CommandSpec::new("part.new", "New Part", run_new).params("{name?, parameters?: {name: expr}} → {part}"),
        needs(CommandSpec::new("part.parameter", "Part Parameter", run_parameter)).params("{part?, name, expr: number | expression (null removes)} → the rebuild and every parameter's value"),
        needs(CommandSpec::new("part.sketch", "Part Sketch", run_sketch)).params(
            "{part?, name?, plane?: xy|xz|yz | {origin, normal, x?}, offset?: expr, face?: {point}, geometry: [{type: line, a, b} | {type: arc, center, radius, start°, end°} | {type: circle, center, radius}, construction?], constraints?: [{kind, refs: [{geom, sub?}], value?: expr}]} → {id, features}",
        ),
        needs(CommandSpec::new("part.pad", "Pad", run_pad)).params("{part?, sketch: id|name, length: expr, reversed?, symmetric?, name?} → {id, volume, features}"),
        needs(CommandSpec::new("part.pocket", "Pocket", run_pocket)).params("{part?, sketch, depth?: expr | through_all: true, reversed?, name?} → {id, volume, features}"),
        needs(CommandSpec::new("part.revolve", "Revolve Feature", run_revolve)).params("{part?, sketch, angle?: expr (degrees, 360), axis?: v|h, cut?, name?} → {id, volume, features}"),
        needs(CommandSpec::new("part.fillet", "Fillet Feature", run_fillet)).params("{part?, edges: [[x, y, z]], radius: expr, name?} → {id, volume, features}"),
        needs(CommandSpec::new("part.chamfer", "Chamfer Feature", run_chamfer)).params("{part?, edges: [[x, y, z]], distance: expr, name?} → {id, volume, features}"),
        needs(CommandSpec::new("part.edit", "Edit Feature", run_edit))
            .params("{part?, feature: id|name, name?, suppressed?, length?|depth?|angle?|radius?|distance?: expr, reversed?, symmetric?, through_all?, axis?, cut?, geometry?, constraints?, plane?, offset?} → the rebuild"),
        needs(CommandSpec::new("part.reorder", "Reorder Feature", run_reorder)).params("{part?, feature, to: position} → the rebuild"),
        needs(CommandSpec::new("part.delete", "Delete Feature", run_delete)).params("{part?, feature} → the rebuild"),
        needs(CommandSpec::new("part.rebuild", "Rebuild Part", run_rebuild)).params("{part?} → {solid, volume, features: [{id, name, type, state, error?}]}"),
        needs(CommandSpec::new("part.tree", "Model Tree", run_tree)).params("{part?} → {parameters, features, volume}").noundo(),
        CommandSpec::new("part.list", "List Parts", run_list).params("{} → {parts}").noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_chain_lines_and_arcs() {
        // A slot: two lines and two half arcs, given in a jumbled order and direction.
        let g = vec![
            Geom::Line { a: [0.0, 0.0], b: [4.0, 0.0], construction: false },
            Geom::Arc { center: [0.0, 1.0], radius: 1.0, start: 90f64.to_radians(), end: 270f64.to_radians(), construction: false },
            Geom::Line { a: [4.0, 2.0], b: [0.0, 2.0], construction: false },
            Geom::Arc { center: [4.0, 1.0], radius: 1.0, start: (-90f64).to_radians(), end: 90f64.to_radians(), construction: false },
            Geom::Line { a: [-9.0, -9.0], b: [9.0, 9.0], construction: true },
        ];
        let l = profiles(&g).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].len(), 4);
        assert!(l[0].iter().filter(|v| (v.bulge.abs() - 1.0).abs() < 1e-9).count() == 2);
        assert!(profiles(&[Geom::Line { a: [0.0, 0.0], b: [1.0, 0.0], construction: false }]).is_err());
        assert_eq!(profiles(&[Geom::Circle { center: [0.0, 0.0], radius: 2.0, construction: false }]).unwrap().len(), 1);
    }
}
