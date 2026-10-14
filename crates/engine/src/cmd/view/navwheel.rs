//! The steering wheel (NAVSWHEEL) and the ViewCube's settings (NAVVCUBE). The wheel's wedges
//! are commands, so the app's wheel, scripts and the control channel move the view alike:
//! `navswheel.zoom` about a point, `navswheel.pan`, `navswheel.orbit` about the pivot,
//! `navswheel.center` (which also sets the pivot), `navswheel.look` (turning from where the
//! viewer stands), `navswheel.rewind` (back through the views the wheel started from) and
//! `navswheel.walk`. Orbit and Look turn model space's 3D view on the Model tab; Zoom, Pan, Center
//! and Rewind also work on a layout's sheet and in a layout viewport (MSPACE).

use cadaddy_doc::{Handle, Space};
use cadaddy_geom::view3::{self, Projection};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{DocState, EngineError, Result, Session, View};

/// How many views Rewind keeps.
const REWIND_MAX: usize = 100;
/// Orbit and Look stop short of looking straight down or up (degrees of elevation).
const MAX_ELEVATION: f64 = 89.9;

/// A view to rewind to: where it was, its centre and height, and (on the Model tab) its
/// direction and twist.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    space: Space,
    mspace: Option<Handle>,
    center: Vec2,
    height: f64,
    dir: Vec3,
    twist: f64,
}

/// The wheel's state in a drawing: the views to rewind to (the latest last) and the pivot Orbit
/// turns about (set by Center; `None`: the view's centre).
#[derive(Clone, Debug, Default)]
pub struct Wheel {
    pub rewind: Vec<Frame>,
    pub pivot: Option<Vec3>,
}

/// The wedges of the full navigation wheel and their commands.
pub const WEDGES: [(&str, &str); 7] = [
    ("ZOOM", "navswheel.zoom"),
    ("REWIND", "navswheel.rewind"),
    ("PAN", "navswheel.pan"),
    ("ORBIT", "navswheel.orbit"),
    ("CENTER", "navswheel.center"),
    ("WALK", "navswheel.walk"),
    ("LOOK", "navswheel.look"),
];

pub fn specs() -> Vec<CommandSpec> {
    let wheel = |id: &'static str, label: &'static str, run: fn(&mut Session, &Value) -> Result<Value>, params: &'static str| {
        CommandSpec::new(id, label, run).params(params).noundo().transparent()
    };
    vec![
        CommandSpec::new("navswheel", "SteeringWheels", run_wheel)
            .menu(&["View", "SteeringWheels"])
            .alias(&["wheel"])
            .params("{} → {wedges: [{label, command}], size, opacity}: the app shows the wheel at the pointer; its wedges are the navswheel.* commands")
            .noundo()
            .transparent(),
        wheel(
            "navswheel.zoom",
            "Wheel Zoom",
            run_zoom,
            "{factor: > 1 zooms in, at?: [x, y] or [x, y, z] (the point that stays put; default the view centre), record?: true} → {center, height, dir, rewind}",
        ),
        wheel("navswheel.pan", "Wheel Pan", run_pan, "{by: [dx, dy] (drawing units across and up the screen), record?: true} → {center, height, dir, rewind}"),
        wheel(
            "navswheel.orbit",
            "Wheel Orbit",
            run_orbit,
            "{by: [azimuth, elevation] (degrees), pivot?: [x, y, z] (default the Center point, else the view centre), record?: true} → {center, height, dir, rewind} (Model tab)",
        ),
        wheel(
            "navswheel.look",
            "Wheel Look",
            run_look,
            "{by: [left, up] (degrees the viewer turns), record?: true} → {center, height, dir, rewind} (Model tab)",
        ),
        wheel("navswheel.center", "Wheel Center", run_center, "{point: [x, y] or [x, y, z]} → {center, height, dir, rewind}: centres the view there and orbits about it"),
        wheel("navswheel.rewind", "Wheel Rewind", run_rewind, "{steps?: 1} → {center, height, dir, rewind}: back to a view the wheel started from"),
        wheel("navswheel.walk", "Wheel Walk", |s, _| super::view3d::walk_mode(s, false), "{} → walk mode (in the app window)"),
        CommandSpec::new("navvcube", "ViewCube Settings", run_cube)
            .params(
                "{display?: on|off|2d|3d|0-3 (NAVVCUBEDISPLAY), location?: topright|topleft|bottomleft|bottomright, size?: small|normal|large|tiny|auto, \
                 opacity?: 0-100 (while the pointer is away), orient?: wcs|ucs} → the settings",
            )
            .noundo()
            .transparent(),
    ]
}

