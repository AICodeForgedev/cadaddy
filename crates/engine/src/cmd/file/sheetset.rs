//! Sheet sets: named, numbered sheets (layouts of drawings) in subsets, kept in our own JSON file
//! (`*.sheetset.json`), published together to one PDF with a bookmark per sheet.
//!
//! The file: `{"cadaddySheetSet": 1, "name", "description", "properties": {name: value},
//! "subsets": [names, in order], "sheets": [{"id", "number", "title", "description", "drawing",
//! "layout", "subset", "properties"}], "pageSetups": {name: page setup}, "pageSetup": name used
//! for publishing ("" each layout's own), "subsetPageSetups": {subset: name}, "views": [{"id",
//! "number", "title", "sheet" (id), "view", "viewport", "label"}], "callouts": [{"sheet" (id),
//! "view" (id), "handle"}], "nextId"}`. A sheet's `drawing` is relative to the sheet set's file
//! when it is in or below its folder, else absolute; `subset` is "" for a sheet in no subset.
//! Sheets and views keep their ids when renumbered, so callouts and views follow them.
//!
//! Every action names the file (`set`) and reads it, changes it and writes it again, so nothing
//! is held between commands. Sheet numbers and titles reach a drawing's fields through its layout
//! names: `sync` renames each sheet's layout to "number title", which the CurrentSheetNumber /
//! CurrentSheetTitle fields read, and writes the set's and its sheets' names, descriptions and
//! custom properties into each drawing (its `CADADDY_SHEETSET` header data), which the `\AcSm SheetSet.…`
//! and `\AcSm Sheet.…` fields read; it also puts each view's current number and title in its label
//! and each callout's view and sheet numbers in it. AutoCAD's `.dst` sheet set files are a binary format of their
//! own and are not read.

use std::collections::BTreeMap;
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

/// The header variable holding a drawing's sheet set data (JSON: the set's name, description and
/// properties, its sheets' by layout name), which the `\AcSm` fields read.
pub const SHEET_SET_VAR: &str = "CADADDY_SHEETSET";

/// Custom properties: name to value.
pub type Properties = BTreeMap<String, String>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sheet {
    /// Kept through renumbering (0 in files from before ids: given one when read).
    pub id: u64,
    pub number: String,
    pub title: String,
    pub description: String,
    pub drawing: String,
    pub layout: String,
    pub subset: String,
    pub properties: Properties,
}

/// A named view of a sheet's drawing placed on the sheet: its viewport and label.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SheetView {
    pub id: u64,
    pub number: String,
    pub title: String,
    /// The sheet it is on (id).
    pub sheet: u64,
    /// The drawing's named view it shows.
    pub view: String,
    /// The viewport and the label (handles in the sheet's drawing).
    pub viewport: String,
    pub label: String,
}

/// A callout on a sheet pointing at a view: it shows the view's number over its sheet's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Callout {
    /// The sheet it is on and the view it points at (ids).
    pub sheet: u64,
    pub view: u64,
    /// The text (a handle in the sheet's drawing).
    pub handle: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SheetSet {
    #[serde(rename = "cadaddySheetSet")]
    pub format: u32,
    pub name: String,
    pub description: String,
    pub properties: Properties,
    pub subsets: Vec<String>,
    pub sheets: Vec<Sheet>,
    /// Named page setups for publishing, and the one used ("" each layout's own) for every sheet
    /// or a subset's sheets.
    pub page_setups: BTreeMap<String, cadaddy_doc::PageSetup>,
    pub page_setup: String,
    pub subset_page_setups: BTreeMap<String, String>,
    pub views: Vec<SheetView>,
    pub callouts: Vec<Callout>,
    pub next_id: u64,
}

impl SheetSet {
    fn new_id(&mut self) -> u64 {
        let used = self.sheets.iter().map(|s| s.id).chain(self.views.iter().map(|v| v.id)).max().unwrap_or(0);
        self.next_id = self.next_id.max(used).saturating_add(1);
        self.next_id
    }
    /// Ids for sheets that have none (files from before ids).
    fn give_ids(&mut self) {
        for i in 0..self.sheets.len() {
            if self.sheets.get(i).is_some_and(|s| s.id == 0) {
                let id = self.new_id();
                if let Some(s) = self.sheets.get_mut(i) {
                    s.id = id;
                }
            }
        }
    }
    fn sheet(&self, id: u64) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.id == id)
    }
    /// The page setup a sheet is published with, when one overrides its layout's.
    fn override_for(&self, sheet: &Sheet) -> Option<&cadaddy_doc::PageSetup> {
        let name = self.subset_page_setups.get(&sheet.subset).filter(|n| !n.is_empty()).unwrap_or(&self.page_setup);
        self.page_setups.get(name)
    }
}

