//! File menu: new, close, switching drawings. Open/save of .cdy, DWG and DXF go through the io layer,
//! registered by the host (desktop/CLI) via [`crate::cmd::file::set_io`]. On the web, where there is
//! no file system, saved files reach the user through the host's [`crate::cmd::file::set_deliver`].

mod etransmit;
pub mod kept;
mod pgp;
pub(crate) mod sheetset;
mod template;

use std::sync::OnceLock;

use cadaddy_doc::Drawing;
pub use cadaddy_dxf::Version;
use serde_json::{Value, json};

use super::*;

pub use super::exchange::guard_image_output;
use crate::{Accept, Input, Prompt, Result, Session, Step};

pub use pgp::{alias_conflicts, alias_file, built_in as built_in_aliases, user_aliases};
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
    /// DWFATTACH (and DWF underlays drawn after a reopen): one page of a DWFx/DWF file as a
    /// drawing in sheet inches, and a report (optional).
    pub read_dwf: Option<DwfReadHook>,
    /// DWFXOUT, DWFOUT and PUBLISH to DWF: spaces as one DWFx or DWF 6 file (optional).
    pub write_dwfx: Option<DwfxWriteHook>,
}

/// Read a PDF page with JSON options (`{page?, scale?, layers?, whiteFills?, text?, join?,
/// layout?}`) → the drawing and a report (scale used, counts, notes).
pub type PdfHook = fn(&[u8], &Value) -> std::result::Result<(Drawing, Value), String>;

/// Read one page (1-based) of a DWFx or DWF file as a drawing in sheet inches, with a report:
/// `{sheet: [w, h] inches, pages, name?, origin?: [x, y], scale?}` (`origin`/`scale`: where the
/// page was drawn from, when the file records it).
pub type DwfReadHook = fn(&[u8], u32) -> std::result::Result<(Drawing, Value), String>;

/// Write spaces of a drawing as one DWFx or DWF 6 file, one sheet each, with options
/// `{format?: "dwfx" | "dwf", scale?: drawing units per sheet inch, fit?: bool}`.
pub type DwfxWriteHook = fn(&Drawing, &[cadaddy_doc::Space], &Value) -> std::result::Result<Vec<u8>, String>;

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
pub mod convert;
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

/// Files larger than this are identified by size, modification time and inode only (no content
/// hash): hashing them on every save would cost more than the save.
const HASH_LIMIT: u64 = 64 << 20;

/// What the file at a drawing's path looked like when the drawing was read from it or last saved
/// there. A save to the same path first checks the file is still this: anything else that wrote
/// it in between (another session, another program) would be overwritten without a word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskId {
    path: String,
    len: u64,
    modified: Option<std::time::SystemTime>,
    inode: Option<u64>,
    /// A hash of the contents (files up to [`HASH_LIMIT`]): a file only touched is not changed.
    hash: Option<u64>,
}

/// How the file a drawing came from stands now.
#[derive(Debug, PartialEq, Eq)]
pub enum DiskState {
    /// As it was (or no record to compare with).
    Same,
    /// No longer there (deleted or moved): saving writes it again.
    Gone,
    /// Changed by someone else; says how.
    Changed(String),
}

fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn inode_of(m: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(m.ino())
}
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
fn inode_of(_: &std::fs::Metadata) -> Option<u64> {
    None
}

