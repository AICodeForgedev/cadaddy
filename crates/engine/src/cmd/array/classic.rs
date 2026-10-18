//! -ARRAY: AutoCAD's classic command-line array. Select objects, Rectangular or Polar, then the
//! rows, columns and distances, or the centre, item count, angle to fill and rotation. It makes
//! plain copies (no associative array), as the classic command always did.

use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase, number};
use super::{run_arraypolar, run_arrayrect};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_doc::Handle;
use cadaddy_geom::Vec2;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ask {
    Type,
    Rows,
    Cols,
    RowDist,
    ColDist,
    Center,
    Items,
    Fill,
    Rotate,
}

pub(super) struct ClassicM {
    sel: SelectPhase,
    objs: Vec<Handle>,
    ask: Ask,
    rows: u64,
    cols: u64,
    row_dist: Option<f64>,
    /// First corner of a unit cell given for the row distance.
    cell_from: Option<Vec2>,
    center: Vec2,
    items: u64,
    fill: f64,
}

impl Default for ClassicM {
    fn default() -> Self {
        ClassicM {
            sel: SelectPhase::default(),
            objs: Vec::new(),
            ask: Ask::Type,
            rows: 1,
            cols: 1,
            row_dist: None,
            cell_from: None,
            center: Vec2::ZERO,
            items: 0,
            fill: 360.0,
        }
    }
}

fn handles(objs: &[Handle]) -> Value {
    json!(objs.iter().map(|h| h.hex()).collect::<Vec<_>>())
}