/// Most custom properties, views and callouts in a set.
const MAX_ITEMS: usize = 10_000;

pub(super) fn spec() -> CommandSpec {
    CommandSpec::new(CMD, "Sheet Set Manager", run)
        .menu(&["Tools", "Palettes", "Sheet Set Manager"])
        .alias(&["ssm"])
        .params(
            "{set: path of the sheet set file (.sheetset.json), action: new {name?, description?, overwrite?} | list | add {drawing | drawings: [paths], layouts?: [names] (default every layout, in tab order), subset?, number?, title?} \
             | remove {sheets: [index | number]} | set {sheet: index | number, number?, title?, subset?} | move {sheet, to: index} | renumber {start? (1), step? (1), prefix?, digits?, subset? (only those)} \
             | subset {add?: name, remove?: name (its sheets leave it), rename?: [old, new]} | sync (rename each sheet's layout to \"number title\" in its drawing, for the sheet fields) \
             | publish {path: OUT.pdf} (one PDF, a bookmark per sheet under its subset; a page setup override applies) \
             | properties {sheet?, values: {name: value | null}, description?} (custom properties of the set, or of a sheet) \
             | pagesetup {name, page?: page setup fields | from?: sheet (its layout's), remove?: name, use?: name (\"\" each layout's own), subset?} \
             | view {sheet, view: the drawing's named view, at: [x, y] on the sheet, scale? (paper per drawing unit, 1), number?, title?, height? (label text, 3.5)} \
             | callout {sheet (where it goes), view: view number or id, at: [x, y], height?} | fields (the field codes the set offers) \
             | insertField {sheet, field: e.g. SheetSet.Name, Sheet.Number, Sheet.Client, at: [x, y], height?}} → the set's listing (and {path, pages} for publish). AutoCAD .dst files are not read.",
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

/// What a new sheet set file is called: `<name>.sheetset.json`, whatever a file dialog added
/// (a drawing's `.cdy`, a bare `.json`) or left off.
pub(crate) fn new_set_path(path: &Path) -> PathBuf {
    let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{}{SET_EXT}", set_name(Path::new(&file))))
}

/// A sheet set's name from its file: the file name without `.sheetset.json` (or the drawing or
/// JSON extension a file dialog put on it).
pub(crate) fn set_name(path: &Path) -> String {
    let mut name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    loop {
        let lower = name.to_ascii_lowercase();
        let Some(ext) = [".json", ".sheetset", ".cdy", ".dwg", ".dxf", ".dst"].iter().find(|e| lower.ends_with(*e)) else { break };
        name.truncate(name.len() - ext.len());
    }
    let name = name.trim();
    if name.is_empty() { "Sheets".to_string() } else { name.to_string() }
}

/// A sheet set file's extension.
pub(crate) const SET_EXT: &str = ".sheetset.json";

#[cfg(not(target_arch = "wasm32"))]
fn load(path: &Path) -> Result<SheetSet> {
    use std::io::Read;
    let f = crate::readfile::open(path).map_err(|e| bad(CMD, format!("{}: {e}", path.display())))?;
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
    if set.views.len() > MAX_ITEMS || set.callouts.len() > MAX_ITEMS || set.properties.len() > MAX_ITEMS || set.page_setups.len() > MAX_ITEMS {
        return Err(bad(CMD, format!("{}: more than {MAX_ITEMS} views, callouts, properties or page setups", path.display())));
    }
    let mut set = set;
    set.give_ids();
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
    crate::writefile::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
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
        let f = crate::readfile::open(file).map_err(|e| bad(CMD, format!("{}: {e}", file.display())))?;
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
            let found = drawing_file(path, &sh.drawing).is_ok_and(|f| cadaddy_geom::filename::find_file(f).is_some());
            json!({
                "index": i, "id": sh.id, "number": sh.number, "title": sh.title, "description": sh.description, "drawing": sh.drawing,
                "layout": sh.layout, "subset": sh.subset, "properties": sh.properties, "found": found
            })
        })
        .collect();
    let number_of = |id: u64| set.sheet(id).map(|s| s.number.clone()).unwrap_or_default();
    let views: Vec<Value> =
        set.views.iter().map(|v| json!({ "id": v.id, "number": v.number, "title": v.title, "sheet": number_of(v.sheet), "view": v.view })).collect();
    let callouts: Vec<Value> = set
        .callouts
        .iter()
        .map(|c| {
            let view = set.views.iter().find(|v| v.id == c.view);
            json!({ "sheet": number_of(c.sheet), "view": view.map(|v| v.number.clone()), "target": view.map(|v| number_of(v.sheet)), "handle": c.handle })
        })
        .collect();
    json!({
        "set": path.to_string_lossy(), "name": set.name, "description": set.description, "properties": set.properties,
        "subsets": set.subsets, "sheets": sheets, "pageSetups": set.page_setups.keys().collect::<Vec<_>>(), "pageSetup": set.page_setup,
        "subsetPageSetups": set.subset_page_setups, "views": views, "callouts": callouts
    })
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let path = set_path(p)?;
    let action = str_param(p, "action").unwrap_or("list").to_ascii_lowercase();
    if action == "new" {
        let path = new_set_path(&path);
        if path.exists() && !bool_or(p, "overwrite", false) {
            return Err(bad(CMD, format!("{} already exists (overwrite: true replaces it)", path.display())));
        }
        let name = text(p, "name").filter(|n| !n.is_empty()).unwrap_or_else(|| set_name(&path));
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
                let gone = set.sheets.remove(i).id;
                // Its views go too, and the callouts on it or pointing at them.
                let views: Vec<u64> = set.views.iter().filter(|v| v.sheet == gone).map(|v| v.id).collect();
                set.views.retain(|v| v.sheet != gone);
                set.callouts.retain(|c| c.sheet != gone && !views.contains(&c.view));
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
                if let Some(t) = text(p, "description") {
                    sh.description = t;
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
        "publish" => return publish(s, &path, &set, p),
        "properties" => properties(&mut set, p)?,
        "pagesetup" => page_setup(&path, &mut set, p)?,
        "view" => place_view(&path, &mut set, p)?,
        "callout" => callout(&path, &mut set, p)?,
        "fields" => return Ok(json!({ "fields": field_names(&set) })),
        "insertfield" | "insert_field" => insert_field(&path, &set, p)?,
        other => {
            return Err(bad(
                CMD,
                format!(
                    "unknown action \"{other}\" (new, list, add, remove, set, move, renumber, subset, sync, publish, properties, pagesetup, view, callout, fields, insertField)"
                ),
            ));
        }
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
            let id = set.new_id();
            set.sheets.push(Sheet { id, number, title, drawing: stored_path(path, &file), layout, subset: subset.clone(), ..Default::default() });
        }
    }
    Ok(())
}

