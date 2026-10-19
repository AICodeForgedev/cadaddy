//! File menu: new, close, switching drawings. Open/save of .cdy, DWG and DXF go through the io layer,
//! registered by the host (desktop/CLI) via [`crate::cmd::file::set_io`]. On the web, where there is
//! no file system, saved files reach the user through the host's [`crate::cmd::file::set_deliver`].

mod etransmit;
pub mod kept;
mod pgp;
mod sheetset;
mod template;

use std::sync::OnceLock;

use cadaddy_doc::Drawing;
pub use cadaddy_dxf::Version;
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Prompt, Result, Session, Step};

pub use pgp::{built_in as built_in_aliases, user_aliases};
pub(crate) use pgp::{specs as alias_specs, user_alias};
pub use template::{qnew_template, set_qnew_template};

/// Plot a space to PDF bytes with JSON options (`{paper?, landscape?, fit?, lineweights?, …}`).
pub type PlotHook = fn(&Drawing, &cadaddy_doc::Space, &Value) -> std::result::Result<Vec<u8>, String>;

/// File format hooks (installed by the app so the engine stays I/O-agnostic and wasm-safe).
pub struct IoHooks {
    pub read: fn(&[u8], &str) -> std::result::Result<Drawing, String>,
    pub write: fn(&Drawing, &str) -> std::result::Result<Vec<u8>, String>,
    /// PLOT / EXPORTPDF (optional: builds without a plotter report "not available").
    pub plot: Option<PlotHook>,
    /// PDFIMPORT: one page of a PDF as a drawing, with JSON options, and a report (optional).
    pub read_pdf: Option<PdfHook>,
}

/// Read a PDF page with JSON options (`{page?, scale?, layers?, whiteFills?, text?, join?,
/// layout?}`) → the drawing and a report (scale used, counts, notes).
pub type PdfHook = fn(&[u8], &Value) -> std::result::Result<(Drawing, Value), String>;

static IO: OnceLock<IoHooks> = OnceLock::new();

/// The system's printers (a desktop host): their names, and printing a plot's PDF on one
/// (`printer`, `pdf`, a job title).
pub struct PrintHooks {
    pub list: fn() -> std::result::Result<Vec<String>, String>,
    pub print: fn(&str, &[u8], &str) -> std::result::Result<(), String>,
}

static PRINT: OnceLock<PrintHooks> = OnceLock::new();

pub fn set_print(h: PrintHooks) {
    let _ = PRINT.set(h);
}
pub fn print_hooks() -> Option<&'static PrintHooks> {
    PRINT.get()
}

pub fn set_io(h: IoHooks) {
    let _ = IO.set(h);
}
pub fn io() -> Option<&'static IoHooks> {
    IO.get()
}

/// An entity as DXF groups and back, for AutoLISP's `entget`, `entmake` and `entmod`
/// (installed by the app with the DXF reader and writer, a lower layer).
pub struct EntityCodec {
    /// The entity's type name and the groups of its record, without its `0` group.
    pub groups: fn(&Drawing, &cadaddy_doc::Entity) -> (String, Vec<cadaddy_dxf::Tag>),
    /// An entity from a type name and groups.
    pub entity: fn(&str, &[cadaddy_dxf::Tag]) -> Option<(cadaddy_doc::Common, cadaddy_doc::EntityKind)>,
    /// Every record of the entity (an INSERT, its ATTRIBs and SEQEND), as `groups` gives one.
    pub records: fn(&Drawing, &cadaddy_doc::Entity) -> Vec<(String, Vec<cadaddy_dxf::Tag>)>,
    /// An ATTRIB record's attribute, for a reference on the given layer.
    pub attrib: fn(&[cadaddy_dxf::Tag], &str) -> cadaddy_doc::Attrib,
}

static CODEC: OnceLock<EntityCodec> = OnceLock::new();

pub fn set_entity_codec(c: EntityCodec) {
    let _ = CODEC.set(c);
}
pub fn entity_codec() -> Option<&'static EntityCodec> {
    CODEC.get()
}

/// COMPARE (drawing comparison).
pub mod compare;
mod r12;
pub mod zip;

/// Hand finished file bytes to the user where there is no file system (the browser: a download).
/// Arguments: file name, MIME type, bytes. `Ok` only once the delivery was actually started.
pub type DeliverHook = fn(&str, &str, &[u8]) -> std::result::Result<(), String>;

static DELIVER: OnceLock<DeliverHook> = OnceLock::new();

/// Install the host's file delivery (the web app: a browser download). Saving uses it on wasm;
/// anything else that produces a file on the web (plots) can use [`deliver`] too.
pub fn set_deliver(h: DeliverHook) {
    let _ = DELIVER.set(h);
}
pub fn deliver() -> Option<DeliverHook> {
    DELIVER.get().copied()
}

/// How a drawing file is written: its version (`None`: the drawing's own, `$ACADVER`) and
/// binary instead of ASCII DXF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveAs {
    pub version: Option<Version>,
    pub binary: bool,
}

/// Write a drawing with [`SaveAs`] options (installed by hosts whose writer supports them).
pub type WriteAsHook = fn(&Drawing, &str, &SaveAs) -> std::result::Result<Vec<u8>, String>;

static WRITE_AS: OnceLock<WriteAsHook> = OnceLock::new();

/// Install the host's writer for [`SaveAs`] options; saves that ask for none use [`IoHooks::write`].
pub fn set_write_as(h: WriteAsHook) {
    let _ = WRITE_AS.set(h);
}
pub fn write_as() -> Option<WriteAsHook> {
    WRITE_AS.get().copied()
}

/// What a .cdy file holds and where, as the io layer's .cdy saver last wrote or read it (opaque
/// here: the engine only keeps it for the next save).
pub type CdyState = std::sync::Arc<dyn std::any::Any + Send + Sync>;

/// How a .cdy save is stored.
pub enum CdyWrite {
    /// A whole file, written atomically.
    Full(Vec<u8>),
    /// Bytes appended to the file, which must still be `from` bytes long: the changed chunks,
    /// then from `index_at` the new index and trailer.
    Append { from: u64, bytes: Vec<u8>, index_at: usize },
}

/// The .cdy saver (installed by hosts with the io layer): incremental saves of a drawing to the
/// .cdy file it came from.
pub struct CdyHooks {
    /// Save a drawing given what its file held (`None`: write it in full).
    pub save: fn(&Drawing, Option<&CdyState>) -> std::result::Result<(CdyWrite, CdyState), String>,
    /// What a .cdy file just read holds, matched to the drawing read from it (`None` when it
    /// can't be: the first save is then a full one).
    pub seed: fn(&Drawing, &[u8]) -> Option<CdyState>,
}

static CDY: OnceLock<CdyHooks> = OnceLock::new();

pub fn set_cdy_hooks(h: CdyHooks) {
    let _ = CDY.set(h);
}
pub fn cdy_hooks() -> Option<&'static CdyHooks> {
    CDY.get()
}

/// The .cdy file a drawing was last saved to or read from: its path, its length and modification
/// time then (a file changed since by anything else is written in full), and what it holds.
#[derive(Clone)]
pub struct CdySaved {
    path: String,
    len: u64,
    modified: Option<std::time::SystemTime>,
    state: CdyState,
}

impl std::fmt::Debug for CdySaved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdySaved").field("path", &self.path).field("len", &self.len).finish()
    }
}

/// The length and modification time of the file at `path`, if it is one.
fn on_disk(path: &str) -> Option<(u64, Option<std::time::SystemTime>)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let m = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
        Some((m.len(), m.modified().ok()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        None
    }
}

impl CdySaved {
    fn new(path: &str, state: CdyState) -> Option<Self> {
        let (len, modified) = on_disk(path)?;
        Some(CdySaved { path: path.to_string(), len, modified, state })
    }
    /// Whether the file at `path` is still exactly what this describes.
    fn holds(&self, path: &str) -> bool {
        same_file(&self.path, path) && on_disk(path) == Some((self.len, self.modified))
    }
}

