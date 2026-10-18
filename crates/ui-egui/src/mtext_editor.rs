//! The in-place MTEXT editor: while MTEXT asks for its text, a text box opens on the canvas at
//! the text's location and width with a formatting toolbar (font, height, bold, italic,
//! underline, overline, colour, alignment, lists, tabs) and a ruler for the paragraph's indents
//! and tab stops. The text and its formats live in the engine's
//! [`cadaddy_engine::mtext_edit::Formatted`]; this file only draws and forwards edits.
//!
//! The box shows the text laid out as the MTEXT object will be: the final object's own layout
//! ([`cadaddy_render::layout_mtext_entity`]: its font and weights, height, width, columns,
//! justification, indents, tab stops and line spacing), drawn at the box's scale. The caret, the
//! selection and the pointer follow that layout ([`caret::CaretMap`]); the keyboard goes to an
//! egui text field that isn't shown (typing, deleting, Left/Right, the clipboard, undo).

use cadaddy_engine::mtext_edit::{CharFmt, Flag, Formatted, ListStyle, MTextColor, ParaAlign, TabKind};
use cadaddy_fonts::MTextLayout;
use cadaddy_geom::Vec2;
use egui::text::{CCursor, CCursorRange};
use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, pos2, vec2};

mod caret;

use crate::CadApp;
use crate::theme::Tokens;

/// Smallest and largest size of the text in the box, in points: the box stays readable at any zoom.
const MIN_PX: f32 = 13.0;
const MAX_PX: f32 = 36.0;
/// Families the font list offers besides the one set in the text.
const FAMILIES: [&str; 6] = ["Arial", "Times New Roman", "Courier New", "Verdana", "Georgia", "Calibri"];
/// The standard colours (ACI 1..=7) of the colour menu.
const SWATCHES: [u16; 7] = [1, 2, 3, 4, 5, 6, 7];
/// Ruler snap, in text heights.
const SNAP: f64 = 0.25;

#[derive(Clone)]
struct State {
    fmt: Formatted,
    /// Format of the next typed character when nothing is selected.
    typing: CharFmt,
    /// Caret the `typing` format was set for.
    typing_at: usize,
    /// Height of the toolbar and ruler above the text box (last frame).
    top: f32,
    /// Selection (characters) of the last frame, for toolbar actions of this one.
    sel: (usize, usize),
    /// A caret position to set on the text box in the next frame (after a toolbar insertion).
    caret: Option<usize>,
    /// The text box has taken the keyboard once (with the caret at the end): typing goes into
    /// the drawing's text, never to the command line behind the editor.
    focused: bool,
    /// Up/Down keep the caret's x across lines of different lengths (text frame).
    keep_x: Option<f64>,
    /// Where a pointer drag in the box started (caret position).
    anchor: Option<usize>,
}

fn state_id() -> egui::Id {
    egui::Id::new("mtext_editor_state")
}

fn edit_id() -> egui::Id {
    egui::Id::new("mtext_editor_text")
}

/// The ACI index or true colour as a screen colour.
fn color32(c: MTextColor) -> Color32 {
    let rgb = match c {
        MTextColor::Aci(v) => cadaddy_color::aci_rgb(u8::try_from(v).unwrap_or(7)),
        MTextColor::Rgb(v) => cadaddy_color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
    };
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// Whether the in-place editor is showing.
pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<State>(state_id()).is_some())
}

