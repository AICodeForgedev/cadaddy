//! Inquiry: DIST, ID, LIST, AREA, drawing inspection for agents.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Entity, EntityKind, Handle};
use cadaddy_geom::{Polyline, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::units::format_angle;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub(super) mod live;
mod loops;
mod regions;
mod shapes;
mod usage;

/// The inquiry commands: distances, points, listing, area, time and status, the drawing and
/// entity queries an agent reads, SHAPES, USAGE, LOOPS, COUNT and the region commands.
pub fn specs() -> Vec<CommandSpec> {
    let mut v = vec![
        CommandSpec::new("dist", "Distance", run_dist)
            .menu(&["Tools", "Inquiry", "Distance"])
            .alias(&["di"])
            .params("{p1, p2}")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(DistM::default()))),
        CommandSpec::new("id", "ID Point", run_id)
            .menu(&["Tools", "Inquiry", "ID Point"])
            .params("{at}")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(IdM))),
        CommandSpec::new("list", "List", run_list)
            .menu(&["Tools", "Inquiry", "List"])
            .alias(&["li", "ls"])
            .params("{handles?}")
            .noundo()
            .interactive(|_| Ok(Box::new(ListM::default()))),
        CommandSpec::new("area", "Area", run_area)
            .menu(&["Tools", "Inquiry", "Area"])
            .alias(&["aa"])
            .params("{points: [[x,y],...]} | {handle} | {add: [{points} | {handle}, ...], subtract?: [...]} → running total, as the Add/Subtract area modes")
            .noundo()
            .interactive(|_| Ok(Box::new(AreaM::default()))),
        CommandSpec::new("time", "Time", run_time).menu(&["Tools", "Inquiry", "Time"]).noundo(),
        CommandSpec::new("status", "Status", run_status).menu(&["Tools", "Inquiry", "Status"]).noundo(),
        CommandSpec::new("drawing.inspect", "Inspect Drawing", run_inspect).params("{entities?: bool, limit?: n, index? (another open drawing, as DOCUMENT.LIST numbers them)}").noundo(),
        CommandSpec::new("drawing.summary", "Drawing Summary", summary::run_summary).params(summary::SUMMARY_PARAMS).noundo(),
        CommandSpec::new("drawing.query", "Query Drawing", summary::run_query).params(summary::QUERY_PARAMS).noundo(),
        CommandSpec::new("entities", "Query Entities", run_entities)
            .params("{type?, layer?, selected?: bool (default false; true = current selection in active space), window?: [[x,y],[x,y]], crossing?: bool (default true; false = entirely inside), limit?, offset?} → {count (all matches), returned, offset, entities: [{handle, type, layer, color, transparency, geometry, extents: {min: [x,y,z], max: [x,y,z]}}], warning? (a type or layer filter no name matches)}: `type` (Line, LINE or AcDbLine) and `layer` take wildcards (* ? # @ . ~ [...]) and comma lists, any case, as the CLass option and object filters do")
            .noundo(),
        CommandSpec::new("shapes", "Analyze Shapes", shapes::run).params(shapes::PARAMS).noundo(),
        CommandSpec::new("usage", "Analyze Usage", usage::run).params(usage::PARAMS).noundo(),
        CommandSpec::new("loops", "Implied Loops", loops::run).params(loops::PARAMS).noundo(),
        CommandSpec::new("texts", "Query Texts", run_texts)
            .params("{contains?: text (any case), layer?, window?: [[x,y],[x,y]] (by insertion point), limit? (default 200), offset?} → {count (all matches), returned, offset, texts: [{handle, type, layer, text, at, height, rotation (degrees), style, tag? (attributes)}]}: the readable text of TEXT, MTEXT (formatting removed), block attributes and multileaders")
            .noundo(),
        CommandSpec::new("countlist", "Count List", count::run_countlist)
            .menu(&["Tools", "Count"])
            .params("{space?: all (default)|current|model|<layout>, nested?: bool} → per-block counts, as COUNT")
            .noundo(),
        CommandSpec::new("counttable", "Count Table", count::run_counttable)
            .params("{at: [x,y], space?: current (default)|model|all|<layout>, area?: [[x,y],[x,y]] | polygon, nested?: bool, scale?: number (default: the annotation scale in model space, 1 on a layout)} → {handle, linked}: a table of the block counts, kept up to date (on REGEN, open, save, plot) with its size and formatting kept | {update: true, handles?} → {updated}")
            .interactive(|_| Ok(count::table_machine())),
        CommandSpec::new("count", "Count", count::run_count)
            .params(count::PARAMS)
            .interactive(|_| Ok(count::machine()))
            .noundo(),
    ];
    v.extend(regions::specs());
    v
}

pub(crate) fn fmt_d(s: &Session, v: f64) -> String {
    crate::units::dist(s, v)
}
/// A quantity that is not a length (an area, a volume, a moment of inertia) as MASSPROP prints it:
/// a plain number at LUPREC, with at least 4 decimals in architectural and fractional units whose
/// precision counts fraction steps.
pub(crate) fn fmt_n(s: &Session, v: f64) -> String {
    let (lunits, prec) = s.doc().map(|d| (d.header.i64("LUNITS", 2), d.header.i64("LUPREC", 4))).unwrap_or((2, 4));
    let p = if matches!(lunits, 4 | 5) { 4 } else { usize::try_from(prec.clamp(0, 8)).unwrap_or(4) };
    cadaddy_doc::numfmt::fixed(v, p)
}
/// An area as AutoCAD reports it: square inches with square feet in architectural and engineering
/// units ("11520.0000 square in. (80.0000 square ft.)"), else a plain number at LUPREC.
pub(crate) fn fmt_area(s: &Session, v: f64) -> String {
    let (lunits, prec) = s.doc().map(|d| (d.header.i64("LUNITS", 2), d.header.i64("LUPREC", 4))).unwrap_or((2, 4));
    let p = usize::try_from(prec.clamp(0, 8)).unwrap_or(4);
    if matches!(lunits, 3 | 4) {
        // Architectural precision counts fraction steps; areas show at least 4 decimals.
        let p = if lunits == 4 { 4 } else { p };
        format!("{} square in. ({} square ft.)", cadaddy_doc::numfmt::fixed(v, p), cadaddy_doc::numfmt::fixed(v / 144.0, p))
    } else {
        cadaddy_doc::numfmt::fixed(v, p)
    }
}
fn fmt_a(s: &Session, v: f64) -> String {
    let h = s.doc().map(|d| (d.header.i64("AUNITS", 0), d.header.i64("AUPREC", 0))).unwrap_or((0, 0));
    format_angle(v, h.0, h.1)
}

/// Schema of the analysis self-check block (#1440). Fields may be added within a version; a
/// rename or a changed meaning bumps it.
pub(super) const SELF_CHECK_SCHEMA: u64 = 1;

/// How far past its bound a value may be before it is implausible: a relative slack for the
/// rounding of two ways of measuring the same thing.
const IMPLAUSIBLE_TOLERANCE: f64 = 1e-9;

/// How many objects of one type an analysis skipped, and every distinct reason it gave.
#[derive(Default)]
struct SkipTally {
    count: u64,
    reasons: BTreeSet<String>,
}

/// The self-check every analysis result carries (#1440): how many objects it found, how many it
/// analysed, what it skipped and why, the caps it hit with their values, and values so
/// implausible that they point at a bug rather than at the drawing.
#[derive(Default)]
pub(super) struct SelfCheck {
    found: u64,
    analyzed: u64,
    skipped: BTreeMap<String, SkipTally>,
    caps: BTreeMap<String, u64>,
    implausible: Vec<Value>,
}

impl SelfCheck {
    /// An empty self-check: nothing found, analysed or skipped yet.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Count objects the analysis found (matched its filters).
    pub(super) fn found(&mut self, n: u64) {
        self.found = self.found.saturating_add(n);
    }

    /// Count objects the analysis actually analysed.
    pub(super) fn analyzed(&mut self, n: u64) {
        self.analyzed = self.analyzed.saturating_add(n);
    }

    /// Count `n` objects of `ty` skipped, with the reason.
    pub(super) fn skip(&mut self, ty: &str, reason: &str, n: u64) {
        let skip = self.skipped.entry(ty.to_string()).or_default();
        skip.count = skip.count.saturating_add(n);
        skip.reasons.insert(reason.to_string());
    }

    /// Record a cap the analysis hit, with its value (e.g. `limit`, 200).
    pub(super) fn cap(&mut self, name: &str, value: u64) {
        self.caps.insert(name.to_string(), value);
    }

