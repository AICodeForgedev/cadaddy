//! Screen-space ambient occlusion and the sampling pattern of soft shadows, for the Realistic
//! visual style. The GPU shader (`ui-egui/src/shaded/shader.wgsl`: `occlusion`, `sun_visibility`)
//! runs the same steps with the same constants, so both paths show the same image.
//!
//! Views are orthographic: a pixel is `px` drawing units across, and depth (larger nearer the
//! viewer) is in drawing units too. For each pixel the occlusion looks at [`AO_TAPS`] neighbours
//! on a disc of [`ao_pixels`] pixels round it: a neighbour that stands above the pixel's tangent
//! plane, within the reach (fading out by twice it), shades it (the more, the steeper it rises).

/// Neighbours each pixel's ambient occlusion looks at.
pub const AO_TAPS: usize = 16;
/// Samples of the soft shadows' blocker search and filter.
pub const SOFT_TAPS: usize = 16;
/// Most pixels the occlusion's disc reaches (a close view shades only nearby pixels).
pub const AO_MAX_PX: f64 = 96.0;
/// Most shadow-map texels a penumbra spans (radius).
pub const SOFT_MAX_TEXELS: f64 = 24.0;
/// How far above the tangent plane (as a cosine) a neighbour must rise to shade: flat surfaces
/// and gentle curves stay open.
pub const AO_BIAS: f64 = 0.1;

/// The `k`-th of `n` points of a golden-angle spiral filling the unit disc evenly.
pub fn disc(k: usize, n: usize) -> (f64, f64) {
    const GOLDEN: f64 = 2.399_963_23;
    let a = k as f64 * GOLDEN;
    let r = ((k as f64 + 0.5) / n.max(1) as f64).sqrt();
    (a.cos() * r, a.sin() * r)
}

/// The occlusion's disc radius in pixels for a reach of `reach` drawing units at `px` units a
/// pixel.
pub fn ao_pixels(reach: f64, px: f64) -> f64 {
    let r = reach / px.max(1e-300);
    if r.is_finite() { r.clamp(0.0, AO_MAX_PX) } else { 0.0 }
}

