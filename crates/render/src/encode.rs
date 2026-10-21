//! Raster images written as files: PNG and JPEG (through `image`), and BMP and baseline TIFF
//! written here (both uncompressed, from their public specifications: the Windows BITMAPINFOHEADER
//! layout and TIFF 6.0 baseline), in colour, greys or black and white.

use std::io::Cursor;

/// An image file format for raster output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpg,
    Bmp,
    Tif,
}

impl Format {
    /// From a file extension or output name (`png`, `jpg`/`jpeg`, `bmp`, `tif`/`tiff`).
    pub fn parse(s: &str) -> Option<Format> {
        match s.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpg),
            "bmp" => Some(Format::Bmp),
            "tif" | "tiff" => Some(Format::Tif),
            _ => None,
        }
    }
}

/// Colour depth: 24-bit colour, 8-bit greys or 1-bit black and white.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Color,
    Grey,
    Mono,
}

impl Depth {
    /// From bits per pixel: 24 (or 32), 8, 1; anything else is colour.
    pub fn from_bits(bits: u64) -> Depth {
        match bits {
            8 => Depth::Grey,
            1 => Depth::Mono,
            _ => Depth::Color,
        }
    }
}

/// JPEG quality (0–100).
const JPEG_QUALITY: u8 = 90;

/// The grey level of an RGB pixel (ITU-R BT.601 weights).
fn luma(p: &[u8; 4]) -> u8 {
    ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
}

/// Straight RGBA pixels (`width × height`, rows top down) as `format` at `depth`, with `dpi` in
/// the file where the format holds it. `None` when the sizes don't match or encoding fails.
pub fn encode(rgba: &[u8], width: u32, height: u32, format: Format, depth: Depth, dpi: f64) -> Option<Vec<u8>> {
    let n = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || rgba.len() != n.checked_mul(4)? {
        return None;
    }
    let px = rgba.as_chunks::<4>().0;
    // Greys for 8 and 1 bit (1 bit: dark ink is black, the rest white).
    let grey = || -> Vec<u8> {
        px.iter()
            .map(|p| {
                let g = luma(p);
                if depth == Depth::Mono { if g < 128 { 0 } else { 255 } } else { g }
            })
            .collect()
    };
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi.min(100_000.0) } else { 96.0 };
    match format {
        Format::Png | Format::Jpg => {
            let mut out = Cursor::new(Vec::new());
            let (bytes, kind) = match depth {
                Depth::Color => (px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>(), image::ExtendedColorType::Rgb8),
                _ => (grey(), image::ExtendedColorType::L8),
            };
            let ok = if format == Format::Png {
                image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut out), &bytes, width, height, kind).is_ok()
            } else {
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode(&bytes, width, height, kind).is_ok()
            };
            ok.then(|| out.into_inner())
        }
        Format::Bmp => Some(bmp(px, width, height, depth, &grey(), dpi)),
        Format::Tif => Some(tif(px, width, height, depth, &grey(), dpi)),
    }
}

/// Bits of a 1-bit row packed most significant first (1 = white).
fn pack_bits(row: &[u8]) -> Vec<u8> {
    row.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, g)| if *g >= 128 { b | (0x80 >> i) } else { b })).collect()
}

/// A Windows bitmap: BITMAPFILEHEADER + BITMAPINFOHEADER, rows bottom up padded to 4 bytes;
/// 24-bit BGR, or 8 / 1 bit with a grey palette.
fn bmp(px: &[[u8; 4]], w: u32, h: u32, depth: Depth, grey: &[u8], dpi: f64) -> Vec<u8> {
    let (bits, palette): (u16, Vec<u8>) = match depth {
        Depth::Color => (24, Vec::new()),
        Depth::Grey => (8, (0..=255u8).flat_map(|g| [g, g, g, 0]).collect()),
        Depth::Mono => (1, vec![0, 0, 0, 0, 255, 255, 255, 0]),
    };
    let wu = w as usize;
    let row_bytes = (wu * usize::from(bits)).div_ceil(32) * 4;
    let image_size = row_bytes * h as usize;
    let offset = 14 + 40 + palette.len();
    let mut out = Vec::with_capacity(offset + image_size);
    let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(b"BM");
    u32le(&mut out, u32::try_from(offset + image_size).unwrap_or(u32::MAX));
    u32le(&mut out, 0);
    u32le(&mut out, offset as u32);
    // BITMAPINFOHEADER
    u32le(&mut out, 40);
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    u32le(&mut out, 0); // no compression
    u32le(&mut out, u32::try_from(image_size).unwrap_or(u32::MAX));
    let ppm = (dpi / 0.0254).round() as u32;
    u32le(&mut out, ppm);
    u32le(&mut out, ppm);
    u32le(&mut out, if palette.is_empty() { 0 } else { (palette.len() / 4) as u32 });
    u32le(&mut out, 0);
    out.extend_from_slice(&palette);
    for y in (0..h as usize).rev() {
        let start = out.len();
        let row = y * wu..(y + 1) * wu;
        match depth {
            Depth::Color => {
                for p in px.get(row).unwrap_or_default() {
                    out.extend_from_slice(&[p[2], p[1], p[0]]);
                }
            }
            Depth::Grey => out.extend_from_slice(grey.get(row).unwrap_or_default()),
            Depth::Mono => out.extend_from_slice(&pack_bits(grey.get(row).unwrap_or_default())),
        }
        out.resize(start + row_bytes, 0);
    }
    out
}

