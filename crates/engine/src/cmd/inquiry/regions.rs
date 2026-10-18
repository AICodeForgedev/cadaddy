//! Region booleans (UNION, SUBTRACT, INTERSECT) and MASSPROP.
//!
//! The booleans are exact on lines and arcs ([`cadaddy_geom::boolean`]): boundaries may cross,
//! touch or share edges; arcs stay arcs and holes are kept. The result is one region with every
//! loop of the result (its outer loops and islands worked out again by nesting).

use cadaddy_doc::{EntityKind, Handle, HatchLoop, MAX_REGION_LOOPS, Region};
use cadaddy_geom::PolyVertex;
use cadaddy_geom::boolean::{BoolError, BoolOp, boolean};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase, SelectRun};
use super::super::*;
use crate::{EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("union", "Union", run_union)
            .menu(&["Modify", "Solid Editing", "Union"])
            .alias(&["uni"])
            .params("{handles?} (regions)")
            .interactive(|_| Ok(Box::new(SelectRun::new("union", "UNION")))),
        CommandSpec::new("subtract", "Subtract", run_subtract)
            .menu(&["Modify", "Solid Editing", "Subtract"])
            .alias(&["su"])
            .params("{from: [handles], remove: [handles]} (regions)")
            .interactive(|_| Ok(Box::new(SubtractM::default()))),
        CommandSpec::new("intersect", "Intersect", run_intersect)
            .menu(&["Modify", "Solid Editing", "Intersect"])
            .alias(&["in"])
            .params("{handles?} (regions)")
            .interactive(|_| Ok(Box::new(SelectRun::new("intersect", "INTERSECT")))),
        CommandSpec::new("massprop", "Region/Mass Properties", run_massprop)
            .menu(&["Tools", "Inquiry", "Region/Mass Properties"])
            .params("{handles?} (regions) → properties of each")
            .noundo()
            .interactive(|_| Ok(Box::new(SelectRun::new("massprop", "MASSPROP")))),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

/// The regions among `hs` (others are left alone).
fn regions(s: &Session, hs: &[Handle]) -> Result<Vec<(Handle, Region)>> {
    let d = s.doc()?;
    Ok(hs
        .iter()
        .filter_map(|h| match &d.entity(*h)?.kind {
            EntityKind::Region(r) => Some((*h, r.clone())),
            _ => None,
        })
        .collect())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Union,
    Subtract,
    Intersect,
}

/// `a` op `b`.
fn combine(a: &Region, b: &Region, op: Op) -> Result<Region> {
    let size = a.bounds().union(&b.bounds());
    let tol = ((size.width() + size.height()) * 1e-9).max(1e-12);
    let loops = |r: &Region| -> Vec<Vec<PolyVertex>> { r.loops.iter().map(|l| l.vertices.clone()).collect() };
    let op = match op {
        Op::Union => BoolOp::Union,
        Op::Subtract => BoolOp::Difference,
        Op::Intersect => BoolOp::Intersection,
    };
    let out = boolean(&loops(a), &loops(b), op, tol).map_err(|e| match e {
        BoolError::TooComplex => other("These regions are too complex to combine."),
    })?;
    if out.len() > MAX_REGION_LOOPS {
        return Err(other("Too many loops."));
    }
    let mut r =
        Region { loops: out.into_iter().map(|vertices| HatchLoop { curves: Vec::new(), vertices, outer: false, sources: Vec::new() }).collect() };
    r.renest();
    Ok(r)
}

/// Replace the first region with `r` (or erase it when empty) and erase the others.
fn apply(s: &mut Session, keep: Handle, r: Region, erase: &[Handle]) -> Result<()> {
    let d = s.doc_mut()?;
    for h in erase.iter().filter(|h| **h != keep) {
        d.remove_entity(*h);
    }
    if r.loops.is_empty() {
        d.remove_entity(keep);
    } else {
        d.modify_entity(keep, |e| e.kind = EntityKind::Region(r))?;
    }
    Ok(())
}

fn fold(s: &mut Session, hs: &[Handle], op: Op) -> Result<Value> {
    let rs = regions(s, hs)?;
    let Some(((h0, first), rest)) = rs.split_first() else { return Err(other("Select at least one region.")) };
    let mut acc = first.clone();
    for (_, r) in rest {
        acc = combine(&acc, r, op)?;
    }
    let all: Vec<Handle> = rs.iter().map(|(h, _)| *h).collect();
    let empty = acc.loops.is_empty();
    apply(s, *h0, acc, &all)?;
    Ok(
        json!({ "handle": if empty { Value::Null } else { json!(h0.hex()) }, "message": if empty { "The result is empty: the regions were erased." } else { "" } }),
    )
}

fn run_union(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    fold(s, &hs, Op::Union)
}

fn run_intersect(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    fold(s, &hs, Op::Intersect)
}

fn handles(p: &Value, key: &str) -> Result<Vec<Handle>> {
    let a = p.get(key).and_then(Value::as_array).ok_or_else(|| bad("subtract", format!("`{key}` (array of handles) is required")))?;
    a.iter().map(|v| super::super::curves::handle_value(v).ok_or_else(|| bad("subtract", format!("`{key}`: {v} is not a handle")))).collect()
}

fn subtract(s: &mut Session, from: &[Handle], remove: &[Handle]) -> Result<Value> {
    let (a, b) = (regions(s, from)?, regions(s, remove)?);
    let Some(((h0, first), rest)) = a.split_first() else { return Err(other("Select regions to subtract from.")) };
    let mut acc = first.clone();
    for (_, r) in rest {
        acc = combine(&acc, r, Op::Union)?;
    }
    for (h, r) in &b {
        if a.iter().any(|(x, _)| x == h) {
            return Err(other("A region can't be subtracted from itself."));
        }
        acc = combine(&acc, r, Op::Subtract)?;
    }
    let all: Vec<Handle> = a.iter().chain(&b).map(|(h, _)| *h).collect();
    let empty = acc.loops.is_empty();
    apply(s, *h0, acc, &all)?;
    Ok(
        json!({ "handle": if empty { Value::Null } else { json!(h0.hex()) }, "message": if empty { "The result is empty: the regions were erased." } else { "" } }),
    )
}

fn run_subtract(s: &mut Session, p: &Value) -> Result<Value> {
    let (from, remove) = (handles(p, "from")?, handles(p, "remove")?);
    subtract(s, &from, &remove)
}

#[derive(Default)]
struct SubtractM {
    sel: SelectPhase,
    from: Option<Vec<Handle>>,
}

impl Interactive for SubtractM {
    fn name(&self) -> &'static str {
        "SUBTRACT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        s.echo("Select regions to subtract from ..");
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        self.sel.prompt()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => match self.from.take() {
                None => {
                    self.from = Some(hs);
                    self.sel = SelectPhase::default();
                    s.set_selection(Vec::new());
                    s.echo("Select regions to subtract ..");
                    Ok(Step::Continue)
                }
                Some(from) => {
                    let r = subtract(s, &from, &hs)?;
                    if let Some(m) = r.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                        s.echo(m.to_string());
                    }
                    s.set_selection(Vec::new());
                    Ok(Step::Done)
                }
            },
        }
    }
}

