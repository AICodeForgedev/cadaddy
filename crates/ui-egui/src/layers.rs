//! The Layer Properties Manager (LAYER): filter tree, layer table with inline editing, colour /
//! linetype / lineweight pickers, layer states and, inside a layout viewport, the VP Freeze and
//! VP Color columns. Every change runs an engine command (`layer.*`, `layerstate.*`, `vplayer`,
//! `linetype`).

use cadaddy_color::{Color, aci_rgb};
use cadaddy_doc::{Handle, Layer, Lineweight};
use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

/// Dialog-local UI state (kept in egui memory between frames).
/// The filter chosen in the tree.
#[derive(Clone, Debug, Default, PartialEq)]
enum Shown {
    #[default]
    All,
    Used,
    /// A layer filter of the drawing.
    Filter(String),
}

/// The property filter being made or edited.
#[derive(Clone, Debug, Default)]
struct FilterForm {
    /// The filter edited (None: a new one).
    editing: Option<String>,
    name: String,
    /// Nest it in this filter.
    parent: Option<String>,
    rule_name: String,
    color: String,
    linetype: String,
    description: String,
    /// Any / Yes / No for On, Frozen, Locked, Plot.
    on: Option<bool>,
    frozen: Option<bool>,
    locked: Option<bool>,
    plot: Option<bool>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct LpmState {
    filter: Shown,
    /// Show the layers the filter leaves out.
    invert: bool,
    form: Option<FilterForm>,
    search: String,
    selected: Option<String>,
    /// (layer, buffer) while renaming.
    renaming: Option<(String, String)>,
    /// (layer, buffer) while editing the description.
    describing: Option<(String, String)>,
    sort_desc: bool,
    state_name: String,
    state_sel: Option<String>,
}

fn rgb32(c: Color) -> Color32 {
    let r = c.resolve(Color::Index(7), Color::Index(7));
    Color32::from_rgb(r.0, r.1, r.2)
}

/// A colour swatch.
pub fn swatch(p: &egui::Painter, r: Rect, c: Color) {
    p.rect_filled(r, 1.0, rgb32(c));
    p.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::from_gray(25)), egui::StrokeKind::Inside);
}

/// A colour button (swatch + name) that opens the Select Color popup: the AutoCAD Color Index
/// palette and a true-colour editor. Returns the picked colour. `by` adds ByLayer/ByBlock.
pub fn color_button(ui: &mut egui::Ui, id: egui::Id, current: Color, by: bool, width: f32) -> Option<Color> {
    let t = Tokens::get();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click());
    crate::icons::describe_control(ui, &resp, &format!("{}: {}", crate::i18n::t("Color"), crate::i18n::t(&current.name())), None, false);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 2.0, t.control_hover.gamma_multiply(0.5));
    }
    swatch(p, Rect::from_min_size(pos2(rect.left() + 3.0, rect.center().y - 6.0), vec2(12.0, 12.0)), current);
    p.text(pos2(rect.left() + 20.0, rect.center().y), egui::Align2::LEFT_CENTER, crate::i18n::t(&current.name()), crate::theme::body(), t.text);
    let mut out = None;
    egui::Popup::from_toggle_button_response(&resp).id(id).width(300.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        out = color_palette(ui, id, current, by);
        if out.is_some() {
            ui.close();
        }
    });
    out
}

