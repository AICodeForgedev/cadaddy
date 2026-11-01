//! The CADaddy engine.
//!
//! Every user-visible action is a command with a stable id (AutoCAD's command names in lower
//! case: `line`, `circle`, `zoom`, `layer`…, plus dotted ids for UI operations). Commands run two
//! ways: programmatically with JSON parameters ([`Session::execute`], no dialogs, used by the
//! control channel, MCP, CLI and scripts) and interactively through prompts ([`Session::start`],
//! [`Session::input`], [`Session::cmdline`]) exactly like typing at AutoCAD's command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![allow(unknown_lints)] // the lint below exists only in newer clippy
#![allow(clippy::chunks_exact_to_as_chunks)]
#![forbid(unsafe_code)]

pub mod activity;
pub mod agent;
pub mod assoc;
#[cfg(not(target_arch = "wasm32"))]
pub mod atomic_write;
pub mod audit;
pub mod autosave;
pub mod batch;
pub mod blockedit;
pub mod cmd;
pub mod control_auth;
pub mod crash_report;
pub mod doc_path;
pub mod events;
pub mod fields;
mod finite;
pub mod grip_dims;
pub mod grips;
mod guard;
pub mod i18n;
pub mod journal;
pub mod json_clean;
pub mod legibility;
pub mod lisp;
pub mod mtext_edit;
pub mod pointmod;
pub mod prompt;
pub mod readfile;
mod recent;
pub mod rhai_engine;
pub mod sample;
pub mod script_tools;
pub mod secureload;
pub mod select;
pub mod snap;
pub mod snap3;
pub mod spatial;
pub mod sysvars;
pub mod ucs;
pub mod undoctl;
pub mod units;
pub mod workflows;
pub mod writefile;

#[cfg(not(target_arch = "wasm32"))]
pub mod plugins;

/// Read `bytes` with one of the engine's own readers, for the fuzzer (`examples/fuzz.rs`):
/// `xlsx` (TABLE's Excel import), `zip` and `lisp` (the LISP reader, as files are loaded). The
/// result is dropped; only returning matters.
#[doc(hidden)]
pub fn fuzz_read(target: &str, bytes: &[u8]) {
    match target {
        "xlsx" => {
            let _ = cmd::table::xlsx::open(bytes);
        }
        "zip" => {
            let _ = cmd::file::zip::read(bytes);
        }
        "lisp" => {
            let _ = lisp::read::read_all(&String::from_utf8_lossy(bytes));
        }
        _ => {}
    }
}

/// A small workbook TABLE's xlsx writer made, as the fuzzer's seed.
#[doc(hidden)]
pub fn fuzz_xlsx_seed() -> Option<Vec<u8>> {
    let rows = vec![
        vec!["Room".to_string(), "Area".into(), "=B2*2".into()],
        vec!["101".into(), "12.5".into(), "Lobby & Hall <1>".into()],
        vec![String::new(), "-3e10".into(), "\u{e9}t\u{e9}".into()],
    ];
    cmd::table::xlsx::write(&rows, "Sheet1").ok()
}

use std::sync::Arc;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Space};
use cadaddy_geom::{Bounds2, Vec2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use cadaddy_doc as doc;

/// The local port (127.0.0.1 only) the desktop app listens on for AI agents unless its "Let AI
/// agents control CADaddy" preference is off, and where `cadaddy-cli mcp` looks for it first.
pub const AGENT_PORT: u16 = 47_820;
pub use cadaddy_geom as geom;
pub use cadaddy_render as render;
pub use cadaddy_walk3d as walk3d;
pub use cmd::blocks::extract::EXTRACT_PROPERTIES;
pub use cmd::hatch::{HatchPicks, PatternChoice, PickMode, pattern_choices};
pub use cmd::render::cancel_pending;

/// Ask what is running to stop: a guided job at its next sheet or stage, else a RENDER at its
/// next sample pass. Safe to call from any thread; the MCP server calls it when a
/// `notifications/cancelled` or the `cancel` tool arrives.
pub fn request_cancel() {
    if !cmd::guided::request_cancel() {
        cmd::render::request_cancel();
    }
}
/// A shaded or 3D view of model space as a PNG (exports; see the function).
pub use cmd::view::view3d::shaded_png;
pub use cmd::{CommandInfo, CommandSpec, HelpPage, command_specs, commands_without_help, find_by_id_or_alias, find_command};
pub use prompt::{Accept, Drag, Input, Interactive, LiveLabel, Picker, Prompt, Step};

/// Most characters of typed input an error or the history repeats.
const ECHO_CHARS: usize = 80;

/// `text` as an error or the history repeats it: the first 80 characters, then `… (N chars)` when
/// there are more (a 30 MB line typed as a command comes back as a short message, not 90 MB).
pub fn clip_input(text: &str) -> String {
    let n = text.chars().count();
    if n <= ECHO_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(ECHO_CHARS).collect();
    format!("{head}\u{2026} ({n} chars)")
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no active document")]
    NoDocument,
    #[error("{0}")]
    Other(String),
    /// A command panicked; the guard kept the drawing as it was (a bug: please report it).
    #[error("internal error in `{0}` (the drawing was kept as it was): {1}")]
    Internal(String, String),
}

impl EngineError {
    /// The error as the command line shows it: the reason alone, without the command id and
    /// "invalid parameters" wording meant for scripts and agents (they get [`ToString`]).
    pub fn user_message(&self) -> String {
        let text = match self {
            EngineError::UnknownCommand(c) => return format!("Unknown command \"{}\". Press F1 for help.", c.to_ascii_uppercase()),
            EngineError::Disabled(_, why) => why.clone(),
            EngineError::BadParams { msg, .. } => msg.clone(),
            EngineError::NoDocument => "No drawing is open.".into(),
            EngineError::Other(m) => m.clone(),
            EngineError::Internal(..) => return self.to_string(),
        };
        // Parameter names in backticks read as plain words.
        let text = text.replace('`', "");
        let mut c = text.chars();
        match c.next() {
            Some(f) => f.to_uppercase().chain(c).collect(),
            None => text,
        }
    }
}

impl From<cadaddy_doc::DocError> for EngineError {
    fn from(e: cadaddy_doc::DocError) -> Self {
        EngineError::Other(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// A 2D view of a space: world centre and visible height in drawing units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub center: Vec2,
    pub height: f64,
}

impl Default for View {
    fn default() -> Self {
        View { center: Vec2::new(6.0, 4.5), height: 10.0 }
    }
}

fn usable_view(v: &View) -> bool {
    v.center.is_finite() && v.height.is_finite() && v.height > 1e-12
}

/// Tiled model viewports (VPORTS): the session side of the drawing's `*Active` configuration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTiles {
    /// The tile corners these views belong to (when the drawing's tiles change, by VPORTS or
    /// undo, its views take over).
    pub rects: Vec<(Vec2, Vec2)>,
    /// Each tile's view; the current tile shows the model space view itself.
    pub views: Vec<View>,
    /// The current tile.
    pub active: usize,
}

/// One tile of the model space drawing area, for the UI ([`Session::model_tiles`]): its corners
/// as fractions of the drawing area (0,0 lower left) and its view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelTile {
    pub min: Vec2,
    pub max: Vec2,
    pub view: View,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub label: String,
    pub doc: Arc<Drawing>,
    pub selection: Vec<Handle>,
    /// Roughly the memory this step holds that the drawing after it doesn't share (set when it is
    /// recorded; 0 until then). Undo keeps the total under [`undoctl::UNDO_BUDGET`].
    pub bytes: usize,
    /// Model space's look before the step, when the step turned its view or changed its visual
    /// style: undo puts it back with the drawing (a BIM build leaves the model in a shaded SW iso
    /// view; undone, the empty drawing was still shown, and plotted, that way: S5-007).
    pub look: Option<ModelLook>,
}

/// How model space is looked at: its view direction and twist, visual style and view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelLook {
    pub dir: cadaddy_geom::Vec3,
    pub twist: f64,
    pub style: cadaddy_render::shade::VisualStyle,
    pub view: Option<View>,
}

impl ModelLook {
    /// Whether `other` looks from the same direction in the same style (a pan or zoom alone is not
    /// a change an undo step puts back).
    pub fn same_3d(&self, other: &ModelLook) -> bool {
        self.dir == other.dir && self.twist == other.twist && self.style == other.style
    }
}

/// The extents of a space, remembered for the last few drawings: a drawing is never changed in
/// place (every edit makes a new one), so the same drawing has the same extents. ZOOM Extents
/// on a 190k-object drawing walked every object each time (about 70 ms).
pub(crate) fn cached_extents(d: &Arc<Drawing>, space: &Space) -> Bounds2 {
    type Entry = (std::sync::Weak<Drawing>, Space, Bounds2);
    static CACHE: std::sync::Mutex<Vec<Entry>> = std::sync::Mutex::new(Vec::new());
    if let Ok(c) = CACHE.lock()
        && let Some((_, _, b)) = c.iter().find(|(w, sp, _)| sp == space && std::ptr::eq(w.as_ptr(), Arc::as_ptr(d)) && w.strong_count() > 0)
    {
        return *b;
    }
    let b = d.extents(space);
    if let Ok(mut c) = CACHE.lock() {
        c.retain(|(w, _, _)| w.strong_count() > 0);
        if c.len() >= 8 {
            c.remove(0);
        }
        c.push((Arc::downgrade(d), space.clone(), b));
    }
    b
}

/// One open drawing.
#[derive(Clone, Debug)]
pub struct DocState {
    pub doc: Arc<Drawing>,
    /// What changed, step by step ([`activity`]).
    pub activity: activity::Activity,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
    /// UNDO marks, BEgin/End group and Control (see [`undoctl`]).
    pub undo_ctl: undoctl::UndoCtl,
    pub selection: Vec<Handle>,
    pub previous_selection: Vec<Handle>,
    pub path: Option<String>,
    pub title: String,
    pub saved: Arc<Drawing>,
    pub revision: u64,
    pub space: Space,
    /// Tiled model viewports (VPORTS).
    pub tiles: ModelTiles,
    /// MSPACE: the layout viewport whose model space is being edited (None = paper space).
    pub mspace: Option<Handle>,
    pub views: Vec<(Space, View)>,
    pub view_history: Vec<View>,
    /// Model space's view direction (VIEWDIR, from the target towards the viewer); +Z is the
    /// plan view. In other views [`View::center`] is on the view plane (see
    /// [`DocState::projection`]).
    pub view_dir: cadaddy_geom::Vec3,
    /// Model space's view twist (VIEWTWIST, radians counterclockwise) in the plan view; a
    /// twisted plan view is drawn through [`DocState::projection`] like a 3D view.
    pub view_twist: f64,
    /// The solid background colour of the named view last restored in model space (`None`:
    /// the drawing area's own colour).
    pub view_background: Option<cadaddy_color::Color>,
    /// The render environment of the current view (RENDERENVIRONMENT).
    pub render_environment: Option<cadaddy_doc::lighting::RenderEnvironment>,
    /// The UCSs UCS Previous goes back to, the latest last.
    pub ucs_previous: Vec<ucs::Frame>,
    /// What the last ERASE removed, with the spaces it was in, for OOPS.
    pub oops: Vec<(Space, Arc<Entity>)>,
    /// VPMAX's maximized viewport and what VPMIN puts back.
    pub vpmax: Option<cmd::classic::VpMax>,
    /// The steering wheel's views to rewind to and its orbit pivot (NAVSWHEEL).
    pub wheel: cmd::view::navwheel::Wheel,
    /// Model space's visual style (VSCURRENT): how 3D objects show on the Model tab.
    pub visual_style: cadaddy_render::shade::VisualStyle,
    /// The 3D view's fidelity layers (`view.fidelity`).
    pub fidelity: cmd::fidelity::View,
    pub uid: u64,
    /// The drawing is saved as binary DXF (it was opened from one, or last saved as one).
    pub binary_dxf: bool,
    /// The Block Editor (BEDIT), when it is open: `doc` is then the editor's drawing, and the
    /// drawing itself waits in it ([`DocState::drawing`]).
    pub block_edit: Option<blockedit::BlockEdit>,
    /// The fonts missing on Windows that a save last named: the next save names them again only
    /// when they change.
    pub(crate) fonts_noted: String,
    /// The test window (BTESTBLOCK), when it is open: `doc` is then a temporary copy of the
    /// block in a drawing of its own, and the Block Editor waits in the state kept here.
    pub block_test: Option<Box<DocState>>,
    /// The file at `path` holds what reading it left out (a DWG's unreadable objects, a cut-off
    /// DXF): what the user was told. Saving over that file asks first; cleared once it's saved.
    pub read_loss: Option<String>,
    /// The user was told that saving over the file would lose what reading left out: saving
    /// again overwrites it.
    pub loss_warned: bool,
    /// The DWG/DXF targets (format, version, binary) whose save was checked for what that file
    /// wouldn't keep of the drawing (and said so, when anything): each is checked once, and only
    /// a check that succeeded is recorded.
    pub kept_checked: Vec<cmd::file::KeptKey>,
    /// What the first DWG or DXF save of the drawing couldn't keep, feature by feature
    /// (`{path, format, losses}`), for the app to show once in a dialog; taken by the app.
    pub exchange_notice: Option<serde_json::Value>,
    /// That check running in the background ([`Session::background_checks`]): its result, once
    /// there, becomes the notice ([`Session::poll_background`]).
    pub kept_job: Option<cmd::file::KeptJob>,
    /// The .cdy file this drawing was last saved to or read from, and what it holds: the next
    /// save there appends only what changed ([`cmd::file::CdySaved`]).
    pub cdy_saved: Option<cmd::file::CdySaved>,
    /// The DWG copy (DWGCOPY) this drawing last wrote: a file at that path is the drawing's own
    /// to replace; any other `.dwg` there is kept as `.dwg.bak` first.
    pub dwg_copy_written: Option<String>,
    /// The file at `path` as it was when the drawing was read from it or last saved there: a save
    /// refuses to overwrite a file something else changed since ([`cmd::file::DiskId`]).
    pub disk_id: Option<cmd::file::DiskId>,
    /// The drawing's associative 3D building (WALLGEN): what it builds again from.
    pub wallgen: Option<cmd::wallgen::WallGen>,
    /// The plan the model was last built from and what that build made (`model.revise`).
    pub model_record: Option<cmd::revise3d::ModelRecord>,
    /// The drawing states the plan has moved past the associative model in: built again before
    /// the next command that reads the model, or when editing stops ([`cmd::wallgen::Behind`]).
    pub model_behind: cmd::wallgen::Behind,
    /// The plan's rooms as last traced, by drawing state (`rooms_found` in `cmd::perceive`).
    pub(crate) room_trace: cmd::RoomTraceCache,
    /// The last revision that found plan changes (`model.revise`, or the rebuild an associative
    /// model does): every list whole, paged when `model.revise {last: true}` returns it.
    pub last_revision: Option<serde_json::Value>,
    /// What opening the drawing changed and told the user (missing current styles reset): the
    /// OPEN reply lists them with its warnings, for agents that see no command-line history.
    pub open_notes: Vec<String>,
    /// Baked indirect light (LIGHTBAKE), and how many times it has changed (views cache their
    /// scene by it).
    pub lightmap: Option<Arc<cadaddy_render::lightmap::Lightmap>>,
    pub lightmap_rev: u64,
    /// The lightmap beside the drawing's file was looked for.
    pub lightmap_tried: bool,
    /// Opened from a walkthrough package (`.cdywalk`, `walk.publish`): the app shows it in Walk.
    pub walkthrough: bool,
    /// The layouts read from the file with nothing on them and not opened since: each gets a
    /// viewport zoomed to the drawing's extents when it is first opened.
    pub bare_layouts: Vec<String>,
    /// Which kinds of new annotation were found too small to read and hinted about
    /// ([`legibility`]), so the hint shows once per drawing.
    pub legibility_hinted: u8,
    /// The IFC references' files as last loaded (upper-case name → size and time), so a changed
    /// file is reloaded ([`cmd::ifcref::poll`]).
    pub ifc_stamps: std::collections::HashMap<String, cmd::ifcref::Stamp>,
    /// The IFC model viewer's section box and camera ([`cmd::ifcmodel`]).
    pub ifc_view: cmd::ifcmodel::ViewState,
}

/// The view that shows `ext` with `margin` (1.05 = 5% more) in a viewport `px` wide and high,
/// between the `inset` pixels covered at its top and bottom (each at most 40% of it). `aspect`
/// turns the free area's width/height into the one to fit (tiled viewports adjust it).
/// The largest coordinate a view is fitted to (the export limit).
pub const VIEW_LIMIT: f64 = 1e12;

fn framed(ext: cadaddy_geom::Bounds2, px: (f64, f64), inset: (f64, f64), aspect: impl Fn(f64) -> f64, margin: f64) -> View {
    let (w, h) = (px.0.max(1.0), px.1.max(1.0));
    let clamp = |v: f64| if v.is_finite() { v.clamp(0.0, h * 0.4) } else { 0.0 };
    let (top, bottom) = (clamp(inset.0), clamp(inset.1));
    let free = (h - top - bottom).max(1.0);
    let fit = ext.height().max(ext.width() / aspect(w / free).max(1e-6)).max(1e-6) * margin;
    let height = (fit * h / free).clamp(1e-9, VIEW_LIMIT);
    // The free area's middle is (bottom - top) / 2 pixels above the viewport's.
    let shift = (bottom - top) / 2.0 * height / h;
    View { center: ext.center() - Vec2::new(0.0, shift), height }
}

