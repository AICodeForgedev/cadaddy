//! Interface customisation without CUI files (CUI, Tools ▸ Customize ▸ Interface…): the Quick
//! Access Toolbar's buttons (add, remove, reorder), ribbon panels of the user's own on any tab or on
//! tabs of their own (buttons for any command or LISP macro), built-in panels hidden, and the
//! keyboard shortcuts (the shortcut editor, [`crate::aliasdlg`]). Kept in the profile as JSON
//! (`customUi`), exported and imported as a `.cdyui` file. Contextual ribbon tabs (Text Editor,
//! Hatch Editor, Table Cell, Array, Block Editor) appear while that editor or object is current.
//!
//! The ribbon's tabs and panels are static data; a customisation is laid out as such once per
//! change ([`effective`]) and kept, so drawing stays as fast as before.

use std::cell::RefCell;

use egui::{RichText, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CadApp;
use crate::icons::Icon;
use crate::ribbon::{Btn, Item, Panel, Tab};
use crate::theme::Tokens;

pub const DIALOG: &str = "cui";
/// The extension of exported customisations.
pub const EXTENSION: &str = "cdyui";
/// Most buttons, panels and tabs one customisation holds (a profile is untrusted input).
const MAX_ITEMS: usize = 500;

/// A button: a command id, or a LISP expression or macro (`(command "line")`, `^C^CLINE`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomButton {
    pub cmd: String,
    /// Shown under the button (default: the command's label).
    pub label: String,
}

/// A panel of the user's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomPanel {
    /// The built-in tab it goes on (empty on a custom tab).
    pub tab: String,
    pub name: String,
    pub buttons: Vec<CustomButton>,
}

/// A tab of the user's own, after the built-in ones.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomTab {
    pub name: String,
    pub panels: Vec<CustomPanel>,
}

/// The user's customisation (the default changes nothing).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Custom {
    /// The Quick Access Toolbar's commands in order (`None`: the default buttons).
    pub qat: Option<Vec<String>>,
    /// Panels added to built-in tabs.
    pub panels: Vec<CustomPanel>,
    /// Tabs of the user's own.
    pub tabs: Vec<CustomTab>,
    /// Built-in panels hidden, as `Tab/Panel`.
    pub hidden: Vec<String>,
}

impl Custom {
    pub fn is_default(&self) -> bool {
        *self == Custom::default()
    }

