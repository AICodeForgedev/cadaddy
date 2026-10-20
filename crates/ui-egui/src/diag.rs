//! Telemetry-free diagnostics: Help ▸ Report a Problem collects what a bug report needs into a
//! ZIP file the user saves and can attach to an email. Nothing is sent anywhere.
//!
//! The bundle holds the log files (the current run's and the two before it), the last commands,
//! the versions, the operating system and the graphics adapter, and on request a summary of the
//! open drawings (counts only: objects by type, layers, blocks…, no geometry) and the drawing
//! itself (only when the user ticks it). Names (file paths, layer, block, style and layout names,
//! what was typed after a command) are left out unless the user asks for them: they are replaced
//! by placeholders wherever they appear, including in the logs. Who the user is — the home
//! folder (either slash, any case), the account name and the computer's name — is always
//! replaced, names or not. The dialog lists the report's files before it is saved. See
//! PRIVACY.md.
//!
//! The watchdog notices the UI thread staying inside one frame for more than [`STALL`] and
//! writes to the log what it was doing (the trail of activities since the frame began: the
//! phase, the command running), and again when it responds. The host starts it with
//! [`start_watchdog`]; the app marks frames with [`busy`] and activities with [`note`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use cadaddy_doc::Drawing;

use crate::CadApp;

/// How long one frame may take before the watchdog logs a stall.
pub const STALL: Duration = Duration::from_secs(5);
/// Commands listed in the bundle.
pub const MAX_COMMANDS: usize = 50;
/// The most of each log file the bundle takes (its end).
pub const MAX_LOG_BYTES: usize = 2 << 20;
/// Log file names in the log directory, newest first (the app's logger rotates them).
pub const LOG_FILES: [&str; 3] = ["cadaddy.log", "cadaddy.1.log", "cadaddy.2.log"];
/// Activities kept per frame.
const MAX_TRAIL: usize = 16;

/// What the host tells the bundle about itself.
#[derive(Clone, Debug, Default)]
pub struct HostInfo {
    /// The app's version line (with its build).
    pub version: String,
    /// The graphics adapter: name, backend, driver.
    pub gpu: String,
    /// Where the log files are.
    pub log_dir: Option<PathBuf>,
}

static HOST: Mutex<Option<HostInfo>> = Mutex::new(None);

/// Record what the host knows (called once the log file and the graphics adapter are known; a
/// later call replaces the earlier one).
pub fn set_host(info: HostInfo) {
    *HOST.lock().unwrap_or_else(PoisonError::into_inner) = Some(info);
}

fn host() -> HostInfo {
    HOST.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_default()
}

/// The version with its build as About shows it: "0.7.8 (e760908)" for a release build, the bare
/// version for a development build or before the host has said.
pub fn version_with_build() -> String {
    with_build(env!("CARGO_PKG_VERSION"), &host().version)
}

/// `version` and the build from the host's version line ("cadaddy 0.7.8 (e760908)").
fn with_build(version: &str, line: &str) -> String {
    let build = line.rsplit_once('(').and_then(|(_, b)| b.strip_suffix(')')).filter(|b| !b.is_empty() && *b != "dev" && b.len() <= 40);
    match build {
        Some(b) if b.chars().all(|c| c.is_ascii_hexdigit()) => format!("{version} ({b})"),
        _ => version.to_string(),
    }
}

/// What the user chose to include.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    /// A summary of the open drawings (counts only).
    pub drawing: bool,
    /// Names: file paths, layer, block, style and layout names, typed input.
    pub names: bool,
    /// The active drawing itself, as a DXF file.
    pub file: bool,
}

/// Who the user is, as it can show up in a log or a path: always replaced in a report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    /// Home folders (`HOME`, `USERPROFILE`).
    pub homes: Vec<String>,
    /// The account name.
    pub user: Option<String>,
    /// The computer's name (often the person's: "Sams-MacBook-Pro").
    pub host: Option<String>,
}

impl Identity {
    /// This computer's.
    pub fn current() -> Identity {
        Identity { homes: home_dirs(), user: user_name(), host: host_name() }
    }
}

