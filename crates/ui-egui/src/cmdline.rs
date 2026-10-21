//! The floating command line: history, the active prompt with clickable keywords, typed input
//! (keyboard focus stays on the drawing — typing anywhere goes here) and AutoComplete.

use cadaddy_engine::Input;
use egui::{Key, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

#[derive(Default)]
pub struct CmdLine {
    pub buffer: String,
    /// The selection as of the last frame, to notice when it changes.
    seen_selection: Vec<cadaddy_doc::Handle>,
    /// Something was typed at the command line since the selection was made.
    typed_since_selection: bool,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    /// What was typed at command prompts (not command names), oldest first: the in-command
    /// shortcut menu's Recent Input.
    pub recent_input: Vec<String>,
    /// The expanded history window is open (F2, the chevron, or a click on the history lines).
    pub expanded: bool,
    /// Dragging the history's top edge: (pointer y, line count) when the drag started.
    resize_from: Option<(f32, usize)>,
    /// Dragging an end of the bar: (pointer x, width, -1 left end / 1 right end) at the start.
    width_from: Option<(f32, f32, f32)>,
    /// The Dynamic Input options menu at the cursor (Down Arrow at a prompt with keywords).
    pub options: Option<crate::dyninput::OptionsMenu>,
    /// The text whose second Dynamic Input value Tab locked; the lock holds while it is unchanged.
    locked_second: Option<String>,
    /// Where the options menu's rows were drawn last frame (screen), for clicks on them.
    pub option_rows: Vec<(Rect, String)>,
    /// Where the Dynamic Input tooltip stays while the options menu is open.
    pub options_anchor: Option<Pos2>,
    /// An input method's composition (Chinese, Japanese, Korean) not yet committed: shown
    /// underlined after what's typed; while it is open the IME has the editing keys.
    pub preedit: String,
    /// Where the composition sits in the bar as last drawn (screen), for the IME's candidate
    /// window.
    ime_area: Option<Rect>,
}

/// The most history lines the command line shows above its bar.
pub const MAX_LINES: usize = 12;
const LINE_H: f32 = 16.0;

/// The Dynamic Input frame for the prompt on screen, or `None` when the value boxes are not live
/// (Dynamic Input off, no point prompt or a pending selection window).
pub fn dyn_frame(app: &CadApp) -> Option<crate::dyninput::Frame> {
    let st = &app.session.settings;
    if !st.dynmode || app.session.pending_window.is_some() {
        return None;
    }
    let p = app.session.current_prompt().filter(cadaddy_engine::pointmod::accepts)?;
    Some(crate::dyninput::Frame { base: p.base, cartesian: st.dynpi_cartesian, absolute: st.dynpi_absolute })
}

/// Take Tab away from egui before it moves keyboard focus to the next widget (which would leave
/// the command line deaf to typing). While no widget has focus, a pressed Tab becomes a `"\t"`
/// text event, which keeps its place among the other keystrokes; [`keyboard`] handles it.
pub fn capture_tab(ctx: &egui::Context, raw: &mut egui::RawInput) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    raw.events.retain_mut(|e| {
        let tab = match e {
            egui::Event::Key { key: Key::Tab, pressed, .. } => Some(*pressed),
            _ => None,
        };
        match tab {
            Some(true) => {
                *e = egui::Event::Text("\t".into());
                true
            }
            Some(false) => false,
            None => true,
        }
    });
}

/// Tab on the command line: cycle the object snaps under the cursor, type the Dynamic Input
/// separator (`,` or `<`) after a value, or complete a command name.
fn tab(app: &mut CadApp) {
    // Grip dimensional input: Tab locks the typed value and moves to the next field.
    if crate::grip_dyn::tab(app) {
        return;
    }
    // Over a snap marker with nothing typed, Tab cycles the candidate snaps.
    if app.cmd.buffer.is_empty() && app.canvas.snap.is_some() {
        app.canvas.snap_tab = true;
    } else if let Some(f) = dyn_frame(app)
        && let Some(sep) = crate::dyninput::tab_separator(&app.cmd.buffer, &f)
    {
        app.cmd.buffer.push(sep);
    } else if let Some(f) = dyn_frame(app)
        && crate::dyninput::can_lock_second(&app.cmd.buffer, &f)
    {
        // Both values typed: Tab locks the second one as well.
        app.cmd.locked_second = Some(app.cmd.buffer.clone());
    } else if let Some(s) = current_suggestions(app).first() {
        app.cmd.buffer = s.id.clone();
    }
}

pub use crate::autocomplete::Suggestion;

/// Command names and aliases for `prefix` with the default settings and no usage counts.
#[cfg(test)]
fn suggestions(prefix: &str) -> Vec<Suggestion> {
    crate::autocomplete::complete_commands(prefix, &crate::autocomplete::Settings::default(), &crate::autocomplete::Usage::default(), &[])
}

/// The AutoComplete list for what is typed now: commands, aliases and system variables at the
/// command prompt (ranked by how often each command was used); at a prompt asking for a named
/// object, the drawing's names of that kind.
pub fn current_suggestions(app: &CadApp) -> Vec<Suggestion> {
    let set = app.ui.autocomplete;
    if !set.enabled || app.cmd.buffer.is_empty() {
        return Vec::new();
    }
    if !app.session.prompting() {
        let vars: Vec<String> = match cadaddy_engine::sysvars::list(&app.session) {
            serde_json::Value::Object(m) if set.sysvars => m.keys().cloned().collect(),
            _ => Vec::new(),
        };
        return crate::autocomplete::complete_commands(&app.cmd.buffer, &set, &app.ui.command_usage, &vars);
    }
    let Some(p) = app.session.current_prompt().filter(|p| p.accept.text) else { return Vec::new() };
    let (Some(kind), Ok(d)) = (crate::autocomplete::named_kind(&p.message), app.session.doc()) else { return Vec::new() };
    crate::autocomplete::complete_names(&app.cmd.buffer, kind, &crate::autocomplete::names(d, kind), set.mid_string)
}

impl CmdLine {
    /// Track the selection: a new selection starts with nothing typed since.
    pub fn watch_selection(&mut self, sel: &[cadaddy_doc::Handle]) {
        if self.seen_selection != sel {
            self.seen_selection = sel.to_vec();
            self.typed_since_selection = false;
        }
    }

    /// Note that the command line holds typed text.
    pub fn note_typing(&mut self) {
        if !self.buffer.is_empty() {
            self.typed_since_selection = true;
        }
    }

