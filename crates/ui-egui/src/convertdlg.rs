//! The Convert Drawings window (DWGCONVERT, File ▸ Convert Drawings… and the Start page): files
//! and folders to convert, the format (.dwg at a version, .cdy, .dxf), where the files go and what
//! to do with ones already there; Convert runs the engine's batch ([`convert::run`], the same one
//! `cadaddy-cli convert` and the JSON command use) on a thread of its own, listing each drawing as
//! it is done, with Cancel (stops after the current drawing) and the log at the end.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cadaddy_engine::cmd::file::Version;
use cadaddy_engine::cmd::file::convert::{self, Exists, Outcome, Status};
use egui::{RichText, vec2};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "dwgconvert";

/// Output formats, as listed.
const FORMATS: [(&str, &str); 3] = [("dwg", "DWG drawing (.dwg)"), ("cdy", "CADaddy drawing (.cdy)"), ("dxf", "DXF drawing (.dxf)")];

/// What to convert: a file, or a folder's drawings.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    File(PathBuf),
    Folder(PathBuf),
}

/// A conversion running (or done): shared with its thread.
#[derive(Default)]
struct Running {
    cancel: AtomicBool,
    done: AtomicBool,
    total: Mutex<usize>,
    outcomes: Mutex<Vec<Outcome>>,
    summary: Mutex<Option<(convert::Summary, Option<PathBuf>)>>,
}

/// The window's choices.
#[derive(Clone)]
struct State {
    sources: Vec<Source>,
    recurse: bool,
    format: usize,
    /// DWG/DXF version (`None`: each drawing's own).
    version: Option<Version>,
    /// Output folder; empty: next to each original.
    out: String,
    next_to: bool,
    exists: Exists,
    bak: bool,
    typed: String,
    running: Option<Arc<Running>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            recurse: true,
            format: 0,
            version: None,
            out: String::new(),
            next_to: true,
            exists: Exists::Skip,
            bak: true,
            typed: String::new(),
            running: None,
        }
    }
}

impl State {
    fn job(&self) -> Result<convert::Job, String> {
        let mut inputs = Vec::new();
        for s in &self.sources {
            match s {
                Source::File(p) => inputs.push(convert::Input { path: p.clone(), sub: PathBuf::new() }),
                Source::Folder(p) => inputs.extend(convert::folder_inputs(p, self.recurse)?),
            }
        }
        if inputs.is_empty() {
            return Err(crate::i18n::t("There are no drawings (.dwg, .dxf, .cdy) to convert.").to_string());
        }
        inputs.truncate(convert::MAX_FILES);
        let to = FORMATS.get(self.format).map_or("dwg", |f| f.0).to_string();
        let out = (!self.next_to && !self.out.trim().is_empty()).then(|| PathBuf::from(self.out.trim()));
        if !self.next_to && out.is_none() {
            return Err(crate::i18n::t("Choose the output folder, or Next to the originals.").to_string());
        }
        Ok(convert::Job { inputs, version: if to == "cdy" { None } else { self.version }, to, out, exists: self.exists, bak: self.bak })
    }

    fn busy(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.done.load(Ordering::Relaxed))
    }
}

fn state_id() -> egui::Id {
    egui::Id::new("dwgconvert state")
}

