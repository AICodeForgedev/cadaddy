//! User coordinate systems in the plan (gaps F24): UCS World / Origin / Z / Object / View /
//! Previous / NAmed, typed coordinates and Ortho, polar tracking and the snap grid in the UCS,
//! PLAN of the current UCS, UCSFOLLOW, UCSICON, ID in UCS coordinates, and the UCS table and
//! current UCS through DXF.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::snap::tracking::CursorQuery;
use cadaddy_engine::ucs::Frame;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.dist(b) < 1e-9
}

fn ucs(s: &Session) -> Frame {
    s.ucs()
}

fn set(s: &mut Session, origin: [f64; 2], angle: f64) -> Value {
    s.execute("ucs", &json!({ "option": "set", "origin": origin, "angle": angle })).unwrap()
}

fn typed(s: &mut Session, lines: &[&str]) {
    for l in lines {
        s.cmdline(l).unwrap();
    }
}

fn last_line(s: &Session) -> (Vec2, Vec2) {
    match s.doc().unwrap().model.last().map(|e| e.kind.clone()) {
        Some(EntityKind::Line(l)) => (l.a.xy(), l.b.xy()),
        k => panic!("not a line: {k:?}"),
    }
}

#[test]
fn typed_coordinates_are_read_in_the_ucs() {
    let mut s = session();
    set(&mut s, [10.0, 5.0], 90.0);
    // Absolute: UCS 0,0 is the world 10,5; UCS X runs up the world Y axis.
    typed(&mut s, &["line", "0,0", "4,0", ""]);
    let (a, b) = last_line(&s);
    assert!(near(a, Vec2::new(10.0, 5.0)) && near(b, Vec2::new(10.0, 9.0)), "{a:?} {b:?}");
    // Relative and polar follow the UCS axes; `*` is the world.
    typed(&mut s, &["line", "0,0", "@0,3", "@2<0", "*0,0", ""]);
    let d = s.doc().unwrap();
    let all: Vec<Vec2> = d.model.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.b.xy()) } else { None }).collect();
    let pts: Vec<Vec2> = all.iter().rev().take(3).copied().collect();
    assert!(near(pts[2], Vec2::new(7.0, 5.0)), "@0,3 along UCS Y = world -X: {pts:?}");
    assert!(near(pts[1], Vec2::new(7.0, 7.0)), "@2<0 along UCS X = world +Y: {pts:?}");
    assert!(near(pts[0], Vec2::ZERO), "*0,0 is the world origin: {pts:?}");
    // The same through the session's parser, as Dynamic Input and agents see it.
    assert!(near(s.parse_typed_point("1,0").unwrap(), Vec2::new(10.0, 6.0)));
    assert!(near(s.parse_typed_point("*1,0").unwrap(), Vec2::new(1.0, 0.0)));
    // ID reports UCS coordinates.
    let r = s.execute("id", &json!({ "at": [10, 7] })).unwrap();
    let u = Vec2::new(r["ucs"][0].as_f64().unwrap(), r["ucs"][1].as_f64().unwrap());
    assert!(near(u, Vec2::new(2.0, 0.0)), "{r}");
    assert!(r["message"].as_str().unwrap().starts_with("X = 2"), "{r}");
}

