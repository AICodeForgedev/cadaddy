//! System printers for PLOT's `printer`: listed and printed to through the operating system's
//! own tools — CUPS (`lpstat`, `lp`) on macOS, Linux and the BSDs; the print spooler through
//! PowerShell on Windows (each PDF page rasterised with the system's PDF renderer and printed
//! with .NET's `PrintDocument`, so no PDF application is needed; "print to" is the fallback).
//!
//! The command builders and the parsers are pure functions (tested on canned output on every
//! platform); only `list` and `print` run programs.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest printer name accepted.
const MAX_NAME: usize = 256;
/// Most printers listed.
const MAX_PRINTERS: usize = 1000;
/// How long a listing may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a print command may take before it is stopped and reported.
const PRINT_TIMEOUT: Duration = Duration::from_secs(120);
/// Most error text carried back.
const MAX_ERR: usize = 600;

/// A program to run: its arguments and environment (names travel in the environment, never in
/// command text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

fn cmd(program: &str, args: &[&str]) -> Cmd {
    Cmd { program: program.into(), args: args.iter().map(|a| (*a).to_string()).collect(), env: vec![] }
}

fn clean_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    names
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}'))
        .filter(|l| !l.is_empty() && !l.chars().any(char::is_control))
        .map(str::to_string)
        .take(MAX_PRINTERS)
        .collect()
}

/// `lpstat -e`: one destination per line.
pub fn parse_lpstat_e(out: &str) -> Vec<String> {
    clean_names(out.lines())
}

/// `lpstat -p`: `printer NAME is idle.` / `printer NAME disabled since …` lines (other lines are
/// job or detail text).
pub fn parse_lpstat_p(out: &str) -> Vec<String> {
    clean_names(out.lines().filter_map(|l| l.strip_prefix("printer ").and_then(|r| r.split_whitespace().next())))
}

/// `Get-Printer | Select-Object -ExpandProperty Name`: one name per line (names may hold spaces).
pub fn parse_get_printer(out: &str) -> Vec<String> {
    clean_names(out.lines())
}

/// The PowerShell command that lists printers.
pub fn windows_list_command() -> Cmd {
    cmd("powershell", &["-NoProfile", "-NonInteractive", "-Command", "Get-Printer | Select-Object -ExpandProperty Name"])
}

/// The name check: a name must be printable text of a sane length, and — when the system listed
/// printers — one of them. An empty list is an error that says what to do, never a pass.
pub fn check_name(printer: &str, known: &[String]) -> Result<(), String> {
    if printer.trim().is_empty() || printer.chars().any(char::is_control) || printer.chars().count() > MAX_NAME {
        return Err("not a printer name".into());
    }
    if known.is_empty() {
        return Err(format!(
            "no printers are installed or the printer list could not be read, so `{printer}` can't be checked: add a printer in the system settings, or plot to a PDF file instead"
        ));
    }
    if !known.iter().any(|n| n == printer) {
        return Err(format!("no printer `{printer}` (printers: {})", known.join(", ")));
    }
    Ok(())
}

