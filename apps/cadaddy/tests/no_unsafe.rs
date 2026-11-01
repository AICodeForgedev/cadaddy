//! The workspace forbids `unsafe` (`unsafe_code = "forbid"`), but that lint only bites on the
//! platform that compiles the file: a macOS-only or Windows-only module with an `unsafe` block
//! passes every Linux build and breaks the real one. This scan reads every source file on every
//! platform and fails on `unsafe {`, `unsafe fn`, `unsafe impl` or `unsafe trait` outside test
//! code, except in a crate whose `Cargo.toml` opts out (`unsafe_code = "allow"`), which is where
//! such code belongs.

use std::path::{Path, PathBuf};

const NEEDLES: [&str; 4] = ["unsafe {", "unsafe fn", "unsafe impl", "unsafe trait"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The directory of the crate `file` belongs to: the nearest parent with a `Cargo.toml`.
fn crate_dir(file: &Path) -> Option<PathBuf> {
    file.ancestors().skip(1).find(|d| d.join("Cargo.toml").is_file()).map(Path::to_path_buf)
}

fn opts_out(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|t| {
        t.lines().any(|l| {
            let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
            l.starts_with("unsafe_code=\"allow\"")
        })
    })
}

/// Lines that are not test code: a test file is skipped whole, a `#[cfg(test)] mod` runs to the
/// end of its file, and comments are ignored.
fn production_lines(path: &Path, text: &str) -> Vec<(usize, String)> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "tests.rs" || name.ends_with("_tests.rs") || path.components().any(|c| c.as_os_str() == "tests") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("#[cfg(test)]") {
            break;
        }
        if !t.starts_with("//") {
            out.push((i + 1, line.to_string()));
        }
    }
    out
}

/// The offending lines (`path:line: text`) in `files`, each read as the crate it belongs to.
fn unsafe_uses(files: &[PathBuf], root: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    for file in files {
        if crate_dir(file).is_some_and(|d| opts_out(&d)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for (n, line) in production_lines(file, &text) {
            if NEEDLES.iter().any(|needle| line.contains(needle)) {
                bad.push(format!("{}:{n}: {}", file.strip_prefix(root).unwrap_or(file).display(), line.trim()));
            }
        }
    }
    bad
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn no_unsafe_outside_crates_that_opt_out() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("apps"), &mut files);
    rust_files(&root.join("crates"), &mut files);
    assert!(files.len() > 100, "the scan found only {} source files; is the path right?", files.len());
    let bad = unsafe_uses(&files, &root);
    assert!(
        bad.is_empty(),
        "`unsafe` in a crate that forbids it (the lint only fires on the platform that compiles the file). \
         Use a safe API, or put it in a small crate with `unsafe_code = \"allow\"` like crates/macos-open:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_scan_catches_an_unsafe_block_and_honours_the_opt_out() {
    let dir = std::env::temp_dir().join(format!("cadaddy-no-unsafe-{}", std::process::id()));
    let (strict, loose) = (dir.join("strict"), dir.join("loose"));
    for d in [&strict, &loose] {
        std::fs::create_dir_all(d.join("src")).expect("make dir");
    }
    std::fs::write(strict.join("Cargo.toml"), "[package]\nname = \"strict\"\n").expect("write");
    std::fs::write(loose.join("Cargo.toml"), "[lints.rust]\nunsafe_code = \"allow\"\n").expect("write");
    let code = "fn f() {\n    // unsafe { not code }\n    let x = unsafe { 1 };\n}\n#[cfg(test)]\nmod t { fn g() { unsafe { 2 }; } }\n";
    let (a, b) = (strict.join("src/lib.rs"), loose.join("src/lib.rs"));
    std::fs::write(&a, code).expect("write");
    std::fs::write(&b, code).expect("write");
    let bad = unsafe_uses(&[a, b], &dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].contains("strict") && bad[0].contains(":3:"), "{bad:?}");
}