/// Whether the wheel records the view it starts from (`record`, on unless false: a drag's
/// later steps pass false, so Rewind goes back to where the drag began).
fn recording(p: &Value) -> bool {
    p.get("record").and_then(Value::as_bool).unwrap_or(true)
}

/// A point `[x, y]` (on the XY plane) or `[x, y, z]`.
fn point3(p: &Value, key: &str) -> Option<Vec3> {
    let a = p.get(key)?.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64);
    let q = match a.len() {
        2 => Vec3::new(n(0)?, n(1)?, 0.0),
        3 => Vec3::new(n(0)?, n(1)?, n(2)?),
        _ => return None,
    };
    q.is_finite().then_some(q)
}

/// Two numbers `[a, b]`.
fn pair(cmd: &str, p: &Value, key: &str) -> Result<(f64, f64)> {
    point_param(p, key).map(|v| (v.x, v.y)).ok_or_else(|| bad(cmd, format!("`{key}` must be two numbers [a, b]")))
}

/// The view the wheel works on: the active viewport's model view in MSPACE, else the view on
/// screen. Its centre is in the coordinates [`on_view`] gives.
fn current(st: &DocState) -> View {
    st.zoom_frame()
}

fn put(st: &mut DocState, v: View) -> Result<()> {
    if !(v.center.is_finite() && v.height.is_finite() && v.height > 1e-12 && v.height < 1e15) {
        return Err(EngineError::Other("The view would be out of range.".into()));
    }
    st.set_zoom_frame(v);
    Ok(())
}

/// A drawing point where the wheel's view has it: in a viewport (and in paper space) the
/// plan coordinates, on the Model tab the view plane.
fn on_view(st: &DocState, q: Vec3) -> Vec2 {
    if st.space != Space::Model || st.mspace.is_some() {
        return q.xy();
    }
    st.projection().map_or(q.xy(), |pr| pr.project(q))
}

/// The drawing point at the view's centre (on the view plane through the origin).
fn center_point(st: &DocState) -> Vec3 {
    let c = st.view().center;
    match st.projection() {
        Some(p) => p.right * c.x + p.up * c.y,
        None => c.to3(0.0),
    }
}

fn frame(st: &DocState) -> Frame {
    let v = current(st);
    Frame { space: st.space.clone(), mspace: st.mspace, center: v.center, height: v.height, dir: st.view_dir, twist: st.view_twist }
}

/// Keep the view the wheel starts from, for Rewind.
fn record(st: &mut DocState) {
    let f = frame(st);
    if st.wheel.rewind.last() != Some(&f) {
        st.wheel.rewind.push(f);
        if st.wheel.rewind.len() > REWIND_MAX {
            st.wheel.rewind.remove(0);
        }
    }
}

fn report(s: &Session) -> Result<Value> {
    let st = s.state()?;
    let v = current(st);
    let d = st.view_dir;
    Ok(json!({
        "center": [v.center.x, v.center.y],
        "height": v.height,
        "dir": [d.x, d.y, d.z],
        "label": view3::preset_label(d),
        "pivot": st.wheel.pivot.map(|p| json!([p.x, p.y, p.z])),
        "rewind": st.wheel.rewind.len(),
    }))
}