static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl DocState {
    /// Show the drawing an undo or redo step holds, with nothing selected; returns the drawing as
    /// it was, as the step back.
    pub(crate) fn swap_in(&mut self, snap: Snapshot) -> Snapshot {
        let look = snap.look.map(|_| self.model_look());
        if let Some(l) = snap.look {
            self.set_model_look(l);
        }
        let cur = Snapshot { label: snap.label, doc: std::mem::replace(&mut self.doc, snap.doc), selection: std::mem::take(&mut self.selection), bytes: snap.bytes, look };
        self.follow_layout(&cur.doc);
        // Views aren't all part of the drawing's undo: the untiled twist record follows the
        // live model view again.
        self.keep_model_twist();
        self.revision += 1;
        cur
    }
    pub fn new(doc: Drawing, title: &str, path: Option<String>) -> Self {
        let doc = Arc::new(doc);
        DocState {
            activity: activity::Activity::default(),
            saved: doc.clone(),
            doc,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_ctl: undoctl::UndoCtl::default(),
            selection: Vec::new(),
            previous_selection: Vec::new(),
            path,
            title: title.into(),
            revision: 1,
            space: Space::Model,
            tiles: ModelTiles::default(),
            mspace: None,
            views: Vec::new(),
            view_history: Vec::new(),
            view_dir: cadaddy_geom::Vec3::Z,
            view_twist: 0.0,
            view_background: None,
            render_environment: None,
            ucs_previous: Vec::new(),
            oops: Vec::new(),
            vpmax: None,
            wheel: Default::default(),
            visual_style: cadaddy_render::shade::VisualStyle::Wireframe2d,
            fidelity: cmd::fidelity::View::default(),
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            binary_dxf: false,
            read_loss: None,
            loss_warned: false,
            kept_checked: Vec::new(),
            exchange_notice: None,
            kept_job: None,
            cdy_saved: None,
            dwg_copy_written: None,
            disk_id: None,
            wallgen: None,
            model_record: None,
            model_behind: cmd::wallgen::Behind::default(),
            room_trace: cmd::RoomTraceCache::default(),
            last_revision: None,
            open_notes: Vec::new(),
            block_edit: None,
            fonts_noted: String::new(),
            block_test: None,
            lightmap: None,
            lightmap_rev: 0,
            lightmap_tried: false,
            walkthrough: false,
            bare_layouts: Vec::new(),
            legibility_hinted: 0,
            ifc_stamps: std::collections::HashMap::new(),
            ifc_view: cmd::ifcmodel::ViewState::default(),
        }
    }
    /// The drawing has changes not saved to its file, or the Block Editor has changes not yet
    /// saved into its block.
    pub fn is_dirty(&self) -> bool {
        !Arc::ptr_eq(self.drawing(), &self.saved) || self.block_unsaved()
    }
    /// After undo/redo replaced the drawing (`before` is the one shown until then): if the current
    /// layout no longer exists, follow it to its other name when it was renamed (same tab order,
    /// a name `before` did not have), else go to the Model tab.
    fn follow_layout(&mut self, before: &Drawing) {
        let Space::Paper(name) = &self.space else { return };
        if self.doc.layouts.iter().any(|l| l.name == *name) {
            return;
        }
        let renamed = before
            .layouts
            .iter()
            .find(|l| l.name == *name)
            .and_then(|old| self.doc.layouts.iter().find(|l| l.tab_order == old.tab_order && !before.layouts.iter().any(|b| b.name == l.name)));
        match renamed.map(|l| Space::Paper(l.name.clone())) {
            Some(to) => {
                let from = self.space.clone();
                for (sp, _) in self.views.iter_mut() {
                    if *sp == from {
                        *sp = to.clone();
                    }
                }
                self.space = to;
            }
            None => {
                self.space = Space::Model;
                self.mspace = None;
            }
        }
    }
    /// The space edits, picks and snaps act on: model space inside an active viewport.
    pub fn edit_space(&self) -> Space {
        if self.active_viewport().is_some() { Space::Model } else { self.space.clone() }
    }
    /// The active MSPACE viewport, if it still exists in the current layout.
    pub fn active_viewport(&self) -> Option<(Handle, cadaddy_doc::Viewport)> {
        let h = self.mspace?;
        let e = self.doc.space(&self.space)?.get(h)?;
        match &e.kind {
            cadaddy_doc::EntityKind::Viewport(v) if v.height > 1e-12 && v.view_height > 1e-12 => Some((h, v.as_ref().clone())),
            _ => None,
        }
    }
    /// How model space is on screen when that is not the plain plan view: a 3D view, a
    /// twisted plan view, or (in MSPACE) the active viewport's view twist. [`View::center`] and
    /// screen picks are then on its view plane. `None` in the untwisted plan view and in paper
    /// space.
    pub fn projection(&self) -> Option<cadaddy_geom::view3::Projection> {
        use cadaddy_geom::view3::{Projection, is_plan};
        if self.space != Space::Model || self.mspace.is_some() {
            return self.active_viewport().and_then(|(_, v)| Projection::plan_twist(v.twist));
        }
        if is_plan(self.view_dir) { Projection::plan_twist(self.view_twist) } else { Projection::new(self.view_dir) }
    }
    /// The view twist on screen (radians, counterclockwise): the active MSPACE viewport's, else
    /// model space's plan view's; 0 in paper space and in 3D views.
    pub fn twist(&self) -> f64 {
        self.projection().map_or(0.0, |p| p.twist())
    }
    /// Set the view twist (radians, counterclockwise) of the view on screen: the active MSPACE
    /// viewport's (turning about its centre) or model space's plan view (turning about the
    /// centre of the screen). False, changing nothing, in paper space, in a 3D view and for a
    /// twist that isn't finite.
    pub fn set_twist(&mut self, twist: f64) -> bool {
        use cadaddy_geom::view3::{Projection, is_plan};
        if !twist.is_finite() {
            return false;
        }
        let t = cadaddy_geom::norm_angle(twist);
        if let Some((h, _)) = self.active_viewport() {
            let space = self.space.clone();
            let doc = Arc::make_mut(&mut self.doc);
            return doc.space_mut(&space).is_some_and(|store| {
                store.modify(h, |e| {
                    if let cadaddy_doc::EntityKind::Viewport(x) = &mut e.kind {
                        x.twist = t;
                    }
                })
            });
        }
        if self.space != Space::Model || !is_plan(self.view_dir) {
            return false;
        }
        let v = self.view();
        // The model point at the centre of the screen stays there.
        let m = self.projection().and_then(|p| p.unproject(v.center, 0.0)).unwrap_or(v.center);
        self.view_twist = t;
        self.keep_model_twist();
        let center = Projection::plan_twist(t).map_or(m, |p| p.project(m.to3(0.0)));
        self.put_paper_view(View { center, height: v.height });
        true
    }
    /// A model space box as the current view sees it: the centre and width/height of its four
    /// corners on the view plane (the box itself when the view isn't turned).
    pub fn seen_box(&self, ext: Bounds2) -> (Vec2, f64, f64) {
        let Some(p) = self.projection() else { return (ext.center(), ext.width(), ext.height()) };
        let corners = [ext.min, Vec2::new(ext.max.x, ext.min.y), ext.max, Vec2::new(ext.min.x, ext.max.y)];
        let seen = Bounds2::from_points(corners.iter().map(|c| p.project(c.to3(0.0))));
        if seen.is_empty() { (ext.center(), ext.width(), ext.height()) } else { (seen.center(), seen.width(), seen.height()) }
    }
    /// Keep model space's view twist in the drawing, where files carry it: the twist of the
    /// lone `*Active` VPORT tile when the model view isn't tiled (DXF VPORT group 51), the
    /// current tile's when it is. Changes nothing when the drawing already says so.
    pub fn keep_model_twist(&mut self) {
        use cadaddy_doc::{VportConfig, VportTile};
        let t = if self.view_twist.is_finite() { cadaddy_geom::norm_angle(self.view_twist) } else { 0.0 };
        let active = self.doc.vports.iter().position(|c| c.is_active());
        let current = active.and_then(|i| self.doc.vports.get(i)).and_then(|c| c.tiles.first()).map(|x| x.twist);
        if current.unwrap_or(0.0) == t {
            return;
        }
        let v = self.views.iter().find(|(s, _)| *s == Space::Model).map(|(_, v)| *v).unwrap_or_default();
        let doc = Arc::make_mut(&mut self.doc);
        match active.and_then(|i| doc.vports.get_mut(i)) {
            Some(c) if c.tiles.len() > 1 => {
                if let Some(x) = c.tiles.first_mut() {
                    x.twist = t;
                }
            }
            Some(_) if t == 0.0 => doc.vports.retain(|c| !c.is_active()),
            Some(c) => {
                c.tiles = vec![VportTile { min: Vec2::ZERO, max: Vec2::new(1.0, 1.0), center: v.center, height: v.height, aspect: 0.0, twist: t }];
            }
            None => doc.vports.push(VportConfig {
                name: VportConfig::ACTIVE.to_string(),
                tiles: vec![VportTile { min: Vec2::ZERO, max: Vec2::new(1.0, 1.0), center: v.center, height: v.height, aspect: 0.0, twist: t }],
            }),
        }
    }
    /// The projection the current space's display list is built with: [`Self::projection`],
    /// except in a layout, whose sheet is drawn flat (an MSPACE viewport's model space is drawn
    /// through the viewport).
    /// Model space's look ([`ModelLook`]).
    pub fn model_look(&self) -> ModelLook {
        let view = self.views.iter().find(|(s, _)| *s == Space::Model).map(|(_, v)| *v);
        ModelLook { dir: self.view_dir, twist: self.view_twist, style: self.visual_style, view }
    }
    /// Put model space's look back (whichever space is current).
    pub fn set_model_look(&mut self, l: ModelLook) {
        self.view_dir = l.dir;
        self.view_twist = l.twist;
        self.visual_style = l.style;
        if let Some(v) = l.view.filter(usable_view) {
            match self.views.iter_mut().find(|(s, _)| *s == Space::Model) {
                Some((_, slot)) => *slot = v,
                None => self.views.push((Space::Model, v)),
            }
        }
    }
    pub fn list_projection(&self) -> Option<cadaddy_geom::view3::Projection> {
        if self.space != Space::Model { None } else { self.projection() }
    }
    pub fn paper_view(&self) -> View {
        self.views.iter().find(|(s, _)| *s == self.space).map(|(_, v)| *v).unwrap_or_default()
    }
    /// The screen view. Inside an MSPACE viewport this is the model-space view that maps the
    /// screen consistently with the paper view, so picks, snaps and previews work in model units.
    pub fn view(&self) -> View {
        let pv = self.paper_view();
        match self.active_viewport() {
            Some((_, vp)) => {
                let k = vp.view_height / vp.height;
                // On the view plane: the model turned by the viewport's twist.
                View { center: vp.view_center.rotate(vp.twist) + (pv.center - vp.center.xy()) * k, height: pv.height * k }
            }
            None => pv,
        }
    }
    /// Map a requested screen view to the paper view, or (in MSPACE) to the viewport's model view.
    /// A finite request can still map to a view too large to represent: then nothing changes and
    /// this returns false.
    fn store_view(&mut self, v: View) -> bool {
        if let Some((h, vp)) = self.active_viewport() {
            let pv = self.paper_view();
            let k = v.height / pv.height.max(1e-12);
            if vp.locked {
                // A locked viewport keeps its scale: pan/zoom the sheet instead.
                let k0 = vp.view_height / vp.height;
                let pc = vp.center.xy() + (v.center - vp.view_center.rotate(vp.twist)) / k0;
                let paper = View { center: pc, height: v.height / k0 };
                if !usable_view(&paper) {
                    return false;
                }
                self.put_paper_view(paper);
            } else {
                let center = (v.center - (pv.center - vp.center.xy()) * k).rotate(-vp.twist);
                let height = vp.height * k;
                if !usable_view(&View { center, height }) {
                    return false;
                }
                let doc = Arc::make_mut(&mut self.doc);
                if let Some(store) = doc.space_mut(&self.space.clone()) {
                    store.modify(h, |e| {
                        if let cadaddy_doc::EntityKind::Viewport(x) = &mut e.kind {
                            x.view_center = center;
                            x.view_height = height;
                        }
                    });
                }
            }
            return true;
        }
        self.put_paper_view(v);
        true
    }
    fn put_paper_view(&mut self, v: View) {
        match self.views.iter_mut().find(|(s, _)| *s == self.space) {
            Some((_, slot)) => *slot = v,
            None => self.views.push((self.space.clone(), v)),
        }
    }
    /// Set the view, recording view history. Returns false, changing nothing, when the view (or,
    /// in MSPACE, the viewport view it maps to) isn't finite with a positive height.
    pub fn set_view(&mut self, v: View) -> bool {
        if !usable_view(&v) {
            return false;
        }
        let old = self.view();
        if !self.store_view(v) {
            return false;
        }
        if old != v {
            self.view_history.push(old);
            if self.view_history.len() > 50 {
                self.view_history.remove(0);
            }
        }
        true
    }
    /// The active MSPACE viewport when ZOOM works on its own view, as in AutoCAD: extents and
    /// windows fit the viewport, and zooms keep its centre. A locked viewport keeps its scale, so
    /// zooming it moves the sheet instead (see `store_view`).
    pub fn zoom_viewport(&self) -> Option<cadaddy_doc::Viewport> {
        self.active_viewport().map(|(_, v)| v).filter(|v| !v.locked && v.width > 1e-12)
    }
    /// The view ZOOM starts from: the model view of the active unlocked viewport, else [`Self::view`].
    pub fn zoom_frame(&self) -> View {
        match self.zoom_viewport() {
            Some(v) => View { center: v.view_center.rotate(v.twist), height: v.view_height },
            None => self.view(),
        }
    }
    /// Width / height of the area ZOOM fits into: the active unlocked viewport, else `screen`.
    pub fn zoom_aspect(&self, screen: f64) -> f64 {
        self.zoom_viewport().map_or(screen, |v| v.width / v.height)
    }
    /// Set the view ZOOM works on (see [`Self::zoom_frame`]), recording view history.
    pub fn set_zoom_frame(&mut self, f: View) {
        let v = match self.zoom_viewport() {
            // The screen view that `store_view` maps to this model view of the viewport.
            Some(vp) => {
                let pv = self.paper_view();
                let k = f.height / vp.height;
                View { center: f.center + (pv.center - vp.center.xy()) * k, height: pv.height * k }
            }
            None => f,
        };
        self.set_view(v);
    }
    /// Set without recording view history (realtime pan/zoom frames).
    pub fn set_view_quiet(&mut self, v: View) {
        if !(v.center.is_finite() && v.height.is_finite() && v.height > 1e-12) {
            return;
        }
        self.store_view(v);
    }
}

/// The workspace a new profile starts in (WSCURRENT).
pub const DEFAULT_WORKSPACE: &str = "Drafting & Annotation";

/// Session-level drafting settings. [`Session::drafting_settings`] supplies the active drawing's
/// snap/grid spacing in the corresponding fields for UI and JSON consumers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub osmode: u32,
    /// 3D object snaps (3DOSMODE, [`snap3::mode`]); 0 is off.
    pub osmode3: u32,
    /// Dynamic UCS (UCSDETECT): points drawn in a 3D view land on the face under the cursor.
    pub ducs: bool,
    pub orthomode: bool,
    pub polarmode: bool,
    /// POLARANG (radians).
    pub polarang: f64,
    pub gridmode: bool,
    pub snapmode: bool,
    /// SNAPUNIT in a drafting settings snapshot; stored in the drawing header.
    pub snapunit: Vec2,
    /// OSNAPHATCH: object snaps find hatch objects (off by default, as in AutoCAD).
    pub osnaphatch: bool,
    /// GRIDUNIT in a drafting settings snapshot; zero components follow SNAPUNIT.
    pub gridunit: Vec2,
    /// Major grid line every N minor lines (GRIDMAJOR).
    pub gridmajor: u32,
    /// ZOOMFACTOR: how far one mouse wheel step zooms (3..=100).
    pub zoomfactor: u32,
    /// MEASUREINIT: the units of a new drawing made without a template (NEW, QNEW and the first
    /// drawing at startup): `false` (0, the default) imperial, `true` (1) metric.
    pub measureinit: bool,
    /// -VIEW sEttings > Layer snapshot: named views save the layer settings with them.
    pub view_layer_snapshot: bool,
    pub dynmode: bool,
    /// DYNDIGRIP: which dimensions grip dimensional input shows while a grip is stretched (bits:
    /// 1 resulting length, 2 length change, 4 absolute angle, 8 angle change, 16 radius); read
    /// when DYNDIVIS = 2.
    pub dyndigrip: u32,
    /// DYNDIVIS: how many of them: 0 the first, 1 the first two (default), 2 all DYNDIGRIP asks for.
    pub dyndivis: u8,
    /// DYNPIFORMAT = 1: Dynamic Input shows second and next points as Cartesian `x,y` instead of
    /// polar `distance<angle` (the default).
    pub dynpi_cartesian: bool,
    /// DYNPICOORDS = 1: second and next points typed into Dynamic Input are absolute instead of
    /// relative to the last point (the default).
    pub dynpi_absolute: bool,
    /// DYNPROMPT: Dynamic Input shows the prompt beside the cursor, and Down Arrow opens the
    /// prompt's options there (on by default).
    pub dynprompt: bool,
    pub lwdisplay: bool,
    /// TRANSPARENCYDISPLAY: draw objects with their transparency on screen (on by default).
    pub transparency_display: bool,
    /// NAVVCUBEDISPLAY: where the ViewCube shows (bits: 1 in 2D views, 2 in 3D views; 0 never).
    pub navvcube_display: u8,
    /// NAVVCUBELOCATION: the ViewCube's corner: 0 top right, 1 top left, 2 bottom left, 3 bottom
    /// right.
    pub navvcube_location: u8,
    /// NAVVCUBESIZE: 0 small, 1 normal, 2 large, 3 tiny, 4 automatic (by the drawing area's size).
    pub navvcube_size: u8,
    /// NAVVCUBEOPACITY: how opaque the ViewCube is while the pointer is away from it (0–100 %).
    pub navvcube_opacity: u8,
    /// NAVVCUBEORIENT: the cube shows the directions of 0 the world, 1 the current UCS.
    pub navvcube_orient: u8,
    /// NAVSWHEELSIZEBIG: the steering wheel's size: 0 small, 1 normal, 2 large, 3 extra large.
    pub navswheel_size: u8,
    /// NAVSWHEELOPACITYBIG: how opaque the steering wheel is (25–90 %).
    pub navswheel_opacity: u8,
    /// PLOTTRANSPARENCYOVERRIDE: 0 never plots transparency, 1 (default) uses the page setup's or
    /// the Plot dialog's "Plot transparency", 2 always plots it.
    pub plot_transparency_override: u8,
    /// SELECTIONCYCLING: a pick over overlapping objects can step through them (Shift+Space);
    /// 2 also lists them to choose from, 1 doesn't. 0 is off; -1 and -2 are off and remember the
    /// mode the status-bar toggle turns back on.
    pub selectioncycling: i8,
    /// NAVBARDISPLAY: the navigation bar on the drawing area's right edge.
    pub navbardisplay: bool,
    /// WSCURRENT: the current workspace's name (the interface applies it).
    pub wscurrent: String,
    /// SELECTSIMILARMODE: what SELECTSIMILAR matches besides the object type (bits: 1 colour,
    /// 2 layer, 4 linetype, 8 linetype scale, 16 lineweight, 32 plot style, 64 object style,
    /// 128 name).
    pub selectsimilarmode: u32,
    /// REDIRMODE: what REDIR changes paths in (any of `styles`, `xrefs`, `images`, `rtext`).
    pub redirmode: Vec<String>,
    pub otrack: bool,
    /// PICKBOX in pixels.
    pub pickbox: f64,
    /// APERTURE in pixels.
    pub aperture: f64,
    pub pickfirst: bool,
    /// ACADLSPASDOC: acad.lsp loads into every drawing, not once per session.
    pub acadlspasdoc: bool,
    /// SECURELOAD: 0 loads LISP and scripts from anywhere, 1 (default) asks about files outside
    /// the trusted folders, 2 loads only from trusted folders ([`secureload`]).
    pub secureload: u8,
    /// TRUSTEDPATHS: folders LISP and scripts load from without asking (`;`-separated; `\...`
    /// at the end includes subfolders). The support folder is always trusted.
    pub trustedpaths: String,
    pub pickadd: bool,
    /// FILEDIA: 0 makes file commands ask on the command line instead of opening dialogs. Kept for
    /// the session (AutoCAD keeps it in the registry), not per drawing.
    pub filedia: u8,
    /// CMDECHO: commands run from LISP echo their prompts (1) or not (0).
    pub cmdecho: u8,
    /// EXPERT: 0–5, how many "are you sure?" questions are suppressed.
    pub expert: u8,
    /// ATTDIA: 1 asks for attribute values in a dialog when inserting, 0 on the command line.
    pub attdia: u8,
    /// PICKDRAG: false = two-click windows (0), true = clicks or dragging (2).
    /// Drag-only mode (1) is not supported yet.
    pub pickdrag: bool,
    /// PICKSTYLE: bit 1 picks whole selectable groups ([`cmd::groups`]), bit 2 associative hatches
    /// with their boundaries (stored, not used yet).
    pub pickstyle: u8,
    /// GROUPDISPLAYMODE: a selected group shows every member's grips (0), one grip at its centre
    /// (1), or that grip and its bounding box (2, the default).
    pub groupdisplaymode: u8,
    /// OBJECTISOLATIONMODE: objects hidden by ISOLATEOBJECTS / HIDEOBJECTS stay hidden for this
    /// session only (0, the default) or are saved with the drawing (1).
    pub objectisolationmode: u8,
    pub gripsize: f64,
    pub cursorsize: f64,
    /// MAXARRAY: the most objects one array command creates (items × selected objects).
    pub maxarray: u64,
    /// SNAPSTYL 1: isometric snap and grid, ortho and crosshair along the isometric plane's axes.
    pub isodraft: bool,
    /// SNAPISOPAIR: the current isometric plane (ISOPLANE, F5).
    pub isoplane: snap::iso::IsoPlane,
    pub annoallvisible: bool,
    pub annoautoscale: bool,
    /// QPMODE: show the Quick Properties palette when objects are selected.
    pub qpmode: bool,
    /// HIGHLIGHT: selected objects are highlighted (their grips show either way). Not saved:
    /// every session starts with it on, as in AutoCAD, so it can't stay off unnoticed.
    pub highlight: bool,
    /// SELECTIONEFFECT: how selected objects are highlighted: 0 dashed, 1 redrawn in the
    /// selection colour.
    pub selectioneffect: u8,
    /// SELECTIONPREVIEW bits: the object under the cursor is highlighted (1) when no command is
    /// active, (2) when a command asks for objects.
    pub selectionpreview: u8,
    /// MEASURELABELS: DIST, AREA and MEASUREGEOM label what they measure on the drawing while you
    /// pick (1, the default), also keep the final labels as multiline text (2), or only report at
    /// the command line (0).
    pub measurelabels: u8,
    /// PREVIEWEFFECT: how that object is highlighted: 0 dashed, 1 thicker, 2 dashed and thicker.
    pub previeweffect: u8,
    /// PREVIEWFILTER bits: objects the cursor never highlights: 1 on locked layers, 2 xrefs,
    /// 4 tables, 8 multiline text, 16 hatches, 32 in groups.
    pub previewfilter: u8,
    /// GRIPS: selected objects show their grips (0 off; AutoCAD's 2, more grips on polylines,
    /// shows them as 1 does).
    pub grips: u8,
    /// GRIPOBJLIMIT: no grips while more objects than this are selected (0: no limit).
    pub gripobjlimit: u32,
    /// SELECTIONAREA: the selection window and crossing areas are filled.
    pub selectionarea: bool,
    /// WINDOWAREACOLOR, CROSSINGAREACOLOR: their colours (ACI 1-255).
    pub windowareacolor: u8,
    pub crossingareacolor: u8,
    /// SELECTIONAREAOPACITY: their opacity, 0-100.
    pub selectionareaopacity: u8,
    /// POLARMODE bits ([`snap::tracking::polarmode`]); polar tracking itself is on while
    /// `polarmode` is (AUTOSNAP bit 8, F10).
    pub polar_flags: u32,
    /// POLARADDANG: additional polar tracking angles (radians), used with POLARMODE bit 4.
    pub polaraddang: Vec<f64>,
    /// SNAPTYPE 1: snap mode (F9) snaps along tracking paths at POLARDIST steps (PolarSnap)
    /// instead of to the grid.
    pub polarsnap: bool,
    /// POLARDIST: the PolarSnap step; 0 uses the snap X spacing.
    pub polardist: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            navbardisplay: true,
            wscurrent: DEFAULT_WORKSPACE.into(),
            osmode: 4133,
            osmode3: snap3::mode::VERTEX | snap3::mode::MIDPOINT | snap3::mode::CENTER,
            ducs: true,
            orthomode: false,
            polarmode: true,
            polarang: std::f64::consts::FRAC_PI_2,
            gridmode: true,
            snapmode: false,
            snapunit: Vec2::new(0.5, 0.5),
            osnaphatch: false,
            gridunit: Vec2::new(0.5, 0.5),
            gridmajor: 5,
            zoomfactor: 60,
            measureinit: false,
            view_layer_snapshot: true,
            dynmode: true,
            dyndigrip: 31,
            dyndivis: 1,
            dynpi_cartesian: false,
            dynpi_absolute: false,
            dynprompt: true,
            lwdisplay: false,
            transparency_display: true,
            navvcube_display: 3,
            navvcube_location: 0,
            navvcube_size: 4,
            navvcube_opacity: 50,
            navvcube_orient: 0,
            navswheel_size: 1,
            navswheel_opacity: 50,
            plot_transparency_override: 1,
            selectioncycling: 2,
            selectsimilarmode: 130,
            redirmode: ["styles", "xrefs", "images", "rtext"].map(String::from).to_vec(),
            otrack: true,
            pickbox: 3.0,
            aperture: 10.0,
            pickfirst: true,
            acadlspasdoc: false,
            secureload: 1,
            trustedpaths: String::new(),
            pickadd: true,
            filedia: 1,
            cmdecho: 1,
            expert: 0,
            attdia: 0,
            pickdrag: true,
            pickstyle: 1,
            groupdisplaymode: 2,
            objectisolationmode: 0,
            gripsize: 5.0,
            cursorsize: 5.0,
            maxarray: 100_000,
            isodraft: false,
            isoplane: snap::iso::IsoPlane::Left,
            annoallvisible: true,
            annoautoscale: false,
            qpmode: false,
            highlight: true,
            selectioneffect: 1,
            selectionpreview: 3,
            measurelabels: 1,
            previeweffect: 2,
            previewfilter: 3,
            grips: 1,
            gripobjlimit: 100,
            selectionarea: true,
            windowareacolor: 150,
            crossingareacolor: 100,
            selectionareaopacity: 25,
            polar_flags: 0,
            polaraddang: Vec::new(),
            polarsnap: false,
            polardist: 0.0,
        }
    }
}

/// An interactive command in progress.
pub struct Running {
    pub id: String,
    pub machine: Box<dyn Interactive>,
    pub before: Arc<Drawing>,
    pub selection_before: Vec<Handle>,
}

/// The names a command's parameter documentation mentions (`{center, radius?: …}`): every word
/// after `{` `,` `|` `(` `:` `;` `/` or `+` and followed by `:` `?` `,` `}` `)` `|` `+` `/` `→` or a
/// note in brackets (`base | baseHandle+index`, `first?/second?`, `path arrays: method?`) — nested
/// ones too, so it errs towards accepting. Only the parameter part counts: nothing after the
/// top-level `→` (the result's keys), and no type word after a `:` (`closed?: bool`,
/// `tolerance_in?: inches`), battle test 5 B7-019.
///
/// Only a word right inside a `{…}` names a parameter (battle test 7 B7-020: the lists offered
/// `default`, `MVIEW`, `PNG`, `null`, `Title`): not one in a note in brackets (`(default: the
/// selection)`, `(as MVIEW)`), an item of a `[…]` list (`[x, y]`, `[vertex, midpoint]`), a quoted
/// value (`"Model"`), the value after a colon (`level: name`, `align?: TL|TC`) or a sign
/// (`from +Y`). A doc without any `{…}` is read as one plain list.
pub fn documented_params(doc: &str) -> Vec<&str> {
    documented_names(doc).into_iter().map(|(name, _)| name).collect()
}

