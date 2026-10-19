//! ETRANSMIT: the drawing with every file it uses, packed as a ZIP archive or into a folder for
//! sending: its xrefs (nested ones too), images, PDF underlays, table data-link files, the plot
//! style tables it keeps and the fonts its text styles use when their licence lets them be passed
//! on (the SIL Open Font License, read from the font itself; others are listed, not copied), with
//! a transmittal report.
//!
//! Paths in the packed drawings are rewritten relative to where each file lands, so the package
//! opens anywhere. A referenced drawing is written again only when one of its own paths changes;
//! otherwise its file is copied as it is. With the `folders` layout (the default) the files go
//! in Xrefs, Images, Underlays, DataLinks, Fonts and PlotStyles folders beside the drawing; with
//! `flat`, all in one folder.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_doc::{Drawing, EntityKind, Handle, MAX_BLOCK_DEPTH};
use serde_json::{Value, json};

use super::{IoHooks, io, store};
use crate::cmd::{CommandSpec, bad, bool_or, has_doc, str_param};
use crate::{Result, Session};

const CMD: &str = "etransmit";
/// Most files one package holds, and most bytes read from referenced files.
const MAX_FILES: usize = 10_000;
const MAX_BYTES: u64 = 2 << 30;
/// Largest single referenced file.
const MAX_FILE_BYTES: u64 = 512 << 20;

pub(super) fn spec() -> CommandSpec {
    CommandSpec::new(CMD, "eTransmit...", run)
        .menu(&["File", "eTransmit..."])
        .params(
            "{path?: \"OUT.zip\" (a ZIP archive) | a folder (created; must be empty), layout?: \"folders\" (default) | \"flat\", report?: bool (default true: transmittal.txt and .html)} → {path?, files: [{target, kind, source, bytes, note}], left: [{kind, name, note}], report}; without `path` it only lists what would be packed",
        )
        .enabled(has_doc)
        .noundo()
}

/// One file of the package.
#[derive(Clone, Debug)]
struct Item {
    /// Path inside the package (`/` separators).
    target: String,
    kind: &'static str,
    /// Where it came from.
    source: String,
    bytes: Vec<u8>,
    note: String,
}

/// Something the drawing uses that isn't in the package, and why.
#[derive(Clone, Debug)]
struct Left {
    kind: &'static str,
    name: String,
    note: String,
}

struct Pack {
    hooks: &'static IoHooks,
    flat: bool,
    items: Vec<Item>,
    left: Vec<Left>,
    /// Source files already packed (canonical path → target), so each goes in once.
    placed: HashMap<PathBuf, String>,
    /// Targets taken (lower case).
    taken: HashSet<String>,
    fonts: BTreeSet<String>,
    tables: BTreeSet<String>,
    bytes: u64,
}

fn folder(kind: &str) -> &'static str {
    match kind {
        "xref" => "Xrefs",
        "image" => "Images",
        "underlay" => "Underlays",
        "data link" => "DataLinks",
        "font" => "Fonts",
        "plot style table" => "PlotStyles",
        _ => "",
    }
}

/// The path from the file at `from` to the file at `to` (both package paths), as drawings store
/// relative paths: `Xrefs\a.dwg`, `..\Images\b.png`.
pub(crate) fn relative(from: &str, to: &str) -> String {
    let mut from_dir: Vec<&str> = from.split('/').collect();
    from_dir.pop();
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir.iter().zip(&to_parts).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
    let mut out: Vec<&str> = std::iter::repeat_n("..", from_dir.len().saturating_sub(common)).collect();
    out.extend(to_parts.iter().skip(common));
    out.join("\\")
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into())
}

/// A name safe as one path component inside the package.
fn clean(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let n = n.trim().trim_matches('.').to_string();
    if n.is_empty() { "file".into() } else { n.chars().take(200).collect() }
}

