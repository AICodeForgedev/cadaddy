//! The CADaddy egui front end.
//!
//! A thin shell over [`cadaddy_engine::Session`]: panels read engine state and act through
//! commands (`app.run(id, params)`) or the command line (`app.cmdline(text)`). Nothing here owns
//! drawing data. The layout follows the familiar CAD desktop: title and tool bar, file tabs,
//! Tool Sets on the left, the drawing area with its command line, Layers and Properties on the
//! right, layout tabs and drafting toggles in the status bar.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

/// FILEDIA is 0: native file dialogs stay closed ([`file_dialogs_allowed`]).
static FILE_DIALOGS_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// An agent's control request is being handled ([`file_dialogs_allowed`]).
static AGENT_CALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A file dialog was refused during the agent request being handled.
static DIALOG_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a native file dialog may open now: not with FILEDIA 0, and never for an agent's
/// request, where it opened on the user's screen and blocked the whole app, every later agent
/// call timing out, until someone answered it (menu sweep 0.7.7e: Insert ▸ PDF Import…).
/// The host's pickers ([`Services`]) check it first.
pub fn file_dialogs_allowed() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if AGENT_CALL.load(Relaxed) {
        DIALOG_REFUSED.store(true, Relaxed);
        return false;
    }
    !FILE_DIALOGS_OFF.load(Relaxed)
}

pub mod aliasdlg;
pub mod animation;
pub mod autosave;
pub mod bedit;
pub mod blockspalette;
pub mod canvas;
pub mod chrome;
pub mod clipboard;
pub mod closing;
pub mod cmddlgs;
pub mod cmdline;
pub mod context_menu;
pub mod control;
pub mod credits;
pub mod dialogs;
pub mod docking;
pub mod hatchdlg;
pub mod hyperlinks;
pub mod materials;
mod mtext_editor;
mod shaded;
pub mod snap3ui;
pub mod spacemouse;
// Report a Bug, Request a Feature, Donate.
pub mod support;
pub mod update;
// The External References palette.
pub mod xrefs;

pub mod dyngrips;
pub mod dyninput;
pub mod extractdlg;
pub mod gpu;
pub mod grip_dyn;
mod groupgrips;
pub mod historypalettes;
pub mod i18n;
pub mod icons;
pub mod layers;
pub mod layerstates;
pub mod layouttabs;
pub mod managers;
pub mod matchpropdlg;
pub mod menus;
pub mod opening;
pub mod options;
pub mod palettes;
pub mod parametric;
pub mod pdfdlg;
pub mod planstomodel;
pub mod plotstyles;
pub mod quick;
pub mod renderwin;
pub mod ribbon;
pub mod shortcut_menus;
pub mod spelldlg;
pub mod theme;
pub mod toldlg;
pub mod toolpalettes;
pub mod viewcube;
pub mod vpscale;
pub mod walk;
pub mod workspaces;

pub mod autocomplete;
pub mod designcenter;
pub mod diag;
pub mod etransmit;
pub mod exchange;
pub mod keytips;
pub mod publishdlg;
mod roomsdlg;
pub mod sheetsets;
pub mod standardsdlg;

use std::sync::mpsc::Receiver;

use cadaddy_engine::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use control::{ControlRequest, ControlResponse, PendingReply};

/// Interface sizes the app takes (50 % to 300 %); anything else (or not a number) is 100 %.
pub fn clamp_ui_scale(s: f32) -> f32 {
    if s.is_finite() { s.clamp(0.5, 3.0) } else { 1.0 }
}

/// The host storage key for [`CadApp::prefs_json`].
pub const PREFS_KEY: &str = "cadaddy.prefs";

/// How many recently opened drawings are kept (AutoCAD's default for its file history).
pub const RECENT_FILES: usize = 9;

/// Persisted UI state (serde, so automation can read and set it).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub docking: docking::Workspace,

    pub interface_language: i18n::Preference,
    pub show_toolsets: bool,
    /// The ribbon (AutoCAD for Windows' tabbed command bar) instead of the tool bar.
    pub ribbon: bool,
    /// The ribbon's active tab ("Home", "Insert", …).
    pub ribbon_tab: String,
    /// Split buttons remember the variant used last: (button's default command, variant).
    pub ribbon_recent: Vec<(String, String)>,
    pub show_palettes: bool,
    pub show_toolbar: bool,
    pub show_file_tabs: bool,
    pub show_status_bar: bool,
    pub show_command_line: bool,
    pub show_viewcube: bool,
    pub show_ucs_icon: bool,
    pub show_layer_list: bool,
    /// Keep the system cursor visible over the drawing area (a small crosshair at the centre of the
    /// drawn one) instead of hiding it. Screen magnifiers (Windows Magnifier "follow the mouse
    /// pointer"), screen readers, recorders and remote desktops track the system cursor and lose it
    /// when it is hidden. Off by default; `CADADDY_SYSTEM_CURSOR=1` turns it on at startup.
    pub system_cursor: bool,
    /// "Drafting" or "Modeling".
    pub toolset_tab: String,
    pub collapsed_groups: Vec<String>,
    /// Tools the user took out of the Tool Sets palette (command ids); each category's gear
    /// menu puts them back.
    pub hidden_tools: Vec<String>,
    pub properties_all: bool,
    /// The properties shown in the Properties palette's "My" view (row names, in English).
    pub my_properties: Vec<String>,
    /// The sheet set file the Sheet Set Manager shows.
    pub sheet_set: Option<String>,
    /// Show an in-window menu bar (platforms without a native menu bar, or on request).
    pub in_window_menu: bool,
    pub start_tab: bool,
    /// Interface size, on top of the operating system's display scaling (1 = 100 %).
    pub ui_scale: f32,
    pub dialog: Option<String>,
    pub history_lines: usize,
    /// AutoComplete's settings (Input Settings on the command line's menu).
    pub autocomplete: autocomplete::Settings,
    /// How often each command was typed, to rank AutoComplete.
    pub command_usage: autocomplete::Usage,
    /// The command line's transparency, percent (0 = opaque as designed).
    pub cmdline_transparency: u8,
    /// The command line's width as a fraction of the drawing area, once dragged (None: automatic).
    pub cmdline_width: Option<f32>,
    /// Interface theme: "system" (follow the OS light/dark appearance), "light" or "dark".
    pub theme: theme::ThemePref,
    /// Animation effects (dialog and menu fades, smoothed scrolling): "system" (follow the OS
    /// setting), "on" or "off".
    pub animations: animation::AnimationPref,
    /// The format Save As suggests for a drawing that has no file yet: "cdy", "dxf" or "dwg".
    pub save_format: SaveFormat,
    /// The version Save As writes for a drawing that has no file yet ("2000" … "2018"); a
    /// drawing opened from a file keeps that file's version.
    pub save_version: String,
    /// "Let AI agents control CADaddy": the app listens on [`cadaddy_engine::AGENT_PORT`]
    /// (this computer only) from its next start. On by default.
    pub agent_control: bool,
    /// Drawings opened from a path, newest first (File ▸ Open Recent and the Start page).
    pub recent_files: Vec<String>,
    /// Keyboard shortcuts the user changed ([`aliasdlg`]).
    pub shortcuts: Vec<aliasdlg::Binding>,
    /// The ribbon tabs shown (the current workspace's); empty shows them all.
    pub ribbon_tabs: Vec<String>,
    /// Workspaces the user saved ([`workspaces`]).
    pub user_workspaces: Vec<workspaces::Def>,
    /// The palette layout is mirrored for a right-to-left interface language ([`CadApp::follow_direction`]).
    pub layout_rtl: bool,
    /// The Blocks palette's recent and favourite blocks, library folder and insertion options.
    pub blocks_palette: blockspalette::Settings,
    /// The user's tool palettes ([`toolpalettes`]).
    pub tool_palettes: Vec<toolpalettes::Palette>,
}

