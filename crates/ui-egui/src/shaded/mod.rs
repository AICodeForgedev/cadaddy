//! The Model tab in a visual style other than 2D Wireframe (VSCURRENT): the drawing's 3D scene
//! ([`cadaddy_render::shade`]) drawn with a depth buffer — on the GPU into an offscreen texture
//! (4× MSAA) when the app has a wgpu device, else by the CPU rasterizer that plots and exports
//! share — and shown under the canvas's overlays (grid, highlights, grips, cursor).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cadaddy_color::Rgb;
use cadaddy_geom::Vec3;
use cadaddy_render::model3::{self, Force, Model3, Stream, Tier};
use cadaddy_render::shade::{self, Scene, ShadeView, VisualStyle};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

use crate::CadApp;
use crate::canvas::Xf;
use crate::gpu::{Tracked, TrackedView};

/// What a model was built from: the drawing (its allocation and revision) and the detail band.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SceneKey {
    doc: usize,
    revision: u64,
    band: i32,
    /// The baked lighting's version (LIGHTBAKE): a bake builds the model again.
    lightmap: u64,
    /// The 4D date shown (the model as it stands then).
    fourd: Option<crate::fourdpalette::Shown>,
}

/// A model being built off the UI thread, and how far it has got.
struct Pending {
    key: SceneKey,
    progress: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    started: web_time::Instant,
    #[cfg(not(target_arch = "wasm32"))]
    handle: std::thread::JoinHandle<Model3>,
}

/// What the 3D view costs and draws (the control channel's `ui.perf3d`).
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Triangles in the model, instances counted as placed.
    pub tris_total: usize,
    /// Triangles the last frame drew (levels of detail and culling applied).
    pub tris_drawn: usize,
    pub instances: usize,
    pub instances_drawn: usize,
    pub prototypes: usize,
    pub chunks: usize,
    pub chunks_drawn: usize,
    /// Bytes the model takes in memory, and in GPU buffers.
    pub model_bytes: usize,
    pub gpu_bytes: usize,
    /// Milliseconds the last model took to build; the last frame's encoding; the average time
    /// between frames lately.
    pub build_ms: f64,
    pub encode_ms: f64,
    pub frame_ms: f64,
    /// A model is being built: the share done (0 … 1).
    pub building: Option<f64>,
    /// The last orbit benchmark ([`bench`]): frames per second and frames drawn.
    pub bench_fps: Option<f64>,
    pub bench_frames: u32,
    /// Triangles lit by baked light (LIGHTBAKE).
    pub baked: usize,
    /// The fidelity tier drawn (none: the model has no tiers), and whether Auto chose it.
    pub tier: Option<&'static str>,
    pub auto: bool,
    /// Streamed cells held, their bytes and triangles, cells still to come, and the last
    /// update's milliseconds.
    pub stream_cells: usize,
    pub stream_bytes: usize,
    pub stream_tris: usize,
    pub stream_pending: usize,
    pub stream_ms: f64,
    /// Elements a search marks that the tier doesn't draw.
    pub markers: usize,
}

/// An orbit benchmark under way: frames left, the azimuth reached, when it started.
#[derive(Clone, Copy, Debug)]
struct Bench {
    left: u32,
    azimuth: f64,
    frames: u32,
    start: web_time::Instant,
}

/// Start an orbit benchmark of `frames` frames: each turns the view 2° about Z (at 30° up), as
/// fast as frames are drawn; [`Stats::bench_fps`] holds the result once done.
pub fn bench(app: &mut CadApp, frames: u32) {
    app.canvas.shade.bench = Some(Bench { left: frames.clamp(1, 100_000), azimuth: 0.0, frames: 0, start: web_time::Instant::now() });
    app.canvas.shade.stats.bench_fps = None;
}

/// The shaded view's caches.
#[derive(Default)]
pub struct State {
    model: Option<(SceneKey, Arc<Model3>)>,
    pending: Option<Pending>,
    /// The model as one scene, for the CPU rasterizer.
    flat: Option<(SceneKey, Arc<Scene>)>,
    gpu: Option<Gpu>,
    /// What the GPU buffers hold: model, style and background.
    uploaded: Option<(SceneKey, VisualStyle, [u8; 3])>,
    texture: Option<egui::TextureId>,
    /// The CPU path's last image and what it showed.
    cpu: Option<(String, egui::TextureHandle)>,
    pub stats: Stats,
    last_frame: Option<web_time::Instant>,
    bench: Option<Bench>,
    /// The section box while one of its faces is dragged (min, max), set when let go.
    sbox_drag: Option<([f64; 3], [f64; 3])>,
    /// Fidelity: the tier Auto chose last (its hysteresis), the streamed cells, and Auto's
    /// thresholds and the drawing's units per foot (for the drawing they were read from).
    auto_tier: Option<Tier>,
    stream: Stream,
    auto_for: Option<((usize, u64), model3::AutoTiers, f64)>,
}

impl State {
    /// Drop the GPU renderer (limits changed, or the GPU failed): the next frame starts again.
    pub(crate) fn reset_gpu(&mut self) {
        self.gpu = None;
        self.uploaded = None;
    }

    /// The fidelity tier the view drew last (none: everything).
    pub fn tier(&self) -> Option<&'static str> {
        self.stats.tier
    }

    /// The size the GPU drew the last frame at (the render target's), if it did.
    pub fn gpu_size(&self) -> Option<(u32, u32)> {
        self.gpu.as_ref().and_then(|g| g.target.as_ref()).map(|t| t.size)
    }
}

/// The visual style the Model tab shows in, when it is a 3D one.
pub fn active(app: &CadApp) -> Option<VisualStyle> {
    let st = app.session.state().ok()?;
    if st.space != cadaddy_doc::Space::Model || st.mspace.is_some() || st.block_edit.is_some() {
        return None;
    }
    (st.visual_style != VisualStyle::Wireframe2d).then_some(st.visual_style)
}