/// The words a command's parameter documentation puts where a name or a value goes, notes
/// included, but not its result: what [`CommandSpec::reach`] looks for file and network words in
/// (`colorMap?: path` reaches files although `path` there isn't a parameter's name).
pub fn param_words(doc: &str) -> Vec<&str> {
    words_in(doc, false)
}

/// The names the doc reader took before battle test 7 (a value word after a colon, a plain
/// choice's values and a capitalised word left out): what the job, BIM and MEP lane's commands
/// still take.
fn lane_words(doc: &str) -> Vec<&str> {
    words_in(doc, true)
}

/// [`param_words`]; `names` leaves out what the earlier reader didn't take as names.
fn words_in(doc: &str, names: bool) -> Vec<&str> {
    let b = doc.as_bytes();
    let mut out = Vec::new();
    let mut nest = 0usize;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let prev = doc[..start].trim_end().chars().last();
            let next = doc[i..].trim_start().chars().next();
            let word = &doc[start..i];
            let named = !matches!(next, Some('?' | ':'));
            let left_out = names && named && (prev == Some(':') && VALUE_WORDS.contains(&word) || plain_choice(doc, start, prev));
            if matches!(prev, Some('{' | ',' | '|' | '(' | ':' | ';' | '/' | '+'))
                && matches!(next, Some(':' | '?' | ',' | '}' | ')' | '(' | '|' | '+' | '/' | '→'))
                && !word.starts_with(|c: char| c.is_ascii_uppercase())
                && !left_out
            {
                out.push(word);
            }
            continue;
        }
        match c {
            b'{' | b'[' => nest += 1,
            b'}' | b']' => nest = nest.saturating_sub(1),
            _ if nest == 0 && doc.get(i..).is_some_and(|r| r.starts_with('→')) => {
                i += '→'.len_utf8();
                i = skip_group(doc, skip_group(doc, i, b'{', b'}', true), b'(', b')', false);
                if doc.get(i..).is_some_and(|r| r.starts_with(':')) {
                    i = doc.get(i..).and_then(|r| r.find('|')).map_or(doc.len(), |at| i + at);
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// [`documented_params`] with how deep in `{…}` each name is: 1 for a command's own parameters,
/// more for the fields of a value (`polar: {count, angle?}`).
fn documented_names(doc: &str) -> Vec<(&str, usize)> {
    let b = doc.as_bytes();
    let braced = doc.contains('{');
    // Quoted spans are values; an odd count of quotes (an inch mark) leaves them alone.
    let quotes = |q: u8| b.iter().filter(|c| **c == q).count() % 2 == 0;
    let (skip_dq, skip_bq) = (quotes(b'"'), quotes(b'`'));
    let choices = choice_words(doc);
    let mut out = Vec::new();
    let mut groups: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &doc[start..i];
            let prev = doc[..start].trim_end().chars().last();
            let next = doc[i..].trim_start().chars().next();
            let named = !matches!(next, Some('?' | ':'));
            // `{name (e.g. …), at: …}`: a name with a note in brackets counts too (models3d.place
            // warned that its own `name` was ignored; tester 0.7.7f).
            let is_choice = named && choice_value(doc, start, prev);
            let inside = match groups.last() {
                // A value after a colon (`closed?: bool`, `level: name`, `align?: TL|TC`) isn't
                // a name; one followed by `?` or `:` (`path arrays: method?`) is.
                // A format or a value joined to one (`path: CSV/TSV/XLSX file`) isn't either.
                Some(b'{' | b'f') => {
                    let joined = named && word.starts_with(|c: char| c.is_ascii_uppercase()) && (prev == Some('/') || next == Some('/'));
                    let value = prev == Some(':') && named;
                    !value && !joined
                }
                // A note in brackets names parameters too (`(file | text: its contents)`, `(+
                // base?)`), but not a variable, a format or a value (`(HPASSOC)`, `(image: PNG,
                // JPEG)`, `(Model, default true)`), one word alone (`(attach)`) or options
                // (`(save/restore)`).
                Some(b'(') => {
                    let capital = word.starts_with(|c: char| c.is_ascii_uppercase());
                    let alone = prev == Some('(') && next == Some(')');
                    let option = named && (prev == Some('/') || next == Some('/'));
                    !capital && !alone && !option
                }
                // A list's items are values (`[x, y]`, `[vertex, midpoint]`).
                Some(_) => false,
                // Outside every `{…}`: a name with its `?` or `:` (`…}; fit?: false`).
                None => !braced || !named,
            };
            let doc_word = matches!(word, "default" | "e" | "g" | "eg" | "ie" | "true" | "false" | "null" | "none") && next != Some('?')
                // An option's value named in a note or as a label (`(origin, 3point, set)`, `;
                // set: leaderColor?`), where the option is a choice (`option: …|set|…`).
                || choices.contains(&word) && next != Some('?') && (groups.last() == Some(&b'(') || next == Some(':'));
            if inside
                && !doc_word
                && !is_choice
                && !signed(b, start, prev)
                && matches!(prev, Some('{' | ',' | '|' | '(' | ':' | ';' | '/' | '+'))
                && matches!(next, Some(':' | '?' | ',' | '}' | ')' | '(' | '|' | '+' | '/' | '→'))
            {
                let depth = groups.iter().filter(|g| **g == b'{').count();
                out.push((word, depth));
            }
        } else {
            match c {
                b'"' if skip_dq => {
                    i = doc.get(i + 1..).and_then(|r| r.find('"')).map_or(b.len(), |at| i + at + 2);
                    continue;
                }
                b'`' if skip_bq => {
                    i = doc.get(i + 1..).and_then(|r| r.find('`')).map_or(b.len(), |at| i + at + 2);
                    continue;
                }
                // `{mode: … | align {base, handles}}`, `{action: "save" {path, sheets}}`: the
                // parameters of one form, after the value that picks it, are the command's own.
                b'{' if doc[..i].trim_end().ends_with(|p: char| p.is_ascii_alphanumeric() || p == '"') => groups.push(b'f'),
                b'{' | b'[' | b'(' => groups.push(c),
                b'}' | b']' | b')' => {
                    groups.pop();
                }
                // A result in a note (`(no name: → {current, configurations})`).
                _ if groups.last() == Some(&b'(') && doc.get(i..).is_some_and(|r| r.starts_with('→')) => {
                    i = skip_group(doc, i + '→'.len_utf8(), b'{', b'}', true);
                    continue;
                }
                // The result of the command, not its parameters: the group after the arrow (and
                // a note in brackets after it). Docs of several forms go on after it. A result
                // told in words (`→ list: [{…}]; load: {name, …}`) runs to the next form.
                _ if groups.is_empty() && doc.get(i..).is_some_and(|r| r.starts_with('→')) => {
                    let at = i + '→'.len_utf8();
                    let group = skip_group(doc, at, b'{', b'}', true);
                    if group == at {
                        i = next_form(doc, at);
                        continue;
                    }
                    i = skip_group(doc, group, b'(', b')', false);
                    // `…: the readable text of TEXT, MTEXT` is prose about the result, up to the
                    // next form (`|`) or the end.
                    if doc.get(i..).is_some_and(|r| r.starts_with(':')) {
                        i = doc.get(i..).and_then(|r| r.find('|')).map_or(doc.len(), |at| i + at);
                    }
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
    }
    out
}

/// The values the doc's choices offer (`option: save|restore|set`, `mode?: on|off`).
fn choice_words(doc: &str) -> Vec<&str> {
    let b = doc.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let prev = doc[..start].trim_end().chars().last();
            let next = doc[i..].trim_start().chars().next();
            let first = prev == Some(':') && next == Some('|');
            if !matches!(next, Some('?' | ':')) && (first || choice_value(doc, start, prev)) {
                out.push(&doc[start..i]);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// A word after a sign (`from +Y`, `-X`): a direction, not a name joined to another with `+`
/// (`baseHandle+index`, `base + handle`).
fn signed(b: &[u8], start: usize, prev: Option<char>) -> bool {
    prev == Some('+') && start >= 2 && b.get(start - 1) == Some(&b'+') && b.get(start - 2).is_some_and(|c| c.is_ascii_whitespace() || *c == b'(')
}

/// Where the next form of a doc starts after a command's result at `at` (a top-level `|`
/// followed by `{`), or the doc's end.
fn next_form(doc: &str, at: usize) -> usize {
    let b = doc.as_bytes();
    let mut depth = 0usize;
    let mut i = at;
    while let Some(&c) = b.get(i) {
        match c {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth = depth.saturating_sub(1),
            b'|' if depth == 0 && doc.get(i + 1..).is_some_and(|r| r.trim_start().starts_with('{')) => return i,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// `at` moved past the balanced `open … close` group that starts there (after spaces); `at` itself
/// when none does. `any` also takes a `[…]` group in place of `{…}`.
fn skip_group(doc: &str, at: usize, open: u8, close: u8, any: bool) -> usize {
    let b = doc.as_bytes();
    let mut i = at;
    while b.get(i).is_some_and(|c| *c == b' ') {
        i += 1;
    }
    let first = b.get(i).copied();
    let (open, close) = if any && first == Some(b'[') { (b'[', b']') } else { (open, close) };
    if first != Some(open) {
        return at;
    }
    let mut depth = 0usize;
    while let Some(&c) = b.get(i) {
        i += 1;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    i
}

/// The doc's parameter part with every quoted span and note in brackets left out.
fn without_notes(t: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    for c in t.chars() {
        match c {
            '"' if depth == 0 => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth = depth.saturating_sub(1),
            _ if depth == 0 && !quoted => out.push(c),
            _ => {}
        }
    }
    out
}

/// Words that follow a `:` in a parameter list as a value's type or unit, never as a name.
const VALUE_WORDS: [&str; 16] =
    ["bool", "boolean", "number", "string", "int", "integer", "degrees", "deg", "inches", "feet", "hex", "base64", "true", "false", "null", "typed"];

/// [`choice_value`] as the reader took it before battle test 7: only a list of plain words.
fn plain_choice(doc: &str, at: usize, prev: Option<char>) -> bool {
    choice_segment(doc, at, prev).is_some_and(|list| list.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '|' | ' ' | '-')))
}

/// The text after the colon of the choice the word at `at` (after a `|`) is one value of.
fn choice_segment(doc: &str, at: usize, prev: Option<char>) -> Option<&str> {
    if prev != Some('|') {
        return None;
    }
    let before = doc.get(..at).unwrap_or("");
    let mut depth = 0i32;
    let mut segment = before.len();
    for (i, c) in before.char_indices().rev() {
        match c {
            ')' | ']' | '}' => depth += 1,
            '(' | '[' | '{' if depth == 0 => {
                segment = i + 1;
                break;
            }
            '(' | '[' | '{' => depth -= 1,
            ',' if depth == 0 => {
                segment = i + 1;
                break;
            }
            _ => {}
        }
        segment = i;
    }
    before.get(segment..).and_then(|t| t.split_once(':')).map(|(_, list)| list)
}

/// A word after `|` in `type?: all | layers | blocks`: one of the values the parameter before the
/// colon takes, not a parameter (`purge {layers: 12345}` was accepted as if `layers` existed).
fn choice_value(doc: &str, at: usize, prev: Option<char>) -> bool {
    // Only a plain list of values after the colon (`type?: all | blocks | layers`, `space?: all
    // (default)|current|<layout>`, `option: "?"|bind`); anything with a structure in it may name
    // real parameters (`vertices?: [...] | points`).
    choice_segment(doc, at, prev).is_some_and(|list| {
        without_notes(list).chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '|' | ' ' | '-' | '<' | '>' | '*' | '.' | '\''))
    })
}

/// Parameters every command takes: the objects it works on, and the agent rails' own
/// (`confirmed`: an agent claiming the user's answer gets "Waiting for the user", not a refusal).
const ALWAYS_PARAMS: [&str; 3] = ["handle", "handles", "confirmed"];

/// Whether a command's documentation leaves its parameters open: `<style fields or DIM*
/// variables…>`, `…: any profile variable` or `...the parameters of arrayrect` at the top level
/// of a parameter list stand for more names than it lists. One inside a nested `{…}` or `[…]`
/// (`shade?: {…}`, `points: [[x,y]…]`) is that value's, not the command's, and so is one where a
/// value goes (`type?: "Line"|"Circle"|...`, `space?: current|<layout>`, battle test 7 B7-019:
/// those left qselect, count and pagesetup taking any parameter).
fn open_params(doc: &str) -> bool {
    let doc = doc.split('→').next().unwrap_or(doc);
    let mut groups: Vec<char> = Vec::new();
    for (i, c) in doc.char_indices() {
        let rest = doc.get(i..).unwrap_or_default();
        // Where a name goes: right after the `{` or a `,` of the command's own list.
        let name_place = || groups == ['{'] && matches!(doc.get(..i).unwrap_or_default().trim_end().chars().last(), Some('{' | ','));
        match c {
            '{' | '[' | '(' => groups.push(c),
            '}' | ']' | ')' => {
                groups.pop();
            }
            // An ellipsis (`…`, `...`), not a range (`1..255`).
            '…' if name_place() => return true,
            '.' if rest.starts_with("...") && name_place() => return true,
            // A placeholder `<…>` closed in the same list, not a comparison.
            '<' if closes(rest) && name_place() => return true,
            _ => {}
        }
    }
    false
}

/// [`open_params`] as it was before battle test 7: an ellipsis or placeholder anywhere in the
/// top-level list, a value's place too (the job, BIM and MEP lane's commands keep it).
fn open_params_loose(doc: &str) -> bool {
    let doc = doc.split('→').next().unwrap_or(doc);
    let mut depth = 0usize;
    for (i, c) in doc.char_indices() {
        let rest = doc.get(i..).unwrap_or_default();
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            '…' if depth == 1 => return true,
            '.' if depth == 1 && rest.starts_with("...") => return true,
            '<' if depth == 1 && closes(rest) => return true,
            _ => {}
        }
    }
    false
}

/// Whether the `<` that starts `rest` is closed by a `>` before its parameter list ends.
fn closes(rest: &str) -> bool {
    let mut depth = 0usize;
    for c in rest.chars().skip(1) {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' if depth == 0 => return false,
            '}' | ']' => depth -= 1,
            '>' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// The documentation of `spec`'s parameters: `same as `other`` is the other command's.
pub fn params_doc(spec: &CommandSpec) -> &'static str {
    let doc = spec.params;
    let same = doc.trim().strip_prefix("same as `").or_else(|| doc.trim().strip_prefix("as ")).and_then(|r| r.split(['`', ' ']).next());
    match same.filter(|id| !id.is_empty()).and_then(find_command) {
        Some(other) if other.id != spec.id => other.params,
        _ => doc,
    }
}

/// Whether `spec` takes parameters beyond the ones its documentation names (its doc leaves them
/// open), so that an agent's unknown ones aren't refused.
pub fn takes_open_params(spec: &CommandSpec) -> bool {
    if in_job_lane(spec.id) { open_params_loose(params_doc(spec)) } else { open_params(params_doc(spec)) }
}

/// Command families of the job, BIM and MEP lane (the print-set pipeline and the building model):
/// the agent guard keeps its earlier, looser checks for them ([`own_params`], the wrong-typed
/// checks in [`refuse_wrong_typed_params`]); their callers are checked in that lane.
const JOB_LANE: &[&str] = &[
    "bim.",
    "bspec.",
    "mep.",
    "model.",
    "job.",
    "rooms",
    "walls.",
    "wallgen",
    "code.",
    "estimate.",
    "takeoff",
    "schedule.",
    "project.",
    "rules.",
    "validate",
    "automation.",
    "tags.",
    "site.",
    "finishes.",
    "framing.",
    "casework.",
    "envelope.",
    "plan.",
    "prints.",
    "arch.",
    "levels.",
    "stairs.",
    "unittypes.",
    "fieldview.",
    "asbuilt.",
    "override.",
    "review.",
    "rfi.",
    "callouts.",
    "cover.",
    "calibrate.",
    "fourd.",
    "p2m.",
    "lv.",
    "parts.",
    "furnish",
    "drywall.",
    "coordination.",
    "ifc.",
    "import.check",
    "sheet.facts",
    "labels.",
    "roof.",
    "grounding.",
    "wholejob.",
    "progress.",
    "printset.",
    "printrev.",
    "co.",
    "concrete.",
    "ugsheets.",
    "procure.",
    "cobie.",
    "bcf.",
    "sheetset.from_model",
];

/// Whether command `id` belongs to the job, BIM and MEP lane ([`JOB_LANE`]).
pub fn in_job_lane(id: &str) -> bool {
    JOB_LANE.iter().any(|p| id.starts_with(p))
}

/// A command's own parameters among the names its doc mentions: the ones at the top of a form
/// (`{handles?, polar: {count, angle?}}` → `handles`, `polar`), each once. A field of a value
/// given at the top (`3darray {count: 5}`) was silently ignored.
fn own_params<'a>(names: &[(&'a str, usize)]) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for (n, d) in names {
        if *d <= 1 && !out.contains(n) {
            out.push(n);
        }
    }
    out
}

/// Whether a command's doc says it takes no parameters: `{}` (or only a result, `→ […]`) before
/// any note (`hatchtoback {moved: …}` ran as if nothing was given; battle test 7 B7-019).
fn takes_nothing(doc: &str) -> bool {
    let part = doc.split('→').next().unwrap_or(doc).trim();
    !doc.trim().is_empty() && (part.is_empty() || part.starts_with("{}")) && documented_names(doc).is_empty()
}

/// The parameters given that `spec` doesn't take (none when its documentation leaves them open,
/// or names none without saying it takes none).
pub fn unknown_params(spec: &CommandSpec, params: &Value) -> Vec<String> {
    let Some(given) = params.as_object() else { return Vec::new() };
    let doc = params_doc(spec);
    let lane = in_job_lane(spec.id);
    let delegated = if lane { None } else { delegated_params(doc) };
    if takes_open_params(spec) && delegated.is_none() {
        return Vec::new();
    }
    if let Some(known) = delegated {
        return given.keys().filter(|k| !known.contains(&k.as_str()) && !ALWAYS_PARAMS.contains(&k.as_str())).cloned().collect();
    }
    let names = documented_names(doc);
    let known: Vec<&str> = if in_job_lane(spec.id) {
        // Every word its doc puts where a name or a value goes, as the reader before battle test
        // 7 took them (that lane's callers aren't checked here); none: anything goes.
        let mut all: Vec<&str> = names.iter().map(|(n, _)| *n).collect();
        all.extend(lane_words(doc));
        if all.is_empty() {
            return Vec::new();
        }
        all
    } else {
        own_params(&names)
    };
    if known.is_empty() && !takes_nothing(doc) {
        return Vec::new();
    }
    given.keys().filter(|k| !known.contains(&k.as_str()) && !ALWAYS_PARAMS.contains(&k.as_str())).cloned().collect()
}

/// The parameters a refusal offers: the command's own, each once (battle test 7 B7-020: purge
/// offered `blocks` twice and `n`, dimangular `at` three times).
pub fn offered_params(spec: &CommandSpec) -> Vec<&'static str> {
    let doc = params_doc(spec);
    match delegated_params(doc) {
        Some(names) if !in_job_lane(spec.id) => names,
        _ => own_params(&documented_names(doc)),
    }
}

/// An agent's call with parameters the command doesn't take is refused before it runs (battle
/// test 0.7.8 #9: `layout.rename {name, to}` ran without its `from` and renamed the current
/// layout). The error names them and the ones it takes.
fn refuse_unknown_params(spec: &CommandSpec, params: &Value) -> Result<()> {
    let unknown = unknown_params(spec, params);
    if unknown.is_empty() {
        return Ok(());
    }
    let known = offered_params(spec);
    if known.is_empty() {
        let names: Vec<String> = unknown.iter().map(|k| format!("`{k}`")).collect();
        return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("{} takes no parameters (got {})", spec.id, names.join(", ")) });
    }
    let list = |v: &[&str]| v.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ");
    let names: Vec<&str> = unknown.iter().map(String::as_str).collect();
    Err(EngineError::BadParams {
        cmd: spec.id.into(),
        msg: format!("{} {} not a parameter of {}; it takes {}", list(&names), if names.len() == 1 { "is" } else { "are" }, spec.id, list(&known)),
    })
}

/// What a parameter's documentation says its value is, where it says so plainly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// `radius?: number`, `count?: n`, `fade?: 0-80`, `angle?: degrees`.
    Number,
    /// `closed?: bool`.
    Bool,
    /// `text?: string`, `path: the reviewed PDF`.
    Text,
    /// A single value of a kind the doc doesn't name (`page?: 1`, `scale?: 1:50`).
    Single,
}

/// The kind the documentation `doc` gives parameter `key`, when it gives a plain one (not a
/// point, a list, an object or a choice among those).
pub fn param_kind(doc: &str, key: &str) -> Option<ParamKind> {
    let at = documented_params(doc).into_iter().find(|n| *n == key).map(|n| n.as_ptr() as usize - doc.as_ptr() as usize)?;
    let rest = doc.get(at + key.len()..)?.trim_start().strip_prefix('?').unwrap_or(doc.get(at + key.len()..)?.trim_start());
    annotation_kind(rest.trim_start().strip_prefix(':')?.trim_start())
}

/// The kind an annotation after a `:` gives (`bool`, `number (default 1)`, `degrees`), when it
/// is a plain one.
fn annotation_kind(ann: &str) -> Option<ParamKind> {
    let first = ann.split(|c: char| c.is_whitespace() || matches!(c, ',' | '}' | '|' | '(' | ')' | '/')).next().unwrap_or("");
    // A choice that includes a structure (`bool | {…}`, `number | [x,y]`) takes both.
    let tail = ann.split([',', '}']).next().unwrap_or(ann);
    if tail.contains(['{', '[']) || tail.contains('|') && !matches!(first, "true" | "false") {
        return None;
    }
    match first {
        "number" | "n" | "count" | "degrees" | "deg" | "inches" | "feet" | "ft" | "mm" => Some(ParamKind::Number),
        "bool" | "boolean" => Some(ParamKind::Bool),
        "string" | "text" => Some(ParamKind::Text),
        f if f.starts_with(|c: char| c.is_ascii_digit()) => Some(ParamKind::Single),
        _ => None,
    }
}

/// Text that reads as NaN or infinity (`"nan"`, `" -Inf"`, `"infinity"`).
fn spells_non_finite(t: &str) -> bool {
    let t = t.trim().trim_start_matches(['+', '-']).to_ascii_lowercase();
    matches!(t.as_str(), "nan" | "inf" | "infinity")
}

/// Parameter names that hold a number by their name (`textHeight`, `offset_in`, `radius`).
fn is_numeric_name(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    [
        "radius",
        "dist",
        "count",
        "scale",
        "angle",
        "height",
        "width",
        "length",
        "size",
        "offset",
        "rows",
        "columns",
        "spacing",
        "factor",
        "tolerance",
        "thickness",
        "gap",
        "depth",
        "diameter",
        "elevation",
        "limit",
        "segments",
        "step",
        "dogleg",
        "_in",
        "_ft",
        "_mm",
        // Battle test 7 B7-019: 'nan' was taken for these.
        "cols",
        "weight",
        "margin",
        "origin",
        "ratio",
        "duration",
        "number",
        "rotation",
        "lens",
        "exposure",
        "intensity",
        "latitude",
        "longitude",
        "timezone",
    ]
    .iter()
    .any(|w| k.contains(w))
        || matches!(k.as_str(), "x" | "y" | "z" | "n" | "min" | "max")
}

