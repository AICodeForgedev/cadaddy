//! Scanned prints: a fixture sheet printed on paper and scanned back, as a grey image with the
//! faults real scans have — scanner noise and specks, a slightly turned and shifted sheet, JPEG
//! artefacts. Deterministic by seed. [`crate::scan_reader`] reads these back.
//!
//! The print is monochrome the way plotters print consultant sets: black ink, with the
//! background (architecture, grid; colour 8) screened grey, and every pen at its lineweight.

use crate::layout;
use crate::rng::Rng;
use crate::truth::Sheet;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::{Bounds2, Vec2};

/// Most pixels a scan may have (a 36" × 24" sheet at 300 dpi is 78 MP; we stop well before).
pub const MAX_SCAN_PIXELS: usize = cadaddy_geom::vectorize::MAX_PIXELS;

/// A grey scan: `w` × `h` pixels, row by row from the top, 0 black … 255 white.
#[derive(Clone, Debug)]
pub struct Scan {
    pub w: usize,
    pub h: usize,
    pub gray: Vec<u8>,
    /// Pixels per paper inch.
    pub dpi: f64,
}

impl Scan {
    fn at(&self, x: i64, y: i64) -> f64 {
        if x < 0 || y < 0 {
            return 255.0;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.w || y >= self.h {
            return 255.0;
        }
        self.gray.get(y * self.w + x).map_or(255.0, |&g| f64::from(g))
    }

    /// The scan as a PNG (to look at it).
    pub fn png(&self) -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        let (w, h) = (u32::try_from(self.w).map_err(|e| e.to_string())?, u32::try_from(self.h).map_err(|e| e.to_string())?);
        image::write_buffer_with_format(&mut std::io::Cursor::new(&mut buf), &self.gray, w, h, image::ExtendedColorType::L8, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }
}

/// What happened to the print between the plotter and the scanner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dirt {
    /// Grey jitter on every pixel and black/white specks.
    pub noise: bool,
    /// The sheet lay turned on the glass, degrees counter-clockwise as seen.
    pub turn_deg: f64,
    /// … and shifted, pixels (right, down).
    pub shift_px: [f64; 2],
    /// Saved as a JPEG at this quality.
    pub jpeg_quality: Option<u8>,
}

impl Dirt {
    pub fn clean() -> Dirt {
        Dirt::default()
    }

    /// Everything at once: noise, 1.5° turned, shifted, JPEG at quality 40.
    pub fn all() -> Dirt {
        Dirt { noise: true, turn_deg: 1.5, shift_px: [23.0, -17.0], jpeg_quality: Some(40) }
    }
}

/// The sheet's drawing printed at `dpi`: the whole sheet (its paper), black ink on white, the
/// background screened grey, lineweights at their widths.
pub fn print(d: &Drawing, sh: &Sheet, dpi: f64) -> Result<Scan, String> {
    if !dpi.is_finite() || dpi <= 0.0 {
        return Err("the resolution must be positive".into());
    }
    let [pw, ph] = sh.paper_in;
    let (w, h) = ((pw * dpi).round(), (ph * dpi).round());
    if !(w >= 1.0 && h >= 1.0 && w * h <= MAX_SCAN_PIXELS as f64 && w <= 16384.0 && h <= 16384.0) {
        return Err(format!("a {pw}\" × {ph}\" sheet at {dpi} dpi is too large to scan"));
    }
    let o = layout::paper_to_model(sh, [0.0, 0.0]);
    let e = layout::paper_to_model(sh, [pw, ph]);
    let frame = Bounds2::new(Vec2::new(o[0], o[1]), Vec2::new(e[0], e[1]));
    let view = cadaddy_render::raster::View::fit(&frame, w as u32, h as u32, 0.0);
    let ro = cadaddy_render::Options { tolerance: 0.25 / view.scale.max(1e-9), lineweights: true, ..Default::default() };
    let list = cadaddy_render::build(d, &Space::Model, &ro);
    let o = cadaddy_render::raster::RasterOptions {
        background: cadaddy_color::Rgb(255, 255, 255),
        hairline: (0.18 / 25.4 * dpi).max(1.0) as f32,
        px_per_mm: (dpi / 25.4) as f32,
        antialias: true,
    };
    let pm = cadaddy_render::raster::render(&list, &view, &o).ok_or("the sheet could not be printed")?;
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    // Monochrome: any ink colour prints black (its darkest channel), colour 8 (the drawing's
    // grey) prints screened at its grey.
    let gray = pm.data().chunks_exact(4).map(|p| p.iter().take(3).copied().min().unwrap_or(255)).collect();
    Ok(Scan { w, h, gray, dpi })
}

