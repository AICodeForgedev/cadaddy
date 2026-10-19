//! Application panel identities and persistence around the shared docking renderer.
//! Document contents remain owned by their existing view implementations.

use craft_ui::docking::{Action, DockArea, DockStyle, Layout, Location, Node, PanelLimits, Permissions, Placement, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Panel {
    Canvas,
    ToolSets,
    QuickCalc,
    Layers,
    Properties,
    #[serde(rename = "externalReferences")]
    ExternalRefs,
    /// The symbol library (plumbing fixtures and piping symbols).
    Symbols,
}

impl Panel {
    pub const ALL: &[Self] = &[Self::Canvas, Self::ToolSets, Self::Layers, Self::Properties, Self::ExternalRefs, Self::Symbols];
    pub fn id(self) -> &'static str {
        match self {
            Self::Canvas => "canvas",
            Self::ToolSets => "toolSets",
            Self::QuickCalc => "quickCalc",
            Self::Layers => "layers",
            Self::Properties => "properties",
            Self::ExternalRefs => "externalReferences",
            Self::Symbols => "symbols",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Drawing",
            Self::ToolSets => "Tool Sets",
            Self::QuickCalc => "QuickCalc",
            Self::Layers => "Layers",
            Self::Properties => "Properties",
            Self::ExternalRefs => "External References",
            Self::Symbols => "Symbols",
        }
    }
    fn default_side(self) -> Zone {
        match self {
            Self::Canvas => Zone::Right,
            Self::ToolSets => Zone::Left,
            Self::QuickCalc => Zone::Right,
            Self::Layers => Zone::Right,
            Self::Properties => Zone::Right,
            Self::ExternalRefs => Zone::Right,
            Self::Symbols => Zone::Right,
        }
    }
    /// Palettes outside the default arrangement, opened on request (QUICKCALC).
    pub const ON_DEMAND: &[Self] = &[Self::QuickCalc];
    pub fn parse(id: &str) -> Result<Self, String> {
        Self::ALL.iter().chain(Self::ON_DEMAND).copied().find(|panel| panel.id() == id).ok_or_else(|| format!("Unknown panel: {id}"))
    }
}

fn permissions(panel: &Panel) -> Permissions {
    if *panel == Panel::Canvas { Permissions::PROTECTED } else { Permissions::default() }
}

fn tabs(panel: Panel) -> Node<Panel> {
    Node::Tabs { panels: vec![panel], active: 0 }
}

fn split(axis: SplitAxis, size: SplitSize, first: Node<Panel>, second: Node<Panel>) -> Node<Panel> {
    Node::Split { axis, size, first: Box::new(first), second: Box::new(second) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub layout: Layout<Panel>,
    /// Closed panels retain their most recent group, side, size or floating rectangle.
    pub hidden: Vec<(Panel, Location<Panel>)>,
    /// Palettes in windows of their own (another screen, say), outside the main window.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<Detached>,
}

/// A palette in its own OS window: where that window is (screen points: left, top, width,
/// height) and the size of the screen it was on, so it reopens there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detached {
    pub panel: Panel,
    pub rect: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<[f32; 2]>,
}

/// Largest coordinate or size a detached window takes (hostile saved settings).
const MAX_COORD: f32 = 100_000.0;

/// A window rectangle that can be used: finite, within bounds, at least a small window.
fn usable(rect: &[f32; 4]) -> bool {
    rect.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) && rect[2] >= 120.0 && rect[3] >= 80.0
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            layout: Layout {
                root: Some(split(
                    SplitAxis::Horizontal,
                    SplitSize::FixedFirst(220.0),
                    tabs(Panel::ToolSets),
                    split(
                        SplitAxis::Horizontal,
                        SplitSize::FixedSecond(300.0),
                        tabs(Panel::Canvas),
                        Node::Stack {
                            entries: vec![
                                // Properties takes the room left: Layers and Symbols get modest fixed
                                // heights (users resize them; a saved layout keeps their sizes).
                                craft_ui::docking::StackEntry { panel: Panel::Layers, open: true, height: Some(LAYERS_HEIGHT) },
                                craft_ui::docking::StackEntry { panel: Panel::Properties, open: true, height: Some(PROPERTIES_HEIGHT) },
                                craft_ui::docking::StackEntry { panel: Panel::Symbols, open: false, height: Some(240.0) },
                            ],
                        },
                    ),
                )),
                floating: Vec::new(),
            },
            hidden: Vec::new(),
            detached: Vec::new(),
        }
    }
}

