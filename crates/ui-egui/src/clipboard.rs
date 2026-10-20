//! The system clipboard: Cut/Copy/Paste key presses, each copy put on the system clipboard (the
//! objects for other CADaddy windows, and their picture for documents, email and image editors),
//! and what other programs copied (a picture, text) read before a paste.
//!
//! egui turns Cmd/Ctrl+C, X and V into `Copy`, `Cut` and `Paste(text)` events instead of key
//! presses, and sends no `Paste` when the system clipboard holds no text (a picture, CADaddy
//! objects in their own format): the V key's release without a `Paste` is that paste.
//!
//! The host gives the system clipboard as a [`SystemClipboard`] ([`crate::Services`]); without
//! one, a copy goes on it as DXF text through egui and only pasted text is read.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use cadaddy_engine::cmd::clipboard;
use cadaddy_render::images::Raster;

use crate::CadApp;

/// What was last put on the system clipboard.
#[derive(Debug, Default)]
pub struct Mirror {
    /// The copy (`ClipSource::serial`) last mirrored.
    serial: Option<u64>,
    /// The hash of the DXF text it became, so reading it back isn't taken for another copy.
    text: Option<u64>,
    /// A `Paste` event came since the V key was last released.
    paste_seen: bool,
}

/// Commands that paste: the system clipboard is read before they run.
pub const PASTE_COMMANDS: [&str; 5] = ["pasteclip", "pastespec", "pasteblock", "pasteorig", "pa"];

/// Line endings normalised (pasted text arrives with `\n`), so our own text is recognised.
fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.replace("\r\n", "\n").hash(&mut h);
    h.finish()
}

/// Objects at most for which a copy is also drawn as a picture.
const MAX_PICTURE_OBJECTS: usize = 20_000;

/// Put a new copy (COPYCLIP, CUTCLIP, COPYBASE from anywhere) on the system clipboard, once: the
/// objects as DXF and their picture, or (without a host clipboard) the DXF as text.
pub fn mirror(app: &mut CadApp, ctx: &egui::Context) {
    let Some(src) = &app.session.clipboard_source else { return };
    if src.uid.is_none() || app.clip_mirror.serial == Some(src.serial) {
        return;
    }
    app.clip_mirror.serial = Some(src.serial);
    let text = clipboard::system_text(&app.session);
    app.clip_mirror.text = text.as_deref().map(hash);
    if let Some(sys) = app.services.system_clipboard.as_mut() {
        let picture =
            (app.session.clipboard.len() <= MAX_PICTURE_OBJECTS).then(|| clipboard::system_image(&app.session, clipboard::IMAGE_SIDE)).flatten();
        let formats = encode(text.as_deref(), picture.as_ref());
        if !formats.is_empty() && sys.set(&formats) {
            return;
        }
    }
    if let Some(text) = text {
        ctx.copy_text(text);
    }
}

/// Take what the system clipboard holds into the session: another CADaddy's objects become the
/// clipboard; a picture or text copied in another program is newer than CADaddy's own copy,
/// which it replaces (PASTECLIP then pastes it as a raster image or multiline text). Nothing
/// readable leaves the clipboard as it was.
pub fn take(app: &mut CadApp, c: Contents) {
    if let Some(d) = c.drawing {
        let h = hash(&d);
        if app.clip_mirror.text != Some(h) && clipboard::load_system_text(&mut app.session, &d) {
            app.clip_mirror.text = Some(h);
        }
        app.session.system_clipboard = clipboard::SystemContents::default();
        return;
    }
    if c.image.is_none() && c.text.is_none() {
        return;
    }
    app.session.clipboard.clear();
    app.session.system_clipboard = clipboard::SystemContents { image: c.image.map(Arc::new), text: c.text };
}

/// Before a paste command: read the system clipboard (when the host offers one).
pub fn before_paste(app: &mut CadApp, id: &str) {
    if !PASTE_COMMANDS.contains(&id.to_ascii_lowercase().as_str()) {
        return;
    }
    if let Some(sys) = app.services.system_clipboard.as_mut() {
        let c = decode(sys.as_mut());
        take(app, c);
    }
}

