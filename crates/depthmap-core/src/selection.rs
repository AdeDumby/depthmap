//! Selections and flood regions.
//!
//! Both are stored as **row spans**: for every row, a sorted list of
//! inclusive `(x0, x1)` runs. A filled rectangle or a lake costs one span per
//! row instead of one entry per cell, so even huge regions are cheap, and
//! "everything" is just an inverted empty selection.

use std::collections::BTreeMap;

use crate::{CHUNK_SIZE, Cell, EMPTY, World};

/// Sorted, disjoint, non-touching inclusive runs of x.
pub type Spans = Vec<(i64, i64)>;
pub type Rows = BTreeMap<i64, Spans>;

/// A selection (or a flood region) never holds more rows than this.
pub const MAX_SELECTION_ROWS: usize = 1 << 21;
/// The largest region flood fill and magic select will work on, in cells.
pub const MAX_REGION_CELLS: u64 = 2_000_000;

// ---- span arithmetic ------------------------------------------------------

/// Sorts and merges overlapping or touching runs.
fn normalized(mut v: Spans) -> Spans {
    v.sort_unstable();
    let mut out: Spans = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

pub fn spans_union(a: &[(i64, i64)], b: &[(i64, i64)]) -> Spans {
    normalized(a.iter().chain(b).copied().collect())
}

pub fn spans_intersect(a: &[(i64, i64)], b: &[(i64, i64)]) -> Spans {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo <= hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The parts of `a` that are not in `b`.
pub fn spans_subtract(a: &[(i64, i64)], b: &[(i64, i64)]) -> Spans {
    let mut out = Vec::new();
    let mut j = 0;
    for &(mut lo, hi) in a {
        while j < b.len() && b[j].1 < lo {
            j += 1;
        }
        let mut k = j;
        while lo <= hi {
            match b.get(k) {
                Some(&(bl, bh)) if bl <= hi => {
                    if bl > lo {
                        out.push((lo, bl - 1));
                    }
                    lo = bh.saturating_add(1);
                    k += 1;
                    if bh == i64::MAX {
                        break;
                    }
                }
                _ => {
                    out.push((lo, hi));
                    break;
                }
            }
        }
    }
    out
}

pub fn spans_contain(s: &[(i64, i64)], x: i64) -> bool {
    let i = s.partition_point(|&(_, hi)| hi < x);
    s.get(i).is_some_and(|&(lo, _)| lo <= x)
}

fn spans_clip(s: &[(i64, i64)], lo: i64, hi: i64) -> Spans {
    s.iter()
        .filter(|&&(a, b)| b >= lo && a <= hi)
        .map(|&(a, b)| (a.max(lo), b.min(hi)))
        .collect()
}

fn insert_span(v: &mut Spans, span: (i64, i64)) {
    let i = v.partition_point(|&(_, hi)| hi.saturating_add(1) < span.0);
    let mut merged = span;
    let mut j = i;
    while j < v.len() && v[j].0 <= merged.1.saturating_add(1) {
        merged = (merged.0.min(v[j].0), merged.1.max(v[j].1));
        j += 1;
    }
    v.splice(i..j, [merged]);
}

fn combine(a: &Rows, b: &Rows, f: impl Fn(&[(i64, i64)], &[(i64, i64)]) -> Spans) -> Rows {
    let mut out = Rows::new();
    let empty: Spans = Vec::new();
    let mut keys: Vec<i64> = a.keys().chain(b.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    for y in keys {
        let s = f(a.get(&y).unwrap_or(&empty), b.get(&y).unwrap_or(&empty));
        if !s.is_empty() {
            out.insert(y, s);
        }
    }
    out
}

/// `(x, y)` at depth `from`, expressed at depth `to`: the containing cell
/// going coarser, the middle sub-cell going finer.
fn convert(x: i64, y: i64, from: u8, to: u8) -> (i64, i64) {
    if to == from {
        return (x, y);
    }
    if to < from {
        let s = 2 * (from - to) as u32;
        return (x >> s, y >> s);
    }
    let s = 2 * (to - from) as u32;
    let mid = (1i128 << s) >> 1;
    let up = |v: i64| ((v as i128) << s) + mid;
    let clamp = |v: i128| v.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    (clamp(up(x)), clamp(up(y)))
}

// ---- selection ------------------------------------------------------------

/// A set of cells of one depth. Cells of other depths belong to it when the
/// cell that stands for them at this depth does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    depth: u8,
    rows: Rows,
    /// The selection is everything *except* `rows`.
    inverted: bool,
}

/// Consecutive rows `y0..=y1` that share the same spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub y0: i64,
    pub y1: i64,
    pub spans: Spans,
}

impl Selection {
    /// Every cell.
    pub fn all(depth: u8) -> Self {
        Self { depth, rows: Rows::new(), inverted: true }
    }

    /// The inclusive rectangle between two corner cells, in any order.
    pub fn rect(depth: u8, a: (i64, i64), b: (i64, i64)) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        let mut rows = Rows::new();
        for y in y0..=y1 {
            rows.insert(y, vec![(x0, x1)]);
        }
        Self { depth, rows, inverted: false }
    }

    pub fn from_rows(depth: u8, rows: Rows) -> Self {
        Self { depth, rows, inverted: false }
    }

    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn is_inverted(&self) -> bool {
        self.inverted
    }

    /// True when nothing is selected.
    pub fn is_empty(&self) -> bool {
        !self.inverted && self.rows.is_empty()
    }

    pub fn invert(&mut self) {
        self.inverted = !self.inverted;
    }

    /// Number of cells, or `None` for an inverted selection (unbounded).
    pub fn cell_count(&self) -> Option<u64> {
        (!self.inverted).then(|| {
            self.rows
                .values()
                .flat_map(|s| s.iter())
                .map(|&(a, b)| (b - a) as u64 + 1)
                .sum()
        })
    }

    pub fn contains(&self, depth: u8, x: i64, y: i64) -> bool {
        let (x, y) = convert(x, y, depth, self.depth);
        let inside = self.rows.get(&y).is_some_and(|s| spans_contain(s, x));
        inside != self.inverted
    }

    /// The same selection at a finer (or equal) depth; `None` if that would
    /// be too big or `depth` is coarser.
    pub fn rebased(&self, depth: u8) -> Option<Selection> {
        if depth < self.depth {
            return None;
        }
        if depth == self.depth {
            return Some(self.clone());
        }
        let shift = 2 * (depth - self.depth) as u32;
        let f = 1i64 << shift;
        if self.rows.len().checked_mul(f as usize)? > MAX_SELECTION_ROWS {
            return None;
        }
        let mut rows = Rows::new();
        for (&y, spans) in &self.rows {
            let scaled: Spans = spans
                .iter()
                .map(|&(a, b)| Some((a.checked_mul(f)?, b.checked_add(1)?.checked_mul(f)? - 1)))
                .collect::<Option<_>>()?;
            let y0 = y.checked_mul(f)?;
            for k in 0..f {
                rows.insert(y0 + k, scaled.clone());
            }
        }
        Some(Selection { depth, rows, inverted: self.inverted })
    }

    /// Adds `other` to this selection (OR). Returns false, changing nothing,
    /// when the result would be too large to store.
    pub fn union(&mut self, other: &Selection) -> bool {
        let depth = self.depth.max(other.depth);
        let (Some(a), Some(b)) = (self.rebased(depth), other.rebased(depth)) else {
            return false;
        };
        let (rows, inverted) = match (a.inverted, b.inverted) {
            (false, false) => (combine(&a.rows, &b.rows, spans_union), false),
            (true, false) => (combine(&a.rows, &b.rows, spans_subtract), true),
            (false, true) => (combine(&b.rows, &a.rows, spans_subtract), true),
            (true, true) => (combine(&a.rows, &b.rows, spans_intersect), true),
        };
        *self = Selection { depth, rows, inverted };
        true
    }

    /// The selected runs of row `y` within `lo..=hi`.
    pub fn row_spans(&self, y: i64, lo: i64, hi: i64) -> Spans {
        let base = self.rows.get(&y).map_or(&[][..], |s| &s[..]);
        if self.inverted {
            spans_subtract(&[(lo, hi)], base)
        } else {
            spans_clip(base, lo, hi)
        }
    }

    /// The selected cells inside the window `x0..=x1` × `y0..=y1`, as
    /// blocks of identical rows, top to bottom. Used for drawing.
    pub fn blocks(&self, x0: i64, x1: i64, y0: i64, y1: i64) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        if x1 < x0 || y1 < y0 {
            return out;
        }
        let mut push = |y0: i64, y1: i64, spans: Spans| {
            if spans.is_empty() {
                return;
            }
            match out.last_mut() {
                Some(l) if l.y1 + 1 == y0 && l.spans == spans => l.y1 = y1,
                _ => out.push(Block { y0, y1, spans }),
            }
        };
        let mut next = y0;
        for (&y, _) in self.rows.range(y0..=y1) {
            if y > next && self.inverted {
                push(next, y - 1, vec![(x0, x1)]);
            }
            push(y, y, self.row_spans(y, x0, x1));
            next = y + 1;
        }
        if next <= y1 && self.inverted {
            push(next, y1, vec![(x0, x1)]);
        }
        out
    }
}

