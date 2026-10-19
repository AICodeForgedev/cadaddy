//! A LISP routine's dialog (`start_dialog`): the DCL tiles the session describes
//! ([`cadaddy_engine::Session::lisp_dialog`]) drawn with egui. A click or an edit goes back to the
//! session as a tile event, and the routine runs the tile's action; Esc and the close box are
//! Cancel. The text on the tiles is the routine's own, shown as its file gives it.

use cadaddy_engine::lisp::dcl::Tile;
use cadaddy_engine::lisp::dialog::{CHAR_H, CHAR_W, Event, ImageOp, OpenDialog, tile_px};
use egui::{Color32, Pos2, Rect, Sense, Stroke, vec2};

use crate::CadApp;
use crate::theme::Tokens;

/// What the user did this frame.
enum Act {
    /// A tile's action runs.
    Event(Event),
    /// A value changes without an action (typing in an edit box).
    Set(String, String),
    Cancel,
}

pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let Some(d) = app.session.lisp_dialog() else { return };
    let mut acts = Vec::new();
    let mut open = true;
    let title = d.tile.attr("label").unwrap_or(&d.name).to_string();
    egui::Window::new(title)
        .id(egui::Id::new(("lisp_dcl_dialog", d.name.as_str())))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.vertical(|ui| children(ui, d, &d.tile, &mut acts));
        });
    if !open {
        acts.push(Act::Cancel);
    }
    for a in acts {
        let r = match a {
            Act::Event(ev) => app.session.dcl_event(ev),
            Act::Set(key, value) => app.session.dcl_set_value(&key, &value),
            Act::Cancel => {
                app.session.dcl_cancel();
                Ok(())
            }
        };
        if let Err(e) = r {
            app.session.echo(e);
        }
        // The action may have ended the dialog (or opened another).
        if app.session.lisp_dialog().is_none() {
            break;
        }
    }
}

/// Esc while a LISP dialog is up: its Cancel. `true` when it took the key.
pub fn escape(app: &mut CadApp, ctx: &egui::Context) -> bool {
    if app.session.lisp_dialog().is_none() || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return false;
    }
    app.session.dcl_cancel();
    true
}

fn children(ui: &mut egui::Ui, d: &OpenDialog, t: &Tile, acts: &mut Vec<Act>) {
    for c in &t.children {
        tile(ui, d, c, acts);
    }
}

fn event(key: &str, value: Option<String>, reason: i32) -> Act {
    Act::Event(Event { key: key.to_string(), value, reason })
}

/// A tile's width in points from its `width` attribute (characters), when it has one.
fn width(t: &Tile) -> Option<f32> {
    let w = tile_px(t, "width", CHAR_W);
    (w > 0).then_some(w as f32)
}

