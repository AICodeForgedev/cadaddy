//! Sections and drawing views of the 3D model: SECTIONPLANE (a plane that can cut the model
//! live), LIVESECTION, SECTIONPLANETOBLOCK (the 2D section — cut faces outlined and hatched, the
//! model beyond with hidden lines removed — as a block), FLATSHOT (a 2D drawing of the model
//! seen from the view, as a block), and VIEWBASE / VIEWSECTION (drawing views on a layout).

use cadaddy_doc::color::Color;
use cadaddy_doc::{
    BoolOp, Common, Entity, EntityKind, Handle, Hatch, HatchLoop, Insert, Line, LwPolyline, SectionPlane, Space, Text, combine, half_space,
    section_loops,
};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_render::hlr::{Scene3, View, hidden_lines};
use serde_json::{Value, json};
use std::sync::Arc;

use super::machines::{SelOutcome, SelectPhase, number};
use super::{CommandSpec, bad, bool_or, point_param, point_req, str_param, targets};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sectionplane", "Section Plane", run_sectionplane)
            .menu(&["Draw", "Modeling", "Section Plane"])
            .alias(&["splane"])
            .params("{from: [x,y], to: [x,y]} (a vertical plane through them, looking to the right of from→to) | {point: [x,y,z], normal: [nx,ny,nz]} | {orthographic: front|back|top|bottom|left|right, point?}; live? (true), name?")
            .interactive(|_| Ok(Box::new(SectionPlaneM::default()))),
        CommandSpec::new("livesection", "Live Section", run_livesection)
            .menu(&["Modify", "3D Operations", "Live Section"])
            .params("{handle, on?} (toggles when `on` is left out)")
            .interactive(|_| Ok(Box::new(super::machines::SelectRun::single("livesection", "LIVESECTION", "Select section object")))),
        CommandSpec::new("sectionplanetoblock", "Generate Section Block", run_sectionplanetoblock)
            .menu(&["Modify", "3D Operations", "Generate Section"])
            .params("{handle (section plane), at?: [x,y] (where the block goes), background?: true (the model beyond, hidden lines removed), hidden?: false, name?}")
            .interactive(|_| Ok(Box::new(PlaceM::new(Place::Section)))),
        CommandSpec::new("flatshot", "Flatshot", run_flatshot)
            .menu(&["Modify", "3D Operations", "Flatshot"])
            .params("{at?: [x,y], view?: \"current\" | top|bottom|front|back|left|right|swiso|seiso|neiso|nwiso | [dx,dy,dz] (towards the viewer), hidden?: false (also hidden lines), name?}")
            .interactive(|_| Ok(Box::new(PlaceM::new(Place::Flatshot)))),
        CommandSpec::new("viewbase", "Base View", run_viewbase)
            .menu(&["View", "Drawing Views", "Base View"])
            .params("{layout?, view?: front (as FLATSHOT), at: [x,y] (on the layout), scale? (fits a third of the sheet), hidden?}")
            .interactive(|_| Ok(Box::new(PlaceM::new(Place::Base)))),
        CommandSpec::new("viewsection", "Section View", run_viewsection)
            .menu(&["View", "Drawing Views", "Section View"])
            .params("{base: handle (a VIEWBASE view), from: [x,y], to: [x,y] (the cut line, on the layout), at: [x,y], label?: \"A\"}")
            .interactive(|_| Ok(Box::new(ViewSectionM::default()))),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

/// The model's 3D extents (solids and meshes; everything else at its plan extents).
pub(super) fn model_box(s: &Session) -> Result<(Vec3, Vec3)> {
    let d = s.doc()?;
    let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    let mut grow = |p: Vec3| {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    };
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Solid3d(so) => {
                let p = so.props(so.mesh_tol());
                grow(p.min);
                grow(p.max);
            }
            EntityKind::Mesh(m) => m.vertices.iter().take(100_000).for_each(|v| grow(*v)),
            _ => {}
        }
    }
    if lo.x > hi.x {
        let b = d.extents(&Space::Model);
        if b.is_empty() {
            return Ok((Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0)));
        }
        return Ok((Vec3::new(b.min.x, b.min.y, 0.0), Vec3::new(b.max.x, b.max.y, 0.0)));
    }
    Ok((lo, hi))
}

