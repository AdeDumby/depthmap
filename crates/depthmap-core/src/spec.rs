use crate::MAX_DEPTH;

/// Smallest and largest map size, in cells of depth 0.
pub const MIN_MAP_SIZE: u32 = 4;
pub const MAX_MAP_SIZE: u32 = 1024;

/// The shape of a map, chosen when it is created and saved with it.
///
/// * `width` x `height` is the size of the map in cells of depth 0, i.e. what
///   you see when fully zoomed out. Depth `d` has `4^d` times as many cells
///   along each axis inside the same area.
/// * `max_depth` is how many times you can zoom into a cell: depths
///   `0..=max_depth` exist.
///
/// A size of `0 x 0` means "unbounded" (infinite canvas); it is what maps
/// from before sizes existed load as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapSpec {
    pub width: u32,
    pub height: u32,
    pub max_depth: u8,
}

impl MapSpec {
    /// An infinite canvas with every depth available.
    pub const UNBOUNDED: MapSpec = MapSpec { width: 0, height: 0, max_depth: MAX_DEPTH };

    /// What the New Map dialog offers first.
    pub const DEFAULT_NEW: MapSpec = MapSpec { width: 128, height: 128, max_depth: 3 };

    /// A bounded map; the numbers are clamped into the supported range.
    pub fn new(width: u32, height: u32, max_depth: u8) -> Self {
        Self {
            width: width.clamp(MIN_MAP_SIZE, MAX_MAP_SIZE),
            height: height.clamp(MIN_MAP_SIZE, MAX_MAP_SIZE),
            max_depth: max_depth.min(MAX_DEPTH),
        }
    }

    pub fn is_bounded(&self) -> bool {
        self.width != 0 && self.height != 0
    }

    /// Whether the numbers are something this program can work with. Used to
    /// reject hostile or corrupt files.
    pub fn is_valid(&self) -> bool {
        self.max_depth <= MAX_DEPTH
            && match (self.width, self.height) {
                (0, 0) => true,
                (w, h) => {
                    (MIN_MAP_SIZE..=MAX_MAP_SIZE).contains(&w)
                        && (MIN_MAP_SIZE..=MAX_MAP_SIZE).contains(&h)
                }
            }
    }

    /// Size of the map in cells of `depth`, or `None` when unbounded.
    pub fn extent(&self, depth: u8) -> Option<(i64, i64)> {
        self.is_bounded().then(|| {
            let shift = 2 * depth as u32; // SUBDIV = 4
            ((self.width as i64) << shift, (self.height as i64) << shift)
        })
    }

    /// True if the cell exists in this map.
    #[inline]
    pub fn contains(&self, depth: u8, x: i64, y: i64) -> bool {
        if depth > self.max_depth {
            return false;
        }
        match self.extent(depth) {
            Some((w, h)) => x >= 0 && y >= 0 && x < w && y < h,
            None => true,
        }
    }

    /// Cells at the finest depth along each axis (for the New Map dialog).
    pub fn finest_extent(&self) -> Option<(i64, i64)> {
        self.extent(self.max_depth)
    }
}

impl Default for MapSpec {
    fn default() -> Self {
        Self::UNBOUNDED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_grows_four_times_per_depth() {
        let s = MapSpec::new(64, 32, 3);
        assert_eq!(s.extent(0), Some((64, 32)));
        assert_eq!(s.extent(2), Some((1024, 512)));
        assert_eq!(s.finest_extent(), Some((4096, 2048)));
        assert_eq!(MapSpec::UNBOUNDED.extent(5), None);
    }

    #[test]
    fn contains_respects_edges_and_depth() {
        let s = MapSpec::new(8, 8, 2);
        assert!(s.contains(0, 0, 0) && s.contains(0, 7, 7));
        assert!(!s.contains(0, 8, 0) && !s.contains(0, 0, -1) && !s.contains(0, -1, 3));
        assert!(s.contains(2, 127, 127) && !s.contains(2, 128, 0));
        assert!(!s.contains(3, 0, 0), "deeper than the map allows");
        assert!(MapSpec::UNBOUNDED.contains(16, -5, 1 << 40));
        assert!(!MapSpec::UNBOUNDED.contains(17, 0, 0));
    }

    #[test]
    fn new_clamps_and_validity_is_checked() {
        let s = MapSpec::new(1, 100_000, 99);
        assert_eq!((s.width, s.height, s.max_depth), (MIN_MAP_SIZE, MAX_MAP_SIZE, MAX_DEPTH));
        assert!(s.is_valid() && MapSpec::UNBOUNDED.is_valid());
        assert!(!MapSpec { width: 0, height: 8, max_depth: 1 }.is_valid());
        assert!(!MapSpec { width: 8, height: 8, max_depth: 17 }.is_valid());
        assert!(!MapSpec { width: 2000, height: 8, max_depth: 1 }.is_valid());
    }

    #[test]
    fn deepest_extent_fits_an_i64() {
        let s = MapSpec::new(MAX_MAP_SIZE, MAX_MAP_SIZE, MAX_DEPTH);
        let (w, _) = s.finest_extent().unwrap();
        assert_eq!(w, 1024i64 << 32);
    }
}