/// A DXF/DWG file version (2000 … 2018).
pub use cadaddy_engine::cmd::file::Version as SaveVersion;

/// The default save format for new drawings, like AutoCAD's Options > Open and Save > "Save as".
/// A drawing opened from a file keeps that file's format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveFormat {
    /// CADaddy's native format (.cdy: keeps everything the drawing holds), and the default.
    #[default]
    Cdy,
    Dxf,
    Dwg,
}

/// Which save formats preferences were written with (2: with .cdy), so [`CadApp::load_prefs`]
/// tells a chosen DXF from the old default.
const SAVE_FORMATS_PREFS: u64 = 2;

impl SaveFormat {
    pub const ALL: [SaveFormat; 3] = [SaveFormat::Cdy, SaveFormat::Dxf, SaveFormat::Dwg];

    /// The file extension, which is also the saved name ("cdy", "dxf" or "dwg").
    pub fn as_str(self) -> &'static str {
        match self {
            SaveFormat::Cdy => "cdy",
            SaveFormat::Dxf => "dxf",
            SaveFormat::Dwg => "dwg",
        }
    }

    pub fn parse(s: &str) -> Option<SaveFormat> {
        SaveFormat::ALL.into_iter().find(|f| f.as_str().eq_ignore_ascii_case(s.trim().trim_start_matches('.')))
    }

    /// The name Save As suggests: a title that already has an extension (a drawing opened from a
    /// file) keeps it; an untitled drawing gets this format's extension.
    pub fn suggested_name(self, title: Option<&str>) -> String {
        match title.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) if std::path::Path::new(t).extension().is_some() => t.to_owned(),
            Some(t) => format!("{t}.{}", self.as_str()),
            None => format!("Drawing.{}", self.as_str()),
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            docking: docking::Workspace::default(),

            interface_language: i18n::Preference::default(),
            show_toolsets: false,
            ribbon: true,
            ribbon_tab: "Home".into(),
            ribbon_recent: Vec::new(),
            show_palettes: true,
            show_toolbar: true,
            show_file_tabs: true,
            show_status_bar: true,
            show_command_line: true,
            show_viewcube: true,
            show_ucs_icon: true,
            show_layer_list: false,
            system_cursor: std::env::var("CADADDY_SYSTEM_CURSOR").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "off")),
            toolset_tab: "Drafting".into(),
            collapsed_groups: Vec::new(),
            hidden_tools: Vec::new(),
            properties_all: true,
            my_properties: Vec::new(),
            sheet_set: None,
            // AutoCAD hides the menu bar under the ribbon (MENUBAR 0).
            in_window_menu: false,
            start_tab: false,
            ui_scale: 1.0,
            dialog: None,
            history_lines: 3,
            autocomplete: autocomplete::Settings::default(),
            command_usage: autocomplete::Usage::default(),
            cmdline_transparency: 0,
            cmdline_width: None,
            theme: theme::ThemePref::default(),
            animations: animation::AnimationPref::default(),
            save_format: SaveFormat::default(),
            save_version: SaveVersion::default().year().to_string(),
            agent_control: true,
            recent_files: Vec::new(),
            shortcuts: Vec::new(),
            ribbon_tabs: Vec::new(),
            user_workspaces: Vec::new(),
            layout_rtl: false,
            blocks_palette: blockspalette::Settings::default(),
            tool_palettes: toolpalettes::default_palettes(),
        }
    }
}

/// Host services (file pickers, clipboard) injected by the app so this crate stays portable.
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn() -> Option<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
    /// A PDF to convert or import (PDFIMPORT, Convert PDF to DWG).
    pub pick_pdf: Option<Box<dyn Fn() -> Option<String>>>,
    /// A raster image to attach (IMAGEATTACH, ATTACH).
    pub pick_image: Option<Box<dyn Fn() -> Option<String>>>,
    /// The system clipboard's text (Paste to Command Line).
    pub read_clipboard: Option<Box<dyn Fn() -> Option<String>>>,
    /// A drawing file's preview as PNG bytes (a .cdy file's embedded thumbnail), for the Start
    /// page's recent files; `None` when the file has none.
    pub thumbnail: Option<Box<dyn Fn(&str) -> Option<Vec<u8>>>>,
    /// Where to save an image (the Render window's Save), given a suggested name.
    pub pick_save_image: Option<Box<dyn Fn(&str) -> Option<String>>>,
}

