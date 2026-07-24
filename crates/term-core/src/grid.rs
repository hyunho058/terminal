//! The cell grid: the mutable model that VT parsing writes into.
//!
//! Row-major `cols * rows` cells plus a cursor. This is the hand-rolled "manual
//! cell control" surface the project exists to own — deliberately GPU-free so it
//! can be unit-tested headlessly.

use unicode_width::UnicodeWidthChar;

use crate::image::{Decoded, ImageLayer};
use crate::scrollback::{History, DEFAULT_HISTORY_LIMIT};
use crate::selection::Selection;

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

/// Packed text attributes set by SGR (bold/dim/italic/underline/reverse). A `u8`
/// bitset kept **inside** [`Cell`] with a derived `PartialEq` so an attribute-only
/// change still makes two [`GridSnapshot`]s compare unequal — the equality basis
/// the renderer's idle-CPU cache depends on (see `specs/idle-cpu` SD2).
///
/// [`GridSnapshot`]: crate::GridSnapshot
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CellAttrs(pub u8);

impl CellAttrs {
    pub const NONE: CellAttrs = CellAttrs(0);
    pub const BOLD: CellAttrs = CellAttrs(1 << 0);
    pub const DIM: CellAttrs = CellAttrs(1 << 1);
    pub const ITALIC: CellAttrs = CellAttrs(1 << 2);
    pub const UNDERLINE: CellAttrs = CellAttrs(1 << 3);
    pub const REVERSE: CellAttrs = CellAttrs(1 << 4);