/// Rename each sheet's layout to "number title" in its drawing (written again, its previous
/// version kept as .bak), so the drawing's sheet number and title fields show them; write the
/// set's data for the `\AcSm` fields into each drawing; put the current numbers and titles in the
/// views' labels and the callouts.
fn sync(path: &Path, set: &mut SheetSet) -> Result<()> {
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let mut files: Vec<String> = set.sheets.iter().map(|s| s.drawing.clone()).collect();
    files.sort();
    files.dedup();
    for stored in files {
        let file = drawing_file(path, &stored)?;
        let mut d = read_drawing(&file)?;
        let before = d.clone();
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
        }
        d.header.set_str(SHEET_SET_VAR, &sheet_data(set, &stored).to_string());
        // Labels and callouts in this drawing.
        let number_of = |id: u64| set.sheet(id).map(|s| s.number.clone()).unwrap_or_default();
        for v in set.views.iter().filter(|v| set.sheet(v.sheet).is_some_and(|s| s.drawing == stored)) {
            let label = format!("{} {}", v.number, v.title).trim().to_string();
            set_text(&mut d, &v.label, &label);
        }
        for c in set.callouts.iter().filter(|c| set.sheet(c.sheet).is_some_and(|s| s.drawing == stored)) {
            if let Some(v) = set.views.iter().find(|v| v.id == c.view) {
                set_text(&mut d, &c.handle, &format!("{}/{}", v.number, number_of(v.sheet)));
            }
        }
        if d != before {
            let name = file.to_string_lossy().into_owned();
            let bytes = (hooks.write)(&d, &name).map_err(|e| bad(CMD, format!("{name}: {e}")))?;
            store(&name, &bytes)?;
        }
    }
    Ok(())
}

