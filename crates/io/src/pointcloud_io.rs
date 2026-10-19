//! Point cloud files: LAS 1.0–1.4 uncompressed (point data record formats 0–10, after the ASPRS
//! LAS Specification 1.4 R15, which is public), and PTS / XYZ text. Compressed LAZ is refused.
//! Written: LAS 1.2, point format 2 (coordinates, intensity, colour), which every LAS reader
//! takes.

use cadaddy_doc::pointcloud::Cloud;

/// Most points read from one file.
pub const MAX_POINTS: u64 = 50_000_000;

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn i32le(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}
fn f64le(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Where a point record keeps its colour (format → offset of red), if it has one.
fn rgb_offset(format: u8) -> Option<usize> {
    match format {
        2 => Some(20),
        3 | 5 => Some(28),
        7 | 8 | 10 => Some(30),
        _ => None,
    }
}

/// The shortest record of each format.
fn min_record(format: u8) -> Option<usize> {
    Some(match format {
        0 => 20,
        1 => 28,
        2 => 26,
        3 => 34,
        4 => 57,
        5 => 63,
        6 => 30,
        7 => 36,
        8 => 38,
        9 => 59,
        10 => 67,
        _ => return None,
    })
}

/// Read a LAS file.
pub fn read_las(b: &[u8]) -> Result<Cloud, String> {
    if b.get(..4) != Some(b"LASF") {
        return Err("not a LAS file".into());
    }
    let bad = || "a damaged LAS header".to_string();
    let (major, minor) = (*b.get(24).ok_or_else(bad)?, *b.get(25).ok_or_else(bad)?);
    let offset = u32le(b, 96).ok_or_else(bad)? as usize;
    let raw_format = *b.get(104).ok_or_else(bad)?;
    if raw_format & 0xC0 != 0 {
        return Err("compressed LAZ point clouds can't be read; decompress the file to LAS first".into());
    }
    let format = raw_format & 0x3F;
    let len = usize::from(u16le(b, 105).ok_or_else(bad)?);
    let min = min_record(format).ok_or_else(|| format!("LAS point format {format} isn't known"))?;
    if len < min {
        return Err(format!("LAS records of {len} bytes are too short for format {format}"));
    }
    let legacy = u64::from(u32le(b, 107).ok_or_else(bad)?);
    let count = if (major, minor) >= (1, 4) && legacy == 0 { u64le(b, 247).unwrap_or(0) } else { legacy };
    let scale = [f64le(b, 131), f64le(b, 139), f64le(b, 147)];
    let off = [f64le(b, 155), f64le(b, 163), f64le(b, 171)];
    let [Some(sx), Some(sy), Some(sz)] = scale else { return Err(bad()) };
    let [Some(ox), Some(oy), Some(oz)] = off else { return Err(bad()) };
    if ![sx, sy, sz, ox, oy, oz].iter().all(|v| v.is_finite()) {
        return Err(bad());
    }
    // What the file really holds bounds what it claims.
    let fits = (b.len().saturating_sub(offset) / len) as u64;
    let n = count.min(fits).min(MAX_POINTS) as usize;
    let rgb = rgb_offset(format);
    let mut cloud = Cloud::default();
    cloud.points.reserve(n);
    cloud.intensity.reserve(n);
    if rgb.is_some() {
        cloud.colors.reserve(n);
    }
    let mut raw_colors: Vec<[u16; 3]> = Vec::new();
    for i in 0..n {
        let at = offset + i * len;
        let (Some(x), Some(y), Some(z)) = (i32le(b, at), i32le(b, at + 4), i32le(b, at + 8)) else { break };
        cloud.points.push([(f64::from(x) * sx + ox) as f32, (f64::from(y) * sy + oy) as f32, (f64::from(z) * sz + oz) as f32]);
        cloud.intensity.push(u16le(b, at + 12).unwrap_or(0));
        if let Some(c) = rgb {
            raw_colors.push([u16le(b, at + c).unwrap_or(0), u16le(b, at + c + 2).unwrap_or(0), u16le(b, at + c + 4).unwrap_or(0)]);
        }
    }
    // Colours are 16-bit in the specification; some writers store 8-bit values.
    let wide = raw_colors.iter().any(|c| c.iter().any(|v| *v > 255));
    cloud.colors = raw_colors.iter().map(|c| c.map(|v| if wide { (v >> 8) as u8 } else { v as u8 })).collect();
    Ok(cloud)
}

/// Read PTS (a count line, then `x y z [intensity [r g b]]`) or XYZ (`x y z [r g b]`) text.
pub fn read_text(text: &str, pts: bool) -> Result<Cloud, String> {
    let mut cloud = Cloud::default();
    let mut has_color = true;
    let mut has_intensity = pts;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let v: Vec<f64> = line.split([' ', ',', '\t', ';']).filter(|s| !s.is_empty()).filter_map(|s| s.parse::<f64>().ok()).collect();
        if pts && i == 0 && v.len() == 1 {
            continue;
        }
        let [x, y, z, rest @ ..] = v.as_slice() else { continue };
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            continue;
        }
        if cloud.points.len() as u64 >= MAX_POINTS {
            break;
        }
        cloud.points.push([*x as f32, *y as f32, *z as f32]);
        let (intensity, rgb) = if pts { (rest.first(), rest.get(1..4)) } else { (None, rest.get(0..3)) };
        match intensity {
            // PTS intensities run -2048..2047.
            Some(v) => cloud.intensity.push(((v + 2048.0).clamp(0.0, 4095.0) * 16.0) as u16),
            None => has_intensity = false,
        }
        match rgb {
            Some([r, g, b]) => cloud.colors.push([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8]),
            _ => has_color = false,
        }
    }
    if cloud.points.is_empty() {
        return Err("no points in the file".into());
    }
    if !has_color {
        cloud.colors.clear();
    }
    if !has_intensity {
        cloud.intensity.clear();
    }
    Ok(cloud)
}