impl Workspace {
    pub fn validate(&self) -> Result<(), String> {
        self.layout.validate().map_err(|error| error.to_string())?;
        let mut own = std::collections::HashSet::new();
        for d in &self.detached {
            if d.panel == Panel::Canvas || self.layout.contains(&d.panel) || !own.insert(d.panel) || !usable(&d.rect) {
                return Err("A palette window has an invalid panel or rectangle".into());
            }
        }
        if !self.layout.contains(&Panel::Canvas) || self.hidden.len() > Panel::ALL.len() + Panel::ON_DEMAND.len() {
            return Err("The panel layout must contain its document view and bounded hidden panels".into());
        }
        // Saved settings cannot turn the protected document view into a floating or closable tab.
        if self.layout.floating.iter().any(|group| group.panels.contains(&Panel::Canvas)) {
            return Err("The document view cannot float".into());
        }
        let mut nodes: Vec<_> = self.layout.root.iter().collect();
        while let Some(node) = nodes.pop() {
            match node {
                Node::Split { first, second, .. } => {
                    nodes.push(first);
                    nodes.push(second);
                }
                Node::Tabs { panels, .. } if panels.contains(&Panel::Canvas) && panels.len() != 1 => {
                    return Err("The document view cannot share a tab group".into());
                }
                Node::Stack { entries } if entries.iter().any(|entry| entry.panel == Panel::Canvas) => {
                    return Err("The document view cannot be collapsed".into());
                }
                _ => {}
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (panel, location) in &self.hidden {
            if *panel == Panel::Canvas || self.layout.contains(panel) || !seen.insert(*panel) {
                return Err("A hidden panel has an invalid or duplicate identity".into());
            }
            if location.anchor == Some(Panel::Canvas) && matches!(location.placement, Placement::Tab { .. } | Placement::Split(Zone::Center)) {
                return Err("A hidden panel cannot restore over the document view".into());
            }
            let mut probe = self.layout.clone();
            if let Err(error) = probe.restore(*panel, location)
                && error != craft_ui::docking::DockError::MissingTarget
            {
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    pub fn apply(&mut self, action: Action<Panel>) -> Result<(), String> {
        self.validate()?;
        let mut next = self.clone();
        if let Action::Close { panel } = &action {
            let location = next.layout.location(panel).map_err(|error| error.to_string())?;
            next.hidden.retain(|(id, _)| id != panel);
            next.hidden.push((*panel, location));
        } else if let Action::Open { panel, .. } | Action::OpenAt { panel, .. } = &action {
            next.hidden.retain(|(id, _)| id != panel);
        }
        next.layout.apply_with_permissions(action, permissions).map_err(|error| error.to_string())?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Explicit opens select the requested tab and expand a collapsed stack entry.
    pub fn reveal_panel(&mut self, panel: Panel) -> Result<(), String> {
        let mut next = self.clone();
        next.set_visible(panel, true)?;
        next.apply(Action::Activate { panel })?;
        *self = next;
        Ok(())
    }

    pub fn set_visible(&mut self, panel: Panel, visible: bool) -> Result<(), String> {
        if self.layout.contains(&panel) == visible {
            return Ok(());
        }
        if !visible {
            return self.apply(Action::Close { panel });
        }
        self.validate()?;
        let mut next = self.clone();
        let location = next.hidden.iter().find(|(id, _)| *id == panel).map(|(_, location)| location.clone());
        // Recreate the last surviving member first. Restoring a member also restores any
        // hidden anchor it needs, so either close order retains the original tab order.
        // Remove this member before recursion to bound malformed/cyclic saved anchors.
        next.hidden.retain(|(id, _)| *id != panel);
        if let Some(anchor) = location.as_ref().and_then(|saved| saved.anchor)
            && !next.layout.contains(&anchor)
            && next.hidden.iter().any(|(id, _)| *id == anchor)
        {
            next.set_visible(anchor, true)?;
        }
        let restored = location.as_ref().is_some_and(|location| next.layout.restore(panel, location).is_ok());
        if !restored {
            next.layout
                .apply_with_permissions(
                    Action::OpenAt { panel, anchor: Panel::Canvas, placement: Placement::Split(panel.default_side()) },
                    permissions,
                )
                .map_err(|error| error.to_string())?;
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn command(&mut self, params: &Value) -> Result<(), String> {
        self.validate()?;
        if let Some(action) = params.get("action") {
            let action: Action<Panel> = serde_json::from_value(action.clone()).map_err(|error| error.to_string())?;
            return self.apply(action);
        }
        let operation = params.get("operation").and_then(Value::as_str).ok_or("operation is required")?;
        if matches!(operation, "detach" | "dock" | "moveDetached") {
            let panel = Panel::parse(params.get("panel").and_then(Value::as_str).ok_or("panel is required")?)?;
            let rect = match params.get("rect") {
                None | Some(Value::Null) => None,
                Some(r) => Some(rect_param(r)?),
            };
            return match operation {
                // 380 wide: Properties' header row (selector, quick-select buttons) was cut off at 320
                // (UI review 0.7.7c/d).
                "detach" => self.detach(panel, rect.unwrap_or([120.0, 120.0, 380.0, 560.0]), None),
                "dock" => self.dock(panel),
                _ => self.move_detached(panel, rect.ok_or("rect is required")?, None),
            };
        }
        if operation == "resizeSplit" {
            let path: Vec<bool> =
                serde_json::from_value(params.get("path").cloned().ok_or("path is required")?).map_err(|error| error.to_string())?;
            let size: SplitSize =
                serde_json::from_value(params.get("size").cloned().ok_or("size is required")?).map_err(|error| error.to_string())?;
            return self.apply(Action::ResizeSplit { path, size });
        }
        let panel = Panel::parse(params.get("panel").and_then(Value::as_str).ok_or("panel is required")?)?;
        if self.is_detached(panel) {
            // Its window is already open; closing it keeps its place in the layout for later.
            return match operation {
                "open" | "activate" => Ok(()),
                "close" => {
                    self.detached.retain(|d| d.panel != panel);
                    Ok(())
                }
                _ => Err(format!("{} is in a window of its own; dock it first", panel.id())),
            };
        }
        let action = match operation {
            "open" => return self.reveal_panel(panel),
            "close" => Action::Close { panel },
            "activate" => Action::Activate { panel },
            "setStackOpen" => Action::SetStackOpen { panel, open: params.get("open").and_then(Value::as_bool).ok_or("open must be a boolean")? },
            "resizeStack" => {
                let value = params.get("height").ok_or("height is required")?;
                let height = if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_f64()
                            .filter(|height| height.is_finite() && (0.0..=1_000_000.0).contains(height))
                            .ok_or("height must be finite and bounded")? as f32,
                    )
                };
                Action::ResizeStack { panel, height }
            }
            "float" | "moveFloating" => {
                let rect = params.get("rect").and_then(Value::as_array).filter(|rect| rect.len() == 4).ok_or("rect must be [x,y,width,height]")?;
                let mut values = [0.0f32; 4];
                for (output, value) in values.iter_mut().zip(rect) {
                    *output = value
                        .as_f64()
                        .filter(|number| number.is_finite() && number.abs() <= 1_000_000.0)
                        .ok_or("rect values must be finite and bounded")? as f32;
                }
                if operation == "float" { Action::Float { panel, rect: values } } else { Action::MoveFloating { panel, rect: values } }
            }
            "move" => {
                let anchor = Panel::parse(params.get("target").and_then(Value::as_str).ok_or("target is required")?)?;
                let placement = match params.get("zone").and_then(Value::as_str).unwrap_or("center") {
                    "center" => Placement::Split(Zone::Center),
                    "left" => Placement::Split(Zone::Left),
                    "right" => Placement::Split(Zone::Right),
                    "top" => Placement::Split(Zone::Top),
                    "bottom" => Placement::Split(Zone::Bottom),
                    _ => return Err("zone must be center, left, right, top or bottom".into()),
                };
                let placement = if let Some(before) = params.get("before") {
                    if !matches!(placement, Placement::Split(Zone::Center)) {
                        return Err("before only applies to a center move".into());
                    }
                    Placement::Tab {
                        before: if before.is_null() {
                            None
                        } else {
                            Some(Panel::parse(before.as_str().ok_or("before must be a panel ID or null")?)?)
                        },
                    }
                } else {
                    placement
                };
                Action::Move { panel, anchor, placement }
            }
            _ => return Err("operation must be open, close, activate, float, move, moveFloating, resizeSplit, setStackOpen, resizeStack, detach, dock or moveDetached".into()),
        };
        self.apply(action)
    }
}

impl Workspace {
    /// Whether `panel` is in a window of its own.
    pub fn is_detached(&self, panel: Panel) -> bool {
        self.detached.iter().any(|d| d.panel == panel)
    }

    /// Move `panel` out of the main window into its own window at `rect`; its place in the
    /// layout is kept for docking back.
    pub fn detach(&mut self, panel: Panel, rect: [f32; 4], monitor: Option<[f32; 2]>) -> Result<(), String> {
        if panel == Panel::Canvas {
            return Err("The document view cannot leave the main window".into());
        }
        if !usable(&rect) {
            return Err("rect must be finite, bounded and at least 120 × 80".into());
        }
        if self.is_detached(panel) {
            return self.move_detached(panel, rect, monitor);
        }
        let mut next = self.clone();
        if next.layout.contains(&panel) {
            next.apply(Action::Close { panel })?;
        }
        next.detached.push(Detached { panel, rect, monitor });
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Bring `panel` back into the main window, where it was before it was detached.
    pub fn dock(&mut self, panel: Panel) -> Result<(), String> {
        if !self.is_detached(panel) {
            return Err(format!("{} is not in a window of its own", panel.id()));
        }
        let mut next = self.clone();
        next.detached.retain(|d| d.panel != panel);
        next.reveal_panel(panel)?;
        *self = next;
        Ok(())
    }

    /// Remember where `panel`'s window is now (and on which screen).
    pub fn move_detached(&mut self, panel: Panel, rect: [f32; 4], monitor: Option<[f32; 2]>) -> Result<(), String> {
        if !usable(&rect) {
            return Err("rect must be finite, bounded and at least 120 × 80".into());
        }
        let d = self.detached.iter_mut().find(|d| d.panel == panel).ok_or_else(|| format!("{} is not in a window of its own", panel.id()))?;
        d.rect = rect;
        if monitor.is_some() {
            d.monitor = monitor.filter(|m| m.iter().all(|v| v.is_finite() && *v > 0.0 && *v <= MAX_COORD));
        }
        Ok(())
    }
}

fn rect_param(v: &Value) -> Result<[f32; 4], String> {
    let a = v.as_array().filter(|r| r.len() == 4).ok_or("rect must be [x,y,width,height]")?;
    let mut out = [0.0f32; 4];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64().filter(|n| n.is_finite() && n.abs() <= f64::from(MAX_COORD)).ok_or("rect values must be finite and bounded")? as f32;
    }
    Ok(out)
}

fn sync(workspace: &mut Workspace, app: &crate::CadApp) -> Result<(), String> {
    // A palette in its own window stays there while its switch is on; turned off, its window closes.
    if !workspace.is_detached(Panel::ToolSets) {
        workspace.set_visible(Panel::ToolSets, app.ui.show_toolsets)?;
    } else if !app.ui.show_toolsets {
        workspace.detached.retain(|d| d.panel != Panel::ToolSets);
    }
    if workspace.is_detached(Panel::Layers) || workspace.is_detached(Panel::Properties) {
        if !app.ui.show_palettes {
            workspace.detached.retain(|d| !matches!(d.panel, Panel::Layers | Panel::Properties));
            workspace.set_visible(Panel::Layers, false)?;
            workspace.set_visible(Panel::Properties, false)?;
        }
        return Ok(());
    }
    if !app.ui.show_palettes {
        workspace.set_visible(Panel::Layers, false)?;
        workspace.set_visible(Panel::Properties, false)?;
    } else if !workspace.layout.contains(&Panel::Layers) && !workspace.layout.contains(&Panel::Properties) {
        // Reopen the last surviving accordion entry before restoring its earlier sibling.
        workspace.set_visible(Panel::Properties, true)?;
        workspace.set_visible(Panel::Layers, true)?;
    }
    Ok(())
}

fn publish_visibility(app: &mut crate::CadApp, workspace: &Workspace) {
    let shown = |p: Panel| workspace.layout.contains(&p) || workspace.is_detached(p);
    app.ui.show_toolsets = shown(Panel::ToolSets);
    app.ui.show_palettes = shown(Panel::Layers) || shown(Panel::Properties);
}

pub fn command(app: &mut crate::CadApp, reset: bool, params: &Value) -> Result<Value, String> {
    let mut workspace = app.ui.docking.clone();
    if reset {
        workspace = Workspace::default();
    } else {
        sync(&mut workspace, app)?;
        workspace.command(params)?;
    }
    publish_visibility(app, &workspace);
    app.ui.docking = workspace;
    Ok(json!({"docking": app.ui.docking}))
}

pub fn show(app: &mut crate::CadApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get();
    let mut workspace = app.ui.docking.clone();
    if let Err(error) = workspace.validate().and_then(|()| sync(&mut workspace, app)) {
        app.set_status(format!("Panel layout could not be restored: {error}"));
        workspace = Workspace::default();
        let _ = sync(&mut workspace, app);
    }
    if let Some(root) = workspace.layout.root.as_mut() {
        let column = ui.available_height();
        keep_properties_room(root, below_properties(column), column);
    }
    // Bodies can dispatch commands (including the drawing's typed command line).
    // Keep the canonical workspace accessible while rendering an immutable snapshot.
    app.ui.docking = workspace.clone();
    let mut style = DockStyle::from_ui(ui);
    style.min_pane = 96.0;
    style.background = t.panel;
    style.tab_background = t.chrome;
    style.active_background = t.tab_active;
    style.text = t.text;
    style.inactive_text = t.text_dim;
    style.border = egui::Stroke::new(1.0, t.border);
    style.accent = t.accent;
    style.font = crate::theme::body();
    let output = DockArea::new(egui::Id::new("cadaddy-docking")).show_with_limits(
        ui,
        &workspace.layout,
        &style,
        |panel| crate::i18n::t(panel.label()).into(),
        permissions,
        panel_limits,
        |ui, panel| panel_body(app, ui, *panel),
    );
    // Palettes in windows of their own.
    show_detached(app, ui.ctx(), &workspace);
    // Existing panel close buttons and commands still own their visibility switches.
    finish(app, ui, workspace, output.actions);
}

/// A panel's contents, in the dock area or a palette window.
fn panel_body(app: &mut crate::CadApp, ui: &mut egui::Ui, panel: Panel) {
    let t = crate::theme::Tokens::get();
    match panel {
        Panel::Canvas => {
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.canvas);
            // Walk mode (3DWALK, 3DFLY, View ▸ Walk) takes the drawing's place in its tab.
            if app.walk.is_some() {
                crate::walk::show(app, ui);
            } else {
                crate::canvas::show(app, ui);
            }
        }
        Panel::ToolSets => crate::palettes::toolsets(app, ui),
        Panel::QuickCalc => crate::palettes::quickcalc::show(app, ui),
        Panel::Layers => {
            egui::ScrollArea::vertical().id_salt("dock-layers").auto_shrink([false, false]).show(ui, |ui| crate::palettes::layers_section(app, ui));
        }
        Panel::Properties => {
            egui::ScrollArea::vertical().id_salt("dock-properties").auto_shrink([false, false]).show(ui, |ui| {
                crate::shortcut_menus::palette(app, ui, crate::palettes::properties_section, crate::shortcut_menus::properties_entries)
            });
        }
        Panel::ExternalRefs => {
            egui::ScrollArea::both().id_salt("dock-xrefs").auto_shrink([false, false]).show(ui, |ui| crate::xrefs::section(app, ui));
        }
        Panel::Symbols => {
            egui::ScrollArea::vertical()
                .id_salt("dock-symbols")
                .auto_shrink([false, false])
                .show(ui, |ui| crate::palettes::symbols::symbols_section(app, ui));
        }
    }
}

/// Each detached palette in an OS window of its own (an egui viewport; on the web, where there
/// is only one window, it shows inside it). Its window's place and screen are remembered; closing
/// it or "Dock" brings the palette back into the main window.
fn show_detached(app: &mut crate::CadApp, ctx: &egui::Context, workspace: &Workspace) {
    for d in workspace.detached.clone() {
        let panel = d.panel;
        let id = egui::ViewportId::from_hash_of(("cadaddy-palette", panel.id()));
        let title = crate::i18n::t(panel.label()).to_string();
        let builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_position(egui::pos2(d.rect[0], d.rect[1]))
            .with_inner_size(egui::vec2(d.rect[2], d.rect[3]))
            .with_min_inner_size(egui::vec2(160.0, 120.0));
        let mut dock = false;
        let mut moved: Option<([f32; 4], Option<[f32; 2]>)> = None;
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            let mut body = |ui: &mut egui::Ui| {
                ui.horizontal(|ui| {
                    // The window's title bar names the palette; only the way back goes here.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        dock = ui.small_button(crate::tl!("Dock")).on_hover_text(crate::tl!("Back into the main window")).clicked();
                    });
                });
                ui.separator();
                panel_body(app, ui, panel);
            };
            if class == egui::ViewportClass::EmbeddedWindow {
                // No windows of its own here (the web): egui already put it in a window.
                body(ui);
                return;
            }
            // Content wider than the window scrolls instead of being cut off at its edge.
            egui::CentralPanel::default()
                .show(ui, |ui| egui::ScrollArea::horizontal().id_salt("detached-palette").auto_shrink([false, false]).show(ui, |ui| body(ui)));
            let info = ui.ctx().input(|i| i.viewport().clone());
            if info.close_requested() {
                dock = true;
            }
            if let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) {
                let r = [outer.min.x, outer.min.y, inner.width(), inner.height()];
                if r.iter().zip(d.rect.iter()).any(|(a, b)| (a - b).abs() > 0.5) {
                    moved = Some((r, info.monitor_size.map(|m| [m.x, m.y])));
                }
            }
        });
        let mut ws = app.ui.docking.clone();
        let result = if dock {
            ws.dock(panel)
        } else if let Some((r, m)) = moved {
            ws.move_detached(panel, r, m)
        } else {
            Ok(())
        };
        match result {
            Ok(()) => {
                publish_visibility(app, &ws);
                app.ui.docking = ws;
            }
            Err(e) => app.set_status(format!("Palette window: {e}")),
        }
    }
}