impl Pack {
    /// A free package path for a file called `name` of `kind`.
    fn place(&mut self, kind: &str, name: &str) -> String {
        let name = clean(name);
        let dir = if self.flat { "" } else { folder(kind) };
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (name.clone(), String::new()),
        };
        let path = |n: &str| if dir.is_empty() { n.to_string() } else { format!("{dir}/{n}") };
        let mut target = path(&name);
        let mut i = 2;
        while self.taken.contains(&target.to_ascii_lowercase()) {
            target = path(&format!("{stem}-{i}{ext}"));
            i += 1;
        }
        self.taken.insert(target.to_ascii_lowercase());
        target
    }

    fn room(&self) -> std::result::Result<(), String> {
        if self.items.len() >= MAX_FILES {
            return Err(format!("more than {MAX_FILES} files"));
        }
        Ok(())
    }

    fn read(&mut self, file: &Path) -> std::result::Result<Vec<u8>, String> {
        let bytes = read_file(file)?;
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if self.bytes > MAX_BYTES {
            return Err(format!("the files are larger than {} MB together", MAX_BYTES >> 20));
        }
        Ok(bytes)
    }

    /// Pack the file at `stored` (a path saved in the drawing at `host`, itself packed at `me`)
    /// as `kind`; returns the path to store instead, or `None` when it wasn't found (listed).
    fn file(&mut self, kind: &'static str, stored: &str, host: Option<&Path>, me: &str) -> std::result::Result<Option<String>, String> {
        let Ok(file) = crate::cmd::blocks::xref::load::find(stored, host, false) else {
            self.left.push(Left { kind, name: stored.to_string(), note: "not found".into() });
            return Ok(None);
        };
        let canon = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        if let Some(t) = self.placed.get(&canon) {
            return Ok(Some(relative(me, t)));
        }
        self.room()?;
        let target = self.place(kind, &file_name(&file));
        self.placed.insert(canon, target.clone());
        let bytes = match self.read(&file) {
            Ok(b) => b,
            Err(e) => {
                self.left.push(Left { kind, name: stored.to_string(), note: format!("couldn't be read: {e}") });
                return Ok(None);
            }
        };
        self.items.push(Item { target: target.clone(), kind, source: file.to_string_lossy().into_owned(), bytes, note: String::new() });
        Ok(Some(relative(me, &target)))
    }

    /// Pack xref `stored` of the drawing at `host` (packed at `me`), with its own references.
    fn xref(&mut self, stored: &str, host: Option<&Path>, me: &str, depth: usize) -> std::result::Result<Option<String>, String> {
        let Ok(file) = crate::cmd::blocks::xref::load::find(stored, host, false) else {
            self.left.push(Left { kind: "xref", name: stored.to_string(), note: "not found".into() });
            return Ok(None);
        };
        let canon = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        // Packed already (or being packed further up: a circular reference).
        if let Some(t) = self.placed.get(&canon) {
            return Ok(Some(relative(me, t)));
        }
        self.room()?;
        let target = self.place("xref", &file_name(&file));
        self.placed.insert(canon, target.clone());
        let bytes = match self.read(&file) {
            Ok(b) => b,
            Err(e) => {
                self.left.push(Left { kind: "xref", name: stored.to_string(), note: format!("couldn't be read: {e}") });
                return Ok(None);
            }
        };
        let source = file.to_string_lossy().into_owned();
        let (bytes, note) = match (self.hooks.read)(&bytes, &source) {
            Ok(mut x) if depth < MAX_BLOCK_DEPTH => {
                if self.drawing(&mut x, Some(&file), &target, depth + 1)? {
                    match (self.hooks.write)(&x, &target) {
                        Ok(b) => (b, "paths rewritten".to_string()),
                        Err(e) => (bytes, format!("copied as it is (couldn't be written again: {e}); its paths are unchanged")),
                    }
                } else {
                    (bytes, String::new())
                }
            }
            Ok(_) => (bytes, "nested too deeply: copied as it is".into()),
            Err(e) => (bytes, format!("copied as it is (couldn't be read as a drawing: {e})")),
        };
        self.items.push(Item { target: target.clone(), kind: "xref", source, bytes, note });
        Ok(Some(relative(me, &target)))
    }

    /// Pack what drawing `d` (saved at `host`, packed at `me`) refers to and rewrite its paths;
    /// whether any path changed.
    fn drawing(&mut self, d: &mut Drawing, host: Option<&Path>, me: &str, depth: usize) -> std::result::Result<bool, String> {
        let mut changed = false;
        // Xrefs (their dependent blocks `x|name` come from the xref itself).
        let xrefs: Vec<(String, String)> =
            d.blocks.values().filter(|b| !b.name.contains('|')).filter_map(|b| b.xref_path.clone().map(|p| (b.name.clone(), p))).collect();
        for (name, stored) in xrefs {
            if let Some(new) = self.xref(&stored, host, me, depth)?
                && new != stored
                && let Some(b) = d.blocks.get_mut(&name)
            {
                Arc::make_mut(b).xref_path = Some(new);
                changed = true;
            }
        }
        // Images, underlays and data links, wherever they are (not in xrefs' blocks: those are
        // the xrefs' own).
        let paths = |k: &EntityKind| -> Option<(&'static str, String)> {
            match k {
                EntityKind::Image(i) if !i.path.trim().is_empty() => Some(("image", i.path.clone())),
                EntityKind::Underlay(u) if !u.path.trim().is_empty() => Some(("underlay", u.path.clone())),
                EntityKind::Table(t) => t.data_link.as_ref().filter(|l| !l.path.trim().is_empty()).map(|l| ("data link", l.path.clone())),
                _ => None,
            }
        };
        let mut found: Vec<(Option<String>, Handle, &'static str, String)> = Vec::new();
        for e in d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())) {
            if let Some((kind, p)) = paths(&e.kind) {
                found.push((None, e.handle, kind, p));
            }
        }
        for b in d.blocks.values().filter(|b| b.xref_path.is_none() && !b.name.contains('|')) {
            for e in b.entities.iter() {
                if let Some((kind, p)) = paths(&e.kind) {
                    found.push((Some(b.name.clone()), e.handle, kind, p));
                }
            }
        }
        let mut done: HashMap<String, Option<String>> = HashMap::new();
        for (block, h, kind, stored) in found {
            let new = match done.get(&stored) {
                Some(n) => n.clone(),
                None => {
                    let n = self.file(kind, &stored, host, me)?;
                    done.insert(stored.clone(), n.clone());
                    n
                }
            };
            let Some(new) = new.filter(|n| *n != stored) else { continue };
            let set = |k: &mut EntityKind| match k {
                EntityKind::Image(i) => i.path = new.clone(),
                EntityKind::Underlay(u) => u.path = new.clone(),
                EntityKind::Table(t) => {
                    if let Some(l) = &mut t.data_link {
                        l.path = new.clone();
                    }
                }
                _ => {}
            };
            match block {
                None => {
                    let _ = d.modify_entity(h, |e| set(&mut e.kind));
                }
                Some(n) => {
                    if let Some(b) = d.blocks.get_mut(&n) {
                        Arc::make_mut(b).entities.modify(h, |e| set(&mut e.kind));
                    }
                }
            }
            changed = true;
        }
        // Fonts of its text styles, and the plot style tables its layouts use.
        let fonts: Vec<String> = d.text_styles.iter().flat_map(|t| [t.font.clone(), t.big_font.clone()]).filter(|f| !f.trim().is_empty()).collect();
        for f in fonts {
            self.font(&f)?;
        }
        let tables: Vec<String> =
            d.layouts.iter().map(|l| l.page.plot_style_table.clone()).filter(|t| !cadaddy_doc::no_plot_style_table(t)).collect();
        for t in tables {
            self.table(d, &t)?;
        }
        Ok(changed)
    }

    fn font(&mut self, name: &str) -> std::result::Result<(), String> {
        let name = name.trim();
        if !self.fonts.insert(name.to_ascii_lowercase()) {
            return Ok(());
        }
        let left = |note: &str| Left { kind: "font", name: name.to_string(), note: note.to_string() };
        if cadaddy_fonts::is_builtin_font(name) {
            self.left.push(left("CADaddy's own stroke font: built into CADaddy"));
            return Ok(());
        }
        if name.to_ascii_lowercase().ends_with(".shx") {
            self.left.push(left("SHX font: not copied (its licence isn't known)"));
            return Ok(());
        }
        let Some(face) = cadaddy_fonts::ttf::find(name) else {
            self.left.push(left("not found on this computer"));
            return Ok(());
        };
        if !face.is_ofl() {
            self.left.push(left("not copied: its licence doesn't say it may be passed on (only SIL Open Font License fonts are)"));
            return Ok(());
        }
        self.room()?;
        let stem = name.rsplit_once('.').filter(|(_, e)| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc")).map_or(name, |(s, _)| s);
        let target = self.place("font", &format!("{stem}.{}", face.file_extension()));
        self.items.push(Item {
            target,
            kind: "font",
            source: name.to_string(),
            bytes: face.bytes.as_ref().clone(),
            note: "SIL Open Font License".into(),
        });
        Ok(())
    }

    fn table(&mut self, d: &Drawing, name: &str) -> std::result::Result<(), String> {
        if !self.tables.insert(name.to_ascii_lowercase()) {
            return Ok(());
        }
        let kept = d.plot_style_tables.iter().find(|t| t.name.eq_ignore_ascii_case(name));
        match (kept, cadaddy_doc::PlotStyleTable::builtin(name)) {
            (Some(t), _) => {
                self.room()?;
                let target = self.place("plot style table", &format!("{}.json", t.name));
                let note = "CADaddy plot style table (STYLESMANAGER Load); the drawing keeps it too".to_string();
                self.items.push(Item { target, kind: "plot style table", source: t.name.clone(), bytes: t.to_file_text().into_bytes(), note });
            }
            (None, Some(_)) => self.left.push(Left { kind: "plot style table", name: name.into(), note: "built into CADaddy".into() }),
            (None, None) => self.left.push(Left { kind: "plot style table", name: name.into(), note: "not found".into() }),
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_file(file: &Path) -> std::result::Result<Vec<u8>, String> {
    use std::io::Read;
    let f = crate::readfile::open(file).map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = Vec::new();
    f.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("larger than {} MB", MAX_FILE_BYTES >> 20));
    }
    Ok(bytes)
}
#[cfg(target_arch = "wasm32")]
fn read_file(_file: &Path) -> std::result::Result<Vec<u8>, String> {
    Err("files are not available on the web".into())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The transmittal report as text and as HTML.
fn reports(main: &str, items: &[Item], left: &[Left]) -> (String, String) {
    let mut t = format!("Transmittal report: {main}\nPacked by CADaddy eTransmit.\n\nFiles ({}):\n", items.len());
    for i in items {
        t.push_str(&format!(
            "  {}  [{}]  from {}{}\n",
            i.target,
            i.kind,
            i.source,
            if i.note.is_empty() { String::new() } else { format!(" ({})", i.note) }
        ));
    }
    if !left.is_empty() {
        t.push_str(&format!("\nNot included ({}):\n", left.len()));
        for l in left {
            t.push_str(&format!("  [{}] {}: {}\n", l.kind, l.name, l.note));
        }
    }
    let row = |cells: &[&str]| format!("<tr>{}</tr>", cells.iter().map(|c| format!("<td>{}</td>", html_escape(c))).collect::<String>());
    let mut h = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Transmittal: {0}</title></head><body>\n<h1>Transmittal report: {0}</h1>\n<p>Packed by CADaddy eTransmit.</p>\n<h2>Files</h2>\n<table border=\"1\" cellpadding=\"4\"><tr><th>File</th><th>Kind</th><th>From</th><th>Note</th></tr>\n",
        html_escape(main)
    );
    for i in items {
        h.push_str(&row(&[&i.target, i.kind, &i.source, &i.note]));
        h.push('\n');
    }
    h.push_str("</table>\n");
    if !left.is_empty() {
        h.push_str("<h2>Not included</h2>\n<table border=\"1\" cellpadding=\"4\"><tr><th>Kind</th><th>Name</th><th>Why</th></tr>\n");
        for l in left {
            h.push_str(&row(&[l.kind, &l.name, &l.note]));
            h.push('\n');
        }
        h.push_str("</table>\n");
    }
    h.push_str("</body></html>\n");
    (t, h)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let flat = match str_param(p, "layout").map(str::to_ascii_lowercase).as_deref() {
        None | Some("folders") => false,
        Some("flat") => true,
        Some(o) => return Err(bad(CMD, format!("`layout` is \"folders\" or \"flat\", not \"{o}\""))),
    };
    let with_report = bool_or(p, "report", true);
    let out = str_param(p, "path").map(str::trim).filter(|o| !o.is_empty()).map(str::to_string);
    let st = s.state()?;
    let host = st.path.clone();
    let main = match host.as_deref().map(Path::new).and_then(|h| h.file_name()) {
        Some(n) => clean(&n.to_string_lossy()),
        None => format!("{}.dwg", clean(st.title.trim_end_matches(".dwg").trim_end_matches(".dxf"))),
    };
    let mut d = st.doc.committed().into_owned();
    let mut pack = Pack {
        hooks,
        flat,
        items: Vec::new(),
        left: Vec::new(),
        placed: HashMap::new(),
        taken: HashSet::new(),
        fonts: BTreeSet::new(),
        tables: BTreeSet::new(),
        bytes: 0,
    };
    pack.taken.insert(main.to_ascii_lowercase());
    for r in ["transmittal.txt", "transmittal.html"] {
        pack.taken.insert(r.into());
    }
    if let Some(h) = host.as_deref() {
        let canon = std::fs::canonicalize(h).unwrap_or_else(|_| PathBuf::from(h));
        pack.placed.insert(canon, main.clone());
    }
    let host_path = host.as_deref().map(Path::new);
    pack.drawing(&mut d, host_path, &main, 0).map_err(|e| bad(CMD, e))?;
    let bytes = (hooks.write)(&d, &main).map_err(|e| bad(CMD, format!("{main}: {e}")))?;
    let mut items = vec![Item {
        target: main.clone(),
        kind: "drawing",
        source: host.clone().unwrap_or_else(|| "this drawing".into()),
        bytes,
        note: String::new(),
    }];
    items.append(&mut pack.items);
    let (text, html) = reports(&main, &items, &pack.left);
    if with_report {
        for (name, body) in [("transmittal.txt", &text), ("transmittal.html", &html)] {
            items.push(Item {
                target: name.into(),
                kind: "report",
                source: "eTransmit".into(),
                bytes: body.clone().into_bytes(),
                note: String::new(),
            });
        }
    }
    let files: Vec<Value> =
        items.iter().map(|i| json!({ "target": i.target, "kind": i.kind, "source": i.source, "bytes": i.bytes.len(), "note": i.note })).collect();
    let left: Vec<Value> = pack.left.iter().map(|l| json!({ "kind": l.kind, "name": l.name, "note": l.note })).collect();
    let mut r = json!({ "files": files, "left": left, "report": text });
    let Some(out) = out else { return Ok(r) };
    if out.to_ascii_lowercase().ends_with(".zip") {
        let entries: Vec<(String, Vec<u8>)> = items.into_iter().map(|i| (i.target, i.bytes)).collect();
        let zip = super::zip::write(&entries).map_err(|e| bad(CMD, e))?;
        store(&out, &zip).map_err(|e| bad(CMD, e.to_string()))?;
    } else {
        write_folder(&out, &items)?;
    }
    r["path"] = json!(out);
    Ok(r)
}

#[cfg(not(target_arch = "wasm32"))]
fn write_folder(out: &str, items: &[Item]) -> Result<()> {
    let root = Path::new(out);
    if root.exists() && std::fs::read_dir(root).map_or(true, |mut d| d.next().is_some()) {
        return Err(bad(CMD, format!("{out} isn't an empty folder; choose a new one or a .zip file")));
    }
    for i in items {
        let path = i.target.split('/').fold(root.to_path_buf(), |p, c| p.join(c));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(CMD, format!("{}: {e}", dir.display())))?;
        }
        crate::writefile::write(&path, &i.bytes).map_err(|e| bad(CMD, format!("{}: {e}", path.display())))?;
    }
    Ok(())
}
#[cfg(target_arch = "wasm32")]
fn write_folder(_out: &str, _items: &[Item]) -> Result<()> {
    Err(bad(CMD, "the web has no folders: choose a .zip file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_between_package_files() {
        assert_eq!(relative("host.dwg", "Xrefs/a.dwg"), "Xrefs\\a.dwg");
        assert_eq!(relative("Xrefs/a.dwg", "Xrefs/b.dwg"), "b.dwg");
        assert_eq!(relative("Xrefs/a.dwg", "Images/p.png"), "..\\Images\\p.png");
        assert_eq!(relative("a.dwg", "b.png"), "b.png");
        assert_eq!(clean("..\\a:b?.dwg"), "_a_b_.dwg");
    }
}