    /// Tab locked the second Dynamic Input value and nothing was typed since.
    pub fn second_locked(&self) -> bool {
        self.locked_second.as_deref() == Some(self.buffer.as_str())
    }

    /// The options menu row under `at` (screen), if the menu is open.
    pub fn option_at(&self, at: Pos2) -> Option<String> {
        self.options.as_ref()?;
        self.option_rows.iter().find(|(r, _)| r.contains(at)).map(|(_, k)| k.clone())
    }
}

/// Keep the options menu only while its prompt is on screen with Dynamic Input and DYNPROMPT on.
fn options_still_fit(app: &mut CadApp) {
    let st = &app.session.settings;
    let on = st.dynmode && st.dynprompt && app.canvas.hot_grips.is_empty();
    let kws = app.session.current_prompt().map(|p| p.keywords).unwrap_or_default();
    if app.cmd.options.as_ref().is_some_and(|m| !on || !m.fits(&kws)) {
        app.cmd.options = None;
    }
}

/// Down Arrow at a prompt with keywords (Dynamic Input on, DYNPROMPT on, not browsing the
/// history): open the options menu at the cursor, or move its highlight. `false` when the key is
/// left for the history.
fn options_key(app: &mut CadApp, step: isize) -> bool {
    if let Some(m) = app.cmd.options.as_mut() {
        m.step(step);
        return true;
    }
    let st = &app.session.settings;
    if step < 0 || !st.dynmode || !st.dynprompt || app.cmd.history_pos.is_some() || !app.canvas.hot_grips.is_empty() {
        return false;
    }
    let kws = app.session.current_prompt().map(|p| p.keywords).unwrap_or_default();
    app.cmd.options = crate::dyninput::OptionsMenu::open(&kws);
    app.cmd.options.is_some()
}

/// Send a keyword picked from the options menu, as a click on it at the command line does.
pub fn pick_option(app: &mut CadApp, keyword: String) {
    app.cmd.options = None;
    app.cmd.buffer.clear();
    if let Err(e) = app.session.input(Input::Keyword(keyword)) {
        app.session.echo(e.user_message());
    }
}

/// Whether Backspace should erase the selection instead of editing the command line. The key
/// labelled "delete" on a Mac sends Backspace, so on macOS it erases selected objects, but only
/// when it can't be meant for the command line: a fresh, plain press (never an auto-repeat, so
/// holding it to clear typing can't run on into the drawing, and not Option/Cmd+delete, which
/// delete a word or line of text on macOS), no command running, the command line empty, and
/// nothing typed there since the selection was made.
pub fn backspace_erases(mac: bool, press: KeyPress, buffer: &str, command_running: bool, has_selection: bool, typed_since_selection: bool) -> bool {
    mac && press.plain && !press.repeat && buffer.is_empty() && !command_running && has_selection && !typed_since_selection
}

/// How a key was pressed.
#[derive(Clone, Copy, Debug)]
pub struct KeyPress {
    /// The operating system's auto-repeat while the key is held.
    pub repeat: bool,
    /// No Option, Control or Command held (Shift doesn't matter).
    pub plain: bool,
}

/// Whether typing goes to the command line: no text field has focus, the in-place text editor
/// is closed (an Enter here would answer TEXTEDIT's prompt behind it with the old text) and no
/// modal dialog is up (the save-changes question when closing: an Enter here would repeat the
/// last command behind it).
fn has_keyboard(app: &CadApp, ctx: &egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input()
        && !crate::mtext_editor::is_open(ctx)
        && app.close_prompt().is_none()
        && ctx.memory(|m| m.top_modal_layer()).is_none()
}

/// Turn the system input method on for the command line, its candidate window at `area`: egui
/// enables IMEs only while something says where text goes (its text fields do), so without
/// this Chinese, Japanese and Korean input never reached the command line. Not on the web,
/// where it would move the keyboard to the page's hidden text field, away from the drawing.
fn allow_ime(ctx: &egui::Context, area: Rect) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let caret = Rect::from_x_y_ranges(area.right() - 1.0..=area.right(), area.y_range());
    ctx.output_mut(|o| {
        o.ime =
            Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Normal, rect: area, cursor_rect: caret, should_interrupt_composition: false });
    });
}

/// An input method event at the command line: the composition shows as it changes; committed
/// text is typed (a committed space at a prompt that doesn't take text enters, like Space).
fn ime_event(app: &mut CadApp, ev: &egui::ImeEvent, text_prompt: bool) {
    match ev {
        egui::ImeEvent::Preedit { text, .. } => {
            if !text.is_empty() {
                app.cmd.options = None;
            }
            app.cmd.preedit.clone_from(text);
        }
        egui::ImeEvent::Commit(text) => {
            app.cmd.preedit.clear();
            if text.is_empty() {
                return;
            }
            app.cmd.options = None;
            if text == " " && !text_prompt {
                submit(app);
            } else {
                app.cmd.buffer.push_str(text);
            }
        }
        _ => {}
    }
}

