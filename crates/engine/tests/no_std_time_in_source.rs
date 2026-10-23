//! Web-clock guard (webtest #1/#2): `std::time::Instant` and `std::time::SystemTime::now()`
//! panic on wasm32-unknown-unknown and take the whole web page down. Every crate the web build
//! compiles must use `web_time` (the same types on native) outside native-only and test code.
//!
//! The scan follows each crate's module tree from `lib.rs` (`main.rs` for the web app) like the
//! compiler does, drops what `#[cfg(...)]` leaves out of a wasm32 non-test build (items,
//! statements, blocks, whole modules and files), and flags in what is left:
//! - `Instant` from `std::time` in any form: `std::time::Instant`, `use std::time::Instant`,
//!   `use std::time::{Duration, Instant}`, `use std::{time::Instant}`, `time::Instant` after
//!   `use std::time`;
//! - `SystemTime::now` from `std::time`: by path, or after a `use` that imports it from `std`.
//!
//! Lines that are compiled for wasm but never run there are listed in [`ALLOWED`], each with why.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Every workspace crate the web build (apps/cadaddy-web) compiles, as
/// `cargo tree -p cadaddy-web --target wasm32-unknown-unknown` lists them.
const WASM_CRATES: &[&str] = &[
    "crates/cam",
    "crates/color",
    "crates/constraints",
    "crates/doc",
    "crates/dwg",
    "crates/dwgcodec",
    "crates/dxf",
    "crates/engine",
    "crates/fonts",
    "crates/geom",
    "crates/io",
    "crates/mepfix",
    "crates/render",
    "crates/rhai-bridge",
    "crates/ui-egui",
    "crates/walk3d",
    "apps/cadaddy-web",
];

/// (file under the workspace, text on the flagged line, why it never runs on wasm).
const ALLOWED: &[(&str, &str, &str)] = &[];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn wasm_built_crates_use_web_time_not_std_clocks() {
    let root = normal(&workspace());
    let mut bad = Vec::new();
    let mut scanned = 0;
    for krate in WASM_CRATES {
        let src = root.join(krate).join("src");
        let entry = if krate.starts_with("apps/") { src.join("main.rs") } else { src.join("lib.rs") };
        assert!(entry.is_file(), "{} is missing: the crate list is stale", entry.display());
        let tree = scan_tree(&entry);
        assert!(tree.files.contains(&normal(&entry)), "{krate}: {} not scanned", entry.display());
        // Every source file was either scanned or left out of the build by a cfg: nothing is
        // skipped because the walk lost its way.
        // (src/bin holds the crate's own tools, never built for the web app.)
        let bin = normal(&src.join("bin"));
        let missed: Vec<_> = rs_files(&src)
            .into_iter()
            .filter(|f| !f.starts_with(&bin) && !tree.files.contains(f) && !tree.excluded.iter().any(|x| f.starts_with(x)))
            .collect();
        assert!(missed.is_empty(), "{krate}: files the module walk did not reach: {missed:#?}");
        scanned += tree.files.len();
        for hit in tree.hits {
            let rel = hit.file.strip_prefix(&root).unwrap_or(&hit.file).to_string_lossy().replace('\\', "/");
            let allowed = ALLOWED.iter().any(|(f, text, _)| rel.ends_with(f) && hit.text.contains(text));
            if !allowed {
                bad.push(format!("  {rel}:{}: {} ({})", hit.line, hit.text, hit.what));
            }
        }
    }
    assert!(scanned > 500, "only {scanned} files scanned");
    assert!(
        bad.is_empty(),
        "std clocks in code the web build compiles (use web_time::Instant / web_time::SystemTime, or gate it with #[cfg(not(target_arch = \"wasm32\"))]):\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_guard_catches_every_form_and_skips_native_and_test_code() {
    let flagged = |src: &str| scan_text(src).into_iter().map(|h| h.line).collect::<Vec<_>>();
    // Each form, on the line it is written.
    assert_eq!(flagged("fn a() { let t = std::time::Instant::now(); }"), [1]);
    assert_eq!(flagged("use std::time::{Duration, Instant};\nfn a() { Instant::now(); }"), [1]);
    assert_eq!(flagged("use std::time::Instant as Clock;"), [1]);
    assert_eq!(flagged("use std::{fmt, time::{Instant}};"), [1]);
    assert_eq!(flagged("use std::time;\nfn a() { time::Instant::now(); }"), [2]);
    assert_eq!(flagged("fn a() { ::std::time::SystemTime::now(); }"), [1]);
    assert_eq!(flagged("use std::time::{SystemTime, UNIX_EPOCH};\nfn a() {\n    SystemTime::now();\n}"), [3]);
    // web_time is the fix; Duration and UNIX_EPOCH are fine; comments and strings don't count.
    assert_eq!(flagged("use web_time::{Duration, Instant, SystemTime};\nfn a() { Instant::now(); SystemTime::now(); }"), [0; 0]);
    assert_eq!(flagged("fn a() { web_time::Instant::now(); std::time::Duration::from_secs(1); std::time::UNIX_EPOCH; }"), [0; 0]);
    assert_eq!(flagged("// std::time::Instant::now()\n/* SystemTime::now() */\nconst S: &str = \"std::time::Instant\";"), [0; 0]);
    // Native-only and test code: items, statements, blocks and modules.
    let gated = r#"
#[cfg(not(target_arch = "wasm32"))]
fn native() { std::time::Instant::now(); }
#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};
    fn t() { Instant::now(); }
}
fn mixed() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
    #[cfg(target_arch = "wasm32")]
    { 0 }
}
#[cfg(all(not(target_arch = "wasm32"), feature = "x"))]
use std::time::Instant;
#[cfg(any(windows, target_os = "macos"))]
fn desktop(a: u8, b: u8) { std::time::Instant::now(); }
"#;
    assert_eq!(flagged(gated), [0; 0]);
    // ...but not code a wasm build keeps, even next to gated code.
    let kept = r#"
