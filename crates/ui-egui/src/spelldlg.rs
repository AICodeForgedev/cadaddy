//! The Check Spelling dialog (SPELL typed or from a menu; -SPELL keeps the command line): where
//! to check, each word not in the dictionary with the text around it and suggestions, and
//! Ignore / Ignore All / Change / Change All / Add to Dictionary. Every action is a `spell` call.

use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// Check the selected objects (else the whole current space).
    selected: Vec<String>,
    use_selection: bool,
    /// What the last check found (`spell` results), and the one shown.
    found: Vec<Value>,
    at: usize,
    /// Words passed over with Ignore: (object, word).
    skipped: Vec<(String, String)>,
    /// The replacement being edited.
    to: String,
    started: bool,
    message: Option<String>,
}

pub fn open(app: &mut CadApp) {
    let selected: Vec<String> = app.session.selection().iter().map(|h| h.hex()).collect();
    app.spell_dlg = Some(State {
        use_selection: !selected.is_empty(),
        selected,
        found: Vec::new(),
        at: 0,
        skipped: Vec::new(),
        to: String::new(),
        started: false,
        message: None,
    });
    app.ui.dialog = Some("spell".into());
}

impl State {
    fn scope(&self) -> Value {
        if self.use_selection && !self.selected.is_empty() { json!({ "handles": self.selected }) } else { json!({}) }
    }
    fn current(&self) -> Option<&Value> {
        self.found.get(self.at)
    }
    fn word(&self) -> String {
        self.current().and_then(|m| m["word"].as_str()).unwrap_or_default().to_string()
    }
    /// Show the found word at `self.at`, with its first suggestion ready.
    fn show(&mut self) {
        self.to = self.current().and_then(|m| m["suggestions"][0].as_str().map(str::to_string)).unwrap_or_else(|| self.word());
        if self.at >= self.found.len() {
            self.message = Some(crate::tl!("Spelling check complete.").to_string());
        }
    }
}

/// Check again, leaving out what was ignored.
fn recheck(app: &mut CadApp, st: &mut State) {
    let mut p = st.scope();
    p["action"] = json!("check");
    match app.run("spell", p) {
        Ok(r) => {
            st.found = r["misspelled"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|m| !st.skipped.iter().any(|(h, w)| m["handle"].as_str() == Some(h) && m["word"].as_str() == Some(w)))
                .collect();
            st.at = 0;
            st.started = true;
            st.message = None;
            st.show();
        }
        Err(e) => st.message = Some(e),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.spell_dlg.clone() else {
        *open = false;
        return;
    };
    #[derive(PartialEq)]
    enum Act {
        None,
        Start,
        Ignore,
        IgnoreAll,
        Change,
        ChangeAll,
        Add,
        Close,
    }
    let mut act = Act::None;
    egui::Window::new(crate::tl!("Check Spelling"))
        .id(egui::Id::new("Check Spelling"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Where to check"));
                egui::ComboBox::from_id_salt("spell-where")
                    .selected_text(if st.use_selection { crate::tl!("Selected objects") } else { crate::tl!("Entire drawing") })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.use_selection, false, crate::tl!("Entire drawing"));
                        ui.add_enabled_ui(!st.selected.is_empty(), |ui| {
                            ui.selectable_value(&mut st.use_selection, true, crate::tl!("Selected objects"));
                        });
                    });
                if ui.button(crate::tl!("Start")).clicked() {
                    act = Act::Start;
                }
            });
            ui.separator();
            let active = st.started && st.current().is_some();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(260.0);
                    ui.label(crate::tl!("Not in dictionary:"));
                    ui.label(egui::RichText::new(st.word()).strong());
                    if let Some(c) = st.current().and_then(|m| m["context"].as_str()) {
                        ui.weak(c);
                    }
                    ui.add_space(4.0);
                    ui.label(crate::tl!("Suggestions:"));
                    ui.add_enabled(active, egui::TextEdit::singleline(&mut st.to));
                    let sugg: Vec<String> = st
                        .current()
                        .and_then(|m| m["suggestions"].as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    egui::ScrollArea::vertical().max_height(120.0).id_salt("spell-sugg").show(ui, |ui| {
                        for sg in sugg {
                            if ui.selectable_label(st.to == sg, &sg).clicked() {
                                st.to = sg;
                            }
                        }
                    });
                });
                ui.vertical(|ui| {
                    ui.add_enabled_ui(active, |ui| {
                        for (label, a) in [
                            ("Ignore", Act::Ignore),
                            ("Ignore All", Act::IgnoreAll),
                            ("Change", Act::Change),
                            ("Change All", Act::ChangeAll),
                            ("Add to Dictionary", Act::Add),
                        ] {
                            if ui.add_sized([150.0, 22.0], egui::Button::new(crate::i18n::t(label))).clicked() {
                                act = a;
                            }
                        }
                    });
                });
            });
            ui.separator();
            ui.weak(format!("{} {}", crate::tl!("Main dictionary:"), crate::tl!("American English")));
            if let Ok(v) = app.session.execute("getvar", &json!({"name": "DCTCUST"})) {
                ui.weak(format!("{} {}", crate::tl!("Custom dictionary:"), v["value"].as_str().unwrap_or_default()));
            }
            if let Some(m) = &st.message {
                ui.label(m);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(crate::tl!("Close")).clicked() {
                    act = Act::Close;
                }
            });
        });
    let word = st.word();
    let current = st.current().cloned();
    match act {
        Act::None => {}
        Act::Start => recheck(app, &mut st),
        Act::Ignore => {
            if let Some(m) = &current {
                st.skipped.push((m["handle"].as_str().unwrap_or_default().to_string(), word));
            }
            st.at += 1;
            st.show();
        }
        Act::IgnoreAll | Act::Add => {
            let action = if act == Act::Add { "add" } else { "ignoreAll" };
            match app.run("spell", json!({ "action": action, "word": word })) {
                Ok(_) => recheck(app, &mut st),
                Err(e) => st.message = Some(e),
            }
        }
        Act::Change | Act::ChangeAll => {
            let p = match (&current, act == Act::ChangeAll) {
                (_, true) => json!({ "action": "changeAll", "word": word, "to": st.to }),
                (Some(m), false) => json!({ "action": "change", "word": word, "to": st.to, "handle": m["handle"], "where": m["where"] }),
                (None, false) => Value::Null,
            };
            if !p.is_null() {
                match app.run("spell", p) {
                    Ok(_) => recheck(app, &mut st),
                    Err(e) => st.message = Some(e),
                }
            }
        }
        Act::Close => *open = false,
    }
    if *open {
        app.spell_dlg = Some(st);
    } else {
        app.spell_dlg = None;
    }
}
