//! 3D solids (BOX, WEDGE, CYLINDER, CONE, SPHERE, TORUS, PYRAMID, EXTRUDE, REVOLVE, SWEEP,
//! LOFT, PRESSPULL): CADaddy's own solid object. A solid is its exact definition — a primitive's
//! dimensions, or a profile and how it was extruded, revolved, swept or lofted — and a placement.
//! Its surfaces and display curves are derived from it to a tolerance; its volume, surface area and
//! centroid are exact for the primitives and straight extrusions, and computed from the surfaces
//! for the rest.
//!
//! Every shape is a sweep of the same loops through a sequence of stations ([`Rings`]): ruled
//! strips join consecutive stations, and planar caps close the ends unless the stations wrap round
//! (a torus, a full revolution) or end in a point (a sphere's poles, a cone's apex).

use cadaddy_geom::{Mat4, PolyVertex, Polyline, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use std::f64::consts::{PI, TAU};

pub mod bsp;
mod csg;
mod edit;
mod tri;
pub use csg::*;
pub use edit::*;
pub use tri::triangulate;

/// A 3D solid: its definition in its own coordinates, and where those are in the drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Solid3d {
    pub shape: SolidShape,
    pub xf: Mat4,
}

/// What a solid is, in its own coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SolidShape {
    /// From the origin along +X, +Y and +Z.
    Box {
        length: f64,
        width: f64,
        height: f64,
    },
    /// A box cut diagonally: full height along the YZ face at x = 0, down to nothing at x = length.
    Wedge {
        length: f64,
        width: f64,
        height: f64,
    },
    /// A cylinder (equal radii), cone (top radius 0) or frustum about +Z from its base centre.
    Frustum {
        #[serde(rename = "baseRadius")]
        base_radius: f64,
        #[serde(rename = "topRadius")]
        top_radius: f64,
        height: f64,
    },
    Sphere {
        radius: f64,
    },
    /// About +Z: the tube's centre circle has `radius`, the tube itself `tube`.
    Torus {
        radius: f64,
        tube: f64,
    },
    /// A regular polygon base of `sides` (its corners at `base_radius`, the first on +X) rising
    /// along +Z to a top of `top_radius` (0: a point).
    Pyramid {
        sides: u32,
        #[serde(rename = "baseRadius")]
        base_radius: f64,
        #[serde(rename = "topRadius")]
        top_radius: f64,
        height: f64,
    },
    /// A closed profile (in XY; loops inside others are holes) extruded along +Z, its edges
    /// moving in by `height · tan(taper)` at the top.
    Extrusion {
        loops: Vec<Vec<PolyVertex>>,
        height: f64,
        taper: f64,
    },
    /// A closed profile in XY (at x ≥ 0) revolved about the Y axis by `angle` radians.
    Revolution {
        loops: Vec<Vec<PolyVertex>>,
        angle: f64,
    },
    /// A closed profile in XY carried along `path` (its own coordinates), square to the path
    /// and with its origin on it, turning as little as it can from one point to the next.
    Sweep {
        loops: Vec<Vec<PolyVertex>>,
        path: Vec<Vec3>,
    },
    /// Closed cross sections passed through in order.
    Loft {
        sections: Vec<Vec<Vec3>>,
    },
    /// Two solids combined (UNION, SUBTRACT, INTERSECT), each placed in this solid's coordinates.
    Boolean {
        op: BoolOp,
        a: Box<Solid3d>,
        b: Box<Solid3d>,
    },
}

/// Most stations, loops and points per loop a solid is drawn with.
const MAX_STATIONS: usize = 4096;
const MAX_LOOPS: usize = 256;
const MAX_LOOP_POINTS: usize = 8192;
/// The tolerance plan-view curves of a solid are drawn to (snaps, selection, extents).
pub const SOLID_PRIM_TOL: f64 = 0.01;

/// The points of a solid worth snapping to: its corners and the ends of its display curves.
pub fn solid_corners(s: &Solid3d) -> Vec<Vec3> {
    let mut out = Vec::new();
    for w in s.wires(SOLID_PRIM_TOL) {
        out.extend(w.first().copied());
        out.extend(w.last().copied());
    }
    out.dedup_by(|a, b| (*a - *b).len() < 1e-9);
    out
}

/// Largest dimension accepted (drawing units).
pub const MAX_SOLID_SIZE: f64 = 1e9;

/// A solid as stations of loops: `stations[s][l][p]` is point `p` of loop `l` at station `s`
/// (every station has the same loops and point counts), in drawing coordinates.
#[derive(Clone, Debug, Default)]
pub struct Rings {
    pub stations: Vec<Vec<Vec<Vec3>>>,
    /// The last station joins the first (a torus, a full revolution).
    pub wrap: bool,
    /// The first and last stations are closed by planar caps.
    pub caps: bool,
    /// The stations whose loops are display curves.
    pub section_wires: Vec<usize>,
    /// Per loop, the points whose path through the stations is a display curve.
    pub long_wires: Vec<Vec<bool>>,
}

/// What a grip sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dim {
    Base,
    Length,
    Width,
    Height,
    Radius,
    TopRadius,
    Tube,
}

/// Volume, surface area and centroid of a solid, and its bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidProps {
    pub volume: f64,
    pub area: f64,
    pub centroid: Vec3,
    pub min: Vec3,
    pub max: Vec3,
}

/// Segments a full circle of radius `r` takes to stay within `tol`: a multiple of four (so the
/// quarter isolines fall on points), at least 16.
pub fn circle_segments(r: f64, tol: f64) -> usize {
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { 0.01 };
    let r = r.abs();
    if !(r.is_finite() && r > tol) {
        return 16;
    }
    let step = 2.0 * (1.0 - tol / r).clamp(-1.0, 1.0).acos();
    let n = if step > 1e-9 { (TAU / step).ceil() as usize } else { 720 };
    n.clamp(16, 720).div_ceil(4) * 4
}

fn mat_vec(m: &Mat4, v: Vec3) -> Vec3 {
    let r = &m.m;
    Vec3::new(
        r[0][0] * v.x + r[0][1] * v.y + r[0][2] * v.z,
        r[1][0] * v.x + r[1][1] * v.y + r[1][2] * v.z,
        r[2][0] * v.x + r[2][1] * v.y + r[2][2] * v.z,
    )
}

/// The determinant of the linear part.
fn det3(m: &Mat4) -> f64 {
    let a = &m.m;
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

/// A placement: translation `o`, then the local X/Y/Z axes `x`, `y`, `z` (columns).
pub fn placement(o: Vec3, x: Vec3, y: Vec3, z: Vec3) -> Mat4 {
    Mat4 { m: [[x.x, y.x, z.x, o.x], [x.y, y.y, z.y, o.y], [x.z, y.z, z.z, o.z], [0.0, 0.0, 0.0, 1.0]] }
}

/// Axes square to `z` (unit): an X axis in the drawing's XY plane when it can be, and Y = Z × X.
pub fn frame_for(z: Vec3) -> (Vec3, Vec3) {
    let z = z.normalized();
    let x = if z.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0).cross(z).cross(z) * -1.0 } else { Vec3::new(0.0, 1.0, 0.0).cross(z).cross(z) * -1.0 };
    let x = x.normalized();
    (x, z.cross(x))
}

