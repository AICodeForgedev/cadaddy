//! Orthographic 3D views: the view direction (AutoCAD's VIEWDIR, from the target towards the
//! viewer) and the projection of world points onto the view plane.
//!
//! The view plane has its own 2D coordinates: `right` and `up` are unit world vectors, so a
//! point projects to `(p · right, p · up)`. The plan view (looking down the Z axis) projects
//! (x, y, z) to (x, y): nothing changes for 2D drawing.

use crate::{Mat3, Vec2, Vec3};

/// AutoCAD's preset views (VIEW ▸ Orthographic and the ViewCube), with their view directions.
pub const PRESETS: &[(&str, Vec3)] = &[
    ("top", Vec3::new(0.0, 0.0, 1.0)),
    ("bottom", Vec3::new(0.0, 0.0, -1.0)),
    ("front", Vec3::new(0.0, -1.0, 0.0)),
    ("back", Vec3::new(0.0, 1.0, 0.0)),
    ("left", Vec3::new(-1.0, 0.0, 0.0)),
    ("right", Vec3::new(1.0, 0.0, 0.0)),
    ("swiso", Vec3::new(-1.0, -1.0, 1.0)),
    ("seiso", Vec3::new(1.0, -1.0, 1.0)),
    ("neiso", Vec3::new(1.0, 1.0, 1.0)),
    ("nwiso", Vec3::new(-1.0, 1.0, 1.0)),
];

/// The preset view direction called `name` (case-insensitive; "sw iso" and "SW Isometric" too).
pub fn preset(name: &str) -> Option<Vec3> {
    let key: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let key = key.strip_suffix("metric").unwrap_or(&key);
    PRESETS.iter().find(|(n, _)| *n == key).map(|(_, d)| *d)
}

/// The preset name of `dir`, if it is one ("Top", "SW Isometric", …), as AutoCAD labels it.
pub fn preset_label(dir: Vec3) -> Option<&'static str> {
    let d = dir.normalized();
    let labels = ["Top", "Bottom", "Front", "Back", "Left", "Right", "SW Isometric", "SE Isometric", "NE Isometric", "NW Isometric"];
    PRESETS.iter().zip(labels).find(|((_, p), _)| p.normalized().dot(d) > 1.0 - 1e-9).map(|(_, l)| l)
}

/// Whether `dir` looks straight down the Z axis (the plan view).
pub fn is_plan(dir: Vec3) -> bool {
    let len = dir.len();
    !(len.is_finite() && len > 1e-12) || (dir.x.abs() <= 1e-12 * len && dir.y.abs() <= 1e-12 * len && dir.z > 0.0)
}

/// An orthographic projection onto the view plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    /// Unit vector from the target towards the viewer.
    pub dir: Vec3,
    /// World direction of the view plane's +X (screen right).
    pub right: Vec3,
    /// World direction of the view plane's +Y (screen up).
    pub up: Vec3,
}

