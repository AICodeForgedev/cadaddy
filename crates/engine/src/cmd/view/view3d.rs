//! 3D views of model space: the preset views (View ▸ 3D Views, the ViewCube), PLAN and the
//! view direction 3DORBIT changes. Views are orthographic; the drawing is seen in wireframe.

use cadaddy_doc::{Drawing, Space, entity_bounds};
use cadaddy_geom::view3::{self, Projection};
use cadaddy_geom::{Bounds2, Vec3};
use serde_json::{Value, json};

use super::super::*;
use crate::{Result, Session, View};
use cadaddy_render::shade::VisualStyle;

/// A command's JSON form.
type CommandFn = fn(&mut Session, &Value) -> Result<Value>;

pub(super) fn specs() -> Vec<CommandSpec> {
    // View ▸ 3D Views, in AutoCAD's order.
    let mut v = vec![
        CommandSpec::new("view.top", "Top", |s, _| preset_view(s, "top")).menu(&["View", "3D Views", "Top"]).noundo().transparent(),
        CommandSpec::new("view.bottom", "Bottom", |s, _| preset_view(s, "bottom")).menu(&["View", "3D Views", "Bottom"]).noundo().transparent(),
        CommandSpec::new("view.left", "Left", |s, _| preset_view(s, "left")).menu(&["View", "3D Views", "Left"]).noundo().transparent(),
        CommandSpec::new("view.right", "Right", |s, _| preset_view(s, "right")).menu(&["View", "3D Views", "Right"]).noundo().transparent(),
        CommandSpec::new("view.front", "Front", |s, _| preset_view(s, "front")).menu(&["View", "3D Views", "Front"]).noundo().transparent(),
        CommandSpec::new("view.back", "Back", |s, _| preset_view(s, "back")).menu(&["View", "3D Views", "Back"]).noundo().transparent(),
        CommandSpec::new("view.swiso", "SW Isometric", |s, _| preset_view(s, "swiso"))
            .menu(&["View", "3D Views", "SW Isometric"])
            .noundo()
            .transparent(),
        CommandSpec::new("view.seiso", "SE Isometric", |s, _| preset_view(s, "seiso"))
            .menu(&["View", "3D Views", "SE Isometric"])
            .noundo()
            .transparent(),
        CommandSpec::new("view.neiso", "NE Isometric", |s, _| preset_view(s, "neiso"))
            .menu(&["View", "3D Views", "NE Isometric"])
            .noundo()
            .transparent(),
        CommandSpec::new("view.nwiso", "NW Isometric", |s, _| preset_view(s, "nwiso"))
            .menu(&["View", "3D Views", "NW Isometric"])
            .noundo()
            .transparent(),
    ];
    v.push(
        CommandSpec::new("vpoint", "Viewpoint", vpoint_cmd)
            .params("{dir: [x, y, z] (from the target towards the viewer)} | {azimuth, elevation} (degrees: from +X in the plan, up from it); fit?: false")
            .noundo()
            .transparent(),
    );
    v.push(
        CommandSpec::new("plan", "Plan View", run_plan)
            .menu(&["View", "3D Views", "Plan View", "Current UCS"])
            .params("{option?: current|world|ucs (default current), name? (ucs: a named UCS)} — the plan view of that coordinate system (its X axis across the screen), zoomed to the extents")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(PlanM))),
    );
    v.push(
        CommandSpec::new("plan.world", "Plan View (World)", |s, _| run_plan(s, &json!({ "option": "world" })))
            .menu(&["View", "3D Views", "Plan View", "World UCS"])
            .noundo()
            .transparent(),
    );
    // 3DWALK and 3DFLY are modes of the app window (the walk view of the drawing); the engine
    // only says so, and reports the walking speed (STEPSIZE × STEPSPERSEC).
    const WALK_PARAMS: &str =
        "{} → {mode, stepSize, stepsPerSec} (the app shows the drawing in 3D and walks or flies through it; WASD and the mouse, Esc returns)";
    v.push(
        CommandSpec::new("3dwalk", "Walk", |s, _| walk_mode(s, false))
            .menu(&["View", "Walk and Fly", "Walk"])
            .alias(&["3dw"])
            .params(WALK_PARAMS)
            .noundo(),
    );
    v.push(
        CommandSpec::new("3dfly", "Fly", |s, _| walk_mode(s, true))
            .menu(&["View", "Walk and Fly", "Fly"])
            .alias(&["3df"])
            .params(WALK_PARAMS)
            .noundo(),
    );
    v.push(
        CommandSpec::new("vscurrent", "Visual Style", vscurrent_cmd)
            .alias(&["vs", "shademode"])
            .params(
                "{style: 2dwireframe|wireframe|hidden|shaded|shadedwithedges|realistic} → {style} — how model space shows its 3D objects (VSCURRENT)",
            )
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(VsM))),
    );
    // View ▸ Visual Styles, in AutoCAD's order.
    let styles: [(&'static str, VisualStyle, &'static [&'static str], CommandFn); 6] = [
        ("vs.2dwireframe", VisualStyle::Wireframe2d, &["View", "Visual Styles", "2D Wireframe"], |s, _| set_style(s, VisualStyle::Wireframe2d)),
        ("vs.wireframe", VisualStyle::Wireframe, &["View", "Visual Styles", "Wireframe"], |s, _| set_style(s, VisualStyle::Wireframe)),
        ("vs.hidden", VisualStyle::Hidden, &["View", "Visual Styles", "Hidden"], |s, _| set_style(s, VisualStyle::Hidden)),
        ("vs.realistic", VisualStyle::Realistic, &["View", "Visual Styles", "Realistic"], |s, _| set_style(s, VisualStyle::Realistic)),
        ("vs.shaded", VisualStyle::Shaded, &["View", "Visual Styles", "Shaded"], |s, _| set_style(s, VisualStyle::Shaded)),
        ("vs.shadededges", VisualStyle::ShadedEdges, &["View", "Visual Styles", "Shaded with Edges"], |s, _| set_style(s, VisualStyle::ShadedEdges)),
    ];
    for (id, vs, menu, run) in styles {
        v.push(CommandSpec::new(id, vs.name(), run).menu(menu).noundo().transparent());
    }
    v.push(
        CommandSpec::new("view.twist", "View Twist", view_twist_cmd)
            .params("{angle: degrees (counterclockwise) | by: degrees (added to the current twist)} → {twist} — turns the plan view of model space about the centre of the screen, or the active MSPACE viewport's view about its centre (VIEWTWIST)")
            .noundo()
            .transparent(),
    );
    v.push(
        CommandSpec::new("view.dir", "View Direction", view_dir_cmd)
            .params("{dir: [x, y, z] (from the target towards the viewer; [0,0,1] = plan) | preset: top|bottom|front|back|left|right|swiso|seiso|neiso|nwiso, fit?: bool (zoom to the extents; default: true for a preset)} → {dir, label?}")
            .noundo()
            .transparent(),
    );
    v
}

fn view_twist_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let num = |k: &str| p.get(k).map(|v| v.as_f64().filter(|a| a.is_finite() && a.abs() <= 1e6));
    let st = s.state_mut()?;
    let t = match (num("angle"), num("by")) {
        (Some(Some(a)), None) => a.to_radians(),
        (None, Some(Some(d))) => st.twist() + d.to_radians(),
        _ => return Err(bad("view.twist", "pass `angle` or `by` (degrees)")),
    };
    if !st.set_twist(t) {
        return Err(EngineError::Other("View twist is available in the plan view of model space and in layout viewports.".into()));
    }
    let twist = st.twist();
    s.touch();
    Ok(json!({ "twist": twist.to_degrees() }))
}