/// Append a .cdy save to `path`: the chunks, made durable, then the index and trailer. A file no
/// longer `from` bytes long is left alone; a failed append cuts the file back to what it was, so
/// it still holds the previous save.
fn append_cdy(path: &str, from: u64, bytes: &[u8], index_at: usize) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::{Seek, SeekFrom, Write};
        let fail = |e: std::io::Error| bad("save", format!("{path}: {e}"));
        crate::writefile::check(std::path::Path::new(path)).map_err(fail)?;
        let mut f = std::fs::OpenOptions::new().write(true).open(path).map_err(fail)?;
        let m = f.metadata().map_err(fail)?;
        if m.permissions().readonly() {
            return Err(bad("save", format!("{path} is read-only. Save it under another name with SAVEAS.")));
        }
        if m.len() != from {
            return Err(bad("save", format!("{path} changed since it was last saved")));
        }
        let (chunks, index) = bytes.split_at(index_at.min(bytes.len()));
        let r = f
            .seek(SeekFrom::Start(from))
            .and_then(|_| f.write_all(chunks))
            .and_then(|()| f.sync_data())
            .and_then(|()| f.write_all(index))
            .and_then(|()| f.sync_all());
        if let Err(e) = r {
            let _ = f.set_len(from);
            return Err(fail(e));
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (from, bytes, index_at);
        Err(bad("save", format!("{path}: appending is not available on the web")))
    }
}

static DWG_COPY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// DWGCOPY: saving a .cdy file also writes a DWG copy next to it (the same name with `.dwg`), for
/// people who exchange drawings with AutoCAD users. A profile setting, off by default.
pub fn dwg_copy() -> bool {
    DWG_COPY.load(std::sync::atomic::Ordering::Relaxed)
}
pub fn set_dwg_copy(on: bool) {
    DWG_COPY.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The extension a typed file name without one gets on SAVEAS: the app's "Save new drawings as"
/// format (`cdy` by default; the app keeps it in step with its preference).
static DEFAULT_EXT: std::sync::Mutex<&'static str> = std::sync::Mutex::new("cdy");

pub fn default_ext() -> &'static str {
    DEFAULT_EXT.lock().map(|e| *e).unwrap_or("cdy")
}
/// `cdy`, `dwg` or `dxf`; anything else is ignored.
pub fn set_default_ext(ext: &str) {
    let ext = match ext.to_ascii_lowercase().as_str() {
        "cdy" => "cdy",
        "dwg" => "dwg",
        "dxf" => "dxf",
        _ => return,
    };
    if let Ok(mut e) = DEFAULT_EXT.lock() {
        *e = ext;
    }
}

/// Whether `path` names a .cdy file (CADaddy's own format).
pub fn is_cdy(path: &str) -> bool {
    std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("cdy"))
}

/// Whether `path` is a DWG, DXF or DWG template file: a format that may not keep everything a
/// drawing holds ([`kept::not_kept`]).
fn is_exchange_file(path: &str) -> bool {
    !is_cdy(path) && is_drawing_file(path)
}

/// Merge PDFs into one, each part with a bookmark title and a group ("" for none): a sheet set
/// published to one file.
pub type PdfMergeHook = fn(&[(Vec<u8>, String, String)]) -> std::result::Result<Vec<u8>, String>;

static PDF_MERGE: OnceLock<PdfMergeHook> = OnceLock::new();

/// Install the host's PDF merger (builds without one can't publish a sheet set to one PDF).
pub fn set_pdf_merge(h: PdfMergeHook) {
    let _ = PDF_MERGE.set(h);
}
pub fn pdf_merge() -> Option<PdfMergeHook> {
    PDF_MERGE.get().copied()
}

/// The versions a drawing can be saved in, for messages ("2000, 2004, …, 2018, 12 (DXF)").
fn version_list() -> String {
    let mut v: Vec<&str> = Version::ALL.iter().map(|v| v.year()).collect();
    v.push("12 (DXF)");
    v.join(", ")
}

/// The `version` and `binary` parameters of a save: a version is a year (2018 or "2018"), "R2018"
/// or an `$ACADVER` code ("AC1032").
fn save_options(cmd: &str, p: &Value) -> Result<SaveAs> {
    let version = match p.get("version") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            let name: String = text.chars().take(40).collect();
            Some(Version::parse(&text).ok_or_else(|| bad(cmd, format!("unknown version `{name}` (use {})", version_list())))?)
        }
    };
    let binary = match p.get("binary") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`binary` must be true or false")),
    };
    Ok(SaveAs { version, binary })
}

/// Whether `path` names a DXF file (or has no extension, which saves as DXF).
fn is_dxf(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "dxf" | "")
}

