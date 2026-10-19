//! 3D editing: 3DMOVE, 3DROTATE, 3DSCALE (with axis constraints and handles shown at the base
//! point), 3DALIGN, 3DMIRROR, 3DARRAY, and the 3D object snap settings (3DOSNAP). Objects are
//! moved in space ([`EntityKind::transform3`]); a flat object a transform would tilt out of its
//! plane is left as it is and counted.

use cadaddy_doc::{Handle, align, mirror_in, rotation_about, scaling, translation};
use cadaddy_geom::{Mat4, Vec3};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase, number};
use super::{CommandSpec, bad, targets};
use crate::prompt::{Gizmo, GizmoKind};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("3dmove", "3D Move", run_3dmove)
            .menu(&["Modify", "3D Operations", "3D Move"])
            .alias(&["3m"])
            .params("{handles?, delta: [dx,dy,dz]} | {handles?, base: [x,y,z], to: [x,y,z]}")
            .interactive(|_| Ok(Box::new(EditM::new(Kind::Move)))),
        CommandSpec::new("3drotate", "3D Rotate", run_3drotate)
            .menu(&["Modify", "3D Operations", "3D Rotate"])
            .alias(&["3r"])
            .params("{handles?, base: [x,y,z], axis: x|y|z | [dx,dy,dz], angle (degrees)}")
            .interactive(|_| Ok(Box::new(EditM::new(Kind::Rotate)))),
        CommandSpec::new("3dscale", "3D Scale", run_3dscale)
            .menu(&["Modify", "3D Operations", "3D Scale"])
            .params("{handles?, base: [x,y,z], factor, axis?: x|y|z | [dx,dy,dz] (only along it)}")
            .interactive(|_| Ok(Box::new(EditM::new(Kind::Scale)))),
        CommandSpec::new("3dalign", "3D Align", run_3dalign)
            .menu(&["Modify", "3D Operations", "3D Align"])
            .alias(&["3al"])
            .params("{handles?, source: [p1, p2?, p3?], dest: [q1, q2?, q3?] ([x,y,z] each), scale?: false}")
            .interactive(|_| Ok(Box::new(AlignM::default()))),
        CommandSpec::new("3dmirror", "3D Mirror", run_3dmirror)
            .menu(&["Modify", "3D Operations", "3D Mirror"])
            .params("{handles?, points: [p1, p2, p3]} | {handles?, plane: xy|yz|zx, point?: [x,y,z]}; erase?: false (keep the originals)")
            .interactive(|_| Ok(Box::new(MirrorM::default()))),
        CommandSpec::new("3darray", "3D Array", run_3darray)
            .menu(&["Modify", "3D Operations", "3D Array"])
            .alias(&["3a"])
            .params("{handles?, rows?, columns?, levels?, rowSpacing?, columnSpacing?, levelSpacing?} | {handles?, polar: {count, angle? (360), rotate? (true), center: [x,y,z], axisEnd: [x,y,z]}}")
            .interactive(|_| Ok(Box::new(ArrayM::default()))),
        CommandSpec::new("3dosnap", "3D Object Snap", run_3dosnap)
            .params("{on?: true|false (toggles when left out), modes?: [vertex, midpoint, center, perpendicular, nearest]}")
            .key("F4")
            .noundo(),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

fn p3(p: &Value, key: &str) -> Option<Vec3> {
    p.get(key).and_then(super::solids::point3_value)
}

fn p3_req(cmd: &str, p: &Value, key: &str) -> Result<Vec3> {
    p3(p, key).ok_or_else(|| bad(cmd, format!("`{key}` ([x, y, z]) is required")))
}

/// An axis: x, y, z or a direction.
fn axis(cmd: &str, v: Option<&Value>) -> Result<Option<Vec3>> {
    let Some(v) = v else { return Ok(None) };
    let a = match v.as_str().map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("x") => Vec3::new(1.0, 0.0, 0.0),
        Some("y") => Vec3::new(0.0, 1.0, 0.0),
        Some("z") => Vec3::new(0.0, 0.0, 1.0),
        Some(_) => return Err(bad(cmd, "`axis` is x, y, z or [dx, dy, dz]")),
        None => super::solids::point3_value(v).ok_or_else(|| bad(cmd, "`axis` is x, y, z or [dx, dy, dz]"))?,
    };
    let a = a.normalized();
    if a.len() < 0.5 {
        return Err(bad(cmd, "the axis needs a direction"));
    }
    Ok(Some(a))
}

