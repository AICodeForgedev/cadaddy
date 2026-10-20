//! DWGCONVERT (CDYCONVERT): drawings converted in a batch between .cdy, DWG (2000–2018) and DXF,
//! each read and written on its own (nothing is opened in the app), into an output folder (with
//! the input folder's subfolders) or next to the originals. A file that fails is reported and the
//! rest go on; what DWG/DXF didn't keep of each is listed (the exchange notes). The Convert
//! Drawings window runs [`run`] off the UI thread with a cancel flag and a progress callback;
//! `cadaddy-cli convert` uses the same [`run`] with its own converter for the image formats.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::{Value, json};

use super::kept::{Loss, check_written};
use super::{SaveAs, Version, bad, io, read_loss, store_keeping, write_drawing};
use crate::cmd::{CommandSpec, always, bool_or, str_param};
use crate::{EngineError, Result, Session};

/// Extensions of the drawings a conversion picks up (matched without regard to case).
pub const DRAWINGS: [&str; 3] = ["dwg", "dxf", "cdy"];
/// Formats the DWGCONVERT command writes.
pub const TARGETS: [&str; 3] = ["cdy", "dwg", "dxf"];
/// Most files one conversion takes.
pub const MAX_FILES: usize = 100_000;
/// Deepest subfolder a folder conversion goes into.
const MAX_DEPTH: usize = 32;
/// Most "name (n)" tries for a free output name.
const MAX_RENAME: usize = 1_000;

/// What to do when the output file is already there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Exists {
    /// Write over it (keeping it as .bak when [`Job::bak`]).
    Overwrite,
    /// Leave it and skip that drawing.
    #[default]
    Skip,
    /// Write under the next free name: "plan (2).dwg".
    Rename,
}

impl Exists {
    pub fn parse(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(true) => Some(Self::Overwrite),
            Value::Bool(false) => Some(Self::Skip),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "overwrite" | "replace" | "yes" => Some(Self::Overwrite),
                "skip" | "no" => Some(Self::Skip),
                "rename" => Some(Self::Rename),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Overwrite => "overwrite",
            Self::Skip => "skip",
            Self::Rename => "rename",
        }
    }
}

/// One drawing to convert: its file and, from a folder, the subfolder it is in (kept in the
/// output folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub path: PathBuf,
    pub sub: PathBuf,
}

/// A conversion: the drawings, the format to write and where.
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub inputs: Vec<Input>,
    /// The output format's extension, lower case, no dot.
    pub to: String,
    /// DWG/DXF version (`None`: each drawing's own).
    pub version: Option<Version>,
    /// Output folder (`None`: next to each original).
    pub out: Option<PathBuf>,
    pub exists: Exists,
    /// Keep a file written over as .bak; also what allows converting a drawing over itself
    /// (to another DWG version, say): the original is then the .bak.
    pub bak: bool,
}

/// What converting one drawing found.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    /// What reading the original left out (a damaged DWG's records).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// What the DWG/DXF file didn't keep.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub losses: Vec<Loss>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Converted,
    Skipped,
    Failed,
}

/// One drawing's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub from: String,
    /// The file written (or that would have been).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub status: Status,
    /// Why it was skipped or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(flatten)]
    pub report: Report,
}

/// Converts one drawing `from` → `to` for the job: [`convert_drawing`], or the CLI's for images.
pub type Converter<'a> = &'a dyn Fn(&Path, &Path, &Job) -> std::result::Result<Report, String>;

/// A whole conversion's outcomes, in input order; `cancelled` when it stopped early.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub outcomes: Vec<Outcome>,
    pub cancelled: bool,
    /// The drawings that were to be converted.
    pub total: usize,
}

impl Summary {
    pub fn count(&self, s: Status) -> usize {
        self.outcomes.iter().filter(|o| o.status == s).count()
    }

    /// `{converted: [{from, to, losses?, notes?}], skipped: [{file, to?, reason}], failed: [{file,
    /// error}], cancelled, total}` (`cadaddy-cli convert`'s report too).
    pub fn to_json(&self) -> Value {
        let mut converted = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();
        for o in &self.outcomes {
            match o.status {
                Status::Converted => {
                    let mut v = json!({ "from": o.from, "to": o.to });
                    if !o.report.losses.is_empty() {
                        v["losses"] = json!(o.report.losses);
                    }
                    if !o.report.notes.is_empty() {
                        v["notes"] = json!(o.report.notes);
                    }
                    converted.push(v);
                }
                Status::Skipped => skipped.push(json!({ "file": o.from, "to": o.to, "reason": o.reason })),
                Status::Failed => failed.push(json!({ "file": o.from, "error": o.reason })),
            }
        }
        json!({ "converted": converted, "skipped": skipped, "failed": failed, "cancelled": self.cancelled, "total": self.total })
    }