/// What the `\AcSm` fields of the drawing `stored` read: the set's name, description and
/// properties, and each of its sheets' (by layout name).
fn sheet_data(set: &SheetSet, stored: &str) -> Value {
    let sheets: serde_json::Map<String, Value> = set
        .sheets
        .iter()
        .filter(|s| s.drawing == stored)
        .map(|s| (s.layout.clone(), json!({ "number": s.number, "title": s.title, "description": s.description, "properties": s.properties })))
        .collect();
    json!({ "name": set.name, "description": set.description, "properties": set.properties, "sheets": sheets })
}

/// Set the contents of the MTEXT (or text) with handle `hex`, when there is one.
fn set_text(d: &mut cadaddy_doc::Drawing, hex: &str, text: &str) {
    let Ok(h) = u64::from_str_radix(hex.trim(), 16) else { return };
    let _ = d.modify_entity(cadaddy_doc::Handle(h), |e| match &mut e.kind {
        cadaddy_doc::EntityKind::MText(m) => m.contents = text.to_string(),
        cadaddy_doc::EntityKind::Text(t) => t.value = text.to_string(),
        _ => {}
    });
}

/// Read the drawing of `sheet`, change it with `f` and write it again (its previous version kept
/// as .bak).
fn edit_drawing<R>(path: &Path, sheet: &Sheet, f: impl FnOnce(&mut cadaddy_doc::Drawing, &str) -> Result<R>) -> Result<R> {
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let file = drawing_file(path, &sheet.drawing)?;
    let mut d = read_drawing(&file)?;
    let layout =
        d.layout(&sheet.layout).map(|l| l.name.clone()).ok_or_else(|| bad(CMD, format!("{}: no layout `{}`", sheet.drawing, sheet.layout)))?;
    let r = f(&mut d, &layout)?;
    let name = file.to_string_lossy().into_owned();
    let bytes = (hooks.write)(&d, &name).map_err(|e| bad(CMD, format!("{name}: {e}")))?;
    store(&name, &bytes)?;
    Ok(r)
}

fn point(p: &Value, k: &str) -> Result<cadaddy_geom::Vec3> {
    let a = p.get(k).and_then(Value::as_array).ok_or_else(|| bad(CMD, format!("`{k}` ([x, y] on the sheet) is required")))?;
    let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|v| v.is_finite() && v.abs() < 1e9);
    match (c(0), c(1)) {
        (Some(x), Some(y)) => Ok(cadaddy_geom::Vec3::new(x, y, 0.0)),
        _ => Err(bad(CMD, format!("`{k}` is [x, y]"))),
    }
}

fn height(p: &Value) -> f64 {
    p.get("height").and_then(Value::as_f64).filter(|h| h.is_finite() && *h > 0.0 && *h < 1e6).unwrap_or(3.5)
}

/// An MTEXT on the sheet's layout.
fn add_text(d: &mut cadaddy_doc::Drawing, layout: &str, at: cadaddy_geom::Vec3, height: f64, contents: &str) -> Result<cadaddy_doc::Handle> {
    let m = cadaddy_doc::MText {
        insert: at,
        height,
        width: 0.0,
        attach: 1,
        rotation: 0.0,
        style: "Standard".into(),
        contents: contents.to_string(),
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
        background: None,
    };
    d.add(&cadaddy_doc::Space::Paper(layout.to_string()), cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::MText(m))
        .map_err(|e| bad(CMD, e.to_string()))
}

