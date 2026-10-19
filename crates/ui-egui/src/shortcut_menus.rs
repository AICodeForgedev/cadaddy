//! Right-click menus outside the drawing area, after AutoCAD's shortcut menus: the command line,
//! the drawing (file) tabs, the layout tabs, and the Layers and Properties palettes. (The
//! drawing area's menus are [`crate::context_menu`].)
//!
//! Every item goes through a command: an item id is a command id, started like a menu item, or
//! one of the forms [`choose`] reads: `run:<command>:<JSON params>`, `layout:<op>:<name>`,
//! `drawing:<op>:<index>` and `cmdline:<op>`. Items for commands that aren't registered are
//! left out. The External References palette's rows and the Properties palette's rows have menus
//! too.

use cadaddy_engine::{Input, find_command};
use serde_json::{Value, json};

use crate::CadApp;
use crate::menus::{Entry, activate};

/// How many recent commands the command line menu lists.
const RECENT: usize = 8;
/// The command line history heights offered (lines).
const LINES: &[usize] = &[0, 1, 3, 5, 8, 12];

fn item(app: &CadApp, id: &str, label: Option<&str>) -> Option<Entry> {
    let spec = find_command(id).map(|s| (s.label, s.shortcut, (s.enabled)(&app.session).is_ok()));
    let ui = crate::menus::UI_COMMANDS.iter().find(|c| c.0 == id).map(|c| (c.1, c.3, true));
    let (default, shortcut, enabled) = spec.or(ui)?;
    Some(Entry::Item { label: label.unwrap_or(default).to_string(), id: id.to_string(), shortcut: shortcut.map(str::to_string), enabled })
}

/// An item that is not a command of its own (it runs one with parameters, or acts on a tab).
fn action(label: &str, id: String, enabled: bool) -> Entry {
    Entry::Item { label: label.into(), id, shortcut: None, enabled }
}

/// An item running `command` with `params`, when the command exists.
fn run(label: &str, command: &str, params: Value) -> Option<Entry> {
    let known = find_command(command).is_some() || crate::menus::is_ui_command(command);
    known.then(|| action(label, format!("run:{command}:{params}"), true))
}

fn push(groups: &mut Vec<Vec<Entry>>, group: Vec<Entry>) {
    if !group.is_empty() {
        groups.push(group);
    }
}

/// The command line's menu: Recent Commands, Dynamic Input, the history window and its height,
/// Copy History and Clear.
pub fn command_line_entries(app: &CadApp) -> Vec<Vec<Entry>> {
    let mut groups = Vec::new();
    let recent: Vec<Entry> = app
        .session
        .recent_commands(RECENT)
        .iter()
        .filter_map(|id| find_command(id))
        .map(|c| Entry::Item { label: c.id.to_ascii_uppercase(), id: c.id.to_string(), shortcut: None, enabled: (c.enabled)(&app.session).is_ok() })
        .collect();
    if !recent.is_empty() {
        groups.push(vec![Entry::Sub { label: "Recent Commands".into(), children: recent }]);
    }
    push(&mut groups, ["dynmode", "ui.toggle.history"].iter().filter_map(|id| item(app, id, None)).collect());
    let lines: Vec<Entry> = LINES.iter().filter_map(|n| run(&n.to_string(), "ui.cmdline.lines", json!({ "lines": n }))).collect();
    if !lines.is_empty() {
        groups.push(vec![Entry::Sub { label: "Lines of Command History".into(), children: lines }]);
    }
    let mut edit = vec![
        action("Copy History", "cmdline:copy".into(), !app.session.log.is_empty()),
        action("Paste to Command Line", "cmdline:paste".into(), app.services.read_clipboard.is_some()),
    ];
    edit.extend(item(app, "ui.cmdline.clear", None));
    groups.push(edit);
    groups
}

