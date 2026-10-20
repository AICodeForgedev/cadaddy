//! The Plot Stamp dialog (PLOTSTAMP typed or File ▸ Plot Stamp...): which fields the stamp says,
//! lines of the user's own, the corner and offset, the text's orientation, font and height, the
//! units those are shown in, whether this sheet's plots carry it, and the log file. It edits a
//! copy; OK runs PLOTSTAMP with the copy (the same options the command line writes) and Cancel
//! changes nothing. JSON calls keep running the command.

use cadaddy_engine::doc::Space;
use cadaddy_engine::doc::plotstamp::{MAX_STAMP_OFFSET_MM, MAX_USER_FIELDS, PlotStamp, STAMP_FONTS, STAMP_HEIGHT_MM, StampCorner};
use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "plotstamp";

#[derive(Clone)]
struct State {
    stamp: PlotStamp,
    /// This sheet's plots carry the stamp (the layout's page setup, or model space's setting).
    on: bool,
    /// The user's own fields, one per line.
    user: String,
    /// Offset and height in the units shown.
    offset: [f64; 2],
    height: f64,
    status: String,
}

fn id() -> egui::Id {
    egui::Id::new("plotstamp_dialog_state")
}

fn unit(inches: bool) -> f64 {
    if inches { 25.4 } else { 1.0 }
}

/// The settings as they are now, and whether the current sheet carries the stamp.
fn current(app: &CadApp) -> State {
    let (stamp, on) = app
        .session
        .doc()
        .map(|d| {
            let st = d.plot_stamp.clone().unwrap_or_default();
            let on = match app.session.space() {
                Space::Model => st.model,
                Space::Paper(n) => d.layout(&n).is_some_and(|l| l.page.plot_stamp),
            };
            (st, on)
        })
        .unwrap_or_default();
    let k = unit(stamp.inches);
    State {
        user: stamp.user_fields.join("\n"),
        offset: [stamp.offset_mm[0] / k, stamp.offset_mm[1] / k],
        height: stamp.height_mm / k,
        on,
        stamp,
        status: String::new(),
    }
}

/// The PLOTSTAMP options that write `st`.
fn params(st: &State) -> Value {
    let s = &st.stamp;
    let user: Vec<&str> = st.user.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let corner = match s.corner {
        StampCorner::BottomLeft => "bottomLeft",
        StampCorner::BottomRight => "bottomRight",
        StampCorner::TopLeft => "topLeft",
        StampCorner::TopRight => "topRight",
    };
    let mut p = json!({
        "on": st.on,
        "drawingName": s.drawing_name,
        "layoutName": s.layout_name,
        "dateTime": s.date_time,
        "deviceName": s.device_name,
        "paperSize": s.paper_size,
        "plotScale": s.plot_scale,
        "userFields": user,
        "corner": corner,
        "units": if s.inches { "inches" } else { "mm" },
        "offset": st.offset,
        "fromPaperEdge": s.from_paper_edge,
        "vertical": s.vertical,
        "upsideDown": s.upside_down,
        "font": s.font,
        "height": st.height,
        "singleLine": s.single_line,
        "logFile": s.log_file,
    });
    // An empty path leaves the one set before (the log needs one only when it is on).
    if !s.log_path.trim().is_empty() {
        p["logPath"] = json!(s.log_path.trim());
    }
    p
}