/// Handle keyboard input destined for the command line (when no text field has focus).
pub fn keyboard(app: &mut CadApp, ctx: &egui::Context) {
    if !has_keyboard(app, ctx) {
        // A composition left open belongs to whatever has the keyboard now.
        app.cmd.preedit.clear();
        return;
    }
    let area = app.cmd.ime_area.unwrap_or_else(|| {
        let r = ctx.content_rect();
        Rect::from_center_size(pos2(r.center().x, r.bottom() - 22.0), vec2(2.0, 24.0))
    });
    allow_ime(ctx, area);
    let events = ctx.input(|i| i.events.clone());
    let selection = app.session.selection();
    app.cmd.watch_selection(&selection);
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let text_prompt = app.session.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point && !p.accept.number);
    options_still_fit(app);
    for ev in events {
        if let egui::Event::Ime(ime) = &ev {
            ime_event(app, ime, text_prompt);
            app.cmd.note_typing();
            continue;
        }
        // While a composition is open, Enter, Esc, Backspace, Tab and the arrows are the IME's
        // (choosing, cancelling, editing it): Windows marks them processed and egui drops them,
        // and other systems don't send them, but none may run a command or cancel one here.
        if !app.cmd.preedit.is_empty() && matches!(ev, egui::Event::Key { .. } | egui::Event::Text(_)) {
            continue;
        }
        // The options menu takes the arrows, Enter and Escape; typing closes it.
        if app.cmd.options.is_some() {
            let space = matches!(&ev, egui::Event::Text(t) if t == " ") && !text_prompt;
            match &ev {
                _ if space => {
                    if let Some(k) = app.cmd.options.as_ref().and_then(|m| m.current()).map(str::to_owned) {
                        pick_option(app, k);
                    }
                    continue;
                }
                egui::Event::Key { key: Key::Enter, pressed: true, .. } => {
                    if let Some(k) = app.cmd.options.as_ref().and_then(|m| m.current()).map(str::to_owned) {
                        pick_option(app, k);
                    }
                    continue;
                }
                egui::Event::Key { key: Key::Escape, pressed: true, .. } => {
                    app.cmd.options = None;
                    continue;
                }
                egui::Event::Key { key: Key::ArrowUp, pressed: true, modifiers, .. } if !modifiers.any() => {
                    options_key(app, -1);
                    continue;
                }
                egui::Event::Key { key: Key::ArrowDown | Key::ArrowUp, pressed: true, .. } => {}
                // Space arrives as a key and as text; the text picks.
                egui::Event::Key { key: Key::Space, .. } => {}
                egui::Event::Text(_) | egui::Event::Key { pressed: true, .. } => app.cmd.options = None,
                _ => {}
            }
        }
        match ev {
            egui::Event::Text(t) => {
                if t == "\t" {
                    tab(app);
                } else if t == " " && !text_prompt {
                    submit(app);
                } else {
                    app.cmd.buffer.push_str(&t);
                }
            }
            egui::Event::Key { key, pressed: true, repeat, modifiers, .. } => match key {
                Key::Enter => submit(app),
                Key::Escape => {
                    // With nothing to cancel, Esc leaves walk mode: the 3D view doesn't get keys
                    // while the command line has them.
                    let idle = app.cmd.buffer.is_empty() && !app.session.prompting();
                    app.cmd.buffer.clear();
                    app.session.cancel();
                    if idle && app.walk.is_some() {
                        crate::walk::close(app);
                    }
                }
                Key::Backspace => {
                    let press = KeyPress { repeat, plain: !(modifiers.alt || modifiers.ctrl || modifiers.command) };
                    let erase = backspace_erases(
                        mac,
                        press,
                        &app.cmd.buffer,
                        app.session.prompting(),
                        !app.session.selection().is_empty(),
                        app.cmd.typed_since_selection,
                    );
                    if erase {
                        let _ = app.run("erase.selection", serde_json::json!({}));
                    } else {
                        app.cmd.buffer.pop();
                    }
                }
                Key::ArrowUp if !modifiers.any() => {
                    if !app.cmd.history.is_empty() {
                        let n = app.cmd.history.len();
                        let pos = app.cmd.history_pos.map(|p| p.saturating_sub(1)).unwrap_or(n - 1);
                        app.cmd.history_pos = Some(pos);
                        app.cmd.buffer = app.cmd.history.get(pos).cloned().unwrap_or_default();
                    }
                }
                Key::ArrowDown if !modifiers.any() => {
                    if !options_key(app, 1)
                        && let Some(p) = app.cmd.history_pos
                    {
                        let np = p + 1;
                        if np >= app.cmd.history.len() {
                            app.cmd.history_pos = None;
                            app.cmd.buffer.clear();
                        } else {
                            app.cmd.history_pos = Some(np);
                            app.cmd.buffer = app.cmd.history.get(np).cloned().unwrap_or_default();
                        }
                    }
                }
                Key::Tab => tab(app),
                Key::Delete if app.cmd.buffer.is_empty() && !app.session.prompting() => {
                    let _ = app.run("erase.selection", serde_json::json!({}));
                }
                _ => {}
            },
            _ => {}
        }
        app.cmd.note_typing();
    }
}

pub fn submit(app: &mut CadApp) {
    // Grip dimensional input: the typed lengths and angles place the grip.
    if crate::grip_dyn::submit(app) {
        return;
    }
    // Dynamic Input: a split entry (`40,30`, `40<90`, `40,`) is completed from the cursor and
    // measured from the last point.
    if let Some(f) = dyn_frame(app) {
        let cursor = app.canvas.cursor.unwrap_or(app.session.cursor);
        let last = app.session.last_point;
        let done = crate::dyninput::parse(&app.cmd.buffer, &f).and_then(|e| crate::dyninput::complete(&e, &f, cursor, last));
        if let Some(text) = done {
            app.cmd.buffer = text;
        }
    }
    let text = std::mem::take(&mut app.cmd.buffer);
    app.cmd.locked_second = None;
    app.cmd.history_pos = None;
    if !text.trim().is_empty() && !app.session.prompting() {
        // How often each command is typed ranks AutoComplete.
        let id = cadaddy_engine::cmd::resolve_alias(&text.trim().to_ascii_lowercase());
        if let Some(c) = cadaddy_engine::cmd::find_command(&id) {
            app.ui.command_usage.note(c.id);
        }
        app.cmd.history.push(text.clone());
        if app.cmd.history.len() > 200 {
            app.cmd.history.remove(0);
        }
    }
    crate::context_menu::note_input(app, &text);
    if text.is_empty() {
        if let Err(e) = app.session.input(Input::Enter) {
            app.session.echo(e.user_message());
        }
        return;
    }
    app.cmdline(&text);
}

/// The command bar's width in a drawing area `canvas_w` wide: the width it was dragged to (a
/// fraction of the drawing area, kept with the interface preferences), else about half of it.
pub fn bar_width(app: &CadApp, canvas_w: f32) -> f32 {
    let room = (canvas_w - 8.0).max(0.0);
    match app.ui.cmdline_width.filter(|f| f.is_finite()) {
        Some(f) => (canvas_w * f.clamp(0.2, 1.0)).clamp(240.0_f32.min(room), room),
        None => (canvas_w * 0.48).clamp(360.0, 760.0).min(room),
    }
}

/// How opaque the command line's backgrounds are (its Transparency setting).
fn opacity(app: &CadApp) -> f32 {
    1.0 - f32::from(app.ui.cmdline_transparency.min(80)) / 100.0
}

fn visible_lines(app: &CadApp, canvas: Rect, bar: Rect) -> usize {
    let available = ((bar.top() - canvas.top() - 8.0).max(0.0) / LINE_H) as usize;
    app.ui.history_lines.min(MAX_LINES).min(available)
}

/// The command bar's rectangle in a drawing area.
fn bar_rect(app: &CadApp, canvas: Rect) -> Rect {
    let w = bar_width(app, canvas.width());
    let h = 24.0;
    Rect::from_min_size(pos2(canvas.center().x - w / 2.0, canvas.bottom() - h - 10.0), vec2(w, h))
}