/// What the doc says after parameter `key` each time it names it: from the end of the name to
/// the next `,`, `;` or the end of its list (`?: bool | {width, height}`, `? (tile numbers from 1)`).
fn param_notes<'a>(doc: &'a str, key: &str) -> Vec<&'a str> {
    let base = doc.as_ptr() as usize;
    documented_names(doc)
        .into_iter()
        .filter(|(n, _)| *n == key)
        .filter_map(|(n, _)| {
            let rest = doc.get(n.as_ptr() as usize - base + n.len()..)?;
            let (mut depth, mut quoted) = (0usize, false);
            for (i, c) in rest.char_indices() {
                match c {
                    '"' => quoted = !quoted,
                    _ if quoted => {}
                    '{' | '[' | '(' => depth += 1,
                    '}' | ']' | ')' | ',' | ';' | '→' if depth == 0 => return rest.get(..i),
                    '}' | ']' | ')' => depth -= 1,
                    _ => {}
                }
            }
            Some(rest)
        })
        .collect()
}

/// The value part of a note: after the `?` and the `:` (`?: bool | {width, height}` → `bool |
/// {width, height}`); `None` for a note in brackets only (`? (save: …)`).
fn note_value(note: &str) -> Option<&str> {
    note.trim_start().trim_start_matches('?').trim_start().strip_prefix(':').map(str::trim_start)
}

/// Whether a note says the value may be an object: a `{…}` in its value, not in a note in
/// brackets after it (`load?: path (reads one: {map})`); a note in brackets alone may show one
/// (`where? ("text" | {attribute: n})`).
fn note_takes_object(note: &str) -> bool {
    match note_value(note) {
        Some(v) => without_notes(v).contains('{'),
        None => note.contains('{'),
    }
}

/// Whether a note says the value is a list only (`elements?: [{offset, color?}]`).
fn note_is_list(note: &str) -> bool {
    note_value(note)
        .is_some_and(|v| v.starts_with('[') && v.get(skip_group(v, 0, b'[', b']', false)..).is_some_and(|rest| without_notes(rest).trim().is_empty()))
}

/// The command a doc hands the rest of its parameters to: `…the parameters of plot`, `…the
/// fields of light`, `(see tablestyle.cellstyle)`.
fn delegate(doc: &str) -> Option<&'static CommandSpec> {
    delegates(doc, &["the parameters of ", "the fields of ", "(see "]).into_iter().next()
}

/// The commands named after one of `leads` (`…the parameters of arrayrect, arraypolar or
/// arraypath`).
fn delegates(doc: &str, leads: &[&str]) -> Vec<&'static CommandSpec> {
    let mut out = Vec::new();
    for lead in leads {
        let Some(at) = doc.find(lead) else { continue };
        let tail = doc.get(at + lead.len()..).unwrap_or_default();
        let tail = tail.split(['}', ')', ';', '(']).next().unwrap_or_default();
        for part in tail.split(',').flat_map(|p| p.split(" or ")).flat_map(|p| p.split(" and ")) {
            let id = part.trim().trim_matches('`');
            match find_command(id).filter(|c| c.params != doc) {
                Some(c) => out.push(c),
                None => break,
            }
        }
    }
    out
}

/// What a doc that hands its other parameters to commands of its own (`{…the parameters of plot,
/// device?}`) takes, when that is all that leaves it open: its names and theirs.
fn delegated_params(doc: &'static str) -> Option<Vec<&'static str>> {
    let leads = ["…the parameters of ", "...the parameters of ", "…the fields of ", "...the fields of "];
    let others = delegates(doc, &leads);
    if others.is_empty() || others.iter().any(|o| open_params(params_doc(o))) {
        return None;
    }
    // Open for another reason as well (a `<placeholder>`): still open.
    let mut rest = doc.to_string();
    for lead in leads {
        while let Some(at) = rest.find(lead) {
            let end = rest.get(at..).and_then(|r| r.find([',', '}'])).map_or(rest.len(), |e| at + e);
            rest.replace_range(at..end, "");
        }
    }
    if open_params(&rest) {
        return None;
    }
    let mut names = own_params(&documented_names(doc));
    for o in others {
        let theirs = own_params(&documented_names(params_doc(o)));
        names.extend(theirs.into_iter().filter(|n| !names.contains(n)).collect::<Vec<_>>());
    }
    Some(names)
}

/// A note that is only a choice of plain values (`?: on|off`, `: extents|all|window (default)`).
fn note_is_choice(note: &str) -> bool {
    let n = without_notes(note);
    let n = n.trim_start().trim_start_matches('?').trim_start();
    n.strip_prefix(':')
        .is_some_and(|list| list.contains('|') && list.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '|' | ' ' | '-' | '.' | '*')))
}

/// Whether a note says the value is a number or a point (`(tile numbers from 1)`, `[x,y]`,
/// `degrees`, `mm`).
fn note_is_numeric(note: &str) -> bool {
    let n = note.to_ascii_lowercase();
    n.contains("[x") || n.contains("number") || n.contains("degrees") || [" mm", " inches", " feet", " pixels", " dpi"].iter().any(|w| n.contains(w))
}

/// The fields a parameter's value takes when every note shows one object (`background?: {kind,
/// color?, …}`); `None` when one note allows something else or leaves its fields open.
fn value_fields(notes: &[&str]) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    for note in notes {
        let body = note_value(note)?;
        // Each alternative that is an object (`{kind, color?}`, `bool | {width, height}`); one
        // that leaves its fields open takes them from the command it names, else anything.
        let mut objects = 0;
        let mut at = 0;
        while at < body.len() {
            let alt = body.get(at..)?.trim_start();
            let start = body.len() - alt.len();
            if alt.starts_with('{') {
                let end = skip_group(body, start, b'{', b'}', false);
                let group = body.get(start..end)?;
                objects += 1;
                // `{TAG: value}`, `{key: value}`: a map whose keys are the caller's own names.
                if is_map_doc(group) {
                    return None;
                }
                if open_params(group) {
                    let other = delegate(note)?;
                    fields.extend(own_params(&documented_names(params_doc(other))).into_iter().map(str::to_string));
                }
                fields.extend(own_params(&documented_names(group)).into_iter().map(str::to_string));
                at = end;
            }
            // On to the next alternative (`|` outside any group).
            let rest = body.get(at..)?;
            let mut depth = 0usize;
            let next = rest.char_indices().find_map(|(i, c)| {
                match c {
                    '{' | '[' | '(' => depth += 1,
                    '}' | ']' | ')' => depth = depth.saturating_sub(1),
                    '|' if depth == 0 => return Some(i + 1),
                    _ => {}
                }
                None
            });
            match next {
                Some(n) => at += n,
                None => break,
            }
        }
        if objects == 0 {
            return None;
        }
    }
    (!fields.is_empty()).then_some(fields)
}

/// A one-entry `{word: …}` group: the word stands for any key and what follows is each entry's
/// value (`attribs?: {TAG: value}`, aliasedit's `set?: {alias: command}`, `visible?: {trade:
/// bool}`). A field of a value is optional or one of several (`{on?: bool}`, `{kind, color?}`).
fn is_map_doc(group: &str) -> bool {
    map_entry_doc(group).is_some()
}

/// What a map group ([`is_map_doc`]) says each entry's value is (`command`, `bool`, `[r, g, b] |
/// "#rrggbb"`).
fn map_entry_doc(group: &str) -> Option<&str> {
    let inner = group.trim().strip_prefix('{')?.strip_suffix('}')?;
    let (key, value) = inner.split_once(':')?;
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || value.is_empty() {
        return None;
    }
    // One entry only: no `,` outside the value's own groups.
    let mut depth = 0usize;
    for c in value.chars() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return None,
            _ => {}
        }
    }
    Some(value)
}

/// The entry value doc of the map a parameter's notes show ([`map_entry_doc`]); `None` when no
/// note shows one, or two notes show different ones.
fn map_value_doc<'a>(notes: &[&'a str]) -> Option<&'a str> {
    let mut found = None;
    for note in notes {
        let Some(body) = note_value(note) else { continue };
        let mut at = 0;
        while let Some(open) = body.get(at..).and_then(|r| r.find('{')) {
            let start = at + open;
            let end = skip_group(body, start, b'{', b'}', false);
            if end <= start {
                break;
            }
            if let Some(entry) = body.get(start..end).and_then(map_entry_doc) {
                if found.is_some_and(|f| f != entry) {
                    return None;
                }
                found = Some(entry);
            }
            at = end;
        }
    }
    found
}

/// An entry `name` of map parameter `k` whose value isn't what the map's doc `entry` says (a list
/// or an object where it shows a single value, text where it shows a number…).
fn wrong_map_entry(k: &str, name: &str, v: &Value, entry: &str) -> Option<String> {
    if v.is_null() {
        return None;
    }
    let plain = without_notes(entry);
    let want = match annotation_kind(entry) {
        Some(ParamKind::Number) if !v.is_number() => "a number",
        Some(ParamKind::Bool) if !(v.is_boolean() || v.is_number() || v.is_string()) => "true or false",
        Some(ParamKind::Text) if !v.is_string() => "a string",
        _ if v.is_object() && !plain.contains('{') => "a single value, not an object",
        _ if v.is_array() && !plain.contains('[') => "a single value, not a list",
        _ => return None,
    };
    Some(format!("`{k}` entry `{name}` must be {want}"))
}

/// Parameter names that can't be negative: a radius, a size, a count, a gap, a tolerance.
fn is_non_negative_name(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["radius", "diameter", "height", "width", "size", "count", "segments", "gap", "dogleg", "tolerance", "limit"].iter().any(|w| k.contains(w))
}

/// An agent's call with a parameter of the wrong JSON type is refused before it runs, naming the
/// parameter and the type it takes (battle test 5 B7-019: `mleaderstyle {arrowSize: {}}` and
/// `purge {layers: 12345}` ran and used the default). Only where the documentation says plainly
/// what the value is ([`param_kind`]); `null` is "not given".
fn refuse_wrong_typed_params(spec: &CommandSpec, params: &Value) -> Result<()> {
    let Some(given) = params.as_object() else { return Ok(()) };
    let doc = params_doc(spec);
    for (k, v) in given {
        if v.is_null() {
            continue;
        }
        let kind = param_kind(doc, k);
        let lane = !in_job_lane(spec.id);
        let notes = if lane { param_notes(doc, k) } else { Vec::new() };
        // A number spelt as text that isn't finite ('nan', '-inf') was silently dropped (battle
        // test 6 B7-019): refused for every parameter that takes a number.
        let numeric = kind.is_some() || is_numeric_name(k) || notes.iter().any(|n| note_is_numeric(n));
        if kind != Some(ParamKind::Text) && numeric && v.as_str().is_some_and(spells_non_finite) {
            let want = if kind == Some(ParamKind::Bool) { "true or false" } else { "a finite number" };
            return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("`{k}` must be {want}, not {v}") });
        }
        // A size, count or tolerance below zero was dropped or replaced by the default.
        if is_non_negative_name(k) && v.as_f64().is_some_and(|n| n < 0.0) {
            return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("`{k}` must be 0 or more (got {v})") });
        }
        // An object or a list where the doc shows a single value was read as "not given" (battle
        // test 7 B7-019: 603 of 3,896 object values ran in 225 commands).
        if lane && kind.is_none() && !notes.is_empty() {
            if let Some(o) = v.as_object() {
                let list_only = notes.iter().all(|n| note_is_list(n));
                if list_only || !notes.iter().any(|n| note_takes_object(n)) {
                    let want = if list_only {
                        "a list"
                    } else if notes.iter().any(|n| n.contains('[')) {
                        "a list or a single value"
                    } else {
                        "a single value"
                    };
                    return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("`{k}` must be {want}, not an object") });
                }
                // A map (`set?: {alias: command}`): any key, each value as the doc says.
                if let Some(entry) = map_value_doc(&notes) {
                    if let Some(msg) = o.iter().find_map(|(name, v)| wrong_map_entry(k, name, v, entry)) {
                        return Err(EngineError::BadParams { cmd: spec.id.into(), msg });
                    }
                } else if let Some(fields) = value_fields(&notes)
                    && let Some(bad) = o.keys().find(|f| !fields.contains(f))
                {
                    let list = fields.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
                    return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("`{k}` has no field `{bad}`; it takes {list}") });
                }
            }
            if v.is_array() && notes.iter().all(|n| note_is_choice(n)) {
                return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("`{k}` must be a single value, not a list") });
            }
        }
        let Some(kind) = kind else { continue };
        let ok = match kind {
            ParamKind::Number => v.is_number(),
            ParamKind::Bool => v.is_boolean() || v.is_number() || v.is_string(),
            ParamKind::Text => v.is_string(),
            ParamKind::Single => !v.is_object() && !v.is_array(),
        };
        if !ok {
            let want = match kind {
                ParamKind::Number => "a number",
                ParamKind::Bool => "true or false",
                ParamKind::Text => "a string",
                ParamKind::Single => "a single value, not a list or an object",
            };
            return Err(EngineError::BadParams { cmd: spec.id.into(), msg: format!("`{k}` must be {want}") });
        }
    }
    Ok(())
}

/// A command's JSON result with `warnings` naming parameters it doesn't take (a misspelt
/// `raduis` was silently ignored; tester 0.7.7e). It still ran: they only warn (the app's own
/// calls; an agent's are refused, [`refuse_unknown_params`]).
fn with_unknown_param_warnings(mut v: Value, spec: &CommandSpec, params: &Value) -> Value {
    let unknown = unknown_params(spec, params);
    if unknown.is_empty() {
        return v;
    }
    let msg = |k: &String| format!("`{k}` is not a parameter of {} (it was ignored); see list_commands for its parameters", spec.id);
    match &mut v {
        Value::Object(o) => {
            let w = o.entry("warnings").or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(a) = w {
                a.extend(unknown.iter().map(|k| Value::String(msg(k))));
            }
        }
        Value::Null => v = serde_json::json!({ "warnings": unknown.iter().map(msg).collect::<Vec<_>>() }),
        _ => {}
    }
    v
}

/// A pending window/crossing selection started by a click on empty space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PendingWindow {
    pub corner: Vec2,
    pub during_command: bool,
}

/// Options and values the interactive commands remember for the rest of the session, offered as
/// the `<default>` the next time they ask (as AutoCAD does). Values AutoCAD keeps in a drawing
/// system variable (HPNAME, OFFSETDIST, FILLETRAD, CHAMFERA, POLYSIDES…) live in the drawing
/// header instead. JSON calls never read these: their explicit parameters and documented defaults
/// stay authoritative.
#[derive(Clone, Debug, PartialEq)]
pub struct LastUsed {
    /// HATCH/GRADIENT starts at "Select objects" instead of "Pick internal point".
    pub hatch_select: bool,
    /// POLYGON: circumscribed about the circle (the `C` option) instead of inscribed.
    pub polygon_circumscribed: bool,
    /// ROTATE angle (radians).
    pub rotate_angle: f64,
    /// SCALE factor.
    pub scale_factor: f64,
    /// MATCHPROP Settings: the properties it copies.
    pub matchprop: cmd::MatchProps,
    /// MLEADER placement order and Options.
    pub mleader: MLeaderOptions,
    /// MLEADER's block content name (Options > Content type > Block).
    pub mleader_block: String,
    /// The last value entered for each attribute of a library symbol ("EL_SWITCH|CIRCUIT"): the
    /// next placement offers it, and fills its hidden fields (PANEL, MOUNT_HT) with it.
    pub symbol_attribs: std::collections::HashMap<String, String>,
    /// The last AREA measured (AREA and PERIMETER) and the last DIST (DISTANCE).
    pub area: f64,
    pub perimeter: f64,
    pub distance: f64,
    /// The last height a solid command (BOX, EXTRUDE…) used: its next `<default>`.
    pub solid_height: Option<f64>,
    /// RECTANG's chamfer, fillet, width, elevation, thickness and rotation.
    pub rectang: RectangModes,
}

/// The modes RECTANG keeps from one rectangle to the next, as AutoCAD 2024 does. Chamfer and
/// fillet exclude each other: setting one clears the other.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectangModes {
    /// First and second chamfer distances.
    pub chamfer: (f64, f64),
    pub fillet: f64,
    pub width: f64,
    pub elevation: f64,
    pub thickness: f64,
    /// Radians.
    pub rotation: f64,
}

impl Default for LastUsed {
    fn default() -> Self {
        LastUsed {
            hatch_select: false,
            polygon_circumscribed: false,
            rotate_angle: 0.0,
            scale_factor: 1.0,
            matchprop: cmd::MatchProps::default(),
            mleader: MLeaderOptions::default(),
            mleader_block: String::new(),
            symbol_attribs: std::collections::HashMap::new(),
            area: 0.0,
            perimeter: 0.0,
            distance: 0.0,
            solid_height: None,
            rectang: RectangModes::default(),
        }
    }
}

/// Which part of a multileader MLEADER places first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeaderOrder {
    #[default]
    Arrowhead,
    Landing,
    Content,
}

/// The MLEADER placement order and the values set at its Options prompt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MLeaderOptions {
    pub order: LeaderOrder,
    /// Maximum points of the leader line (2..=64).
    pub max_points: usize,
    /// First and second angle constraints (radians; 0 = free).
    pub angles: [f64; 2],
    /// Draw the landing line (dogleg).
    pub landing: bool,
    /// Ask for multiline text content (Content type None turns it off).
    pub content: bool,
    /// Leader lines: straight, spline, or none (content only).
    pub leader_type: LeaderType,
    /// The content is a block (`LastUsed::mleader_block`) instead of multiline text.
    pub block: bool,
    /// The block's extents centre (not its insertion point) sits at the end of the landing.
    pub block_center: bool,
}

/// MLEADER's leader line type (Options > Leader type).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeaderType {
    #[default]
    Straight,
    Spline,
    None,
}

impl Default for MLeaderOptions {
    fn default() -> Self {
        MLeaderOptions {
            order: LeaderOrder::Arrowhead,
            max_points: 2,
            angles: [0.0; 2],
            landing: true,
            content: true,
            leader_type: LeaderType::Straight,
            block: false,
            block_center: true,
        }
    }
}

/// A finished render (RENDER, RENDERCROP).
#[derive(Clone, Debug)]
pub struct RenderResult {
    pub image: Arc<cadaddy_render::images::Raster>,
    pub samples: u32,
    pub seconds: f64,
    pub seconds_per_sample: f64,
    /// Bumped by every render, so a viewer knows to reload.
    pub serial: u64,
}

