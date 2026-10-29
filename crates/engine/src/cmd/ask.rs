//! Command-line prompts for commands that only had a JSON form: each step asks for one parameter
//! (a point, a number, text, a selection or one object), then the command runs with them, so a
//! typed `LTSCALE`, `LIMITS` or `LAYISO` prompts as in AutoCAD instead of reporting a missing
//! parameter.

use serde_json::{Map, Value, json};

use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step, units};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Point,
    Number,
    /// A number whose absolute value is at most this; a larger one is reported and asked again.
    Bounded(f64),
    Text,
    /// Objects until Enter → `handles`.
    Select,
    /// One object → `handle`.
    One,
    /// A layer: an object picked on it, or the name of one that exists.
    Layer,
    /// One of these words (any case) → the lower-case word.
    Keyword(&'static [&'static str]),
    /// A point in space typed `x,y,z` → `[x, y, z]`.
    Point3,
}

/// One prompt: the parameter it fills, its message and kind, and the current value Enter keeps.
#[derive(Clone, Copy)]
pub(crate) struct Ask {
    pub key: &'static str,
    pub message: &'static str,
    pub kind: Kind,
    pub default: Option<fn(&Session) -> Option<String>>,
}

pub(crate) const fn ask(key: &'static str, message: &'static str, kind: Kind) -> Ask {
    Ask { key, message, kind, default: None }
}

pub(crate) const fn ask_or(key: &'static str, message: &'static str, kind: Kind, default: fn(&Session) -> Option<String>) -> Ask {
    Ask { key, message, kind, default: Some(default) }
}

/// The interactive form of `cmd`: `steps` in order, then the command.
pub(crate) fn machine<const N: usize>(cmd: &'static str, name: &'static str, steps: [Ask; N]) -> Box<dyn Interactive> {
    machine_of(cmd, name, steps.to_vec())
}

/// [`machine`] for a list of steps known only at run time.
pub(crate) fn machine_of(cmd: &'static str, name: &'static str, steps: Vec<Ask>) -> Box<dyn Interactive> {
    Box::new(Asker { cmd, name, steps, at: 0, params: Map::new(), picked: Vec::new() })
}

struct Asker {
    cmd: &'static str,
    name: &'static str,
    steps: Vec<Ask>,
    at: usize,
    params: Map<String, Value>,
    picked: Vec<String>,
}

fn point_text(t: &str) -> Option<Value> {
    let mut it = t.split(',').map(|v| units::parse_distance(v.trim()));
    let (Some(Some(x)), Some(Some(y))) = (it.next(), it.next()) else { return None };
    (x.is_finite() && y.is_finite()).then(|| json!([x, y]))
}

impl Asker {
    fn step(&self) -> Option<&Ask> {
        self.steps.get(self.at)
    }

    /// Take `v` for the current step; after the last one, run the command.
    fn take(&mut self, s: &mut Session, v: Value) -> Result<Step> {
        let Some(a) = self.step() else { return Ok(Step::Done) };
        self.params.insert(a.key.into(), v);
        self.at += 1;
        if self.at < self.steps.len() {
            return Ok(Step::Continue);
        }
        let spec = super::find_command(self.cmd).ok_or_else(|| EngineError::UnknownCommand(self.cmd.into()))?;
        let r = match (spec.run)(s, &Value::Object(self.params.clone())) {
            Ok(r) => r,
            Err(e) => {
                // Refused: ask for the last value again (the session shows why).
                self.at -= 1;
                return Err(e);
            }
        };
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            for l in m.lines() {
                s.echo(l.to_string());
            }
        } else if let Some(v) = r.get("value") {
            s.echo(match v {
                Value::String(t) => format!("\"{t}\""),
                other => other.to_string(),
            });
        }
        Ok(Step::Done)
    }
}

