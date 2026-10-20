//! CADaddy desktop app.
//!
//! Usage: `cadaddy [--control <port>] [--sample] [files…]`
//!
//! `--control <port>` (or `CADADDY_CONTROL_PORT`) starts a localhost JSON-lines control server:
//! `{"id":1,"method":"cmdline.input","params":{"text":"circle 0,0 5"}}` → `{"id":1,"ok":true,…}`.
//! See `cadaddy_ui_egui::control` for the methods.
//!
//! `CADADDY_VSYNC=1` presents with vsync on Linux/BSD, where the default is the low-latency
//! present mode (`CADADDY_VSYNC=0` selects that elsewhere); see `cadaddy_ui_egui::gpu::surface_config`.
//!
//! `log` records go to standard error and `<settings dir>/logs/cadaddy.log` (`RUST_LOG` sets the
//! levels); see [`logging`] and [`config_dir`].
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod control_server;
mod desktop_theme;
mod graphics;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;
mod printing;
mod relsig;
mod spacemouse;
mod sysclip;
mod system_animations;
mod updater;
mod workspace;

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, i18n};

struct App {
    cad: CadApp,
    spacemouse: spacemouse::SpaceMouse,
    /// The desktop's light/dark choice where winit reports none (Linux, the BSDs).
    desktop_theme: desktop_theme::DesktopTheme,
    /// Whether the OS shows animations, for the System choice of View ▸ Accessibility ▸ Animation Effects.
    system_animations: system_animations::SystemAnimations,
    workspace: workspace::Workspace,
    workspace_error: Option<String>,
    /// Frames left to check that the first window fits its screen ([`too_big_for`]).
    fit_frames: u8,
    #[cfg(target_os = "macos")]
    native_menu: Option<native_menu::NativeMenu>,
}

impl App {
    /// The first window is 1600 × 1000 points; on a smaller screen (a 4K laptop at 250 %, a
    /// 1366 × 768 one) it reached past the screen, title bar buttons and all. It opens maximised
    /// there instead. The monitor's size comes in after a frame or two.
    fn fit_first_window(&mut self, ctx: &egui::Context) {
        if self.fit_frames == 0 {
            return;
        }
        self.fit_frames -= 1;
        let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
        if let (Some(outer), Some(monitor)) = (outer, monitor) {
            self.fit_frames = 0;
            if too_big_for(outer.size(), monitor) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
        } else {
            ctx.request_repaint();
        }
    }

    fn persist_workspace(&mut self, final_attempt: bool) {
        let result = if final_attempt { self.workspace.save_on_exit(&self.cad.ui) } else { self.workspace.save_if_changed(&self.cad.ui) };
        if let Err(error) = result {
            if self.workspace_error.as_ref() != Some(&error) {
                eprintln!("CADaddy workspace: {error}");
                self.cad.set_status(format!("Workspace could not be saved: {error}"));
                self.workspace_error = Some(error);
            }
        } else {
            self.workspace_error = None;
        }
    }
}

/// Whether a window of `window` points doesn't fit a monitor of `monitor` points (sizes that
/// aren't known yet, or aren't numbers, never count as too big).
fn too_big_for(window: egui::Vec2, monitor: egui::Vec2) -> bool {
    let known = |v: egui::Vec2| v.x.is_finite() && v.y.is_finite() && v.x > 0.0 && v.y > 0.0;
    known(window) && known(monitor) && (window.x > monitor.x || window.y > monitor.y)
}

