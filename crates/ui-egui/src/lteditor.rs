//! The Linetype Editor and the Hatch Pattern Editor (Format ▸ Linetype Editor…, Format ▸ Hatch
//! Pattern Editor…): small editors with a live preview. A linetype is a list of dashes, gaps,
//! dots, texts and shapes (CADaddy's own shapes from `cadaddy.shx`); a hatch pattern a list of
//! line families. Applying runs `linetype.define` / `hatch.pattern.define`; saving writes the
//! definition into a user `.lin` / `.pat` file as well.

use cadaddy_engine::doc::{DashElement, Linetype};
use cadaddy_engine::geom::Vec2;
use egui::{RichText, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::icons::{self, Icon};

pub const LINETYPE: &str = "ltypeeditor";
pub const PATTERN: &str = "pateditor";

/// One row of a linetype: what `linetype.define` takes as one element.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Dash(f64),
    Gap(f64),
    Dot,
    Text { text: String, style: String, scale: f64, rotation: f64, x: f64, y: f64 },
    Shape { name: String, scale: f64, rotation: f64, x: f64, y: f64 },
}

impl Row {
    const KINDS: [&'static str; 5] = ["Dash", "Gap", "Dot", "Text", "Shape"];

    fn kind(&self) -> &'static str {
        match self {
            Row::Dash(_) => "Dash",
            Row::Gap(_) => "Gap",
            Row::Dot => "Dot",
            Row::Text { .. } => "Text",
            Row::Shape { .. } => "Shape",
        }
    }

    fn of_kind(kind: &str) -> Row {
        match kind {
            "Gap" => Row::Gap(0.25),
            "Dot" => Row::Dot,
            "Text" => Row::Text { text: "TEXT".into(), style: "Standard".into(), scale: 0.1, rotation: 0.0, x: 0.0, y: -0.05 },
            "Shape" => Row::Shape { name: "BOX".into(), scale: 0.1, rotation: 0.0, x: 0.0, y: 0.0 },
            _ => Row::Dash(0.5),
        }
    }

    fn json(&self) -> Value {
        match self {
            Row::Dash(v) => json!({ "dash": v }),
            Row::Gap(v) => json!({ "gap": v }),
            Row::Dot => json!({ "dot": true }),
            Row::Text { text, style, scale, rotation, x, y } => {
                json!({ "text": text, "style": style, "scale": scale, "rotation": rotation, "x": x, "y": y })
            }
            Row::Shape { name, scale, rotation, x, y } => json!({ "shape": name, "scale": scale, "rotation": rotation, "x": x, "y": y }),
        }
    }
}

/// The rows as a linetype (as `linetype.define` builds it: a text or shape sits where the
/// element before it ends), for the preview.
fn linetype(rows: &[Row]) -> Linetype {
    let mut pattern: Vec<DashElement> = Vec::new();
    let attach = |e: DashElement, pattern: &mut Vec<DashElement>| {
        if pattern.last().is_none_or(|l| l.text.is_some() || l.shape_name.is_some()) {
            pattern.push(DashElement::dash(0.0));
        }
        if let Some(last) = pattern.last_mut() {
            *last = DashElement { length: last.length, ..e };
        }
    };
    for r in rows {
        match r {
            Row::Dash(v) => pattern.push(DashElement::dash(v.abs())),
            Row::Gap(v) => pattern.push(DashElement::dash(-v.abs())),
            Row::Dot => pattern.push(DashElement::dash(0.0)),
            Row::Text { text, style, scale, rotation, x, y } => attach(
                DashElement {
                    text: Some(text.clone()),
                    style: Some(style.clone()),
                    scale: *scale,
                    rotation: rotation.to_radians(),
                    offset: Vec2::new(*x, *y),
                    ..DashElement::dash(0.0)
                },
                &mut pattern,
            ),
            Row::Shape { name, scale, rotation, x, y } => attach(
                DashElement {
                    shape_name: Some(name.clone()),
                    style: Some(cadaddy_engine::doc::library::SHAPE_FILE.into()),
                    scale: *scale,
                    rotation: rotation.to_radians(),
                    offset: Vec2::new(*x, *y),
                    ..DashElement::dash(0.0)
                },
                &mut pattern,
            ),
        }
    }
    Linetype { name: "PREVIEW".into(), description: String::new(), pattern }
}

