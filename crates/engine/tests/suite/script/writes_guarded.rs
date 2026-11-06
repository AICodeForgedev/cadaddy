//! "Dry run writes nothing" is structural: a command that writes, creates, renames, copies or
//! deletes a file goes through `writefile` (refused during an agent's dry run, and never into a
//! system folder). This scan fails on any non-test file-changing call in `src/` outside
//! `writefile.rs`, except the allowlist below, where each entry says why it can't be a command's
//! write in a dry run.

use std::path::{Path, PathBuf};

const NEEDLES: [&str; 6] = ["fs::write(", "File::create(", "fs::remove_file(", "fs::rename(", "fs::copy(", "OpenOptions::new("];

/// (file under `src/`, text of the line, why that direct file call can't be a dry run's write).
const ALLOWED: &[(&str, &str, &str)] = &[
    ("atomic_write.rs", "", "the primitive under writefile::write_atomic, which checks the path first; its other callers check too"),
    ("autosave.rs", "", "crash-recovery files and locks the app keeps on its own, never an agent's command output"),
    ("control_auth.rs", "", "the agent port's token file, written at startup (no session, no dry run)"),
    ("crash_report.rs", "", "the panic hook's report, written when the app dies"),
    ("cmd/file.rs", "OpenOptions::new().write(true)", "appends to the file being saved; writefile::check on the line before refuses a dry run"),
    ("lisp/builtins.rs", "OpenOptions::new()", "(open ... \"w\"); writefile::check just above refuses a dry run"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The lines of `text` that are not test code: a test file is skipped whole, a file's
/// `#[cfg(test)] mod` runs to its end, and any other `#[cfg(test)]` item is skipped by itself.
fn production_lines(path: &Path, text: &str) -> Vec<(usize, String)> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "tests.rs" || name.ends_with("_tests.rs") || path.components().any(|c| c.as_os_str() == "tests") {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.starts_with("#[cfg(test)]") {
            if lines.get(i + 1).is_some_and(|l| l.trim_start().starts_with("mod ")) {
                break;
            }
            // One item: to the closing brace (or semicolon) at the attribute's own indent.
            let indent = lines[i].len() - t.len();
            i += 1;
            while i < lines.len() {
                let l = lines[i];
                let at = l.len() - l.trim_start().len();
                i += 1;
                if at == indent && (l.trim_start().starts_with('}') || l.trim_end().ends_with(';')) {
                    break;
                }
            }
            continue;
        }
        if !t.starts_with("//") {
            out.push((i + 1, lines[i].to_string()));
        }
        i += 1;
    }
    out
}

#[test]
fn no_unguarded_file_changes_outside_writefile() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 50, "found the sources");
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for f in files {
        let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "writefile.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in production_lines(&f, &text) {
            if NEEDLES.iter().any(|k| line.contains(k)) {
                match ALLOWED.iter().find(|(p, l, _)| *p == rel && line.contains(l)) {
                    Some((p, l, _)) => {
                        used.insert((*p, *l));
                    }
                    None => bad.push(format!("{rel}:{n}: {}", line.trim())),
                }
            }
        }
    }
    assert!(bad.is_empty(), "file changes that bypass writefile (a dry run would perform them):\n{}", bad.join("\n"));
    for (p, l, why) in ALLOWED {
        assert!(!why.is_empty() && used.contains(&(*p, *l)), "allowlist entry {p} {l} is stale or has no reason");
    }
}
