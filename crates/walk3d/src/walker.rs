//! First-person camera: walks on what is underfoot (floors, stair treads, ramps; up a step no
//! higher than [`Walker::max_step`], falling under gravity off higher edges), slides along walls
//! and shut doors, or flies freely.

use crate::derive::{Scene, Seg, closest};
use crate::ground::Ground;
use crate::math::{Mat4, V3, v3};
use cadaddy_doc::geom::Vec2;

/// Body radius against walls, feet.
const RADIUS: f64 = 0.9;
/// Highest step up the walker takes, and how far it steps down without falling, feet (a
/// riser is at most 7¾"; a ramp's surface moves less per sub-step).
const STEP_UP: f64 = 1.5;
/// Longest move in one step, feet (a stalled frame must not throw the walker across the building).
const MAX_STRIDE: f64 = 50.0;
/// Gravity, feet per second².
const GRAVITY: f64 = 32.2;
/// Falling no faster than this, feet per second.
const MAX_FALL: f64 = 120.0;

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
    /// The highest step walked up without a jump, feet (a stair riser is about ⅝ ft).
    pub max_step: f64,
    /// Falling speed, feet per second (0 standing).
    pub vz: f64,
}

impl Walker {
    /// Standing in the middle of a level, looking along its long side.
    pub fn spawn(scene: &Scene, level: usize) -> Walker {
        let mut w = Walker { eye: V3::default(), yaw: 0.0, pitch: 0.0, level: 0, eye_height: 5.5, fly: false, max_step: 1.0, vz: 0.0 };
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
    /// The walker stays at its level's floor height (no ground model: [`Walker::step_on`]).
    pub fn step(&mut self, scene: &Scene, fwd: f64, right: f64, up: f64) {
        self.step_on(scene, None, fwd, right, up);
    }

    /// The feet: the eye less the eye height.
    pub fn feet(&self) -> f64 {
        self.eye.z - self.eye_height
    }

    /// Move like [`Walker::step`] on `ground`: up steps no higher than [`Walker::max_step`], down
    /// steps and ramps followed, a higher edge stops the walker like a wall. Off an edge it starts
    /// falling ([`Walker::fall`]).
    pub fn step_on(&mut self, scene: &Scene, ground: Option<&Ground>, fwd: f64, right: f64, up: f64) {
        if !(fwd.is_finite() && right.is_finite() && up.is_finite()) {
            return;
        }
        let Some(ground) = ground.filter(|g| !g.is_empty() && !self.fly) else {
            return self.step_flat(scene, fwd, right, up);
        };
        let (s, c) = self.yaw.sin_cos();
        let d = Vec2 { x: c * fwd + s * right, y: s * fwd - c * right };
        let d = if d.len() > MAX_STRIDE { d * (MAX_STRIDE / d.len()) } else { d };
        let n = ((d.len() / (RADIUS * 0.5)).ceil() as usize).clamp(1, 512);
        let part = d * (1.0 / n as f64);
        let mut p = Vec2 { x: self.eye.x, y: self.eye.y };
        let mut feet = self.feet();
        for _ in 0..n {
            let q = self.collide(scene, p + part);
            // Something higher than a step but below the head (a riser too tall, a low solid)
            // stops the walker like a wall.
            if ground.below(q, feet + self.eye_height).is_some_and(|z| z > feet + self.max_step) {
                break;
            }
            // A step up or down, or a ramp: stand on it. A drop (or nothing underfoot): walk off
            // the edge; [`Walker::fall`] brings the walker down.
            if let Some(z) = ground.below(q, feet + self.max_step).filter(|z| *z >= feet - self.max_step)
                && self.vz == 0.0
            {
                feet = z;
            }
            p = q;
        }
        self.eye = v3(p.x, p.y, feet + self.eye_height);
        self.update_level(scene);
    }

    /// Fall under gravity for `dt` seconds onto what is below (or the level's floor). Returns
    /// whether the walker is standing.
    pub fn fall(&mut self, scene: &Scene, ground: Option<&Ground>, dt: f64) -> bool {
        if self.fly || !dt.is_finite() || dt <= 0.0 {
            self.vz = 0.0;
            return true;
        }
        let p = Vec2 { x: self.eye.x, y: self.eye.y };
        let feet = self.feet();
        let floor = scene.levels.iter().map(|l| l.elevation_ft).filter(|z| *z <= feet + 1e-6).fold(f64::NEG_INFINITY, f64::max);
        let below =
            ground.and_then(|g| g.below(p, feet + 1e-6)).into_iter().chain(floor.is_finite().then_some(floor)).fold(f64::NEG_INFINITY, f64::max);
        if !below.is_finite() || feet <= below + 1e-6 {
            self.vz = 0.0;
            if below.is_finite() {
                self.eye.z = below + self.eye_height;
            }
            return true;
        }
        self.vz = (self.vz - GRAVITY * dt.min(0.1)).max(-MAX_FALL);
        let next = (feet + self.vz * dt.min(0.1)).max(below);
        if next <= below + 1e-6 {
            self.vz = 0.0;
        }
        self.eye.z = next + self.eye_height;
        self.update_level(scene);
        self.vz == 0.0
    }

    /// The level whose floor the feet are on (stairs lead to the next one).
    fn update_level(&mut self, scene: &Scene) {
        let feet = self.feet();
        if let Some((i, _)) =
            scene.levels.iter().enumerate().filter(|(_, l)| l.elevation_ft <= feet + 0.5).max_by(|a, b| a.1.elevation_ft.total_cmp(&b.1.elevation_ft))
        {
            self.level = i;
        }
    }

    /// Walk at the level's floor height, sliding along walls (no ground model).
    fn step_flat(&mut self, scene: &Scene, fwd: f64, right: f64, up: f64) {
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
        let mut z = self.eye.z - self.eye_height;
        if !z.is_finite() {
            z = scene.levels.get(self.level).map(|l| l.elevation_ft).unwrap_or(0.0);
        }
        for _ in 0..n {
            p = self.collide(scene, p + part);
            z = self.ground(scene, p, z);
        }
        self.eye = v3(p.x, p.y, z + self.eye_height);
    }

    /// Where the feet go at `p` from height `z`: a stair or ramp within a step up or down (the
    /// one nearest; a floor has an opening where a stair rises through it), else a level's floor
    /// within a step, else the highest one below (stepping off a stair's side falls to it). On a
    /// level's floor the walker is on that level (its walls stop it).
    fn ground(&mut self, scene: &Scene, p: Vec2, z: f64) -> f64 {
        let floors = scene.levels.iter().enumerate().map(|(i, l)| (l.elevation_ft, Some(i)));
        let slopes = scene.slopes.iter().filter_map(|s| s.height_at(p)).map(|h| (h, None));
        let all: Vec<(f64, Option<usize>)> = slopes.chain(floors).filter(|(h, _)| h.is_finite()).collect();
        let within = |h: f64| h <= z + STEP_UP && h >= z - STEP_UP;
        let nearest =
            |stairs: bool| all.iter().filter(|(h, l)| l.is_none() == stairs && within(*h)).min_by(|a, b| (a.0 - z).abs().total_cmp(&(b.0 - z).abs()));
        let below = || all.iter().filter(|(h, _)| *h <= z + 1e-9).max_by(|a, b| a.0.total_cmp(&b.0));
        match nearest(true).or_else(|| nearest(false)).or_else(below) {
            Some((h, level)) => {
                if let Some(l) = level {
                    self.level = *l;
                }
                *h
            }
            None => z,
        }
    }

    /// Push a body at `p` out of every wall it overlaps on this level.
    fn collide(&self, scene: &Scene, mut p: Vec2) -> Vec2 {
        let shut: Vec<Seg> = scene.shut_doors().filter(|w| w.level == self.level).collect();
        for _ in 0..4 {
            let mut moved = false;
            for w in scene.walls.iter().filter(|w| w.level == self.level).chain(shut.iter()) {
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
            levels: vec![LevelInfo {
                name: "L1".into(),
                elevation_ft: 12.0,
                height_ft: 10.0,
                ceiling_ft: 8.0,
                min: p(0., 0.),
                max: p(20., 10.),
                base: p(0., 0.),
            }],
            ..Scene::default()
        }
    }

    #[test]
    fn the_walker_climbs_a_stair_to_the_next_level() {
        let mut s = room();
        let upper = LevelInfo { name: "L2".into(), elevation_ft: 22.0, ..s.levels[0].clone() };
        s.levels.push(upper);
        // A flight from (−8, −8) east 14' up to L2 at 22'.
        s.slopes.push(crate::stairs::Slope { a: Vec2 { x: -8.0, y: -8.0 }, b: Vec2 { x: 6.0, y: -8.0 }, half_width: 2.0, za: 12.0, zb: 22.0 });
        let mut w = Walker::spawn(&s, 0);
        w.eye = v3(-9.0, -8.0, 12.0 + w.eye_height);
        w.yaw = 0.0;
        for _ in 0..40 {
            w.step(&s, 0.5, 0.0, 0.0);
        }
        assert!((w.eye.z - (22.0 + w.eye_height)).abs() < 1e-9, "{:?}", w.eye);
        assert_eq!(w.level, 1);
        // Halfway back down, then off the side: it lands on L1's floor.
        w.yaw = std::f64::consts::PI;
        for _ in 0..16 {
            w.step(&s, 0.5, 0.0, 0.0);
        }
        assert!(w.eye.z > 12.0 + w.eye_height + 1.0 && w.eye.z < 22.0 + w.eye_height);
        w.step(&s, 0.0, 3.0, 0.0);
        assert!((w.eye.z - (12.0 + w.eye_height)).abs() < 1e-9, "{:?}", w.eye);
        assert_eq!(w.level, 0);
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
