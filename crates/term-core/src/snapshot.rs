//! `GridSnapshot` — the internal contract between `term-core` and `renderer`.
//!
//! The renderer never touches a live [`crate::Grid`]; it consumes an immutable
//! snapshot. This is the versioned boundary type (C4 dependency direction): the
//! renderer depends on `term-core` only for this shape, nothing mutable.

use crate::grid::{Cell, Color, Cursor, Grid};

/// An immutable, cheaply-clonable view of the visible grid at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct GridSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub cursor: Cursor,
    /// Row-major, length == `cols * rows`.
    cells: Vec<Cell>,
}

impl GridSnapshot {
    pub fn from_grid(grid: &Grid) -> Self {
        GridSnapshot {
            cols: grid.cols(),
            rows: grid.rows(),
            cursor: grid.cursor(),
            cells: grid.cells().to_vec(),
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
                line.push(self.cell(col, row).map(|c| c.ch).unwrap_or(' '));
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
}

/// Marker so downstream code can name the color type without reaching into `grid`.
pub type SnapshotColor = Color;
