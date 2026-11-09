//! The 3D view: the map as a heightfield, seen isometrically or from straight
//! above with relief shading. Height comes from the tile and colour of each
//! cell (`World::cell_height`). Only the layer nearest the camera's depth is
//! drawn; empty cells show the parent layer through, as on the flat map.

use crate::renderer::Renderer;
use bytemuck::{Pod, Zeroable};
use depthmap_core::{Camera, EMPTY, SUBDIV, World};
use depthmap_core::{Cell, cell_color, cell_texture};

/// Most cells either side of the camera that are drawn; farther out the view
/// switches to a coarser layer.
const MAX_HALF: i64 = 96;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const MAX_HEIGHT_CELLS: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode3d {
    /// Fixed angle from above and to the side.
    Isometric,
    /// Straight down; height shows as shading.
    TopDown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View3d {
    pub mode: Mode3d,
    /// Turn of the map about the camera, in radians.
    pub yaw: f32,
    /// How many cells tall the highest possible column (255) is.
    pub height_cells: f32,
}

impl Default for View3d {
    fn default() -> Self {
        Self { mode: Mode3d::Isometric, yaw: std::f32::consts::FRAC_PI_4, height_cells: 8.0 }
    }
}

impl View3d {
    /// Sine and cosine of the camera's elevation above the ground.
    fn elevation(&self) -> (f32, f32) {
        match self.mode {
            Mode3d::Isometric => {
                let e = 35.264_f32.to_radians();
                (e.sin(), e.cos())
            }
            Mode3d::TopDown => (1.0, 0.0),
        }
    }
}

/// One drawn face of one column.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Face {
    pub pos: [f32; 2],
    pub z: [f32; 2],
    pub kind: u32,
    pub tex: u32,
    pub color: u32,
    pub shade: f32,
}

const TOP: u32 = 0;
const SOUTH: u32 = 1;
const EAST: u32 = 2;
const NORTH: u32 = 3;
const WEST: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    ppc: f32,
    depth_range: f32,
    frac: [f32; 2],
    yaw: [f32; 2],
    elev: [f32; 2],
    lod_ppc: f32,
    _pad: f32,
    background: [f32; 4],
    colors: [[f32; 4]; 256],
}

/// The cell at `(x, y)` of depth `k`, or what shows through from the parent
/// layers when it is empty. `None` outside the map.
fn effective(world: &World, k: u8, x: i64, y: i64) -> Option<Cell> {
    if !world.in_bounds(k, x, y) {
        return None;
    }
    for kk in (0..=k).rev() {
        let s = SUBDIV.pow((k - kk) as u32);
        let c = world.get(kk, x.div_euclid(s), y.div_euclid(s));
        if c != EMPTY {
            return Some(c);
        }
    }
    Some(EMPTY)
}

/// What [`faces`] decided to draw.
pub struct Scene {
    pub faces: Vec<Face>,
    /// Layer the faces are cells of.
    pub depth: u8,
    /// The camera's cell and position inside it, at that layer.
    pub centre: (i64, i64),
    pub frac: (f32, f32),
    /// Pixels per cell at that layer.
    pub ppc: f64,
}

