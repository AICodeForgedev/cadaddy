//! File names stored in drawings or typed by users, resolved the way Windows resolves them:
//! letter case doesn't matter and `\` separates folders, on every OS. (Here because this is the
//! lowest crate everything that opens files shares; it only uses `std`.)

use std::path::{Component, Path, PathBuf};

/// Entries read from one folder while looking for a name (a folder, not a whole disk).
const MAX_DIR_ENTRIES: usize = 20_000;
/// Folders deep a name may go.
const MAX_DEPTH: usize = 64;

/// `name` with `\` read as a folder separator (on Windows it already is).
pub fn native(name: &str) -> String {
    if cfg!(windows) { name.to_string() } else { name.replace('\\', "/") }
}

/// Two file names that are the same to Windows (ASCII and Unicode letter case alike).
pub fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || (!a.is_ascii() && a.to_lowercase() == b.to_lowercase())
}

/// The entry of `dir` named `name` ignoring case: the exact name when it exists.
fn child(dir: &Path, name: &std::ffi::OsStr) -> Option<PathBuf> {
    let exact = dir.join(name);
    if std::fs::symlink_metadata(&exact).is_ok() {
        return Some(exact);
    }
    let want = name.to_str()?;
    let scan = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    std::fs::read_dir(scan)
        .ok()?
        .flatten()
        .take(MAX_DIR_ENTRIES)
        .find(|e| e.file_name().to_str().is_some_and(|n| same_name(n, want)))
        .map(|e| dir.join(e.file_name()))
}

/// The existing file or folder `path` names, ignoring letter case in every part of it (and
/// with `\` as a separator): `path` itself when it exists; `None` when nothing matches.
pub fn find(path: impl AsRef<Path>) -> Option<PathBuf> {
    let path = path.as_ref();
    if path.exists() {
        return Some(path.to_path_buf());
    }
    let text = path.to_str()?;
    let norm = native(text);
    let path = Path::new(&norm);
    if path.exists() {
        return Some(path.to_path_buf());
    }
    let mut out = PathBuf::new();
    for (i, c) in path.components().enumerate() {
        if i > MAX_DEPTH {
            return None;
        }
        match c {
            Component::Normal(n) => out = child(&out, n)?,
            other => out.push(other.as_os_str()),
        }
    }
    out.exists().then_some(out)
}

/// [`find`], or `path` unchanged (so the caller's error names what was asked for).
pub fn resolve(path: impl AsRef<Path>) -> PathBuf {
    let p = path.as_ref();
    find(p).unwrap_or_else(|| p.to_path_buf())
}

/// The regular file `path` names (see [`find`]).
pub fn find_file(path: impl AsRef<Path>) -> Option<PathBuf> {
    find(path).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cadaddy-filename-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("xref/sub")).unwrap();
        std::fs::write(d.join("xref/base.dwg"), b"x").unwrap();
        std::fs::write(d.join("xref/sub/ÄNDERUNG.dxf"), b"y").unwrap();
        d
    }

    #[test]
    fn mixed_case_and_backslash_names_find_lower_case_files() {
        let d = dir("find");
        let host = d.to_str().unwrap();
        // Compared by content: a case-insensitive disk (macOS, Windows) hands back the name as asked.
        let read = |p: Option<PathBuf>| p.and_then(|p| std::fs::read(p).ok());
        assert_eq!(read(find_file(format!("{host}/XREF/Base.DWG"))), Some(b"x".to_vec()));
        assert_eq!(read(find_file(format!("{host}\\Xref\\BASE.dwg"))), Some(b"x".to_vec()));
        assert_eq!(read(find_file(d.join("xref").join("..").join("XREF/base.DWG"))), Some(b"x".to_vec()));
        assert_eq!(read(find_file(format!("{host}/xref/SUB/änderung.DXF"))), Some(b"y".to_vec()));
        assert!(find(format!("{host}/xref/missing.dwg")).is_none());
        assert!(find_file(format!("{host}/XREF")).is_none(), "a folder is not a file");
        assert_eq!(resolve(format!("{host}/nope/x.dwg")), PathBuf::from(format!("{host}/nope/x.dwg")));
        // Relative to the current folder.
        assert!(find("CARGO.TOML").is_some() || find("Cargo.toml").is_some());
        assert!(same_name("Ölwanne", "ÖLWANNE") && !same_name("a", "b"));
        std::fs::remove_dir_all(&d).ok();
    }
}
