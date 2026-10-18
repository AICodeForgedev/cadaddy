//! First-person camera: walks on a level's floor and slides along its walls, or flies freely.

use crate::derive::{Scene, closest};
use crate::math::{Mat4, V3, v3};
use cadaddy_doc::geom::Vec2;

/// Body radius against walls, feet.
const RADIUS: f64 = 0.9;
/// Longest move in one step, feet (a stalled frame must not throw the walker across the building).
const MAX_STRIDE: f64 = 50.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Walker {
    /// Eye position, feet.
    pub eye: V3,
    /// Heading, radians from +x (east) toward +y (north).
    pub yaw: f64,
    /// Look up (+) or down (−), radians.
    pub pitch: f64,
    pub level: usize,
    /// Eye height above the floor, feet.
    pub eye_height: f64,
    /// Fly: no floor, no walls.
    pub fly: bool,
}

impl Walker {
    /// Standing in the middle of a level, looking along its long side.
    pub fn spawn(scene: &Scene, level: usize) -> Walker {
        let mut w = Walker { eye: V3::default(), yaw: 0.0, pitch: 0.0, level: 0, eye_height: 5.5, fly: false };
        w.go_to_level(scene, level);
        w
    }

    pub fn go_to_level(&mut self, scene: &Scene, level: usize) {
        let Some(l) = scene.levels.get(level) else { return };
        self.level = level;
        let c = (l.min + l.max) * 0.5;
        let start = if self.eye == V3::default() || !self.inside(l.min, l.max) { c } else { Vec2 { x: self.eye.x, y: self.eye.y } };
        self.eye = v3(start.x, start.y, l.elevation_ft + self.eye_height);
        if (l.max.y - l.min.y) > (l.max.x - l.min.x) {
            self.yaw = std::f64::consts::FRAC_PI_2;
        }
    }

    fn inside(&self, min: Vec2, max: Vec2) -> bool {
        self.eye.x >= min.x && self.eye.x <= max.x && self.eye.y >= min.y && self.eye.y <= max.y
    }

    pub fn forward(&self) -> V3 {
        v3(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin())
    }

    /// Turn by mouse deltas (radians).
    pub fn look(&mut self, d_yaw: f64, d_pitch: f64) {
        if d_yaw.is_finite() && d_pitch.is_finite() {
            self.yaw = (self.yaw + d_yaw).rem_euclid(std::f64::consts::TAU);
            self.pitch = (self.pitch + d_pitch).clamp(-1.5, 1.5);
        }
    }

    /// Move `fwd`/`right`/`up` feet relative to the heading; walls stop a walker, not a flyer.
    pub fn step(&mut self, scene: &Scene, fwd: f64, right: f64, up: f64) {
        if !(fwd.is_finite() && right.is_finite() && up.is_finite()) {
            return;
        }
        let (s, c) = self.yaw.sin_cos();
        let d = Vec2 { x: c * fwd + s * right, y: s * fwd - c * right };
        if self.fly {
            let f = self.forward();
            self.eye = self.eye + v3(d.x, d.y, 0.0) + v3(0.0, 0.0, up + f.z * fwd);
            return;
        }
        // Sub-steps so a fast stride can't tunnel through a wall.
        let d = if d.len() > MAX_STRIDE { d * (MAX_STRIDE / d.len()) } else { d };
        let n = ((d.len() / (RADIUS * 0.5)).ceil() as usize).clamp(1, 512);
        let part = d * (1.0 / n as f64);
        let mut p = Vec2 { x: self.eye.x, y: self.eye.y };
        for _ in 0..n {
            p = self.collide(scene, p + part);
        }
        let floor = scene.levels.get(self.level).map(|l| l.elevation_ft).unwrap_or(0.0);
        self.eye = v3(p.x, p.y, floor + self.eye_height);
    }

