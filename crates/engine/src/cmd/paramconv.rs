//! PARAMCONSTRAINT: associative dimensions become dimensional constraints (the dimension's
//! measurement becomes the constraint's value) and dimensional constraints become associative
//! dimensions again. The geometry doesn't move either way.

use cadaddy_constraints as cs;
use cadaddy_doc::{AssocSnap, Constraint, ConstraintKind as K, DimAssoc, DimKind, DistAxis, Drawing, EntityKind, GeomRef, Handle, Sub};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::constraints::{add_constraint, constraint_json};
use super::curves::handle_value;
use super::machines::SelectRun;
use super::{CommandSpec, bad, has_doc, str_param};
use crate::{Result, Session};

/// Objects converted at most in one call.
const MAX_ITEMS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("paramconstraint", "Convert Dimensions to Constraints", run)
            .params("{to?: constraint (default: the dimensions in `handles` or the selection become dimensional constraints) | dimension (the dimensional constraints in `constraints` — ids or names, default every one — become associative dimensions), handles?} → {converted, skipped: [{item, why}], constraints | dimensions}")
            .enabled(has_doc)
            .interactive(|_| Ok(Box::new(SelectRun::new("paramconstraint", "PARAMCONSTRAINT")))),
        CommandSpec::new("paramconstraint.todimension", "Convert Constraints to Dimensions", |s, p| {
            let mut q = p.as_object().cloned().unwrap_or_default();
            q.insert("to".into(), json!("dimension"));
            run(s, &Value::Object(q))
        })
        .params("{constraints?: [ids or names] (default every dimensional constraint)} → {converted, skipped, dimensions}")
        .enabled(has_doc),
    ]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    match str_param(p, "to").map(|t| t.trim().to_ascii_lowercase()).as_deref() {
        None | Some("constraint" | "constraints") => to_constraints(s, p),
        Some("dimension" | "dimensions") => to_dimensions(s, p),
        Some(other) => Err(bad("paramconstraint", format!("`to` must be constraint or dimension, not \"{other}\""))),
    }
}

fn snap_sub(snap: &AssocSnap) -> Option<Sub> {
    Some(match snap {
        AssocSnap::Start => Sub::Start,
        AssocSnap::End => Sub::End,
        AssocSnap::Mid => Sub::Mid,
        AssocSnap::Center => Sub::Center,
        AssocSnap::Vertex { index } => Sub::Vertex(u32::try_from(*index).ok()?),
        AssocSnap::OnCircle { .. } | AssocSnap::Intersection { .. } => return None,
    })
}

fn attached<'a>(assoc: &'a [DimAssoc], point: &str) -> Option<&'a DimAssoc> {
    assoc.iter().find(|a| a.point == point)
}

/// The constraint an associative dimension stands for (kind and references), or why it has none.
fn constraint_for(dm: &cadaddy_doc::Dimension) -> std::result::Result<(K, Vec<GeomRef>), &'static str> {
    if dm.assoc.is_empty() {
        return Err("not associative (only associative dimensions convert)");
    }
    let pair = || -> std::result::Result<Vec<GeomRef>, &'static str> {
        let (a, b) = (attached(&dm.assoc, "p13"), attached(&dm.assoc, "p14"));
        match (a, b) {
            (Some(a), Some(b)) => {
                let (sa, sb) = (
                    snap_sub(&a.snap).ok_or("attached to an intersection or a curve")?,
                    snap_sub(&b.snap).ok_or("attached to an intersection or a curve")?,
                );
                Ok(vec![GeomRef::new(a.handle, sa), GeomRef::new(b.handle, sb)])
            }
            _ => Err("both extension line origins must be attached to objects"),
        }
    };
    let circle = || dm.assoc.iter().map(|a| a.handle).next().map(GeomRef::whole).ok_or("not attached to a circle or arc");
    match dm.kind {
        DimKind::Linear { rotation } => {
            let r = rotation.rem_euclid(std::f64::consts::PI);
            let axis = if r.min(std::f64::consts::PI - r) < 1e-9 {
                DistAxis::Horizontal
            } else if (r - std::f64::consts::FRAC_PI_2).abs() < 1e-9 {
                DistAxis::Vertical
            } else {
                return Err("a rotated linear dimension has no matching constraint");
            };
            Ok((K::Distance(axis), pair()?))
        }
        DimKind::Aligned => Ok((K::Distance(DistAxis::Aligned), pair()?)),
        DimKind::Radius => Ok((K::Radius, vec![circle()?])),
        DimKind::Diameter => Ok((K::Diameter, vec![circle()?])),
        DimKind::Angular => {
            let mut lines: Vec<Handle> = Vec::new();
            for a in &dm.assoc {
                if !lines.contains(&a.handle) {
                    lines.push(a.handle);
                }
            }
            match lines.as_slice() {
                [a, b] => Ok((K::Angular, vec![GeomRef::whole(*a), GeomRef::whole(*b)])),
                _ => Err("an angular dimension converts when it is attached to two lines"),
            }
        }
        DimKind::Angular3P | DimKind::ArcLength | DimKind::Ordinate { .. } => Err("this kind of dimension has no matching constraint"),
    }
}