/// Show the editor while MTEXT collects its text; clears its state when it isn't running.
pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let mtext = app.session.editing_mtext();
    let asking = app.session.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point);
    let (Some(mt), true) = (mtext, asking) else {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
        return;
    };
    let t = Tokens::get();
    let (rect, scale) = app.canvas.xf.map_or((ctx.content_rect(), 20.0), |x| (x.rect, x.scale.max(1e-6)));
    let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).unwrap_or_else(|| {
        // An existing MTEXT (TEXTEDIT) opens with its text and formatting.
        let mut fmt = if mt.contents.is_empty() { Formatted::default() } else { Formatted::from_contents(&mt.contents) };
        if let Some(f) = app.session.doc().ok().and_then(|d| d.text_style(&mt.style).map(|s| s.font.clone())) {
            // A family name, not a font file (`txt.shx`, `arial.ttf`).
            if !f.is_empty() && !f.contains('.') {
                fmt.base_font = f;
            }
        }
        State { fmt, typing: CharFmt::default(), typing_at: 0, top: 70.0, sel: (0, 0), caret: None, focused: false, keep_x: None, anchor: None }
    });
    let base_h = if mt.height > 0.0 { mt.height } else { 0.2 };
    // The text as the object will lay it out, and the box's scale: a readable size for the text
    // height, smaller when the object is wider than the screen allows.
    let lay = layout(app.session.doc().ok(), &mt, &st.fmt);
    let content_w = if lay.bounds.is_empty() { 0.0 } else { lay.bounds.width() }.max(mt.width.max(0.0));
    let mut k = f64::from(((base_h * scale) as f32).clamp(MIN_PX, MAX_PX)) / base_h;
    let max_w = f64::from(rect.width().max(280.0) * 0.9);
    if content_w * k > max_w && content_w > 0.0 {
        k = max_w / content_w;
    }
    let px = (base_h * k) as f32;
    let width = ((content_w * k) as f32 + 2.0 * BOX_MARGIN).max(280.0);
    // The box's top-left corner on screen (rotation is ignored while editing).
    let code = usize::from(mt.attach.clamp(1, 9) - 1);
    let (col, row) = ((code % 3) as f64, (code / 3) as f64);
    let ww = f64::from(width) / scale;
    let corner = cadaddy_geom::Vec2::new(mt.insert.x - col * ww / 2.0, mt.insert.y + row * base_h * 2.0);
    let at = app.canvas.xf.map_or(rect.center(), |x| x.w2s(corner));
    let max_x = (rect.right() - width - 8.0).max(rect.left());
    let pos = pos2(at.x.clamp(rect.left(), max_x), at.y.clamp(rect.top() + st.top, (rect.bottom() - 120.0).max(rect.top() + st.top)));

    let mut done = Done::No;
    let top_start = pos.y - st.top;
    egui::Area::new(egui::Id::new("mtext_editor_area")).order(egui::Order::Foreground).fixed_pos(pos2(pos.x, top_start)).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.accent)).inner_margin(egui::Margin::same(0)).show(ui, |ui| {
            ui.set_width(width);
            let y0 = ui.cursor().top();
            toolbar(ui, &mut st, base_h);
            ruler(ui, &mut st, px, width, base_h);
            st.top = ui.cursor().top() - y0;
            text_box(ui, &mut st, &lay, k, width, &t);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if ui.button(crate::tl!("OK")).clicked() {
                    done = Done::Ok;
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    done = Done::Cancel;
                }
                let finish = if ui.ctx().os().is_mac() { crate::tl!("⌘↩ to finish") } else { crate::tl!("Ctrl+Enter to finish") };
                ui.label(RichText::new(finish).small().color(t.text_dim));
            });
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        done = Done::Cancel;
    } else if ctx.input(|i| {
        // Ctrl/⌘+Enter, as the frame's modifiers or the key press itself carries them.
        i.events.iter().any(
            |e| matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.command || i.modifiers.command),
        )
    }) {
        done = Done::Ok;
    }
    match done {
        Done::No => {
            ctx.data_mut(|d| d.insert_temp(state_id(), st));
        }
        Done::Ok => {
            ctx.data_mut(|d| d.remove::<State>(state_id()));
            // The editor holds the whole contents: one input, then Enter ends MTEXT's text entry.
            if !st.fmt.is_empty() {
                let _ = app.session.input(cadaddy_engine::Input::Text(st.fmt.to_contents()));
            }
            let running = app.session.running.as_ref().map(|r| r.id.clone());
            if running.as_deref() == Some("mtext") {
                let _ = app.session.input(cadaddy_engine::Input::Enter);
            } else if running.as_deref() == Some("textedit") && st.fmt.is_empty() {
                // Emptied: keep the text as it was (Enter at "Enter new text").
                let _ = app.session.input(cadaddy_engine::Input::Enter);
            }
        }
        Done::Cancel => {
            ctx.data_mut(|d| d.remove::<State>(state_id()));
            app.session.cancel();
        }
    }
}

