//! The Model tab in a visual style other than 2D Wireframe (VSCURRENT): the drawing's 3D scene
//! ([`cadaddy_render::shade`]) drawn with a depth buffer — on the GPU into an offscreen texture
//! (4× MSAA) when the app has a wgpu device, else by the CPU rasterizer that plots and exports
//! share — and shown under the canvas's overlays (grid, highlights, grips, cursor).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cadaddy_color::Rgb;
use cadaddy_geom::Vec3;
use cadaddy_render::model3::{self, Model3};
use cadaddy_render::shade::{self, Scene, ShadeView, VisualStyle};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

use crate::CadApp;
use crate::canvas::Xf;

/// What a model was built from: the drawing (its allocation and revision) and the detail band.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SceneKey {
    doc: usize,
    revision: u64,
    band: i32,
    /// The baked lighting's version (LIGHTBAKE): a bake builds the model again.
    lightmap: u64,
}

/// A model being built off the UI thread, and how far it has got.
struct Pending {
    key: SceneKey,
    progress: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    started: std::time::Instant,
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
}

/// An orbit benchmark under way: frames left, the azimuth reached, when it started.
#[derive(Clone, Copy, Debug)]
struct Bench {
    left: u32,
    azimuth: f64,
    frames: u32,
    start: std::time::Instant,
}

/// Start an orbit benchmark of `frames` frames: each turns the view 2° about Z (at 30° up), as
/// fast as frames are drawn; [`Stats::bench_fps`] holds the result once done.
pub fn bench(app: &mut CadApp, frames: u32) {
    app.canvas.shade.bench = Some(Bench { left: frames.clamp(1, 100_000), azimuth: 0.0, frames: 0, start: std::time::Instant::now() });
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
    last_frame: Option<std::time::Instant>,
    bench: Option<Bench>,
}

