//! The canvas's right-click menu while no command runs, after AutoCAD's default and edit-mode
//! shortcut menus: Repeat the last command, the Clipboard commands, editing commands for a
//! selection, Undo/Redo, Pan/Zoom, Quick Select and Find. (While a command runs, a right-click
//! is Enter.) Every item starts a command, like choosing it from the menu bar.
//!
//! Shift+right-click or Ctrl+right-click opens the object snap menu instead ([`snap_entries`]):
//! its items type the point modifiers and snap overrides (`_from`, `_endp`, `_non`…) at the
//! current point prompt, exactly as if typed on the command line.

use cadcraft_engine::find_command;
use cadcraft_engine::snap::mode;

use crate::CadApp;
use crate::menus::{Entry, activate, entry_ui};

/// Clipboard ▸ (AutoCAD's order).
const CLIPBOARD: &[&str] = &["cutclip", "copyclip", "copybase", "pasteclip", "pasteblock", "pasteorig"];
/// With a selection: commands that act on it, and its label when it differs from the command's.
const SELECTION: &[(&str, Option<&str>)] = &[
    ("erase", None),
    ("move", None),
    ("copy", Some("Copy Selection")),
    ("scale", None),
    ("rotate", None),
    ("ai_deselect", None),
    ("properties", None),
];
const VIEW: &[&str] = &["u", "redo", "pan", "zoom"];
const TOOLS: &[&str] = &["qselect", "find"];

fn item(app: &CadApp, id: &str, label: Option<&str>) -> Option<Entry> {
    let spec = find_command(id)?;
    Some(Entry::Item {
        label: label.unwrap_or(spec.label).to_string(),
        id: id.to_string(),
        shortcut: spec.shortcut.map(str::to_string),
        enabled: (spec.enabled)(&app.session).is_ok(),
    })
}

/// The menu's groups of entries (separated by lines).
pub fn entries(app: &CadApp) -> Vec<Vec<Entry>> {
    let mut groups = Vec::new();
    if let Some(last) = app.session.last_command.as_deref().and_then(find_command) {
        let label = crate::tf!("Repeat {command}", command = last.id.to_ascii_uppercase());
        groups.push(vec![Entry::Item { label, id: last.id.to_string(), shortcut: None, enabled: (last.enabled)(&app.session).is_ok() }]);
    }
    let clip = CLIPBOARD.iter().filter_map(|id| item(app, id, None)).collect();
    groups.push(vec![Entry::Sub { label: "Clipboard".into(), children: clip }]);
    if !app.session.selection().is_empty() {
        groups.push(SELECTION.iter().filter_map(|(id, label)| item(app, id, *label)).collect());
    }
    groups.push(VIEW.iter().filter_map(|id| item(app, id, None)).collect());
    groups.push(TOOLS.iter().filter_map(|id| item(app, id, None)).collect());
    groups
}

/// The object snap menu's groups: (label, what it types) per item; the label of a snap mode is
/// its name in [`mode::ALL`].
const SNAP_GROUPS: &[&[(u32, &str)]] = &[
    &[(mode::END, "_endp"), (mode::MID, "_mid"), (mode::INT, "_int"), (mode::APP, "_app"), (mode::EXT, "_ext")],
    &[(mode::CEN, "_cen"), (mode::GCEN, "_gcen"), (mode::QUA, "_qua"), (mode::TAN, "_tan")],
    &[(mode::PER, "_per"), (mode::PAR, "_par"), (mode::NOD, "_nod"), (mode::INS, "_ins"), (mode::NEA, "_nea")],
];
/// The point filters submenu.
const FILTERS: &[&str] = &[".X", ".Y", ".Z", ".XY", ".XZ", ".YZ"];
/// The id of the Osnap Settings item (the Drafting Settings dialog).
const OSNAP_SETTINGS: &str = "dsettings";

/// The object snap menu (Shift/Ctrl+right-click): From, Mid Between 2 Points, Point Filters, the
/// snap modes, None and Osnap Settings. An item's id is the text it types; the modifiers are
/// only available at a point prompt.
pub fn snap_entries(app: &CadApp) -> Vec<Vec<Entry>> {
    let on = app.session.current_prompt().is_some_and(|p| cadcraft_engine::pointmod::accepts(&p));
    let typed = |label: &str, id: &str| Entry::Item { label: label.into(), id: id.into(), shortcut: None, enabled: on };
    let name = |m: u32| mode::ALL.iter().find(|(b, _)| *b == m).map_or("", |(_, n)| *n);
    let mut groups = vec![vec![
        typed("From", "_from"),
        typed("Mid Between 2 Points", "_m2p"),
        Entry::Sub { label: "Point Filters".into(), children: FILTERS.iter().map(|f| typed(f, &f.to_ascii_lowercase())).collect() },
    ]];
    groups.extend(SNAP_GROUPS.iter().map(|g| g.iter().map(|(m, id)| typed(name(*m), id)).collect()));
    groups.push(vec![typed("None", "_non")]);
    groups.push(vec![Entry::Item { label: "Osnap Settings...".into(), id: OSNAP_SETTINGS.into(), shortcut: None, enabled: true }]);
    groups
}

/// Run an item of the object snap menu: type its modifier at the prompt, or open the settings.
pub fn choose_snap(app: &mut CadApp, id: &str) {
    if id == OSNAP_SETTINGS {
        activate(app, id);
    } else {
        app.cmdline(id);
    }
}

/// Attach the menu to the canvas response: it opens on a right-click, as the object snap menu
/// when that click held Shift or Ctrl (`CanvasState::snap_menu`).
pub fn show(app: &mut CadApp, resp: &egui::Response) {
    let mut clicked = None;
    let snap = app.canvas.snap_menu;
    resp.context_menu(|ui| {
        let groups = if snap { snap_entries(app) } else { entries(app) };
        for (i, group) in groups.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            for e in group {
                entry_ui(ui, e, &mut clicked);
            }
        }
    });
    match clicked {
        Some(id) if snap => choose_snap(app, &id),
        Some(id) => activate(app, &id),
        None => {}
    }
}
