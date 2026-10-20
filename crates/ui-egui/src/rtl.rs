//! Dialogs in a right-to-left interface (Arabic). egui lays widgets out left to right; a dialog's
//! contents go through [`body`], which, when the interface reads right to left, lays them out
//! top-down aligned to the right edge. egui's rows (`ui.horizontal`) then run from the right too:
//! a row's first widget is its rightmost, as Arabic readers expect. Grids don't follow a layout's
//! direction, so a label–control row goes through [`pair`] (or [`cells`]), which adds its cells in
//! reading order: the label in the first column left to right, in the last right to left.
//! Left-to-right interfaces are untouched.

use egui::{Align, Layout, Ui, WidgetText};

/// Whether the interface being drawn reads right to left.
pub fn active() -> bool {
    crate::i18n::rtl()
}

/// A dialog's contents laid out for the reading direction.
pub fn body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    if active() { ui.with_layout(Layout::top_down(Align::Max), add).inner } else { add(ui) }
}

/// [`body`] without a closure, for a dialog's whole contents: the rest of the closure draws into
/// it (`let mut body = rtl::Body::new(ui); let ui = &mut *body;`), and what it took is given back
/// to the dialog when it goes out of scope. Left to right, it is the dialog's own `Ui`.
pub struct Body<'a> {
    parent: &'a mut Ui,
    child: Option<Ui>,
    /// Where the contents' width is remembered between frames.
    key: egui::Id,
}

/// The width a right-to-left block gets: the dialog's whole width when the dialog has one (a
/// fixed or resizable dialog: less than most of the screen), else what the contents took last
/// frame (a dialog that sizes itself to its contents is offered about the screen's width, and
/// right-aligned contents would fill it). `None` the first frame: laid out left-aligned once to
/// measure.
fn width(ui: &Ui, key: egui::Id) -> Option<f32> {
    let available = ui.available_rect_before_wrap().width();
    let previous: f32 = ui.ctx().data(|d| d.get_temp(key))?;
    let screen = ui.ctx().content_rect().width();
    Some(if available < screen * 0.7 || available <= previous * 1.25 + 40.0 { available } else { previous.min(available) })
}

impl<'a> Body<'a> {
    pub fn new(ui: &'a mut Ui) -> Self {
        let key = ui.id().with("rtl body width");
        if !active() {
            return Body { parent: ui, child: None, key };
        }
        let avail = ui.available_rect_before_wrap();
        let child = match width(ui, key) {
            Some(w) => {
                let rect = egui::Rect::from_min_size(avail.min, egui::vec2(w, avail.height()));
                ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Max)))
            }
            None => {
                // Measure: the contents left-aligned this once, not shown.
                ui.ctx().request_discard("right-to-left dialog measured");
                ui.new_child(egui::UiBuilder::new().max_rect(avail).layout(Layout::top_down(Align::Min)))
            }
        };
        Body { parent: ui, child: Some(child), key }
    }
}

impl std::ops::Deref for Body<'_> {
    type Target = Ui;
    fn deref(&self) -> &Ui {
        match &self.child {
            Some(c) => c,
            None => self.parent,
        }
    }
}

impl std::ops::DerefMut for Body<'_> {
    fn deref_mut(&mut self) -> &mut Ui {
        match &mut self.child {
            Some(c) => c,
            None => self.parent,
        }
    }
}

impl Drop for Body<'_> {
    fn drop(&mut self) {
        if let Some(c) = &self.child {
            let r = c.min_rect();
            self.parent.ctx().data_mut(|d| d.insert_temp(self.key, r.width()));
            self.parent.advance_cursor_after_rect(r);
        }
    }
}

/// A grid, shown where it can be laid out (egui's grids work left to right only: in a right-to-left
/// dialog it sits in a left-to-right scope, its rows mirrored by [`lead`]/[`trail`] or [`pair`]).
pub fn show_grid<R>(grid: egui::Grid, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    if !active() {
        return grid.show(ui, add);
    }
    // A block as wide as the grid was last frame, at the reading edge: the right one in a
    // right-to-left row or a right-aligned column, else the left.
    let key = ui.id().with(("rtl grid width", ui.next_auto_id()));
    let avail = ui.available_rect_before_wrap();
    let w: Option<f32> = ui.ctx().data(|d| d.get_temp(key));
    let w = w.unwrap_or(avail.width()).min(avail.width());
    let rect = if ui.layout().prefer_right_to_left() {
        egui::Rect::from_min_max(egui::pos2(avail.right() - w, avail.top()), avail.max)
    } else {
        egui::Rect::from_min_size(avail.min, egui::vec2(w, avail.height()))
    };
    let r = ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| grid.show(ui, add));
    ui.ctx().data_mut(|d| d.insert_temp(key, r.response.rect.width()));
    r.inner
}

/// A row of controls in reading order (right to left in Arabic), as `ui.horizontal` inside
/// [`body`]; for rows outside one.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    if active() { ui.with_layout(Layout::right_to_left(Align::Center), add).inner } else { ui.horizontal(add).inner }
}

