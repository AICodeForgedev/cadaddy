//! A drawing brought down to what an AutoCAD Release 12 DXF file can hold, before it is written
//! as R12 (SAVEAS version 12). R12 knows lines, arcs, circles, points, text, attribute
//! definitions, block references, 2D solids, traces, 3D faces and heavy polylines (2D, 3D,
//! polyface and polygon meshes); everything newer becomes those, as AutoCAD does when it saves
//! R12: multiline text becomes text lines, dimensions, tables, tolerances, multilines and
//! regions explode, ellipses, splines and helixes become polylines through their points, pattern
//! hatches become their lines and solid ones their outline, leaders a polyline, 3D solids and
//! subdivision meshes 3D faces. What R12 has no form for (xlines, rays, images, underlays,
//! wipeouts, multileaders, lights, section planes, proxies) is left out and counted.
//! Lightweight polylines stay: the R12 writer turns them into heavy ones.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, EntityStore, Handle};
use cadaddy_geom::{PolyVertex, Vec2};

use crate::cmd::helpers::lwpoly;

/// How deep a piece's own pieces are brought down (a dimension's text, a table's).
const MAX_DEPTH: usize = 4;
/// Most points one curve becomes.
const MAX_POINTS: usize = 4096;

/// `d` with only what R12 holds, and what was left out by type name.
pub(crate) fn downgrade(d: &Drawing) -> (Drawing, BTreeMap<String, usize>) {
    let mut out = d.clone();
    let mut lost = BTreeMap::new();
    let mut next = d.handseed;
    out.model = store(d, &d.model, &mut lost, &mut next);
    for l in &mut out.layouts {
        l.entities = store(d, &l.entities, &mut lost, &mut next);
    }
    for b in out.blocks.values_mut() {
        if b.xref_path.is_some() {
            continue;
        }
        let ents = store(d, &b.entities, &mut lost, &mut next);
        Arc::make_mut(b).entities = ents;
    }
    out.handseed = next;
    (out, lost)
}

fn store(d: &Drawing, s: &EntityStore, lost: &mut BTreeMap<String, usize>, next: &mut u64) -> EntityStore {
    let mut out = EntityStore::new();
    for e in s.iter() {
        let keep_handle = e.handle;
        let parts = piece(d, e, lost, 0);
        let single = parts.len() == 1;
        for mut p in parts {
            // An object that stays one object keeps its handle; pieces get new ones.
            p.handle = if single {
                keep_handle
            } else {
                *next = next.saturating_add(1);
                Handle(*next - 1)
            };
            out.push(p);
        }
    }
    out
}

/// What `e` becomes in R12.
fn piece(d: &Drawing, e: &Entity, lost: &mut BTreeMap<String, usize>, depth: usize) -> Vec<Entity> {
    let with = |kind: EntityKind| Entity { handle: Handle(0), common: e.common.clone(), kind };
    let drop = |lost: &mut BTreeMap<String, usize>| {
        *lost.entry(e.kind.type_name().to_string()).or_default() += 1;
        Vec::new()
    };
    match &e.kind {
        EntityKind::Line(_)
        | EntityKind::Point(_)
        | EntityKind::Circle(_)
        | EntityKind::Arc(_)
        | EntityKind::Text(_)
        | EntityKind::AttDef(_)
        | EntityKind::Solid(_)
        | EntityKind::Trace(_)
        | EntityKind::Face3d(_)
        | EntityKind::LwPolyline(_)
        | EntityKind::Polyline3d(_)
        | EntityKind::Viewport(_) => vec![e.clone()],
        EntityKind::Mesh(m) if !matches!(m.kind, cadaddy_doc::MeshKind::Subd) => vec![e.clone()],
        EntityKind::Insert(_) if super::super::array::explode_array(d, e).is_none() => vec![e.clone()],
        EntityKind::Ellipse(el) => {
            let g = cadaddy_geom::Ellipse { center: el.center.xy(), major: el.major.xy(), ratio: el.ratio, start: el.start, end: el.end };
            let mut pts = Vec::new();
            g.tessellate(tol(el.major.xy().len()), &mut pts);
            polyline(pts, g.is_full(), el.center.z).map(with).into_iter().collect()
        }
        EntityKind::Spline(sp) => {
            let pts = sp.tessellate(tol(extent(&sp.control)));
            polyline(pts, sp.closed, 0.0).map(with).into_iter().collect()
        }
        EntityKind::Leader(l) => {
            let z = l.vertices.first().map_or(0.0, |p| p.z);
            polyline(l.vertices.iter().map(|p| p.xy()).collect(), false, z).map(with).into_iter().collect()
        }
        EntityKind::Hatch(h) => {
            let loops = cadaddy_render::hatch_fill_loops(h, 0.0);
            let lines = if h.solid || h.gradient.is_some() { loops.clone() } else { cadaddy_render::hatch_pattern_lines(d, h, &loops, 0.0, None) };
            let closed = h.solid || h.gradient.is_some();
            if closed {
                *lost.entry("Hatch fill".to_string()).or_default() += 1;
            }
            lines.into_iter().filter_map(|l| polyline(l, closed, h.elevation)).map(with).collect()
        }
        _ if depth < MAX_DEPTH => match super::super::modify::explode_kind(d, e) {
            Some(parts) => parts.iter().flat_map(|p| piece(d, p, lost, depth + 1)).collect(),
            None => drop(lost),
        },
        _ => drop(lost),
    }
}

/// A chord tolerance for a curve about `size` across.
fn tol(size: f64) -> f64 {
    if size.is_finite() && size > 0.0 { size * 1e-3 } else { 1e-3 }
}

fn extent(pts: &[Vec2]) -> f64 {
    let b = cadaddy_geom::Bounds2::from_points(pts.iter().copied());
    if b.is_empty() { 0.0 } else { b.width().hypot(b.height()) }
}

/// A polyline through `pts` (thinned to [`MAX_POINTS`]); `None` with fewer than two points.
fn polyline(pts: Vec<Vec2>, closed: bool, z: f64) -> Option<EntityKind> {
    let pts: Vec<Vec2> = pts.into_iter().filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
    if pts.len() < 2 {
        return None;
    }
    let step = pts.len().div_ceil(MAX_POINTS).max(1);
    let mut vs: Vec<PolyVertex> = pts.iter().step_by(step).map(|p| PolyVertex::new(*p)).collect();
    if let (Some(last), true) = (pts.last(), step > 1) {
        vs.push(PolyVertex::new(*last));
    }
    let mut k = lwpoly(vs, closed);
    if let EntityKind::LwPolyline(p) = &mut k {
        p.elevation = z;
    }
    Some(k)
}