pub(super) fn next_name(s: &Session, prefix: &str) -> Result<String> {
    let d = s.doc()?;
    Ok((1..100_000)
        .map(|i| format!("{prefix}{i}"))
        .find(|n| !d.blocks.keys().any(|k| k.eq_ignore_ascii_case(n)))
        .unwrap_or_else(|| format!("{prefix}x")))
}

/// Define a block of `ents` (base at its lower left) and insert it at `at` in `space`.
fn place_block(
    s: &mut Session,
    name: &str,
    ents: Vec<(Common, EntityKind)>,
    at: Vec2,
    scale: f64,
    space: &Space,
    description: String,
) -> Result<Handle> {
    if ents.is_empty() {
        return Err(other("There is nothing to draw: no 3D objects were found."));
    }
    let mut lo = Vec2::new(f64::MAX, f64::MAX);
    for (_, k) in &ents {
        for p in k.grips() {
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        }
    }
    if !lo.is_finite() {
        lo = Vec2::ZERO;
    }
    let d = s.doc_mut()?;
    let mut blk = cadaddy_doc::Block::new(name);
    blk.base = Vec3::new(lo.x, lo.y, 0.0);
    blk.description = description;
    for (common, kind) in ents {
        let h = d.new_handle();
        blk.entities.push(Entity { handle: h, common, kind });
    }
    d.blocks.insert(name.to_string(), Arc::new(blk));
    let ins = Insert {
        block: name.to_string(),
        insert: Vec3::new(at.x, at.y, 0.0),
        scale: Vec3::new(scale, scale, scale),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    let common = Common { layer: d.header.str("CLAYER", "0"), ..Common::default() };
    Ok(d.add(space, common, EntityKind::Insert(ins))?)
}

pub(super) fn line2(a: Vec2, b: Vec2) -> EntityKind {
    EntityKind::Line(Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) })
}

/// Lines for visible and (when asked) hidden edges; hidden ones dashed where the drawing has
/// HIDDEN loaded, else grey.
fn edge_entities(s: &Session, vis: Vec<[Vec2; 2]>, hid: Vec<[Vec2; 2]>, hidden: bool) -> Vec<(Common, EntityKind)> {
    let dashed = s.doc().ok().and_then(|d| d.linetype("HIDDEN").map(|l| l.name.clone()));
    let hid_common = match dashed {
        Some(lt) => Common { linetype: lt, ..Common::default() },
        None => Common { color: Color::Index(8), ..Common::default() },
    };
    let mut out: Vec<(Common, EntityKind)> = vis.into_iter().map(|[a, b]| (Common::default(), line2(a, b))).collect();
    if hidden {
        out.extend(hid.into_iter().map(|[a, b]| (hid_common.clone(), line2(a, b))));
    }
    out
}

/// A view from its name, a direction, or the current 3D view.
pub(super) fn view_param(s: &Session, v: Option<&Value>, default: &str) -> Result<View> {
    let dir = match v {
        Some(Value::String(n)) if n.eq_ignore_ascii_case("current") => s.state()?.projection().map(|p| p.dir).unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
        Some(Value::String(n)) => cadaddy_geom::view3::preset(&n.to_ascii_lowercase()).ok_or_else(|| other(format!("Unknown view \"{n}\".")))?,
        Some(v @ Value::Array(_)) => super::solids::point3_value(v).ok_or_else(|| other("The view is [dx, dy, dz]."))?,
        _ => cadaddy_geom::view3::preset(default).unwrap_or(Vec3::new(0.0, -1.0, 0.0)),
    };
    View::new(dir).ok_or_else(|| other("The view needs a direction."))
}

/// A drawing of the model from `view`: (visible, hidden) lines.
fn drawing_of(s: &Session, view: &View) -> Result<(Vec<[Vec2; 2]>, Vec<[Vec2; 2]>)> {
    let d = s.doc()?;
    let sc = Scene3::of_space(d, &Space::Model, view, |_| true);
    Ok(hidden_lines(&sc, view))
}

