//! The walker on a ground model: up stairs (risers no higher than its step), down them, along a
//! ramp, stopped by a step too high, off an edge and down under gravity, and onto the next level.

use cadaddy_doc::geom::Vec2;
use cadaddy_walk3d::{Ground, LevelInfo, Scene, V3, Vertex, Walker};

/// A level rectangle at height `z` (two upward triangles).
fn slab(s: &mut Scene, x0: f64, y0: f64, x1: f64, y1: f64, z0: f64, z1: f64) {
    // z0 at x0, z1 at x1: a ramp when they differ.
    let p = |x: f64, y: f64| {
        let t = if (x1 - x0).abs() > 1e-12 { (x - x0) / (x1 - x0) } else { 0.0 };
        [x as f32, y as f32, (z0 + (z1 - z0) * t) as f32]
    };
    let v = |p: [f32; 3]| Vertex { p, n: [0.0, 0.0, 1.0], c: [128, 128, 128, 255] };
    for q in [p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y0), p(x1, y1), p(x0, y1)] {
        s.tris.push(v(q));
    }
    s.owners.push(Default::default());
    s.owners.push(Default::default());
}

fn level(name: &str, z: f64) -> LevelInfo {
    LevelInfo {
        name: name.into(),
        elevation_ft: z,
        height_ft: 10.0,
        ceiling_ft: 8.0,
        min: Vec2 { x: -50.0, y: -50.0 },
        max: Vec2 { x: 100.0, y: 50.0 },
        base: Vec2 { x: 0.0, y: 0.0 },
    }
}

/// A walker at (x, 0) on the floor at `z`, facing east.
fn walker(s: &Scene, x: f64, z: f64) -> Walker {
    let mut w = Walker::spawn(s, 0);
    w.eye = V3 { x, y: 0.0, z: z + w.eye_height };
    w.yaw = 0.0;
    w
}

/// Walk east `ft` feet in small steps, letting gravity act between them.
fn walk(w: &mut Walker, s: &Scene, g: &Ground, ft: f64) {
    let n = (ft / 0.25).ceil() as usize;
    for _ in 0..n {
        w.step_on(s, Some(g), ft / n as f64, 0.0, 0.0);
        w.fall(s, Some(g), 0.05);
    }
}

#[test]
fn stairs_up_and_down_and_onto_the_next_level() {
    let mut s = {
        let mut sc = Scene::default();
        sc.levels = vec![level("L1", 0.0), level("L2", 6.0)];
        sc
    };
    slab(&mut s, -20.0, -5.0, 0.0, 5.0, 0.0, 0.0);
    // Ten 0.6' risers on 1' treads, 0 → 6'.
    for i in 0..10 {
        let z = 0.6 * f64::from(i + 1);
        slab(&mut s, f64::from(i), -5.0, f64::from(i + 1), 5.0, z, z);
    }
    slab(&mut s, 10.0, -5.0, 30.0, 5.0, 6.0, 6.0);
    let g = Ground::new(&s);
    let mut w = walker(&s, -3.0, 0.0);
    walk(&mut w, &s, &g, 18.0);
    assert!((w.feet() - 6.0).abs() < 1e-6 && w.eye.x > 14.0, "up the stairs: feet {} at x {}", w.feet(), w.eye.x);
    assert_eq!(w.level, 1, "on the upper level");
    // And back down.
    w.yaw = std::f64::consts::PI;
    walk(&mut w, &s, &g, 18.0);
    assert!(w.feet().abs() < 1e-6 && w.eye.x < -2.0, "down again: feet {} at x {}", w.feet(), w.eye.x);
    assert_eq!(w.level, 0);
}

#[test]
fn a_ramp_is_followed_and_a_step_too_high_stops_the_walker() {
    let mut s = {
        let mut sc = Scene::default();
        sc.levels = vec![level("L1", 0.0)];
        sc
    };
    slab(&mut s, -10.0, -5.0, 0.0, 5.0, 0.0, 0.0);
    slab(&mut s, 0.0, -5.0, 20.0, 5.0, 0.0, 2.0);
    slab(&mut s, 20.0, -5.0, 30.0, 5.0, 2.0, 2.0);
    // A 3' high box on the landing.
    slab(&mut s, 25.0, -5.0, 27.0, 5.0, 5.0, 5.0);
    let g = Ground::new(&s);
    let mut w = walker(&s, -2.0, 0.0);
    walk(&mut w, &s, &g, 12.0);
    assert!((w.feet() - 1.0).abs() < 0.05, "halfway up the ramp: {}", w.feet());
    walk(&mut w, &s, &g, 30.0);
    assert!(w.eye.x < 25.0 && (w.feet() - 2.0).abs() < 1e-6, "stopped by the box: x {} feet {}", w.eye.x, w.feet());
}

#[test]
fn off_an_edge_the_walker_falls_and_lands() {
    let mut s = {
        let mut sc = Scene::default();
        sc.levels = vec![level("L1", 0.0)];
        sc
    };
    slab(&mut s, -10.0, -5.0, 0.0, 5.0, 6.0, 6.0);
    slab(&mut s, 0.0, -5.0, 20.0, 5.0, 0.0, 0.0);
    let g = Ground::new(&s);
    let mut w = walker(&s, -2.0, 6.0);
    for _ in 0..12 {
        w.step_on(&s, Some(&g), 0.25, 0.0, 0.0);
    }
    assert!(w.eye.x > 0.5 && (w.feet() - 6.0).abs() < 1e-6, "walked off, not yet fallen");
    let mut t = 0.0;
    while !w.fall(&s, Some(&g), 0.05) && t < 5.0 {
        t += 0.05;
    }
    assert!(w.feet().abs() < 1e-6 && w.vz == 0.0, "landed: {}", w.feet());
    assert!(t > 0.4 && t < 1.0, "a 6' fall takes about 0.6 s: {t}");
    // Flying: no gravity.
    let mut f = walker(&s, 5.0, 3.0);
    f.fly = true;
    assert!(f.fall(&s, Some(&g), 0.5) && (f.feet() - 3.0).abs() < 1e-9);
}

#[test]
fn walls_still_stop_the_walker_on_the_ground() {
    let mut s = {
        let mut sc = Scene::default();
        sc.levels = vec![level("L1", 0.0)];
        sc
    };
    slab(&mut s, -10.0, -5.0, 20.0, 5.0, 0.0, 0.0);
    s.walls.push(cadaddy_walk3d::Seg { a: Vec2 { x: 5.0, y: -5.0 }, b: Vec2 { x: 5.0, y: 5.0 }, level: 0 });
    let g = Ground::new(&s);
    let mut w = walker(&s, 0.0, 0.0);
    walk(&mut w, &s, &g, 10.0);
    assert!(w.eye.x < 5.0, "{}", w.eye.x);
    // Hostile moves change nothing.
    let before = w.clone();
    w.step_on(&s, Some(&g), f64::NAN, 0.0, 0.0);
    assert_eq!(w, before);
}
