//! Moving, turning and scaling objects in space (3DMOVE, 3DROTATE, 3DSCALE, 3DMIRROR, 3DALIGN,
//! 3DARRAY). Objects of points in space (lines, 3D polylines, 3D faces, meshes, solids, section
//! planes, rays) take any transform. Flat objects (circles, text, hatches…) take the transforms
//! that keep them flat and level: a turn about Z, a move, a scale. Tilted out of their plane, a
//! straight polyline becomes a 3D polyline; other flat objects are left as they are.

use cadaddy_geom::{Mat3, Mat4, Vec3};

use crate::{EntityKind, Polyline3d};

fn lin(m: &Mat4, v: Vec3) -> Vec3 {
    let r = &m.m;
    Vec3::new(
        r[0][0] * v.x + r[0][1] * v.y + r[0][2] * v.z,
        r[1][0] * v.x + r[1][1] * v.y + r[1][2] * v.z,
        r[2][0] * v.x + r[2][1] * v.y + r[2][2] * v.z,
    )
}

/// The transform as a plan transform and the map of heights (`z ↦ k·z + c`), when it keeps the XY
/// plane level.
pub fn level_parts(m: &Mat4) -> Option<(Mat3, f64, f64)> {
    let r = &m.m;
    let eps = 1e-12;
    let level = r[2][0].abs() < eps && r[2][1].abs() < eps && r[0][2].abs() < eps && r[1][2].abs() < eps && r[2][2].abs() > eps;
    level.then(|| (Mat3 { a: r[0][0], b: r[1][0], c: r[0][1], d: r[1][1], e: r[0][3], f: r[1][3] }, r[2][2], r[2][3]))
}

impl EntityKind {
    /// Every height a flat object has (its plane's, its points'), through `f`.
    pub fn map_z(&mut self, f: &dyn Fn(f64) -> f64) {
        let z = |p: &mut Vec3| p.z = f(p.z);
        match self {
            EntityKind::Line(l) => {
                z(&mut l.a);
                z(&mut l.b);
            }
            EntityKind::Point(p) => z(&mut p.p),
            EntityKind::Circle(c) => z(&mut c.center),
            EntityKind::Arc(a) => z(&mut a.center),
            EntityKind::Ellipse(e) => z(&mut e.center),
            EntityKind::Helix(h) => z(&mut h.center),
            EntityKind::LwPolyline(p) => p.elevation = f(p.elevation),
            EntityKind::Polyline3d(p) => p.points.iter_mut().for_each(z),
            EntityKind::Ray(r) | EntityKind::XLine(r) => z(&mut r.base),
            EntityKind::Text(t) => {
                z(&mut t.insert);
                t.align_pt.iter_mut().for_each(z);
            }
            EntityKind::AttDef(a) => {
                z(&mut a.text.insert);
                a.text.align_pt.iter_mut().for_each(z);
                if let Some(ml) = &mut a.multiline {
                    z(&mut ml.mtext.insert);
                }
            }
            EntityKind::MText(t) => z(&mut t.insert),
            EntityKind::Insert(i) => {
                z(&mut i.insert);
                for a in &mut i.attribs {
                    z(&mut a.text.insert);
                    a.text.align_pt.iter_mut().for_each(z);
                    if let Some(ml) = &mut a.multiline {
                        z(&mut ml.mtext.insert);
                    }
                }
            }
            EntityKind::Dimension(d) => {
                for p in [&mut d.defpt, &mut d.text_mid, &mut d.p13, &mut d.p14, &mut d.p15, &mut d.p16] {
                    z(p);
                }
            }
            EntityKind::Leader(l) => l.vertices.iter_mut().for_each(z),
            EntityKind::MLine(m) => m.vertices.iter_mut().for_each(|v| z(&mut v.p)),
            EntityKind::MLeader(m) => {
                m.leaders.iter_mut().for_each(|l| l.iter_mut().for_each(z));
                z(&mut m.landing);
                m.block.iter_mut().for_each(|b| z(&mut b.insert));
            }
            EntityKind::Hatch(h) => h.elevation = f(h.elevation),
            EntityKind::Solid(so) | EntityKind::Trace(so) => so.corners.iter_mut().for_each(z),
            EntityKind::Face3d(fc) => fc.corners.iter_mut().for_each(z),
            EntityKind::Mesh(m) => m.vertices.iter_mut().for_each(z),
            EntityKind::Image(i) => z(&mut i.insert),
            EntityKind::Underlay(u) => z(&mut u.insert),
            EntityKind::PointCloud(pc) => z(&mut pc.insert),
            EntityKind::Table(t) => z(&mut t.insert),
            EntityKind::Tolerance(t) => z(&mut t.insert),
            EntityKind::Light(l) => {
                z(&mut l.position);
                z(&mut l.target);
            }
            EntityKind::Solid3d(_)
            | EntityKind::SectionPlane(_)
            | EntityKind::Region(_)
            | EntityKind::Spline(_)
            | EntityKind::Viewport(_)
            | EntityKind::Wipeout(_)
            | EntityKind::Unknown(_) => {}
        }
    }

