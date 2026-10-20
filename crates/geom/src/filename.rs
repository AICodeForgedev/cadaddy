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

/// Whether `name` (a file name inside an archive or package) stays inside the folder it is
/// unpacked into: relative, no `..` part, no drive (`C:`) or root, `/` or `\` as separators, no
/// control characters, not empty.
pub fn is_safe_relative(name: &str) -> bool {
    if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
        return false;
    }
    if name.starts_with(['/', '\\']) || name.chars().nth(1) == Some(':') {
        return false;
    }
    name.split(['/', '\\']).all(|part| part != ".." && !part.contains(':')) && name.split(['/', '\\']).any(|part| !part.is_empty() && part != ".")
}

/// The system folder `path` is in, if any: where nothing CADaddy writes belongs (Windows,
/// Program Files, ProgramData; /System, /Library, /usr, /bin, /sbin, /etc and the other Unix
/// system folders). Compared without regard to case or separator.
pub fn system_folder(path: &Path) -> Option<&'static str> {
    const UNIX: [&str; 13] =
        ["/system", "/library", "/usr", "/bin", "/sbin", "/etc", "/private/etc", "/private/var/db", "/boot", "/dev", "/proc", "/sys", "/var/lib"];
    const WINDOWS: [&str; 4] = ["windows", "program files", "program files (x86)", "programdata"];
    let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
    // A drive's root folder: `c:/windows/…` (or `//?/c:/…`).
    let after_drive = text.trim_start_matches("//?/").get(2..).filter(|_| text.trim_start_matches("//?/").get(1..2) == Some(":"));
    if let Some(rest) = after_drive {
        let first = rest.trim_start_matches('/').split('/').next().unwrap_or_default();
        return WINDOWS.iter().find(|w| **w == first).copied();
    }
    UNIX.iter().find(|u| text == **u || text.starts_with(&format!("{u}/"))).copied()
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

    #[test]
    fn archive_names_that_leave_the_folder_are_unsafe() {
        for ok in ["a.dwg", "Xrefs/a.dwg", "Xrefs\\b.png", "./x/y.txt", "d\u{e9}j\u{e0}/vu.dxf"] {
            assert!(is_safe_relative(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "\\\\server\\share\\x",
            "C:\\Windows\\x.dll",
            "c:x",
            "../x",
            "a/../../x",
            "a\\..\\x",
            "x\u{0}y",
            "x\ny",
            ".",
            "./",
            "a:b",
        ] {
            assert!(!is_safe_relative(bad), "{bad:?}");
        }
    }

    #[test]
    fn system_folders_are_known() {
        for (p, want) in [
            ("C:\\Windows\\System32\\x.dll", Some("windows")),
            ("c:/program files/app/x", Some("program files")),
            ("D:\\ProgramData\\x", Some("programdata")),
            ("/etc/hosts", Some("/etc")),
            ("/System/Library/x", Some("/system")),
            ("/usr/local/bin/x", Some("/usr")),
            ("/Library/LaunchDaemons/x.plist", Some("/library")),
        ] {
            assert_eq!(system_folder(Path::new(p)), want, "{p}");
        }
        for p in ["/Users/x/Library/a.dwg", "/home/x/etc/a.dwg", "C:\\Users\\x\\Windows\\a.dwg", "/usrdata/a.dwg", "relative/etc/x"] {
            assert_eq!(system_folder(Path::new(p)), None, "{p}");
        }
    }
}