/// The commands the Cut/Copy/Paste events of this frame ask for. Without a host clipboard, pasted
/// text is read here: DXF that isn't our own last copy (another CADaddy's) becomes the clipboard,
/// other text pastes as multiline text.
pub fn shortcut_commands(app: &mut CadApp, ctx: &egui::Context) -> Vec<&'static str> {
    let (events, shift) = ctx.input(|i| {
        let ev: Vec<egui::Event> = i
            .events
            .iter()
            .filter(|e| {
                matches!(e, egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_))
                    || matches!(e, egui::Event::Key { key: egui::Key::V, pressed: false, .. })
            })
            .cloned()
            .collect();
        (ev, i.modifiers.shift)
    });
    let paste = if shift { "pasteblock" } else { "pasteclip" };
    let mut out = Vec::new();
    for e in events {
        match e {
            egui::Event::Copy => out.push(if shift { "copybase" } else { "copyclip" }),
            egui::Event::Cut => out.push("cutclip"),
            egui::Event::Paste(text) => {
                app.clip_mirror.paste_seen = true;
                if app.services.system_clipboard.is_none() {
                    let c = if is_dxf(&text) {
                        Contents { drawing: Some(text), ..Contents::default() }
                    } else {
                        Contents { text: Some(text), ..Contents::default() }
                    };
                    take(app, c);
                }
                out.push(paste);
            }
            // Cmd/Ctrl+V with no text on the clipboard: egui sent no Paste, only the release.
            egui::Event::Key { modifiers, .. } => {
                let seen = std::mem::take(&mut app.clip_mirror.paste_seen);
                if modifiers.command && !seen && app.services.system_clipboard.is_some() && !out.contains(&paste) {
                    out.push(paste);
                }
            }
            _ => {}
        }
    }
    out
}

/// A format CADaddy writes to or reads from the system clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The copied objects as DXF text, under [`DRAWING_FORMAT`]: other CADaddy windows paste them.
    Drawing,
    /// A picture as a PNG file (documents, email, image editors).
    Png,
    /// A picture as a device-independent bitmap (Windows' `CF_DIB`: older programs).
    Dib,
    /// Plain text, UTF-8.
    Text,
}

/// The name CADaddy registers for [`Format::Drawing`].
pub const DRAWING_FORMAT: &str = "CADaddy Drawing";

/// The system clipboard as the host offers it: formats put on it together, and read one at a time.
pub trait SystemClipboard {
    /// Replace the clipboard's contents with `formats` (best first). `false` when the host can't
    /// put these there (the front end then puts the drawing there as text).
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool;
    /// The bytes of `format` on the clipboard, if it holds that format.
    fn get(&mut self, format: Format) -> Option<Vec<u8>>;
}

/// A clipboard in memory: the test double for the platform's, and its stand-in where there is none.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    pub formats: Vec<(Format, Vec<u8>)>,
}

impl SystemClipboard for MemoryClipboard {
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool {
        self.formats = formats.to_vec();
        true
    }
    fn get(&mut self, format: Format) -> Option<Vec<u8>> {
        self.formats.iter().find(|(f, _)| *f == format).map(|(_, b)| b.clone())
    }
}

/// What one copy puts on the system clipboard, best first: the objects as DXF (CADaddy windows
/// paste them), then their picture as PNG and as a bitmap (documents, email, image editors).
pub fn encode(drawing: Option<&str>, image: Option<&Raster>) -> Vec<(Format, Vec<u8>)> {
    let mut out = Vec::new();
    if let Some(d) = drawing {
        out.push((Format::Drawing, d.as_bytes().to_vec()));
    }
    if let Some(img) = image {
        if let Some(png) = img.encode_png() {
            out.push((Format::Png, png));
        }
        if let Some(dib) = dib_encode(img) {
            out.push((Format::Dib, dib));
        }
    }
    out
}

/// What the system clipboard holds, read the way CADaddy pastes it.
#[derive(Debug, Default, PartialEq)]
pub struct Contents {
    /// CADaddy objects as DXF text.
    pub drawing: Option<String>,
    /// A picture copied in another program.
    pub image: Option<Raster>,
    /// Text copied in another program.
    pub text: Option<String>,
}

