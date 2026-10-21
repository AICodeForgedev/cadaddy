//! 2D drawings from solids: SOLVIEW (Ucs, Ortho, Auxiliary and Section viewports on a layout,
//! each with its NAME-VIS, NAME-HID, NAME-DIM and NAME-HAT layers frozen in every other
//! viewport), SOLDRAW (the visible and hidden profile lines and section hatches of those views)
//! and SOLPROF (the profile of selected solids in the current view, as blocks on PV- and PH-
//! layers).
//!
//! Layout viewports show model space in plan, so a SOLVIEW view's drawing is made flat: SOLDRAW
//! projects the solids into the view's plane and lays the result in model space at the view's own
//! place (right of the model, one place per view), and the viewport looks at that place. What a
//! view is (its direction, place and cut) is kept on its viewport as xdata, so it survives
//! saving.

use cadaddy_doc::color::Color;
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Insert, Layer, Line, RawTag, Space, Viewport};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_render::hlr::{Scene3, View, hidden_lines};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

use super::machines::{SelOutcome, SelectPhase, SelectRun, number};
use super::sections::{model_box, next_name, section_faces, section_parts, view_param};
use super::{CommandSpec, bad, bool_or, point_param, str_param, targets};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// The xdata application a SOLVIEW viewport carries its view under.
const APP: &str = "CADADDY_SOLVIEW";
/// The layer SOLVIEW puts its viewports on.
const VPORTS_LAYER: &str = "VPORTS";
/// The layers of a view, after its name.
const SUFFIXES: [&str; 4] = ["-VIS", "-HID", "-DIM", "-HAT"];
/// Longest view name.
const MAX_NAME: usize = 200;
/// Characters a layer name may not hold.
const BAD_CHARS: &[char] = &['<', '>', '/', '\\', '"', ':', ';', '?', '*', '|', ',', '=', '`'];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("solview", "Setup View", run_solview)
            .menu(&["Draw", "Modeling", "Setup", "View"])
            .params("{type: ucs|ortho|auxiliary|section, name, layout?, scale? (ucs, section; 1), ucs?: current|world (ucs), view? (ucs: a FLATSHOT view instead of the UCS's plan), parent?: viewport handle (ortho, auxiliary, section; found from `side` when left out), side: [x,y] (ortho: a point on the parent's edge; auxiliary, section: the side to view from), from, to: [x,y] (auxiliary: the inclined plane; section: the cutting plane; points on the parent viewport), center: [x,y] (where the view goes on the sheet), p1?, p2?: [x,y] (the viewport's corners; it fits the model when left out)}")
            .interactive(|_| Ok(Box::new(SolviewM::default()))),
        CommandSpec::new("soldraw", "Setup Drawing", run_soldraw)
            .menu(&["Draw", "Modeling", "Setup", "Drawing"])
            .params("{handles?: SOLVIEW viewports (all of the current layout's when left out and nothing is selected)}")
            .interactive(|_| Ok(Box::new(SelectRun::new("soldraw", "SOLDRAW")))),
        CommandSpec::new("solprof", "Setup Profile", run_solprof)
            .menu(&["Draw", "Modeling", "Setup", "Profile"])
            .params("{handles: solids, hidden?: true (hidden lines on their own PH- layer; false: every profile line on the PV- layer), project?: true (onto the plane through the UCS origin; false: through the solids), tangent?: true (leave out tangential edges), view? (the current view)}")
            .interactive(|_| Ok(Box::new(SolprofM::default()))),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

fn v3(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}
fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// What a SOLVIEW viewport shows.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SolInfo {
    name: String,
    /// ucs, ortho, auxiliary or section.
    kind: String,
    dir: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    /// Where the view's drawing lies in model space: the model point shown is
    /// `place + view.project(p)`.
    place: [f64; 2],
    /// A section's cut: a point of the plane and its normal (pointing away from the viewer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cut: Option<([f64; 3], [f64; 3])>,
}

impl SolInfo {
    fn view(&self) -> View {
        View { dir: v3(self.dir), right: v3(self.right), up: v3(self.up) }
    }
    fn place(&self) -> Vec2 {
        Vec2::new(self.place[0], self.place[1])
    }
    /// A point in the view's drawing coordinates, in model space.
    fn to_model(&self, p: Vec2) -> Vec2 {
        p + self.place()
    }
    fn layer(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.name)
    }
}

/// The view a viewport's xdata holds, if it is a SOLVIEW viewport.
fn info_of(c: &Common) -> Option<SolInfo> {
    let mut tags = c.xdata.iter().skip_while(|t| !(t.code == 1001 && t.value == APP)).skip(1);
    let mut text = String::new();
    for t in tags.by_ref() {
        match t.code {
            1001 => break,
            1000 => text.push_str(&t.value),
            _ => {}
        }
    }
    serde_json::from_str(&text).ok()
}

/// `xdata` with the view stored (in pieces short enough for a DXF string).
fn store_info(xdata: &mut Vec<RawTag>, info: &SolInfo) {
    // Drop an earlier copy.
    if let Some(start) = xdata.iter().position(|t| t.code == 1001 && t.value == APP) {
        let end = xdata.iter().skip(start + 1).position(|t| t.code == 1001).map(|n| start + 1 + n).unwrap_or(xdata.len());
        xdata.drain(start..end);
    }
    let text = serde_json::to_string(info).unwrap_or_default();
    xdata.push(RawTag { code: 1001, value: APP.into() });
    let chars: Vec<char> = text.chars().collect();
    for piece in chars.chunks(200) {
        xdata.push(RawTag { code: 1000, value: piece.iter().collect() });
    }
}

