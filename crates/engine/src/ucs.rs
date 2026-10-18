//! The current user coordinate system (UCS) of a drawing, in the plan: an origin and a rotation
//! of the X axis about Z. It lives in the drawing header as AutoCAD keeps it ($UCSORG, $UCSXDIR,
//! $UCSYDIR, $UCSNAME), so it is saved with the drawing. Typed coordinates, Ortho, polar
//! tracking, the snap grid and PLAN work in it; objects are always stored in world coordinates.

use cadaddy_doc::{Drawing, HVal, Ucs};
use cadaddy_geom::{Vec2, Vec3};

/// A UCS in the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub origin: Vec2,
    /// The X axis's angle from the world X axis (radians, counterclockwise).
    pub angle: f64,
    /// The named UCS it is (empty: unnamed; "World" is never stored).
    pub name: String,
}

impl Default for Frame {
    fn default() -> Self {
        Frame::WORLD
    }
}

impl Frame {
    pub const WORLD: Frame = Frame { origin: Vec2::ZERO, angle: 0.0, name: String::new() };

    pub fn new(origin: Vec2, angle: f64) -> Frame {
        Frame { origin, angle: cadaddy_geom::norm_angle(angle), name: String::new() }
    }

    /// The current UCS of `d` (the world when the header holds none, or one that isn't usable
    /// in the plan: an X axis without length, or tilted out of the XY plane).
    pub fn current(d: &Drawing) -> Frame {
        let pt = |k: &str| match d.header.get(k) {
            Some(HVal::Point(p)) if p.is_finite() => Some(*p),
            _ => None,
        };
        let (Some(o), Some(x)) = (pt("UCSORG"), pt("UCSXDIR")) else { return Frame::WORLD };
        if x.xy().len() <= 1e-9 || x.z.abs() > 1e-9 {
            return Frame::WORLD;
        }
        let name = d.header.get("UCSNAME").and_then(HVal::as_str).unwrap_or_default().to_string();
        Frame { origin: o.xy(), angle: cadaddy_geom::norm_angle(x.y.atan2(x.x)), name }
    }

    /// Make this the current UCS of `d`.
    pub fn make_current(&self, d: &mut Drawing) {
        let (x, y) = self.axes();
        d.header.set("UCSORG", HVal::Point(self.origin.to3(0.0)));
        d.header.set("UCSXDIR", HVal::Point(x.to3(0.0)));
        d.header.set("UCSYDIR", HVal::Point(y.to3(0.0)));
        d.header.set("UCSNAME", HVal::Str(self.name.clone()));
    }

    /// The world coordinate system (origin at 0,0, world axes).
    pub fn is_world(&self) -> bool {
        self.origin.len() <= 1e-12 && cadaddy_geom::norm_angle(self.angle).min(std::f64::consts::TAU - cadaddy_geom::norm_angle(self.angle)) <= 1e-12
    }

    /// The X and Y axes in world coordinates.
    pub fn axes(&self) -> (Vec2, Vec2) {
        let x = Vec2::from_angle(self.angle);
        (x, x.perp())
    }

    /// A point given in this UCS, in world coordinates.
    pub fn to_wcs(&self, p: Vec2) -> Vec2 {
        self.origin + p.rotate(self.angle)
    }

    /// A world point in this UCS.
    pub fn to_ucs(&self, p: Vec2) -> Vec2 {
        (p - self.origin).rotate(-self.angle)
    }

    /// A direction given in this UCS, in world coordinates.
    pub fn dir_to_wcs(&self, v: Vec2) -> Vec2 {
        v.rotate(self.angle)
    }

    /// As a named UCS table record.
    pub fn record(&self, name: &str) -> Ucs {
        let (x, y) = self.axes();
        Ucs { name: name.to_string(), origin: self.origin.to3(0.0), x_axis: x.to3(0.0), y_axis: y.to3(0.0) }
    }

    /// A named UCS table record as a UCS in the plan; `None` when it is tilted out of the plan.
    pub fn from_record(u: &Ucs) -> Option<Frame> {
        let ok = |v: Vec3| v.is_finite() && v.z.abs() <= 1e-9 && v.xy().len() > 1e-9;
        if !(ok(u.x_axis) && u.origin.is_finite()) {
            return None;
        }
        Some(Frame { origin: u.origin.xy(), angle: cadaddy_geom::norm_angle(u.x_axis.y.atan2(u.x_axis.x)), name: u.name.clone() })
    }
}

impl crate::Session {
    /// The current UCS of the active drawing (the world without one).
    pub fn ucs(&self) -> Frame {
        self.doc().map(Frame::current).unwrap_or_default()
    }

    /// A typed point (`x,y`, `@dx,dy`, `@d<a`, `*x,y`…) read in the current UCS, in world
    /// coordinates: what the command line does with it.
    pub fn parse_typed_point(&self, text: &str) -> Option<Vec2> {
        crate::prompt::parse_point_in(text, self.last_point, &self.angle_settings(), &self.ucs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let f = Frame::new(Vec2::new(10.0, 5.0), 30f64.to_radians());
        let p = Vec2::new(3.0, 4.0);
        assert!(f.to_ucs(f.to_wcs(p)).near(p, 1e-12));
        assert!(f.to_wcs(Vec2::new(2.0, 0.0)).near(Vec2::new(10.0 + 3f64.sqrt(), 6.0), 1e-12));
        let mut d = Drawing::new_metric();
        assert!(Frame::current(&d).is_world());
        f.make_current(&mut d);
        let g = Frame::current(&d);
        assert!(g.origin.near(f.origin, 1e-12) && (g.angle - f.angle).abs() < 1e-12);
        assert!(Frame::from_record(&f.record("A")).is_some_and(|r| (r.angle - f.angle).abs() < 1e-12 && r.name == "A"));
        d.header.set("UCSXDIR", HVal::Point(Vec3::new(0.0, 0.0, 1.0)));
        assert!(Frame::current(&d).is_world(), "a tilted UCS isn't used in the plan");
    }
}
