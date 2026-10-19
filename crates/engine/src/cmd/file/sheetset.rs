//! Sheet sets: named, numbered sheets (layouts of drawings) in subsets, kept in our own JSON file
//! (`*.sheetset.json`), published together to one PDF with a bookmark per sheet.
//!
//! The file: `{"cadaddySheetSet": 1, "name", "description", "subsets": [names, in order],
//! "sheets": [{"number", "title", "drawing", "layout", "subset"}]}`. A sheet's `drawing` is
//! relative to the sheet set's file when it is in or below its folder, else absolute; `subset`
//! is "" for a sheet in no subset.
//!
//! Every action names the file (`set`) and reads it, changes it and writes it again, so nothing
//! is held between commands. Sheet numbers and titles reach a drawing's fields through its layout
//! names: `sync` renames each sheet's layout to "number title", which the CurrentSheetNumber /
//! CurrentSheetTitle fields read. AutoCAD's `.dst` sheet set files are a binary format of their
//! own and are not read.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{io, pdf_merge, store};
use crate::cmd::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const CMD: &str = "sheetset";
/// Most sheets in a set, and the largest set file read.
const MAX_SHEETS: usize = 5_000;
const MAX_FILE: u64 = 16 << 20;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sheet {
    pub number: String,
    pub title: String,
    pub drawing: String,
    pub layout: String,
    pub subset: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SheetSet {
    #[serde(rename = "cadaddySheetSet")]
    pub format: u32,
    pub name: String,
    pub description: String,
    pub subsets: Vec<String>,
    pub sheets: Vec<Sheet>,
}

pub(super) fn spec() -> CommandSpec {
    CommandSpec::new(CMD, "Sheet Set Manager", run)
        .menu(&["Tools", "Palettes", "Sheet Set Manager"])
        .alias(&["ssm"])
        .params(
            "{set: path of the sheet set file (.sheetset.json), action: new {name?, description?, overwrite?} | list | add {drawing | drawings: [paths], layouts?: [names] (default every layout, in tab order), subset?, number?, title?} \
             | remove {sheets: [index | number]} | set {sheet: index | number, number?, title?, subset?} | move {sheet, to: index} | renumber {start? (1), step? (1), prefix?, digits?, subset? (only those)} \
             | subset {add?: name, remove?: name (its sheets leave it), rename?: [old, new]} | sync (rename each sheet's layout to \"number title\" in its drawing, for the sheet fields) \
             | publish {path: OUT.pdf} (one PDF, a bookmark per sheet under its subset)} → the set's listing (and {path, pages} for publish). AutoCAD .dst files are not read.",
        )
        .noundo()
}

fn set_path(p: &Value) -> Result<PathBuf> {
    let s = str_param(p, "set").map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(CMD, "`set` (the sheet set file) is required"))?;
    if s.to_ascii_lowercase().ends_with(".dst") {
        return Err(bad(
            CMD,
            "AutoCAD .dst sheet sets are a binary format CADaddy doesn't read; make a new sheet set from the drawings (action add)",
        ));
    }
    Ok(PathBuf::from(s))
}

#[cfg(not(target_arch = "wasm32"))]
fn load(path: &Path) -> Result<SheetSet> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| bad(CMD, format!("{}: {e}", path.display())))?;
    let mut text = String::new();
    f.take(MAX_FILE + 1).read_to_string(&mut text).map_err(|e| bad(CMD, format!("{}: {e}", path.display())))?;
    if text.len() as u64 > MAX_FILE {
        return Err(bad(CMD, format!("{}: larger than {} MB", path.display(), MAX_FILE >> 20)));
    }
    let set: SheetSet = serde_json::from_str(&text).map_err(|e| bad(CMD, format!("{}: not a sheet set: {e}", path.display())))?;
    if set.format != 1 {
        return Err(bad(CMD, format!("{}: not a CADaddy sheet set (version {})", path.display(), set.format)));
    }
    if set.sheets.len() > MAX_SHEETS {
        return Err(bad(CMD, format!("{}: more than {MAX_SHEETS} sheets", path.display())));
    }
    Ok(set)
}
#[cfg(target_arch = "wasm32")]
fn load(_path: &Path) -> Result<SheetSet> {
    Err(bad(CMD, "sheet sets need files, which the web doesn't have"))
}