/// A drawing's linetype as rows.
fn rows_of(lt: &Linetype) -> Vec<Row> {
    let mut out = Vec::new();
    for e in &lt.pattern {
        out.push(if e.length > 0.0 {
            Row::Dash(e.length)
        } else if e.length < 0.0 {
            Row::Gap(-e.length)
        } else {
            Row::Dot
        });
        let (rotation, x, y) = (e.rotation.to_degrees(), e.offset.x, e.offset.y);
        if let Some(t) = &e.text {
            out.push(Row::Text { text: t.clone(), style: e.style.clone().unwrap_or_else(|| "Standard".into()), scale: e.scale, rotation, x, y });
        } else if let Some(n) = &e.shape_name {
            out.push(Row::Shape { name: n.clone(), scale: e.scale, rotation, x, y });
        }
    }
    out
}

#[derive(Clone)]
struct LtState {
    name: String,
    description: String,
    rows: Vec<Row>,
    message: String,
}

impl Default for LtState {
    fn default() -> Self {
        LtState {
            name: "MY_LINETYPE".into(),
            description: String::new(),
            rows: vec![Row::Dash(0.5), Row::Gap(0.1), Row::Shape { name: "CIRC".into(), scale: 0.1, rotation: 0.0, x: 0.0, y: 0.0 }, Row::Gap(0.1)],
            message: String::new(),
        }
    }
}

fn number(ui: &mut egui::Ui, v: &mut f64, speed: f64, tip: &str) {
    ui.add(egui::DragValue::new(v).speed(speed).max_decimals(4)).on_hover_text(tip);
}

/// The linetype along a sample path (straight, then turning), its pattern repeating a few times.
fn preview(ui: &mut egui::Ui, app: &CadApp, lt: &Linetype) {
    let t = crate::theme::Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(260.0), 80.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, t.field);
    let path = [Vec2::new(0.0, 0.0), Vec2::new(6.0, 0.0), Vec2::new(9.0, 0.8)];
    let len = lt.pattern_length();
    if !(len.is_finite() && len > 1e-9) {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, crate::tl!("Add a dash or a gap"), crate::theme::small(), t.text_dim);
        return;
    }
    let k = f64::from(rect.width() - 24.0) / 9.0;
    let scale = 1.6 / len;
    let at = |v: &Vec2| pos2(rect.left() + 12.0 + (v.x * k) as f32, rect.center().y + 20.0 - (v.y * k) as f32);
    let stroke = Stroke::new(1.2, t.text);
    for dash in cadaddy_engine::render::linetype_dashes(&path, lt, scale, 0.0) {
        match dash.as_slice() {
            [dot] => {
                p.circle_filled(at(dot), 1.0, t.text);
            }
            pts => {
                p.add(egui::Shape::line(pts.iter().map(at).collect(), stroke));
            }
        }
    }
    if let Ok(d) = app.session.doc() {
        for sh in cadaddy_engine::render::linetype_text::texts(d, &path, lt, scale, 0.0) {
            for line in sh.strokes.iter().chain(sh.glyphs.iter().flatten()) {
                p.add(egui::Shape::line(line.iter().map(at).collect(), Stroke::new(1.0, t.text)));
            }
        }
    }
}

