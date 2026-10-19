//! TCOUNT: number text objects in order: sorted by X (left to right), by Y (top to bottom) or in
//! the order they were selected, from a start number by an increment. The number overwrites the
//! text, goes before or after it, or replaces a string found in it.

use cadaddy_doc::{EntityKind, Handle};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most objects one call numbers.
const MAX_OBJECTS: usize = 100_000;
/// Largest start or increment (numbers stay exact integers).
const MAX_NUMBER: f64 = 1e12;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Sort {
    X,
    #[default]
    Y,
    Selection,
}

#[derive(Clone, Debug, PartialEq)]
enum Place {
    Overwrite,
    Prefix,
    Suffix,
    Find(String),
}

/// Where a text object is (its insertion point) and its shown string, for TEXT and MTEXT.
fn text_of(k: &EntityKind) -> Option<(Vec2, String, bool)> {
    match k {
        EntityKind::Text(t) => Some((t.justify_point(), t.value.clone(), false)),
        EntityKind::MText(t) => Some((t.insert.xy(), t.contents.clone(), true)),
        _ => None,
    }
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

fn numbered(old: &str, n: &str, place: &Place, mtext: bool) -> String {
    match place {
        Place::Overwrite => n.to_string(),
        Place::Prefix => format!("{n} {old}"),
        Place::Suffix => format!("{old} {n}"),
        Place::Find(f) if f.is_empty() => old.to_string(),
        // In MTEXT the number goes in as plain text: braces and backslashes can't appear in it.
        Place::Find(f) => old.replace(f.as_str(), n),
    }
    .chars()
    .filter(|c| !mtext || !matches!(c, '{' | '}'))
    .collect()
}

fn order(s: &Session, hs: &[Handle], sort: Sort) -> Result<Vec<(Handle, Vec2)>> {
    let d = s.doc()?;
    let mut v: Vec<(Handle, Vec2)> =
        hs.iter().take(MAX_OBJECTS).filter_map(|h| d.entity(*h).and_then(|e| text_of(&e.kind)).map(|(at, _, _)| (*h, at))).collect();
    match sort {
        Sort::X => v.sort_by(|a, b| a.1.x.total_cmp(&b.1.x).then(b.1.y.total_cmp(&a.1.y))),
        Sort::Y => v.sort_by(|a, b| b.1.y.total_cmp(&a.1.y).then(a.1.x.total_cmp(&b.1.x))),
        Sort::Selection => {}
    }
    Ok(v)
}

fn count(s: &mut Session, hs: &[Handle], sort: Sort, start: f64, step: f64, place: &Place) -> Result<Value> {
    if !(start.is_finite() && step.is_finite() && start.abs() <= MAX_NUMBER && step.abs() <= MAX_NUMBER) {
        return Err(other("The start number and increment must be numbers."));
    }
    let objs = order(s, hs, sort)?;
    if objs.is_empty() {
        return Err(other("No text or mtext selected."));
    }
    let mut out = Vec::new();
    let mut n = start;
    for (h, _) in objs {
        if super::super::curves::is_locked(s, h) {
            continue;
        }
        let label = number(n);
        n += step;
        let d = s.doc_mut()?;
        d.modify_entity(h, |e| match &mut e.kind {
            EntityKind::Text(t) => t.value = numbered(&t.value, &label, place, false),
            EntityKind::MText(t) => t.contents = numbered(&t.contents, &label, place, true),
            _ => {}
        })?;
        out.push(h.hex());
    }
    Ok(json!({ "handles": out }))
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let sort = match p.get("sort").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None => Sort::Y,
        Some("x") => Sort::X,
        Some("y") => Sort::Y,
        Some("select" | "selection") => Sort::Selection,
        Some(_) => return Err(bad("tcount", "`sort` is \"x\", \"y\" or \"select\"")),
    };
    let start = p.get("start").and_then(Value::as_f64).unwrap_or(1.0);
    let step = p.get("increment").and_then(Value::as_f64).unwrap_or(1.0);
    let place = match p.get("placement").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None | Some("prefix") => Place::Prefix,
        Some("overwrite") => Place::Overwrite,
        Some("suffix") => Place::Suffix,
        Some("find") => {
            let f = p
                .get("find")
                .and_then(Value::as_str)
                .filter(|f| !f.is_empty())
                .ok_or_else(|| bad("tcount", "`find` (the string to replace) is required"))?;
            if f.len() > 10_000 {
                return Err(bad("tcount", "`find` is too long"));
            }
            Place::Find(f.to_string())
        }
        Some(_) => return Err(bad("tcount", "`placement` is \"overwrite\", \"prefix\", \"suffix\" or \"find\"")),
    };
    count(s, &hs, sort, start, step, &place)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Select,
    Sort,
    Start,
    Place,
    Find,
}

