//! The desktop updater ([`cadaddy_ui_egui::update::Updater`]): asks GitHub's API for the latest
//! release, downloads this platform's installer with the system `curl` (Windows 10+, macOS and
//! Linux all have it; no TLS stack in CADaddy), checks it against the SHA-256 GitHub lists for it,
//! and installs it after CADaddy exits:
//! - Windows: the per-user MSI (`msiexec /passive`), once this process has ended;
//! - macOS: the app from the DMG replaces this `.app` (kept as `.old` until the copy is in place);
//! - Linux: a new AppImage replaces this one (`$APPIMAGE`). A .deb or a source build can't update
//!   itself: the card offers the download page.
//!
//! Nothing but the request for the latest release goes out: no identifiers, no drawing data.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use cadaddy_ui_egui::update::{Event, Release, Updater, VERSION, newer};
use serde_json::Value;
use sha2::{Digest, Sha256};

const API: &str = "https://api.github.com/repos/AICodeForgedev/cadaddy/releases/latest";
/// Larger release answers or installers are refused.
const MAX_JSON: usize = 1 << 20;
const MAX_INSTALLER: u64 = 512 << 20;

/// This platform's installer on the release page.
fn asset_name() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("CADaddy-Windows.msi")
    } else if cfg!(target_os = "macos") {
        Some("CADaddy-Mac.dmg")
    } else if cfg!(target_os = "linux") {
        Some("CADaddy-Linux.AppImage")
    } else {
        None
    }
}

/// What gets replaced: the running AppImage, the `.app` bundle, or (Windows) the installed exe.
fn install_target() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        return std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file());
    }
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos") {
        // …/CADaddy.app/Contents/MacOS/cadaddy
        let app = exe.parent()?.parent()?.parent()?.to_path_buf();
        return app.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")).then_some(app);
    }
    if cfg!(target_os = "windows") {
        // Installed by the MSI next to its uninstaller entry; a cargo build runs from target\.
        let installed = !exe.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"));
        return installed.then_some(exe);
    }
    None
}

/// A writable folder for the download: beside an AppImage (so the swap is a rename), else temp.
fn download_dir(target: &Path) -> PathBuf {
    if cfg!(target_os = "linux") {
        target.parent().map_or_else(std::env::temp_dir, Path::to_path_buf)
    } else {
        std::env::temp_dir().join("cadaddy-update")
    }
}

fn curl() -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c.stdin(Stdio::null());
    c
}

/// The latest release: its version, page, and this platform's installer (url, sha256).
fn latest() -> Result<(Release, Option<(String, String)>), String> {
    let out = curl()
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-A"])
        .arg(format!("CADaddy/{VERSION}"))
        .arg(API)
        .output()
        .map_err(|e| format!("curl isn't available ({e})"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Couldn't reach GitHub: {}", why.trim()));
    }
    if out.stdout.len() > MAX_JSON {
        return Err("GitHub's answer was too large".into());
    }
    parse(&out.stdout)
}

fn parse(json: &[u8]) -> Result<(Release, Option<(String, String)>), String> {
    let v: Value = serde_json::from_slice(json).map_err(|e| format!("GitHub's answer wasn't readable ({e})"))?;
    let tag = v.get("tag_name").and_then(Value::as_str).ok_or("no version in GitHub's answer")?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let page = v
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://github.com/AICodeForgedev/cadaddy/"))
        .map_or_else(|| format!("{}/latest", cadaddy_ui_egui::update::RELEASES), str::to_string);
    let asset = asset_name().and_then(|name| {
        let a = v.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(name))?;
        let url = a.get("browser_download_url")?.as_str()?;
        let sha = a.get("digest")?.as_str()?.strip_prefix("sha256:")?;
        let ok = url.starts_with("https://github.com/AICodeForgedev/cadaddy/releases/download/")
            && sha.len() == 64
            && sha.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| (url.to_string(), sha.to_ascii_lowercase()))
    });
    Ok((Release { version, page }, asset))
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        let Some(chunk) = buf.get(..n).filter(|c| !c.is_empty()) else { break };
        h.update(chunk);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download `url` to `to` and check it; the file is removed when anything is wrong.
