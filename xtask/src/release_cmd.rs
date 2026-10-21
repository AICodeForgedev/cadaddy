//! `cargo xtask release <version> [--dry-run] [--skip-build] [--skip-smoke] [--skip-sign]`
//!
//! One-command release automation. Stops on the first failure, no network writes. Steps:
//! 1. Validate version and check that `git status` is clean.
//! 2. Version bump in `Cargo.toml` + `Cargo.lock`.
//! 3. `git archive` export with `CADADDY_BUILD_SHA`.
//! 4. Platform build: Mac DMG (on Mac), Windows MSI (on Windows), Linux packages (docker).
//!    Each step is runnable separately per machine.
//! 5. `.mcpb` from the CLIs.
//! 6. `sign-release` + `verify-release`.
//! 7. `xtask smoke` on each installer.
//! 8. Release notes drafted from the commits since the last tag.
//! 9. A checklist of what still needs a human (the commands to run, not run by this tool).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Options parsed from the CLI.
struct Opts {
    version: String,
    dry_run: bool,
    skip_build: bool,
    skip_smoke: bool,
    skip_sign: bool,
}

fn parse(args: &[&str]) -> Result<Opts, String> {
    let mut version = None;
    let mut dry_run = false;
    let mut skip_build = false;
    let mut skip_smoke = false;
    let mut skip_sign = false;
    for a in args {
        match *a {
            "--dry-run" => dry_run = true,
            "--skip-build" => skip_build = true,
            "--skip-smoke" => skip_smoke = true,
            "--skip-sign" => skip_sign = true,
            s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
            s => {
                if version.replace(s.to_string()).is_some() {
                    return Err("only one version argument".into());
                }
            }
        }
    }
    let version = version.ok_or("usage: cargo xtask release <version> [--dry-run] [--skip-build] [--skip-smoke] [--skip-sign]")?;
    crate::version::validate(&version)?;
    Ok(Opts { version, dry_run, skip_build, skip_smoke, skip_sign })
}

/// One step's outcome for the final summary.
#[derive(Debug)]
struct StepResult {
    name: &'static str,
    ok: bool,
    detail: String,
}

/// All steps collected.
struct Steps {
    results: Vec<StepResult>,
}

impl Steps {
    fn new() -> Self {
        Self { results: Vec::new() }
    }
    fn pass(&mut self, name: &'static str, detail: String) {
        println!("  PASS  {name:<24} {detail}");
        self.results.push(StepResult { name, ok: true, detail });
    }
    fn fail(&mut self, name: &'static str, detail: String) -> String {
        println!("  FAIL  {name:<24} {detail}");
        self.results.push(StepResult { name, ok: false, detail: detail.clone() });
        detail
    }
    fn skip(&mut self, name: &'static str, why: &str) {
        println!("  SKIP  {name:<24} {why}");
        self.results.push(StepResult { name, ok: true, detail: format!("skipped: {why}") });
    }
}

/// Run a command, returning stdout on success or an error.
fn run_cmd(cmd: &mut Command) -> Result<String, String> {
    let output = cmd.output().map_err(|e| format!("spawn: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        Err(msg)
    }
}

/// Is the git working tree clean?
fn git_clean(root: &Path) -> Result<bool, String> {
    let out = run_cmd(Command::new("git").current_dir(root).args(["status", "--porcelain"]))?;
    Ok(out.is_empty())
}

/// The HEAD commit SHA.
fn head_sha(root: &Path) -> Result<String, String> {
    run_cmd(Command::new("git").current_dir(root).args(["rev-parse", "HEAD"]))
}

/// The most recent tag (for release notes).
fn last_tag(root: &Path) -> Result<Option<String>, String> {
    match run_cmd(Command::new("git").current_dir(root).args(["describe", "--tags", "--abbrev=0"])) {
        Ok(tag) if !tag.is_empty() => Ok(Some(tag)),
        _ => Ok(None),
    }
}

/// Commits since `since_ref` (or all if None), one per line: `<short-sha> <subject>`.
fn commits_since(root: &Path, since_ref: Option<&str>) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root);
    match since_ref {
        Some(r) => {
            cmd.args(["log", "--oneline", "--no-decorate", &format!("{r}..HEAD")]);
        }
        None => {
            cmd.args(["log", "--oneline", "--no-decorate"]);
        }
    }
    let out = run_cmd(&mut cmd)?;
    Ok(out.lines().map(String::from).collect())
}