fn run_wheel(s: &mut Session, _p: &Value) -> Result<Value> {
    let st = &s.settings;
    Ok(json!({
        "wedges": WEDGES.iter().map(|(l, c)| json!({ "label": l, "command": c })).collect::<Vec<_>>(),
        "size": st.navswheel_size,
        "opacity": st.navswheel_opacity,
        "message": "The steering wheel follows the pointer in the app window: drag a wedge to zoom, orbit, pan or look; click Rewind, Center or Walk. Esc closes it.",
    }))
}

fn run_zoom(s: &mut Session, p: &Value) -> Result<Value> {
    let f = p.get("factor").and_then(Value::as_f64).filter(|f| f.is_finite() && *f > 1e-6 && *f < 1e6);
    let f = f.ok_or_else(|| bad("navswheel.zoom", "`factor` must be a positive number (> 1 zooms in)"))?;
    let at = point3(p, "at");
    let st = s.state_mut()?;
    if recording(p) {
        record(st);
    }
    let v = current(st);
    let at = at.map_or(v.center, |q| on_view(st, q));
    put(st, View { center: at + (v.center - at) * (1.0 / f), height: v.height / f })?;
    report(s)
}

fn run_pan(s: &mut Session, p: &Value) -> Result<Value> {
    let (dx, dy) = pair("navswheel.pan", p, "by")?;
    let st = s.state_mut()?;
    if recording(p) {
        record(st);
    }
    let v = current(st);
    put(st, View { center: v.center - Vec2::new(dx, dy), height: v.height })?;
    report(s)
}

fn model_tab(s: &Session, cmd: &str) -> Result<()> {
    let st = s.state()?;
    if st.space != Space::Model || st.mspace.is_some() {
        return Err(bad(cmd, "orbiting and looking around turn model space's view: use them on the Model tab"));
    }
    Ok(())
}

/// Look from azimuth/elevation `az`, `el` (radians), keeping `pivot` where it is on screen.
fn turn(st: &mut DocState, az: f64, el: f64, pivot: Vec3) -> Result<()> {
    let v = st.view();
    let at = |pr: Option<Projection>| pr.map_or(pivot.xy(), |pr| pr.project(pivot));
    let offset = at(st.projection()) - v.center;
    let el = el.clamp(-MAX_ELEVATION.to_radians(), MAX_ELEVATION.to_radians());
    let dir = Projection::dir_from_angles(az, el);
    if !dir.is_finite() {
        return Err(EngineError::Other("The view direction would be out of range.".into()));
    }
    st.view_dir = if view3::is_plan(dir) { Vec3::Z } else { dir };
    st.view_twist = 0.0;
    st.keep_model_twist();
    let center = at(st.projection()) - offset;
    st.set_view(View { center, height: v.height });
    Ok(())
}

/// The azimuth and elevation of the view (radians); the plan view looks from the south.
fn angles(st: &DocState) -> (f64, f64) {
    if view3::is_plan(st.view_dir) {
        (-std::f64::consts::FRAC_PI_2 - st.view_twist, std::f64::consts::FRAC_PI_2)
    } else {
        Projection::angles(st.view_dir)
    }
}

fn run_orbit(s: &mut Session, p: &Value) -> Result<Value> {
    model_tab(s, "navswheel.orbit")?;
    let (daz, del) = pair("navswheel.orbit", p, "by")?;
    let pivot = point3(p, "pivot");
    let st = s.state_mut()?;
    if recording(p) {
        record(st);
    }
    let pivot = pivot.or(st.wheel.pivot).unwrap_or_else(|| center_point(st));
    let (az, el) = angles(st);
    turn(st, az + daz.to_radians(), el + del.to_radians(), pivot)?;
    report(s)
}

/// Look: the viewer stays where they are and turns. In CADaddy's parallel views the viewer
/// stands in front of the view centre, a view's height away.
fn run_look(s: &mut Session, p: &Value) -> Result<Value> {
    model_tab(s, "navswheel.look")?;
    let (left, up) = pair("navswheel.look", p, "by")?;
    let st = s.state_mut()?;
    if recording(p) {
        record(st);
    }
    let eye = center_point(st) + st.view_dir.normalized() * st.view().height;
    let (az, el) = angles(st);
    // Turning left turns the line of sight (and the direction back to the viewer) left; looking
    // up lowers the direction back to the viewer.
    turn(st, az + left.to_radians(), el - up.to_radians(), eye)?;
    report(s)
}