    /// Flag `value` as implausible when it is past `bound`, with the rule that makes it so. A
    /// non-finite `bound` means no bound applies (an empty drawing has no extents). Values within
    /// `IMPLAUSIBLE_TOLERANCE` of the bound are not flagged: they are the rounding of two ways
    /// of measuring the same thing (a border rectangle's shoelace area against the extents'
    /// `width * height`), not a bug.
    pub(super) fn implausible(&mut self, handle: Handle, ty: &str, field: &str, value: f64, bound: f64, rule: &str) {
        if value.is_finite() && bound.is_finite() && value > bound + bound.abs() * IMPLAUSIBLE_TOLERANCE {
            self.implausible.push(json!({
                "handle": handle.hex(),
                "type": ty,
                "field": field,
                "value": value,
                "bound": bound,
                "reason": rule,
            }));
        }
    }

    /// The skip map in the shape analyses have always reported: type → {count, reason}.
    pub(super) fn skipped_json(&self) -> Value {
        Value::Object(
            self.skipped
                .iter()
                .map(|(ty, skip)| {
                    let reason = skip.reasons.iter().map(String::as_str).collect::<Vec<_>>().join("; ");
                    (ty.clone(), json!({ "count": skip.count, "reason": reason }))
                })
                .collect(),
        )
    }

    /// The self-check as the JSON object every analysis returns: schema, counts, skip map, caps
    /// and implausible values.
    pub(super) fn to_json(&self) -> Value {
        json!({
            "schema": SELF_CHECK_SCHEMA,
            "found": self.found,
            "analyzed": self.analyzed,
            "skipped": self.skipped_json(),
            "caps": self.caps,
            "implausible": self.implausible,
        })
    }
}

/// The angle of `d` up from the XY plane.
fn angle_from_xy(d: Vec3) -> f64 {
    d.z.atan2(d.xy().len())
}

const DIST_TOO_LARGE: &str = "the distance is too large to compute";

/// Whether the offset's length and parts are numbers that can be reported (not infinite or past
/// the ±1e12 the commands work within).
fn distance_ok(d: Vec3) -> bool {
    [d.len(), d.x, d.y, d.z].iter().all(|v| v.is_finite() && v.abs() <= 1e13)
}

/// The "Distance = …" line DIST echoes for a path length.
fn distance_line(s: &Session, d: f64) -> String {
    if d.is_finite() && d.abs() <= 1e13 { format!("Distance = {}", fmt_d(s, d)) } else { format!("The {DIST_TOO_LARGE}.") }
}

fn dist_text(s: &Session, a: Vec3, b: Vec3) -> String {
    let d = b - a;
    format!(
        "Distance = {},  Angle in XY Plane = {},  Angle from XY Plane = {}\nDelta X = {},  Delta Y = {},   Delta Z = {}",
        fmt_d(s, (b - a).len()),
        fmt_a(s, d.xy().angle()),
        fmt_a(s, angle_from_xy(d)),
        fmt_d(s, d.x),
        fmt_d(s, d.y),
        fmt_d(s, d.z)
    )
}

/// A point parameter with its Z: the third number when given, else the Z of what is drawn
/// there ([`z_at`]).
fn point3_req(s: &Session, cmd: &str, p: &Value, key: &str) -> Result<Vec3> {
    let q = point_req(cmd, p, key)?;
    let z = p.get(key).and_then(|v| v.get(2)).and_then(Value::as_f64).filter(|z| z.is_finite());
    Ok(Vec3::new(q.x, q.y, z.unwrap_or_else(|| z_at(s, q))))
}

fn run_dist(s: &mut Session, p: &Value) -> Result<Value> {
    let a = point3_req(s, "dist", p, "p1")?;
    let b = point3_req(s, "dist", p, "p2")?;
    let d = b - a;
    if !distance_ok(d) {
        return Err(bad("dist", DIST_TOO_LARGE));
    }
    s.last_used.distance = d.len();
    Ok(json!({
        "distance": d.len(),
        "angle": d.xy().angle().to_degrees(),
        "angleFromXY": angle_from_xy(d).to_degrees(),
        "dx": d.x,
        "dy": d.y,
        "dz": d.z,
        "message": dist_text(s, a, b),
    }))
}

/// The ID report: X and Y in the current UCS, as AutoCAD does; Z as given.
fn id_text(s: &Session, a: Vec3) -> String {
    let u = s.ucs().to_ucs(a.xy());
    format!("X = {}     Y = {}     Z = {}", fmt_d(s, u.x), fmt_d(s, u.y), fmt_d(s, a.z))
}

fn run_id(s: &mut Session, p: &Value) -> Result<Value> {
    let a = point3_req(s, "id", p, "at")?;
    if ![a.x, a.y, a.z].iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
        return Err(bad("id", "the point is too large to report (limit \u{b1}1e12)"));
    }
    s.last_point = a.xy();
    // `x`, `y` stay the world point.
    let u = s.ucs().to_ucs(a.xy());
    Ok(json!({ "x": a.x, "y": a.y, "z": a.z, "ucs": [u.x, u.y], "message": id_text(s, a) }))
}

/// The Z of what is drawn at `p`: the object a pick finds there, at that point, or the current
/// elevation (ELEVATION) when there is none. Points come from the canvas in 2D; the object
/// under a snapped point gives its Z back, as AutoCAD's ID and DIST report a snapped point's.
pub(crate) fn z_at(s: &Session, p: Vec2) -> f64 {
    let Ok(d) = s.doc() else { return 0.0 };
    let elevation = d.header.f64("ELEVATION", 0.0);
    let ap = s.pixel_size() * s.settings.pickbox.max(1.0) * 1.5;
    let space = s.space();
    crate::select::pick(d, &space, p, ap)
        .and_then(|h| d.entity(h))
        .and_then(|e| entity_z(e, p))
        .filter(|z| z.is_finite())
        .unwrap_or(if elevation.is_finite() { elevation } else { 0.0 })
}