/// A section through the model: its cut faces (outlined and hatched) and, behind them, the model
/// it leaves (hidden lines removed); in the plane's drawing coordinates.
fn section_drawing(s: &Session, sp: &SectionPlane, background: bool, hidden: bool) -> Result<Vec<(Common, EntityKind)>> {
    let (h, v) = sp.axes();
    let view = View { dir: sp.normal * -1.0, right: h, up: v };
    let parts = section_parts(s, sp.origin, sp.normal, &view, background)?;
    let mut out = Vec::new();
    if background {
        out.extend(edge_entities(s, parts.visible, parts.hidden, hidden));
    }
    let (outlines, hatch) = section_faces(s.doc()?, &parts.cut);
    out.extend(outlines.into_iter().map(|k| (Common::default(), k)));
    out.push((Common::default(), hatch));
    Ok(out)
}

/// What a section is drawn from, in `view`'s 2D coordinates.
pub(super) struct SectionParts {
    /// The cut faces' outlines.
    pub cut: Vec<Vec<Vec2>>,
    /// The model beyond the cut (empty without a background): visible and hidden lines.
    pub visible: Vec<[Vec2; 2]>,
    pub hidden: Vec<[Vec2; 2]>,
}

/// The model's solids cut by the plane through `origin` facing `normal` (the side `normal` points
/// to is cut away), seen from `view`.
pub(super) fn section_parts(s: &Session, origin: Vec3, normal: Vec3, view: &View, background: bool) -> Result<SectionParts> {
    let d = s.doc()?;
    let mut cut: Vec<Vec<Vec2>> = Vec::new();
    let mut kept = Scene3::default();
    for e in d.model.iter().filter(|e| d.is_visible(e)) {
        let EntityKind::Solid3d(so) = &e.kind else { continue };
        let tol = so.mesh_tol();
        for l in section_loops(&so.polys(tol), origin, normal) {
            let pts: Vec<Vec2> = l.iter().map(|p| view.project(*p)).collect();
            if pts.len() >= 3 {
                cut.push(pts);
            }
        }
        if background {
            let pr = so.props(tol);
            let size = (pr.max - pr.min).len() + (origin - pr.min).len() + 1.0;
            let part = combine((**so).clone(), half_space(origin, normal * -1.0, size), BoolOp::Intersect);
            if !part.polys(tol).is_empty() {
                kept.add_solid(&part, tol, view);
            }
        }
    }
    if cut.is_empty() {
        return Err(other("The section plane doesn't cut any solid."));
    }
    let (visible, hidden) = if background { hidden_lines(&kept, view) } else { (Vec::new(), Vec::new()) };
    Ok(SectionParts { cut, visible, hidden })
}

/// The cut faces as closed outlines and one ANSI31 hatch of them all.
pub(super) fn section_faces(d: &cadaddy_doc::Drawing, loops: &[Vec<Vec2>]) -> (Vec<EntityKind>, EntityKind) {
    let mut outlines = Vec::new();
    let mut hatch_loops = Vec::new();
    for l in loops {
        let mut pts = l.clone();
        if pts.len() > 3 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.near(*b, 1e-9)) {
            pts.pop();
        }
        let vertices: Vec<PolyVertex> = pts.iter().map(|p| PolyVertex::new(*p)).collect();
        outlines.push(EntityKind::LwPolyline(LwPolyline {
            vertices: vertices.clone(),
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        }));
        hatch_loops.push(HatchLoop { vertices, outer: true, sources: Vec::new(), curves: Vec::new() });
    }
    let spacing =
        cadaddy_doc::library::pattern_for(d, "ANSI31").and_then(|p| p.lines.first().map(|l| l.delta.1.abs())).filter(|x| *x > 0.0).unwrap_or(0.125);
    let b = cadaddy_geom::Bounds2::from_points(loops.iter().flatten().copied());
    let size = b.width().max(b.height()).max(1e-9);
    let scale = (size / 40.0 / spacing).max(1e-6);
    let hatch = EntityKind::Hatch(Hatch {
        pattern: "ANSI31".into(),
        solid: false,
        loops: hatch_loops,
        scale,
        angle: 0.0,
        associative: false,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::ZERO,
        background: None,
        pattern_lines: Vec::new(),
    });
    (outlines, hatch)
}

fn section_of(s: &Session, h: Handle) -> Option<SectionPlane> {
    match &s.doc().ok()?.entity(h)?.kind {
        EntityKind::SectionPlane(sp) => Some(sp.clone()),
        _ => None,
    }
}