#[test]
fn origin_z_object_view_world_and_previous() {
    let mut s = session();
    // Origin with a point on the X axis.
    s.execute("ucs", &json!({ "option": "origin", "origin": [2, 2], "xpoint": [2, 7] })).unwrap();
    assert!(near(ucs(&s).origin, Vec2::new(2.0, 2.0)) && (ucs(&s).angle.to_degrees() - 90.0).abs() < 1e-9);
    // Z turns it further (90 by default).
    s.execute("ucs", &json!({ "option": "z", "angle": 45 })).unwrap();
    assert!((ucs(&s).angle.to_degrees() - 135.0).abs() < 1e-9);
    s.execute("ucs", &json!({ "option": "z" })).unwrap();
    assert!((ucs(&s).angle.to_degrees() - 225.0).abs() < 1e-9);
    // Object: a line from its end nearest the pick.
    let l = s.execute("line", &json!({ "points": [[0, 0], [3, 4]] })).unwrap();
    let h = l["handle"].as_str().or_else(|| l["handles"][0].as_str()).unwrap().to_string();
    s.execute("ucs", &json!({ "option": "object", "handle": h, "pick": [2.9, 3.9] })).unwrap();
    let f = ucs(&s);
    assert!(near(f.origin, Vec2::new(3.0, 4.0)) && (f.angle - Vec2::new(-3.0, -4.0).angle()).abs() < 1e-9, "{f:?}");
    let c = s.execute("circle", &json!({ "center": [5, 5], "radius": 1 })).unwrap();
    s.execute("ucs", &json!({ "option": "object", "handle": c["handle"], "pick": [5, 9] })).unwrap();
    assert!(near(ucs(&s).origin, Vec2::new(5.0, 5.0)) && (ucs(&s).angle.to_degrees() - 90.0).abs() < 1e-9);
    let t = s.execute("text", &json!({ "at": [1, 1], "text": "A", "rotation": 30 })).unwrap();
    s.execute("ucs", &json!({ "option": "object", "handle": t["handle"] })).unwrap();
    assert!((ucs(&s).angle.to_degrees() - 30.0).abs() < 1e-9);
    let hatch_less = s.execute("ray", &json!({ "base": [0, 0], "through": [1, 1] }));
    if let Ok(r) = hatch_less {
        assert!(s.execute("ucs", &json!({ "option": "object", "handle": r["handle"] })).is_err(), "a ray doesn't define one");
    }
    // View: X across the screen.
    s.execute("view.twist", &json!({ "angle": 20 })).unwrap();
    s.execute("ucs", &json!({ "option": "view" })).unwrap();
    assert!(
        (ucs(&s).angle.to_degrees() + 20.0).rem_euclid(360.0) < 1e-9 || ((ucs(&s).angle.to_degrees() + 20.0).rem_euclid(360.0) - 360.0).abs() < 1e-9
    );
    // Previous walks back; World resets.
    s.execute("ucs", &json!({ "option": "previous" })).unwrap();
    assert!((ucs(&s).angle.to_degrees() - 30.0).abs() < 1e-9);
    s.execute("ucs", &json!({ "option": "world" })).unwrap();
    assert!(ucs(&s).is_world());
    // A UCS change is an undo step.
    s.undo().unwrap();
    assert!((ucs(&s).angle.to_degrees() - 30.0).abs() < 1e-9);
    for bad in [
        json!({ "option": "x" }),
        json!({ "option": "origin" }),
        json!({ "option": "origin", "origin": [1, 1], "xpoint": [1, 1] }),
        json!({ "option": "z", "angle": "steep" }),
        json!({ "option": "object", "handle": "FFFFFF" }),
        json!({ "option": "sideways" }),
    ] {
        assert!(s.execute("ucs", &bad).is_err(), "{bad}");
    }
}

