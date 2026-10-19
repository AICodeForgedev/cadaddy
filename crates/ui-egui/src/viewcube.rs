//! The navigation overlays on the drawing area: the ViewCube (its faces and corners, Home,
//! compass ring and the WCS menu) and the UCS icon.
//!
//! Every overlay registers a widget that senses clicks *and* drags, so egui hands the pointer to
//! the overlay instead of the canvas underneath (a click on the ViewCube must never also pick or
//! start a selection window in the drawing). What the overlays do goes through commands:
//! `view.dir` (faces, corners, Home, dragging the ring), `ui.toggle.viewcube`,
//! `ui.toggle.ucsicon`.
//!
//! The cube is drawn in the current view direction, as AutoCAD's: in the plan view only its TOP
//! face shows. A face click looks at that face, a corner click gives the isometric view from that
//! corner, Home goes to the home view (SW isometric, fitted). Dragging the compass ring turns a 3D
//! view about the Z axis, and twists the plan view (`view.twist`).

use cadaddy_geom::Vec3;
use cadaddy_geom::view3::Projection;
use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::CadApp;
use crate::theme::Tokens;

/// Half the cube's edge on screen.
const HALF: f32 = 22.0;
/// How close to a corner a click picks the corner.
const CORNER_HIT: f32 = 7.0;

/// The cube's faces: outward normal, label and preset view.
const FACES: [(Vec3, &str, &str); 6] = [
    (Vec3::new(0.0, 0.0, 1.0), "TOP", "top"),
    (Vec3::new(0.0, 0.0, -1.0), "BOTTOM", "bottom"),
    (Vec3::new(0.0, -1.0, 0.0), "FRONT", "front"),
    (Vec3::new(0.0, 1.0, 0.0), "BACK", "back"),
    (Vec3::new(-1.0, 0.0, 0.0), "LEFT", "left"),
    (Vec3::new(1.0, 0.0, 0.0), "RIGHT", "right"),
];

/// Where the ViewCube's parts are on the canvas.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub center: Pos2,
    /// Radius of the compass ring's centre line.
    pub ring: f32,
    /// The TOP face in the plan view.
    pub face: Rect,
    pub home: Rect,
    /// The "WCS" coordinate-system menu under the cube.
    pub pill: Rect,
    /// Everything that belongs to the cube (the area kept away from the canvas).
    pub bounds: Rect,
    /// Half a cube face on screen: smaller on small canvases.
    half: f32,
}

/// The view's screen axes in world terms: the plan view's when there is no projection.
fn axes(proj: Option<Projection>) -> (Vec3, Vec3, Vec3) {
    match proj {
        Some(p) => (p.right, p.up, p.dir),
        None => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Z),
    }
}

impl Layout {
    pub fn new(canvas: Rect) -> Self {
        // Full size on canvases 700 px and up, down to 60% on small windows (at 1024×640 the
        // full-size cube covered about a quarter of the canvas width).
        let k = (canvas.width().min(canvas.height()) / 700.0).clamp(0.6, 1.0);
        let center = pos2(canvas.right() - 92.0 * k, canvas.top() + 82.0 * k);
        let ring = 58.0 * k;
        let half = HALF * k;
        Layout {
            center,
            ring,
            face: Rect::from_center_size(center, vec2(2.0 * half, 2.0 * half)),
            home: Rect::from_center_size(center + vec2(-ring + 2.0 * k, -ring + 2.0 * k), vec2(20.0 * k, 20.0 * k)),
            pill: Rect::from_center_size(center + vec2(0.0, ring + 26.0 * k), vec2(56.0, 16.0)),
            bounds: Rect::from_center_size(center, vec2(2.0 * (ring + 10.0), 2.0 * (ring + 10.0))),
            half,
        }
    }

    /// A point of the unit cube on screen.
    fn at(&self, proj: Option<Projection>, p: Vec3) -> Pos2 {
        let (r, u, _) = axes(proj);
        self.center + vec2(p.dot(r) as f32, -(p.dot(u) as f32)) * self.half
    }

