//! Stamps the engine with its reading code (`CADADDY_SOURCE_STAMP`): a hash of the sources of the
//! engine and of every workspace crate below it (each one's `src` and `Cargo.toml`), the shared
//! `assets` and the lock file. A job's checkpoint records the stamp of the build that ran each
//! stage, so a resume can tell a stage this build read from one another build read (the guided
//! job's `refresh`). The same sources stamp the same on every machine: paths are taken below the
//! workspace with `/`, files in name order, and carriage returns are left out (a Windows checkout).
//! It only reads the local checkout; a file it can't read is skipped.

use std::path::{Path, PathBuf};

/// Most files hashed (a runaway folder can't stall the build).
const MAX_FILES: usize = 200_000;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let root = manifest.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| manifest.clone());
    let mut inputs: Vec<PathBuf> = vec![manifest.join("src"), manifest.join("Cargo.toml"), manifest.join("build.rs")];
    // The crates below the engine: every crate of the workspace but the ones built on it.
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        let mut crates: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p != &manifest).collect();
        crates.sort();
        for dir in crates {
            let Ok(toml) = std::fs::read_to_string(dir.join("Cargo.toml")) else { continue };
            if toml.lines().any(|l| l.trim_start().starts_with("cadaddy-engine")) {
                continue;
            }
            inputs.push(dir.join("src"));
            inputs.push(dir.join("Cargo.toml"));
        }
    }
    inputs.push(root.join("assets"));
    inputs.push(root.join("Cargo.lock"));

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    // (A path that isn't there would run this script on every build.)
    for input in inputs.iter().filter(|p| p.exists()) {
        println!("cargo::rerun-if-changed={}", input.display());
        collect(&root, input, &mut files);
    }
    files.sort();
    files.dedup();

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for (name, path) in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        feed(name.as_bytes());
        feed(&[0]);
        let text: Vec<u8> = bytes.into_iter().filter(|b| *b != b'\r').collect();
        feed(&(text.len() as u64).to_le_bytes());
        feed(&text);
    }
    println!("cargo::rustc-env=CADADDY_SOURCE_STAMP={h:016x}");
}

/// Every file at `path` (a file, or a folder and the folders in it; hidden ones left out), named
/// below `root` with `/`.
fn collect(root: &Path, path: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        if out.len() >= MAX_FILES {
            return;
        }
        let hidden = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if p.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                stack.extend(entries.filter_map(|e| e.ok().map(|e| e.path())));
            }
        } else if p.is_file() {
            let rel = p.strip_prefix(root).unwrap_or(&p);
            let name: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            out.push((name.join("/"), p));
        }
    }
}