/// Set or clear custom properties (a value of null removes one) of the set or a sheet, and a
/// sheet's description.
fn properties(set: &mut SheetSet, p: &Value) -> Result<()> {
    let values = p.get("values").and_then(Value::as_object);
    let target: &mut Properties = match p.get("sheet") {
        Some(v) => {
            let i = sheet_index(set, Some(v))?;
            let sh = set.sheets.get_mut(i).ok_or_else(|| bad(CMD, "no such sheet"))?;
            if let Some(t) = text(p, "description") {
                sh.description = t;
            }
            &mut sh.properties
        }
        None => {
            if let Some(t) = text(p, "description") {
                set.description = t;
            }
            &mut set.properties
        }
    };
    for (k, v) in values.into_iter().flatten() {
        let k: String = k.trim().chars().take(64).collect();
        if k.is_empty() || k.contains(['.', '%', '<', '>']) {
            return Err(bad(CMD, format!("\"{k}\" can't be a property name (no dots, % or angle brackets)")));
        }
        match v {
            Value::Null => {
                target.remove(&k);
            }
            other => {
                if target.len() >= MAX_ITEMS {
                    return Err(bad(CMD, format!("at most {MAX_ITEMS} properties")));
                }
                let v = other.as_str().map_or_else(|| other.to_string(), str::to_string);
                target.insert(k, v.chars().take(1000).collect());
            }
        }
    }
    Ok(())
}

/// Named page setups for publishing: add one (its fields, or a sheet's layout's), remove one,
/// or choose the one used for every sheet or a subset's.
fn page_setup(path: &Path, set: &mut SheetSet, p: &Value) -> Result<()> {
    if let Some(r) = text(p, "remove") {
        set.page_setups.remove(&r);
        if set.page_setup == r {
            set.page_setup.clear();
        }
        set.subset_page_setups.retain(|_, n| *n != r);
    }
    if let Some(name) = text(p, "name").filter(|n| !n.is_empty()) {
        let page = match (p.get("page"), p.get("from")) {
            (Some(v @ Value::Object(_)), _) => {
                let mut base = serde_json::to_value(cadaddy_doc::PageSetup::default()).map_err(|e| bad(CMD, e.to_string()))?;
                if let (Some(b), Some(o)) = (base.as_object_mut(), v.as_object()) {
                    for (k, x) in o {
                        b.insert(k.clone(), x.clone());
                    }
                }
                serde_json::from_value::<cadaddy_doc::PageSetup>(base).map_err(|e| bad(CMD, format!("`page`: {e}")))?
            }
            (_, Some(v)) => {
                let i = sheet_index(set, Some(v))?;
                let sh = set.sheets.get(i).cloned().ok_or_else(|| bad(CMD, "no such sheet"))?;
                let d = read_drawing(&drawing_file(path, &sh.drawing)?)?;
                d.layout(&sh.layout).map(|l| l.page.clone()).ok_or_else(|| bad(CMD, format!("{}: no layout `{}`", sh.drawing, sh.layout)))?
            }
            _ => return Err(bad(CMD, "a page setup needs `page` (its fields) or `from` (a sheet whose layout's it copies)")),
        };
        if !(page.width_mm.is_finite() && page.height_mm.is_finite() && page.width_mm > 0.0 && page.height_mm > 0.0) {
            return Err(bad(CMD, "a page setup's paper needs a size"));
        }
        if set.page_setups.len() >= MAX_ITEMS && !set.page_setups.contains_key(&name) {
            return Err(bad(CMD, format!("at most {MAX_ITEMS} page setups")));
        }
        set.page_setups.insert(name, page);
    }
    if let Some(u) = text(p, "use") {
        if !u.is_empty() && !set.page_setups.contains_key(&u) {
            return Err(bad(CMD, format!("no page setup \"{u}\" in the set")));
        }
        match text(p, "subset").filter(|s| !s.is_empty()) {
            Some(sub) => {
                if u.is_empty() {
                    set.subset_page_setups.remove(&sub);
                } else {
                    set.subset_page_setups.insert(sub, u);
                }
            }
            None => set.page_setup = u,
        }
    }
    Ok(())
}