/// How much of the drawing area's bottom the floating command line and its history cover, in
/// points: zooming to the extents keeps the drawing above it.
pub fn covered_bottom(app: &CadApp, canvas: Rect) -> f32 {
    if !app.ui.show_command_line {
        return 0.0;
    }
    let bar = bar_rect(app, canvas);
    let n = if app.cmd.expanded { 0 } else { visible_lines(app, canvas, bar).min(app.session.log.len()) };
    let history = if n > 0 { 16.0 * n as f32 + 4.0 } else { 0.0 };
    (canvas.bottom() - bar.top() + history).max(0.0)
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui, canvas: Rect) {
    let t = Tokens::get();
    let bar = bar_rect(app, canvas);
    let (w, h) = (bar.width(), bar.height());
    let p = ui.painter_at(canvas);
    history(app, ui, canvas, bar);
    // History lines above the bar (the expanded window replaces them).
    let n = if app.cmd.expanded { 0 } else { visible_lines(app, canvas, bar) };
    let lines: Vec<String> = app.session.log.iter().rev().take(n).rev().cloned().collect();
    if !lines.is_empty() {
        let lh = 16.0;
        let hr = Rect::from_min_size(pos2(bar.left(), bar.top() - lh * lines.len() as f32 - 4.0), vec2(w, lh * lines.len() as f32 + 2.0));
        p.rect_filled(hr, 3.0, t.cmd_history.gamma_multiply(opacity(app)));
        // A right-to-left interface reads the history from the right.
        let (x, align) = if app.rtl() { (hr.right() - 8.0, egui::Align2::RIGHT_TOP) } else { (hr.left() + 8.0, egui::Align2::LEFT_TOP) };
        for (i, l) in lines.iter().enumerate() {
            p.text(pos2(x, hr.top() + 1.0 + lh * i as f32), align, crate::i18n::t_engine(l), crate::theme::small(), t.text_dim);
        }
    }
    p.rect_filled(bar, 3.0, t.cmd_bg.gamma_multiply(opacity(app)));
    p.rect_stroke(bar, 3.0, Stroke::new(1.0, t.cmd_border), egui::StrokeKind::Inside);
    // Prompt glyph.
    p.text(pos2(bar.left() + 8.0, bar.center().y), egui::Align2::LEFT_CENTER, ">_", crate::theme::mono(), t.text_dim);
    // The prompt and what's typed scroll left inside the bar, like a text field, when they don't
    // fit (PURGE's keyword list ran past the bar over the drawing); the bar shows the whole
    // prompt on hover then.
    let text_left = bar.left() + 30.0;
    let text_right = bar.right() - 26.0;
    let full = format!("{}{}{}", app.session.prompt_text(), app.cmd.buffer, app.cmd.preedit);
    let full_w = p.layout_no_wrap(full.clone(), crate::theme::body(), t.text).size().x + 8.0;
    let overflow = (full_w - (text_right - text_left)).max(0.0);
    let pt = p.with_clip_rect(Rect::from_x_y_ranges(text_left..=text_right, bar.y_range()));
    if overflow > 0.0 {
        ui.interact(bar, ui.id().with("cmd_bar_hover"), Sense::hover()).on_hover_text(full);
    }
    let mut x = text_left - overflow;
    let prompt = app.session.current_prompt();
    let font = crate::theme::body();
    match &prompt {
        Some(pr) => {
            // The grip modes name themselves ("** STRETCH **…").
            let name = app.session.running.as_ref().filter(|r| r.id != "grip").map(|r| r.id.to_ascii_uppercase() + " ").unwrap_or_default();
            let g = p.layout_no_wrap(name, font.clone(), t.text_faint);
            let gw = g.size().x;
            pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text_faint);
            x += gw;
            // A LISP `get…` prompt shows as the routine wrote it (no keywords, default or colon added).
            let message = if pr.raw { pr.display() } else { crate::i18n::t_engine(&pr.message).into_owned() };
            let g = p.layout_no_wrap(message, font.clone(), t.text);
            let gw = g.size().x;
            pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
            x += gw;
            if !pr.raw && !pr.keywords.is_empty() {
                let g = p.layout_no_wrap(if pr.message.is_empty() { " [".into() } else { crate::tl!(" or [").into() }, font.clone(), t.text);
                let gw = g.size().x;
                pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
                x += gw;
                let kws = pr.keywords.clone();
                for (i, k) in kws.iter().enumerate() {
                    let g = p.layout_no_wrap(k.clone(), font.clone(), t.cmd_keyword);
                    let r = Rect::from_min_size(pos2(x, bar.top() + 3.0), vec2(g.size().x, h - 6.0));
                    // Only a keyword inside the bar takes the pointer: one scrolled out of it would
                    // sit over the drawing and swallow its clicks.
                    let resp = (r.left() >= text_left && r.right() <= text_right).then(|| ui.interact(r, ui.id().with(("kw", i)), Sense::click()));
                    if let Some(resp) = &resp {
                        crate::icons::describe_control(ui, resp, k, None, false);
                    }
                    if resp.as_ref().is_some_and(egui::Response::hovered) {
                        pt.rect_filled(r, 2.0, t.cmd_keyword_hover);
                    }
                    let gw = g.size().x;
                    pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.cmd_keyword);
                    x += gw;
                    if resp.as_ref().is_some_and(egui::Response::clicked) {
                        let _ = app.session.input(Input::Keyword(k.clone()));
                    }
                    if i + 1 < kws.len() {
                        let g = p.layout_no_wrap("/".into(), font.clone(), t.text);
                        let gw = g.size().x;
                        pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
                        x += gw;
                    }
                }
                let g = p.layout_no_wrap("]".into(), font.clone(), t.text);
                let gw = g.size().x;
                pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
                x += gw;
            }
            if let Some(d) = pr.default.as_ref().filter(|_| !pr.raw) {
                let g = p.layout_no_wrap(format!(" <{d}>"), font.clone(), t.text);
                let gw = g.size().x;
                pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
                x += gw;
            }
            if !pr.raw {
                let g = p.layout_no_wrap(": ".into(), font.clone(), t.text);
                let gw = g.size().x;
                pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
                x += gw;
            } else if !pr.message.ends_with(' ') {
                x += 4.0;
            }
        }
        None => {
            if app.cmd.buffer.is_empty() && app.cmd.preedit.is_empty() {
                pt.text(
                    pos2(x, bar.center().y),
                    egui::Align2::LEFT_CENTER,
                    crate::tl!("Type a command"),
                    egui::FontId::proportional(12.5),
                    t.text_faint,
                );
            }
        }
    }
    let g = p.layout_no_wrap(app.cmd.buffer.clone(), font.clone(), t.text);
    let mut gw = g.size().x;
    pt.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text);
    // An input method's composition, underlined after what's typed; the IME's candidate window
    // opens over it.
    let compose_x = x + gw;
    if !app.cmd.preedit.is_empty() {
        let g = p.layout_no_wrap(app.cmd.preedit.clone(), font, t.text);
        let pw = g.size().x;
        let y = bar.center().y + g.size().y / 2.0;
        pt.galley(pos2(compose_x, bar.center().y - g.size().y / 2.0), g, t.text);
        pt.line_segment([pos2(compose_x, y), pos2(compose_x + pw, y)], Stroke::new(1.0, t.text));
        gw += pw;
    }
    let ime_area = Rect::from_x_y_ranges(compose_x.clamp(text_left, text_right)..=(x + gw + 2.0).clamp(text_left, text_right), bar.y_range());
    app.cmd.ime_area = Some(ime_area);
    if has_keyboard(app, ui.ctx()) {
        allow_ime(ui.ctx(), ime_area);
    }
    // Caret.
    let blink = (ui.input(|i| i.time) * 2.0).floor() as i64 % 2 == 0;
    if blink {
        pt.line_segment([pos2(x + gw + 1.0, bar.top() + 5.0), pos2(x + gw + 1.0, bar.bottom() - 5.0)], Stroke::new(1.0, t.text));
    }
    // History chevron.
    crate::icons::paint(
        &p,
        Rect::from_center_size(pos2(bar.right() - 14.0, bar.center().y), vec2(14.0, 14.0)),
        if app.cmd.expanded { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronUp },
        false,
    );
    // AutoComplete list.
    {
        let sug = current_suggestions(app);
        if !sug.is_empty() {
            let rh = 20.0;
            let lr = Rect::from_min_size(
                pos2(
                    bar.left() + 24.0,
                    bar.top() - 6.0 - rh * sug.len() as f32 - (if lines.is_empty() { 0.0 } else { 16.0 * lines.len() as f32 + 6.0 }),
                ),
                vec2((bar.width() - 24.0).max(0.0), rh * sug.len() as f32),
            );
            p.rect_filled(lr, 3.0, t.list_bg);
            p.rect_stroke(lr, 3.0, Stroke::new(1.0, t.cmd_border), egui::StrokeKind::Inside);
            for (i, s) in sug.iter().enumerate() {
                let r = Rect::from_min_size(pos2(lr.left(), lr.top() + rh * i as f32), vec2(lr.width(), rh));
                let resp = ui.interact(r, ui.id().with(("sug", i)), Sense::click());
                crate::icons::describe_control(ui, &resp, &s.display, None, false);
                if resp.hovered() || i == 0 {
                    p.rect_filled(r.shrink(1.0), 2.0, if resp.hovered() { t.cmd_keyword_hover } else { t.list_row });
                }
                p.text(Pos2::new(r.left() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, &s.display, crate::theme::body(), t.text);
                p.text(
                    Pos2::new(r.right() - 8.0, r.center().y),
                    egui::Align2::RIGHT_CENTER,
                    crate::i18n::t(&s.label),
                    crate::theme::small(),
                    t.text_faint,
                );
                if resp.clicked() {
                    match s.kind {
                        crate::autocomplete::Kind::Command => {
                            app.cmd.buffer.clear();
                            app.ui.command_usage.note(&s.id);
                            app.start(&s.id.to_ascii_lowercase());
                        }
                        // A variable or a name: as if typed and entered.
                        _ => {
                            app.cmd.buffer = s.id.clone();
                            submit(app);
                        }
                    }
                }
            }
        }
    }
}

