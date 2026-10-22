//! The cell grid: the mutable model that VT parsing writes into.
//!
//! Row-major `cols * rows` cells plus a cursor. This is the hand-rolled "manual
//! cell control" surface the project exists to own — deliberately GPU-free so it
//! can be unit-tested headlessly.

use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;

use crate::image::{Decoded, ImageLayer};
use crate::scrollback::{History, DEFAULT_HISTORY_LIMIT};
use crate::selection::{ContentSelection, Point};
use crate::snapshot::soft_wraps;

/// How long an open synchronized update (`?2026h` with no `?2026l` yet) may defer
/// the selection check, measured at drains (selection-scroll-anchor D5).
///
/// An app that dies mid-frame never sends its `?2026l`; without a bound every later
/// check would be skipped and a changed selection would stay highlighted for good.
/// [`Terminal::settle_selection`](crate::Terminal::settle_selection) drops a block
/// at the first settle at least this long after it first saw it open. The time is
/// always the caller's — `term-core` owns the rule, not a clock.
pub const SYNC_STALE: Duration = Duration::from_secs(1);

/// The stored drag selection plus the reference its change check compares against
/// (selection-scroll-anchor D2/D4). One value, so every path that drops the
/// selection — a clear, a resize, a failed check — drops its reference with it.
#[derive(Clone, Debug)]
struct StoredSelection {
    span: ContentSelection,
    /// D4's reference: the selected characters of every selected line that was on
    /// the **live screen** when the selection was set, re-taken on every set. Lines
    /// already in history are left out — history rows are never rewritten, so they
    /// cannot change — which bounds this by one screen (`rows × cols` cells) however
    /// far into history the selection reaches. Empty for a selection lying wholly in
    /// history.
    reference: Vec<ReferenceLine>,
}

/// The characters (`Cell::ch`, continuation spacers included) of one line's
/// selected cells, starting at `first_col`, as they were when the selection was set.
#[derive(Clone, Debug)]
struct ReferenceLine {
    line: u64,
    first_col: u16,
    chars: Vec<char>,
}

/// DECSET `?2026` — synchronized update — as far as the selection check needs it
/// (selection-scroll-anchor D5). Rendering ignores it.
///
/// One enum rather than a flag plus a stamp, so "closed" can never carry a stale
/// stamp: every close (`?2026l`, RIS, leaving the alt screen, the stale timeout)
/// drops the stamp by construction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SyncUpdate {
    Closed,
    /// `seen_at` is the `now` of the first settle that found the block open, or
    /// `None` before any settle has.
    Open {
        seen_at: Option<Instant>,
    },
}

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

/// The state DECSC (`ESC 7`) snapshots and DECRC (`ESC 8`) restores.
///
/// VT100 DECSC saves the cursor position, the graphic rendition, the character
/// set, origin mode, and the wrap flag. We save position, pen, and the deferred
/// wrap; character sets and origin mode are project non-goals, so there is nothing
/// to snapshot for them.
#[derive(Clone, Copy, Debug)]
struct SavedCursor {
    cursor: Cursor,
    pen_fg: Color,
    pen_bg: Color,
    pen_attrs: CellAttrs,
    pending_wrap: bool,
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
    /// Which mouse events the app asked for (`?9`/`?1000`/`?1002`/`?1003`) and how
    /// they should be encoded (`?1006`).
    ///
    /// Keyboard/mouse *input* modes, exactly like `bracketed_paste`: they live here,
    /// survive resize, are cleared by RIS, and are absent from `GridSnapshot`. The
    /// app reads them to decide whether a wheel gesture is a mouse report for the
    /// application or a scrollback move for the terminal.
    /// One flag per mode, as in xterm — NOT a single collapsing value. Apps enable
    /// several at once (`?1000` + `?1002` + `?1003`) and may disable them one at a
    /// time; collapsing that to "off" killed the wheel mid-session.
    mouse_x10: bool,
    mouse_normal: bool,
    mouse_button_event: bool,
    mouse_any_event: bool,
    mouse_encoding: MouseEncoding,
    /// The drag selection as two content [`Point`]s (selection-scroll-anchor D2), or
    /// `None`. Content-anchored, so it follows its text through terminal-owned
    /// scrolling. The renderer sees it projected into viewport cells on
    /// `GridSnapshot.selection` (at every `view_offset`); its text comes from
    /// [`selection_text`](Grid::selection_text), which reads history + live by line.
    /// Cleared on `resize` (D7: no rewrap, so line content is dimension-bound), and
    /// by the D4 change check when its selected text changes.
    selection: Option<StoredSelection>,
    /// DECSET `?2026` synchronized update, which times the D4 change check
    /// (selection-scroll-anchor D5): the check runs at `?2026l` and at each drain's
    /// settle, never inside an open block. Absent from `GridSnapshot` — rendering
    /// does not honour it.
    sync: SyncUpdate,
    /// Inline-image placements anchored to cells (image-paste D2, SD5).
    images: ImageLayer,
    /// DECSTBM top/bottom margins — the inclusive row range scrolling is confined
    /// to (`vt-tui-completeness` D9). A fresh grid, and every reset path, spans the
    /// whole screen (`0..=rows-1`), which is what keeps every pre-existing scroll
    /// behavior byte-identical (R1.1/R3.4).
    ///
    /// Terminal *mode* state like `alt_screen`/`app_cursor_keys`: it lives here and
    /// is deliberately absent from `GridSnapshot` — the renderer never reads it
    /// (R1.6). Lifetime: reset by `CSI r`, alt-screen enter **and** leave (D2), RIS
    /// (D5), and `resize` (D7). Only the `CSI r` path homes the cursor (D9/R4.5).
    scroll_top: u16,
    scroll_bottom: u16,
    /// DECAWM — autowrap mode (`?7h`/`?7l`). When off, printing at the last column
    /// overwrites that cell in place instead of moving to the next row
    /// (vt-tui-completeness D3). Defaults on, which is the VT default and the
    /// pre-feature behavior.
    autowrap: bool,
    /// DECSC/DECRC saved state (`ESC 7`/`ESC 8`, `CSI s`/`CSI u`).
    ///
    /// Seeded to "home, default pen" so a DECRC with no prior DECSC homes the
    /// cursor — the VT behavior. Claude Code brackets its `CSI r` with this pair,
    /// and because `CSI r` homes the cursor (D9), dropping DECRC left the cursor
    /// stranded at the origin; implementing it is what makes that idiom safe.
    saved_cursor: SavedCursor,
    /// The DEC *deferred* wrap flag: set when a character lands in the last column,
    /// consumed by the next printable character (D3).
    ///
    /// This is what lets the cursor rest **on** the last column. Eager wrapping
    /// could not — it moved to the next row immediately, so a full-width line ate a
    /// blank row and every box-drawing TUI sat one row off from its first frame.
    /// Cleared by any explicit repositioning (D8): all cursor moves, CR, LF,
    /// backspace, tab, and `resize`. Deliberately *not* cleared by `?7l` (the flag
    /// is retained but not honored while autowrap is off), nor by the erase ops
    /// that leave the cursor alone — that retention is what makes `CSI K` after a
    /// full line erase only the last column (R7.3/R7.7). `CSI 2J`/`3J` are the
    /// exception among erases: they route through [`clear`](Grid::clear), which
    /// *homes* the cursor, and homing clears the flag like any other move.
    pending_wrap: bool,
    /// Most-recent OSC 133 command-end event parsed from the shell's output
    /// (shell-integration D10/D21). Transient, one-shot: the app drains it via
    /// [`take_command_end`](Grid::take_command_end) after each `advance`. Absent
    /// from `GridSnapshot` — the renderer never reads it, exactly like
    /// `app_cursor_keys` (spec D21).
    command_end: Option<CommandEnd>,
    /// Whether the shell is running a foreground command: set by an OSC 133 `C`
    /// (command start), cleared by a well-formed `D` (command end), in byte order
    /// (sidebar-weather-cards D8). A **state**, not a one-shot event like
    /// `command_end`: the app reads the value the stream left it at, so a chunk
    /// carrying `C…D` (a fast `ls`) or `D…C` (typeahead) resolves by order alone.
    /// Untouched by RIS — it is the shell's state, not a terminal mode.
    command_running: bool,
    /// Most-recent OSC 0 / OSC 2 window-title set by the shell or user
    /// (`ESC ] 2 ; <title> ST`) — the tab-identity manual-label hook. Transient,
    /// one-shot like `command_end`: the app drains it via
    /// [`take_osc_title`](Grid::take_osc_title) after each `advance` and stores it on
    /// the session. An empty title is a valid value (it clears the manual label back to
    /// the auto/cwd label). Absent from `GridSnapshot` — the renderer never reads it.
    osc_title: Option<String>,
}

