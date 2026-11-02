//! Nodes: named points of interest placed on the map.

use crate::MAX_DEPTH;

pub type NodeId = u64;

pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_DESCRIPTION_CHARS: usize = 4000;
/// Icon size multiplier range (1.0 = one cell of the node's depth wide).
pub const MIN_SCALE: f32 = 0.1;
pub const MAX_SCALE: f32 = 10.0;

/// Where a node's icon sits relative to the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Snap {
    /// Centred on a cell when the icon is an odd number of cells wide, on a
    /// grid corner when even, so a 2x icon covers exactly 2x2 cells.
    #[default]
    Auto,
    /// Always on the centre of a cell.
    Centre,
    /// Always on a grid corner (where four cells meet).
    Edge,
}

impl Snap {
    pub const ALL: [Snap; 3] = [Snap::Auto, Snap::Centre, Snap::Edge];

    pub fn name(self) -> &'static str {
        match self {
            Snap::Auto => "Auto",
            Snap::Centre => "Centre",
            Snap::Edge => "Edge",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Snap::Auto => "Centre of a cell for odd sizes (1x, 3x...), grid corner for even sizes (2x, 4x...)",
            Snap::Centre => "Centre of a cell",
            Snap::Edge => "On a grid corner, where four cells meet",
        }
    }

    pub fn to_byte(self) -> u8 {
        match self {
            Snap::Auto => 0,
            Snap::Centre => 1,
            Snap::Edge => 2,
        }
    }

    pub fn from_byte(b: u8) -> Option<Snap> {
        Some(match b {
            0 => Snap::Auto,
            1 => Snap::Centre,
            2 => Snap::Edge,
            _ => return None,
        })
    }

    /// Whether an icon of this scale sits on a grid corner.
    pub fn is_edge(self, scale: f32) -> bool {
        match self {
            Snap::Centre => false,
            Snap::Edge => true,
            Snap::Auto => (scale.round().max(1.0) as i64) % 2 == 0,
        }
    }
}

/// What a node looks like.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// One of the procedural icons (`1..=icons::BUILTIN_COUNT`; `0` is the
    /// "missing icon" placeholder).
    Builtin(u8),
    /// An imported PNG, by file name (see `icons::valid_icon_name`).
    Custom(String),
}

impl Default for Icon {
    fn default() -> Self {
        Icon::Builtin(1)
    }
}

/// A point of interest, sitting on a cell of one depth.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub depth: u8,
    pub x: i64,
    pub y: i64,
    pub name: String,
    pub description: String,
    pub icon: Icon,
    /// Icon size relative to a cell of `depth`.
    pub scale: f32,
    pub snap: Snap,
    /// The icon sits on the grid corner at the bottom-right of cell `x, y`
    /// instead of at the centre of it (see [`Snap`]).
    pub edge: bool,
}

impl Node {
    pub fn new(id: NodeId, depth: u8, x: i64, y: i64, icon: Icon) -> Self {
        Self { id, depth, x, y, name: String::new(), description: String::new(), icon, scale: 1.0, snap: Snap::Auto, edge: false }
    }

    /// Where the icon's centre is, in cells of `depth`.
    pub fn position(&self) -> (f64, f64) {
        let o = if self.edge { 1.0 } else { 0.5 };
        (self.x as f64 + o, self.y as f64 + o)
    }

    /// Puts the node at the spot of cell space `(fx, fy)` (fractional cells
    /// of `depth`), snapped the way [`Snap`] says for the current scale.
    pub fn place_at(&mut self, fx: f64, fy: f64) {
        self.edge = self.snap.is_edge(self.scale);
        if self.edge {
            // The nearest grid corner is the bottom-right corner of the cell before it.
            (self.x, self.y) = (fx.round() as i64 - 1, fy.round() as i64 - 1);
        } else {
            (self.x, self.y) = (fx.floor() as i64, fy.floor() as i64);
        }
    }

    /// After a change of size or snapping: keeps the node on its cell but
    /// moves it onto the centre or corner the snapping asks for.
    pub fn resnap(&mut self) {
        self.edge = self.snap.is_edge(self.scale);
    }

    /// Clamps everything into what the program supports: text length, no
    /// line breaks in names, a sane scale, a valid depth.
    pub fn sanitized(mut self) -> Self {
        self.name = self
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME_CHARS)
            .collect();
        self.description = self
            .description
            .chars()
            .filter(|&c| !c.is_control() || c == '\n' || c == '\t')
            .take(MAX_DESCRIPTION_CHARS)
            .collect();
        self.scale = if self.scale.is_finite() {
            self.scale.clamp(MIN_SCALE, MAX_SCALE)
        } else {
            1.0
        };
        self.depth = self.depth.min(MAX_DEPTH);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_snapping_follows_the_icon_size() {
        for (scale, edge) in [(0.1, false), (0.6, false), (1.0, false), (1.4, false), (2.0, true), (2.6, false), (4.0, true), (10.0, true)] {
            assert_eq!(Snap::Auto.is_edge(scale), edge, "{scale}");
        }
        assert!(!Snap::Centre.is_edge(2.0) && Snap::Edge.is_edge(1.0));
        for s in Snap::ALL {
            assert_eq!(Snap::from_byte(s.to_byte()), Some(s));
        }
        assert_eq!(Snap::from_byte(9), None);
    }

    #[test]
    fn placing_snaps_to_centres_or_corners() {
        let mut n = Node::new(1, 0, 0, 0, Icon::default());
        n.snap = Snap::Centre;
        n.place_at(3.2, 4.9);
        assert_eq!((n.x, n.y, n.edge, n.position()), (3, 4, false, (3.5, 4.5)));
        n.snap = Snap::Edge;
        n.place_at(3.2, 4.9); // nearest corner is (3, 5)
        assert_eq!(n.position(), (3.0, 5.0));
        assert!(n.edge);
        n.place_at(3.6, 4.4); // nearest corner is (4, 4)
        assert_eq!(n.position(), (4.0, 4.0));
        // A 2x icon on its corner covers the 2x2 cells around it.
        n.snap = Snap::Auto;
        n.scale = 2.0;
        n.place_at(10.4, 10.6);
        let (cx, cy) = n.position();
        assert_eq!((cx - 1.0, cy - 1.0, cx + 1.0, cy + 1.0), (9.0, 10.0, 11.0, 12.0));
        n.scale = 1.0;
        n.resnap();
        assert!(!n.edge);
    }

    #[test]
    fn sanitizing_limits_text_and_scale() {
        let mut n = Node::new(1, 2, 3, 4, Icon::default());
        n.name = format!("a\nb{}", "x".repeat(500));
        n.description = format!("line1\nline2\u{7}{}", "y".repeat(9000));
        n.scale = f32::NAN;
        let n = n.sanitized();
        assert_eq!(n.name.chars().count(), MAX_NAME_CHARS);
        assert!(!n.name.contains('\n'));
        assert_eq!(n.description.chars().count(), MAX_DESCRIPTION_CHARS);
        assert!(n.description.starts_with("line1\nline2y"));
        assert_eq!(n.scale, 1.0);
        let mut big = Node::new(1, 0, 0, 0, Icon::default());
        big.scale = 100.0;
        assert_eq!(big.clone().sanitized().scale, MAX_SCALE);
        big.scale = 0.001;
        assert_eq!(big.sanitized().scale, MIN_SCALE);
    }
}
