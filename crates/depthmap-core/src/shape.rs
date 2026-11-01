//! Shape rasteriser: which cells a dragged line, rectangle, ellipse, ... covers,
//! and how it is filled (outline, solid, dither patterns, hatching).
//!
//! Pure geometry on integer cells. The caller picks the depth, the texture
//! and the colour; this only says *where*. Patterns are evaluated in absolute
//! cell coordinates, so two shapes drawn next to each other tile seamlessly.

use crate::raster::line_cells;

/// Inclusive cell rectangle `(min, max)`; nothing outside it is produced.
pub type Clip = ((i64, i64), (i64, i64));

/// Longest line / polygon edge we are willing to walk cell by cell.
const MAX_EDGE: i64 = 1 << 22;
/// Largest bitmap (in cells) used while rasterising one shape.
const MAX_AREA: i64 = 8 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Line,
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
}

impl Shape {
    pub const ALL: [Shape; 5] =
        [Shape::Line, Shape::Rectangle, Shape::Ellipse, Shape::Triangle, Shape::Diamond];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Line => "Line",
            Shape::Rectangle => "Rectangle",
            Shape::Ellipse => "Ellipse",
            Shape::Triangle => "Triangle",
            Shape::Diamond => "Diamond",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fill {
    /// Only the edge.
    Outline,
    Solid,
    /// Ordered (Bayer) dithering at 25 %, 50 % and 75 % coverage.
    Dither25,
    Dither50,
    Dither75,
    /// Diagonal stripes.
    Stripes,
    /// Crossing diagonal stripes.
    Crosshatch,
}

/// 4x4 Bayer matrix; a cell is covered when its entry is below the coverage.
const BAYER4: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

impl Fill {
    pub const ALL: [Fill; 7] = [
        Fill::Outline,
        Fill::Solid,
        Fill::Dither25,
        Fill::Dither50,
        Fill::Dither75,
        Fill::Stripes,
        Fill::Crosshatch,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Fill::Outline => "Outline",
            Fill::Solid => "Solid",
            Fill::Dither25 => "Dither 25%",
            Fill::Dither50 => "Dither 50%",
            Fill::Dither75 => "Dither 75%",
            Fill::Stripes => "Stripes",
            Fill::Crosshatch => "Crosshatch",
        }
    }

    /// Whether the pattern paints the cell `(x, y)`.
    #[inline]
    pub fn covers(self, x: i64, y: i64) -> bool {
        let bayer = BAYER4[((y & 3) * 4 + (x & 3)) as usize];
        match self {
            Fill::Outline | Fill::Solid => true,
            Fill::Dither25 => bayer < 4,
            Fill::Dither50 => bayer < 8,
            Fill::Dither75 => bayer < 12,
            Fill::Stripes => (x - y).rem_euclid(3) == 0,
            Fill::Crosshatch => (x - y).rem_euclid(4) == 0 || (x + y).rem_euclid(4) == 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeStyle {
    pub shape: Shape,
    pub fill: Fill,
    /// Also draw a solid edge around a patterned or solid fill.
    pub border: bool,
    /// Edge thickness: the edge is drawn with a `(2*half+1)`-cell square brush.
    pub half: i64,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self { shape: Shape::Rectangle, fill: Fill::Outline, border: false, half: 0 }
    }
}

/// Cells covered by `style` for a drag from `a` to `b` (both inclusive),
/// restricted to `clip`. `out` is cleared first and filled in row-major order
/// without duplicates. Nothing is produced for absurdly large requests.
pub fn shape_cells(
    style: &ShapeStyle,
    a: (i64, i64),
    b: (i64, i64),
    clip: Clip,
    out: &mut Vec<(i64, i64)>,
) {
    out.clear();
    let h = style.half.max(0);
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));

    // Everything we can touch: the box grown by the brush, inside the clip.
    let (rx0, ry0) = ((x0 - h).max(clip.0.0), (y0 - h).max(clip.0.1));
    let (rx1, ry1) = ((x1 + h).min(clip.1.0), (y1 + h).min(clip.1.1));
    if rx0 > rx1 || ry0 > ry1 {
        return;
    }
    let (w, hh) = (rx1 - rx0 + 1, ry1 - ry0 + 1);
    if w.saturating_mul(hh) > MAX_AREA {
        return;
    }
    let is_line = style.shape == Shape::Line;
    if (is_line || matches!(style.shape, Shape::Triangle | Shape::Diamond))
        && (x1 - x0).max(y1 - y0) > MAX_EDGE
    {
        return;
    }