/// A drawing tab's menu (`index` into the open drawings): New, Open, Save, Save As, Save All,
/// Close, Close All Other Drawings and Copy Full File Path. Save and Save As act on that drawing.
pub fn drawing_tab_entries(app: &CadApp, index: usize) -> Vec<Vec<Entry>> {
    let path = app.session.docs.get(index).and_then(|d| d.path.clone());
    let others = app.session.docs.len() > 1;
    let mut groups = Vec::new();
    push(&mut groups, ["new", "ui.open"].iter().filter_map(|id| item(app, id, None)).collect());
    let save: Vec<Entry> = [("qsave", "Save"), ("ui.saveas", "Save As...")]
        .iter()
        .filter_map(|(id, label)| item(app, id, Some(label)).map(|_| action(label, format!("drawing:{id}:{index}"), true)))
        .chain(item(app, "saveall", None))
        .collect();
    push(&mut groups, save);
    let close = vec![
        action("Close", format!("drawing:close:{index}"), true),
        action("Close All Other Drawings", format!("drawing:closeothers:{index}"), others),
    ];
    push(&mut groups, close);
    groups.push(vec![action("Copy Full File Path", format!("drawing:copypath:{index}"), path.is_some())]);
    groups
}

/// A layout tab's menu (`name` is the layout, "Model" the model tab): New Layout, From Template,
/// Delete, Rename and Move or Copy (layouts only), Select All Layouts, Page Setup Manager and
/// Print. Rename edits the tab in place; Delete acts on every selected tab when this one is
/// selected ([`crate::layouttabs`]).
pub fn layout_tab_entries(app: &CadApp, name: &str) -> Vec<Vec<Entry>> {
    let model = name == "Model";
    let layout = |op: &str, label: &str| action(label, format!("layout:{op}:{name}"), true);
    let mut groups = Vec::new();
    let mut new: Vec<Entry> = item(app, "layout.new", None).map(|_| layout("new", "New Layout")).into_iter().collect();
    if find_command("layout").is_some() {
        new.push(layout("template", "From Template..."));
        if !model {
            new.extend([layout("delete", "Delete Layout"), layout("rename", "Rename Layout"), layout("movecopy", "Move or Copy...")]);
        }
    }
    push(&mut groups, new);
    groups.push(vec![action("Select All Layouts", format!("layout:selectall:{name}"), true)]);
    let out: Vec<Entry> = [("pagesetup", "Page Setup Manager..."), ("plot", "Print...")]
        .iter()
        .filter(|(id, _)| find_command(id).is_some())
        .map(|(id, label)| layout(id, label))
        .collect();
    push(&mut groups, out);
    groups
}

