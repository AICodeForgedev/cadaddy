//! Writing files from commands: like `std::fs::write`, except during an agent's dry run
//! ([`crate::agent`]), when nothing is written and the command is told so, and never into a
//! system folder (a path a drawing or script supplied can't put files where the system runs
//! them from).

/// Write `bytes` to `path` (refused during a dry run).
pub fn write(path: impl AsRef<std::path::Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    check(path.as_ref())?;
    std::fs::write(path, bytes)
}

/// An error when files may not be written now (an agent's dry run) or not there (a system folder).
pub fn check(path: &std::path::Path) -> std::io::Result<()> {
    protected(path)?;
    if crate::agent::writes_allowed() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("dry run: {} was not written", path.display())))
    }
}

/// An error for a path in a system folder ([`cadaddy_geom::filename::system_folder`]).
pub fn protected(path: &std::path::Path) -> std::io::Result<()> {
    match cadaddy_geom::filename::system_folder(path) {
        Some(folder) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is in a system folder ({folder}); CADaddy doesn't write there", path.display()),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_is_written_into_system_folders() {
        let e = super::write("/etc/cadaddy-test.dwg", b"x").unwrap_err();
        assert!(e.to_string().contains("system folder"), "{e}");
        assert!(super::check(std::path::Path::new("C:\\Windows\\System32\\evil.dll")).is_err());
        assert!(super::check(&std::env::temp_dir().join("ok.dwg")).is_ok());
    }
}