/// The command line's areas, laid out like [`show`] draws them.
struct Zones {
    /// The inline history lines (when there are any and the window is closed).
    lines: Option<Rect>,
    /// The expanded history window.
    window: Option<Rect>,
    chevron: Rect,
    /// The top edge that resizes the inline history.
    grip: Rect,
    /// Everything, AutoComplete list included: kept away from the drawing underneath.
    all: Rect,
}

fn zones(app: &CadApp, canvas: Rect, bar: Rect) -> Zones {
    let n = if app.cmd.expanded { 0 } else { visible_lines(app, canvas, bar).min(app.session.log.len()) };
    let lines =
        (n > 0).then(|| Rect::from_min_size(pos2(bar.left(), bar.top() - LINE_H * n as f32 - 4.0), vec2(bar.width(), LINE_H * n as f32 + 2.0)));
    let window = app.cmd.expanded.then(|| {
        // As tall as the log (at least three lines), at most half the drawing area.
        let full = LINE_H * app.session.log.len().max(3) as f32 + 4.0;
        let h = full.min((canvas.height() * 0.5).max(3.0 * LINE_H));
        Rect::from_min_size(pos2(bar.left(), bar.top() - h - 4.0), vec2(bar.width(), h + 2.0))
    });
    let top = window.or(lines).map_or(bar.top(), |r| r.top());
    let grip = Rect::from_min_max(pos2(bar.left(), top - 3.0), pos2(bar.right(), top + 3.0));
    let mut all = bar.union(grip);
    {
        let k = current_suggestions(app).len();
        if k > 0 {
            let above = if n == 0 { 0.0 } else { LINE_H * n as f32 + 6.0 };
            all = all.union(Rect::from_min_size(pos2(bar.left(), bar.top() - 6.0 - 20.0 * k as f32 - above), vec2(bar.width(), 1.0)));
        }
    }
    Zones { lines, window, chevron: Rect::from_center_size(pos2(bar.right() - 14.0, bar.center().y), vec2(24.0, 22.0)), grip, all }
}

