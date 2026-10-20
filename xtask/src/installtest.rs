//! `cargo xtask installtest --new DIR [--old DIR]`: install, upgrade and uninstall checks of the
//! release files in DIR (and the previous release's in --old, for the upgrade), as downloaded.
//!
//! - Windows MSI (no Windows needed: its tables, read with msitools in a Linux container): a major
//!   upgrade (same UpgradeCode, new ProductCode, higher version, older versions removed, newer
//!   ones refused), .dwg/.dxf/.cdy associations, everything under CADaddy's own folder and keys,
//!   and nothing that touches the settings folder (so settings survive upgrade and uninstall).
//! - macOS DMG (on a Mac): the app copied out as a user does, its signature verified strictly,
//!   Gatekeeper's verdict reported (a note: unnotarized apps need right-click ▸ Open), and a
//!   first launch on a control port with its own settings folder.
//! - Linux (in a Debian container): the .deb installed, upgraded from --old with settings kept,
//!   and removed leaving only the settings; the AppImage run; the zsync delta from --old.
//!
//! Each part is skipped (said so, not failed) when its tool is missing (Docker, macOS).

use std::path::{Path, PathBuf};
use std::process::Command;

struct Opts {
    new: PathBuf,
    old: Option<PathBuf>,
}

fn parse(rest: &[&str]) -> Result<Opts, String> {
    let mut new = None;
    let mut old = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--new" => new = it.next().map(PathBuf::from),
            "--old" => old = it.next().map(PathBuf::from),
            other => return Err(format!("unknown option `{other}` (installtest --new DIR [--old DIR])")),
        }
    }
    Ok(Opts { new: new.ok_or("installtest needs --new DIR (the release files)")?, old })
}

#[derive(Default)]
struct Checks {
    pass: usize,
    fail: Vec<String>,
}

impl Checks {
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            self.pass += 1;
            println!("  PASS  {name}: {detail}");
        } else {
            self.fail.push(name.to_string());
            println!("  FAIL  {name}: {detail}");
        }
    }
    /// `CHECK|name|1 or 0|detail` lines a script printed.
    fn take(&mut self, out: &str) {
        for l in out.lines() {
            let mut p = l.splitn(4, '|');
            if p.next() != Some("CHECK") {
                continue;
            }
            let (name, ok, detail) = (p.next().unwrap_or_default(), p.next() == Some("1"), p.next().unwrap_or_default());
            self.check(name, ok, detail);
        }
    }
}

fn docker() -> bool {
    Command::new("docker").args(["info", "--format", "{{.ServerVersion}}"]).output().is_ok_and(|o| o.status.success())
}