fn run_sectionplane(s: &mut Session, p: &Value) -> Result<Value> {
    let (lo, hi) = model_box(s)?;
    let centre = (lo + hi) * 0.5;
    let p3 = |k: &str| p.get(k).and_then(super::solids::point3_value);
    let (origin, normal) = if let (Some(a), Some(b)) = (point_param(p, "from"), point_param(p, "to")) {
        let dd = b - a;
        if dd.len() < 1e-12 {
            return Err(bad("sectionplane", "`from` and `to` must differ"));
        }
        let m = (a + b) * 0.5;
        (Vec3::new(m.x, m.y, centre.z), Vec3::new(dd.y, -dd.x, 0.0).normalized())
    } else if let Some(o) = str_param(p, "orthographic") {
        let n = match o.to_ascii_lowercase().as_str() {
            // Looking at the front: the plane faces back (+Y).
            "front" => Vec3::new(0.0, 1.0, 0.0),
            "back" => Vec3::new(0.0, -1.0, 0.0),
            "top" => Vec3::new(0.0, 0.0, -1.0),
            "bottom" => Vec3::new(0.0, 0.0, 1.0),
            "left" => Vec3::new(1.0, 0.0, 0.0),
            "right" => Vec3::new(-1.0, 0.0, 0.0),
            _ => return Err(bad("sectionplane", "`orthographic` is front, back, top, bottom, left or right")),
        };
        (p3("point").unwrap_or(centre), n)
    } else {
        let o = p3("point").ok_or_else(|| bad("sectionplane", "give `from` and `to`, `point` and `normal`, or `orthographic`"))?;
        let n = p3("normal").map(|n| n.normalized()).filter(|n| n.len() > 0.5).ok_or_else(|| bad("sectionplane", "`normal` is required"))?;
        (o, n)
    };
    // Big enough to show across the model.
    let span = (hi - lo).len().max(1.0) * 1.2;
    let sp = SectionPlane {
        origin,
        normal,
        width: span,
        height: span,
        live: bool_or(p, "live", true),
        name: str_param(p, "name").map(str::to_string).unwrap_or_default(),
    };
    if !sp.is_valid() {
        return Err(bad("sectionplane", "the plane isn't usable"));
    }
    let h = s.add_entity(EntityKind::SectionPlane(sp))?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_livesection(s: &mut Session, p: &Value) -> Result<Value> {
    let h = targets(s, p)?.into_iter().find(|h| section_of(s, *h).is_some()).ok_or_else(|| bad("livesection", "select a section plane"))?;
    let on = p.get("on").and_then(Value::as_bool);
    let mut now = false;
    s.doc_mut()?.modify_entity(h, |e| {
        if let EntityKind::SectionPlane(sp) = &mut e.kind {
            sp.live = on.unwrap_or(!sp.live);
            now = sp.live;
        }
    })?;
    Ok(json!({ "handle": h.hex(), "live": now }))
}

/// Where a generated drawing goes when no point is given: right of the model.
pub(super) fn beside_model(s: &Session) -> Result<Vec2> {
    let (lo, hi) = model_box(s)?;
    Ok(Vec2::new(hi.x + (hi.x - lo.x).max(1.0) * 0.25, lo.y))
}

fn run_sectionplanetoblock(s: &mut Session, p: &Value) -> Result<Value> {
    let h = targets(s, p)?
        .into_iter()
        .find(|h| section_of(s, *h).is_some())
        .ok_or_else(|| bad("sectionplanetoblock", "select a section plane (`handle`)"))?;
    let sp = section_of(s, h).ok_or_else(|| bad("sectionplanetoblock", "not a section plane"))?;
    let ents = section_drawing(s, &sp, bool_or(p, "background", true), bool_or(p, "hidden", false))?;
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() && n.len() <= 255 => n.trim().to_string(),
        _ => next_name(s, "Section")?,
    };
    let at = match point_param(p, "at") {
        Some(a) => a,
        None => beside_model(s)?,
    };
    let ins = place_block(s, &name, ents, at, 1.0, &Space::Model, String::new())?;
    Ok(json!({ "block": name, "handle": ins.hex() }))
}