/// Draw the shaded view over `xf.rect`; false (nothing drawn) when the Model tab isn't in a 3D
/// visual style.
pub fn paint(app: &mut CadApp, ui: &egui::Ui, painter: &egui::Painter, xf: &Xf, bg: Rgb) -> bool {
    let Some(style) = active(app) else { return false };
    let ppp = ui.ctx().pixels_per_point();
    let (w, h) = ((xf.rect.width() * ppp).round().max(1.0) as u32, (xf.rect.height() * ppp).round().max(1.0) as u32);
    // Curves to about half a pixel, in power-of-two bands so zooming rebuilds rarely.
    let px_world = 1.0 / (xf.scale * f64::from(ppp)).max(1e-300);
    let band = (px_world * 0.5).log2().floor().clamp(-60.0, 60.0) as i32;
    let Some((key, model)) = self::model(app, band) else {
        // The first model is still being built.
        building_note(app, ui, painter, xf);
        return false;
    };
    let now = web_time::Instant::now();
    let st = &mut app.canvas.shade;
    if let Some(last) = st.last_frame {
        let dt = now.duration_since(last).as_secs_f64() * 1000.0;
        // Lately: a running average that forgets a pause.
        st.stats.frame_ms = if dt > 1000.0 || st.stats.frame_ms == 0.0 { dt.min(1000.0) } else { st.stats.frame_ms * 0.9 + dt * 0.1 };
    }
    st.last_frame = Some(now);
    let height = f64::from(xf.rect.height()) / xf.scale.max(1e-300);
    let view = ShadeView::new(xf.proj, xf.center, height, w, h);
    // Fidelity: the tier fixed, else the one the zoom calls for (with hysteresis); the cells of
    // streamed elements that tier needs in view (a few milliseconds a frame, the rest next).
    let (tier, force) = fidelity(app, &model, height);
    let st = &mut app.canvas.shade;
    let step = st.stream.update(&model, &view, tier, STREAM_MS);
    if step.pending > 0 {
        ui.ctx().request_repaint();
    }
    st.stats.stream_cells = st.stream.cells.len();
    st.stats.stream_bytes = st.stream.bytes;
    st.stats.stream_tris = st.stream.tris();
    st.stats.stream_pending = step.pending;
    if step.added + step.dropped > 0 {
        st.stats.stream_ms = step.ms;
    }
    // The model's lighting with the render environment (RENDERENVIRONMENT) applied: background,
    // fog, exposure, white balance, the environment map's sky. A file gone since leaves the rest.
    let lit = environment_lighting(app, &model);
    let mut drawn = false;
    let mut markers: Vec<u32> = Vec::new();
    // On the GPU unless it can't (see `gpucaps`): at most the size its limits and memory allow
    // (shown scaled up), with its multisampling.
    let caps = app.gpu_caps.clone();
    if let Some(rs) = app.wgpu_rs.clone().filter(|_| !caps.cpu) {
        let (gw, gh) = caps.target_size(w, h);
        let view = ShadeView::new(xf.proj, xf.center, height, gw, gh);
        let (w, h) = (gw, gh);
        let st = &mut app.canvas.shade;
        if st.gpu.as_ref().is_some_and(|g| g.samples != caps.msaa) {
            st.gpu = None;
            st.uploaded = None;
        }
        let g = st.gpu.get_or_insert_with(|| Gpu::new(&rs.device, caps.msaa));
        let up = Some((key, style, [bg.0, bg.1, bg.2]));
        if st.uploaded != up {
            g.upload(&rs.device, &rs.queue, &model, style, bg);
            g.stream_gen = None;
            st.uploaded = up;
        }
        g.load_tiers(&rs.device, &model, tier);
        g.upload_stream(&rs.device, &st.stream, style, bg);
        g.set_lighting(&rs.device, &rs.queue, lit.as_ref());
        let t0 = web_time::Instant::now();
        let vis = g.render(&rs.device, &rs.queue, &view, style, bg, tier, force.as_ref(), &st.stream);
        st.stats.markers = vis.markers.len();
        markers = vis.markers;
        st.stats.encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
        st.stats.tris_drawn = vis.tris;
        st.stats.instances_drawn = vis.instances.iter().map(|g| g.2.len()).sum();
        st.stats.chunks_drawn = vis.chunks.len();
        // Buffers and textures, plus the render targets (4× colour and depth, the resolved image,
        // the pre-pass depth) and the shadow map.
        let (tw, th) = (u64::from(w), u64::from(h));
        st.stats.gpu_bytes = g.bytes
            + g.tier_bytes.iter().sum::<usize>()
            + g.stream_bytes
            + (tw * th * (u64::from(g.samples) * 8 + 8)) as usize
            + (u64::from(SHADOW_PX) * u64::from(SHADOW_PX) * 4) as usize;
        if let Some(tv) = g.view() {
            let mut r = rs.renderer.write();
            match st.texture {
                Some(id) => r.update_egui_texture_from_wgpu_texture(&rs.device, tv, wgpu::FilterMode::Linear, id),
                None => st.texture = Some(r.register_native_texture(&rs.device, tv, wgpu::FilterMode::Linear)),
            }
        }
        if let Some(id) = st.texture {
            painter.image(id, xf.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            drawn = true;
        }
    }
    if !drawn {
        // The CPU rasterizer, redrawn when the view, model or style changes.
        let env = app.session.state().ok().map(|st| format!("{:?}", st.render_environment)).unwrap_or_default();
        let ck = format!(
            "{:?}|{:?}|{:?}|{}x{}|{:?}|{:?}|{:?}|{env}|{tier:?}|{}|{force:?}",
            key,
            view.center,
            view.height,
            w,
            h,
            view.dir,
            view.right,
            (style, bg),
            app.canvas.shade.stream.generation
        );
        let fresh = app.canvas.shade.cpu.as_ref().is_some_and(|(k, _)| *k == ck);
        if !fresh {
            // With fidelity: what the tier draws in view and the streamed cells; else the whole
            // model, kept.
            let flat = if model.fid.is_some() {
                let vis = model.visible_at(&view, tier, force.as_ref());
                app.canvas.shade.stats.markers = vis.markers.len();
                let mut f = model.flatten_visible(&vis);
                let stream = &app.canvas.shade.stream;
                for k in stream.visible(&view) {
                    if let Some(c) = stream.cells.get(&k) {
                        f.tris.extend_from_slice(&c.scene.tris);
                        f.lines.extend_from_slice(&c.scene.lines);
                    }
                }
                Arc::new(f)
            } else {
                match &app.canvas.shade.flat {
                    Some((k, f)) if *k == key => Arc::clone(f),
                    _ => {
                        let f = Arc::new(model.flatten());
                        app.canvas.shade.flat = Some((key, Arc::clone(&f)));
                        f
                    }
                }
            };
            let r = shade::rasterize_lit(&flat, &view, style, bg, lit.as_ref().or(flat.lighting.as_ref()));
            let img = egui::ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &r.rgba);
            let tex = ui.ctx().load_texture("shaded_view", img, egui::TextureOptions::LINEAR);
            app.canvas.shade.cpu = Some((ck, tex));
        }
        if let Some((_, tex)) = &app.canvas.shade.cpu {
            painter.image(tex.id(), xf.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            drawn = true;
        }
    }
    building_note(app, ui, painter, xf);
    if model.fid.is_some() {
        if markers.is_empty()
            && let Some(force) = force.as_ref()
        {
            markers = model.visible_at(&view, tier, Some(force)).markers;
        }
        draw_markers(painter, xf, &model, &markers);
        fidelity_bar(app, ui, xf, tier);
    }
    // An orbit benchmark: the next frame's view, and another frame straight away.
    if let Some(mut b) = app.canvas.shade.bench {
        b.frames += 1;
        b.left = b.left.saturating_sub(1);
        b.azimuth += 2.0;
        if b.left == 0 {
            let secs = b.start.elapsed().as_secs_f64().max(1e-9);
            app.canvas.shade.stats.bench_fps = Some(f64::from(b.frames) / secs);
            app.canvas.shade.stats.bench_frames = b.frames;
            app.canvas.shade.bench = None;
        } else {
            app.canvas.shade.bench = Some(b);
            let _ = app.session.execute("vpoint", &serde_json::json!({ "azimuth": b.azimuth, "elevation": 30.0 }));
            ui.ctx().request_repaint();
        }
    }
    drawn
}

/// Milliseconds a frame spends bringing streamed cells in (the rest come in the next frames).
const STREAM_MS: f64 = 12.0;

/// The fidelity tier the view draws `model` at, `height` drawing units tall, and the elements a
/// search marks: the session's fixed tier, else Auto's for the zoom (the last one held until the
/// zoom is past the margin). The session hears which tier was drawn: a click on a piece drawn
/// only as part of its run picks the run. A model without tiers draws everything.
fn fidelity(app: &mut CadApp, model: &Model3, height: f64) -> (Tier, Option<Force>) {
    if model.fid.is_none() {
        app.canvas.shade.stats.tier = None;
        app.canvas.shade.stats.auto = false;
        return (Tier::Detail, None);
    }
    let Ok(st) = app.session.state() else { return (Tier::Detail, None) };
    let (mode, force) = (st.fidelity.mode, st.fidelity.force.clone());
    let doc_key = (Arc::as_ptr(&st.doc) as usize, st.revision);
    let (auto, upf) = match app.canvas.shade.auto_for {
        Some((k, a, u)) if k == doc_key => (a, u),
        _ => {
            let a = cadaddy_engine::cmd::fidelity::Rules::of(&st.doc).0.auto.tiers();
            let u = cadaddy_engine::cmd::fidelity::units_per_foot(&st.doc);
            app.canvas.shade.auto_for = Some((doc_key, a, u));
            (a, u)
        }
    };
    let tier = mode.unwrap_or_else(|| auto.tier(height / upf.max(1e-9), app.canvas.shade.auto_tier));
    let shade = &mut app.canvas.shade;
    shade.auto_tier = Some(tier);
    shade.stats.tier = Some(tier.name());
    shade.stats.auto = mode.is_none();
    if let Ok(st) = app.session.state_mut()
        && st.fidelity.shown != Some(tier)
    {
        st.fidelity.shown = Some(tier);
    }
    (tier, force)
}

/// The elements a search marks (`view.fidelity {show}`) that the tier doesn't draw: a ring at
/// each.
fn draw_markers(painter: &egui::Painter, xf: &Xf, model: &Model3, markers: &[u32]) {
    let Some(f) = model.fid.as_ref() else { return };
    let t = crate::theme::Tokens::get();
    for i in markers.iter().take(model3::MAX_MARKERS) {
        let Some(e) = f.elems.get(*i as usize) else { continue };
        let c = e.bounds.center();
        let at = match xf.proj {
            Some(pr) => xf.to_screen(pr.project(c)),
            None => xf.to_screen(cadaddy_geom::Vec2::new(c.x, c.y)),
        };
        if xf.rect.contains(at) {
            painter.circle_stroke(at, 5.0, egui::Stroke::new(2.0, t.accent));
        }
    }
}

/// A tier's name in the view's menu.
fn tier_label(t: Tier) -> &'static str {
    match t {
        Tier::Building => crate::i18n::t("Building"),
        Tier::Floor => crate::i18n::t("Floor"),
        Tier::Room => crate::i18n::t("Room"),
        Tier::Detail => crate::i18n::t("Detail"),
    }
}

/// The view's fidelity in its top left corner, Auto (and the tier it chose) or the fixed tier,
/// with a menu to fix a tier or go back to Auto (`view.fidelity {level}`).
fn fidelity_bar(app: &mut CadApp, ui: &egui::Ui, xf: &Xf, tier: Tier) {
    let auto = app.canvas.shade.stats.auto;
    let label = if auto {
        format!("{}: {} · {}", crate::i18n::t("Fidelity"), crate::i18n::t("Auto"), tier_label(tier))
    } else {
        format!("{}: {}", crate::i18n::t("Fidelity"), tier_label(tier))
    };
    let mut pick: Option<Option<Tier>> = None;
    egui::Area::new(ui.id().with("fidelity-bar")).order(egui::Order::Middle).fixed_pos(xf.rect.left_top() + egui::vec2(8.0, 8.0)).show(
        ui.ctx(),
        |ui| {
            egui::ComboBox::from_id_salt("fidelity-level").selected_text(label).show_ui(ui, |ui| {
                if ui.selectable_label(auto, crate::i18n::t("Auto")).clicked() {
                    pick = Some(None);
                }
                for t in Tier::ALL {
                    if ui.selectable_label(!auto && t == tier, tier_label(t)).clicked() {
                        pick = Some(Some(t));
                    }
                }
            });
        },
    );
    if let Some(p) = pick
        && let Err(e) = app.run("view.fidelity", serde_json::json!({ "level": p.map_or("auto", Tier::name) }))
    {
        app.set_status(format!("{}: {e}", crate::i18n::t("Fidelity")));
    }
}

/// The model of `doc`: with fidelity tiers when it is the built model (the BIM view's drawing).
#[allow(clippy::too_many_arguments)]
fn build3(
    doc: &Arc<cadaddy_doc::Drawing>,
    bim: bool,
    tol: f64,
    bg: Rgb,
    progress: &AtomicUsize,
    total: &AtomicUsize,
    planes: &[cadaddy_doc::SectionPlane],
) -> Model3 {
    if bim {
        let classify = cadaddy_engine::cmd::fidelity::classifier(doc);
        model3::build_fid(Arc::clone(doc), &cadaddy_doc::Space::Model, tol, bg, progress, total, planes, &classify, model3::FidOptions::default())
    } else {
        model3::build_clipped(doc, &cadaddy_doc::Space::Model, tol, bg, progress, total, planes)
    }
}

/// The box's 12 edges, as pairs of corner numbers (bit 0: max x, bit 1: max y, bit 2: max z).
const BOX_EDGES: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];

/// The coordination section box over the shaded view: its outline and a handle on each face.
/// Dragging a handle moves that face along its axis; letting go sets the box
/// (`coordination.sectionbox {min, max}`), which rebuilds the clipped model.
pub fn section_box_handles(app: &mut CadApp, ui: &egui::Ui, painter: &egui::Painter, xf: &Xf) {
    use cadaddy_engine::cmd::coordination::{self as coord, BoxFace};
    let Ok(st) = app.session.state() else { return };
    let crs = coord::coord_render_state(&st.doc);
    if !crs.sbox_on {
        app.canvas.shade.sbox_drag = None;
        return;
    }
    let (mut lo, mut hi) = app.canvas.shade.sbox_drag.unwrap_or((crs.sbox_min, crs.sbox_max));
    if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) {
        return;
    }
    let to_screen = |p: [f64; 3]| {
        let v = Vec3::new(p[0], p[1], p[2]);
        match xf.proj {
            Some(pr) => xf.to_screen(pr.project(v)),
            None => xf.to_screen(cadaddy_geom::Vec2::new(v.x, v.y)),
        }
    };
    let t = crate::theme::Tokens::get();
    {
        let corner =
            |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        let stroke = egui::Stroke::new(1.5, t.accent);
        for (a, b) in BOX_EDGES {
            painter.line_segment([to_screen(corner(a)), to_screen(corner(b))], stroke);
        }
    }
    let mut set = None;
    for (k, face) in BoxFace::ALL.into_iter().enumerate() {
        let centre = coord::sbox_face_centre(lo, hi, face);
        let at = to_screen(centre);
        let r = egui::Rect::from_center_size(at, egui::vec2(12.0, 12.0));
        let resp = ui.interact(r, ui.id().with(("sbox-face", k)), egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::Grab);
        if resp.dragged() {
            // One box length along the face's axis, on screen.
            let a = face.axis();
            let step = hi.get(a).zip(lo.get(a)).map_or(1.0, |(h, l)| (h - l).abs().max(1.0));
            let mut ahead = centre;
            if let Some(v) = ahead.get_mut(a) {
                *v += step;
            }
            let axis = to_screen(ahead) - at;
            let drag = resp.drag_delta();
            if let Some(d) = coord::sbox_drag_delta([f64::from(axis.x), f64::from(axis.y)], step, [f64::from(drag.x), f64::from(drag.y)]) {
                (lo, hi) = coord::sbox_drag_face(lo, hi, face, d);
                app.canvas.shade.sbox_drag = Some((lo, hi));
            }
        }
        if resp.drag_stopped() {
            set = Some((lo, hi));
        }
        let fill = if resp.dragged() || resp.hovered() { t.grip_hot } else { t.grip };
        painter.rect_filled(r.shrink(2.0), 2.0, fill);
        painter.rect_stroke(r.shrink(2.0), 2.0, egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Middle);
    }
    if let Some((lo, hi)) = set {
        app.canvas.shade.sbox_drag = None;
        if let Err(e) = app.run("coordination.sectionbox", serde_json::json!({ "on": true, "min": lo, "max": hi })) {
            app.set_status(format!("Section box: {e}"));
        }
    }
}

