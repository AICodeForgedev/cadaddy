//! `cargo xtask merge-check <base> <head>`: verify a merge batch in the least time, with the same
//! coverage as "clippy the workspace, then test every crate the batch touched".
//!
//! 1. **What changed.** `git diff --name-only <base> <head>` mapped to workspace packages: a file
//!    inside a package's directory is that package; a workspace file (`Cargo.toml`, `Cargo.lock`,
//!    `clippy.toml`, `rust-toolchain*`, `.cargo/`, `.config/`) is every package; any other file
//!    (or a package file another package reads) is every package whose sources name it as
//!    `../<dir>` or `join("<dir>")` (tests that read `docs/`, `assets/`, another crate's sources).
//! 2. **What it can break.** Those packages plus every package that depends on them (normal and
//!    build dependencies, transitively), plus the packages that use one of them as a
//!    dev-dependency (their tests link it). The old rule "add ui-egui and cli when engine is
//!    touched" falls out of this; packages outside the set cannot change their clippy or test
//!    result, so clippy runs once for the set only.
//! 3. **Tests** with cargo-nextest (installed when missing; plain `cargo test` when it can't be):
//!    every test binary runs side by side instead of one after another. Tests whose recorded time
//!    (the last runs, kept in `<target>/merge-check/timings.json`) reaches `--slow-secs` are moved
//!    to a second stage (`--stage after`) that runs once the batch is merged; the two stages
//!    together run exactly the tests of the old single stage, doc-tests included (merge stage).
//!    Integration test files that serialize their tests with a `static ...: Mutex<()>` keep doing
//!    so (one test at a time per such binary), as they did inside one `cargo test` process.
//! 4. **Build cache.** One target directory kept warm between batches (third-party crates and
//!    untouched workspace crates are not rebuilt). A fresh worktree gives every file a new mtime,
//!    which would rebuild the whole workspace: files whose content is unchanged since the last
//!    run get their recorded mtime back, unless a build outside this tool touched their package
//!    since (then cargo decides from the new mtimes, as usual). After the run the directory is
//!    held under `--cap-gb`: first the incremental caches go, then (still over) the build outputs.
//!
//! Failure output is cargo's own (clippy diagnostics; each failing test's output and panic, as
//! nextest prints them), followed by a list of the failing tests with the command that re-runs
//! each one. Exit status is non-zero when any step fails. See docs/merge-check.md.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const USAGE: &str = "\
usage: cargo xtask merge-check <base> <head> [options]
  (light build, without the engine: cargo merge-check <base> <head> [options])

  <base> <head>         git revisions: the batch is the diff base..head; head must be checked out
  --stage merge|after|all
                        merge (default): clippy, tests except the slow ones, doc-tests;
                        after: only the slow tests (run it once the batch is merged); all: both
  --dry-run             print what changed, the packages and the commands; run nothing
  --root DIR            the worktree to check (default: this workspace)
  --target-dir DIR      the warm build cache (default: $CARGO_TARGET_DIR, else <root>/target)
  --cap-gb N            keep the build cache under N GB after the run (default 120)
  --slow-secs S         a test whose recorded time reaches S seconds runs in the after stage (default 60)
  --test-threads N      tests run side by side in the merge stage (default $CARGO_BUILD_JOBS, else 6)
  --after-threads N     tests run side by side in the after stage (default 2)
  --no-nextest          plain `cargo test` for everything (no after stage)
  --no-install          don't install cargo-nextest when it is missing (fall back to cargo test)
  --no-clippy           skip clippy
  --no-mtime-restore    leave file mtimes alone (cargo rebuilds what a fresh checkout touched)
  -p, --package NAME    also check NAME (repeatable)
  --all                 check every workspace package
";

/// The nextest profile this tool writes and uses.
const PROFILE: &str = "merge-check";
/// Default cap of the build cache, in GB.
const DEFAULT_CAP_GB: u64 = 120;
/// Default slow-test threshold, in seconds.
const DEFAULT_SLOW_SECS: f64 = 60.0;
/// How many recorded durations a test keeps (its slowness is the largest of them).
const TIMING_HISTORY: usize = 3;
/// Upper bound on the files hashed for the mtime restore (a hostile or broken tree).
const MAX_TRACKED_FILES: usize = 200_000;
/// Files larger than this are not hashed (their mtime is left alone).
const MAX_HASHED_BYTES: u64 = 64 * 1024 * 1024;
/// Upper bound on the number of tests recorded from one JUnit report.
const MAX_JUNIT_CASES: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Merge,
    After,
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opts {
    pub base: String,
    pub head: String,
    pub stage: Stage,
    pub dry_run: bool,
    pub root: Option<PathBuf>,
    pub target_dir: Option<PathBuf>,
    pub cap_gb: u64,
    pub slow_secs: f64,
    pub test_threads: Option<usize>,
    pub after_threads: usize,
    pub nextest: bool,
    pub install: bool,
    pub clippy: bool,
    pub mtime_restore: bool,
    pub packages: Vec<String>,
    pub all: bool,
}

fn positive<T: std::str::FromStr + PartialOrd + Default>(flag: &str, v: Option<&&str>) -> Result<T, String> {
    let v = v.ok_or_else(|| format!("{flag} needs a value"))?;
    match v.parse::<T>() {
        Ok(n) if n > T::default() => Ok(n),
        _ => Err(format!("{flag}: `{v}` is not a positive number")),
    }
}

pub fn parse_args(args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts {
        base: String::new(),
        head: String::new(),
        stage: Stage::Merge,
        dry_run: false,
        root: None,
        target_dir: None,
        cap_gb: DEFAULT_CAP_GB,
        slow_secs: DEFAULT_SLOW_SECS,
        test_threads: None,
        after_threads: 2,
        nextest: true,
        install: true,
        clippy: true,
        mtime_restore: true,
        packages: Vec::new(),
        all: false,
    };
    let mut revs = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--stage" => {
                o.stage = match it.next().copied() {
                    Some("merge") => Stage::Merge,
                    Some("after") => Stage::After,
                    Some("all") => Stage::All,
                    other => return Err(format!("--stage: expected merge, after or all, got {other:?}")),
                }
            }
            "--dry-run" => o.dry_run = true,
            "--root" => o.root = Some(PathBuf::from(it.next().ok_or("--root needs a directory")?)),
            "--target-dir" => o.target_dir = Some(PathBuf::from(it.next().ok_or("--target-dir needs a directory")?)),
            "--cap-gb" => o.cap_gb = positive("--cap-gb", it.next())?,
            "--slow-secs" => {
                o.slow_secs = positive("--slow-secs", it.next())?;
                if !o.slow_secs.is_finite() {
                    return Err("--slow-secs must be finite".into());
                }
            }
            "--test-threads" => o.test_threads = Some(positive("--test-threads", it.next())?),
            "--after-threads" => o.after_threads = positive("--after-threads", it.next())?,
            "--no-nextest" => o.nextest = false,
            "--no-install" => o.install = false,
            "--no-clippy" => o.clippy = false,
            "--no-mtime-restore" => o.mtime_restore = false,
            "-p" | "--package" => o.packages.push((*it.next().ok_or("--package needs a name")?).to_string()),
            "--all" => o.all = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            f if f.starts_with('-') => return Err(format!("unknown option {f}\n\n{USAGE}")),
            rev => revs.push(rev.to_string()),
        }
    }
    match <[String; 2]>::try_from(revs) {
        Ok([base, head]) => {
            o.base = base;
            o.head = head;
            Ok(o)
        }
        Err(_) => Err(format!("expected exactly two revisions, <base> <head>\n\n{USAGE}")),
    }
}

// ---------------------------------------------------------------------------------------------
// The workspace model.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub name: String,
    /// Directory relative to the workspace root, `/`-separated, no trailing slash.
    pub dir: String,
    /// Workspace dependencies only.
    pub deps: Vec<(String, Kind)>,
    /// Has a library target (doc-tests run for these).
    pub has_lib: bool,
    /// Integration test targets: (target name, source path relative to the root).
    pub test_targets: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workspace {
    pub packages: Vec<Package>,
}

fn rel_slash(path: &str, root: &str) -> String {
    let p = path.replace('\\', "/");
    let r = root.replace('\\', "/");
    let r = r.trim_end_matches('/');
    match p.strip_prefix(r) {
        Some(rest) => rest.trim_start_matches('/').to_string(),
        None => p,
    }
}

