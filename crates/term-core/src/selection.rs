//! The selection model — the anchor/focus cell range a user drags out, plus its
//! linear-span membership test.
//!
//! GPU-free and window-free (dependency-direction C4): the renderer reads the
//! selection through [`crate::GridSnapshot`], and the app extracts its text via
//! [`crate::GridSnapshot::selection_text`]. Coordinates are **live-viewport** cells
//! (`(col, row)` within the visible `cols × rows` grid). The selected region is a
//! **linear / stream** span (start-of-selection to end, wrapping across whole rows
//! in between) — not a rectangular block.

/// A drag selection: `anchor` is where the drag began, `focus` the current end.
/// Both are viewport cell coordinates `(col, row)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub focus: (u16, u16),
}

impl Selection {
    /// A fresh selection anchored (and focused) at one cell.
    pub fn new(anchor: (u16, u16)) -> Self {
        Selection {
            anchor,
            focus: anchor,
        }
    }

    /// `(start, end)` ordered in reading order — by `(row, col)` — so `start`
    /// never comes after `end`. Returns the two `(col, row)` endpoints.
    pub fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        // Compare on (row, col) so rows dominate columns.
        let key = |p: (u16, u16)| (p.1, p.0);
        if key(self.anchor) <= key(self.focus) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// Whether cell `(col, row)` is inside the linear span on a `cols`-wide grid:
    /// from `start.col` on the start row, every full interior row, and up to and
    /// including `end.col` on the end row. A single-cell selection matches exactly
    /// its one cell. Never panics on inverted or edge coordinates.
    pub fn contains(&self, col: u16, row: u16, cols: u16) -> bool {
        let (start, end) = self.normalized();
        // Linearize each cell to a scalar index so the span is one range test.
        let cols = cols.max(1) as u64;
        let idx = row as u64 * cols + col as u64;
        let s = start.1 as u64 * cols + start.0 as u64;
        let e = end.1 as u64 * cols + end.0 as u64;
        idx >= s && idx <= e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_orders_anchor_and_focus_in_reading_order() {
        // R1.1: focus before anchor → normalized swaps them.
        let sel = Selection {
            anchor: (5, 2),
            focus: (1, 0),
        };
        assert_eq!(sel.normalized(), ((1, 0), (5, 2)));
        // Already ordered → unchanged.
        let sel2 = Selection {
            anchor: (1, 0),
            focus: (5, 2),
        };
        assert_eq!(sel2.normalized(), ((1, 0), (5, 2)));
    }

    #[test]
    fn contains_is_a_linear_span_across_rows() {
        // R1.2: normalized start (3,0) end (2,2) on a 10-col grid.
        let sel = Selection {
            anchor: (3, 0),
            focus: (2, 2),
        };
        for &(c, r) in &[(3, 0), (9, 0), (0, 1), (9, 1), (0, 2), (2, 2)] {
            assert!(sel.contains(c, r, 10), "({c},{r}) should be selected");
        }
        assert!(!sel.contains(2, 0, 10), "(2,0) is before the start");
        assert!(!sel.contains(3, 2, 10), "(3,2) is after the end");
    }

    #[test]
    fn single_cell_selection_selects_exactly_one_cell() {
        // R1.3.
        let sel = Selection::new((4, 1));
        assert!(sel.contains(4, 1, 10));
        assert!(!sel.contains(3, 1, 10));
        assert!(!sel.contains(5, 1, 10));
        assert!(!sel.contains(4, 0, 10));
    }

    #[test]
    fn contains_never_panics_on_inverted_or_edge_coords() {
        // R3.6 (membership half): inverted anchor/focus + edge query.
        let sel = Selection {
            anchor: (9, 4),
            focus: (0, 0),
        };
        assert!(sel.contains(5, 2, 10));
        assert!(sel.contains(0, 0, 10));
        assert!(sel.contains(9, 4, 10));
    }
}