/// Replaces private text with placeholders, longest first so a path goes before its file name;
/// then the account and computer names wherever they stand as a word, however short.
struct Redact {
    pairs: Vec<(String, &'static str)>,
    words: Vec<(String, &'static str)>,
}

/// Table names every drawing has: not private, and replacing them would garble the log.
const COMMON: &[&str] = &["standard", "continuous", "bylayer", "byblock", "defpoints", "model", "annotative", "iso-25", "layout1", "layout2"];

impl Redact {
    fn new(app: &CadApp, names: bool, who: &Identity) -> Redact {
        let mut pairs: Vec<(String, &'static str)> = Vec::new();
        let mut add = |s: &str, with: &'static str| {
            let s = s.trim();
            if s.chars().count() >= 3 && !COMMON.contains(&s.to_ascii_lowercase().as_str()) && !pairs.iter().any(|(p, _)| p == s) {
                pairs.push((s.to_string(), with));
            }
        };
        // Who the user is, always (with names, the paths and names are what they agreed to show).
        for home in &who.homes {
            for v in path_forms(home) {
                add(&v, "~");
            }
        }
        if !names {
            let mut paths: Vec<String> = app.session.docs.iter().filter_map(|d| d.path.clone()).collect();
            paths.extend(app.ui.recent_files.iter().cloned());
            for p in &paths {
                add(p, "<path>");
                let path = Path::new(p);
                if let Some(n) = path.file_name() {
                    add(&n.to_string_lossy(), "<file>");
                }
                if let Some(n) = path.file_stem() {
                    add(&n.to_string_lossy(), "<file>");
                }
            }
            for d in &app.session.docs {
                add(&d.title, "<file>");
                if let Some(n) = Path::new(&d.title).file_stem() {
                    add(&n.to_string_lossy(), "<file>");
                }
                for n in drawing_names(&d.doc) {
                    add(&n, "<name>");
                }
            }
        }
        pairs.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        let mut words: Vec<(String, &'static str)> = Vec::new();
        if let Some(h) = who.host.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
            // "sams-macbook-pro.local" and "sams-macbook-pro".
            words.push((h.to_string(), "<computer>"));
            if let Some(short) = h.split('.').next().filter(|s| *s != h && !s.is_empty()) {
                words.push((short.to_string(), "<computer>"));
            }
        }
        if let Some(u) = who.user.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            words.push((u.to_string(), "<user>"));
        }
        words.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
        Redact { pairs, words }
    }

    fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (p, with) in &self.pairs {
            if out.contains(p.as_str()) {
                out = out.replace(p.as_str(), with);
            }
        }
        for (w, with) in &self.words {
            out = replace_word(&out, w, with);
        }
        out
    }
}

/// A home folder as it may be written: as given, with the other slash, and (Windows paths are
/// case-blind) in lower and upper case.
fn path_forms(home: &str) -> Vec<String> {
    let mut v = vec![home.to_string(), home.replace('\\', "/"), home.replace('/', "\\")];
    if home.contains('\\') || home.get(1..2) == Some(":") {
        let more: Vec<String> = v.iter().flat_map(|p| [p.to_lowercase(), p.to_uppercase()]).collect();
        v.extend(more);
    }
    v.sort();
    v.dedup();
    v
}

/// `word` replaced wherever it stands alone (not inside a longer word or name), ignoring case.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let w = word.to_lowercase();
    // Lower-casing can change byte lengths (rare letters): then match as written only.
    let (hay, needle) = if lower.len() == text.len() && w.len() == word.len() { (lower.as_str(), w.as_str()) } else { (text, word) };
    if needle.is_empty() {
        return text.to_string();
    }
    let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = hay.get(from..).and_then(|h| h.find(needle)).map(|i| i + from) {
        let end = i + needle.len();
        let before = text.get(..i).and_then(|t| t.chars().next_back());
        let after = text.get(end..).and_then(|t| t.chars().next());
        if !part(before) && !part(after) {
            out.push_str(text.get(last..i).unwrap_or_default());
            out.push_str(with);
            last = end;
        }
        from = end;
    }
    out.push_str(text.get(last..).unwrap_or_default());
    out
}