/// After the dock area drew: keep commands' visibility changes and apply the area's moves.
fn finish(app: &mut crate::CadApp, ui: &mut egui::Ui, workspace: Workspace, actions: Vec<Action<Panel>>) {
    let rendered = workspace;
    let mut workspace = app.ui.docking.clone();
    let _ = sync(&mut workspace, app);
    let stale = workspace.layout != rendered.layout || workspace.hidden != rendered.hidden;
    if stale {
        craft_ui::docking::cancel_drag::<Panel>(ui.ctx(), egui::Id::new("cadaddy-docking"));
    }
    publish_visibility(app, &workspace);
    app.ui.docking = workspace;
    if stale {
        return;
    }
    for action in actions {
        if let Err(error) =
            serde_json::to_value(action).map_err(|error| error.to_string()).and_then(|action| command(app, false, &json!({"action": action})))
        {
            app.set_status(format!("Panel move was not applied: {error}"));
        }
    }
}

/// The Layers palette's height in the right-hand stack: its list starts collapsed, and the 190
/// points it had were mostly empty while Properties got no rows on a small window (UI review
/// 0.7.7d).
const LAYERS_HEIGHT: f32 = 130.0;
/// Properties' share of the stack. The docking area splits a stack's height in proportion to its
/// entries' heights, an entry without one counting as its minimum (96): Layers' 130 then took
/// over half and Properties showed one row on a small window (tester 0.7.7e).
const PROPERTIES_HEIGHT: f32 = 600.0;
/// Layers' height before [`LAYERS_HEIGHT`]: a saved layout still at it moves to the new one.
const OLD_LAYERS_HEIGHT: f32 = 190.0;