#[cfg(any(test, target_arch = "wasm32"))]
fn both() { std::time::Instant::now(); }
#[cfg(not(test))]
fn release() { std::time::Instant::now(); }
#[cfg(not(target_arch = "wasm32"))]
fn native(a: u8, b: u8) {}
fn after() { std::time::Instant::now(); }
#[cfg(feature = "x")]
fn feature() { std::time::Instant::now(); }
"#;
    assert_eq!(flagged(kept), [3, 5, 8, 10]);
    // A whole file left out.
    assert_eq!(flagged("#![cfg(not(target_arch = \"wasm32\"))]\nfn a() { std::time::Instant::now(); }"), [0; 0]);
}

/// The webtest #1 crash came back in ui-egui's gpu.rs: the guard sees it there (the canvas
/// callback's frame timer, which runs on the web; the native-only frame cap may keep std).
#[test]
fn the_guard_fails_if_gpu_rs_goes_back_to_std_instant() {
    let gpu = workspace().join("crates/ui-egui/src/gpu.rs");
    let text = std::fs::read_to_string(&gpu).unwrap();
    assert!(scan_text(&text).is_empty(), "gpu.rs is clean today");
    let timer = "let t0 = web_time::Instant::now();";
    let at = text.lines().position(|l| l.trim() == timer).expect("the canvas callback's timer") + 1;
    let back = text.replacen(timer, "let t0 = std::time::Instant::now();", 1);
    assert_eq!(scan_text(&back).iter().map(|h| h.line).collect::<Vec<_>>(), [at]);
    let lines = text.lines().count();
    let regressed = format!("{text}\nfn frame_start() -> std::time::Instant {{\n    std::time::Instant::now()\n}}\n");
    assert_eq!(scan_text(&regressed).iter().map(|h| h.line).collect::<Vec<_>>(), [lines + 2, lines + 3]);
    let imported = format!("use std::time::{{Duration, Instant}};\n{text}");
    assert_eq!(scan_text(&imported).iter().map(|h| h.line).collect::<Vec<_>>(), [1]);
    // And the tree scan reaches gpu.rs.
    let tree = scan_tree(&workspace().join("crates/ui-egui/src/lib.rs"));
    assert!(tree.files.iter().any(|f| f.ends_with("gpu.rs")), "gpu.rs scanned");
}

// ---------- the scanner ----------

struct Hit {
    file: PathBuf,
    line: usize,
    text: String,
    what: &'static str,
}

