//! Redline (our own free PDF editor for construction drawings, by AICodeForge): SEND TO REDLINE
//! plots the layouts to one PDF (a bookmark per sheet, the drawing's hyperlinks as links) and
//! opens it in Redline when it is installed, else just saves it; IMPORT REDLINE MARKUPS reads that
//! PDF's annotations back (MARKUPIMPORT) on a markup layer per reviewer. No accounts, no network.

use std::sync::OnceLock;

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Opens a PDF in Redline: `Ok(true)` when Redline took it, `Ok(false)` when it isn't installed.
/// Installed by the desktop app (it knows where Redline lives on this system).
pub type Opener = fn(&str) -> std::result::Result<bool, String>;

static OPENER: OnceLock<Opener> = OnceLock::new();

pub fn set_opener(f: Opener) {
    let _ = OPENER.set(f);
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("redline", "Send to Redline", run_send)
            .menu(&["File", "Send to Redline"])
            .params("{path?: where to write the PDF (default beside the drawing, same name; an unsaved drawing: the temporary folder), layouts?: [names] (default as PUBLISH: Model if it has objects, then every layout)} → {path, pages, opened: bool (Redline took it; false: not installed, the PDF is saved)}")
            .noundo()
            .enabled(has_doc),
        CommandSpec::new("redline.import", "Import Redline Markups", run_import)
            .menu(&["Insert", "Import Redline Markups..."])
            .params("{path: the reviewed PDF, page?, placement as MARKUPIMPORT} → MARKUPIMPORT's result: the markups on MARKUP-<reviewer> layers")
            .enabled(has_doc),
    ]
}

/// The PDF's default path: the drawing's file with `.pdf`, else the title in the temporary folder.
fn default_path(s: &Session) -> Result<String> {
    let st = s.state()?;
    Ok(match &st.path {
        Some(p) => std::path::Path::new(p).with_extension("pdf").to_string_lossy().into_owned(),
        None => {
            let stem = std::path::Path::new(&st.title).file_stem().map_or_else(|| "Drawing".into(), |s| s.to_string_lossy().into_owned());
            std::env::temp_dir().join(format!("{stem}.pdf")).to_string_lossy().into_owned()
        }
    })
}

fn run_send(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => x.to_string(),
        None => default_path(s)?,
    };
    let mut q = json!({ "path": path, "single": true });
    if let Some(l) = p.get("layouts").filter(|v| v.is_array()) {
        q["layouts"] = l.clone();
    }
    let publish = find_command("publish").ok_or_else(|| crate::EngineError::Other("Publishing is not available in this build.".into()))?;
    // A build that can't plot (or a bad layout name) is a state the user can't fix with parameters.
    let r = (publish.run)(s, &q).map_err(|e| match e {
        crate::EngineError::BadParams { msg: m, .. } => crate::EngineError::Other(format!("SEND TO REDLINE: {m}")),
        e => e,
    })?;
    let pages = r.get("pages").cloned().unwrap_or(Value::Null);
    let written = r.get("path").and_then(Value::as_str).unwrap_or(&path).to_string();
    let opened = match OPENER.get() {
        Some(open) => open(&written).unwrap_or(false),
        None => false,
    };
    s.echo(if opened {
        format!("Sent to Redline: {written}")
    } else {
        format!("Saved {written}. Redline isn't installed here (it's free from aicodeforge.dev); the PDF opens in any PDF reader.")
    });
    Ok(json!({ "path": written, "pages": pages, "opened": opened }))
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        o.insert("byAuthor".into(), json!(true));
    }
    let import = find_command("markupimport").ok_or_else(|| crate::EngineError::Other("Markup import is not available in this build.".into()))?;
    (import.run)(s, &q)
}
