//! VT/ANSI parsing: drives `vte` and translates its callbacks into [`Grid`]
//! mutations. `vte` owns the escape-sequence state machine (D5); we own the
//! grid semantics — this is where "manual cell control" lives.
//!
//! Coverage here is intentionally a scaffold baseline: printable text, the core
//! C0 controls, cursor positioning, erase, and 16-color SGR. Richer sequences
//! (scroll regions, 256/truecolor, modes) are the first extension via `/specify`.

use vte::{Params, Parser, Perform};

use crate::grid::{Color, Grid};
use crate::image::parse_graphics_apc;

/// Where the APC pre-scanner is in the byte stream.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Apc {
    Normal,
    Esc,    // saw ESC in normal text
    Inside, // inside an APC string (Kitty graphics)
    EscIn,  // saw ESC inside the APC string (maybe the `ESC \` terminator)
}

/// Cap on a single APC payload, so a never-terminated APC can't grow unbounded.
const APC_MAX: usize = 16 * 1024 * 1024;

/// Owns the `vte` state machine across calls. Feed it PTY bytes with [`advance`].
///
/// Kitty inline-image escapes ride **APC** (`ESC _ … ESC \`), which `vte` 0.15
/// silently discards (it routes APC into an ignored string state). So we hand-scan
/// APC out of the byte stream here (image-paste, correcting spec D1's `vte
/// hook/put/unhook` mechanism — those are DCS), decode the graphics payload, and
/// feed everything else to `vte` unchanged.
///
/// [`advance`]: Processor::advance
pub struct Processor {
    parser: Parser,
    apc: Apc,
    apc_buf: Vec<u8>,
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
            apc: Apc::Normal,
            apc_buf: Vec::new(),
        }
    }

    /// Feed a chunk of PTY output; mutates `grid` in place. Non-APC bytes go to
    /// `vte`; a complete Kitty graphics APC is decoded and placed at the cursor.
    pub fn advance(&mut self, grid: &mut Grid, bytes: &[u8]) {
        let mut vte_buf: Vec<u8> = Vec::new();
        for &b in bytes {
            match self.apc {
                Apc::Normal => {
                    if b == 0x1B {
                        self.apc = Apc::Esc;
                    } else {
                        vte_buf.push(b);
                    }
                }
                Apc::Esc => match b {
                    // ESC _ → start of an APC string. Flush pending vte bytes first
                    // so the cursor is where the image should anchor.
                    b'_' => {
                        flush(&mut self.parser, grid, &mut vte_buf);
                        self.apc_buf.clear();
                        self.apc = Apc::Inside;
                    }
                    // ESC ESC → the first ESC was lone text; keep scanning the 2nd.
                    0x1B => vte_buf.push(0x1B),
                    // A normal escape (CSI, etc.) → hand ESC + byte back to vte.
                    _ => {
                        vte_buf.push(0x1B);
                        vte_buf.push(b);
                        self.apc = Apc::Normal;
                    }
                },
                Apc::Inside => {
                    if b == 0x1B {
                        self.apc = Apc::EscIn;
                    } else if self.apc_buf.len() < APC_MAX {
                        self.apc_buf.push(b);
                    } else {
                        // Runaway APC — abort, resync to normal text.
                        self.apc = Apc::Normal;
                        self.apc_buf.clear();
                    }
                }
                Apc::EscIn => match b {
                    // ESC \ → String Terminator: the APC is complete.
                    b'\\' => {
                        self.finish_apc(grid);
                        self.apc = Apc::Normal;
                    }
                    // ESC ESC inside APC → first ESC is content, keep looking.
                    0x1B => self.apc_buf.push(0x1B),
                    // ESC <other> → not a terminator; both bytes are content.
                    _ => {
                        self.apc_buf.push(0x1B);
                        self.apc_buf.push(b);
                        self.apc = Apc::Inside;
                    }
                },
            }
        }
        flush(&mut self.parser, grid, &mut vte_buf);
    }

    /// Decode a finished APC payload; on success place the image at the cursor
    /// (R1.1). Malformed payloads are silently dropped (R1.3).
    fn finish_apc(&mut self, grid: &mut Grid) {
        if let Some(decoded) = parse_graphics_apc(&self.apc_buf) {
            grid.place_image(decoded);
        }
        self.apc_buf.clear();
    }
}

