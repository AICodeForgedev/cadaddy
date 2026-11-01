//! Windows: an enhanced metafile on the open clipboard as `CF_ENHMETAFILE`. The system takes the
//! metafile handle, so the bytes become one first. Other platforms: nothing to do.

/// Put `bytes` (an enhanced metafile) on the open clipboard. The clipboard must be open and ours.
/// `false` when the bytes are not a metafile or the system refuses it.
#[cfg(target_os = "windows")]
pub fn put_emf(bytes: &[u8]) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Graphics::Gdi::{DeleteEnhMetaFile, SetEnhMetaFileBits};
    use windows::Win32::System::DataExchange::SetClipboardData;
    /// `CF_ENHMETAFILE`.
    const CF_ENHMETAFILE: u32 = 14;
    // SAFETY: the clipboard is open and ours; the handle is either handed to the system (which
    // then owns it) or deleted here.
    unsafe {
        let meta = SetEnhMetaFileBits(bytes);
        if meta.is_invalid() {
            return false;
        }
        if SetClipboardData(CF_ENHMETAFILE, Some(HANDLE(meta.0))).is_ok() {
            true
        } else {
            let _ = DeleteEnhMetaFile(Some(meta));
            false
        }
    }
}