impl Interactive for Asker {
    fn name(&self) -> &'static str {
        self.name
    }

    fn prompt(&self, s: &Session) -> Prompt {
        let Some(a) = self.step() else { return Prompt::new("", Accept::TEXT) };
        let accept = match a.kind {
            Kind::Point => Accept::POINT,
            Kind::Number | Kind::Bounded(_) => Accept::NUMBER,
            Kind::Text => Accept::TEXT,
            Kind::Select | Kind::One | Kind::Layer => Accept::SELECT,
            Kind::Keyword(_) => Accept::TEXT,
            Kind::Point3 => Accept { point: true, ..Accept::TEXT },
        };
        // A keyword question lists its answers in the message: "Enter an option [ON/OFF] <ON>:".
        let p = match a.kind {
            Kind::Keyword(k) => Prompt::new(format!("{} [{}]", a.message, k.join("/")), accept).kw(k),
            Kind::Point3 => Prompt::new(a.message, accept).point3(),
            _ => Prompt::new(a.message, accept),
        };
        match a.default.and_then(|d| d(s)) {
            Some(d) => p.default(d),
            None => p,
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Some(a) = self.step().copied() else { return Ok(Step::Done) };
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let default = a.default.and_then(|d| d(s));
        match (a.kind, i) {
            (Kind::Select, Input::Pick(hs)) => {
                let mut have: std::collections::HashSet<String> = self.picked.iter().cloned().collect();
                for h in hs {
                    let h = h.hex();
                    if have.insert(h.clone()) {
                        self.picked.push(h);
                    }
                }
                Ok(Step::Continue)
            }
            (Kind::Select, Input::Enter) => {
                if self.picked.is_empty() {
                    return Ok(Step::Done);
                }
                let hs = std::mem::take(&mut self.picked);
                self.take(s, json!(hs))
            }
            (Kind::One, Input::Pick(hs)) => match hs.first() {
                Some(h) => self.take(s, json!(h.hex())),
                None => Ok(Step::Continue),
            },
            (Kind::One, Input::Enter) => Ok(Step::Done),
            (Kind::Layer, Input::Pick(hs)) => {
                let layer = hs.first().and_then(|h| s.doc().ok()?.entity(*h).map(|e| e.common.layer.clone()));
                match layer {
                    Some(l) => self.take(s, json!(l)),
                    None => Ok(Step::Continue),
                }
            }
            (Kind::Layer, Input::Text(t) | Input::Keyword(t)) => {
                let name = t.trim();
                match s.doc()?.layer(name).map(|l| l.name.clone()) {
                    Some(l) => self.take(s, json!(l)),
                    None => {
                        s.echo(format!("Layer \"{name}\" not found."));
                        Ok(Step::Continue)
                    }
                }
            }
            (Kind::Layer, Input::Enter) => Ok(Step::Done),
            (Kind::Keyword(words), Input::Text(t) | Input::Keyword(t)) => match words.iter().find(|w| w.eq_ignore_ascii_case(t.trim())) {
                Some(w) => self.take(s, json!(w.to_ascii_lowercase())),
                None => {
                    s.echo(format!("Invalid option keyword. Enter one of: {}.", words.join(", ")));
                    Ok(Step::Continue)
                }
            },
            (Kind::Keyword(words), Input::Enter) => match default.as_deref().and_then(|d| words.iter().find(|w| w.eq_ignore_ascii_case(d))) {
                Some(w) => self.take(s, json!(w.to_ascii_lowercase())),
                None => Ok(Step::Done),
            },
            (Kind::Point3, Input::Point3(p)) => self.take(s, json!([p.x, p.y, p.z])),
            (Kind::Point3, Input::Point(p)) => self.take(s, json!([p.x, p.y, 0.0])),
            (Kind::Point3, Input::Text(t) | Input::Keyword(t)) => match crate::prompt::parse_point3(&t, cadaddy_geom::Vec3::new(0.0, 0.0, 0.0)) {
                Some(p) => self.take(s, json!([p.x, p.y, p.z])),
                None => {
                    s.echo("Requires a point: x,y,z.");
                    Ok(Step::Continue)
                }
            },
            (Kind::Point3, Input::Enter) => Ok(Step::Done),
            (Kind::Point, Input::Point(p)) => self.take(s, json!([p.x, p.y])),
            (Kind::Point, Input::Text(t) | Input::Keyword(t)) => match point_text(&t) {
                Some(v) => self.take(s, v),
                None => {
                    s.echo("Invalid point.");
                    Ok(Step::Continue)
                }
            },
            (Kind::Point, Input::Enter) => match default.as_deref().and_then(point_text) {
                Some(v) => self.take(s, v),
                None => Ok(Step::Done),
            },
            (Kind::Number, Input::Text(t) | Input::Keyword(t)) => match units::parse_distance(t.trim()).filter(|v| v.is_finite()) {
                Some(v) => self.take(s, json!(v)),
                None => {
                    s.echo("Requires a number.");
                    Ok(Step::Continue)
                }
            },
            (Kind::Bounded(max), Input::Text(t) | Input::Keyword(t)) => match t.trim().parse::<f64>().ok().or_else(|| units::parse_distance(t.trim())).filter(|v| v.is_finite()) {
                Some(v) if v.abs() <= max => self.take(s, json!(v)),
                Some(_) => {
                    s.echo(format!("Requires a value between -{max:e} and {max:e}."));
                    Ok(Step::Continue)
                }
                None => {
                    s.echo("Requires a number.");
                    Ok(Step::Continue)
                }
            },
            (Kind::Number | Kind::Bounded(_), Input::Enter) => match default.as_deref().and_then(|d| d.trim().parse::<f64>().ok()) {
                Some(v) => self.take(s, json!(v)),
                None => Ok(Step::Done),
            },
            (Kind::Text, Input::Text(t) | Input::Keyword(t)) if !t.trim().is_empty() => self.take(s, json!(t.trim())),
            (Kind::Text, Input::Text(_) | Input::Enter) => match default {
                Some(d) => self.take(s, json!(d)),
                None => Ok(Step::Done),
            },
            _ => Ok(Step::Continue),
        }
    }
}