/// Every SOLVIEW viewport in the drawing: (layout, handle, viewport, view).
fn solviews(d: &Drawing) -> Vec<(String, Handle, Viewport, SolInfo)> {
    let mut out = Vec::new();
    for l in &d.layouts {
        for e in l.entities.iter() {
            if let EntityKind::Viewport(v) = &e.kind
                && let Some(i) = info_of(&e.common)
            {
                out.push((l.name.clone(), e.handle, v.clone(), i));
            }
        }
    }
    out
}

/// Paper units per model unit of a viewport.
fn vp_scale(v: &Viewport) -> Option<f64> {
    let k = v.height / v.view_height;
    (k.is_finite() && k > 0.0).then_some(k)
}

/// A paper point on a SOLVIEW viewport in its view's drawing coordinates.
fn paper_to_view(v: &Viewport, i: &SolInfo, q: Vec2) -> Option<Vec2> {
    let k = vp_scale(v)?;
    Some((q - v.center.xy()) * (1.0 / k) + v.view_center - i.place())
}

/// A view's drawing point on the sheet.
fn view_to_paper(v: &Viewport, i: &SolInfo, p: Vec2) -> Option<Vec2> {
    let k = vp_scale(v)?;
    Some((i.to_model(p) - v.view_center) * k + v.center.xy())
}

/// The view folded off `parent` across an edge running along `edge` (a unit vector in the
/// parent's drawing), looking from the `side` (a unit vector square to it): as a drawing's
/// adjacent views unfold, the parent's line of sight becomes the new view's direction back
/// towards the parent.
fn fold(parent: &View, edge: Vec2, side: Vec2) -> Option<View> {
    let to3 = |q: Vec2| parent.right * q.x + parent.up * q.y;
    let dir = to3(side).normalized();
    let e3 = to3(edge);
    let back = parent.dir * -1.0;
    let right = (e3 * edge.x + back * side.x).normalized();
    let up = (e3 * edge.y + back * side.y).normalized();
    let ok = |v: Vec3| v.is_finite() && v.len() > 0.5;
    (ok(dir) && ok(right) && ok(up)).then_some(View { dir, right, up })
}

/// A view name usable for layers, not taken by another SOLVIEW view.
fn check_name(d: &Drawing, name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(bad("solview", "`name` (the view's name) is required"));
    }
    if n.chars().count() > MAX_NAME || n.contains(BAD_CHARS) {
        return Err(bad("solview", format!("\"{n}\" can't name layers")));
    }
    if solviews(d).iter().any(|(_, _, _, i)| i.name.eq_ignore_ascii_case(n)) {
        return Err(bad("solview", format!("A view named \"{n}\" already exists.")));
    }
    Ok(n.to_string())
}

/// The layout a new view goes on: the one named, else the one shown, else the first.
fn layout_of(s: &Session, p: &Value) -> Result<String> {
    let d = s.doc()?;
    if let Some(n) = str_param(p, "layout") {
        return d.layout(n).map(|l| l.name.clone()).ok_or_else(|| bad("solview", format!("No layout \"{n}\".")));
    }
    match s.layout_space() {
        Space::Paper(n) => Ok(n),
        Space::Model => d.layouts.first().map(|l| l.name.clone()).ok_or_else(|| other("The drawing has no layout.")),
    }
}

/// A SOLVIEW viewport given by `parent`, or the one whose edge or inside holds `near`.
fn parent_of(s: &Session, layout: &str, p: &Value, near: Option<Vec2>) -> Result<(Handle, Viewport, SolInfo)> {
    let views: Vec<_> = solviews(s.doc()?).into_iter().filter(|(l, ..)| l == layout).collect();
    if let Some(h) = p.get("parent").and_then(super::curves::handle_value) {
        return views
            .into_iter()
            .find(|(_, x, ..)| *x == h)
            .map(|(_, h, v, i)| (h, v, i))
            .ok_or_else(|| bad("solview", "`parent` is not a SOLVIEW viewport on this layout"));
    }
    let q = near.ok_or_else(|| bad("solview", "`parent` (a SOLVIEW viewport) is required"))?;
    views
        .into_iter()
        .filter_map(|(_, h, v, i)| {
            let r = vp_rect(&v);
            let tol = r.width().max(r.height()) * 0.05;
            let grown = Bounds2::new(r.min - Vec2::new(tol, tol), r.max + Vec2::new(tol, tol));
            grown.contains(q).then(|| (edge_distance(&r, q), h, v, i))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, h, v, i)| (h, v, i))
        .ok_or_else(|| other("Viewport selected is not a SOLVIEW viewport."))
}

fn vp_rect(v: &Viewport) -> Bounds2 {
    let c = v.center.xy();
    let h = Vec2::new(v.width / 2.0, v.height / 2.0);
    Bounds2::new(c - h, c + h)
}

fn edge_distance(r: &Bounds2, q: Vec2) -> f64 {
    let dx = (q.x - r.min.x).abs().min((q.x - r.max.x).abs());
    let dy = (q.y - r.min.y).abs().min((q.y - r.max.y).abs());
    dx.min(dy)
}