/// A profile loop as points (arcs to `tol`) and which of them are corners (its vertices; for a
/// circle, its quarter points).
pub fn loop_points(l: &[PolyVertex], tol: f64) -> (Vec<Vec2>, Vec<bool>) {
    let mut pts = Vec::new();
    let mut corner = Vec::new();
    let n = l.len();
    let circle = n == 2 && l.iter().all(|v| (v.bulge.abs() - 1.0).abs() < 1e-9);
    for i in 0..n {
        let (Some(a), Some(b)) = (l.get(i), l.get((i + 1) % n)) else { continue };
        pts.push(a.p);
        corner.push(!circle);
        if let Some((arc, ccw)) = cadaddy_geom::bulge_to_arc(a.p, b.p, a.bulge) {
            let seg = cadaddy_geom::Segment::Arc { arc, ccw };
            let k = ((circle_segments(arc.radius, tol) as f64) * arc.sweep() / TAU).ceil().clamp(2.0, 720.0) as usize;
            for j in 1..k {
                pts.push(seg.at(j as f64 / k as f64));
                corner.push(false);
            }
        }
        if pts.len() > MAX_LOOP_POINTS {
            break;
        }
    }
    if circle && !corner.is_empty() {
        let m = corner.len();
        for q in 0..4 {
            if let Some(c) = corner.get_mut(q * m / 4) {
                *c = true;
            }
        }
    }
    (pts, corner)
}

/// Signed area of a polygon (positive counter-clockwise).
fn signed_area(p: &[Vec2]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].cross(p[(i + 1) % n])).sum::<f64>() / 2.0
}

/// Loops ordered so outer boundaries run counter-clockwise and holes (inside an odd number of
/// others) clockwise.
fn orient(loops: Vec<(Vec<Vec2>, Vec<bool>)>) -> Vec<(Vec<Vec2>, Vec<bool>)> {
    let firsts: Vec<Option<Vec2>> = loops.iter().map(|(p, _)| p.first().copied()).collect();
    let polys: Vec<Vec<Vec2>> = loops.iter().map(|(p, _)| p.clone()).collect();
    loops
        .into_iter()
        .enumerate()
        .map(|(i, (mut p, mut c))| {
            let depth = firsts
                .get(i)
                .copied()
                .flatten()
                .map_or(0, |q| polys.iter().enumerate().filter(|(j, o)| *j != i && cadaddy_geom::point_in_polygon(o, q)).count());
            let want_ccw = depth % 2 == 0;
            if (signed_area(&p) > 0.0) != want_ccw {
                p.reverse();
                c.reverse();
            }
            (p, c)
        })
        .collect()
}

impl Solid3d {
    pub fn new(shape: SolidShape) -> Solid3d {
        Solid3d { shape, xf: Mat4::IDENTITY }
    }

