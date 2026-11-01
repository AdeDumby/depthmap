//! Renders offscreen and checks real pixels. Skips (with a message) when the
//! machine has no GPU or software Vulkan/GL adapter.
//! Set `DEPTHMAP_PNG_DIR` to also dump the frames as PNGs.

use depthmap::renderer::{RenderOptions, Renderer};
use depthmap_core::colors::DEFAULT_COLORS;
use depthmap_core::texture;
use depthmap_core::{Camera, MapSpec, World, make_cell};

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
        opts: &RenderOptions,
        name: &str,
    ) -> (Vec<u8>, depthmap::renderer::FrameStats) {
        let view = self.target.create_view(&Default::default());
        let stats = self.renderer.render(&view, (W, H), world, cam, opts);

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

const NO_GRID: RenderOptions = RenderOptions {
    cursor: None,
    brush_half: 0,
    show_grid: false,
    checker: false,
    ghost_opacity: 0.35,
};

fn color(i: usize) -> [u8; 3] {
    DEFAULT_COLORS[i]
}

/// Centre of a cell (at the camera's active depth) in screen pixels.
fn cell_centre(cam: &Camera, cell: (i64, i64)) -> (u32, u32) {
    let v = cam.view();
    let (x, y) = v.cell_origin_px(cell.0, cell.1, W as f64, H as f64);
    ((x + v.ppc * 0.5) as u32, (y + v.ppc * 0.5) as u32)
}

#[test]
fn solid_cells_render_in_their_colour() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let mut world = World::new();
    // Three colours in a row, one cell left of the origin to cover negative chunks.
    world.set(0, -1, 0, make_cell(1, 5)); // grass green
    world.set(0, 0, 0, make_cell(1, 9)); // lava
    world.set(0, 1, 0, make_cell(1, 8)); // deep water
    let cam = Camera::new(); // depth 0, 32 px/cell, centred on cell (0,0)

    let (img, stats) = gpu.render(&world, &cam, &NO_GRID, "basic");
    assert_eq!(stats.chunks_drawn, 2, "cells span two chunks (x=-1 and x>=0)");
    assert_eq!(stats.chunks_uploaded, 2);

    for (cell, c) in [((-1, 0), 5usize), ((0, 0), 9), ((1, 0), 8)] {
        let (x, y) = cell_centre(&cam, cell);
        assert!(
            near(px(&img, x + 3, y + 3), color(c), 2),
            "cell {cell:?}: got {:?}, want {:?}",
            px(&img, x + 3, y + 3),
            color(c)
        );
    }
    // Empty cell shows the parchment background.
    let (x, y) = cell_centre(&cam, (3, 0));
    assert!(near(px(&img, x + 3, y + 3), [230, 222, 204], 4));

    // Second frame: nothing changed, nothing re-uploaded.
    let (_, stats) = gpu.render(&world, &cam, &NO_GRID, "basic2");
    assert_eq!(stats.chunks_uploaded, 0);

    // Editing one cell re-uploads exactly its chunk and the new colour shows.
    world.set(0, 0, 0, make_cell(1, 7));
    let (img, stats) = gpu.render(&world, &cam, &NO_GRID, "edited");
    assert_eq!(stats.chunks_uploaded, 1);
    let (x, y) = cell_centre(&cam, (0, 0));
    assert!(near(px(&img, x + 3, y + 3), color(7), 2));
}

#[test]
fn every_texture_matches_its_cpu_tile_when_tinted() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    // At 32 px/cell the 32x32 tile maps 1:1 onto screen pixels, so the GPU
    // output must equal the CPU tile times the colour, pixel for pixel.
    let tint = DEFAULT_COLORS[10]; // red
    let cam = Camera::new();
    let mut worst = 0i32;
    for id in 1..=texture::TEXTURE_COUNT {
        let mut world = World::new();
        world.set(0, 0, 0, make_cell(id, 10));
        let (img, _) = gpu.render(&world, &cam, &NO_GRID, &format!("tex{id:02}"));
        let (ox, oy) = cell_centre(&cam, (0, 0));
        let (ox, oy) = (ox - 16, oy - 16);
        let tile = texture::tile(id);
        for ty in 0..32u32 {
            for tx in 0..32u32 {
                let s = tile[(ty * 32 + tx) as usize] as f32 / 255.0;
                let got = px(&img, ox + tx, oy + ty);
                for ch in 0..3 {
                    let want = (tint[ch] as f32 * s).round() as i32;
                    worst = worst.max((got[ch] as i32 - want).abs());
                }
            }
        }
    }
    assert!(worst <= 3, "GPU texture output deviates from the CPU tile by {worst}");
}