/// The pointer on the command line: one widget under all of it keeps clicks, drags and the wheel
/// off the drawing (the keywords and AutoComplete rows sit on top of it). Clicking the history
/// lines or the chevron opens/closes the history window (`ui.toggle.history`, also F2); dragging
/// the top edge sets how many lines show (`ui.cmdline.lines`).
fn history(app: &mut CadApp, ui: &mut egui::Ui, canvas: Rect, bar: Rect) {
    let z = zones(app, canvas, bar);
    let resp = ui.interact(z.all, ui.id().with("cmdline"), Sense::click_and_drag());
    crate::icons::describe_area(&resp, "Command line");
    let at = resp.hover_pos().or(resp.interact_pointer_pos());
    let on_grip = |p: Pos2| z.grip.contains(p) && z.window.is_none();
    // The bar's ends: dragging them sets the command line's width (both ends move, it stays
    // centred).
    let side = |p: Pos2| -> Option<f32> {
        let band = bar.y_range();
        if !band.contains(p.y) {
            return None;
        }
        if (p.x - bar.left()).abs() <= 4.0 {
            Some(-1.0)
        } else if (p.x - bar.right()).abs() <= 4.0 {
            Some(1.0)
        } else {
            None
        }
    };
    if app.cmd.width_from.is_some() || at.and_then(side).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
        && let Some(dir) = side(o)
    {
        app.cmd.width_from = Some((o.x, bar.width(), dir));
    }
    if let Some((x0, w0, dir)) = app.cmd.width_from {
        if let (true, Some(p)) = (resp.dragged(), resp.interact_pointer_pos()) {
            let w = w0 + 2.0 * dir * (p.x - x0);
            if canvas.width() > 0.0 {
                app.ui.cmdline_width = Some((w / canvas.width()).clamp(0.2, 1.0));
            }
        } else {
            app.cmd.width_from = None;
        }
    }
    let on_toggle = |p: Pos2| z.chevron.contains(p) || z.lines.is_some_and(|r| r.contains(p));
    if app.cmd.resize_from.is_some() || at.is_some_and(on_grip) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    } else if at.is_some_and(on_toggle) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.drag_started()
        && app.cmd.width_from.is_none()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
        && on_grip(o)
    {
        app.cmd.resize_from = Some((o.y, visible_lines(app, canvas, bar)));
    }
    if let Some((y0, n0)) = app.cmd.resize_from {
        if let (true, Some(p)) = (resp.dragged(), resp.interact_pointer_pos()) {
            let n = (n0 as f32 + (y0 - p.y) / LINE_H).round().clamp(0.0, MAX_LINES as f32) as usize;
            if n != app.ui.history_lines {
                let _ = app.run("ui.cmdline.lines", serde_json::json!({ "lines": n }));
            }
        } else {
            app.cmd.resize_from = None;
        }
    }
    if resp.clicked() && resp.interact_pointer_pos().is_some_and(on_toggle) {
        let _ = app.run("ui.toggle.history", serde_json::json!({}));
    }
    crate::shortcut_menus::attach(app, &resp, crate::shortcut_menus::command_line_entries);
    if let Some(r) = zones(app, canvas, bar).window {
        history_window(app, ui, r);
    }
}

