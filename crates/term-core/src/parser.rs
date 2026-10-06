//! VT/ANSI parsing: drives `vte` and translates its callbacks into [`Grid`]
//! mutations. `vte` owns the escape-sequence state machine (D5); we own the
//! grid semantics — this is where "manual cell control" lives.
//!
//! Coverage is no longer the original scaffold baseline. Implemented here:
//! printable text and the core C0 controls; cursor positioning (CUP/HVP, CUU/CUD/
//! CUF/CUB, CHA, VPA); erase (ED `J`, EL `K` with its parameter, ECH `X`); the
//! full SGR set (16-color, 256-color, truecolor, bold/dim/italic/underline/
//! reverse); the DECSTBM scroll region (`r`) with region-aware scrolling (SU `S`,
//! SD `T`, IL `L`, DL `M`, plus `ESC D`/`ESC M`); in-line edits (ICH `@`,
//! DCH `P`); and the modes DECCKM `?1`, DECAWM `?7`, alt-screen `?1049`,
//! bracketed paste `?2004`, and synchronized update `?2026` (which times only the
//! selection change check — rendering does not honour it).
//!
//! Still unimplemented on purpose (see `specs/vt-tui-completeness` Known Gaps):
//! DECTCEM `?25` cursor visibility, DECSC/DECRC (`ESC 7`/`ESC 8`, `CSI s`/`u`),
//! and DECOM origin mode. Anything else still falls through `_ => {}` silently.

use vte::{Params, Parser, Perform};

