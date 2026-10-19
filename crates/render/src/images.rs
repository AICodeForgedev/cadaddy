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

/// `r` with an IMAGE's brightness and contrast (0..=100, 50 leaves it as it is) applied, made once
/// per image and setting. Contrast scales each channel about mid-grey (0 flat grey, 100 twice as
/// steep), then brightness shifts it (0 black, 100 white); alpha is kept. Only the source's weak
/// reference is held, so an image dropped from the cache frees its adjusted copies too.
pub fn adjusted(r: &Arc<Raster>, brightness: u8, contrast: u8) -> Arc<Raster> {
    type Adjusted = HashMap<(usize, u8, u8), (std::sync::Weak<Raster>, Arc<Raster>)>;
    static ADJUSTED: OnceLock<Mutex<Adjusted>> = OnceLock::new();
    let (b, c) = (brightness.min(100), contrast.min(100));
    if (b, c) == (50, 50) {
        return Arc::clone(r);
    }
    let key = (Arc::as_ptr(r) as usize, b, c);
    let map = ADJUSTED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = map.lock()
        && let Some((src, a)) = m.get(&key)
        && src.upgrade().is_some_and(|s| Arc::ptr_eq(&s, r))
    {
        return Arc::clone(a);
    }
    let k = f64::from(c) / 50.0;
    let shift = (f64::from(b) - 50.0) / 50.0 * 255.0;
    let lut: Vec<u8> = (0..=255u16).map(|v| ((f64::from(v) - 127.5) * k + 127.5 + shift).round().clamp(0.0, 255.0) as u8).collect();
    let tone = |v: u8| lut.get(usize::from(v)).copied().unwrap_or(v);
    let rgba = r.rgba.as_chunks::<4>().0.iter().flat_map(|p| [tone(p[0]), tone(p[1]), tone(p[2]), p[3]]).collect();
    let a = Arc::new(Raster { width: r.width, height: r.height, rgba });
    if let Ok(mut m) = map.lock() {
        m.retain(|_, (src, _)| src.strong_count() > 0);
        m.insert(key, (Arc::downgrade(r), Arc::clone(&a)));
    }
    a
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
    /// The folder relative image paths are taken from: the current drawing's.
    dir: Option<PathBuf>,
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
    let dir = {
        let c = cache().lock().ok()?;
        if let Some(hit) = c.map.get(path) {
            return hit.clone();
        }
        c.dir.clone()
    };
    // Decode outside the lock: other threads keep drawing what is already loaded.
    let loaded = resolve(path, dir.as_deref()).and_then(|p| load(&p)).map(Arc::new);
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

/// The pixels of an image file a drawing carries inside itself (its bytes), decoded on first use
/// and cached by content.
pub fn embedded(bytes: &[u8]) -> Option<Arc<Raster>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    // Not a path any IMAGE names.
    let key = format!("\u{1}embedded:{:016x}:{}", h.finish(), bytes.len());
    if let Some(hit) = cache().lock().ok()?.map.get(&key) {
        return hit.clone();
    }
    let loaded = decode(bytes).map(Arc::new);
    let mut c = cache().lock().ok()?;
    let loaded = loaded.filter(|r| c.bytes.saturating_add(r.rgba.len()) <= MAX_CACHE_BYTES);
    if let Some(r) = &loaded {
        c.bytes += r.rgba.len();
        c.generation += 1;
    }
    c.map.insert(key, loaded.clone());
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

/// Take relative image paths from `dir`, the current drawing's folder (as it is opened, saved or
/// made current), and from no other folder: two jobs converted alike use the same relative
/// names, and one's images must never show in the other. A new folder, or the same one opened
/// again, looks every relative path and every missing image up again (an images folder copied in
/// after the drawing was opened is found).
pub fn add_search_dir(dir: &Path) {
    let Ok(mut c) = cache().lock() else { return };
    c.dir = Some(dir.to_path_buf());
    let mut freed = 0usize;
    c.map.retain(|k, v| {
        // Relative paths are looked up again in the new folder; full paths and registered pixels
        // (`mem://…` and other URL-like keys, no file behind them) stay. Registered pixels were
        // dropped here, a drawing's in-memory images going with a change of folder.
        let keep = v.is_some() && (Path::new(k).is_absolute() || k.contains("://"));
        if !keep {
            freed += v.as_ref().map_or(0, |r| r.rgba.len());
        }
        keep
    });
    c.bytes = c.bytes.saturating_sub(freed);
    c.generation += 1;
}

/// Drop the cached pixels of images no open drawing names (`used`: the paths their IMAGEs
/// give), as drawings close. A closed drawing's images stayed decoded until the program exited:
/// hundreds of megabytes after a sheet full of them.
pub fn keep_only(used: &std::collections::HashSet<&str>) {
    let Ok(mut c) = cache().lock() else { return };
    let mut freed = 0usize;
    let before = c.map.len();
    c.map.retain(|k, v| {
        let keep = used.contains(k.as_str());
        if !keep {
            freed += v.as_ref().map_or(0, |r| r.rgba.len());
        }
        keep
    });
    if c.map.len() != before {
        c.bytes = c.bytes.saturating_sub(freed);
        c.generation += 1;
    }
}

/// Look image files up again where they may have changed on disk: cached pixels whose file path
/// no longer names a file (moved, renamed, deleted) are dropped, and "couldn't be read" results
/// are forgotten, so the next draw reads the files afresh. Registered pixels (no file behind
/// them) are kept. REGEN and opening a drawing call this.
pub fn revalidate() {
    let Ok(mut c) = cache().lock() else { return };
    let mut freed = 0usize;
    let before = c.map.len();
    c.map.retain(|k, v| match v {
        None => false,
        // An absolute path whose file is gone (moved, renamed, deleted). Relative paths follow the
        // drawing's folder (`add_search_dir`), and keys with no file behind them hold registered
        // pixels; both are left alone.
        Some(r) if Path::new(k).is_absolute() && !Path::new(k).is_file() => {
            freed += r.rgba.len();
            false
        }
        Some(_) => true,
    });
    if c.map.len() != before {
        c.bytes = c.bytes.saturating_sub(freed);
        c.generation += 1;
    }
}

/// Changes whenever cached pixels change (loaded, registered or dropped).
pub fn generation() -> u64 {
    cache().lock().map(|c| c.generation).unwrap_or(0)
}

/// The file `path` names: itself when it is a full path to a file, else its relative path (or
/// its name) in the current drawing's folder. A relative path is never taken from the program's
/// working folder, which has nothing to do with the drawing (a program started from the Start
/// menu works in its install or system folder), nor from another drawing's folder.
fn resolve(path: &str, dir: Option<&Path>) -> Option<PathBuf> {
    use cadaddy_geom::filename::{find_file, native};
    // Names match ignoring case, `\` separating folders, as on Windows.
    let norm = native(path);
    let p = Path::new(&norm);
    if p.is_absolute()
        && let Some(f) = find_file(p)
    {
        return Some(f);
    }
    // A Windows path read on another system: its last component.
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let d = dir?;
    [d.join(&norm), d.join(name)].into_iter().find_map(find_file)
}

/// Whether `dir` is already the folder relative image paths are taken from.
pub fn is_search_dir(dir: &Path) -> bool {
    cache().lock().is_ok_and(|c| c.dir.as_deref() == Some(dir))
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

    /// The image folder is process-wide: tests that set it take turns.
    fn folder_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn png(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(px));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn relative_images_follow_the_drawing_opened_or_saved() {
        let _turn = folder_lock();
        let base = std::env::temp_dir().join(format!("cadaddy-relimg-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(b.join("pics")).unwrap();
        let name = "pics/relimg-logo.png";
        // Opened before its images folder was copied in: missing, then found on the next open.
        add_search_dir(&a);
        assert!(get(name).is_none());
        std::fs::create_dir_all(a.join("pics")).unwrap();
        std::fs::write(a.join(name), png(2, 2, [255, 0, 0, 255])).unwrap();
        add_search_dir(&a);
        assert_eq!(get(name).and_then(|r| r.pixel(0, 0)), Some([255, 0, 0, 255]));
        // Another drawing using the same relative name in its own folder gets its own image.
        std::fs::write(b.join(name), png(2, 2, [0, 0, 255, 255])).unwrap();
        add_search_dir(&b);
        assert_eq!(get(name).and_then(|r| r.pixel(0, 0)), Some([0, 0, 255, 255]));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn files_load_once_and_missing_ones_are_remembered() {
        let _turn = folder_lock();
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
    fn brightness_and_contrast_adjust_once_per_setting() {
        let r = Arc::new(Raster::new(2, 1, vec![0, 128, 255, 255, 32, 32, 32, 10]).unwrap());
        assert!(Arc::ptr_eq(&r, &adjusted(&r, 50, 50)), "the defaults leave it alone");
        let bright = adjusted(&r, 100, 50);
        assert_eq!(bright.pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(bright.pixel(1, 0).map(|p| p[3]), Some(10), "alpha kept");
        assert_eq!(adjusted(&r, 0, 50).pixel(0, 0), Some([0, 0, 0, 255]));
        let flat = adjusted(&r, 50, 0);
        assert_eq!(flat.pixel(0, 0), Some([128, 128, 128, 255]));
        let steep = adjusted(&r, 50, 100);
        assert_eq!(steep.pixel(1, 0), Some([0, 0, 0, 10]), "dark grey pushed to black");
        assert!(Arc::ptr_eq(&steep, &adjusted(&r, 50, 100)), "cached");
        assert_eq!(adjusted(&r, 200, 200).pixel(0, 0), adjusted(&r, 100, 100).pixel(0, 0), "clamped to 100");
    }

    #[test]
    fn fit_shrinks_to_the_largest_side() {
        let r = Arc::new(Raster::new(10, 4, vec![9; 160]).unwrap());
        let s = r.fit(5);
        assert_eq!((s.width, s.height), (5, 2));
        assert!(Arc::ptr_eq(&r, &r.fit(10)));
    }
}