fn run_flatshot(s: &mut Session, p: &Value) -> Result<Value> {
    // The current view (the plan view: from above) unless one is named.
    let current = json!("current");
    let view = view_param(s, Some(p.get("view").unwrap_or(&current)), "top")?;
    let (vis, hid) = drawing_of(s, &view)?;
    let hidden = bool_or(p, "hidden", false);
    let ents = edge_entities(s, vis, hid, hidden);
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() && n.len() <= 255 => n.trim().to_string(),
        _ => next_name(s, "Flatshot")?,
    };
    let at = match point_param(p, "at") {
        Some(a) => a,
        None => beside_model(s)?,
    };
    let ins = place_block(s, &name, ents, at, 1.0, &Space::Model, String::new())?;
    Ok(json!({ "block": name, "handle": ins.hex() }))
}

/// The layout a drawing view goes on: the one named, else the one shown, else the first.
fn layout_space(s: &Session, p: &Value) -> Result<(Space, f64)> {
    let d = s.doc()?;
    let name = match str_param(p, "layout") {
        Some(n) => {
            d.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(n)).map(|l| l.name.clone()).ok_or_else(|| other(format!("No layout \"{n}\".")))?
        }
        None => match s.layout_space() {
            Space::Paper(n) => n,
            Space::Model => d.layouts.first().map(|l| l.name.clone()).ok_or_else(|| other("The drawing has no layout."))?,
        },
    };
    // The sheet's long side in drawing units (inches in an imperial drawing).
    let mm = d
        .layouts
        .iter()
        .find(|l| l.name == name)
        .map(|l| l.page.width_mm.max(l.page.height_mm))
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(297.0);
    let sheet = if d.header.i64("MEASUREMENT", 1) == 0 { mm / 25.4 } else { mm };
    Ok((Space::Paper(name), sheet))
}

/// What a drawing view remembers (in its block's description): how model points map onto it.
#[derive(serde::Serialize, serde::Deserialize)]
struct ViewInfo {
    dir: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
}

fn run_viewbase(s: &mut Session, p: &Value) -> Result<Value> {
    let (space, sheet) = layout_space(s, p)?;
    let view = view_param(s, p.get("view"), "front")?;
    let at = point_req("viewbase", p, "at")?;
    let (vis, hid) = drawing_of(s, &view)?;
    let b = cadaddy_geom::Bounds2::from_points(vis.iter().flatten().copied());
    let scale = match p.get("scale").and_then(Value::as_f64) {
        Some(sc) if sc.is_finite() && sc > 0.0 && sc < 1e9 => sc,
        Some(_) => return Err(bad("viewbase", "`scale` must be more than zero")),
        None => (sheet / 3.0 / b.width().max(b.height()).max(1e-9)).min(1e6),
    };
    let ents = edge_entities(s, vis, hid, bool_or(p, "hidden", false));
    let name = next_name(s, "VIEW-")?;
    let info = ViewInfo {
        dir: [view.dir.x, view.dir.y, view.dir.z],
        right: [view.right.x, view.right.y, view.right.z],
        up: [view.up.x, view.up.y, view.up.z],
    };
    let desc = serde_json::to_string(&info).unwrap_or_default();
    let ins = place_block(s, &name, ents, at, scale, &space, desc)?;
    Ok(json!({ "handle": ins.hex(), "block": name, "scale": scale }))
}