/// The drawing written to `path` with `opts`: through [`IoHooks::write`] when no option is set,
/// else through the [`write_as`] hook.
fn write_bytes(s: &Session, cmd: &str, path: &str, opts: &SaveAs) -> Result<Vec<u8>> {
    let hooks = io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    if opts.binary && !is_dxf(path) {
        return Err(bad(cmd, "binary applies to DXF files only"));
    }
    // A drawing file is the drawing outside the Block Editor; an image is what is shown.
    let d: &cadaddy_doc::Drawing = if is_drawing_file(path) { s.state()?.drawing() } else { s.doc()? };
    let r = if opts.version == Some(Version::R12) {
        // Release 12 DXF: the drawing brought down to what R12 holds first.
        if !is_dxf(path) {
            return Err(bad(cmd, "Release 12 is a DXF version: save a .dxf file"));
        }
        let w = write_as().ok_or_else(|| bad(cmd, "saving at a chosen version or as binary DXF is not available in this build"))?;
        w(&r12::downgrade(d).0, path, opts)
    } else if *opts == SaveAs::default() {
        (hooks.write)(d, path)
    } else {
        let w = write_as().ok_or_else(|| bad(cmd, "saving at a chosen version or as binary DXF is not available in this build"))?;
        w(d, path, opts)
    };
    r.map_err(|e| bad(cmd, e))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("new", "New Drawing...", run_new)
            .menu(&["File", "New Drawing..."])
            .key("Cmd+N")
            .alias(&["qnew"])
            .params("{metric?: bool, template?: path to a .dwt/.dxf template (\".\" for none; default QNEWTEMPLATE)}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("open", "Open...", run_open)
            .menu(&["File", "Open..."])
            .key("Cmd+O")
            .params("{path} | {data: base64, name} → {index, entities?, warnings: what reading left out}")
            .enabled(always)
            .noundo()
            .interactive(|_| Ok(Box::new(FileM { name: "OPEN", message: "Enter name of drawing to open", run: run_open }))),
        CommandSpec::new("close", "Close", run_close).menu(&["File", "Close"]).key("Cmd+W").params("{index?}").noundo(),
        CommandSpec::new("closeall", "Close All", run_closeall).menu(&["File", "Close All"]).enabled(always).noundo(),
        CommandSpec::new("qsave", "Save", run_qsave_reporting).menu(&["File", "Save"]).key("Cmd+S").alias(&["save"]).params("{path?}").noundo(),
        CommandSpec::new("saveas", "Save As...", run_saveas_reporting)
            .menu(&["File", "Save As..."])
            .key("Cmd+Shift+S")
            .params("{path (.cdy: CADaddy's own format, keeps everything; .dwg, .dxf, .dwt), version?: 2000|2004|2007|2010|2013|2018 (DWG/DXF), 12 (Release 12 DXF: newer objects converted, some left out), binary?: bool (DXF)} → {path, bytes, version?, notKept?: what DWG/DXF wouldn't keep (said once per drawing), dwgCopy?: the DWG copy DWGCOPY saved}")
            .noundo()
            .interactive(|_| Ok(Box::new(SaveAsM::default()))),
        CommandSpec::new("checkexchange", "Check Exchange", run_checkexchange)
            .menu(&["File", "Drawing Utilities", "Check Exchange"])
            .params("{format?: dwg|dxf (default: the drawing's file's, else dwg), version?: 2000|2004|2007|2010|2013|2018} → {format, version, kept: bool, losses: [{feature, what: dropped|changed, count, handles?, names?}], message}: what saving as DWG or DXF wouldn't keep, without saving")
            .noundo(),
        CommandSpec::new("document.switch", "Switch Drawing", run_switch)
            .params("{index} | {path} | {title}: the open drawing to make current (path and title in any case)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("document.list", "Open Drawings", run_list_docs)
            .params("{} → {active, drawings: [{index, title, path, modified (unsaved changes), active, entities}]}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("document.close", "Close Drawing", run_close_doc)
            .params("{index? | path? | title? (default the current drawing), save?: bool (save to its file first), saveAs?: path, discard?: bool} → {closed, saved?}. A drawing with unsaved changes is closed only with save, saveAs or discard (CLOSE itself discards)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("document.closeall", "Close All Drawings", run_closeall_docs)
            .params("{save?: bool, discard?: bool} → {closed: [titles], saved: [paths]}. Refused, closing nothing, while a drawing has unsaved changes and neither is given (or a drawing to save has no file name)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("document.bytes", "Drawing as bytes", run_bytes)
            .params("{format?: dxf (or cdy, dwg), version?: 2000…2018, binary?: bool} → {data: base64}")
            .noundo(),
        CommandSpec::new("script", "Run Script...", run_script)
            .menu(&["Tools", "Run Script..."])
            .alias(&["scr"])
            .params("{path: script (.scr) file} | {data: base64} → {lines}")
            .noundo()
            .interactive(|_| Ok(Box::new(ScriptM))),
        etransmit::spec(),
        sheetset::spec(),
    ]
}

/// SCRIPT: run the lines of a `.scr` file through [`Session::script`] — the same execution the
/// CLI, MCP and the control channel already use, now reachable from the command line and menus.
fn run_script(s: &mut Session, p: &Value) -> Result<Value> {
    let text = if let Some(_path) = str_param(p, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        let path = _path;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let bytes = crate::readfile::read(path).map_err(|e| bad("script", format!("{path}: {e}")))?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        #[cfg(target_arch = "wasm32")]
        return Err(bad("script", "paths are not available on the web; pass `data`"));
    } else {
        let data = str_param(p, "data").ok_or_else(|| bad("script", "`path` or `data` (base64) is required"))?;
        let bytes = base64_decode(data).ok_or_else(|| bad("script", "invalid base64"))?;
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let lines = text.lines().count();
    s.script(&text)?;
    Ok(json!({ "lines": lines }))
}

/// SCRIPT typed at the command line or chosen from the menu: asks for the script file by name
/// (the prompt form; the menu item runs this same machine).
struct ScriptM;

impl Interactive for ScriptM {
    fn name(&self) -> &'static str {
        "SCRIPT"
    }
    fn prompt(&self, _: &Session) -> Prompt {
        Prompt::new("Enter name of script file", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel | Input::Enter) {
            return Ok(Step::Cancel);
        }
        let Input::Text(t) = i else { return Ok(Step::Continue) };
        let t = t.trim();
        if t.is_empty() {
            return Ok(Step::Cancel);
        }
        match run_script(s, &json!({ "path": t })) {
            Ok(_) => Ok(Step::Done),
            Err(EngineError::BadParams { msg, .. }) => {
                s.echo(msg);
                Ok(Step::Done)
            }
            Err(e) => Err(e),
        }
    }
}

fn run_new(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(r) = template::run_new(s, p)? {
        return Ok(r);
    }
    let i = s.new_drawing_with_viewports(bool_or(p, "metric", false));
    Ok(json!({ "index": i, "title": s.docs.get(i).map(|d| d.title.clone()) }))
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// A file read and decoded but not yet added to a [`Session`]. Reading is the slow part of OPEN and
/// needs no session, so a host can do it on a worker thread and hand the result to
/// [`finish_open`] on the thread that owns the session.
pub struct ReadFile {
    drawing: Drawing,
    binary_dxf: bool,
    name: String,
    path: Option<String>,
    /// For a .cdy file: what it holds, for incremental saves.
    cdy: Option<CdyState>,
    /// Read from a walkthrough package (`.cdywalk`): its baked lightmap, if it has one.
    walkthrough: Option<Option<cadaddy_render::lightmap::Lightmap>>,
}

impl ReadFile {
    /// The decoded drawing, for callers that read a file without opening it (walk mode's sheets).
    pub fn into_drawing(self) -> Drawing {
        self.drawing
    }
}

/// Relative image paths in a drawing are relative to its folder: look for images there too.
fn image_folder(path: Option<&str>) {
    let Some(p) = path.filter(|p| !p.trim().is_empty()) else { return };
    // A drawing named relative to the working folder: its folder as a full path.
    let full = std::path::absolute(p).unwrap_or_else(|_| std::path::PathBuf::from(p));
    if let Some(dir) = full.parent().filter(|d| !d.as_os_str().is_empty()) {
        cadaddy_render::images::add_search_dir(dir);
        cadaddy_fonts::shx::set_drawing_dir(&dir.to_string_lossy());
    }
}

/// Point relative image paths at the current drawing's folder when the current drawing changed
/// (a tab switched to, a drawing closed); the UI calls it every frame, and it does nothing while
/// the folder stays the same.
pub fn sync_image_folder(s: &Session) {
    let Some(p) = s.state().ok().and_then(|st| st.path.clone()) else { return };
    let full = std::path::absolute(&p).unwrap_or_else(|_| std::path::PathBuf::from(&p));
    if let Some(dir) = full.parent().filter(|d| !d.as_os_str().is_empty()) {
        // Shape fonts too: the drawing's folder first (does nothing while it stays the same).
        cadaddy_fonts::shx::set_drawing_dir(&dir.to_string_lossy());
        if !cadaddy_render::images::is_search_dir(dir) {
            cadaddy_render::images::add_search_dir(dir);
        }
    }
}

/// Read and decode `path` (the slow half of `OPEN {path}`). Needs no session; runs anywhere.
pub fn read_file(path: &str) -> std::result::Result<ReadFile, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let r = std::panic::catch_unwind(|| {
            let bytes = crate::readfile::read(path).map_err(|e| bad("open", format!("{path}: {e}")))?;
            read_bytes(&bytes, path, Some(path.to_string()))
        });
        match r {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err(bad("open", format!("{path}: the file could not be read")).to_string()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(bad("open", "paths are not available on the web; pass `data`").to_string())
    }
}

/// Add a [`read_file`] result to the session as the active drawing: the fast half of OPEN.
pub fn finish_open(s: &mut Session, f: ReadFile) -> Result<Value> {
    let ReadFile { drawing, binary_dxf, name, path, cdy, walkthrough } = f;
    if let Some(i) = path.as_deref().and_then(|p| already_open(s, p)) {
        return Ok(switch_to_open(s, i));
    }
    let i = add_read(s, drawing, binary_dxf, &name, path, cdy);
    mark_walkthrough(s, i, walkthrough);
    super::blocks::xref::after_open(s, i);
    crate::activity::after_open(s, i);
    Ok(opened(s, i, true))
}

fn read_bytes(bytes: &[u8], name: &str, path: Option<String>) -> Result<ReadFile> {
    let hooks = io().ok_or_else(|| bad("open", "file formats are not available in this build"))?;
    let drawing = (hooks.read)(bytes, name).map_err(|e| bad("open", e))?;
    // A file that isn't a drawing (an imported SVG) gives the drawing no file name: QSAVE then
    // asks where to save instead of exporting over the original.
    let path = path.filter(|p| is_drawing_file(p));
    let cdy = path.as_ref().filter(|p| is_cdy(p)).and_then(|_| cdy_hooks()).and_then(|h| (h.seed)(&drawing, bytes));
    let walkthrough = cadaddy_walk3d::package::is_package(bytes).then(|| package_lightmap(bytes));
    Ok(ReadFile { drawing, binary_dxf: cadaddy_dxf::is_binary(bytes), name: name.to_string(), path, cdy, walkthrough })
}

/// The baked lightmap in a walkthrough package, when it has a whole one.
fn package_lightmap(bytes: &[u8]) -> Option<cadaddy_render::lightmap::Lightmap> {
    use cadaddy_walk3d::package::{LIGHTMAP_JSON, LIGHTMAP_PNG, Package};
    let p = Package::decode(bytes).ok()?;
    super::lightbake::read_lightmap(p.get(LIGHTMAP_PNG)?, std::str::from_utf8(p.get(LIGHTMAP_JSON)?).ok()?)
}

/// A drawing opened from a walkthrough package: Walk opens on it, with the package's lightmap.
fn mark_walkthrough(s: &mut Session, i: usize, w: Option<Option<cadaddy_render::lightmap::Lightmap>>) {
    let Some(lm) = w else { return };
    if let Some(st) = s.docs.get_mut(i) {
        st.walkthrough = true;
        if let Some(lm) = lm {
            st.lightmap = Some(std::sync::Arc::new(lm));
            st.lightmap_rev += 1;
        }
        st.lightmap_tried = true;
    }
}

fn add_read(s: &mut Session, mut drawing: Drawing, binary_dxf: bool, name: &str, path: Option<String>, cdy: Option<CdyState>) -> usize {
    let reset = crate::audit::current_styles(&mut drawing);
    // Every way a drawing is opened (the OPEN command, the background open, scripts) looks for
    // its relative image paths in its folder first, and images that moved are looked up again.
    image_folder(path.as_deref());
    cadaddy_render::images::revalidate();
    let notes = drawing.read_notes.clone();
    let loss = read_loss(&drawing);
    let has_path = path.is_some();
    let cdy_saved = path.as_deref().zip(cdy).and_then(|(p, state)| CdySaved::new(p, state));
    let i = s.open_drawing(drawing, &file_name(name), path);
    if let Some(st) = s.docs.get_mut(i) {
        // QSAVE keeps a binary DXF binary.
        st.binary_dxf = binary_dxf;
        st.read_loss = loss.filter(|_| has_path);
        st.open_notes = reset.clone();
        st.cdy_saved = cdy_saved;
    }
    // Fields worked out on opening (FIELDEVAL 1): what they show isn't an edit to save.
    if s.active == i
        && crate::fields::on_event(s, crate::fields::Event::Open) > 0
        && let Some(st) = s.docs.get_mut(i)
    {
        st.saved = st.doc.clone();
    }
    for note in notes.into_iter().chain(reset) {
        s.echo(note);
    }
    i
}

/// What reading left out of a drawing that the file still has, when anything was: a DWG's
/// unreadable object records, a cut-off DXF, records a damaged DXF couldn't place.
pub(crate) fn read_loss(d: &Drawing) -> Option<String> {
    if let Some(note) = d.read_notes.first() {
        return Some(note.clone());
    }
    let n = d.skipped_records.values().fold(0usize, |a, n| a.saturating_add(*n));
    (n > 0).then(|| {
        format!("{n} damaged record{} couldn't be placed and {} left out", if n == 1 { "" } else { "s" }, if n == 1 { "was" } else { "were" })
    })
}

/// Whether two paths name the same file (resolved when they exist, else compared as written;
/// without regard to case on Windows and macOS, whose file systems usually ignore it).
pub(crate) fn same_file(a: &str, b: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if let (Ok(x), Ok(y)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return x == y;
    }
    if cfg!(any(windows, target_os = "macos")) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// The open drawing read from `path`, if any: opening it again switches to it instead of making a
/// second copy whose save would overwrite the first one's edits.
pub(super) fn already_open(s: &Session, path: &str) -> Option<usize> {
    s.docs.iter().position(|d| d.path.as_deref().is_some_and(|p| same_file(p, path)))
}

/// OPEN of a file that is already open: that drawing becomes current, as in AutoCAD.
fn switch_to_open(s: &mut Session, i: usize) -> Value {
    s.active = i;
    image_folder(s.docs.get(i).and_then(|d| d.path.clone()).as_deref());
    let title = s.docs.get(i).map(|d| d.title.clone()).unwrap_or_default();
    s.echo(format!("{title} is already open."));
    let mut r = opened(s, i, true);
    r["alreadyOpen"] = json!(true);
    r
}

pub(crate) fn open_bytes(s: &mut Session, bytes: &[u8], name: &str, path: Option<String>) -> Result<usize> {
    let ReadFile { drawing, binary_dxf, name, path, cdy, walkthrough } = read_bytes(bytes, name, path)?;
    let i = add_read(s, drawing, binary_dxf, &name, path, cdy);
    mark_walkthrough(s, i, walkthrough);
    Ok(i)
}

/// OPEN's reply: the drawing's index and what reading it left out.
fn opened(s: &Session, i: usize, entities: bool) -> Value {
    let d = s.docs.get(i).map(|st| &st.doc);
    let mut warnings = d.map(|d| d.read_notes.clone()).unwrap_or_default();
    warnings.extend(s.docs.get(i).map(|st| st.open_notes.clone()).unwrap_or_default());
    // Xrefs that didn't load, and why (agents see no command-line history).
    if let Some(d) = d {
        for b in d.blocks.values().filter(|b| b.xref_path.is_some() && !b.xref_reason.is_empty()) {
            warnings.push(format!("Xref \"{}\" {}: {}", b.name, b.xref_status.label().to_ascii_lowercase(), b.xref_reason));
        }
    }
    if entities {
        json!({ "index": i, "entities": d.map(|d| d.entity_count()), "warnings": warnings })
    } else {
        json!({ "index": i, "warnings": warnings })
    }
}

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(_path) = str_param(p, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        let path = _path;
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(i) = already_open(s, path) {
                return Ok(switch_to_open(s, i));
            }
            let bytes = crate::readfile::read(path).map_err(|e| bad("open", format!("{path}: {e}")))?;
            let i = open_bytes(s, &bytes, path, Some(path.to_string()))?;
            super::blocks::xref::after_open(s, i);
            crate::activity::after_open(s, i);
            return Ok(opened(s, i, true));
        }
        #[cfg(target_arch = "wasm32")]
        return Err(bad("open", "paths are not available on the web; pass `data`"));
    }
    let data = str_param(p, "data").ok_or_else(|| bad("open", "`path` or `data` (base64) is required"))?;
    let bytes = base64_decode(data).ok_or_else(|| bad("open", "invalid base64"))?;
    let name = str_param(p, "name").unwrap_or("Drawing.dxf");
    let i = open_bytes(s, &bytes, name, None)?;
    super::blocks::xref::after_open(s, i);
    crate::activity::after_open(s, i);
    Ok(opened(s, i, false))
}

pub(super) fn run_close(s: &mut Session, p: &Value) -> Result<Value> {
    let i = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(s.active);
    if i >= s.docs.len() {
        return Err(bad("close", "no such drawing"));
    }
    s.cancel();
    s.docs.remove(i);
    if i < s.active {
        // An earlier drawing went away: the active one shifted down by one.
        s.active -= 1;
    }
    if s.active >= s.docs.len() {
        s.active = s.docs.len().saturating_sub(1);
    }
    release_closed(s);
    ok()
}

/// Let go of what process-wide caches hold for drawings no longer open: the selection indexes
/// (they hold their drawing's objects; the open drawings' are rebuilt when next used) and the
/// decoded pixels of images only closed drawings showed. Either kept a closed drawing's memory
/// (gigabytes for a large one) until the program exited.
fn release_closed(s: &Session) {
    crate::spatial::clear_cache();
    let used: std::collections::HashSet<&str> = s
        .docs
        .iter()
        .flat_map(|st| {
            let d = &st.doc;
            std::iter::once(&d.model)
                .chain(d.layouts.iter().map(|l| &l.entities))
                .chain(d.blocks.values().map(|b| &b.entities))
                .flat_map(|e| e.iter())
        })
        .filter_map(|e| if let cadaddy_doc::EntityKind::Image(i) = &e.kind { Some(i.path.as_str()) } else { None })
        .collect();
    cadaddy_render::images::keep_only(&used);
}

fn run_closeall(s: &mut Session, _p: &Value) -> Result<Value> {
    s.cancel();
    s.docs.clear();
    s.active = 0;
    release_closed(s);
    ok()
}

/// What a save wrote: the file's size, and with DWGCOPY the DWG copy's path (or why it failed).
pub(crate) struct Saved {
    bytes: usize,
    copy: Option<std::result::Result<String, String>>,
    /// Appended to the .cdy file (only what changed) rather than written in full.
    incremental: bool,
}

pub(crate) fn save_to(s: &mut Session, path: &str) -> Result<Saved> {
    let opts = SaveAs { version: None, binary: s.state()?.binary_dxf && is_dxf(path) };
    save_as(s, "save", path, &opts)
}

/// Save to `path` with `opts`. A drawing file then keeps the version and binary choice: the
/// version goes into the drawing's `$ACADVER`, so later saves (QSAVE) write it again.
fn save_as(s: &mut Session, cmd: &str, path: &str, opts: &SaveAs) -> Result<Saved> {
    if s.doc()?.refedit.is_some() {
        return Err(bad(cmd, "A block reference is being edited in place: finish with REFCLOSE (Save or Discard) before saving."));
    }
    // Saving over the file a drawing was read from with parts left out loses them for good: the
    // first save there explains and stops; saving again overwrites (keeping a .bak).
    let st = s.state_mut()?;
    let over_original = st.path.as_deref().is_some_and(|p| same_file(p, path));
    if over_original
        && !st.loss_warned
        && let Some(why) = st.read_loss.clone()
    {
        st.loss_warned = true;
        return Err(bad(
            cmd,
            format!(
                "{} was opened with parts missing ({why}). Saving over it would lose them for good. \
                 Use SAVEAS to save under another name and keep the original, or save again to overwrite it \
                 (the original is kept as {}).",
                st.title,
                file_name(&bak_path(path))
            ),
        ));
    }
    // Another open drawing's file: saving over it would wipe that drawing's work, and both tabs
    // would then claim the file (AutoCAD refuses too).
    if is_drawing_file(path) {
        let active = s.active;
        if let Some(other) = s.docs.iter().enumerate().find(|(i, d)| *i != active && d.path.as_deref().is_some_and(|p| same_file(p, path))) {
            return Err(bad(
                cmd,
                format!("{} is open in another tab ({}). Close it first, or save under another name.", file_name(path), other.1.title),
            ));
        }
    }
    // The save date, and fields worked out on saving (FIELDEVAL 2).
    if is_drawing_file(path) {
        crate::fields::on_save(s);
    }
    let (n, incremental) = match save_cdy(s, path, opts)? {
        Some(r) => r,
        None => {
            let bytes = write_bytes(s, cmd, path, opts)?;
            (commit_save(s, path, &bytes, store)?, false)
        }
    };
    if is_drawing_file(path)
        && let Ok(st) = s.state_mut()
    {
        // The drawing's file now holds just what the drawing has: nothing more to lose (a SAVEAS
        // left the original, with what reading left out, where it was).
        st.read_loss = None;
    }
    if is_drawing_file(path) {
        let st = s.state_mut()?;
        st.binary_dxf = opts.binary;
        if let Some(v) = opts.version
            && st.drawing().header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str) != Some(v.code())
        {
            std::sync::Arc::make_mut(st.drawing_arc_mut()).header.set_str("ACADVER", v.code());
            st.saved = st.drawing().clone();
        }
    }
    if is_drawing_file(path) {
        crate::activity::after_save(s, path);
    }
    let copy = (is_cdy(path) && dwg_copy()).then(|| save_dwg_copy(s, cmd, path));
    Ok(Saved { bytes: n, copy, incremental })
}

/// Save to a .cdy file through the .cdy saver: appended to the file when it is the one the drawing
/// was last saved to or read from and nothing else changed it since, else in full. `None` when
/// this isn't such a save (another format, or no saver: [`write_bytes`] writes it). The bytes
/// written and whether they were appended.
fn save_cdy(s: &mut Session, path: &str, opts: &SaveAs) -> Result<Option<(usize, bool)>> {
    let Some(h) = cdy_hooks().filter(|_| is_cdy(path) && *opts == SaveAs::default()) else { return Ok(None) };
    let st = s.state()?;
    let prev = st.cdy_saved.as_ref().filter(|c| c.holds(path)).map(|c| c.state.clone());
    let d = st.drawing().clone();
    let (out, mut state) = (h.save)(&d, prev.as_ref()).map_err(|e| bad("save", e))?;
    let (n, incremental) = match out {
        CdyWrite::Append { from, bytes, index_at } => match commit_save(s, path, &bytes, |p, b| append_cdy(p, from, b, index_at)) {
            Ok(n) => (n, true),
            // Appending failed (the file was cut back to the previous save): write it in full.
            Err(_) => {
                let (out, full) = (h.save)(&d, None).map_err(|e| bad("save", e))?;
                state = full;
                let CdyWrite::Full(bytes) = out else { return Err(bad("save", "the .cdy saver didn't write the whole file")) };
                (commit_save(s, path, &bytes, store)?, false)
            }
        },
        CdyWrite::Full(bytes) => (commit_save(s, path, &bytes, store)?, false),
    };
    if let Ok(st) = s.state_mut() {
        st.cdy_saved = CdySaved::new(path, state);
    }
    Ok(Some((n, incremental)))
}

/// DWGCOPY: write the DWG copy of a drawing just saved to the .cdy file `path`. A copy that fails
/// is reported, never failing the save itself.
fn save_dwg_copy(s: &mut Session, cmd: &str, path: &str) -> std::result::Result<String, String> {
    let copy = std::path::Path::new(path).with_extension("dwg").to_string_lossy().into_owned();
    let r = write_bytes(s, cmd, &copy, &SaveAs::default()).and_then(|b| store_keeping(&copy, &b, false));
    match r {
        Ok(()) => {
            s.echo(format!("Saved a DWG copy: {}", file_name(&copy)));
            Ok(copy)
        }
        Err(e) => {
            let msg = format!("The DWG copy wasn't saved: {e}");
            s.echo(msg.clone());
            Err(msg)
        }
    }
}

/// Put saved bytes where the user gets them: natively the file at `path` (written atomically); on
/// the web, which has no file system, a browser download named after `path` via [`deliver`].
/// Where saving over a drawing keeps the previous file: the same name with `.bak` (AutoCAD's
/// ISAVEBAK).
fn bak_path(path: &str) -> String {
    std::path::Path::new(path).with_extension("bak").to_string_lossy().into_owned()
}

pub(crate) fn store(path: &str, bytes: &[u8]) -> Result<()> {
    store_keeping(path, bytes, true)
}

/// [`store`], keeping the previous drawing file as `.bak` only when `keep_bak`: a DWG copy
/// (DWGCOPY) keeps none, as its `.bak` would replace the drawing's own.
pub(crate) fn store_keeping(path: &str, bytes: &[u8], keep_bak: bool) -> Result<()> {
    // An agent's dry run changes nothing, files included.
    if !crate::agent::writes_allowed() {
        return Err(crate::EngineError::Other(format!("Dry run: {path} was not written.")));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The file being replaced, if any: a read-only one is left alone (as AutoCAD does: save
        // under another name), and the new file keeps its permissions.
        let before = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file);
        if let Some(m) = &before
            && m.permissions().readonly()
        {
            return Err(bad("save", format!("{path} is read-only. Save it under another name with SAVEAS.")));
        }
        // A drawing saved over keeps its previous version as .bak, as AutoCAD does.
        if keep_bak && before.is_some() && is_drawing_file(path) {
            let bak = bak_path(path);
            let keep = || -> std::io::Result<()> {
                // An earlier .bak (perhaps read-only) is replaced, not written into.
                if let Ok(m) = std::fs::metadata(&bak) {
                    let mut p = m.permissions();
                    if p.readonly() {
                        #[allow(clippy::permissions_set_readonly_false)]
                        p.set_readonly(false);
                        std::fs::set_permissions(&bak, p)?;
                    }
                    std::fs::remove_file(&bak)?;
                }
                std::fs::copy(path, &bak).map(|_| ())
            };
            keep().map_err(|e| bad("save", format!("{path}: couldn't keep the previous version as {bak}: {e}")))?;
        }
        let tmp = format!("{path}.cadaddy-tmp");
        let written = crate::writefile::write(&tmp, bytes)
            .and_then(|()| before.as_ref().map_or(Ok(()), |m| std::fs::set_permissions(&tmp, m.permissions())))
            .and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            // A failed save leaves nothing behind but the file as it was.
            let _ = std::fs::remove_file(&tmp);
            return Err(bad("save", format!("{path}: {e}")));
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = keep_bak;
        let name = file_name(path);
        let deliver = deliver().ok_or_else(|| bad("save", "saving files is not available in this build; use document.bytes"))?;
        deliver(&name, "application/octet-stream", bytes).map_err(|e| bad("save", format!("{name}: {e}")))
    }
}

/// Store the bytes, then mark the drawing saved under `path`. If storing fails the drawing keeps
/// its unsaved changes, file name and title. On the web a started download counts as saved: the
/// browser owns the file from there (we can't see whether the user keeps it), and the name stays so
/// a later QSAVE downloads the drawing again under it. An image or PDF export (SVG, PNG, PDF) only
/// stores the file: the drawing keeps its name, title and unsaved changes, so closing still asks.
fn commit_save(s: &mut Session, path: &str, bytes: &[u8], store: impl FnOnce(&str, &[u8]) -> Result<()>) -> Result<usize> {
    s.state()?;
    store(path, bytes)?;
    if !is_drawing_file(path) {
        return Ok(bytes.len());
    }
    image_folder(Some(path));
    let st = s.state_mut()?;
    st.saved = st.drawing().clone();
    st.path = Some(path.to_string());
    st.title = file_name(path);
    // A BEgin group still open when the drawing is saved ends there.
    st.end_undo_group();
    Ok(bytes.len())
}

/// The save result; on the web it also names the file the browser downloaded.
fn saved(path: &str, sv: &Saved) -> Value {
    let mut r = json!({ "path": path, "bytes": sv.bytes });
    if sv.incremental {
        r["incremental"] = json!(true);
    }
    if cfg!(target_arch = "wasm32") {
        r["download"] = json!(file_name(path));
    }
    match &sv.copy {
        Some(Ok(copy)) => r["dwgCopy"] = json!(copy),
        Some(Err(why)) => r["dwgCopyError"] = json!(why),
        None => {}
    }
    r
}

fn run_save(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match str_param(p, "path") {
        Some(p) => p.to_string(),
        None => s.state()?.path.clone().ok_or_else(|| bad("qsave", "drawing has no file name yet; use saveas {path}"))?,
    };
    let sv = save_to(s, &path)?;
    Ok(saved(&path, &sv))
}

/// A file name with no extension saves as a drawing in the default format ([`default_ext`]), as
/// AutoCAD adds `.dwg`.
fn with_drawing_ext(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if name.contains('.') || name.trim().is_empty() { path.to_string() } else { format!("{path}.{}", default_ext()) }
}

fn run_saveas(s: &mut Session, p: &Value) -> Result<Value> {
    let path = with_drawing_ext(str_param(p, "path").ok_or_else(|| bad("saveas", "`path` is required"))?);
    let mut opts = save_options("saveas", p)?;
    // A version is a DWG/DXF one: a .cdy file keeps everything whatever is asked.
    if is_cdy(&path) {
        opts.version = None;
    }
    let sv = save_as(s, "saveas", &path, &opts)?;
    let mut r = saved(&path, &sv);
    if is_exchange_file(&path) {
        r["version"] = json!(opts.version.unwrap_or_else(|| drawing_version(s, &path)).year());
        r["binary"] = json!(opts.binary);
    }
    Ok(r)
}

/// The version a drawing is saved in at `path` when none is asked for: its `$ACADVER` (2018 by
/// default), and for DWG at least 2004 (the io layer's rule: R2000 DWG has no true colours).
fn drawing_version(s: &Session, path: &str) -> Version {
    let v = s.doc().ok().and_then(|d| d.header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str).and_then(Version::for_acadver)).unwrap_or_default();
    if is_dxf(path) { v } else { v.max(Version::R2004) }
}

fn run_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let i = doc_index(s, "document.switch", p)?.ok_or_else(|| bad("document.switch", "`index`, `path` or `title` is required"))?;
    s.cancel();
    s.active = i;
    image_folder(s.docs.get(i).and_then(|d| d.path.clone()).as_deref());
    ok()
}

/// The open drawing `p` names by `index`, `path` or `title`; `None` when it names none.
fn doc_index(s: &Session, cmd: &str, p: &Value) -> Result<Option<usize>> {
    if let Some(i) = p.get("index").filter(|v| !v.is_null()) {
        let i = i.as_u64().map(|i| i as usize).filter(|i| *i < s.docs.len()).ok_or_else(|| bad(cmd, "no such drawing"))?;
        return Ok(Some(i));
    }
    if let Some(path) = str_param(p, "path") {
        return s
            .docs
            .iter()
            .position(|d| d.path.as_deref().is_some_and(|q| same_file(q, path)))
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("{path} is not open")));
    }
    if let Some(title) = str_param(p, "title") {
        return s
            .docs
            .iter()
            .position(|d| d.title.eq_ignore_ascii_case(title))
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("no open drawing is called {title}")));
    }
    Ok(None)
}

