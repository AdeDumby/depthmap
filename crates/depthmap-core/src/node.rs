//! Nodes: named points of interest placed on the map.

use crate::MAX_DEPTH;

pub type NodeId = u64;

pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_DESCRIPTION_CHARS: usize = 4000;
/// Icon size multiplier range (1.0 = one cell of the node's depth wide).
pub const MIN_SCALE: f32 = 0.25;
pub const MAX_SCALE: f32 = 10.0;

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
}

impl Node {
    pub fn new(id: NodeId, depth: u8, x: i64, y: i64, icon: Icon) -> Self {
        Self { id, depth, x, y, name: String::new(), description: String::new(), icon, scale: 1.0 }
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
        assert_eq!(big.sanitized().scale, MAX_SCALE);
    }
}