#[cfg(not(target_arch = "wasm32"))]
fn save(path: &Path, set: &SheetSet) -> Result<()> {
    let text = serde_json::to_string_pretty(set).map_err(|e| bad(CMD, e.to_string()))?;
    let tmp = path.with_extension("cadaddy-tmp");
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        bad(CMD, format!("{}: {e}", path.display()))
    })
}
#[cfg(target_arch = "wasm32")]
fn save(_path: &Path, _set: &SheetSet) -> Result<()> {
    Err(bad(CMD, "sheet sets need files, which the web doesn't have"))
}

/// The file of a sheet's drawing.
fn drawing_file(set: &Path, stored: &str) -> Result<PathBuf> {
    let abs = std::path::absolute(set).unwrap_or_else(|_| set.to_path_buf());
    crate::doc_path::resolve(stored, Some(&abs)).map_err(|r| bad(CMD, format!("{stored}: not opened, {r}")))
}

/// How a drawing at `file` is stored in the set at `set`: relative when it is in or below the
/// set's folder.
fn stored_path(set: &Path, file: &Path) -> String {
    let dir = std::path::absolute(set).ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    match dir.as_deref().and_then(|d| file.strip_prefix(d).ok()) {
        Some(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"),
        None => file.to_string_lossy().into_owned(),
    }
}

fn read_drawing(file: &Path) -> Result<cadaddy_doc::Drawing> {
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Read;
        let f = std::fs::File::open(file).map_err(|e| bad(CMD, format!("{}: {e}", file.display())))?;
        let mut bytes = Vec::new();
        f.take((512 << 20) + 1).read_to_end(&mut bytes).map_err(|e| bad(CMD, format!("{}: {e}", file.display())))?;
        if bytes.len() > 512 << 20 {
            return Err(bad(CMD, format!("{}: larger than 512 MB", file.display())));
        }
        (hooks.read)(&bytes, &file.to_string_lossy()).map_err(|e| bad(CMD, format!("{}: {e}", file.display())))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (hooks, file);
        Err(bad(CMD, "sheet sets need files, which the web doesn't have"))
    }
}

/// The index of the sheet `v` names: its position or its number.
fn sheet_index(set: &SheetSet, v: Option<&Value>) -> Result<usize> {
    let i = match v {
        Some(Value::Number(n)) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Some(Value::String(s)) => set.sheets.iter().position(|sh| sh.number.eq_ignore_ascii_case(s.trim())),
        _ => None,
    };
    i.filter(|i| *i < set.sheets.len()).ok_or_else(|| bad(CMD, format!("no sheet {}", v.map_or("(none given)".to_string(), Value::to_string))))
}

fn text(p: &Value, k: &str) -> Option<String> {
    str_param(p, k).map(|s| s.trim().chars().take(255).collect())
}

fn listing(path: &Path, set: &SheetSet) -> Value {
    let sheets: Vec<Value> = set
        .sheets
        .iter()
        .enumerate()
        .map(|(i, sh)| {
            let found = drawing_file(path, &sh.drawing).is_ok_and(|f| f.is_file());
            json!({ "index": i, "number": sh.number, "title": sh.title, "drawing": sh.drawing, "layout": sh.layout, "subset": sh.subset, "found": found })
        })
        .collect();
    json!({ "set": path.to_string_lossy(), "name": set.name, "description": set.description, "subsets": set.subsets, "sheets": sheets })
}