/// The eight corners of the model's box.
fn model_corners(s: &Session) -> Result<[Vec3; 8]> {
    let (lo, hi) = model_box(s)?;
    let c = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
    Ok([
        c(false, false, false),
        c(true, false, false),
        c(false, true, false),
        c(true, true, false),
        c(false, false, true),
        c(true, false, true),
        c(false, true, true),
        c(true, true, true),
    ])
}

fn scale_param(p: &Value, default: f64) -> Result<f64> {
    match p.get("scale") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => match super::layout::parse_scale(v) {
            Some(k) if k.is_finite() && k > 1e-9 && k < 1e9 => Ok(k),
            _ => Err(bad("solview", "`scale` must be more than zero")),
        },
    }
}

fn run_solview(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "type").unwrap_or("ucs").to_ascii_lowercase();
    let kind = match kind.as_str() {
        "u" | "ucs" => "ucs",
        "o" | "ortho" => "ortho",
        "a" | "aux" | "auxiliary" => "auxiliary",
        "s" | "section" => "section",
        _ => return Err(bad("solview", "`type` is ucs, ortho, auxiliary or section")),
    };
    let name = check_name(s.doc()?, str_param(p, "name").unwrap_or(""))?;
    let layout = layout_of(s, p)?;
    let corners = model_corners(s)?;
    let (lo, hi) = model_box(s)?;
    let centre = (lo + hi) * 0.5;
    let side = point_param(p, "side");
    let mut cut = None;
    // The view, its scale, and (for an orthographic view) the sheet axis it shares with its parent.
    let (view, scale, align) = match kind {
        "ucs" => {
            let view = if p.get("view").is_some_and(|v| !v.is_null()) {
                view_param(s, p.get("view"), "top")?
            } else {
                let a = match str_param(p, "ucs").map(str::to_ascii_lowercase).as_deref() {
                    None | Some("current" | "c") => s.ucs().angle,
                    Some("world" | "w") => 0.0,
                    Some(_) => return Err(bad("solview", "`ucs` is current or world")),
                };
                let right = Vec3::new(a.cos(), a.sin(), 0.0);
                View { dir: Vec3::new(0.0, 0.0, 1.0), right, up: Vec3::new(-a.sin(), a.cos(), 0.0) }
            };
            (view, scale_param(p, 1.0)?, None)
        }
        "ortho" => {
            let q = side.ok_or_else(|| bad("solview", "`side` (a point on the parent viewport's edge) is required"))?;
            let (_, pv, pi) = parent_of(s, &layout, p, Some(q))?;
            let r = vp_rect(&pv);
            // The nearest edge: its outward direction is the side the new view lies on.
            let d = [(q.x - r.min.x).abs(), (q.x - r.max.x).abs(), (q.y - r.min.y).abs(), (q.y - r.max.y).abs()];
            let nearest = (0..4).min_by(|a, b| d.get(*a).unwrap_or(&f64::MAX).total_cmp(d.get(*b).unwrap_or(&f64::MAX))).unwrap_or(3);
            let (edge, out) = match nearest {
                0 => (Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0)),
                1 => (Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)),
                2 => (Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0)),
                _ => (Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)),
            };
            let view = fold(&pi.view(), edge, out).ok_or_else(|| other("That view can't be folded."))?;
            let k = vp_scale(&pv).ok_or_else(|| other("The parent viewport has no scale."))?;
            let at = view_to_paper(&pv, &pi, pi.view().project(centre)).ok_or_else(|| other("The parent viewport has no scale."))?;
            (view, k, Some((at, out)))
        }
        _ => {
            let (a, b) = (
                point_param(p, "from").ok_or_else(|| bad("solview", "`from` ([x, y] on the parent viewport) is required"))?,
                point_param(p, "to").ok_or_else(|| bad("solview", "`to` ([x, y] on the parent viewport) is required"))?,
            );
            let q = side.ok_or_else(|| bad("solview", "`side` (the side to view from) is required"))?;
            let (_, pv, pi) = parent_of(s, &layout, p, Some(a))?;
            let pview = pi.view();
            let (a2, b2) = match (paper_to_view(&pv, &pi, a), paper_to_view(&pv, &pi, b), paper_to_view(&pv, &pi, q)) {
                (Some(a2), Some(b2), Some(q2)) => {
                    let edge = (b2 - a2).normalized();
                    if !(edge.is_finite() && edge.len() > 0.5) {
                        return Err(bad("solview", "`from` and `to` must differ"));
                    }
                    let mut out = Vec2::new(-edge.y, edge.x);
                    if (q2 - a2).dot(out) < 0.0 {
                        out = out * -1.0;
                    }
                    let view = fold(&pview, edge, out).ok_or_else(|| other("That view can't be folded."))?;
                    ((view, edge, out), b2)
                }
                _ => return Err(other("The parent viewport has no scale.")),
            };
            let (view, _, _) = a2;
            let k = vp_scale(&pv).ok_or_else(|| other("The parent viewport has no scale."))?;
            let scale = if kind == "section" { scale_param(p, k)? } else { k };
            if kind == "section" {
                // The cutting plane holds the line (at the model's middle depth) and the parent's
                // line of sight; what lies on the viewer's side of it is cut away.
                let depth = centre.dot(pview.dir);
                let first = paper_to_view(&pv, &pi, a).unwrap_or(b2);
                let origin = pview.right * first.x + pview.up * first.y + pview.dir * depth;
                cut = Some((arr(origin), arr(view.dir * -1.0)));
            }
            (view, scale, None)
        }
    };
    // Each view's drawing has its own place, right of the model.
    let d = s.doc()?;
    let count = solviews(d).len() as f64;
    let diag = (hi - lo).len().max(1.0);
    let target = Vec2::new(hi.x + diag * (2.0 + 2.5 * count), (lo.y + hi.y) * 0.5);
    let place = target - view.project(centre);
    let info = SolInfo {
        name: name.clone(),
        kind: kind.into(),
        dir: arr(view.dir),
        right: arr(view.right),
        up: arr(view.up),
        place: [place.x, place.y],
        cut,
    };
    // Where the model's middle goes on the sheet; an orthographic view lines up with its parent.
    let mut at = match point_param(p, "center") {
        Some(c) => c,
        None => return Err(bad("solview", "`center` ([x, y], where the view goes on the sheet) is required")),
    };
    if let Some((parent_at, out)) = align {
        if out.x.abs() > 0.5 {
            at.y = parent_at.y;
        } else {
            at.x = parent_at.x;
        }
    }
    // The viewport: the corners given, else the model's projection with a margin.
    let rect = match (point_param(p, "p1"), point_param(p, "p2")) {
        (Some(a), Some(b)) => Bounds2::new(a, b),
        _ => {
            let b = Bounds2::from_points(corners.iter().map(|c| view.project(*c) - view.project(centre)));
            let half = Vec2::new(b.width().max(1e-6), b.height().max(1e-6)) * (0.6 * scale);
            Bounds2::new(at - half, at + half)
        }
    };
    if !(rect.min.is_finite() && rect.max.is_finite()) || rect.width() < 1e-6 || rect.height() < 1e-6 {
        return Err(bad("solview", "the viewport has no area"));
    }
    if rect.width() > 1e7 || rect.height() > 1e7 {
        return Err(bad("solview", "the viewport is too large"));
    }
    let vc = info.to_model(view.project(centre)) + (rect.center() - at) * (1.0 / scale);
    let others: Vec<String> = solviews(d).iter().flat_map(|(.., i)| SUFFIXES.map(|x| i.layer(x))).collect();
    let mine: Vec<String> = SUFFIXES.map(|x| info.layer(x)).to_vec();
    let id = super::clip_partners::next_viewport_id(d);
    let mut frozen: Vec<String> = d.layers.iter().filter(|l| l.vp_freeze_new).map(|l| l.name.clone()).collect();
    frozen.extend(others.into_iter().filter(|n| !mine.iter().any(|m| m.eq_ignore_ascii_case(n))));
    let vp = Viewport {
        center: rect.center().to3(0.0),
        width: rect.width(),
        height: rect.height(),
        view_center: vc,
        view_height: rect.height() / scale,
        id,
        frozen_layers: frozen,
        ..Viewport::default()
    };
    let d = s.doc_mut()?;
    ensure_layers(d, &info);
    // The new view's layers are frozen in every other viewport.
    let all: Vec<Handle> =
        d.layouts.iter().flat_map(|l| l.entities.iter()).filter(|e| matches!(e.kind, EntityKind::Viewport(_))).map(|e| e.handle).collect();
    for h in all {
        d.modify_entity(h, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                for m in &mine {
                    if !v.frozen_layers.iter().any(|f| f.eq_ignore_ascii_case(m)) {
                        v.frozen_layers.push(m.clone());
                    }
                }
            }
        })?;
    }
    d.ensure_layer(VPORTS_LAYER);
    let mut common = Common { layer: VPORTS_LAYER.into(), ..Common::default() };
    store_info(&mut common.xdata, &info);
    let h = d.add(&Space::Paper(layout.clone()), common, EntityKind::Viewport(vp))?;
    Ok(json!({ "handle": h.hex(), "name": name, "layout": layout, "scale": scale, "layers": mine }))
}