/// The expanded history: the whole command log, scrollable, newest at the bottom.
fn history_window(app: &CadApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get();
    ui.painter().rect_filled(r, 3.0, t.cmd_bg.gamma_multiply(opacity(app)));
    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let inner = r.shrink2(vec2(8.0, 3.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    child.spacing_mut().item_spacing.y = 0.0;
    let log = &app.session.log;
    let rtl = app.rtl();
    egui::ScrollArea::vertical().id_salt("cmd_history").stick_to_bottom(true).auto_shrink([false, false]).show_rows(
        &mut child,
        LINE_H,
        log.len(),
        |ui, rows| {
            for l in log.get(rows).into_iter().flatten() {
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), LINE_H), Sense::hover());
                let p = ui.painter().with_clip_rect(row.intersect(ui.clip_rect()));
                let (at, align) = if rtl {
                    (pos2(row.right() - 4.0, row.center().y), egui::Align2::RIGHT_CENTER)
                } else {
                    (pos2(row.left(), row.center().y), egui::Align2::LEFT_CENTER)
                };
                p.text(at, align, crate::i18n::t_engine(l), crate::theme::small(), t.text_dim);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use egui::{Event, PointerButton};

    #[test]
    fn compact_canvas_contains_history_controls_without_changing_line_preference() {
        let mut app = CadApp::new(Session::empty(), crate::Services::default());
        app.ui.history_lines = 12;
        for i in 0..20 {
            app.session.echo(format!("line {i}"));
        }
        let canvas = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 180.0));
        let w = bar_width(&app, canvas.width());
        let bar = Rect::from_min_size(pos2(canvas.center().x - w / 2.0, canvas.bottom() - 34.0), vec2(w, 24.0));
        let z = zones(&app, canvas, bar);
        assert!(canvas.contains_rect(bar));
        assert!(canvas.contains_rect(z.chevron));
        assert!(z.lines.is_some_and(|r| canvas.contains_rect(r)));
        assert!(canvas.contains_rect(z.grip));
        assert_eq!(app.ui.history_lines, 12);
    }

    fn frame(app: &mut CadApp, ctx: &egui::Context, events: Vec<Event>) -> egui::FullOutput {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        out
    }

    fn preedit(text: &str) -> Event {
        Event::Ime(egui::ImeEvent::Preedit { text: text.into(), active_range_chars: None })
    }

    fn commit(text: &str) -> Event {
        Event::Ime(egui::ImeEvent::Commit(text.into()))
    }

    fn key(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn ime_composition_commits_into_the_command_line() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, Vec::new());
        // The command line turns the IME on, its candidate window at the bar on screen.
        let out = frame(&mut app, &ctx, Vec::new());
        let ime = out.platform_output.ime.expect("the command line enables the IME");
        let bar = app.canvas.rect.unwrap_or(Rect::NOTHING);
        assert!(bar.contains(ime.rect.center()) && ime.rect.center().y > bar.center().y, "{:?} in {bar:?}", ime.rect);
        // Preedit shows, isn't typed yet; the commit is.
        frame(&mut app, &ctx, vec![preedit("ce")]);
        assert_eq!((app.cmd.preedit.as_str(), app.cmd.buffer.as_str()), ("ce", ""));
        assert!(!app.needs_cjk, "pinyin letters need no CJK font");
        let at = frame(&mut app, &ctx, vec![preedit("测试")]).platform_output.ime.map(|i| i.rect);
        assert!(app.needs_cjk, "a composition in Chinese loads the CJK interface fonts (an English interface showed boxes)");
        assert!(at.is_some_and(|r| r.width() > 10.0), "the candidate window sits over the composition: {at:?}");
        frame(&mut app, &ctx, vec![preedit(""), commit("测试")]);
        assert_eq!((app.cmd.preedit.as_str(), app.cmd.buffer.as_str()), ("", "测试"));
        // A cancelled composition leaves nothing behind (Windows ends it with an empty commit).
        frame(&mut app, &ctx, vec![preedit("か")]);
        frame(&mut app, &ctx, vec![preedit(""), commit("")]);
        assert_eq!((app.cmd.preedit.as_str(), app.cmd.buffer.as_str()), ("", "测试"));
        // Committed text is typed text: Enter runs it.
        app.cmd.buffer.clear();
        frame(&mut app, &ctx, vec![preedit("circle"), commit("circle"), key(Key::Enter)]);
        assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("circle"));
        app.session.cancel();
    }

    #[test]
    fn a_dialog_text_box_keeps_its_own_ime() {
        // A dialog's text box (an egui text field) composes for itself: the command line takes
        // neither the composition nor the IME's position while the box has the keyboard.
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        let ctx = egui::Context::default();
        let mut name = String::new();
        let mut run = |app: &mut CadApp, events: Vec<Event>| {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
                egui::Window::new("Layer name").fixed_pos(pos2(200.0, 200.0)).show(ui.ctx(), |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut name).id(egui::Id::new("dialog_name")));
                    r.request_focus();
                });
            });
            out.textures_delta.clear();
            out.platform_output.ime
        };
        run(&mut app, Vec::new());
        run(&mut app, Vec::new());
        run(&mut app, vec![preedit("じ")]);
        let ime = run(&mut app, vec![preedit("字"), preedit(""), commit("字")]);
        assert!(app.cmd.preedit.is_empty() && app.cmd.buffer.is_empty(), "nothing reached the command line");
        // The box is near the dialog's corner (200, 200), the command line at the bottom.
        assert!(ime.is_some_and(|i| i.rect.top() < 400.0 && i.rect.left() < 700.0), "the box's IME: {ime:?}");
        assert_eq!(name, "字");
    }

    #[test]
    fn keys_during_a_composition_are_the_imes() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, Vec::new());
        app.cmdline("line");
        assert!(app.session.prompting());
        // Esc, Enter, Backspace, Tab and arrows while composing neither cancel LINE nor edit.
        app.cmd.buffer = "1,".into();
        frame(&mut app, &ctx, vec![preedit("に")]);
        for k in [Key::Escape, Key::Enter, Key::Backspace, Key::ArrowUp, Key::ArrowDown] {
            frame(&mut app, &ctx, vec![key(k)]);
        }
        // Tab reaches the command line as text (`capture_tab`), as does Space.
        frame(&mut app, &ctx, vec![Event::Text("	".into()), Event::Text(" ".into())]);
        assert!(app.session.prompting(), "LINE still running");
        assert_eq!((app.cmd.preedit.as_str(), app.cmd.buffer.as_str()), ("に", "1,"));
        // The IME cancels its composition (Esc there); the next Esc is the command line's.
        frame(&mut app, &ctx, vec![preedit("")]);
        assert!(app.session.prompting());
        frame(&mut app, &ctx, vec![key(Key::Escape)]);
        assert!(!app.session.prompting(), "Esc after the composition cancels LINE");
        assert!(app.cmd.buffer.is_empty());
    }

    fn press(app: &mut CadApp, ctx: &egui::Context, at: Pos2, pressed: bool) {
        frame(app, ctx, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() }]);
    }

    #[test]
    fn history_expands_by_click_chevron_f2_and_resizes_by_drag_without_touching_the_drawing() {
        let mut app = CadApp::new(Session::empty(), crate::Services::default());
        app.start("ui.sample");
        for i in 0..20 {
            app.session.echo(format!("line {i}"));
        }
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        let canvas = app.canvas.rect.unwrap_or(Rect::NOTHING);
        let w = bar_width(&app, canvas.width());
        let bar = Rect::from_min_size(pos2(canvas.center().x - w / 2.0, canvas.bottom() - 34.0), vec2(w, 24.0));
        let z = zones(&app, canvas, bar);
        let Some(lines) = z.lines else { panic!("three history lines show by default") };
        // A click on the history lines opens the history window, and doesn't start a selection.
        for (at, pressed) in [(lines.center(), true), (lines.center(), false)] {
            frame(&mut app, &ctx, vec![Event::PointerMoved(at)]);
            press(&mut app, &ctx, at, pressed);
        }
        frame(&mut app, &ctx, Vec::new());
        assert!(app.cmd.expanded);
        assert!(app.session.pending_window.is_none(), "the click stayed on the command line");
        // The chevron closes it; F2 and TEXTSCR open it.
        let chevron = z.chevron.center();
        press(&mut app, &ctx, chevron, true);
        press(&mut app, &ctx, chevron, false);
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.cmd.expanded);
        let f2 = |pressed| Event::Key { key: Key::F2, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        frame(&mut app, &ctx, vec![f2(true), f2(false)]);
        assert!(app.cmd.expanded);
        assert_eq!(app.run("ui.toggle.history", serde_json::json!({"on": false})), Ok(serde_json::json!({"on": false})));
        app.cmdline("textscr");
        assert!(app.cmd.expanded);
        let _ = app.run("ui.toggle.history", serde_json::json!({"on": false}));
        frame(&mut app, &ctx, Vec::new());
        // Dragging the top edge up by two lines shows two more lines (capped).
        let top = pos2(lines.center().x, lines.top() - 1.0);
        frame(&mut app, &ctx, vec![Event::PointerMoved(top)]);
        press(&mut app, &ctx, top, true);
        for dy in [8.0, 20.0, 2.0 * LINE_H] {
            frame(&mut app, &ctx, vec![Event::PointerMoved(top - vec2(0.0, dy))]);
        }
        press(&mut app, &ctx, top - vec2(0.0, 2.0 * LINE_H), false);
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(app.ui.history_lines, 5);
        assert!(!app.cmd.expanded, "a drag isn't a click");
        assert!(app.session.pending_window.is_none());
        assert_eq!(app.run("ui.cmdline.lines", serde_json::json!({"lines": 99})), Ok(serde_json::json!({"lines": MAX_LINES})));
    }
}

#[cfg(test)]
mod delete_key_tests {
    use super::*;
    use cadaddy_doc::Handle;

    const PRESS: KeyPress = KeyPress { repeat: false, plain: true };

    #[test]
    fn mac_delete_erases_a_fresh_selection() {
        assert!(backspace_erases(true, PRESS, "", false, true, false));
        // Not on Windows/Linux, where Backspace never deletes objects.
        assert!(!backspace_erases(false, PRESS, "", false, true, false));
        // Nothing selected, a command running, or text on the command line: just edit text.
        assert!(!backspace_erases(true, PRESS, "", false, false, false));
        assert!(!backspace_erases(true, PRESS, "", true, true, false));
        assert!(!backspace_erases(true, PRESS, "m", false, true, false));
        // Option/Cmd+delete are text-editing gestures on macOS.
        assert!(!backspace_erases(true, KeyPress { plain: false, ..PRESS }, "", false, true, false));
    }