/// View ▸ Theme ▸ System resolves against `raw.system_theme`, which winit leaves empty on Linux
/// and the BSDs: the desktop's answer, where it gives one, stands in for it.
fn follow_desktop(raw: &mut egui::RawInput, desktop: Option<egui::Theme>) {
    if desktop.is_some() {
        raw.system_theme = desktop;
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        {
            if self.native_menu.is_none() && std::env::var_os("CADADDY_NO_NATIVE_MENU").is_none() {
                self.native_menu = Some(native_menu::NativeMenu::install(&mut self.cad));
            }
            if let Some(m) = &mut self.native_menu {
                m.poll(&mut self.cad, ctx);
            }
        }
        self.cad.system_animations = self.system_animations.current();
        self.fit_first_window(ctx);
        self.cad.logic(ctx);
        self.spacemouse.poll(&mut self.cad, ctx);
        self.persist_workspace(false);
        if std::mem::take(&mut self.cad.quit_requested) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        follow_desktop(raw, self.desktop_theme.current());
        self.cad.raw_input_hook(raw);
        cadaddy_ui_egui::cmdline::capture_tab(ctx, raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.cad.ui(ui);
        self.persist_workspace(false);
    }
    /// A normal exit: the workspace is saved, and this run's autosave files are not needed (a
    /// crash leaves them for recovery).
    fn on_exit(&mut self) {
        self.persist_workspace(true);
        self.cad.shutdown_autosave();
        if let Some(u) = &self.cad.updater {
            u.on_exit();
        }
    }
    /// Preferences (the interface theme) survive restarts; window and egui state are not kept.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(cadaddy_ui_egui::options::PROFILE_KEY, self.cad.profile_json());
        storage.set_string(cadaddy_ui_egui::PREFS_KEY, self.cad.prefs_json());
        storage.set_string(cadaddy_ui_egui::autosave::PREFS_KEY, cadaddy_ui_egui::autosave::prefs_json());
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

/// File format hooks for the engine (it stays I/O-agnostic).
pub fn install_io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
    cadaddy_engine::cmd::pointcloud::set_reader(cadaddy_io::pointcloud_io::read);
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    cadaddy_engine::cmd::redline::set_opener(open_in_redline);
    cadaddy_engine::cmd::file::set_print(cadaddy_engine::cmd::file::PrintHooks { list: printing::list, print: printing::print });
    // .cdy saves append only what changed to the file a drawing was saved to or read from.
    cadaddy_engine::cmd::file::set_cdy_hooks(cadaddy_engine::cmd::file::CdyHooks {
        save: |d, prev| {
            use cadaddy_engine::cmd::file::CdyWrite;
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| cadaddy_io::cdy::preview(d))?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, std::sync::Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| std::sync::Arc::new(s) as cadaddy_engine::cmd::file::CdyState),
    });
    cadaddy_engine::cmd::file::set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
    cadaddy_engine::cmd::file::set_write_as(|d, name, o| {
        cadaddy_io::write_with(d, name, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    // AutoLISP's entget, entmake and entmod see entities as DXF groups.
    cadaddy_engine::cmd::file::set_entity_codec(cadaddy_engine::cmd::file::EntityCodec {
        groups: cadaddy_io::entity_groups,
        entity: cadaddy_io::entity_from_groups,
        records: cadaddy_io::entity_records,
        attrib: cadaddy_io::attrib_from_groups,
    });
}

fn services() -> Services {
    Services {
        pick_open: Some(Box::new(|| {
            if !cadaddy_ui_egui::file_dialogs_allowed() {
                return None;
            }
            rfd::FileDialog::new()
                .add_filter(i18n::t("Drawings (CDY, DWG, DXF)"), &["cdy", "dwg", "dxf", "dwt"])
                .add_filter(i18n::t("SVG drawings (SVG, SVGZ)"), &["svg", "svgz"])
                .add_filter(i18n::t("PDF (converted to CAD objects)"), &["pdf"])
                .add_filter(i18n::t("3D models (glTF, OBJ, STL)"), &["glb", "gltf", "obj", "stl"])
                .add_filter(i18n::t("Building models (IFC)"), &["ifc"])
                .add_filter(i18n::t("Walkthroughs (CDYWALK)"), &["cdywalk"])
                .add_filter(i18n::t("All files"), &["*"])
                .pick_file()
                .map(|p| p.to_string_lossy().to_string())
        })),
        read_clipboard: Some(Box::new(|| arboard::Clipboard::new().ok()?.get_text().ok())),
        system_clipboard: Some(Box::new(sysclip::HostClipboard)),
        // Only .cdy files carry a preview; reading one is cheap (the preview chunk alone).
        thumbnail: Some(Box::new(|path: &str| {
            if !path.to_ascii_lowercase().ends_with(".cdy") {
                return None;
            }
            cadaddy_io::cdy::thumbnail(&std::fs::read(path).ok()?)
        })),
        pick_pdf: Some(Box::new(|| {
            if !cadaddy_ui_egui::file_dialogs_allowed() {
                return None;
            }
            rfd::FileDialog::new().add_filter(i18n::t("PDF"), &["pdf"]).pick_file().map(|p| p.to_string_lossy().to_string())
        })),
        pick_image: Some(Box::new(|| {
            if !cadaddy_ui_egui::file_dialogs_allowed() {
                return None;
            }
            rfd::FileDialog::new()
                .add_filter(i18n::t("Images (PNG, JPEG)"), &["png", "jpg", "jpeg"])
                .pick_file()
                .map(|p| p.to_string_lossy().to_string())
        })),
        pick_save_image: Some(Box::new(|name: &str| {
            rfd::FileDialog::new()
                .set_file_name(name)
                .add_filter(i18n::t("PNG image"), &["png"])
                .add_filter(i18n::t("JPEG image"), &["jpg", "jpeg"])
                .save_file()
                .map(|p| p.to_string_lossy().to_string())
        })),
        pick_folder: Some(Box::new(|| {
            if !cadaddy_ui_egui::file_dialogs_allowed() {
                return None;
            }
            rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string())
        })),
        pick_save: Some(Box::new(|name: &str| {
            if !cadaddy_ui_egui::file_dialogs_allowed() {
                return None;
            }
            let (dir, file) = save_suggestion(name);
            let dialog = rfd::FileDialog::new().set_file_name(&file);
            let dialog = match dir {
                Some(dir) => dialog.set_directory(dir),
                None => dialog,
            };
            filters_for_save(&file)
                .iter()
                .fold(dialog, |d, (label, ext)| d.add_filter(i18n::t(label), &[*ext]))
                .save_file()
                .map(|p| p.to_string_lossy().to_string())
        })),
    }
}

/// File types for the Save dialog, the suggested name's type first: Windows adds the first
/// filter's extension, and macOS accepts only listed extensions, replacing any other with the
/// first one. DWG must be listed for a `.dwg` name to survive, and a drawing opened from DWG
/// then defaults to DWG; other names keep CADaddy's own format (.cdy) first.
fn filters_for_save(name: &str) -> Vec<(&'static str, &'static str)> {
    // A sheet set is no drawing: its own type alone (Windows adds `.json`, which the sheet set
    // command turns into `.sheetset.json`).
    if name.to_ascii_lowercase().ends_with(".sheetset.json") {
        return vec![("Sheet Set (.sheetset.json)", "json")];
    }
    let mut filters = vec![
        ("CADaddy Drawing (CDY)", "cdy"),
        ("Drawing (DXF)", "dxf"),
        ("Drawing (DWG)", "dwg"),
        ("Drawing Template (DWT)", "dwt"),
        ("SVG", "svg"),
        ("PNG", "png"),
        ("PDF", "pdf"),
    ];
    let lower = name.to_ascii_lowercase();
    // Exporting (an image or PDF suggested): the 3D model formats too.
    if [".svg", ".png", ".pdf"].iter().any(|e| lower.ends_with(e)) {
        filters.extend([
            ("3D model (glTF binary)", "glb"),
            ("3D model (glTF)", "gltf"),
            ("3D model (OBJ)", "obj"),
            ("3D model (STL)", "stl"),
            ("Building model (IFC4)", "ifc"),
            ("3D printing (3MF)", "3mf"),
        ]);
    }
    filters.sort_by_key(|(_, ext)| !lower.ends_with(&format!(".{ext}")));
    filters
}

/// A suggested name for the Save dialog split into the folder to open in and the file name: a
/// suggestion with a folder that exists (a sheet set's PDF next to the set) opens there; a bare
/// name opens wherever the dialog was last.
fn save_suggestion(name: &str) -> (Option<std::path::PathBuf>, String) {
    let path = std::path::Path::new(name);
    match (path.parent().filter(|d| !d.as_os_str().is_empty() && d.is_dir()), path.file_name()) {
        (Some(dir), Some(file)) => (Some(dir.to_path_buf()), file.to_string_lossy().into_owned()),
        _ => (None, name.to_string()),
    }
}

/// The per-user settings directory (it holds `logs/` and `autosave/`): `CADADDY_CONFIG_DIR` if set, else
/// `$XDG_CONFIG_HOME/cadaddy` or `~/.config/cadaddy` on Linux and the BSDs,
/// `~/Library/Application Support/CADaddy` on macOS, `%APPDATA%\CADaddy` on Windows.
fn config_dir() -> Option<std::path::PathBuf> {
    config_dir_from(|k| std::env::var_os(k))
}

/// [`config_dir`] with the environment passed in (`var` looks a variable up); shared with the
/// control-port clients that look for the token file there.
fn config_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    cadaddy_engine::control_auth::config_dir_from(var)
}

const APP_ID: &str = "dev.aicodeforge.cadaddy";

/// Where eframe keeps the preferences (`app.ron`): inside `CADADDY_CONFIG_DIR` when that is set, so
/// a test or smoke run with its own settings folder never reads or writes the user's; otherwise
/// eframe's own folder (`None`), where existing installs have theirs.
fn persistence_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    var("CADADDY_CONFIG_DIR").filter(|v| !v.is_empty()).map(|d| std::path::PathBuf::from(d).join("app-state"))
}

