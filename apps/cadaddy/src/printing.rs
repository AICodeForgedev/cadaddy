//! System printers for PLOT's `printer`: listed and printed to through the operating system's
//! own tools — CUPS (`lpstat`, `lp`) on macOS, Linux and the BSDs; the print spooler through
//! PowerShell on Windows (the PDF printed with its registered application's "print to").

use std::process::Command;

/// Printer names, as the system lists them.
pub fn list() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Get-Printer | Select-Object -ExpandProperty Name"])
            .output()
            .map_err(|e| format!("powershell: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).take(1000).collect())
    }
    #[cfg(not(windows))]
    {
        // `lpstat -e` lists destinations one per line; older CUPS only has `lpstat -p`.
        if let Ok(out) = Command::new("lpstat").arg("-e").output()
            && out.status.success()
        {
            return Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .take(1000)
                .collect());
        }
        let out = Command::new("lpstat").arg("-p").output().map_err(|e| format!("lpstat: {e}"))?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.strip_prefix("printer ").and_then(|r| r.split_whitespace().next()))
            .map(str::to_string)
            .take(1000)
            .collect())
    }
}

/// Print `pdf` on `printer`, as job `title`.
pub fn print(printer: &str, pdf: &[u8], title: &str) -> Result<(), String> {
    if printer.chars().any(char::is_control) || printer.len() > 256 {
        return Err("not a printer name".into());
    }
    let known = list().unwrap_or_default();
    if !known.is_empty() && !known.iter().any(|n| n == printer) {
        return Err(format!("no printer `{printer}` (printers: {})", known.join(", ")));
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("cadaddy-plot-{}-{stamp}.pdf", std::process::id()));
    std::fs::write(&path, pdf).map_err(|e| format!("{}: {e}", path.display()))?;
    let title: String = title.chars().filter(|c| !c.is_control()).take(200).collect();
    #[cfg(windows)]
    let status = Command::new("powershell")
        // Names travel in the environment, never inside the command text.
        .env("CADADDY_PRINTER", printer)
        .env("CADADDY_FILE", &path)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Process -FilePath $env:CADADDY_FILE -Verb PrintTo -ArgumentList ('\"' + $env:CADADDY_PRINTER + '\"') -Wait",
        ])
        .status();
    #[cfg(not(windows))]
    let status = Command::new("lp").arg("-d").arg(printer).arg("-t").arg(if title.is_empty() { "CADaddy plot" } else { &title }).arg(&path).status();
    let r = match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("the print command failed ({s})")),
        Err(e) => Err(format!("the print command could not run: {e}")),
    };
    // CUPS copies the file when the job is queued; on Windows the application has it by now.
    let _ = std::fs::remove_file(&path);
    let _ = title;
    r
}