/// Which mouse events the application has asked to receive (`?9`, `?1000`,
/// `?1002`, `?1003`).
///
/// Input state, like [`Grid::bracketed_paste`]: the app reads it to decide whether
/// a wheel gesture becomes a mouse report or a viewport scroll. Deliberately absent
/// from `GridSnapshot` — the renderer never reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseProtocol {
    /// No tracking requested — the wheel belongs to the terminal.
    #[default]
    Off,
    /// `?9` X10 compatibility: button presses only.
    X10,
    /// `?1000` normal tracking: press and release.
    Normal,
    /// `?1002` button-event tracking: press, release, and motion while held.
    ButtonEvent,
    /// `?1003` any-event tracking: press, release, and all motion.
    AnyEvent,
}

impl MouseProtocol {
    /// True when the application wants mouse reports at all.
    pub fn is_on(self) -> bool {
        self != MouseProtocol::Off
    }
}

/// How mouse reports are encoded on the wire (`?1006`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// The original form: `ESC [ M` then three bytes, each `value + 32`. Cannot
    /// express a coordinate past 223. Named `Legacy`, not `X10`, so it is not
    /// confused with [`MouseProtocol::X10`] — those are different axes, and
    /// `MouseProtocol::X10 + MouseEncoding::Sgr` is a legal combination.
    #[default]
    Legacy,
    /// `?1006` SGR form: `ESC [ < b ; col ; row M`. No coordinate ceiling.
    Sgr,
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
            mouse_x10: false,
            mouse_normal: false,
            mouse_button_event: false,
            mouse_any_event: false,
            mouse_encoding: MouseEncoding::Legacy,
            selection: None,
            sync: SyncUpdate::Closed,
            images: ImageLayer::new(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            autowrap: true,
            pending_wrap: false,
            saved_cursor: SavedCursor {
                cursor: Cursor::default(),
                pen_fg: fg,
                pen_bg: bg,
                pen_attrs: CellAttrs::NONE,
                pending_wrap: false,
            },
            command_end: None,
            command_running: false,
            osc_title: None,
        }
    }

    /// A grid whose scrollback keeps at most `limit` rows — **test-only**, so a
    /// test can drive eviction in a few lines instead of 100 000
    /// (selection-scroll-anchor R1.4/R4.5). Production grids always use
    /// [`DEFAULT_HISTORY_LIMIT`].
    #[cfg(test)]
    pub(crate) fn with_history_limit(
        cols: u16,
        rows: u16,
        fg: Color,
        bg: Color,
        limit: usize,
    ) -> Self {
        let mut grid = Grid::new(cols, rows, fg, bg);
        grid.history = History::new(limit);
        grid
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
        // A glyph too wide for the whole grid can never be placed without violating
        // the leader+spacer pair invariant, so drop it outright. Checked BEFORE the
        // deferred wrap is consumed: a glyph that is never drawn must not scroll the
        // screen on its way to being discarded.
        if w > self.cols {
            return;
        }
        // Take the deferred wrap left by the previous character, if any (D3). This
        // runs before the fit check so the cursor is already on the new row when a
        // wide char measures itself against the right margin.
        if self.pending_wrap {
            self.pending_wrap = false;
            if self.autowrap {
                self.line_feed();
                self.cursor.col = 0;
            }
        }
        // Wrap if the glyph would not fit on the current line (a wide char never
        // straddles the right margin). With autowrap off there is nowhere to go —
        // drop the glyph and leave the cursor at the margin. It must NOT back up to
        // make room: no VT operation moves the cursor left as a side effect of
        // printing, and doing so destroyed the cell to the left of the margin.
        if self.cursor.col + w > self.cols {
            if !self.autowrap {
                return;
            }
            self.line_feed();
            self.cursor.col = 0;
        }
        // Landing on half of an existing double-width pair orphans the other half,
        // so break the pair before overwriting it. Without this, printing over a
        // spacer left its leader behind and the renderer painted that leader across
        // two cells — e.g. `abcdef한` then `Z` over the spacer yielded `abcdef한Z`,
        // nine display columns in an eight-cell row. O(1), unlike the row-wide
        // `repair_wide_pairs` the shifting ops use — `print` is the hot path.
        let (col, row) = (self.cursor.col, self.cursor.row);
        self.break_wide_pair_at(col, row);
        if w == 2 && col + 1 < self.cols {
            self.break_wide_pair_at(col + 1, row);
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
        // Land ON the last column and defer the wrap, rather than moving past it
        // (D3/R7.1). With autowrap off the cursor just parks there and the next
        // character overwrites in place (R7.5).
        if self.cursor.col + w >= self.cols {
            self.cursor.col = self.cols - 1;
            self.pending_wrap = self.autowrap;
        } else {
            self.cursor.col += w;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.pending_wrap = false;
    }

    // --- DECAWM autowrap + the deferred-wrap flag (vt-tui-completeness R7) ---

    /// Set/clear DECAWM (`?7h`/`?7l`). Deliberately does **not** clear a pending
    /// wrap: the flag is retained but only honored while autowrap is on, which
    /// matches DEC retention semantics (D8/R7.7).
    pub fn set_autowrap(&mut self, on: bool) {
        self.autowrap = on;
    }

    pub fn autowrap(&self) -> bool {
        self.autowrap
    }

    /// True when the previous character landed in the last column and its wrap is
    /// still deferred. Exposed for tests and the composite-frame assertions; the
    /// renderer never reads it, so it stays out of `GridSnapshot`.
    pub fn pending_wrap(&self) -> bool {
        self.pending_wrap
    }

    /// Move down one row, scrolling when the cursor is sitting on the **scroll
    /// region's** bottom margin rather than the screen's (vt-tui-completeness R2.1).
    ///
    /// With no region set the two coincide, so this is the pre-feature behavior
    /// exactly. With a region set, a TUI's line feed at the region bottom scrolls
    /// only its pane. A cursor parked *below* the region on the last screen row
    /// simply stays put — it is outside the scrolling band.
    pub fn line_feed(&mut self) {
        self.pending_wrap = false;
        if self.cursor.row == self.scroll_bottom {
            self.feed_scroll();
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
    }

    pub fn backspace(&mut self) {
        self.cursor.col = self.cursor.col.saturating_sub(1);
        self.pending_wrap = false;
    }

    /// Advance to the next multiple-of-8 tab stop.
    pub fn tab(&mut self) {
        let next = (self.cursor.col / 8 + 1) * 8;
        self.cursor.col = next.min(self.cols - 1);
        self.pending_wrap = false;
    }

    /// Move the cursor to an absolute (1-based in VT terms) position, clamped.
    ///
    /// **Every cursor repositioning must route through here**, because this is the
    /// single place that clears `pending_wrap` — that is what satisfies D8's "all
    /// cursor moves clear the pending wrap" for CUP/CUU/CUD/CUF/CUB/CHA/VPA, and
    /// also why `set_scroll_region` and `clear` home through it rather than
    /// assigning `self.cursor` directly. `reverse_index` is the one repositioning
    /// path that cannot (it may scroll instead of move), so it clears the flag
    /// itself.
    pub fn move_to(&mut self, col: u16, row: u16) {
        self.cursor.col = col.min(self.cols - 1);
        self.cursor.row = row.min(self.rows - 1);
        self.pending_wrap = false;
    }

    /// A blank cell carrying the **active pen background** and the default
    /// foreground — the Background-Color-Erase fill (vt-tui-completeness D6).
    ///
    /// Used by every scroll-exposed or erased-by-a-new-op cell — LF, IND, SU, SD,
    /// IL, DL, RI, ICH, DCH and ECH — so a TUI that paints a colored pane and then
    /// scrolls or erases does not get default-colored holes.
    ///
    /// **Not** used by the pre-existing `J`/`K` erases, which keep [`Cell::blank`].
    /// That split is deliberate (spec D6) and recorded as Known Gap G-A: retrofitting
    /// BCE onto ED/EL would move already-shipped behavior and existing snapshots.
    fn pen_blank(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: self.default_fg,
            bg: self.pen_bg,
            attrs: CellAttrs::NONE,
        }
    }

    /// Move rows within an arbitrary inclusive `[top, bottom]` band up by `n`,
    /// filling the vacated bottom rows with `fill`. Pure cell movement — **no**
    /// history, offset, or image side effects (those live in
    /// [`scroll_region_up`](Grid::scroll_region_up)).
    ///
    /// `n` saturates at the band height, so an over-large count blanks the band
    /// rather than reading out of bounds (R2.10).
    fn scroll_rows_up(&mut self, top: u16, bottom: u16, n: u16, fill: Cell) {
        if top > bottom || bottom >= self.rows {
            return;
        }
        let row_len = self.cols as usize;
        let band = (bottom - top + 1) as usize;
        let n = (n as usize).min(band);
        if n == 0 {
            return;
        }
        let first = top as usize * row_len;
        let end = (bottom as usize + 1) * row_len;
        if n < band {
            self.cells.copy_within(first + n * row_len..end, first);
        }
        for cell in &mut self.cells[end - n * row_len..end] {
            *cell = fill;
        }
    }

    /// Move rows within an inclusive `[top, bottom]` band down by `n`, filling the
    /// vacated top rows with `fill`. Never touches history: scrolling *down*
    /// retires rows off the band's bottom, which are discarded, not retained.
    fn scroll_rows_down(&mut self, top: u16, bottom: u16, n: u16, fill: Cell) {
        if top > bottom || bottom >= self.rows {
            return;
        }
        let row_len = self.cols as usize;
        let band = (bottom - top + 1) as usize;
        let n = (n as usize).min(band);
        if n == 0 {
            return;
        }
        let first = top as usize * row_len;
        let end = (bottom as usize + 1) * row_len;
        if n < band {
            self.cells
                .copy_within(first..end - n * row_len, first + n * row_len);
        }
        for cell in &mut self.cells[first..first + n * row_len] {
            *cell = fill;
        }
    }

    /// Scroll the **active scroll region** up by `n`, retaining the departing rows
    /// in scrollback when — and only when — this is a real full-screen scroll.
    ///
    /// This is the single place the history push / `view_offset` pin / image shift
    /// live (D10). The guard is the heart of D1: a TUI streaming into a partial
    /// region scrolls hundreds of times per response, and retaining those frames
    /// would bury the user's real scrollback in redraw garbage. `alt_screen` is the
    /// pre-existing half of the same guard (D5).
    fn scroll_region_up(&mut self, n: u16, fill: Cell) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let band = bottom.saturating_sub(top) + 1;
        let n = n.min(band);
        if n == 0 {
            return;
        }
        if self.region_is_full_screen() && !self.alt_screen {
            let row_len = self.cols as usize;
            for i in 0..n as usize {
                let start = (top as usize + i) * row_len;
                let row: crate::scrollback::Row = self.cells[start..start + row_len].to_vec();
                self.history.push(row);
                if self.view_offset > 0 {
                    self.view_offset = (self.view_offset + 1).min(self.history.len());
                }
                // Image placements scroll up with primary content (image-paste R1.2).
                self.images.scroll_up_one();
            }
        }
        self.scroll_rows_up(top, bottom, n, fill);
    }

    /// Scroll the active scroll region down by `n`. Never retains history.
    fn scroll_region_down(&mut self, n: u16, fill: Cell) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        self.scroll_rows_down(top, bottom, n, fill);
    }

    // --- Region-aware redraw sequences (vt-tui-completeness R2) ---

    /// SU (`CSI n S`) — scroll the region up `n`, blanking in at the region bottom.
    /// The cursor does not move.
    pub fn scroll_up_lines(&mut self, n: u16) {
        let fill = self.pen_blank();
        self.scroll_region_up(n.max(1), fill);
    }

    /// SD (`CSI n T`) — scroll the region down `n`, blanking in at the region top.
    /// The cursor does not move.
    pub fn scroll_down_lines(&mut self, n: u16) {
        let fill = self.pen_blank();
        self.scroll_region_down(n.max(1), fill);
    }

    /// IL (`CSI n L`) — insert `n` blank lines at the cursor row, pushing the rest
    /// of the region down. A no-op when the cursor sits outside the region (R2.6).
    ///
    /// Scrolls only `[cursor_row, region_bottom]` and never retires anything to
    /// scrollback — IL is a *down*-scroll, so no row departs the top and there is
    /// nothing to retain (D1's per-op predicate: "IL / SD / RI — never retain").
    /// The cursor's row and column are both left alone (D11).
    pub fn insert_lines(&mut self, n: u16) {
        if !self.cursor_in_region() {
            return;
        }
        let fill = self.pen_blank();
        self.scroll_rows_down(self.cursor.row, self.scroll_bottom, n.max(1), fill);
    }

    /// DL (`CSI n M`) — delete `n` lines at the cursor row, pulling the rest of the
    /// region up. A no-op when the cursor sits outside the region (R2.6).
    ///
    /// Unlike IL, DL **can** retire content off the top of the screen: when the
    /// cursor sits on the region top, the band it scrolls IS the region, so the
    /// departing rows are the same ones a line feed would retire. It therefore
    /// routes through [`scroll_region_up`](Grid::scroll_region_up) in that case,
    /// picking up the history/`view_offset`/image block — which is what makes
    /// `ESC[1M` and `ESC[1S` agree on a full-screen primary region instead of
    /// silently dropping a row from scrollback. Below the region top the band is a
    /// strict subset, nothing leaves the screen, and no history is retained.
    pub fn delete_lines(&mut self, n: u16) {
        if !self.cursor_in_region() {
            return;
        }
        let fill = self.pen_blank();
        if self.cursor.row == self.scroll_top {
            // Band == region: goes through the one place that owns history (D10).
            self.scroll_region_up(n.max(1), fill);
        } else {
            self.scroll_rows_up(self.cursor.row, self.scroll_bottom, n.max(1), fill);
        }
    }

    /// RI (`ESC M`) — reverse index: move the cursor up one row, scrolling the
    /// region *down* instead when it is already at the region top (R2.8/R2.9).
    pub fn reverse_index(&mut self) {
        // RI repositions the cursor, so it clears the deferred wrap like every
        // other cursor move (D8). Missing this let a pending wrap survive an RI and
        // fire on the next character, clobbering the row it landed on.
        self.pending_wrap = false;
        if self.cursor.row == self.scroll_top {
            let fill = self.pen_blank();
            self.scroll_region_down(1, fill);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    /// True when the cursor row lies inside the active scroll region.
    fn cursor_in_region(&self) -> bool {
        self.cursor.row >= self.scroll_top && self.cursor.row <= self.scroll_bottom
    }

    /// Scroll on a line feed that reached the region bottom: retain the departing
    /// row in history (primary screen, full-screen region only — D1/D5), shift
    /// cells up, blank the new bottom row, and auto-increment `view_offset` to pin
    /// the user's scrolled content (D2).
    ///
    /// Fills the new row with [`pen_blank`](Grid::pen_blank) — Background Color
    /// Erase, like every other scroll-exposed row (D6).
    ///
    /// LF and IND are the same index operation and must fill identically; xterm
    /// advertises `bce` and applies it to both. R3.4's equivalence requirement does
    /// **not** constrain fill colour — it pins history growth, `view_offset`
    /// pinning, and image placements — so BCE here is compatible with it.
    fn feed_scroll(&mut self) {
        let fill = self.pen_blank();
        self.scroll_region_up(1, fill);
    }

    // --- DECSC / DECRC saved cursor (vt-tui-completeness D17) ---

    /// DECSC (`ESC 7`, and `CSI s`) — snapshot the cursor position, the active pen,
    /// and the deferred-wrap flag.
    pub fn save_cursor(&mut self) {
        self.saved_cursor = SavedCursor {
            cursor: self.cursor,
            pen_fg: self.pen_fg,
            pen_bg: self.pen_bg,
            pen_attrs: self.pen_attrs,
            pending_wrap: self.pending_wrap,
        };
    }

    /// DECRC (`ESC 8`, and `CSI u`) — restore what DECSC saved.
    ///
    /// The position is clamped to the current dimensions, because the grid may have
    /// been resized between the save and the restore. With no prior DECSC this
    /// homes the cursor and resets the pen, which is the VT behavior.
    pub fn restore_cursor(&mut self) {
        let s = self.saved_cursor;
        self.cursor = Cursor {
            col: s.cursor.col.min(self.cols - 1),
            row: s.cursor.row.min(self.rows - 1),
        };
        self.pen_fg = s.pen_fg;
        self.pen_bg = s.pen_bg;
        self.pen_attrs = s.pen_attrs;
        // Assigning `cursor` directly bypasses `move_to`'s flag clear, so the saved
        // wrap state is restored explicitly — that is the point of saving it.
        self.pending_wrap = s.pending_wrap;
    }

    // --- DECSTBM scroll region (vt-tui-completeness D9/D10) ---

    /// The active scroll region as an inclusive `(top, bottom)` row pair.
    pub fn scroll_region(&self) -> (u16, u16) {
        (self.scroll_top, self.scroll_bottom)
    }

    /// True when the region spans the whole screen — the predicate that decides
    /// whether a scroll is "real" terminal scrolling (retained in history, moves
    /// image placements) or a TUI repainting its own pane (D1/R3.1).
    pub fn region_is_full_screen(&self) -> bool {
        self.scroll_top == 0 && self.scroll_bottom == self.rows - 1
    }

    /// DECSTBM (`CSI t ; b r`) — set the scroll region from **0-based, inclusive**
    /// rows and home the cursor (R1.2).
    ///
    /// An inverted region (`top >= bottom`) is rejected: the whole sequence is
    /// ignored and the previous region survives, matching real terminals (R1.4).
    /// `bottom` is clamped to the last row, so an over-large parameter widens the
    /// region rather than storing an out-of-bounds index (R1.5).
    ///
    /// This is the **only** path that homes the cursor. The internal resets
    /// (alt-screen, RIS, resize) go through [`reset_scroll_region`] instead —
    /// see D9's cursor-homing boundary.
    ///
    /// [`reset_scroll_region`]: Grid::reset_scroll_region
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let bottom = bottom.min(self.rows - 1);
        let top = top.min(self.rows - 1);
        if top >= bottom {
            return;
        }
        self.scroll_top = top;
        self.scroll_bottom = bottom;
        // Home via `move_to`, never by assigning `cursor` directly: `move_to` is
        // also what clears `pending_wrap`. Assigning the field bypassed that and
        // left a deferred wrap armed on a homed cursor, so the next character
        // landed on row 1 instead of row 0.
        self.move_to(0, 0);
    }

    /// Reset the region to the full screen **without touching the cursor**.
    ///
    /// Used by every non-`CSI r` reset path: alt-screen enter/leave (D2), RIS (D5),
    /// and `resize` (D7). Keeping this separate from [`set_scroll_region`] is what
    /// makes R4.5 hold — `resize` clamps the cursor deliberately and `ESC c` is a
    /// deliberately minimal RIS that moves nothing.
    ///
    /// [`set_scroll_region`]: Grid::set_scroll_region
    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
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
    ///
    /// Both directions reset the scroll region (vt-tui-completeness D2). A TUI
    /// killed mid-run never sends `CSI r` to restore full-screen margins, and this
    /// codebase's alt-screen is a history-preserving *switch* rather than a second
    /// buffer — so there is no per-buffer margin state to preserve, and without
    /// this the shell would come back scrolling inside the dead app's region. The
    /// cursor is deliberately **not** homed (R4.1/R4.5).
    ///
    /// Leaving also closes an open synchronized update, for the same reason: an app
    /// that dies mid-frame leaves its `?2026h` open, and its shell's prompt must not
    /// inherit the deferral (selection-scroll-anchor D5). That close runs no check
    /// of its own (D7) — the next settle does.
    pub fn set_alt_screen(&mut self, on: bool) {
        self.alt_screen = on;
        self.reset_scroll_region();
        if !on {
            self.reset_synchronized_update();
        }
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

    /// Set or clear ONE mouse-tracking mode (`?9`/`?1000`/`?1002`/`?1003`).
    ///
    /// Each mode is tracked independently, matching xterm: disabling `?1003` while
    /// `?1000` is still on leaves tracking on. `MouseProtocol::Off` clears all four
    /// (the RIS path).
    pub fn set_mouse_mode(&mut self, mode: MouseProtocol, on: bool) {
        match mode {
            MouseProtocol::X10 => self.mouse_x10 = on,
            MouseProtocol::Normal => self.mouse_normal = on,
            MouseProtocol::ButtonEvent => self.mouse_button_event = on,
            MouseProtocol::AnyEvent => self.mouse_any_event = on,
            MouseProtocol::Off => {
                self.mouse_x10 = false;
                self.mouse_normal = false;
                self.mouse_button_event = false;
                self.mouse_any_event = false;
            }
        }
    }

    /// The effective protocol: the most capable mode currently enabled. Reporting a
    /// single winner keeps the app's decision simple — for the wheel, any enabled
    /// mode means "the app owns this gesture".
    pub fn mouse_protocol(&self) -> MouseProtocol {
        if self.mouse_any_event {
            MouseProtocol::AnyEvent
        } else if self.mouse_button_event {
            MouseProtocol::ButtonEvent
        } else if self.mouse_normal {
            MouseProtocol::Normal
        } else if self.mouse_x10 {
            MouseProtocol::X10
        } else {
            MouseProtocol::Off
        }
    }

    /// Select the mouse report encoding (`?1006` on → SGR, off → X10).
    pub fn set_mouse_encoding(&mut self, encoding: MouseEncoding) {
        self.mouse_encoding = encoding;
    }
    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.mouse_encoding
    }

    /// Set the drag selection from two content points (selection-scroll-anchor D3).
    /// `anchor` is where the drag began, `focus` its current end; either order is
    /// accepted. Build them with [`point_at`](Grid::point_at) at the moment the cell
    /// is learned.
    ///
    /// Every set re-takes the D4 reference from the cells as they are **now**, so
    /// each drag move re-baselines it. There is no deferred capture.
    pub fn set_selection(&mut self, anchor: Point, focus: Point) {
        let span = ContentSelection { anchor, focus };
        let reference = self.live_reference(span);
        self.selection = Some(StoredSelection { span, reference });
    }
    /// Clear any selection (and with it, its reference).
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }
    /// The stored selection (content points, as set), if any.
    pub fn selection(&self) -> Option<ContentSelection> {
        self.selection.as_ref().map(|s| s.span)
    }

    /// D4's reference for `span`: the selected characters of each selected line on
    /// the live screen at this moment — lines `evicted + history_len` and below,
    /// whatever the `view_offset`. At most one screen of cells; empty when the span
    /// lies wholly in history.
    fn live_reference(&self, span: ContentSelection) -> Vec<ReferenceLine> {
        let (start, end) = span.normalized();
        let last_col = self.cols - 1;
        let first_line = start.line.max(self.live_top_line());
        let last_line = end.line.min(self.live_bottom_line());
        (first_line..=last_line)
            .filter_map(|line| {
                let (first, last) = selected_cols(start, end, line, last_col);
                let cells = self
                    .line(line)?
                    .get(usize::from(first)..=usize::from(last))?;
                Some(ReferenceLine {
                    line,
                    first_col: first,
                    chars: cells.iter().map(|c| c.ch).collect(),
                })
            })
            .collect()
    }

    /// The D4 test, read-only — the **one** predicate both the change check and
    /// copy's re-verify (D6) use: whether `sel` no longer names the text it was set
    /// on. True when
    ///
    /// - **(a)** its start — the earlier point in reading order, not necessarily the
    ///   anchor — is older than the oldest retained line. Eviction takes the oldest
    ///   lines first, so any evicted selected line implies an evicted start; or
    /// - **(b)** any referenced cell, re-read at its line (now in history or still
    ///   live), holds a different character.
    ///
    /// Characters only: a colour or attribute change is not a change (copy would
    /// return the same text). A line that moved into history compares equal by
    /// construction, since history rows are never rewritten — which is exactly why
    /// terminal-owned scrolling keeps a selection and an app's repaint does not.
    fn selection_changed(&self, sel: &StoredSelection) -> bool {
        let (start, _) = sel.span.normalized();
        start.line < self.history.evicted()
            || sel.reference.iter().any(|r| !self.reference_line_holds(r))
    }

    /// Whether `r`'s line still holds `r`'s characters at `r`'s columns.
    fn reference_line_holds(&self, r: &ReferenceLine) -> bool {
        let first = usize::from(r.first_col);
        self.line(r.line)
            .and_then(|cells| cells.get(first..first + r.chars.len()))
            .is_some_and(|cells| cells.iter().map(|c| c.ch).eq(r.chars.iter().copied()))
    }

    /// Run the D4 check now: clear the selection if its text changed, logging the
    /// `trigger` that caught it (selection-scroll-anchor R5.9 — `sync-end`, `drain`
    /// or `sync-timeout`). No work at all without a selection (C5); otherwise at most
    /// one screen of cell compares.
    fn check_selection(&mut self, trigger: &'static str) {
        if self
            .selection
            .as_ref()
            .is_some_and(|sel| self.selection_changed(sel))
        {
            tracing::debug!(
                trigger,
                "selection cleared: its text changed or its start left scrollback"
            );
            self.selection = None;
        }
    }

    /// DECSET/DECRST `?2026` — open or close a synchronized update
    /// (selection-scroll-anchor D5).
    ///
    /// `?2026l` that closes an **open** block is the exact end of the app's frame, so
    /// it runs the D4 check (trigger `sync-end`) right here, mid-chunk, with no settle
    /// needed: whatever was painted inside the block is complete.
    ///
    /// A `?2026l` with no block open ends no frame. It changes nothing and checks
    /// nothing. Checking there would be a check in the middle of whatever the app is
    /// painting (D4/D5: never mid-frame), so a stray close from an app that never
    /// opened a block could clear a selection whose text the rest of its repaint
    /// restores. The drain settle still checks that output.
    ///
    /// A `?2026h` while a block is **already** open changes nothing — in particular
    /// it keeps the block's stamp. Re-opening would reset the stamp, and an app that
    /// repeated `?2026h` without ever closing could then defer the check forever,
    /// which is the unbounded case [`SYNC_STALE`] exists to rule out.
    pub(crate) fn set_synchronized_update(&mut self, on: bool) {
        let was_open = self.sync != SyncUpdate::Closed;
        if !on {
            self.sync = SyncUpdate::Closed;
            if was_open {
                self.check_selection("sync-end");
            }
        } else if !was_open {
            self.sync = SyncUpdate::Open { seen_at: None };
        }
    }

    /// Close an open synchronized update **without** a check of its own — RIS and
    /// leaving the alt screen (selection-scroll-anchor D5). Neither is a frame end,
    /// and D7 gives neither a dedicated clear: the next settle checks, now that
    /// nothing defers it.
    pub(crate) fn reset_synchronized_update(&mut self) {
        self.sync = SyncUpdate::Closed;
    }

    /// Whether a synchronized update is open — test-only; nothing outside the
    /// selection check reads it.
    #[cfg(test)]
    pub(crate) fn synchronized_update(&self) -> bool {
        matches!(self.sync, SyncUpdate::Open { .. })
    }

    /// The end-of-drain half of the D4 timing (selection-scroll-anchor D5); see
    /// [`Terminal::settle_selection`](crate::Terminal::settle_selection).
    ///
    /// - no block open → check now (trigger `drain`);
    /// - a block open that no settle has seen yet → stamp it `now`, skip;
    /// - a block open for less than [`SYNC_STALE`] since its stamp → skip;
    /// - one open for at least [`SYNC_STALE`] → drop it, then check (trigger
    ///   `sync-timeout`).
    ///
    /// A `now` earlier than the stamp counts as no time elapsed.
    pub(crate) fn settle_selection(&mut self, now: Instant) {
        match self.sync {
            SyncUpdate::Closed => self.check_selection("drain"),
            SyncUpdate::Open { seen_at: None } => {
                self.sync = SyncUpdate::Open { seen_at: Some(now) };
            }
            SyncUpdate::Open {
                seen_at: Some(seen),
            } => {
                if now.saturating_duration_since(seen) >= SYNC_STALE {
                    self.sync = SyncUpdate::Closed;
                    self.check_selection("sync-timeout");
                }
            }
        }
    }

    /// Rows that have aged out of the scrollback cap since this grid was created
    /// (selection-scroll-anchor D1). History row `i` is content line
    /// `evicted() + i`.
    pub fn evicted(&self) -> u64 {
        self.history.evicted()
    }

    /// The content line of live row 0: `evicted + history_len` (D1).
    fn live_top_line(&self) -> u64 {
        self.history.evicted() + self.history.len() as u64
    }

    /// The content line of the last live row — the newest line that exists.
    fn live_bottom_line(&self) -> u64 {
        self.live_top_line() + u64::from(self.rows) - 1
    }

    /// The content line shown on viewport row 0 at the current `view_offset`
    /// (selection-scroll-anchor D2's `top`). `view_offset` never exceeds
    /// `history_len` (every writer clamps it), so this cannot underflow.
    pub(crate) fn top_line(&self) -> u64 {
        self.live_top_line() - self.view_offset as u64
    }

    /// The content [`Point`] under viewport cell `(col, row)` at the current
    /// `view_offset` (selection-scroll-anchor D3): line `top + row`, same column.
    ///
    /// Pure arithmetic, deliberately unclamped: a cell outside the viewport maps to
    /// a line outside it, which the snapshot projection then clips or drops.
    pub fn point_at(&self, col: u16, row: u16) -> Point {
        Point {
            line: self.top_line() + u64::from(row),
            col,
        }
    }

    /// The cells of content line `line` — a retained history row at the width it
    /// was **captured** at, or a live row at the current width — or `None` when the
    /// line has been evicted or lies below the live bottom.
    pub(crate) fn line(&self, line: u64) -> Option<&[Cell]> {
        let idx = line.checked_sub(self.history.evicted())?;
        let hist = self.history.len() as u64;
        if idx < hist {
            return self.history_row(usize::try_from(idx).ok()?);
        }
        let cols = self.cols as usize;
        let start = usize::try_from(idx - hist).ok()?.checked_mul(cols)?;
        self.cells.get(start..start.checked_add(cols)?)
    }

    /// The stored selection's text, read from history + live **by line number**
    /// (selection-scroll-anchor D6), or `None` when there is no selection **or its
    /// text has changed**.
    ///
    /// This is the one extractor: it returns the whole selection even when part of
    /// it has scrolled off-screen, which the visible snapshot cannot. It walks the
    /// linear span (the start line from `start.col`, interior lines full-width, the
    /// end line up to `end.col`), skipping double-width continuation cells and
    /// `trim_end`-ing each line segment — the same shape as
    /// [`GridSnapshot::to_text`](crate::GridSnapshot::to_text). Lines are joined with
    /// `\n`, **except** that a soft-wrapped line joins the next with no separator,
    /// by the D8 rule shared with hyperlinks ([`soft_wraps`]).
    ///
    /// **Copy re-verifies** (D6). Before reading, it runs the D4 test itself —
    /// read-only, and whatever the check's timing (D5) — and returns `None` when the
    /// selection's start has been evicted or a referenced cell now holds a different
    /// character. So it can never return text other than what was selected, even in
    /// the window before the next check clears the highlight. (This supersedes the
    /// earlier rule that a line no longer retained reads as blank: an evicted start
    /// now refuses the copy, and every later selected line is still retained.)
    ///
    /// Every line is read through the **current** `cols`: a history row captured
    /// wider is cut at the right margin and one captured narrower is padded blank,
    /// matching what the snapshot shows. A span reaching past the live bottom stops
    /// there. Never panics on out-of-range points.
    pub fn selection_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        if self.selection_changed(sel) {
            return None;
        }
        let (start, end) = sel.span.normalized();
        let last_col = self.cols - 1;
        let end_line = end.line.min(self.live_bottom_line());

        let mut out = String::new();
        for line in start.line..=end_line {
            let cells = self.line(line);
            let (first, last) = selected_cols(start, end, line, last_col);
            let mut segment = String::new();
            for col in first..=last {
                match cells.and_then(|r| r.get(col as usize)) {
                    // The wide leader already carries the glyph — drop its spacer.
                    // Leader-validated continuation (D6): orphan continuations are emitted as ' '.
                    Some(c) if c.is_continuation() => {
                        let is_valid_continuation =
                            col > 0
                                && cells.and_then(|r| r.get(col as usize - 1)).is_some_and(
                                    |prev| prev.ch.width() == Some(2) && !prev.is_continuation(),
                                );
                        if !is_valid_continuation {
                            segment.push(' ');
                        }
                    }
                    Some(c) => segment.push(c.ch),
                    None => segment.push(' '),
                }
            }
            out.push_str(segment.trim_end());
            // Join to the next selected line: newline unless this line soft-wraps.
            let wraps = cells.is_some_and(|r| soft_wraps(r, self.cols));
            if line < end_line && !wraps {
                out.push('\n');
            }
        }
        Some(out)
    }

    /// The last `rows` non-blank-terminated rows of the visible viewport as text.
    ///
    /// Same visible-row mapping as [`GridSnapshot::from_grid`](crate::GridSnapshot) — it
    /// honours `view_offset`, so a scrolled-back viewport yields what is actually on
    /// screen — but it clones no cells and walks up from the bottom, stopping as soon as it
    /// has `rows` rows of content. Trailing blank rows (the padding every partly-filled
    /// screen carries) are skipped first, so "the last N rows" means the last N rows that
    /// have something on them.
    ///
    /// Cheap enough to call per PTY chunk, which is the point: callers that only care about
    /// the bottom of the screen should not pay for a full-grid clone.
    pub fn viewport_tail_text(&self, rows: usize) -> String {
        if rows == 0 {
            return String::new();
        }
        let hist = self.history.len();
        let offset = self.view_offset;
        let row_text = |row: u16| {
            // Global row index over [history .. history+rows); offset scrolls up.
            let global = hist as isize - offset as isize + row as isize;
            let mut line = String::with_capacity(self.cols as usize);
            for col in 0..self.cols {
                let cell = if global >= 0 && (global as usize) < hist {
                    self.history_row(global as usize)
                        .and_then(|r| r.get(col as usize).copied())
                } else {
                    let live = (global - hist as isize).max(0) as u16;
                    self.cell(col, live).copied()
                };
                match cell {
                    // Skip a double-width char's trailing spacer, exactly like `to_text`.
                    Some(c) if c.is_continuation() => {}
                    Some(c) => line.push(c.ch),
                    None => line.push(' '),
                }
            }
            while line.ends_with(' ') {
                line.pop();
            }
            line
        };

        let mut collected: Vec<String> = Vec::with_capacity(rows);
        let mut seen_content = false;
        for row in (0..self.rows).rev() {
            let line = row_text(row);
            if !seen_content {
                if line.is_empty() {
                    continue; // trailing padding below the last written row
                }
                seen_content = true;
            }
            collected.push(line);
            if collected.len() >= rows {
                break;
            }
        }
        collected.reverse();
        let mut out = collected.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }

    /// Record the most-recent OSC 133 command-end event (parser → grid). A newer
    /// event overwrites an unconsumed one — only the latest matters (spec D17:
    /// one waiting item per session).
    pub fn record_command_end(&mut self, event: CommandEnd) {
        self.command_end = Some(event);
        self.command_running = false;
    }

    /// Record an OSC 133 command-start marker (parser → grid): the shell is now
    /// running a foreground command until the next command end.
    pub fn record_command_start(&mut self) {
        self.command_running = true;
    }

    /// Whether a foreground command is running — the last OSC 133 marker was a `C`
    /// with no well-formed `D` after it. Not drained by reading.
    pub fn command_running(&self) -> bool {
        self.command_running
    }

    /// Take the most-recent OSC 133 command-end event, clearing it (one-shot
    /// drain). Returns `None` if none is pending.
    pub fn take_command_end(&mut self) -> Option<CommandEnd> {
        self.command_end.take()
    }

    /// Record the most-recent OSC 0 / OSC 2 window title (parser → grid). A newer
    /// title overwrites an unconsumed one — only the latest matters (tab-identity).
    pub fn record_title(&mut self, title: String) {
        self.osc_title = Some(title);
    }

    /// Take the most-recent OSC 0 / OSC 2 window title, clearing it (one-shot drain).
    /// `None` if none arrived since the last take; `Some("")` means the title was
    /// explicitly cleared (revert the manual label to the auto/cwd label).
    pub fn take_osc_title(&mut self) -> Option<String> {
        self.osc_title.take()
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
        // Home through `move_to` so the deferred wrap is cleared too — `CSI 2J`
        // homes the cursor, and an armed `pending_wrap` surviving that made the
        // next character land one row low.
        self.move_to(0, 0);
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

    /// Erase from the cursor to end of the current line (`CSI 0 K`).
    pub fn clear_line_from_cursor(&mut self) {
        if self.cursor.row >= self.rows || self.cols == 0 {
            return;
        }
        let col = self.cursor.col.min(self.cols.saturating_sub(1));
        let start = self.index(col, self.cursor.row);
        let end = self.index(0, self.cursor.row) + self.cols as usize;
        self.blank_span(start, end);
    }

    /// Erase from the start of the line to the cursor **inclusive** (`CSI 1 K`).
    pub fn clear_line_to_cursor(&mut self) {
        if self.cursor.row >= self.rows || self.cols == 0 {
            return;
        }
        let col = self.cursor.col.min(self.cols.saturating_sub(1));
        let start = self.index(0, self.cursor.row);
        let end = self.index(col, self.cursor.row) + 1;
        self.blank_span(start, end);
    }

    /// Erase the entire cursor line (`CSI 2 K`).
    pub fn clear_line(&mut self) {
        if self.cursor.row >= self.rows || self.cols == 0 {
            return;
        }
        let start = self.index(0, self.cursor.row);
        let end = start + self.cols as usize;
        self.blank_span(start, end);
    }

    /// Blank a raw cell span with the **default** colors, then repair any wide-char
    /// pair the span's edges cut in half. Shared by the three EL variants, which
    /// keep default-background fill by D6 (Known Gap G-A).
    fn blank_span(&mut self, start: usize, end: usize) {
        let end = end.min(self.cells.len());
        if start >= end {
            return;
        }
        let blank = Cell::blank(self.default_fg, self.default_bg);
        for cell in &mut self.cells[start..end] {
            *cell = blank;
        }
        let row = self.cursor.row;
        self.repair_wide_pairs(row, blank);
    }

    // --- In-line character operations (vt-tui-completeness R5) ---

    /// ICH (`CSI n @`) — insert `n` blank cells at the cursor, shifting the rest of
    /// the line right. Cells pushed past the right edge are discarded. The cursor
    /// does not move (D12).
    pub fn insert_chars(&mut self, n: u16) {
        let (col, row) = (self.cursor.col, self.cursor.row);
        let cols = self.cols;
        let n = n.max(1).min(cols - col);
        let base = self.index(0, row);
        let src = base + col as usize;
        let dst = src + n as usize;
        let end = base + cols as usize;
        if dst < end {
            self.cells.copy_within(src..end - n as usize, dst);
        }
        let fill = self.pen_blank();
        for cell in &mut self.cells[src..(src + n as usize).min(end)] {
            *cell = fill;
        }
        self.repair_wide_pairs(row, fill);
    }

    /// DCH (`CSI n P`) — delete `n` cells at the cursor, pulling the line's tail
    /// left and blanking in at the right edge. The cursor does not move (D12).
    pub fn delete_chars(&mut self, n: u16) {
        let (col, row) = (self.cursor.col, self.cursor.row);
        let cols = self.cols;
        let n = n.max(1).min(cols - col);
        let base = self.index(0, row);
        let src = base + col as usize;
        let end = base + cols as usize;
        if src + (n as usize) < end {
            self.cells.copy_within(src + n as usize..end, src);
        }
        let fill = self.pen_blank();
        for cell in &mut self.cells[end - n as usize..end] {
            *cell = fill;
        }
        self.repair_wide_pairs(row, fill);
    }

    /// ECH (`CSI n X`) — overwrite `n` cells from the cursor with blanks. Unlike
    /// DCH this does **not** shift: the tail of the line stays where it is (D12).
    pub fn erase_chars(&mut self, n: u16) {
        let (col, row) = (self.cursor.col, self.cursor.row);
        let n = n.max(1).min(self.cols - col);
        let start = self.index(col, row);
        let fill = self.pen_blank();
        for cell in &mut self.cells[start..start + n as usize] {
            *cell = fill;
        }
        self.repair_wide_pairs(row, fill);
    }

    /// Blank the partner of the double-width pair occupying `(col, row)`, if the
    /// cell is half of one. The constant-time counterpart to
    /// [`repair_wide_pairs`](Grid::repair_wide_pairs), used by `print` before it
    /// overwrites a cell so a pair is never left with one half orphaned.
    pub fn break_wide_pair_at(&mut self, col: u16, row: u16) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let idx = self.index(col, row);
        let fill = self.pen_blank();
        if self.cells[idx].is_continuation() {
            // Spacer here → its leader is the cell to the left.
            if col > 0
                && self.cells[idx - 1].ch.width().unwrap_or(0) == 2
                && !self.cells[idx - 1].is_continuation()
            {
                self.cells[idx - 1] = fill;
            }
        } else if self.cells[idx].ch.width().unwrap_or(0) == 2
            && !self.cells[idx].is_continuation()
            && col + 1 < self.cols
            && self.cells[idx + 1].is_continuation()
        {
            // Leader here → its spacer is the cell to the right.
            self.cells[idx + 1] = fill;
        }
    }

    /// Repair double-width pairs that a shift or blank cut in half (D4/R5.6/R5.7).
    ///
    /// `print` writes a wide char as a leader plus a [`Cell::CONTINUATION`] spacer.
    /// A raw `copy_within` happily splits that pair, after which the renderer either
    /// draws a stray spacer or paints a double-width glyph over its neighbour's
    /// cell — the wide-char corruption class the `wide-chars` work already paid to
    /// fix. One pass over the mutated row restores the invariant: every spacer has a
    /// wide leader to its left, and every wide leader has its spacer to its right.
    pub fn repair_wide_pairs(&mut self, row: u16, fill: Cell) {
        if row >= self.rows {
            return;
        }
        let base = self.index(0, row);
        let cols = self.cols as usize;
        for i in 0..cols {
            let idx = base + i;
            if self.cells[idx].is_continuation() {
                // A spacer is only legal directly after a wide leader.
                let orphan = i == 0
                    || self.cells[idx - 1].ch.width().unwrap_or(0) != 2
                    || self.cells[idx - 1].is_continuation();
                if orphan {
                    self.cells[idx] = fill;
                }
            } else if self.cells[idx].ch.width().unwrap_or(0) == 2
                && !self.cells[idx].is_continuation()
            {
                // A wide leader is only legal when its spacer survived.
                let orphan = i + 1 >= cols || !self.cells[base + i + 1].is_continuation();
                if orphan {
                    self.cells[idx] = fill;
                }
            }
        }
    }

    /// Erase from the cursor (inclusive) to the end of the display (CSI 0J).
    /// Does NOT move the cursor or touch rows above it — this is the erase a shell
    /// sends (`\r ESC[J`) when redrawing its prompt, so it must preserve scrollback.
    pub fn clear_from_cursor(&mut self) {
        if self.cursor.row >= self.rows || self.cols == 0 {
            return;
        }
        let col = self.cursor.col.min(self.cols.saturating_sub(1));
        let row = self.cursor.row;
        let start = self.index(col, row);
        let blank = Cell::blank(self.default_fg, self.default_bg);
        for cell in &mut self.cells[start..] {
            *cell = blank;
        }
        self.repair_wide_pairs(row, blank);
    }

    /// Erase from the start of the display to the cursor (inclusive) (CSI 1J).
    pub fn clear_to_cursor(&mut self) {
        if self.cursor.row >= self.rows || self.cols == 0 {
            return;
        }
        let col = self.cursor.col.min(self.cols.saturating_sub(1));
        let row = self.cursor.row;
        let end = (self.index(col, row) + 1).min(self.cells.len());
        let blank = Cell::blank(self.default_fg, self.default_bg);
        for cell in &mut self.cells[..end] {
            *cell = blank;
        }
        self.repair_wide_pairs(row, blank);
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
        // A selection is dimension-bound: with no rewrap (architecture G4), a line's
        // columns mean something else at the new width, so a resize clears it
        // (selection-model D9, kept by selection-scroll-anchor D7/R7.1).
        self.selection = None;
        // A deferred wrap is dimension-bound too: the last column just moved, so a
        // retained flag would fire a stray wrap on a cursor no longer at the margin
        // (D8/R7.4).
        self.pending_wrap = false;
        // A scroll region is dimension-bound for the same reason — its bottom may
        // no longer exist. Drop it rather than clamp: the app gets SIGWINCH and
        // re-sends `CSI r` on its next frame (vt-tui-completeness D7/R4.4). Runs
        // after `self.rows` is updated so the new full-screen bottom is correct.
        self.reset_scroll_region();
    }

    /// Iterate cells in row-major order. Used to build a [`crate::GridSnapshot`].
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }
}

