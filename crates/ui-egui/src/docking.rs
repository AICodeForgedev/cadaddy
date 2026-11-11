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
    /// The building spec and the model built from it (PLANSTOMODEL).
    PlansToModel,
    /// The Sheet Set Manager (SHEETSET).
    SheetSets,
    /// The BIM coordination palette (COORDINATION).
    Coordination,
    /// The Validate panel (Automation ▸ Validate): the model checked against its prints.
    Validate,
    /// The Job panel (Automation ▸ Job): a guided job from the prints, its progress, questions,
    /// findings and report.
    Job,
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
            Self::PlansToModel => "plansToModel",
            Self::SheetSets => "sheetSets",
            Self::Coordination => "coordination",
            Self::Validate => "validate",
            Self::Job => "job",
        }
    }
    /// The palette's name: its tab title and its rail button's tooltip (English; translated where shown).
    pub fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Drawing",
            Self::ToolSets => "Tool Sets",
            Self::QuickCalc => "QuickCalc",
            Self::Layers => "Layers",
            Self::Properties => "Properties",
            Self::ExternalRefs => "External References",
            Self::Symbols => "Symbols",
            Self::PlansToModel => "Plans to Model",
            Self::SheetSets => "Sheet Set Manager",
            Self::Coordination => "Coordination",
            Self::Validate => "Validate",
            Self::Job => "Job",
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
            Self::PlansToModel => Zone::Right,
            Self::SheetSets => Zone::Right,
            Self::Coordination => Zone::Right,
            Self::Validate => Zone::Right,
            Self::Job => Zone::Right,
        }
    }
    /// The palettes on the icon rail down the window's edge, top to bottom: the ones used most
    /// first. Every palette but the drawing itself.
    pub const RAIL: &[Self] = &[
        Self::Properties,
        Self::Layers,
        Self::ToolSets,
        Self::Symbols,
        Self::ExternalRefs,
        Self::Coordination,
        Self::SheetSets,
        Self::PlansToModel,
        Self::Validate,
        Self::Job,
        Self::QuickCalc,
    ];
    /// The width a palette opened from the rail takes beside it, points (the user's own width is
    /// kept once they drag it).
    fn rail_width(self) -> f32 {
        match self {
            Self::ToolSets => 220.0,
            Self::Coordination | Self::PlansToModel => 340.0,
            Self::Validate | Self::Job => 420.0,
            _ => 300.0,
        }
    }
    /// Palettes outside the default arrangement, opened on request (QUICKCALC).
    pub const ON_DEMAND: &[Self] = &[Self::QuickCalc, Self::PlansToModel, Self::SheetSets, Self::Coordination, Self::Validate, Self::Job];
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

/// `n` mirrored left to right: every side-by-side split swaps its sides and keeps each side's size.
fn mirror(n: Node<Panel>) -> Node<Panel> {
    match n {
        Node::Split { axis: SplitAxis::Horizontal, size, first, second } => {
            Node::Split { axis: SplitAxis::Horizontal, size: mirror_size(size), first: Box::new(mirror(*second)), second: Box::new(mirror(*first)) }
        }
        Node::Split { axis, size, first, second } => Node::Split { axis, size, first: Box::new(mirror(*first)), second: Box::new(mirror(*second)) },
        other => other,
    }
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
    /// Palettes the user pinned: they stay docked where they are when another palette opens
    /// from the rail. One that isn't pinned closes when another one opens.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<Panel>,
    /// The arrangement the layout was saved with: [`RAIL_LAYOUT`] since the icon rail; missing
    /// (0) in settings written before it, when every palette stood docked ([`upgrade`]).
    #[serde(default = "before_rail")]
    pub rail: u32,
    /// The layout is mirrored for a right-to-left interface: the rail and the palettes it opens
    /// are on the right.
    #[serde(skip_serializing_if = "is_false")]
    pub mirrored: bool,
}

/// [`Workspace::rail`] of a layout made for the icon rail.
pub const RAIL_LAYOUT: u32 = 1;