/// The palette body (index colours + true colour).
pub fn color_palette(ui: &mut egui::Ui, id: egui::Id, current: Color, by: bool) -> Option<Color> {
    let t = Tokens::get();
    let tab_id = id.with("tab");
    let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(if matches!(current, Color::True(_)) { 1 } else { 0 });
    let mut out = None;
    ui.horizontal(|ui| {
        for (i, l) in ["Index Color", "True Color"].iter().enumerate() {
            if ui.selectable_label(tab == i as u8, crate::i18n::t(l)).clicked() {
                tab = i as u8;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, tab));
    ui.separator();
    let cell = |ui: &mut egui::Ui, i: u8, size: f32, out: &mut Option<Color>| {
        let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
        let c = aci_rgb(i);
        ui.painter().rect_filled(r.shrink(0.5), 0.0, Color32::from_rgb(c.0, c.1, c.2));
        let sel = current == Color::Index(i);
        crate::icons::describe_control(ui, &resp, &format!("{} {i}", crate::i18n::t("Color")), Some(sel), false);
        if sel || resp.hovered() {
            ui.painter().rect_stroke(r, 0.0, Stroke::new(if sel { 2.0 } else { 1.0 }, Color32::WHITE), egui::StrokeKind::Inside);
        }
        if resp.on_hover_text(crate::tf!("Index color: {index}", index = i)).clicked() {
            *out = Some(Color::Index(i));
        }
    };
    if tab == 0 {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // The 240 graded colours: 24 hues × 10 shades.
        for shade in [0u8, 2, 4, 6, 8, 1, 3, 5, 7, 9] {
            ui.horizontal(|ui| {
                for hue in 0..24u8 {
                    cell(ui, 10 + hue * 10 + shade, 11.0, &mut out);
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for i in 1..=9u8 {
                cell(ui, i, 18.0, &mut out);
                ui.add_space(2.0);
            }
            ui.add_space(10.0);
            for i in 250..=255u8 {
                cell(ui, i, 18.0, &mut out);
                ui.add_space(2.0);
            }
        });
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        ui.add_space(6.0);
        if by {
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("ByLayer")).clicked() {
                    out = Some(Color::ByLayer);
                }
                if ui.button(crate::tl!("ByBlock")).clicked() {
                    out = Some(Color::ByBlock);
                }
            });
        }
        ui.label(RichText::new(crate::tf!("Current: {color}", color = crate::i18n::t(&current.name()))).small().color(t.text_dim));
    } else {
        let rid = id.with("rgb");
        let start = current.resolve(Color::Index(7), Color::Index(7));
        let mut c = ui.data_mut(|d| d.get_temp::<[u8; 3]>(rid)).unwrap_or([start.0, start.1, start.2]);
        let mut c32 = Color32::from_rgb(c[0], c[1], c[2]);
        egui::color_picker::color_picker_color32(ui, &mut c32, egui::color_picker::Alpha::Opaque);
        c = [c32.r(), c32.g(), c32.b()];
        ui.horizontal(|ui| {
            for (i, l) in ["R", "G", "B"].iter().enumerate() {
                ui.label(*l);
                if let Some(v) = c.get_mut(i) {
                    ui.add(egui::DragValue::new(v).range(0..=255));
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(rid, c));
        if ui.button(crate::tl!("Apply true color")).clicked() {
            out = Some(Color::True(cadaddy_color::Rgb(c[0], c[1], c[2])));
            ui.data_mut(|d| d.remove::<[u8; 3]>(rid));
        }
    }
    out
}

/// A small horizontal bar showing a lineweight.
fn lw_sample(p: &egui::Painter, r: Rect, lw: Lineweight, c: Color32) {
    let w = match lw {
        Lineweight::Mm100(v) => (f32::from(v) / 100.0 * 3.0).clamp(1.0, 7.0),
        _ => 1.0,
    };
    p.line_segment([pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)], Stroke::new(w, c));
}

/// A linetype preview drawn from its own dash pattern.
fn lt_sample(p: &egui::Painter, r: Rect, lt: Option<&cadaddy_doc::Linetype>, c: Color32) {
    let y = r.center().y;
    let Some(lt) = lt.filter(|l| !l.pattern.is_empty() && l.pattern.iter().any(|e| e.length.abs() > 1e-9)) else {
        p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, c));
        return;
    };
    let total: f64 = lt.pattern.iter().map(|e| e.length.abs()).sum::<f64>().max(1e-9);
    let k = f64::from(r.width()) / 2.0 / total;
    let mut x = f64::from(r.left());
    let mut guard = 0;
    while x < f64::from(r.right()) && guard < 200 {
        for e in &lt.pattern {
            let l = e.length.abs() * k;
            let x1 = (x + l).min(f64::from(r.right()));
            if e.length > 0.0 {
                p.line_segment([pos2(x as f32, y), pos2(x1 as f32, y)], Stroke::new(1.0, c));
            } else if e.length == 0.0 {
                p.circle_filled(pos2(x as f32, y), 0.8, c);
            }
            x = x1.max(x + 1.0);
            guard += 1;
        }
    }
}

/// Status column: a green check for the current layer, a sheet for the others.
fn status_icon(p: &egui::Painter, r: Rect, current: bool, used: bool) {
    if current {
        let g = Color32::from_rgb(0x4c, 0xc2, 0x5a);
        p.line_segment([pos2(r.left() + 3.0, r.center().y), pos2(r.left() + 7.0, r.bottom() - 4.0)], Stroke::new(2.0, g));
        p.line_segment([pos2(r.left() + 7.0, r.bottom() - 4.0), pos2(r.right() - 3.0, r.top() + 3.0)], Stroke::new(2.0, g));
    } else {
        let t = Tokens::get();
        let c = if used { t.icon } else { t.text_faint };
        let pts = vec![
            pos2(r.left() + 2.0, r.center().y + 1.0),
            pos2(r.center().x, r.top() + 4.0),
            pos2(r.right() - 2.0, r.center().y - 1.0),
            pos2(r.center().x, r.bottom() - 3.0),
        ];
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, c)));
    }
}

fn natural_lower(s: &str) -> String {
    s.to_lowercase()
}

