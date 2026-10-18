//! 3D views of model space: the preset views (View ▸ 3D Views, the ViewCube), PLAN and the
//! view direction 3DORBIT changes. Views are orthographic; the drawing is seen in wireframe.

use cadaddy_doc::{Drawing, Space, entity_bounds};
use cadaddy_geom::view3::{self, Projection};
use cadaddy_geom::{Bounds2, Vec3};
use serde_json::{Value, json};

use super::super::*;
use crate::{Result, Session, View};

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
        CommandSpec::new("plan", "Plan View", |s, _| preset_view(s, "top"))
            .menu(&["View", "3D Views", "Plan View", "World UCS"])
            .params("{} — the plan view of the world coordinate system, zoomed to the extents")
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
