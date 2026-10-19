//! The alias and shortcut editor (ALIASEDIT typed alone, Tools ▸ Customize ▸ Edit Aliases and
//! Shortcuts): command aliases (built-in and the user's, through the ALIASEDIT command, saved to
//! the user's `.pgp` file and the profile) and keyboard shortcuts (the built-in ones as the user
//! changed them, kept with the interface state). Both lists can be searched; a user alias that
//! takes over a built-in one and a shortcut that replaces a built-in one are flagged; Reset to
//! Defaults goes back to the built-in ones.

use egui::{KeyboardShortcut, Modifiers, RichText};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CadApp;

/// Most shortcuts the user changes.
pub const MAX_BINDINGS: usize = 500;

/// A shortcut the user set: `key` ("Ctrl+Shift+K", "F6") runs `command`; an empty command takes a
/// built-in shortcut away.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub key: String,
    pub command: String,
}

/// A shortcut as text: "Ctrl+Shift+S" (Ctrl is Cmd on the Mac), "F5".
pub fn key_text(s: &KeyboardShortcut) -> String {
    let mut t = String::new();
    if s.modifiers.command || s.modifiers.ctrl || s.modifiers.mac_cmd {
        t.push_str("Ctrl+");
    }
    if s.modifiers.alt {
        t.push_str("Alt+");
    }
    if s.modifiers.shift {
        t.push_str("Shift+");
    }
    t.push_str(s.logical_key.name());
    t
}

/// A shortcut typed as text ("ctrl+shift+k", "Cmd+K", "F6"); `None` when it isn't one.
pub fn parse_key(text: &str) -> Option<KeyboardShortcut> {
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last()?;
    let mut m = Modifiers::NONE;
    for p in mods {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => m |= Modifiers::COMMAND,
            "alt" | "option" | "opt" => m |= Modifiers::ALT,
            "shift" => m |= Modifiers::SHIFT,
            _ => return None,
        }
    }
    let name = if key.chars().count() == 1 { key.to_ascii_uppercase() } else { (*key).to_string() };
    let name = match name.to_ascii_lowercase().as_str() {
        n if n.starts_with('f') && n[1..].chars().all(|c| c.is_ascii_digit()) && n.len() > 1 => name.to_ascii_uppercase(),
        _ => name,
    };
    let k = egui::Key::from_name(&name)?;
    Some(KeyboardShortcut::new(m, k))
}

/// Whether `id` is a command (the engine's or the interface's).
fn is_command(id: &str) -> bool {
    cadaddy_engine::cmd::find_command(id).is_some() || crate::menus::UI_COMMANDS.iter().any(|(c, ..)| *c == id)
}

/// The shortcuts in force: the built-in ones the user didn't change, then the user's.
pub fn effective_shortcuts(app: &CadApp) -> Vec<(KeyboardShortcut, String)> {
    let user: Vec<(KeyboardShortcut, &str)> = app.ui.shortcuts.iter().filter_map(|b| Some((parse_key(&b.key)?, b.command.as_str()))).collect();
    let mut out: Vec<(KeyboardShortcut, String)> =
        crate::menus::default_shortcuts().into_iter().filter(|(k, _)| !user.iter().any(|(u, _)| u == k)).map(|(k, c)| (k, c.to_string())).collect();
    out.extend(user.into_iter().filter(|(_, c)| !c.is_empty()).map(|(k, c)| (k, c.to_string())));
    out
}

/// Bind `key` to `command` (replacing what the key did).
pub fn set_shortcut(app: &mut CadApp, key: &str, command: &str) -> Result<String, String> {
    let k = parse_key(key).ok_or_else(|| format!("`{}` isn't a key (e.g. Ctrl+Shift+K or F6)", key.trim()))?;
    let command = command.trim().to_ascii_lowercase();
    if !is_command(&command) {
        return Err(format!("unknown command {command}"));
    }
    let text = key_text(&k);
    let full = app.ui.shortcuts.len() >= MAX_BINDINGS;
    match app.ui.shortcuts.iter_mut().find(|b| parse_key(&b.key) == Some(k)) {
        Some(b) => *b = Binding { key: text.clone(), command },
        None if full => return Err(format!("at most {MAX_BINDINGS} shortcuts")),
        None => app.ui.shortcuts.push(Binding { key: text.clone(), command }),
    }
    Ok(text)
}