/// HIDDEN in the drawing (from our linetype library) if it isn't yet; its name if it can be.
fn hidden_linetype(d: &mut Drawing) -> Option<String> {
    if let Some(l) = d.linetype("HIDDEN") {
        return Some(l.name.clone());
    }
    let lt = cadaddy_doc::library::standard_linetypes_for(d).into_iter().find(|l| l.name.eq_ignore_ascii_case("HIDDEN"))?;
    let name = lt.name.clone();
    d.linetypes.push(lt);
    Some(name)
}

/// Add a layer with a colour and linetype if it is missing.
fn add_layer(d: &mut Drawing, name: &str, color: u8, linetype: Option<&str>) {
    if d.layer(name).is_some() {
        return;
    }
    let mut l = Layer::new(name);
    l.color = Color::Index(color);
    if let Some(lt) = linetype {
        l.linetype = lt.to_string();
    }
    d.layers.push(l);
}

fn ensure_layers(d: &mut Drawing, i: &SolInfo) {
    let hidden = hidden_linetype(d);
    add_layer(d, &i.layer("-VIS"), 7, None);
    add_layer(d, &i.layer("-HID"), 7, hidden.as_deref());
    add_layer(d, &i.layer("-DIM"), 7, None);
    add_layer(d, &i.layer("-HAT"), 7, None);
}

/// Most segments [`tidy`] merges (beyond them the lines are left as they are).
const MAX_TIDY: usize = 500_000;

/// One straight line's pieces: (direction angle, offset from the origin, (start, end) along it).
type Run = (f64, f64, f64, f64);