/// The Z of `e` at the point of it nearest `p` (seen from above), when it has one there.
fn entity_z(e: &Entity, p: Vec2) -> Option<f64> {
    let on_segment = |a: Vec3, b: Vec3| {
        let d = b.xy() - a.xy();
        let l2 = d.dot(d);
        let t = if l2 > 0.0 { ((p - a.xy()).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
        (a.z + (b.z - a.z) * t, (a.xy() + d * t).dist(p))
    };
    let along = |pts: &[Vec3], closed: bool| {
        let n = pts.len();
        let segs = if closed { n } else { n.saturating_sub(1) };
        (0..segs)
            .filter_map(|i| Some(on_segment(*pts.get(i)?, *pts.get((i + 1) % n)?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(z, _)| z)
            .or_else(|| pts.first().map(|q| q.z))
    };
    let n = e.common.extrusion;
    let plan = n.x.abs() < 1e-9 && n.y.abs() < 1e-9 && n.z != 0.0;
    match &e.kind {
        EntityKind::Point(q) => Some(q.p.z),
        EntityKind::Line(l) => Some(on_segment(l.a, l.b).0),
        EntityKind::Polyline3d(pl) => along(&pl.points, pl.closed),
        // Elevation is in the polyline's OCS: down Z for a mirrored one.
        EntityKind::LwPolyline(pl) if plan => Some(if n.z < 0.0 { -pl.elevation } else { pl.elevation }),
        // Read in world coordinates already.
        EntityKind::Circle(c) => Some(c.center.z),
        EntityKind::Arc(a) => Some(a.center.z),
        EntityKind::Text(t) => Some(t.insert.z),
        EntityKind::AttDef(a) => Some(a.text.insert.z),
        EntityKind::MText(t) => Some(t.insert.z),
        EntityKind::Insert(i) => Some(i.insert.z),
        EntityKind::Face3d(f) => face_z(&f.corners, p).or_else(|| along(&f.corners, true)),
        _ => None,
    }
}

/// The Z at `p` on a 3D face: on the plane of the triangle of its corners that holds `p`.
fn face_z(c: &[Vec3; 4], p: Vec2) -> Option<f64> {
    let tri = |a: Vec3, b: Vec3, d: Vec3| -> Option<f64> {
        let n = (b - a).cross(d - a);
        if n.z.abs() < 1e-12 * (1.0 + n.len()) {
            return None;
        }
        let inside = cadaddy_geom::point_in_polygon(&[a.xy(), b.xy(), d.xy()], p);
        inside.then(|| a.z - (n.x * (p.x - a.x) + n.y * (p.y - a.y)) / n.z)
    };
    tri(c[0], c[1], c[2]).or_else(|| if c[3] == c[2] { None } else { tri(c[0], c[2], c[3]) })
}

fn list_text(s: &Session, h: Handle) -> String {
    let Ok(d) = s.doc() else { return String::new() };
    let Some(e) = d.entity(h) else { return String::new() };
    let space = if d.model.contains(h) { "Model space" } else { "Paper space" };
    let mut t = format!(
        "                  {}     Layer: \"{}\"\n                            Space: {}\n                   Handle = {}",
        e.kind.dxf_name(),
        e.common.layer,
        space,
        h.hex()
    );
    let eb = cadaddy_doc::entity_bounds(d, e, 0);
    if !eb.is_empty() && !cadaddy_doc::within_limit(&eb, 1e12) {
        t.push_str("\n                  The object lies beyond \u{b1}1e12: its values are too large to list.");
        return t;
    }
    match &e.kind {
        EntityKind::Line(l) => {
            t.push_str(&format!(
                "\n              from point, X={}  Y={}  Z={}\n                to point, X={}  Y={}  Z={}",
                fmt_d(s, l.a.x),
                fmt_d(s, l.a.y),
                fmt_d(s, l.a.z),
                fmt_d(s, l.b.x),
                fmt_d(s, l.b.y),
                fmt_d(s, l.b.z)
            ));
            t.push_str(&format!(
                "\n          Length = {},  Angle in XY Plane = {}",
                fmt_d(s, (l.b - l.a).len()),
                fmt_a(s, l.a.xy().angle_to(l.b.xy()))
            ));
            let d = l.b - l.a;
            t.push_str(&format!("\n          Delta X = {},  Delta Y = {},  Delta Z = {}", fmt_d(s, d.x), fmt_d(s, d.y), fmt_d(s, d.z)));
        }
        EntityKind::Circle(c) => {
            t.push_str(&format!(
                "\n            center point, X={}  Y={}  Z={}\n            radius {}\n     circumference {}\n             area {}",
                fmt_d(s, c.center.x),
                fmt_d(s, c.center.y),
                fmt_d(s, c.center.z),
                fmt_d(s, c.radius),
                fmt_d(s, c.radius * cadaddy_geom::TAU),
                fmt_area(s, c.radius * c.radius * cadaddy_geom::PI)
            ));
        }
        EntityKind::Arc(a) => {
            t.push_str(&format!(
                "\n            center point, X={}  Y={}  Z={}\n            radius {}\n            start angle {}\n              end angle {}",
                fmt_d(s, a.center.x),
                fmt_d(s, a.center.y),
                fmt_d(s, a.center.z),
                fmt_d(s, a.radius),
                fmt_a(s, a.start),
                fmt_a(s, a.end)
            ));
        }
        EntityKind::LwPolyline(p) => {
            let pl = Polyline { vertices: p.vertices.clone(), closed: p.closed };
            t.push_str(&format!("\n            {}\n    Constant width    {}", if p.closed { "Closed" } else { "Open" }, fmt_d(s, p.const_width)));
            if p.closed {
                t.push_str(&format!("\n             area   {}\n        perimeter   {}", fmt_area(s, pl.area().abs()), fmt_d(s, pl.len())));
            } else {
                t.push_str(&format!("\n           length   {}", fmt_d(s, pl.len())));
            }
            // The elevation, in world Z (down Z for a mirrored polyline).
            let z = entity_z(e, Vec2::ZERO).unwrap_or(p.elevation);
            t.push_str(&format!("\n        elevation   {}", fmt_d(s, z)));
            for v in &p.vertices {
                t.push_str(&format!("\n          at point  X={}  Y={}  Z={}", fmt_d(s, v.p.x), fmt_d(s, v.p.y), fmt_d(s, z)));
            }
        }
        EntityKind::Text(tx) => {
            t.push_str(&format!(
                "\n            Style = \"{}\"\n       start point, X={}  Y={}  Z={}\n          height {}\n           text {}\n        rotation angle {}",
                tx.style,
                fmt_d(s, tx.insert.x),
                fmt_d(s, tx.insert.y),
                fmt_d(s, tx.insert.z),
                fmt_d(s, tx.height),
                tx.value,
                fmt_a(s, tx.rotation)
            ));
        }
        EntityKind::Dimension(dm) => {
            let st = d.dim_style(&dm.style).cloned().unwrap_or_default().with_overrides(&dm.overrides);
            let g = cadaddy_render::dimension_geometry(dm, &st, 1.0);
            t.push_str(&format!(
                "\n            Style = \"{}\"\n             Type = {}\n      Measurement = {}\n             Text = {}\n  Definition point, X={}  Y={}",
                dm.style,
                dim_type(&dm.kind),
                g.value,
                if dm.text.is_empty() { "<>".to_string() } else { dm.text.clone() },
                fmt_d(s, dm.defpt.x),
                fmt_d(s, dm.defpt.y),
            ));
        }
        EntityKind::Mesh(m) => {
            let what = match m.kind {
                cadaddy_doc::MeshKind::Subd => "Subdivision mesh".to_string(),
                cadaddy_doc::MeshKind::Polyface => "Polyface mesh".to_string(),
                cadaddy_doc::MeshKind::Polygon { m: rows, n: cols, .. } => format!("Polygon mesh, {rows} x {cols}"),
            };
            t.push_str(&format!(
                "\n            Mesh type = {what}\n         Vertex count = {}\n           Face count = {}",
                m.vertices.len(),
                m.faces.len()
            ));
        }
        // Other objects: their properties one per line (LIST printed them as raw JSON).
        other => {
            if let Ok(serde_json::Value::Object(m)) = serde_json::to_value(other) {
                for (k, v) in m.iter().filter(|(k, _)| k.as_str() != "type").take(40) {
                    t.push_str(&format!("\n          {k} = {}", list_value(v)));
                }
            }
        }
    }
    // 3D objects print where they are, as LIST's bounding box: their points alone don't say it.
    if matches!(e.kind, EntityKind::Mesh(_) | EntityKind::Solid3d(_) | EntityKind::Surface(_) | EntityKind::Polyline3d(_) | EntityKind::Face3d(_))
        && let Some((lo, hi)) = extents3(d, e)
    {
        t.push_str(&format!(
            "\n         Bounding box:  Lower bound X={}  Y={}  Z={}\n                        Upper bound X={}  Y={}  Z={}",
            fmt_d(s, lo.x),
            fmt_d(s, lo.y),
            fmt_d(s, lo.z),
            fmt_d(s, hi.x),
            fmt_d(s, hi.y),
            fmt_d(s, hi.z)
        ));
    }
    t
}

/// A dimension's type as LIST names it.
fn dim_type(k: &cadaddy_doc::DimKind) -> String {
    let name = format!("{k:?}");
    name.split([' ', '{', '(']).next().unwrap_or("").to_string()
}

/// A property value on one LIST line: a point as X, Y, Z; a list by its length.
fn list_value(v: &Value) -> String {
    match v {
        Value::Object(o) if o.contains_key("x") && o.contains_key("y") => {
            let n = |k: &str| o.get(k).and_then(Value::as_f64).map(|x| format!("{x}")).unwrap_or_default();
            if o.contains_key("z") { format!("X={}  Y={}  Z={}", n("x"), n("y"), n("z")) } else { format!("X={}  Y={}", n("x"), n("y")) }
        }
        Value::Array(a) => format!("({} items)", a.len()),
        Value::Object(o) => format!("({} values)", o.len()),
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let texts: Vec<String> = hs.iter().map(|h| list_text(s, *h)).collect();
    Ok(json!({ "message": texts.join("\n\n") }))
}

pub(super) fn area_of(pts: &[Vec2]) -> (f64, f64) {
    let a = cadaddy_geom::shoelace(pts).abs();
    let n = pts.len();
    let per: f64 = (0..n).filter_map(|i| Some(pts.get(i)?.dist(*pts.get((i + 1) % n)?))).sum();
    (a, per)
}

/// One boundary of AREA's add/subtract lists: `{points}` or `{handle}`.
fn area_item(s: &Session, v: &Value) -> Result<(f64, f64)> {
    if let Some(pts) = points_param(v, "points") {
        if pts.len() < 3 {
            return Err(bad("area", "a boundary needs at least 3 points"));
        }
        return Ok(area_of(&pts));
    }
    let h =
        v.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex).ok_or_else(|| bad("area", "each boundary is {points} or {handle}"))?;
    object_area(s, h)
}

fn run_area(s: &mut Session, p: &Value) -> Result<Value> {
    let list = |k: &str| -> Result<Option<&Vec<Value>>> {
        p.get(k).filter(|v| !v.is_null()).map(|v| v.as_array().ok_or_else(|| bad("area", format!("`{k}` must be a list of boundaries")))).transpose()
    };
    let (add, sub) = (list("add")?, list("subtract")?);
    if add.is_some() || sub.is_some() {
        // AREA's Add and Subtract modes: each area is reported with the running total.
        let mut total = 0.0;
        let mut lines = Vec::new();
        let mut last = (0.0, 0.0);
        for (sign, items, mode) in [(1.0, add, "(ADD mode)"), (-1.0, sub, "(SUBTRACT mode)")] {
            for it in items.into_iter().flatten() {
                last = area_item(s, it)?;
                if let Some(what) = too_large(last.0, last.1) {
                    return Err(bad("area", format!("the {what} is too large")));
                }
                total += sign * last.0;
                lines.push(format!("{mode} {}", area_text(s, last.0, last.1)));
                lines.push(format!("Total area = {}", fmt_area(s, total)));
            }
        }
        (s.last_used.area, s.last_used.perimeter) = (total, last.1);
        return Ok(json!({ "area": total, "message": lines.join("\n") }));
    }
    let (a, per) = if let Some(pts) = points_param(p, "points") {
        area_of(&pts)
    } else {
        let h = targets(s, p)?.first().copied().ok_or_else(|| bad("area", "`points` or `handle` is required"))?;
        object_area(s, h)?
    };
    if let Some(what) = too_large(a, per) {
        return Err(bad("area", format!("the {what} is too large")));
    }
    (s.last_used.area, s.last_used.perimeter) = (a, per);
    Ok(json!({ "area": a, "perimeter": per, "message": area_text(s, a, per) }))
}

/// MEASUREGEOM's Volume option: the area of a polygon of three or more points, or of a closed 2D
/// object (as AREA measures it), times the absolute value of a height.
pub(super) fn run_volume(s: &mut Session, p: &Value) -> Result<Value> {
    let a = if let Some(pts) = points_param(p, "points") {
        if pts.len() < 3 {
            return Err(bad("volume.measure", "`points` needs 3 or more [x,y] points"));
        }
        area_of(&pts).0
    } else if p.get("points").is_some_and(|v| !v.is_null()) {
        return Err(bad("volume.measure", "`points` must be a list of finite [x,y] points"));
    } else {
        let h = targets(s, p)?.first().copied().ok_or_else(|| bad("volume.measure", "`points` or `handle` is required"))?;
        object_area(s, h)?.0
    };
    let height = f64_req("volume.measure", p, "height")?;
    let v = a * height.abs();
    if !v.is_finite() {
        return Err(bad("volume.measure", "the volume is too large"));
    }
    Ok(json!({ "volume": v, "area": a, "height": height, "message": volume_text(s, v) }))
}

fn volume_text(s: &Session, v: f64) -> String {
    format!("Volume = {}", fmt_n(s, v))
}

/// "area" or "perimeter" when that value is past f64's range, `None` when both can be reported.
fn too_large(a: f64, per: f64) -> Option<&'static str> {
    if !a.is_finite() {
        Some("area")
    } else if !per.is_finite() {
        Some("perimeter")
    } else {
        None
    }
}

pub(crate) fn area_text(s: &Session, a: f64, per: f64) -> String {
    format!("Area = {}, Perimeter = {}", fmt_area(s, a), fmt_d(s, per))
}

/// Area and perimeter of a closed object (AREA's Object option).
pub(crate) fn object_area(s: &Session, h: Handle) -> Result<(f64, f64)> {
    let d = s.doc()?;
    let e = d.entity(h).ok_or_else(|| bad("area", "no such object"))?;
    kind_area(&e.kind)
}

/// Area and perimeter of a closed shape (TAKEOFF measures transformed copies of block contents).
pub(crate) fn kind_area(k: &EntityKind) -> Result<(f64, f64)> {
    Ok(match k {
        EntityKind::Circle(c) => (c.radius * c.radius * cadaddy_geom::PI, c.radius * cadaddy_geom::TAU),
        EntityKind::LwPolyline(pl) => {
            // An open polyline is measured as if closed for the area, but its perimeter is its own length (AutoCAD's AREA help).
            let g = Polyline { vertices: pl.vertices.clone(), closed: true };
            let drawn = Polyline { vertices: pl.vertices.clone(), closed: pl.closed };
            (g.area().abs(), drawn.len())
        }
        EntityKind::Ellipse(el) => {
            let arc = cadaddy_geom::Ellipse { center: el.center.xy(), major: el.major.xy(), ratio: el.ratio, start: el.start, end: el.end };
            if !arc.is_full() {
                return Err(bad("area", "object has no area"));
            }
            let a = el.major.xy().len();
            let b = a * el.ratio;
            // The perimeter integrated around the ellipse, not a closed-form estimate (those
            // fall short as the ellipse flattens: 0.3% on a 100:1 one).
            (cadaddy_geom::PI * a * b, arc.arc_length_from(0.0, cadaddy_geom::TAU))
        }
        EntityKind::Hatch(hh) => {
            // Odd parity, as drawn: a loop nested inside an odd number of other loops is an unfilled island.
            let geoms: Vec<Polyline> = hh.loops.iter().map(|l| Polyline { vertices: l.vertices.clone(), closed: true }).collect();
            let polys: Vec<Vec<Vec2>> = geoms.iter().map(|g| g.tessellate(1e-3)).collect();
            let mut total = (0.0, 0.0);
            for (i, (g, l)) in geoms.iter().zip(&hh.loops).enumerate() {
                let probe = polys.get(i).and_then(|p| p.first().copied());
                let depth = probe.map_or(0, |pt| polys.iter().enumerate().filter(|(j, q)| *j != i && cadaddy_geom::point_in_polygon(q, pt)).count());
                // Exact from the loop's edges when it has curves (an ellipse, a spline).
                let a = l.area().abs();
                total.0 += if depth % 2 == 0 { a } else { -a };
                total.1 += g.len();
            }
            total
        }
        EntityKind::Region(r) => (r.area(), r.perimeter()),
        // Like an open polyline: the area closed by the chord, the perimeter its own length.
        EntityKind::Spline(sp) if sp.is_valid() => {
            let b = sp.bounds();
            let pts = sp.tessellate_spans((b.min.dist(b.max) * 1e-7).max(1e-12));
            // A length past f64's range is too large to report, as a polyline's is, not 0.
            (cadaddy_geom::shoelace(&pts).abs(), sp.checked_length().unwrap_or(f64::INFINITY))
        }
        _ => return Err(bad("area", "object has no area")),
    })
}

fn run_time(s: &mut Session, _p: &Value) -> Result<Value> {
    let rev = s.state()?.revision;
    Ok(json!({ "message": format!("Drawing revision: {rev}\nEdits recorded: {}", s.state()?.undo.len()) }))
}

fn run_status(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let msg = format!(
        "{} objects in {}\nModel space limits are X: {}  Y: {}  (Off)\nModel space uses X: {}  Y: {}\n                 X: {}  Y: {}\nCurrent layer: {}\nCurrent color: {}\nCurrent linetype: {}",
        d.entity_count(),
        s.state()?.title,
        fmt_d(s, d.header.point("LIMMIN").map(|p| p.x).unwrap_or(0.0)),
        fmt_d(s, d.header.point("LIMMIN").map(|p| p.y).unwrap_or(0.0)),
        fmt_d(s, ext.min.x),
        fmt_d(s, ext.min.y),
        fmt_d(s, ext.max.x),
        fmt_d(s, ext.max.y),
        d.header.str("CLAYER", "0"),
        cadaddy_color::Color::from_aci(d.header.i64("CECOLOR", 256) as i16).name(),
        d.header.str("CELTYPE", "ByLayer"),
    );
    Ok(json!({ "message": msg }))
}

fn entity_summary(e: &cadaddy_doc::Entity) -> Value {
    json!({ "handle": e.handle.hex(), "type": e.kind.type_name(), "layer": e.common.layer, "color": e.common.color.name(), "transparency": super::props::transparency_value(e.common.transparency), "geometry": e.kind })
}

fn run_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    // Another open drawing (by its place in DOCUMENT.LIST), without switching to it.
    let st = match p.get("index").filter(|v| !v.is_null()) {
        Some(i) => {
            let i = i.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad("drawing.inspect", "`index` is a drawing number"))?;
            s.docs.get(i).ok_or_else(|| bad("drawing.inspect", format!("there is no open drawing {i}")))?
        }
        None => s.state()?,
    };
    let d = &st.doc;
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
    let space = st.space.clone();
    let ext = crate::cached_extents(d, &space);
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    if let Some(store) = d.space(&space) {
        for e in store.iter() {
            *counts.entry(e.kind.type_name()).or_default() += 1;
        }
    }
    let mut out = json!({
        "title": st.title,
        "path": st.path,
        "dirty": st.is_dirty(),
        "blockEditor": st.block_editor(),
        "space": space.tab_name(),
        // INSUNITS by name ("Unitless" when the drawing declares none).
        "units": summary::units_name(d.header.i64("INSUNITS", 0)),
        "entityCount": d.entity_count(),
        "counts": counts,
        "extents": if ext.is_empty() { Value::Null } else { json!([[ext.min.x, ext.min.y], [ext.max.x, ext.max.y]]) },
        "layers": d.layers.iter().map(|l| json!({"name": l.name, "on": l.on, "frozen": l.frozen, "locked": l.locked, "color": l.color.name(), "linetype": l.linetype})).collect::<Vec<_>>(),
        "currentLayer": d.header.str("CLAYER", "0"),
        "linetypes": d.linetypes.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
        "textStyles": d.text_styles.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
        "dimStyles": d.dim_styles.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
        "blocks": d.blocks.iter().filter(|(k, b)| !k.starts_with('*') && !b.anonymous).map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        "layouts": d.layouts.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
        "selection": st.selection.iter().map(|h| h.hex()).collect::<Vec<_>>(),
        "undo": st.undo.iter().rev().take(20).map(|u| u.label.clone()).collect::<Vec<_>>(),
        "view": { "center": [st.view().center.x, st.view().center.y], "height": st.view().height },
        // Records of the file the drawing came from that the reader could not place.
        "skipped": { "count": d.skipped_records.values().fold(0usize, |a, n| a.saturating_add(*n)), "sections": d.skipped_records },
        // What reading the file left out (a cut-off DXF, unreadable DWG objects), in words:
        // `readNotes`, and `warnings` as before.
        "warnings": d.read_notes,
        "readNotes": d.read_notes,
    });
    if bool_or(p, "entities", true)
        && let (Some(o), Some(store)) = (out.as_object_mut(), d.space(&space))
    {
        o.insert("entities".into(), Value::Array(store.iter().take(limit).map(|e| entity_summary(e)).collect()));
    }
    Ok(out)
}

fn run_entities(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "entities";
    let given = |k: &str| p.get(k).filter(|v| !v.is_null());
    let text = |k: &str| -> Result<Option<String>> {
        given(k).map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(CMD, format!("`{k}` must be a string")))).transpose()
    };
    let filter = |k: &str| -> Result<Option<(String, crate::select::NameFilter)>> {
        text(k)?
            .map(|t| {
                let f = crate::select::NameFilter::new(&t)
                    .ok_or_else(|| bad(CMD, format!("`{k}` is longer than {} characters", crate::select::MAX_FILTER_LEN)))?;
                Ok((t, f))
            })
            .transpose()
    };
    let count = |k: &str, default: usize| -> Result<usize> {
        given(k).map_or(Ok(default), |v| {
            v.as_u64().map(|n| usize::try_from(n).unwrap_or(usize::MAX)).ok_or_else(|| bad(CMD, format!("`{k}` must be a whole number, 0 or more")))
        })
    };
    let mut ty = filter("type")?;
    let mut layer = filter("layer")?;
    let window = given("window")
        .map(|w| {
            let corners: Option<Vec<Vec2>> = w.as_array().filter(|a| a.len() == 2).and_then(|a| a.iter().map(point_value).collect());
            match corners.as_deref() {
                Some([a, b]) => Ok(cadaddy_geom::Bounds2::new(*a, *b)),
                _ => Err(bad(CMD, "`window` must be two corner points [[x0, y0], [x1, y1]]")),
            }
        })
        .transpose()?;
    let crossing = given("crossing").map_or(Ok(true), |v| v.as_bool().ok_or_else(|| bad(CMD, "`crossing` must be true or false")))?;
    let selected = given("selected").map_or(Ok(false), |v| v.as_bool().ok_or_else(|| bad(CMD, "`selected` must be true or false")))?;
    let limit = count("limit", 500)?;
    let offset = count("offset", 0)?;
    let st = s.state()?;
    let d = &st.doc;
    let store = d.space(&st.space);
    // A filter that no name in the drawing passes says so, instead of a bare 0 (Q6125).
    let mut warnings: Vec<String> = Vec::new();
    if let Some((t, f)) = layer.as_mut() {
        let used = store.into_iter().flat_map(|sp| sp.iter()).map(|e| e.common.layer.as_str());
        let mut names: BTreeSet<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
        names.extend(used);
        if !names.iter().any(|n| f.matches(n)) {
            warnings.push(format!("No layer in the drawing matches \"{t}\"."));
        }
    }
    if let Some((t, f)) = ty.as_mut() {
        let present: BTreeSet<&str> = store.into_iter().flat_map(|sp| sp.iter()).map(|e| e.kind.type_name()).collect();
        let hit = store.into_iter().flat_map(|sp| sp.iter()).any(|e| crate::select::class_matches(e, f));
        if !hit {
            let have: Vec<&str> = present.iter().copied().take(20).collect();
            let list = if have.is_empty() { "none".to_string() } else { have.join(", ") };
            warnings.push(format!("No object in {} is of a type matching \"{t}\" (types here: {list}).", st.space.tab_name()));
        }
    }
    let Some(store) = store else {
        let mut out = json!({ "count": 0, "returned": 0, "offset": offset, "entities": [] });
        attach_warning(&mut out, &warnings);
        return Ok(out);
    };
    let tol = window.as_ref().map(crate::select::window_tol).unwrap_or(0.0);
    let selection: std::collections::HashSet<Handle> = if selected { st.selection.iter().copied().collect() } else { Default::default() };
    let matches: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = store
        .iter()
        .filter(|e| !selected || selection.contains(&e.handle))
        .filter(|e| ty.as_mut().is_none_or(|(_, f)| crate::select::class_matches(e, f)))
        .filter(|e| layer.as_mut().is_none_or(|(_, f)| f.matches(&e.common.layer)))
        .filter(|e| window.as_ref().is_none_or(|w| crate::select::in_window(d, e, None, w, crossing, tol)))
        .collect();
    if ty.iter().chain(layer.iter()).any(|(_, f)| f.exhausted()) {
        return Err(bad(CMD, "a `type` or `layer` pattern is too complex to match; use fewer `*` wildcards"));
    }
    let page: Vec<Value> = matches
        .iter()
        .skip(offset)
        .take(limit)
        .map(|e| {
            let mut v = entity_summary(e);
            if let Some(o) = v.as_object_mut() {
                o.insert(
                    "extents".into(),
                    extents3(d, e).map_or(Value::Null, |(lo, hi)| json!({ "min": [lo.x, lo.y, lo.z], "max": [hi.x, hi.y, hi.z] })),
                );
            }
            v
        })
        .collect();
    // `count` is every match; `returned` is this page of them (`offset`, `limit`).
    let mut out = json!({ "count": matches.len(), "returned": page.len(), "offset": offset, "entities": page });
    attach_warning(&mut out, &warnings);
    Ok(out)
}

