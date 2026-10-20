//! Workspace tooling: `cargo xtask <command>`.
//!
//! Pure Rust (std + serde_json). External tools (`cargo`, `curl`, `tar`) are
//! invoked through `std::process::Command`.

mod assets;
mod casefs;
mod ico;
mod installtest;
mod layers;
mod mcptest;
mod parity;
mod perf;
mod release;
mod secrets;
mod signing;
mod smoke;
mod soak;
mod stats;
mod supplychain;
mod version;
mod webtest;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  assets          check that every icon/image/font/asset is attributed in ATTRIBUTION.md
  layers          enforce the crate dependency layering (plan/architecture.md §3)
  wasm            cargo check --target wasm32-unknown-unknown for the wasm-safe crates
  ci              fmt --check, clippy -D warnings, test, assets, layers, wasm, perf, web, soak (2 min
                  headless), supplychain, advisories, corpus (stops at first failure)
  corpus [--download]
                  show where test corpora live; --download fetches PngSuite into corpus/pngsuite
  corpus synthetic [--sizes 10000,100000] [--seed N] [--out DIR] [--keep]
                  made-up floor plans of 10k-500k objects (seeded) round-tripped through DXF, DWG
                  and .cdy: exact differences and save/open/regen times against budgets (warnings);
                  report in target/corpus/report.md
  stats [--exact] count tests and lines per crate (--exact: ask the test harness via `-- --list`)
  parity          recompute the AutoCAD menu-breadth checklist in docs/parity-checklist.md
  ico <out.ico> <in.png>...
                  pack square PNGs (<= 256 px) into a Windows .ico (see packaging/icons.sh)
  casefs [--size 8g] [-- <cargo test args>]
                  run the workspace tests on a case-sensitive disk image (macOS), so Linux-only
                  failures show up on a Mac; the image is deleted afterwards
  smoke <CADaddy-Mac.dmg | CADaddy.app | app executable | CADaddy.mcpb | cadaddy-cli> [--version X.Y.Z] [--out DIR] [--keep]
                  release smoke test: version, draw, save DWG/DXF/.cdy, reopen, plot PDF, sheet set
                  publish, Walk on/off and update check (app); report in target/smoke/report.json
  mcptest [--port 47820] [--cli PATH] [--mcpb PATH]
                  the MCP suite against the running app (start it with --control 47820): every
                  tool normal and bad, the app's tools, the agent guard, and the .mcpb's server
  perf [--full] [--update-budget]
                  performance budgets (perf/budget.json): open/save DWG/DXF/.cdy, regen, select and
                  move all, zoom extents, 3D build, plot PDF on 1k/10k objects (--full: 100k, 500k
                  too); warns over budget, fails over 2x; trend in target/perf/history.jsonl
  webtest         the web build in a headless browser (open DXF/DWG, draw, undo, save, recovery,
                  Walk, autosave); skipped without a Chromium-family browser or a matching wasm-bindgen
  installtest --new DIR [--old DIR]
                  install, upgrade and uninstall checks of release files: MSI tables (msitools in
                  Docker), the DMG (Mac), the .deb and AppImage (Docker); --old for the upgrade
  soak [--minutes N] [--mode headless|window|both] [--app PATH] [--cli PATH] [--out DIR] [--profile release|dev]
                  long-session soak: a mixed workload for N minutes (default 2, headless); memory,
                  handles, GPU buffers and frame times sampled; fails on a leak or a stall;
                  report in target/soak/report.md
  supplychain     offline: shipped crates' licences (deny.toml; no GPL/AGPL/unknown), sources
                  (crates.io, the workspace, pinned allowed git), banned crates, and a secrets scan
  advisories      RustSec advisories: cargo deny check + cargo audit (needs both tools and the network)
  sbom [--out FILE]
                  CycloneDX 1.5 SBOM of the shipped crates (default target/sbom/cadaddy-<version>.cdx.json),
                  to attach to the release
  secrets         scan the tree for credentials (keys, tokens, private keys); exceptions in .secrets-allow
  signing [--require]
                  what a release built now would be signed with on each platform (Windows Authenticode,
                  macOS Developer ID + notarization, Linux GPG, the updater's SHA256SUMS.sig);
                  --require fails if any would ship unsigned (docs/code-signing.md)
  fuzz [dxf|dwg|pdf|svg|image|cdy|all] [seconds]
                  mutation-fuzz the file readers (default all, 60 s each); crashes and hangs are
  fuzz [TARGET|all] [seconds]
                  mutation-fuzz the readers (default all, 60 s each): dxf dwg pdf svg image cdy shx
                  las pts xyz cdywalk markups stl obj gltf xlsx zip lisp; crashes and hangs are
                  saved in target/fuzz-findings/ (coverage-guided targets: fuzz/, nightly)
  release-key --out FILE
                  make a release signing key: the secret to FILE (keep it off the repository), the
                  public key line for apps/cadaddy/src/release_keys.txt printed
  sign-release DIR [--version X.Y.Z]
                  write SHA256SUMS and SHA256SUMS.sig for the installers in DIR, signed with the key
                  file named by CADADDY_RELEASE_KEY (the updater refuses unsigned releases)
  verify-release DIR [--version X.Y.Z]
                  check DIR's signature and hashes as the updater will
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("assets") => assets::run(&root()),
        Some("layers") => cmd_layers(),
        Some("wasm") => cmd_wasm(),
        Some("ci") => cmd_ci(),
        Some("corpus") if rest.first() == Some(&"synthetic") => cmd_corpus_synthetic(rest.get(1..).unwrap_or_default()),
        Some("corpus") => cmd_corpus(rest.contains(&"--download")),
        Some("stats") => stats::run(&root(), rest.contains(&"--exact")),
        Some("parity") => parity::run(&root()),
        Some("ico") => ico::run(&rest),
        Some("version") => version::run(&root(), &rest),
        Some("fuzz") => cmd_fuzz(&rest),
        Some("casefs") => casefs::run(&root(), &rest),
        Some("smoke") => smoke::run(&root(), &rest),
        Some("mcptest") => mcptest::run(&root(), &rest),
        Some("perf") => perf::run(&root(), &rest),
        Some("webtest") => webtest::run(&root(), &rest),
        Some("installtest") => installtest::run(&root(), &rest),
        Some("release-key") => release::key_cmd(&rest),
        Some("sign-release") => release::sign_cmd(&root(), &rest),
        Some("verify-release") => release::verify_cmd(&root(), &rest),
        Some("soak") => soak::run(&root(), &rest),
        Some("supplychain") => supplychain::run(&root()),
        Some("advisories") => supplychain::advisories(&root()),
        Some("sbom") => supplychain::sbom(&root(), &rest),
        Some("secrets") => secrets::run(&root()),
        Some("signing") => signing::run(&rest),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

pub fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))
}

