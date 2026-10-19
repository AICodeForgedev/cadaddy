//! The Rooms palette (typed ROOMS or Tools ▸ Building ▸ Rooms): the plan's rooms as spaces —
//! name, number, area, ceiling and volume — with Go to for each, and Build Volumes. It reads
//! `rooms.list` and acts through `rooms.goto` / `rooms.build`, as agents do.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "rooms";

/// The rooms last read, kept while the palette is open (Refresh reads them again).
fn list_id() -> egui::Id {
    egui::Id::new("rooms_palette_list")
}

fn read(app: &mut CadApp) -> Vec<Value> {
    app.run("rooms.list", json!({ "outline": false })).ok().and_then(|r| r["rooms"].as_array().cloned()).unwrap_or_default()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let cached: Option<Vec<Value>> = ctx.data(|d| d.get_temp(list_id()));
    let rooms = match cached {
        Some(r) => r,
        None => {
            let r = read(app);
            ctx.data_mut(|d| d.insert_temp(list_id(), r.clone()));
            r
        }
    };
    let metric = cadaddy_engine::sysvars::get(&app.session, "MEASUREMENT").and_then(|v| v.as_i64()) == Some(1);
    let mut action: Option<(&str, Value)> = None;
    let mut refresh = false;
    egui::Window::new(crate::tl!("Rooms")).id(egui::Id::new("Rooms palette")).open(open).default_size(vec2(520.0, 360.0)).collapsible(false).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tf!("{n} rooms", n = rooms.len()));
                if ui.button(crate::tl!("Refresh")).clicked() {
                    refresh = true;
                }
                if ui.button(crate::tl!("Build Volumes")).clicked() {
                    action = Some(("rooms.build", json!({})));
                }
            });
            ui.separator();
            if rooms.is_empty() {
                ui.label(RichText::new(crate::tl!("No rooms found: rooms are found from name labels inside closed outlines.")).color(t.text_dim));
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("rooms_grid").striped(true).num_columns(6).show(ui, |ui| {
                    for h in ["Name", "Number", "Area", "Ceiling", "Volume", ""] {
                        ui.label(RichText::new(crate::i18n::t(h)).strong());
                    }
                    ui.end_row();
                    for r in &rooms {
                        let name = r["name"].as_str().unwrap_or_default().to_string();
                        ui.label(&name);
                        ui.label(r["number"].as_str().unwrap_or("—"));
                        let num = |k: &str| r[k].as_f64().unwrap_or(0.0);
                        if metric {
                            ui.label(format!("{:.1} m²", num("areaM2")));
                            ui.label(format!("{:.2} m", num("ceilingFt") * 0.3048));
                            ui.label(format!("{:.1} m³", num("volumeM3")));
                        } else {
                            ui.label(format!("{:.0} sq ft", num("areaSqFt")));
                            ui.label(format!("{:.2}'", num("ceilingFt")));
                            ui.label(format!("{:.0} cu ft", num("volumeCuFt")));
                        }
                        if ui.button(crate::tl!("Go to")).clicked() {
                            action = Some(("rooms.goto", json!({ "name": name })));
                        }
                        ui.end_row();
                    }
                });
            });
        },
    );
    if let Some((id, p)) = action
        && let Err(e) = app.run(id, p)
    {
        app.session.echo(e);
    }
    if refresh || !*open {
        ctx.data_mut(|d| d.remove::<Vec<Value>>(list_id()));
    }
}
