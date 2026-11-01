//! The process's memory, for the job's progress (Q6099: find the stage where the peak jumps).
//! Read without unsafe code: the peak from `getrusage` on Linux and macOS (its maximum resident
//! size), Windows through PowerShell (`PeakWorkingSet64`); the memory now from Linux's
//! `/proc/self/status`, macOS's `ps`, the same PowerShell call.

/// The resident memory now and the peak so far, MB; None where it can't be read.
pub(crate) fn process_mb() -> Option<(u64, u64)> {
    let (now, peak) = read()?;
    Some((now, peak.max(now)))
}

/// The most resident memory the process has had, MB (`ru_maxrss`: kilobytes on Linux, bytes on
/// macOS).
#[cfg(unix)]
fn peak_mb() -> Option<u64> {
    let max = nix::sys::resource::getrusage(nix::sys::resource::UsageWho::RUSAGE_SELF).ok()?.max_rss();
    let max = u64::try_from(max).ok()?;
    Some(if cfg!(target_os = "macos") { max >> 20 } else { max >> 10 })
}

#[cfg(target_os = "linux")]
fn read() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = |key: &str| -> Option<u64> { status.lines().find_map(|l| l.strip_prefix(key))?.split_whitespace().next()?.parse().ok() };
    let now = kb("VmRSS:")? / 1024;
    Some((now, peak_mb().or_else(|| kb("VmHWM:").map(|k| k / 1024)).unwrap_or(now)))
}

#[cfg(target_os = "windows")]
fn read() -> Option<(u64, u64)> {
    use std::os::windows::process::CommandExt;
    /// No console window flashes up over the app.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let pid = std::process::id();
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &format!("$p = Get-Process -Id {pid}; \"$($p.WorkingSet64) $($p.PeakWorkingSet64)\"")])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.split_whitespace().map(|v| v.parse::<u64>().ok());
    let (now, peak) = (it.next()??, it.next()??);
    Some((now >> 20, peak >> 20))
}

#[cfg(target_os = "macos")]
fn read() -> Option<(u64, u64)> {
    let out = std::process::Command::new("/bin/ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    let now = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok()? / 1024;
    Some((now, peak_mb().unwrap_or(now)))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn read() -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_peak_is_never_under_the_memory_now() {
        if let Some((now, peak)) = super::process_mb() {
            assert!(now > 0 && peak >= now, "{now} {peak}");
        }
    }
}