fn run_massprop(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let rs = regions(s, &hs)?;
    if rs.is_empty() {
        return Err(other("Select one or more regions."));
    }
    let f = |v: f64| super::fmt_d(s, v);
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for (h, r) in &rs {
        let m = r.props();
        let c = m.centroid;
        // Moments about the centroid, and the principal moments and directions.
        let [ixc, iyc, ixyc] = m.centroid_moments;
        let mid = (ixc + iyc) / 2.0;
        let rad = (((ixc - iyc) / 2.0).powi(2) + ixyc * ixyc).sqrt();
        let (i1, i2) = (mid - rad, mid + rad);
        let ang = 0.5 * (-2.0 * ixyc).atan2(ixc - iyc);
        let gyr = |i: f64| if m.area > 0.0 { (i / m.area).abs().sqrt() } else { 0.0 };
        out.push(json!({
            "handle": h.hex(), "area": m.area, "perimeter": m.perimeter,
            "boundingBox": [[m.bounds.min.x, m.bounds.min.y], [m.bounds.max.x, m.bounds.max.y]],
            "centroid": [c.x, c.y], "momentsOfInertia": [m.ixx, m.iyy], "productOfInertia": m.ixy,
            "radiiOfGyration": [gyr(m.ixx), gyr(m.iyy)], "principalMoments": [i1, i2],
            "principalDirections": [[ang.cos(), ang.sin()], [-ang.sin(), ang.cos()]],
        }));
        lines.push(format!(
            "----------------   REGIONS   ----------------\nArea:                    {}\nPerimeter:               {}\nBounding box:         X: {}  --  {}\n                      Y: {}  --  {}\nCentroid:             X: {}\n                      Y: {}\nMoments of inertia:   X: {}\n                      Y: {}\nProduct of inertia:  XY: {}\nRadii of gyration:    X: {}\n                      Y: {}\nPrincipal moments and X-Y directions about centroid:\n                      I: {} along [{:.4} {:.4}]\n                      J: {} along [{:.4} {:.4}]",
            f(m.area),
            f(m.perimeter),
            f(m.bounds.min.x),
            f(m.bounds.max.x),
            f(m.bounds.min.y),
            f(m.bounds.max.y),
            f(c.x),
            f(c.y),
            f(m.ixx),
            f(m.iyy),
            f(m.ixy),
            f(gyr(m.ixx)),
            f(gyr(m.iyy)),
            f(i1),
            ang.cos(),
            ang.sin(),
            f(i2),
            -ang.sin(),
            ang.cos(),
        ));
    }
    Ok(json!({ "regions": out, "message": lines.join("\n") }))
}