fn view_dir_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(name) = str_param(p, "preset") {
        let fit = p.get("fit").and_then(Value::as_bool).unwrap_or(true);
        let dir = view3::preset(name).ok_or_else(|| bad("view.dir", format!("unknown preset view \"{name}\"")))?;
        return set_dir(s, dir, fit);
    }
    let dir = p
        .get("dir")
        .and_then(Value::as_array)
        .and_then(|a| match a.as_slice() {
            [x, y, z] => Some(Vec3::new(x.as_f64()?, y.as_f64()?, z.as_f64()?)),
            _ => None,
        })
        .filter(|d| d.is_finite() && d.len() > 1e-12)
        .ok_or_else(|| bad("view.dir", "`dir` must be three numbers, not all zero"))?;
    set_dir(s, dir, p.get("fit").and_then(Value::as_bool).unwrap_or(false))
}

/// VPOINT: look from a direction (a vector, or an azimuth and elevation).
fn vpoint_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
    let dir = match p.get("dir").and_then(Value::as_array) {
        Some(a) => {
            let c: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            let [x, y, z] = c[..] else { return Err(bad("vpoint", "`dir` is [x, y, z]")) };
            Vec3::new(x, y, z)
        }
        None => match (num("azimuth"), num("elevation")) {
            (Some(a), Some(e)) => Projection::dir_from_angles(a.to_radians(), e.to_radians()),
            _ => return Err(bad("vpoint", "give `dir` or `azimuth` and `elevation`")),
        },
    };
    if !(dir.is_finite() && dir.len() > 1e-12) {
        return Err(bad("vpoint", "the direction needs a length"));
    }
    set_dir(s, dir, p.get("fit").and_then(Value::as_bool).unwrap_or(false))
}

