//! A sprite batcher: each frame, push textured quads in drawing order, then draw them all in one render pass. Quads
//! that share a texture and follow each other go in one draw call.

use super::gpu::{Gpu, OFFSCREEN_FORMAT};

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// A texture the batch owns. `TexId(0)` is a single white pixel, for solid colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TexId(pub usize);

pub const WHITE: TexId = TexId(0);

struct Texture {
    bind: wgpu::BindGroup,
    width: f32,
    height: f32,
}

/// Bytes per vertex: position and texture coordinate (four f32) and a tint (four u8).
const VERTEX: usize = 20;

pub struct SpriteBatch {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    screen: wgpu::Buffer,
    screen_bind: wgpu::BindGroup,
    textures: Vec<Texture>,
    vertices: Vec<u8>,
    /// Runs of quads drawn with one texture: (texture, first quad, quad count).
    runs: Vec<(TexId, u32, u32)>,
    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
}

impl SpriteBatch {
    /// A batch that draws into targets of colour `format`.
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> SpriteBatch {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite"),
            bind_group_layouts: &[Some(&screen_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Nearest-pixel sampling keeps pixel art sharp at any zoom.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pixels"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let screen = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("screen"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() }],
        });
        let mut batch = SpriteBatch {
            pipeline,
            texture_layout,
            sampler,
            screen,
            screen_bind,
            textures: Vec::new(),
            vertices: Vec::new(),
            runs: Vec::new(),
            vertex_buffer: None,
            index_buffer: None,
        };
        batch.texture(gpu, 1, 1, &[255, 255, 255, 255]);
        batch
    }

    /// Upload an RGBA image (8 bits a channel, rows top to bottom) and return its id.
    pub fn texture(&mut self, gpu: &Gpu, width: u32, height: u32, rgba: &[u8]) -> TexId {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "RGBA size");
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.textures.push(Texture { bind, width: width as f32, height: height as f32 });
        TexId(self.textures.len() - 1)
    }

    /// Queue part `src` (in the texture's pixels) of a texture, drawn over `dst` (in screen pixels) and multiplied
    /// by `tint`.
    pub fn sprite(&mut self, tex: TexId, src: Rect, dst: Rect, tint: [u8; 4]) {
        let t = &self.textures[tex.0];
        let (u0, v0) = (src.x / t.width, src.y / t.height);
        let (u1, v1) = ((src.x + src.w) / t.width, (src.y + src.h) / t.height);
        for (x, y, u, v) in [
            (dst.x, dst.y, u0, v0),
            (dst.x + dst.w, dst.y, u1, v0),
            (dst.x + dst.w, dst.y + dst.h, u1, v1),
            (dst.x, dst.y + dst.h, u0, v1),
        ] {
            for f in [x, y, u, v] {
                self.vertices.extend_from_slice(&f.to_le_bytes());
            }
            self.vertices.extend_from_slice(&tint);
        }
        let quad = (self.vertices.len() / VERTEX / 4 - 1) as u32;
        match self.runs.last_mut() {
            Some((t, _, n)) if *t == tex => *n += 1,
            _ => self.runs.push((tex, quad, 1)),
        }
    }

    /// Queue a solid rectangle.
    pub fn fill(&mut self, dst: Rect, colour: [u8; 4]) {
        self.sprite(WHITE, Rect::new(0.0, 0.0, 1.0, 1.0), dst, colour);
    }

    /// Queue a rectangle's outline, `width` pixels thick, drawn inside it.
    pub fn outline(&mut self, r: Rect, width: f32, colour: [u8; 4]) {
        self.fill(Rect::new(r.x, r.y, r.w, width), colour);
        self.fill(Rect::new(r.x, r.y + r.h - width, r.w, width), colour);
        self.fill(Rect::new(r.x, r.y, width, r.h), colour);
        self.fill(Rect::new(r.x + r.w - width, r.y, width, r.h), colour);
    }

    /// Draw everything queued into `target`, `width` by `height` pixels, over `clear`, and empty the queue.
    pub fn draw(&mut self, gpu: &Gpu, target: &wgpu::TextureView, width: u32, height: u32, clear: [u8; 4]) {
        let quads = self.vertices.len() / VERTEX / 4;
        let mut screen = Vec::with_capacity(16);
        for f in [width as f32, height as f32, 0.0, 0.0] {
            screen.extend_from_slice(&f.to_le_bytes());
        }
        gpu.queue.write_buffer(&self.screen, 0, &screen);
        if quads > 0 {
            self.upload(gpu, quads);
        }
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let c = |v: u8| v as f64 / 255.0;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprites"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c(clear[0]),
                            g: c(clear[1]),
                            b: c(clear[2]),
                            a: c(clear[3]),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (Some(vb), Some(ib)) = (&self.vertex_buffer, &self.index_buffer)
                && quads > 0
            {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.screen_bind, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                for &(tex, first, count) in &self.runs {
                    pass.set_bind_group(1, &self.textures[tex.0].bind, &[]);
                    pass.draw_indexed(first * 6..(first + count) * 6, 0, 0..1);
                }
            }
        }
        gpu.queue.submit([encoder.finish()]);
        self.vertices.clear();
        self.runs.clear();
    }

    /// Copy the queued vertices to the GPU, growing the buffers when they are too small.
    fn upload(&mut self, gpu: &Gpu, quads: usize) {
        let need = self.vertices.len() as u64;
        if self.vertex_buffer.as_ref().is_none_or(|b| b.size() < need) {
            let size = need.next_power_of_two().max(4096);
            self.vertex_buffer = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vertices"),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            let capacity = size as usize / VERTEX / 4;
            let mut indices = Vec::with_capacity(capacity * 6 * 4);
            for q in 0..capacity as u32 {
                for i in [0, 1, 2, 0, 2, 3] {
                    indices.extend_from_slice(&(q * 4 + i).to_le_bytes());
                }
            }
            let ib = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indices"),
                size: indices.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue.write_buffer(&ib, 0, &indices);
            self.index_buffer = Some(ib);
        }
        debug_assert!(quads * VERTEX * 4 == self.vertices.len());
        gpu.queue.write_buffer(self.vertex_buffer.as_ref().expect("made above"), 0, &self.vertices);
    }

    /// Draw everything queued into a new `width` by `height` image and return its RGBA pixels, rows top to bottom.
    /// For tests and screenshots; needs no window.
    pub fn draw_to_image(&mut self, gpu: &Gpu, width: u32, height: u32, clear: [u8; 4]) -> Vec<u8> {
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.draw(gpu, &view, width, height, clear);
        // Rows in a copy are padded to a multiple of 256 bytes.
        let row = (width * 4).div_ceil(256) * 256;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("copy") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("readback maps"));
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("GPU finishes");
        let data = slice.get_mapped_range().expect("readback is mapped");
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let start = (y * row) as usize;
            out.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        out
    }
}
