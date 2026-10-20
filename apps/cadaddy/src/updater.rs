//! The desktop updater ([`cadaddy_ui_egui::update::Updater`]): asks GitHub's API for the latest
//! release, downloads this platform's installer with the system `curl` (Windows 10+, macOS and
//! Linux all have it; no TLS stack in CADaddy), checks it against the SHA-256 in the release's
//! signed SHA256SUMS (relsig.rs: an ed25519 signature with a key compiled into CADaddy, over the
//! release's version and sums; GitHub's own digest must agree too), refuses anything not newer
//! than this version, and installs it after CADaddy exits:
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
use cadaddy_ui_egui::{i18n, tf, tl};
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

fn no_curl(e: std::io::Error) -> String {
    tf!("curl isn't available ({error}).", error = e)
}

/// A signature problem in the interface language.
fn sig_message(e: &crate::relsig::SigError) -> String {
    use crate::relsig::SigError;
    match e {
        SigError::NoKey => tl!("This build has no release key to check updates with.").to_string(),
        SigError::TooLarge => tl!("The release's signature files are too large.").to_string(),
        SigError::Unreadable => tl!("The release's signature isn't one CADaddy reads.").to_string(),
        SigError::Damaged => tl!("The release's signature is missing or damaged.").to_string(),
        SigError::OtherVersion(v) => tf!("The release's signature is for another version ({version}).", version = v),
        SigError::UntrustedKey => tl!("The release is signed with a key this build doesn't trust.").to_string(),
        SigError::BadKey => tl!("This build's release key is damaged.").to_string(),
        SigError::Mismatch => tl!("The release's signature doesn't match its files.").to_string(),
    }
}

/// Where a release's installer for this platform and its signed sums are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Installer {
    url: String,
    /// GitHub's SHA-256 for it (checked against the signed one when present).
    digest: Option<String>,
    sums: Option<String>,
    sig: Option<String>,
}

/// Where release downloads come from.
const DOWNLOADS: &str = "https://github.com/AICodeForgedev/cadaddy/releases/download/";

/// Where releases are looked up and downloaded from, and the keys they must be signed with:
/// GitHub and the compiled-in keys (a local stand-in in tests).
#[derive(Clone, Debug)]
struct Source {
    api: String,
    /// Installers and signature files are only taken from under this address.
    downloads: String,
    keys: Vec<[u8; 32]>,
}

impl Source {
    fn github() -> Source {
        Source { api: API.into(), downloads: DOWNLOADS.into(), keys: crate::relsig::trusted_keys() }
    }
}

/// The latest release at `src`: its version, page, and this platform's installer.
fn latest_from(src: &Source) -> Result<(Release, Option<Installer>), String> {
    let out = curl()
        .args(["-fsSL", "--max-time", "20", "--max-filesize"])
        .arg(MAX_JSON.to_string())
        .args(["-H", "Accept: application/vnd.github+json", "-A"])
        .arg(format!("CADaddy/{VERSION}"))
        .arg(&src.api)
        .output()
        .map_err(no_curl)?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(tf!("Couldn't reach GitHub: {why}", why = why.trim()));
    }
    if out.stdout.len() > MAX_JSON {
        return Err(tl!("GitHub's answer was too large.").to_string());
    }
    parse(&out.stdout, &src.downloads)
}

/// Whether `v` (a release tag without its `v`) is a plain version: digits, letters, `.`, `-`
/// and `+`. It names the download's file, so nothing else (no `/`) is taken from the answer.
fn plain_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 40 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
}

fn parse(json: &[u8], downloads: &str) -> Result<(Release, Option<Installer>), String> {
    let v: Value = serde_json::from_slice(json).map_err(|e| tf!("GitHub's answer wasn't readable ({error}).", error = e))?;
    let tag = v.get("tag_name").and_then(Value::as_str).ok_or_else(|| tl!("GitHub's answer has no version.").to_string())?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    if !plain_version(&version) {
        return Err(tf!("GitHub's answer has an unreadable version ({version}).", version = tag.chars().take(40).collect::<String>()));
    }
    let page = v
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://github.com/AICodeForgedev/cadaddy/"))
        .map_or_else(|| format!("{}/latest", cadaddy_ui_egui::update::RELEASES), str::to_string);
    // An asset's download address, only from this project's releases.
    let url_of = |name: &str| -> Option<String> {
        let a = v.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(name))?;
        let url = a.get("browser_download_url")?.as_str()?;
        url.starts_with(downloads).then(|| url.to_string())
    };
    let asset = asset_name().and_then(|name| {
        let url = url_of(name)?;
        let a = v.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(name))?;
        let digest = a
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase);
        Some(Installer { url, digest, sums: url_of(crate::relsig::SUMS), sig: url_of(crate::relsig::SIG) })
    });
    Ok((Release { version, page }, asset))
}

