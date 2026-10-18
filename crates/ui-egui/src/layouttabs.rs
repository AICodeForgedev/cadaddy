//! The layout tabs' own state beyond the status bar drawing ([`crate::chrome::status_bar`]):
//! renaming a tab in place (double-click, or Rename in its menu), the tabs selected together
//! (Ctrl/Cmd-click, Select All Layouts: Delete acts on all of them), and the Move or Copy dialog
//! (before which tab, and whether to make a copy).

use serde_json::json;

use crate::CadApp;

/// The Move or Copy dialog while open.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveCopy {
    /// The layout to move or copy.
    pub name: String,
    /// The tab it goes before; `None`: to the end.
    pub before: Option<String>,
    pub copy: bool,
    /// The last command error, shown in the dialog.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutTabs {
    /// The tab being renamed in place: (layout, text typed so far).
    pub rename: Option<(String, String)>,
    /// Layout tabs selected together (besides the current one).
    pub selected: Vec<String>,
    pub move_copy: Option<MoveCopy>,
}

/// The drawing's layout names in tab order.
pub fn layout_names(app: &CadApp) -> Vec<String> {
    let Ok(d) = app.session.doc() else { return Vec::new() };
    let mut ls: Vec<_> = d.layouts.iter().collect();
    ls.sort_by_key(|l| l.tab_order);
    ls.into_iter().map(|l| l.name.clone()).collect()
}

/// Start renaming tab `name` in place.
pub fn start_rename(app: &mut CadApp, name: &str) {
    if name != "Model" {
        app.layout_tabs.rename = Some((name.to_string(), name.to_string()));
    }
}

/// Finish an in-place rename: `commit` renames (an unchanged or empty name changes nothing).
/// Returns the error to show, if the rename was refused.
pub fn finish_rename(app: &mut CadApp, commit: bool) -> Option<String> {
    let (from, to) = app.layout_tabs.rename.take()?;
    let to = to.trim().to_string();
    if !commit || to.is_empty() || to == from {
        return None;
    }
    match app.run("layout.rename", json!({ "from": from, "to": to })) {
        Ok(_) => {
            for n in &mut app.layout_tabs.selected {
                if *n == from {
                    n.clone_from(&to);
                }
            }
            None
        }
        Err(e) => {
            app.session.echo(e.clone());
            Some(e)
        }
    }
}

/// Ctrl/Cmd-click on a layout tab: add it to the selected tabs, or take it out.
pub fn toggle_selected(app: &mut CadApp, name: &str) {
    if name == "Model" {
        return;
    }
    let sel = &mut app.layout_tabs.selected;
    match sel.iter().position(|n| n == name) {
        Some(i) => {
            sel.remove(i);
        }
        None => sel.push(name.to_string()),
    }
}

/// Select All Layouts.
pub fn select_all(app: &mut CadApp) {
    app.layout_tabs.selected = layout_names(app);
}

/// The layouts a tab menu acts on: the selected tabs when `name` is one of them, else `name`.
pub fn targets(app: &CadApp, name: &str) -> Vec<String> {
    if app.layout_tabs.selected.iter().any(|n| n == name) { app.layout_tabs.selected.clone() } else { vec![name.to_string()] }
}

/// Delete the layouts a tab menu acts on (see [`targets`]); the drawing always keeps one.
pub fn delete(app: &mut CadApp, name: &str) {
    let names = targets(app, name);
    for n in &names {
        if layout_names(app).len() <= 1 {
            app.session.echo(crate::tl!("A drawing keeps at least one layout."));
            break;
        }
        if let Err(e) = app.run("layout.delete", json!({ "name": n })) {
            app.session.echo(e);
        }
    }
    app.layout_tabs.selected.retain(|n| !names.contains(n));
}

/// Open the Move or Copy dialog on layout `name`.
pub fn open_move_copy(app: &mut CadApp, name: &str) {
    if name != "Model" {
        app.layout_tabs.move_copy = Some(MoveCopy { name: name.to_string(), before: None, copy: false, error: None });
    }
}

/// The Move or Copy dialog's OK: move the layout before the chosen tab, or copy it there.
/// True when the dialog closes.
pub fn move_copy_ok(app: &mut CadApp) -> bool {
    let Some(mc) = app.layout_tabs.move_copy.clone() else { return true };
    let r = if mc.copy {
        let mut p = json!({ "from": mc.name });
        if let Some(b) = &mc.before {
            p["before"] = json!(b);
        }
        app.run("layout.copy", p)
    } else {
        match &mc.before {
            Some(b) => app.run("layout.move", json!({ "name": mc.name, "before": b })),
            None => app.run("layout.move", json!({ "name": mc.name })),
        }
    };
    match r {
        Ok(_) => {
            app.layout_tabs.move_copy = None;
            true
        }
        Err(e) => {
            if let Some(m) = app.layout_tabs.move_copy.as_mut() {
                m.error = Some(e);
            }
            false
        }
    }
}