pub struct CadApp {
    pub system_languages: Vec<String>,
    /// Whether the operating system shows animations, as the host reads it; `None` when the OS
    /// states nothing or the host cannot ask ([`animation`]).
    pub system_animations: Option<bool>,
    pub session: Session,
    pub ui: UiState,
    /// The interface's own preferences from the Options dialog ([`options`]).
    pub options: options::UiOptions,
    pub services: Services,
    pub canvas: canvas::CanvasState,
    /// The PDF Import / Convert PDF to DWG dialog's choices while it is open ([`pdfdlg`]).
    pub pdf: Option<pdfdlg::PdfDialog>,
    /// The Geometric Tolerance dialog's fields while it is open ([`toldlg`]).
    pub tol_dlg: Option<toldlg::ToleranceDialog>,
    /// Desktop HID status and navigation preferences; the web host supplies no HID backend.
    pub spacemouse: spacemouse::State,
    pub cmd: cmdline::CmdLine,
    pub status: Option<(String, f64)>,
    /// The workspace last arranged (WSCURRENT when it was applied; [`workspaces::follow`]).
    pub workspace_applied: Option<String>,
    /// Lay the interface out right to left (`Some(true)`) or left to right whatever the language:
    /// for tests and screenshots on a computer without an Arabic font.
    pub direction_override: Option<bool>,
    /// A block the Blocks palette is placing, and how many block references there were when it
    /// started (Repeat Placement; [`blockspalette::follow`]).
    pub placing: Option<(blockspalette::BlockRef, usize)>,
    /// Drawings being read on worker threads ([`opening`]).
    pub opening: opening::Opening,
    pub integrated_titlebar: bool,
    /// The window title last sent to the OS ("CADaddy - name*"), so it is only sent on a change.
    os_title: String,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_shots: Vec<(u64, Option<String>, std::sync::mpsc::Sender<Value>, f64)>,
    queued_shots: Vec<(u64, f64, u32)>,
    shot_token: u64,
    pub synthetic: Vec<egui::Event>,
    /// Caps the frame rate when presenting without vsync (see [`gpu::FrameCap`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub frame_cap: Option<gpu::FrameCap>,
    /// Language whose interface fonts are installed (`None` until the first frame).
    fonts_language: Option<&'static str>,
    /// The interface fonts include the system CJK fonts.
    fonts_cjk: bool,
    /// Open drawings (by uid) whose names were checked for CJK characters.
    cjk_scanned: std::collections::HashSet<u64>,
    /// An open drawing had CJK names: the CJK fonts stay installed from then on.
    needs_cjk: bool,
    /// Each open drawing's file as last seen ([`CadApp::track_documents`]).
    seen_docs: std::collections::HashMap<u64, Option<String>>,
    /// The slowest time of each part of a frame since `ui.inspect` last reported them (ms).
    pub slowest: std::collections::BTreeMap<&'static str, f64>,
    /// When the last frame started (for the gap between frames, which includes painting and
    /// presenting).
    last_frame_start: Option<f64>,
    /// Until when (ms) an agent counts as active: the app then looks for control requests every
    /// few milliseconds instead of sleeping until something repaints it.
    control_active_until: f64,
    /// The command an agent started from the control channel and may go on typing into (see
    /// `control::users_command`).
    pub(crate) agent_run: Option<usize>,
    /// The theme last installed into egui (`None` until the first frame).
    shown_theme: Option<egui::Theme>,
    /// Whether the last applied theme was handed to egui as the System preference.
    shown_follows_system: bool,
    /// The high-contrast palette was applied (with `shown_theme`).
    shown_contrast: bool,
    /// Whether egui was last told to animate (`None` until the first frame).
    shown_animations: Option<bool>,
    pub frame_ms: f64,
    pub quit_requested: bool,
    /// A web page to open in the browser on the next frame (Report a Bug, Donate).
    pub pending_url: Option<String>,
    /// What the support banner remembers ([`support`]).
    pub donate: support::DonatePrefs,
    /// QuickCalc's history and variables, kept in the preferences.
    pub quickcalc: palettes::quickcalc::Saved,
    /// Looking for, downloading and installing new versions ([`update`]); the desktop host
    /// supplies the updater.
    pub updater: Option<Box<dyn update::Updater>>,
    /// A palette asked to turn the current layer off: the question waits ([`layers::off_prompt`]).
    pub layer_off_ask: Option<String>,
    pub update: update::UpdateState,
    /// The app may close although drawings have unsaved changes (they were dealt with, or the
    /// quit came from the control channel, which never asks).
    pub quit_confirmed: bool,
    /// A close request waiting for "Save changes?" answers ([`closing`]).
    pub closing: Option<closing::Closing>,
    /// Autosave (desktop; `None` until [`Self::enable_autosave`]).
    pub autosave: Option<cadaddy_engine::autosave::Autosaver>,
    /// Drawing Recovery: autosave files a crashed app left behind.
    pub recovery: autosave::Recovery,
    /// GPU errors and device loss reported by wgpu ([`gpu::GpuFaults`]).
    pub gpu_faults: std::sync::Arc<gpu::GpuFaults>,
    /// The notice shown after one (see [`autosave::gpu_notice`]).
    pub gpu_notice: Option<autosave::GpuNotice>,
    /// The Hatch and Gradient dialog while open or picking ([`hatchdlg`]).
    pub hatch_dlg: Option<hatchdlg::HatchDlgState>,
    /// The Data Extraction wizard while open ([`extractdlg`]).
    pub extract_dlg: Option<extractdlg::State>,
    /// The Check Spelling dialog while open ([`spelldlg`]).
    pub spell_dlg: Option<spelldlg::State>,
    /// HYPERLINK is selecting objects; its dialog opens once that ends ([`hyperlinks::resume`]).
    pub hyperlink_pick: bool,
    /// The Layer States Manager while open ([`layerstates`]).
    pub layer_states_dlg: Option<layerstates::State>,
    /// The layout tabs' rename, selection and Move or Copy dialog ([`layouttabs`]).
    pub layout_tabs: layouttabs::LayoutTabs,
    /// The Edit Block Definition dialog ([`bedit`]).
    pub bedit: bedit::State,
    /// Walk mode, while it is open: the drawing in 3D in place of the canvas ([`walk`]).
    pub walk: Option<walk::Walk>,
    /// The app's wgpu render state (walk mode draws its 3D with it).
    pub wgpu_rs: Option<egui_wgpu::RenderState>,
}

impl CadApp {
    pub fn new(session: Session, services: Services) -> Self {
        CadApp {
            system_languages: Vec::new(),
            system_animations: None,
            session,
            ui: UiState::default(),
            options: options::UiOptions::default(),
            services,
            canvas: canvas::CanvasState::default(),
            pdf: None,
            tol_dlg: None,
            spacemouse: spacemouse::State::default(),
            cmd: cmdline::CmdLine::default(),
            status: None,
            workspace_applied: None,
            direction_override: None,
            placing: None,
            opening: opening::Opening::default(),
            integrated_titlebar: false,
            os_title: String::new(),
            control_rx: None,
            pending_shots: Vec::new(),
            queued_shots: Vec::new(),
            shot_token: 0,
            synthetic: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            frame_cap: None,
            fonts_language: None,
            fonts_cjk: false,
            cjk_scanned: std::collections::HashSet::new(),
            needs_cjk: false,
            seen_docs: std::collections::HashMap::new(),
            slowest: std::collections::BTreeMap::new(),
            last_frame_start: None,
            control_active_until: 0.0,
            agent_run: None,
            shown_theme: None,
            shown_follows_system: false,
            shown_contrast: false,
            shown_animations: None,
            frame_ms: 0.0,
            quit_requested: false,
            pending_url: None,
            donate: support::DonatePrefs::default(),
            quickcalc: Default::default(),
            updater: None,
            layer_off_ask: None,
            update: update::UpdateState::default(),
            quit_confirmed: false,
            closing: None,
            autosave: None,
            recovery: autosave::Recovery::default(),
            gpu_faults: std::sync::Arc::default(),
            gpu_notice: None,
            hatch_dlg: None,
            extract_dlg: None,
            spell_dlg: None,
            hyperlink_pick: false,
            layer_states_dlg: None,
            layout_tabs: layouttabs::LayoutTabs::default(),
            bedit: bedit::State::default(),
            walk: None,
            wgpu_rs: None,
        }
    }