/// Stencil bits of the window's render pass (see `NativeOptions::stencil_buffer`).
const STENCIL_BITS: u8 = 8;

#[cfg(target_os = "macos")]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/cadaddy-macos-512.png");
#[cfg(not(target_os = "macos"))]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/cadaddy-256.png");

fn app_icon() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).map_err(|e| log::warn!("app icon: {e}")).ok()
}

fn version_string() -> String {
    let sha = option_env!("CADADDY_BUILD_SHA").unwrap_or("dev");
    format!("cadaddy {} ({sha})", env!("CARGO_PKG_VERSION"))
}

/// The control port from `--control <port>` or `CADADDY_CONTROL_PORT` (`source`); a value that is
/// not a port is logged and ignored instead of silently starting no control channel.
fn control_port_from(source: &str, value: Option<String>) -> Option<u16> {
    let value = value?;
    let port = value.trim().parse().ok();
    if port.is_none() {
        log::warn!("{source}: {value:?} is not a port number; the control channel is off");
    }
    port
}

/// `--control` followed by `value`: a bare `--control` (last argument) keeps the port already
/// read from `CADADDY_CONTROL_PORT` (`current`) instead of turning the control channel off.
fn control_flag(current: Option<u16>, value: Option<String>) -> Option<u16> {
    if value.is_some() {
        return control_port_from("--control", value);
    }
    match current {
        Some(port) => log::warn!("--control: no port given; using CADADDY_CONTROL_PORT ({port})"),
        None => log::warn!("--control: no port given; the control channel is off"),
    }
    current
}

