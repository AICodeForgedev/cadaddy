//! Compiled shape fonts (SHX), read from a user's own files: regular shape fonts
//! (`AutoCAD-86 shapes 1.0/1.1`), Unicode shape fonts (`AutoCAD-86 unifont 1.0`) and big fonts
//! (`AutoCAD-86 bigfont 1.0`, double-byte codes for Asian scripts). Nothing is bundled: a text style
//! naming an .shx file draws with it when the file is found on the font search path, and with the
//! built-in stroke font otherwise.
//!
//! Written from the public descriptions of the shape definition codes (vector lengths and 16
//! directions; codes 0-14: end, pen down/up, scale, push/pop, subshape, displacements, octant and
//! fractional arcs, bulge arcs, vertical-only) and of the compiled file layouts. Every count,
//! offset and recursion is bounded: these are untrusted files.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use cadaddy_geom::Vec2;

/// Largest SHX file read.
const MAX_FILE: u64 = 32 * 1024 * 1024;
/// Most shapes a font holds, bytes one definition takes, nested subshapes, commands run per glyph,
/// points per glyph and stack depth.
const MAX_SHAPES: usize = 70_000;
const MAX_DEF: usize = 4096;
const MAX_DEPTH: usize = 8;
const MAX_COMMANDS: usize = 20_000;
const MAX_POINTS: usize = 20_000;
const MAX_STACK: usize = 16;

/// What kind of compiled font a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shapes,
    Unifont,
    Bigfont,
}

/// A parsed shape font: each shape's definition bytes (the spec after its name) by number, and
/// the font's height above and below the baseline (from shape 0 or the font information).
#[derive(Clone, Debug)]
pub struct ShxFont {
    pub kind: Kind,
    pub above: f64,
    pub below: f64,
    shapes: HashMap<u32, Vec<u8>>,
    /// Big fonts: lead-byte ranges of the double-byte codes.
    escapes: Vec<(u8, u8)>,
}

/// One drawn glyph in font units (baseline at y = 0): its strokes and where the pen ended (the
/// next glyph's origin).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glyph {
    pub strokes: Vec<Vec<Vec2>>,
    pub advance: Vec2,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}

/// A definition's spec: the bytes after its zero-terminated name.
fn spec_of(def: &[u8]) -> Vec<u8> {
    let start = def.iter().position(|b| *b == 0).map_or(0, |i| i + 1);
    def.get(start..).unwrap_or(&[]).iter().take(MAX_DEF).copied().collect()
}

/// Above / below from a font information spec (`above, below, modes, …`).
fn font_info(spec: &[u8]) -> (f64, f64) {
    let above = spec.first().copied().filter(|a| *a > 0).map_or(1.0, f64::from);
    let below = spec.get(1).copied().map_or(0.0, f64::from);
    (above, below)
}

impl ShxFont {
    /// Parse a compiled shape font; `None` when it isn't one or is damaged.
    pub fn parse(bytes: &[u8]) -> Option<ShxFont> {
        let end = bytes.iter().take(64).position(|b| *b == 0x1a)?;
        let header = String::from_utf8_lossy(bytes.get(..end)?).to_ascii_lowercase();
        let body = bytes.get(end + 1..)?;
        if header.starts_with("autocad-86 shapes") {
            Self::parse_shapes(body)
        } else if header.starts_with("autocad-86 unifont") {
            Self::parse_unifont(body)
        } else if header.starts_with("autocad-86 bigfont") {
            Self::parse_bigfont(bytes, body, end + 1)
        } else {
            None
        }
    }

    /// `first, last, count` (u16), `count` × (number, length) (u16), then the definitions in turn.
    fn parse_shapes(b: &[u8]) -> Option<ShxFont> {
        let n = usize::from(u16_at(b, 4)?).min(MAX_SHAPES);
        let mut at = 6 + 4 * n;
        let mut shapes = HashMap::new();
        for k in 0..n {
            let num = u16_at(b, 6 + 4 * k)?;
            let len = usize::from(u16_at(b, 8 + 4 * k)?);
            let def = b.get(at..at.checked_add(len)?)?;
            shapes.insert(u32::from(num), spec_of(def));
            at += len;
        }
        let (above, below) = shapes.get(&0).map_or((1.0, 0.0), |s| font_info(s));
        Some(ShxFont { kind: Kind::Shapes, above, below, shapes, escapes: Vec::new() })
    }