    /// The log as text: a line per drawing, then the totals.
    pub fn log_text(&self) -> String {
        let mut t = String::new();
        for o in &self.outcomes {
            let to = o.to.as_deref().unwrap_or("");
            let line = match o.status {
                Status::Converted => format!("converted  {} -> {to}", o.from),
                Status::Skipped => format!("skipped    {} ({})", o.from, o.reason.as_deref().unwrap_or("")),
                Status::Failed => format!("FAILED     {}: {}", o.from, o.reason.as_deref().unwrap_or("")),
            };
            t.push_str(&line);
            t.push('\n');
            for n in &o.report.notes {
                t.push_str(&format!("    read: {n}\n"));
            }
            if !o.report.losses.is_empty() {
                t.push_str(&format!("    not kept: {}\n", super::kept::losses_line(&o.report.losses)));
            }
        }
        t.push_str(&format!(
            "\n{} converted, {} skipped, {} failed of {}{}\n",
            self.count(Status::Converted),
            self.count(Status::Skipped),
            self.count(Status::Failed),
            self.total,
            if self.cancelled { " (cancelled)" } else { "" }
        ));
        t
    }
}

fn is_drawing(p: &Path) -> bool {
    p.extension().and_then(|x| x.to_str()).is_some_and(|x| DRAWINGS.contains(&x.to_ascii_lowercase().as_str()))
}

/// The drawings in `folder` (and its subfolders when `recurse`), sorted by name, each with the
/// subfolder it is in.
pub fn folder_inputs(folder: &Path, recurse: bool) -> std::result::Result<Vec<Input>, String> {
    let mut out = Vec::new();
    let mut stack = vec![(folder.to_path_buf(), PathBuf::new(), 0usize)];
    while let Some((dir, sub, depth)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        let mut dirs = Vec::new();
        for p in paths {
            if p.is_file() && is_drawing(&p) {
                if out.len() >= MAX_FILES {
                    return Err(format!("more than {MAX_FILES} drawings: convert fewer at a time"));
                }
                out.push(Input { path: p, sub: sub.clone() });
            } else if recurse && depth < MAX_DEPTH && p.is_dir() && !p.is_symlink() {
                let name = p.file_name().map(PathBuf::from).unwrap_or_default();
                dirs.push((p, sub.join(name), depth + 1));
            }
        }
        // Subfolders after the folder's own files, in name order.
        stack.extend(dirs.into_iter().rev());
    }
    Ok(out)
}

/// The job from DWGCONVERT's parameters.
pub fn job_from(p: &Value) -> Result<Job> {
    const CMD: &str = "dwgconvert";
    let err = |m: String| EngineError::Other(format!("DWGCONVERT: {m}"));
    let to = str_param(p, "to")
        .map(|t| t.trim().trim_start_matches('.').to_ascii_lowercase())
        .ok_or_else(|| err("`to` is required: cdy, dwg or dxf".into()))?;
    if !TARGETS.contains(&to.as_str()) {
        let shown: String = to.chars().take(20).collect();
        return Err(bad(CMD, format!("`to`: unknown format `{shown}` (cdy, dwg or dxf)")));
    }
    let mut inputs = Vec::new();
    match p.get("files") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for f in a.iter().take(MAX_FILES) {
                let f = f.as_str().ok_or_else(|| bad(CMD, "`files` must be a list of file names"))?;
                inputs.push(Input { path: PathBuf::from(f), sub: PathBuf::new() });
            }
        }
        Some(Value::String(f)) => inputs.push(Input { path: PathBuf::from(f), sub: PathBuf::new() }),
        Some(_) => return Err(bad(CMD, "`files` must be a list of file names")),
    }
    if let Some(folder) = str_param(p, "folder").filter(|f| !f.trim().is_empty()) {
        let found = folder_inputs(Path::new(folder), bool_or(p, "recurse", false)).map_err(err)?;
        if found.is_empty() {
            return Err(err(format!("{folder}: no drawings (.dwg, .dxf, .cdy) in this folder")));
        }
        inputs.extend(found);
    }
    if inputs.is_empty() {
        return Err(err("nothing to convert: give `files` or a `folder`".into()));
    }
    inputs.truncate(MAX_FILES);
    let opts = super::save_options(CMD, p)?;
    if opts.version == Some(Version::R12) && to != "dxf" {
        return Err(bad(CMD, "Release 12 is a DXF version: convert to dxf"));
    }
    let exists = match p.get("overwrite") {
        None | Some(Value::Null) => Exists::default(),
        Some(v) => Exists::parse(v).ok_or_else(|| bad(CMD, "`overwrite` is true, false, \"overwrite\", \"skip\" or \"rename\""))?,
    };
    let out = str_param(p, "out").map(str::trim).filter(|o| !o.is_empty()).map(PathBuf::from);
    Ok(Job { inputs, to, version: opts.version, out, exists, bak: bool_or(p, "bak", true) })
}

