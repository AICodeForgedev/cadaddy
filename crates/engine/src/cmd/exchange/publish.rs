//! PUBLISH with a sheet list: sheets from this and other drawings, each with its own page setup
//! overrides, in the list's order, published to one PDF with a bookmark per sheet (or a PDF per
//! sheet); and publish lists saved and loaded as our own JSON file (a `.dsd`-like list:
//! `{"cadaddyPublishList": 1, "sheets": [{drawing?, layout, title?, page?}]}`, a sheet's
//! `drawing` is omitted for the drawing published from).

use std::path::Path;

use serde_json::{Value, json};

use super::super::file::{base64_decode, base64_encode, io, pdf_merge, store};
use crate::cmd::{CommandSpec, bad, str_param};
use crate::{Result, Session};

/// Most sheets in one publish, and the largest list file read.
const MAX_SHEETS: usize = 1_000;
const MAX_LIST: u64 = 4 << 20;
/// The page setup overrides a sheet may carry (PLOT's options).
const PAGE_KEYS: [&str; 7] = ["paper", "landscape", "plotStyleTable", "fit", "scale", "lineweights", "plotTransparency"];

pub(super) fn spec() -> CommandSpec {
    CommandSpec::new("publishlist", "Publish List", run_list)
        .params("{action: \"save\" {path, sheets} | \"load\" {path} → {sheets} | \"layouts\" {drawing?} → {sheets} (Model if it has objects, then the layouts in tab order)}")
        .noundo()
}

/// One sheet of a publish list.
#[derive(Clone, Debug)]
pub(super) struct Sheet {
    pub drawing: Option<String>,
    pub layout: String,
    pub title: Option<String>,
    pub page: serde_json::Map<String, Value>,
}

fn sheet(cmd: &str, v: &Value) -> Result<Sheet> {
    let layout = str_param(v, "layout").map(str::trim).filter(|l| !l.is_empty()).ok_or_else(|| bad(cmd, "each sheet needs a `layout`"))?;
    let mut page = serde_json::Map::new();
    if let Some(pg) = v.get("page").and_then(Value::as_object) {
        for k in PAGE_KEYS {
            if let Some(x) = pg.get(k).filter(|x| !x.is_null()) {
                page.insert(k.into(), x.clone());
            }
        }
    }
    Ok(Sheet {
        drawing: str_param(v, "drawing").map(str::trim).filter(|d| !d.is_empty()).map(str::to_string),
        layout: layout.chars().take(255).collect(),
        title: str_param(v, "title").map(|t| t.chars().take(255).collect()),
        page,
    })
}

pub(super) fn sheets(cmd: &str, v: &Value) -> Result<Vec<Sheet>> {
    let a = v.as_array().ok_or_else(|| bad(cmd, "`sheets` must be a list of {drawing?, layout, title?, page?}"))?;
    if a.len() > MAX_SHEETS {
        return Err(bad(cmd, format!("at most {MAX_SHEETS} sheets")));
    }
    a.iter().map(|x| sheet(cmd, x)).collect()
}

fn sheet_json(s: &Sheet) -> Value {
    let mut v = json!({ "layout": s.layout });
    if let Some(d) = &s.drawing {
        v["drawing"] = json!(d);
    }
    if let Some(t) = &s.title {
        v["title"] = json!(t);
    }
    if !s.page.is_empty() {
        v["page"] = Value::Object(s.page.clone());
    }
    v
}

#[cfg(not(target_arch = "wasm32"))]
fn read_text(cmd: &str, path: &str, max: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    let mut b = Vec::new();
    f.take(max + 1).read_to_end(&mut b).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    if b.len() as u64 > max {
        return Err(bad(cmd, format!("{path}: too large")));
    }
    Ok(b)
}
#[cfg(target_arch = "wasm32")]
fn read_text(cmd: &str, _path: &str, _max: u64) -> Result<Vec<u8>> {
    Err(bad(cmd, "files are not available on the web"))
}

/// The sheets of the publish list at `path`.
pub(super) fn load(cmd: &str, path: &str) -> Result<Vec<Sheet>> {
    let v: Value = serde_json::from_slice(&read_text(cmd, path, MAX_LIST)?).map_err(|e| bad(cmd, format!("{path}: not a publish list: {e}")))?;
    if v.get("cadaddyPublishList").and_then(Value::as_u64) != Some(1) {
        return Err(bad(cmd, format!("{path}: not a CADaddy publish list")));
    }
    sheets(cmd, v.get("sheets").unwrap_or(&Value::Null))
}

/// A drawing's sheets: Model if it has objects, then its layouts in tab order.
fn layouts_of(d: &cadaddy_doc::Drawing, drawing: Option<&str>) -> Vec<Sheet> {
    let mut ls: Vec<&cadaddy_doc::Layout> = d.layouts.iter().collect();
    ls.sort_by_key(|l| l.tab_order);
    let model = (!d.model.is_empty()).then(|| "Model".to_string());
    model
        .into_iter()
        .chain(ls.iter().map(|l| l.name.clone()))
        .map(|layout| Sheet { drawing: drawing.map(str::to_string), layout, title: None, page: serde_json::Map::new() })
        .collect()
}

