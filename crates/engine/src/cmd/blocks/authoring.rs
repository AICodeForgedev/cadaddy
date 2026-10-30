//! The Block Editor's authoring tools: BAUTHORPALETTE lists the Block Authoring Palettes (a
//! Parameters tab and an Actions tab with a tool for every parameter and action type) and shows
//! or hides them; a tool runs BPARAMETER or BACTIONTOOL with picks at the prompt instead of JSON
//! (the points are the editor's, the block's coordinates follow from its base point). BTESTBLOCK
//! opens a test window on a temporary copy of the block, as the editor has it, and closing it
//! leaves the drawing as it was.

use cadaddy_doc::Handle;
use cadaddy_doc::dynblock::{self, DynamicBlock, ParameterKind};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::actionbar::edited;
use crate::cmd::machines::{SelOutcome, SelectPhase};
use crate::cmd::{CommandSpec, bad, find_command, str_param};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// A parameter tool of the palette: its id (BPARAMETER's `type`), label and the points it takes.
const PARAMETERS: &[(&str, &str, usize)] = &[
    ("point", "Point", 1),
    ("linear", "Linear", 2),
    ("polar", "Polar", 2),
    ("xy", "XY", 2),
    ("rotation", "Rotation", 2),
    ("alignment", "Alignment", 2),
    ("flip", "Flip", 2),
    ("visibility", "Visibility", 1),
    ("lookup", "Lookup", 1),
    ("basepoint", "Base Point", 1),
];

/// An action tool of the palette: its id (BACTIONTOOL's `type`) and label.
const ACTIONS: &[(&str, &str)] = &[
    ("move", "Move"),
    ("scale", "Scale"),
    ("stretch", "Stretch"),
    ("polarstretch", "Polar Stretch"),
    ("rotate", "Rotate"),
    ("flip", "Flip"),
    ("array", "Array"),
    ("lookup", "Lookup"),
];

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bauthorpalette", "Block Authoring Palettes", run_palette)
            .menu(&["Tools", "Palettes", "Block Authoring Palettes"])
            .params("{action?: list (default) | show | hide | toggle} → {visible, block, tabs: [{id, label, tools: [{id, label, command, picks: [prompt…]}]}]} (Block Editor); a tool is run with BPARAMETER or BACTIONTOOL, which ask for their picks when typed without parameters")
            .enabled(editing_a_block)
            .noundo(),
        CommandSpec::new("btestblock", "Test Block", run_test)
            .params("{action?: open (default) | close} → {block, reference?} (Block Editor): open shows the block, as the editor has it, in a temporary drawing with one reference to try its properties and grips on; close throws it away")
            .enabled(testing_or_editing)
            .noundo(),
    ]
}

