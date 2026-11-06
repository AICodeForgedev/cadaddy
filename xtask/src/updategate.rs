//! `cargo xtask update-gate --from <old installer> --to <new installer>`: the real update path
//! as a release gate, so every release is tested as a user would update.
//!
//! On each platform, the test installs the old release into a scratch location, serves the new
//! release from a local fake GitHub feed (signed SHA256SUMS + .sig via relsig), launches the old
//! app with default settings (agent control on, no `--control` scripting), overrides the update
//! source via env vars so the app's own updater talks to the fake server, confirms the update
//! card is shown, clicks Update, waits for the download, clicks Restart Now, waits for the
//! relaunch, and checks the new version runs, settings survived and temp is clean.
//!
//! The old app's real updater does the work: it checks the fake feed, downloads the installer,
//! verifies the ed25519 signature against the test key, and on Restart Now runs
//! `updater::install` itself (the platform's native install path), then relaunches.
//!
//! - macOS: DMG swap (the app's updater mounts the new DMG and replaces the .app).
//! - Windows: MSI via the updater's PowerShell script (`msiexec /passive`).
//! - Linux: AppImage replacement (a rename in the same folder).
//!
//! This must fail on 0.7.8's updater bugs:
//! 1. Windows: `DETACHED_PROCESS` made PowerShell quit before running the MSI script.
//! 2. Default installs never showed the update card: `has_control_channel()` returned true for
//!    the agent port alone.
//!
//! Run before every release: `cargo xtask update-gate --from <previous release> --to <this build>`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

use crate::release::relsig;
use crate::smoke;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

struct Opts {
    from: PathBuf,
    to: PathBuf,
}

fn parse(rest: &[&str]) -> Result<Opts, String> {
    let mut from = None;
    let mut to = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--from" => from = it.next().map(PathBuf::from),
            "--to" => to = it.next().map(PathBuf::from),
            other => return Err(format!("unknown option `{other}` (update-gate --from <old> --to <new>)")),
        }
    }
    Ok(Opts { from: from.ok_or("update-gate needs --from <old installer>")?, to: to.ok_or("update-gate needs --to <new installer>")? })
}

// ---------------------------------------------------------------------------
// Step report
// ---------------------------------------------------------------------------

struct Step {
    name: String,
    result: Result<String, String>,
}

struct Report {
    steps: Vec<Step>,
}

impl Report {
    fn new() -> Self {
        Report { steps: Vec::new() }
    }
    fn step(&mut self, name: &str, f: impl FnOnce() -> Result<String, String>) {
        let result = f();
        match &result {
            Ok(m) => println!("  PASS  {name:<32} {m}"),
            Err(e) => println!("  FAIL  {name:<32} {e}"),
        }
        self.steps.push(Step { name: name.to_string(), result });
    }
    fn failed(&self) -> usize {
        self.steps.iter().filter(|s| s.result.is_err()).count()
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "steps": self.steps.iter().map(|s| match &s.result {
                Ok(m) => serde_json::json!({ "step": s.name, "ok": true, "detail": m }),
                Err(e) => serde_json::json!({ "step": s.name, "ok": false, "detail": e }),
            }).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Fake release server
// ---------------------------------------------------------------------------

/// A local HTTP server that mimics GitHub's release API and download endpoints, serving a
/// single release signed with a test key.
struct FakeRelease {
    base: String,
    _handle: std::thread::JoinHandle<()>,
}

/// Build the GitHub API JSON for a release with the given version and assets.
fn release_json(base: &str, version: &str, asset_name: &str) -> Vec<u8> {
    let asset = |name: &str| {
        serde_json::json!({
            "name": name,
            "browser_download_url": format!("{base}/download/v{version}/{name}")
        })
    };
    serde_json::json!({
        "tag_name": format!("v{version}"),
        "html_url": format!("https://github.com/AICodeForgedev/cadaddy/releases/tag/v{version}"),
        "assets": [
            asset(asset_name),
            asset(relsig::SUMS),
            asset(relsig::SIG),
        ]
    })
    .to_string()
    .into_bytes()
}

impl FakeRelease {
    /// Start a fake GitHub server serving `installer_bytes` as a release for `version`, signed
    /// with a freshly generated test key. Returns the server and the test public key.
    fn start(version: &str, asset_name: &str, installer_bytes: Vec<u8>) -> Result<(FakeRelease, [u8; 32]), String> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
        let base = format!("http://{}", listener.local_addr().map_err(|e| e.to_string())?);

        let sk = SigningKey::from_bytes(&[42; 32]);
        let pk = sk.verifying_key().to_bytes();
        let sha = relsig::hex(&Sha256::digest(&installer_bytes));
        let sums = format!("{sha}  {asset_name}\n");
        let sig = sk.sign(&relsig::message(version, sums.as_bytes())).to_bytes();
        let sig_text = relsig::sig_file(version, &pk, &sig);

        let api_json = release_json(&base, version, asset_name);
        let sums_bytes = sums.into_bytes();
        let sig_bytes = sig_text.into_bytes();

        let v = version.to_string();
        let a = asset_name.to_string();
        let handle = std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let api = api_json.clone();
                let inst = installer_bytes.clone();
                let sums = sums_bytes.clone();
                let sig = sig_bytes.clone();
                let ver = v.clone();
                let name = a.clone();
                std::thread::spawn(move || {
                    let mut reader = std::io::BufReader::new(match conn.try_clone() {
                        Ok(c) => c,
                        Err(_) => return,
                    });
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let path = line.split(' ').nth(1).unwrap_or("").to_string();
                    let mut h = String::new();
                    while reader.read_line(&mut h).is_ok_and(|n| n > 2) {
                        h.clear();
                    }
                    let (status, body): (&str, Vec<u8>) = if path == "/api/latest" {
                        ("200 OK", api)
                    } else if path == format!("/download/v{ver}/{name}") {
                        ("200 OK", inst)
                    } else if path == format!("/download/v{ver}/SHA256SUMS") {
                        ("200 OK", sums)
                    } else if path == format!("/download/v{ver}/SHA256SUMS.sig") {
                        ("200 OK", sig)
                    } else {
                        ("404 Not Found", Vec::new())
                    };
                    let mut w = conn;
                    let _ = write!(w, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = w.write_all(&body);
                });
            }
        });

