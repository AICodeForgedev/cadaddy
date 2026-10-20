//! The eTransmit dialog (ETRANSMIT from the File menu): the files the package will hold as a
//! tree by folder, what is left out and why, the layout (folders or one folder) and the
//! transmittal report, then Create ZIP or Create Folder. It runs the ETRANSMIT command.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

#[derive(Clone, Default)]
struct State {
    flat: bool,
    no_report: bool,
    /// The listing for the current options (ETRANSMIT without a path), or why it failed.
    listing: Option<Result<Value, String>>,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-etransmit-dialog")
}

fn params(st: &State) -> Value {
    json!({ "layout": if st.flat { "flat" } else { "folders" }, "report": !st.no_report })
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1u32 << 20)),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    if st.listing.is_none() {
        st.listing = Some(app.session.execute("etransmit", &params(&st)).map_err(|e| e.user_message()));
    }
    let title = app.session.state().map(|s| s.title.clone()).unwrap_or_default();
    let stem = std::path::Path::new(&title)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Drawing".into());
    let mut create: Option<bool> = None;
    let mut refresh = false;
    egui::Window::new(crate::tl!("Create Transmittal"))
        .id(egui::Id::new("Create Transmittal"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_width(520.0)
        .show(ctx, |ui| {
            match &st.listing {
                Some(Ok(v)) => {
                    let files = v["files"].as_array().cloned().unwrap_or_default();
                    ui.label(RichText::new(crate::tl!("Files in the package")).strong());
                    egui::ScrollArea::vertical().id_salt("etransmit-files").max_height(260.0).show(ui, |ui| {
                        // A tree by folder: the drawing and reports at the top, then each folder.
                        let mut folders: Vec<(String, Vec<&Value>)> = Vec::new();
                        for f in &files {
                            let target = f["target"].as_str().unwrap_or_default();
                            let dir = target.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                            match folders.iter_mut().find(|(d, _)| *d == dir) {
                                Some((_, v)) => v.push(f),
                                None => folders.push((dir, vec![f])),
                            }
                        }
                        for (dir, fs) in &folders {
                            let row = |ui: &mut egui::Ui, f: &Value| {
                                let target = f["target"].as_str().unwrap_or_default();
                                let name = target.rsplit('/').next().unwrap_or(target);
                                ui.horizontal(|ui| {
                                    ui.label(name);
                                    ui.label(RichText::new(crate::i18n::t(f["kind"].as_str().unwrap_or_default())).small().color(t.text_dim));
                                    ui.label(RichText::new(human(f["bytes"].as_u64().unwrap_or(0))).small().color(t.text_dim));
                                })
                                .response
                                .on_hover_text(f["source"].as_str().unwrap_or_default());
                            };
                            if dir.is_empty() {
                                for f in fs {
                                    row(ui, f);
                                }
                            } else {
                                egui::CollapsingHeader::new(format!("📁 {dir}")).id_salt(("etransmit-dir", dir)).default_open(true).show(ui, |ui| {
                                    for f in fs {
                                        row(ui, f);
                                    }
                                });
                            }
                        }
                    });
                    let left = v["left"].as_array().cloned().unwrap_or_default();
                    if !left.is_empty() {
                        ui.separator();
                        ui.label(RichText::new(crate::tl!("Not included")).strong());
                        egui::ScrollArea::vertical().id_salt("etransmit-left").max_height(120.0).show(ui, |ui| {
                            for l in &left {
                                ui.label(
                                    RichText::new(format!(
                                        "{}: {} — {}",
                                        crate::i18n::t(l["kind"].as_str().unwrap_or_default()),
                                        l["name"].as_str().unwrap_or_default(),
                                        l["note"].as_str().unwrap_or_default()
                                    ))
                                    .small()
                                    .color(t.text_dim),
                                );
                            }
                        });
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(t.warn));
                }
                None => {}
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Layout"));
                refresh |= ui.radio_value(&mut st.flat, false, crate::tl!("Organized folders")).changed();
                refresh |= ui.radio_value(&mut st.flat, true, crate::tl!("One folder")).changed();
            });
            let mut report = !st.no_report;
            if ui.checkbox(&mut report, crate::tl!("Include a transmittal report")).changed() {
                st.no_report = !report;
                refresh = true;
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = matches!(st.listing, Some(Ok(_)));
                if ui.add_enabled(ready, egui::Button::new(crate::tl!("Create ZIP..."))).clicked() {
                    create = Some(true);
                }
                if ui.add_enabled(ready, egui::Button::new(crate::tl!("Create Folder..."))).clicked() {
                    create = Some(false);
                }
            });
        });
    if refresh {
        st.listing = None;
    }
    if let Some(zip) = create {
        let name = if zip { format!("{stem}.zip") } else { format!("{stem} transmittal") };
        match app.services.pick_save.as_ref().map(|pick| pick(&name)) {
            Some(Some(path)) => {
                let mut p = params(&st);
                p["path"] = json!(path);
                if let Ok(r) = app.run("etransmit", p) {
                    let n = r["files"].as_array().map_or(0, Vec::len);
                    app.session.echo(format!("eTransmit: {n} files in {}.", r["path"].as_str().unwrap_or_default()));
                    *open = false;
                }
            }
            Some(None) => {}
            None => app.session.echo("ETRANSMIT: there is no file dialog here; run it with parameters, e.g. etransmit {\"path\": \"package.zip\"}"),
        }
    }
    // Closed: the next opening lists the drawing afresh.
    if *open {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_opens_the_dialog_and_it_lists_the_package() {
        // Hooks are installed once per test process: whichever test comes first wins.
        cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
            read: |_, _| Err("no reader in tests".into()),
            write: |_, _| Ok(b"drawing".to_vec()),
            plot: None,
            read_pdf: None,
        });
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("line", json!({ "points": [[0, 0], [1, 1]] })).unwrap();
        crate::menus::run_ui_command(&mut app, "etransmit", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some("etransmit"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
        match st.listing {
            Some(Ok(v)) => assert_eq!(v["files"].as_array().map_or(0, Vec::len), 3, "the drawing and the two reports"),
            // Another test's hooks (a writer that fails): the dialog shows why.
            Some(Err(e)) => assert!(!e.is_empty()),
            None => panic!("no listing"),
        }
    }
}