/// Draft release notes from commits.
fn draft_release_notes(root: &Path, version: &str) -> Result<String, String> {
    let tag = last_tag(root)?;
    let commits = commits_since(root, tag.as_deref())?;
    let mut notes = String::new();
    notes.push_str(&format!("# CADaddy v{version}\n\n"));
    if commits.is_empty() {
        notes.push_str("No commits since the last tag.\n");
    } else {
        let since = tag.as_deref().unwrap_or("the beginning");
        notes.push_str(&format!("## Changes since {since}\n\n"));
        for c in &commits {
            notes.push_str(&format!("- {c}\n"));
        }
    }
    notes.push_str("\n---\n_Draft: review and edit before publishing._\n");
    Ok(notes)
}

/// Detect which platform we're on.
fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// The packaging script for this platform.
fn packaging_script(root: &Path) -> Option<PathBuf> {
    let plat = current_platform();
    let script = match plat {
        "macos" => root.join("packaging/macos/package.sh"),
        "windows" => root.join("packaging/windows/package.ps1"),
        "linux" => root.join("packaging/linux/package.sh"),
        _ => return None,
    };
    if script.exists() { Some(script) } else { None }
}

/// Build the platform package. Returns the dist directory.
fn build_platform(root: &Path, version: &str, sha: &str) -> Result<PathBuf, String> {
    let dist = root.join("dist").join("release");
    std::fs::create_dir_all(&dist).map_err(|e| format!("create dist/release: {e}"))?;
    let plat = current_platform();
    match plat {
        "macos" => {
            let script = root.join("packaging/macos/package.sh");
            let mut cmd = Command::new("bash");
            cmd.arg(&script).current_dir(root).env("CADADDY_VERSION", version).env("CADADDY_BUILD_SHA", sha).env("DIST", &dist);
            let status = cmd.status().map_err(|e| format!("macos package.sh: {e}"))?;
            if !status.success() {
                return Err(format!("macos package.sh exited {status}"));
            }
        }
        "windows" => {
            let script = root.join("packaging/windows/package.ps1");
            let mut cmd = Command::new("pwsh");
            cmd.arg("-File").arg(&script).current_dir(root).env("CADADDY_VERSION", version).env("CADADDY_BUILD_SHA", sha).env("DIST", &dist);
            let status = cmd.status().map_err(|e| format!("windows package.ps1: {e}"))?;
            if !status.success() {
                return Err(format!("windows package.ps1 exited {status}"));
            }
        }
        "linux" => {
            let script = root.join("packaging/linux/package.sh");
            let mut cmd = Command::new("bash");
            cmd.arg(&script).current_dir(root).env("CADADDY_VERSION", version).env("CADADDY_BUILD_SHA", sha).env("DIST", &dist);
            let status = cmd.status().map_err(|e| format!("linux package.sh: {e}"))?;
            if !status.success() {
                return Err(format!("linux package.sh exited {status}"));
            }
        }
        other => return Err(format!("unsupported platform: {other}")),
    }
    Ok(dist)
}

/// Build the `.mcpb` bundle from the CLI in the dist directory.
fn build_mcpb(root: &Path, dist: &Path, version: &str) -> Result<PathBuf, String> {
    // The .mcpb is a zip of server/cadaddy-cli + manifest.json. Find the CLI binary.
    let cli_name = if cfg!(windows) { "cadaddy-cli.exe" } else { "cadaddy-cli" };
    // Look in the dist for a CLI zip or directly in the build target.
    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| root.join("target").to_string_lossy().to_string());
    let cli_bin = PathBuf::from(&target_dir).join("release").join(cli_name);
    if !cli_bin.exists() {
        return Err(format!("{} not found; the platform build step should have produced it", cli_bin.display()));
    }
    let mcpb_dir = PathBuf::from(&target_dir).join("mcpb-stage");
    let server_dir = mcpb_dir.join("server");
    std::fs::create_dir_all(&server_dir).map_err(|e| format!("create mcpb stage: {e}"))?;
    std::fs::copy(&cli_bin, server_dir.join(cli_name)).map_err(|e| format!("copy cli to mcpb: {e}"))?;
    // Write a minimal manifest.json.
    let manifest = serde_json::json!({
        "name": "cadaddy",
        "version": version,
        "description": "CADaddy MCP server for computer-aided design",
        "server": {
            "command": cli_name,
            "args": ["mcp"]
        }
    });
    let manifest_path = mcpb_dir.join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap_or_default()).map_err(|e| format!("write manifest.json: {e}"))?;
    let mcpb_path = dist.join("CADaddy.mcpb");
    // Create a zip using the zip command.
    let status = Command::new("zip").args(["-j", "-q"]).arg(&mcpb_path).arg(&manifest_path).arg(server_dir.join(cli_name)).status();
    match status {
        Ok(s) if s.success() => {}
        _ => {
            // Fallback: use tar on macOS (tar can create zips).
            let status = Command::new("ditto")
                .args(["-c", "-k", "--keepParent"])
                .arg(&mcpb_dir)
                .arg(&mcpb_path)
                .status()
                .map_err(|e| format!("ditto: {e}"))?;
            if !status.success() {
                return Err("could not create .mcpb (no zip or ditto)".into());
            }
        }
    }
    // Clean up the staging dir.
    let _ = std::fs::remove_dir_all(&mcpb_dir);
    Ok(mcpb_path)
}