/// Place a named view of a sheet's drawing on the sheet: a viewport showing it, and a label
/// under it with the view's number and title.
fn place_view(path: &Path, set: &mut SheetSet, p: &Value) -> Result<()> {
    if set.views.len() >= MAX_ITEMS {
        return Err(bad(CMD, format!("at most {MAX_ITEMS} views")));
    }
    let i = sheet_index(set, p.get("sheet"))?;
    let sheet = set.sheets.get(i).cloned().ok_or_else(|| bad(CMD, "no such sheet"))?;
    let view = text(p, "view").filter(|v| !v.is_empty()).ok_or_else(|| bad(CMD, "`view` (a named view of the sheet's drawing) is required"))?;
    let at = point(p, "at")?;
    let scale = p.get("scale").and_then(Value::as_f64).filter(|s| s.is_finite() && *s > 0.0 && *s < 1e6).unwrap_or(1.0);
    let id = set.new_id();
    let number =
        text(p, "number").filter(|n| !n.is_empty()).unwrap_or_else(|| (set.views.iter().filter(|v| v.sheet == sheet.id).count() + 1).to_string());
    let title = text(p, "title").filter(|t| !t.is_empty()).unwrap_or_else(|| view.clone());
    let label_text = format!("{number} {title}").trim().to_string();
    let h = height(p);
    let (viewport, label) = edit_drawing(path, &sheet, |d, layout| {
        let nv = d
            .views
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(&view) && v.layout.is_none())
            .cloned()
            .ok_or_else(|| bad(CMD, format!("{}: no model space view `{view}`", sheet.drawing)))?;
        let (w, vh) = (nv.width * scale, nv.height * scale);
        if !(w.is_finite() && vh.is_finite() && w > 0.0 && vh > 0.0) {
            return Err(bad(CMD, format!("view `{view}` has no size")));
        }
        let next = d
            .space(&cadaddy_doc::Space::Paper(layout.to_string()))
            .map(|st| st.iter().filter_map(|e| if let cadaddy_doc::EntityKind::Viewport(v) = &e.kind { Some(v.id) } else { None }).max().unwrap_or(1))
            .unwrap_or(1);
        let vp = cadaddy_doc::Viewport {
            center: at,
            width: w,
            height: vh,
            view_center: nv.center,
            view_height: nv.height,
            id: next + 1,
            ..Default::default()
        };
        let vh_ = d
            .add(&cadaddy_doc::Space::Paper(layout.to_string()), cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Viewport(vp))
            .map_err(|e| bad(CMD, e.to_string()))?;
        let under = cadaddy_geom::Vec3::new(at.x - w / 2.0, at.y - vh / 2.0 - h * 0.5, 0.0);
        let lh = add_text(d, layout, under, h, &label_text)?;
        Ok((vh_.hex(), lh.hex()))
    })?;
    set.views.push(SheetView { id, number, title, sheet: sheet.id, view, viewport, label });
    Ok(())
}

/// Put a callout on a sheet pointing at a view: it shows "view number/sheet number" (kept up to
/// date by sync).
fn callout(path: &Path, set: &mut SheetSet, p: &Value) -> Result<()> {
    if set.callouts.len() >= MAX_ITEMS {
        return Err(bad(CMD, format!("at most {MAX_ITEMS} callouts")));
    }
    let i = sheet_index(set, p.get("sheet"))?;
    let sheet = set.sheets.get(i).cloned().ok_or_else(|| bad(CMD, "no such sheet"))?;
    let view = match p.get("view") {
        Some(Value::Number(n)) => set.views.iter().find(|v| Some(v.id) == n.as_u64()),
        Some(Value::String(s)) => {
            // A view number, on its own or as "number/sheet".
            let (vn, sn) = s.split_once('/').map_or((s.trim(), None), |(a, b)| (a.trim(), Some(b.trim())));
            set.views.iter().find(|v| v.number == vn && sn.is_none_or(|sn| set.sheet(v.sheet).is_some_and(|s| s.number == sn)))
        }
        _ => None,
    }
    .cloned()
    .ok_or_else(|| bad(CMD, "`view` (a view's number, \"number/sheet\", or id) is required"))?;
    let at = point(p, "at")?;
    let target = set.sheet(view.sheet).map(|s| s.number.clone()).unwrap_or_default();
    let h = height(p);
    let handle = edit_drawing(path, &sheet, |d, layout| Ok(add_text(d, layout, at, h, &format!("{}/{}", view.number, target))?.hex()))?;
    set.callouts.push(Callout { sheet: sheet.id, view: view.id, handle });
    Ok(())
}

/// The fields the set offers: the set's name, description and properties, the sheets' number,
/// title, description and properties.
fn field_names(set: &SheetSet) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut push = |name: String| {
        let code = format!("%<\\AcSm {name}>%");
        out.push(json!({ "name": name, "code": code }));
    };
    for n in ["SheetSet.Name", "SheetSet.Description"] {
        push(n.to_string());
    }
    for k in set.properties.keys() {
        push(format!("SheetSet.{k}"));
    }
    for n in ["Sheet.Number", "Sheet.Title", "Sheet.Description"] {
        push(n.to_string());
    }
    let mut sheet_props: Vec<&String> = set.sheets.iter().flat_map(|s| s.properties.keys()).collect();
    sheet_props.sort();
    sheet_props.dedup();
    for k in sheet_props {
        push(format!("Sheet.{k}"));
    }
    out
}

