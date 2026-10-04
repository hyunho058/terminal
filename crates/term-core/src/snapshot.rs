//! `GridSnapshot` — the internal contract between `term-core` and `renderer`.
//!
//! The renderer never touches a live [`crate::Grid`]; it consumes an immutable
//! snapshot. This is the versioned boundary type (C4 dependency direction): the
//! renderer depends on `term-core` only for this shape, nothing mutable.

use crate::grid::{Cell, Color, Cursor, Grid};
use crate::image::Placement;
use crate::selection::{Point, Selection};

/// An immutable, cheaply-clonable view of the visible grid at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct GridSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub cursor: Cursor,
    /// Scrollback viewport offset (0 == live) — a side-channel field (SD5).
    pub view_offset: usize,
    /// Whether the viewport is at the live bottom / oldest retained row (D2).
    pub at_bottom: bool,
    pub at_top: bool,
    /// Row-major, length == `cols * rows`. Composed from history + live for the
    /// current `view_offset`.
    cells: Vec<Cell>,
    /// Inline image placements at their live-view cell coords (image-paste SD5).
    /// Populated only at the live bottom; empty while scrolled into history (a
    /// first cut — images render in the live view).
    pub images: Vec<Placement>,
    /// The drag selection as **viewport** cells, or `None` when no part of it is on
    /// screen. The renderer reads this to highlight.
    ///
    /// The grid stores the selection as content points; this is their projection
    /// into the current viewport, made at **every** `view_offset`
    /// (selection-scroll-anchor D2, which reverses selection-model D9's "live bottom
    /// only"): the highlight moves with its text and stays visible in scrollback.
    /// The part above or below the viewport is clipped, so the span here is exactly
    /// the visible part of the selection. Its text is read from the grid, not from
    /// here — [`crate::Terminal::selection_text`] — because the visible snapshot
    /// cannot return a selection that has partly scrolled off-screen.
    pub selection: Option<Selection>,
}