#[derive(PartialEq)]
enum Done {
    No,
    Ok,
    Cancel,
}

/// Change the selected characters' format, or the next typed ones' when nothing is selected.
fn change(st: &mut State, f: impl Fn(&mut CharFmt)) {
    let (a, b) = st.sel;
    if a == b {
        f(&mut st.typing);
    } else {
        st.fmt.edit(a.min(b)..a.max(b), f);
    }
}

fn current(st: &State) -> CharFmt {
    let (a, b) = st.sel;
    if a == b { st.typing.clone() } else { st.fmt.fmt_at(a.min(b)) }
}

fn toggle(st: &mut State, flag: Flag) {
    let (a, b) = st.sel;
    if a == b {
        let on = !flag_of(&st.typing, flag);
        change(st, |c| set_flag(c, flag, on));
    } else {
        st.fmt.toggle(a.min(b)..a.max(b), flag);
    }
}

fn flag_of(c: &CharFmt, f: Flag) -> bool {
    match f {
        Flag::Bold => c.bold,
        Flag::Italic => c.italic,
        Flag::Underline => c.underline,
        Flag::Overline => c.overline,
    }
}

fn set_flag(c: &mut CharFmt, f: Flag, on: bool) {
    match f {
        Flag::Bold => c.bold = on,
        Flag::Italic => c.italic = on,
        Flag::Underline => c.underline = on,
        Flag::Overline => c.overline = on,
    }
}