#[test]
fn texture_cells_at_other_scales_stay_sane() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    // Zoomed far out the mip chain must average a pattern towards its mean
    // colour instead of shimmering; zoomed in it must still show the pattern.
    let mut world = World::new();
    for y in 0..16 {
        for x in 0..16 {
            world.set(0, x, y, make_cell(10, 0)); // hatch, white
        }
    }
    let mut cam = Camera::new();
    cam.ppc = 2.0; // 2 px per cell
    cam.cell = (8, 8);
    let (img, _) = gpu.render(&world, &cam, &NO_GRID, "far");
    let vals: Vec<u8> = (0..8).map(|i| px(&img, W / 2 - 8 + i, H / 2 - 8)[0]).collect();
    let (lo, hi) = (*vals.iter().min().unwrap(), *vals.iter().max().unwrap());
    assert!(hi - lo <= 12, "far-away hatch still shimmers: {vals:?}");
    assert!(lo > 180 && hi < 250, "should settle near the mean shade: {vals:?}");

    let mut cam = Camera::new();
    cam.ppc = 48.0;
    cam.cell = (8, 8);
    let (img, _) = gpu.render(&world, &cam, &NO_GRID, "near");
    let row: Vec<u8> = (0..48).map(|i| px(&img, W / 2 - 24 + i, H / 2)[0]).collect();
    assert!(row.iter().max().unwrap() - row.iter().min().unwrap() > 40, "pattern lost: {row:?}");
}

#[test]
fn parent_layer_is_ghosted_behind_the_detail_layer() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let mut world = World::new();
    world.set(0, 0, 0, make_cell(1, 9)); // lava cell at depth 0

    // Zoom in until the camera sits at depth 1 and the lava cell fills the view.
    let mut cam = Camera::new();
    cam.zoom_at(2.5, W as f64 * 0.5, H as f64 * 0.5, W as f64, H as f64);
    assert_eq!(cam.depth, 1, "ppc = {}", cam.ppc);
    // Zoomed far enough into depth 1 that the parent has settled at its
    // ghost opacity (it is still solid right after the depth switch).
    cam.ppc = 40.0;
    // Put the camera in the middle of the lava cell: depth-1 cell (2,2).
    cam.cell = (2, 2);
    cam.frac = (0.5, 0.5);

    // One detail cell at depth 1, directly right of the centre.
    world.set(1, 3, 2, make_cell(1, 1)); // charcoal

    let (img, _) = gpu.render(&world, &cam, &NO_GRID, "ghost");
    let (cx, cy) = (W / 2, H / 2);

    // Where the detail layer is empty we see the parent as a faded tint:
    // strictly between pure lava and the background.
    let lava = color(9);
    let bg = [230u8, 222, 204];
    let ghost = px(&img, cx, cy - 20);
    for i in 0..3 {
        let (lo, hi) = (lava[i].min(bg[i]), lava[i].max(bg[i]));
        assert!(ghost[i] >= lo && ghost[i] <= hi, "channel {i}: {ghost:?}");
    }
    assert!(!near(ghost, lava, 20) && !near(ghost, bg, 10), "not faded: {ghost:?}");

    // The detail cell is drawn at full strength on top.
    let (dx, dy) = cell_centre(&cam, (3, 2));
    assert!(near(px(&img, dx, dy), color(1), 2));

    // Ghost opacity 0 hides the parent; 1.0 shows it undiluted.
    let hidden = RenderOptions { ghost_opacity: 0.0, ..NO_GRID };
    let (img, _) = gpu.render(&world, &cam, &hidden, "ghost0");
    assert!(near(px(&img, cx, cy - 20), bg, 4));
    let full = RenderOptions { ghost_opacity: 1.0, ..NO_GRID };
    let (img, _) = gpu.render(&world, &cam, &full, "ghost1");
    assert!(near(px(&img, cx, cy - 20), lava, 3));
}