/// A job title without control characters, at most 200 characters.
pub fn clean_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// Rasterise each page of the PDF with the system renderer and print the pages on the printer.
const WINDOWS_RASTER_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Data.Pdf.PdfDocument, Windows.Data.Pdf, ContentType = WindowsRuntime]
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.InMemoryRandomAccessStream, Windows.Storage.Streams, ContentType = WindowsRuntime]
$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } | Select-Object -First 1
function Wait-Op($op, $type) {
    $task = $asTask.MakeGenericMethod($type).Invoke($null, @($op))
    if (-not $task.Wait(60000)) { throw 'the system PDF renderer timed out' }
    $task.Result
}
$file = Wait-Op ([Windows.Storage.StorageFile]::GetFileFromPathAsync($env:CADADDY_FILE)) ([Windows.Storage.StorageFile])
$pdf = Wait-Op ([Windows.Data.Pdf.PdfDocument]::LoadFromFileAsync($file)) ([Windows.Data.Pdf.PdfDocument])
if ($pdf.PageCount -lt 1) { throw 'the PDF has no pages' }
$pages = New-Object System.Collections.ArrayList
for ($i = 0; $i -lt [Math]::Min($pdf.PageCount, 200); $i++) {
    $page = $pdf.GetPage($i)
    $opt = New-Object Windows.Data.Pdf.PdfPageRenderOptions
    $scale = [Math]::Min(300.0 / 72.0, 6000.0 / [Math]::Max($page.Size.Width, $page.Size.Height))
    $opt.DestinationWidth = [uint32]($page.Size.Width * $scale)
    $opt.DestinationHeight = [uint32]($page.Size.Height * $scale)
    $ms = New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
    $act = $page.RenderToStreamAsync($ms, $opt)
    $t = [System.WindowsRuntimeSystemExtensions]::AsTask($act)
    if (-not $t.Wait(60000)) { throw 'the system PDF renderer timed out' }
    $s = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($ms.GetInputStreamAt(0))
    $copy = New-Object System.IO.MemoryStream
    $s.CopyTo($copy)
    $null = $pages.Add([System.Drawing.Image]::FromStream($copy))
    $page.Dispose()
}
$doc = New-Object System.Drawing.Printing.PrintDocument
$doc.PrinterSettings.PrinterName = $env:CADADDY_PRINTER
if (-not $doc.PrinterSettings.IsValid) { throw ('printer not available: ' + $env:CADADDY_PRINTER) }
$doc.DocumentName = $env:CADADDY_TITLE
$doc.PrintController = New-Object System.Drawing.Printing.StandardPrintController
$script:n = 0
$doc.add_PrintPage({
    param($sender, $e)
    $img = $pages[$script:n]
    $e.PageSettings.Landscape = ($img.Width -gt $img.Height)
    $b = $e.PageBounds
    $k = [Math]::Min($b.Width / $img.Width, $b.Height / $img.Height)
    $w = [int]($img.Width * $k); $h = [int]($img.Height * $k)
    $e.Graphics.DrawImage($img, [int](($b.Width - $w) / 2), [int](($b.Height - $h) / 2), $w, $h)
    $script:n++
    $e.HasMorePages = ($script:n -lt $pages.Count)
})
$doc.Print()
"#;

/// The fallback: the PDF's registered application's "print to" verb (not on a stock machine).
const WINDOWS_PRINTTO_SCRIPT: &str =
    "Start-Process -FilePath $env:CADADDY_FILE -Verb PrintTo -ArgumentList ('\"' + $env:CADADDY_PRINTER + '\"') -WindowStyle Hidden";

fn windows_command(script: &str, printer: &str, file: &str, title: &str) -> Cmd {
    Cmd {
        program: "powershell".into(),
        args: ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script].iter().map(|a| (*a).to_string()).collect(),
        env: vec![("CADADDY_PRINTER".into(), printer.into()), ("CADADDY_FILE".into(), file.into()), ("CADADDY_TITLE".into(), title.into())],
    }
}

/// The Windows print commands in the order they are tried: rasterise-and-print through the
/// spooler first, "print to" only if that fails. The printer, file and title travel as
/// environment variables, never inside the command text.
pub fn windows_print_commands(printer: &str, file: &str, title: &str) -> Vec<Cmd> {
    vec![windows_command(WINDOWS_RASTER_SCRIPT, printer, file, title), windows_command(WINDOWS_PRINTTO_SCRIPT, printer, file, title)]
}

/// The CUPS command.
pub fn cups_print_command(printer: &str, file: &str, title: &str) -> Cmd {
    let title = if title.is_empty() { "CADaddy plot" } else { title };
    cmd("lp", &["-d", printer, "-t", title, file])
}

/// A failure message: what ran, how it ended and what it said.
pub fn failure_text(what: &str, code: Option<i32>, stderr: &str) -> String {
    let said: String = stderr.trim().chars().take(MAX_ERR).collect();
    let how = match code {
        Some(c) => format!("exit code {c}"),
        None => "stopped".to_string(),
    };
    if said.is_empty() { format!("{what} failed ({how}, no message)") } else { format!("{what} failed ({how}): {said}") }
}

struct Ran {
    code: Option<i32>,
    ok: bool,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// Run `c`, stop it after `limit`, and collect what it wrote.
fn run(c: &Cmd, limit: Duration) -> Result<Ran, String> {
    let mut child = Command::new(&c.program)
        .args(&c.args)
        .envs(c.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} could not run: {e}", c.program))?;
    // Read the pipes on threads so a chatty child can't fill them and stall.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.take(1 << 20).read_to_end(&mut s);
        }
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.take(1 << 16).read_to_end(&mut s);
        }
        s
    });
    let start = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(s)) => break (Some(s), false),
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{}: {e}", c.program)),
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    Ok(Ran {
        code: status.and_then(|s| s.code()),
        ok: status.is_some_and(|s| s.success()),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
    })
}