/// Add the filter warnings to a reply as one `warning` line (nothing when there are none).
fn attach_warning(out: &mut Value, warnings: &[String]) {
    if let (false, Some(o)) = (warnings.is_empty(), out.as_object_mut()) {
        o.insert("warning".into(), Value::String(warnings.join(" ")));
    }
}

/// Most points one object's extents read (a bigger mesh is measured by its first ones).
const MAX_EXTENT_POINTS: usize = 1_000_000;
/// Objects inside block references one object's Z range reads.
const MAX_EXTENT_NESTED: usize = 20_000;

/// A growing 3D box.
struct Box3(Option<(Vec3, Vec3)>);

impl Box3 {
    fn add(&mut self, p: Vec3) {
        if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
            return;
        }
        self.0 = Some(match self.0 {
            None => (p, p),
            Some((lo, hi)) => (Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)), Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z))),
        });
    }
}

/// The world box of an object's own 3D points (meshes, polyface and polygon meshes, 3D solids,
/// surfaces, 3D polylines, 3D faces, lines), or `None` for any other object.
fn points3(e: &Entity) -> Option<(Vec3, Vec3)> {
    let mut b = Box3(None);
    match &e.kind {
        EntityKind::Mesh(m) => m.vertices.iter().take(MAX_EXTENT_POINTS).for_each(|v| b.add(*v)),
        EntityKind::Solid3d(so) => {
            let pr = so.props(so.mesh_tol());
            if pr.min.x <= pr.max.x && pr.min.y <= pr.max.y && pr.min.z <= pr.max.z {
                b.add(pr.min);
                b.add(pr.max);
            }
        }
        // A NURBS surface lies inside the hull of its control points.
        EntityKind::Surface(sf) => sf.nurbs.control.iter().take(MAX_EXTENT_POINTS).for_each(|v| b.add(*v)),
        EntityKind::Polyline3d(pl) => pl.points.iter().take(MAX_EXTENT_POINTS).for_each(|v| b.add(*v)),
        EntityKind::Face3d(f) => f.corners.iter().for_each(|v| b.add(*v)),
        EntityKind::Line(l) => [l.a, l.b].into_iter().for_each(|v| b.add(v)),
        _ => return None,
    }
    b.0
}