#[test]
fn grid_and_cursor_are_drawn_and_the_grid_can_be_hidden() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let world = World::new();
    let cam = Camera::new();
    let grid = RenderOptions { show_grid: true, ..NO_GRID };
    let (plain, _) = gpu.render(&world, &cam, &grid, "grid");
    // A grid line sits on the cell edge at x = W/2 (frac 0 => cell edge in the middle).
    let on_line = px(&plain, W / 2, 50);
    let off_line = px(&plain, W / 2 + 10, 50);
    assert!(on_line[0] < off_line[0] - 10, "grid line missing: {on_line:?} vs {off_line:?}");

    let (hidden, _) = gpu.render(&world, &cam, &NO_GRID, "nogrid");
    assert!(near(px(&hidden, W / 2, 50), px(&hidden, W / 2 + 10, 50), 1), "grid still visible");

    let with_cursor = RenderOptions { cursor: Some((2, 2)), ..grid };
    let (img, _) = gpu.render(&world, &cam, &with_cursor, "cursor");
    let (x, y) = cell_centre(&cam, (2, 2));
    assert!(px(&img, x, y)[0] > px(&plain, x, y)[0], "cursor highlight missing");
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
    cam.ppc = 4.0; // far zoomed out
    let (img, stats) = gpu.render(&world, &cam, &RenderOptions::default(), "stress");
    assert!(stats.chunks_drawn > 10);
    assert_eq!(stats.chunks_skipped, 0);
    // Not just background.
    let colourful = (0..W).step_by(7).filter(|&x| !near(px(&img, x, 100), [230, 222, 204], 6)).count();
    assert!(colourful > 5);

    // Zoomed in on the same map the textures are visible.
    cam.ppc = 32.0;
    let (_, stats) = gpu.render(&world, &cam, &RenderOptions::default(), "stress_near");
    assert!(stats.chunks_drawn >= 1);
}

/// Largest per-channel difference between two frames.
fn max_diff(a: &[u8], b: &[u8]) -> i32 {
    a.iter().zip(b).map(|(&x, &y)| (x as i32 - y as i32).abs()).max().unwrap()
}

#[test]
fn grid_and_ghost_are_identical_either_side_of_a_depth_switch() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    // Detail on three consecutive depths so every layer shows up.
    let mut world = World::new();
    world.set(0, 1, 1, make_cell(1, 9));
    world.set(1, 5, 5, make_cell(1, 8));
    world.set(1, 6, 5, make_cell(1, 5));
    world.set(2, 21, 21, make_cell(1, 10));
    world.set(2, 22, 21, make_cell(1, 1));

    // The camera sits exactly on the switch size at depth 1 (ppc 64) ...
    let mut before = Camera::new();
    before.depth = 1;
    before.cell = (5, 5);
    before.frac = (0.3, 0.6);
    before.ppc = 63.999;
    // ... and the very same view after it has re-based onto depth 2.
    let mut after = before;
    after.ppc = 64.001;
    after.normalize();
    assert_eq!(before.depth, 1);
    assert_eq!(after.depth, 2, "should have switched depth");

    let opts = RenderOptions { show_grid: true, ..NO_GRID };
    let (a, _) = gpu.render(&world, &before, &opts, "hop_before");
    let (b, _) = gpu.render(&world, &after, &opts, "hop_after");
    let d = max_diff(&a, &b);
    assert!(d <= 6, "frames differ by {d} across a depth switch: grid or ghosting pops");

    // The same holds when the grid is the only thing on screen.
    let empty = World::new();
    let (a, _) = gpu.render(&empty, &before, &opts, "hop_grid_before");
    let (b, _) = gpu.render(&empty, &after, &opts, "hop_grid_after");
    let d = max_diff(&a, &b);
    assert!(d <= 4, "grid differs by {d} across a depth switch");
}

#[test]
fn grid_shows_every_nearby_depth_with_heavier_parent_lines() {
    let Some(mut gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter available");
        return;
    };
    let world = World::new();
    let mut cam = Camera::new();
    cam.depth = 2;
    cam.ppc = 40.0;
    cam.cell = (0, 0); // cell edges (and parent edges) pass through the view centre
    cam.frac = (0.0, 0.0);
    let opts = RenderOptions { show_grid: true, ..NO_GRID };
    let (img, _) = gpu.render(&world, &cam, &opts, "levels");
    let bg = [230u8, 222, 204];
    let dark = |x: u32, y: u32| bg[0] as i32 - px(&img, x, y)[0] as i32;

    // Row far from any horizontal line; sample vertical lines.
    let y = H / 2 + 17;
    let cell_line = dark(W / 2 + 40, y); // an ordinary cell edge of this depth
    let parent_line = dark(W / 2 + 160, y); // edge of the parent (4 cells = 160 px)
    let between = dark(W / 2 + 20, y);
    assert!(cell_line > between + 8, "cell edge missing: {cell_line} vs {between}");
    assert!(
        parent_line > cell_line + 15,
        "parent edge should be heavier: {parent_line} vs {cell_line}"
    );

    // The next finer level starts to show up inside a cell as the camera
    // approaches the depth switch.
    cam.ppc = 60.0;
    let (img, _) = gpu.render(&world, &cam, &opts, "levels_fine");
    let dark = |x: u32, y: u32| bg[0] as i32 - px(&img, x, y)[0] as i32;
    let quarter = dark(W / 2 + 15, y); // 1/4 cell = 15 px: next depth's cell edge
    let between = dark(W / 2 + 7, y);
    assert!(quarter > between + 3, "finer grid should be fading in: {quarter} vs {between}");
}