/// Whether two paths are the same file (resolved when they exist; without regard to case on
/// Windows and macOS).
fn same(a: &Path, b: &Path) -> bool {
    super::same_file(&a.to_string_lossy(), &b.to_string_lossy())
}

/// A file name key that ignores case: two outputs differing only in case are one file on Windows
/// and macOS.
fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// Where `input` is written, or why it isn't.
fn target(job: &Job, input: &Input, taken: &mut HashSet<String>) -> std::result::Result<PathBuf, (Status, Option<PathBuf>, String)> {
    let src = &input.path;
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = match &job.out {
        Some(o) => o.join(&input.sub),
        None => src.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let first = dir.join(format!("{stem}.{}", job.to));
    let mut out = first.clone();
    if !taken.insert(key(&out)) {
        // plan.dwg and plan.dxf can't both become plan.cdy.
        if job.exists != Exists::Rename {
            return Err((Status::Failed, Some(out.clone()), format!("another drawing in this conversion already became {}", out.display())));
        }
    } else if same(src, &out) {
        // Over itself: only when the original is kept as .bak.
        if job.exists == Exists::Overwrite && job.bak {
            return Ok(out);
        }
        if job.exists != Exists::Rename {
            taken.remove(&key(&out));
            return Err((Status::Skipped, Some(out), "it would be written over itself (choose Overwrite with a .bak, or Rename)".into()));
        }
    } else if !out.exists() || job.exists == Exists::Overwrite {
        return Ok(out);
    } else if job.exists == Exists::Skip {
        return Err((Status::Skipped, Some(out), "the output file is already there".into()));
    }
    // Rename: the next free "name (n)".
    taken.remove(&key(&first));
    for n in 2..MAX_RENAME {
        out = dir.join(format!("{stem} ({n}).{}", job.to));
        if !out.exists() && !same(src, &out) && taken.insert(key(&out)) {
            return Ok(out);
        }
    }
    Err((Status::Failed, Some(first), "no free name for the output file".into()))
}

/// Read the drawing at `from` and write it to `to` in the job's format and version: what reading
/// left out and what DWG/DXF didn't keep.
pub fn convert_drawing(from: &Path, to: &Path, job: &Job) -> std::result::Result<Report, String> {
    let hooks = io().ok_or("file formats are not available in this build")?;
    let src = from.to_string_lossy();
    let dst = to.to_string_lossy();
    let bytes = crate::readfile::read(from).map_err(|e| e.to_string())?;
    let mut d = (hooks.read)(&bytes, &src)?;
    drop(bytes);
    crate::audit::current_styles(&mut d);
    let mut notes = d.read_notes.clone();
    if let Some(l) = read_loss(&d).filter(|l| !notes.contains(l)) {
        notes.push(l);
    }
    // A .cdy keeps everything whatever version is asked. Without a version each drawing keeps its
    // own, 2000 included (a plain save makes DWG at least 2004; Release 12 is DXF only).
    let own = || d.header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str).and_then(Version::for_acadver).filter(|v| *v != Version::R12);
    let version = match (job.to.as_str(), job.version) {
        ("cdy", _) => None,
        ("dwg", None) => own(),
        (_, v) => v,
    };
    let opts = SaveAs { version, binary: false };
    // The reason as a person reads it ("…: Permission denied"), not "invalid parameters for `save`".
    let out = write_drawing(&d, "dwgconvert", &dst, &opts).map_err(|e| e.user_message())?;
    let losses = if job.to == "cdy" { Vec::new() } else { check_written(&d, &dst, &out, hooks.read).map(|(_, l)| l).unwrap_or_default() };
    store_keeping(&dst, &out, job.bak).map_err(|e| e.user_message())?;
    Ok(Report { notes, losses })
}