/// A baseline TIFF (little endian, one uncompressed strip): RGB, greys or bilevel (black is 0).
fn tif(px: &[[u8; 4]], w: u32, h: u32, depth: Depth, grey: &[u8], dpi: f64) -> Vec<u8> {
    let wu = w as usize;
    let data: Vec<u8> = match depth {
        Depth::Color => px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
        Depth::Grey => grey.to_vec(),
        Depth::Mono => grey.chunks(wu.max(1)).flat_map(pack_bits).collect(),
    };
    let (spp, bits): (u16, u16) = match depth {
        Depth::Color => (3, 8),
        Depth::Grey => (1, 8),
        Depth::Mono => (1, 1),
    };
    // Layout: header, image data, then the extra values (bits per sample, resolutions), then the IFD.
    let mut out: Vec<u8> = Vec::with_capacity(data.len() + 256);
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // IFD offset, set below
    let data_at = out.len() as u32;
    out.extend_from_slice(&data);
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let bps_at = out.len() as u32;
    for _ in 0..spp {
        out.extend_from_slice(&bits.to_le_bytes());
    }
    let res_at = out.len() as u32;
    let res = (dpi * 100.0).round() as u32;
    for _ in 0..2 {
        out.extend_from_slice(&res.to_le_bytes());
        out.extend_from_slice(&100u32.to_le_bytes());
    }
    let ifd_at = out.len() as u32;
    out.splice(4..8, ifd_at.to_le_bytes());
    // (tag, type, count, value): type 3 SHORT, 4 LONG, 5 RATIONAL; values in ascending tag order.
    let photometric: u32 = if depth == Depth::Color { 2 } else { 1 };
    let bps_value = if spp == 1 { u32::from(bits) } else { bps_at };
    let entries: [(u16, u16, u32, u32); 12] = [
        (256, 4, 1, w),
        (257, 4, 1, h),
        (258, 3, u32::from(spp), bps_value),
        (259, 3, 1, 1),
        (262, 3, 1, photometric),
        (273, 4, 1, data_at),
        (277, 3, 1, u32::from(spp)),
        (278, 4, 1, h),
        (279, 4, 1, data.len() as u32),
        (282, 5, 1, res_at),
        (283, 5, 1, res_at + 8),
        (296, 3, 1, 2),
    ];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        // A SHORT value sits in the first two bytes of the field.
        if kind == 3 && count == 1 {
            out.extend_from_slice(&(value as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
        } else {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 × 2: red, green, blue / black, white, grey.
    fn sample() -> Vec<u8> {
        [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [0, 0, 0, 255], [255, 255, 255, 255], [128, 128, 128, 255]].concat()
    }

    #[test]
    fn every_format_and_depth_reads_back() {
        let px = sample();
        for f in [Format::Png, Format::Jpg, Format::Bmp, Format::Tif] {
            for d in [Depth::Color, Depth::Grey, Depth::Mono] {
                let bytes = encode(&px, 3, 2, f, d, 150.0).unwrap_or_else(|| panic!("{f:?} {d:?}"));
                let img = image::load_from_memory(&bytes).unwrap_or_else(|e| panic!("{f:?} {d:?}: {e}"));
                assert_eq!((img.width(), img.height()), (3, 2), "{f:?} {d:?}");
                let rgb = img.to_rgb8();
                if f == Format::Jpg {
                    continue; // lossy
                }
                let at = |x, y| rgb.get_pixel(x, y).0;
                match d {
                    Depth::Color => {
                        assert_eq!((at(0, 0), at(1, 0), at(2, 0), at(1, 1)), ([255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]), "{f:?}")
                    }
                    Depth::Grey => assert_eq!((at(0, 1), at(1, 1), at(2, 1)), ([0, 0, 0], [255, 255, 255], [128, 128, 128]), "{f:?}"),
                    Depth::Mono => {
                        assert_eq!((at(0, 1), at(1, 1), at(2, 1), at(2, 0)), ([0, 0, 0], [255, 255, 255], [255, 255, 255], [0, 0, 0]), "{f:?}")
                    }
                }
            }
        }
    }

    #[test]
    fn bad_sizes_are_refused() {
        assert!(encode(&[0; 8], 3, 2, Format::Bmp, Depth::Color, 96.0).is_none());
        assert!(encode(&[], 0, 0, Format::Tif, Depth::Color, 96.0).is_none());
        assert_eq!(Format::parse(".JPEG"), Some(Format::Jpg));
        assert_eq!(Format::parse("tiff"), Some(Format::Tif));
        assert_eq!(Format::parse("gif"), None);
    }
}