#[test]
fn bounded_map_dims_everything_outside_and_outlines_the_edge() {
    let Some(mut gpu) = gpu() else { return eprintln!("no GPU adapter; skipping") };
    let cam = Camera::new(); // the centre of the screen is the map's top-left corner
    let plain = World::new();
    let map = World::with_spec(MapSpec::new(8, 8, 1));
    let (a, _) = gpu.render(&plain, &cam, &NO_GRID, "bounds_off");
    let (b, _) = gpu.render(&map, &cam, &NO_GRID, "bounds_on");

    // Inside the map (right of and below the centre): untouched.
    let inside = (400, 300);
    assert_eq!(px(&a, inside.0, inside.1), px(&b, inside.0, inside.1));
    // Outside (left of and above): clearly darker.
    for out in [(100, 100), (100, 300), (400, 50)] {
        let (pa, pb) = (px(&a, out.0, out.1), px(&b, out.0, out.1));
        assert!(pb[0] < pa[0] / 2 + 10 && pb[1] < pa[1] / 2 + 10, "{out:?}: {pa:?} -> {pb:?}");
    }
    // The vertical edge at x = 256 and the horizontal one at y = 192 are drawn
    // in the amber border colour.
    for edge in [(256, 300), (400, 192)] {
        let p = px(&b, edge.0, edge.1);
        assert!(p[0] > 180 && p[0] > p[2] + 60, "edge pixel {edge:?} = {p:?}");
    }
}

#[test]
fn grid_stops_at_the_maps_deepest_level_and_outside_the_map() {
    let Some(mut gpu) = gpu() else { return eprintln!("no GPU adapter; skipping") };
    let cam = Camera::new();
    let grid = RenderOptions { show_grid: true, ..NO_GRID };
    // A map with no deeper level: the 1/4-cell grid that would fade in does not exist.
    let shallow = World::with_spec(MapSpec { width: 0, height: 0, max_depth: 0 });
    let deep = World::new();
    let (s, _) = gpu.render(&shallow, &cam, &grid, "grid_shallow");
    let (d, _) = gpu.render(&deep, &cam, &grid, "grid_deep");
    // 8 px right of a cell line the finer grid has a line.
    let (x, y) = (256 + 8, 300);
    assert!(px(&d, x, y)[0] < px(&s, x, y)[0], "{:?} vs {:?}", px(&d, x, y), px(&s, x, y));
    // The ordinary cell line is there in both.
    assert_eq!(px(&d, 288, 300), px(&s, 288, 300));

    // No grid lines outside a bounded map: a pixel on a cell line out there is
    // exactly the dimmed background.
    let map = World::with_spec(MapSpec::new(8, 8, 1));
    let (g, _) = gpu.render(&map, &cam, &grid, "grid_bounded");
    let (n, _) = gpu.render(&map, &cam, &NO_GRID, "grid_bounded_off");
    assert_eq!(px(&g, 224, 100), px(&n, 224, 100));
}

#[test]
fn checker_overlay_darkens_every_other_cell_only() {
    let Some(mut gpu) = gpu() else { return eprintln!("no GPU adapter; skipping") };
    let mut world = World::new();
    for y in -6..6 {
        for x in -8..8 {
            world.set(0, x, y, make_cell(1, 0)); // flat white
        }
    }
    let cam = Camera::new(); // 32 px/cell, cell (0,0) starts at the screen centre
    let on = RenderOptions { checker: true, ..NO_GRID };
    let (a, _) = gpu.render(&world, &cam, &NO_GRID, "checker_off");
    let (b, _) = gpu.render(&world, &cam, &on, "checker_on");
    let c = |x: u32, y: u32| px(&b, 256 + 16 + 32 * x, 192 + 16 + 32 * y);
    // Cells (0,0) and (1,1) are even, (1,0) and (0,1) odd.
    assert_eq!(c(0, 0), px(&a, 272, 208));
    assert_eq!(c(1, 1), px(&a, 304, 240));
    assert!(c(1, 0)[0] < c(0, 0)[0] - 15, "{:?} vs {:?}", c(1, 0), c(0, 0));
    assert_eq!(c(1, 0), c(0, 1));
    // Left of the origin the pattern continues: (-1, 0) is odd, (-1,-1) even.
    assert!(px(&b, 256 - 16, 192 + 16)[0] < px(&b, 256 - 16, 192 - 16)[0] - 15);
    // Zoomed far out the shimmer is faded away.
    let mut far = Camera::new();
    far.ppc = 2.0;
    let (f, _) = gpu.render(&world, &far, &on, "checker_far");
    let (g, _) = gpu.render(&world, &far, &NO_GRID, "checker_far_off");
    assert_eq!(f, g);
}
