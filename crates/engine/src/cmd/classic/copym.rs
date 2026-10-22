//! COPYM (Express): copy objects many times from one base point — to each point picked, again by
//! the last displacement (Repeat), a number of copies spread evenly up to a point (Divide), a
//! copy every so far towards a point (Measure), or a grid whose spacing is the first copy's
//! displacement and whose size reaches a corner (Array).

use cadaddy_doc::Handle;
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::modify::transform_entities;
use super::super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Most copies one COPYM makes (all modes).
const MAX_COPIES: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("copym", "Copy Multiple", run_copym)
            .params(
                "{handles?, base: [x,y], mode?: points (default) | repeat | divide | measure | array, to: [x,y] (points: also \
                 [[x,y],...]), count? (repeat: copies by the displacement; divide: copies spread up to `to`), spacing? (measure), \
                 corner?: [x,y] | rows?, columns? (array: `to` gives the column and row spacing)} → {handles, copies}",
            )
            .interactive(|_| Ok(Box::new(CopymM::default()))),
    ]
}

/// The displacements of the copies a mode makes, from the base point.
fn displacements(mode: &str, base: Vec2, p: &Value) -> Result<Vec<Vec2>> {
    let cmd = "copym";
    let count = |key: &str| -> Result<usize> {
        let n = p.get(key).and_then(Value::as_u64).filter(|n| *n >= 1).ok_or_else(|| bad(cmd, format!("`{key}` (1 or more) is required")))?;
        usize::try_from(n).ok().filter(|n| *n <= MAX_COPIES).ok_or_else(|| bad(cmd, format!("at most {MAX_COPIES} copies")))
    };
    let to = || point_req(cmd, p, "to");
    let out: Vec<Vec2> = match mode {
        "points" | "" => {
            let pts = points_param(p, "to")
                .or_else(|| point_param(p, "to").map(|q| vec![q]))
                .ok_or_else(|| bad(cmd, "`to` (a point or points) is required"))?;
            pts.into_iter().map(|q| q - base).collect()
        }
        "repeat" => {
            let d = to()? - base;
            (1..=count("count")?).map(|k| d * k as f64).collect()
        }
        "divide" => {
            let d = to()? - base;
            let n = count("count")?;
            (1..=n).map(|k| d * (k as f64 / n as f64)).collect()
        }
        "measure" => {
            let d = to()? - base;
            let step = size_param(cmd, p, "spacing", false)?.ok_or_else(|| bad(cmd, "`spacing` is required"))?;
            let len = d.len();
            if len < 1e-12 {
                return Err(bad(cmd, "`to` is the base point"));
            }
            let n = (len / step + 1e-9).floor();
            if !(n.is_finite() && n <= MAX_COPIES as f64) {
                return Err(bad(cmd, format!("at most {MAX_COPIES} copies")));
            }
            let u = d / len;
            (1..=n as usize).map(|k| u * (step * k as f64)).collect()
        }
        "array" => {
            let d = to()? - base;
            let (cols, rows) = match point_param(p, "corner") {
                Some(c) => {
                    let span = c - base;
                    let fit = |span: f64, step: f64| if step.abs() < 1e-12 { 1.0 } else { ((span / step) + 1e-9).floor().max(0.0) + 1.0 };
                    (fit(span.x, d.x), fit(span.y, d.y))
                }
                None => (count("columns")? as f64, count("rows")? as f64),
            };
            if !(cols.is_finite() && rows.is_finite() && cols * rows <= MAX_COPIES as f64 + 1.0) {
                return Err(bad(cmd, format!("at most {MAX_COPIES} copies")));
            }
            let mut v = Vec::new();
            for r in 0..rows as usize {
                for c in 0..cols as usize {
                    if r + c > 0 {
                        v.push(Vec2::new(d.x * c as f64, d.y * r as f64));
                    }
                }
            }
            v
        }
        m => return Err(bad(cmd, format!("unknown mode `{m}` (points, repeat, divide, measure or array)"))),
    };
    if out.len() > MAX_COPIES {
        return Err(bad(cmd, format!("at most {MAX_COPIES} copies")));
    }
    if out.iter().any(|d| !d.is_finite()) {
        return Err(bad(cmd, "the copies' positions must be finite"));
    }
    Ok(out)
}

/// Copy `hs` by each displacement: the new handles.
fn copy_by(s: &mut Session, hs: &[Handle], ds: &[Vec2]) -> Result<Vec<Handle>> {
    let mut out = Vec::new();
    for d in ds {
        out.extend(transform_entities(s, hs, &Mat3::translate(*d), true)?);
    }
    Ok(out)
}