fn run_list_docs(s: &mut Session, _p: &Value) -> Result<Value> {
    let drawings: Vec<Value> = s
        .docs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            json!({ "index": i, "title": d.title, "path": d.path, "modified": d.is_dirty(), "active": i == s.active, "entities": d.drawing().entity_count() })
        })
        .collect();
    Ok(json!({ "active": s.active, "drawings": drawings }))
}

/// Save drawing `i` (to `save_as`, else its own file) and make it current: what closing with
/// `save` does first. The path it was saved to.
fn save_doc(s: &mut Session, cmd: &str, i: usize, save_as: Option<&str>) -> Result<String> {
    let path = match save_as {
        Some(p) => with_drawing_ext(p),
        None => s.docs.get(i).and_then(|d| d.path.clone()).ok_or_else(|| {
            bad(cmd, format!("\"{}\" has no file name yet: pass saveAs with a path", s.docs.get(i).map(|d| d.title.as_str()).unwrap_or("")))
        })?,
    };
    s.cancel();
    s.active = i;
    run_saveas(s, &json!({ "path": path }))?;
    Ok(path)
}

fn run_close_doc(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "document.close";
    let i = doc_index(s, CMD, p)?.unwrap_or(s.active);
    let st = s.docs.get(i).ok_or_else(|| bad(CMD, "no drawing is open"))?;
    let (title, dirty) = (st.title.clone(), st.is_dirty());
    let save_as = str_param(p, "saveAs");
    let save = p.get("save").and_then(Value::as_bool).unwrap_or(false) || save_as.is_some();
    let discard = p.get("discard").and_then(Value::as_bool).unwrap_or(false);
    if save && discard {
        return Err(bad(CMD, "pass save or discard, not both"));
    }
    let mut saved = None;
    if dirty || save_as.is_some() {
        if save {
            saved = Some(save_doc(s, CMD, i, save_as)?);
        } else if !discard {
            return Err(bad(CMD, format!("\"{title}\" has unsaved changes: pass save: true (or saveAs) to keep them, or discard: true")));
        }
    }
    run_close(s, &json!({ "index": i }))?;
    Ok(json!({ "closed": title, "saved": saved }))
}