/// Picks the layer to draw for this camera and builds the column faces.
pub fn faces(world: &World, camera: &Camera, size: (u32, u32), view: &View3d) -> Scene {
    // Choose the finest layer, no deeper than the camera's, whose visible
    // square stays within MAX_HALF cells each way.
    let reach = (size.0.max(size.1) as f64) * 0.75;
    let mut k = camera.depth.min(world.spec().max_depth);
    let mut lv = camera.at_depth(k);
    while k > 0 && reach / lv.ppc > MAX_HALF as f64 {
        k -= 1;
        lv = camera.at_depth(k);
    }
    let half = ((reach / lv.ppc).ceil() as i64 + 2).clamp(3, MAX_HALF);
    let (cx, cy) = lv.cell;
    let (x0, y0) = (cx - half, cy - half);
    let n = (2 * half + 1) as usize;

    // Cells one wider on every side, so every column knows its neighbours.
    let w = n + 2;
    let mut cells: Vec<Option<Cell>> = Vec::with_capacity(w * w);
    for j in 0..w as i64 {
        for i in 0..w as i64 {
            cells.push(effective(world, k, x0 - 1 + i, y0 - 1 + j));
        }
    }
    let scale = view.height_cells / 255.0;
    let height = |c: &Option<Cell>| c.map_or(0.0, |c| world.cell_height(c) as f32 * scale);
    let at = |i: usize, j: usize| &cells[j * w + i];

    let (yc, ys) = (view.yaw.cos(), view.yaw.sin());
    // Light comes from the upper left of the screen; turn it into map space.
    let (lx, ly) = (-0.6_f32, -0.8_f32);
    let light = (lx * yc + ly * ys, -lx * ys + ly * yc);

    let mut faces = Vec::with_capacity(n * n * 2);
    for j in 1..=n {
        for i in 1..=n {
            let Some(c) = *at(i, j) else { continue };
            if c == EMPTY {
                continue;
            }
            let h = height(at(i, j));
            let pos = [(x0 + i as i64 - 1 - cx) as f32, (y0 + j as i64 - 1 - cy) as f32];
            let (tex, color) = (cell_texture(c) as u32, cell_color(c) as u32);
            let top = if view.mode == Mode3d::TopDown {
                // Relief shading from the slope towards the light.
                let gx = height(at(i + 1, j)) - height(at(i - 1, j));
                let gy = height(at(i, j + 1)) - height(at(i, j - 1));
                (1.0 + 1.6 * (gx * light.0 + gy * light.1)).clamp(0.55, 1.45)
            } else {
                1.0
            };
            faces.push(Face { pos, z: [h, h], kind: TOP, tex, color, shade: top });
            if view.mode == Mode3d::Isometric {
                for (kind, di, dj) in [(SOUTH, 0, 1), (EAST, 1, 0), (NORTH, 0, -1), (WEST, -1, 0)] {
                    let nh = height(at((i as i32 + di) as usize, (j as i32 + dj) as usize));
                    if nh < h {
                        faces.push(Face { pos, z: [nh, h], kind, tex, color, shade: 1.0 });
                    }
                }
            }
        }
    }
    Scene { faces, depth: k, centre: (cx, cy), frac: (lv.frac.0 as f32, lv.frac.1 as f32), ppc: lv.ppc }
}

pub struct Iso {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    globals: wgpu::Buffer,
    faces: wgpu::Buffer,
    face_cap: usize,
    depth: Option<(wgpu::Texture, (u32, u32))>,
}

impl Iso {
    pub fn new(renderer: &Renderer, target_format: wgpu::TextureFormat) -> Self {
        let device = renderer.device().clone();
        let queue = renderer.queue().clone();
        let patterns = renderer.pattern_texture().create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bgl"),
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain bind group"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&patterns) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Face>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Uint32,
                        3 => Uint32,
                        4 => Uint32,
                        5 => Float32,
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let faces = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain faces"),
            size: 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { device, queue, pipeline, bind_group, globals, faces, face_cap: 4096 / std::mem::size_of::<Face>(), depth: None }
    }