/// The editor session: open drawings, settings, the running command and the command log.
pub struct Session {
    pub docs: Vec<DocState>,
    pub active: usize,
    pub settings: Settings,
    pub running: Option<Running>,
    /// See [`Session::keep_undo_step`].
    keep_undo_step: bool,
    /// MULTIPLE: the command started again each time it ends, until it is cancelled.
    pub(crate) multiple: Option<String>,
    /// A script is running ([`Session::script`]); a SCRIPT command inside it is refused.
    pub(crate) in_script: bool,
    /// Set by an interface just before it starts TABLEDIT for a person (a double-click on a
    /// cell, a table just inserted): that TABLEDIT walks on to the next cell after each one, as
    /// Tab does in AutoCAD's cell editor, starting at `Some(cell)` when given. Scripts and agents
    /// never set it, so their TABLEDIT edits one cell.
    pub tabledit_walk: Option<Option<(Handle, usize, usize)>>,
    /// The last script run (RSCRIPT at the command line runs it again).
    pub script_last: Option<String>,
    /// The script a failed line stopped: its lines, the next one, how deep it was called.
    pub script_interrupted: Option<(Vec<String>, usize, usize)>,
    /// SYSVARMONITOR's watched variables.
    pub monitor: script_tools::Monitor,
    /// The watched variables' values at the last look (changes are reported once).
    pub monitor_seen: std::collections::BTreeMap<String, serde_json::Value>,
    /// A LISP file or script outside the trusted folders that wasn't loaded, for the app to ask
    /// about ([`secureload`]); taken by the app.
    pub secureload_ask: Option<secureload::Ask>,
    /// Files the user allowed to load this session (normalized paths).
    pub(crate) secureload_once: std::collections::HashSet<String>,
    /// The command suspended while a transparent command (`'ZOOM`, `'PAN`…) prompts inside it;
    /// it resumes when that command ends. One level, as in AutoCAD.
    pub suspended: Option<Running>,
    pub log: Vec<String>,
    /// The session as a script ([`journal`]): each command started, each input and each JSON call
    /// made from outside, one per line, so replaying it with SCRIPT redoes the work.
    pub journal: journal::Journal,
    /// The Action Recorder's macros and recording ([`cmd::actions`]).
    pub actions: cmd::actions::Actions,
    pub last_command: Option<String>,
    /// LASTPOINT.
    pub last_point: Vec2,
    /// The last point in space given ([`Input::Point3`]): `@dx,dy,dz` is from it.
    pub last_point3: cadaddy_geom::Vec3,
    /// The last segment given at a point prompt (its base point and the point): polar angles
    /// relative to the last segment (POLARMODE bit 1) are measured from it.
    pub last_segment: Option<(Vec2, Vec2)>,
    /// The last typed `@distance<angle` input, as `(distance, angle in radians)`.
    /// Set by [`Session::typed`] and consumed by commands that resolve a deferred
    /// tangent first point (e.g. [`cmd::draw::LineM`]) when the next point is typed
    /// rather than clicked. Cleared at [`Session::start`].
    pub last_polar: Option<(f64, f64)>,
    /// Point modifiers pending at the current point prompt (`FROM`, `M2P`, `.x`, `<a`…).
    pub point_mods: Vec<pointmod::Frame>,
    /// Cursor position in world coordinates (from the UI; used for direct distance entry).
    pub cursor: Vec2,
    /// The deferred tangent/perpendicular snap under the cursor, if any (from the UI; lets the
    /// rubber band show the line it resolves to).
    pub cursor_deferred: Option<snap::Deferred>,
    /// Whether Ctrl is held right now (from the UI, overwritten every frame). Commands that
    /// change behavior on Ctrl — currently only ARC's "hold Ctrl to switch direction" — read
    /// this instead of receiving modifiers through `Input`, which stays free of UI state.
    pub ctrl_down: bool,
    /// Origin of the point currently being fed by `input_snapped`; never reused by typed points.
    picked_viewport: Option<snap::ViewportPoint>,
    /// The last drawing comparison (COMPARE), drawn over the drawing it was made for.
    pub compare: Option<cmd::file::compare::Comparison>,
    /// Object snap tracking: acquired points and the path under the cursor (from the UI's
    /// [`Session::snap_cursor`] calls; direct distance entry follows the path).
    pub tracking: snap::tracking::Tracker,
    /// Viewport size in pixels (from the UI; used for zoom and pick apertures).
    pub viewport_px: (f64, f64),
    /// Pixels at the top and bottom of the viewport that the interface covers (the floating
    /// command line and its history): zooming to the extents fits the drawing between them.
    pub viewport_inset_px: (f64, f64),
    pub clipboard: Vec<Entity>,
    pub clipboard_base: Vec2,
    pub pending_window: Option<PendingWindow>,
    /// A selection mode typed at "Select objects" (W, C, F, WP, CP) collecting its points.
    pub select_mode: Option<select::SelectMode>,
    /// SIngle typed at "Select objects": the next selection ends the prompt.
    pub select_single: bool,
    pub untitled_counter: u32,
    /// The last dimension created (DIMCONTINUE / DIMBASELINE).
    pub last_dim: Option<Handle>,
    /// Options remembered between invocations of interactive commands.
    pub last_used: LastUsed,
    /// The last RENDER's image and what it took (the Render window shows it).
    pub last_render: Option<RenderResult>,
    /// Where `clipboard` came from and the layers, styles and blocks its objects need.
    pub clipboard_source: Option<cmd::clipboard::ClipSource>,
    /// What the operating system's clipboard holds that isn't CADaddy objects (an image or text
    /// copied in another program), as the front end last read it: PASTESPEC and PASTECLIP paste it.
    pub system_clipboard: cmd::clipboard::SystemContents,
    /// The window last picked with PLOTWINDOW (the Plot dialog's "Window <").
    pub plot_window: Option<cadaddy_geom::Bounds2>,
    /// AutoLISP: each drawing's symbols and the routine running ([`lisp`]).
    lisp: lisp::LispState,
    /// How the last LISP run ended (its value or error message).
    lisp_last: Option<std::result::Result<lisp::value::Value, String>>,
    /// What the last HATCHPICK kept (the Hatch and Gradient dialog's picks).
    pub hatch_picks: Option<cmd::hatch::HatchPicks>,
    /// The object named with a point for the input a prompt picking one object is taking
    /// ([`Session::input_object`]): a pick at exactly that point picks that object
    /// ([`Session::pick_object`]).
    pick_hint: Option<(Handle, Vec2)>,
    /// Reads electrical prints and returns element tags; set by whoever has the reader pipeline.
    pub tag_reader: Option<cadaddy_walk3d::tags::TagReaderFn>,
    /// Safety rails for agents: requests, dry run, confirmations, the change journal.
    pub agent: agent::AgentGuard,
    /// What happened, for agents watching (`events.subscribe` / `events.poll`).
    pub events: events::EventLog,
    /// Slow checks after a save run on a worker thread (the app turns this on): the first DWG or
    /// DXF save of a drawing returns once the file is written, and what that file doesn't keep
    /// comes later ([`Self::poll_background`]). Off (scripts, the CLI, tests), the save's result
    /// holds it.
    pub background_checks: bool,
    /// Drywall view mode: walls coloured by type, fire rating, board layers or height rule.
    pub drywall_viewmode: Option<cmd::drywall::ViewMode>,
    /// Pre-computed handle→colour map for the drywall view mode (for the renderer).
    pub drywall_color_overrides: std::collections::BTreeMap<cadaddy_doc::Handle, [u8; 3]>,
    /// Layer boundary lines for the renderer: each entry is two 3D points forming a line segment.
    pub drywall_layer_lines: Vec<([f64; 3], [f64; 3])>,
    /// The active project (Q325): its manifest, which drawing is open.
    pub project: Option<cmd::project::ProjectState>,
    /// The settings folder holding the project registry and the company standards library
    /// (`projects.json`, `standards.json`). `None`: the user's settings folder.
    pub project_config: Option<std::path::PathBuf>,
    /// The open print-set project (Q310): classification, stages, dashboard.
    pub printset_project: Option<Box<cadaddy_mepfix::printset::PrintSetProject>>,
    /// The job's framing (Q5103), built from its sheets with the field points and shop drawings applied.
    pub framing: Option<Box<cmd::framing::Cache>>,
    /// Where `printset.keep` notes the last print set kept, for the next session's
    /// `printset.resume {}` (Q5276). `None`: `last-printset.txt` in the settings folder.
    pub printset_pointer: Option<std::path::PathBuf>,
    /// Rhai scripting engine state (scope and engine; see [`rhai_engine`]).
    pub rhai: rhai_engine::RhaiState,
    /// Sandboxed WASM plug-ins (desktop only; the web version runs in the browser's sandbox).
    #[cfg(not(target_arch = "wasm32"))]
    pub plugins: plugins::PluginHost,
    /// Plug-in commands running now, one inside another's host call ([`plugins::MAX_PLUGIN_DEPTH`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) plugin_depth: u32,
    /// Inside [`Session::as_one_undo_step`]: the undoable commands run record no step of their
    /// own (the calling command's one step holds them all).
    pub(crate) one_undo_step: u32,
    /// `model.issues`' findings for one revision of one drawing: its pages and kinds are read
    /// from them, not found again (a print set's checks took two minutes a call).
    pub(crate) issues_cache: Option<Box<cmd::verify::IssuesCache>>,
    /// An associative model is being built again ([`cmd::wallgen::update_model`]): the commands
    /// the build runs don't start another.
    pub(crate) model_updating: bool,
}

/// Give a new drawing its own FINGERPRINTGUID (as AutoCAD does): kept in DWG, DXF and .cdy, it
/// tells drawings apart (IFC export seeds its GlobalIds with it, so two drawings' never collide).
pub fn fresh_fingerprint(d: &mut Drawing) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |t| t.as_nanos() as u64);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let seed = nanos ^ n.rotate_left(17);
    let (hi, lo) = (mix(seed), mix(seed ^ 0xA5A5_5A5A_DEAD_BEEF));
    let g = format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        hi >> 32,
        (hi >> 16) & 0xFFFF,
        // Version 4 (random) and the RFC 4122 variant.
        (hi & 0x0FFF) | 0x4000,
        ((lo >> 48) & 0x3FFF) | 0x8000,
        lo & 0xFFFF_FFFF_FFFF
    );
    d.header.set_str("FINGERPRINTGUID", &g);
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

/// CMATERIAL as an object's material: `None` for ByLayer, and for a value that names no material
/// of the drawing (files keep CMATERIAL as a handle).
fn current_material(d: &Drawing) -> Option<String> {
    let m = d.header.str("CMATERIAL", cadaddy_doc::material::BY_LAYER);
    if m.eq_ignore_ascii_case(cadaddy_doc::material::BY_BLOCK) {
        return Some(cadaddy_doc::material::BY_BLOCK.into());
    }
    d.material(&m).map(|x| x.name.clone())
}

impl Session {
    /// Whether a script is running (its commands are the script's, not a person's).
    pub fn in_script(&self) -> bool {
        self.in_script
    }
    pub fn new() -> Self {
        let mut s = Session::empty();
        s.new_drawing(false);
        s
    }
    /// A session with no open drawing (the Start tab).
    pub fn empty() -> Self {
        Session {
            docs: Vec::new(),
            active: 0,
            settings: Settings::default(),
            running: None,
            keep_undo_step: false,
            multiple: None,
            in_script: false,
            tabledit_walk: None,
            script_last: None,
            script_interrupted: None,
            monitor: script_tools::Monitor::default(),
            monitor_seen: Default::default(),
            secureload_ask: None,
            secureload_once: Default::default(),
            suspended: None,
            log: Vec::new(),
            journal: journal::Journal::default(),
            actions: cmd::actions::Actions::default(),
            last_command: None,
            last_point: Vec2::ZERO,
            last_point3: cadaddy_geom::Vec3::ZERO,
            last_segment: None,
            last_polar: None,
            point_mods: Vec::new(),
            cursor: Vec2::ZERO,
            cursor_deferred: None,
            ctrl_down: false,
            picked_viewport: None,
            compare: None,
            tracking: snap::tracking::Tracker::default(),
            viewport_px: (1200.0, 800.0),
            viewport_inset_px: (0.0, 0.0),
            clipboard: Vec::new(),
            clipboard_base: Vec2::ZERO,
            pending_window: None,
            select_mode: None,
            select_single: false,
            untitled_counter: 0,
            last_dim: None,
            last_used: LastUsed::default(),
            last_render: None,
            clipboard_source: None,
            system_clipboard: cmd::clipboard::SystemContents::default(),
            plot_window: None,
            lisp: lisp::LispState::default(),
            lisp_last: None,
            hatch_picks: None,
            pick_hint: None,
            tag_reader: None,
            agent: agent::AgentGuard::default(),
            events: events::EventLog::default(),
            background_checks: false,
            drywall_viewmode: None,
            drywall_color_overrides: std::collections::BTreeMap::new(),
            drywall_layer_lines: Vec::new(),
            project: None,
            printset_project: None,
            framing: None,
            printset_pointer: None,
            project_config: None,
            rhai: rhai_engine::RhaiState::new(),
            #[cfg(not(target_arch = "wasm32"))]
            plugins: plugins::PluginHost::new(),
            #[cfg(not(target_arch = "wasm32"))]
            plugin_depth: 0,
            one_undo_step: 0,
            issues_cache: None,
            model_updating: false,
        }
    }
    /// Take the results of background checks that have finished: what a DWG or DXF save didn't
    /// keep becomes the drawing's exchange notice and a command-line line. Cheap when none run.
    pub fn poll_background(&mut self) {
        let mut lines = Vec::new();
        for st in &mut self.docs {
            let Some(done) = st.kept_job.as_ref().and_then(cmd::file::KeptJob::take) else { continue };
            st.kept_job = None;
            if done.succeeded && !st.kept_checked.contains(&done.key) {
                st.kept_checked.push(done.key);
            }
            if let Some((notice, line)) = done.notice {
                st.exchange_notice = Some(notice);
                lines.push(line);
            }
        }
        for l in lines {
            self.echo(l);
        }
    }

    /// When the plan's edits have left the active drawing's associative model behind and nothing
    /// has been edited for `idle`, build it again ([`cmd::wallgen::update_model`]). The app's frame
    /// loop and the MCP server call it while waiting. Returns how long until it is due when a
    /// model is behind but editing stopped less than `idle` ago (wake up then), else `None`.
    pub fn update_model_when_idle(&mut self, idle: std::time::Duration) -> Option<std::time::Duration> {
        let st = self.state().ok()?;
        if !st.model_behind.holds_doc(&st.doc) {
            return None;
        }
        let since = st.model_behind.since_last_edit();
        if since < idle {
            return Some(idle - since);
        }
        cmd::wallgen::update_model(self);
        None
    }

    pub fn new_drawing(&mut self, metric: bool) -> usize {
        self.add_new_drawing(metric, false)
    }
    /// NEW without a template: a new drawing whose layouts each have a viewport, like AutoCAD's
    /// default template. ([`Self::new_drawing`] leaves the layouts empty.)
    pub fn new_drawing_with_viewports(&mut self, metric: bool) -> usize {
        self.add_new_drawing(metric, true)
    }
    fn add_new_drawing(&mut self, metric: bool, viewports: bool) -> usize {
        self.untitled_counter += 1;
        let mut d = if metric { Drawing::new_metric() } else { Drawing::new_imperial() };
        fresh_fingerprint(&mut d);
        if viewports {
            cmd::layout::default_viewports(&mut d);
        }
        let mut st = DocState::new(d, &format!("Drawing{}", self.untitled_counter), None);
        st.set_view_quiet(if metric { View { center: Vec2::new(210.0, 148.5), height: 297.0 } } else { View::default() });
        self.end_running();
        self.docs.push(st);
        self.active = self.docs.len() - 1;
        self.active
    }
    pub fn open_drawing(&mut self, d: Drawing, title: &str, path: Option<String>) -> usize {
        // Its layouts with nothing on them get a viewport when they are first opened.
        let bare = cmd::layout::bare_layouts(&d);
        let mut st = DocState::new(d, title, path);
        st.bare_layouts = bare;
        let ext = st.doc.extents(&Space::Model);
        if !ext.is_empty() {
            st.set_view_quiet(framed(ext, self.viewport_px, self.viewport_inset_px, |a| a, 1.1));
        }
        // The model view's twist the file saved (VPORT group 51 of its `*Active` viewport), the
        // extents fitted as the turned view sees them.
        let twist = st.doc.vports.iter().find(|c| c.is_active()).and_then(|c| c.tiles.first()).map_or(0.0, |t| t.twist);
        if twist != 0.0 && st.space == Space::Model && st.set_twist(twist) && !ext.is_empty() {
            let (c, w, h) = st.seen_box(ext);
            let seen = Bounds2::new(c - Vec2::new(w / 2.0, h / 2.0), c + Vec2::new(w / 2.0, h / 2.0));
            st.set_view_quiet(framed(seen, self.viewport_px, self.viewport_inset_px, |a| a, 1.1));
        }
        self.end_running();
        self.docs.push(st);
        self.active = self.docs.len() - 1;
        self.active
    }
    /// A command still waiting for input belongs to the drawing it started in: end it there
    /// before another drawing becomes current, so its undo step lands in that drawing.
    fn end_running(&mut self) {
        if self.running.is_some() {
            self.cancel();
        }
    }