/// `model`'s lighting with the session's render environment applied (`None`: not lit physically).
fn environment_lighting(app: &CadApp, model: &Model3) -> Option<cadaddy_render::light::Lighting> {
    let mut lit = model.base.lighting.clone()?;
    let _ = cadaddy_engine::cmd::apply_environment(&app.session, &mut lit);
    Some(lit)
}

/// While a model is built, a note in the view's corner with how far it has got.
fn building_note(app: &CadApp, ui: &egui::Ui, painter: &egui::Painter, xf: &Xf) {
    let Some(share) = app.canvas.shade.stats.building else { return };
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    let t = crate::theme::Tokens::get();
    let text = format!("{} {:.0}%", crate::i18n::t("Preparing the 3D view…"), share * 100.0);
    let at = egui::pos2(xf.rect.right() - 12.0, xf.rect.bottom() - 12.0);
    painter.text(at, egui::Align2::RIGHT_BOTTOM, text, egui::FontId::proportional(13.0), t.text);
}

/// The model at detail `band` (curves to 2^band drawing units). A changed drawing or band starts
/// a new build off the UI thread; until it is done the last model stays on screen (`None` before
/// the first is ready).
fn model(app: &mut CadApp, band: i32) -> Option<(SceneKey, Arc<Model3>)> {
    // Looks for a lightmap beside the drawing the first time.
    let _ = cadaddy_engine::cmd::lightbake::current(&mut app.session);
    let st = app.session.state().ok()?;
    let key = SceneKey { doc: Arc::as_ptr(&st.doc) as usize, revision: st.revision, band, lightmap: st.lightmap_rev, fourd: app.fourd };
    let mut doc = Arc::clone(&st.doc);
    let shade_st = &mut app.canvas.shade;
    if shade_st.model.as_ref().is_some_and(|(k, _)| *k == key) {
        shade_st.stats.building = None;
        return shade_st.model.clone();
    }
    let tcol = crate::theme::Tokens::get().canvas;
    let bg = Rgb(tcol.r(), tcol.g(), tcol.b());
    let tol = 2f64.powi(band);
    // Read section box planes from coordination state for 3D clipping.
    let crs = cadaddy_engine::cmd::coordination::coord_render_state(&doc);
    let sbox_planes = crs.sbox_planes();
    // The BIM view: only the built model, in trade colours, the shell ghosted; drawn in
    // fidelity tiers.
    let mut bim = false;
    if let Some(d) = cadaddy_engine::cmd::bimlook::display(&doc) {
        doc = Arc::new(d);
        bim = true;
    }
    if let Some(f) = key.fourd
        && let Ok((staged, _)) = cadaddy_engine::cmd::fourd::staged(&doc, f.day, f.ghost)
    {
        doc = Arc::new(staged);
    }
    // A build for another key is dropped (it finishes in the background and is ignored).
    if shade_st.pending.as_ref().is_some_and(|p| p.key != key) {
        shade_st.pending = None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if shade_st.pending.is_none() {
            let (progress, total) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(1)));
            let (p2, t2) = (Arc::clone(&progress), Arc::clone(&total));
            let planes = sbox_planes.clone();
            let doc_t = Arc::clone(&doc);
            let handle =
                std::thread::Builder::new().name("cadaddy-3d-model".into()).spawn(move || build3(&doc_t, bim, tol, bg, &p2, &t2, &planes)).ok();
            match handle {
                Some(handle) => shade_st.pending = Some(Pending { key, progress, total, started: web_time::Instant::now(), handle }),
                // No thread: build here.
                None => {
                    let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
                    let started = web_time::Instant::now();
                    let m = Arc::new(build3(&doc, bim, tol, bg, &p, &t, &sbox_planes));
                    return Some(finish(app, key, m, started));
                }
            }
        }
        let done = app.canvas.shade.pending.as_ref().is_some_and(|p| p.handle.is_finished());
        if done
            && let Some(p) = app.canvas.shade.pending.take()
            && let Ok(m) = p.handle.join()
        {
            return Some(finish(app, key, Arc::new(m), p.started));
        }
        if let Some(p) = &app.canvas.shade.pending {
            let total = p.total.load(Ordering::Relaxed).max(1);
            app.canvas.shade.stats.building = Some((p.progress.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0));
        }
        app.canvas.shade.model.clone()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let started = web_time::Instant::now();
        let m = Arc::new(build3(&doc, bim, tol, bg, &p, &t, &sbox_planes));
        Some(finish(app, key, m, started))
    }
}

/// Keep a finished model and its numbers.
fn finish(app: &mut CadApp, key: SceneKey, m: Arc<Model3>, started: web_time::Instant) -> (SceneKey, Arc<Model3>) {
    // The baked light for the base geometry's objects (instances keep the sky's).
    let mut m = Arc::unwrap_or_clone(m);
    m.base.baked = cadaddy_engine::cmd::lightbake::baked_for(&mut app.session, &m.base);
    let baked = m.base.baked.as_ref().map_or(0, |b| b.cells.iter().filter(|c| c.is_some()).count());
    let m = Arc::new(m);
    let st = &mut app.canvas.shade;
    st.stats.baked = baked;
    st.stats.build_ms = started.elapsed().as_secs_f64() * 1000.0;
    st.stats.tris_total = m.total_tris();
    st.stats.instances = m.instances.len();
    st.stats.prototypes = m.protos.len();
    st.stats.chunks = m.chunks.len();
    st.stats.model_bytes = m.bytes();
    st.stats.building = None;
    st.model = Some((key, Arc::clone(&m)));
    (key, m)
}

/// Display values (sRGB-encoded by the shader), as egui shows a texture.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Position (3 × f32, from the scene's origin) + normal (3 × f32) + sRGB colour (4 × u8) +
/// texture coordinates (2 × f32) + material (roughness, metallic, texture layer, opacity: 4 × f32) +
/// lightmap coordinates of its front and back (4 × f32; negative: none).
const VERTEX_SIZE: u64 = 68;
/// An instance's placement: three rows of four f32.
const INSTANCE_SIZE: u64 = 48;
/// The frame's uniform block (see `shader.wgsl`).
const UNIFORM_SIZE: u64 = 1488;
/// Texture array layer size and most layers.
const TEX_PX: u32 = 256;
const MAX_LAYERS: usize = 64;
/// Shadow map size.
const SHADOW_PX: u32 = 2048;

struct Target {
    size: (u32, u32),
    /// The resolved frame (what `view` shows; copied out by the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    frame: Tracked<wgpu::Texture>,
    /// The multisampled colour (none without MSAA: the frame is drawn into directly).
    msaa: Option<TrackedView>,
    depth: TrackedView,
    view: wgpu::TextureView,
    /// The opaque surfaces' depth, one sample a pixel (the ambient occlusion's pre-pass).
    prepass: TrackedView,
}

struct Buffer {
    buffer: Tracked,
}

/// The shaded view's GPU renderer.
struct Gpu {
    tri_pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bg_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    /// The ambient occlusion's depth pre-pass.
    depth_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The shadow pass's own group (the uniform only: it writes the shadow map the others read).
    shadow_group: wgpu::BindGroup,
    uniform: Tracked,
    sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    /// The lightmap's: linear, clamped (cells have borders).
    lm_sampler: wgpu::Sampler,
    shadow_view: TrackedView,
    /// Made for the current target and model.
    bind_group: Option<wgpu::BindGroup>,
    tris: Option<Buffer>,
    /// See-through triangles (drawn after the others, blended).
    clear_tris: Option<Buffer>,
    lines: Option<Buffer>,
    /// Vertex ranges in the three buffers (opaque, see-through, lines): each base chunk's, and
    /// each prototype's at each level of detail.
    /// Each chunk's buffers (0: the main ones; 1 to 3: its fidelity tier's) and its ranges there.
    chunk_ranges: Vec<(usize, [std::ops::Range<u32>; 3])>,
    proto_ranges: Vec<[[std::ops::Range<u32>; 3]; 3]>,
    /// The floor, room and detail tiers' chunks (opaque, see-through, lines), loaded while the view
    /// shows the tier; their bytes; how the main upload packed (the tiers pack alike).
    tier_bufs: [Option<[Option<Buffer>; 3]>; 3],
    tier_bytes: [usize; 3],
    packing: Option<Packing>,
    /// The streamed cells' triangles and lines (opaque, see-through, lines), each cell's vertex
    /// ranges in them, and the stream generation they hold.
    stream_bufs: [Option<Buffer>; 3],
    stream_ranges: std::collections::BTreeMap<(u8, u32), [std::ops::Range<u32>; 3]>,
    stream_gen: Option<u64>,
    stream_bytes: usize,
    /// The frame's instance placements (slot 0: the identity, for the base geometry).
    instances: Option<Tracked>,
    instance_cap: u64,
    model: Option<Arc<Model3>>,
    /// Bytes in the GPU buffers.
    bytes: usize,
    /// The scene's centre (vertices are stored relative to it, keeping f32 precision) and its
    /// extent, for the depth range.
    origin: Vec3,
    radius: f64,
    lighting: Option<cadaddy_render::light::Lighting>,
    /// The bind group's textures (bound with the target's pre-pass depth): the materials', the
    /// baked light's and the background image's (one white texel without one; `bg_image` the
    /// image it holds, by address).
    tex_view: Option<TrackedView>,
    lm_view: Option<TrackedView>,
    bg_view: TrackedView,
    bg_sampler: wgpu::Sampler,
    bg_image: usize,
    target: Option<Target>,
    /// Samples per pixel (see [`crate::gpucaps`]).
    samples: u32,
}

