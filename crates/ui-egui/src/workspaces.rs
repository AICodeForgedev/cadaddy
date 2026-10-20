//! Workspaces and the navigation bar.
//!
//! A workspace is a named arrangement of the interface: the ribbon or the tool bar, which ribbon
//! tabs show, the menu bar, the Tool Sets palette and its tab, the palettes and where they sit,
//! the layer list, the file tabs and the navigation bar. Three come with CADaddy (Drafting &
//! Annotation, 3D Modeling and an Electrical example laid out for placing symbols); the user
//! saves more from the current arrangement. The current one is the WSCURRENT system variable,
//! kept in the user profile: setting it (from the status bar's workspace menu, Tools ▸
//! Workspaces, WORKSPACE or typed WSCURRENT) applies the workspace. User workspaces are kept with
//! the interface state.
//!
//! The navigation bar runs down the drawing area's right edge, under the ViewCube: Pan, Zoom
//! Extents, Zoom Window, Zoom Previous and Orbit. NAVBARDISPLAY (and NAVBAR On/Off) shows it.

use egui::{Rect, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CadApp;
use crate::docking::Panel;
use crate::icons::{self, Icon};

/// Most user workspaces kept.
pub const MAX_USER: usize = 50;
/// Longest workspace name.
const MAX_NAME: usize = 64;

/// One workspace: what it shows and where.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Def {
    pub name: String,
    pub ribbon: bool,
    /// The ribbon tabs shown, by name; empty shows them all.
    pub ribbon_tabs: Vec<String>,
    pub ribbon_tab: String,
    pub show_toolbar: bool,
    pub in_window_menu: bool,
    pub show_toolsets: bool,
    pub toolset_tab: String,
    pub show_layer_list: bool,
    pub show_file_tabs: bool,
    /// The palettes and where they are; `None` leaves them as they are.
    pub docking: Option<crate::docking::Workspace>,
    pub navbar: bool,
}

impl Default for Def {
    fn default() -> Self {
        let ui = crate::UiState::default();
        Def {
            name: String::new(),
            ribbon: ui.ribbon,
            ribbon_tabs: Vec::new(),
            ribbon_tab: "Home".into(),
            show_toolbar: ui.show_toolbar,
            in_window_menu: ui.in_window_menu,
            show_toolsets: ui.show_toolsets,
            toolset_tab: ui.toolset_tab,
            show_layer_list: ui.show_layer_list,
            show_file_tabs: ui.show_file_tabs,
            docking: Some(crate::docking::Workspace::default()),
            navbar: true,
        }
    }
}

/// The workspaces that come with CADaddy.
pub fn built_in() -> Vec<Def> {
    let drafting = Def { name: cadaddy_engine::DEFAULT_WORKSPACE.into(), ..Def::default() };
    // 3D Modeling: the Tool Sets palette open on its Modeling tools.
    let modeling =
        Def { name: "3D Modeling".into(), ribbon_tab: "View".into(), show_toolsets: true, toolset_tab: "Modeling".into(), ..Def::default() };
    // The Electrical example: symbols out, the tabs for placing and annotating them, no Tool Sets.
    let mut docking = crate::docking::Workspace::default();
    let _ = docking.set_visible(Panel::Symbols, true);
    let _ = docking.set_visible(Panel::ToolSets, false);
    let electrical = Def {
        name: "Electrical".into(),
        ribbon_tabs: ["Home", "Insert", "Annotate", "View", "Output"].map(String::from).to_vec(),
        ribbon_tab: "Insert".into(),
        show_toolsets: false,
        docking: Some(docking),
        ..Def::default()
    };
    vec![drafting, modeling, electrical]
}

/// Every workspace: the built-in ones, then the user's.
pub fn all(app: &CadApp) -> Vec<Def> {
    let mut v = built_in();
    v.extend(app.ui.user_workspaces.iter().cloned());
    v
}

fn find(app: &CadApp, name: &str) -> Option<Def> {
    all(app).into_iter().find(|d| d.name.eq_ignore_ascii_case(name.trim()))
}

fn is_built_in(name: &str) -> bool {
    built_in().iter().any(|d| d.name.eq_ignore_ascii_case(name.trim()))
}