/// The model from `cargo metadata --format-version 1 --no-deps`.
pub fn workspace_from_metadata(meta: &Value) -> Result<Workspace, String> {
    let root = meta["workspace_root"].as_str().ok_or("metadata: no workspace_root")?;
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let names: BTreeSet<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("metadata: package without a name")?.to_string();
        let manifest = p["manifest_path"].as_str().ok_or("metadata: package without manifest_path")?;
        let manifest = rel_slash(manifest, root);
        let dir = match manifest.rfind('/') {
            Some(i) => manifest.get(..i).unwrap_or_default().to_string(),
            None => String::new(),
        };
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let Some(dn) = d["name"].as_str() else { continue };
            if !names.contains(dn) || dn == name {
                continue;
            }
            let kind = match d["kind"].as_str() {
                Some("dev") => Kind::Dev,
                Some("build") => Kind::Build,
                _ => Kind::Normal,
            };
            deps.push((dn.to_string(), kind));
        }
        let mut has_lib = false;
        let mut test_targets = Vec::new();
        for t in p["targets"].as_array().into_iter().flatten() {
            let kinds: Vec<&str> = t["kind"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if kinds.iter().any(|k| matches!(*k, "lib" | "rlib" | "dylib" | "proc-macro")) {
                has_lib = true;
            }
            if kinds.contains(&"test")
                && let (Some(tn), Some(src)) = (t["name"].as_str(), t["src_path"].as_str())
            {
                test_targets.push((tn.to_string(), rel_slash(src, root)));
            }
        }
        out.push(Package { name, dir, deps, has_lib, test_targets });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Workspace { packages: out })
}

impl Workspace {
    fn get(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// The package whose directory holds `path` (the deepest one).
    pub fn owner(&self, path: &str) -> Option<&Package> {
        self.packages.iter().filter(|p| !p.dir.is_empty() && (path == p.dir || path.starts_with(&format!("{}/", p.dir)))).max_by_key(|p| p.dir.len())
    }

    pub fn names(&self) -> BTreeSet<String> {
        self.packages.iter().map(|p| p.name.clone()).collect()
    }
}

/// Files that change how every package builds or is linted.
pub fn is_workspace_file(path: &str) -> bool {
    matches!(path, "Cargo.toml" | "Cargo.lock" | "clippy.toml" | ".clippy.toml" | "rust-toolchain" | "rust-toolchain.toml")
        || path.starts_with(".cargo/")
        || path.starts_with(".config/")
}

/// The names under which other packages' sources may refer to `path`: its first directory
/// (`docs`, `assets`, a top-level file), and for a file inside a package, that package's
/// directory name (`ui-egui` for `crates/ui-egui/src/x.rs`).
pub fn reference_keys(ws: &Workspace, path: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if let Some(owner) = ws.owner(path) {
        if let Some(last) = owner.dir.rsplit('/').next() {
            keys.insert(last.to_string());
        }
    } else {
        let mut parts = path.split('/');
        let first = parts.next().unwrap_or_default();
        keys.insert(first.to_string());
        // A file under crates/<x>/ or apps/<x>/ of a package that no longer exists.
        if ws.packages.iter().any(|p| p.dir.starts_with(&format!("{first}/")))
            && let Some(second) = parts.next()
        {
            keys.insert(second.to_string());
        }
    }
    keys.retain(|k| !k.is_empty() && k != "." && k != "..");
    keys
}

fn path_word(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or(s.len());
    s.get(..end).unwrap_or_default()
}

/// The directory or file names a source text reaches outside its own directory: every
/// `../<name>` and every `join("<name>...")`.
pub fn ref_tokens(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, _) in text.match_indices("../") {
        let w = path_word(text.get(i + 3..).unwrap_or_default());
        if !w.is_empty() && w != "." && w != ".." {
            out.insert(w.to_string());
        }
    }
    for (i, _) in text.match_indices("join(\"") {
        let w = path_word(text.get(i + 6..).unwrap_or_default());
        if !w.is_empty() && w != "." && w != ".." {
            out.insert(w.to_string());
        }
    }
    out
}

/// Does this test source serialize its tests with a process-wide lock?
pub fn uses_serial_lock(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("static ") && l.contains("Mutex<()>")
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Affected {
    /// Every package (a workspace file changed, or `--all`).
    pub all: bool,
    pub packages: BTreeSet<String>,
    /// Why each package is in the set (the first reason found).
    pub reasons: BTreeMap<String, String>,
    /// Changed files no package builds or reads.
    pub unmapped: Vec<String>,
}

/// The packages a batch can affect. `tokens` holds each package's [`ref_tokens`].
pub fn affected(ws: &Workspace, changed: &[String], tokens: &BTreeMap<String, BTreeSet<String>>) -> Affected {
    let mut a = Affected::default();
    let mut seeds: BTreeMap<String, String> = BTreeMap::new();
    for f in changed {
        if is_workspace_file(f) {
            if !a.all {
                a.all = true;
                for p in &ws.packages {
                    a.reasons.entry(p.name.clone()).or_insert_with(|| format!("workspace file {f}"));
                }
            }
            continue;
        }
        let owner = ws.owner(f).map(|p| p.name.clone());
        let mut mapped = false;
        if let Some(o) = &owner {
            seeds.entry(o.clone()).or_insert_with(|| format!("changed {f}"));
            mapped = true;
        }
        for key in reference_keys(ws, f) {
            for (pkg, toks) in tokens {
                if Some(pkg) != owner.as_ref() && toks.contains(&key) {
                    seeds.entry(pkg.clone()).or_insert_with(|| format!("reads ../{key} ({f})"));
                    mapped = true;
                }
            }
        }
        if !mapped {
            a.unmapped.push(f.clone());
        }
    }
    if a.all {
        a.packages = ws.names();
        return a;
    }
    // Reverse edges: who depends on whom (normal/build) and who uses whom in tests (dev).
    let mut users: BTreeMap<&str, Vec<(&str, Kind)>> = BTreeMap::new();
    for p in &ws.packages {
        for (d, k) in &p.deps {
            users.entry(d.as_str()).or_default().push((p.name.as_str(), *k));
        }
    }
    let mut lib: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for (p, why) in &seeds {
        if ws.get(p).is_some() && lib.insert(p.clone()) {
            a.reasons.insert(p.clone(), why.clone());
            queue.push_back(p.clone());
        }
    }
    while let Some(p) = queue.pop_front() {
        for (u, k) in users.get(p.as_str()).into_iter().flatten() {
            if *k != Kind::Dev && lib.insert((*u).to_string()) {
                a.reasons.entry((*u).to_string()).or_insert_with(|| format!("depends on {p}"));
                queue.push_back((*u).to_string());
            }
        }
    }
    let mut set = lib.clone();
    for p in &lib {
        for (u, k) in users.get(p.as_str()).into_iter().flatten() {
            if *k == Kind::Dev && set.insert((*u).to_string()) {
                a.reasons.entry((*u).to_string()).or_insert_with(|| format!("tests use {p}"));
            }
        }
    }
    a.packages = set;
    a
}

// ---------------------------------------------------------------------------------------------
// Timing data and the slow-test filter.

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub binary_id: String,
    pub name: String,
    pub secs: f64,
    pub failed: bool,
}

fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let tail = rest.get(i..).unwrap_or_default();
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = tail.get(1..end).unwrap_or_default();
        let ch = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e => {
                let n = if let Some(h) = e.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()
                } else {
                    e.strip_prefix('#').and_then(|d| d.parse().ok())
                };
                n.and_then(char::from_u32)
            }
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(tail.get(..=end).unwrap_or_default()),
        }
        rest = tail.get(end + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = tag.get(from..).and_then(|t| t.find(&format!("{name}=\""))) {
        let at = from + i;
        let before_ok = at == 0 || tag.get(..at).and_then(|b| b.chars().last()).is_some_and(char::is_whitespace);
        let vstart = at + name.len() + 2;
        if before_ok {
            let v = tag.get(vstart..)?;
            let end = v.find('"')?;
            return Some(xml_unescape(v.get(..end)?));
        }
        from = vstart;
    }
    None
}

/// The test cases of a nextest JUnit report.
pub fn parse_junit(xml: &str) -> Vec<Case> {
    let mut out = Vec::new();
    let mut suite = String::new();
    let mut pos = 0;
    while out.len() < MAX_JUNIT_CASES {
        let rest = xml.get(pos..).unwrap_or_default();
        let next_suite = rest.find("<testsuite ");
        let next_case = rest.find("<testcase ");
        let (at, is_suite) = match (next_suite, next_case) {
            (Some(s), Some(c)) if s < c => (s, true),
            (Some(s), None) => (s, true),
            (_, Some(c)) => (c, false),
            (None, None) => break,
        };
        let tag_start = pos + at;
        let Some(tag_len) = xml.get(tag_start..).and_then(|t| t.find('>')) else { break };
        let tag = xml.get(tag_start..tag_start + tag_len).unwrap_or_default();
        pos = tag_start + tag_len + 1;
        if is_suite {
            suite = xml_attr(tag, "name").unwrap_or_default();
            continue;
        }
        let name = xml_attr(tag, "name").unwrap_or_default();
        let binary_id = xml_attr(tag, "classname").filter(|c| !c.is_empty()).unwrap_or_else(|| suite.clone());
        let secs = xml_attr(tag, "time").and_then(|t| t.parse::<f64>().ok()).filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(0.0);
        let mut failed = false;
        if !tag.ends_with('/') {
            let body_end = xml.get(pos..).and_then(|t| t.find("</testcase>")).map(|e| pos + e).unwrap_or(xml.len());
            let body = xml.get(pos..body_end).unwrap_or_default();
            failed = body.contains("<failure") || body.contains("<error");
            pos = body_end;
        }
        if !name.is_empty() {
            out.push(Case { binary_id, name, secs, failed });
        }
    }
    out
}