    /// The shape's name as LIST and the Properties palette show it ("Box", "Extrusion"…).
    pub fn shape_name(&self) -> &'static str {
        match &self.shape {
            SolidShape::Box { .. } => "Box",
            SolidShape::Wedge { .. } => "Wedge",
            SolidShape::Frustum { top_radius, base_radius, .. } if (top_radius - base_radius).abs() < 1e-12 => "Cylinder",
            SolidShape::Frustum { .. } => "Cone",
            SolidShape::Sphere { .. } => "Sphere",
            SolidShape::Torus { .. } => "Torus",
            SolidShape::Pyramid { .. } => "Pyramid",
            SolidShape::Extrusion { .. } => "Extrusion",
            SolidShape::Revolution { .. } => "Revolve",
            SolidShape::Sweep { .. } => "Sweep",
            SolidShape::Loft { .. } => "Loft",
            SolidShape::Boolean { .. } => "Composite",
        }
    }

    /// Whether the definition is usable: finite, positive dimensions within [`MAX_SOLID_SIZE`];
    /// a composite no deeper than [`MAX_CSG_DEPTH`] with at most [`MAX_CSG_LEAVES`] leaves.
    pub fn is_valid(&self) -> bool {
        let mut nodes = 0usize;
        self.valid_at(0, &mut nodes)
    }

    fn valid_at(&self, depth: usize, nodes: &mut usize) -> bool {
        *nodes += 1;
        if *nodes > 2 * MAX_CSG_LEAVES || depth > MAX_CSG_DEPTH {
            return false;
        }
        if let SolidShape::Boolean { a, b, .. } = &self.shape {
            let xf_ok = self.xf.m.iter().flatten().all(|v| v.is_finite()) && det3(&self.xf).abs() > 1e-300;
            return xf_ok && a.valid_at(depth + 1, nodes) && b.valid_at(depth + 1, nodes);
        }
        let ok = |v: f64| v.is_finite() && v > 0.0 && v <= MAX_SOLID_SIZE;
        let ok0 = |v: f64| v.is_finite() && (0.0..=MAX_SOLID_SIZE).contains(&v);
        let xf_ok = self.xf.m.iter().flatten().all(|v| v.is_finite()) && det3(&self.xf).abs() > 1e-300;
        xf_ok
            && match &self.shape {
                SolidShape::Box { length, width, height } | SolidShape::Wedge { length, width, height } => ok(*length) && ok(*width) && ok(*height),
                SolidShape::Frustum { base_radius, top_radius, height } => {
                    ok0(*base_radius) && ok0(*top_radius) && base_radius + top_radius > 0.0 && ok(*height)
                }
                SolidShape::Sphere { radius } => ok(*radius),
                SolidShape::Torus { radius, tube } => ok(*radius) && ok(*tube) && tube < radius,
                SolidShape::Pyramid { sides, base_radius, top_radius, height } => {
                    (3..=32).contains(sides) && ok0(*base_radius) && ok0(*top_radius) && base_radius + top_radius > 0.0 && ok(*height)
                }
                SolidShape::Extrusion { loops, height, taper } => {
                    !loops.is_empty()
                        && loops.len() <= MAX_LOOPS
                        && ok(*height)
                        && taper.is_finite()
                        && taper.abs() < PI / 2.0 - 1e-6
                        && loops_ok(loops)
                }
                SolidShape::Revolution { loops, angle } => {
                    !loops.is_empty() && loops.len() <= MAX_LOOPS && angle.is_finite() && *angle > 1e-9 && *angle <= TAU + 1e-9 && loops_ok(loops)
                }
                SolidShape::Sweep { loops, path } => {
                    !loops.is_empty()
                        && loops.len() <= MAX_LOOPS
                        && loops_ok(loops)
                        && path.len() >= 2
                        && path.len() <= MAX_STATIONS
                        && path.iter().all(|p| p.is_finite())
                }
                SolidShape::Boolean { .. } => false,
                SolidShape::Loft { sections } => {
                    sections.len() >= 2
                        && sections.len() <= MAX_STATIONS
                        && sections.iter().all(|s| s.len() >= 3 && s.len() <= MAX_LOOP_POINTS && s.iter().all(|p| p.is_finite()))
                }
            }
    }

    /// The solid as stations of loops, curves to `tol`, in drawing coordinates.
    pub fn rings(&self, tol: f64) -> Rings {
        let mut r = self.local_rings(tol);
        for st in &mut r.stations {
            for l in st.iter_mut() {
                for p in l.iter_mut() {
                    *p = self.xf.apply(*p);
                }
            }
        }
        r
    }

    fn local_rings(&self, tol: f64) -> Rings {
        if !self.is_valid() {
            return Rings::default();
        }
        let flat = |pts: &[Vec2], z: f64| pts.iter().map(|p| Vec3::new(p.x, p.y, z)).collect::<Vec<_>>();
        let circle = |r: f64, n: usize| (0..n).map(|k| Vec2::from_angle(TAU * k as f64 / n as f64) * r).collect::<Vec<_>>();
        let quarters = |n: usize| (0..n).map(|k| k % (n / 4).max(1) == 0).collect::<Vec<_>>();
        match &self.shape {
            SolidShape::Box { length: l, width: w, height: h } => {
                let p = [Vec2::new(0.0, 0.0), Vec2::new(*l, 0.0), Vec2::new(*l, *w), Vec2::new(0.0, *w)];
                prism(vec![(p.to_vec(), vec![true; 4])], *h, 0.0)
            }
            SolidShape::Wedge { length: l, width: w, height: h } => {
                // The triangle in XZ, carried along Y.
                let tri = |y: f64| vec![Vec3::new(0.0, y, 0.0), Vec3::new(*l, y, 0.0), Vec3::new(0.0, y, *h)];
                Rings {
                    stations: vec![vec![tri(0.0)], vec![tri(*w)]],
                    wrap: false,
                    caps: true,
                    section_wires: vec![0, 1],
                    long_wires: vec![vec![true; 3]],
                }
            }
            SolidShape::Frustum { base_radius, top_radius, height } => {
                let n = circle_segments(base_radius.max(*top_radius), tol);
                Rings {
                    stations: vec![vec![flat(&circle(*base_radius, n), 0.0)], vec![flat(&circle(*top_radius, n), *height)]],
                    wrap: false,
                    caps: true,
                    section_wires: vec![0, 1],
                    long_wires: vec![quarters(n)],
                }
            }
            SolidShape::Pyramid { sides, base_radius, top_radius, height } => {
                let n = *sides as usize;
                Rings {
                    stations: vec![vec![flat(&circle(*base_radius, n), 0.0)], vec![flat(&circle(*top_radius, n), *height)]],
                    wrap: false,
                    caps: true,
                    section_wires: vec![0, 1],
                    long_wires: vec![vec![true; n]],
                }
            }
            SolidShape::Sphere { radius } => {
                let n = circle_segments(*radius, tol);
                let rows = n / 2;
                let stations = (0..=rows)
                    .map(|k| {
                        let a = -PI / 2.0 + PI * k as f64 / rows as f64;
                        vec![flat(&circle(radius * a.cos(), n), radius * a.sin())]
                    })
                    .collect();
                Rings { stations, wrap: false, caps: false, section_wires: vec![rows / 2], long_wires: vec![quarters(n)] }
            }
            SolidShape::Torus { radius, tube } => {
                let n = circle_segments(radius + tube, tol);
                let m = circle_segments(*tube, tol);
                let stations = (0..n)
                    .map(|k| {
                        let a = TAU * k as f64 / n as f64;
                        let (s, c) = a.sin_cos();
                        let ring = (0..m)
                            .map(|j| {
                                let b = TAU * j as f64 / m as f64;
                                let rr = radius + tube * b.cos();
                                Vec3::new(rr * c, rr * s, tube * b.sin())
                            })
                            .collect();
                        vec![ring]
                    })
                    .collect();
                Rings {
                    stations,
                    wrap: true,
                    caps: false,
                    section_wires: (0..4).map(|q| q * n / 4).collect(),
                    long_wires: vec![(0..m).map(|j| j == 0 || j == m / 2).collect()],
                }
            }
            SolidShape::Extrusion { loops, height, taper } => {
                let ls: Vec<(Vec<Vec2>, Vec<bool>)> = loops.iter().map(|l| loop_points(l, tol)).collect();
                prism(orient(ls), *height, *taper)
            }
            SolidShape::Revolution { loops, angle } => {
                let ls = orient(loops.iter().map(|l| loop_points(l, tol)).collect());
                let reach = ls.iter().flat_map(|(p, _)| p.iter().map(|q| q.x.abs())).fold(0.0, f64::max);
                let full = (angle - TAU).abs() < 1e-9;
                let k = ((circle_segments(reach, tol) as f64) * angle / TAU).ceil().max(2.0) as usize;
                let count = if full { k } else { k + 1 };
                let stations = (0..count.min(MAX_STATIONS))
                    .map(|i| {
                        let t = angle * i as f64 / k as f64;
                        let (s, c) = t.sin_cos();
                        ls.iter().map(|(p, _)| p.iter().map(|q| Vec3::new(q.x * c, q.y, -q.x * s)).collect()).collect()
                    })
                    .collect::<Vec<Vec<Vec<Vec3>>>>();
                let last = stations.len().saturating_sub(1);
                Rings {
                    stations,
                    wrap: full,
                    caps: !full,
                    section_wires: if full { vec![] } else { vec![0, last] },
                    long_wires: ls.iter().map(|(_, c)| c.clone()).collect(),
                }
            }
            SolidShape::Sweep { loops, path } => {
                let ls = orient(loops.iter().map(|l| loop_points(l, tol)).collect());
                let frames = sweep_frames(path);
                let stations = frames
                    .iter()
                    .map(|f| {
                        ls.iter()
                            .map(|(p, _)| {
                                p.iter()
                                    .map(|q| {
                                        let off = f.x * q.x + f.y * q.y;
                                        f.o + off + f.bend * (off.dot(f.bend) * (f.stretch - 1.0))
                                    })
                                    .collect()
                            })
                            .collect()
                    })
                    .collect::<Vec<Vec<Vec<Vec3>>>>();
                let last = stations.len().saturating_sub(1);
                Rings { stations, wrap: false, caps: true, section_wires: vec![0, last], long_wires: ls.iter().map(|(_, c)| c.clone()).collect() }
            }
            SolidShape::Loft { sections } => loft_rings(sections),
            SolidShape::Boolean { .. } => Rings::default(),
        }
    }

    /// The display curves (silhouette-free wireframe: section loops and lines along corners), in
    /// drawing coordinates.
    pub fn wires(&self, tol: f64) -> Vec<Vec<Vec3>> {
        if matches!(self.shape, SolidShape::Boolean { .. }) {
            return self.feature_edges(tol).into_iter().map(|[a, b]| vec![a, b]).collect();
        }
        let r = self.rings(tol);
        let mut out = Vec::new();
        for &s in &r.section_wires {
            for l in r.stations.get(s).map(Vec::as_slice).unwrap_or(&[]) {
                let mut l = l.clone();
                if let Some(f) = l.first().copied() {
                    l.push(f);
                }
                out.push(l);
            }
        }
        for (li, mask) in r.long_wires.iter().enumerate() {
            for (pi, on) in mask.iter().enumerate() {
                if !on {
                    continue;
                }
                let mut line: Vec<Vec3> = r.stations.iter().filter_map(|st| st.get(li).and_then(|l| l.get(pi)).copied()).collect();
                if r.wrap
                    && let Some(f) = line.first().copied()
                {
                    line.push(f);
                }
                line.dedup_by(|a, b| (*a - *b).len() < 1e-12);
                if line.len() >= 2 {
                    out.push(line);
                }
            }
        }
        out
    }

    /// The side surfaces as triangles (ruled strips between stations) and the caps as planar
    /// loops (outer boundaries counter-clockwise seen from outside, holes the other way), in
    /// drawing coordinates.
    pub fn surfaces(&self, tol: f64) -> (Vec<[Vec3; 3]>, Vec<Vec<Vec<Vec3>>>) {
        let r = self.rings(tol);
        let n = r.stations.len();
        let mut tris = Vec::new();
        let pairs = if r.wrap { n } else { n.saturating_sub(1) };
        for s in 0..pairs {
            let (Some(a), Some(b)) = (r.stations.get(s), r.stations.get((s + 1) % n)) else { continue };
            for (la, lb) in a.iter().zip(b) {
                let m = la.len().min(lb.len());
                for i in 0..m {
                    let j = (i + 1) % m;
                    let (p0, p1, q0, q1) = (la[i], la[j], lb[i], lb[j]);
                    if (p0 - p1).len() > 1e-15 {
                        tris.push([p0, p1, q1]);
                    }
                    if (q0 - q1).len() > 1e-15 {
                        tris.push([p0, q1, q0]);
                    }
                }
            }
        }
        let mut caps = Vec::new();
        if r.caps && n >= 2 {
            if let Some(first) = r.stations.first() {
                caps.push(first.iter().map(|l| l.iter().rev().copied().collect()).collect());
            }
            if let Some(last) = r.stations.last() {
                caps.push(last.clone());
            }
        }
        (tris, caps)
    }

    /// Volume, surface area, centroid and bounds. Exact for the primitives and for extrusions
    /// without taper (under rotations, moves and uniform scales); otherwise from the surfaces at
    /// `tol`.
    pub fn props(&self, tol: f64) -> SolidProps {
        if matches!(self.shape, SolidShape::Boolean { .. }) {
            let polys = self.polys(tol);
            let (mut min, mut max) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
            for p in polys.iter().flat_map(|p| p.pts.iter()) {
                min = Vec3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
                max = Vec3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
            }
            let (volume, area, centroid) = bsp::mass(&polys);
            return SolidProps { volume, area, centroid, min, max };
        }
        let r = self.rings(tol);
        let (mut min, mut max) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for p in r.stations.iter().flatten().flatten() {
            min = Vec3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
            max = Vec3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
        }
        let numeric = || self.numeric_props(tol);
        let det = det3(&self.xf);
        // A uniform scale (with any rotation): lengths scale by s, so exact formulas apply.
        let s = det.abs().cbrt();
        let cols = [0, 1, 2].map(|c| Vec3::new(self.xf.m[0][c], self.xf.m[1][c], self.xf.m[2][c]));
        let uniform = cols.iter().all(|c| (c.len() - s).abs() <= 1e-9 * s.max(1.0))
            && cols[0].dot(cols[1]).abs() <= 1e-9 * s * s
            && cols[0].dot(cols[2]).abs() <= 1e-9 * s * s
            && cols[1].dot(cols[2]).abs() <= 1e-9 * s * s;
        let exact = if uniform { self.exact_local() } else { None };
        let (volume, area, centroid) = match exact {
            Some((v, a, c)) => (v * s.powi(3), a * s * s, self.xf.apply(c)),
            None => numeric(),
        };
        SolidProps { volume, area, centroid, min, max }
    }

    /// Exact volume, area and centroid in the solid's own coordinates, where known.
    fn exact_local(&self) -> Option<(f64, f64, Vec3)> {
        Some(match &self.shape {
            SolidShape::Box { length: l, width: w, height: h } => (l * w * h, 2.0 * (l * w + l * h + w * h), Vec3::new(l / 2.0, w / 2.0, h / 2.0)),
            SolidShape::Wedge { length: l, width: w, height: h } => {
                let slope = (l * l + h * h).sqrt();
                (l * w * h / 2.0, l * h + l * w + h * w + slope * w, Vec3::new(l / 3.0, w / 2.0, h / 3.0))
            }
            SolidShape::Frustum { base_radius: r1, top_radius: r2, height: h } => {
                let v = PI * h * (r1 * r1 + r1 * r2 + r2 * r2) / 3.0;
                let a = PI * (r1 * r1 + r2 * r2) + PI * (r1 + r2) * ((r1 - r2).powi(2) + h * h).sqrt();
                let z = h * (r1 * r1 + 2.0 * r1 * r2 + 3.0 * r2 * r2) / (4.0 * (r1 * r1 + r1 * r2 + r2 * r2));
                (v, a, Vec3::new(0.0, 0.0, z))
            }
            SolidShape::Sphere { radius: r } => (4.0 / 3.0 * PI * r.powi(3), 4.0 * PI * r * r, Vec3::ZERO),
            SolidShape::Torus { radius: rr, tube: r } => (2.0 * PI * PI * rr * r * r, 4.0 * PI * PI * rr * r, Vec3::ZERO),
            SolidShape::Pyramid { sides, base_radius: r1, top_radius: r2, height: h } => {
                let n = f64::from(*sides);
                let area_of = |r: f64| n / 2.0 * r * r * (TAU / n).sin();
                let (a1, a2) = (area_of(*r1), area_of(*r2));
                let v = h / 3.0 * (a1 + a2 + (a1 * a2).sqrt());
                let (ap1, ap2) = (r1 * (PI / n).cos(), r2 * (PI / n).cos());
                let (s1, s2) = (2.0 * r1 * (PI / n).sin(), 2.0 * r2 * (PI / n).sin());
                let lateral = n * (s1 + s2) / 2.0 * (h * h + (ap1 - ap2).powi(2)).sqrt();
                // The centroid of a frustum of similar sections, as for a cone.
                let (k1, k2) = (r1 * r1, r2 * r2);
                let z = h * (k1 + 2.0 * (k1 * k2).sqrt() + 3.0 * k2) / (4.0 * (k1 + (k1 * k2).sqrt() + k2));
                (v, a1 + a2 + lateral, Vec3::new(0.0, 0.0, z))
            }
            SolidShape::Extrusion { loops, height, taper } if taper.abs() < 1e-15 => {
                // Exact with arcs: the profile's area, centroid and perimeter.
                let (mut area, mut cx, mut cy, mut per) = (0.0, 0.0, 0.0, 0.0);
                let pts: Vec<Vec<Vec2>> = loops.iter().map(|l| l.iter().map(|v| v.p).collect()).collect();
                for (i, l) in loops.iter().enumerate() {
                    let pl = Polyline { vertices: l.clone(), closed: true };
                    let a = pl.area().abs();
                    let c = loop_centroid(l);
                    let depth =
                        l.first().map_or(0, |f| pts.iter().enumerate().filter(|(j, o)| *j != i && cadaddy_geom::point_in_polygon(o, f.p)).count());
                    let sign = if depth % 2 == 0 { 1.0 } else { -1.0 };
                    area += sign * a;
                    cx += sign * a * c.x;
                    cy += sign * a * c.y;
                    per += pl.len();
                }
                if area <= 0.0 {
                    return None;
                }
                (area * height, 2.0 * area + per * height, Vec3::new(cx / area, cy / area, height / 2.0))
            }
            _ => return None,
        })
    }

    /// Volume, area and centroid from the surfaces: signed tetrahedra from the origin over the side
    /// triangles and a fan over each cap.
    fn numeric_props(&self, tol: f64) -> (f64, f64, Vec3) {
        let (tris, caps) = self.surfaces(tol);
        let mut all: Vec<[Vec3; 3]> = tris;
        for cap in &caps {
            let Some(o) = cap.first().and_then(|l| l.first()).copied() else { continue };
            for l in cap {
                let m = l.len();
                for i in 0..m {
                    all.push([o, l[i], l[(i + 1) % m]]);
                }
            }
        }
        let (mut v, mut c, mut a) = (0.0, Vec3::ZERO, 0.0);
        for [p, q, r] in &all {
            let tv = p.dot(q.cross(*r)) / 6.0;
            v += tv;
            c = c + (*p + *q + *r) * (tv / 4.0);
        }
        // Area: the side triangles, and each cap's loops (holes subtract).
        let (tris, _) = self.surfaces(tol);
        for [p, q, r] in &tris {
            a += (*q - *p).cross(*r - *p).len() / 2.0;
        }
        for cap in &caps {
            let mut vec = Vec3::ZERO;
            for l in cap {
                let m = l.len();
                for i in 0..m {
                    vec = vec + l[i].cross(l[(i + 1) % m]) * 0.5;
                }
            }
            a += vec.len();
        }
        if v.abs() < 1e-300 {
            return (0.0, a, Vec3::ZERO);
        }
        (v.abs(), a, c * (1.0 / v))
    }

    /// The grips in the solid's own coordinates: the first is its base point (dragging it moves
    /// the solid), each other one sets a dimension, with the local direction that dimension grows
    /// in.
    fn local_grips(&self) -> Vec<(Vec3, Vec3, Dim)> {
        let (x, y, z) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let mut g = vec![(Vec3::ZERO, Vec3::ZERO, Dim::Base)];
        match &self.shape {
            SolidShape::Box { length: l, width: w, height: h } => g.extend([
                (Vec3::new(*l, w / 2.0, 0.0), x, Dim::Length),
                (Vec3::new(l / 2.0, *w, 0.0), y, Dim::Width),
                (Vec3::new(l / 2.0, w / 2.0, *h), z, Dim::Height),
            ]),
            SolidShape::Wedge { length: l, width: w, height: h } => g.extend([
                (Vec3::new(*l, w / 2.0, 0.0), x, Dim::Length),
                (Vec3::new(l / 2.0, *w, 0.0), y, Dim::Width),
                (Vec3::new(0.0, w / 2.0, *h), z, Dim::Height),
            ]),
            // A cylinder has one radius.
            SolidShape::Frustum { base_radius: r1, top_radius: r2, height: h } if (r1 - r2).abs() < 1e-12 => {
                g.extend([(Vec3::new(*r1, 0.0, 0.0), x, Dim::Radius), (Vec3::new(0.0, 0.0, *h), z, Dim::Height)])
            }
            SolidShape::Frustum { base_radius: r1, top_radius: r2, height: h }
            | SolidShape::Pyramid { base_radius: r1, top_radius: r2, height: h, .. } => g.extend([
                (Vec3::new(*r1, 0.0, 0.0), x, Dim::Radius),
                (Vec3::new(*r2, 0.0, *h), x, Dim::TopRadius),
                (Vec3::new(0.0, 0.0, *h), z, Dim::Height),
            ]),
            SolidShape::Sphere { radius } => g.push((Vec3::new(*radius, 0.0, 0.0), x, Dim::Radius)),
            SolidShape::Torus { radius, tube } => {
                g.extend([(Vec3::new(*radius, 0.0, 0.0), x, Dim::Radius), (Vec3::new(radius + tube, 0.0, 0.0), x, Dim::Tube)])
            }
            SolidShape::Extrusion { loops, height, .. } => {
                let p = loops.first().and_then(|l| l.first()).map_or(Vec2::ZERO, |v| v.p);
                g.push((Vec3::new(p.x, p.y, *height), z, Dim::Height));
            }
            SolidShape::Revolution { .. } | SolidShape::Sweep { .. } | SolidShape::Loft { .. } | SolidShape::Boolean { .. } => {}
        }
        g
    }

    /// The grip points in drawing coordinates (see [`Solid3d::grip_moved`]).
    pub fn grip_points(&self) -> Vec<Vec3> {
        self.local_grips().into_iter().map(|(p, _, _)| self.xf.apply(p)).collect()
    }

    /// The solid after dragging dimension grip `index` (1…) by `delta` in the drawing's XY: the
    /// dimension changes by the drag along its direction as seen from above, or — for a
    /// direction that points out of the XY plane (a height seen from above) — by the drag along
    /// Y. `None` for the base grip, a grip that doesn't exist, or a result that isn't a solid.
    pub fn grip_moved(&self, index: usize, delta: Vec2) -> Option<Solid3d> {
        if !delta.is_finite() {
            return None;
        }
        let (_, dir, dim) = *self.local_grips().get(index)?;
        let w = mat_vec(&self.xf, dir);
        let scale = w.len();
        if scale <= 0.0 {
            return None;
        }
        let flat = Vec2::new(w.x, w.y);
        let amount = if flat.len() > 0.1 * scale { delta.dot(flat) / flat.len2() } else { delta.y / scale };
        let mut out = self.clone();
        let cylinder = out.shape_name() == "Cylinder";
        let grow = |v: &mut f64| *v += amount;
        match (&mut out.shape, dim) {
            (SolidShape::Box { length, .. } | SolidShape::Wedge { length, .. }, Dim::Length) => grow(length),
            (SolidShape::Box { width, .. } | SolidShape::Wedge { width, .. }, Dim::Width) => grow(width),
            (SolidShape::Box { height, .. } | SolidShape::Wedge { height, .. } | SolidShape::Extrusion { height, .. }, Dim::Height) => grow(height),
            (SolidShape::Frustum { base_radius, top_radius, .. }, Dim::Radius) if cylinder => {
                grow(base_radius);
                grow(top_radius);
            }
            (SolidShape::Frustum { base_radius, .. } | SolidShape::Pyramid { base_radius, .. }, Dim::Radius) => grow(base_radius),
            (SolidShape::Frustum { top_radius, .. } | SolidShape::Pyramid { top_radius, .. }, Dim::TopRadius) => grow(top_radius),
            (SolidShape::Frustum { height, .. } | SolidShape::Pyramid { height, .. }, Dim::Height) => grow(height),
            (SolidShape::Sphere { radius } | SolidShape::Torus { radius, .. }, Dim::Radius) => grow(radius),
            (SolidShape::Torus { tube, .. }, Dim::Tube) => grow(tube),
            _ => return None,
        }
        out.is_valid().then_some(out)
    }

    /// A tolerance for drawing the solid as a whole (a 4000th of its size): what files store
    /// and EXPLODE makes, and what picking and 3D extents use.
    pub fn mesh_tol(&self) -> f64 {
        let leaves = self.leaves();
        let rings: Vec<Rings> = leaves.iter().map(|(_, l)| l.rings(f64::MAX)).collect();
        let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for q in rings.iter().flat_map(|r| r.stations.iter().flatten().flatten()) {
            lo = Vec3::new(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
            hi = Vec3::new(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
        }
        let diag = (hi - lo).len();
        if diag.is_finite() && diag > 0.0 { diag / 4000.0 } else { 0.01 }
    }

    /// Move, turn or scale the solid.
    pub fn transform(&mut self, m: &Mat4) {
        self.xf = m.mul(&self.xf);
    }

    /// Where the solid's own origin is, and its own axes, in the drawing.
    pub fn origin(&self) -> Vec3 {
        self.xf.apply(Vec3::ZERO)
    }

    /// The direction of the solid's own `v` in the drawing.
    pub fn axis(&self, v: Vec3) -> Vec3 {
        mat_vec(&self.xf, v)
    }
}

/// The centroid of a closed loop with bulges: its chord polygon and the circular segments
/// the arcs add (or take away).
fn loop_centroid(l: &[PolyVertex]) -> Vec2 {
    let Some(o) = l.first().map(|v| v.p) else { return Vec2::ZERO };
    let n = l.len();
    let (mut a, mut m) = (0.0, Vec2::ZERO);
    for i in 0..n {
        let (Some(p), Some(q)) = (l.get(i), l.get((i + 1) % n)) else { continue };
        let (u, w) = (p.p - o, q.p - o);
        let t = u.cross(w) / 2.0;
        a += t;
        m += (u + w) * (t / 3.0);
        if let Some((arc, ccw)) = cadaddy_geom::bulge_to_arc(p.p, q.p, p.bulge) {
            let th = arc.sweep();
            let sa = arc.radius * arc.radius * (th - th.sin()) / 2.0;
            let sa = if ccw { sa } else { -sa };
            let d = if th - th.sin() > 1e-15 { 4.0 * arc.radius * (th / 2.0).sin().powi(3) / (3.0 * (th - th.sin())) } else { 0.0 };
            let c = arc.center + Vec2::from_angle(arc.start + th / 2.0) * d - o;
            a += sa;
            m += c * sa;
        }
    }
    if a.abs() < 1e-300 { o } else { o + m * (1.0 / a) }
}

fn loops_ok(loops: &[Vec<PolyVertex>]) -> bool {
    loops
        .iter()
        .all(|l| l.len() >= 2 && l.len() <= MAX_LOOP_POINTS && l.iter().all(|v| v.p.is_finite() && v.bulge.is_finite() && v.bulge.abs() <= 1e6))
}

/// A profile extruded along +Z by `h`, its loops offset inward by `h · tan(taper)` at the top.
fn prism(loops: Vec<(Vec<Vec2>, Vec<bool>)>, h: f64, taper: f64) -> Rings {
    let d = h * taper.tan();
    let bottom: Vec<Vec<Vec3>> = loops.iter().map(|(p, _)| p.iter().map(|q| Vec3::new(q.x, q.y, 0.0)).collect()).collect();
    let top: Vec<Vec<Vec3>> = loops.iter().map(|(p, _)| offset_loop(p, d).into_iter().map(|q| Vec3::new(q.x, q.y, h)).collect()).collect();
    Rings {
        stations: vec![bottom, top],
        wrap: false,
        caps: true,
        section_wires: vec![0, 1],
        long_wires: loops.iter().map(|(_, c)| c.clone()).collect(),
    }
}

/// A closed loop moved inward by `d` (to its left for a counter-clockwise loop), each vertex along
/// its corner's bisector (mitred, at most 4·d out).
fn offset_loop(p: &[Vec2], d: f64) -> Vec<Vec2> {
    if d == 0.0 {
        return p.to_vec();
    }
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
            let n1 = (b - a).normalized().perp();
            let n2 = (c - b).normalized().perp();
            let bis = n1 + n2;
            let len = bis.len();
            if !(len.is_finite() && len > 1e-9) {
                return b + n1 * d;
            }
            let bis = bis * (1.0 / len);
            let k = (1.0 / bis.dot(n1).max(0.25)).min(4.0);
            b + bis * (d * k)
        })
        .collect()
}