/// Apply `m` to the objects (or, with `copy`, to copies of them); the copies' handles and how
/// many objects stayed as they were.
fn apply(s: &mut Session, hs: &[Handle], m: &Mat4, copy: bool) -> Result<(Vec<Handle>, usize)> {
    if hs.is_empty() {
        return Err(other("Select the objects first."));
    }
    let space = s.space();
    let d = s.doc_mut()?;
    let mut made = Vec::new();
    let mut kept = 0usize;
    for h in hs {
        let Some(e) = d.entity(*h) else { continue };
        let mut k = e.kind.clone();
        if !k.transform3(m) {
            kept += 1;
            continue;
        }
        if copy {
            let common = e.common.clone();
            made.push(d.add(&space, common, k)?);
        } else {
            d.modify_entity(*h, |e| e.kind = k)?;
        }
    }
    Ok((made, kept))
}

fn report(kept: usize, out: Value) -> Value {
    let mut out = out;
    if let Some(o) = out.as_object_mut() {
        o.insert("unchanged".into(), json!(kept));
        if kept > 0 {
            o.insert("message".into(), json!(format!("{kept} flat object(s) can't be turned out of their plane and were left as they were.")));
        }
    }
    out
}

fn run_3dmove(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let d = match p3(p, "delta") {
        Some(d) => d,
        None => p3_req("3dmove", p, "to")? - p3_req("3dmove", p, "base")?,
    };
    let (_, kept) = apply(s, &hs, &translation(d), false)?;
    Ok(report(kept, json!({ "moved": hs.len() - kept })))
}

fn run_3drotate(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let base = p3_req("3drotate", p, "base")?;
    let ax = axis("3drotate", p.get("axis"))?.unwrap_or(Vec3::new(0.0, 0.0, 1.0));
    let a = p.get("angle").and_then(Value::as_f64).filter(|a| a.is_finite()).ok_or_else(|| bad("3drotate", "`angle` (degrees) is required"))?;
    let (_, kept) = apply(s, &hs, &rotation_about(base, ax, a.to_radians()), false)?;
    Ok(report(kept, json!({ "rotated": hs.len() - kept })))
}

fn run_3dscale(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let base = p3_req("3dscale", p, "base")?;
    let k = p
        .get("factor")
        .and_then(Value::as_f64)
        .filter(|k| k.is_finite() && *k > 0.0 && *k < 1e9)
        .ok_or_else(|| bad("3dscale", "`factor` must be more than zero"))?;
    let ax = axis("3dscale", p.get("axis"))?;
    let (_, kept) = apply(s, &hs, &scaling(base, k, ax), false)?;
    Ok(report(kept, json!({ "scaled": hs.len() - kept })))
}

