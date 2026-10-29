/// Maximum number of distinct colours in one map.
pub const MAX_COLORS: usize = 256;

/// Colours every new map starts with (indices `0..DEFAULT_COLORS.len()`).
pub const DEFAULT_COLORS: [[u8; 3]; 16] = [
    [255, 255, 255], // white
    [34, 34, 38],    // charcoal
    [150, 146, 140], // stone grey
    [168, 124, 80],  // wood
    [122, 90, 62],   // dirt
    [106, 152, 80],  // grass
    [224, 202, 142], // sand
    [118, 176, 216], // shallow water
    [52, 104, 170],  // deep water
    [234, 98, 34],   // lava
    [196, 60, 60],   // red
    [140, 86, 36],   // door brown
    [120, 80, 160],  // purple
    [240, 210, 70],  // yellow
    [70, 150, 140],  // teal
    [200, 200, 205], // light grey
];

/// The map's colour table. Cells store an index into it, so any RGB value is
/// possible while cells stay one byte of colour. Append-only, which keeps
/// indices stored in cells and in the undo log valid forever.
///
/// Colours enter the table only when something is actually painted with
/// them, so scrubbing through a colour picker does not fill it up.
#[derive(Clone, Debug)]
pub struct ColorTable {
    colors: Vec<[u8; 3]>,
    version: u64,
}

impl Default for ColorTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorTable {
    pub fn new() -> Self {
        Self {
            colors: DEFAULT_COLORS.to_vec(),
            version: 1,
        }
    }

    /// Builds a table from loaded data. Empty input falls back to defaults;
    /// extra entries beyond the limit are ignored.
    pub fn from_vec(mut colors: Vec<[u8; 3]>) -> Self {
        colors.truncate(MAX_COLORS);
        if colors.is_empty() {
            return Self::new();
        }
        Self { colors, version: 1 }
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    pub fn as_slice(&self) -> &[[u8; 3]] {
        &self.colors
    }

    /// Colour for an index; out-of-range indices (corrupt data) give white.
    pub fn get(&self, index: u8) -> [u8; 3] {
        self.colors.get(index as usize).copied().unwrap_or([255; 3])
    }

    /// Changes whenever a colour is added, so a renderer knows when to
    /// re-upload the table.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Index of `rgb`, adding it to the table if it is new. When the table
    /// is full, the nearest existing colour is returned instead.
    pub fn index_of(&mut self, rgb: [u8; 3]) -> u8 {
        if let Some(i) = self.colors.iter().position(|&c| c == rgb) {
            return i as u8;
        }
        if self.colors.len() < MAX_COLORS {
            self.colors.push(rgb);
            self.version += 1;
            return (self.colors.len() - 1) as u8;
        }
        self.nearest(rgb)
    }

    /// Index of the existing colour closest to `rgb` (never adds).
    pub fn nearest(&self, rgb: [u8; 3]) -> u8 {
        let dist = |c: &[u8; 3]| -> i32 {
            (0..3).map(|i| (c[i] as i32 - rgb[i] as i32).pow(2)).sum()
        };
        self.colors
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| dist(c))
            .map_or(0, |(i, _)| i as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_colours_are_reused_and_new_ones_appended() {
        let mut t = ColorTable::new();
        let n = t.len();
        let v = t.version();
        assert_eq!(t.index_of(DEFAULT_COLORS[5]), 5);
        assert_eq!(t.version(), v, "no change, no new version");
        let i = t.index_of([1, 2, 3]);
        assert_eq!(i as usize, n);
        assert_eq!(t.get(i), [1, 2, 3]);
        assert_eq!(t.index_of([1, 2, 3]), i);
        assert!(t.version() > v);
    }

    #[test]
    fn full_table_falls_back_to_nearest() {
        let mut t = ColorTable::new();
        for i in 0..400u32 {
            t.index_of([(i % 256) as u8, (i / 256) as u8, 7]);
        }
        assert_eq!(t.len(), MAX_COLORS);
        let v = t.version();
        let idx = t.index_of([200, 0, 7]);
        assert_eq!(t.get(idx), [200, 0, 7]); // exact colour already present
        let idx = t.index_of([9, 9, 250]);
        assert_eq!(t.len(), MAX_COLORS);
        assert_eq!(t.version(), v);
        let _ = t.get(idx);
    }

    #[test]
    fn out_of_range_index_is_white_and_empty_input_gives_defaults() {
        let t = ColorTable::from_vec(vec![[1, 1, 1]]);
        assert_eq!(t.get(50), [255, 255, 255]);
        assert_eq!(ColorTable::from_vec(vec![]).len(), DEFAULT_COLORS.len());
    }
}