use crate::grid::{CellAttrs, Color, CommandEnd, Grid, MouseEncoding, MouseProtocol};
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

    /// Bare ESC sequences (no CSI/OSC introducer). Three are implemented: IND
    /// (`ESC D`), RI (`ESC M`), and RIS (`ESC c`).
    ///
    /// RIS is handled only far enough to undo DECCKM (arrow-key-input D9), to drop
    /// the scroll region (vt-tui-completeness D5), and to close an open synchronized
    /// update (selection-scroll-anchor D5) — that is the recovery path
    /// D3 relies on, since nothing else clears those once a TUI dies without
    /// sending `?1l` and `reset`/`tput reset` sends RIS.
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
        if !intermediates.is_empty() {
            return;
        }
        // IND (`ESC D`) — index. Semantically identical to a line feed, so it routes
        // through the same path rather than duplicating the region logic — which
        // also keeps their fill colour identical, as it must be. Both apply
        // Background Color Erase (spec D6, as amended); see `Grid::feed_scroll`.
        if byte == b'D' {
            self.grid.line_feed();
            return;
        }
        // RI (`ESC M`) — reverse index. Scrolls the region DOWN at the region top,
        // which is how a TUI redraws upward; without it region scrolling only ever
        // works in one direction (R2.8/R2.9).
        if byte == b'M' {
            self.grid.reverse_index();
            return;
        }
        // DECSC / DECRC (`ESC 7` / `ESC 8`) — save and restore the cursor + pen.
        // Claude Code brackets its `CSI r` with this pair; since `CSI r` homes the
        // cursor (D9), dropping DECRC stranded the cursor at the origin — strictly
        // worse than before `CSI r` was implemented at all (D17).
        if byte == b'7' {
            self.grid.save_cursor();
            return;
        }
        if byte == b'8' {
            self.grid.restore_cursor();
            return;
        }
        if byte == b'c' {
            self.grid.set_app_cursor_keys(false);
            self.grid.set_bracketed_paste(false);
            // A TUI killed mid-run leaves mouse tracking on; without this the wheel
            // would keep being sent to a dead app instead of scrolling scrollback.
            self.grid.set_mouse_mode(MouseProtocol::Off, false);
            self.grid.set_mouse_encoding(MouseEncoding::Legacy);
            // Also drop any DECSTBM scroll region (vt-tui-completeness D5). This is
            // the recovery path for a TUI that renders on the *primary* screen —
            // Claude Code never sends `?1049h`, so D2's alt-screen reset never fires
            // for it, and a crash would otherwise strand zsh inside the dead app's
            // margins forever. `reset`/`tput reset` sends RIS, so this makes the
            // documented escape hatch work. Uses `reset_scroll_region`, not
            // `set_scroll_region`: RIS must not move the cursor (R4.3/R4.5).
            self.grid.reset_scroll_region();
            // And close any synchronized update the dead app left open
            // (selection-scroll-anchor D5), so the selection check stops deferring.
            // No check here (D7): the next settle runs one.
            self.grid.reset_synchronized_update();
        }
    }

    /// OSC — Operating System Command. The only sequence we act on is **OSC 133**
    /// shell-integration command markers, injected by our generated zshrc
    /// (`pty.rs` / `shell_integration.rs`). vte splits the payload on `;`, so a
    /// command-end `ESC ] 133 ; D ; <exit> ; <elapsed_ms> ST` arrives as
    /// `params = [b"133", b"D", b"<exit>", b"<elapsed_ms>"]`.
    ///
    /// The wire contract (emitter + parser both live in this task — keep in sync):
    /// - Command start: `ESC ] 133 ; C ST`  (no payload; sets the running state)
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
        // `C` = command start (sidebar-weather-cards D8): no payload, it only moves
        // the running state. `D` = command end, the one subkind carrying the exit +
        // elapsed payload. Any other subkind is recognized and ignored.
        match params.get(1).copied() {
            Some(b"C") => {
                self.grid.record_command_start();
                return;
            }
            Some(b"D") => {}
            _ => return,
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
            // `CSI ? Pm ; Pm ; ... h` is legal — xterm applies EVERY parameter, and
            // apps do batch them (`ESC[?1000;1002;1003;1006h`). Reading only the
            // first silently dropped the rest; the SGR selector in particular is
            // load-bearing, so losing it downgraded reports to legacy X10 and its
            // 223-column ceiling.
            let on = action == 'h';
            for p in params.iter() {
                match p.first().copied() {
                    Some(1049) | Some(1047) | Some(47) => self.grid.set_alt_screen(on),
                    // DECCKM — application cursor key mode. Only flips the flag the
                    // app reads when encoding arrows (arrow-key-input D3/D10);
                    // nothing about the grid changes.
                    Some(1) => self.grid.set_app_cursor_keys(on),
                    // DECAWM — autowrap. `?7l` makes printing at the last column
                    // overwrite in place; `?7h` restores wrapping. TUIs turn it off
                    // to paint the bottom-right cell without scrolling the screen
                    // (vt-tui-completeness R7.5).
                    Some(7) => self.grid.set_autowrap(on),
                    // DECSET/DECRST ?2004 — bracketed paste. Flips the flag the app
                    // reads to wrap a paste in `ESC[200~`/`ESC[201~` (text-paste);
                    // the grid is unchanged and the flag is absent from
                    // `GridSnapshot`.
                    Some(2004) => self.grid.set_bracketed_paste(on),
                    // DECSET/DECRST ?2026 — synchronized update. Used ONLY to time
                    // the selection change check (selection-scroll-anchor D5): an app
                    // brackets each frame with it, and a frame can arrive split
                    // across PTY reads, so the check waits for `?2026l` — and that
                    // close runs it, at the exact frame end. Rendering ignores it.
                    Some(2026) => self.grid.set_synchronized_update(on),
                    // Mouse tracking. Each mode is an INDEPENDENT flag, as in xterm:
                    // an app enables several at once and may disable them one at a
                    // time, and collapsing that to "off" would silently kill the
                    // wheel mid-session — the very symptom this feature fixes.
                    Some(9) => self.grid.set_mouse_mode(MouseProtocol::X10, on),
                    Some(1000) => self.grid.set_mouse_mode(MouseProtocol::Normal, on),
                    Some(1002) => self.grid.set_mouse_mode(MouseProtocol::ButtonEvent, on),
                    Some(1003) => self.grid.set_mouse_mode(MouseProtocol::AnyEvent, on),
                    // `?1006` — SGR encoding for those reports.
                    Some(1006) => self.grid.set_mouse_encoding(if on {
                        MouseEncoding::Sgr
                    } else {
                        MouseEncoding::Legacy
                    }),
                    _ => {}
                }
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
            // DECSTBM — set the top/bottom scroll region (vt-tui-completeness R1).
            // `CSI t;b r` is 1-based inclusive; a bare `CSI r` (or `0;0`) resets to
            // the full screen. Both forms home the cursor. Per D9's homing boundary
            // this is the only *region* path that homes — the internal resets
            // (alt-screen, RIS, resize) do not. `CSI 2J`/`3J` also home, via
            // `Grid::clear`, but that is the erase path, not a region change.
            'r' => {
                let mut it = params.iter();
                let top = it.next().and_then(|p| p.first().copied()).unwrap_or(0);
                let bottom = it.next().and_then(|p| p.first().copied()).unwrap_or(0);
                if top == 0 && bottom == 0 {
                    self.grid.reset_scroll_region();
                    self.grid.move_to(0, 0);
                } else {
                    // 1-based → 0-based. A `0` top means "line 1"; an omitted
                    // bottom means "last line", which the grid clamps for us.
                    self.grid.set_scroll_region(
                        top.saturating_sub(1),
                        if bottom == 0 { u16::MAX } else { bottom - 1 },
                    );
                }
            }
            // SCOSC / SCORC (`CSI s` / `CSI u`) — the ANSI spelling of DECSC/DECRC.
            // `CSI s` only means "save cursor" in its bare form; the two-parameter
            // `CSI l ; r s` is DECSLRM (left/right margins), which we do not
            // implement and must not misread as a save (D17).
            //
            // The test is the parameter *count*, not `first.is_none()`: vte reports
            // a bare `CSI s` as one default param (`Some(0)`), never as zero params,
            // so an is-none check would silently never save.
            's' if params.iter().count() < 2 => self.grid.save_cursor(),
            'u' => self.grid.restore_cursor(),
            // SU / SD — scroll the region up/down. Cursor unmoved (R2.2/R2.3).
            'S' => self.grid.scroll_up_lines(first.unwrap_or(1).max(1)),
            'T' => self.grid.scroll_down_lines(first.unwrap_or(1).max(1)),
            // IL / DL — insert/delete lines from the cursor row to the region
            // bottom. Ignored when the cursor is outside the region (R2.4-R2.6).
            'L' => self.grid.insert_lines(first.unwrap_or(1).max(1)),
            'M' => self.grid.delete_lines(first.unwrap_or(1).max(1)),
            // ICH / DCH / ECH — in-line character edits. Strictly line-scoped and
            // region-independent (D12); the cursor never moves (R5.1-R5.3).
            '@' => self.grid.insert_chars(first.unwrap_or(1).max(1)),
            'P' => self.grid.delete_chars(first.unwrap_or(1).max(1)),
            'X' => self.grid.erase_chars(first.unwrap_or(1).max(1)),
            // EL — erase line. 0/none = cursor→EOL, 1 = start→cursor (inclusive),
            // 2 = the whole line. Same unknown-param treatment as the `J` arm.
            'K' => match first {
                None | Some(0) => self.grid.clear_line_from_cursor(),
                Some(1) => self.grid.clear_line_to_cursor(),
                Some(2) => self.grid.clear_line(),
                _ => {}
            },
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
    use unicode_width::UnicodeWidthChar;

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

    // --- DECSTBM scroll region (vt-tui-completeness R1) ---

    #[test]
    fn fresh_grid_region_spans_the_whole_screen() {
        // R1.1: the default must be full-screen, because that is the predicate every
        // pre-existing scroll behavior now rides on (R3.4).
        let g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        assert_eq!(g.scroll_region(), (0, 23));
        assert!(g.region_is_full_screen());
    }

    #[test]
    fn decstbm_sets_region_and_homes_the_cursor() {
        // R1.2: `CSI 3;20r` on a 24-row grid → rows 2..=19, cursor to (0,0).
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[11;6H"); // cursor to row 10, col 5
        assert_eq!(cur(&g), (5, 10));
        p.advance(&mut g, b"\x1b[3;20r");
        assert_eq!(g.scroll_region(), (2, 19));
        assert!(!g.region_is_full_screen());
        assert_eq!(cur(&g), (0, 0), "DECSTBM homes the cursor");
    }

    #[test]
    fn decstbm_bare_and_zeroed_reset_to_full_screen() {
        // R1.3: both `CSI r` and `CSI 0;0r` restore the full screen and home.
        for seq in [b"\x1b[r".as_slice(), b"\x1b[0;0r".as_slice()] {
            let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
            let mut p = Processor::new();
            p.advance(&mut g, b"\x1b[3;20r");
            p.advance(&mut g, b"\x1b[11;6H");
            p.advance(&mut g, seq);
            assert_eq!(g.scroll_region(), (0, 23), "{seq:?} resets the region");
            assert!(g.region_is_full_screen());
            assert_eq!(cur(&g), (0, 0), "{seq:?} homes the cursor");
        }
    }

    #[test]
    fn decstbm_inverted_region_is_ignored_entirely() {
        // R1.4: top >= bottom → the whole sequence is dropped; the previous region,
        // the cursor, and every cell survive untouched.
        let mut g = Grid::new(10, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[3;20r"); // establish rows 2..=19
        p.advance(&mut g, b"\x1b[11;6Hmark");
        let before = *g.cell(5, 10).unwrap();
        let cursor_before = g.cursor();

        p.advance(&mut g, b"\x1b[20;3r"); // inverted
        assert_eq!(g.scroll_region(), (2, 19), "previous region survives");
        assert_eq!(g.cursor(), cursor_before, "inverted DECSTBM does not home");
        assert_eq!(*g.cell(5, 10).unwrap(), before, "no cell changed");

        // Equal top and bottom is degenerate too — a one-row region cannot scroll.
        p.advance(&mut g, b"\x1b[5;5r");
        assert_eq!(g.scroll_region(), (2, 19), "top == bottom is also rejected");
    }

    #[test]
    fn decstbm_clamps_out_of_range_bottom() {
        // R1.5: an over-large bottom widens to the last row rather than storing an
        // out-of-bounds index (which would panic the scroll primitive in T2).
        let mut g = Grid::new(10, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[1;999r");
        assert_eq!(g.scroll_region(), (0, 23));
        assert!(g.region_is_full_screen());
    }

    // --- Region lifetime + the cursor-homing boundary (R4) ---

    #[test]
    fn alt_screen_enter_and_leave_reset_the_region_without_homing() {
        // R4.1/R4.2: a TUI killed mid-run never restores its margins, so both
        // directions reset — but neither may move the cursor.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[3;20r\x1b[11;6H");
        assert_eq!(cur(&g), (5, 10));

        p.advance(&mut g, b"\x1b[?1049h"); // enter
        assert_eq!(g.scroll_region(), (0, 23), "enter resets the region");
        assert_eq!(cur(&g), (5, 10), "enter does NOT home the cursor");

        p.advance(&mut g, b"\x1b[3;20r"); // the app sets its own region
        assert_eq!(g.scroll_region(), (2, 19));
        p.advance(&mut g, b"\x1b[?1049l"); // leave
        assert_eq!(g.scroll_region(), (0, 23), "leave resets the region");
    }

    #[test]
    fn ris_resets_the_region_without_clearing_or_moving_anything() {
        // R4.3: the primary-screen escape hatch. `ESC c` must drop the region while
        // staying the deliberately-minimal RIS it already is — no clear, no move.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[?1h\x1b[?2004h"); // DECCKM + bracketed paste on
        p.advance(&mut g, b"\x1b[3;20r");
        p.advance(&mut g, b"\x1b[11;6Hmark");
        // R4.3 requires "every image placement is unchanged" — so there must BE one.
        let rgba: Vec<u8> = std::iter::repeat_n([0u8, 255, 0, 255], 1)
            .flatten()
            .collect();
        p.advance(&mut g, &kitty_apc(&rgba, 1, 1));
        assert_eq!(g.images().placements().len(), 1, "precondition: one image");
        let img_before = {
            let p0 = &g.images().placements()[0];
            (p0.col, p0.row)
        };
        let cursor_before = g.cursor();

        p.advance(&mut g, b"\x1bc");

        assert_eq!(g.scroll_region(), (0, 23), "RIS resets the region");
        assert_eq!(g.cursor(), cursor_before, "RIS does not move the cursor");
        assert_eq!(g.cell(5, 10).unwrap().ch, 'm', "RIS does not clear cells");
        assert_eq!(row_text(&g, 10).trim(), "mark");
        assert_eq!(
            g.images().placements().len(),
            1,
            "RIS does not drop image placements"
        );
        assert_eq!(
            (
                g.images().placements()[0].col,
                g.images().placements()[0].row
            ),
            img_before,
            "RIS does not move image placements"
        );
        assert!(!g.app_cursor_keys(), "RIS still clears DECCKM");
        assert!(!g.bracketed_paste(), "RIS still clears bracketed paste");
    }

    #[test]
    fn synchronized_update_is_tracked_inside_a_multi_parameter_decset() {
        // selection-scroll-anchor R5.1 — `?2026` sets and resets the flag, and is
        // honoured as the second parameter of a batched DECSET, not only alone.
        let mut g = grid();
        let mut p = Processor::new();
        assert!(!g.synchronized_update(), "fresh: closed");
        p.advance(&mut g, b"\x1b[?2026h");
        assert!(g.synchronized_update());
        p.advance(&mut g, b"\x1b[?2026l");
        assert!(!g.synchronized_update());
        p.advance(&mut g, b"\x1b[?1049;2026h");
        assert!(g.synchronized_update(), "the second parameter counts");
        assert!(g.alt_screen(), "and so does the first");
    }

    #[test]
    fn resize_resets_the_region_to_the_new_full_screen() {
        // R4.4: the region's bottom may no longer exist after a shrink. Dropping it
        // (rather than clamping) matches how resize already treats the selection.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[3;20r");
        assert_eq!(g.scroll_region(), (2, 19));

        g.resize(20, 10);
        assert_eq!(g.scroll_region(), (0, 9), "region follows the new height");
        assert!(g.region_is_full_screen());
    }

    #[test]
    fn only_decstbm_homes_the_cursor_across_the_four_reset_paths() {
        // R4.5: the boundary that justifies having both `set_` and `reset_`. Three
        // of the four reset paths must leave the cursor where it was.
        let place = |g: &mut Grid, p: &mut Processor| {
            p.advance(g, b"\x1b[3;20r\x1b[11;6H");
            assert_eq!(cur(g), (5, 10));
        };

        // 1. CSI r — homes.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        place(&mut g, &mut p);
        p.advance(&mut g, b"\x1b[r");
        assert_eq!(cur(&g), (0, 0), "CSI r homes");

        // 2. alt-screen enter — does not.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        place(&mut g, &mut p);
        p.advance(&mut g, b"\x1b[?1049h");
        assert_eq!(cur(&g), (5, 10), "alt-screen enter does not home");

        // 3. RIS — does not.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        place(&mut g, &mut p);
        p.advance(&mut g, b"\x1bc");
        assert_eq!(cur(&g), (5, 10), "RIS does not home");

        // 4. resize — clamps only (its own pre-existing rule), never homes. Uses a
        // real dimension change so the "subject to resize's existing clamp" half of
        // R4.5 is actually exercised, not a no-op resize.
        let mut g = Grid::new(20, 24, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        place(&mut g, &mut p);
        g.resize(30, 30); // grow: cursor keeps its position untouched
        assert_eq!(cur(&g), (5, 10), "growing resize does not home or move");
        g.resize(8, 8); // shrink past the cursor: clamped, still not homed
        assert_eq!(
            cur(&g),
            (5, 7),
            "shrinking resize clamps the row but does not home"
        );
    }

    // --- Region-aware scrolling: SU/SD/IL/DL/IND/RI (R2) ---

    /// A 6-row grid whose rows read `a`..`f`, with the scroll region set to rows
    /// 1..=3 — so rows 0, 4 and 5 are the "outside the region must not move" guard.
    fn banded_grid() -> (Grid, Processor) {
        let mut g = Grid::new(4, 6, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        for (i, ch) in "abcdef".chars().enumerate() {
            p.advance(&mut g, format!("\x1b[{};1H{ch}", i + 1).as_bytes());
        }
        p.advance(&mut g, b"\x1b[2;4r"); // region = rows 1..=3 (0-based)
        (g, p)
    }

    /// Every row of the grid, trimmed — the shape most region assertions want.
    fn rows_of(g: &Grid) -> Vec<String> {
        (0..g.rows()).map(|r| row_text(g, r)).collect()
    }

    #[test]
    fn line_feed_at_region_bottom_scrolls_only_the_region() {
        // R2.1: rows outside the band are untouched by a TUI's line feed.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[4;1H"); // cursor to row 3 = region bottom
        p.advance(&mut g, b"\n");
        assert_eq!(rows_of(&g), ["a", "c", "d", "", "e", "f"]);
        assert_eq!(g.cursor().row, 3, "cursor stays on the region bottom");
    }

    #[test]
    fn line_feed_below_the_region_does_not_scroll() {
        // A cursor parked below the region on the last screen row is outside the
        // scrolling band — it must not drag the whole screen up.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[6;1H"); // row 5, below the region
        p.advance(&mut g, b"\n");
        assert_eq!(rows_of(&g), ["a", "b", "c", "d", "e", "f"], "nothing moved");
    }

    #[test]
    fn su_scrolls_the_region_up_blanking_at_the_bottom() {
        // R2.2
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[1;1H"); // cursor outside the region — must not move
        p.advance(&mut g, b"\x1b[2S");
        assert_eq!(rows_of(&g), ["a", "d", "", "", "e", "f"]);
        assert_eq!(cur(&g), (0, 0), "SU does not move the cursor");
    }

    #[test]
    fn sd_scrolls_the_region_down_blanking_at_the_top() {
        // R2.3
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[1;1H");
        p.advance(&mut g, b"\x1b[2T");
        assert_eq!(rows_of(&g), ["a", "", "", "b", "e", "f"]);
        assert_eq!(cur(&g), (0, 0), "SD does not move the cursor");
    }

    #[test]
    fn il_inserts_blank_lines_from_the_cursor_to_the_region_bottom() {
        // R2.4: `e` is outside the region and must survive; `d` is pushed out of the
        // region's bottom and discarded.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[3;1H"); // cursor row 2, inside the region
        p.advance(&mut g, b"\x1b[1L");
        assert_eq!(rows_of(&g), ["a", "b", "", "c", "e", "f"]);
        assert_eq!(cur(&g), (0, 2), "IL leaves the cursor row AND column alone");
    }

    #[test]
    fn dl_deletes_lines_pulling_the_region_tail_up() {
        // R2.5
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[3;1H"); // cursor row 2
        p.advance(&mut g, b"\x1b[1M");
        assert_eq!(rows_of(&g), ["a", "b", "d", "", "e", "f"]);
        assert_eq!(cur(&g), (0, 2), "DL leaves the cursor alone");
    }

    #[test]
    fn il_and_dl_outside_the_region_are_no_ops() {
        // R2.6: the cursor is above the region — both sequences must do nothing at
        // all, rather than operating on the wrong band.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[1;1H"); // row 0, above the region
        p.advance(&mut g, b"\x1b[3L");
        assert_eq!(rows_of(&g), ["a", "b", "c", "d", "e", "f"], "IL ignored");
        p.advance(&mut g, b"\x1b[3M");
        assert_eq!(rows_of(&g), ["a", "b", "c", "d", "e", "f"], "DL ignored");
        assert_eq!(cur(&g), (0, 0));

        // Below the region too.
        p.advance(&mut g, b"\x1b[6;1H\x1b[2L\x1b[2M");
        assert_eq!(rows_of(&g), ["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn ind_indexes_within_the_region() {
        // R2.7: `ESC D` at the region bottom scrolls the band, like a line feed.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[4;1H"); // region bottom
        p.advance(&mut g, b"\x1bD");
        assert_eq!(rows_of(&g), ["a", "c", "d", "", "e", "f"]);
        assert_eq!(g.cursor().row, 3);

        // Above the bottom it just moves down.
        p.advance(&mut g, b"\x1b[2;1H\x1bD");
        assert_eq!(g.cursor().row, 2);
    }

    #[test]
    fn ri_at_the_region_top_scrolls_the_region_down() {
        // R2.8
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[2;1H"); // row 1 = region top
        p.advance(&mut g, b"\x1bM");
        assert_eq!(rows_of(&g), ["a", "", "b", "c", "e", "f"]);
        assert_eq!(g.cursor().row, 1, "RI at the top does not move the cursor");
    }

    #[test]
    fn ri_below_the_region_top_only_moves_the_cursor() {
        // R2.9
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[4;1H"); // row 3
        p.advance(&mut g, b"\x1bM");
        assert_eq!(g.cursor().row, 2);
        assert_eq!(rows_of(&g), ["a", "b", "c", "d", "e", "f"], "no cell moved");
    }

    #[test]
    fn scroll_counts_default_saturate_and_never_over_scroll() {
        // R2.10: no param and `0` both mean 1; a huge count blanks the band without
        // panicking or touching rows outside it.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[S");
        assert_eq!(rows_of(&g), ["a", "c", "d", "", "e", "f"], "bare S == 1");

        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[0S");
        assert_eq!(rows_of(&g), ["a", "c", "d", "", "e", "f"], "0 S == 1");

        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[9999S");
        assert_eq!(
            rows_of(&g),
            ["a", "", "", "", "e", "f"],
            "band blanked only"
        );

        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[9999T");
        assert_eq!(rows_of(&g), ["a", "", "", "", "e", "f"]);
    }

    #[test]
    fn rows_scrolled_into_the_region_use_the_pen_background() {
        // R2.11 / D6: a TUI paints a colored pane, then scrolls — the incoming row
        // must carry the pane color, not punch a default-colored hole.
        let (mut g, mut p) = banded_grid();
        p.advance(&mut g, b"\x1b[48;5;21m"); // blue pen background
        p.advance(&mut g, b"\x1b[1S");
        let incoming = *g.cell(0, 3).unwrap(); // new bottom row of the region
        assert_eq!(
            incoming.bg,
            ansi_256(21),
            "BCE: incoming row takes the pen bg"
        );
        assert_eq!(incoming.fg, Color(0xFFFFFF), "…and the DEFAULT fg");
        assert_eq!(incoming.ch, ' ');
    }

    // --- In-line character ops: ICH / DCH / ECH (R5) ---

    /// A 10x3 grid whose every row reads `abcdefghij`, with the cursor left on
    /// row 1 col 3 — so rows 0 and 2 are the line-scoping guard.
    fn char_op_grid() -> (Grid, Processor) {
        let mut g = Grid::new(10, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        for r in 1..=3 {
            p.advance(&mut g, format!("\x1b[{r};1Habcdefghij").as_bytes());
        }
        p.advance(&mut g, b"\x1b[2;4H"); // row 1, col 3
        (g, p)
    }

    #[test]
    fn ich_inserts_blanks_and_shifts_the_line_right() {
        // R5.1: two cells pushed past the right edge are discarded.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2@");
        assert_eq!(row_text(&g, 1), "abc  defgh");
        assert_eq!(cur(&g), (3, 1), "ICH does not move the cursor");
    }

    #[test]
    fn dch_deletes_and_pulls_the_tail_left() {
        // R5.2
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2P");
        assert_eq!(row_text(&g, 1), "abcfghij");
        assert_eq!(cur(&g), (3, 1), "DCH does not move the cursor");
    }

    #[test]
    fn ech_overwrites_in_place_without_shifting() {
        // R5.3: the distinguishing property — cols 6..=9 stay put.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[3X");
        assert_eq!(row_text(&g, 1), "abc   ghij");
        assert_eq!(g.cell(3, 1).unwrap().ch, ' ');
        assert_eq!(g.cell(5, 1).unwrap().ch, ' ');
        assert_eq!(g.cell(6, 1).unwrap().ch, 'g', "no shift");
        assert_eq!(cur(&g), (3, 1));
    }

    #[test]
    fn char_ops_are_strictly_line_scoped() {
        // R5.4: adjacent rows are byte-identical after every op.
        for seq in [
            b"\x1b[2@".as_slice(),
            b"\x1b[2P".as_slice(),
            b"\x1b[3X".as_slice(),
        ] {
            let (mut g, mut p) = char_op_grid();
            p.advance(&mut g, seq);
            assert_eq!(row_text(&g, 0), "abcdefghij", "{seq:?} left row 0 alone");
            assert_eq!(row_text(&g, 2), "abcdefghij", "{seq:?} left row 2 alone");
        }
    }

    #[test]
    fn char_op_blanks_use_the_pen_background() {
        // R5.5 / D6: the colored-pane idiom — set a bg, erase, keep the color.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[48;5;21m\x1b[3X");
        let c = *g.cell(3, 1).unwrap();
        assert_eq!(c.bg, ansi_256(21), "ECH fills with the pen bg");
        assert_eq!(c.fg, Color(0xFFFFFF), "…and the default fg");
        // ICH's inserted cells and DCH's shifted-in tail get the same treatment.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[48;5;21m\x1b[2@");
        assert_eq!(g.cell(3, 1).unwrap().bg, ansi_256(21), "ICH fill");
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[48;5;21m\x1b[2P");
        assert_eq!(
            g.cell(9, 1).unwrap().bg,
            ansi_256(21),
            "DCH right-edge fill"
        );
    }

    #[test]
    fn dch_repairs_a_wide_char_split_by_the_shift() {
        // R5.6: `ab한글cd` — 한 occupies cols 2-3 as leader+spacer. Deleting 3 cells
        // at col 0 shifts the tail left by an ODD amount, so the pairs land
        // half-overlapping. No spacer may survive without its leader.
        let mut g = Grid::new(12, 1, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, "ab한글cd".as_bytes());
        p.advance(&mut g, b"\x1b[1;1H"); // col 0
        p.advance(&mut g, b"\x1b[3P");

        for c in 0..g.cols() {
            let cell = *g.cell(c, 0).unwrap();
            if cell.is_continuation() {
                let leader = g.cell(c - 1, 0).unwrap().ch;
                assert_eq!(
                    leader.width().unwrap_or(0),
                    2,
                    "col {c}: spacer must follow a wide leader, found {leader:?}"
                );
            }
            if cell.ch.width().unwrap_or(0) == 2 {
                assert!(
                    g.cell(c + 1, 0).is_some_and(|n| n.is_continuation()),
                    "col {c}: wide leader {:?} lost its spacer",
                    cell.ch
                );
            }
        }
    }

    #[test]
    fn ich_repairs_a_wide_leader_whose_spacer_was_pushed_off() {
        // R5.7: the mirror case — an insert shoves the last pair's spacer past the
        // right edge, leaving a leader that would paint over its neighbour.
        let mut g = Grid::new(6, 1, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, "ab한글".as_bytes()); // cols 0,1 narrow; 2-3 한; 4-5 글
        p.advance(&mut g, b"\x1b[1;1H\x1b[1@"); // insert 1 at col 0

        let last = *g.cell(5, 0).unwrap();
        assert_ne!(
            last.ch.width().unwrap_or(0),
            2,
            "a wide leader must not survive in the last column without a spacer"
        );
        // And the surviving pair is still intact.
        assert_eq!(g.cell(3, 0).unwrap().ch, '한');
        assert!(g.cell(4, 0).unwrap().is_continuation());
    }

    #[test]
    fn char_op_counts_default_clamp_and_never_overrun() {
        // R5.8: bare and `0` mean 1; a huge count stops at the row's end.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2;9H"); // col 8, two cells from the edge
        p.advance(&mut g, b"\x1b[9999X");
        assert_eq!(row_text(&g, 1), "abcdefgh", "clamped to the row");
        assert_eq!(row_text(&g, 0), "abcdefghij", "and never spilled upward");
        assert_eq!(row_text(&g, 2), "abcdefghij", "or downward");

        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[P"); // bare DCH == 1
        assert_eq!(row_text(&g, 1), "abcefghij");
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[0P"); // 0 == 1
        assert_eq!(row_text(&g, 1), "abcefghij");
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[0@");
        assert_eq!(row_text(&g, 1), "abc defghi");
    }

    // --- EL parameter handling (R6) ---

    #[test]
    fn el_honors_its_parameter() {
        // R6.1/R6.2/R6.3 — the bug: `K` used to ignore its param and always erase
        // cursor→EOL, so `1K` and `2K` erased the wrong span.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2;5H"); // col 4
        p.advance(&mut g, b"\x1b[K");
        assert_eq!(row_text(&g, 1), "abcd", "0K: cursor→EOL");
        assert_eq!(cur(&g), (4, 1), "EL does not move the cursor");

        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2;5H\x1b[1K");
        assert_eq!(
            g.cell(4, 1).unwrap().ch,
            ' ',
            "1K erases the cursor cell too"
        );
        assert_eq!(row_text(&g, 1), "     fghij", "1K: start→cursor inclusive");

        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2;5H\x1b[2K");
        assert_eq!(row_text(&g, 1), "", "2K: the whole line");
        assert_eq!(row_text(&g, 0), "abcdefghij", "2K stays on its own row");
        assert_eq!(row_text(&g, 2), "abcdefghij");
        assert_eq!(cur(&g), (4, 1));
    }

    #[test]
    fn el_unknown_parameter_is_ignored() {
        // R6.4: matches how the `J` arm treats an unknown param.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[2;5H\x1b[7K");
        assert_eq!(row_text(&g, 1), "abcdefghij", "nothing erased");
    }

    #[test]
    fn el_fills_with_the_default_background_not_the_pen() {
        // R6.5 / Known Gap G-A: the deliberate divergence from R5.5. `K` is the
        // pre-existing erase and keeps default-bg fill; only the new ops do BCE.
        let (mut g, mut p) = char_op_grid();
        p.advance(&mut g, b"\x1b[48;5;21m\x1b[2;5H\x1b[2K");
        assert_eq!(
            g.cell(0, 1).unwrap().bg,
            Color(0),
            "EL keeps the DEFAULT bg while ECH takes the pen bg"
        );
    }

    // --- DECAWM + pending-wrap (R7) ---

    /// An 8x3 grid with all 8 columns of row 0 filled — the state that leaves the
    /// cursor resting ON the last column with its wrap deferred.
    fn wrapped_grid() -> (Grid, Processor) {
        let mut g = Grid::new(8, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"abcdefgh");
        (g, p)
    }

    #[test]
    fn printing_into_the_last_column_defers_the_wrap() {
        // R7.1 — the property eager wrapping could not express. Before this, the 8th
        // character moved the cursor to row 1, so a full-width line ate a blank row.
        let (g, _) = wrapped_grid();
        assert_eq!(row_text(&g, 0), "abcdefgh", "all 8 chars on row 0");
        assert_eq!(row_text(&g, 1), "", "row 1 is still untouched");
        assert_eq!(cur(&g), (7, 0), "cursor rests ON the last column");
        assert!(g.pending_wrap(), "…with the wrap deferred");
    }

    #[test]
    fn the_next_printable_character_performs_the_deferred_wrap() {
        // R7.2
        let (mut g, mut p) = wrapped_grid();
        p.advance(&mut g, b"i");
        assert_eq!(row_text(&g, 0), "abcdefgh", "row 0 unchanged");
        assert_eq!(row_text(&g, 1), "i", "the wrap happened now, not earlier");
        assert_eq!(cur(&g), (1, 1));
        assert!(!g.pending_wrap(), "flag consumed");
    }

    #[test]
    fn el_while_pending_erases_only_the_last_column() {
        // R7.3 — the concrete scenario from decision D3. The erase must not be
        // treated as a reposition, or it would wipe the wrong cell.
        let (mut g, mut p) = wrapped_grid();
        p.advance(&mut g, b"\x1b[K");
        assert_eq!(row_text(&g, 0), "abcdefg", "only col 7 was blanked");
        assert_eq!(g.cell(7, 0).unwrap().ch, ' ');
        assert_eq!(cur(&g), (7, 0), "EL does not move the cursor");
        assert!(g.pending_wrap(), "erases do NOT clear the pending wrap");
    }

    #[test]
    fn explicit_repositioning_clears_the_pending_wrap() {
        // R7.4 — every clear-path from D8, each from a fresh pending state, each
        // proved by the fact that the NEXT character does not wrap.
        let cases: [(&[u8], &str); 7] = [
            (b"\r", "carriage return"),
            (b"\x08", "backspace"),
            (b"\t", "tab"),
            (b"\x1b[1;1H", "CUP"),
            (b"\x1b[A", "CUU"),
            (b"\x1b[5G", "CHA"),
            (b"\x1b[1d", "VPA"),
        ];
        for (seq, name) in cases {
            let (mut g, mut p) = wrapped_grid();
            p.advance(&mut g, seq);
            assert!(!g.pending_wrap(), "{name} must clear the pending wrap");
            let row_before = g.cursor().row;
            p.advance(&mut g, b"Z");
            assert_eq!(
                g.cursor().row,
                row_before,
                "{name}: the next character must not wrap to a new row"
            );
        }

        // A line feed clears it as well (it is a reposition, and it already moved).
        let (mut g, mut p) = wrapped_grid();
        p.advance(&mut g, b"\n");
        assert!(!g.pending_wrap(), "line feed clears the pending wrap");

        // …and so does a resize: the last column just moved, so a retained flag
        // would fire a stray wrap on a cursor no longer at the margin.
        let (mut g, _p) = wrapped_grid();
        assert!(g.pending_wrap());
        g.resize(12, 3);
        assert!(!g.pending_wrap(), "resize clears the pending wrap");
    }

    #[test]
    fn decawm_off_overwrites_the_last_column_in_place() {
        // R7.5: with `?7l` the last column becomes a sink — chars 8, 9 and 10 each
        // overwrite it and the screen never scrolls.
        let mut g = Grid::new(8, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[?7l");
        assert!(!g.autowrap());
        p.advance(&mut g, b"abcdefghij"); // 10 chars into 8 columns

        assert_eq!(row_text(&g, 0), "abcdefgj", "h, i, j all landed on col 7");
        assert_eq!(row_text(&g, 1), "", "nothing wrapped");
        assert_eq!(cur(&g), (7, 0));
        assert!(
            !g.pending_wrap(),
            "the flag is never set while autowrap is off"
        );
    }

    #[test]
    fn decawm_back_on_restores_wrapping() {
        // R7.6
        let mut g = Grid::new(8, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[?7labcdefghij");
        p.advance(&mut g, b"\x1b[?7h");
        assert!(g.autowrap());

        p.advance(&mut g, b"X"); // re-fills col 7 and arms the deferred wrap
        assert_eq!(g.cell(7, 0).unwrap().ch, 'X');
        assert!(g.pending_wrap());
        p.advance(&mut g, b"Y"); // now it wraps
        assert_eq!(row_text(&g, 1), "Y");
        assert_eq!(cur(&g), (1, 1));
    }

    #[test]
    fn a_pending_wrap_is_retained_but_not_honored_while_autowrap_is_off() {
        // R7.7 / D8: `?7l` does not clear the flag — DEC retention semantics. What
        // it changes is whether the flag is *honored*.
        let (mut g, mut p) = wrapped_grid();
        assert!(g.pending_wrap());
        p.advance(&mut g, b"\x1b[?7l");
        assert!(
            g.pending_wrap(),
            "the mode change itself must not clear the flag"
        );

        p.advance(&mut g, b"Z");
        assert_eq!(
            row_text(&g, 1),
            "",
            "no wrap happened while autowrap was off"
        );
        assert_eq!(
            g.cell(7, 0).unwrap().ch,
            'Z',
            "it overwrote the last column"
        );
        assert_eq!(cur(&g), (7, 0));
    }

    #[test]
    fn dl_at_the_region_top_retains_history_exactly_like_su() {
        // Gate-3 finding B4: `ESC[1M` and `ESC[1S` at row 0 on a full-screen primary
        // region produce byte-identical grids, so they must agree on history too —
        // otherwise content deleted off row 0 vanishes from scrollback for good.
        let build = || {
            let mut g = Grid::new(4, 3, Color(0xFFFFFF), Color(0));
            let mut p = Processor::new();
            p.advance(&mut g, b"0\r\n1\r\n2\x1b[1;1H");
            (g, p)
        };
        let (mut su_g, mut su_p) = build();
        let (mut dl_g, mut dl_p) = build();
        su_p.advance(&mut su_g, b"\x1b[1S");
        dl_p.advance(&mut dl_g, b"\x1b[1M");

        assert_eq!(rows_of(&su_g), rows_of(&dl_g), "precondition: same grid");
        assert_eq!(su_g.history_len(), 1, "SU retires the top row");
        assert_eq!(
            dl_g.history_len(),
            su_g.history_len(),
            "DL at the region top must retire it too"
        );

        // Below the region top nothing leaves the screen, so nothing is retained.
        let (mut g, mut p) = build();
        p.advance(&mut g, b"\x1b[2;1H\x1b[1M");
        assert_eq!(g.history_len(), 0, "DL below the top retains nothing");

        // A partial region never retains, wherever the cursor sits (D1).
        let (mut g, mut p) = build();
        p.advance(&mut g, b"\x1b[1;2r\x1b[1;1H\x1b[1M");
        assert_eq!(g.history_len(), 0, "partial region retains nothing");
    }

    #[test]
    fn a_wide_char_that_cannot_fit_is_dropped_when_autowrap_is_off() {
        // Gate-3 finding B5. With `?7l` there is nowhere to wrap to, and the glyph
        // must NOT back the cursor up to make room — that destroyed the cell to the
        // left of the margin (`abcdefgh` became `abcdef한`, losing `g`).
        let mut g = Grid::new(8, 2, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[?7labcdefgh");
        p.advance(&mut g, "한".as_bytes());

        assert_eq!(row_text(&g, 0), "abcdefgh", "the row is untouched");
        assert_eq!(g.cell(6, 0).unwrap().ch, 'g', "the neighbour survived");
        assert_eq!(cur(&g), (7, 0), "the cursor stayed at the margin");
        assert_eq!(row_text(&g, 1), "", "and nothing wrapped");

        // A grid narrower than the glyph drops it rather than emitting a wide
        // leader with no continuation spacer.
        let mut tiny = Grid::new(1, 2, Color(0xFFFFFF), Color(0));
        let mut tp = Processor::new();
        tp.advance(&mut tiny, "한한".as_bytes());
        // Assert it landed NOWHERE — checking only (0,0) would miss the glyph
        // wrapping onto row 1, which is exactly what happens without the guard.
        for r in 0..tiny.rows() {
            for c in 0..tiny.cols() {
                let cell = *tiny.cell(c, r).unwrap();
                assert_eq!(
                    cell.ch, ' ',
                    "({c},{r}): a 1-column grid must drop a wide char entirely, \
                     not place a leader with no continuation spacer"
                );
            }
        }
    }

    /// Assert no double-width pair on `row` is left with one half orphaned.
    fn assert_wide_pairs_intact(g: &Grid, row: u16, ctx: &str) {
        for c in 0..g.cols() {
            let cell = *g.cell(c, row).unwrap();
            if cell.is_continuation() {
                assert!(
                    c > 0 && g.cell(c - 1, row).unwrap().ch.width().unwrap_or(0) == 2,
                    "{ctx}: col {c} is a spacer with no wide leader to its left"
                );
            }
            if cell.ch.width().unwrap_or(0) == 2 {
                assert!(
                    g.cell(c + 1, row).is_some_and(|n| n.is_continuation()),
                    "{ctx}: col {c} holds wide {:?} but its spacer is gone — the \
                     renderer would paint it across two cells",
                    cell.ch
                );
            }
        }
    }

    // --- DECSC / DECRC (R10) ---

    #[test]
    fn decsc_decrc_round_trip_cursor_and_pen() {
        // R10.1/R10.2: `ESC 7` snapshots position + pen; `ESC 8` restores both.
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[6;6H\x1b[38;5;196;48;5;21;1m"); // (5,5), red on blue, bold
        p.advance(&mut g, b"\x1b7"); // save

        p.advance(&mut g, b"\x1b[1;1H\x1b[0m"); // move home, reset the pen
        assert_eq!(cur(&g), (0, 0));

        p.advance(&mut g, b"\x1b8"); // restore
        assert_eq!(cur(&g), (5, 5), "DECRC restores the position");
        p.advance(&mut g, b"X");
        let c = *g.cell(5, 5).unwrap();
        assert_eq!(c.fg, ansi_256(196), "DECRC restores the pen fg");
        assert_eq!(c.bg, ansi_256(21), "…and the pen bg");
        assert!(c.attrs.contains(CellAttrs::BOLD), "…and the attributes");
    }

    #[test]
    fn decrc_without_a_prior_decsc_homes_the_cursor() {
        // R10.3: the VT default — a restore with nothing saved homes and resets.
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[6;6H\x1b[1m");
        p.advance(&mut g, b"\x1b8");
        assert_eq!(cur(&g), (0, 0));
        p.advance(&mut g, b"X");
        assert!(
            !g.cell(0, 0).unwrap().attrs.contains(CellAttrs::BOLD),
            "the pen resets to defaults too"
        );
    }

    #[test]
    fn decrc_clamps_a_saved_position_past_a_resize() {
        // R10.4: the grid may shrink between save and restore — the restored
        // position must stay in bounds rather than index out of the cell buffer.
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[9;19H\x1b7"); // save (18, 8)
        g.resize(5, 4);
        p.advance(&mut g, b"\x1b8");
        assert_eq!(cur(&g), (4, 3), "clamped to the new bounds");
        p.advance(&mut g, b"X"); // must not panic
    }

    #[test]
    fn claude_codes_save_reset_region_restore_idiom_keeps_the_cursor() {
        // R10.5 — the reason this shipped. Claude Code emits, verbatim:
        //     ESC 7   ESC [ r   ESC 8
        // save cursor, reset the scroll region, restore cursor. `CSI r` homes the
        // cursor (D9), so without DECRC the cursor was stranded at the origin —
        // strictly worse than before `CSI r` was implemented at all, since the whole
        // idiom used to be a no-op.
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[6;6H");
        let before = g.cursor();
        p.advance(&mut g, b"\x1b7\x1b[r\x1b8");
        assert_eq!(g.cursor(), before, "the cursor survives the idiom");
        assert_eq!(g.scroll_region(), (0, 9), "and the region really did reset");
    }

    #[test]
    fn ansi_save_restore_variants_and_the_decslrm_guard() {
        // R10.6: `CSI s`/`CSI u` are the ANSI spelling. But `CSI l;r s` is DECSLRM
        // (left/right margins), which we do not implement — it must NOT be misread
        // as a save, or a margin-setting app would corrupt the saved cursor.
        let mut g = Grid::new(20, 10, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[6;6H\x1b[s"); // save via CSI s
        p.advance(&mut g, b"\x1b[1;1H\x1b[u"); // restore via CSI u
        assert_eq!(cur(&g), (5, 5));

        // A parameterised `CSI s` is DECSLRM: ignored, and it must not overwrite
        // the saved position.
        p.advance(&mut g, b"\x1b[3;3H\x1b[2;10s"); // DECSLRM-shaped — not a save
        p.advance(&mut g, b"\x1b[1;1H\x1b[u");
        assert_eq!(cur(&g), (5, 5), "DECSLRM must not clobber the saved cursor");
    }

    #[test]
    fn printing_over_half_a_wide_pair_breaks_the_pair() {
        // Gate-3 re-check finding N1. `print` was the only row mutator that never
        // maintained the leader+spacer invariant, so overwriting one half left the
        // other stranded: `abcdef한` + `Z` produced `abcdef한Z` — nine display
        // columns painted into an eight-cell row, with 한 drawn on top of Z.
        //
        // `?7l` is what makes this reachable from plain text with no cursor motion:
        // the cursor parks ON the spacer at the margin instead of wrapping away.
        let mut g = Grid::new(8, 2, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[?7labcdef");
        p.advance(&mut g, "한".as_bytes()); // leader col 6, spacer col 7
        p.advance(&mut g, b"Z"); // overwrites the spacer
        assert_wide_pairs_intact(&g, 0, "print over a spacer");
        assert_eq!(g.cell(7, 0).unwrap().ch, 'Z');
        assert_eq!(
            row_text(&g, 0),
            "abcdef Z",
            "the orphaned leader was blanked"
        );

        // The mirror case: overwrite the LEADER and the spacer must go too.
        let mut g = Grid::new(8, 2, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, "ab한cd".as_bytes()); // 한 at cols 2-3
        p.advance(&mut g, b"\x1b[1;3HX"); // overwrite the leader at col 2
        assert_wide_pairs_intact(&g, 0, "print over a leader");
        assert_eq!(row_text(&g, 0), "abX cd", "the orphaned spacer was blanked");
    }

    #[test]
    fn a_glyph_too_wide_for_the_grid_is_dropped_without_scrolling() {
        // Gate-3 re-check finding N2: the oversize guard sat AFTER the deferred-wrap
        // consumption, so a glyph that was never drawn still scrolled the screen and
        // retired a row to scrollback.
        let mut g = Grid::new(1, 1, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"a"); // arms the deferred wrap on a 1x1 grid
        assert_eq!(g.history_len(), 0);
        p.advance(&mut g, "한".as_bytes()); // dropped — must not scroll
        assert_eq!(
            g.history_len(),
            0,
            "a dropped glyph must not scroll the screen"
        );
        assert_eq!(
            g.cell(0, 0).unwrap().ch,
            'a',
            "and must not disturb the cell"
        );
    }

    #[test]
    fn every_repositioning_path_clears_the_pending_wrap() {
        // R7.4, widened after a Gate-3 review found three repositioning paths that
        // bypassed `move_to` and left a deferred wrap armed. Each one made the next
        // character land a row low, clobbering whatever was there.
        //
        // The shape of the bug, for RI: print 8 chars (pending armed at col 7), RI
        // to row 0, print `Z` → `Z` overwrote (0,1) instead of landing at (7,0).
        let cases: [(&[u8], &str); 5] = [
            (b"\x1bM", "RI (ESC M)"),
            (b"\x1b[2;5r", "DECSTBM with params"),
            (b"\x1b[r", "bare DECSTBM"),
            (b"\x1b[2J", "ED2"),
            (b"\x1b[3J", "ED3"),
        ];
        for (seq, name) in cases {
            let mut g = Grid::new(8, 6, Color(0xFFFFFF), Color(0));
            let mut p = Processor::new();
            p.advance(&mut g, b"\x1b[2;1Habcdefgh"); // row 1 full, pending armed
            assert!(g.pending_wrap(), "{name}: precondition");
            p.advance(&mut g, seq);
            assert!(!g.pending_wrap(), "{name} must clear the pending wrap");

            // The observable consequence: the next char must not wrap.
            let before = g.cursor();
            p.advance(&mut g, b"Z");
            assert_eq!(
                g.cursor().row,
                before.row,
                "{name}: next char wrapped to a new row"
            );
            assert_eq!(
                g.cell(before.col, before.row).unwrap().ch,
                'Z',
                "{name}: Z landed somewhere other than the cursor"
            );
        }
    }

    #[test]
    fn a_wide_char_that_cannot_fit_the_last_column_still_wraps() {
        // R7.8: the pre-existing "a wide char never straddles the right margin" rule
        // must survive the pending-wrap rewrite. Reached via a cursor move, so the
        // deferred flag is clear and the fit check is what does the work.
        let mut g = Grid::new(8, 3, Color(0xFFFFFF), Color(0));
        let mut p = Processor::new();
        p.advance(&mut g, b"\x1b[1;8H"); // col 7, pending clear
        assert!(!g.pending_wrap());
        p.advance(&mut g, "한".as_bytes());

        assert_eq!(g.cell(0, 1).unwrap().ch, '한', "wrapped to the next row");
        assert!(g.cell(1, 1).unwrap().is_continuation(), "with its spacer");
        assert_eq!(g.cursor().row, 1);
        assert_eq!(g.cursor().col, 2);
    }

    #[test]
    fn line_feed_and_ind_both_apply_background_color_erase() {
        // D6/R2.11, corrected after a Gate-3 review: LF and IND are the same index
        // operation and must fill identically, and xterm applies `bce` to both. An
        // earlier revision filled them from the default background, which gave a TUI
        // streaming into a colored pane black rows on every scroll — the most common
        // scroll path of all. R3.4 does not constrain fill colour.
        for seq in [b"\n".as_slice(), b"\x1bD".as_slice()] {
            let mut g = Grid::new(4, 2, Color(0xFFFFFF), Color(0));
            let mut p = Processor::new();
            p.advance(&mut g, b"\x1b[48;5;21m"); // blue pen background
            p.advance(&mut g, b"\x1b[2;1Hx"); // park on the last row…
            p.advance(&mut g, seq); // …and scroll it
            assert_eq!(
                g.cell(1, 1).unwrap().bg,
                ansi_256(21),
                "{seq:?} must fill the exposed row from the pen (BCE)"
            );
            assert_eq!(
                g.cell(1, 1).unwrap().fg,
                Color(0xFFFFFF),
                "{seq:?} keeps the DEFAULT foreground"
            );
        }
    }
}
