//! The control port's token: when the app requires one (the default), it writes a fresh random
//! token for each port it listens on to a file only the user can read, and a connection must
//! present it before anything else (`{"method": "auth", "params": {"token": …}}`, or a `token`
//! field on its first request). Other programs on the computer that can't read the user's files
//! can no longer drive the app. Clients (`cadaddy-cli mcp`, scripts) find the token here.

use std::path::{Path, PathBuf};

/// A client's token from the environment, ahead of the file.
pub const TOKEN_ENV: &str = "CADADDY_CONTROL_TOKEN";

/// The per-user settings directory: `CADADDY_CONFIG_DIR` if set, else `$XDG_CONFIG_HOME/cadaddy`
/// or `~/.config/cadaddy` on Linux and the BSDs, `~/Library/Application Support/CADaddy` on macOS,
/// `%APPDATA%\CADaddy` on Windows.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(|k| std::env::var_os(k))
}

/// [`config_dir`] with the environment passed in. An empty variable counts as unset, so a blank
/// value never means the working directory.
pub fn config_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("CADADDY_CONFIG_DIR") {
        return Some(dir);
    }
    if cfg!(target_os = "macos") {
        set("HOME").map(|h| h.join("Library/Application Support/CADaddy"))
    } else if cfg!(windows) {
        set("APPDATA").map(|a| a.join("CADaddy"))
    } else {
        // The XDG spec ignores a relative XDG_CONFIG_HOME.
        let base = set("XDG_CONFIG_HOME").filter(|p| p.is_absolute()).or_else(|| set("HOME").map(|h| h.join(".config")))?;
        Some(base.join("cadaddy"))
    }
}

/// Where the token for `port` is kept under the settings directory `dir`.
pub fn token_path(dir: &Path, port: u16) -> PathBuf {
    dir.join("control").join(format!("{port}.token"))
}

/// The token a client sends to `port`: `CADADDY_CONTROL_TOKEN`, else the app's token file.
pub fn client_token(port: u16) -> Option<String> {
    if let Some(t) = std::env::var(TOKEN_ENV).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        return Some(t);
    }
    let text = std::fs::read_to_string(token_path(&config_dir()?, port)).ok()?;
    Some(text.trim().to_string()).filter(|t| !t.is_empty() && t.len() <= 256)
}

/// Write `token` to `path`, readable by the user only (0600 in a 0700 folder on Unix; on Windows
/// the settings folder under the user's profile is already private to them).
pub fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    // A fresh file each time, so an earlier one's permissions don't carry over.
    let _ = std::fs::remove_file(path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(token.as_bytes())?;
    f.write_all(b"\n")
}

/// Whether `given` is `token`, compared in time that doesn't depend on where they differ.
pub fn same(given: &str, token: &str) -> bool {
    let (a, b) = (given.as_bytes(), token.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (i, x) in b.iter().enumerate() {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ x);
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_exactly() {
        assert!(same("abc123", "abc123"));
        for bad in ["", "abc12", "abc1234", "abd123", "ABC123"] {
            assert!(!same(bad, "abc123"), "{bad}");
        }
    }

    #[test]
    fn the_token_file_is_private_and_found_by_port() {
        let dir = std::env::temp_dir().join(format!("cadaddy-token-{}", std::process::id()));
        let p = token_path(&dir, 47999);
        assert!(p.ends_with("control/47999.token"));
        write_token(&p, "secret").unwrap();
        write_token(&p, "again").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "again\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