    /// `count` (u32), font information (u16 length, then name and above, below, …), then each
    /// shape as number (u16), length (u16) and definition.
    fn parse_unifont(b: &[u8]) -> Option<ShxFont> {
        let n = usize::try_from(u32_at(b, 0)?).ok()?.min(MAX_SHAPES);
        let info_len = usize::from(u16_at(b, 4)?);
        let info = b.get(6..6usize.checked_add(info_len)?)?;
        let (above, below) = font_info(&spec_of(info));
        let mut at = 6 + info_len;
        let mut shapes = HashMap::new();
        for _ in 0..n {
            let (Some(num), Some(len)) = (u16_at(b, at), u16_at(b, at + 2)) else { break };
            let len = usize::from(len);
            let Some(def) = b.get(at + 4..(at + 4).saturating_add(len)) else { break };
            shapes.insert(u32::from(num), spec_of(def));
            at += 4 + len;
        }
        Some(ShxFont { kind: Kind::Unifont, above, below, shapes, escapes: Vec::new() })
    }

    /// Index size (u16), shape count (u16), escape range count (u16), the lead-byte ranges
    /// (u16 start, u16 end), then (number u16, length u16, file offset u32) per shape.
    fn parse_bigfont(file: &[u8], b: &[u8], body_at: usize) -> Option<ShxFont> {
        let n = usize::from(u16_at(b, 2)?).min(MAX_SHAPES);
        let ranges = usize::from(u16_at(b, 4)?).min(256);
        let mut escapes = Vec::new();
        for k in 0..ranges {
            let (s, e) = (u16_at(b, 6 + 4 * k)?, u16_at(b, 8 + 4 * k)?);
            escapes.push((u8::try_from(s.min(255)).ok()?, u8::try_from(e.min(255)).ok()?));
        }
        let index = 6 + 4 * ranges;
        let mut shapes = HashMap::new();
        for k in 0..n {
            let at = index + 8 * k;
            let (Some(num), Some(len), Some(off)) = (u16_at(b, at), u16_at(b, at + 2), u32_at(b, at + 4)) else { break };
            let off = usize::try_from(off).ok()?;
            // Offsets count from the start of the file; also accept them from the data's start.
            let def = file.get(off..off.saturating_add(usize::from(len))).or_else(|| b.get(off.saturating_sub(body_at)..)?.get(..usize::from(len)));
            if let Some(def) = def {
                shapes.insert(u32::from(num), spec_of(def));
            }
        }
        let (above, below) = shapes.get(&0).map_or((1.0, 0.0), |s| font_info(s));
        Some(ShxFont { kind: Kind::Bigfont, above, below, shapes, escapes })
    }

    /// Whether the font has a shape for `code`.
    pub fn has(&self, code: u32) -> bool {
        code != 0 && self.shapes.contains_key(&code)
    }

    /// The shape number a character has in this font: its code point (shape and Unicode fonts),
    /// or its double-byte code in an Asian code page (big fonts).
    pub fn code_of(&self, c: char) -> Option<u32> {
        match self.kind {
            Kind::Shapes | Kind::Unifont => Some(u32::from(c)).filter(|k| self.has(*k)),
            Kind::Bigfont => {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                for enc in [encoding_rs::SHIFT_JIS, encoding_rs::GBK, encoding_rs::BIG5, encoding_rs::EUC_KR] {
                    let (bytes, _, bad) = enc.encode(s);
                    if bad {
                        continue;
                    }
                    let code = match bytes.as_ref() {
                        [lead, trail] if self.escapes.is_empty() || self.escapes.iter().any(|(a, b)| (*a..=*b).contains(lead)) => {
                            (u32::from(*lead) << 8) | u32::from(*trail)
                        }
                        [one] => u32::from(*one),
                        _ => continue,
                    };
                    if self.has(code) {
                        return Some(code);
                    }
                }
                None
            }
        }
    }

    /// Draw shape `code` (`vertical`: the vertical-text-only commands run too).
    pub fn glyph(&self, code: u32, vertical: bool) -> Option<Glyph> {
        let spec = self.shapes.get(&code)?;
        let mut st = Pen {
            pos: Vec2::ZERO,
            down: true,
            scale: 1.0,
            stack: Vec::new(),
            strokes: Vec::new(),
            line: vec![Vec2::ZERO],
            budget: MAX_COMMANDS,
            points: 0,
        };
        self.run(spec, vertical, 0, &mut st);
        st.lift();
        Some(Glyph { strokes: st.strokes, advance: st.pos })
    }