fn run_of([a, b]: [Vec2; 2]) -> Option<Run> {
    let mut d = (b - a).normalized();
    if !(d.is_finite() && d.len() > 0.5) {
        return None;
    }
    // One direction per line, snapped to the axes so nearly vertical lines don't split.
    if d.x.abs() < 1e-9 {
        d = Vec2::new(0.0, 1.0);
    } else if d.y.abs() < 1e-9 {
        d = Vec2::new(1.0, 0.0);
    } else if d.x < 0.0 {
        d = d * -1.0;
    }
    let (t0, t1) = (a.dot(d), b.dot(d));
    Some((d.y.atan2(d.x), d.x * a.y - d.y * a.x, t0.min(t1), t0.max(t1)))
}

/// Pieces on one line that overlap or touch, joined: sorted by line, then along it.
fn merged(mut runs: Vec<Run>, tol: f64) -> Vec<Run> {
    runs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.total_cmp(&b.2)));
    // Group the same lines (chained within the tolerance), then join each group's intervals.
    let mut out: Vec<Run> = Vec::new();
    let mut group: Vec<Run> = Vec::new();
    let flush = |group: &mut Vec<Run>, out: &mut Vec<Run>| {
        group.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut cur: Option<Run> = None;
        for r in group.drain(..) {
            cur = match cur {
                Some(c) if r.2 <= c.3 + tol => Some((c.0, c.1, c.2, c.3.max(r.3))),
                Some(c) => {
                    out.push(c);
                    Some(r)
                }
                None => Some(r),
            };
        }
        out.extend(cur);
    };
    for r in runs {
        if group.last().is_some_and(|g| (g.0 - r.0).abs() > 1e-9 || (g.1 - r.1).abs() > tol) {
            flush(&mut group, &mut out);
        }
        group.push(r);
    }
    flush(&mut group, &mut out);
    out
}

/// `hidden` pieces with the parts lying under a visible line taken away.
fn uncovered(hidden: Vec<Run>, visible: &[Run], tol: f64) -> Vec<Run> {
    let mut out = Vec::new();
    for h in hidden {
        let mut parts = vec![(h.2, h.3)];
        for v in visible.iter().filter(|v| (v.0 - h.0).abs() <= 1e-9 && (v.1 - h.1).abs() <= tol) {
            parts = parts
                .into_iter()
                .flat_map(|(a, b)| {
                    let mut keep = Vec::new();
                    if v.2 > a + tol {
                        keep.push((a, b.min(v.2)));
                    }
                    if v.3 < b - tol {
                        keep.push((a.max(v.3), b));
                    }
                    if v.3 <= a || v.2 >= b {
                        keep = vec![(a, b)];
                    }
                    keep
                })
                .collect();
        }
        out.extend(parts.into_iter().filter(|(a, b)| b - a > tol).map(|(a, b)| (h.0, h.1, a, b)));
    }
    out
}

fn segment(r: &Run) -> [Vec2; 2] {
    // The axes exactly (cos(π/2) is not quite zero).
    let d = if r.0 == 0.0 {
        Vec2::new(1.0, 0.0)
    } else if r.0 == std::f64::consts::FRAC_PI_2 {
        Vec2::new(0.0, 1.0)
    } else {
        Vec2::new(r.0.cos(), r.0.sin())
    };
    let n = Vec2::new(-d.y, d.x);
    [n * r.1 + d * r.2, n * r.1 + d * r.3]
}

/// A drawing's lines tidied: pieces of one straight edge joined into one line (a hole's rim
/// seen edge on lies along the face's edge), and hidden pieces under visible lines left out.
fn tidy(vis: Vec<[Vec2; 2]>, hid: Vec<[Vec2; 2]>) -> (Vec<[Vec2; 2]>, Vec<[Vec2; 2]>) {
    if vis.len() + hid.len() > MAX_TIDY {
        return (vis, hid);
    }
    let size = Bounds2::from_points(vis.iter().chain(&hid).flatten().copied());
    let tol = 1e-7 * (size.width().max(size.height()).max(0.0) + 1.0);
    let runs = |v: &[[Vec2; 2]]| v.iter().filter_map(|s| run_of(*s)).collect::<Vec<Run>>();
    let v = merged(runs(&vis), tol);
    let h = merged(uncovered(runs(&hid), &v, tol), tol);
    (v.iter().map(segment).collect(), h.iter().map(segment).collect())
}

fn line_at(a: Vec2, b: Vec2) -> EntityKind {
    EntityKind::Line(Line { a: a.to3(0.0), b: b.to3(0.0) })
}