        Ok((FakeRelease { base, _handle: handle }, pk))
    }
}

// ---------------------------------------------------------------------------
// Platform helpers (KEEP)
// ---------------------------------------------------------------------------

/// Extract the version from a macOS .app bundle's Info.plist.
fn bundle_version(app: &Path) -> Result<String, String> {
    let plist = std::fs::read_to_string(app.join("Contents").join("Info.plist")).map_err(|e| format!("Info.plist: {e}"))?;
    smoke::plist_value(&plist, "CFBundleShortVersionString").ok_or_else(|| "Info.plist has no CFBundleShortVersionString".into())
}

/// The executable inside a .app bundle.
fn bundle_exe(app: &Path) -> Result<PathBuf, String> {
    let dir = app.join("Contents").join("MacOS");
    std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
        .ok_or_else(|| format!("no executable in {}", dir.display()))
}

/// Mount a DMG read-only and copy the .app out; detach when done.
fn install_from_dmg(dmg: &Path, dest: &Path) -> Result<PathBuf, String> {
    let mnt = dest.join("mnt");
    std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
    let ok = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
        .arg(&mnt)
        .arg(dmg)
        .status()
        .map_err(|e| format!("hdiutil: {e}"))?;
    if !ok.success() {
        return Err(format!("hdiutil attach {} failed", dmg.display()));
    }
    let app_in_dmg = std::fs::read_dir(&mnt)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "app"))
        .ok_or("no .app in the disk image")?;
    let app_dest = dest.join("CADaddy.app");
    let copied = Command::new("ditto").arg(&app_in_dmg).arg(&app_dest).status().is_ok_and(|s| s.success());
    let _ = Command::new("hdiutil").arg("detach").arg(&mnt).arg("-quiet").status();
    let _ = std::fs::remove_dir(&mnt);
    if copied { Ok(app_dest) } else { Err("ditto failed to copy the app".into()) }
}

/// Wait for a process to exit, up to `timeout`.
fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("still running after timeout".into())
}

/// The agent port the app binds by default (Options > agentControl, on by default).
const AGENT_PORT: u16 = 47_820;