/// A small release file (the sums or the signature) as text.
fn fetch_text(url: &str) -> Result<String, String> {
    let name = url.rsplit('/').next().unwrap_or(url);
    let out =
        curl().args(["-fsSL", "--max-time", "60", "--max-filesize"]).arg(crate::relsig::MAX_FILE.to_string()).arg(url).output().map_err(no_curl)?;
    if !out.status.success() {
        return Err(tf!(
            "Couldn't download the release's {name}, so nothing was installed: {why}",
            name = name,
            why = String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // curl stops at the limit; a server that sends more anyway is refused too.
    if out.stdout.len() > crate::relsig::MAX_FILE {
        return Err(tf!("The release's {name} is too large, so nothing was installed.", name = name));
    }
    String::from_utf8(out.stdout).map_err(|_| tf!("The release's {name} isn't text, so nothing was installed.", name = name))
}

/// The installer's SHA-256 from the release's signed sums, checked with `keys`: `Err` (nothing
/// is downloaded) when the release isn't signed, the signature doesn't verify, the sums don't
/// list the installer or GitHub's digest disagrees, or the release isn't newer than this one.
fn signed_sha(version: &str, a: &Installer, name: &str, keys: &[[u8; 32]], get: &dyn Fn(&str) -> Result<String, String>) -> Result<String, String> {
    // One sentence per line: each is translated (and, right to left, ordered) on its own.
    let not = |why: String| format!("{why}\n{}", tl!("It wasn't downloaded. Download it from the release page instead."));
    if !newer(version, VERSION) {
        return Err(not(tf!("Version {version} isn't newer than this one ({current}).", version = version, current = VERSION)));
    }
    let (Some(sums_url), Some(sig_url)) = (&a.sums, &a.sig) else {
        return Err(not(tl!("This release isn't signed.").to_string()));
    };
    let sums = get(sums_url)?;
    let sig = get(sig_url)?;
    crate::relsig::verify(version, sums.as_bytes(), &sig, keys)
        .map_err(|e| not(format!("{}\n{}", tl!("The release failed its check."), sig_message(&e))))?;
    let sha = crate::relsig::sum_for(&sums, name).ok_or_else(|| not(tf!("The signed release doesn't list {name}.", name = name)))?;
    if a.digest.as_ref().is_some_and(|d| *d != sha) {
        return Err(not(tf!("GitHub's fingerprint for {name} differs from the signed one.", name = name)));
    }
    Ok(sha)
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
        std::fs::create_dir_all(dir).map_err(|e| tf!("Couldn't make the folder {folder}: {error}", folder = dir.display(), error = e))?;
    }
    let out = curl()
        .args(["-fsSL", "--max-time", "900", "--max-filesize"])
        .arg(MAX_INSTALLER.to_string())
        .arg("-o")
        .arg(to)
        .arg(url)
        .output()
        .map_err(no_curl)?;
    let checked = if out.status.success() {
        match sha256_file(to) {
            Ok(got) if got == sha => Ok(()),
            Ok(_) => Err(tl!("The download didn't match its published SHA-256 fingerprint, so it wasn't installed.").to_string()),
            Err(e) => Err(tf!("Couldn't read the download: {error}", error = e)),
        }
    } else {
        Err(tf!("The download failed: {why}", why = String::from_utf8_lossy(&out.stderr).trim()))
    };
    if checked.is_err() {
        let _ = std::fs::remove_file(to);
    }
    checked
}

#[derive(Default)]
struct Pending {
    /// The latest release's installer, from the last check.
    asset: Option<Installer>,
    /// The checked download, waiting for exit.
    ready: Option<PathBuf>,
    relaunch: bool,
}

pub struct DesktopUpdater {
    tx: Sender<Event>,
    rx: Mutex<Receiver<Event>>,
    pending: std::sync::Arc<Mutex<Pending>>,
    target: Option<PathBuf>,
    source: std::sync::Arc<Source>,
}

impl Default for DesktopUpdater {
    fn default() -> Self {
        DesktopUpdater::new(Source::github(), install_target())
    }
}

impl DesktopUpdater {
    fn new(source: Source, target: Option<PathBuf>) -> Self {
        let (tx, rx) = channel();
        DesktopUpdater { tx, rx: Mutex::new(rx), pending: Default::default(), target, source: source.into() }
    }
}

impl Updater for DesktopUpdater {
    fn check(&self) {
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let source = self.source.clone();
        let language = i18n::current();
        std::thread::spawn(move || {
            let event = i18n::with_language(language, || match latest_from(&source) {
                Ok((r, asset)) if newer(&r.version, VERSION) => {
                    if let Ok(mut p) = pending.lock() {
                        p.asset = asset;
                    }
                    Event::Available(r)
                }
                Ok(_) => Event::Latest,
                Err(e) => Event::Failed(e),
            });
            let _ = tx.send(event);
        });
    }

    fn download(&self, release: &Release) {
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let Some(target) = self.target.clone() else { return };
        let version = release.version.clone();
        let source = self.source.clone();
        let language = i18n::current();
        std::thread::spawn(move || {
            // An earlier download is dropped: only this one can install (a failed one, nothing).
            let asset = pending.lock().ok().and_then(|mut p| {
                if let Some(old) = p.ready.take() {
                    let _ = std::fs::remove_file(old);
                }
                p.asset.clone()
            });
            let event = i18n::with_language(language, || match (asset, asset_name()) {
                (Some(a), Some(name)) => {
                    let to = download_dir(&target).join(format!(".{version}-{name}.download"));
                    let checked = signed_sha(&version, &a, name, &source.keys, &fetch_text);
                    match checked.and_then(|sha| fetch(&a.url, &sha, &to)) {
                        Ok(()) => {
                            if let Ok(mut p) = pending.lock() {
                                p.ready = Some(to);
                            }
                            Event::Downloaded
                        }
                        Err(e) => Event::Failed(e),
                    }
                }
                _ => Event::Failed(tl!("This release has no installer for this computer yet.").to_string()),
            });
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
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: a hidden console of its own, outliving
        // CADaddy. Not DETACHED_PROCESS: PowerShell without any console quits before running its
        // -File script, so the update never installed (0.7.7, 0.7.8).
        c.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
}

#[cfg(test)]
mod tests {
    // The fake release server serves the Linux-only end-to-end tests (AppImage installs).
    #![cfg_attr(not(target_os = "linux"), allow(dead_code))]
    use super::*;

    fn release_json(digest: &str, url: &str) -> Vec<u8> {
        let name = asset_name().unwrap_or("CADaddy-Linux.AppImage");
        serde_json::json!({
            "tag_name": "v0.9.0",
            "html_url": "https://github.com/AICodeForgedev/cadaddy/releases/tag/v0.9.0",
            "assets": [
                {"name": "CADaddy.mcpb", "browser_download_url": "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/CADaddy.mcpb", "digest": format!("sha256:{}", "1".repeat(64))},
                {"name": name, "browser_download_url": url, "digest": digest},
                {"name": "SHA256SUMS", "browser_download_url": "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/SHA256SUMS"},
                {"name": "SHA256SUMS.sig", "browser_download_url": "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/SHA256SUMS.sig"},
            ]
        })
        .to_string()
        .into_bytes()
    }

    fn parse_gh(json: &[u8]) -> Result<(Release, Option<Installer>), String> {
        parse(json, DOWNLOADS)
    }

    fn latest_gh(api: String) -> Result<(Release, Option<Installer>), String> {
        latest_from(&Source { api, ..Source::github() })
    }

    #[test]
    fn the_release_answer_gives_version_page_and_this_platforms_installer() {
        let sha = "AB".repeat(32);
        let url = "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/x";
        let (r, asset) = parse_gh(&release_json(&format!("sha256:{sha}"), url)).unwrap();
        assert_eq!(r.version, "0.9.0");
        assert_eq!(r.page, "https://github.com/AICodeForgedev/cadaddy/releases/tag/v0.9.0");
        if asset_name().is_some() {
            let a = asset.unwrap();
            assert_eq!((a.url.as_str(), a.digest), (url, Some(sha.to_ascii_lowercase())));
            assert_eq!(a.sums.as_deref(), Some("https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/SHA256SUMS"));
            assert!(a.sig.is_some());
        }
    }

    #[test]
    fn installers_from_elsewhere_are_refused() {
        let good = "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/x";
        assert_eq!(parse_gh(&release_json(&format!("sha256:{}", "a".repeat(64)), "https://example.com/x")).unwrap().1, None);
        // No (or a bad) GitHub digest: the signed sums decide alone.
        if asset_name().is_some() {
            assert_eq!(parse_gh(&release_json("", good)).unwrap().1.unwrap().digest, None);
            assert_eq!(parse_gh(&release_json("sha256:abc", good)).unwrap().1.unwrap().digest, None);
        }
        assert!(parse_gh(b"not json").is_err());
        // The version names the download's file: nothing but a plain version is taken.
        for tag in ["v1.0-/../../x", "v1.0-\\x", "v", "v1.0 x", &format!("v1.0-{}", "a".repeat(40))] {
            let json = serde_json::json!({ "tag_name": tag, "assets": [] }).to_string();
            assert!(parse_gh(json.as_bytes()).unwrap_err().contains("unreadable version"), "{tag}");
        }
    }

    /// A stand-in for GitHub's release feed on a local port: answers every request with `body`.
    fn feed(body: Vec<u8>, status: &'static str) -> String {
        use std::io::{BufRead, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/repos/AICodeForgedev/cadaddy/releases/latest", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let mut r = std::io::BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                while r.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let mut w = c;
                let _ = write!(w, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = w.write_all(&body);
            }
        });
        url
    }

    /// The version check end to end against a mocked feed (through the system curl, as the app
    /// does): a newer release is offered with this platform's installer, this version is up to
    /// date, and a broken or failing feed is an error, never an offer.
    #[test]
    fn the_version_check_against_a_mocked_release_feed() {
        if Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let url = "https://github.com/AICodeForgedev/cadaddy/releases/download/v0.9.0/x";
        let (r, asset) = latest_gh(feed(release_json(&format!("sha256:{}", "c".repeat(64)), url), "200 OK")).unwrap();
        assert_eq!(r.version, "0.9.0");
        assert!(cadaddy_ui_egui::update::newer(&r.version, VERSION), "0.9.0 is newer than {VERSION}");
        assert_eq!(asset.is_some(), asset_name().is_some());
        let same = serde_json::json!({ "tag_name": format!("v{VERSION}"), "assets": [] }).to_string().into_bytes();
        let (r, _) = latest_gh(feed(same, "200 OK")).unwrap();
        assert!(!cadaddy_ui_egui::update::newer(&r.version, VERSION), "this version is up to date");
        assert!(latest_gh(feed(b"<html>rate limited</html>".to_vec(), "200 OK")).is_err());
        assert!(latest_gh(feed(b"{}".to_vec(), "403 Forbidden")).is_err());
    }

    /// A release `version` whose sums list `name` with `sha`, signed with key `seed`.
    fn signed_release(seed: u8, version: &str, name: &str, sha: &str) -> ([u8; 32], String, String) {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let pk = sk.verifying_key().to_bytes();
        let sums = format!("{sha}  {name}\n{}  CADaddy.mcpb\n", "1".repeat(64));
        let sig = sk.sign(&crate::relsig::message(version, sums.as_bytes())).to_bytes();
        (pk, sums, crate::relsig::sig_file(version, &pk, &sig))
    }

    fn installer(digest: Option<&str>) -> Installer {
        Installer {
            url: format!("{DOWNLOADS}v99.0.0/x"),
            digest: digest.map(str::to_string),
            sums: Some(format!("{DOWNLOADS}v99.0.0/SHA256SUMS")),
            sig: Some(format!("{DOWNLOADS}v99.0.0/SHA256SUMS.sig")),
        }
    }

    #[test]
    fn only_a_newer_validly_signed_release_is_downloaded() {
        let name = "CADaddy-Test.bin";
        let sha = "ab".repeat(32);
        let (pk, sums, sig) = signed_release(5, "99.0.0", name, &sha);
        let served = |sums: String, sig: String| move |u: &str| Ok::<_, String>(if u.ends_with(".sig") { sig.clone() } else { sums.clone() });
        let get = served(sums.clone(), sig.clone());
        assert_eq!(signed_sha("99.0.0", &installer(None), name, &[pk], &get), Ok(sha.clone()));
        assert_eq!(signed_sha("99.0.0", &installer(Some(&sha)), name, &[pk], &get), Ok(sha.clone()));
        // GitHub's digest disagrees with the signed sums.
        assert!(signed_sha("99.0.0", &installer(Some(&"cd".repeat(32))), name, &[pk], &get).unwrap_err().contains("differs"));
        // Tampered sums (installer and sums swapped together).
        let tampered = served(sums.replace(&sha, &"cd".repeat(32)), sig.clone());
        assert!(signed_sha("99.0.0", &installer(None), name, &[pk], &tampered).unwrap_err().contains("failed its check"));
        // Signed with another key.
        let (other, ..) = signed_release(6, "99.0.0", name, &sha);
        assert!(signed_sha("99.0.0", &installer(None), name, &[other], &get).unwrap_err().contains("doesn't trust"));
        // No signature on the release.
        let unsigned = Installer { sig: None, ..installer(None) };
        assert!(signed_sha("99.0.0", &unsigned, name, &[pk], &get).unwrap_err().contains("isn't signed"));
        // A downgrade (or the same version) is refused before anything is fetched.
        let (opk, osums, osig) = signed_release(5, "0.0.1", name, &sha);
        let never = |_: &str| -> Result<String, String> { panic!("nothing is fetched for a downgrade") };
        assert!(signed_sha("0.0.1", &installer(None), name, &[opk], &never).unwrap_err().contains("isn't newer"));
        assert!(signed_sha(VERSION, &installer(None), name, &[opk], &never).is_err());
        // An old release's signed files presented as a newer release.
        let replay = served(osums, osig);
        assert!(signed_sha("99.0.0", &installer(None), name, &[opk], &replay).unwrap_err().contains("another version"));
        // Sums that don't list this platform's installer.
        assert!(signed_sha("99.0.0", &installer(None), "CADaddy-Other.bin", &[pk], &get).unwrap_err().contains("doesn't list"));
        // A failed fetch is reported.
        let down = |_: &str| -> Result<String, String> { Err("offline".into()) };
        assert_eq!(signed_sha("99.0.0", &installer(None), name, &[pk], &down), Err("offline".into()));
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

    /// What the fake release server answers for one path.
    #[derive(Clone)]
    enum Reply {
        Body(Vec<u8>),
        /// An HTTP error status.
        Status(&'static str),
        /// Promises `.1` bytes, sends `.0`, then hangs up.
        Cut(Vec<u8>, usize),
        /// `.0` bytes with no length given, until the client stops reading.
        Endless(usize),
    }

    /// A local stand-in for GitHub (the release feed and the release downloads), answering each
    /// path from `routes` and recording what was asked for. Nothing reaches the internet.
    struct FakeReleases {
        base: String,
        routes: std::sync::Arc<Mutex<std::collections::HashMap<String, Reply>>>,
        asked: std::sync::Arc<Mutex<Vec<String>>>,
    }

    impl FakeReleases {
        fn start() -> FakeReleases {
            use std::io::{BufRead, Write};
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            let routes = std::sync::Arc::new(Mutex::new(std::collections::HashMap::<String, Reply>::new()));
            let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
            let (r2, a2) = (routes.clone(), asked.clone());
            std::thread::spawn(move || {
                for c in l.incoming().flatten() {
                    let (routes, asked) = (r2.clone(), a2.clone());
                    std::thread::spawn(move || {
                        let mut r = std::io::BufReader::new(c.try_clone().unwrap());
                        let mut line = String::new();
                        let _ = r.read_line(&mut line);
                        let path = line.split(' ').nth(1).unwrap_or("").to_string();
                        let mut h = String::new();
                        while r.read_line(&mut h).is_ok_and(|n| n > 2) {
                            h.clear();
                        }
                        asked.lock().unwrap().push(path.clone());
                        let reply = routes.lock().unwrap().get(&path).cloned().unwrap_or(Reply::Status("404 Not Found"));
                        let mut w = c;
                        let head = |len: Option<usize>| match len {
                            Some(n) => format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n"),
                            None => "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".into(),
                        };
                        let _ = match reply {
                            Reply::Body(b) => w.write_all(head(Some(b.len())).as_bytes()).and_then(|_| w.write_all(&b)),
                            Reply::Status(st) => write!(w, "HTTP/1.1 {st}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                            Reply::Cut(b, n) => w.write_all(head(Some(n)).as_bytes()).and_then(|_| w.write_all(&b)),
                            Reply::Endless(n) => w.write_all(head(None).as_bytes()).and_then(|_| {
                                let chunk = [b'0'; 1 << 16];
                                (0..n.div_ceil(chunk.len())).try_for_each(|_| w.write_all(&chunk))
                            }),
                        };
                    });
                }
            });
            FakeReleases { base, routes, asked }
        }

        fn set(&self, path: &str, reply: Reply) {
            self.routes.lock().unwrap().insert(path.to_string(), reply);
        }

        fn source(&self, keys: Vec<[u8; 32]>) -> Source {
            Source { api: format!("{}/api/latest", self.base), downloads: format!("{}/download/", self.base), keys }
        }

        fn asked(&self) -> Vec<String> {
            std::mem::take(&mut *self.asked.lock().unwrap())
        }
    }

    const APPIMAGE: &str = "CADaddy-Linux.AppImage";

    /// The feed's answer for release `version`, listing the installer and, when `signed`, the
    /// sums and signature.
    fn feed_json(fake: &FakeReleases, version: &str, signed: bool) -> Reply {
        let asset = |name: &str| serde_json::json!({ "name": name, "browser_download_url": format!("{}/download/v{version}/{name}", fake.base) });
        let mut assets = vec![asset(APPIMAGE)];
        if signed {
            assets.extend([asset(crate::relsig::SUMS), asset(crate::relsig::SIG)]);
        }
        Reply::Body(serde_json::json!({ "tag_name": format!("v{version}"), "assets": assets }).to_string().into_bytes())
    }

    /// Release `version` on `fake`, signed with key `seed`: its installer, sums and signature.
    fn publish(fake: &FakeReleases, seed: u8, version: &str, installer: &[u8]) -> [u8; 32] {
        let sha = crate::relsig::hex(&Sha256::digest(installer));
        let (pk, sums, sig) = signed_release(seed, version, APPIMAGE, &sha);
        fake.set("/api/latest", feed_json(fake, version, true));
        fake.set(&format!("/download/v{version}/{APPIMAGE}"), Reply::Body(installer.to_vec()));
        fake.set(&format!("/download/v{version}/SHA256SUMS"), Reply::Body(sums.into_bytes()));
        fake.set(&format!("/download/v{version}/SHA256SUMS.sig"), Reply::Body(sig.into_bytes()));
        pk
    }

    fn wait(u: &DesktopUpdater) -> Event {
        let start = std::time::Instant::now();
        loop {
            if let Some(e) = u.poll() {
                return e;
            }
            assert!(start.elapsed().as_secs() < 60, "no answer from the updater");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    const INSTALLED: &[u8] = b"the installed CADaddy";

    /// One update attempt the way the app makes it (check, Update, quit) against `fake`, trusting
    /// `keys`, over an installed AppImage in a fresh folder: the download's outcome, then the
    /// installed file afterwards and every other file left in its folder.
    fn attempt(fake: &FakeReleases, keys: Vec<[u8; 32]>, case: &str) -> (Result<(), String>, Vec<u8>, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("cadaddy-updatetest-{}-{case}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("CADaddy.AppImage");
        std::fs::write(&target, INSTALLED).unwrap();
        let u = DesktopUpdater::new(fake.source(keys), Some(target.clone()));
        u.check();
        let outcome = match wait(&u) {
            Event::Available(r) => {
                u.download(&r);
                match wait(&u) {
                    Event::Downloaded => Ok(()),
                    Event::Failed(e) => Err(e),
                    e => panic!("{case}: {e:?}"),
                }
            }
            Event::Latest => Err("no newer release".into()),
            Event::Failed(e) => Err(e),
            Event::Downloaded => panic!("{case}: downloaded before asking"),
        };
        u.on_exit();
        let installed = std::fs::read(&target).unwrap();
        let others = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "CADaddy.AppImage")
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        (outcome, installed, others)
    }

    /// Q205: the signed update against a local fake release server, attacked every way we know.
    /// Each attack is refused with a clear message and leaves the installed app as it was (and
    /// no download behind); the genuine release installs.
    #[cfg(target_os = "linux")]
    #[test]
    fn signed_updates_under_attack() {
        if Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let fake = FakeReleases::start();
        let new = b"CADaddy 99.0.0".repeat(1000);
        let refused = |keys: Vec<[u8; 32]>, case: &str, says: &str| {
            let (outcome, installed, others) = attempt(&fake, keys, case);
            let e = outcome.expect_err(case);
            assert!(e.contains(says), "{case}: {e}");
            assert_eq!(installed, INSTALLED, "{case}: the installed app was changed");
            assert!(others.is_empty(), "{case}: left {others:?}");
            e
        };
        let installer_path = format!("/download/v99.0.0/{APPIMAGE}");
        let sums_path = "/download/v99.0.0/SHA256SUMS";
        let sig_path = "/download/v99.0.0/SHA256SUMS.sig";

        // A tampered installer (the signed sums are genuine).
        let pk = publish(&fake, 5, "99.0.0", &new);
        fake.set(&installer_path, Reply::Body(b"CADaddy 99.0.0 with something extra".to_vec()));
        refused(vec![pk], "tampered-installer", "didn't match its published SHA-256");

        // A truncated installer.
        fake.set(&installer_path, Reply::Cut(new[..1000].to_vec(), new.len()));
        refused(vec![pk], "cut-installer", "The download failed");

        // Tampered sums: the installer and its line in the sums swapped together.
        let evil = b"not CADaddy".to_vec();
        publish(&fake, 5, "99.0.0", &new);
        fake.set(&installer_path, Reply::Body(evil.clone()));
        let (_, genuine_sums, _) = signed_release(5, "99.0.0", APPIMAGE, &crate::relsig::hex(&Sha256::digest(&new)));
        let swapped = genuine_sums.replace(&crate::relsig::hex(&Sha256::digest(&new)), &crate::relsig::hex(&Sha256::digest(&evil)));
        fake.set(sums_path, Reply::Body(swapped.into_bytes()));
        fake.asked();
        refused(vec![pk], "tampered-sums", "doesn't match its files");
        assert!(!fake.asked().contains(&installer_path), "a release that fails its check isn't downloaded");

        // A whole release signed with a key this build doesn't trust.
        publish(&fake, 66, "99.0.0", &evil);
        refused(vec![pk], "unknown-key", "signed with a key this build doesn't trust");

        // No signature: not listed, or listed but not there.
        publish(&fake, 5, "99.0.0", &new);
        fake.set("/api/latest", feed_json(&fake, "99.0.0", false));
        refused(vec![pk], "no-sig-asset", "isn't signed");
        publish(&fake, 5, "99.0.0", &new);
        fake.set(sig_path, Reply::Status("404 Not Found"));
        refused(vec![pk], "missing-sig", "Couldn't download the release's SHA256SUMS.sig");

        // An older release's signed sums and signature served as 99.0.0.
        let old = b"CADaddy 1.0.0".to_vec();
        publish(&fake, 5, "1.0.0", &old);
        for f in ["SHA256SUMS", "SHA256SUMS.sig"] {
            let r = fake.routes.lock().unwrap().get(&format!("/download/v1.0.0/{f}")).cloned().unwrap();
            fake.set(&format!("/download/v99.0.0/{f}"), r);
        }
        fake.set(&installer_path, Reply::Body(old.clone()));
        fake.set("/api/latest", feed_json(&fake, "99.0.0", true));
        refused(vec![pk], "other-version", "signature is for another version (1.0.0)");

        // A downgrade, even genuinely signed, isn't offered.
        publish(&fake, 5, "0.0.1", &old);
        refused(vec![pk], "downgrade", "no newer release");
        publish(&fake, 5, VERSION, &old);
        refused(vec![pk], "same-version", "no newer release");

        // Huge or truncated sums and signatures.
        publish(&fake, 5, "99.0.0", &new);
        fake.set(sums_path, Reply::Endless(64 << 20));
        refused(vec![pk], "endless-sums", "Couldn't download the release's SHA256SUMS");
        fake.set(sums_path, Reply::Body(vec![b'0'; crate::relsig::MAX_FILE + 1]));
        refused(vec![pk], "huge-sums", "SHA256SUMS");
        publish(&fake, 5, "99.0.0", &new);
        fake.set(sig_path, Reply::Endless(64 << 20));
        refused(vec![pk], "endless-sig", "SHA256SUMS.sig");
        publish(&fake, 5, "99.0.0", &new);
        let (_, sums, _) = signed_release(5, "99.0.0", APPIMAGE, &crate::relsig::hex(&Sha256::digest(&new)));
        fake.set(sums_path, Reply::Cut(sums.as_bytes()[..sums.len() / 2].to_vec(), sums.len()));
        refused(vec![pk], "cut-sums", "Couldn't download the release's SHA256SUMS");
        // Cut short but complete as far as HTTP knows: the signature no longer matches.
        fake.set(sums_path, Reply::Body(sums.as_bytes()[..sums.len() / 2].to_vec()));
        refused(vec![pk], "short-sums", "doesn't match its files");
        fake.set(sums_path, Reply::Body(Vec::new()));
        refused(vec![pk], "empty-sums", "doesn't match its files");
        fake.set(sums_path, Reply::Body(vec![0xff; 100]));
        refused(vec![pk], "binary-sums", "isn't text");

        // A huge or unreadable release feed.
        fake.set("/api/latest", Reply::Endless(64 << 20));
        refused(vec![pk], "endless-feed", "Couldn't reach GitHub");
        fake.set("/api/latest", Reply::Body(br#"{"tag_name": "v99.0.0-/../../x", "assets": []}"#.to_vec()));
        refused(vec![pk], "path-in-version", "unreadable version");

        // A build with no key trusts nothing.
        publish(&fake, 5, "99.0.0", &new);
        refused(vec![], "no-keys", "no release key");

        // The genuine release installs.
        let pk = publish(&fake, 5, "99.0.0", &new);
        let (outcome, installed, others) = attempt(&fake, vec![pk], "genuine");
        assert_eq!(outcome, Ok(()));
        assert_eq!(installed, new, "the new version is installed");
        assert!(others.is_empty(), "left {others:?}");
    }

    /// A download that is ready, then a newer one that fails: quitting installs nothing and
    /// leaves no download behind (the card said the update failed).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_later_download_installs_nothing() {
        if Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let fake = FakeReleases::start();
        let dir = std::env::temp_dir().join(format!("cadaddy-updatetest-{}-retry", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("CADaddy.AppImage");
        std::fs::write(&target, INSTALLED).unwrap();
        let pk = publish(&fake, 5, "98.0.0", b"CADaddy 98.0.0");
        let u = DesktopUpdater::new(fake.source(vec![pk]), Some(target.clone()));
        let step = |want: &str| {
            u.check();
            let Event::Available(r) = wait(&u) else { panic!("no offer") };
            u.download(&r);
            let got = wait(&u);
            assert!(format!("{got:?}").contains(want), "{got:?}");
        };
        step("Downloaded");
        publish(&fake, 5, "99.0.0", b"CADaddy 99.0.0");
        fake.set(&format!("/download/v99.0.0/{APPIMAGE}"), Reply::Body(b"not CADaddy".to_vec()));
        step("didn't match");
        u.on_exit();
        assert_eq!(std::fs::read(&target).unwrap(), INSTALLED);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "only the installed app is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The installer script runs: PowerShell started the way [`install`] starts it (hidden,
    /// detached from CADaddy, `-File` with arguments) carries its script out.
    #[cfg(windows)]
    #[test]
    fn a_detached_powershell_script_runs() {
        let dir = std::env::temp_dir().join(format!("cadaddy-detach-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ps1 = dir.join("t.ps1");
        let marker = dir.join("ran.txt");
        std::fs::write(
            &ps1,
            "param($out, $word)
Set-Content -LiteralPath $out -Value $word
Remove-Item -LiteralPath $PSCommandPath
",
        )
        .unwrap();
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&ps1)
            .arg(&marker)
            .arg("installed");
        detach(&mut c);
        let mut child = c.spawn().unwrap();
        let _ = child.wait();
        let start = std::time::Instant::now();
        while (!marker.exists() || ps1.exists()) && start.elapsed() < std::time::Duration::from_secs(30) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let ran = std::fs::read_to_string(&marker).unwrap_or_default();
        let script_left = ps1.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ran.trim(), "installed", "the detached PowerShell didn't run its script");
        assert!(!script_left, "the script removes itself");
    }
}