#[test]
fn named_ucs_save_restore_delete_and_dxf() {
    let mut s = session();
    set(&mut s, [100.0, 50.0], 30.0);
    let r = s.execute("ucs", &json!({ "option": "save", "name": "Site" })).unwrap();
    assert_eq!(r["name"], "Site");
    assert_eq!(ucs(&s).name, "Site");
    s.execute("ucs", &json!({ "option": "world" })).unwrap();
    s.execute("ucs", &json!({ "option": "restore", "name": "site" })).unwrap();
    let f = ucs(&s);
    assert!(near(f.origin, Vec2::new(100.0, 50.0)) && (f.angle.to_degrees() - 30.0).abs() < 1e-9 && f.name == "Site");
    let l = s.execute("ucs", &json!({ "option": "list" })).unwrap();
    assert_eq!(l["ucs"][0]["name"], "Site");
    assert!(l["message"].as_str().unwrap().contains("Current ucs name:  Site"));
    // Saved with the drawing: the table and the current UCS.
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    assert!(text.contains("$UCSORG") && text.contains("$UCSNAME"));
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    let g = Frame::current(&back);
    assert!(near(g.origin, f.origin) && (g.angle - f.angle).abs() < 1e-9 && g.name == "Site", "{g:?}");
    assert_eq!(back.ucss.len(), 1);
    // Delete: the current one stays, unnamed.
    s.execute("ucs", &json!({ "option": "delete", "names": "S*" })).unwrap();
    assert!(s.doc().unwrap().ucss.is_empty() && ucs(&s).name.is_empty() && !ucs(&s).is_world());
    for bad in [
        json!({ "option": "save", "name": "World" }),
        json!({ "option": "save", "name": "a*b" }),
        json!({ "option": "save" }),
        json!({ "option": "restore", "name": "Nope" }),
        json!({ "option": "delete", "names": "Nope" }),
    ] {
        assert!(s.execute("ucs", &bad).is_err(), "{bad}");
    }
    // A named UCS tilted out of the plan can't be restored in 2D.
    std::sync::Arc::make_mut(&mut s.state_mut().unwrap().doc).ucss.push(cadaddy_engine::doc::Ucs {
        name: "Tilted".into(),
        origin: cadaddy_engine::geom::Vec3::ZERO,
        x_axis: cadaddy_engine::geom::Vec3::new(0.0, 0.0, 1.0),
        y_axis: cadaddy_engine::geom::Vec3::new(0.0, 1.0, 0.0),
    });
    assert!(s.execute("ucs", &json!({ "option": "restore", "name": "Tilted" })).is_err());
}

fn cursor(s: &mut Session, raw: Vec2, base: Option<Vec2>) -> Vec2 {
    s.snap_cursor(raw, &CursorQuery { base, deferred: false, corner: false, aperture: 1e-6, tol: 1e-6, now: 0.0, shift: false }).point
}

#[test]
fn ortho_polar_and_snap_follow_the_ucs() {
    let mut s = session();
    set(&mut s, [1.0, 1.0], 45.0);
    // Ortho: along the UCS axes from the base.
    s.settings.orthomode = true;
    let p = cursor(&mut s, Vec2::new(5.0, 4.0), Some(Vec2::new(1.0, 1.0)));
    let d = p - Vec2::new(1.0, 1.0);
    assert!((d.angle().to_degrees() - 45.0).abs() < 1e-6, "{p:?}");
    s.settings.orthomode = false;
    // Polar tracking at 90°: the UCS's axes, here the world's 45° and 135° directions.
    s.settings.polarmode = true;
    s.settings.polarang = 90f64.to_radians();
    let p = s
        .snap_cursor(
            Vec2::new(-2.0, 4.05),
            &CursorQuery { base: Some(Vec2::new(1.0, 1.0)), deferred: false, corner: false, aperture: 1e-6, tol: 0.5, now: 0.0, shift: false },
        )
        .point;
    let a = (p - Vec2::new(1.0, 1.0)).angle().to_degrees();
    assert!((a - 135.0).abs() < 1e-6, "tracked {a}");
    s.settings.polarmode = false;
    // Grid snap: the grid runs from the UCS origin along its axes.
    s.execute("setvar", &json!({ "name": "SNAPUNIT", "value": [1, 1] })).ok();
    s.settings.snapmode = true;
    let p = cursor(&mut s, Vec2::new(1.0 + 0.72, 1.0 + 0.69), None);
    assert!(near(p, Vec2::new(1.0, 1.0) + Vec2::from_angle(45f64.to_radians()) * 1.0), "{p:?}");
    let (o, ang) = cadaddy_engine::snap::grid_frame(s.doc().unwrap());
    assert!(near(o, Vec2::new(1.0, 1.0)) && (ang.to_degrees() - 45.0).abs() < 1e-9);
    assert!(near(cadaddy_engine::snap::ortho_at(Vec2::ZERO, Vec2::new(3.0, 1.0), 0.0), Vec2::new(3.0, 0.0)));
}