/// The scan of a print with `dirt`.
pub fn dirty(s: &Scan, dirt: &Dirt, seed: u64) -> Result<Scan, String> {
    let mut out = if dirt.turn_deg != 0.0 || dirt.shift_px != [0.0, 0.0] { turn(s, dirt.turn_deg, dirt.shift_px) } else { s.clone() };
    if dirt.noise {
        noise(&mut out, seed);
    }
    if let Some(q) = dirt.jpeg_quality {
        out = jpeg(&out, q)?;
    }
    Ok(out)
}

/// Turned by `deg` (counter-clockwise as seen) about the image centre and shifted, bilinear,
/// paper outside.
pub(crate) fn turn(s: &Scan, deg: f64, shift: [f64; 2]) -> Scan {
    let (sn, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (s.w as f64 / 2.0, s.h as f64 / 2.0);
    let mut gray = vec![255u8; s.w * s.h];
    for (i, g) in gray.iter_mut().enumerate() {
        let (x, y) = ((i % s.w) as f64, (i / s.w) as f64);
        // y down: a counter-clockwise turn as seen is clockwise in these axes.
        let (dx, dy) = (x + 0.5 - cx - shift[0], y + 0.5 - cy - shift[1]);
        let (sx, sy) = (c * dx - sn * dy + cx - 0.5, sn * dx + c * dy + cy - 0.5);
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let v = s.at(x0, y0) * (1.0 - fx) * (1.0 - fy)
            + s.at(x0 + 1, y0) * fx * (1.0 - fy)
            + s.at(x0, y0 + 1) * (1.0 - fx) * fy
            + s.at(x0 + 1, y0 + 1) * fx * fy;
        *g = v.round().clamp(0.0, 255.0) as u8;
    }
    Scan { w: s.w, h: s.h, gray, dpi: s.dpi }
}

/// Scanner noise: every pixel jittered (±25 grey), and specks (0.3% black, 0.3% white).
fn noise(s: &mut Scan, seed: u64) {
    let mut r = Rng::new(seed ^ 0x5343_414E);
    for g in &mut s.gray {
        let u = unit(&mut r);
        let v = if u < 0.003 {
            0.0
        } else if u < 0.006 {
            255.0
        } else {
            f64::from(*g) + (unit(&mut r) - 0.5) * 50.0
        };
        *g = v.round().clamp(0.0, 255.0) as u8;
    }
}

fn unit(r: &mut Rng) -> f64 {
    (r.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// Saved as a JPEG at `quality` and read back.
fn jpeg(s: &Scan, quality: u8) -> Result<Scan, String> {
    let (w, h) = (u32::try_from(s.w).map_err(|e| e.to_string())?, u32::try_from(s.h).map_err(|e| e.to_string())?);
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100))
        .encode(&s.gray, w, h, image::ExtendedColorType::L8)
        .map_err(|e| e.to_string())?;
    let back = image::load_from_memory(&buf).map_err(|e| e.to_string())?.to_luma8();
    if back.width() != w || back.height() != h {
        return Err("the JPEG came back a different size".into());
    }
    Ok(Scan { w: s.w, h: s.h, gray: back.into_raw(), dpi: s.dpi })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: usize, h: usize) -> Scan {
        Scan { w, h, gray: vec![255; w * h], dpi: 100.0 }
    }

    #[test]
    fn a_turn_by_nothing_keeps_the_image() {
        let mut s = blank(20, 10);
        if let Some(g) = s.gray.get_mut(5 * 20 + 7) {
            *g = 0;
        }
        let t = turn(&s, 0.0, [0.0, 0.0]);
        assert_eq!(t.gray, s.gray);
    }

    #[test]
    fn a_shift_moves_ink_right_and_down() {
        let mut s = blank(20, 10);
        if let Some(g) = s.gray.get_mut(5 * 20 + 7) {
            *g = 0;
        }
        let t = turn(&s, 0.0, [3.0, 2.0]);
        assert_eq!(t.gray.get(7 * 20 + 10), Some(&0));
    }

    #[test]
    fn noise_and_jpeg_are_deterministic() {
        let s = blank(64, 48);
        let a = dirty(&s, &Dirt { noise: true, jpeg_quality: Some(40), ..Dirt::default() }, 7);
        let b = dirty(&s, &Dirt { noise: true, jpeg_quality: Some(40), ..Dirt::default() }, 7);
        assert_eq!(a.map(|s| s.gray), b.map(|s| s.gray));
    }

    #[test]
    fn absurd_resolutions_are_refused() {
        let sh = Sheet { paper_in: [36.0, 24.0], ..Sheet::default() };
        let d = Drawing::new_imperial();
        assert!(print(&d, &sh, f64::NAN).is_err());
        assert!(print(&d, &sh, 0.0).is_err());
        assert!(print(&d, &sh, 10_000.0).is_err());
    }
}