fn run_closeall_docs(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "document.closeall";
    let save = p.get("save").and_then(Value::as_bool).unwrap_or(false);
    let discard = p.get("discard").and_then(Value::as_bool).unwrap_or(false);
    if save && discard {
        return Err(bad(CMD, "pass save or discard, not both"));
    }
    let dirty: Vec<(usize, String, bool)> =
        s.docs.iter().enumerate().filter(|(_, d)| d.is_dirty()).map(|(i, d)| (i, d.title.clone(), d.path.is_some())).collect();
    if !dirty.is_empty() && !save && !discard {
        let names: Vec<&str> = dirty.iter().map(|(_, t, _)| t.as_str()).collect();
        return Err(bad(CMD, format!("unsaved changes in {}: pass save: true or discard: true", names.join(", "))));
    }
    // Check every drawing to save can be, before saving or closing anything.
    if save && let Some((_, t, _)) = dirty.iter().find(|(_, _, has_path)| !has_path) {
        return Err(bad(CMD, format!("\"{t}\" has no file name yet: save it with document.close and saveAs first")));
    }
    let mut saved = Vec::new();
    if save {
        for (i, _, _) in &dirty {
            saved.push(save_doc(s, CMD, *i, None)?);
        }
    }
    let closed: Vec<String> = s.docs.iter().map(|d| d.title.clone()).collect();
    run_closeall(s, &json!({}))?;
    Ok(json!({ "closed": closed, "saved": saved }))
}

