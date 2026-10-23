//! A layout set up for the first time (AutoCAD's layout initialisation): a new drawing's Layout1
//! shows what was drawn by the time it is first opened, an empty layout read from a file gets a
//! viewport, a layout already set up is left alone, and a paper size change makes the sheet's
//! viewport fit the new sheet.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::{Bounds2, Vec2, Vec3};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Line, Space, Viewport};
use serde_json::{Value, json};

fn plot_hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn viewports(s: &Session, layout: &str) -> Vec<Viewport> {
    s.doc()
        .unwrap()
        .layout(layout)
        .unwrap()
        .entities
        .iter()
        .filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { Some(v.as_ref().clone()) } else { None })
        .filter(|v| v.id != 1)
        .collect()
}

/// The model area a viewport shows.
fn seen(v: &Viewport) -> Bounds2 {
    let h = v.view_height / 2.0;
    let w = h * v.width / v.height;
    Bounds2::new(v.view_center - Vec2::new(w, h), v.view_center + Vec2::new(w, h))
}

fn shows(v: &Viewport, b: Bounds2) -> bool {
    let s = seen(v).expand(1e-6);
    s.contains(b.min) && s.contains(b.max)
}

fn printable(s: &Session, layout: &str) -> Bounds2 {
    cadaddy_engine::render::sheet(s.doc().unwrap(), layout).unwrap().printable
}

/// A 40' × 30' outline (inches) in model space.
fn plan(s: &mut Session) -> Bounds2 {
    s.cmdline("rectang 0,0 480,360").unwrap();
    s.cmdline("line 120,0 120,360").unwrap();
    s.cmdline("").unwrap();
    Bounds2::new(Vec2::ZERO, Vec2::new(480.0, 360.0))
}

#[test]
fn a_new_drawings_layout1_shows_the_plan_drawn_before_it_was_opened() {
    let mut s = Session::empty();
    s.new_drawing_with_viewports(false);
    let before = viewports(&s, "Layout1");
    assert_eq!(before.len(), 1);
    // The placeholder shows the empty drawing's limits (12 × 9) only.
    let ext = plan(&mut s);
    assert!(!shows(&before[0], ext));
    s.execute("layout.set", &json!({ "name": "Layout1" })).unwrap();
    let vps = viewports(&s, "Layout1");
    assert_eq!(vps.len(), 1, "set up, not a second viewport");
    assert!(shows(&vps[0], ext), "zoomed to the plan: {:?}", seen(&vps[0]));
    let tight = seen(&vps[0]);
    assert!(tight.height() < ext.height() * 1.2 || tight.width() < ext.width() * 1.2, "fitted, not just big: {tight:?}");
    // Its rectangle stays the printable area.
    let r = printable(&s, "Layout1");
    assert!(vps[0].center.xy().near(r.center(), 1e-9) && (vps[0].width - r.width()).abs() < 1e-9);
    // Layout2 is set up when it is opened, on its own.
    assert!(!shows(&viewports(&s, "Layout2")[0], ext));
    s.execute("layout.set", &json!({ "name": "Layout2" })).unwrap();
    assert!(shows(&viewports(&s, "Layout2")[0], ext));
    assert!(!s.doc().unwrap().layout("Layout1").unwrap().pending_setup);
}

#[test]
fn a_layout_already_set_up_keeps_its_view() {
    let mut s = Session::empty();
    s.new_drawing_with_viewports(false);
    plan(&mut s);
    s.execute("layout.set", &json!({ "name": "Layout1" })).unwrap();
    s.execute("layout.set", &json!({ "name": "Model" })).unwrap();
    // More drawn far away: the layout, set up already, doesn't jump to it.
    s.cmdline("circle 5000,5000 10").unwrap();
    let v1 = viewports(&s, "Layout1");
    s.execute("layout.set", &json!({ "name": "Layout1" })).unwrap();
    assert_eq!(viewports(&s, "Layout1"), v1);
    // A drawing made without the template viewports (Session::new) keeps its layouts empty.
    let mut t = Session::new();
    plan(&mut t);
    t.execute("layout.set", &json!({ "name": "Layout1" })).unwrap();
    assert!(viewports(&t, "Layout1").is_empty());
}

#[test]
fn an_empty_layout_of_a_drawing_read_from_a_file_gets_a_viewport_on_its_extents() {
    let mut d = Drawing::new_imperial();
    let h = d.new_handle();
    d.space_mut(&Space::Model)
        .unwrap()
        .push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-200.0, 50.0, 0.0), b: Vec3::new(900.0, 700.0, 0.0) })));
    assert!(d.layouts.iter().all(|l| l.entities.is_empty()));
    let mut s = Session::empty();
    s.open_drawing(d, "Granary Lane.dwg", None);
    s.execute("layout.set", &json!({ "name": "Layout2" })).unwrap();
    let vps = viewports(&s, "Layout2");
    assert_eq!(vps.len(), 1);
    assert!(shows(&vps[0], Bounds2::new(Vec2::new(-200.0, 50.0), Vec2::new(900.0, 700.0))));
    // Erased and opened again: it stays empty (it was set up).
    let hs: Vec<String> = s.doc().unwrap().layout("Layout2").unwrap().entities.iter().map(|e| e.handle.hex()).collect();
    s.execute("erase", &json!({ "handles": hs })).unwrap();
    s.execute("layout.set", &json!({ "name": "Model" })).unwrap();
    s.execute("layout.set", &json!({ "name": "Layout2" })).unwrap();
    assert!(viewports(&s, "Layout2").is_empty());
}