    /// Add a file read off-thread with [`cmd::file::read_file`] as the active drawing (OPEN's fast
    /// half). A panic while adding it is reported as an error, never propagated.
    pub fn finish_open(&mut self, f: cmd::file::ReadFile) -> Result<Value> {
        let n = self.docs.len();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd::file::finish_open(self, f))) {
            Ok(r) => {
                // Opened from the front end (not a command): `drawing_opened` handlers run now.
                if r.is_ok() && self.journal.is_outer() && self.rhai_watching() {
                    self.rhai_dispatch_pending();
                }
                r
            }
            Err(p) => {
                let msg = guard::message(&*p);
                crash_report::record(self, "open", &msg);
                self.docs.truncate(n);
                self.active = self.active.min(self.docs.len().saturating_sub(1));
                Err(EngineError::Internal("open".into(), msg))
            }
        }
    }

    pub fn state(&self) -> Result<&DocState> {
        self.docs.get(self.active).ok_or(EngineError::NoDocument)
    }
    pub fn state_mut(&mut self) -> Result<&mut DocState> {
        self.docs.get_mut(self.active).ok_or(EngineError::NoDocument)
    }
    pub fn doc(&self) -> Result<&Drawing> {
        Ok(&self.state()?.doc)
    }
    /// Copy-on-write access to the drawing.
    pub fn doc_mut(&mut self) -> Result<&mut Drawing> {
        Ok(Arc::make_mut(&mut self.state_mut()?.doc))
    }
    /// Settings for the active drawing, without caching drawing variables in the profile.
    pub fn drafting_settings(&self) -> Settings {
        let mut settings = self.settings.clone();
        if let Ok(d) = self.doc() {
            settings.snapunit = d.header.snap_unit();
            settings.gridunit = d.header.grid_unit();
        }
        settings
    }
    /// The space commands act on (model space while working inside a layout viewport).
    pub fn space(&self) -> Space {
        self.state().map(|s| s.edit_space()).unwrap_or_default()
    }
    /// The tab being displayed: Model or a layout, regardless of MSPACE.
    pub fn layout_space(&self) -> Space {
        self.state().map(|s| s.space.clone()).unwrap_or_default()
    }
    pub fn selection(&self) -> Vec<Handle> {
        self.state().map(|s| s.selection.clone()).unwrap_or_default()
    }
    pub fn set_selection(&mut self, sel: Vec<Handle>) {
        if let Ok(st) = self.state_mut() {
            let mut seen = std::collections::HashSet::new();
            st.selection = sel.into_iter().filter(|h| seen.insert(*h)).collect();
        }
    }

    /// Message on the command line history.  Control characters are stripped so the log
    /// never injects raw C0 bytes into JSON responses that carry it (`cmdline.input`'s
    /// `output` array, `cmdline.state`'s `history`).
    pub fn echo(&mut self, msg: impl Into<String>) {
        let mut m = msg.into();
        json_clean::sanitize_string_in_place(&mut m);
        log::debug!("{m}");
        self.log.push(m);
        if self.log.len() > 5000 {
            self.log.drain(..1000);
        }
    }

    /// Common properties for new objects (CLAYER, CECOLOR, CELTYPE, CELWEIGHT, CELTSCALE,
    /// CMATERIAL).
    pub fn current_common(&self) -> Common {
        let Ok(d) = self.doc() else { return Common::default() };
        let h = &d.header;
        Common {
            layer: h.str("CLAYER", "0").into(),
            color: cadaddy_color::Color::from_aci(h.i64("CECOLOR", 256) as i16),
            linetype: h.str("CELTYPE", "ByLayer").into(),
            lineweight: cadaddy_doc::Lineweight::from_dxf(h.i64("CELWEIGHT", -1) as i16),
            ltscale: h.f64("CELTSCALE", 1.0),
            plot_style: h.str("CPLOTSTYLE", "ByLayer").into(),
            material: current_material(d).into(),
            ..Common::default()
        }
    }

    /// Add an entity to the current space with current properties.
    pub fn add_entity(&mut self, kind: EntityKind) -> Result<Handle> {
        let common = self.current_common();
        let space = self.space();
        let true_color = self.doc()?.header.get("CECOLOR_RGB").and_then(|v| v.as_i64());
        let mut common = common;
        if let Some(rgb) = true_color {
            common.color = cadaddy_color::Color::True(cadaddy_color::Rgb::from_u32(rgb as u32));
        }
        // Text, dimensions, multileaders, hatches and block references of annotative styles are
        // annotative, supporting the current annotation scale.
        let d = self.doc()?;
        if d.annotative_by_style(&kind) {
            common.anno_scales = vec![d.current_anno_scale()].into();
        }
        // Lines, arcs, circles, points, polylines and 2D solids take the current 3D thickness
        // (THICKNESS). RECTANG sets its own, and text never takes it.
        if matches!(
            kind,
            EntityKind::Line(_)
                | EntityKind::Arc(_)
                | EntityKind::Circle(_)
                | EntityKind::Point(_)
                | EntityKind::LwPolyline(_)
                | EntityKind::Solid(_)
        ) {
            common.thickness = Some(d.header.f64("THICKNESS", 0.0)).filter(|t| t.is_finite()).unwrap_or(0.0);
        }
        // DIMLAYER: new dimensions on their own layer (".": the current one).
        let dim_layer =
            matches!(kind, EntityKind::Dimension(_)).then(|| d.header.str("DIMLAYER", ".")).filter(|l| !l.trim().is_empty() && l.trim() != ".");
        if let Some(l) = dim_layer {
            self.doc_mut()?.ensure_layer(l.trim());
            common.layer = l.trim().into();
        }
        let h = self.doc_mut()?.add(&space, common, kind)?;
        Ok(h)
    }

    /// World units per pixel at the current view.
    pub fn pixel_size(&self) -> f64 {
        let v = self.state().map(|s| s.view()).unwrap_or_default();
        v.height / self.viewport_px.1.max(1.0)
    }

    // ---------------- programmatic execution ----------------

    /// Run a command with JSON parameters (no dialogs). Records undo when the drawing changes.
    ///
    /// The command itself is guarded below, where its own panic can be told apart. This guard is
    /// for everything around it: the passes after a command (constraints, associativity, groups)
    /// are as able to panic as the command, and a panic there restores the drawing the same way.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let before = self.state().ok().map(|s| (s.doc.clone(), s.selection.clone(), s.uid));
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.execute_guarded(id, params))) {
            Ok(r) => r,
            Err(p) => {
                let msg = crate::guard::message(&*p);
                crash_report::record(self, id, &msg);
                if let Some((doc, sel, uid)) = before
                    && let Some(st) = self.docs.iter_mut().find(|d| d.uid == uid)
                {
                    st.doc = doc;
                    st.selection = sel;
                }
                Err(EngineError::Internal(id.into(), msg))
            }
        }
    }

    fn execute_guarded(&mut self, id: &str, params: &Value) -> Result<Value> {
        self.poll_background();
        // An agent's outermost call: parameters the command doesn't take are refused (before a
        // dry run too), then a dry run or a confirmation ([`agent`]).
        if self.journal.is_outer()
            && self.agent.from_agent
            && let Some(spec) = find_command(&cmd::resolve_alias(id))
        {
            refuse_unknown_params(spec, params)?;
            refuse_wrong_typed_params(spec, params)?;
        }
        if self.journal.is_outer()
            && let Some(r) = self.guard_call(id, params)
        {
            return r;
        }
        let outer = self.journal.enter();
        // A call from outside starts outside any one-step group (one a failed command left).
        if outer {
            self.one_undo_step = 0;
        }
        if outer && find_command(&cmd::resolve_alias(id)).is_some_and(|c| c.undoable) {
            self.note_selection_gap();
        }
        // A call from outside while a command runs (an agent's `engine.execute` while the user
        // is in TEXT): the command's work so far, the call's and the command's rest are undo
        // steps of their own, so U never takes the call's work away with the command's.
        // A call that records no undo step of its own (queries, `document.list`, the MCP server's
        // read after every tool call) splits nothing unless it changes the drawing: a typed
        // command stays one undo step however many calls come between its inputs.
        let lazy = outer && self.running.is_some() && find_command(&cmd::resolve_alias(id)).is_none_or(|c| !c.undoable && !c.restores_history);
        let lazy_pre = if lazy { self.state().ok().map(|st| (st.uid, Arc::clone(&st.doc), st.selection.clone())) } else { None };
        let uid = if outer && !lazy && self.running.is_some() { self.split_running_undo() } else { None };
        // An agent's or script's call of a command that edits the drawing sets off reactors, as
        // the same command typed does (`start`).
        let react = outer && find_command(id).is_some_and(|c| c.undoable);
        if react {
            self.lisp_command_event(lisp::reactor::CommandEvent::WillStart, id);
        }
        // An agent's edit of a drawing whose prints gate edits: re-checked round what it changed.
        let gate = if outer && self.agent.from_agent && !self.agent.dry_run { cmd::validate::gate_before(self, id) } else { None };
        // Rhai event handlers hear about the command (a script's own calls are told by `command()`).
        let rhai_watch = outer && self.rhai_watching() && !cmd::rhai_cmd::is_rhai(id);
        let rhai_before = if rhai_watch { self.rhai_before() } else { None };
        let mut r = self.execute_inner(id, params);
        if let Some(g) = gate {
            r = cmd::validate::gate_after(self, g, r);
        }
        if let Some(uid) = uid {
            self.rebase_running(uid);
        }
        if let Some(pre) = lazy_pre {
            self.split_running_undo_after(id, pre);
        }
        self.journal.leave();
        if outer {
            self.journal_selection();
        }
        if react {
            let event = if r.is_ok() { lisp::reactor::CommandEvent::Ended } else { lisp::reactor::CommandEvent::Failed };
            self.lisp_command_event(event, id);
        }
        if outer && r.is_ok() {
            self.journal.call(id, params);
        }
        // A project-scoped answer names its project (print set, briefing, BIM job, review).
        let r = r.map(|v| cmd::project_hub::tag_project(self, id, v));
        if rhai_watch {
            self.rhai_command_ended(id, rhai_before, r.is_ok());
        }
        if outer && self.rhai_watching() {
            self.rhai_dispatch_pending();
        }
        // Events reads (and subscribing) aren't news themselves, nor is the list of open drawings
        // the MCP server reads after every call to notice drawings opened and closed.
        if outer && !id.starts_with("events.") && id != "document.list" {
            self.event("command.run", serde_json::json!({ "command": id, "ok": r.is_ok() }));
        }
        // Strip control characters so every JSON serialisation of the result is accepted by
        // a strict parser (Q6011).
        r.map(json_clean::sanitized)
    }

    /// What the running command (or the one a transparent command interrupted) changed so far
    /// becomes an undo step, and it goes on from here. The drawing's uid, for [`Self::rebase_running`].
    fn split_running_undo(&mut self) -> Option<u64> {
        let active = self.active;
        let owner = self.suspended.as_mut().or(self.running.as_mut())?;
        let st = self.docs.get_mut(active)?;
        let undoable = find_command(&owner.id).is_some_and(|c| c.undoable);
        if undoable && !Arc::ptr_eq(&owner.before, &st.doc) {
            let label = find_command(&owner.id).map_or("Command", |c| c.label);
            st.push_undo(Snapshot {
                label: label.to_string(),
                doc: Arc::clone(&owner.before),
                selection: owner.selection_before.clone(),
                bytes: 0,
                look: None,
            });
            st.revision += 1;
        }
        owner.before = Arc::clone(&st.doc);
        Some(st.uid)
    }

    /// [`Self::split_running_undo`] for a call that records no step of its own, decided after it
    /// ran: `pre` is the drawing's uid, document and selection from before the call. A call that
    /// left the drawing as it was splits nothing; one that changed it (the command's work so far
    /// and the call's are steps of their own, as when the split comes first) does.
    fn split_running_undo_after(&mut self, id: &str, pre: (u64, Arc<Drawing>, Vec<Handle>)) {
        let (uid, doc, selection) = pre;
        let Some(idx) = self.docs.iter().position(|d| d.uid == uid) else { return };
        if self.docs.get(idx).is_none_or(|st| Arc::ptr_eq(&st.doc, &doc)) {
            return;
        }
        let Some(owner) = self.suspended.as_ref().or(self.running.as_ref()) else { return };
        let owner_spec = find_command(&owner.id);
        let (label, before, sel_before) = (owner_spec.map_or("Command", |c| c.label), Arc::clone(&owner.before), owner.selection_before.clone());
        let undoable = owner_spec.is_some_and(|c| c.undoable);
        let call_label = find_command(&cmd::resolve_alias(id)).map_or("Command", |c| c.label);
        let Some(st) = self.docs.get_mut(idx) else { return };
        if undoable && !Arc::ptr_eq(&before, &doc) {
            st.push_undo(Snapshot { label: label.to_string(), doc: Arc::clone(&before), selection: sel_before, bytes: 0, look: None });
        }
        st.push_undo(Snapshot { label: call_label.to_string(), doc, selection, bytes: 0, look: None });
        st.revision += 1;
        self.rebase_running(uid);
    }

    /// After a call from outside: the running command's step starts from the drawing the call
    /// left, so it holds only the command's own work.
    fn rebase_running(&mut self, uid: u64) {
        let Some(doc) = self.docs.iter().find(|d| d.uid == uid).map(|d| Arc::clone(&d.doc)) else { return };
        for run in [self.running.as_mut(), self.suspended.as_mut()].into_iter().flatten() {
            run.before = Arc::clone(&doc);
        }
    }

    /// A session as an app starts one (the desktop app, MCP, the CLI): [`Session::empty`] with
    /// the plug-ins installed in the plug-ins folder loaded.
    pub fn for_app() -> Session {
        let mut s = Session::empty();
        s.load_installed_plugins();
        s
    }

    /// Load the plug-ins installed in the plug-ins folder; what fails to load is said in the log.
    pub fn load_installed_plugins(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut log = Vec::new();
            self.plugins.load_all(&mut log);
            self.log.extend(log);
        }
    }

    /// Run a plug-in command with undo support: the drawing before and after are compared, and an
    /// undo step is created if the plug-in changed anything.
    #[cfg(not(target_arch = "wasm32"))]
    fn execute_plugin(&mut self, id: &str, params: &Value) -> Result<Value> {
        // Extract what we need from the plugin host before the mutable borrow of self.
        let (engine, module, _plugin_idx) = self.plugins.command_info(id)?;
        let owner = self.plugins.owner(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        // A plug-in calling itself (or another) through host calls: refused past the depth.
        if self.plugin_depth >= plugins::MAX_PLUGIN_DEPTH {
            return Err(EngineError::Other(format!(
                "plug-in commands nest more than {} deep ({id} started from a plug-in's host call): refused",
                plugins::MAX_PLUGIN_DEPTH
            )));
        }
        let before = self.state().ok().map(|s| (s.doc.clone(), s.selection.clone(), s.uid));
        self.plugin_depth += 1;
        let result = plugins::execute_plugin_command(&engine, &module, id, params, self, &owner);
        self.plugin_depth -= 1;
        // Create an undo step if the plug-in changed the drawing.
        let changed = match (&before, self.state().ok()) {
            (Some((doc_before, _, uid_before)), Some(st)) if st.uid == *uid_before => !Arc::ptr_eq(&st.doc, doc_before),
            _ => false,
        };
        if changed {
            self.after_command(before.as_ref().map(|b| &b.0), before.as_ref().map(|b| b.2), result.is_ok());
        }
        result
    }

    fn execute_inner(&mut self, id: &str, params: &Value) -> Result<Value> {
        // Try plug-in commands before reporting "unknown command".
        #[cfg(not(target_arch = "wasm32"))]
        if find_command(id).is_none() && self.plugins.find_command(id).is_some() {
            return self.execute_plugin(id, params);
        }
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        (spec.enabled)(self).map_err(|m| EngineError::Disabled(spec.id.into(), m))?;
        if let Some(why) = cmd::blocks::bedit::refused_in_editor(self, spec.id) {
            return Err(EngineError::Disabled(spec.id.into(), why));
        }
        // A model the plan's edits left behind is built before a command that may read it.
        if !cmd::wallgen::runs_with_model_behind(spec.id) {
            cmd::wallgen::update_model(self);
        }
        let before = self.state().ok().map(|s| (s.doc.clone(), s.selection.clone(), s.uid));
        let look_before = self.state().ok().map(DocState::model_look);
        let editing = self.state().is_ok_and(|s| s.block_edit.is_some());
        let run = spec.run;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, params)));
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                let msg =
                    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
                crash_report::record(self, id, &msg);
                // Restore the drawing as it was.
                if let Some((doc, sel, uid)) = &before
                    && let Some(st) = self.docs.iter_mut().find(|d| d.uid == *uid)
                {
                    st.doc = doc.clone();
                    st.selection = sel.clone();
                }
                return Err(EngineError::Internal(id.into(), msg));
            }
        };
        let violation = before.as_ref().and_then(|b| cmd::blocks::refedit::after_command(self, &b.0, spec.id));
        let result = match (violation, result) {
            (Some(m), Ok(_)) => Err(EngineError::Other(m)),
            (_, r) => r,
        };
        // Opening or closing the Block Editor swaps the whole drawing: nothing in it changed. Undo
        // and redo restore a drawing as it was, already consistent: the follow-ups (associative
        // dimensions, groups, tables…) would take the restored objects for moved ones and "fix"
        // them (redo dropped an associative dimension's links: undofuzz).
        if self.state().is_ok_and(|s| s.block_edit.is_some()) == editing && !moves_through_history(spec.id) {
            self.after_command(before.as_ref().map(|b| &b.0), before.as_ref().map(|b| b.2), spec.undoable && result.is_ok());
        } else {
            cmd::wallgen::touch(self);
        }
        let result = match (result, &before) {
            (Ok(v), Some((doc, sel, uid))) if spec.undoable => self.refuse_non_finite(Some(*uid), doc, sel, spec.id).map(|()| v),
            (r, _) => r,
        };
        // A refused command that took the drawing for writing (copy on write) without changing it
        // gets the very same drawing back, so it leaves no undo step: an empty step made the
        // next U do nothing (layer.delete of layer 0, layer.new of a name in use: undofuzz).
        if result.is_err()
            && let Some((doc, _, uid)) = &before
            && let Some(st) = self.docs.iter_mut().find(|d| d.uid == *uid)
            && !Arc::ptr_eq(doc, &st.doc)
            && **doc == *st.doc
        {
            st.doc = Arc::clone(doc);
        }
        let in_lisp = self.lisp_groups_undo() && self.one_undo_step == 0;
        let grouped = self.lisp_groups_undo() || self.one_undo_step > 0;
        let mut in_routine = None;
        if spec.undoable
            && let Some((doc, sel, uid)) = before
            && let Some(st) = self.docs.iter_mut().find(|d| d.uid == uid)
            && !Arc::ptr_eq(&doc, &st.doc)
        {
            let look = look_before.filter(|l| !l.same_3d(&st.model_look()));
            let snap = Snapshot { label: spec.label.to_string(), doc, selection: sel, bytes: 0, look };
            // Inside a LISP routine the routine's single undo step records it, and the routine
            // keeps it for U inside it.
            if !grouped {
                st.push_undo(snap);
            } else if in_lisp {
                in_routine = Some(snap);
            }
            st.revision += 1;
        }
        if let Some(snap) = in_routine {
            self.lisp_record_step(snap);
        }
        result.map(|v| with_unknown_param_warnings(v, spec, params))
    }

    /// Run `f` as one undo step: the undoable commands it runs record none of their own, so the
    /// command calling it (undoable itself) is a single step (BIM.BUILD drafting the spec,
    /// reading the MEP and building the model is one U).
    pub(crate) fn as_one_undo_step<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.one_undo_step = self.one_undo_step.saturating_add(1);
        let r = f(self);
        self.one_undo_step = self.one_undo_step.saturating_sub(1);
        r
    }

    /// Bump the revision without an undo step (view-only or settings changes that redraw).
    pub fn touch(&mut self) {
        if let Ok(st) = self.state_mut() {
            st.revision += 1;
        }
    }

    // ---------------- interactive commands ----------------

    /// Start a command by name or alias as if typed at the command line.
    pub fn start(&mut self, name: &str) -> Result<()> {
        self.pick_hint = None;
        // A command an agent types has the rails of its `execute` calls (dry run, confirmations).
        if self.journal.is_outer()
            && self.agent.from_agent
            && let Some(r) = self.guard_typed(name)
        {
            return r;
        }
        let outer = self.journal.enter();
        if outer {
            self.note_selection_gap();
            self.journal.started(name);
        }
        let r = self.start_inner(name);
        self.journal.leave();
        if outer {
            // Written now the commands it cancelled have ended; kept when it is running, or done
            // at once, and left out when refused.
            self.journal.start_over(r.is_ok(), self.open_commands());
            self.journal_selection();
        }
        if outer && r.is_ok() {
            let id = self.running.as_ref().map_or_else(|| name.trim().to_ascii_lowercase(), |run| run.id.clone());
            self.event("command.started", serde_json::json!({ "command": id, "running": self.running.is_some() }));
        }
        r
    }

    /// Commands running, as the journal counts them: the running one and the one it interrupted.
    fn open_commands(&self) -> usize {
        usize::from(self.running.is_some()) + usize::from(self.suspended.is_some())
    }

    /// The selection in a form the journal compares: how many objects and the sum of their handles.
    fn selection_print(&self) -> (usize, u64) {
        self.state().map(|st| (st.selection.len(), st.selection.iter().fold(0u64, |a, h| a.wrapping_add(h.0)))).unwrap_or_default()
    }

    /// Before a command that may take the selection: objects selected since the last journaled
    /// call (a click, a window) are a gap in the script, as picks are.
    fn note_selection_gap(&mut self) {
        let now = self.selection_print();
        if self.journal.selection_differs(now) {
            self.journal.gap("objects were selected outside a command");
        }
    }

    /// The selection a journaled call left: what the script, replayed, leaves too.
    fn journal_selection(&mut self) {
        let now = self.selection_print();
        self.journal.selection_left(now);
    }

    /// A command running was cancelled: its lines stay, ended by a cancel, when it changed the
    /// drawing (Esc keeps what the command drew), else they go.
    fn journal_cancel(&mut self, before: &Arc<cadaddy_doc::Drawing>) {
        if self.journal.at_top() {
            let changed = self.state().is_ok_and(|st| !Arc::ptr_eq(before, &st.doc));
            self.journal.cancelled(changed);
        }
    }

    fn start_inner(&mut self, name: &str) -> Result<()> {
        self.last_polar = None;
        let name = name.trim();
        let (transparent, name) = match name.strip_prefix('\'') {
            Some(n) => (true, n),
            None => (false, name),
        };
        let lower = name.trim_start_matches(['_', '.', '-']).to_ascii_lowercase();
        // A command-line form with an id of its own (-ATTEDIT) is not the command it prefixes.
        let dashed = cmd::resolve_alias(&name.trim_start_matches(['_', '.']).to_ascii_lowercase());
        let id = if find_command(&dashed).is_some() { dashed } else { cmd::resolve_alias(&lower) };
        let Some(spec) = find_command(&id) else {
            // A command defined in LISP: (defun c:NAME …).
            if !transparent && let Some(f) = self.lisp_command(&lower) {
                if self.running.is_some() {
                    self.cancel();
                }
                self.last_command = Some(lower.clone());
                self.echo(format!("Command: {}", lower.to_ascii_uppercase()));
                return self.lisp_run_command(&lower, f);
            }
            // An action macro's name plays it.
            if !transparent && let Some(r) = cmd::actions::start_by_name(self, &lower) {
                return r;
            }
            // A bare system variable name (ORTHOMODE, GRIDMODE…) acts as SETVAR on that variable.
            let var = lower.to_ascii_uppercase();
            if !name.starts_with(['_', '.', '-']) && sysvars::get(self, &var).is_some() {
                self.start(if transparent { "'setvar" } else { "setvar" })?;
                return self.input(Input::Text(var));
            }
            return Err(EngineError::UnknownCommand(clip_input(name)));
        };
        if transparent && spec.transparent && self.running.is_some() {
            // Transparent commands (zoom, pan…) run without cancelling the active one.
            if spec.interactive.is_none() {
                self.echo(format!(">>{}", spec.label));
                return self.execute(spec.id, &Value::Null).map(|_| ());
            }
            if self.suspended.is_some() {
                self.echo("A transparent command is already in progress.");
                return Ok(());
            }
        }
        let nested = transparent && spec.transparent && self.running.is_some() && spec.interactive.is_some();
        // Ctrl+Z / U / UNDO while a command's prompt offers its own Undo keyword (LINE, PLINE…)
        // takes back the last segment and keeps the command running, as the keyword does.
        if !nested && matches!(spec.id, "u" | "undo") && self.current_prompt().is_some_and(|p| p.keywords.iter().any(|k| k == "Undo")) {
            self.echo(format!("Command: {}", spec.id.to_ascii_uppercase()));
            return self.input(Input::Keyword("Undo".into()));
        }
        if !nested && self.running.is_some() {
            self.cancel();
        }
        (spec.enabled)(self).map_err(|m| EngineError::Disabled(spec.id.into(), m))?;
        if let Some(why) = cmd::blocks::bedit::refused_in_editor(self, spec.id) {
            return Err(EngineError::Disabled(spec.id.into(), why));
        }
        // A model the plan's edits left behind is built before a typed command that may read it.
        if !nested && !cmd::wallgen::runs_with_model_behind(spec.id) {
            cmd::wallgen::update_model(self);
        }
        if nested {
            // The outer command stays the one Enter repeats.
            self.echo(format!(">>{}", spec.id.to_ascii_uppercase()));
        } else {
            self.last_command = Some(spec.id.to_string());
            self.echo(format!("Command: {}", spec.id.to_ascii_uppercase()));
        }
        if !nested {
            self.lisp_command_event(lisp::reactor::CommandEvent::WillStart, spec.id);
        }
        match spec.interactive {
            Some(factory) => {
                let machine = guard::Guarded::create(spec.id, factory, self)?;
                let st = self.state()?;
                let before = st.doc.clone();
                let selection_before = st.selection.clone();
                if nested {
                    self.suspended = self.running.take();
                }
                self.running = Some(Running { id: spec.id.to_string(), machine, before, selection_before });
                self.pending_window = None;
                // Some commands complete immediately (e.g. ERASE with a pickfirst selection).
                self.feed(None)?;
                Ok(())
            }
            None => {
                // Typed with nothing after it, a command with no prompts of its own asks for what
                // it needs (DELAY: "Enter delay time (in milliseconds):").
                if let Some(machine) = cmd::typed_ask::machine_for(spec.id) {
                    let st = self.state()?;
                    let (before, selection_before) = (st.doc.clone(), st.selection.clone());
                    self.running = Some(Running { id: spec.id.to_string(), machine: guard::Guarded::wrap(machine), before, selection_before });
                    self.pending_window = None;
                    self.feed(None)?;
                    return Ok(());
                }
                let r = self.execute(spec.id, &Value::Null);
                let event = if r.is_ok() { lisp::reactor::CommandEvent::Ended } else { lisp::reactor::CommandEvent::Failed };
                self.lisp_command_event(event, spec.id);
                let r = r?;
                if let Some(s) = r.as_str() {
                    self.echo(s.to_string());
                } else if !r.is_null()
                    && let Some(msg) = r.get("message").and_then(Value::as_str)
                {
                    for l in msg.lines() {
                        self.echo(l.to_string());
                    }
                }
                Ok(())
            }
        }
    }

    /// Feed an object-snap pick, retaining its viewport origin for measurements. Point modifiers
    /// that combine or constrain coordinates produce ordinary drawing-space points.
    pub fn input_snapped(&mut self, hit: snap::SnapHit) -> Result<()> {
        let source = hit.viewport.filter(|_| self.point_mods.iter().all(|f| matches!(f, pointmod::Frame::Snap { mode } if *mode != 0)));
        let previous = std::mem::replace(&mut self.picked_viewport, source);
        let result = self.input(hit.input());
        self.picked_viewport = previous;
        result
    }

    /// Give an input to the running command. With no command running, `Enter` repeats the last
    /// command (as in AutoCAD) and points do nothing. A LISP routine waiting in a `get…`
    /// function takes the input instead; one paused in `command` carries on after it.
    /// The 3D object snap under the cursor (`cursor`: the view-plane point in a 3D view, the plan
    /// point in the plan view) while the prompt takes a point: 3DOSMODE's snaps, and with dynamic
    /// UCS in a 3D view at a prompt taking points in space, the face under the cursor.
    pub fn osnap3_at(&self, cursor: Vec2) -> Option<snap3::Snap3> {
        let prompt = self.current_prompt()?;
        if !prompt.accept.point {
            return None;
        }
        let st = self.state().ok()?;
        let proj = st.projection();
        let ap = self.pixel_size() * self.settings.aperture.max(1.0);
        let base = prompt.base.map(|_| self.last_point3);
        let on_face = self.settings.ducs && prompt.point3 && proj.is_some();
        snap3::osnap3(&st.doc, &st.edit_space(), proj, cursor, ap, self.settings.osmode3, base, on_face)
    }

    pub fn input(&mut self, input: Input) -> Result<()> {
        let outer = self.journal.enter();
        let running = self.running.is_some();
        if outer && running {
            self.journal.input(&input);
        }
        let r = self.input_inner(input.clone());
        self.journal.leave();
        if outer
            && !running
            && let Some(id) = self.running.as_ref().map(|r| r.id.to_string())
        {
            // Enter at `Command:` repeated the last command.
            self.journal.started(&id);
            self.journal.start_over(true, self.open_commands());
        } else if outer && running {
            // Commands that finished keep their lines (a cancel was written by `feed`).
            self.journal.settle(self.open_commands());
        }
        if outer {
            self.journal_selection();
        }
        r
    }

    fn input_inner(&mut self, input: Input) -> Result<()> {
        if self.lisp_input(input.clone()) {
            return Ok(());
        }
        let r = self.input_to_command(input);
        self.lisp_after_command_input();
        r
    }

    fn input_to_command(&mut self, input: Input) -> Result<()> {
        if self.running.is_none() {
            if input == Input::Enter
                && let Some(last) = self.last_command.clone()
            {
                return self.start(&last);
            }
            return Ok(());
        }
        if !self.point_mods.is_empty() {
            return self.modifier_input(input);
        }
        // A deferred snap is only meaningful where the prompt resolves it; elsewhere it is the
        // point it was picked at.
        let input = match input {
            Input::Deferred(d) if !self.current_prompt().is_some_and(|p| p.deferred) => Input::Point(d.at),
            // A point in space where the prompt takes a plan point: its X and Y.
            Input::Point3(p) if !self.current_prompt().is_some_and(|q| q.point3) => Input::Point(p.xy()),
            Input::Point3(p) => {
                self.last_point3 = p;
                self.last_point = p.xy();
                Input::Point3(p)
            }
            other => other,
        };
        // Generic selection handling during "Select objects:" prompts.
        let input = self.preprocess_selection(input)?;
        match input {
            Some(i) => {
                // SIngle: the first selection that finds objects also ends the prompt.
                let single = self.select_single && matches!(&i, Input::Pick(hs) if !hs.is_empty());
                self.feed(Some(i))?;
                if single {
                    self.select_single = false;
                    if self.current_prompt().is_some_and(|p| p.accept.select) {
                        self.feed(Some(Input::Enter))?;
                    }
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn preprocess_selection(&mut self, input: Input) -> Result<Option<Input>> {
        // The command's own prompt: a typed selection mode only applies at its "Select objects".
        let Some(prompt) = self.running.as_ref().map(|r| r.machine.prompt(self)) else { return Ok(Some(input)) };
        if !prompt.accept.select {
            return Ok(Some(input));
        }
        if self.select_mode.is_some() {
            return self.select_mode_input(input);
        }
        // `_L`, `_ALL`, `_W`: the English name of a selection keyword, as scripts write it.
        let input = match input {
            Input::Text(t) => Input::Text(t.trim().strip_prefix('_').map(str::to_string).unwrap_or(t)),
            other => other,
        };
        if let Input::Text(t) | Input::Keyword(t) = &input
            && let Some(m) = select::SelectMode::from_keyword(t)
        {
            self.pending_window = None;
            self.select_mode = Some(m);
            return Ok(None);
        }
        if let Input::Text(t) | Input::Keyword(t) = &input
            && !prompt.keywords.iter().any(|k| k.eq_ignore_ascii_case(t.trim()))
        {
            match t.trim().to_ascii_lowercase().as_str() {
                "si" | "single" => {
                    self.select_single = true;
                    return Ok(None);
                }
                // AUto (a pick, or a window where nothing is picked) is how points always select;
                // Multiple picks the same way until Enter.
                "au" | "auto" | "m" | "multiple" => return Ok(None),
                _ => {}
            }
        }
        let space = self.space();
        match input {
            Input::Point(p) => {
                let ap = self.pixel_size() * self.settings.pickbox.max(1.0) * 1.5;
                if let Some(pw) = self.pending_window.take() {
                    let hs = self.window_selection(&space, pw.corner, p)?;
                    return Ok(Some(Input::Pick(cmd::groups::expand(self, hs))));
                }
                match select::pick(self.doc()?, &space, p, ap) {
                    Some(h) => Ok(Some(Input::Pick(cmd::groups::expand(self, vec![h])))),
                    None => {
                        self.pending_window = Some(PendingWindow { corner: p, during_command: true });
                        Ok(None)
                    }
                }
            }
            Input::Text(ref t) | Input::Keyword(ref t) => {
                let tl = t.trim().to_ascii_lowercase();
                let d = self.doc()?;
                let store = d.space(&space);
                let picked = match tl.as_str() {
                    "all" => store.map(|s| s.iter().filter(|e| d.is_visible(e)).map(|e| e.handle).collect()),
                    "l" | "last" => store.and_then(|s| s.last()).map(|e| vec![e.handle]),
                    "p" | "previous" => Some(self.state()?.previous_selection.clone()),
                    _ => None,
                };
                match picked {
                    Some(hs) => Ok(Some(Input::Pick(cmd::groups::expand(self, hs)))),
                    None => Ok(Some(input)),
                }
            }
            Input::Cancel => {
                self.pending_window = None;
                Ok(Some(Input::Cancel))
            }
            other => Ok(Some(other)),
        }
    }

    /// Input while a typed selection mode (W, C, F, WP, CP) collects its points: the selected
    /// objects go to the command as a pick once the mode is complete.
    fn select_mode_input(&mut self, input: Input) -> Result<Option<Input>> {
        let Some(mut mode) = self.select_mode.take() else { return Ok(Some(input)) };
        if mode.kind == select::SelectKind::Class {
            return match input {
                Input::Text(t) | Input::Keyword(t) if t.trim() == "?" => {
                    let names = select::classes_in(self.doc()?, &self.space());
                    self.echo(if names.is_empty() { "No objects.".to_string() } else { names.join(", ") });
                    self.select_mode = Some(mode);
                    Ok(None)
                }
                Input::Text(name) | Input::Keyword(name) => {
                    let hs = select::select_class(self.doc()?, &self.space(), &name);
                    if hs.is_empty() {
                        self.echo(format!("No objects of class \"{}\".", name.trim()));
                        return Ok(None);
                    }
                    Ok(Some(Input::Pick(cmd::groups::expand(self, hs))))
                }
                Input::Cancel => Ok(Some(input)),
                Input::Enter => Ok(None),
                _ => {
                    self.echo("Invalid class name.");
                    self.select_mode = Some(mode);
                    Ok(None)
                }
            };
        }
        if mode.kind == select::SelectKind::Group {
            return match input {
                Input::Text(name) | Input::Keyword(name) => match select::select_group(self.doc()?, &self.space(), &name) {
                    Some(hs) => Ok(Some(Input::Pick(hs))),
                    None => {
                        self.echo(format!("No group named \"{}\".", name.trim()));
                        Ok(None)
                    }
                },
                // Enter or Esc leaves the group name prompt.
                Input::Cancel => Ok(Some(input)),
                Input::Enter => Ok(None),
                _ => {
                    self.echo("Invalid group name.");
                    self.select_mode = Some(mode);
                    Ok(None)
                }
            };
        }
        match input {
            Input::Point(p) => {
                if mode.push(p) {
                    return Ok(mode.select(self.doc()?, &self.space()).map(Input::Pick));
                }
            }
            Input::Keyword(k) if k == "Undo" => {
                mode.pts.pop();
            }
            // Enter ends a fence or polygon; with too few points the mode is dropped.
            Input::Enter => return Ok(mode.select(self.doc()?, &self.space()).map(Input::Pick)),
            Input::Cancel | Input::Pick(_) => return Ok(Some(input)),
            _ => self.echo("Invalid point."),
        }
        self.select_mode = Some(mode);
        Ok(None)
    }

    fn feed(&mut self, input: Option<Input>) -> Result<()> {
        let base = if matches!(input, Some(Input::Point(_))) { self.current_prompt().and_then(|p| p.base) } else { None };
        let enter = matches!(input, Some(Input::Enter));
        let beginning = input.is_none();
        let too_large = matches!(&input, Some(Input::Text(t)) if beyond_limit(t));
        let Some(mut run) = self.running.take() else { return Ok(()) };
        let step = match input {
            Some(Input::Cancel) => Ok(Step::Cancel),
            Some(i) => {
                if let Input::Point(p) = &i {
                    self.last_point = *p;
                    if let Some(b) = base.filter(|b| b.is_finite() && p.is_finite() && b.dist(*p) > 1e-12) {
                        self.last_segment = Some((b, *p));
                    }
                    // A temporary tracking point (TT) serves one point.
                    self.tracking.release_temporary();
                }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.machine.input(self, i)));
                match r {
                    Ok(r) => r,
                    Err(p) => {
                        let msg = guard::message(&*p);
                        crash_report::record(self, &run.id, &msg);
                        Err(EngineError::Internal(run.id.clone(), msg))
                    }
                }
            }
            // A command starts with no modifiers pending.
            None => {
                self.point_mods.clear();
                run.machine.begin(self)
            }
        };
        // Redraw what the command has added so far (LINE adds a segment per point), not only when it ends.
        if let Ok(st) = self.state_mut()
            && !Arc::ptr_eq(&run.before, &st.doc)
        {
            st.revision += 1;
        }
        match step {
            Ok(Step::Continue) => {
                self.running = Some(run);
                Ok(())
            }
            Ok(Step::Done) => {
                let id = run.id.clone();
                self.finish(run, false);
                self.resume_suspended();
                self.repeat_multiple(&id, beginning);
                Ok(())
            }
            Ok(Step::Cancel) => {
                self.multiple = None;
                self.echo("*Cancel*");
                self.journal_cancel(&run.before);
                self.finish(run, true);
                self.resume_suspended();
                Ok(())
            }
            Err(EngineError::Internal(id, m)) => {
                self.multiple = None;
                // Restore and end the command.
                if let Ok(st) = self.state_mut() {
                    st.doc = run.before.clone();
                }
                self.echo(format!("Internal error in {id}: {m}"));
                self.resume_suspended();
                Err(EngineError::Internal(id, m))
            }
            Err(e) if enter => {
                // Enter the command can't take ends it, keeping what it did so far: asking again
                // left Esc as the only way out, and what was typed next went to the command
                // (battle test 0.7.8 #1, the stuck prompts sweep).
                self.echo(e.user_message());
                let id = run.id.clone();
                self.finish(run, false);
                self.resume_suspended();
                self.repeat_multiple(&id, beginning);
                Ok(())
            }
            Err(e) => {
                // Invalid input: report and keep prompting (AutoCAD re-prompts). A number past
                // the limit is named as that, not as "requires a distance" (battle test 5 B7-021).
                self.echo(if too_large { TOO_LARGE.to_string() } else { e.user_message() });
                self.running = Some(run);
                Ok(())
            }
        }
    }

    /// MULTIPLE: once the command it repeats (or MULTIPLE itself) has ended, start it again. A
    /// command that ended as it began (nothing to ask) stops the repeating instead of spinning.
    fn repeat_multiple(&mut self, finished: &str, beginning: bool) {
        let Some(target) = self.multiple.clone() else { return };
        if self.running.is_some() || self.suspended.is_some() || (finished != target && finished != "multiple") {
            return;
        }
        if finished == target && beginning {
            self.multiple = None;
            return;
        }
        if let Err(e) = self.start(&target) {
            self.multiple = None;
            self.echo(e.user_message());
        }
    }

    /// End a command: every way out of one comes through here. The passes after a command
    /// (constraints, associativity, groups) can panic like the command itself; a panic restores
    /// the drawing as it was before the command, as a panic in its `input` does.
    fn finish(&mut self, run: Running, cancelled: bool) {
        let (id, before, selection) = (run.id.clone(), run.before.clone(), run.selection_before.clone());
        if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.finish_guarded(run, cancelled))) {
            let msg = crate::guard::message(&*p);
            crash_report::record(self, &id, &msg);
            if let Ok(st) = self.state_mut() {
                st.doc = before;
                st.selection = selection;
            }
            self.echo(format!("Internal error in {id}: {msg}"));
        }
    }

    fn finish_guarded(&mut self, run: Running, cancelled: bool) {
        self.event("command.ended", serde_json::json!({ "command": run.id, "cancelled": cancelled }));
        let id = run.id.clone();
        let rhai_before = (self.rhai_watching() && !cmd::rhai_cmd::is_rhai(&id))
            .then(|| (self.state().map(|st| st.uid).unwrap_or_default(), Arc::clone(&run.before)));
        self.finish_steps(run, cancelled);
        if rhai_before.is_some() {
            self.rhai_command_ended(&id, rhai_before, !cancelled);
            self.rhai_dispatch_pending();
        }
        // Reactors react once the command's undo step is in place.
        let event = if cancelled { lisp::reactor::CommandEvent::Cancelled } else { lisp::reactor::CommandEvent::Ended };
        self.lisp_command_event(event, &id);
    }

    fn finish_steps(&mut self, run: Running, cancelled: bool) {
        // A typed selection mode or SIngle never outlives its command.
        self.select_mode = None;
        self.select_single = false;
        if find_command(&run.id).is_some_and(|c| c.undoable)
            && let Err(e) = self.refuse_non_finite(None, &run.before, &run.selection_before, &run.id)
        {
            self.echo(e.user_message());
        }
        // A command outside undo that changed the drawing (UNDO, which swapped it for an earlier
        // one) records no undo step, as in `execute`.
        if find_command(&run.id).is_some_and(|c| !c.undoable) && self.state().is_ok_and(|st| !Arc::ptr_eq(&run.before, &st.doc)) {
            self.pending_window = None;
            return;
        }
        self.pending_window = None;
        self.select_mode = None;
        let label = find_command(&run.id).map(|c| c.label).unwrap_or("Command");
        // A cancelled command leaves the selection as it found it: objects picked at its "Select
        // objects" must not stay selected for the next command (ERASE would take them unasked).
        if cancelled {
            self.set_selection(run.selection_before.clone());
        }
        if let Some(m) = cmd::blocks::refedit::after_command(self, &run.before, &run.id) {
            self.echo(m);
        }
        if !moves_through_history(&run.id) {
            self.after_command(Some(&run.before), None, true);
        }
        // A transparent command's changes belong to the command it ran inside, and a command a
        // LISP routine ran belongs to the routine's single undo step.
        if self.suspended.is_some() || self.lisp_groups_undo() {
            let changed = if let Ok(st) = self.state_mut()
                && !Arc::ptr_eq(&run.before, &st.doc)
            {
                st.revision += 1;
                true
            } else {
                false
            };
            // A command a LISP routine ran: the routine keeps it for U inside it.
            if changed && self.suspended.is_none() {
                self.lisp_record_step(Snapshot { label: label.to_string(), doc: run.before, selection: run.selection_before, bytes: 0, look: None });
            }
            return;
        }
        // A command that changed nothing but asked for its step ([`Session::keep_undo_step`]) gets
        // one, as AutoCAD records every command: U then undoes it, not the command before.
        let keep = std::mem::take(&mut self.keep_undo_step) && !cancelled;
        if let Ok(st) = self.state_mut()
            && (keep || !Arc::ptr_eq(&run.before, &st.doc))
        {
            st.push_undo(Snapshot { label: label.to_string(), doc: run.before, selection: run.selection_before, bytes: 0, look: None });
            st.revision += 1;
        }
    }

    /// The running command changed nothing but is still an undo step of its own (EXPLODE of an
    /// object that can't be exploded: U after it used to undo the command before; battle test
    /// 0.7.7e).
    pub fn keep_undo_step(&mut self) {
        self.keep_undo_step = true;
    }

    /// The post-command passes, in order, run after every command — by [`Session::execute`] and
    /// when an interactive command finishes — so their edits land in the command's own undo step:
    /// group membership is pruned, constraints are inferred, purged and re-solved, and
    /// associative dimensions and hatches follow their sources. A new pass is added here, once.
    ///
    /// `before` is the drawing the command started from; `uid` (when known) is the document it
    /// started in, so associativity is not applied across a document switch; `ok` is false for a
    /// failed or non-undoable programmatic command, which skips the constraint pass.
    fn after_command(&mut self, before: Option<&Arc<Drawing>>, uid: Option<u64>, ok: bool) {
        #[cfg(test)]
        crate::guard::tests::panic_if_armed();
        cmd::groups::after_command(self, before);
        cmd::constraints::after_command(self, before, ok);
        assoc::after_command(self, before, uid);
        cmd::clip_partners::after_command(self, before);
        cmd::table::after_command(self, before);
        cmd::annotate::dimbreak_after_command(self, before);
        fields::after_command(self, before);
        cmd::wallgen::after_command(self, before);
        // After the live rebuild: the model's count index follows every change to its pieces.
        cmd::mepatoms::after_command(self, before, uid);
        script_tools::monitor_after(self);
    }

    fn resume_suspended(&mut self) {
        if let Some(outer) = self.suspended.take() {
            self.echo(format!("Resuming {} command.", outer.id.to_ascii_uppercase()));
            self.running = Some(outer);
        }
    }

    /// Cancel the running command (Esc). With no command, clears the selection. Inside a
    /// transparent command, cancels it and the command it interrupted.
    pub fn cancel(&mut self) {
        self.point_mods.clear();
        // Esc stops a LISP routine waiting for input, and the command it paused in.
        if self.lisp_waiting() {
            if self.running.is_some() {
                let _ = self.feed(Some(Input::Cancel));
            }
            self.echo("*Cancel*");
            self.lisp_abort("Function cancelled");
            return;
        }
        if self.running.is_some() {
            if self.suspended.is_some() {
                if let Some(nested) = self.running.take() {
                    self.journal_cancel(&nested.before);
                    self.finish(nested, true);
                }
                self.running = self.suspended.take();
            }
            let _ = self.feed(Some(Input::Cancel));
        } else {
            self.pending_window = None;
            self.set_selection(Vec::new());
        }
        if self.journal.at_top() {
            self.journal.settle(self.open_commands());
            if self.journal.is_outer() {
                self.journal_selection();
            }
        }
    }

    /// The multiline text the running command is collecting, for the in-place editor
    /// ([`Interactive::editing_mtext`]).
    pub fn editing_mtext(&self) -> Option<cadaddy_doc::MText> {
        self.running.as_ref().and_then(|r| r.machine.editing_mtext())
    }

    pub fn current_prompt(&self) -> Option<Prompt> {
        if let Some(p) = self.lisp_prompt() {
            return Some(p);
        }
        self.running.as_ref().map(|r| {
            let p = r.machine.prompt(self);
            match &self.select_mode {
                Some(m) if p.accept.select => m.prompt(),
                _ => p,
            }
        })
    }

    /// Whether the cursor picks objects right now, so the UI shows the pick box (PICKBOX) at the
    /// crosshair: at prompts that select objects, and with no command running when PICKFIRST is on
    /// (noun-verb selection).
    pub fn picking_objects(&self) -> bool {
        match self.current_prompt() {
            Some(p) => p.picks_objects(),
            None => self.settings.pickfirst,
        }
    }

    /// Whether the object under the cursor is highlighted (SELECTIONPREVIEW): the cursor picks
    /// objects ([`Self::picking_objects`]) and the bit for a command asking for them (2) or for
    /// no command (1) is on.
    pub fn previews_selection(&self) -> bool {
        let bit = if self.current_prompt().is_some() { 2 } else { 1 };
        self.picking_objects() && self.settings.selectionpreview & bit != 0
    }

    /// Whether selected objects show grips while `selected` objects are selected (GRIPS,
    /// GRIPOBJLIMIT).
    pub fn shows_grips(&self, selected: usize) -> bool {
        let limit = self.settings.gripobjlimit;
        self.settings.grips > 0 && (limit == 0 || selected <= limit as usize)
    }

    /// The text shown on the command line: the active prompt, or "Command:".
    pub fn prompt_text(&self) -> String {
        if let Some(p) = self.lisp_prompt() {
            return p.display();
        }
        match &self.running {
            Some(r) if self.suspended.is_some() => format!(">>{} {}", r.id.to_ascii_uppercase(), r.machine.prompt(self).display()),
            Some(r) => format!("{} {}", r.id.to_ascii_uppercase(), r.machine.prompt(self).display()),
            None => "Command:".into(),
        }
    }

    /// Preview entities for the cursor: objects drawn with their own properties
    /// ([`Interactive::preview_objects`]), then the rubber band with the current properties, in
    /// the grip blue over such objects (so it shows over their fills, whatever their colour).
    pub fn preview(&self, cursor: Vec2) -> Vec<Entity> {
        let mut common = self.current_common();
        let (kinds, mut out) = match (self.lisp_preview(cursor), &self.running) {
            (Some(k), _) => (k, Vec::new()),
            (None, Some(r)) => (r.machine.preview(self, cursor), r.machine.preview_objects(self, cursor)),
            (None, None) => (Vec::new(), Vec::new()),
        };
        if !out.is_empty() {
            common.color = cadaddy_doc::color::Color::True(cadaddy_doc::color::Rgb(0x2f, 0x6b, 0xff));
        }
        out.extend(kinds.into_iter().map(|k| Entity { handle: Handle(0), common: common.clone(), kind: k }));
        out
    }

    /// The active command's live measurement labels for the cursor (MEASURELABELS; see
    /// [`Interactive::labels`]).
    pub fn live_labels(&self, cursor: Vec2) -> Vec<LiveLabel> {
        if self.settings.measurelabels == 0 || self.lisp_prompt().is_some() {
            return Vec::new();
        }
        self.running.as_ref().map(|r| r.machine.labels(self, cursor)).unwrap_or_default()
    }

    /// The active command's preview as a transform of existing objects (see
    /// [`Interactive::drag`]), with its extra rubber-band geometry as entities.
    pub fn drag(&self, cursor: Vec2) -> Option<(Drag, Vec<Entity>)> {
        let r = self.running.as_ref()?;
        let d = r.machine.drag(self, cursor)?;
        let common = self.current_common();
        let extra = d.extra.iter().map(|k| Entity { handle: Handle(0), common: common.clone(), kind: k.clone() }).collect();
        Some((d, extra))
    }

    /// Process one line typed at the command line (Enter pressed). Spaces separate inputs as in a
    /// script line (see [`Session::script`]), so `MOVE L  0,0 5,5` ends the selection with the
    /// second space and `ERASE L ` with the trailing one. Input after the command ends is ignored.
    pub fn cmdline(&mut self, text: &str) -> Result<()> {
        let t = text.trim_end_matches(['\r', '\n']);
        // The command line types spaces literally at free-text prompts: the line is one input.
        if self.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point && !p.accept.number) {
            return self.typed(t);
        }
        // At `Command:` leading blanks before a command name are not Enter (`" LINE"` from the
        // control channel starts LINE); a line of only blanks still is.
        let t = if self.running.is_none() && !t.trim().is_empty() { t.trim_start() } else { t };
        self.input_line(t, false)
    }

    /// One line of input: each space ends an input and an empty input (two spaces in a row, a
    /// trailing space, an empty line) is Enter, which at `Command:` repeats the last command. In a
    /// script, a free-text prompt (TEXT/MTEXT contents) takes the rest of the line, a prompt with
    /// keywords still splits so `I 15` reaches the keyword and the value, and the line goes on
    /// after its command ends; at the command line the rest after the command ends is ignored.
    fn input_line(&mut self, line: &str, script: bool) -> Result<()> {
        let mut rest = line;
        let mut first = true;
        loop {
            if self.running.is_none() {
                if !script && !first {
                    return Ok(());
                }
                // `cmd {json}`: programmatic call with parameters.
                if let Some((name, json)) = rest.trim().split_once(' ')
                    && json.trim_start().starts_with('{')
                {
                    let params: Value =
                        serde_json::from_str(json.trim()).map_err(|e| EngineError::BadParams { cmd: name.into(), msg: e.to_string() })?;
                    let id = cmd::resolve_alias(&name.to_ascii_lowercase());
                    let r = self.execute(&id, &params)?;
                    if !r.is_null() {
                        self.echo(r.to_string());
                    }
                    return Ok(());
                }
            } else if script
                && self.current_prompt().is_some_and(|p| {
                    p.accept.text
                        && !p.accept.point
                        && !p.accept.number
                        && (p.keywords.is_empty() || (p.whole_line && p.match_keyword(rest.trim()).is_none()))
                })
            {
                return self.typed(rest);
            }
            first = false;
            let (tok, more) = next_token(rest);
            if self.running.is_some() || self.lisp_prompt().is_some() {
                self.typed(tok)?;
            } else if tok.trim() == journal::CANCEL {
                // Esc with no command running clears the selection.
                self.cancel();
            } else if tok.trim().is_empty() {
                self.input(Input::Enter)?;
            } else if lisp_expression(tok) {
                // `(expression)` or `!variable` at Command: evaluates LISP and shows the value.
                let src = tok.trim().strip_prefix('!').unwrap_or(tok.trim()).to_string();
                if let Err(e) = self.lisp_eval(&src, true, false, Vec::new()) {
                    self.echo(e.user_message());
                }
            } else {
                self.start(tok)?;
            }
            match more {
                Some(more) => rest = more,
                None => return Ok(()),
            }
        }
    }

    /// One token typed while a command runs.
    fn typed(&mut self, t: &str) -> Result<()> {
        self.typed_token(t, true)
    }

    /// One token typed while a command runs; `lisp` evaluates a LISP expression first. The value
    /// of that expression is the answer, so it comes back with `lisp` off: re-evaluating it would
    /// let `(setq q "!q")` evaluate itself forever and overflow the stack (#1920).
    fn typed_token(&mut self, t: &str, lisp: bool) -> Result<()> {
        // The cancel a saved session script ends a command with ([`journal::CANCEL`]).
        if t.trim() == journal::CANCEL && self.current_prompt().is_some() {
            return self.input(Input::Cancel);
        }
        let Some(prompt) = self.current_prompt() else { return Ok(()) };
        let tt = t.trim();
        if tt.is_empty() {
            return self.input(Input::Enter);
        }
        // FROM, M2P, point filters, `<a` (before `'`: `'_from` is a modifier).
        if let Some(r) = self.typed_modifier(&prompt, tt) {
            return r;
        }
        // Transparent command.
        if tt.starts_with('\'') {
            return self.start(tt);
        }
        // A LISP expression typed at a command's prompt: its value is the answer.
        // Not at a free-text prompt: there the text is the answer (CAL takes "(3+4)*2").
        if lisp && lisp_expression(tt) && !prompt.accept.text && self.lisp_prompt().is_none() && !self.lisp_groups_undo() {
            let src = tt.strip_prefix('!').unwrap_or(tt).to_string();
            let v = match self.lisp_eval(&src, false, true, Vec::new()) {
                Ok(Some(Ok(v))) => v,
                Ok(_) => return Ok(()),
                Err(e) => {
                    self.echo(e.user_message());
                    return Ok(());
                }
            };
            return match v {
                lisp::value::Value::Nil => Ok(()),
                lisp::value::Value::Int(i) => self.typed_token(&i.to_string(), false),
                lisp::value::Value::Real(r) => self.typed_token(&format!("{r}"), false),
                lisp::value::Value::Str(s) => self.typed_token(&s, false),
                other => match other.as_point() {
                    Some(p) => self.input(Input::Point(Vec2::new(p[0], p[1]))),
                    None => {
                        self.echo(format!("Invalid input: {}", lisp::value::to_text(&other, true)));
                        Ok(())
                    }
                },
            };
        }
        // At "Select objects" prompts the selection modes (Previous, Last, ALL) win over a keyword
        // they only begin (HATCH's "picK internal point" against `P`); a keyword typed whole or
        // by its shortcut wins (COUNT's `L` is List, as in AutoCAD).
        let selection_mode = prompt.accept.select
            && matches!(tt.strip_prefix('_').unwrap_or(tt).to_ascii_lowercase().as_str(), "all" | "l" | "last" | "p" | "previous")
            && !prompt.exact_keyword(tt);
        // A leading underscore asks for a keyword by its English name, whatever the interface
        // language (`_M`, `_Yes`): scripts and LISP routines write them that way. (Snap
        // overrides such as `_cen` were taken above.)
        if !selection_mode
            && let Some(k) = prompt.match_keyword(tt).or_else(|| tt.strip_prefix('_').and_then(|r| prompt.match_keyword(r)))
            && !(prompt.accept.number && tt.parse::<f64>().is_ok())
        {
            return self.input(Input::Keyword(k));
        }
        // `x,y,z` (or `@dx,dy,dz`) where the prompt takes points in space.
        if prompt.point3
            && let Some(p) = prompt::parse_point3(tt, self.last_point3)
        {
            return self.input(Input::Point3(p));
        }
        if prompt.accept.point {
            if let Some(p) = prompt::parse_point_in(tt, self.last_point, &self.angle_settings(), &self.ucs()) {
                self.last_polar = prompt::parse_polar_text(tt, &self.angle_settings());
                return self.input(Input::Point(p));
            }
            // Direct distance entry along the tracking path the cursor is on, from the point it
            // comes from (an acquired or temporary tracking point needs no base point).
            if let Some(dist) = units::parse_distance(tt).filter(|_| !prompt.accept.number)
                && let Some((from, dir)) = self.tracking.last.as_ref().filter(|t| t.point.near(self.cursor, 1e-9)).and_then(|t| t.along())
            {
                return self.input(Input::Point(from + dir * dist));
            }
            // Direct distance entry along the rubber band.
            if let (Some(base), Some(dist)) = (prompt.base, units::parse_distance(tt))
                && !prompt.accept.number
            {
                let mut c = self.cursor;
                if self.settings.orthomode && !prompt.corner {
                    c = snap::iso::constrain(&self.drafting_settings(), base, c, self.ucs().angle);
                }
                let dir = (c - base).normalized();
                let dir = if dir == Vec2::ZERO { Vec2::X } else { dir };
                return self.input(Input::Point(base + dir * dist));
            }
        }
        // Text the prompt can't take that the command passed over without a word: say what the
        // prompt wants, as AutoCAD does (the stuck prompts sweep found hundreds of silent ones).
        let before = (self.log.len(), self.state().ok().map(|st| Arc::as_ptr(&st.doc)));
        let r = self.input(Input::Text(tt.to_string()));
        let unchanged = self.log.len() == before.0
            && self.state().ok().map(|st| Arc::as_ptr(&st.doc)) == before.1
            && self.current_prompt().is_some_and(|p| p.message == prompt.message && p.keywords == prompt.keywords);
        if r.is_ok() && !prompt.accept.text && self.running.is_some() && unchanged {
            self.echo(not_taken(&prompt));
        }
        r
    }

    /// Run a script, as AutoCAD reads one: a space or a line end ends an input, and an empty
    /// input (two spaces in a row, a space at the end of a line, an empty line) is Enter. At
    /// free-text prompts spaces are part of the text. Lines starting with `;` are comments.
    ///
    /// Script lines may control the script ([`script_tools`]): `SCRIPTCALL file` runs another
    /// script and comes back (at most [`script_tools::MAX_DEPTH`] deep), `RSCRIPT` starts this one
    /// again (at most [`script_tools::MAX_REPEATS`] times), `DELAY ms` waits. A line that fails
    /// stops the script; RESUME carries on from the next line. The SCRIPT command (or another
    /// `cmd {json}` call) inside a running one is refused, so two files that run each other
    /// cannot recurse until the stack dies.
    pub fn script(&mut self, text: &str) -> Result<()> {
        if self.in_script {
            return Err(EngineError::BadParams { cmd: "script".into(), msg: "a script is already running; nested scripts are not supported".into() });
        }
        self.in_script = true;
        self.script_last = Some(text.to_string());
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let r = self.script_lines(&lines, 0, 0);
        self.in_script = false;
        r
    }

    /// RESUME: the rest of the script a failed line stopped (or `false` when there is none).
    pub fn script_resume(&mut self) -> Result<bool> {
        let Some((lines, at, depth)) = self.script_interrupted.take() else { return Ok(false) };
        if self.in_script {
            return Ok(true);
        }
        self.in_script = true;
        let r = self.script_lines(&lines, at, depth);
        self.in_script = false;
        r.map(|()| true)
    }

    fn script_lines(&mut self, lines: &[String], start: usize, depth: usize) -> Result<()> {
        let mut at = start;
        let mut repeats = 0usize;
        while let Some(raw) = lines.get(at) {
            at += 1;
            // Trailing spaces are inputs (Enter); other trailing whitespace (`\r`) is not.
            let line = raw.trim_end_matches(|c: char| c.is_whitespace() && c != ' ');
            if line.trim_start().starts_with(';') {
                continue;
            }
            // The script's own controls (only with no command waiting for input).
            if self.running.is_none() {
                match script_tools::control(line) {
                    Some(script_tools::Control::Repeat) => {
                        repeats += 1;
                        if repeats > script_tools::MAX_REPEATS {
                            self.echo(format!("RSCRIPT: stopped after {} repeats.", script_tools::MAX_REPEATS));
                            return Ok(());
                        }
                        at = 0;
                        continue;
                    }
                    Some(script_tools::Control::Delay(ms)) => {
                        script_tools::delay(ms);
                        continue;
                    }
                    Some(script_tools::Control::Call(path)) => {
                        let text = script_tools::read_called(self, &path, depth)?;
                        let called: Vec<String> = text.lines().map(str::to_string).collect();
                        self.script_lines(&called, 0, depth + 1)?;
                        continue;
                    }
                    Some(script_tools::Control::Resume) => continue,
                    None => {}
                }
            }
            if let Err(e) = self.input_line(line, true) {
                // RESUME carries on from the next line.
                self.script_interrupted = Some((lines.to_vec(), at, depth));
                return Err(e);
            }
            // A script's next line waits for a LISP routine it started, as in AutoCAD (in the
            // app too: the script itself already holds the interface).
            self.lisp_finish(std::time::Duration::from_secs(30));
        }
        Ok(())
    }

    // ---------------- undo ----------------

    /// Take the last step back. Inside a LISP routine, the routine's last command first; past
    /// those, the drawing's step before the routine, which then starts from there.
    pub fn undo(&mut self) -> Result<Option<String>> {
        if let Some(label) = self.lisp_take_back(false) {
            return Ok(Some(label));
        }
        let st = self.state_mut()?;
        let Some(snap) = st.undo.pop() else { return Ok(None) };
        let cur = st.swap_in(snap);
        let label = cur.label.clone();
        st.redo.push(cur);
        self.lisp_rebase();
        Ok(Some(label))
    }
    /// Bring back what the last undo took back; inside a LISP routine, what U inside it took first.
    pub fn redo(&mut self) -> Result<Option<String>> {
        if let Some(label) = self.lisp_take_back(true) {
            return Ok(Some(label));
        }
        let st = self.state_mut()?;
        let Some(snap) = st.redo.pop() else { return Ok(None) };
        let cur = st.swap_in(snap);
        let label = cur.label.clone();
        st.undo.push(cur);
        self.lisp_rebase();
        Ok(Some(label))
    }

    // ---------------- picking without a command ----------------

    /// Open a selection window with one corner at `corner`, for press-and-drag selection
    /// (AutoCAD's PICKDRAG = 2: a drag opens the window wherever it starts, even over an object;
    /// the next point, where the drag ends, closes it). Only while objects are being selected: no
    /// command running, or a command asking for objects. Returns whether a window was opened.
    pub fn begin_window(&mut self, corner: Vec2) -> bool {
        let during_command = self.running.is_some();
        if !self.settings.pickdrag || !corner.is_finite() || (during_command && !self.current_prompt().is_some_and(|p| p.accept.select)) {
            return false;
        }
        self.pending_window = Some(PendingWindow { corner, during_command });
        true
    }

    /// Give the running command an object by name, as AutoLISP's `command` does with an entity
    /// name or an `entsel` list (name and point). A prompt picking one object with a point
    /// (BREAK's "Select object", MEASURE's "Select object to measure"…) takes the point, and a
    /// pick there finds that object ([`Session::pick_object`]) wherever it lies. A bare name
    /// stands at the object's DXF group 10 point (a line's start, a circle's centre, a
    /// polyline's first vertex), the point AutoCAD picks it at. The name holds for that input
    /// only: a later pick at the same point finds what lies there. Other prompts take the object
    /// as a pick.
    pub fn input_object(&mut self, h: Handle, at: Option<Vec2>) -> Result<()> {
        let one = self.current_prompt().is_some_and(|p| p.accept.point && !p.accept.select && p.picks_objects());
        let p = if one { at.or_else(|| self.doc().ok().and_then(|d| group_10(d, d.entity(h)?))) } else { None };
        let Some(p) = p.filter(|p| p.is_finite()) else { return self.input(Input::Pick(vec![h])) };
        self.pick_hint = Some((h, p));
        let r = self.input(Input::Point(p));
        self.pick_hint = None;
        r
    }

    /// The object a prompt picking one object finds at `p` in `space`: the one named for that
    /// point ([`Session::input_object`]), which is none when it lies in another space, else the
    /// object under `p` within `aperture`.
    pub fn pick_object(&self, space: &Space, p: Vec2, aperture: f64) -> Result<Option<Handle>> {
        let d = self.doc()?;
        if let Some((h, q)) = self.pick_hint
            && q == p
        {
            return Ok((d.space_of(h).as_ref() == Some(space)).then_some(h));
        }
        Ok(select::pick(d, space, p, aperture))
    }

    /// A click with no command running: pick/toggle objects or start a selection window.
    ///
    /// PICKADD on (the default): picks add to the selection and Shift removes. PICKADD off: each pick
    /// replaces the selection and Shift adds (Shift+pick on a selected object removes it).
    pub fn idle_click(&mut self, p: Vec2, shift: bool) -> Result<()> {
        let space = self.space();
        if let Some(pw) = self.pending_window.take() {
            let hs = self.window_selection(&space, pw.corner, p)?;
            self.idle_select_area(hs, shift);
            return Ok(());
        }
        let ap = self.pick_aperture();
        match select::pick(self.doc()?, &space, p, ap) {
            // A piece the 3D view draws only as part of its run (a coarse fidelity tier): the run.
            Some(h) => match cmd::fidelity::pick(self, h) {
                run if run.len() > 1 => self.idle_select_area(run, shift),
                _ => self.idle_pick(h, shift),
            },
            None => self.pending_window = Some(PendingWindow { corner: p, during_command: false }),
        }
        Ok(())
    }

    /// The pick aperture in drawing units: PICKBOX (at least one pixel) and a half again.
    pub fn pick_aperture(&self) -> f64 {
        self.pixel_size() * self.settings.pickbox.max(1.0) * 1.5
    }

    /// The objects an area (a window, crossing or lasso) selected with no command running join
    /// the selection, or leave it with Shift (PICKADD on); with PICKADD off they replace it.
    fn idle_select_area(&mut self, hs: Vec<Handle>, shift: bool) {
        let pickadd = self.settings.pickadd;
        let hs = cmd::groups::expand(self, hs);
        let mut sel = if pickadd || shift { self.selection() } else { Vec::new() };
        if shift && pickadd {
            sel.retain(|h| !hs.contains(h));
        } else {
            sel.extend(hs);
        }
        self.set_selection(sel);
    }

    /// One object picked with no command running: PICKADD adds it (Shift removes it), PICKADD
    /// off replaces the selection with it (Shift adds, or removes it when it was selected).
    fn idle_pick(&mut self, h: Handle, shift: bool) {
        let pickadd = self.settings.pickadd;
        // With group selection the object's selectable groups come (or go) with it.
        let hs = cmd::groups::expand(self, vec![h]);
        let mut sel = if pickadd || shift { self.selection() } else { Vec::new() };
        let had = sel.contains(&h);
        if shift && (pickadd || had) {
            sel.retain(|x| !hs.contains(x));
        } else if !had {
            sel.extend(hs);
        }
        self.set_selection(sel);
    }

    /// Whether objects are being selected: no command running, or a command at "Select objects"
    /// (not while a typed W, C, F… mode collects its points).
    pub fn selecting_objects(&self) -> bool {
        match &self.running {
            None => true,
            Some(_) => self.select_mode.is_none() && self.current_prompt().is_some_and(|p| p.accept.select),
        }
    }

    /// Finish a lasso dragged along `path` (drawing coordinates): the objects it selects go to
    /// the command asking for objects, or with no command running join the selection (Shift
    /// removes them), like a window. Ends any pending window. Does nothing while a command asks
    /// for something other than objects.
    pub fn lasso(&mut self, path: &[Vec2], mode: select::LassoMode, shift: bool) -> Result<()> {
        self.pending_window = None;
        if !self.selecting_objects() {
            return Ok(());
        }
        let hs = select::select_lasso(self.doc()?, &self.space(), path, mode);
        if self.running.is_some() {
            let hs = cmd::groups::expand(self, hs);
            return self.input(Input::Pick(hs));
        }
        self.idle_select_area(hs, shift);
        Ok(())
    }

    /// The objects under `p` that selection cycling steps through, nearest first.
    pub fn pick_candidates(&self, p: Vec2) -> Result<Vec<Handle>> {
        Ok(select::pick_all(self.doc()?, &self.space(), p, self.pick_aperture()))
    }

    /// Pick the object `h` (one of [`Self::pick_candidates`], chosen by selection cycling) as a
    /// click on it would: to the command asking for objects, or into the selection with no
    /// command running (PICKADD and Shift as for any pick). Ends any pending window.
    pub fn pick_handle(&mut self, h: Handle, shift: bool) -> Result<()> {
        self.pending_window = None;
        if !self.selecting_objects() {
            return Ok(());
        }
        let space = self.space();
        if self.doc()?.space(&space).and_then(|st| st.get(h)).is_none() {
            return Err(EngineError::Other(format!("no object {} in this space", h.hex())));
        }
        if self.running.is_some() {
            let hs = cmd::groups::expand(self, vec![h]);
            return self.input(Input::Pick(hs));
        }
        self.idle_pick(h, shift);
        Ok(())
    }

    /// The objects in the selection window from `a` to `b` (drawing points): a window left to
    /// right on screen, a crossing window right to left. In a twisted plan view the window is
    /// the screen's rectangle, a turned one in the drawing.
    fn window_selection(&self, space: &Space, a: Vec2, b: Vec2) -> Result<Vec<Handle>> {
        let d = self.doc()?;
        let twisted = self.state()?.projection().filter(|p| cadaddy_geom::view3::is_plan(p.dir));
        let Some(pr) = twisted else { return Ok(select::select_window(d, space, Bounds2::new(a, b), b.x < a.x)) };
        let (sa, sb) = (pr.project(a.to3(0.0)), pr.project(b.to3(0.0)));
        let screen = Bounds2::new(sa, sb);
        let poly: Option<Vec<Vec2>> = screen.corners().into_iter().map(|c| pr.unproject(c, 0.0)).collect();
        Ok(match poly {
            Some(poly) => select::select_polygon(d, space, &poly, sb.x < sa.x),
            None => select::select_window(d, space, Bounds2::new(a, b), b.x < a.x),
        })
    }

    /// Remember the selection used by a command as "Previous".
    pub fn remember_selection(&mut self, sel: &[Handle]) {
        if let Ok(st) = self.state_mut() {
            st.previous_selection = sel.to_vec();
        }
    }

    // ---------------- views ----------------

    /// The limits of the space being edited: the sheet in paper space, else LIMMIN/LIMMAX.
    pub fn zoom_limits(&self) -> Result<Bounds2> {
        let d = self.doc()?;
        if let Space::Paper(name) = self.space()
            && let Some(sheet) = cadaddy_render::sheet(d, &name)
        {
            return Ok(sheet.bounds());
        }
        let finite = |name: &str, default: Vec2| {
            d.header.point(name).map(|p| p.xy()).filter(|p| p.is_finite() && p.x.abs() <= VIEW_LIMIT && p.y.abs() <= VIEW_LIMIT).unwrap_or(default)
        };
        let lo = finite("LIMMIN", Vec2::ZERO);
        let hi = finite("LIMMAX", Vec2::new(12.0, 9.0));
        Ok(Bounds2::new(lo, hi))
    }

    /// The extents of the current space as seen on screen: on the view plane in a 3D view, of
    /// what 3D draws (the BIM view's built model, levels stacked, without the sheets' lines a
    /// print set's import spreads far and wide).
    pub fn view_extents(&self) -> Result<Bounds2> {
        let st = self.state()?;
        Ok(match st.projection() {
            Some(p) => match cmd::bimlook::display(&st.doc) {
                Some(shown) => cmd::view::view3d::projected_extents(&shown, &p),
                None => cmd::view::view3d::projected_extents(&st.doc, &p),
            },
            None => cached_extents(&st.doc, &self.space()),
        })
    }

    pub fn zoom_extents(&mut self) -> Result<()> {
        let mut ext = self.view_extents()?;
        if !cadaddy_doc::within_limit(&ext, VIEW_LIMIT) {
            // Fit to the finite part and say what was left out.
            let (finite, skipped) = {
                let st = self.state()?;
                st.doc.extents_within(&self.space(), VIEW_LIMIT)
            };
            if skipped > 0 {
                self.echo(format!("Zoom extents: {skipped} object(s) lie beyond \u{b1}1e12 or have non-finite bounds and were left out of the fit."));
            }
            ext = finite;
        }
        let (w, h) = self.viewport_px;
        let ext = if ext.is_empty() { self.zoom_limits()? } else { ext };
        let inset = self.viewport_inset_px;
        let st = self.state_mut()?;
        let view = framed(ext, (w, h), inset, |a| st.zoom_aspect(a), 1.05);
        st.set_zoom_frame(view);
        Ok(())
    }

    /// Zoom by `factor` (> 1 zooms in) keeping `about` fixed on screen.
    pub fn zoom_about(&mut self, factor: f64, about: Vec2) -> Result<()> {
        if !(factor.is_finite() && factor > 0.0) {
            return Ok(());
        }
        let st = self.state_mut()?;
        let v = st.view();
        let h = (v.height / factor).clamp(1e-9, 1e12);
        let k = h / v.height;
        let c = about + (v.center - about) * k;
        st.set_view_quiet(View { center: c, height: h });
        Ok(())
    }

    /// Serializable summary for inspect/automation.
    pub fn summary(&self) -> Value {
        let docs: Vec<Value> = self
            .docs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                serde_json::json!({
                    "index": i, "title": d.title, "path": d.path, "dirty": d.is_dirty(),
                    "entities": d.doc.entity_count(), "active": i == self.active, "revision": d.revision,
                    "blockEditor": d.block_editor(),
                })
            })
            .collect();
        serde_json::json!({
            "documents": docs,
            "prompt": self.prompt_text(),
            "running": self.running.as_ref().map(|r| r.id.clone()),
            "selection": self.selection().iter().map(|h| h.hex()).collect::<Vec<_>>(),
            "settings": self.drafting_settings(),
        })
    }
}

