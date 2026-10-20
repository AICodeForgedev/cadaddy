//! The DesignCenter palette (ADCENTER, Cmd+2): a folder's subfolders and drawings on the left,
//! the chosen drawing's blocks (with previews), layers, linetypes, text and dimension styles,
//! layouts and xrefs on the right. Double-click (or Add to Drawing) brings the selection into
//! the current drawing, a block then placed with -INSERT; dragging a block onto the drawing
//! area inserts it where it is dropped. Everything goes through the ADCENTER command.

use egui::{RichText, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::CadApp;

/// The categories of a drawing, as (ADCENTER kind, listing key, label).
const KINDS: [(&str, &str, &str); 7] = [
    ("blocks", "blocks", "Blocks"),
    ("layers", "layers", "Layers"),
    ("linetypes", "linetypes", "Linetypes"),
    ("textStyles", "textStyles", "Text Styles"),
    ("dimStyles", "dimStyles", "Dimension Styles"),
    ("layouts", "layouts", "Layouts"),
    ("xrefs", "xrefs", "Xrefs"),
];

#[derive(Clone, Default)]
struct State {
    folder: String,
    /// The folder's listing, or why it failed.
    entries: Option<Result<Value, String>>,
    drawing: Option<String>,
    listing: Option<Result<Value, String>>,
    kind: usize,
    selected: Option<String>,
    /// A block being dragged out of the palette.
    dragging: Option<String>,
    message: Option<String>,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-designcenter")
}

/// Where the palette starts: the current drawing's folder, else the home folder.
fn start_folder(app: &CadApp) -> String {
    app.session
        .state()
        .ok()
        .and_then(|st| st.path.clone())
        .and_then(|p| std::path::Path::new(&p).parent().map(|d| d.to_string_lossy().into_owned()))
        .filter(|d| !d.is_empty())
        .or_else(|| ["HOME", "USERPROFILE"].iter().find_map(|k| std::env::var(k).ok()))
        .unwrap_or_else(|| ".".into())
}

fn join(folder: &str, name: &str) -> String {
    std::path::Path::new(folder).join(name).to_string_lossy().into_owned()
}

/// The names in a listing's category (xrefs and blocks are objects with a name).
fn names(listing: &Value, key: &str) -> Vec<String> {
    listing[key]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().or_else(|| v["name"].as_str()).map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Bring `name` (of `kind`) from `drawing` into the current drawing; a block is then placed with
/// -INSERT, or at `at` when it was dropped there.
pub fn import(app: &mut CadApp, drawing: &str, kind: &str, name: &str, at: Option<[f64; 2]>) -> Result<Value, String> {
    let mut p = json!({ "action": "import", "path": drawing, "kind": kind, "names": [name] });
    if let Some(at) = at {
        p["at"] = json!(at);
    }
    let r = app.run("adcenter", p)?;
    if kind == "blocks" && at.is_none() {
        app.cmdline(&format!("-insert {name}"));
    }
    Ok(r)
}

/// The palette window.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    if st.folder.is_empty() {
        st.folder = start_folder(app);
    }
    if st.entries.is_none() {
        st.entries = Some(app.session.execute("adcenter", &json!({ "action": "folder", "path": st.folder })).map_err(|e| e.user_message()));
    }
    if let (Some(d), None) = (&st.drawing, &st.listing) {
        st.listing = Some(app.session.execute("adcenter", &json!({ "path": d })).map_err(|e| e.user_message()));
    }
    let mut go: Option<String> = None;
    let mut open_drawing: Option<String> = None;
    let mut add: Option<String> = None;
    let window = egui::Window::new(crate::tl!("DesignCenter"))
        .id(egui::Id::new("DesignCenter"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([640.0, 420.0])
        .show(ctx, |ui| {
            // Right to left in Arabic (rtl.rs).
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
            ui.horizontal(|ui| {
                // Icons drawn in code: "⬆" has no glyph in the interface fonts (an empty box).
                if crate::icons::button(ui, crate::icons::Icon::ChevronUp, 20.0, "Up One Level", false).clicked()
                    && let Some(parent) = std::path::Path::new(&st.folder).parent()
                {
                    go = Some(parent.to_string_lossy().into_owned());
                }
                let mut f = st.folder.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut f).desired_width(ui.available_width()));
                crate::icons::describe_area(&r, "Folder");
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = Some(f);
                }
            });
            ui.separator();
            ui.columns(2, |cols| {
                egui::ScrollArea::vertical().id_salt("dc-folder").max_height(320.0).show(&mut cols[0], |ui| match &st.entries {
                    Some(Ok(v)) => {
                        for f in names(v, "folders") {
                            let open = ui
                                .horizontal(|ui| {
                                    let icon = crate::icons::button(ui, crate::icons::Icon::Open, 16.0, "Open", false).clicked();
                                    icon | ui.selectable_label(false, f.as_str()).clicked()
                                })
                                .inner;
                            if open {
                                go = Some(join(&st.folder, &f));
                            }
                        }
                        for d in names(v, "drawings") {
                            let path = join(&st.folder, &d);
                            if ui.selectable_label(st.drawing.as_deref() == Some(path.as_str()), format!("📄 {d}")).clicked() {
                                open_drawing = Some(path);
                            }
                        }
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(t.warn));
                    }
                    None => {}
                });
                let ui = &mut cols[1];
                match &st.listing {
                    Some(Ok(listing)) => {
                        ui.horizontal_wrapped(|ui| {
                            for (i, (_, key, label)) in KINDS.iter().enumerate() {
                                let n = names(listing, key).len();
                                if ui.selectable_label(st.kind == i, format!("{} ({n})", crate::i18n::t(label))).clicked() {
                                    st.kind = i;
                                    st.selected = None;
                                }
                            }
                        });
                        ui.separator();
                        let (kind, key, _) = KINDS.get(st.kind).copied().unwrap_or(KINDS[0]);
                        egui::ScrollArea::vertical().id_salt("dc-items").max_height(260.0).show(ui, |ui| {
                            if kind == "blocks" {
                                ui.horizontal_wrapped(|ui| {
                                    for b in listing["blocks"].as_array().into_iter().flatten() {
                                        let name = b["name"].as_str().unwrap_or_default().to_string();
                                        let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 96.0), Sense::click_and_drag());
                                        let on = st.selected.as_deref() == Some(name.as_str());
                                        let p = ui.painter();
                                        p.rect_filled(
                                            rect,
                                            3.0,
                                            if on {
                                                t.toggle_on.gamma_multiply(0.3)
                                            } else if resp.hovered() {
                                                t.control_hover
                                            } else {
                                                t.panel
                                            },
                                        );
                                        let pic = egui::Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(68.0, 68.0));
                                        for line in b["preview"].as_array().into_iter().flatten() {
                                            let pts: Vec<egui::Pos2> = line
                                                .as_array()
                                                .into_iter()
                                                .flatten()
                                                .filter_map(|q| {
                                                    Some(pos2(
                                                        pic.left() + q[0].as_f64()? as f32 * pic.width(),
                                                        pic.bottom() - q[1].as_f64()? as f32 * pic.height(),
                                                    ))
                                                })
                                                .collect();
                                            p.add(egui::Shape::line(pts, Stroke::new(1.0, t.text)));
                                        }
                                        p.text(
                                            pos2(rect.center().x, rect.bottom() - 10.0),
                                            egui::Align2::CENTER_CENTER,
                                            &name,
                                            crate::theme::small(),
                                            t.text,
                                        );
                                        crate::icons::describe_control(ui, &resp, &name, None, false);
                                        if resp.clicked() {
                                            st.selected = Some(name.clone());
                                        }
                                        if resp.double_clicked() {
                                            add = Some(name.clone());
                                        }
                                        if resp.drag_started() {
                                            st.dragging = Some(name.clone());
                                        }
                                    }
                                });
                            } else {
                                for n in names(listing, key) {
                                    let r = ui.selectable_label(st.selected.as_deref() == Some(n.as_str()), &n);
                                    if r.clicked() {
                                        st.selected = Some(n.clone());
                                    }
                                    if r.double_clicked() {
                                        add = Some(n.clone());
                                    }
                                }
                            }
                        });
                        if ui.add_enabled(st.selected.is_some(), egui::Button::new(crate::tl!("Add to Drawing"))).clicked() {
                            add = st.selected.clone();
                        }
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(t.warn));
                    }
                    None => {
                        ui.label(RichText::new(crate::tl!("Choose a drawing to see what it holds.")).color(t.text_dim));
                    }
                }
            });
            if let Some(m) = &st.message {
                ui.label(RichText::new(m).small().color(t.text_dim));
            }
        });
    // A block dropped on the drawing area goes in where it lands.
    if let Some(name) = st.dragging.clone() {
        let released = ctx.input(|i| i.pointer.any_released());
        let over_palette = window.as_ref().is_some_and(|w| ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| w.response.rect.contains(p)));
        if released {
            st.dragging = None;
            if !over_palette && let (Some(at), Some(d)) = (app.canvas.cursor, st.drawing.clone()) {
                st.message = Some(match import(app, &d, "blocks", &name, Some([at.x, at.y])) {
                    Ok(_) => crate::tf!("{name} inserted.", name = name),
                    Err(e) => e,
                });
            }
        } else if let Some(p) = ctx.input(|i| i.pointer.hover_pos()) {
            egui::Area::new(egui::Id::new("dc-drag")).fixed_pos(p + vec2(12.0, 12.0)).interactable(false).show(ctx, |ui| {
                ui.label(RichText::new(&name).strong());
            });
        }
    }
    if let Some(f) = go {
        st.folder = f;
        st.entries = None;
    }
    if let Some(d) = open_drawing {
        st.drawing = Some(d);
        st.listing = None;
        st.selected = None;
    }
    if let (Some(name), Some(d)) = (add, st.drawing.clone()) {
        let (kind, _, _) = KINDS.get(st.kind).copied().unwrap_or(KINDS[0]);
        st.message = Some(match import(app, &d, kind, &name, None) {
            Ok(r) if r["kept"].as_array().is_some_and(|k| !k.is_empty()) => crate::tf!("{name} is already in the drawing; it was kept.", name = name),
            Ok(_) => crate::tf!("{name} added.", name = name),
            Err(e) => e,
        });
    }
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
    fn typed_adcenter_opens_the_palette() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.cmdline("adcenter");
        assert_eq!(app.ui.dialog.as_deref(), Some("adcenter"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
        assert!(!st.folder.is_empty() && st.entries.is_some(), "the start folder is listed");
    }
}
