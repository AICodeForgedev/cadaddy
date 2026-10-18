//! The textures of CADaddy's material library, generated from its recipes
//! ([`cadaddy_doc::material::Recipe`]): base colour, normal and roughness maps that tile
//! seamlessly, the same for the same recipe and size every time. The patterns are our own
//! (periodic value noise, brick and tile layouts, wood grain), so the library needs no image files.

use cadaddy_doc::color::Rgb;
use cadaddy_doc::material::{Pattern, Recipe, parse_hex};

/// Largest texture size generated (pixels on a side).
pub const MAX_SIZE: u32 = 2048;

/// A material's textures, each `size`×`size` RGBA8 pixels, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Maps {
    pub size: u32,
    pub color: Vec<u8>,
    /// Tangent-space normals, `(n * 0.5 + 0.5) * 255` (flat is `128, 128, 255`).
    pub normal: Vec<u8>,
    /// Roughness in every colour channel.
    pub roughness: Vec<u8>,
}

/// A value in `0..1` for lattice point `(x, y)`.
fn hash(x: i64, y: i64, seed: u32) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ u64::from(seed).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise over a lattice of `pu`×`pv` cells across one tile (`u`, `v` in `0..1`), wrapping so
/// the tile repeats seamlessly.
fn noise(u: f64, v: f64, pu: i64, pv: i64, seed: u32) -> f64 {
    let (pu, pv) = (pu.max(1), pv.max(1));
    let (x, y) = (u.rem_euclid(1.0) * pu as f64, v.rem_euclid(1.0) * pv as f64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (smooth(x - x0 as f64), smooth(y - y0 as f64));
    let at = |i: i64, j: i64| hash(i.rem_euclid(pu), j.rem_euclid(pv), seed);
    let a = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * fx;
    let b = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * fx;
    a + (b - a) * fy
}

/// Fractal noise: `octaves` layers from a `pu`×`pv` lattice up, each twice as fine and half as
/// strong; `0..1`.
fn fbm(u: f64, v: f64, pu: i64, pv: i64, octaves: u32, seed: u32) -> f64 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in 0..octaves.clamp(1, 10) {
        let k = 1i64 << o;
        sum += amp * noise(u, v, pu.saturating_mul(k), pv.saturating_mul(k), seed.wrapping_add(o * 7919));
        norm += amp;
        amp *= 0.5;
    }
    sum / norm
}