/// The Move or Copy window, while open.
pub fn move_copy_dialog(app: &mut CadApp, ctx: &egui::Context) {
    let Some(mut mc) = app.layout_tabs.move_copy.clone() else { return };
    let names = layout_names(app);
    if !names.contains(&mc.name) {
        app.layout_tabs.move_copy = None;
        return;
    }
    let mut open = true;
    let (mut ok, mut cancel) = (false, false);
    egui::Window::new(crate::tl!("Move or Copy")).id(egui::Id::new("layout_move_copy")).open(&mut open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            ui.label(crate::tf!("Move or copy layout \"{name}\"", name = mc.name));
            ui.label(crate::tl!("Before layout:"));
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for n in names.iter().filter(|n| **n != mc.name) {
                    ui.selectable_value(&mut mc.before, Some(n.clone()), n);
                }
                ui.selectable_value(&mut mc.before, None, crate::tl!("(move to end)"));
            });
            ui.checkbox(&mut mc.copy, crate::tl!("Create a copy"));
            if let Some(e) = &mc.error {
                ui.colored_label(crate::theme::Tokens::get().warn, e);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ok = ui.button(crate::tl!("OK")).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
            });
        },
    );
    app.layout_tabs.move_copy = Some(mc);
    if ok {
        move_copy_ok(app);
    } else if cancel || !open {
        app.layout_tabs.move_copy = None;
    }
}

#[cfg(test)]
mod tests {
    use cadaddy_engine::Session;

    use super::*;
    use crate::Services;

    fn app() -> CadApp {
        let mut s = Session::new();
        s.new_drawing(false);
        // Layout1, Layout2 and a third.
        s.execute("layout.new", &json!({ "name": "Sheet3" })).unwrap();
        CadApp::new(s, Services::default())
    }

    #[test]
    fn rename_in_place_commits_or_cancels() {
        let mut app = app();
        start_rename(&mut app, "Layout1");
        if let Some((_, t)) = app.layout_tabs.rename.as_mut() {
            *t = "Plan".into();
        }
        assert_eq!(finish_rename(&mut app, true), None);
        assert_eq!(layout_names(&app).first().map(String::as_str), Some("Plan"));
        start_rename(&mut app, "Plan");
        if let Some((_, t)) = app.layout_tabs.rename.as_mut() {
            *t = "Other".into();
        }
        finish_rename(&mut app, false);
        assert!(layout_names(&app).contains(&"Plan".to_string()));
        // A name another layout has is refused and reported.
        start_rename(&mut app, "Plan");
        if let Some((_, t)) = app.layout_tabs.rename.as_mut() {
            *t = "Layout2".into();
        }
        assert!(finish_rename(&mut app, true).is_some());
        // Model can't be renamed.
        start_rename(&mut app, "Model");
        assert!(app.layout_tabs.rename.is_none());
    }

    #[test]
    fn move_or_copy_places_the_tab() {
        let mut app = app();
        assert_eq!(layout_names(&app), ["Layout1", "Layout2", "Sheet3"]);
        open_move_copy(&mut app, "Sheet3");
        if let Some(m) = app.layout_tabs.move_copy.as_mut() {
            m.before = Some("Layout1".into());
        }
        assert!(move_copy_ok(&mut app));
        assert_eq!(layout_names(&app), ["Sheet3", "Layout1", "Layout2"]);
        // To the end.
        open_move_copy(&mut app, "Sheet3");
        assert!(move_copy_ok(&mut app));
        assert_eq!(layout_names(&app), ["Layout1", "Layout2", "Sheet3"]);
        // A copy before Layout2.
        open_move_copy(&mut app, "Layout1");
        if let Some(m) = app.layout_tabs.move_copy.as_mut() {
            m.before = Some("Layout2".into());
            m.copy = true;
        }
        assert!(move_copy_ok(&mut app));
        assert_eq!(layout_names(&app), ["Layout1", "Layout1 (2)", "Layout2", "Sheet3"]);
        assert!(app.layout_tabs.move_copy.is_none());
        // One undo step each.
        app.session.undo().unwrap();
        assert_eq!(layout_names(&app), ["Layout1", "Layout2", "Sheet3"]);
    }

    /// Frames of the whole window with `events` in the last one.
    fn frames(app: &mut CadApp, ctx: &egui::Context, n: usize, events: Vec<egui::Event>) {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        for k in 0..n {
            let input =
                egui::RawInput { screen_rect: Some(rect), events: if k + 1 == n { events.clone() } else { Vec::new() }, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.ui(ui));
            out.textures_delta.clear();
        }
    }

    #[test]
    fn the_tab_turns_into_a_text_field_until_escape() {
        let mut app = app();
        let ctx = egui::Context::default();
        app.logic(&ctx);
        start_rename(&mut app, "Layout2");
        frames(&mut app, &ctx, 5, Vec::new());
        assert!(app.layout_tabs.rename.is_some(), "still renaming while the field has focus");
        let esc = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        frames(&mut app, &ctx, 1, vec![esc]);
        assert!(app.layout_tabs.rename.is_none());
        assert_eq!(layout_names(&app), ["Layout1", "Layout2", "Sheet3"]);
        // The Move or Copy window draws too.
        open_move_copy(&mut app, "Sheet3");
        frames(&mut app, &ctx, 3, Vec::new());
        assert!(app.layout_tabs.move_copy.is_some());
    }

    #[test]
    fn select_all_then_delete_keeps_one_layout() {
        let mut app = app();
        select_all(&mut app);
        assert_eq!(app.layout_tabs.selected.len(), 3);
        assert_eq!(targets(&app, "Layout2").len(), 3);
        delete(&mut app, "Layout2");
        assert_eq!(layout_names(&app).len(), 1);
        assert!(app.layout_tabs.selected.is_empty() || app.layout_tabs.selected.iter().all(|n| layout_names(&app).contains(n)));
        // Ctrl-click toggles.
        let mut app = super::tests::app();
        toggle_selected(&mut app, "Layout1");
        toggle_selected(&mut app, "Sheet3");
        assert_eq!(targets(&app, "Sheet3"), ["Layout1", "Sheet3"]);
        toggle_selected(&mut app, "Layout1");
        assert_eq!(targets(&app, "Layout2"), ["Layout2"]);
    }
}
