//! Redline (our own free PDF editor for construction drawings, by AICodeForge): SEND TO REDLINE
//! plots the layouts to one PDF (a bookmark per sheet, the drawing's hyperlinks as links) and
//! opens it in Redline when it is installed, else just saves it; IMPORT REDLINE MARKUPS reads that
//! PDF's annotations back on a markup layer per reviewer, matching each page to the layout it came
//! from. No accounts, no network.

use std::sync::OnceLock;

use cadaddy_doc::markup::{PdfBookmarks, PdfPageCount};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Opens a PDF in Redline: `Ok(true)` when Redline took it, `Ok(false)` when it isn't installed.
/// Installed by the desktop app (it knows where Redline lives on this system).
pub type Opener = fn(&str) -> std::result::Result<bool, String>;

static OPENER: OnceLock<Opener> = OnceLock::new();
static PAGE_COUNT: OnceLock<PdfPageCount> = OnceLock::new();
static BOOKMARKS: OnceLock<PdfBookmarks> = OnceLock::new();

pub fn set_opener(f: Opener) {
    let _ = OPENER.set(f);
}

pub fn set_page_count(f: PdfPageCount) {
    let _ = PAGE_COUNT.set(f);
}

pub fn set_bookmarks(f: PdfBookmarks) {
    let _ = BOOKMARKS.set(f);
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("redline", "Send to Redline", run_send)
            .menu(&["File", "Send to Redline"])
            .params("{path?: where to write the PDF (default beside the drawing, same name; an unsaved drawing: the temporary folder), force?: bool (replace an existing `path`; the default path never replaces a file: it becomes NAME-redline.pdf), layouts?: [names] (default as PUBLISH: Model if it has objects, then every layout)} → {path, pages, opened: bool (Redline took it; false: not installed, the PDF is saved)}")
            .noundo()
            .enabled(has_doc),
        CommandSpec::new("redline.import", "Import Redline Markups", run_import)
            .menu(&["Insert", "Import Redline Markups..."])
            .params("{path: the reviewed PDF, page?: (restricts to one page on the current layout), placement as MARKUPIMPORT} → {pages: [{page, layout, placed}], unmatched: [{page, markups}]}")
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
    let mut moved = false;
    let path = match str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => {
            super::exchange::refuse_existing("redline", p, x)?;
            x.to_string()
        }
        None => {
            // Beside the drawing a PDF of that name may be the original print: never replace it.
            let (path, m) = super::exchange::unused_path(&default_path(s)?, "redline");
            moved = m;
            path
        }
    };
    let mut q = json!({ "path": path, "single": true });
    if let Some(f) = p.get("force") {
        q["force"] = f.clone();
    }
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
    if moved {
        s.echo(format!("A PDF of that name already exists, so it was kept; the redline PDF is {written}."));
    }
    let opened = match OPENER.get() {
        Some(open) => open(&written).unwrap_or(false),
        None => false,
    };
    s.echo(if opened {
        format!("Sent to Redline: {written}")
    } else {
        format!("Saved {written}. Redline isn't installed here (it's free from aicodeforge.dev); the PDF opens in any PDF reader.")
    });
    Ok(json!({ "path": written, "pages": pages, "opened": opened, "keptExisting": moved }))
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let import = find_command("markupimport").ok_or_else(|| crate::EngineError::Other("Markup import is not available in this build.".into()))?;

    // Single-page mode: `page` param given, or no page-count hook installed — old behaviour.
    if p.get("page").is_some() || PAGE_COUNT.get().is_none() {
        let mut q = p.clone();
        if let Some(o) = q.as_object_mut() {
            o.insert("byAuthor".into(), json!(true));
        }
        return (import.run)(s, &q);
    }

    let path = str_param(p, "path")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad("redline.import", "`path` of a PDF is required"))?
        .to_string();

    #[cfg(not(target_arch = "wasm32"))]
    let bytes = crate::readfile::read(&path).map_err(|e| bad("redline.import", format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let bytes: Vec<u8> = return Err(bad("redline.import", "files can't be read on the web"));

    let page_count_fn = PAGE_COUNT.get().ok_or_else(|| bad("redline.import", "PDF page count is not available"))?;
    let total = page_count_fn(&bytes).map_err(|e| bad("redline.import", e))?;

    // Bookmark titles → page→layout name map.
    let bmarks = BOOKMARKS.get().and_then(|f| f(&bytes).ok()).unwrap_or_default();
    let page_title: std::collections::HashMap<usize, String> = bmarks.into_iter().map(|(t, pg)| (pg, t)).collect();

    // The drawing's layout names.
    let layout_names: Vec<String> = s.doc()?.layouts.iter().map(|l| l.name.clone()).collect();

    // The layout order PUBLISH would use (Model if it has objects, then layouts in order).
    let model_has_objects = !s.doc()?.model.is_empty();
    let mut publish_order: Vec<String> = Vec::new();
    if model_has_objects {
        publish_order.push("Model".to_string());
    }
    publish_order.extend(layout_names.iter().cloned());

    // Map each page to a layout: by bookmark title first, then by publish page order.
    let match_layout = |pg: u32| -> Option<String> {
        if let Some(title) = page_title.get(&(pg as usize)) {
            if layout_names.iter().any(|n| n == title) {
                return Some(title.clone());
            }
            if title == "Model" {
                return Some("Model".to_string());
            }
        }
        // Fallback: page order of published layouts.
        publish_order.get(pg as usize - 1).cloned()
    };

    // Single-page PDF: fall back to the current layout.
    let current_layout = match s.layout_space() {
        cadaddy_doc::Space::Paper(name) => name,
        _ => String::new(),
    };

    let mut pages_result: Vec<Value> = Vec::new();
    let mut total_placed = 0usize;
    let mut unmatched: Vec<Value> = Vec::new();

    for pg in 1..=total {
        let layout = if total == 1 && match_layout(pg).is_none() {
            if current_layout.is_empty() { None } else { Some(current_layout.clone()) }
        } else {
            match_layout(pg)
        };

        match layout.as_deref() {
            Some("Model") => {
                // MUST 2: refuse Model pages with a message about from/to placement.
                let reader = super::markup::READER.get().ok_or_else(|| bad("redline.import", "PDF markups can't be read in this build"))?;
                let count = reader(&bytes, pg).map(|p| p.markups.len()).unwrap_or(0);
                if count > 0 {
                    unmatched.push(json!({
                        "page": pg,
                        "markups": count,
                        "reason": format!(
                            "page {pg} came from Model; place its markups in model space with \
                             `from`/`to` (two page points and where they go in model space)")
                    }));
                    s.echo(format!(
                        "Page {pg}: {count} markup(s) from Model skipped — use \
                         markupimport with `from`/`to` to place them in model space."
                    ));
                }
            }
            Some(name) => {
                // Switch to this layout, import, collect results.
                s.execute("layout", &json!({ "option": "set", "name": name }))?;
                let q = json!({
                    "path": path,
                    "page": pg,
                    "byAuthor": true,
                });
                match (import.run)(s, &q) {
                    Ok(r) => {
                        let placed = r.pointer("/placed").cloned().unwrap_or(json!([]));
                        let n = placed.as_array().map_or(0, Vec::len);
                        if n > 0 {
                            total_placed += n;
                            pages_result.push(json!({
                                "page": pg,
                                "layout": name,
                                "placed": placed,
                            }));
                        }
                    }
                    Err(e) => {
                        s.echo(format!("Page {pg} ({name}): {e}"));
                    }
                }
            }
            None => {
                let reader = super::markup::READER.get().ok_or_else(|| bad("redline.import", "PDF markups can't be read in this build"))?;
                let count = reader(&bytes, pg).map(|p| p.markups.len()).unwrap_or(0);
                if count > 0 {
                    unmatched.push(json!({ "page": pg, "markups": count }));
                    s.echo(format!("Page {pg}: {count} markup(s) could not be matched to a layout."));
                }
            }
        }
    }

    // Restore the layout we started on, if it still exists.
    if !current_layout.is_empty() {
        let _ = s.execute("layout", &json!({ "option": "set", "name": current_layout }));
    }

    let msg = format!(
        "{total_placed} markup(s) imported from {pg} page(s){unm}.",
        pg = pages_result.len(),
        unm = if unmatched.is_empty() { String::new() } else { format!(", {} page(s) unmatched", unmatched.len()) },
    );
    s.echo(&msg);

    Ok(json!({ "pages": pages_result, "unmatched": unmatched, "message": msg }))
}
