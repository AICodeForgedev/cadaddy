//! Associative path arrays: ARRAYPATH keeps the path (a copy in the array's coordinates, and
//! the curve's handle) so the items follow when the curve is edited, and ARRAYEDIT can change
//! the count, spacing, method and alignment later.

use std::sync::Arc;

use cadaddy_doc::{ArrayKind, Drawing, EntityKind, Handle, PathArray};
use cadaddy_geom::{Mat3, PolyVertex, Polyline, Segment, Vec2};

use super::super::curves::Chain;
use super::*;

/// A curve as a polyline with bulges (lines and arcs exact; ellipses and splines tessellated).
pub(crate) fn path_polyline(k: &EntityKind) -> Option<Polyline> {
    let c = Chain::of(k)?;
    let mut vertices = Vec::new();
    let vertex = |p: Vec2, bulge: f64| PolyVertex { p, bulge, ..PolyVertex::default() };
    for seg in &c.segs {
        match seg {
            Segment::Line(_) => vertices.push(vertex(seg.start(), 0.0)),
            Segment::Arc { arc, ccw } => {
                let sweep = arc.sweep();
                let sign = if *ccw { 1.0 } else { -1.0 };
                // A bulge describes at most a half circle well: longer arcs go in two halves.
                if sweep > std::f64::consts::PI {
                    vertices.push(vertex(seg.start(), sign * cadaddy_geom::arc_to_bulge(sweep / 2.0)));
                    vertices.push(vertex(seg.at(0.5), sign * cadaddy_geom::arc_to_bulge(sweep / 2.0)));
                } else {
                    vertices.push(vertex(seg.start(), sign * cadaddy_geom::arc_to_bulge(sweep)));
                }
            }
        }
        if vertices.len() > cadaddy_doc::MAX_PATH_VERTICES {
            return None;
        }
    }
    if !c.closed {
        vertices.push(vertex(c.end()?, 0.0));
    }
    let p = Polyline { vertices, closed: c.closed };
    (p.vertices.len() >= 2 && p.len() > 1e-12).then_some(p)
}

/// `path` (world) in the coordinates of a block reference placed by `to_world`.
fn local_path(path: &EntityKind, to_world: &Mat3) -> Option<Polyline> {
    let inv = to_world.inverse()?;
    let mut k = path.clone();
    k.transform(&inv);
    path_polyline(&k)
}

/// Make `objs` an associative path array along curve `curve`: `count` items spread over the
/// whole path (Divide), or items every `spacing` (Measure).
pub(crate) fn make_path(s: &mut Session, objs: &[Handle], curve: Handle, count: Option<usize>, spacing: Option<f64>, align: bool) -> Result<Value> {
    let d = s.doc()?;
    let pk = d.entity(curve).map(|e| e.kind.clone()).ok_or_else(|| other("No such path curve."))?;
    let world = path_polyline(&pk).ok_or_else(|| other("Select a line, arc, circle, polyline, ellipse or spline as the path."))?;
    let len = world.len();
    let objs: Vec<Handle> = objs.iter().copied().filter(|h| *h != curve).collect();
    if objs.is_empty() {
        return Err(other("Select objects to array."));
    }
    let probe = PathArray {
        curve: Some(curve),
        path: world.clone(),
        items: 1,
        spacing: 0.0,
        divide: true,
        align,
        ref_angle: 0.0,
        rows: 1,
        row_spacing: 0.0,
        row_elevation: 0.0,
    };
    let (start, tangent) = probe.at_length(0.0).ok_or_else(|| other("The path has no length."))?;
    let (items, spacing, divide) = match (count, spacing) {
        (_, Some(sp)) if sp.is_finite() && sp > 0.0 => {
            let n = (len / sp + 1e-9).floor();
            let n = if n.is_finite() && n < f64::from(u32::MAX - 1) { n as u64 + 1 } else { u64::MAX };
            (n, sp, false)
        }
        (_, Some(_)) => return Err(other("Requires a spacing greater than 0.")),
        (Some(n), _) => (n.max(1) as u64, 0.0, true),
        _ => (6, 0.0, true),
    };
    check_size(s, items, objs.len())?;
    // The array's base point is the path's start: the source objects stay where they are.
    let path = local_path(&pk, &Mat3::translate(start)).ok_or_else(|| other("The path has no length."))?;
    let kind = ArrayKind::Path(PathArray { items: count32(items), spacing, divide, ref_angle: tangent.angle(), path, ..probe });
    let p = params(kind, 1, 0.0);
    if !p.is_valid() {
        return Err(other("The path array's parameters are not valid."));
    }
    let n = p.items().map_or(0, |v| v.len());
    let h = make_assoc(s, &objs, start, p)?;
    Ok(json!({ "handle": h.hex(), "created": (n.saturating_sub(1)).saturating_mul(objs.len()), "associative": true, "items": n }))
}

/// After a command: path arrays whose path curve changed are rebuilt along it.
pub(crate) fn after_command(s: &mut Session, before: &Arc<Drawing>) {
    let Ok(d) = s.doc() else { return };
    let mut work = Vec::new();
    let spaces = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities));
    for e in spaces.flat_map(|sp| sp.iter_kinds(cadaddy_doc::KIND_INSERT)) {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        let Some(b) = d.block(&ins.block) else { continue };
        let Some(ArrayKind::Path(pa)) = b.array.as_ref().map(|a| &a.kind) else { continue };
        let Some(c) = pa.curve else { continue };
        let Some(now) = d.entity(c) else { continue };
        let changed = match before.entity(c) {
            Some(was) => !Arc::ptr_eq(was, now) && was.kind != now.kind,
            None => false,
        };
        // A moved or rotated array whose path did not change keeps its items.
        if changed {
            work.push((e.handle, now.kind.clone(), ins.transform(b.base.xy())));
        }
    }
    for (h, curve, to_world) in work {
        let Some(path) = local_path(&curve, &to_world) else { continue };
        let Ok(d) = s.doc() else { return };
        let Ok((_, _, mut p)) = super::edit::array_of(d, h) else { continue };
        if let ArrayKind::Path(pa) = &mut p.kind {
            pa.path = path;
        }
        if let Err(e) = super::edit::rebuild(s, h, p, None) {
            s.echo(format!("The path array could not follow its path: {e}"));
        }
    }
}
