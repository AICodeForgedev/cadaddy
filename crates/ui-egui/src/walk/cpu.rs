//! Walk mode drawn by the CPU, when there is no usable GPU (no wgpu device, a GPU error, or the
//! CPU forced: see `gpucaps`). The same picture as the GPU's shader in a simpler form: triangles
//! clipped at the near plane in clip space, a depth buffer, the shader's lighting and fog, the
//! cut height, one-sided ceilings, and the edges drawn a hair in front.
//!
//! Clipping against the near plane in homogeneous coordinates follows I. E. Sutherland and
//! G. W. Hodgman, "Reentrant Polygon Clipping", CACM 17(1), 1974; filling uses edge functions
//! (J. Pineda, "A Parallel Algorithm for Polygon Rasterization", SIGGRAPH 1988).

use cadaddy_walk3d::{Mat4, Scene, V3, Walker};

/// Most pixels the CPU draws a frame at (the view is scaled up to its size).
pub const MAX_PIXELS: u32 = 640 * 400;

/// A point in clip space with its world position.
#[derive(Clone, Copy)]
struct Cv {
    c: [f64; 4],
    w: [f64; 3],
}

fn clip(m: &Mat4, p: [f32; 3]) -> Cv {
    let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
    let r = |i: usize| m.0[i][0] * x + m.0[i][1] * y + m.0[i][2] * z + m.0[i][3];
    Cv { c: [r(0), r(1), r(2), r(3)], w: [x, y, z] }
}

fn lerp(a: &Cv, b: &Cv, t: f64) -> Cv {
    let l = |x: f64, y: f64| x + (y - x) * t;
    Cv {
        c: [l(a.c[0], b.c[0]), l(a.c[1], b.c[1]), l(a.c[2], b.c[2]), l(a.c[3], b.c[3])],
        w: [l(a.w[0], b.w[0]), l(a.w[1], b.w[1]), l(a.w[2], b.w[2])],
    }
}

/// The polygon's part in front of the near plane (clip z ≥ 0, w > 0).
fn near_clip(poly: &[Cv]) -> Vec<Cv> {
    let mut out = Vec::new();
    let d = |v: &Cv| v.c[2].min(v.c[3] - 1e-6);
    for (i, p) in poly.iter().enumerate() {
        let q = &poly[(i + poly.len() - 1) % poly.len()];
        let (dp, dq) = (d(p), d(q));
        if (dp >= 0.0) != (dq >= 0.0) {
            out.push(lerp(q, p, dq / (dq - dp)));
        }
        if dp >= 0.0 {
            out.push(*p);
        }
    }
    out
}