fn home_dirs() -> Vec<String> {
    ["HOME", "USERPROFILE"].iter().filter_map(|k| std::env::var(k).ok()).filter(|h| h.len() > 1).collect()
}

fn user_name() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"].iter().find_map(|k| std::env::var(k).ok()).filter(|u| !u.trim().is_empty())
}

/// The computer's name: `COMPUTERNAME`/`HOSTNAME`, else what `hostname` prints.
fn host_name() -> Option<String> {
    let env = ["COMPUTERNAME", "HOSTNAME"].iter().find_map(|k| std::env::var(k).ok());
    #[cfg(not(target_arch = "wasm32"))]
    let env = env.or_else(|| {
        let out = std::process::Command::new("hostname").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    env.filter(|h| !h.trim().is_empty())
}

/// The names a drawing holds: its tables, blocks, layouts, views, groups.
fn drawing_names(d: &Drawing) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    v.extend(d.layers.iter().map(|x| x.name.clone()));
    v.extend(d.linetypes.iter().map(|x| x.name.clone()));
    v.extend(d.text_styles.iter().map(|x| x.name.clone()));
    v.extend(d.dim_styles.iter().map(|x| x.name.clone()));
    v.extend(d.blocks.keys().cloned());
    v.extend(d.blocks.values().filter_map(|b| b.xref_path.clone()));
    v.extend(d.layouts.iter().map(|x| x.name.clone()));
    v.extend(d.views.iter().map(|x| x.name.clone()));
    v.extend(d.groups.iter().map(|x| x.name.clone()));
    v.extend(d.layer_states.iter().map(|x| x.name.clone()));
    v
}

/// Objects by type name.
fn counts(d: &Drawing) -> (BTreeMap<&'static str, usize>, usize, usize, usize) {
    let mut by: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut model, mut paper, mut in_blocks) = (0, 0, 0);
    for e in d.model.iter() {
        *by.entry(e.kind.type_name()).or_default() += 1;
        model += 1;
    }
    for l in &d.layouts {
        for e in l.entities.iter() {
            *by.entry(e.kind.type_name()).or_default() += 1;
            paper += 1;
        }
    }
    for b in d.blocks.values() {
        in_blocks += b.entities.len();
    }
    (by, model, paper, in_blocks)
}