/// The Layer Properties Manager window.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let sid = egui::Id::new("lpm_state");
    let mut st: LpmState = ctx.data_mut(|d| d.get_temp(sid)).unwrap_or_default();
    let mut actions: Vec<(&str, Value)> = Vec::new();
    let Ok(dst) = app.session.state() else {
        *open = false;
        return;
    };
    let d = &dst.doc;
    let cur = d.header.str("CLAYER", "0");
    let used = used_layers(d);
    let vp: Option<(Handle, cadaddy_doc::Viewport)> = dst.active_viewport();
    let linetypes: Vec<cadaddy_doc::Linetype> = d.linetypes.clone();
    let library: Vec<String> = cadaddy_doc::library::standard_linetypes().into_iter().map(|l| l.name).filter(|n| d.linetype(n).is_none()).collect();
    let states: Vec<String> = d.layer_states.iter().map(|s| s.name.clone()).collect();
    let filters: Vec<cadaddy_doc::LayerFilter> = d.layer_filters.clone();
    if let Shown::Filter(f) = &st.filter
        && !filters.iter().any(|x| &x.name == f)
    {
        st.filter = Shown::All;
    }
    let in_filter: Option<std::collections::HashSet<String>> = match &st.filter {
        Shown::Filter(f) => {
            cadaddy_engine::cmd::layer_filter_layers(d, f, false).ok().map(|v| v.into_iter().map(|n| n.to_ascii_lowercase()).collect())
        }
        _ => None,
    };
    let mut layers: Vec<Layer> = d.layers.clone();
    layers.sort_by_key(|l| natural_lower(&l.name));
    if st.sort_desc {
        layers.reverse();
    }
    let total = layers.len();
    let q = st.search.to_lowercase();
    let shown: Vec<Layer> = layers
        .into_iter()
        .filter(|l| {
            let key = l.name.to_ascii_lowercase();
            let pass = match &st.filter {
                Shown::All => true,
                Shown::Used => used.contains(&key),
                Shown::Filter(_) => in_filter.as_ref().is_some_and(|f| f.contains(&key)),
            };
            pass != st.invert
        })
        .filter(|l| q.is_empty() || crate_wild(&q, &l.name.to_lowercase()))
        .collect();
    let title = match &vp {
        Some(_) => "Layer Properties Manager — current viewport",
        None => "Layer Properties Manager",
    };
    egui::Window::new(crate::i18n::t(title))
        .id(egui::Id::new("lpm_window"))
        .open(open)
        .default_size(vec2(1180.0, 440.0))
        .min_width(640.0)
        // Never wider than the window it sits in: the table scrolls sideways instead of its last
        // columns being cut off at the screen edge (UI review 0.7.7b).
        .max_width((ctx.content_rect().width() - 16.0).max(640.0))
        .resizable(true)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            // ----- tool row -----
            ui.horizontal(|ui| {
                if icons::button(ui, Icon::Plus, 24.0, "New Layer", false).clicked() {
                    let mut n = 1;
                    while d.layer(&format!("Layer{n}")).is_some() {
                        n += 1;
                    }
                    let name = format!("Layer{n}");
                    actions.push(("layer.new", json!({ "name": name })));
                    st.renaming = Some((name.clone(), name.clone()));
                    st.selected = Some(name);
                }
                if icons::button(ui, Icon::Close, 24.0, "Delete Layer", false).clicked()
                    && let Some(s) = &st.selected
                {
                    actions.push(("layer.delete", json!({ "name": s })));
                }
                if icons::button(ui, Icon::MakeCurrent, 24.0, "Set Current", false).clicked()
                    && let Some(s) = &st.selected
                {
                    actions.push(("layer.current", json!({ "name": s })));
                }
                ui.separator();
                if ui.button(crate::tl!("Layer States Manager...")).clicked() {
                    actions.push(("ui.layerstates", json!({})));
                }
                // Layer states.
                ui.label(crate::tl!("Layer State"));
                let label = st.state_sel.clone().unwrap_or_else(|| crate::tl!("Unsaved Layer State").into());
                egui::ComboBox::from_id_salt("lpm_states").selected_text(label).width(170.0).show_ui(ui, |ui| {
                    for s in &states {
                        if ui.selectable_label(st.state_sel.as_deref() == Some(s), s).clicked() {
                            st.state_sel = Some(s.clone());
                        }
                    }
                    if states.is_empty() {
                        ui.label(RichText::new(crate::tl!("No saved layer states")).color(t.text_dim));
                    }
                });
                let has = st.state_sel.is_some();
                if ui.add_enabled(has, egui::Button::new(crate::tl!("Restore"))).clicked()
                    && let Some(s) = &st.state_sel
                {
                    actions.push(("layerstate.restore", json!({ "name": s })));
                }
                if ui.add_enabled(has, egui::Button::new(crate::tl!("Delete"))).clicked()
                    && let Some(s) = st.state_sel.take()
                {
                    actions.push(("layerstate.delete", json!({ "name": s })));
                }
                ui.add(egui::TextEdit::singleline(&mut st.state_name).hint_text(crate::tl!("New state name")).desired_width(120.0));
                if ui.add_enabled(!st.state_name.trim().is_empty(), egui::Button::new(crate::tl!("Save State"))).clicked() {
                    let n = st.state_name.trim().to_string();
                    actions.push(("layerstate.save", json!({ "name": n })));
                    st.state_sel = Some(n);
                    st.state_name.clear();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut st.search).hint_text(crate::tl!("Search for layer")).desired_width(160.0));
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(ui.cursor().right_center() - vec2(8.0, 0.0), vec2(14.0, 14.0)),
                        Icon::Search,
                        false,
                    );
                    ui.add_space(18.0);
                });
            });
            ui.separator();
            // A fixed-height table: the window keeps its size instead of growing to the screen.
            let avail = 330.0;
            ui.horizontal_top(|ui| {
                // Cap the row: the vertical separator below takes all the height it is offered, and a
                // resizable window offers its whole previous height, so the window grew every frame (#29).
                ui.set_max_height(avail);
                // ----- filter tree -----
                ui.allocate_ui_with_layout(vec2(190.0, avail), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_width(190.0);
                    ui.label(RichText::new(crate::tl!("Filters")).small().color(t.text_dim));
                    egui::ScrollArea::vertical().id_salt("lpm_filters").max_height(avail - 120.0).show(ui, |ui| {
                        for (f, label, indent) in [(Shown::All, "All", 0.0), (Shown::Used, "All Used Layers", 14.0)] {
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                if ui.selectable_label(st.filter == f, crate::i18n::t(label)).clicked() {
                                    st.filter = f.clone();
                                }
                            });
                        }
                        // The drawing's filters, nested under their parents.
                        fn depth(fs: &[cadaddy_doc::LayerFilter], f: &cadaddy_doc::LayerFilter) -> usize {
                            let mut n = 0;
                            let mut p = f.parent.clone();
                            while let Some(name) = p.filter(|_| n < 16) {
                                n += 1;
                                p = fs.iter().find(|x| x.name == name).and_then(|x| x.parent.clone());
                            }
                            n
                        }
                        let mut order: Vec<&cadaddy_doc::LayerFilter> = Vec::new();
                        fn visit<'a>(
                            fs: &'a [cadaddy_doc::LayerFilter],
                            parent: Option<&str>,
                            out: &mut Vec<&'a cadaddy_doc::LayerFilter>,
                            guard: usize,
                        ) {
                            if guard > 16 {
                                return;
                            }
                            for f in fs.iter().filter(|f| f.parent.as_deref() == parent) {
                                out.push(f);
                                visit(fs, Some(&f.name), out, guard + 1);
                            }
                        }
                        visit(&filters, None, &mut order, 0);
                        for f in order {
                            let group = matches!(f.kind, cadaddy_doc::LayerFilterKind::Group { .. });
                            ui.horizontal(|ui| {
                                ui.add_space(14.0 * (1 + depth(&filters, f)) as f32);
                                let mark = if group { "▣ " } else { "⚲ " };
                                let sel = st.filter == Shown::Filter(f.name.clone());
                                if ui.selectable_label(sel, format!("{mark}{}", f.name)).clicked() {
                                    st.filter = Shown::Filter(f.name.clone());
                                }
                            });
                        }
                    });
                    ui.checkbox(&mut st.invert, crate::tl!("Invert filter"));
                    if ui.small_button(crate::tl!("New Property Filter...")).clicked() {
                        let parent = match &st.filter {
                            Shown::Filter(f) => Some(f.clone()),
                            _ => None,
                        };
                        let mut n = 1;
                        while filters.iter().any(|f| f.name == format!("Filter{n}")) {
                            n += 1;
                        }
                        st.form = Some(FilterForm { name: format!("Filter{n}"), parent, ..FilterForm::default() });
                    }
                    if ui.small_button(crate::tl!("New Group Filter")).clicked() {
                        let mut n = 1;
                        while filters.iter().any(|f| f.name == format!("Group{n}")) {
                            n += 1;
                        }
                        let layers: Vec<String> = st.selected.iter().cloned().collect();
                        actions.push(("layer.filter.new", json!({ "name": format!("Group{n}"), "type": "group", "layers": layers })));
                        st.filter = Shown::Filter(format!("Group{n}"));
                    }
                    if let Shown::Filter(name) = st.filter.clone()
                        && let Some(f) = filters.iter().find(|f| f.name == name)
                    {
                        ui.horizontal(|ui| {
                            match &f.kind {
                                cadaddy_doc::LayerFilterKind::Property(r) => {
                                    if ui.small_button(crate::tl!("Edit")).clicked() {
                                        st.form = Some(FilterForm {
                                            editing: Some(f.name.clone()),
                                            name: f.name.clone(),
                                            parent: f.parent.clone(),
                                            rule_name: r.name.clone().unwrap_or_default(),
                                            color: r.color.clone().unwrap_or_default(),
                                            linetype: r.linetype.clone().unwrap_or_default(),
                                            description: r.description.clone().unwrap_or_default(),
                                            on: r.on,
                                            frozen: r.frozen,
                                            locked: r.locked,
                                            plot: r.plot,
                                            error: None,
                                        });
                                    }
                                }
                                cadaddy_doc::LayerFilterKind::Group { .. } => {
                                    if ui.add_enabled(st.selected.is_some(), egui::Button::new(crate::tl!("Add Layer")).small()).clicked()
                                        && let Some(l) = &st.selected
                                    {
                                        actions.push(("layer.filter.edit", json!({ "name": name, "addLayers": [l] })));
                                    }
                                    if ui.add_enabled(st.selected.is_some(), egui::Button::new(crate::tl!("Remove Layer")).small()).clicked()
                                        && let Some(l) = &st.selected
                                    {
                                        actions.push(("layer.filter.edit", json!({ "name": name, "removeLayers": [l] })));
                                    }
                                }
                            }
                            if ui.small_button(crate::tl!("Delete")).clicked() {
                                actions.push(("layer.filter.delete", json!({ "name": name })));
                                st.filter = Shown::All;
                            }
                        });
                    }
                    ui.add_space(8.0);
                    if let Some((h, _)) = &vp {
                        ui.label(RichText::new(crate::tf!("Viewport {id}", id = h.hex())).small().color(t.text_dim));
                    }
                });
                ui.separator();
                // ----- the table -----
                ui.vertical(|ui| {
                    let width = ui.available_width();
                    // Reopened (not shown last frame): back to the first columns, Status and Name,
                    // instead of where it was scrolled sideways before.
                    let pass = ui.ctx().cumulative_pass_nr();
                    let shown_id = egui::Id::new("lpm_table_shown");
                    let reopened = ui.data(|d| d.get_temp::<u64>(shown_id)).is_none_or(|last| last + 1 < pass);
                    ui.data_mut(|d| d.insert_temp(shown_id, pass));
                    let mut table = egui::ScrollArea::both()
                        .id_salt("lpm_table")
                        .auto_shrink([false, false])
                        .max_height(avail)
                        .max_width(width)
                        // Visible without hovering, so the columns past the edge are discoverable.
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
                    if reopened {
                        table = table.scroll_offset(vec2(0.0, 0.0));
                    }
                    table.show(ui, |ui| {
                        let mut cols: Vec<&str> =
                            vec!["Status", "Name", "On", "Freeze", "Lock", "Plot", "Color", "Linetype", "Lineweight", "Transparency"];
                        if vp.is_some() {
                            cols.extend(["VP Freeze", "VP Color"]);
                        }
                        cols.extend(["New VP Freeze", "Description"]);
                        egui::Grid::new("lpm_grid").striped(true).num_columns(cols.len()).spacing(vec2(10.0, 3.0)).min_row_height(22.0).show(
                            ui,
                            |ui| {
                                for h in &cols {
                                    if *h == "Name" {
                                        let arrow = if st.sort_desc { " ▼" } else { " ▲" };
                                        if ui
                                            .add(
                                                egui::Label::new(RichText::new(format!("{}{arrow}", crate::tl!("Name"))).strong())
                                                    .sense(Sense::click()),
                                            )
                                            .clicked()
                                        {
                                            st.sort_desc = !st.sort_desc;
                                        }
                                    } else {
                                        ui.label(RichText::new(crate::i18n::t(h)).strong());
                                    }
                                }
                                ui.end_row();
                                for l in &shown {
                                    row(ui, l, &cur, &used, vp.as_ref(), &linetypes, &library, &mut st, &mut actions);
                                    ui.end_row();
                                }
                            },
                        );
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::tf!(
                        "Layers displayed: {shown} of {total}.   Current layer: {layer}",
                        shown = shown.len(),
                        total = total,
                        layer = cur
                    ))
                    .small()
                    .color(t.text_dim),
                );
            });
        });
    if let Some(mut form) = st.form.take() {
        let (done, cancel) = filter_form(ctx, &mut form);
        if done {
            let rules = json!({
                "name": form.rule_name, "color": form.color, "linetype": form.linetype, "description": form.description,
                "on": form.on, "frozen": form.frozen, "locked": form.locked, "plot": form.plot,
            });
            let r = match &form.editing {
                Some(old) => app.run("layer.filter.edit", json!({ "name": old, "newName": form.name, "rules": rules })),
                None => app.run("layer.filter.new", json!({ "name": form.name, "type": "property", "parent": form.parent, "rules": rules })),
            };
            match r {
                Ok(_) => st.filter = Shown::Filter(form.name.trim().to_string()),
                Err(e) => {
                    form.error = Some(e);
                    st.form = Some(form);
                }
            }
        } else if !cancel {
            st.form = Some(form);
        }
    }
    ctx.data_mut(|dd| dd.insert_temp(sid, st));
    for (c, p) in actions {
        if c == "ui.layerstates" {
            crate::layerstates::open(app, Some("layers"));
            continue;
        }
        if c == "layer.set"
            && p.get("on") == Some(&Value::Bool(false))
            && let Some(name) = p.get("name").and_then(Value::as_str)
        {
            set_flag(app, name, "on", false);
        } else {
            let _ = app.run(c, p);
        }
    }
}