/// A grid row of a label and its control, in reading order (the caller ends the row).
pub fn pair<R>(ui: &mut Ui, label: impl Into<WidgetText>, control: impl FnOnce(&mut Ui) -> R) -> R {
    if active() {
        let r = control(ui);
        ui.label(label);
        r
    } else {
        ui.label(label);
        control(ui)
    }
}

/// A grid row's leading label: added here left to right; right to left, [`trail`] adds it last
/// (call both, with the same label: `lead` first in the row, `trail` just before `end_row`).
pub fn lead(ui: &mut Ui, label: impl Into<WidgetText>) {
    if !active() {
        ui.label(label);
    }
}

/// The other half of [`lead`]: the label, last in the row, when the interface reads right to left.
pub fn trail(ui: &mut Ui, label: impl Into<WidgetText>) {
    if active() {
        ui.label(label);
    }
}

/// A grid row's cells in reading order (the caller ends the row).
pub fn cells(ui: &mut Ui, cells: Vec<Box<dyn FnOnce(&mut Ui) + '_>>) {
    if active() {
        for c in cells.into_iter().rev() {
            c(ui);
        }
    } else {
        for c in cells {
            c(ui);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `pair`'s label and control land, and a row's first and second widgets, in `language`.
    fn layout(language: &'static str) -> (egui::Rect, egui::Rect, egui::Rect, egui::Rect) {
        crate::i18n::with_language(language, || {
            let ctx = egui::Context::default();
            let mut out = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);
            let mut frame = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        body(ui, |ui| {
                            show_grid(egui::Grid::new("g"), ui, |ui| {
                                let mut c = egui::Rect::NOTHING;
                                pair(ui, "Name", |ui| c = ui.button("Control").rect);
                                out.1 = c;
                                ui.end_row();
                            });
                            out.0 = ui.min_rect();
                            ui.horizontal(|ui| {
                                out.2 = ui.button("First").rect;
                                out.3 = ui.button("Second").rect;
                            });
                        });
                    });
                },
            );
            frame.textures_delta.clear();
            out
        })
    }

    #[test]
    fn rows_run_from_the_right_in_arabic_and_from_the_left_otherwise() {
        let (_, _, first, second) = layout("en");
        assert!(first.center().x < second.center().x, "English: left to right");
        assert!(first.left() < 100.0, "English rows start at the left");
        let (_, _, first, second) = layout("ar");
        assert!(first.center().x > second.center().x, "Arabic: the first button is the rightmost");
        assert!(first.right() > 700.0, "Arabic rows start at the right: {first:?}");
    }

    /// A grid row (`lead` label, control, `trail` label): the control's offset in its grid.
    fn control_offset(language: &'static str, use_pair: bool) -> f32 {
        crate::i18n::with_language(language, || {
            let ctx = egui::Context::default();
            let mut off = 0.0;
            let mut frame = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let mut body = Body::new(ui);
                        let ui = &mut *body;
                        let mut b = egui::Rect::NOTHING;
                        let g = show_grid(egui::Grid::new("g"), ui, |ui| {
                            if use_pair {
                                pair(ui, "A rather long label", |ui| b = ui.button("Control").rect);
                            } else {
                                lead(ui, "A rather long label");
                                b = ui.button("Control").rect;
                                trail(ui, "A rather long label");
                            }
                            ui.end_row();
                        });
                        off = b.left() - g.response.rect.left();
                    });
                },
            );
            frame.textures_delta.clear();
            off
        })
    }

    #[test]
    fn grid_rows_put_the_label_first_in_reading_order() {
        crate::i18n::with_language("ar", || assert!(active()));
        crate::i18n::with_language("en", || assert!(!active()));
        for use_pair in [false, true] {
            assert!(control_offset("en", use_pair) > 50.0, "English: the label's column comes first");
            assert!(control_offset("ar", use_pair) < 1.0, "Arabic: the control's column comes first (the label is to its right)");
        }
    }

    /// The main dialogs drawn in Arabic and in English: they lay out (egui's grids, which can't
    /// work right to left, panic in debug builds when one is left in a right-to-left layout).
    #[test]
    fn the_main_dialogs_lay_out_right_to_left_in_arabic() {
        let names = [
            "options",
            "layers",
            "find",
            "adcenter",
            "publish",
            "sheetset",
            "color",
            "textstyle",
            "dimstyle",
            "plot",
            "linetype",
            "purge",
            "pagesetup",
            "csettings",
            "ptype",
        ];
        for language in ["ar", "en"] {
            crate::i18n::with_language(language, || {
                let mut app = crate::CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
                let ctx = egui::Context::default();
                for name in names {
                    for _ in 0..2 {
                        app.ui.dialog = Some(name.into());
                        let mut frame = ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
                                ..Default::default()
                            },
                            |ui| {
                                crate::dialogs::show(&mut app, ui.ctx());
                            },
                        );
                        frame.textures_delta.clear();
                    }
                }
            });
        }
    }
}
