//! The Script Editor palette (RHAIEDIT, Tools ▸ Rhai ▸ Script Editor): an editor pane, a Run
//! button, a Stop button, and an output console.  Run starts the script on a worker thread
//! (`Session::rhai_start`; its calls into the drawing run here, served every frame by
//! `CadApp::logic`), so the window keeps drawing and Stop ends a runaway script.  Where no thread
//! can start (the browser) Run goes through the `rhai` command, bounded by its time budget.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "scripteditor";

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let code_id = egui::Id::new("rhai_editor_code");
    let output_id = egui::Id::new("rhai_editor_output");
    let mut code: String = ctx.data(|d| d.get_temp(code_id)).unwrap_or_else(|| {
        "// Rhai script\n// command(\"line\", #{ points: [[0.0, 0.0], [10.0, 10.0]] });\nprint(\"Hello from Rhai!\");\n".to_string()
    });
    let mut output: Vec<String> = ctx.data(|d| d.get_temp(output_id)).unwrap_or_default();
    // The script started here ended: its output and result go to the console.
    if let Some(done) = app.session.rhai_take_finished() {
        let (lines, tail) = match done {
            Ok((val, out)) => (out, (!val.is_null()).then(|| format!("=> {val}"))),
            Err((e, out)) => (out, Some(format!("Error: {e}"))),
        };
        output.extend(lines);
        output.extend(tail);
    }
    let running = app.session.rhai_running();
    if running {
        // The worker's calls are served in `logic`, once a frame.
        ctx.request_repaint();
    }

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
                if ui.add_enabled(running, egui::Button::new(crate::tl!("Stop"))).clicked() {
                    app.session.rhai_stop();
                }
                if ui.button(crate::tl!("Clear")).clicked() {
                    output.clear();
                }
                if running {
                    ui.spinner();
                }

                if run_clicked {
                    match app.session.rhai_start(&code) {
                        Ok(()) => ctx.request_repaint(),
                        // No worker thread here: run it through the rhai command.
                        Err(_) if !app.session.rhai_running() => run_inline(app, &code, &mut output),
                        Err(e) => output.push(format!("Error: {e}")),
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
    });
}

/// Run the script through the `rhai` command (on this thread, within its time budget).
fn run_inline(app: &mut CadApp, code: &str, output: &mut Vec<String>) {
    match app.run("rhai", json!({ "code": code })) {
        Ok(result) => {
            if let Some(out) = result.get("output").and_then(Value::as_array) {
                output.extend(out.iter().filter_map(Value::as_str).map(str::to_string));
            }
            if let Some(err) = result.get("error").and_then(Value::as_str) {
                output.push(format!("Error: {err}"));
            } else if let Some(val) = result.get("value")
                && !val.is_null()
            {
                output.push(format!("=> {val}"));
            }
        }
        Err(e) => output.push(format!("Error: {e}")),
    }
}