fn run_bytes(s: &mut Session, p: &Value) -> Result<Value> {
    let fmt = str_param(p, "format").unwrap_or("dxf");
    let opts = save_options("document.bytes", p)?;
    let version = opts.version.unwrap_or_else(|| drawing_version(s, &format!("drawing.{fmt}")));
    let lost = not_saved_as(s.doc()?, version);
    let bytes = write_bytes(s, "document.bytes", &format!("drawing.{fmt}"), &opts)?;
    let mut r = json!({ "data": base64_encode(&bytes), "bytes": bytes.len() });
    if drops_unknown(&format!("drawing.{fmt}")) {
        report_not_saved(s, &mut r, &lost);
    }
    Ok(r)
}

/// Entities a DXF or DWG file doesn't keep: those of object types CADaddy doesn't model
/// (3DSOLID, MULTILEADER… read from other programs' files and kept in the drawing, but
/// not written), counted by DXF type across model space, layouts and block definitions. Saved
/// at the drawing's own version; see [`not_saved_as`].
pub fn not_saved(d: &Drawing) -> std::collections::BTreeMap<String, usize> {
    let v = d.header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str).and_then(Version::for_acadver).unwrap_or_default();
    not_saved_as(d, v)
}

/// Solid modeler entities whose ACIS geometry a file of AutoCAD 2013 and later keeps in its data
/// store section, which is written back with the drawing (`Foreign::data_store`).
const MODELER_TYPES: [&str; 3] = ["3DSOLID", "REGION", "BODY"];

/// [`not_saved`] for a file saved as `version`. From 2013 on, a 3D solid, region or body the data
/// store has a record for keeps its geometry and is not counted.
pub fn not_saved_as(d: &Drawing, version: Version) -> std::collections::BTreeMap<String, usize> {
    let mut out: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let stored = if version >= Version::R2013 { d.foreign.data_store_entities() } else { Default::default() };
    let stores = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)).chain(d.blocks.values().map(|b| &b.entities));
    for e in stores.flat_map(|s| s.iter()) {
        if let cadaddy_doc::EntityKind::Unknown(u) = &e.kind {
            if MODELER_TYPES.iter().any(|t| u.dxf_type.eq_ignore_ascii_case(t)) && stored.contains(&e.handle) {
                continue;
            }
            let n = out.entry(u.dxf_type.clone()).or_insert(0);
            *n = n.saturating_add(1);
        }
    }
    out
}

/// The command-line warning for [`not_saved`] entities, or `None` when the file keeps everything.
pub fn not_saved_message(lost: &std::collections::BTreeMap<String, usize>) -> Option<String> {
    if lost.is_empty() {
        return None;
    }
    let list: Vec<String> = lost.iter().map(|(t, n)| format!("{n} {t}")).collect();
    Some(format!("Not saved in the DWG file (its writer can't write these object types yet; DXF keeps them): {}", list.join(", ")))
}