/// Recorded test durations: `"<binary id> <test name>"` -> the last few runs, newest last.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timings {
    pub tests: BTreeMap<String, Vec<f64>>,
}

impl Timings {
    pub fn from_json(v: &Value) -> Timings {
        let mut t = Timings::default();
        for (k, arr) in v["tests"].as_object().into_iter().flatten() {
            let secs: Vec<f64> = arr.as_array().into_iter().flatten().filter_map(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0).collect();
            if !secs.is_empty() {
                t.tests.insert(k.clone(), secs);
            }
        }
        t
    }

    pub fn to_json(&self) -> Value {
        json!({ "version": 1, "tests": self.tests })
    }

    /// Add one run's results (failed tests too: a test that times out slowly is still slow).
    pub fn record(&mut self, cases: &[Case]) {
        for c in cases {
            let v = self.tests.entry(format!("{} {}", c.binary_id, c.name)).or_default();
            v.push(c.secs);
            if v.len() > TIMING_HISTORY {
                let drop = v.len() - TIMING_HISTORY;
                v.drain(..drop);
            }
        }
    }

    /// Tests whose slowest recorded run reaches `slow_secs`, in the given packages:
    /// (binary id, test name), sorted.
    pub fn slow(&self, packages: &BTreeSet<String>, all: bool, slow_secs: f64) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (k, secs) in &self.tests {
            let Some((bin, name)) = k.split_once(' ') else { continue };
            let pkg = bin.split("::").next().unwrap_or(bin);
            if !all && !packages.contains(pkg) {
                continue;
            }
            if secs.iter().copied().fold(0.0_f64, f64::max) >= slow_secs {
                out.push((bin.to_string(), name.to_string()));
            }
        }
        out
    }
}

/// A string inside a nextest filterset matcher: `(`, `)`, `,` and `\` are escaped.
pub fn filter_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '(' | ')' | ',' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The filterset matching exactly the given tests (`None` for no tests).
pub fn slow_filter(slow: &[(String, String)]) -> Option<String> {
    if slow.is_empty() {
        return None;
    }
    let parts: Vec<String> = slow.iter().map(|(b, n)| format!("(binary_id(={}) & test(={}))", filter_escape(b), filter_escape(n))).collect();
    Some(parts.join(" | "))
}

/// The nextest configuration this tool runs with: its profile (JUnit report for the timing data,
/// failures printed as they happen and again at the end) and one-at-a-time groups for the test
/// binaries that serialize their tests with a lock.
pub fn nextest_config(serial_binaries: &[String]) -> String {
    let mut s = String::new();
    s.push_str("# Written by `cargo xtask merge-check`; regenerated each run.\n");
    s.push_str(&format!("[profile.{PROFILE}]\n"));
    s.push_str("fail-fast = false\nfailure-output = \"immediate-final\"\nsuccess-output = \"never\"\nstatus-level = \"slow\"\nfinal-status-level = \"fail\"\nslow-timeout = { period = \"60s\" }\n\n");
    s.push_str(&format!("[profile.{PROFILE}.junit]\npath = \"junit.xml\"\nstore-success-output = false\nstore-failure-output = true\n\n"));
    if !serial_binaries.is_empty() {
        s.push_str("[test-groups]\n");
        for i in 0..serial_binaries.len() {
            s.push_str(&format!("serial-{i} = {{ max-threads = 1 }}\n"));
        }
        s.push('\n');
        for (i, b) in serial_binaries.iter().enumerate() {
            let filter = format!("binary_id(={})", filter_escape(b));
            s.push_str(&format!("[[profile.{PROFILE}.overrides]]\nfilter = {}\ntest-group = \"serial-{i}\"\n\n", toml_str(&filter)));
        }
    }
    s
}

fn toml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------------------------
// Commands.

fn package_args(a: &Affected) -> Vec<String> {
    if a.all {
        return vec!["--workspace".into()];
    }
    a.packages.iter().flat_map(|p| ["-p".to_string(), p.clone()]).collect()
}

pub fn clippy_args(a: &Affected) -> Vec<String> {
    let mut v = vec!["clippy".to_string()];
    v.extend(package_args(a));
    v.extend(["--all-targets", "--", "-D", "warnings"].map(String::from));
    v
}

pub fn nextest_args(a: &Affected, config: &Path, threads: usize, filter: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = ["nextest", "run", "--no-fail-fast", "--no-tests=warn", "--profile", PROFILE].map(String::from).to_vec();
    v.push("--config-file".into());
    v.push(config.to_string_lossy().into_owned());
    v.push("--test-threads".into());
    v.push(threads.to_string());
    v.extend(package_args(a));
    if let Some(f) = filter {
        v.push("-E".into());
        v.push(f.to_string());
    }
    v
}

pub fn doctest_args(ws: &Workspace, a: &Affected) -> Option<Vec<String>> {
    let libs: Vec<&Package> = ws.packages.iter().filter(|p| p.has_lib && a.packages.contains(&p.name)).collect();
    if libs.is_empty() {
        return None;
    }
    let mut v: Vec<String> = ["test", "--doc", "--no-fail-fast"].map(String::from).to_vec();
    for p in libs {
        v.push("-p".into());
        v.push(p.name.clone());
    }
    Some(v)
}

pub fn cargo_test_args(a: &Affected) -> Vec<String> {
    let mut v: Vec<String> = ["test", "--no-fail-fast"].map(String::from).to_vec();
    v.extend(package_args(a));
    v
}

/// The command that re-runs one failing test with plain cargo.
pub fn rerun_hint(binary_id: &str, name: &str) -> String {
    let (pkg, bin) = match binary_id.split_once("::") {
        Some((p, b)) => (p, Some(b)),
        None => (binary_id, None),
    };
    let target = match bin {
        None => "--lib".to_string(),
        Some(b) => match b.split_once('/') {
            Some(("bin", n)) => format!("--bin {n}"),
            Some((kind, n)) => format!("--{kind} {n}"),
            None => format!("--test {b}"),
        },
    };
    format!("cargo test -p {pkg} {target} -- --exact {name}")
}

fn shell_line(args: &[String]) -> String {
    let mut s = String::from("cargo");
    for a in args {
        s.push(' ');
        if a.is_empty() || a.chars().any(|c| c.is_whitespace() || "'\"()|&;<>*$\\".contains(c)) {
            s.push('\'');
            s.push_str(&a.replace('\'', "'\\''"));
            s.push('\'');
        } else {
            s.push_str(a);
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Build cache: size cap and mtime restore.

/// What to delete to bring the cache under the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trim {
    Nothing,
    Incremental,
    Outputs,
}

pub fn trim_plan(total_bytes: u64, incremental_bytes: u64, cap_bytes: u64) -> Trim {
    if total_bytes <= cap_bytes {
        Trim::Nothing
    } else if total_bytes.saturating_sub(incremental_bytes) <= cap_bytes {
        Trim::Incremental
    } else {
        Trim::Outputs
    }
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                total = total.saturating_add(e.metadata().map(|m| m.len()).unwrap_or(0));
            }
        }
    }
    total
}

/// Workspace package names whose units (any profile, any mode) were built after `since`:
/// the fingerprint directories `<target>/<profile>/.fingerprint/<package>-<hash>/`.
pub fn built_since(target: &Path, packages: &BTreeSet<String>, since: SystemTime) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(profiles) = std::fs::read_dir(target) else { return out };
    for prof in profiles.flatten() {
        let fp = prof.path().join(".fingerprint");
        let Ok(units) = std::fs::read_dir(&fp) else { continue };
        for u in units.flatten() {
            let unit = u.file_name().to_string_lossy().into_owned();
            let Some(pkg) = unit_package(&unit) else { continue };
            if !packages.contains(pkg) || out.contains(pkg) {
                continue;
            }
            if newest_mtime(&u.path()).is_some_and(|t| t > since) {
                out.insert(pkg.to_string());
            }
        }
    }
    out
}

