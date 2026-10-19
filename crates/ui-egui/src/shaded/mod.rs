//! The Model tab in a visual style other than 2D Wireframe (VSCURRENT): the drawing's 3D scene
//! ([`cadaddy_render::shade`]) drawn with a depth buffer — on the GPU into an offscreen texture
//! (4× MSAA) when the app has a wgpu device, else by the CPU rasterizer that plots and exports
//! share — and shown under the canvas's overlays (grid, highlights, grips, cursor).

use std::sync::Arc;

use cadaddy_color::Rgb;
use cadaddy_geom::Vec3;
use cadaddy_render::shade::{self, Scene, ShadeView, VisualStyle};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

use crate::CadApp;
use crate::canvas::Xf;

/// What a scene was built from: the drawing (its allocation and revision) and the detail band.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SceneKey {
    doc: usize,
    revision: u64,
    band: i32,
}

/// The shaded view's caches.
#[derive(Default)]
pub struct State {
    scene: Option<(SceneKey, Arc<Scene>)>,
    gpu: Option<Gpu>,
    /// What the GPU buffers hold: scene, style and background.
    uploaded: Option<(SceneKey, VisualStyle, [u8; 3])>,
    texture: Option<egui::TextureId>,
    /// The CPU path's last image and what it showed.
    cpu: Option<(String, egui::TextureHandle)>,
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
    let Some(scene) = scene(app, band) else { return false };
    let height = f64::from(xf.rect.height()) / xf.scale.max(1e-300);
    let view = ShadeView::new(xf.proj, xf.center, height, w, h);
    if let Some(rs) = app.wgpu_rs.clone() {
        let st = &mut app.canvas.shade;
        let g = st.gpu.get_or_insert_with(|| Gpu::new(&rs.device));
        let key = st.scene.as_ref().map(|(k, _)| *k);
        let up = key.map(|k| (k, style, [bg.0, bg.1, bg.2]));
        if up.is_some() && st.uploaded != up {
            g.upload(&rs.device, &rs.queue, &scene, style, bg);
            st.uploaded = up;
        }
        g.render(&rs.device, &rs.queue, &view, style, bg);
        if let Some(tv) = g.view() {
            let mut r = rs.renderer.write();
            match st.texture {
                Some(id) => r.update_egui_texture_from_wgpu_texture(&rs.device, tv, wgpu::FilterMode::Linear, id),
                None => st.texture = Some(r.register_native_texture(&rs.device, tv, wgpu::FilterMode::Linear)),
            }
        }
        if let Some(id) = st.texture {
            painter.image(id, xf.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            return true;
        }
    }
    // The CPU rasterizer, redrawn when the view, scene or style changes.
    let key = format!(
        "{:?}|{:?}|{:?}|{}x{}|{:?}|{:?}|{:?}",
        app.canvas.shade.scene.as_ref().map(|(k, _)| *k),
        view.center,
        view.height,
        w,
        h,
        view.dir,
        view.right,
        (style, bg)
    );
    let fresh = app.canvas.shade.cpu.as_ref().is_some_and(|(k, _)| *k == key);
    if !fresh {
        let r = shade::rasterize(&scene, &view, style, bg);
        let img = egui::ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &r.rgba);
        let tex = ui.ctx().load_texture("shaded_view", img, egui::TextureOptions::LINEAR);
        app.canvas.shade.cpu = Some((key, tex));
    }
    if let Some((_, tex)) = &app.canvas.shade.cpu {
        painter.image(tex.id(), xf.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        return true;
    }
    false
}

/// The model's scene at detail `band` (curves to 2^band drawing units), rebuilt when the drawing
/// changes.
fn scene(app: &mut CadApp, band: i32) -> Option<Arc<Scene>> {
    let st = app.session.state().ok()?;
    let key = SceneKey { doc: Arc::as_ptr(&st.doc) as usize, revision: st.revision, band };
    if let Some((k, s)) = &app.canvas.shade.scene
        && *k == key
    {
        return Some(Arc::clone(s));
    }
    let t = crate::theme::Tokens::get();
    let bg = Rgb(t.canvas.r(), t.canvas.g(), t.canvas.b());
    let s = Arc::new(shade::scene(&st.doc, &cadaddy_doc::Space::Model, 2f64.powi(band), bg));
    app.canvas.shade.scene = Some((key, Arc::clone(&s)));
    Some(s)
}

/// Display values (sRGB-encoded by the shader), as egui shows a texture.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
/// Position (3 × f32, from the scene's origin) + normal (3 × f32) + sRGB colour (4 × u8) +
/// texture coordinates (2 × f32) + material (roughness, metallic, texture layer, opacity: 4 × f32).
const VERTEX_SIZE: u64 = 52;
/// The frame's uniform block (see `shader.wgsl`).
const UNIFORM_SIZE: u64 = 1328;
/// Texture array layer size and most layers.
const TEX_PX: u32 = 256;
const MAX_LAYERS: usize = 64;
/// Shadow map size.
const SHADOW_PX: u32 = 2048;

struct Target {
    size: (u32, u32),
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    view: wgpu::TextureView,
}

struct Buffer {
    buffer: wgpu::Buffer,
    count: u32,
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
    shadow_view: wgpu::TextureView,
    bind_group: Option<wgpu::BindGroup>,
    tris: Option<Buffer>,
    /// See-through triangles (drawn after the others, blended).
    clear_tris: Option<Buffer>,
    lines: Option<Buffer>,
    /// The scene's centre (vertices are stored relative to it, keeping f32 precision) and its
    /// extent, for the depth range.
    origin: Vec3,
    radius: f64,
    lighting: Option<cadaddy_render::light::Lighting>,
    target: Option<Target>,
}

impl Gpu {
    fn new(device: &wgpu::Device) -> Gpu {
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
        const ATTRS: [wgpu::VertexAttribute; 5] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Float32x2, 4 => Float32x4];
        let vb = [Some(wgpu::VertexBufferLayout { array_stride: VERTEX_SIZE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS })];
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
                multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
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
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
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
            shadow_view: shadow_tex.create_view(&wgpu::TextureViewDescriptor::default()),
            bind_group: None,
            tris: None,
            clear_tris: None,
            lines: None,
            origin: Vec3::ZERO,
            radius: 1.0,
            lighting: None,
            target: None,
        }
    }

    /// The scene's triangles (none for Wireframe), the lines `style` shows, and its textures.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene, style: VisualStyle, bg: Rgb) {
        let pts = scene.tris.iter().flat_map(|t| t.p).chain(scene.lines.iter().flat_map(|l| [l.a, l.b]));
        let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for p in pts {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        self.origin = if lo.x <= hi.x { (lo + hi) * 0.5 } else { Vec3::ZERO };
        self.radius = if lo.x <= hi.x { ((hi - lo).len() * 0.5).max(1e-9) } else { 1.0 };
        let physical = style == VisualStyle::Realistic && scene.lighting.is_some();
        self.lighting = if physical { scene.lighting.clone() } else { None };
        let o = self.origin;
        let rel = |p: Vec3| [(p.x - o.x) as f32, (p.y - o.y) as f32, (p.z - o.z) as f32];
        let srgb = |c: f32| (c.max(0.0).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
        let layers = scene.textures.len().min(MAX_LAYERS);
        let (mut tris, mut clear): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        if style != VisualStyle::Wireframe {
            tris.reserve(scene.tris.len() * 3 * VERTEX_SIZE as usize);
            for t in &scene.tris {
                let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
                let n = if n.is_finite() { [n.x as f32, n.y as f32, n.z as f32] } else { [0.0, 0.0, 1.0] };
                let surface = scene.surfaces.get(t.mat as usize).filter(|_| physical);
                let (color, m) = match surface {
                    Some(s) => {
                        let layer = s.texture.filter(|i| (*i as usize) < layers).map_or(-1.0, |i| i as f32);
                        (Rgb(srgb(s.base[0]), srgb(s.base[1]), srgb(s.base[2])), [s.roughness, s.metallic, layer, s.opacity])
                    }
                    None => (t.color, [0.6, 0.0, -1.0, 1.0]),
                };
                let out = if m[3] < 0.99 { &mut clear } else { &mut tris };
                for (p, uv) in t.p.iter().zip(t.uv) {
                    push_vertex(out, rel(*p), n, color, uv, m);
                }
            }
        }
        let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges);
        let dark = edge_ink(bg);
        let mut lines: Vec<u8> = Vec::new();
        for l in scene.lines.iter().filter(|l| !l.edge || edges) {
            let c = if l.edge && style == VisualStyle::ShadedEdges { dark } else { l.color };
            push_vertex(&mut lines, rel(l.a), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0]);
            push_vertex(&mut lines, rel(l.b), [0.0, 0.0, 1.0], c, [0.0; 2], [0.6, 0.0, -1.0, 1.0]);
        }
        self.tris = buffer(device, "shaded_tris", &tris);
        self.clear_tris = buffer(device, "shaded_glass", &clear);
        self.lines = buffer(device, "shaded_lines", &lines);
        // The textures, one layer each, resized to the layer size (at least one layer).
        let count = layers.max(1) as u32;
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
        for (i, r) in scene.textures.iter().take(layers).enumerate() {
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
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shaded"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.shadow_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.shadow_sampler) },
            ],
        }));
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

    /// Draw into the offscreen target sized like the view.
    fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, v: &ShadeView, style: VisualStyle, bg: Rgb) {
        let size = (v.width_px, v.height_px);
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(target(device, size));
        }
        let (Some(t), Some(group)) = (&self.target, &self.bind_group) else { return };
        let (u, sun) = self.uniforms(v, style, bg);
        queue.write_buffer(&self.uniform, 0, &u);
        let lin = |c: u8| f64::from(c) / 255.0;
        let physical = self.lighting.is_some();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shaded") });
        // The sun's shadow map first.
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
                pass.draw(0..m.count, 0..1);
            }
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shaded"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    depth_slice: None,
                    resolve_target: Some(&t.view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: lin(bg.0), g: lin(bg.1), b: lin(bg.2), a: 1.0 }),
                        store: wgpu::StoreOp::Discard,
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
            for (pipeline, m) in [(&self.tri_pipeline, &self.tris), (&self.line_pipeline, &self.lines), (&self.clear_pipeline, &self.clear_tris)] {
                if let Some(m) = m {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, m.buffer.slice(..));
                    pass.draw(0..m.count, 0..1);
                }
            }
        }
        queue.submit([enc.finish()]);
    }

    fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }
}

/// The colour of surface edges in Shaded with Edges: near black, which reads on every lit face.
pub(crate) fn edge_ink(_bg: Rgb) -> Rgb {
    Rgb(16, 16, 16)
}

fn push_vertex(out: &mut Vec<u8>, p: [f32; 3], n: [f32; 3], c: Rgb, uv: [f32; 2], m: [f32; 4]) {
    for x in p.iter().chain(n.iter()) {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&[c.0, c.1, c.2, 255]);
    for x in uv.iter().chain(m.iter()) {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

fn buffer(device: &wgpu::Device, label: &str, bytes: &[u8]) -> Option<Buffer> {
    if bytes.is_empty() {
        return None;
    }
    let count = u32::try_from(bytes.len() as u64 / VERTEX_SIZE).unwrap_or(u32::MAX);
    let buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage: wgpu::BufferUsages::VERTEX });
    Some(Buffer { buffer, count })
}

fn target(device: &wgpu::Device, size: (u32, u32)) -> Target {
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
    let msaa = tex("shaded_msaa", FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let depth = tex("shaded_depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let resolve = tex("shaded_frame", FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    Target {
        size,
        msaa: msaa.create_view(&wgpu::TextureViewDescriptor::default()),
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