/// Where the profile is at one point of a sweep path.
struct SweepFrame {
    o: Vec3,
    /// The profile's X and Y axes.
    x: Vec3,
    y: Vec3,
    /// At a bend the profile lies in the mitre plane, stretched by `stretch` along `bend` (the
    /// direction the path turns) so the solid keeps its section on both sides.
    bend: Vec3,
    stretch: f64,
}

/// Frames along a sweep path: the first square to the path; the rest turned as little as
/// possible from one point to the next.
fn sweep_frames(path: &[Vec3]) -> Vec<SweepFrame> {
    let n = path.len();
    let dirs = |i: usize| -> (Vec3, Vec3) {
        let prev = if i > 0 { (path[i] - path[i - 1]).normalized() } else { Vec3::ZERO };
        let next = if i + 1 < n { (path[i + 1] - path[i]).normalized() } else { Vec3::ZERO };
        (prev, next)
    };
    let mut out = Vec::with_capacity(n);
    let (p0, n0) = dirs(0);
    let (mut x, _) = frame_for(if n0.len() > 0.0 { n0 } else { p0 });
    for (i, o) in path.iter().enumerate() {
        let (prev, next) = dirs(i);
        let t = prev + next;
        let t = if t.len() > 1e-12 {
            t.normalized()
        } else if next.len() > 0.0 {
            next
        } else {
            prev
        };
        // Remove the part of the previous X along the new tangent.
        let xx = (x - t * x.dot(t)).normalized();
        if xx.is_finite() && xx.len() > 0.5 {
            x = xx;
        }
        let y = t.cross(x);
        let (mut bend, mut stretch) = (Vec3::ZERO, 1.0);
        if prev.len() > 0.0 && next.len() > 0.0 {
            let b = next - prev;
            let b = (b - t * b.dot(t)).normalized();
            let c = prev.dot(t);
            if b.len() > 0.5 && c > 0.25 {
                bend = b;
                stretch = 1.0 / c;
            }
        }
        out.push(SweepFrame { o: *o, x, y, bend, stretch });
    }
    out
}

