//! NURBS surfaces (Q160): PLANESURF, SURFBLEND, SURFOFFSET, SURFTRIM / SURFUNTRIM, SURFEXTEND,
//! SURFPATCH, CVADD / CVREMOVE / CVREBUILD, CVSHOW / CVHIDE, CONVTOSURFACE, and INTERFERE
//! (where solids overlap, as new solids).
//!
//! Methods (L. Piegl and W. Tiller, *The NURBS Book*, 2nd ed., 1997, as in
//! `cadaddy_geom::surface`): a blend is a cubic across the gap whose first and last rows of
//! control vertices are the two edges and whose inner rows follow each surface's tangent out of
//! its edge (positional continuity G0 with straight inner rows; tangent continuity G1 with the
//! tangents, §10.3 skinning); a patch is the bilinearly blended Coons patch of its four edges
//! (S. A. Coons, *Surfaces for Computer-Aided Design of Space Forms*, MIT MAC-TR-41, 1967),
//! interpolated (A9.4); an offset and an extension are interpolated from points moved along the
//! normal or along the tangent out of the edge; CVREBUILD and CVREMOVE are least-squares fits
//! to the surface (§9.4.3), CVADD is knot insertion (A5.3). A trim is a loop of the surface's own
//! parameters (each point of the cutting curve at its closest point on the surface, §6.1), so
//! SURFUNTRIM gives the surface back exactly. INTERFERE intersects each pair of solids with the
//! solid booleans (BSP trees: W. Thibault and B. Naylor, *Set Operations on Polyhedra Using
//! Binary Space Partitioning Trees*, SIGGRAPH 1987).
//!
//! SURFFILLET (a rolling-ball fillet between two surfaces) and CONVTOSOLID (a closed surface or
//! mesh as a solid, which needs a polyhedron solid shape) are not available yet: they say so.

use cadaddy_doc::{BoolOp, Entity, EntityKind, Handle, Solid3d, Surface3d};
use cadaddy_geom::{NurbsSurface, Vec3};
use serde_json::{Value, json};

use super::ask::{Kind, ask, machine};
use super::solids::point3_value;
use super::*;
use crate::{EngineError, Result, Session};

