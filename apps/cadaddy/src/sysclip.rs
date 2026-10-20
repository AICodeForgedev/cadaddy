//! The system clipboard with its formats, for the interface ([`SystemClipboard`]).
//!
//! Windows: a copy goes on the clipboard as CADaddy objects (DXF under the "CADaddy Drawing"
//! format) and as a picture ("PNG" and `CF_DIB`), so it pastes into other CADaddy windows,
//! documents, email and image editors; pictures and text other programs copy are read back.
//! Elsewhere the clipboard holds one thing at a time here: a copy goes on it as DXF text, and
//! pictures and text other programs copy are read.

use cadaddy_ui_egui::clipboard::{Format, SystemClipboard};

pub struct HostClipboard;

#[cfg(target_os = "windows")]
impl HostClipboard {
    fn id(format: Format) -> Option<u32> {
        use clipboard_win::{formats, register_format};
        match format {
            Format::Drawing => register_format(cadaddy_ui_egui::clipboard::DRAWING_FORMAT).map(std::num::NonZeroU32::get),
            Format::Png => register_format("PNG").map(std::num::NonZeroU32::get),
            Format::Dib => Some(formats::CF_DIB),
            Format::Text => Some(formats::CF_UNICODETEXT),
        }
    }
}

#[cfg(target_os = "windows")]
impl SystemClipboard for HostClipboard {
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool {
        use clipboard_win::{options, raw};
        let Ok(_open) = clipboard_win::Clipboard::new_attempts(10) else { return false };
        if raw::empty().is_err() {
            return false;
        }
        let mut any = false;
        for (format, bytes) in formats {
            let ok = match format {
                Format::Text => std::str::from_utf8(bytes).is_ok_and(|t| raw::set_string_with(t, options::NoClear).is_ok()),
                f => Self::id(*f).is_some_and(|id| raw::set_without_clear(id, bytes).is_ok()),
            };
            if !ok {
                log::warn!("clipboard: couldn't put {format:?} on the clipboard");
            }
            any |= ok;
        }
        any
    }
    fn get(&mut self, format: Format) -> Option<Vec<u8>> {
        use clipboard_win::raw;
        let id = Self::id(format)?;
        let _open = clipboard_win::Clipboard::new_attempts(10).ok()?;
        if !raw::is_format_avail(id) {
            return None;
        }
        let mut out = Vec::new();
        let read = if format == Format::Text { raw::get_string(&mut out) } else { raw::get_vec(id, &mut out) };
        read.ok().filter(|n| *n > 0).map(|_| out)
    }
}

#[cfg(not(target_os = "windows"))]
impl SystemClipboard for HostClipboard {
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool {
        // One thing at a time: the objects, as DXF text.
        let Some(text) = formats.iter().find(|(f, _)| *f == Format::Drawing).and_then(|(_, b)| std::str::from_utf8(b).ok()) else {
            return false;
        };
        arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).is_ok()
    }
    fn get(&mut self, format: Format) -> Option<Vec<u8>> {
        let mut c = arboard::Clipboard::new().ok()?;
        match format {
            Format::Text => c.get_text().ok().map(String::into_bytes),
            Format::Png => {
                let img = c.get_image().ok()?;
                cadaddy_ui_egui::clipboard::png_from_rgba(u32::try_from(img.width).ok()?, u32::try_from(img.height).ok()?, img.bytes.into_owned())
            }
            Format::Drawing | Format::Dib => None,
        }
    }
}