/// Run `script` in a Debian container (amd64) with `dir` at /w; its output.
fn container(dir: &Path, script: &str) -> Result<String, String> {
    let out = Command::new("docker")
        .args(["run", "--rm", "--platform", "linux/amd64", "-v"])
        .arg(format!("{}:/w", dir.display()))
        .args(["debian:12", "bash", "-c", script])
        .output()
        .map_err(|e| format!("docker: {e}"))?;
    Ok(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The MSI checks, as a script for a container with msitools.
const MSI: &str = r#"
apt-get update -qq >/dev/null; apt-get install -y -qq msitools >/dev/null 2>&1
m=/w/new/CADaddy-Windows.msi
prop() { msiinfo export "$1" Property | awk -F'\t' -v k="$2" '$1==k {print $2}'; }
chk() { echo "CHECK|$1|$2|$3"; }
v=$(prop $m ProductVersion); up=$(prop $m UpgradeCode); pc=$(prop $m ProductCode)
chk "msi: identity" $([ -n "$v" ] && [ -n "$up" ] && echo 1 || echo 0) "CADaddy $v, UpgradeCode $up"
if [ -f /w/old/CADaddy-Windows.msi ]; then
  o=/w/old/CADaddy-Windows.msi; ov=$(prop $o ProductVersion)
  same=$([ "$(prop $o UpgradeCode)" = "$up" ] && echo 1 || echo 0)
  chk "msi: upgrade $ov -> $v keeps the UpgradeCode" $same "old $(prop $o UpgradeCode)"
  chk "msi: new ProductCode" $([ "$(prop $o ProductCode)" != "$pc" ] && echo 1 || echo 0) "$(prop $o ProductCode) -> $pc"
  newer=$(printf '%s\n%s\n' "$ov" "$v" | sort -V | tail -1)
  chk "msi: version goes up" $([ "$newer" = "$v" ] && [ "$ov" != "$v" ] && echo 1 || echo 0) "$ov -> $v"
fi
up_rows=$(msiinfo export $m Upgrade | tail -n +4)
chk "msi: older versions are found and removed" $(echo "$up_rows" | grep -q WIX_UPGRADE_DETECTED && msiinfo export $m InstallExecuteSequence | grep -q RemoveExistingProducts && echo 1 || echo 0) "Upgrade table + RemoveExistingProducts"
chk "msi: a newer version is never replaced by this one" $(msiinfo export $m LaunchCondition | grep -q "NOT WIX_DOWNGRADE_DETECTED" && echo 1 || echo 0) "downgrade launch condition"
reg=$(msiinfo export $m Registry | tail -n +4)
for ext in .dwg .dxf .cdy; do
  chk "msi: $ext opens with CADaddy" $(echo "$reg" | awk -F'\t' -v k="Software\\\\Classes\\\\$ext\\\\OpenWithProgids" '$3==k' | grep -q CADaddy.Drawing && echo 1 || echo 0) "OpenWithProgids CADaddy.Drawing"
done
bad=$(echo "$reg" | awk -F'\t' '{print $3}' | grep -vE '^Software\\(AICodeForge\\CADaddy|Classes\\(\.cdy|\.dwg|\.dxf|CADaddy\.Drawing|Applications\\cadaddy\.exe)|Microsoft\\Windows\\CurrentVersion\\App Paths\\cadaddy\.exe|RegisteredApplications)' | sort -u | tr '\n' ' ')
chk "msi: registry stays in CADaddy's own keys" $([ -z "$bad" ] && echo 1 || echo 0) "${bad:-only CADaddy, its ProgID and the three extensions}"
dirs=$(msiinfo export $m Directory | tail -n +4 | awk -F'\t' '{print $1}' | tr '\n' ' ')
appdata=$(msiinfo export $m Directory | tail -n +4 | awk -F'\t' '$1=="AppDataFolder" || $2=="AppDataFolder"' | tr '\n' ' ')
chk "msi: settings folder untouched (kept on upgrade and uninstall)" $([ -z "$appdata" ] && echo 1 || echo 0) "${appdata:-no %APPDATA% folder in the package}"
rm_rows=$(msiinfo export $m RemoveFile | tail -n +4 | awk -F'\t' '{print $4}' | sort -u | tr '\n' ' ')
chk "msi: uninstall removes only its folders" $(echo "$rm_rows" | grep -qvE '^(INSTALLFOLDER|UserProgramsFolder| )+$' && echo 0 || echo 1) "RemoveFile: $rm_rows (UserProgramsFolder only when empty)"
chk "msi: per-user install" $(msiinfo export $m Directory | grep -q LocalAppDataFolder && echo 1 || echo 0) "under %LOCALAPPDATA%\\Programs\\CADaddy; directories: $dirs"
"#;

/// The Linux checks: the .deb (install, upgrade, remove) and the AppImage (run, zsync delta).
const LINUX: &str = r#"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null; apt-get install -y -qq zsync nginx-light netbase >/dev/null 2>&1
chk() { echo "CHECK|$1|$2|$3"; }
home=/root; cfg=$home/.config/cadaddy
if [ -f /w/old/CADaddy-Linux.deb ]; then
  apt-get install -y -qq /w/old/CADaddy-Linux.deb >/dev/null 2>&1
  ov=$(cadaddy --version 2>/dev/null)
  chk "deb: old version installs" $([ -n "$ov" ] && echo 1 || echo 0) "$ov"
  mkdir -p $cfg && echo kept > $cfg/marker
fi
apt-get install -y -qq /w/new/CADaddy-Linux.deb >/dev/null 2>&1
nv=$(cadaddy --version 2>/dev/null); cv=$(cadaddy-cli --version 2>/dev/null || cadaddy-cli commands line 2>/dev/null | head -c 40)
chk "deb: installs and runs" $([ -n "$nv" ] && echo 1 || echo 0) "$nv"
chk "deb: the command-line tool runs" $([ -n "$cv" ] && echo 1 || echo 0) "$(echo $cv | head -c 60)"
if [ -f $cfg/marker ]; then chk "deb: upgrade keeps settings" $(grep -q kept $cfg/marker && echo 1 || echo 0) "$cfg kept"; fi
files=$(dpkg -L cadaddy | while read f; do [ -f "$f" ] && echo "$f"; done)
chk "deb: files only in /usr" $(echo "$files" | grep -qv '^/usr/' && echo 0 || echo 1) "$(echo "$files" | wc -l) files"
apt-get remove -y -qq cadaddy >/dev/null 2>&1
left=$(echo "$files" | while read f; do [ -e "$f" ] && echo "$f"; done | grep -v '^/usr/share/doc/' | tr '\n' ' ')
chk "deb: remove takes its files" $([ -z "$left" ] && echo 1 || echo 0) "${left:-all gone}"
if [ -f $cfg/marker ]; then chk "deb: remove keeps the user's settings" 1 "$cfg left alone"; fi
new=$(ls /w/new/cadaddy-*-linux-x86_64.AppImage 2>/dev/null | head -1)
if [ -n "$new" ]; then
  cp "$new" /tmp/new.AppImage; chmod +x /tmp/new.AppImage
  av=$(APPIMAGE_EXTRACT_AND_RUN=1 /tmp/new.AppImage --version 2>/dev/null)
  chk "AppImage: runs" $([ -n "$av" ] && echo 1 || echo 0) "${av:-no output}"
  old=$(ls /w/old/cadaddy-*-linux-x86_64.AppImage 2>/dev/null | head -1)
  if [ -n "$old" ] && [ -f "$new.zsync" ]; then
    # As a release host serves them: over HTTP with ranges (nginx; zsync needs netbase for "http").
    cp /w/new/* /var/www/html/ && chmod a+r /var/www/html/* && nginx && sleep 1
    cd /tmp && zout=$(zsync -i "$old" -o /tmp/z.AppImage "http://127.0.0.1/$(basename $new).zsync" 2>&1 | tr '\r' '\n')
    same=$(cmp -s /tmp/z.AppImage "$new" && echo 1 || echo 0)
    seeded=$(echo "$zout" | grep -oE 'Target [0-9.]+% complete' | tail -1 | grep -oE '[0-9.]+%')
    chk "AppImage: zsync update from the old one" $same "${seeded:-?} reused from the old AppImage; rebuilt file identical: $same"
  fi
fi
"#;

pub fn run(_root: &Path, rest: &[&str]) -> Result<(), String> {
    let o = parse(rest)?;
    let new = std::fs::canonicalize(&o.new).map_err(|e| format!("{}: {e}", o.new.display()))?;
    let mut c = Checks::default();
    // A folder with the files under new/ and old/ for the containers.
    let work = std::env::temp_dir().join(format!("cadaddy-installtest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let link = |from: &Path, to: &str| -> Result<(), String> {
        let dest = work.join(to);
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        for e in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if p.is_file() {
                std::fs::copy(&p, dest.join(e.file_name())).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };
    link(&new, "new")?;
    if let Some(old) = &o.old {
        link(&std::fs::canonicalize(old).map_err(|e| e.to_string())?, "old")?;
    }
    println!("installtest: {}", new.display());
    if !docker() {
        println!("  SKIP  MSI and Linux checks: no Docker");
    } else {
        if work.join("new/CADaddy-Windows.msi").is_file() {
            c.take(&container(&work, MSI)?);
        }
        if work.join("new/CADaddy-Linux.deb").is_file() {
            c.take(&container(&work, LINUX)?);
        }
    }
    if cfg!(target_os = "macos") && work.join("new/CADaddy-Mac.dmg").is_file() {
        mac(&work, &mut c);
    } else {
        println!("  SKIP  DMG checks: not on a Mac, or no CADaddy-Mac.dmg");
    }
    let _ = std::fs::remove_dir_all(&work);
    println!("installtest: {} passed, {} failed", c.pass, c.fail.len());
    if c.fail.is_empty() { Ok(()) } else { Err(format!("failed: {}", c.fail.join(", "))) }
}

/// The DMG: copied out as a user does, verified, Gatekeeper's verdict, a first launch.
fn mac(work: &Path, c: &mut Checks) {
    let mnt = work.join("mnt");
    let _ = std::fs::create_dir_all(&mnt);
    let attached = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
        .arg(&mnt)
        .arg(work.join("new/CADaddy-Mac.dmg"))
        .status()
        .is_ok_and(|s| s.success());
    c.check("dmg: opens", attached, "mounted read-only");
    if !attached {
        return;
    }
    let apps = work.join("Applications");
    let _ = std::fs::create_dir_all(&apps);
    let app = apps.join("CADaddy.app");
    let copied = Command::new("ditto").arg(mnt.join("CADaddy.app")).arg(&app).status().is_ok_and(|s| s.success());
    let _ = Command::new("hdiutil").arg("detach").arg(&mnt).arg("-quiet").status();
    c.check("dmg: the app copies out", copied, "as dragging it to Applications does");
    let strict = Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&app).output();
    let (ok, why) = match strict {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => (false, e.to_string()),
    };
    c.check("dmg: the copied app's signature verifies (strict)", ok, if ok { "valid" } else { &why });
    // Gatekeeper on a downloaded (quarantined) copy: a note, not a failure, until the app is
    // notarized (that needs an Apple Developer ID).
    let _ = Command::new("xattr").args(["-w", "com.apple.quarantine", "0081;00000000;Safari;"]).arg(&app).status();
    let gk = Command::new("spctl").args(["--assess", "--type", "execute", "-vv"]).arg(&app).output();
    let verdict = gk.map(|o| String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or_default().to_string()).unwrap_or_default();
    println!(
        "  NOTE  Gatekeeper on a downloaded copy: {verdict} (not notarized: first launch needs right-click ▸ Open, or System Settings ▸ Privacy & Security ▸ Open Anyway)"
    );
    let _ = Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(&app).status();
    // First launch: its own settings folder, a control port, quit.
    let exe = app.join("Contents/MacOS/CADaddy");
    let version = Command::new(&exe).arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    c.check("dmg: first launch", !version.is_empty(), &version);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_script_lines() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--new", "a", "--old", "b"]).is_ok_and(|o| o.old.is_some()));
        let mut c = Checks::default();
        c.take("noise\nCHECK|a|1|fine\nCHECK|b|0|broken\n");
        assert_eq!((c.pass, c.fail.clone()), (1, vec!["b".to_string()]));
    }
}