/// `cadaddy-engine-0123456789abcdef` -> `cadaddy-engine`.
pub fn unit_package(unit: &str) -> Option<&str> {
    let (name, hash) = unit.rsplit_once('-')?;
    (hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some(name)
}

fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    }
    newest
}

fn newest_fingerprint(target: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    for prof in std::fs::read_dir(target).ok()?.flatten() {
        let Ok(units) = std::fs::read_dir(prof.path().join(".fingerprint")) else { continue };
        for u in units.flatten() {
            if let Some(t) = newest_mtime(&u.path()) {
                newest = Some(newest.map_or(t, |n| n.max(t)));
            }
        }
    }
    newest
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stamp {
    pub secs: u64,
    pub nanos: u32,
}

impl Stamp {
    fn of(t: SystemTime) -> Stamp {
        let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        Stamp { secs: d.as_secs(), nanos: d.subsec_nanos() }
    }

    fn time(self) -> SystemTime {
        SystemTime::UNIX_EPOCH.checked_add(Duration::new(self.secs, self.nanos.min(999_999_999))).unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

/// The content and mtime of every tracked file at the last run, and when that run's build ended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MtimeManifest {
    pub root: String,
    /// Newest fingerprint in the cache when the last run ended.
    pub built: Option<Stamp>,
    pub files: BTreeMap<String, (String, Stamp)>,
}

impl MtimeManifest {
    pub fn from_json(v: &Value) -> MtimeManifest {
        let stamp = |a: &Value| -> Option<Stamp> {
            let secs = a.get(0)?.as_u64()?;
            let nanos = u32::try_from(a.get(1)?.as_u64()?).ok()?;
            Some(Stamp { secs, nanos })
        };
        let mut m = MtimeManifest { root: v["root"].as_str().unwrap_or_default().to_string(), built: stamp(&v["built"]), files: BTreeMap::new() };
        for (k, e) in v["files"].as_object().into_iter().flatten() {
            if let (Some(h), Some(s)) = (e.get(0).and_then(Value::as_str), e.get(1).and_then(stamp)) {
                m.files.insert(k.clone(), (h.to_string(), s));
            }
        }
        m
    }

    pub fn to_json(&self) -> Value {
        let files: serde_json::Map<String, Value> = self.files.iter().map(|(k, (h, s))| (k.clone(), json!([h, [s.secs, s.nanos]]))).collect();
        json!({ "version": 1, "root": self.root, "built": self.built.map(|s| json!([s.secs, s.nanos])), "files": files })
    }
}

/// Which files get their recorded mtime back: content unchanged since the last run, mtime
/// different, and no build since the last run touched the file's package (`dirty`; `None` owner =
/// outside every package, restored only when nothing at all was built since).
pub fn restore_plan(
    manifest: &MtimeManifest,
    current: &BTreeMap<String, (String, Stamp)>,
    owner: &dyn Fn(&str) -> Option<String>,
    dirty: &BTreeSet<String>,
) -> Vec<(String, Stamp)> {
    let mut out = Vec::new();
    for (path, (hash, now)) in current {
        let Some((old_hash, old)) = manifest.files.get(path) else { continue };
        if old_hash != hash || old == now {
            continue;
        }
        let clean = match owner(path) {
            Some(p) => !dirty.contains(&p),
            None => dirty.is_empty(),
        };
        if clean {
            out.push((path.clone(), *old));
        }
    }
    out
}

fn sha256_file(p: &Path) -> Option<String> {
    let md = std::fs::metadata(p).ok()?;
    if !md.is_file() || md.len() > MAX_HASHED_BYTES {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    let digest = Sha256::digest(&bytes);
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn tracked_files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git").current_dir(root).args(["ls-files", "-z"]).output().map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).split('\0').filter(|s| !s.is_empty()).take(MAX_TRACKED_FILES).map(str::to_string).collect())
}

fn current_state(root: &Path, files: &[String]) -> BTreeMap<String, (String, Stamp)> {
    let mut out = BTreeMap::new();
    for f in files {
        let p = root.join(f);
        let Some(h) = sha256_file(&p) else { continue };
        let Ok(t) = std::fs::metadata(&p).and_then(|m| m.modified()) else { continue };
        out.insert(f.clone(), (h, Stamp::of(t)));
    }
    out
}

fn set_mtime(p: &Path, t: SystemTime) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().write(true).open(p)?;
    f.set_times(std::fs::FileTimes::new().set_modified(t))
}

// ---------------------------------------------------------------------------------------------
// Running.

struct Step {
    name: String,
    secs: f64,
    ok: bool,
    /// Process exit code (`None`: killed by a signal or not started).
    code: Option<i32>,
    note: String,
}

struct Runner {
    root: PathBuf,
    target: PathBuf,
    steps: Vec<Step>,
}

impl Runner {
    fn cargo(&self, args: &[String]) -> Command {
        let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        c.current_dir(&self.root).args(args).env("CARGO_TARGET_DIR", &self.target);
        for var in ["CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG"] {
            if std::env::var_os(var).is_none() {
                c.env(var, "0");
            }
        }
        c
    }

    /// Run a cargo command, inheriting the terminal; record its time and result.
    fn step(&mut self, name: &str, args: &[String]) -> bool {
        eprintln!("\n=== merge-check: {name}\n$ {}", shell_line(args));
        let t = Instant::now();
        let (ok, code) = match self.cargo(args).status() {
            Ok(s) => (s.success(), s.code()),
            Err(e) => {
                eprintln!("merge-check: could not start cargo: {e}");
                (false, None)
            }
        };
        let secs = t.elapsed().as_secs_f64();
        eprintln!("=== merge-check: {name}: {} in {}", if ok { "ok" } else { "FAILED" }, fmt_secs(secs));
        self.steps.push(Step { name: name.to_string(), secs, ok, code, note: String::new() });
        ok
    }
}

fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s >= 60 { format!("{}m {:02}s", s / 60, s % 60) } else { format!("{s}s") }
}

fn git_lines(root: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let out = Command::new("git").current_dir(root).args(args).output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

fn metadata(root: &Path) -> Result<Value, String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                let name = e.file_name();
                if name != "target" && name != ".git" && name != "node_modules" {
                    stack.push(p);
                }
            } else if ft.is_file() && p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
}

fn package_tokens(root: &Path, ws: &Workspace) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for p in &ws.packages {
        let mut files = Vec::new();
        rs_files(&root.join(&p.dir), &mut files);
        let mut toks = BTreeSet::new();
        for f in files {
            if let Ok(text) = std::fs::read_to_string(&f) {
                toks.extend(ref_tokens(&text));
            }
        }
        out.insert(p.name.clone(), toks);
    }
    out
}