pub fn linetype_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("linetype-editor");
    let mut st: LtState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let drawing_linetypes: Vec<String> =
        app.session.doc().map(|d| d.linetypes.iter().filter(|l| !l.pattern.is_empty()).map(|l| l.name.clone()).collect()).unwrap_or_default();
    let shapes: Vec<&str> = cadaddy_fonts::ltshapes::SHAPES.iter().map(|s| s.1).collect();
    let mut act: Option<Option<String>> = None;
    egui::Window::new(crate::tl!("Linetype Editor"))
        .id(egui::Id::new("Linetype Editor"))
        .open(open)
        .resizable(true)
        .default_size([520.0, 440.0])
        .show(ctx, |ui| {
            egui::Grid::new("lt-head").num_columns(2).show(ui, |ui| {
                ui.label(crate::tl!("Name"));
                ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(220.0));
                ui.end_row();
                ui.label(crate::tl!("Description"));
                ui.add(egui::TextEdit::singleline(&mut st.description).desired_width(320.0));
                ui.end_row();
                ui.label(crate::tl!("Start from"));
                egui::ComboBox::from_id_salt("lt-from").selected_text(crate::tl!("A linetype in this drawing")).show_ui(ui, |ui| {
                    for n in &drawing_linetypes {
                        if ui.selectable_label(false, n).clicked()
                            && let Some(lt) = app.session.doc().ok().and_then(|d| d.linetype(n).cloned())
                        {
                            st.rows = rows_of(&lt);
                            st.name = lt.name.clone();
                            st.description = lt.description.clone();
                        }
                    }
                });
                ui.end_row();
            });
            ui.separator();
            let lt = linetype(&st.rows);
            preview(ui, app, &lt);
            ui.label(RichText::new(crate::tf!("Pattern length {n}", n = format!("{:.4}", lt.pattern_length()))).small().color(t.text_dim));
            ui.separator();
            let n = st.rows.len();
            let mut remove = None;
            let mut swap = None;
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for (i, row) in st.rows.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let mut kind = row.kind();
                        egui::ComboBox::from_id_salt(("lt-kind", i)).width(70.0).selected_text(crate::i18n::t(kind)).show_ui(ui, |ui| {
                            for k in Row::KINDS {
                                ui.selectable_value(&mut kind, k, crate::i18n::t(k));
                            }
                        });
                        if kind != row.kind() {
                            *row = Row::of_kind(kind);
                        }
                        match row {
                            Row::Dash(v) | Row::Gap(v) => number(ui, v, 0.01, crate::tl!("Length")),
                            Row::Dot => {}
                            Row::Text { text, style, scale, rotation, x, y } => {
                                ui.add(egui::TextEdit::singleline(text).desired_width(70.0)).on_hover_text(crate::tl!("Text"));
                                ui.add(egui::TextEdit::singleline(style).desired_width(70.0)).on_hover_text(crate::tl!("Text style"));
                                number(ui, scale, 0.01, crate::tl!("Scale"));
                                number(ui, rotation, 1.0, crate::tl!("Rotation"));
                                number(ui, x, 0.01, "X");
                                number(ui, y, 0.01, "Y");
                            }
                            Row::Shape { name, scale, rotation, x, y } => {
                                egui::ComboBox::from_id_salt(("lt-shape", i)).width(70.0).selected_text(name.as_str()).show_ui(ui, |ui| {
                                    for s in &shapes {
                                        ui.selectable_value(name, (*s).to_string(), *s);
                                    }
                                });
                                number(ui, scale, 0.01, crate::tl!("Scale"));
                                number(ui, rotation, 1.0, crate::tl!("Rotation"));
                                number(ui, x, 0.01, "X");
                                number(ui, y, 0.01, "Y");
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::button(ui, Icon::Erase, 18.0, crate::tl!("Delete"), false).clicked() {
                                remove = Some(i);
                            }
                            if ui
                                .add_enabled_ui(i + 1 < n, |ui| icons::button(ui, Icon::ChevronDown, 18.0, crate::tl!("Move Down"), false))
                                .inner
                                .clicked()
                            {
                                swap = Some((i, i + 1));
                            }
                            if ui.add_enabled_ui(i > 0, |ui| icons::button(ui, Icon::ChevronUp, 18.0, crate::tl!("Move Up"), false)).inner.clicked() {
                                swap = Some((i - 1, i));
                            }
                        });
                    });
                }
            });
            if let Some(i) = remove {
                st.rows.remove(i);
            }
            if let Some((a, b)) = swap {
                st.rows.swap(a, b);
            }
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Add Row")).clicked() && st.rows.len() < 256 {
                    st.rows.push(Row::Gap(0.25));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::tl!("Save to .lin File...")).clicked() {
                        act = Some(app.services.pick_save.as_ref().and_then(|f| f(&format!("{}.lin", st.name.trim()))));
                        if act == Some(None) {
                            act = None;
                        }
                    }
                    if ui.button(crate::tl!("Apply")).clicked() {
                        act = Some(None);
                    }
                });
            });
            if !st.message.is_empty() {
                ui.label(RichText::new(&st.message).color(t.text_dim));
            }
        });
    if let Some(save) = act {
        let mut p = json!({ "name": st.name.trim(), "description": st.description, "elements": st.rows.iter().map(Row::json).collect::<Vec<_>>() });
        if let Some(path) = save {
            p["save"] = json!(path);
        }
        st.message = match app.run("linetype.define", p) {
            Ok(v) => {
                let warnings: Vec<&str> = v["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if warnings.is_empty() { crate::tf!("Linetype {name} is in the drawing", name = st.name.trim()) } else { warnings.join("; ") }
            }
            Err(e) => e,
        };
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

// ---------------- hatch patterns ----------------

/// One line family as typed (the dashes as text: `0.25, -0.125`).
#[derive(Clone, Debug, PartialEq)]
struct Family {
    angle: f64,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    dashes: String,
}

impl Family {
    fn dashes(&self) -> Vec<f64> {
        self.dashes.split([',', ' ', ';']).filter_map(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).take(256).collect()
    }
    fn line(&self) -> cadaddy_engine::doc::library::PatternLine {
        cadaddy_engine::doc::library::PatternLine { angle: self.angle, origin: (self.x, self.y), delta: (self.dx, self.dy), dashes: self.dashes() }
    }
}

#[derive(Clone)]
struct PatState {
    name: String,
    description: String,
    families: Vec<Family>,
    message: String,
}

impl Default for PatState {
    fn default() -> Self {
        PatState {
            name: "MY_PATTERN".into(),
            description: String::new(),
            families: vec![
                Family { angle: 0.0, x: 0.0, y: 0.0, dx: 0.0, dy: 0.25, dashes: String::new() },
                Family { angle: 90.0, x: 0.0, y: 0.0, dx: 0.25, dy: 0.5, dashes: "0.25, -0.25".into() },
            ],
            message: String::new(),
        }
    }
}

pub fn pattern_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("pattern-editor");
    let mut st: PatState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut act: Option<Option<String>> = None;
    egui::Window::new(crate::tl!("Hatch Pattern Editor"))
        .id(egui::Id::new("Hatch Pattern Editor"))
        .open(open)
        .resizable(true)
        .default_size([520.0, 420.0])
        .show(ctx, |ui| {
            egui::Grid::new("pat-head").num_columns(2).show(ui, |ui| {
                ui.label(crate::tl!("Name"));
                ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(220.0));
                ui.end_row();
                ui.label(crate::tl!("Description"));
                ui.add(egui::TextEdit::singleline(&mut st.description).desired_width(320.0));
                ui.end_row();
                ui.label(crate::tl!("Start from"));
                egui::ComboBox::from_id_salt("pat-from").selected_text(crate::tl!("A pattern")).height(320.0).show_ui(ui, |ui| {
                    for c in cadaddy_engine::pattern_choices().into_iter().filter(|c| !c.lines.is_empty()) {
                        if ui.selectable_label(false, &c.name).clicked() {
                            st.families = c
                                .lines
                                .iter()
                                .map(|l| Family {
                                    angle: l.angle,
                                    x: l.origin.0,
                                    y: l.origin.1,
                                    dx: l.delta.0,
                                    dy: l.delta.1,
                                    dashes: l.dashes.iter().map(|d| format!("{d}")).collect::<Vec<_>>().join(", "),
                                })
                                .collect();
                            st.name = format!("{}_COPY", c.name);
                            st.description = c.description.clone();
                        }
                    }
                });
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                // The pattern in a square, a few repeats across.
                let (rect, _) = ui.allocate_exact_size(vec2(150.0, 150.0), egui::Sense::hover());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 3.0, t.field);
                let inner = rect.shrink(4.0);
                let lines: Vec<_> = st.families.iter().filter(|f| f.dy.abs() > 1e-9).map(Family::line).collect();
                let at = |v: &Vec2| pos2(inner.min.x + v.x as f32 * inner.width(), inner.max.y - v.y as f32 * inner.height());
                for run in cadaddy_engine::render::pattern_swatch(&lines, 1.0) {
                    match run.as_slice() {
                        [] => {}
                        [dot] => {
                            p.circle_filled(at(dot), 0.8, t.text);
                        }
                        pts => {
                            p.add(egui::Shape::line(pts.iter().map(at).collect(), Stroke::new(1.0, t.text)));
                        }
                    }
                }
                ui.vertical(|ui| {
                    let n = st.families.len();
                    let mut remove = None;
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                        egui::Grid::new("pat-rows").num_columns(7).striped(true).show(ui, |ui| {
                            for h in ["Angle", "X", "Y", "ΔX", "ΔY", "Dashes"] {
                                ui.label(RichText::new(crate::i18n::t(h)).small().color(t.text_dim));
                            }
                            ui.end_row();
                            for (i, f) in st.families.iter_mut().enumerate() {
                                number(ui, &mut f.angle, 1.0, crate::tl!("Angle"));
                                number(ui, &mut f.x, 0.01, "X");
                                number(ui, &mut f.y, 0.01, "Y");
                                number(ui, &mut f.dx, 0.01, "ΔX");
                                number(ui, &mut f.dy, 0.01, "ΔY");
                                ui.add_sized([130.0, 20.0], egui::TextEdit::singleline(&mut f.dashes).hint_text("0.25, -0.125"))
                                    .on_hover_text(crate::tl!("Dashes: > 0 a dash, < 0 a gap, 0 a dot"));
                                if ui.add_enabled_ui(n > 1, |ui| icons::button(ui, Icon::Erase, 18.0, crate::tl!("Delete"), false)).inner.clicked() {
                                    remove = Some(i);
                                }
                                ui.end_row();
                            }
                        });
                    });
                    if let Some(i) = remove {
                        st.families.remove(i);
                    }
                    if ui.button(crate::tl!("Add Line Family")).clicked() && st.families.len() < 1024 {
                        st.families.push(Family { angle: 45.0, x: 0.0, y: 0.0, dx: 0.0, dy: 0.25, dashes: String::new() });
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::tl!("Save to .pat File...")).clicked() {
                        act = Some(app.services.pick_save.as_ref().and_then(|f| f(&format!("{}.pat", st.name.trim()))));
                        if act == Some(None) {
                            act = None;
                        }
                    }
                    if ui.button(crate::tl!("Apply")).clicked() {
                        act = Some(None);
                    }
                });
            });
            if !st.message.is_empty() {
                ui.label(RichText::new(&st.message).color(t.text_dim));
            }
        });
    if let Some(save) = act {
        let lines: Vec<Value> =
            st.families.iter().map(|f| json!({ "angle": f.angle, "x": f.x, "y": f.y, "dx": f.dx, "dy": f.dy, "dashes": f.dashes() })).collect();
        let mut p = json!({ "name": st.name.trim(), "description": st.description, "lines": lines });
        if let Some(path) = save {
            p["save"] = json!(path);
        }
        st.message = match app.run("hatch.pattern.define", p) {
            Ok(v) => crate::tf!("Pattern {name} is offered for new hatches", name = v["name"].as_str().unwrap_or_default()),
            Err(e) => e,
        };
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_linetypes_turn_into_each_other() {
        let rows = vec![
            Row::Dash(0.5),
            Row::Gap(0.1),
            Row::Shape { name: "BOX".into(), scale: 0.1, rotation: 0.0, x: 0.0, y: 0.0 },
            Row::Gap(0.1),
            Row::Dot,
        ];
        let lt = linetype(&rows);
        assert_eq!(lt.pattern.iter().map(|e| e.length).collect::<Vec<_>>(), [0.5, -0.1, -0.1, 0.0]);
        assert_eq!(lt.pattern[1].shape_name.as_deref(), Some("BOX"));
        assert_eq!(rows_of(&lt), rows);
        // A text before any dash sits on a zero-length element.
        let t = linetype(&[Row::of_kind("Text"), Row::Gap(0.2)]);
        assert_eq!(t.pattern.len(), 2);
        assert!(t.pattern[0].text.is_some() && t.pattern[0].length == 0.0);
    }
}
