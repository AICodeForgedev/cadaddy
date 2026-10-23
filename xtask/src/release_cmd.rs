//! `cargo xtask release <version> [--dry-run] [--step STEP]... [--update-from OLD] [--dist DIR]
//! [--skip-build] [--skip-smoke] [--skip-sign]`
//!
//! One-command release automation. No network writes: the publishing commands are printed in a
//! checklist, never run. The steps, in order (each runnable alone with `--step`, so each machine
//! can run its own part):
//! 1. `bump`: set the version in `Cargo.toml` + `Cargo.lock` and commit just those two files.
//! 2. `build`: `git archive` the bump commit into a clean folder (`target/release-src`) and run
//!    this platform's packaging script there (Mac DMG on a Mac, Windows MSI on Windows, Linux
//!    packages on Linux), with `CADADDY_BUILD_SHA` = that commit.
//! 3. `mcpb`: `CADaddy.mcpb` from the CLI the platform script built (its `--version` must be the
//!    release version): `manifest.json` + `server/cadaddy-cli[.exe]`, in the format `mcptest` reads.
//! 4. `smoke`: `xtask smoke` on every artifact it supports (DMG, .mcpb, the CLI inside its
//!    zip/tar.gz); every other installer is reported as NOT SMOKED. Output goes to
//!    `target/release-smoke`, never into the release folder.
//! 5. `update-gate`: `xtask update-gate --from <--update-from> --to <the new installer>`. Without
//!    `--update-from` the checklist starts with a blocking "UPDATE GATE NOT RUN" item.
//! 6. `sign`: `sign-release` + `verify-release` over the release folder (`--dist DIR`: a folder
//!    gathered from every machine, so the whole release gets one SHA256SUMS).
//! 7. `notes`: release notes drafted from the commits since the last tag.
//! 8. `checklist`: what still needs a human (the commands to run, not run by this tool).
//!
//! The run stops at the first failed step and says what to do; no later step runs, and a failed
//! run never leaves a checklist behind.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The release steps, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Bump,
    Build,
    Mcpb,
    Smoke,
    UpdateGate,
    Sign,
    Notes,
    Checklist,
}

const ALL_STEPS: [Step; 8] = [Step::Bump, Step::Build, Step::Mcpb, Step::Smoke, Step::UpdateGate, Step::Sign, Step::Notes, Step::Checklist];

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Bump => "bump",
            Step::Build => "build",
            Step::Mcpb => "mcpb",
            Step::Smoke => "smoke",
            Step::UpdateGate => "update-gate",
            Step::Sign => "sign",
            Step::Notes => "notes",
            Step::Checklist => "checklist",
        }
    }

    fn parse(s: &str) -> Option<Step> {
        ALL_STEPS.into_iter().find(|st| st.name() == s)
    }

    /// What to do when this step fails.
    fn fix(self) -> &'static str {
        match self {
            Step::Bump => "fix Cargo.toml / Cargo.lock by hand (or `git reset --hard` to drop the half-done bump) and re-run",
            Step::Build => "fix the packaging error above, then re-run with `--step build` (and the steps after it)",
            Step::Mcpb => "run `--step build` on this machine first so the CLI is built at the release version, then `--step mcpb`",
            Step::Smoke => "the artifact above is broken: fix it and rebuild; do not publish this release",
            Step::UpdateGate => "the update from the previous release fails: fix the updater and rebuild; do not publish this release",
            Step::Sign => "set CADADDY_RELEASE_KEY to the release key file and re-run `--step sign` over the release folder",
            Step::Notes => "check `git log` works in this checkout, then re-run `--step notes`",
            Step::Checklist => "check the dist/ folder is writable and re-run `--step checklist`",
        }
    }
}

/// Options parsed from the CLI.
struct Opts {
    version: String,
    dry_run: bool,
    skip_build: bool,
    skip_smoke: bool,
    skip_sign: bool,
    /// The steps asked for with `--step` (empty: all of them).
    only: Vec<Step>,
    update_from: Option<PathBuf>,
    dist: Option<PathBuf>,
}

const USAGE: &str = "usage: cargo xtask release <version> [--dry-run] [--step bump|build|mcpb|smoke|update-gate|sign|notes|checklist]... \
                     [--update-from <previous installer>] [--dist DIR] [--skip-build] [--skip-smoke] [--skip-sign]";

fn parse(args: &[&str]) -> Result<Opts, String> {
    let mut version = None;
    let mut o = Opts {
        version: String::new(),
        dry_run: false,
        skip_build: false,
        skip_smoke: false,
        skip_sign: false,
        only: Vec::new(),
        update_from: None,
        dist: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--dry-run" => o.dry_run = true,
            "--skip-build" => o.skip_build = true,
            "--skip-smoke" => o.skip_smoke = true,
            "--skip-sign" => o.skip_sign = true,
            "--step" => {
                let v = it.next().ok_or("--step needs a step name")?;
                let st = Step::parse(v).ok_or_else(|| format!("unknown step `{v}` (one of: {})", ALL_STEPS.map(Step::name).join(", ")))?;
                if !o.only.contains(&st) {
                    o.only.push(st);
                }
            }
            "--update-from" => o.update_from = Some(PathBuf::from(it.next().ok_or("--update-from needs the previous release's installer")?)),
            "--dist" => o.dist = Some(PathBuf::from(it.next().ok_or("--dist needs a folder")?)),
            s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
            s => {
                if version.replace(s.to_string()).is_some() {
                    return Err("only one version argument".into());
                }
            }
        }
    }
    o.version = version.ok_or(USAGE)?;
    crate::version::validate(&o.version)?;
    Ok(o)
}

/// Each step either runs or is skipped (with the reason), in run order.
fn plan(o: &Opts) -> Vec<(Step, Option<&'static str>)> {
    ALL_STEPS
        .into_iter()
        .map(|st| {
            let skip = if !o.only.is_empty() && !o.only.contains(&st) {
                Some("not asked for (--step)")
            } else {
                match st {
                    Step::Build | Step::Mcpb if o.skip_build => Some("skipped (--skip-build)"),
                    Step::Smoke if o.skip_smoke => Some("skipped (--skip-smoke)"),
                    Step::Sign if o.skip_sign => Some("skipped (--skip-sign)"),
                    _ => None,
                }
            };
            (st, skip)
        })
        .collect()
}

#[derive(Debug, PartialEq)]
enum Status {
    Pass,
    Fail,
    Skip,
    NotRun,
    NotSmoked,
}

/// One line of the final summary.
#[derive(Debug)]
struct StepResult {
    name: &'static str,
    status: Status,
    detail: String,
}

/// All step outcomes, for the summary.
struct Steps {
    results: Vec<StepResult>,
}