/// The lowest and highest Z of an object: its 3D points, a block reference's contents at its
/// insertion and Z scale, else its elevation.
fn z_span(d: &cadaddy_doc::Drawing, e: &Entity, depth: usize, budget: &mut usize) -> Option<(f64, f64)> {
    if let Some((lo, hi)) = points3(e) {
        return Some((lo.z, hi.z));
    }
    if let EntityKind::Insert(ins) = &e.kind {
        let base = ins.insert.z;
        let inner = d.block(&ins.block).filter(|_| depth < 8).and_then(|blk| {
            let mut span: Option<(f64, f64)> = None;
            for c in blk.entities.iter() {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                if let Some((a, b)) = z_span(d, c, depth + 1, budget) {
                    span = Some(span.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b))));
                }
            }
            let (lo, hi) = span?;
            let (a, b) = (base + ins.scale.z * (lo - blk.base.z), base + ins.scale.z * (hi - blk.base.z));
            Some((a.min(b), a.max(b)))
        });
        return Some(inner.unwrap_or((base, base))).filter(|(a, b)| a.is_finite() && b.is_finite());
    }
    let pb = cadaddy_doc::entity_bounds(d, e, 0);
    let at = if pb.is_empty() { Vec2::ZERO } else { Vec2::new((pb.min.x + pb.max.x) / 2.0, (pb.min.y + pb.max.y) / 2.0) };
    let z = entity_z(e, at).unwrap_or(0.0);
    z.is_finite().then_some((z, z))
}

