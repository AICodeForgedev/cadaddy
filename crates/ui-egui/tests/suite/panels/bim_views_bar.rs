//! The BIM view's control (Q6419): always in the 3D view's corner, one of Systems (Q6617),
//! Architectural, MEP, Floor and Room active, Full MEP in the MEP view, the switches each view offers, the
//! breadcrumb back up; Esc goes up a view. A made-up building (`sample::bim_building`).

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn app() -> (CadApp, egui_wgpu::RenderState) {
    let mut s = Session::empty();
    s.open_drawing(cadaddy_engine::sample::bim_building(3, 6), "Sample Building", None);
    let mut app = CadApp::new(s, Services::default());
    let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::default());
    app.set_wgpu(&rs, None);
    app.session.execute("bim.look", &json!({"on": true})).unwrap();
    app.session.execute("view.dir", &json!({"dir": [-1, -1.5, 1], "fit": true})).unwrap();
    (app, rs)
}

fn harness(app: CadApp, rs: egui_wgpu::RenderState) -> Harness<'static, CadApp> {
    Harness::builder().with_size(vec2(1200.0, 800.0)).renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    )
}

/// Frames until the model is built and drawn.
fn settle(h: &mut Harness<'static, CadApp>) {
    for _ in 0..600 {
        h.run_steps(1);
        let st = h.state().canvas.shade.stats;
        if st.building.is_none() && st.tris_total > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(3);
}

/// The control labelled `label` (the first one: the view's bar is drawn over the canvas).
fn click(h: &Harness<'static, CadApp>, label: &str) {
    h.query_all_by_label(label).next().unwrap_or_else(|| panic!("no {label}")).click();
}

fn view(h: &mut Harness<'static, CadApp>) -> serde_json::Value {
    h.state_mut().session.execute("bim.view", &json!({})).unwrap()
}

/// Where a screenshot goes when `CADADDY_VIEWS_SHOTS` names a folder (to look at by hand).
fn shot(h: &mut Harness<'static, CadApp>, name: &str) {
    if let Ok(dir) = std::env::var("CADADDY_VIEWS_SHOTS") {
        let img = h.render().expect("a frame");
        let _ = std::fs::create_dir_all(&dir);
        img.save(std::path::Path::new(&dir).join(format!("{name}.png"))).expect("save");
    }
}

#[test]
fn the_view_bar_switches_views_and_esc_goes_up() {
    let (app, rs) = app();
    let mut h = harness(app, rs);
    settle(&mut h);
    let v = view(&mut h);
    assert_eq!(v["view"], "systems", "a model with MEP opens in the Systems view");
    for k in ["Branches", "Interior Walls", "Site", "Presentation"] {
        assert!(h.query_all_by_label(k).next().is_some(), "{k} is offered in the Systems view");
    }
    assert!(h.query_all_by_label("Full MEP").next().is_none());
    // The legend: each system drawn, under the bar.
    let first = v["legend"][0]["label"].as_str().expect("a legend").to_string();
    assert!(h.query_all_by_label(&first).next().is_some(), "{first} in the legend");
    shot(&mut h, "systems");
    click(&h, "MEP");
    settle(&mut h);
    assert_eq!(view(&mut h)["view"], "mep");
    assert!(h.query_all_by_label("Full MEP").next().is_some(), "Full MEP is offered in the MEP view");
    assert!(h.query_all_by_label("Furniture").next().is_none(), "furniture only in the Room view");
    shot(&mut h, "mep");
    click(&h, "Full MEP");
    settle(&mut h);
    assert_eq!(view(&mut h)["full"], true);
    shot(&mut h, "mep-full");
    click(&h, "Architectural");
    settle(&mut h);
    assert_eq!(view(&mut h)["view"], "architectural");
    assert!(h.query_all_by_label("Full MEP").next().is_none());
    shot(&mut h, "architectural");
    click(&h, "Floor");
    settle(&mut h);
    let v = view(&mut h);
    assert_eq!((v["view"].as_str(), v["level"].as_str()), (Some("floor"), Some("L1")), "{v}");
    assert!(h.query_all_by_label("Cabinets").next().is_some());
    shot(&mut h, "floor");
    h.state_mut().session.execute("bim.view", &json!({"room": "102"})).unwrap();
    settle(&mut h);
    assert!(h.query_all_by_label("Furniture").next().is_some(), "the Room view offers furniture");
    click(&h, "Furniture");
    settle(&mut h);
    assert_eq!(view(&mut h)["switches"]["furniture"], true);
    shot(&mut h, "room");
    // Esc with nothing selected: up to the Floor view, then to the whole building.
    h.state_mut().session.set_selection(Vec::new());
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    assert_eq!(view(&mut h)["view"], "floor");
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    let v = view(&mut h);
    assert_eq!((v["view"].as_str(), v["level"].as_str()), (Some("architectural"), None), "back to the whole-building view it came from: {v}");
}

/// The Systems view in the window (Q6617): on white in the presentation look, the systems
/// without dark edge lines, the same view model.render draws.
#[test]
fn the_systems_view_draws_on_white_without_dark_edges() {
    let (app, rs) = app();
    let mut h = harness(app, rs);
    settle(&mut h);
    let r = h.state_mut().session.execute("bim.look", &json!({"presentation": true})).unwrap();
    assert_eq!(r["presentation"], true);
    settle(&mut h);
    shot(&mut h, "systems-presentation");
    let img = h.render().expect("a frame");
    let canvas = h.state().canvas.rect.expect("the canvas");
    let ppp = img.width() as f32 / 1200.0;
    let px = |x: f32, y: f32| img.get_pixel((x * ppp) as u32, (y * ppp) as u32).0;
    // The ground is white: a point near the canvas's lower right corner, away from the building.
    let c = px(canvas.right() - 120.0, canvas.bottom() - 60.0);
    assert!(c[0] > 245 && c[1] > 245 && c[2] > 245, "white ground: {c:?}");
    // The middle of the view (the building): coloured systems, no near-black edge lines.
    let (mut dark, mut coloured) = (0, 0);
    let (x0, x1, y0, y1) = (
        canvas.left() + canvas.width() * 0.25,
        canvas.right() - canvas.width() * 0.25,
        canvas.top() + canvas.height() * 0.3,
        canvas.bottom() - canvas.height() * 0.2,
    );
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let p = px(x, y);
            if p[0] < 50 && p[1] < 50 && p[2] < 50 {
                dark += 1;
            }
            let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            if hi - lo > 80 {
                coloured += 1;
            }
            x += 1.0;
        }
        y += 1.0;
    }
    assert!(coloured > 200, "the systems in their colours: {coloured} pixels");
    assert!(dark * 20 < coloured, "{dark} near-black pixels beside {coloured} coloured");
}

/// `ui.set {dialog: null}` closes the Drawing Recovery window as its Later does (the files
/// stay): an agent can take a clean screenshot after a crash left autosave files (Q6617).
#[test]
fn ui_set_dialog_null_closes_drawing_recovery() {
    let mut app = CadApp::new(Session::empty(), Services::default());
    app.recovery.items.push(cadaddy_engine::autosave::Recoverable {
        path: "Sample Plan.dxf_1-a_3.sv$".into(),
        drawing: "Sample Plan.dxf".into(),
        modified: 0,
        bytes: 10,
    });
    let ctx = egui::Context::default();
    assert_eq!(cadaddy_ui_egui::control::inspect(&app, &ctx)["recovery"], 1);
    let (req, _) = cadaddy_ui_egui::ControlRequest::new("ui.set", json!({"dialog": null}));
    let _ = cadaddy_ui_egui::control::handle(&mut app, &ctx, &req);
    assert!(app.recovery.items.is_empty(), "closed");
    assert_eq!(cadaddy_ui_egui::control::inspect(&app, &ctx)["recovery"], 0);
}