fn toolbar(ui: &mut egui::Ui, st: &mut State, base_h: f64) {
    let cur = current(st);
    let range = st.sel.0.min(st.sel.1)..st.sel.0.max(st.sel.1);
    let mut refocus = false;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(4.0);
        // Font
        let shown = cur.font.clone().unwrap_or_else(|| st.fmt.base_font.clone());
        egui::ComboBox::from_id_salt("mtext_font").width(120.0).selected_text(shown.clone()).show_ui(ui, |ui| {
            for fam in FAMILIES.iter().map(|s| s.to_string()).chain(cur.font.clone().filter(|f| !FAMILIES.contains(&f.as_str()))) {
                if ui.selectable_label(shown == fam, &fam).clicked() {
                    change(st, |c| c.font = Some(fam.clone()));
                    refocus = true;
                }
            }
        });
        // Height
        let mut h = cur.height.unwrap_or(base_h);
        if ui
            .add(egui::DragValue::new(&mut h).speed(base_h * 0.02).range(base_h * 0.05..=base_h * 50.0).max_decimals(4))
            .on_hover_text(crate::tl!("Text height"))
            .changed()
        {
            let none = (h - base_h).abs() < 1e-9;
            change(st, |c| c.height = (!none).then_some(h));
        }
        ui.separator();
        for (flag, label, tip) in [
            (Flag::Bold, RichText::new("B").strong(), crate::tl!("Bold")),
            (Flag::Italic, RichText::new("I").italics(), crate::tl!("Italic")),
            (Flag::Underline, RichText::new("U").underline(), crate::tl!("Underline")),
            (Flag::Overline, RichText::new("O").strikethrough(), crate::tl!("Overline")),
        ] {
            if ui.selectable_label(flag_of(&cur, flag), label).on_hover_text(tip).clicked() {
                toggle(st, flag);
                refocus = true;
            }
        }
        // Colour
        let swatch = cur.color.map_or(ui.visuals().text_color(), color32);
        ui.menu_button(RichText::new("A▾").color(swatch).strong(), |ui| {
            if ui.button(crate::tl!("ByLayer")).clicked() {
                change(st, |c| c.color = None);
                refocus = true;
                ui.close();
            }
            ui.horizontal(|ui| {
                for v in SWATCHES {
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                    ui.painter().rect_filled(r, 2.0, color32(MTextColor::Aci(v)));
                    if resp.clicked() {
                        change(st, |c| c.color = Some(MTextColor::Aci(v)));
                        refocus = true;
                        ui.close();
                    }
                }
            });
            let mut rgb = cur.color.map_or([255, 255, 255], |c| {
                let k = color32(c);
                [k.r(), k.g(), k.b()]
            });
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                let v = (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
                change(st, |c| c.color = Some(MTextColor::Rgb(v)));
            }
        })
        .response
        .on_hover_text(crate::tl!("Text color"));
        ui.separator();
        // Paragraph alignment
        let para = st.fmt.para(st.fmt.paragraph_of(range.start));
        for (al, glyph, tip) in [
            (ParaAlign::Left, "⇤", crate::tl!("Left")),
            (ParaAlign::Center, "↔", crate::tl!("Center")),
            (ParaAlign::Right, "⇥", crate::tl!("Right")),
            (ParaAlign::Justify, "≡", crate::tl!("Justify")),
        ] {
            let on = para.align == al;
            if ui.selectable_label(on, glyph).on_hover_text(tip).clicked() {
                st.fmt.edit_paras(range.clone(), |p| p.align = if on { ParaAlign::Default } else { al });
                refocus = true;
            }
        }
        ui.separator();
        // Lists and special characters
        if ui.button("•≡").on_hover_text(crate::tl!("Bulleted list")).clicked() {
            st.fmt.make_list(range.clone(), ListStyle::Bullet, 3.0 * base_h);
            refocus = true;
        }
        if ui.button("1≡").on_hover_text(crate::tl!("Numbered list")).clicked() {
            st.fmt.make_list(range.clone(), ListStyle::Number, 3.0 * base_h);
            refocus = true;
        }
        for (glyph, tip, code) in [
            ("⇥|", crate::tl!("Insert tab"), "\t"),
            ("°", crate::tl!("Degree"), "°"),
            ("±", crate::tl!("Plus/minus"), "±"),
            ("⌀", crate::tl!("Diameter"), "⌀"),
        ] {
            if ui.button(glyph).on_hover_text(tip).clicked() {
                insert(st, code);
                refocus = true;
            }
        }
    });
    if refocus {
        ui.ctx().memory_mut(|m| m.request_focus(edit_id()));
    }
}

/// Insert `s` at the caret (over the selection).
fn insert(st: &mut State, s: &str) {
    let (a, b) = (st.sel.0.min(st.sel.1), st.sel.0.max(st.sel.1));
    let chars: Vec<char> = st.fmt.text().chars().collect();
    let new: String = chars.iter().take(a).chain(s.chars().collect::<Vec<_>>().iter()).chain(chars.iter().skip(b)).collect();
    if st.fmt.set_text(&new, &st.typing.clone()) {
        let at = a + s.chars().count();
        st.sel = (at, at);
        st.typing_at = at;
        st.caret = Some(at);
    }
}