/// Read the system clipboard: CADaddy objects first, then a picture (PNG, then a bitmap), then
/// text (DXF text from another CADaddy counts as objects).
pub fn decode(clip: &mut dyn SystemClipboard) -> Contents {
    if let Some(d) = clip.get(Format::Drawing).and_then(|b| String::from_utf8(b).ok()) {
        return Contents { drawing: Some(d), ..Contents::default() };
    }
    let image = clip.get(Format::Png).and_then(|b| cadaddy_render::images::decode(&b)).or_else(|| clip.get(Format::Dib).and_then(|b| dib_decode(&b)));
    if image.is_some() {
        return Contents { image, ..Contents::default() };
    }
    match clip.get(Format::Text).and_then(|b| String::from_utf8(b).ok()) {
        Some(t) if is_dxf(&t) => Contents { drawing: Some(t), ..Contents::default() },
        Some(t) if !t.trim().is_empty() => Contents { text: Some(t), ..Contents::default() },
        _ => Contents::default(),
    }
}

fn is_dxf(text: &str) -> bool {
    let mut lines = text.lines().map(str::trim);
    lines.next() == Some("0") && lines.next() == Some("SECTION")
}

/// RGBA pixels (as a host's clipboard gives a picture) as a PNG file.
pub fn png_from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    Raster::new(width, height, rgba)?.encode_png()
}

/// Largest bitmap read from the clipboard, in pixels.
const MAX_DIB_PIXELS: u64 = 64 << 20;

