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

        GridSnapshot {
            cols,
            rows,
            cursor: grid.cursor(),
            view_offset: offset,
            at_bottom: grid.at_bottom(),
            at_top: grid.at_top(),
            cells,
            images,
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

    /// The text inside `sel` in reading order, for Cmd+C copy. Mirrors [`to_text`]:
    /// double-width continuation cells are skipped (so `한글`, not `한 글`) and each
    /// row is `trim_end`ed; rows join with `\n` and there is no trailing newline.
    /// Returns an empty string when the selection covers nothing.
    ///
    /// [`to_text`]: GridSnapshot::to_text
    pub fn selection_text(&self, sel: &Selection) -> String {
        let (start, end) = sel.ordered();
        let mut lines: Vec<String> = Vec::new();
        for row in start.1..=end.1 {
            let Some((c0, c1)) = sel.row_span(row, self.cols) else {
                continue;
            };
            let mut line = String::new();
            for col in c0..=c1 {
                match self.cell(col, row) {
                    // Skip the trailing spacer of a double-width char.
                    Some(c) if c.is_continuation() => {}
                    Some(c) => line.push(c.ch),
                    None => line.push(' '),
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }
}

/// Marker so downstream code can name the color type without reaching into `grid`.
pub type SnapshotColor = Color;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CellAttrs;

    /// Build a snapshot from rows of text (padded to `cols` with spaces).
    fn snap(rows: &[&str], cols: u16) -> GridSnapshot {
        let mut cells = Vec::new();
        for r in rows {
            let mut chars: Vec<char> = r.chars().collect();
            chars.resize(cols as usize, ' ');
            for ch in chars {
                cells.push(Cell {
                    ch,
                    fg: Color(0xffffff),
                    bg: Color(0),
                    attrs: CellAttrs::NONE,
                });
            }
        }
        GridSnapshot {
            cols,
            rows: rows.len() as u16,
            cursor: Cursor { col: 0, row: 0 },
            view_offset: 0,
            at_bottom: true,
            at_top: true,
            cells,
            images: Vec::new(),
        }
    }

    #[test]
    fn selection_text_single_row() {
        let s = snap(&["hello world"], 11);
        // cols 0..=4 on row 0 → "hello"
        let sel = Selection {
            anchor: (0, 0),
            head: (4, 0),
        };
        assert_eq!(s.selection_text(&sel), "hello");
    }

    #[test]
    fn selection_text_multi_row_stream_trims_each_line() {
        let s = snap(&["abc", "de", "fghi"], 4);
        // Stream from (1,0) to (2,2):
        //   row0 cols[1,3] "bc " → "bc"; row1 cols[0,3] "de  " → "de";
        //   row2 cols[0,2] "fgh". Joined with '\n', no trailing newline.
        let sel = Selection {
            anchor: (1, 0),
            head: (2, 2),
        };
        assert_eq!(s.selection_text(&sel), "bc\nde\nfgh");
    }

    #[test]
    fn selection_text_skips_wide_char_continuation() {
        // A double-width leader '한' followed by its continuation spacer, then 'x'.
        let mut s = snap(&["    "], 4);
        s.cells[0].ch = '한';
        s.cells[1].ch = Cell::CONTINUATION;
        s.cells[2].ch = 'x';
        let sel = Selection {
            anchor: (0, 0),
            head: (2, 0),
        };
        // The continuation cell contributes nothing → "한x", not "한\0x".
        assert_eq!(s.selection_text(&sel), "한x");
    }
}