fn run_soldraw(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let views = solviews(d);
    let mut picked: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| views.iter().any(|(_, x, ..)| x == h)).collect();
    if picked.is_empty() {
        if p.get("handles").is_some() || p.get("handle").is_some() {
            return Err(bad("soldraw", "select SOLVIEW viewports"));
        }
        // Nothing picked: every view on the current layout.
        let here = match s.layout_space() {
            Space::Paper(n) => Some(n),
            Space::Model => None,
        };
        picked = views.iter().filter(|(l, ..)| here.as_ref().is_none_or(|h| h == l)).map(|(_, h, ..)| *h).collect();
    }
    if picked.is_empty() {
        return Err(other("There are no SOLVIEW viewports to draw."));
    }
    let mut done = Vec::new();
    for h in picked {
        let Some((_, _, _, info)) = views.iter().find(|(_, x, ..)| *x == h) else { continue };
        let view = info.view();
        // The drawing in view coordinates: (layer suffix, entity).
        let mut ents: Vec<(&str, EntityKind)> = Vec::new();
        let by = info.place();
        let (vis, hid) = match info.cut {
            Some((o, n)) => {
                let parts = section_parts(s, v3(o), v3(n), &view, true)?;
                let cut: Vec<Vec<Vec2>> = parts.cut.iter().map(|l| l.iter().map(|q| *q + by).collect()).collect();
                let (outlines, hatch) = section_faces(s.doc()?, &cut);
                ents.extend(outlines.into_iter().map(|k| ("-VIS", k)));
                ents.push(("-HAT", hatch));
                (parts.visible, parts.hidden)
            }
            None => {
                let d = s.doc()?;
                let sc = Scene3::of_space(d, &Space::Model, &view, |e| matches!(e.kind, EntityKind::Solid3d(_) | EntityKind::Mesh(_)));
                hidden_lines(&sc, &view)
            }
        };
        let (vis, hid) = tidy(vis, hid);
        let (nv, nh) = (vis.len(), hid.len());
        ents.extend(vis.into_iter().map(|[a, b]| ("-VIS", line_at(a + by, b + by))));
        ents.extend(hid.into_iter().map(|[a, b]| ("-HID", line_at(a + by, b + by))));
        let d = s.doc_mut()?;
        ensure_layers(d, info);
        // A view drawn again replaces what it had.
        let old: Vec<Handle> = d
            .model
            .iter()
            .filter(|e| ["-VIS", "-HID", "-HAT"].iter().any(|x| e.common.layer.eq_ignore_ascii_case(&info.layer(x))))
            .map(|e| e.handle)
            .collect();
        for o in old {
            d.remove_entity(o);
        }
        let hatches = ents.iter().filter(|(_, k)| matches!(k, EntityKind::Hatch(_))).count();
        for (suffix, k) in ents {
            let common = Common { layer: info.layer(suffix), ..Common::default() };
            d.add(&Space::Model, common, k)?;
        }
        done.push(json!({ "viewport": h.hex(), "name": info.name, "visible": nv, "hidden": nh, "hatches": hatches }));
    }
    Ok(json!({ "views": done }))
}

fn run_solprof(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let solids: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| d.entity(*h).is_some_and(|e| matches!(e.kind, EntityKind::Solid3d(_)))).collect();
    if solids.is_empty() {
        return Err(bad("solprof", "select 3D solids (`handles`)"));
    }
    let view = match p.get("view") {
        Some(v) if !v.is_null() => view_param(s, Some(v), "top")?,
        _ => match s.state()?.projection() {
            Some(pr) => View { dir: pr.dir, right: pr.right, up: pr.up },
            None => {
                let a = s.ucs().angle;
                View { dir: Vec3::new(0.0, 0.0, 1.0), right: Vec3::new(a.cos(), a.sin(), 0.0), up: Vec3::new(-a.sin(), a.cos(), 0.0) }
            }
        },
    };
    let separate = bool_or(p, "hidden", true);
    let project = bool_or(p, "project", true);
    let d = s.doc()?;
    let mut sc = Scene3::default();
    let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    for h in &solids {
        if let Some(EntityKind::Solid3d(so)) = d.entity(*h).map(|e| &e.kind) {
            let tol = so.mesh_tol();
            sc.add_solid(so, tol, &view);
            let pr = so.props(tol);
            lo = Vec3::new(lo.x.min(pr.min.x), lo.y.min(pr.min.y), lo.z.min(pr.min.z));
            hi = Vec3::new(hi.x.max(pr.max.x), hi.y.max(pr.max.y), hi.z.max(pr.max.z));
        }
    }
    let (vis, hid) = hidden_lines(&sc, &view);
    let (vis, hid) = tidy(vis, hid);
    if vis.is_empty() && hid.is_empty() {
        return Err(other("The solids have no profile in this view."));
    }
    // The plane the profile lies in: through the UCS origin, or through the solids' middle.
    let depth = if project {
        s.ucs().origin.to3(0.0).dot(view.dir)
    } else {
        let c = (lo + hi) * 0.5;
        if c.is_finite() { c.dot(view.dir) } else { 0.0 }
    };
    let lift = |q: Vec2| view.right * q.x + view.up * q.y + view.dir * depth;
    let line3 = |[a, b]: [Vec2; 2]| EntityKind::Line(Line { a: lift(a), b: lift(b) });
    // Named after the viewport the profile is seen in (model space: MODEL).
    let key = s.state()?.active_viewport().map(|(h, _)| h.hex()).unwrap_or_else(|| "MODEL".into());
    let (pv, ph) = (format!("PV-{key}"), format!("PH-{key}"));
    let nv = vis.len();
    let nh = hid.len();
    let (vis_ents, hid_ents): (Vec<EntityKind>, Vec<EntityKind>) = if separate {
        (vis.into_iter().map(line3).collect(), hid.into_iter().map(line3).collect())
    } else {
        (vis.into_iter().chain(hid).map(line3).collect(), Vec::new())
    };
    let vblock = next_name(s, &format!("{pv}-"))?;
    let d = s.doc_mut()?;
    let hidden_lt = hidden_linetype(d);
    add_layer(d, &pv, 7, None);
    let mut out = json!({ "visible": nv, "hidden": nh, "layers": [pv] });
    out["blocks"] = json!([profile_block(d, &vblock, &pv, vis_ents)?]);
    if separate && !hid_ents.is_empty() {
        add_layer(d, &ph, 7, hidden_lt.as_deref());
        let hblock = (1..100_000).map(|i| format!("{ph}-{i}")).find(|n| d.block(n).is_none()).ok_or_else(|| other("No free block name."))?;
        let hb = profile_block(d, &hblock, &ph, hid_ents)?;
        out["blocks"] = json!([out["blocks"][0].clone(), hb]);
        out["layers"] = json!([pv, ph]);
    }
    Ok(out)
}