/// The current workspace's name (WSCURRENT).
pub fn current(app: &CadApp) -> String {
    cadaddy_engine::sysvars::get(&app.session, "WSCURRENT").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// The interface as it is now, as workspace `name`.
pub fn capture(app: &CadApp, name: &str) -> Def {
    let ui = &app.ui;
    Def {
        name: name.trim().to_string(),
        ribbon: ui.ribbon,
        ribbon_tabs: ui.ribbon_tabs.clone(),
        ribbon_tab: ui.ribbon_tab.clone(),
        show_toolbar: ui.show_toolbar,
        in_window_menu: ui.in_window_menu,
        show_toolsets: ui.show_toolsets,
        toolset_tab: ui.toolset_tab.clone(),
        show_layer_list: ui.show_layer_list,
        show_file_tabs: ui.show_file_tabs,
        docking: Some(ui.docking.clone()),
        navbar: navbar_shown(app),
    }
}

/// Arrange the interface as `d` says.
fn apply(app: &mut CadApp, d: &Def) {
    let ui = &mut app.ui;
    ui.ribbon = d.ribbon;
    ui.ribbon_tabs = d.ribbon_tabs.iter().filter(|t| crate::ribbon::TABS.iter().any(|tab| tab.name == t.as_str())).cloned().collect();
    ui.ribbon_tab = d.ribbon_tab.clone();
    ui.show_toolbar = d.show_toolbar;
    ui.in_window_menu = d.in_window_menu;
    ui.show_toolsets = d.show_toolsets;
    ui.toolset_tab = d.toolset_tab.clone();
    ui.show_layer_list = d.show_layer_list;
    ui.show_file_tabs = d.show_file_tabs;
    if let Some(dock) = d.docking.as_ref().filter(|w| w.validate().is_ok()) {
        ui.docking = dock.clone();
    }
    let _ = cadaddy_engine::sysvars::set(&mut app.session, "NAVBARDISPLAY", &json!(i32::from(d.navbar)));
    app.workspace_applied = Some(d.name.clone());
}

/// Make `name` the current workspace and arrange the interface for it.
pub fn set(app: &mut CadApp, name: &str) -> Result<(), String> {
    let d = find(app, name).ok_or_else(|| format!("no workspace \"{}\"", name.trim()))?;
    cadaddy_engine::sysvars::set(&mut app.session, "WSCURRENT", &json!(d.name)).map_err(|e| e.user_message())?;
    apply(app, &d);
    Ok(())
}

/// Save the interface as it is now as user workspace `name` (replacing one of that name) and
/// make it current.
pub fn save(app: &mut CadApp, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!("a workspace name has 1 to {MAX_NAME} characters"));
    }
    if is_built_in(name) {
        return Err(format!("\"{name}\" comes with CADaddy; save under another name"));
    }
    let d = capture(app, name);
    let full = app.ui.user_workspaces.len() >= MAX_USER;
    match app.ui.user_workspaces.iter_mut().find(|w| w.name.eq_ignore_ascii_case(name)) {
        Some(w) => *w = d,
        None if full => return Err(format!("at most {MAX_USER} workspaces")),
        None => app.ui.user_workspaces.push(d),
    }
    cadaddy_engine::sysvars::set(&mut app.session, "WSCURRENT", &json!(name)).map_err(|e| e.user_message())?;
    app.workspace_applied = Some(name.to_string());
    Ok(())
}

/// Delete user workspace `name`; the current one falls back to the default.
pub fn delete(app: &mut CadApp, name: &str) -> Result<(), String> {
    if is_built_in(name) {
        return Err(format!("\"{}\" comes with CADaddy and can't be deleted", name.trim()));
    }
    let n = app.ui.user_workspaces.len();
    app.ui.user_workspaces.retain(|w| !w.name.eq_ignore_ascii_case(name.trim()));
    if app.ui.user_workspaces.len() == n {
        return Err(format!("no workspace \"{}\"", name.trim()));
    }
    if current(app).eq_ignore_ascii_case(name.trim()) {
        set(app, cadaddy_engine::DEFAULT_WORKSPACE)?;
    }
    Ok(())
}