impl GridSnapshot {
    pub fn from_grid(grid: &Grid) -> Self {
        let cols = grid.cols();
        let rows = grid.rows();
        let offset = grid.view_offset();
        let hist = grid.history_len();

        let mut cells = Vec::with_capacity(cols as usize * rows as usize);
        for row in 0..rows {
            // Global row index over [history .. history+rows); offset scrolls up.
            let global = hist as isize - offset as isize + row as isize;
            for col in 0..cols {
                let cell = if global >= 0 && (global as usize) < hist {
                    grid.history_row(global as usize)
                        .and_then(|r| r.get(col as usize).copied())
                        .unwrap_or_else(|| grid.blank_cell())
                } else {
                    let live_row = (global - hist as isize).max(0) as u16;
                    grid.cell(col, live_row)
                        .copied()
                        .unwrap_or_else(|| grid.blank_cell())
                };
                cells.push(cell);
            }
        }

        // Images show in the live view only (empty when scrolled into history).
        let images = if offset == 0 {
            grid.images().placements().to_vec()
        } else {
            Vec::new()
        };

        // The stored content span, projected into this viewport at every offset
        // (selection-scroll-anchor D2) — unlike images, which stay live-only.
        let selection = grid.selection().and_then(|sel| {
            let (start, end) = sel.normalized();
            project_selection(start, end, grid.top_line(), cols, rows)
        });

        GridSnapshot {
            cols,
            rows,
            cursor: grid.cursor(),
            view_offset: offset,
            at_bottom: grid.at_bottom(),
            at_top: grid.at_top(),
            cells,
            images,
            selection,
        }
    }

    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            Some(&self.cells[row as usize * self.cols as usize + col as usize])
        } else {
            None
        }
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Render the snapshot as plain text (blank cells → spaces, trailing blanks
    /// trimmed). Used by snapshot tests and debugging — not a display path.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(self.cells.len() + self.rows as usize);
        for row in 0..self.rows {
            let mut line = String::with_capacity(self.cols as usize);
            for col in 0..self.cols {
                match self.cell(col, row) {
                    // Skip the trailing spacer of a double-width char (the leader
                    // already carries the glyph) so copied text is `한글`, not `한 글`.
                    Some(c) if c.is_continuation() => {}
                    Some(c) => line.push(c.ch),
                    None => line.push(' '),
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Convenience for callers that only want the default colors of empty cells.
    pub fn is_empty_at(&self, col: u16, row: u16) -> bool {
        self.cell(col, row).map(Cell::is_blank).unwrap_or(true)
    }

    /// D8 soft-wrap heuristic for one snapshot row — see [`soft_wraps`], which this
    /// delegates to.
    ///
    /// `pub(crate)` so the sibling [`crate::links`] module can stitch a URL across
    /// a wrap using the **same** rule copy uses (hyperlinks D5) — one heuristic, not
    /// two that could drift. A row past the bottom has no cells and never wraps.
    pub(crate) fn row_soft_wraps(&self, row: u16) -> bool {
        let cols = self.cols as usize;
        let start = row as usize * cols;
        let cells = self.cells.get(start..start + cols).unwrap_or(&[]);
        soft_wraps(cells, self.cols)
    }
}

/// The D8 soft-wrap rule, over one row's cells read at width `cols`: the row is
/// treated as soft-wrapped into the next iff its last column (`cols - 1`) holds a
/// non-blank, non-continuation cell.
///
/// A free function over a row slice (selection-scroll-anchor D6) so the grid's
/// extractor, which reads history rows that are not in any snapshot, and
/// [`GridSnapshot::row_soft_wraps`], which hyperlinks use, apply **one** rule. A
/// row shorter than `cols` (a history row captured narrower) has blank padding at
/// the last column, so it does not wrap — matching what the snapshot shows. Known
/// limitations, accepted by selection-model D8: a full-width hard line reads as
/// wrapped, and a CJK margin-wrap that leaves the last column blank reads as hard.
pub(crate) fn soft_wraps(row: &[Cell], cols: u16) -> bool {
    let Some(last) = usize::from(cols).checked_sub(1) else {
        return false;
    };
    row.get(last)
        .is_some_and(|c| !c.is_blank() && !c.is_continuation())
}

/// Project a stored content span into a `cols × rows` viewport whose row 0 shows
/// content line `top` (selection-scroll-anchor D2). `start <= end` in reading order.
///
/// Line `L` sits on viewport row `L - top`, computed **signed** so a line above or
/// below the viewport cannot underflow. A start above the viewport clips to
/// `(0, 0)` and an end below it clips to `(cols - 1, rows - 1)`, which keeps the
/// linear-span rule: the clipped span is exactly the visible part of the stream.
/// `None` iff the span ends above the viewport or starts below it; otherwise at
/// least one cell is visible.
pub(crate) fn project_selection(
    start: Point,
    end: Point,
    top: u64,
    cols: u16,
    rows: u16,
) -> Option<Selection> {
    let last_row = i128::from(rows) - 1;
    let start_row = i128::from(start.line) - i128::from(top);
    let end_row = i128::from(end.line) - i128::from(top);
    if end_row < 0 || start_row > last_row {
        return None;
    }
    // Each `as u16` below is reached only for a row in `0..=last_row`, which fits
    // because `rows` is a `u16`.
    let anchor = if start_row < 0 {
        (0, 0) // starts above the viewport
    } else {
        (start.col, start_row as u16)
    };
    let focus = if end_row > last_row {
        (cols - 1, rows - 1) // ends below the viewport
    } else {
        (end.col, end_row as u16)
    };
    Some(Selection { anchor, focus })
}

/// Marker so downstream code can name the color type without reaching into `grid`.
pub type SnapshotColor = Color;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CellAttrs;

    fn pt(line: u64, col: u16) -> Point {
        Point { line, col }
    }

    fn sel(anchor: (u16, u16), focus: (u16, u16)) -> Option<Selection> {
        Some(Selection { anchor, focus })
    }

    fn cell(ch: char) -> Cell {
        Cell {
            ch,
            fg: Color(0),
            bg: Color(0),
            attrs: CellAttrs::NONE,
        }
    }

    // --- project_selection (selection-scroll-anchor D2) ---

    #[test]
    fn a_span_inside_the_viewport_projects_unclipped() {
        // D2 — line L sits on viewport row L - top, at its own column.
        assert_eq!(
            project_selection(pt(101, 2), pt(102, 7), 100, 10, 3),
            sel((2, 1), (7, 2))
        );
        // Row 0 at a non-zero column is not mistaken for "above the viewport".
        assert_eq!(
            project_selection(pt(100, 3), pt(100, 5), 100, 10, 3),
            sel((3, 0), (5, 0))
        );
    }

    #[test]
    fn a_start_above_clips_to_the_origin() {
        // selection-scroll-anchor R2.4 — (top − 2, 5) .. (top + 1, 3) → (0,0)–(3,1).
        assert_eq!(
            project_selection(pt(98, 5), pt(101, 3), 100, 10, 3),
            sel((0, 0), (3, 1))
        );
    }

    #[test]
    fn an_end_below_clips_to_the_last_cell() {
        // selection-scroll-anchor R2.5 — start on row 1 col 4, end below row 2.
        assert_eq!(
            project_selection(pt(101, 4), pt(105, 0), 100, 10, 3),
            sel((4, 1), (9, 2))
        );
        // Ending exactly on the last row is not clipped.
        assert_eq!(
            project_selection(pt(101, 4), pt(102, 6), 100, 10, 3),
            sel((4, 1), (6, 2))
        );
    }

    #[test]
    fn a_span_wholly_outside_projects_to_none_and_an_edge_touch_does_not() {
        // selection-scroll-anchor R2.6.
        assert_eq!(project_selection(pt(90, 0), pt(99, 9), 100, 10, 3), None);
        assert_eq!(project_selection(pt(103, 0), pt(110, 0), 100, 10, 3), None);
        assert_eq!(
            project_selection(pt(99, 4), pt(100, 0), 100, 10, 3),
            sel((0, 0), (0, 0)),
            "ending at (top, 0) is one visible cell"
        );
        assert_eq!(
            project_selection(pt(102, 8), pt(102, 9), 100, 10, 3),
            sel((8, 2), (9, 2)),
            "starting on the last row is visible"
        );
    }

    #[test]
    fn projection_never_underflows_at_the_extremes() {
        // D2 — the row arithmetic is signed: a line far above or below `top`, or a
        // `top` near u64::MAX, must neither panic nor wrap into the viewport.
        assert_eq!(project_selection(pt(0, 0), pt(1, 0), u64::MAX, 10, 3), None);
        assert_eq!(
            project_selection(pt(u64::MAX - 1, 0), pt(u64::MAX, 0), 0, 10, 3),
            None
        );
        assert_eq!(
            project_selection(pt(0, 4), pt(u64::MAX, 2), 5, 10, 3),
            sel((0, 0), (9, 2)),
            "covers the whole viewport from far above to far below"
        );
        // `top + rows - 1` would overflow in u64 here.
        assert_eq!(
            project_selection(pt(u64::MAX, 1), pt(u64::MAX, 3), u64::MAX - 1, 10, 3),
            sel((1, 1), (3, 1))
        );
    }

    // --- soft_wraps (the D8 rule as a free function, selection-scroll-anchor D6) ---

    #[test]
    fn soft_wraps_reads_the_last_column_at_the_given_width() {
        let row: Vec<Cell> = "abcde".chars().map(cell).collect();
        assert!(soft_wraps(&row, 5), "a filled last column wraps");
        assert!(!soft_wraps(&row, 6), "past the row's end is blank padding");
        assert!(
            soft_wraps(&row, 3),
            "a wider row is read at the current width"
        );
        assert!(!soft_wraps(&row, 0), "a zero width never wraps");

        let blank_end: Vec<Cell> = "ab ".chars().map(cell).collect();
        assert!(
            !soft_wraps(&blank_end, 3),
            "a blank last column is a hard break"
        );

        let mut wide_end: Vec<Cell> = "ab".chars().map(cell).collect();
        wide_end.push(cell(Cell::CONTINUATION));
        assert!(
            !soft_wraps(&wide_end, 3),
            "a continuation in the last column is not a wrap"
        );
    }

    #[test]
    fn row_soft_wraps_delegates_per_row() {
        // The snapshot method reads row `r`'s own cells, not row 0's.
        let mut t = crate::Terminal::new(5, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"ab\r\nfghij\r\nk");
        let snap = t.snapshot();
        assert!(!snap.row_soft_wraps(0), "row 0 ends blank");
        assert!(snap.row_soft_wraps(1), "row 1 fills its last column");
        assert!(!snap.row_soft_wraps(2));
        assert!(!snap.row_soft_wraps(3), "a row past the bottom never wraps");
    }
}