/// Switch to the preset view `name` and zoom to the extents, as AutoCAD does.
pub(super) fn preset_view(s: &mut Session, name: &str) -> Result<Value> {
    let dir = view3::preset(name).ok_or_else(|| bad("view", format!("unknown preset view \"{name}\"")))?;
    set_dir(s, dir, true)
}

/// Look along `-dir`. The point under the view centre stays there (orbiting turns about it);
/// `fit` then zooms to the extents.
pub fn set_dir(s: &mut Session, dir: Vec3, fit: bool) -> Result<Value> {
    {
        let st = s.state()?;
        if st.space != Space::Model || st.mspace.is_some() {
            return Err(EngineError::Other("3D views are available in model space.".into()));
        }
    }
    let st = s.state_mut()?;
    let v = st.view();
    // A world point that projects to the view centre: on the view plane through the origin.
    let target = match st.projection() {
        Some(p) => p.right * v.center.x + p.up * v.center.y,
        None => Vec3::new(v.center.x, v.center.y, 0.0),
    };
    st.view_dir = if view3::is_plan(dir) { Vec3::Z } else { dir.normalized() };
    // A new direction comes with the standard orientation (as PLAN and the ViewCube faces do).
    st.view_twist = 0.0;
    let center = match st.projection() {
        Some(p) => p.project(target),
        None => target.xy(),
    };
    st.set_view(View { center, height: v.height });
    if fit {
        s.zoom_extents()?;
    }
    let dir = s.state()?.view_dir;
    Ok(json!({ "dir": [dir.x, dir.y, dir.z], "label": view3::preset_label(dir) }))
}

/// The extents of model space as seen through `p`: on the view plane.
pub fn projected_extents(d: &Drawing, p: &Projection) -> Bounds2 {
    let mut b = Bounds2::EMPTY;
    for e in d.model.iter() {
        if !d.is_visible(e) {
            continue;
        }
        if let cadaddy_doc::EntityKind::Mesh(m) = &e.kind {
            b = b.union(&Bounds2::from_points(m.vertices.iter().map(|q| p.project(*q))));
            continue;
        }
        if let cadaddy_doc::EntityKind::Solid3d(so) = &e.kind {
            b = b.union(&Bounds2::from_points(so.wires(so.mesh_tol()).iter().flatten().map(|q| p.project(*q))));
            continue;
        }
        if let Some((pts, _)) = e.kind.points3d() {
            for q in pts {
                b = b.union(&Bounds2::from_points([p.project(q)]));
            }
            continue;
        }
        let eb = entity_bounds(d, e, 0);
        if eb.is_empty() {
            continue;
        }
        let z = e.kind.elevation();
        // A circle or arc in a tilted plane: the box of its sphere.
        let tilted = e.common.extrusion.x.abs() > 1e-9 || e.common.extrusion.y.abs() > 1e-9;
        let (lo, hi) = match &e.kind {
            cadaddy_doc::EntityKind::Circle(c) if tilted => {
                (c.center - Vec3::new(c.radius, c.radius, c.radius), c.center + Vec3::new(c.radius, c.radius, c.radius))
            }
            cadaddy_doc::EntityKind::Arc(a) if tilted => {
                (a.center - Vec3::new(a.radius, a.radius, a.radius), a.center + Vec3::new(a.radius, a.radius, a.radius))
            }
            _ => (Vec3::new(eb.min.x, eb.min.y, z), Vec3::new(eb.max.x, eb.max.y, z)),
        };
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for zz in [lo.z, hi.z] {
                    b = b.union(&Bounds2::from_points([p.project(Vec3::new(x, y, zz))]));
                }
            }
        }
    }
    b
}