/// Take `key` away: a built-in shortcut is turned off, the user's removed.
pub fn remove_shortcut(app: &mut CadApp, key: &str) -> Result<(), String> {
    let k = parse_key(key).ok_or_else(|| format!("`{}` isn't a key", key.trim()))?;
    app.ui.shortcuts.retain(|b| parse_key(&b.key) != Some(k));
    if crate::menus::default_shortcuts().iter().any(|(d, _)| *d == k) {
        app.ui.shortcuts.push(Binding { key: key_text(&k), command: String::new() });
    }
    Ok(())
}

/// The built-in command a user shortcut replaces, if any.
fn replaces(k: &KeyboardShortcut, command: &str) -> Option<&'static str> {
    crate::menus::default_shortcuts().into_iter().find(|(d, c)| d == k && *c != command).map(|(_, c)| c)
}

/// `ui.shortcuts` {action?: list (default) | set | remove | reset, key, command}.
pub fn run(app: &mut CadApp, params: &Value) -> Result<Value, String> {
    let s = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match s("action").to_ascii_lowercase().as_str() {
        "" | "list" => {}
        "set" => {
            set_shortcut(app, &s("key"), &s("command"))?;
        }
        "remove" => remove_shortcut(app, &s("key"))?,
        "reset" => app.ui.shortcuts.clear(),
        other => return Err(format!("unknown action \"{other}\" (list, set, remove or reset)")),
    }
    let list: Vec<Value> = effective_shortcuts(app)
        .iter()
        .map(|(k, c)| {
            let mut v = json!({ "key": key_text(k), "command": c });
            if let Some(r) = replaces(k, c) {
                v["replaces"] = json!(r);
            }
            v
        })
        .collect();
    Ok(json!({ "shortcuts": list, "changed": app.ui.shortcuts }))
}

#[derive(Clone, Default)]
struct State {
    tab: u8,
    search: String,
    alias: String,
    command: String,
    key: String,
    key_command: String,
    error: Option<String>,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-alias-editor")
}