/// Each frame: WSCURRENT set some other way (typed, a script, the profile) applies its workspace.
/// At start-up the interface keeps the arrangement it was left in.
pub fn follow(app: &mut CadApp) {
    let cur = current(app);
    match &app.workspace_applied {
        None => app.workspace_applied = Some(cur),
        Some(a) if *a == cur => {}
        Some(prev) => {
            let prev = prev.clone();
            if set(app, &cur).is_err() {
                app.session.echo(format!("No workspace \"{cur}\"; the workspace stays \"{prev}\"."));
                let _ = cadaddy_engine::sysvars::set(&mut app.session, "WSCURRENT", &json!(prev));
                app.workspace_applied = Some(prev);
            }
        }
    }
}

/// The ribbon tabs the current workspace shows.
pub fn tab_shown(app: &CadApp, name: &str) -> bool {
    app.ui.ribbon_tabs.is_empty() || app.ui.ribbon_tabs.iter().any(|t| t == name)
}

/// WORKSPACE {action?: list (default) | set | save | delete, name}.
pub fn run(app: &mut CadApp, params: &Value) -> Result<Value, String> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let action = params.get("action").and_then(Value::as_str).unwrap_or(if name.is_empty() { "list" } else { "set" }).to_ascii_lowercase();
    match action.as_str() {
        "list" => {}
        "set" => set(app, &name)?,
        "save" | "saveas" => save(app, &name)?,
        "delete" => delete(app, &name)?,
        other => return Err(format!("unknown action \"{other}\" (list, set, save or delete)")),
    }
    let list: Vec<Value> = all(app).iter().map(|d| json!({ "name": d.name, "builtIn": is_built_in(&d.name) })).collect();
    Ok(json!({ "current": current(app), "workspaces": list }))
}

pub fn navbar_shown(app: &CadApp) -> bool {
    cadaddy_engine::sysvars::get(&app.session, "NAVBARDISPLAY").and_then(|v| v.as_i64()) != Some(0)
}

/// NAVBAR {on?: bool} (no value: toggle).
pub fn run_navbar(app: &mut CadApp, params: &Value) -> Result<Value, String> {
    let on = match params.get("on") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("on") => true,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("off") => false,
        None | Some(Value::Null) => !navbar_shown(app),
        Some(_) => return Err("`on` is true or false".into()),
    };
    cadaddy_engine::sysvars::set(&mut app.session, "NAVBARDISPLAY", &json!(i32::from(on))).map_err(|e| e.user_message())?;
    Ok(json!({ "on": on }))
}

/// The status bar's workspace menu: every workspace (the current one ticked), then Save
/// Current As and Workspace Settings.
pub fn menu(app: &mut CadApp, ui: &mut egui::Ui) {
    let cur = current(app);
    let mut pick: Option<String> = None;
    for d in all(app) {
        if ui.radio(d.name.eq_ignore_ascii_case(&cur), crate::i18n::t(&d.name)).clicked() {
            pick = Some(d.name.clone());
        }
    }
    ui.separator();
    if ui.button(crate::tl!("Save Current As...")).clicked() {
        app.ui.dialog = Some("workspace.save".into());
    }
    if ui.button(crate::tl!("Workspace Settings...")).clicked() {
        app.ui.dialog = Some("workspace.settings".into());
    }
    let mut nav = navbar_shown(app);
    if ui.checkbox(&mut nav, crate::tl!("Navigation Bar")).changed() {
        let _ = run_navbar(app, &json!({ "on": nav }));
    }
    if let Some(name) = pick
        && let Err(e) = set(app, &name)
    {
        app.session.echo(e);
    }
}

/// Save Current As: a name for the interface as it is now.
pub fn save_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("cadaddy-workspace-save");
    let mut name: String = ctx.data(|d| d.get_temp(id)).unwrap_or_else(|| {
        let cur = current(app);
        if is_built_in(&cur) { String::new() } else { cur }
    });
    let mut done = false;
    egui::Window::new(crate::tl!("Save Workspace"))
        .id(egui::Id::new("Save Workspace"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(crate::tl!("Name"));
            let r = ui.text_edit_singleline(&mut name);
            crate::icons::describe_area(&r, "Name");
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.add_enabled(!name.trim().is_empty(), egui::Button::new(crate::tl!("Save"))).clicked() || (enter && !name.trim().is_empty()) {
                match save(app, &name) {
                    Ok(()) => done = true,
                    Err(e) => app.session.echo(e),
                }
            }
        });
    if done {
        *open = false;
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id, name));
    } else {
        ctx.data_mut(|d| d.remove::<String>(id));
    }
}