fn serial_binaries(root: &Path, ws: &Workspace, a: &Affected) -> Vec<String> {
    let mut out = Vec::new();
    for p in ws.packages.iter().filter(|p| a.packages.contains(&p.name)) {
        for (t, src) in &p.test_targets {
            if std::fs::read_to_string(root.join(src)).is_ok_and(|s| uses_serial_lock(&s)) {
                out.push(format!("{}::{t}", p.name));
            }
        }
    }
    out
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn write_json(p: &Path, v: &Value) {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    if let Err(e) = std::fs::write(p, text) {
        eprintln!("merge-check: could not write {}: {e}", p.display());
    }
}

fn nextest_available(r: &Runner) -> bool {
    r.cargo(&["nextest".into(), "--version".into()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn ensure_nextest(r: &mut Runner, install: bool) -> bool {
    if nextest_available(r) {
        return true;
    }
    if !install {
        eprintln!("merge-check: cargo-nextest is not installed (--no-install): falling back to cargo test");
        return false;
    }
    let ok = r.step("install cargo-nextest", &["install".into(), "cargo-nextest".into(), "--locked".into()]);
    if ok && nextest_available(r) {
        true
    } else {
        eprintln!("merge-check: cargo-nextest could not be installed: falling back to cargo test");
        false
    }
}

/// Run one nextest pass; record its timings; return the failing tests.
fn nextest_pass(r: &mut Runner, name: &str, args: &[String], timings: &mut Timings, timings_path: &Path) -> (bool, Vec<Case>) {
    let junit = r.target.join("nextest").join(PROFILE).join("junit.xml");
    let _ = std::fs::remove_file(&junit);
    let ok = r.step(name, args);
    let cases = std::fs::read_to_string(&junit).map(|x| parse_junit(&x)).unwrap_or_default();
    if !cases.is_empty() {
        timings.record(&cases);
        write_json(timings_path, &timings.to_json());
    }
    (ok, cases.into_iter().filter(|c| c.failed).collect())
}

/// nextest's exit code when tests ran and some failed (anything else: build or setup failure).
const NEXTEST_TESTS_FAILED: i32 = 100;
/// More failures than this in one pass are a broken batch, not tests disturbed by load.
const MAX_RERUN_ALONE: usize = 50;

/// Re-run a pass's failures alone? Only when tests ran and a few failed (not a build error, not a
/// broken batch).
pub fn rerun_alone(ok: bool, code: Option<i32>, failed: usize) -> bool {
    !ok && code == Some(NEXTEST_TESTS_FAILED) && (1..=MAX_RERUN_ALONE).contains(&failed)
}

/// One nextest pass with `threads` tests side by side; the tests that fail are re-run alone, one
/// at a time (timing and kill-a-child tests can fail while a dozen other test processes and other
/// builds share the machine; the old single `cargo test` ran one binary at a time). A failure
/// that repeats alone is a failure; one that passes alone is reported and the pass counts as
/// green. Returns the failing tests.
#[allow(clippy::too_many_arguments)]
fn tests_with_rerun(
    r: &mut Runner,
    name: &str,
    a: &Affected,
    config: &Path,
    threads: usize,
    filter: Option<&str>,
    timings: &mut Timings,
    timings_path: &Path,
) -> Vec<Case> {
    let (ok, failed) = nextest_pass(r, name, &nextest_args(a, config, threads, filter), timings, timings_path);
    let first = r.steps.len().saturating_sub(1);
    let code = r.steps.get(first).and_then(|s| s.code);
    if !rerun_alone(ok, code, failed.len()) {
        return failed;
    }
    let pairs: Vec<(String, String)> = failed.iter().map(|c| (c.binary_id.clone(), c.name.clone())).collect();
    let Some(only) = slow_filter(&pairs) else { return failed };
    eprintln!(
        "
{} test(s) failed with {threads} side by side; re-running them alone",
        failed.len()
    );
    let (again_ok, again) =
        nextest_pass(r, &format!("{name}: failures re-run alone"), &nextest_args(a, config, 1, Some(&only)), timings, timings_path);
    if again_ok {
        let names: Vec<String> = failed.iter().map(|c| format!("{} {}", c.binary_id, c.name)).collect();
        eprintln!("passed alone (failed only under load): {}", names.join(", "));
        if let Some(s) = r.steps.get_mut(first) {
            s.ok = true;
            s.note = format!("{} failed under load, passed alone", failed.len());
        }
        Vec::new()
    } else {
        again
    }
}

/// A canonical path without Windows' `\\?\` verbatim prefix (cargo and git take it, people
/// don't read it well).
fn plain(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

pub fn run(default_root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(args)?;
    let root = o.root.clone().unwrap_or_else(|| default_root.to_path_buf());
    let root = plain(root.canonicalize().map_err(|e| format!("--root {}: {e}", root.display()))?);
    let target = o.target_dir.clone().or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from)).unwrap_or_else(|| root.join("target"));
    std::fs::create_dir_all(&target).map_err(|e| format!("target dir {}: {e}", target.display()))?;
    let target = plain(target.canonicalize().map_err(|e| format!("target dir {}: {e}", target.display()))?);
    let state_dir = target.join("merge-check");
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("{}: {e}", state_dir.display()))?;

    // The checked-out tree must be the head being verified.
    let head_sha = git_lines(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", o.head)])?;
    let checked_out = git_lines(&root, &["rev-parse", "HEAD"])?;
    if head_sha.first() != checked_out.first() {
        return Err(format!(
            "{} is not checked out in {} (HEAD is {}); check it out first",
            o.head,
            root.display(),
            checked_out.first().map_or("?", String::as_str)
        ));
    }
    let changed = git_lines(&root, &["diff", "--name-only", "--no-renames", &o.base, &o.head])?;

    let ws = workspace_from_metadata(&metadata(&root)?)?;
    let tokens = package_tokens(&root, &ws);
    let mut a = affected(&ws, &changed, &tokens);
    if o.all {
        a.all = true;
        a.packages = ws.names();
    }
    for p in &o.packages {
        if ws.get(p).is_none() {
            return Err(format!("--package {p}: no such workspace package"));
        }
        if a.packages.insert(p.clone()) {
            a.reasons.insert(p.clone(), "--package".into());
        }
    }

    eprintln!("merge-check {}..{}: {} changed file(s)", o.base, o.head, changed.len());
    if a.packages.is_empty() {
        eprintln!("no workspace package is affected (only {} non-code file(s) changed); nothing to check", a.unmapped.len());
        return Ok(());
    }
    if a.all {
        eprintln!("every package ({}): {}", a.packages.len(), a.reasons.values().next().map_or("--all", String::as_str));
    } else {
        eprintln!("{} package(s):", a.packages.len());
        for p in &a.packages {
            eprintln!("  {p:<24} {}", a.reasons.get(p).map_or("", String::as_str));
        }
    }
    if !a.unmapped.is_empty() {
        let shown: Vec<&str> = a.unmapped.iter().take(8).map(String::as_str).collect();
        eprintln!("not read by any package: {}{}", shown.join(", "), if a.unmapped.len() > 8 { ", ..." } else { "" });
    }

    let timings_path = state_dir.join("timings.json");
    let mut timings = read_json(&timings_path).map(|v| Timings::from_json(&v)).unwrap_or_default();
    let slow = timings.slow(&a.packages, a.all, o.slow_secs);
    let filter = slow_filter(&slow);
    let threads =
        o.test_threads.or_else(|| std::env::var("CARGO_BUILD_JOBS").ok().and_then(|j| j.parse().ok()).filter(|j: &usize| *j > 0)).unwrap_or(6);
    let config = state_dir.join("nextest.toml");
    let serial = serial_binaries(&root, &ws, &a);
    let run_merge = matches!(o.stage, Stage::Merge | Stage::All);
    let run_after = matches!(o.stage, Stage::After | Stage::All);

    let mut r = Runner { root: root.clone(), target: target.clone(), steps: Vec::new() };
    if o.dry_run {
        eprintln!("\nbuild cache: {}", target.display());
        eprintln!("{} test(s) recorded at >= {}s run in the after stage", slow.len(), o.slow_secs);
        for (b, n) in &slow {
            eprintln!("  {b} {n}");
        }
        if run_merge && o.clippy {
            eprintln!("$ {}", shell_line(&clippy_args(&a)));
        }
        let merge_filter = filter.as_ref().map(|f| format!("not ({f})"));
        if o.nextest {
            if run_merge {
                eprintln!("$ {}", shell_line(&nextest_args(&a, &config, threads, merge_filter.as_deref())));
                if let Some(d) = doctest_args(&ws, &a) {
                    eprintln!("$ {}", shell_line(&d));
                }
            }
            if run_after && let Some(f) = &filter {
                eprintln!("$ {}", shell_line(&nextest_args(&a, &config, o.after_threads, Some(f))));
            }
        } else if run_merge {
            eprintln!("$ {}", shell_line(&cargo_test_args(&a)));
        }
        if !serial.is_empty() {
            eprintln!("one test at a time in: {}", serial.join(", "));
        }
        return Ok(());
    }

    // Give unchanged files their recorded mtimes back (see the module docs).
    let manifest_path = state_dir.join("mtimes.json");
    let mut current = BTreeMap::new();
    let root_str = root.to_string_lossy().into_owned();
    if o.mtime_restore {
        match tracked_files(&root) {
            Ok(files) => {
                current = current_state(&root, &files);
                let manifest = read_json(&manifest_path).map(|v| MtimeManifest::from_json(&v)).unwrap_or_default();
                if manifest.root == root_str
                    && let Some(built) = manifest.built
                {
                    let dirty = built_since(&target, &ws.names(), built.time());
                    let owner = |p: &str| ws.owner(p).map(|q| q.name.clone());
                    let plan = restore_plan(&manifest, &current, &owner, &dirty);
                    let mut restored = 0usize;
                    for (path, stamp) in &plan {
                        if set_mtime(&root.join(path), stamp.time()).is_ok() {
                            restored += 1;
                            current.insert(path.clone(), (current.get(path).map(|c| c.0.clone()).unwrap_or_default(), *stamp));
                        }
                    }
                    eprintln!(
                        "mtimes: {restored} unchanged file(s) restored{}",
                        if dirty.is_empty() {
                            String::new()
                        } else {
                            format!(" (built outside merge-check since the last run: {})", dirty.iter().cloned().collect::<Vec<_>>().join(", "))
                        }
                    );
                } else {
                    eprintln!("mtimes: no record for this worktree yet (first run): cargo rebuilds what the checkout touched");
                }
            }
            Err(e) => eprintln!("mtimes: not restored ({e})"),
        }
    }

    let started = Instant::now();
    let mut failed_tests: Vec<Case> = Vec::new();
    let mut fallback = false;
    if run_merge && o.clippy {
        r.step("clippy", &clippy_args(&a));
    }
    let use_nextest = o.nextest && ensure_nextest(&mut r, o.install);
    if use_nextest {
        if let Err(e) = std::fs::write(&config, nextest_config(&serial)) {
            return Err(format!("{}: {e}", config.display()));
        }
        if run_merge {
            let merge_filter = filter.as_ref().map(|f| format!("not ({f})"));
            eprintln!("{} slow test(s) wait for the after stage", slow.len());
            let f = tests_with_rerun(&mut r, "tests (merge stage)", &a, &config, threads, merge_filter.as_deref(), &mut timings, &timings_path);
            failed_tests.extend(f);
            if let Some(d) = doctest_args(&ws, &a) {
                r.step("doc-tests", &d);
            }
        }
        if run_after {
            match &filter {
                Some(f) => {
                    let fails =
                        tests_with_rerun(&mut r, "slow tests (after stage)", &a, &config, o.after_threads, Some(f), &mut timings, &timings_path);
                    failed_tests.extend(fails);
                }
                None => eprintln!("\nno recorded slow tests in these packages: the after stage has nothing to run"),
            }
        }
    } else if run_merge {
        fallback = true;
        r.step("tests (cargo test)", &cargo_test_args(&a));
    } else {
        eprintln!("\nwithout nextest every test ran in the merge stage: the after stage has nothing to run");
    }

    // Record what this run built from, then keep the cache under its cap.
    if o.mtime_restore && !current.is_empty() {
        let m = MtimeManifest { root: root_str, built: newest_fingerprint(&target).map(Stamp::of), files: current };
        write_json(&manifest_path, &m.to_json());
    }
    let cap = o.cap_gb.saturating_mul(1 << 30);
    let total = dir_size(&target);
    let mut incr = 0u64;
    let profiles: Vec<PathBuf> =
        std::fs::read_dir(&target).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join(".fingerprint").is_dir()).collect();
    for p in &profiles {
        incr = incr.saturating_add(dir_size(&p.join("incremental")));
    }
    match trim_plan(total, incr, cap) {
        Trim::Nothing => eprintln!("\nbuild cache {:.1} GB (cap {} GB)", total as f64 / 1e9, o.cap_gb),
        Trim::Incremental => {
            for p in &profiles {
                let _ = std::fs::remove_dir_all(p.join("incremental"));
            }
            eprintln!("\nbuild cache {:.1} GB over the {} GB cap: incremental caches removed", total as f64 / 1e9, o.cap_gb);
        }
        Trim::Outputs => {
            for p in &profiles {
                let _ = std::fs::remove_dir_all(p);
            }
            let _ = std::fs::remove_file(&manifest_path);
            eprintln!("\nbuild cache {:.1} GB over the {} GB cap: build outputs removed (the next run builds cold)", total as f64 / 1e9, o.cap_gb);
        }
    }

    // Summary.
    let ok = r.steps.iter().all(|s| s.ok);
    eprintln!("\n=== merge-check summary ({} in total)", fmt_secs(started.elapsed().as_secs_f64()));
    for s in &r.steps {
        eprintln!(
            "  {:<44} {:>8}  {}{}",
            s.name,
            fmt_secs(s.secs),
            if s.ok { "ok" } else { "FAILED" },
            if s.note.is_empty() { String::new() } else { format!(" ({})", s.note) }
        );
    }
    if fallback {
        eprintln!("  (cargo test fallback: every test ran in the merge stage)");
    }
    if !failed_tests.is_empty() {
        eprintln!("\nfailing tests ({}):", failed_tests.len());
        for c in &failed_tests {
            eprintln!("  {} {}\n      re-run: {}", c.binary_id, c.name, rerun_hint(&c.binary_id, &c.name));
        }
    }
    if ok {
        Ok(())
    } else {
        Err(format!("merge-check failed: {}", r.steps.iter().filter(|s| !s.ok).map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, dir: &str, deps: &[(&str, Kind)]) -> Package {
        Package {
            name: name.into(),
            dir: dir.into(),
            deps: deps.iter().map(|(n, k)| ((*n).into(), *k)).collect(),
            has_lib: true,
            test_targets: Vec::new(),
        }
    }

    /// A made-up workspace: geom <- doc <- engine <- ui <- app; cli -> engine; tool dev-uses ui;
    /// fonts (standalone) <- render (build dep).
    fn ws() -> Workspace {
        Workspace {
            packages: vec![
                pkg("geom", "crates/geom", &[]),
                pkg("doc", "crates/doc", &[("geom", Kind::Normal)]),
                pkg("engine", "crates/engine", &[("doc", Kind::Normal)]),
                pkg("ui", "crates/ui", &[("engine", Kind::Normal)]),
                pkg("app", "apps/app", &[("ui", Kind::Normal)]),
                pkg("cli", "apps/cli", &[("engine", Kind::Normal)]),
                pkg("tool", "tool", &[("ui", Kind::Dev)]),
                pkg("toolsuser", "toolsuser", &[("tool", Kind::Normal)]),
                pkg("fonts", "crates/fonts", &[]),
                pkg("render", "crates/render", &[("fonts", Kind::Build)]),
            ],
        }
    }

    fn files(f: &[&str]) -> Vec<String> {
        f.iter().map(|s| (*s).to_string()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn args_parse() {
        let o = parse_args(&["next", "HEAD", "--stage", "all", "--slow-secs", "30", "-p", "geom", "--test-threads", "4", "--no-install"]).unwrap();
        assert_eq!((o.base.as_str(), o.head.as_str(), o.stage), ("next", "HEAD", Stage::All));
        assert_eq!((o.slow_secs, o.test_threads, o.install, o.packages.clone()), (30.0, Some(4), false, vec!["geom".to_string()]));
        assert!(parse_args(&["next"]).is_err());
        assert!(parse_args(&["a", "b", "c"]).is_err());
        assert!(parse_args(&["a", "b", "--stage", "later"]).is_err());
        assert!(parse_args(&["a", "b", "--cap-gb", "0"]).is_err());
        assert!(parse_args(&["a", "b", "--slow-secs", "NaN"]).is_err());
        assert!(parse_args(&["a", "b", "--slow-secs", "inf"]).is_err());
        assert!(parse_args(&["a", "b", "--test-threads"]).is_err());
        assert!(parse_args(&["a", "b", "--bogus"]).is_err());
    }

    #[test]
    fn a_leaf_change_checks_only_the_leaf() {
        let a = affected(&ws(), &files(&["apps/app/src/main.rs"]), &BTreeMap::new());
        assert_eq!(a.packages, set(&["app"]));
        assert!(!a.all);
    }

    #[test]
    fn an_engine_change_checks_its_dependents_and_their_test_users() {
        let a = affected(&ws(), &files(&["crates/engine/src/cmd/x.rs"]), &BTreeMap::new());
        // The old rule (engine touched => also ui and cli) is part of the closure.
        assert_eq!(a.packages, set(&["engine", "ui", "app", "cli", "tool"]));
        assert_eq!(a.reasons["ui"], "depends on engine");
        assert_eq!(a.reasons["tool"], "tests use ui");
        // A dev-dependency does not propagate: toolsuser does not build tool's tests.
        assert!(!a.packages.contains("toolsuser"));
    }

    #[test]
    fn a_foundation_change_reaches_everything_above_it() {
        let a = affected(&ws(), &files(&["crates/geom/src/lib.rs"]), &BTreeMap::new());
        assert_eq!(a.packages, set(&["geom", "doc", "engine", "ui", "app", "cli", "tool"]));
        let b = affected(&ws(), &files(&["crates/fonts/build.rs"]), &BTreeMap::new());
        assert_eq!(b.packages, set(&["fonts", "render"]), "build dependencies propagate");
    }

    #[test]
    fn workspace_files_check_everything() {
        for f in ["Cargo.lock", "Cargo.toml", "clippy.toml", "rust-toolchain.toml", ".cargo/config.toml", ".config/nextest.toml"] {
            let a = affected(&ws(), &files(&[f]), &BTreeMap::new());
            assert!(a.all, "{f}");
            assert_eq!(a.packages.len(), ws().packages.len(), "{f}");
        }
        // A crate's own manifest is that crate, not the workspace.
        assert!(!affected(&ws(), &files(&["crates/doc/Cargo.toml"]), &BTreeMap::new()).all);
    }

    #[test]
    fn files_read_from_outside_a_package_map_to_their_readers() {
        let mut tokens = BTreeMap::new();
        tokens.insert("cli".to_string(), set(&["docs", "src"]));
        tokens.insert("ui".to_string(), set(&["engine"]));
        let a = affected(&ws(), &files(&["docs/compare-facts.json", "plan/notes.md"]), &tokens);
        assert_eq!(a.packages, set(&["cli"]));
        assert_eq!(a.reasons["cli"], "reads ../docs (docs/compare-facts.json)");
        assert_eq!(a.unmapped, files(&["plan/notes.md"]));
        // A package file another package reads by path: ui's tests read engine's sources.
        let b = affected(&ws(), &files(&["crates/doc/src/a.rs"]), &tokens);
        assert!(b.packages.contains("ui"));
        tokens.insert("app".to_string(), set(&["doc"]));
        let c = affected(&ws(), &files(&["crates/doc/data/table.json"]), &tokens);
        assert_eq!(c.reasons["app"], "reads ../doc (crates/doc/data/table.json)");
    }

    #[test]
    fn reference_tokens() {
        let t = ref_tokens(
            r#"include_str!("../../docs/x.json"); let p = root.join("assets/fonts"); concat!(env!("CARGO_MANIFEST_DIR"), "/../ui-egui/src/control.rs"); "../deny.toml" "../." "#,
        );
        assert_eq!(t, set(&["docs", "assets", "ui-egui", "deny.toml"]));
        assert!(ref_tokens("no paths here; a/b/c and ..x").is_empty());
        let keys = reference_keys(&ws(), "crates/ui/src/control.rs");
        assert_eq!(keys, set(&["ui"]));
        assert_eq!(reference_keys(&ws(), "docs/guide/x.md"), set(&["docs"]));
        assert_eq!(reference_keys(&ws(), "crates/gone/src/lib.rs"), set(&["crates", "gone"]));
    }

    #[test]
    fn owner_is_the_deepest_package_dir() {
        let mut w = ws();
        w.packages.push(pkg("inner", "crates/engine/inner", &[]));
        assert_eq!(w.owner("crates/engine/inner/src/lib.rs").map(|p| p.name.as_str()), Some("inner"));
        assert_eq!(w.owner("crates/engine/src/lib.rs").map(|p| p.name.as_str()), Some("engine"));
        assert_eq!(w.owner("crates/engineering/x.rs"), None, "a prefix that is not a directory");
        assert_eq!(w.owner("README.md"), None);
    }

    #[test]
    fn metadata_model() {
        let meta: Value = serde_json::from_str(
            r#"{"workspace_root":"/w","packages":[
              {"name":"geom","manifest_path":"/w/crates/geom/Cargo.toml","dependencies":[{"name":"serde","kind":null}],
               "targets":[{"name":"geom","kind":["lib"],"src_path":"/w/crates/geom/src/lib.rs"},
                          {"name":"bigtest","kind":["test"],"src_path":"/w/crates/geom/tests/bigtest.rs"}]},
              {"name":"cli","manifest_path":"/w/apps/cli/Cargo.toml","dependencies":[
                  {"name":"geom","kind":null,"path":"/w/crates/geom"},{"name":"geom","kind":"dev"},{"name":"cli","kind":"dev"}],
               "targets":[{"name":"cli","kind":["bin"],"src_path":"/w/apps/cli/src/main.rs"}]}]}"#,
        )
        .unwrap();
        let w = workspace_from_metadata(&meta).unwrap();
        let geom = w.get("geom").unwrap();
        assert_eq!((geom.dir.as_str(), geom.has_lib, geom.deps.len()), ("crates/geom", true, 0));
        assert_eq!(geom.test_targets, vec![("bigtest".to_string(), "crates/geom/tests/bigtest.rs".to_string())]);
        let cli = w.get("cli").unwrap();
        assert_eq!((cli.dir.as_str(), cli.has_lib), ("apps/cli", false));
        assert_eq!(cli.deps, vec![("geom".to_string(), Kind::Normal), ("geom".to_string(), Kind::Dev)], "self dev-dep dropped");
        // Windows paths.
        let win: Value = serde_json::from_str(
            r#"{"workspace_root":"C:\\w","packages":[{"name":"a","manifest_path":"C:\\w\\crates\\a\\Cargo.toml","dependencies":[],"targets":[]}]}"#,
        )
        .unwrap();
        assert_eq!(workspace_from_metadata(&win).unwrap().packages[0].dir, "crates/a");
        assert!(workspace_from_metadata(&serde_json::json!({})).is_err());
    }

    #[test]
    fn this_workspace_engine_change_covers_the_old_rule() {
        // The real graph: an engine change must test ui-egui and cli (the old merger rule).
        let Ok(meta) = metadata(&crate::root()) else { return };
        let w = workspace_from_metadata(&meta).unwrap();
        let tokens = package_tokens(&crate::root(), &w);
        let a = affected(&w, &files(&["crates/engine/src/lib.rs"]), &tokens);
        for p in ["cadaddy-engine", "cadaddy-ui-egui", "cadaddy-cli", "cadaddy-mcp"] {
            assert!(a.packages.contains(p), "{p} missing from {:?}", a.packages);
        }
        assert!(!a.packages.contains("cadaddy-geom"), "dependencies of engine are not affected by it");
        // The compare-facts test reads docs/: a docs change checks the cli.
        let d = affected(&w, &files(&["docs/compare-facts.json"]), &tokens);
        assert!(d.packages.contains("cadaddy-cli"), "{:?}", d.packages);
    }

    const JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="4" failures="1" errors="0">
  <testsuite name="pkg-a::slowbin" tests="2" disabled="0" errors="0" failures="1">
    <testcase name="big_build" classname="pkg-a::slowbin" timestamp="2026-11-05T10:00:00Z" time="123.5">
    </testcase>
    <testcase name="mod_x::breaks" classname="pkg-a::slowbin" time="0.25">
      <failure message="test failure" type="test failure">thread panicked at &apos;x &lt; y&apos;</failure>
      <system-err>left: 1 &amp; right: 2</system-err>
    </testcase>
  </testsuite>
  <testsuite name="pkg-b" tests="2">
    <testcase name="quick" time="0.01"/>
    <testcase name="errs" classname="pkg-b" time="2"><error message="crash"/></testcase>
  </testsuite>
</testsuites>"#;

    #[test]
    fn junit_cases() {
        let c = parse_junit(JUNIT);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0], Case { binary_id: "pkg-a::slowbin".into(), name: "big_build".into(), secs: 123.5, failed: false });
        assert!(c[1].failed && c[1].name == "mod_x::breaks");
        assert_eq!((c[2].binary_id.as_str(), c[2].failed), ("pkg-b", false), "classname missing: the suite name");
        assert!(c[3].failed, "an <error> is a failure");
        assert!(parse_junit("").is_empty());
        assert!(parse_junit("<testcase name=\"x\" time=\"NaN\"").is_empty(), "truncated report");
        assert_eq!(xml_unescape("a &lt;b&gt; &amp; &#65;&#x42; &bogus; &"), "a <b> & AB &bogus; &");
    }

    #[test]
    fn slow_tests_from_the_recorded_runs() {
        let mut t = Timings::default();
        t.record(&parse_junit(JUNIT));
        let pkgs = set(&["pkg-a", "pkg-b"]);
        assert_eq!(t.slow(&pkgs, false, 60.0), vec![("pkg-a::slowbin".to_string(), "big_build".to_string())]);
        assert!(t.slow(&set(&["pkg-b"]), false, 60.0).is_empty(), "only the packages being checked");
        assert_eq!(t.slow(&BTreeSet::new(), true, 1.0).len(), 2, "all: every package");
        // The slowest of the last runs counts; older runs fall out.
        for secs in [5.0, 5.0, 5.0] {
            t.record(&[Case { binary_id: "pkg-a::slowbin".into(), name: "big_build".into(), secs, failed: false }]);
        }
        assert!(t.slow(&pkgs, false, 60.0).is_empty());
        t.record(&[Case { binary_id: "pkg-a::slowbin".into(), name: "big_build".into(), secs: 90.0, failed: false }]);
        assert_eq!(t.slow(&pkgs, false, 60.0).len(), 1);
        assert_eq!(t.tests["pkg-a::slowbin big_build"].len(), TIMING_HISTORY);
        // Round trip; hostile values dropped.
        assert_eq!(Timings::from_json(&t.to_json()), t);
        let bad = Timings::from_json(&serde_json::json!({"tests": {"x y": [-1.0, "a", 2.0], "z w": []}}));
        assert_eq!(bad.tests.len(), 1);
        assert_eq!(bad.tests["x y"], vec![2.0]);
    }

    #[test]
    fn the_two_stages_split_the_tests_exactly() {
        let slow = vec![("pkg-a::slowbin".to_string(), "big_build".to_string()), ("pkg-b".to_string(), "odd(name),x".to_string())];
        let f = slow_filter(&slow).unwrap();
        assert_eq!(f, r"(binary_id(=pkg-a::slowbin) & test(=big_build)) | (binary_id(=pkg-b) & test(=odd\(name\)\,x))");
        assert_eq!(slow_filter(&[]), None);
        let a = Affected { packages: set(&["pkg-a", "pkg-b"]), ..Affected::default() };
        let cfg = Path::new("cfg.toml");
        let merge = nextest_args(&a, cfg, 6, Some(&format!("not ({f})")));
        let after = nextest_args(&a, cfg, 2, Some(&f));
        // Same packages and profile; the filters are complements of each other.
        assert_eq!(merge.iter().position(|x| x == "-E").map(|i| merge[i + 1].clone()), Some(format!("not ({f})")));
        assert_eq!(after.iter().position(|x| x == "-E").map(|i| after[i + 1].clone()), Some(f.clone()));
        assert!(merge.windows(2).any(|w| w == ["-p", "pkg-a"]) && after.windows(2).any(|w| w == ["-p", "pkg-b"]));
        assert!(merge.contains(&"--no-fail-fast".to_string()));
    }

    #[test]
    fn commands() {
        let w = ws();
        let a = affected(&w, &files(&["apps/cli/src/main.rs", "crates/fonts/src/lib.rs"]), &BTreeMap::new());
        assert_eq!(shell_line(&clippy_args(&a)), "cargo clippy -p cli -p fonts -p render --all-targets -- -D warnings");
        assert_eq!(shell_line(&cargo_test_args(&a)), "cargo test --no-fail-fast -p cli -p fonts -p render");
        let all = affected(&w, &files(&["Cargo.lock"]), &BTreeMap::new());
        assert_eq!(shell_line(&clippy_args(&all)), "cargo clippy --workspace --all-targets -- -D warnings");
        let mut w2 = w.clone();
        for p in &mut w2.packages {
            p.has_lib = p.name != "cli";
        }
        assert_eq!(doctest_args(&w2, &a).map(|d| shell_line(&d)), Some("cargo test --doc --no-fail-fast -p fonts -p render".to_string()));
        let only_cli = affected(&w2, &files(&["apps/cli/src/main.rs"]), &BTreeMap::new());
        assert_eq!(doctest_args(&w2, &only_cli), None, "no library: no doc-tests");
        assert_eq!(shell_line(&["nextest".into(), "-E".into(), "not (test(=a))".into()]), "cargo nextest -E 'not (test(=a))'");
    }

    #[test]
    fn rerun_hints() {
        assert_eq!(rerun_hint("engine::bigtest", "t"), "cargo test -p engine --test bigtest -- --exact t");
        assert_eq!(rerun_hint("engine", "m::t"), "cargo test -p engine --lib -- --exact m::t");
        assert_eq!(rerun_hint("app::bin/app", "t"), "cargo test -p app --bin app -- --exact t");
    }

    #[test]
    fn serial_locks_detected() {
        assert!(uses_serial_lock("use std::sync::Mutex;\nstatic SERIAL: Mutex<()> = Mutex::new(());\n#[test] fn a() {}"));
        assert!(uses_serial_lock("    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());"));
        assert!(!uses_serial_lock("let m: Mutex<()> = Mutex::new(());\nstatic N: AtomicUsize = AtomicUsize::new(0);"));
        let cfg = nextest_config(&["pkg-a::slowbin".to_string(), "pkg-b::x".to_string()]);
        assert!(cfg.contains("[profile.merge-check]") && cfg.contains("[profile.merge-check.junit]"));
        assert!(cfg.contains("serial-0 = { max-threads = 1 }") && cfg.contains("serial-1 = { max-threads = 1 }"));
        assert!(cfg.contains("filter = \"binary_id(=pkg-a::slowbin)\"\ntest-group = \"serial-0\""));
        assert!(!nextest_config(&[]).contains("[test-groups]"));
    }

    #[test]
    fn failures_rerun_alone_only_when_tests_ran() {
        assert!(rerun_alone(false, Some(100), 5));
        assert!(!rerun_alone(true, Some(0), 0), "green");
        assert!(!rerun_alone(false, Some(101), 0), "build failed");
        assert!(!rerun_alone(false, Some(100), 0), "failed without a report: no list to re-run");
        assert!(!rerun_alone(false, None, 3), "killed");
        assert!(!rerun_alone(false, Some(100), MAX_RERUN_ALONE + 1), "a broken batch");
    }

    #[test]
    fn cache_cap() {
        assert_eq!(trim_plan(10, 5, 20), Trim::Nothing);
        assert_eq!(trim_plan(30, 15, 20), Trim::Incremental);
        assert_eq!(trim_plan(30, 5, 20), Trim::Outputs);
        assert_eq!(trim_plan(u64::MAX, u64::MAX, 0), Trim::Incremental);
    }

    #[test]
    fn fingerprint_units() {
        assert_eq!(unit_package("cadaddy-engine-0123456789abcdef"), Some("cadaddy-engine"));
        assert_eq!(unit_package("serde-0123456789ABCDEF"), Some("serde"));
        assert_eq!(unit_package("cadaddy-engine"), None);
        assert_eq!(unit_package("x-0123"), None);
    }

    fn st(secs: u64) -> Stamp {
        Stamp { secs, nanos: 0 }
    }

    #[test]
    fn mtimes_restored_only_for_unchanged_content_of_unbuilt_packages() {
        let w = ws();
        let mut m = MtimeManifest { root: "/w".into(), built: Some(st(150)), files: BTreeMap::new() };
        for (p, h) in [("crates/geom/src/lib.rs", "g"), ("crates/doc/src/lib.rs", "d"), ("crates/engine/src/lib.rs", "e"), ("docs/x.json", "x")] {
            m.files.insert(p.into(), (h.into(), st(100)));
        }
        let mut now = BTreeMap::new();
        now.insert("crates/geom/src/lib.rs".to_string(), ("g".to_string(), st(200))); // same content, fresh checkout
        now.insert("crates/doc/src/lib.rs".to_string(), ("D2".to_string(), st(200))); // changed
        now.insert("crates/engine/src/lib.rs".to_string(), ("e".to_string(), st(100))); // already right
        now.insert("docs/x.json".to_string(), ("x".to_string(), st(200)));
        now.insert("crates/geom/src/new.rs".to_string(), ("n".to_string(), st(200))); // new file
        let owner = |p: &str| w.owner(p).map(|q| q.name.clone());
        let plan = restore_plan(&m, &now, &owner, &BTreeSet::new());
        assert_eq!(plan, vec![("crates/geom/src/lib.rs".to_string(), st(100)), ("docs/x.json".to_string(), st(100))]);
        // geom was built outside this tool since the last run: its files and every file outside a
        // package keep their new mtimes (cargo can't be told an older build is current).
        let plan = restore_plan(&m, &now, &owner, &set(&["geom"]));
        assert!(plan.is_empty(), "{plan:?}");
        let plan = restore_plan(&m, &now, &owner, &set(&["engine"]));
        assert_eq!(plan, vec![("crates/geom/src/lib.rs".to_string(), st(100))]);
        // Round trip.
        assert_eq!(MtimeManifest::from_json(&m.to_json()), m);
        let junk = MtimeManifest::from_json(&serde_json::json!({"root": 3, "built": ["x"], "files": {"a": ["h", [1, 99999999999u64]], "b": 7}}));
        assert_eq!((junk.root.as_str(), junk.built, junk.files.len()), ("", None, 0));
    }

    #[test]
    fn mtime_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!("merge-check-mtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fp/.fingerprint/cadaddy-geom-0123456789abcdef")).unwrap();
        let f = dir.join("a.rs");
        std::fs::write(&f, "fn a() {}").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        set_mtime(&f, old).unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().modified().unwrap(), old);
        let cur = current_state(&dir, &["a.rs".to_string(), "missing.rs".to_string()]);
        assert_eq!(cur.len(), 1);
        assert_eq!(cur["a.rs"].1, Stamp { secs: 1_700_000_000, nanos: 0 });
        assert_eq!(cur["a.rs"].0.len(), 64);
        // A fingerprint written now is newer than an old stamp: the package counts as built.
        std::fs::write(dir.join("fp/.fingerprint/cadaddy-geom-0123456789abcdef/lib-cadaddy_geom"), "x").unwrap();
        let built = built_since(&dir, &set(&["cadaddy-geom", "cadaddy-doc"]), old);
        assert_eq!(built, set(&["cadaddy-geom"]));
        assert!(built_since(&dir, &set(&["cadaddy-geom"]), SystemTime::now() + Duration::from_secs(3600)).is_empty());
        assert!(newest_fingerprint(&dir).is_some());
        assert!(dir_size(&dir) >= 10);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
