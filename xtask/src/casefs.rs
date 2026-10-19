//! `cargo xtask casefs`: run the workspace tests on a case-sensitive file system, so failures that
//! only Linux would show (a file found by the wrong case, `Base.DWG` vs `base.dwg`) show up on a
//! Mac too. macOS disks ignore case; this makes a small case-sensitive APFS disk image, copies
//! the source in (tracked and untracked-but-not-ignored files, so uncommitted work is tested),
//! points TMPDIR inside it (tests make their files under the temporary folder), runs
//! `cargo test --workspace` there, then detaches and deletes the image whatever happened. The
//! build output stays outside the image (`target/casefs`, or CARGO_TARGET_DIR) to keep it small.
//! On Linux the disk already is case-sensitive: the tests just run.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The image's largest size; a sparse image only takes what is written to it.
const DEFAULT_SIZE: &str = "8g";

/// Options: `--size 8g`, then anything after `--` goes to `cargo test`.
#[derive(Debug, PartialEq)]
pub struct Opts {
    pub size: String,
    pub test_args: Vec<String>,
}

pub fn parse(rest: &[&str]) -> Result<Opts, String> {
    let mut opts = Opts { size: DEFAULT_SIZE.into(), test_args: Vec::new() };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--size" => {
                let v = it.next().ok_or("--size needs a value such as 8g")?;
                let ok = v.len() > 1 && v[..v.len() - 1].chars().all(|c| c.is_ascii_digit()) && v.ends_with(['g', 'm', 'G', 'M']);
                if !ok {
                    return Err(format!("--size {v}: use a number with g or m, such as 8g"));
                }
                opts.size = (*v).to_string();
            }
            "--" => {
                opts.test_args = it.map(|s| (*s).to_string()).collect();
                break;
            }
            other => return Err(format!("unknown option `{other}` (casefs [--size 8g] [-- <cargo test args>])")),
        }
    }
    Ok(opts)
}

/// The files to copy: `git ls-files` (tracked, plus untracked files that aren't ignored).
fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(split_z(&out.stdout))
}

/// NUL-separated paths, without empties and without any that climb out (`..`) or are absolute.
pub fn split_z(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned()))
        .filter(|p| p.is_relative() && !p.components().any(|c| matches!(c, std::path::Component::ParentDir)))
        .collect()
}

/// Copy `list` (relative to `from`) into `to`; files that are gone (deleted, not yet committed)
/// are skipped. Returns how many were copied.
pub fn copy_files(from: &Path, to: &Path, list: &[PathBuf]) -> Result<usize, String> {
    let mut n = 0;
    for rel in list {
        let src = from.join(rel);
        if !src.is_file() {
            continue;
        }
        let dst = to.join(rel);
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::copy(&src, &dst).map_err(|e| format!("{} → {}: {e}", src.display(), dst.display()))?;
        n += 1;
    }
    Ok(n)
}

/// The mounted image: detached and deleted when dropped, whatever happened.
struct Image {
    dir: PathBuf,
    mount: PathBuf,
    attached: bool,
}