impl Gpu {
    fn new(device: &wgpu::Device, samples: u32) -> Gpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaded"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shaded"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(
                    3,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(4, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
                entry(
                    5,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(6, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(
                    7,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    8,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(9, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shaded"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let uniform = Tracked::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shaded_frame"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        const ATTRS: [wgpu::VertexAttribute; 6] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Float32x2, 4 => Float32x4, 8 => Float32x4];
        const INST: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let vb = [
            Some(wgpu::VertexBufferLayout { array_stride: VERTEX_SIZE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }),
            Some(wgpu::VertexBufferLayout { array_stride: INSTANCE_SIZE, step_mode: wgpu::VertexStepMode::Instance, attributes: &INST }),
        ];
        let pipeline = |label: &str, vs: &str, fs: &str, topology: wgpu::PrimitiveTopology, write_depth: bool, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &vb, compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(write_depth),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shaded_sky"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_bg"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_bg"),
                targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shaded_shadow"),
            entries: &[entry(
                0,
                wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            )],
        });
        let shadow_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shaded_shadow"),
            bind_group_layouts: &[Some(&shadow_layout)],
            immediate_size: 0,
        });
        let shadow_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shaded_shadow"),
            layout: &shadow_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shaded_shadow"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_shadow"), buffers: &vb, compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shaded_depth"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_depth"), buffers: &vb, compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shaded_textures"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shaded_shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_tex = Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_shadow_map"),
            size: wgpu::Extent3d { width: SHADOW_PX, height: SHADOW_PX, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        Gpu {
            tri_pipeline: pipeline("shaded_tris", "vs_tri", "fs_tri", wgpu::PrimitiveTopology::TriangleList, true, None),
            clear_pipeline: pipeline("shaded_glass", "vs_tri", "fs_tri", wgpu::PrimitiveTopology::TriangleList, false, alpha),
            line_pipeline: pipeline("shaded_lines", "vs_line", "fs_line", wgpu::PrimitiveTopology::LineList, false, None),
            bg_pipeline,
            shadow_pipeline,
            depth_pipeline,
            layout,
            shadow_group,
            uniform,
            sampler,
            shadow_sampler,
            lm_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("shaded_lightmap"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            shadow_view: shadow_tex.into_view(&wgpu::TextureViewDescriptor::default()),
            bind_group: None,
            tris: None,
            clear_tris: None,
            lines: None,
            chunk_ranges: Vec::new(),
            proto_ranges: Vec::new(),
            tier_bufs: Default::default(),
            tier_bytes: [0; 3],
            packing: None,
            stream_bufs: [None, None, None],
            stream_ranges: std::collections::BTreeMap::new(),
            stream_gen: None,
            stream_bytes: 0,
            instances: None,
            instance_cap: 0,
            model: None,
            bytes: 0,
            origin: Vec3::ZERO,
            radius: 1.0,
            lighting: None,
            tex_view: None,
            lm_view: None,
            bg_view: image_texture(device, None, 1, 1, &[255, 255, 255, 255]).into_view(&wgpu::TextureViewDescriptor::default()),
            bg_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("shaded_background"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            }),
            bg_image: 0,
            target: None,
            samples,
        }
    }

    /// The model's triangles (none for Wireframe), the lines `style` shows, and the textures:
    /// the base chunk by chunk (the chunks of fidelity tiers finer than `loaded` left out: they
    /// aren't drawn), then each prototype at each level of detail, with the ranges each takes.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, model: &Arc<Model3>, style: VisualStyle, bg: Rgb) {
        let b = model.bounds;
        self.origin = if b.is_empty() { Vec3::ZERO } else { b.center() };
        self.radius = if b.is_empty() { 1.0 } else { (b.diag() * 0.5).max(1e-9) };
        let physical = style == VisualStyle::Realistic && model.base.lighting.is_some();
        self.lighting = if physical { model.base.lighting.clone() } else { None };
        // The textures of every scene, one layer each; each scene's numbering offset.
        let scenes: Vec<&Scene> = std::iter::once(&model.base).chain(model.protos.iter().flat_map(|p| p.lods.iter())).collect();
        let mut layers: Vec<&Arc<cadaddy_render::images::Raster>> = Vec::new();
        let mut offsets = Vec::with_capacity(scenes.len());
        for sc in &scenes {
            offsets.push(layers.len());
            layers.extend(sc.textures.iter().take(MAX_LAYERS.saturating_sub(layers.len())));
        }
        let how = Packing { style, bg, physical, layers: layers.len() };
        let mut pack = Pack::default();
        let o = self.origin;
        // The building tier's chunks (and everything of a model without tiers) here; the finer
        // tiers' in their own buffers, when a view needs them (`load_tiers`).
        let chunk_ranges: Vec<(usize, [std::ops::Range<u32>; 3])> = model
            .chunks
            .iter()
            .map(|c| match c.show.from {
                Tier::Building => (0, pack.emit(&how, &model.base, 0, c.tris.clone(), c.lines.clone(), o)),
                t => (t.index(), [0..0, 0..0, 0..0]),
            })
            .collect();
        let mut k = 1;
        let proto_ranges: Vec<[[std::ops::Range<u32>; 3]; 3]> = model
            .protos
            .iter()
            .map(|p| {
                let r = [0, 1, 2].map(|l| {
                    let sc = &p.lods[l];
                    let off = offsets.get(k + l).copied().unwrap_or(0);
                    pack.emit(&how, sc, off, 0..sc.tris.len(), 0..sc.lines.len(), Vec3::ZERO)
                });
                k += 3;
                r
            })
            .collect();
        self.bytes = pack.tris.len() + pack.clear.len() + pack.lines.len() + layers.len().max(1) * (TEX_PX * TEX_PX * 4) as usize;
        self.tris = buffer(device, "shaded_tris", &pack.tris);
        self.clear_tris = buffer(device, "shaded_glass", &pack.clear);
        self.lines = buffer(device, "shaded_lines", &pack.lines);
        self.chunk_ranges = chunk_ranges;
        self.proto_ranges = proto_ranges;
        self.model = Some(Arc::clone(model));
        self.packing = Some(how);
        self.tier_bufs = Default::default();
        self.tier_bytes = [0; 3];
        // The textures, one layer each, resized to the layer size (at least one layer).
        let count = layers.len().max(1) as u32;
        let tex = Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_textures"),
            size: wgpu::Extent3d { width: TEX_PX, height: TEX_PX, depth_or_array_layers: count },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        for (i, r) in layers.iter().enumerate() {
            let Some(img) = image::RgbaImage::from_raw(r.width, r.height, r.rgba.clone()) else { continue };
            let px = image::imageops::resize(&img, TEX_PX, TEX_PX, image::imageops::FilterType::Triangle).into_raw();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                &px,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(TEX_PX * 4), rows_per_image: Some(TEX_PX) },
                wgpu::Extent3d { width: TEX_PX, height: TEX_PX, depth_or_array_layers: 1 },
            );
        }
        let tex_view = tex.into_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        // The baked lighting atlas (RGBM), or one black texel.
        let (lw, lh, lbytes) = match model.base.baked.as_ref().filter(|_| physical) {
            Some(b) => (b.map.width, b.map.height, cadaddy_render::lightmap::rgbm(&b.map)),
            None => (1, 1, vec![0, 0, 0, 255]),
        };
        let lm_tex = Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_lightmap"),
            size: wgpu::Extent3d { width: lw.max(1), height: lh.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        if lbytes.len() as u64 == u64::from(lw) * u64::from(lh) * 4 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &lm_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &lbytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
            );
        }
        self.bytes += lbytes.len();
        self.lm_view = Some(lm_tex.into_view(&wgpu::TextureViewDescriptor::default()));
        self.tex_view = Some(tex_view);
        self.bind_group = None;
    }

    /// The group binding the frame's uniform, the model's textures, the shadow map, the baked
    /// light, the background image and target `t`'s pre-pass depth.
    fn group(&self, device: &wgpu::Device, t: &Target) -> Option<wgpu::BindGroup> {
        let (tex_view, lm_view) = (self.tex_view.as_ref()?, self.lm_view.as_ref()?);
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shaded"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tex_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.shadow_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.shadow_sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(lm_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.lm_sampler) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&t.prepass) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&self.bg_view) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(&self.bg_sampler) },
            ],
        }))
    }

    /// The frame's lighting (the render environment applied), and its background image on the
    /// GPU when it changed. Only a physically lit view (Realistic) takes it.
    fn set_lighting(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lit: Option<&cadaddy_render::light::Lighting>) {
        if self.lighting.is_none() {
            return;
        }
        let Some(lit) = lit else { return };
        let image = match &lit.background {
            cadaddy_render::light::Background::Image(r) => Some(r),
            _ => None,
        };
        let key = image.map_or(0, |r| Arc::as_ptr(r) as usize);
        if key != self.bg_image {
            let max = device.limits().max_texture_dimension_2d.max(1);
            let texture = match image {
                Some(r) if r.width <= max && r.height <= max => image_texture(device, Some(queue), r.width, r.height, &r.rgba),
                Some(r) => {
                    // Larger than the GPU takes: scaled to fit.
                    let k = f64::from(max) / f64::from(r.width.max(r.height));
                    let (w, h) = (((f64::from(r.width) * k) as u32).max(1), ((f64::from(r.height) * k) as u32).max(1));
                    let px = image::RgbaImage::from_raw(r.width, r.height, r.rgba.clone())
                        .map(|img| image::imageops::resize(&img, w, h, image::imageops::FilterType::Nearest).into_raw())
                        .unwrap_or_else(|| vec![255; (w * h * 4) as usize]);
                    image_texture(device, Some(queue), w, h, &px)
                }
                None => image_texture(device, None, 1, 1, &[255, 255, 255, 255]),
            };
            self.bg_view = texture.into_view(&wgpu::TextureViewDescriptor::default());
            self.bg_image = key;
            self.bind_group = None;
        }
        self.lighting = Some(lit.clone());
    }

    /// The chunks of the tiers up to `tier` that aren't on the GPU yet, each tier in its own
    /// buffers; the finer tiers' let go (a zoom out frees what it no longer draws).
    fn load_tiers(&mut self, device: &wgpu::Device, model: &Model3, tier: Tier) {
        let Some(how) = self.packing else { return };
        let o = self.origin;
        for slot in 1..=3usize {
            let t = Tier::from_index(slot);
            let Some(have) = self.tier_bufs.get_mut(slot - 1) else { continue };
            if t > tier {
                if have.take().is_some() {
                    for (s, r) in self.chunk_ranges.iter_mut() {
                        if *s == slot {
                            *r = [0..0, 0..0, 0..0];
                        }
                    }
                    if let Some(b) = self.tier_bytes.get_mut(slot - 1) {
                        *b = 0;
                    }
                }
                continue;
            }
            if have.is_some() {
                continue;
            }
            let mut pack = Pack::default();
            for (c, (s, r)) in model.chunks.iter().zip(self.chunk_ranges.iter_mut()) {
                if *s == slot {
                    *r = pack.emit(&how, &model.base, 0, c.tris.clone(), c.lines.clone(), o);
                }
            }
            if let Some(b) = self.tier_bytes.get_mut(slot - 1) {
                *b = pack.tris.len() + pack.clear.len() + pack.lines.len();
            }
            *have = Some([
                buffer(device, "shaded_tier_tris", &pack.tris),
                buffer(device, "shaded_tier_glass", &pack.clear),
                buffer(device, "shaded_tier_lines", &pack.lines),
            ]);
        }
    }

    /// The vertex buffer of `kind` (0 opaque, 1 see-through, 2 lines) in buffer set `slot` (0: the
    /// main ones, 1 to 3: a tier's).
    fn buffer_of(&self, slot: usize, kind: usize) -> Option<&Buffer> {
        match slot {
            0 => [&self.tris, &self.clear_tris, &self.lines].get(kind).and_then(|b| b.as_ref()),
            s => self.tier_bufs.get(s - 1).and_then(Option::as_ref).and_then(|set| set.get(kind)).and_then(Option::as_ref),
        }
    }

    /// The streamed cells' triangles and lines (in their own colours, unlit by materials), each
    /// cell's ranges; again only when the stream changed.
    fn upload_stream(&mut self, device: &wgpu::Device, stream: &Stream, style: VisualStyle, bg: Rgb) {
        if self.stream_gen == Some(stream.generation) {
            return;
        }
        let o = self.origin;
        let rel = |p: Vec3| [(p.x - o.x) as f32, (p.y - o.y) as f32, (p.z - o.z) as f32];
        let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges);
        let dark = edge_ink(bg);
        let (mut tris, mut clear, mut lines): (Vec<u8>, Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new(), Vec::new());
        let count = |v: &Vec<u8>| (v.len() as u64 / VERTEX_SIZE) as u32;
        let mut ranges = std::collections::BTreeMap::new();
        for (k, c) in &stream.cells {
            let sc = &c.scene;
            let (t0, c0, l0) = (count(&tris), count(&clear), count(&lines));
            if style != VisualStyle::Wireframe {
                for t in &sc.tris {
                    let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
                    let n = if n.is_finite() { [n.x as f32, n.y as f32, n.z as f32] } else { [0.0, 0.0, 1.0] };
                    let opacity = sc.surfaces.get(t.mat as usize).map_or(1.0, |sm| sm.opacity.clamp(0.0, 1.0));
                    let out = if opacity < 0.99 { &mut clear } else { &mut tris };
                    for (p, uv) in t.p.iter().zip(t.uv) {
                        push_vertex(out, rel(*p), n, t.color, uv, [0.6, 0.0, -1.0, opacity], [-1.0; 4]);
                    }
                }
            }
            for l in sc.lines.iter().filter(|l| !l.edge || edges) {
                let c = match (l.edge && style == VisualStyle::ShadedEdges, l.faint) {
                    (true, true) => cadaddy_render::shade::faint_ink(dark, bg),
                    (true, false) => dark,
                    _ => l.color,
                };
                push_vertex(&mut lines, rel(l.a), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0], [-1.0; 4]);
                push_vertex(&mut lines, rel(l.b), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0], [-1.0; 4]);
            }
            ranges.insert(*k, [t0..count(&tris), c0..count(&clear), l0..count(&lines)]);
        }
        self.stream_bytes = tris.len() + clear.len() + lines.len();
        self.stream_bufs = [
            buffer(device, "shaded_stream_tris", &tris),
            buffer(device, "shaded_stream_glass", &clear),
            buffer(device, "shaded_stream_lines", &lines),
        ];
        self.stream_ranges = ranges;
        self.stream_gen = Some(stream.generation);
    }

    /// The frame's placements: the identity, then each visible group's instances in order
    /// (relative to the origin); the groups' instance ranges.
    fn placements(&self, model: &Model3, vis: &model3::Visible) -> (Vec<u8>, Vec<std::ops::Range<u32>>) {
        let o = self.origin;
        let mut out: Vec<u8> = Vec::with_capacity((1 + vis.instances.iter().map(|g| g.2.len()).sum::<usize>()) * INSTANCE_SIZE as usize);
        let mut put = |rows: [[f64; 4]; 3]| {
            for r in rows {
                for x in r {
                    out.extend_from_slice(&(x as f32).to_le_bytes());
                }
            }
        };
        put([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        let mut ranges = Vec::with_capacity(vis.instances.len());
        let mut n = 1u32;
        for (_, _, list) in &vis.instances {
            let start = n;
            for i in list {
                let Some(inst) = model.instances.get(*i as usize) else { continue };
                let m = &inst.m.m;
                put([
                    [m[0][0], m[0][1], m[0][2], m[0][3] - o.x],
                    [m[1][0], m[1][1], m[1][2], m[1][3] - o.y],
                    [m[2][0], m[2][1], m[2][2], m[2][3] - o.z],
                ]);
                n += 1;
            }
            ranges.push(start..n);
        }
        (out, ranges)
    }

    /// The frame's uniform block, and the sun's view for the shadow map (when it shines).
    fn uniforms(&self, v: &ShadeView, style: VisualStyle, bg: Rgb) -> (Vec<u8>, bool) {
        // Orthographic: the view plane's right/up through the view centre across the image;
        // depth from the nearest to the farthest the scene can reach.
        let o = self.origin;
        let hh = v.height / 2.0;
        let hw = hh * f64::from(v.width_px) / f64::from(v.height_px);
        let (cx, cy) = (v.center.x - o.dot(v.right), v.center.y - o.dot(v.up));
        let r = self.radius * 1.05;
        let rows = [
            [v.right.x / hw, v.right.y / hw, v.right.z / hw, -cx / hw],
            [v.up.x / hh, v.up.y / hh, v.up.z / hh, -cy / hh],
            [-v.dir.x / (2.0 * r), -v.dir.y / (2.0 * r), -v.dir.z / (2.0 * r), 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let mut u: Vec<u8> = Vec::with_capacity(UNIFORM_SIZE as usize);
        let mut put = |x: f32| u.extend_from_slice(&x.to_le_bytes());
        let mat = |put: &mut dyn FnMut(f32), rows: &[[f64; 4]; 4]| {
            for col in 0..4 {
                for row in rows {
                    put(row.get(col).copied().unwrap_or(0.0) as f32);
                }
            }
        };
        mat(&mut put, &rows);
        let light = (v.dir * 0.6 + v.up * 0.6 - v.right * 0.35).normalized();
        let lit = self.lighting.as_ref();
        let f = |c: u8| f32::from(c) / 255.0;
        let hidden = if style == VisualStyle::Hidden { 1.0f32 } else { 0.0 };
        let spec = if style == VisualStyle::Realistic { 1.0f32 } else { 0.0 };
        for x in [
            light.x as f32,
            light.y as f32,
            light.z as f32,
            0.0,
            v.dir.x as f32,
            v.dir.y as f32,
            v.dir.z as f32,
            0.0,
            f(bg.0),
            f(bg.1),
            f(bg.2),
            spec,
        ] {
            put(x);
        }
        let lights = lit.map_or(&[][..], |l| &l.lights[..]);
        for x in [hidden, if lit.is_some() { 1.0 } else { 0.0 }, lit.map_or(1.0, |l| l.exposure), lights.len().min(16) as f32] {
            put(x);
        }
        put(o.x as f32);
        put(o.y as f32);
        put(o.z as f32);
        put(lit.map_or(1.0, |l| l.unit_m as f32));
        let sun = lit.and_then(|l| l.sun);
        let (sd, srgb) = sun.unwrap_or((Vec3::Z, [0.0; 3]));
        for x in [sd.x as f32, sd.y as f32, sd.z as f32, if sun.is_some() { 1.0 } else { 0.0 }, srgb[0], srgb[1], srgb[2], 0.0] {
            put(x);
        }
        let sky = lit.map(|l| l.sky);
        for c in [sky.map_or([0.0; 3], |s| s.zenith), sky.map_or([0.0; 3], |s| s.horizon), sky.map_or([0.0; 3], |s| s.ground)] {
            for x in [c[0], c[1], c[2], 0.0] {
                put(x);
            }
        }
        // The sun's view: an orthographic box round the scene (relative coordinates), depth
        // growing away from the sun.
        let d = sd.normalized();
        let helper = if d.z.abs() < 0.9 { Vec3::Z } else { Vec3::new(1.0, 0.0, 0.0) };
        let su = helper.cross(d).normalized();
        let sv = d.cross(su).normalized();
        let rr = self.radius * 1.02;
        let srows = [
            [su.x / rr, su.y / rr, su.z / rr, 0.0],
            [sv.x / rr, sv.y / rr, sv.z / rr, 0.0],
            [-d.x / (2.0 * rr), -d.y / (2.0 * rr), -d.z / (2.0 * rr), 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ];
        mat(&mut put, &srows);
        let texel_world = 2.0 * rr / f64::from(SHADOW_PX);
        let bias = (2e-3 + texel_world * 1.5 / (2.0 * rr)) as f32;
        for x in [bias, if sun.is_some() { 1.0 } else { 0.0 }, (texel_world * 1.5) as f32, 0.0] {
            put(x);
        }
        // Soft shadows, ambient occlusion and the tone curve (cadaddy_render::ao).
        let tan_half = lit.map_or(0.0, |l| shade::sun_tan_half(l.sun_size));
        let texels = f64::from(SHADOW_PX) / (2.0 * rr);
        let search = (tan_half * 2.0 * self.radius * texels).clamp(1.0, cadaddy_render::ao::SOFT_MAX_TEXELS);
        for x in [tan_half as f32, texels as f32, (2.0 * rr) as f32, search as f32] {
            put(x);
        }
        let px = v.height / f64::from(v.height_px.max(1));
        let reach = lit.map_or(1.0, |l| cadaddy_render::light::ao_reach(l.ao_radius, 2.0 * self.radius));
        let strength = lit.map_or(0.0, |l| l.ao);
        let r_px = cadaddy_render::ao::ao_pixels(reach, px);
        let tone = lit.map_or(0.0, |l| cadaddy_doc::lighting::ToneMap::ALL.iter().position(|t| *t == l.tone).unwrap_or(0) as f32);
        for x in [
            v.right.x as f32,
            v.right.y as f32,
            v.right.z as f32,
            strength,
            v.up.x as f32,
            v.up.y as f32,
            v.up.z as f32,
            r_px as f32,
            px as f32,
            reach as f32,
            (2.0 * r) as f32,
            tone,
        ] {
            put(x);
        }
        for i in 0..16 {
            let l = lights.get(i);
            let kind = l.map_or(1.0, |l| match l.kind {
                cadaddy_doc::lighting::LightKind::Distant => 0.0,
                cadaddy_doc::lighting::LightKind::Point => 1.0,
                cadaddy_doc::lighting::LightKind::Spot => 2.0,
            });
            let pos = l.map_or(Vec3::ZERO, |l| if l.kind == cadaddy_doc::lighting::LightKind::Distant { l.pos } else { l.pos - o });
            let dir = l.map_or(Vec3::Z, |l| l.dir);
            let rgb = l.map_or([0.0; 3], |l| l.rgb);
            let att = l.map_or(2.0, |l| l.attenuation.dxf() as f32);
            for x in [
                pos.x as f32,
                pos.y as f32,
                pos.z as f32,
                kind,
                dir.x as f32,
                dir.y as f32,
                dir.z as f32,
                l.map_or(1.0, |l| l.cos_inner),
                rgb[0],
                rgb[1],
                rgb[2],
                l.map_or(0.0, |l| l.cos_outer),
                att,
                0.0,
                0.0,
                0.0,
            ] {
                put(x);
            }
        }
        // The render environment: fog (distances from the front of the scene, relative to the
        // origin), white balance, background.
        let fog = lit.and_then(|l| l.fog);
        let fc = fog.map_or([0.0; 3], |f| f.color);
        let dir = v.dir.normalized();
        let front = lit.map_or(0.0, |l| cadaddy_render::light::fog_front(l, dir)) - o.dot(dir);
        let wb = lit.map_or([1.0; 3], |l| l.white_balance);
        let (kind, bottom, top) = match lit.map(|l| &l.background) {
            Some(cadaddy_render::light::Background::Solid(c)) => (1.0, *c, *c),
            Some(cadaddy_render::light::Background::Gradient { bottom, top }) => (1.0, *bottom, *top),
            Some(cadaddy_render::light::Background::Image(_)) => (2.0, [0.0; 3], [0.0; 3]),
            _ => (0.0, [0.0; 3], [0.0; 3]),
        };
        for x in [
            fc[0],
            fc[1],
            fc[2],
            if fog.is_some() { 1.0 } else { 0.0 },
            fog.map_or(0.0, |f| f.near),
            fog.map_or(1.0, |f| f.far),
            front as f32,
            0.0,
            wb[0],
            wb[1],
            wb[2],
            0.0,
            kind,
            0.0,
            0.0,
            0.0,
            bottom[0],
            bottom[1],
            bottom[2],
            0.0,
            top[0],
            top[1],
            top[2],
            0.0,
        ] {
            put(x);
        }
        (u, sun.is_some())
    }

    /// Draw into the offscreen target sized like the view: what it shows, nearest first.
    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        v: &ShadeView,
        style: VisualStyle,
        bg: Rgb,
        tier: Tier,
        force: Option<&Force>,
        stream: &Stream,
    ) -> model3::Visible {
        let size = (v.width_px, v.height_px);
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(target(device, size, self.samples));
            self.bind_group = None;
        }
        if self.bind_group.is_none() {
            self.bind_group = self.target.as_ref().and_then(|t| self.group(device, t));
        }
        let Some(model) = self.model.clone() else { return model3::Visible::default() };
        let vis = model.visible_at(v, tier, force);
        // The streamed cells in view: their ranges in the stream buffers (opaque, see-through,
        // lines).
        let cells: Vec<[std::ops::Range<u32>; 3]> = stream.visible(v).iter().filter_map(|k| self.stream_ranges.get(k).cloned()).collect();
        let stream_draws =
            |kind: usize| -> Vec<std::ops::Range<u32>> { cells.iter().filter_map(|r| r.get(kind).cloned()).filter(|r| !r.is_empty()).collect() };
        let (inst, groups) = self.placements(&model, &vis);
        let need = inst.len() as u64;
        if self.instances.is_none() || self.instance_cap < need {
            let cap = need.next_power_of_two().max(INSTANCE_SIZE * 64);
            self.instances = Some(Tracked::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shaded_instances"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })));
            self.instance_cap = cap;
        }
        let (Some(t), Some(group), Some(ib)) = (&self.target, &self.bind_group, &self.instances) else { return vis };
        queue.write_buffer(ib, 0, &inst);
        let (u, sun) = self.uniforms(v, style, bg);
        queue.write_buffer(&self.uniform, 0, &u);
        let lin = |c: u8| f64::from(c) / 255.0;
        let physical = self.lighting.is_some();
        // The draws of one buffer (0 opaque, 1 see-through, 2 lines): the visible chunks with
        // the identity placement, then each group's prototype with its placements (a group's
        // lines only at full detail).
        let draws = |kind: usize, slot: usize| -> Vec<(std::ops::Range<u32>, std::ops::Range<u32>)> {
            let mut d: Vec<(std::ops::Range<u32>, std::ops::Range<u32>)> = vis
                .chunks
                .iter()
                .filter_map(|c| self.chunk_ranges.get(*c))
                .filter(|(s, _)| *s == slot)
                .filter_map(|(_, r)| r.get(kind).cloned())
                .map(|r| (r, 0..1))
                .collect();
            for ((p, lod, _), ir) in vis.instances.iter().zip(&groups).filter(|_| slot == 0) {
                if kind == 2 && *lod != 0 {
                    continue;
                }
                if let Some(r) = self.proto_ranges.get(*p as usize).and_then(|l| l.get(*lod as usize)) {
                    d.push((r[kind].clone(), ir.clone()));
                }
            }
            d.retain(|(r, i)| !r.is_empty() && !i.is_empty());
            d
        };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shaded") });
        // The opaque surfaces' depth, for the ambient occlusion.
        if physical && self.lighting.as_ref().is_some_and(|l| l.ao > 0.0) {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shaded_depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.prepass,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.shadow_group, &[]);
            for slot in 0..4 {
                if let Some(m) = self.buffer_of(slot, 0) {
                    pass.set_pipeline(&self.depth_pipeline);
                    pass.set_vertex_buffer(0, m.buffer.slice(..));
                    pass.set_vertex_buffer(1, ib.slice(..));
                    for (r, i) in draws(0, slot) {
                        pass.draw(r, i);
                    }
                }
            }
            if let Some(m) = &self.stream_bufs[0] {
                pass.set_pipeline(&self.depth_pipeline);
                pass.set_vertex_buffer(0, m.buffer.slice(..));
                pass.set_vertex_buffer(1, ib.slice(..));
                for r in stream_draws(0) {
                    pass.draw(r, 0..1);
                }
            }
        }
        // The sun's shadow map first (from what the view shows).
        if physical && sun {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shaded_shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.shadow_group, &[]);
            for slot in 0..4 {
                if let Some(m) = self.buffer_of(slot, 0) {
                    pass.set_pipeline(&self.shadow_pipeline);
                    pass.set_vertex_buffer(0, m.buffer.slice(..));
                    pass.set_vertex_buffer(1, ib.slice(..));
                    for (r, i) in draws(0, slot) {
                        pass.draw(r, i);
                    }
                }
            }
            if let Some(m) = &self.stream_bufs[0] {
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_vertex_buffer(0, m.buffer.slice(..));
                pass.set_vertex_buffer(1, ib.slice(..));
                for r in stream_draws(0) {
                    pass.draw(r, 0..1);
                }
            }
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shaded"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: t.msaa.as_ref().map_or(&t.view, |m| &**m),
                    depth_slice: None,
                    resolve_target: t.msaa.as_ref().map(|_| &t.view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: lin(bg.0), g: lin(bg.1), b: lin(bg.2), a: 1.0 }),
                        store: if t.msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, group, &[]);
            if physical {
                pass.set_pipeline(&self.bg_pipeline);
                pass.draw(0..3, 0..1);
            }
            for (kind, pipeline) in [(0, &self.tri_pipeline), (2, &self.line_pipeline), (1, &self.clear_pipeline)] {
                for slot in 0..4 {
                    if let Some(m) = self.buffer_of(slot, kind) {
                        pass.set_pipeline(pipeline);
                        pass.set_vertex_buffer(0, m.buffer.slice(..));
                        pass.set_vertex_buffer(1, ib.slice(..));
                        for (r, i) in draws(kind, slot) {
                            pass.draw(r, i);
                        }
                    }
                }
                // The streamed cells of the same kind, placed as they are.
                if let Some(m) = self.stream_bufs.get(kind).and_then(Option::as_ref) {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, m.buffer.slice(..));
                    pass.set_vertex_buffer(1, ib.slice(..));
                    for r in stream_draws(kind) {
                        pass.draw(r, 0..1);
                    }
                }
            }
        }
        queue.submit([enc.finish()]);
        vis
    }

    fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }
}