/// Whether saving to `path` leaves out the entities [`not_saved`] counts: DWG does (the DWG
/// bridge drops types it doesn't know); DXF writes them back as read.
pub fn drops_unknown(path: &str) -> bool {
    // Templates (.dwt) are written as DWG too.
    std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("dwg") || e.eq_ignore_ascii_case("dwt"))
}

/// Whether `path` is saved as a drawing (.cdy, DXF, DWG) rather than exported as an image.
fn is_drawing_file(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "cdy" | "dxf" | "dwg" | "dwt" | "")
}

/// Say on the command line what the file couldn't keep, and list it in the result's `notSaved`.
fn report_not_saved(s: &mut Session, r: &mut Value, lost: &std::collections::BTreeMap<String, usize>) {
    let Some(msg) = not_saved_message(lost) else { return };
    s.echo(msg);
    if let Some(o) = r.as_object_mut() {
        o.insert("notSaved".into(), json!(lost));
    }
}

/// Save, then report the entities the file couldn't keep (nothing is dropped silently).
fn save_reporting(s: &mut Session, p: &Value, run: fn(&mut Session, &Value) -> Result<Value>) -> Result<Value> {
    // What the file will hold depends on the version it is saved as (the data store from 2013).
    let path = str_param(p, "path").map(str::to_string).or_else(|| s.state().ok().and_then(|st| st.path.clone())).unwrap_or_default();
    let version = save_options("save", p).ok().and_then(|o| o.version).unwrap_or_else(|| drawing_version(s, &path));
    let lost = s.doc().map(|d| not_saved_as(d, version)).unwrap_or_default();
    // Release 12 has no form for some objects (the rest are converted to R12 ones).
    let r12_lost = if version == Version::R12 { s.doc().map(|d| r12::downgrade(d).1).unwrap_or_default() } else { Default::default() };
    let mut r = run(s, p)?;
    if !r12_lost.is_empty() && r.get("path").and_then(Value::as_str).is_some_and(is_dxf) {
        let list: Vec<String> = r12_lost.iter().map(|(t, n)| format!("{n} {t}")).collect();
        s.echo(format!("Not saved in the Release 12 DXF file (R12 has no such objects): {}", list.join(", ")));
        if let Some(o) = r.as_object_mut() {
            o.insert("notSaved".into(), json!(r12_lost));
        }
    }
    if r.get("path").and_then(Value::as_str).is_some_and(drops_unknown) {
        report_not_saved(s, &mut r, &lost);
    }
    if let Some(path) = r.get("path").and_then(Value::as_str).filter(|p| is_exchange_file(p)).map(str::to_string) {
        let opts = save_options("save", p).unwrap_or_default();
        report_not_kept(s, &mut r, &path, opts);
    }
    Ok(r)
}

/// The first save of a drawing into DWG or DXF: say what that file doesn't keep that a .cdy file
/// would (once per drawing), and list it in the result's `notKept`.
fn report_not_kept(s: &mut Session, r: &mut Value, path: &str, opts: SaveAs) {
    let Some(hooks) = io() else { return };
    let Ok(st) = s.state_mut() else { return };
    if st.kept_checked {
        return;
    }
    st.kept_checked = true;
    let d = st.drawing();
    // Written as the save wrote it (the version and binary DXF asked for).
    let bytes = match WRITE_AS.get() {
        Some(write_as) if opts != SaveAs::default() => write_as(d, path, &opts),
        _ => (hooks.write)(d, path),
    };
    let Ok((lost, detail)) = bytes.and_then(|b| kept::check_written(d, path, &b, hooks.read)) else { return };
    if lost.is_empty() && detail.is_empty() {
        return;
    }
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_uppercase()).unwrap_or_else(|| "DXF".into());
    st.exchange_notice = Some(json!({ "path": path, "format": ext, "losses": detail }));
    let what = if detail.is_empty() { lost.join(", ") } else { kept::losses_line(&detail) };
    s.echo(format!("Not kept in the {ext} file: {what}. Save as a CADaddy drawing (.cdy) to keep everything."));
    if let Some(o) = r.as_object_mut() {
        if !lost.is_empty() {
            o.insert("notKept".into(), json!(lost));
        }
        o.insert("notKeptDetail".into(), json!(detail));
    }
}

/// CHECKEXCHANGE: what saving as DWG or DXF wouldn't keep, without saving.
fn run_checkexchange(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "checkexchange";
    let hooks = io().ok_or_else(|| crate::EngineError::Other("File formats are not available in this build.".into()))?;
    let from_path = s
        .state()?
        .path
        .as_deref()
        .filter(|p| is_exchange_file(p))
        .and_then(|p| std::path::Path::new(p).extension())
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    let format = match str_param(p, "format").map(str::to_ascii_lowercase) {
        Some(f) if matches!(f.as_str(), "dwg" | "dxf") => f,
        Some(f) => return Err(bad(cmd, format!("`format` is dwg or dxf, not {f}"))),
        None => match from_path.as_deref() {
            Some("dxf") => "dxf".to_string(),
            _ => "dwg".to_string(),
        },
    };
    let version = match p.get("version") {
        None | Some(Value::Null) => None,
        Some(v) => Some(save_options(cmd, &json!({ "version": v }))?.version.ok_or_else(|| bad(cmd, "unknown `version`"))?),
    };
    let name = format!("check.{format}");
    let d = s.state()?.drawing();
    // The version asked for: written as the save would write it.
    let lost = match version {
        Some(v) => {
            let write_as = WRITE_AS.get().copied().ok_or_else(|| bad(cmd, "saving in another version is not available in this build"))?;
            let bytes = write_as(d, &name, &SaveAs { version: Some(v), binary: false }).map_err(|e| bad(cmd, e))?;
            let back = (hooks.read)(&bytes, &name).map_err(|e| bad(cmd, e))?;
            kept::losses_between(d, &back)
        }
        None => kept::losses(d, &name, hooks.write, hooks.read).map_err(|e| bad(cmd, e))?,
    };
    let upper = format.to_ascii_uppercase();
    let message = if lost.is_empty() {
        format!("A {upper} file keeps everything in this drawing.")
    } else {
        format!("Not kept in a {upper} file: {}. A CADaddy drawing (.cdy) keeps everything.", kept::losses_line(&lost))
    };
    s.echo(message.clone());
    Ok(json!({ "format": format, "version": version.map(|v| v.code()), "kept": lost.is_empty(), "losses": lost, "message": message }))
}

fn run_qsave_reporting(s: &mut Session, p: &Value) -> Result<Value> {
    save_reporting(s, p, run_save)
}

fn run_saveas_reporting(s: &mut Session, p: &Value) -> Result<Value> {
    save_reporting(s, p, run_saveas)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn active_title(s: &Session) -> String {
        s.state().unwrap().title.clone()
    }

    fn temp_script(name: &str, text: &str) -> String {
        let path = std::env::temp_dir().join(format!("cadaddy-script-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn script_command_runs_a_scr_file() {
        let path = temp_script("run.scr", "; a comment\ncircle 50,25 20\nrectang 0,0 100,60\n");
        let mut s = Session::new();
        let r = s.execute("script", &json!({ "path": path })).unwrap();
        assert_eq!(r["lines"], 3);
        assert_eq!(s.state().unwrap().doc.model.len(), 2, "the comment is skipped, circle and rectang draw");
    }

    #[test]
    fn script_command_reports_a_missing_file() {
        let mut s = Session::new();
        let e = s.execute("script", &json!({ "path": "no/such/script.scr" })).unwrap_err().to_string();
        assert!(e.contains("no/such/script.scr"), "{e}");
        assert_eq!(s.state().unwrap().doc.model.len(), 0);
    }

    /// A script that runs itself must be refused, not recurse until the stack dies.
    #[test]
    fn a_script_cannot_start_a_script() {
        let inner = temp_script("inner.scr", "circle 0,0 1\n");
        let outer = temp_script("outer.scr", &format!("script {}\ncircle 0,0 2\n", json!({ "path": inner })));
        let mut s = Session::new();
        let e = s.execute("script", &json!({ "path": outer })).unwrap_err().to_string();
        assert!(e.contains("already running"), "{e}");
    }

    #[test]
    fn typed_script_prompts_for_the_file_then_runs_or_reports_it() {
        let good = temp_script("typed.scr", "circle 0,0 1\n");
        let mut s = Session::new();
        s.cmdline("script").unwrap();
        assert!(s.running.is_some(), "SCRIPT asks for the file name");
        s.cmdline(&good).unwrap();
        assert!(s.running.is_none());
        assert_eq!(s.state().unwrap().doc.model.len(), 1);

        // A file that cannot be read: the error is echoed, the command still ends.
        s.cmdline("script").unwrap();
        s.cmdline("no/such/script.scr").unwrap();
        assert!(s.running.is_none());
        assert!(s.log.iter().any(|l| l.contains("no/such/script.scr")), "{:?}", s.log);
    }

    #[test]
    fn closing_earlier_inactive_drawing_keeps_active_drawing() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        s.execute("new", &json!({})).unwrap();
        s.execute("document.switch", &json!({"index": 1})).unwrap();
        assert_eq!(active_title(&s), "Drawing2");
        s.execute("close", &json!({"index": 0})).unwrap();
        assert_eq!(active_title(&s), "Drawing2");
        assert_eq!(s.docs.len(), 2);
    }

    #[test]
    fn closing_later_inactive_drawing_keeps_active_drawing() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        s.execute("new", &json!({})).unwrap();
        s.execute("document.switch", &json!({"index": 1})).unwrap();
        s.execute("close", &json!({"index": 2})).unwrap();
        assert_eq!(active_title(&s), "Drawing2");
    }
}