/// PLAN: the plan view of the current UCS, the world or a named UCS.
fn run_plan(s: &mut Session, p: &Value) -> Result<Value> {
    let f = match str_param(p, "option").unwrap_or("current").trim().to_ascii_lowercase().as_str() {
        "current" | "c" | "current ucs" => crate::ucs::Frame::current(s.doc()?),
        "world" | "w" => crate::ucs::Frame::WORLD,
        "ucs" | "u" => {
            let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad("plan", "`name` (a named UCS) is required"))?;
            let rec = s
                .doc()?
                .ucss
                .iter()
                .find(|u| u.name.eq_ignore_ascii_case(name))
                .cloned()
                .ok_or_else(|| EngineError::Other(format!("Could not find UCS \"{name}\".")))?;
            crate::ucs::Frame::from_record(&rec).ok_or_else(|| EngineError::Other(format!("UCS \"{name}\" is not in the plan.")))?
        }
        _ => return Err(bad("plan", "option must be current, world or ucs")),
    };
    super::super::ucs::plan_of(s, &f)?;
    Ok(json!({ "twist": s.state()?.twist().to_degrees() }))
}

/// PLAN typed: "Enter an option [Current ucs/Ucs/World] <Current>:".
struct PlanM;

impl crate::Interactive for PlanM {
    fn name(&self) -> &'static str {
        "PLAN"
    }
    fn prompt(&self, _s: &Session) -> crate::Prompt {
        crate::Prompt::new("Enter an option", crate::Accept::TEXT).kw(&["Current ucs", "Ucs", "World"]).default("Current ucs")
    }
    fn input(&mut self, s: &mut Session, i: crate::Input) -> Result<crate::Step> {
        let t = match &i {
            crate::Input::Text(t) | crate::Input::Keyword(t) => t.trim().to_ascii_lowercase(),
            crate::Input::Enter => "current".into(),
            _ => return Err(EngineError::Other("Invalid option keyword.".into())),
        };
        let p = match t.as_str() {
            "c" | "current" | "current ucs" => json!({ "option": "current" }),
            "w" | "world" => json!({ "option": "world" }),
            // A named UCS: its name, typed as the option.
            "u" | "ucs" => {
                return Err(EngineError::Other("Type the name of the UCS (for example PLAN then a UCS name), or use Current ucs / World.".into()));
            }
            name => json!({ "option": "ucs", "name": name }),
        };
        run_plan(s, &p)?;
        Ok(crate::Step::Done)
    }
}

fn set_style(s: &mut Session, vs: VisualStyle) -> Result<Value> {
    s.state_mut()?.visual_style = vs;
    s.touch();
    Ok(json!({ "style": vs.name() }))
}

fn vscurrent_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "style").ok_or_else(|| bad("vscurrent", "`style` is required"))?;
    let vs = VisualStyle::parse(name).ok_or_else(|| {
        bad("vscurrent", format!("unknown visual style \"{name}\" (2dwireframe, wireframe, hidden, shaded, shadedwithedges, realistic)"))
    })?;
    set_style(s, vs)
}

/// VSCURRENT typed: "Enter an option [2dwireframe/Wireframe/Hidden/Realistic/Shaded/shaded with
/// Edges] <current>:".
struct VsM;

impl crate::Interactive for VsM {
    fn name(&self) -> &'static str {
        "VSCURRENT"
    }
    fn prompt(&self, s: &Session) -> crate::Prompt {
        let cur = s.state().map(|st| st.visual_style.name()).unwrap_or("2D Wireframe");
        crate::Prompt::new("Enter an option", crate::Accept::TEXT)
            .kw(&["2dwireframe", "Wireframe", "Hidden", "Realistic", "Shaded", "shaded with Edges"])
            .default(cur)
    }
    fn input(&mut self, s: &mut Session, i: crate::Input) -> Result<crate::Step> {
        match i {
            crate::Input::Enter => Ok(crate::Step::Done),
            crate::Input::Text(t) | crate::Input::Keyword(t) => {
                let vs = VisualStyle::parse(&t).ok_or_else(|| EngineError::Other("Invalid option keyword.".into()))?;
                set_style(s, vs)?;
                Ok(crate::Step::Done)
            }
            _ => Err(EngineError::Other("Invalid option keyword.".into())),
        }
    }
}