/// Loft stations: each section resampled to the same number of points by length, starting at the
/// point nearest the previous section's start and turning the same way.
fn loft_rings(sections: &[Vec<Vec3>]) -> Rings {
    let n = sections.iter().map(Vec::len).max().unwrap_or(3).clamp(16, 256);
    let mut stations: Vec<Vec<Vec<Vec3>>> = Vec::new();
    let mut prev_start: Option<Vec3> = None;
    let mut prev_normal: Option<Vec3> = None;
    for s in sections {
        let mut pts = resample(s, n);
        let normal = pts.iter().enumerate().fold(Vec3::ZERO, |acc, (i, p)| acc + p.cross(pts[(i + 1) % pts.len()]));
        if let Some(pn) = prev_normal
            && pn.dot(normal) < 0.0
        {
            pts.reverse();
        }
        if let Some(ps) = prev_start
            && let Some(k) = (0..pts.len()).min_by(|a, b| (pts[*a] - ps).len().total_cmp(&(pts[*b] - ps).len()))
        {
            pts.rotate_left(k);
        }
        prev_start = pts.first().copied();
        prev_normal = Some(pts.iter().enumerate().fold(Vec3::ZERO, |acc, (i, p)| acc + p.cross(pts[(i + 1) % pts.len()])));
        stations.push(vec![pts]);
    }
    let last = stations.len().saturating_sub(1);
    Rings {
        stations,
        wrap: false,
        caps: true,
        section_wires: (0..=last).collect(),
        long_wires: vec![(0..n).map(|k| k % (n / 4).max(1) == 0).collect()],
    }
}

