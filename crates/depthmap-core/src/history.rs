use crate::node::Node;
use crate::{Cell, World};

/// One changed cell. Undo history stores these deltas, never snapshots, so
/// its cost scales with what you drew, not with the size of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellEdit {
    pub depth: u8,
    pub x: i64,
    pub y: i64,
    pub old: Cell,
    pub new: Cell,
}

/// A change to one node: its state before and after (`None` = absent).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeEdit {
    pub before: Option<Node>,
    pub after: Option<Node>,
}

impl NodeEdit {
    fn id(&self) -> Option<crate::NodeId> {
        self.after.as_ref().or(self.before.as_ref()).map(|n| n.id)
    }
}

enum Entry {
    Cells(Vec<CellEdit>),
    Node(NodeEdit),
}

const MAX_STROKES: usize = 2000;

/// Stroke-grouped undo/redo log (cell strokes and node edits share it).
#[derive(Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    current: Vec<CellEdit>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Paints one cell and records the change in the current stroke.
    pub fn paint(&mut self, world: &mut World, depth: u8, x: i64, y: i64, m: Cell) {
        let old = world.set(depth, x, y, m);
        if old != m {
            self.current.push(CellEdit { depth, x, y, old, new: m });
        }
    }

    /// Records edits that were already applied to the world (a shape that
    /// was previewed live) as one undo step.
    pub fn record_applied(&mut self, edits: Vec<CellEdit>) {
        self.current.extend(edits);
        self.end_stroke();
    }

    /// Closes the current stroke so it becomes one undo step.
    pub fn end_stroke(&mut self) {
        if self.current.is_empty() {
            return;
        }
        self.redo.clear();
        let stroke = std::mem::take(&mut self.current);
        self.push_undo(Entry::Cells(stroke));
    }

    fn push_undo(&mut self, entry: Entry) {
        self.undo.push(entry);
        if self.undo.len() > MAX_STROKES {
            self.undo.remove(0);
        }
    }

    /// Records a node change that was already applied to the world.
    pub fn record_node(&mut self, before: Option<Node>, after: Option<Node>) {
        if before == after {
            return;
        }
        self.end_stroke();
        self.redo.clear();
        self.push_undo(Entry::Node(NodeEdit { before, after }));
    }

    pub fn undo(&mut self, world: &mut World) -> bool {
        self.end_stroke();
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        match &entry {
            Entry::Cells(stroke) => {
                for e in stroke.iter().rev() {
                    world.set(e.depth, e.x, e.y, e.old);
                }
            }
            Entry::Node(e) => {
                if let Some(id) = e.id() {
                    world.set_node_state(id, e.before.as_ref());
                }
            }
        }
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self, world: &mut World) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        match &entry {
            Entry::Cells(stroke) => {
                for e in stroke {
                    world.set(e.depth, e.x, e.y, e.new);
                }
            }
            Entry::Node(e) => {
                if let Some(id) = e.id() {
                    world.set_node_state(id, e.after.as_ref());
                }
            }
        }
        self.undo.push(entry);
        true
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.current.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || !self.current.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EMPTY;

    #[test]
    fn undo_redo_restores_exact_state() {
        let mut w = World::new();
        let mut h = History::new();

        for x in 0..5 {
            h.paint(&mut w, 0, x, 0, 1);
        }
        h.end_stroke();
        // Second stroke overwrites some cells and adds a deeper one.
        h.paint(&mut w, 0, 2, 0, 2);
        h.paint(&mut w, 0, 3, 0, EMPTY);
        h.paint(&mut w, 1, 40, -3, 5);
        h.end_stroke();

        assert!(h.undo(&mut w));
        assert_eq!((w.get(0, 2, 0), w.get(0, 3, 0), w.get(1, 40, -3)), (1, 1, EMPTY));
        assert!(h.undo(&mut w));
        assert_eq!(w.total_chunks(), 0);
        assert!(!h.undo(&mut w));

        assert!(h.redo(&mut w));
        assert!(h.redo(&mut w));
        assert_eq!((w.get(0, 2, 0), w.get(0, 3, 0), w.get(1, 40, -3)), (2, EMPTY, 5));
        assert!(!h.redo(&mut w));
    }

    #[test]
    fn node_edits_undo_and_redo_in_order_with_cell_strokes() {
        use crate::node::Icon;
        let mut w = World::new();
        let mut h = History::new();
        h.paint(&mut w, 0, 1, 1, 3);
        // Adding a node closes the pending stroke first.
        let mut n = Node::new(w.alloc_node_id(), 0, 4, 4, Icon::Builtin(2));
        w.put_node(n.clone());
        h.record_node(None, Some(n.clone()));
        let before = n.clone();
        n.name = "Keep".into();
        w.put_node(n.clone());
        h.record_node(Some(before), Some(n.clone()));
        h.record_node(Some(n.clone()), Some(n.clone())); // no change: not recorded

        assert!(h.undo(&mut w));
        assert_eq!(w.node(n.id).unwrap().name, "");
        assert!(h.undo(&mut w));
        assert!(w.node(n.id).is_none());
        assert_eq!(w.get(0, 1, 1), 3, "the cell stroke is still there");
        assert!(h.undo(&mut w));
        assert_eq!(w.get(0, 1, 1), EMPTY);
        assert!(!h.undo(&mut w));
        assert!(h.redo(&mut w) && h.redo(&mut w) && h.redo(&mut w));
        assert_eq!(w.node(n.id).unwrap().name, "Keep");
        // Deleting a node is undoable too.
        w.remove_node(n.id);
        h.record_node(Some(n.clone()), None);
        assert!(!h.can_redo());
        h.undo(&mut w);
        assert!(w.node(n.id).is_some());
    }

    #[test]
    fn applied_edits_become_one_undo_step() {
        let mut w = World::new();
        let mut h = History::new();
        let mut edits = Vec::new();
        for x in 0..4 {
            let old = w.set(0, x, 0, 3);
            edits.push(CellEdit { depth: 0, x, y: 0, old, new: 3 });
        }
        h.record_applied(edits);
        assert!(h.can_undo());
        assert!(h.undo(&mut w));
        assert_eq!(w.total_chunks(), 0);
        assert!(h.redo(&mut w));
        assert_eq!(w.get(0, 3, 0), 3);
        h.record_applied(Vec::new());
        assert!(!h.can_redo() || h.can_undo(), "an empty record changes nothing");
    }

    #[test]
    fn new_stroke_discards_redo_and_noops_are_not_recorded() {
        let mut w = World::new();
        let mut h = History::new();
        h.paint(&mut w, 0, 0, 0, 1);
        h.end_stroke();
        h.undo(&mut w);
        assert!(h.can_redo());

        h.paint(&mut w, 0, 0, 0, EMPTY); // already empty: nothing to record
        h.end_stroke();
        assert!(h.can_redo(), "a no-op stroke must not clear redo");

        h.paint(&mut w, 0, 1, 1, 3);
        h.end_stroke();
        assert!(!h.can_redo());
    }
}