/// Read a point cloud file by its name's extension (`.las`, `.pts`, `.xyz`, `.txt`, `.csv`).
pub fn read(bytes: &[u8], name: &str) -> Result<Cloud, String> {
    let ext = std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "las" => read_las(bytes),
        "laz" => Err("compressed LAZ point clouds can't be read; decompress the file to LAS first".into()),
        "pts" => read_text(&String::from_utf8_lossy(bytes), true),
        "xyz" | "txt" | "csv" => read_text(&String::from_utf8_lossy(bytes), false),
        _ if bytes.starts_with(b"LASF") => read_las(bytes),
        other => Err(format!("`{other}` point clouds can't be read (LAS, PTS or XYZ)")),
    }
}

/// `cloud` as a LAS 1.2 file, point format 2, millimetre-ish precision (scale 0.001).
pub fn write_las(cloud: &Cloud) -> Vec<u8> {
    let n = cloud.points.len();
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for p in &cloud.points {
        for k in 0..3 {
            min[k] = min[k].min(f64::from(p[k]));
            max[k] = max[k].max(f64::from(p[k]));
        }
    }
    if n == 0 {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    let scale: f64 = 0.001;
    let mut h = vec![0u8; 227];
    h[..4].copy_from_slice(b"LASF");
    h[24] = 1;
    h[25] = 2;
    let sw = b"CADaddy";
    h[58..58 + sw.len()].copy_from_slice(sw);
    h[94..96].copy_from_slice(&227u16.to_le_bytes());
    h[96..100].copy_from_slice(&227u32.to_le_bytes());
    h[104] = 2;
    h[105..107].copy_from_slice(&26u16.to_le_bytes());
    h[107..111].copy_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
    for at in [131usize, 139, 147] {
        h[at..at + 8].copy_from_slice(&scale.to_le_bytes());
    }
    for (k, at) in [155usize, 163, 171].iter().enumerate() {
        h[*at..*at + 8].copy_from_slice(&min[k].to_le_bytes());
    }
    for (k, (hi, lo)) in [(179usize, 187usize), (195, 203), (211, 219)].iter().enumerate() {
        h[*hi..*hi + 8].copy_from_slice(&max[k].to_le_bytes());
        h[*lo..*lo + 8].copy_from_slice(&min[k].to_le_bytes());
    }
    let mut out = h;
    out.reserve(n * 26);
    for (i, p) in cloud.points.iter().enumerate() {
        for k in 0..3 {
            let v = ((f64::from(p[k]) - min[k]) / scale).round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&cloud.intensity.get(i).copied().unwrap_or(0).to_le_bytes());
        // Return, classification, scan angle, user data, point source (6 bytes), then RGB at 20.
        out.extend_from_slice(&[0u8; 6]);
        let c = cloud.colors.get(i).copied().unwrap_or([0; 3]);
        for v in c {
            out.extend_from_slice(&(u16::from(v) << 8 | u16::from(v)).to_le_bytes());
        }
    }
    out
}