impl Steps {
    fn new() -> Self {
        Self { results: Vec::new() }
    }
    fn push(&mut self, name: &'static str, status: Status, detail: String) {
        let mark = match status {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
            Status::NotRun => "NOT RUN",
            Status::NotSmoked => "NOT SMOKED",
        };
        println!("  {mark:<10} {name:<16} {detail}");
        self.results.push(StepResult { name, status, detail });
    }
    fn pass(&mut self, name: &'static str, detail: String) {
        self.push(name, Status::Pass, detail);
    }
    fn fail(&mut self, name: &'static str, detail: String) -> String {
        self.push(name, Status::Fail, detail.clone());
        detail
    }
    fn skip(&mut self, name: &'static str, why: &str) {
        self.push(name, Status::Skip, why.to_string());
    }
    fn failed(&self) -> usize {
        self.results.iter().filter(|s| s.status == Status::Fail).count()
    }
}

/// What a step that didn't fail did.
#[derive(Debug)]
enum Outcome {
    Pass(String),
    Skip(String),
    /// Not done, and a human has to do it before publishing (e.g. the update gate).
    NotRun(String),
}

/// Run `plan` in order through `exec`, stopping at the first failure: the failed step's error and
/// what to do come back as `Err`, and no later step runs.
fn execute(
    plan: &[(Step, Option<&'static str>)],
    steps: &mut Steps,
    exec: &mut dyn FnMut(Step, &mut Steps) -> Result<Outcome, String>,
) -> Result<(), String> {
    for (i, (st, skip)) in plan.iter().enumerate() {
        if let Some(why) = skip {
            steps.skip(st.name(), why);
            continue;
        }
        match exec(*st, steps) {
            Ok(Outcome::Pass(d)) => steps.pass(st.name(), d),
            Ok(Outcome::Skip(d)) => steps.skip(st.name(), &d),
            Ok(Outcome::NotRun(d)) => steps.push(st.name(), Status::NotRun, d),
            Err(e) => {
                steps.fail(st.name(), e.clone());
                let later: Vec<&str> = plan.iter().skip(i + 1).filter(|(_, s)| s.is_none()).map(|(s, _)| s.name()).collect();
                if !later.is_empty() {
                    println!("  stopped: {} not run", later.join(", "));
                }
                return Err(format!("release step `{}` failed: {e}\nWhat to do: {}", st.name(), st.fix()));
            }
        }
    }
    Ok(())
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

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_cmd(Command::new("git").current_dir(root).args(args)).map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// Is the git working tree clean?
fn git_clean(root: &Path) -> Result<bool, String> {
    Ok(git(root, &["status", "--porcelain"])?.is_empty())
}

/// The HEAD commit SHA.
fn head_sha(root: &Path) -> Result<String, String> {
    git(root, &["rev-parse", "HEAD"])
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

/// Commit the version bump (just `Cargo.toml` and `Cargo.lock`); returns the new HEAD.
fn commit_bump(root: &Path, version: &str) -> Result<String, String> {
    git(root, &["add", "--", "Cargo.toml", "Cargo.lock"])?;
    git(root, &["commit", "-q", "-m", &format!("release: bump version to {version}"), "--", "Cargo.toml", "Cargo.lock"])?;
    head_sha(root)
}

/// The version in `Cargo.toml` at commit `sha`.
fn version_at(root: &Path, sha: &str) -> Result<String, String> {
    crate::version::read(&git(root, &["show", &format!("{sha}:Cargo.toml")])?)
}

/// Export commit `sha` with `git archive` into a fresh `dest` (only committed files, no `.git`).
fn export_source(root: &Path, sha: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| format!("clear {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut archive = Command::new("git")
        .current_dir(root)
        .args(["archive", "--format=tar", sha])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git archive: {e}"))?;
    let tar_in = archive.stdout.take().ok_or("git archive: no output")?;
    let untar = Command::new("tar").arg("-x").arg("-C").arg(dest).stdin(tar_in).status().map_err(|e| format!("tar: {e}"))?;
    let archived = archive.wait().map_err(|e| format!("git archive: {e}"))?;
    if !archived.success() || !untar.success() {
        return Err(format!("git archive {sha} | tar -x -C {} failed ({archived}, {untar})", dest.display()));
    }
    Ok(())
}

/// How the platform build runs: the packaging script inside the exported source.
#[derive(Debug)]
struct BuildPlan {
    program: &'static str,
    args: Vec<PathBuf>,
    cwd: PathBuf,
    env: Vec<(&'static str, String)>,
}

fn build_plan(platform: &str, src: &Path, version: &str, sha: &str, dist: &Path, target_dir: &Path) -> Result<BuildPlan, String> {
    let (program, args) = match platform {
        "macos" => ("bash", vec![src.join("packaging/macos/package.sh")]),
        "windows" => ("pwsh", vec![PathBuf::from("-File"), src.join("packaging/windows/package.ps1")]),
        "linux" => ("bash", vec![src.join("packaging/linux/package.sh")]),
        other => return Err(format!("unsupported platform: {other}")),
    };
    let env = vec![
        ("CADADDY_VERSION", version.to_string()),
        ("CADADDY_BUILD_SHA", sha.to_string()),
        ("DIST", dist.to_string_lossy().into_owned()),
        ("CARGO_TARGET_DIR", target_dir.to_string_lossy().into_owned()),
    ];
    Ok(BuildPlan { program, args, cwd: src.to_path_buf(), env })
}

fn run_build(p: &BuildPlan) -> Result<(), String> {
    let mut cmd = Command::new(p.program);
    cmd.args(&p.args).current_dir(&p.cwd);
    for (k, v) in &p.env {
        cmd.env(k, v);
    }
    let status = cmd.status().map_err(|e| format!("{}: {e}", p.program))?;
    if status.success() { Ok(()) } else { Err(format!("{} exited {status}", p.args.last().map(|a| a.display().to_string()).unwrap_or_default())) }
}

/// Where this platform's packaging script leaves the CLI it packaged.
fn cli_path(platform: &str, target_dir: &Path, cross_target: Option<&str>) -> PathBuf {
    match platform {
        // package.sh lipos the per-target builds into one universal binary here.
        "macos" => target_dir.join("macos-package").join("bin").join("cadaddy-cli"),
        // package.ps1 builds `--target x86_64-pc-windows-msvc` (its default -Arch x64).
        "windows" => target_dir.join("x86_64-pc-windows-msvc").join("release").join("cadaddy-cli.exe"),
        _ => match cross_target {
            Some(t) => target_dir.join(t).join("release").join("cadaddy-cli"),
            None => target_dir.join("release").join("cadaddy-cli"),
        },
    }
}

/// Does `cadaddy-cli --version`'s output name exactly `version`?
fn cli_version_ok(output: &str, version: &str) -> bool {
    output.split_whitespace().any(|w| w.trim_start_matches('v') == version)
}

/// The CLI for the .mcpb: present, and its `--version` is the release version.
fn release_cli(platform: &str, target_dir: &Path, version: &str) -> Result<PathBuf, String> {
    let cross = std::env::var("CROSS_TARGET").ok().filter(|t| !t.is_empty());
    let cli = cli_path(platform, target_dir, cross.as_deref());
    if !cli.is_file() {
        return Err(format!("{} not found; the platform build step makes it", cli.display()));
    }
    let out = run_cmd(Command::new(&cli).arg("--version")).map_err(|e| format!("{} --version: {e}", cli.display()))?;
    if !cli_version_ok(&out, version) {
        return Err(format!("{} --version says `{out}`, not {version} (a stale build?)", cli.display()));
    }
    Ok(cli)
}

/// The .mcpb manifest, in the format `mcptest` reads (`server.mcp_config.command`).
fn mcpb_manifest(version: &str, platform: &str) -> serde_json::Value {
    let exe = if platform == "windows" { "cadaddy-cli.exe" } else { "cadaddy-cli" };
    let mcpb_platform = match platform {
        "macos" => "darwin",
        "windows" => "win32",
        _ => "linux",
    };
    serde_json::json!({
        "manifest_version": "0.2",
        "name": "cadaddy",
        "display_name": "CADaddy",
        "version": version,
        "description": "CADaddy MCP server for computer-aided design",
        "author": { "name": "AICodeForge", "email": "feedback@aicodeforge.dev" },
        "server": {
            "type": "binary",
            "entry_point": format!("server/{exe}"),
            "mcp_config": {
                "command": format!("${{__dirname}}/server/{exe}"),
                "args": ["mcp"]
            }
        },
        "compatibility": { "platforms": [mcpb_platform] }
    })
}

/// Build `CADaddy.mcpb` in `dist` from `cli`: a zip holding exactly `manifest.json` and
/// `server/cadaddy-cli[.exe]`.
fn build_mcpb(cli: &Path, version: &str, platform: &str, dist: &Path) -> Result<PathBuf, String> {
    let exe = if platform == "windows" { "cadaddy-cli.exe" } else { "cadaddy-cli" };
    let bin = std::fs::read(cli).map_err(|e| format!("read {}: {e}", cli.display()))?;
    let manifest = serde_json::to_string_pretty(&mcpb_manifest(version, platform)).map_err(|e| e.to_string())?;
    let zip = zip_stored(&[("manifest.json", manifest.as_bytes(), 0o644), (&format!("server/{exe}"), &bin, 0o755)])?;
    std::fs::create_dir_all(dist).map_err(|e| format!("create {}: {e}", dist.display()))?;
    let path = dist.join("CADaddy.mcpb");
    std::fs::write(&path, zip).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table.get(((crc ^ u32::from(b)) & 0xFF) as usize).copied().unwrap_or(0) ^ (crc >> 8);
    }
    !crc
}

/// A zip of uncompressed ("stored") entries: (name, data, unix mode). Fixed 1980-01-01 timestamps,
/// so the same inputs give the same bytes.
fn zip_stored(entries: &[(&str, &[u8], u32)]) -> Result<Vec<u8>, String> {
    const DOS_DATE: u16 = 0x21; // 1980-01-01
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, mode) in entries {
        let size = u32::try_from(data.len()).map_err(|_| format!("{name} is too big for a zip"))?;
        let name_len = u16::try_from(name.len()).map_err(|_| format!("{name}: name too long"))?;
        let offset = u32::try_from(out.len()).map_err(|_| "the zip is too big".to_string())?;
        let crc = crc32(data);
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Central directory entry (made by Unix, so the mode is kept).
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&0x031Eu16.to_le_bytes()); // made by: Unix, zip 3.0
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&((0o100_000 | mode) << 16).to_le_bytes()); // external: regular file + mode
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = u32::try_from(out.len()).map_err(|_| "the zip is too big".to_string())?;
    let cd_size = u32::try_from(central.len()).map_err(|_| "the zip is too big".to_string())?;
    let count = u16::try_from(entries.len()).map_err(|_| "too many zip entries".to_string())?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// Artifacts `xtask smoke` can't test here: (file name, why).
type NotSmoked = Vec<(String, String)>;

/// How an artifact is smoked.
#[derive(Debug, PartialEq)]
enum SmokeHow {
    /// `xtask smoke <file>` directly (DMG, .mcpb, a bare CLI).
    Direct,
    /// Unpack the archive and smoke the `cadaddy-cli` inside.
    CliArchive,
}

/// Split the release folder's file names into the ones `xtask smoke` can test on `host` and the
/// ones it can't (with the reason). Checksums, signatures and delta files aren't artifacts.
fn select_smoke(names: &[String], host: &str) -> (Vec<(String, SmokeHow)>, NotSmoked) {
    let mut smoked = Vec::new();
    let mut not = Vec::new();
    let built_for = |n: &str| ["macos", "windows", "linux"].into_iter().find(|p| n.contains(&format!("-{p}-")));
    for name in names {
        let n = name.to_ascii_lowercase();
        if n.starts_with('.') || n == "sha256sums" || n == "sha256sums.sig" || n.ends_with(".zsync") || n.ends_with(".md") {
            continue;
        }
        let is_cli_archive = (n.starts_with("cadaddy-cli-") && (n.ends_with(".zip") || n.ends_with(".tar.gz")))
            || (n.contains("-windows-") && n.ends_with("-portable.zip"))
            || (n.contains("-linux-") && n.ends_with(".tar.gz"));
        if n.ends_with(".dmg") {
            if host == "macos" {
                smoked.push((name.clone(), SmokeHow::Direct));
            } else {
                not.push((name.clone(), "a DMG is smoked on a Mac".into()));
            }
        } else if n.ends_with(".mcpb") || n == "cadaddy-cli" || n == "cadaddy-cli.exe" {
            smoked.push((name.clone(), SmokeHow::Direct));
        } else if is_cli_archive {
            match built_for(&n) {
                Some(p) if p != host => not.push((name.clone(), format!("built for {p}; smoke it there"))),
                _ => smoked.push((name.clone(), SmokeHow::CliArchive)),
            }
        } else {
            let kind = n.rsplit('.').next().unwrap_or_default();
            not.push((name.clone(), format!("xtask smoke can't test .{kind} installers; install and check it by hand")));
        }
    }
    (smoked, not)
}

/// The file names in `dir` (sorted), or none if it doesn't exist.
fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).filter_map(|e| e.file_name().to_str().map(String::from)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Find `cadaddy-cli[.exe]` under `dir` (a few levels deep, as release archives nest it).
fn find_cli(dir: &Path, depth: usize) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    for p in &entries {
        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if p.is_file() && (n == "cadaddy-cli" || n == "cadaddy-cli.exe") {
            return Some(p.clone());
        }
    }
    if depth == 0 {
        return None;
    }
    entries.iter().filter(|p| p.is_dir()).find_map(|p| find_cli(p, depth - 1))
}

/// Smoke one artifact, writing under `out` (outside the release folder).
fn smoke_one(root: &Path, file: &Path, how: &SmokeHow, version: &str, out: &Path) -> Result<(), String> {
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(|e| format!("clear {}: {e}", out.display()))?;
    }
    std::fs::create_dir_all(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let target = match how {
        SmokeHow::Direct => file.to_path_buf(),
        SmokeHow::CliArchive => {
            let un = out.join("unpacked");
            std::fs::create_dir_all(&un).map_err(|e| e.to_string())?;
            let gz = file.to_string_lossy().to_ascii_lowercase().ends_with(".tar.gz");
            let ok = if gz {
                Command::new("tar").arg("-xzf").arg(file).arg("-C").arg(&un).status().is_ok_and(|s| s.success())
            } else {
                Command::new("tar").arg("-xf").arg(file).arg("-C").arg(&un).status().is_ok_and(|s| s.success())
                    || Command::new("unzip").arg("-oq").arg(file).arg("-d").arg(&un).status().is_ok_and(|s| s.success())
            };
            if !ok {
                return Err(format!("couldn't unpack {}", file.display()));
            }
            let cli = find_cli(&un, 4).ok_or_else(|| format!("no cadaddy-cli inside {}", file.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755));
            }
            cli
        }
    };
    crate::smoke::run(root, &[&target.to_string_lossy(), "--version", version, "--out", &out.join("report").to_string_lossy()])
}

/// The new installer the update gate updates to: this platform's one in `names`.
fn gate_installer(names: &[String], host: &str) -> Result<String, String> {
    let ext = match host {
        "macos" => ".dmg",
        "windows" => ".msi",
        _ => ".appimage",
    };
    let found: Vec<&String> = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(ext)).collect();
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no {ext} installer in the release folder to update to; run --step build first")),
        many => Err(format!("more than one {ext} installer: {}", many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// The record a passed update gate leaves (`dist/update-gate-<version>.txt`), so a later
/// `--step checklist` knows it ran.
fn gate_record_path(out: &Path, version: &str) -> PathBuf {
    out.join(format!("update-gate-{version}.txt"))
}

/// The human checklist: what still needs a person. `update_gate` is the passed gate's record
/// (None: not run, which blocks the release); `not_smoked` are the artifacts to test by hand.
fn human_checklist(version: &str, dist: &Path, notes_path: &Path, update_gate: Option<&str>, not_smoked: &[(String, String)]) -> String {
    let tag = format!("v{version}");
    let mut checklist = String::new();
    checklist.push_str(&format!("# Release v{version} — remaining manual steps\n\n"));

    let mut n = 0;
    let mut section = |checklist: &mut String, title: &str| {
        n += 1;
        checklist.push_str(&format!("## {n}. {title}\n"));
    };

    if update_gate.is_none() {
        let to =
            gate_installer(&dir_names(dist), current_platform()).map(|i| dist.join(i).display().to_string()).unwrap_or("<the new installer>".into());
        section(&mut checklist, "BLOCKING: UPDATE GATE NOT RUN");
        checklist.push_str("Do not publish until the update from the previous release passes:\n");
        checklist.push_str(&format!("```\ncargo xtask update-gate --from <the previous release's installer> --to {to}\n```\n"));
        checklist.push_str(&format!("(or re-run `cargo xtask release {version} --step update-gate --update-from <previous installer>`)\n\n"));
    }

    if !not_smoked.is_empty() {
        section(&mut checklist, "NOT SMOKED: test these by hand");
        for (name, why) in not_smoked {
            checklist.push_str(&format!("- [ ] NOT SMOKED {name}: {why}\n"));
        }
        checklist.push('\n');
    }

    section(&mut checklist, "Review the version bump commit");
    checklist.push_str("```\ngit show --stat HEAD\n```\n\n");

    section(&mut checklist, "Tag the release");
    checklist.push_str(&format!("```\ngit tag -a {tag} -m \"CADaddy {tag}\"\n```\n\n"));

    section(&mut checklist, "Push");
    checklist.push_str(&format!("```\ngit push origin main\ngit push origin {tag}\n```\n\n"));

    section(&mut checklist, "Create the GitHub Release");
    checklist.push_str(&format!(
        "```\ngh release create {tag} {}/* --title \"CADaddy {tag}\" --notes-file {}\n```\n",
        dist.display(),
        notes_path.display()
    ));
    checklist.push_str("(Review the release notes before publishing.)\n\n");

    section(&mut checklist, "MCP Registry");
    checklist.push_str(&format!("Set the version in server.json to {version} (and the CADaddy.mcpb sha256 from SHA256SUMS), then:\n"));
    checklist.push_str("```\nmcp-publisher login github\nmcp-publisher publish\n```\n\n");

    section(&mut checklist, "Website version");
    checklist.push_str(&format!("Update aicodeforge.dev and its llms.txt to version {version}.\n\n"));

    section(&mut checklist, "PNG review");
    checklist.push_str("Check every new screenshot/golden PNG in the release for personal info.\n\n");

    section(&mut checklist, "Announce");
    checklist.push_str("Post the release announcement.\n");

    checklist
}

/// What the steps share.
struct Ctx<'a> {
    root: &'a Path,
    opts: &'a Opts,
    version: String,
    current: String,
    sha: String,
    /// The release artifacts folder (default `dist/release`, or `--dist`).
    dist: PathBuf,
    /// Where notes, the checklist and the gate record go (`dist/`).
    out: PathBuf,
    target_dir: PathBuf,
}

impl Ctx<'_> {
    fn notes_path(&self) -> PathBuf {
        self.out.join(format!("release-notes-{}.md", self.version))
    }
    fn checklist_path(&self) -> PathBuf {
        self.out.join(format!("release-checklist-{}.md", self.version))
    }
}

fn exec_step(c: &mut Ctx, st: Step, steps: &mut Steps) -> Result<Outcome, String> {
    let dry = c.opts.dry_run;
    let v = c.version.clone();
    let host = current_platform();
    match st {
        Step::Bump => {
            if c.current == v {
                return Ok(Outcome::Pass(format!("already at {v}")));
            }
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would set {} -> {v} and commit Cargo.toml + Cargo.lock", c.current)));
            }
            crate::version::run(c.root, &["set", &v])?;
            c.sha = commit_bump(c.root, &v)?;
            Ok(Outcome::Pass(format!("{} -> {v}, committed {}", c.current, c.sha.get(..12).unwrap_or(&c.sha))))
        }
        Step::Build => {
            let src = c.target_dir.join("release-src");
            let short = c.sha.get(..12).unwrap_or(&c.sha).to_string();
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would git archive {short} into {} and run the {host} packaging script", src.display())));
            }
            let at = version_at(c.root, &c.sha)?;
            if at != v {
                return Err(format!("commit {short} is at version {at}, not {v}; run `--step bump` first"));
            }
            export_source(c.root, &c.sha, &src)?;
            let p = build_plan(host, &src, &v, &c.sha, &c.dist, &c.target_dir)?;
            run_build(&p)?;
            Ok(Outcome::Pass(format!("{} file(s) in {}, built from git archive {short}", dir_names(&c.dist).len(), c.dist.display())))
        }
        Step::Mcpb => {
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would package {} into CADaddy.mcpb", cli_path(host, &c.target_dir, None).display())));
            }
            let cli = release_cli(host, &c.target_dir, &v)?;
            let p = build_mcpb(&cli, &v, host, &c.dist)?;
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Ok(Outcome::Pass(format!("{} ({size} bytes) from {}", p.display(), cli.display())))
        }
        Step::Smoke => {
            let (smoked, not) = select_smoke(&dir_names(&c.dist), host);
            for (name, why) in &not {
                steps.push("smoke", Status::NotSmoked, format!("{name}: {why}"));
            }
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would smoke {} artifact(s)", smoked.len())));
            }
            if smoked.is_empty() {
                return Err(format!("nothing in {} that xtask smoke can test; run --step build first", c.dist.display()));
            }
            for (name, how) in &smoked {
                let out = c.target_dir.join("release-smoke").join(name.replace('.', "-"));
                smoke_one(c.root, &c.dist.join(name), how, &v, &out).map_err(|e| format!("{name}: {e}"))?;
                steps.pass("smoke", format!("{name}: passed"));
            }
            Ok(Outcome::Pass(format!("{} smoked, {} not smoked", smoked.len(), not.len())))
        }
        Step::UpdateGate => {
            let Some(from) = &c.opts.update_from else {
                if dry {
                    return Ok(Outcome::Skip("dry run (pass --update-from <previous installer> to run it)".into()));
                }
                return Ok(Outcome::NotRun("UPDATE GATE NOT RUN: no --update-from; the checklist blocks the release".into()));
            };
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would run update-gate --from {}", from.display())));
            }
            let to = c.dist.join(gate_installer(&dir_names(&c.dist), host)?);
            crate::updategate::run(c.root, &["--from", &from.to_string_lossy(), "--to", &to.to_string_lossy()])?;
            std::fs::create_dir_all(&c.out).map_err(|e| e.to_string())?;
            let record = format!("update gate passed: --from {} --to {}\n", from.display(), to.display());
            std::fs::write(gate_record_path(&c.out, &v), &record).map_err(|e| format!("write the update gate record: {e}"))?;
            Ok(Outcome::Pass(record.trim().to_string()))
        }
        Step::Sign => {
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would sign and verify {}", c.dist.display())));
            }
            crate::release::sign_cmd(c.root, &[&c.dist.to_string_lossy(), "--version", &v])?;
            Ok(Outcome::Pass(format!("one SHA256SUMS + SHA256SUMS.sig for {}, verified", c.dist.display())))
        }
        Step::Notes => {
            let notes = draft_release_notes(c.root, &v)?;
            let count = notes.lines().filter(|l| l.starts_with("- ")).count();
            let path = c.notes_path();
            if dry {
                return Ok(Outcome::Skip(format!("dry run: {count} commits; would write {}", path.display())));
            }
            std::fs::create_dir_all(&c.out).map_err(|e| format!("create {}: {e}", c.out.display()))?;
            std::fs::write(&path, &notes).map_err(|e| format!("write release notes: {e}"))?;
            Ok(Outcome::Pass(format!("{count} commits; draft at {}", path.display())))
        }
        Step::Checklist => {
            let path = c.checklist_path();
            if dry {
                return Ok(Outcome::Skip(format!("dry run: would write {}", path.display())));
            }
            let gate = std::fs::read_to_string(gate_record_path(&c.out, &v)).ok();
            let (_, not) = select_smoke(&dir_names(&c.dist), host);
            let checklist = human_checklist(&v, &c.dist, &c.notes_path(), gate.as_deref(), &not);
            std::fs::create_dir_all(&c.out).map_err(|e| format!("create {}: {e}", c.out.display()))?;
            std::fs::write(&path, &checklist).map_err(|e| format!("write checklist: {e}"))?;
            let blocking = if gate.is_none() { " (starts with BLOCKING: UPDATE GATE NOT RUN)" } else { "" };
            Ok(Outcome::Pass(format!("written to {}{blocking}", path.display())))
        }
    }
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let opts = parse(args)?;
    let version = opts.version.clone();
    println!("=== cargo xtask release {version}{} ===", if opts.dry_run { " (dry run)" } else { "" });
    let mut steps = Steps::new();
    let plan = plan(&opts);
    let runs = |st: Step| plan.iter().any(|(s, skip)| *s == st && skip.is_none());

    let sha = match head_sha(root) {
        Ok(s) => {
            steps.pass("git sha", s.chars().take(12).collect());
            s
        }
        Err(e) => return Err(steps.fail("git sha", format!("not a git repo or no commits: {e}"))),
    };

    // The bump commits and the build exports a commit: both need a clean tree.
    if !opts.dry_run && (runs(Step::Bump) || runs(Step::Build)) {
        match git_clean(root) {
            Ok(true) => steps.pass("git clean", "working tree is clean".into()),
            Ok(false) => return Err(steps.fail("git clean", "uncommitted changes; commit or stash first".into())),
            Err(e) => return Err(steps.fail("git clean", e)),
        }
    } else {
        steps.skip("git clean", if opts.dry_run { "dry run" } else { "not needed for these steps" });
    }

    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let current = crate::version::read(&manifest)?;
    steps.pass("current version", current.clone());

    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).map_or_else(|| root.join("target"), |t| root.join(t));
    let out = root.join("dist");
    let dist = opts.dist.as_ref().map_or_else(|| out.join("release"), |d| root.join(d));
    let mut ctx = Ctx { root, opts: &opts, version: version.clone(), current, sha, dist, out, target_dir };

    let result = execute(&plan, &mut steps, &mut |st, steps| exec_step(&mut ctx, st, steps));

    // Signing report (read only).
    let signing_plans = crate::signing::plans(&|k: &str| std::env::var(k).ok());
    let ready_count = signing_plans.iter().filter(|p| p.ready).count();
    let total = signing_plans.len();
    if ready_count == total {
        steps.pass("signing status", format!("all {total} platforms ready"));
    } else {
        let missing: Vec<&str> = signing_plans.iter().filter(|p| !p.ready).map(|p| p.platform).collect();
        steps.pass("signing status", format!("{ready_count}/{total} ready; not ready: {}", missing.join(", ")));
    }

    // A failed run leaves no checklist that says to publish.
    let checklist_path = ctx.checklist_path();
    if result.is_err() && !opts.dry_run && checklist_path.exists() {
        match std::fs::remove_file(&checklist_path) {
            Ok(()) => println!("  removed the stale {}", checklist_path.display()),
            Err(e) => println!("  WARNING: couldn't remove the stale {}: {e}; don't follow it", checklist_path.display()),
        }
    }

    println!("\n=== release {version} summary ===");
    let failed = steps.failed();
    let passed = steps.results.iter().filter(|s| s.status == Status::Pass).count();
    println!("{passed} passed, {failed} failed");
    for s in &steps.results {
        let mark = match s.status {
            Status::Pass => "ok",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
            Status::NotRun => "NOT RUN",
            Status::NotSmoked => "NOT SMOKED",
        };
        println!("  {mark:<10} {:<16} {}", s.name, s.detail);
    }

    match &result {
        Err(e) => println!("\nRelease stopped. {e}"),
        Ok(()) if opts.dry_run => println!("\nDry run complete. No files were changed."),
        Ok(()) if runs(Step::Checklist) => {
            println!("\nAll steps passed. Complete the release with the manual steps in:");
            println!("  {}", checklist_path.display());
        }
        Ok(()) => println!("\nThe steps asked for passed."),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn parse_basic() {
        let o = parse(&["1.2.3"]).unwrap();
        assert_eq!(o.version, "1.2.3");
        assert!(!o.dry_run && !o.skip_build && !o.skip_smoke && !o.skip_sign);
        assert!(o.only.is_empty() && o.update_from.is_none() && o.dist.is_none());
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
        assert!(parse(&["1.0.0", "--step"]).is_err(), "--step without a name");
        assert!(parse(&["1.0.0", "--step", "publish"]).is_err(), "unknown step");
        assert!(parse(&["1.0.0", "--update-from"]).is_err(), "--update-from without a file");
        assert!(parse(&["1.0.0", "--dist"]).is_err(), "--dist without a folder");
    }

    #[test]
    fn parse_prerelease() {
        let o = parse(&["1.0.0-rc.1", "--dry-run"]).unwrap();
        assert_eq!(o.version, "1.0.0-rc.1");
    }

    #[test]
    fn parse_steps_and_plan() {
        let o =
            parse(&["1.0.0", "--step", "sign", "--step", "notes", "--step", "sign", "--dist", "all-machines", "--update-from", "old.dmg"]).unwrap();
        assert_eq!(o.only, vec![Step::Sign, Step::Notes]);
        assert_eq!(o.dist, Some(PathBuf::from("all-machines")));
        assert_eq!(o.update_from, Some(PathBuf::from("old.dmg")));
        let p = plan(&o);
        let run: Vec<Step> = p.iter().filter(|(_, s)| s.is_none()).map(|(s, _)| *s).collect();
        assert_eq!(run, vec![Step::Sign, Step::Notes], "run order stays the release order");
        assert_eq!(p.len(), ALL_STEPS.len(), "every other step shows as skipped");
        for name in ["bump", "build", "mcpb", "smoke", "update-gate", "sign", "notes", "checklist"] {
            assert_eq!(Step::parse(name).map(Step::name), Some(name));
        }
        let all = plan(&parse(&["1.0.0", "--skip-build"]).unwrap());
        assert!(all.iter().any(|(s, skip)| *s == Step::Mcpb && skip.is_some()));
        assert!(all.iter().any(|(s, skip)| *s == Step::Smoke && skip.is_none()));
    }

    /// MUST 1: a failing step stops the run; nothing after it runs.
    #[test]
    fn stops_at_the_first_failed_step() {
        let p = plan(&parse(&["1.0.0"]).unwrap());
        for failing in [Step::Mcpb, Step::Smoke, Step::UpdateGate, Step::Sign, Step::Notes] {
            let mut ran = Vec::new();
            let mut steps = Steps::new();
            let r = execute(&p, &mut steps, &mut |st, _| {
                ran.push(st);
                if st == failing { Err("injected failure".into()) } else { Ok(Outcome::Pass("ok".into())) }
            });
            let e = r.expect_err("a failed step must fail the run");
            assert!(e.contains(failing.name()) && e.contains("injected failure") && e.contains("What to do:"), "{e}");
            let at = ALL_STEPS.iter().position(|s| *s == failing).unwrap();
            assert_eq!(ran, ALL_STEPS[..=at].to_vec(), "no step after {} ran", failing.name());
            assert!(!ran.contains(&Step::Checklist), "no checklist after a failed {}", failing.name());
            assert_eq!(steps.failed(), 1);
        }
    }

    /// A temp git repo with a workspace manifest at version 0.1.0 (made-up content).
    fn fixture(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("cadaddy-release-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(d.join("Cargo.lock"), "version = 4\n").unwrap();
        std::fs::write(d.join(".gitignore"), "/dist/\n/target/\n").unwrap();
        std::fs::write(d.join("README.md"), "Sample project\n").unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Sample Builder"],
            &["config", "user.email", "builder@example.com"],
            &["config", "commit.gpgsign", "false"],
            &["add", "."],
            &["commit", "-q", "-m", "first commit"],
        ] {
            git(&d, args).unwrap();
        }
        d
    }

    /// Every file under `d` but `.git`, relative.
    fn tree(d: &Path) -> Vec<String> {
        fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut v = Vec::new();
        walk(d, d, &mut v);
        v.sort();
        v
    }

    /// MUST 1 end to end: a failing smoke (nothing built) stops before the checklist, and a stale
    /// checklist for that version is removed.
    #[test]
    fn failed_run_leaves_no_checklist() {
        let d = fixture("fail");
        std::fs::create_dir_all(d.join("dist")).unwrap();
        std::fs::write(d.join("dist/release-checklist-0.1.0.md"), "old: gh release create").unwrap();
        let r = run(&d, &["0.1.0", "--step", "smoke", "--step", "notes", "--step", "checklist"]);
        let e = r.expect_err("smoke with nothing built must fail");
        assert!(e.contains("smoke") && e.contains("What to do:"), "{e}");
        assert!(!d.join("dist/release-checklist-0.1.0.md").exists(), "no checklist after a failed run");
        assert!(!d.join("dist/release-notes-0.1.0.md").exists(), "notes run after smoke, so they didn't run");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A stored zip's entries: (name, data, unix mode), read from the central directory.
    fn read_zip(z: &[u8]) -> Vec<(String, Vec<u8>, u32)> {
        let u16at = |i: usize| u16::from_le_bytes([z[i], z[i + 1]]) as usize;
        let u32at = |i: usize| u32::from_le_bytes([z[i], z[i + 1], z[i + 2], z[i + 3]]) as usize;
        let eocd = (0..z.len() - 21).rev().find(|&i| u32at(i) == 0x0605_4b50).expect("end of central directory");
        let (count, mut at) = (u16at(eocd + 10), u32at(eocd + 16));
        let mut v = Vec::new();
        for _ in 0..count {
            assert_eq!(u32at(at), 0x0201_4b50);
            assert_eq!(u16at(at + 10), 0, "stored");
            let (size, nlen, xlen, clen) = (u32at(at + 24), u16at(at + 28), u16at(at + 30), u16at(at + 32));
            let mode = (u32at(at + 38) >> 16) as u32 & 0o7777;
            let local = u32at(at + 42);
            let name = String::from_utf8(z[at + 46..at + 46 + nlen].to_vec()).unwrap();
            let data_at = local + 30 + u16at(local + 26) + u16at(local + 28);
            let data = z[data_at..data_at + size].to_vec();
            assert_eq!(crc32(&data), u32at(at + 16) as u32, "crc of {name}");
            v.push((name, data, mode));
            at += 46 + nlen + xlen + clen;
        }
        v
    }

    /// MUST 2: the .mcpb holds exactly manifest.json + server/cadaddy-cli, and mcptest's manifest
    /// reading finds the server there.
    #[test]
    fn mcpb_layout_and_manifest() {
        let d = fixture("mcpb");
        for platform in ["linux", "macos", "windows"] {
            let exe = if platform == "windows" { "cadaddy-cli.exe" } else { "cadaddy-cli" };
            let cli = d.join("fake-cli");
            std::fs::write(&cli, b"#!/bin/sh\necho cadaddy-cli 9.8.7\n").unwrap();
            let dist = d.join("dist").join(platform);
            let path = build_mcpb(&cli, "9.8.7", platform, &dist).unwrap();
            assert_eq!(path, dist.join("CADaddy.mcpb"));
            let entries = read_zip(&std::fs::read(&path).unwrap());
            let names: Vec<&str> = entries.iter().map(|(n, ..)| n.as_str()).collect();
            assert_eq!(names, vec!["manifest.json".to_string(), format!("server/{exe}")], "{platform}");
            let (_, bin, mode) = &entries[1];
            assert_eq!(bin, &std::fs::read(&cli).unwrap());
            assert_eq!(*mode, 0o755, "the server stays executable");
            // Unpack and resolve it the way `cargo xtask mcptest --mcpb` does.
            let un = d.join("unpacked").join(platform);
            for (name, data, _) in &entries {
                let p = un.join(name);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, data).unwrap();
            }
            let manifest: serde_json::Value = serde_json::from_slice(&entries[0].1).unwrap();
            let (server, args) = crate::mcptest::mcpb_server(&manifest, &un);
            assert_eq!(server, PathBuf::from(format!("{}/server/{exe}", un.to_string_lossy())), "{platform}");
            assert!(server.is_file(), "{platform}: {}", server.display());
            assert_eq!(args, vec!["mcp".to_string()]);
            assert_eq!(manifest["version"], "9.8.7");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MUST 2: the CLI comes from where each platform script builds it, and must be this version.
    #[test]
    fn cli_found_where_the_platform_builds_it() {
        let t = Path::new("tgt");
        assert_eq!(cli_path("macos", t, None), t.join("macos-package/bin/cadaddy-cli"));
        assert_eq!(cli_path("windows", t, None), t.join("x86_64-pc-windows-msvc/release/cadaddy-cli.exe"));
        assert_eq!(cli_path("linux", t, None), t.join("release/cadaddy-cli"));
        assert_eq!(cli_path("linux", t, Some("riscv64gc-unknown-linux-gnu")), t.join("riscv64gc-unknown-linux-gnu/release/cadaddy-cli"));
        assert!(cli_version_ok("cadaddy-cli 0.7.7\n", "0.7.7"));
        assert!(!cli_version_ok("cadaddy-cli 0.7.6", "0.7.7"), "a stale CLI is refused");
        assert!(!cli_version_ok("cadaddy-cli 0.7.77", "0.7.7"));
        let missing = std::env::temp_dir().join(format!("cadaddy-release-nocli-{}", std::process::id()));
        let e = release_cli(current_platform(), &missing, "0.7.7").unwrap_err();
        assert!(e.contains("not found"), "{e}");
    }

    /// MUST 3: without --update-from the checklist starts with a blocking UPDATE GATE NOT RUN item
    /// carrying the exact command; a passed gate removes it.
    #[test]
    fn checklist_blocks_without_update_gate() {
        let cl = human_checklist("0.8.0", Path::new("dist-x"), Path::new("notes.md"), None, &[]);
        let first = cl.lines().find(|l| l.starts_with("## ")).unwrap();
        assert_eq!(first, "## 1. BLOCKING: UPDATE GATE NOT RUN");
        assert!(cl.contains("cargo xtask update-gate --from <the previous release's installer> --to "), "{cl}");
        let ok = human_checklist("0.8.0", Path::new("dist-x"), Path::new("notes.md"), Some("passed"), &[]);
        assert!(!ok.contains("UPDATE GATE NOT RUN"));
        assert!(ok.lines().find(|l| l.starts_with("## ")).unwrap().contains("Review the version bump"));
        assert_eq!(gate_installer(&["a.msi".into(), "cadaddy-1-macos-universal.dmg".into()], "macos").unwrap(), "cadaddy-1-macos-universal.dmg");
        assert!(gate_installer(&["a.deb".into()], "linux").is_err());
    }

    /// MUST 3 through the run: no --update-from leaves NOT RUN in the summary and the blocking
    /// first item in the written checklist.
    #[test]
    fn run_without_update_from_writes_blocking_checklist() {
        let d = fixture("gate");
        run(&d, &["0.1.0", "--step", "update-gate", "--step", "checklist"]).unwrap();
        let cl = std::fs::read_to_string(d.join("dist/release-checklist-0.1.0.md")).unwrap();
        assert_eq!(cl.lines().find(|l| l.starts_with("## ")), Some("## 1. BLOCKING: UPDATE GATE NOT RUN"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MUST 4: the build runs in a git archive export of the bump commit, with that commit's SHA.
    #[test]
    fn build_uses_git_archive_of_the_bump_commit() {
        let d = fixture("archive");
        std::fs::write(d.join("Cargo.toml"), "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.2.0\"\n").unwrap();
        let sha = commit_bump(&d, "0.2.0").unwrap();
        assert_eq!(sha, git(&d, &["rev-parse", "HEAD"]).unwrap());
        assert!(git_clean(&d).unwrap(), "the bump is committed");
        assert_eq!(version_at(&d, &sha).unwrap(), "0.2.0");
        // Uncommitted work must not reach the build.
        std::fs::write(d.join("README.md"), "uncommitted edit\n").unwrap();
        std::fs::write(d.join("scratch.txt"), "not in git\n").unwrap();
        let src = d.join("target/release-src");
        export_source(&d, &sha, &src).unwrap();
        assert_eq!(std::fs::read_to_string(src.join("README.md")).unwrap(), "Sample project\n");
        assert!(!src.join("scratch.txt").exists() && !src.join(".git").exists());
        assert!(std::fs::read_to_string(src.join("Cargo.toml")).unwrap().contains("0.2.0"));
        let p = build_plan("linux", &src, "0.2.0", &sha, &d.join("dist/release"), &d.join("target")).unwrap();
        assert_eq!(p.cwd, src);
        assert_eq!(p.args, vec![src.join("packaging/linux/package.sh")], "the export's own script");
        let env_sha = p.env.iter().find(|(k, _)| *k == "CADADDY_BUILD_SHA").map(|(_, v)| v.clone());
        assert_eq!(env_sha, Some(git(&d, &["rev-parse", "HEAD"]).unwrap()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MUST 5: `--step notes` alone writes the notes and touches nothing else.
    #[test]
    fn only_notes_step_touches_nothing_else() {
        let d = fixture("notes");
        let before = tree(&d);
        let toml = std::fs::read(d.join("Cargo.toml")).unwrap();
        run(&d, &["0.2.0", "--step", "notes"]).unwrap();
        let mut want = before.clone();
        want.push("dist/release-notes-0.2.0.md".into());
        want.sort();
        assert_eq!(tree(&d), want);
        assert_eq!(std::fs::read(d.join("Cargo.toml")).unwrap(), toml);
        let notes = std::fs::read_to_string(d.join("dist/release-notes-0.2.0.md")).unwrap();
        assert!(notes.contains("# CADaddy v0.2.0") && notes.contains("first commit"), "{notes}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MUST 6: smoke picks what xtask smoke can test and reports the rest as NOT SMOKED.
    #[test]
    fn smoke_selection() {
        let names: Vec<String> = [
            "cadaddy-0.8.0-macos-universal.dmg",
            "cadaddy-cli-0.8.0-macos-universal.zip",
            "CADaddy.mcpb",
            "cadaddy-0.8.0-windows-x64.msi",
            "cadaddy-0.8.0-linux-x86_64.AppImage",
            "cadaddy-0.8.0-linux-x86_64.deb",
            "SHA256SUMS",
            "SHA256SUMS.sig",
        ]
        .map(String::from)
        .to_vec();
        let (smoked, not) = select_smoke(&names, "macos");
        let s: Vec<&str> = smoked.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(s, ["cadaddy-0.8.0-macos-universal.dmg", "cadaddy-cli-0.8.0-macos-universal.zip", "CADaddy.mcpb"]);
        assert_eq!(smoked[1].1, SmokeHow::CliArchive);
        let n: Vec<&str> = not.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(n, ["cadaddy-0.8.0-windows-x64.msi", "cadaddy-0.8.0-linux-x86_64.AppImage", "cadaddy-0.8.0-linux-x86_64.deb"]);
        // On Linux, the Linux tar.gz CLI is smoked; the Mac DMG and CLI zip are not.
        let (smoked, not) = select_smoke(&["cadaddy-0.8.0-linux-x86_64.tar.gz".into(), "cadaddy-0.8.0-macos-universal.dmg".into()], "linux");
        assert_eq!(smoked, vec![("cadaddy-0.8.0-linux-x86_64.tar.gz".to_string(), SmokeHow::CliArchive)]);
        assert_eq!(not.len(), 1);
        // NOT SMOKED goes into the checklist.
        let cl = human_checklist("0.8.0", Path::new("d"), Path::new("n.md"), Some("ok"), &not);
        assert!(cl.contains("NOT SMOKED cadaddy-0.8.0-macos-universal.dmg"), "{cl}");
    }

    /// MUST 7: the checklist prints the mcp-publisher command and the llms.txt item.
    #[test]
    fn human_checklist_has_all_sections() {
        let cl = human_checklist("0.8.0", Path::new("/tmp/dist"), Path::new("/tmp/notes.md"), Some("ok"), &[]);
        assert!(cl.contains("v0.8.0"));
        assert!(cl.contains("git tag"));
        assert!(cl.contains("gh release create"));
        assert!(cl.contains("MCP Registry"));
        assert!(cl.contains("mcp-publisher publish"));
        assert!(cl.contains("llms.txt"));
        assert!(cl.contains("Website version"));
        assert!(cl.contains("PNG review"));
    }

    /// MUST 8: a dry run creates no dist/, leaves Cargo.toml/Cargo.lock byte-identical and
    /// `git status` unchanged.
    #[test]
    fn dry_run_changes_nothing() {
        let d = fixture("dry");
        std::fs::write(d.join("notes.txt"), "an untracked file\n").unwrap();
        let status = git(&d, &["status", "--porcelain"]).unwrap();
        let toml = std::fs::read(d.join("Cargo.toml")).unwrap();
        let lock = std::fs::read(d.join("Cargo.lock")).unwrap();
        let before = tree(&d);
        run(&d, &["0.2.0", "--dry-run", "--update-from", "old-installer.dmg"]).unwrap();
        run(&d, &["0.2.0", "--dry-run"]).unwrap();
        assert!(!d.join("dist").exists(), "dry run created dist/");
        assert_eq!(std::fs::read(d.join("Cargo.toml")).unwrap(), toml);
        assert_eq!(std::fs::read(d.join("Cargo.lock")).unwrap(), lock);
        assert_eq!(git(&d, &["status", "--porcelain"]).unwrap(), status);
        assert_eq!(tree(&d), before);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn zip_is_reproducible() {
        let a = zip_stored(&[("a.txt", b"hello", 0o644)]).unwrap();
        assert_eq!(a, zip_stored(&[("a.txt", b"hello", 0o644)]).unwrap());
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926, "the standard CRC-32 check value");
    }
}
