//! `term-core` — the headless terminal engine.
//!
//! Owns the VT parser (`vte`), the cell grid, scrollback, and the local PTY.
//! Has **no** dependency on wgpu or winit (constraint C4): every type here is
//! testable without a GPU or a window. The renderer consumes only
//! [`GridSnapshot`], the versioned boundary type.
//!
//! ```
//! use term_core::Terminal;
//! let mut term = Terminal::new(20, 3, term_core::Color(0xE0E0E0), term_core::Color(0x101010));
//! term.advance(b"hi\r\nthere");
//! let snap = term.snapshot();
//! assert_eq!(snap.to_text().lines().next(), Some("hi"));
//! ```

pub mod error;
pub mod grid;
pub mod image;
pub mod links;
pub mod parser;
pub mod pty;
pub mod scrollback;
pub mod selection;
mod shell_integration;
pub mod snapshot;
pub mod testing;

pub use error::{Error, Result};
pub use grid::{
    Cell, CellAttrs, Color, CommandEnd, Cursor, Grid, MouseEncoding, MouseProtocol, SYNC_STALE,
};
pub use image::{Decoded, ImageLayer, Placement};
pub use links::{link_at, LinkSpan};
pub use pty::Pty;
pub use scrollback::History;
pub use selection::{ContentSelection, Point, Selection};
pub use snapshot::GridSnapshot;

use parser::Processor;

