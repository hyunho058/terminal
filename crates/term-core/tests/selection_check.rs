//! Headless tests for the selection change check (`specs/selection-scroll-anchor`
//! D4/D5/D6): a selection whose selected characters change is cleared, at frame
//! boundaries only, and copy re-verifies on its own.
//!
//! Requirements that need crate-internal access — the test-only history limit
//! (R4.5, R6.5), the reference's size (R4.9) or the sync flag (R5.1, R5.6, R5.7) —
//! are unit tests in `src/lib.rs`, `src/grid.rs` and `src/parser.rs`. Every comment
//! names `scroll-anchor Rx.y`, after the spec, as `tests/selection.rs` does.

use std::time::Instant;

use term_core::{Color, Selection, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

/// The spec's fixture: a 10×3 terminal reading `alpha` / `bravo` / `charlie` on
/// rows 0–2, at the live bottom with nothing in history yet.
fn bravo_term() -> Terminal {
    let mut t = Terminal::new(10, 3, FG, BG);
    t.advance(b"alpha\r\nbravo\r\ncharlie");
    t
}

/// Select viewport cells `anchor`..`focus` the way the app does: each cell becomes
/// a content point at the current offset.
fn select(t: &mut Terminal, anchor: (u16, u16), focus: (u16, u16)) {
    let a = t.point_at(anchor.0, anchor.1);
    let f = t.point_at(focus.0, focus.1);
    t.set_selection(a, f);
}

/// `bravo` selected as `(0,1)–(4,1)`, the spec's default fixture.
fn bravo_selected() -> Terminal {
    let mut t = bravo_term();
    select(&mut t, (0, 1), (4, 1));
    assert_eq!(t.selection_text().as_deref(), Some("bravo"), "precondition");
    t
}

/// A settle at an arbitrary fixed instant — what `Session::drain` does once after
/// its chunk loop. Only R5.6-style tests care which instant.
fn settle(t: &mut Terminal) {
    t.settle_selection(Instant::now());
}

// ---- scroll-anchor R0: the reported defect, end to end ----

#[test]
fn shell_output_moves_the_selection_and_copy_returns_its_text() {
    // scroll-anchor R0.1 — the defect read `"charl"`, the text that moved under a
    // viewport-anchored highlight. Anchored to content, it rides up with `bravo`,
    // and the settle that follows the drain keeps it: the text did not change.
    let mut t = bravo_selected();
    t.advance(b"\r\ndelta");
    settle(&mut t);
    assert_eq!(
        t.snapshot().selection,
        Some(Selection {
            anchor: (0, 0),
            focus: (4, 0)
        })
    );
    assert_eq!(t.selection_text().as_deref(), Some("bravo"));
}

#[test]
fn a_claude_code_shaped_repaint_clears_the_selection() {
    // scroll-anchor R0.2 — Claude Code's shape, measured: alt screen, mouse
    // tracking on, every frame wrapped in `?2026h … ?2026l`, and a scroll is a
    // repaint of every row shifted by one. Row 1 now reads `line-3`, so the
    // selection is cleared at the frame end — no settle needed.
    let mut t = Terminal::new(20, 5, FG, BG);
    t.advance(b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
    for row in 1..=5u8 {
        t.advance(format!("\x1b[{row};1Hline-{row}").as_bytes());
    }
    select(&mut t, (0, 1), (5, 1));
    assert_eq!(
        t.selection_text().as_deref(),
        Some("line-2"),
        "precondition"
    );

    let mut frame = b"\x1b[?2026h".to_vec();
    for row in 1..=5u8 {
        frame.extend_from_slice(format!("\x1b[{row};1Hline-{}", row + 1).as_bytes());
    }
    frame.extend_from_slice(b"\x1b[?2026l");
    t.advance(&frame);

    assert_eq!(t.selection(), None);
    assert_eq!(t.selection_text(), None);
}

// ---- scroll-anchor R4: changed text clears; terminal-owned motion does not ----

#[test]
fn a_full_screen_scroll_keeps_the_selection() {
    // scroll-anchor R4.1 — three lines push `bravo` (and the rest) into history at
    // the same line numbers; history rows are never rewritten, so the referenced
    // cells compare equal and the selection stays.
    let mut t = bravo_selected();
    t.advance(b"\r\nd\r\ne\r\nf");
    assert!(
        !t.snapshot().to_text().contains("bravo"),
        "precondition: `bravo` scrolled into history"
    );
    settle(&mut t);
    assert!(t.selection().is_some());
    assert_eq!(t.selection_text().as_deref(), Some("bravo"));
}

#[test]
fn an_in_place_repaint_of_a_selected_cell_clears_it() {
    // scroll-anchor R4.2.
    let mut t = bravo_selected();
    t.advance(b"\x1b[2;1Hx");
    settle(&mut t);
    assert_eq!(t.selection(), None);
}

#[test]
fn a_change_outside_the_selected_columns_does_not_clear() {
    // scroll-anchor R4.3 — `bra` is cols 0–2 of row 1; `zzz` lands on cols 7–9 of
    // the same line. Only the selected cells are referenced.
    let mut t = bravo_term();
    select(&mut t, (0, 1), (2, 1));
    t.advance(b"\x1b[2;8Hzzz");
    settle(&mut t);
    assert!(t.selection().is_some());
    assert_eq!(t.selection_text().as_deref(), Some("bra"));
}

#[test]
fn a_change_left_of_the_start_column_does_not_clear() {
    // scroll-anchor R4.3 (the other edge) — a selection starting mid-line, at col
    // 2, ignores a change at col 1: the reference starts at the start column, not
    // at the line's first.
    let mut t = bravo_term();
    select(&mut t, (2, 1), (4, 1));
    t.advance(b"\x1b[2;2HX");
    settle(&mut t);
    assert_eq!(t.selection_text().as_deref(), Some("avo"));
    // …while a change at the start column itself is seen.
    t.advance(b"\x1b[2;3HX");
    settle(&mut t);
    assert_eq!(t.selection(), None);
}

#[test]
fn a_colour_only_change_does_not_clear() {
    // scroll-anchor R4.4 — the same characters repainted red. Apps restyle text
    // (hover, dim, focus) without changing it, and copy is unaffected.
    let mut t = bravo_selected();
    t.advance(b"\x1b[2;1H\x1b[31mbravo");
    let cell = t.snapshot().cell(0, 1).copied().expect("row 1 exists");
    assert_eq!(cell.ch, 'b', "precondition: the same character…");
    assert_ne!(cell.fg, FG, "precondition: …in a different colour");
    settle(&mut t);
    assert!(t.selection().is_some());
    assert_eq!(t.selection_text().as_deref(), Some("bravo"));
}

/// A 10×3 terminal with `l0`…`l4` in history and `l5`…`l7` live, scrolled back so
/// the viewport shows `l1`…`l3`, with `l1`–`l2` selected: wholly in history.
fn history_only_selection() -> Terminal {
    let mut t = Terminal::new(10, 3, FG, BG);
    t.advance(b"l0\r\nl1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6\r\nl7");
    assert_eq!(t.history_len(), 5, "precondition");
    t.scroll_up(4);
    select(&mut t, (0, 0), (1, 1));
    assert_eq!(
        t.selection_text().as_deref(),
        Some("l1\nl2"),
        "precondition"
    );
    t
}

#[test]
fn a_selection_wholly_in_history_ignores_live_repaints() {
    // scroll-anchor R4.6 — its reference is empty (nothing was live when it was
    // set), and history rows cannot change, so a live clear plus new text leaves
    // it alone.
    let mut t = history_only_selection();
    t.advance(b"\x1b[2J\x1b[1;1Hnew\x1b[2;1Htext");
    settle(&mut t);
    assert!(t.selection().is_some());
    assert_eq!(t.selection_text().as_deref(), Some("l1\nl2"));
}

#[test]
fn a_span_from_history_into_live_is_cleared_by_its_live_part() {
    // scroll-anchor R4.7 — one line of output moves `alpha` into history; a
    // selection from `alpha` (history) to `charlie` (live), made scrolled back,
    // is cleared when a selected live cell is overwritten.
    let mut t = bravo_term();
    t.advance(b"\r\ndelta"); // alpha → history; live: bravo, charlie, delta
    t.scroll_up(1);
    select(&mut t, (0, 0), (6, 2)); // alpha (history) .. charlie (live row 1)
    assert_eq!(
        t.selection_text().as_deref(),
        Some("alpha\nbravo\ncharlie"),
        "precondition"
    );
    t.scroll_to_bottom();
    t.advance(b"\x1b[2;3HX"); // live row 1 col 2: charlie → chXrlie
    settle(&mut t);
    assert_eq!(t.selection(), None);
}

#[test]
fn every_set_retakes_the_reference() {
    // scroll-anchor R4.8 — the change lands with no settle; the same span set again
    // re-baselines on `xravo`, so the settle that follows finds nothing changed.
    let mut t = bravo_selected();
    t.advance(b"\x1b[2;1Hx");
    select(&mut t, (0, 1), (4, 1));
    settle(&mut t);
    assert!(t.selection().is_some());
    assert_eq!(t.selection_text().as_deref(), Some("xravo"));
}

// ---- scroll-anchor R5: the check runs at frame boundaries, never mid-frame ----

#[test]
fn sync_end_checks_at_the_exact_frame_end_without_a_settle() {
    // scroll-anchor R5.2.
    let mut t = bravo_selected();
    t.advance(b"\x1b[?2026h\x1b[2;1Hxravo\x1b[?2026l");
    assert_eq!(t.selection(), None);
}

#[test]
fn a_split_frame_that_restores_the_text_keeps_the_selection() {
    // scroll-anchor R5.3 — measured: macOS hands out at most 1024 bytes per read,
    // so a frame arrives in pieces. The drain that ends mid-frame must not check:
    // its row reads blank there.
    let mut t = bravo_selected();
    t.advance(b"\x1b[?2026h\x1b[2;1H\x1b[2K");
    settle(&mut t);
    assert!(t.selection().is_some(), "the block is open: settle skips");
    t.advance(b"bravo\x1b[?2026l");
    assert!(
        t.selection().is_some(),
        "the text at frame end is unchanged"
    );
    assert_eq!(t.selection_text().as_deref(), Some("bravo"));
}

#[test]
fn a_split_frame_that_changes_the_text_clears_at_its_end() {
    // scroll-anchor R5.4.
    let mut t = bravo_selected();
    t.advance(b"\x1b[?2026h\x1b[2;1H\x1b[2K");
    settle(&mut t);
    assert!(t.selection().is_some(), "precondition: skipped mid-frame");
    t.advance(b"BRAVO\x1b[?2026l");
    assert_eq!(t.selection(), None);
}

#[test]
fn without_sync_the_check_waits_for_the_drain_not_the_chunk() {
    // scroll-anchor R5.5 — `advance` never checks; the drain's settle does.
    let mut t = bravo_selected();
    t.advance(b"\x1b[2;1Hx");
    assert!(t.selection().is_some(), "advance alone does not check");
    settle(&mut t);
    assert_eq!(t.selection(), None);
}

// ---- scroll-anchor R6: copy re-verifies ----

#[test]
fn copy_refuses_changed_text_before_any_settle() {
    // scroll-anchor R6.4 — the highlight is still up (no check has run), but Cmd+C
    // must not return `xravo`, which the user never selected.
    let mut t = bravo_selected();
    t.advance(b"\x1b[2;1Hx");
    assert!(t.selection().is_some(), "no check ran");
    assert_eq!(t.selection_text(), None);
}

#[test]
fn copy_re_verifies_inside_an_open_block_too() {
    // scroll-anchor R6.4 / D6 — "regardless of D5's timing": an open synchronized
    // update defers the check, never the re-verify.
    let mut t = bravo_selected();
    t.advance(b"\x1b[?2026h\x1b[2;1Hx");
    settle(&mut t);
    assert!(t.selection().is_some(), "the check is deferred");
    assert_eq!(t.selection_text(), None, "the copy is not");
}

// ---- scroll-anchor R7: existing lifetime rules are unchanged ----

#[test]
fn erase_display_does_not_clear_a_selection_it_did_not_touch() {
    // scroll-anchor R7.2 — no dedicated clear on `CSI 2J`: the history-only
    // selection's text survived it, so it stays.
    let mut t = history_only_selection();
    t.advance(b"\x1b[2J");
    settle(&mut t);
    assert!(t.selection().is_some());
}

#[test]
fn an_alt_switch_inside_an_open_block_clears_at_the_blocks_end() {
    // scroll-anchor R7.3 — `?1049h` + `CSI 2J` blank `bravo`, but inside an open
    // block: the drain's settle skips, and the frame's `?2026l` clears.
    let mut t = bravo_selected();
    t.advance(b"\x1b[?2026h\x1b[?1049h\x1b[2J");
    settle(&mut t);
    assert!(t.selection().is_some(), "deferred to the block's end");
    t.advance(b"\x1b[?2026l");
    assert_eq!(t.selection(), None);
}