#[derive(Default)]
pub(crate) struct TcountM {
    sel: SelectPhase,
    objs: Vec<Handle>,
    stage: Stage,
    sort: Sort,
    start: (f64, f64),
}

const KEYS: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

impl TcountM {
    fn finish(&mut self, s: &mut Session, place: Place) -> Result<Step> {
        let r = count(s, &self.objs.clone(), self.sort, self.start.0, self.start.1, &place)?;
        s.echo(format!("{} object(s) numbered.", r["handles"].as_array().map_or(0, Vec::len)));
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

impl Interactive for TcountM {
    fn name(&self) -> &'static str {
        "TCOUNT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.start = (1.0, 1.0);
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
            self.stage = Stage::Sort;
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Select => self.sel.prompt(),
            Stage::Sort => Prompt::new("Sort selected objects by", KEYS).kw(&["X", "Y", "Select-order"]).default("Y"),
            Stage::Start => Prompt::new("Specify starting number and increment (Start,increment)", Accept::TEXT).default("1,1"),
            Stage::Place => {
                Prompt::new("Placement of numbers in text", KEYS).kw(&["Overwrite", "Prefix", "Suffix", "Find&replace.."]).default("Prefix")
            }
            Stage::Find => Prompt::new("Enter search string", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        match self.stage {
            Stage::Select => match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Cancel),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    self.stage = Stage::Sort;
                }
            },
            Stage::Sort => {
                self.sort = match &i {
                    Input::Enter => Sort::Y,
                    Input::Keyword(k) if k == "X" => Sort::X,
                    Input::Keyword(k) if k == "Y" => Sort::Y,
                    Input::Keyword(_) => Sort::Selection,
                    _ => return Err(other("Invalid option keyword.")),
                };
                self.stage = Stage::Start;
            }
            Stage::Start => {
                if let Input::Text(t) = &i {
                    let mut parts = t.split(',').map(|x| x.trim().parse::<f64>());
                    let start = parts.next().and_then(|x| x.ok()).ok_or_else(|| other("Requires a starting number, or start,increment."))?;
                    let step = match parts.next() {
                        Some(Ok(x)) => x,
                        Some(Err(_)) => return Err(other("Requires a starting number, or start,increment.")),
                        None => 1.0,
                    };
                    if !(start.is_finite() && step.is_finite()) {
                        return Err(other("Requires numbers."));
                    }
                    self.start = (start, step);
                }
                self.stage = Stage::Place;
            }
            Stage::Place => {
                let place = match &i {
                    Input::Enter => Place::Prefix,
                    Input::Keyword(k) if k == "Overwrite" => Place::Overwrite,
                    Input::Keyword(k) if k == "Prefix" => Place::Prefix,
                    Input::Keyword(k) if k == "Suffix" => Place::Suffix,
                    Input::Keyword(_) => {
                        self.stage = Stage::Find;
                        return Ok(Step::Continue);
                    }
                    _ => return Err(other("Invalid option keyword.")),
                };
                return self.finish(s, place);
            }
            Stage::Find => match &i {
                Input::Text(t) if !t.is_empty() => return self.finish(s, Place::Find(t.clone())),
                _ => return Err(other("Requires a search string.")),
            },
        }
        Ok(Step::Continue)
    }
}

fn other(m: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(m.into())
}
