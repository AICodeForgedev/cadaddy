//! The BIM view's look ("Shaded with edges" as BIM tools show a building): solid faces lit by
//! a sun and the sky, soft ambient occlusion in the corners, crisp dark edges; see-through
//! surfaces (a ghosted shell, glass) blended over what lies behind with faint edges; line
//! objects on top.
//!
//! The view is orthographic ([`ShadeView`]); depth is larger nearer the viewer.

use cadaddy_color::Rgb;
use cadaddy_geom::Vec3;

use crate::images::Raster;
use crate::shade::{Scene, ShadeView, Tri};

/// How the BIM view lights and draws a scene.
#[derive(Clone, Copy, Debug)]
pub struct BimShade {
    /// Towards the sun (world).
    pub sun: Vec3,
    /// How strongly the sun lights a face square to it (0 … 1).
    pub sun_strength: f64,
    /// Ambient occlusion strength (0 off … 2).
    pub ao: f32,
    /// How far the occlusion reaches, drawing units (`None`: a 60th of the scene's size).
    pub ao_reach: Option<f64>,
    /// Draw the surfaces' edges.
    pub edges: bool,
    /// The ink the see-through surfaces' faint edges are mixed from (opaque surfaces' edges are
    /// their own colour darkened, [`EDGE_SHADE`]).
    pub ink: Rgb,
}

impl Default for BimShade {
    fn default() -> Self {
        BimShade { sun: Vec3::new(-0.45, -0.6, 0.66), sun_strength: 0.5, ao: 1.0, ao_reach: None, edges: true, ink: Rgb(38, 38, 42) }
    }
}

/// How dark an opaque surface's edges are: its colour times this.
pub const EDGE_SHADE: f64 = 0.45;
/// How much a see-through surface's edge tints what is behind it: a building's thousands of
/// ghosted walls overlap, and stronger edges pile up into a grey mass hiding the MEP inside.
pub const GHOST_EDGE: f64 = 0.2;

/// Most pixels a BIM view draws (beyond it the image is refused: a 1 × 1 raster).
pub const MAX_PIXELS: usize = 64 << 20;

/// Opacity of triangle `t` in `scene` (1 without a material).
fn opacity(scene: &Scene, t: &Tri) -> f64 {
    scene.surfaces.get(t.mat as usize).map_or(1.0, |m| f64::from(m.opacity).clamp(0.0, 1.0))
}

/// How bright a face with world normal `n` (facing the viewer) is: the sky (brighter facing
/// up) plus the sun.
fn light(n: Vec3, o: &BimShade, sun: Vec3) -> f64 {
    let sky = 0.52 + 0.18 * n.z;
    let direct = o.sun_strength.clamp(0.0, 1.0) * n.dot(sun).max(0.0);
    (sky + direct).clamp(0.0, 1.25)
}

fn shade_rgb(c: Rgb, k: f64) -> [f64; 3] {
    [f64::from(c.0) * k, f64::from(c.1) * k, f64::from(c.2) * k]
}

/// A screen triangle: pixel corners, the triangle's world normal facing the viewer, colour,
/// opacity.
struct ScreenTri {
    s: [(f64, f64, f64); 3],
    n: Vec3,
    color: Rgb,
    alpha: f64,
}

/// Each pixel the triangle covers in the box `x0..x1`, `y0..y1`: `(index, depth)`.
fn cover(s: &[(f64, f64, f64); 3], w: usize, h: usize, mut f: impl FnMut(usize, f32)) {
    let [(x0, y0, z0), (x1, y1, z1), (x2, y2, z2)] = *s;
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if !(area.is_finite() && area.abs() > 1e-12) {
        return;
    }
    let ymin = y0.min(y1).min(y2).floor().max(0.0);
    let ymax = y0.max(y1).max(y2).ceil().min(h as f64);
    let xmin = x0.min(x1).min(x2).floor().max(0.0);
    let xmax = x0.max(x1).max(x2).ceil().min(w as f64);
    if !(ymin < ymax && xmin < xmax) {
        return;
    }
    for y in ymin as usize..ymax as usize {
        let py = y as f64 + 0.5;
        for x in xmin as usize..xmax as usize {
            let px = x as f64 + 0.5;
            let a = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) / area;
            let b = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) / area;
            let c = 1.0 - a - b;
            if a < -1e-9 || b < -1e-9 || c < -1e-9 {
                continue;
            }
            f(y * w + x, (a * z0 + b * z1 + c * z2) as f32);
        }
    }
}