fn fetch(url: &str, sha: &str, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    }
    let out = curl()
        .args(["-fsSL", "--max-time", "900", "--max-filesize"])
        .arg(MAX_INSTALLER.to_string())
        .arg("-o")
        .arg(to)
        .arg(url)
        .output()
        .map_err(|e| format!("curl isn't available ({e})"))?;
    let checked = if out.status.success() {
        match sha256_file(to) {
            Ok(got) if got == sha => Ok(()),
            Ok(_) => Err("The download didn't match its published SHA-256 fingerprint, so it wasn't installed.".to_string()),
            Err(e) => Err(format!("Couldn't read the download: {e}")),
        }
    } else {
        Err(format!("The download failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    };
    if checked.is_err() {
        let _ = std::fs::remove_file(to);
    }
    checked
}

#[derive(Default)]
struct Pending {
    /// The latest release's installer (url, sha256), from the last check.
    asset: Option<(String, String)>,
    /// The checked download, waiting for exit.
    ready: Option<PathBuf>,
    relaunch: bool,
}

pub struct DesktopUpdater {
    tx: Sender<Event>,
    rx: Mutex<Receiver<Event>>,
    pending: std::sync::Arc<Mutex<Pending>>,
    target: Option<PathBuf>,
}

impl Default for DesktopUpdater {
    fn default() -> Self {
        let (tx, rx) = channel();
        DesktopUpdater { tx, rx: Mutex::new(rx), pending: Default::default(), target: install_target() }
    }
}

impl Updater for DesktopUpdater {
    fn check(&self) {
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        std::thread::spawn(move || {
            let event = match latest() {
                Ok((r, asset)) if newer(&r.version, VERSION) => {
                    if let Ok(mut p) = pending.lock() {
                        p.asset = asset;
                    }
                    Event::Available(r)
                }
                Ok(_) => Event::Latest,
                Err(e) => Event::Failed(e),
            };
            let _ = tx.send(event);
        });
    }

    fn download(&self, release: &Release) {
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let Some(target) = self.target.clone() else { return };
        let version = release.version.clone();
        std::thread::spawn(move || {
            let asset = pending.lock().ok().and_then(|p| p.asset.clone());
            let event = match (asset, asset_name()) {
                (Some((url, sha)), Some(name)) => {
                    let to = download_dir(&target).join(format!(".{version}-{name}.download"));
                    match fetch(&url, &sha, &to) {
                        Ok(()) => {
                            if let Ok(mut p) = pending.lock() {
                                p.ready = Some(to);
                            }
                            Event::Downloaded
                        }
                        Err(e) => Event::Failed(e),
                    }
                }
                _ => Event::Failed("This release has no installer for this computer yet.".into()),
            };
            let _ = tx.send(event);
        });
    }

    fn poll(&self) -> Option<Event> {
        self.rx.lock().ok()?.try_recv().ok()
    }

    fn can_install(&self) -> bool {
        self.target.is_some() && asset_name().is_some()
    }

    fn relaunch_after_install(&self) {
        if let Ok(mut p) = self.pending.lock() {
            p.relaunch = true;
        }
    }

    /// Start installing the checked download, if there is one.
    fn on_exit(&self) {
        let Ok(p) = self.pending.lock() else { return };
        let (Some(file), Some(target)) = (&p.ready, &self.target) else { return };
        if let Err(e) = install(file, target, p.relaunch) {
            log::error!("update: couldn't start the installer: {e}");
        }
    }
}

/// Start the platform's installer for `file` over `target`, after this process ends.
fn install(file: &Path, target: &Path, relaunch: bool) -> std::io::Result<()> {
    let pid = std::process::id().to_string();
    if cfg!(target_os = "windows") {
        // PowerShell waits for CADaddy to exit, runs the MSI (it upgrades in place), deletes it
        // and starts the new version when asked. The script is a file (-File passes the paths as
        // arguments; they never become script text).
        let script = "param($id, $msi, $exe, $again)\r\n\
            Wait-Process -Id $id -ErrorAction SilentlyContinue\r\n\
            $p = Start-Process msiexec.exe -ArgumentList @('/i', ('\"' + $msi + '\"'), '/passive', '/norestart') -Wait -PassThru\r\n\
            Remove-Item -LiteralPath $msi -ErrorAction SilentlyContinue\r\n\
            if ($again -eq '1' -and $p.ExitCode -in 0, 3010) { Start-Process -FilePath $exe }\r\n\
            Remove-Item -LiteralPath $PSCommandPath -ErrorAction SilentlyContinue\r\n";
        let ps1 = file.with_extension("ps1");
        std::fs::write(&ps1, script)?;
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&ps1)
            .arg(&pid)
            .arg(file)
            .arg(target)
            .arg(if relaunch { "1" } else { "0" });
        detach(&mut c);
        c.spawn().map(|_| ())
    } else if cfg!(target_os = "macos") {
        let script = r#"
            while kill -0 "$1" 2>/dev/null; do sleep 0.3; done
            mnt=$(mktemp -d /tmp/cadaddy-update.XXXXXX) || exit 1
            if hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$2" >/dev/null; then
                app=$(ls -d "$mnt"/*.app 2>/dev/null | head -n 1)
                if [ -n "$app" ] && ditto "$app" "$3.updating"; then
                    rm -rf "$3.old"
                    if mv "$3" "$3.old"; then
                        if mv "$3.updating" "$3"; then rm -rf "$3.old"; else mv "$3.old" "$3"; fi
                    fi
                fi
                rm -rf "$3.updating"
                hdiutil detach "$mnt" -quiet
            fi
            rmdir "$mnt" 2>/dev/null
            rm -f "$2"
            if [ "$4" = 1 ]; then open "$3"; fi
        "#;
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script).arg("cadaddy-update").arg(&pid).arg(file).arg(target).arg(if relaunch { "1" } else { "0" });
        detach(&mut c);
        c.spawn().map(|_| ())
    } else {
        // The AppImage: a rename in the same folder (the running copy keeps its open file).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(file, target)?;
        if relaunch {
            let mut c = Command::new(target);
            detach(&mut c);
            c.spawn()?;
        }
        Ok(())
    }
}

/// Run `c` on its own, without our console or standard streams.
fn detach(c: &mut Command) {
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000 | 0x0000_0008); // CREATE_NO_WINDOW | DETACHED_PROCESS
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(digest: &str, url: &str) -> Vec<u8> {
        let name = asset_name().unwrap_or("CADaddy-Linux.AppImage");
        serde_json::json!({
            "tag_name": "v0.9.0",
            "html_url": "https://github.com/AICodeForgedev/cadaddy/releases/tag/v0.9.0",
            "assets": [
                {"name": "CADaddy.mcpb", "browser_download_url": "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/CADaddy.mcpb", "digest": format!("sha256:{}", "1".repeat(64))},
                {"name": name, "browser_download_url": url, "digest": digest},
            ]
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn the_release_answer_gives_version_page_and_this_platforms_installer() {
        let sha = "AB".repeat(32);
        let url = "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/x";
        let (r, asset) = parse(&release_json(&format!("sha256:{sha}"), url)).unwrap();
        assert_eq!(r.version, "0.9.0");
        assert_eq!(r.page, "https://github.com/AICodeForgedev/cadaddy/releases/tag/v0.9.0");
        if asset_name().is_some() {
            assert_eq!(asset, Some((url.to_string(), sha.to_ascii_lowercase())));
        }
    }

    #[test]
    fn installers_from_elsewhere_or_without_a_fingerprint_are_refused() {
        let good = "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/x";
        assert_eq!(parse(&release_json(&format!("sha256:{}", "a".repeat(64)), "https://example.com/x")).unwrap().1, None);
        assert_eq!(parse(&release_json("", good)).unwrap().1, None);
        assert_eq!(parse(&release_json("sha256:abc", good)).unwrap().1, None);
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn downloads_are_fingerprinted_with_sha256() {
        let dir = std::env::temp_dir().join(format!("cadaddy-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x");
        std::fs::write(&f, b"hello").unwrap();
        assert_eq!(sha256_file(&f).unwrap(), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