/// Open a PDF in Redline (AICodeForge's free PDF editor) when it is installed: `Ok(false)` when it
/// isn't (SEND TO REDLINE then just saves the PDF).
fn open_in_redline(path: &str) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        // By its bundle identifier: fails (without opening anything else) when it isn't installed.
        let ok = std::process::Command::new("open")
            .args(["-b", "dev.aicodeforge.redline", path])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?
            .success();
        Ok(ok)
    }
    #[cfg(target_os = "windows")]
    {
        // Where its installer puts it for the current user.
        let Some(exe) = std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("Programs").join("Redline").join("Redline.exe"))
        else {
            return Ok(false);
        };
        if !exe.is_file() {
            return Ok(false);
        }
        std::process::Command::new(exe).arg(path).spawn().map(|_| true).map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        match std::process::Command::new("redline").arg(path).spawn() {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn main() -> eframe::Result {
    cadaddy_ui_egui::startup::begin();
    // First, so every start-up record is captured; see `logging`.
    let logger = logging::install();
    // lossy, so a value that is not Unicode is reported like any other bad port instead of ignored
    let mut control_port =
        control_port_from("CADADDY_CONTROL_PORT", std::env::var_os("CADADDY_CONTROL_PORT").map(|p| p.to_string_lossy().into_owned()));
    let mut files = Vec::new();
    let mut sample = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => control_port = control_flag(control_port, args.next()),
            "--sample" => sample = true,
            "--version" | "-V" => {
                println!("{}", version_string());
                return Ok(());
            }
            _ => files.push(a),
        }
    }
    // The log file lives under the settings directory; opened after the arguments, so `--version`
    // leaves no file behind. Records logged until now are written to it first.
    if let Some(logger) = logger {
        match config_dir().map(|d| logger.attach_dir(&d.join(logging::LOG_DIR))) {
            Some(Ok(path)) => log::info!("{} ({} {}), log file {}", version_string(), std::env::consts::OS, std::env::consts::ARCH, path.display()),
            Some(Err(e)) => log::warn!("no log file: {e}"),
            None => {
                logger.stderr_only();
                log::warn!("no log file: no settings directory (set CADADDY_CONFIG_DIR)");
            }
        }
    }
    install_io();
    cadaddy_ui_egui::startup::mark("logging and file formats");
    let surface = cadaddy_ui_egui::gpu::surface_config(std::env::var("CADADDY_VSYNC").ok().as_deref(), cfg!(all(unix, not(target_os = "macos"))));
    let make_options = |attempt: graphics::Attempt| {
        let mut options = eframe::NativeOptions {
            // The crosshair is drawn by the app: present without a queue on Linux/BSD, where X11
            // swapchains hold two frames back under vsync (`CADADDY_VSYNC=1` restores vsync).
            wgpu_options: eframe::egui_wgpu::WgpuConfiguration::default().with_surface_config(surface),
            viewport: egui::ViewportBuilder::default()
                .with_title("CADaddy")
                .with_inner_size([1600.0, 1000.0])
                .with_min_inner_size([900.0, 560.0])
                .with_drag_and_drop(true)
                .with_fullsize_content_view(true)
                .with_titlebar_shown(false)
                .with_title_shown(false)
                .with_app_id(APP_ID),
            persist_window: false,
            persistence_path: persistence_dir_from(|k| std::env::var_os(k)),
            // A stencil buffer clips the contents of clipped layout viewports on the GPU.
            stencil_buffer: STENCIL_BITS,
            ..Default::default()
        };
        if let Some(icon) = app_icon() {
            options.viewport = options.viewport.with_icon(icon);
        }
        // Before eframe creates the wgpu instance: default Windows to DirectX 12 with FXC (see graphics.rs).
        #[cfg(target_os = "windows")]
        graphics::configure(&mut options, eframe::wgpu::Backends::from_env(), eframe::wgpu::Dx12Compiler::from_env());
        // Command previews (COPY, MOVE...) of large selections are tessellated by egui, whose index
        // buffer outgrew wgpu's default 256 MiB cap; the validation error is fatal. Ask for what the
        // adapter actually supports.
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
            let base = std::sync::Arc::clone(&create.device_descriptor);
            create.device_descriptor = std::sync::Arc::new(move |adapter| {
                let mut d = base(adapter);
                d.required_limits.max_buffer_size = d.required_limits.max_buffer_size.max(adapter.limits().max_buffer_size);
                d
            });
        }
        // winit has no file drag-and-drop on Wayland (only on X11), so dropping files from the file
        // manager showed a "no" cursor. Run through XWayland when it's there; CADADDY_WAYLAND=1
        // keeps the native Wayland backend.
        #[cfg(all(unix, not(target_os = "macos")))]
        if std::env::var_os("DISPLAY").is_some() && std::env::var_os("CADADDY_WAYLAND").is_none() {
            options.event_loop_builder = Some(Box::new(|b| {
                use winit::platform::x11::EventLoopBuilderExtX11;
                b.with_x11();
            }));
        }
        graphics::apply(&mut options, attempt);
        options
    };
    // The window's graphics: the default, then the fallbacks (graphics.rs) when it can't start.
    let attempts = graphics::attempts(eframe::wgpu::Backends::from_env());
    let mut result = Ok(());
    for (n, attempt) in attempts.iter().copied().enumerate() {
        let options = make_options(attempt);
        let files = files.clone();
        let fallback = graphics::note(attempt);
        result = eframe::run_native(
            "CADaddy",
            options,
            Box::new(move |cc| {
                cadaddy_ui_egui::startup::mark("window and graphics");
                let mut app = CadApp::new(Session::empty(), services());
                // The first DWG save returns once written; its "not kept" check follows.
                app.session.background_checks = true;
                cadaddy_ui_egui::startup::mark("app state");
                if let Some(note) = fallback {
                    log::warn!("{note}");
                    app.set_status(note);
                }
                app.donate.enabled = true;
                if std::env::var_os("CADADDY_NO_UPDATES").is_none() {
                    app.updater = Some(Box::new(updater::DesktopUpdater::default()));
                }
                let (workspace, restored, workspace_error) = workspace::Workspace::load();
                cadaddy_ui_egui::startup::mark("workspace");
                if let Some(ui) = restored {
                    app.ui = ui;
                }
                if let Some(error) = &workspace_error {
                    eprintln!("CADaddy workspace: {error}");
                    app.set_status(error);
                }
                app.system_languages = std::env::var("CADADDY_LOCALE").map(|s| vec![s]).unwrap_or_else(|_| sys_locale::get_locales().collect());
                // The user profile (Options): drafting settings, selection sizes, drawing-area colours.
                if let Some(profile) = cc.storage.and_then(|s| s.get_string(cadaddy_ui_egui::options::PROFILE_KEY)) {
                    app.load_profile(&profile);
                }
                if let Some(prefs) = cc.storage.and_then(|s| s.get_string(cadaddy_ui_egui::PREFS_KEY)) {
                    app.load_prefs(&prefs);
                }
                if let Some(prefs) = cc.storage.and_then(|s| s.get_string(cadaddy_ui_egui::autosave::PREFS_KEY)) {
                    cadaddy_ui_egui::autosave::load_prefs(&prefs);
                }
                // Autosave files (SAVEFILEPATH) default to `<settings dir>/autosave`.
                if let Some(dir) = config_dir() {
                    app.enable_autosave(&dir.join("autosave"));
                    // Startup LISP (acad.lsp, acaddoc.lsp) is read from `<settings>/support`; the
                    // folder is made so it is there to put them in.
                    let support = dir.join("support");
                    if let Err(e) = std::fs::create_dir_all(&support) {
                        log::warn!("no LISP support folder {}: {e}", support.display());
                    }
                    app.session.set_lisp_support_dir(Some(support));
                    // Action macros (ACTSTOP) are kept in `<settings>/actions`.
                    app.session.set_action_dir(Some(dir.join("actions")));
                }
                app.integrated_titlebar = cfg!(target_os = "macos");
                if surface.present_mode == eframe::wgpu::PresentMode::AutoNoVsync {
                    app.frame_cap = Some(cadaddy_ui_egui::gpu::FrameCap::new(240));
                }
                if let Some(rs) = &cc.wgpu_render_state {
                    app.set_wgpu(rs, eframe::egui_wgpu::depth_format_from_bits(0, STENCIL_BITS));
                }
                // Help ▸ Report a Problem: what the bundle says about this computer, and the watchdog
                // that logs UI stalls.
                let gpu = cc.wgpu_render_state.as_ref().map(|rs| {
                    let i = rs.adapter.get_info();
                    format!("{} ({:?}, {:?}; driver {} {})", i.name, i.backend, i.device_type, i.driver, i.driver_info)
                });
                cadaddy_ui_egui::diag::set_host(cadaddy_ui_egui::diag::HostInfo {
                    version: version_string(),
                    gpu: gpu.unwrap_or_default(),
                    log_dir: config_dir().map(|d| d.join(logging::LOG_DIR)),
                });
                cadaddy_ui_egui::diag::start_watchdog();
                // An explicit --control port, and the agent port unless the user turned that off in
                // Options: both serve the same window.
                let ports: Vec<u16> = control_port.into_iter().chain(app.ui.agent_control.then_some(cadaddy_engine::AGENT_PORT)).collect();
                if !ports.is_empty() {
                    // A fresh token each start unless the user turned it off (Options ▸ System).
                    let token = app.ui.control_token.then(control_server::new_token);
                    if token == Some(None) {
                        log::error!("control server: no system randomness for a token; the control channel stays closed");
                    } else {
                        let rx = control_server::start_ports(&ports, cc.egui_ctx.clone(), token.flatten(), config_dir().as_deref());
                        app = app.with_control(rx, control_port.is_some());
                    }
                }
                if sample {
                    let _ = app.run("ui.sample", serde_json::json!({}));
                }
                for f in &files {
                    app.open_path(f);
                }
                if app.session.docs.is_empty() {
                    app.session.new_drawing_with_viewports(false);
                }
                cadaddy_ui_egui::startup::mark("drawings opened");
                let ctx = cc.egui_ctx.clone();
                let desktop_theme = desktop_theme::DesktopTheme::start(move || ctx.request_repaint());
                let ctx = cc.egui_ctx.clone();
                let system_animations = system_animations::SystemAnimations::start(move || ctx.request_repaint());
                cadaddy_ui_egui::startup::mark("app ready");
                Ok(Box::new(App {
                    desktop_theme,
                    system_animations,
                    cad: app,
                    spacemouse: spacemouse::SpaceMouse::start(cc.egui_ctx.clone()),
                    workspace,
                    workspace_error,
                    fit_frames: 30,
                    #[cfg(target_os = "macos")]
                    native_menu: None,
                }))
            }),
        );
        match &result {
            Err(e) if n + 1 < attempts.len() => log::warn!("graphics didn't start ({e}); trying {:?}", attempts.get(n + 1)),
            _ => break,
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        config_dir_from, control_flag, control_port_from, filters_for_save, follow_desktop, persistence_dir_from, save_suggestion, too_big_for,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn save_dialog_offers_dwg_with_the_drawing_format_first() {
        let first = |name: &str| filters_for_save(name)[0].1;
        assert_eq!(first("Sample Floor Plan.dwg"), "dwg");
        assert_eq!(first("FLOORPLAN.DWG"), "dwg");
        // DXF files keep DXF first; new drawings get CADaddy's own format.
        assert_eq!(first("Drawing1.dxf"), "dxf");
        assert_eq!(first("Drawing1.cdy"), "cdy");
        assert_eq!(first("Drawing1"), "cdy");
        assert_eq!(first("plan.pdf"), "pdf");
        assert_eq!(first("office.dwt"), "dwt");
        for name in ["a.dwg", "b.dxf", "c", "d.cdy"] {
            let exts: Vec<_> = filters_for_save(name).iter().map(|(_, e)| *e).collect();
            assert!(exts.contains(&"dwg") && exts.contains(&"dxf") && exts.contains(&"cdy"), "{name}");
        }
    }

    #[test]
    fn a_sheet_set_is_saved_as_a_sheet_set_not_a_drawing() {
        assert_eq!(filters_for_save("Sheets.sheetset.json"), vec![("Sheet Set (.sheetset.json)", "json")]);
        assert_eq!(filters_for_save("Riverside Annex.SHEETSET.JSON")[0].1, "json");
        assert!(!filters_for_save("Riverside Annex.sheetset.json").iter().any(|(_, e)| *e == "cdy"));
    }

    #[test]
    fn a_save_suggestion_with_a_folder_opens_in_that_folder() {
        let dir = std::env::temp_dir();
        let name = dir.join("Riverside Annex.pdf");
        let (d, f) = save_suggestion(&name.to_string_lossy());
        assert_eq!(d.as_deref(), Some(dir.as_path()));
        assert_eq!(f, "Riverside Annex.pdf");
        assert_eq!(save_suggestion("Drawing1.cdy"), (None, "Drawing1.cdy".to_string()));
        // A folder that doesn't exist: the bare suggestion as it was.
        let gone = dir.join("no-such-folder-q224").join("x.pdf");
        assert_eq!(save_suggestion(&gone.to_string_lossy()).0, None);
    }

    #[test]
    fn a_first_window_bigger_than_its_screen_is_maximised() {
        let v = egui::vec2;
        // 1600 × 1000 points with the frame, on a 3840 × 2400 screen at 250 % (1536 × 960 points).
        assert!(too_big_for(v(1613.0, 1036.0), v(1536.0, 960.0)));
        assert!(too_big_for(v(1613.0, 1036.0), v(1366.0, 768.0)));
        assert!(!too_big_for(v(1613.0, 1036.0), v(1920.0, 1080.0)));
        assert!(!too_big_for(v(1613.0, 1036.0), v(0.0, 0.0)));
        assert!(!too_big_for(v(f32::NAN, 1036.0), v(1536.0, 960.0)));
    }

    #[test]
    fn control_port_values_that_are_not_ports_are_ignored() {
        assert_eq!(control_port_from("--control", Some("7979".into())), Some(7979));
        assert_eq!(control_port_from("--control", Some(" 7979 ".into())), Some(7979));
        assert_eq!(control_port_from("--control", None), None);
        for bad in ["", "abc", "-1", "65536", "99999999999999999999", "\u{0}"] {
            assert_eq!(control_port_from("--control", Some(bad.into())), None, "{bad:?}");
        }
    }

    #[test]
    fn control_without_a_port_keeps_the_environment_port() {
        // `CADADDY_CONTROL_PORT=47321 cadaddy --control` (#844)
        assert_eq!(control_flag(Some(47321), None), Some(47321));
        assert_eq!(control_flag(None, None), None);
        // A port given on the command line still wins over the variable.
        assert_eq!(control_flag(Some(47321), Some("7979".into())), Some(7979));
        assert_eq!(control_flag(Some(47321), Some("abc".into())), None);
    }

    #[test]
    fn the_override_variable_wins_and_empty_values_are_unset() {
        let vars = [("CADADDY_CONFIG_DIR", "/tmp/cc"), ("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x"), ("APPDATA", "C:\\a")];
        assert_eq!(config_dir_from(env(&vars)), Some(PathBuf::from("/tmp/cc")));
        let empty = [("CADADDY_CONFIG_DIR", ""), ("HOME", ""), ("XDG_CONFIG_HOME", ""), ("APPDATA", "")];
        assert_eq!(config_dir_from(env(&empty)), None, "no usable variable means no directory, not the working directory");
        assert_eq!(config_dir_from(env(&[])), None);
    }

    #[test]
    fn preferences_follow_the_override_variable_only() {
        // A run with its own settings folder keeps app.ron there, away from the user's.
        let p = persistence_dir_from(env(&[("CADADDY_CONFIG_DIR", "/tmp/cc"), ("HOME", "/home/u")]));
        assert_eq!(p, Some(PathBuf::from("/tmp/cc/app-state")));
        // Otherwise eframe's own folder, where installs already have their preferences.
        assert_eq!(persistence_dir_from(env(&[("HOME", "/home/u")])), None);
        assert_eq!(persistence_dir_from(env(&[("CADADDY_CONFIG_DIR", "")])), None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_and_bsd_follow_xdg_config_home() {
        assert_eq!(config_dir_from(env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/u")])), Some(PathBuf::from("/x/cadaddy")));
        assert_eq!(config_dir_from(env(&[("HOME", "/home/u")])), Some(PathBuf::from("/home/u/.config/cadaddy")));
        // The XDG spec says a relative XDG_CONFIG_HOME is ignored.
        assert_eq!(config_dir_from(env(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/u")])), Some(PathBuf::from("/home/u/.config/cadaddy")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_application_support() {
        assert_eq!(config_dir_from(env(&[("HOME", "/Users/u")])), Some(PathBuf::from("/Users/u/Library/Application Support/CADaddy")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_appdata() {
        assert_eq!(
            config_dir_from(env(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")])),
            Some(PathBuf::from("C:\\Users\\u\\AppData\\Roaming").join("CADaddy"))
        );
    }

    /// View ▸ Theme ▸ System on Linux: winit reports no theme, so the desktop's answer stands in;
    /// without one (or where winit answers and the desktop doesn't) the input is left alone.
    #[test]
    fn the_desktop_answers_for_the_system_theme() {
        let mut raw = egui::RawInput::default();
        follow_desktop(&mut raw, Some(egui::Theme::Light));
        assert_eq!(raw.system_theme, Some(egui::Theme::Light));
        let mut raw = egui::RawInput { system_theme: Some(egui::Theme::Dark), ..Default::default() };
        follow_desktop(&mut raw, None);
        assert_eq!(raw.system_theme, Some(egui::Theme::Dark));
        let mut raw = egui::RawInput::default();
        follow_desktop(&mut raw, None);
        assert_eq!(raw.system_theme, None);
    }
}
