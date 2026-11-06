//! Live measurement labels on the drawing: DIST, AREA and MEASUREGEOM label what they measure
//! beside the geometry while you pick, at the same size on screen at any zoom, and the labels go
//! when the command ends (MEASURELABELS 2 keeps them as multiline text). Set CADADDY_LIVE_SHOTS to a
//! folder to save the screenshots.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Line, Space};
use cadaddy_engine::geom::Vec2;
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// A made-up floor plan: a 240 x 144 room with an inner wall.
fn harness() -> Harness<'static, CadApp> {
    let mut d = Drawing::new_imperial();
    let room = [(0.0, 0.0), (240.0, 0.0), (240.0, 144.0), (0.0, 144.0)];
    for i in 0..room.len() {
        let (a, b) = (room[i], room[(i + 1) % room.len()]);
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec2::new(a.0, a.1).to3(0.0), b: Vec2::new(b.0, b.1).to3(0.0) })).unwrap();
    }
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec2::new(150.0, 0.0).to3(0.0), b: Vec2::new(150.0, 90.0).to3(0.0) }))
        .unwrap();
    let mut session = Session::empty();
    session.open_drawing(d, "Sample Floor Plan", None);
    session.execute("view.set", &json!({"center": [120, 50], "height": 240})).unwrap();
    session.execute("setvar", &json!({"name": "LUNITS", "value": 4})).unwrap();
    // Exact picks: no snaps pulling the cursor.
    session.settings.osmode = 0;
    let mut app = CadApp::new(session, Services::default());
    app.canvas.zoom_pending = false;
    let mut h = Harness::builder().with_size(vec2(1280.0, 800.0)).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn screen(h: &Harness<'_, CadApp>, p: Vec2) -> Pos2 {
    h.state().canvas.xf.unwrap().w2s(p)
}

fn hover(h: &mut Harness<'_, CadApp>, p: Vec2) {
    let pos = screen(h, p);
    h.event(Event::PointerMoved(pos));
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, CadApp>, p: Vec2) {
    hover(h, p);
    let pos = screen(h, p);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(2);
    }
}

fn typed(h: &mut Harness<'_, CadApp>, line: &str) {
    h.state_mut().session.cmdline(line).unwrap();
    h.run_steps(3);
}

fn render(h: &mut Harness<'_, CadApp>, name: &str) -> image::RgbaImage {
    let img = h.render().unwrap();
    if let Some(dir) = std::env::var_os("CADADDY_LIVE_SHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(name)).unwrap();
    }
    img
}

/// Pixels that differ between `a` and `b` within `r` screen points of `at`.
fn changed_near(a: &image::RgbaImage, b: &image::RgbaImage, at: Pos2, r: f32, ppp: f32) -> usize {
    let (cx, cy, r) = ((at.x * ppp) as i64, (at.y * ppp) as i64, (r * ppp) as i64);
    let mut n = 0;
    for y in (cy - r).max(0)..(cy + r).min(i64::from(a.height())) {
        for x in (cx - r).max(0)..(cx + r).min(i64::from(a.width())) {
            if a.get_pixel(x as u32, y as u32) != b.get_pixel(x as u32, y as u32) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn dist_area_and_angle_label_the_drawing_while_picking() {
    let mut h = harness();
    let ppp = h.ctx.pixels_per_point();

    // DIST: the distance at the middle of the rubber band, in feet and inches.
    typed(&mut h, "dist");
    click(&mut h, Vec2::new(0.0, 0.0));
    hover(&mut h, Vec2::new(150.0, 0.0));
    let c = h.state().canvas.cursor.unwrap();
    let labels = h.state().session.live_labels(c);
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].lines[0], ("Distance", "12'-6\"".to_string()));
    let at = screen(&h, labels[0].at);
    let with = render(&mut h, "dist-live.png");
    // The label is drawn there: switching MEASURELABELS off changes those pixels.
    h.state_mut().session.settings.measurelabels = 0;
    h.state_mut().session.cancel();
    typed(&mut h, "dist");
    click(&mut h, Vec2::new(0.0, 0.0));
    hover(&mut h, Vec2::new(150.0, 0.0));
    let without = render(&mut h, "dist-labels-off.png");
    assert!(changed_near(&with, &without, at, 30.0, ppp) > 100, "a label at {at:?}");
    h.state_mut().session.settings.measurelabels = 1;

    // The same size on screen zoomed out ten times: still readable.
    h.state_mut().session.execute("view.set", &json!({"center": [120, 50], "height": 2400})).unwrap();
    h.run_steps(3);
    hover(&mut h, Vec2::new(150.0, 0.0));
    let c = h.state().canvas.cursor.unwrap();
    assert_eq!(h.state().session.live_labels(c)[0].lines[0].1, "12'-6\"");
    render(&mut h, "dist-zoomed-out.png");
    h.state_mut().session.execute("view.set", &json!({"center": [120, 50], "height": 240})).unwrap();
    h.run_steps(3);
    click(&mut h, Vec2::new(150.0, 0.0));
    assert!(h.state().session.running.is_none(), "{} {:?}", h.state().session.prompt_text(), h.state().session.log);
    let gone = render(&mut h, "dist-done.png");
    assert!(changed_near(&with, &gone, at, 30.0, ppp) > 100, "the label went with the command");

    // AREA: the running area and perimeter inside the boundary.
    typed(&mut h, "area");
    for p in [(0.0, 0.0), (150.0, 0.0), (150.0, 90.0)] {
        click(&mut h, Vec2::new(p.0, p.1));
    }
    hover(&mut h, Vec2::new(0.0, 90.0));
    let c = h.state().canvas.cursor.unwrap();
    let labels = h.state().session.live_labels(c);
    assert_eq!(labels[0].lines[0], ("Area", "13500.0000 square in. (93.7500 square ft.)".to_string()));
    render(&mut h, "area-live.png");
    h.state_mut().session.cancel();

    // MEASUREGEOM Angle: the angle inside the corner.
    typed(&mut h, "measuregeom");
    typed(&mut h, "a");
    click(&mut h, Vec2::new(150.0, 0.0));
    click(&mut h, Vec2::new(150.0, 90.0));
    hover(&mut h, Vec2::new(240.0, 90.0));
    let c = h.state().canvas.cursor.unwrap();
    assert_eq!(h.state().session.live_labels(c)[0].text(), "Angle = 45°");
    render(&mut h, "angle-live.png");
    h.state_mut().session.cancel();

    // MEASUREGEOM Quick: the distances to the walls around the cursor.
    typed(&mut h, "measuregeom");
    typed(&mut h, "q");
    hover(&mut h, Vec2::new(60.0, 50.0));
    let c = h.state().canvas.cursor.unwrap();
    assert!(h.state().session.live_labels(c).len() >= 4);
    render(&mut h, "quick-live.png");
    h.state_mut().session.cancel();
}

#[test]
fn measurelabels_two_keeps_the_labels_as_text() {
    let mut h = harness();
    h.state_mut().session.execute("setvar", &json!({"name": "MEASURELABELS", "value": 2})).unwrap();
    typed(&mut h, "dist");
    click(&mut h, Vec2::new(0.0, 0.0));
    click(&mut h, Vec2::new(240.0, 144.0));
    assert!(h.state().session.running.is_none());
    let d = h.state().session.doc().unwrap();
    let kept: Vec<_> = d.model.iter().filter(|e| matches!(e.kind, EntityKind::MText(_))).collect();
    assert_eq!(kept.len(), 1);
    render(&mut h, "dist-kept.png");
}