/// The Layer Filter Properties window: a property filter's name and rules. Returns (OK, Cancel).
fn filter_form(ctx: &egui::Context, f: &mut FilterForm) -> (bool, bool) {
    let (mut ok, mut cancel) = (false, false);
    let mut open = true;
    egui::Window::new(crate::tl!("Layer Filter Properties"))
        .id(egui::Id::new("lpm_filter_form"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("lpm_filter_grid").num_columns(2).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
                ui.label(crate::tl!("Filter name"));
                ui.text_edit_singleline(&mut f.name);
                ui.end_row();
                if let Some(p) = &f.parent {
                    ui.label(crate::tl!("Within"));
                    ui.label(p);
                    ui.end_row();
                }
                for (label, v) in
                    [("Name", &mut f.rule_name), ("Color", &mut f.color), ("Linetype", &mut f.linetype), ("Description", &mut f.description)]
                {
                    ui.label(crate::i18n::t(label));
                    ui.add(egui::TextEdit::singleline(v).hint_text("*"));
                    ui.end_row();
                }
                for (label, v) in [("On", &mut f.on), ("Freeze", &mut f.frozen), ("Lock", &mut f.locked), ("Plot", &mut f.plot)] {
                    ui.label(crate::i18n::t(label));
                    egui::ComboBox::from_id_salt(("lpm_rule", label))
                        .selected_text(match v {
                            None => crate::tl!("Any"),
                            Some(true) => crate::tl!("Yes"),
                            Some(false) => crate::tl!("No"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(v, None, crate::tl!("Any"));
                            ui.selectable_value(v, Some(true), crate::tl!("Yes"));
                            ui.selectable_value(v, Some(false), crate::tl!("No"));
                        });
                    ui.end_row();
                }
            });
            ui.label(RichText::new(crate::tl!("Patterns: * any, ? one character, # one digit; separate with commas; ~ excludes.")).small().weak());
            if let Some(e) = &f.error {
                ui.colored_label(Tokens::get().warn, e);
            }
            ui.horizontal(|ui| {
                ok = ui.button(crate::tl!("OK")).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
            });
        });
    (ok, cancel || !open)
}