fn mix(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale(c: [f64; 3], k: f64) -> [f64; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn rgb(c: Rgb) -> [f64; 3] {
    [f64::from(c.0) / 255.0, f64::from(c.1) / 255.0, f64::from(c.2) / 255.0]
}

/// One texel: base colour, height (`0..1`) and roughness.
struct Texel {
    color: [f64; 3],
    height: f64,
    roughness: f64,
}

/// A running bond or grid of units (bricks, tiles, planks; `joint` metres between them) across a
/// tile of `tile` metres: the
/// unit index and the distance (metres) to the nearest joint. `stagger` offsets alternate rows by
/// half a unit (bricks); `random_stagger` offsets each row by a random amount (planks).
fn units(x: f64, y: f64, tile: [f64; 2], unit: [f64; 2], joint: f64, stagger: bool, random_stagger: bool) -> ((i64, i64), f64) {
    // The pitch (unit and joint), adjusted so a whole number of units fills the tile: seamless.
    let fit = |t: f64, u: f64| {
        let n = (t / (u.max(1e-4) + joint.max(0.0))).round().clamp(1.0, 4096.0);
        t / n
    };
    let (w, h) = (fit(tile[0], unit[0]), fit(tile[1], unit[1]));
    let (px, py) = (x * tile[0], y * tile[1]);
    let row = (py / h).floor() as i64;
    let shift = if random_stagger {
        hash(row, 0, 99) * w
    } else if stagger && row.rem_euclid(2) == 1 {
        w * 0.5
    } else {
        0.0
    };
    let col = ((px + shift) / w).floor() as i64;
    let lx = (px + shift).rem_euclid(w);
    let ly = py.rem_euclid(h);
    let d = lx.min(w - lx).min(ly).min(h - ly);
    // Wrap the column index to the number of units across the tile, so units match at the seam.
    let across = (tile[0] / w).round().max(1.0) as i64;
    ((col.rem_euclid(across), row), d)
}

fn wood(u: f64, v: f64, rings: u32, contrast: f64, seed: u32) -> (f64, f64) {
    // Rings across v, stretched along u, warped by noise.
    let warp = fbm(u, v, 2, 4, 4, seed) * 2.0;
    let r = f64::from(rings.clamp(1, 64));
    let g = ((v * r + warp) * std::f64::consts::TAU).sin() * 0.5 + 0.5;
    let fine = fbm(u, v, 4, 64, 3, seed + 3);
    let t = g.powf(3.0) * 0.8 + fine * 0.2;
    (1.0 - contrast * t, t)
}

fn texel(r: &Recipe, base: [f64; 3], u: f64, v: f64) -> Texel {
    let p: &Pattern = &r.pattern;
    let contrast = p.contrast.clamp(0.0, 1.0);
    let rough = r.roughness.clamp(0.0, 1.0);
    let joint_color = p.joint_color.as_deref().and_then(parse_hex).map(rgb).unwrap_or(scale(base, 0.8));
    let tile = [r.tile[0].max(1e-3), r.tile[1].max(1e-3)];
    match p.kind.as_str() {
        "speckle" => {
            let n = fbm(u, v, 6, 6, p.detail.clamp(1, 10), 11);
            // Relief as subtle as the colour variation: paint is smooth, cast concrete less so.
            Texel {
                color: scale(base, 1.0 + contrast * (n - 0.5) * 2.0),
                height: 0.5 + (n - 0.5) * (contrast * 3.0).min(1.0),
                roughness: rough + (n - 0.5) * 0.08,
            }
        }
        "brick" | "tile" | "planks" => {
            let brick = p.kind == "brick";
            let planks = p.kind == "planks";
            let ((cx, cy), d) = units(u, v, tile, p.unit, p.joint, brick, planks);
            let joint = p.joint.max(0.0) * 0.5;
            let vary = (hash(cx, cy, 5) - 0.5) * 2.0 * p.variation.clamp(0.0, 1.0);
            let grit = fbm(u, v, 16, 16, 4, 21);
            if d < joint {
                // Mortar or grout: recessed, rough.
                Texel { color: scale(joint_color, 0.95 + grit * 0.1), height: 0.15 + grit * 0.05, roughness: (rough + 0.1).min(1.0) }
            } else {
                let edge = ((d - joint) / joint.max(1e-4)).min(1.0);
                let mut c = scale(base, 1.0 + vary);
                let mut rr = rough;
                if planks {
                    let (k, t) = wood(u * 3.0 + hash(cx, cy, 8), v, p.rings.max(4), 0.25, (hash(cx, cy, 9) * 1000.0) as u32);
                    c = scale(c, k);
                    rr += t * 0.05;
                } else if brick {
                    c = scale(c, 0.9 + grit * 0.2);
                } else if p.speckle > 0.0 {
                    c = scale(c, 1.0 + (grit - 0.5) * p.speckle);
                }
                Texel { color: c, height: 0.6 + 0.35 * edge + grit * 0.05, roughness: rr }
            }
        }
        "wood" => {
            let (k, t) = wood(u, v, p.rings, contrast, 31);
            Texel { color: scale(base, k), height: 0.5 + t * 0.1, roughness: rough + t * 0.05 }
        }
        "carpet" => {
            let n = fbm(u, v, 64, 64, 2, 41);
            let mut c = scale(base, 1.0 + contrast * (n - 0.5) * 2.0);
            if p.grid > 0.0 {
                let cells = (tile[0] / p.grid).round().max(1.0) as i64;
                let (i, j) = ((u * cells as f64).floor() as i64, (v * cells as f64).floor() as i64);
                c = scale(c, if (i + j).rem_euclid(2) == 0 { 1.04 } else { 0.96 });
            }
            Texel { color: c, height: n, roughness: rough }
        }
        "ceiling" => {
            let ((_, _), d) = units(u, v, tile, p.unit, p.joint, false, false);
            let joint = p.joint.max(0.0) * 0.5;
            if d < joint {
                return Texel { color: joint_color, height: 0.0, roughness: rough };
            }
            // Fissured mineral board: small dark pits.
            let n = fbm(u, v, 24, 24, 3, 51);
            let pit = (1.0 - ((n - 0.5).abs() / 0.04).min(1.0)).powf(2.0);
            Texel { color: scale(base, 1.0 - pit * 0.25), height: 0.8 - pit * 0.4, roughness: rough }
        }
        "marble" => {
            let w = fbm(u, v, 3, 3, 5, 61) * 3.0;
            let s = ((u + v + w) * std::f64::consts::PI * 2.0).sin().abs();
            let vein = (1.0 - s).powf(10.0);
            let cloud = fbm(u, v, 4, 4, 4, 62);
            let c = mix(scale(base, 0.97 + cloud * 0.06), scale(base, 0.55), vein * contrast * 2.0);
            Texel { color: c, height: 0.5, roughness: rough }
        }
        "brushed" => {
            let n = fbm(u, v, 2, 256, 3, 71);
            Texel { color: scale(base, 1.0 + contrast * (n - 0.5) * 2.0), height: n * 0.3, roughness: rough + (n - 0.5) * 0.1 }
        }
        _ => Texel { color: base, height: 0.5, roughness: rough },
    }
}

fn byte(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The textures of recipe `r` at `size`×`size` pixels (clamped to 4 … [`MAX_SIZE`]).
pub fn generate(r: &Recipe, size: u32) -> Maps {
    let size = size.clamp(4, MAX_SIZE);
    let n = size as usize;
    let base = rgb(parse_hex(&r.color).unwrap_or(Rgb(180, 180, 180)));
    let mut height = vec![0.0f64; n * n];
    let mut color = Vec::with_capacity(n * n * 4);
    let mut roughness = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = ((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64);
            let t = texel(r, base, u, v);
            if let Some(h) = height.get_mut(y * n + x) {
                *h = t.height;
            }
            color.extend([byte(t.color[0]), byte(t.color[1]), byte(t.color[2]), 255]);
            let g = byte(t.roughness);
            roughness.extend([g, g, g, 255]);
        }
    }
    // Normals from the height's slope (wrapping, so the tile stays seamless); the bump depth is
    // a fraction of a texel's width.
    let depth = 4.0;
    let h = |x: usize, y: usize| height.get((y % n) * n + (x % n)).copied().unwrap_or(0.5);
    let mut normal = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let dx = (h(x + 1, y) - h(x + n - 1, y)) * depth;
            let dy = (h(x, y + 1) - h(x, y + n - 1)) * depth;
            let len = (dx * dx + dy * dy + 1.0).sqrt();
            let (nx, ny, nz) = (-dx / len, dy / len, 1.0 / len);
            normal.extend([byte(nx * 0.5 + 0.5), byte(ny * 0.5 + 0.5), byte(nz * 0.5 + 0.5), 255]);
        }
    }
    Maps { size, color, normal, roughness }
}

