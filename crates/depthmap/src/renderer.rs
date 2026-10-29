//! GPU renderer.
//!
//! * Chunk contents live in one big `R8Uint` **atlas** texture (one 32x32
//!   slot per chunk). A chunk is re-uploaded only when its version changes.
//! * Every visible chunk of every depth becomes one 20-byte instance, and all
//!   of them are drawn with **one** instanced draw call (coarse to fine, so
//!   alpha blending stacks the ghosted parents under the active detail).
//! * Grid and cursor are a single analytic full-screen pass.

use bytemuck::{Pod, Zeroable};
use depthmap_core::camera::Camera;
use depthmap_core::colors::MAX_COLORS;
use depthmap_core::texture::{self, MAX_TEXTURES, MIP_LEVELS, TILE};
use depthmap_core::{CHUNK_SHIFT, CHUNK_SIZE, ChunkPos, MAX_DEPTH, SUBDIV, World};
use rustc_hash::FxHashMap;

/// Each ancestor layer beyond the first is this much fainter than the one
/// below it.
const GHOST_FALLOFF: f32 = 0.55;
/// Layers whose cells would be thinner than this are not drawn.
const MIN_CELL_PX: f64 = 0.5;
/// Page background (sRGB).
const BACKGROUND: [f64; 4] = [0.90, 0.87, 0.80, 1.0];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    origin: [f32; 2],
    size: f32,
    slot: u32,
    opacity: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    atlas_cols: u32,
    _pad: u32,
    colors: [[f32; 4]; MAX_COLORS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OverlayUniform {
    viewport: [f32; 2],
    ppc: f32,
    major: f32,
    frac: [f32; 2],
    cell_mod: [f32; 2],
    cursor_min: [f32; 2],
    cursor_size: f32,
    cursor_on: f32,
    grid: f32,
    _pad: f32,
}

/// Per-frame view settings.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// Cell under the mouse at the active depth, for the brush highlight.
    pub cursor: Option<(i64, i64)>,
    /// Brush radius in cells.
    pub brush_half: i64,
    pub show_grid: bool,
    /// Opacity of the layer just above the active one (`0` hides all
    /// ancestors): the "ghost of the original map".
    pub ghost_opacity: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            cursor: None,
            brush_half: 0,
            show_grid: true,
            ghost_opacity: 0.35,
        }
    }
}

