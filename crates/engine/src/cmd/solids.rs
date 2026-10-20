//! 3D solids: the primitives (BOX, WEDGE, CYLINDER, CONE, SPHERE, TORUS, PYRAMID) and solids
//! made from closed profiles (EXTRUDE, REVOLVE, SWEEP, LOFT, PRESSPULL). Each makes a
//! [`cadaddy_doc::Solid3d`]: CADaddy's own exact solid, drawn from its definition.

mod edits;
mod prompts;

pub(crate) use edits::solid_boolean;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Solid3d, SolidShape, placement};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};
use std::f64::consts::PI;

use super::{CommandSpec, bad, bool_or, size_param, targets};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    let mut v = edits::specs();
    v.extend(vec![
        CommandSpec::new("box", "Box", run_box)
            .menu(&["Draw", "Modeling", "Box"])
            .params("{corner: [x,y,z?], length, width?, height?} | {corner, opposite: [x,y], height} | {center, length, width?, height?} (cube when width/height are left out; a negative height goes down)")
            .interactive(|_| Ok(Box::new(prompts::BoxM::new(false)))),
        CommandSpec::new("wedge", "Wedge", run_wedge)
            .menu(&["Draw", "Modeling", "Wedge"])
            .alias(&["we"])
            .params("{corner: [x,y,z?], length, width?, height?} | {corner, opposite, height} | {center, …} (as BOX; the top slopes down along +X)")
            .interactive(|_| Ok(Box::new(prompts::BoxM::new(true)))),
        CommandSpec::new("cylinder", "Cylinder", run_cylinder)
            .menu(&["Draw", "Modeling", "Cylinder"])
            .alias(&["cyl"])
            .params("{center: [x,y,z?], radius | diameter, height}")
            .interactive(|_| Ok(Box::new(prompts::RoundM::new(prompts::Round::Cylinder)))),
        CommandSpec::new("cone", "Cone", run_cone)
            .menu(&["Draw", "Modeling", "Cone"])
            .params("{center: [x,y,z?], radius | diameter, height, topRadius? (0: a point)}")
            .interactive(|_| Ok(Box::new(prompts::RoundM::new(prompts::Round::Cone)))),
        CommandSpec::new("sphere", "Sphere", run_sphere)
            .menu(&["Draw", "Modeling", "Sphere"])
            .params("{center: [x,y,z?], radius | diameter}")
            .interactive(|_| Ok(Box::new(prompts::RoundM::new(prompts::Round::Sphere)))),
        CommandSpec::new("torus", "Torus", run_torus)
            .menu(&["Draw", "Modeling", "Torus"])
            .alias(&["tor"])
            .params("{center: [x,y,z?], radius | diameter, tube | tubeDiameter}")
            .interactive(|_| Ok(Box::new(prompts::RoundM::new(prompts::Round::Torus)))),
        CommandSpec::new("pyramid", "Pyramid", run_pyramid)
            .menu(&["Draw", "Modeling", "Pyramid"])
            .alias(&["pyr"])
            .params("{center: [x,y,z?], radius, height, sides? (3–32, 4), inscribed? (radius to the corners; else to the edges' midpoints), topRadius?, rotation? (degrees)}")
            .interactive(|_| Ok(Box::new(prompts::RoundM::new(prompts::Round::Pyramid)))),
        CommandSpec::new("extrude", "Extrude", run_extrude)
            .menu(&["Draw", "Modeling", "Extrude"])
            .alias(&["ext"])
            .params("{handles?, height, taper? (degrees)} | {handles?, direction: [dx,dy,dz]} | {handles?, path: handle} — closed profiles (polylines, circles, ellipses, splines, regions); keep?: true keeps them")
            .interactive(|_| Ok(Box::new(prompts::ExtrudeM::default()))),
        CommandSpec::new("revolve", "Revolve", run_revolve)
            .menu(&["Draw", "Modeling", "Revolve"])
            .alias(&["rev"])
            .params("{handles?, axisStart: [x,y], axisEnd: [x,y], angle? (degrees, 360)} — closed profiles on one side of the axis; keep?")
            .interactive(|_| Ok(Box::new(prompts::RevolveM::default()))),
        CommandSpec::new("sweep", "Sweep", run_sweep)
            .menu(&["Draw", "Modeling", "Sweep"])
            .params("{handles?, path: handle} — closed profiles carried along a line, arc, polyline, spline or helix, square to it; keep?")
            .interactive(|_| Ok(Box::new(prompts::SweepM::default()))),
        CommandSpec::new("loft", "Loft", run_loft)
            .menu(&["Draw", "Modeling", "Loft"])
            .params("{handles: [cross sections in order]} — two or more closed curves; keep?")
            .interactive(|_| Ok(Box::new(prompts::LoftM::default()))),
        CommandSpec::new("presspull", "Presspull", run_presspull)
            .menu(&["Draw", "Modeling", "Presspull"])
            .params("{point: [x,y], height} — the closed area around the point, extruded (negative: down)")
            .interactive(|_| Ok(Box::new(prompts::PresspullM::default()))),
    ]);
    v
}

