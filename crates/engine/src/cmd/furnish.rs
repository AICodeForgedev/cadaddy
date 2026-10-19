//! FURNISH: furniture and equipment from the plan's 2D drawing, as 3D library models
//! ([`cadaddy_doc::library::models3d`]).
//!
//! - **Block references** match a model by name: the block's name words against the models'
//!   names, labels and keywords (`BED-QUEEN`, `WC`, `DESK_60`…), the closest in size winning a tie;
//!   failing that, by size.
//! - **Closed outlines** on furniture, equipment, casework and fixture layers match by size: the
//!   outline's smallest rectangle (width × depth) against each model's plan, within a tolerance.
//!
//! A matched model is placed with its plan's centre on the source's centre, turned as the
//! reference is (an outline: along its long side, its back to the nearest wall line), at the
//! drawing's units. It is tagged with its source (xdata `CADADDY_FURNISH`) so running FURNISH
//! again adds only what is new. What matches nothing is reported for an agent to resolve.

use cadaddy_doc::library::models3d::{self, Model};
use cadaddy_doc::{Drawing, EntityKind, Handle, RawTag};
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub const FURNISH_APP: &str = "CADADDY_FURNISH";
/// Layers whose closed outlines are furniture or equipment.
const OUTLINE_LAYERS: &str = "*FURN*,*EQPM*,*EQUIP*,*CASE*,*FIXT*,*PLUM*,*APPL*";
/// Most sources looked at.
const MAX_SOURCES: usize = 20_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("furnish", "Furnish from Plan", run)
        .menu(&["Tools", "Building", "Furnish from Plan"])
        .params("{place?: bool (default true; false only reports), layers?: outline layers (wildcards; default furniture, equipment, casework, fixture, plumbing, appliance layers), tolerance?: share of a size (default 0.2), handles? (only these sources)} → {placed: [{source, model, by: name|size, at, rotation, handle?}], unmatched: [{source, kind, name?, layer, size: [w, d]}]}")]
}

/// A thing on the plan to furnish: its centre, the direction its width runs, width × depth
/// (drawing units), and a block name.
#[derive(Clone, Debug)]
struct Source {
    handle: Handle,
    kind: &'static str,
    layer: String,
    name: Option<String>,
    center: Vec2,
    /// Radians: the source's own x axis (the reference's rotation; an outline's long side).
    angle: f64,
    size: (f64, f64),
    /// An outline: its back side is unknown (decided by the nearest wall).
    free: bool,
}

/// A model's plan in inches: its bounds.
fn plan_bounds(m: &Model) -> Bounds2 {
    let d = Drawing::new_imperial();
    let mut b = Bounds2::EMPTY;
    for e in m.plan() {
        let eb = cadaddy_doc::entity_bounds(&d, &e, 0);
        if !eb.is_empty() {
            b = b.union(&eb);
        }
    }
    b
}

/// Drawing units per inch (INSUNITS; unitless follows MEASUREMENT).
fn units_per_inch(d: &Drawing) -> f64 {
    match d.header.i64("INSUNITS", 0) {
        2 => 1.0 / 12.0,
        4 => 25.4,
        5 => 2.54,
        6 => 0.0254,
        1 => 1.0,
        _ if d.header.i64("MEASUREMENT", 0) == 1 => 25.4,
        _ => 1.0,
    }
}

/// The words of a name, upper case (`BED-QUEEN_60` → BED, QUEEN, 60).
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect()
}

/// How well a block name names a model: matching words (long words count more).
fn name_score(name: &str, m: &Model) -> f64 {
    let hay: Vec<String> = words(&format!("{} {} {}", m.name, m.label, m.keywords)).into_iter().filter(|w| w != "M3").collect();
    words(name)
        .iter()
        .filter(|w| w.len() >= 2 && !w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| hay.iter().any(|h| h == *w || (w.len() >= 4 && h.starts_with(w.as_str()))))
        .map(|w| w.len() as f64)
        .sum()
}

/// How far a size is from a model's plan (sorted sides, relative); `None` past the tolerance.
fn size_error(size: (f64, f64), m: (f64, f64), tol: f64) -> Option<f64> {
    let (a, b) = (size.0.max(size.1), size.0.min(size.1));
    let (c, d) = (m.0.max(m.1), m.0.min(m.1));
    if c <= 1e-9 || d <= 1e-9 {
        return None;
    }
    let e = ((a - c) / c).abs().max(((b - d) / d).abs());
    (e <= tol).then_some(e)
}