/// The terminal engine: a grid plus the VT parser that writes into it.
///
/// This is the headless facade the app drives — feed it PTY bytes with
/// [`advance`](Terminal::advance) and read frames with
/// [`snapshot`](Terminal::snapshot).
pub struct Terminal {
    grid: Grid,
    processor: Processor,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, fg: Color, bg: Color) -> Self {
        Terminal {
            grid: Grid::new(cols, rows, fg, bg),
            processor: Processor::new(),
        }
    }

    /// A terminal whose scrollback keeps at most `limit` rows — **test-only**, so a
    /// test can evict history in a few lines (selection-scroll-anchor R1.4/R4.5).
    /// `#[cfg(test)]`, so it is reachable from this crate's unit tests only, not
    /// from `tests/`.
    #[cfg(test)]
    pub(crate) fn with_history_limit(
        cols: u16,
        rows: u16,
        fg: Color,
        bg: Color,
        limit: usize,
    ) -> Self {
        Terminal {
            grid: Grid::with_history_limit(cols, rows, fg, bg, limit),
            processor: Processor::new(),
        }
    }

    /// Feed a chunk of PTY output through the parser into the grid.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.grid, bytes);
    }

    /// Capture the current visible grid as an immutable snapshot (the renderer
    /// contract).
    pub fn snapshot(&self) -> GridSnapshot {
        GridSnapshot::from_grid(&self.grid)
    }

    /// Resize the grid (call after a window resize; also resize the PTY).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.grid.resize(cols, rows);
    }

    /// Re-theme default cell colors live (settings feature).
    pub fn set_default_colors(&mut self, fg: Color, bg: Color) {
        self.grid.set_default_colors(fg, bg);
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.grid.cols(), self.grid.rows())
    }

    // --- Scrollback viewport controls (D2/D4) — display-only, never write the PTY ---

    /// Scroll the viewport up `n` lines into retained history.
    pub fn scroll_up(&mut self, n: usize) {
        self.grid.scroll_up(n);
    }

    /// Scroll the viewport back down `n` lines toward the live view.
    pub fn scroll_down(&mut self, n: usize) {
        self.grid.scroll_down(n);
    }

    /// Jump the viewport to the live bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.grid.scroll_to_bottom();
    }

    /// Current scrollback offset (0 == live bottom).
    pub fn view_offset(&self) -> usize {
        self.grid.view_offset()
    }

    /// Whether the viewport is pinned to the live bottom.
    pub fn at_bottom(&self) -> bool {
        self.grid.at_bottom()
    }

    /// Whether the viewport is at the oldest retained row.
    pub fn at_top(&self) -> bool {
        self.grid.at_top()
    }

    /// Number of rows currently retained in scrollback history.
    pub fn history_len(&self) -> usize {
        self.grid.history_len()
    }

    /// Rows that have aged out of the scrollback cap since this terminal was
    /// created (selection-scroll-anchor D1). Monotonic. Together with
    /// [`history_len`](Terminal::history_len) it fixes the content line numbering:
    /// history row `i` is line `evicted() + i`, live row `r` is line
    /// `evicted() + history_len() + r`.
    pub fn evicted(&self) -> u64 {
        self.grid.evicted()
    }

    /// Whether the alternate screen is active (`?1049h`).
    pub fn alt_screen(&self) -> bool {
        self.grid.alt_screen()
    }

    /// Whether DECCKM — application cursor key mode (`?1h`) — is active.
    ///
    /// The app reads this to choose the SS3 arrow encoding (`ESC O A`) over the
    /// normal CSI one (`ESC [ A`). It is exposed as an accessor rather than a
    /// [`GridSnapshot`] field because the renderer never reads it (arrow-key-input
    /// D2); the snapshot is the *renderer* contract.
    pub fn app_cursor_keys(&self) -> bool {
        self.grid.app_cursor_keys()
    }

    /// Whether bracketed-paste mode (DECSET `?2004`) is active. The app reads this
    /// to wrap a paste in `ESC[200~`/`ESC[201~` (text-paste). Exposed as an accessor
    /// rather than a [`GridSnapshot`] field for the same reason as
    /// [`app_cursor_keys`](Terminal::app_cursor_keys): the renderer never reads it.
    pub fn bracketed_paste(&self) -> bool {
        self.grid.bracketed_paste()
    }

    /// Which mouse events the running application asked for (`?9`/`?1000`/`?1002`/
    /// `?1003`). The app reads this to decide whether a wheel gesture becomes a
    /// mouse report for the application or a scrollback move for the terminal.
    pub fn mouse_protocol(&self) -> MouseProtocol {
        self.grid.mouse_protocol()
    }

    /// How mouse reports should be encoded (`?1006` → SGR, else X10).
    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.grid.mouse_encoding()
    }

    /// The active DECSTBM scroll region as an inclusive 0-based `(top, bottom)`
    /// row pair (vt-tui-completeness R1). A full-screen region is `(0, rows - 1)`.
    pub fn scroll_region(&self) -> (u16, u16) {
        self.grid.scroll_region()
    }

    /// Whether DECAWM autowrap (`?7h`) is on.
    pub fn autowrap(&self) -> bool {
        self.grid.autowrap()
    }

    /// Whether the last printed character landed in the final column and left its
    /// wrap deferred (vt-tui-completeness R7.1).
    ///
    /// An accessor rather than a [`GridSnapshot`] field, for the same reason as
    /// [`app_cursor_keys`](Terminal::app_cursor_keys): the renderer never reads it,
    /// and the snapshot is the *renderer* contract.
    pub fn pending_wrap(&self) -> bool {
        self.grid.pending_wrap()
    }

    /// The content [`Point`] under viewport cell `(col, row)` at the current
    /// scrollback offset (selection-scroll-anchor D3): line `top + row`, where `top`
    /// is the line on viewport row 0, and the same column.
    ///
    /// Convert a cell **when the app learns it** — the press anchor at press time,
    /// the focus at move time — because the content under a cell moves with output
    /// and with the wheel. Unclamped: a cell outside the viewport maps to a line
    /// outside it.
    pub fn point_at(&self, col: u16, row: u16) -> Point {
        self.grid.point_at(col, row)
    }

    /// Set the drag selection from two content points (the app drives this from
    /// the mouse — see the `selection-input` feature). `anchor` is where the drag
    /// began, `focus` its current end, in either order. Build both with
    /// [`point_at`](Terminal::point_at).
    ///
    /// Anchored to content, so it moves with its text through terminal-owned
    /// scrolling. The renderer sees it projected into viewport cells on
    /// [`snapshot`](Terminal::snapshot)'s `selection`; its text is read with
    /// [`selection_text`](Terminal::selection_text).
    pub fn set_selection(&mut self, anchor: Point, focus: Point) {
        self.grid.set_selection(anchor, focus);
    }

    /// Clear any drag selection.
    pub fn clear_selection(&mut self) {
        self.grid.clear_selection();
    }

    /// The stored drag selection — the two content points as last set — if any.
    pub fn selection(&self) -> Option<ContentSelection> {
        self.grid.selection()
    }

    /// The selection's text, read from scrollback + live by line number, or `None`
    /// when there is no selection **or its text has changed** (selection-scroll-anchor
    /// D6).
    ///
    /// This is what Cmd+C copies. It returns the **whole** selection, including any
    /// part that has scrolled off-screen, which a [`GridSnapshot`] — the visible
    /// rows only — cannot. Lines join with `\n` except across a soft wrap, by the
    /// same rule hyperlinks use.
    ///
    /// It re-verifies before reading: if the selection's start has aged out of
    /// scrollback, or a selected cell that was on the live screen when the selection
    /// was set now holds a different character, it returns `None` — read-only, and
    /// without waiting for [`settle_selection`](Terminal::settle_selection). So a copy
    /// never returns text the user did not select, even while a stale highlight is
    /// still drawn.
    pub fn selection_text(&self) -> Option<String> {
        self.grid.selection_text()
    }

    /// Run the selection change check at the end of a drain
    /// (selection-scroll-anchor D4/D5): clear the selection if the text under it
    /// has changed since it was set. Call it **once after** feeding a batch of PTY
    /// chunks through [`advance`](Terminal::advance) — never per chunk, because a
    /// frame can arrive split across reads and a per-chunk check would see it
    /// half-painted. `now` is the caller's clock (the app passes `Instant::now()`,
    /// tests pass fixed instants); `term-core` keeps none.
    ///
    /// While an app holds a synchronized update open (`?2026h`), the check is left
    /// to that block's `?2026l`, which runs it at the exact frame end. A block still
    /// open [`SYNC_STALE`] after the first settle that saw it is dropped and checked
    /// here, so an app that dies mid-frame cannot defer the check forever.
    ///
    /// Moves that keep the text — output scrolling it into history, the wheel —
    /// never clear; an app repainting different characters over it does.
    pub fn settle_selection(&mut self, now: std::time::Instant) {
        self.grid.settle_selection(now);
    }

    /// Take the most-recent OSC 133 command-end event parsed from the shell's
    /// output, clearing it (one-shot drain). Poll it right after
    /// [`advance`](Terminal::advance); `None` if no marker has arrived.
    ///
    /// Exposed as an accessor rather than a [`GridSnapshot`] field for the same
    /// reason as [`app_cursor_keys`](Terminal::app_cursor_keys): the renderer
    /// never reads it. The event is a raw shell fact ([`CommandEnd`]) — the `app`
    /// keys it to a session and owns every UI policy (the ≥30s completion
    /// threshold, the `{1, 130}` error ignore-list). `term-core` stays free of
    /// UI/wgpu/winit concerns (spec D21).
    pub fn take_command_end(&mut self) -> Option<CommandEnd> {
        self.grid.take_command_end()
    }

    /// Whether the shell is running a foreground command: the last OSC 133 marker was
    /// a command start with no well-formed command end after it (sidebar-weather-cards
    /// D8). A state read, not a drain — the app mirrors it every `advance`. Like
    /// [`take_command_end`](Terminal::take_command_end) it is a raw shell fact; what
    /// "running" means for the UI is the app's policy.
    pub fn command_running(&self) -> bool {
        self.grid.command_running()
    }

    /// Take the most-recent OSC 0 / OSC 2 window title set since the last take, clearing
    /// it (one-shot drain). Poll it right after [`advance`](Terminal::advance). `None` if
    /// no title arrived; `Some("")` when the title was explicitly cleared (the app then
    /// reverts the tab's manual label to its auto/cwd label). Like
    /// [`take_command_end`](Terminal::take_command_end), it is an accessor, not a
    /// [`GridSnapshot`] field — the renderer never reads it; the `app` owns the tab label.
    pub fn take_osc_title(&mut self) -> Option<String> {
        self.grid.take_osc_title()
    }

    /// The last `rows` **non-blank-terminated** rows of the visible viewport as text, one
    /// row per line, trailing blank rows dropped.
    ///
    /// A cheap alternative to `snapshot().to_text()` for callers that only need the bottom
    /// of the screen: it clones no cells and builds a string bounded by `rows * cols`,
    /// so it is safe to call on every PTY chunk. A viewport is padded with blank rows to
    /// the full grid height, so those are trimmed first — otherwise "the last N rows" of a
    /// half-full screen is mostly padding.
    ///
    /// Raw text only. What any of it *means* is the caller's business (spec D21).
    pub fn viewport_tail_text(&self, rows: usize) -> String {
        self.grid.viewport_tail_text(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_tail_text_trims_padding_and_bounds_rows() {
        // A 20-row viewport with 3 written rows: the tail must be those 3, not 17 blanks
        // plus them. This is the property the caller depends on — a dialog that ends well
        // above the last row would otherwise fall outside any "last N rows" window.
        let mut t = Terminal::new(30, 20, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"alpha\r\nbeta\r\ngamma\r\n");
        assert_eq!(t.viewport_tail_text(10), "alpha\nbeta\ngamma\n");
        // Bounded: asking for fewer rows returns the LAST ones.
        assert_eq!(t.viewport_tail_text(2), "beta\ngamma\n");
        assert_eq!(t.viewport_tail_text(1), "gamma\n");
    }

    #[test]
    fn viewport_tail_text_is_empty_on_a_blank_screen() {
        let t = Terminal::new(30, 10, Color(0xE0E0E0), Color(0x101010));
        assert_eq!(t.viewport_tail_text(5), "");
    }

    #[test]
    fn viewport_tail_text_follows_the_scrollback_viewport() {
        // Honours `view_offset` like `GridSnapshot::from_grid`: scrolled back, it reports
        // what is on screen, not the live bottom.
        let mut t = Terminal::new(30, 4, Color(0xE0E0E0), Color(0x101010));
        for i in 0..12 {
            t.advance(format!("line{i}\r\n").as_bytes());
        }
        let live = t.viewport_tail_text(4);
        assert!(
            live.contains("line11"),
            "live view shows the newest: {live:?}"
        );
        t.scroll_up(6);
        let scrolled = t.viewport_tail_text(4);
        assert!(
            !scrolled.contains("line11"),
            "scrolled view must not show the live bottom: {scrolled:?}"
        );
        assert!(scrolled.contains("line5"), "shows history: {scrolled:?}");
    }

    #[test]
    fn take_command_end_exposes_osc133_and_drains() {
        let mut t = Terminal::new(20, 5, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"\x1b]133;D;101;40000\x07");
        let ev = t.take_command_end().expect("accessor returns the event");
        assert_eq!(ev.exit_code, 101);
        assert_eq!(ev.elapsed_ms, 40000);
        assert!(t.take_command_end().is_none(), "one-shot: drains on take");
    }

    // --- OSC 133 command-start → running state (sidebar-weather-cards D8) ---

    fn term() -> Terminal {
        Terminal::new(20, 5, Color(0xE0E0E0), Color(0x101010))
    }

    #[test]
    fn command_start_sets_running_and_end_clears_it() {
        // R8.1 — `C` sets the state, a well-formed `D` clears it, and the command-end
        // one-shot the waiting band consumes still fires exactly once.
        let mut t = term();
        assert!(!t.command_running(), "a fresh shell runs nothing");
        t.advance(b"\x1b]133;C\x07");
        assert!(t.command_running(), "C → running");
        assert!(t.command_running(), "reading is not a drain");
        t.advance(b"\x1b]133;D;0;5\x07");
        assert!(!t.command_running(), "D → not running");
        let ev = t.take_command_end().expect("the one-shot still fires");
        assert_eq!((ev.exit_code, ev.elapsed_ms), (0, 5));
        assert!(t.take_command_end().is_none(), "and only once");
    }

    #[test]
    fn only_the_final_marker_in_a_chunk_counts() {
        // R8.2 — a fast command puts C then D in one read; typeahead can put the old
        // D then the new C in one read. Byte order decides, not the drain.
        let mut t = term();
        t.advance(b"\x1b]133;C\x07file.txt\r\n\x1b]133;D;0;3\x07");
        assert!(!t.command_running(), "C…D in one chunk → finished");
        let mut t = term();
        t.advance(b"\x1b]133;D;101;40\x07$ \x1b]133;C\x07");
        assert!(
            t.command_running(),
            "D…C in one chunk → the new command runs"
        );
    }

    #[test]
    fn a_malformed_end_leaves_the_running_state_untouched() {
        // R8.3 — the parser's drop-the-whole-event contract covers the state too.
        let mut t = term();
        t.advance(b"\x1b]133;C\x07");
        t.advance(b"\x1b]133;D;notanumber;5\x07");
        assert!(t.command_running(), "a garbage D is not an end");
        assert!(t.take_command_end().is_none(), "and records no event");
        t.advance(b"\x1b]133;D;7\x07"); // missing elapsed
        assert!(t.command_running(), "nor is a partial one");
        t.advance(b"\x1b]133;A\x07"); // another subkind (prompt start)
        assert!(t.command_running(), "unknown subkinds do not move it");
    }

    #[test]
    fn malformed_osc133_yields_no_event() {
        let mut t = Terminal::new(20, 5, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"\x1b]133;D;notanumber\x07");
        assert!(
            t.take_command_end().is_none(),
            "garbage → no event, no panic"
        );
    }

    // --- Content lines through eviction (selection-scroll-anchor D1) ---
    //
    // These need `with_history_limit`, which is `#[cfg(test)]`, so they live here
    // rather than in `tests/selection.rs`.

    /// The text of content line `line`, or `None` once it is evicted or below the
    /// live bottom — read straight from the grid's line lookup.
    fn line_of(t: &Terminal, line: u64) -> Option<String> {
        let cells = t.grid.line(line)?;
        let text: String = cells
            .iter()
            .filter(|c| !c.is_continuation())
            .map(|c| c.ch)
            .collect();
        Some(text.trim_end().to_string())
    }

    #[test]
    fn eviction_keeps_line_numbers_on_their_text() {
        // selection-scroll-anchor R1.4 — history limit 2. Line numbers are
        // recorded while retained; each eviction grows `evicted` by exactly one and
        // leaves every retained line reading the same text.
        let mut t = Terminal::with_history_limit(10, 2, Color(0xE0E0E0), Color(0x101010), 2);
        t.advance(b"l0\r\nl1\r\nl2\r\nl3"); // history [l0, l1] (full), live [l2, l3]
        assert_eq!((t.evicted(), t.history_len()), (0, 2));
        assert_eq!(line_of(&t, 1).as_deref(), Some("l1"));

        t.advance(b"\r\nl4"); // l2 enters history, l0 is evicted
        assert_eq!(t.evicted(), 1, "one row evicted");
        assert_eq!(
            line_of(&t, 0),
            None,
            "the evicted line is gone, not re-pointed"
        );
        assert_eq!(
            line_of(&t, 1).as_deref(),
            Some("l1"),
            "L still reads its text"
        );
        assert_eq!(
            line_of(&t, 2).as_deref(),
            Some("l2"),
            "entered history unchanged"
        );
        assert_eq!(line_of(&t, 4).as_deref(), Some("l4"), "a live line");
        assert_eq!(line_of(&t, 5), None, "below the live bottom");

        t.advance(b"\r\nl5\r\nl6"); // two more evictions: l1, l2
        assert_eq!(t.evicted(), 3, "grew by exactly the rows evicted");
        assert_eq!(line_of(&t, 2), None);
        assert_eq!(line_of(&t, 3).as_deref(), Some("l3"));
        assert_eq!(line_of(&t, 6).as_deref(), Some("l6"));
    }

    #[test]
    fn point_at_counts_evicted_rows() {
        // selection-scroll-anchor R1.2 with eviction under way: live row r is line
        // `evicted + history_len + r`, and scrolled back the same line stays on the
        // same text.
        let mut t = Terminal::with_history_limit(10, 3, Color(0xE0E0E0), Color(0x101010), 2);
        t.advance(b"l0\r\nl1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6");
        let (e, h) = (t.evicted(), t.history_len() as u64);
        assert_eq!((e, h), (2, 2), "precondition: eviction under way");
        assert_eq!(
            t.point_at(4, 1),
            Point {
                line: e + h + 1,
                col: 4
            }
        );
        assert_eq!(line_of(&t, e + h + 1).as_deref(), Some("l5"));
        t.scroll_up(2);
        assert_eq!(
            t.point_at(0, 0),
            Point { line: e, col: 0 },
            "oldest retained"
        );
        assert_eq!(line_of(&t, e).as_deref(), Some("l2"));
    }

    // --- The selection change check (selection-scroll-anchor D4/D5/D6) ---
    //
    // Here rather than in `tests/selection_check.rs` because each needs the
    // test-only history limit or reads the sync flag, both crate-internal.

    use std::time::{Duration, Instant};

    /// `alpha` / `bravo` / `charlie` on a 10×3 terminal, `bravo` selected.
    fn bravo_selected() -> Terminal {
        let mut t = Terminal::new(10, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"alpha\r\nbravo\r\ncharlie");
        let (a, f) = (t.point_at(0, 1), t.point_at(4, 1));
        t.set_selection(a, f);
        assert_eq!(t.selection_text().as_deref(), Some("bravo"), "precondition");
        t
    }

    /// History limit 2, `l0`/`l1` in history (line 0 is the oldest retained) and
    /// `l2`/`l3` live.
    fn full_history() -> Terminal {
        let mut t = Terminal::with_history_limit(10, 2, Color(0xE0E0E0), Color(0x101010), 2);
        t.advance(b"l0\r\nl1\r\nl2\r\nl3");
        assert_eq!((t.evicted(), t.history_len()), (0, 2), "precondition");
        t
    }

    #[test]
    fn evicting_the_start_of_a_backward_selection_clears_it() {
        // selection-scroll-anchor R4.5 — anchor on the later line, focus on the
        // earlier one, both in history (so the reference is empty and only the
        // eviction test can fire). The start — the earlier point, not the anchor —
        // is what eviction is measured against.
        let mut t = full_history();
        t.set_selection(Point { line: 1, col: 1 }, Point { line: 0, col: 0 });
        t.settle_selection(Instant::now());
        assert!(
            t.selection().is_some(),
            "the start is the oldest retained line, not yet evicted"
        );
        assert_eq!(t.selection_text().as_deref(), Some("l0\nl1"));

        t.advance(b"\r\nl4"); // l2 enters history, l0 is evicted
        assert_eq!(t.evicted(), 1, "precondition: the start line is gone");
        t.settle_selection(Instant::now());
        assert_eq!(t.selection(), None);
    }

    #[test]
    fn copy_refuses_an_evicted_selection_before_any_settle() {
        // selection-scroll-anchor R6.5 — no settle since the eviction, so the
        // selection is still stored; the copy re-verifies on its own.
        let mut t = full_history();
        t.set_selection(Point { line: 0, col: 0 }, Point { line: 1, col: 1 });
        assert_eq!(
            t.selection_text().as_deref(),
            Some("l0\nl1"),
            "precondition"
        );
        t.advance(b"\r\nl4");
        assert!(t.selection().is_some(), "no check has run");
        assert_eq!(t.selection_text(), None);
    }

    #[test]
    fn an_unclosed_block_stops_deferring_one_second_after_it_was_first_seen() {
        // selection-scroll-anchor R5.6 — the first settle stamps the block, and the
        // bound is inclusive: 999 ms keeps deferring, 1000 ms checks.
        let mut t = bravo_selected();
        t.advance(b"\x1b[?2026h\x1b[2;1Hx");
        let t0 = Instant::now();
        t.settle_selection(t0);
        assert!(t.selection().is_some(), "first settle only stamps");
        t.settle_selection(t0 + Duration::from_millis(999));
        assert!(t.selection().is_some(), "999 ms: still deferring");
        t.settle_selection(t0 + Duration::from_millis(1000));
        assert_eq!(t.selection(), None, "1000 ms: stale, checked");
        assert!(!t.grid.synchronized_update(), "and the block is dropped");
        assert_eq!(SYNC_STALE, Duration::from_secs(1));
    }

    #[test]
    fn the_first_drain_well_past_the_bound_checks_too() {
        // selection-scroll-anchor D5 — "the first drain at least one second after":
        // output that pauses for 5 s after the stamp still gets its check.
        let mut t = bravo_selected();
        t.advance(b"\x1b[?2026h\x1b[2;1Hx");
        let t0 = Instant::now();
        t.settle_selection(t0);
        t.settle_selection(t0 + Duration::from_secs(5));
        assert_eq!(t.selection(), None);
    }

    #[test]
    fn a_repeated_open_does_not_move_the_stamp_forward() {
        // selection-scroll-anchor D5 — a second `?2026h` inside an open block must
        // not restart the clock, or an app repeating it could defer forever.
        let mut t = bravo_selected();
        t.advance(b"\x1b[?2026h\x1b[2;1Hx");
        let t0 = Instant::now();
        t.settle_selection(t0);
        t.advance(b"\x1b[?2026h");
        t.settle_selection(t0 + Duration::from_millis(1000));
        assert_eq!(t.selection(), None, "timed from the first stamp");
    }

    #[test]
    fn a_stale_block_whose_text_held_keeps_the_selection_and_stops_deferring() {
        // selection-scroll-anchor D5 — the timeout drops the block and checks; a
        // selection whose text did not change survives it, and the very next
        // settle checks again instead of re-stamping.
        let mut t = bravo_selected();
        t.advance(b"\x1b[?2026h");
        let t0 = Instant::now();
        t.settle_selection(t0);
        t.settle_selection(t0 + SYNC_STALE);
        assert!(
            t.selection().is_some(),
            "unchanged text survives the timeout"
        );
        assert!(!t.grid.synchronized_update());
        t.advance(b"\x1b[2;1Hx");
        t.settle_selection(t0 + SYNC_STALE);
        assert_eq!(t.selection(), None, "no block left to defer this drain");
    }

    #[test]
    fn ris_and_leaving_the_alt_screen_close_an_open_block() {
        // selection-scroll-anchor R5.7 — in two terminals, each with a changed
        // selection inside an open block. Neither close checks on its own (D7);
        // the next settle does, without waiting for the stale bound.
        let mut ris = bravo_selected();
        ris.advance(b"\x1b[?2026h\x1b[2;1Hx");
        assert!(ris.grid.synchronized_update(), "precondition: open");
        ris.advance(b"\x1bc");
        assert!(!ris.grid.synchronized_update(), "RIS closes it");
        assert!(ris.selection().is_some(), "RIS runs no check of its own");
        ris.settle_selection(Instant::now());
        assert_eq!(ris.selection(), None, "the first settle checks");

        let mut alt = bravo_selected();
        alt.advance(b"\x1b[?2026h\x1b[2;1Hx");
        alt.advance(b"\x1b[?1049h");
        assert!(
            alt.grid.synchronized_update(),
            "entering the alt screen leaves it open"
        );
        alt.advance(b"\x1b[?1049l");
        assert!(!alt.grid.synchronized_update(), "leaving closes it");
        assert!(
            alt.selection().is_some(),
            "leaving runs no check of its own"
        );
        alt.settle_selection(Instant::now());
        assert_eq!(alt.selection(), None, "the first settle checks");
    }

    #[test]
    fn take_osc_title_exposes_osc2_and_drains() {
        // tab-identity manual label — OSC 2 (and OSC 0) set the title; one-shot drain.
        let mut t = Terminal::new(20, 5, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"\x1b]2;Frontend\x07");
        assert_eq!(t.take_osc_title().as_deref(), Some("Frontend"));
        assert!(t.take_osc_title().is_none(), "one-shot: drains on take");
        // OSC 0 also sets the title.
        t.advance(b"\x1b]0;Backend\x07");
        assert_eq!(t.take_osc_title().as_deref(), Some("Backend"));
        // An empty title is a valid value — it clears the manual label.
        t.advance(b"\x1b]2;\x07");
        assert_eq!(t.take_osc_title().as_deref(), Some(""));
    }
}
