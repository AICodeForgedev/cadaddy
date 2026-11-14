//! A lost graphics device (a driver reset, or the GPU out of memory) is not a crash: the frame
//! after it draws nothing on the dead device (egui's renderer panicked writing its buffers to it),
//! the drawings stay open, and the window made again on a new device draws them as before.

use cadaddy_doc::{Common, Drawing, EntityKind, Line, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_wgpu::wgpu::{self, DeviceLostReason};

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    for i in 0..20 {
        let y = f64::from(i) * 100.0;
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(2000.0, y, 0.0) })).unwrap();
    }
    d
}

fn harness(app: CadApp, rs: egui_wgpu::RenderState) -> Harness<'static, CadApp> {
    Harness::builder().with_size(vec2(900.0, 600.0)).renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    )
}

/// Distinct colours in the middle of the image (drawn lines add some to the background's).
fn colours(img: &image::RgbaImage) -> usize {
    let (w, h) = img.dimensions();
    let mut seen = std::collections::BTreeSet::new();
    for y in (h / 4..3 * h / 4).step_by(2) {
        for x in (w / 4..3 * w / 4).step_by(2) {
            let p = img.get_pixel(x, y).0;
            seen.insert((p[0] / 8, p[1] / 8, p[2] / 8));
        }
    }
    seen.len()
}

/// What the window's painter does with the last frame (eframe's `paint_and_update_textures`):
/// write its vertices and indices to the device and record the pass. Nothing is read back (the
/// test renderer's read-back waits on the device, which a window never does).
fn paint(rs: &egui_wgpu::RenderState, h: &Harness<'static, CadApp>) {
    let ctx = &h.ctx;
    let ppp = ctx.pixels_per_point();
    let size = ctx.content_rect().size() * ppp;
    let screen = egui_wgpu::ScreenDescriptor { pixels_per_point: ppp, size_in_pixels: [size.x.round() as u32, size.y.round() as u32] };
    let prims = ctx.tessellate(h.output().shapes.clone(), ppp);
    let mut renderer = rs.renderer.write();
    let mut encoder = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("paint") });
    let user = renderer.update_buffers(&rs.device, &rs.queue, &mut encoder, &prims, &screen);
    let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("paint"),
        size: wgpu::Extent3d { width: screen.size_in_pixels[0], height: screen.size_in_pixels[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: rs.target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paint"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                ..Default::default()
            })
            .forget_lifetime();
        renderer.render(&mut pass, &prims, &screen);
    }
    rs.queue.submit(user.into_iter().chain([encoder.finish()]));
}

fn new_device() -> egui_wgpu::RenderState {
    egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::default())
}

#[test]
fn a_lost_device_draws_nothing_more_and_the_window_is_made_again_with_the_drawings() {
    let rs = new_device();
    let mut s = Session::empty();
    s.open_drawing(drawing(), "Plan", None);
    let mut app = CadApp::new(s, Services::default());
    app.set_wgpu(&rs, None);
    let mut h = harness(app, rs.clone());
    h.run_steps(3);
    let before = h.render().expect("a frame on the working device");
    assert!(colours(&before) >= 2, "the lines are drawn");
    let entities = h.state().session.doc().unwrap().entity_count();
    assert!(!h.state().graphics_lost());

    // An error on the working device reaches the uncaptured-error handler `gpu::install`
    // registered (wgpu's default handler panics): recorded, and the app goes on drawing.
    let too_big = rs.device.limits().max_buffer_size.saturating_add(1);
    let _ = rs.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("too big"),
        size: too_big,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    h.run_steps(2);
    let n = h.state().gpu_notice.clone().expect("the error is recorded");
    assert!(n.fault.errors >= 1 && !n.fault.lost, "{n:?}");
    assert!(!h.state().graphics_lost());
    assert!(colours(&h.render().expect("still drawn")) >= 2);

    // What wgpu does when the driver runs out of memory: the device becomes invalid and its lost
    // callback (registered by `gpu::install`) is told why.
    rs.device.destroy();
    h.state().gpu_faults.device_lost(DeviceLostReason::Unknown, "Out of memory".into());
    assert!(h.state().graphics_lost());
    for _ in 0..3 {
        h.run_steps(1);
        assert!(h.output().shapes.is_empty(), "nothing is painted for the dead device");
        // egui's renderer writing this frame's vertices to the dead device panicked here: "Failed
        // to create staging buffer" (the window closed with exit 101).
        paint(&rs, &h);
    }
    h.run_steps(1);
    {
        let app = h.state();
        assert!(app.graphics_lost(), "the host is asked to make the window again");
        assert_eq!(app.session.docs.len(), 1, "the drawing is still open");
        assert_eq!(app.session.doc().unwrap().entity_count(), entities, "and unchanged");
        let n = app.gpu_notice.as_ref().expect("the loss is recorded");
        assert!(n.fault.lost && n.fault.message.contains("Out of memory") && !n.restarted, "{n:?}");
    }

    // The host makes the window again on a new device with the same app.
    let rs2 = new_device();
    let mut app = h.into_state();
    app.restart_graphics(&rs2, None, None);
    assert!(!app.graphics_lost());
    let mut h = harness(app, rs2);
    h.run_steps(3);
    let after = h.render().expect("a frame on the new device");
    assert!(colours(&after) >= 2, "the lines are drawn again");
    let app = h.state();
    assert_eq!(app.session.doc().unwrap().entity_count(), entities);
    assert!(app.canvas.gpu.is_some(), "drawn on the new device");
    let n = app.gpu_notice.as_ref().expect("told once");
    assert!(n.restarted && n.fault.lost, "{n:?}");
}

/// `ui.gpu {"lose": true}` loses the device the same way, so the recovery can be tried on a real
/// window through the control channel.
#[test]
fn the_device_can_be_lost_on_request() {
    let rs = new_device();
    let mut s = Session::empty();
    s.open_drawing(drawing(), "Plan", None);
    let mut app = CadApp::new(s, Services::default());
    assert_eq!(app.run("ui.gpu", serde_json::json!({"lose": true})).unwrap(), serde_json::json!({"lost": false}), "no device: the CPU draws");
    app.set_wgpu(&rs, None);
    assert_eq!(app.run("ui.gpu", serde_json::json!({"lose": true})).unwrap(), serde_json::json!({"lost": true}));
    assert!(app.graphics_lost());
    let mut h = harness(app, rs.clone());
    h.run_steps(2);
    assert!(h.output().shapes.is_empty());
    paint(&rs, &h);
    let mut app = h.into_state();
    app.restart_graphics(&new_device(), None, None);
    let n = app.gpu_notice.clone().unwrap();
    assert!(n.restarted && n.fault.message.contains("on request"), "{n:?}");
}
