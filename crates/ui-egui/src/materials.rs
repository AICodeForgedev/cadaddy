//! The Materials Browser (MATERIALS, View ▸ Render ▸ Materials Browser…): CADaddy's material
//! library as swatches by category, the drawing's materials with their settings, assigning to the
//! selection and attaching to layers. Every change goes through the engine's material commands.

use cadaddy_engine::doc::material::{self, Material, Projection, Texture};
use egui::{RichText, vec2};
use serde_json::json;

use crate::CadApp;
use crate::theme::Tokens;

/// The dialog's name in `ui.dialog`.
pub const DIALOG: &str = "materials";
/// Swatch size, points.
const SWATCH: f32 = 56.0;
/// Texture size generated for a swatch, pixels.
const SWATCH_PX: u32 = 96;

/// What the browser has picked and is editing.
#[derive(Clone, Debug, Default)]
struct State {
    library: Option<String>,
    selected: Option<String>,
    edit: Option<Material>,
    layer: String,
    attach: String,
    filter: String,
}

/// A swatch texture: a library recipe's generated textures lit, an image file's pixels, or a flat
/// colour; made once and kept.
fn swatch(ctx: &egui::Context, m: &Material) -> Option<egui::TextureHandle> {
    let key = match &m.color_map {
        Some(Texture::Library(id)) => format!("lib:{id}"),
        Some(Texture::File(p)) => format!("file:{p}"),
        None => format!("color:{:02x}{:02x}{:02x}", m.color.0, m.color.1, m.color.2),
    };
    let id = egui::Id::new(("cc_material_swatch", key.clone()));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return t;
    }
    let n = SWATCH_PX as usize;
    let pixels: Option<Vec<u8>> = match &m.color_map {
        Some(Texture::Library(id)) => {
            material::recipe(id).map(|r| cadaddy_render::materials::swatch(&cadaddy_render::materials::generate(r, SWATCH_PX)))
        }
        Some(Texture::File(p)) => {
            image::open(p).ok().map(|img| img.resize_exact(SWATCH_PX, SWATCH_PX, image::imageops::FilterType::Triangle).to_rgba8().into_raw())
        }
        None => None,
    };
    let pixels = pixels.unwrap_or_else(|| {
        // A flat colour, shaded top-left to bottom-right so it reads as a surface.
        (0..n * n)
            .flat_map(|i| {
                let (x, y) = ((i % n) as f32 / n as f32, (i / n) as f32 / n as f32);
                let k = 1.1 - 0.25 * (x + y);
                let c = |v: u8| (f32::from(v) * k).clamp(0.0, 255.0) as u8;
                [c(m.color.0), c(m.color.1), c(m.color.2), 255]
            })
            .collect()
    });
    let tex = (pixels.len() == n * n * 4).then(|| {
        ctx.load_texture(format!("material:{key}"), egui::ColorImage::from_rgba_unmultiplied([n, n], &pixels), egui::TextureOptions::LINEAR)
    });
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

