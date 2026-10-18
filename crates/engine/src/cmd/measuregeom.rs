//! MEASUREGEOM at the command line: pick what to measure (Distance by default), then the points or
//! the object it needs; each result is echoed and the option prompt comes back, as in AutoCAD.

use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Choose,
    Distance,
    Radius,
    Angle,
    Area,
}

#[derive(Default)]
pub(super) struct MeasureM {
    mode: Mode,
    points: Vec<Vec2>,
}

const OPTIONS: &[&str] = &["Distance", "Radius", "Angle", "ARea", "eXit"];

impl MeasureM {
    fn show(&mut self, s: &mut Session, cmd: &str, p: Value) -> Result<Step> {
        let r = s.execute(cmd, &p);
        self.mode = Mode::Choose;
        self.points.clear();
        let r = r?;
        match r.get("message").and_then(Value::as_str) {
            Some(m) => m.lines().for_each(|l| s.echo(l.to_string())),
            None => s.echo(r.to_string()),
        }
        Ok(Step::Continue)
    }
    fn pts(&self) -> Vec<[f64; 2]> {
        self.points.iter().map(|p| [p.x, p.y]).collect()
    }
}

impl Interactive for MeasureM {
    fn name(&self) -> &'static str {
        "MEASUREGEOM"
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        let n = self.points.len();
        let last = self.points.last().copied();
        match self.mode {
            Mode::Choose => Prompt::new("Enter an option", Accept::TEXT).kw(OPTIONS).default("Distance"),
            Mode::Distance if n == 0 => Prompt::new("Specify first point", Accept::POINT),
            Mode::Distance => Prompt::new("Specify second point", Accept::POINT).base_opt(last),
            Mode::Radius => Prompt::new("Select arc or circle", Accept::SELECT),
            Mode::Angle => Prompt::new(
                ["Specify angle vertex", "Specify first angle endpoint", "Specify second angle endpoint"].get(n).copied().unwrap_or(""),
                Accept::POINT,
            ),
            Mode::Area if n == 0 => Prompt::new("Specify first corner point", Accept::POINT),
            Mode::Area => Prompt::new("Specify next point or press Enter to total", Accept::POINT).base_opt(last),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        if self.mode == Mode::Choose {
            let k = match &i {
                Input::Enter => "distance".to_string(),
                Input::Keyword(k) | Input::Text(k) => k.trim().to_ascii_lowercase(),
                _ => return Ok(Step::Continue),
            };
            self.mode = match k.as_str() {
                "d" | "distance" => Mode::Distance,
                "r" | "radius" => Mode::Radius,
                "a" | "angle" => Mode::Angle,
                "ar" | "area" => Mode::Area,
                "x" | "exit" => return Ok(Step::Done),
                _ => {
                    s.echo("Invalid option keyword.");
                    Mode::Choose
                }
            };
            return Ok(Step::Continue);
        }
        match (self.mode, i) {
            (Mode::Radius, Input::Pick(hs)) => match hs.first() {
                Some(h) => self.show(s, "radius.measure", json!({ "handle": h.hex() })),
                None => Ok(Step::Continue),
            },
            (_, Input::Point(p)) => {
                self.points.push(p);
                match (self.mode, self.points.len()) {
                    (Mode::Distance, 2) => {
                        let pts = self.pts();
                        self.show(s, "dist", json!({ "p1": pts[0], "p2": pts[1] }))
                    }
                    (Mode::Angle, 3) => {
                        let pts = self.pts();
                        self.show(s, "angle.measure", json!({ "vertex": pts[0], "p1": pts[1], "p2": pts[2] }))
                    }
                    _ => Ok(Step::Continue),
                }
            }
            (Mode::Area, Input::Enter) if self.points.len() >= 3 => {
                let pts = self.pts();
                self.show(s, "area", json!({ "points": pts }))
            }
            (_, Input::Enter) => {
                self.mode = Mode::Choose;
                self.points.clear();
                Ok(Step::Continue)
            }
            (_, Input::Text(_) | Input::Keyword(_)) => {
                s.echo("Point or option keyword required.");
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }
}
