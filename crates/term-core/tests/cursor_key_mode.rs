//! DECCKM (application cursor key mode) tracking, headless.
//!
//! `term-core` owns the mode; the app reads it to pick the SS3 (`ESC O A`) vs CSI
//! (`ESC [ A`) arrow encoding. No GPU, no window — the whole point of keeping the
//! engine dependency-free (C4). Mirrors the shape of the alt-screen toggle test in
//! `scrollback.rs`.

use term_core::{Color, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term() -> Terminal {
    Terminal::new(20, 5, FG, BG)
}

/// R1.3 — a fresh terminal is in normal cursor key mode.
#[test]
fn default_is_normal_cursor_keys() {
    assert!(
        !term().app_cursor_keys(),
        "DECCKM starts off; zsh never turns it on (measured), so normal CSI arrows \
         are the primary path"
    );
}

/// R1.1 / R1.2 — `?1h` sets, `?1l` resets.
#[test]
fn decckm_set_and_reset() {
    let mut t = term();

    t.advance(b"\x1b[?1h");
    assert!(
        t.app_cursor_keys(),
        "?1h enters application cursor key mode"
    );

    t.advance(b"\x1b[?1l");
    assert!(
        !t.app_cursor_keys(),
        "?1l returns to normal cursor key mode"
    );
}

/// R1.4 — neighbouring private modes must not disturb DECCKM. `?2004`
/// (bracketed paste) matters concretely: zsh emits it on *every* prompt cycle,
/// so a sloppy private-mode arm would flip DECCKM constantly in normal use.
#[test]
fn other_private_modes_leave_decckm_alone() {
    let mut t = term();
    t.advance(b"\x1b[?1h");

    t.advance(b"\x1b[?1049h\x1b[?2004h\x1b[?1049l\x1b[?2004l");

    assert!(
        t.app_cursor_keys(),
        "alt-screen and bracketed-paste toggles must not touch DECCKM"
    );
}

/// R1.4 (converse) — DECCKM must not disturb the alt-screen flag either.
#[test]
fn decckm_leaves_alt_screen_alone() {
    let mut t = term();
    t.advance(b"\x1b[?1049h");

    t.advance(b"\x1b[?1h\x1b[?1l");

    assert!(
        t.alt_screen(),
        "?1h/?1l must not leave the alternate screen"
    );
}

/// R1.5 — the flag is keyboard-input state, not screen content: it survives a
/// resize, a full erase, and alt-screen switching. Pins the property so a future
/// "rebuild the Grid on resize" refactor can't silently revert the arrow fix.
#[test]
fn decckm_survives_resize_clear_and_alt_screen() {
    let mut t = term();
    t.advance(b"\x1b[?1h");

    t.resize(40, 10);
    assert!(t.app_cursor_keys(), "resize must not reset DECCKM");

    t.advance(b"\x1b[2J");
    assert!(
        t.app_cursor_keys(),
        "ED2 (erase display) must not reset DECCKM"
    );

    t.advance(b"\x1b[?1049h");
    assert!(
        t.app_cursor_keys(),
        "entering alt-screen must not reset DECCKM"
    );

    t.advance(b"\x1b[?1049l");
    assert!(
        t.app_cursor_keys(),
        "leaving alt-screen must not reset DECCKM"
    );
}

/// R1.7 — the mode is per-`Terminal`, hence per-tab: each tab owns its own zsh,
/// so a TUI in one tab must not change how arrows encode in another.
#[test]
fn decckm_is_per_terminal() {
    let mut a = term();
    let b = term();

    a.advance(b"\x1b[?1h");

    assert!(a.app_cursor_keys());
    assert!(
        !b.app_cursor_keys(),
        "a sibling tab keeps normal cursor keys"
    );
}

/// R1.6 — DECCKM is *not* part of the renderer contract. The snapshot is what the
/// renderer consumes; toggling a keyboard mode must produce an identical frame.
#[test]
fn decckm_does_not_alter_the_snapshot() {
    let mut t = term();
    t.advance(b"hello");
    let before = t.snapshot();

    t.advance(b"\x1b[?1h");
    let after = t.snapshot();

    assert_eq!(
        before, after,
        "DECCKM lives behind Terminal::app_cursor_keys(), never in GridSnapshot"
    );
}

// --- RIS and the rest of the ESC dispatch surface (R2, D9) ---

/// Build a Kitty transmit-and-display APC (`ESC _ G … ; b64 ESC \`) — same shape
/// as the helper in `parser.rs`'s own tests. The cheapest way to get a real
/// inline image placement into the grid, which is what RIS must not destroy.
fn kitty_apc(rgba: &[u8], s: u32, v: u32) -> Vec<u8> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(rgba);
    let mut out = b"\x1b_G".to_vec();
    out.extend_from_slice(format!("f=32,s={s},v={v},c=1,r=1;").as_bytes());
    out.extend_from_slice(b64.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

/// R2.1 — RIS (`ESC c`) returns DECCKM to normal. Without this the flag is a
/// one-way door: zsh never sends `?1l` (measured), so a TUI that dies mid-session
/// leaves the tab in application mode with no way back but closing it.
#[test]
fn ris_resets_decckm() {
    let mut t = term();
    t.advance(b"\x1b[?1h");

    t.advance(b"\x1bc");

    assert!(
        !t.app_cursor_keys(),
        "`reset` / `tput reset` sends RIS — that is the recovery path DECCKM's \
         persistence assumes"
    );
}

/// R2.2 — RIS is deliberately *minimal*: DECCKM and nothing else. A full RIS
/// would go through `Grid::clear()`, which also drops every inline image
/// placement, and would force-leave an alt-screen that here is a history
/// switch, not a second buffer — turning two bytes any program can emit into a
/// destructive primitive that does not exist today.
#[test]
fn ris_leaves_screen_images_alt_screen_and_pen_alone() {
    let mut t = term();
    let rgba: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 255], 4)
        .flatten()
        .collect();

    t.advance(b"\x1b[?1049h"); // alt-screen
    t.advance(b"\x1b[?1h"); // DECCKM — the one thing RIS may touch
    t.advance(b"\x1b[31m"); // red pen
    t.advance(b"hi");
    t.advance(&kitty_apc(&rgba, 2, 2));

    let before = t.snapshot();
    assert!(
        !before.images.is_empty(),
        "fixture guard: the APC actually placed an image to protect"
    );

    t.advance(b"\x1bc");
    let after = t.snapshot();

    assert_eq!(
        before.to_text(),
        after.to_text(),
        "RIS must not erase cells — Grid::clear() is deliberately not called"
    );
    assert_eq!(
        after.images.len(),
        before.images.len(),
        "Grid::clear() also clears the image layer; RIS must not reach it"
    );
    assert!(
        t.alt_screen(),
        "forcing out of alt-screen mid-vim would strand its frames in the main \
         grid and pollute scrollback"
    );
    assert!(!t.app_cursor_keys(), "DECCKM is the only state RIS resets");

    // The pen survives too: a cell printed *after* the RIS still carries the SGR
    // color set before it. Compared against the default fg first, so the equality
    // below can't pass by both cells being unpainted.
    t.advance(b"X");
    let painted = t.snapshot();
    assert_ne!(
        painted.cell(0, 0).map(|c| c.fg),
        Some(FG),
        "fixture guard: SGR 31 really did paint the pre-RIS text"
    );
    assert_eq!(
        painted.cell(2, 0).map(|c| c.fg),
        painted.cell(0, 0).map(|c| c.fg),
        "RIS does not reset the pen — output after it keeps the SGR color"
    );
}