impl Drop for Image {
    fn drop(&mut self) {
        if self.attached {
            let ok = Command::new("hdiutil").arg("detach").arg(&self.mount).arg("-quiet").status().is_ok_and(|s| s.success());
            if !ok {
                let _ = Command::new("hdiutil").arg("detach").arg(&self.mount).args(["-force", "-quiet"]).status();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn hdiutil(args: &[&std::ffi::OsStr], what: &str) -> Result<(), String> {
    let out = Command::new("hdiutil").args(args).output().map_err(|e| format!("hdiutil {what}: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("hdiutil {what} failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// `cargo test` arguments: the whole workspace unless packages are named.
pub fn test_args(opts: &Opts) -> Vec<String> {
    let named = opts.test_args.iter().any(|a| a == "-p" || a == "--package" || a.starts_with("--package="));
    let mut v = vec!["test".to_string()];
    if !named {
        v.push("--workspace".into());
    }
    v.extend(opts.test_args.iter().cloned());
    v
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let opts = parse(rest)?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target").join("casefs"));
    if cfg!(target_os = "linux") {
        eprintln!("casefs: this disk is already case-sensitive; running the tests here");
        let mut c = crate::cargo();
        c.args(test_args(&opts));
        return crate::run(c, &format!("cargo {}", test_args(&opts).join(" ")));
    }
    if !cfg!(target_os = "macos") {
        return Err("casefs makes its case-sensitive disk with hdiutil: macOS only (on Linux the tests run as they are)".into());
    }
    let dir = std::env::temp_dir().join(format!("cadaddy-casefs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut img = Image { mount: dir.join("mnt"), dir: dir.clone(), attached: false };
    let image = dir.join("case.sparseimage");
    eprintln!("casefs: creating a case-sensitive APFS image ({}, sparse)", opts.size);
    hdiutil(
        &["create", "-quiet", "-size", &opts.size, "-type", "SPARSE", "-fs", "Case-sensitive APFS", "-volname", "CADaddyCase"]
            .iter()
            .map(std::ffi::OsStr::new)
            .chain([image.as_os_str()])
            .collect::<Vec<_>>(),
        "create",
    )?;
    std::fs::create_dir_all(&img.mount).map_err(|e| format!("{}: {e}", img.mount.display()))?;
    hdiutil(
        &["attach", "-quiet", "-nobrowse", "-noverify", "-mountpoint"]
            .iter()
            .map(std::ffi::OsStr::new)
            .chain([img.mount.as_os_str(), image.as_os_str()])
            .collect::<Vec<_>>(),
        "attach",
    )?;
    img.attached = true;
    let src = img.mount.join("src");
    let tmp = img.mount.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    // Prove the volume tells case apart before trusting a green run.
    std::fs::write(tmp.join("Probe"), b"1").map_err(|e| format!("probe: {e}"))?;
    if tmp.join("probe").exists() {
        return Err("the image is not case-sensitive".into());
    }
    let n = copy_files(root, &src, &files(root)?)?;
    eprintln!("casefs: copied {n} files; build output in {}", target.display());
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(&src).env("TMPDIR", &tmp).env("CARGO_TARGET_DIR", &target).args(test_args(&opts));
    crate::run(c, &format!("cargo {} (case-sensitive disk)", test_args(&opts).join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        assert_eq!(parse(&[]).unwrap(), Opts { size: "8g".into(), test_args: vec![] });
        let o = parse(&["--size", "4g", "--", "-p", "cadaddy-engine", "--test", "path_case"]).unwrap();
        assert_eq!((o.size.as_str(), o.test_args.len()), ("4g", 4));
        assert_eq!(test_args(&o), ["test", "-p", "cadaddy-engine", "--test", "path_case"], "a package named: not the whole workspace");
        assert_eq!(test_args(&parse(&["--", "--release"]).unwrap()), ["test", "--workspace", "--release"]);
        for bad in [&["--size"][..], &["--size", "lots"], &["--size", "g"], &["--keep"]] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn file_lists_stay_inside_and_copy_keeps_case() {
        let list = split_z(b"a/B.rs\0c.txt\0\0../escape\0/abs\0");
        assert_eq!(list, vec![PathBuf::from("a/B.rs"), PathBuf::from("c.txt")]);
        let dir = std::env::temp_dir().join(format!("cadaddy-casefs-test-{}", std::process::id()));
        let (from, to) = (dir.join("from"), dir.join("to"));
        std::fs::create_dir_all(from.join("a")).unwrap();
        std::fs::write(from.join("a/B.rs"), b"x").unwrap();
        assert_eq!(copy_files(&from, &to, &list).unwrap(), 1, "c.txt is gone: skipped");
        let names: Vec<String> = std::fs::read_dir(to.join("a")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["B.rs"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