fn handles(s: &Session, p: &Value) -> Result<Vec<Handle>> {
    match p.get("handles") {
        None | Some(Value::Null) => Ok(s.selection()),
        Some(Value::Array(a)) => {
            a.iter().take(MAX_ITEMS).map(|h| handle_value(h).ok_or_else(|| bad("paramconstraint", format!("{h} is not an object handle")))).collect()
        }
        Some(_) => Err(bad("paramconstraint", "`handles` must be an array of handles")),
    }
}

fn to_constraints(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = handles(s, p)?;
    if hs.is_empty() {
        return Err(bad("paramconstraint", "select the associative dimensions to convert"));
    }
    let mut made = Vec::new();
    let mut skipped = Vec::new();
    for h in hs {
        let found = match s.doc()?.entity(h).map(|e| &e.kind) {
            Some(EntityKind::Dimension(dm)) => constraint_for(dm),
            Some(_) => Err("not a dimension"),
            None => Err("no such object"),
        };
        let (kind, refs) = match found {
            Ok(x) => x,
            Err(why) => {
                skipped.push(json!({ "item": h.hex(), "why": why }));
                continue;
            }
        };
        let value = match cs::measure(s.doc()?, kind, &refs) {
            Ok(v) if v.is_finite() => v,
            Ok(_) => {
                skipped.push(json!({ "item": h.hex(), "why": "its objects can't be measured" }));
                continue;
            }
            Err(m) => {
                skipped.push(json!({ "item": h.hex(), "why": m }));
                continue;
            }
        };
        match add_constraint(s, Constraint { id: 0, kind, refs, name: String::new(), expr: cs::format_value(value) }) {
            Ok(id) => {
                s.doc_mut()?.remove_entity(h);
                made.push(constraint_json(s.doc()?, id));
            }
            Err(e) => skipped.push(json!({ "item": h.hex(), "why": e.to_string() })),
        }
    }
    s.set_selection(Vec::new());
    let n = made.len();
    Ok(json!({
        "converted": n,
        "skipped": skipped,
        "constraints": made,
        "message": format!("{n} dimension(s) converted to constraints{}.", if skipped.is_empty() { String::new() } else { format!("; {} skipped", skipped.len()) }),
    }))
}

/// Where a constraint reference is now.
fn ref_point(d: &Drawing, r: &GeomRef) -> Option<Vec2> {
    cs::ref_points(d, r.handle).into_iter().find(|(sub, _)| *sub == r.sub).map(|(_, p)| p)
}

/// The two points a distance constraint measures between.
fn distance_points(d: &Drawing, c: &Constraint) -> Option<(Vec2, Vec2)> {
    match c.refs.as_slice() {
        [a, b] => Some((ref_point(d, a)?, ref_point(d, b)?)),
        [a] => Some((ref_point(d, &GeomRef::new(a.handle, Sub::Start))?, ref_point(d, &GeomRef::new(a.handle, Sub::End))?)),
        _ => None,
    }
}

