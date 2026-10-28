//! Renders offscreen and checks real pixels. Skips (with a message) when the
//! machine has no GPU or software Vulkan/GL adapter.
//! Set `DEPTHMAP_PNG_DIR` to also dump the frames as PNGs.

use depthmap::renderer::Renderer;
use depthmap_core::palette::MATERIALS;
use depthmap_core::{Camera, World};

const W: u32 = 512;
const H: u32 = 384;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    target: wgpu::Texture,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: true,
            ..Default::default()
        }))
    })
    .ok()?;
    eprintln!("adapter: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .ok()?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let renderer = Renderer::new(device.clone(), queue.clone(), format);
    Some(Gpu { device, queue, renderer, target })
}

impl Gpu {
    fn render(
        &mut self,
        world: &World,
        cam: &Camera,
        cursor: Option<(i64, i64)>,
        name: &str,
    ) -> (Vec<u8>, depthmap::renderer::FrameStats) {
        let view = self.target.create_view(&Default::default());
        let stats = self.renderer.render(&view, (W, H), world, cam, cursor, 0);

        let bpr = W * 4; // 2048 = multiple of 256
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * H) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range().unwrap().to_vec();

        if let Ok(dir) = std::env::var("DEPTHMAP_PNG_DIR") {
            let path = std::path::Path::new(&dir).join(format!("{name}.png"));
            let file = std::fs::File::create(path).unwrap();
            let mut enc = png::Encoder::new(file, W, H);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&data).unwrap();
        }
        (data, stats)
    }
}

fn px(data: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

fn material_rgb(m: usize) -> [u8; 3] {
    let c = MATERIALS[m].rgba;
    [c[0], c[1], c[2]]
}

/// Centre of a cell (at the camera's active depth) in screen pixels.
fn cell_centre(cam: &Camera, cell: (i64, i64)) -> (u32, u32) {
    let v = cam.view();
    let (x, y) = v.cell_origin_px(cell.0, cell.1, W as f64, H as f64);
    ((x + v.ppc * 0.5) as u32, (y + v.ppc * 0.5) as u32)
}

#[test]
fn renders_cells_with_palette_colours() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let mut world = World::new();
    // 3 materials in a row, one cell left of the origin to cover negative chunks.
    world.set(0, -1, 0, 1);
    world.set(0, 0, 0, 5);
    world.set(0, 1, 0, 9);
    let cam = Camera::new(); // depth 0, 32 px/cell, centred on cell (0,0)

    let (img, stats) = gpu.render(&world, &cam, None, "basic");
    assert_eq!(stats.chunks_drawn, 2, "cells span two chunks (x=-1 and x>=0)");
    assert_eq!(stats.chunks_uploaded, 2);

    for (cell, m) in [((-1, 0), 1usize), ((0, 0), 5), ((1, 0), 9)] {
        let (x, y) = cell_centre(&cam, cell);
        assert!(
            near(px(&img, x + 3, y + 3), material_rgb(m), 3),
            "cell {cell:?}: got {:?}, want {:?}",
            px(&img, x + 3, y + 3),
            material_rgb(m)
        );
    }
    // Empty cell shows the parchment background.
    let (x, y) = cell_centre(&cam, (3, 0));
    assert!(near(px(&img, x + 3, y + 3), [230, 222, 204], 4));

    // Second frame: nothing changed, nothing re-uploaded.
    let (_, stats) = gpu.render(&world, &cam, None, "basic2");
    assert_eq!(stats.chunks_uploaded, 0);

    // Editing one cell re-uploads exactly its chunk.
    world.set(0, 0, 0, 2);
    let (img, stats) = gpu.render(&world, &cam, None, "edited");
    assert_eq!(stats.chunks_uploaded, 1);
    let (x, y) = cell_centre(&cam, (0, 0));
    assert!(near(px(&img, x + 3, y + 3), material_rgb(2), 3));
}

#[test]
fn parent_layer_is_ghosted_behind_the_detail_layer() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let mut world = World::new();
    world.set(0, 0, 0, 8); // lava cell at depth 0

    // Zoom in until the camera sits at depth 1 and the lava cell fills the view.
    let mut cam = Camera::new();
    cam.zoom_at(2.5, W as f64 * 0.5, H as f64 * 0.5, W as f64, H as f64);
    assert_eq!(cam.depth, 1, "ppc = {}", cam.ppc);
    // Put the camera in the middle of the lava cell: depth-1 cell (2,2).
    cam.cell = (2, 2);
    cam.frac = (0.5, 0.5);

    // One detail cell at depth 1, directly right of the centre.
    world.set(1, 3, 2, 9);

    let (img, _) = gpu.render(&world, &cam, None, "ghost");
    let (cx, cy) = (W / 2, H / 2);

    // Where the detail layer is empty we see the parent as a faded tint:
    // strictly between pure lava and the background.
    let lava = material_rgb(8);
    let bg = [230u8, 222, 204];
    let ghost = px(&img, cx, cy - 20);
    for i in 0..3 {
        let (lo, hi) = (lava[i].min(bg[i]), lava[i].max(bg[i]));
        assert!(ghost[i] >= lo && ghost[i] <= hi, "channel {i}: {ghost:?}");
    }
    assert!(!near(ghost, lava, 20) && !near(ghost, bg, 10), "not faded: {ghost:?}");

    // The detail cell is drawn at full strength on top.
    let (dx, dy) = cell_centre(&cam, (3, 2));
    assert!(near(px(&img, dx, dy), material_rgb(9), 3));
}

#[test]
fn grid_and_cursor_are_drawn() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let world = World::new();
    let cam = Camera::new();
    let (plain, _) = gpu.render(&world, &cam, None, "grid");
    // A grid line sits on the cell edge at x = W/2 (frac 0 => cell edge in the middle).
    let on_line = px(&plain, W / 2, 50);
    let off_line = px(&plain, W / 2 + 10, 50);
    assert!(on_line[0] < off_line[0] - 10, "grid line missing: {on_line:?} vs {off_line:?}");

    let (with_cursor, _) = gpu.render(&world, &cam, Some((2, 2)), "cursor");
    let (x, y) = cell_centre(&cam, (2, 2));
    assert!(px(&with_cursor, x, y)[0] > px(&plain, x, y)[0], "cursor highlight missing");
}

#[test]
fn stress_map_renders_and_reports_every_chunk() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let mut world = World::new();
    depthmap::stress_fill(&mut world, 2048); // 4 million cells at depth 0 + detail at depth 1
    let mut cam = Camera::new();
    cam.ppc = 4.0; // far zoomed out: the whole 128x96 cell viewport fits ~ 4x3 chunks per 512px
    let (img, stats) = gpu.render(&world, &cam, None, "stress");
    assert!(stats.chunks_drawn > 10);
    assert_eq!(stats.chunks_skipped, 0);
    // Not just background.
    let colourful = (0..W).step_by(7).filter(|&x| !near(px(&img, x, 100), [230, 222, 204], 6)).count();
    assert!(colourful > 5);
}