fn run_copym(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("copym", "no objects selected"));
    }
    let base = point_req("copym", p, "base")?;
    let mode = str_param(p, "mode").unwrap_or("points").trim().to_ascii_lowercase();
    let ds = displacements(&mode, base, p)?;
    if ds.len().saturating_mul(hs.len()) > MAX_COPIES * 10 {
        return Err(EngineError::Other(format!("too many objects to copy ({} × {})", hs.len(), ds.len())));
    }
    let out = copy_by(s, &hs, &ds)?;
    Ok(json!({ "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>(), "copies": ds.len() }))
}

/// What COPYM asks next.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Ask {
    #[default]
    Select,
    Base,
    Next,
    /// Divide / Measure / Array: the second point, then their number.
    Second(Sub),
    Count(Sub, Vec2),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sub {
    Divide,
    Measure,
    Array,
}

/// "Select objects:", "Base point:", then "Second point or [Repeat (last)/Divide/Measure/Array
/// (dynamic)] <exit>:" until Enter.
#[derive(Default)]
struct CopymM {
    ask: Ask,
    sel: SelectPhase,
    hs: Vec<Handle>,
    base: Vec2,
    /// The last copy's displacement from the base, and the step Repeat adds to it (the last
    /// picked point's displacement from the copy before it).
    last: Option<Vec2>,
    step: Option<Vec2>,
}

impl CopymM {
    fn copy(&mut self, s: &mut Session, ds: &[Vec2]) -> Result<()> {
        copy_by(s, &self.hs, ds)?;
        if let Some(d) = ds.last() {
            let before = if ds.len() >= 2 { ds.get(ds.len() - 2).copied() } else { self.last };
            self.step = Some(*d - before.unwrap_or(Vec2::ZERO));
            self.last = Some(*d);
        }
        self.ask = Ask::Next;
        Ok(())
    }
    fn picked(&mut self, hs: Vec<Handle>) -> Step {
        if hs.is_empty() {
            return Step::Cancel;
        }
        self.hs = hs;
        self.ask = Ask::Base;
        Step::Continue
    }
}

impl Interactive for CopymM {
    fn name(&self) -> &'static str {
        "COPYM"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return Ok(self.picked(hs));
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.ask {
            Ask::Select => self.sel.prompt(),
            Ask::Base => Prompt::new("Base point", Accept::POINT),
            Ask::Next => {
                // The options read as in other CAD programs; the keywords are their plain words (a
                // whole "(last)" typed would be read as an AutoLISP expression).
                Prompt::new("Second point or [Repeat (last)/Divide/Measure/Array (dynamic)]", Accept::POINT)
                    .kw(&["Repeat", "Divide", "Measure", "Array"])
                    .base(self.base)
                    .default("exit")
            }
            Ask::Second(Sub::Array) => Prompt::new("Specify second point (the spacing)", Accept::POINT).base(self.base),
            Ask::Second(_) => Prompt::new("Specify second point", Accept::POINT).base(self.base),
            Ask::Count(Sub::Divide, _) => Prompt::new("Number of copies", Accept::NUMBER),
            Ask::Count(Sub::Measure, to) => Prompt::new("Specify spacing", Accept::POINT_OR_NUMBER).base(to),
            Ask::Count(Sub::Array, to) => Prompt::new("Specify array corner", Accept::POINT).base(to),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(if self.ask == Ask::Select || self.ask == Ask::Base { Step::Cancel } else { Step::Done });
        }
        let num = |t: &str| t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0);
        let base = self.base;
        match (self.ask, i) {
            (Ask::Select, i) => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Cancel),
                SelOutcome::Done(hs) => Ok(self.picked(hs)),
            },
            (Ask::Base, Input::Point(p)) => {
                self.base = p;
                self.ask = Ask::Next;
                Ok(Step::Continue)
            }
            (Ask::Base, _) => Ok(Step::Continue),
            (Ask::Next, Input::Point(p)) => {
                self.copy(s, &[p - base])?;
                Ok(Step::Continue)
            }
            (Ask::Next, Input::Enter) => Ok(Step::Done),
            (Ask::Next, Input::Keyword(k) | Input::Text(k)) => {
                match k.trim().to_ascii_lowercase().chars().next() {
                    Some('r') => match (self.last, self.step) {
                        // The next copy one more step on from the last.
                        (Some(l), Some(st)) => self.copy(s, &[l + st])?,
                        _ => s.echo("Nothing to repeat yet."),
                    },
                    Some('d') => self.ask = Ask::Second(Sub::Divide),
                    Some('m') => self.ask = Ask::Second(Sub::Measure),
                    Some('a') => self.ask = Ask::Second(Sub::Array),
                    _ => s.echo("Point or option keyword required."),
                }
                Ok(Step::Continue)
            }
            (Ask::Second(sub), Input::Point(p)) => {
                self.ask = Ask::Count(sub, p);
                Ok(Step::Continue)
            }
            (Ask::Count(Sub::Divide, to), Input::Text(t)) => {
                match num(&t).filter(|n| n.fract() == 0.0 && *n <= MAX_COPIES as f64) {
                    Some(n) => {
                        let ds = displacements("divide", base, &json!({ "to": [to.x, to.y], "count": n as u64 }))?;
                        self.copy(s, &ds)?;
                    }
                    None => s.echo(format!("Enter a whole number from 1 to {MAX_COPIES}.")),
                }
                Ok(Step::Continue)
            }
            (Ask::Count(Sub::Measure, to), i) => {
                let step = match i {
                    Input::Text(t) => num(&t),
                    Input::Point(p) => Some(p.dist(to)).filter(|d| *d > 1e-12),
                    _ => None,
                };
                if let Some(step) = step {
                    let ds = displacements("measure", base, &json!({ "to": [to.x, to.y], "spacing": step }))?;
                    self.copy(s, &ds)?;
                }
                Ok(Step::Continue)
            }
            (Ask::Count(Sub::Array, to), Input::Point(c)) => {
                let ds = displacements("array", base, &json!({ "to": [to.x, to.y], "corner": [c.x, c.y] }))?;
                self.copy(s, &ds)?;
                Ok(Step::Continue)
            }
            (Ask::Second(_) | Ask::Count(..), Input::Enter) => {
                self.ask = Ask::Next;
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, s: &Session, cursor: Vec2) -> Vec<cadaddy_doc::EntityKind> {
        if self.ask != Ask::Next {
            return Vec::new();
        }
        let Ok(d) = s.doc() else { return Vec::new() };
        let m = Mat3::translate(cursor - self.base);
        self.hs
            .iter()
            .take(1000)
            .filter_map(|h| d.entity(*h))
            .map(|e| {
                let mut k = e.kind.clone();
                k.transform(&m);
                k
            })
            .collect()
    }
}