fn drawing_summary(app: &CadApp, names: bool) -> String {
    let mut s = String::from("Open drawings (counts only; no geometry)\n");
    for (i, doc) in app.session.docs.iter().enumerate() {
        let d = &doc.doc;
        let (by, model, paper, in_blocks) = counts(d);
        s.push_str(&format!("\nDrawing {}\n", i + 1));
        if names {
            s.push_str(&format!("  file: {}\n", doc.path.as_deref().unwrap_or(&doc.title)));
        }
        let xrefs = d.blocks.values().filter(|b| b.xref_path.is_some()).count();
        s.push_str(&format!("  objects: {model} in model space, {paper} on layouts, {in_blocks} in block definitions\n"));
        s.push_str(&format!(
            "  layers {}, linetypes {}, text styles {}, dimension styles {}, blocks {} ({xrefs} xrefs), layouts {}, groups {}, constraints {}\n",
            d.layers.len(),
            d.linetypes.len(),
            d.text_styles.len(),
            d.dim_styles.len(),
            d.blocks.len(),
            d.layouts.len(),
            d.groups.len(),
            d.constraints.len()
        ));
        for (k, n) in &by {
            s.push_str(&format!("  {k}: {n}\n"));
        }
        if names {
            s.push_str(&format!("  layer names: {}\n", d.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")));
            s.push_str(&format!("  block names: {}\n", d.blocks.keys().map(String::as_str).collect::<Vec<_>>().join(", ")));
            s.push_str(&format!("  layouts: {}\n", d.layouts.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
    }
    s
}

/// The last commands: without names only each line's first word (the command), since what
/// follows can be text, a file or a layer.
fn commands(app: &CadApp, names: bool) -> String {
    let start = app.cmd.history.len().saturating_sub(MAX_COMMANDS);
    let mut s = String::from("Last commands, oldest first\n");
    for line in &app.cmd.history[start..] {
        let line = if names { line.trim() } else { line.split_whitespace().next().unwrap_or_default() };
        s.push_str(line);
        s.push('\n');
    }
    s
}

fn system(app: &CadApp, host: &HostInfo) -> String {
    let version = if host.version.is_empty() { format!("cadaddy {}", env!("CARGO_PKG_VERSION")) } else { host.version.clone() };
    let gpu = if host.gpu.is_empty() { "unknown" } else { host.gpu.as_str() };
    format!(
        "{version}\noperating system: {} {} ({})\ngraphics: {gpu}\ninterface language: {}\ninterface size: {:.0}%\ntheme: {:?}\nopen drawings: {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY,
        app.language(),
        app.ui.ui_scale * 100.0,
        app.ui.theme,
        app.session.docs.len(),
    )
}

/// The end of a log file (at most [`MAX_LOG_BYTES`], from a line start).
fn log_tail(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let start = bytes.len().saturating_sub(MAX_LOG_BYTES);
    let tail = &bytes[start..];
    let tail = if start > 0 { tail.iter().position(|b| *b == b'\n').map_or(tail, |i| &tail[i + 1..]) } else { tail };
    Some(String::from_utf8_lossy(tail).into_owned())
}

/// The files of the bundle, by name.
pub fn entries(app: &CadApp, choices: Choices) -> Vec<(String, Vec<u8>)> {
    entries_for(app, choices, &Identity::current())
}

/// The bundle's files with who the user is given (the tests' made-up identity).
pub fn entries_for(app: &CadApp, choices: Choices, who: &Identity) -> Vec<(String, Vec<u8>)> {
    let host = host();
    let redact = Redact::new(app, choices.names, who);
    let mut out: Vec<(String, String)> = Vec::new();
    let made = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    out.push((
        "README.txt".into(),
        format!(
            "CADaddy problem report, made on this computer (Unix time {made}). Nothing was sent anywhere.\n\
             To report the problem, attach this file to an email to {} and say what happened.\n\n\
             system.txt   versions, operating system, graphics adapter\n\
             commands.txt the last commands{}\n\
             logs/        the log of this run and the two before it\n{}{}{}",
            crate::support::FEEDBACK_EMAIL,
            if choices.names { " with what was typed" } else { " (command names only)" },
            if choices.drawing { "drawings.txt what the open drawings hold (counts only)\n" } else { "" },
            if choices.file { "drawing.dxf  the active drawing, as the user chose to include it\n" } else { "" },
            if choices.names {
                "\nNames (files, layers, blocks…) are included.\n"
            } else {
                "\nFile paths and names from drawings are replaced by <path>, <file> and <name>.\n"
            },
        ),
    ));
    out.push(("system.txt".into(), system(app, &host)));
    out.push(("commands.txt".into(), commands(app, choices.names)));
    if choices.drawing {
        out.push(("drawings.txt".into(), drawing_summary(app, choices.names)));
    }
    if let Some(dir) = &host.log_dir {
        for name in LOG_FILES {
            if let Some(text) = log_tail(&dir.join(name)) {
                out.push((format!("logs/{name}"), text));
            }
        }
    }
    // What CADaddy wrote itself without names (versions, counts by type) is left as it is: a
    // layer called "Line" mustn't turn the line count into a placeholder. Who the user is goes
    // from every file.
    let own = |n: &str| !choices.names && matches!(n, "README.txt" | "system.txt" | "drawings.txt");
    let mut files: Vec<(String, Vec<u8>)> = out
        .into_iter()
        .map(|(n, t)| {
            let t = if own(&n) { Redact { pairs: Vec::new(), words: redact.words.clone() }.apply(&t) } else { redact.apply(&t) };
            (n, t.into_bytes())
        })
        .collect();
    // The drawing itself, only when ticked: as it is (its contents are what the user chose to send).
    if choices.file {
        let dxf = app.session.doc().ok().map(|d| match cadaddy_engine::cmd::file::io() {
            Some(io) => (io.write)(d, "drawing.dxf").map_err(|e| e.to_string()),
            None => Err("no file formats in this build".to_string()),
        });
        match dxf {
            Some(Ok(bytes)) => files.push(("drawing.dxf".into(), bytes)),
            Some(Err(e)) => files.push(("drawing.txt".into(), format!("The drawing couldn't be written: {e}\n").into_bytes())),
            None => {}
        }
    }
    files
}

/// The report's files and their sizes, as the dialog lists them before saving.
pub fn file_list(app: &CadApp, choices: Choices) -> Vec<(String, usize)> {
    entries(app, choices).into_iter().map(|(n, b)| (n, b.len())).collect()
}

/// The bundle as a ZIP file.
pub fn bundle(app: &CadApp, choices: Choices) -> Result<Vec<u8>, String> {
    cadaddy_engine::cmd::file::zip::write(&entries(app, choices))
}

fn human_size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// The UI thread's frames as the watchdog sees them.
#[derive(Debug, Default)]
pub struct Watchdog {
    state: Mutex<Frame>,
}

#[derive(Debug, Default)]
struct Frame {
    /// When the current frame began (milliseconds on the watchdog's clock); `None` between frames.
    since: Option<u64>,
    /// What the frame has been doing, oldest first.
    trail: Vec<String>,
    /// The stall of this frame was logged.
    reported: bool,
    /// Nesting of [`Watchdog::begin`] (a phase inside a phase).
    depth: u32,
}

impl Watchdog {
    pub fn begin(&self, now_ms: u64, what: &str) {
        let mut f = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if f.depth == 0 {
            f.since = Some(now_ms);
            f.trail.clear();
            f.reported = false;
        }
        f.depth += 1;
        push(&mut f.trail, what);
    }

    /// The frame ended; the time it stalled for when that was logged.
    pub fn end(&self, now_ms: u64) -> Option<u64> {
        let mut f = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f.depth = f.depth.saturating_sub(1);
        if f.depth > 0 {
            return None;
        }
        let since = f.since.take()?;
        f.reported.then(|| now_ms.saturating_sub(since))
    }

    pub fn note(&self, what: &str) {
        let mut f = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if f.since.is_some() {
            push(&mut f.trail, what);
        }
    }

    /// A stall to log: the frame has run longer than `limit` and wasn't reported yet.
    pub fn check(&self, now_ms: u64, limit: Duration) -> Option<String> {
        let mut f = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let since = f.since?;
        let busy = now_ms.saturating_sub(since);
        if f.reported || u128::from(busy) < limit.as_millis() {
            return None;
        }
        f.reported = true;
        Some(format!("UI thread not responding for {:.1} s; what it was doing, latest last: {}", busy as f64 / 1000.0, f.trail.join(" > ")))
    }
}

fn push(trail: &mut Vec<String>, what: &str) {
    if trail.len() >= MAX_TRAIL {
        trail.remove(1);
    }
    trail.push(what.to_string());
}

fn watchdog() -> &'static Watchdog {
    static W: OnceLock<Watchdog> = OnceLock::new();
    W.get_or_init(Watchdog::default)
}

/// Milliseconds since the first call.
fn now_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static T0: OnceLock<std::time::Instant> = OnceLock::new();
        u64::try_from(T0.get_or_init(std::time::Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Marks the UI thread busy with `what` until dropped.
pub struct Busy(());

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(ms) = watchdog().end(now_ms()) {
            log::warn!("UI thread responding again after {:.1} s", ms as f64 / 1000.0);
        }
    }
}

/// The UI thread works on `what` (a frame phase) until the returned guard drops.
pub fn busy(what: &str) -> Busy {
    watchdog().begin(now_ms(), what);
    Busy(())
}

/// Record what the UI thread is doing inside the current frame (a command running).
pub fn note(what: &str) {
    watchdog().note(what);
}

/// Start the watchdog thread (once; later calls do nothing). Not on the web, which has no threads.
pub fn start_watchdog() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static STARTED: OnceLock<()> = OnceLock::new();
        if STARTED.set(()).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("cadaddy-watchdog".into()).spawn(|| {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(msg) = watchdog().check(now_ms(), STALL) {
                    log::warn!("{msg}");
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("no UI watchdog: {e}");
        }
    }
}

#[derive(Clone, Default)]
struct DialogState {
    choices: Choices,
    /// The report's files for `listed` (worked out again when the choices change).
    files: Vec<(String, usize)>,
    listed: Option<Choices>,
}

fn dialog_id() -> egui::Id {
    egui::Id::new("cadaddy-diag-dialog")
}

/// Help ▸ Report a Problem: what goes in, then Save.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: DialogState = ctx.data(|d| d.get_temp(dialog_id())).unwrap_or_default();
    let mut save = false;
    egui::Window::new(crate::tl!("Report a Problem")).id(egui::Id::new("Report a Problem")).default_pos(crate::palette_pos(app)).open(open).resizable(false).default_width(460.0).show(ctx, |ui| {
        ui.label(crate::tl!("Collects what helps find a problem into a ZIP file on this computer. Nothing is sent: attach the file to an email if you want to report it."));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::tl!("Always included")).strong());
        for l in ["The log of this run and the two before it", "The last commands", "Versions, operating system and graphics adapter"] {
            ui.label(format!("• {}", crate::i18n::t(l)));
        }
        ui.add_space(6.0);
        ui.checkbox(&mut st.choices.drawing, crate::tl!("A summary of the open drawings (counts only, no geometry)"));
        ui.checkbox(&mut st.choices.names, crate::tl!("Names: file paths, layer and block names, typed text"));
        if !st.choices.names {
            ui.label(egui::RichText::new(crate::tl!("Names are replaced by placeholders, also in the log.")).small().color(t.text_dim));
        }
        ui.checkbox(&mut st.choices.file, crate::tl!("Include the drawing (the active drawing as a DXF file)"));
        ui.label(
            egui::RichText::new(crate::tl!("Your account, home folder and computer name are always left out.")).small().color(t.text_dim),
        );
        ui.add_space(6.0);
        if st.listed != Some(st.choices) {
            st.files = file_list(app, st.choices);
            st.listed = Some(st.choices);
        }
        ui.label(egui::RichText::new(crate::tl!("Files in the report")).strong());
        for (name, size) in &st.files {
            ui.label(egui::RichText::new(format!("{name}  ({})", human_size(*size))).monospace().small());
        }
        ui.add_space(6.0);
        save = ui.button(crate::tl!("Save Report...")).clicked();
        ui.label(egui::RichText::new(crate::tf!("Email it to {address}", address = crate::support::FEEDBACK_EMAIL)).small().color(t.text_dim));
    });
    if save {
        match app.services.pick_save.as_ref().map(|pick| pick("cadaddy-problem-report.zip")) {
            Some(Some(path)) => match bundle(app, st.choices).and_then(|zip| std::fs::write(&path, zip).map_err(|e| e.to_string())) {
                Ok(()) => {
                    app.session.echo(format!("Problem report saved to {path}."));
                    *open = false;
                }
                Err(e) => app.session.echo(format!("Problem report: {e}")),
            },
            Some(None) => {}
            None => app.session.echo("There is no file dialog here; the problem report can't be saved."),
        }
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(dialog_id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<DialogState>(dialog_id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_shows_the_release_build() {
        assert_eq!(with_build("0.7.8", "cadaddy 0.7.8 (e760908)"), "0.7.8 (e760908)");
        assert_eq!(with_build("0.7.8", "cadaddy 0.7.8 (dev)"), "0.7.8");
        assert_eq!(with_build("0.7.8", ""), "0.7.8");
        assert_eq!(with_build("0.7.8", "cadaddy 0.7.8 (<b>x</b>)"), "0.7.8");
    }
    use serde_json::json;

    fn text(entries: &[(String, Vec<u8>)]) -> String {
        entries.iter().map(|(n, b)| format!("== {n}\n{}\n", String::from_utf8_lossy(b))).collect()
    }

    fn app_with_secrets(log_dir: &Path) -> CadApp {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("line", json!({ "points": [[0, 0], [1, 1]] })).unwrap();
        {
            let doc = app.session.doc_mut().unwrap();
            doc.layers.push(cadaddy_doc::Layer::new("SECRETLAYER"));
            // A name that is also a type name: the counts stay readable.
            doc.layers.push(cadaddy_doc::Layer::new("Line"));
            doc.blocks.insert("SECRETBLOCK".into(), std::sync::Arc::new(cadaddy_doc::Block::new("SECRETBLOCK")));
        }
        app.session.docs[0].path = Some("/jobs/SECRETJOB/SECRETFILE.dxf".into());
        app.session.docs[0].title = "SECRETFILE.dxf".into();
        app.ui.recent_files = vec!["/jobs/SECRETJOB/OTHERSECRET.dwg".into()];
        app.cmd.history = vec!["line".into(), "-layer make SECRETLAYER".into(), "text SECRETTEXT".into()];
        std::fs::create_dir_all(log_dir).unwrap();
        std::fs::write(
            log_dir.join("cadaddy.log"),
            "INFO opened /jobs/SECRETJOB/SECRETFILE.dxf\nWARN layer SECRETLAYER has no linetype\nINFO OTHERSECRET.dwg recent\n",
        )
        .unwrap();
        set_host(HostInfo { version: "cadaddy test".into(), gpu: "Test GPU (Metal)".into(), log_dir: Some(log_dir.to_path_buf()) });
        app
    }

    // One test, since the host info is process-wide.
    #[test]
    fn the_bundle_holds_no_names_unless_asked() {
        let dir = std::env::temp_dir().join(format!("cadaddy-diag-{}", std::process::id()));
        let app = app_with_secrets(&dir.join("logs"));

        let plain = entries(&app, Choices { drawing: true, names: false, file: false });
        let all = text(&plain);
        assert!(!all.contains("SECRET"), "{all}");
        let files: Vec<&str> = plain.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(files, ["README.txt", "system.txt", "commands.txt", "drawings.txt", "logs/cadaddy.log"]);
        assert!(all.contains("Test GPU (Metal)") && all.contains("cadaddy test"));
        assert!(all.contains("Line: 1"), "counts are there: {all}");
        assert!(all.contains("opened <path>") && all.contains("layer <name> has"), "{all}");
        assert!(all.contains("\n-layer\n") && all.contains("\ntext\n"), "command names only: {all}");

        let named = text(&entries(&app, Choices { drawing: true, names: true, file: false }));
        assert!(
            named.contains("SECRETLAYER")
                && named.contains("SECRETBLOCK")
                && named.contains("/jobs/SECRETJOB/SECRETFILE.dxf")
                && named.contains("SECRETTEXT")
        );

        // No drawing summary unless asked; the ZIP reads back.
        let zip = bundle(&app, Choices::default()).unwrap();
        let read = cadaddy_engine::cmd::file::zip::read(&zip).unwrap();
        assert!(!read.iter().any(|(n, _)| n == "drawings.txt" || n == "drawing.dxf"));
        assert!(!text(&read).contains("SECRET"));

        // The drawing itself only when ticked; the dialog's list is the report's files.
        cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
            read: |_, _| Err("not in this test".into()),
            write: |_, _| Ok(b"0\nSECTION\n".to_vec()),
            plot: None,
            read_pdf: None,
        });
        let with = Choices { file: true, ..Choices::default() };
        let listed: Vec<String> = file_list(&app, with).into_iter().map(|(n, _)| n).collect();
        assert!(listed.contains(&"drawing.dxf".to_string()), "{listed:?}");
        assert_eq!(listed, entries(&app, with).into_iter().map(|(n, _)| n).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&dir);

        // Made in a home folder, by a user with a short name, on a computer named after them: none
        // of it in the report, names or not; words that merely contain the name stay.
        let dir = std::env::temp_dir().join(format!("cadaddy-diag-home-{}", std::process::id()));
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("line", json!({ "points": [[0, 0], [1, 1]] })).unwrap();
        app.session.docs[0].path = Some("/Users/jo/Projects/Plan.dxf".into());
        app.session.docs[0].title = "Plan.dxf".into();
        app.ui.recent_files = vec!["C:\\Users\\Jo\\Desktop\\Old.dwg".into()];
        app.cmd.history = vec!["line".into(), "open /Users/jo/Projects/Plan.dxf".into()];
        std::fs::write(
            logs.join("cadaddy.log"),
            "INFO opened /Users/jo/Projects/Plan.dxf on Jos-MBP.local\nINFO started by jo\nWARN settings c:/users/jo/AppData/x.json\nINFO Jos-MBP ready; total 3 jobs, joined\n",
        )
        .unwrap();
        set_host(HostInfo { version: "cadaddy test".into(), gpu: "Test GPU".into(), log_dir: Some(logs) });
        let who = Identity { homes: vec!["/Users/jo".into(), "C:\\Users\\Jo".into()], user: Some("jo".into()), host: Some("Jos-MBP.local".into()) };
        for names in [false, true] {
            let all = text(&entries_for(&app, Choices { drawing: true, names, file: false }, &who));
            let lower = all.to_lowercase();
            assert!(!lower.contains("users/jo") && !lower.contains("users\\jo"), "names {names}: {all}");
            assert!(!lower.contains("jos-mbp"), "the computer's name, names {names}: {all}");
            assert!(!all.contains(" jo\n") && !all.contains("by jo"), "the account, names {names}: {all}");
            assert!(all.contains("<user>") && all.contains("<computer>") && all.contains("~/"), "names {names}: {all}");
            assert!(all.contains("total 3 jobs, joined"), "words containing the name stay: {all}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whole_words_and_path_forms() {
        assert_eq!(replace_word("jo, Jo. jobs ajo jo_x jo", "jo", "<user>"), "<user>, <user>. jobs ajo jo_x <user>");
        assert_eq!(replace_word("", "jo", "<user>"), "");
        let forms = path_forms("C:\\Users\\Sam");
        for f in ["C:\\Users\\Sam", "C:/Users/Sam", "c:/users/sam", "c:\\users\\sam"] {
            assert!(forms.contains(&f.to_string()), "{f}: {forms:?}");
        }
        assert!(path_forms("/home/sam").contains(&"\\home\\sam".to_string()));
    }

    #[test]
    fn help_opens_the_dialog() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        crate::menus::run_ui_command(&mut app, "ui.diag", &serde_json::Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some("diag"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert_eq!(app.ui.dialog.as_deref(), Some("diag"));
    }

    #[test]
    fn home_and_account_are_always_hidden() {
        let r = Redact { pairs: vec![("/home/someone".into(), "~")], words: vec![("someone".into(), "<user>")] };
        assert_eq!(r.apply("log /home/someone/x.dxf by someone"), "log ~/x.dxf by <user>");
    }

    #[test]
    fn a_long_frame_is_reported_once_with_what_it_was_doing() {
        let w = Watchdog::default();
        assert_eq!(w.check(10_000, STALL), None, "between frames: idle, not stalled");
        w.begin(1_000, "logic");
        w.note("command hatch");
        assert_eq!(w.check(5_000, STALL), None);
        let msg = w.check(6_500, STALL).unwrap();
        assert!(msg.contains("5.5 s") && msg.contains("logic > command hatch"), "{msg}");
        assert_eq!(w.check(9_000, STALL), None, "once per stall");
        assert_eq!(w.end(9_500), Some(8_500));
        // A quick frame: nothing.
        w.begin(10_000, "draw");
        assert_eq!(w.check(11_000, STALL), None);
        assert_eq!(w.end(11_000), None);
        // Nested phases end with the outer one; the trail is capped.
        w.begin(20_000, "logic");
        w.begin(20_001, "draw");
        for i in 0..100 {
            w.note(&format!("step {i}"));
        }
        assert_eq!(w.end(20_002), None);
        let msg = w.check(30_000, STALL).unwrap();
        assert!(msg.starts_with("UI thread not responding for 10.0 s; what it was doing, latest last: logic > "), "{msg}");
        assert!(msg.ends_with("step 99") && !msg.contains("step 50 "), "{msg}");
        assert!(w.end(30_001).is_some());
    }
}