/// The smallest rectangle around `pts`: centre, angle of its long side, (long, short).
fn min_rect(pts: &[Vec2]) -> Option<(Vec2, f64, (f64, f64))> {
    if pts.len() < 3 {
        return None;
    }
    let mut best: Option<(f64, Vec2, f64, (f64, f64))> = None;
    let n = pts.len();
    for i in 0..n.min(2000) {
        let (Some(a), Some(b)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
        let dir = *b - *a;
        if dir.len() < 1e-12 {
            continue;
        }
        let ang = dir.angle();
        let (u, v) = (Vec2::from_angle(ang), Vec2::from_angle(ang).perp());
        let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
        for p in pts {
            let q = Vec2::new(p.dot(u), p.dot(v));
            lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
            hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
        }
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        let area = w * h;
        if best.as_ref().is_none_or(|x| area < x.0) {
            let mid = (lo + hi) * 0.5;
            let center = u * mid.x + v * mid.y;
            // The angle of the long side.
            let (ang, size) = if w >= h { (ang, (w, h)) } else { (ang + std::f64::consts::FRAC_PI_2, (h, w)) };
            best = Some((area, center, ang, size));
        }
    }
    best.map(|(_, c, a, s)| (c, a, s))
}

/// The plan's furnishable things.
fn sources(s: &Session, p: &Value, layers: &str) -> Result<Vec<Source>> {
    let d = s.doc()?;
    let sp = s.space();
    let only: Option<Vec<Handle>> = match p.get("handles") {
        Some(_) => Some(targets(s, p)?),
        None => None,
    };
    let furnished: Vec<Handle> = d
        .space(&sp)
        .map(|st| {
            st.iter()
                .filter_map(|e| {
                    let i = e.common.xdata.iter().position(|t| t.code == 1001 && t.value == FURNISH_APP)?;
                    e.common.xdata.get(i + 1).and_then(|t| Handle::parse_hex(&t.value))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    let Some(st) = d.space(&sp) else { return Ok(out) };
    for e in st.iter().take(MAX_SOURCES * 4) {
        if out.len() >= MAX_SOURCES || only.as_ref().is_some_and(|o| !o.contains(&e.handle)) || furnished.contains(&e.handle) {
            continue;
        }
        if e.common.xdata.iter().any(|t| t.code == 1001 && t.value == FURNISH_APP) {
            continue;
        }
        match &e.kind {
            EntityKind::Insert(i) => {
                // Library models already placed are not sources.
                if models3d::model(&i.block).is_some() || i.block.to_ascii_uppercase().ends_with("_BODY") {
                    continue;
                }
                let Some(b) = d.block(&i.block) else { continue };
                let mut bb = Bounds2::EMPTY;
                for be in b.entities.iter() {
                    let x = cadaddy_doc::entity_bounds(d, be, 1);
                    if !x.is_empty() {
                        bb = bb.union(&x);
                    }
                }
                if bb.is_empty() {
                    continue;
                }
                // The block's box placed by the reference: centre, rotation, scaled size.
                let (sx, sy) = (i.scale.x.abs(), i.scale.y.abs());
                let off = bb.center() - b.base.xy();
                let local = Vec2::new(off.x * i.scale.x, off.y * i.scale.y);
                let center = i.insert.xy() + local.rotate(i.rotation);
                out.push(Source {
                    handle: e.handle,
                    kind: "block",
                    layer: e.common.layer.clone(),
                    name: Some(i.block.clone()),
                    center,
                    angle: i.rotation,
                    size: (bb.width() * sx, bb.height() * sy),
                    free: false,
                });
            }
            EntityKind::LwPolyline(pl) if pl.closed && layers.split(',').any(|q| super::qselect::wildcard(q.trim(), &e.common.layer)) => {
                let pts: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
                let Some((center, angle, size)) = min_rect(&pts) else { continue };
                out.push(Source { handle: e.handle, kind: "outline", layer: e.common.layer.clone(), name: None, center, angle, size, free: true });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The direction (unit) from `c` to the nearest wall line (layers named like walls), if any is
/// near.
fn wall_side(d: &Drawing, space: &cadaddy_doc::Space, c: Vec2, reach: f64) -> Option<Vec2> {
    let st = d.space(space)?;
    let mut best: Option<(f64, Vec2)> = None;
    for e in st.iter().filter(|e| e.common.layer.to_ascii_uppercase().contains("WALL")).take(200_000) {
        let segs: Vec<(Vec2, Vec2)> = match &e.kind {
            EntityKind::Line(l) => vec![(l.a.xy(), l.b.xy())],
            EntityKind::LwPolyline(p) => {
                let v: Vec<Vec2> = p.vertices.iter().map(|x| x.p).collect();
                let n = v.len();
                (0..if p.closed { n } else { n.saturating_sub(1) }).filter_map(|i| Some((*v.get(i)?, *v.get((i + 1) % n)?))).collect()
            }
            _ => continue,
        };
        for (a, b) in segs {
            let ab = b - a;
            let l2 = ab.dot(ab);
            let t = if l2 > 1e-18 { ((c - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let q = a + ab * t;
            let dist = q.dist(c);
            if dist <= reach && dist > 1e-9 && best.is_none_or(|x| dist < x.0) {
                best = Some((dist, (q - c) * (1.0 / dist)));
            }
        }
    }
    best.map(|x| x.1)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "furnish";
    let tol = match p.get("tolerance") {
        None | Some(Value::Null) => 0.2,
        Some(v) => v.as_f64().filter(|t| t.is_finite() && (0.0..=1.0).contains(t)).ok_or_else(|| bad(cmd, "`tolerance` must be from 0 to 1"))?,
    };
    let layers = str_param(p, "layers").filter(|l| !l.trim().is_empty()).unwrap_or(OUTLINE_LAYERS).to_string();
    let place = p.get("place").and_then(Value::as_bool).unwrap_or(true);
    let srcs = sources(s, p, &layers)?;
    let d = s.doc()?;
    let k = units_per_inch(d);
    let space = s.space();
    // Each model's plan size and centre, in drawing units.
    let plans: Vec<(&'static Model, (f64, f64), Vec2)> = models3d::models()
        .iter()
        .map(|m| {
            let b = plan_bounds(m);
            (m, (b.width() * k, b.height() * k), b.center() * k)
        })
        .filter(|(_, sz, _)| sz.0 > 1e-9 && sz.1 > 1e-9)
        .collect();
    let mut matches: Vec<(Source, &'static Model, &'static str, f64, Vec2)> = Vec::new();
    let mut unmatched: Vec<Value> = Vec::new();
    for src in srcs {
        // By name, the closest in size among the best named.
        let named = src.name.as_deref().and_then(|n| {
            let best = plans.iter().map(|(m, _, _)| name_score(n, m)).fold(0.0, f64::max);
            (best >= 2.0).then(|| {
                plans
                    .iter()
                    .filter(|(m, _, _)| name_score(n, m) >= best)
                    .min_by(|a, b| {
                        size_error(src.size, a.1, f64::MAX).unwrap_or(f64::MAX).total_cmp(&size_error(src.size, b.1, f64::MAX).unwrap_or(f64::MAX))
                    })
                    .map(|x| (x.0, x.1, x.2, "name"))
            })?
        });
        let by_size = || {
            plans
                .iter()
                .filter_map(|(m, sz, c)| size_error(src.size, *sz, tol).map(|e| (e, *m, *sz, *c)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, m, sz, c)| (m, sz, c, "size"))
        };
        let Some((m, msize, mcenter, by)) = named.or_else(by_size) else {
            unmatched.push(json!({
                "source": src.handle.hex(), "kind": src.kind, "name": src.name, "layer": src.layer,
                "size": [super::perceive::round(src.size.0), super::perceive::round(src.size.1)], "at": [src.center.x, src.center.y],
            }));
            continue;
        };
        // The model's width along the source's width: a quarter turn when the source's sides
        // run the other way.
        let mut angle = src.angle;
        let model_long_x = msize.0 >= msize.1;
        let source_long_x = src.size.0 >= src.size.1;
        if model_long_x != source_long_x {
            angle += std::f64::consts::FRAC_PI_2;
        }
        // An outline's back (the model's +Y) faces the nearest wall.
        if src.free
            && let Some(w) = wall_side(d, &space, src.center, src.size.0.max(src.size.1) * 1.5)
            && Vec2::from_angle(angle).perp().dot(w) < 0.0
        {
            angle += std::f64::consts::PI;
        }
        // The model's plan centre on the source's centre.
        let at = src.center - mcenter.rotate(angle);
        matches.push((src, m, by, angle, at));
    }
    let mut placed = Vec::new();
    let place_cmd = find_command("models3d.place").ok_or_else(|| EngineError::UnknownCommand("models3d.place".into()))?;
    for (src, m, by, angle, at) in matches {
        let deg = angle.to_degrees().rem_euclid(360.0);
        let mut v = json!({ "source": src.handle.hex(), "model": m.name, "by": by, "at": [at.x, at.y], "rotation": deg });
        if place {
            let r = (place_cmd.run)(s, &json!({ "name": m.name, "at": [at.x, at.y], "rotation": deg }))?;
            if let Some(h) = r.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex) {
                let layer = src.layer.clone();
                s.doc_mut()?.modify_entity(h, |e| {
                    e.common.layer = layer;
                    e.common.xdata = vec![RawTag { code: 1001, value: FURNISH_APP.into() }, RawTag { code: 1005, value: src.handle.hex() }];
                })?;
                if let Some(o) = v.as_object_mut() {
                    o.insert("handle".into(), json!(h.hex()));
                }
            }
        }
        placed.push(v);
    }
    if place {
        s.echo(format!("{} placed, {} not matched.", placed.len(), unmatched.len()));
    }
    Ok(json!({ "placed": placed, "unmatched": unmatched }))
}