/// R2.3 — `ESC =` / `ESC >` (DECKPAM/DECKPNM) reach the new `esc_dispatch` for
/// real: vim sends them as part of terminfo `smkx`/`rmkx`. DECNKM is a declared
/// non-goal, so they must do nothing at all rather than fall into a loose arm.
#[test]
fn keypad_mode_escapes_are_inert() {
    let mut t = term();
    t.advance(b"\x1b[?1h");
    t.advance(b"hello");
    let before = t.snapshot();

    t.advance(b"\x1b=\x1b>");

    assert!(
        t.app_cursor_keys(),
        "DECKPAM/DECKPNM are keypad mode, not cursor-key mode — they must not \
         reset DECCKM"
    );
    assert_eq!(
        before,
        t.snapshot(),
        "an unimplemented ESC sequence leaves the grid byte-identical"
    );
}

/// R2.4 — the RIS arm fires only on `intermediates.is_empty() && byte == b'c'`.
/// `ESC ( c` is a charset selection that happens to share the final byte; a
/// looser match would reset DECCKM on a sequence that has nothing to do with it.
#[test]
fn escape_with_intermediate_is_not_ris() {
    let mut t = term();
    t.advance(b"\x1b[?1h");

    t.advance(b"\x1b(c");

    assert!(
        t.app_cursor_keys(),
        "an intermediate byte means this is not RIS — only a bare `ESC c` is"
    );
}

/// R2.5 — a PTY read can split anywhere, and `Processor::advance` hand-scans APC
/// out of the stream: a lone trailing `ESC` is parked in its own state until the
/// next chunk. Verify the byte really is handed back to vte across that boundary,
/// otherwise RIS would work only when it happens to land inside one read.
#[test]
fn ris_split_across_advance_calls_still_resets() {
    let mut t = term();
    t.advance(b"\x1b[?1h");

    t.advance(b"\x1b"); // parked by the APC pre-scanner…
    t.advance(b"c"); // …and must be replayed to vte here

    assert!(
        !t.app_cursor_keys(),
        "RIS must survive the chunk boundary the APC scanner introduces"
    );
}