#[derive(Default)]
struct Tree {
    /// Files scanned (compiled into a wasm32 non-test build).
    files: BTreeSet<PathBuf>,
    /// Module files and folders a cfg leaves out (with everything under them).
    excluded: Vec<PathBuf>,
    hits: Vec<Hit>,
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(normal(&p));
            }
        }
    }
    out
}

fn normal(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Scan `entry` and every module file it declares, as compiled for wasm32 without tests.
fn scan_tree(entry: &Path) -> Tree {
    let mut tree = Tree::default();
    let mut todo = vec![normal(entry)];
    while let Some(file) = todo.pop() {
        if !tree.files.insert(file.clone()) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let scan = scan(&text);
        if scan.file_excluded {
            tree.files.remove(&file);
            tree.excluded.push(file.clone());
            continue;
        }
        for m in &scan.mods {
            let candidates = module_files(&file, m);
            let found = candidates.iter().find(|c| c.is_file()).map(|c| normal(c));
            if m.excluded {
                // Its file, and the folder its submodules would live in.
                tree.excluded.extend(candidates.iter().filter(|c| c.is_file()).map(|c| normal(c)));
                if let Some(f) = &found {
                    tree.excluded.push(f.with_extension(""));
                }
                continue;
            }
            let found = found.unwrap_or_else(|| panic!("{}: `mod {}` has no file among {candidates:?}", file.display(), m.name));
            todo.push(found);
        }
        tree.hits.extend(scan.hits.into_iter().map(|(line, what)| Hit {
            file: file.clone(),
            line,
            text: text.lines().nth(line - 1).unwrap_or("").trim().to_string(),
            what,
        }));
    }
    tree
}

/// Where `mod name;` declared in `file` lives (Rust's module path rules).
fn module_files(file: &Path, m: &ModDecl) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new("."));
    if let Some(p) = &m.path {
        return vec![dir.join(p)];
    }
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut base = if matches!(stem, "lib" | "main" | "mod") { dir.to_path_buf() } else { dir.join(stem) };
    for inline in &m.inline {
        base = base.join(inline);
    }
    vec![base.join(format!("{}.rs", m.name)), base.join(&m.name).join("mod.rs")]
}

fn scan_text(text: &str) -> Vec<Hit> {
    let s = scan(text);
    if s.file_excluded {
        return Vec::new();
    }
    s.hits.into_iter().map(|(line, what)| Hit { file: PathBuf::new(), line, text: String::new(), what }).collect()
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Ident,
    Str,
    Punct,
}

#[derive(Clone, Debug)]
struct Tok {
    kind: Kind,
    text: String,
    line: usize,
}

/// Rust tokens without comments; string and char literals become one `Str` token (char
/// literals and lifetimes are dropped).
fn tokens(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
        } else if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && at(i + 1) == '/' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == '*' {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if c[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if (ch == 'r' || (ch == 'b' && at(i + 1) == 'r')) && {
            let j = if ch == 'b' { i + 2 } else { i + 1 };
            let mut k = j;
            while at(k) == '#' {
                k += 1;
            }
            at(k) == '"' && (ch == 'b' || at(j) == '#' || at(j) == '"')
        } {
            // Raw string r"…", r#"…"#, br"…".
            let mut j = if ch == 'b' { i + 2 } else { i + 1 };
            let mut hashes = 0;
            while at(j) == '#' {
                hashes += 1;
                j += 1;
            }
            j += 1;
            let start_line = line;
            let start = j;
            loop {
                if j >= c.len() {
                    break;
                }
                if c[j] == '"' && (1..=hashes).all(|h| at(j + h) == '#') {
                    break;
                }
                if c[j] == '\n' {
                    line += 1;
                }
                j += 1;
            }
            out.push(Tok { kind: Kind::Str, text: c[start..j.min(c.len())].iter().collect(), line: start_line });
            i = j + 1 + hashes;
        } else if ch == '"' || (ch == 'b' && at(i + 1) == '"') {
            let mut j = if ch == 'b' { i + 2 } else { i + 1 };
            let start_line = line;
            let mut s = String::new();
            while j < c.len() && c[j] != '"' {
                if c[j] == '\\' {
                    s.push(at(j + 1));
                    if at(j + 1) == '\n' {
                        line += 1;
                    }
                    j += 2;
                    continue;
                }
                if c[j] == '\n' {
                    line += 1;
                }
                s.push(c[j]);
                j += 1;
            }
            out.push(Tok { kind: Kind::Str, text: s, line: start_line });
            i = j + 1;
        } else if ch == '\'' {
            // A char literal ('a', '\n', '\u{1F}') or a lifetime ('a).
            if at(i + 1) == '\\' {
                let mut j = i + 2;
                while j < c.len() && c[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
            } else if at(i + 2) == '\'' {
                i += 3;
            } else {
                i += 1;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
            }
        } else if ch.is_alphanumeric() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            // Raw identifiers (r#type) are their plain name.
            if i - start == 1 && ch == 'r' && at(i) == '#' && (at(i + 1).is_alphabetic() || at(i + 1) == '_') {
                i += 1;
                continue;
            }
            out.push(Tok { kind: Kind::Ident, text: c[start..i].iter().collect(), line });
        } else if ch == ':' && at(i + 1) == ':' {
            out.push(Tok { kind: Kind::Punct, text: "::".into(), line });
            i += 2;
        } else {
            out.push(Tok { kind: Kind::Punct, text: ch.to_string(), line });
            i += 1;
        }
    }
    out
}