    /// The interface language shown: the chosen one, or English when this computer has no font
    /// for it (Chinese or Korean without a suitable font would be boxes).
    /// The interface reads right to left (Arabic): the palettes swap sides, the ribbon, menu bar
    /// and command line run from the right. The drawing itself stays as drawn.
    pub fn rtl(&self) -> bool {
        self.direction_override.unwrap_or_else(|| i18n::is_rtl(self.language()))
    }

    /// Mirror the palette layout when the interface changes reading direction.
    pub fn follow_direction(&mut self) {
        let rtl = self.rtl();
        if rtl != self.ui.layout_rtl {
            self.ui.docking = self.ui.docking.mirrored();
            self.ui.layout_rtl = rtl;
        }
    }

    pub fn language(&self) -> &'static str {
        let l = self.ui.interface_language.resolve(&self.system_languages);
        if theme::can_show(l) { l } else { "en" }
    }

    /// Draw the canvas on the GPU with the app's wgpu render state (eframe's
    /// `CreationContext::wgpu_render_state`). Without it the canvas draws on the CPU. `stencil`:
    /// the format of the render pass's stencil buffer when the app asked for one (eframe
    /// `NativeOptions::stencil_buffer`); clipped viewports are clipped with it.
    pub fn set_wgpu(&mut self, rs: &egui_wgpu::RenderState, stencil: Option<egui_wgpu::wgpu::TextureFormat>) {
        self.canvas.gpu = Some(gpu::install(rs, &self.gpu_faults, stencil));
        self.canvas.gpu_device = Some(rs.device.clone());
        self.wgpu_rs = Some(rs.clone());
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run a command programmatically (or a UI command).
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        diag::note(&format!("command {id}"));
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        let r = self.session.execute(id, &params);
        if let Ok(v) = &r
            && let Some(m) = v.get("message").and_then(Value::as_str)
        {
            for l in m.lines() {
                self.session.echo(l.to_string());
            }
        }
        if let Err(e) = &r {
            self.session.echo(e.user_message());
        }
        r.map_err(|e| e.to_string())
    }

    /// Start a command as if typed (interactive when it has prompts).
    pub fn start(&mut self, name: &str) {
        if menus::run_ui_command(self, name, &Value::Null).is_some() {
            return;
        }
        if let Err(e) = self.session.start(name) {
            self.session.echo(e.user_message());
        }
    }

    /// Submit a line of command-line text. A refused line is echoed to the history; the error is
    /// also returned.
    pub fn cmdline(&mut self, text: &str) -> Option<String> {
        if !self.session.prompting() {
            let first = text.split_whitespace().next().unwrap_or("");
            // `cmd {json}` is a programmatic call: the engine runs it with those parameters.
            let json_form = text.trim_start().get(first.len()..).is_some_and(|rest| rest.trim_start().starts_with('{'));
            let word = first.to_ascii_lowercase();
            let more = text.split_whitespace().nth(1).is_some();
            if !first.is_empty()
                && !json_form
                && (!more || !menus::DIALOG_WHEN_ALONE.contains(&word.as_str()))
                && menus::run_ui_command(self, &word, &Value::Null).is_some()
            {
                return None;
            }
        }
        let e = self.session.cmdline(text).err()?.to_string();
        self.session.echo(e.clone());
        Some(e)
    }

    /// The theme the interface shows now (the choice resolved against the OS appearance).
    pub fn shown_theme(&self) -> egui::Theme {
        self.shown_theme.unwrap_or_else(|| self.ui.theme.resolve(None))
    }

    /// Whether the interface shows animation effects now (the choice resolved against the OS
    /// setting).
    pub fn shows_animations(&self) -> bool {
        self.ui.animations.resolve(self.system_animations)
    }

    /// Preferences kept across restarts, as JSON for the host's storage ([`PREFS_KEY`]).
    pub fn prefs_json(&self) -> String {
        let mut prefs = json!({
            "theme": self.ui.theme.as_str(),
            "animations": self.ui.animations.as_str(),
            "interfaceLanguage": self.ui.interface_language.code(),
            "saveFormat": self.ui.save_format.as_str(),
            "saveFormats": SAVE_FORMATS_PREFS,
            "saveVersion": self.ui.save_version,
        });
        if self.spacemouse.settings != spacemouse::Settings::default() {
            prefs["spacemouse"] = json!(self.spacemouse.settings);
        }
        if !self.quickcalc.is_empty() {
            prefs["quickcalc"] = self.quickcalc.to_json();
        }
        if !self.ui.recent_files.is_empty() {
            prefs["recentFiles"] = json!(self.ui.recent_files);
        }
        if self.donate.shown_ms > 0.0 {
            prefs["supportBannerShown"] = json!(self.donate.shown_ms);
        }
        if !self.update.auto {
            prefs["checkForUpdates"] = json!(false);
        }
        if !self.ui.agent_control {
            prefs["agentControl"] = json!(false);
        }
        if self.update.checked_ms > 0.0 {
            prefs["updateChecked"] = json!(self.update.checked_ms);
        }
        if !self.update.skip.is_empty() {
            prefs["updateSkip"] = json!(self.update.skip);
        }
        prefs.to_string()
    }

    /// Restore preferences saved by [`Self::prefs_json`]; unknown or malformed values are ignored.
    pub fn load_prefs(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<Value>(json) else { return };
        if let Some(prefs) = v.get("spacemouse") {
            let _ = self.spacemouse.load_settings(prefs);
        }
        if let Some(t) = v.get("theme").and_then(Value::as_str).and_then(theme::ThemePref::parse) {
            self.ui.theme = t;
        }
        if let Some(a) = v.get("animations").and_then(Value::as_str).and_then(animation::AnimationPref::parse) {
            self.ui.animations = a;
        }
        if let Some(l) = v.get("interfaceLanguage").and_then(Value::as_str).and_then(i18n::Preference::parse) {
            self.ui.interface_language = l;
        }
        if let Some(f) = v.get("saveFormat").and_then(Value::as_str).and_then(SaveFormat::parse) {
            // Preferences from before .cdy always held DXF, then the default: that is no choice,
            // so new drawings get the new default (a chosen DWG stays).
            let before_cdy = v.get("saveFormats").and_then(Value::as_u64).is_none_or(|n| n < SAVE_FORMATS_PREFS);
            self.ui.save_format = if before_cdy && f == SaveFormat::Dxf { SaveFormat::default() } else { f };
        }
        if let Some(v) = v.get("saveVersion").and_then(Value::as_str).and_then(SaveVersion::parse) {
            self.ui.save_version = v.year().to_string();
        }
        if let Some(files) = v.get("recentFiles").and_then(Value::as_array) {
            self.ui.recent_files = files.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).take(RECENT_FILES).map(str::to_string).collect();
        }
        if let Some(ms) = v.get("supportBannerShown").and_then(Value::as_f64).filter(|ms| ms.is_finite()) {
            self.donate.shown_ms = ms;
        }
        if let Some(b) = v.get("checkForUpdates").and_then(Value::as_bool) {
            self.update.auto = b;
        }
        if let Some(b) = v.get("agentControl").and_then(Value::as_bool) {
            self.ui.agent_control = b;
        }
        if let Some(ms) = v.get("updateChecked").and_then(Value::as_f64).filter(|ms| ms.is_finite()) {
            self.update.checked_ms = ms;
        }
        if let Some(skip) = v.get("updateSkip").and_then(Value::as_str).filter(|s| s.len() <= 32) {
            self.update.skip = skip.to_string();
        }
        if let Some(q) = v.get("quickcalc") {
            self.quickcalc = palettes::quickcalc::Saved::from_json(q);
        }
    }

    /// Whether an agent can drive this window over the control channel (`--control PORT`).
    pub fn has_control_channel(&self) -> bool {
        self.control_rx.is_some()
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), now_ms()));
    }

    /// Open the drawing at `path`. Natively the file is read on a worker thread while an
    /// "Opening ..." card shows ([`opening`]); [`Self::finish_opening`] adds it when ready. On the
    /// web it opens at once.
    pub fn open_path(&mut self, path: &str) {
        // A LISP file (dropped on the window): loaded as APPLOAD loads it.
        if is_lisp_file(path) {
            match cadaddy_engine::lisp::builtins::read_text_file(path) {
                Some(text) => self.load_lisp(path, &text),
                None => self.open_failed(format!("APPLOAD: can't read {path}")),
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.opening.start(path);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let r = self.run("open", json!({ "path": path }));
            self.opened(r);
        }
    }

    /// Add the files whose background read has finished; report the ones that failed as a failed
    /// OPEN is reported. Called every frame.
    pub fn finish_opening(&mut self) {
        for (_, outcome) in self.opening.poll() {
            let r = match outcome {
                Ok(f) => self.session.finish_open(f).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            if let Err(e) = &r {
                self.session.echo(e.clone());
            }
            self.opened(r);
        }
    }

    /// Load a LISP file: its routine runs and may wait for the user (APPLOAD typed or from the
    /// menu runs this way too).
    fn load_lisp(&mut self, name: &str, text: &str) {
        if let Err(e) = self.session.lisp_load(name, text, false) {
            self.session.echo(e.to_string());
        }
    }

    /// The largest file [`Self::open_bytes`] opens. A drawing opened from its contents (the
    /// browser, where files have no path) is held in memory several times over while it is decoded,
    /// so it is capped below the DWG reader's own limit instead of running out of memory.
    pub const MAX_OPEN_BYTES: u64 = 256 << 20;

    /// Why a `len`-byte file called `name` is too large for [`Self::open_bytes`], if it is.
    pub fn open_size_error(name: &str, len: u64) -> Option<String> {
        (len > Self::MAX_OPEN_BYTES).then(|| {
            format!(
                "Open: {name} is {:.1} MB, larger than the {} MB CADaddy opens in the browser",
                len as f64 / 1048576.0,
                Self::MAX_OPEN_BYTES >> 20
            )
        })
    }

    /// Open a drawing from its contents where there is no path to open (on the web: the file
    /// picker, a dropped file). Runs OPEN with `{data, name}` as [`Self::open_path`] runs it with
    /// `{path}`; failures show on the command line and in the status bar.
    pub fn open_bytes(&mut self, name: &str, bytes: &[u8]) {
        if let Some(e) = Self::open_size_error(name, bytes.len() as u64) {
            return self.open_failed(e);
        }
        if is_lisp_file(name) {
            return self.load_lisp(name, &String::from_utf8_lossy(bytes));
        }
        let data = cadaddy_engine::cmd::file::base64_encode(bytes);
        let r = self.run("open", json!({ "data": data, "name": name }));
        self.opened(r);
    }

    /// Report a file that could not be opened before OPEN ran (unreadable, too large) as a failed
    /// OPEN is reported: on the command line and in the status bar.
    pub fn open_failed(&mut self, e: impl Into<String>) {
        let e = e.into();
        self.session.echo(e.clone());
        self.set_status(e);
    }

    fn opened(&mut self, r: Result<Value, String>) {
        match r {
            Err(e) => self.set_status(e),
            Ok(v) => {
                self.ui.start_tab = false;
                self.canvas.zoom_pending = true;
                // Recent files and replacing an untouched Drawing1: [`CadApp::track_documents`].
                self.track_documents();
                // A file that was already open switches to its tab; it still goes to the top of the
                // recent files, though no new drawing appeared for track_documents to see.
                if v.get("alreadyOpen").and_then(Value::as_bool) == Some(true)
                    && let Some(path) = self.session.docs.get(self.session.active).and_then(|d| d.path.clone())
                {
                    self.remember_recent(path);
                }
                // A partial read (DWG records left out) shows in the status bar too, not only on
                // the command line.
                if let Some(w) = v.get("warnings").and_then(Value::as_array).and_then(|w| w.first()).and_then(Value::as_str) {
                    self.set_status(w.to_string());
                }
                // A dynamic block's own drawing: ask whether to open it in the Block Editor.
                if self.session.state().is_ok_and(|st| cadaddy_engine::blockedit::has_authoring_elements(&st.doc)) {
                    self.ui.dialog = Some("authoring".into());
                }
            }
        }
    }

    /// Put `path` first in File ▸ Open Recent and on the Start page.
    /// Every way a drawing gets a file (the Open dialog, typed OPEN, SAVEAS, an agent's open):
    /// the file goes to the top of the recent files, and a newly opened file replaces untouched
    /// new drawings (never saved, no changes, nothing to undo), as in AutoCAD.
    fn track_documents(&mut self) {
        let mut opened = None;
        let mut recent = Vec::new();
        for d in &self.session.docs {
            let before = self.seen_docs.get(&d.uid);
            if d.path.is_some() && before != Some(&d.path) {
                recent.extend(d.path.clone());
                if before.is_none() {
                    opened = Some(d.uid);
                }
            }
        }
        for path in recent {
            self.remember_recent(path);
        }
        if let Some(uid) = opened {
            // An agent's open shows the drawing, not the Start page it was opened from.
            self.ui.start_tab = false;
            let pristine: Vec<usize> = self
                .session
                .docs
                .iter()
                .enumerate()
                .filter(|(_, d)| d.uid != uid && d.path.is_none() && !d.is_dirty() && d.undo.is_empty() && d.redo.is_empty())
                .map(|(i, _)| i)
                .collect();
            for i in pristine.into_iter().rev() {
                let _ = self.session.execute("close", &json!({ "index": i }));
            }
            if let Some(i) = self.session.docs.iter().position(|d| d.uid == uid) {
                self.session.active = i;
            }
        }
        self.seen_docs = self.session.docs.iter().map(|d| (d.uid, d.path.clone())).collect();
    }

    fn remember_recent(&mut self, path: String) {
        let recent = &mut self.ui.recent_files;
        recent.retain(|p| *p != path);
        recent.insert(0, path);
        recent.truncate(RECENT_FILES);
    }

    /// Per-frame logic before layout.
    /// Record `ms` for the frame part `name` if it is its slowest yet ([`CadApp::slowest`]).
    fn note_time(&mut self, name: &'static str, ms: f64) {
        let e = self.slowest.entry(name).or_insert(0.0);
        if ms > *e {
            *e = ms;
        }
    }

    pub fn logic(&mut self, ctx: &egui::Context) {
        let _busy = diag::busy("logic");
        workspaces::follow(self);
        self.follow_direction();
        blockspalette::follow(self);
        // FILEDIA 0: no native file dialogs (the commands ask on the command line).
        FILE_DIALOGS_OFF
            .store(cadaddy_engine::sysvars::get(&self.session, "FILEDIA").and_then(|v| v.as_i64()) == Some(0), std::sync::atomic::Ordering::Relaxed);
        let t0 = now_ms();
        if let Some(last) = self.last_frame_start {
            self.note_time("betweenFrames", t0 - last);
        }
        self.last_frame_start = Some(t0);
        self.track_documents();
        cadaddy_engine::cmd::file::set_default_ext(self.ui.save_format.as_str());
        // A long LISP routine runs a slice per frame, so the window stays live (and Esc stops it).
        self.session.lisp_tick();
        if self.session.lisp_busy() {
            ctx.request_repaint();
        }
        // Esc in a dialog is Cancel: it closes the dialog, and the command line doesn't also
        // get it (cancelling a command waiting behind the dialog).
        if self.ui.dialog.is_some() && self.closing.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            // As the dialog's Cancel does: what it was editing goes with it.
            match self.ui.dialog.take().as_deref() {
                Some("hatch") => self.hatch_dlg = None,
                Some("tolerance") => self.tol_dlg = None,
                Some("pdfimport") => self.pdf = None,
                // The Layer States Manager goes back to where it was opened from, as its Close does.
                Some("layerstates") => self.ui.dialog = self.layer_states_dlg.take().and_then(|s| s.back()),
                _ => {}
            }
        }
        // Closing the window (title bar, File ▸ Exit, Cmd+Q) with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && self.request_quit() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        // The OS window title names the current drawing, `*` when it has unsaved changes.
        let os_title = match self.session.state() {
            Ok(st) if !self.ui.start_tab => format!("CADaddy - {}{}", st.title, if st.is_dirty() { "*" } else { "" }),
            _ => "CADaddy".to_string(),
        };
        if os_title != self.os_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(os_title.clone()));
            self.os_title = os_title;
        }
        let language = self.language();
        // Drawings opened since the last frame: do their names need a CJK font in the interface?
        for st in &self.session.docs {
            if self.cjk_scanned.insert(st.uid) {
                let d = &st.doc;
                self.needs_cjk |= theme::has_cjk(&st.title)
                    || d.layers.iter().any(|l| theme::has_cjk(&l.name))
                    || d.blocks.keys().any(|k| theme::has_cjk(k))
                    || d.layouts.iter().any(|l| theme::has_cjk(&l.name))
                    || d.text_styles.iter().any(|t| theme::has_cjk(&t.name));
            }
        }
        // The language pickers (the Language dialog, Options) name languages in their own scripts.
        let picker = self.ui.dialog.as_deref().is_some_and(|d| d == "language" || d.starts_with("options"));
        let cjk = self.needs_cjk || picker || matches!(language, "zh" | "ja" | "ko");
        if self.fonts_language != Some(language) || (cjk && !self.fonts_cjk) {
            theme::install_fonts(ctx, language, cjk);
            self.fonts_language = Some(language);
            self.fonts_cjk = cjk;
        }
        // Resolved every frame, so a System choice follows the OS appearance live (winit and the
        // browser report changes as input); the saved choice itself never changes here.
        // Interface size apart from the OS display scaling.
        let scale = clamp_ui_scale(self.ui.ui_scale);
        if (ctx.zoom_factor() - scale).abs() > 1e-4 {
            ctx.set_zoom_factor(scale);
        }
        let shown = self.ui.theme.resolve(ctx.system_theme());
        let contrast = self.ui.theme == theme::ThemePref::HighContrast;
        theme::set_active(shown);
        theme::set_high_contrast(contrast);
        theme::set_canvas(self.options.canvas());
        let follows_system = self.ui.theme == theme::ThemePref::System;
        if self.shown_theme != Some(shown) || self.shown_follows_system != follows_system || self.shown_contrast != contrast {
            theme::apply(ctx, shown, follows_system);
            self.shown_theme = Some(shown);
            self.shown_follows_system = follows_system;
            self.shown_contrast = contrast;
        }
        // Resolved every frame too, so a System choice follows the OS setting live.
        let animate = self.shows_animations();
        if self.shown_animations != Some(animate) {
            animation::apply(ctx, animate);
            self.shown_animations = Some(animate);
        }
        if !animate {
            animation::unsmooth_scroll(ctx);
        }
        let t1 = now_ms();
        self.note_time("logicBeforeControl", t1 - t0);
        self.drain_control(ctx);
        let t2 = now_ms();
        self.note_time("control", t2 - t1);
        // While an agent is sending requests, look for the next one every few milliseconds. The
        // control thread's wake-up can be lost on Windows (it arrives while a frame is drawn),
        // and an idle window then took its next request only at the next timed repaint, 0.1 to
        // 0.35 s later (UI review 0.7.7c: the "stalls" after edits).
        if t2 < self.control_active_until {
            ctx.request_repaint_after(std::time::Duration::from_millis(5));
        }
        self.finish_opening();
        cadaddy_engine::cmd::file::sync_image_folder(&self.session);
        self.opening.show(ctx);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
        self.collect_screenshots(ctx);
        self.issue_screenshots(ctx);
        self.gpu_fault_tick(ctx);
        managers::plot_window_resume(self, ctx);
        hatchdlg::resume(self);
        hyperlinks::resume(self);
        let t3 = now_ms();
        self.autosave_tick(ctx);
        self.note_time("autosave", now_ms() - t3);
        menus::shortcuts(self, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            let p = f.path().to_string_lossy().to_string();
            if !p.is_empty() {
                self.open_path(&p);
            }
        }
        self.note_time("logicAfterControl", now_ms() - t2);
    }

    /// Wait for the frame cap (if any), then inject synthetic events (one pointer event per frame).
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(cap) = &mut self.frame_cap {
            cap.wait();
        }
        if self.synthetic.is_empty() {
            return;
        }
        let n = match self.synthetic.first() {
            Some(egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }) => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        if let Some(egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. }) = self.synthetic.first() {
            raw.events.push(egui::Event::PointerMoved(*p));
        }
        // A synthetic click holds its Shift/Ctrl (Shift+click a grip); the next move lets go.
        match self.synthetic.first() {
            Some(egui::Event::PointerButton { modifiers, .. }) => raw.events.push(egui::Event::ModifiersChanged(*modifiers)),
            Some(egui::Event::PointerMoved(_)) => raw.events.push(egui::Event::ModifiersChanged(egui::Modifiers::default())),
            _ => {}
        }
        let n = n.min(self.synthetic.len());
        raw.events.extend(self.synthetic.drain(..n));
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _busy = diag::busy("draw");
        i18n::with_language(self.language(), || self.draw(ui));
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let t0 = now_ms();
        control::begin_frame(ui.ctx());
        let t = theme::Tokens::get();
        // With the ribbon, a menu bar sits above it (AutoCAD's MENUBAR 1); otherwise under the tool bar.
        if self.ui.ribbon && self.ui.in_window_menu {
            menus::menu_bar(self, ui);
        }
        chrome::title_and_toolbar(self, ui);
        if !self.ui.ribbon && self.ui.in_window_menu {
            menus::menu_bar(self, ui);
        }
        if self.ui.show_status_bar {
            chrome::status_bar(self, ui);
        }
        if self.ui.show_file_tabs {
            chrome::file_tabs(self, ui);
        }
        let has_doc = !self.session.docs.is_empty() && !self.ui.start_tab;
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
            // Walk mode shows in the drawing's own tab (docking), the palettes beside it.
            if has_doc {
                docking::show(self, ui);
            } else {
                chrome::start_page(self, ui);
            }
        });
        dialogs::show(self, ui.ctx());
        layers::off_prompt(self, ui.ctx());
        palettes::patterns::prompt_picker(self, ui.ctx());
        layouttabs::move_copy_dialog(self, ui.ctx());
        matchpropdlg::show(self, ui.ctx());
        renderwin::show(self, ui.ctx());
        closing::prompt(self, ui.ctx());
        autosave::show(self, ui.ctx());
        autosave::gpu_notice(self, ui.ctx());
        update::tick(self);
        update::card(self, ui.ctx());
        if !self.update.busy() {
            support::banner(self, ui.ctx());
        }
        if let Some(url) = self.pending_url.take() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        control::observe_drawn(ui.ctx());
        self.frame_ms = now_ms() - t0;
        self.note_time("draw", self.frame_ms);
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        // A minimised window is woken at most every 100 ms (eframe throttles hidden windows to save
        // CPU), which made every agent call take that long. While minimised, keep listening a
        // little after each request so an agent's run of calls is served back to back; there is
        // nothing to draw meanwhile.
        // (Desktop only: the web build can't block.)
        let minimised = cfg!(not(target_arch = "wasm32")) && ctx.input(|i| i.viewport().minimized == Some(true));
        loop {
            while let Ok(req) = rx.try_recv() {
                self.handle_control_request(ctx, req);
            }
            if !minimised {
                break;
            }
            match rx.recv_timeout(std::time::Duration::from_millis(80)) {
                Ok(req) => self.handle_control_request(ctx, req),
                Err(_) => break,
            }
        }
        self.control_rx = Some(rx);
    }

    /// Handle one control request as the app does between frames (an agent's call: no file
    /// dialogs, see [`file_dialogs_allowed`]); the reply goes to the request's sender.
    pub fn serve_control_request(&mut self, ctx: &egui::Context, req: control::ControlRequest) {
        self.handle_control_request(ctx, req);
    }

    fn handle_control_request(&mut self, ctx: &egui::Context, req: control::ControlRequest) {
        self.control_active_until = now_ms() + 2000.0;
        // Timed out while queued: the client was told it did not run (#345).
        if !req.begin() {
            return;
        }
        let reply = req.reply.clone();
        AGENT_CALL.store(true, std::sync::atomic::Ordering::Relaxed);
        let outcome = control::handle(self, ctx, &req);
        AGENT_CALL.store(false, std::sync::atomic::Ordering::Relaxed);
        match outcome {
            control::Outcome::Done(mut v) => {
                if DIALOG_REFUSED.swap(false, std::sync::atomic::Ordering::Relaxed)
                    && let Some(o) = v.as_object_mut()
                {
                    o.insert(
                        "warning".into(),
                        json!("a file dialog was not opened for this agent request (it would block the app until someone answers it): give the file as a parameter, e.g. engine.execute with `path`"),
                    );
                }
                let _ = reply.send(v);
            }
            control::Outcome::Screenshot { path } => {
                self.shot_token += 1;
                let token = self.shot_token;
                self.queued_shots.push((token, now_ms() + 120.0, 0));
                self.pending_shots.push((token, path, reply, now_ms() + 8000.0));
            }
        }
        ctx.request_repaint();
    }

    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        // Wait for a background rebuild for the current view, so the shot shows the drawing at
        // that view, but not past the reply's deadline.
        let building = self.canvas.building();
        self.queued_shots.retain_mut(|(token, at, frames)| {
            *frames += 1;
            if now >= *at && *frames >= 3 && (!building || now >= *at + SHOT_REBUILD_WAIT_MS) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(*token)));
                false
            } else {
                true
            }
        });
        if !self.queued_shots.is_empty() || !self.pending_shots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_shots.is_empty() {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            if let Some(i) = self.pending_shots.iter().position(|(t, ..)| *t == token) {
                let (_, path, reply, _) = self.pending_shots.remove(i);
                let _ = reply.send(control::save_screenshot(&image, path.as_deref()));
            }
        }
        let now = now_ms();
        self.pending_shots.retain(|(_, _, reply, deadline)| {
            if now < *deadline {
                return true;
            }
            let _ = reply.send(json!({"ok": false, "error": "no frame was presented (screen locked or window hidden); use ui.render"}));
            false
        });
    }
}

