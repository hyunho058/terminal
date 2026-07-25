//! VT/ANSI parsing: drives `vte` and translates its callbacks into [`Grid`]
//! mutations. `vte` owns the escape-sequence state machine (D5); we own the
//! grid semantics — this is where "manual cell control" lives.
//!
//! Coverage here is intentionally a scaffold baseline: printable text, the core
//! C0 controls, cursor positioning, erase, and 16-color SGR. Richer sequences
//! (scroll regions, 256/truecolor, modes) are the first extension via `/specify`.

use vte::{Params, Parser, Perform};

use crate::grid::{CellAttrs, Color, CommandEnd, Grid};
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
    /// SGR — select graphic rendition. Handles the 16 base colors, 256-color
    /// (`38;5;n` / `48;5;n`), truecolor (`38;2;r;g;b` / `48;2;…`, both the
    /// semicolon and colon-subparam forms), the text attributes
    /// bold/dim/italic/underline/reverse (set 1/2/3/4/7, clear 22/23/24/27), and
    /// per-channel default reset (39/49). Unknown codes are ignored without
    /// disturbing the surrounding parameters.
    fn handle_sgr(&mut self, params: &Params) {
        if params.is_empty() {
            self.grid.reset_pen();
            return;
        }
        // A single iterator drives both the main loop and the extended-color
        // consumption of following params, so `38;5;n` / `38;2;r;g;b` read their
        // trailing params rather than being treated as independent codes.
        let mut iter = params.iter();
        while let Some(p) = iter.next() {
            match p.first().copied().unwrap_or(0) {
                0 => self.grid.reset_pen(),
                1 => self.grid.set_pen_attrs(CellAttrs::BOLD),
                2 => self.grid.set_pen_attrs(CellAttrs::DIM),
                3 => self.grid.set_pen_attrs(CellAttrs::ITALIC),
                4 => self.grid.set_pen_attrs(CellAttrs::UNDERLINE),
                7 => self.grid.set_pen_attrs(CellAttrs::REVERSE),
                // 22 = normal intensity: clears BOTH bold and dim.
                22 => self
                    .grid
                    .clear_pen_attrs(CellAttrs(CellAttrs::BOLD.0 | CellAttrs::DIM.0)),
                23 => self.grid.clear_pen_attrs(CellAttrs::ITALIC),
                24 => self.grid.clear_pen_attrs(CellAttrs::UNDERLINE),
                27 => self.grid.clear_pen_attrs(CellAttrs::REVERSE),
                c @ 30..=37 => self.grid.set_pen(Some(ansi_16((c - 30) as u8)), None),
                38 => {
                    if let Some(color) = resolve_ext_color(p, &mut iter) {
                        self.grid.set_pen(Some(color), None);
                    }
                }
                39 => self.grid.reset_pen_fg(),
                c @ 40..=47 => self.grid.set_pen(None, Some(ansi_16((c - 40) as u8))),
                48 => {
                    if let Some(color) = resolve_ext_color(p, &mut iter) {
                        self.grid.set_pen(None, Some(color));
                    }
                }
                49 => self.grid.reset_pen_bg(),
                c @ 90..=97 => self.grid.set_pen(Some(ansi_16((c - 90 + 8) as u8)), None),
                c @ 100..=107 => self.grid.set_pen(None, Some(ansi_16((c - 100 + 8) as u8))),
                _ => {}
            }
        }
    }
}

/// Clamp a `u16` SGR parameter to a color byte (values should be 0..=255).
fn to_u8(v: u16) -> u8 {
    v.min(255) as u8
}

/// Parse an OSC 133 field as a signed exit code. `None` (not a panic) on
/// non-UTF-8 or non-numeric bytes, so a garbage field drops the event (D20).
fn parse_ascii_i32(bytes: &[u8]) -> Option<i32> {
    std::str::from_utf8(bytes).ok()?.trim().parse().ok()
}

