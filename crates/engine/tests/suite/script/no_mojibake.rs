//! Guard against UTF-8 text that was read as Windows-1252 and saved again ("mojibake"): `…`
//! turning into three junk characters, `→` and `×` likewise. A merge once did this to the
//! `bspec.draft` params help that agents read, so check the command texts themselves and every
//! Rust source in the workspace.

use std::path::Path;

use cadaddy_engine::cmd::{command_specs, find_command};

/// UTF-8 bytes of common punctuation read as Windows-1252. Written as escapes so this file
/// doesn't trip its own check.
const MOJIBAKE: &[&str] = &[
    "\u{e2}\u{20ac}", // … – — ‘ ’ “ ” • (E2 80 xx)
    "\u{e2}\u{2020}", // ← ↑ → ↓ (E2 86 xx)
    "\u{e2}\u{2030}", // ≈ ≠ ≤ ≥ (E2 89 xx)
    "\u{c3}\u{2014}", // × (C3 97)
    "\u{c3}\u{a9}",   // é (C3 A9)
    "\u{c2}\u{b0}",   // ° (C2 B0)
    "\u{c2}\u{b1}",   // ± (C2 B1)
];

fn mojibake_in(text: &str) -> Option<&'static str> {
    MOJIBAKE.iter().copied().find(|m| text.contains(m))
}

#[test]
fn bspec_draft_params_text_is_clean_utf8() {
    let spec = find_command("bspec.draft").expect("bspec.draft is registered");
    assert!(spec.params.contains('→'), "params lost its arrow: {}", spec.params);
    assert_eq!(mojibake_in(spec.params), None, "mojibake in bspec.draft params: {}", spec.params);
}

#[test]
fn no_command_text_has_mojibake() {
    let bad: Vec<String> = command_specs()
        .iter()
        .flat_map(|c| [("label", c.label), ("params", c.params), ("help", c.help)].map(|(f, t)| (c.id, f, t)))
        .filter(|(_, _, t)| mojibake_in(t).is_some())
        .map(|(id, f, t)| format!("  {id} {f}: {t}"))
        .collect();
    assert!(bad.is_empty(), "mojibake in command texts:\n{}", bad.join("\n"));
}

#[test]
fn no_rust_source_has_mojibake() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut bad = Vec::new();
    let mut files = 0;
    for dir in ["crates", "apps", "xtask"] {
        walk(&root.join(dir), &mut bad, &mut files);
    }
    assert!(files > 100, "found only {files} .rs files under {}", root.display());
    assert!(bad.is_empty(), "UTF-8 read as Windows-1252 in Rust sources (retype the characters):\n{}", bad.join("\n"));
}

fn walk(dir: &Path, bad: &mut Vec<String>, files: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !path.ends_with("target") {
                walk(&path, bad, files);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            *files += 1;
            for (i, line) in text.lines().enumerate() {
                if let Some(m) = mojibake_in(line) {
                    bad.push(format!("  {}:{}: {m:?} in {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    }
}
