//! MOCORO: move, copy, rotate and scale the selected objects in one command, about a base point.
//! Moving carries the base point along; copying leaves copies behind; Undo steps back one
//! operation; the whole command is one undo step.

use std::sync::Arc;

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most operations one JSON call may hold.
const MAX_STEPS: usize = 1000;
/// Largest scale factor (and smallest, as its inverse).
const MAX_SCALE: f64 = 1e6;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Move(Vec2),
    Copy(Vec2),
    /// Radians, counterclockwise about the base.
    Rotate(f64),
    Scale(f64),
    Base(Vec2),
}

/// Apply `op` to `objs` about `base`; returns the new base point.
fn apply(s: &mut Session, objs: &[Handle], base: Vec2, op: Op) -> Result<Vec2> {
    let tm = |s: &mut Session, m: Mat3, copy: bool| super::super::modify::transform_entities(s, objs, &m, copy).map(|_| ());
    match op {
        Op::Move(to) => {
            tm(s, Mat3::translate(to - base), false)?;
            Ok(to)
        }
        Op::Copy(to) => {
            tm(s, Mat3::translate(to - base), true)?;
            Ok(base)
        }
        Op::Rotate(a) => {
            tm(s, Mat3::rotate_about(base, a), false)?;
            Ok(base)
        }
        Op::Scale(f) => {
            tm(s, Mat3::scale_about(base, f), false)?;
            Ok(base)
        }
        Op::Base(p) => Ok(p),
    }
}

fn scale_ok(f: f64) -> bool {
    f.is_finite() && f > 1.0 / MAX_SCALE && f < MAX_SCALE
}

/// One step of the JSON form: `{move|copy|base: [x, y]}`, `{rotate: degrees}` or `{scale: f}`.
fn op_of(cmd: &str, v: &Value) -> Result<Op> {
    let pt = |k: &str| v.get(k).and_then(point_value);
    if let Some(p) = pt("move") {
        return Ok(Op::Move(p));
    }
    if let Some(p) = pt("copy") {
        return Ok(Op::Copy(p));
    }
    if let Some(p) = pt("base") {
        return Ok(Op::Base(p));
    }
    if let Some(a) = v.get("rotate").and_then(Value::as_f64).filter(|a| a.is_finite() && a.abs() <= 1e6) {
        return Ok(Op::Rotate(a.to_radians()));
    }
    if let Some(f) = v.get("scale").and_then(Value::as_f64).filter(|f| scale_ok(*f)) {
        return Ok(Op::Scale(f));
    }
    Err(bad(cmd, format!("step {v} must be {{move|copy|base: [x, y]}}, {{rotate: degrees}} or {{scale: factor}}")))
}

/// `{handles?, base, steps: [{move|copy|base: [x, y]} | {rotate: degrees} | {scale: factor}, …]}`
/// → `{handles}` (the objects, then any copies).
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mocoro";
    let objs = targets(s, p)?;
    if objs.is_empty() {
        return Err(bad(cmd, "no objects selected"));
    }
    let mut base = point_req(cmd, p, "base")?;
    let steps = p.get("steps").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`steps` (an array of operations) is required"))?;
    if steps.len() > MAX_STEPS {
        return Err(bad(cmd, format!("at most {MAX_STEPS} steps")));
    }
    let ops: Vec<Op> = steps.iter().map(|v| op_of(cmd, v)).collect::<Result<_>>()?;
    let before: Vec<Handle> = all_handles(s.doc()?);
    for op in ops {
        base = apply(s, &objs, base, op)?;
    }
    let mut out = objs.clone();
    let d = s.doc()?;
    let known: std::collections::HashSet<Handle> = before.into_iter().collect();
    out.extend(all_handles(d).into_iter().filter(|h| !known.contains(h)));
    Ok(json!({ "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>(), "base": [base.x, base.y] }))
}

fn all_handles(d: &Drawing) -> Vec<Handle> {
    d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).map(|e| e.handle).collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Move,
    Copy,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Select,
    Base,
    Main,
    NewBase,
}

/// Typed MOCORO: select objects, a base point, then "Second point or [Copy/Rotate/Scale/Base/
/// Undo/eXit]" (the prompt follows the current mode) until Enter or eXit.
pub(super) struct MocoroM {
    sel: SelectPhase,
    stage: Stage,
    mode: Mode,
    objs: Vec<Handle>,
    base: Vec2,
    /// The drawing and base point before each operation, for Undo.
    history: Vec<(Arc<Drawing>, Vec2)>,
}

impl MocoroM {
    pub(super) fn new() -> Self {
        MocoroM { sel: SelectPhase::default(), stage: Stage::Select, mode: Mode::Move, objs: Vec::new(), base: Vec2::ZERO, history: Vec::new() }
    }

    fn keywords(&self) -> Vec<&'static str> {
        let mut k = Vec::new();
        for (m, name) in [(Mode::Move, "Move"), (Mode::Copy, "Copy"), (Mode::Rotate, "Rotate"), (Mode::Scale, "Scale")] {
            if m != self.mode {
                k.push(name);
            }
        }
        k.extend(["Base", "Undo", "eXit"]);
        k
    }
}

fn point_of(i: &Input) -> Option<Vec2> {
    match i {
        Input::Point(p) => Some(*p),
        Input::Deferred(d) => Some(d.at),
        _ => None,
    }
}