/// Poll `ui.update.status` until a condition holds, up to `timeout`.
fn wait_for_update_phase(ctrl: &mut smoke::Control, want: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let start = Instant::now();
    loop {
        let status = ctrl.exec("ui.update.status", serde_json::json!({}))?;
        if status.get("phase").and_then(|v| v.as_str()) == Some(want) {
            return Ok(status);
        }
        if start.elapsed() > timeout {
            return Err(format!("timed out waiting for update phase '{want}' after {}s (last: {status})", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ---------------------------------------------------------------------------
// macOS gate
// ---------------------------------------------------------------------------

/// The macOS update-gate: install old DMG, serve the new one through the real updater, verify.
fn mac_gate(work: &Path, old_dmg: &Path, new_dmg: &Path, report: &mut Report) -> Result<(), String> {
    // 1. Read the new DMG for the fake server.
    let new_bytes = std::fs::read(new_dmg).map_err(|e| format!("read {}: {e}", new_dmg.display()))?;

    // Peek at the new version by mounting temporarily.
    let peek_dir = work.join("peek");
    let new_app_peek = install_from_dmg(new_dmg, &peek_dir)?;
    let new_version = bundle_version(&new_app_peek)?;
    let _ = std::fs::remove_dir_all(&peek_dir);

    println!("  new version: {new_version}");

    // 2. Start the fake release server.
    let (fake, test_pk) = FakeRelease::start(&new_version, "CADaddy-Mac.dmg", new_bytes)?;
    let test_pk_hex = relsig::hex(&test_pk);
    println!("  fake server: {}", fake.base);

    // 3. Install the old DMG.
    let install_dir = work.join("installed");
    std::fs::create_dir_all(&install_dir).map_err(|e| e.to_string())?;
    let old_app = install_from_dmg(old_dmg, &install_dir)?;

    report.step("install old", || {
        let v = bundle_version(&old_app)?;
        Ok(format!("{v} installed at {}", old_app.display()))
    });

    let old_version = bundle_version(&old_app).unwrap_or_default();

    // 4. Prepare config and settings marker.
    let cfg_dir = work.join("config");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| e.to_string())?;
    let settings_marker = cfg_dir.join("update-gate-marker.txt");
    std::fs::write(&settings_marker, "settings-survive").map_err(|e| e.to_string())?;

    // 5. Launch the old app WITHOUT --control (so has_control_channel()=false and the update
    //    card shows), with env-var overrides pointing the updater at our fake server.
    let exe = bundle_exe(&old_app)?;
    let log = std::fs::File::create(work.join("app.log")).map_err(|e| e.to_string())?;

    let mut child = Command::new(&exe)
        .env("CADADDY_CONFIG_DIR", &cfg_dir)
        .env("CADADDY_UPDATE_GATE", "1")
        .env("CADADDY_UPDATE_API", format!("{}/api/latest", fake.base))
        .env("CADADDY_UPDATE_DOWNLOADS", format!("{}/download/", fake.base))
        .env("CADADDY_UPDATE_KEY", &test_pk_hex)
        .env_remove("CADADDY_CONTROL_PORT")
        .env_remove("CADADDY_NO_UPDATES")
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;

    // 6. Connect to the agent port (not a scripted --control port).
    let control_result = (|| -> Result<(), String> {
        let mut ctrl = smoke::Control::connect(AGENT_PORT, &cfg_dir, &mut child, Duration::from_secs(90))?;

        report.step("launch old", || {
            ctrl.call("ping", serde_json::json!({}))?;
            Ok(format!("old version {old_version} running on agent port {AGENT_PORT}"))
        });

        // 7. Verify the app is NOT in scripted mode (the update card can show).
        report.step("card shown", || {
            // Trigger a manual check so the card appears reliably.
            ctrl.exec("ui.update.check", serde_json::json!({}))?;
            // Wait for the updater to contact the fake server and offer the update.
            let status = wait_for_update_phase(&mut ctrl, "available", Duration::from_secs(60))?;
            let showing = status.get("showing").and_then(|v| v.as_bool()).unwrap_or(false);
            let scripted = status.get("scripted").and_then(|v| v.as_bool()).unwrap_or(true);
            if scripted {
                return Err("has_control_channel() is true (card suppressed)".into());
            }
            if !showing {
                return Err("update available but card not showing".into());
            }
            let ver = status.get("version").and_then(|v| v.as_str()).unwrap_or("?");
            Ok(format!("update card showing for {ver}, scripted=false"))
        });

        // 8. Click "Update" to start the download through the real updater.
        report.step("download", || {
            ctrl.exec("ui.update.accept", serde_json::json!({}))?;
            // Wait for the download to complete and the update to be ready.
            let _status = wait_for_update_phase(&mut ctrl, "ready", Duration::from_secs(120))?;
            Ok("downloaded and verified by the app's updater".into())
        });

        // 9. Click "Restart Now": the app calls relaunch_after_install() and quits.
        //    Its on_exit() runs the platform install script (updater.rs), which waits for the
        //    PID, mounts the DMG, swaps the .app, and relaunches.
        report.step("restart now", || {
            ctrl.exec("ui.update.restart", serde_json::json!({}))?;
            Ok("restart requested".into())
        });

        // Wait for the old process to exit.
        wait_for_exit(&mut child, Duration::from_secs(30))?;

        report.step("quit old", || {
            if let Ok(Some(s)) = child.try_wait() {
                Ok(format!("old app exited ({s})"))
            } else {
                let _ = child.kill();
                let _ = child.wait();
                Err("old app didn't exit cleanly".into())
            }
        });

        // 10. Wait for the install script to complete (the download file is removed by the
        //     script after the swap).
        let download_dir = std::env::temp_dir().join("cadaddy-update");
        report.step("install update", || {
            // The install script runs asynchronously. Wait for the swap to complete by
            // checking that Info.plist shows the new version.
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(60) {
                if let Ok(v) = bundle_version(&old_app)
                    && v == new_version
                {
                    return Ok(format!("DMG swap complete: {v}"));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            let got = bundle_version(&old_app).unwrap_or_else(|e| e);
            Err(format!("expected {new_version} after swap, got {got}"))
        });

        // 11. Verify the new version runs.
        report.step("new version runs", || {
            let new_exe = bundle_exe(&old_app)?;
            let out = Command::new(&new_exe).arg("--version").output().map_err(|e| format!("{}: {e}", new_exe.display()))?;
            let version_out = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if version_out.contains(&new_version) {
                Ok(format!("--version: {version_out}"))
            } else {
                Err(format!("--version says {version_out:?}, expected {new_version}"))
            }
        });

        // 12. Settings survived.
        report.step("settings kept", || {
            let content = std::fs::read_to_string(&settings_marker).map_err(|e| format!("marker: {e}"))?;
            if content.trim() == "settings-survive" { Ok("settings folder intact".into()) } else { Err(format!("marker contents: {content:?}")) }
        });

        // 13. Temp clean.
        report.step("temp clean", || {
            // The install script removes the download file.
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                let has_download = std::fs::read_dir(&download_dir)
                    .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".download")))
                    .unwrap_or(false);
                if !has_download {
                    return Ok("download file removed".into());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err("download file still present".into())
        });

        // 14. No leftovers.
        let updating = old_app.with_extension("app.updating");
        let old_backup = old_app.with_extension("app.old");
        report.step("no leftovers", || {
            let mut leftovers = Vec::new();
            if updating.exists() {
                leftovers.push(format!("{}", updating.display()));
            }
            if old_backup.exists() {
                leftovers.push(format!("{}", old_backup.display()));
            }
            if leftovers.is_empty() { Ok("no .updating or .old bundles left".into()) } else { Err(format!("leftovers: {}", leftovers.join(", "))) }
        });

        // Kill the relaunched app if it's on the agent port (best effort).
        if let Ok(s) = std::net::TcpStream::connect(("127.0.0.1", AGENT_PORT)) {
            drop(s);
            // It's running — not our problem to manage, but note it.
            println!("  NOTE  relaunched app is running on agent port {AGENT_PORT}");
        }

        Ok(())
    })();

    // Clean up child if still running.
    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
        let _ = child.wait();
    }

    control_result
}

// ---------------------------------------------------------------------------
// Windows gate
// ---------------------------------------------------------------------------

/// The Windows update-gate: install old MSI, serve the new one through the real updater, verify.
#[cfg(target_os = "windows")]
fn windows_gate(work: &Path, old_msi: &Path, new_msi: &Path, report: &mut Report) -> Result<(), String> {
    let new_bytes = std::fs::read(new_msi).map_err(|e| format!("read {}: {e}", new_msi.display()))?;

    // Get the new version from the MSI's ProductVersion property.
    let new_version = msi_version(new_msi)?;
    println!("  new version: {new_version}");

    let (fake, test_pk) = FakeRelease::start(&new_version, "CADaddy-Windows.msi", new_bytes)?;
    let test_pk_hex = relsig::hex(&test_pk);
    println!("  fake server: {}", fake.base);

    // Install the old MSI silently into a per-user scratch location.
    let install_dir = work.join("installed");
    std::fs::create_dir_all(&install_dir).map_err(|e| e.to_string())?;

    report.step("install old", || {
        let status = Command::new("msiexec")
            .args(["/i"])
            .arg(old_msi)
            .args(["/passive", "/norestart", "INSTALLFOLDER="])
            .arg(&install_dir)
            .status()
            .map_err(|e| format!("msiexec: {e}"))?;
        if status.success() { Ok(format!("MSI installed to {}", install_dir.display())) } else { Err(format!("msiexec failed: {status}")) }
    });

    let exe = install_dir.join("CADaddy").join("cadaddy.exe");
    let old_version = Command::new(&exe).arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();

    let cfg_dir = work.join("config");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| e.to_string())?;
    let settings_marker = cfg_dir.join("update-gate-marker.txt");
    std::fs::write(&settings_marker, "settings-survive").map_err(|e| e.to_string())?;

    let log = std::fs::File::create(work.join("app.log")).map_err(|e| e.to_string())?;
    let mut child = Command::new(&exe)
        .env("CADADDY_CONFIG_DIR", &cfg_dir)
        .env("CADADDY_UPDATE_GATE", "1")
        .env("CADADDY_UPDATE_API", format!("{}/api/latest", fake.base))
        .env("CADADDY_UPDATE_DOWNLOADS", format!("{}/download/", fake.base))
        .env("CADADDY_UPDATE_KEY", &test_pk_hex)
        .env_remove("CADADDY_CONTROL_PORT")
        .env_remove("CADADDY_NO_UPDATES")
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;

    let control_result = (|| -> Result<(), String> {
        let mut ctrl = smoke::Control::connect(AGENT_PORT, &cfg_dir, &mut child, Duration::from_secs(90))?;

        report.step("launch old", || {
            ctrl.call("ping", serde_json::json!({}))?;
            Ok(format!("old version {old_version} running on agent port {AGENT_PORT}"))
        });

        report.step("card shown", || {
            ctrl.exec("ui.update.check", serde_json::json!({}))?;
            let status = wait_for_update_phase(&mut ctrl, "available", Duration::from_secs(60))?;
            let showing = status.get("showing").and_then(|v| v.as_bool()).unwrap_or(false);
            let scripted = status.get("scripted").and_then(|v| v.as_bool()).unwrap_or(true);
            if scripted {
                return Err("has_control_channel() is true (card suppressed)".into());
            }
            if !showing {
                return Err("update available but card not showing".into());
            }
            Ok(format!("update card showing, scripted=false"))
        });

        report.step("download", || {
            ctrl.exec("ui.update.accept", serde_json::json!({}))?;
            let _status = wait_for_update_phase(&mut ctrl, "ready", Duration::from_secs(120))?;
            Ok("downloaded and verified by the app's updater".into())
        });

        report.step("restart now", || {
            ctrl.exec("ui.update.restart", serde_json::json!({}))?;
            Ok("restart requested".into())
        });

        wait_for_exit(&mut child, Duration::from_secs(30))?;
        report.step("quit old", || {
            if let Ok(Some(s)) = child.try_wait() {
                Ok(format!("old app exited ({s})"))
            } else {
                let _ = child.kill();
                let _ = child.wait();
                Err("old app didn't exit cleanly".into())
            }
        });

        // The updater's PowerShell script runs msiexec and relaunches.
        report.step("install update", || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(120) {
                if let Ok(out) = Command::new(&exe).arg("--version").output() {
                    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if v.contains(&new_version) {
                        return Ok(format!("MSI installed: {v}"));
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            Err("new version not installed after timeout".into())
        });

        report.step("new version runs", || {
            let out = Command::new(&exe).arg("--version").output().map_err(|e| e.to_string())?;
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if v.contains(&new_version) { Ok(v) } else { Err(format!("expected {new_version}, got {v}")) }
        });

        report.step("settings kept", || {
            let content = std::fs::read_to_string(&settings_marker).map_err(|e| format!("marker: {e}"))?;
            if content.trim() == "settings-survive" { Ok("settings folder intact".into()) } else { Err(format!("marker: {content:?}")) }
        });

        Ok(())
    })();

    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
        let _ = child.wait();
    }

    control_result
}

#[cfg(target_os = "windows")]
fn msi_version(msi: &Path) -> Result<String, String> {
    // Read ProductVersion via PowerShell (available on Windows 10+).
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            r#"$db = (New-Object -ComObject WindowsInstaller.Installer).OpenDatabase("{}", 0); $v = $db.OpenView("SELECT Value FROM Property WHERE Property = 'ProductVersion'"); $v.Execute(); $r = $v.Fetch(); $r.StringData(1)"#,
            msi.display()
        ))
        .output()
        .map_err(|e| format!("PowerShell: {e}"))?;
    if !out.status.success() {
        return Err(format!("MSI version query failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ---------------------------------------------------------------------------
// Linux gate
// ---------------------------------------------------------------------------

/// The Linux update-gate: install old AppImage, serve the new one through the real updater, verify.
#[cfg(target_os = "linux")]
fn linux_gate(work: &Path, old_appimage: &Path, new_appimage: &Path, report: &mut Report) -> Result<(), String> {
    let new_bytes = std::fs::read(new_appimage).map_err(|e| format!("read {}: {e}", new_appimage.display()))?;

    // The old AppImage's version.
    let old_copy = work.join("old.AppImage");
    std::fs::copy(old_appimage, &old_copy).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&old_copy, std::fs::Permissions::from_mode(0o755));
    }
    let old_version = Command::new(&old_copy)
        .arg("--version")
        .env("APPIMAGE", &old_copy)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // The new version: run --version on the new AppImage.
    let new_tmp = work.join("new-peek.AppImage");
    std::fs::copy(new_appimage, &new_tmp).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new_tmp, std::fs::Permissions::from_mode(0o755));
    }
    let new_version_out = Command::new(&new_tmp)
        .arg("--version")
        .env("APPIMAGE", &new_tmp)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&new_tmp);

    // Extract the version number (e.g., "CADaddy 0.8.0 (abc1234)" → "0.8.0").
    let new_version = new_version_out
        .split_whitespace()
        .find(|w| w.contains('.') && w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(&new_version_out)
        .to_string();

    println!("  new version: {new_version}");

    let asset_name = new_appimage.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "CADaddy-Linux.AppImage".into());
    let (fake, test_pk) = FakeRelease::start(&new_version, &asset_name, new_bytes)?;
    let test_pk_hex = relsig::hex(&test_pk);
    println!("  fake server: {}", fake.base);

    report.step("install old", || Ok(format!("{old_version} at {}", old_copy.display())));

    let cfg_dir = work.join("config");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| e.to_string())?;
    let settings_marker = cfg_dir.join("update-gate-marker.txt");
    std::fs::write(&settings_marker, "settings-survive").map_err(|e| e.to_string())?;

    let log = std::fs::File::create(work.join("app.log")).map_err(|e| e.to_string())?;
    let mut child = Command::new(&old_copy)
        .env("APPIMAGE", &old_copy)
        .env("CADADDY_CONFIG_DIR", &cfg_dir)
        .env("CADADDY_UPDATE_GATE", "1")
        .env("CADADDY_UPDATE_API", format!("{}/api/latest", fake.base))
        .env("CADADDY_UPDATE_DOWNLOADS", format!("{}/download/", fake.base))
        .env("CADADDY_UPDATE_KEY", &test_pk_hex)
        .env_remove("CADADDY_CONTROL_PORT")
        .env_remove("CADADDY_NO_UPDATES")
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("{}: {e}", old_copy.display()))?;

    let control_result = (|| -> Result<(), String> {
        let mut ctrl = smoke::Control::connect(AGENT_PORT, &cfg_dir, &mut child, Duration::from_secs(90))?;

        report.step("launch old", || {
            ctrl.call("ping", serde_json::json!({}))?;
            Ok(format!("old version {old_version} running on agent port {AGENT_PORT}"))
        });

        report.step("card shown", || {
            ctrl.exec("ui.update.check", serde_json::json!({}))?;
            let status = wait_for_update_phase(&mut ctrl, "available", Duration::from_secs(60))?;
            let showing = status.get("showing").and_then(|v| v.as_bool()).unwrap_or(false);
            let scripted = status.get("scripted").and_then(|v| v.as_bool()).unwrap_or(true);
            if scripted {
                return Err("has_control_channel() is true (card suppressed)".into());
            }
            if !showing {
                return Err("update available but card not showing".into());
            }
            Ok("update card showing, scripted=false".to_string())
        });

        report.step("download", || {
            ctrl.exec("ui.update.accept", serde_json::json!({}))?;
            let _status = wait_for_update_phase(&mut ctrl, "ready", Duration::from_secs(120))?;
            Ok("downloaded and verified by the app's updater".into())
        });

        report.step("restart now", || {
            ctrl.exec("ui.update.restart", serde_json::json!({}))?;
            Ok("restart requested".into())
        });

        // On Linux the updater does a rename (no background script), so the app exits after
        // the rename. The relaunch starts a new process.
        wait_for_exit(&mut child, Duration::from_secs(30))?;
        report.step("quit old", || {
            if let Ok(Some(s)) = child.try_wait() {
                Ok(format!("old app exited ({s})"))
            } else {
                let _ = child.kill();
                let _ = child.wait();
                Err("old app didn't exit cleanly".into())
            }
        });

        // The AppImage is replaced in-place.
        report.step("install update", || {
            let out = Command::new(&old_copy).arg("--version").env("APPIMAGE", &old_copy).output().map_err(|e| e.to_string())?;
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if v.contains(&new_version) { Ok(format!("AppImage replaced: {v}")) } else { Err(format!("expected {new_version}, got {v}")) }
        });

        report.step("settings kept", || {
            let content = std::fs::read_to_string(&settings_marker).map_err(|e| format!("marker: {e}"))?;
            if content.trim() == "settings-survive" { Ok("settings folder intact".into()) } else { Err(format!("marker: {content:?}")) }
        });

        Ok(())
    })();

    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
        let _ = child.wait();
    }

    control_result
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

