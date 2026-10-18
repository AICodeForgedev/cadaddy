//! Dimensions of a picked device, as a field measure would take them: its height above the floor,
//! and its distances to the walls around it (along its own wall to the nearest crossing walls for
//! a wall device; to the nearest wall east–west and north–south for a ceiling device).

use crate::derive::{Scene, Seg};
use crate::math::{V3, v3};
use cadaddy_doc::geom::Vec2;

/// Walls further than this (feet) are not measured to.
const REACH_FT: f64 = 80.0;
/// A wall-mounted device's host wall is within this (feet) of its centre.
const HOST_FT: f64 = 1.0;
/// A wall whose end stops this close (feet) to the measuring line still counts as met.
const BUTT_FT: f64 = 0.15;
/// Pieces of one face this close (feet) end to end are one face.
const JOINT_FT: f64 = 0.05;
/// Joints followed along one face (hostile input).
const MAX_JOINTS: usize = 500;

/// `w` lies on the same line as `s` (parallel, and its ends on `s`'s line).
fn collinear(s: &Seg, w: &Seg) -> bool {
    let d = s.b - s.a;
    let l = d.len();
    if l < 1e-9 {
        return false;
    }
    let off = |p: Vec2| ((p - s.a).x * d.y - (p - s.a).y * d.x).abs() / l;
    off(w.a) < 0.02 && off(w.b) < 0.02
}

/// One dimension: a line from `a` to `b` (feet, world) and its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dim {
    pub a: V3,
    pub b: V3,
    pub feet: f64,
    pub kind: DimKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimKind {
    /// Floor to the device's centre.
    Height,
    /// Device centre to a wall.
    ToWall,
}

/// The dimensions of device `i` (empty when there is no such device).
pub fn measure(scene: &Scene, i: usize) -> Vec<Dim> {
    let Some(d) = scene.devices.get(i) else { return Vec::new() };
    let Some(level) = scene.levels.get(d.level) else { return Vec::new() };
    let walls: Vec<&Seg> = scene.walls.iter().filter(|w| w.level == d.level).collect();
    let p = Vec2 { x: d.pos.x, y: d.pos.y };
    let mut out = vec![Dim { a: v3(p.x, p.y, level.elevation_ft), b: d.pos, feet: d.pos.z - level.elevation_ft, kind: DimKind::Height }];
    let host = if d.ceiling { None } else { nearest(&walls, p).filter(|(_, dist)| *dist <= HOST_FT).map(|(s, _)| s) };
    // A wall device measures along its wall's own face (where the walls butting into it end); a
    // ceiling device from where it hangs.
    // Along a wall, the face itself also ends: at a corner, a door jamb or an opening (pieces of
    // one straight face drawn end to end are followed through their joints).
    let face_end = |dir: Vec2| -> Option<f64> {
        let s = host?;
        let q = crate::derive::closest(s.a, s.b, p).0;
        let along = |v: Vec2| (v - q).x * dir.x + (v - q).y * dir.y;
        let mut end = if along(s.a) > along(s.b) { s.a } else { s.b };
        if along(end) <= 0.02 {
            return None;
        }
        for _ in 0..MAX_JOINTS {
            let next = walls.iter().filter(|w| collinear(s, w)).find_map(|w| {
                let (near, far) = if along(w.a) < along(w.b) { (w.a, w.b) } else { (w.b, w.a) };
                ((near - end).len() <= JOINT_FT && along(far) > along(end) + 0.01).then_some(far)
            });
            match next {
                Some(f) => end = f,
                None => break,
            }
        }
        Some(along(end))
    };
    let (from, dirs): (Vec2, Vec<Vec<Vec2>>) = match host {
        // Along the host wall, both ways.
        Some(s) => {
            let t = s.b - s.a;
            let t = t * (1.0 / t.len().max(1e-12));
            (crate::derive::closest(s.a, s.b, p).0, vec![vec![t], vec![t * -1.0]])
        }
        // The nearest wall on each axis.
        None => (p, vec![vec![Vec2 { x: 1.0, y: 0.0 }, Vec2 { x: -1.0, y: 0.0 }], vec![Vec2 { x: 0.0, y: 1.0 }, Vec2 { x: 0.0, y: -1.0 }]]),
    };
    for group in dirs {
        let best = group
            .iter()
            .filter_map(|dir| {
                let hit = [ray(&walls, from, *dir), face_end(*dir)].into_iter().flatten().min_by(f64::total_cmp);
                hit.map(|s| (s, *dir))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((s, dir)) = best {
            let q = p + dir * s;
            out.push(Dim { a: d.pos, b: v3(q.x, q.y, d.pos.z), feet: s, kind: DimKind::ToWall });
        }
    }
    out
}

/// The wall segment nearest `p`, and its distance.
fn nearest<'a>(walls: &[&'a Seg], p: Vec2) -> Option<(&'a Seg, f64)> {
    walls.iter().map(|s| (*s, crate::derive::closest(s.a, s.b, p).1)).min_by(|x, y| x.1.total_cmp(&y.1))
}

