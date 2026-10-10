//! The canvas's right-click menu while no command runs, after AutoCAD's default and edit-mode
//! shortcut menus: Repeat the last command, the Clipboard commands, editing commands for a
//! selection, Undo/Redo, Pan/Zoom, Quick Select and Find. (While a command runs, a right-click
//! is Enter.) Every item starts a command, like choosing it from the menu bar.

use cadcraft_engine::find_command;

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

/// Attach the menu to the canvas response: it opens on a right-click.
pub fn show(app: &mut CadApp, resp: &egui::Response) {
    let mut clicked = None;
    resp.context_menu(|ui| {
        for (i, group) in entries(app).iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            for e in group {
                entry_ui(ui, e, &mut clicked);
            }
        }
    });
    if let Some(id) = clicked {
        activate(app, &id);
    }
}