fn tile(ui: &mut egui::Ui, d: &OpenDialog, t: &Tile, acts: &mut Vec<Act>) {
    let key = t.key().unwrap_or("");
    let label = t.attr("label").unwrap_or("");
    let enabled = !d.disabled.contains(key) && !t.flag("is_enabled_false");
    let kind = t.kind.as_str();
    match kind {
        "row" | "radio_row" | "concatenation" | "paragraph" => {
            ui.horizontal(|ui| children(ui, d, t, acts));
        }
        "column" | "radio_column" | "dialog" => {
            if !label.is_empty() && kind != "dialog" {
                ui.label(label);
            }
            ui.vertical(|ui| children(ui, d, t, acts));
        }
        "boxed_row" | "boxed_radio_row" | "boxed_column" | "boxed_radio_column" => {
            ui.group(|ui| {
                ui.vertical(|ui| {
                    if !label.is_empty() {
                        ui.label(egui::RichText::new(label).strong());
                    }
                    if kind.ends_with("row") {
                        ui.horizontal(|ui| children(ui, d, t, acts));
                    } else {
                        children(ui, d, t, acts);
                    }
                });
            });
        }
        "button" => {
            let mut b = egui::Button::new(label);
            if let Some(w) = width(t) {
                b = b.min_size(vec2(w, 0.0));
            }
            if ui.add_enabled(enabled, b).clicked() {
                acts.push(event(key, None, 1));
            }
        }
        "edit_box" => {
            ui.horizontal(|ui| {
                if !label.is_empty() {
                    ui.label(label);
                }
                let mut text = d.value(key).to_string();
                let w = tile_px(t, "edit_width", CHAR_W).max(10 * CHAR_W as i32) as f32;
                let mut edit = egui::TextEdit::singleline(&mut text).desired_width(w);
                if t.attr("password_char").is_some() {
                    edit = edit.password(true);
                }
                let r = ui.add_enabled(enabled, edit);
                if d.focus.as_deref() == Some(key) && !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                if r.changed() {
                    acts.push(Act::Set(key.to_string(), text.clone()));
                }
                if r.lost_focus() {
                    let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    acts.push(event(key, Some(text), if entered { 1 } else { 2 }));
                }
            });
        }
        "toggle" => {
            let mut on = d.value(key) == "1";
            if ui.add_enabled(enabled, egui::Checkbox::new(&mut on, label)).changed() {
                acts.push(event(key, Some(if on { "1" } else { "0" }.into()), 1));
            }
        }
        "radio_button" => {
            let on = d.value(key) == "1";
            if ui.add_enabled(enabled, egui::RadioButton::new(on, label)).clicked() && !on {
                acts.push(event(key, None, 1));
            }
        }
        "popup_list" => {
            let items = d.lists.get(key).map(Vec::as_slice).unwrap_or_default();
            let at: usize = d.value(key).trim().parse().unwrap_or(0);
            ui.horizontal(|ui| {
                if !label.is_empty() {
                    ui.label(label);
                }
                ui.add_enabled_ui(enabled, |ui| {
                    egui::ComboBox::from_id_salt(("dcl_popup", key))
                        .selected_text(items.get(at).map(String::as_str).unwrap_or(""))
                        .width(width(t).unwrap_or(120.0))
                        .show_ui(ui, |ui| {
                            for (i, item) in items.iter().enumerate() {
                                if ui.selectable_label(i == at, item).clicked() && i != at {
                                    acts.push(event(key, Some(i.to_string()), 1));
                                }
                            }
                        });
                });
            });
        }
        "list_box" => {
            let items = d.lists.get(key).map(Vec::as_slice).unwrap_or_default();
            let multi = t.flag("multiple_select");
            let chosen: Vec<usize> = d.value(key).split_whitespace().filter_map(|n| n.parse().ok()).collect();
            if !label.is_empty() {
                ui.label(label);
            }
            let rows = tile_px(t, "height", CHAR_H).max(3 * CHAR_H as i32) as f32;
            ui.add_enabled_ui(enabled, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt(("dcl_list", key)).max_height(rows).min_scrolled_height(rows).show(ui, |ui| {
                        ui.set_min_width(width(t).unwrap_or(160.0));
                        for (i, item) in items.iter().enumerate() {
                            let r = ui.selectable_label(chosen.contains(&i), item);
                            if r.double_clicked() {
                                acts.push(event(key, Some(i.to_string()), 4));
                            } else if r.clicked() {
                                let value = if multi {
                                    let mut set: Vec<usize> = chosen.iter().copied().filter(|c| *c != i).collect();
                                    if !chosen.contains(&i) {
                                        set.push(i);
                                        set.sort_unstable();
                                    }
                                    set.iter().map(usize::to_string).collect::<Vec<_>>().join(" ")
                                } else {
                                    i.to_string()
                                };
                                acts.push(event(key, Some(value), 1));
                            }
                        }
                    });
                });
            });
        }
        "slider" => {
            let num = |a: &str, def: f64| t.attr(a).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(def);
            let (lo, hi) = (num("min_value", 0.0), num("max_value", 10_000.0));
            let mut v = d.value(key).trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(lo);
            let r = ui.add_enabled(enabled, egui::Slider::new(&mut v, lo.min(hi)..=hi.max(lo)).integer());
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                acts.push(event(key, Some(format!("{}", v.round())), 1));
            } else if r.changed() {
                acts.push(event(key, Some(format!("{}", v.round())), 3));
            }
        }
        "text" | "text_part" | "errtile" => {
            let shown = d.values.get(key).filter(|v| !v.is_empty()).map(String::as_str).unwrap_or(label);
            if t.flag("is_error_tile") || kind == "errtile" {
                ui.colored_label(ui.visuals().error_fg_color, shown);
            } else {
                ui.label(shown);
            }
        }
        "image" | "image_button" => {
            let size = vec2(tile_px(t, "width", CHAR_W).max(1) as f32, tile_px(t, "height", CHAR_H).max(1) as f32);
            let sense = if kind == "image_button" && enabled { Sense::click() } else { Sense::hover() };
            let (rect, r) = ui.allocate_exact_size(size, sense);
            paint_image(ui, rect, t, d.images.get(key).map(Vec::as_slice).unwrap_or_default());
            if r.clicked() {
                acts.push(event(key, None, 1));
            }
        }
        "spacer" | "spacer_0" | "spacer_1" => {
            ui.add_space(tile_px(t, "height", CHAR_H).clamp(2, 60) as f32);
        }
        _ => children(ui, d, t, acts),
    }
}

/// An AutoCAD colour index on the image: -2 the dialog's background, -15 its text colour, the
/// palette otherwise.
fn color(i: i32, t: &Tokens) -> Color32 {
    match i {
        -2 => t.panel,
        -15 | -16 | -18 => t.text,
        _ => {
            let c = cadaddy_color::aci_rgb(u8::try_from(i).unwrap_or(7));
            Color32::from_rgb(c.0, c.1, c.2)
        }
    }
}

fn paint_image(ui: &egui::Ui, rect: Rect, t: &Tile, ops: &[ImageOp]) {
    let tok = Tokens::get();
    let p = ui.painter_at(rect);
    let bg = t.attr("color").and_then(|c| c.trim().parse::<i32>().ok()).map(|c| color(c, &tok)).unwrap_or(tok.canvas);
    p.rect_filled(rect, 0.0, bg);
    let at = |x: i32, y: i32| Pos2::new(rect.left() + x as f32, rect.top() + y as f32);
    for op in ops {
        match op {
            ImageOp::Fill { x, y, w, h, color: c } => {
                p.rect_filled(Rect::from_min_size(at(*x, *y), vec2(*w as f32, *h as f32)), 0.0, color(*c, &tok));
            }
            ImageOp::Vector { x1, y1, x2, y2, color: c } => {
                p.line_segment([at(*x1, *y1), at(*x2, *y2)], Stroke::new(1.0, color(*c, &tok)));
            }
            ImageOp::Slide { x, y, w, h, name } => {
                let r = Rect::from_min_size(at(*x, *y), vec2(*w as f32, *h as f32));
                p.rect_stroke(r, 0.0, Stroke::new(1.0, tok.border), egui::StrokeKind::Inside);
                p.text(r.center(), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(11.0), tok.text_dim);
            }
        }
    }
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, tok.border), egui::StrokeKind::Inside);
}