    /// Draws the world into `target` (replacing what is there).
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        target: &wgpu::TextureView,
        size: (u32, u32),
        world: &World,
        camera: &Camera,
        view: &View3d,
    ) {
        renderer.sync_tiles(world);
        let scene = faces(world, camera, size, view);

        if scene.faces.len() > self.face_cap {
            self.face_cap = scene.faces.len().next_power_of_two();
            self.faces = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("terrain faces"),
                size: (self.face_cap * std::mem::size_of::<Face>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !scene.faces.is_empty() {
            self.queue.write_buffer(&self.faces, 0, bytemuck::cast_slice(&scene.faces));
        }
        if self.depth.as_ref().is_none_or(|(_, s)| *s != size) {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain depth"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((tex, size));
        }
        let depth_view = self.depth.as_ref().unwrap().0.create_view(&Default::default());

        let mut colors = [[0.0f32; 4]; 256];
        for (dst, src) in colors.iter_mut().zip(world.colors.as_slice()) {
            *dst = [src[0] as f32 / 255.0, src[1] as f32 / 255.0, src[2] as f32 / 255.0, 1.0];
        }
        let (es, ec) = view.elevation();
        let globals = Globals {
            viewport: [size.0 as f32, size.1 as f32],
            ppc: scene.ppc as f32,
            depth_range: 2.0 * MAX_HALF as f32 + 2.0 * MAX_HEIGHT_CELLS,
            frac: [scene.frac.0, scene.frac.1],
            yaw: [view.yaw.cos(), view.yaw.sin()],
            elev: [es, ec],
            lod_ppc: scene.ppc as f32,
            _pad: 0.0,
            background: [230.0 / 255.0, 222.0 / 255.0, 204.0 / 255.0, 1.0],
            colors,
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: globals.background[0] as f64,
                            g: globals.background[1] as f64,
                            b: globals.background[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !scene.faces.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.faces.slice(..));
                pass.draw(0..4, 0..scene.faces.len() as u32);
            }
        }
        self.queue.submit([enc.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use depthmap_core::{MapSpec, make_cell};

    fn world() -> World {
        let mut w = World::with_spec(MapSpec::new(16, 16, 2));
        for y in 4..8 {
            for x in 4..8 {
                w.set(0, x, y, make_cell(4, 1)); // brick: tall
            }
        }
        w.set(0, 10, 10, make_cell(9, 2)); // water: flat
        w
    }

    fn scene(w: &World, view: &View3d) -> Scene {
        let mut cam = Camera::new();
        cam.max_depth = w.spec().max_depth;
        cam.cell = (6, 6);
        faces(w, &cam, (800, 600), view)
    }

    #[test]
    fn isometric_draws_tops_and_the_walls_that_rise_above_neighbours() {
        let w = world();
        let s = scene(&w, &View3d::default());
        let tops = s.faces.iter().filter(|f| f.kind == TOP).count();
        assert_eq!(tops, 16 + 1, "one top per non-empty cell");
        // The 4x4 block is taller than the empty floor around it: 16 wall
        // faces on its outer edge, none inside it. Water (height 0) has none.
        let walls = s.faces.iter().filter(|f| f.kind != TOP).count();
        assert_eq!(walls, 16);
        assert!(s.faces.iter().filter(|f| f.kind != TOP).all(|f| f.z[1] > f.z[0]));
    }

    #[test]
    fn top_down_has_no_walls_but_shades_slopes() {
        let w = world();
        let s = scene(&w, &View3d { mode: Mode3d::TopDown, ..Default::default() });
        assert!(s.faces.iter().all(|f| f.kind == TOP));
        let shades: Vec<f32> = s.faces.iter().map(|f| f.shade).collect();
        assert!(shades.iter().any(|&v| v < 0.99) && shades.iter().any(|&v| v > 1.01), "{shades:?}");
    }

    #[test]
    fn heights_follow_tiles_and_colours_and_the_scale() {
        let mut w = world();
        let v = View3d { height_cells: 10.0, ..Default::default() };
        let tall = |w: &World| {
            scene(w, &v).faces.iter().filter(|f| f.kind == TOP).map(|f| f.z[1]).fold(0.0, f32::max)
        };
        let before = tall(&w);
        assert!((before - w.tile_height(4) as f32 * 10.0 / 255.0).abs() < 1e-4);
        w.set_color_height(1, 100);
        assert!(tall(&w) > before);
    }

    #[test]
    fn zooming_out_uses_a_coarser_layer_so_the_cell_count_stays_bounded() {
        let w = world();
        let mut cam = Camera::new();
        cam.max_depth = 2;
        cam.depth = 2;
        cam.ppc = 4.0; // 800 px wide at 4 px per cell: 150 cells each way
        let s = faces(&w, &cam, (800, 600), &View3d::default());
        assert!(s.depth < 2, "chose depth {}", s.depth);
        assert!(s.faces.len() < 100_000);
    }

    #[test]
    fn empty_cells_show_the_parent_layer_through() {
        let w = world();
        let mut cam = Camera::new();
        cam.max_depth = 2;
        cam.depth = 1;
        cam.cell = (24, 24);
        cam.ppc = 32.0;
        let s = faces(&w, &cam, (800, 600), &View3d::default());
        assert_eq!(s.depth, 1);
        assert!(s.faces.iter().any(|f| f.kind == TOP && f.tex == 4), "the depth-0 bricks show at depth 1");
    }
}