    /// The faces facing the viewer: (normal, label, preset, screen corners, how squarely it
    /// faces the viewer 0..1), the most oblique first.
    fn visible_faces(&self, proj: Option<Projection>) -> Vec<(Vec3, &'static str, &'static str, [Pos2; 4], f64)> {
        let (_, _, dir) = axes(proj);
        let mut out: Vec<_> = FACES
            .iter()
            .filter_map(|(n, label, preset)| {
                let facing = n.dot(dir);
                (facing > 1e-6).then(|| {
                    // Two edge directions in the face.
                    let a = if n.z.abs() > 0.5 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::Z };
                    let b = n.cross(a);
                    let q = [*n + a + b, *n + a - b, *n - a - b, *n - a + b].map(|c| self.at(proj, c));
                    (*n, *label, *preset, q, facing)
                })
            })
            .collect();
        out.sort_by(|x, y| x.4.total_cmp(&y.4));
        out
    }

    /// The cube's corners the viewer sees (on at least one visible face), as view directions.
    fn visible_corners(&self, proj: Option<Projection>) -> Vec<(Vec3, Pos2)> {
        let (_, _, dir) = axes(proj);
        let mut out = Vec::new();
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let c = Vec3::new(sx, sy, sz);
                    if sx * dir.x > 1e-6 || sy * dir.y > 1e-6 || sz * dir.z > 1e-6 {
                        out.push((c, self.at(proj, c)));
                    }
                }
            }
        }
        out
    }

    /// The part of the cube at `p` in the view `proj` (not the pill, which is its own widget).
    pub fn part_at(&self, p: Pos2, proj: Option<Projection>) -> Option<Part> {
        if self.home.contains(p) {
            return Some(Part::Home);
        }
        if let Some((c, _)) = self.visible_corners(proj).into_iter().find(|(_, at)| at.distance(p) <= CORNER_HIT) {
            return Some(Part::Corner(c));
        }
        if let Some((_, _, preset, _, _)) = self.visible_faces(proj).into_iter().rev().find(|(_, _, _, q, _)| point_in_quad(p, q)) {
            return Some(Part::Face(preset));
        }
        let d = p.distance(self.center);
        ((self.ring - 10.0..=self.ring + 10.0).contains(&d)).then_some(Part::Ring)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Part {
    /// A face: its preset view.
    Face(&'static str),
    /// A corner: the view direction from it.
    Corner(Vec3),
    Home,
    Ring,
}

impl Part {
    fn tip(self, proj: Option<Projection>) -> String {
        match self {
            Part::Face(preset) => format!("{}: look at this face (zoom extents)", preset[..1].to_uppercase() + &preset[1..]),
            Part::Corner(c) => match cadaddy_geom::view3::preset_label(c) {
                Some(l) => format!("{l} view"),
                None => "View from this corner".into(),
            },
            Part::Home => "Home: SW isometric view of the whole drawing".into(),
            Part::Ring if proj.is_some() => "Drag to turn the view about the Z axis".into(),
            Part::Ring => "Compass: view rotation isn't available in the 2D plan view yet".into(),
        }
    }
}

/// Paint the ViewCube in the current view and handle the pointer. The canvas paints the WCS
/// pill itself.
pub fn show(app: &mut CadApp, ui: &mut egui::Ui, l: &Layout) {
    let proj = app.session.state().ok().and_then(|st| st.projection());
    let resp = ui.interact(l.bounds, ui.id().with("viewcube"), Sense::click_and_drag());
    let part = resp.hover_pos().and_then(|p| l.part_at(p, proj));
    let p = ui.painter_at(l.bounds.union(l.pill));
    let ring_col = if part == Some(Part::Ring) { Color32::from_rgb(0x5c, 0x67, 0x7c) } else { Color32::from_rgb(0x48, 0x50, 0x5c) };
    p.circle_stroke(l.center, l.ring, Stroke::new(9.0, ring_col));
    p.circle_stroke(l.center, l.ring + 4.5, Stroke::new(1.0, Color32::from_rgb(0x5c, 0x65, 0x72)));
    // Compass letters where the world directions point on screen (hidden when one points at the
    // viewer, as north does in a front view).
    let f = egui::FontId::proportional(17.0);
    let lc = Color32::from_rgb(0xc8, 0xcc, 0xd2);
    let (r, u, _) = axes(proj);
    for (w, letter) in
        [(Vec3::new(0.0, 1.0, 0.0), "N"), (Vec3::new(0.0, -1.0, 0.0), "S"), (Vec3::new(1.0, 0.0, 0.0), "E"), (Vec3::new(-1.0, 0.0, 0.0), "W")]
    {
        let d = vec2(w.dot(r) as f32, -(w.dot(u) as f32));
        if d.length() > 0.3 {
            p.text(l.center + d.normalized() * (l.ring + 1.0), egui::Align2::CENTER_CENTER, crate::i18n::t(letter), f.clone(), lc);
        }
    }
    // Faces, the most oblique first, shaded by how squarely they face the viewer.
    let edge = Stroke::new(1.0, Color32::from_rgb(0x6c, 0x70, 0x76));
    for (_, label, preset, q, facing) in l.visible_faces(proj) {
        let hot = part == Some(Part::Face(preset));
        let base = if hot { 0xb8 } else { 0x7a + (0x24 as f64 * facing) as u8 };
        p.add(egui::Shape::convex_polygon(q.to_vec(), Color32::from_rgb(base, base + 4, base + 10), edge));
        if facing > 0.3 {
            let c = pos2(q.iter().map(|v| v.x).sum::<f32>() / 4.0, q.iter().map(|v| v.y).sum::<f32>() / 4.0);
            let size = if label.len() > 4 { 9.5 } else { 12.0 };
            p.text(c, egui::Align2::CENTER_CENTER, crate::i18n::t(label), egui::FontId::proportional(size), Color32::from_rgb(0x40, 0x44, 0x4a));
        }
    }
    if let Some(Part::Corner(c)) = part {
        p.circle_filled(l.at(proj, c), 4.5, Color32::from_rgb(0x4d, 0x9c, 0xf5));
    }
    paint_home(&p, l.home, part == Some(Part::Home));

    let resp = match part {
        Some(part) => resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(part.tip(proj)),
        None => resp,
    };
    let pressed_on = resp.interact_pointer_pos().and_then(|o| l.part_at(o, proj));
    if resp.clicked() {
        match pressed_on {
            Some(Part::Face(preset)) => {
                let _ = app.run("view.dir", json!({ "preset": preset }));
            }
            Some(Part::Corner(c)) => {
                let _ = app.run("view.dir", json!({ "dir": [c.x, c.y, c.z], "fit": true }));
            }
            Some(Part::Home) => {
                let _ = app.run("view.dir", json!({ "preset": "swiso" }));
            }
            _ => {}
        }
    }
    if pressed_on == Some(Part::Ring)
        && resp.dragged()
        && let Some(now) = resp.interact_pointer_pos()
    {
        // Turn about Z by the angle the pointer swept around the cube's centre (screen y is down:
        // a counterclockwise sweep on screen is a negative angle here).
        let before = now - resp.drag_delta();
        let ang = |q: Pos2| f64::from((q.y - l.center.y).atan2(q.x - l.center.x));
        let swept = ang(now) - ang(before);
        match proj.filter(|p| !cadaddy_geom::view3::is_plan(p.dir)) {
            Some(pr) => {
                let (az, el) = Projection::angles(pr.dir);
                let d = Projection::dir_from_angles(az + swept, el);
                let _ = app.run("view.dir", json!({ "dir": [d.x, d.y, d.z] }));
            }
            // The plan view twists: the compass follows the pointer round.
            None if swept.is_finite() && swept != 0.0 => {
                let _ = app.run("view.twist", json!({ "by": -swept.to_degrees() }));
            }
            None => {}
        }
    }
    let mut action = None;
    resp.context_menu(|ui| {
        if ui.button("Home").clicked() {
            action = Some(("view.dir", json!({"preset": "swiso"})));
            ui.close();
        }
        if ui.button("Plan View").clicked() {
            action = Some(("plan", json!({})));
            ui.close();
        }
        if ui.button("Hide ViewCube").clicked() {
            action = Some(("ui.toggle.viewcube", json!({"on": false})));
            ui.close();
        }
    });
    if let Some((id, params)) = action {
        let _ = app.run(id, params);
    }
    // The WCS pill (painted by the canvas) opens the coordinate-system menu.
    let pill = ui.interact(l.pill, ui.id().with("viewcube_wcs"), Sense::click_and_drag()).on_hover_cursor(CursorIcon::PointingHand);
    let pill = pill.on_hover_text("Coordinate system");
    ucs_menu(app, egui::Popup::menu(&pill));
}

/// Whether `p` is inside the convex quadrilateral `q` (either winding).
fn point_in_quad(p: Pos2, q: &[Pos2; 4]) -> bool {
    let side = |a: Pos2, b: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let s = [side(q[0], q[1]), side(q[1], q[2]), side(q[2], q[3]), side(q[3], q[0])];
    s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0)
}

/// A small house, drawn in code.
fn paint_home(p: &egui::Painter, r: Rect, hot: bool) {
    let t = Tokens::get();
    if hot {
        p.rect_filled(r, 3.0, t.control_hover);
    }
    let st = Stroke::new(1.3, if hot { t.text } else { t.text_dim });
    let c = r.center();
    let roof = [c + vec2(-6.5, 0.5), c + vec2(0.0, -6.0), c + vec2(6.5, 0.5)];
    p.line(roof.to_vec(), st);
    p.line(vec![c + vec2(-4.5, -1.0), c + vec2(-4.5, 6.0), c + vec2(4.5, 6.0), c + vec2(4.5, -1.0)], st);
    p.line_segment([c + vec2(-1.2, 6.0), c + vec2(-1.2, 2.5)], st);
    p.line_segment([c + vec2(1.2, 6.0), c + vec2(1.2, 2.5)], st);
    p.line_segment([c + vec2(-1.2, 2.5), c + vec2(1.2, 2.5)], st);
}

/// The UCS icon's arms (as drawn by the canvas at the lower-left corner): the X arm and the Y arm.
pub fn ucs_arms(canvas: Rect) -> [Rect; 2] {
    let o = pos2(canvas.left() + 36.0, canvas.bottom() - 34.0);
    [Rect::from_min_max(o + vec2(-8.0, -8.0), o + vec2(80.0, 8.0)), Rect::from_min_max(o + vec2(-8.0, -82.0), o + vec2(8.0, 8.0))]
}

/// Make the UCS icon clickable: hovering highlights it, a click (either button) opens the
/// coordinate-system menu.
pub fn ucs_icon(app: &mut CadApp, ui: &mut egui::Ui, canvas: Rect) {
    let t = Tokens::get();
    let [x, y] = ucs_arms(canvas);
    let rx = ui.interact(x, ui.id().with("ucs_x"), Sense::click_and_drag());
    let ry = ui.interact(y, ui.id().with("ucs_y"), Sense::click_and_drag());
    let resp = (rx | ry).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("UCS icon: World Coordinate System");
    if resp.hovered() {
        let o = pos2(x.left() + 8.0, x.center().y);
        let st = Stroke::new(2.0, t.hover);
        let p = ui.painter_at(canvas);
        p.line_segment([o, o + vec2(60.0, 0.0)], st);
        p.line_segment([o, o + vec2(0.0, -60.0)], st);
    }
    // Opened with `Bool(true)`: a pointer-anchored popup records its position only then.
    let open = (resp.clicked() || resp.secondary_clicked()).then_some(egui::SetOpenCommand::Bool(true));
    ucs_menu(app, egui::Popup::menu(&resp).open_memory(open).at_pointer_fixed());
}

/// The coordinate-system menu: World, the named UCSs (the current one checked; an unnamed
/// current UCS listed as Unnamed), New UCS from the command line, and the icon switch.
fn ucs_menu(app: &mut CadApp, popup: egui::Popup<'_>) {
    let mut hide = false;
    let mut run: Option<(&'static str, serde_json::Value)> = None;
    let cur = app.session.ucs();
    let names: Vec<String> = app.session.doc().map(|d| d.ucss.iter().map(|u| u.name.clone()).take(50).collect()).unwrap_or_default();
    popup.show(|ui| {
        if ui.radio(cur.is_world() && cur.name.is_empty(), crate::tl!("World (WCS)")).clicked() {
            run = Some(("ucs", json!({"option": "world"})));
            ui.close();
        }
        for n in &names {
            if ui.radio(cur.name.eq_ignore_ascii_case(n), n.as_str()).clicked() {
                run = Some(("ucs", json!({"option": "restore", "name": n})));
                ui.close();
            }
        }
        if !cur.is_world() && cur.name.is_empty() {
            let _ = ui.radio(true, crate::tl!("Unnamed"));
        }
        ui.separator();
        if ui.button(crate::tl!("New UCS")).clicked() {
            app.start("ucs");
            ui.close();
        }
        if ui.button(crate::tl!("Hide UCS Icon")).clicked() {
            hide = true;
            ui.close();
        }
    });
    if let Some((id, p)) = run {
        let _ = app.run(id, p);
    }
    if hide {
        let _ = app.run("ui.toggle.ucsicon", json!({"on": false}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;

    /// The app with the sample drawing, laid out once in a 1400×900 window.
    fn app_and_ctx() -> (CadApp, egui::Context) {
        let mut app = CadApp::new(Session::empty(), crate::Services::default());
        app.ui.in_window_menu = true;
        app.start("ui.sample");
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        (app, ctx)
    }

    fn frame(app: &mut CadApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }

    /// A real pointer click (move, press, release in separate frames, like the control channel).
    pub(crate) fn click(app: &mut CadApp, ctx: &egui::Context, at: Pos2, button: egui::PointerButton) {
        let m = egui::Modifiers::default();
        frame(app, ctx, vec![egui::Event::PointerMoved(at)]);
        frame(app, ctx, vec![egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]);
        frame(app, ctx, vec![egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]);
        frame(app, ctx, Vec::new());
    }

    /// A menu popup was shown in the last frame.
    fn menu_shown(ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.order == egui::Order::Foreground))
    }

    fn canvas(app: &CadApp) -> Rect {
        app.canvas.rect.unwrap_or(Rect::NOTHING)
    }

    fn view(app: &CadApp) -> (f64, f64, f64) {
        let v = app.session.state().map(|s| s.view()).unwrap_or_default();
        (v.center.x, v.center.y, v.height)
    }

    #[test]
    fn parts_are_found_where_they_are_drawn() {
        let l = Layout::new(Rect::from_min_size(pos2(200.0, 80.0), vec2(1000.0, 800.0)));
        assert_eq!(l.part_at(l.center, None), Some(Part::Face("top")));
        assert_eq!(l.part_at(l.home.center(), None), Some(Part::Home));
        assert_eq!(l.part_at(l.center + vec2(0.0, -l.ring), None), Some(Part::Ring), "N");
        assert_eq!(l.part_at(l.center + vec2(l.ring + 3.0, 0.0), None), Some(Part::Ring), "E");
        assert_eq!(l.part_at(l.center + vec2(30.0, 30.0), None), None, "between the face and the ring");
        // The top face's corners give the isometric views (screen y runs down: the lower-left
        // corner is south-west).
        assert_eq!(l.part_at(l.center + vec2(-HALF, HALF), None), Some(Part::Corner(Vec3::new(-1.0, -1.0, 1.0))));
        // Seen from the front, the middle of the cube is the FRONT face.
        let front = Projection::new(Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(l.part_at(l.center, front), Some(Part::Face("front")));
        assert!(l.bounds.contains_rect(l.home) && l.bounds.contains_rect(l.face));
    }

    #[test]
    fn viewcube_clicks_reach_the_cube_not_the_drawing() {
        let (mut app, ctx) = app_and_ctx();
        let l = Layout::new(canvas(&app));
        let fitted = view(&app);
        // Zoomed in, the Top face fits the drawing again (zoom extents) and nothing is picked.
        let _ = app.run("zoom.in", json!({}));
        assert_ne!(view(&app), fitted);
        click(&mut app, &ctx, l.center, egui::PointerButton::Primary);
        assert_eq!(view(&app), fitted, "Top face zooms to extents");
        assert!(app.session.pending_window.is_none(), "the click didn't start a selection window");
        assert!(app.session.selection().is_empty());
        // A click on the ring doesn't touch the drawing or the view.
        click(&mut app, &ctx, l.center + vec2(0.0, -l.ring), egui::PointerButton::Primary);
        assert!(app.session.pending_window.is_none());
        assert_eq!(view(&app), fitted);
        // A click in the drawing still reaches the drawing.
        let empty = canvas(&app).center() + vec2(0.0, -300.0);
        click(&mut app, &ctx, empty, egui::PointerButton::Primary);
        assert!(app.session.pending_window.is_some(), "an empty-space click starts a selection window");
    }

    #[test]
    fn home_and_corners_turn_the_view_and_top_returns_to_plan() {
        let (mut app, ctx) = app_and_ctx();
        let l = Layout::new(canvas(&app));
        let dir = |app: &CadApp| app.session.state().map(|s| s.view_dir).unwrap_or(Vec3::Z);
        click(&mut app, &ctx, l.home.center(), egui::PointerButton::Primary);
        assert_eq!(cadaddy_geom::view3::preset_label(dir(&app)), Some("SW Isometric"), "Home");
        // In the SW isometric view the cube shows TOP, FRONT and LEFT; the middle of the screen
        // cube is their shared corner, which is the SW isometric view again.
        let proj = app.session.state().ok().and_then(|s| s.projection());
        assert_eq!(l.part_at(l.center, proj), Some(Part::Corner(Vec3::new(-1.0, -1.0, 1.0))));
        // A point inside the TOP face goes back to the plan view.
        let top = l.visible_faces(proj).into_iter().find(|f| f.2 == "top").map(|f| f.3);
        let inside = top.map(|q| pos2((q[0].x + q[2].x) / 2.0, (q[0].y + q[2].y) / 2.0));
        if let Some(at) = inside {
            click(&mut app, &ctx, at, egui::PointerButton::Primary);
        }
        assert_eq!(dir(&app), Vec3::Z, "TOP face returns to the plan view");
    }

    #[test]
    fn ucs_icon_and_wcs_pill_open_the_coordinate_system_menu() {
        let (mut app, ctx) = app_and_ctx();
        let [x, _] = ucs_arms(canvas(&app));
        click(&mut app, &ctx, x.center(), egui::PointerButton::Primary);
        assert!(menu_shown(&ctx), "UCS icon menu");
        assert!(app.session.pending_window.is_none(), "the click stayed on the icon");
        let l = Layout::new(canvas(&app));
        click(&mut app, &ctx, l.pill.center(), egui::PointerButton::Primary);
        assert!(menu_shown(&ctx), "WCS menu");
        assert!(app.session.pending_window.is_none());
    }
}