/// The dimension command and its JSON for a dimensional constraint.
fn dimension_for(d: &Drawing, c: &Constraint) -> std::result::Result<(&'static str, Value), &'static str> {
    match c.kind {
        K::Distance(axis) => {
            let (p1, p2) = distance_points(d, c).ok_or("its points can't be found")?;
            let len = p1.dist(p2);
            let off = if len > 1e-9 { len * 0.25 } else { 1.0 };
            Ok(match axis {
                DistAxis::Horizontal => {
                    ("dimlinear", json!({ "p1": [p1.x, p1.y], "p2": [p2.x, p2.y], "at": [(p1.x + p2.x) / 2.0, p1.y.max(p2.y) + off], "rotation": 0 }))
                }
                DistAxis::Vertical => (
                    "dimlinear",
                    json!({ "p1": [p1.x, p1.y], "p2": [p2.x, p2.y], "at": [p1.x.max(p2.x) + off, (p1.y + p2.y) / 2.0], "rotation": 90 }),
                ),
                DistAxis::Aligned => {
                    let n = (p2 - p1).normalized().perp();
                    let at = p1.mid(p2) + n * off;
                    ("dimaligned", json!({ "p1": [p1.x, p1.y], "p2": [p2.x, p2.y], "at": [at.x, at.y] }))
                }
            })
        }
        K::Radius | K::Diameter => {
            let r = c.refs.first().ok_or("it has no circle")?;
            let id = if c.kind == K::Radius { "dimradius" } else { "dimdiameter" };
            Ok((id, json!({ "handle": r.handle.hex() })))
        }
        K::Angular => match c.refs.as_slice() {
            [a, b] => {
                let pa = ref_point(d, &GeomRef::new(a.handle, Sub::Mid)).ok_or("its lines can't be found")?;
                let pb = ref_point(d, &GeomRef::new(b.handle, Sub::Mid)).ok_or("its lines can't be found")?;
                let at = pa.mid(pb);
                Ok(("dimangular", json!({ "lines": [a.handle.hex(), b.handle.hex()], "at": [at.x, at.y] })))
            }
            _ => Err("an angle of one arc has no dimension yet"),
        },
        _ => Err("not a dimensional constraint"),
    }
}

/// The dimensional constraints chosen by `constraints` (ids or names); every one by default.
fn chosen(d: &Drawing, p: &Value) -> Result<Vec<u32>> {
    let dims = d.constraints.iter().filter(|c| c.kind.is_dimensional());
    let Some(list) = p.get("constraints").filter(|v| !v.is_null()) else { return Ok(dims.map(|c| c.id).collect()) };
    let items = list.as_array().ok_or_else(|| bad("paramconstraint", "`constraints` must be an array of ids or names"))?;
    let mut out = Vec::new();
    for it in items.iter().take(MAX_ITEMS) {
        let id = match it {
            Value::Number(n) => {
                n.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|id| d.constraints.iter().any(|c| c.id == *id && c.kind.is_dimensional()))
            }
            Value::String(t) => d.constraints.iter().find(|c| c.kind.is_dimensional() && c.name.eq_ignore_ascii_case(t.trim())).map(|c| c.id),
            _ => None,
        };
        out.push(id.ok_or_else(|| bad("paramconstraint", format!("no dimensional constraint {it}")))?);
    }
    Ok(out)
}

/// Another constraint's or a user parameter's expression uses `name`.
fn used_by(d: &Drawing, id: u32, name: &str) -> Option<String> {
    let uses = |expr: &str| {
        let mut names = Vec::new();
        if let Ok(e) = cs::expr::parse(expr) {
            cs::expr::names(&e, &mut names);
        }
        names.iter().any(|n| n.eq_ignore_ascii_case(name))
    };
    d.constraints
        .iter()
        .filter(|c| c.id != id && c.kind.is_dimensional())
        .find(|c| uses(&c.expr))
        .map(|c| c.name.clone())
        .or_else(|| d.parametric.parameters.iter().find(|p| uses(&p.expr)).map(|p| p.name.clone()))
}

fn to_dimensions(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = chosen(s.doc()?, p)?;
    let mut made = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let Some(c) = s.doc()?.constraints.iter().find(|c| c.id == id).cloned() else { continue };
        if let Some(user) = used_by(s.doc()?, id, &c.name) {
            skipped.push(json!({ "item": c.name, "why": format!("{user} uses it") }));
            continue;
        }
        let (cmd, params) = match dimension_for(s.doc()?, &c) {
            Ok(x) => x,
            Err(why) => {
                skipped.push(json!({ "item": c.name, "why": why }));
                continue;
            }
        };
        let spec = super::find_command(cmd).ok_or_else(|| bad("paramconstraint", format!("no command {cmd}")))?;
        let before = s.doc()?.clone();
        match (spec.run)(s, &params) {
            Ok(v) => {
                s.doc_mut()?.constraints.retain(|k| k.id != id);
                made.push(json!({ "constraint": c.name, "handle": v.get("handle").cloned().unwrap_or(Value::Null) }));
            }
            Err(e) => {
                *s.doc_mut()? = before;
                skipped.push(json!({ "item": c.name, "why": e.to_string() }));
            }
        }
    }
    let n = made.len();
    Ok(json!({
        "converted": n,
        "skipped": skipped,
        "dimensions": made,
        "message": format!("{n} constraint(s) converted to dimensions{}.", if skipped.is_empty() { String::new() } else { format!("; {} skipped", skipped.len()) }),
    }))
}