/// A block of profile lines (in place: based at the origin, inserted there) on `layer`.
fn profile_block(d: &mut Drawing, name: &str, layer: &str, ents: Vec<EntityKind>) -> Result<String> {
    let mut blk = cadaddy_doc::Block::new(name);
    for k in ents {
        let h = d.new_handle();
        blk.entities.push(Entity { handle: h, common: Common::default(), kind: k });
    }
    d.blocks.insert(name.to_string(), Arc::new(blk));
    let ins = Insert {
        block: name.to_string(),
        insert: Vec3::ZERO,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    let h = d.add(&Space::Model, Common { layer: layer.into(), ..Common::default() }, EntityKind::Insert(ins))?;
    Ok(h.hex())
}

// ---------------- prompts ----------------

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Stage {
    #[default]
    Option,
    Ucs,
    Side,
    First,
    Second,
    ViewFrom,
    Scale,
    Center,
    Corner1,
    Corner2,
    Name,
}

/// SOLVIEW's prompts.
#[derive(Default)]
struct SolviewM {
    stage: Stage,
    kind: &'static str,
    ucs: &'static str,
    scale: Option<f64>,
    side: Option<Vec2>,
    from: Option<Vec2>,
    to: Option<Vec2>,
    center: Option<Vec2>,
    p1: Option<Vec2>,
    p2: Option<Vec2>,
}

impl SolviewM {
    fn params(&self, name: &str) -> Value {
        let pt = |p: Option<Vec2>| p.map(|p| json!([p.x, p.y])).unwrap_or(Value::Null);
        json!({
            "type": self.kind,
            "name": name,
            "ucs": self.ucs,
            "scale": self.scale,
            "side": pt(self.side),
            "from": pt(self.from),
            "to": pt(self.to),
            "center": pt(self.center),
            "p1": pt(self.p1),
            "p2": pt(self.p2),
        })
    }
}

impl Interactive for SolviewM {
    fn name(&self) -> &'static str {
        "SOLVIEW"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Option => Prompt::new("Enter an option", Accept::TEXT).kw(&["Ucs", "Ortho", "Auxiliary", "Section"]),
            Stage::Ucs => Prompt::new("Enter an option", Accept::TEXT).kw(&["World", "Current"]).default("Current"),
            Stage::Side => Prompt::new("Specify side of viewport to project", Accept::POINT),
            Stage::First if self.kind == "section" => Prompt::new("Specify first point of cutting plane", Accept::POINT),
            Stage::First => Prompt::new("Specify first point of inclined plane", Accept::POINT),
            Stage::Second if self.kind == "section" => Prompt::new("Specify second point of cutting plane", Accept::POINT).base_opt(self.from),
            Stage::Second => Prompt::new("Specify second point of inclined plane", Accept::POINT).base_opt(self.from),
            Stage::ViewFrom => Prompt::new("Specify side to view from", Accept::POINT),
            Stage::Scale => Prompt::new("Enter view scale", Accept::NUMBER).default("1"),
            Stage::Center => Prompt::new("Specify view center", Accept::POINT),
            Stage::Corner1 => Prompt::new("Specify first corner of viewport", Accept::POINT),
            Stage::Corner2 => Prompt::new("Specify opposite corner of viewport", Accept::POINT).base_opt(self.p1),
            Stage::Name => Prompt::new("Enter view name", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_ascii_lowercase()),
            _ => None,
        };
        let point = match i {
            Input::Point(p) => Some(p),
            Input::Point3(p) => Some(p.xy()),
            _ => None,
        };
        match self.stage {
            Stage::Option => match (text.as_deref(), &i) {
                (_, Input::Enter) => return Ok(Step::Done),
                (Some(t), _) if !t.is_empty() => {
                    let (kind, next) = match t.chars().next() {
                        Some('u') => ("ucs", Stage::Ucs),
                        Some('o') => ("ortho", Stage::Side),
                        Some('a') => ("auxiliary", Stage::First),
                        Some('s') => ("section", Stage::First),
                        _ => {
                            s.echo("Invalid option keyword.");
                            return Ok(Step::Continue);
                        }
                    };
                    self.kind = kind;
                    self.stage = next;
                }
                _ => {}
            },
            Stage::Ucs => {
                self.ucs = match (text.as_deref(), &i) {
                    (Some(t), _) if t.starts_with('w') => "world",
                    (Some(t), _) if t.starts_with('c') => "current",
                    (_, Input::Enter) => "current",
                    _ => {
                        s.echo("Invalid option keyword.");
                        return Ok(Step::Continue);
                    }
                };
                self.stage = Stage::Scale;
            }
            Stage::Side => {
                if let Some(p) = point {
                    self.side = Some(p);
                    self.stage = Stage::Center;
                }
            }
            Stage::First => {
                if let Some(p) = point {
                    self.from = Some(p);
                    self.stage = Stage::Second;
                }
            }
            Stage::Second => {
                if let Some(p) = point.filter(|p| self.from.is_none_or(|f| !f.near(*p, 1e-12))) {
                    self.to = Some(p);
                    self.stage = Stage::ViewFrom;
                }
            }
            Stage::ViewFrom => {
                if let Some(p) = point {
                    self.side = Some(p);
                    self.stage = if self.kind == "section" { Stage::Scale } else { Stage::Center };
                }
            }
            Stage::Scale => match (text.as_deref(), &i) {
                (_, Input::Enter) => self.stage = Stage::Center,
                (Some(t), _) => match number(t).filter(|k| k.is_finite() && *k > 0.0) {
                    Some(k) => {
                        self.scale = Some(k);
                        self.stage = Stage::Center;
                    }
                    None => s.echo("Requires a positive number."),
                },
                _ => {}
            },
            Stage::Center => {
                if let Some(p) = point {
                    self.center = Some(p);
                    self.stage = Stage::Corner1;
                }
            }
            Stage::Corner1 => {
                if let Some(p) = point {
                    self.p1 = Some(p);
                    self.stage = Stage::Corner2;
                }
            }
            Stage::Corner2 => {
                if let Some(p) = point.filter(|p| self.p1.is_none_or(|f| (f.x - p.x).abs() > 1e-9 && (f.y - p.y).abs() > 1e-9)) {
                    self.p2 = Some(p);
                    self.stage = Stage::Name;
                }
            }
            Stage::Name => {
                if let Input::Text(t) | Input::Keyword(t) = &i {
                    if t.trim().is_empty() {
                        return Ok(Step::Continue);
                    }
                    let r = run_solview(s, &self.params(t))?;
                    if let Some(n) = r.get("name").and_then(Value::as_str) {
                        s.echo(format!("View {n} set up."));
                    }
                    return Ok(Step::Done);
                }
            }
        }
        Ok(Step::Continue)
    }
}

