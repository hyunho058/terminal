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
pub mod parser;
pub mod pty;
pub mod scrollback;
pub mod selection;
mod shell_integration;
pub mod snapshot;

pub use error::{Error, Result};
pub use grid::{Cell, CellAttrs, Color, CommandEnd, Cursor, Grid};
pub use image::{Decoded, ImageLayer, Placement};
pub use pty::Pty;
pub use scrollback::History;
pub use selection::Selection;
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

    /// Set the live drag selection (the app drives this from the mouse — see the
    /// `selection-input` feature). Carried to the renderer via
    /// [`snapshot`](Terminal::snapshot); its text is read via
    /// [`GridSnapshot::selection_text`](crate::GridSnapshot::selection_text).
    pub fn set_selection(&mut self, selection: Selection) {
        self.grid.set_selection(selection);
    }

    /// Clear any drag selection.
    pub fn clear_selection(&mut self) {
        self.grid.clear_selection();
    }

    /// The current drag selection, if any.
    pub fn selection(&self) -> Option<Selection> {
        self.grid.selection()
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

    /// Take the most-recent OSC 0 / OSC 2 window title set since the last take, clearing
    /// it (one-shot drain). Poll it right after [`advance`](Terminal::advance). `None` if
    /// no title arrived; `Some("")` when the title was explicitly cleared (the app then
    /// reverts the tab's manual label to its auto/cwd label). Like
    /// [`take_command_end`](Terminal::take_command_end), it is an accessor, not a
    /// [`GridSnapshot`] field — the renderer never reads it; the `app` owns the tab label.
    pub fn take_osc_title(&mut self) -> Option<String> {
        self.grid.take_osc_title()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_command_end_exposes_osc133_and_drains() {
        let mut t = Terminal::new(20, 5, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"\x1b]133;D;101;40000\x07");
        let ev = t.take_command_end().expect("accessor returns the event");
        assert_eq!(ev.exit_code, 101);
        assert_eq!(ev.elapsed_ms, 40000);
        assert!(t.take_command_end().is_none(), "one-shot: drains on take");
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