/// A layer's menu in the Layers palette: Set Current, On/Off, Freeze/Thaw, Lock/Unlock, Delete and
/// the Layer Properties Manager.
pub fn layer_entries(app: &CadApp, name: &str) -> Vec<Vec<Entry>> {
    let Ok(d) = app.session.doc() else { return Vec::new() };
    let Some(l) = d.layers.iter().find(|l| l.name.eq_ignore_ascii_case(name)) else { return Vec::new() };
    let set = |label: &str, key: &str, value: bool| run(label, "layer.set", json!({ "name": name, key: value }));
    let mut groups = Vec::new();
    push(&mut groups, run("Set Current", "layer.current", json!({ "name": name })).into_iter().collect());
    push(
        &mut groups,
        [
            if l.on { set("Turn Off Layer", "on", false) } else { set("Turn On Layer", "on", true) },
            if l.frozen { set("Thaw Layer", "frozen", false) } else { set("Freeze Layer", "frozen", true) },
            if l.locked { set("Unlock Layer", "locked", false) } else { set("Lock Layer", "locked", true) },
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    push(&mut groups, run("Delete Layer", "layer.delete", json!({ "name": name })).into_iter().collect());
    push(&mut groups, item(app, "ui.dialog.layers", None).into_iter().collect());
    groups
}

/// An external reference's menu in the External References palette: Open, Attach DWG, Unload,
/// Reload, Detach, Bind and Insert (as the palette's buttons do).
pub fn xref_entries(app: &CadApp, name: &str, status: &str) -> Vec<Vec<Entry>> {
    let loaded = status == "Loaded" || status == "Unreferenced";
    let op = |option: &str| json!({ "option": option, "names": name });
    let mut groups = Vec::new();
    push(&mut groups, run("Open", "xopen", json!({ "name": name })).into_iter().chain(item(app, "xattach", Some("Attach DWG..."))).collect());
    push(
        &mut groups,
        [
            (status != "Unloaded").then(|| run("Unload", "-xref", op("unload"))).flatten(),
            run("Reload", "-xref", op("reload")),
            run("Detach", "-xref", op("detach")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    if loaded {
        push(
            &mut groups,
            [run("Bind", "-xref", op("bind")), run("Insert", "-xref", json!({ "option": "bind", "names": name, "insert": true }))]
                .into_iter()
                .flatten()
                .collect(),
        );
    }
    groups
}

/// SELECTSIMILARMODE's bit for a Properties palette row (the properties Select Similar compares).
fn similar_bit(label: &str) -> Option<u32> {
    Some(match label {
        "Color" => 1,
        "Layer" => 2,
        "Linetype" => 4,
        "Linetype scale" | "Linetype Scale" => 8,
        "Lineweight" => 16,
        "Plot style" | "Plot Style" => 32,
        _ => return None,
    })
}

/// A Properties palette row's menu (`label` is the row's property): Select Similar by that
/// property (with objects selected), Quick Select, Select All and Deselect All. Built without the
/// app: the rows are drawn while it is borrowed (see [`crate::palettes`]).
pub fn property_row_entries(label: &str, has_selection: bool, mine: bool) -> Vec<Vec<Entry>> {
    let mut groups = Vec::new();
    // The palette's "My" view: rows the user picked.
    let my = if mine {
        run("Remove from My Properties", "ui.properties", json!({ "remove": label }))
    } else {
        run("Add to My Properties", "ui.properties", json!({ "add": label }))
    };
    push(&mut groups, my.into_iter().collect());
    if has_selection && let Some(bit) = similar_bit(label) {
        push(&mut groups, run("Select Similar", "selectsimilar", json!({ "mode": bit })).into_iter().collect());
    }
    let cmd = |id: &str| {
        find_command(id).map(|s| Entry::Item {
            label: s.label.to_string(),
            id: id.to_string(),
            shortcut: s.shortcut.map(str::to_string),
            enabled: true,
        })
    };
    push(&mut groups, ["qselect", "selectall", "ai_deselect"].iter().filter_map(|id| cmd(id)).collect());
    groups
}

/// The Properties palette's menu: Quick Select, Select All, Deselect All, and hiding the palette.
pub fn properties_entries(app: &CadApp) -> Vec<Vec<Entry>> {
    let mut groups = Vec::new();
    push(&mut groups, ["qselect", "selectall", "ai_deselect"].iter().filter_map(|id| item(app, id, None)).collect());
    push(&mut groups, run("Hide Palette", "ui.dock", json!({ "operation": "close", "panel": "properties" })).into_iter().collect());
    push(&mut groups, window_entry(app, crate::docking::Panel::Properties).into_iter().collect());
    groups
}

/// Float a palette in a window of its own (to put it on another screen), or dock it back.
pub fn window_entry(app: &CadApp, panel: crate::docking::Panel) -> Option<Entry> {
    if app.ui.docking.is_detached(panel) {
        run("Dock Palette", "ui.dock", json!({ "operation": "dock", "panel": panel.id() }))
    } else {
        run("Float in Own Window", "ui.dock", json!({ "operation": "detach", "panel": panel.id() }))
    }
}

/// Run a chosen item. `ctx` is for the items that copy text to the system clipboard.
pub fn choose(app: &mut CadApp, ctx: &egui::Context, id: &str) {
    if let Some(rest) = id.strip_prefix("run:") {
        let Some((command, params)) = rest.split_once(':') else { return };
        let params = serde_json::from_str(params).unwrap_or(Value::Null);
        if let Err(e) = app.run(command, params) {
            app.session.echo(e);
        }
    } else if id == "cmdline:copy" {
        ctx.copy_text(app.session.log.join("\n"));
    } else if id == "cmdline:paste" {
        // Into the command line as typed: one line (line breaks become spaces), not run yet.
        if let Some(text) = app.services.read_clipboard.as_ref().and_then(|f| f()) {
            let line: String = text.split(['\r', '\n']).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
            app.cmd.buffer.push_str(&line);
        }
    } else if let Some(rest) = id.strip_prefix("drawing:") {
        let Some((op, index)) = rest.split_once(':') else { return };
        let Ok(index) = index.parse::<usize>() else { return };
        drawing_action(app, ctx, op, index);
    } else if let Some(rest) = id.strip_prefix("layout:") {
        let Some((op, name)) = rest.split_once(':') else { return };
        layout_action(app, op, name);
    } else {
        activate(app, id);
    }
}

fn drawing_action(app: &mut CadApp, ctx: &egui::Context, op: &str, index: usize) {
    match op {
        "close" => app.request_close(Some(index)),
        "closeothers" => app.request_close_others(index),
        "copypath" => {
            if let Some(p) = app.session.docs.get(index).and_then(|d| d.path.clone()) {
                ctx.copy_text(p);
            }
        }
        // Save, Save As: on that drawing.
        command => {
            if index != app.session.active {
                let _ = app.run("document.switch", json!({ "index": index }));
            }
            app.ui.start_tab = false;
            activate(app, command);
        }
    }
}

/// Give the running command a keyword, then type `text` (a name; the prompt takes it whole).
fn keyword_then(app: &mut CadApp, keyword: &str, text: Option<&str>) {
    if let Err(e) = app.session.input(Input::Keyword(keyword.into())) {
        app.session.echo(e.to_string());
        return;
    }
    if let Some(t) = text {
        app.cmdline(t);
    }
}

fn layout_action(app: &mut CadApp, op: &str, name: &str) {
    match op {
        "new" => {
            if let Ok(v) = app.run("layout.new", json!({}))
                && let Some(n) = v.get("name").and_then(Value::as_str)
            {
                let _ = app.run("layout.set", json!({ "name": n }));
            }
        }
        "pagesetup" | "plot" => {
            let _ = app.run("layout.set", json!({ "name": name }));
            activate(app, op);
        }
        "rename" => crate::layouttabs::start_rename(app, name),
        "movecopy" => crate::layouttabs::open_move_copy(app, name),
        "selectall" => crate::layouttabs::select_all(app),
        "delete" => crate::layouttabs::delete(app, name),
        // LAYOUT's options, as typed: Template asks for the file, Copy for the new name.
        "template" | "copy" => {
            activate(app, "layout");
            if app.session.running.as_ref().is_some_and(|r| r.id == "layout") {
                let keyword = if op == "template" { "Template" } else { "Copy" };
                keyword_then(app, keyword, (op != "template").then_some(name));
            }
        }
        _ => {}
    }
}

/// Show `groups` as the context menu of `resp` and run the chosen item.
pub fn attach(app: &mut CadApp, resp: &egui::Response, groups: impl FnOnce(&CadApp) -> Vec<Vec<Entry>>) {
    let mut clicked = None;
    resp.context_menu(|ui| crate::context_menu::groups_ui(ui, &groups(app), &mut clicked));
    if let Some(id) = clicked {
        choose(app, &resp.ctx, &id);
    }
}

/// A palette's content with a right-click menu on its background (rows with menus of their own
/// keep theirs).
pub fn palette(app: &mut CadApp, ui: &mut egui::Ui, content: fn(&mut CadApp, &mut egui::Ui), groups: fn(&CadApp) -> Vec<Vec<Entry>>) {
    let r = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| content(app, ui));
    attach(app, &r.response, groups);
}