/// Workspace Settings: switch, delete the user's workspaces, the navigation bar.
pub fn settings_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let cur = current(app);
    let mut action: Option<(bool, String)> = None;
    egui::Window::new(crate::tl!("Workspace Settings"))
        .id(egui::Id::new("Workspace Settings"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(crate::tl!("Workspaces")).strong());
            for d in all(app) {
                ui.horizontal(|ui| {
                    if ui.radio(d.name.eq_ignore_ascii_case(&cur), crate::i18n::t(&d.name)).clicked() {
                        action = Some((false, d.name.clone()));
                    }
                    if !is_built_in(&d.name) && ui.small_button("✖").on_hover_text(crate::tl!("Delete")).clicked() {
                        action = Some((true, d.name.clone()));
                    }
                });
            }
            ui.separator();
            let mut nav = navbar_shown(app);
            if ui.checkbox(&mut nav, crate::tl!("Navigation Bar")).changed() {
                let _ = run_navbar(app, &json!({ "on": nav }));
            }
        });
    match action {
        Some((false, name)) => {
            if let Err(e) = set(app, &name) {
                app.session.echo(e);
            }
        }
        Some((true, name)) => {
            if let Err(e) = delete(app, &name) {
                app.session.echo(e);
            }
        }
        None => {}
    }
}

/// The navigation bar's tools: (icon, command, label).
pub const NAV_TOOLS: [(Icon, &str, &str); 5] = [
    (Icon::Pan, "pan", "Pan"),
    (Icon::ZoomExtents, "zoom.extents", "Zoom Extents"),
    (Icon::ZoomWindow, "zoom.window", "Zoom Window"),
    (Icon::ZoomWindow, "zoom.previous", "Zoom Previous"),
    (Icon::Orbit, "3dorbit", "Orbit"),
];

/// Where the navigation bar sits in drawing area `canvas`: the right edge, under the ViewCube
/// when that shows.
pub fn navbar_rect(canvas: Rect, under: Option<f32>) -> Rect {
    let size = 26.0;
    let top = under.unwrap_or(canvas.top() + 12.0) + 10.0;
    let height = size * (NAV_TOOLS.len() as f32 + 1.0) + 8.0;
    Rect::from_min_size(pos2(canvas.right() - size - 14.0, top), vec2(size + 4.0, height))
}

/// Draw the navigation bar in `canvas` (NAVBARDISPLAY 1).
pub fn navbar(app: &mut CadApp, ui: &mut egui::Ui, canvas: Rect, under: Option<f32>) {
    if !navbar_shown(app) {
        return;
    }
    let t = crate::theme::Tokens::get();
    let r = navbar_rect(canvas, under);
    if r.bottom() > canvas.bottom() - 8.0 {
        return;
    }
    let p = ui.painter_at(canvas);
    p.rect_filled(r, 4.0, t.panel.gamma_multiply(0.92));
    p.rect_stroke(r, 4.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let mut run: Option<&str> = None;
    let mut y = r.top() + 4.0;
    for (icon, cmd, label) in NAV_TOOLS {
        let br = Rect::from_min_size(pos2(r.left() + 2.0, y), vec2(26.0, 26.0));
        let resp = ui.interact(br, ui.id().with(("navbar", cmd)), Sense::click());
        if resp.hovered() {
            p.rect_filled(br, 3.0, t.control_hover);
        }
        icons::paint(&p, br.shrink(4.0), icon, false);
        icons::describe_control(ui, &resp, label, None, false);
        if resp.on_hover_text(crate::i18n::t(label)).clicked() {
            run = Some(cmd);
        }
        y += 26.0;
    }
    // Close: NAVBARDISPLAY 0.
    let br = Rect::from_min_size(pos2(r.left() + 2.0, y), vec2(26.0, 26.0));
    let resp = ui.interact(br, ui.id().with("navbar-close"), Sense::click());
    if resp.hovered() {
        p.rect_filled(br, 3.0, t.control_hover);
    }
    icons::paint(&p, br.shrink(7.0), Icon::Close, false);
    icons::describe_control(ui, &resp, "Close Navigation Bar", None, false);
    if resp.on_hover_text(crate::tl!("Close Navigation Bar")).clicked() {
        let _ = run_navbar(app, &json!({ "on": false }));
    }
    if let Some(cmd) = run {
        app.start(cmd);
    }
}
