//! `GridSnapshot` — the internal contract between `term-core` and `renderer`.
//!
//! The renderer never touches a live [`crate::Grid`]; it consumes an immutable
//! snapshot. This is the versioned boundary type (C4 dependency direction): the
//! renderer depends on `term-core` only for this shape, nothing mutable.

use crate::grid::{Cell, Color, Cursor, Grid};
use crate::image::Placement;
use crate::selection::Selection;

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
    /// The live drag selection (viewport cell range), or `None`. Populated only at
    /// the live bottom (`view_offset == 0`) — like `images`, a selection is not
    /// shown while scrolled into history. The renderer reads this to highlight;
    /// [`selection_text`](GridSnapshot::selection_text) extracts its text.
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

        // A selection is a live-viewport concept: shown only at the live bottom,
        // absent while scrolled into history (D9 — same policy as images).
        let selection = if offset == 0 { grid.selection() } else { None };

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

    /// Extract the selected cell range as text (`None` when there is no selection).
    ///
    /// Walks the linear span (start row from `start.col`; interior rows full-width;
    /// end row up to `end.col`), skipping double-width continuation cells and
    /// `trim_end`-ing each row segment — the same shape as [`to_text`]. Rows are
    /// joined with `\n`, **except** a soft-wrapped row joins the next with no
    /// separator: a row is treated as soft-wrapped iff its last column (`cols-1`)
    /// holds a non-blank, non-continuation cell (the D8 heuristic; its known
    /// limitations — a full-width hard line, and a CJK margin-wrap that leaves the
    /// last column blank — are accepted). Never panics on inverted/edge coords.
    ///
    /// [`to_text`]: GridSnapshot::to_text
    pub fn selection_text(&self) -> Option<String> {
        let sel = self.selection?;
        if self.cols == 0 || self.rows == 0 {
            return Some(String::new());
        }
        let (start, end) = sel.normalized(); // ((col, row), (col, row))
        let last_row = self.rows - 1;
        let last_col = self.cols - 1;
        let s_row = start.1.min(last_row);
        let e_row = end.1.min(last_row);

        let mut out = String::new();
        for row in s_row..=e_row {
            let first = if row == s_row { start.0 } else { 0 }.min(last_col);
            let last = if row == e_row { end.0 } else { last_col }.min(last_col);
            // `first > last` only when out-of-range rows collapse onto one grid row
            // (column ordering, valid per-row, no longer holds) — `first..=last` is
            // then an empty range; guard the capacity math against underflow (R3.6).
            let mut line = String::with_capacity((last.saturating_sub(first) + 1) as usize);
            for col in first..=last {
                match self.cell(col, row) {
                    // The wide leader already carries the glyph — drop its spacer.
                    Some(c) if c.is_continuation() => {}
                    Some(c) => line.push(c.ch),
                    None => line.push(' '),
                }
            }
            out.push_str(line.trim_end());
            // Join to the next selected row: newline unless this row soft-wraps.
            if row < e_row && !self.row_soft_wraps(row) {
                out.push('\n');
            }
        }
        Some(out)
    }

    /// D8 soft-wrap heuristic: a row is treated as soft-wrapped into the next iff
    /// its last column holds a non-blank, non-continuation cell.
    fn row_soft_wraps(&self, row: u16) -> bool {
        if self.cols == 0 {
            return false;
        }
        match self.cell(self.cols - 1, row) {
            Some(c) => !c.is_blank() && !c.is_continuation(),
            None => false,
        }
    }
}

/// Marker so downstream code can name the color type without reaching into `grid`.
pub type SnapshotColor = Color;