    let mut mask = Mask { x0: rx0, y0: ry0, w, h: hh, bits: vec![false; (w * hh) as usize] };

    // ---- interior ---------------------------------------------------------
    if !is_line && style.fill != Fill::Outline {
        let inside = Inside::new(style.shape, x0, y0, x1, y1);
        for y in y0.max(clip.0.1)..=y1.min(clip.1.1) {
            for x in x0.max(clip.0.0)..=x1.min(clip.1.0) {
                if style.fill.covers(x, y) && inside.test(x, y) {
                    mask.set(x, y);
                }
            }
        }
    }

    // ---- edge -------------------------------------------------------------
    if is_line || style.fill == Fill::Outline || style.border {
        // Only edge cells whose brush can reach the region matter.
        let reach: Clip = ((rx0 - h, ry0 - h), (rx1 + h, ry1 + h));
        // A line is patterned like a fill (dashes, dots); other edges are solid.
        let pattern = if is_line { style.fill } else { Fill::Solid };
        let mut stamp = |cx: i64, cy: i64| {
            if cx < reach.0.0 || cx > reach.1.0 || cy < reach.0.1 || cy > reach.1.1 {
                return;
            }
            for y in (cy - h).max(ry0)..=(cy + h).min(ry1) {
                for x in (cx - h).max(rx0)..=(cx + h).min(rx1) {
                    if pattern.covers(x, y) {
                        mask.set(x, y);
                    }
                }
            }
        };
        edge_cells(style.shape, a, b, (x0, y0, x1, y1), reach, &mut stamp);
    }

    for (i, &on) in mask.bits.iter().enumerate() {
        if on {
            out.push((rx0 + i as i64 % w, ry0 + i as i64 / w));
        }
    }
}

struct Mask {
    x0: i64,
    y0: i64,
    w: i64,
    h: i64,
    bits: Vec<bool>,
}

impl Mask {
    #[inline]
    fn set(&mut self, x: i64, y: i64) {
        let (dx, dy) = (x - self.x0, y - self.y0);
        if dx >= 0 && dy >= 0 && dx < self.w && dy < self.h {
            self.bits[(dy * self.w + dx) as usize] = true;
        }
    }
}

/// "Is this cell inside the shape" for the filled shapes.
enum Inside {
    /// The whole bounding box (rectangles, and polygons squashed to a line).
    Box { x0: i64, y0: i64, x1: i64, y1: i64 },
    Ellipse { cx: f64, cy: f64, a: f64, b: f64 },
    Polygon { pts: [(f64, f64); 4], n: usize, sign: f64 },
}

impl Inside {
    fn new(shape: Shape, x0: i64, y0: i64, x1: i64, y1: i64) -> Self {
        match shape {
            Shape::Ellipse => {
                let (w, h) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
                Inside::Ellipse { cx: x0 as f64 + w / 2.0, cy: y0 as f64 + h / 2.0, a: w / 2.0, b: h / 2.0 }
            }
            Shape::Triangle | Shape::Diamond if x0 != x1 && y0 != y1 => {
                let (xf0, yf0) = (x0 as f64 + 0.5, y0 as f64 + 0.5);
                let (xf1, yf1) = (x1 as f64 + 0.5, y1 as f64 + 0.5);
                let (mx, my) = ((xf0 + xf1) / 2.0, (yf0 + yf1) / 2.0);
                let (pts, n) = if shape == Shape::Triangle {
                    ([(mx, yf0), (xf1, yf1), (xf0, yf1), (0.0, 0.0)], 3)
                } else {
                    ([(mx, yf0), (xf1, my), (mx, yf1), (xf0, my)], 4)
                };
                // Orientation, so the same test works for either winding.
                let mut area = 0.0;
                for i in 0..n {
                    let (p, q) = (pts[i], pts[(i + 1) % n]);
                    area += p.0 * q.1 - q.0 * p.1;
                }
                Inside::Polygon { pts, n, sign: if area < 0.0 { -1.0 } else { 1.0 } }
            }
            _ => Inside::Box { x0, y0, x1, y1 },
        }
    }

    #[inline]
    fn test(&self, x: i64, y: i64) -> bool {
        match *self {
            Inside::Box { x0, y0, x1, y1 } => x >= x0 && x <= x1 && y >= y0 && y <= y1,
            Inside::Ellipse { cx, cy, a, b } => {
                let dx = (x as f64 + 0.5 - cx) / a;
                let dy = (y as f64 + 0.5 - cy) / b;
                dx * dx + dy * dy <= 1.0
            }
            Inside::Polygon { pts, n, sign } => {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                (0..n).all(|i| {
                    let (p, q) = (pts[i], pts[(i + 1) % n]);
                    let cross = (q.0 - p.0) * (py - p.1) - (q.1 - p.1) * (px - p.0);
                    cross * sign >= -1e-9
                })
            }
        }
    }
}