impl Projection {
    /// The projection looking along `-dir`, with world Z pointing up on screen (AutoCAD's
    /// default twist); looking straight up or down, world Y points up. `None` for the plan view
    /// and for a direction that isn't a usable vector: plan views draw without a projection.
    pub fn new(dir: Vec3) -> Option<Projection> {
        if is_plan(dir) {
            return None;
        }
        let dir = dir.normalized();
        if !dir.is_finite() {
            return None;
        }
        // Near-vertical views take world Y as their up hint.
        let hint = if dir.x.abs() < 1e-9 && dir.y.abs() < 1e-9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::Z };
        let right = hint.cross(dir).normalized();
        let up = dir.cross(right).normalized();
        (right.is_finite() && up.is_finite()).then_some(Projection { dir, right, up })
    }

    /// The plan view turned by a view twist of `twist` radians (counterclockwise: the drawing
    /// turns that way on screen). `None` for no twist (or one that isn't finite): the plain
    /// plan view draws without a projection.
    pub fn plan_twist(twist: f64) -> Option<Projection> {
        let t = crate::norm_angle(twist);
        if !t.is_finite() || t.abs() < 1e-12 || (t - std::f64::consts::TAU).abs() < 1e-12 {
            return None;
        }
        let (sin, cos) = t.sin_cos();
        Some(Projection { dir: Vec3::Z, right: Vec3::new(cos, -sin, 0.0), up: Vec3::new(sin, cos, 0.0) })
    }

    /// The view twist of a plan projection (0 for a 3D view).
    pub fn twist(&self) -> f64 {
        if is_plan(self.dir) { (-self.right.y).atan2(self.right.x) } else { 0.0 }
    }

    /// The view direction from an azimuth (radians, from +X towards +Y, of the viewer seen from
    /// above) and an elevation (radians above the XY plane), as 3DORBIT changes them.
    pub fn dir_from_angles(azimuth: f64, elevation: f64) -> Vec3 {
        let e = elevation.clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
        Vec3::new(e.cos() * azimuth.cos(), e.cos() * azimuth.sin(), e.sin())
    }

    /// Azimuth and elevation of `dir` (see [`Projection::dir_from_angles`]).
    pub fn angles(dir: Vec3) -> (f64, f64) {
        let d = dir.normalized();
        (d.y.atan2(d.x), d.z.clamp(-1.0, 1.0).asin())
    }

    /// `p` on the view plane.
    pub fn project(&self, p: Vec3) -> Vec2 {
        Vec2::new(p.dot(self.right), p.dot(self.up))
    }

    /// The map from the plane at height `z` (world x, y) to the view plane: 2D geometry drawn
    /// at elevation `z` is this affine image of its plan.
    pub fn plane_xf(&self, z: f64) -> Mat3 {
        let o = self.project(Vec3::new(0.0, 0.0, z));
        Mat3 { a: self.right.x, b: self.up.x, c: self.right.y, d: self.up.y, e: o.x, f: o.y }
    }

    /// The world point at height `z` that projects to `q`; `None` when that plane is seen
    /// edge-on (a front or side view of the XY plane).
    pub fn unproject(&self, q: Vec2, z: f64) -> Option<Vec2> {
        let m = self.plane_xf(z);
        // An almost edge-on plane maps clicks to points far away: refuse instead.
        if m.det().abs() < 1e-6 {
            return None;
        }
        m.inverse().map(|inv| inv.apply(q))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn plan_twist_turns_the_drawing_counterclockwise() {
        assert!(Projection::plan_twist(0.0).is_none());
        assert!(Projection::plan_twist(std::f64::consts::TAU).is_none());
        assert!(Projection::plan_twist(f64::NAN).is_none());
        let p = Projection::plan_twist(std::f64::consts::FRAC_PI_2).unwrap_or(Projection { dir: Vec3::Z, right: Vec3::Z, up: Vec3::Z });
        // World +X points up on screen; the plane map round-trips.
        assert!(close(p.project(Vec3::new(1.0, 0.0, 0.0)), Vec2::new(0.0, 1.0)));
        assert!(close(p.unproject(Vec2::new(0.0, 1.0), 0.0).unwrap_or(Vec2::ZERO), Vec2::new(1.0, 0.0)));
        assert!((p.twist() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!(is_plan(p.dir));
    }

    #[test]
    fn plan_view_has_no_projection() {
        assert!(Projection::new(Vec3::Z).is_none());
        assert!(Projection::new(Vec3::new(0.0, 0.0, 5.0)).is_none());
        assert!(Projection::new(Vec3::ZERO).is_none());
        assert!(Projection::new(Vec3::new(f64::NAN, 0.0, 1.0)).is_none());
    }

    #[test]
    fn front_view_shows_x_right_and_z_up() {
        let p = Projection::new(preset("front").unwrap_or(Vec3::Z)).unwrap_or(Projection { dir: Vec3::Z, right: Vec3::Z, up: Vec3::Z });
        assert!(close(p.project(Vec3::new(3.0, 7.0, 5.0)), Vec2::new(3.0, 5.0)));
    }

    #[test]
    fn right_view_shows_y_right_and_z_up() {
        let p = Projection::new(Vec3::new(1.0, 0.0, 0.0)).unwrap_or(Projection { dir: Vec3::Z, right: Vec3::Z, up: Vec3::Z });
        assert!(close(p.project(Vec3::new(9.0, 2.0, 4.0)), Vec2::new(2.0, 4.0)));
    }

    #[test]
    fn bottom_view_mirrors_x() {
        let p = Projection::new(Vec3::new(0.0, 0.0, -1.0)).unwrap_or(Projection { dir: Vec3::Z, right: Vec3::Z, up: Vec3::Z });
        assert!(close(p.project(Vec3::new(2.0, 3.0, 0.0)), Vec2::new(-2.0, 3.0)));
    }

    #[test]
    fn plane_xf_matches_project_and_unproject_inverts_it() {
        let p = Projection::new(preset("SW Isometric").unwrap_or(Vec3::Z)).unwrap_or(Projection { dir: Vec3::Z, right: Vec3::Z, up: Vec3::Z });
        let w = Vec3::new(2.5, -1.0, 4.0);
        assert!(close(p.plane_xf(4.0).apply(w.xy()), p.project(w)));
        assert!(p.unproject(p.project(w), 4.0).is_some_and(|q| close(q, w.xy())));
        // The XY plane is edge-on in a front view: no click maps onto it.
        let front = Projection::new(Vec3::new(0.0, -1.0, 0.0)).unwrap_or(p);
        assert!(front.unproject(Vec2::new(1.0, 1.0), 0.0).is_none());
    }

    #[test]
    fn presets_parse_and_label() {
        assert_eq!(preset("_SWISO"), preset("sw iso"));
        assert_eq!(preset("SW Isometric"), Some(Vec3::new(-1.0, -1.0, 1.0)));
        assert_eq!(preset_label(Vec3::new(2.0, 2.0, 2.0)), Some("NE Isometric"));
        assert_eq!(preset_label(Vec3::new(0.1, 2.0, 2.0)), None);
        assert!(preset("diagonal").is_none());
    }

    #[test]
    fn orbit_angles_round_trip() {
        let d = Projection::dir_from_angles(1.0, 0.4);
        let (az, el) = Projection::angles(d);
        assert!((az - 1.0).abs() < 1e-12 && (el - 0.4).abs() < 1e-12);
    }
}