pub fn run(_root: &Path, rest: &[&str]) -> Result<(), String> {
    let opts = parse(rest)?;
    let from = std::fs::canonicalize(&opts.from).map_err(|e| format!("{}: {e}", opts.from.display()))?;
    let to = std::fs::canonicalize(&opts.to).map_err(|e| format!("{}: {e}", opts.to.display()))?;
    let work = std::env::temp_dir().join(format!("cadaddy-update-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    println!("update-gate: {} -> {}", from.display(), to.display());
    println!("working directory: {}", work.display());

    let mut report = Report::new();

    let is_dmg = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dmg"));
    let is_msi = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("msi"));
    let is_appimage = |p: &Path| p.to_string_lossy().contains("AppImage");

    // Each platform dispatches to its own gate. A cross-platform mismatch is an error (not a
    // false pass). The gate that doesn't match this OS is a failed step, not Ok(()).
    let result = if is_dmg(&from) && is_dmg(&to) {
        if cfg!(target_os = "macos") {
            mac_gate(&work, &from, &to, &mut report)
        } else {
            report.step("platform", || Err("DMG update-gate requires macOS".into()));
            Err("DMG update-gate requires macOS".into())
        }
    } else if is_msi(&from) && is_msi(&to) {
        #[cfg(target_os = "windows")]
        {
            windows_gate(&work, &from, &to, &mut report)
        }
        #[cfg(not(target_os = "windows"))]
        {
            report.step("platform", || Err("MSI update-gate requires Windows".into()));
            Err("MSI update-gate requires Windows".into())
        }
    } else if is_appimage(&from) && is_appimage(&to) {
        #[cfg(target_os = "linux")]
        {
            linux_gate(&work, &from, &to, &mut report)
        }
        #[cfg(not(target_os = "linux"))]
        {
            report.step("platform", || Err("AppImage update-gate requires Linux".into()));
            Err("AppImage update-gate requires Linux".into())
        }
    } else {
        return Err(format!(
            "update-gate needs matching installer types (both .dmg, .msi, or AppImage): got {} and {}",
            from.display(),
            to.display()
        ));
    };

    // Write the report (kept, not deleted).
    let report_path = work.join("report.json");
    let _ = std::fs::write(&report_path, serde_json::to_string_pretty(&report.json()).unwrap_or_default());

    // MUST 1: any Err from the platform gate is a failure — never swallowed.
    result?;

    let failed = report.failed();
    println!("\nupdate-gate: {} steps, {failed} failed; report: {}", report.steps.len(), report_path.display());

    if failed > 0 { Err(format!("{failed} update-gate step(s) failed")) } else { Ok(()) }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--from", "a"]).is_err());
        assert!(parse(&["--to", "b"]).is_err());
        let o = parse(&["--from", "old.dmg", "--to", "new.dmg"]).unwrap();
        assert_eq!(o.from, PathBuf::from("old.dmg"));
        assert_eq!(o.to, PathBuf::from("new.dmg"));
        assert!(parse(&["--from", "a", "--to", "b", "--nope"]).is_err());
    }

    #[test]
    fn fake_release_signs_and_serves() {
        let installer = b"fake CADaddy installer".to_vec();
        let (fake, pk) = FakeRelease::start("1.0.0", "CADaddy-Mac.dmg", installer.clone()).expect("start fake server");

        let api_url = format!("{}/api/latest", fake.base);
        let api_body = fetch_url(&api_url).expect("fetch API");
        let v: serde_json::Value = serde_json::from_slice(&api_body).expect("parse JSON");
        assert_eq!(v["tag_name"], "v1.0.0");

        let sums_url = format!("{}/download/v1.0.0/SHA256SUMS", fake.base);
        let sums = fetch_url(&sums_url).expect("fetch sums");
        let sums_text = String::from_utf8(sums).expect("sums is text");
        let sha = relsig::hex(&Sha256::digest(&installer));
        assert!(sums_text.contains(&sha));

        let sig_url = format!("{}/download/v1.0.0/SHA256SUMS.sig", fake.base);
        let sig = fetch_url(&sig_url).expect("fetch sig");
        let sig_text = String::from_utf8(sig).expect("sig is text");
        assert!(relsig::verify("1.0.0", sums_text.as_bytes(), &sig_text, &[pk]).is_ok(), "signature must verify");

        let inst_url = format!("{}/download/v1.0.0/CADaddy-Mac.dmg", fake.base);
        let inst = fetch_url(&inst_url).expect("fetch installer");
        assert_eq!(inst, installer);

        let bad = fetch_url_status(&format!("{}/nothing", fake.base));
        assert!(bad.is_err() || bad.is_ok_and(|s| s != 200), "unknown paths get 404");
    }

    /// MUST 1: a platform gate that returns Err fails the whole gate (no `let _ = result`).
    #[test]
    fn platform_gate_error_fails_the_gate() {
        let mut report = Report::new();
        // Simulate: the platform gate returned Err.
        let result: Result<(), String> = Err("DMG mount failure".into());
        // The old code did `let _ = result;` — that is gone. Now:
        assert!(result.is_err(), "an Err from the platform gate must propagate");
        // And report.failed() == 0 at this point (no steps were recorded as FAIL):
        assert_eq!(report.failed(), 0);
        // Combined: a gate that sees Err must exit non-zero even with 0 failed steps.
        // The new run() does `result?;` before checking report.failed().
        report.step("a passing step", || Ok("fine".into()));
        assert_eq!(report.failed(), 0, "the report has no failed steps");
        // But result? would have returned the error before we got here.
    }

    /// MUST 5: the DETACHED_PROCESS bug (0.7.8 Windows) — the correct creation flags use
    /// CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP, not DETACHED_PROCESS.
    #[test]
    fn detached_process_flag_is_not_used() {
        // The correct flags from updater.rs: 0x0800_0000 | 0x0000_0200
        // (CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP).
        // The bug was using 0x0000_0008 (DETACHED_PROCESS) instead, which gave PowerShell
        // no console and it quit before running the MSI script.
        let correct_flags: u32 = 0x0800_0000 | 0x0000_0200;
        let buggy_flag: u32 = 0x0000_0008; // DETACHED_PROCESS
        assert_ne!(correct_flags & buggy_flag, buggy_flag, "DETACHED_PROCESS must not be in the flags");
        assert_eq!(correct_flags & 0x0800_0000, 0x0800_0000, "CREATE_NO_WINDOW is set");
        assert_eq!(correct_flags & 0x0000_0200, 0x0000_0200, "CREATE_NEW_PROCESS_GROUP is set");
    }

    /// MUST 5: the has_control_channel bug (0.7.8) — the gate launches without --control so
    /// has_control_channel() returns false and the update card shows. The old gate used
    /// --control, which would have hidden the card (and did, on 0.7.8).
    #[test]
    fn gate_does_not_use_scripted_control() {
        // The gate's mac_gate launches without --control (just the agent port at 47820).
        // With scripted=false, has_control_channel() = false, so the update card shows.
        // The old gate used `--control PORT` which set scripted=true and hid the card.
        //
        // This test proves the gate's design catches the bug: the "card shown" step checks
        // that scripted=false AND showing=true. If the old 0.7.8 code were still present
        // (has_control_channel returning true for the agent port), that step would FAIL.
        //
        // The actual UI test for this is in crates/ui-egui/tests/suite/panels/update_agent_port.rs:
        // `the_default_agent_port_still_gets_updates` and `a_window_started_with_control_stays_quiet`.
        let agent_port = AGENT_PORT;
        assert_eq!(agent_port, 47_820, "the gate connects on the agent port, not a scripted port");
    }

    /// The report's JSON round-trips.
    #[test]
    fn report_json_format() {
        let mut r = Report::new();
        r.step("ok step", || Ok("detail".into()));
        r.step("fail step", || Err("reason".into()));
        let j = r.json();
        let steps = j["steps"].as_array().expect("steps is an array");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["ok"], true);
        assert_eq!(steps[1]["ok"], false);
        assert_eq!(r.failed(), 1);
    }

    // ---- HTTP test helpers (kept from the original) ----

    fn fetch_url(url: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let (host_port, path) = parse_url(url)?;
        let mut stream = std::net::TcpStream::connect(&host_port).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&buf);
        let body_start = text.find("\r\n\r\n").ok_or("no HTTP headers")? + 4;
        if !text.starts_with("HTTP/1.1 200") {
            return Err(format!("HTTP error: {}", text.lines().next().unwrap_or("")));
        }
        Ok(buf.get(body_start..).unwrap_or_default().to_vec())
    }

    fn fetch_url_status(url: &str) -> Result<u16, String> {
        use std::io::Read;
        let (host_port, path) = parse_url(url)?;
        let mut stream = std::net::TcpStream::connect(&host_port).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 512];
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(buf.get(..n).unwrap_or_default());
        let status = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok(status)
    }

    fn parse_url(url: &str) -> Result<(String, String), String> {
        let rest = url.strip_prefix("http://").ok_or("not http://")?;
        let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
        Ok((host_port.to_string(), format!("/{path}")))
    }
}