// ---- flood regions --------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionError {
    /// More than [`MAX_REGION_CELLS`] cells (an open area, or just huge).
    TooLarge,
    /// Matching every empty cell of the map makes no sense.
    Everything,
}

/// All cells connected (4-way) to `start` for which `matches` is true.
/// Empty when `start` itself does not match.
pub fn flood_region(
    start: (i64, i64),
    mut matches: impl FnMut(i64, i64) -> bool,
    max_cells: u64,
) -> Result<Rows, RegionError> {
    let mut rows = Rows::new();
    if !matches(start.0, start.1) {
        return Ok(rows);
    }
    let mut count = 0u64;
    let mut stack = vec![start];
    let seen = |rows: &Rows, x: i64, y: i64| rows.get(&y).is_some_and(|s| spans_contain(s, x));
    while let Some((x, y)) = stack.pop() {
        if seen(&rows, x, y) || !matches(x, y) {
            continue;
        }
        let (mut l, mut r) = (x, x);
        count += 1;
        while !seen(&rows, l - 1, y) && matches(l - 1, y) {
            l -= 1;
            count += 1;
            if count > max_cells {
                return Err(RegionError::TooLarge);
            }
        }
        while !seen(&rows, r + 1, y) && matches(r + 1, y) {
            r += 1;
            count += 1;
            if count > max_cells {
                return Err(RegionError::TooLarge);
            }
        }
        if count > max_cells || rows.len() > MAX_SELECTION_ROWS {
            return Err(RegionError::TooLarge);
        }
        insert_span(rows.entry(y).or_default(), (l, r));
        for ny in [y - 1, y + 1] {
            let mut cx = l;
            while cx <= r {
                if matches(cx, ny) {
                    if !seen(&rows, cx, ny) {
                        stack.push((cx, ny));
                    }
                    while cx <= r && matches(cx, ny) {
                        cx += 1;
                    }
                } else {
                    cx += 1;
                }
            }
        }
    }
    Ok(rows)
}

