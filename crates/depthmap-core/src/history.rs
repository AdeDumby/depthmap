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

const MAX_STROKES: usize = 2000;

/// Stroke-grouped undo/redo log.
#[derive(Default)]
pub struct History {
    undo: Vec<Vec<CellEdit>>,
    redo: Vec<Vec<CellEdit>>,
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
        self.undo.push(std::mem::take(&mut self.current));
        if self.undo.len() > MAX_STROKES {
            self.undo.remove(0);
        }
    }

    pub fn undo(&mut self, world: &mut World) -> bool {
        self.end_stroke();
        let Some(stroke) = self.undo.pop() else {
            return false;
        };
        for e in stroke.iter().rev() {
            world.set(e.depth, e.x, e.y, e.old);
        }
        self.redo.push(stroke);
        true
    }

    pub fn redo(&mut self, world: &mut World) -> bool {
        let Some(stroke) = self.redo.pop() else {
            return false;
        };
        for e in &stroke {
            world.set(e.depth, e.x, e.y, e.new);
        }
        self.undo.push(stroke);
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