/// How model space would print or export as displayed: `None` in the plain plan view with 2D
/// Wireframe (the usual vector output), else the visual style (2D Wireframe in a 3D view: see-
/// through Wireframe) with the view's direction and twist.
pub fn shade_options(s: &Session) -> Option<Value> {
    let st = s.state().ok()?;
    if st.space != Space::Model || st.mspace.is_some() {
        return None;
    }
    let p = st.projection();
    let three_d = p.is_some_and(|p| !view3::is_plan(p.dir));
    if st.visual_style == VisualStyle::Wireframe2d && !three_d {
        return None;
    }
    let style = if st.visual_style == VisualStyle::Wireframe2d { VisualStyle::Wireframe } else { st.visual_style };
    let dir = p.map_or(Vec3::Z, |p| p.dir);
    Some(json!({ "style": style.name(), "dir": [dir.x, dir.y, dir.z], "twist": st.twist() }))
}

/// A PNG of model space as displayed in a 3D view or a visual style, fitted into `w` × `h` on
/// white; `None` when the plain plan vectors apply ([`shade_options`]) or nothing is drawn.
pub fn shaded_png(s: &Session, w: u32, h: u32) -> Option<Vec<u8>> {
    let opts = shade_options(s)?;
    let st = s.state().ok()?;
    let style = opts.get("style").and_then(Value::as_str).and_then(VisualStyle::parse)?;
    let proj = st.projection();
    cadaddy_render::shade::render_fitted(&st.doc, proj, style, w, h, cadaddy_color::Rgb(255, 255, 255))?.encode_png()
}

fn walk_mode(s: &mut Session, fly: bool) -> Result<Value> {
    let d = s.doc()?;
    let size = d.header.f64("STEPSIZE", 6.0);
    let rate = d.header.f64("STEPSPERSEC", 2.0);
    Ok(json!({
        "mode": if fly { "fly" } else { "walk" },
        "stepSize": size,
        "stepsPerSec": rate,
        "message": "Walking and flying happen in the app window (View ▸ Walk and Fly).",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Line};

    fn session_with(lines: &[(Vec3, Vec3)]) -> Session {
        let mut s = Session::default();
        let _ = s.execute("new", &json!({}));
        if let Ok(st) = s.state_mut() {
            let d = std::sync::Arc::make_mut(&mut st.doc);
            for (a, b) in lines {
                let _ = d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: *a, b: *b }));
            }
        }
        s
    }

    #[test]
    fn presets_set_the_direction_and_plan_goes_back() {
        let mut s = session_with(&[(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)), (Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0))]);
        let r = s.execute("view.swiso", &json!({}));
        assert!(r.is_ok(), "{r:?}");
        assert!(s.state().is_ok_and(|st| st.projection().is_some()));
        assert_eq!(r.ok().and_then(|v| v["label"].as_str().map(String::from)).as_deref(), Some("SW Isometric"));
        assert!(s.execute("plan", &json!({})).is_ok());
        assert!(s.state().is_ok_and(|st| st.projection().is_none() && st.view_dir == Vec3::Z));
    }

    #[test]
    fn front_view_extents_show_heights() {
        let mut s = session_with(&[(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)), (Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0))]);
        assert!(s.execute("view.front", &json!({})).is_ok());
        let st = s.state();
        let b = st.ok().and_then(|st| st.projection().map(|p| projected_extents(&st.doc, &p)));
        // Front: x across, z up.
        assert!(b.is_some_and(|b| (b.width() - 10.0).abs() < 1e-9 && (b.height() - 5.0).abs() < 1e-9), "{b:?}");
        // The view is zoomed to them.
        assert!(s.state().is_ok_and(|st| (st.view().center.y - 2.5).abs() < 1e-6));
    }

    #[test]
    fn view_dir_rejects_nonsense_and_layouts() {
        let mut s = session_with(&[]);
        assert!(s.execute("view.dir", &json!({"dir": [0, 0, 0]})).is_err());
        assert!(s.execute("view.dir", &json!({"preset": "diagonal"})).is_err());
        assert!(s.execute("view.dir", &json!({"dir": [1, 1, 1]})).is_ok());
    }
}