/// Height External References or Symbols takes below the Properties palette in a column
/// `column` points high: 30 %, from 80 to 240 points.
fn below_properties(column: f32) -> f32 {
    (column * 0.25).clamp(80.0, 240.0)
}

/// Least height Layers keeps in the stack: its icon row and current-layer list (they were cut
/// in half at 71 points on a small window; tester 0.7.7f).
const LAYERS_MIN: f32 = 120.0;

/// Properties' weight in a stack `stack` points high so Layers ([`LAYERS_HEIGHT`] weight) keeps
/// [`LAYERS_MIN`]: the stack is shared in proportion to the weights.
fn properties_weight(stack: f32) -> f32 {
    if stack <= LAYERS_MIN * 2.0 {
        return LAYERS_HEIGHT;
    }
    (LAYERS_HEIGHT * (stack / LAYERS_MIN - 1.0)).clamp(LAYERS_HEIGHT, PROPERTIES_HEIGHT)
}

/// The weight [`keep_properties_room`] last gave Properties: it follows the window's size, a
/// height the user dragged to is kept.
static AUTO_PROPERTIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The height [`keep_properties_room`] last gave the panel below Properties: it follows the
/// window's size, while a height the user dragged to is kept.
static AUTO_BELOW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A panel of External References or Symbols opened below the Layers/Properties stack splits the
/// column half and half, and the docking area doesn't keep a stack's inner minimums: Properties
/// shrank to one row (UI review 0.7.7c). Such a split gets a fixed height for the panel below
/// instead, so Properties keeps the rest; a split the user resized (fixed already) is left alone.
fn keep_properties_room(node: &mut Node<Panel>, below: f32, column: f32) {
    fn has(node: &Node<Panel>, f: &dyn Fn(&Panel) -> bool) -> bool {
        match node {
            Node::Split { first, second, .. } => has(first, f) || has(second, f),
            Node::Tabs { panels, .. } => panels.iter().any(f),
            Node::Stack { entries } => entries.iter().any(|e| f(&e.panel)),
        }
    }
    if let Node::Split { axis, size, first, second } = node {
        let small = |p: &Panel| matches!(p, Panel::ExternalRefs | Panel::Symbols);
        let auto = f32::from_bits(AUTO_BELOW.load(std::sync::atomic::Ordering::Relaxed));
        let ours = match size {
            SplitSize::Ratio(_) => true,
            SplitSize::FixedSecond(h) => (*h - auto).abs() < 0.5,
            SplitSize::FixedFirst(_) => false,
        };
        if *axis == SplitAxis::Vertical && ours && has(first, &|p| *p == Panel::Properties) && has(second, &small) && !has(second, &|p| !small(p)) {
            *size = SplitSize::FixedSecond(below);
            AUTO_BELOW.store(below.to_bits(), std::sync::atomic::Ordering::Relaxed);
        }
        // Below Properties' stack there is the other panel: the stack gets the rest.
        let stack = if *axis == SplitAxis::Vertical && has(second, &small) { column - below } else { column };
        keep_properties_room(first, below, stack);
        keep_properties_room(second, below, column);
    }
    if let Node::Stack { entries } = node {
        for e in entries.iter_mut() {
            match e.panel {
                Panel::Layers if e.height.is_some_and(|h| (h - OLD_LAYERS_HEIGHT).abs() < 0.5) => e.height = Some(LAYERS_HEIGHT),
                Panel::Properties => {
                    // Its share follows the stack's height unless the user set one.
                    let auto = f32::from_bits(AUTO_PROPERTIES.load(std::sync::atomic::Ordering::Relaxed));
                    let ours = e.height.is_none_or(|h| (h - PROPERTIES_HEIGHT).abs() < 0.5 || (h - auto).abs() < 0.5);
                    if ours {
                        let w = properties_weight(column);
                        e.height = Some(w);
                        AUTO_PROPERTIES.store(w.to_bits(), std::sync::atomic::Ordering::Relaxed);
                    }
                }
                _ => {}
            }
        }
    }
}

fn panel_limits(panel: &Panel) -> PanelLimits {
    let (min, max_width) = match panel {
        Panel::Canvas => (egui::vec2(300.0, 180.0), 1_000_000.0),
        Panel::QuickCalc => (egui::vec2(240.0, 160.0), 600.0),
        Panel::ToolSets => (egui::vec2(220.0, 150.0), 480.0),
        // Properties is the palette used most: it keeps room for its fields (it shrank to one
        // row beside an open External References panel). External References and Symbols need
        // less.
        Panel::Properties => (egui::vec2(220.0, 220.0), 720.0),
        Panel::Layers => (egui::vec2(220.0, 120.0), 720.0),
        Panel::ExternalRefs | Panel::Symbols => (egui::vec2(220.0, 80.0), 720.0),
    };
    PanelLimits { min, max: egui::vec2(max_width, 1_000_000.0) }
}
