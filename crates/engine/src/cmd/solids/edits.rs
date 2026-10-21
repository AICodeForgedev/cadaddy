//! Composite solids and solid edits: UNION/SUBTRACT/INTERSECT on solids, SLICE, THICKEN,
//! FILLETEDGE, CHAMFEREDGE and SOLIDEDIT (faces: move, offset, delete; body: shell).

use cadaddy_doc::{BoolOp, EntityKind, Handle, Solid3d, SolidShape, combine, half_space, placement};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase, number};
use super::super::{CommandSpec, bad, targets};
use super::point3_value;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("slice", "Slice", run_slice)
            .menu(&["Modify", "3D Operations", "Slice"])
            .alias(&["sl"])
            .params("{handles?, point: [x,y,z], normal?: [nx,ny,nz] | through?: [x,y] (a vertical plane through point and this) | plane?: \"xy\"|\"yz\"|\"zx\", keep?: \"both\" | [x,y,z] (a point on the side to keep)}")
            .interactive(|_| Ok(Box::new(SliceM::default()))),
        CommandSpec::new("thicken", "Thicken", run_thicken)
            .menu(&["Modify", "3D Operations", "Thicken"])
            .params("{handles?, thickness} — regions and 3D faces become solids (a negative thickness goes the other way); keep?")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Thicken)))),
        CommandSpec::new("filletedge", "Fillet Edge", run_filletedge)
            .menu(&["Modify", "Solid Editing", "Fillet Edges"])
            .params("{handle, edges: [[x,y] (seen from above) | [x,y,z]], radius}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Fillet)))),
        CommandSpec::new("chamferedge", "Chamfer Edge", run_chamferedge)
            .menu(&["Modify", "Solid Editing", "Chamfer Edges"])
            .params("{handle, edges: [[x,y] | [x,y,z]], distance}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Chamfer)))),
        CommandSpec::new("solidedit", "Solid Editing", run_solidedit)
            .params("{handle, op: \"move\"|\"offset\"|\"delete\"|\"shell\", faces: [[x,y] (seen from above) | [x,y,z]], delta?: [dx,dy,dz] (move), distance? (offset: + adds material), thickness? (shell; faces = those removed)}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Offset)))),
        CommandSpec::new("solidedit.move", "Move Faces", run_move_faces)
            .menu(&["Modify", "Solid Editing", "Move Faces"])
            .params("{handle, faces, delta}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Move)))),
        CommandSpec::new("solidedit.offset", "Offset Faces", run_offset_faces)
            .menu(&["Modify", "Solid Editing", "Offset Faces"])
            .params("{handle, faces, distance}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Offset)))),
        CommandSpec::new("solidedit.delete", "Delete Faces", run_delete_faces)
            .menu(&["Modify", "Solid Editing", "Delete Faces"])
            .params("{handle, faces}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Delete)))),
        CommandSpec::new("solidedit.shell", "Shell", run_shell)
            .menu(&["Modify", "Solid Editing", "Shell"])
            .params("{handle, thickness, faces?: points on faces to remove}")
            .interactive(|_| Ok(Box::new(PickM::new(Mode::Shell)))),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

fn solid_of(s: &Session, h: Handle) -> Option<Solid3d> {
    match &s.doc().ok()?.entity(h)?.kind {
        EntityKind::Solid3d(so) => Some((**so).clone()),
        _ => None,
    }
}

/// Replace a solid's definition, refusing one that isn't a solid or comes out empty.
fn store(s: &mut Session, h: Handle, so: Solid3d) -> Result<()> {
    if !so.is_valid() || so.polys(so.mesh_tol()).is_empty() {
        return Err(other("The result would not be a solid; nothing was changed."));
    }
    s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Solid3d(Box::new(so)))?;
    Ok(())
}

/// UNION, SUBTRACT, INTERSECT when solids are among the objects: `None` when there are none (the
/// region booleans take over).
pub(crate) fn solid_boolean(s: &mut Session, from: &[Handle], remove: &[Handle], op: BoolOp) -> Result<Option<Value>> {
    let a: Vec<(Handle, Solid3d)> = from.iter().filter_map(|h| Some((*h, solid_of(s, *h)?))).collect();
    let b: Vec<(Handle, Solid3d)> = remove.iter().filter_map(|h| Some((*h, solid_of(s, *h)?))).collect();
    if a.is_empty() && b.is_empty() {
        return Ok(None);
    }
    let Some(((h0, first), rest)) = a.split_first() else { return Err(other("Select solids to subtract from.")) };
    if b.iter().any(|(h, _)| a.iter().any(|(x, _)| x == h)) {
        return Err(other("A solid can't be subtracted from itself."));
    }
    let mut acc = first.clone();
    for (_, so) in rest {
        acc = combine(acc, so.clone(), if op == BoolOp::Subtract { BoolOp::Union } else { op });
    }
    if op != BoolOp::Subtract && rest.is_empty() {
        return Err(other("Select two or more solids."));
    }
    for (_, so) in &b {
        acc = combine(acc, so.clone(), BoolOp::Subtract);
    }
    if !acc.is_valid() {
        return Err(other("The solid would have too many operations; nothing was changed."));
    }
    if acc.polys(acc.mesh_tol()).is_empty() {
        return Err(other(if op == BoolOp::Intersect {
            "The solids don't overlap; nothing was changed."
        } else {
            "Nothing would remain; nothing was changed."
        }));
    }
    s.doc_mut()?.modify_entity(*h0, |e| e.kind = EntityKind::Solid3d(Box::new(acc)))?;
    let d = s.doc_mut()?;
    for (h, _) in rest.iter().chain(&b) {
        d.remove_entity(*h);
    }
    Ok(Some(json!({ "handle": h0.hex(), "combined": a.len() + b.len() })))
}

/// A picked point: `[x, y]` is seen from above, `[x, y, z]` is a point in space.
fn pick(v: &Value) -> Option<(Vec3, bool)> {
    let from_above = v.as_array().is_some_and(|a| a.len() == 2) || v.is_string();
    point3_value(v).map(|p| (p, from_above))
}

fn picks(cmd: &str, p: &Value, key: &str) -> Result<Vec<(Vec3, bool)>> {
    let a = p.get(key).and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{key}` (an array of points) is required")))?;
    if a.len() > 1000 {
        return Err(bad(cmd, format!("`{key}`: at most 1000 points")));
    }
    a.iter().map(|v| pick(v).ok_or_else(|| bad(cmd, format!("`{key}`: {v} is not a point")))).collect()
}

fn one_solid(s: &Session, cmd: &str, p: &Value) -> Result<(Handle, Solid3d)> {
    let h = targets(s, p)?.into_iter().find(|h| solid_of(s, *h).is_some()).ok_or_else(|| bad(cmd, "select a 3D solid (`handle`)"))?;
    Ok((h, solid_of(s, h).ok_or_else(|| bad(cmd, "not a 3D solid"))?))
}

fn run_slice(s: &mut Session, p: &Value) -> Result<Value> {
    let hs: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| solid_of(s, *h).is_some()).collect();
    if hs.is_empty() {
        return Err(bad("slice", "select 3D solids"));
    }
    let at = p.get("point").and_then(point3_value).ok_or_else(|| bad("slice", "`point` is required"))?;
    let normal = if let Some(n) = p.get("normal").and_then(point3_value) {
        n
    } else if let Some(t) = p.get("through").and_then(point3_value) {
        // The vertical plane through both points.
        let d = t - at;
        Vec3::new(-d.y, d.x, 0.0)
    } else {
        match p.get("plane").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("xy") => Vec3::new(0.0, 0.0, 1.0),
            Some("yz") => Vec3::new(1.0, 0.0, 0.0),
            Some("zx") => Vec3::new(0.0, 1.0, 0.0),
            _ => return Err(bad("slice", "give the plane: `normal`, `through` or `plane`")),
        }
    };
    let normal = normal.normalized();
    if normal.len() < 0.5 {
        return Err(bad("slice", "the plane needs a direction"));
    }
    // Keep the side a point is on (its side of the plane), or both.
    let keep = match p.get("keep") {
        None | Some(Value::Null) => None,
        Some(Value::String(k)) if k.eq_ignore_ascii_case("both") => None,
        Some(v) => Some(point3_value(v).ok_or_else(|| bad("slice", "`keep` is \"both\" or a point"))?),
    };
    let mut made = Vec::new();
    for h in hs {
        let Some(so) = solid_of(s, h) else { continue };
        let pr = so.props(so.mesh_tol());
        let size = 2.0 * ((pr.max - pr.min).len() + (at - (pr.min + pr.max) * 0.5).len()) + 1.0;
        // `half_space(p, n)` is the side `n` points away from.
        let below = combine(so.clone(), half_space(at, normal, size), BoolOp::Intersect);
        let above = combine(so.clone(), half_space(at, normal * -1.0, size), BoolOp::Intersect);
        let full = |x: &Solid3d| !x.polys(x.mesh_tol()).is_empty();
        match keep {
            Some(k) => {
                let piece = if (k - at).dot(normal) >= 0.0 { above } else { below };
                if full(&piece) {
                    store(s, h, piece)?;
                }
            }
            None => {
                // Unless the plane misses it.
                if full(&below) && full(&above) {
                    store(s, h, below)?;
                    let common = s.doc()?.entity(h).map(|e| e.common.clone()).unwrap_or_default();
                    let space = s.space();
                    let nh = s.doc_mut()?.add(&space, common, EntityKind::Solid3d(Box::new(above)))?;
                    made.push(nh.hex());
                }
            }
        }
    }
    Ok(json!({ "created": made }))
}

fn run_thicken(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let t = p
        .get("thickness")
        .and_then(Value::as_f64)
        .filter(|t| t.is_finite() && *t != 0.0 && t.abs() <= cadaddy_doc::MAX_SOLID_SIZE)
        .ok_or_else(|| bad("thicken", "`thickness` must be a non-zero number"))?;
    let d = s.doc()?;
    let mut solids = Vec::new();
    let mut used = Vec::new();
    for h in &hs {
        let Some(e) = d.entity(*h) else { continue };
        let so = match &e.kind {
            EntityKind::Region(r) => {
                let loops: Vec<Vec<PolyVertex>> = r.loops.iter().map(|l| l.vertices.clone()).filter(|v| v.len() >= 2).collect();
                super::extrusion(&loops, e.kind.elevation(), t, 0.0)
            }
            EntityKind::Face3d(f) => {
                let [a, b, c, dd] = f.corners;
                let pts: Vec<Vec3> = if c == dd { vec![a, b, c] } else { vec![a, b, c, dd] };
                let n = (b - a).cross(c - a).normalized();
                if n.len() < 0.5 {
                    continue;
                }
                let n = if t < 0.0 { n * -1.0 } else { n };
                let (x, y) = cadaddy_doc::frame_for(n);
                let loops = vec![pts.iter().map(|q| PolyVertex::new(Vec2::new((*q - a).dot(x), (*q - a).dot(y)))).collect()];
                Solid3d { shape: SolidShape::Extrusion { loops, height: t.abs(), taper: 0.0 }, xf: placement(a, x, y, n) }
            }
            _ => continue,
        };
        solids.push(so);
        used.push(*h);
    }
    if solids.is_empty() {
        return Err(bad("thicken", "select regions or 3D faces"));
    }
    let mut made = Vec::new();
    for so in solids {
        if !so.is_valid() {
            return Err(other("The result would not be a solid; nothing was changed."));
        }
        made.push(s.add_entity(EntityKind::Solid3d(Box::new(so)))?.hex());
    }
    if !p.get("keep").and_then(Value::as_bool).unwrap_or(false) {
        let d = s.doc_mut()?;
        for h in used {
            d.remove_entity(h);
        }
    }
    Ok(json!({ "handles": made, "handle": made.first() }))
}

fn edge_op(s: &mut Session, p: &Value, cmd: &str, size_key: &str, fillet: bool) -> Result<Value> {
    let (h, mut so) = one_solid(s, cmd, p)?;
    let size = p
        .get(size_key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= cadaddy_doc::MAX_SOLID_SIZE)
        .ok_or_else(|| bad(cmd, format!("`{size_key}` must be more than zero")))?;
    let pts = picks(cmd, p, "edges")?;
    if pts.is_empty() {
        return Err(bad(cmd, "pick one or more edges"));
    }
    // All the edges are found on the solid as it was, then cut one after another.
    let tol = so.mesh_tol();
    let edges: Vec<cadaddy_doc::Edge> = pts
        .iter()
        .map(|(q, above)| so.edge_near(*q, *above, tol).ok_or_else(|| other("No straight edge between two flat faces there.")))
        .collect::<Result<_>>()?;
    for e in &edges {
        so = if fillet { so.fillet_edge(e, size) } else { so.chamfer_edge(e, size) }.map_err(other)?;
    }
    store(s, h, so)?;
    Ok(json!({ "handle": h.hex(), "edges": edges.len() }))
}

fn run_filletedge(s: &mut Session, p: &Value) -> Result<Value> {
    edge_op(s, p, "filletedge", "radius", true)
}

fn run_chamferedge(s: &mut Session, p: &Value) -> Result<Value> {
    edge_op(s, p, "chamferedge", "distance", false)
}

fn run_solidedit(s: &mut Session, p: &Value) -> Result<Value> {
    let op = p.get("op").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
    let (h, mut so) = one_solid(s, "solidedit", p)?;
    let tol = so.mesh_tol();
    let faces = if op == "shell" && p.get("faces").is_none() { Vec::new() } else { picks("solidedit", p, "faces")? };
    let found = |so: &Solid3d, (q, above): (Vec3, bool)| so.face_at(q, above, so.mesh_tol()).ok_or_else(|| other("No face of the solid there."));
    match op.as_str() {
        "move" => {
            let v = p.get("delta").and_then(point3_value).ok_or_else(|| bad("solidedit", "`delta` ([dx,dy,dz]) is required"))?;
            let &first = faces.first().ok_or_else(|| bad("solidedit", "pick a face"))?;
            let (t, n) = found(&so, first)?;
            so = so.move_face(t, v, n).ok_or_else(|| other("That face can't be moved (only flat faces, holes and added parts)."))?;
        }
        "offset" => {
            let dist =
                p.get("distance").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad("solidedit", "`distance` is required"))?;
            let tags: Vec<u64> = faces.iter().map(|f| found(&so, *f).map(|x| x.0)).collect::<Result<_>>()?;
            let mut done: Vec<(u32, u32)> = Vec::new();
            for t in tags {
                // The faces of a cylinder are found in parts; each face once.
                let key = (cadaddy_doc::tag_leaf(t), cadaddy_doc::tag_face(t).min(2));
                if done.contains(&key) {
                    continue;
                }
                done.push(key);
                so = so.offset_face(t, dist).ok_or_else(|| {
                    other("That face can't be offset (box sides, the ends of cylinders, cones and extrusions, cylinder walls and spheres).")
                })?;
            }
        }
        "delete" => {
            for f in faces {
                // Deleting removes a whole hole or fillet: later picks on it find nothing.
                let Some((t, _)) = so.face_at(f.0, f.1, so.mesh_tol()) else { continue };
                match so.delete_face(t) {
                    Some(next) => so = next,
                    None => return Err(other("That face can't be deleted (only holes, fillets, chamfers and added parts).")),
                }
            }
        }
        "shell" => {
            let t = p.get("thickness").and_then(Value::as_f64).ok_or_else(|| bad("solidedit", "`thickness` is required"))?;
            let tags: Vec<u64> = faces.iter().filter_map(|f| so.face_at(f.0, f.1, tol).map(|x| x.0)).collect();
            so = so.shell(t, &tags).map_err(other)?;
        }
        _ => return Err(bad("solidedit", "`op` is move, offset, delete or shell")),
    }
    store(s, h, so)?;
    Ok(json!({ "handle": h.hex() }))
}

fn with_op(p: &Value, op: &str) -> Value {
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        o.insert("op".into(), json!(op));
    }
    q
}

fn run_move_faces(s: &mut Session, p: &Value) -> Result<Value> {
    run_solidedit(s, &with_op(p, "move"))
}
fn run_offset_faces(s: &mut Session, p: &Value) -> Result<Value> {
    run_solidedit(s, &with_op(p, "offset"))
}
fn run_delete_faces(s: &mut Session, p: &Value) -> Result<Value> {
    run_solidedit(s, &with_op(p, "delete"))
}
fn run_shell(s: &mut Session, p: &Value) -> Result<Value> {
    run_solidedit(s, &with_op(p, "shell"))
}

// ---------------- prompts ----------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Thicken,
    Fillet,
    Chamfer,
    Move,
    Offset,
    Delete,
    Shell,
}

/// Select a solid (or objects), pick edges or faces until Enter, then a size or a move.
struct PickM {
    mode: Mode,
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    points: Vec<[f64; 2]>,
    picking_done: bool,
    base: Option<Vec2>,
}

impl PickM {
    fn new(mode: Mode) -> Self {
        let sel = if mode == Mode::Thicken { SelectPhase::default() } else { SelectPhase::single() };
        PickM { mode, sel, picked: None, points: Vec::new(), picking_done: mode == Mode::Thicken, base: None }
    }
    fn run(&self, s: &mut Session, value: Value) -> Result<Step> {
        let hs: Vec<String> = self.picked.iter().flatten().map(|h| h.hex()).collect();
        let pts = json!(self.points);
        let r = match self.mode {
            Mode::Thicken => run_thicken(s, &json!({ "handles": hs, "thickness": value })),
            Mode::Fillet => run_filletedge(s, &json!({ "handles": hs, "edges": pts, "radius": value })),
            Mode::Chamfer => run_chamferedge(s, &json!({ "handles": hs, "edges": pts, "distance": value })),
            Mode::Move => run_solidedit(s, &json!({ "handles": hs, "op": "move", "faces": pts, "delta": value })),
            Mode::Offset => run_solidedit(s, &json!({ "handles": hs, "op": "offset", "faces": pts, "distance": value })),
            Mode::Delete => run_solidedit(s, &json!({ "handles": hs, "op": "delete", "faces": pts })),
            Mode::Shell => run_solidedit(s, &json!({ "handles": hs, "op": "shell", "faces": pts, "thickness": value })),
        };
        s.set_selection(Vec::new());
        r?;
        Ok(Step::Done)
    }
}

impl Interactive for PickM {
    fn name(&self) -> &'static str {
        match self.mode {
            Mode::Thicken => "THICKEN",
            Mode::Fillet => "FILLETEDGE",
            Mode::Chamfer => "CHAMFEREDGE",
            Mode::Move | Mode::Offset | Mode::Delete | Mode::Shell => "SOLIDEDIT",
        }
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return match self.mode {
                Mode::Thicken => Prompt::new("Select surfaces to thicken", Accept::SELECT),
                Mode::Shell => Prompt::new("Select a 3D solid", Accept::SELECT),
                _ => Prompt::new("Select a 3D solid", Accept::SELECT),
            };
        }
        if !self.picking_done {
            return match self.mode {
                Mode::Fillet | Mode::Chamfer => Prompt::new("Select an edge", Accept::POINT),
                Mode::Shell => Prompt::new("Remove faces", Accept::POINT),
                _ => Prompt::new("Select faces", Accept::POINT),
            };
        }
        match self.mode {
            Mode::Thicken => Prompt::new("Specify thickness", Accept::NUMBER).default("1.0000"),
            Mode::Fillet => Prompt::new("Enter fillet radius", Accept::NUMBER).default("1.0000"),
            Mode::Chamfer => Prompt::new("Enter distance", Accept::NUMBER).default("1.0000"),
            Mode::Move if self.base.is_none() => Prompt::new("Specify a base point or displacement", Accept::POINT),
            Mode::Move => Prompt::new("Specify a second point of displacement", Accept::POINT).base_opt(self.base),
            Mode::Offset => Prompt::new("Specify the offset distance", Accept::NUMBER),
            Mode::Delete => Prompt::new("", Accept::default()),
            Mode::Shell => Prompt::new("Enter the shell offset distance", Accept::NUMBER),
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
                SelOutcome::Done(hs) => {
                    self.picked = Some(hs);
                    if self.mode == Mode::Delete && self.picking_done {
                        return self.run(s, Value::Null);
                    }
                }
            }
            return Ok(Step::Continue);
        }
        if !self.picking_done {
            match i {
                Input::Point(p) => {
                    self.points.push([p.x, p.y]);
                    let what = if matches!(self.mode, Mode::Fillet | Mode::Chamfer) { "edge" } else { "face" };
                    s.echo(format!("{} {what} pick(s); Enter when done.", self.points.len()));
                }
                Input::Enter => {
                    self.picking_done = true;
                    if self.mode == Mode::Delete {
                        return self.run(s, Value::Null);
                    }
                }
                _ => {}
            }
            return Ok(Step::Continue);
        }
        if self.mode == Mode::Move {
            return match (self.base, i) {
                (None, Input::Point(p)) => {
                    self.base = Some(p);
                    Ok(Step::Continue)
                }
                (Some(b), Input::Point(p)) => self.run(s, json!([p.x - b.x, p.y - b.y, 0.0])),
                (_, Input::Text(t)) => {
                    let v = point3_value(&json!(t.trim())).ok_or_else(|| other("Requires a displacement."))?;
                    self.run(s, json!([v.x, v.y, v.z]))
                }
                _ => Ok(Step::Continue),
            };
        }
        let v = match i {
            Input::Enter if matches!(self.mode, Mode::Thicken | Mode::Fillet | Mode::Chamfer) => Some(1.0),
            Input::Text(t) => number(&t),
            _ => None,
        };
        match v {
            Some(v) if v.is_finite() => self.run(s, json!(v)),
            _ => Ok(Step::Continue),
        }
    }
}

/// SLICE at the prompts: objects, then two points of a vertical plane (or XY/YZ/ZX and a point),
/// then the side to keep.
#[derive(Default)]
struct SliceM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    first: Option<Vec2>,
    plane: Option<Value>,
    named: Option<&'static str>,
}

impl SliceM {
    fn run(&self, s: &mut Session, keep: Value) -> Result<Step> {
        let mut p = self.plane.clone().unwrap_or(Value::Null);
        if let Some(o) = p.as_object_mut() {
            o.insert("handles".into(), json!(self.picked.iter().flatten().map(|h| h.hex()).collect::<Vec<_>>()));
            o.insert("keep".into(), keep);
        }
        let r = run_slice(s, &p);
        s.set_selection(Vec::new());
        r?;
        Ok(Step::Done)
    }
}

impl Interactive for SliceM {
    fn name(&self) -> &'static str {
        "SLICE"
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
            return Prompt::new("Select objects to slice", Accept::SELECT);
        }
        if self.plane.is_some() {
            return Prompt::new("Specify a point on desired side or [keep Both sides]", Accept::POINT).kw(&["Both"]).default("Both");
        }
        match (self.named, self.first) {
            // A typed point here would lose its Z: the plane's level is asked for as a number.
            (Some(n), _) => Prompt::new(
                format!(
                    "Specify the {} of the {n}-plane",
                    match n {
                        "XY" => "Z",
                        "YZ" => "X",
                        _ => "Y",
                    }
                ),
                Accept::POINT_OR_NUMBER,
            )
            .default("0"),
            (None, None) => Prompt::new("Specify start point of slicing plane or", Accept::POINT).kw(&["XY", "YZ", "ZX"]),
            (None, Some(f)) => Prompt::new("Specify second point on plane", Accept::POINT).base(f),
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
        if self.plane.is_some() {
            return match i {
                Input::Point(p) => self.run(s, json!([p.x, p.y, 0.0])),
                Input::Enter => self.run(s, json!("both")),
                Input::Keyword(_) | Input::Text(_) => self.run(s, json!("both")),
                _ => Ok(Step::Continue),
            };
        }
        match (self.named, self.first, i) {
            (Some(n), _, i @ (Input::Point(_) | Input::Text(_) | Input::Enter)) => {
                let level = match &i {
                    Input::Point(p) if n == "YZ" => p.x,
                    Input::Point(p) => p.y,
                    Input::Text(t) => number(t).filter(|v| v.is_finite()).ok_or_else(|| other("Requires a number."))?,
                    _ => 0.0,
                };
                let at = match n {
                    "XY" => [0.0, 0.0, level],
                    "YZ" => [level, 0.0, 0.0],
                    _ => [0.0, level, 0.0],
                };
                self.plane = Some(json!({ "point": at, "plane": n.to_ascii_lowercase() }));
            }
            (None, None, Input::Point(p)) => self.first = Some(p),
            (None, None, Input::Keyword(k) | Input::Text(k)) => {
                self.named = match k.trim().to_ascii_uppercase().as_str() {
                    "XY" => Some("XY"),
                    "YZ" => Some("YZ"),
                    "ZX" => Some("ZX"),
                    _ => None,
                }
            }
            (None, Some(f), Input::Point(p)) if !p.near(f, 1e-12) => {
                self.plane = Some(json!({ "point": [f.x, f.y, 0.0], "through": [p.x, p.y, 0.0] }))
            }
            _ => {}
        }
        Ok(Step::Continue)
    }
}
