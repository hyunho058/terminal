//! Text selection over a grid viewport — a headless (GPU/window-free) value type.
//!
//! A [`Selection`] is two cell coordinates: the `anchor` (where a drag began) and
//! the `head` (where it currently is). It owns the *geometry* of a reading-order
//! ("stream") selection — which columns of each row fall inside — so both the
//! renderer's highlight layer and [`crate::GridSnapshot::selection_text`] share one
//! source of truth. Mouse handling and rendering live in `app`/`renderer`; keeping
//! this in `term-core` (dependency-direction rule) makes it unit-testable without a
//! window.

/// A stream (reading-order) text selection in **(col, row)** cell coordinates over
/// the visible viewport. `anchor` and `head` may be in either order; [`ordered`]
/// normalises them.
///
/// [`ordered`]: Selection::ordered
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    /// Where the drag started (col, row).
    pub anchor: (u16, u16),
    /// Where the drag currently is (col, row).
    pub head: (u16, u16),
}

impl Selection {
    /// A zero-length selection at a single cell.
    pub fn at(col: u16, row: u16) -> Self {
        Selection {
            anchor: (col, row),
            head: (col, row),
        }
    }

    /// `(start, end)` normalised to reading order: `start` is the earlier cell in
    /// row-major order (row first, then column), `end` the later. Both inclusive.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let a = self.anchor;
        let h = self.head;
        // Compare by (row, col) so a lower row always sorts first.
        if (a.1, a.0) <= (h.1, h.0) {
            (a, h)
        } else {
            (h, a)
        }
    }

    /// True when the selection covers a single cell (anchor == head) — a click, not
    /// a drag. The app treats these as "no selection".
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The inclusive column range `[c0, c1]` selected in `row`, or `None` if `row`
    /// lies outside the selection. Stream semantics: the first row runs from the
    /// start column to the right edge, interior rows span the full width, and the
    /// last row runs from the left edge to the end column. `cols` is the grid width.
    pub fn row_span(&self, row: u16, cols: u16) -> Option<(u16, u16)> {
        if cols == 0 {
            return None;
        }
        let (start, end) = self.ordered();
        let (sc, sr) = start;
        let (ec, er) = end;
        if row < sr || row > er {
            return None;
        }
        let last = cols - 1;
        let c0 = if row == sr { sc } else { 0 };
        let c1 = if row == er { ec } else { last };
        // Clamp defensively: a head dragged past the right edge can exceed `last`.
        let c0 = c0.min(last);
        let c1 = c1.min(last);
        if c0 > c1 {
            None
        } else {
            Some((c0, c1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_normalises_either_direction() {
        // Forward drag (anchor before head) is already ordered.
        let fwd = Selection {
            anchor: (2, 1),
            head: (5, 3),
        };
        assert_eq!(fwd.ordered(), ((2, 1), (5, 3)));
        // Backward drag (head before anchor) swaps.
        let back = Selection {
            anchor: (5, 3),
            head: (2, 1),
        };
        assert_eq!(back.ordered(), ((2, 1), (5, 3)));
        // Same row: column decides the order.
        let same_row = Selection {
            anchor: (7, 2),
            head: (3, 2),
        };
        assert_eq!(same_row.ordered(), ((3, 2), (7, 2)));
    }

    #[test]
    fn row_span_stream_shape() {
        // A 3-row selection from (2,1) to (5,3) over an 10-col grid.
        let sel = Selection {
            anchor: (2, 1),
            head: (5, 3),
        };
        assert_eq!(sel.row_span(0, 10), None, "above the selection");
        assert_eq!(
            sel.row_span(1, 10),
            Some((2, 9)),
            "first row: start col → right edge"
        );
        assert_eq!(
            sel.row_span(2, 10),
            Some((0, 9)),
            "interior row: full width"
        );
        assert_eq!(
            sel.row_span(3, 10),
            Some((0, 5)),
            "last row: left edge → end col"
        );
        assert_eq!(sel.row_span(4, 10), None, "below the selection");
    }

    #[test]
    fn row_span_single_row() {
        // Within one row the span is exactly [min_col, max_col].
        let sel = Selection {
            anchor: (6, 2),
            head: (3, 2),
        };
        assert_eq!(sel.row_span(2, 80), Some((3, 6)));
        assert_eq!(sel.row_span(1, 80), None);
    }

    #[test]
    fn row_span_clamps_head_past_right_edge() {
        // A head dragged past the last column clamps to the grid width.
        let sel = Selection {
            anchor: (0, 0),
            head: (999, 0),
        };
        assert_eq!(sel.row_span(0, 10), Some((0, 9)));
    }

    #[test]
    fn empty_selection_is_a_click() {
        assert!(Selection::at(4, 4).is_empty());
        assert!(!Selection {
            anchor: (4, 4),
            head: (5, 4),
        }
        .is_empty());
    }
}
