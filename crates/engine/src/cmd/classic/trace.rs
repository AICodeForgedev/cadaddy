//! TRACE: a run of wide lines (traces) of the width TRACEWID, each segment a filled band whose
//! ends meet the next one's in a mitred corner. Each segment is its own object; FILL off shows
//! them as outlines.

use cadaddy_doc::{EntityKind, Solid};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::helpers::v3;
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most points one TRACE call takes.
const MAX_POINTS: usize = 100_000;
/// A corner sharper than this (the miter longer than this many widths) is cut square instead.
const MAX_MITER: f64 = 50.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("trace", "Trace", run_trace)
            .params(
                "{points: [[x,y],...] (2 or more), width?: (default TRACEWID; sets it)} → {handles} — one trace per segment, \
                 corners mitred",
            )
            .interactive(|_| Ok(Box::new(TraceM::default()))),
    ]
}

/// The current trace width (TRACEWID).
fn width_of(s: &Session) -> f64 {
    crate::sysvars::get(s, "TRACEWID").and_then(|v| v.as_f64()).filter(|w| w.is_finite() && *w >= 0.0).unwrap_or(0.05)
}

/// The two sides of the band at a vertex where segment `a→b` meets `b→c`: the offset lines'
/// meeting points (left, right); square ends when there is no next segment, the turn is
/// straight back, or the miter would be too long.
fn corner(a: Vec2, b: Vec2, c: Option<Vec2>, half: f64) -> (Vec2, Vec2) {
    let n1 = (b - a).normalized().perp() * half;
    let square = (b + n1, b - n1);
    let Some(c) = c else { return square };
    let n2 = (c - b).normalized().perp() * half;
    let left = cadaddy_geom::line_line_infinite(a + n1, b + n1, b + n2, c + n2);
    let right = cadaddy_geom::line_line_infinite(a - n1, b - n1, b - n2, c - n2);
    match (left, right) {
        (Some((l, _, _)), Some((r, _, _))) if l.dist(b) <= MAX_MITER * half.max(1e-12) && r.dist(b) <= MAX_MITER * half.max(1e-12) => (l, r),
        _ => square,
    }
}

/// The traces along `pts` (repeated points dropped): per segment its four corners in SOLID
/// order — start left, start right, end left, end right.
pub(crate) fn pieces(pts: &[Vec2], width: f64) -> Vec<[Vec2; 4]> {
    let pts = distinct_points(pts);
    let half = width / 2.0;
    let mut out = Vec::new();
    let mut start: Option<(Vec2, Vec2)> = None;
    for (i, w) in pts.windows(2).enumerate() {
        let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
        let (sl, sr) = start.unwrap_or_else(|| {
            let n = (b - a).normalized().perp() * half;
            (a + n, a - n)
        });
        let (el, er) = corner(a, b, pts.get(i + 2).copied(), half);
        out.push([sl, sr, el, er]);
        start = Some((el, er));
    }
    out
}

fn piece(c: [Vec2; 4]) -> EntityKind {
    let [a, b, cc, d] = c;
    EntityKind::Trace(Solid { corners: [v3(a), v3(b), v3(cc), v3(d)] })
}

fn run_trace(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param(p, "points").ok_or_else(|| bad("trace", "`points` [[x,y],...] (2 or more) is required"))?;
    if pts.len() > MAX_POINTS {
        return Err(bad("trace", format!("at most {MAX_POINTS} points")));
    }
    if distinct_points(&pts).len() < 2 {
        return Err(bad("trace", "a trace needs two different points"));
    }
    let width = match size_param("trace", p, "width", true)? {
        Some(w) => {
            s.doc_mut()?.header.set_f64("TRACEWID", w);
            w
        }
        None => width_of(s),
    };
    let mut hs = Vec::new();
    for c in pieces(&pts, width) {
        hs.push(s.add_entity(piece(c))?.hex());
    }
    Ok(json!({ "handles": hs }))
}

/// "Specify trace width <0.0500>:", "Specify start point:", "Specify next point:" … — a segment
/// is drawn once the point after it is known (its far corner depends on the next turn); Enter
/// finishes the last one square.
#[derive(Default)]
struct TraceM {
    width: Option<f64>,
    pts: Vec<Vec2>,
}

impl TraceM {
    /// Add the finished segment ending at the second-last point (its end mitred to the last).
    fn emit(&self, s: &mut Session, last: bool) -> Result<()> {
        let w = self.width.unwrap_or(0.05);
        let n = self.pts.len();
        let all = pieces(&self.pts, w);
        let want = if last { all.last() } else { all.get(n.saturating_sub(3)) };
        if let Some(c) = want {
            s.add_entity(piece(*c))?;
        }
        Ok(())
    }
}

impl Interactive for TraceM {
    fn name(&self) -> &'static str {
        "TRACE"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match (self.width, self.pts.last()) {
            (None, _) => Prompt::new("Specify trace width", Accept::POINT_OR_NUMBER).default(format!("{:.4}", width_of(s))),
            (Some(_), None) => Prompt::new("Specify start point", Accept::POINT),
            (Some(_), Some(&l)) => Prompt::new("Specify next point", Accept::POINT).base(l),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.width.is_none() {
            let w = match i {
                Input::Enter => Some(width_of(s)),
                Input::Text(t) => t.trim().parse::<f64>().ok().filter(|w| w.is_finite() && *w >= 0.0),
                Input::Cancel => return Ok(Step::Cancel),
                _ => None,
            };
            if let Some(w) = w {
                s.doc_mut()?.header.set_f64("TRACEWID", w);
                self.width = Some(w);
            }
            return Ok(Step::Continue);
        }
        match i {
            Input::Point(p) if p.is_finite() => {
                if self.pts.last().is_some_and(|l| l.near(p, 1e-12)) {
                    return Ok(Step::Continue);
                }
                self.pts.push(p);
                if self.pts.len() >= 3 {
                    self.emit(s, false)?;
                }
                Ok(Step::Continue)
            }
            Input::Enter | Input::Cancel => {
                if self.pts.len() >= 2 {
                    self.emit(s, true)?;
                }
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let (Some(w), Some(_)) = (self.width, self.pts.last()) else { return Vec::new() };
        let n = self.pts.len();
        let mut pts: Vec<Vec2> = self.pts.iter().skip(n.saturating_sub(2)).copied().collect();
        pts.push(cursor);
        pieces(&pts, w).into_iter().map(piece).collect()
    }
}