impl ClassicM {
    fn rect(&self, s: &mut Session, row_dist: f64, col_dist: f64) -> Result<Step> {
        if self.rows * self.cols > 1 {
            let p = json!({ "handles": handles(&self.objs), "rows": self.rows, "cols": self.cols, "rowSpacing": row_dist, "colSpacing": col_dist, "associative": false });
            run_arrayrect(s, &p)?;
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
    fn polar(&self, s: &mut Session, rotate: bool) -> Result<Step> {
        let p = json!({ "handles": handles(&self.objs), "center": [self.center.x, self.center.y], "count": self.items, "angle": self.fill, "rotate": rotate, "associative": false });
        run_arraypolar(s, &p)?;
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
    /// After the columns (or the row distance): ask for what is still needed, else make the array.
    fn after_counts(&mut self, s: &mut Session) -> Result<Step> {
        match (self.rows > 1, self.cols > 1, self.row_dist) {
            (true, _, None) => {
                self.ask = Ask::RowDist;
                Ok(Step::Continue)
            }
            (_, true, _) => {
                self.ask = Ask::ColDist;
                Ok(Step::Continue)
            }
            _ => self.rect(s, self.row_dist.unwrap_or(0.0), 0.0),
        }
    }
}

fn count(t: &str, min: u64) -> Result<u64> {
    number(t)
        .filter(|n| n.fract() == 0.0 && *n >= min as f64 && *n <= 100_000.0)
        .map(|n| n as u64)
        .ok_or_else(|| EngineError::Other(format!("Requires an integer of at least {min}.")))
}

fn distance(t: &str) -> Result<f64> {
    number(t).filter(|v| v.is_finite()).ok_or_else(|| EngineError::Other("Requires a distance.".into()))
}

impl Interactive for ClassicM {
    fn name(&self) -> &'static str {
        "-ARRAY"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.sel.done {
            return self.sel.prompt();
        }
        match self.ask {
            Ask::Type => Prompt::new("Enter the type of array", Accept::TEXT).kw(&["Rectangular", "Polar"]).default("R"),
            Ask::Rows => Prompt::new("Enter the number of rows (---)", Accept::NUMBER).default("1"),
            Ask::Cols => Prompt::new("Enter the number of columns (|||)", Accept::NUMBER).default("1"),
            Ask::RowDist if self.cell_from.is_some() => Prompt::new("Specify other corner", Accept::POINT).base_opt(self.cell_from),
            Ask::RowDist => Prompt::new("Enter the distance between rows or specify unit cell (---)", Accept::POINT_OR_NUMBER),
            Ask::ColDist => Prompt::new("Specify the distance between columns (|||)", Accept::NUMBER),
            Ask::Center => Prompt::new("Specify center point of array", Accept::POINT),
            Ask::Items => Prompt::new("Enter the number of items in the array", Accept::NUMBER),
            Ask::Fill => Prompt::new("Specify the angle to fill (+=ccw, -=cw)", Accept::NUMBER).default("360"),
            Ask::Rotate => Prompt::new("Rotate arrayed objects?", Accept::TEXT).kw(&["Yes", "No"]).default("Y"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.sel.done {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    Ok(Step::Continue)
                }
            };
        }
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let word = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            _ => String::new(),
        };
        match (self.ask, i) {
            (Ask::Type, Input::Enter | Input::Text(_) | Input::Keyword(_)) => {
                self.ask = match word.to_ascii_lowercase().chars().next() {
                    None | Some('r') => Ask::Rows,
                    Some('p') => Ask::Center,
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
            }
            (Ask::Rows, Input::Enter) => self.ask = Ask::Cols,
            (Ask::Rows, Input::Text(_)) => {
                self.rows = count(&word, 1)?;
                self.ask = Ask::Cols;
            }
            (Ask::Cols, Input::Enter) => return self.after_counts(s),
            (Ask::Cols, Input::Text(_)) => {
                self.cols = count(&word, 1)?;
                if self.rows * self.cols == 1 {
                    s.echo("A one-element array is not made.");
                    return Ok(Step::Done);
                }
                return self.after_counts(s);
            }
            (Ask::RowDist, Input::Point(p)) => match self.cell_from.take() {
                // Unit cell: its width is the column distance, its height the row distance.
                Some(a) => {
                    self.row_dist = Some(p.y - a.y);
                    return self.rect(s, p.y - a.y, p.x - a.x);
                }
                None => self.cell_from = Some(p),
            },
            (Ask::RowDist, Input::Text(_)) => {
                self.row_dist = Some(distance(&word)?);
                return self.after_counts(s);
            }
            (Ask::ColDist, Input::Text(_)) => return self.rect(s, self.row_dist.unwrap_or(0.0), distance(&word)?),
            (Ask::Center, Input::Point(p)) => {
                self.center = p;
                self.ask = Ask::Items;
            }
            (Ask::Items, Input::Text(_)) => {
                self.items = count(&word, 2)?;
                self.ask = Ask::Fill;
            }
            (Ask::Fill, Input::Enter) => self.ask = Ask::Rotate,
            (Ask::Fill, Input::Text(_)) => {
                self.fill = number(&word)
                    .filter(|a| a.is_finite() && *a != 0.0 && a.abs() <= 360.0)
                    .ok_or_else(|| EngineError::Other("Requires an angle between -360 and 360, not 0.".into()))?;
                self.ask = Ask::Rotate;
            }
            (Ask::Rotate, Input::Enter) => return self.polar(s, true),
            (Ask::Rotate, Input::Text(_) | Input::Keyword(_)) => {
                let rotate = match word.to_ascii_lowercase().chars().next() {
                    Some('y') => true,
                    Some('n') => false,
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
                return self.polar(s, rotate);
            }
            _ => {}
        }
        Ok(Step::Continue)
    }
}

#[cfg(test)]
mod tests {
    use cadaddy_doc::EntityKind;

    use crate::Session;

    fn circles(s: &Session) -> Vec<(f64, f64)> {
        let r = |v: f64| (v * 1e6).round() / 1e6 + 0.0;
        let mut v: Vec<(f64, f64)> = s
            .doc()
            .unwrap()
            .model
            .iter()
            .filter_map(|e| if let EntityKind::Circle(c) = &e.kind { Some((r(c.center.x), r(c.center.y))) } else { None })
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    }

    #[test]
    fn rectangular_and_polar_make_plain_copies() {
        let mut s = Session::new();
        s.cmdline("CIRCLE 0,0 0.5").unwrap();
        s.script("-ARRAY 0.5,0\n\nR 2 3 10 5\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        assert_eq!(circles(&s), vec![(0.0, 0.0), (0.0, 10.0), (5.0, 0.0), (5.0, 10.0), (10.0, 0.0), (10.0, 10.0)]);
        assert!(s.doc().unwrap().model.iter().all(|e| matches!(e.kind, EntityKind::Circle(_))), "no associative array");

        let mut s = Session::new();
        s.cmdline("CIRCLE 10,0 0.5").unwrap();
        s.script("-ARRAY 10.5,0\n\nP 0,0 4\n\n\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        assert_eq!(circles(&s), vec![(-10.0, 0.0), (0.0, -10.0), (0.0, 10.0), (10.0, 0.0)]);
    }
}