    /// Transform the object in space. `false` (the object unchanged) for a flat object the
    /// transform would tilt out of its plane.
    pub fn transform3(&mut self, m: &Mat4) -> bool {
        if !m.m.iter().flatten().all(|v| v.is_finite()) {
            return false;
        }
        let p = |q: &mut Vec3| *q = m.apply(*q);
        match self {
            EntityKind::Line(l) => {
                p(&mut l.a);
                p(&mut l.b);
                return true;
            }
            EntityKind::Point(pt) => {
                p(&mut pt.p);
                return true;
            }
            EntityKind::Polyline3d(pl) => {
                pl.points.iter_mut().for_each(p);
                return true;
            }
            EntityKind::Face3d(f) => {
                f.corners.iter_mut().for_each(p);
                return true;
            }
            EntityKind::Mesh(me) => {
                me.vertices.iter_mut().for_each(p);
                return true;
            }
            EntityKind::Solid3d(so) => {
                so.transform(m);
                return true;
            }
            EntityKind::SectionPlane(sp) => {
                sp.transform(m);
                return true;
            }
            // A light is a point in space and where it points: both follow any transform.
            EntityKind::Light(l) => {
                p(&mut l.position);
                p(&mut l.target);
                return true;
            }
            EntityKind::Ray(r) | EntityKind::XLine(r) => {
                p(&mut r.base);
                r.dir = lin(m, r.dir).normalized();
                return true;
            }
            _ => {}
        }
        if let Some((plan, k, c)) = level_parts(m) {
            self.transform(&plan);
            self.map_z(&|z| k * z + c);
            return true;
        }
        // Tilted: a straight polyline becomes a 3D polyline.
        if let EntityKind::LwPolyline(pl) = self
            && pl.vertices.iter().all(|v| v.bulge.abs() < 1e-12)
        {
            let points = pl.vertices.iter().map(|v| m.apply(Vec3::new(v.p.x, v.p.y, pl.elevation))).collect();
            *self = EntityKind::Polyline3d(Polyline3d { points, closed: pl.closed });
            return true;
        }
        false
    }
}

/// A turn by `angle` radians about the axis through `at` along `axis` (right-handed).
pub fn rotation_about(at: Vec3, axis: Vec3, angle: f64) -> Mat4 {
    let u = axis.normalized();
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let r = [
        [t * u.x * u.x + c, t * u.x * u.y - s * u.z, t * u.x * u.z + s * u.y],
        [t * u.x * u.y + s * u.z, t * u.y * u.y + c, t * u.y * u.z - s * u.x],
        [t * u.x * u.z - s * u.y, t * u.y * u.z + s * u.x, t * u.z * u.z + c],
    ];
    let o = Vec3::new(
        r[0][0] * at.x + r[0][1] * at.y + r[0][2] * at.z,
        r[1][0] * at.x + r[1][1] * at.y + r[1][2] * at.z,
        r[2][0] * at.x + r[2][1] * at.y + r[2][2] * at.z,
    );
    let tr = at - o;
    Mat4 { m: [[r[0][0], r[0][1], r[0][2], tr.x], [r[1][0], r[1][1], r[1][2], tr.y], [r[2][0], r[2][1], r[2][2], tr.z], [0.0, 0.0, 0.0, 1.0]] }
}

