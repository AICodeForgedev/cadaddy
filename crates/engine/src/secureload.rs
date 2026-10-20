//! SECURELOAD and TRUSTEDPATHS: LISP files (APPLOAD, `(load …)`) and scripts (SCRIPT) load
//! silently only from trusted folders — CADaddy's support folder and the TRUSTEDPATHS list
//! (`;`-separated; a folder ending in `\...` or `/...` includes its subfolders).
//!
//! - SECURELOAD 0: anything loads (not recommended);
//! - 1 (default): a file elsewhere isn't loaded; the app asks (Load Once / Always Load from This
//!   Folder / Don't Load) and runs the command again when allowed. A programmatic call gets the
//!   refusal and can allow the file with `secureload.allow`;
//! - 2: only trusted folders; nothing is asked.
//!
//! LISP and scripts can't change SECURELOAD or TRUSTEDPATHS or allow a file themselves, so a
//! routine can't unlock the next one. LISP files next to an opened drawing are never run on
//! opening (startup files come from the support folder only, see `lisp/startup.rs`).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bool_or, str_param};
use crate::{EngineError, Result, Session};

/// A load the user is asked about: the file, and the command to run again when allowed.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub path: PathBuf,
    /// The folder "Always" adds to TRUSTEDPATHS.
    pub folder: PathBuf,
    /// The command and parameters to run again once allowed (`None`: a `(load …)` inside a
    /// routine; the user runs it again).
    pub retry: Option<(String, Value)>,
}

/// A folder as compared: resolved when it exists, `\` as `/`, without a trailing separator, and
/// lower case on Windows and macOS (their file systems ignore case).
fn norm(p: &Path) -> String {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let t = p.to_string_lossy().replace('\\', "/");
    let t = t.trim_end_matches('/').to_string();
    if cfg!(any(windows, target_os = "macos")) { t.to_lowercase() } else { t }
}

/// The TRUSTEDPATHS entries: (folder, subfolders too).
fn trusted_entries(list: &str) -> Vec<(String, bool)> {
    list.split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| match e.strip_suffix("\\...").or_else(|| e.strip_suffix("/...")) {
            Some(dir) => (norm(Path::new(dir)), true),
            None => (norm(Path::new(e)), false),
        })
        .collect()
}

impl Session {
    /// Whether `file` is in a trusted folder (the support folder, TRUSTEDPATHS) or was allowed
    /// once this session.
    pub fn is_trusted_file(&self, file: &Path) -> bool {
        if self.secureload_once.contains(&norm(file)) {
            return true;
        }
        let dir = norm(file.parent().unwrap_or(Path::new(".")));
        let support = self.lisp_support_dir().map(norm);
        if support.is_some_and(|s| s == dir || dir.starts_with(&format!("{s}/"))) {
            return true;
        }
        trusted_entries(&self.settings.trustedpaths).iter().any(|(t, sub)| *t == dir || (*sub && dir.starts_with(&format!("{t}/"))))
    }

    /// Whether code (LISP or a script) is running now: it may not change the load rules.
    pub fn running_code(&self) -> bool {
        self.lisp_active() || self.in_script
    }

    /// May `file` be loaded (by `what`: "APPLOAD", "SCRIPT", "LOAD")? An error saying why not;
    /// at SECURELOAD 1 the question is kept for the app ([`Session::secureload_ask`]), with
    /// `retry` to run again once allowed.
    pub fn check_load(&mut self, file: &Path, what: &str, retry: Option<(String, Value)>) -> Result<()> {
        // The file the readers open (names match ignoring case); one that isn't there is
        // reported by the reader ("can't read"), not here.
        let Some(found) = cadaddy_geom::filename::find_file(file) else { return Ok(()) };
        let file = found.as_path();
        if self.settings.secureload == 0 || self.is_trusted_file(file) {
            return Ok(());
        }
        let shown = file.display();
        if self.settings.secureload >= 2 {
            return Err(EngineError::Other(format!(
                "{what}: {shown} isn't in a trusted folder and SECURELOAD is 2, so it wasn't loaded. Add its folder to TRUSTEDPATHS to allow it."
            )));
        }
        let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
        self.secureload_ask = Some(Ask { path: file.to_path_buf(), folder, retry });
        Err(EngineError::Other(format!(
            "{what}: {shown} isn't in a trusted folder, so it wasn't loaded (SECURELOAD 1). Load it only if you trust where it came from: allow it once, or add its folder to TRUSTEDPATHS."
        )))
    }
}

/// `secureload.allow {path, always?}`: allow a file once this session, or always (its folder
/// goes into TRUSTEDPATHS). Refused while LISP or a script runs.
fn run_allow(s: &mut Session, p: &Value) -> Result<Value> {
    if s.running_code() {
        return Err(EngineError::Other("secureload.allow can't be used from LISP or a script".into()));
    }
    let path =
        str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| crate::cmd::bad("secureload.allow", "`path` is required"))?;
    let file = PathBuf::from(path);
    if bool_or(p, "always", false) {
        let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let entry = folder.to_string_lossy().into_owned();
        if !trusted_entries(&s.settings.trustedpaths).iter().any(|(t, _)| *t == norm(&folder)) {
            let list = &mut s.settings.trustedpaths;
            if !list.trim().is_empty() && !list.trim_end().ends_with(';') {
                list.push(';');
            }
            list.push_str(&entry);
        }
        s.echo(format!("Added {entry} to TRUSTEDPATHS."));
    } else {
        s.secureload_once.insert(norm(&file));
    }
    if s.secureload_ask.as_ref().is_some_and(|a| norm(&a.path) == norm(&file)) {
        s.secureload_ask = None;
    }
    Ok(json!({ "allowed": path, "trustedpaths": s.settings.trustedpaths }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("secureload.allow", "Allow Loading a File", run_allow)
            .params("{path, always?: bool (add its folder to TRUSTEDPATHS; default: this session only)} → {allowed, trustedpaths} — refused from LISP and scripts")
            .enabled(always)
            .noundo(),
    ]
}