/// SOLPROF: the solids, then its three questions.
#[derive(Default)]
struct SolprofM {
    sel: SelectPhase,
    solids: Vec<Handle>,
    answers: Vec<bool>,
}

const SOLPROF_QUESTIONS: [&str; 3] =
    ["Display hidden profile lines on separate layer?", "Project profile lines onto a plane?", "Delete tangential edges?"];

impl Interactive for SolprofM {
    fn name(&self) -> &'static str {
        "SOLPROF"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.solids = self.sel.picked.clone();
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.solids.is_empty() {
            return self.sel.prompt();
        }
        let q = SOLPROF_QUESTIONS.get(self.answers.len()).copied().unwrap_or("");
        Prompt::new(q, Accept::TEXT).kw(&["Yes", "No"]).default("Y")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.solids.is_empty() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.solids = hs,
            }
            return Ok(Step::Continue);
        }
        let yes = match &i {
            Input::Enter => true,
            Input::Text(t) | Input::Keyword(t) => match t.trim().chars().next().map(|c| c.to_ascii_lowercase()) {
                Some('y') => true,
                Some('n') => false,
                _ => {
                    s.echo("Invalid option keyword.");
                    return Ok(Step::Continue);
                }
            },
            _ => return Ok(Step::Continue),
        };
        self.answers.push(yes);
        if self.answers.len() < SOLPROF_QUESTIONS.len() {
            return Ok(Step::Continue);
        }
        let a = |k: usize| self.answers.get(k).copied().unwrap_or(true);
        let hs: Vec<String> = self.solids.iter().map(|h| h.hex()).collect();
        run_solprof(s, &json!({ "handles": hs, "hidden": a(0), "project": a(1), "tangent": a(2) }))?;
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).len() < 1e-12
    }

    #[test]
    fn views_fold_like_an_unfolded_drawing() {
        let top = View { dir: Vec3::new(0.0, 0.0, 1.0), right: Vec3::new(1.0, 0.0, 0.0), up: Vec3::new(0.0, 1.0, 0.0) };
        // Below the top view: the front view, looking from -Y with Z up.
        let front = fold(&top, Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0)).unwrap();
        assert!(near(front.dir, Vec3::new(0.0, -1.0, 0.0)) && near(front.up, Vec3::new(0.0, 0.0, 1.0)));
        assert!(near(front.right, Vec3::new(1.0, 0.0, 0.0)));
        // Right of the front view: the right side view.
        let side = fold(&front, Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)).unwrap();
        assert!(near(side.dir, Vec3::new(1.0, 0.0, 0.0)) && near(side.up, Vec3::new(0.0, 0.0, 1.0)));
        // Every fold stays right-handed.
        for v in [front, side] {
            assert!(near(v.right.cross(v.up), v.dir));
        }
    }

    #[test]
    fn the_view_survives_in_xdata() {
        let info = SolInfo {
            name: "Front".into(),
            kind: "ortho".into(),
            dir: [0.0, -1.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            place: [12.5, -3.0],
            cut: Some(([1.0, 2.0, 3.0], [0.0, 1.0, 0.0])),
        };
        let mut x = vec![RawTag { code: 1001, value: "OTHER".into() }, RawTag { code: 1000, value: "keep".into() }];
        store_info(&mut x, &info);
        store_info(&mut x, &info);
        assert_eq!(x.iter().filter(|t| t.value == APP).count(), 1);
        assert!(x.iter().all(|t| t.value.chars().count() <= 255));
        let back = info_of(&Common { xdata: x.clone(), ..Common::default() }).unwrap();
        assert_eq!(back.name, "Front");
        assert_eq!(back.place, [12.5, -3.0]);
        assert!(x.iter().any(|t| t.value == "keep"));
        assert!(info_of(&Common::default()).is_none());
    }
}
