//! Ray picking against the scene's triangles.

use crate::derive::{Owner, Scene};
use crate::math::{V3, v3};
use cadaddy_doc::Handle;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The entity hit, and the drawing of the set it came from.
    pub owner: Owner,
    /// Distance along the ray, feet.
    pub t: f64,
    pub point: V3,
}

/// Nearest triangle hit by the ray from `origin` along `dir` (floors carry handle 0 and are
/// skipped, so a pick lands on walls and devices).
pub fn pick(scene: &Scene, origin: V3, dir: V3) -> Option<Hit> {
    let dir = dir.norm();
    let mut best: Option<Hit> = None;
    for (i, tri) in scene.tris.as_chunks::<3>().0.iter().enumerate() {
        let Some(&owner) = scene.owners.get(i) else { break };
        if owner.handle == Handle(0) {
            continue;
        }
        let [a, b, c] = tri.each_ref().map(|v| v3(f64::from(v.p[0]), f64::from(v.p[1]), f64::from(v.p[2])));
        if let Some(t) = ray_tri(origin, dir, a, b, c)
            && best.is_none_or(|h| t < h.t)
        {
            best = Some(Hit { owner, t, point: origin + dir * t });
        }
    }
    best
}

/// Möller–Trumbore, both faces.
fn ray_tri(o: V3, d: V3, a: V3, b: V3, c: V3) -> Option<f64> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-6).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::Vertex;

    #[test]
    fn nearest_face_wins() {
        let vt = |x: f32, y: f32, z: f32| Vertex { p: [x, y, z], n: [0.0; 3], c: [0; 4] };
        let wall = |x: f32| [vt(x, -1.0, -1.0), vt(x, 1.0, -1.0), vt(x, 0.0, 1.0)];
        let mut s = Scene::default();
        s.tris.extend(wall(10.0));
        s.owners.push(Owner { src: 0, handle: Handle(7) });
        s.tris.extend(wall(5.0));
        s.owners.push(Owner { src: 1, handle: Handle(9) });
        let h = pick(&s, v3(0.0, 0.0, 0.0), v3(1.0, 0.0, 0.0));
        assert_eq!(h.map(|h| h.owner), Some(Owner { src: 1, handle: Handle(9) }));
        assert!(pick(&s, v3(0.0, 0.0, 0.0), v3(-1.0, 0.0, 0.0)).is_none());
    }
}