/// Calls `emit` for every one-cell-thin edge cell of the shape that lies in
/// `reach` (cells outside may or may not be reported).
fn edge_cells(
    shape: Shape,
    a: (i64, i64),
    b: (i64, i64),
    (x0, y0, x1, y1): (i64, i64, i64, i64),
    reach: Clip,
    emit: &mut impl FnMut(i64, i64),
) {
    let (lx0, ly0) = (x0.max(reach.0.0), y0.max(reach.0.1));
    let (lx1, ly1) = (x1.min(reach.1.0), y1.min(reach.1.1));
    match shape {
        Shape::Line => {
            // Always walk from the same end so a drag and its reverse agree.
            let (p, q) = if a <= b { (a, b) } else { (b, a) };
            line_cells(p, q, |x, y| emit(x, y));
        }
        Shape::Rectangle => {
            for x in lx0..=lx1 {
                if (ly0..=ly1).contains(&y0) {
                    emit(x, y0);
                }
                if (ly0..=ly1).contains(&y1) {
                    emit(x, y1);
                }
            }
            for y in ly0..=ly1 {
                if (lx0..=lx1).contains(&x0) {
                    emit(x0, y);
                }
                if (lx0..=lx1).contains(&x1) {
                    emit(x1, y);
                }
            }
        }
        Shape::Ellipse | Shape::Triangle | Shape::Diamond => {
            let inside = Inside::new(shape, x0, y0, x1, y1);
            // Cells that are inside but touch the outside.
            for y in ly0..=ly1 {
                for x in lx0..=lx1 {
                    if inside.test(x, y)
                        && !(inside.test(x - 1, y)
                            && inside.test(x + 1, y)
                            && inside.test(x, y - 1)
                            && inside.test(x, y + 1))
                    {
                        emit(x, y);
                    }
                }
            }
            // Thin, steep polygons have rows with no cell centre inside, so
            // their edges are also walked explicitly to keep them connected.
            if shape != Shape::Ellipse {
                let (mx, my) = ((x0 + x1) / 2, (y0 + y1) / 2);
                let verts: &[(i64, i64)] = if shape == Shape::Triangle {
                    &[(mx, y0), (x1, y1), (x0, y1)]
                } else {
                    &[(mx, y0), (x1, my), (mx, y1), (x0, my)]
                };
                for i in 0..verts.len() {
                    let (p, q) = (verts[i], verts[(i + 1) % verts.len()]);
                    let (p, q) = if p <= q { (p, q) } else { (q, p) };
                    line_cells(p, q, |x, y| emit(x, y));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const BIG: Clip = ((-10_000, -10_000), (10_000, 10_000));

    fn cells(style: ShapeStyle, a: (i64, i64), b: (i64, i64)) -> Vec<(i64, i64)> {
        let mut v = Vec::new();
        shape_cells(&style, a, b, BIG, &mut v);
        v
    }

    fn style(shape: Shape, fill: Fill) -> ShapeStyle {
        ShapeStyle { shape, fill, border: false, half: 0 }
    }

    #[test]
    fn rectangle_outline_is_a_one_cell_ring() {
        let v = cells(style(Shape::Rectangle, Fill::Outline), (2, 3), (11, 8)); // 10 x 6
        assert_eq!(v.len(), 10 * 6 - 8 * 4);
        let set: HashSet<_> = v.iter().copied().collect();
        assert!(set.contains(&(2, 3)) && set.contains(&(11, 8)) && set.contains(&(2, 8)));
        assert!(!set.contains(&(5, 5)), "interior is empty");
    }

    #[test]
    fn solid_rectangle_covers_the_whole_box_whichever_way_it_is_dragged() {
        let s = style(Shape::Rectangle, Fill::Solid);
        let v = cells(s, (11, 8), (2, 3));
        assert_eq!(v.len(), 60);
        assert_eq!(v, cells(s, (2, 3), (11, 8)));
        assert_eq!(v, cells(s, (2, 8), (11, 3)));
    }

    #[test]
    fn a_single_click_makes_a_single_cell() {
        for shape in Shape::ALL {
            for fill in [Fill::Outline, Fill::Solid] {
                let v = cells(style(shape, fill), (5, -7), (5, -7));
                assert_eq!(v, vec![(5, -7)], "{shape:?} {fill:?}");
            }
        }
    }

    #[test]
    fn ellipse_is_symmetric_and_roughly_pi_over_four_of_its_box() {
        let v = cells(style(Shape::Ellipse, Fill::Solid), (0, 0), (39, 39));
        let set: HashSet<_> = v.iter().copied().collect();
        let ratio = v.len() as f64 / 1600.0;
        assert!((ratio - std::f64::consts::FRAC_PI_4).abs() < 0.02, "{ratio}");
        for &(x, y) in &v {
            assert!(set.contains(&(39 - x, y)) && set.contains(&(x, 39 - y)));
        }
        assert!(!set.contains(&(0, 0)) && set.contains(&(20, 20)));
    }

    #[test]
    fn ellipse_outline_is_a_closed_thin_curve() {
        let v = cells(style(Shape::Ellipse, Fill::Outline), (0, 0), (30, 20));
        let set: HashSet<_> = v.iter().copied().collect();
        let filled = cells(style(Shape::Ellipse, Fill::Solid), (0, 0), (30, 20));
        assert!(v.len() < filled.len() / 2);
        // Every outline cell has an 8-neighbour on the outline (no loose dots)...
        for &(x, y) in &v {
            let n = (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter(|&d| d != (0, 0) && set.contains(&(x + d.0, y + d.1)))
                .count();
            assert!(n >= 2, "({x},{y}) has {n} neighbours");
        }
        // ... and the interior is empty.
        assert!(!set.contains(&(15, 10)));
    }

    #[test]
    fn dither_patterns_hit_their_coverage() {
        for (fill, expect) in [
            (Fill::Dither25, 0.25),
            (Fill::Dither50, 0.5),
            (Fill::Dither75, 0.75),
            (Fill::Stripes, 1.0 / 3.0),
            (Fill::Crosshatch, 0.375), // two 1/4 diagonals overlapping on 1/8
        ] {
            let v = cells(style(Shape::Rectangle, fill), (0, 0), (63, 63));
            let got = v.len() as f64 / 4096.0;
            assert!((got - expect).abs() < 0.01, "{fill:?}: {got} vs {expect}");
        }
    }

    #[test]
    fn patterns_are_anchored_to_the_grid_not_to_the_shape() {
        // The same cell is painted or not regardless of where the drag started.
        let s = style(Shape::Rectangle, Fill::Dither50);
        let a: HashSet<_> = cells(s, (0, 0), (15, 15)).into_iter().collect();
        let b: HashSet<_> = cells(s, (-3, -5), (20, 30)).into_iter().collect();
        for x in 0..=15 {
            for y in 0..=15 {
                assert_eq!(a.contains(&(x, y)), b.contains(&(x, y)), "({x},{y})");
            }
        }
        // Negative coordinates still follow the same pattern.
        assert!(Fill::Dither50.covers(0, 0) == Fill::Dither50.covers(4, -4));
    }

    #[test]
    fn border_adds_a_solid_edge_to_a_pattern() {
        let mut s = style(Shape::Rectangle, Fill::Dither25);
        let plain: HashSet<_> = cells(s, (0, 0), (9, 9)).into_iter().collect();
        s.border = true;
        let with: HashSet<_> = cells(s, (0, 0), (9, 9)).into_iter().collect();
        assert!(with.len() > plain.len());
        for i in 0..10 {
            for e in [(i, 0), (i, 9), (0, i), (9, i)] {
                assert!(with.contains(&e), "{e:?}");
            }
        }
        assert!(plain.is_subset(&with));
    }

    #[test]
    fn thicker_edges_grow_outward_and_inward() {
        let mut s = style(Shape::Rectangle, Fill::Outline);
        s.half = 1;
        let set: HashSet<_> = cells(s, (10, 10), (29, 29)).into_iter().collect();
        assert!(set.contains(&(9, 9)) && set.contains(&(11, 11)) && set.contains(&(30, 30)));
        assert!(!set.contains(&(12, 12)) && !set.contains(&(8, 8)));
        assert_eq!(set.len(), 22 * 22 - 16 * 16); // outer box 9..=30, hole 12..=27
    }

    #[test]
    fn line_matches_bresenham_and_thick_lines_are_wider() {
        let v = cells(style(Shape::Line, Fill::Outline), (0, 0), (10, 3));
        let mut want = Vec::new();
        line_cells((0, 0), (10, 3), |x, y| want.push((x, y)));
        let (mut v2, mut want) = (v.clone(), want);
        v2.sort();
        want.sort();
        assert_eq!(v2, want);
        // Same line whichever end the drag started from.
        let mut rev = cells(style(Shape::Line, Fill::Outline), (10, 3), (0, 0));
        rev.sort();
        assert_eq!(rev, v2);

        let mut thick = style(Shape::Line, Fill::Outline);
        thick.half = 2;
        assert!(cells(thick, (0, 0), (10, 3)).len() > 4 * v.len());
    }

    #[test]
    fn patterned_lines_become_dashed() {
        let solid = cells(style(Shape::Line, Fill::Solid), (0, 0), (63, 0)).len();
        let dashed = cells(style(Shape::Line, Fill::Dither50), (0, 0), (63, 0)).len();
        assert_eq!(solid, 64);
        assert!(dashed > 10 && dashed < solid, "{dashed}");
    }

    #[test]
    fn triangle_and_diamond_have_the_right_extent_and_area() {
        let tri = cells(style(Shape::Triangle, Fill::Solid), (0, 0), (40, 40));
        let set: HashSet<_> = tri.iter().copied().collect();
        assert!(set.contains(&(20, 0)), "apex");
        assert!(set.contains(&(0, 40)) && set.contains(&(40, 40)), "base corners");
        assert!(!set.contains(&(0, 0)) && !set.contains(&(40, 0)));
        let ratio = tri.len() as f64 / (41.0 * 41.0);
        assert!((ratio - 0.5).abs() < 0.05, "{ratio}");

        let dia = cells(style(Shape::Diamond, Fill::Solid), (0, 0), (40, 40));
        let ratio = dia.len() as f64 / (41.0 * 41.0);
        assert!((ratio - 0.5).abs() < 0.05, "{ratio}");
        let set: HashSet<_> = dia.iter().copied().collect();
        assert!(set.contains(&(20, 0)) && set.contains(&(40, 20)) && set.contains(&(0, 20)));
        assert!(!set.contains(&(1, 1)));
    }

    #[test]
    fn thin_polygons_stay_connected_in_outline_mode() {
        let v = cells(style(Shape::Triangle, Fill::Outline), (0, 0), (2, 60));
        let set: HashSet<_> = v.iter().copied().collect();
        for y in 0..=60 {
            assert!((0..=2).any(|x| set.contains(&(x, y))), "row {y} is empty");
        }
    }

    #[test]
    fn output_never_leaves_the_clip_and_is_the_matching_subset() {
        let s = style(Shape::Ellipse, Fill::Solid);
        let full: HashSet<_> = cells(s, (-50, -50), (50, 50)).into_iter().collect();
        let clip: Clip = ((-5, -20), (30, 8));
        let mut v = Vec::new();
        shape_cells(&s, (-50, -50), (50, 50), clip, &mut v);
        assert!(!v.is_empty());
        for &(x, y) in &v {
            assert!(x >= -5 && x <= 30 && y >= -20 && y <= 8);
            assert!(full.contains(&(x, y)));
        }
        let want = full
            .iter()
            .filter(|&&(x, y)| x >= -5 && x <= 30 && y >= -20 && y <= 8)
            .count();
        assert_eq!(v.len(), want, "clipping must not change what is inside");
    }

    #[test]
    fn thick_edge_reaching_into_the_clip_from_outside_is_kept() {
        let mut s = style(Shape::Rectangle, Fill::Outline);
        s.half = 2;
        // The rectangle edge x = 0 lies outside the clip but its brush reaches x = 1, 2.
        let mut v = Vec::new();
        shape_cells(&s, (0, 0), (50, 50), ((1, 10), (40, 12)), &mut v);
        assert!(v.contains(&(1, 11)) && v.contains(&(2, 11)));
        assert!(!v.contains(&(3, 11)));
    }

    #[test]
    fn huge_requests_are_refused_instead_of_hanging() {
        let mut v = Vec::new();
        let s = style(Shape::Rectangle, Fill::Solid);
        shape_cells(&s, (0, 0), (100_000, 100_000), ((0, 0), (100_000, 100_000)), &mut v);
        assert!(v.is_empty());
        let line = style(Shape::Line, Fill::Solid);
        shape_cells(&line, (0, 0), (1 << 40, 5), ((0, 0), (10, 10)), &mut v);
        assert!(v.is_empty());
    }

    #[test]
    fn output_has_no_duplicates() {
        let mut s = style(Shape::Diamond, Fill::Solid);
        s.border = true;
        s.half = 2;
        let v = cells(s, (3, 3), (40, 30));
        let set: HashSet<_> = v.iter().copied().collect();
        assert_eq!(set.len(), v.len());
    }
}