/// `fuzz [target|all] [seconds]`: the stable mutation fuzzer over the file readers
/// (crates/io/examples/fuzz.rs); findings land in target/fuzz-findings/<target>/. The
/// coverage-guided cargo-fuzz targets are in fuzz/ (nightly: `cargo +nightly fuzz run dxf`).
/// The engine's own readers; every other target is a file format `cadaddy-io` reads.
const ENGINE_FUZZ: [&str; 3] = ["xlsx", "zip", "lisp"];

fn cmd_fuzz(rest: &[&str]) -> Result<(), String> {
    let target = rest.first().copied().unwrap_or("all");
    let secs = rest.get(1).copied().unwrap_or("60");
    let out = root().join("target").join("fuzz-findings");
    let crates: &[&str] = match target {
        "all" => &["cadaddy-io", "cadaddy-engine"],
        t if ENGINE_FUZZ.contains(&t) => &["cadaddy-engine"],
        _ => &["cadaddy-io"],
    };
    for krate in crates {
        let mut c = cargo();
        c.args(["run", "--release", "-p", krate, "--example", "fuzz", "--", target, secs]).arg(&out);
        run(c, &format!("cargo run --release -p {krate} --example fuzz -- {target} {secs}"))?;
    }
    Ok(())
}

fn cmd_layers() -> Result<(), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    println!("Dependency layering (plan/architecture.md §3)\n");
    println!("{:<28} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| {
                let k = match d.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{k}", layers::short_name(&d.name))
            })
            .collect();
        println!("{:<28} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        Ok(())
    } else {
        println!("{} violation(s):", violations.len());
        for v in &violations {
            println!("  - {v}");
        }
        Err(format!("{} layering violation(s)", violations.len()))
    }
}

