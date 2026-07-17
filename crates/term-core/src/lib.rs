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
pub mod parser;
pub mod pty;
pub mod snapshot;

pub use error::{Error, Result};
pub use grid::{Cell, Color, Cursor, Grid};
pub use pty::Pty;
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

    pub fn dimensions(&self) -> (u16, u16) {
        (self.grid.cols(), self.grid.rows())
    }
}
