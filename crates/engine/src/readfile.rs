//! Reading a whole input file with guards: only regular files, and only up to a size limit, so
//! that a device or pipe (`/dev/zero`) or a runaway file can't exhaust memory.

use std::io::Read;

/// The largest file read whole (drawings, scripts, templates, xrefs): 2 GiB.
pub const MAX_FILE_BYTES: u64 = 2 << 30;

/// Read `path` into memory: an error for anything but a regular file, or a file over
/// [`MAX_FILE_BYTES`]. The name is matched ignoring case, `\` separating folders (as Windows
/// does), so a drawing made on Windows finds its files anywhere.
pub fn read(path: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
    read_max(path.as_ref(), MAX_FILE_BYTES)
}

/// [`read`] as UTF-8 text.
pub fn read_string(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Open `path` for reading, its name matched as [`read`] matches it.
pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<std::fs::File> {
    std::fs::File::open(cadaddy_geom::filename::resolve(path))
}

fn read_max(path: &std::path::Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind};
    let not_a_file = || Error::new(ErrorKind::InvalidInput, "not a regular file");
    let resolved = cadaddy_geom::filename::resolve(path);
    // Windows refuses to open a folder ("access denied"): report it as not a regular file, as
    // the other platforms do.
    let file = std::fs::File::open(&resolved).map_err(|e| if std::fs::metadata(&resolved).is_ok_and(|m| !m.is_file()) { not_a_file() } else { e })?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(not_a_file());
    }
    let too_big = || Error::new(ErrorKind::InvalidData, format!("larger than {} MB", max >> 20));
    if meta.len() > max {
        return Err(too_big());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    // The file may grow while it is read.
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(too_big());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_files_only_and_within_the_limit() {
        let dir = std::env::temp_dir().join(format!("cadaddy-readfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.dxf");
        std::fs::write(&f, b"0\nEOF\n").unwrap();
        assert_eq!(read(&f).unwrap(), b"0\nEOF\n");
        assert_eq!(read_max(&f, 3).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(read(&dir).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        #[cfg(unix)]
        assert_eq!(read("/dev/zero").unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        std::fs::remove_dir_all(&dir).ok();
    }
}