/// Workspace packages that must build for wasm32: all L0–L5 crates plus
/// the egui shell.
fn wasm_set() -> Result<Vec<String>, String> {
    let crates = layers::from_metadata(&metadata()?)?;
    Ok(crates
        .into_iter()
        .filter(|c| match layers::classify(&c.name) {
            Some(layers::Class::Layer(l)) => l <= 4 || layers::short_name(&c.name) == "ui-egui",
            Some(layers::Class::Standalone) => true,
            _ => false,
        })
        .map(|c| c.name)
        .collect())
}

fn cmd_wasm() -> Result<(), String> {
    let set = wasm_set()?;
    let mut results = Vec::new();
    for pkg in &set {
        let mut c = cargo();
        c.args(["check", "--target", "wasm32-unknown-unknown", "-p", pkg]);
        let ok = run(c, &format!("cargo check --target wasm32-unknown-unknown -p {pkg}")).is_ok();
        results.push((pkg.clone(), ok));
    }
    println!("\nwasm32-unknown-unknown check:");
    for (p, ok) in &results {
        println!("  {:<6} {p}", if *ok { "ok" } else { "FAIL" });
    }
    let failed = results.iter().filter(|r| !r.1).count();
    if failed == 0 { Ok(()) } else { Err(format!("{failed} crate(s) failed the wasm check")) }
}

fn cmd_ci() -> Result<(), String> {
    type Step = (&'static str, Box<dyn Fn() -> Result<(), String>>);
    let steps: Vec<Step> = vec![
        (
            "fmt",
            Box::new(|| {
                let mut c = cargo();
                c.args(["fmt", "--all", "--", "--check"]);
                run(c, "cargo fmt --all -- --check")
            }),
        ),
        (
            "clippy",
            Box::new(|| {
                let mut c = cargo();
                c.args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
                run(c, "cargo clippy --workspace --all-targets -- -D warnings")
            }),
        ),
        (
            "test",
            Box::new(|| {
                let mut c = cargo();
                c.args(["test", "--workspace"]);
                run(c, "cargo test --workspace")
            }),
        ),
        ("assets", Box::new(|| assets::run(&root()))),
        ("layers", Box::new(cmd_layers)),
        ("wasm", Box::new(cmd_wasm)),
        ("perf", Box::new(|| perf::run(&root(), &[]))),
        // The web build in a headless browser (skipped where there is none).
        ("web", Box::new(|| webtest::run(&root(), &[]))),
        // Two minutes of the headless app under a mixed workload: a leak or a stall fails.
        ("soak", Box::new(|| soak::run(&root(), &["--minutes", "2", "--profile", "dev"]))),
        ("supplychain", Box::new(|| supplychain::run(&root()))),
        ("advisories", Box::new(|| supplychain::advisories(&root()))),
    ];
    let mut done = Vec::new();
    for (name, f) in &steps {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI summary:");
            for d in &done {
                println!("  ok    {d}");
            }
            println!("  FAIL  {name}: {e}");
            for (n, _) in steps.iter().skip(done.len() + 1) {
                println!("  skip  {n}");
            }
            return Err(format!("ci failed at `{name}`"));
        }
        done.push(*name);
    }
    println!("\nCI summary: all {} steps passed ({})", done.len(), done.join(", "));
    Ok(())
}

/// The synthetic corpus (`crates/engine/examples/corpus.rs`), built and run in release.
fn cmd_corpus_synthetic(args: &[&str]) -> Result<(), String> {
    const FLAGS: &[&str] = &["--sizes", "--seed", "--out", "--keep"];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !FLAGS.contains(a) {
            return Err(format!("corpus synthetic: unknown option {a}\n{USAGE}"));
        }
        if *a != "--keep" && it.next().is_none() {
            return Err(format!("corpus synthetic: {a} needs a value"));
        }
    }
    let status = Command::new(env!("CARGO"))
        .current_dir(root())
        .args(["run", "--release", "-p", "cadaddy-engine", "--example", "corpus", "--"])
        .args(args)
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("corpus synthetic: {status}")) }
}

fn cmd_corpus(_download: bool) -> Result<(), String> {
    println!("Test corpora live in cadaddy-corpus (planned); never commit large binaries here.");
    Ok(())
}