/// A saved layout without [`Workspace::rail`] dates from before the icon rail.
fn before_rail() -> u32 {
    0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Where a palette opened from the rail goes: beside the rail, at the drawing's edge, at its own
/// width (on the right in a right-to-left interface, where the rail is).
fn rail_location(panel: Panel, mirrored: bool) -> Location<Panel> {
    let w = panel.rail_width();
    let (zone, size) = if mirrored { (Zone::Right, SplitSize::FixedSecond(w)) } else { (Zone::Left, SplitSize::FixedFirst(w)) };
    Location { floating: None, anchor: Some(Panel::Canvas), placement: Placement::Split(zone), size: Some(size), stack: None, sibling: Vec::new() }
}

fn mirror_size(size: SplitSize) -> SplitSize {
    match size {
        SplitSize::Ratio(r) => SplitSize::Ratio(1.0 - r),
        SplitSize::FixedFirst(v) => SplitSize::FixedSecond(v),
        SplitSize::FixedSecond(v) => SplitSize::FixedFirst(v),
    }
}

/// A closed palette's place mirrored left to right: beside something on its left goes to its right.
fn mirror_location(location: &mut Location<Panel>) {
    if let Placement::Split(zone @ (Zone::Left | Zone::Right)) = location.placement {
        location.placement = Placement::Split(if zone == Zone::Left { Zone::Right } else { Zone::Left });
        location.size = location.size.map(mirror_size);
    }
}

/// Stack entries' heights and open states cleared: two layouts that differ only in those have
/// the same arrangement.
fn clear_stacks(node: &mut Node<Panel>) {
    match node {
        Node::Split { first, second, .. } => {
            clear_stacks(first);
            clear_stacks(second);
        }
        Node::Stack { entries } => {
            for e in entries {
                e.height = None;
                e.open = true;
            }
        }
        Node::Tabs { .. } => {}
    }
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

/// The default: the drawing fills the window, every palette is closed and waits on the rail
/// beside it.
impl Default for Workspace {
    fn default() -> Self {
        Self {
            layout: Layout { root: Some(tabs(Panel::Canvas)), floating: Vec::new() },
            hidden: Panel::RAIL.iter().map(|p| (*p, rail_location(*p, false))).collect(),
            detached: Vec::new(),
            pinned: Vec::new(),
            rail: RAIL_LAYOUT,
            mirrored: false,
        }
    }
}

impl Workspace {
    /// The default before the icon rail: Tool Sets on the left, the Layers, Properties and
    /// Symbols column standing open on the right. Only to recognise a saved layout nobody changed.
    fn classic() -> Self {
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
            pinned: Vec::new(),
            rail: before_rail(),
            mirrored: false,
        }
    }

    /// The arrangement from before the icon rail, made with the rail: Tool Sets on the left, the
    /// Layers, Properties and Symbols column on the right, all pinned so they stay; the other
    /// palettes wait on the rail. For users (and tests) who work with docked palettes.
    pub fn docked() -> Self {
        let mut w = Self::classic();
        w.rail = RAIL_LAYOUT;
        w.pinned = vec![Panel::ToolSets, Panel::Layers, Panel::Properties, Panel::Symbols];
        w.hidden = Panel::RAIL.iter().filter(|p| !w.layout.contains(p)).map(|p| (*p, rail_location(*p, false))).collect();
        w
    }

    /// The layout is the pre-rail default as it came (Tool Sets shown or not, sizes the app set
    /// itself): nothing the user arranged would be lost by the rail's default.
    fn is_classic(&self) -> bool {
        if !self.layout.floating.is_empty() || !self.detached.is_empty() {
            return false;
        }
        let arrangement = |layout: &Layout<Panel>| -> Option<Node<Panel>> {
            let mut layout = layout.clone();
            if layout.contains(&Panel::ToolSets) {
                layout.apply(Action::Close { panel: Panel::ToolSets }).ok()?;
            }
            let mut root = layout.root?;
            clear_stacks(&mut root);
            Some(root)
        };
        let Some(mine) = arrangement(&self.layout) else { return false };
        let classic = Self::classic().layout;
        let mirrored = Layout { root: classic.root.clone().map(mirror), floating: Vec::new() };
        arrangement(&classic).is_some_and(|c| c == mine) || arrangement(&mirrored).is_some_and(|c| c == mine)
    }

    /// Whether `panel` can be seen now: docked as the selected tab or an expanded entry, floating
    /// as its group's selected tab, or in a window of its own.
    pub fn showing(&self, panel: Panel) -> bool {
        fn shown(node: &Node<Panel>, panel: Panel) -> bool {
            match node {
                Node::Split { first, second, .. } => shown(first, panel) || shown(second, panel),
                Node::Tabs { panels, active } => panels.get(*active) == Some(&panel),
                Node::Stack { entries } => entries.iter().any(|e| e.panel == panel && e.open),
            }
        }
        self.is_detached(panel)
            || self.layout.root.as_ref().is_some_and(|root| shown(root, panel))
            || self.layout.floating.iter().any(|g| g.panels.get(g.active) == Some(&panel))
    }

    /// Show `panel` the rail's way: where it was last (beside the rail the first time), selected;
    /// every other palette that isn't pinned closes, so one stands open unless the user pinned
    /// more. Palettes in windows of their own stay.
    pub fn open_one(&mut self, panel: Panel) -> Result<(), String> {
        if panel == Panel::Canvas {
            return Err("The drawing is not a palette".into());
        }
        let mut next = self.clone();
        next.reveal_panel(panel)?;
        let loose: Vec<Panel> =
            next.layout.panels().into_iter().copied().filter(|p| *p != panel && *p != Panel::Canvas && !next.pinned.contains(p)).collect();
        for p in loose {
            next.apply(Action::Close { panel: p })?;
        }
        *self = next;
        Ok(())
    }

    /// A rail button: a palette that shows closes (its window, if it has one of its own); one
    /// that doesn't opens the rail's way ([`Self::open_one`]); one behind another tab or folded
    /// in its stack comes to the front.
    pub fn toggle(&mut self, panel: Panel) -> Result<(), String> {
        if panel == Panel::Canvas {
            return Err("The drawing is not a palette".into());
        }
        if self.is_detached(panel) {
            self.detached.retain(|d| d.panel != panel);
            return Ok(());
        }
        if self.showing(panel) {
            self.apply(Action::Close { panel })
        } else if self.layout.contains(&panel) {
            self.reveal_panel(panel)
        } else {
            self.open_one(panel)
        }
    }

    /// Pin `panel` (it stays docked when another palette opens) or unpin it.
    pub fn pin(&mut self, panel: Panel, pinned: bool) -> Result<(), String> {
        if panel == Panel::Canvas {
            return Err("The drawing is not a palette".into());
        }
        self.pinned.retain(|p| *p != panel);
        if pinned {
            self.pinned.push(panel);
        }
        Ok(())
    }
}

impl Workspace {
    /// The layout mirrored for a right-to-left interface (and back): palettes docked on the right
    /// go to the left and the other way round, with their sizes.
    pub fn mirrored(&self) -> Workspace {
        let mut w = self.clone();
        w.layout.root = w.layout.root.take().map(mirror);
        for (_, location) in &mut w.hidden {
            mirror_location(location);
        }
        w.mirrored = !w.mirrored;
        w
    }

    pub fn validate(&self) -> Result<(), String> {
        self.layout.validate().map_err(|error| error.to_string())?;
        let mut own = std::collections::HashSet::new();
        for d in &self.detached {
            if d.panel == Panel::Canvas || self.layout.contains(&d.panel) || !own.insert(d.panel) || !usable(&d.rect) {
                return Err("A palette window has an invalid panel or rectangle".into());
            }
        }
        let mut pins = std::collections::HashSet::new();
        if self.pinned.iter().any(|p| *p == Panel::Canvas || !pins.insert(*p)) {
            return Err("A pinned palette has an invalid or duplicate identity".into());
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
        // Never placed before (a layout saved before the palette existed): beside the rail.
        let restored = location.as_ref().is_some_and(|location| next.layout.restore(panel, location).is_ok())
            || next.layout.restore(panel, &rail_location(panel, next.mirrored)).is_ok();
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
        match operation {
            "toggle" => return self.toggle(panel),
            "pin" => {
                let pinned = match params.get("pinned") {
                    None | Some(Value::Null) => !self.pinned.contains(&panel),
                    Some(v) => v.as_bool().ok_or("pinned must be a boolean")?,
                };
                return self.pin(panel, pinned);
            }
            _ => {}
        }
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
            "open" => return self.open_one(panel),
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
            _ => return Err("operation must be open, close, toggle, pin, activate, float, move, moveFloating, resizeSplit, setStackOpen, resizeStack, detach, dock or moveDetached".into()),
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

/// Follow the Tool Sets and palettes switches (View ▸ Tool Sets, View ▸ Palettes, workspaces),
/// then report what shows back to them.
fn sync(workspace: &mut Workspace, ui: &mut crate::UiState) -> Result<(), String> {
    // A palette in its own window stays there while its switch is on; turned off, its window closes.
    // A switch turned on opens its palette the rail's way, so other palettes that aren't pinned
    // close and only one switch's palette opens at a time (the other switch then reads off).
    let mut opened = false;
    if !workspace.is_detached(Panel::ToolSets) {
        if ui.show_toolsets && !workspace.layout.contains(&Panel::ToolSets) {
            workspace.open_one(Panel::ToolSets)?;
            opened = true;
        } else if !ui.show_toolsets {
            workspace.set_visible(Panel::ToolSets, false)?;
        }
    } else if !ui.show_toolsets {
        workspace.detached.retain(|d| d.panel != Panel::ToolSets);
    }
    if workspace.is_detached(Panel::Layers) || workspace.is_detached(Panel::Properties) {
        if !ui.show_palettes {
            workspace.detached.retain(|d| !matches!(d.panel, Panel::Layers | Panel::Properties));
            workspace.set_visible(Panel::Layers, false)?;
            workspace.set_visible(Panel::Properties, false)?;
        }
    } else if !ui.show_palettes {
        workspace.set_visible(Panel::Layers, false)?;
        workspace.set_visible(Panel::Properties, false)?;
    } else if !opened && !workspace.layout.contains(&Panel::Layers) && !workspace.layout.contains(&Panel::Properties) {
        if workspace.pinned.iter().any(|p| matches!(p, Panel::Layers | Panel::Properties)) {
            // Pinned ones come back where they were, beside whatever else shows. Reopen the last
            // surviving accordion entry before restoring its earlier sibling.
            for p in [Panel::Properties, Panel::Layers] {
                if workspace.pinned.contains(&p) {
                    workspace.set_visible(p, true)?;
                }
            }
        } else {
            workspace.open_one(Panel::Properties)?;
        }
    }
    publish(ui, workspace);
    Ok(())
}

/// The Tool Sets and palettes switches say what shows.
fn publish(ui: &mut crate::UiState, workspace: &Workspace) {
    let shown = |p: Panel| workspace.layout.contains(&p) || workspace.is_detached(p);
    ui.show_toolsets = shown(Panel::ToolSets);
    ui.show_palettes = shown(Panel::Layers) || shown(Panel::Properties);
}

fn publish_visibility(app: &mut crate::CadApp, workspace: &Workspace) {
    publish(&mut app.ui, workspace);
}

/// A layout saved before the icon rail moves to it. The default arrangement as it came becomes
/// the rail's default: the palettes close and the drawing gets the window. A layout the user
/// arranged keeps every palette where it is, pinned, so it stays as it was; the palettes it
/// doesn't show wait on the rail.
pub fn upgrade(ui: &mut crate::UiState) {
    if ui.docking.rail >= RAIL_LAYOUT {
        return;
    }
    if ui.docking.is_classic() {
        ui.docking = if ui.layout_rtl { Workspace::default().mirrored() } else { Workspace::default() };
        ui.show_palettes = false;
        ui.show_toolsets = false;
        return;
    }
    let mut w = ui.docking.clone();
    w.rail = RAIL_LAYOUT;
    w.mirrored = ui.layout_rtl;
    w.pinned = w.layout.panels().into_iter().copied().filter(|p| *p != Panel::Canvas).collect();
    for p in Panel::RAIL {
        if !w.layout.contains(p) && !w.is_detached(*p) && !w.hidden.iter().any(|(h, _)| h == p) {
            w.hidden.push((*p, rail_location(*p, w.mirrored)));
        }
    }
    if w.validate().is_ok() {
        ui.docking = w;
    } else {
        // Not usable as saved (it would be replaced by the default anyway): don't look again.
        ui.docking.rail = RAIL_LAYOUT;
    }
}

pub fn command(app: &mut crate::CadApp, reset: bool, params: &Value) -> Result<Value, String> {
    upgrade(&mut app.ui);
    let mut workspace = app.ui.docking.clone();
    if reset {
        workspace = if app.ui.layout_rtl { Workspace::default().mirrored() } else { Workspace::default() };
    } else {
        sync(&mut workspace, &mut app.ui)?;
        workspace.command(params)?;
    }
    publish_visibility(app, &workspace);
    app.ui.docking = workspace;
    Ok(json!({"docking": app.ui.docking}))
}

pub fn show(app: &mut crate::CadApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get();
    upgrade(&mut app.ui);
    let mut workspace = app.ui.docking.clone();
    if let Err(error) = workspace.validate().and_then(|()| sync(&mut workspace, &mut app.ui)) {
        app.set_status(format!("Panel layout could not be restored: {error}"));
        workspace = if app.ui.layout_rtl { Workspace::default().mirrored() } else { Workspace::default() };
        let _ = sync(&mut workspace, &mut app.ui);
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
        |ui, panel| {
            if *panel != Panel::Canvas {
                pane_bar(app, ui, *panel);
            }
            panel_body(app, ui, *panel)
        },
    );
    // Palettes in windows of their own.
    show_detached(app, ui.ctx(), &workspace);
    // Existing panel close buttons and commands still own their visibility switches.
    finish(app, ui, workspace, output.actions);
}

/// A docked palette's own buttons, at its top edge on the rail's side's far end: pin it (it
/// stays when another palette opens from the rail) and hide it (back onto the rail).
fn pane_bar(app: &mut crate::CadApp, ui: &mut egui::Ui, panel: Panel) {
    let pinned = app.ui.docking.pinned.contains(&panel);
    let rtl = app.ui.docking.mirrored;
    let mut operation = None;
    let layout = if rtl { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::right_to_left(egui::Align::Center) };
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), PANE_BAR), layout, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let hide = if rtl { crate::icons::Icon::ChevronRight } else { crate::icons::Icon::ChevronLeft };
        if crate::icons::button(ui, hide, PANE_BAR - 2.0, "Hide", false).clicked() {
            operation = Some(json!({ "operation": "close", "panel": panel.id() }));
        }
        if crate::icons::button(ui, crate::icons::Icon::Pin, PANE_BAR - 2.0, if pinned { "Unpin" } else { "Pin" }, pinned).clicked() {
            operation = Some(json!({ "operation": "pin", "panel": panel.id(), "pinned": !pinned }));
        }
    });
    if let Some(params) = operation
        && let Err(e) = command(app, false, &params)
    {
        app.set_status(format!("Palette: {e}"));
    }
}

