//! CAMERA: placed cameras. A camera stands at a point, looks at a target through a lens, and
//! keeps a named view (the VIEW table's camera views): restoring it (`camera.view`, or VIEW) looks
//! from the camera. Its glyph shows in model space while CAMERADISPLAY is 1, which CAMERA turns
//! on. CADaddy's 3D views are parallel, so the view shows what the lens frames at the target.

use cadaddy_doc::{NamedView, ViewCamera};
use cadaddy_geom::view3::{self, Projection};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// At most this many cameras in a drawing.
const MAX_CAMERAS: usize = 1000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("camera", "Create Camera", run_camera)
            .menu(&["View", "Create Camera"])
            .alias(&["cam"])
            .params("{position: [x, y] (CAMERAHEIGHT up) or [x, y, z], target: [x, y] or [x, y, z], lens?: 50 (mm), name?: (default Camera1, Camera2 and so on)} → {name, view}")
            .interactive(|_| Ok(Box::new(CameraM::default()))),
        CommandSpec::new("camera.list", "Cameras", run_list).params("{} → {cameras: [{name, position, target, lens, fieldOfView}]}").noundo(),
        CommandSpec::new("camera.set", "Change Camera", run_set).params("{name, position?, target?, lens?, rename?} → {name, view}"),
        CommandSpec::new("camera.delete", "Delete Camera", run_delete).params("{name} (its view goes with it)"),
        CommandSpec::new("camera.view", "View from Camera", run_view).params("{name} → the camera's view on the Model tab").noundo(),
    ]
}

/// A point `[x, y]` (standing `z` up) or `[x, y, z]`.
fn point3(cmd: &str, p: &Value, key: &str, z: f64) -> Result<Option<Vec3>> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let a = v.as_array().ok_or_else(|| bad(cmd, format!("`{key}` must be [x, y] or [x, y, z]")))?;
    let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
    let q = match (a.len(), n.as_slice()) {
        (2, [x, y]) => Vec3::new(*x, *y, z),
        (3, [x, y, zz]) => Vec3::new(*x, *y, *zz),
        _ => return Err(bad(cmd, format!("`{key}` must be [x, y] or [x, y, z]"))),
    };
    if !q.is_finite() || q.x.abs() > 1e12 || q.y.abs() > 1e12 || q.z.abs() > 1e12 {
        return Err(bad(cmd, format!("`{key}` must be finite")));
    }
    Ok(Some(q))
}

fn lens_param(cmd: &str, p: &Value) -> Result<Option<f64>> {
    match p.get("lens") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_f64().filter(|l| l.is_finite() && (1.0..=100_000.0).contains(l)).map(Some).ok_or_else(|| bad(cmd, "`lens` must be 1-100000 (mm)"))
        }
    }
}

/// The named view of camera `c`: looking from it, framing at the target what its lens sees
/// across a drawing area of `aspect` (width / height).
fn view_of(name: &str, c: ViewCamera, aspect: f64) -> NamedView {
    let dir = (c.position - c.target).normalized();
    let center = Projection::new(dir).map_or(c.target.xy(), |pr| pr.project(c.target));
    let width = (2.0 * c.distance() * (c.field_of_view() / 2.0).tan()).max(1e-9);
    NamedView {
        name: name.to_string(),
        center,
        width,
        height: width / aspect.max(1e-6),
        dir: (!view3::is_plan(dir)).then_some(dir),
        camera: Some(c),
        ..NamedView::default()
    }
}

fn aspect(s: &Session) -> f64 {
    let (w, h) = s.viewport_px;
    if w > 0.0 && h > 0.0 { w / h } else { 1.5 }
}

fn camera_json(v: &NamedView) -> Option<Value> {
    let c = v.camera?;
    Some(json!({
        "name": v.name,
        "position": [c.position.x, c.position.y, c.position.z],
        "target": [c.target.x, c.target.y, c.target.z],
        "lens": c.lens,
        "fieldOfView": c.field_of_view().to_degrees(),
    }))
}

fn find(s: &Session, cmd: &str, name: &str) -> Result<NamedView> {
    s.doc()?
        .views
        .iter()
        .find(|v| v.camera.is_some() && v.name.eq_ignore_ascii_case(name.trim()))
        .cloned()
        .ok_or_else(|| bad(cmd, format!("no camera named \"{}\"", name.trim())))
}

fn name_param<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))
}

fn valid_name(n: &str) -> bool {
    super::super::names::is_valid(n)
}