#[test]
fn a_paper_size_change_makes_the_sheets_viewport_fit_the_new_sheet() {
    let mut s = Session::empty();
    s.new_drawing_with_viewports(false);
    let ext = plan(&mut s);
    s.execute("layout.set", &json!({ "name": "Layout1" })).unwrap();
    // A viewport of the user's own, smaller than the sheet, stays where it is.
    s.execute("mview", &json!({ "p1": [1, 1], "p2": [3, 2] })).unwrap();
    let old = printable(&s, "Layout1");
    s.execute("pagesetup", &json!({ "paper": "ANSI B", "landscape": true })).unwrap();
    let new = printable(&s, "Layout1");
    assert!(new.width() > old.width() + 1.0, "{old:?} → {new:?}");
    let vps = viewports(&s, "Layout1");
    assert_eq!(vps.len(), 2);
    let full = vps.iter().find(|v| v.width > 5.0).unwrap();
    assert!(full.center.xy().near(new.center(), 1e-9), "{:?} vs {new:?}", full.center);
    assert!((full.width - new.width()).abs() < 1e-9 && (full.height - new.height()).abs() < 1e-9);
    assert!(shows(full, ext));
    let small = vps.iter().find(|v| v.width < 5.0).unwrap();
    assert!(small.center.xy().near(Vec2::new(2.0, 1.5), 1e-9) && (small.width - 2.0).abs() < 1e-9);
    // Undo puts the paper and the viewport back together.
    s.undo().unwrap();
    let back = viewports(&s, "Layout1");
    let full = back.iter().find(|v| v.width > 5.0).unwrap();
    assert!((full.width - old.width()).abs() < 1e-9);
}

// ---------- PLOT and PUBLISH set up bare layouts (MUST 1) ----------

#[test]
fn plot_sets_up_a_new_drawings_layout_without_layout_set() {
    plot_hooks();
    let mut s = Session::empty();
    s.new_drawing_with_viewports(false);
    let ext = plan(&mut s);
    // The placeholder shows only the empty drawing's limits, not the plan.
    assert!(!shows(&viewports(&s, "Layout1")[0], ext));
    // PLOT without any layout.set: the layout is set up in memory before plotting.
    let r = s.execute("plot", &json!({ "layout": "Layout1" })).unwrap();
    assert!(r.get("data").is_some() || r.get("bytes").is_some(), "plot succeeded: {r}");
    // The viewport now shows the plan's extents.
    let vps = viewports(&s, "Layout1");
    assert_eq!(vps.len(), 1, "set up, not a second viewport");
    assert!(shows(&vps[0], ext), "viewport zoomed to the plan after plot: {:?}", seen(&vps[0]));
    // The printable area is the viewport's rectangle (same as the layout.set path).
    let r_area = printable(&s, "Layout1");
    assert!(vps[0].center.xy().near(r_area.center(), 1e-9));
    assert!((vps[0].width - r_area.width()).abs() < 1e-9);
}

#[test]
fn publish_sets_up_empty_layouts_of_a_drawing_opened_from_a_file() {
    plot_hooks();
    // A drawing with model content and empty layouts (read from a file via open_drawing).
    let mut d = Drawing::new_imperial();
    let h = d.new_handle();
    d.space_mut(&Space::Model)
        .unwrap()
        .push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(480.0, 360.0, 0.0) })));
    assert!(d.layouts.iter().all(|l| l.entities.is_empty()));
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan.dwg", None);
    // Without any layout.set, publish all layouts.
    let r = s.execute("publish", &json!({})).unwrap();
    assert!(r.get("sheets").is_some(), "publish succeeded: {r}");
    // Layout1 now has a viewport showing the model content.
    let vps = viewports(&s, "Layout1");
    assert_eq!(vps.len(), 1, "a viewport was created for Layout1");
    assert!(
        shows(&vps[0], Bounds2::new(Vec2::ZERO, Vec2::new(480.0, 360.0))),
        "viewport shows the drawing: {:?}",
        seen(&vps[0])
    );
}

// ---------- blank-sheet warnings (MUST 2) ----------

#[test]
fn plot_and_publish_warn_about_blank_sheets() {
    plot_hooks();
    // A drawing with nothing in model space: every layout plots blank.
    let mut s = Session::empty();
    s.new_drawing_with_viewports(false);
    let log_before = s.log.len();
    let r = s.execute("plot", &json!({ "layout": "Layout1" })).unwrap();
    let w = r["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 1, "one blank warning: {r}");
    assert!(w[0].as_str().unwrap().contains("nothing on it"), "{r}");
    assert!(
        s.log[log_before..].iter().any(|l| l.starts_with("Warning:") && l.contains("Layout1")),
        "Warning echoed: {:?}",
        &s.log[log_before..]
    );

    // Publish all layouts of the empty drawing: one warning per layout.
    let log_before = s.log.len();
    let r = s.execute("publish", &json!({})).unwrap();
    let w: Vec<&str> = r["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    // Layout1 and Layout2 are both blank.
    assert_eq!(w.len(), 2, "one warning per layout: {w:?}");
    assert!(
        s.log[log_before..].iter().filter(|l| l.starts_with("Warning:")).count() >= 2,
        "Warning lines echoed: {:?}",
        &s.log[log_before..]
    );

    // A drawing with content: no warnings.
    let mut s2 = Session::empty();
    s2.new_drawing_with_viewports(false);
    plan(&mut s2);
    let r = s2.execute("plot", &json!({ "layout": "Layout1" })).unwrap();
    assert_eq!(r["warnings"], json!([]), "no warnings for a plan: {r}");
    let r = s2.execute("publish", &json!({})).unwrap();
    assert_eq!(r["warnings"], json!([]), "no warnings for publish of a plan: {r}");
}
