//! Walk mode's offscreen 3D renderer: the scene is drawn into a texture (4× MSAA where the GPU
//! has it, see `gpucaps`; depth), which
//! the walk view shows as an egui image and the headless `cadaddy-walk --shot` copies to a PNG.

use cadaddy_walk3d::{Scene, Vertex};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Position (3 × f32) + normal (3 × f32) + sRGB colour (4 × u8).
const VERTEX_SIZE: u64 = 28;
const UNIFORM_SIZE: u64 = 112;
/// Largest render target side, pixels.
const MAX_SIDE: u32 = 8192;

/// Per-frame values the shader reads.
pub struct Frame {
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    /// Fragments above this height (feet) are cut away.
    pub cut_z: f32,
    pub fog_ft: f32,
}

struct Target {
    size: (u32, u32),
    /// The multisampled colour (none without MSAA: the frame is drawn into directly).
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    // Read back by headless screenshots, which the web build has none of.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    resolve: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Mesh {
    buffer: wgpu::Buffer,
    count: u32,
}

pub struct Gpu {
    tri_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    tris: Option<Mesh>,
    edges: Option<Mesh>,
    target: Option<Target>,
    samples: u32,
}

impl Gpu {
    /// Samples per pixel the pipelines draw with.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn new(device: &wgpu::Device, samples: u32) -> Gpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("walk"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("walk"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("walk"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("walk_frame"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("walk"),
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
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
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
            tri_pipeline: pipeline("walk_tris", "vs_tri", "fs_tri", wgpu::PrimitiveTopology::TriangleList, true),
            line_pipeline: pipeline("walk_edges", "vs_line", "fs_line", wgpu::PrimitiveTopology::LineList, false),
            uniform,
            bind_group,
            tris: None,
            edges: None,
            target: None,
            samples,
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, scene: &Scene) {
        self.tris = mesh(device, "walk_tris", &scene.tris);
        self.edges = mesh(device, "walk_edges", &scene.edges);
    }

    /// Draw into the offscreen target (resized to `size`); the result is [`Gpu::view`].
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), f: &Frame) {
        let size = (size.0.clamp(1, MAX_SIDE), size.1.clamp(1, MAX_SIDE));
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            self.target = Some(target(device, size, self.samples));
        }
        let Some(t) = &self.target else { return };
        let mut u: Vec<u8> = Vec::with_capacity(UNIFORM_SIZE as usize);
        for col in f.view_proj {
            for v in col {
                u.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in [0.35f32, 0.75, 0.55, 0.0, f.eye[0], f.eye[1], f.eye[2], 1.0, f.cut_z, f.fog_ft, 0.0, 0.0] {
            u.extend_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.uniform, 0, &u);
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("walk") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("walk"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: t.msaa.as_ref().unwrap_or(&t.view),
                    depth_slice: None,
                    resolve_target: t.msaa.as_ref().map(|_| &t.view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.63, g: 0.69, b: 0.76, a: 1.0 }),
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            for (pipeline, m) in [(&self.tri_pipeline, &self.tris), (&self.line_pipeline, &self.edges)] {
                if let Some(m) = m {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, m.buffer.slice(..));
                    pass.draw(0..m.count, 0..1);
                }
            }
        }
        queue.submit([enc.finish()]);
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }

    /// Copy the last frame out as RGBA rows (headless screenshots).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(u32, u32, Vec<u8>), String> {
        let t = self.target.as_ref().ok_or("nothing rendered yet")?;
        let (w, h) = t.size;
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("walk_read"),
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("walk_read") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.resolve, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let data = buf.slice(..).get_mapped_range().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let start = y * row as usize;
            out.extend_from_slice(data.get(start..start + (w * 4) as usize).ok_or("short read")?);
        }
        Ok((w, h, out))
    }
}

fn mesh(device: &wgpu::Device, label: &str, verts: &[Vertex]) -> Option<Mesh> {
    if verts.is_empty() {
        return None;
    }
    let mut bytes: Vec<u8> = Vec::with_capacity(verts.len() * VERTEX_SIZE as usize);
    for v in verts {
        for c in v.p.iter().chain(v.n.iter()) {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        bytes.extend_from_slice(&v.c);
    }
    let buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: &bytes, usage: wgpu::BufferUsages::VERTEX });
    Some(Mesh { buffer, count: u32::try_from(verts.len()).unwrap_or(u32::MAX) })
}

fn target(device: &wgpu::Device, size: (u32, u32), samples: u32) -> Target {
    let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
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
    let msaa = (samples > 1).then(|| tex("walk_msaa", FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
    let depth = tex("walk_depth", DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let resolve =
        tex("walk_frame", FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
    let view = resolve.create_view(&wgpu::TextureViewDescriptor::default());
    Target {
        size,
        msaa: msaa.map(|m| m.create_view(&wgpu::TextureViewDescriptor::default())),
        depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
        resolve,
        view,
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
