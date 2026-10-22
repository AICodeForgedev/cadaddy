//! Radiance HDR (.hdr / .pic) reader: the public RGBE format described in Greg Ward's "Real
//! Pixels" (Graphics Gems II, 1991) and the file format notes at
//! <https://www.graphics.cornell.edu/online/formats/rgbe/> (the "Radiance" page at Cornell).
//!
//! The format stores high-dynamic-range pixels as four bytes each: R, G, B mantissas and a
//! shared exponent E, where the linear value of each channel is `mantissa * 2^(E − 128 − 8)`.
//! Rows are run-length encoded (the "new" adaptive RLE described in the format notes).
//!
//! We read only the `32-bit_rle_rgbe` variant (the one every HDR tool writes today); XY and
//! XYZE colour spaces are accepted but treated as RGB (close enough for environment lighting).

#![allow(clippy::manual_range_contains)]

/// A decoded HDR image: linear RGB floats, rows from the top, `width × height × 3` values.
#[derive(Clone, Debug, PartialEq)]
pub struct HdrImage {
    pub width: u32,
    pub height: u32,
    /// Linear RGB, row-major, 3 floats per pixel.
    pub data: Vec<f32>,
}

/// Maximum pixels decoded (16k × 16k).
const MAX_PIXELS: u64 = 16_384 * 16_384;
/// Maximum encoded file size accepted (256 MB).
const MAX_FILE: usize = 256 << 20;

impl HdrImage {
    /// The map as the sky the shading's ambient term uses (CPU and GPU alike): the mean radiance
    /// near the zenith, about the horizon and below it, each pixel weighted by its solid angle.
    pub fn sky(&self) -> crate::light::Sky {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut sums = [[0.0f64; 3]; 3];
        let mut weights = [0.0f64; 3];
        for y in 0..h {
            let lat = (0.5 - (y as f64 + 0.5) / h as f64) * std::f64::consts::PI;
            let (up, solid) = (lat.sin(), lat.cos());
            // Zenith: by how high it is; horizon: by how near it, above; ground: by how low.
            let shares = [up.max(0.0).powi(2), if up >= 0.0 { 1.0 - up } else { 0.0 }, (-up).max(0.0)];
            for x in 0..w {
                let i = (y * w + x) * 3;
                for (band, share) in shares.iter().enumerate() {
                    let k = share * solid;
                    for (c, sum) in sums[band].iter_mut().enumerate() {
                        *sum += f64::from(self.data.get(i + c).copied().unwrap_or(0.0)) * k;
                    }
                    weights[band] += k;
                }
            }
        }
        let mean = |b: usize| sums[b].map(|s| if weights[b] > 0.0 { (s / weights[b]) as f32 } else { 0.0 });
        crate::light::Sky { zenith: mean(0), horizon: mean(1), ground: mean(2) }
    }

    /// Sample the environment map in direction `d` (unit vector) using a lat-long (equirect)
    /// mapping: longitude from +X through +Y, latitude from the equator up to +Z.
    pub fn sample(&self, d: cadaddy_geom::Vec3) -> [f32; 3] {
        if self.width == 0 || self.height == 0 {
            return [0.0; 3];
        }
        let lon = f64::atan2(d.y, d.x); // −π … +π
        let lat = d.z.clamp(-1.0, 1.0).asin(); // −π/2 … +π/2
        let u = (0.5 + lon / std::f64::consts::TAU) as f32; // 0 … 1
        let v = (0.5 - lat / std::f64::consts::PI) as f32; // 0 (top/+Z) … 1 (bottom/−Z)
        let fx = u * self.width as f32;
        let fy = v * self.height as f32;
        let x0 = (fx.floor() as i64).rem_euclid(self.width as i64) as u32;
        let y0 = (fy.floor() as u32).min(self.height - 1);
        let x1 = (x0 + 1) % self.width;
        let y1 = (y0 + 1).min(self.height - 1);
        let (sx, sy) = (fx.fract(), fy.fract());
        let px = |x: u32, y: u32| -> [f32; 3] {
            let i = (y as usize * self.width as usize + x as usize) * 3;
            [self.data.get(i).copied().unwrap_or(0.0), self.data.get(i + 1).copied().unwrap_or(0.0), self.data.get(i + 2).copied().unwrap_or(0.0)]
        };
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let top = mix(px(x0, y0), px(x1, y0), sx);
        let bot = mix(px(x0, y1), px(x1, y1), sx);
        mix(top, bot, sy)
    }
}