fn run_viewsection(s: &mut Session, p: &Value) -> Result<Value> {
    let base = p.get("base").and_then(super::curves::handle_value).ok_or_else(|| bad("viewsection", "`base` (a drawing view) is required"))?;
    let (a, b) = (point_req("viewsection", p, "from")?, point_req("viewsection", p, "to")?);
    let at = point_req("viewsection", p, "at")?;
    let label = str_param(p, "label").filter(|l| !l.trim().is_empty() && l.len() <= 16).unwrap_or("A").to_string();
    let d = s.doc()?;
    let e = d.entity(base).ok_or_else(|| bad("viewsection", "`base` isn't in the drawing"))?;
    let EntityKind::Insert(ins) = &e.kind else { return Err(bad("viewsection", "`base` must be a drawing view (from VIEWBASE)")) };
    let space = d.space_of(base).unwrap_or_default();
    let blk = d.block(&ins.block).ok_or_else(|| bad("viewsection", "the view's block is missing"))?;
    let info: ViewInfo = serde_json::from_str(&blk.description).map_err(|_| bad("viewsection", "`base` must be a drawing view (from VIEWBASE)"))?;
    let scale = ins.scale.x;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(bad("viewsection", "the view's scale isn't usable"));
    }
    let v3 = |x: [f64; 3]| Vec3::new(x[0], x[1], x[2]);
    let view = View { dir: v3(info.dir), right: v3(info.right), up: v3(info.up) };
    // Layout point → the view's drawing coordinates → the model.
    let to_model = |q: Vec2| {
        let local = (q - ins.insert.xy()) * (1.0 / scale) + blk.base.xy();
        view.right * local.x + view.up * local.y
    };
    let (ma, mb) = (to_model(a), to_model(b));
    let u = (mb - ma).normalized();
    if u.len() < 0.5 {
        return Err(bad("viewsection", "`from` and `to` must differ"));
    }
    // The cut is square to the view; it looks away from the viewer's right of from→to.
    let normal = u.cross(view.dir).normalized();
    let (lo, hi) = model_box(s)?;
    let span = (hi - lo).len().max(1.0) * 1.2;
    let sp = SectionPlane { origin: (ma + mb) * 0.5, normal, width: span, height: span, live: false, name: label.clone() };
    let ents = section_drawing(s, &sp, true, false)?;
    let name = next_name(s, "VIEW-")?;
    let (h, vv) = sp.axes();
    let info = ViewInfo { dir: [-normal.x, -normal.y, -normal.z], right: [h.x, h.y, h.z], up: [vv.x, vv.y, vv.z] };
    let section = place_block(s, &name, ents, at, scale, &space, serde_json::to_string(&info).unwrap_or_default())?;
    // The cut line on the base view, lettered at its ends, and the view's title.
    let height = 3.5_f64.max(s.doc()?.header.f64("TEXTSIZE", 2.5));
    let d = s.doc_mut()?;
    let common = Common { layer: d.header.str("CLAYER", "0"), ..Common::default() };
    let dash = d.linetype("CENTER").map(|l| l.name.clone());
    let line_common = Common { linetype: dash.unwrap_or_else(|| "ByLayer".into()), ..common.clone() };
    d.add(&space, line_common, line2(a, b))?;
    let text = |at: Vec2, value: String| {
        EntityKind::Text(Text {
            insert: Vec3::new(at.x, at.y, 0.0),
            align_pt: None,
            height,
            value,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        })
    };
    let off = (b - a).normalized() * height;
    d.add(&space, common.clone(), text(a - off, label.clone()))?;
    d.add(&space, common.clone(), text(b + off * 0.5, label.clone()))?;
    d.add(&space, common, text(at - Vec2::new(0.0, height * 2.0), format!("SECTION {label}-{label}")))?;
    Ok(json!({ "handle": section.hex(), "block": name }))
}

// ---------------- prompts ----------------

/// SECTIONPLANE: two points of a vertical plane (or an orthographic one).
#[derive(Default)]
struct SectionPlaneM {
    first: Option<Vec2>,
}

impl Interactive for SectionPlaneM {
    fn name(&self) -> &'static str {
        "SECTIONPLANE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.first {
            None => Prompt::new("Select face or any point to locate section line or", Accept::POINT).kw(&["Orthographic"]),
            Some(f) => Prompt::new("Specify through point", Accept::POINT).base(f),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.first, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (None, Input::Point(p)) => {
                self.first = Some(p);
                Ok(Step::Continue)
            }
            (None, Input::Keyword(k) | Input::Text(k)) => {
                let k = k.trim().to_ascii_lowercase();
                let o = ["front", "back", "top", "bottom", "left", "right"]
                    .into_iter()
                    .find(|o| !k.is_empty() && (o.starts_with(&k) || k.starts_with('o')));
                if k.starts_with('o') {
                    // "Orthographic": front by default.
                    run_sectionplane(s, &json!({ "orthographic": "front" }))?;
                    return Ok(Step::Done);
                }
                match o {
                    Some(o) => {
                        run_sectionplane(s, &json!({ "orthographic": o }))?;
                        Ok(Step::Done)
                    }
                    None => Ok(Step::Continue),
                }
            }
            (Some(f), Input::Point(p)) if !p.near(f, 1e-12) => {
                run_sectionplane(s, &json!({ "from": [f.x, f.y], "to": [p.x, p.y] }))?;
                Ok(Step::Done)
            }
            (None, Input::Enter) => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        self.first.filter(|f| !f.near(c, 1e-12)).map(|f| vec![line2(f, c)]).unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Section,
    Flatshot,
    Base,
}

