//! The Drywall palette: wall inspector (click a wall to see its type, layers, ratings), view mode
//! controls (colour walls by type / fire rating / board layers / height rule, with a legend), and
//! a drywall takeoff button.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "drywallpalette";

#[derive(Clone, Default)]
struct State {
    viewmode: String,
    last_info: Option<Value>,
    last_info_handles: Vec<String>,
}

fn id() -> egui::Id {
    egui::Id::new("drywall_palette_state")
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();

    // If the selection changed, fetch wall info for the selected walls.
    let sel: Vec<String> = app.session.selection().iter().map(|h| h.hex()).collect();
    if sel != st.last_info_handles {
        st.last_info_handles = sel.clone();
        if !sel.is_empty() {
            st.last_info = app.session.execute("wall.info", &json!({"handles": sel})).ok();
        } else {
            st.last_info = None;
        }
    }

    egui::Window::new(crate::tl!("Drywall"))
        .id(egui::Id::new("drywall_palette"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([340.0, 500.0])
        .show(ctx, |ui| {
            // ---------- Wall inspector section ----------
            ui.heading(RichText::new(crate::tl!("Wall Inspector")).strong());
            ui.separator();

            if let Some(info) = &st.last_info {
                let walls = info["walls"].as_array();
                match walls {
                    Some(walls) if !walls.is_empty() => {
                        for w in walls {
                            wall_card(ui, w, &t);
                            ui.add_space(4.0);
                        }
                    }
                    _ => {
                        ui.label(RichText::new(crate::tl!("Select a wall to inspect it.")).color(t.text_dim));
                    }
                }
            } else {
                ui.label(RichText::new(crate::tl!("Select a wall to inspect it.")).color(t.text_dim));
            }

            ui.add_space(8.0);
            ui.separator();

            // ---------- View mode section ----------
            ui.heading(RichText::new(crate::tl!("View Mode")).strong());
            ui.add_space(4.0);
            let modes = [("off", "Off"), ("type", "By Type"), ("rating", "By Rating"),
                         ("boardLayers", "By Board Layers"), ("heightRule", "By Height Rule")];
            ui.horizontal_wrapped(|ui| {
                for (mode, label) in &modes {
                    let is_current = st.viewmode == *mode;
                    if ui.selectable_label(is_current, crate::i18n::t(label)).clicked() && !is_current {
                        st.viewmode = mode.to_string();
                        let _ = app.session.execute("drywall.viewmode", &json!({"mode": mode}));
                    }
                }
            });

            // Show the legend if a view mode is active.
            if let Some(vm) = &app.session.drywall_viewmode {
                ui.add_space(4.0);
                for (label, [r, g, b]) in &vm.legend {
                    ui.horizontal(|ui| {
                        let color = egui::Color32::from_rgb(*r, *g, *b);
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, color);
                        ui.label(label);
                    });
                }
            }

            ui.add_space(8.0);
            ui.separator();

            // ---------- Takeoff section ----------
            ui.heading(RichText::new(crate::tl!("Drywall Takeoff")).strong());
            ui.add_space(4.0);
            if ui.button(crate::tl!("Run Takeoff")).clicked()
                && let Ok(r) = app.session.execute("drywall.takeoff", &json!({}))
            {
                let total_area = r["totals"]["boardAreaSqFt"].as_f64().unwrap_or(0.0);
                let total_sheets = r["totals"]["sheets"].as_u64().unwrap_or(0);
                let stud_ft = r["totals"]["studLengthFt"].as_f64().unwrap_or(0.0);
                app.session.log.push(format!(
                    "Drywall takeoff: {total_area:.0} sq ft board, {total_sheets} sheets, {stud_ft:.0} ft studs"
                ));
            }
            if ui.button(crate::tl!("Export CSV...")).clicked() {
                let path = std::env::temp_dir().join("cadaddy-drywall-takeoff.csv");
                let path_str = path.to_string_lossy().to_string();
                match app.session.execute("drywall.takeoff", &json!({"file": path_str})) {
                    Ok(_) => app.session.log.push(format!("Takeoff saved to {path_str}")),
                    Err(e) => app.session.log.push(format!("Takeoff export failed: {e}")),
                }
            }

            // ---------- Finish faces section ----------
            ui.add_space(8.0);
            ui.separator();
            ui.heading(RichText::new(crate::tl!("Finish Faces")).strong());
            ui.add_space(4.0);
            if ui.button(crate::tl!("Show Finish Faces")).clicked() {
                match app.session.execute("drywall.finishfaces", &json!({})) {
                    Ok(r) => {
                        let count = r["count"].as_u64().unwrap_or(0);
                        app.session.log.push(format!("Finish faces: {count} faces found"));
                    }
                    Err(e) => app.session.log.push(format!("Finish faces failed: {e}")),
                }
            }
        });

    ctx.data_mut(|d| d.insert_temp(id(), st));
}

/// A card showing one wall's info in the inspector.
fn wall_card(ui: &mut egui::Ui, w: &Value, t: &crate::theme::Tokens) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        // Wall type and level.
        let type_name = w["wallType"].as_str().unwrap_or("(untyped)");
        let level = w["level"].as_str().unwrap_or("");
        ui.horizontal(|ui| {
            ui.label(RichText::new(type_name).strong());
            if !level.is_empty() {
                ui.label(RichText::new(format!("Level {level}")).color(t.text_dim));
            }
        });

        // Assembly details.
        if let Some(asm) = w.get("assembly") {
            if let Some(desc) = asm["description"].as_str() {
                ui.label(RichText::new(desc).small().color(t.text_dim));
            }

            // Ratings.
            ui.horizontal(|ui| {
                if let Some(fr) = asm["fireRating"].as_str() {
                    ui.label(RichText::new(format!("Fire: {fr}")).small());
                }
                if let Some(ar) = asm["acousticRating"].as_str() {
                    ui.label(RichText::new(format!("Sound: {ar}")).small());
                }
                if asm["external"].as_bool() == Some(true) {
                    ui.label(RichText::new("Exterior").small());
                }
            });

            // Height rule.
            if let Some(h) = asm["height"].as_str() {
                ui.label(RichText::new(format!("Height: {h}")).small());
            }

            // Thickness.
            if let Some(thick) = asm["thicknessIn"].as_f64() {
                ui.label(RichText::new(format!("Thickness: {thick:.3}\"")).small());
            }

            // Layers.
            if let Some(layers) = asm["layers"].as_array() {
                ui.add_space(2.0);
                ui.label(RichText::new("Layers:").small().strong());
                for layer in layers {
                    let mat = layer["material"].as_str().unwrap_or("");
                    let thick = layer["thicknessIn"].as_f64().unwrap_or(0.0);
                    let side = layer["side"].as_str().unwrap_or("core");
                    let in_cavity = layer["inCavity"].as_bool() == Some(true);
                    let suffix = if in_cavity { " (cavity)" } else { "" };
                    ui.label(RichText::new(format!("  {side}: {mat} ({thick:.3}\"){suffix}")).small());
                }
            }
        }

        // Gross area.
        if let Some(area) = w["grossAreaSqFt"].as_f64() {
            ui.label(RichText::new(format!("Area: {area:.1} sq ft")).small());
        }
    });
}