/// What `cfg(...)` is in a wasm32 non-test build: `Some(false)` left out, `None` can't tell
/// (a feature, a pointer width...), which counts as compiled.
fn eval_cfg(t: &[Tok], i: &mut usize) -> Option<bool> {
    let name = t.get(*i)?.text.clone();
    *i += 1;
    match t.get(*i).map(|t| t.text.as_str()) {
        Some("=") => {
            *i += 1;
            let value = t.get(*i).map(|t| t.text.clone()).unwrap_or_default();
            *i += 1;
            match name.as_str() {
                "target_arch" => Some(value == "wasm32"),
                "target_family" => Some(value == "wasm"),
                "target_os" => Some(value == "unknown"),
                "panic" => None,
                _ => None,
            }
        }
        Some("(") => {
            *i += 1;
            let mut args = Vec::new();
            while t.get(*i).is_some_and(|t| t.text != ")") {
                args.push(eval_cfg(t, i));
                if t.get(*i).is_some_and(|t| t.text == ",") {
                    *i += 1;
                }
            }
            *i += 1;
            match name.as_str() {
                "not" => args.first().copied().flatten().map(|b| !b),
                "all" => {
                    if args.contains(&Some(false)) {
                        Some(false)
                    } else if args.contains(&None) {
                        None
                    } else {
                        Some(true)
                    }
                }
                "any" => {
                    if args.contains(&Some(true)) {
                        Some(true)
                    } else if args.contains(&None) {
                        None
                    } else {
                        Some(false)
                    }
                }
                _ => None,
            }
        }
        _ => match name.as_str() {
            "test" | "windows" | "unix" | "doc" | "doctest" | "miri" => Some(false),
            _ => None,
        },
    }
}

struct ModDecl {
    name: String,
    path: Option<String>,
    /// Inline `mod x { … }` blocks it is declared in, outermost first.
    inline: Vec<String>,
    excluded: bool,
}