struct Slot {
    index: u32,
    version: u64,
    used: u64,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct FrameStats {
    pub chunks_drawn: usize,
    pub chunks_uploaded: usize,
    /// Visible chunks that did not fit in the atlas this frame.
    pub chunks_skipped: usize,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,

    chunk_pipeline: wgpu::RenderPipeline,
    chunk_bind_group: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    atlas: wgpu::Texture,
    atlas_cols: u32,
    colors_cpu: Box<[[f32; 4]; MAX_COLORS]>,
    colors_version: u64,

    overlay_pipeline: wgpu::RenderPipeline,
    overlay_bind_group: wgpu::BindGroup,
    overlay_buf: wgpu::Buffer,

    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    instances: Vec<Instance>,

    cache: FxHashMap<(u8, ChunkPos), Slot>,
    free: Vec<u32>,
    next_unused: u32,
    frame: u64,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
        // Atlas: as large as is reasonable (8192² = 65k chunk slots = 67M cells).
        let dim = device.limits().max_texture_dimension_2d.min(8192);
        let atlas_cols = dim / CHUNK_SIZE as u32;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chunk atlas"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());

        // Texture patterns: one 32x32 layer per texture id, full mip chain.
        let pattern_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture patterns"),
            size: wgpu::Extent3d {
                width: TILE as u32,
                height: TILE as u32,
                depth_or_array_layers: MAX_TEXTURES as u32 + 1,
            },
            mip_level_count: MIP_LEVELS as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for layer in 0..=MAX_TEXTURES {
            for (level, data) in texture::mips(layer).iter().enumerate() {
                let size = (TILE >> level) as u32;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &pattern_tex,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size),
                        rows_per_image: Some(size),
                    },
                    wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                );
            }
        }
        let pattern_view = pattern_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let pattern_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pattern sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let overlay_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay"),
            size: std::mem::size_of::<OverlayUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- chunk pipeline
        let chunk_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let chunk_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chunk bind group"),
            layout: &chunk_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&pattern_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&pattern_sampler),
                },
            ],
        });
        let chunk_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunks.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/chunks.wgsl").into()),
        });
        let chunk_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chunk layout"),
            bind_group_layouts: &[Some(&chunk_bgl)],
            immediate_size: 0,
        });
        let chunk_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chunks"),
            layout: Some(&chunk_layout),
            vertex: wgpu::VertexState {
                module: &chunk_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32,
                        2 => Uint32,
                        3 => Float32,
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &chunk_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // --- overlay pipeline
        let overlay_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let overlay_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay bind group"),
            layout: &overlay_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: overlay_buf.as_entire_binding(),
            }],
        });
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/overlay.wgsl").into()),
        });
        let overlay_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay layout"),
            bind_group_layouts: &[Some(&overlay_bgl)],
            immediate_size: 0,
        });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&overlay_layout),
            vertex: wgpu::VertexState {
                module: &overlay_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &overlay_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instance_cap = 4096;
        let instance_buf = Self::make_instance_buffer(&device, instance_cap);

        Self {
            device,
            queue,
            chunk_pipeline,
            chunk_bind_group,
            globals_buf,
            atlas,
            atlas_cols,
            colors_cpu: Box::new([[0.0; 4]; MAX_COLORS]),
            colors_version: 0,
            overlay_pipeline,
            overlay_bind_group,
            overlay_buf,
            instance_buf,
            instance_cap,
            instances: Vec::with_capacity(instance_cap),
            cache: FxHashMap::default(),
            free: Vec::new(),
            next_unused: 0,
            frame: 0,
        }
    }

    fn make_instance_buffer(device: &wgpu::Device, cap: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk instances"),
            size: (cap * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Total chunk slots in the atlas.
    pub fn atlas_capacity(&self) -> u32 {
        self.atlas_cols * self.atlas_cols
    }

    fn alloc_slot(&mut self) -> Option<u32> {
        if let Some(i) = self.free.pop() {
            return Some(i);
        }
        if self.next_unused < self.atlas_capacity() {
            self.next_unused += 1;
            return Some(self.next_unused - 1);
        }
        // Atlas full: evict everything not touched this frame.
        let frame = self.frame;
        let free = &mut self.free;
        self.cache.retain(|_, s| {
            if s.used == frame {
                true
            } else {
                free.push(s.index);
                false
            }
        });
        self.free.pop()
    }

    fn upload(&self, slot: u32, cells: &[u8]) {
        let cols = self.atlas_cols;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: (slot % cols) * CHUNK_SIZE as u32,
                    y: (slot / cols) * CHUNK_SIZE as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            cells,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CHUNK_SIZE as u32 * 2), // 2 bytes per cell
                rows_per_image: Some(CHUNK_SIZE as u32),
            },
            wgpu::Extent3d {
                width: CHUNK_SIZE as u32,
                height: CHUNK_SIZE as u32,
                depth_or_array_layers: 1,
            },
        );
    }

    fn opacity_for(active: u8, k: u8, ghost: f32) -> f32 {
        if k >= active {
            1.0
        } else {
            ghost * GHOST_FALLOFF.powi((active - k - 1) as i32)
        }
    }

    /// Draws the world into `target`.
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        size: (u32, u32),
        world: &World,
        camera: &Camera,
        opts: &RenderOptions,
    ) -> FrameStats {
        self.frame += 1;
        let mut stats = FrameStats::default();
        let (vw, vh) = (size.0 as f64, size.1 as f64);
        self.instances.clear();

        // ---- gather visible chunks of every drawable depth, coarse to fine
        for k in 0..=MAX_DEPTH {
            if world.layer_is_empty(k) {
                continue;
            }
            let ppc_k = camera.ppc * (SUBDIV as f64).powi(camera.depth as i32 - k as i32);
            if ppc_k < MIN_CELL_PX {
                continue;
            }
            let opacity = Self::opacity_for(camera.depth, k, opts.ghost_opacity);
            if opacity < 0.03 {
                continue;
            }
            let view = camera.at_depth(k);
            let ((x0, y0), (x1, y1)) = view.visible_cells(vw, vh);
            let min = ChunkPos { x: x0 >> CHUNK_SHIFT, y: y0 >> CHUNK_SHIFT };
            let max = ChunkPos { x: x1 >> CHUNK_SHIFT, y: y1 >> CHUNK_SHIFT };
            let chunk_px = (CHUNK_SIZE as f64 * view.ppc) as f32;

            // Split borrows: the closure needs the cache, free list and queue.
            let mut pending: Vec<(ChunkPos, u32)> = Vec::new();
            world.visit_chunks(k, min, max, |pos, chunk| {
                let slot = match self.cache.get_mut(&(k, pos)) {
                    Some(s) => {
                        s.used = self.frame;
                        if s.version != chunk.version() {
                            s.version = chunk.version();
                            let idx = s.index;
                            pending.push((pos, idx));
                            stats.chunks_uploaded += 1;
                        }
                        Some(s.index)
                    }
                    None => match self.alloc_slot() {
                        Some(idx) => {
                            self.cache.insert(
                                (k, pos),
                                Slot { index: idx, version: chunk.version(), used: self.frame },
                            );
                            pending.push((pos, idx));
                            stats.chunks_uploaded += 1;
                            Some(idx)
                        }
                        None => None,
                    },
                };
                match slot {
                    Some(idx) => {
                        let (ox, oy) = chunk_origin(pos);
                        let (px, py) = view.cell_origin_px(ox, oy, vw, vh);
                        self.instances.push(Instance {
                            origin: [px as f32, py as f32],
                            size: chunk_px,
                            slot: idx,
                            opacity,
                        });
                    }
                    None => stats.chunks_skipped += 1,
                }
            });
            for (pos, idx) in pending {
                if let Some(chunk) = world.chunk(k, pos) {
                    self.upload(idx, bytemuck::cast_slice(chunk.cells()));
                }
            }
        }
        stats.chunks_drawn = self.instances.len();

        // ---- upload per-frame data
        if self.instances.len() > self.instance_cap {
            self.instance_cap = self.instances.len().next_power_of_two();
            self.instance_buf = Self::make_instance_buffer(&self.device, self.instance_cap);
        }
        if !self.instances.is_empty() {
            self.queue
                .write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&self.instances));
        }
        if self.colors_version != world.colors.version() {
            self.colors_version = world.colors.version();
            for (dst, src) in self.colors_cpu.iter_mut().zip(world.colors.as_slice()) {
                *dst = [
                    src[0] as f32 / 255.0,
                    src[1] as f32 / 255.0,
                    src[2] as f32 / 255.0,
                    1.0,
                ];
            }
        }
        let globals = Globals {
            viewport: [size.0 as f32, size.1 as f32],
            atlas_cols: self.atlas_cols,
            _pad: 0,
            colors: *self.colors_cpu,
        };
        self.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        let view = camera.view();
        let half = opts.brush_half as f32;
        let cursor = opts.cursor;
        let overlay = OverlayUniform {
            viewport: [size.0 as f32, size.1 as f32],
            ppc: camera.ppc as f32,
            major: if camera.depth > 0 { 1.0 } else { 0.0 },
            frac: [view.frac.0 as f32, view.frac.1 as f32],
            cell_mod: [
                view.cell.0.rem_euclid(SUBDIV) as f32,
                view.cell.1.rem_euclid(SUBDIV) as f32,
            ],
            cursor_min: cursor.map_or([0.0; 2], |(cx, cy)| {
                [
                    (cx - view.cell.0) as f32 - half,
                    (cy - view.cell.1) as f32 - half,
                ]
            }),
            cursor_size: 2.0 * half + 1.0,
            cursor_on: if cursor.is_some() { 1.0 } else { 0.0 },
            grid: if opts.show_grid { 1.0 } else { 0.0 },
            _pad: 0.0,
        };
        self.queue
            .write_buffer(&self.overlay_buf, 0, bytemuck::bytes_of(&overlay));

        // ---- record and submit
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: BACKGROUND[0],
                            g: BACKGROUND[1],
                            b: BACKGROUND[2],
                            a: BACKGROUND[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !self.instances.is_empty() {
                pass.set_pipeline(&self.chunk_pipeline);
                pass.set_bind_group(0, &self.chunk_bind_group, &[]);
                let bytes = (self.instances.len() * std::mem::size_of::<Instance>()) as u64;
                pass.set_vertex_buffer(0, self.instance_buf.slice(..bytes));
                pass.draw(0..4, 0..self.instances.len() as u32);
            }
            pass.set_pipeline(&self.overlay_pipeline);
            pass.set_bind_group(0, &self.overlay_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        stats
    }
}

#[inline]
fn chunk_origin(pos: ChunkPos) -> (i64, i64) {
    pos.origin()
}
