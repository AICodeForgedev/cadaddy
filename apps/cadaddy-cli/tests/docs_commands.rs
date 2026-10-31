//! The `cadaddy-cli convert`, `info` and `verify` lines in the guides and the README run: each is
//! taken from a code block, its paths swapped for sample files and folders made in a temp folder,
//! and the CLI must not answer "unknown option" or print its usage text.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cli(args: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cli-docs-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Split a documented line into words, honouring double quotes.
fn words(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Every `cadaddy-cli convert|info|verify` line inside a code block of `text`.
fn documented(text: &str) -> Vec<String> {
    let mut in_block = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_block = !in_block;
        } else if in_block && ["convert", "info", "verify"].iter().any(|c| t.starts_with(&format!("cadaddy-cli {c} "))) {
            lines.push(t.to_string());
        }
    }
    lines
}

/// Make a sample drawing at `path`, whose extension picks the format (a .cdy goes through `convert`).
fn make_sample(path: &Path) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("dxf").to_string();
    let target = if ext == "cdy" { path.with_extension("dxf") } else { path.to_path_buf() };
    let out = cli(&["sample".into(), "floorplan".into(), target.display().to_string()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    if ext == "cdy" {
        let out = cli(&["convert".into(), target.display().to_string(), path.display().to_string()]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
}

/// The line's arguments with each path swapped for one under `dir`.
fn swap_paths(dir: &Path, n: usize, line: &str) -> Vec<String> {
    let w = words(line);
    let (command, rest) = (w[1].clone(), &w[2..]);
    let mut out = vec![command.clone()];
    let mut positional = 0;
    let mut skip_value = false;
    for a in rest {
        if skip_value {
            skip_value = false;
            out.push(a.clone());
        } else if a.starts_with("--") {
            skip_value = matches!(a.as_str(), "--to" | "--window" | "--view");
            out.push(a.clone());
        } else {
            positional += 1;
            let is_dir = a.ends_with('/') || Path::new(a).extension().is_none();
            let ext = Path::new(a).extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
            let is_input = positional == 1;
            let path = if is_dir {
                dir.join(format!("{}{n}-{positional}", if is_input { "in" } else { "out" }))
            } else {
                dir.join(format!("{n}-{positional}.{ext}"))
            };
            if is_input {
                if is_dir {
                    std::fs::create_dir_all(&path).unwrap();
                    make_sample(&path.join("sample.dxf"));
                } else {
                    make_sample(&path);
                }
            }
            out.push(path.display().to_string());
        }
    }
    out
}

#[test]
fn documented_cli_lines_are_accepted_by_the_cli() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = vec![root.join("README.md")];
    for e in std::fs::read_dir(root.join("docs/guide")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "md") {
            files.push(p);
        }
    }
    let dir = folder("lines");
    let (mut n, mut ran) = (0, 0);
    let mut seen = Vec::new();
    for f in files {
        for line in documented(&std::fs::read_to_string(&f).unwrap()) {
            n += 1;
            let args = swap_paths(&dir, n, &line);
            let out = cli(&args);
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(!err.contains("unknown option"), "{}: `{line}`: {err}", f.display());
            assert!(!err.contains("usage:"), "{}: `{line}` printed the usage text: {err}", f.display());
            // `verify` and every conversion of a sample must also succeed.
            assert!(out.status.success(), "{}: `{line}`: {err}", f.display());
            seen.push(args[0].clone());
            ran += 1;
        }
    }
    assert!(ran >= 6, "found only {ran} documented lines");
    for c in ["convert", "info", "verify"] {
        assert!(seen.iter().any(|s| s == c), "no documented `{c}` line");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_old_input_output_form_is_rejected_so_the_docs_must_not_show_it() {
    let out = cli(&["convert".into(), "--input".into(), "a".into(), "--output".into(), "b".into(), "--format".into(), "cdy".into()]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for f in ["README.md", "docs/guide/for-ai-agents.md", "docs/guide/working-with-files.md"] {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        assert!(!text.contains("cadaddy-cli convert --input"), "{f}");
    }
    for f in ["docs/guide/for-ai-agents.md", "docs/guide/working-with-files.md"] {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        assert!(text.contains("cadaddy-cli verify") && text.contains("--recurse"), "{f} must document verify and --recurse");
    }
}