fn points(cmd: &str, p: &Value, key: &str) -> Result<Vec<Vec3>> {
    let a = p.get(key).and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{key}` (1 to 3 points) is required")))?;
    if a.is_empty() || a.len() > 3 {
        return Err(bad(cmd, format!("`{key}` holds 1 to 3 points")));
    }
    a.iter().map(|v| super::solids::point3_value(v).ok_or_else(|| bad(cmd, format!("`{key}`: {v} is not a point")))).collect()
}

fn run_3dalign(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let (src, dst) = (points("3dalign", p, "source")?, points("3dalign", p, "dest")?);
    if src.len() != dst.len() {
        return Err(bad("3dalign", "`source` and `dest` need as many points"));
    }
    let m = align(&src, &dst, p.get("scale").and_then(Value::as_bool).unwrap_or(false))
        .ok_or_else(|| other("The points must not coincide or line up."))?;
    let (_, kept) = apply(s, &hs, &m, false)?;
    Ok(report(kept, json!({ "aligned": hs.len() - kept })))
}

/// A mirror plane from three points, or a named plane through a point.
fn mirror_plane(p: &Value) -> Result<(Vec3, Vec3)> {
    if let Some(a) = p.get("points").and_then(Value::as_array) {
        let pts: Vec<Vec3> = a.iter().filter_map(super::solids::point3_value).collect();
        let [p1, p2, p3] = pts[..] else { return Err(bad("3dmirror", "`points` holds three points")) };
        let n = (p2 - p1).cross(p3 - p1).normalized();
        if n.len() < 0.5 {
            return Err(other("The three points line up: they make no plane."));
        }
        return Ok((p1, n));
    }
    let at = p3(p, "point").unwrap_or(Vec3::ZERO);
    let n = match p.get("plane").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        Some("xy") => Vec3::new(0.0, 0.0, 1.0),
        Some("yz") => Vec3::new(1.0, 0.0, 0.0),
        Some("zx") => Vec3::new(0.0, 1.0, 0.0),
        _ => return Err(bad("3dmirror", "give `points` or `plane` (xy, yz, zx)")),
    };
    Ok((at, n))
}

fn run_3dmirror(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let (at, n) = mirror_plane(p)?;
    let erase = p.get("erase").and_then(Value::as_bool).unwrap_or(false);
    let (made, kept) = apply(s, &hs, &mirror_in(at, n), !erase)?;
    Ok(report(kept, json!({ "handles": made.iter().map(|h| h.hex()).collect::<Vec<_>>() })))
}

/// Most copies one array makes.
const MAX_COPIES: usize = 100_000;

fn run_3darray(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let count = |k: &str| -> Result<u64> {
        match p.get(k) {
            None => Ok(1),
            Some(v) => v.as_u64().filter(|n| (1..=10_000).contains(n)).ok_or_else(|| bad("3darray", format!("`{k}` is 1 to 10000"))),
        }
    };
    let dist = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(1.0);
    let mut matrices = Vec::new();
    if let Some(pol) = p.get("polar") {
        let n = pol
            .get("count")
            .and_then(Value::as_u64)
            .filter(|n| (2..=10_000).contains(n))
            .ok_or_else(|| bad("3darray", "`polar.count` is 2 to 10000"))?;
        let fill = pol.get("angle").and_then(Value::as_f64).filter(|a| a.is_finite() && *a != 0.0 && a.abs() <= 360.0).unwrap_or(360.0);
        let c = p3_req("3darray", pol, "center")?;
        let e = p3_req("3darray", pol, "axisEnd")?;
        let ax = (e - c).normalized();
        if ax.len() < 0.5 {
            return Err(bad("3darray", "`center` and `axisEnd` must differ"));
        }
        // A full turn spreads the copies evenly; a part turn puts the last at its end.
        let step = if (fill.abs() - 360.0).abs() < 1e-9 { fill / n as f64 } else { fill / (n - 1) as f64 };
        let rotate = pol.get("rotate").and_then(Value::as_bool).unwrap_or(true);
        for i in 1..n {
            let r = rotation_about(c, ax, (step * i as f64).to_radians());
            matrices.push(if rotate {
                r
            } else {
                // Moved round, not turned: the move of the selection's middle.
                let mid = selection_middle(s, &hs).unwrap_or(c);
                translation(r.apply(mid) - mid)
            });
        }
    } else {
        let (rows, cols, levels) = (count("rows")?, count("columns")?, count("levels")?);
        if rows.saturating_mul(cols).saturating_mul(levels) as usize > MAX_COPIES {
            return Err(bad("3darray", format!("at most {MAX_COPIES} copies")));
        }
        let (dr, dc, dl) = (dist("rowSpacing"), dist("columnSpacing"), dist("levelSpacing"));
        for l in 0..levels {
            for r in 0..rows {
                for c in 0..cols {
                    if (l, r, c) != (0, 0, 0) {
                        matrices.push(translation(Vec3::new(c as f64 * dc, r as f64 * dr, l as f64 * dl)));
                    }
                }
            }
        }
    }
    if matrices.len().saturating_mul(hs.len()) > MAX_COPIES {
        return Err(bad("3darray", format!("at most {MAX_COPIES} copies")));
    }
    let mut made = Vec::new();
    let mut kept = 0;
    for m in &matrices {
        let (h, k) = apply(s, &hs, m, true)?;
        made.extend(h);
        kept += k;
    }
    Ok(report(kept, json!({ "copies": made.len() })))
}

/// The middle of the selection's extents.
fn selection_middle(s: &Session, hs: &[Handle]) -> Option<Vec3> {
    let d = s.doc().ok()?;
    let mut b = cadaddy_geom::Bounds2::EMPTY;
    for h in hs {
        if let Some(e) = d.entity(*h) {
            b = b.union(&cadaddy_doc::entity_bounds(d, e, 0));
        }
    }
    (!b.is_empty()).then(|| Vec3::new(b.center().x, b.center().y, 0.0))
}

fn run_3dosnap(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::snap3::mode;
    let st = &mut s.settings;
    if let Some(list) = p.get("modes").and_then(Value::as_array) {
        let mut bits = 0;
        for m in list {
            bits |= match m.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("vertex") => mode::VERTEX,
                Some("midpoint") => mode::MIDPOINT,
                Some("center") | Some("centre") => mode::CENTER,
                Some("perpendicular") => mode::PERPENDICULAR,
                Some("nearest") => mode::NEAREST,
                _ => return Err(bad("3dosnap", "`modes` are vertex, midpoint, center, perpendicular, nearest")),
            };
        }
        st.osmode3 = bits | (st.osmode3 & mode::OFF);
    }
    let on = p.get("on").and_then(Value::as_bool).unwrap_or(st.osmode3 & mode::OFF != 0 || p.get("modes").is_some());
    st.osmode3 = if on { st.osmode3 & !mode::OFF } else { st.osmode3 | mode::OFF };
    let now = st.osmode3 & mode::OFF == 0;
    s.echo(if now { "<3D Object Snap on>" } else { "<3D Object Snap off>" });
    Ok(json!({ "on": now, "osmode3": s.settings.osmode3 }))
}

// ---------------- picking points in space ----------------

/// A point given at a prompt that takes points in space: a 3D snap or typed point as it is; a
/// plain click (on the XY plane) on the axis held to, where the view's ray through it passes
/// closest to that axis; otherwise at the base point's height.
fn point_in_space(s: &Session, i: &Input, base: Option<Vec3>, axis: Option<Vec3>) -> Option<Vec3> {
    let q = match i {
        Input::Point3(p) => return Some(constrain(*p, base, axis)),
        Input::Point(p) => *p,
        _ => return None,
    };
    let (Some(b), Some(a)) = (base, axis) else { return Some(Vec3::new(q.x, q.y, base.map_or(0.0, |b| b.z))) };
    let dir = s.state().ok().and_then(|st| st.projection()).map_or(Vec3::new(0.0, 0.0, 1.0), |p| p.dir);
    // The ray through the click: the XY-plane point along the view.
    let o = Vec3::new(q.x, q.y, 0.0);
    let w0 = b - o;
    let (aa, ab, bb) = (a.dot(a), a.dot(dir), dir.dot(dir));
    let den = aa * bb - ab * ab;
    if den.abs() < 1e-9 {
        // Looking along the axis: the click says nothing about it.
        return Some(b);
    }
    let t = (ab * dir.dot(w0) - bb * a.dot(w0)) / den;
    Some(b + a * t)
}

/// `p` moved onto the axis through `base`, when there is one.
fn constrain(p: Vec3, base: Option<Vec3>, axis: Option<Vec3>) -> Vec3 {
    match (base, axis) {
        (Some(b), Some(a)) => b + a * (p - b).dot(a),
        _ => p,
    }
}

fn axis_kw(t: &str) -> Option<(u8, Vec3)> {
    match t.trim().to_ascii_lowercase().as_str() {
        "x" => Some((0, Vec3::new(1.0, 0.0, 0.0))),
        "y" => Some((1, Vec3::new(0.0, 1.0, 0.0))),
        "z" => Some((2, Vec3::new(0.0, 0.0, 1.0))),
        _ => None,
    }
}

fn hexes(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

fn finish(s: &mut Session, r: Result<Value>) -> Result<Step> {
    let v = r?;
    if let Some(m) = v.get("message").and_then(Value::as_str) {
        s.echo(m.to_string());
    }
    s.set_selection(Vec::new());
    Ok(Step::Done)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Move,
    Rotate,
    Scale,
}

/// 3DMOVE, 3DROTATE, 3DSCALE at the prompts: objects, a base point, then (held to X, Y or Z by
/// keyword or the handles) a second point, an angle or a factor.
struct EditM {
    kind: Kind,
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    base: Option<Vec3>,
    axis: Option<(u8, Vec3)>,
}

impl EditM {
    fn new(kind: Kind) -> Self {
        EditM { kind, sel: SelectPhase::default(), picked: None, base: None, axis: None }
    }
    fn gizmo(&self) -> Option<Gizmo> {
        let kind = match self.kind {
            Kind::Move => GizmoKind::Move,
            Kind::Rotate => GizmoKind::Rotate,
            Kind::Scale => GizmoKind::Scale,
        };
        self.base.map(|at| Gizmo { at, kind, axis: self.axis.map(|a| a.0) })
    }
}

impl Interactive for EditM {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::Move => "3DMOVE",
            Kind::Rotate => "3DROTATE",
            Kind::Scale => "3DSCALE",
        }
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return self.sel.prompt();
        }
        let Some(b) = self.base else { return Prompt::new("Specify base point", Accept::POINT).point3() };
        let g = self.gizmo();
        let p = match self.kind {
            // Held to an axis, a number is the distance along it.
            Kind::Move if self.axis.is_some() => Prompt::new("Specify second point or distance", Accept::POINT_OR_NUMBER).base(b.xy()),
            Kind::Move => Prompt::new("Specify second point", Accept::POINT).base(b.xy()),
            Kind::Rotate => Prompt::new("Specify angle", Accept::NUMBER).default("90"),
            Kind::Scale => Prompt::new("Specify scale factor", Accept::NUMBER).default("1"),
        };
        let p = p.point3().kw(&["X", "Y", "Z"]);
        match g {
            Some(g) => p.gizmo(g),
            None => p,
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        let hs = hexes(self.picked.as_deref().unwrap_or(&[]));
        let Some(b) = self.base else {
            if let Some(p) = point_in_space(s, &i, None, None) {
                self.base = Some(p);
            }
            return Ok(Step::Continue);
        };
        if let Input::Keyword(k) | Input::Text(k) = &i
            && let Some(a) = axis_kw(k)
        {
            self.axis = if self.axis.map(|x| x.0) == Some(a.0) { None } else { Some(a) };
            return Ok(Step::Continue);
        }
        let ax = self.axis.map(|a| a.1);
        let axis_json = self.axis.map(|a| json!(["x", "y", "z"][a.0 as usize]));
        match (self.kind, &i) {
            (Kind::Move, Input::Point(_) | Input::Point3(_)) => {
                let Some(p) = point_in_space(s, &i, Some(b), ax) else { return Ok(Step::Continue) };
                let r = run_3dmove(s, &json!({ "handles": hs, "base": [b.x, b.y, b.z], "to": [p.x, p.y, p.z] }));
                finish(s, r)
            }
            // A distance along the axis held to.
            (Kind::Move, Input::Text(t)) => {
                let (Some(a), Some(dd)) = (ax, number(t)) else { return Err(other("Requires a point, or a distance along an axis.")) };
                let to = b + a * dd;
                let r = run_3dmove(s, &json!({ "handles": hs, "base": [b.x, b.y, b.z], "to": [to.x, to.y, to.z] }));
                finish(s, r)
            }
            (Kind::Rotate, Input::Text(t) | Input::Keyword(t)) | (Kind::Scale, Input::Text(t) | Input::Keyword(t)) => {
                let v = number(t).filter(|v| v.is_finite()).ok_or_else(|| other("Requires a number."))?;
                let r = if self.kind == Kind::Rotate {
                    run_3drotate(s, &json!({ "handles": hs, "base": [b.x, b.y, b.z], "axis": axis_json.unwrap_or(json!("z")), "angle": v }))
                } else {
                    let mut p = json!({ "handles": hs, "base": [b.x, b.y, b.z], "factor": v });
                    if let (Some(a), Some(o)) = (axis_json, p.as_object_mut()) {
                        o.insert("axis".into(), a);
                    }
                    run_3dscale(s, &p)
                };
                finish(s, r)
            }
            (Kind::Rotate | Kind::Scale, Input::Enter) => {
                let v = if self.kind == Kind::Rotate { 90.0 } else { 1.0 };
                self.input(s, Input::Text(v.to_string()))
            }
            _ => Ok(Step::Continue),
        }
    }
}

/// 3DALIGN at the prompts: objects, up to three source points (Enter ends early), as many
/// destination points.
#[derive(Default)]
struct AlignM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    src: Vec<Vec3>,
    dst: Vec<Vec3>,
    src_done: bool,
}

impl Interactive for AlignM {
    fn name(&self) -> &'static str {
        "3DALIGN"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return self.sel.prompt();
        }
        let n = ["first", "second", "third"];
        if !self.src_done {
            return Prompt::new(format!("Specify source plane: {} point", n[self.src.len().min(2)]), Accept::POINT).point3();
        }
        Prompt::new(format!("Specify destination plane: {} point", n[self.dst.len().min(2)]), Accept::POINT).point3()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        if matches!(i, Input::Enter) && !self.src_done && !self.src.is_empty() {
            self.src_done = true;
            return Ok(Step::Continue);
        }
        let Some(p) = point_in_space(s, &i, None, None) else { return Ok(Step::Continue) };
        if !self.src_done {
            self.src.push(p);
            self.src_done = self.src.len() == 3;
            return Ok(Step::Continue);
        }
        self.dst.push(p);
        if self.dst.len() < self.src.len() {
            return Ok(Step::Continue);
        }
        let pts = |v: &[Vec3]| v.iter().map(|p| json!([p.x, p.y, p.z])).collect::<Vec<_>>();
        let hs = hexes(self.picked.as_deref().unwrap_or(&[]));
        let r = run_3dalign(s, &json!({ "handles": hs, "source": pts(&self.src), "dest": pts(&self.dst) }));
        finish(s, r)
    }
}

/// 3DMIRROR at the prompts: objects, a plane (three points, or XY/YZ/ZX and a point), whether
/// to erase the originals.
#[derive(Default)]
struct MirrorM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    pts: Vec<Vec3>,
    named: Option<&'static str>,
    plane: Option<Value>,
}

impl Interactive for MirrorM {
    fn name(&self) -> &'static str {
        "3DMIRROR"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return self.sel.prompt();
        }
        if self.plane.is_some() {
            return Prompt::new("Delete source objects?", Accept::TEXT).kw(&["Yes", "No"]).default("No");
        }
        if let Some(n) = self.named {
            return Prompt::new(format!("Specify point on {n} plane"), Accept::POINT).point3().default("0,0,0");
        }
        match self.pts.len() {
            0 => Prompt::new("Specify first point of mirror plane (3 points) or", Accept::POINT).point3().kw(&["XY", "YZ", "ZX"]),
            1 => Prompt::new("Specify second point on mirror plane", Accept::POINT).point3(),
            _ => Prompt::new("Specify third point on mirror plane", Accept::POINT).point3(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        if let Some(plane) = self.plane.clone() {
            let erase = match &i {
                Input::Keyword(k) | Input::Text(k) => k.trim().to_ascii_lowercase().starts_with('y'),
                _ => false,
            };
            let mut p = plane;
            if let Some(o) = p.as_object_mut() {
                o.insert("handles".into(), json!(hexes(self.picked.as_deref().unwrap_or(&[]))));
                o.insert("erase".into(), json!(erase));
            }
            let r = run_3dmirror(s, &p);
            return finish(s, r);
        }
        if let Some(n) = self.named {
            let at = match &i {
                Input::Enter => Some(Vec3::ZERO),
                _ => point_in_space(s, &i, None, None),
            };
            if let Some(at) = at {
                self.plane = Some(json!({ "plane": n.to_ascii_lowercase(), "point": [at.x, at.y, at.z] }));
            }
            return Ok(Step::Continue);
        }
        if let Input::Keyword(k) | Input::Text(k) = &i {
            self.named = match k.trim().to_ascii_uppercase().as_str() {
                "XY" => Some("XY"),
                "YZ" => Some("YZ"),
                "ZX" => Some("ZX"),
                _ => None,
            };
            if self.named.is_some() {
                return Ok(Step::Continue);
            }
        }
        if let Some(p) = point_in_space(s, &i, None, None) {
            self.pts.push(p);
        }
        if self.pts.len() == 3 {
            let v: Vec<Value> = self.pts.iter().map(|p| json!([p.x, p.y, p.z])).collect();
            self.plane = Some(json!({ "points": v }));
        }
        Ok(Step::Continue)
    }
}

/// 3DARRAY at the prompts: objects, Rectangular or Polar, then counts, spacings or the axis.
#[derive(Default)]
struct ArrayM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    polar: Option<bool>,
    nums: Vec<f64>,
    pts: Vec<Vec3>,
}

impl ArrayM {
    /// The questions in order: (prompt, default) for the kind chosen.
    fn questions(polar: bool) -> &'static [(&'static str, &'static str)] {
        if polar {
            &[("Enter the number of items in the array", "6"), ("Specify the angle to fill (+=ccw, -=cw)", "360")]
        } else {
            &[
                ("Enter the number of rows (---)", "1"),
                ("Enter the number of columns (|||)", "1"),
                ("Enter the number of levels (...)", "1"),
                ("Specify the distance between rows (---)", "1"),
                ("Specify the distance between columns (|||)", "1"),
                ("Specify the distance between levels (...)", "1"),
            ]
        }
    }
}

impl Interactive for ArrayM {
    fn name(&self) -> &'static str {
        "3DARRAY"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return self.sel.prompt();
        }
        let Some(polar) = self.polar else {
            return Prompt::new("Enter the type of array", Accept::TEXT).kw(&["Rectangular", "Polar"]).default("R");
        };
        let q = ArrayM::questions(polar);
        if let Some((m, d)) = q.get(self.nums.len()) {
            return Prompt::new(*m, Accept::NUMBER).default(*d);
        }
        match self.pts.len() {
            0 => Prompt::new("Specify center point of array", Accept::POINT).point3(),
            _ => Prompt::new("Specify second point on axis of rotation", Accept::POINT).point3(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        let Some(polar) = self.polar else {
            self.polar = Some(match &i {
                Input::Keyword(k) | Input::Text(k) => k.trim().to_ascii_lowercase().starts_with('p'),
                _ => false,
            });
            return Ok(Step::Continue);
        };
        let q = ArrayM::questions(polar);
        if let Some((_, d)) = q.get(self.nums.len()) {
            let v = match &i {
                Input::Enter => d.parse().ok(),
                Input::Text(t) => number(t),
                _ => None,
            };
            self.nums.push(v.filter(|v| v.is_finite()).ok_or_else(|| other("Requires a number."))?);
            if polar || self.nums.len() < q.len() {
                return Ok(Step::Continue);
            }
        } else if let Some(p) = point_in_space(s, &i, None, None) {
            self.pts.push(p);
            if self.pts.len() < 2 {
                return Ok(Step::Continue);
            }
        } else {
            return Ok(Step::Continue);
        }
        let hs = hexes(self.picked.as_deref().unwrap_or(&[]));
        let n = |k: usize| self.nums.get(k).copied().unwrap_or(1.0);
        let r = if polar {
            let (c, e) = (self.pts[0], self.pts[1]);
            run_3darray(
                s,
                &json!({ "handles": hs, "polar": { "count": n(0).round().max(0.0) as u64, "angle": n(1), "center": [c.x, c.y, c.z], "axisEnd": [e.x, e.y, e.z] } }),
            )
        } else {
            let u = |k: usize| n(k).round().max(0.0) as u64;
            run_3darray(
                s,
                &json!({ "handles": hs, "rows": u(0), "columns": u(1), "levels": u(2), "rowSpacing": n(3), "columnSpacing": n(4), "levelSpacing": n(5) }),
            )
        };
        finish(s, r)
    }
}