/// `n` points evenly along a closed loop.
fn resample(s: &[Vec3], n: usize) -> Vec<Vec3> {
    let m = s.len();
    let lens: Vec<f64> = (0..m).map(|i| (s[(i + 1) % m] - s[i]).len()).collect();
    let total: f64 = lens.iter().sum();
    if !(total.is_finite() && total > 0.0) {
        return vec![s.first().copied().unwrap_or(Vec3::ZERO); n];
    }
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut acc) = (0usize, 0.0);
    for k in 0..n {
        let target = total * k as f64 / n as f64;
        while seg < m && acc + lens[seg] < target {
            acc += lens[seg];
            seg += 1;
        }
        let i = seg.min(m - 1);
        let t = if lens[i] > 0.0 { ((target - acc) / lens[i]).clamp(0.0, 1.0) } else { 0.0 };
        out.push(s[i] + (s[(i + 1) % m] - s[i]) * t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs()))
    }

    fn near(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-12)
    }

    #[test]
    fn primitives_are_exact_and_the_surfaces_agree() {
        let tol = 1e-4;
        let cases = [
            (SolidShape::Box { length: 2.0, width: 3.0, height: 4.0 }, 24.0, 52.0),
            (SolidShape::Wedge { length: 3.0, width: 2.0, height: 4.0 }, 12.0, 6.0 + 6.0 + 6.0 + 8.0 + 10.0),
            (SolidShape::Frustum { base_radius: 1.0, top_radius: 1.0, height: 2.0 }, 2.0 * PI, 6.0 * PI),
            (SolidShape::Frustum { base_radius: 3.0, top_radius: 0.0, height: 4.0 }, 12.0 * PI, 9.0 * PI + 15.0 * PI),
            (SolidShape::Sphere { radius: 2.0 }, 32.0 / 3.0 * PI, 16.0 * PI),
            (SolidShape::Torus { radius: 5.0, tube: 1.0 }, 10.0 * PI * PI, 20.0 * PI * PI),
            (SolidShape::Pyramid { sides: 4, base_radius: 2f64.sqrt(), top_radius: 0.0, height: 3.0 }, 4.0, 4.0 + 4.0 * (10f64).sqrt()),
        ];
        for (shape, v, a) in cases {
            let s = Solid3d::new(shape.clone());
            let p = s.props(tol);
            assert!(close(p.volume, v), "{shape:?}: volume {} vs {v}", p.volume);
            assert!(close(p.area, a), "{shape:?}: area {} vs {a}", p.area);
            // The surfaces give the same within the tessellation's error.
            let (nv, na, _) = s.numeric_props(tol);
            assert!(near(nv, v, 2e-3) && near(na, a, 2e-3), "{shape:?}: surfaces {nv} {na}");
        }
    }

    #[test]
    fn placement_moves_and_scales_exactly() {
        let mut s = Solid3d::new(SolidShape::Box { length: 1.0, width: 1.0, height: 1.0 });
        s.transform(&placement(Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 2.0, 0.0), Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0)));
        let p = s.props(0.01);
        assert!(close(p.volume, 8.0) && close(p.area, 24.0));
        assert!((p.centroid - Vec3::new(9.0, 1.0, 1.0)).len() < 1e-9, "{:?}", p.centroid);
        assert!((p.min - Vec3::new(8.0, 0.0, 0.0)).len() < 1e-9 && (p.max - Vec3::new(10.0, 2.0, 2.0)).len() < 1e-9);
    }

    #[test]
    fn extrusions_with_holes_and_arcs_are_exact() {
        // A 4 × 4 square with a circular hole of radius 1, extruded 2.
        let sq = vec![
            PolyVertex::new(Vec2::new(0.0, 0.0)),
            PolyVertex::new(Vec2::new(4.0, 0.0)),
            PolyVertex::new(Vec2::new(4.0, 4.0)),
            PolyVertex::new(Vec2::new(0.0, 4.0)),
        ];
        let hole = vec![
            PolyVertex { p: Vec2::new(1.0, 2.0), bulge: 1.0, start_width: 0.0, end_width: 0.0 },
            PolyVertex { p: Vec2::new(3.0, 2.0), bulge: 1.0, start_width: 0.0, end_width: 0.0 },
        ];
        let s = Solid3d::new(SolidShape::Extrusion { loops: vec![sq, hole], height: 2.0, taper: 0.0 });
        let p = s.props(1e-4);
        assert!(close(p.volume, (16.0 - PI) * 2.0), "{}", p.volume);
        assert!(close(p.area, 2.0 * (16.0 - PI) + (16.0 + TAU) * 2.0), "{}", p.area);
        assert!((p.centroid - Vec3::new(2.0, 2.0, 1.0)).len() < 1e-9);
        let (nv, _, _) = s.numeric_props(1e-4);
        assert!(near(nv, p.volume, 1e-3), "{nv}");
        // Tapered: a frustum of a pyramid (computed from the surfaces).
        let t = Solid3d::new(SolidShape::Extrusion {
            loops: vec![vec![
                PolyVertex::new(Vec2::new(0.0, 0.0)),
                PolyVertex::new(Vec2::new(4.0, 0.0)),
                PolyVertex::new(Vec2::new(4.0, 4.0)),
                PolyVertex::new(Vec2::new(0.0, 4.0)),
            ]],
            height: 1.0,
            taper: 45f64.to_radians(),
        });
        // Top 2 × 2: V = h/3 (16 + 4 + 8).
        assert!(near(t.props(1e-4).volume, 28.0 / 3.0, 1e-9), "{}", t.props(1e-4).volume);
    }

    #[test]
    fn revolutions_sweeps_and_lofts_follow_pappus_and_friends() {
        // A unit square at x = 2..3 revolved fully: V = 2π · 2.5 · 1.
        let sq = vec![
            PolyVertex::new(Vec2::new(2.0, 0.0)),
            PolyVertex::new(Vec2::new(3.0, 0.0)),
            PolyVertex::new(Vec2::new(3.0, 1.0)),
            PolyVertex::new(Vec2::new(2.0, 1.0)),
        ];
        let r = Solid3d::new(SolidShape::Revolution { loops: vec![sq.clone()], angle: TAU });
        assert!(near(r.props(1e-4).volume, TAU * 2.5, 1e-3), "{}", r.props(1e-4).volume);
        // Half way round: half of it.
        let h = Solid3d::new(SolidShape::Revolution { loops: vec![sq.clone()], angle: PI });
        assert!(near(h.props(1e-4).volume, PI * 2.5, 1e-3));
        // A square swept along a straight path of 5: 5.
        let unit = vec![
            PolyVertex::new(Vec2::new(-0.5, -0.5)),
            PolyVertex::new(Vec2::new(0.5, -0.5)),
            PolyVertex::new(Vec2::new(0.5, 0.5)),
            PolyVertex::new(Vec2::new(-0.5, 0.5)),
        ];
        let sw = Solid3d::new(SolidShape::Sweep { loops: vec![unit], path: vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0)] });
        assert!(near(sw.props(1e-4).volume, 5.0, 1e-9), "{}", sw.props(1e-4).volume);
        // A loft between two equal squares 3 apart: a box of 3 (whatever the start points).
        let sqz = |z: f64, rot: usize| {
            let mut v = vec![Vec3::new(0.0, 0.0, z), Vec3::new(1.0, 0.0, z), Vec3::new(1.0, 1.0, z), Vec3::new(0.0, 1.0, z)];
            v.rotate_left(rot);
            v
        };
        let lf = Solid3d::new(SolidShape::Loft { sections: vec![sqz(0.0, 0), sqz(3.0, 2)] });
        assert!(near(lf.props(1e-4).volume, 3.0, 1e-6), "{}", lf.props(1e-4).volume);
    }

    #[test]
    fn grips_set_dimensions() {
        let mut c = Solid3d::new(SolidShape::Frustum { base_radius: 2.0, top_radius: 2.0, height: 5.0 });
        c.transform(&placement(Vec3::new(10.0, 10.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        let g = c.grip_points();
        assert_eq!(g.len(), 3);
        assert!((g[1] - Vec3::new(12.0, 10.0, 0.0)).len() < 1e-12);
        // The radius out by 1 (a cylinder keeps one radius); the height (straight up, seen from
        // above) by the drag in Y.
        let wider = c.grip_moved(1, Vec2::new(1.0, 0.0)).unwrap();
        assert!(matches!(wider.shape, SolidShape::Frustum { base_radius, top_radius, .. } if base_radius == 3.0 && top_radius == 3.0));
        let taller = c.grip_moved(2, Vec2::new(0.0, 2.0)).unwrap();
        assert!(matches!(taller.shape, SolidShape::Frustum { height, .. } if height == 7.0));
        // A cone's top radius has its own grip.
        let cone = Solid3d::new(SolidShape::Frustum { base_radius: 2.0, top_radius: 1.0, height: 5.0 });
        let top = cone.grip_moved(2, Vec2::new(0.5, 0.0)).unwrap();
        assert!(matches!(top.shape, SolidShape::Frustum { base_radius, top_radius, .. } if base_radius == 2.0 && top_radius == 1.5));
        // Not through zero, not the base grip, not a grip that isn't there.
        assert!(
            c.grip_moved(1, Vec2::new(-5.0, 0.0)).is_none()
                && c.grip_moved(0, Vec2::new(1.0, 0.0)).is_none()
                && c.grip_moved(9, Vec2::new(1.0, 0.0)).is_none()
        );
    }

    #[test]
    fn invalid_definitions_draw_nothing() {
        for shape in [
            SolidShape::Box { length: -1.0, width: 1.0, height: 1.0 },
            SolidShape::Sphere { radius: f64::NAN },
            SolidShape::Torus { radius: 1.0, tube: 2.0 },
            SolidShape::Pyramid { sides: 2, base_radius: 1.0, top_radius: 0.0, height: 1.0 },
            SolidShape::Extrusion { loops: vec![], height: 1.0, taper: 0.0 },
            SolidShape::Revolution { loops: vec![vec![PolyVertex::new(Vec2::ZERO)]], angle: 1.0 },
            SolidShape::Loft { sections: vec![vec![Vec3::ZERO; 3]] },
        ] {
            let s = Solid3d::new(shape);
            assert!(!s.is_valid() && s.rings(0.1).stations.is_empty() && s.wires(0.1).is_empty());
        }
        assert_eq!(circle_segments(f64::INFINITY, 0.1), 16);
        assert_eq!(circle_segments(1.0, 0.0) % 4, 0);
    }
}