    /// Push a body at `p` out of every wall it overlaps on this level.
    fn collide(&self, scene: &Scene, mut p: Vec2) -> Vec2 {
        for _ in 0..4 {
            let mut moved = false;
            for w in scene.walls.iter().filter(|w| w.level == self.level) {
                let (q, dist) = closest(w.a, w.b, p);
                if dist < RADIUS {
                    let away = if dist > 1e-9 {
                        (p - q) * (1.0 / dist)
                    } else {
                        Vec2 { x: -(w.b - w.a).y, y: (w.b - w.a).x } * (1.0 / (w.b - w.a).len().max(1e-9))
                    };
                    p = q + away * RADIUS;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        p
    }

    /// View-projection for an `aspect` (width / height) viewport.
    pub fn view_proj(&self, aspect: f64) -> Mat4 {
        Mat4::perspective(FOV_Y_DEG.to_radians(), sane_aspect(aspect), 0.25, 3000.0).mul(&Mat4::look(self.eye, self.forward()))
    }

    /// Direction of the ray through a viewport point given in normalised device coordinates
    /// (x right, y up, both −1..1).
    pub fn ray(&self, ndc_x: f64, ndc_y: f64, aspect: f64) -> V3 {
        let f = self.forward();
        let mut r = f.cross(v3(0.0, 0.0, 1.0));
        if r.len() < 1e-9 {
            r = v3(1.0, 0.0, 0.0);
        }
        let r = r.norm();
        let u = r.cross(f);
        let t = (FOV_Y_DEG.to_radians() / 2.0).tan();
        (f + r * (ndc_x * t * sane_aspect(aspect)) + u * (ndc_y * t)).norm()
    }
}

/// Vertical field of view, degrees.
pub const FOV_Y_DEG: f64 = 70.0;

fn sane_aspect(aspect: f64) -> f64 {
    if aspect.is_finite() && aspect > 0.01 { aspect } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{LevelInfo, Seg};

    fn room() -> Scene {
        let p = |x, y| Vec2 { x, y };
        let walls = [(p(0., 0.), p(20., 0.)), (p(20., 0.), p(20., 10.)), (p(20., 10.), p(0., 10.)), (p(0., 10.), p(0., 0.))];
        Scene {
            walls: walls.iter().map(|(a, b)| Seg { a: *a, b: *b, level: 0 }).collect(),
            levels: vec![LevelInfo { name: "L1".into(), elevation_ft: 12.0, height_ft: 10.0, ceiling_ft: 8.0, min: p(0., 0.), max: p(20., 10.) }],
            ..Scene::default()
        }
    }

    #[test]
    fn walls_stop_the_walker_but_not_a_flyer() {
        let s = room();
        let mut w = Walker::spawn(&s, 0);
        assert_eq!(w.eye, v3(10.0, 5.0, 17.5));
        w.step(&s, 100.0, 0.0, 0.0); // east, straight into the wall
        assert!(w.eye.x <= 20.0 - RADIUS + 1e-6 && w.eye.x > 18.0, "{:?}", w.eye);
        w.fly = true;
        w.step(&s, 30.0, 0.0, 0.0);
        assert!(w.eye.x > 40.0);
    }

    #[test]
    fn strafing_right_goes_clockwise_of_heading() {
        let s = room();
        let mut w = Walker::spawn(&s, 0); // facing east
        w.step(&s, 0.0, 2.0, 0.0);
        assert!((w.eye.y - 3.0).abs() < 1e-9, "{:?}", w.eye); // right of east is south
    }

    #[test]
    fn view_projection_puts_what_is_ahead_in_front() {
        let s = room();
        let w = Walker::spawn(&s, 0);
        let m = w.view_proj(1.5);
        let p = [15.0, 5.0, 17.5, 1.0];
        let clip: Vec<f64> = (0..4).map(|i| (0..4).map(|k| m.0[i][k] * p[k]).sum()).collect();
        let (x, y, z, ww) = (clip[0], clip[1], clip[2], clip[3]);
        assert!(ww > 0.0);
        assert!((x / ww).abs() < 1e-9 && (y / ww).abs() < 1e-9);
        assert!(z / ww > 0.0 && z / ww < 1.0);
        // The ray through the centre is the heading; through the right edge, rotated right.
        let c = w.ray(0.0, 0.0, 1.5);
        assert!((c - w.forward()).len() < 1e-9);
        let r = w.ray(1.0, 0.0, 1.5);
        assert!(r.y < 0.0 && r.x > 0.0);
    }
}