struct Scan {
    file_excluded: bool,
    mods: Vec<ModDecl>,
    /// (line, what) of each std clock use the wasm build keeps.
    hits: Vec<(usize, &'static str)>,
}

/// A code region a cfg leaves out: from its attribute to the end of the item, statement, field
/// or block it is on.
struct Region {
    depth: usize,
    nest: usize,
    in_block: bool,
}

fn scan(src: &str) -> Scan {
    let t = tokens(src);
    let mut live: Vec<Tok> = Vec::new();
    let mut mods = Vec::new();
    let mut regions: Vec<Region> = Vec::new();
    let mut inline: Vec<(String, usize)> = Vec::new();
    let (mut depth, mut nest) = (0usize, 0usize);
    let mut pending_path: Option<String> = None;
    let mut i = 0;
    while i < t.len() {
        let tok = &t[i];
        // Attributes.
        if tok.text == "#" {
            let inner = t.get(i + 1).is_some_and(|t| t.text == "!");
            let open = if inner { i + 2 } else { i + 1 };
            if t.get(open).is_some_and(|t| t.text == "[") {
                let mut j = open + 1;
                let mut d = 1;
                while j < t.len() && d > 0 {
                    match t[j].text.as_str() {
                        "[" => d += 1,
                        "]" => d -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                let body = &t[open + 1..j.saturating_sub(1)];
                match body.first().map(|t| t.text.as_str()) {
                    Some("cfg") if regions.is_empty() && body.get(1).is_some_and(|t| t.text == "(") => {
                        let mut k = 2;
                        if eval_cfg(body, &mut k) == Some(false) {
                            if inner {
                                return Scan { file_excluded: true, mods: Vec::new(), hits: Vec::new() };
                            }
                            regions.push(Region { depth, nest, in_block: false });
                        }
                    }
                    Some("path") if !inner => pending_path = body.get(2).filter(|t| t.kind == Kind::Str).map(|t| t.text.clone()),
                    _ => {}
                }
                i = j;
                continue;
            }
        }
        let excluded = !regions.is_empty();
        // Module declarations.
        if tok.kind == Kind::Ident
            && tok.text == "mod"
            && let (Some(name), Some(next)) = (t.get(i + 1), t.get(i + 2))
        {
            if next.text == ";" {
                mods.push(ModDecl {
                    name: name.text.clone(),
                    path: pending_path.take(),
                    inline: inline.iter().map(|(n, _)| n.clone()).collect(),
                    excluded,
                });
            } else if next.text == "{" && !excluded {
                inline.push((name.text.clone(), depth));
            }
        }
        if !excluded {
            live.push(tok.clone());
        }
        match tok.text.as_str() {
            "{" => {
                if let Some(r) = regions.last_mut()
                    && !r.in_block
                    && r.depth == depth
                {
                    r.in_block = true;
                }
                depth += 1;
            }
            "}" => {
                depth = depth.saturating_sub(1);
                if inline.last().is_some_and(|(_, d)| *d == depth) {
                    inline.pop();
                }
                if let Some(r) = regions.last()
                    && (r.depth > depth || (r.in_block && r.depth == depth))
                {
                    regions.pop();
                }
            }
            "(" | "[" => nest += 1,
            ")" | "]" => nest = nest.saturating_sub(1),
            ";" | "," => {
                if let Some(r) = regions.last()
                    && !r.in_block
                    && r.depth == depth
                    && r.nest == nest
                {
                    regions.pop();
                }
                pending_path = None;
            }
            _ => {}
        }
        i += 1;
    }
    Scan { file_excluded: false, mods, hits: clock_uses(&live) }
}

/// The std clocks among the tokens a wasm build compiles.
fn clock_uses(t: &[Tok]) -> Vec<(usize, &'static str)> {
    let text = |i: usize| t.get(i).map_or("", |t| t.text.as_str());
    let mut hits = Vec::new();
    let mut std_system_time = false;
    // `use` statements rooted at std.
    let mut i = 0;
    while i < t.len() {
        if text(i) == "use" {
            let start = if text(i + 1) == "::" { i + 2 } else { i + 1 };
            let end = (i..t.len()).find(|&k| text(k) == ";").unwrap_or(t.len());
            if text(start) == "std" {
                let stmt = &t[start..end];
                if let Some(tok) = stmt.iter().find(|x| x.text == "Instant") {
                    hits.push((tok.line, "Instant imported from std::time"));
                }
                std_system_time |= stmt.iter().any(|x| x.text == "SystemTime");
            }
            i = end;
        }
        i += 1;
    }
    for (i, tok) in t.iter().enumerate() {
        let after_path = i > 0 && text(i - 1) == "::";
        if text(i) == "time" && tok.kind == Kind::Ident && text(i + 1) == "::" {
            // Not `web_time::…` (one identifier) and not some other crate's `time` module path.
            let rooted = !after_path || (i >= 2 && text(i - 2) == "std");
            if rooted && text(i + 2) == "Instant" && !hits.iter().any(|(l, _)| *l == tok.line) {
                hits.push((tok.line, "std::time::Instant"));
            }
            if rooted && text(i + 2) == "SystemTime" && text(i + 3) == "::" && text(i + 4) == "now" {
                hits.push((tok.line, "std::time::SystemTime::now"));
            }
        }
        if std_system_time && text(i) == "SystemTime" && !after_path && text(i + 1) == "::" && text(i + 2) == "now" {
            hits.push((tok.line, "SystemTime::now with SystemTime from std::time"));
        }
    }
    hits.sort();
    hits.dedup_by_key(|h| h.0);
    hits
}
