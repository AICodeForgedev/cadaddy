//! COPYTOLAYER (Format ▸ Layer Tools ▸ Copy Objects to New Layer): copies of the selected
//! objects on another layer, in place or moved by a displacement.

use cadaddy_doc::Handle;
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("copytolayer", "Copy Objects to New Layer", run_copytolayer)
            .menu(&["Format", "Layer Tools", "Copy Objects to New Layer"])
            .params(
                "{handles? (default: selection), layer: destination layer (made when missing, unless create: false), \
                 delta?: [dx,dy] | from?: [x,y] + to?: [x,y] (default: in place)} → {handles, layer}",
            )
            .interactive(|_| Ok(Box::new(CopyToLayerM::default()))),
    ]
}

fn valid_layer_name(n: &str) -> bool {
    !n.is_empty() && n.chars().count() <= 255 && !n.chars().any(|c| "<>/\\\":;?*|,=`".contains(c) || c.is_control())
}

fn run_copytolayer(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "copytolayer";
    let layer =
        str_param(p, "layer").map(str::trim).filter(|l| !l.is_empty()).ok_or_else(|| bad(cmd, "`layer` (the destination layer) is required"))?;
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(cmd, "nothing to copy: give `handles` or select objects"));
    }
    let delta = match (point_param(p, "delta"), point_param(p, "from"), point_param(p, "to")) {
        (Some(d), _, _) => d,
        (None, Some(a), Some(b)) => b - a,
        (None, None, None) => Vec2::ZERO,
        _ => return Err(bad(cmd, "give `delta`, or both `from` and `to`")),
    };
    if !delta.is_finite() || delta.x.abs() > 1e15 || delta.y.abs() > 1e15 {
        return Err(bad(cmd, "the displacement must be finite"));
    }
    copy_to_layer(s, &hs, layer, delta, bool_or(p, "create", true))
}

fn copy_to_layer(s: &mut Session, hs: &[Handle], layer: &str, delta: Vec2, create: bool) -> Result<Value> {
    let d = s.doc_mut()?;
    let name = match d.layer(layer) {
        Some(l) => l.name.clone(),
        None if !create => return Err(EngineError::Other(format!("Layer \"{layer}\" not found."))),
        None if !valid_layer_name(layer) => return Err(EngineError::Other(format!("\"{layer}\" is not a valid layer name."))),
        None => {
            d.ensure_layer(layer);
            layer.to_string()
        }
    };
    let copies = super::super::modify::transform_entities(s, hs, &Mat3::translate(delta), true)?;
    let d = s.doc_mut()?;
    for h in &copies {
        d.modify_entity(*h, |e| e.common.layer = name.clone())?;
    }
    s.set_selection(Vec::new());
    let n = copies.len();
    Ok(json!({
        "handles": copies.iter().map(|h| h.hex()).collect::<Vec<_>>(),
        "layer": name,
        "message": format!("{n} object(s) copied and placed on layer \"{name}\"."),
    }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Select,
    Destination,
    Name,
    Base,
    Second(Vec2),
    Displacement,
}

/// "Select objects to copy:", "Select object on destination layer or [Name] <Name>:" (or the
/// layer's name), then "Specify base point or [Displacement/Exit] <eXit>:" and the second point.
#[derive(Default)]
struct CopyToLayerM {
    stage: Stage,
    sel: SelectPhase,
    picked: Vec<Handle>,
    layer: String,
}

impl CopyToLayerM {
    fn finish(&self, s: &mut Session, delta: Vec2) -> Result<Step> {
        let r = copy_to_layer(s, &self.picked, &self.layer, delta, true)?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
    fn destination(&mut self, s: &mut Session, layer: String) -> Step {
        s.echo(format!("Destination layer: \"{layer}\""));
        self.layer = layer;
        self.stage = Stage::Base;
        Step::Continue
    }
}

impl Interactive for CopyToLayerM {
    fn name(&self) -> &'static str {
        "COPYTOLAYER"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = self.sel.picked.clone();
            self.stage = Stage::Destination;
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            Stage::Select => Prompt::new("Select objects to copy", Accept::SELECT),
            Stage::Destination => Prompt::new("Select object on destination layer", Accept::POINT).kw(&["Name"]).default("Name"),
            Stage::Name => {
                let cur = s.doc().map(|d| d.header.str("CLAYER", "0")).unwrap_or_else(|_| "0".into());
                Prompt::new("Enter destination layer name", super::NAME).default(cur)
            }
            Stage::Base => Prompt::new("Specify base point", Accept::POINT).kw(&["Displacement", "Exit"]).default("eXit"),
            Stage::Second(b) => Prompt::new("Specify second point of displacement or <use first point as displacement>", Accept::POINT).base(b),
            Stage::Displacement => Prompt::new("Specify displacement", Accept::POINT).default("0,0"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()),
            _ => None,
        };
        match self.stage {
            Stage::Select => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.picked = hs;
                    self.stage = Stage::Destination;
                    Ok(Step::Continue)
                }
            },
            Stage::Destination => match (&i, text.as_deref()) {
                (Input::Point(p), _) => {
                    let h = curves::pick_at(s, *p).ok_or_else(|| EngineError::Other("*Invalid selection*".into()))?;
                    let layer = curves::entity(s, h)?.common.layer;
                    Ok(self.destination(s, layer))
                }
                (Input::Enter, _) => {
                    self.stage = Stage::Name;
                    Ok(Step::Continue)
                }
                (_, Some(t)) if t.eq_ignore_ascii_case("name") || t.eq_ignore_ascii_case("n") => {
                    self.stage = Stage::Name;
                    Ok(Step::Continue)
                }
                _ => Err(EngineError::Other("Invalid option keyword.".into())),
            },
            Stage::Name => {
                let name = match (&i, text) {
                    (Input::Enter, _) => s.doc()?.header.str("CLAYER", "0"),
                    (_, Some(t)) if !t.is_empty() => t.trim_matches('"').to_string(),
                    _ => return Ok(Step::Continue),
                };
                if s.doc()?.layer(&name).is_none() {
                    if !valid_layer_name(&name) {
                        return Err(EngineError::Other(format!("\"{name}\" is not a valid layer name.")));
                    }
                    s.echo(format!("Layer \"{name}\" will be created."));
                }
                Ok(self.destination(s, name))
            }
            Stage::Base => match (&i, text.as_deref()) {
                (Input::Point(p), _) => {
                    self.stage = Stage::Second(*p);
                    Ok(Step::Continue)
                }
                (Input::Enter, _) => self.finish(s, Vec2::ZERO),
                (_, Some(t)) if t.eq_ignore_ascii_case("exit") || t.eq_ignore_ascii_case("x") => self.finish(s, Vec2::ZERO),
                (_, Some(t)) if t.eq_ignore_ascii_case("displacement") || t.eq_ignore_ascii_case("d") => {
                    self.stage = Stage::Displacement;
                    Ok(Step::Continue)
                }
                _ => Err(EngineError::Other("Invalid option keyword.".into())),
            },
            Stage::Second(b) => match i {
                Input::Point(p) => self.finish(s, p - b),
                Input::Enter => self.finish(s, b),
                _ => Ok(Step::Continue),
            },
            Stage::Displacement => match i {
                Input::Point(p) => self.finish(s, p),
                Input::Enter => self.finish(s, Vec2::ZERO),
                _ => Ok(Step::Continue),
            },
        }
    }
}