impl State {
    /// Drop the GPU renderer (limits changed, or the GPU failed): the next frame starts again.
    pub(crate) fn reset_gpu(&mut self) {
        self.gpu = None;
        self.uploaded = None;
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
    let now = std::time::Instant::now();
    let st = &mut app.canvas.shade;
    if let Some(last) = st.last_frame {
        let dt = now.duration_since(last).as_secs_f64() * 1000.0;
        // Lately: a running average that forgets a pause.
        st.stats.frame_ms = if dt > 1000.0 || st.stats.frame_ms == 0.0 { dt.min(1000.0) } else { st.stats.frame_ms * 0.9 + dt * 0.1 };
    }
    st.last_frame = Some(now);
    let height = f64::from(xf.rect.height()) / xf.scale.max(1e-300);
    let view = ShadeView::new(xf.proj, xf.center, height, w, h);
    let mut drawn = false;
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
            st.uploaded = up;
        }
        let t0 = std::time::Instant::now();
        let vis = g.render(&rs.device, &rs.queue, &view, style, bg);
        st.stats.encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
        st.stats.tris_drawn = vis.tris;
        st.stats.instances_drawn = vis.instances.iter().map(|g| g.2.len()).sum();
        st.stats.chunks_drawn = vis.chunks.len();
        // Buffers and textures, plus the render targets (4× colour and depth, the resolved image)
        // and the shadow map.
        let (tw, th) = (u64::from(w), u64::from(h));
        st.stats.gpu_bytes =
            g.bytes + (tw * th * (u64::from(g.samples) * 8 + 4)) as usize + (u64::from(SHADOW_PX) * u64::from(SHADOW_PX) * 4) as usize;
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
        let flat = match &app.canvas.shade.flat {
            Some((k, f)) if *k == key => Arc::clone(f),
            _ => {
                let f = Arc::new(model.flatten());
                app.canvas.shade.flat = Some((key, Arc::clone(&f)));
                f
            }
        };
        let ck = format!("{:?}|{:?}|{:?}|{}x{}|{:?}|{:?}|{:?}", key, view.center, view.height, w, h, view.dir, view.right, (style, bg));
        let fresh = app.canvas.shade.cpu.as_ref().is_some_and(|(k, _)| *k == ck);
        if !fresh {
            let r = shade::rasterize(&flat, &view, style, bg);
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
    let key = SceneKey { doc: Arc::as_ptr(&st.doc) as usize, revision: st.revision, band, lightmap: st.lightmap_rev };
    let doc = Arc::clone(&st.doc);
    let shade_st = &mut app.canvas.shade;
    if shade_st.model.as_ref().is_some_and(|(k, _)| *k == key) {
        shade_st.stats.building = None;
        return shade_st.model.clone();
    }
    let tcol = crate::theme::Tokens::get().canvas;
    let bg = Rgb(tcol.r(), tcol.g(), tcol.b());
    let tol = 2f64.powi(band);
    // A build for another key is dropped (it finishes in the background and is ignored).
    if shade_st.pending.as_ref().is_some_and(|p| p.key != key) {
        shade_st.pending = None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if shade_st.pending.is_none() {
            let (progress, total) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(1)));
            let (p2, t2) = (Arc::clone(&progress), Arc::clone(&total));
            let handle = std::thread::Builder::new()
                .name("cadaddy-3d-model".into())
                .spawn(move || model3::build(&doc, &cadaddy_doc::Space::Model, tol, bg, &p2, &t2))
                .ok();
            match handle {
                Some(handle) => shade_st.pending = Some(Pending { key, progress, total, started: std::time::Instant::now(), handle }),
                // No thread: build here.
                None => {
                    let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(1));
                    let started = std::time::Instant::now();
                    let m = Arc::new(model3::build(&*st_doc(app)?, &cadaddy_doc::Space::Model, tol, bg, &p, &t));
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
        let started = std::time::Instant::now();
        let m = Arc::new(model3::build(&doc, &cadaddy_doc::Space::Model, tol, bg, &p, &t));
        Some(finish(app, key, m, started))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn st_doc(app: &CadApp) -> Option<Arc<cadaddy_doc::Drawing>> {
    app.session.state().ok().map(|st| Arc::clone(&st.doc))
}

/// Keep a finished model and its numbers.
fn finish(app: &mut CadApp, key: SceneKey, m: Arc<Model3>, started: std::time::Instant) -> (SceneKey, Arc<Model3>) {
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
const UNIFORM_SIZE: u64 = 1328;
/// Texture array layer size and most layers.
const TEX_PX: u32 = 256;
const MAX_LAYERS: usize = 64;
/// Shadow map size.
const SHADOW_PX: u32 = 2048;

struct Target {
    size: (u32, u32),
    /// The multisampled colour (none without MSAA: the frame is drawn into directly).
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    view: wgpu::TextureView,
}

struct Buffer {
    buffer: wgpu::Buffer,
}

/// The shaded view's GPU renderer.
struct Gpu {
    tri_pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bg_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The shadow pass's own group (the uniform only: it writes the shadow map the others read).
    shadow_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    /// The lightmap's: linear, clamped (cells have borders).
    lm_sampler: wgpu::Sampler,
    shadow_view: wgpu::TextureView,
    bind_group: Option<wgpu::BindGroup>,
    tris: Option<Buffer>,
    /// See-through triangles (drawn after the others, blended).
    clear_tris: Option<Buffer>,
    lines: Option<Buffer>,
    /// Vertex ranges in the three buffers (opaque, see-through, lines): each base chunk's, and
    /// each prototype's at each level of detail.
    chunk_ranges: Vec<[std::ops::Range<u32>; 3]>,
    proto_ranges: Vec<[[std::ops::Range<u32>; 3]; 3]>,
    /// The frame's instance placements (slot 0: the identity, for the base geometry).
    instances: Option<wgpu::Buffer>,
    instance_cap: u64,
    model: Option<Arc<Model3>>,
    /// Bytes in the GPU buffers.
    bytes: usize,
    /// The scene's centre (vertices are stored relative to it, keeping f32 precision) and its
    /// extent, for the depth range.
    origin: Vec3,
    radius: f64,
    lighting: Option<cadaddy_render::light::Lighting>,
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
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shaded"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shaded_frame"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_shadow_map"),
            size: wgpu::Extent3d { width: SHADOW_PX, height: SHADOW_PX, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        Gpu {
            tri_pipeline: pipeline("shaded_tris", "vs_tri", "fs_tri", wgpu::PrimitiveTopology::TriangleList, true, None),
            clear_pipeline: pipeline("shaded_glass", "vs_tri", "fs_tri", wgpu::PrimitiveTopology::TriangleList, false, alpha),
            line_pipeline: pipeline("shaded_lines", "vs_line", "fs_line", wgpu::PrimitiveTopology::LineList, false, None),
            bg_pipeline,
            shadow_pipeline,
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
            shadow_view: shadow_tex.create_view(&wgpu::TextureViewDescriptor::default()),
            bind_group: None,
            tris: None,
            clear_tris: None,
            lines: None,
            chunk_ranges: Vec::new(),
            proto_ranges: Vec::new(),
            instances: None,
            instance_cap: 0,
            model: None,
            bytes: 0,
            origin: Vec3::ZERO,
            radius: 1.0,
            lighting: None,
            target: None,
            samples,
        }
    }

    /// The model's triangles (none for Wireframe), the lines `style` shows, and the textures:
    /// the base chunk by chunk, then each prototype at each level of detail, with the ranges
    /// each takes.
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
        let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges);
        let dark = edge_ink(bg);
        let srgb = |c: f32| (c.max(0.0).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
        let (mut tris, mut clear, mut lines): (Vec<u8>, Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new(), Vec::new());
        let count = |v: &Vec<u8>| (v.len() as u64 / VERTEX_SIZE) as u32;
        // One scene's triangles and lines in the given ranges into the buffers: their vertex ranges.
        let mut emit = |sc: &Scene, offset: usize, tr: std::ops::Range<usize>, lr: std::ops::Range<usize>, o: Vec3| -> [std::ops::Range<u32>; 3] {
            let rel = |p: Vec3| [(p.x - o.x) as f32, (p.y - o.y) as f32, (p.z - o.z) as f32];
            let (t0, c0, l0) = (count(&tris), count(&clear), count(&lines));
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
                            let layer = sm.texture.map(|i| i as usize + offset).filter(|i| *i < layers.len()).map_or(-1.0, |i| i as f32);
                            (Rgb(srgb(sm.base[0]), srgb(sm.base[1]), srgb(sm.base[2])), [sm.roughness, sm.metallic, layer, sm.opacity])
                        }
                        None => (t.color, [0.6, 0.0, -1.0, 1.0]),
                    };
                    let out = if m[3] < 0.99 { &mut clear } else { &mut tris };
                    for ((p, uv), l) in t.p.iter().zip(t.uv).zip(lm) {
                        push_vertex(out, rel(*p), n, color, uv, m, l);
                    }
                }
            }
            for l in sc.lines.get(lr).unwrap_or(&[]).iter().filter(|l| !l.edge || edges) {
                let c = if l.edge && style == VisualStyle::ShadedEdges { dark } else { l.color };
                push_vertex(&mut lines, rel(l.a), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0], [-1.0; 4]);
                push_vertex(&mut lines, rel(l.b), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0], [-1.0; 4]);
            }
            [t0..count(&tris), c0..count(&clear), l0..count(&lines)]
        };
        let o = self.origin;
        let chunk_ranges: Vec<[std::ops::Range<u32>; 3]> =
            model.chunks.iter().map(|c| emit(&model.base, 0, c.tris.clone(), c.lines.clone(), o)).collect();
        let mut k = 1;
        let proto_ranges: Vec<[[std::ops::Range<u32>; 3]; 3]> = model
            .protos
            .iter()
            .map(|p| {
                let r = [0, 1, 2].map(|l| {
                    let sc = &p.lods[l];
                    let off = offsets.get(k + l).copied().unwrap_or(0);
                    emit(sc, off, 0..sc.tris.len(), 0..sc.lines.len(), Vec3::ZERO)
                });
                k += 3;
                r
            })
            .collect();
        self.bytes = tris.len() + clear.len() + lines.len() + layers.len().max(1) * (TEX_PX * TEX_PX * 4) as usize;
        self.tris = buffer(device, "shaded_tris", &tris);
        self.clear_tris = buffer(device, "shaded_glass", &clear);
        self.lines = buffer(device, "shaded_lines", &lines);
        self.chunk_ranges = chunk_ranges;
        self.proto_ranges = proto_ranges;
        self.model = Some(Arc::clone(model));
        // The textures, one layer each, resized to the layer size (at least one layer).
        let count = layers.len().max(1) as u32;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_textures"),
            size: wgpu::Extent3d { width: TEX_PX, height: TEX_PX, depth_or_array_layers: count },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
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
        let tex_view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        // The baked lighting atlas (RGBM), or one black texel.
        let (lw, lh, lbytes) = match model.base.baked.as_ref().filter(|_| physical) {
            Some(b) => (b.map.width, b.map.height, cadaddy_render::lightmap::rgbm(&b.map)),
            None => (1, 1, vec![0, 0, 0, 255]),
        };
        let lm_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shaded_lightmap"),
            size: wgpu::Extent3d { width: lw.max(1), height: lh.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if lbytes.len() as u64 == u64::from(lw) * u64::from(lh) * 4 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &lm_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &lbytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
            );
        }
        self.bytes += lbytes.len();
        let lm_view = lm_tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shaded"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.shadow_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.shadow_sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&lm_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.lm_sampler) },
            ],
        }));
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
        (u, sun.is_some())
    }

    /// Draw into the offscreen target sized like the view: what it shows, nearest first.
    fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, v: &ShadeView, style: VisualStyle, bg: Rgb) -> model3::Visible {
        let size = (v.width_px, v.height_px);
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(target(device, size, self.samples));
        }
        let Some(model) = self.model.clone() else { return model3::Visible::default() };
        let vis = model.visible(v);
        let (inst, groups) = self.placements(&model, &vis);
        let need = inst.len() as u64;
        if self.instances.is_none() || self.instance_cap < need {
            let cap = need.next_power_of_two().max(INSTANCE_SIZE * 64);
            self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shaded_instances"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
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
        let draws = |kind: usize| -> Vec<(std::ops::Range<u32>, std::ops::Range<u32>)> {
            let mut d: Vec<(std::ops::Range<u32>, std::ops::Range<u32>)> =
                vis.chunks.iter().filter_map(|c| self.chunk_ranges.get(*c)).map(|r| (r[kind].clone(), 0..1)).collect();
            for ((p, lod, _), ir) in vis.instances.iter().zip(&groups) {
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
            if let Some(m) = &self.tris {
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_vertex_buffer(0, m.buffer.slice(..));
                pass.set_vertex_buffer(1, ib.slice(..));
                for (r, i) in draws(0) {
                    pass.draw(r, i);
                }
            }
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shaded"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: t.msaa.as_ref().unwrap_or(&t.view),
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
            for (kind, pipeline, m) in
                [(0, &self.tri_pipeline, &self.tris), (2, &self.line_pipeline, &self.lines), (1, &self.clear_pipeline, &self.clear_tris)]
            {
                if let Some(m) = m {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, m.buffer.slice(..));
                    pass.set_vertex_buffer(1, ib.slice(..));
                    for (r, i) in draws(kind) {
                        pass.draw(r, i);
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
    if bytes.is_empty() {
        return None;
    }
    let buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage: wgpu::BufferUsages::VERTEX });
    Some(Buffer { buffer })
}

fn target(device: &wgpu::Device, size: (u32, u32), samples: u32) -> Target {
    let extent = wgpu::Extent3d { width: size.0.clamp(1, shade::MAX_SIDE), height: size.1.clamp(1, shade::MAX_SIDE), depth_or_array_layers: 1 };
    let tex = |label: &str, format, samples, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let msaa = (samples > 1).then(|| tex("shaded_msaa", FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
    let depth = tex("shaded_depth", DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let resolve = tex("shaded_frame", FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    Target {
        size,
        msaa: msaa.map(|m| m.create_view(&wgpu::TextureViewDescriptor::default())),
        depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
        view: resolve.create_view(&wgpu::TextureViewDescriptor::default()),
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
}