/// Samples along an edge, and across a blend or patch, for the interpolated surfaces.
const EDGE_SAMPLES: usize = 17;
/// Points in a cutting curve or a boundary, at most.
const MAX_POINTS: usize = 4_000;
/// Objects one call works on, at most.
const MAX_OBJECTS: usize = 1_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("planesurf", "Planar Surface", run_planesurf)
            .menu(&["Draw", "Modeling", "Surfaces", "Planar"])
            .params("{corners: [[x,y,z], [x,y,z]] | corner + opposite (a rectangle at the first corner's height) | points: [[x,y,z]…] (a closed planar boundary) | object: hex (a closed polyline, circle or 3D polyline)} → {handle}")
            .interactive(|_| Ok(machine("planesurf", "PLANESURF", [ask("corner", "Specify first corner", Kind::Point), ask("opposite", "Specify other corner", Kind::Point)]))),
        CommandSpec::new("surfblend", "Blend Surface", run_surfblend)
            .menu(&["Draw", "Modeling", "Surfaces", "Blend"])
            .params("{first: hex, first_edge: [x,y,z] (a point near the edge), second: hex, second_edge: [x,y,z], continuity?: g0|g1 (default g1), bulge?: 0.1-2 (default 0.5)} → {handle}")
            .interactive(|_| {
                Ok(machine("surfblend", "SURFBLEND", [
                    ask("first", "Select first surface", Kind::One),
                    ask("first_edge", "Pick a point on its edge to blend", Kind::Point),
                    ask("second", "Select second surface", Kind::One),
                    ask("second_edge", "Pick a point on its edge to blend", Kind::Point),
                ]))
            }),
        CommandSpec::new("surfpatch", "Patch Surface", run_surfpatch)
            .menu(&["Draw", "Modeling", "Surfaces", "Patch"])
            .params("{curves: [[[x,y,z]…] ×4] (four edges round the patch, in order; each a polyline) | points: [[x,y,z]…] | object: hex (one closed planar loop)} → {handle}")
            .interactive(|_| Ok(machine("surfpatch", "SURFPATCH", [ask("object", "Select a closed curve to patch", Kind::One)]))),
        CommandSpec::new("surfoffset", "Offset Surface", run_surfoffset)
            .menu(&["Draw", "Modeling", "Surfaces", "Offset"])
            .params("{handle, distance (along the surface's normal; negative: the other side)} → {handle}")
            .interactive(|_| Ok(machine("surfoffset", "SURFOFFSET", [ask("handle", "Select surface to offset", Kind::One), ask("distance", "Specify offset distance", Kind::Number)]))),
        CommandSpec::new("surfextend", "Extend Surface", run_surfextend)
            .menu(&["Modify", "Surface Editing", "Extend"])
            .params("{handle, edge: [x,y,z] (a point near the edge), distance} → {handle}")
            .interactive(|_| {
                Ok(machine("surfextend", "SURFEXTEND", [
                    ask("handle", "Select surface to extend", Kind::One),
                    ask("edge", "Pick a point on the edge to extend", Kind::Point),
                    ask("distance", "Specify extension distance", Kind::Number),
                ]))
            }),
        CommandSpec::new("surftrim", "Trim Surface", run_surftrim)
            .menu(&["Modify", "Surface Editing", "Trim"])
            .params("{handle, curve: [[x,y,z]…] | object: hex (a closed curve; projected onto the surface), keep?: outside|inside (default outside: the area inside the curve is cut away)} → {handle, trims}")
            .interactive(|_| Ok(machine("surftrim", "SURFTRIM", [ask("handle", "Select surface to trim", Kind::One), ask("object", "Select cutting curve", Kind::One)]))),
        CommandSpec::new("surfuntrim", "Untrim Surface", run_surfuntrim)
            .menu(&["Modify", "Surface Editing", "Untrim"])
            .params("{handle?|handles?} → {untrimmed}")
            .interactive(|_| Ok(machine("surfuntrim", "SURFUNTRIM", [ask("handles", "Select surfaces to untrim", Kind::Select)]))),
        CommandSpec::new("surffillet", "Fillet Surface", run_surffillet)
            .menu(&["Modify", "Surface Editing", "Fillet"])
            .params("{first: hex, second: hex, radius} (not available yet)"),
        CommandSpec::new("cvshow", "Show CV", |s, p| run_cv_shown(s, p, true))
            .menu(&["Modify", "Surface Editing", "Show CV"])
            .params("{handle?|handles?}")
            .interactive(|_| Ok(machine("cvshow", "CVSHOW", [ask("handles", "Select surfaces", Kind::Select)]))),
        CommandSpec::new("cvhide", "Hide CV", |s, p| run_cv_shown(s, p, false))
            .menu(&["Modify", "Surface Editing", "Hide CV"])
            .params("{handle?|handles?}")
            .interactive(|_| Ok(machine("cvhide", "CVHIDE", [ask("handles", "Select surfaces", Kind::Select)]))),
        CommandSpec::new("cvadd", "Add CV", run_cvadd)
            .menu(&["Modify", "Surface Editing", "Add CV"])
            .params("{handle, direction: u|v, at?: parameter (0-1) | point?: [x,y,z] (its closest parameter)} → {count_u, count_v}")
            .interactive(|_| Ok(machine("cvadd", "CVADD", [ask("handle", "Select a surface", Kind::One), ask("point", "Specify a point for the new row of CVs", Kind::Point)]))),
        CommandSpec::new("cvremove", "Remove CV", run_cvremove)
            .menu(&["Modify", "Surface Editing", "Remove CV"])
            .params("{handle, direction: u|v} → {count_u, count_v, deviation} (one row fewer, the surface refitted)")
            .interactive(|_| Ok(machine("cvremove", "CVREMOVE", [ask("handle", "Select a surface", Kind::One)]))),
        CommandSpec::new("cvrebuild", "Rebuild Surface", run_cvrebuild)
            .menu(&["Modify", "Surface Editing", "Rebuild"])
            .params("{handle, count_u, count_v, degree_u? (default 3), degree_v? (default 3)} → {count_u, count_v, deviation}")
            .interactive(|_| {
                Ok(machine("cvrebuild", "CVREBUILD", [
                    ask("handle", "Select a surface to rebuild", Kind::One),
                    ask("count_u", "Number of CVs in U", Kind::Number),
                    ask("count_v", "Number of CVs in V", Kind::Number),
                ]))
            }),
        CommandSpec::new("convtosurface", "Convert to Surface", run_convtosurface)
            .menu(&["Modify", "3D Operations", "Convert to Surface"])
            .params("{handle?|handles?: 3D faces, M×N polygon meshes, closed planar polylines and circles} → {converted, skipped}")
            .interactive(|_| Ok(machine("convtosurface", "CONVTOSURFACE", [ask("handles", "Select objects to convert", Kind::Select)]))),
        CommandSpec::new("convtosolid", "Convert to Solid", run_convtosolid)
            .menu(&["Modify", "3D Operations", "Convert to Solid"])
            .params("{handle?|handles?} (not available yet)"),
        CommandSpec::new("interfere", "Interference Checking", run_interfere)
            .menu(&["Modify", "3D Operations", "Interference Checking"])
            .params("{first: [hex] (solids), second?: [hex] (default: the first set against itself), create?: bool (default true: each overlap as a new solid)} → {pairs: [{a, b, volume, handle?}], volume}")
            .interactive(|_| Ok(machine("interfere", "INTERFERE", [ask("first", "Select first set of objects", Kind::Select)]))),
    ]
}

// ---------------- helpers ----------------

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

fn surface_of(s: &Session, h: Handle) -> Option<Surface3d> {
    match &s.doc().ok()?.entity(h)?.kind {
        EntityKind::Surface(sf) => Some((**sf).clone()),
        _ => None,
    }
}

fn hex(v: &Value) -> Option<Handle> {
    v.as_str().and_then(Handle::parse_hex).or_else(|| v.as_u64().map(Handle))
}

