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
use crate::gpucaps::Quality;

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

/// Everything a GPU frame's pixels come from: the model, the style and background, the view and
/// the target's size and samples, the fidelity tier and marks, the streamed cells held and the
/// lighting. Two frames with equal keys are the same picture.
#[derive(Clone, Debug, PartialEq)]
struct FrameKey {
    scene: SceneKey,
    style: VisualStyle,
    bg: [u8; 3],
    /// The view's direction, right, up, centre and height, bit for bit.
    view: [u64; 12],
    size: (u32, u32),
    samples: u32,
    tier: Tier,
    force: Option<Force>,
    stream: u64,
    lighting: Option<cadaddy_render::light::Lighting>,
    quality: Quality,
}

impl FrameKey {
    #[allow(clippy::too_many_arguments)]
    fn new(
        scene: SceneKey,
        style: VisualStyle,
        bg: Rgb,
        v: &ShadeView,
        samples: u32,
        tier: Tier,
        force: Option<Force>,
        stream: u64,
        lighting: Option<cadaddy_render::light::Lighting>,
        quality: Quality,
    ) -> FrameKey {
        let view =
            [v.dir.x, v.dir.y, v.dir.z, v.right.x, v.right.y, v.right.z, v.up.x, v.up.y, v.up.z, v.center.x, v.center.y, v.height].map(f64::to_bits);
        FrameKey { scene, style, bg: [bg.0, bg.1, bg.2], view, size: (v.width_px, v.height_px), samples, tier, force, stream, lighting, quality }
    }
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
    /// Of the GPU bytes: the vertex buffers' (with their second streams) and the index
    /// buffers' held now (main, tiers, streamed cells); the last model upload's milliseconds.
    pub vertex_bytes: usize,
    pub index_bytes: usize,
    pub upload_ms: f64,
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
    /// Frames the GPU drew, and frames shown from the last one's picture because nothing it
    /// was drawn from had changed.
    pub frames_rendered: u64,
    pub frames_reused: u64,
    /// The lighting quality drawn ([`crate::gpucaps::Quality`]); the times the last drawn frame
    /// drew the scene (the main pass, the shadow map, the ambient occlusion's depth), and frames
    /// whose shadow map was kept from an earlier one.
    pub quality: Option<&'static str>,
    pub scene_passes: u32,
    pub shadows_kept: u64,
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
    /// When the GPU last drew a frame, and while frames follow one another (a moving view): how
    /// many have, and their running average interval in milliseconds.
    drawn_at: Option<web_time::Instant>,
    moving: Option<(u32, f64)>,
    /// The section box and coordination mode of the drawing (for the state they were read from).
    sbox_for: Option<((usize, u64), cadaddy_engine::cmd::coordination::CoordRenderState)>,
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
    let mut step_down: Option<f64> = None;
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
        let g = st.gpu.get_or_insert_with(|| {
            let relative = rs.adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::BASE_VERTEX);
            Gpu::new(&rs.device, caps.msaa, relative)
        });
        // The last frame's picture stands while nothing it was drawn from has changed (the
        // pointer moving over a still view, overlays, a hover): no upload, cull or pass.
        let frame = FrameKey::new(key, style, bg, &view, caps.msaa, tier, force.clone(), st.stream.generation, lit.clone(), caps.quality);
        g.quality = caps.quality;
        let reused = g.shows(&frame);
        st.stats.quality = Some(caps.quality.name());
        if reused {
            st.stats.frames_reused += 1;
        } else {
            let up = Some((key, style, [bg.0, bg.1, bg.2]));
            if st.uploaded != up {
                g.upload(&rs.device, &rs.queue, &model, style, bg);
                g.stream_gen = None;
                st.uploaded = up;
                st.stats.upload_ms = g.upload_ms;
            }
            g.load_tiers(&rs.device, &rs.queue, &model, tier);
            g.upload_stream(&rs.device, &rs.queue, &st.stream, style, bg);
            g.set_lighting(&rs.device, &rs.queue, lit.as_ref());
            let t0 = web_time::Instant::now();
            let vis = g.render(&rs.device, &rs.queue, &view, style, bg, tier, force.as_ref(), &st.stream);
            st.stats.encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
            st.stats.frames_rendered += 1;
            // Frames drawn one after another (a moving view): how long each takes, lately.
            let now = web_time::Instant::now();
            match st.drawn_at.map(|at| now.duration_since(at).as_secs_f64() * 1000.0) {
                Some(dt) if dt < MOVING_GAP_MS => st.moving = Some(st.moving.map_or((1, dt), |(n, avg)| (n + 1, avg * 0.8 + dt * 0.2))),
                _ => st.moving = None,
            }
            st.drawn_at = Some(now);
            st.stats.scene_passes = g.scene_passes;
            st.stats.shadows_kept = g.shadows_kept;
            g.last = Some((frame, vis));
        }
        if let Some((_, vis)) = &g.last {
            st.stats.markers = vis.markers.len();
            markers = vis.markers.clone();
            st.stats.tris_drawn = vis.tris;
            st.stats.instances_drawn = vis.instances.iter().map(|g| g.2.len()).sum();
            st.stats.chunks_drawn = vis.chunks.len();
        }
        // Buffers and textures, plus the render targets (4× colour and depth, the resolved image,
        // the pre-pass depth) and the shadow map.
        let (tw, th) = (u64::from(w), u64::from(h));
        st.stats.gpu_bytes = g.bytes
            + g.tier_bytes.iter().sum::<usize>()
            + g.stream_bytes
            + (tw * th * (u64::from(g.samples) * 8 + 8)) as usize
            + (u64::from(SHADOW_PX) * u64::from(SHADOW_PX) * 4) as usize;
        (st.stats.vertex_bytes, st.stats.index_bytes) = g.buffer_bytes();
        // A view that stays slow while it moves draws less lighting from then on (never a level
        // someone asked for, never during a benchmark).
        step_down = st.moving.filter(|(n, avg)| *n >= SLOW_FRAMES && *avg > SLOW_MS && st.bench.is_none()).map(|(_, avg)| avg);
        if let Some(tv) = g.view().filter(|_| !reused || st.texture.is_none()) {
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
    if let Some(avg) = step_down
        && !app.gpu_caps.quality_forced
        && !app.gpu_caps.software
        && let Some(lower) = app.gpu_caps.quality.lower()
    {
        app.gpu_caps.quality = lower;
        app.canvas.shade.moving = None;
        let note = format!("the 3D view was slow ({avg:.0} ms a frame): lighting stepped down to {}", lower.name());
        app.set_status(format!("3D graphics: {note}."));
        app.gpu_caps.notes.push(note);
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
/// Frames drawn closer together than this follow one another (the view is moving); this many of
/// them averaging longer than this each is a slow view: its lighting steps down a level.
const MOVING_GAP_MS: f64 = 400.0;
const SLOW_FRAMES: u32 = 30;
const SLOW_MS: f64 = 50.0;

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
    // The section box is read out of the building spec, which is parsed whole to find it: on a
    // hotel-size model that is most of a frame, so it is read once per state of the drawing.
    let doc_key = (Arc::as_ptr(&st.doc) as usize, st.revision);
    let crs = match &app.canvas.shade.sbox_for {
        Some((k, crs)) if *k == doc_key => crs.clone(),
        _ => {
            let crs = coord::coord_render_state(&st.doc);
            app.canvas.shade.sbox_for = Some((doc_key, crs.clone()));
            crs
        }
    };
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
/// A triangle's corner: position (3 × f32, from the scene's origin) + normal (2 × snorm16,
/// octahedral) + sRGB colour (4 × u8) + material (u32: its row in the material table). Equal
/// corners are welded into one vertex and triangles drawn from index buffers.
const VERTEX_SIZE: u64 = 24;
/// A line's end: position (3 × f32) + sRGB colour (4 × u8).
const LINE_VERTEX_SIZE: u64 = 16;
/// A corner's second stream, only in the buffers of a physically lit view whose model has
/// textures or baked light: texture coordinates (2 × f32) + lightmap coordinates of its front and
/// back (4 × f32; negative: none).
const EXTRA_SIZE: u64 = 24;
/// The material table (roughness, metallic, texture layer, opacity: 4 × f32 a row) is a texture
/// this many rows across; at most this many rows (past them a corner takes the plain row 0).
const MATERIAL_ROW: u32 = 256;
const MAX_MATERIALS: usize = 65_536;
/// A run of corners with fewer vertices than this is drawn with 16-bit indices (0xFFFF unused).
const U16_VERTICES: u32 = 0xFFFF;
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

/// One buffer's vertices on the GPU (opaque, see-through or lines), their second stream when the
/// packing has one, and the indices that draw them: 16-bit for the runs that fit, 32-bit for the
/// rest. Their bytes: vertices (with the second stream) and indices.
struct Buffer {
    buffer: Tracked,
    extra: Option<Tracked>,
    idx16: Option<Tracked>,
    idx32: Option<Tracked>,
    vertex_bytes: usize,
    index_bytes: usize,
}

/// A run of indexed draws in one buffer: its indices (in the 16- or 32-bit index buffer) and the
/// vertex they count from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct Span {
    wide: bool,
    idx: std::ops::Range<u32>,
    base: i32,
}

impl Span {
    fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }
}