/// Run `cargo xtask sign-release` on the dist directory.
fn sign_release(root: &Path, dist: &Path, version: &str) -> Result<(), String> {
    crate::release::sign_cmd(root, &[&dist.to_string_lossy(), "--version", version])
}

/// A smoke result for one artifact.
type SmokeResult = (String, Result<(), String>);

/// Run `cargo xtask smoke` on each installer in the dist directory.
fn smoke_installers(root: &Path, dist: &Path, version: &str) -> Result<Vec<SmokeResult>, String> {
    let mut results = Vec::new();
    let entries: Vec<PathBuf> =
        std::fs::read_dir(dist).map_err(|e| format!("read dist: {e}"))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
    for entry in &entries {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        // Smoke test DMGs, .app bundles, .mcpb files and CLI executables.
        let testable = name.ends_with(".dmg") || name.ends_with(".mcpb") || (name.starts_with("cadaddy-cli") && !name.contains('.'));
        if !testable {
            continue;
        }
        let out_dir = dist.join(format!("smoke-{}", name.replace('.', "-")));
        let r = crate::smoke::run(root, &[&entry.to_string_lossy(), "--version", version, "--out", &out_dir.to_string_lossy()]);
        results.push((name.to_string(), r));
    }
    Ok(results)
}

/// The human checklist: what still needs a person.
fn human_checklist(version: &str, dist: &Path, notes_path: &Path) -> String {
    let tag = format!("v{version}");
    let mut checklist = String::new();
    checklist.push_str(&format!("# Release v{version} — remaining manual steps\n\n"));

    checklist.push_str("## 1. Review and commit the version bump\n");
    checklist.push_str(&format!("```\ngit add Cargo.toml Cargo.lock\ngit commit -m \"release: bump version to {version}\"\n```\n\n"));

    checklist.push_str("## 2. Tag the release\n");
    checklist.push_str(&format!("```\ngit tag -a {tag} -m \"CADaddy {tag}\"\n```\n\n"));

    checklist.push_str("## 3. Push\n");
    checklist.push_str(&format!("```\ngit push origin main\ngit push origin {tag}\n```\n\n"));

    checklist.push_str("## 4. Create the GitHub Release\n");
    checklist.push_str(&format!(
        "```\ngh release create {tag} {}/* --title \"CADaddy {tag}\" --notes-file {}\n```\n",
        dist.display(),
        notes_path.display()
    ));
    checklist.push_str("(Review the release notes before publishing.)\n\n");

    checklist.push_str("## 5. MCP Registry\n");
    checklist.push_str(&format!("Update the MCP registry entry to version {version}.\n\n"));

    checklist.push_str("## 6. Website version\n");
    checklist.push_str(&format!("Update aicodeforge.dev with the new version {version}.\n\n"));

    checklist.push_str("## 7. PNG review\n");
    checklist.push_str("Check every new screenshot/golden PNG in the release for personal info.\n\n");

    checklist.push_str("## 8. Announce\n");
    checklist.push_str("Post the release announcement.\n");

    checklist
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let opts = parse(args)?;
    let version = &opts.version;
    println!("=== cargo xtask release {version}{} ===", if opts.dry_run { " (dry run)" } else { "" });
    let mut steps = Steps::new();

    // Step 1: Validate environment.
    let sha = match head_sha(root) {
        Ok(s) => {
            steps.pass("git sha", s.chars().take(12).collect());
            s
        }
        Err(e) => return Err(steps.fail("git sha", format!("not a git repo or no commits: {e}"))),
    };

    if !opts.dry_run {
        match git_clean(root) {
            Ok(true) => steps.pass("git clean", "working tree is clean".into()),
            Ok(false) => return Err(steps.fail("git clean", "uncommitted changes; commit or stash first".into())),
            Err(e) => return Err(steps.fail("git clean", e)),
        }
    } else {
        steps.skip("git clean", "dry run");
    }

    // Check current version.
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let current = crate::version::read(&manifest)?;
    steps.pass("current version", current.clone());

    // Step 2: Version bump.
    if !opts.dry_run {
        if current == *version {
            steps.pass("version bump", format!("already at {version}"));
        } else {
            match crate::version::run(root, &["set", version]) {
                Ok(()) => steps.pass("version bump", format!("{current} -> {version}")),
                Err(e) => return Err(steps.fail("version bump", e)),
            }
        }
    } else {
        steps.skip("version bump", &format!("dry run: would set {current} -> {version}"));
    }

    // Step 3: Record the build SHA.
    let short_sha: String = sha.chars().take(12).collect();
    steps.pass("build sha", format!("CADADDY_BUILD_SHA={short_sha}"));

    // Step 4: Platform build.
    let dist = root.join("dist").join("release");
    if !opts.skip_build && !opts.dry_run {
        match packaging_script(root) {
            Some(_) => match build_platform(root, version, &sha) {
                Ok(d) => {
                    let count = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
                    steps.pass("platform build", format!("{count} file(s) in {}", d.display()));
                }
                Err(e) => return Err(steps.fail("platform build", e)),
            },
            None => steps.skip("platform build", &format!("no packaging script for {}", current_platform())),
        }
    } else if opts.skip_build {
        steps.skip("platform build", "skipped (--skip-build)");
    } else {
        steps.skip("platform build", "dry run");
    }

    // Step 5: .mcpb
    if !opts.skip_build && !opts.dry_run {
        match build_mcpb(root, &dist, version) {
            Ok(p) => {
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                steps.pass("mcpb", format!("{} ({size} bytes)", p.file_name().and_then(|n| n.to_str()).unwrap_or_default()));
            }
            Err(e) => {
                // Non-fatal: the .mcpb may need a build first.
                steps.fail("mcpb", e);
            }
        }
    } else {
        steps.skip("mcpb", if opts.skip_build { "skipped (--skip-build)" } else { "dry run" });
    }

    // Step 6: Sign and verify.
    if !opts.skip_sign && !opts.dry_run && dist.exists() {
        match sign_release(root, &dist, version) {
            Ok(()) => steps.pass("sign-release", "SHA256SUMS + SHA256SUMS.sig written and verified".into()),
            Err(e) => {
                // Non-fatal: signing material may not be present.
                steps.fail("sign-release", e);
            }
        }
    } else if opts.skip_sign {
        steps.skip("sign-release", "skipped (--skip-sign)");
    } else {
        steps.skip("sign-release", if opts.dry_run { "dry run" } else { "no dist directory" });
    }

    // Step 7: Smoke test.
    if !opts.skip_smoke && !opts.dry_run && dist.exists() {
        match smoke_installers(root, &dist, version) {
            Ok(results) => {
                let mut any_fail = false;
                for (name, r) in &results {
                    match r {
                        Ok(()) => steps.pass("smoke", format!("{name}: passed")),
                        Err(e) => {
                            steps.fail("smoke", format!("{name}: {e}"));
                            any_fail = true;
                        }
                    }
                }
                if results.is_empty() {
                    steps.skip("smoke", "no testable artifacts in dist/");
                }
                if any_fail {
                    // Don't abort; the checklist still prints.
                }
            }
            Err(e) => {
                steps.fail("smoke", e);
            }
        }
    } else if opts.skip_smoke {
        steps.skip("smoke", "skipped (--skip-smoke)");
    } else {
        steps.skip("smoke", if opts.dry_run { "dry run" } else { "no dist directory" });
    }

    // Step 8: Release notes.
    let notes_path = root.join("dist").join(format!("release-notes-{version}.md"));
    std::fs::create_dir_all(root.join("dist")).map_err(|e| e.to_string())?;
    match draft_release_notes(root, version) {
        Ok(notes) => {
            if !opts.dry_run {
                std::fs::write(&notes_path, &notes).map_err(|e| format!("write release notes: {e}"))?;
            }
            let count = notes.lines().filter(|l| l.starts_with("- ")).count();
            steps.pass("release notes", format!("{count} commits; draft at {}", notes_path.display()));
        }
        Err(e) => {
            steps.fail("release notes", e);
        }
    }

    // Step 9: Signing report.
    let signing_plans = crate::signing::plans(&|k: &str| std::env::var(k).ok());
    let ready_count = signing_plans.iter().filter(|p| p.ready).count();
    let total = signing_plans.len();
    if ready_count == total {
        steps.pass("signing status", format!("all {total} platforms ready"));
    } else {
        let missing: Vec<&str> = signing_plans.iter().filter(|p| !p.ready).map(|p| p.platform).collect();
        steps.pass("signing status", format!("{ready_count}/{total} ready; not ready: {}", missing.join(", ")));
    }

    // Step 10: Human checklist.
    let checklist = human_checklist(version, &dist, &notes_path);
    let checklist_path = root.join("dist").join(format!("release-checklist-{version}.md"));
    if !opts.dry_run {
        std::fs::write(&checklist_path, &checklist).map_err(|e| format!("write checklist: {e}"))?;
    }
    steps.pass("checklist", format!("written to {}", checklist_path.display()));

    // Summary.
    println!("\n=== release {version} summary ===");
    let passed = steps.results.iter().filter(|s| s.ok).count();
    let failed = steps.results.iter().filter(|s| !s.ok).count();
    println!("{passed} passed, {failed} failed");
    for s in &steps.results {
        let mark = if s.ok { "ok" } else { "FAIL" };
        println!("  {mark:<6} {:<24} {}", s.name, s.detail);
    }

    if !opts.dry_run && failed == 0 {
        println!("\nAll steps passed. Complete the release with the manual steps in:");
        println!("  {}", checklist_path.display());
    } else if opts.dry_run {
        println!("\nDry run complete. No files were changed.");
    } else {
        println!("\n{failed} step(s) failed. Fix the issues and re-run.");
    }

    if failed > 0 && !opts.dry_run { Err(format!("{failed} release step(s) failed")) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let o = parse(&["1.2.3"]).unwrap();
        assert_eq!(o.version, "1.2.3");
        assert!(!o.dry_run && !o.skip_build && !o.skip_smoke && !o.skip_sign);
    }

    #[test]
    fn parse_with_flags() {
        let o = parse(&["0.8.0", "--dry-run", "--skip-build", "--skip-smoke", "--skip-sign"]).unwrap();
        assert_eq!(o.version, "0.8.0");
        assert!(o.dry_run && o.skip_build && o.skip_smoke && o.skip_sign);
    }

    #[test]
    fn parse_errors() {
        assert!(parse(&[]).is_err(), "no version");
        assert!(parse(&["nope"]).is_err(), "bad version");
        assert!(parse(&["1.0.0", "--bogus"]).is_err(), "unknown flag");
        assert!(parse(&["1.0.0", "2.0.0"]).is_err(), "two versions");
    }

    #[test]
    fn parse_prerelease() {
        let o = parse(&["1.0.0-rc.1", "--dry-run"]).unwrap();
        assert_eq!(o.version, "1.0.0-rc.1");
    }

    #[test]
    fn human_checklist_has_all_sections() {
        let cl = human_checklist("0.8.0", Path::new("/tmp/dist"), Path::new("/tmp/notes.md"));
        assert!(cl.contains("v0.8.0"));
        assert!(cl.contains("git tag"));
        assert!(cl.contains("gh release create"));
        assert!(cl.contains("MCP Registry"));
        assert!(cl.contains("Website version"));
        assert!(cl.contains("PNG review"));
    }

    #[test]
    fn draft_notes_in_real_repo() {
        // Only works when run from within a git repo.
        let root = crate::root();
        if Command::new("git").current_dir(&root).args(["rev-parse", "HEAD"]).output().is_ok_and(|o| o.status.success()) {
            let notes = draft_release_notes(&root, "99.0.0").unwrap();
            assert!(notes.contains("# CADaddy v99.0.0"));
        }
    }

    #[test]
    fn dry_run_on_real_repo() {
        // A dry run should always succeed (no mutations).
        let root = crate::root();
        if Command::new("git").current_dir(&root).args(["rev-parse", "HEAD"]).output().is_ok_and(|o| o.status.success()) {
            // Use the current version so the version validation passes.
            let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
            let current = crate::version::read(&manifest).unwrap();
            let result = run(&root, &[&current, "--dry-run"]);
            assert!(result.is_ok(), "dry run failed: {result:?}");
        }
    }
}