/// Height of a docked palette's pin and hide buttons' row, points.
const PANE_BAR: f32 = 20.0;

/// A panel's contents, in the dock area or a palette window.
fn panel_body(app: &mut crate::CadApp, ui: &mut egui::Ui, panel: Panel) {
    // Each palette's own part of the frame (`perf.sections`: `panel:<id>`).
    crate::framecost::time(panel_part(panel), || panel_contents(app, ui, panel));
}

/// A palette's name in `perf.sections`.
fn panel_part(panel: Panel) -> &'static str {
    match panel {
        Panel::Canvas => "panel:canvas",
        Panel::ToolSets => "panel:toolSets",
        Panel::QuickCalc => "panel:quickCalc",
        Panel::Layers => "panel:layers",
        Panel::Properties => "panel:properties",
        Panel::ExternalRefs => "panel:externalReferences",
        Panel::PlansToModel => "panel:plansToModel",
        Panel::SheetSets => "panel:sheetSets",
        Panel::Symbols => "panel:symbols",
        Panel::Coordination => "panel:coordination",
        Panel::Validate => "panel:validate",
        Panel::Job => "panel:job",
    }
}

fn panel_contents(app: &mut crate::CadApp, ui: &mut egui::Ui, panel: Panel) {
    let t = crate::theme::Tokens::get();
    match panel {
        Panel::Canvas => {
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.canvas);
            // Walk mode (3DWALK, 3DFLY, View ▸ Walk) takes the drawing's place in its tab.
            if app.walk.is_some() {
                crate::walk::show(app, ui);
            } else {
                // The drawing in the main window: not one torn off into its own window.
                crate::tearoff::with_main_doc(app, ui, crate::canvas::show);
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
        Panel::PlansToModel => crate::planstomodel::show(app, ui),
        Panel::SheetSets => crate::sheetsets::show(app, ui),
        Panel::Symbols => {
            egui::ScrollArea::vertical()
                .id_salt("dock-symbols")
                .auto_shrink([false, false])
                .show(ui, |ui| crate::palettes::symbols::symbols_section(app, ui));
        }
        Panel::Coordination => {
            egui::ScrollArea::vertical().id_salt("dock-coordination").auto_shrink([false, false]).show(ui, |ui| crate::bimpalette::show(app, ui));
        }
        Panel::Validate => crate::validatepalette::show(app, ui),
        Panel::Job => crate::jobpalette::show(app, ui),
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
    let _ = sync(&mut workspace, &mut app.ui);
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
        Panel::PlansToModel => (egui::vec2(280.0, 200.0), 900.0),
        Panel::SheetSets => (egui::vec2(280.0, 220.0), 600.0),
        Panel::ToolSets => (egui::vec2(220.0, 150.0), 480.0),
        // Properties is the palette used most: it keeps room for its fields (it shrank to one
        // row beside an open External References panel). External References and Symbols need
        // less.
        Panel::Properties => (egui::vec2(220.0, 220.0), 720.0),
        Panel::Layers => (egui::vec2(220.0, 120.0), 720.0),
        Panel::ExternalRefs | Panel::Symbols => (egui::vec2(220.0, 80.0), 720.0),
        Panel::Coordination => (egui::vec2(280.0, 240.0), 900.0),
        Panel::Validate => (egui::vec2(320.0, 260.0), 1200.0),
        Panel::Job => (egui::vec2(320.0, 260.0), 1200.0),
    };
    PanelLimits { min, max: egui::vec2(max_width, 1_000_000.0) }
}

#[cfg(test)]
mod rail_tests {
    use super::*;

    fn shown(w: &Workspace) -> Vec<Panel> {
        w.layout.panels().into_iter().copied().filter(|p| *p != Panel::Canvas).collect()
    }

    /// The default leaves the drawing alone in the window; every palette has its place beside
    /// the rail ready.
    #[test]
    fn default_shows_only_the_drawing_and_every_palette_waits_on_the_rail() {
        let w = Workspace::default();
        w.validate().unwrap();
        assert!(shown(&w).is_empty());
        assert_eq!(w.layout.root, Some(tabs(Panel::Canvas)));
        for p in Panel::RAIL {
            assert!(w.hidden.iter().any(|(h, _)| h == p), "{p:?} has a place");
        }
        assert_eq!(Panel::RAIL.len(), Panel::ALL.len() + Panel::ON_DEMAND.len() - 1, "every palette but the drawing is on the rail");
    }

    /// One palette at a time unless pinned; the button again closes it.
    #[test]
    fn rail_opens_one_palette_unless_pinned() {
        let mut w = Workspace::default();
        w.toggle(Panel::Layers).unwrap();
        assert_eq!(shown(&w), [Panel::Layers]);
        // Beside the rail, at its own width.
        assert_eq!(w.layout.location(&Panel::Layers).unwrap().placement, Placement::Split(Zone::Left));
        w.toggle(Panel::Coordination).unwrap();
        assert_eq!(shown(&w), [Panel::Coordination]);
        w.pin(Panel::Coordination, true).unwrap();
        w.toggle(Panel::Properties).unwrap();
        let mut both = shown(&w);
        both.sort_by_key(|p| p.id());
        assert_eq!(both, [Panel::Coordination, Panel::Properties]);
        w.toggle(Panel::Layers).unwrap();
        assert!(w.layout.contains(&Panel::Coordination) && w.layout.contains(&Panel::Layers) && !w.layout.contains(&Panel::Properties));
        w.toggle(Panel::Layers).unwrap();
        assert_eq!(shown(&w), [Panel::Coordination]);
        w.pin(Panel::Coordination, false).unwrap();
        w.toggle(Panel::Coordination).unwrap();
        assert!(shown(&w).is_empty());
        assert!(w.toggle(Panel::Canvas).is_err() && w.pin(Panel::Canvas, true).is_err());
        w.validate().unwrap();
    }

    /// A palette dragged to another width comes back at it.
    #[test]
    fn closed_palette_comes_back_at_the_width_it_had() {
        let mut w = Workspace::default();
        w.toggle(Panel::Properties).unwrap();
        w.apply(Action::ResizeSplit { path: Vec::new(), size: SplitSize::FixedFirst(410.0) }).unwrap();
        w.toggle(Panel::Properties).unwrap();
        w.toggle(Panel::Properties).unwrap();
        assert!(
            matches!(&w.layout.root, Some(Node::Split { size: SplitSize::FixedFirst(v), .. }) if (*v - 410.0).abs() < 0.01),
            "{:?}",
            w.layout.root
        );
    }

    /// A palette behind another tab comes to the front instead of closing.
    #[test]
    fn rail_brings_a_palette_behind_a_tab_to_the_front() {
        let mut w = Workspace::default();
        w.pin(Panel::Properties, true).unwrap();
        w.pin(Panel::Layers, true).unwrap();
        w.toggle(Panel::Properties).unwrap();
        w.toggle(Panel::Layers).unwrap();
        w.apply(Action::Move { panel: Panel::Layers, anchor: Panel::Properties, placement: Placement::Tab { before: None } }).unwrap();
        w.apply(Action::Activate { panel: Panel::Properties }).unwrap();
        assert!(!w.showing(Panel::Layers));
        w.toggle(Panel::Layers).unwrap();
        assert!(w.showing(Panel::Layers) && w.layout.contains(&Panel::Properties));
    }

    /// Mirroring moves the rail's places to the other side too, and twice is the original.
    #[test]
    fn mirroring_moves_the_closed_palettes_places_too() {
        let w = Workspace::default();
        let m = w.mirrored();
        assert!(m.mirrored);
        for (_, l) in &m.hidden {
            assert_eq!(l.placement, Placement::Split(Zone::Right));
            assert!(matches!(l.size, Some(SplitSize::FixedSecond(_))));
        }
        let back = m.mirrored();
        assert_eq!(back.hidden, w.hidden);
        assert!(!back.mirrored);
        let mut m = m;
        m.toggle(Panel::Layers).unwrap();
        assert_eq!(m.layout.location(&Panel::Layers).unwrap().placement, Placement::Split(Zone::Right));
    }

    /// Hostile saved settings: a pinned drawing or a duplicate pin is refused.
    #[test]
    fn invalid_pins_are_refused() {
        let mut w = Workspace { pinned: vec![Panel::Canvas], ..Workspace::default() };
        assert!(w.validate().is_err());
        w.pinned = vec![Panel::Layers, Panel::Layers];
        assert!(w.validate().is_err());
    }

    fn old_settings(docking: &Workspace) -> crate::UiState {
        let mut v = serde_json::to_value(docking).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("rail");
        o.remove("pinned");
        o.remove("mirrored");
        let mut ui = crate::UiState { docking: serde_json::from_value(v).unwrap(), ..crate::UiState::default() };
        ui.show_palettes = true;
        ui
    }

    /// Settings saved before the rail with the default as it came move to the rail's default;
    /// a layout the user arranged stays exactly as it was, its palettes pinned.
    #[test]
    fn saved_layouts_from_before_the_rail() {
        let mut ui = old_settings(&Workspace::classic());
        assert_eq!(ui.docking.rail, 0, "missing means before the rail");
        upgrade(&mut ui);
        assert_eq!(ui.docking.layout, Workspace::default().layout);
        assert!(!ui.show_palettes);
        // Tool Sets closed, the app's own Properties height: still the default as it came.
        let mut w = Workspace::classic();
        w.apply(Action::Close { panel: Panel::ToolSets }).unwrap();
        w.apply(Action::ResizeStack { panel: Panel::Properties, height: Some(377.0) }).unwrap();
        let mut ui = old_settings(&w);
        upgrade(&mut ui);
        assert_eq!(ui.docking.layout, Workspace::default().layout);
        // Arranged by the user: Symbols floated, Layers moved left of the drawing.
        let mut w = Workspace::classic();
        w.apply(Action::Float { panel: Panel::Symbols, rect: [400.0, 200.0, 260.0, 300.0] }).unwrap();
        w.apply(Action::Move { panel: Panel::Layers, anchor: Panel::Canvas, placement: Placement::Split(Zone::Left) }).unwrap();
        let mut ui = old_settings(&w);
        let before = ui.docking.layout.clone();
        upgrade(&mut ui);
        assert_eq!(ui.docking.layout, before, "the user's layout is kept");
        assert_eq!(ui.docking.rail, RAIL_LAYOUT);
        for p in [Panel::ToolSets, Panel::Layers, Panel::Properties, Panel::Symbols] {
            assert!(ui.docking.pinned.contains(&p), "{p:?} pinned");
        }
        ui.docking.validate().unwrap();
        // It stays: opening another palette from the rail leaves them.
        ui.docking.toggle(Panel::Coordination).unwrap();
        for p in [Panel::ToolSets, Panel::Layers, Panel::Properties, Panel::Symbols, Panel::Coordination] {
            assert!(ui.docking.layout.contains(&p), "{p:?}");
        }
        // Upgraded once: a second look changes nothing.
        let again = ui.docking.clone();
        upgrade(&mut ui);
        assert_eq!(ui.docking.layout, again.layout);
        // A mirrored (right-to-left) default as it came is recognised too.
        let mut ui = old_settings(&Workspace::classic().mirrored());
        ui.layout_rtl = true;
        upgrade(&mut ui);
        assert_eq!(ui.docking.layout, Workspace::default().layout);
        assert!(ui.docking.mirrored);
    }

    /// Workspaces made before the rail (saved user workspaces) load as they were saved.
    #[test]
    fn rail_fields_round_trip_and_default_when_missing() {
        let mut w = Workspace::docked();
        w.toggle(Panel::Coordination).unwrap();
        w.pin(Panel::Coordination, true).unwrap();
        let text = serde_json::to_string(&w).unwrap();
        let back: Workspace = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&w).unwrap());
        assert_eq!(back.rail, RAIL_LAYOUT);
        let empty: Workspace = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.rail, 0);
    }
}

#[cfg(test)]
mod height_tests {
    use super::*;

    /// UI review 0.7.7b #3: Properties, the palette used most, keeps most of the right-hand
    /// column at every window height; Layers keeps its icon row and list; External References
    /// below them takes a bounded slice, not half.
    #[test]
    fn properties_keeps_most_of_the_column_and_layers_its_minimum() {
        for stack in [260.0_f32, 300.0, 400.0, 480.0, 640.0, 900.0, 1400.0] {
            let w = properties_weight(stack);
            let properties = stack * w / (w + LAYERS_HEIGHT);
            let layers = stack - properties;
            assert!(layers >= LAYERS_MIN - 0.5, "{stack}: Layers {layers}");
            assert!(properties >= layers - 0.5, "{stack}: Properties {properties} vs Layers {layers}");
        }
        for column in [300.0_f32, 520.0, 800.0, 1400.0] {
            let below = below_properties(column);
            assert!((80.0..=240.0).contains(&below) && below <= column * 0.3, "{column}: {below}");
        }
    }
}
