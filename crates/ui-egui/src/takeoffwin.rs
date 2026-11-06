//! The Takeoff window (Automation ▸ Takeoff, Q6117): `automation.takeoff`'s table — the model's
//! takeoff per level, trade and type, or the 2D takeoff by layer when no model is built — with
//! Refresh and Export (CSV or XLSX, through the engine's output guard).

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// The ribbon's Takeoff button: the engine command, run from a click without parameters.
pub const COMMAND: &str = "automation.takeoff";
pub const DIALOG: &str = "takeoff.table";
const ROW_H: f32 = 20.0;
/// The widest a column is drawn, points.
const COL_MAX: f32 = 260.0;

/// TAKEOFF from the ribbon: runs the takeoff and shows its table.
pub fn run(app: &mut CadApp) -> Result<Value, String> {
    let r = app.run(COMMAND, json!({}));
    if let Ok(v) = &r {
        app.takeoff_table = Some(v.clone());
    }
    app.ui.dialog = Some(DIALOG.into());
    r.map(|v| json!({"source": v["source"], "rows": v["total_rows"], "dialog": DIALOG}))
}

fn format_id() -> egui::Id {
    egui::Id::new("takeoff window format")
}

fn cell(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Export the takeoff: pick a file, then the engine writes it (replacing the picked file).
fn export(app: &mut CadApp, format: &str) {
    let name = format!("Takeoff.{format}");
    let Some(path) = app.services.pick_save.as_ref().and_then(|p| p(&name)) else {
        app.set_status(crate::tf!("No file chosen. Type: automation.takeoff {params}", params = format!("{{\"path\": \"{name}\"}}")));
        return;
    };
    match app.run(COMMAND, json!({"path": path, "format": format, "force": true})) {
        Ok(v) => {
            let written = v["path"].as_str().unwrap_or_default().to_string();
            app.takeoff_table = Some(v);
            app.set_status(crate::tf!("Takeoff written to {path}", path = written));
        }
        Err(e) => app.set_status(e),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut format: String = ctx.data(|d| d.get_temp(format_id())).unwrap_or_else(|| "csv".to_string());
    let mut refresh = false;
    let mut save = false;
    let table = app.takeoff_table.clone();
    let title = table.as_ref().and_then(|v| v["title"].as_str()).map_or_else(|| crate::tl!("Takeoff").to_string(), |s| crate::i18n::t(s).to_string());
    egui::Window::new(title).id(egui::Id::new("takeoff_window")).default_size([720.0, 420.0]).resizable(true).open(open).show(ctx, |ui| {
        ui.horizontal(|ui| {
            let r = ui.button(crate::tl!("Refresh"));
            crate::control::record_widget(ui, &r, crate::tl!("Refresh"), "button", None);
            if r.clicked() {
                refresh = true;
            }
            ui.separator();
            egui::ComboBox::from_id_salt("takeoff format").selected_text(format.to_ascii_uppercase()).show_ui(ui, |ui| {
                for f in ["csv", "xlsx"] {
                    ui.selectable_value(&mut format, f.to_string(), f.to_ascii_uppercase());
                }
            });
            let r = ui.add_enabled(table.is_some(), egui::Button::new(crate::tl!("Export")));
            crate::control::record_widget(ui, &r, crate::tl!("Export"), "button", None);
            if r.clicked() {
                save = true;
            }
        });
        let Some(v) = &table else {
            ui.label(RichText::new(crate::tl!("No takeoff yet: Refresh runs it.")).color(t.text_dim));
            return;
        };
        let source = if v["source"] == "model" { crate::tl!("From the BIM model") } else { crate::tl!("From the 2D drawing") };
        let total = v["total_rows"].as_u64().unwrap_or(0);
        let shown = v["shown"].as_u64().unwrap_or(total);
        let mut line = crate::tf!("{source}: {n} lines", source = source, n = total);
        if shown < total {
            line.push_str(&crate::tf!(" ({n} shown; Export writes them all)", n = shown));
        }
        ui.label(RichText::new(line).color(t.text_dim));
        if let Some(note) = v["note"].as_str() {
            ui.label(RichText::new(note).small().color(t.text_dim));
        }
        ui.separator();
        let columns: Vec<String> = v["columns"].as_array().into_iter().flatten().map(cell).collect();
        let rows: Vec<Vec<String>> =
            v["rows"].as_array().into_iter().flatten().map(|r| r.as_array().into_iter().flatten().map(cell).collect()).collect();
        // Column widths from the longest text (header included), capped.
        let font = egui::TextStyle::Body.resolve(ui.style());
        let width = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), t.text).size().x;
        let widths: Vec<f32> = columns
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let w = rows.iter().take(400).filter_map(|r| r.get(i)).map(|c| width(c)).fold(width(crate::i18n::t(h)), f32::max);
                (w + 14.0).min(COL_MAX)
            })
            .collect();
        egui::ScrollArea::horizontal().id_salt("takeoff h").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (h, w) in columns.iter().zip(&widths) {
                    let h = crate::i18n::t(h);
                    let r = ui.add_sized([*w, ROW_H], egui::Label::new(RichText::new(h).strong()).truncate());
                    crate::control::record_widget(ui, &r, h, "text", None);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("takeoff v").auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                for r in rows.get(range).unwrap_or_default() {
                    ui.horizontal(|ui| {
                        for (c, w) in r.iter().zip(&widths) {
                            let r = ui.add_sized([*w, ROW_H], egui::Label::new(c.as_str()).truncate());
                            crate::control::record_widget(ui, &r, c, "text", None);
                        }
                    });
                }
            });
        });
    });
    ctx.data_mut(|d| d.insert_temp(format_id(), format.clone()));
    if refresh {
        let r = app.run(COMMAND, json!({}));
        match r {
            Ok(v) => app.takeoff_table = Some(v),
            Err(e) => app.set_status(e),
        }
    }
    if save {
        export(app, &format);
    }
}