/// A move by `d`.
pub fn translation(d: Vec3) -> Mat4 {
    Mat4 { m: [[1.0, 0.0, 0.0, d.x], [0.0, 1.0, 0.0, d.y], [0.0, 0.0, 1.0, d.z], [0.0, 0.0, 0.0, 1.0]] }
}

/// A scale by `k` about `at`, along all three axes or only along `axis` (unit).
pub fn scaling(at: Vec3, k: f64, axis: Option<Vec3>) -> Mat4 {
    let mut r = [[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, k]];
    if let Some(a) = axis {
        let a = a.normalized();
        let v = [a.x, a.y, a.z];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = f64::from(u8::from(i == j)) + (k - 1.0) * v[i] * v[j];
            }
        }
    }
    let o = Vec3::new(
        r[0][0] * at.x + r[0][1] * at.y + r[0][2] * at.z,
        r[1][0] * at.x + r[1][1] * at.y + r[1][2] * at.z,
        r[2][0] * at.x + r[2][1] * at.y + r[2][2] * at.z,
    );
    let tr = at - o;
    Mat4 { m: [[r[0][0], r[0][1], r[0][2], tr.x], [r[1][0], r[1][1], r[1][2], tr.y], [r[2][0], r[2][1], r[2][2], tr.z], [0.0, 0.0, 0.0, 1.0]] }
}

/// A mirror in the plane through `at` with normal `n`.
pub fn mirror_in(at: Vec3, n: Vec3) -> Mat4 {
    let n = n.normalized();
    let v = [n.x, n.y, n.z];
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = f64::from(u8::from(i == j)) - 2.0 * v[i] * v[j];
        }
    }
    let o = Vec3::new(
        r[0][0] * at.x + r[0][1] * at.y + r[0][2] * at.z,
        r[1][0] * at.x + r[1][1] * at.y + r[1][2] * at.z,
        r[2][0] * at.x + r[2][1] * at.y + r[2][2] * at.z,
    );
    let tr = at - o;
    Mat4 { m: [[r[0][0], r[0][1], r[0][2], tr.x], [r[1][0], r[1][1], r[1][2], tr.y], [r[2][0], r[2][1], r[2][2], tr.z], [0.0, 0.0, 0.0, 1.0]] }
}

