//! Named, coloured regions of the map ("zones"): the boundaries of places.
//!
//! A zone is a set of cells at the depth it was drawn on (a [`Selection`]).
//! Selections already answer "does this cell of any depth belong to me", so a
//! zone drawn at one depth shows up on every other depth.

use crate::selection::{Rows, Selection};

pub const MAX_ZONE_NAME: usize = 60;

/// Colours new zones cycle through.
pub const ZONE_COLORS: [[u8; 3]; 8] = [
    [231, 76, 60],
    [52, 152, 219],
    [46, 204, 113],
    [241, 196, 15],
    [155, 89, 182],
    [230, 126, 34],
    [26, 188, 156],
    [236, 112, 190],
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    pub name: String,
    pub color: [u8; 3],
    pub visible: bool,
    area: Selection,
}

impl Zone {
    pub fn new(name: &str, color: [u8; 3], depth: u8) -> Self {
        Self {
            name: name.chars().take(MAX_ZONE_NAME).collect(),
            color,
            visible: true,
            area: Selection::from_rows(depth, Rows::new()),
        }
    }

    pub fn from_area(name: &str, color: [u8; 3], visible: bool, area: Selection) -> Self {
        let mut z = Self::new(name, color, area.depth());
        z.visible = visible;
        z.area = area;
        z
    }

    pub fn area(&self) -> &Selection {
        &self.area
    }

    pub fn depth(&self) -> u8 {
        self.area.depth()
    }

    pub fn is_empty(&self) -> bool {
        self.area.is_empty()
    }

    pub fn contains(&self, depth: u8, x: i64, y: i64) -> bool {
        self.area.contains(depth, x, y)
    }

    /// Adds cells. False (nothing changed) when the result would be too big.
    pub fn add(&mut self, other: &Selection) -> bool {
        self.area.union(other)
    }

    /// Removes cells. False (nothing changed) when too big or unsupported.
    pub fn subtract(&mut self, other: &Selection) -> bool {
        self.area.subtract(other)
    }
}

/// Why a lasso made no region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LassoError {
    /// Fewer than three points, or no area.
    TooSmall,
    /// Taller than the row limit.
    TooTall,
}