/// Opens the dialog on the drawing's settings.
pub fn open(app: &mut CadApp) {
    app.ui.dialog = Some(DIALOG.into());
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_else(|| current(app));
    let (mut ok, mut cancel) = (false, false);
    let space = app.session.space();
    egui::Window::new(crate::tl!("Plot Stamp"))
        .id(egui::Id::new("plotstamp_dialog"))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.strong(crate::tl!("Plot stamp fields"));
                let s = &mut st.stamp;
                ui.checkbox(&mut s.drawing_name, crate::tl!("Drawing name"));
                ui.checkbox(&mut s.layout_name, crate::tl!("Layout name"));
                ui.checkbox(&mut s.date_time, crate::tl!("Date and Time"));
                ui.checkbox(&mut s.device_name, crate::tl!("Device name"));
                ui.checkbox(&mut s.paper_size, crate::tl!("Paper size"));
                ui.checkbox(&mut s.plot_scale, crate::tl!("Plot scale"));
                ui.label(RichText::new(crate::tl!("No login or computer name is ever stamped.")).small().color(t.text_dim));
                ui.add_space(6.0);
                let label = ui.strong(crate::tl!("User defined fields"));
                ui.add(egui::TextEdit::multiline(&mut st.user).desired_rows(3).desired_width(220.0)).labelled_by(label.id);
                ui.label(RichText::new(crate::tf!("One per line, up to {n}.", n = MAX_USER_FIELDS)).small().color(t.text_dim));
                ui.add_space(6.0);
                let sheet = match &space {
                    Space::Model => crate::tl!("Stamp model space plots"),
                    Space::Paper(_) => crate::tl!("Stamp this layout's plots"),
                };
                ui.checkbox(&mut st.on, sheet);

                let ui = &mut cols[1];
                let s = &mut st.stamp;
                ui.strong(crate::tl!("Location and offset"));
                egui::Grid::new("plotstamp_where").num_columns(2).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
                    ui.label(crate::tl!("Location"));
                    let corners = [
                        (StampCorner::BottomLeft, crate::tl!("Bottom left")),
                        (StampCorner::BottomRight, crate::tl!("Bottom right")),
                        (StampCorner::TopLeft, crate::tl!("Top left")),
                        (StampCorner::TopRight, crate::tl!("Top right")),
                    ];
                    let shown = corners.iter().find(|c| c.0 == s.corner).map_or("", |c| c.1);
                    egui::ComboBox::from_id_salt("plotstamp_corner").selected_text(shown).show_ui(ui, |ui| {
                        for (c, l) in &corners {
                            ui.selectable_value(&mut s.corner, *c, *l);
                        }
                    });
                    ui.end_row();
                    ui.label(crate::tl!("Orientation"));
                    let shown = if s.vertical { crate::tl!("Vertical") } else { crate::tl!("Horizontal") };
                    egui::ComboBox::from_id_salt("plotstamp_orient").selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.vertical, false, crate::tl!("Horizontal"));
                        ui.selectable_value(&mut s.vertical, true, crate::tl!("Vertical"));
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut s.upside_down, crate::tl!("Upside-down"));
                    ui.end_row();
                    let k = unit(s.inches);
                    let most = MAX_STAMP_OFFSET_MM / k;
                    let step = if s.inches { 0.01 } else { 0.1 };
                    ui.label(crate::tl!("X offset"));
                    ui.add(egui::DragValue::new(&mut st.offset[0]).range(-most..=most).speed(step).max_decimals(3));
                    ui.end_row();
                    ui.label(crate::tl!("Y offset"));
                    ui.add(egui::DragValue::new(&mut st.offset[1]).range(-most..=most).speed(step).max_decimals(3));
                    ui.end_row();
                });
                ui.label(crate::tl!("Offset relative to"));
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.from_paper_edge, false, crate::tl!("Printable area"));
                    ui.radio_value(&mut s.from_paper_edge, true, crate::tl!("Paper border"));
                });
                ui.add_space(6.0);
                ui.strong(crate::tl!("Text properties"));
                egui::Grid::new("plotstamp_text").num_columns(2).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
                    ui.label(crate::tl!("Font"));
                    egui::ComboBox::from_id_salt("plotstamp_font").selected_text(s.font.clone()).show_ui(ui, |ui| {
                        for f in STAMP_FONTS {
                            ui.selectable_value(&mut s.font, f.to_string(), f);
                        }
                    });
                    ui.end_row();
                    let k = unit(s.inches);
                    ui.label(crate::tl!("Height"));
                    ui.add(
                        egui::DragValue::new(&mut st.height)
                            .range(STAMP_HEIGHT_MM.0 / k..=STAMP_HEIGHT_MM.1 / k)
                            .speed(if s.inches { 0.005 } else { 0.1 })
                            .max_decimals(3),
                    );
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut s.single_line, crate::tl!("Single line plot stamp"));
                    ui.end_row();
                    // Changing units shows the same offset and height in the new ones.
                    ui.label(crate::tl!("Units"));
                    let was = s.inches;
                    let shown = if s.inches { crate::tl!("Inches") } else { crate::tl!("Millimeters") };
                    egui::ComboBox::from_id_salt("plotstamp_units").selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.inches, false, crate::tl!("Millimeters"));
                        ui.selectable_value(&mut s.inches, true, crate::tl!("Inches"));
                    });
                    if was != s.inches {
                        let f = unit(was) / unit(s.inches);
                        st.offset = st.offset.map(|v| v * f);
                        st.height *= f;
                    }
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.strong(crate::tl!("Log file location"));
                let s = &mut st.stamp;
                ui.checkbox(&mut s.log_file, crate::tl!("Create a log file"));
                ui.add_enabled_ui(s.log_file, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.log_path).hint_text("plot.log").desired_width(170.0));
                        if ui.button(crate::tl!("Browse...")).clicked()
                            && let Some(p) = app.services.pick_save.as_ref().and_then(|f| f("plot.log"))
                        {
                            s.log_path = p;
                        }
                    });
                });
                ui.label(RichText::new(crate::tl!("The log is kept for this session, never saved with the drawing.")).small().color(t.text_dim));
            });
            if !st.status.is_empty() {
                ui.colored_label(t.warn, &st.status);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ok = ui.button(crate::tl!("OK")).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
            });
        });
    if ok {
        match app.run("plotstamp", params(&st)) {
            Ok(_) => *open = false,
            Err(e) => st.status = e,
        }
    }
    if cancel || !*open {
        ctx.data_mut(|d| d.remove::<State>(id()));
        *open = false;
    } else {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_carry_every_setting_in_the_units_shown() {
        let st = State {
            stamp: PlotStamp { inches: true, vertical: true, log_file: true, log_path: " a.log ".into(), ..PlotStamp::default() },
            on: true,
            user: "Issued for bid\n\n  Rev B ".into(),
            offset: [0.25, 0.5],
            height: 0.1,
            status: String::new(),
        };
        let p = params(&st);
        assert_eq!(p["userFields"], json!(["Issued for bid", "Rev B"]));
        assert_eq!((p["units"].as_str(), p["offset"].clone(), p["height"].as_f64()), (Some("inches"), json!([0.25, 0.5]), Some(0.1)));
        assert_eq!((p["vertical"].as_bool(), p["logPath"].as_str(), p["corner"].as_str()), (Some(true), Some("a.log"), Some("bottomLeft")));
    }
}