/// How open each pixel of a `w` × `h` depth buffer is to the sky (1 open … 0 enclosed);
/// background pixels (depth −∞) are 1. `normal(i)` is pixel `i`'s surface normal in view
/// coordinates (right, up, towards the viewer). `strength` scales the darkening (0 … 2).
pub fn occlusion(
    depth: &[f32],
    normal: impl Fn(usize) -> Option<[f64; 3]> + Sync,
    w: usize,
    h: usize,
    px: f64,
    reach: f64,
    strength: f32,
) -> Vec<f32> {
    let mut open = vec![1.0f32; w * h];
    let r_px = ao_pixels(reach, px);
    if !(strength > 0.0 && r_px >= 1.0 && px.is_finite() && px > 0.0) || depth.len() < w * h {
        return open;
    }
    let taps: Vec<(f64, f64)> = (0..AO_TAPS).map(|k| disc(k, AO_TAPS)).map(|(x, y)| (x * r_px, y * r_px)).collect();
    let row = |(y, out): (usize, &mut [f32])| {
        for (x, o) in out.iter_mut().enumerate() {
            let i = y * w + x;
            let z = depth.get(i).copied().unwrap_or(f32::NEG_INFINITY);
            if !z.is_finite() {
                continue;
            }
            let Some(n) = normal(i) else { continue };
            *o = open_at(depth, w, h, x, y, f64::from(z), n, px, reach, &taps, strength);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        open.par_chunks_mut(w.max(1)).enumerate().for_each(row);
    }
    #[cfg(target_arch = "wasm32")]
    open.chunks_mut(w.max(1)).enumerate().for_each(row);
    open
}

#[allow(clippy::too_many_arguments)]
fn open_at(
    depth: &[f32],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    z: f64,
    n: [f64; 3],
    px: f64,
    reach: f64,
    taps: &[(f64, f64)],
    strength: f32,
) -> f32 {
    let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
    let mut occ = 0.0;
    for &(ox, oy) in taps {
        let (sx, sy) = ((cx + ox).floor(), (cy + oy).floor());
        if sx < 0.0 || sy < 0.0 || sx >= w as f64 || sy >= h as f64 {
            continue;
        }
        let (dx, dy) = (sx - x as f64, sy - y as f64);
        if dx == 0.0 && dy == 0.0 {
            continue;
        }
        let zs = depth.get(sy as usize * w + sx as usize).copied().unwrap_or(f32::NEG_INFINITY);
        if !zs.is_finite() {
            continue;
        }
        // From the pixel to its neighbour, in drawing units (screen y runs down).
        let d = [dx * px, -dy * px, f64::from(zs) - z];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if !(len > 1e-12 && len.is_finite()) {
            continue;
        }
        let rise = ((n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / len - AO_BIAS).max(0.0);
        // Full weight within the reach, none beyond twice it (no halos from what stands far off).
        let fall = (2.0 - len / reach).clamp(0.0, 1.0);
        occ += rise * fall;
    }
    // A crease can hide at most half the disc: that half, fully risen, closes it.
    (1.0 - f64::from(strength) * 2.0 * occ / AO_TAPS as f64).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor (depth 0) beside a block 4 units high from column 20 on, seen from straight above.
    fn corner(w: usize, h: usize) -> Vec<f32> {
        (0..w * h).map(|i| if i % w >= 20 { 4.0 } else { 0.0 }).collect()
    }

    #[test]
    fn a_flat_floor_stays_open() {
        let (w, h) = (40, 30);
        let depth = vec![0.0f32; w * h];
        let open = occlusion(&depth, |_| Some([0.0, 0.0, 1.0]), w, h, 1.0, 8.0, 1.0);
        assert!(open.iter().all(|o| (*o - 1.0).abs() < 1e-6));
    }

    #[test]
    fn a_floor_darkens_next_to_a_wall_and_not_far_from_it() {
        let (w, h) = (40, 30);
        let depth = corner(w, h);
        let open = occlusion(&depth, |_| Some([0.0, 0.0, 1.0]), w, h, 1.0, 8.0, 1.0);
        let at = |x: usize| open[15 * w + x];
        assert!(at(19) < 0.9, "next to the wall: {}", at(19));
        assert!(at(19) < at(16) && at(16) <= at(13), "{} {} {}", at(19), at(16), at(13));
        assert!((at(5) - 1.0).abs() < 1e-6, "far from it: {}", at(5));
        // Stronger darkens more; zero is off.
        let strong = occlusion(&depth, |_| Some([0.0, 0.0, 1.0]), w, h, 1.0, 8.0, 2.0);
        // The block's top is open: nothing rises above it.
        assert!((open[15 * w + 30] - 1.0).abs() < 1e-6);
        // Something tall far above the floor (beyond the reach) doesn't shade it.
        let tall: Vec<f32> = (0..w * h).map(|i| if i % w >= 20 { 50.0 } else { 0.0 }).collect();
        let far = occlusion(&tall, |_| Some([0.0, 0.0, 1.0]), w, h, 1.0, 8.0, 1.0);
        assert!(far[15 * w + 19] > at(19));
        assert!(strong[15 * w + 19] < at(19));
        let off = occlusion(&depth, |_| Some([0.0, 0.0, 1.0]), w, h, 1.0, 8.0, 0.0);
        assert!(off.iter().all(|o| *o == 1.0));
    }

    #[test]
    fn hostile_sizes_do_not_panic() {
        let depth = vec![f32::NAN, f32::INFINITY, 0.0, f32::NEG_INFINITY];
        for (px, reach) in [(f64::NAN, 1.0), (0.0, 1.0), (1.0, f64::INFINITY), (1e-300, 1e300), (1.0, -5.0)] {
            let o = occlusion(&depth, |_| Some([0.0, 0.0, 1.0]), 2, 2, px, reach, 1.0);
            assert_eq!(o.len(), 4);
            assert!(o.iter().all(|v| v.is_finite()));
        }
        // A buffer shorter than the size claims.
        assert_eq!(occlusion(&[0.0], |_| None, 5, 5, 1.0, 3.0, 1.0).len(), 25);
    }

    #[test]
    fn the_disc_fills_the_unit_circle() {
        for k in 0..AO_TAPS {
            let (x, y) = disc(k, AO_TAPS);
            assert!(x * x + y * y <= 1.0 + 1e-9);
        }
        assert!(disc(0, 0).0.is_finite());
    }
}