/// Decode a Radiance .hdr file from its bytes. Returns `None` on any format or size error.
pub fn decode(bytes: &[u8]) -> Option<HdrImage> {
    if bytes.len() > MAX_FILE {
        return None;
    }
    let mut pos = 0usize;
    // Skip the header lines until the empty line before the resolution string.
    // The first line must be "#?" (the Radiance magic).
    if !bytes.starts_with(b"#?") {
        return None;
    }
    loop {
        let end = memchr(b'\n', &bytes[pos..])?;
        let line = &bytes[pos..pos + end];
        pos += end + 1;
        if line.is_empty() || line == b"\r" {
            break;
        }
        if pos > 4096 {
            return None; // header too long
        }
    }
    // Resolution line: "-Y height +X width\n" (top to bottom, left to right).
    let end = memchr(b'\n', &bytes[pos..])?;
    let res_line = std::str::from_utf8(bytes.get(pos..pos + end)?).ok()?;
    pos += end + 1;
    let (height, width) = parse_resolution(res_line)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let npx = width as usize * height as usize;
    let mut data = Vec::with_capacity(npx * 3);
    for _ in 0..height {
        let row = decode_row(bytes, &mut pos, width as usize)?;
        for px in row.as_chunks::<4>().0 {
            let (r, g, b, e) = (px[0], px[1], px[2], px[3]);
            if e == 0 {
                data.extend([0.0f32, 0.0, 0.0]);
            } else {
                data.extend([r, g, b].map(|m| rgbe(m, e)));
            }
        }
    }
    Some(HdrImage { width, height, data })
}

/// A channel's linear value: `mantissa × 2^(e − 128 − 8)`, in f64 so that the smallest exponents
/// give tiny positive values (2^−135 isn't a normal f32), never garbage.
fn rgbe(mantissa: u8, e: u8) -> f32 {
    (f64::from(mantissa) * 2f64.powi(i32::from(e) - 136)) as f32
}

/// Parse "-Y H +X W" or "+X W -Y H" (the only orientations we handle).
fn parse_resolution(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[0] == "-Y" && parts[2] == "+X" {
        let h: u32 = parts[1].parse().ok()?;
        let w: u32 = parts[3].parse().ok()?;
        Some((h, w))
    } else if parts[0] == "+X" && parts[2] == "-Y" {
        let w: u32 = parts[1].parse().ok()?;
        let h: u32 = parts[3].parse().ok()?;
        Some((h, w))
    } else {
        None
    }
}

/// Find the first occurrence of `byte` in `buf`, returning its index.
fn memchr(byte: u8, buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == byte)
}

/// Decode one scanline of `width` pixels (adaptive RLE) starting at `pos` in `bytes`.
/// Returns 4 bytes per pixel (RGBE).
fn decode_row(bytes: &[u8], pos: &mut usize, width: usize) -> Option<Vec<u8>> {
    if width < 8 || width > 0x7fff {
        // Old format or too wide: raw RGBE pixels.
        return decode_row_flat(bytes, pos, width);
    }
    // Check for the new adaptive RLE marker: bytes 2, 2, (width >> 8), (width & 0xff).
    let hdr = bytes.get(*pos..*pos + 4)?;
    if hdr[0] != 2 || hdr[1] != 2 || (hdr[2] as usize) << 8 | hdr[3] as usize != width {
        return decode_row_flat(bytes, pos, width);
    }
    *pos += 4;
    // Each of the 4 channels is RLE-encoded separately.
    let mut scanline = vec![0u8; width * 4];
    for ch in 0..4 {
        let mut i = 0usize;
        while i < width {
            let code = *bytes.get(*pos)?;
            *pos += 1;
            if code > 128 {
                // Run of identical values.
                let count = (code as usize) - 128;
                if i + count > width {
                    return None;
                }
                let val = *bytes.get(*pos)?;
                *pos += 1;
                for j in 0..count {
                    if let Some(p) = scanline.get_mut((i + j) * 4 + ch) {
                        *p = val;
                    }
                }
                i += count;
            } else {
                // Literal values.
                let count = code as usize;
                if count == 0 || i + count > width {
                    return None;
                }
                for j in 0..count {
                    let val = *bytes.get(*pos)?;
                    *pos += 1;
                    if let Some(p) = scanline.get_mut((i + j) * 4 + ch) {
                        *p = val;
                    }
                }
                i += count;
            }
        }
    }
    Some(scanline)
}

