//! VT/ANSI parsing: drives `vte` and translates its callbacks into [`Grid`]
//! mutations. `vte` owns the escape-sequence state machine (D5); we own the
//! grid semantics — this is where "manual cell control" lives.
//!
//! Coverage here is intentionally a scaffold baseline: printable text, the core
//! C0 controls, cursor positioning, erase, and 16-color SGR. Richer sequences
//! (scroll regions, 256/truecolor, modes) are the first extension via `/specify`.

use vte::{Params, Parser, Perform};

use crate::grid::{Color, Grid};

/// Owns the `vte` state machine across calls. Feed it PTY bytes with [`advance`].
///
/// [`advance`]: Processor::advance
pub struct Processor {
    parser: Parser,
}

impl Default for Processor {
    fn default() -> Self {
        Self::new()
    }
}

impl Processor {
    pub fn new() -> Self {
        Processor {
            parser: Parser::new(),
        }
    }

    /// Feed a chunk of PTY output; mutates `grid` in place.
    pub fn advance(&mut self, grid: &mut Grid, bytes: &[u8]) {
        let mut performer = Performer { grid };
        self.parser.advance(&mut performer, bytes);
    }
}

/// Short-lived binding of the parser callbacks to a mutable grid.
struct Performer<'a> {
    grid: &'a mut Grid,
}

impl Performer<'_> {
    fn handle_sgr(&mut self, params: &Params) {
        if params.is_empty() {
            self.grid.reset_pen();
            return;
        }
        for p in params.iter() {
            match p.first().copied().unwrap_or(0) {
                0 => self.grid.reset_pen(),
                30..=37 => self.grid.set_pen(Some(ansi_16(p[0] as u8 - 30)), None),
                40..=47 => self.grid.set_pen(None, Some(ansi_16(p[0] as u8 - 40))),
                90..=97 => self.grid.set_pen(Some(ansi_16(p[0] as u8 - 90 + 8)), None),
                100..=107 => self.grid.set_pen(None, Some(ansi_16(p[0] as u8 - 100 + 8))),
                _ => {}
            }
        }
    }
}

impl Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        self.grid.print(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.grid.line_feed(),
            b'\r' => self.grid.carriage_return(),
            0x08 => self.grid.backspace(),
            b'\t' => self.grid.tab(),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let first = params.iter().next().and_then(|p| p.first().copied());
        match action {
            // CUP / HVP — cursor position (1-based row;col → 0-based).
            'H' | 'f' => {
                let mut it = params.iter();
                let row = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let col = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                self.grid
                    .move_to(col.saturating_sub(1), row.saturating_sub(1));
            }
            // ED — erase display (only the "all" variant handled at baseline).
            'J' => {
                if matches!(first, None | Some(0) | Some(2)) {
                    self.grid.clear();
                }
            }
            // EL — erase line from cursor.
            'K' => self.grid.clear_line_from_cursor(),
            // SGR — select graphic rendition (colors).
            'm' => self.handle_sgr(params),
            _ => {}
        }
    }
}

/// Standard 16-color ANSI palette (indices 0..=15).
fn ansi_16(index: u8) -> Color {
    const PALETTE: [u32; 16] = [
        0x000000, 0xCD3131, 0x0DBC79, 0xE5E510, 0x2472C8, 0xBC3FBC, 0x11A8CD, 0xE5E5E5, 0x666666,
        0xF14C4C, 0x23D18B, 0xF5F543, 0x3B8EEA, 0xD670D6, 0x29B8DB, 0xFFFFFF,
    ];
    Color(PALETTE[(index & 0x0F) as usize])
}
