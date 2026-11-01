//! Node icons: a set of procedural built-in icons, PNG decoding for imported
//! ones, and helpers for the "Custom Icons" folder. No GPU or UI types here.

use std::fs;
use std::path::Path;

/// Size of the generated built-in icons (they are square).
pub const BUILTIN_SIZE: usize = 64;
/// Number of built-in icons; ids are `1..=BUILTIN_COUNT`.
pub const BUILTIN_COUNT: u8 = 15;
/// Largest accepted PNG file and image side.
pub const MAX_PNG_BYTES: usize = 8 << 20;
pub const MAX_PNG_SIDE: u32 = 4096;
/// Longest accepted icon file name.
pub const MAX_NAME_LEN: usize = 120;

const NAMES: [&str; BUILTIN_COUNT as usize] = [
    "Pin", "Star", "Flag", "House", "Tower", "Tree", "Chest", "Skull", "Sword", "Door", "Flame",
    "Cross", "Gem", "Shield", "Camp",
];

pub fn builtin_name(id: u8) -> &'static str {
    match id {
        1..=BUILTIN_COUNT => NAMES[id as usize - 1],
        _ => "Missing icon",
    }
}

/// An RGBA image, straight (non-premultiplied) alpha, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

// ---- file names --------------------------------------------------------------------

/// A plain `something.png` file name: no folders, nothing odd.
pub fn valid_icon_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && lower.ends_with(".png")
        && lower.len() > 4
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.starts_with('.')
        && !name.chars().any(|c| c.is_control())
}

/// PNG files in `dir`, sorted by name; empty if the folder does not exist.
pub fn list_custom(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_icon_name(n))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// A file name based on `wanted` that does not exist in `dir` yet
/// (`dragon.png`, `dragon (2).png`, ...).
pub fn unique_name(dir: &Path, wanted: &str) -> String {
    let stem = wanted.strip_suffix(".png").or_else(|| wanted.strip_suffix(".PNG")).unwrap_or(wanted);
    let mut name = format!("{stem}.png");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{stem} ({n}).png");
        n += 1;
    }
    name
}

// ---- PNG ---------------------------------------------------------------------------------