/// Feed the accumulated non-APC bytes to `vte` and clear the buffer.
fn flush(parser: &mut Parser, grid: &mut Grid, buf: &mut Vec<u8>) {
    if !buf.is_empty() {
        let mut performer = Performer { grid };
        parser.advance(&mut performer, buf);
        buf.clear();
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

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let first = params.iter().next().and_then(|p| p.first().copied());
        // Private modes (`CSI ? ... h/l`) — alt-screen enter/leave (D5, owned by
        // scrollback; image-paste adds its own arms as an ordered extension).
        if intermediates.first() == Some(&b'?') && matches!(action, 'h' | 'l') {
            if matches!(first, Some(1049) | Some(1047) | Some(47)) {
                self.grid.set_alt_screen(action == 'h');
            }
            return;
        }
        match action {
            // CUP / HVP — cursor position (1-based row;col → 0-based).
            'H' | 'f' => {
                let mut it = params.iter();
                let row = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let col = it.next().and_then(|p| p.first().copied()).unwrap_or(1);
                self.grid
                    .move_to(col.saturating_sub(1), row.saturating_sub(1));
            }
            // ED — erase display. 0/none = cursor→end (what a shell sends to redraw
            // its prompt — must NOT wipe scrollback), 1 = start→cursor, 2/3 = all.
            'J' => match first {
                None | Some(0) => self.grid.clear_from_cursor(),
                Some(1) => self.grid.clear_to_cursor(),
                Some(2) | Some(3) => self.grid.clear(),
                _ => {}
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// Build a Kitty transmit-and-display APC (`ESC _ G … ; b64 ESC \`).
    fn kitty_apc(rgba: &[u8], s: u32, v: u32) -> Vec<u8> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(rgba);
        let mut out = b"\x1b_G".to_vec();
        out.extend_from_slice(format!("f=32,s={s},v={v},c=1,r=1;").as_bytes());
        out.extend_from_slice(b64.as_bytes());
        out.extend_from_slice(b"\x1b\\");
        out
    }

    fn grid() -> Grid {
        Grid::new(20, 5, Color(0xFFFFFF), Color(0))
    }

    #[test]
    fn apc_kitty_image_is_placed_at_cursor() {
        let mut g = grid();
        let mut p = Processor::new();
        p.advance(&mut g, b"hi"); // cursor advances to col 2
        let rgba: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 255], 4)
            .flatten()
            .collect();
        p.advance(&mut g, &kitty_apc(&rgba, 2, 2));
        assert_eq!(g.images().placements().len(), 1, "one image placed");
        assert_eq!(g.images().placements()[0].col, 2, "anchored at the cursor");
        // The APC wrote no cells — "hi" is intact, nothing after it.
        assert_eq!(g.cell(0, 0).unwrap().ch, 'h');
        assert_eq!(g.cell(2, 0).unwrap().ch, ' ');
    }

    #[test]
    fn malformed_apc_is_dropped_and_stream_continues() {
        let mut g = grid();
        let mut p = Processor::new();
        // A bogus graphics payload, then normal text — the text must still land.
        p.advance(&mut g, b"\x1b_Gbroken\x1b\\XY");
        assert!(
            g.images().placements().is_empty(),
            "malformed → no placement"
        );
        assert_eq!(g.cell(0, 0).unwrap().ch, 'X', "vte resumed after the APC");
        assert_eq!(g.cell(1, 0).unwrap().ch, 'Y');
    }

    #[test]
    fn apc_split_across_advance_calls_still_parses() {
        let mut g = grid();
        let mut p = Processor::new();
        let rgba = vec![0u8, 255, 0, 255]; // 1x1 green
        let apc = kitty_apc(&rgba, 1, 1);
        let (a, b) = apc.split_at(apc.len() / 2);
        p.advance(&mut g, a); // APC begins in chunk 1…
        p.advance(&mut g, b); // …completes in chunk 2
        assert_eq!(g.images().placements().len(), 1, "chunk-split APC parses");
    }

    #[test]
    fn normal_escapes_still_reach_vte_after_apc_scanning() {
        // A CSI cursor move must not be swallowed by the APC pre-scan.
        let mut g = grid();
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[2;3HX"); // CUP row2,col3 then 'X'
        assert_eq!(g.cell(2, 1).unwrap().ch, 'X');
    }

    fn row_text(g: &Grid, r: u16) -> String {
        (0..g.cols())
            .map(|c| g.cell(c, r).unwrap().ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn ed0_erases_below_cursor_only_and_keeps_scrollback() {
        // Regression: `ESC[J` (ED0) is what a shell sends (`\r ESC[J`) to redraw its
        // prompt after command output. It must clear only cursor→end, NOT the whole
        // screen — otherwise every prompt wipes the command's output.
        let mut g = Grid::new(10, 5, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"aaa\r\nbbb\r\nccc\r\n"); // rows 0..2 filled; cursor at row 3
        p.advance(&mut g, b"\x1b[J"); // ED0 at row 3
        assert_eq!(row_text(&g, 0), "aaa", "rows above the cursor survive ED0");
        assert_eq!(row_text(&g, 1), "bbb");
        assert_eq!(row_text(&g, 2), "ccc");
        // The shell then writes its prompt on the cleared line — not at row 0.
        p.advance(&mut g, b"% ");
        assert_eq!(row_text(&g, 3), "%");
        assert_eq!(row_text(&g, 0), "aaa", "prompt redraw did not home the cursor");
    }

    #[test]
    fn ed1_erases_up_to_cursor() {
        let mut g = Grid::new(10, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"aaa\r\nbbb\r\ncc"); // cursor mid-row-2
        p.advance(&mut g, b"\x1b[1J"); // start→cursor
        assert_eq!(row_text(&g, 0), "", "row above the cursor is erased");
        assert_eq!(row_text(&g, 1), "");
        // Row 2 up to and including the cursor cell is erased; the tail survives.
        assert_eq!(g.cell(0, 2).unwrap().ch, ' ');
    }

    #[test]
    fn ed2_clears_the_whole_display() {
        let mut g = Grid::new(10, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"x\r\ny\r\nz");
        p.advance(&mut g, b"\x1b[2J");
        let any_ink = (0..g.rows())
            .any(|r| (0..g.cols()).any(|c| g.cell(c, r).unwrap().ch != ' '));
        assert!(!any_ink, "ED2 blanks the entire grid");
    }
}