/// Whether `id` restores a drawing from the undo/redo history (the after-command follow-ups
/// must not run on the restored drawing). Uses the [`CommandSpec::restores_history`] flag.
fn moves_through_history(id: &str) -> bool {
    find_command(id).is_some_and(|c| c.restores_history)
}

/// What a typed number or point beyond the export limit is told.
const TOO_LARGE: &str = "The value is too large (limit ±1e12).";

/// Whether typed `t` is a number, or a point (`x,y`, `@x,y`), with a part beyond ±1e12.
fn beyond_limit(t: &str) -> bool {
    let t = t.trim().trim_start_matches('@');
    let parts: Vec<Option<f64>> = t.split(',').take(4).map(|x| x.trim().parse::<f64>().ok().filter(|v| v.is_finite())).collect();
    parts.iter().all(Option::is_some) && parts.iter().flatten().any(|v| v.abs() > 1e12)
}

/// Typed text that is LISP: `(expression)` or `!variable`.
/// What a prompt wants, for text it can't take.
fn not_taken(p: &Prompt) -> &'static str {
    let keywords = !p.keywords.is_empty();
    match (p.accept.select, p.accept.point, p.accept.number) {
        (true, _, _) => "*Invalid selection*",
        (false, true, _) if keywords => "Point or option keyword required.",
        (false, true, _) => "Invalid point.",
        (false, false, true) if keywords => "Requires a number or option keyword.",
        (false, false, true) => "Requires a number.",
        _ if keywords => "Invalid option keyword.",
        _ => "Invalid input.",
    }
}