/// How scenes' vertices are packed: the style and background, whether lit physically (materials,
/// textures, baked light), the texture layers there are.
#[derive(Clone, Copy, Debug)]
struct Packing {
    style: VisualStyle,
    bg: Rgb,
    physical: bool,
    layers: usize,
}

/// Vertex bytes being packed for the three buffers (opaque, see-through, lines).
#[derive(Default)]
struct Pack {
    tris: Vec<u8>,
    clear: Vec<u8>,
    lines: Vec<u8>,
}

impl Pack {
    fn count(v: &[u8]) -> u32 {
        (v.len() as u64 / VERTEX_SIZE) as u32
    }

    /// One scene's triangles and lines in the given ranges (`offset`: its first texture's layer;
    /// `o`: the origin vertices are stored from): their vertex ranges in the three buffers.
    fn emit(
        &mut self,
        how: &Packing,
        sc: &Scene,
        offset: usize,
        tr: std::ops::Range<usize>,
        lr: std::ops::Range<usize>,
        o: Vec3,
    ) -> [std::ops::Range<u32>; 3] {
        let (style, bg, physical) = (how.style, how.bg, how.physical);
        let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges);
        let dark = edge_ink(bg);
        let srgb = |c: f32| (c.max(0.0).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
        let rel = |p: Vec3| [(p.x - o.x) as f32, (p.y - o.y) as f32, (p.z - o.z) as f32];
        let (t0, c0, l0) = (Pack::count(&self.tris), Pack::count(&self.clear), Pack::count(&self.lines));
        let first = tr.start;
        if style != VisualStyle::Wireframe {
            for (k, t) in sc.tris.get(tr).unwrap_or(&[]).iter().enumerate() {
                // Baked light (LIGHTBAKE): the cells of its front and back, per corner.
                let lm = match sc.baked.as_ref().filter(|_| physical).and_then(|b| b.corner_uv(first + k)) {
                    Some([f, b]) => [0, 1, 2].map(|c| [f[c][0], f[c][1], b[c][0], b[c][1]]),
                    None => [[-1.0; 4]; 3],
                };
                let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
                let n = if n.is_finite() { [n.x as f32, n.y as f32, n.z as f32] } else { [0.0, 0.0, 1.0] };
                let surface = sc.surfaces.get(t.mat as usize).filter(|_| physical);
                let (color, m) = match surface {
                    Some(sm) => {
                        let layer = sm.texture.map(|i| i as usize + offset).filter(|i| *i < how.layers).map_or(-1.0, |i| i as f32);
                        (Rgb(srgb(sm.base[0]), srgb(sm.base[1]), srgb(sm.base[2])), [sm.roughness, sm.metallic, layer, sm.opacity])
                    }
                    // Unlit styles: its own colour, see-through as its material is (a
                    // ghosted shell).
                    None => (t.color, [0.6, 0.0, -1.0, sc.surfaces.get(t.mat as usize).map_or(1.0, |sm| sm.opacity.clamp(0.0, 1.0))]),
                };
                let out = if m[3] < 0.99 { &mut self.clear } else { &mut self.tris };
                for ((p, uv), l) in t.p.iter().zip(t.uv).zip(lm) {
                    push_vertex(out, rel(*p), n, color, uv, m, l);
                }
            }
        }
        for l in sc.lines.get(lr).unwrap_or(&[]).iter().filter(|l| !l.edge || edges) {
            let c = match (l.edge && style == VisualStyle::ShadedEdges, l.faint) {
                (true, true) => cadaddy_render::shade::faint_ink(dark, bg),
                (true, false) => dark,
                _ => l.color,
            };
            push_vertex(&mut self.lines, rel(l.a), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0], [-1.0; 4]);
            push_vertex(&mut self.lines, rel(l.b), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0], [-1.0; 4]);
        }
        [t0..Pack::count(&self.tris), c0..Pack::count(&self.clear), l0..Pack::count(&self.lines)]
    }
}