/// Distance from `p` along unit `dir` to the first wall it meets (walls running along the ray are
/// not met; one ending within [`BUTT_FT`] of the ray, as a wall butting into the measured face
/// does, is), within [`REACH_FT`].
pub(crate) fn ray(walls: &[&Seg], p: Vec2, dir: Vec2) -> Option<f64> {
    let mut best: Option<f64> = None;
    for s in walls {
        let e = s.b - s.a;
        let el = e.len();
        if el < 1e-9 {
            continue;
        }
        let denom = dir.x * e.y - dir.y * e.x;
        // Nearly parallel: the host wall and its far face.
        if denom.abs() / el < 0.2 {
            continue;
        }
        let w = s.a - p;
        let t = (w.x * e.y - w.y * e.x) / denom;
        let u = (w.x * dir.y - w.y * dir.x) / denom;
        let slack = BUTT_FT / el;
        if t > 0.02 && t <= REACH_FT && (-slack..=1.0 + slack).contains(&u) && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{DeviceInfo, LevelInfo};
    use cadaddy_doc::Handle;

    /// A 20' × 10' room on a level at 12', with a receptacle on the east wall and a light.
    fn room() -> Scene {
        let p = |x: f64, y: f64| Vec2 { x, y };
        let mut s = Scene::default();
        for (a, b) in [(p(0., 0.), p(20., 0.)), (p(20., 0.), p(20., 10.)), (p(20., 10.), p(0., 10.)), (p(0., 10.), p(0., 0.))] {
            s.walls.push(Seg { a, b, level: 0 });
        }
        s.levels.push(LevelInfo { name: "L2".into(), elevation_ft: 12.0, height_ft: 10.0, ceiling_ft: 8.0, min: p(0., 0.), max: p(20., 10.) });
        let dev = |x: f64, y: f64, z: f64, ceiling: bool| DeviceInfo {
            handle: Handle(1),
            src: 0,
            block: "B".into(),
            level: 0,
            pos: v3(x, y, z),
            aff_ft: z - 12.0,
            ceiling,
            attrs: Vec::new(),
        };
        s.devices.push(dev(19.925, 3.0, 13.5, false));
        s.devices.push(dev(6.0, 4.0, 19.9, true));
        s
    }

    #[test]
    fn a_wall_device_measures_along_its_wall_and_to_the_floor() {
        let d = measure(&room(), 0);
        assert_eq!(d.len(), 3, "{d:?}");
        assert!(d.iter().any(|x| x.kind == DimKind::Height && (x.feet - 1.5).abs() < 1e-9));
        let mut along: Vec<f64> = d.iter().filter(|x| x.kind == DimKind::ToWall).map(|x| x.feet).collect();
        along.sort_by(f64::total_cmp);
        assert!((along[0] - 3.0).abs() < 1e-9 && (along[1] - 7.0).abs() < 1e-9, "{along:?}");
    }

    #[test]
    fn a_wall_butting_into_the_host_wall_is_measured_to() {
        let mut s = room();
        // A partition from the west wall to the east wall's face at y = 5 (it stops at x = 20).
        s.walls.push(Seg { a: Vec2 { x: 0.0, y: 5.0 }, b: Vec2 { x: 20.0, y: 5.0 }, level: 0 });
        let mut along: Vec<f64> = measure(&s, 0).iter().filter(|x| x.kind == DimKind::ToWall).map(|x| x.feet).collect();
        along.sort_by(f64::total_cmp);
        assert!((along[0] - 2.0).abs() < 1e-9 && (along[1] - 3.0).abs() < 1e-9, "{along:?}");
    }

    #[test]
    fn along_a_wall_its_face_ending_at_a_jamb_counts() {
        let mut s = room();
        // The east wall's face stops 1' short of the north wall (a doorway): 6' from the device.
        if let Some(w) = s.walls.get_mut(1) {
            w.b = Vec2 { x: 20.0, y: 9.0 };
        }
        let mut along: Vec<f64> = measure(&s, 0).iter().filter(|x| x.kind == DimKind::ToWall).map(|x| x.feet).collect();
        along.sort_by(f64::total_cmp);
        assert!((along[0] - 3.0).abs() < 1e-9 && (along[1] - 6.0).abs() < 1e-9, "{along:?}");
    }

    #[test]
    fn a_face_drawn_in_pieces_is_followed_to_its_real_end() {
        let mut s = room();
        // The east wall drawn as three pieces, with a doorway from y 9 to 10.
        s.walls.retain(|w| !(w.a.x == 20.0 && w.b.x == 20.0));
        for (a, b) in [(0.0, 2.0), (2.0, 4.5), (4.5, 9.0)] {
            s.walls.push(Seg { a: Vec2 { x: 20.0, y: a }, b: Vec2 { x: 20.0, y: b }, level: 0 });
        }
        let mut along: Vec<f64> = measure(&s, 0).iter().filter(|x| x.kind == DimKind::ToWall).map(|x| x.feet).collect();
        along.sort_by(f64::total_cmp);
        assert!((along[0] - 3.0).abs() < 1e-9 && (along[1] - 6.0).abs() < 1e-9, "{along:?}");
    }

    #[test]
    fn a_ceiling_device_measures_to_the_nearest_wall_on_each_axis() {
        let d = measure(&room(), 1);
        let to: Vec<f64> = d.iter().filter(|x| x.kind == DimKind::ToWall).map(|x| x.feet).collect();
        assert_eq!(to.len(), 2);
        assert!((to[0] - 6.0).abs() < 1e-9 && (to[1] - 4.0).abs() < 1e-9, "{to:?}");
        assert!(measure(&room(), 9).is_empty());
    }
}
