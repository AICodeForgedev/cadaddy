//! A 3D solid as a mesh: the quads between its stations, sharing their corners, and its caps as
//! single faces (triangulated when they have holes). The shaded view draws it, files store it
//! (other programs see this mesh), and EXPLODE turns a solid into it.

use cadaddy_doc::{Mesh, MeshFace, MeshKind, Solid3d};
use cadaddy_geom::Vec3;
use lyon_tessellation::geometry_builder::{BuffersBuilder, VertexBuffers};
use lyon_tessellation::math::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{FillOptions, FillTessellator, FillVertex, VertexSource};

/// The solid's surfaces to `tol` as one closed mesh, display curves visible and the
/// tessellation's own edges hidden.
pub fn solid_mesh(s: &Solid3d, tol: f64) -> Mesh {
    if matches!(s.shape, cadaddy_doc::SolidShape::Boolean { .. }) {
        return composite_mesh(s, tol);
    }
    let r = s.rings(tol);
    let mut vertices: Vec<Vec3> = Vec::new();
    let mut faces: Vec<MeshFace> = Vec::new();
    // Vertex ids per station, loop and point; a loop shrunk to a point shares one vertex.
    let mut ids: Vec<Vec<Vec<u32>>> = Vec::with_capacity(r.stations.len());
    for st in &r.stations {
        let mut st_ids = Vec::with_capacity(st.len());
        for l in st {
            let first = l.first().copied().unwrap_or(Vec3::ZERO);
            let point = l.iter().all(|p| (*p - first).len() <= 1e-12 * (1.0 + first.len()));
            let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
            if point {
                vertices.push(first);
                st_ids.push(vec![base; l.len()]);
            } else {
                vertices.extend(l.iter().copied());
                st_ids.push((0..l.len()).map(|k| base.saturating_add(u32::try_from(k).unwrap_or(u32::MAX))).collect());
            }
        }
        ids.push(st_ids);
        if vertices.len() > cadaddy_doc::MAX_MESH_VERTICES {
            return Mesh { vertices: Vec::new(), faces: Vec::new(), kind: MeshKind::Subd };
        }
    }
    let n = ids.len();
    let pairs = if r.wrap { n } else { n.saturating_sub(1) };
    let section = |s: usize| r.section_wires.contains(&(s % n.max(1)));
    for s in 0..pairs {
        let (Some(a), Some(b)) = (ids.get(s), ids.get((s + 1) % n)) else { continue };
        for (li, (la, lb)) in a.iter().zip(b).enumerate() {
            let m = la.len().min(lb.len());
            let long = |k: usize| r.long_wires.get(li).and_then(|w| w.get(k)).copied().unwrap_or(false);
            for i in 0..m {
                let j = (i + 1) % m;
                // Corners a_i, a_j, b_j, b_i; each edge visible where it lies on a display curve.
                let corners = [(la[i], section(s)), (la[j], long(j)), (lb[j], section(s + 1)), (lb[i], long(i))];
                let mut v: Vec<(u32, bool)> = Vec::with_capacity(4);
                for &(id, shown) in &corners {
                    // A corner repeating the one before it (an apex or pole) drops out; the edge
                    // leaving it now leaves the one before.
                    match v.last_mut() {
                        Some(last) if last.0 == id => last.1 = shown,
                        _ => v.push((id, shown)),
                    }
                }
                if v.len() > 1 && v.first().map(|f| f.0) == v.last().map(|l| l.0) {
                    v.pop();
                }
                let hidden = v.iter().enumerate().filter(|(_, c)| !c.1).fold(0u64, |h, (k, _)| h | 1 << k);
                let v: Vec<u32> = v.into_iter().map(|c| c.0).collect();
                if v.len() >= 3 {
                    faces.push(MeshFace { v, hidden });
                }
            }
        }
        if faces.len() > cadaddy_doc::MAX_MESH_FACES {
            break;
        }
    }
    if r.caps && n >= 2 {
        for (si, flip) in [(0, true), (n - 1, false)] {
            let (Some(st), Some(st_ids)) = (r.stations.get(si), ids.get(si)) else { continue };
            cap(&mut vertices, &mut faces, st, st_ids, flip);
        }
    }
    // Faces turned outward: a solid's stations can run either way round (and a mirrored placement
    // turns them all), so the enclosed volume decides.
    let mut mesh = Mesh { vertices, faces, kind: MeshKind::Subd };
    let volume: f64 = mesh.triangles().iter().map(|[a, b, c]| a.dot(b.cross(*c))).sum();
    if volume < 0.0 {
        for f in &mut mesh.faces {
            let n = f.v.len();
            f.v.reverse();
            // Edge k of the reversed face is edge n − 2 − k of the original.
            let old = f.hidden;
            f.hidden = (0..n.min(64)).filter(|k| old & (1 << ((2 * n - 2 - k) % n)) != 0).fold(0, |h, k| h | 1 << k);
        }
    }
    mesh
}