/// The rigid motion (and, with `scale`, uniform scale) taking source points to destination
/// points: the first point onto the first; the direction to the second onto the direction to
/// the second; the plane of the three onto the plane of the three (3DALIGN).
pub fn align(src: &[Vec3], dst: &[Vec3], scale: bool) -> Option<Mat4> {
    let (&s1, &d1) = (src.first()?, dst.first()?);
    let frame = |a: Vec3, b: Option<&Vec3>, c: Option<&Vec3>| -> Option<[Vec3; 3]> {
        let x = match b {
            Some(b) => (*b - a).normalized(),
            None => return Some([Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)]),
        };
        if x.len() < 0.5 {
            return None;
        }
        let up = match c {
            Some(c) => {
                let n = x.cross(*c - a).normalized();
                if n.len() < 0.5 { return None } else { n }
            }
            // No third point: keep the direction square to Z level where it can be.
            None => {
                let h = Vec3::new(0.0, 0.0, 1.0).cross(x);
                if h.len() > 1e-9 { x.cross(h.normalized()).normalized() * -1.0 } else { Vec3::new(0.0, 1.0, 0.0).cross(x).normalized() }
            }
        };
        let y = up.cross(x).normalized();
        Some([x, y, up])
    };
    let fs = frame(s1, src.get(1), src.get(2))?;
    let fd = frame(d1, dst.get(1), dst.get(2))?;
    let k = match (scale, src.get(1), dst.get(1)) {
        (true, Some(s2), Some(d2)) if (*s2 - s1).len() > 1e-12 => (*d2 - d1).len() / (*s2 - s1).len(),
        _ => 1.0,
    };
    // R maps the source frame's axes onto the destination's: R = Fd · Fsᵀ.
    let mut r = [[0.0; 3]; 3];
    let col = |f: &[Vec3; 3], i: usize| [f[i].x, f[i].y, f[i].z];
    for (row, line) in r.iter_mut().enumerate() {
        for (c, cell) in line.iter_mut().enumerate() {
            *cell = (0..3).map(|i| col(&fd, i)[row] * col(&fs, i)[c]).sum::<f64>() * k;
        }
    }
    let o = Vec3::new(
        r[0][0] * s1.x + r[0][1] * s1.y + r[0][2] * s1.z,
        r[1][0] * s1.x + r[1][1] * s1.y + r[1][2] * s1.z,
        r[2][0] * s1.x + r[2][1] * s1.y + r[2][2] * s1.z,
    );
    let t = d1 - o;
    Some(Mat4 { m: [[r[0][0], r[0][1], r[0][2], t.x], [r[1][0], r[1][1], r[1][2], t.y], [r[2][0], r[2][1], r[2][2], t.z], [0.0, 0.0, 0.0, 1.0]] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Circle, Line};

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).len() < 1e-9
    }

    #[test]
    fn rotations_mirrors_scales_and_alignment() {
        let r = rotation_about(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2);
        assert!(near(r.apply(Vec3::new(2.0, 0.0, 5.0)), Vec3::new(1.0, 1.0, 5.0)));
        let x = rotation_about(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), std::f64::consts::FRAC_PI_2);
        assert!(near(x.apply(Vec3::new(0.0, 1.0, 0.0)), Vec3::new(0.0, 0.0, 1.0)));
        assert!(near(mirror_in(Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, 1.0)).apply(Vec3::new(1.0, 1.0, 5.0)), Vec3::new(1.0, 1.0, -1.0)));
        assert!(near(
            scaling(Vec3::new(1.0, 1.0, 1.0), 3.0, Some(Vec3::new(0.0, 0.0, 1.0))).apply(Vec3::new(2.0, 2.0, 2.0)),
            Vec3::new(2.0, 2.0, 4.0)
        ));
        // Three points onto three points.
        let src = [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
        let dst = [Vec3::new(5.0, 5.0, 5.0), Vec3::new(5.0, 5.0, 6.0), Vec3::new(5.0, 6.0, 5.0)];
        let m = align(&src, &dst, false).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            assert!(near(m.apply(*s), *d), "{:?} → {:?}", m.apply(*s), d);
        }
        // Two points with scale: the length follows.
        let m = align(&[Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)], &[Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)], true).unwrap();
        assert!(near(m.apply(Vec3::new(1.0, 0.0, 0.0)), Vec3::new(0.0, 3.0, 0.0)));
    }

    #[test]
    fn flat_objects_take_level_transforms_only() {
        let mut c = EntityKind::Circle(Circle { center: Vec3::new(1.0, 0.0, 2.0), radius: 1.0 });
        assert!(c.transform3(&translation(Vec3::new(0.0, 0.0, 3.0))));
        assert!(matches!(&c, EntityKind::Circle(cc) if cc.center.z == 5.0));
        // Tilting it is refused; a line takes it.
        assert!(!c.transform3(&rotation_about(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 1.0)));
        let mut l = EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(0.0, 1.0, 0.0) });
        assert!(l.transform3(&rotation_about(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), std::f64::consts::FRAC_PI_2)));
        assert!(matches!(&l, EntityKind::Line(ll) if near(ll.b, Vec3::new(0.0, 0.0, 1.0))));
    }
}