fn editing_a_block(s: &Session) -> std::result::Result<(), String> {
    match s.state() {
        Ok(st) if st.block_edit.as_ref().is_some_and(|b| !b.current) => Ok(()),
        Ok(st) if st.block_edit.is_some() => Err("The Block Editor is open on the current drawing, not on a block.".into()),
        Ok(st) if st.block_testing() => Err("The test window is open; close it first (BTESTBLOCK close).".into()),
        Ok(_) => Err("The Block Editor is not open (BEDIT).".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn testing_or_editing(s: &Session) -> std::result::Result<(), String> {
    match s.state() {
        Ok(st) if st.block_testing() => Ok(()),
        _ => editing_a_block(s),
    }
}

// ---------------- BAUTHORPALETTE ----------------

/// The palettes' tabs: every parameter and action type, and what each asks for.
pub fn palette_tabs() -> Value {
    let parameters: Vec<Value> = PARAMETERS
        .iter()
        .map(|(id, label, points)| {
            let picks: Vec<&str> = if *points == 2 { vec!["Specify start point", "Specify end point"] } else { vec!["Specify parameter location"] };
            json!({ "id": id, "label": label, "command": "bparameter", "picks": picks })
        })
        .collect();
    let actions: Vec<Value> = ACTIONS
        .iter()
        .map(|(id, label)| {
            let picks: Vec<&str> = match *id {
                "stretch" => vec!["Select parameter", "Specify first corner of the stretch frame", "Specify opposite corner", "Select objects"],
                "polarstretch" => vec![
                    "Select parameter",
                    "Specify first corner of the stretch frame",
                    "Specify opposite corner",
                    "Select objects",
                    "Select objects to rotate only",
                ],
                "array" => vec!["Select parameter", "Select objects", "Specify column offset"],
                "lookup" => Vec::new(),
                _ => vec!["Select parameter", "Select objects"],
            };
            json!({ "id": id, "label": label, "command": "bactiontool", "picks": picks, "typed": *id != "lookup" })
        })
        .collect();
    json!([
        { "id": "parameters", "label": "Parameters", "tools": parameters },
        { "id": "actions", "label": "Actions", "tools": actions },
    ])
}

fn run_palette(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bauthorpalette";
    let (block, _) = edited(s, cmd)?;
    let action = str_param(p, "action").unwrap_or("list").to_ascii_lowercase();
    let visible = s.state()?.block_edit.as_ref().is_some_and(|b| b.palette);
    let now = match action.as_str() {
        "list" => visible,
        "show" => true,
        "hide" => false,
        "toggle" => !visible,
        other => return Err(bad(cmd, format!("unknown action \"{other}\" (list, show, hide or toggle)"))),
    };
    if now != visible
        && let Some(be) = s.state_mut()?.block_edit.as_mut()
    {
        be.palette = now;
        s.touch();
    }
    Ok(json!({ "visible": now, "block": block, "tabs": palette_tabs() }))
}

// ---------------- BTESTBLOCK ----------------

fn run_test(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "btestblock";
    let action = str_param(p, "action").unwrap_or("open").to_ascii_lowercase();
    match action.as_str() {
        "open" => {
            let (block, _) = edited(s, cmd)?;
            s.state_mut()?.open_block_test()?;
            // One reference to the block at the origin: its properties and grips can be tried.
            let spec = find_command("insert").ok_or_else(|| EngineError::UnknownCommand("insert".into()))?;
            (spec.run)(s, &json!({ "name": block, "at": [0, 0] }))?;
            let reference = s.doc()?.model.handles().last().map(|h| h.hex());
            s.zoom_extents()?;
            let c = s.state()?.view().center;
            s.zoom_about(0.75, c)?;
            s.echo(format!("Test window for block \"{block}\": change its properties and grips; BTESTBLOCK close throws the changes away."));
            Ok(json!({ "block": block, "reference": reference }))
        }
        "close" => {
            let block = s.state()?.block_test.as_ref().and_then(|h| h.block_editor().map(str::to_string));
            s.state_mut()?.close_block_test()?;
            Ok(json!({ "block": block }))
        }
        other => Err(bad(cmd, format!("unknown action \"{other}\" (open or close)"))),
    }
}

// ---------------- BPARAMETER at the prompt ----------------

/// A parameter tool's id for what was typed (`Linear`, `x y`, `base point`).
fn parameter_tool(text: &str) -> Option<&'static str> {
    let t: String = text.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    PARAMETERS.iter().find(|(id, label, _)| *id == t || label.to_ascii_lowercase().replace(' ', "") == t).map(|(id, _, _)| *id)
}

/// BPARAMETER typed without parameters: the parameter type, then its points, in the Block Editor.
pub(super) struct ParameterRun {
    block: String,
    /// The editor's origin in block coordinates.
    base: Vec2,
    tool: Option<(&'static str, usize)>,
    points: Vec<Vec2>,
}

impl ParameterRun {
    pub(super) fn new(s: &Session) -> Result<Self> {
        let (block, base) = edited(s, "bparameter").map_err(|_| EngineError::Other("Open a block in the Block Editor first (BEDIT).".into()))?;
        Ok(ParameterRun { block, base, tool: None, points: Vec::new() })
    }

    fn choose(&mut self, text: &str) -> Result<Step> {
        let id = parameter_tool(text).ok_or_else(|| EngineError::Other(format!("\"{}\" is not a parameter type.", text.trim())))?;
        let points = PARAMETERS.iter().find(|(i, _, _)| *i == id).map_or(1, |(_, _, n)| *n);
        self.tool = Some((id, points));
        Ok(Step::Continue)
    }

    fn finish(&self, s: &mut Session) -> Result<()> {
        let Some((id, points)) = self.tool else { return Ok(()) };
        let at = |p: &Vec2| json!([p.x + self.base.x, p.y + self.base.y]);
        let mut p = json!({ "block": self.block, "type": id });
        if points == 2 {
            p["base"] = self.points.first().map(at).unwrap_or(Value::Null);
            p["end"] = self.points.get(1).map(at).unwrap_or(Value::Null);
        } else {
            p["at"] = self.points.first().map(at).unwrap_or(Value::Null);
        }
        let spec = find_command("bparameter").ok_or_else(|| EngineError::UnknownCommand("bparameter".into()))?;
        let r = (spec.run)(s, &p)?;
        let label = r.get("label").and_then(Value::as_str).unwrap_or(id);
        s.echo(format!("Parameter \"{label}\" added to block \"{}\".", self.block));
        Ok(())
    }
}

impl Interactive for ParameterRun {
    fn name(&self) -> &'static str {
        "BPARAMETER"
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        match self.tool {
            None => Prompt::new("Enter parameter type", Accept::TEXT).kw(&["Point", "Linear", "POlar", "XY", "Rotation", "ALignment", "Flip", "Visibility", "LOokup", "Basepoint"]),
            Some((_, 2)) if self.points.is_empty() => Prompt::new("Specify start point", Accept::POINT),
            Some((_, 2)) => Prompt::new("Specify end point", Accept::POINT).base_opt(self.points.first().copied()),
            Some(_) => Prompt::new("Specify parameter location", Accept::POINT),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.tool, i) {
            (None, Input::Keyword(k) | Input::Text(k)) => self.choose(&k),
            (Some((_, n)), Input::Point(p)) => {
                self.points.push(p);
                if self.points.len() < n {
                    return Ok(Step::Continue);
                }
                self.finish(s)?;
                Ok(Step::Done)
            }
            (_, Input::Enter | Input::Cancel) => Ok(Step::Cancel),
            _ => Err(EngineError::Other("Requires a point.".into())),
        }
    }

    fn preview(&self, _s: &Session, c: Vec2) -> Vec<cadaddy_doc::EntityKind> {
        match (self.tool, self.points.first()) {
            (Some((_, 2)), Some(a)) => vec![crate::cmd::helpers::line(*a, c)],
            _ => Vec::new(),
        }
    }
}

// ---------------- BACTIONTOOL at the prompt ----------------

/// What an action tool asks for next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Tool,
    Parameter,
    FrameA,
    FrameB,
    Objects,
    Rotate,
    Column,
    Row,
}

/// Whether `kind` is a parameter the action tool `tool` can follow.
fn follows(tool: &str, kind: &ParameterKind) -> bool {
    use ParameterKind::{Alignment, Flip, Linear, Point, Polar, Rotation, Xy};
    match tool {
        "move" | "stretch" => matches!(kind, Point { .. } | Linear { .. } | Polar { .. } | Xy { .. }),
        "scale" => matches!(kind, Linear { .. } | Polar { .. }),
        "polarstretch" => matches!(kind, Polar { .. }),
        "rotate" => matches!(kind, Rotation { .. } | Polar { .. } | Alignment { .. }),
        "flip" => matches!(kind, Flip { .. }),
        "array" => matches!(kind, Linear { .. } | Polar { .. } | Xy { .. }),
        _ => false,
    }
}

/// BACTIONTOOL typed without parameters: the action, the parameter it follows, then its picks.
pub(super) struct ActionRun {
    block: String,
    base: Vec2,
    tool: Option<&'static str>,
    stage: Stage,
    parameter: Option<i32>,
    xy: bool,
    corner: Option<Vec2>,
    frame: Vec<Vec2>,
    select: SelectPhase,
    objects: Vec<Handle>,
    rotate: Vec<Handle>,
    column: f64,
    row: f64,
}

impl ActionRun {
    pub(super) fn new(s: &Session) -> Result<Self> {
        let (block, base) = edited(s, "bactiontool").map_err(|_| EngineError::Other("Open a block in the Block Editor first (BEDIT).".into()))?;
        Ok(ActionRun {
            block,
            base,
            tool: None,
            stage: Stage::Tool,
            parameter: None,
            xy: false,
            corner: None,
            frame: Vec::new(),
            select: SelectPhase::default(),
            objects: Vec::new(),
            rotate: Vec::new(),
            column: 0.0,
            row: 0.0,
        })
    }

    fn model(&self, s: &Session) -> Result<DynamicBlock> {
        dynblock::dynamic_block(s.doc()?, &self.block)
            .ok_or_else(|| EngineError::Other(format!("Block \"{}\" has no parameters yet; add one first (BPARAMETER).", self.block)))
    }

    /// The parameters of the block the chosen tool can follow.
    fn suitable(&self, s: &Session) -> Result<Vec<(i32, String)>> {
        let tool = self.tool.unwrap_or_default();
        let found: Vec<(i32, String)> = self
            .model(s)?
            .parameters
            .iter()
            .filter(|p| follows(tool, &p.kind))
            .map(|p| (p.node, p.kind.label().unwrap_or(&p.element).to_string()))
            .collect();
        if found.is_empty() {
            return Err(EngineError::Other(format!("The block has no parameter a {tool} action can follow (BPARAMETER).")));
        }
        Ok(found)
    }

    fn choose_tool(&mut self, s: &Session, text: &str) -> Result<Step> {
        let t: String = text.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let (id, _) = ACTIONS.iter().find(|(id, label)| *id == t || label.to_ascii_lowercase().replace(' ', "") == t).ok_or_else(|| {
            EngineError::Other(format!("\"{}\" is not an action type (move, scale, stretch, polar stretch, rotate, flip, array).", text.trim()))
        })?;
        if *id == "lookup" {
            return Err(EngineError::Other("A lookup table is entered as BACTIONTOOL parameters (type: lookup, columns, rows), not at the prompt.".into()));
        }
        self.tool = Some(id);
        let found = self.suitable(s)?;
        if let [(node, label)] = found.as_slice() {
            self.parameter = Some(*node);
            self.after_parameter(s, label)?;
        } else {
            self.stage = Stage::Parameter;
        }
        Ok(Step::Continue)
    }

    fn after_parameter(&mut self, s: &Session, _label: &str) -> Result<()> {
        let m = self.model(s)?;
        self.xy = m.parameters.iter().any(|p| Some(p.node) == self.parameter && matches!(p.kind, ParameterKind::Xy { .. }));
        self.stage = if matches!(self.tool, Some("stretch" | "polarstretch")) { Stage::FrameA } else { Stage::Objects };
        Ok(())
    }

    fn choose_parameter(&mut self, s: &Session, text: &str) -> Result<Step> {
        let want = text.trim();
        let found = self.suitable(s)?;
        let m = self.model(s)?;
        let hit = found.iter().find(|(node, label)| {
            label.eq_ignore_ascii_case(want)
                || want.parse::<i32>().is_ok_and(|n| n == *node)
                || m.parameters.iter().any(|p| p.node == *node && p.element.eq_ignore_ascii_case(want))
        });
        let Some((node, label)) = hit else {
            let names: Vec<&str> = found.iter().map(|(_, l)| l.as_str()).collect();
            return Err(EngineError::Other(format!("\"{want}\" is not a parameter this action can follow: {}.", names.join(", "))));
        };
        self.parameter = Some(*node);
        let label = label.clone();
        self.after_parameter(s, &label)?;
        Ok(Step::Continue)
    }

    fn finish(&self, s: &mut Session) -> Result<()> {
        let Some(tool) = self.tool else { return Ok(()) };
        let hexes = |hs: &[Handle]| hs.iter().map(|h| h.hex()).collect::<Vec<_>>();
        let mut p = json!({ "block": self.block, "type": tool, "parameter": self.parameter, "handles": hexes(&self.objects) });
        if matches!(tool, "stretch" | "polarstretch") {
            p["frame"] = json!(self.frame.iter().map(|f| [f.x + self.base.x, f.y + self.base.y]).collect::<Vec<_>>());
        }
        if tool == "polarstretch" {
            p["rotate"] = json!(hexes(&self.rotate));
        }
        if tool == "array" {
            p["columnOffset"] = json!(self.column);
            if self.xy {
                p["rowOffset"] = json!(self.row);
            }
        }
        let spec = find_command("bactiontool").ok_or_else(|| EngineError::UnknownCommand("bactiontool".into()))?;
        let r = (spec.run)(s, &p)?;
        let name = r.get("name").and_then(Value::as_str).unwrap_or(tool);
        s.echo(format!("Action \"{name}\" added to block \"{}\".", self.block));
        Ok(())
    }

    /// The selection stage is done: another stage, or the action.
    fn objects_done(&mut self, s: &mut Session, picked: Vec<Handle>) -> Result<Step> {
        match self.stage {
            Stage::Objects => {
                self.objects = picked;
                match self.tool {
                    Some("polarstretch") => {
                        self.stage = Stage::Rotate;
                        self.select = SelectPhase::default();
                        Ok(Step::Continue)
                    }
                    Some("array") => {
                        self.stage = Stage::Column;
                        Ok(Step::Continue)
                    }
                    _ => {
                        self.finish(s)?;
                        Ok(Step::Done)
                    }
                }
            }
            _ => {
                self.rotate = picked;
                self.finish(s)?;
                Ok(Step::Done)
            }
        }
    }
}

impl Interactive for ActionRun {
    fn name(&self) -> &'static str {
        "BACTIONTOOL"
    }

    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            Stage::Tool => Prompt::new("Enter action type", Accept::TEXT).kw(&["Move", "Scale", "Stretch", "POlarstretch", "Rotate", "Flip", "Array"]),
            Stage::Parameter => {
                let names: Vec<String> = self.suitable(s).map(|f| f.into_iter().map(|(_, l)| l).collect()).unwrap_or_default();
                Prompt::new(format!("Select parameter [{}]", names.join("/")), Accept::TEXT)
            }
            Stage::FrameA => Prompt::new("Specify first corner of the stretch frame", Accept::POINT),
            Stage::FrameB => Prompt::new("Specify opposite corner", Accept::POINT).corner(self.corner.unwrap_or_default()),
            Stage::Objects => {
                let mut p = self.select.prompt();
                p.message = if matches!(self.tool, Some("stretch" | "polarstretch")) { "Select objects to stretch".into() } else { p.message };
                p
            }
            Stage::Rotate => {
                let mut p = self.select.prompt();
                p.message = "Select objects to rotate only, or press Enter".into();
                p
            }
            Stage::Column => Prompt::new("Specify column offset", Accept::NUMBER),
            Stage::Row => Prompt::new("Specify row offset", Accept::NUMBER),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.stage, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (Stage::Tool, Input::Keyword(k) | Input::Text(k)) => self.choose_tool(s, &k),
            (Stage::Parameter, Input::Keyword(k) | Input::Text(k)) => self.choose_parameter(s, &k),
            (Stage::FrameA, Input::Point(p)) => {
                self.corner = Some(p);
                self.stage = Stage::FrameB;
                Ok(Step::Continue)
            }
            (Stage::FrameB, Input::Point(b)) => {
                let a = self.corner.unwrap_or(b);
                self.frame = vec![a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)];
                self.stage = Stage::Objects;
                self.select = SelectPhase::begin(s);
                if self.select.done {
                    let picked = self.select.picked.clone();
                    return self.objects_done(s, picked);
                }
                Ok(Step::Continue)
            }
            (Stage::Objects, i) => match self.select.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Err(EngineError::Other("Select the objects the action works on.".into())),
                SelOutcome::Done(hs) => self.objects_done(s, hs),
            },
            (Stage::Rotate, i) => match self.select.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => self.objects_done(s, Vec::new()),
                SelOutcome::Done(hs) => self.objects_done(s, hs),
            },
            (Stage::Column | Stage::Row, Input::Text(t)) => {
                let Some(v) = t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0) else {
                    return Err(EngineError::Other("Requires a distance above 0.".into()));
                };
                if self.stage == Stage::Column && self.xy {
                    self.column = v;
                    self.stage = Stage::Row;
                    return Ok(Step::Continue);
                }
                if self.stage == Stage::Column {
                    self.column = v;
                } else {
                    self.row = v;
                }
                self.finish(s)?;
                Ok(Step::Done)
            }
            (Stage::FrameA | Stage::FrameB, _) => Err(EngineError::Other("Requires a point.".into())),
            _ => Err(EngineError::Other("Requires a name.".into())),
        }
    }

    fn preview(&self, _s: &Session, c: Vec2) -> Vec<cadaddy_doc::EntityKind> {
        match (self.stage, self.corner) {
            (Stage::FrameB, Some(a)) => {
                let (b, d) = (Vec2::new(c.x, a.y), Vec2::new(a.x, c.y));
                [(a, b), (b, c), (c, d), (d, a)].iter().map(|(p, q)| crate::cmd::helpers::line(*p, *q)).collect()
            }
            _ => Vec::new(),
        }
    }
}