#[test]
fn plan_shows_the_ucs_and_ucsfollow_turns_the_view() {
    let mut s = session();
    s.execute("line", &json!({ "points": [[0, 0], [10, 10]] })).unwrap();
    set(&mut s, [0.0, 0.0], 30.0);
    s.execute("plan", &json!({})).unwrap();
    let p = s.state().unwrap().projection().expect("a twisted plan view");
    // The UCS X axis runs straight across the screen.
    let x = Vec2::from_angle(30f64.to_radians());
    assert!(near(Vec2::new(x.to3(0.0).dot(p.right), x.to3(0.0).dot(p.up)), Vec2::new(1.0, 0.0)), "{p:?}");
    s.execute("plan", &json!({ "option": "world" })).unwrap();
    assert!(s.state().unwrap().projection().is_none());
    s.execute("ucs", &json!({ "option": "save", "name": "A" })).unwrap();
    s.execute("plan", &json!({ "option": "ucs", "name": "A" })).unwrap();
    assert!(
        (s.state().unwrap().twist().to_degrees() + 30.0).rem_euclid(360.0) < 1e-6
            || ((s.state().unwrap().twist().to_degrees() + 30.0).rem_euclid(360.0) - 360.0).abs() < 1e-6
    );
    // Typed PLAN: Enter is the current UCS.
    s.execute("plan", &json!({ "option": "world" })).unwrap();
    typed(&mut s, &["plan", ""]);
    assert!(s.state().unwrap().projection().is_some());
    // UCSFOLLOW 1: a new UCS brings its plan view.
    s.execute("plan", &json!({ "option": "world" })).unwrap();
    s.doc_mut().unwrap().header.set_i64("UCSFOLLOW", 1);
    set(&mut s, [0.0, 0.0], 60.0);
    assert!(s.state().unwrap().projection().is_some_and(
        |p| (p.twist().to_degrees() + 60.0).rem_euclid(360.0) < 1e-6 || ((p.twist().to_degrees() + 60.0).rem_euclid(360.0) - 360.0).abs() < 1e-6
    ));
}

#[test]
fn typed_ucs_prompts() {
    let mut s = session();
    // A point, then a point on the X axis.
    typed(&mut s, &["ucs", "10,10", "10,20"]);
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let f = ucs(&s);
    assert!(near(f.origin, Vec2::new(10.0, 10.0)) && (f.angle.to_degrees() - 90.0).abs() < 1e-9, "{f:?}");
    // Z, NAmed Save, World, NAmed Restore, Previous.
    typed(&mut s, &["ucs", "z", "-90"]);
    assert!(ucs(&s).angle.abs() < 1e-9);
    typed(&mut s, &["ucs", "na", "s", "Here"]);
    typed(&mut s, &["ucs", ""]);
    assert!(ucs(&s).is_world());
    typed(&mut s, &["ucs", "na", "r", "Here"]);
    assert_eq!(ucs(&s).name, "Here");
    typed(&mut s, &["ucs", "p"]);
    assert!(ucs(&s).is_world());
    // Object, picked.
    let l = s.execute("line", &json!({ "points": [[0, 0], [0, 5]] })).unwrap();
    let h = Handle::parse_hex(l["handle"].as_str().or_else(|| l["handles"][0].as_str()).unwrap()).unwrap();
    s.cmdline("ucs").unwrap();
    s.cmdline("ob").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![h])).unwrap();
    assert!((ucs(&s).angle.to_degrees() - 90.0).abs() < 1e-9 || (ucs(&s).angle.to_degrees() - 270.0).abs() < 1e-9);
    // 3D options say so and re-prompt.
    s.cmdline("ucs").unwrap();
    assert!(s.cmdline("x").is_err() || s.running.is_some());
    s.cancel();
}

#[test]
fn ucsicon_sets_ucsicon() {
    let mut s = session();
    assert_eq!(s.execute("ucsicon", &json!({ "option": "off" })).unwrap()["ucsicon"], 2);
    assert_eq!(s.execute("ucsicon", &json!({ "option": "on" })).unwrap()["ucsicon"], 3);
    assert_eq!(s.execute("ucsicon", &json!({ "option": "noorigin" })).unwrap()["ucsicon"], 1);
    assert!(s.execute("ucsicon", &json!({ "option": "maybe" })).is_err());
}