    /// True if every bit in `other` is set.
    pub const fn contains(self, other: CellAttrs) -> bool {
        self.0 & other.0 == other.0
    }
    /// Set the bits in `other`.
    pub fn insert(&mut self, other: CellAttrs) {
        self.0 |= other.0;
    }
    /// Clear the bits in `other`.
    pub fn remove(&mut self, other: CellAttrs) {
        self.0 &= !other.0;
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// One terminal cell: a glyph, fg/bg color, and text attributes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

impl Cell {
    /// Sentinel char for the trailing cell of a double-width character (a leader
    /// occupies this + the next cell). Never produced by `print` of a real char —
    /// `vte` routes NUL to `execute`, not `print`.
    pub const CONTINUATION: char = '\0';

    pub fn blank(fg: Color, bg: Color) -> Self {
        Cell {
            ch: ' ',
            fg,
            bg,
            attrs: CellAttrs::NONE,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' '
    }

    /// True if this is the trailing spacer cell of a double-width character. The
    /// renderer draws no glyph for it (the wide leader's glyph already spans both
    /// cells) and `to_text` skips it.
    pub fn is_continuation(&self) -> bool {
        self.ch == Cell::CONTINUATION
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
    /// Active text attributes for newly printed cells (set by SGR 1/2/3/4/7).
    pen_attrs: CellAttrs,
    /// Retained scrolled-off primary-screen rows (D1).
    history: History,
    /// Rows scrolled up from the live view (0 == bottom/live) (D2).
    view_offset: usize,
    /// True while an alternate screen (tmux/vim, `?1049h`) is active (D5).
    alt_screen: bool,
    /// DECCKM — application cursor key mode (`?1h`/`?1l`). Read by the app to pick
    /// the SS3 (`ESC O A`) over the CSI (`ESC [ A`) arrow encoding
    /// (arrow-key-input D10). Terminal *mode* state, like `alt_screen`; it lives
    /// here for the same reason and is deliberately absent from `GridSnapshot`
    /// (the renderer never reads it — D2).
    app_cursor_keys: bool,
    /// Bracketed-paste mode (DECSET/DECRST `?2004`). A keyboard-input mode like
    /// `app_cursor_keys`: read by the app via `Terminal::bracketed_paste` to wrap a
    /// paste in `ESC[200~`/`ESC[201~`, absent from `GridSnapshot` (the renderer
    /// never reads it), cleared by RIS, and surviving resize/alt-screen.
    bracketed_paste: bool,
    /// The live drag selection (anchor/focus viewport cells), or `None`. Carried to
    /// the renderer on `GridSnapshot.selection` and extracted to text via
    /// `GridSnapshot::selection_text`. Cleared on `resize` (coords are dimension-
    /// bound); when to clear it on new output/scroll is the app's policy.
    selection: Option<Selection>,
    /// Inline-image placements anchored to cells (image-paste D2, SD5).
    images: ImageLayer,
    /// Most-recent OSC 133 command-end event parsed from the shell's output
    /// (shell-integration D10/D21). Transient, one-shot: the app drains it via
    /// [`take_command_end`](Grid::take_command_end) after each `advance`. Absent
    /// from `GridSnapshot` — the renderer never reads it, exactly like
    /// `app_cursor_keys` (spec D21).
    command_end: Option<CommandEnd>,
}

/// A parsed OSC 133 command-end event
/// (`ESC ] 133 ; D ; <exit_code> ; <elapsed_ms> ST`).
///
/// Raw shell-reported facts only (spec D21): `term-core` applies **no** policy —
/// no ≥30s completion threshold, no `{1, 130}` error ignore-list, no session
/// keying. The `app` owns all of that. Elapsed time is measured shell-side (from
/// `EPOCHREALTIME`) so it survives laptop sleep without an app-side clock (D19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandEnd {
    /// Exit status of the just-finished foreground command.
    pub exit_code: i32,
    /// Elapsed wall-clock time the shell measured, in milliseconds.
    pub elapsed_ms: u64,
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
            pen_attrs: CellAttrs::NONE,
            history: History::new(DEFAULT_HISTORY_LIMIT),
            view_offset: 0,
            alt_screen: false,
            app_cursor_keys: false,
            bracketed_paste: false,
            selection: None,
            images: ImageLayer::new(),
            command_end: None,
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

    /// Reset the pen to the configured defaults and clear all attributes (SGR 0).
    pub fn reset_pen(&mut self) {
        self.pen_fg = self.default_fg;
        self.pen_bg = self.default_bg;
        self.pen_attrs = CellAttrs::NONE;
    }

    /// Reset only the foreground to the configured default (SGR 39).
    pub fn reset_pen_fg(&mut self) {
        self.pen_fg = self.default_fg;
    }

    /// Reset only the background to the configured default (SGR 49).
    pub fn reset_pen_bg(&mut self) {
        self.pen_bg = self.default_bg;
    }

    /// Set the given attribute bits in the active pen (SGR 1/2/3/4/7).
    pub fn set_pen_attrs(&mut self, attrs: CellAttrs) {
        self.pen_attrs.insert(attrs);
    }

    /// Clear the given attribute bits in the active pen (SGR 22/23/24/27).
    pub fn clear_pen_attrs(&mut self, attrs: CellAttrs) {
        self.pen_attrs.remove(attrs);
    }

    /// Re-theme the grid's default colors live (settings feature): new cells use
    /// the new fg/bg, and existing cells still carrying the *old* defaults (e.g.
    /// blanks) are recolored so a live theme change repaints cleanly. Cells with
    /// explicit SGR colours matching an old default also shift — an accepted
    /// first-cut approximation.
    pub fn set_default_colors(&mut self, fg: Color, bg: Color) {
        let (old_fg, old_bg) = (self.default_fg, self.default_bg);
        for cell in &mut self.cells {
            if cell.fg == old_fg {
                cell.fg = fg;
            }
            if cell.bg == old_bg {
                cell.bg = bg;
            }
        }
        self.default_fg = fg;
        self.default_bg = bg;
        self.pen_fg = fg;
        self.pen_bg = bg;
    }

    /// Print a character at the cursor, advancing by its **display width** and
    /// wrapping. A double-width char (한글/CJK/emoji) occupies two cells — a leader
    /// plus a `Cell::CONTINUATION` spacer carrying the same colors — and advances
    /// the cursor by 2, so the column model matches the shell (a mismatch is what
    /// let backspace erase the prompt). Zero-width chars (combining marks) are
    /// dropped — a first-cut limitation.
    pub fn print(&mut self, ch: char) {
        let w = match ch.width() {
            Some(0) | None => return,
            Some(w) => w as u16,
        };
        // Wrap first if the glyph would not fit on the current line (a wide char
        // never straddles the right margin).
        if self.cursor.col + w > self.cols {
            self.line_feed();
            self.cursor.col = 0;
        }
        let idx = self.index(self.cursor.col, self.cursor.row);
        let cell = Cell {
            ch,
            fg: self.pen_fg,
            bg: self.pen_bg,
            attrs: self.pen_attrs,
        };
        self.cells[idx] = cell;
        // Trailing spacer for a double-width char (guarded against a <2-col grid).
        if w == 2 && self.cursor.col + 1 < self.cols {
            let idx2 = self.index(self.cursor.col + 1, self.cursor.row);
            self.cells[idx2] = Cell {
                ch: Cell::CONTINUATION,
                ..cell
            };
        }
        self.cursor.col += w;
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
    }

    /// Move down one row, scrolling the viewport if at the bottom.
    pub fn line_feed(&mut self) {
        if self.cursor.row + 1 >= self.rows {
            self.feed_scroll();
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

    /// Scroll the viewport up one line on a bottom-row line feed: retain the
    /// departing top row in history (primary screen only, D1/D5), shift cells up,
    /// blank the new bottom row, and auto-increment `view_offset` to pin the
    /// user's scrolled content (D2).
    fn feed_scroll(&mut self) {
        let row_len = self.cols as usize;
        if !self.alt_screen {
            let top: crate::scrollback::Row = self.cells[..row_len].to_vec();
            self.history.push(top);
            if self.view_offset > 0 {
                self.view_offset = (self.view_offset + 1).min(self.history.len());
            }
            // Image placements scroll up with primary content (R1.2).
            self.images.scroll_up_one();
        }
        self.cells.copy_within(row_len.., 0);
        let start = (self.rows as usize - 1) * row_len;
        for cell in &mut self.cells[start..] {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
    }

    // --- Scrollback viewport controls (D2/D4) — display-only, never write the PTY ---

    /// Scroll up `n` lines through retained history (clamped to `history.len()`).
    pub fn scroll_up(&mut self, n: usize) {
        self.view_offset = (self.view_offset + n).min(self.history.len());
    }

    /// Scroll back down `n` lines toward the live view.
    pub fn scroll_down(&mut self, n: usize) {
        self.view_offset = self.view_offset.saturating_sub(n);
    }

    /// Jump to the live view (offset 0).
    pub fn scroll_to_bottom(&mut self) {
        self.view_offset = 0;
    }

    pub fn view_offset(&self) -> usize {
        self.view_offset
    }
    pub fn at_bottom(&self) -> bool {
        self.view_offset == 0
    }
    pub fn at_top(&self) -> bool {
        self.view_offset >= self.history.len()
    }
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Enter/leave the alternate screen (`?1049h`/`?1049l`). While active, output
    /// is not retained in history and scrollback shows the frozen primary history
    /// (D5).
    pub fn set_alt_screen(&mut self, on: bool) {
        self.alt_screen = on;
    }
    pub fn alt_screen(&self) -> bool {
        self.alt_screen
    }

    /// Set/clear DECCKM — application cursor key mode (`?1h`/`?1l`, and cleared by
    /// RIS). Deliberately survives `resize`, `clear`, and alt-screen switching: it
    /// is a keyboard-input mode, not screen content (arrow-key-input R1.5).
    pub fn set_app_cursor_keys(&mut self, on: bool) {
        self.app_cursor_keys = on;
    }
    pub fn app_cursor_keys(&self) -> bool {
        self.app_cursor_keys
    }

    /// Set/clear bracketed-paste mode (`?2004h`/`?2004l`, cleared by RIS). Survives
    /// `resize`/alt-screen like `app_cursor_keys` — it is a keyboard-input mode, not
    /// screen content. Read by the app via `Terminal::bracketed_paste`.
    pub fn set_bracketed_paste(&mut self, on: bool) {
        self.bracketed_paste = on;
    }
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// Set the live drag selection (anchor/focus viewport cells).
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = Some(selection);
    }
    /// Clear any selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }
    /// The current selection, if any.
    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// Record the most-recent OSC 133 command-end event (parser → grid). A newer
    /// event overwrites an unconsumed one — only the latest matters (spec D17:
    /// one waiting item per session).
    pub fn record_command_end(&mut self, event: CommandEnd) {
        self.command_end = Some(event);
    }

    /// Take the most-recent OSC 133 command-end event, clearing it (one-shot
    /// drain). Returns `None` if none is pending.
    pub fn take_command_end(&mut self) -> Option<CommandEnd> {
        self.command_end.take()
    }

    /// Read a retained history row (0 == oldest). Used by snapshot composition.
    pub fn history_row(&self, idx: usize) -> Option<&[Cell]> {
        self.history.get(idx).map(|r| r.as_slice())
    }

    /// A blank cell in the grid's default colors (snapshot padding fallback).
    pub fn blank_cell(&self) -> Cell {
        Cell::blank(self.default_fg, self.default_bg)
    }

    /// Clear the entire grid and home the cursor (CSI 2J-ish).
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
        self.images.clear();
        self.cursor = Cursor::default();
    }

    /// Place a decoded inline image at the cursor cell (image-paste R1.1, SD5).
    /// Returns the placement id. Never writes any cell (images are a side channel).
    pub fn place_image(&mut self, decoded: Decoded) -> u32 {
        self.images.place(decoded, self.cursor.col, self.cursor.row)
    }

    /// The current image placements (read by snapshot composition).
    pub fn images(&self) -> &ImageLayer {
        &self.images
    }

    /// Erase from the cursor to end of the current line (CSI K).
    pub fn clear_line_from_cursor(&mut self) {
        let start = self.index(self.cursor.col, self.cursor.row);
        let end = self.index(0, self.cursor.row) + self.cols as usize;
        for cell in &mut self.cells[start..end] {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
    }

    /// Erase from the cursor (inclusive) to the end of the display (CSI 0J).
    /// Does NOT move the cursor or touch rows above it — this is the erase a shell
    /// sends (`\r ESC[J`) when redrawing its prompt, so it must preserve scrollback.
    pub fn clear_from_cursor(&mut self) {
        let start = self.index(self.cursor.col, self.cursor.row);
        for cell in &mut self.cells[start..] {
            *cell = Cell::blank(self.default_fg, self.default_bg);
        }
    }

    /// Erase from the start of the display to the cursor (inclusive) (CSI 1J).
    pub fn clear_to_cursor(&mut self) {
        let end = (self.index(self.cursor.col, self.cursor.row) + 1).min(self.cells.len());
        for cell in &mut self.cells[..end] {
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
        // A selection's coords are dimension-bound; a resize invalidates it (D9).
        self.selection = None;
    }

    /// Iterate cells in row-major order. Used to build a [`crate::GridSnapshot`].
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Processor;

    fn grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, Color(0xFFFFFF), Color(0))
    }

    fn row_text(g: &Grid, r: u16) -> String {
        let s: String = (0..g.cols())
            .map(|c| g.cell(c, r).unwrap().ch)
            .filter(|&ch| ch != Cell::CONTINUATION)
            .collect();
        s.trim_end().to_string()
    }

    #[test]
    fn wide_char_occupies_two_cells_and_advances_two() {
        let mut g = grid(20, 3);
        let mut p = Processor::new();
        p.advance(&mut g, "한글".as_bytes());
        assert_eq!(
            g.cursor().col,
            4,
            "two double-width chars advance 4 columns"
        );
        assert_eq!(g.cell(0, 0).unwrap().ch, '한');
        assert!(g.cell(1, 0).unwrap().is_continuation(), "leader spacer");
        assert_eq!(g.cell(2, 0).unwrap().ch, '글');
        assert!(g.cell(3, 0).unwrap().is_continuation());
    }

    #[test]
    fn wide_char_wraps_rather_than_splitting_the_margin() {
        let mut g = grid(4, 3);
        let mut p = Processor::new();
        p.advance(&mut g, "aaa한".as_bytes()); // 3 narrow fill cols 0-2; 한 can't fit in col3
        assert_eq!(row_text(&g, 0), "aaa");
        assert_eq!(
            g.cell(0, 1).unwrap().ch,
            '한',
            "wide char wrapped to next row"
        );
        assert_eq!(g.cursor().row, 1);
        assert_eq!(g.cursor().col, 2);
    }

    #[test]
    fn korean_backspace_does_not_erase_the_prompt() {
        // The reported bug: zsh erases a wide char with `\b\b  \b\b` (back 2, two
        // spaces, back 2). With correct width tracking our cursor matches zsh's, so
        // the erases land on the typed chars, not the prompt.
        let mut g = grid(40, 3);
        let mut p = Processor::new();
        p.advance(&mut g, b"PP> "); // prompt cols 0-3
        p.advance(&mut g, "한글".as_bytes()); // cols 4-7, cursor col 8
        assert_eq!(g.cursor().col, 8);
        p.advance(&mut g, b"\x08\x08  \x08\x08"); // erase 글
        p.advance(&mut g, b"\x08\x08  \x08\x08"); // erase 한
        assert_eq!(
            g.cursor().col,
            4,
            "cursor back at the prompt end, not inside it"
        );
        assert_eq!(g.cell(0, 0).unwrap().ch, 'P');
        assert_eq!(g.cell(1, 0).unwrap().ch, 'P');
        assert_eq!(g.cell(2, 0).unwrap().ch, '>');
        assert_eq!(g.cell(3, 0).unwrap().ch, ' ');
        assert_eq!(row_text(&g, 0), "PP>", "prompt intact, typed text cleared");
    }

    #[test]
    fn to_text_skips_wide_continuation() {
        let mut g = grid(20, 2);
        let mut p = Processor::new();
        p.advance(&mut g, "한글".as_bytes());
        let text = crate::snapshot::GridSnapshot::from_grid(&g).to_text();
        assert_eq!(
            text.lines().next().unwrap(),
            "한글",
            "no spacer/NUL in copied text"
        );
    }
}