/// Pick what (a section plane) when needed, then where the drawing goes.
struct PlaceM {
    what: Place,
    sel: SelectPhase,
    picked: Option<Handle>,
}

impl PlaceM {
    fn new(what: Place) -> Self {
        PlaceM { what, sel: SelectPhase::single(), picked: None }
    }
}

impl Interactive for PlaceM {
    fn name(&self) -> &'static str {
        match self.what {
            Place::Section => "SECTIONPLANETOBLOCK",
            Place::Flatshot => "FLATSHOT",
            Place::Base => "VIEWBASE",
        }
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.what == Place::Section && self.picked.is_none() {
            return Prompt::new("Select section object", Accept::SELECT);
        }
        match self.what {
            Place::Base => Prompt::new("Specify location of base view", Accept::POINT),
            _ => Prompt::new("Specify insertion point", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.what == Place::Section && self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = hs.into_iter().find(|h| section_of(s, *h).is_some()),
            }
            if self.picked.is_none() && self.sel.done {
                return Err(other("That is not a section plane."));
            }
            return Ok(Step::Continue);
        }
        let at = match i {
            Input::Point(at) => at,
            // Enter places nothing and ends the command.
            Input::Enter => return Ok(Step::Done),
            _ => return Ok(Step::Continue),
        };
        let r = match self.what {
            Place::Section => run_sectionplanetoblock(s, &json!({ "handle": self.picked.map(|h| h.hex()), "at": [at.x, at.y] })),
            Place::Flatshot => run_flatshot(s, &json!({ "at": [at.x, at.y] })),
            Place::Base => run_viewbase(s, &json!({ "at": [at.x, at.y] })),
        };
        // What can't be drawn here can't be drawn elsewhere either.
        if let Err(e) = r {
            return super::machines::end_with(s, &e);
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

/// VIEWSECTION: the base view, two points of the cut line, where the section goes.
#[derive(Default)]
struct ViewSectionM {
    sel: SelectPhase,
    base: Option<Handle>,
    from: Option<Vec2>,
    to: Option<Vec2>,
}

impl Interactive for ViewSectionM {
    fn name(&self) -> &'static str {
        "VIEWSECTION"
    }
    fn begin(&mut self, _s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::single();
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.base, self.from, self.to) {
            (None, _, _) => Prompt::new("Select parent view", Accept::SELECT),
            (Some(_), None, _) => Prompt::new("Specify start point", Accept::POINT),
            (Some(_), Some(f), None) => Prompt::new("Specify end point", Accept::POINT).base(f),
            _ => Prompt::new("Specify location of section view", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.base.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.base = hs.first().copied(),
            }
            return Ok(Step::Continue);
        }
        let Input::Point(p) = i else {
            // Enter ends the command.
            if i == Input::Enter {
                return Ok(Step::Done);
            }
            if let Input::Text(t) = &i
                && number(t).is_some()
            {
                return Err(other("Requires a point."));
            }
            return Ok(Step::Continue);
        };
        match (self.from, self.to) {
            (None, _) => self.from = Some(p),
            (Some(_), None) => self.to = Some(p),
            (Some(f), Some(t)) => {
                let base = self.base.map(|h| h.hex());
                // A base that isn't a drawing view, or a cut through nothing, stays so wherever
                // the view goes.
                if let Err(e) = run_viewsection(s, &json!({ "base": base, "from": [f.x, f.y], "to": [t.x, t.y], "at": [p.x, p.y] })) {
                    return super::machines::end_with(s, &e);
                }
                s.set_selection(Vec::new());
                return Ok(Step::Done);
            }
        }
        Ok(Step::Continue)
    }
}