    #[test]
    fn holding_delete_to_clear_typing_never_erases() {
        // Auto-repeat after the last character is gone.
        assert!(!backspace_erases(true, KeyPress { repeat: true, ..PRESS }, "", false, true, false));
        // Typed, then cleared with separate presses: still not erased.
        assert!(!backspace_erases(true, PRESS, "", false, true, true));
    }

    #[test]
    fn typing_is_forgotten_when_the_selection_changes() {
        let mut c = CmdLine::default();
        c.watch_selection(&[Handle(1)]);
        c.buffer.push('m');
        c.note_typing();
        c.buffer.clear();
        c.watch_selection(&[Handle(1)]);
        assert!(c.typed_since_selection);
        c.watch_selection(&[Handle(1), Handle(2)]);
        assert!(!c.typed_since_selection);
    }
}

#[cfg(test)]
mod autocomplete_tests {
    use super::*;

    #[test]
    fn exact_alias_is_first_suggestion_with_alias_display() {
        let rec = suggestions("rec");
        assert!(!rec.is_empty());
        assert_eq!(rec[0].id, "RECTANG");
        assert_eq!(rec[0].display, "REC (RECTANG)");
        assert_eq!(rec[0].label, "Rectangle");
        // RECOVER should follow RECTANG, not precede it
        assert!(rec.iter().any(|s| s.id == "RECOVER"));
        assert_eq!(rec[1].id, "RECOVER");
        assert_eq!(rec[1].display, "RECOVER");

        let l = suggestions("l");
        assert_eq!(l[0].id, "LINE");
        assert_eq!(l[0].display, "L (LINE)");

        let c = suggestions("c");
        assert_eq!(c[0].id, "CIRCLE");
        assert_eq!(c[0].display, "C (CIRCLE)");

        let co = suggestions("co");
        assert_eq!(co[0].id, "COPY");
        assert_eq!(co[0].display, "CO (COPY)");
    }

    #[test]
    fn prefix_without_alias_shows_command_id() {
        let rect = suggestions("rect");
        assert_eq!(rect[0].id, "RECTANG");
        assert_eq!(rect[0].display, "RECTANG");

        let line = suggestions("line");
        assert_eq!(line[0].id, "LINE");
        assert_eq!(line[0].display, "LINE");
    }

    fn app() -> CadApp {
        CadApp::new(cadaddy_engine::Session::new(), crate::Services::default())
    }

    #[test]
    fn a_layer_prompt_completes_layer_names() {
        let mut app = app();
        for n in ["Walls", "Walkway", "Fire walls", "Doors"] {
            app.run("layer.new", serde_json::json!({ "name": n })).unwrap();
        }
        app.cmdline("-layer");
        app.cmdline("S");
        assert!(app.session.prompt_text().contains("layer name"), "{}", app.session.prompt_text());
        app.cmd.buffer = "wal".into();
        let ids: Vec<String> = current_suggestions(&app).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["Walls", "Walkway", "Fire walls"]);
        tab(&mut app);
        assert_eq!(app.cmd.buffer, "Walls");
        submit(&mut app);
        app.cmdline("");
        assert_eq!(app.session.doc().unwrap().header.str("CLAYER", "0"), "Walls");
        // Not a naming prompt: nothing offered.
        app.cmdline("line");
        app.cmd.buffer = "wal".into();
        assert!(current_suggestions(&app).is_empty());
        app.session.cancel();
    }

    #[test]
    fn typed_commands_rank_autocomplete_and_settings_apply() {
        let mut app = app();
        app.cmd.buffer = "di".into();
        assert_eq!(current_suggestions(&app)[0].id, "DIST");
        for _ in 0..3 {
            app.cmd.buffer = "dimlinear".into();
            submit(&mut app);
            app.session.cancel();
        }
        assert_eq!(app.ui.command_usage.count("dimlinear"), 3);
        app.cmd.buffer = "di".into();
        let ids: Vec<String> = current_suggestions(&app).into_iter().map(|s| s.id).collect();
        assert_eq!(&ids[..2], ["DIST", "DIMLINEAR"], "{ids:?}");
        // System variables, then AutoComplete off.
        app.cmd.buffer = "orthom".into();
        assert!(current_suggestions(&app).iter().any(|s| s.id == "ORTHOMODE"));
        app.run("ui.autocomplete", serde_json::json!({ "toggle": "sysvars" })).unwrap();
        assert!(!current_suggestions(&app).iter().any(|s| s.id == "ORTHOMODE"));
        app.run("ui.autocomplete", serde_json::json!({ "enabled": false })).unwrap();
        app.cmd.buffer = "li".into();
        assert!(current_suggestions(&app).is_empty());
        assert!(app.run("ui.autocomplete", serde_json::json!({ "toggle": "nope" })).is_err());
        // Transparency, kept with the preferences.
        app.run("ui.cmdline.transparency", serde_json::json!({ "percent": 50 })).unwrap();
        assert!((opacity(&app) - 0.5).abs() < 1e-6);
        assert!(app.run("ui.cmdline.transparency", serde_json::json!({ "percent": 99 })).is_err());
        let saved = serde_json::to_value(&app.ui).unwrap();
        assert_eq!(
            (saved["cmdlineTransparency"].clone(), saved["autocomplete"]["enabled"].clone()),
            (serde_json::json!(50), serde_json::json!(false))
        );
    }

    #[test]
    fn recent_input_in_the_command_line_menu_and_bar_width() {
        let mut app = app();
        // Typed as at the command line, so the prompts' answers are remembered.
        for t in ["circle", "0,0", "5", "circle"] {
            app.cmd.buffer = t.into();
            submit(&mut app);
        }
        let groups = crate::shortcut_menus::command_line_entries(&app);
        let recent = groups
            .iter()
            .flatten()
            .any(|e| matches!(e, crate::menus::Entry::Sub { label, children } if label == "Recent Input" && !children.is_empty()));
        assert!(recent, "the inputs typed at prompts are offered");
        app.session.cancel();
        assert_eq!(bar_width(&app, 1000.0), 480.0);
        app.ui.cmdline_width = Some(0.8);
        assert_eq!(bar_width(&app, 1000.0), 800.0);
        app.ui.cmdline_width = Some(f32::NAN);
        assert_eq!(bar_width(&app, 1000.0), 480.0);
    }

    #[test]
    fn tab_autocompletes_alias_to_resolved_command() {
        let mut app = CadApp::new(cadaddy_engine::Session::empty(), crate::Services::default());
        app.cmd.buffer = "rec".to_string();
        tab(&mut app);
        assert_eq!(app.cmd.buffer, "RECTANG");
    }
}
