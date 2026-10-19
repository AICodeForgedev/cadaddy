//! Writing files from commands: like `std::fs::write`, except during an agent's dry run
//! ([`crate::agent`]), when nothing is written and the command is told so.

/// Write `bytes` to `path` (refused during a dry run).
pub fn write(path: impl AsRef<std::path::Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    check(path.as_ref())?;
    std::fs::write(path, bytes)
}

/// An error when files may not be written now (an agent's dry run).
pub fn check(path: &std::path::Path) -> std::io::Result<()> {
    if crate::agent::writes_allowed() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("dry run: {} was not written", path.display())))
    }
}