/// The colour of surface edges in Shaded with Edges: near black, which reads on every lit face.
pub(crate) fn edge_ink(_bg: Rgb) -> Rgb {
    Rgb(16, 16, 16)
}

fn push_vertex(out: &mut Vec<u8>, p: [f32; 3], n: [f32; 3], c: Rgb, uv: [f32; 2], m: [f32; 4], lm: [f32; 4]) {
    for x in p.iter().chain(n.iter()) {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&[c.0, c.1, c.2, 255]);
    for x in uv.iter().chain(m.iter()).chain(lm.iter()) {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

fn buffer(device: &wgpu::Device, label: &str, bytes: &[u8]) -> Option<Buffer> {
    // Nothing, or more than the device takes in one buffer (that would be a validation error,
    // which stops the app): none, and what it held isn't drawn.
    if bytes.is_empty() || bytes.len() as u64 > device.limits().max_buffer_size {
        return None;
    }
    let buffer = Tracked::new(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: wgpu::BufferUsages::VERTEX,
    }));
    Some(Buffer { buffer })
}

/// An RGBA texture of `w` × `h` (as stored, not sRGB-decoded), written with `px` when a queue
/// is given (else left white: one texel).
fn image_texture(device: &wgpu::Device, queue: Option<&wgpu::Queue>, w: u32, h: u32, px: &[u8]) -> Tracked<wgpu::Texture> {
    let size = wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 };
    let tex = Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shaded_background"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }));
    if let Some(q) = queue.filter(|_| px.len() as u64 == u64::from(size.width) * u64::from(size.height) * 4) {
        q.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            px,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width * 4), rows_per_image: Some(size.height) },
            size,
        );
    }
    tex
}

