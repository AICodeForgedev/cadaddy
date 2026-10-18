//! Raster image pixels for IMAGE entities. An IMAGE only names its file; the pixels are decoded
//! from that file the first time a view or plot needs them and kept in one process-wide cache,
//! so every view, plot and export of a drawing shares a single copy.
//!
//! A relative file name is looked for in the folders of the drawings opened or saved
//! ([`add_search_dir`]), as AutoCAD resolves it against the drawing's folder. Pixels can also be
//! registered directly ([`insert`]): the web build has no files, and the PDF importer hands over
//! what it extracted. Files that can't be read or decoded are remembered as missing, so a drawing
//! with a broken path doesn't retry every frame; [`forget`] clears that.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Decoded image pixels: `width` × `height`, rows from the top, 4 bytes per pixel (straight,
/// not premultiplied, RGBA).
#[derive(Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    /// `rgba` must hold `width × height × 4` bytes; `None` otherwise or for an empty image.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Raster> {
        let n = u64::from(width).checked_mul(u64::from(height))?.checked_mul(4)?;
        (width > 0 && height > 0 && n == rgba.len() as u64).then_some(Raster { width, height, rgba })
    }

    /// The pixel at column `x`, row `y` (from the top).
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.rgba.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    /// The pixels as a PNG file (RGBA, or RGB when every pixel is opaque).
    pub fn encode_png(&self) -> Option<Vec<u8>> {
        let mut out = std::io::Cursor::new(Vec::new());
        if self.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
            let rgb: Vec<u8> = self.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            image::RgbImage::from_raw(self.width, self.height, rgb)?.write_to(&mut out, image::ImageFormat::Png).ok()?;
        } else {
            image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())?.write_to(&mut out, image::ImageFormat::Png).ok()?;
        }
        Some(out.into_inner())
    }

    /// The image scaled down so neither side exceeds `max` (nearest pixel: GPU textures have a
    /// largest size); itself when it fits.
    pub fn fit(self: &Arc<Self>, max: u32) -> Arc<Raster> {
        if self.width <= max && self.height <= max || max == 0 {
            return Arc::clone(self);
        }
        let k = f64::from(max) / f64::from(self.width.max(self.height));
        let w = ((f64::from(self.width) * k).round() as u32).clamp(1, max);
        let h = ((f64::from(self.height) * k).round() as u32).clamp(1, max);
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            let sy = (u64::from(y) * u64::from(self.height) / u64::from(h)) as u32;
            for x in 0..w {
                let sx = (u64::from(x) * u64::from(self.width) / u64::from(w)) as u32;
                rgba.extend_from_slice(&self.pixel(sx, sy).unwrap_or([0; 4]));
            }
        }
        Arc::new(Raster { width: w, height: h, rgba })
    }
}

/// A one-colour dark image with transparent background (a black logo or stamp from a stencil
/// mask): shown like colour 7, white on a dark background and black on paper.
pub fn is_dark_stencil(r: &Raster) -> bool {
    let mut clear = false;
    for p in r.rgba.as_chunks::<4>().0 {
        if p[3] == 0 {
            clear = true;
        } else if p[0].max(p[1]).max(p[2]) > 38 {
            return false;
        }
    }
    clear
}

/// `r` with every pixel's colour replaced by white (its alpha kept), made once per image.
pub fn white_version(r: &Arc<Raster>) -> Arc<Raster> {
    static WHITE: OnceLock<Mutex<HashMap<usize, (Arc<Raster>, Arc<Raster>)>>> = OnceLock::new();
    let key = Arc::as_ptr(r) as usize;
    let map = WHITE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = map.lock()
        && let Some((_, w)) = m.get(&key)
    {
        return Arc::clone(w);
    }
    let rgba = r.rgba.as_chunks::<4>().0.iter().flat_map(|p| [255, 255, 255, p[3]]).collect();
    let w = Arc::new(Raster { width: r.width, height: r.height, rgba });
    if let Ok(mut m) = map.lock() {
        m.insert(key, (Arc::clone(r), Arc::clone(&w)));
    }
    w
}

/// Largest image decoded, in pixels (a 16k × 16k scan); larger files are treated as missing.
pub const MAX_PIXELS: u64 = 16_384 * 16_384;
/// Pixel memory the cache holds at most; images past it are treated as missing.
pub const MAX_CACHE_BYTES: usize = 2 << 30;