/// A layer's On, Frozen or Locked from a palette or the ribbon. Turning the current layer off asks
/// first, as AutoCAD does: new objects would be drawn on a layer that doesn't show.
pub(crate) fn set_flag(app: &mut CadApp, name: &str, key: &'static str, value: bool) {
    let current = app.session.doc().is_ok_and(|d| d.header.str("CLAYER", "0").eq_ignore_ascii_case(name));
    if key == "on" && !value && current {
        app.layer_off_ask = Some(name.to_string());
        return;
    }
    let _ = app.run("layer.set", json!({ "name": name, key: value }));
}

/// The question [`set_flag`] asks before turning the current layer off.
pub fn off_prompt(app: &mut CadApp, ctx: &egui::Context) {
    let Some(name) = app.layer_off_ask.clone() else { return };
    let mut answer = None;
    egui::Modal::new(egui::Id::new("layer_off_current")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.label(RichText::new(crate::tl!("The current layer is being turned off")).strong());
        ui.label(crate::tf!("Layer \"{layer}\" is the current layer. Objects you draw on it won't show until you turn it back on.", layer = name));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(crate::tl!("Keep the Current Layer On")).clicked() {
                answer = Some(false);
            }
            if ui.button(crate::tl!("Turn the Current Layer Off")).clicked() {
                answer = Some(true);
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
    });
    if let Some(off) = answer {
        app.layer_off_ask = None;
        if off {
            let _ = app.run("layer.set", json!({ "name": name, "on": false }));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    l: &Layer,
    cur: &str,
    used: &std::collections::HashSet<String>,
    vp: Option<&(Handle, cadaddy_doc::Viewport)>,
    linetypes: &[cadaddy_doc::Linetype],
    library: &[String],
    st: &mut LpmState,
    actions: &mut Vec<(&'static str, Value)>,
) {
    let t = Tokens::get();
    let name = l.name.clone();
    let current = name.eq_ignore_ascii_case(cur);
    let is_used = used.contains(&name.to_ascii_lowercase());
    let selected = st.selected.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(&name));
    // Status.
    let (sr, sresp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    crate::icons::describe_control(ui, &sresp, &format!("{}: {name}", crate::i18n::t("Status")), Some(current), false);
    if selected {
        ui.painter().rect_filled(sr, 2.0, t.accent.gamma_multiply(0.35));
    }
    status_icon(ui.painter(), sr.shrink(2.0), current, is_used);
    let tip = if current {
        "Current layer"
    } else if is_used {
        "Layer contains objects (double-click to make current)"
    } else {
        "Layer is empty (double-click to make current)"
    };
    if sresp.on_hover_text(crate::i18n::t(tip)).double_clicked() {
        actions.push(("layer.current", json!({ "name": name })));
    }
    // Name (double-click or F2-style rename).
    match st.renaming.clone() {
        Some((n, mut buf)) if n.eq_ignore_ascii_case(&name) => {
            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(150.0).id(egui::Id::new(("lpm_rename", &name))));
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.lost_focus() {
                let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if !cancel && buf.trim() != name && !buf.trim().is_empty() {
                    actions.push(("layer.set", json!({ "name": name, "newName": buf.trim() })));
                    st.selected = Some(buf.trim().to_string());
                }
                st.renaming = None;
            } else {
                st.renaming = Some((n, buf));
            }
        }
        _ => {
            let (nr, r) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::click());
            crate::icons::describe_control(ui, &r, &name, Some(selected), false);
            if selected {
                ui.painter().rect_filled(nr, 2.0, t.accent.gamma_multiply(0.45));
            } else if r.hovered() {
                ui.painter().rect_filled(nr, 2.0, t.control_hover.gamma_multiply(0.5));
            }
            ui.painter().text(pos2(nr.left() + 4.0, nr.center().y), egui::Align2::LEFT_CENTER, &name, crate::theme::body(), t.text);
            if r.clicked() {
                st.selected = Some(name.clone());
            }
            if r.double_clicked() && name != "0" && !name.eq_ignore_ascii_case("Defpoints") {
                st.renaming = Some((name.clone(), name.clone()));
            }
            r.context_menu(|ui| {
                if ui.button(crate::tl!("Set Current")).clicked() {
                    actions.push(("layer.current", json!({ "name": name })));
                    ui.close();
                }
                if ui.button(crate::tl!("Rename Layer")).clicked() {
                    st.renaming = Some((name.clone(), name.clone()));
                    ui.close();
                }
                if ui.button(crate::tl!("Delete Layer")).clicked() {
                    actions.push(("layer.delete", json!({ "name": name })));
                    ui.close();
                }
            });
        }
    }
    // On / Freeze / Lock / Plot.
    if icons::button(ui, if l.on { Icon::Bulb } else { Icon::BulbOff }, 20.0, "On/Off", false).clicked() {
        actions.push(("layer.set", json!({ "name": name, "on": !l.on })));
    }
    if icons::button(ui, if l.frozen { Icon::Snowflake } else { Icon::Sun }, 20.0, "Freeze/Thaw in all viewports", false).clicked() {
        actions.push(("layer.set", json!({ "name": name, "frozen": !l.frozen })));
    }
    if icons::button(ui, if l.locked { Icon::Lock } else { Icon::Unlock }, 20.0, "Lock/Unlock", false).clicked() {
        actions.push(("layer.set", json!({ "name": name, "locked": !l.locked })));
    }
    let pr = icons::button(ui, Icon::Plot, 20.0, if l.plot { "Plot (click: do not plot)" } else { "Do not plot (click: plot)" }, false);
    if !l.plot {
        let r = pr.rect.shrink(3.0);
        ui.painter().line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.6, Color32::from_rgb(0xe0, 0x50, 0x50)));
    }
    if pr.clicked() {
        actions.push(("layer.set", json!({ "name": name, "plot": !l.plot })));
    }
    // Color.
    if let Some(c) = color_button(ui, egui::Id::new(("lpm_color", &name)), l.color, false, 96.0) {
        actions.push(("layer.set", json!({ "name": name, "color": c.name() })));
    }
    // Linetype.
    let lt_resp = ui
        .allocate_ui(vec2(130.0, 20.0), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(130.0, 20.0), Sense::click());
            crate::icons::describe_control(ui, &resp, &format!("{}: {}", crate::i18n::t("Linetype"), l.linetype), None, false);
            if resp.hovered() {
                ui.painter().rect_filled(r, 2.0, t.control_hover.gamma_multiply(0.5));
            }
            lt_sample(
                ui.painter(),
                Rect::from_min_size(pos2(r.left() + 3.0, r.top()), vec2(30.0, r.height())),
                linetypes.iter().find(|x| x.name.eq_ignore_ascii_case(&l.linetype)),
                t.text,
            );
            ui.painter().text(pos2(r.left() + 38.0, r.center().y), egui::Align2::LEFT_CENTER, &l.linetype, crate::theme::body(), t.text);
            resp
        })
        .inner;
    egui::Popup::from_toggle_button_response(&lt_resp)
        .id(egui::Id::new(("lpm_lt", &name)))
        .width(240.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.label(RichText::new(crate::tl!("Loaded linetypes")).small().color(t.text_dim));
            for lt in linetypes.iter().filter(|x| !["bylayer", "byblock"].contains(&x.name.to_ascii_lowercase().as_str())) {
                let (r, resp) = ui.allocate_exact_size(vec2(230.0, 20.0), Sense::click());
                crate::icons::describe_control(ui, &resp, &lt.name, Some(lt.name.eq_ignore_ascii_case(&l.linetype)), false);
                if resp.hovered() || lt.name.eq_ignore_ascii_case(&l.linetype) {
                    ui.painter().rect_filled(r, 2.0, t.accent.gamma_multiply(if resp.hovered() { 0.6 } else { 0.3 }));
                }
                lt_sample(ui.painter(), Rect::from_min_size(pos2(r.left() + 4.0, r.top()), vec2(50.0, r.height())), Some(lt), t.text);
                ui.painter().text(pos2(r.left() + 62.0, r.center().y), egui::Align2::LEFT_CENTER, &lt.name, crate::theme::body(), t.text);
                if resp.on_hover_text(&lt.description).clicked() {
                    actions.push(("layer.set", json!({ "name": name, "linetype": lt.name })));
                    ui.close();
                }
            }
            if !library.is_empty() {
                ui.separator();
                ui.menu_button("Load...", |ui| {
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for n in library {
                            if ui.button(n).clicked() {
                                actions.push(("linetype", json!({ "load": n })));
                                actions.push(("layer.set", json!({ "name": name, "linetype": n })));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button(crate::tl!("Load all")).clicked() {
                        actions.push(("linetype", json!({ "load": "*" })));
                        ui.close();
                    }
                });
            }
        });
    // Lineweight.
    let lw_resp = ui
        .allocate_ui(vec2(100.0, 20.0), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(100.0, 20.0), Sense::click());
            crate::icons::describe_control(
                ui,
                &resp,
                &format!("{}: {}", crate::i18n::t("Lineweight"), crate::i18n::t(&l.lineweight.name())),
                None,
                false,
            );
            if resp.hovered() {
                ui.painter().rect_filled(r, 2.0, t.control_hover.gamma_multiply(0.5));
            }
            lw_sample(ui.painter(), Rect::from_min_size(pos2(r.left() + 3.0, r.top()), vec2(22.0, r.height())), l.lineweight, t.text);
            ui.painter().text(
                pos2(r.left() + 30.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                crate::i18n::t(&l.lineweight.name()),
                crate::theme::body(),
                t.text,
            );
            resp
        })
        .inner;
    egui::Popup::from_toggle_button_response(&lw_resp)
        .id(egui::Id::new(("lpm_lw", &name)))
        .width(170.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                let opts = std::iter::once(Lineweight::Default).chain(Lineweight::STANDARD.iter().map(|v| Lineweight::Mm100(*v)));
                for lw in opts {
                    let (r, resp) = ui.allocate_exact_size(vec2(160.0, 20.0), Sense::click());
                    crate::icons::describe_control(ui, &resp, crate::i18n::t(&lw.name()), Some(lw == l.lineweight), false);
                    if resp.hovered() || lw == l.lineweight {
                        ui.painter().rect_filled(r, 2.0, t.accent.gamma_multiply(if resp.hovered() { 0.6 } else { 0.3 }));
                    }
                    lw_sample(ui.painter(), Rect::from_min_size(pos2(r.left() + 4.0, r.top()), vec2(40.0, r.height())), lw, t.text);
                    ui.painter().text(
                        pos2(r.left() + 52.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        crate::i18n::t(&lw.name()),
                        crate::theme::body(),
                        t.text,
                    );
                    if resp.clicked() {
                        let v = match lw {
                            Lineweight::Mm100(v) => json!(f64::from(v) / 100.0),
                            _ => json!("Default"),
                        };
                        actions.push(("layer.set", json!({ "name": name, "lineweight": v })));
                        ui.close();
                    }
                }
            });
        });
    // Transparency (commits when the drag or edit ends).
    let tid = egui::Id::new(("lpm_tr", &name));
    let mut tr = ui.data_mut(|d| d.get_temp::<u8>(tid)).unwrap_or(l.transparency);
    let r = ui.add(egui::DragValue::new(&mut tr).range(0..=90).speed(0.5));
    if r.dragged() || r.has_focus() {
        ui.data_mut(|d| d.insert_temp(tid, tr));
    }
    if (r.drag_stopped() || r.lost_focus()) && tr != l.transparency {
        actions.push(("layer.set", json!({ "name": name, "transparency": tr })));
        ui.data_mut(|d| d.remove::<u8>(tid));
    } else if !r.dragged() && !r.has_focus() {
        ui.data_mut(|d| d.remove::<u8>(tid));
    }
    // VP Freeze / VP Color (inside a layout viewport).
    if let Some((h, v)) = vp {
        let vf = v.frozen_layers.iter().any(|f| f.eq_ignore_ascii_case(&name));
        if icons::button(ui, if vf { Icon::Snowflake } else { Icon::Sun }, 20.0, "Freeze/Thaw in the current viewport", false).clicked() {
            let key = if vf { "thaw" } else { "freeze" };
            actions.push(("vplayer", json!({ "handle": h.hex(), key: [name] })));
        }
        let vc = v.layer_colors.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)).map(|(_, c)| *c);
        ui.horizontal(|ui| {
            if let Some(c) = color_button(ui, egui::Id::new(("lpm_vpcolor", &name)), vc.unwrap_or(l.color), false, 96.0) {
                actions.push(("vplayer", json!({ "handle": h.hex(), "colors": { name.clone(): c.name() } })));
            }
            if vc.is_some() && ui.small_button("×").on_hover_text(crate::tl!("Remove the viewport override")).clicked() {
                actions.push(("vplayer", json!({ "handle": h.hex(), "colors": { name.clone(): Value::Null } })));
            }
        });
    }
    // New VP Freeze.
    let nvf = icons::button(ui, if l.vp_freeze_new { Icon::Snowflake } else { Icon::Sun }, 20.0, "Freeze in new viewports", false);
    {
        // A small "new" mark distinguishes it from the Freeze column.
        let r = nvf.rect;
        ui.painter().rect_filled(Rect::from_min_size(r.right_bottom() - vec2(7.0, 7.0), vec2(6.0, 6.0)), 1.0, t.icon_accent);
    }
    if nvf.clicked() {
        actions.push(("layer.set", json!({ "name": name, "newVpFreeze": !l.vp_freeze_new })));
    }
    // Description (double-click to edit).
    match st.describing.clone() {
        Some((n, mut buf)) if n.eq_ignore_ascii_case(&name) => {
            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(200.0).id(egui::Id::new(("lpm_desc", &name))));
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.lost_focus() {
                if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && buf != l.description {
                    actions.push(("layer.set", json!({ "name": name, "description": buf })));
                }
                st.describing = None;
            } else {
                st.describing = Some((n, buf));
            }
        }
        _ => {
            let text = if l.description.is_empty() { RichText::new("—").color(t.text_faint) } else { RichText::new(&l.description) };
            let r = ui.add_sized([200.0, 20.0], egui::Label::new(text).sense(Sense::click()).truncate());
            if r.on_hover_text(crate::tl!("Double-click to edit the description")).double_clicked() {
                st.describing = Some((name.clone(), l.description.clone()));
            }
        }
    }
}