fn srgb_to_lin(c: u8) -> f64 {
    let v = f64::from(c) / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn lin_to_srgb(v: f64) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// The frame: `size` pixels (call with a size within [`MAX_PIXELS`]), as sRGB RGBA.
pub fn render(scene: &Scene, walker: &Walker, size: (u32, u32), cut_z: f32) -> egui::ColorImage {
    let (w, h) = (size.0.max(1) as usize, size.1.max(1) as usize);
    let aspect = w as f64 / h as f64;
    let m = walker.view_proj(aspect);
    let eye = walker.eye;
    let light = V3 { x: 0.35, y: 0.75, z: 0.55 }.norm();
    let bg = [0.63, 0.69, 0.76];
    let fog_col = [srgb_to_lin(209), srgb_to_lin(217), srgb_to_lin(227)];
    let mut color = vec![[lin_to_srgb(bg[0]), lin_to_srgb(bg[1]), lin_to_srgb(bg[2]), 255u8]; w * h];
    let mut depth = vec![f64::INFINITY; w * h];
    let fog = |col: [f64; 3], p: [f64; 3]| {
        let d = ((p[0] - eye.x).powi(2) + (p[1] - eye.y).powi(2) + (p[2] - eye.z).powi(2)).sqrt();
        let f = (d / 400.0).clamp(0.0, 1.0) * 0.65;
        [col[0] + (fog_col[0] - col[0]) * f, col[1] + (fog_col[1] - col[1]) * f, col[2] + (fog_col[2] - col[2]) * f]
    };
    let to_screen = |v: &Cv| {
        let iw = 1.0 / v.c[3];
        ((v.c[0] * iw + 1.0) * 0.5 * w as f64, (1.0 - v.c[1] * iw) * 0.5 * h as f64, v.c[2] * iw)
    };
    let cut = f64::from(cut_z);
    for t in scene.tris.as_chunks::<3>().0.iter().take(5_000_000) {
        let (a, b, c) = (&t[0], &t[1], &t[2]);
        // The cut and one-sided ceilings, per triangle (the shader decides per fragment).
        let zmin = f64::from(a.p[2].min(b.p[2]).min(c.p[2]));
        if zmin > cut {
            continue;
        }
        let n = V3 { x: f64::from(a.n[0]), y: f64::from(a.n[1]), z: f64::from(a.n[2]) }.norm();
        if n.z < -0.9 && eye.z > zmin {
            continue;
        }
        let lit = 0.38 + 0.62 * n.dot(light).abs();
        let base = [srgb_to_lin(a.c[0]) * lit, srgb_to_lin(a.c[1]) * lit, srgb_to_lin(a.c[2]) * lit];
        let poly = near_clip(&[clip(&m, a.p), clip(&m, b.p), clip(&m, c.p)]);
        if poly.len() < 3 {
            continue;
        }
        let pts: Vec<((f64, f64, f64), [f64; 3])> = poly.iter().map(|v| (to_screen(v), v.w)).collect();
        for k in 1..pts.len() - 1 {
            let (p0, p1, p2) = (pts[0], pts[k], pts[k + 1]);
            let area = (p1.0.0 - p0.0.0) * (p2.0.1 - p0.0.1) - (p2.0.0 - p0.0.0) * (p1.0.1 - p0.0.1);
            if area.abs() < 1e-12 {
                continue;
            }
            let (x0, x1) = (p0.0.0.min(p1.0.0).min(p2.0.0).floor().max(0.0) as usize, p0.0.0.max(p1.0.0).max(p2.0.0).ceil().min(w as f64) as usize);
            let (y0, y1) = (p0.0.1.min(p1.0.1).min(p2.0.1).floor().max(0.0) as usize, p0.0.1.max(p1.0.1).max(p2.0.1).ceil().min(h as f64) as usize);
            // Fogged at the triangle's middle (cheaper than per pixel, close enough at walking scale).
            let mid = [(p0.1[0] + p1.1[0] + p2.1[0]) / 3.0, (p0.1[1] + p1.1[1] + p2.1[1]) / 3.0, (p0.1[2] + p1.1[2] + p2.1[2]) / 3.0];
            let col = fog(base, mid);
            let rgba = [lin_to_srgb(col[0]), lin_to_srgb(col[1]), lin_to_srgb(col[2]), 255];
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let e = |a: (f64, f64, f64), b: (f64, f64, f64)| (b.0 - a.0) * (py - a.1) - (px - a.0) * (b.1 - a.1);
                    let (w0, w1, w2) = (e(p1.0, p2.0) / area, e(p2.0, p0.0) / area, e(p0.0, p1.0) / area);
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * p0.0.2 + w1 * p1.0.2 + w2 * p2.0.2;
                    let i = y * w + x;
                    if let (Some(dz), Some(px)) = (depth.get_mut(i), color.get_mut(i))
                        && z < *dz
                    {
                        *dz = z;
                        *px = rgba;
                    }
                }
            }
        }
    }
    // Edges, a hair in front.
    for e in scene.edges.as_chunks::<2>().0.iter().take(5_000_000) {
        let (a, b) = (&e[0], &e[1]);
        if f64::from(a.p[2].min(b.p[2])) > cut {
            continue;
        }
        let seg = near_clip(&[clip(&m, a.p), clip(&m, b.p)]);
        let [p, q] = match seg.as_slice() {
            [p, q] | [p, q, _] => [*p, *q],
            _ => continue,
        };
        let (sp, sq) = (to_screen(&p), to_screen(&q));
        let col = fog([srgb_to_lin(a.c[0]), srgb_to_lin(a.c[1]), srgb_to_lin(a.c[2])], p.w);
        let rgba = [lin_to_srgb(col[0]), lin_to_srgb(col[1]), lin_to_srgb(col[2]), 255];
        let steps = ((sq.0 - sp.0).abs().max((sq.1 - sp.1).abs()).ceil() as usize).clamp(1, 8192);
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let (x, y, z) = (sp.0 + (sq.0 - sp.0) * t, sp.1 + (sq.1 - sp.1) * t, sp.2 + (sq.2 - sp.2) * t - 0.00015);
            if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
                continue;
            }
            let i = y as usize * w + x as usize;
            if let (Some(dz), Some(px)) = (depth.get_mut(i), color.get_mut(i))
                && z <= *dz
            {
                *dz = z;
                *px = rgba;
            }
        }
    }
    let flat: Vec<u8> = color.into_iter().flatten().collect();
    egui::ColorImage::from_rgba_unmultiplied([w, h], &flat)
}

/// A view's size scaled down to [`MAX_PIXELS`], keeping its shape.
pub fn frame_size(size: (u32, u32)) -> (u32, u32) {
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let k = (f64::from(MAX_PIXELS) / (w * h)).sqrt().min(1.0);
    (((w * k).floor() as u32).max(1), ((h * k).floor() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_is_drawn_in_front_of_the_sky() {
        // A wall face 10' ahead, across the view.
        let mut s = Scene::default();
        let v = |x: f32, y: f32, z: f32| cadaddy_walk3d::Vertex { p: [x, y, z], n: [-1.0, 0.0, 0.0], c: [200, 40, 40, 255] };
        s.tris.extend([
            v(10.0, -50.0, -10.0),
            v(10.0, 50.0, -10.0),
            v(10.0, 50.0, 30.0),
            v(10.0, -50.0, -10.0),
            v(10.0, 50.0, 30.0),
            v(10.0, -50.0, 30.0),
        ]);
        let mut w = Walker::spawn(&s, 0);
        w.eye = V3 { x: 0.0, y: 0.0, z: 5.5 };
        w.yaw = 0.0;
        w.pitch = 0.0;
        let img = render(&s, &w, (64, 40), 1000.0);
        let centre = img.pixels[20 * 64 + 32];
        assert!(centre.r() > centre.b() + 40, "the red wall fills the view: {centre:?}");
        // Behind the walker: sky only.
        w.yaw = std::f64::consts::PI;
        let img = render(&s, &w, (64, 40), 1000.0);
        let c = img.pixels[20 * 64 + 32];
        assert!(c.b() > c.r(), "sky: {c:?}");
        // Cut away below the eye: the wall is gone.
        w.yaw = 0.0;
        let img = render(&s, &w, (64, 40), -20.0);
        assert!(img.pixels[20 * 64 + 32].b() > img.pixels[20 * 64 + 32].r());
        assert_eq!(frame_size((1280, 800)), (640, 400));
        assert_eq!(frame_size((320, 200)), (320, 200));
    }
}