#[cfg(test)]
mod pr193_tests {
    use super::*;

    #[test]
    fn failed_store_keeps_unsaved_changes_and_delivered_save_clears_them() {
        let mut s = Session::new();
        s.cmdline("circle 4,4 1").unwrap();
        let st = s.state().unwrap();
        let title = st.title.clone();
        assert!(st.is_dirty() && st.path.is_none());

        // The file route is unavailable or fails (the web without a download): no success, still dirty.
        let failed = commit_save(&mut s, "a/Owned.dxf", b"0\nEOF\n", |_, _| Err(bad("save", "no download")));
        assert!(failed.is_err());
        let st = s.state().unwrap();
        assert!(st.is_dirty(), "a save that delivered nothing must keep the unsaved state");
        assert_eq!((st.path.as_deref(), st.title.as_str()), (None, title.as_str()));

        // The file route took the bytes (written, or a download started): saved under that name.
        let mut got = None;
        let n = commit_save(&mut s, "a/Owned.dxf", b"0\nEOF\n", |p, b| {
            got = Some((p.to_string(), b.len()));
            Ok(())
        })
        .unwrap();
        assert_eq!((n, got), (6, Some(("a/Owned.dxf".to_string(), 6))));
        let st = s.state().unwrap();
        assert!(!st.is_dirty());
        assert_eq!((st.path.as_deref(), st.title.as_str()), (Some("a/Owned.dxf"), "Owned.dxf"));
        assert_eq!(saved("a/Owned.dxf", &Saved { bytes: n, copy: None, incremental: false })["bytes"], 6);
    }
}

/// SAVEAS typed with FILEDIA 0 (or without a file picker): the file format, its version for
/// DWG and DXF (Release 12 too, for DXF), then the file name, each with the current one as the
/// default (Enter keeps it).
#[derive(Default)]
pub(crate) struct SaveAsM {
    /// The format chosen (`cdy`, `dwg`, `dxf`, `dwt`, `dws`); `None` until asked.
    format: Option<&'static str>,
    /// The version chosen; asked after a DWG-type format.
    version: Option<Version>,
}

/// The formats SAVEAS offers on the command line: (extension, keyword).
const SAVE_FORMATS: [(&str, &str); 5] = [("cdy", "Cdy"), ("dwg", "Dwg"), ("dxf", "dXf"), ("dwt", "Template"), ("dws", "Standards")];

impl SaveAsM {
    /// The drawing's current format and version (what Enter keeps).
    fn current(s: &Session) -> (&'static str, Version) {
        let path = s.state().ok().and_then(|st| st.path.clone()).unwrap_or_default();
        let ext = std::path::Path::new(&path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let fmt = SAVE_FORMATS.iter().map(|(e, _)| *e).find(|e| *e == ext).unwrap_or_else(default_ext);
        (fmt, drawing_version(s, &format!("x.{fmt}")))
    }

    /// The suggested file: the drawing's file (or its title) with the chosen extension.
    fn suggested(s: &Session, fmt: &str) -> String {
        let base = s.state().ok().map(|st| st.path.clone().unwrap_or_else(|| st.title.clone())).unwrap_or_else(|| "Drawing".into());
        std::path::Path::new(&base).with_extension(fmt).to_string_lossy().into_owned()
    }

    fn versions(fmt: &str) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = Version::ALL.iter().rev().map(|v| v.year()).collect();
        if fmt == "dxf" {
            v.push("R12");
        }
        v
    }
}

impl crate::Interactive for SaveAsM {
    fn name(&self) -> &'static str {
        "SAVEAS"
    }
    fn prompt(&self, s: &Session) -> crate::Prompt {
        let (cur_fmt, cur_ver) = Self::current(s);
        let mut p = match (self.format, self.version) {
            (None, _) => {
                crate::Prompt::new(format!("Current format: {} {}. File format", cur_fmt.to_ascii_uppercase(), cur_ver.year()), crate::Accept::TEXT)
                    .kw(&SAVE_FORMATS.map(|(_, k)| k))
            }
            (Some(fmt), None) if fmt != "cdy" => crate::Prompt::new("Version", crate::Accept::TEXT).kw(&Self::versions(fmt)),
            (Some(fmt), _) => crate::Prompt::new("Save drawing as", crate::Accept::TEXT).default(Self::suggested(s, fmt)),
        };
        if p.default.is_none() {
            p.default = Some(match self.format {
                None => SAVE_FORMATS.iter().find(|(e, _)| *e == cur_fmt).map_or("Cdy", |(_, k)| *k).to_string(),
                Some(_) => cur_ver.year().to_string(),
            });
        }
        p
    }
    fn input(&mut self, s: &mut Session, i: crate::Input) -> Result<crate::Step> {
        use crate::{Input, Step};
        let text = match i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Enter => None,
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().trim_matches('"').to_string()).filter(|t| !t.is_empty()),
            _ => return Ok(Step::Continue),
        };
        let (cur_fmt, cur_ver) = Self::current(s);
        match self.format {
            None => {
                let t = text.map(|t| t.to_ascii_lowercase());
                let fmt = match t.as_deref() {
                    None => Some(cur_fmt),
                    // The extension or the keyword (the command line expands a keyword's letters).
                    Some(t) => SAVE_FORMATS.iter().find(|(e, k)| *e == t || k.eq_ignore_ascii_case(t)).map(|(e, _)| *e),
                };
                match fmt {
                    Some(f) => {
                        self.format = Some(f);
                        if f == "cdy" {
                            self.version = Some(cur_ver);
                        }
                        Ok(Step::Continue)
                    }
                    None => Err(bad("saveas", "choose a file format: CDY, DWG, DXF, Template (DWT) or Standards (DWS)")),
                }
            }
            Some(fmt) if self.version.is_none() => {
                let v = match &text {
                    None => Some(if fmt == "dxf" || cur_ver != Version::R12 { cur_ver } else { Version::default() }),
                    Some(t) => Version::parse(t).filter(|v| *v != Version::R12 || fmt == "dxf"),
                };
                match v {
                    Some(v) => {
                        self.version = Some(v);
                        Ok(Step::Continue)
                    }
                    None => Err(bad("saveas", format!("choose a version: {}", Self::versions(fmt).join(", ")))),
                }
            }
            Some(fmt) => {
                let mut path = text.unwrap_or_else(|| Self::suggested(s, fmt));
                if std::path::Path::new(&path).extension().is_none() {
                    path = format!("{path}.{fmt}");
                }
                let mut p = json!({ "path": path });
                if let Some(v) = self.version.filter(|_| fmt != "cdy") {
                    p["version"] = json!(v.year());
                }
                run_saveas_reporting(s, &p)?;
                Ok(Step::Done)
            }
        }
    }
}

/// A file command typed with FILEDIA 0 (scripts, or no file picker): the file name on the command
/// line, then the command, as AutoCAD asks when its dialogs are off.
pub(crate) struct FileM {
    pub name: &'static str,
    pub message: &'static str,
    pub run: fn(&mut Session, &Value) -> Result<Value>,
}

impl crate::Interactive for FileM {
    fn name(&self) -> &'static str {
        self.name
    }
    fn prompt(&self, _s: &Session) -> crate::Prompt {
        crate::Prompt::new(self.message, crate::Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: crate::Input) -> Result<crate::Step> {
        use crate::{Input, Step};
        match i {
            Input::Cancel | Input::Enter => Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) => {
                let path = t.trim().trim_matches('"');
                if path.is_empty() {
                    return Ok(Step::Cancel);
                }
                (self.run)(s, &json!({ "path": path }))?;
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}
