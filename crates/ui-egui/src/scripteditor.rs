//! The Script Editor palette (RHAIEDIT, Tools ▸ Rhai ▸ Script Editor): an editor pane, a Run
//! button, a Stop button (clears the output), and an output console.  Everything runs through the
//! `rhai` command.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "scripteditor";

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let code_id = egui::Id::new("rhai_editor_code");
    let output_id = egui::Id::new("rhai_editor_output");
    let running_id = egui::Id::new("rhai_editor_running");
    let mut code: String = ctx.data(|d| d.get_temp(code_id)).unwrap_or_else(|| {
        "// Rhai script\n// command(\"line\", #{ points: [[0.0, 0.0], [10.0, 10.0]] });\nprint(\"Hello from Rhai!\");\n".to_string()
    });
    let mut output: Vec<String> = ctx.data(|d| d.get_temp(output_id)).unwrap_or_default();
    let running: bool = ctx.data(|d| d.get_temp(running_id)).unwrap_or(false);

    egui::Window::new(crate::tl!("Script Editor"))
        .id(egui::Id::new("script_editor_palette"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([480.0, 520.0])
        .show(ctx, |ui| {
            // Toolbar.
            ui.horizontal(|ui| {
                let run_clicked = ui.add_enabled(!running, egui::Button::new(crate::tl!("Run"))).clicked();
                if ui.button(crate::tl!("Clear")).clicked() {
                    output.clear();
                }

                if run_clicked {
                    // Run the script through the rhai command.
                    let src = code.clone();
                    match app.run("rhai", json!({"code": src})) {
                        Ok(result) => {
                            if let Some(out) = result.get("output").and_then(Value::as_array) {
                                for line in out {
                                    if let Some(s) = line.as_str() {
                                        output.push(s.to_string());
                                    }
                                }
                            }
                            if let Some(err) = result.get("error").and_then(Value::as_str) {
                                output.push(format!("Error: {err}"));
                            } else if let Some(val) = result.get("value")
                                && !val.is_null()
                            {
                                output.push(format!("=> {val}"));
                            }
                        }
                        Err(e) => {
                            output.push(format!("Error: {e}"));
                        }
                    }
                }
            });

            ui.separator();

            // Code editor.
            ui.label(RichText::new(crate::tl!("Script")).strong());
            egui::ScrollArea::vertical().id_salt("rhai_code_scroll").max_height(240.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut code).code_editor().desired_width(f32::INFINITY).desired_rows(12).font(egui::TextStyle::Monospace),
                );
            });

            ui.separator();

            // Output console.
            ui.label(RichText::new(crate::tl!("Output")).strong());
            egui::ScrollArea::vertical().id_salt("rhai_output_scroll").max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &output {
                    ui.label(RichText::new(line).monospace().color(t.text));
                }
                if output.is_empty() {
                    ui.label(RichText::new(crate::tl!("(no output)")).color(t.text_dim));
                }
            });
        });

    ctx.data_mut(|d| {
        d.insert_temp(code_id, code);
        d.insert_temp(output_id, output);
        d.insert_temp(running_id, running);
    });
}