fn swatch_button(ui: &mut egui::Ui, ctx: &egui::Context, m: &Material, label: &str, selected: bool) -> egui::Response {
    let t = Tokens::get();
    let (rect, resp) = ui.allocate_exact_size(vec2(SWATCH + 26.0, SWATCH + 30.0), egui::Sense::click());
    let img = egui::Rect::from_min_size(rect.min + vec2(13.0, 2.0), vec2(SWATCH, SWATCH));
    let p = ui.painter();
    if selected || resp.hovered() {
        p.rect_filled(rect, 4.0, if selected { t.accent.gamma_multiply(0.35) } else { t.control_hover.gamma_multiply(0.5) });
    }
    match swatch(ctx, m) {
        Some(tex) => {
            p.image(tex.id(), img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        None => {
            p.rect_filled(img, 2.0, egui::Color32::from_rgb(m.color.0, m.color.1, m.color.2));
        }
    }
    let galley = p.layout(label.to_string(), crate::theme::small(), t.text, rect.width() - 4.0);
    p.galley(egui::pos2(rect.center().x - galley.size().x / 2.0, img.bottom() + 3.0), galley, t.text);
    resp.on_hover_text(label)
}

/// The Materials Browser.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("cc_materials_dialog");
    let mut st = ctx.data(|d| d.get_temp::<State>(id)).unwrap_or_default();
    let Ok(d) = app.session.doc() else { return };
    let drawing: Vec<Material> = d.materials.clone();
    let layers: Vec<String> = d.layers.iter().map(|l| l.name.clone()).collect();
    let layer_material = |name: &str| d.layer(name).and_then(|l| l.material.clone());
    if st.layer.is_empty() {
        st.layer = layers.first().cloned().unwrap_or_default();
        st.attach = layer_material(&st.layer).unwrap_or_else(|| material::GLOBAL.into());
    }
    let unit_m = material::metres_per_unit(d.header.i64("INSUNITS", 0), d.header.i64("MEASUREMENT", 0) == 1);
    let selection = app.session.selection().len();
    let mut actions: Vec<(&'static str, serde_json::Value)> = Vec::new();
    let center = ctx.content_rect().center();
    egui::Window::new(crate::tl!("Materials Browser"))
        .id(id.with("window"))
        .open(open)
        .default_size(vec2(760.0, 560.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(center)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Search"));
                ui.add(egui::TextEdit::singleline(&mut st.filter).desired_width(220.0));
            });
            ui.separator();
            ui.columns(2, |cols| {
                // Library, by category.
                cols[0].strong(crate::tl!("Library"));
                egui::ScrollArea::vertical().id_salt("cc_materials_library").max_height(330.0).show(&mut cols[0], |ui| {
                    let f = st.filter.trim().to_lowercase();
                    let mut categories: Vec<&str> = material::library().iter().map(|r| r.category.as_str()).collect();
                    categories.dedup();
                    for cat in categories {
                        let items: Vec<&material::Recipe> =
                            material::library().iter().filter(|r| r.category == cat && (f.is_empty() || r.name.to_lowercase().contains(&f) || cat.to_lowercase().contains(&f))).collect();
                        if items.is_empty() {
                            continue;
                        }
                        ui.label(RichText::new(cat).small().color(Tokens::get().text_dim));
                        ui.horizontal_wrapped(|ui| {
                            for r in items {
                                let m = r.material(unit_m);
                                let sel = st.library.as_deref() == Some(r.id.as_str());
                                let resp = swatch_button(ui, ctx, &m, &r.name, sel);
                                if resp.clicked() {
                                    st.library = Some(r.id.clone());
                                }
                                if resp.double_clicked() {
                                    actions.push(("material.new", json!({ "library": r.id })));
                                }
                            }
                        });
                    }
                });
                let can_add = st.library.as_deref().and_then(material::recipe).is_some_and(|r| !drawing.iter().any(|m| m.name.eq_ignore_ascii_case(&r.name)));
                if cols[0].add_enabled(can_add, egui::Button::new(crate::tl!("Add to drawing"))).clicked()
                    && let Some(lib) = st.library.clone()
                {
                    actions.push(("material.new", json!({ "library": lib })));
                }
                // The drawing's materials.
                cols[1].strong(crate::tl!("In this drawing"));
                egui::ScrollArea::vertical().id_salt("cc_materials_drawing").max_height(150.0).show(&mut cols[1], |ui| {
                    if drawing.is_empty() {
                        ui.label(RichText::new(crate::tl!("No materials yet: add one from the library.")).color(Tokens::get().text_dim));
                    }
                    ui.horizontal_wrapped(|ui| {
                        for m in &drawing {
                            let sel = st.selected.as_deref() == Some(m.name.as_str());
                            if swatch_button(ui, ctx, m, &m.name, sel).clicked() {
                                st.selected = Some(m.name.clone());
                                st.edit = Some(m.clone());
                            }
                        }
                    });
                });
                let ui = &mut cols[1];
                if let Some(e) = st.edit.as_mut() {
                    ui.separator();
                    egui::Grid::new("cc_material_edit").num_columns(2).spacing(vec2(10.0, 5.0)).show(ui, |ui| {
                        ui.label(crate::tl!("Name"));
                        ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(200.0));
                        ui.end_row();
                        ui.label(crate::tl!("Colour"));
                        let mut c = [e.color.0, e.color.1, e.color.2];
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            e.color = cadaddy_engine::doc::color::Rgb(c[0], c[1], c[2]);
                        }
                        ui.end_row();
                        ui.label(crate::tl!("Roughness"));
                        ui.add(egui::Slider::new(&mut e.roughness, 0.0..=1.0));
                        ui.end_row();
                        ui.label(crate::tl!("Metallic"));
                        ui.add(egui::Slider::new(&mut e.metallic, 0.0..=1.0));
                        ui.end_row();
                        ui.label(crate::tl!("Opacity"));
                        ui.add(egui::Slider::new(&mut e.opacity, 0.0..=1.0));
                        ui.end_row();
                        ui.label(crate::tl!("Mapping"));
                        egui::ComboBox::from_id_salt("cc_material_projection").selected_text(e.mapping.projection.as_str()).show_ui(ui, |ui| {
                            for p in Projection::ALL {
                                ui.selectable_value(&mut e.mapping.projection, p, p.as_str());
                            }
                        });
                        ui.end_row();
                        ui.label(crate::tl!("Tile size"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut e.mapping.size_u).speed(0.5).range(0.001..=1e6));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut e.mapping.size_v).speed(0.5).range(0.001..=1e6));
                        });
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button(crate::tl!("Apply")).clicked()
                            && let Some(old) = st.selected.clone()
                        {
                            let hex = format!("#{:02x}{:02x}{:02x}", e.color.0, e.color.1, e.color.2);
                            let mp = e.mapping;
                            actions.push((
                                "material.edit",
                                json!({ "name": old, "rename": e.name, "color": hex, "roughness": e.roughness, "metallic": e.metallic, "opacity": e.opacity,
                                        "mapping": { "projection": mp.projection.as_str(), "sizeU": mp.size_u, "sizeV": mp.size_v } }),
                            ));
                            st.selected = Some(e.name.clone());
                        }
                        let assign = ui.add_enabled(selection > 0, egui::Button::new(crate::tf!("Assign to selection ({n})", n = selection)));
                        if assign.clicked() {
                            actions.push(("materialassign", json!({ "material": e.name })));
                        }
                        if ui.button(crate::tl!("Delete")).clicked()
                            && let Some(old) = st.selected.take()
                        {
                            actions.push(("material.delete", json!({ "name": old })));
                        }
                    });
                }
            });
            // Attach by layer.
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(crate::tl!("Attach to layer"));
                let before = st.layer.clone();
                egui::ComboBox::from_id_salt("cc_material_layer").selected_text(&st.layer).show_ui(ui, |ui| {
                    for l in &layers {
                        ui.selectable_value(&mut st.layer, l.clone(), l);
                    }
                });
                if st.layer != before {
                    st.attach = layer_material(&st.layer).unwrap_or_else(|| material::GLOBAL.into());
                }
                egui::ComboBox::from_id_salt("cc_material_attach").selected_text(&st.attach).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.attach, material::GLOBAL.to_string(), material::GLOBAL);
                    for m in &drawing {
                        ui.selectable_value(&mut st.attach, m.name.clone(), &m.name);
                    }
                });
                if ui.button(crate::tl!("Attach")).clicked() {
                    actions.push(("materialattach", json!({ "layer": st.layer, "material": st.attach })));
                }
            });
        });
    for (cmd, p) in actions {
        match app.run(cmd, p) {
            Ok(r) => {
                if cmd == "material.new"
                    && let Some(n) = r.get("name").and_then(serde_json::Value::as_str)
                {
                    st.selected = Some(n.to_string());
                    st.edit = app.session.doc().ok().and_then(|d| d.material(n).cloned());
                }
                if cmd == "material.delete" {
                    st.edit = None;
                }
            }
            Err(e) => app.session.echo(e),
        }
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id, st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id));
    }
}