/// A composite solid's polygons, their corners welded, the edges between faces visible.
fn composite_mesh(s: &Solid3d, tol: f64) -> Mesh {
    let polys = s.polys(tol);
    let scale = polys.iter().flat_map(|p| p.pts.iter()).fold(1.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs()));
    let quantum = scale * 1e-9;
    let key = |p: Vec3| [(p.x / quantum).round() as i64, (p.y / quantum).round() as i64, (p.z / quantum).round() as i64];
    let mut ids: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut vertices: Vec<Vec3> = Vec::new();
    let mut faces: Vec<MeshFace> = Vec::new();
    let features = cadaddy_doc::feature_edges(&polys);
    // An edge is visible where it lies along a feature segment.
    let on_feature = |a: Vec3, b: Vec3| {
        let m = (a + b) * 0.5;
        features.iter().any(|[p, q]| {
            let d = *q - *p;
            let l2 = d.dot(d);
            if l2 <= 0.0 {
                return false;
            }
            let t = (m - *p).dot(d) / l2;
            let near = |x: Vec3| {
                let tt = (x - *p).dot(d) / l2;
                (*p + d * tt - x).len() <= quantum * 100.0
            };
            (-1e-9..=1.0 + 1e-9).contains(&t) && near(a) && near(b)
        })
    };
    for p in polys.iter() {
        if vertices.len() > cadaddy_doc::MAX_MESH_VERTICES || faces.len() > cadaddy_doc::MAX_MESH_FACES {
            break;
        }
        let mut v: Vec<u32> = Vec::with_capacity(p.pts.len());
        for q in &p.pts {
            let id = *ids.entry(key(*q)).or_insert_with(|| {
                vertices.push(*q);
                u32::try_from(vertices.len() - 1).unwrap_or(u32::MAX)
            });
            if v.last() != Some(&id) {
                v.push(id);
            }
        }
        if v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        if v.len() < 3 {
            continue;
        }
        let n = p.pts.len();
        let hidden = (0..n.min(64)).filter(|&k| !on_feature(p.pts[k], p.pts[(k + 1) % n])).fold(0u64, |h, k| h | 1 << k);
        let hidden = if v.len() == n { hidden } else { 0 };
        faces.push(MeshFace { v, hidden });
    }
    Mesh { vertices, faces, kind: MeshKind::Subd }
}