fn run(_s: &mut Session, p: &Value) -> Result<Value> {
    let path = set_path(p)?;
    let action = str_param(p, "action").unwrap_or("list").to_ascii_lowercase();
    if action == "new" {
        if path.exists() && !bool_or(p, "overwrite", false) {
            return Err(bad(CMD, format!("{} already exists (overwrite: true replaces it)", path.display())));
        }
        let name = text(p, "name").filter(|n| !n.is_empty()).unwrap_or_else(|| {
            let f = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            f.trim_end_matches(".json").trim_end_matches(".sheetset").to_string()
        });
        let set = SheetSet { format: 1, name, description: text(p, "description").unwrap_or_default(), ..Default::default() };
        save(&path, &set)?;
        return Ok(listing(&path, &set));
    }
    let mut set = load(&path)?;
    match action.as_str() {
        "list" => return Ok(listing(&path, &set)),
        "add" => add(&path, &mut set, p)?,
        "remove" => {
            let mut idx: Vec<usize> = match p.get("sheets").or_else(|| p.get("sheet")) {
                Some(Value::Array(a)) => a.iter().map(|v| sheet_index(&set, Some(v))).collect::<Result<_>>()?,
                v => vec![sheet_index(&set, v)?],
            };
            idx.sort_unstable();
            idx.dedup();
            for i in idx.into_iter().rev() {
                set.sheets.remove(i);
            }
        }
        "set" => {
            let i = sheet_index(&set, p.get("sheet"))?;
            let subset = text(p, "subset");
            if let Some(sub) = subset.as_deref().filter(|s| !s.is_empty())
                && !set.subsets.iter().any(|x| x == sub)
            {
                set.subsets.push(sub.to_string());
            }
            if let Some(sh) = set.sheets.get_mut(i) {
                if let Some(n) = text(p, "number") {
                    sh.number = n;
                }
                if let Some(t) = text(p, "title") {
                    sh.title = t;
                }
                if let Some(sub) = subset {
                    sh.subset = sub;
                }
            }
        }
        "move" => {
            let i = sheet_index(&set, p.get("sheet"))?;
            let to =
                p.get("to").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(CMD, "`to` (a position) is required"))?;
            let sh = set.sheets.remove(i);
            set.sheets.insert(to.min(set.sheets.len()), sh);
        }
        "renumber" => {
            let num = |k: &str, d: i64| p.get(k).and_then(Value::as_i64).unwrap_or(d);
            let (start, step) = (num("start", 1), num("step", 1));
            let digits = usize::try_from(num("digits", 0).clamp(0, 12)).unwrap_or(0);
            let prefix = text(p, "prefix").unwrap_or_default();
            let only = text(p, "subset");
            let mut n = start;
            for sh in set.sheets.iter_mut().filter(|sh| only.as_ref().is_none_or(|o| sh.subset.eq_ignore_ascii_case(o))) {
                sh.number = format!("{prefix}{n:0digits$}");
                n = n.saturating_add(step);
            }
        }
        "subset" => {
            if let Some(a) = text(p, "add").filter(|a| !a.is_empty())
                && !set.subsets.iter().any(|x| x.eq_ignore_ascii_case(&a))
            {
                set.subsets.push(a);
            }
            if let Some(r) = text(p, "remove") {
                set.subsets.retain(|x| !x.eq_ignore_ascii_case(&r));
                for sh in set.sheets.iter_mut().filter(|sh| sh.subset.eq_ignore_ascii_case(&r)) {
                    sh.subset.clear();
                }
            }
            if let Some(Value::Array(a)) = p.get("rename") {
                let (Some(old), Some(new)) =
                    (a.first().and_then(Value::as_str), a.get(1).and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()))
                else {
                    return Err(bad(CMD, "`rename` is [old, new]"));
                };
                for x in set.subsets.iter_mut().filter(|x| x.eq_ignore_ascii_case(old)) {
                    *x = new.to_string();
                }
                for sh in set.sheets.iter_mut().filter(|sh| sh.subset.eq_ignore_ascii_case(old)) {
                    sh.subset = new.to_string();
                }
            }
        }
        "sync" => sync(&path, &mut set)?,
        "publish" => return publish(&path, &set, p),
        other => return Err(bad(CMD, format!("unknown action \"{other}\" (new, list, add, remove, set, move, renumber, subset, sync, publish)"))),
    }
    save(&path, &set)?;
    Ok(listing(&path, &set))
}

/// Add sheets: the layouts of one or more drawings.
fn add(path: &Path, set: &mut SheetSet, p: &Value) -> Result<()> {
    let drawings: Vec<String> = match (p.get("drawings"), str_param(p, "drawing")) {
        (Some(Value::Array(a)), _) => {
            a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(CMD, "`drawings` holds file paths"))).collect::<Result<_>>()?
        }
        (_, Some(d)) => vec![d.to_string()],
        _ => return Err(bad(CMD, "`drawing` or `drawings` is required")),
    };
    let wanted: Option<Vec<String>> = match p.get("layouts") {
        Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_str).map(str::to_string).collect()),
        _ => None,
    };
    let subset = text(p, "subset").unwrap_or_default();
    if !subset.is_empty() && !set.subsets.iter().any(|x| x.eq_ignore_ascii_case(&subset)) {
        set.subsets.push(subset.clone());
    }
    let single = drawings.len() == 1 && wanted.as_ref().is_some_and(|w| w.len() == 1);
    for d in &drawings {
        let file = PathBuf::from(d);
        let doc = read_drawing(&file)?;
        let mut layouts: Vec<&cadaddy_doc::Layout> = doc.layouts.iter().collect();
        layouts.sort_by_key(|l| l.tab_order);
        let names: Vec<String> = match &wanted {
            Some(w) => w
                .iter()
                .map(|n| doc.layout(n).map(|l| l.name.clone()).ok_or_else(|| bad(CMD, format!("{d}: no layout `{n}`"))))
                .collect::<Result<_>>()?,
            None => layouts.iter().map(|l| l.name.clone()).collect(),
        };
        if names.is_empty() {
            return Err(bad(CMD, format!("{d} has no layouts to make sheets of")));
        }
        for layout in names {
            if set.sheets.len() >= MAX_SHEETS {
                return Err(bad(CMD, format!("a sheet set holds at most {MAX_SHEETS} sheets")));
            }
            let number = if single { text(p, "number") } else { None }.unwrap_or_else(|| (set.sheets.len() + 1).to_string());
            let title = if single { text(p, "title") } else { None }.unwrap_or_else(|| layout.clone());
            set.sheets.push(Sheet { number, title, drawing: stored_path(path, &file), layout, subset: subset.clone() });
        }
    }
    Ok(())
}