fn read_drawing(cmd: &str, path: &str) -> Result<cadaddy_doc::Drawing> {
    let hooks = io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    let bytes = read_text(cmd, path, 512 << 20)?;
    (hooks.read)(&bytes, path).map_err(|e| bad(cmd, format!("{path}: {e}")))
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "publishlist";
    match str_param(p, "action").unwrap_or("load").to_ascii_lowercase().as_str() {
        "save" => {
            let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
            let list = sheets(cmd, p.get("sheets").unwrap_or(&Value::Null))?;
            let v = json!({ "cadaddyPublishList": 1, "sheets": list.iter().map(sheet_json).collect::<Vec<_>>() });
            let text = serde_json::to_string_pretty(&v).map_err(|e| bad(cmd, e.to_string()))?;
            store(path, text.as_bytes())?;
            Ok(json!({ "path": path, "sheets": list.len() }))
        }
        "load" => {
            let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
            Ok(json!({ "sheets": load(cmd, path)?.iter().map(sheet_json).collect::<Vec<_>>() }))
        }
        "layouts" => {
            let list = match str_param(p, "drawing") {
                Some(path) => layouts_of(&read_drawing(cmd, path)?, Some(path)),
                None => layouts_of(s.doc()?, None),
            };
            Ok(json!({ "sheets": list.iter().map(sheet_json).collect::<Vec<_>>() }))
        }
        other => Err(bad(cmd, format!("unknown action \"{other}\" (save, load or layouts)"))),
    }
}

/// The bookmark of a sheet: its title, else the drawing's name and the layout when the list
/// spans drawings, else the layout.
fn bookmark(s: &Sheet, many: bool) -> String {
    if let Some(t) = s.title.as_deref().filter(|t| !t.trim().is_empty()) {
        return t.to_string();
    }
    match (&s.drawing, many) {
        (Some(d), true) => format!("{} - {}", Path::new(d).file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(), s.layout),
        (None, true) => format!("{} - {}", "This drawing", s.layout),
        _ => s.layout.clone(),
    }
}

/// Publish `list`: each sheet plotted to PDF with its overrides, then one PDF (`single`) with a
/// bookmark per sheet, or a PDF per sheet beside `path`.
pub(super) fn publish(s: &mut Session, list: &[Sheet], path: Option<&str>, single: bool) -> Result<Value> {
    let cmd = "publish";
    if list.is_empty() {
        return Err(bad(cmd, "there are no sheets to publish"));
    }
    let hook = io().and_then(|h| h.plot).ok_or_else(|| bad(cmd, "plotting is not available in this build"))?;
    let many = list.iter().any(|x| x.drawing.is_some());
    let mut cache: Option<(String, cadaddy_doc::Drawing)> = None;
    let mut parts: Vec<(Vec<u8>, String, String)> = Vec::with_capacity(list.len());
    for sh in list {
        let mut q = Value::Object(sh.page.clone());
        q["layout"] = json!(sh.layout);
        let bytes = match &sh.drawing {
            // This drawing: through PLOT, as a plot of it would be.
            None => {
                let r = s.execute("plot", &q).map_err(|e| bad(cmd, format!("{}: {e}", sh.layout)))?;
                r["data"].as_str().and_then(base64_decode).ok_or_else(|| bad(cmd, format!("{}: the plotter returned nothing", sh.layout)))?
            }
            Some(file) => {
                if cache.as_ref().is_none_or(|(f, _)| f != file) {
                    cache = Some((file.clone(), read_drawing(cmd, file)?));
                }
                let Some((_, d)) = &cache else { continue };
                let space = if sh.layout.eq_ignore_ascii_case("model") {
                    cadaddy_doc::Space::Model
                } else {
                    cadaddy_doc::Space::Paper(
                        d.layout(&sh.layout).map(|l| l.name.clone()).ok_or_else(|| bad(cmd, format!("{file}: no layout `{}`", sh.layout)))?,
                    )
                };
                let mut opts = Value::Object(sh.page.clone());
                if let Some(v) = opts.get("scale").cloned()
                    && let Some(k) = crate::cmd::layout::parse_scale(&v)
                {
                    opts["scale"] = json!(k);
                }
                hook(d, &space, &opts).map_err(|e| bad(cmd, format!("{file} {}: {e}", sh.layout)))?
            }
        };
        parts.push((bytes, bookmark(sh, many), String::new()));
    }
    if single {
        let merge = pdf_merge().ok_or_else(|| bad(cmd, "publishing to one PDF is not available in this build"))?;
        let pdf = merge(&parts).map_err(|e| bad(cmd, e))?;
        let pages = parts.len();
        return match path {
            Some(path) => {
                store(path, &pdf)?;
                Ok(json!({ "path": path, "pages": pages, "bytes": pdf.len(), "message": format!("Published {pages} sheet(s) to {path}") }))
            }
            None => Ok(json!({ "data": base64_encode(&pdf), "pages": pages, "bytes": pdf.len() })),
        };
    }
    let base = path.map(|path| {
        let pb = Path::new(path);
        let stem = pb.file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Drawing".into());
        (pb.parent().map(Path::to_path_buf).unwrap_or_default(), stem)
    });
    let mut out = Vec::new();
    for (bytes, title, _) in parts {
        let mut r = json!({ "layout": title, "bytes": bytes.len() });
        match &base {
            Some((dir, stem)) => {
                let file = dir.join(format!("{stem}-{}.pdf", super::file_part(&title))).to_string_lossy().into_owned();
                store(&file, &bytes)?;
                r["path"] = json!(file);
            }
            None => r["data"] = json!(base64_encode(&bytes)),
        }
        out.push(r);
    }
    Ok(json!({ "sheets": out, "message": format!("Published {} sheet(s)", out.len()) }))
}