/// The connected region of cells equal to the one at `start`, limited to
/// the map and to cells for which `allow` is true.
pub fn region_of_same(
    world: &World,
    depth: u8,
    start: (i64, i64),
    mut allow: impl FnMut(i64, i64) -> bool,
) -> Result<Rows, RegionError> {
    if !world.in_bounds(depth, start.0, start.1) {
        return Ok(Rows::new());
    }
    let target: Cell = world.get(depth, start.0, start.1);
    flood_region(
        start,
        |x, y| world.in_bounds(depth, x, y) && world.get(depth, x, y) == target && allow(x, y),
        MAX_REGION_CELLS,
    )
}

/// Every cell of the layer equal to the one at `start`, connected or not.
/// Empty cells cannot be matched this way.
pub fn region_of_same_everywhere(
    world: &World,
    depth: u8,
    start: (i64, i64),
    mut allow: impl FnMut(i64, i64) -> bool,
) -> Result<Rows, RegionError> {
    if !world.in_bounds(depth, start.0, start.1) {
        return Ok(Rows::new());
    }
    let target = world.get(depth, start.0, start.1);
    if target == EMPTY {
        return Err(RegionError::Everything);
    }
    let mut rows = Rows::new();
    let mut count = 0u64;
    for (pos, chunk) in world.iter_chunks(depth) {
        let (ox, oy) = pos.origin();
        let cells = chunk.cells();
        for ly in 0..CHUNK_SIZE {
            let y = oy + ly;
            let mut run: Option<i64> = None;
            for lx in 0..=CHUNK_SIZE {
                let hit = lx < CHUNK_SIZE
                    && cells[(ly * CHUNK_SIZE + lx) as usize] == target
                    && allow(ox + lx, y);
                match (hit, run) {
                    (true, None) => run = Some(lx),
                    (false, Some(s)) => {
                        count += (lx - s) as u64;
                        if count > MAX_REGION_CELLS * 4 || rows.len() > MAX_SELECTION_ROWS {
                            return Err(RegionError::TooLarge);
                        }
                        insert_span(rows.entry(y).or_default(), (ox + s, ox + lx - 1));
                        run = None;
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(rows)
}

/// Number of cells in a set of rows.
pub fn rows_cell_count(rows: &Rows) -> u64 {
    rows.values().flat_map(|s| s.iter()).map(|&(a, b)| (b - a) as u64 + 1).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MapSpec, make_cell};

    #[test]
    fn span_operations() {
        let a = vec![(0, 4), (10, 12)];
        let b = vec![(3, 10)];
        assert_eq!(spans_union(&a, &b), vec![(0, 12)]);
        assert_eq!(spans_intersect(&a, &b), vec![(3, 4), (10, 10)]);
        assert_eq!(spans_subtract(&a, &b), vec![(0, 2), (11, 12)]);
        assert_eq!(spans_subtract(&[(0, 9)], &[(2, 3), (6, 6)]), vec![(0, 1), (4, 5), (7, 9)]);
        assert_eq!(spans_union(&[(0, 1)], &[(2, 3)]), vec![(0, 3)], "touching runs merge");
        assert!(spans_contain(&a, 11) && !spans_contain(&a, 5));
        let mut v = vec![(0, 1), (5, 6)];
        insert_span(&mut v, (2, 4));
        assert_eq!(v, vec![(0, 6)]);
    }

    #[test]
    fn rectangles_contain_exactly_their_cells_whatever_the_corner_order() {
        let s = Selection::rect(2, (3, 5), (1, 2));
        assert_eq!(s.cell_count(), Some(3 * 4));
        assert!(s.contains(2, 1, 2) && s.contains(2, 3, 5));
        assert!(!s.contains(2, 0, 2) && !s.contains(2, 4, 5) && !s.contains(2, 2, 6));
        assert!(Selection::rect(0, (0, 0), (0, 0)).contains(0, 0, 0));
    }

    #[test]
    fn inverting_selects_everything_else() {
        let mut s = Selection::rect(0, (0, 0), (2, 2));
        s.invert();
        assert!(!s.contains(0, 1, 1) && s.contains(0, 3, 1) && s.contains(0, -50, 9000));
        assert_eq!(s.cell_count(), None);
        s.invert();
        assert!(s.contains(0, 1, 1));
        let mut all = Selection::all(1);
        assert!(all.contains(1, 7, 7) && !all.is_empty());
        all.invert();
        assert!(all.is_empty());
    }

    #[test]
    fn union_is_a_real_or_for_every_inversion_combination() {
        let a0 = Selection::rect(0, (0, 0), (3, 3));
        let b0 = Selection::rect(0, (2, 2), (5, 5));
        for ia in [false, true] {
            for ib in [false, true] {
                let (mut a, mut b) = (a0.clone(), b0.clone());
                if ia {
                    a.invert();
                }
                if ib {
                    b.invert();
                }
                let mut u = a.clone();
                assert!(u.union(&b));
                for y in -2..9 {
                    for x in -2..9 {
                        assert_eq!(
                            u.contains(0, x, y),
                            a.contains(0, x, y) || b.contains(0, x, y),
                            "ia={ia} ib={ib} at {x},{y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn selections_of_different_depths_combine_at_the_finer_one() {
        let mut s = Selection::rect(0, (0, 0), (1, 0)); // two coarse cells
        let fine = Selection::rect(1, (20, 20), (21, 20));
        assert!(s.union(&fine));
        assert_eq!(s.depth(), 1);
        // coarse cell (1,0) is fine cells 4..8 x 0..4
        assert!(s.contains(1, 7, 3) && s.contains(1, 0, 0) && !s.contains(1, 8, 0));
        assert!(s.contains(1, 20, 20) && s.contains(1, 21, 20));
        // Asking from other depths: cell (1,0) of depth 0 is inside.
        assert!(s.contains(0, 1, 0) && !s.contains(0, 2, 0));
        assert!(s.contains(2, 4 * 7 + 1, 4 * 3 + 1), "child of a selected cell");
    }

    #[test]
    fn coarse_cells_count_by_their_middle_child() {
        // A fine selection covering the middle of coarse cell (0,0) only.
        let s = Selection::rect(1, (2, 2), (2, 2));
        assert!(s.contains(0, 0, 0));
        let s = Selection::rect(1, (0, 0), (0, 0));
        assert!(!s.contains(0, 0, 0));
    }

    #[test]
    fn rebasing_refuses_huge_results() {
        let tall = Selection::rect(0, (0, 0), (0, 100_000));
        assert!(tall.rebased(0).is_some());
        assert!(tall.rebased(1).is_some());
        assert!(tall.rebased(3).is_none(), "100k rows x 64 is too many");
        let mut t = tall.clone();
        assert!(!t.union(&Selection::rect(3, (0, 0), (0, 0))));
        assert_eq!(t, tall, "failed union changes nothing");
        assert!(Selection::rect(2, (0, 0), (0, 0)).rebased(1).is_none(), "never coarser");
    }

    #[test]
    fn blocks_merge_identical_rows_and_follow_inversion() {
        let s = Selection::rect(0, (2, 3), (5, 6));
        let b = s.blocks(0, 10, 0, 10);
        assert_eq!(b, vec![Block { y0: 3, y1: 6, spans: vec![(2, 5)] }]);
        assert_eq!(s.blocks(0, 3, 4, 5), vec![Block { y0: 4, y1: 5, spans: vec![(2, 3)] }]);
        assert!(s.blocks(20, 30, 0, 10).is_empty());

        let mut inv = s.clone();
        inv.invert();
        let b = inv.blocks(0, 10, 0, 10);
        assert_eq!(b.first().unwrap(), &Block { y0: 0, y1: 2, spans: vec![(0, 10)] });
        assert_eq!(b[1], Block { y0: 3, y1: 6, spans: vec![(0, 1), (6, 10)] });
        assert_eq!(b.last().unwrap(), &Block { y0: 7, y1: 10, spans: vec![(0, 10)] });
        assert_eq!(Selection::all(0).blocks(0, 4, 0, 4), vec![Block { y0: 0, y1: 4, spans: vec![(0, 4)] }]);
    }

    fn painted(w: &mut World, depth: u8, picture: &[&str]) {
        for (y, row) in picture.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                let c = match ch {
                    '#' => make_cell(2, 1),
                    'o' => make_cell(2, 5),
                    _ => EMPTY,
                };
                w.set(depth, x as i64, y as i64, c);
            }
        }
    }

    #[test]
    fn flooding_follows_connected_equal_cells_only() {
        let mut w = World::with_spec(MapSpec::new(8, 8, 1));
        painted(
            &mut w,
            0,
            &["#####...", "#...#...", "#.#.#...", "#...#...", "#####...", "........", "........", "........"],
        );
        // The room inside the walls, around the pillar.
        let room = region_of_same(&w, 0, (1, 1), |_, _| true).unwrap();
        assert_eq!(rows_cell_count(&room), 8);
        assert!(!room.get(&2).unwrap().contains(&(2, 2)), "the pillar is not part of it");
        // Outside the walls is a different, bigger empty region.
        let out = region_of_same(&w, 0, (7, 7), |_, _| true).unwrap();
        assert_eq!(rows_cell_count(&out), 64 - 25);
        // The wall ring; the pillar is a separate piece.
        let wall = region_of_same(&w, 0, (0, 0), |_, _| true).unwrap();
        assert_eq!(rows_cell_count(&wall), 16);
    }

    #[test]
    fn flood_respects_the_allow_mask_and_size_limit() {
        let mut w = World::with_spec(MapSpec::new(8, 8, 1));
        painted(&mut w, 0, &["........"]);
        // Only the left half is allowed: the flood stops at the mask.
        let half = region_of_same(&w, 0, (0, 0), |x, _| x < 4).unwrap();
        assert_eq!(rows_cell_count(&half), 4 * 8);
        assert!(region_of_same(&w, 0, (7, 0), |x, _| x < 4).unwrap().is_empty());
        // An open unbounded canvas is too large to flood.
        let open = World::new();
        assert_eq!(region_of_same(&open, 0, (0, 0), |_, _| true), Err(RegionError::TooLarge));
        // Outside the map: nothing.
        assert!(region_of_same(&w, 0, (50, 50), |_, _| true).unwrap().is_empty());
    }

    #[test]
    fn same_everywhere_finds_disconnected_cells_but_not_empty_ones() {
        let mut w = World::with_spec(MapSpec::new(64, 64, 1));
        let red = make_cell(2, 5);
        for &(x, y) in &[(1, 1), (40, 3), (-0 + 2, 60), (33, 33)] {
            w.set(0, x, y, red);
        }
        w.set(0, 5, 5, make_cell(2, 6));
        w.set(0, 6, 6, make_cell(3, 5));
        let r = region_of_same_everywhere(&w, 0, (1, 1), |_, _| true).unwrap();
        assert_eq!(rows_cell_count(&r), 4);
        assert!(r.get(&3).unwrap().contains(&(40, 40)));
        let masked = region_of_same_everywhere(&w, 0, (1, 1), |x, _| x < 10).unwrap();
        assert_eq!(rows_cell_count(&masked), 2);
        assert_eq!(region_of_same_everywhere(&w, 0, (9, 9), |_, _| true), Err(RegionError::Everything));
    }
}