/// Lower-case names of layers with objects on them.
fn used_layers(d: &cadaddy_doc::Drawing) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let ents = d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).chain(d.blocks.values().flat_map(|b| b.entities.iter()));
    for e in ents.take(5_000_000) {
        if !set.contains(&e.common.layer.to_ascii_lowercase()) {
            set.insert(e.common.layer.to_ascii_lowercase());
        }
    }
    set
}

/// Layer search: plain text matches anywhere; `*` / `?` are wildcards.
fn crate_wild(q: &str, name: &str) -> bool {
    if !q.contains('*') && !q.contains('?') {
        return name.contains(q);
    }
    wild(q.as_bytes(), name.as_bytes())
}

fn wild(p: &[u8], t: &[u8]) -> bool {
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some(pi);
                pi += 1;
                mark = ti;
            }
            Some(&c) if c == b'?' || Some(&c) == t.get(ti) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += 1;
                    ti = mark;
                }
                None => return false,
            },
        }
    }
    p.iter().skip(pi).all(|c| *c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_search() {
        assert!(crate_wild("wal", "walls"));
        assert!(crate_wild("w*s", "walls"));
        assert!(crate_wild("?alls", "walls"));
        assert!(!crate_wild("x*", "walls"));
    }

    /// Issue #29: just opening the dialog must not grow its window frame after frame.
    #[test]
    fn dialog_keeps_its_height() {
        let mut session = cadaddy_engine::Session::new();
        session.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
        let mut app = CadApp::new(session, crate::Services::default());
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 1000.0));
        let mut heights = Vec::new();
        for _ in 0..30 {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let mut open = true;
                dialog(&mut app, ui.ctx(), &mut open);
            });
            out.textures_delta.clear();
            let id = egui::LayerId::new(egui::Order::Middle, egui::Id::new("lpm_window"));
            heights.push(ctx.memory(|m| m.area_rect(id.id)).map_or(0.0, |r| r.height()));
        }
        let (first, last) = (heights[5], heights[29]);
        assert!(first > 0.0 && first < 700.0 && (last - first).abs() < 0.5, "window height drifted: {heights:?}");
    }
}
