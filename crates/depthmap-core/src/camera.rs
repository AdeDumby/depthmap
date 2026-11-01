use crate::{MAX_DEPTH, SUBDIV};

/// When cells at the active depth grow past this many pixels, the camera
/// re-bases onto the next (finer) depth, where cells are `SUBDIV`× smaller.
pub const ZOOM_HI: f64 = 64.0;
/// When they shrink below this, it re-bases onto the parent depth.
pub const ZOOM_LO: f64 = 16.0;
/// Zoom-out limit at depth 0.
pub const MIN_PPC: f64 = 0.25;
/// Pixels per cell of the default view (depth 0, "×1").
pub const BASE_PPC: f64 = 32.0;
/// Smallest and largest zoom levels the camera can reach (see [`Camera::zoom_level`]).
pub const MIN_ZOOM_LEVEL: f64 = -7.0; // log2(MIN_PPC / BASE_PPC)
pub const MAX_ZOOM_LEVEL: f64 = 2.0 * MAX_DEPTH as f64 + 1.0;

/// One depth's view of the camera: position and scale expressed in that
/// depth's own integer cell grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerView {
    pub depth: u8,
    /// Cell under the viewport centre.
    pub cell: (i64, i64),
    /// Position inside that cell, each in `[0, 1)`.
    pub frac: (f64, f64),
    /// Pixels per cell at this depth.
    pub ppc: f64,
}

impl LayerView {
    /// Screen position (pixels, y down) of the top-left corner of a cell.
    ///
    /// The subtraction happens in integers before converting to float, so
    /// the result stays precise however deep the camera is.
    #[inline]
    pub fn cell_origin_px(&self, cx: i64, cy: i64, vw: f64, vh: f64) -> (f64, f64) {
        (
            ((cx - self.cell.0) as f64 - self.frac.0) * self.ppc + vw * 0.5,
            ((cy - self.cell.1) as f64 - self.frac.1) * self.ppc + vh * 0.5,
        )
    }

    /// Inclusive range of cells touching a `vw` x `vh` pixel viewport.
    pub fn visible_cells(&self, vw: f64, vh: f64) -> ((i64, i64), (i64, i64)) {
        let hx = vw * 0.5 / self.ppc;
        let hy = vh * 0.5 / self.ppc;
        (
            (
                self.cell.0 + (self.frac.0 - hx).floor() as i64,
                self.cell.1 + (self.frac.1 - hy).floor() as i64,
            ),
            (
                self.cell.0 + (self.frac.0 + hx).floor() as i64,
                self.cell.1 + (self.frac.1 + hy).floor() as i64,
            ),
        )
    }
}