/// The shaded view's GPU renderer.
struct Gpu {
    tri_pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    /// The same for buffers with a second stream (texture and lightmap coordinates).
    tri_x_pipeline: wgpu::RenderPipeline,
    clear_x_pipeline: wgpu::RenderPipeline,
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
    /// The draws in the three buffers (opaque, see-through, lines): each base chunk's buffers
    /// (0: the main ones; 1 to 3: its fidelity tier's) and its spans there, and each
    /// prototype's at each level of detail.
    chunk_ranges: Vec<(usize, [Span; 3])>,
    proto_ranges: Vec<[[Span; 3]; 3]>,
    /// The floor, room and detail tiers' chunks (opaque, see-through, lines), loaded while the view
    /// shows the tier; their bytes; how the main upload packed (the tiers pack alike).
    tier_bufs: [Option<[Option<Buffer>; 3]>; 3],
    tier_bytes: [usize; 3],
    packing: Option<Packing>,
    /// The streamed cells' triangles and lines (opaque, see-through, lines), each cell's spans in
    /// them, and the stream generation they hold.
    stream_bufs: [Option<Buffer>; 3],
    stream_ranges: std::collections::BTreeMap<(u8, u32), [Span; 3]>,
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
    /// What the target shows: the frame drawn last and what it drew.
    last: Option<(FrameKey, model3::Visible)>,
    /// The lighting quality frames are drawn at.
    quality: Quality,
    /// What the shadow map holds (a hash of the sun's view and of everything drawn into it):
    /// a frame that would draw the same into it keeps it. The times the last frame drew the
    /// scene, and frames that kept the shadow map.
    shadow_shows: Option<u64>,
    scene_passes: u32,
    shadows_kept: u64,
    target: Option<Target>,
    /// Samples per pixel (see [`crate::gpucaps`]).
    samples: u32,
    /// The materials the corners name, and the texture holding them (and the rows it holds).
    materials: Materials,
    mat_view: Option<TrackedView>,
    mat_rows: usize,
    /// Index runs count from their own first vertex (the device takes a base vertex); else
    /// indices count from the buffer's start (WebGL2).
    relative: bool,
    /// Corners are welded (off only to compare pictures in the tests).
    weld: bool,
    /// Milliseconds the last upload of the model's buffers took (packing and copying).
    upload_ms: f64,
}

/// The materials corners name (roughness, metallic, texture layer, opacity), each once: the plain
/// one (0.6, 0, none, opaque) is row 0.
struct Materials {
    rows: Vec<[f32; 4]>,
    ids: std::collections::HashMap<[u32; 4], u32>,
    /// The last one asked for (triangles in a row mostly share theirs).
    last: ([u32; 4], u32),
}

impl Default for Materials {
    fn default() -> Self {
        let plain = [0.6f32, 0.0, -1.0, 1.0].map(f32::to_bits);
        Materials { rows: vec![plain.map(f32::from_bits)], ids: std::iter::once((plain, 0)).collect(), last: (plain, 0) }
    }
}

impl Materials {
    /// The row of material `m`, added when new (a full table gives the plain row).
    fn id(&mut self, m: [f32; 4]) -> u32 {
        let key = m.map(f32::to_bits);
        if self.last.0 == key {
            return self.last.1;
        }
        if let Some(i) = self.ids.get(&key) {
            self.last = (key, *i);
            return *i;
        }
        if self.rows.len() >= MAX_MATERIALS {
            return 0;
        }
        let i = self.rows.len() as u32;
        self.rows.push(m);
        self.ids.insert(key, i);
        self.last = (key, i);
        i
    }
}