fn lisp_expression(t: &str) -> bool {
    let t = t.trim();
    t.starts_with('(') || (t.starts_with('!') && t.len() > 1)
}

/// The next command-line input in `rest` and what follows it: up to a space or tab, except that
/// a LISP expression runs to its closing parenthesis (spaces and quoted strings inside it).
fn next_token(rest: &str) -> (&str, Option<&str>) {
    let lead = rest.len() - rest.trim_start_matches('!').len();
    if rest.get(lead..).is_some_and(|r| r.starts_with('(')) {
        let (mut depth, mut in_str, mut escaped) = (0i32, false, false);
        for (i, c) in rest.char_indices().skip(lead) {
            if in_str {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let end = i + 1;
                        // As after any input, a space ends it and a trailing one is Enter.
                        let more = rest.get(end..).filter(|m| !m.is_empty()).map(|m| m.strip_prefix([' ', '\t']).unwrap_or(m));
                        return (&rest[..end], more);
                    }
                }
                _ => {}
            }
        }
        // Unbalanced: all of it (the reader reports the error).
        return (rest, None);
    }
    match rest.split_once([' ', '\t']) {
        Some((tok, more)) => (tok, Some(more)),
        None => (rest, None),
    }
}

#[cfg(test)]
mod tests;

/// An object's DXF group 10 point: the first point of its entity list, or its first grip where
/// entity lists are not built in.
fn group_10(d: &Drawing, e: &Entity) -> Option<Vec2> {
    let Some(codec) = cmd::file::entity_codec() else { return e.kind.grips().first().copied() };
    let (_, tags) = (codec.groups)(d, e);
    let i = tags.iter().position(|t| t.code == 10)?;
    let y = tags.get(i + 1).filter(|t| t.code == 20)?;
    Some(Vec2::new(tags.get(i)?.value.as_f64(), y.value.as_f64()))
}