/// Decodes any PNG (grey, palette, 16-bit, with or without alpha) to RGBA.
pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() > MAX_PNG_BYTES {
        return Err("the file is larger than 8 MB".into());
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("not a valid PNG ({e})"))?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > MAX_PNG_SIDE || h > MAX_PNG_SIDE {
        return Err(format!("the image is {w}x{h}; the limit is {MAX_PNG_SIDE} pixels a side"));
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let frame = reader.next_frame(&mut buf).map_err(|e| format!("not a valid PNG ({e})"))?;
    let data = &buf[..frame.buffer_size()];
    let n = (w * h) as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    match frame.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => {
            for p in data.chunks_exact(3) {
                rgba.extend([p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in data.chunks_exact(2) {
                rgba.extend([p[0], p[0], p[0], p[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in data {
                rgba.extend([g, g, g, 255]);
            }
        }
        png::ColorType::Indexed => return Err("unsupported palette PNG".into()),
    }
    if rgba.len() != n * 4 {
        return Err("unexpected PNG layout".into());
    }
    Ok(Image { w: w as usize, h: h as usize, rgba })
}

/// Encodes an image as PNG (used by tests and tooling).
pub fn encode_png(img: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.w as u32, img.h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&img.rgba).unwrap();
    out
}

/// Shrinks `img` (keeping its shape) so that neither side exceeds
/// `max_side`, averaging pixels with alpha weighting so transparent edges
/// do not bleed dark fringes. Images that already fit are returned as is.
pub fn downscale(img: &Image, max_side: usize) -> Image {
    let big = img.w.max(img.h);
    if big <= max_side || max_side == 0 {
        return img.clone();
    }
    let (w, h) = ((img.w * max_side / big).max(1), (img.h * max_side / big).max(1));
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let (y0, y1) = (y * img.h / h, ((y + 1) * img.h).div_ceil(h).min(img.h).max(y * img.h / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * img.w / w, ((x + 1) * img.w).div_ceil(w).min(img.w).max(x * img.w / w + 1));
            let (mut r, mut g, mut b, mut a, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = &img.rgba[(yy * img.w + xx) * 4..][..4];
                    let al = p[3] as f32 / 255.0;
                    r += p[0] as f32 * al;
                    g += p[1] as f32 * al;
                    b += p[2] as f32 * al;
                    a += al;
                    n += 1.0;
                }
            }
            if a > 0.0 {
                rgba.extend([(r / a) as u8, (g / a) as u8, (b / a) as u8, (a / n * 255.0).round() as u8]);
            } else {
                rgba.extend([0, 0, 0, 0]);
            }
        }
    }
    Image { w, h, rgba }
}

// ---- procedural icons ------------------------------------------------------------------

type P = (f32, f32);

enum Sd {
    Circle(P, f32),
    /// Convex polygon, any winding.
    Poly(Vec<P>),
}

struct Part {
    shape: Sd,
    color: [u8; 3],
    /// Solid parts form the silhouette that gets the dark outline; the
    /// others are details painted on top.
    solid: bool,
}

const RED: [u8; 3] = [214, 60, 52];
const WHITE: [u8; 3] = [245, 243, 236];
const BROWN: [u8; 3] = [150, 98, 54];
const DARK_BROWN: [u8; 3] = [98, 62, 36];
const GREEN: [u8; 3] = [86, 160, 78];
const DARK_GREEN: [u8; 3] = [52, 116, 60];
const GOLD: [u8; 3] = [244, 200, 64];
const GREY: [u8; 3] = [168, 172, 182];
const DARK: [u8; 3] = [44, 40, 46];
const BLUE: [u8; 3] = [64, 114, 204];
const CYAN: [u8; 3] = [96, 214, 234];
const ORANGE: [u8; 3] = [242, 142, 44];
const YELLOW: [u8; 3] = [255, 226, 96];
const SAND: [u8; 3] = [222, 196, 146];
const OUTLINE: [u8; 3] = [30, 24, 22];

fn circle(x: f32, y: f32, r: f32, color: [u8; 3], solid: bool) -> Part {
    Part { shape: Sd::Circle((x, y), r), color, solid }
}
fn poly(pts: &[P], color: [u8; 3], solid: bool) -> Part {
    Part { shape: Sd::Poly(pts.to_vec()), color, solid }
}
fn rect(x: f32, y: f32, hx: f32, hy: f32, color: [u8; 3], solid: bool) -> Part {
    poly(&[(x - hx, y - hy), (x + hx, y - hy), (x + hx, y + hy), (x - hx, y + hy)], color, solid)
}
fn line(a: P, b: P, w: f32, color: [u8; 3], solid: bool) -> Part {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (nx, ny) = (-dy / len * w * 0.5, dx / len * w * 0.5);
    poly(
        &[(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny)],
        color,
        solid,
    )
}

fn rotate(parts: Vec<Part>, degrees: f32) -> Vec<Part> {
    let (s, c) = degrees.to_radians().sin_cos();
    let rot = |p: P| (p.0 * c - p.1 * s, p.0 * s + p.1 * c);
    parts
        .into_iter()
        .map(|mut part| {
            part.shape = match part.shape {
                Sd::Circle(centre, r) => Sd::Circle(rot(centre), r),
                Sd::Poly(v) => Sd::Poly(v.into_iter().map(rot).collect()),
            };
            part
        })
        .collect()
}

impl Sd {
    /// Signed distance (negative inside), exact for circles and a mitred
    /// approximation for polygons, which is all an icon needs.
    fn distance(&self, p: P) -> f32 {
        match self {
            Sd::Circle(c, r) => ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt() - r,
            Sd::Poly(v) => {
                let area: f32 = (0..v.len())
                    .map(|i| {
                        let (a, b) = (v[i], v[(i + 1) % v.len()]);
                        a.0 * b.1 - b.0 * a.1
                    })
                    .sum();
                let flip = if area < 0.0 { -1.0 } else { 1.0 };
                (0..v.len())
                    .map(|i| {
                        let (a, b) = (v[i], v[(i + 1) % v.len()]);
                        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                        let len = (ex * ex + ey * ey).sqrt().max(1e-6);
                        // Outward normal for a clockwise (area < 0 in y-down) or ccw polygon.
                        let (nx, ny) = (ey / len * flip, -ex / len * flip);
                        (p.0 - a.0) * nx + (p.1 - a.1) * ny
                    })
                    .fold(f32::NEG_INFINITY, f32::max)
            }
        }
    }
}

fn parts(id: u8) -> Vec<Part> {
    match id {
        1 => vec![
            circle(0.0, -0.2, 0.52, RED, true),
            poly(&[(-0.46, 0.0), (0.46, 0.0), (0.0, 0.92)], RED, true),
            circle(0.0, -0.2, 0.2, WHITE, false),
        ],
        2 => {
            let tip = |k: i32| {
                let a = (-90.0 + 72.0 * k as f32).to_radians();
                (a.cos() * 0.92, a.sin() * 0.92)
            };
            let inner = |k: i32| {
                let a = (-54.0 + 72.0 * k as f32).to_radians();
                (a.cos() * 0.38, a.sin() * 0.38)
            };
            let mut v = vec![poly(&(0..5).map(inner).collect::<Vec<_>>(), GOLD, true)];
            for k in 0..5 {
                v.push(poly(&[inner(k - 1), tip(k), inner(k)], GOLD, true));
            }
            v
        }
        3 => vec![
            rect(-0.55, 0.05, 0.07, 0.9, DARK_BROWN, true),
            poly(&[(-0.48, -0.85), (0.85, -0.5), (-0.48, -0.15)], RED, true),
        ],
        4 => vec![
            rect(0.0, 0.4, 0.58, 0.48, SAND, true),
            poly(&[(-0.82, -0.05), (0.0, -0.82), (0.82, -0.05)], RED, true),
            rect(0.0, 0.62, 0.15, 0.26, DARK_BROWN, false),
            rect(-0.36, 0.3, 0.1, 0.1, BLUE, false),
            rect(0.36, 0.3, 0.1, 0.1, BLUE, false),
        ],
        5 => vec![
            rect(0.0, 0.2, 0.46, 0.7, GREY, true),
            rect(-0.34, -0.62, 0.12, 0.14, GREY, true),
            rect(0.0, -0.62, 0.12, 0.14, GREY, true),
            rect(0.34, -0.62, 0.12, 0.14, GREY, true),
            rect(0.0, 0.62, 0.15, 0.28, DARK, false),
            circle(0.0, 0.36, 0.15, DARK, false),
        ],
        6 => vec![
            rect(0.0, 0.62, 0.12, 0.3, BROWN, true),
            circle(-0.32, 0.15, 0.36, DARK_GREEN, true),
            circle(0.32, 0.15, 0.36, DARK_GREEN, true),
            circle(0.0, -0.18, 0.48, GREEN, true),
        ],
        7 => vec![
            rect(0.0, 0.32, 0.72, 0.4, BROWN, true),
            rect(0.0, -0.2, 0.72, 0.2, DARK_BROWN, true),
            rect(0.0, 0.05, 0.72, 0.05, GOLD, false),
            rect(0.0, 0.14, 0.1, 0.14, GOLD, false),
        ],
        8 => vec![
            circle(0.0, -0.15, 0.6, WHITE, true),
            rect(0.0, 0.45, 0.32, 0.3, WHITE, true),
            circle(-0.24, -0.12, 0.17, DARK, false),
            circle(0.24, -0.12, 0.17, DARK, false),
            poly(&[(-0.07, 0.14), (0.07, 0.14), (0.0, 0.27)], DARK, false),
            rect(-0.12, 0.58, 0.03, 0.14, DARK, false),
            rect(0.12, 0.58, 0.03, 0.14, DARK, false),
            rect(0.0, 0.58, 0.03, 0.14, DARK, false),
        ],
        9 => rotate(
            vec![
                poly(&[(0.0, -0.98), (0.11, -0.72), (0.11, 0.3), (-0.11, 0.3), (-0.11, -0.72)], GREY, true),
                rect(0.0, 0.38, 0.4, 0.07, GOLD, true),
                rect(0.0, 0.62, 0.065, 0.2, BROWN, true),
                circle(0.0, 0.88, 0.1, GOLD, true),
            ],
            45.0,
        ),
        10 => vec![
            rect(0.0, 0.32, 0.56, 0.62, GREY, true),
            circle(0.0, -0.3, 0.56, GREY, true),
            rect(0.0, 0.4, 0.4, 0.54, BROWN, false),
            circle(0.0, -0.14, 0.4, BROWN, false),
            rect(0.0, 0.3, 0.02, 0.64, DARK_BROWN, false),
            circle(0.22, 0.38, 0.06, GOLD, false),
        ],
        11 => vec![
            circle(0.0, 0.3, 0.52, ORANGE, true),
            poly(&[(-0.5, 0.3), (0.5, 0.3), (0.3, -0.3), (0.0, -0.92), (-0.3, -0.3)], ORANGE, true),
            circle(0.0, 0.38, 0.3, YELLOW, false),
            poly(&[(-0.22, 0.38), (0.22, 0.38), (0.0, -0.2)], YELLOW, false),
        ],
        12 => vec![
            line((-0.62, -0.62), (0.62, 0.62), 0.3, RED, true),
            line((0.62, -0.62), (-0.62, 0.62), 0.3, RED, true),
        ],
        13 => vec![
            poly(&[(-0.5, -0.58), (0.5, -0.58), (0.84, -0.1), (0.0, 0.88), (-0.84, -0.1)], CYAN, true),
            line((-0.84, -0.1), (0.84, -0.1), 0.05, WHITE, false),
            line((-0.5, -0.58), (-0.2, -0.1), 0.05, WHITE, false),
            line((0.5, -0.58), (0.2, -0.1), 0.05, WHITE, false),
            line((-0.2, -0.1), (0.0, 0.88), 0.05, WHITE, false),
            line((0.2, -0.1), (0.0, 0.88), 0.05, WHITE, false),
        ],
        14 => vec![
            poly(&[(-0.72, -0.78), (0.72, -0.78), (0.72, 0.1), (0.0, 0.94), (-0.72, 0.1)], BLUE, true),
            rect(0.0, -0.1, 0.09, 0.56, GOLD, false),
            rect(0.0, -0.3, 0.42, 0.09, GOLD, false),
        ],
        15 => vec![
            poly(&[(-0.88, 0.72), (0.88, 0.72), (0.0, -0.78)], SAND, true),
            poly(&[(-0.24, 0.72), (0.24, 0.72), (0.0, 0.0)], DARK, false),
            line((0.0, -0.78), (0.0, -0.95), 0.06, DARK_BROWN, true),
        ],
        _ => vec![
            rect(0.0, 0.0, 0.72, 0.72, GREY, true),
            line((-0.5, -0.5), (0.5, 0.5), 0.14, RED, false),
            line((0.5, -0.5), (-0.5, 0.5), 0.14, RED, false),
        ],
    }
}

/// The built-in icon `id` as a 64 x 64 image (id 0 and unknown ids give
/// the "missing icon" picture).
pub fn builtin_image(id: u8) -> Image {
    let id = if (1..=BUILTIN_COUNT).contains(&id) { id } else { 0 };
    let parts = parts(id);
    let n = BUILTIN_SIZE;
    let px = 2.0 / n as f32; // one pixel in icon units
    let outline_width = 0.06;
    let mut rgba = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let p = ((x as f32 + 0.5) * px - 1.0, (y as f32 + 0.5) * px - 1.0);
            // Premultiplied accumulation, painter's algorithm.
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            let mut over = |col: [u8; 3], alpha: f32| {
                let alpha = alpha.clamp(0.0, 1.0);
                r = col[0] as f32 * alpha + r * (1.0 - alpha);
                g = col[1] as f32 * alpha + g * (1.0 - alpha);
                b = col[2] as f32 * alpha + b * (1.0 - alpha);
                a = alpha + a * (1.0 - alpha);
            };
            let silhouette = parts
                .iter()
                .filter(|q| q.solid)
                .map(|q| q.shape.distance(p))
                .fold(f32::INFINITY, f32::min);
            over(OUTLINE, 0.5 - (silhouette - outline_width) / px);
            for q in &parts {
                over(q.color, 0.5 - q.shape.distance(p) / px);
            }
            let (r, g, b) = if a > 0.0 { (r / a, g / a, b / a) } else { (0.0, 0.0, 0.0) };
            rgba.extend([r.round() as u8, g.round() as u8, b.round() as u8, (a * 255.0).round() as u8]);
        }
    }
    Image { w: n, h: n, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(img: &Image, x: usize, y: usize) -> u8 {
        img.rgba[(y * img.w + x) * 4 + 3]
    }

    #[test]
    fn every_builtin_icon_is_drawn_inside_its_square() {
        let mut seen = std::collections::HashSet::new();
        for id in 0..=BUILTIN_COUNT {
            let img = builtin_image(id);
            assert_eq!((img.w, img.h), (BUILTIN_SIZE, BUILTIN_SIZE));
            let opaque = img.rgba.chunks_exact(4).filter(|p| p[3] > 200).count();
            let total = BUILTIN_SIZE * BUILTIN_SIZE;
            assert!(opaque > total / 12, "icon {id} is nearly empty ({opaque})");
            assert!(opaque < total * 9 / 10, "icon {id} fills the whole square");
            // The very corners stay transparent so icons never look boxed in.
            for (x, y) in [(0, 0), (63, 0), (0, 63), (63, 63)] {
                if id != 0 {
                    assert_eq!(alpha(&img, x, y), 0, "icon {id} corner ({x},{y})");
                }
            }
            assert!(seen.insert(img.rgba), "icon {id} duplicates another one");
            assert!(!builtin_name(id).is_empty());
        }
        assert_eq!(builtin_image(200), builtin_image(0));
    }

    #[test]
    fn icons_have_a_dark_outline_and_symmetric_pin() {
        let pin = builtin_image(1);
        // The edge of the pin's head is the outline colour, its middle is red.
        let row = 20 * pin.w * 4;
        let first = (0..pin.w).find(|&x| pin.rgba[row + x * 4 + 3] > 250).unwrap();
        let p = &pin.rgba[row + first * 4..][..3];
        assert!(p[0] < 120 && p[1] < 80, "outline pixel {p:?}");
        let mid = &pin.rgba[row + 32 * 4..][..3];
        assert!(mid[0] > 180, "fill pixel {mid:?}");
    }

    #[test]
    fn png_round_trip_and_all_colour_types() {
        let img = Image { w: 3, h: 2, rgba: (0..24).map(|i| (i * 10) as u8).collect() };
        let back = decode_png(&encode_png(&img)).unwrap();
        assert_eq!(back, img);

        // Greyscale, RGB and 16-bit files decode to RGBA too.
        for (color, depth, bytes_per_px) in [
            (png::ColorType::Grayscale, png::BitDepth::Eight, 1),
            (png::ColorType::Rgb, png::BitDepth::Eight, 3),
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight, 2),
            (png::ColorType::Rgba, png::BitDepth::Sixteen, 8),
        ] {
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, 2, 2);
            enc.set_color(color);
            enc.set_depth(depth);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![200u8; 4 * bytes_per_px])
                .unwrap();
            let img = decode_png(&out).unwrap();
            assert_eq!((img.w, img.h, img.rgba.len()), (2, 2, 16), "{color:?}");
        }
    }

    #[test]
    fn bad_pngs_are_errors_not_panics() {
        assert!(decode_png(b"not a png").is_err());
        assert!(decode_png(&[]).is_err());
        let good = encode_png(&builtin_image(2));
        assert!(decode_png(&good[..good.len() / 2]).is_err());
        assert!(decode_png(&vec![0u8; MAX_PNG_BYTES + 1]).is_err());
        // Dimensions over the limit are refused before allocating.
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, MAX_PNG_SIDE + 1, 1);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&vec![0u8; MAX_PNG_SIDE as usize + 1])
            .unwrap();
        assert!(decode_png(&out).unwrap_err().contains("limit"));
    }

    #[test]
    fn downscale_keeps_shape_and_does_not_bleed_transparent_colour() {
        let img = builtin_image(1);
        assert_eq!(downscale(&img, 64), img);
        let small = downscale(&img, 16);
        assert_eq!((small.w, small.h), (16, 16));
        // Corners stay empty; semi-transparent edge pixels keep the icon colours.
        assert_eq!(alpha(&small, 0, 0), 0);
        let opaque_dark = small
            .rgba
            .chunks_exact(4)
            .filter(|p| p[3] > 0 && p[3] < 255)
            .filter(|p| p[0] < 40 && p[1] < 40)
            .count();
        let edge = small.rgba.chunks_exact(4).filter(|p| p[3] > 0 && p[3] < 255).count();
        assert!(opaque_dark <= edge);
        // A wide image keeps its aspect ratio.
        let wide = Image { w: 400, h: 100, rgba: vec![255; 400 * 100 * 4] };
        let s = downscale(&wide, 40);
        assert_eq!((s.w, s.h), (40, 10));
        assert!(s.rgba.iter().all(|&b| b == 255));
    }

    #[test]
    fn icon_names_must_be_plain_png_file_names() {
        for good in ["dragon.png", "My Icon (2).PNG", "ü.png"] {
            assert!(valid_icon_name(good), "{good}");
        }
        for bad in ["", ".png", "a.jpg", "../x.png", "a/b.png", "a\\b.png", ".hidden.png", "c:x.png", "x\n.png"] {
            assert!(!valid_icon_name(bad), "{bad:?}");
        }
        assert!(!valid_icon_name(&format!("{}.png", "a".repeat(200))));
    }

    #[test]
    fn folder_listing_and_unique_names() {
        let dir = std::env::temp_dir().join(format!("depthmap-icons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(list_custom(&dir).is_empty(), "missing folder is fine");
        fs::create_dir_all(&dir).unwrap();
        for f in ["b.png", "A.png", "note.txt", "c.PNG"] {
            fs::write(dir.join(f), b"x").unwrap();
        }
        fs::create_dir(dir.join("sub.png")).unwrap();
        assert_eq!(list_custom(&dir), vec!["A.png", "b.png", "c.PNG"]);
        assert_eq!(unique_name(&dir, "new.png"), "new.png");
        assert_eq!(unique_name(&dir, "b.png"), "b (2).png");
        fs::write(dir.join("b (2).png"), b"x").unwrap();
        assert_eq!(unique_name(&dir, "b.png"), "b (3).png");
        fs::remove_dir_all(dir).unwrap();
    }
}