/// The ruler: `px` points per text height (`base_h` drawing units); paragraph indents and tab
/// stops are in drawing units, as `\p` codes are.
fn ruler(ui: &mut egui::Ui, st: &mut State, px: f32, width: f32, base_h: f64) {
    let t = Tokens::get();
    let range = st.sel.0.min(st.sel.1)..st.sel.0.max(st.sel.1);
    let p = st.fmt.para(st.fmt.paragraph_of(range.start));
    let (rect, bg) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, t.control);
    let tick = |u: f64| rect.left() + (u as f32) * px;
    let x_of = |v: f64| tick(v / base_h);
    let units = (f64::from(width) / f64::from(px)).floor() as i32;
    for u in 0..=units.clamp(0, 400) {
        let x = tick(f64::from(u));
        let major = u % 5 == 0;
        painter.line_segment([pos2(x, rect.bottom() - if major { 7.0 } else { 3.0 }), pos2(x, rect.bottom())], Stroke::new(1.0, t.text_faint));
        if major && u > 0 {
            painter.text(pos2(x + 2.0, rect.top() + 1.0), Align2::LEFT_TOP, u.to_string(), FontId::proportional(8.0), t.text_faint);
        }
    }
    let to_units = |x: f32| ((f64::from(x - rect.left()) / f64::from(px) / SNAP).round() * SNAP).clamp(0.0, 400.0) * base_h;
    // Tab stops: an L at the stop; a right-click removes one.
    for (pos, _kind) in &p.tabs {
        let x = x_of(*pos);
        painter.add(Shape::line(
            vec![pos2(x, rect.bottom() - 9.0), pos2(x, rect.bottom() - 2.0), pos2(x + 4.0, rect.bottom() - 2.0)],
            Stroke::new(1.5, t.accent),
        ));
    }
    let hover = bg.hover_pos();
    // Indent markers: the first line's above, the left indent's below.
    let first_abs = p.left + p.first;
    let mut drag = None;
    for (k, (u, up)) in [(first_abs, false), (p.left, true)].into_iter().enumerate() {
        let x = x_of(u);
        let y = if up { rect.bottom() - 1.0 } else { rect.top() + 1.0 };
        let dir = if up { -1.0 } else { 1.0 };
        let hit = Rect::from_center_size(pos2(x, y + dir * 4.0), vec2(10.0, 10.0));
        let r = ui.interact(hit, ui.id().with(("mtext_indent", k)), Sense::drag());
        let col = if r.hovered() || r.dragged() { t.accent } else { t.icon };
        painter.add(Shape::convex_polygon(vec![pos2(x - 4.0, y), pos2(x + 4.0, y), pos2(x, y + dir * 8.0)], col, Stroke::NONE));
        if r.dragged()
            && let Some(m) = r.interact_pointer_pos()
        {
            drag = Some((k, to_units(m.x)));
        }
    }
    if let Some((k, u)) = drag {
        if k == 0 {
            st.fmt.set_indents(range.clone(), u - p.left, p.left);
        } else {
            st.fmt.set_indents(range.clone(), p.first, u);
        }
    } else if bg.clicked()
        && let Some(m) = hover
    {
        st.fmt.add_tab(range.clone(), to_units(m.x), TabKind::Left);
    } else if bg.secondary_clicked()
        && let Some(m) = hover
    {
        st.fmt.remove_tab(range, to_units(m.x), 0.6 * base_h);
    }
    bg.on_hover_text(crate::tl!("Click the ruler to add a tab stop, right-click to remove one"));
}

/// The editor's text laid out as the MTEXT object will be (at the origin, unrotated: the text's
/// own frame), with the drawing's text style when there is a drawing.
fn layout(d: Option<&cadaddy_engine::doc::Drawing>, mt: &cadaddy_engine::doc::MText, fmt: &Formatted) -> MTextLayout {
    let mut t = mt.clone();
    t.contents = fmt.to_contents();
    t.insert = cadaddy_geom::Vec3::ZERO;
    t.rotation = 0.0;
    match d {
        Some(d) => cadaddy_render::layout_mtext_entity(d, &t),
        None => cadaddy_fonts::layout_mtext(&t.contents, Vec2::ZERO, t.height, t.width, t.attach, 0.0, t.line_spacing),
    }
}

/// Margin round the text in the box, in points.
const BOX_MARGIN: f32 = 6.0;