fn line(a: Vec2, b: Vec2) -> EntityKind {
    super::super::helpers::line(a, b)
}

impl Interactive for MocoroM {
    fn name(&self) -> &'static str {
        "MOCORO"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
            if self.objs.is_empty() {
                return Ok(Step::Done);
            }
            self.stage = Stage::Base;
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Select => self.sel.prompt(),
            Stage::Base => Prompt::new("Base point", Accept::POINT),
            Stage::NewBase => Prompt::new("New base point", Accept::POINT),
            Stage::Main => {
                let msg = match self.mode {
                    Mode::Move => "Second point or",
                    Mode::Copy => "Second point of copy or",
                    Mode::Rotate => "Rotation angle or",
                    Mode::Scale => "Scale factor or",
                };
                let accept = match self.mode {
                    Mode::Move | Mode::Copy => Accept::POINT,
                    Mode::Rotate | Mode::Scale => Accept::POINT_OR_NUMBER,
                };
                Prompt::new(msg, Accept { text: true, ..accept }).kw(&self.keywords()).default("eXit").base(self.base)
            }
        }
    }
    fn drag(&self, _s: &Session, c: Vec2) -> Option<crate::prompt::Drag> {
        if self.stage != Stage::Main {
            return None;
        }
        let b = self.base;
        let matrix = match self.mode {
            Mode::Move | Mode::Copy => Mat3::translate(c - b),
            Mode::Rotate => Mat3::rotate_about(b, b.angle_to(c)),
            Mode::Scale => {
                let f = b.dist(c);
                if !scale_ok(f) {
                    return None;
                }
                Mat3::scale_about(b, f)
            }
        };
        Some(crate::prompt::Drag { handles: self.objs.clone(), matrix, extra: vec![line(b, c)], clipboard: false })
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        match self.stage {
            Stage::Select => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    self.stage = Stage::Base;
                    Ok(Step::Continue)
                }
            },
            Stage::Base | Stage::NewBase => {
                let p = point_of(&i).ok_or_else(|| EngineError::Other("Requires a point.".into()))?;
                if self.stage == Stage::NewBase {
                    let snapshot = s.state()?.doc.clone();
                    self.history.push((snapshot, self.base));
                }
                self.base = p;
                self.stage = Stage::Main;
                Ok(Step::Continue)
            }
            Stage::Main => {
                if let Some(p) = point_of(&i) {
                    let op = match self.mode {
                        Mode::Move => Op::Move(p),
                        Mode::Copy => Op::Copy(p),
                        Mode::Rotate => Op::Rotate(self.base.angle_to(p)),
                        Mode::Scale => {
                            let f = self.base.dist(p);
                            if !scale_ok(f) {
                                return Err(EngineError::Other("Scale factor must be positive.".into()));
                            }
                            Op::Scale(f)
                        }
                    };
                    let before = self.base;
                    let snapshot = s.state()?.doc.clone();
                    self.base = apply(s, &self.objs, self.base, op)?;
                    self.history.push((snapshot, before));
                    return Ok(Step::Continue);
                }
                let t = match &i {
                    Input::Enter => return Ok(Step::Done),
                    Input::Text(t) | Input::Keyword(t) => t.trim().to_ascii_lowercase(),
                    _ => return Err(EngineError::Other("Point or option keyword required.".into())),
                };
                match t.as_str() {
                    "" | "x" | "exit" => Ok(Step::Done),
                    "m" | "move" => {
                        self.mode = Mode::Move;
                        Ok(Step::Continue)
                    }
                    "c" | "copy" => {
                        self.mode = Mode::Copy;
                        Ok(Step::Continue)
                    }
                    "r" | "rotate" => {
                        self.mode = Mode::Rotate;
                        Ok(Step::Continue)
                    }
                    "s" | "scale" => {
                        self.mode = Mode::Scale;
                        Ok(Step::Continue)
                    }
                    "b" | "base" => {
                        self.stage = Stage::NewBase;
                        Ok(Step::Continue)
                    }
                    "u" | "undo" => {
                        match self.history.pop() {
                            Some((doc, base)) => {
                                s.state_mut()?.doc = doc;
                                self.base = base;
                            }
                            None => s.echo("Nothing to undo."),
                        }
                        Ok(Step::Continue)
                    }
                    _ => {
                        // A typed angle or scale factor in those modes.
                        match self.mode {
                            Mode::Rotate => {
                                let a = s
                                    .angle_settings()
                                    .direction(&t)
                                    .ok_or_else(|| EngineError::Other("Requires an angle, a point or an option.".into()))?;
                                let snapshot = s.state()?.doc.clone();
                                let before = self.base;
                                self.base = apply(s, &self.objs, self.base, Op::Rotate(a))?;
                                self.history.push((snapshot, before));
                                Ok(Step::Continue)
                            }
                            Mode::Scale => {
                                let f = crate::units::parse_distance(&t)
                                    .filter(|f| scale_ok(*f))
                                    .ok_or_else(|| EngineError::Other("Requires a positive number, a point or an option.".into()))?;
                                let snapshot = s.state()?.doc.clone();
                                let before = self.base;
                                self.base = apply(s, &self.objs, self.base, Op::Scale(f))?;
                                self.history.push((snapshot, before));
                                Ok(Step::Continue)
                            }
                            Mode::Move | Mode::Copy => Err(EngineError::Other("Point or option keyword required.".into())),
                        }
                    }
                }
            }
        }
    }
}