/// Put one of the set's fields on a sheet (an MTEXT holding the field code).
fn insert_field(path: &Path, set: &SheetSet, p: &Value) -> Result<()> {
    let i = sheet_index(set, p.get("sheet"))?;
    let sheet = set.sheets.get(i).cloned().ok_or_else(|| bad(CMD, "no such sheet"))?;
    let field = text(p, "field").filter(|f| !f.is_empty()).ok_or_else(|| bad(CMD, "`field` (e.g. SheetSet.Name or Sheet.Number) is required"))?;
    if !field_names(set).iter().any(|f| f["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(&field))) {
        return Err(bad(CMD, format!("the set has no field \"{field}\" (action fields lists them)")));
    }
    let at = point(p, "at")?;
    let h = height(p);
    edit_drawing(path, &sheet, |d, layout| add_text(d, layout, at, h, &format!("%<\\AcSm {field}>%")).map(|_| ()))
}

/// Every sheet into one PDF, in order, with a bookmark per sheet ("number title") under its subset.
/// A layout not set up yet is set up for the plot (its viewport zoomed to the drawing's extents;
/// the drawing's file is left as it is), and a sheet that would still plot blank is warned about
/// on the command line and in the result's `warnings`.
fn publish(s: &mut Session, path: &Path, set: &SheetSet, p: &Value) -> Result<Value> {
    let out = str_param(p, "path").map(str::trim).filter(|o| !o.is_empty()).ok_or_else(|| bad(CMD, "`path` (the PDF to write) is required"))?;
    let plot = io().and_then(|h| h.plot).ok_or_else(|| bad(CMD, "plotting is not available in this build"))?;
    let merge = pdf_merge().ok_or_else(|| bad(CMD, "publishing to one PDF is not available in this build"))?;
    if set.sheets.is_empty() {
        return Err(bad(CMD, "the sheet set has no sheets"));
    }
    let mut parts = Vec::with_capacity(set.sheets.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut cache: Option<(String, cadaddy_doc::Drawing)> = None;
    for sh in &set.sheets {
        if cache.as_ref().is_none_or(|(f, _)| *f != sh.drawing) {
            let file = drawing_file(path, &sh.drawing)?;
            let mut d = read_drawing(&file)?;
            crate::cmd::layout::mark_bare_layouts(&mut d);
            cache = Some((sh.drawing.clone(), d));
        }
        let Some((_, d)) = &mut cache else { continue };
        let layout = d
            .layout(&sh.layout)
            .map(|l| l.name.clone())
            .ok_or_else(|| bad(CMD, format!("sheet {}: {} has no layout `{}`", sh.number, sh.drawing, sh.layout)))?;
        crate::cmd::layout::set_up_layout(d, &layout);
        // The set's page setup over the layout's own, when one applies.
        let overridden = set.override_for(sh).map(|page| {
            let mut d = d.clone();
            if let Some(l) = d.layouts.iter_mut().find(|l| l.name == layout) {
                l.page = page.clone();
            }
            d
        });
        let d = overridden.as_ref().unwrap_or(d);
        let title = format!("{} {}", sh.number, sh.title).trim().to_string();
        if crate::cmd::layout::sheet_is_blank(d, &layout) {
            warnings.push(format!("Sheet {title} ({}, {layout}) has nothing on it: its page is blank.", sh.drawing));
        }
        let bytes = plot(d, &cadaddy_doc::Space::Paper(layout), &json!({})).map_err(|e| bad(CMD, format!("sheet {}: {e}", sh.number)))?;
        parts.push((bytes, title, sh.subset.clone()));
    }
    for w in &warnings {
        s.echo(format!("Warning: {w}"));
    }
    let pdf = merge(&parts).map_err(|e| bad(CMD, e))?;
    store(out, &pdf)?;
    let mut r = listing(path, set);
    r["path"] = json!(out);
    r["pages"] = json!(parts.len());
    r["warnings"] = json!(warnings);
    Ok(r)
}