/// An object's 3D extents in world coordinates, `(min, max)`, or `None` when it has none (an
/// empty block reference, a ray). 3D objects measure their points; everything else its plan
/// extents between its lowest and highest Z.
pub(crate) fn extents3(d: &cadaddy_doc::Drawing, e: &Entity) -> Option<(Vec3, Vec3)> {
    if let Some(b) = points3(e) {
        return Some(b);
    }
    let pb = cadaddy_doc::entity_bounds(d, e, 0);
    if pb.is_empty() || ![pb.min.x, pb.min.y, pb.max.x, pb.max.y].iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut budget = MAX_EXTENT_NESTED;
    let (z0, z1) = z_span(d, e, 0, &mut budget)?;
    Some((Vec3::new(pb.min.x, pb.min.y, z0), Vec3::new(pb.max.x, pb.max.y, z1)))
}

/// TEXTS: the readable text of the space's texts, multiline texts, block attributes and
/// multileaders, with where they are, filtered by content, layer and a window around their
/// insertion points. Read-only (FIND selects and can replace; this only reports).
fn run_texts(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "texts";
    let given = |k: &str| p.get(k).filter(|v| !v.is_null());
    let text = |k: &str| -> Result<Option<String>> {
        given(k).map(|v| v.as_str().map(str::to_lowercase).ok_or_else(|| bad(CMD, format!("`{k}` must be a string")))).transpose()
    };
    let count = |k: &str, default: usize| -> Result<usize> {
        given(k).map_or(Ok(default), |v| {
            v.as_u64().map(|n| usize::try_from(n).unwrap_or(usize::MAX)).ok_or_else(|| bad(CMD, format!("`{k}` must be a whole number, 0 or more")))
        })
    };
    let contains = text("contains")?;
    let layer = text("layer")?;
    let window = given("window")
        .map(|w| {
            let corners: Option<Vec<Vec2>> = w.as_array().filter(|a| a.len() == 2).and_then(|a| a.iter().map(point_value).collect());
            match corners.as_deref() {
                Some([a, b]) => Ok(cadaddy_geom::Bounds2::new(*a, *b)),
                _ => Err(bad(CMD, "`window` must be two corner points [[x0, y0], [x1, y1]]")),
            }
        })
        .transpose()?;
    let limit = count("limit", 200)?;
    let offset = count("offset", 0)?;
    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else { return Ok(json!({ "count": 0, "returned": 0, "offset": offset, "texts": [] })) };
    let mut found: Vec<Value> = Vec::new();
    for e in store.iter() {
        if layer.as_ref().is_some_and(|l| e.common.layer.to_lowercase() != *l) {
            continue;
        }
        // value: (as stored, as it reads); the search matches what it reads.
        let mut push = |ty: &str, value: (String, String), at: cadaddy_geom::Vec3, height: f64, rotation: f64, style: &str, tag: Option<&str>| {
            let value = (value.0, value.1.trim().to_string());
            if value.1.is_empty()
                || contains.as_ref().is_some_and(|c| !value.1.to_lowercase().contains(c.as_str()))
                || window.as_ref().is_some_and(|w| !w.contains(Vec2::new(at.x, at.y)))
            {
                return;
            }
            let mut v = json!({
                "handle": e.handle.hex(), "type": ty, "layer": e.common.layer, "text": value.1,
                "at": [at.x, at.y], "height": height, "rotation": rotation.to_degrees(), "style": style,
            });
            if let Some(o) = v.as_object_mut() {
                if let Some(t) = tag {
                    o.insert("tag".into(), json!(t));
                }
                // As stored, when the codes in it read differently (%%c, \U+2300, MTEXT formatting).
                if value.0 != value.1 {
                    o.insert("raw".into(), json!(value.0));
                }
            }
            found.push(v);
        };
        match &e.kind {
            EntityKind::Text(t) => {
                push("Text", (t.value.clone(), cadaddy_fonts::plain_text(&t.value)), t.insert, t.height, t.rotation, &t.style, None)
            }
            EntityKind::MText(t) => {
                push("MText", (t.contents.clone(), cadaddy_fonts::plain_mtext(&t.contents)), t.insert, t.height, t.rotation, &t.style, None)
            }
            EntityKind::Insert(i) => {
                for a in i.attribs.iter().filter(|a| !a.invisible) {
                    let v = (a.text.value.clone(), cadaddy_fonts::plain_text(&a.text.value));
                    push("Attribute", v, a.text.insert, a.text.height, a.text.rotation, &a.text.style, Some(&a.tag));
                }
            }
            EntityKind::MLeader(m) => {
                if let Some(t) = &m.text {
                    push("MLeader", (t.contents.clone(), cadaddy_fonts::plain_mtext(&t.contents)), t.insert, t.height, t.rotation, &t.style, None);
                }
            }
            _ => {}
        }
    }
    let n = found.len();
    let page: Vec<Value> = found.into_iter().skip(offset).take(limit).collect();
    Ok(json!({ "count": n, "returned": page.len(), "offset": offset, "texts": page }))
}

mod count;
mod summary;

/// Count tables whose counts changed, worked out again (with the drawing's fields).
pub(crate) fn update_count_tables(s: &mut Session) -> usize {
    count::update_tables(s, None).unwrap_or(0)
}

#[derive(Default)]
struct DistM {
    first: Option<Vec2>,
    first_source: Option<crate::snap::ViewportPoint>,
    /// Multiple points: the points so far, the first included.
    multi: Option<Vec<Vec2>>,
    sources: Vec<Option<crate::snap::ViewportPoint>>,
    /// Asking for the length of a segment (the Length option).
    length: bool,
    /// Multiple points: the bulge of the segment from each point to the next (arcs).
    bulges: Vec<f64>,
    /// The Arc option: the next points end arcs.
    arc: bool,
}

/// The path through `pts` with the bulge of the segment from each point to the next.
fn path(pts: &[Vec2], bulges: &[f64], closed: bool) -> Polyline {
    let vertices = pts.iter().enumerate().map(|(i, p)| cadaddy_geom::PolyVertex::with_bulge(*p, bulges.get(i).copied().unwrap_or(0.0))).collect();
    Polyline { vertices, closed }
}

/// Total length of the segments (lines and arcs) through `pts`.
fn path_len(pts: &[Vec2], bulges: &[f64]) -> f64 {
    path(pts, bulges, false).len()
}

/// The direction the path through `pts` ends in: along its last segment (the end tangent of an
/// arc), along X after a single point.
fn end_dir(pts: &[Vec2], bulges: &[f64]) -> Vec2 {
    let n = pts.len();
    let (Some(prev), Some(last)) = (n.checked_sub(2).and_then(|i| pts.get(i)), pts.last()) else { return Vec2::X };
    let chord = (*last - *prev).normalized();
    let b = n.checked_sub(2).and_then(|i| bulges.get(i)).copied().unwrap_or(0.0);
    let d = if b.is_finite() { chord.rotate(2.0 * b.atan()) } else { chord };
    if d == Vec2::ZERO || !d.is_finite() { Vec2::X } else { d }
}

/// The bulge of an arc from the last of `pts` to `to`, tangent to the path's end (the Arc option).
fn tangent_bulge(pts: &[Vec2], bulges: &[f64], to: Vec2) -> f64 {
    let Some(last) = pts.last() else { return 0.0 };
    let chord = to - *last;
    if chord.len() < 1e-12 {
        return 0.0;
    }
    let t = end_dir(pts, bulges);
    // Half the arc's sweep is the turn from the tangent to the chord.
    let phi = t.cross(chord).atan2(t.dot(chord));
    let b = (phi / 2.0).tan();
    if b.is_finite() && b.abs() < 1e6 { b } else { 0.0 }
}

/// The point `len` beyond the last of `pts`, continuing the path's end (along X after a single
/// point): the Length option of AREA and DIST's point sequences.
fn extend_last(pts: &[Vec2], bulges: &[f64], len: f64) -> Option<Vec2> {
    let last = *pts.last()?;
    let p = last + end_dir(pts, bulges) * len;
    p.is_finite().then_some(p)
}

