//! Release signing (apps/cadaddy/src/relsig.rs has the format the updater checks):
//! - `release-key --out FILE`: a new ed25519 key; the secret goes to FILE (never into the
//!   repository; owner-only permissions), the public key line to paste into
//!   `apps/cadaddy/src/release_keys.txt` is printed.
//! - `sign-release DIR [--version X.Y.Z]`: SHA256SUMS of the installers in DIR and
//!   SHA256SUMS.sig, signed with the key file named by `CADADDY_RELEASE_KEY`; upload both with
//!   the installers. Refuses a key the app doesn't trust.
//! - `verify-release DIR [--version X.Y.Z]`: what the updater checks, before uploading.

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

#[path = "../../apps/cadaddy/src/relsig.rs"]
mod relsig;

/// The environment variable naming the secret key file.
pub const KEY_ENV: &str = "CADADDY_RELEASE_KEY";

fn version_arg(root: &Path, args: &[&str]) -> Result<String, String> {
    match args.iter().position(|a| *a == "--version") {
        Some(i) => args.get(i + 1).map(|v| v.trim_start_matches('v').to_string()).ok_or_else(|| "--version needs X.Y.Z".to_string()),
        None => crate::version::read(&std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?),
    }
}

fn dir_arg<'a>(args: &[&'a str], cmd: &str) -> Result<&'a str, String> {
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if *a == "--version" {
            skip = true;
        } else if !a.starts_with("--") {
            return Ok(a);
        }
    }
    Err(format!("usage: cargo xtask {cmd} DIR [--version X.Y.Z]"))
}

pub fn key_cmd(args: &[&str]) -> Result<(), String> {
    let out = match args {
        ["--out", f] => PathBuf::from(f),
        _ => return Err("usage: cargo xtask release-key --out FILE (outside the repository)".into()),
    };
    if out.exists() {
        return Err(format!("{} exists; a release key is never overwritten", out.display()));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no system randomness: {e}"))?;
    let sk = SigningKey::from_bytes(&seed);
    write_secret(&out, &format!("{}\n", relsig::hex(&seed)))?;
    seed.fill(0);
    let pk = sk.verifying_key().to_bytes();
    println!("Secret key written to {} (keep it off the repository, backed up).", out.display());
    println!("Add this line to apps/cadaddy/src/release_keys.txt:\n{}  # key {}", relsig::hex(&pk), relsig::key_id(&pk));
    Ok(())
}

fn write_secret(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_key() -> Result<SigningKey, String> {
    let path = std::env::var_os(KEY_ENV).ok_or_else(|| format!("set {KEY_ENV} to the release key file (made by `cargo xtask release-key`)"))?;
    let path = PathBuf::from(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(format!("{} can be read by other users (chmod 600 it)", path.display()));
        }
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t = text.trim();
    let bytes: Option<Vec<u8>> =
        (t.len() == 64).then(|| (0..32).map(|i| t.get(i * 2..i * 2 + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()).flatten();
    let mut seed: [u8; 32] = bytes.and_then(|b| b.try_into().ok()).ok_or_else(|| format!("{} isn't a release key", path.display()))?;
    let sk = SigningKey::from_bytes(&seed);
    seed.fill(0);
    Ok(sk)
}

/// The release files in `dir`: every regular, non-hidden file but the sums and signature.
fn release_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| !n.starts_with('.') && n != relsig::SUMS && n != relsig::SIG))
        .collect();
    v.sort();
    if v.is_empty() {
        return Err(format!("{}: no release files", dir.display()));
    }
    Ok(v)
}

fn sha256(p: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", p.display()))?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(relsig::hex(&h.finalize()))
}

/// SHA256SUMS's text for `files` (by name, as `sha256sum` writes it).
pub fn sums_text(files: &[PathBuf]) -> Result<String, String> {
    let mut s = String::new();
    for f in files {
        let name = f.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{}: not a UTF-8 name", f.display()))?;
        s.push_str(&format!("{}  {name}\n", sha256(f)?));
    }
    Ok(s)
}

pub fn sign_cmd(root: &Path, args: &[&str]) -> Result<(), String> {
    let dir = Path::new(dir_arg(args, "sign-release")?);
    let version = version_arg(root, args)?;
    let sk = read_key()?;
    let pk = sk.verifying_key().to_bytes();
    if !relsig::trusted_keys().contains(&pk) {
        return Err(format!("this key ({}) isn't in apps/cadaddy/src/release_keys.txt: updaters would refuse the release", relsig::key_id(&pk)));
    }
    let sums = sums_text(&release_files(dir)?)?;
    let sig = sk.sign(&relsig::message(&version, sums.as_bytes())).to_bytes();
    std::fs::write(dir.join(relsig::SUMS), &sums).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(relsig::SIG), relsig::sig_file(&version, &pk, &sig)).map_err(|e| e.to_string())?;
    print!("{sums}");
    println!(
        "signed {} for {version} with key {}; upload {} and {} with the installers",
        dir.display(),
        relsig::key_id(&pk),
        relsig::SUMS,
        relsig::SIG
    );
    verify_dir(dir, &version)
}

pub fn verify_cmd(root: &Path, args: &[&str]) -> Result<(), String> {
    let dir = Path::new(dir_arg(args, "verify-release")?);
    verify_dir(dir, &version_arg(root, args)?)
}

/// What the updater checks: the signature with the app's keys, then every file's hash.
fn verify_dir(dir: &Path, version: &str) -> Result<(), String> {
    let sums = std::fs::read_to_string(dir.join(relsig::SUMS)).map_err(|e| format!("{}: {e}", relsig::SUMS))?;
    let sig = std::fs::read_to_string(dir.join(relsig::SIG)).map_err(|e| format!("{}: {e}", relsig::SIG))?;
    relsig::verify(version, sums.as_bytes(), &sig, &relsig::trusted_keys()).map_err(|e| e.to_string())?;
    for f in release_files(dir)? {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        match relsig::sum_for(&sums, name) {
            Some(h) if h == sha256(&f)? => {}
            Some(_) => return Err(format!("{name} doesn't match SHA256SUMS")),
            None => return Err(format!("{name} isn't in SHA256SUMS")),
        }
    }
    println!("{}: signature and hashes check out for {version}", dir.display());
    Ok(())
}