#[derive(Default)]
struct Cache {
    /// Resolved (or registered) path → pixels, `None` when it couldn't be read.
    map: HashMap<String, Option<Arc<Raster>>>,
    bytes: usize,
    dirs: Vec<PathBuf>,
    /// Bumped whenever pixels arrive or are dropped: views keyed on it rebuild.
    generation: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

/// The pixels of the image file `path` (as an IMAGE names it), loading them on first use.
pub fn get(path: &str) -> Option<Arc<Raster>> {
    if path.trim().is_empty() {
        return None;
    }
    let dirs = {
        let c = cache().lock().ok()?;
        if let Some(hit) = c.map.get(path) {
            return hit.clone();
        }
        c.dirs.clone()
    };
    // Decode outside the lock: other threads keep drawing what is already loaded.
    let loaded = resolve(path, &dirs).and_then(|p| load(&p)).map(Arc::new);
    let mut c = cache().lock().ok()?;
    if let Some(hit) = c.map.get(path) {
        return hit.clone();
    }
    let loaded = loaded.filter(|r| c.bytes.saturating_add(r.rgba.len()) <= MAX_CACHE_BYTES);
    if let Some(r) = &loaded {
        c.bytes += r.rgba.len();
        c.generation += 1;
    }
    c.map.insert(path.to_string(), loaded.clone());
    loaded
}

/// Register pixels under `path`, replacing what was cached for it.
pub fn insert(path: &str, raster: impl Into<Arc<Raster>>) {
    let raster = raster.into();
    let Ok(mut c) = cache().lock() else { return };
    let n = raster.rgba.len();
    if let Some(Some(old)) = c.map.insert(path.to_string(), Some(raster)) {
        c.bytes = c.bytes.saturating_sub(old.rgba.len());
    }
    c.bytes = c.bytes.saturating_add(n);
    c.generation += 1;
}

/// Drop what is cached for `path` (pixels or "missing"), so the next use reads the file again.
pub fn forget(path: &str) {
    let Ok(mut c) = cache().lock() else { return };
    if let Some(Some(old)) = c.map.remove(path) {
        c.bytes = c.bytes.saturating_sub(old.rgba.len());
    }
    c.generation += 1;
}

/// Also look for relative image paths in `dir` (a drawing's folder). Images found missing are
/// tried again.
pub fn add_search_dir(dir: &Path) {
    let Ok(mut c) = cache().lock() else { return };
    if !c.dirs.iter().any(|d| d == dir) {
        c.dirs.push(dir.to_path_buf());
        c.map.retain(|_, v| v.is_some());
        c.generation += 1;
    }
}

/// Changes whenever cached pixels change (loaded, registered or dropped).
pub fn generation() -> u64 {
    cache().lock().map(|c| c.generation).unwrap_or(0)
}

/// The file `path` names: itself when it exists, else its name (or relative path) in each
/// search folder.
fn resolve(path: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let p = Path::new(path);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    // A Windows path read on another system: its last component.
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    dirs.iter().flat_map(|d| [d.join(path), d.join(name)]).find(|c| c.is_file())
}

/// Decode an image file (PNG, JPEG).
fn load(path: &Path) -> Option<Raster> {
    let bytes = std::fs::read(path).ok()?;
    decode(&bytes)
}

/// Decode image file bytes (PNG, JPEG) into straight RGBA, within [`MAX_PIXELS`].
pub fn decode(bytes: &[u8]) -> Option<Raster> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    let img = reader.decode().ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    Raster::new(w, h, img.into_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(px));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn files_load_once_and_missing_ones_are_remembered() {
        let dir = std::env::temp_dir().join(format!("cadaddy-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("red.png");
        std::fs::write(&file, png(3, 2, [255, 0, 0, 255])).unwrap();
        let r = get(file.to_str().unwrap()).unwrap();
        assert_eq!((r.width, r.height), (3, 2));
        assert_eq!(r.pixel(2, 1), Some([255, 0, 0, 255]));
        assert!(Arc::ptr_eq(&r, &get(file.to_str().unwrap()).unwrap()), "cached");
        // relative: found once its folder is a search folder
        assert!(get("cadaddy-test-elsewhere/red.png").is_none());
        add_search_dir(&dir);
        assert!(get("red.png").is_some());
        assert!(get("C:\\Somewhere\\Else\\red.png").is_some(), "a Windows path by its file name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registered_pixels_and_hostile_bytes() {
        insert("mem://logo", Raster::new(1, 1, vec![1, 2, 3, 4]).unwrap());
        assert_eq!(get("mem://logo").unwrap().pixel(0, 0), Some([1, 2, 3, 4]));
        assert!(decode(b"not an image").is_none());
        assert!(Raster::new(2, 2, vec![0; 3]).is_none());
        assert!(get("").is_none());
    }

    #[test]
    fn png_round_trip() {
        let r = Raster::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 128]).unwrap();
        assert_eq!(decode(&r.encode_png().unwrap()).unwrap(), r);
        let opaque = Raster::new(1, 1, vec![9, 8, 7, 255]).unwrap();
        assert_eq!(decode(&opaque.encode_png().unwrap()).unwrap(), opaque);
    }

    #[test]
    fn fit_shrinks_to_the_largest_side() {
        let r = Arc::new(Raster::new(10, 4, vec![9; 160]).unwrap());
        let s = r.fit(5);
        assert_eq!((s.width, s.height), (5, 2));
        assert!(Arc::ptr_eq(&r, &r.fit(10)));
    }
}