impl Gpu {
    /// The renderer for `samples` per pixel; `relative`: the device takes a base vertex in
    /// indexed draws.
    fn new(device: &wgpu::Device, samples: u32, relative: bool) -> Gpu {
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
                entry(
                    10,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
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
        // Slot 0 the vertices, 1 the instance placements, 2 (in the buffers that have one) the
        // second stream.
        const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm16x2, 2 => Unorm8x4, 4 => Uint32];
        const POS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
        const LINE: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 2 => Unorm8x4];
        const EXTRA: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![3 => Float32x2, 8 => Float32x4];
        const INST: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let layout_of = |array_stride: u64, attributes: &'static [wgpu::VertexAttribute]| {
            Some(wgpu::VertexBufferLayout { array_stride, step_mode: wgpu::VertexStepMode::Vertex, attributes })
        };
        let inst = Some(wgpu::VertexBufferLayout { array_stride: INSTANCE_SIZE, step_mode: wgpu::VertexStepMode::Instance, attributes: &INST });
        let vb = [layout_of(VERTEX_SIZE, &ATTRS), inst.clone()];
        let vb_x = [layout_of(VERTEX_SIZE, &ATTRS), inst.clone(), layout_of(EXTRA_SIZE, &EXTRA)];
        let vb_line = [layout_of(LINE_VERTEX_SIZE, &LINE), inst.clone()];
        let vb_pos = [layout_of(VERTEX_SIZE, &POS), inst];
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        topology: wgpu::PrimitiveTopology,
                        write_depth: bool,
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers, compilation_options: Default::default() },
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
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_shadow"), buffers: &vb_pos, compilation_options: Default::default() },
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
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_depth"), buffers: &vb_pos, compilation_options: Default::default() },
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
            tri_pipeline: pipeline("shaded_tris", "vs_tri", "fs_tri", &vb, wgpu::PrimitiveTopology::TriangleList, true, None),
            clear_pipeline: pipeline("shaded_glass", "vs_tri", "fs_tri", &vb, wgpu::PrimitiveTopology::TriangleList, false, alpha),
            tri_x_pipeline: pipeline("shaded_tris_x", "vs_tri_x", "fs_tri", &vb_x, wgpu::PrimitiveTopology::TriangleList, true, None),
            clear_x_pipeline: pipeline("shaded_glass_x", "vs_tri_x", "fs_tri", &vb_x, wgpu::PrimitiveTopology::TriangleList, false, alpha),
            line_pipeline: pipeline("shaded_lines", "vs_line", "fs_line", &vb_line, wgpu::PrimitiveTopology::LineList, false, None),
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
            last: None,
            quality: Quality::Full,
            shadow_shows: None,
            scene_passes: 0,
            shadows_kept: 0,
            target: None,
            samples,
            materials: Materials::default(),
            mat_view: None,
            mat_rows: 0,
            relative,
            weld: true,
            upload_ms: 0.0,
        }
    }

    /// The model's triangles (none for Wireframe), the lines `style` shows, and the textures:
    /// the base chunk by chunk (the chunks of fidelity tiers finer than `loaded` left out: they
    /// aren't drawn), then each prototype at each level of detail, with the ranges each takes.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, model: &Arc<Model3>, style: VisualStyle, bg: Rgb) {
        let b = model.bounds;
        // Another model (or style): the shadow map's picture is of the old one.
        self.shadow_shows = None;
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
        // Texture and lightmap coordinates only where a texture or baked light is drawn.
        let extra = physical && (!layers.is_empty() || scenes.iter().any(|sc| sc.baked.is_some()));
        let how = Packing { style, bg, physical, layers: layers.len(), extra };
        let t0 = web_time::Instant::now();
        self.materials = Materials::default();
        let mut pack = Pack::new(self.relative, self.weld);
        let o = self.origin;
        // The building tier's chunks (and everything of a model without tiers) here; the finer
        // tiers' in their own buffers, when a view needs them (`load_tiers`).
        let chunk_ranges: Vec<(usize, [Span; 3])> = model
            .chunks
            .iter()
            .map(|c| match c.show.from {
                Tier::Building => (0, pack.emit(&how, &mut self.materials, &model.base, 0, c.tris.clone(), c.lines.clone(), o)),
                t => (t.index(), Default::default()),
            })
            .collect();
        let mut k = 1;
        let proto_ranges: Vec<[[Span; 3]; 3]> = model
            .protos
            .iter()
            .map(|p| {
                let r = [0, 1, 2].map(|l| {
                    let sc = &p.lods[l];
                    let off = offsets.get(k + l).copied().unwrap_or(0);
                    pack.emit(&how, &mut self.materials, sc, off, 0..sc.tris.len(), 0..sc.lines.len(), Vec3::ZERO)
                });
                k += 3;
                r
            })
            .collect();
        self.tris = buffer(device, "shaded_tris", &pack.tris);
        self.clear_tris = buffer(device, "shaded_glass", &pack.clear);
        self.lines = buffer(device, "shaded_lines", &pack.lines);
        self.bytes = pack.bytes() + layers.len().max(1) * (TEX_PX * TEX_PX * 4) as usize;
        self.mat_view = None;
        self.sync_materials(device, queue);
        self.upload_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // The streamed cells' buffers name rows of the table made again and sit from the old
        // origin: they are packed again.
        self.stream_gen = None;
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

    /// The material table on the GPU, again when rows were added since (a new material in a
    /// tier or a streamed cell): `MATERIAL_ROW` rows across, as many lines as it takes.
    fn sync_materials(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let rows = self.materials.rows.len();
        if self.mat_view.is_some() && self.mat_rows == rows {
            return;
        }
        let across = MATERIAL_ROW as usize;
        let lines = rows.div_ceil(across).max(1);
        let mut px: Vec<u8> = Vec::with_capacity(lines * across * 16);
        for m in &self.materials.rows {
            for x in m {
                px.extend_from_slice(&x.to_le_bytes());
            }
        }
        px.resize(lines * across * 16, 0);
        let size = wgpu::Extent3d { width: MATERIAL_ROW, height: lines as u32, depth_or_array_layers: 1 };
        let tex = Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_materials"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &px,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(MATERIAL_ROW * 16), rows_per_image: Some(lines as u32) },
            size,
        );
        self.mat_view = Some(tex.into_view(&wgpu::TextureViewDescriptor::default()));
        self.mat_rows = rows;
        self.bind_group = None;
    }

    /// The group binding the frame's uniform, the model's textures, the shadow map, the baked
    /// light, the background image, target `t`'s pre-pass depth and the material table.
    fn group(&self, device: &wgpu::Device, t: &Target) -> Option<wgpu::BindGroup> {
        let (tex_view, lm_view, mat_view) = (self.tex_view.as_ref()?, self.lm_view.as_ref()?, self.mat_view.as_ref()?);
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
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(mat_view) },
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
    fn load_tiers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, model: &Model3, tier: Tier) {
        let Some(how) = self.packing else { return };
        let o = self.origin;
        for slot in 1..=3usize {
            let t = Tier::from_index(slot);
            let Some(have) = self.tier_bufs.get_mut(slot - 1) else { continue };
            if t > tier {
                if have.take().is_some() {
                    for (s, r) in self.chunk_ranges.iter_mut() {
                        if *s == slot {
                            *r = Default::default();
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
            let mut pack = Pack::new(self.relative, self.weld);
            for (c, (s, r)) in model.chunks.iter().zip(self.chunk_ranges.iter_mut()) {
                if *s == slot {
                    *r = pack.emit(&how, &mut self.materials, &model.base, 0, c.tris.clone(), c.lines.clone(), o);
                }
            }
            if let Some(b) = self.tier_bytes.get_mut(slot - 1) {
                *b = pack.bytes();
            }
            *have = Some([
                buffer(device, "shaded_tier_tris", &pack.tris),
                buffer(device, "shaded_tier_glass", &pack.clear),
                buffer(device, "shaded_tier_lines", &pack.lines),
            ]);
        }
        self.sync_materials(device, queue);
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
    /// cell's spans; again only when the stream changed.
    fn upload_stream(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, stream: &Stream, style: VisualStyle, bg: Rgb) {
        if self.stream_gen == Some(stream.generation) {
            return;
        }
        let how = Packing { style, bg, physical: false, layers: 0, extra: false };
        let mut pack = Pack::new(self.relative, self.weld);
        let mut ranges = std::collections::BTreeMap::new();
        for (k, c) in &stream.cells {
            let sc = &c.scene;
            ranges.insert(*k, pack.emit(&how, &mut self.materials, sc, 0, 0..sc.tris.len(), 0..sc.lines.len(), self.origin));
        }
        self.stream_bytes = pack.bytes();
        self.stream_bufs = [
            buffer(device, "shaded_stream_tris", &pack.tris),
            buffer(device, "shaded_stream_glass", &pack.clear),
            buffer(device, "shaded_stream_lines", &pack.lines),
        ];
        self.stream_ranges = ranges;
        self.stream_gen = Some(stream.generation);
        self.sync_materials(device, queue);
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

    /// The frame's uniform block, and whether the sun's shadow map is drawn and read (the sun
    /// shines and the quality has shadows).
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
        // The lighting quality: point lights, the shadow map, soft shadows and ambient occlusion
        // each only at the levels that draw them.
        let q = self.quality;
        let lights = lit.filter(|_| q.lights()).map_or(&[][..], |l| &l.lights[..]);
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
        let shadows = sun.is_some() && q.shadows();
        for x in [bias, if shadows { 1.0 } else { 0.0 }, (texel_world * 1.5) as f32, 0.0] {
            put(x);
        }
        // Soft shadows, ambient occlusion and the tone curve (cadaddy_render::ao).
        let tan_half = lit.filter(|_| q.soft()).map_or(0.0, |l| shade::sun_tan_half(l.sun_size));
        let texels = f64::from(SHADOW_PX) / (2.0 * rr);
        let search = (tan_half * 2.0 * self.radius * texels).clamp(1.0, cadaddy_render::ao::SOFT_MAX_TEXELS);
        for x in [tan_half as f32, texels as f32, (2.0 * rr) as f32, search as f32] {
            put(x);
        }
        let px = v.height / f64::from(v.height_px.max(1));
        let reach = lit.map_or(1.0, |l| cadaddy_render::light::ao_reach(l.ao_radius, 2.0 * self.radius));
        let strength = lit.filter(|_| q.soft()).map_or(0.0, |l| l.ao);
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
        (u, shadows)
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
        // The target is about to change: what it showed no longer stands (the caller says what
        // it shows once this frame is drawn).
        self.last = None;
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
        let cells: Vec<&[Span; 3]> = stream.visible(v).iter().filter_map(|k| self.stream_ranges.get(k)).collect();
        let stream_draws = |kind: usize| -> Vec<(Span, std::ops::Range<u32>)> {
            cells.iter().filter_map(|r| r.get(kind).cloned()).filter(|r| !r.is_empty()).map(|r| (r, 0..1)).collect()
        };
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
        let draws = |kind: usize, slot: usize| -> Vec<(Span, std::ops::Range<u32>)> {
            let mut d: Vec<(Span, std::ops::Range<u32>)> = vis
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
        let mut passes = 1;
        // The opaque surfaces' depth, for the ambient occlusion.
        if physical && self.quality.soft() && self.lighting.as_ref().is_some_and(|l| l.ao > 0.0) {
            passes += 1;
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
            pass.set_pipeline(&self.depth_pipeline);
            for slot in 0..4 {
                if let Some(m) = self.buffer_of(slot, 0) {
                    draw_spans(&mut pass, m, ib, false, &draws(0, slot));
                }
            }
            if let Some(m) = &self.stream_bufs[0] {
                draw_spans(&mut pass, m, ib, false, &stream_draws(0));
            }
        }
        // The sun's shadow map first (from what the view shows): kept when the sun's view and
        // everything this frame would draw into it are what it was drawn from. The map is fitted
        // to the scene, not the view, so an orbit that shows the same things keeps it.
        let shadow_of = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
            self.lighting.as_ref().and_then(|l| l.sun).map(|(d, _)| bits(d)).hash(&mut h);
            (bits(self.origin), self.radius.to_bits(), self.stream_gen, self.tier_bytes, self.bytes).hash(&mut h);
            for slot in 0..4 {
                for (r, i) in draws(0, slot) {
                    (slot, r).hash(&mut h);
                    // Which placements, not where they sit in this frame's buffer.
                    inst.get(i.start as usize * INSTANCE_SIZE as usize..i.end as usize * INSTANCE_SIZE as usize).hash(&mut h);
                }
            }
            for (r, _) in stream_draws(0) {
                r.hash(&mut h);
            }
            h.finish()
        };
        let shadow_kept = physical && sun && self.shadow_shows == Some(shadow_of);
        if physical && sun && !shadow_kept {
            passes += 1;
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
            pass.set_pipeline(&self.shadow_pipeline);
            for slot in 0..4 {
                if let Some(m) = self.buffer_of(slot, 0) {
                    draw_spans(&mut pass, m, ib, false, &draws(0, slot));
                }
            }
            if let Some(m) = &self.stream_bufs[0] {
                draw_spans(&mut pass, m, ib, false, &stream_draws(0));
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
            // Each buffer with the pipeline for its streams (a second one: texture and lightmap
            // coordinates).
            let pipelines = [
                (0, &self.tri_pipeline, &self.tri_x_pipeline),
                (2, &self.line_pipeline, &self.line_pipeline),
                (1, &self.clear_pipeline, &self.clear_x_pipeline),
            ];
            for (kind, plain, with_extra) in pipelines {
                for slot in 0..4 {
                    if let Some(m) = self.buffer_of(slot, kind) {
                        pass.set_pipeline(if m.extra.is_some() { with_extra } else { plain });
                        draw_spans(&mut pass, m, ib, true, &draws(kind, slot));
                    }
                }
                // The streamed cells of the same kind, placed as they are.
                if let Some(m) = self.stream_bufs.get(kind).and_then(Option::as_ref) {
                    pass.set_pipeline(if m.extra.is_some() { with_extra } else { plain });
                    draw_spans(&mut pass, m, ib, true, &stream_draws(kind));
                }
            }
        }
        queue.submit([enc.finish()]);
        if physical && sun {
            self.shadow_shows = Some(shadow_of);
        }
        if shadow_kept {
            self.shadows_kept += 1;
        }
        self.scene_passes = passes;
        vis
    }

    /// The bytes of the vertex buffers (with their second streams) and of the index buffers held
    /// now: the main ones, the tiers' and the streamed cells'.
    fn buffer_bytes(&self) -> (usize, usize) {
        let main = [&self.tris, &self.clear_tris, &self.lines];
        let tiers = self.tier_bufs.iter().flatten().flatten();
        let all = main.into_iter().flatten().chain(tiers.flatten()).chain(self.stream_bufs.iter().flatten());
        all.fold((0, 0), |(v, i), b| (v + b.vertex_bytes, i + b.index_bytes))
    }

    /// The target already shows the frame `key` asks for.
    fn shows(&self, key: &FrameKey) -> bool {
        self.target.is_some() && self.last.as_ref().is_some_and(|(k, _)| k == key)
    }

    fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }
}

/// How scenes' vertices are packed: the style and background, whether lit physically (materials,
/// textures, baked light), the texture layers there are, and whether corners carry the second
/// stream (texture and lightmap coordinates).
#[derive(Clone, Copy, Debug)]
struct Packing {
    style: VisualStyle,
    bg: Rgb,
    physical: bool,
    layers: usize,
    extra: bool,
}

/// One buffer being packed: its vertices (and their second stream), welded, and the 16- and
/// 32-bit indices that draw them.
#[derive(Default)]
struct Packed {
    verts: Vec<u8>,
    extra: Vec<u8>,
    idx16: Vec<u16>,
    idx32: Vec<u32>,
    /// Vertices so far.
    count: u32,
}

impl Packed {
    fn vertex_bytes(&self) -> usize {
        self.verts.len() + self.extra.len()
    }

    fn index_bytes(&self) -> usize {
        self.idx16.len() * 2 + self.idx32.len() * 4
    }
}

/// The bytes a weld compares: a corner and its second stream.
const KEY: usize = (VERTEX_SIZE + EXTRA_SIZE) as usize;

/// Hashes welding keys quickly, seeded once per process (from the standard library's random
/// state) so a drawing can't be made to collide on purpose.
#[derive(Clone, Copy)]
struct WeldState(u64);

impl Default for WeldState {
    fn default() -> Self {
        static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        WeldState(*SEED.get_or_init(|| {
            use std::hash::{BuildHasher, Hasher};
            std::collections::hash_map::RandomState::new().build_hasher().finish()
        }))
    }
}

impl std::hash::BuildHasher for WeldState {
    type Hasher = WeldHasher;
    fn build_hasher(&self) -> WeldHasher {
        WeldHasher(self.0)
    }
}

struct WeldHasher(u64);

impl WeldHasher {
    fn mix(x: u64) -> u64 {
        let x = (x ^ (x >> 31)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let x = (x ^ (x >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^ (x >> 32)
    }
}

impl std::hash::Hasher for WeldHasher {
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut w = [0u8; 8];
            for (d, b) in w.iter_mut().zip(c) {
                *d = *b;
            }
            self.0 = Self::mix(self.0 ^ u64::from_le_bytes(w)).rotate_left(23);
        }
    }

    fn finish(&self) -> u64 {
        Self::mix(self.0)
    }
}

/// The corners a run has kept, by their bytes.
type Seen = std::collections::HashMap<[u8; KEY], u32, WeldState>;

/// One run of corners (a chunk, a prototype at one level of detail, a streamed cell) going into a
/// buffer: equal corners (every byte the same, so normals across a crease never are) are one
/// vertex.
struct Run {
    /// The run's indices, from its first vertex.
    local: Vec<u32>,
    first: u32,
    added: u32,
}

impl Run {
    /// A run of up to `corners` corners into `out`; `seen` is emptied for it (made anew when a
    /// big run before left it far larger: emptying costs its size).
    fn new(out: &Packed, seen: &mut Seen, corners: usize) -> Run {
        if seen.capacity() > corners.saturating_mul(4).max(4096) {
            *seen = Seen::with_capacity_and_hasher(corners, WeldState::default());
        } else {
            seen.clear();
            seen.reserve(corners);
        }
        Run { local: Vec::with_capacity(corners), first: out.count, added: 0 }
    }

    /// A corner (`v`: its vertex bytes, `x`: its second stream, empty when the buffer has none);
    /// `weld`: equal ones share a vertex.
    fn corner(&mut self, out: &mut Packed, seen: &mut Seen, v: &[u8], x: &[u8], weld: bool) {
        let mut key = [0u8; KEY];
        if let Some(k) = key.get_mut(..v.len()) {
            k.copy_from_slice(v);
        }
        if let Some(k) = key.get_mut(v.len()..v.len() + x.len()) {
            k.copy_from_slice(x);
        }
        let next = self.added;
        let i = if weld { *seen.entry(key).or_insert(next) } else { next };
        if i == next {
            out.verts.extend_from_slice(v);
            out.extra.extend_from_slice(x);
            out.count = out.count.saturating_add(1);
            self.added = self.added.saturating_add(1);
        }
        self.local.push(i);
    }

    /// The run's indices into the buffer: from its own first vertex when draws take a base vertex
    /// (`relative`), else from the buffer's start; 16-bit when its vertices fit.
    fn finish(self, out: &mut Packed, relative: bool) -> Span {
        let (base, offset) = if relative { (i32::try_from(self.first).unwrap_or(i32::MAX), 0) } else { (0, self.first) };
        if offset.saturating_add(self.added) <= U16_VERTICES {
            let start = out.idx16.len() as u32;
            out.idx16.extend(self.local.iter().map(|i| (offset + i) as u16));
            Span { wide: false, idx: start..out.idx16.len() as u32, base }
        } else {
            let start = out.idx32.len() as u32;
            out.idx32.extend(self.local.iter().map(|i| offset.saturating_add(*i)));
            Span { wide: true, idx: start..out.idx32.len() as u32, base }
        }
    }
}

/// Vertex bytes being packed for the three buffers (opaque, see-through, lines), and the corners
/// each one's current run has kept.
struct Pack {
    tris: Packed,
    clear: Packed,
    lines: Packed,
    seen: [Seen; 3],
    relative: bool,
    weld: bool,
}

impl Pack {
    fn new(relative: bool, weld: bool) -> Pack {
        Pack { tris: Packed::default(), clear: Packed::default(), lines: Packed::default(), seen: Default::default(), relative, weld }
    }

    /// The bytes the three buffers take: vertices (with the second stream) and indices.
    fn bytes(&self) -> usize {
        [&self.tris, &self.clear, &self.lines].iter().map(|p| p.vertex_bytes() + p.index_bytes()).sum()
    }

    /// One scene's triangles and lines in the given ranges (`offset`: its first texture's layer;
    /// `o`: the origin vertices are stored from), their materials named in `mats`: their spans in
    /// the three buffers.
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        how: &Packing,
        mats: &mut Materials,
        sc: &Scene,
        offset: usize,
        tr: std::ops::Range<usize>,
        lr: std::ops::Range<usize>,
        o: Vec3,
    ) -> [Span; 3] {
        let (style, bg, physical) = (how.style, how.bg, how.physical);
        let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges);
        let dark = edge_ink(bg);
        let srgb = |c: f32| (c.max(0.0).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
        let rel = |p: Vec3| [(p.x - o.x) as f32, (p.y - o.y) as f32, (p.z - o.z) as f32];
        let weld = self.weld;
        let tris = sc.tris.get(tr.clone()).unwrap_or(&[]);
        let lines = sc.lines.get(lr).unwrap_or(&[]);
        let [seen_tris, seen_clear, seen_lines] = &mut self.seen;
        let (mut opaque, mut glass, mut ends) = (
            Run::new(&self.tris, seen_tris, tris.len() * 3),
            Run::new(&self.clear, seen_clear, 0),
            Run::new(&self.lines, seen_lines, lines.len() * 2),
        );
        let first = tr.start;
        if style != VisualStyle::Wireframe {
            for (k, t) in tris.iter().enumerate() {
                // Baked light (LIGHTBAKE): the cells of its front and back, per corner.
                let lm = match sc.baked.as_ref().filter(|_| physical).and_then(|b| b.corner_uv(first + k)) {
                    Some([f, b]) => [0, 1, 2].map(|c| [f[c][0], f[c][1], b[c][0], b[c][1]]),
                    None => [[-1.0; 4]; 3],
                };
                let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
                let n = if n.is_finite() { oct([n.x as f32, n.y as f32, n.z as f32]) } else { [0, 0] };
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
                let id = mats.id(m);
                // Texture coordinates only matter under a texture: else all alike, so corners weld.
                let textured = m[2] >= 0.0;
                let (out, run, seen) =
                    if m[3] < 0.99 { (&mut self.clear, &mut glass, &mut *seen_clear) } else { (&mut self.tris, &mut opaque, &mut *seen_tris) };
                for ((p, uv), l) in t.p.iter().zip(t.uv).zip(lm) {
                    let v = corner_bytes(rel(*p), n, color, id);
                    if how.extra {
                        run.corner(out, seen, &v, &extra_bytes(if textured { uv } else { [0.0; 2] }, l), weld);
                    } else {
                        run.corner(out, seen, &v, &[], weld);
                    }
                }
            }
        }
        for l in lines.iter().filter(|l| !l.edge || edges) {
            let c = match (l.edge && style == VisualStyle::ShadedEdges, l.faint) {
                (true, true) => cadaddy_render::shade::faint_ink(dark, bg),
                (true, false) => dark,
                _ => l.color,
            };
            ends.corner(&mut self.lines, seen_lines, &line_bytes(rel(l.a), c), &[], weld);
            ends.corner(&mut self.lines, seen_lines, &line_bytes(rel(l.b), c), &[], weld);
        }
        let relative = self.relative;
        [opaque.finish(&mut self.tris, relative), glass.finish(&mut self.clear, relative), ends.finish(&mut self.lines, relative)]
    }
}

/// The colour of surface edges in Shaded with Edges: near black, which reads on every lit face.
pub(crate) fn edge_ink(_bg: Rgb) -> Rgb {
    Rgb(16, 16, 16)
}

/// A unit normal as two signed 16-bit numbers: the sphere folded onto a square (octahedral), the
/// shader's `unoct` unfolds it. No normal: +Z.
fn oct(n: [f32; 3]) -> [i16; 2] {
    let s = n[0].abs() + n[1].abs() + n[2].abs();
    if !(s.is_finite() && s > 1e-30) {
        return [0, 0];
    }
    let (x, y) = (n[0] / s, n[1] / s);
    let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
    let (x, y) = if n[2] < 0.0 { ((1.0 - y.abs()) * sign(x), (1.0 - x.abs()) * sign(y)) } else { (x, y) };
    let q = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    [q(x), q(y)]
}

/// `b` written into `out` from byte `at` (nothing past its end).
fn put<const N: usize, const M: usize>(out: &mut [u8; M], at: usize, b: [u8; N]) {
    if let Some(dst) = out.get_mut(at..at + N) {
        dst.copy_from_slice(&b);
    }
}

/// A triangle corner's bytes (see [`VERTEX_SIZE`]).
fn corner_bytes(p: [f32; 3], n: [i16; 2], c: Rgb, material: u32) -> [u8; VERTEX_SIZE as usize] {
    let mut out = [0u8; VERTEX_SIZE as usize];
    for (k, x) in p.into_iter().enumerate() {
        put(&mut out, k * 4, x.to_le_bytes());
    }
    put(&mut out, 12, n[0].to_le_bytes());
    put(&mut out, 14, n[1].to_le_bytes());
    put(&mut out, 16, [c.0, c.1, c.2, 255]);
    put(&mut out, 20, material.to_le_bytes());
    out
}

/// A corner's second stream (see [`EXTRA_SIZE`]).
fn extra_bytes(uv: [f32; 2], lm: [f32; 4]) -> [u8; EXTRA_SIZE as usize] {
    let mut out = [0u8; EXTRA_SIZE as usize];
    for (k, x) in uv.into_iter().chain(lm).enumerate() {
        put(&mut out, k * 4, x.to_le_bytes());
    }
    out
}

/// A line end's bytes (see [`LINE_VERTEX_SIZE`]).
fn line_bytes(p: [f32; 3], c: Rgb) -> [u8; LINE_VERTEX_SIZE as usize] {
    let mut out = [0u8; LINE_VERTEX_SIZE as usize];
    for (k, x) in p.into_iter().enumerate() {
        put(&mut out, k * 4, x.to_le_bytes());
    }
    put(&mut out, 12, [c.0, c.1, c.2, 255]);
    out
}

/// A packed buffer on the GPU: none when empty, or when a part is more than the device takes in
/// one buffer (that would be a validation error, which stops the app; what it held isn't drawn).
fn buffer(device: &wgpu::Device, label: &str, p: &Packed) -> Option<Buffer> {
    let max = device.limits().max_buffer_size;
    let idx16: Vec<u8> = p.idx16.iter().flat_map(|i| i.to_le_bytes()).collect();
    let idx32: Vec<u8> = p.idx32.iter().flat_map(|i| i.to_le_bytes()).collect();
    if p.verts.is_empty() || [p.verts.len(), p.extra.len(), idx16.len(), idx32.len()].iter().any(|n| *n as u64 > max) {
        return None;
    }
    let make = |bytes: &[u8], usage: wgpu::BufferUsages| {
        (!bytes.is_empty())
            .then(|| Tracked::new(device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage })))
    };
    let buffer = make(&p.verts, wgpu::BufferUsages::VERTEX)?;
    Some(Buffer {
        buffer,
        extra: make(&p.extra, wgpu::BufferUsages::VERTEX),
        idx16: make(&idx16, wgpu::BufferUsages::INDEX),
        idx32: make(&idx32, wgpu::BufferUsages::INDEX),
        vertex_bytes: p.vertex_bytes(),
        index_bytes: p.index_bytes(),
    })
}

/// Draw `draws` (spans and their instance ranges) out of buffer `b`: its vertices and the
/// placements `ib` bound (and the second stream when `extra` and it has one), then each index
/// buffer's runs.
fn draw_spans(pass: &mut wgpu::RenderPass, b: &Buffer, ib: &wgpu::Buffer, extra: bool, draws: &[(Span, std::ops::Range<u32>)]) {
    if draws.is_empty() {
        return;
    }
    pass.set_vertex_buffer(0, b.buffer.slice(..));
    pass.set_vertex_buffer(1, ib.slice(..));
    if let Some(x) = b.extra.as_ref().filter(|_| extra) {
        pass.set_vertex_buffer(2, x.slice(..));
    }
    for (wide, idx, format) in [(false, &b.idx16, wgpu::IndexFormat::Uint16), (true, &b.idx32, wgpu::IndexFormat::Uint32)] {
        let Some(idx) = idx else { continue };
        if !draws.iter().any(|(s, _)| s.wide == wide) {
            continue;
        }
        pass.set_index_buffer(idx.slice(..), format);
        for (s, i) in draws.iter().filter(|(s, _)| s.wide == wide) {
            pass.draw_indexed(s.idx.clone(), s.base, i.clone());
        }
    }
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

    /// A frame is drawn again exactly when something its pixels come from changes: the drawing
    /// (revision, detail band, baked light, 4D date), the style or background, the view, the
    /// target's size or samples, the fidelity tier or marks, the streamed cells, the lighting.
    /// The pointer moving over a still view changes none of them.
    #[test]
    fn a_frame_key_changes_with_every_input_of_the_picture() {
        use super::*;
        use cadaddy_geom::Vec2;
        let scene = SceneKey { doc: 1, revision: 7, band: -3, lightmap: 0, fourd: None };
        let view = ShadeView::new(None, Vec2::new(50.0, 50.0), 100.0, 640, 480);
        let lit = cadaddy_render::light::Lighting::of(&cadaddy_doc::Drawing::new_metric(), &cadaddy_doc::Space::Model);
        let key = |f: &dyn Fn(&mut FrameKey)| {
            let mut k =
                FrameKey::new(scene, VisualStyle::Shaded, Rgb(255, 255, 255), &view, 4, Tier::Floor, None, 3, Some(lit.clone()), Quality::Full);
            f(&mut k);
            k
        };
        let base = key(&|_| {});
        assert_eq!(base, key(&|_| {}), "the same inputs are the same frame");
        let moved = FrameKey::new(
            scene,
            VisualStyle::Shaded,
            Rgb(255, 255, 255),
            &ShadeView::new(None, Vec2::new(50.5, 50.0), 100.0, 640, 480),
            4,
            Tier::Floor,
            None,
            3,
            Some(lit.clone()),
            Quality::Full,
        );
        let zoomed = FrameKey::new(
            scene,
            VisualStyle::Shaded,
            Rgb(255, 255, 255),
            &ShadeView::new(None, Vec2::new(50.0, 50.0), 99.0, 640, 480),
            4,
            Tier::Floor,
            None,
            3,
            Some(lit.clone()),
            Quality::Full,
        );
        let resized = FrameKey::new(
            scene,
            VisualStyle::Shaded,
            Rgb(255, 255, 255),
            &ShadeView::new(None, Vec2::new(50.0, 50.0), 100.0, 641, 480),
            4,
            Tier::Floor,
            None,
            3,
            Some(lit.clone()),
            Quality::Full,
        );
        let mut dimmer = lit.clone();
        dimmer.exposure += 0.5;
        let changed: Vec<(&str, FrameKey)> = vec![
            ("pan", moved),
            ("zoom", zoomed),
            ("size", resized),
            ("edit", key(&|k| k.scene.revision += 1)),
            ("another drawing", key(&|k| k.scene.doc += 1)),
            ("detail band", key(&|k| k.scene.band += 1)),
            ("baked light", key(&|k| k.scene.lightmap += 1)),
            ("style", key(&|k| k.style = VisualStyle::Hidden)),
            ("background", key(&|k| k.bg = [0, 0, 0])),
            ("samples", key(&|k| k.samples = 1)),
            ("tier", key(&|k| k.tier = Tier::Room)),
            ("marks", key(&|k| k.force = Some(Force { kind: "receptacle".into(), level: None }))),
            ("streamed cells", key(&|k| k.stream += 1)),
            ("lighting", key(&|k| k.lighting = Some(dimmer.clone()))),
            ("lighting quality", key(&|k| k.quality = Quality::Balanced)),
            ("no lighting", key(&|k| k.lighting = None)),
        ];
        for (what, k) in &changed {
            assert_ne!(&base, k, "{what} must draw the frame again");
        }
    }

    /// On the GPU: a target shows a frame only once it has drawn it, a second look at the same
    /// frame is not drawn again, and any other frame (or a render by anyone else) is.
    #[test]
    fn the_gpu_keeps_a_frame_until_its_inputs_change() {
        use super::*;
        use serde_json::json;
        let mut s = cadaddy_engine::Session::new();
        s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 })).unwrap();
        let d = s.doc().unwrap().clone();
        let bg = Rgb(255, 255, 255);
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let model = Arc::new(model3::build_clipped(&d, &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[]));
        let (w, h) = (64u32, 64u32);
        let view = shade::fit(&model.flatten(), None, w, h).unwrap();
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let mut g = Gpu::new(device, 1, true);
        let stream = Stream::default();
        let style = VisualStyle::Shaded;
        let scene = SceneKey { doc: 1, revision: 1, band: 0, lightmap: 0, fourd: None };
        let key = |v: &ShadeView| FrameKey::new(scene, style, bg, v, 1, Tier::Detail, None, stream.generation, None, Quality::Full);
        // What the canvas does each frame; true when the frame was drawn.
        let frame = |g: &mut Gpu, v: &ShadeView| {
            let k = key(v);
            if g.shows(&k) {
                return false;
            }
            if g.model.is_none() {
                g.upload(device, queue, &model, style, bg);
            }
            let vis = g.render(device, queue, v, style, bg, Tier::Detail, None, &stream);
            g.last = Some((k, vis));
            true
        };
        assert!(!g.shows(&key(&view)), "nothing is shown before the first frame");
        assert!(frame(&mut g, &view), "the first frame is drawn");
        let tris = g.last.as_ref().map(|(_, v)| v.tris).unwrap();
        assert!(tris > 0);
        for _ in 0..5 {
            assert!(!frame(&mut g, &view), "a still view is not drawn again");
        }
        assert_eq!(g.last.as_ref().map(|(_, v)| v.tris), Some(tris), "what the kept frame drew is still known");
        let mut turned = view;
        turned.center.x += 1.0;
        assert!(frame(&mut g, &turned), "a moved view is drawn");
        assert!(frame(&mut g, &view), "and so is the first view again: the target shows the moved one");
        // A render the canvas didn't ask for leaves the target showing no known frame.
        let _ = g.render(device, queue, &turned, style, bg, Tier::Detail, None, &stream);
        assert!(!g.shows(&key(&view)) && !g.shows(&key(&turned)));
    }

    /// Each lighting quality draws the scene as many times as it says: the main pass alone at
    /// Performance, the shadow map too at Balanced, and at Full the ambient occlusion's depth
    /// when the lighting has any. A second frame that draws the same things from another side
    /// keeps the shadow map instead of drawing it again.
    #[test]
    fn lighting_quality_sets_the_passes_and_the_shadow_map_is_kept() {
        use super::*;
        use serde_json::json;
        let mut s = cadaddy_engine::Session::new();
        s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 })).unwrap();
        s.execute("box", &json!({ "corner": [20, 0, 0], "length": 10, "width": 10, "height": 1 })).unwrap();
        s.doc_mut().unwrap().sun.on = true;
        let d = s.doc().unwrap().clone();
        let bg = Rgb(255, 255, 255);
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let model = Arc::new(model3::build_clipped(&d, &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[]));
        let lit = model.base.lighting.clone().expect("a model has lighting");
        assert!(lit.sun.is_some(), "the sun is on");
        let (w, h) = (64u32, 64u32);
        let view = shade::fit(&model.flatten(), None, w, h).unwrap();
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let style = VisualStyle::Realistic;
        let stream = Stream::default();
        let passes = |q: Quality| {
            let mut g = Gpu::new(device, 1, true);
            g.quality = q;
            g.upload(device, queue, &model, style, bg);
            g.render(device, queue, &view, style, bg, Tier::Detail, None, &stream);
            let first = g.scene_passes;
            // The same things from a little to the side: the shadow map is the same picture.
            let mut aside = view;
            aside.center.x += 0.5;
            g.render(device, queue, &aside, style, bg, Tier::Detail, None, &stream);
            (first, g.scene_passes, g.shadows_kept)
        };
        let occluded = u32::from(lit.ao > 0.0);
        assert_eq!(passes(Quality::Performance), (1, 1, 0), "the main pass alone, and no shadow map to keep");
        assert_eq!(passes(Quality::Balanced), (2, 1, 1), "the shadow map once, then kept");
        assert_eq!(passes(Quality::Full), (2 + occluded, 1 + occluded, 1));
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
            let mut g = Gpu::new(device, 1, true);
            g.upload(device, queue, &model, VisualStyle::Realistic, bg);
            g.load_tiers(device, queue, &model, Tier::Detail);
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
    /// A slab, five devices (instances at the room tier), a conduit stick (streamed), a wall (the
    /// floor tier), by layer.
    fn tier_model() -> std::sync::Arc<super::Model3> {
        use super::*;
        use cadaddy_doc::{Common, EntityKind};
        let mut d = cadaddy_doc::Drawing::new_metric();
        let mut put = |layer: &str, lo: Vec3, hi: Vec3, rgb: Rgb| {
            let c = Common { layer: layer.into(), color: cadaddy_doc::color::Color::True(rgb), ..Common::default() };
            d.add(&cadaddy_doc::Space::Model, c, EntityKind::Mesh(box_mesh(lo, hi))).unwrap();
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
        Arc::new(model3::build_fid(Arc::new(d), &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[], &classify, model3::FidOptions::default()))
    }

    #[test]
    fn fidelity_tiers_on_the_gpu() {
        use super::*;
        use cadaddy_geom::Vec2;
        let bg = Rgb(255, 255, 255);
        let model = tier_model();
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
        let mut g = Gpu::new(device, 1, true);
        let mut stream = Stream::default();
        // The building tier: nothing streamed, the slab only.
        stream.update(&model, &view, Tier::Building, f64::INFINITY);
        g.upload(device, queue, &model, VisualStyle::Shaded, bg);
        g.load_tiers(device, queue, &model, Tier::Building);
        g.upload_stream(device, queue, &stream, VisualStyle::Shaded, bg);
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
        g.load_tiers(device, queue, &model, Tier::Room);
        g.upload_stream(device, queue, &stream, VisualStyle::Shaded, bg);
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
        g.upload_stream(device, queue, &stream, VisualStyle::Shaded, bg);
        g.load_tiers(device, queue, &model, Tier::Building);
        assert_eq!(g.stream_bytes, 0);
        assert_eq!(g.tier_bytes, [0, 0, 0]);
        g.render(device, queue, &view, VisualStyle::Shaded, bg, Tier::Building, None, &stream);
        let again = read_frame(&g, device, queue, w, h);
        assert_eq!(at(&again, dev), at(&far, dev), "back to the building tier's picture");
    }

    /// The corners' inputs are WebGL2's too: the vertex stages (and the line and sky fragments)
    /// translate to GLSL ES 3.0. (The lit triangles' fragment reads depth textures with
    /// `textureLoad` for ambient occlusion and soft shadows, which GLSL has no form for.)
    #[test]
    fn vertex_stages_translate_for_webgl2() {
        let module = naga::front::wgsl::parse_str(include_str!("shader.wgsl")).unwrap_or_else(|e| panic!("{e}"));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let vertex = ["vs_tri", "vs_tri_x", "vs_line", "vs_shadow", "vs_depth", "vs_bg"].map(|e| (naga::ShaderStage::Vertex, e));
        let fragment = ["fs_line", "fs_bg"].map(|e| (naga::ShaderStage::Fragment, e));
        for (stage, entry) in vertex.into_iter().chain(fragment) {
            let options = naga::back::glsl::Options { version: naga::back::glsl::Version::new_gles(300), ..Default::default() };
            let pipeline = naga::back::glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut out = String::new();
            let mut w = naga::back::glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
                .unwrap_or_else(|e| panic!("{entry}: {e:?}"));
            w.write().unwrap_or_else(|e| panic!("{entry}: {e:?}"));
            assert!(out.contains("#version 300 es"), "{entry}");
        }
    }

    /// A normal folded onto two 16-bit numbers comes back within a ten-thousandth of a radian, in
    /// every direction (the shader's `unoct` mirrored here); no normal comes back as +Z.
    #[test]
    fn normals_fold_onto_two_numbers_and_back() {
        use super::{Vec3, oct};
        let unoct = |e: [i16; 2]| {
            let (x, y) = ((f64::from(e[0]) / 32767.0).max(-1.0), (f64::from(e[1]) / 32767.0).max(-1.0));
            let z = 1.0 - x.abs() - y.abs();
            let t = (-z).max(0.0);
            let x = x + if x >= 0.0 { -t } else { t };
            let y = y + if y >= 0.0 { -t } else { t };
            let l = (x * x + y * y + z * z).sqrt();
            [x / l, y / l, z / l]
        };
        let mut dirs: Vec<[f32; 3]> =
            vec![[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [0.6, -0.8, 0.0]];
        // A spiral over the whole sphere.
        let n = 20_000;
        for k in 0..n {
            let z = 1.0 - 2.0 * (k as f64 + 0.5) / n as f64;
            let (r, a) = ((1.0 - z * z).sqrt(), k as f64 * 2.399_963_23);
            dirs.push([(r * a.cos()) as f32, (r * a.sin()) as f32, z as f32]);
        }
        let mut worst = 0.0f64;
        for d in dirs {
            let m = unoct(oct(d));
            let d = Vec3::new(f64::from(d[0]), f64::from(d[1]), f64::from(d[2]));
            let m = Vec3::new(m[0], m[1], m[2]);
            worst = worst.max(d.cross(m).len().atan2(d.dot(m)));
        }
        assert!(worst < 1e-4, "worst error {worst} rad");
        assert_eq!(unoct(oct([0.0; 3])), [0.0, 0.0, 1.0]);
        assert_eq!(unoct(oct([f32::NAN, 0.0, 1.0])), [0.0, 0.0, 1.0]);
    }

    /// Corners weld only when every value is the same: a flat quad's two triangles share their
    /// diagonal (4 vertices, not 6), a triangle across a crease shares none (its normal differs),
    /// a line loop's ends are shared. Each run counts from its own first vertex (or, without a
    /// base vertex, from the buffer's start) and never welds into another run; a run past 16-bit
    /// indices takes 32-bit ones.
    #[test]
    fn corners_weld_on_a_flat_face_and_not_across_a_crease() {
        use super::*;
        use cadaddy_render::shade::{Line3, Tri};
        let p = Vec3::new;
        let red = Rgb(200, 0, 0);
        let mut sc = Scene::default();
        sc.tris = vec![
            Tri::plain([p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(1.0, 1.0, 0.0)], red),
            Tri::plain([p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(0.0, 1.0, 0.0)], red),
            // Up from the quad's edge x = 1.
            Tri::plain([p(1.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(2.0, 0.5, 1.0)], red),
        ];
        let corners = [p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(0.0, 1.0, 0.0)];
        sc.lines = (0..4).map(|i| Line3 { a: corners[i], b: corners[(i + 1) % 4], color: red, edge: false, faint: false }).collect();
        let how = Packing { style: VisualStyle::Shaded, bg: Rgb(255, 255, 255), physical: false, layers: 0, extra: false };
        let mut mats = Materials::default();
        let mut pack = Pack::new(true, true);
        let s1 = pack.emit(&how, &mut mats, &sc, 0, 0..3, 0..4, Vec3::ZERO);
        assert_eq!(pack.tris.count, 4 + 3, "the quad shares its diagonal; the creased triangle shares nothing");
        assert_eq!(pack.tris.idx16, vec![0, 1, 2, 0, 2, 3, 4, 5, 6]);
        assert_eq!(pack.tris.verts.len(), 7 * VERTEX_SIZE as usize);
        assert_eq!(s1[0], Span { wide: false, idx: 0..9, base: 0 });
        assert_eq!((pack.lines.count, s1[2].idx.len()), (4, 8), "a loop of 4 lines: 4 ends, 8 indices");
        assert_eq!(pack.lines.verts.len(), 4 * LINE_VERTEX_SIZE as usize);
        assert!(s1[1].is_empty() && pack.clear.count == 0);
        let s2 = pack.emit(&how, &mut mats, &sc, 0, 0..3, 0..4, Vec3::ZERO);
        assert_eq!(s2[0], Span { wide: false, idx: 9..18, base: 7 }, "another run counts from its own first vertex");
        assert_eq!(pack.tris.count, 14, "and doesn't weld into the one before");
        assert_eq!(mats.rows.len(), 1, "one material: the plain one");
        // Without a base vertex (WebGL2) the second run's indices count from the buffer's start.
        let mut abs = Pack::new(false, true);
        abs.emit(&how, &mut mats, &sc, 0, 0..3, 0..4, Vec3::ZERO);
        let a2 = abs.emit(&how, &mut mats, &sc, 0, 0..3, 0..4, Vec3::ZERO);
        assert_eq!(a2[0].base, 0);
        assert_eq!(abs.tris.idx16.get(9..18), Some(&[7, 8, 9, 7, 9, 10, 11, 12, 13][..]));
        assert_eq!(abs.tris.verts, pack.tris.verts);
        // Unwelded: every corner its own vertex.
        let mut each = Pack::new(true, false);
        each.emit(&how, &mut mats, &sc, 0, 0..3, 0..4, Vec3::ZERO);
        assert_eq!((each.tris.count, each.lines.count), (9, 8));
        // A run of more vertices than 16 bits number: 32-bit indices.
        let mut big = Scene::default();
        big.tris = (0..22_000).map(|i| Tri::plain([p(f64::from(i), 0.0, 0.0), p(f64::from(i), 1.0, 0.0), p(f64::from(i), 0.0, 1.0)], red)).collect();
        let mut wide = Pack::new(true, true);
        let s = wide.emit(&how, &mut mats, &big, 0, 0..big.tris.len(), 0..0, Vec3::ZERO);
        assert!(s[0].wide && wide.tris.idx16.is_empty());
        assert_eq!((wide.tris.count, wide.tris.idx32.len()), (66_000, 66_000));
    }

    /// A model of boxes on the GPU takes at most a third of the bytes per triangle it did (68-byte
    /// corners, three per triangle, no indices: 204 bytes): welded 24-byte corners with 16-bit
    /// indices. The textures (the same with or without triangles) are left out.
    #[test]
    fn triangles_take_a_third_of_the_gpu_bytes() {
        use super::*;
        use serde_json::json;
        let bg = Rgb(255, 255, 255);
        let built = |boxes: usize| {
            let mut s = cadaddy_engine::Session::new();
            for i in 0..boxes {
                let (x, y) = ((i % 10) as f64 * 15.0, (i / 10) as f64 * 15.0);
                s.execute("box", &json!({ "corner": [x, y, 0], "length": 10, "width": 10, "height": 3 + (i % 4) })).unwrap();
            }
            let d = s.doc().unwrap().clone();
            let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
            Arc::new(model3::build_clipped(&d, &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[]))
        };
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let bytes = |m: &Arc<Model3>| {
            let mut g = Gpu::new(device, 1, true);
            g.upload(device, queue, m, VisualStyle::Shaded, bg);
            g.bytes
        };
        let (none, many) = (built(0), built(60));
        let tris = many.total_tris();
        assert_eq!(tris, 60 * 12);
        let per_tri = (bytes(&many) - bytes(&none)) as f64 / tris as f64;
        assert!(per_tri <= 68.0, "{per_tri} bytes a triangle");
    }

    /// Welding and indexing change no pixel: at every tier, in Shaded, Shaded with Edges and
    /// Realistic, the frame is the same with every corner its own vertex, with welded corners, and
    /// with indices counted from the buffer's start (WebGL2) instead of each run's first vertex.
    #[test]
    fn welded_indexed_corners_draw_the_same_picture_at_every_tier() {
        use super::*;
        let model = tier_model();
        let bg = Rgb(255, 255, 255);
        let (w, h) = (128u32, 128u32);
        let view = shade::fit_bounds(&model.bounds, cadaddy_geom::view3::Projection::new(Vec3::new(-1.0, -1.0, 1.0)), w, h).unwrap();
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let draw = |weld: bool, relative: bool, tier: Tier, style: VisualStyle| {
            let mut g = Gpu::new(device, 1, relative);
            g.weld = weld;
            let mut stream = Stream::default();
            stream.update(&model, &view, tier, f64::INFINITY);
            g.upload(device, queue, &model, style, bg);
            g.load_tiers(device, queue, &model, tier);
            g.upload_stream(device, queue, &stream, style, bg);
            let vis = g.render(device, queue, &view, style, bg, tier, None, &stream);
            (read_frame(&g, device, queue, w, h), vis.tris, g.buffer_bytes())
        };
        for style in [VisualStyle::Shaded, VisualStyle::ShadedEdges, VisualStyle::Realistic] {
            for tier in Tier::ALL {
                let (welded, tris, (vb, ib)) = draw(true, true, tier, style);
                let (each, _, (vb_each, _)) = draw(false, true, tier, style);
                assert!(tris > 0 && ib > 0);
                assert!(vb < vb_each, "{style:?} {tier:?}: welding keeps fewer vertices ({vb} vs {vb_each})");
                assert!(welded == each, "{style:?} {tier:?}: welded corners draw another picture");
                assert!(welded == draw(true, false, tier, style).0, "{style:?} {tier:?}: indices from the buffer's start draw another picture");
                assert!(welded.chunks(4).any(|px| px[0] < 200), "{style:?} {tier:?}: something is drawn");
            }
        }
    }

    /// Under a texture the corners carry their second stream (texture and lightmap coordinates)
    /// and the texture shows: a box topped with a red and blue checker, in Realistic, is red in
    /// one quarter of its top and blue in the next, the same with every corner its own vertex. A
    /// style that draws no textures (Shaded) packs no second stream.
    #[test]
    fn textured_corners_carry_their_second_stream() {
        use super::*;
        use cadaddy_render::shade::SurfaceMat;
        use serde_json::json;
        let mut s = cadaddy_engine::Session::new();
        s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 })).unwrap();
        s.doc_mut().unwrap().sun.on = false;
        let d = s.doc().unwrap().clone();
        let bg = Rgb(255, 255, 255);
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let mut m = model3::build_clipped(&d, &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[]);
        let (red, blue) = ([220u8, 30, 30, 255], [30u8, 30, 220, 255]);
        let checker = [red, blue, blue, red].concat();
        m.base.textures = vec![Arc::new(cadaddy_render::images::Raster::new(2, 2, checker).unwrap())];
        m.base.surfaces = vec![SurfaceMat { name: "checker".into(), base: [1.0; 3], roughness: 0.8, metallic: 0.0, opacity: 1.0, texture: Some(0) }];
        for t in &mut m.base.tris {
            t.mat = 0;
            t.uv = t.p.map(|p| [(p.x / 10.0) as f32, (p.y / 10.0) as f32]);
        }
        let model = Arc::new(m);
        assert!(model.base.lighting.is_some());
        let (w, h) = (64u32, 64u32);
        let view = shade::fit(&model.flatten(), None, w, h).unwrap();
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let draw = |weld: bool, style: VisualStyle| {
            let mut g = Gpu::new(device, 1, true);
            g.weld = weld;
            g.upload(device, queue, &model, style, bg);
            g.render(device, queue, &view, style, bg, Tier::Detail, None, &Stream::default());
            let extra = g.tris.as_ref().map(|b| b.extra.is_some());
            (read_frame(&g, device, queue, w, h), extra)
        };
        let (img, extra) = draw(true, VisualStyle::Realistic);
        assert_eq!(extra, Some(true), "a textured surface's corners have a second stream");
        assert!(img == draw(false, VisualStyle::Realistic).0, "welded corners draw another picture");
        // The top seen from above: two quarters side by side.
        let at = |x: f64, y: f64| {
            let (px, py) = (((x - view.center.x) / view.height + 0.5) * f64::from(h), ((view.center.y - y) / view.height + 0.5) * f64::from(h));
            let i = (py as usize * w as usize + (px + f64::from(w - h) / 2.0) as usize) * 4;
            i32::from(img[i]) - i32::from(img[i + 2])
        };
        let (a, b) = (at(2.5, 2.5), at(7.5, 2.5));
        assert!(a.signum() != b.signum() && a.abs() > 20 && b.abs() > 20, "the checker shows: red − blue {a} and {b}");
        assert_eq!(draw(true, VisualStyle::Shaded).1, Some(false), "no textures drawn, no second stream");
    }

    /// Flat faces stay flat and keep hard edges: a box seen from a corner shows three faces in
    /// three shades, each one shade all over.
    #[test]
    fn a_box_keeps_its_hard_edges() {
        use super::*;
        use serde_json::json;
        let mut s = cadaddy_engine::Session::new();
        s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 })).unwrap();
        let d = s.doc().unwrap().clone();
        let bg = Rgb(255, 255, 255);
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
        let model = Arc::new(model3::build_clipped(&d, &cadaddy_doc::Space::Model, 0.05, bg, &p, &t, &[]));
        let (w, h) = (128u32, 128u32);
        let view = shade::fit_bounds(&model.bounds, cadaddy_geom::view3::Projection::new(Vec3::new(-1.0, -1.0, 1.0)), w, h).unwrap();
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let mut g = Gpu::new(device, 1, true);
        g.upload(device, queue, &model, VisualStyle::Shaded, bg);
        g.render(device, queue, &view, VisualStyle::Shaded, bg, Tier::Detail, None, &Stream::default());
        let img = read_frame(&g, device, queue, w, h);
        // A point's pixel colour.
        let at = |q: Vec3| {
            let half_w = view.height * f64::from(w) / f64::from(h) / 2.0;
            let x = ((q.dot(view.right) - view.center.x + half_w) / view.height * f64::from(h)) as usize;
            let y = ((view.center.y - q.dot(view.up) + view.height / 2.0) / view.height * f64::from(h)) as usize;
            let i = (y * w as usize + x) * 4;
            [img[i], img[i + 1], img[i + 2]]
        };
        // Two points on each face seen: the top, the face towards −x and the one towards −y.
        let faces = [
            (Vec3::new(3.0, 3.0, 10.0), Vec3::new(7.0, 6.0, 10.0)),
            (Vec3::new(0.0, 3.0, 3.0), Vec3::new(0.0, 7.0, 7.0)),
            (Vec3::new(3.0, 0.0, 3.0), Vec3::new(7.0, 0.0, 6.0)),
        ];
        let shades: Vec<[u8; 3]> = faces
            .iter()
            .map(|(a, b)| {
                let (ca, cb) = (at(*a), at(*b));
                assert!((0..3).all(|c| ca[c].abs_diff(cb[c]) <= 1), "a flat face is one shade: {ca:?} {cb:?}");
                ca
            })
            .collect();
        for (i, a) in shades.iter().enumerate() {
            for b in shades.iter().skip(i + 1) {
                assert!((0..3).any(|c| a[c].abs_diff(b[c]) > 8), "faces meeting at an edge are shaded apart: {shades:?}");
            }
        }
    }

    /// A box mesh from `lo` to `hi`.
    fn box_mesh(lo: super::Vec3, hi: super::Vec3) -> cadaddy_doc::Mesh {
        use cadaddy_doc::{Mesh, MeshFace, MeshKind};
        let v = |x: bool, y: bool, z: bool| super::Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
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
    }

    /// A round bar of `sides` flat sides from `a` to `b` (along x or z), radius `r`, capped.
    fn bar_mesh(a: super::Vec3, b: super::Vec3, r: f64, sides: u32) -> cadaddy_doc::Mesh {
        use cadaddy_doc::{Mesh, MeshFace, MeshKind};
        let along = (b - a).normalized();
        let side = if along.z.abs() > 0.9 { super::Vec3::new(1.0, 0.0, 0.0) } else { super::Vec3::Z };
        let (u, v) = (along.cross(side).normalized(), along.cross(along.cross(side)).normalized());
        let ring = |c: super::Vec3| {
            (0..sides).map(move |k| {
                let t = f64::from(k) / f64::from(sides) * std::f64::consts::TAU;
                c + u * (r * t.cos()) + v * (r * t.sin())
            })
        };
        let vertices: Vec<super::Vec3> = ring(a).chain(ring(b)).collect();
        let mut faces: Vec<MeshFace> =
            (0..sides).map(|k| MeshFace { v: vec![k, (k + 1) % sides, sides + (k + 1) % sides, sides + k], hidden: 0 }).collect();
        faces.push(MeshFace { v: (0..sides).rev().collect(), hidden: 0 });
        faces.push(MeshFace { v: (sides..2 * sides).collect(), hidden: 0 });
        Mesh { vertices, faces, kind: MeshKind::Subd }
    }

    /// A made-up hotel-size building (`storeys` floors of 16 × 16 rooms, about 30 × 15 ft, `upf`
    /// units a foot), for the bench: a slab a storey and the risers at the building tier; each
    /// room's four walls, its pipe and its duct at the floor tier; six devices alike (instances)
    /// and four conduit sticks (streamed) at the room tier; eight fittings alike (instances) at
    /// the detail tier. Rooms differ a little in size, as built ones do, so their walls, pipes,
    /// ducts and sticks aren't repeats. Its classifier tiers elements by layer.
    #[allow(clippy::type_complexity)]
    fn synthetic_building(
        storeys: usize,
        upf: f64,
    ) -> (cadaddy_doc::Drawing, fn(&cadaddy_doc::Drawing, &cadaddy_doc::Entity) -> Option<super::model3::ElemClass>) {
        use super::*;
        use cadaddy_doc::{Common, EntityKind};
        let mut d = cadaddy_doc::Drawing::new_metric();
        let mut put = |layer: &str, m: cadaddy_doc::Mesh, rgb: Rgb| {
            let c = Common { layer: layer.into(), color: cadaddy_doc::color::Color::True(rgb), ..Common::default() };
            d.add(&cadaddy_doc::Space::Model, c, EntityKind::Mesh(m)).unwrap();
        };
        let ft = |x: f64, y: f64, z: f64| Vec3::new(x * upf, y * upf, z * upf);
        let (rooms, rw, rd, storey_h) = (16usize, 30.0, 15.0, 10.0);
        for s in 0..storeys {
            let z = s as f64 * storey_h;
            put("SLAB", box_mesh(ft(0.0, 0.0, z - 1.0), ft(rooms as f64 * rw, rooms as f64 * rd, z)), Rgb(200, 200, 200));
            for i in 0..rooms {
                for j in 0..rooms {
                    let (x, y) = (i as f64 * rw, j as f64 * rd);
                    // This room's own size: up to a foot shorter, no two alike.
                    let n = ((s * rooms + i) * rooms + j) as f64 / (storeys * rooms * rooms) as f64;
                    let (rw, rd) = (rw - n, rd - (1.0 - n));
                    let (wall, h) = (Rgb(150, 150, 140), z + 9.0);
                    put("WALL", box_mesh(ft(x, y, z), ft(x + rw, y + 0.4, h)), wall);
                    put("WALL", box_mesh(ft(x, y + rd - 0.4, z), ft(x + rw, y + rd, h)), wall);
                    put("WALL", box_mesh(ft(x, y, z), ft(x + 0.4, y + rd, h)), wall);
                    put("WALL", box_mesh(ft(x + rw - 0.4, y, z), ft(x + rw, y + rd, h)), wall);
                    put("PIPE", bar_mesh(ft(x + 1.0, y + 3.0, z + 8.5), ft(x + rw - 1.0, y + 3.0, z + 8.5), 0.15 * upf, 24), Rgb(200, 40, 40));
                    put("DUCT", box_mesh(ft(x + 1.0, y + 6.0, z + 7.5), ft(x + rw - 1.0, y + 7.5, z + 8.5)), Rgb(160, 170, 190));
                    for k in 0..6 {
                        let at = ft(x + 2.0 + k as f64 * 4.0, y + 0.4, z + 1.5);
                        put("RECEPTACLE", box_mesh(at, at + ft(0.25, 0.1, 0.4)), Rgb(240, 240, 240));
                    }
                    for k in 0..4 {
                        let (a, b) =
                            (ft(x + 2.0, y + 9.0 + k as f64 * 0.5, z + 8.0), ft(x + rw - 2.0 - k as f64 * 5.0, y + 9.0 + k as f64 * 0.5, z + 8.0));
                        put("CONDUIT", bar_mesh(a, b, 0.04 * upf, 16), Rgb(90, 90, 210));
                    }
                    for k in 0..8 {
                        let at = ft(x + 2.0 + k as f64 * 3.0, y + 9.0, z + 8.0);
                        put("FITTING", bar_mesh(at, at + ft(0.15, 0.0, 0.0), 0.06 * upf, 12), Rgb(120, 120, 120));
                    }
                }
            }
        }
        for i in 0..8 {
            let (x, y) = (i as f64 * 60.0 + 15.0, 7.0);
            put("RISER", bar_mesh(ft(x, y, -1.0), ft(x, y, storeys as f64 * storey_h), 0.5 * upf, 24), Rgb(40, 160, 40));
        }
        let classify = |_: &cadaddy_doc::Drawing, e: &cadaddy_doc::Entity| {
            let tier = match e.common.layer.to_ascii_uppercase().as_str() {
                "SLAB" | "RISER" => Tier::Building,
                "WALL" | "PIPE" | "DUCT" => Tier::Floor,
                "RECEPTACLE" | "CONDUIT" => Tier::Room,
                "FITTING" => Tier::Detail,
                _ => return None,
            };
            Some(model3::ElemClass { tier, kind: e.common.layer.to_ascii_lowercase(), level: String::new(), group: None, line_from: None })
        };
        (d, classify)
    }

    /// The hotel-scale numbers on this machine's wgpu (a benchmark, run by hand):
    /// `CADADDY_FIDELITY_BENCH=model.cdy cargo test -p cadaddy-ui-egui --lib fidelity_bench -- --ignored --nocapture`;
    /// `CADADDY_FIDELITY_BENCH=synthetic` (and `CADADDY_FIDELITY_STOREYS`, 8 by default) the
    /// made-up building of [`synthetic_building`]; `CADADDY_FIDELITY_REPEAT` frames timed a tier (5),
    /// `CADADDY_FIDELITY_FRAMES` a folder for each tier's first frame (raw RGBA).
    /// The whole building at each tier, and without fidelity: build, upload and frame times, the
    /// buffers' bytes; then a zoom into the room round one receptacle: the time until it is drawn.
    #[test]
    #[ignore = "a benchmark: set CADADDY_FIDELITY_BENCH to a built model"]
    fn fidelity_bench() {
        use super::*;
        let Ok(path) = std::env::var("CADADDY_FIDELITY_BENCH") else { return };
        let (w, h) = (1600u32, 1000u32);
        let synthetic = (path == "synthetic").then(|| {
            let storeys = std::env::var("CADADDY_FIDELITY_STOREYS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
            let upf = cadaddy_engine::cmd::fidelity::units_per_foot(&cadaddy_doc::Drawing::new_metric());
            synthetic_building(storeys, upf)
        });
        let md = match &synthetic {
            Some((d, _)) => Arc::new(d.clone()),
            None => {
                let doc = Arc::new(cadaddy_io::cdy::read_cdy(&std::fs::read(&path).unwrap()).unwrap());
                Arc::new(cadaddy_engine::cmd::bimlook::display(&doc).unwrap())
            }
        };
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
            let model = Arc::new(match &synthetic {
                Some((_, classify)) if fid => {
                    model3::build_fid(Arc::clone(&md), &cadaddy_doc::Space::Model, tol, bg, &p, &t, &[], classify, model3::FidOptions::default())
                }
                Some(_) => model3::build_clipped(&md, &cadaddy_doc::Space::Model, tol, bg, &p, &t, &[]),
                None => build3(&md, fid, tol, bg, &p, &t, &[]),
            });
            let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let whole = shade::fit_bounds(&model.bounds, proj, w, h).unwrap();
            let tiers: Vec<Tier> = if fid { Tier::ALL.to_vec() } else { vec![Tier::Detail] };
            for tier in tiers {
                let mut g = Gpu::new(device, 4, true);
                let mut stream = Stream::default();
                let t1 = web_time::Instant::now();
                stream.update(&model, &whole, tier, f64::INFINITY);
                g.upload(device, queue, &model, style, bg);
                if !fid && g.tris.is_none() && model.total_tris() > 0 {
                    out.insert(
                        "without".into(),
                        serde_json::json!({
                            "build_ms": build_ms.round(), "tris": model.total_tris(), "model_mb": (model.bytes() as f64 / 1048576.0).round(),
                            "gpu": format!("needs {} MB of buffers, a part over this device's {} MB buffer limit: not drawn", g.bytes >> 20, device.limits().max_buffer_size >> 20),
                        }),
                    );
                    continue;
                }
                g.load_tiers(device, queue, &model, tier);
                g.upload_stream(device, queue, &stream, style, bg);
                let vis = g.render(device, queue, &whole, style, bg, tier, None, &stream);
                wait();
                let first_ms = t1.elapsed().as_secs_f64() * 1000.0;
                // The frame, for comparing pictures between builds.
                if let Ok(dir) = std::env::var("CADADDY_FIDELITY_FRAMES") {
                    let name = format!("{}-{}.rgba", if fid { "tiers" } else { "all" }, tier.name());
                    std::fs::write(std::path::Path::new(&dir).join(name), read_frame(&g, device, queue, w, h)).unwrap();
                }
                // The triangles' buffers (opaque and see-through, main, tiers, streamed cells):
                // their bytes per triangle held.
                let tri_bufs =
                    (0..4).flat_map(|slot| [g.buffer_of(slot, 0), g.buffer_of(slot, 1)]).chain(g.stream_bufs.iter().take(2).map(Option::as_ref));
                let tri_bytes: usize = tri_bufs.flatten().map(|b| b.vertex_bytes + b.index_bytes).sum();
                let held = model.loaded_at(tier).0 + stream.tris();
                let (vb, ib) = g.buffer_bytes();
                let n: u32 = std::env::var("CADADDY_FIDELITY_REPEAT").ok().and_then(|s| s.parse().ok()).unwrap_or(5).clamp(1, 10_000);
                let t2 = web_time::Instant::now();
                // The CPU's share of a frame (culling, placements, encoding).
                let mut encode = 0.0;
                for _ in 0..n {
                    let t3 = web_time::Instant::now();
                    g.render(device, queue, &whole, style, bg, tier, None, &stream);
                    encode += t3.elapsed().as_secs_f64() * 1000.0;
                    wait();
                }
                let frame_ms = t2.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
                let key = if fid { tier.name().to_string() } else { "without".to_string() };
                out.insert(
                    key,
                    serde_json::json!({
                        "build_ms": build_ms.round(), "upload_and_first_frame_ms": first_ms.round(), "frame_ms": (frame_ms * 10.0).round() / 10.0,
                        "fps": (1000.0 / frame_ms * 10.0).round() / 10.0, "encode_ms": (encode / f64::from(n) * 10.0).round() / 10.0, "tris_drawn": vis.tris + stream.tris(),
                        "gpu_buffers_mb": ((g.bytes + g.tier_bytes.iter().sum::<usize>() + g.stream_bytes) as f64 / 1048576.0).round(), "model_mb": ((model.bytes() + stream.bytes) as f64 / 1048576.0).round(),
                        "vertex_mb": (vb as f64 / 1048576.0 * 10.0).round() / 10.0, "index_mb": (ib as f64 / 1048576.0 * 10.0).round() / 10.0,
                        "tris_held": held, "bytes_per_tri": (tri_bytes as f64 / held.max(1) as f64 * 10.0).round() / 10.0, "upload_ms": g.upload_ms.round(),
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
            let mut g = Gpu::new(device, 4, true);
            let mut stream = Stream::default();
            g.upload(device, queue, &model, style, bg);
            g.load_tiers(device, queue, &model, whole_tier);
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
            g.load_tiers(device, queue, &model, floor_tier);
            g.upload_stream(device, queue, &stream, style, bg);
            let floor_cpu_ms = tf.elapsed().as_secs_f64() * 1000.0;
            g.render(device, queue, &floor_view, style, bg, floor_tier, None, &stream);
            wait();
            let floor_ms = tf.elapsed().as_secs_f64() * 1000.0;
            // Then the room: the CPU's share (stream, tiers, buffers, what is visible), then the frame.
            let t3 = web_time::Instant::now();
            let step = stream.update(&model, &room, tier, STREAM_MS);
            g.load_tiers(device, queue, &model, tier);
            g.upload_stream(device, queue, &stream, style, bg);
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
                "zoom_room".into(),
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
