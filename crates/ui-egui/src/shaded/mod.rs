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
            g.upload(&rs.device, &scene, style, bg);
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
/// Position (3 × f32, from the scene's origin) + normal (3 × f32) + sRGB colour (4 × u8).
const VERTEX_SIZE: u64 = 28;
/// The view-projection matrix and four vectors.
const UNIFORM_SIZE: u64 = 128;

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
    line_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    tris: Option<Buffer>,
    lines: Option<Buffer>,
    /// The scene's centre (vertices are stored relative to it, keeping f32 precision) and its
    /// extent, for the depth range.
    origin: Vec3,
    radius: f64,
    target: Option<Target>,
}

impl Gpu {
    fn new(device: &wgpu::Device) -> Gpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaded"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shaded"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shaded"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shaded_frame"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shaded"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4];
        let pipeline = |label: &str, vs: &str, fs: &str, topology: wgpu::PrimitiveTopology, write_depth: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_SIZE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRS,
                    })],
                    compilation_options: Default::default(),
                },
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
                    targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Gpu {
            tri_pipeline: pipeline("shaded_tris", "vs_tri", "fs_tri", wgpu::PrimitiveTopology::TriangleList, true),
            line_pipeline: pipeline("shaded_lines", "vs_line", "fs_line", wgpu::PrimitiveTopology::LineList, false),
            uniform,
            bind_group,
            tris: None,
            lines: None,
            origin: Vec3::ZERO,
            radius: 1.0,
            target: None,
        }
    }

    /// The scene's triangles (none for Wireframe) and the lines `style` shows, as vertex data.
    fn upload(&mut self, device: &wgpu::Device, scene: &Scene, style: VisualStyle, bg: Rgb) {
        let pts = scene.tris.iter().flat_map(|t| t.p).chain(scene.lines.iter().flat_map(|l| [l.a, l.b]));
        let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for p in pts {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        self.origin = if lo.x <= hi.x { (lo + hi) * 0.5 } else { Vec3::ZERO };
        self.radius = if lo.x <= hi.x { ((hi - lo).len() * 0.5).max(1e-9) } else { 1.0 };
        let o = self.origin;
        let rel = |p: Vec3| [(p.x - o.x) as f32, (p.y - o.y) as f32, (p.z - o.z) as f32];
        let mut tris: Vec<u8> = Vec::new();
        if style != VisualStyle::Wireframe {
            tris.reserve(scene.tris.len() * 3 * VERTEX_SIZE as usize);
            for t in &scene.tris {
                let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
                let n = if n.is_finite() { [n.x as f32, n.y as f32, n.z as f32] } else { [0.0, 0.0, 1.0] };
                for p in t.p {
                    push_vertex(&mut tris, rel(p), n, t.color);
                }
            }
        }
        let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges);
        let dark = edge_ink(bg);
        let mut lines: Vec<u8> = Vec::new();
        for l in scene.lines.iter().filter(|l| !l.edge || edges) {
            let c = if l.edge && style == VisualStyle::ShadedEdges { dark } else { l.color };
            push_vertex(&mut lines, rel(l.a), [0.0, 0.0, 1.0], c);
            push_vertex(&mut lines, rel(l.b), [0.0, 0.0, 1.0], c);
        }
        self.tris = buffer(device, "shaded_tris", &tris);
        self.lines = buffer(device, "shaded_lines", &lines);
    }

    /// Draw into the offscreen target sized like the view.
    fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, v: &ShadeView, style: VisualStyle, bg: Rgb) {
        let size = (v.width_px, v.height_px);
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(target(device, size));
        }
        let Some(t) = &self.target else { return };
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
        let light = (v.dir * 0.6 + v.up * 0.6 - v.right * 0.35).normalized();
        let mut u: Vec<u8> = Vec::with_capacity(UNIFORM_SIZE as usize);
        for col in 0..4 {
            for row in &rows {
                u.extend_from_slice(&(row.get(col).copied().unwrap_or(0.0) as f32).to_le_bytes());
            }
        }
        let hidden = if style == VisualStyle::Hidden { 1.0f32 } else { 0.0 };
        let spec = if style == VisualStyle::Realistic { 1.0f32 } else { 0.0 };
        let f = |c: u8| f32::from(c) / 255.0;
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
            hidden,
            0.0,
            0.0,
            0.0,
        ] {
            u.extend_from_slice(&x.to_le_bytes());
        }
        queue.write_buffer(&self.uniform, 0, &u);
        // The target holds display values: the background as it is.
        let lin = |c: u8| f64::from(c) / 255.0;
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shaded") });
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            for (pipeline, m) in [(&self.tri_pipeline, &self.tris), (&self.line_pipeline, &self.lines)] {
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

fn push_vertex(out: &mut Vec<u8>, p: [f32; 3], n: [f32; 3], c: Rgb) {
    for x in p.iter().chain(n.iter()) {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&[c.0, c.1, c.2, 255]);
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