/// Run a print command; the error says why it failed.
fn run_print(what: &str, c: &Cmd) -> Result<(), String> {
    let r = run(c, PRINT_TIMEOUT)?;
    if r.ok {
        Ok(())
    } else if r.timed_out {
        Err(format!("{what} did not finish within {} s and was stopped", PRINT_TIMEOUT.as_secs()))
    } else {
        Err(failure_text(what, r.code, &r.stderr))
    }
}

/// Printer names, as the system lists them.
pub fn list() -> Result<Vec<String>, String> {
    if cfg!(windows) {
        let r = run(&windows_list_command(), LIST_TIMEOUT)?;
        if !r.ok {
            return Err(failure_text("listing printers", r.code, &r.stderr));
        }
        return Ok(parse_get_printer(&r.stdout));
    }
    // `lpstat -e` lists destinations one per line; older CUPS only has `lpstat -p`.
    if let Ok(r) = run(&cmd("lpstat", &["-e"]), LIST_TIMEOUT)
        && r.ok
    {
        return Ok(parse_lpstat_e(&r.stdout));
    }
    let r = run(&cmd("lpstat", &["-p"]), LIST_TIMEOUT)?;
    Ok(parse_lpstat_p(&r.stdout))
}

/// Print `pdf` on `printer`, as job `title`.
pub fn print(printer: &str, pdf: &[u8], title: &str) -> Result<(), String> {
    // A name that is not a name is refused before the (slower) listing.
    check_name(printer, &[printer.to_string()])?;
    check_name(printer, &list()?)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("cadaddy-plot-{}-{stamp}.pdf", std::process::id()));
    std::fs::write(&path, pdf).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = path.to_string_lossy().into_owned();
    let title = clean_title(title);
    let r = if cfg!(windows) {
        let cmds = windows_print_commands(printer, &file, &title);
        let first = cmds.first().map(|c| run_print("printing through the print spooler", c)).unwrap_or(Ok(()));
        match first {
            Ok(()) => Ok(()),
            Err(why) => match cmds.get(1).map(|c| run_print("the PDF application's print-to", c)) {
                Some(Ok(())) => Ok(()),
                Some(Err(fallback)) => Err(format!("{why}; then {fallback}")),
                None => Err(why),
            },
        }
    } else {
        run_print("lp", &cups_print_command(printer, &file, &title))
    };
    // CUPS copies the file when the job is queued; on Windows the job has been spooled by now.
    let _ = std::fs::remove_file(&path);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["Office_Laser".into(), "Plotter 24in".into()]
    }

    #[test]
    fn lpstat_e_lists_one_destination_per_line() {
        assert_eq!(parse_lpstat_e("Office_Laser\r\n  Plotter_A1 \n\n"), vec!["Office_Laser", "Plotter_A1"]);
        assert!(parse_lpstat_e("").is_empty());
    }

    #[test]
    fn lpstat_p_takes_the_name_after_printer() {
        let out = "printer Office_Laser is idle.  enabled since Mon 02 Oct 2026\nprinter Plotter_A1 disabled since Mon 02 Oct 2026 -\n\treason unknown\nOffice_Laser-12 root 1024 Mon\n";
        assert_eq!(parse_lpstat_p(out), vec!["Office_Laser", "Plotter_A1"]);
        assert!(parse_lpstat_p("lpstat: No destinations added.\n").is_empty());
    }

    #[test]
    fn get_printer_keeps_names_with_spaces() {
        assert_eq!(
            parse_get_printer("\u{feff}Microsoft Print to PDF\r\nOffice Laser (Copy 1)\r\n\r\n"),
            vec!["Microsoft Print to PDF", "Office Laser (Copy 1)"]
        );
        assert!(parse_get_printer("\r\n  \r\n").is_empty());
    }

    #[test]
    fn listing_is_capped() {
        let many: String = (0..1500).map(|i| format!("p{i}\n")).collect();
        assert_eq!(parse_lpstat_e(&many).len(), 1000);
    }

    #[test]
    fn name_check_accepts_a_listed_name() {
        assert_eq!(check_name("Plotter 24in", &known()), Ok(()));
    }

    #[test]
    fn name_check_refuses_control_characters_and_long_names() {
        assert_eq!(check_name("Office\nLaser", &known()), Err("not a printer name".to_string()));
        assert_eq!(check_name("Office\u{0}Laser", &known()), Err("not a printer name".to_string()));
        assert_eq!(check_name("", &known()), Err("not a printer name".to_string()));
        assert_eq!(check_name(&"x".repeat(257), &known()), Err("not a printer name".to_string()));
        let edge = "x".repeat(256);
        assert_eq!(check_name(&edge, std::slice::from_ref(&edge)), Ok(()));
    }

    #[test]
    fn unknown_name_lists_the_known_printers() {
        assert_eq!(check_name("Copier", &known()), Err("no printer `Copier` (printers: Office_Laser, Plotter 24in)".to_string()));
        // Names are exact: a different case is a different (unknown) name.
        assert!(check_name("office_laser", &known()).is_err());
    }

    #[test]
    fn empty_list_guides_instead_of_passing_the_name_on() {
        let e = check_name("Office_Laser", &[]).unwrap_err();
        assert_eq!(
            e,
            "no printers are installed or the printer list could not be read, so `Office_Laser` can't be checked: add a printer in the system settings, or plot to a PDF file instead"
        );
    }

    #[test]
    fn titles_lose_control_characters_and_are_capped() {
        assert_eq!(clean_title("Sheet\n1\t(A)"), "Sheet1(A)");
        assert_eq!(clean_title(&"t".repeat(500)).len(), 200);
    }

    #[test]
    fn windows_commands_rasterise_first_and_print_to_last() {
        let cmds = windows_print_commands("Plotter 24in", "C:\\Temp\\p.pdf", "Plan A");
        assert_eq!(cmds.len(), 2);
        for c in &cmds {
            assert_eq!(c.program, "powershell");
            assert_eq!(c.args[..5], ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
            assert_eq!(c.args.len(), 6);
            // Names only in the environment, never in the command text.
            assert!(!c.args[5].contains("Plotter 24in") && !c.args[5].contains("p.pdf") && !c.args[5].contains("Plan A"));
            assert_eq!(c.env[0], ("CADADDY_PRINTER".to_string(), "Plotter 24in".to_string()));
            assert_eq!(c.env[1], ("CADADDY_FILE".to_string(), "C:\\Temp\\p.pdf".to_string()));
            assert_eq!(c.env[2], ("CADADDY_TITLE".to_string(), "Plan A".to_string()));
        }
        assert!(cmds[0].args[5].contains("PdfDocument") && cmds[0].args[5].contains("PrintDocument"));
        assert!(!cmds[0].args[5].contains("PrintTo"));
        assert!(cmds[1].args[5].contains("-Verb PrintTo"));
        // "Print to" never waits on the application.
        assert!(!cmds[1].args[5].contains("-Wait"));
    }

    #[test]
    fn cups_command_passes_the_name_as_one_argument() {
        let c = cups_print_command("Plotter 24in", "/tmp/p.pdf", "Plan A");
        assert_eq!(
            c,
            Cmd { program: "lp".into(), args: ["-d", "Plotter 24in", "-t", "Plan A", "/tmp/p.pdf"].map(String::from).to_vec(), env: vec![] }
        );
        assert_eq!(cups_print_command("P", "/tmp/p.pdf", "").args[3], "CADaddy plot");
    }

    #[test]
    fn failures_say_why() {
        assert_eq!(failure_text("lp", Some(1), " lp: Unknown printer\n"), "lp failed (exit code 1): lp: Unknown printer");
        assert_eq!(failure_text("lp", Some(1), ""), "lp failed (exit code 1, no message)");
        assert_eq!(failure_text("lp", None, "x"), "lp failed (stopped): x");
        assert_eq!(failure_text("lp", Some(2), &"e".repeat(2000)).len(), "lp failed (exit code 2): ".len() + 600);
    }

    #[cfg(unix)]
    #[test]
    fn a_hanging_command_is_stopped_and_a_failing_one_reports_stderr() {
        let sleeper = cmd("sleep", &["30"]);
        let t = Instant::now();
        let r = run(&sleeper, Duration::from_millis(300)).unwrap();
        assert!(r.timed_out && !r.ok && r.code.is_none());
        assert!(t.elapsed() < Duration::from_secs(10));
        let failing = cmd("sh", &["-c", "echo reason-text 1>&2; exit 3"]);
        let r = run(&failing, Duration::from_secs(10)).unwrap();
        assert!(!r.ok && !r.timed_out);
        assert_eq!(failure_text("job", r.code, &r.stderr), "job failed (exit code 3): reason-text");
        assert!(run(&cmd("cadaddy-no-such-program", &[]), Duration::from_secs(1)).is_err());
    }
}