/// Start converting on a thread of its own.
fn start(job: convert::Job) -> Arc<Running> {
    let running = Arc::new(Running::default());
    if let Ok(mut t) = running.total.lock() {
        *t = job.inputs.len();
    }
    let r = Arc::clone(&running);
    let work = move || {
        let sum = convert::run(&job, None, &r.cancel, &mut |_, o| {
            if let Ok(mut v) = r.outcomes.lock() {
                v.push(o.clone());
            }
        });
        let log = convert::write_log(&sum, job.out.as_deref());
        if let Ok(mut s) = r.summary.lock() {
            *s = Some((sum, log));
        }
        r.done.store(true, Ordering::Relaxed);
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let r = Arc::clone(&running);
        if std::thread::Builder::new().name("dwgconvert".into()).spawn(work).is_err() {
            r.done.store(true, Ordering::Relaxed);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = work;
        running.done.store(true, Ordering::Relaxed);
    }
    running
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut st: State = ctx.data_mut(|d| d.get_temp(state_id())).unwrap_or_default();
    let busy = st.busy();
    let mut close = false;
    let mut error: Option<String> = None;
    egui::Window::new(crate::tl!("Convert Drawings"))
        .id(egui::Id::new("dwgconvert window"))
        .open(open)
        .collapsible(false)
        .default_size(vec2(620.0, 520.0))
        .max_width(760.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            if cfg!(target_arch = "wasm32") {
                ui.label(crate::tl!("Converting files is not available on the web."));
                return;
            }
            ui.add_enabled_ui(!busy, |ui| {
                ui.label(RichText::new(crate::tl!("Drawings to convert")).strong());
                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Add Files...")).clicked()
                        && let Some(p) = app.services.pick_open.as_ref().and_then(|p| p())
                    {
                        add(&mut st, Source::File(p.into()));
                    }
                    if ui.button(crate::tl!("Add Folder...")).clicked()
                        && let Some(p) = app.services.pick_folder.as_ref().and_then(|p| p())
                    {
                        add(&mut st, Source::Folder(p.into()));
                    }
                    ui.checkbox(&mut st.recurse, crate::tl!("Include subfolders"));
                    if ui.add_enabled(!st.sources.is_empty(), egui::Button::new(crate::tl!("Clear"))).clicked() {
                        st.sources.clear();
                    }
                });
                // A path typed or pasted (a file or a folder).
                ui.horizontal(|ui| {
                    let r =
                        ui.add(egui::TextEdit::singleline(&mut st.typed).hint_text(crate::tl!("Or type a file or folder path")).desired_width(440.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button(crate::tl!("Add")).clicked() || enter) && !st.typed.trim().is_empty() {
                        let p = PathBuf::from(st.typed.trim());
                        add(&mut st, if p.is_dir() { Source::Folder(p) } else { Source::File(p) });
                        st.typed.clear();
                    }
                });
                let mut remove = None;
                egui::ScrollArea::vertical().id_salt("dwgconvert sources").max_height(110.0).show(ui, |ui| {
                    if st.sources.is_empty() {
                        ui.label(RichText::new(crate::tl!("Add drawings or a folder of them.")).color(t.text_dim));
                    }
                    for (i, s) in st.sources.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button(crate::tl!("Remove")).clicked() {
                                remove = Some(i);
                            }
                            match s {
                                Source::File(p) => ui.add(egui::Label::new(p.display().to_string()).truncate()),
                                Source::Folder(p) => ui.add(egui::Label::new(crate::tf!("{folder} (folder)", folder = p.display())).truncate()),
                            };
                        });
                    }
                });
                if let Some(i) = remove {
                    st.sources.remove(i);
                }
                ui.separator();
                egui::Grid::new("dwgconvert options").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                    ui.label(crate::tl!("Convert to"));
                    egui::ComboBox::from_id_salt("dwgconvert format")
                        .selected_text(crate::i18n::t(FORMATS.get(st.format).map_or("", |f| f.1)))
                        .show_ui(ui, |ui| {
                            for (i, f) in FORMATS.iter().enumerate() {
                                ui.selectable_value(&mut st.format, i, crate::i18n::t(f.1));
                            }
                        });
                    ui.end_row();
                    if FORMATS.get(st.format).is_some_and(|f| f.0 != "cdy") {
                        ui.label(crate::tl!("Version"));
                        let shown = st.version.map_or_else(|| crate::i18n::t("Each drawing's own"), |v| v.year());
                        egui::ComboBox::from_id_salt("dwgconvert version").selected_text(shown).show_ui(ui, |ui| {
                            ui.selectable_value(&mut st.version, None, crate::tl!("Each drawing's own"));
                            for v in Version::ALL.iter().rev() {
                                ui.selectable_value(&mut st.version, Some(*v), v.year());
                            }
                        });
                        ui.end_row();
                    }
                    ui.label(crate::tl!("Save to"));
                    ui.vertical(|ui| {
                        ui.radio_value(&mut st.next_to, true, crate::tl!("Next to the originals"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut st.next_to, false, crate::tl!("Folder:"));
                            let r = ui.add_enabled(!st.next_to, egui::TextEdit::singleline(&mut st.out).desired_width(300.0));
                            if r.gained_focus() {
                                st.next_to = false;
                            }
                            if ui.button(crate::tl!("Browse...")).clicked()
                                && let Some(p) = app.services.pick_folder.as_ref().and_then(|p| p())
                            {
                                st.out = p;
                                st.next_to = false;
                            }
                        });
                    });
                    ui.end_row();
                    ui.label(crate::tl!("Files already there"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut st.exists, Exists::Skip, crate::tl!("Skip"));
                        ui.radio_value(&mut st.exists, Exists::Overwrite, crate::tl!("Overwrite"));
                        ui.radio_value(&mut st.exists, Exists::Rename, crate::tl!("Rename"));
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut st.bak, crate::tl!("Keep a file written over as .bak"));
                    ui.end_row();
                });
            });
            ui.separator();
            progress(ui, &st, &t);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if busy {
                    let cancelling = st.running.as_ref().is_some_and(|r| r.cancel.load(Ordering::Relaxed));
                    if ui.add_enabled(!cancelling, egui::Button::new(crate::tl!("Cancel"))).clicked()
                        && let Some(r) = &st.running
                    {
                        r.cancel.store(true, Ordering::Relaxed);
                    }
                    ui.spinner();
                } else {
                    if ui.button(RichText::new(crate::tl!("Convert")).strong()).clicked() {
                        match st.job() {
                            Ok(job) => st.running = Some(start(job)),
                            Err(e) => error = Some(e),
                        }
                    }
                    let log = st.running.as_ref().and_then(|r| r.summary.lock().ok().and_then(|s| s.as_ref().and_then(|s| s.1.clone())));
                    if let Some(log) = log
                        && ui.button(crate::tl!("Open Log")).on_hover_text(log.display().to_string()).clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file://{}", log.display())));
                    }
                    if ui.button(crate::tl!("Close")).clicked() {
                        close = true;
                    }
                }
            });
        });
    if let Some(e) = error {
        app.session.echo(e);
    }
    if busy {
        // Keep the list moving while the thread works.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if close {
        *open = false;
    }
    // A conversion keeps running (and its list) when the window closes; otherwise closing resets it.
    if !*open && !st.busy() {
        st.running = None;
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

fn add(st: &mut State, s: Source) {
    if !st.sources.contains(&s) {
        st.sources.push(s);
    }
    // A finished list is replaced by the next conversion's.
    if !st.busy() {
        st.running = None;
    }
}

/// The per-drawing list and the totals.
fn progress(ui: &mut egui::Ui, st: &State, t: &Tokens) {
    let Some(r) = &st.running else { return };
    let outcomes = r.outcomes.lock().map(|v| v.clone()).unwrap_or_default();
    let total = r.total.lock().map(|t| *t).unwrap_or_default();
    let done = r.done.load(Ordering::Relaxed);
    let frac = if total == 0 { 1.0 } else { outcomes.len() as f32 / total as f32 };
    ui.add(egui::ProgressBar::new(frac).show_percentage().desired_width(ui.available_width()));
    egui::ScrollArea::vertical().id_salt("dwgconvert results").max_height(170.0).stick_to_bottom(true).show(ui, |ui| {
        egui::Grid::new("dwgconvert results grid").striped(true).num_columns(3).show(ui, |ui| {
            for o in &outcomes {
                let (word, color) = match o.status {
                    Status::Converted if o.report.losses.is_empty() => (crate::tl!("Converted"), t.text),
                    Status::Converted => (crate::tl!("Converted, not all kept"), t.text),
                    Status::Skipped => (crate::tl!("Skipped"), t.text_dim),
                    Status::Failed => (crate::tl!("Failed"), egui::Color32::from_rgb(230, 90, 80)),
                };
                ui.label(RichText::new(word).color(color));
                let name = std::path::Path::new(&o.from).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ui.label(name).on_hover_text(&o.from);
                let detail = match o.status {
                    Status::Converted if !o.report.losses.is_empty() => convert_losses(o),
                    Status::Converted => o.to.clone().unwrap_or_default(),
                    _ => o.reason.clone().unwrap_or_default(),
                };
                ui.add(egui::Label::new(RichText::new(detail).color(t.text_dim)).truncate()).on_hover_text(o.to.as_deref().unwrap_or_default());
                ui.end_row();
            }
        });
    });
    if done
        && let Ok(s) = r.summary.lock()
        && let Some((sum, _)) = s.as_ref()
    {
        let line = crate::tf!(
            "{converted} converted, {skipped} skipped, {failed} failed of {total}",
            converted = sum.count(Status::Converted),
            skipped = sum.count(Status::Skipped),
            failed = sum.count(Status::Failed),
            total = sum.total
        );
        let line = if sum.cancelled { format!("{line} — {}", crate::i18n::t("cancelled")) } else { line };
        ui.label(RichText::new(line).strong());
    }
}

fn convert_losses(o: &Outcome) -> String {
    o.report.losses.iter().map(|l| format!("{} {} {}", l.count, l.feature, l.what)).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use serde_json::{Value, json};

    #[test]
    fn the_menu_opens_the_window_and_it_draws() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        app.start("dwgconvert");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        let ctx = egui::Context::default();
        let mut open = true;
        ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut open)).textures_delta.clear();
        assert!(open);
        // With parameters it is the command itself (agents never get a window).
        app.ui.dialog = None;
        let r = app.run("dwgconvert", json!({ "files": ["/no/such.dwg"], "to": "cdy" })).unwrap();
        assert_eq!(r["failed"].as_array().map(Vec::len), Some(1));
        assert!(app.ui.dialog.is_none());
        let _: Value = r;
    }

    #[test]
    fn a_job_needs_drawings_and_a_folder_when_not_next_to_them() {
        let mut st = State::default();
        assert!(st.job().is_err());
        st.sources.push(Source::File("a.dwg".into()));
        st.format = 1;
        st.version = Some(Version::R2000);
        let job = st.job().unwrap();
        assert_eq!((job.to.as_str(), job.version, job.out), ("cdy", None, None), "a .cdy keeps everything: no version");
        st.next_to = false;
        assert!(st.job().is_err());
        st.out = "/tmp/out".into();
        assert_eq!(st.job().unwrap().out, Some(PathBuf::from("/tmp/out")));
    }

    #[test]
    fn convert_runs_off_the_ui_thread_and_lists_each_drawing() {
        let dir = std::env::temp_dir().join(format!("cadaddy-convertdlg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = State { sources: vec![Source::File(dir.join("missing.dwg"))], ..State::default() };
        let r = start(st.job().unwrap());
        for _ in 0..500 {
            if r.done.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(r.done.load(Ordering::Relaxed));
        let outcomes = r.outcomes.lock().unwrap().clone();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, Status::Failed);
        let s = r.summary.lock().unwrap();
        let (sum, log) = s.as_ref().unwrap();
        assert_eq!(sum.total, 1);
        if let Some(log) = log {
            let _ = std::fs::remove_file(log);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