    /// Read from JSON, keeping what fits the limits (a damaged profile changes less, never fails).
    pub fn from_json(v: &Value) -> Result<Custom, String> {
        let mut c: Custom = serde_json::from_value(v.clone()).map_err(|e| format!("not an interface customisation ({e})"))?;
        let trim = |s: &mut String| *s = s.trim().chars().take(200).collect();
        if let Some(q) = &mut c.qat {
            q.truncate(MAX_ITEMS);
            q.iter_mut().for_each(trim);
            q.retain(|x| !x.is_empty());
        }
        c.panels.truncate(MAX_ITEMS);
        c.tabs.truncate(MAX_ITEMS);
        c.hidden.truncate(MAX_ITEMS);
        for p in c.panels.iter_mut().chain(c.tabs.iter_mut().flat_map(|t| t.panels.iter_mut())) {
            trim(&mut p.name);
            trim(&mut p.tab);
            p.buttons.truncate(MAX_ITEMS);
            for b in &mut p.buttons {
                trim(&mut b.cmd);
                trim(&mut b.label);
            }
            p.buttons.retain(|b| !b.cmd.is_empty());
        }
        for t in &mut c.tabs {
            trim(&mut t.name);
            t.panels.truncate(MAX_ITEMS);
        }
        c.tabs.retain(|t| !t.name.is_empty());
        Ok(c)
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Every built-in button's icon by command (ribbon, flyouts, Quick Access, application menu).
fn icon_for(cmd: &str) -> Icon {
    for tab in crate::ribbon::TABS {
        for panel in tab.panels {
            for item in panel.items {
                let btns: &[Btn] = match item {
                    Item::Large(b) => std::slice::from_ref(b),
                    Item::Smalls(bs) => bs,
                    Item::Layers => &[],
                };
                for b in btns {
                    if b.cmd == cmd {
                        return b.icon;
                    }
                    if let Some((i, _, _)) = b.flyout.iter().find(|(_, c, _)| *c == cmd) {
                        return *i;
                    }
                }
            }
            if let Some((i, _, _)) = panel.more.iter().find(|(_, c, _)| *c == cmd) {
                return *i;
            }
        }
    }
    crate::ribbon::qat_default().iter().find(|(_, c, _)| *c == cmd).map_or(Icon::Gear, |(i, _, _)| *i)
}

/// A row in the dialog: "Label (command)", or the macro alone when it is its own label.
fn row_text(b: &CustomButton) -> String {
    let l = label_for(b);
    if l == b.cmd || b.cmd.starts_with(l.as_str()) { b.cmd.clone() } else { format!("{l} ({})", b.cmd) }
}

/// What a button shows: its own label, the command's, or the macro's text.
fn label_for(b: &CustomButton) -> String {
    if !b.label.is_empty() {
        return b.label.clone();
    }
    if let Some(spec) = cadaddy_engine::find_command(&b.cmd) {
        return spec.label.trim_end_matches("...").to_string();
    }
    if let Some((_, l, _, _)) = crate::menus::UI_COMMANDS.iter().find(|(id, ..)| *id == b.cmd) {
        return l.trim_end_matches("...").to_string();
    }
    b.cmd.chars().take(24).collect()
}

fn panel_of(p: &CustomPanel) -> Panel {
    let btn = |b: &CustomButton| Btn { icon: icon_for(&b.cmd), cmd: leak(&b.cmd), label: leak(&label_for(b)), flyout: &[] };
    let items: Vec<Item> = if p.buttons.len() <= 3 {
        p.buttons.iter().map(|b| Item::Large(btn(b))).collect()
    } else {
        p.buttons.chunks(3).map(|c| Item::Smalls(Box::leak(c.iter().map(btn).collect::<Vec<_>>().into_boxed_slice()))).collect()
    };
    Panel { name: leak(&p.name), items: Box::leak(items.into_boxed_slice()), more: &[] }
}

/// The ribbon's tabs with the customisation applied: built-in tabs (hidden panels left out, the
/// user's panels added), then the user's tabs. Laid out once per change.
pub fn effective(custom: &Custom) -> &'static [Tab] {
    thread_local! {
        static CACHE: RefCell<Option<(Custom, &'static [Tab])>> = const { RefCell::new(None) };
    }
    if custom.is_default() {
        return crate::ribbon::TABS;
    }
    CACHE.with(|c| {
        if let Some((k, tabs)) = &*c.borrow()
            && k == custom
        {
            return *tabs;
        }
        let mut tabs: Vec<Tab> = Vec::new();
        for tab in crate::ribbon::TABS {
            let mut panels: Vec<Panel> = tab
                .panels
                .iter()
                .filter(|p| !custom.hidden.iter().any(|h| *h == format!("{}/{}", tab.name, p.name)))
                .map(|p| Panel { name: p.name, items: p.items, more: p.more })
                .collect();
            panels.extend(custom.panels.iter().filter(|p| p.tab == tab.name && !p.buttons.is_empty()).map(panel_of));
            tabs.push(Tab { name: tab.name, panels: Box::leak(panels.into_boxed_slice()) });
        }
        for t in &custom.tabs {
            let panels: Vec<Panel> = t.panels.iter().filter(|p| !p.buttons.is_empty()).map(panel_of).collect();
            tabs.push(Tab { name: leak(&t.name), panels: Box::leak(panels.into_boxed_slice()) });
        }
        let tabs: &'static [Tab] = Box::leak(tabs.into_boxed_slice());
        *c.borrow_mut() = Some((custom.clone(), tabs));
        tabs
    })
}

/// The Quick Access Toolbar's buttons: (icon, command, tooltip).
pub fn qat(custom: &Custom) -> Vec<(Icon, &'static str, &'static str)> {
    thread_local! {
        static CACHE: RefCell<Option<(Vec<String>, Vec<(Icon, &'static str, &'static str)>)>> = const { RefCell::new(None) };
    }
    let Some(list) = &custom.qat else { return crate::ribbon::qat_default().to_vec() };
    CACHE.with(|c| {
        if let Some((k, v)) = &*c.borrow()
            && k == list
        {
            return v.clone();
        }
        let v: Vec<(Icon, &'static str, &'static str)> =
            list.iter().map(|cmd| (icon_for(cmd), leak(cmd), leak(&label_for(&CustomButton { cmd: cmd.clone(), label: String::new() })))).collect();
        *c.borrow_mut() = Some((list.clone(), v.clone()));
        v
    })
}

/// Run a button: a command by id, a LISP expression or a macro typed at the command line.
pub fn run_button(app: &mut CadApp, cmd: &str) {
    let macro_text = cmd.trim().trim_start_matches("^C").trim_start_matches("^C");
    if macro_text.starts_with('(') || macro_text.contains(' ') || macro_text.contains(';') || macro_text != cmd {
        // `;` is Enter in a menu macro, as AutoCAD writes them.
        for line in macro_text.split(';') {
            app.cmdline(line.trim());
        }
    } else {
        app.start(cmd);
    }
}

// ---------------- contextual tabs ----------------

use crate::ribbon::{b_btn as b, ctx_tab};

pub const TEXT_EDITOR: Tab = ctx_tab(
    "Text Editor",
    &[Panel {
        name: "Tools",
        items: &[Item::Large(b(Icon::Search, "find", "Find and Replace")), Item::Large(b(Icon::Text, "spell", "Spell Check"))],
        more: &[],
    }],
);
pub const HATCH_EDITOR: Tab = ctx_tab(
    "Hatch Editor",
    &[Panel {
        name: "Options",
        items: &[
            Item::Large(b(Icon::Hatch, "hatchedit", "Hatch Edit")),
            Item::Large(b(Icon::MatchProp, "matchprop", "Match Properties")),
            Item::Large(b(Icon::Boundary, "hatchtoback", "Send Hatches to Back")),
        ],
        more: &[],
    }],
);
pub const TABLE_CELL: Tab = ctx_tab(
    "Table Cell",
    &[
        Panel {
            name: "Rows & Columns",
            items: &[
                Item::Smalls(&[b(Icon::Plus, "table.insertrow", "Insert Row"), b(Icon::Erase, "table.deleterow", "Delete Row")]),
                Item::Smalls(&[b(Icon::Plus, "table.insertcol", "Insert Column"), b(Icon::Erase, "table.deletecol", "Delete Column")]),
            ],
            more: &[],
        },
        Panel {
            name: "Merge",
            items: &[Item::Smalls(&[b(Icon::Join, "table.merge", "Merge Cells"), b(Icon::Explode, "table.unmerge", "Unmerge Cells")])],
            more: &[],
        },
        Panel {
            name: "Insert",
            items: &[Item::Large(b(Icon::Table, "table.formula", "Formula")), Item::Large(b(Icon::Export, "tableexport", "Export"))],
            more: &[],
        },
    ],
);
pub const ARRAY: Tab = ctx_tab(
    "Array",
    &[Panel {
        name: "Options",
        items: &[Item::Large(b(Icon::ArrayRect, "arrayedit", "Edit Source")), Item::Large(b(Icon::Close, "arrayclose", "Close Array"))],
        more: &[],
    }],
);
pub const BLOCK_EDITOR: Tab = ctx_tab(
    "Block Editor",
    &[
        Panel {
            name: "Open/Save",
            items: &[Item::Large(b(Icon::Save, "bsave", "Save Block")), Item::Smalls(&[b(Icon::SaveAs, "bsaveas", "Save Block As")])],
            more: &[],
        },
        Panel {
            name: "Dynamic",
            items: &[Item::Smalls(&[
                b(Icon::Constraint, "bparameter", "Parameter"),
                b(Icon::Gear, "bactiontool", "Action"),
                b(Icon::Layers, "bvstate", "Visibility States"),
            ])],
            more: &[],
        },
        Panel {
            name: "Attributes",
            items: &[Item::Large(b(Icon::AttDef, "attdef", "Define Attributes")), Item::Smalls(&[b(Icon::List, "battman", "Manage Attributes")])],
            more: &[],
        },
        Panel { name: "Close", items: &[Item::Large(b(Icon::Close, "bclose", "Close Block Editor"))], more: &[] },
    ],
);

/// The contextual tab for what is being edited or selected, if any.
pub fn contextual(app: &CadApp, ctx: &egui::Context) -> Option<&'static Tab> {
    if crate::mtext_editor::is_open(ctx) {
        return Some(&TEXT_EDITOR);
    }
    let st = app.session.state().ok()?;
    if st.block_edit.is_some() {
        return Some(&BLOCK_EDITOR);
    }
    let sel = app.session.selection();
    if sel.is_empty() || sel.len() > 10_000 {
        return None;
    }
    let d = app.session.doc().ok()?;
    let kinds: Vec<&cadaddy_engine::doc::Entity> = sel.iter().filter_map(|h| d.entity(*h)).map(|e| e.as_ref()).collect();
    if kinds.is_empty() {
        return None;
    }
    use cadaddy_engine::doc::EntityKind;
    if kinds.iter().all(|e| matches!(e.kind, EntityKind::Hatch(_))) {
        return Some(&HATCH_EDITOR);
    }
    if kinds.len() == 1 && matches!(kinds[0].kind, EntityKind::Table(_)) {
        return Some(&TABLE_CELL);
    }
    if kinds.len() == 1 && d.array_params(kinds[0]).is_some() {
        return Some(&ARRAY);
    }
    None
}

// ---------------- the command and the dialog ----------------

/// `ui.customize {action: get|set|reset|export|import, custom?, path?}`.
pub fn command(app: &mut CadApp, p: &Value) -> Result<Value, String> {
    match p.get("action").and_then(Value::as_str).unwrap_or("get") {
        "get" => Ok(app.custom_ui.to_json()),
        "set" => {
            app.custom_ui = Custom::from_json(p.get("custom").ok_or("`custom` is required")?)?;
            Ok(app.custom_ui.to_json())
        }
        "reset" => {
            app.custom_ui = Custom::default();
            Ok(app.custom_ui.to_json())
        }
        "export" => {
            let path = p.get("path").and_then(Value::as_str).ok_or("`path` is required")?;
            let text = serde_json::to_string_pretty(&json!({ "format": "cadaddy-interface", "version": 1, "custom": app.custom_ui.to_json() }))
                .unwrap_or_default();
            cadaddy_engine::writefile::write(path, text).map_err(|e| format!("{path}: {e}"))?;
            Ok(json!({ "path": path }))
        }
        "import" => {
            let path = p.get("path").and_then(Value::as_str).ok_or("`path` is required")?;
            let bytes = cadaddy_engine::readfile::read(path).map_err(|e| format!("{path}: {e}"))?;
            let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{path}: not a customisation file ({e})"))?;
            if v.get("format").and_then(Value::as_str) != Some("cadaddy-interface") {
                return Err(format!("{path}: not a CADaddy interface customisation"));
            }
            app.custom_ui = Custom::from_json(v.get("custom").unwrap_or(&Value::Null))?;
            Ok(app.custom_ui.to_json())
        }
        other => Err(format!("unknown action \"{other}\" (get, set, reset, export or import)")),
    }
}

/// The dialog's entry fields between frames.
#[derive(Clone, Default)]
struct State {
    add_qat: String,
    new_panel: String,
    panel_tab: String,
    new_tab: String,
    add_btn: Vec<String>,
}

/// Commands matching `filter`, for the "add" lists.
fn matches(filter: &str) -> Vec<(String, String)> {
    let f = filter.trim().to_ascii_lowercase();
    if f.is_empty() {
        return Vec::new();
    }
    cadaddy_engine::cmd::command_specs()
        .iter()
        .filter(|c| c.id.contains(&f) || c.label.to_ascii_lowercase().contains(&f))
        .take(8)
        .map(|c| (c.id.to_string(), c.label.to_string()))
        .collect()
}

/// A text field with up to eight matching commands under it; returns the command chosen.
fn command_picker(ui: &mut egui::Ui, text: &mut String, hint: &str) -> Option<String> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(260.0));
        if (ui.button(crate::tl!("Add")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))) && !text.trim().is_empty() {
            chosen = Some(text.trim().to_string());
        }
    });
    for (id, label) in matches(text) {
        if ui.small_button(format!("{label} ({id})")).clicked() {
            chosen = Some(id);
        }
    }
    if chosen.is_some() {
        text.clear();
    }
    chosen
}