/// The one surface a command works on (`handle`).
fn one_surface(s: &Session, cmd: &str, p: &Value) -> Result<(Handle, Surface3d)> {
    let h = targets(s, p)?.into_iter().find(|h| surface_of(s, *h).is_some()).ok_or_else(|| bad(cmd, "select a surface (`handle`)"))?;
    Ok((h, surface_of(s, h).ok_or_else(|| bad(cmd, "not a surface"))?))
}

fn named_surface(s: &Session, cmd: &str, p: &Value, key: &str) -> Result<(Handle, Surface3d)> {
    let h = p.get(key).and_then(hex).ok_or_else(|| bad(cmd, format!("`{key}` (a surface's handle) is required")))?;
    Ok((h, surface_of(s, h).ok_or_else(|| bad(cmd, format!("`{key}` is not a surface")))?))
}

fn add(s: &mut Session, sf: Surface3d) -> Result<Handle> {
    if !sf.is_valid() {
        return Err(other("The result would not be a valid surface; nothing was made."));
    }
    s.add_entity(EntityKind::Surface(Box::new(sf)))
}

fn store(s: &mut Session, h: Handle, sf: Surface3d) -> Result<()> {
    if !sf.is_valid() {
        return Err(other("The result would not be a valid surface; nothing was changed."));
    }
    s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Surface(Box::new(sf)))?;
    Ok(())
}

fn points_param(cmd: &str, p: &Value, key: &str) -> Result<Option<Vec<Vec3>>> {
    let Some(a) = p.get(key) else { return Ok(None) };
    let a = a.as_array().ok_or_else(|| bad(cmd, format!("`{key}`: an array of points")))?;
    if a.len() > MAX_POINTS {
        return Err(bad(cmd, format!("`{key}`: at most {MAX_POINTS} points")));
    }
    a.iter().map(|v| point3_value(v).ok_or_else(|| bad(cmd, format!("`{key}`: {v} is not a point")))).collect::<Result<Vec<_>>>().map(Some)
}