/// Rename each sheet's layout to "number title" in its drawing (written again, its previous
/// version kept as .bak), so the drawing's sheet number and title fields show them.
fn sync(path: &Path, set: &mut SheetSet) -> Result<()> {
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let mut files: Vec<String> = set.sheets.iter().map(|s| s.drawing.clone()).collect();
    files.sort();
    files.dedup();
    for stored in files {
        let file = drawing_file(path, &stored)?;
        let mut d = read_drawing(&file)?;
        let mut changed = false;
        for sh in set.sheets.iter_mut().filter(|s| s.drawing == stored) {
            let want: String = format!("{} {}", sh.number, sh.title).trim().chars().take(255).collect();
            if want.is_empty() || want == sh.layout {
                continue;
            }
            if d.layout(&want).is_some() {
                return Err(bad(CMD, format!("{stored}: a layout `{want}` already exists")));
            }
            let Some(l) = d.layouts.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&sh.layout)) else {
                return Err(bad(CMD, format!("{stored}: no layout `{}`", sh.layout)));
            };
            l.name = want.clone();
            sh.layout = want;
            changed = true;
        }
        if changed {
            let name = file.to_string_lossy().into_owned();
            let bytes = (hooks.write)(&d, &name).map_err(|e| bad(CMD, format!("{name}: {e}")))?;
            store(&name, &bytes)?;
        }
    }
    Ok(())
}

/// Every sheet into one PDF, in order, with a bookmark per sheet ("number title") under its subset.
fn publish(path: &Path, set: &SheetSet, p: &Value) -> Result<Value> {
    let out = str_param(p, "path").map(str::trim).filter(|o| !o.is_empty()).ok_or_else(|| bad(CMD, "`path` (the PDF to write) is required"))?;
    let plot = io().and_then(|h| h.plot).ok_or_else(|| bad(CMD, "plotting is not available in this build"))?;
    let merge = pdf_merge().ok_or_else(|| bad(CMD, "publishing to one PDF is not available in this build"))?;
    if set.sheets.is_empty() {
        return Err(bad(CMD, "the sheet set has no sheets"));
    }
    let mut parts = Vec::with_capacity(set.sheets.len());
    let mut cache: Option<(String, cadaddy_doc::Drawing)> = None;
    for sh in &set.sheets {
        if cache.as_ref().is_none_or(|(f, _)| *f != sh.drawing) {
            let file = drawing_file(path, &sh.drawing)?;
            cache = Some((sh.drawing.clone(), read_drawing(&file)?));
        }
        let Some((_, d)) = &cache else { continue };
        let layout = d
            .layout(&sh.layout)
            .map(|l| l.name.clone())
            .ok_or_else(|| bad(CMD, format!("sheet {}: {} has no layout `{}`", sh.number, sh.drawing, sh.layout)))?;
        let bytes = plot(d, &cadaddy_doc::Space::Paper(layout), &json!({})).map_err(|e| bad(CMD, format!("sheet {}: {e}", sh.number)))?;
        let title = format!("{} {}", sh.number, sh.title).trim().to_string();
        parts.push((bytes, title, sh.subset.clone()));
    }
    let pdf = merge(&parts).map_err(|e| bad(CMD, e))?;
    store(out, &pdf)?;
    let mut r = listing(path, set);
    r["path"] = json!(out);
    r["pages"] = json!(parts.len());
    Ok(r)
}
