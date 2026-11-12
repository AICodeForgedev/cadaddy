//! The process's memory, for the job's progress (Q6099: find the stage where the peak jumps).
//! Read without unsafe code: the peak from `getrusage` on Linux (its maximum resident size),
//! Windows through PowerShell (`PeakWorkingSet64`); the memory now from Linux's
//! `/proc/self/status`, the same PowerShell call. macOS: the physical footprint and its peak
//! (`phys_footprint`, what Activity Monitor and `time -l` count) from the system's `footprint`
//! tool (about 20 ms); its resident size (`ps`) also counts pages the allocator has already given
//! back for reuse, so after a stage that freed a lot it said several GB more than the process
//! held (Q6389: 3.5 GB resident beside a 1.2 GB footprint after reading a print set). `ps` and
//! `getrusage` only where `footprint` can't be run. On Windows that call takes about
//! 0.3 s: a stage line within `FRESH_S` (2 s) of the last reading says the one taken in the
//! background since (Q6329: seven stages of a job that built nothing waited 2 s on it); after a
//! longer stage it is read then, so the peak is the stage's own.

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

/// A reading this recent (seconds) is said again for a stage line, and a new one taken in the
/// background; an older one is read again before the line (a long stage's peak is its own).
#[cfg(target_os = "windows")]
const FRESH_S: f64 = 2.0;

/// The latest reading and when it was taken, and whether one is being taken in the background.
#[cfg(target_os = "windows")]
static LATEST: std::sync::Mutex<Option<((u64, u64), web_time::Instant)>> = std::sync::Mutex::new(None);
#[cfg(target_os = "windows")]
static READING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "windows")]
fn keep(v: (u64, u64)) {
    if let Ok(mut l) = LATEST.lock() {
        *l = Some((v, web_time::Instant::now()));
    }
}

/// None before the first reading is in (it is taken in the background), the latest when it is
/// `FRESH_S` old or less (a new one started in the background), else a reading taken now.
#[cfg(target_os = "windows")]
fn read() -> Option<(u64, u64)> {
    use std::sync::atomic::Ordering;
    let latest = LATEST.lock().ok().and_then(|l| *l);
    if let Some((_, at)) = latest
        && at.elapsed().as_secs_f64() > FRESH_S
    {
        let v = read_now()?;
        keep(v);
        return Some(v);
    }
    if !READING.swap(true, Ordering::AcqRel) {
        let started = std::thread::Builder::new().name("memstat".into()).spawn(|| {
            if let Some(v) = read_now() {
                keep(v);
            }
            READING.store(false, Ordering::Release);
        });
        if started.is_err() {
            READING.store(false, Ordering::Release);
        }
    }
    latest.map(|(v, _)| v)
}

#[cfg(target_os = "windows")]
fn read_now() -> Option<(u64, u64)> {
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
    let pid = std::process::id().to_string();
    let footprint = std::process::Command::new("/usr/bin/footprint").args(["-p", &pid, "--noCategories", "-f", "bytes"]).output().ok();
    if let Some(v) = footprint.and_then(|o| footprint_mb(&String::from_utf8_lossy(&o.stdout))) {
        return Some(v);
    }
    let out = std::process::Command::new("/bin/ps").args(["-o", "rss=", "-p", &pid]).output().ok()?;
    let now = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok()? / 1024;
    Some((now, peak_mb().unwrap_or(now)))
}

/// The physical footprint now and its peak, MB, from `footprint --noCategories -f bytes`'s
/// `phys_footprint: <n> B` and `phys_footprint_peak: <n> B` lines (the peak is the footprint
/// when that line is missing).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn footprint_mb(text: &str) -> Option<(u64, u64)> {
    let bytes = |key: &str| -> Option<u64> {
        text.lines().find_map(|l| l.trim_start().strip_prefix(key)?.strip_prefix(':')?.split_whitespace().next()?.parse().ok())
    };
    let now = bytes("phys_footprint")? >> 20;
    Some((now, bytes("phys_footprint_peak").map_or(now, |p| p >> 20)))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn read() -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    /// macOS's `footprint` summary: the auxiliary lines give the footprint and its peak.
    #[test]
    fn the_footprint_tools_numbers_are_read() {
        let text = "======\nsample [42]: 64-bit    Footprint: 1918896936 B (16384 bytes per page)\n======\n\nAuxiliary data:\n    phys_footprint: 1918913320 B\n    phys_footprint_peak: 3033140008 B\n";
        assert_eq!(super::footprint_mb(text), Some((1830, 2892)));
        assert_eq!(super::footprint_mb("    phys_footprint: 1048576 B\n"), Some((1, 1)));
        assert_eq!(super::footprint_mb("footprint: no such process\n"), None);
        // The `Footprint:` headline is not the auxiliary line.
        assert_eq!(super::footprint_mb("x [1]: 64-bit    Footprint: 5 B\n"), None);
    }

    #[test]
    fn the_peak_is_never_under_the_memory_now() {
        if let Some((now, peak)) = super::process_mb() {
            assert!(now > 0 && peak >= now, "{now} {peak}");
        }
    }
}