/// Most solids one command makes.
const MAX_SOLIDS: usize = 10_000;

fn added(hs: &[Handle]) -> Result<Value> {
    Ok(json!({ "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(), "handle": hs.first().map(|h| h.hex()) }))
}

/// A point with an optional Z: `[x, y]`, `[x, y, z]`, `{x, y, z?}` or typed text.
pub(crate) fn point3_value(v: &Value) -> Option<Vec3> {
    let z = match v {
        Value::Array(a) => a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
        Value::Object(o) => o.get("z").and_then(Value::as_f64).unwrap_or(0.0),
        _ => 0.0,
    };
    let p = super::point_value(v)?;
    let q = Vec3::new(p.x, p.y, z);
    q.is_finite().then_some(q)
}

fn point3_req(cmd: &str, p: &Value, key: &str) -> Result<Vec3> {
    p.get(key).and_then(point3_value).ok_or_else(|| bad(cmd, format!("`{key}` ([x, y] or [x, y, z]) is required")))
}

/// A signed, non-zero, finite size (a height may be negative).
fn signed_req(cmd: &str, p: &Value, key: &str) -> Result<f64> {
    match p.get(key).and_then(Value::as_f64) {
        Some(v) if v.is_finite() && v != 0.0 && v.abs() <= cadaddy_doc::MAX_SOLID_SIZE => Ok(v),
        Some(_) => Err(bad(cmd, format!("`{key}` must be a non-zero number"))),
        None => Err(bad(cmd, format!("`{key}` (number) is required"))),
    }
}

/// A radius from `radius` or `diameter` (`dkey`).
fn radius_req(cmd: &str, p: &Value, rkey: &str, dkey: &str) -> Result<f64> {
    if let Some(r) = size_param(cmd, p, rkey, false)? {
        return Ok(r);
    }
    size_param(cmd, p, dkey, false)?.map(|d| d / 2.0).ok_or_else(|| bad(cmd, format!("`{rkey}` or `{dkey}` is required")))
}

/// A solid placed with its own origin at `o`, turned `angle` about Z, upside down for a
/// negative height (turned half round X, so it hangs below `o`).
fn placed(shape: SolidShape, o: Vec3, angle: f64, down: bool) -> Solid3d {
    let (s, c) = angle.sin_cos();
    let (x, y) = (Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0));
    let (y, z) = if down { (y * -1.0, Vec3::new(0.0, 0.0, -1.0)) } else { (y, Vec3::new(0.0, 0.0, 1.0)) };
    Solid3d { shape, xf: placement(o, x, y, z) }
}

/// Add solids, refusing definitions that aren't solids (zero or huge sizes).
fn add_solids(s: &mut Session, cmd: &str, solids: Vec<Solid3d>) -> Result<Vec<Handle>> {
    if let Some(bad_one) = solids.iter().find(|so| !so.is_valid()) {
        return Err(bad(cmd, format!("the {} would not be a solid (check its sizes)", bad_one.shape_name().to_ascii_lowercase())));
    }
    solids.into_iter().take(MAX_SOLIDS).map(|so| s.add_entity(EntityKind::Solid3d(Box::new(so)))).collect()
}

// ---------------- primitives ----------------

/// BOX and WEDGE: (corner, length, width, height) from the corner/opposite/center forms.
fn box_dims(cmd: &str, p: &Value) -> Result<(Vec3, f64, f64, f64)> {
    let center = p.get("center").and_then(point3_value);
    let corner = p.get("corner").and_then(point3_value);
    let (mut l, mut w);
    let mut base = match (center, corner) {
        (Some(c), _) => c,
        (None, Some(c)) => c,
        (None, None) => return Err(bad(cmd, "`corner` or `center` is required")),
    };
    if let Some(o) = super::point_param(p, "opposite") {
        l = (o.x - base.x).abs();
        w = (o.y - base.y).abs();
        base.x = base.x.min(o.x);
        base.y = base.y.min(o.y);
    } else {
        l = size_param(cmd, p, "length", false)?.ok_or_else(|| bad(cmd, "`length` or `opposite` is required"))?;
        w = size_param(cmd, p, "width", false)?.unwrap_or(l);
    }
    if l <= 0.0 || w <= 0.0 {
        return Err(bad(cmd, "the base must have a length and width"));
    }
    let h = match p.get("height") {
        None | Some(Value::Null) => l,
        _ => signed_req(cmd, p, "height")?,
    };
    if center.is_some() {
        base = base - Vec3::new(l / 2.0, w / 2.0, h / 2.0);
        l = l.abs();
        w = w.abs();
    }
    if h < 0.0 {
        base.z += h;
    }
    Ok((base, l, w, h.abs()))
}

fn run_box(s: &mut Session, p: &Value) -> Result<Value> {
    let (o, length, width, height) = box_dims("box", p)?;
    added(&add_solids(s, "box", vec![placed(SolidShape::Box { length, width, height }, o, 0.0, false)])?)
}

fn run_wedge(s: &mut Session, p: &Value) -> Result<Value> {
    let (o, length, width, height) = box_dims("wedge", p)?;
    added(&add_solids(s, "wedge", vec![placed(SolidShape::Wedge { length, width, height }, o, 0.0, false)])?)
}

fn run_cylinder(s: &mut Session, p: &Value) -> Result<Value> {
    let c = point3_req("cylinder", p, "center")?;
    let r = radius_req("cylinder", p, "radius", "diameter")?;
    let h = signed_req("cylinder", p, "height")?;
    added(&add_solids(s, "cylinder", vec![placed(SolidShape::Frustum { base_radius: r, top_radius: r, height: h.abs() }, c, 0.0, h < 0.0)])?)
}

fn run_cone(s: &mut Session, p: &Value) -> Result<Value> {
    let c = point3_req("cone", p, "center")?;
    let r = radius_req("cone", p, "radius", "diameter")?;
    let h = signed_req("cone", p, "height")?;
    let top = size_param("cone", p, "topRadius", true)?.unwrap_or(0.0);
    added(&add_solids(s, "cone", vec![placed(SolidShape::Frustum { base_radius: r, top_radius: top, height: h.abs() }, c, 0.0, h < 0.0)])?)
}

fn run_sphere(s: &mut Session, p: &Value) -> Result<Value> {
    let c = point3_req("sphere", p, "center")?;
    let r = radius_req("sphere", p, "radius", "diameter")?;
    added(&add_solids(s, "sphere", vec![placed(SolidShape::Sphere { radius: r }, c, 0.0, false)])?)
}

fn run_torus(s: &mut Session, p: &Value) -> Result<Value> {
    let c = point3_req("torus", p, "center")?;
    let r = radius_req("torus", p, "radius", "diameter")?;
    let t = radius_req("torus", p, "tube", "tubeDiameter")?;
    if t >= r {
        return Err(bad("torus", "the tube radius must be smaller than the torus radius"));
    }
    added(&add_solids(s, "torus", vec![placed(SolidShape::Torus { radius: r, tube: t }, c, 0.0, false)])?)
}

fn run_pyramid(s: &mut Session, p: &Value) -> Result<Value> {
    let c = point3_req("pyramid", p, "center")?;
    let sides = p
        .get("sides")
        .map_or(Some(4), Value::as_u64)
        .filter(|n| (3..=32).contains(n))
        .ok_or_else(|| bad("pyramid", "`sides` must be from 3 to 32"))?;
    let n = sides as f64;
    let r = radius_req("pyramid", p, "radius", "diameter")?;
    let h = signed_req("pyramid", p, "height")?;
    let top = size_param("pyramid", p, "topRadius", true)?.unwrap_or(0.0);
    // The radius reaches the corners (inscribed) or the middles of the edges.
    let to_corner = if bool_or(p, "inscribed", false) { 1.0 } else { 1.0 / (PI / n).cos() };
    // Unturned, an edge lies square to X (a square base is axis-aligned).
    let rot = p.get("rotation").and_then(Value::as_f64).filter(|a| a.is_finite()).unwrap_or(0.0).to_radians() + PI / n;
    let shape = SolidShape::Pyramid { sides: sides as u32, base_radius: r * to_corner, top_radius: top * to_corner, height: h.abs() };
    added(&add_solids(s, "pyramid", vec![placed(shape, c, rot, h < 0.0)])?)
}

// ---------------- profiles ----------------

/// A closed profile: its loops (holes inside) in the XY plane and the height it lies at.
pub(crate) struct Profile {
    pub loops: Vec<Vec<PolyVertex>>,
    pub z: f64,
}

/// The profile of a closed curve object or region; `None` for anything else.
pub(crate) fn profile_of(d: &Drawing, e: &Entity) -> Option<Profile> {
    let z = e.kind.elevation();
    let loops = match &e.kind {
        EntityKind::Region(r) => r.loops.iter().map(|l| l.vertices.clone()).filter(|v| v.len() >= 2).collect(),
        EntityKind::Polyline3d(p)
            if p.closed && p.points.windows(2).all(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| (a.z - b.z).abs() < 1e-9)) =>
        {
            vec![p.points.iter().map(|q| PolyVertex::new(q.xy())).collect()]
        }
        _ => vec![super::hatch::closed_geom(d, e)?.0],
    };
    let z = match &e.kind {
        EntityKind::Polyline3d(p) => p.points.first().map_or(0.0, |q| q.z),
        _ => z,
    };
    let loops: Vec<Vec<PolyVertex>> =
        loops.into_iter().filter(|l| l.len() >= 2 && cadaddy_geom::Polyline { vertices: l.clone(), closed: true }.area().abs() > 1e-18).collect();
    (!loops.is_empty()).then_some(Profile { loops, z })
}

/// The profiles among `hs`, and the objects they came from; an error when there are none.
fn profiles(s: &Session, cmd: &str, hs: &[Handle]) -> Result<Vec<(Handle, Profile)>> {
    let d = s.doc()?;
    let out: Vec<(Handle, Profile)> = hs.iter().filter_map(|h| Some((*h, profile_of(d, d.entity(*h)?)?))).collect();
    if out.is_empty() {
        return Err(bad(cmd, "select closed curves or regions (polylines, circles, ellipses, splines)"));
    }
    Ok(out)
}

/// Erase the profiles the solids were made from, unless `keep`.
fn consume(s: &mut Session, p: &Value, hs: &[Handle]) -> Result<()> {
    if bool_or(p, "keep", false) {
        return Ok(());
    }
    let d = s.doc_mut()?;
    for h in hs {
        d.remove_entity(*h);
    }
    Ok(())
}

/// Loops turned upside down (y and bulges negated), for a solid turned half round X.
fn flipped(loops: &[Vec<PolyVertex>]) -> Vec<Vec<PolyVertex>> {
    loops.iter().map(|l| l.iter().map(|v| PolyVertex { p: Vec2::new(v.p.x, -v.p.y), bulge: -v.bulge, ..*v }).collect()).collect()
}

/// An extrusion of `loops` (at height `z`) by `h` (negative: down), edges tapered by `taper`.
pub(crate) fn extrusion(loops: &[Vec<PolyVertex>], z: f64, h: f64, taper: f64) -> Solid3d {
    let down = h < 0.0;
    let loops = if down { flipped(loops) } else { loops.to_vec() };
    placed(SolidShape::Extrusion { loops, height: h.abs(), taper }, Vec3::new(0.0, 0.0, z), 0.0, down)
}

/// The points along a path object, in drawing coordinates, and its length.
pub(crate) fn path_points(e: &Entity) -> Option<Vec<Vec3>> {
    let z = e.kind.elevation();
    let flat = |pts: Vec<Vec2>| pts.into_iter().map(|q| Vec3::new(q.x, q.y, z)).collect::<Vec<_>>();
    let size = |pts: &[Vec2]| {
        cadaddy_geom::Bounds2::from_points(pts.iter().copied()).width().max(cadaddy_geom::Bounds2::from_points(pts.iter().copied()).height())
    };
    let pts: Vec<Vec3> = match &e.kind {
        EntityKind::Line(l) => vec![l.a, l.b],
        EntityKind::Polyline3d(p) => {
            let mut v = p.points.clone();
            if p.closed {
                v.extend(p.points.first().copied());
            }
            v
        }
        EntityKind::Helix(h) => {
            let n = ((h.turns * 64.0).ceil() as usize).clamp(16, 4000);
            (0..=n).map(|k| h.point_at(k as f64 / n as f64)).collect()
        }
        EntityKind::Spline(sp) => {
            let coarse = sp.tessellate(1.0);
            flat(sp.tessellate((size(&coarse) / 2000.0).max(1e-9)))
        }
        k => {
            let mut out = Vec::new();
            for prim in k.prims() {
                if let cadaddy_doc::Prim::Seg(seg) = prim {
                    let r = match seg {
                        cadaddy_geom::Segment::Arc { arc, .. } => arc.radius,
                        cadaddy_geom::Segment::Line(_) => 0.0,
                    };
                    let mut pts = Vec::new();
                    seg.tessellate((r / 500.0).max(1e-9), &mut pts);
                    let skip = usize::from(out.last().is_some_and(|l: &Vec2| pts.first().is_some_and(|f| l.near(*f, 1e-9))));
                    out.extend(pts.into_iter().skip(skip));
                } else if let cadaddy_doc::Prim::Circle(c) = prim {
                    let mut pts = Vec::new();
                    c.tessellate((c.radius / 500.0).max(1e-9), &mut pts);
                    out.extend(pts);
                } else if let cadaddy_doc::Prim::Ellipse(el) = prim {
                    let mut pts = Vec::new();
                    el.tessellate((el.major.len() / 500.0).max(1e-9), &mut pts);
                    out.extend(pts);
                }
            }
            flat(out)
        }
    };
    let mut pts = pts;
    pts.dedup_by(|a, b| (*a - *b).len() < 1e-12);
    (pts.len() >= 2 && pts.len() <= 4096 && pts.iter().all(|p| p.is_finite())).then_some(pts)
}

fn run_extrude(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    // The path itself is never a profile.
    let path = p.get("path").and_then(Value::as_str).and_then(Handle::parse_hex);
    if let Some(ph) = path {
        let mut q = p.clone();
        if let Some(o) = q.as_object_mut() {
            o.insert("handles".into(), json!(hs.iter().filter(|h| **h != ph).map(|h| h.hex()).collect::<Vec<_>>()));
        }
        return run_sweep(s, &q);
    }
    let profs = profiles(s, "extrude", &hs)?;
    let taper = p.get("taper").and_then(Value::as_f64).unwrap_or(0.0);
    if !taper.is_finite() || taper.abs() >= 90.0 {
        return Err(bad("extrude", "`taper` must be between -90 and 90 degrees"));
    }
    let solids: Vec<Solid3d> = if let Some(dir) = p.get("direction").and_then(point3_value) {
        if dir.z.abs() < 1e-12 {
            return Err(bad("extrude", "the direction must leave the profile's plane"));
        }
        // Along the direction: the extrusion of height 1 sheared onto it.
        profs
            .iter()
            .map(|(_, pr)| {
                // Below the profile: upside down, as for a negative height.
                let lo = if dir.z < 0.0 { flipped(&pr.loops) } else { pr.loops.clone() };
                let y = if dir.z < 0.0 { Vec3::new(0.0, -1.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                let o = Vec3::new(0.0, 0.0, pr.z);
                Solid3d { shape: SolidShape::Extrusion { loops: lo, height: 1.0, taper: 0.0 }, xf: placement(o, Vec3::new(1.0, 0.0, 0.0), y, dir) }
            })
            .collect()
    } else {
        let h = signed_req("extrude", p, "height")?;
        profs.iter().map(|(_, pr)| extrusion(&pr.loops, pr.z, h, taper.to_radians())).collect()
    };
    let made = add_solids(s, "extrude", solids)?;
    consume(s, p, &profs.iter().map(|(h, _)| *h).collect::<Vec<_>>())?;
    added(&made)
}

fn run_revolve(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let a = super::point_req("revolve", p, "axisStart")?;
    let b = super::point_req("revolve", p, "axisEnd")?;
    let axis = (b - a).normalized();
    if axis.len() < 0.5 {
        return Err(bad("revolve", "the axis needs two different points"));
    }
    let angle = p.get("angle").and_then(Value::as_f64).unwrap_or(360.0);
    if !angle.is_finite() || angle == 0.0 || angle.abs() > 360.0 {
        return Err(bad("revolve", "`angle` must be from -360 to 360 degrees, not 0"));
    }
    let profs = profiles(s, "revolve", &hs)?;
    let mut solids = Vec::new();
    for (_, pr) in &profs {
        // The profile's side of the axis is the solid's +X; its Y runs along the axis.
        let side = Vec2::new(-axis.y, axis.x) * -1.0;
        let local = |q: Vec2| Vec2::new((q - a).dot(side), (q - a).dot(axis));
        let xs: Vec<f64> = pr.loops.iter().flatten().map(|v| local(v.p).x).collect();
        let (lo, hi) = xs.iter().fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
        let tiny = 1e-9 * (hi - lo).abs().max(1.0);
        let flip = if lo >= -tiny {
            false
        } else if hi <= tiny {
            true
        } else {
            return Err(bad("revolve", "the profile crosses the axis"));
        };
        let sx = if flip { side * -1.0 } else { side };
        let loops: Vec<Vec<PolyVertex>> = pr
            .loops
            .iter()
            // (side, axis) keeps the plane's turning; (−side, axis) mirrors it.
            .map(|l| {
                l.iter()
                    .map(|v| PolyVertex {
                        p: Vec2::new((v.p - a).dot(sx).max(0.0), (v.p - a).dot(axis)),
                        bulge: if flip { -v.bulge } else { v.bulge },
                        ..*v
                    })
                    .collect()
            })
            .collect();
        let (x0, y3) = (Vec3::new(sx.x, sx.y, 0.0), Vec3::new(axis.x, axis.y, 0.0));
        let z0 = x0.cross(y3);
        // A negative angle: the same turn, started that far back.
        let back = angle.min(0.0).to_radians();
        let (sb, cb) = back.sin_cos();
        let (x3, z3) = (x0 * cb - z0 * sb, x0 * sb + z0 * cb);
        solids.push(Solid3d {
            shape: SolidShape::Revolution { loops, angle: angle.abs().to_radians() },
            xf: placement(Vec3::new(a.x, a.y, pr.z), x3, y3, z3),
        });
    }
    let made = add_solids(s, "revolve", solids)?;
    consume(s, p, &profs.iter().map(|(h, _)| *h).collect::<Vec<_>>())?;
    added(&made)
}

fn run_sweep(s: &mut Session, p: &Value) -> Result<Value> {
    let ph = p
        .get("path")
        .and_then(|v| v.as_str().and_then(Handle::parse_hex).or_else(|| v.as_u64().map(Handle)))
        .ok_or_else(|| bad("sweep", "`path` (a handle) is required"))?;
    let hs: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| *h != ph).collect();
    let path = s
        .doc()?
        .entity(ph)
        .and_then(|e| path_points(e))
        .ok_or_else(|| bad("sweep", "the path must be a line, arc, circle, ellipse, polyline, spline or helix"))?;
    let profs = profiles(s, "sweep", &hs)?;
    let solids = profs
        .iter()
        .map(|(_, pr)| {
            // The profile about its centre, so the path runs through its middle.
            let c = profile_center(&pr.loops);
            let loops = pr.loops.iter().map(|l| l.iter().map(|v| PolyVertex { p: v.p - c, ..*v }).collect()).collect();
            Solid3d::new(SolidShape::Sweep { loops, path: path.clone() })
        })
        .collect();
    let made = add_solids(s, "sweep", solids)?;
    consume(s, p, &profs.iter().map(|(h, _)| *h).collect::<Vec<_>>())?;
    added(&made)
}

/// The middle of a profile's outer loop (its area centroid, or its first point).
fn profile_center(loops: &[Vec<PolyVertex>]) -> Vec2 {
    let Some(outer) = loops.iter().max_by(|a, b| {
        let ar = |l: &Vec<PolyVertex>| cadaddy_geom::Polyline { vertices: l.clone(), closed: true }.area().abs();
        ar(a).total_cmp(&ar(b))
    }) else {
        return Vec2::ZERO;
    };
    let so = Solid3d::new(SolidShape::Extrusion { loops: vec![outer.clone()], height: 1.0, taper: 0.0 });
    let c = so.props(cadaddy_doc::SOLID_PRIM_TOL).centroid;
    if c.is_finite() { Vec2::new(c.x, c.y) } else { outer.first().map_or(Vec2::ZERO, |v| v.p) }
}

fn run_loft(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let mut sections = Vec::new();
    let mut used = Vec::new();
    for h in &hs {
        let Some(pr) = d.entity(*h).and_then(|e| profile_of(d, e)) else { continue };
        let Some(outer) = pr.loops.first() else { continue };
        let pts = cadaddy_geom::Polyline { vertices: outer.clone(), closed: true }.tessellate(loft_tol(outer));
        let mut pts: Vec<Vec3> = pts.into_iter().map(|q| Vec3::new(q.x, q.y, pr.z)).collect();
        if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| (*a - *b).len() < 1e-12) {
            pts.pop();
        }
        sections.push(pts);
        used.push(*h);
    }
    if sections.len() < 2 {
        return Err(bad("loft", "select two or more closed cross sections, in order"));
    }
    let made = add_solids(s, "loft", vec![Solid3d::new(SolidShape::Loft { sections })])?;
    consume(s, p, &used)?;
    added(&made)
}

fn loft_tol(l: &[PolyVertex]) -> f64 {
    let b = cadaddy_geom::Bounds2::from_points(l.iter().map(|v| v.p));
    (b.width().max(b.height()) / 20_000.0).max(1e-9)
}

fn run_presspull(s: &mut Session, p: &Value) -> Result<Value> {
    let at = super::point_req("presspull", p, "point")?;
    let h = signed_req("presspull", p, "height")?;
    let loops = super::hatch::loops_at(s, at).map_err(|_| EngineError::Other("No closed area found there.".into()))?;
    if loops.is_empty() {
        return Err(EngineError::Other("No closed area found there.".into()));
    }
    let d = s.doc()?;
    let z = loops.iter().flat_map(|l| l.sources.iter()).find_map(|h| d.entity(*h)).map_or(0.0, |e| e.kind.elevation());
    let rings: Vec<Vec<PolyVertex>> = loops.into_iter().map(|l| l.vertices).filter(|v| v.len() >= 2).collect();
    added(&add_solids(s, "presspull", vec![extrusion(&rings, z, h, 0.0)])?)
}

/// MASSPROP for solids: volume, surface area, bounding box and centroid of each.
pub(crate) fn massprop_solids(s: &Session, hs: &[Handle]) -> Result<(Vec<Value>, Vec<String>)> {
    let d = s.doc()?;
    let f = |v: f64| super::inquiry::fmt_d(s, v);
    let n = |v: f64| super::inquiry::fmt_n(s, v);
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for h in hs {
        let Some(EntityKind::Solid3d(so)) = d.entity(*h).map(|e| &e.kind) else { continue };
        let m = so.props(so.mesh_tol() / 10.0);
        let c = m.centroid;
        out.push(json!({
            "handle": h.hex(), "type": so.shape_name(), "volume": m.volume, "surfaceArea": m.area,
            "boundingBox": [[m.min.x, m.min.y, m.min.z], [m.max.x, m.max.y, m.max.z]], "centroid": [c.x, c.y, c.z],
        }));
        lines.push(format!(
            "----------------   SOLIDS    ----------------\nMass:                    {}\nVolume:                  {}\nSurface area:            {}\nBounding box:         X: {}  --  {}\n                      Y: {}  --  {}\n                      Z: {}  --  {}\nCentroid:             X: {}\n                      Y: {}\n                      Z: {}",
            n(m.volume),
            n(m.volume),
            n(m.area),
            f(m.min.x),
            f(m.max.x),
            f(m.min.y),
            f(m.max.y),
            f(m.min.z),
            f(m.max.z),
            f(c.x),
            f(c.y),
            f(c.z),
        ));
    }
    Ok((out, lines))
}

/// Set a solid's dimensions from Properties (`height`, `length`, `width`, `radius`,
/// `topRadius`, `tube`, `taper` in degrees); values that would not make a solid are ignored.
pub(crate) fn set_solid_props(so: &mut Solid3d, p: &Value) {
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
    let mut next = so.clone();
    match &mut next.shape {
        SolidShape::Box { length, width, height } | SolidShape::Wedge { length, width, height } => {
            num("length").into_iter().for_each(|v| *length = v);
            num("width").into_iter().for_each(|v| *width = v);
            num("height").into_iter().for_each(|v| *height = v);
        }
        SolidShape::Frustum { base_radius, top_radius, height } => {
            let cyl = (*base_radius - *top_radius).abs() < 1e-12;
            if let Some(r) = num("radius").or(num("baseRadius")) {
                *base_radius = r;
                if cyl {
                    *top_radius = r;
                }
            }
            num("topRadius").into_iter().for_each(|v| *top_radius = v);
            num("height").into_iter().for_each(|v| *height = v);
        }
        SolidShape::Pyramid { base_radius, top_radius, height, .. } => {
            num("radius").or(num("baseRadius")).into_iter().for_each(|v| *base_radius = v);
            num("topRadius").into_iter().for_each(|v| *top_radius = v);
            num("height").into_iter().for_each(|v| *height = v);
        }
        SolidShape::Sphere { radius } => num("radius").into_iter().for_each(|v| *radius = v),
        SolidShape::Torus { radius, tube } => {
            num("radius").into_iter().for_each(|v| *radius = v);
            num("tube").into_iter().for_each(|v| *tube = v);
        }
        SolidShape::Extrusion { height, taper, .. } => {
            num("height").into_iter().for_each(|v| *height = v);
            num("taper").into_iter().for_each(|v| *taper = v.to_radians());
        }
        SolidShape::Revolution { angle, .. } => num("angle").into_iter().for_each(|v| *angle = v.to_radians()),
        SolidShape::Sweep { .. } | SolidShape::Loft { .. } | SolidShape::Boolean { .. } => {}
    }
    if next.is_valid() {
        *so = next;
    }
}

/// The solid's properties as the Properties palette and `properties.get` show them.
pub(crate) fn solid_props_json(so: &Solid3d) -> Value {
    let m = so.props(so.mesh_tol() / 10.0);
    let o = so.origin();
    json!({
        "solidType": so.shape_name(),
        "volume": m.volume,
        "surfaceArea": m.area,
        "centroid": [m.centroid.x, m.centroid.y, m.centroid.z],
        "position": [o.x, o.y, o.z],
    })
}
