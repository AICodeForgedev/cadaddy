//! SOLID (2D Solid): filled triangles and quadrilaterals. The points go in AutoCAD's order —
//! first, second, then third and fourth across from them — and each further pair makes the next
//! piece with the last two.

use cadaddy_doc::{EntityKind, Solid};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::helpers::v3;
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most points one SOLID call takes.
const MAX_POINTS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("solid", "2D Solid", run_solid)
            .alias(&["so"])
            .params(
                "{points: [[x,y],...] (3 or more: first, second, third, fourth?; every further two points make the next piece with \
                 the last two; an odd last point makes a triangle)} → {handles}",
            )
            .interactive(|_| Ok(Box::new(SolidM::default()))),
    ]
}

fn piece(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> EntityKind {
    EntityKind::Solid(Solid { corners: [v3(a), v3(b), v3(c), v3(d)] })
}

/// The pieces of a SOLID point run: (1,2,3,4), then (3,4,5,6), … ; a last single point makes a
/// triangle (its fourth corner the same as its third).
pub(crate) fn pieces(pts: &[Vec2]) -> Vec<[Vec2; 4]> {
    let mut out = Vec::new();
    let (Some(&a), Some(&b)) = (pts.first(), pts.get(1)) else { return out };
    let (mut a, mut b) = (a, b);
    let mut i = 2;
    while let Some(&c) = pts.get(i) {
        let d = pts.get(i + 1).copied().unwrap_or(c);
        out.push([a, b, c, d]);
        (a, b) = (c, d);
        i += 2;
    }
    out
}

fn run_solid(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param(p, "points").ok_or_else(|| bad("solid", "`points` [[x,y],...] (3 or more) is required"))?;
    if pts.len() < 3 {
        return Err(bad("solid", "a 2D solid needs at least 3 points"));
    }
    if pts.len() > MAX_POINTS {
        return Err(bad("solid", format!("at most {MAX_POINTS} points")));
    }
    if pts.iter().any(|q| !q.is_finite()) {
        return Err(bad("solid", "points must be finite"));
    }
    let mut hs = Vec::new();
    for [a, b, c, d] in pieces(&pts) {
        hs.push(s.add_entity(piece(a, b, c, d))?.hex());
    }
    Ok(json!({ "handles": hs }))
}

/// "Specify first point:", "Specify second point:", "Specify third point:", "Specify fourth
/// point or <exit>:" — Enter at the fourth makes a triangle; then third and fourth again, the
/// next piece sharing the last edge, until Enter at "Specify third point:".
#[derive(Default)]
struct SolidM {
    pts: Vec<Vec2>,
}

impl Interactive for SolidM {
    fn name(&self) -> &'static str {
        "SOLID"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let last = self.pts.last().copied();
        match self.pts.len() {
            0 => Prompt::new("Specify first point", Accept::POINT),
            1 => Prompt::new("Specify second point", Accept::POINT).base_opt(last),
            2 => Prompt::new("Specify third point", Accept::POINT).base_opt(last),
            _ => Prompt::new("Specify fourth point or <exit>", Accept::POINT).base_opt(last),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(p) if p.is_finite() => {
                self.pts.push(p);
                if let &[a, b, c, d] = self.pts.as_slice() {
                    s.add_entity(piece(a, b, c, d))?;
                    self.pts = vec![c, d];
                }
                Ok(Step::Continue)
            }
            Input::Enter if self.pts.len() == 3 => {
                if let &[a, b, c] = self.pts.as_slice() {
                    s.add_entity(piece(a, b, c, c))?;
                    self.pts = vec![c, c];
                }
                Ok(Step::Continue)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let mut pts = self.pts.clone();
        pts.push(cursor);
        match pts.as_slice() {
            [a, b] => vec![helpers::line(*a, *b)],
            [a, b, c] => vec![piece(*a, *b, *c, *c)],
            [a, b, c, d] => vec![piece(*a, *b, *c, *d)],
            _ => Vec::new(),
        }
    }
}