fn num(cmd: &str, p: &Value, key: &str) -> Result<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` (a number) is required")))
}

/// A closed curve object's points in 3D (a polyline, a 3D polyline, a circle).
fn loop_of(s: &Session, h: Handle) -> Option<Vec<Vec3>> {
    let e = s.doc().ok()?.entity(h)?.clone();
    match &e.kind {
        EntityKind::LwPolyline(pl) if pl.closed => {
            let poly = cadaddy_geom::Polyline { vertices: pl.vertices.clone(), closed: true };
            Some(poly.tessellate(1e-3).into_iter().map(|q| Vec3::new(q.x, q.y, pl.elevation)).collect())
        }
        EntityKind::Polyline3d(pl) if pl.closed => Some(pl.points.clone()),
        EntityKind::Circle(c) => Some(
            (0..64)
                .map(|i| {
                    let a = i as f64 / 64.0 * std::f64::consts::TAU;
                    Vec3::new(c.center.x + c.radius * a.cos(), c.center.y + c.radius * a.sin(), c.center.z)
                })
                .collect(),
        ),
        _ => None,
    }
}

/// The plane of a closed loop (Newell's normal), as an origin and two axes.
fn plane_of(pts: &[Vec3]) -> Option<(Vec3, Vec3, Vec3)> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let mut nrm = Vec3::ZERO;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        nrm = nrm + Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    if nrm.len() < 1e-12 {
        return None;
    }
    let z = nrm.normalized();
    let x = (pts.iter().map(|q| *q - pts[0]).find(|d| d.len() > 1e-9)? - z * (pts.iter().map(|q| *q - pts[0]).find(|d| d.len() > 1e-9)?.dot(z)))
        .normalized();
    let y = z.cross(x);
    Some((pts[0], x, y))
}

/// A planar surface over a closed loop: the rectangle round it in its plane, trimmed to it.
fn planar_over(pts: &[Vec3]) -> Result<Surface3d> {
    let (o, x, y) = plane_of(pts).ok_or_else(|| other("The boundary is not a closed planar loop."))?;
    let z = x.cross(y);
    if pts.iter().any(|q| (*q - o).dot(z).abs() > 1e-6 * (1.0 + (*q - o).len())) {
        return Err(other("The boundary is not planar."));
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for q in pts {
        let (a, b) = ((*q - o).dot(x), (*q - o).dot(y));
        lo = [lo[0].min(a), lo[1].min(b)];
        hi = [hi[0].max(a), hi[1].max(b)];
    }
    let at = |a: f64, b: f64| o + x * a + y * b;
    let nurbs = NurbsSurface::bilinear(at(lo[0], lo[1]), at(hi[0], lo[1]), at(lo[0], hi[1]), at(hi[0], hi[1]))
        .ok_or_else(|| other("The boundary has no area."))?;
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let outer: Vec<[f64; 2]> = pts.iter().map(|q| [((*q - o).dot(x) - lo[0]) / w, ((*q - o).dot(y) - lo[1]) / h]).collect();
    // A loop that is the rectangle itself needs no trim.
    let full = outer.len() == 4
        && outer.iter().all(|p| (p[0].abs() < 1e-9 || (p[0] - 1.0).abs() < 1e-9) && (p[1].abs() < 1e-9 || (p[1] - 1.0).abs() < 1e-9));
    Ok(Surface3d { nurbs, outer: if full { Vec::new() } else { outer }, ..Surface3d::default() })
}

/// The four edges of a surface: points along each and the tangent out of the surface there.
/// Edge 0: u = start, 1: u = end, 2: v = start, 3: v = end.
fn edge(sf: &NurbsSurface, which: usize, n: usize) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
    let (u0, u1) = sf.domain_u();
    let (v0, v1) = sf.domain_v();
    let mut pts = Vec::with_capacity(n);
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let t = k as f64 / (n - 1) as f64;
        let (u, v) = match which {
            0 => (u0, v0 + (v1 - v0) * t),
            1 => (u1, v0 + (v1 - v0) * t),
            2 => (u0 + (u1 - u0) * t, v0),
            _ => (u0 + (u1 - u0) * t, v1),
        };
        let (p, su, sv) = sf.derivs(u, v)?;
        let d = match which {
            0 => su * -1.0,
            1 => su,
            2 => sv * -1.0,
            _ => sv,
        };
        pts.push(p);
        out.push(if d.len() > 1e-12 { d.normalized() } else { Vec3::ZERO });
    }
    Some((pts, out))
}

/// The edge nearest a picked point.
fn edge_near(sf: &NurbsSurface, pick: Vec3) -> Option<usize> {
    (0..4)
        .filter_map(|w| {
            let (pts, _) = edge(sf, w, EDGE_SAMPLES)?;
            let d = pts.windows(2).map(|s| dist_seg(pick, s[0], s[1])).fold(f64::INFINITY, f64::min);
            Some((w, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(w, _)| w)
}

fn dist_seg(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-300 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + d * t)).len()
}

/// Points evenly along a polyline by length.
fn resample(pts: &[Vec3], n: usize) -> Option<Vec<Vec3>> {
    let total: f64 = pts.windows(2).map(|w| (w[1] - w[0]).len()).sum();
    if pts.len() < 2 || !(total.is_finite() && total > 1e-12) {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    let (mut i, mut acc) = (0usize, 0.0);
    for k in 0..n {
        let want = total * k as f64 / (n - 1) as f64;
        while i + 1 < pts.len() - 1 && acc + (pts[i + 1] - pts[i]).len() < want {
            acc += (pts[i + 1] - pts[i]).len();
            i += 1;
        }
        let seg = (pts[i + 1] - pts[i]).len();
        let t = if seg > 1e-300 { ((want - acc) / seg).clamp(0.0, 1.0) } else { 0.0 };
        out.push(pts[i] + (pts[i + 1] - pts[i]) * t);
    }
    Some(out)
}

fn transpose(g: &[Vec<Vec3>]) -> Vec<Vec<Vec3>> {
    let n = g.first().map_or(0, Vec::len);
    (0..n).map(|j| g.iter().filter_map(|r| r.get(j).copied()).collect()).collect()
}

/// A dense grid of the surface for refitting: a few samples per control vertex.
fn dense(sf: &NurbsSurface) -> Option<Vec<Vec<Vec3>>> {
    let n = (sf.count_u.max(sf.count_v) * 3).clamp(12, 60);
    sf.grid(n, n)
}

/// The largest distance between two surfaces over a grid of their parameters.
fn deviation(a: &NurbsSurface, b: &NurbsSurface) -> f64 {
    (0..=16)
        .flat_map(|i| (0..=16).map(move |j| (i as f64 / 16.0, j as f64 / 16.0)))
        .filter_map(|(u, v)| Some((a.point(u, v)? - b.point(u, v)?).len()))
        .fold(0.0, f64::max)
}

// ---------------- commands ----------------

fn run_planesurf(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "planesurf";
    let pair = match (p.get("corner").and_then(point3_value), p.get("opposite").and_then(point3_value)) {
        (Some(a), Some(b)) => Some(vec![a, b]),
        _ => None,
    };
    let sf = if let Some(c) = pair.or(points_param(cmd, p, "corners")?) {
        let (a, b) = match c.as_slice() {
            [a, b] => (*a, *b),
            _ => return Err(bad(cmd, "`corners`: two opposite corners")),
        };
        if (a.x - b.x).abs() < 1e-12 || (a.y - b.y).abs() < 1e-12 {
            return Err(bad(cmd, "the corners must make a rectangle with area"));
        }
        let (x0, x1, y0, y1) = (a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y));
        let z = a.z;
        Surface3d::new(
            NurbsSurface::bilinear(Vec3::new(x0, y0, z), Vec3::new(x1, y0, z), Vec3::new(x0, y1, z), Vec3::new(x1, y1, z))
                .ok_or_else(|| other("bad corners"))?,
        )
    } else if let Some(pts) = points_param(cmd, p, "points")? {
        planar_over(&pts)?
    } else if let Some(h) = p.get("object").and_then(hex) {
        planar_over(&loop_of(s, h).ok_or_else(|| bad(cmd, "`object`: a closed polyline, 3D polyline or circle"))?)?
    } else {
        return Err(bad(cmd, "`corners`, `points` or `object` is required"));
    };
    let h = add(s, sf)?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_surfblend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surfblend";
    let (_, a) = named_surface(s, cmd, p, "first")?;
    let (_, b) = named_surface(s, cmd, p, "second")?;
    let pa = p.get("first_edge").and_then(point3_value).ok_or_else(|| bad(cmd, "`first_edge` (a point near the edge) is required"))?;
    let pb = p.get("second_edge").and_then(point3_value).ok_or_else(|| bad(cmd, "`second_edge` (a point near the edge) is required"))?;
    let g1 = match str_param(p, "continuity").unwrap_or("g1").to_ascii_lowercase().as_str() {
        "g0" => false,
        "g1" => true,
        _ => return Err(bad(cmd, "`continuity`: g0 or g1")),
    };
    let bulge = p.get("bulge").and_then(Value::as_f64).unwrap_or(0.5);
    if !(bulge.is_finite() && (0.1..=2.0).contains(&bulge)) {
        return Err(bad(cmd, "`bulge`: 0.1 to 2"));
    }
    let wa = edge_near(&a.nurbs, pa).ok_or_else(|| other("The first surface has no edge there."))?;
    let wb = edge_near(&b.nurbs, pb).ok_or_else(|| other("The second surface has no edge there."))?;
    let (ea, ta) = edge(&a.nurbs, wa, EDGE_SAMPLES).ok_or_else(|| other("The first edge is degenerate."))?;
    let (mut eb, mut tb) = edge(&b.nurbs, wb, EDGE_SAMPLES).ok_or_else(|| other("The second edge is degenerate."))?;
    // Run both edges the same way.
    let (fa, la, fb, lb) = (ea[0], ea[EDGE_SAMPLES - 1], eb[0], eb[EDGE_SAMPLES - 1]);
    if (fa - lb).len() + (la - fb).len() < (fa - fb).len() + (la - lb).len() {
        eb.reverse();
        tb.reverse();
    }
    let mut rows: Vec<Vec<Vec3>> = (0..4).map(|_| Vec::with_capacity(EDGE_SAMPLES)).collect();
    for k in 0..EDGE_SAMPLES {
        let (pa, pb) = (ea[k], eb[k]);
        let d = (pb - pa).len();
        let (r1, r2) =
            if g1 { (pa + ta[k] * (bulge * d), pb + tb[k] * (bulge * d)) } else { (pa + (pb - pa) * (1.0 / 3.0), pa + (pb - pa) * (2.0 / 3.0)) };
        rows[0].push(pa);
        rows[1].push(r1);
        rows[2].push(r2);
        rows[3].push(pb);
    }
    let nurbs = NurbsSurface::skin(&rows, 3, 3).ok_or_else(|| other("The edges can't be blended (they touch or are degenerate)."))?;
    let h = add(s, Surface3d::new(nurbs))?;
    Ok(json!({ "handle": h.hex(), "continuity": if g1 { "g1" } else { "g0" } }))
}

/// The bilinearly blended Coons patch of four boundary curves sampled `n` each (bottom along u at
/// v = 0, right along v at u = 1, top along u at v = 1, left along v at u = 0).
fn coons(bottom: &[Vec3], right: &[Vec3], top: &[Vec3], left: &[Vec3], n: usize) -> Vec<Vec<Vec3>> {
    let (p00, p10, p01, p11) = (bottom[0], bottom[n - 1], top[0], top[n - 1]);
    (0..n)
        .map(|i| {
            let u = i as f64 / (n - 1) as f64;
            (0..n)
                .map(|j| {
                    let v = j as f64 / (n - 1) as f64;
                    bottom[i] * (1.0 - v) + top[i] * v + left[j] * (1.0 - u) + right[j] * u
                        - (p00 * ((1.0 - u) * (1.0 - v)) + p10 * (u * (1.0 - v)) + p01 * ((1.0 - u) * v) + p11 * (u * v))
                })
                .collect()
        })
        .collect()
}

fn run_surfpatch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surfpatch";
    let pts = match (points_param(cmd, p, "points")?, p.get("object").and_then(hex)) {
        (Some(pts), _) => Some(pts),
        (None, Some(o)) => Some(loop_of(s, o).ok_or_else(|| bad(cmd, "`object`: a closed polyline, 3D polyline or circle"))?),
        _ => None,
    };
    if let Some(pts) = pts {
        let h = add(s, planar_over(&pts)?)?;
        return Ok(json!({ "handle": h.hex() }));
    }
    let curves =
        p.get("curves").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`curves` (four edges) or `points` (a closed loop) is required"))?;
    if curves.len() != 4 {
        return Err(bad(cmd, "`curves`: exactly four edges round the patch"));
    }
    let mut cs: Vec<Vec<Vec3>> = Vec::with_capacity(4);
    for c in curves {
        let pts: Vec<Vec3> =
            c.as_array().ok_or_else(|| bad(cmd, "each curve is an array of points"))?.iter().take(MAX_POINTS).filter_map(point3_value).collect();
        cs.push(resample(&pts, EDGE_SAMPLES).ok_or_else(|| bad(cmd, "each curve needs two points or more and a length"))?);
    }
    // Chain the edges end to end, turning each to start where the last one ended.
    let tol = cs.iter().flatten().map(|q| q.len()).fold(1.0, f64::max) * 1e-6;
    for k in 1..4 {
        let end = cs[k - 1][EDGE_SAMPLES - 1];
        if (cs[k][EDGE_SAMPLES - 1] - end).len() < (cs[k][0] - end).len() {
            cs[k].reverse();
        }
        if (cs[k][0] - end).len() > tol.max(1e-6) * 1e3 {
            return Err(other("The four edges don't meet end to end round the patch."));
        }
    }
    if (cs[3][EDGE_SAMPLES - 1] - cs[0][0]).len() > tol.max(1e-6) * 1e3 {
        return Err(other("The edges don't close round the patch."));
    }
    let bottom = cs[0].clone();
    let right = cs[1].clone();
    let top: Vec<Vec3> = cs[2].iter().rev().copied().collect();
    let left: Vec<Vec3> = cs[3].iter().rev().copied().collect();
    let grid = coons(&bottom, &right, &top, &left, EDGE_SAMPLES);
    let nurbs = NurbsSurface::interpolate(&grid, 3, 3).ok_or_else(|| other("The edges make no surface."))?;
    let h = add(s, Surface3d::new(nurbs))?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_surfoffset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surfoffset";
    let (_, sf) = one_surface(s, cmd, p)?;
    let d = num(cmd, p, "distance")?;
    let nurbs = if sf.nurbs.degree_u == 1 && sf.nurbs.degree_v == 1 && sf.nurbs.count_u == 2 && sf.nurbs.count_v == 2 {
        // A flat patch: its corners along the normal (exact).
        let n = sf.nurbs.normal(0.5, 0.5).ok_or_else(|| other("The surface has no normal."))?;
        let mut t = sf.nurbs.clone();
        t.control.iter_mut().for_each(|q| *q = *q + n * d);
        t
    } else {
        let n = (sf.nurbs.count_u.max(sf.nurbs.count_v) * 2).clamp(8, 40);
        let (u0, u1) = sf.nurbs.domain_u();
        let (v0, v1) = sf.nurbs.domain_v();
        let mut grid = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let mut row = Vec::with_capacity(n + 1);
            for j in 0..=n {
                let (u, v) = (u0 + (u1 - u0) * i as f64 / n as f64, v0 + (v1 - v0) * j as f64 / n as f64);
                let q = sf.nurbs.point(u, v).ok_or_else(|| other("bad surface"))?;
                let nn = sf.nurbs.normal(u, v).ok_or_else(|| other("The surface is degenerate; it can't be offset."))?;
                row.push(q + nn * d);
            }
            grid.push(row);
        }
        NurbsSurface::interpolate(&grid, 3, 3).ok_or_else(|| other("The offset makes no surface."))?
    };
    let h = add(s, Surface3d { nurbs, ..sf })?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_surfextend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surfextend";
    let (h, sf) = one_surface(s, cmd, p)?;
    let pick = p.get("edge").and_then(point3_value).ok_or_else(|| bad(cmd, "`edge` (a point near the edge) is required"))?;
    let dist = num(cmd, p, "distance")?;
    if dist <= 0.0 {
        return Err(bad(cmd, "`distance` must be positive"));
    }
    let which = edge_near(&sf.nurbs, pick).ok_or_else(|| other("No edge there."))?;
    let g = dense(&sf.nurbs).ok_or_else(|| other("bad surface"))?;
    // Work on rows across u: a v edge works on the grid turned round.
    let (mut rows, at_end) = match which {
        0 => (g, false),
        1 => (g, true),
        2 => (transpose(&g), false),
        _ => (transpose(&g), true),
    };
    if !at_end {
        rows.reverse();
    }
    let (edge_pts, tangents) = edge(&sf.nurbs, which, rows.first().map_or(0, Vec::len)).ok_or_else(|| other("degenerate edge"))?;
    let spacing = rows.windows(2).map(|w| w[0].iter().zip(&w[1]).map(|(a, b)| (*b - *a).len()).sum::<f64>() / w[0].len().max(1) as f64).sum::<f64>()
        / rows.len().saturating_sub(1).max(1) as f64;
    let k = ((dist / spacing.max(1e-9)).ceil() as usize).clamp(1, 20);
    for r in 1..=k {
        let f = dist * r as f64 / k as f64;
        rows.push(edge_pts.iter().zip(&tangents).map(|(q, t)| *q + *t * f).collect());
    }
    if !at_end {
        rows.reverse();
    }
    let grid = if which >= 2 { transpose(&rows) } else { rows };
    let nurbs = NurbsSurface::interpolate(&grid, 3, 3).ok_or_else(|| other("The extension makes no surface."))?;
    // The trims were on the old parameters: an extended surface is untrimmed.
    store(s, h, Surface3d { nurbs, outer: Vec::new(), holes: Vec::new(), ..sf })?;
    Ok(json!({ "handle": h.hex(), "edge": which }))
}

fn run_surftrim(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surftrim";
    let (h, mut sf) = one_surface(s, cmd, p)?;
    let pts = match points_param(cmd, p, "curve")? {
        Some(c) => c,
        None => {
            let o = p.get("object").and_then(hex).ok_or_else(|| bad(cmd, "`curve` or `object` (a closed curve) is required"))?;
            loop_of(s, o).ok_or_else(|| bad(cmd, "`object`: a closed polyline, 3D polyline or circle"))?
        }
    };
    if pts.len() < 3 {
        return Err(bad(cmd, "the cutting curve needs three points or more"));
    }
    let uv = sf.to_params(&pts);
    if uv.len() < 3 {
        return Err(other("The curve doesn't project onto the surface."));
    }
    match str_param(p, "keep").unwrap_or("outside").to_ascii_lowercase().as_str() {
        "outside" => {
            if sf.holes.len() >= cadaddy_doc::MAX_TRIM_LOOPS {
                return Err(other("The surface has as many trims as it can hold."));
            }
            sf.holes.push(uv);
        }
        "inside" => sf.outer = uv,
        _ => return Err(bad(cmd, "`keep`: outside or inside")),
    }
    let trims = sf.holes.len() + usize::from(!sf.outer.is_empty());
    store(s, h, sf)?;
    Ok(json!({ "handle": h.hex(), "trims": trims }))
}

fn run_surfuntrim(s: &mut Session, p: &Value) -> Result<Value> {
    let hs: Vec<Handle> = targets(s, p)?.into_iter().take(MAX_OBJECTS).filter(|h| surface_of(s, *h).is_some_and(|sf| sf.is_trimmed())).collect();
    for h in &hs {
        if let Some(sf) = surface_of(s, *h) {
            store(s, *h, sf.untrimmed())?;
        }
    }
    Ok(json!({ "untrimmed": hs.len() }))
}

fn run_surffillet(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(other("SURFFILLET is not available yet. Use SURFBLEND between the two surfaces' edges for now."))
}

fn run_convtosolid(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(other("CONVTOSOLID is not available yet: closed surfaces and meshes can't become solids until solids can hold free-form faces."))
}

fn run_cv_shown(s: &mut Session, p: &Value, shown: bool) -> Result<Value> {
    let hs: Vec<Handle> = targets(s, p)?.into_iter().take(MAX_OBJECTS).filter(|h| surface_of(s, *h).is_some()).collect();
    for h in &hs {
        if let Some(mut sf) = surface_of(s, *h) {
            sf.cvs_shown = shown;
            store(s, *h, sf)?;
        }
    }
    Ok(json!({ "surfaces": hs.len(), "shown": shown }))
}

fn direction(cmd: &str, p: &Value) -> Result<bool> {
    match str_param(p, "direction").unwrap_or("u").to_ascii_lowercase().as_str() {
        "u" => Ok(true),
        "v" => Ok(false),
        _ => Err(bad(cmd, "`direction`: u or v")),
    }
}

fn run_cvadd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cvadd";
    let (h, sf) = one_surface(s, cmd, p)?;
    let along_u = direction(cmd, p)?;
    let t = match (p.get("at").and_then(Value::as_f64), p.get("point").and_then(point3_value)) {
        (Some(t), _) => t,
        (None, Some(q)) => {
            let (u, v) = sf.nurbs.closest(q).ok_or_else(|| other("The point doesn't project onto the surface."))?;
            if along_u { u } else { v }
        }
        _ => 0.5,
    };
    let nurbs = sf
        .nurbs
        .insert_knot(along_u, t)
        .ok_or_else(|| bad(cmd, "`at`: a parameter inside the surface (0-1), and the surface below its vertex limit"))?;
    let (cu, cv) = (nurbs.count_u, nurbs.count_v);
    store(s, h, Surface3d { nurbs, ..sf })?;
    Ok(json!({ "count_u": cu, "count_v": cv }))
}

fn run_cvremove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cvremove";
    let (h, sf) = one_surface(s, cmd, p)?;
    let along_u = direction(cmd, p)?;
    let (cu, cv) = (sf.nurbs.count_u - usize::from(along_u), sf.nurbs.count_v - usize::from(!along_u));
    if cu <= sf.nurbs.degree_u.min(1) || cv <= sf.nurbs.degree_v.min(1) || cu < 2 || cv < 2 {
        return Err(other("The surface has too few control vertices to remove one."));
    }
    let g = dense(&sf.nurbs).ok_or_else(|| other("bad surface"))?;
    let nurbs = NurbsSurface::fit(&g, sf.nurbs.degree_u, sf.nurbs.degree_v, cu, cv).ok_or_else(|| other("The surface can't be refitted."))?;
    let dev = deviation(&sf.nurbs, &nurbs);
    store(s, h, Surface3d { nurbs, ..sf })?;
    Ok(json!({ "count_u": cu, "count_v": cv, "deviation": dev }))
}

fn run_cvrebuild(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cvrebuild";
    let (h, sf) = one_surface(s, cmd, p)?;
    let count = |k: &str| -> Result<usize> {
        p.get(k)
            .and_then(Value::as_f64)
            .filter(|n| n.fract() == 0.0 && (2.0..=100.0).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| bad(cmd, format!("`{k}`: a whole number, 2 to 100")))
    };
    let (cu, cv) = (count("count_u")?, count("count_v")?);
    let deg = |k: &str| p.get(k).and_then(Value::as_u64).map_or(3, |d| (d as usize).clamp(1, 11));
    let g = sf.nurbs.grid((cu * 3).clamp(12, 120), (cv * 3).clamp(12, 120)).ok_or_else(|| other("bad surface"))?;
    let nurbs =
        NurbsSurface::fit(&g, deg("degree_u"), deg("degree_v"), cu, cv).ok_or_else(|| other("The surface can't be rebuilt with those counts."))?;
    let dev = deviation(&sf.nurbs, &nurbs);
    let (ru, rv) = (nurbs.count_u, nurbs.count_v);
    store(s, h, Surface3d { nurbs, ..sf })?;
    Ok(json!({ "count_u": ru, "count_v": rv, "deviation": dev }))
}

fn run_convtosurface(s: &mut Session, p: &Value) -> Result<Value> {
    let hs: Vec<Handle> = targets(s, p)?.into_iter().take(MAX_OBJECTS).collect();
    let (mut converted, mut skipped) = (Vec::new(), 0usize);
    for h in hs {
        let Some(e) = s.doc()?.entity(h).cloned() else { continue };
        let sf = match &e.kind {
            EntityKind::Face3d(f) => {
                let [a, b, c, d] = f.corners;
                NurbsSurface::bilinear(a, b, d, c).map(Surface3d::new)
            }
            EntityKind::Mesh(m) => match m.kind {
                cadaddy_doc::MeshKind::Polygon { m: rows, n: cols, closed_m: false, closed_n: false }
                    if (rows as usize) * (cols as usize) == m.vertices.len() =>
                {
                    NurbsSurface::new(1, 1, rows as usize, cols as usize, m.vertices.clone()).map(Surface3d::new)
                }
                _ => None,
            },
            EntityKind::LwPolyline(_) | EntityKind::Polyline3d(_) | EntityKind::Circle(_) => loop_of(s, h).and_then(|l| planar_over(&l).ok()),
            _ => None,
        };
        match sf.filter(Surface3d::is_valid) {
            Some(sf) => {
                let common = e.common.clone();
                s.doc_mut()?.modify_entity(h, |x| {
                    *x = Entity { handle: x.handle, common, kind: EntityKind::Surface(Box::new(sf)) };
                })?;
                converted.push(h.hex());
            }
            None => skipped += 1,
        }
    }
    Ok(json!({ "converted": converted, "skipped": skipped }))
}

fn run_interfere(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "interfere";
    let set = |k: &str| -> Result<Vec<(Handle, Solid3d)>> {
        let Some(a) = p.get(k) else { return Ok(Vec::new()) };
        let a = a.as_array().ok_or_else(|| bad(cmd, format!("`{k}`: an array of handles")))?;
        Ok(a.iter()
            .take(MAX_OBJECTS)
            .filter_map(hex)
            .filter_map(|h| match &s.doc().ok()?.entity(h)?.kind {
                EntityKind::Solid3d(so) => Some((h, (**so).clone())),
                _ => None,
            })
            .collect())
    };
    let first = set("first")?;
    if first.is_empty() {
        return Err(bad(cmd, "`first`: the solids to check"));
    }
    let second = set("second")?;
    let create = bool_or(p, "create", true);
    let mut pairs: Vec<(usize, usize, bool)> = Vec::new();
    if second.is_empty() {
        for i in 0..first.len() {
            for j in i + 1..first.len() {
                pairs.push((i, j, true));
            }
        }
    } else {
        for (i, (ha, _)) in first.iter().enumerate() {
            for (j, (hb, _)) in second.iter().enumerate() {
                if ha != hb {
                    pairs.push((i, j, false));
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut total = 0.0;
    for (i, j, same) in pairs.into_iter().take(10_000) {
        let (ha, a) = &first[i];
        let (hb, b) = if same { &first[j] } else { &second[j] };
        let both = cadaddy_doc::combine(a.clone(), b.clone(), BoolOp::Intersect);
        let tol = both.mesh_tol();
        if both.polys(tol).is_empty() {
            continue;
        }
        let vol = both.props(tol / 10.0).volume;
        if !(vol.is_finite() && vol > 1e-12) {
            continue;
        }
        total += vol;
        let handle = if create { Some(s.add_entity(EntityKind::Solid3d(Box::new(both)))?.hex()) } else { None };
        out.push(json!({ "a": ha.hex(), "b": hb.hex(), "volume": vol, "handle": handle }));
    }
    Ok(json!({ "pairs": out, "volume": total }))
}