/// The inclusive `(first, last)` columns of `line` that the linear span
/// `start..=end` (in reading order) selects: from `start.col` on the start line, up
/// to `end.col` on the end line, the full width on every line between — each
/// clamped to `last_col`.
///
/// One rule for "which cells are selected" (selection-scroll-anchor D4/D6): the
/// extractor reads exactly these cells, and the change reference records exactly
/// these, so a change the copy would show is a change the check sees.
fn selected_cols(start: Point, end: Point, line: u64, last_col: u16) -> (u16, u16) {
    let first = if line == start.line { start.col } else { 0 };
    let last = if line == end.line { end.col } else { last_col };
    (first.min(last_col), last.min(last_col))
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
    fn the_reference_never_exceeds_one_screen() {
        // selection-scroll-anchor R4.9 — a selection from 50 lines back in history
        // to the bottom live row references only the three live lines, full width:
        // 10 × 3 cells, however far into history it reaches.
        let mut g = grid(10, 3);
        let mut p = Processor::new();
        for i in 0..60 {
            p.advance(&mut g, format!("line{i}\r\n").as_bytes());
        }
        let live_top = g.point_at(0, 0).line;
        assert!(live_top >= 50, "precondition: 50 lines of history");
        g.set_selection(
            Point {
                line: live_top - 50,
                col: 0,
            },
            g.point_at(9, 2),
        );
        let reference = &g.selection.as_ref().expect("set").reference;
        let cells: usize = reference.iter().map(|r| r.chars.len()).sum();
        assert!(cells <= 10 * 3, "bounded by one screen: {cells}");
        let lines: Vec<u64> = reference.iter().map(|r| r.line).collect();
        assert_eq!(
            lines,
            vec![live_top, live_top + 1, live_top + 2],
            "the live lines, and only those"
        );
        assert_eq!(cells, 10 * 3, "each at full width");
    }

    #[test]
    fn the_reference_records_exactly_the_selected_cells() {
        // selection-scroll-anchor D4 — the columns the copy would read: from the
        // start column on the start line, up to the end column on the end line,
        // continuation spacers included. Taken while scrolled back too: "on the live
        // screen" does not depend on the viewport.
        fn reference_of(g: &Grid) -> Vec<(u64, u16, String)> {
            let sel = g.selection.as_ref().expect("set");
            sel.reference
                .iter()
                .map(|r| (r.line, r.first_col, r.chars.iter().collect()))
                .collect()
        }
        let cont = Cell::CONTINUATION;
        let mut g = grid(6, 3);
        let mut p = Processor::new();
        // `h0` scrolls into history as line 0; live lines 1–3 read `x`, `ab한cd`,
        // `efgh`.
        p.advance(&mut g, "h0\r\nx\r\nab한cd\r\nefgh".as_bytes());
        assert_eq!(g.history_len(), 1, "precondition");

        // At the live bottom: line 2 from col 1, line 3 up to col 2.
        g.set_selection(g.point_at(1, 1), g.point_at(2, 2));
        assert_eq!(
            reference_of(&g),
            vec![(2, 1, format!("b한{cont}cd")), (3, 0, "efg".to_string())]
        );

        // Scrolled back by one, viewport row 0 is history line 0: the reference
        // still starts at the first LIVE line, not at the viewport's first row.
        g.scroll_up(1);
        g.set_selection(g.point_at(0, 0), g.point_at(3, 2));
        assert_eq!(
            reference_of(&g),
            vec![(1, 0, "x     ".to_string()), (2, 0, format!("ab한{cont}"))]
        );
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
