//! The selection model — the anchor/focus range a user drags out, in two
//! coordinate systems, plus the viewport span's membership test.
//!
//! GPU-free and window-free (dependency-direction C4). There are two types, and
//! the split is the contract (selection-scroll-anchor D2):
//!
//! - [`ContentSelection`] is what the grid **stores**: two content [`Point`]s, each
//!   an absolute line counted through scrollback plus a column. A content point
//!   names the same text for as long as that text is retained, so the selection
//!   follows its text through terminal-owned scrolling. The app builds points with
//!   [`crate::Terminal::point_at`] and reads the text with
//!   [`crate::Terminal::selection_text`].
//! - [`Selection`] is what the renderer **reads**: **viewport** cells (`(col, row)`
//!   within the visible `cols × rows` grid), produced on
//!   [`crate::GridSnapshot::selection`] by projecting the stored span into the
//!   current viewport. Its shape is unchanged from before content anchoring, so the
//!   renderer never learns about scrollback arithmetic.
//!
//! Either way the selected region is a **linear / stream** span (start-of-selection
//! to end, wrapping across whole rows in between) — not a rectangular block.

/// A position in the content stream (selection-scroll-anchor D1): an absolute
/// `line` counted through scrollback, plus a `col`.
///
/// History row `i` is line `evicted + i`, and live row `r` is line
/// `evicted + history_len + r`, where `evicted` counts rows that have aged out of
/// the scrollback cap. A full-screen primary scroll moves live row 0 into history
/// at the **same** line number, and an eviction raises `evicted` exactly as much as
/// it lowers every history index — so a line number stays on its text with no
/// per-push bookkeeping. Rows that move *without* entering history (a DECSTBM
/// band, the alt screen) keep their positional number on purpose: that motion is
/// app-owned.
///
/// Ordered by `(line, col)` — reading order. The derive compares fields in
/// declaration order, which is why `line` is declared first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Point {
    pub line: u64,
    pub col: u16,
}

/// The stored drag selection (selection-scroll-anchor D2): `anchor` is where the
/// drag began, `focus` its current end, both content [`Point`]s. Either may come
/// first in reading order — a backward drag is stored as dragged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContentSelection {
    pub anchor: Point,
    pub focus: Point,
}

impl ContentSelection {
    /// `(start, end)` in reading order — by `(line, col)` — so `start` never comes
    /// after `end`. A backward drag normalises to the same span as a forward one.
    pub fn normalized(&self) -> (Point, Point) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }
}

/// A **viewport** selection, as the renderer reads it on
/// [`crate::GridSnapshot::selection`]: `anchor`/`focus` are viewport cell
/// coordinates `(col, row)`.
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

    fn pt(line: u64, col: u16) -> Point {
        Point { line, col }
    }

    #[test]
    fn points_order_by_line_then_col() {
        // selection-scroll-anchor D1/D2 — reading order: any later line beats any
        // column, and the column only breaks a tie on the same line. A field-order
        // swap in the struct would flip the first assertion.
        assert!(pt(1, 9) < pt(2, 0), "the line dominates the column");
        assert!(pt(2, 3) < pt(2, 4), "same line → the column decides");
        assert_eq!(pt(2, 3).cmp(&pt(2, 3)), std::cmp::Ordering::Equal);
    }

    #[test]
    fn content_selection_normalizes_a_backward_drag() {
        // selection-scroll-anchor D2 — a backward drag (focus before anchor) and a
        // forward one normalise to the same (start, end).
        let backward = ContentSelection {
            anchor: pt(7, 4),
            focus: pt(5, 1),
        };
        let forward = ContentSelection {
            anchor: pt(5, 1),
            focus: pt(7, 4),
        };
        assert_eq!(backward.normalized(), (pt(5, 1), pt(7, 4)));
        assert_eq!(forward.normalized(), (pt(5, 1), pt(7, 4)));
        // Same line: the column orders them.
        let same_line = ContentSelection {
            anchor: pt(3, 8),
            focus: pt(3, 2),
        };
        assert_eq!(same_line.normalized(), (pt(3, 2), pt(3, 8)));
    }

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