/// Up, down and remove buttons for item `i` of `n`; what was asked.
fn row_buttons(ui: &mut egui::Ui, i: usize, n: usize) -> Option<i32> {
    let mut r = None;
    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text(crate::tl!("Move Up")).clicked() {
        r = Some(-1);
    }
    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).on_hover_text(crate::tl!("Move Down")).clicked() {
        r = Some(1);
    }
    if ui.small_button(crate::tl!("Remove")).clicked() {
        r = Some(0);
    }
    r
}

fn apply_move<T>(v: &mut Vec<T>, i: usize, how: i32) {
    match how {
        0 if i < v.len() => {
            v.remove(i);
        }
        -1 if i > 0 => v.swap(i, i - 1),
        1 if i + 1 < v.len() => v.swap(i, i + 1),
        _ => {}
    }
}

/// A custom panel's editor (its buttons and an add field).
fn panel_editor(ui: &mut egui::Ui, p: &mut CustomPanel, add: &mut String) -> bool {
    let mut remove_panel = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(&p.name).strong());
        if ui.small_button(crate::tl!("Remove Panel")).clicked() {
            remove_panel = true;
        }
    });
    ui.indent(("cui panel", &p.tab, &p.name), |ui| {
        let n = p.buttons.len();
        let mut mv = None;
        for (i, b) in p.buttons.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(row_text(b));
                if let Some(m) = row_buttons(ui, i, n) {
                    mv = Some((i, m));
                }
            });
        }
        if let Some((i, m)) = mv {
            apply_move(&mut p.buttons, i, m);
        }
        if let Some(cmd) = command_picker(ui, add, crate::i18n::t("Command or LISP macro, e.g. (command \"line\")")) {
            p.buttons.push(CustomButton { cmd, label: String::new() });
        }
    });
    remove_panel
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let id = egui::Id::new("cui state");
    let mut st: State = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let mut c = app.custom_ui.clone();
    let mut io: Option<&'static str> = None;
    let mut shortcuts = false;
    egui::Window::new(crate::tl!("Customize User Interface"))
        .id(egui::Id::new("cui window"))
        .open(open)
        .collapsible(false)
        .default_size(vec2(620.0, 560.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                ui.heading(crate::tl!("Quick Access Toolbar"));
                let mut list: Vec<String> =
                    c.qat.clone().unwrap_or_else(|| crate::ribbon::qat_default().iter().map(|(_, cmd, _)| cmd.to_string()).collect());
                let n = list.len();
                let mut mv = None;
                for (i, cmd) in list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        crate::icons::paint(ui.painter(), egui::Rect::from_min_size(ui.cursor().min, vec2(18.0, 18.0)), icon_for(cmd), false);
                        ui.add_space(22.0);
                        ui.label(row_text(&CustomButton { cmd: cmd.clone(), label: String::new() }));
                        if let Some(m) = row_buttons(ui, i, n) {
                            mv = Some((i, m));
                        }
                    });
                }
                let mut changed = false;
                if let Some((i, m)) = mv {
                    apply_move(&mut list, i, m);
                    changed = true;
                }
                if let Some(cmd) = command_picker(ui, &mut st.add_qat, crate::i18n::t("Add a command")) {
                    list.push(cmd);
                    changed = true;
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(c.qat.is_some(), egui::Button::new(crate::tl!("Reset Quick Access Toolbar"))).clicked() {
                        c.qat = None;
                    }
                });
                if changed {
                    c.qat = Some(list);
                }
                ui.separator();
                ui.heading(crate::tl!("Ribbon"));
                ui.label(RichText::new(crate::tl!("Built-in panels shown (untick to hide):")).color(t.text_dim));
                for tab in crate::ribbon::TABS {
                    egui::CollapsingHeader::new(crate::i18n::t(tab.name)).id_salt(("cui tab", tab.name)).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for p in tab.panels {
                                let key = format!("{}/{}", tab.name, p.name);
                                let mut shown = !c.hidden.contains(&key);
                                if ui.checkbox(&mut shown, crate::i18n::t(p.name)).changed() {
                                    if shown {
                                        c.hidden.retain(|h| *h != key);
                                    } else {
                                        c.hidden.push(key);
                                    }
                                }
                            }
                        });
                    });
                }
                ui.add_space(4.0);
                ui.label(RichText::new(crate::tl!("Your panels on built-in tabs:")).strong());
                let mut remove = None;
                st.add_btn.resize(c.panels.len() + c.tabs.iter().map(|t| t.panels.len()).sum::<usize>() + 1, String::new());
                for (i, p) in c.panels.iter_mut().enumerate() {
                    ui.label(RichText::new(crate::i18n::t(&p.tab)).color(t.text_dim));
                    if panel_editor(ui, p, &mut st.add_btn[i]) {
                        remove = Some(i);
                    }
                }
                if let Some(i) = remove {
                    c.panels.remove(i);
                }
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("cui panel tab")
                        .selected_text(if st.panel_tab.is_empty() {
                            crate::i18n::t("Tab").to_string()
                        } else {
                            crate::i18n::t(&st.panel_tab).to_string()
                        })
                        .show_ui(ui, |ui| {
                            for tab in crate::ribbon::TABS {
                                ui.selectable_value(&mut st.panel_tab, tab.name.to_string(), crate::i18n::t(tab.name));
                            }
                        });
                    ui.add(egui::TextEdit::singleline(&mut st.new_panel).hint_text(crate::tl!("New panel name")).desired_width(180.0));
                    if ui
                        .add_enabled(!st.panel_tab.is_empty() && !st.new_panel.trim().is_empty(), egui::Button::new(crate::tl!("Add Panel")))
                        .clicked()
                    {
                        c.panels.push(CustomPanel { tab: st.panel_tab.clone(), name: st.new_panel.trim().to_string(), buttons: Vec::new() });
                        st.new_panel.clear();
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new(crate::tl!("Your tabs:")).strong());
                let mut remove_tab = None;
                let mut k = c.panels.len();
                for (ti, tab) in c.tabs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&tab.name).strong().color(t.accent));
                        if ui.small_button(crate::tl!("Remove Tab")).clicked() {
                            remove_tab = Some(ti);
                        }
                        if ui.small_button(crate::tl!("Add Panel")).clicked() {
                            tab.panels.push(CustomPanel { tab: String::new(), name: format!("Panel {}", tab.panels.len() + 1), buttons: Vec::new() });
                        }
                    });
                    let mut remove_p = None;
                    for (pi, p) in tab.panels.iter_mut().enumerate() {
                        if let Some(add) = st.add_btn.get_mut(k)
                            && panel_editor(ui, p, add)
                        {
                            remove_p = Some(pi);
                        }
                        k += 1;
                    }
                    if let Some(pi) = remove_p {
                        tab.panels.remove(pi);
                    }
                }
                if let Some(ti) = remove_tab {
                    c.tabs.remove(ti);
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut st.new_tab).hint_text(crate::tl!("New tab name")).desired_width(180.0));
                    if ui.add_enabled(!st.new_tab.trim().is_empty(), egui::Button::new(crate::tl!("Add Tab"))).clicked() {
                        c.tabs.push(CustomTab {
                            name: st.new_tab.trim().to_string(),
                            panels: vec![CustomPanel { tab: String::new(), name: "Panel 1".into(), buttons: Vec::new() }],
                        });
                        st.new_tab.clear();
                    }
                });
                ui.separator();
                ui.heading(crate::tl!("Keyboard Shortcuts"));
                if ui.button(crate::tl!("Edit Keyboard Shortcuts...")).clicked() {
                    shortcuts = true;
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Export...")).clicked() {
                    io = Some("export");
                }
                if ui.button(crate::tl!("Import...")).clicked() {
                    io = Some("import");
                }
                if ui.button(crate::tl!("Reset to Defaults")).clicked() {
                    c = Custom::default();
                }
            });
        });
    if c != app.custom_ui {
        app.custom_ui = c;
    }
    match io {
        Some("export") => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p(&format!("Interface.{EXTENSION}"))) {
                let r = command(app, &json!({ "action": "export", "path": path }));
                app.session.echo(r.map_or_else(|e| e, |_| format!("Interface exported to {path}")));
            }
        }
        Some("import") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p())
                && let Err(e) = command(app, &json!({ "action": "import", "path": path }))
            {
                app.session.echo(e);
            }
        }
        _ => {}
    }
    if shortcuts {
        *open = false;
        app.ui.dialog = Some("aliasedit".into());
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;

    fn sample() -> Custom {
        Custom {
            qat: Some(vec!["new".into(), "line".into(), "(command \"circle\" \"0,0\" 5)".into()]),
            panels: vec![CustomPanel {
                tab: "Home".into(),
                name: "Mine".into(),
                buttons: vec![
                    CustomButton { cmd: "circle".into(), label: String::new() },
                    CustomButton { cmd: "dimlinear".into(), label: "Dim".into() },
                ],
            }],
            tabs: vec![CustomTab {
                name: "Electrical".into(),
                panels: vec![CustomPanel {
                    tab: String::new(),
                    name: "Devices".into(),
                    buttons: vec![CustomButton { cmd: "insert".into(), label: "Outlet".into() }],
                }],
            }],
            hidden: vec!["Home/Groups".into()],
        }
    }

    #[test]
    fn the_customisation_round_trips_through_the_profile() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        app.custom_ui = sample();
        let prefs = app.prefs_json();
        let mut again = CadApp::new(Session::new(), crate::Services::default());
        again.load_prefs(&prefs);
        assert_eq!(again.custom_ui, sample());
        // The default writes nothing.
        let plain = CadApp::new(Session::new(), crate::Services::default());
        assert!(!plain.prefs_json().contains("customUi"));
    }

    #[test]
    fn the_ribbon_and_toolbar_show_the_customisation() {
        let c = sample();
        let tabs = effective(&c);
        let home = tabs.iter().find(|t| t.name == "Home").unwrap();
        assert!(home.panels.iter().any(|p| p.name == "Mine"));
        assert!(!home.panels.iter().any(|p| p.name == "Groups"), "hidden");
        let elec = tabs.last().unwrap();
        assert_eq!(elec.name, "Electrical");
        let Item::Large(btn) = &elec.panels[0].items[0] else { panic!("a large button") };
        assert_eq!((btn.cmd, btn.label), ("insert", "Outlet"));
        // Laid out once: the same slice while unchanged.
        assert!(std::ptr::eq(effective(&c), tabs));
        let q = qat(&c);
        assert_eq!(q.iter().map(|(_, c, _)| *c).collect::<Vec<_>>(), ["new", "line", "(command \"circle\" \"0,0\" 5)"]);
        assert!(matches!(q[0].0, Icon::New), "a built-in icon");
        assert_eq!(q[1].2, "Line");
        // Nothing customised: the built-in ribbon itself.
        let plain = effective(&Custom::default());
        assert_eq!(plain.len(), crate::ribbon::TABS.len());
        assert!(plain.iter().zip(crate::ribbon::TABS).all(|(a, b)| a.name == b.name && a.panels.len() == b.panels.len()));
    }

    #[test]
    fn export_import_and_hostile_profiles() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        app.custom_ui = sample();
        let path = std::env::temp_dir().join(format!("cadaddy-cui-{}.cdyui", std::process::id()));
        command(&mut app, &json!({ "action": "export", "path": path.to_string_lossy() })).unwrap();
        app.custom_ui = Custom::default();
        command(&mut app, &json!({ "action": "import", "path": path.to_string_lossy() })).unwrap();
        assert_eq!(app.custom_ui, sample());
        std::fs::write(&path, "{\"format\": \"other\"}").unwrap();
        assert!(command(&mut app, &json!({ "action": "import", "path": path.to_string_lossy() })).is_err());
        let _ = std::fs::remove_file(&path);
        // Oversized and odd values are trimmed, not trusted.
        let huge =
            json!({ "qat": vec!["line"; 5000], "tabs": [{ "name": "  " }], "panels": [{ "tab": "Home", "name": "x", "buttons": [{ "cmd": "" }] }] });
        let c = Custom::from_json(&huge).unwrap();
        assert_eq!(c.qat.as_ref().map(Vec::len), Some(MAX_ITEMS));
        assert!(c.tabs.is_empty() && c.panels[0].buttons.is_empty());
        assert!(Custom::from_json(&json!({ "qat": 5 })).is_err());
        // Through the UI command too.
        assert!(app.run("ui.customize", json!({ "action": "reset" })).is_ok());
        assert!(app.custom_ui.is_default());
    }

    #[test]
    fn macros_run_at_the_command_line_and_contextual_tabs_follow_the_selection() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        run_button(&mut app, "^C^Ccircle;0,0;5");
        assert_eq!(app.session.doc().unwrap().model.len(), 1, "the macro drew a circle");
        let ctx = egui::Context::default();
        assert!(contextual(&app, &ctx).is_none());
        let h = app.run("hatch", json!({ "boundary": [[0, 0], [10, 0], [10, 10], [0, 10]] }));
        if let Ok(r) = h
            && let Some(h) = r["handle"].as_str().or_else(|| r["handles"][0].as_str())
        {
            let hh = cadaddy_engine::doc::Handle::parse_hex(h).unwrap();
            app.session.state_mut().unwrap().selection = vec![hh];
            assert_eq!(contextual(&app, &ctx).map(|t| t.name), Some("Hatch Editor"));
        }
        // Every contextual tab's commands exist.
        for tab in [&TEXT_EDITOR, &HATCH_EDITOR, &TABLE_CELL, &ARRAY, &BLOCK_EDITOR] {
            for p in tab.panels {
                for it in p.items {
                    let btns: &[Btn] = match it {
                        Item::Large(b) => std::slice::from_ref(b),
                        Item::Smalls(bs) => bs,
                        Item::Layers => &[],
                    };
                    for b in btns {
                        assert!(cadaddy_engine::find_command(b.cmd).is_some(), "{}", b.cmd);
                    }
                }
            }
        }
    }
}
