//! The cell grid: the mutable model that VT parsing writes into.
//!
//! Row-major `cols * rows` cells plus a cursor. This is the hand-rolled "manual
//! cell control" surface the project exists to own — deliberately GPU-free so it
//! can be unit-tested headlessly.

/// A packed `0xRRGGBB` color. UI-agnostic: the renderer converts to whatever the
/// GPU needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// Normalized sRGB components in `0.0..=1.0` (no gamma conversion).
    pub fn to_srgb_f32(self) -> [f32; 3] {
        [
            self.r() as f32 / 255.0,
            self.g() as f32 / 255.0,
            self.b() as f32 / 255.0,
        ]
    }

    /// Linear-light components for a linear GPU pipeline.
    pub fn to_linear_f32(self) -> [f32; 3] {
        fn c(u: f32) -> f32 {
            if u <= 0.04045 {
                u / 12.92
            } else {
                ((u + 0.055) / 1.055).powf(2.4)
            }
        }
        let [r, g, b] = self.to_srgb_f32();
        [c(r), c(g), c(b)]
    }
}

/// One terminal cell: a glyph plus foreground/background color.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
}

impl Cell {
    pub fn blank(fg: Color, bg: Color) -> Self {
        Cell { ch: ' ', fg, bg }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' '
    }
}

/// Cursor position in cell coordinates (0-based).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
}

/// The mutable cell grid + cursor. Mutation methods are the vocabulary the VT
/// parser dispatches into (see `parser.rs`).
#[derive(Clone, Debug)]
pub struct Grid {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    cursor: Cursor,
    default_fg: Color,
    default_bg: Color,
    /// Active pen for newly printed cells (set by SGR).
    pen_fg: Color,
    pen_bg: Color,
}

impl Grid {
    pub fn new(cols: u16, rows: u16, fg: Color, bg: Color) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Grid {
            cols,
            rows,
            cells: vec![Cell::blank(fg, bg); cols as usize * rows as usize],
            cursor: Cursor::default(),
            default_fg: fg,
            default_bg: bg,
            pen_fg: fg,
            pen_bg: bg,
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    fn index(&self, col: u16, row: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    /// Read a cell, or `None` if out of bounds.
    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            Some(&self.cells[self.index(col, row)])
        } else {
            None
        }
    }

    /// Set the active pen used for subsequently printed cells.
    pub fn set_pen(&mut self, fg: Option<Color>, bg: Option<Color>) {
        if let Some(fg) = fg {
            self.pen_fg = fg;
        }
        if let Some(bg) = bg {
            self.pen_bg = bg;
        }
    }

    /// Reset the pen to the configured defaults (SGR 0).
    pub fn reset_pen(&mut self) {
        self.pen_fg = self.default_fg;
        self.pen_bg = self.default_bg;
    }

    /// Print a character at the cursor, advancing (and wrapping) it.
    pub fn print(&mut self, ch: char) {
        if self.cursor.col >= self.cols {
            self.line_feed();
            self.cursor.col = 0;
        }
        let idx = self.index(self.cursor.col, self.cursor.row);
        self.cells[idx] = Cell {
            ch,
            fg: self.pen_fg,
            bg: self.pen_bg,
        };
        self.cursor.col += 1;
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
    }

    /// Move down one row, scrolling the viewport if at the bottom.
    pub fn line_feed(&mut self) {
        if self.cursor.row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cursor.row += 1;
        }
    }

    pub fn backspace(&mut self) {
        self.cursor.col = self.cursor.col.saturating_sub(1);
    }

    /// Advance to the next multiple-of-8 tab stop.
    pub fn tab(&mut self) {
        let next = (self.cursor.col / 8 + 1) * 8;
        self.cursor.col = next.min(self.cols - 1);
    }

    /// Move the cursor to an absolute (1-based in VT terms) position, clamped.
    pub fn move_to(&mut self, col: u16, row: u16) {
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.row = row.min(self.rows - 1);
    }

    /// Scroll the whole viewport up one line, blanking the new bottom row.
    fn scroll_up(&mut self) {
        let row_len = self.cols as usize;
        self.cells.copy_within(row_len.., 0);
        let start = (self.rows as usize - 1) * row_len;
        for cell in &mut self.cells[start..] {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
    }

    /// Clear the entire grid and home the cursor (CSI 2J-ish).
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
        self.cursor = Cursor::default();
    }

    /// Erase from the cursor to end of the current line (CSI K).
    pub fn clear_line_from_cursor(&mut self) {
        let start = self.index(self.cursor.col, self.cursor.row);
        let end = self.index(0, self.cursor.row) + self.cols as usize;
        for cell in &mut self.cells[start..end] {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
    }

    /// Resize the grid, preserving overlapping content, and clamp the cursor.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut next =
            vec![Cell::blank(self.default_fg, self.default_bg); cols as usize * rows as usize];
        for row in 0..rows.min(self.rows) {
            for col in 0..cols.min(self.cols) {
                next[row as usize * cols as usize + col as usize] =
                    self.cells[self.index(col, row)];
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.cells = next;
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.row = self.cursor.row.min(rows - 1);
    }

    /// Iterate cells in row-major order. Used to build a [`crate::GridSnapshot`].
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }
}