/// Each pixel of the segment `a`–`b` (pixel x, y, depth): `(index, depth)`.
fn walk(a: (f64, f64, f64), b: (f64, f64, f64), w: usize, h: usize, mut f: impl FnMut(usize, f32)) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let steps = dx.abs().max(dy.abs()).ceil();
    if !steps.is_finite() || steps > 1e6 {
        return;
    }
    let n = (steps as usize).max(1);
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let (x, y, z) = (a.0 + dx * t, a.1 + dy * t, a.2 + (b.2 - a.2) * t);
        if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
            continue;
        }
        f(y as usize * w + x as usize, z as f32);
    }
}

/// Draw `scene` the BIM view's way on `bg`.
pub fn rasterize_bim(scene: &Scene, v: &ShadeView, bg: Rgb, o: &BimShade) -> Raster {
    let (w, h) = (v.width_px as usize, v.height_px as usize);
    let blank = || Raster { width: 1, height: 1, rgba: vec![bg.0, bg.1, bg.2, 255] };
    if w.saturating_mul(h) > MAX_PIXELS || w == 0 || h == 0 {
        return blank();
    }
    let sun = {
        let s = o.sun.normalized();
        if s.is_finite() { s } else { Vec3::Z }
    };
    let px = v.height / f64::from(v.height_px.max(1));
    let to_screen = |t: &Tri| -> Option<ScreenTri> {
        let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]);
        let len = n.len();
        if !(len.is_finite() && len > 1e-300) {
            return None;
        }
        let n = n * (1.0 / len);
        let n = if n.dot(v.dir) < 0.0 { -n } else { n };
        Some(ScreenTri { s: t.p.map(|p| v.screen(p)), n, color: t.color, alpha: opacity(scene, t) })
    };
    let (mut solid, mut clear): (Vec<ScreenTri>, Vec<ScreenTri>) = (Vec::new(), Vec::new());
    for t in &scene.tris {
        let Some(st) = to_screen(t) else { continue };
        if st.alpha >= 0.99 {
            solid.push(st);
        } else if st.alpha > 0.005 {
            clear.push(st);
        }
    }
    // Opaque surfaces: the nearest one at each pixel.
    let mut depth = vec![f32::NEG_INFINITY; w * h];
    let mut owner = vec![u32::MAX; w * h];
    for (i, t) in solid.iter().enumerate() {
        let id = u32::try_from(i).unwrap_or(u32::MAX);
        cover(&t.s, w, h, |k, z| {
            if let (Some(d), Some(ow)) = (depth.get_mut(k), owner.get_mut(k))
                && z > *d
            {
                *d = z;
                *ow = id;
            }
        });
    }
    // Lit, then darkened in the corners.
    let view_normal = |k: usize| -> Option<[f64; 3]> {
        let t = solid.get(*owner.get(k)? as usize)?;
        Some([t.n.dot(v.right), t.n.dot(v.up), t.n.dot(v.dir)])
    };
    let reach = o.ao_reach.unwrap_or_else(|| crate::shade::scene_diag(scene) / 60.0);
    let open = crate::ao::occlusion(&depth, view_normal, w, h, px, reach, o.ao);
    let bgf = [f64::from(bg.0), f64::from(bg.1), f64::from(bg.2)];
    let mut color: Vec<[f64; 3]> = vec![bgf; w * h];
    for (k, c) in color.iter_mut().enumerate() {
        let Some(t) = owner.get(k).and_then(|i| solid.get(*i as usize)) else { continue };
        let ao = f64::from(open.get(k).copied().unwrap_or(1.0));
        *c = shade_rgb(t.color, light(t.n, o, sun) * (0.5 + 0.5 * ao));
    }
    let range = {
        let (lo, hi) = depth.iter().filter(|z| z.is_finite()).fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), z| (a.min(*z), b.max(*z)));
        if hi > lo { f64::from(hi - lo) } else { 1.0 }
    };
    let bias = (range * 2e-3 + px * 1.5) as f32;
    // The opaque surfaces' edges, those behind a surface left out: the surface's own colour
    // darkened (a thin pipe's facets read as its colour, not as black).
    if o.edges {
        for l in scene.lines.iter().filter(|l| l.edge && !l.faint) {
            let lc = [f64::from(l.color.0) * EDGE_SHADE, f64::from(l.color.1) * EDGE_SHADE, f64::from(l.color.2) * EDGE_SHADE];
            walk(v.screen(l.a), v.screen(l.b), w, h, |k, z| {
                if depth.get(k).is_some_and(|d| z + bias >= *d)
                    && let Some(c) = color.get_mut(k)
                {
                    *c = lc;
                }
            });
        }
    }
    // See-through surfaces over what they are in front of: only the nearest one at a pixel (a
    // ghosted shell reads as one pale skin, and what is behind two walls and a roof keeps its
    // colour).
    let mut ghost: Vec<Option<(f32, [f64; 3], f64)>> = vec![None; w * h];
    for t in &clear {
        let lit = shade_rgb(t.color, light(t.n, o, sun));
        let a = t.alpha;
        cover(&t.s, w, h, |k, z| {
            if depth.get(k).is_some_and(|d| z >= *d)
                && let Some(g) = ghost.get_mut(k)
                && g.is_none_or(|(gz, _, _)| z > gz)
            {
                *g = Some((z, lit, a));
            }
        });
    }
    for (c, g) in color.iter_mut().zip(&ghost) {
        if let Some((_, lit, a)) = g {
            for (ch, l) in c.iter_mut().zip(lit) {
                *ch = *ch * (1.0 - a) + l * a;
            }
        }
    }
    // Their edges, faint.
    if o.edges {
        let faint = crate::shade::faint_ink(o.ink, bg);
        let fc = [f64::from(faint.0), f64::from(faint.1), f64::from(faint.2)];
        for l in scene.lines.iter().filter(|l| l.edge && l.faint) {
            walk(v.screen(l.a), v.screen(l.b), w, h, |k, z| {
                if depth.get(k).is_some_and(|d| z + bias >= *d)
                    && let Some(c) = color.get_mut(k)
                {
                    for (ch, f) in c.iter_mut().zip(fc) {
                        *ch = *ch * (1.0 - GHOST_EDGE) + f * GHOST_EDGE;
                    }
                }
            });
        }
    }
    // Line objects on top (behind a surface left out).
    for l in scene.lines.iter().filter(|l| !l.edge) {
        let lc = [f64::from(l.color.0), f64::from(l.color.1), f64::from(l.color.2)];
        walk(v.screen(l.a), v.screen(l.b), w, h, |k, z| {
            if depth.get(k).is_some_and(|d| z + bias >= *d)
                && let Some(c) = color.get_mut(k)
            {
                *c = lc;
            }
        });
    }
    let byte = |c: f64| c.round().clamp(0.0, 255.0) as u8;
    let rgba: Vec<u8> = color.iter().flat_map(|c| [byte(c[0]), byte(c[1]), byte(c[2]), 255]).collect();
    Raster::new(v.width_px, v.height_px, rgba).unwrap_or_else(blank)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shade::{Line3, SurfaceMat};
    use cadaddy_geom::Vec2;

    fn quad(s: &mut Scene, z: f64, c: Rgb, mat: u32) {
        let p = [Vec3::new(0.0, 0.0, z), Vec3::new(10.0, 0.0, z), Vec3::new(10.0, 10.0, z), Vec3::new(0.0, 10.0, z)];
        s.tris.push(Tri { p: [p[0], p[1], p[2]], color: c, mat, uv: [[0.0; 2]; 3] });
        s.tris.push(Tri { p: [p[0], p[2], p[3]], color: c, mat, uv: [[0.0; 2]; 3] });
        for k in 0..4 {
            s.lines.push(Line3 { a: p[k], b: p[(k + 1) % 4], color: c, edge: true, faint: mat != crate::shade::NO_SURFACE });
        }
    }

    fn top_view(w: u32) -> ShadeView {
        ShadeView::new(None, Vec2::new(5.0, 5.0), 12.0, w, w)
    }

    fn at(r: &Raster, x: u32, y: u32) -> [u8; 3] {
        let k = ((y * r.width + x) * 4) as usize;
        [r.rgba[k], r.rgba[k + 1], r.rgba[k + 2]]
    }

    #[test]
    fn a_ghost_over_a_solid_lets_it_show_through() {
        let mut s = Scene::default();
        s.surfaces.push(SurfaceMat { name: "ghost".into(), base: [0.8; 3], roughness: 1.0, metallic: 0.0, opacity: 0.2, texture: None });
        quad(&mut s, 0.0, Rgb(0, 0, 255), crate::shade::NO_SURFACE);
        quad(&mut s, 5.0, Rgb(230, 230, 230), 0);
        let r = rasterize_bim(&s, &top_view(64), Rgb(255, 255, 255), &BimShade { ao: 0.0, ..BimShade::default() });
        let c = at(&r, 32, 32);
        // Mostly the blue below, a little paler.
        assert!(c[2] > 150 && c[0] < 120, "{c:?}");
    }

    #[test]
    fn solid_edges_are_dark_and_ghost_edges_faint() {
        let mut s = Scene::default();
        quad(&mut s, 0.0, Rgb(200, 200, 200), crate::shade::NO_SURFACE);
        let r = rasterize_bim(&s, &top_view(60), Rgb(255, 255, 255), &BimShade::default());
        // The edge x = 0 sits 1/12 of the way across.
        let edge = (0..r.width).map(|x| at(&r, x, 30)).map(|c| c[0]).min().unwrap_or(255);
        assert!(edge < 110, "edges are the surface colour darkened: {edge}");
        let mut g = Scene::default();
        g.surfaces.push(SurfaceMat { name: "ghost".into(), base: [0.8; 3], roughness: 1.0, metallic: 0.0, opacity: 0.2, texture: None });
        quad(&mut g, 0.0, Rgb(200, 200, 200), 0);
        let r = rasterize_bim(&g, &top_view(60), Rgb(255, 255, 255), &BimShade::default());
        let edge = (0..r.width).map(|x| at(&r, x, 30)).map(|c| c[0]).min().unwrap_or(255);
        assert!(edge > 100, "{edge}");
    }

    #[test]
    fn hostile_views_never_panic() {
        let mut s = Scene::default();
        quad(&mut s, 0.0, Rgb(1, 2, 3), crate::shade::NO_SURFACE);
        s.tris.push(Tri::plain([Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(1e300, 1.0, 0.0), Vec3::new(0.0, -1e300, 0.0)], Rgb(9, 9, 9)));
        s.lines.push(Line3 { a: Vec3::new(-1e300, 0.0, 0.0), b: Vec3::new(1e300, 0.0, 0.0), color: Rgb(1, 1, 1), edge: false, faint: false });
        for w in [1, 2, 300] {
            let r = rasterize_bim(&s, &top_view(w), Rgb(255, 255, 255), &BimShade { ao_reach: Some(f64::NAN), ..BimShade::default() });
            assert!(r.width >= 1);
        }
        let mut v = top_view(16);
        v.height = 0.0;
        let _ = rasterize_bim(&s, &v, Rgb(0, 0, 0), &BimShade::default());
    }
}
