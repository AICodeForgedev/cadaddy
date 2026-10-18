//! BREAKLINE: a break line, a polyline from one point to another with a zigzag break symbol
//! along it, extended past both points.
//!
//! The symbol is our own: from the line it rises half the symbol size to one side, crosses to
//! the other and returns to the line, over the symbol's length. Only this built-in symbol is
//! available (a symbol from a drawing file is not).

use cadaddy_doc::EntityKind;
use cadaddy_geom::{PolyVertex, Vec2};
use serde_json::{Value, json};

use super::super::helpers::lwpoly;
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most a size or extension may be (drawing units).
const MAX_LEN: f64 = 1e9;

/// The default symbol size and extension: imperial 0.5 and 0.125, metric 12.5 and 3.
pub(super) fn defaults(s: &Session) -> (f64, f64) {
    let metric = s.doc().is_ok_and(|d| d.header.i64("MEASUREMENT", 0) == 1);
    if metric { (12.5, 3.0) } else { (0.5, 0.125) }
}

/// The break line's vertices from `a` to `b` with the symbol centred at the point of the line
/// nearest `at`. `None` when the points coincide or a value isn't usable.
pub(super) fn vertices(a: Vec2, b: Vec2, at: Vec2, size: f64, ext: f64) -> Option<Vec<Vec2>> {
    let len = a.dist(b);
    if !(len.is_finite() && len > 1e-12 && size.is_finite() && size > 0.0 && ext.is_finite() && ext >= 0.0 && at.is_finite()) {
        return None;
    }
    let u = (b - a) / len;
    let n = u.perp();
    let c = a + u * (at - a).dot(u);
    let (h, q) = (size / 2.0, size / 6.0);
    let pts = vec![a - u * ext, c - u * h, c - u * q + n * h, c + u * q - n * h, c + u * h, b + u * ext];
    pts.iter().all(|p| p.is_finite()).then_some(pts)
}

fn length(cmd: &str, p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && *x >= 0.0 && *x <= MAX_LEN)
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a number from 0 to {MAX_LEN}"))),
    }
}

/// `{p1, p2, at? (symbol location; default the midpoint), size?, extension?}` → `{handle}`.
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "breakline";
    let a = point_req(cmd, p, "p1")?;
    let b = point_req(cmd, p, "p2")?;
    let at = point_param(p, "at").unwrap_or_else(|| a.mid(b));
    let (ds, de) = defaults(s);
    let size = length(cmd, p, "size", ds)?;
    let ext = length(cmd, p, "extension", de)?;
    if size <= 0.0 {
        return Err(bad(cmd, "`size` must be more than 0"));
    }
    let pts = vertices(a, b, at, size, ext).ok_or_else(|| bad(cmd, "the two points must differ"))?;
    let h = s.add_entity(lwpoly(pts.into_iter().map(PolyVertex::new).collect(), false))?;
    Ok(json!({ "handle": h.hex() }))
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    First,
    Size,
    Extension,
    Second,
    Location,
}

/// Typed BREAKLINE: "Specify first point for breakline or [Block/Size/Extension]", the second
/// point, then the symbol's location (Enter: the midpoint).
pub(super) struct BreaklineM {
    stage: Stage,
    size: f64,
    ext: f64,
    a: Option<Vec2>,
    b: Option<Vec2>,
}

impl BreaklineM {
    pub(super) fn new(s: &Session) -> Self {
        let (size, ext) = defaults(s);
        BreaklineM { stage: Stage::First, size, ext, a: None, b: None }
    }

    fn finish(&self, s: &mut Session, at: Option<Vec2>) -> Result<Step> {
        let (Some(a), Some(b)) = (self.a, self.b) else { return Ok(Step::Done) };
        let at = at.unwrap_or_else(|| a.mid(b));
        run(s, &json!({ "p1": [a.x, a.y], "p2": [b.x, b.y], "at": [at.x, at.y], "size": self.size, "extension": self.ext }))?;
        Ok(Step::Done)
    }
}