impl DiskId {
    /// The file at `path` now; `bytes` are its contents when the caller has them (saves the read
    /// for the hash).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn of(path: &str, bytes: Option<&[u8]>) -> Option<DiskId> {
        let m = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
        let hash = match bytes {
            Some(b) if b.len() as u64 == m.len() => Some(content_hash(b)),
            _ if m.len() <= HASH_LIMIT => std::fs::read(path).ok().map(|b| content_hash(&b)),
            _ => None,
        };
        Some(DiskId { path: path.to_string(), len: m.len(), modified: m.modified().ok(), inode: inode_of(&m), hash })
    }
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn of(_path: &str, _bytes: Option<&[u8]>) -> Option<DiskId> {
        None
    }

    /// Whether the file at `path` is still what this describes. Only a save to the same path is
    /// compared.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn state(&self, path: &str) -> DiskState {
        if !same_file(&self.path, path) {
            return DiskState::Same;
        }
        let Some(m) = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file) else { return DiskState::Gone };
        let size = |a: u64, b: u64| format!("its size went from {a} to {b} bytes");
        if m.len() != self.len {
            return DiskState::Changed(size(self.len, m.len()));
        }
        if self.inode.is_some() && inode_of(&m) != self.inode {
            return DiskState::Changed("the file was replaced by another one".into());
        }
        if m.modified().ok() == self.modified {
            return DiskState::Same;
        }
        // Same size, newer stamp: the contents decide (a file only touched is not changed).
        match (self.hash, std::fs::read(path).ok().map(|b| content_hash(&b))) {
            (Some(was), Some(now)) if was == now => DiskState::Same,
            _ => DiskState::Changed("its contents changed".into()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    pub fn state(&self, _path: &str) -> DiskState {
        DiskState::Same
    }
}

thread_local! {
    /// `force: true` on the save being run: overwrite a file changed on disk.
    static FORCE_SAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the `force` parameter of `p` in effect for the saves inside it.
pub(super) fn with_force<T>(p: &Value, f: impl FnOnce() -> T) -> T {
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let before = FORCE_SAVE.replace(force);
    let r = cadaddy_dxf::with_oversize_allowed(force, f);
    FORCE_SAVE.set(before);
    r
}

/// The refusal to save over `path` when it changed on disk since the drawing was read or saved,
/// unless the save is forced; a deleted file is saved with a note.
fn check_disk(s: &Session, cmd: &str, path: &str) -> Result<Option<String>> {
    let Some(id) = s.state()?.disk_id.as_ref() else { return Ok(None) };
    match id.state(path) {
        DiskState::Same => Ok(None),
        DiskState::Gone => Ok(Some(format!("{} was deleted or moved since it was opened; it is written again.", file_name(path)))),
        DiskState::Changed(how) if FORCE_SAVE.get() => {
            Ok(Some(format!("{} changed on disk ({how}); overwritten because force was given.", file_name(path))))
        }
        DiskState::Changed(how) => Err(bad(
            cmd,
            format!(
                "{path} changed on disk since it was opened or last saved ({how}). Saving would overwrite that. \
                 Save under another name (saveas), reopen it, or pass force:true to overwrite it."
            ),
        )),
    }
}

impl Session {
    /// How the active drawing's file stands on disk, when someone else changed it since the
    /// drawing was read or saved (the question put to the user before an agent saves).
    pub fn changed_on_disk(&self, path: &str) -> Option<String> {
        match self.state().ok()?.disk_id.as_ref()?.state(path) {
            DiskState::Changed(how) => Some(how),
            _ => None,
        }
    }
}

/// Append a .cdy save to `path`: the chunks, made durable, then the index and trailer. A file no
/// longer `from` bytes long is left alone; a failed append cuts the file back to what it was, so
/// it still holds the previous save.
pub(crate) fn append_cdy(path: &str, from: u64, bytes: &[u8], index_at: usize) -> Result<()> {
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
    // A drawing file is the drawing outside the Block Editor; an image is what is shown.
    let d: &cadaddy_doc::Drawing = if is_drawing_file(path) { s.state()?.drawing() } else { s.doc()? };
    write_drawing(d, cmd, path, opts)
}

/// [`write_bytes`] for a drawing that isn't open (DWGCONVERT's).
fn write_drawing(d: &cadaddy_doc::Drawing, cmd: &str, path: &str, opts: &SaveAs) -> Result<Vec<u8>> {
    let hooks = io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    if opts.binary && !is_dxf(path) {
        return Err(bad(cmd, "binary applies to DXF files only"));
    }
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
            .files()
            .menu(&["File", "New Drawing..."])
            .key("Cmd+N")
            .alias(&["qnew"])
            .params("{metric?: bool (default MEASUREINIT), template?: path to a .dwt/.dxf template (\".\" for none; default QNEWTEMPLATE)}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("open", "Open...", run_open)
            .menu(&["File", "Open..."])
            .key("Cmd+O")
            .params("{path} | {data: base64, name} → {index, entities?, warnings: what reading left out}")
            .enabled(always)
            .noundo()
            .interactive(|_| Ok(Box::new(FileM { name: "OPEN", message: "Enter name of drawing to open", run: run_open }))),
        CommandSpec::new("close", "Close", run_close_guarded)
            .files().menu(&["File", "Close"]).key("Cmd+W").params("{index?, save?: bool, discard?: bool (agents, when no one can be asked: a drawing with unsaved changes is closed only with one of them)}").noundo(),
        CommandSpec::new("closeall", "Close All", run_closeall_guarded)
            .files().menu(&["File", "Close All"]).enabled(always).params("{save?: bool, discard?: bool (agents, when no one can be asked)}").noundo(),
        CommandSpec::new("qsave", "Save", run_qsave_reporting)
            .files().menu(&["File", "Save"]).key("Cmd+S").alias(&["save"]).params("{path?, force?: bool (overwrite the file even when it changed on disk since the drawing was opened or last saved, and write a DXF/DWG too big for CADaddy to reopen; otherwise that save is refused)} → {path, bytes, note?}").noundo(),
        CommandSpec::new("saveas", "Save As...", run_saveas_reporting)
            .menu(&["File", "Save As..."])
            .key("Cmd+Shift+S")
            .params("{path (.cdy: CADaddy's own format, keeps everything; .dwg, .dxf, .dwt), version?: 2000|2004|2007|2010|2013|2018 (DWG/DXF), 12 (Release 12 DXF: newer objects converted, some left out), binary?: bool (DXF), force?: bool (overwrite the file even when it changed on disk since the drawing was opened or last saved there, and write a DXF/DWG too big for CADaddy to reopen; otherwise refused)} → {path, bytes, version?, notKept?: what DWG/DXF wouldn't keep (said once per drawing), dwgCopy?: the DWG copy DWGCOPY saved, dwgCopyKept?: where an existing .dwg that wasn't this drawing's copy was kept first (<name>.dwg.bak), dwgCopyError?: why the copy wasn't saved}")
            .noundo()
            .interactive(|_| Ok(Box::new(SaveAsM::default()))),
        CommandSpec::new("checkexchange", "Check Exchange", run_checkexchange)
            .files()
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
            .files()
            .params("{index? | path? | title? (default the current drawing), save?: bool (save to its file first), saveAs?: path, discard?: bool} → {closed, saved?}. A drawing with unsaved changes is closed only with save, saveAs or discard (CLOSE itself discards)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("document.closeall", "Close All Drawings", run_closeall_docs)
            .files()
            .params("{save?: bool, discard?: bool} → {closed: [titles], saved: [paths]}. Refused, closing nothing, while a drawing has unsaved changes and neither is given (or a drawing to save has no file name)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("document.bytes", "Drawing as bytes", run_bytes)
            .params("{format?: dxf (or cdy, dwg), version?: 2000…2018, binary?: bool} → {data: base64}")
            .noundo(),
        CommandSpec::new("script", "Run Script...", run_script)
            .files()
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
            s.check_load(std::path::Path::new(path), "SCRIPT", Some(("script".into(), json!({ "path": path }))))?;
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
            Err(EngineError::BadParams { msg, .. } | EngineError::Other(msg)) => {
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
    let metric = s.settings.measureinit;
    let i = s.new_drawing_with_viewports(bool_or(p, "metric", metric));
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
    /// The file as it was when read, to notice something else changing it before a save.
    disk: Option<DiskId>,
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

/// This session's own folder in the system temp folder, for image files a drawing with no file
/// yet needs (PDFIMPORT's). `None` where there is no file system (the web).
fn session_image_root() -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(std::env::temp_dir().join(format!("cadaddy-images-{}", std::process::id())))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// A new folder name for the image files of one PDFIMPORT into a drawing with no file yet: in
/// this session's temp folder, never next to the PDF (often the job's print set). Each import gets
/// its own, so two PDFs of the same name never overwrite each other's images. SAVE moves them next
/// to the drawing ([`adopt_session_images`]). `None` on the web (the images stay in memory).
pub fn session_image_folder(stem: &str) -> Option<std::path::PathBuf> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let clean: String = stem.chars().take(80).map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
    session_image_root().map(|root| root.join(format!("{n} {} images", clean.trim())))
}

/// Before the drawing is saved at `path`: the image files PDFIMPORT left in this session's temp
/// folder ([`session_image_folder`]) are copied into "<drawing name> images" next to it, and the
/// IMAGE objects point there, so the saved drawing never refers to a temp file. A file that can't
/// be copied stays where it was; the returned notes say which.
pub fn adopt_session_images(s: &mut Session, path: &str) -> Vec<String> {
    let Some(root) = session_image_root() else { return Vec::new() };
    if !is_drawing_file(path) || !crate::agent::writes_allowed() {
        return Vec::new();
    }
    let Ok(d) = s.doc() else { return Vec::new() };
    let found: Vec<(cadaddy_doc::Handle, String)> = d
        .model
        .iter()
        .chain(d.layouts.iter().flat_map(|l| l.entities.iter()))
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Image(i) if std::path::Path::new(&i.path).starts_with(&root) => Some((e.handle, i.path.clone())),
            _ => None,
        })
        .collect();
    if found.is_empty() {
        return Vec::new();
    }
    let full = std::path::absolute(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let stem = full.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Drawing".into());
    let dir = full.parent().unwrap_or_else(|| std::path::Path::new(".")).join(format!("{stem} images"));
    let mut notes = Vec::new();
    let mut moved: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    for (h, old) in found {
        let new = match moved.get(&old) {
            Some(n) => n.clone(),
            None => {
                let n = match copy_into(std::path::Path::new(&old), &dir) {
                    Ok(n) => Some(n.to_string_lossy().to_string()),
                    Err(e) => {
                        notes.push(format!("{old} couldn't be copied next to the drawing ({e}); the image still points at the temp folder"));
                        None
                    }
                };
                moved.insert(old.clone(), n.clone());
                n
            }
        };
        let Some(new) = new else { continue };
        if let Ok(d) = s.doc_mut() {
            let _ = d.modify_entity(h, |e| {
                if let cadaddy_doc::EntityKind::Image(i) = &mut e.kind {
                    i.path = new.clone();
                }
            });
        }
    }
    notes
}

/// Copy the file `from` into the folder `dir` (made if needed) under its own name, or, when a
/// different file already has that name there, under the first free "<name>-2", "-3"…; the path
/// it now has.
fn copy_into(from: &std::path::Path, dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let bytes = std::fs::read(from)?;
    crate::writefile::check(dir)?;
    std::fs::create_dir_all(dir)?;
    let name = from.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
    let ext = from.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for k in 1..=999u32 {
        let to = dir.join(if k == 1 { format!("{name}{ext}") } else { format!("{name}-{k}{ext}") });
        match std::fs::read(&to) {
            Ok(have) if have == bytes => return Ok(to),
            Ok(_) => continue,
            Err(_) => {
                crate::writefile::write(&to, &bytes)?;
                return Ok(to);
            }
        }
    }
    Err(std::io::Error::other(format!("no free file name for {name}{ext} in {}", dir.display())))
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
    let ReadFile { drawing, binary_dxf, name, path, cdy, disk, walkthrough } = f;
    if let Some(i) = path.as_deref().and_then(|p| already_open(s, p)) {
        return Ok(switch_to_open(s, i));
    }
    let is_pdf = name.rsplit('.').next().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    let i = add_read(s, drawing, binary_dxf, &name, path, (cdy, disk));
    mark_walkthrough(s, i, walkthrough);
    super::blocks::xref::after_open(s, i);
    crate::activity::after_open(s, i);
    s.rhai_drawing_opened(i);
    // Auto-run import accuracy check when opening a PDF file.
    if is_pdf && let Ok(d) = s.doc() {
        let check = super::import_check::check_drawing(d);
        if let Ok(d) = s.doc_mut() {
            super::import_check::store_result(d, &check);
        }
        if !check.pass {
            let fails: Vec<&str> = check.checks.iter().filter(|c| !c.pass).map(|c| c.id.as_str()).collect();
            s.echo(format!("Import check FAIL (score {}/100): {}", check.score, fails.join(", ")));
        }
    }
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
    let disk = path.as_deref().and_then(|p| DiskId::of(p, Some(bytes)));
    Ok(ReadFile { drawing, binary_dxf: cadaddy_dxf::is_binary(bytes), name: name.to_string(), path, cdy, disk, walkthrough })
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

fn add_read(
    s: &mut Session,
    mut drawing: Drawing,
    binary_dxf: bool,
    name: &str,
    path: Option<String>,
    (cdy, disk): (Option<CdyState>, Option<DiskId>),
) -> usize {
    let reset = crate::audit::current_styles(&mut drawing);
    // Every way a drawing is opened (the OPEN command, the background open, scripts) looks for
    // its relative image paths in its folder first, and images that moved are looked up again.
    image_folder(path.as_deref());
    cadaddy_render::images::revalidate();
    let notes = drawing.read_notes.clone();
    let loss = read_loss(&drawing);
    let has_path = path.is_some();
    let cdy_saved = path.as_deref().zip(cdy).and_then(|(p, state)| CdySaved::new(p, state));
    // Table formulas whose values the file didn't keep (a DWG), worked out: what they show isn't
    // an edit.
    let mut drawing = drawing;
    crate::cmd::table::cells::fill_missing_values(&mut drawing);
    let i = s.open_drawing(drawing, &file_name(name), path);
    if let Some(st) = s.docs.get_mut(i) {
        // QSAVE keeps a binary DXF binary.
        st.binary_dxf = binary_dxf;
        st.read_loss = loss.filter(|_| has_path);
        st.open_notes = reset.clone();
        st.cdy_saved = cdy_saved;
        st.disk_id = disk;
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
    let ReadFile { drawing, binary_dxf, name, path, cdy, disk, walkthrough } = read_bytes(bytes, name, path)?;
    let i = add_read(s, drawing, binary_dxf, &name, path, (cdy, disk));
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
            s.rhai_drawing_opened(i);
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
    s.rhai_drawing_opened(i);
    Ok(opened(s, i, false))
}

/// CLOSE as a command (typed, LISP, script, JSON): an agent's close of a drawing with changes
/// waits for the user ([`Session::guard_run`]). The close tools and the code that has already
/// decided call [`run_close`].
fn run_close_guarded(s: &mut Session, p: &Value) -> Result<Value> {
    s.guard_run("close", p)?;
    run_close(s, p)
}

fn run_closeall_guarded(s: &mut Session, p: &Value) -> Result<Value> {
    s.guard_run("closeall", p)?;
    run_closeall(s, p)
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
/// (gigabytes for a large one) until the program exited. The closed drawings' LISP
/// environments go too.
fn release_closed(s: &mut Session) {
    crate::spatial::clear_cache();
    s.lisp_forget_closed();
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
    /// The DWG copy replaced a `.dwg` the drawing hadn't written: where that file was kept.
    copy_kept: Option<String>,
    /// Appended to the .cdy file (only what changed) rather than written in full.
    incremental: bool,
    /// Something worth telling: the file was deleted since it was opened, or was overwritten by force.
    note: Option<String>,
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
    // A file someone else changed since this drawing was read or saved would be overwritten
    // without a word: refuse unless forced.
    let note = if is_drawing_file(path) { check_disk(s, cmd, path)? } else { None };
    if let Some(n) = &note {
        s.echo(n.clone());
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
        // PDF images still in the session's temp folder go with the drawing.
        for note in adopt_session_images(s, path) {
            s.echo(note);
        }
    }
    let (n, incremental) = match save_cdy(s, path, opts)? {
        Some(r) => r,
        None => {
            let bytes = write_bytes(s, cmd, path, opts)?;
            let n = commit_save(s, path, &bytes, store)?;
            // The first DWG/DXF save checks what the file keeps: it reads these bytes back
            // instead of writing the drawing a second time.
            if is_exchange_file(path) && KEEP_WRITTEN.get() && s.state().is_ok_and(|st| !st.kept_checked.contains(&kept_key(s, path, *opts))) {
                JUST_WRITTEN.with(|w| *w.borrow_mut() = Some((path.to_string(), bytes)));
            }
            (n, false)
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
    let mut copy_kept = None;
    let copy = (is_cdy(path) && dwg_copy()).then(|| save_dwg_copy(s, cmd, path, &mut copy_kept));
    Ok(Saved { bytes: n, copy, copy_kept, incremental, note })
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
fn save_dwg_copy(s: &mut Session, cmd: &str, path: &str, kept: &mut Option<String>) -> std::result::Result<String, String> {
    let copy = std::path::Path::new(path).with_extension("dwg").to_string_lossy().into_owned();
    let r = guard_dwg_copy(s, &copy).and_then(|keep| {
        let bytes = write_bytes(s, cmd, &copy, &SaveAs::default()).map_err(|e| e.to_string())?;
        if let Some(bak) = &keep {
            crate::writefile::copy(&copy, bak).map_err(|e| format!("couldn't keep the existing {} as {}: {e}", file_name(&copy), file_name(bak)))?;
        }
        store_keeping(&copy, &bytes, false).map_err(|e| e.to_string())?;
        *kept = keep;
        Ok(())
    });
    match r {
        Ok(()) => {
            if let Some(bak) = kept.as_deref() {
                s.echo(format!("{} already existed and wasn't a copy of this drawing: kept as {}", file_name(&copy), file_name(bak)));
            }
            if let Ok(st) = s.state_mut() {
                st.dwg_copy_written = Some(copy.clone());
            }
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

/// Whether the DWG copy may be written to `copy`, and where an existing file there must be kept
/// first. A `.dwg` that is not the copy this drawing last wrote (the file it was opened from, or
/// anyone's) is kept as `<name>.dwg.bak`, distinct from the drawing's own `<name>.bak`; a copy
/// open in another tab, or a `.dwg.bak` already in the way, stops the copy with how to go on.
fn guard_dwg_copy(s: &Session, copy: &str) -> std::result::Result<Option<String>, String> {
    let active = s.active;
    if let Some((_, other)) = s.docs.iter().enumerate().find(|(i, d)| *i != active && d.path.as_deref().is_some_and(|p| same_file(p, copy))) {
        return Err(format!("{} is open in another tab ({}). Close it first, or turn the DWG copy off (DWGCOPY 0).", file_name(copy), other.title));
    }
    if !std::fs::metadata(copy).is_ok_and(|m| m.is_file()) {
        return Ok(None);
    }
    let ours = s.state().ok().and_then(|st| st.dwg_copy_written.as_deref()).is_some_and(|w| same_file(w, copy));
    if ours {
        return Ok(None);
    }
    let bak = format!("{copy}.bak");
    if std::fs::symlink_metadata(&bak).is_ok() {
        return Err(format!(
            "{} already exists and isn't a copy this drawing wrote, and {} is already taken. Move or rename one of them, or turn the DWG copy off (DWGCOPY 0).",
            file_name(copy),
            file_name(&bak)
        ));
    }
    Ok(Some(bak))
}

/// Put saved bytes where the user gets them: natively the file at `path` (written atomically); on
/// the web, which has no file system, a browser download named after `path` via [`deliver`].
/// Where saving over a drawing keeps the previous file (ISAVEBAK): [`backup_path`].
fn bak_path(path: &str) -> String {
    backup_path(std::path::Path::new(path)).to_string_lossy().into_owned()
}

/// The longest file name (one path component, in bytes) the file systems CADaddy saves to accept.
const MAX_NAME_BYTES: usize = 255;

/// An error naming the limit when the file name (or the `.bak` kept beside it) is longer than a
/// file system takes, instead of the operating system's "File name too long (os error 63)".
fn name_fits(path: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    let len = |p: &std::path::Path| p.file_name().map_or(0, |n| n.len());
    let longest = len(p).max(len(std::path::Path::new(&bak_path(path))));
    if longest > MAX_NAME_BYTES {
        return Err(bad(
            "save",
            format!("the file name is too long ({longest} bytes; the limit is {MAX_NAME_BYTES}). Save it under a shorter name."),
        ));
    }
    Ok(())
}

pub(crate) fn store(path: &str, bytes: &[u8]) -> Result<()> {
    store_keeping(path, bytes, true)
}

/// [`store`], keeping the previous drawing file as `.bak` only when `keep_bak`: a DWG copy
/// (DWGCOPY) keeps none, as its `.bak` would replace the drawing's own.
pub(crate) fn store_keeping(path: &str, bytes: &[u8], keep_bak: bool) -> Result<()> {
    crate::writefile::protected(std::path::Path::new(path)).map_err(|e| bad("save", e.to_string()))?;
    name_fits(path)?;
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
        // A drawing saved over keeps its previous version as .bak, as AutoCAD does (ISAVEBAK). A
        // file that is cut off never replaces a good .bak ([`backup`]).
        if keep_bak && before.is_some() && is_drawing_file(path) && crate::autosave::isavebak() {
            let bak = bak_path(path);
            let keep = || -> std::io::Result<()> {
                // An earlier read-only .bak is replaced, not refused (a rename can't replace one
                // on Windows).
                if let Ok(m) = std::fs::metadata(&bak)
                    && m.permissions().readonly()
                {
                    let mut p = m.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    p.set_readonly(false);
                    std::fs::set_permissions(&bak, p)?;
                }
                backup(std::path::Path::new(path))
            };
            keep().map_err(|e| bad("save", format!("{path}: couldn't keep the previous version as {bak} ({e}); nothing was saved")))?;
        }
        // A failed save leaves nothing behind but the file as it was; the new file keeps the
        // replaced one's permissions.
        crate::writefile::write_atomic(path, bytes).map_err(|e| bad("save", format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = keep_bak;
        let name = file_name(path);
        let deliver = deliver().ok_or_else(|| bad("save", "saving files is not available in this build; use document.bytes"))?;
        deliver(&name, "application/octet-stream", bytes).map_err(|e| bad("save", format!("{name}: {e}")))
    }
}

/// The backup of the drawing file `path`, beside it. A DWG's is `<name>.bak`, as AutoCAD names it,
/// in the extension's letter case (`plan.DWG` to `plan.BAK`); any other drawing appends `.bak`
/// (`plan.dxf.bak`), so `plan.dwg` and `plan.dxf` in one folder never overwrite each other's backup.
pub fn backup_path(path: &std::path::Path) -> std::path::PathBuf {
    match path.extension().map(|e| e.to_string_lossy()) {
        Some(e) if e.eq_ignore_ascii_case("dwg") => path.with_extension(
            e.chars().zip("bak".chars()).map(|(d, b)| if d.is_ascii_uppercase() { b.to_ascii_uppercase() } else { b }).collect::<String>(),
        ),
        _ => {
            let mut name = path.as_os_str().to_os_string();
            name.push(".bak");
            name.into()
        }
    }
}

/// Whether `bytes` read as a DWG or a DXF (ASCII or binary) by their start: what a save keeps as
/// the backup. A file that doesn't (truncated, overwritten by another program) never replaces a
/// good `.bak`.
pub fn looks_like_drawing(bytes: &[u8]) -> bool {
    // DWG: a version tag like `AC1032` (R12 is `AC1009`).
    let dwg = bytes.len() >= 6 && bytes.starts_with(b"AC") && bytes[2..6].iter().all(|b| b.is_ascii_digit());
    if dwg || cadaddy_dxf::is_binary(bytes) {
        return true;
    }
    // ASCII DXF: optional comments (999), then group 0 SECTION.
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    loop {
        match (lines.next(), lines.next()) {
            (Some("999"), Some(_)) => continue,
            (Some("0"), Some(v)) => return v.eq_ignore_ascii_case("SECTION"),
            _ => return false,
        }
    }
}

/// How much of a drawing's end [`looks_complete`] reads.
const TAIL_BYTES: u64 = 256;

/// Whether a drawing that [`looks_like_drawing`] (`head`, its first bytes, at least 256 of them
/// for a DWG) is whole, from its end (`tail`, its last bytes) and size `len`, without reading it
/// all: a DXF ends with `0`/`EOF` (a binary one with `EOF` and its NUL); a DWG reaches the end its
/// own header gives ([`dwg_complete`]). `None` when that can't be told (a 2007 DWG), so a file cut
/// off by a failed save or another program doesn't replace a good backup.
pub fn completeness(head: &[u8], tail: &[u8], len: u64) -> Option<bool> {
    let dwg = head.len() >= 6 && head.starts_with(b"AC") && head[2..6].iter().all(|b| b.is_ascii_digit());
    if dwg {
        // Too short for any DWG's header: cut off, not unknown.
        return if head.len() < 256 { Some(false) } else { dwg_complete(head, len) };
    }
    if cadaddy_dxf::is_binary(head) {
        return Some(tail.ends_with(b"EOF\0"));
    }
    let text = String::from_utf8_lossy(tail);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty()).rev();
    Some(matches!((lines.next(), lines.next()), (Some(last), Some("0")) if last.eq_ignore_ascii_case("EOF")))
}

/// [`completeness`] known to be whole.
pub fn looks_complete(head: &[u8], tail: &[u8], len: u64) -> bool {
    completeness(head, tail, len) != Some(false)
}

/// Whether a DWG of `len` bytes reaches the end its header gives (Open Design Specification for
/// .dwg files): from 2004 on, the copy of the file header that ends the file (its address is in
/// the encrypted header at 0x80, and the copy is 0x6C bytes); up to 2000, the end of every section
/// the locator records give. `None` for another layout (2007, or before R13).
fn dwg_complete(head: &[u8], len: u64) -> Option<bool> {
    let version = head.get(..6)?;
    match version {
        b"AC1018" | b"AC1024" | b"AC1027" | b"AC1032" => {
            // The header at 0x80 is XORed with the generator the specification gives.
            let enc = head.get(0x80..0x80 + 0x6C)?;
            let mut seed: u32 = 1;
            let h: Vec<u8> = enc
                .iter()
                .map(|b| {
                    seed = seed.wrapping_mul(0x343FD).wrapping_add(0x0026_9EC3);
                    b ^ (seed >> 16) as u8
                })
                .collect();
            if h.get(..12) != Some(&b"AcFssFcAJMB\0"[..]) {
                return Some(false);
            }
            let second = u64::from_le_bytes(h.get(0x34..0x3C)?.try_into().ok()?);
            Some(second >= 0x100 && second.checked_add(0x6C).is_some_and(|end| len >= end))
        }
        b"AC1012" | b"AC1014" | b"AC1015" => {
            let n = i32::from_le_bytes(head.get(0x15..0x19)?.try_into().ok()?);
            if !(1..=16).contains(&n) {
                return Some(false);
            }
            let mut end = 0u64;
            for i in 0..n as usize {
                let r = head.get(0x19 + i * 9..0x19 + i * 9 + 9)?;
                let seek = u64::from(u32::from_le_bytes(r.get(1..5)?.try_into().ok()?));
                let size = u64::from(u32::from_le_bytes(r.get(5..9)?.try_into().ok()?));
                end = end.max(seek + size);
            }
            Some(len >= end)
        }
        _ => None,
    }
}

/// A file's first bytes, last bytes and length, left positioned just after the first bytes.
#[cfg(not(target_arch = "wasm32"))]
fn ends(src: &mut std::fs::File) -> std::io::Result<(Vec<u8>, Vec<u8>, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut head = Vec::with_capacity(4096);
    (&mut *src).take(4096).read_to_end(&mut head)?;
    // Its end too (a DXF's EOF), then on from where the start left off for the copy.
    let len = src.metadata()?.len();
    let mut tail = Vec::new();
    src.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
    (&mut *src).take(TAIL_BYTES).read_to_end(&mut tail)?;
    src.seek(SeekFrom::Start(head.len() as u64))?;
    Ok((head, tail, len))
}

/// Whether saving replaces the `.bak` with this file. A file known to be cut off never replaces a
/// backup already there. One that can't be told (a 2007 DWG) does: it is most likely fine, and it
/// is about to be replaced. With no backup yet any file is kept, because the checks can be wrong
/// about a real file and this save replaces the only copy.
fn backup_replaced(head: &[u8], tail: &[u8], len: u64, backup_exists: bool) -> bool {
    (looks_like_drawing(head) && completeness(head, tail, len) != Some(false)) || (len > 0 && !backup_exists)
}

/// Whether a save over `path` now would leave the current file in its `.bak`, for a warning that
/// must not promise a backup the save won't write.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_keeps_current(path: &std::path::Path) -> bool {
    let Ok(mut src) = std::fs::File::open(path) else { return false };
    crate::autosave::isavebak() && ends(&mut src).is_ok_and(|(head, tail, len)| backup_replaced(&head, &tail, len, backup_path(path).exists()))
}

#[cfg(target_arch = "wasm32")]
pub fn save_keeps_current(_path: &std::path::Path) -> bool {
    false
}

/// Before `path` is replaced: copy it to its `.bak`, streamed and written atomically too
/// ([`crate::atomic_write::write_atomic_with`]). Nothing
/// to keep when there is no file yet. A file that doesn't look like a drawing
/// ([`looks_like_drawing`]) is not backed up, and the `.bak` already there is left alone.
#[cfg(not(target_arch = "wasm32"))]
fn backup(path: &std::path::Path) -> std::io::Result<()> {
    let mut src = match std::fs::File::open(path) {
        Ok(f) if f.metadata()?.is_file() => f,
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let (head, tail, len) = ends(&mut src)?;
    let bak = backup_path(path);
    if !backup_replaced(&head, &tail, len, bak.exists()) {
        log::warn!("save: {} doesn't look like a whole drawing; its backup is left as it was", path.display());
        return Ok(());
    }
    // The backup is no more readable than the drawing, nor than the .bak it replaces (a private
    // 0600 drawing or .bak stays private). The mode is set as the new file is created, so a
    // failed write leaves the old .bak as it was; a symlink at the .bak is replaced, never
    // written through.
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = src.metadata()?.permissions().mode() & 0o777;
        if let Ok(m) = std::fs::symlink_metadata(&bak)
            && m.is_file()
        {
            mode &= m.permissions().mode();
        }
        Some(std::fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let permissions = None;
    crate::atomic_write::write_atomic_with(&bak, permissions, |out| {
        std::io::Write::write_all(out, &head)?;
        std::io::copy(&mut src, out).map(|_| ())
    })
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
    // The file as this save left it: the next save there is checked against it. (A .cdy save may
    // have appended, so its bytes are not the file.)
    st.disk_id = DiskId::of(path, (!is_cdy(path)).then_some(bytes));
    // A BEgin group still open when the drawing is saved ends there.
    st.end_undo_group();
    Ok(bytes.len())
}

/// The save result; on the web it also names the file the browser downloaded.
fn saved(path: &str, sv: &Saved) -> Value {
    let mut r = json!({ "path": path, "bytes": sv.bytes });
    if let Some(n) = &sv.note {
        r["note"] = json!(n);
    }
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
    if let Some(bak) = &sv.copy_kept {
        r["dwgCopyKept"] = json!(bak);
    }
    r
}

fn run_save(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match str_param(p, "path") {
        Some(p) => p.to_string(),
        None => s.state()?.path.clone().ok_or_else(|| bad("qsave", "drawing has no file name yet; use saveas {path}"))?,
    };
    let sv = with_force(p, || save_to(s, &path))?;
    Ok(saved(&path, &sv))
}

/// A file name with no extension saves as a drawing in the default format ([`default_ext`]), as
/// AutoCAD adds `.dwg`.
pub(crate) fn with_drawing_ext(path: &str) -> String {
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
    let sv = with_force(p, || save_as(s, "saveas", &path, &opts))?;
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
    let v = s
        .state()
        .ok()
        .and_then(|st| st.drawing().header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str).and_then(Version::for_acadver))
        .unwrap_or_default();
    if is_dxf(path) { v } else { v.max(Version::R2004) }
}

pub(crate) fn run_switch(s: &mut Session, p: &Value) -> Result<Value> {
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
    } else if version >= Version::R2013 && is_dxf_file(format!("drawing.{fmt}")) {
        report_sat_text_dropped(s, &mut r);
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

/// Entity types of the DXF Reference that stay `Unknown` in a drawing but that both DWG writers
/// do write, with their frame and data bytes (tests `dwg_save_keeps_ole`): not counted as unsaved.
const WRITTEN_UNKNOWN_TYPES: [&str; 1] = ["OLE2FRAME"];

/// [`not_saved`] for a file saved as `version`. From 2013 on, a 3D solid, region or body the data
/// store has a record for keeps its geometry and is not counted; other programs' custom entities
/// (types outside the DXF Reference's, [`cadaddy_dxf::is_entity_type`]) are kept and not counted
/// either: a DWG keeps them as they were read (cadaddy-io's `dwg_keep`).
pub fn not_saved_as(d: &Drawing, version: Version) -> std::collections::BTreeMap<String, usize> {
    let mut out: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let stored = if version >= Version::R2013 { d.foreign.data_store_entities() } else { Default::default() };
    let stores = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)).chain(d.blocks.values().map(|b| &b.entities));
    for e in stores.flat_map(|s| s.iter()) {
        if let cadaddy_doc::EntityKind::Unknown(u) = &e.kind {
            // A DWG before 2013 writes a solid's SAT text as it is; from 2013 the geometry has to be
            // in the data store, and SAT text alone is lost (#2334).
            if MODELER_TYPES.iter().any(|t| u.dxf_type.eq_ignore_ascii_case(t))
                && (stored.contains(&e.handle) || (version < Version::R2013 && cadaddy_doc::foreign::has_sat_text(&u.tags)))
            {
                continue;
            }
            if !cadaddy_dxf::is_entity_type(&u.dxf_type) || WRITTEN_UNKNOWN_TYPES.iter().any(|t| u.dxf_type.eq_ignore_ascii_case(t)) {
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
    Some(format!(
        "Not saved in the DWG file (its writer can't write these object types yet; DXF keeps them, solids only when saved as 2010 or earlier): {}",
        list.join(", ")
    ))
}

/// What saving the active drawing to `path` would leave out, before anything is written: empty
/// when the format keeps everything. The interactive save asks with this; [`save_reporting`]
/// reports the same list afterwards to callers that never get a dialog.
pub fn loss_on_save(s: &Session, path: &str) -> std::collections::BTreeMap<String, usize> {
    if !drops_unknown(path) {
        return Default::default();
    }
    let version = drawing_version(s, path);
    s.doc().map(|d| not_saved_as(d, version)).unwrap_or_default()
}

/// Whether saving to `path` may leave out the entities [`not_saved`] counts: DWG (the DWG writer
/// reads them its own way); DXF writes them back as read.
pub fn drops_unknown(path: &str) -> bool {
    // Templates (.dwt) are written as DWG too.
    std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("dwg") || e.eq_ignore_ascii_case("dwt"))
}

/// Whether `path` is saved as a drawing (.cdy, DXF, DWG) rather than exported as an image.
fn is_drawing_file(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "cdy" | "dxf" | "dwg" | "dwt" | "")
}

/// Whether `path` is a DXF file.
fn is_dxf_file(path: impl AsRef<std::path::Path>) -> bool {
    path.as_ref().extension().is_some_and(|e| e.eq_ignore_ascii_case("dxf"))
}

/// Say on the command line which solid modeler entities a DXF of 2013 or later left out because
/// they hold SAT-text ACIS data, which such files keep in a data store instead (#2334).
fn report_sat_text_dropped(s: &mut Session, r: &mut Value) {
    // The drawing a file holds: the host while the Block Editor is open.
    let lost = s.state().map(|st| cadaddy_doc::foreign::sat_text_dropped(st.drawing())).unwrap_or_default();
    if lost.is_empty() {
        return;
    }
    let list: Vec<String> = lost.iter().map(|(t, n)| format!("{n} {t}")).collect();
    s.echo(format!(
        "Not saved in the DXF file (2013 and later keep ACIS geometry in a data store these entities have no record in; save as 2010 or earlier to keep them): {}",
        list.join(", ")
    ));
    if let Some(o) = r.as_object_mut() {
        o.insert("notSaved".into(), json!(lost));
    }
}

/// Say on the command line what the file couldn't keep, and list it in the result's `notSaved`.
pub(super) fn report_not_saved(s: &mut Session, r: &mut Value, lost: &std::collections::BTreeMap<String, usize>) {
    let Some(msg) = not_saved_message(lost) else { return };
    s.echo(msg);
    if let Some(o) = r.as_object_mut() {
        o.insert("notSaved".into(), json!(lost));
    }
}

thread_local! {
    /// The bytes of the DWG/DXF file a save just wrote, for [`report_not_kept`] (taken there, or
    /// dropped at the end of the save).
    static JUST_WRITTEN: std::cell::RefCell<Option<(String, Vec<u8>)>> = const { std::cell::RefCell::new(None) };
    /// A save that reports what the file keeps is running (only it wants [`JUST_WRITTEN`]).
    static KEEP_WRITTEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Save, then report the entities the file couldn't keep (nothing is dropped silently).
fn save_reporting(s: &mut Session, p: &Value, run: fn(&mut Session, &Value) -> Result<Value>, asked: Option<Version>) -> Result<Value> {
    // What the file will hold depends on the version it is saved as (the data store from 2013).
    // QSAVE takes no `version` and keeps the drawing's own; SAVEAS writes the one asked for.
    let path = str_param(p, "path").map(str::to_string).or_else(|| s.state().ok().and_then(|st| st.path.clone())).unwrap_or_default();
    let version = asked.unwrap_or_else(|| drawing_version(s, &path));
    let lost = s.doc().map(|d| not_saved_as(d, version)).unwrap_or_default();
    // Release 12 has no form for some objects (the rest are converted to R12 ones).
    let r12_lost = if version == Version::R12 { s.doc().map(|d| r12::downgrade(d).1).unwrap_or_default() } else { Default::default() };
    KEEP_WRITTEN.set(true);
    let r = run(s, p);
    KEEP_WRITTEN.set(false);
    let Ok(mut r) = r else {
        JUST_WRITTEN.with(|w| w.borrow_mut().take());
        return r;
    };
    if !r12_lost.is_empty() && r.get("path").and_then(Value::as_str).is_some_and(is_dxf) {
        let list: Vec<String> = r12_lost.iter().map(|(t, n)| format!("{n} {t}")).collect();
        s.echo(format!("Not saved in the Release 12 DXF file (R12 has no such objects): {}", list.join(", ")));
        if let Some(o) = r.as_object_mut() {
            o.insert("notSaved".into(), json!(r12_lost));
        }
    }
    if r.get("path").and_then(Value::as_str).is_some_and(drops_unknown) {
        report_not_saved(s, &mut r, &lost);
    } else if version >= Version::R2013 && r.get("path").and_then(Value::as_str).is_some_and(is_dxf_file) {
        report_sat_text_dropped(s, &mut r);
    }
    if let Some(path) = r.get("path").and_then(Value::as_str).filter(|p| is_exchange_file(p)).map(str::to_string) {
        let opts = save_options("save", p).unwrap_or_default();
        report_not_kept(s, &mut r, &path, opts);
    }
    JUST_WRITTEN.with(|w| w.borrow_mut().take());
    Ok(r)
}

/// A finished "not kept" check: the target it was for, whether it ran to the end (a failed check
/// is retried by the next save), and the notice and command-line line when it found losses.
#[derive(Debug)]
pub struct KeptDone {
    pub key: KeptKey,
    pub succeeded: bool,
    pub notice: Option<(Value, String)>,
}

/// The result of a "not kept" check running on a worker thread.
#[derive(Clone, Debug, Default)]
pub struct KeptJob(std::sync::Arc<std::sync::Mutex<Option<KeptDone>>>);

impl KeptJob {
    /// The finished result, taken (`None` while it runs).
    pub fn take(&self) -> Option<KeptDone> {
        self.0.lock().ok().and_then(|mut r| r.take())
    }
}

/// The exchange notice and command-line line for what `path` didn't keep.
fn kept_notice(path: &str, lost: &[&str], detail: &[kept::Loss]) -> (Value, String) {
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_uppercase()).unwrap_or_else(|| "DXF".into());
    let what = if detail.is_empty() { lost.join(", ") } else { kept::losses_line(detail) };
    (
        json!({ "path": path, "format": ext, "losses": detail }),
        format!("Not kept in the {ext} file: {what}. Save as a CADaddy drawing (.cdy) to keep everything."),
    )
}

/// What a DWG/DXF save is checked for: the format (`true` = DXF), the version (its $ACADVER code) and whether
/// the DXF is binary. A drawing is checked once per target, not once.
pub type KeptKey = (bool, &'static str, bool);

/// The target a save of `path` with `opts` writes (the version it will carry once saved).
fn kept_key(s: &Session, path: &str, opts: SaveAs) -> KeptKey {
    (is_dxf(path), opts.version.unwrap_or_else(|| drawing_version(s, path)).code(), opts.binary)
}

/// A DWG or DXF save into a target not yet checked: say what that file doesn't keep that a .cdy
/// file would, and list it in the result's `notKept`. A target counts as checked once a check of
/// it has succeeded, so saving again in a lossier version is checked too. With background checks
/// on (the app, for saves the user starts), the file is read back on a worker thread so the save
/// returns at once: the result says `notKeptPending`, and the notice comes when the check
/// finishes. Saves an agent makes through the app (`CadApp::run`) are checked as they run.
fn report_not_kept(s: &mut Session, r: &mut Value, path: &str, opts: SaveAs) {
    let Some(hooks) = io() else { return };
    let background = s.background_checks;
    let key = kept_key(s, path, opts);
    let Ok(st) = s.state_mut() else { return };
    if st.kept_checked.contains(&key) {
        return;
    }
    // No threads on the web: checked as the save runs.
    #[cfg(target_arch = "wasm32")]
    let _ = background;
    #[cfg(not(target_arch = "wasm32"))]
    if background {
        // The drawing as saved (shared, not copied).
        let d = st.drawing().clone();
        let written = JUST_WRITTEN.with(|w| w.borrow_mut().take()).filter(|(p, _)| p == path).map(|(_, b)| b);
        let write_as = WRITE_AS.get().copied().filter(|_| opts != SaveAs::default());
        let job = KeptJob::default();
        let (slot, path_owned) = (job.0.clone(), path.to_string());
        let spawned = std::thread::Builder::new().name("kept-check".into()).spawn(move || {
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let bytes = match written {
                    Some(b) => Ok(b),
                    None => match write_as {
                        Some(w) => w(&d, &path_owned, &opts),
                        None => (hooks.write)(&d, &path_owned),
                    },
                };
                let (lost, detail) = bytes.and_then(|b| kept::check_written(&d, &path_owned, &b, hooks.read)).ok()?;
                Some((!lost.is_empty() || !detail.is_empty()).then(|| kept_notice(&path_owned, &lost, &detail)))
            }))
            .ok()
            .flatten();
            if let Ok(mut r) = slot.lock() {
                *r = Some(KeptDone { key, succeeded: checked.is_some(), notice: checked.flatten() });
            }
        });
        if spawned.is_ok() {
            st.kept_job = Some(job);
            if let Some(o) = r.as_object_mut() {
                o.insert("notKeptPending".into(), json!(true));
            }
            return;
        }
    }
    let d = st.drawing();
    // Written as the save wrote it (the version and binary DXF asked for): the bytes it wrote.
    let bytes = match JUST_WRITTEN.with(|w| w.borrow_mut().take()).filter(|(p, _)| p == path) {
        Some((_, b)) => Ok(b),
        None => match WRITE_AS.get() {
            Some(write_as) if opts != SaveAs::default() => write_as(d, path, &opts),
            _ => (hooks.write)(d, path),
        },
    };
    let Ok((lost, detail)) = bytes.and_then(|b| kept::check_written(d, path, &b, hooks.read)) else { return };
    st.kept_checked.push(key);
    if lost.is_empty() && detail.is_empty() {
        return;
    }
    let (notice, line) = kept_notice(path, &lost, &detail);
    st.exchange_notice = Some(notice);
    s.echo(line);
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
    save_reporting(s, p, run_save, None)
}

fn run_saveas_reporting(s: &mut Session, p: &Value) -> Result<Value> {
    s.guard_run("saveas", p)?;
    let asked = save_options("saveas", p)?.version;
    save_reporting(s, p, run_saveas, asked)
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
        s.settings.trustedpaths = std::env::temp_dir().to_string_lossy().into_owned();
        let r = s.execute("script", &json!({ "path": path })).unwrap();
        assert_eq!(r["lines"], 3);
        assert_eq!(s.state().unwrap().doc.model.len(), 2, "the comment is skipped, circle and rectang draw");
    }

    #[test]
    fn script_command_reports_a_missing_file() {
        let mut s = Session::new();
        s.settings.trustedpaths = std::env::temp_dir().to_string_lossy().into_owned();
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
        s.settings.trustedpaths = std::env::temp_dir().to_string_lossy().into_owned();
        let e = s.execute("script", &json!({ "path": outer })).unwrap_err().to_string();
        assert!(e.contains("already running"), "{e}");
    }

    #[test]
    fn typed_script_prompts_for_the_file_then_runs_or_reports_it() {
        let good = temp_script("typed.scr", "circle 0,0 1\n");
        let mut s = Session::new();
        s.settings.trustedpaths = std::env::temp_dir().to_string_lossy().into_owned();
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
        assert_eq!(saved("a/Owned.dxf", &Saved { bytes: n, copy: None, copy_kept: None, incremental: false, note: None })["bytes"], 6);
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

#[cfg(test)]
mod oversize_force_tests {
    use super::*;

    /// `force: true` on a save or export lets it write a DXF/DWG too big to reopen; without it
    /// (or after the save) the write is refused again.
    #[test]
    fn force_allows_a_file_too_big_to_reopen_only_while_it_runs() {
        assert!(!cadaddy_dxf::oversize_allowed());
        let seen = with_force(&json!({"force": true}), cadaddy_dxf::oversize_allowed);
        assert!(seen);
        assert!(!with_force(&json!({}), cadaddy_dxf::oversize_allowed));
        assert!(!with_force(&json!({"force": false}), cadaddy_dxf::oversize_allowed));
        assert!(!cadaddy_dxf::oversize_allowed());
    }
}