/// Decode a scanline of raw (uncompressed) RGBE pixels.
fn decode_row_flat(bytes: &[u8], pos: &mut usize, width: usize) -> Option<Vec<u8>> {
    let n = width.checked_mul(4)?;
    let row = bytes.get(*pos..*pos + n)?.to_vec();
    *pos += n;
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a minimal valid .hdr file from linear RGB pixels.
    /// Matches the decode: `value = mantissa * 2^(E - 128 - 8)`.
    fn encode_hdr(width: u32, height: u32, pixels: &[[f32; 3]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n");
        out.extend(format!("-Y {height} +X {width}\n").as_bytes());
        for px in pixels {
            let max = px[0].max(px[1]).max(px[2]);
            if max < 1e-32 {
                out.extend([0, 0, 0, 0]);
            } else {
                // Pick E so that the max channel's mantissa is in [128, 255].
                // mantissa = value / 2^(E - 136), so E = floor(log2(max)) + 129.
                let e_raw = max.log2().floor() as i32 + 129;
                let e = e_raw.clamp(1, 255) as u8;
                let inv_scale = (1.0 / 2f64.powi(i32::from(e) - 136)) as f32;
                let r = (px[0] * inv_scale).round().clamp(0.0, 255.0) as u8;
                let g = (px[1] * inv_scale).round().clamp(0.0, 255.0) as u8;
                let b = (px[2] * inv_scale).round().clamp(0.0, 255.0) as u8;
                out.extend([r, g, b, e]);
            }
        }
        out
    }

    #[test]
    fn round_trip_small_image() {
        let pixels = vec![[0.0, 0.0, 0.0], [1.0, 0.5, 0.25], [10.0, 5.0, 2.5], [0.001, 0.002, 0.003]];
        let bytes = encode_hdr(2, 2, &pixels);
        let img = decode(&bytes).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        // RGBE has limited precision (~1% for bright values).
        for (i, expected) in pixels.iter().enumerate() {
            let got = [img.data[i * 3], img.data[i * 3 + 1], img.data[i * 3 + 2]];
            for c in 0..3 {
                let tol = expected[c].abs() * 0.02 + 1e-5;
                assert!((got[c] - expected[c]).abs() < tol, "pixel {i} ch {c}: {:.6} vs {:.6}", got[c], expected[c]);
            }
        }
    }

    #[test]
    fn black_image_is_all_zeros() {
        let bytes = encode_hdr(4, 2, &[[0.0; 3]; 8]);
        let img = decode(&bytes).unwrap();
        assert!(img.data.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn hostile_inputs_are_rejected() {
        assert!(decode(b"not an hdr").is_none());
        assert!(decode(b"#?RADIANCE\n\n-Y 0 +X 0\n").is_none());
        assert!(decode(b"").is_none());
        // Absurd dimensions.
        assert!(decode(b"#?RADIANCE\n\n-Y 999999 +X 999999\n").is_none());
    }

    #[test]
    fn environment_map_sampling() {
        // A uniform white environment: every direction should sample ~1.0.
        let n = 16;
        let pixels: Vec<[f32; 3]> = vec![[1.0, 1.0, 1.0]; (n * n) as usize];
        let bytes = encode_hdr(n, n, &pixels);
        let img = decode(&bytes).unwrap();
        use cadaddy_geom::Vec3;
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let directions = [x, -x, y, -y, Vec3::Z, -Vec3::Z, Vec3::new(1.0, 1.0, 1.0).normalized()];
        for d in directions {
            let s = img.sample(d);
            for (c, v) in s.iter().enumerate() {
                assert!((v - 1.0).abs() < 0.05, "dir {d:?} ch {c}: {v}");
            }
        }
    }

    /// Rows written with the adaptive run-length encoding (a 2, 2, width marker, then each
    /// channel as runs and literals), as every HDR tool writes them, decode to the exact values.
    #[test]
    fn adaptive_rle_rows_decode_exactly() {
        let mut f = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 8\n".to_vec();
        // Exponent 136: a channel's value is its mantissa.
        let row = |r: &[u8], g: &[u8], b: &[u8], e: &[u8]| {
            let mut v = vec![2, 2, 0, 8];
            for ch in [r, g, b, e] {
                v.extend_from_slice(ch);
            }
            v
        };
        // Red: a run of 3 tens then 5 literals; green: one run of 8; blue: 8 literals; E: a run.
        f.extend(row(&[128 + 3, 10, 5, 1, 2, 3, 4, 5], &[128 + 8, 20], &[8, 9, 8, 7, 6, 5, 4, 3, 2], &[128 + 8, 136]));
        // Second row: literals then a run (the order reversed), exponent 137 (twice the mantissa).
        f.extend(row(&[2, 100, 101, 128 + 6, 7], &[4, 1, 1, 2, 2, 128 + 4, 3], &[128 + 8, 0], &[128 + 8, 137]));
        let img = decode(&f).unwrap();
        assert_eq!((img.width, img.height), (8, 2));
        let px = |x: usize, y: usize| [0, 1, 2].map(|c| img.data[(y * 8 + x) * 3 + c]);
        let reds = [10.0, 10.0, 10.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let blues = [9.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0];
        for x in 0..8 {
            assert_eq!(px(x, 0), [reds[x], 20.0, blues[x]], "row 0, x {x}");
        }
        let reds = [100.0, 101.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0];
        let greens = [1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 3.0, 3.0];
        for x in 0..8 {
            assert_eq!(px(x, 1), [reds[x] * 2.0, greens[x] * 2.0, 0.0], "row 1, x {x}");
        }
        // A run past the row's end is refused, not read on.
        let mut bad = b"#?RADIANCE\n\n-Y 1 +X 8\n".to_vec();
        bad.extend([2, 2, 0, 8, 128 + 9, 1]);
        assert!(decode(&bad).is_none());
    }

    /// A map that isn't uniform is read the right way up and round: its top row is straight up
    /// (+Z), its bottom row straight down, and the middle column (u = 0.5) is +X.
    #[test]
    fn a_maps_orientation() {
        let (w, h) = (8u32, 4u32);
        let mut pixels = vec![[0.1f32, 0.1, 0.1]; (w * h) as usize];
        for x in 0..w as usize {
            pixels[x] = [4.0, 0.0, 0.0];
            pixels[(h as usize - 1) * w as usize + x] = [0.0, 0.0, 4.0];
        }
        for y in 1..h as usize - 1 {
            pixels[y * w as usize + w as usize / 2] = [0.0, 4.0, 0.0];
        }
        let img = decode(&encode_hdr(w, h, &pixels)).unwrap();
        use cadaddy_geom::Vec3;
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|c| (a[c] - b[c]).abs() < 0.05);
        assert!(near(img.sample(Vec3::Z), [4.0, 0.0, 0.0]), "+Z: {:?}", img.sample(Vec3::Z));
        assert!(near(img.sample(-Vec3::Z), [0.0, 0.0, 4.0]), "-Z: {:?}", img.sample(-Vec3::Z));
        assert!(near(img.sample(Vec3::new(1.0, 0.0, 0.0)), [0.0, 4.0, 0.0]), "+X: {:?}", img.sample(Vec3::new(1.0, 0.0, 0.0)));
        assert!(near(img.sample(Vec3::new(-1.0, 0.0, 0.0)), [0.1, 0.1, 0.1]), "-X: {:?}", img.sample(Vec3::new(-1.0, 0.0, 0.0)));
        // Its sky terms: red overhead, blue below.
        let sky = img.sky();
        assert!(sky.zenith[0] > sky.zenith[2] && sky.ground[2] > sky.ground[0], "{sky:?}");
    }

    /// The smallest exponents give tiny positive values (they were huge negative ones).
    #[test]
    fn low_exponents_are_tiny_and_positive() {
        let mut f = b"#?RADIANCE\n\n-Y 1 +X 3\n".to_vec();
        f.extend([255, 128, 1, 1, 200, 200, 200, 8, 10, 20, 30, 9]);
        let img = decode(&f).unwrap();
        for (i, v) in img.data.iter().enumerate() {
            assert!(v.is_finite() && *v > 0.0 && *v < 1e-30, "value {i}: {v}");
        }
        // e = 1: 255 × 2^−135.
        assert!((f64::from(img.data[0]) - 255.0 * 2f64.powi(-135)).abs() < 1e-44, "{}", img.data[0]);
    }
}