    fn run(&self, spec: &[u8], vertical: bool, depth: usize, st: &mut Pen) {
        let mut i = 0;
        let byte = |i: usize| spec.get(i).copied();
        let sbyte = |i: usize| spec.get(i).map(|b| f64::from(*b as i8));
        // Skip the next command (a vertical-only one in horizontal text).
        let mut skip = false;
        while let Some(b) = byte(i) {
            if st.budget == 0 {
                return;
            }
            st.budget -= 1;
            let draw = !skip;
            skip = false;
            i += 1;
            match b {
                0 => return,
                1 if draw => st.pen(true),
                2 if draw => st.pen(false),
                1 | 2 => {}
                3 => {
                    if draw && let Some(d) = byte(i).filter(|d| *d > 0) {
                        st.scale /= f64::from(d);
                    }
                    i += 1;
                }
                4 => {
                    if draw && let Some(m) = byte(i) {
                        st.scale *= f64::from(m);
                    }
                    i += 1;
                }
                5 if draw => {
                    if st.stack.len() < MAX_STACK {
                        st.stack.push(st.pos);
                    }
                }
                6 if draw => {
                    if let Some(p) = st.stack.pop() {
                        st.jump(p);
                    }
                }
                5 | 6 => {}
                7 => {
                    // Unicode fonts name the subshape in two bytes (high first); big fonts use 0 then
                    // an extended form, which is skipped.
                    let (sub, used) = match self.kind {
                        Kind::Unifont => (byte(i).zip(byte(i + 1)).map(|(h, l)| (u32::from(h) << 8) | u32::from(l)), 2),
                        Kind::Bigfont if byte(i) == Some(0) => (byte(i + 1).zip(byte(i + 2)).map(|(h, l)| (u32::from(h) << 8) | u32::from(l)), 7),
                        _ => (byte(i).map(u32::from), 1),
                    };
                    i += used;
                    if draw
                        && depth < MAX_DEPTH
                        && let Some(s) = sub.and_then(|k| self.shapes.get(&k))
                    {
                        self.run(s, vertical, depth + 1, st);
                    }
                }
                8 => {
                    if let (Some(dx), Some(dy)) = (sbyte(i), sbyte(i + 1))
                        && draw
                    {
                        st.go(Vec2::new(dx, dy) * st.scale);
                    }
                    i += 2;
                }
                9 => loop {
                    let (Some(dx), Some(dy)) = (sbyte(i), sbyte(i + 1)) else { return };
                    i += 2;
                    if dx == 0.0 && dy == 0.0 {
                        break;
                    }
                    if draw {
                        st.go(Vec2::new(dx, dy) * st.scale);
                    }
                },
                10 => {
                    let (Some(r), Some(o)) = (byte(i), byte(i + 1)) else { return };
                    i += 2;
                    if draw {
                        let cw = o & 0x80 != 0;
                        let start = f64::from((o >> 4) & 0x7) * 45.0;
                        let n = match o & 0x0f {
                            0 => 8,
                            n => n,
                        };
                        let sweep = f64::from(n) * 45.0 * if cw { -1.0 } else { 1.0 };
                        st.arc(f64::from(r) * st.scale, start, sweep);
                    }
                }
                11 => {
                    let (Some(so), Some(eo), Some(rh), Some(rl), Some(o)) = (byte(i), byte(i + 1), byte(i + 2), byte(i + 3), byte(i + 4)) else {
                        return;
                    };
                    i += 5;
                    if draw {
                        let cw = o & 0x80 != 0;
                        let r = f64::from(u16::from(rh) << 8 | u16::from(rl));
                        let start = f64::from((o >> 4) & 0x7) * 45.0 + f64::from(so) * 45.0 / 256.0;
                        let n = match o & 0x0f {
                            0 => 8,
                            n => n,
                        };
                        let mut sweep = f64::from(n) * 45.0 - f64::from(so) * 45.0 / 256.0;
                        if eo > 0 {
                            sweep -= 45.0 - f64::from(eo) * 45.0 / 256.0;
                        }
                        st.arc(r * st.scale, start, if cw { -sweep } else { sweep });
                    }
                }
                12 => {
                    let (Some(dx), Some(dy), Some(bu)) = (sbyte(i), sbyte(i + 1), sbyte(i + 2)) else { return };
                    i += 3;
                    if draw {
                        st.bulge(Vec2::new(dx, dy) * st.scale, bu / 127.0);
                    }
                }
                13 => loop {
                    let (Some(dx), Some(dy)) = (sbyte(i), sbyte(i + 1)) else { return };
                    i += 2;
                    if dx == 0.0 && dy == 0.0 {
                        break;
                    }
                    let Some(bu) = sbyte(i) else { return };
                    i += 1;
                    if draw {
                        st.bulge(Vec2::new(dx, dy) * st.scale, bu / 127.0);
                    }
                },
                14 => skip = !vertical,
                v => {
                    if draw {
                        let len = f64::from(v >> 4);
                        st.go(DIRECTIONS.get(usize::from(v & 0x0f)).copied().unwrap_or(Vec2::ZERO) * (len * st.scale));
                    }
                }
            }
        }
    }
}