fn target(device: &wgpu::Device, size: (u32, u32), samples: u32) -> Target {
    let extent = wgpu::Extent3d { width: size.0.clamp(1, shade::MAX_SIDE), height: size.1.clamp(1, shade::MAX_SIDE), depth_or_array_layers: 1 };
    let tex = |label: &str, format, samples, usage| {
        Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        }))
    };
    let msaa = (samples > 1).then(|| tex("shaded_msaa", FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
    let depth = tex("shaded_depth", DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let prepass = tex("shaded_prepass", DEPTH, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let resolve =
        tex("shaded_frame", FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
    Target {
        size,
        msaa: msaa.map(|m| m.into_view(&wgpu::TextureViewDescriptor::default())),
        depth: depth.into_view(&wgpu::TextureViewDescriptor::default()),
        view: resolve.create_view(&wgpu::TextureViewDescriptor::default()),
        prepass: prepass.into_view(&wgpu::TextureViewDescriptor::default()),
        frame: resolve,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("shader.wgsl")).unwrap_or_else(|e| panic!("{e}"));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{e:?}"));
    }

    /// The live view on the GPU and on the CPU show the same render environment: the same
    /// scene and environment come out with a per-channel mean difference of at most 3/255 over
    /// the background and over fogged and clear surfaces. Two boxes from above (the sun off):
    /// the tall one's top is the front of the scene, the low one's 9 units behind it.
    #[test]
    fn the_gpu_and_the_cpu_show_the_same_environment() {
        use super::*;
        use serde_json::json;
        let mut s = cadaddy_engine::Session::new();
        s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 })).unwrap();
        s.execute("box", &json!({ "corner": [20, 0, 0], "length": 10, "width": 10, "height": 1 })).unwrap();
        s.doc_mut().unwrap().sun.on = false;
        let d = s.doc().unwrap().clone();
        let bg = Rgb(255, 255, 255);
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let model = Arc::new(model3::build_clipped(&d, &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[]));
        let flat = model.flatten();
        let (w, h) = (128u32, 64u32);
        let view = shade::fit(&flat, None, w, h).unwrap();
        // Files the environments use: a lighting map and a 2 × 2 picture.
        let dir = std::env::temp_dir().join(format!("cadaddy-gpuenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hdr = dir.join("sky.hdr");
        let mut f = b"#?RADIANCE\n\n-Y 2 +X 4\n".to_vec();
        f.extend([[200, 180, 120, 129]; 4].concat());
        f.extend([[60, 80, 40, 129]; 4].concat());
        std::fs::write(&hdr, f).unwrap();
        let picture = dir.join("backdrop.png");
        let px: Vec<u8> = [[200u8, 30, 30, 255], [30, 200, 30, 255], [30, 30, 200, 255], [220, 220, 40, 255]].concat();
        image::save_buffer(&picture, &px, 2, 2, image::ColorType::Rgba8).unwrap();
        let envs = [
            json!({ "background": {"kind": "gradient", "bottom": [0.05, 0.1, 0.2], "top": [0.6, 0.7, 0.9]},
                    "fog": {"enabled": true, "color": [0.4, 0.45, 0.5], "near": 6, "far": 12},
                    "ibl": hdr.to_string_lossy(), "exposure": 0.5, "whiteBalance": {"temperature": 4500, "tint": 0.1} }),
            json!({ "background": {"kind": "image", "path": picture.to_string_lossy()}, "ibl": null, "exposure": 0, "whiteBalance": {"temperature": 6500, "tint": 0},
                    "fog": {"enabled": true, "color": [0.7, 0.3, 0.2], "near": 1, "far": 2} }),
        ];
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        for env in envs {
            s.execute("renderenvironment", &env).unwrap();
            let mut lit = model.base.lighting.clone().unwrap();
            cadaddy_engine::cmd::apply_environment(&s, &mut lit).unwrap();
            let cpu = shade::rasterize_lit(&flat, &view, VisualStyle::Realistic, bg, Some(&lit));
            let mut g = Gpu::new(device, 1);
            g.upload(device, queue, &model, VisualStyle::Realistic, bg);
            g.load_tiers(device, &model, Tier::Detail);
            g.set_lighting(device, queue, Some(&lit));
            g.render(device, queue, &view, VisualStyle::Realistic, bg, Tier::Detail, None, &Stream::default());
            // The frame, read back (rows of 512 bytes: 256-aligned).
            let frame = &g.target.as_ref().unwrap().frame;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(w * 4 * h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_texture_to_buffer(
                frame.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            queue.submit([enc.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(10)) }).unwrap();
            let gpu = buffer.slice(..).get_mapped_range().unwrap().to_vec();
            // A 5 × 5 block's mean, per channel.
            let mean = |img: &[u8], (x0, y0): (u32, u32)| {
                [0, 1, 2].map(|c| {
                    (0..5).flat_map(|dy| (0..5).map(move |dx| ((y0 + dy) * w + x0 + dx) as usize * 4 + c)).map(|i| f64::from(img[i])).sum::<f64>()
                        / 25.0
                })
            };
            // The corners (background), the tall box's top (clear) and the low one's (fogged).
            let blocks = [
                ("top left", (1, 1)),
                ("top right", (122, 1)),
                ("bottom left", (1, 58)),
                ("bottom right", (122, 58)),
                ("clear", (21, 30)),
                ("fogged", (103, 30)),
            ];
            for (what, at) in blocks {
                let (a, b) = (mean(&cpu.rgba, at), mean(&gpu, at));
                assert!((0..3).all(|c| (a[c] - b[c]).abs() <= 3.0), "{env}\n{what}: cpu {a:?} gpu {b:?}");
            }
            // And the environment shows: the fogged top isn't the clear one.
            let (clear, fogged) = (mean(&gpu, (21, 30)), mean(&gpu, (103, 30)));
            assert!((0..3).any(|c| (clear[c] - fogged[c]).abs() > 10.0), "{clear:?} {fogged:?}");
        }
    }

    /// The last frame of `g`, read back (RGBA rows; `w * 4` a multiple of 256).
    fn read_frame(g: &super::Gpu, device: &super::wgpu::Device, queue: &super::wgpu::Queue, w: u32, h: u32) -> Vec<u8> {
        use super::wgpu;
        let frame = &g.target.as_ref().unwrap().frame;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(w * 4 * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            frame.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(10)) }).unwrap();
        buffer.slice(..).get_mapped_range().unwrap().to_vec()
    }

    /// On the GPU: at the building tier a room's devices and a conduit stick aren't drawn (only
    /// the slab); at the room tier the devices (instances) and the stick (a streamed cell) are,
    /// and the base buffers hold only what their tier draws.
    #[test]
    fn fidelity_tiers_on_the_gpu() {
        use super::*;
        use cadaddy_doc::{Common, EntityKind, Mesh, MeshFace, MeshKind};
        use cadaddy_geom::Vec2;
        let boxmesh = |lo: Vec3, hi: Vec3| {
            let v = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
            let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
            Mesh {
                vertices: vec![
                    v(false, false, false),
                    v(true, false, false),
                    v(true, true, false),
                    v(false, true, false),
                    v(false, false, true),
                    v(true, false, true),
                    v(true, true, true),
                    v(false, true, true),
                ],
                faces: vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
                kind: MeshKind::Subd,
            }
        };
        let mut d = cadaddy_doc::Drawing::new_metric();
        let mut put = |layer: &str, lo: Vec3, hi: Vec3, rgb: Rgb| {
            let c = Common { layer: layer.into(), color: cadaddy_doc::color::Color::True(rgb), ..Common::default() };
            d.add(&cadaddy_doc::Space::Model, c, EntityKind::Mesh(boxmesh(lo, hi))).unwrap();
        };
        put("SLAB", Vec3::new(0.0, 0.0, -1.0), Vec3::new(100.0, 100.0, 0.0), Rgb(200, 200, 200));
        for (x, y) in [(20.0, 20.0), (5.0, 80.0), (40.0, 5.0), (45.0, 45.0), (10.0, 50.0)] {
            put("DEV", Vec3::new(x, y, 0.0), Vec3::new(x + 10.0, y + 10.0, 3.0), Rgb(250, 30, 30));
        }
        put("COND", Vec3::new(60.0, 58.0, 0.0), Vec3::new(92.0, 64.0, 2.0), Rgb(30, 30, 250));
        put("WALL", Vec3::new(0.0, 96.0, 0.0), Vec3::new(100.0, 97.0, 9.0), Rgb(120, 120, 120));
        let classify = |_: &cadaddy_doc::Drawing, e: &cadaddy_doc::Entity| {
            let tier = match e.common.layer.to_ascii_uppercase().as_str() {
                "SLAB" => Tier::Building,
                "WALL" => Tier::Floor,
                "DEV" | "COND" => Tier::Room,
                _ => return None,
            };
            Some(model3::ElemClass { tier, kind: e.common.layer.to_ascii_lowercase(), level: "L1".into(), group: None, line_from: None })
        };
        let bg = Rgb(255, 255, 255);
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let model =
            Arc::new(model3::build_fid(Arc::new(d), &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[], &classify, model3::FidOptions::default()));
        let f = model.fid.as_ref().unwrap();
        assert_eq!(f.elems.iter().filter(|e| e.held == model3::Held::Instance).count(), 5);
        assert_eq!(f.elems.iter().filter(|e| e.held == model3::Held::Streamed).count(), 1);
        let (w, h) = (128u32, 128u32);
        let view = ShadeView::new(None, Vec2::new(50.0, 50.0), 100.0, w, h);
        // A plan point's pixel.
        let px = |x: f64, y: f64| ((x / 100.0 * f64::from(w)) as u32, ((100.0 - y) / 100.0 * f64::from(h)) as u32);
        let at = |img: &[u8], (x, y): (u32, u32)| {
            let i = (y * w + x) as usize * 4;
            [img[i], img[i + 1], img[i + 2]]
        };
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let mut g = Gpu::new(device, 1);
        let mut stream = Stream::default();
        // The building tier: nothing streamed, the slab only.
        stream.update(&model, &view, Tier::Building, f64::INFINITY);
        g.upload(device, queue, &model, VisualStyle::Shaded, bg);
        g.load_tiers(device, &model, Tier::Building);
        g.upload_stream(device, &stream, VisualStyle::Shaded, bg);
        let vis = g.render(device, queue, &view, VisualStyle::Shaded, bg, Tier::Building, None, &stream);
        assert!(vis.instances.is_empty());
        assert_eq!(g.tier_bytes, [0, 0, 0], "the wall's tier isn't on the GPU");
        let far = read_frame(&g, device, queue, w, h);
        let (dev, stick) = (px(25.0, 25.0), px(76.0, 61.0));
        for (what, c) in [("device", at(&far, dev)), ("stick", at(&far, stick))] {
            assert!(c[0].abs_diff(c[1]) < 30 && c[1].abs_diff(c[2]) < 30, "{what} not drawn at the building tier: {c:?}");
        }
        let marked = model.visible_at(&view, Tier::Building, Some(&Force { kind: "dev".into(), level: None })).markers;
        assert_eq!(marked.len(), 5);
        // The room tier: the devices (instances) and the stick (streamed in) are drawn.
        let step = stream.update(&model, &view, Tier::Room, f64::INFINITY);
        assert!(step.added >= 1);
        g.load_tiers(device, &model, Tier::Room);
        g.upload_stream(device, &stream, VisualStyle::Shaded, bg);
        let vis = g.render(device, queue, &view, VisualStyle::Shaded, bg, Tier::Room, None, &stream);
        assert_eq!(vis.instances.iter().map(|x| x.2.len()).sum::<usize>(), 5);
        let near = read_frame(&g, device, queue, w, h);
        let c = at(&near, dev);
        assert!(c[0] > c[1].saturating_add(60), "a device at the room tier: {c:?}");
        let c = at(&near, stick);
        assert!(c[2] > c[0].saturating_add(60), "the streamed stick at the room tier: {c:?}");
        assert!(g.stream_bytes > 0);
        assert!(g.tier_bytes[0] > 0, "the floor tier (the wall) loaded");
        // Back out: the stream lets the stick go, the floor tier's buffers go, the GPU follows.
        stream.update(&model, &view, Tier::Building, f64::INFINITY);
        g.upload_stream(device, &stream, VisualStyle::Shaded, bg);
        g.load_tiers(device, &model, Tier::Building);
        assert_eq!(g.stream_bytes, 0);
        assert_eq!(g.tier_bytes, [0, 0, 0]);
        g.render(device, queue, &view, VisualStyle::Shaded, bg, Tier::Building, None, &stream);
        let again = read_frame(&g, device, queue, w, h);
        assert_eq!(at(&again, dev), at(&far, dev), "back to the building tier's picture");
    }

    /// The hotel-scale numbers on this machine's wgpu (a benchmark, run by hand):
    /// `CADADDY_FIDELITY_BENCH=model.cdy cargo test -p cadaddy-ui-egui --lib fidelity_bench -- --ignored --nocapture`.
    /// The whole building at each tier, and without fidelity: build, upload and frame times, the
    /// buffers' bytes; then a zoom into the room round one receptacle: the time until it is drawn.
    #[test]
    #[ignore = "a benchmark: set CADADDY_FIDELITY_BENCH to a built model"]
    fn fidelity_bench() {
        use super::*;
        let Ok(path) = std::env::var("CADADDY_FIDELITY_BENCH") else { return };
        let (w, h) = (1600u32, 1000u32);
        let doc = Arc::new(cadaddy_io::cdy::read_cdy(&std::fs::read(&path).unwrap()).unwrap());
        let md = Arc::new(cadaddy_engine::cmd::bimlook::display(&doc).unwrap());
        let ext = md.extents(&cadaddy_doc::Space::Model);
        let tol = ext.width().max(ext.height()) / 1500.0;
        let bg = Rgb(255, 255, 255);
        let upf = cadaddy_engine::cmd::fidelity::units_per_foot(&md);
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let wait = || {
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(120)) }).unwrap();
        };
        let proj = cadaddy_geom::view3::Projection::new(Vec3::new(-1.0, -1.0, 1.0));
        let style = VisualStyle::ShadedEdges;
        let mut out = serde_json::Map::new();
        for fid in [false, true] {
            let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
            let t0 = web_time::Instant::now();
            let model = Arc::new(build3(&md, fid, tol, bg, &p, &t, &[]));
            let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let whole = shade::fit_bounds(&model.bounds, proj, w, h).unwrap();
            let tiers: Vec<Tier> = if fid { Tier::ALL.to_vec() } else { vec![Tier::Detail] };
            let (lt, ll) = model.loaded_at(Tier::Detail);
            let need = (lt * 3 + ll * 2) as u64 * VERTEX_SIZE;
            if !fid && need > device.limits().max_buffer_size {
                out.insert(
                    "without".into(),
                    serde_json::json!({
                        "build_ms": build_ms.round(), "tris": model.total_tris(), "model_mb": (model.bytes() as f64 / 1048576.0).round(),
                        "gpu": format!("needs {} MB of vertex buffers, over this device's {} MB buffer limit: not drawn", need >> 20, device.limits().max_buffer_size >> 20),
                    }),
                );
                continue;
            }
            for tier in tiers {
                let mut g = Gpu::new(device, 4);
                let mut stream = Stream::default();
                let t1 = web_time::Instant::now();
                stream.update(&model, &whole, tier, f64::INFINITY);
                g.upload(device, queue, &model, style, bg);
                g.load_tiers(device, &model, tier);
                g.upload_stream(device, &stream, style, bg);
                let vis = g.render(device, queue, &whole, style, bg, tier, None, &stream);
                wait();
                let first_ms = t1.elapsed().as_secs_f64() * 1000.0;
                let n = 5;
                let t2 = web_time::Instant::now();
                for _ in 0..n {
                    g.render(device, queue, &whole, style, bg, tier, None, &stream);
                    wait();
                }
                let frame_ms = t2.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
                let key = if fid { tier.name().to_string() } else { "without".to_string() };
                out.insert(
                    key,
                    serde_json::json!({
                        "build_ms": build_ms.round(), "upload_and_first_frame_ms": first_ms.round(), "frame_ms": (frame_ms * 10.0).round() / 10.0,
                        "fps": (1000.0 / frame_ms * 10.0).round() / 10.0, "tris_drawn": vis.tris + stream.tris(),
                        "gpu_buffers_mb": ((g.bytes + g.tier_bytes.iter().sum::<usize>() + g.stream_bytes) as f64 / 1048576.0).round(), "model_mb": ((model.bytes() + stream.bytes) as f64 / 1048576.0).round(),
                    }),
                );
            }
            if !fid {
                continue;
            }
            // Zoom into the room round the middle receptacle, from the whole building.
            let f = model.fid.as_ref().unwrap();
            let recs: Vec<&model3::Elem> = f.elems.iter().filter(|e| f.kind_of(e).contains("receptacle")).collect();
            let Some(r) = recs.get(recs.len() / 2) else { continue };
            let pad = Vec3::new(10.0 * upf, 10.0 * upf, 4.0 * upf);
            let b = model3::Aabb { lo: r.bounds.lo - pad, hi: r.bounds.hi + pad };
            let room = shade::fit_bounds(&b, proj, w, h).unwrap();
            let auto = cadaddy_engine::cmd::fidelity::Rules::of(&md).0.auto.tiers();
            let whole_tier = auto.tier(whole.height / upf, None);
            let tier = auto.tier(room.height / upf, Some(whole_tier));
            let mut g = Gpu::new(device, 4);
            let mut stream = Stream::default();
            g.upload(device, queue, &model, style, bg);
            g.load_tiers(device, &model, whole_tier);
            g.render(device, queue, &whole, style, bg, whole_tier, None, &stream);
            wait();
            // The wheel passes the floor scale first: a view of the room's floor round it.
            let fb = model3::Aabb {
                lo: r.bounds.lo - Vec3::new(60.0 * upf, 40.0 * upf, 4.0 * upf),
                hi: r.bounds.hi + Vec3::new(60.0 * upf, 40.0 * upf, 4.0 * upf),
            };
            let floor_view = shade::fit_bounds(&fb, proj, w, h).unwrap();
            let floor_tier = auto.tier(floor_view.height / upf, Some(whole_tier));
            let tf = web_time::Instant::now();
            stream.update(&model, &floor_view, floor_tier, STREAM_MS);
            g.load_tiers(device, &model, floor_tier);
            g.upload_stream(device, &stream, style, bg);
            let floor_cpu_ms = tf.elapsed().as_secs_f64() * 1000.0;
            g.render(device, queue, &floor_view, style, bg, floor_tier, None, &stream);
            wait();
            let floor_ms = tf.elapsed().as_secs_f64() * 1000.0;
            // Then the room: the CPU's share (stream, tiers, buffers, what is visible), then the frame.
            let t3 = web_time::Instant::now();
            let step = stream.update(&model, &room, tier, STREAM_MS);
            g.load_tiers(device, &model, tier);
            g.upload_stream(device, &stream, style, bg);
            let vis_only = model.visible_at(&room, tier, None);
            let cpu_ms = t3.elapsed().as_secs_f64() * 1000.0;
            let vis = g.render(device, queue, &room, style, bg, tier, None, &stream);
            wait();
            let zoom_ms = t3.elapsed().as_secs_f64() * 1000.0;
            // The room's receptacles (instances) drawn on that first frame.
            let near: Vec<&&model3::Elem> =
                recs.iter().filter(|e| model3::in_view(&e.bounds, &room) && (e.bounds.center() - r.bounds.center()).len() < 12.0 * upf).collect();
            let drawn = near
                .iter()
                .filter(|e| {
                    vis.instances.iter().any(|(_, _, l)| {
                        l.iter().filter_map(|k| model.instances.get(*k as usize)).any(|x| (x.bounds.center() - e.bounds.center()).len() < 1e-6)
                    })
                })
                .count();
            out.insert(
                "room".into(),
                serde_json::json!({
                    "height_ft": (room.height / upf).round(), "tier": tier.name(), "from": floor_tier.name(), "floor_view_ft": (floor_view.height / upf).round(),
                    "floor_step_cpu_ms": floor_cpu_ms.round(), "floor_step_ms": floor_ms.round(),
                    "stream_cells": step.added, "stream_ms": step.ms.round(), "stream_pending": step.pending,
                    "cpu_ms": (cpu_ms * 10.0).round() / 10.0, "zoom_to_drawn_ms": zoom_ms.round(), "tris_drawn": vis.tris + stream.tris(),
                    "instances_drawn": vis.instances.iter().map(|x| x.2.len()).sum::<usize>(), "receptacles_within_12ft": near.len(), "receptacles_drawn_first_frame": drawn,
                    "visible_tris": vis_only.tris,
                }),
            );
        }
        println!("FIDELITY_BENCH {}", serde_json::Value::Object(out));
    }
}