/// The text box: the laid-out text at `k` points per drawing unit, the selection and the caret
/// placed on it, the pointer and Up/Down/Home/End handled on it; the keyboard otherwise goes to
/// a text field off screen.
fn text_box(ui: &mut egui::Ui, st: &mut State, lay: &MTextLayout, k: f64, width: f32, t: &Tokens) {
    let id = edit_id();
    let mut buf = st.fmt.text().to_string();
    let map = caret::CaretMap::new(&buf, lay.carets.clone());
    // The text frame on screen: the layout's top-left corner at the box's, plus a margin.
    let (bx0, top) = if lay.bounds.is_empty() { (0.0, 0.0) } else { (lay.bounds.min.x, lay.bounds.max.y) };
    let line_px = (lay.carets.first().map_or(1.0, |l| l.height) * k * 5.0 / 3.0) as f32;
    let content_h = if lay.bounds.is_empty() { 0.0 } else { (lay.bounds.height() * k) as f32 };
    let height = (content_h + line_px).max(line_px * 5.0) + 2.0 * BOX_MARGIN;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let origin = rect.min + vec2(BOX_MARGIN, BOX_MARGIN);
    let to_screen = |q: Vec2| pos2(origin.x + ((q.x - bx0) * k) as f32, origin.y + ((top - q.y) * k) as f32);
    let to_local = |p: Pos2| Vec2::new(bx0 + f64::from(p.x - origin.x) / k, top - f64::from(p.y - origin.y) / k);

    // The caret and selection as the field has them, changed by the pointer and the keys the
    // layout answers before the field sees the frame's input.
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let n = buf.chars().count();
    let mut range = state.cursor.char_range().map_or((n, n), |r| (r.secondary.index.0.min(n), r.primary.index.0.min(n)));
    let mut moved = false;
    if let Some(at) = st.caret.take() {
        range = (at.min(n), at.min(n));
        moved = true;
    }
    if !st.focused {
        // Opened on existing text (TEXTEDIT): the caret goes to its end, as AutoCAD's editor.
        range = (n, n);
        moved = true;
    }
    let shift = ui.input(|i| i.modifiers.shift);
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on() || resp.drag_stopped() || resp.clicked()) {
        let at = map.hit(to_local(p));
        if resp.drag_started() || resp.clicked() || st.anchor.is_none() {
            st.anchor = Some(if shift { range.0 } else { at });
        }
        range = (st.anchor.unwrap_or(at), at);
        st.keep_x = None;
        moved = true;
    }
    if !resp.is_pointer_button_down_on() {
        st.anchor = None;
    }
    let has_focus = ui.memory(|m| m.has_focus(id));
    if has_focus {
        let keys = [egui::Key::ArrowUp, egui::Key::ArrowDown, egui::Key::Home, egui::Key::End];
        for key in keys {
            let pressed = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key) || i.consume_key(egui::Modifiers::SHIFT, key));
            if !pressed {
                continue;
            }
            let cur = range.1;
            let next = match key {
                egui::Key::ArrowUp | egui::Key::ArrowDown => {
                    let x = st.keep_x.or_else(|| map.pos(cur).map(|p| p.0)).unwrap_or(0.0);
                    st.keep_x = Some(x);
                    map.vertical(cur, if key == egui::Key::ArrowUp { -1 } else { 1 }, x)
                }
                egui::Key::Home => map.line_start(cur),
                _ => map.line_end(cur),
            };
            if !matches!(key, egui::Key::ArrowUp | egui::Key::ArrowDown) {
                st.keep_x = None;
            }
            range = (if shift { range.0 } else { next }, next);
            moved = true;
        }
    }
    if moved {
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(range.0), CCursor::new(range.1))));
        state.store(ui.ctx(), id);
    }

    // The keyboard: an egui text field off screen (it is never drawn where it could be seen).
    let out = egui::Area::new(egui::Id::new("mtext_editor_keyboard"))
        .order(egui::Order::Background)
        .constrain(false)
        .fixed_pos(pos2(-100_000.0, -100_000.0))
        .show(ui.ctx(), |ui| {
            egui::TextEdit::multiline(&mut buf)
                .id(id)
                .desired_width(200.0)
                .lock_focus(true)
                .char_limit(cadaddy_engine::mtext_edit::MAX_CHARS)
                .show(ui)
        })
        .inner;
    if !st.focused || moved || (!out.response.has_focus() && buf.is_empty() && st.fmt.is_empty()) || resp.clicked() || resp.drag_started() {
        out.response.request_focus();
    }
    st.focused = true;
    if out.response.changed() {
        let typing = st.typing.clone();
        st.fmt.set_text(&buf, &typing);
        st.keep_x = None;
        // The layout follows next frame.
        ui.ctx().request_repaint();
    }

    // Draw: the field, the selection, the text as laid out, the caret.
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, t.field);
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let sel = out.cursor_range.map_or(range, |r| (r.secondary.index.0, r.primary.index.0));
    // Positions from this frame's layout: the text may have just changed (next frame catches up).
    let fresh = map.len() == buf.chars().count() + 1;
    if fresh {
        for (x0, x1, base, h) in map.spans(sel.0, sel.1) {
            let r = Rect::from_two_pos(to_screen(Vec2::new(x0, base + h * 1.25)), to_screen(Vec2::new(x1, base - h * 0.45)));
            painter.rect_filled(r, 0.0, t.select_bg);
        }
    }
    for piece in &lay.pieces {
        let color = piece.color.map_or(t.text, color32);
        for s in &piece.shaped.strokes {
            if s.len() >= 2 {
                painter.add(Shape::line(s.iter().map(|q| to_screen(*q)).collect(), Stroke::new(1.2, color)));
            }
        }
        for glyph in &piece.shaped.glyphs {
            let rings: Vec<Vec<Vec2>> =
                glyph.iter().map(|ring| ring.iter().map(|q| to_screen(*q)).map(|p| Vec2::new(f64::from(p.x), f64::from(p.y))).collect()).collect();
            let tris = cadaddy_render::triangulate_evenodd(&rings);
            let mut mesh = egui::Mesh::default();
            for tri in tris.as_chunks::<3>().0 {
                let Ok(i) = u32::try_from(mesh.vertices.len()) else { break };
                for v in tri {
                    mesh.colored_vertex(pos2(v.x as f32, v.y as f32), color);
                }
                mesh.add_triangle(i, i + 1, i + 2);
            }
            if !mesh.is_empty() {
                painter.add(Shape::mesh(mesh));
            }
        }
    }
    if fresh
        && out.response.has_focus()
        && let Some((x, base, h)) = map.pos(sel.1)
    {
        painter.line_segment([to_screen(Vec2::new(x, base + h * 1.2)), to_screen(Vec2::new(x, base - h * 0.4))], Stroke::new(1.5, t.accent));
    }
    if let Some(r) = out.cursor_range {
        let (a, b) = (r.primary.index.0, r.secondary.index.0);
        st.sel = (a, b);
        if a == b {
            if out.response.changed() {
                st.typing_at = a;
            } else if a != st.typing_at {
                // The caret moved: typing continues the formatting of the character before it.
                st.typing = st.fmt.fmt_at(a.saturating_sub(1));
                st.typing_at = a;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;

    use crate::Services;

    #[test]
    fn editor_opens_only_for_mtext_text_entry_and_finishes_with_formatted_contents() {
        let ctx = egui::Context::default();
        let mut app = CadApp::new(Session::new(), Services::default());
        let frame = |app: &mut CadApp| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))), ..Default::default() };
            for _ in 0..2 {
                let mut out = ctx.run_ui(input.clone(), |ui| show(app, ui.ctx()));
                out.textures_delta.clear();
            }
        };
        frame(&mut app);
        assert!(ctx.data_mut(|d| d.get_temp::<State>(state_id())).is_none());
        app.start("mtext");
        assert!(app.session.editing_mtext().is_none(), "no box yet");
        app.session.input(cadaddy_engine::Input::Point(cadaddy_geom::Vec2::new(1.0, 1.0))).unwrap();
        app.session.input(cadaddy_engine::Input::Point(cadaddy_geom::Vec2::new(6.0, 0.0))).unwrap();
        let m = app.session.editing_mtext().expect("box known");
        assert!((m.width - 5.0).abs() < 1e-9, "{}", m.width);
        frame(&mut app);
        let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).expect("editor open");
        st.fmt.set_text("hello", &CharFmt::default());
        st.fmt.toggle(0..5, Flag::Bold);
        let contents = st.fmt.to_contents();
        app.session.input(cadaddy_engine::Input::Text(contents.clone())).unwrap();
        app.session.input(cadaddy_engine::Input::Enter).unwrap();
        let d = app.session.doc().unwrap();
        let found = d.model.iter().any(|e| matches!(&e.kind, cadaddy_engine::doc::EntityKind::MText(m) if m.contents == contents));
        assert!(found, "{contents}");
    }

    #[test]
    fn textedit_opens_the_editor_on_an_existing_mtext_with_its_formatting() {
        let ctx = egui::Context::default();
        let mut app = CadApp::new(Session::new(), Services::default());
        let frame = |app: &mut CadApp| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))), ..Default::default() };
            for _ in 0..2 {
                let mut out = ctx.run_ui(input.clone(), |ui| show(app, ui.ctx()));
                out.textures_delta.clear();
            }
        };
        let r = app.session.execute("mtext", &serde_json::json!({ "at": [0, 10], "text": "{\\fArial|b1;OLD} NOTE", "height": 2 })).unwrap();
        let h = cadaddy_engine::doc::Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
        app.start("textedit");
        app.session.input(cadaddy_engine::Input::Point(cadaddy_geom::Vec2::new(1.0, 9.0))).unwrap();
        let m = app.session.editing_mtext().expect("TEXTEDIT reports the mtext it edits");
        assert!(m.contents.contains("OLD"));
        frame(&mut app);
        let mut st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).expect("editor open");
        assert_eq!(st.fmt.text(), "OLD NOTE", "the editor starts from the text");
        assert!(st.fmt.fmt_at(0).bold, "with its formatting");
        st.fmt.set_text("NEW NOTE", &CharFmt::default());
        app.session.input(cadaddy_engine::Input::Text(st.fmt.to_contents())).unwrap();
        let d = app.session.doc().unwrap();
        match &d.entity(h).unwrap().kind {
            cadaddy_engine::doc::EntityKind::MText(m) => assert!(m.contents.contains("NEW NOTE"), "{}", m.contents),
            k => panic!("{k:?}"),
        }
    }

    /// Opened on existing text, the editor has the keyboard with the caret at the end: typing
    /// adds to the text (battle test P1: it went to the command line and replaced it all).
    #[test]
    fn textedit_typing_goes_into_the_editor_at_the_end() {
        let ctx = egui::Context::default();
        let mut app = CadApp::new(Session::new(), Services::default());
        let screen = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0)));
        let run = |app: &mut CadApp, events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: screen, events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| show(app, ui.ctx()));
            out.textures_delta.clear();
        };
        let r = app.session.execute("mtext", &serde_json::json!({ "at": [0, 10], "text": "HELLO WORLD", "height": 2 })).unwrap();
        let h = cadaddy_engine::doc::Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
        app.start("textedit");
        app.session.input(cadaddy_engine::Input::Point(cadaddy_geom::Vec2::new(1.0, 9.0))).unwrap();
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        assert!(ctx.egui_wants_keyboard_input(), "the editor has the keyboard, not the command line");
        run(&mut app, vec![egui::Event::Text(" AGAIN".into())]);
        let st = ctx.data_mut(|d| d.get_temp::<State>(state_id())).expect("editor open");
        assert_eq!(st.fmt.text(), "HELLO WORLD AGAIN");
        let ctrl = egui::Modifiers { command: true, ctrl: true, ..Default::default() };
        let input = egui::RawInput {
            screen_rect: screen,
            events: vec![
                egui::Event::ModifiersChanged(ctrl),
                egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: ctrl },
            ],
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let d = app.session.doc().unwrap();
        match &d.entity(h).unwrap().kind {
            cadaddy_engine::doc::EntityKind::MText(m) => assert!(m.contents.contains("HELLO WORLD AGAIN"), "{}", m.contents),
            k => panic!("{k:?}"),
        }
    }
}