fn point_of(i: &Input) -> Option<Vec2> {
    match i {
        Input::Point(p) => Some(*p),
        Input::Deferred(d) => Some(d.at),
        _ => None,
    }
}

fn fmt(v: f64) -> String {
    format!("{v:.4}")
}

impl Interactive for BreaklineM {
    fn name(&self) -> &'static str {
        "BREAKLINE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::First => {
                Prompt::new(format!("Size= {}, Extension= {}\nSpecify first point for breakline or", fmt(self.size), fmt(self.ext)), Accept::POINT)
                    .kw(&["Block", "Size", "Extension"])
            }
            Stage::Size => Prompt::new("Breakline symbol size", Accept::NUMBER).default(fmt(self.size)),
            Stage::Extension => Prompt::new("Breakline extension length", Accept::NUMBER).default(fmt(self.ext)),
            Stage::Second => Prompt::new("Specify second point for breakline", Accept::POINT).base_opt(self.a),
            Stage::Location => Prompt::new("Specify location for break symbol", Accept::POINT).default("Midpoint").base_opt(self.a),
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        let (a, b, at) = match (self.stage, self.a, self.b) {
            (Stage::Second, Some(a), _) => (a, c, a.mid(c)),
            (Stage::Location, Some(a), Some(b)) => (a, b, c),
            _ => return Vec::new(),
        };
        vertices(a, b, at, self.size, self.ext).map(|v| vec![lwpoly(v.into_iter().map(PolyVertex::new).collect(), false)]).unwrap_or_default()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        match self.stage {
            Stage::First => {
                if let Some(p) = point_of(&i) {
                    self.a = Some(p);
                    self.stage = Stage::Second;
                    return Ok(Step::Continue);
                }
                match &i {
                    Input::Enter => Ok(Step::Done),
                    Input::Text(t) | Input::Keyword(t) => match t.trim().to_ascii_lowercase().as_str() {
                        "b" | "block" => {
                            s.echo("Only the built-in break symbol is available.");
                            Ok(Step::Continue)
                        }
                        "s" | "size" => {
                            self.stage = Stage::Size;
                            Ok(Step::Continue)
                        }
                        "e" | "extension" => {
                            self.stage = Stage::Extension;
                            Ok(Step::Continue)
                        }
                        _ => Err(EngineError::Other("Point or option keyword required.".into())),
                    },
                    _ => Err(EngineError::Other("Point or option keyword required.".into())),
                }
            }
            Stage::Size | Stage::Extension => {
                let size = self.stage == Stage::Size;
                match &i {
                    Input::Enter => {}
                    Input::Text(t) => {
                        let v = crate::units::parse_distance(t.trim())
                            .filter(|v| v.is_finite() && *v <= MAX_LEN && if size { *v > 0.0 } else { *v >= 0.0 })
                            .ok_or_else(|| {
                                EngineError::Other(if size { "Requires a positive number." } else { "Requires a number of 0 or more." }.into())
                            })?;
                        if size {
                            self.size = v;
                        } else {
                            self.ext = v;
                        }
                    }
                    _ => return Err(EngineError::Other("Requires a number.".into())),
                }
                self.stage = Stage::First;
                Ok(Step::Continue)
            }
            Stage::Second => {
                let p = point_of(&i).ok_or_else(|| EngineError::Other("Requires a point.".into()))?;
                if self.a.is_some_and(|a| a.near(p, 1e-12)) {
                    return Err(EngineError::Other("The second point must differ from the first.".into()));
                }
                self.b = Some(p);
                self.stage = Stage::Location;
                Ok(Step::Continue)
            }
            Stage::Location => match &i {
                Input::Enter => self.finish(s, None),
                _ => {
                    let p = point_of(&i).ok_or_else(|| EngineError::Other("Requires a point.".into()))?;
                    self.finish(s, Some(p))
                }
            },
        }
    }
}