/// A preview of the textures for a swatch: the colour lit from the upper left through the normal
/// map, so relief shows.
pub fn swatch(m: &Maps) -> Vec<u8> {
    let light = {
        let (x, y, z) = (-0.45f64, 0.45f64, 0.77f64);
        let l = (x * x + y * y + z * z).sqrt();
        (x / l, y / l, z / l)
    };
    m.color
        .as_chunks::<4>()
        .0
        .iter()
        .zip(m.normal.as_chunks::<4>().0)
        .flat_map(|(c, nrm)| {
            let n = |i: usize| f64::from(nrm.get(i).copied().unwrap_or(128)) / 127.5 - 1.0;
            let lit = (n(0) * light.0 + n(1) * light.1 + n(2) * light.2).max(0.0);
            let k = 0.55 + 0.5 * lit;
            let ch = |i: usize| byte(f64::from(c.get(i).copied().unwrap_or(0)) / 255.0 * k);
            [ch(0), ch(1), ch(2), 255]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::material::library;

    #[test]
    fn every_library_material_generates_seamless_textures() {
        for r in library() {
            let m = generate(r, 64);
            assert_eq!((m.color.len(), m.normal.len(), m.roughness.len()), (64 * 64 * 4, 64 * 64 * 4, 64 * 64 * 4), "{}", r.id);
            // Seamless: the first and last columns and rows differ no more than neighbours do.
            let px = |buf: &[u8], x: usize, y: usize| -> i32 { buf.get((y * 64 + x) * 4).copied().map_or(0, i32::from) };
            let seam: i32 = (0..64).map(|y| (px(&m.color, 0, y) - px(&m.color, 63, y)).abs()).sum();
            let inner: i32 = (0..64).map(|y| (px(&m.color, 31, y) - px(&m.color, 32, y)).abs()).sum();
            assert!(seam <= inner * 3 + 64 * 12, "{}: seam {seam} vs inner {inner}", r.id);
            // Not flat black or blown out; patterned materials vary.
            let mean = m.color.as_chunks::<4>().0.iter().map(|c| i64::from(c[0])).sum::<i64>() / (64 * 64);
            assert!((10..=250).contains(&mean), "{}: mean {mean}", r.id);
            if r.pattern.kind != "plain" {
                let first = m.color.get(..3).map(<[u8]>::to_vec);
                assert!(m.color.as_chunks::<4>().0.iter().any(|c| Some(c[..3].to_vec()) != first), "{} has no pattern", r.id);
            }
        }
    }

    #[test]
    fn generation_is_deterministic_and_capped() {
        let r = cadaddy_doc::material::recipe("brick-red").unwrap();
        assert_eq!(generate(r, 32), generate(r, 32));
        assert_eq!(generate(r, 0).size, 4);
        assert_eq!(generate(r, u32::MAX).size, MAX_SIZE);
        assert_eq!(swatch(&generate(r, 16)).len(), 16 * 16 * 4);
        // A recipe of unknown kind or nonsense values still makes a plain texture.
        let odd = Recipe {
            color: "nope".into(),
            roughness: f64::NAN,
            tile: [0.0, -1.0],
            pattern: Pattern { kind: "zz".into(), ..Pattern::default() },
            ..Recipe::default()
        };
        let m = generate(&odd, 8);
        assert_eq!(m.color.len(), 8 * 8 * 4);
    }
}