/// The editor window.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let listing = app.session.execute("aliasedit", &json!({})).unwrap_or(Value::Null);
    let mut alias_call: Option<Value> = None;
    let mut key_call: Option<Value> = None;
    let mut load = false;
    egui::Window::new(crate::tl!("Command Aliases and Shortcuts"))
        .id(egui::Id::new("Command Aliases and Shortcuts"))
        .open(open)
        .resizable(true)
        .default_size([560.0, 480.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, l) in ["Command Aliases", "Keyboard Shortcuts"].iter().enumerate() {
                    if ui.selectable_label(st.tab == i as u8, crate::i18n::t(l)).clicked() {
                        st.tab = i as u8;
                        st.error = None;
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Search"));
                let r = ui.text_edit_singleline(&mut st.search);
                crate::icons::describe_area(&r, "Search");
            });
            let q = st.search.trim().to_ascii_lowercase();
            let hit = |a: &str, b: &str| q.is_empty() || a.to_ascii_lowercase().contains(&q) || b.to_ascii_lowercase().contains(&q);
            if st.tab == 0 {
                let user: Vec<(String, String)> = listing["aliases"]
                    .as_object()
                    .map(|m| m.iter().map(|(a, c)| (a.clone(), c.as_str().unwrap_or_default().to_string())).collect())
                    .unwrap_or_default();
                let conflicts = listing["conflicts"].as_array().cloned().unwrap_or_default();
                egui::ScrollArea::vertical().id_salt("alias-rows").max_height(260.0).show(ui, |ui| {
                    egui::Grid::new("alias-grid").striped(true).num_columns(4).show(ui, |ui| {
                        for (a, c) in user.iter().filter(|(a, c)| hit(a, c)) {
                            ui.label(RichText::new(a.to_ascii_uppercase()).strong());
                            ui.label(c.to_ascii_uppercase());
                            match conflicts.iter().find(|x| x["alias"] == a.as_str()) {
                                Some(x) => ui.label(
                                    RichText::new(crate::tf!(
                                        "replaces {command}",
                                        command = x["overrides"].as_str().unwrap_or_default().to_ascii_uppercase()
                                    ))
                                    .color(t.warn),
                                ),
                                None => ui.label(RichText::new(crate::tl!("User")).color(t.text_dim)),
                            };
                            ui.horizontal(|ui| {
                                if ui.small_button(crate::tl!("Edit")).clicked() {
                                    st.alias = a.to_ascii_uppercase();
                                    st.command = c.to_ascii_uppercase();
                                }
                                if ui.small_button("✖").on_hover_text(crate::tl!("Delete")).clicked() {
                                    alias_call = Some(json!({ "remove": [a] }));
                                }
                            });
                            ui.end_row();
                        }
                        for b in listing["builtIn"].as_array().into_iter().flatten() {
                            let (a, c) = (b["alias"].as_str().unwrap_or_default(), b["command"].as_str().unwrap_or_default());
                            if user.iter().any(|(u, _)| u == a) || !hit(a, c) {
                                continue;
                            }
                            ui.label(a.to_ascii_uppercase());
                            ui.label(c.to_ascii_uppercase());
                            ui.label(RichText::new(crate::tl!("Built-in")).color(t.text_dim));
                            if ui.small_button(crate::tl!("Edit")).clicked() {
                                st.alias = a.to_ascii_uppercase();
                                st.command = c.to_ascii_uppercase();
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Alias"));
                    let r = ui.add(egui::TextEdit::singleline(&mut st.alias).desired_width(80.0));
                    crate::icons::describe_area(&r, "Alias");
                    ui.label(crate::tl!("Command"));
                    let r = ui.add(egui::TextEdit::singleline(&mut st.command).desired_width(140.0));
                    crate::icons::describe_area(&r, "Command");
                    if ui
                        .add_enabled(!st.alias.trim().is_empty() && !st.command.trim().is_empty(), egui::Button::new(crate::tl!("Add or Change")))
                        .clicked()
                    {
                        let mut m = serde_json::Map::new();
                        m.insert(st.alias.trim().to_string(), json!(st.command.trim()));
                        alias_call = Some(json!({ "set": m }));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Save")).on_hover_text(listing["file"].as_str().unwrap_or("cadaddy.pgp")).clicked() {
                        alias_call = Some(json!({ "save": true }));
                    }
                    load = ui.button(crate::tl!("Load...")).clicked();
                    if ui.button(crate::tl!("Reset to Defaults")).clicked() {
                        alias_call = Some(json!({ "clear": true }));
                    }
                });
            } else {
                let rows = effective_shortcuts(app);
                egui::ScrollArea::vertical().id_salt("shortcut-rows").max_height(260.0).show(ui, |ui| {
                    egui::Grid::new("shortcut-grid").striped(true).num_columns(4).show(ui, |ui| {
                        for (k, c) in rows.iter().filter(|(k, c)| hit(&key_text(k), c)) {
                            let text = key_text(k);
                            ui.label(RichText::new(&text).strong());
                            ui.label(c.to_ascii_uppercase());
                            match replaces(k, c) {
                                Some(r) => ui.label(RichText::new(crate::tf!("replaces {command}", command = r.to_ascii_uppercase())).color(t.warn)),
                                None if app.ui.shortcuts.iter().any(|b| b.key == text) => {
                                    ui.label(RichText::new(crate::tl!("User")).color(t.text_dim))
                                }
                                None => ui.label(RichText::new(crate::tl!("Built-in")).color(t.text_dim)),
                            };
                            ui.horizontal(|ui| {
                                if ui.small_button(crate::tl!("Edit")).clicked() {
                                    st.key = text.clone();
                                    st.key_command = c.clone();
                                }
                                if ui.small_button("✖").on_hover_text(crate::tl!("Delete")).clicked() {
                                    key_call = Some(json!({ "action": "remove", "key": text }));
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Keys"));
                    let r = ui.add(egui::TextEdit::singleline(&mut st.key).desired_width(110.0).hint_text("Ctrl+Shift+K"));
                    crate::icons::describe_area(&r, "Keys");
                    ui.label(crate::tl!("Command"));
                    let r = ui.add(egui::TextEdit::singleline(&mut st.key_command).desired_width(140.0));
                    crate::icons::describe_area(&r, "Command");
                    if ui
                        .add_enabled(!st.key.trim().is_empty() && !st.key_command.trim().is_empty(), egui::Button::new(crate::tl!("Add or Change")))
                        .clicked()
                    {
                        key_call = Some(json!({ "action": "set", "key": st.key, "command": st.key_command }));
                    }
                });
                if ui.button(crate::tl!("Reset to Defaults")).clicked() {
                    key_call = Some(json!({ "action": "reset" }));
                }
            }
            if let Some(e) = &st.error {
                ui.label(RichText::new(e).color(t.warn));
            }
        });
    if load {
        match app.services.pick_open.as_ref().map(|p| p()) {
            Some(Some(path)) => alias_call = Some(json!({ "file": path })),
            Some(None) => {}
            None => st.error = Some("There is no file dialog here: ALIASEDIT {\"file\": \"path.pgp\"} loads one.".into()),
        }
    }
    if let Some(p) = alias_call {
        st.error = app.session.execute("aliasedit", &p).err().map(|e| e.user_message());
        if st.error.is_none() && p.get("set").is_some() {
            st.alias.clear();
            st.command.clear();
        }
    }
    if let Some(p) = key_call {
        st.error = run(app, &p).err();
        if st.error.is_none() && p["action"] == "set" {
            st.key.clear();
            st.key_command.clear();
        }
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

    fn app() -> CadApp {
        CadApp::new(cadaddy_engine::Session::new(), crate::Services::default())
    }

    #[test]
    fn keys_read_and_write_as_text() {
        let k = parse_key("ctrl+shift+k").unwrap();
        assert_eq!(k, KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::K));
        assert_eq!(key_text(&k), "Ctrl+Shift+K");
        assert_eq!(key_text(&parse_key("f6").unwrap()), "F6");
        assert_eq!(key_text(&parse_key("Cmd+1").unwrap()), "Ctrl+1");
        assert_eq!(key_text(&parse_key("Alt+Delete").unwrap()), "Alt+Delete");
        for bad in ["", "ctrl+", "hyper+k", "ctrl+nokey", "+"] {
            assert!(parse_key(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn shortcuts_are_set_flagged_removed_and_reset() {
        let mut app = app();
        let n = effective_shortcuts(&app).len();
        // A new key, and one that replaces a built-in shortcut (Ctrl+P was PLOT).
        set_shortcut(&mut app, "ctrl+shift+k", "line").unwrap();
        set_shortcut(&mut app, "Ctrl+P", "pan").unwrap();
        let eff = effective_shortcuts(&app);
        assert_eq!(eff.len(), n + 1);
        let p = parse_key("Ctrl+P").unwrap();
        assert!(eff.iter().any(|(k, c)| *k == p && c == "pan") && !eff.iter().any(|(k, c)| *k == p && c == "plot"));
        let r = run(&mut app, &json!({})).unwrap();
        assert!(r["shortcuts"].as_array().unwrap().iter().any(|s| s["key"] == "Ctrl+P" && s["replaces"] == "plot"));
        assert!(set_shortcut(&mut app, "ctrl+q", "nosuchcommand").is_err());
        assert!(set_shortcut(&mut app, "hyper+q", "line").is_err());

        // Removing a built-in one turns it off; removing the user's restores the built-in.
        remove_shortcut(&mut app, "F7").unwrap();
        assert!(!effective_shortcuts(&app).iter().any(|(k, _)| key_text(k) == "F7"));
        remove_shortcut(&mut app, "Ctrl+P").unwrap();
        assert!(!effective_shortcuts(&app).iter().any(|(k, _)| key_text(k) == "Ctrl+P"), "Ctrl+P was built in: off");
        run(&mut app, &json!({ "action": "reset" })).unwrap();
        assert_eq!(effective_shortcuts(&app).len(), n);
        assert!(app.ui.shortcuts.is_empty());

        // Kept with the interface state.
        set_shortcut(&mut app, "F6", "circle").unwrap();
        let ui: crate::UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
        assert_eq!(ui.shortcuts, [Binding { key: "F6".into(), command: "circle".into() }]);
    }

    #[test]
    fn typed_aliasedit_opens_the_editor() {
        let mut app = app();
        app.cmdline("aliasedit");
        assert_eq!(app.ui.dialog.as_deref(), Some("aliasedit"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert_eq!(app.ui.dialog.as_deref(), Some("aliasedit"));
        // With parameters it is the command.
        assert!(app.run("aliasedit", json!({})).unwrap()["builtIn"].is_array());
    }

    #[test]
    fn the_profile_keeps_the_user_aliases() {
        let mut app = app();
        app.run("aliasedit", json!({ "set": { "qz": "circle" } })).unwrap();
        let profile = app.profile_json();
        app.run("aliasedit", json!({ "clear": true })).unwrap();
        let mut other = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        other.load_profile(&profile);
        assert_eq!(cadaddy_engine::cmd::file::user_aliases().get("qz").map(String::as_str), Some("circle"));
        other.run("aliasedit", json!({ "clear": true })).unwrap();
    }
}