/// How long a control-channel screenshot waits for a background rebuild before it is taken
/// anyway (the reply itself times out after 8 s).
const SHOT_REBUILD_WAIT_MS: f64 = 6000.0;

pub fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}

/// A LISP source file, by its name.
fn is_lisp_file(name: &str) -> bool {
    std::path::Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case("lsp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::{ThemePref, Tokens};

    fn app() -> CadApp {
        CadApp::new(Session::empty(), Services::default())
    }

    /// One frame of `logic` with the OS reporting `system` as its appearance.
    fn frame(app: &mut CadApp, ctx: &egui::Context, system: Option<egui::Theme>) {
        let raw = egui::RawInput { system_theme: system, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| app.logic(ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn theme_choice_persists_and_drives_tokens() {
        let mut a = app();
        assert_eq!(a.ui.theme, ThemePref::Dark, "default keeps today's look");
        assert!(a.run("ui.theme", json!({ "theme": "Purple" })).is_err());
        assert_eq!(a.run("ui.theme", json!({ "theme": "light" })).ok(), Some(json!({ "theme": "light", "shown": "light" })));
        // Saved and restored by the host.
        let mut b = app();
        b.load_prefs(&a.prefs_json());
        assert_eq!(b.ui.theme, ThemePref::Light);
        b.load_prefs("not json");
        b.load_prefs(r#"{"theme": "sepia"}"#);
        assert_eq!(b.ui.theme, ThemePref::Light, "bad prefs are ignored");
        // A fixed choice ignores the OS; the tokens and egui follow the choice.
        let ctx = egui::Context::default();
        frame(&mut b, &ctx, Some(egui::Theme::Dark));
        assert_eq!(Tokens::get(), Tokens::LIGHT);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        // The menu items reach the same setting; so does `ui.set` (the UiState JSON).
        b.start("ui.theme.dark");
        frame(&mut b, &ctx, Some(egui::Theme::Light));
        assert_eq!(Tokens::get(), Tokens::DARK);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        let ui: UiState = serde_json::from_value(json!({ "theme": "system" })).unwrap_or_default();
        assert_eq!(ui.theme, ThemePref::System);
    }

    #[test]
    fn system_theme_follows_the_os_live_and_stays_saved_as_system() {
        let mut a = app();
        a.start("ui.theme.system");
        let ctx = egui::Context::default();
        frame(&mut a, &ctx, Some(egui::Theme::Light));
        assert_eq!(Tokens::get(), Tokens::LIGHT);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        frame(&mut a, &ctx, Some(egui::Theme::Dark));
        assert_eq!(Tokens::get(), Tokens::DARK);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        frame(&mut a, &ctx, None);
        assert_eq!(Tokens::get(), Tokens::of(theme::SYSTEM_FALLBACK), "no OS appearance: the documented fallback");
        assert_eq!(a.ui.theme, ThemePref::System);
        assert_eq!(
            a.prefs_json(),
            r#"{"animations":"system","interfaceLanguage":"auto","saveFormat":"cdy","saveFormats":2,"saveVersion":"2018","theme":"system"}"#
        );
    }

    #[test]
    fn system_theme_keeps_egui_on_the_system_preference() {
        let mut a = app();
        a.start("ui.theme.system");
        let ctx = egui::Context::default();
        frame(&mut a, &ctx, Some(egui::Theme::Light));
        // A concrete Light would pin the native window appearance and hide later OS changes.
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::System);
        a.start("ui.theme.dark");
        frame(&mut a, &ctx, Some(egui::Theme::Light));
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::Dark);
        a.start("ui.theme.system");
        frame(&mut a, &ctx, Some(egui::Theme::Light));
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::System);
    }

    #[test]
    fn default_save_format_persists_and_names_new_drawings() {
        let mut a = app();
        assert_eq!(a.ui.save_format, SaveFormat::Cdy, "new drawings default to .cdy, CADaddy's native format");
        assert_eq!(SaveFormat::Cdy.suggested_name(Some("Drawing1")), "Drawing1.cdy");
        assert!(a.run("ui.saveformat", json!({ "format": "pdf" })).is_err());
        assert_eq!(a.run("ui.saveformat", json!({ "format": "DWG" })).ok(), Some(json!({ "saveFormat": "dwg" })));
        assert_eq!(a.run("ui.saveformat", json!({})).ok(), Some(json!({ "saveFormat": "dwg" })), "no format: report it");
        // Saved and restored by the host; bad values are ignored.
        let mut b = app();
        b.load_prefs(&a.prefs_json());
        assert_eq!(b.ui.save_format, SaveFormat::Dwg);
        b.load_prefs(r#"{"saveFormat": "svg"}"#);
        assert_eq!(b.ui.save_format, SaveFormat::Dwg);
        // DXF in preferences from before .cdy was the old default, not a choice; a DXF chosen since
        // stays, and so does a DWG chosen before.
        b.load_prefs(r#"{"saveFormat": "dxf"}"#);
        assert_eq!(b.ui.save_format, SaveFormat::Cdy);
        b.load_prefs(r#"{"saveFormat": "dxf", "saveFormats": 2}"#);
        assert_eq!(b.ui.save_format, SaveFormat::Dxf);
        b.load_prefs(r#"{"saveFormat": "dwg"}"#);
        assert_eq!(b.ui.save_format, SaveFormat::Dwg);
        // The menu items reach the same setting, and resetting the palettes keeps it.
        b.start("ui.saveformat.dxf");
        assert_eq!(b.ui.save_format, SaveFormat::Dxf);
        b.start("ui.saveformat.cdy");
        assert_eq!(b.ui.save_format, SaveFormat::Cdy);
        b.start("ui.saveformat.dwg");
        b.start("ui.resetpalettes");
        assert_eq!(b.ui.save_format, SaveFormat::Dwg);
        // Untitled drawings get the format's extension; files keep their own.
        assert_eq!(SaveFormat::Dwg.suggested_name(Some("Drawing1")), "Drawing1.dwg");
        assert_eq!(SaveFormat::Dxf.suggested_name(Some("Drawing1")), "Drawing1.dxf");
        assert_eq!(SaveFormat::Dwg.suggested_name(Some("Floorplan.dxf")), "Floorplan.dxf");
        assert_eq!(SaveFormat::Dxf.suggested_name(Some("Floorplan.DWG")), "Floorplan.DWG");
        assert_eq!(SaveFormat::Dwg.suggested_name(None), "Drawing.dwg");
        assert_eq!(SaveFormat::Dwg.suggested_name(Some("  ")), "Drawing.dwg");
        let ui: UiState = serde_json::from_value(json!({ "saveFormat": "dwg" })).unwrap_or_default();
        assert_eq!(ui.save_format, SaveFormat::Dwg);
    }
}