/// The 16 vector directions: east, then counter-clockwise in steps of half a unit.
const DIRECTIONS: [Vec2; 16] = [
    Vec2 { x: 1.0, y: 0.0 },
    Vec2 { x: 1.0, y: 0.5 },
    Vec2 { x: 1.0, y: 1.0 },
    Vec2 { x: 0.5, y: 1.0 },
    Vec2 { x: 0.0, y: 1.0 },
    Vec2 { x: -0.5, y: 1.0 },
    Vec2 { x: -1.0, y: 1.0 },
    Vec2 { x: -1.0, y: 0.5 },
    Vec2 { x: -1.0, y: 0.0 },
    Vec2 { x: -1.0, y: -0.5 },
    Vec2 { x: -1.0, y: -1.0 },
    Vec2 { x: -0.5, y: -1.0 },
    Vec2 { x: 0.0, y: -1.0 },
    Vec2 { x: 0.5, y: -1.0 },
    Vec2 { x: 1.0, y: -1.0 },
    Vec2 { x: 1.0, y: -0.5 },
];

/// The pen while a shape is drawn.
struct Pen {
    pos: Vec2,
    down: bool,
    scale: f64,
    stack: Vec<Vec2>,
    strokes: Vec<Vec<Vec2>>,
    /// The polyline being drawn (from the last pen-down or jump).
    line: Vec<Vec2>,
    budget: usize,
    points: usize,
}

impl Pen {
    fn lift(&mut self) {
        let l = std::mem::take(&mut self.line);
        if l.len() >= 2 {
            self.strokes.push(l);
        }
    }
    fn pen(&mut self, down: bool) {
        if down && !self.down {
            self.line = vec![self.pos];
        } else if !down && self.down {
            self.lift();
        }
        self.down = down;
    }
    fn jump(&mut self, p: Vec2) {
        self.lift();
        self.pos = p;
        self.line = vec![p];
    }
    fn to(&mut self, p: Vec2) {
        if !p.is_finite() || self.points >= MAX_POINTS {
            return;
        }
        self.pos = p;
        if self.down {
            self.line.push(p);
            self.points += 1;
        }
    }
    fn go(&mut self, d: Vec2) {
        let p = self.pos + d;
        if self.down && self.line.is_empty() {
            self.line.push(self.pos);
        }
        if !self.down {
            self.line = vec![p];
        }
        self.to(p);
    }
    /// An arc of radius `r` from the current point (on the circle at angle `start`, degrees),
    /// sweeping `sweep` degrees (negative: clockwise).
    fn arc(&mut self, r: f64, start: f64, sweep: f64) {
        let (a0, s) = (start.to_radians(), sweep.to_radians());
        let c = self.pos - Vec2::from_angle(a0) * r;
        let n = ((s.abs() / (std::f64::consts::PI / 16.0)).ceil() as usize).clamp(2, 64);
        if !self.down {
            self.pos = c + Vec2::from_angle(a0 + s) * r;
            self.line = vec![self.pos];
            return;
        }
        for k in 1..=n {
            let a = a0 + s * k as f64 / n as f64;
            self.to(c + Vec2::from_angle(a) * r);
        }
    }
    /// An arc to `d` away with bulge `b` (the tangent of a quarter of its sweep; 0 = straight).
    fn bulge(&mut self, d: Vec2, b: f64) {
        let (p0, p1) = (self.pos, self.pos + d);
        if b.abs() < 1e-9 || !self.down {
            self.go(d);
            return;
        }
        let sweep = 4.0 * b.atan();
        let chord = d.len();
        if chord < 1e-12 {
            return;
        }
        let r = chord / (2.0 * (sweep / 2.0).sin()).abs();
        let mid = (p0 + p1) * 0.5;
        let h = r * (sweep / 2.0).cos().abs();
        let n = d.perp() / chord;
        let c = mid + n * (if sweep.abs() < std::f64::consts::PI { h } else { -h }) * sweep.signum();
        let a0 = (p0 - c).angle();
        let steps = ((sweep.abs() / (std::f64::consts::PI / 16.0)).ceil() as usize).clamp(2, 64);
        for k in 1..steps {
            self.to(c + Vec2::from_angle(a0 + sweep * k as f64 / steps as f64) * r);
        }
        self.to(p1);
    }
}

/// Folders searched for .shx files named without a folder (a process-wide setting; by default
/// the folders in `CADADDY_FONT_PATH`, separated like `PATH`).
static SEARCH: RwLock<Option<Vec<String>>> = RwLock::new(None);
/// Fonts read so far by name (and names that weren't found, as `None`).
static CACHE: Mutex<Option<HashMap<String, Option<Arc<ShxFont>>>>> = Mutex::new(None);

pub fn set_search_path(dirs: Vec<String>) {
    if let Ok(mut s) = SEARCH.write() {
        *s = Some(dirs);
    }
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
}