/// Place a camera: its view is added, and camera glyphs are shown.
pub(crate) fn place(s: &mut Session, position: Vec3, target: Vec3, lens: f64, name: Option<&str>) -> Result<Value> {
    if (position - target).len() < 1e-9 {
        return Err(bad("camera", "the camera and its target must be apart"));
    }
    let aspect = aspect(s);
    let d = s.doc_mut()?;
    if d.views.iter().filter(|v| v.camera.is_some()).count() >= MAX_CAMERAS {
        return Err(bad("camera", format!("a drawing holds at most {MAX_CAMERAS} cameras")));
    }
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if !valid_name(n) => return Err(bad("camera", format!("\"{n}\" is not a valid camera name"))),
        Some(n) if d.views.iter().any(|v| v.name.eq_ignore_ascii_case(n)) => {
            return Err(bad("camera", format!("a view named \"{n}\" already exists")));
        }
        Some(n) => n.to_string(),
        None => {
            (1..=MAX_CAMERAS + 1).map(|i| format!("Camera{i}")).find(|n| !d.views.iter().any(|v| v.name.eq_ignore_ascii_case(n))).unwrap_or_default()
        }
    };
    let v = view_of(&name, ViewCamera { position, target, lens }, aspect);
    d.views.push(v);
    d.header.set("CAMERADISPLAY", cadaddy_doc::HVal::Int(1));
    Ok(json!({ "name": name, "view": name, "message": format!("Camera \"{name}\" placed: View ▸ Named Views (or camera.view) looks from it.") }))
}

fn run_camera(s: &mut Session, p: &Value) -> Result<Value> {
    let z = s.doc()?.header.f64("CAMERAHEIGHT", 0.0);
    let position = point3("camera", p, "position", z)?.ok_or_else(|| bad("camera", "`position` is required"))?;
    let target = point3("camera", p, "target", 0.0)?.ok_or_else(|| bad("camera", "`target` is required"))?;
    let lens = lens_param("camera", p)?.unwrap_or(50.0);
    place(s, position, target, lens, str_param(p, "name"))
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let list: Vec<Value> = s.doc()?.views.iter().filter_map(camera_json).collect();
    Ok(json!({ "cameras": list }))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "camera.set";
    let name = name_param(cmd, p)?;
    let old = find(s, cmd, name)?;
    let Some(mut c) = old.camera else { return Err(bad(cmd, "not a camera")) };
    if let Some(q) = point3(cmd, p, "position", c.position.z)? {
        c.position = q;
    }
    if let Some(q) = point3(cmd, p, "target", c.target.z)? {
        c.target = q;
    }
    if let Some(l) = lens_param(cmd, p)? {
        c.lens = l;
    }
    if c.distance() < 1e-9 {
        return Err(bad(cmd, "the camera and its target must be apart"));
    }
    let new_name = match str_param(p, "rename").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if !valid_name(n) => return Err(bad(cmd, format!("\"{n}\" is not a valid camera name"))),
        Some(n) if !n.eq_ignore_ascii_case(&old.name) && s.doc()?.views.iter().any(|v| v.name.eq_ignore_ascii_case(n)) => {
            return Err(bad(cmd, format!("a view named \"{n}\" already exists")));
        }
        Some(n) => n.to_string(),
        None => old.name.clone(),
    };
    // The view keeps its own settings (layer snapshot, UCS, background); it looks anew.
    let fresh = view_of(&new_name, c, old.width / old.height.max(1e-12));
    let v = NamedView {
        name: new_name.clone(),
        center: fresh.center,
        width: fresh.width,
        height: fresh.height,
        dir: fresh.dir,
        camera: Some(c),
        ..old.clone()
    };
    let d = s.doc_mut()?;
    if let Some(slot) = d.views.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&old.name)) {
        *slot = v;
    }
    Ok(json!({ "name": new_name, "view": new_name }))
}

fn run_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = find(s, "camera.delete", name_param("camera.delete", p)?)?.name;
    s.doc_mut()?.views.retain(|v| !v.name.eq_ignore_ascii_case(&name));
    Ok(json!({ "deleted": name }))
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    let name = find(s, "camera.view", name_param("camera.view", p)?)?.name;
    super::named_views::restore(s, &name)
}

/// CAMERA typed: the camera's location, then its target.
#[derive(Default)]
struct CameraM {
    position: Option<Vec2>,
}

impl Interactive for CameraM {
    fn name(&self) -> &'static str {
        "CAMERA"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.position {
            None => Prompt::new("Specify camera location", Accept::POINT),
            Some(p) => Prompt::new("Specify target location", Accept::POINT).base(p),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let p = match i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Point(p) => p,
            Input::Enter => return Ok(Step::Cancel),
            _ => return Err(EngineError::Other("Requires a point.".into())),
        };
        match self.position {
            None => {
                self.position = Some(p);
                Ok(Step::Continue)
            }
            Some(at) => {
                let z = s.doc()?.header.f64("CAMERAHEIGHT", 0.0);
                let r = place(s, at.to3(z), p.to3(0.0), 50.0, None)?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m);
                }
                Ok(Step::Done)
            }
        }
    }
}