/// Add `p` to the path through `pts`: a line, or with the Arc option an arc tangent to its end.
fn push_point(pts: &mut Vec<Vec2>, bulges: &mut Vec<f64>, p: Vec2, arc: bool) {
    bulges.resize(pts.len(), 0.0);
    if arc {
        let b = tangent_bulge(pts, bulges, p);
        if let Some(last) = bulges.last_mut() {
            *last = b;
        }
    }
    pts.push(p);
    bulges.push(0.0);
}

/// Take back the last point of the path (its segment's bulge goes with it).
fn pop_point(pts: &mut Vec<Vec2>, bulges: &mut Vec<f64>) {
    pts.pop();
    bulges.truncate(pts.len());
    if let Some(b) = bulges.last_mut() {
        *b = 0.0;
    }
}

fn arc_prompt(total: bool) -> Prompt {
    let p = Prompt::new("Specify endpoint of arc or", Accept::POINT);
    if total { p.kw(&["Line", "Undo", "Total"]).default("Total") } else { p.kw(&["Line", "Undo"]) }
}

fn length_prompt() -> Prompt {
    Prompt::new("Specify length of line", Accept::NUMBER)
}

/// Feed the Length option's answer: the next point, or `None` when the option was dismissed (Enter).
fn length_input(pts: &[Vec2], bulges: &[f64], i: Input) -> Result<Option<Vec2>> {
    let t = match i {
        Input::Text(t) => t,
        _ => return Ok(None),
    };
    let len = crate::units::parse_distance(&t).filter(|v| v.is_finite()).ok_or_else(|| crate::EngineError::Other("Requires a distance.".into()))?;
    Ok(extend_last(pts, bulges, len))
}

/// Model units apply only when every point was snapped through one and the same viewport.
fn model_points(sources: &[Option<crate::snap::ViewportPoint>]) -> Option<Vec<Vec2>> {
    let viewport = sources.first().copied().flatten()?.viewport;
    sources.iter().map(|s| s.filter(|s| s.viewport == viewport).map(|s| s.point)).collect()
}

impl Interactive for DistM {
    fn name(&self) -> &'static str {
        "DIST"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.length {
            return length_prompt();
        }
        match (&self.multi, self.first) {
            (Some(pts), _) if self.arc => arc_prompt(pts.len() >= 2).base_opt(pts.last().copied()),
            (Some(pts), _) if pts.len() < 2 => {
                Prompt::new("Specify second point", Accept::POINT).kw(&["Arc", "Length", "Undo"]).base_opt(pts.last().copied())
            }
            (Some(pts), _) => Prompt::new("Specify next point", Accept::POINT)
                .kw(&["Arc", "Length", "Undo", "Total"])
                .default("Total")
                .base_opt(pts.last().copied()),
            (None, None) => Prompt::new("Specify first point", Accept::POINT),
            (None, Some(a)) => Prompt::new("Specify second point", Accept::POINT).kw(&["Multiple points"]).base(a),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if let Some(pts) = &mut self.multi {
            if self.length {
                let model = model_points(&self.sources);
                if let Some(p) = length_input(model.as_deref().unwrap_or(pts), &self.bulges, i)? {
                    let source = model.as_ref().and_then(|_| self.sources.last().copied().flatten());
                    let paper = match (source, pts.last()) {
                        (Some(source), Some(last)) => *last + (p - source.point) * source.scale,
                        _ => p,
                    };
                    if !paper.is_finite() {
                        return Err(bad("dist", "the next point is too large"));
                    }
                    push_point(pts, &mut self.bulges, paper, false);
                    self.sources.push(source.map(|source| crate::snap::ViewportPoint { point: p, ..source }));
                    let model = model_points(&self.sources);
                    s.echo(distance_line(s, path_len(model.as_deref().unwrap_or(pts), &self.bulges)));
                }
                self.length = false;
                return Ok(Step::Continue);
            }
            match i {
                Input::Point(p) => {
                    push_point(pts, &mut self.bulges, p, self.arc);
                    self.sources.push(s.picked_viewport);
                    let model = model_points(&self.sources);
                    s.echo(distance_line(s, path_len(model.as_deref().unwrap_or(pts), &self.bulges)));
                }
                Input::Keyword(k) if k == "Undo" => {
                    if pts.len() > 1 {
                        pop_point(pts, &mut self.bulges);
                        self.sources.pop();
                    }
                }
                Input::Keyword(k) if k == "Length" => self.length = true,
                Input::Keyword(k) if k == "Arc" => self.arc = true,
                Input::Keyword(k) if k == "Line" => self.arc = false,
                Input::Keyword(k) if k == "Total" => return self.total(s),
                Input::Keyword(k) => s.echo(format!("{k}: invalid option keyword")),
                Input::Enter => return self.total(s),
                _ => {}
            }
            return Ok(Step::Continue);
        }
        match (self.first, i) {
            (None, Input::Point(p)) => {
                self.first = Some(p);
                self.first_source = s.picked_viewport;
                Ok(Step::Continue)
            }
            (Some(a), Input::Point(b)) => {
                let (paper, scale) = ((a, b), self.first_source.map_or(1.0, |f| f.scale));
                let (a, b, scale) = match (self.first_source, s.picked_viewport) {
                    (Some(first), Some(second)) if first.viewport == second.viewport => (first.point, second.point, scale),
                    _ => (a, b, 1.0),
                };
                let flat = a.dist(b);
                let (za, zb) = if self.first_source.is_some() || s.picked_viewport.is_some() { (0.0, 0.0) } else { (z_at(s, a), z_at(s, b)) };
                let (a, b) = (Vec3::new(a.x, a.y, za), Vec3::new(b.x, b.y, zb));
                if !(b - a).len().is_finite() {
                    // The first point stays LASTPOINT, as after a measurement.
                    s.last_point = paper.0;
                    return Err(bad("dist", DIST_TOO_LARGE));
                }
                s.last_used.distance = flat;
                let t = dist_text(s, a, b);
                for l in t.lines() {
                    s.echo(l.to_string());
                }
                live::keep(s, &[live::distance(s, paper.0, paper.1, scale)]);
                // As in AutoCAD, the second point doesn't become LASTPOINT: the first stays.
                s.last_point = paper.0;
                Ok(Step::Done)
            }
            (Some(a), Input::Keyword(k)) if k == "Multiple points" => {
                self.multi = Some(vec![a]);
                self.bulges = vec![0.0];
                self.sources = vec![self.first_source];
                Ok(Step::Continue)
            }
            (_, Input::Enter) => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        match &self.multi {
            Some(pts) => {
                let (mut pts, mut bulges) = (pts.clone(), self.bulges.clone());
                push_point(&mut pts, &mut bulges, c, self.arc);
                vec![super::helpers::lwpoly(path(&pts, &bulges, false).vertices, false)]
            }
            None => self.first.map(|a| vec![super::helpers::line(a, c)]).unwrap_or_default(),
        }
    }
    fn labels(&self, s: &Session, c: Vec2) -> Vec<crate::LiveLabel> {
        if self.length {
            return Vec::new();
        }
        match (&self.multi, self.first) {
            (Some(pts), _) => {
                let (mut path, mut bulges) = (pts.clone(), self.bulges.clone());
                push_point(&mut path, &mut bulges, c, self.arc);
                // Picked through a layout viewport: the cursor's model point at the same scale.
                let model = model_points(&self.sources).and_then(|mut m| {
                    let scale = self.sources.first().copied().flatten()?.scale;
                    let next = *m.last()? + (c - *pts.last()?) / scale;
                    m.push(next);
                    next.is_finite().then_some(m)
                });
                live::running(s, &path, &bulges, model.as_deref())
            }
            (None, Some(a)) => vec![live::distance(s, a, c, self.first_source.map_or(1.0, |f| f.scale))],
            (None, None) => Vec::new(),
        }
    }
}

impl DistM {
    fn total(&self, s: &mut Session) -> Result<Step> {
        let model = model_points(&self.sources);
        let d = model.as_deref().or(self.multi.as_deref()).map_or(0.0, |p| path_len(p, &self.bulges));
        if !d.is_finite() {
            return Err(bad("dist", DIST_TOO_LARGE));
        }
        s.echo(distance_line(s, d));
        // DISTANCE: the total of Multiple points.
        s.last_used.distance = d;
        if let Some(pts) = &self.multi {
            let labels = live::running(s, pts, &self.bulges, model.as_deref());
            live::keep(s, &labels);
        }
        Ok(Step::Done)
    }
}

struct IdM;

impl Interactive for IdM {
    fn name(&self) -> &'static str {
        "ID"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Specify point", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if let Input::Point(p) = i {
            let m = id_text(s, Vec3::new(p.x, p.y, z_at(s, p)));
            s.echo(m);
        }
        Ok(Step::Done)
    }
}

#[derive(Default)]
struct ListM {
    sel: super::machines::SelectPhase,
}