/// The current drawing's folder: searched first, as AutoCAD does ([`set_drawing_dir`]).
static DRAWING_DIR: RwLock<Option<String>> = RwLock::new(None);

/// The folder of the drawing being shown; a font a style names is looked for there first. A
/// change forgets the fonts not found so far, so they are looked for again.
pub fn set_drawing_dir(dir: &str) {
    if let Ok(mut d) = DRAWING_DIR.write() {
        if d.as_deref() == Some(dir) {
            return;
        }
        *d = Some(dir.to_string());
    }
    if let Ok(mut c) = CACHE.lock()
        && let Some(m) = c.as_mut()
    {
        m.retain(|_, f| f.is_some());
    }
}

/// Where shape fonts are looked for: the drawing's folder, then the folders set by
/// [`set_search_path`] (else `CADADDY_FONT_PATH`), then the user's CADaddy fonts folder and the
/// system fonts folder (the Windows app set no path, so only full paths were found).
/// The folder of the drawing being shown, if any.
#[cfg(not(target_arch = "wasm32"))]
fn drawing_dir() -> Option<String> {
    DRAWING_DIR.read().ok().and_then(|d| d.clone())
}

fn search_path() -> Vec<String> {
    let mut dirs: Vec<String> = DRAWING_DIR.read().ok().and_then(|d| d.clone()).into_iter().collect();
    match SEARCH.read().ok().and_then(|s| s.clone()) {
        Some(v) => dirs.extend(v),
        None => dirs.extend(
            std::env::var("CADADDY_FONT_PATH")
                .map(|p| p.split([';', ':']).filter(|d| !d.trim().is_empty()).map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default(),
        ),
    }
    dirs.extend(default_font_dirs());
    dirs
}

/// The user's CADaddy fonts folder and the system fonts folder.
fn default_font_dirs() -> Vec<String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if cfg!(windows) {
        let mut v: Vec<String> = env("LOCALAPPDATA").map(|l| format!("{l}\\CADaddy\\Fonts")).into_iter().collect();
        v.extend(env("WINDIR").map(|w| format!("{w}\\Fonts")));
        v
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| vec![format!("{h}/Library/Fonts/CADaddy"), format!("{h}/Library/Fonts")]).unwrap_or_default()
    } else {
        env("HOME").map(|h| vec![format!("{h}/.local/share/fonts/cadaddy"), format!("{h}/.local/share/fonts")]).unwrap_or_default()
    }
}

/// Whether a style's font name names a shape font (`romans.shx`, `txt`).
pub fn is_shx_name(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n.ends_with(".shx") || (!n.is_empty() && !n.contains('.'))
}

/// The shape font a style names, read from its path or the search path; `None` when not found.
pub fn find(name: &str) -> Option<Arc<ShxFont>> {
    let n = name.trim();
    if n.is_empty() || !is_shx_name(n) {
        return None;
    }
    let key = n.to_ascii_lowercase();
    if let Ok(c) = CACHE.lock()
        && let Some(hit) = c.as_ref().and_then(|m| m.get(&key))
    {
        return hit.clone();
    }
    let found = load(n);
    if let Ok(mut c) = CACHE.lock() {
        let m = c.get_or_insert_with(HashMap::new);
        if m.len() > 256 {
            m.clear();
        }
        m.insert(key, found.clone());
    }
    found
}

