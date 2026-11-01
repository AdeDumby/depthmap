//! Colour maths for the colour wheel: hue and saturation on the disc,
//! brightness on a separate slider.

use std::f32::consts::TAU;

/// `h`, `s`, `v` in `0..=1` to RGB.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let (i, f) = (h.floor(), h - h.floor());
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as u32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b].map(|c| (c * 255.0).round() as u8)
}

/// RGB to `(h, s, v)`; the hue of a grey is 0.
pub fn rgb_to_hsv(rgb: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    [h, if max == 0.0 { 0.0 } else { d / max }, max]
}

/// Position on the wheel (relative to its centre, in units of its radius)
/// to hue and saturation. Positions outside the disc are pulled onto its edge.
pub fn wheel_to_hs(dx: f32, dy: f32) -> (f32, f32) {
    let r = (dx * dx + dy * dy).sqrt();
    let h = dy.atan2(dx).rem_euclid(TAU) / TAU;
    (h, r.min(1.0))
}

/// Where hue and saturation sit on the wheel, relative to its centre in
/// units of its radius.
pub fn hs_to_wheel(h: f32, s: f32) -> (f32, f32) {
    let a = h * TAU;
    (a.cos() * s, a.sin() * s)
}

/// RGBA pixels (straight alpha) of a full-brightness hue/saturation disc,
/// `size` x `size`, with a soft edge.
pub fn wheel_rgba(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 4);
    let c = size as f32 * 0.5;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = ((x as f32 + 0.5 - c) / c, (y as f32 + 0.5 - c) / c);
            let r = (dx * dx + dy * dy).sqrt();
            let (h, s) = wheel_to_hs(dx, dy);
            let [red, green, blue] = hsv_to_rgb(h, s, 1.0);
            let alpha = ((1.0 - r) * c).clamp(0.0, 1.0);
            out.extend_from_slice(&[red, green, blue, (alpha * 255.0) as u8]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_colours() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), [0, 0, 255]);
        assert_eq!(hsv_to_rgb(0.5, 0.0, 1.0), [255, 255, 255]);
        assert_eq!(hsv_to_rgb(0.3, 1.0, 0.0), [0, 0, 0]);
        assert_eq!(hsv_to_rgb(1.0, 1.0, 1.0), [255, 0, 0], "hue wraps");
    }

    #[test]
    fn every_colour_round_trips_through_hsv() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    let [h, s, v] = rgb_to_hsv([r as u8, g as u8, b as u8]);
                    assert_eq!(hsv_to_rgb(h, s, v), [r as u8, g as u8, b as u8]);
                }
            }
        }
    }

    #[test]
    fn wheel_positions_map_to_hue_and_saturation_and_back() {
        let (h, s) = wheel_to_hs(1.0, 0.0);
        assert!((h - 0.0).abs() < 1e-6 && (s - 1.0).abs() < 1e-6);
        let (h, s) = wheel_to_hs(0.0, 0.5);
        assert!((h - 0.25).abs() < 1e-6 && (s - 0.5).abs() < 1e-6);
        assert_eq!(wheel_to_hs(3.0, 4.0).1, 1.0, "outside the disc is clamped");
        let (x, y) = hs_to_wheel(0.25, 0.5);
        assert!(x.abs() < 1e-6 && (y - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wheel_image_has_the_right_colours_and_a_transparent_corner() {
        let n = 64;
        let px = wheel_rgba(n);
        assert_eq!(px.len(), n * n * 4);
        assert_eq!(px[3], 0, "corner is outside the disc");
        let at = |x: usize, y: usize| &px[(y * n + x) * 4..][..4];
        assert_eq!(at(32, 32)[3], 255);
        assert!(at(32, 32)[..3].iter().all(|&c| c > 240), "centre is white");
        let right = at(n - 4, 32);
        assert!(right[0] > 200 && right[1] < 60 && right[2] < 60, "hue 0 is red on the right");
    }
}