impl Interactive for ListM {
    fn name(&self) -> &'static str {
        "LIST"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = super::machines::SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            for h in hs {
                let t = list_text(s, h);
                for l in t.lines() {
                    s.echo(l.to_string());
                }
            }
            return Ok(Step::Done);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        self.sel.prompt()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.sel.feed(s, &i)? {
            super::machines::SelOutcome::More => Ok(Step::Continue),
            super::machines::SelOutcome::Empty => Ok(Step::Done),
            super::machines::SelOutcome::Done(hs) => {
                for h in hs {
                    let t = list_text(s, h);
                    for l in t.lines() {
                        s.echo(l.to_string());
                    }
                }
                Ok(Step::Done)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AreaMode {
    /// Measure one area and end.
    #[default]
    Single,
    /// Add each measured area to the running total.
    Add,
    /// Subtract each measured area from the running total.
    Subtract,
}

#[derive(Default)]
struct AreaM {
    pts: Vec<Vec2>,
    mode: AreaMode,
    total: f64,
    /// Selecting objects (the Object option).
    object: bool,
    /// Asking for the length of a segment (the Length option).
    length: bool,
    /// The bulge of the segment from each point to the next (arcs).
    bulges: Vec<f64>,
    /// The Arc option: the next points end arcs.
    arc: bool,
}

impl AreaM {
    fn mode_prefix(&self) -> &'static str {
        match self.mode {
            AreaMode::Single => "",
            AreaMode::Add => "(ADD mode) ",
            AreaMode::Subtract => "(SUBTRACT mode) ",
        }
    }
    /// Report an area (and the running total in add/subtract mode); ends the command in single mode.
    /// An area, perimeter or total past f64's range is not reported or added to the total.
    fn measured(&mut self, s: &mut Session, a: f64, per: f64) -> Step {
        let total = match self.mode {
            AreaMode::Single => self.total,
            AreaMode::Add => self.total + a,
            AreaMode::Subtract => self.total - a,
        };
        if let Some(what) = too_large(a, per).or_else(|| (!total.is_finite()).then_some("total area")) {
            s.echo(format!("The {what} is too large."));
            return if self.mode == AreaMode::Single { Step::Done } else { Step::Continue };
        }
        (s.last_used.area, s.last_used.perimeter) = (a, per);
        s.echo(area_text(s, a, per));
        if self.mode == AreaMode::Single {
            return Step::Done;
        }
        self.total = total;
        // AREA: in Add and Subtract mode the running total.
        s.last_used.area = total;
        s.echo(format!("Total area = {}", fmt_area(s, self.total)));
        Step::Continue
    }
    /// Close the boundary picked so far (Enter or Total at the next-point prompt).
    fn close(&mut self, s: &mut Session) -> Step {
        let pts = std::mem::take(&mut self.pts);
        let bulges = std::mem::take(&mut self.bulges);
        self.arc = false;
        // Two points with an arc between them enclose an area too.
        if pts.len() < 3 && !(pts.len() == 2 && bulges.iter().any(|b| *b != 0.0)) {
            return if self.mode == AreaMode::Single { Step::Done } else { Step::Continue };
        }
        let g = path(&pts, &bulges, true);
        let (a, per) = (g.area().abs(), g.len());
        let labels = live::area(s, &pts, &bulges, self.running_total());
        let step = self.measured(s, a, per);
        live::keep(s, &labels);
        step
    }
    /// The running total and the sign the next area goes into it with (Add and Subtract modes).
    fn running_total(&self) -> Option<(f64, f64)> {
        match self.mode {
            AreaMode::Single => None,
            AreaMode::Add => Some((self.total, 1.0)),
            AreaMode::Subtract => Some((self.total, -1.0)),
        }
    }
}

impl Interactive for AreaM {
    fn name(&self) -> &'static str {
        "AREA"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.object {
            return Prompt::new(format!("{}Select objects", self.mode_prefix()), Accept::SELECT);
        }
        if self.length {
            return length_prompt();
        }
        let next = format!("{}Specify next point", self.mode_prefix());
        match (self.pts.len(), self.mode) {
            (0, AreaMode::Single) => {
                Prompt::new("Specify first corner point", Accept::POINT).kw(&["Object", "Add area", "Subtract area"]).default("Object")
            }
            (0, AreaMode::Add) => Prompt::new("Specify first corner point", Accept::POINT).kw(&["Object", "Subtract area"]),
            (0, AreaMode::Subtract) => Prompt::new("Specify first corner point", Accept::POINT).kw(&["Object", "Add area"]),
            (n, _) if self.arc => {
                let p = arc_prompt(n >= 2).base_opt(self.pts.last().copied());
                Prompt { message: format!("{}{}", self.mode_prefix(), p.message), ..p }
            }
            (1, _) => Prompt::new(next, Accept::POINT).kw(&["Arc", "Length", "Undo"]).base_opt(self.pts.last().copied()),
            _ => Prompt::new(next, Accept::POINT).kw(&["Arc", "Length", "Undo", "Total"]).default("Total").base_opt(self.pts.last().copied()),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.object {
            match i {
                Input::Pick(hs) => {
                    for h in hs {
                        match object_area(s, h) {
                            Ok((a, per)) => {
                                if self.measured(s, a, per) == Step::Done {
                                    return Ok(Step::Done);
                                }
                            }
                            Err(_) => s.echo("The selected object has no area."),
                        }
                    }
                }
                Input::Enter => {
                    self.object = false;
                    if self.mode == AreaMode::Single {
                        return Ok(Step::Done);
                    }
                }
                Input::Text(_) | Input::Keyword(_) => s.echo("*Invalid selection*"),
                _ => {}
            }
            return Ok(Step::Continue);
        }
        if self.length {
            if let Some(p) = length_input(&self.pts, &self.bulges, i)? {
                push_point(&mut self.pts, &mut self.bulges, p, false);
            }
            self.length = false;
            return Ok(Step::Continue);
        }
        match i {
            Input::Point(p) => push_point(&mut self.pts, &mut self.bulges, p, self.arc),
            Input::Keyword(k) => match k.as_str() {
                "Object" => self.object = true,
                "Add area" => self.mode = AreaMode::Add,
                "Subtract area" => self.mode = AreaMode::Subtract,
                "Undo" => pop_point(&mut self.pts, &mut self.bulges),
                "Length" => self.length = true,
                "Arc" => self.arc = true,
                "Line" => self.arc = false,
                "Total" => return Ok(self.close(s)),
                _ => s.echo(format!("{k}: invalid option keyword")),
            },
            Input::Enter if self.pts.is_empty() => match self.mode {
                // Enter takes the default, Object; in add/subtract mode it ends the command.
                AreaMode::Single => self.object = true,
                AreaMode::Add | AreaMode::Subtract => return Ok(Step::Done),
            },
            Input::Enter => return Ok(self.close(s)),
            _ => {}
        }
        Ok(Step::Continue)
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        if self.pts.is_empty() || self.object || self.length {
            return Vec::new();
        }
        let (mut pts, mut bulges) = (self.pts.clone(), self.bulges.clone());
        push_point(&mut pts, &mut bulges, c, self.arc);
        vec![super::helpers::lwpoly(path(&pts, &bulges, true).vertices, true)]
    }
    fn labels(&self, s: &Session, c: Vec2) -> Vec<crate::LiveLabel> {
        if self.pts.is_empty() || self.object || self.length {
            return Vec::new();
        }
        let (mut pts, mut bulges) = (self.pts.clone(), self.bulges.clone());
        push_point(&mut pts, &mut bulges, c, self.arc);
        live::area(s, &pts, &bulges, self.running_total())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_round_exact_halves_up() {
        let mut s = Session::new();
        s.cmdline("SETVAR LUPREC 0").unwrap();
        assert_eq!(fmt_d(&s, 2.5), "3");
        assert_eq!(fmt_n(&s, 2.5), "3");
        assert_eq!(fmt_area(&s, 0.5), "1");
        assert_eq!(fmt_area(&s, 3.5), "4");
    }

    /// #1440 review: a value one ulp past its bound (a border rectangle's shoelace area against
    /// the extents' `width * height`) is rounding, not a bug; a value clearly past it still is.
    #[test]
    fn implausible_allows_rounding_but_not_a_real_excess() {
        let bound: f64 = 100.0;

        let mut check = SelfCheck::new();
        check.implausible(Handle::default(), "Polyline", "area", bound.next_up(), bound, "past the bound");
        assert!(check.implausible.is_empty(), "one ulp past the bound is rounding: {}", check.to_json());

        let mut check = SelfCheck::new();
        check.implausible(Handle::default(), "Polyline", "area", bound * (1.0 + 1e-6), bound, "past the bound");
        assert_eq!(check.implausible.len(), 1, "a real excess is still flagged: {}", check.to_json());

        // A bound that is not finite means no bound applies: the value is not flagged.
        let mut check = SelfCheck::new();
        check.implausible(Handle::default(), "Region", "area", 1e9, f64::INFINITY, "past the bound");
        assert!(check.implausible.is_empty(), "an infinite bound applies to nothing: {}", check.to_json());
    }
}