/// `img` as a Windows device-independent bitmap: a 40-byte header, then 32-bit BGRA rows from the
/// bottom up.
pub fn dib_encode(img: &Raster) -> Option<Vec<u8>> {
    let (w, h) = (i32::try_from(img.width).ok()?, i32::try_from(img.height).ok()?);
    let size = u32::try_from(img.rgba.len()).ok()?;
    let mut out = Vec::with_capacity(40 + img.rgba.len());
    out.extend(40u32.to_le_bytes());
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // BI_RGB
    out.extend(size.to_le_bytes());
    out.extend(2835i32.to_le_bytes()); // 72 dpi
    out.extend(2835i32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    let row = img.width as usize * 4;
    for r in img.rgba.chunks(row.max(1)).rev() {
        for p in r.as_chunks::<4>().0 {
            out.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    Some(out)
}

/// A device-independent bitmap from the clipboard (24 or 32 bits a pixel, uncompressed or with
/// the standard bit masks; rows bottom-up or top-down) as RGBA. A 32-bit bitmap whose alpha is
/// all zero (most programs leave it unset) is opaque.
pub fn dib_decode(b: &[u8]) -> Option<Raster> {
    let u32_at = |i: usize| b.get(i..i + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
    let i32_at = |i: usize| b.get(i..i + 4).and_then(|s| s.try_into().ok()).map(i32::from_le_bytes);
    let u16_at = |i: usize| b.get(i..i + 2).and_then(|s| s.try_into().ok()).map(u16::from_le_bytes);
    let header = u32_at(0)? as usize;
    if header < 40 {
        return None;
    }
    let (w, h) = (i32_at(4)?, i32_at(8)?);
    let bits = u16_at(14)?;
    let compression = u32_at(16)?;
    let colors = u32_at(32)?;
    if w <= 0 || h == 0 || !matches!(bits, 24 | 32) || !matches!(compression, 0 | 3) || colors != 0 {
        return None;
    }
    let (width, height) = (w.unsigned_abs(), h.unsigned_abs());
    if u64::from(width) * u64::from(height) > MAX_DIB_PIXELS {
        return None;
    }
    // BI_BITFIELDS with a plain 40-byte header: three masks follow it.
    let start = header + if compression == 3 && header == 40 { 12 } else { 0 };
    let bpp = usize::from(bits / 8);
    let stride = (width as usize * bpp).checked_add(3)? & !3;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        // Positive height: the first row stored is the bottom one.
        let src = if h > 0 { height as usize - 1 - y } else { y };
        let row = b.get(start + src * stride..start + src * stride + width as usize * bpp)?;
        for p in row.chunks(bpp) {
            let a = if bpp == 4 { *p.get(3)? } else { 255 };
            rgba.extend([*p.get(2)?, *p.get(1)?, *p.first()?, a]);
        }
    }
    if bpp == 4 && rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        for p in rgba.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
    }
    Raster::new(width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 × 2 picture with every pixel different.
    fn picture() -> Raster {
        let rgba = (0..6u8).flat_map(|i| [i * 40, 255 - i * 40, i * 7, 255]).collect();
        Raster::new(3, 2, rgba).unwrap()
    }

    #[test]
    fn a_copy_is_written_as_objects_png_and_bitmap_best_first() {
        let mut mem = MemoryClipboard::default();
        assert!(mem.set(&encode(Some("0\nSECTION\n0\nEOF\n"), Some(&picture()))));
        let order: Vec<Format> = mem.formats.iter().map(|(f, _)| *f).collect();
        assert_eq!(order, [Format::Drawing, Format::Png, Format::Dib]);
        assert_eq!(cadaddy_render::images::decode(&mem.get(Format::Png).unwrap()).unwrap(), picture());
        assert_eq!(dib_decode(&mem.get(Format::Dib).unwrap()).unwrap(), picture());
        // Read back: the objects win over their own picture.
        let c = decode(&mut mem);
        assert!(c.drawing.is_some() && c.image.is_none());
    }

    #[test]
    fn pictures_and_text_from_other_programs_are_read() {
        // An image editor's copy: a bitmap only, or a PNG.
        let mut mem = MemoryClipboard { formats: vec![(Format::Dib, dib_encode(&picture()).unwrap())] };
        assert_eq!(decode(&mut mem).image, Some(picture()));
        let mut mem = MemoryClipboard { formats: vec![(Format::Png, picture().encode_png().unwrap()), (Format::Text, b"alt".to_vec())] };
        assert_eq!(decode(&mut mem), Contents { image: Some(picture()), ..Contents::default() });
        // Text; DXF text (an older CADaddy's copy) is objects.
        let mut mem = MemoryClipboard { formats: vec![(Format::Text, b"PANEL A".to_vec())] };
        assert_eq!(decode(&mut mem).text.as_deref(), Some("PANEL A"));
        let mut mem = MemoryClipboard { formats: vec![(Format::Text, b"0\nSECTION\n".to_vec())] };
        assert!(decode(&mut mem).drawing.is_some());
        assert_eq!(decode(&mut MemoryClipboard::default()), Contents::default());
    }

    #[test]
    fn bitmaps_of_every_common_layout_decode() {
        let pic = picture();
        // 24 bits, bottom-up, rows padded to 4 bytes (3 × 3 = 9 → 12).
        let mut b24 = Vec::new();
        for v in [40u32, 3, 2] {
            b24.extend(v.to_le_bytes());
        }
        b24.extend(1u16.to_le_bytes());
        b24.extend(24u16.to_le_bytes());
        b24.extend([0u8; 24]);
        for y in (0..2).rev() {
            for x in 0..3 {
                let p = pic.pixel(x, y).unwrap();
                b24.extend([p[2], p[1], p[0]]);
            }
            b24.extend([0, 0, 0]);
        }
        assert_eq!(dib_decode(&b24), Some(picture()));
        // 32 bits top-down (negative height), alpha left at zero: opaque.
        let b32 = dib_encode(&pic).unwrap();
        let mut flipped: Vec<u8> = b32[..40].to_vec();
        flipped[8..12].copy_from_slice(&(-2i32).to_le_bytes());
        for r in b32[40..].chunks(12).rev() {
            flipped.extend(r.chunks(4).flat_map(|p| [p[0], p[1], p[2], 0]));
        }
        assert_eq!(dib_decode(&flipped), Some(pic));
        // Hostile: cut short, absurd sizes, palettes and compression are refused, never panic.
        assert!(dib_decode(&b32[..50]).is_none());
        let mut huge = dib_encode(&picture()).unwrap();
        huge[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(dib_decode(&huge).is_none());
        for bad in [&[0u8; 3][..], &[40, 0, 0, 0][..], &[]] {
            assert!(dib_decode(bad).is_none());
        }
        let mut rle = dib_encode(&picture()).unwrap();
        rle[16] = 1;
        assert!(dib_decode(&rle).is_none());
    }
}
