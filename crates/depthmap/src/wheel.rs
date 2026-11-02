//! Colour maths for the colour picker (hue, saturation, brightness).

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
}