/// The editing camera. `depth` is the layer you paint on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub depth: u8,
    pub cell: (i64, i64),
    pub frac: (f64, f64),
    pub ppc: f64,
    /// Zoom-out limit, in pixels per cell *at depth 0* (see [`MIN_PPC`]).
    pub min_ppc: f64,
    /// Deepest depth the camera may descend to (the map's depth).
    pub max_depth: u8,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    pub fn new() -> Self {
        Self {
            depth: 0,
            cell: (0, 0),
            frac: (0.0, 0.0),
            ppc: BASE_PPC,
            min_ppc: MIN_PPC,
            max_depth: MAX_DEPTH,
        }
    }

    /// Lowest and highest [`zoom_level`](Self::zoom_level) allowed by this
    /// camera's limits.
    pub fn zoom_range(&self) -> (f64, f64) {
        ((self.min_ppc / BASE_PPC).log2(), 2.0 * self.max_depth as f64 + 1.0)
    }

    /// Total zoom as a base-2 logarithm: `0` is the default view, `+1`
    /// doubles the scale, and each depth level adds `2` (cells shrink 4×).
    /// A single number that is continuous across depth hops, for sliders.
    pub fn zoom_level(&self) -> f64 {
        (self.ppc / BASE_PPC).log2() + 2.0 * self.depth as f64
    }

    /// Zooms to an absolute `zoom_level`, keeping the screen point
    /// `(sx, sy)` fixed.
    pub fn set_zoom_level(&mut self, level: f64, sx: f64, sy: f64, vw: f64, vh: f64) {
        let (lo, hi) = self.zoom_range();
        let level = level.clamp(lo, hi);
        let factor = (level - self.zoom_level()).exp2();
        self.zoom_at(factor, sx, sy, vw, vh);
    }

    /// The view at the active (painting) depth.
    pub fn view(&self) -> LayerView {
        self.at_depth(self.depth)
    }

    /// The same camera expressed at another depth. Finer depths multiply the
    /// integer position by `SUBDIV^n`; coarser depths divide it.
    pub fn at_depth(&self, k: u8) -> LayerView {
        use std::cmp::Ordering::*;
        match k.cmp(&self.depth) {
            Equal => LayerView {
                depth: k,
                cell: self.cell,
                frac: self.frac,
                ppc: self.ppc,
            },
            Greater => {
                let s = SUBDIV.pow((k - self.depth) as u32);
                let sf = s as f64;
                let (fx, fy) = (self.frac.0 * sf, self.frac.1 * sf);
                let (flx, fly) = (fx.floor(), fy.floor());
                LayerView {
                    depth: k,
                    cell: (self.cell.0 * s + flx as i64, self.cell.1 * s + fly as i64),
                    frac: (fx - flx, fy - fly),
                    ppc: self.ppc / sf,
                }
            }
            Less => {
                let s = SUBDIV.pow((self.depth - k) as u32);
                let sf = s as f64;
                LayerView {
                    depth: k,
                    cell: (self.cell.0.div_euclid(s), self.cell.1.div_euclid(s)),
                    frac: (
                        (self.cell.0.rem_euclid(s) as f64 + self.frac.0) / sf,
                        (self.cell.1.rem_euclid(s) as f64 + self.frac.1) / sf,
                    ),
                    ppc: self.ppc * sf,
                }
            }
        }
    }

    /// Camera centre in depth-0 cell units. Only meaningful at shallow
    /// depths (it is a float!) — used by tests and debugging.
    pub fn world_pos(&self) -> (f64, f64) {
        let s = (SUBDIV as f64).powi(self.depth as i32);
        (
            (self.cell.0 as f64 + self.frac.0) / s,
            (self.cell.1 as f64 + self.frac.1) / s,
        )
    }

    /// Depth-0 world position under a screen point (float; shallow depths).
    pub fn screen_to_world(&self, sx: f64, sy: f64, vw: f64, vh: f64) -> (f64, f64) {
        let s = (SUBDIV as f64).powi(self.depth as i32);
        let (cx, cy) = self.world_pos();
        (
            cx + (sx - vw * 0.5) / self.ppc / s,
            cy + (sy - vh * 0.5) / self.ppc / s,
        )
    }

    /// Cell at the active depth under a screen point.
    pub fn screen_to_cell(&self, sx: f64, sy: f64, vw: f64, vh: f64) -> (i64, i64) {
        let px = self.frac.0 + (sx - vw * 0.5) / self.ppc;
        let py = self.frac.1 + (sy - vh * 0.5) / self.ppc;
        (self.cell.0 + px.floor() as i64, self.cell.1 + py.floor() as i64)
    }

    /// Drag the map by a pixel delta (content follows the cursor).
    pub fn pan_pixels(&mut self, dx: f64, dy: f64) {
        self.frac.0 -= dx / self.ppc;
        self.frac.1 -= dy / self.ppc;
        self.normalize();
    }

    /// Multiply the zoom by `factor`, keeping the world point under the
    /// screen position `(sx, sy)` fixed.
    pub fn zoom_at(&mut self, factor: f64, sx: f64, sy: f64, vw: f64, vh: f64) {
        let mut new_ppc = self.ppc * factor;
        if self.depth == 0 {
            new_ppc = new_ppc.max(self.min_ppc);
        }
        if self.depth >= self.max_depth {
            new_ppc = new_ppc.min(ZOOM_HI);
        }
        let f = new_ppc / self.ppc;
        let ox = (sx - vw * 0.5) / self.ppc;
        let oy = (sy - vh * 0.5) / self.ppc;
        let k = 1.0 - 1.0 / f;
        self.ppc = new_ppc;
        self.frac.0 += ox * k;
        self.frac.1 += oy * k;
        self.normalize();
    }

    /// Restores the invariants: `frac` in `[0,1)` and `ppc` inside the
    /// depth's comfortable range, hopping between depths as needed. A hop
    /// changes the numbers but not the point being looked at.
    pub fn normalize(&mut self) {
        self.carry();
        let n = SUBDIV;
        let nf = n as f64;
        // A camera left deeper than the map allows climbs back up first.
        while self.depth > self.max_depth {
            self.frac = (
                (self.cell.0.rem_euclid(n) as f64 + self.frac.0) / nf,
                (self.cell.1.rem_euclid(n) as f64 + self.frac.1) / nf,
            );
            self.cell = (self.cell.0.div_euclid(n), self.cell.1.div_euclid(n));
            self.ppc *= nf;
            self.depth -= 1;
        }
        while self.ppc >= ZOOM_HI && self.depth < self.max_depth {
            let (fx, fy) = (self.frac.0 * nf, self.frac.1 * nf);
            let (flx, fly) = (fx.floor(), fy.floor());
            self.cell = (self.cell.0 * n + flx as i64, self.cell.1 * n + fly as i64);
            self.frac = (fx - flx, fy - fly);
            self.ppc /= nf;
            self.depth += 1;
        }
        while self.ppc < ZOOM_LO && self.depth > 0 {
            self.frac = (
                (self.cell.0.rem_euclid(n) as f64 + self.frac.0) / nf,
                (self.cell.1.rem_euclid(n) as f64 + self.frac.1) / nf,
            );
            self.cell = (self.cell.0.div_euclid(n), self.cell.1.div_euclid(n));
            self.ppc *= nf;
            self.depth -= 1;
        }
        if self.depth == 0 {
            self.ppc = self.ppc.max(self.min_ppc);
        }
        if self.depth == self.max_depth {
            self.ppc = self.ppc.min(ZOOM_HI);
        }
    }

    fn carry(&mut self) {
        let fx = self.frac.0.floor();
        let fy = self.frac.1.floor();
        self.cell.0 += fx as i64;
        self.cell.1 += fy as i64;
        self.frac.0 -= fx;
        self.frac.1 -= fy;
        // `x - floor(x)` can round up to exactly 1.0 for tiny negative x.
        if self.frac.0 >= 1.0 {
            self.frac.0 = 0.0;
            self.cell.0 += 1;
        }
        if self.frac.1 >= 1.0 {
            self.frac.1 = 0.0;
            self.cell.1 += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VW: f64 = 1280.0;
    const VH: f64 = 720.0;

    fn close(a: (f64, f64), b: (f64, f64), eps: f64) -> bool {
        (a.0 - b.0).abs() < eps && (a.1 - b.1).abs() < eps
    }

    #[test]
    fn zooming_at_the_centre_keeps_position_across_depth_hops() {
        let mut c = Camera::new();
        c.cell = (3, -5);
        c.frac = (0.3, 0.7);
        let before = c.world_pos();
        for _ in 0..40 {
            c.zoom_at(1.5, VW * 0.5, VH * 0.5, VW, VH);
        }
        assert!(c.depth >= 3, "should have descended, got depth {}", c.depth);
        assert!(close(c.world_pos(), before, 1e-9));
        for _ in 0..40 {
            c.zoom_at(1.0 / 1.5, VW * 0.5, VH * 0.5, VW, VH);
        }
        assert_eq!(c.depth, 0);
        assert!(close(c.world_pos(), before, 1e-9));
        assert!((c.ppc - 32.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_fixed() {
        let mut c = Camera::new();
        c.cell = (10, 4);
        c.frac = (0.25, 0.75);
        let (sx, sy) = (1000.0, 120.0);
        let anchor = c.screen_to_world(sx, sy, VW, VH);
        for _ in 0..12 {
            c.zoom_at(1.7, sx, sy, VW, VH);
            assert!(close(c.screen_to_world(sx, sy, VW, VH), anchor, 1e-8));
        }
        for _ in 0..12 {
            c.zoom_at(1.0 / 1.7, sx, sy, VW, VH);
            assert!(close(c.screen_to_world(sx, sy, VW, VH), anchor, 1e-8));
        }
    }

    #[test]
    fn views_at_different_depths_agree_on_cell_positions() {
        let mut c = Camera::new();
        c.cell = (-7, 13);
        c.frac = (0.9, 0.1);
        c.depth = 3;
        c.ppc = 40.0;
        let here = c.view();
        for k in 0..=6u8 {
            let v = c.at_depth(k);
            // A cell at depth 3 and the aligned cell at depth k must start
            // at the same screen position.
            let (cx, cy) = (here.cell.0 + 2, here.cell.1 - 1);
            let want = here.cell_origin_px(cx, cy, VW, VH);
            let (kx, ky) = if k >= 3 {
                let s = SUBDIV.pow((k - 3) as u32);
                (cx * s, cy * s)
            } else {
                let s = SUBDIV.pow((3 - k) as u32);
                (cx.div_euclid(s), cy.div_euclid(s))
            };
            // Only exact when the coarse cell is aligned to the fine one.
            if k >= 3 {
                let got = v.cell_origin_px(kx, ky, VW, VH);
                assert!(close(got, want, 1e-6), "k={k}: {got:?} vs {want:?}");
            }
            assert!((v.ppc - 40.0 * (SUBDIV as f64).powi(3 - k as i32)).abs() < 1e-9);
        }
    }

    #[test]
    fn deep_zoom_stays_pixel_accurate() {
        let mut c = Camera::new();
        let (sx, sy) = (777.0, 333.0);
        for _ in 0..200 {
            c.zoom_at(1.25, sx, sy, VW, VH);
        }
        assert!(c.depth >= 10);
        let v = c.view();
        let cell = c.screen_to_cell(sx, sy, VW, VH);
        let (ox, oy) = v.cell_origin_px(cell.0, cell.1, VW, VH);
        assert!(ox <= sx + 1e-6 && sx < ox + v.ppc + 1e-6, "{ox} {sx} {}", v.ppc);
        assert!(oy <= sy + 1e-6 && sy < oy + v.ppc + 1e-6);
        // Panning one pixel moves a cell by exactly one pixel.
        let before = v.cell_origin_px(cell.0, cell.1, VW, VH);
        c.pan_pixels(1.0, 0.0);
        let after = c.view().cell_origin_px(cell.0, cell.1, VW, VH);
        assert!((after.0 - before.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_level_is_continuous_across_depth_hops() {
        let mut c = Camera::new();
        assert!(c.zoom_level().abs() < 1e-12);
        let mut last = c.zoom_level();
        for _ in 0..60 {
            c.zoom_at(1.3, 10.0, 10.0, VW, VH);
            let l = c.zoom_level();
            assert!((l - last - 1.3f64.log2()).abs() < 1e-9, "jump at depth {}", c.depth);
            last = l;
        }
    }

    #[test]
    fn set_zoom_level_lands_exactly_and_respects_limits() {
        let mut c = Camera::new();
        c.set_zoom_level(6.0, VW * 0.5, VH * 0.5, VW, VH);
        assert!((c.zoom_level() - 6.0).abs() < 1e-9);
        assert_eq!(c.depth, 3); // 2 levels per depth, ppc back at 32 * 2^0
        c.set_zoom_level(-100.0, 0.0, 0.0, VW, VH);
        assert!((c.zoom_level() - MIN_ZOOM_LEVEL).abs() < 1e-9);
        c.set_zoom_level(1000.0, 0.0, 0.0, VW, VH);
        assert!(c.depth <= MAX_DEPTH);
    }

    #[test]
    fn zoom_limits_hold() {
        let mut c = Camera::new();
        for _ in 0..100 {
            c.zoom_at(0.5, 0.0, 0.0, VW, VH);
        }
        assert_eq!((c.depth, c.ppc), (0, MIN_PPC));
        for _ in 0..500 {
            c.zoom_at(2.0, 0.0, 0.0, VW, VH);
        }
        assert_eq!(c.depth, MAX_DEPTH);
        assert!(c.ppc <= ZOOM_HI);
    }

    #[test]
    fn map_limits_cap_depth_and_zoom_out() {
        let mut c = Camera::new();
        c.max_depth = 2;
        c.min_ppc = 4.0;
        for _ in 0..200 {
            c.zoom_at(2.0, 10.0, 10.0, VW, VH);
        }
        assert_eq!(c.depth, 2);
        assert!(c.ppc <= ZOOM_HI);
        for _ in 0..200 {
            c.zoom_at(0.5, 10.0, 10.0, VW, VH);
        }
        assert_eq!((c.depth, c.ppc), (0, 4.0));
        let (lo, hi) = c.zoom_range();
        assert!((lo - (4.0f64 / 32.0).log2()).abs() < 1e-12 && hi == 5.0);
        c.set_zoom_level(100.0, 0.0, 0.0, VW, VH);
        assert!((c.zoom_level() - 5.0).abs() < 1e-9 && c.depth == 2);
        c.set_zoom_level(-100.0, 0.0, 0.0, VW, VH);
        assert!((c.zoom_level() - lo).abs() < 1e-9);
    }

    #[test]
    fn a_camera_deeper_than_the_map_climbs_back_up() {
        let mut c = Camera::new();
        c.depth = 5;
        c.cell = (4100, 9000);
        c.ppc = 20.0;
        c.max_depth = 3;
        let before = c.world_pos();
        c.normalize();
        assert_eq!(c.depth, 3);
        assert!(c.ppc <= ZOOM_HI);
        let after = c.world_pos();
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
    }

    #[test]
    fn visible_cells_cover_the_viewport() {
        let mut c = Camera::new();
        c.cell = (-2, 9);
        c.frac = (0.5, 0.5);
        let v = c.view();
        let ((x0, y0), (x1, y1)) = v.visible_cells(VW, VH);
        // Everything outside the range is off screen.
        let (ox, oy) = v.cell_origin_px(x0, y0, VW, VH);
        assert!(ox <= 0.0 && oy <= 0.0);
        let (ex, ey) = v.cell_origin_px(x1 + 1, y1 + 1, VW, VH);
        assert!(ex >= VW && ey >= VH);
    }
}
