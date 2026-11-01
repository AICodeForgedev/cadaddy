//! The system clipboard with its formats, for the interface ([`SystemClipboard`]).
//!
//! Windows: a copy goes on the clipboard as CADaddy objects (DXF under the "CADaddy Drawing"
//! format) and as a picture ("PNG" and `CF_DIB`), so it pastes into other CADaddy windows,
//! documents, email and image editors; pictures and text other programs copy are read back.
//! Windows also puts the selection's vector picture on it as `CF_ENHMETAFILE`, before the PNG.
//! macOS: a copy goes on the pasteboard as the objects (DXF, under CADaddy's own type and
//! as plain text) and a PNG picture together; pictures and text other programs copy are read.
//! Linux: the clipboard library holds one thing at a time, so a copy goes on it as DXF text only
//! (no picture beside it); pictures and text other programs copy are read.

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
            // Not bytes in memory: a metafile handle (see `cadaddy_winclip::put_emf`).
            Format::Emf => None,
            Format::Text => Some(formats::CF_UNICODETEXT),
        }
    }
}

#[cfg(target_os = "windows")]
impl SystemClipboard for HostClipboard {
    fn takes_emf(&self) -> bool {
        true
    }
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
                Format::Emf => cadaddy_winclip::put_emf(bytes),
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

/// The pasteboard types a macOS copy carries.
#[cfg(target_os = "macos")]
mod mac {
    use cadaddy_ui_egui::clipboard::Format;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSData, NSString};

    /// The pasteboard type for [`Format::Drawing`] (a reverse-DNS name, as pasteboard types are).
    const DRAWING_TYPE: &str = "dev.aicodeforge.cadaddy.drawing";
    /// The system's plain-text and PNG pasteboard types, by their documented names (the
    /// `NSPasteboardType…` statics need `unsafe`, which the workspace forbids).
    const TEXT_TYPE: &str = "public.utf8-plain-text";
    const PNG_TYPE: &str = "public.png";

    /// Replace the general pasteboard's contents with the objects (their own type and as DXF text)
    /// and the PNG picture, all at once. `false` when nothing could be put there.
    pub fn set(formats: &[(Format, Vec<u8>)]) -> bool {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let mut any = false;
        for (format, bytes) in formats {
            match format {
                Format::Drawing => {
                    any |= pb.setData_forType(Some(&NSData::with_bytes(bytes)), &NSString::from_str(DRAWING_TYPE));
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        any |= pb.setString_forType(&NSString::from_str(text), &NSString::from_str(TEXT_TYPE));
                    }
                }
                Format::Png => any |= pb.setData_forType(Some(&NSData::with_bytes(bytes)), &NSString::from_str(PNG_TYPE)),
                // No metafile or device-independent bitmap on a Mac.
                Format::Emf | Format::Dib | Format::Text => {}
            }
        }
        any
    }

    /// The bytes the pasteboard holds for `format`, if any.
    pub fn get(format: Format) -> Option<Vec<u8>> {
        let pb = NSPasteboard::generalPasteboard();
        let data = match format {
            Format::Drawing => pb.dataForType(&NSString::from_str(DRAWING_TYPE)),
            Format::Png => pb.dataForType(&NSString::from_str(PNG_TYPE)),
            Format::Emf | Format::Dib | Format::Text => None,
        }?;
        Some(data.to_vec()).filter(|b| !b.is_empty())
    }
}

#[cfg(target_os = "macos")]
impl SystemClipboard for HostClipboard {
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool {
        mac::set(formats)
    }
    fn get(&mut self, format: Format) -> Option<Vec<u8>> {
        match format {
            Format::Drawing => mac::get(Format::Drawing),
            // Our own PNG first; screenshots and other programs' pictures come through arboard.
            Format::Png => mac::get(Format::Png).or_else(arboard_png),
            Format::Text => arboard::Clipboard::new().ok()?.get_text().ok().map(String::into_bytes),
            Format::Emf | Format::Dib => None,
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn arboard_png() -> Option<Vec<u8>> {
    let img = arboard::Clipboard::new().ok()?.get_image().ok()?;
    cadaddy_ui_egui::clipboard::png_from_rgba(u32::try_from(img.width).ok()?, u32::try_from(img.height).ok()?, img.bytes.into_owned())
}

/// Linux and the rest: the clipboard library holds one thing at a time, so the objects go on as
/// DXF text and the picture is left off.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl SystemClipboard for HostClipboard {
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool {
        let Some(text) = formats.iter().find(|(f, _)| *f == Format::Drawing).and_then(|(_, b)| std::str::from_utf8(b).ok()) else {
            return false;
        };
        arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).is_ok()
    }
    fn get(&mut self, format: Format) -> Option<Vec<u8>> {
        match format {
            Format::Text => arboard::Clipboard::new().ok()?.get_text().ok().map(String::into_bytes),
            Format::Png => arboard_png(),
            Format::Drawing | Format::Emf | Format::Dib => None,
        }
    }
}

/// The macOS host adapter round trip. It touches the real pasteboard, so it runs on request:
/// `cargo test -p cadaddy macos_host_clipboard -- --ignored` on a Mac.
#[cfg(all(test, target_os = "macos"))]
mod mac_tests {
    use super::*;

    #[test]
    #[ignore = "writes the real pasteboard"]
    fn macos_host_clipboard_round_trips_drawing_and_png_together() {
        let png = cadaddy_ui_egui::clipboard::png_from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let dxf = b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n".to_vec();
        let mut host = HostClipboard;
        assert!(host.set(&[(Format::Drawing, dxf.clone()), (Format::Png, png.clone())]));
        assert_eq!(host.get(Format::Drawing), Some(dxf.clone()), "the objects survive");
        assert_eq!(host.get(Format::Png), Some(png), "the picture is there beside them");
        assert_eq!(host.get(Format::Text), Some(dxf), "and the DXF as plain text for other programs");
    }
}