#[cfg(not(target_arch = "wasm32"))]
fn load(name: &str) -> Option<Arc<ShxFont>> {
    let file = if name.to_ascii_lowercase().ends_with(".shx") { name.to_string() } else { format!("{name}.shx") };
    let file = cadaddy_geom::filename::native(&file);
    let p = std::path::Path::new(&file);
    // The path a style names is opened only when it is on this computer (or the drawing's share);
    // otherwise its file name is still looked for in the font folders.
    let drawing_dir = drawing_dir();
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if cadaddy_geom::filename::stored_path_allowed(name, drawing_dir.as_deref().map(std::path::Path::new)) {
        candidates.push(p.to_path_buf());
    }
    if let Some(base) = p.file_name() {
        candidates.extend(search_path().iter().map(|d| std::path::Path::new(d).join(base)));
    }
    for c in candidates {
        // Font names ignore case, as on Windows: `ROMANS` finds `romans.shx` on Linux too.
        let Some(c) = cadaddy_geom::filename::find(&c) else { continue };
        let Ok(meta) = std::fs::metadata(&c) else { continue };
        if !meta.is_file() || meta.len() > MAX_FILE {
            continue;
        }
        if let Some(f) = std::fs::read(&c).ok().and_then(|b| ShxFont::parse(&b)) {
            return Some(Arc::new(f));
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn load(_name: &str) -> Option<Arc<ShxFont>> {
    None
}

/// Text in a shape font (with an optional big font for characters it lacks), as strokes. The
/// font's `above` height is the text height; characters neither font has come from the
/// built-in stroke font.
pub(crate) fn shape(main: &ShxFont, big: Option<&ShxFont>, s: &str, height: f64, width_factor: f64, oblique: f64) -> crate::Shaped {
    let h = if height.is_finite() && height > 0.0 { height } else { 1.0 };
    let wf = if width_factor.is_finite() && width_factor.abs() > 1e-6 { width_factor } else { 1.0 };
    let shear = if oblique.is_finite() { oblique.tan() } else { 0.0 };
    let mut out = crate::Shaped::default();
    let mut x = 0.0;
    let mut spans = Vec::new();
    for (c, under, over) in crate::decode_controls(s) {
        let x0 = x;
        let hit = main.code_of(c).map(|k| (main, k)).or_else(|| big.and_then(|b| b.code_of(c).map(|k| (b, k))));
        match hit.and_then(|(f, k)| Some((f, f.glyph(k, false)?))) {
            Some((f, g)) => {
                let k = h / f.above.max(1e-9);
                let place = |p: Vec2| Vec2::new(x + (p.x + p.y * shear) * k * wf, p.y * k);
                out.strokes.extend(g.strokes.iter().map(|l| l.iter().map(|p| place(*p)).collect()));
                x += g.advance.x * k * wf;
            }
            None => {
                let mut sh = crate::shape_stroke(&c.to_string(), h, wf, oblique);
                sh.map(|p| Vec2::new(p.x + x, p.y));
                x += sh.width;
                out.extend(sh);
            }
        }
        spans.push((x0, x, under, over));
    }
    out.strokes.extend(crate::decorations(&spans, h));
    out.width = x;
    out
}

/// Character `c` as a shape font draws it in vertical text, when the font has a vertical form
/// for it (shape commands that run only in vertical mode take the pen from the top of the
/// character's cell, centred on the column, to the top of the next one below): its strokes
/// from that pen position, scaled to `height`, and the pen's move. `None` for a character
/// neither font has or one without a vertical form.
pub(crate) fn vertical_char(
    main: &ShxFont,
    big: Option<&ShxFont>,
    c: char,
    height: f64,
    width_factor: f64,
    oblique: f64,
) -> Option<(Vec<Vec<Vec2>>, Vec2)> {
    let (f, code) = main.code_of(c).map(|k| (main, k)).or_else(|| big.and_then(|b| b.code_of(c).map(|k| (b, k))))?;
    let g = f.glyph(code, true)?;
    if g.advance.y > -1e-9 {
        return None;
    }
    let h = if height.is_finite() && height > 0.0 { height } else { 1.0 };
    let wf = if width_factor.is_finite() && width_factor.abs() > 1e-6 { width_factor } else { 1.0 };
    let shear = if oblique.is_finite() { oblique.tan() } else { 0.0 };
    let k = h / f.above.max(1e-9);
    let place = |p: Vec2| Vec2::new((p.x + p.y * shear) * k * wf, p.y * k);
    Some((g.strokes.iter().map(|l| l.iter().map(|p| place(*p)).collect()).collect(), Vec2::new(g.advance.x * k * wf, g.advance.y * k)))
}

/// The width of a line in a shape font, as [`shape`] lays it out.
pub(crate) fn width(main: &ShxFont, big: Option<&ShxFont>, s: &str, height: f64, width_factor: f64) -> f64 {
    shape(main, big, s, height, width_factor, 0.0).width
}

/// A compiled shape font built from shape definitions (number, name, spec bytes), as the SHX
/// compiler lays them out: for tests and tools that make fonts of their own.
pub fn compile_shapes(above: u8, below: u8, shapes: &[(u16, &str, &[u8])]) -> Vec<u8> {
    let mut defs: Vec<(u16, Vec<u8>)> = vec![(0, [b"FONT".as_slice(), &[0, above, below, 0, 0]].concat())];
    for (n, name, spec) in shapes {
        defs.push((*n, [name.as_bytes(), &[0], spec].concat()));
    }
    defs.sort_by_key(|d| d.0);
    let mut out = b"AutoCAD-86 shapes 1.0\r\n\x1a".to_vec();
    let first = defs.first().map_or(0, |d| d.0);
    let last = defs.last().map_or(0, |d| d.0);
    for v in [first, last, u16::try_from(defs.len()).unwrap_or(u16::MAX)] {
        out.extend(v.to_le_bytes());
    }
    for (n, d) in &defs {
        out.extend(n.to_le_bytes());
        out.extend(u16::try_from(d.len()).unwrap_or(u16::MAX).to_le_bytes());
    }
    for (_, d) in &defs {
        out.extend(d);
    }
    out
}

/// A compiled big font (double-byte codes) built from shape definitions, as [`compile_shapes`].
pub fn compile_bigfont(above: u8, escapes: &[(u8, u8)], shapes: &[(u16, &str, &[u8])]) -> Vec<u8> {
    let mut defs: Vec<(u16, Vec<u8>)> = vec![(0, [b"BIG".as_slice(), &[0, above, 0, 0, 0]].concat())];
    for (n, name, spec) in shapes {
        defs.push((*n, [name.as_bytes(), &[0], spec].concat()));
    }
    let mut out = b"AutoCAD-86 bigfont 1.0\r\n\x1a".to_vec();
    let n = u16::try_from(defs.len()).unwrap_or(u16::MAX);
    let ranges = u16::try_from(escapes.len()).unwrap_or(0);
    out.extend(8u16.to_le_bytes());
    out.extend(n.to_le_bytes());
    out.extend(ranges.to_le_bytes());
    for (a, b) in escapes {
        out.extend(u16::from(*a).to_le_bytes());
        out.extend(u16::from(*b).to_le_bytes());
    }
    let index_at = out.len();
    let mut offset = index_at + 8 * defs.len();
    for (num, d) in &defs {
        out.extend(num.to_le_bytes());
        out.extend(u16::try_from(d.len()).unwrap_or(u16::MAX).to_le_bytes());
        out.extend(u32::try_from(offset).unwrap_or(u32::MAX).to_le_bytes());
        offset += d.len();
    }
    for (_, d) in &defs {
        out.extend(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "L": pen up to (0, 4), pen down, down 4 (0x4C: length 4, direction C = south), right 2,
    /// then pen up and on to the next origin at (3, 0).
    const L: &[u8] = &[2, 0x44, 1, 0x4c, 0x20, 2, 0x10, 0];
    /// A square drawn with 8/9 displacements, inside a push/pop.
    const SQ: &[u8] = &[1, 5, 9, 3, 0, 0, 3, 0xfd, 0, 0, 0xfd, 0, 0, 6, 2, 8, 4, 0, 0];

    fn font() -> ShxFont {
        // L at its letter, a square at 'Q', a circle (octant arc) at 'O', and 'X' drawing L as a subshape.
        let o: &[u8] = &[2, 8, 4, 0, 1, 10, 2, 0x00, 2, 8, 4, 0, 0];
        let x: &[u8] = &[7, b'L', 0];
        ShxFont::parse(&compile_shapes(
            4,
            1,
            &[(u16::from(b'L'), "L", L), (u16::from(b'Q'), "Q", SQ), (u16::from(b'O'), "O", o), (u16::from(b'X'), "X", x)],
        ))
        .unwrap()
    }

    #[test]
    fn vertical_forms_stack_down_the_column() {
        // V: a 2-wide stroke 4 tall. In vertical text it first moves from the cell's top centre
        // to its baseline-left corner and ends at the next cell's top centre (5 below).
        let v: &[u8] = &[2, 14, 8, 0xFF, 0xFC, 1, 0x44, 2, 8, 2, 0xFC, 14, 8, 0xFF, 0xFF, 0];
        let f = ShxFont::parse(&compile_shapes(4, 1, &[(u16::from(b'V'), "V", v), (u16::from(b'L'), "L", L)])).unwrap();
        assert_eq!(f.glyph(u32::from(b'V'), false).unwrap().advance, Vec2::new(2.0, 0.0));
        let (strokes, step) = vertical_char(&f, None, 'V', 4.0, 1.0, 0.0).unwrap();
        assert_eq!(strokes, vec![vec![Vec2::new(-1.0, -4.0), Vec2::new(-1.0, 0.0)]]);
        assert_eq!(step, Vec2::new(0.0, -5.0));
        // L has no vertical form: the caller centres it in a cell.
        assert!(vertical_char(&f, None, 'L', 4.0, 1.0, 0.0).is_none());
        // Down the column: the second V one cell (5) below the first; L centred under them.
        let font = crate::TextFont::Shx { main: std::sync::Arc::new(f), big: None };
        let sh = crate::shape_vertical(&font, "VVL", 4.0, 1.0, 0.0);
        assert_eq!(sh.strokes.get(1), Some(&vec![Vec2::new(-1.0, -9.0), Vec2::new(-1.0, -5.0)]));
        let l = sh.strokes.get(2).unwrap();
        let top = l.iter().map(|p| p.y).fold(f64::MIN, f64::max);
        let (x0, x1) = l.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
        assert!((top + 10.0).abs() < 1e-9, "{l:?}");
        assert!(x0 < 0.0 && x1 > 0.0 && (x0 + x1).abs() < 1.5, "{l:?}");
    }

    #[test]
    fn a_compiled_shape_font_reads_back() {
        let f = font();
        assert_eq!((f.kind, f.above, f.below), (Kind::Shapes, 4.0, 1.0));
        assert!(f.has(u32::from(b'L')) && !f.has(u32::from(b'Z')));
        let g = f.glyph(u32::from(b'L'), false).unwrap();
        assert_eq!(g.strokes, vec![vec![Vec2::new(0.0, 4.0), Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0)]]);
        assert_eq!(g.advance, Vec2::new(3.0, 0.0));
    }

    #[test]
    fn displacements_push_pop_arcs_and_subshapes() {
        let f = font();
        let q = f.glyph(u32::from(b'Q'), false).unwrap();
        // The square closes back on its corner; after the pop the pen is at the origin, then +4.
        let pts = q.strokes.first().unwrap();
        assert!(pts.first().unwrap().dist(*pts.last().unwrap()) < 1e-9, "{pts:?}");
        assert_eq!(q.advance, Vec2::new(4.0, 0.0));
        // A full circle of radius 2: all its points 2 from the centre (2, 0).
        let o = f.glyph(u32::from(b'O'), false).unwrap();
        let ring = o.strokes.first().unwrap();
        assert!(ring.len() > 8 && ring.iter().all(|p| (p.dist(Vec2::new(2.0, 0.0)) - 2.0).abs() < 1e-9), "{ring:?}");
        // X draws L.
        assert_eq!(f.glyph(u32::from(b'X'), false).unwrap().strokes, f.glyph(u32::from(b'L'), false).unwrap().strokes);
    }

    #[test]
    fn hostile_fonts_never_panic() {
        let good = compile_shapes(4, 1, &[(65, "A", L)]);
        for cut in 0..good.len() {
            let _ = ShxFont::parse(&good[..cut]);
        }
        // A shape that calls itself forever and one with a huge stack and arcs.
        let me: &[u8] = &[7, 66, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 11, 255, 255, 255, 255, 0x8f, 13, 127, 127, 127, 0];
        let f = ShxFont::parse(&compile_shapes(1, 0, &[(66, "B", me)])).unwrap();
        let g = f.glyph(66, false).unwrap();
        assert!(g.strokes.iter().map(Vec::len).sum::<usize>() <= MAX_POINTS + 64);
        assert!(ShxFont::parse(b"not a font").is_none());
    }

    #[test]
    fn big_fonts_draw_double_byte_characters() {
        // 日 in Shift-JIS is 0x93FA.
        let sq: &[u8] = &[1, 0x44, 0x40, 0x4c, 0x48, 2, 0x50, 0];
        let big = ShxFont::parse(&compile_bigfont(4, &[(0x81, 0x9f), (0xe0, 0xfc)], &[(0x93fa, "NICHI", sq)])).unwrap();
        assert_eq!(big.kind, Kind::Bigfont);
        assert_eq!(big.code_of('日'), Some(0x93fa));
        assert_eq!(big.code_of('月'), None);
        let main = font();
        let sh = shape(&main, Some(&big), "L日Z", 4.0, 1.0, 0.0);
        // L (3 wide), 日 (5), then Z from the stroke font.
        assert!(sh.width > 8.0, "{}", sh.width);
        assert!(sh.strokes.iter().flatten().any(|p| p.x > 3.0 && p.x < 8.0), "the big font glyph is drawn");
    }

    #[test]
    fn found_on_the_search_path_and_cached() {
        let dir = std::env::temp_dir().join(format!("cadaddy-shx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.shx"), compile_shapes(4, 1, &[(65, "A", L)])).unwrap();
        set_search_path(vec![dir.to_string_lossy().to_string()]);
        assert!(find("mine.shx").is_some() && find("MINE").is_some());
        assert!(find("absent.shx").is_none());
        assert!(find("arial.ttf").is_none(), "not a shape font name");
        // A style naming it draws with it; one naming a missing .shx keeps the stroke font.
        let tf = crate::TextFont::resolve("mine.shx");
        assert!(matches!(tf, crate::TextFont::Shx { .. }) && !tf.is_outline());
        let sh = crate::shape_line(&tf, "AA", 8.0, 1.0, 0.0);
        // Two "L"-shaped A's at twice the font's size (text height 8, above 4): 6 units apart.
        assert_eq!(sh.width, 12.0);
        assert_eq!(sh.strokes.first().cloned(), Some(vec![Vec2::new(0.0, 8.0), Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)]));
        assert_eq!(crate::text_width(&tf, "AA", 8.0, 1.0), 12.0);
        assert!(matches!(crate::TextFont::resolve("absent.shx"), crate::TextFont::Stroke | crate::TextFont::Outline(_)));
        std::fs::remove_dir_all(&dir).ok();
    }
}
