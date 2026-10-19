//! 3D object snaps (3DOSNAP): points of objects in space found under the cursor in any view —
//! vertices, midpoints of edges, centres of faces, the nearest point on a face, the foot of the
//! perpendicular to a face — and the face under the cursor that dynamic UCS draws on.
//!
//! The cursor is a point of the view plane (as clicks arrive in 3D views; in the plan view, the
//! plan point): a ray through it along the view finds faces, and a point counts as under the
//! cursor when its image on the view plane is within the aperture.

use cadaddy_doc::{Drawing, EntityKind, Space};
use cadaddy_geom::view3::Projection;
use cadaddy_geom::{Vec2, Vec3};
use serde::Serialize;

/// 3DOSMODE bits.
pub mod mode {
    /// Set: 3D object snaps are off (the others keep which ones come back on).
    pub const OFF: u32 = 1;
    pub const VERTEX: u32 = 2;
    pub const MIDPOINT: u32 = 4;
    pub const CENTER: u32 = 8;
    pub const PERPENDICULAR: u32 = 32;
    pub const NEAREST: u32 = 64;
    pub const ALL: u32 = VERTEX | MIDPOINT | CENTER | PERPENDICULAR | NEAREST;
    /// Every bit, OFF included.
    pub const MASK: u32 = ALL | OFF;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Snap3Kind {
    Vertex,
    Midpoint,
    Center,
    Perpendicular,
    Nearest,
    /// The face under the cursor (dynamic UCS).
    OnFace,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snap3 {
    pub at: Vec3,
    pub kind: Snap3Kind,
    /// The face's normal (faces only).
    pub normal: Option<Vec3>,
}

/// The view a snap looks through: plan (down Z) when there is no projection.
pub fn view_of(p: Option<Projection>) -> Projection {
    p.unwrap_or(Projection { dir: Vec3::new(0.0, 0.0, 1.0), right: Vec3::new(1.0, 0.0, 0.0), up: Vec3::new(0.0, 1.0, 0.0) })
}

/// What an object offers: its vertices, edge midpoints, face centres and surface triangles.
#[derive(Default)]
struct Feats {
    vertices: Vec<Vec3>,
    mids: Vec<Vec3>,
    centers: Vec<(Vec3, Vec3)>,
    tris: Vec<[Vec3; 3]>,
}

/// Most points and triangles one look gathers.
const MAX_FEATS: usize = 2_000_000;

fn gather(d: &Drawing, space: &Space) -> Feats {
    let mut f = Feats::default();
    let Some(store) = d.space(space) else { return f };
    let edge = |f: &mut Feats, a: Vec3, b: Vec3| f.mids.push((a + b) * 0.5);
    for e in store.iter().filter(|e| d.is_visible(e)) {
        if f.tris.len() + f.vertices.len() > MAX_FEATS {
            break;
        }
        match &e.kind {
            EntityKind::Mesh(m) => {
                f.vertices.extend(m.vertices.iter().copied());
                for (a, b) in m.edges() {
                    if let (Some(a), Some(b)) = (m.vertices.get(a as usize), m.vertices.get(b as usize)) {
                        edge(&mut f, *a, *b);
                    }
                }
                for pts in m.face_points() {
                    if pts.len() >= 3 {
                        let c = pts.iter().fold(Vec3::ZERO, |s, p| s + *p) * (1.0 / pts.len() as f64);
                        let n = (pts[1] - pts[0]).cross(pts[2] - pts[0]).normalized();
                        f.centers.push((c, n));
                    }
                }
                f.tris.extend(m.triangles());
            }
            EntityKind::Solid3d(so) => {
                let tol = so.mesh_tol();
                let polys = so.polys(tol);
                for [a, b] in cadaddy_doc::feature_edges(&polys) {
                    f.vertices.extend([a, b]);
                    edge(&mut f, a, b);
                }
                // A flat face's centre: the area-weighted middle of its polygons.
                let mut faces: std::collections::HashMap<u64, (Vec3, f64, Vec3, bool)> = std::collections::HashMap::new();
                for p in polys.iter() {
                    let (_, area, _) = cadaddy_doc::bsp::mass(std::slice::from_ref(p));
                    let c = p.pts.iter().fold(Vec3::ZERO, |s, q| s + *q) * (1.0 / p.pts.len().max(1) as f64);
                    let entry = faces.entry(p.tag).or_insert((Vec3::ZERO, 0.0, p.plane.n, true));
                    entry.0 = entry.0 + c * area;
                    entry.1 += area;
                    entry.3 &= entry.2.dot(p.plane.n) > 1.0 - 1e-9;
                    for k in 1..p.pts.len().saturating_sub(1) {
                        f.tris.push([p.pts[0], p.pts[k], p.pts[k + 1]]);
                    }
                }
                for (_, (sum, area, n, flat)) in faces {
                    if flat && area > 0.0 {
                        f.centers.push((sum * (1.0 / area), n));
                    }
                }
            }
            EntityKind::Face3d(fc) => {
                let [a, b, c, dd] = fc.corners;
                let pts: Vec<Vec3> = if c == dd { vec![a, b, c] } else { vec![a, b, c, dd] };
                f.vertices.extend(pts.iter().copied());
                for i in 0..pts.len() {
                    edge(&mut f, pts[i], pts[(i + 1) % pts.len()]);
                }
                let n = (b - a).cross(c - a).normalized();
                f.centers.push((pts.iter().fold(Vec3::ZERO, |s, p| s + *p) * (1.0 / pts.len() as f64), n));
                f.tris.push([a, b, c]);
                if c != dd {
                    f.tris.push([a, c, dd]);
                }
            }
            k => {
                if let Some((pts, closed)) = k.points3d() {
                    f.vertices.extend(pts.iter().copied());
                    let n = pts.len();
                    for i in 0..if closed { n } else { n.saturating_sub(1) } {
                        edge(&mut f, pts[i], pts[(i + 1) % n]);
                    }
                } else if let EntityKind::Line(l) = k {
                    f.vertices.extend([l.a, l.b]);
                    edge(&mut f, l.a, l.b);
                } else if let EntityKind::Point(p) = k {
                    f.vertices.push(p.p);
                }
            }
        }
    }
    f
}

/// Where the ray from the viewer through `cursor` first meets a surface: the point and the face's
/// normal (turned towards the viewer).
fn ray_hit(tris: &[[Vec3; 3]], v: &Projection, cursor: Vec2) -> Option<(Vec3, Vec3)> {
    let far = tris.iter().flatten().map(|p| p.len()).fold(1.0f64, f64::max) * 4.0;
    let o = v.right * cursor.x + v.up * cursor.y + v.dir * far;
    let d = v.dir * -1.0;
    let mut best: Option<(f64, Vec3)> = None;
    for t in tris {
        let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
        let pv = d.cross(e2);
        let det = e1.dot(pv);
        if det.abs() < 1e-300 {
            continue;
        }
        let inv = 1.0 / det;
        let tv = o - t[0];
        let u = tv.dot(pv) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let qv = tv.cross(e1);
        let w = d.dot(qv) * inv;
        if w < 0.0 || u + w > 1.0 {
            continue;
        }
        let dist = e2.dot(qv) * inv;
        if dist > 0.0 && best.is_none_or(|(b, _)| dist < b) {
            let n = e1.cross(e2).normalized();
            best = Some((dist, if n.dot(v.dir) < 0.0 { n * -1.0 } else { n }));
        }
    }
    best.map(|(t, n)| (o + d * t, n))
}

/// The face under `cursor` (view-plane point) in `space`, seen through `view`.
pub fn face_under(d: &Drawing, space: &Space, view: Option<Projection>, cursor: Vec2) -> Option<(Vec3, Vec3)> {
    ray_hit(&gather(d, space).tris, &view_of(view), cursor)
}

/// The 3D object snap at `cursor` (a view-plane point) within `aperture` (view units) for the
/// `modes` set ([`mode`]), `base` the prompt's base point (for perpendicular), `on_face` adding
/// the face under the cursor as the last resort (dynamic UCS). Vertices come before midpoints,
/// midpoints before centres, and so on; among one kind, the nearest to the cursor, then to the
/// viewer.
pub fn osnap3(
    d: &Drawing,
    space: &Space,
    view: Option<Projection>,
    cursor: Vec2,
    aperture: f64,
    modes: u32,
    base: Option<Vec3>,
    on_face: bool,
) -> Option<Snap3> {
    let modes = if modes & mode::OFF != 0 { 0 } else { modes };
    if modes & mode::ALL == 0 && !on_face {
        return None;
    }
    let v = view_of(view);
    let f = gather(d, space);
    let pick = |pts: &mut dyn Iterator<Item = Vec3>| -> Option<Vec3> {
        let mut best: Option<(f64, f64, Vec3)> = None;
        for p in pts {
            let dd = v.project(p).dist(cursor);
            if dd > aperture {
                continue;
            }
            let depth = p.dot(v.dir);
            let better = match best {
                None => true,
                Some((bd, bz, _)) => dd < bd - aperture * 1e-3 || ((dd - bd).abs() <= aperture * 1e-3 && depth > bz),
            };
            if better {
                best = Some((dd, depth, p));
            }
        }
        best.map(|b| b.2)
    };
    if modes & mode::VERTEX != 0
        && let Some(p) = pick(&mut f.vertices.iter().copied())
    {
        return Some(Snap3 { at: p, kind: Snap3Kind::Vertex, normal: None });
    }
    if modes & mode::MIDPOINT != 0
        && let Some(p) = pick(&mut f.mids.iter().copied())
    {
        return Some(Snap3 { at: p, kind: Snap3Kind::Midpoint, normal: None });
    }
    if modes & mode::CENTER != 0
        && let Some(p) = pick(&mut f.centers.iter().map(|c| c.0))
    {
        let normal = f.centers.iter().find(|c| c.0 == p).map(|c| c.1);
        return Some(Snap3 { at: p, kind: Snap3Kind::Center, normal });
    }
    let hit = ray_hit(&f.tris, &v, cursor);
    if let (Some((p, n)), Some(b)) = (hit, base)
        && modes & mode::PERPENDICULAR != 0
    {
        let foot = b - n * (b - p).dot(n);
        if v.project(foot).dist(cursor) <= aperture * 4.0 {
            return Some(Snap3 { at: foot, kind: Snap3Kind::Perpendicular, normal: Some(n) });
        }
    }
    match hit {
        Some((p, n)) if modes & mode::NEAREST != 0 => Some(Snap3 { at: p, kind: Snap3Kind::Nearest, normal: Some(n) }),
        Some((p, n)) if on_face => Some(Snap3 { at: p, kind: Snap3Kind::OnFace, normal: Some(n) }),
        _ => None,
    }
}