fn run_center(s: &mut Session, p: &Value) -> Result<Value> {
    let q = point3(p, "point").ok_or_else(|| bad("navswheel.center", "`point` ([x, y] or [x, y, z]) is required"))?;
    let st = s.state_mut()?;
    record(st);
    st.wheel.pivot = Some(q);
    let v = current(st);
    put(st, View { center: on_view(st, q), height: v.height })?;
    report(s)
}

fn run_rewind(s: &mut Session, p: &Value) -> Result<Value> {
    let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(1).clamp(1, REWIND_MAX as u64);
    let st = s.state_mut()?;
    let now = frame(st);
    let mut left = steps;
    let mut to = None;
    while let Some(f) = st.wheel.rewind.pop() {
        // Views kept in another space or viewport are passed over, as is the view on screen.
        if f == now || f.space != now.space || f.mspace != now.mspace {
            continue;
        }
        left -= 1;
        if left == 0 {
            to = Some(f);
            break;
        }
    }
    let f = to.ok_or_else(|| EngineError::Other("Nothing to rewind to: the wheel hasn't moved this view yet.".into()))?;
    if f.space == Space::Model && f.mspace.is_none() {
        st.view_dir = f.dir;
        st.view_twist = f.twist;
        st.keep_model_twist();
    }
    put(st, View { center: f.center, height: f.height })?;
    report(s)
}

fn run_cube(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "navvcube";
    let pick = |key: &str, names: &[&str], max: i64| -> Result<Option<u8>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => {
                n.as_i64().filter(|i| (0..=max).contains(i)).map(|i| Some(i as u8)).ok_or_else(|| bad(cmd, format!("`{key}` must be 0-{max}")))
            }
            Some(Value::String(t)) => {
                let t = t.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
                names
                    .iter()
                    .position(|n| *n == t)
                    .map(|i| Some(i as u8))
                    .ok_or_else(|| bad(cmd, format!("`{key}` must be one of {}", names.join(", "))))
            }
            Some(Value::Bool(b)) if key == "display" => Ok(Some(if *b { 3 } else { 0 })),
            Some(_) => Err(bad(cmd, format!("`{key}` must be a name or a number"))),
        }
    };
    let display = pick("display", &["off", "2d", "3d", "on"], 3)?;
    let location = pick("location", &["topright", "topleft", "bottomleft", "bottomright"], 3)?;
    let size = pick("size", &["small", "normal", "large", "tiny", "auto"], 4)?;
    let orient = pick("orient", &["wcs", "ucs"], 1)?;
    let opacity = match p.get("opacity") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|o| (0.0..=100.0).contains(o)).ok_or_else(|| bad(cmd, "`opacity` must be 0-100"))?.round() as u8),
    };
    let st = &mut s.settings;
    if let Some(v) = display {
        st.navvcube_display = v;
    }
    if let Some(v) = location {
        st.navvcube_location = v;
    }
    if let Some(v) = size {
        st.navvcube_size = v;
    }
    if let Some(v) = orient {
        st.navvcube_orient = v;
    }
    if let Some(v) = opacity {
        st.navvcube_opacity = v;
    }
    fn names(list: &[&'static str], i: u8) -> &'static str {
        list.get(usize::from(i)).copied().unwrap_or("")
    }
    Ok(json!({
        "display": st.navvcube_display,
        "location": names(&["topright", "topleft", "bottomleft", "bottomright"], st.navvcube_location),
        "size": names(&["small", "normal", "large", "tiny", "auto"], st.navvcube_size),
        "opacity": st.navvcube_opacity,
        "orient": names(&["wcs", "ucs"], st.navvcube_orient),
    }))
}