/// The cells whose centres lie inside the polygon `pts` (even-odd rule).
/// Points are in cell units relative to `anchor`; the result is absolute and
/// clipped to `extent` (`width, height` in cells) when the map is bounded.
pub fn lasso_rows(
    anchor: (i64, i64),
    pts: &[(f64, f64)],
    extent: Option<(i64, i64)>,
    max_rows: i64,
) -> Result<Rows, LassoError> {
    if pts.len() < 3 {
        return Err(LassoError::TooSmall);
    }
    let (min_y, max_y) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    // Rows whose centre (y + 0.5) can be crossed by the outline.
    let rel_lo = (min_y - 0.5).ceil() as i64;
    let rel_hi = (max_y - 0.5).floor() as i64;
    if rel_hi < rel_lo {
        return Err(LassoError::TooSmall);
    }
    let (mut y0, mut y1) = (anchor.1 + rel_lo, anchor.1 + rel_hi);
    let (xlo, xhi) = extent.map_or((i64::MIN, i64::MAX), |(w, _)| (0, w - 1));
    if let Some((_, h)) = extent {
        (y0, y1) = (y0.max(0), y1.min(h - 1));
    }
    if y1 < y0 {
        return Ok(Rows::new());
    }
    if y1 - y0 >= max_rows {
        return Err(LassoError::TooTall);
    }
    let mut rows = Rows::new();
    let mut xs: Vec<f64> = Vec::new();
    for y in y0..=y1 {
        let yc = (y - anchor.1) as f64 + 0.5;
        xs.clear();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            // Half-open so a vertex exactly on the scanline counts once.
            if (a.1 <= yc) != (b.1 <= yc) {
                xs.push(a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut spans: Vec<(i64, i64)> = Vec::new();
        for pair in xs.chunks_exact(2) {
            // Cells whose centre x + 0.5 lies in [pair[0], pair[1]).
            let a = anchor.0 + (pair[0] - 0.5).ceil() as i64;
            let b = anchor.0 + (pair[1] - 0.5).ceil() as i64 - 1;
            let (a, b) = (a.max(xlo), b.min(xhi));
            if a <= b {
                match spans.last_mut() {
                    Some((_, last)) if *last + 1 >= a => *last = (*last).max(b),
                    _ => spans.push((a, b)),
                }
            }
        }
        if !spans.is_empty() {
            rows.insert(y, spans);
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::rows_cell_count;

    #[test]
    fn a_square_lasso_takes_the_cells_whose_centres_are_inside() {
        // Corners at (0,0) and (3,2) around anchor (10, 20): cells x 10..=12, y 20..=21.
        let pts = [(0.0, 0.0), (3.0, 0.0), (3.0, 2.0), (0.0, 2.0)];
        let rows = lasso_rows((10, 20), &pts, None, 1000).unwrap();
        assert_eq!(rows_cell_count(&rows), 6);
        assert_eq!(rows[&20], vec![(10, 12)]);
        assert_eq!(rows[&21], vec![(10, 12)]);
    }

    #[test]
    fn a_partly_covered_cell_needs_its_centre_inside() {
        let pts = [(0.0, 0.0), (1.4, 0.0), (1.4, 1.0), (0.0, 1.0)];
        let rows = lasso_rows((0, 0), &pts, None, 1000).unwrap();
        assert_eq!(rows[&0], vec![(0, 0)], "centre 1.5 > 1.4 is outside, 0.5 is inside");
        let pts = [(0.0, 0.0), (1.6, 0.0), (1.6, 1.0), (0.0, 1.0)];
        assert_eq!(lasso_rows((0, 0), &pts, None, 1000).unwrap()[&0], vec![(0, 1)]);
    }

    #[test]
    fn a_triangle_and_a_concave_shape_work() {
        let tri = [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)];
        let rows = lasso_rows((0, 0), &tri, None, 1000).unwrap();
        assert_eq!(rows[&0], vec![(0, 6)]);
        assert_eq!(rows[&6], vec![(0, 0)]);
        assert!(!rows.contains_key(&7));
        // A "U": the notch between the arms stays empty.
        let u = [(0.0, 0.0), (2.0, 0.0), (2.0, 3.0), (4.0, 3.0), (4.0, 0.0), (6.0, 0.0), (6.0, 5.0), (0.0, 5.0)];
        let rows = lasso_rows((0, 0), &u, None, 1000).unwrap();
        assert_eq!(rows[&1], vec![(0, 1), (4, 5)]);
        assert_eq!(rows[&4], vec![(0, 5)]);
    }

    #[test]
    fn it_is_clipped_to_a_bounded_map_and_rejects_nonsense() {
        let pts = [(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)];
        let rows = lasso_rows((0, 0), &pts, Some((3, 3)), 1000).unwrap();
        assert_eq!(rows_cell_count(&rows), 9);
        assert_eq!(lasso_rows((0, 0), &pts[..2], None, 10), Err(LassoError::TooSmall));
        assert_eq!(lasso_rows((0, 0), &pts, None, 4), Err(LassoError::TooTall));
        let line = [(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)];
        assert_eq!(lasso_rows((0, 0), &line, None, 10), Err(LassoError::TooSmall));
    }

    #[test]
    fn zones_add_and_subtract_cells() {
        let mut z = Zone::new("Keep", ZONE_COLORS[0], 1);
        assert!(z.is_empty());
        assert!(z.add(&Selection::rect(1, (0, 0), (3, 3))));
        assert!(z.subtract(&Selection::rect(1, (1, 1), (2, 2))));
        assert_eq!(z.area().cell_count(), Some(12));
        assert!(z.contains(1, 0, 0) && !z.contains(1, 1, 1));
        // Seen from a coarser and a finer depth.
        assert!(z.contains(2, 0, 0) && z.contains(2, 3, 3), "finer cells inside the cell");
        assert_eq!(z.contains(0, 0, 0), z.contains(1, 2, 2), "a coarser cell is judged by its middle sub-cell");
    }
}