/// Parse an OSC 133 field as an unsigned elapsed-milliseconds value. Same
/// silent-failure contract as [`parse_ascii_i32`].
fn parse_ascii_u64(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes).ok()?.trim().parse().ok()
}

/// Resolve a `38`/`48` extended-color directive to a [`Color`]. `p` is the param
/// that carried the `38`/`48`; `rest` is the remaining param iterator (consumed for
/// the semicolon form). Returns `None` for an unsupported selector or a truncated
/// sequence, leaving the pen unchanged (R7.2).
fn resolve_ext_color<'a>(p: &[u16], rest: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    if p.len() > 1 {
        // Colon-subparam form: the whole directive is ONE param.
        //   38:5:n            → p = [38, 5, n]
        //   38:2:r:g:b        → p = [38, 2, r, g, b]
        //   38:2::r:g:b       → p = [38, 2, 0, r, g, b]  (empty T.416 color-space id)
        match p.get(1).copied() {
            // `38:5:n` — index at subparam 2; a bare `38:5` (no index) is malformed.
            Some(5) if p.len() >= 3 => Some(ansi_256(to_u8(p[2]))),
            // Take the LAST three subparams as r,g,b so the optional color-space-id
            // slot in the `:2:` form is skipped (never the first three).
            Some(2) if p.len() >= 5 => {
                let n = p.len();
                Some(Color::rgb(
                    to_u8(p[n - 3]),
                    to_u8(p[n - 2]),
                    to_u8(p[n - 1]),
                ))
            }
            _ => None,
        }
    } else {
        // Semicolon form: the selector and values arrive as following params.
        match rest.next().and_then(|q| q.first().copied()) {
            Some(5) => rest
                .next()
                .and_then(|q| q.first().copied())
                .map(|i| ansi_256(to_u8(i))),
            Some(2) => {
                let r = rest.next().and_then(|q| q.first().copied())?;
                let g = rest.next().and_then(|q| q.first().copied())?;
                let b = rest.next().and_then(|q| q.first().copied())?;
                Some(Color::rgb(to_u8(r), to_u8(g), to_u8(b)))
            }
            _ => None,
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

    /// Bare ESC sequences (no CSI/OSC introducer). Exactly one is implemented:
    /// RIS (`ESC c`), and only far enough to undo DECCKM (arrow-key-input D9) —
    /// that is the recovery path D3 relies on, since nothing else clears the flag
    /// once a TUI dies without sending `?1l` and `reset`/`tput reset` sends RIS.
    ///
    /// It is deliberately **not** a full RIS. `Grid::clear()` also drops every
    /// inline image placement, and `alt_screen` here is a history-preserving
    /// switch rather than a second buffer — so clearing on RIS would turn two
    /// harmless bytes (a `cat`ted binary is enough) into a new destructive
    /// primitive. The minimal form buys the recovery and none of the fallout.
    ///
    /// The `intermediates.is_empty()` guard exists because other escapes end in
    /// the same final byte (`ESC ( c` selects a charset); only a bare `ESC c` is
    /// RIS. Every other sequence stays a no-op on purpose — `ESC =` / `ESC >`
    /// (DECKPAM/DECKPNM) really do arrive here, since vim sends them via terminfo
    /// `smkx`/`rmkx`, and DECNKM is a declared non-goal.
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if intermediates.is_empty() && byte == b'c' {
            self.grid.set_app_cursor_keys(false);
            self.grid.set_bracketed_paste(false);
        }
    }

    /// OSC — Operating System Command. The only sequence we act on is **OSC 133**
    /// shell-integration command markers, injected by our generated zshrc
    /// (`pty.rs` / `shell_integration.rs`). vte splits the payload on `;`, so a
    /// command-end `ESC ] 133 ; D ; <exit> ; <elapsed_ms> ST` arrives as
    /// `params = [b"133", b"D", b"<exit>", b"<elapsed_ms>"]`.
    ///
    /// The wire contract (emitter + parser both live in this task — keep in sync):
    /// - Command start: `ESC ] 133 ; C ST`  (recognized, no payload consumed yet)
    /// - Command end:   `ESC ] 133 ; D ; <exit_code> ; <elapsed_ms> ST`
    ///
    /// `elapsed_ms` is measured shell-side from `EPOCHREALTIME` (spec D19). Both
    /// `BEL` and `ESC \` terminators are accepted (vte handles framing).
    ///
    /// Robustness (spec D20, project no-unwrap rule): an unknown OSC number, an
    /// unknown 133 subkind, or a missing/garbage exit/elapsed field is dropped
    /// silently — no panic, no `unwrap`, grid state unchanged.
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 (icon+title) / OSC 2 (window title) → the tab-identity manual label
        // (set by the shell or the user, e.g. `printf '\e]2;Frontend\a'`). The title is
        // everything after the number; an empty title clears the label. Invalid UTF-8 is
        // dropped (lossy would fabricate replacement chars into a user-facing label).
        let osc_num = params.first().copied();
        if osc_num == Some(b"0".as_slice()) || osc_num == Some(b"2".as_slice()) {
            if let Some(title) = params.get(1).copied() {
                if let Ok(s) = std::str::from_utf8(title) {
                    self.grid.record_title(s.to_string());
                }
            }
            return;
        }
        // Only OSC 133 is ours beyond the title; everything else (cwd, other
        // integrations) is intentionally ignored — as it was before osc_dispatch existed.
        if params.first().copied() != Some(b"133".as_slice()) {
            return;
        }
        // `D` = command end — the only subkind carrying the exit + elapsed payload
        // the waiting feature needs. `C` (command start) and any other subkind are
        // recognized but carry nothing we consume, so they fall through to a no-op.
        if params.get(1).copied() != Some(b"D".as_slice()) {
            return;
        }
        // Both fields are required; either missing/garbage → drop the whole event
        // and leave prior state untouched (partial params are not a valid marker).
        let (Some(exit_code), Some(elapsed_ms)) = (
            params.get(2).copied().and_then(parse_ascii_i32),
            params.get(3).copied().and_then(parse_ascii_u64),
        ) else {
            return;
        };
        self.grid.record_command_end(CommandEnd {
            exit_code,
            elapsed_ms,
        });
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let first = params.iter().next().and_then(|p| p.first().copied());
        // Private modes (`CSI ? ... h/l`) — alt-screen enter/leave (D5, owned by
        // scrollback; image-paste adds its own arms as an ordered extension).
        if intermediates.first() == Some(&b'?') && matches!(action, 'h' | 'l') {
            match first {
                Some(1049) | Some(1047) | Some(47) => self.grid.set_alt_screen(action == 'h'),
                // DECCKM — application cursor key mode. Only flips the flag the app
                // reads when encoding arrows (arrow-key-input D3/D10); nothing about
                // the grid changes.
                Some(1) => self.grid.set_app_cursor_keys(action == 'h'),
                // DECSET/DECRST ?2004 — bracketed paste. Flips the flag the app
                // reads to wrap a paste in `ESC[200~`/`ESC[201~` (text-paste); the
                // grid is unchanged and the flag is absent from `GridSnapshot`.
                Some(2004) => self.grid.set_bracketed_paste(action == 'h'),
                _ => {}
            }
            return;
        }
        // Private-marker CSIs (`>`/`<`/`=` — e.g. XTMODKEYS `CSI > 4 ; 2 m`) share
        // their final byte with real commands (`m` with SGR), but we implement none
        // of them. Ignore any such sequence rather than misparsing it as SGR — the
        // bug this fixes: `CSI > 4 ; 2 m` set UNDERLINE (4) + DIM (2) permanently,
        // painting the whole screen underlined. Same early-return shape as the `?`
        // private-mode guard above.
        if matches!(intermediates.first(), Some(&(b'>' | b'<' | b'='))) {
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
            // CUU / CUD / CUF / CUB — relative cursor moves. Param defaults to 1,
            // and 0 is treated as 1. zsh emits these to reposition across wrapped
            // rows while redrawing an edited line; dropping them desyncs the cursor
            // and overwrites the prompt (the bug this fixes). move_to clamps to grid.
            'A' => {
                let n = first.unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_to(cur.col, cur.row.saturating_sub(n));
            }
            'B' => {
                let n = first.unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_to(cur.col, cur.row.saturating_add(n));
            }
            'C' => {
                let n = first.unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_to(cur.col.saturating_add(n), cur.row);
            }
            'D' => {
                let n = first.unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_to(cur.col.saturating_sub(n), cur.row);
            }
            // CHA / VPA — absolute column / row (1-based → 0-based).
            'G' => {
                let col = first.unwrap_or(1).max(1).saturating_sub(1);
                let cur = self.grid.cursor();
                self.grid.move_to(col, cur.row);
            }
            'd' => {
                let row = first.unwrap_or(1).max(1).saturating_sub(1);
                let cur = self.grid.cursor();
                self.grid.move_to(cur.col, row);
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

/// xterm 256-color palette: `0..=15` delegate to [`ansi_16`], `16..=231` are the
/// 6×6×6 RGB cube, and `232..=255` are the 24-step grayscale ramp.
fn ansi_256(n: u8) -> Color {
    match n {
        0..=15 => ansi_16(n),
        16..=231 => {
            // Cube component levels: 0, then 95,135,175,215,255 (55 + c*40).
            let i = n - 16;
            let level = |c: u8| -> u8 {
                if c == 0 {
                    0
                } else {
                    55 + c * 40
                }
            };
            Color::rgb(level(i / 36), level((i % 36) / 6), level(i % 6))
        }
        232..=255 => {
            let g = 8 + (n - 232) * 10; // 8, 18, …, 238
            Color::rgb(g, g, g)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Cell;
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

    // --- OSC 133 shell-integration command markers (left-sidebar-waiting-area) ---

    #[test]
    fn osc133_command_end_records_exit_and_elapsed() {
        // R4.4: `ESC ] 133 ; D ; 101 ; 40000 BEL` → exit_code 101, elapsed_ms 40000.
        let mut g = grid();
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b]133;D;101;40000\x07");
        let ev = g.take_command_end().expect("command-end recorded");
        assert_eq!(ev.exit_code, 101);
        assert_eq!(ev.elapsed_ms, 40000);
        // One-shot drain: a second take yields nothing.
        assert!(g.take_command_end().is_none(), "take_command_end drains");
    }

    #[test]
    fn osc133_accepts_st_terminator_and_negative_exit() {
        // The `ESC \` (ST) terminator and a signed exit code both parse.
        let mut g = grid();
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b]133;D;-1;250\x1b\\");
        let ev = g.take_command_end().expect("ST-terminated marker parsed");
        assert_eq!(ev.exit_code, -1);
        assert_eq!(ev.elapsed_ms, 250);
    }

    #[test]
    fn osc133_malformed_or_unknown_is_ignored_without_panic() {
        // R4.5 / D20: garbage or partial params → no event, no panic, state intact.
        let mut g = grid();
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b]133;D;notanumber;40000\x07"); // bad exit
        assert!(g.take_command_end().is_none(), "garbage exit → dropped");
        p.advance(&mut g, b"\x1b]133;D;5\x07"); // missing elapsed
        assert!(g.take_command_end().is_none(), "missing elapsed → dropped");
        p.advance(&mut g, b"\x1b]133;C\x07"); // command-start, no payload
        assert!(g.take_command_end().is_none(), "C carries no command-end");
        p.advance(&mut g, b"\x1b]999;whatever\x07"); // unknown OSC number
        assert!(
            g.take_command_end().is_none(),
            "unknown OSC number → dropped"
        );
        p.advance(&mut g, b"\x1b]\x07"); // empty OSC
        assert!(g.take_command_end().is_none(), "empty OSC → dropped");
    }

    #[test]
    fn osc133_does_not_disturb_surrounding_text() {
        // The marker must be swallowed cleanly; text before and after still lands.
        let mut g = grid();
        let mut p = Processor::new();
        p.advance(&mut g, b"ok\x1b]133;D;0;12\x07XY");
        let ev = g.take_command_end().expect("marker parsed mid-stream");
        assert_eq!((ev.exit_code, ev.elapsed_ms), (0, 12));
        assert_eq!(g.cell(0, 0).unwrap().ch, 'o');
        assert_eq!(g.cell(1, 0).unwrap().ch, 'k');
        assert_eq!(g.cell(2, 0).unwrap().ch, 'X');
        assert_eq!(g.cell(3, 0).unwrap().ch, 'Y');
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
        assert_eq!(
            row_text(&g, 0),
            "aaa",
            "prompt redraw did not home the cursor"
        );
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
        let any_ink = (0..g.rows()).any(|r| (0..g.cols()).any(|c| g.cell(c, r).unwrap().ch != ' '));
        assert!(!any_ink, "ED2 blanks the entire grid");
    }

    // --- SGR color + attributes (vt-color-attrs) ---

    fn sgr_cell(seq: &[u8]) -> Cell {
        let mut g = Grid::new(4, 1, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, seq);
        p.advance(&mut g, b"X");
        *g.cell(0, 0).unwrap()
    }

    #[test]
    fn sgr_256_color_fg_and_bg() {
        // R1.1/R1.2: 38;5;n and 48;5;n resolve through the xterm 256 palette.
        assert_eq!(sgr_cell(b"\x1b[38;5;196m").fg, ansi_256(196));
        assert_eq!(sgr_cell(b"\x1b[48;5;21m").bg, ansi_256(21));
    }

    #[test]
    fn sgr_256_ramp_bands() {
        // R1.3: 0-15 delegate to ansi_16; 16-231 the 6x6x6 cube; 232-255 grayscale.
        assert_eq!(ansi_256(7), ansi_16(7));
        assert_eq!(ansi_256(16), Color::rgb(0, 0, 0));
        assert_eq!(ansi_256(231), Color::rgb(255, 255, 255));
        assert_eq!(ansi_256(232), Color::rgb(8, 8, 8));
        assert_eq!(ansi_256(255), Color::rgb(238, 238, 238));
    }

    #[test]
    fn sgr_truecolor_semicolon() {
        // R2.1/R2.2: 38;2;r;g;b and 48;2;r;g;b.
        assert_eq!(
            sgr_cell(b"\x1b[38;2;255;128;0m").fg,
            Color::rgb(255, 128, 0)
        );
        assert_eq!(sgr_cell(b"\x1b[48;2;0;40;80m").bg, Color::rgb(0, 40, 80));
    }

    #[test]
    fn sgr_truecolor_colon_skips_colorspace_slot() {
        // R2.3: the colon `:2:` form carries a T.416 color-space-id slot that must
        // be skipped — the LAST three subparams are r,g,b, not the first three.
        assert_eq!(
            sgr_cell(b"\x1b[38:2::255:128:0m").fg,
            Color::rgb(255, 128, 0)
        );
        // Colon indexed form has no such slot.
        assert_eq!(sgr_cell(b"\x1b[38:5:196m").fg, ansi_256(196));
    }

    #[test]
    fn sgr_attributes_set_and_clear() {
        // R3.1: set bits.
        let c = sgr_cell(b"\x1b[1;4;7m");
        assert!(c.attrs.contains(CellAttrs::BOLD));
        assert!(c.attrs.contains(CellAttrs::UNDERLINE));
        assert!(c.attrs.contains(CellAttrs::REVERSE));
        // R3.2: 22 clears BOTH bold and dim; 24 underline; 27 reverse — others kept.
        let mut g = Grid::new(4, 1, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[1;2;3;4;7m"); // bold dim italic underline reverse
        p.advance(&mut g, b"\x1b[22m"); // clear bold + dim
        p.advance(&mut g, b"A");
        let a = *g.cell(0, 0).unwrap();
        assert!(!a.attrs.contains(CellAttrs::BOLD));
        assert!(!a.attrs.contains(CellAttrs::DIM));
        assert!(a.attrs.contains(CellAttrs::ITALIC));
        assert!(a.attrs.contains(CellAttrs::UNDERLINE));
        assert!(a.attrs.contains(CellAttrs::REVERSE));
    }

    #[test]
    fn sgr_private_intermediates_are_ignored() {
        // Regression: XTMODKEYS `CSI > 4 ; 2 m` must NOT be parsed as SGR 4;2
        // (underline + dim). The `>` (and `<`/`=`) private-marker byte means "not
        // SGR"; the csi_dispatch guard drops it. Without the guard, underline latched
        // on for the whole screen. `sgr_cell` prints "X" AFTER the sequence.
        let a = sgr_cell(b"\x1b[>4;2m");
        assert!(
            !a.attrs.contains(CellAttrs::UNDERLINE),
            "`>` (XTMODKEYS) must not set underline"
        );
        assert!(
            !a.attrs.contains(CellAttrs::DIM),
            "`>` (XTMODKEYS) must not set dim"
        );
        assert!(
            !sgr_cell(b"\x1b[=4m").attrs.contains(CellAttrs::UNDERLINE),
            "`=` marker ignored"
        );
        assert!(
            !sgr_cell(b"\x1b[<4m").attrs.contains(CellAttrs::UNDERLINE),
            "`<` marker ignored"
        );
        // Positive controls — the guard must not over-suppress real SGR.
        assert!(
            sgr_cell(b"\x1b[4m").attrs.contains(CellAttrs::UNDERLINE),
            "plain SGR 4 still underlines"
        );
        let m = sgr_cell(b"\x1b[1;4;7m");
        assert!(m.attrs.contains(CellAttrs::BOLD));
        assert!(m.attrs.contains(CellAttrs::UNDERLINE));
        assert!(m.attrs.contains(CellAttrs::REVERSE));
    }

    #[test]
    fn sgr_default_reset_39_49_are_per_channel() {
        // R4.1/R4.2: 39 resets only fg, 49 only bg; other pen state is untouched.
        let mut g = Grid::new(4, 1, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[38;5;196;48;5;21;1m"); // fg, bg, bold
        p.advance(&mut g, b"\x1b[39m"); // reset fg only
        p.advance(&mut g, b"A");
        let a = *g.cell(0, 0).unwrap();
        assert_eq!(a.fg, Color(0xFFFFFF), "39 restores default fg");
        assert_eq!(a.bg, ansi_256(21), "39 leaves bg");
        assert!(a.attrs.contains(CellAttrs::BOLD), "39 leaves attrs");
        p.advance(&mut g, b"\x1b[49m"); // reset bg only
        p.advance(&mut g, b"B");
        let b = *g.cell(1, 0).unwrap();
        assert_eq!(b.bg, Color(0), "49 restores default bg");
        assert!(b.attrs.contains(CellAttrs::BOLD), "49 leaves attrs");
    }

    #[test]
    fn sgr_unknown_and_truncated_are_safe() {
        // R7.1: an unsupported code (9 strikethrough) is skipped without eating the
        // surrounding params — 38;5;196 still applies and 1 (bold) still sets.
        let c = sgr_cell(b"\x1b[38;5;196;9;1m");
        assert_eq!(c.fg, ansi_256(196));
        assert!(c.attrs.contains(CellAttrs::BOLD));
        // R7.2: truncated extended-color sequences do not panic and leave the pen.
        assert_eq!(sgr_cell(b"\x1b[38;5m").fg, Color(0xFFFFFF));
        assert_eq!(sgr_cell(b"\x1b[38;2;255m").fg, Color(0xFFFFFF));
    }

    #[test]
    fn cell_partial_eq_includes_attrs() {
        // R6.1/R6.2: Cell's derived PartialEq covers attrs, so an attribute-only
        // change is unequal — the basis idle-cpu's snapshot cache relies on.
        let plain = sgr_cell(b"");
        let bold = sgr_cell(b"\x1b[1m");
        assert_ne!(
            plain, bold,
            "attribute-only difference must compare unequal"
        );
        assert_eq!(plain.ch, bold.ch);
        assert_eq!(plain.fg, bold.fg);
    }

    // --- relative CSI cursor moves (csi-cursor-moves) ---

    fn cur(g: &Grid) -> (u16, u16) {
        (g.cursor().col, g.cursor().row)
    }

    #[test]
    fn csi_relative_and_absolute_cursor_moves() {
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[6;6H"); // CUP → (col5, row5)
        assert_eq!(cur(&g), (5, 5));
        p.advance(&mut g, b"\x1b[2A"); // CUU 2 → row 3
        assert_eq!(cur(&g), (5, 3));
        p.advance(&mut g, b"\x1b[B"); // CUD default 1 → row 4
        assert_eq!(cur(&g), (5, 4));
        p.advance(&mut g, b"\x1b[3C"); // CUF 3 → col 8
        assert_eq!(cur(&g), (8, 4));
        p.advance(&mut g, b"\x1b[D"); // CUB default 1 → col 7
        assert_eq!(cur(&g), (7, 4));
        p.advance(&mut g, b"\x1b[G"); // CHA no param → col 0
        assert_eq!(cur(&g), (0, 4));
        p.advance(&mut g, b"\x1b[10G"); // CHA col 10 (1-based) → col 9
        assert_eq!(cur(&g), (9, 4));
        p.advance(&mut g, b"\x1b[d"); // VPA row 1 → row 0
        assert_eq!(cur(&g), (9, 0));
    }

    #[test]
    fn csi_cursor_moves_clamp_and_treat_zero_as_one() {
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[6;6H\x1b[0D"); // CUB with 0 == 1: col5 → col4
        assert_eq!(cur(&g), (4, 5));
        p.advance(&mut g, b"\x1b[99A"); // clamp top
        assert_eq!(g.cursor().row, 0);
        p.advance(&mut g, b"\x1b[99C"); // clamp right (cols-1 = 19)
        assert_eq!(g.cursor().col, 19);
        p.advance(&mut g, b"\x1b[99B"); // clamp bottom (rows-1 = 9)
        assert_eq!(g.cursor().row, 9);
    }

    #[test]
    fn prompt_row_survives_wrapped_edit() {
        // Reproduces the reported bug: a wrapped line, then zsh moves the cursor UP
        // (CSI A) to the prompt row to redraw + erase. Before the fix CSI A was
        // dropped, the cursor stayed on the wrapped row, and the erase hit the wrong
        // row. Now CSI A actually moves to row 0 and only the intended region clears.
        let mut g = Grid::new(8, 4, Color(0xFFFFFF), Color(0)); // 8 cols → wraps
        let mut p = Processor::new();
        p.advance(&mut g, b"ab> cdefghij"); // row0="ab> cdef", row1="ghij", cursor (col4,row1)
        assert_eq!(row_text(&g, 0), "ab> cdef");
        assert_eq!(row_text(&g, 1), "ghij");
        assert_eq!(cur(&g), (4, 1));

        p.advance(&mut g, b"\x1b[A"); // CUU → the fix: cursor moves to row 0
        assert_eq!(g.cursor().row, 0, "CSI A must move the cursor up (the bug)");
        p.advance(&mut g, b"\x1b[5G"); // CHA col 5 (1-based) → col 4
        p.advance(&mut g, b"\x1b[K"); // erase row0 from col4 to end

        // The prompt "ab> " (cols 0-3) is intact; only cols 4-7 were cleared.
        assert_eq!(g.cell(0, 0).unwrap().ch, 'a');
        assert_eq!(g.cell(1, 0).unwrap().ch, 'b');
        assert_eq!(g.cell(2, 0).unwrap().ch, '>');
        assert_eq!(g.cell(4, 0).unwrap().ch, ' ');
        assert_eq!(row_text(&g, 0), "ab>");
    }
}