/// Convert the job's drawings with `convert` (default [`convert_drawing`]), in order, telling
/// `progress` each outcome as it comes (with its index). `cancel` stops it after the drawing being
/// converted.
pub fn run(job: &Job, convert: Option<Converter>, cancel: &AtomicBool, progress: &mut dyn FnMut(usize, &Outcome)) -> Summary {
    let mut sum = Summary { total: job.inputs.len(), ..Summary::default() };
    let mut taken = HashSet::new();
    for (i, input) in job.inputs.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            sum.cancelled = true;
            break;
        }
        let from = input.path.display().to_string();
        let place = if input.path.is_file() { target(job, input, &mut taken) } else { Err((Status::Failed, None, "no such file".to_string())) };
        let outcome = match place {
            Err((status, to, reason)) => {
                Outcome { from, to: to.map(|t| t.display().to_string()), status, reason: Some(reason), report: Report::default() }
            }
            Ok(to) => {
                let made = to.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all);
                let r = match made {
                    Err(e) => Err(format!("{}: {e}", to.parent().map(|d| d.display().to_string()).unwrap_or_default())),
                    // A panicking reader or writer fails that drawing, not the batch.
                    Ok(()) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match convert {
                        Some(c) => c(&input.path, &to, job),
                        None => convert_drawing(&input.path, &to, job),
                    }))
                    .unwrap_or_else(|_| Err("the converter stopped on this drawing (an internal error)".into())),
                };
                let to = Some(to.display().to_string());
                match r {
                    Ok(report) => Outcome { from, to, status: Status::Converted, reason: None, report },
                    Err(e) => Outcome { from, to, status: Status::Failed, reason: Some(e), report: Report::default() },
                }
            }
        };
        progress(i, &outcome);
        sum.outcomes.push(outcome);
    }
    sum
}

/// Write the summary's log to `dir` (or the temporary folder) as "CADaddy convert <time>.log".
pub fn write_log(sum: &Summary, dir: Option<&Path>) -> Option<PathBuf> {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    let dir = dir.filter(|d| d.is_dir()).map_or_else(std::env::temp_dir, Path::to_path_buf);
    let path = dir.join(format!("CADaddy convert {secs}.log"));
    std::fs::write(&path, sum.log_text()).ok().map(|()| path)
}

fn run_convert(s: &mut Session, p: &Value) -> Result<Value> {
    if cfg!(target_arch = "wasm32") {
        return Err(EngineError::Other("DWGCONVERT: converting files is not available on the web".into()));
    }
    let job = job_from(p)?;
    let sum = run(&job, None, &AtomicBool::new(false), &mut |_, _| {});
    let mut r = sum.to_json();
    if bool_or(p, "log", false)
        && let Some(log) = write_log(&sum, job.out.as_deref())
    {
        r["log"] = json!(log.display().to_string());
    }
    s.echo(format!(
        "DWGCONVERT: {} converted, {} skipped, {} failed.",
        sum.count(Status::Converted),
        sum.count(Status::Skipped),
        sum.count(Status::Failed)
    ));
    Ok(r)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("dwgconvert", "Convert Drawings...", run_convert)
            .menu(&["File", "Convert Drawings..."])
            .alias(&["cdyconvert"])
            .params("{files?: [paths], folder?: path, recurse?: bool, to: cdy|dwg|dxf, version?: 2000|2004|2007|2010|2013|2018 (12: Release 12 DXF), out?: output folder (default next to each original; a folder's subfolders are kept), overwrite?: false|true|\"skip\"|\"overwrite\"|\"rename\" (default skip), bak?: bool (keep a file written over as .bak; default true), log?: bool} → {converted: [{from, to, losses?: what DWG/DXF didn't keep, notes?: what reading left out}], skipped: [{file, to, reason}], failed: [{file, error}], cancelled, total, log?}")
            .enabled(always)
            .noundo(),
        // The same for agents and scripts, by the name `cadaddy-cli convert` has.
        CommandSpec::new("convert", "Convert Drawings", run_convert).params("as DWGCONVERT").enabled(always).noundo(),
    ]
}