/// One planar end: a single face for one loop, triangles for a profile with holes. `flip`
/// reverses it (the first station faces backwards).
fn cap(vertices: &mut Vec<Vec3>, faces: &mut Vec<MeshFace>, st: &[Vec<Vec3>], ids: &[Vec<u32>], flip: bool) {
    // Loops that shrank to a point close nothing.
    let live: Vec<usize> = (0..st.len()).filter(|&l| ids.get(l).is_some_and(|v| v.len() >= 3 && v.first() != v.get(1))).collect();
    if let [only] = live.as_slice() {
        let Some(v) = ids.get(*only) else { return };
        let mut v = v.clone();
        if flip {
            v.reverse();
        }
        faces.push(MeshFace { v, hidden: 0 });
        return;
    }
    if live.is_empty() {
        return;
    }
    // In the cap's plane (about its first point, so f32 keeps the precision), even-odd triangles
    // whose corners are the loop points themselves.
    let mut nrm = Vec3::ZERO;
    for &l in &live {
        let Some(pts) = st.get(l) else { continue };
        let m = pts.len();
        for i in 0..m {
            nrm = nrm + pts[i].cross(pts[(i + 1) % m]);
        }
    }
    let nrm = nrm.normalized();
    let Some(o) = live.first().and_then(|&l| st.get(l)).and_then(|l| l.first()).copied() else { return };
    if nrm.len() < 0.5 {
        return;
    }
    let (ax, ay) = cadaddy_doc::frame_for(nrm);
    let mut path = Path::builder();
    // Each path endpoint's mesh vertex and point, by the id the path gave it (closing a contour
    // adds an endpoint of its own: its first point again).
    let mut endpoints: Vec<Option<(u32, Vec3)>> = Vec::new();
    let mut set = |id: lyon_tessellation::path::EndpointId, v: (u32, Vec3)| {
        let k = id.to_usize();
        if endpoints.len() <= k {
            endpoints.resize(k + 2, None);
        }
        if let Some(slot) = endpoints.get_mut(k) {
            *slot = Some(v);
        }
        if let Some(slot) = endpoints.get_mut(k + 1) {
            slot.get_or_insert(v);
        }
    };
    for &l in &live {
        let (Some(pts), Some(id)) = (st.get(l), ids.get(l)) else { continue };
        let mut first = None;
        for (k, (p, i)) in pts.iter().zip(id).enumerate() {
            let q = *p - o;
            let at = point(q.dot(ax) as f32, q.dot(ay) as f32);
            let eid = if k == 0 { path.begin(at) } else { path.line_to(at) };
            set(eid, (*i, *p));
            first.get_or_insert((*i, *p));
            // The slot after the last endpoint is where the contour closes.
            if k + 1 == pts.len()
                && let Some(f) = first
            {
                let next = lyon_tessellation::path::EndpointId(eid.0.saturating_add(1));
                set(next, f);
            }
        }
        path.end(true);
    }
    let endpoints = endpoints;
    let mut out: VertexBuffers<u32, u32> = VertexBuffers::new();
    let extra: std::cell::RefCell<Vec<Vec3>> = std::cell::RefCell::new(Vec::new());
    let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let path = path.build();
    let result = FillTessellator::new().tessellate_with_ids(
        path.id_iter(),
        &path,
        None,
        &FillOptions::even_odd(),
        &mut BuffersBuilder::new(&mut out, |v: FillVertex<'_>| {
            let mut found = None;
            for src in v.sources() {
                match src {
                    VertexSource::Endpoint { id } => found = endpoints.get(id.to_usize()).copied().flatten().map(|e| e.0),
                    VertexSource::Edge { from, to, t } => {
                        let at = |e: lyon_tessellation::path::EndpointId| endpoints.get(e.to_usize()).copied().flatten().map(|e| e.1);
                        if let (Some(a), Some(b)) = (at(from), at(to)) {
                            let mut x = extra.borrow_mut();
                            found = Some(base.saturating_add(u32::try_from(x.len()).unwrap_or(u32::MAX)));
                            x.push(a + (b - a) * f64::from(t));
                        }
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            found.unwrap_or(u32::MAX)
        }),
    );
    if result.is_err() {
        return;
    }
    vertices.extend(extra.into_inner());
    let count = vertices.len();
    let (tris, _) = out.indices.as_chunks::<3>();
    for t in tris {
        let v: Vec<u32> = t.iter().filter_map(|&k| out.vertices.get(k as usize).copied()).collect();
        let [a, b, c] = v.as_slice() else { continue };
        if [a, b, c].iter().any(|&&i| i as usize >= count) || a == b || b == c || a == c {
            continue;
        }
        // Facing along the cap's normal (or against it for the first station).
        let (Some(pa), Some(pb), Some(pc)) = (vertices.get(*a as usize), vertices.get(*b as usize), vertices.get(*c as usize)) else { continue };
        let along = (*pb - *pa).cross(*pc - *pa).dot(nrm) > 0.0;
        let v = if along != flip { vec![*a, *b, *c] } else { vec![*a, *c, *b] };
        faces.push(MeshFace { v, hidden: 0b111 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{SolidShape, placement};

    fn volume(m: &Mesh) -> f64 {
        m.triangles().iter().map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0).sum()
    }

    /// A 6 × 6 plate with a 2 × 2 hole, 1 thick.
    fn plate() -> Solid3d {
        let sq = |a: f64, b: f64| {
            vec![
                cadaddy_geom::PolyVertex::new(cadaddy_geom::Vec2::new(a, a)),
                cadaddy_geom::PolyVertex::new(cadaddy_geom::Vec2::new(b, a)),
                cadaddy_geom::PolyVertex::new(cadaddy_geom::Vec2::new(b, b)),
                cadaddy_geom::PolyVertex::new(cadaddy_geom::Vec2::new(a, b)),
            ]
        };
        Solid3d::new(SolidShape::Extrusion { loops: vec![sq(0.0, 6.0), sq(2.0, 4.0)], height: 1.0, taper: 0.0 })
    }

    #[test]
    fn meshes_are_closed_and_face_outward() {
        let mut mirrored = Solid3d::new(SolidShape::Box { length: 1.0, width: 2.0, height: 3.0 });
        mirrored.transform(&placement(Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        for (s, v) in [
            (Solid3d::new(SolidShape::Torus { radius: 3.0, tube: 1.0 }), 6.0 * std::f64::consts::PI.powi(2)),
            (Solid3d::new(SolidShape::Frustum { base_radius: 1.0, top_radius: 0.0, height: 3.0 }), std::f64::consts::PI),
            (Solid3d::new(SolidShape::Sphere { radius: 1.0 }), 4.0 / 3.0 * std::f64::consts::PI),
            (mirrored, 6.0),
            (plate(), 32.0),
        ] {
            let m = solid_mesh(&s, 1e-3);
            assert!((volume(&m) - v).abs() < 1e-2 * v, "{}: {}", s.shape_name(), volume(&m));
            // Every edge is shared by exactly two faces, running opposite ways.
            let mut half: std::collections::HashMap<(u32, u32), i32> = std::collections::HashMap::new();
            for f in &m.faces {
                for k in 0..f.v.len() {
                    *half.entry((f.v[k], f.v[(k + 1) % f.v.len()])).or_default() += 1;
                }
            }
            assert!(half.iter().all(|((a, b), n)| *n == 1 && half.get(&(*b, *a)) == Some(&1)), "{} is not closed", s.shape_name());
        }
        // Only the display curves are visible edges: a box shows its 12.
        assert_eq!(solid_mesh(&Solid3d::new(SolidShape::Box { length: 1.0, width: 1.0, height: 1.0 }), 0.01).edges().len(), 12);
    }
}
