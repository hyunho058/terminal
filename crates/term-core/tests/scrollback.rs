//! Headless scrollback tests (feature: scrollback).
//!
//! Proves the retained-history ring, the viewport offset math, and the
//! alternate-screen freeze — all without a GPU or a real shell (C4). These are
//! assertion-based (not insta) because the invariants are numeric/positional,
//! not visual: history length, offset clamping, which rows compose the view.
//!
//! Requirement anchors: R0 (scroll through history), R1 (bounded eviction),
//! R2 (offset auto-increment / clamp), R5 (alt-screen frozen primary).

use term_core::{Color, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, FG, BG)
}

/// Feed `n` numbered lines ("0\r\n1\r\n...") into a fresh terminal.
fn feed_lines(t: &mut Terminal, n: usize) {
    for i in 0..n {
        if i > 0 {
            t.advance(b"\r\n");
        }
        t.advance(i.to_string().as_bytes());
    }
}

/// The top visible row of the current snapshot, trimmed.
fn top_row(t: &Terminal) -> String {
    let snap = t.snapshot();
    let mut s = String::new();
    for col in 0..snap.cols {
        s.push(snap.cell(col, 0).map(|c| c.ch).unwrap_or(' '));
    }
    s.trim_end().to_string()
}

// --- R0: scrolling up reveals retained history ---------------------------------

#[test]
fn scroll_up_reveals_history() {
    let mut t = term(4, 3);
    // 6 logical lines through a 3-row grid → 3 rows scrolled into history.
    feed_lines(&mut t, 6);
    assert_eq!(t.history_len(), 3, "three rows should have scrolled off");
    assert!(t.at_bottom(), "fresh feed leaves the viewport live");
    assert_eq!(top_row(&t), "3", "live top row is the 4th line");

    // Scroll up one: the departed row "2" reappears at the top.
    t.scroll_up(1);
    assert_eq!(t.view_offset(), 1);
    assert!(!t.at_bottom());
    assert_eq!(top_row(&t), "2", "one line of history now visible on top");

    // All the way up: oldest retained row "0".
    t.scroll_up(999);
    assert!(t.at_top(), "clamped at the oldest retained row");
    assert_eq!(t.view_offset(), 3);
    assert_eq!(top_row(&t), "0");
}

// --- R2: offset clamps and returns to bottom -----------------------------------

#[test]
fn scroll_down_returns_to_live_bottom() {
    let mut t = term(4, 3);
    feed_lines(&mut t, 6);
    t.scroll_up(2);
    assert_eq!(t.view_offset(), 2);

    t.scroll_down(1);
    assert_eq!(t.view_offset(), 1);

    // Over-scrolling down saturates at the live bottom, not underflow.
    t.scroll_down(50);
    assert_eq!(t.view_offset(), 0);
    assert!(t.at_bottom());

    // scroll_to_bottom is idempotent at the bottom.
    t.scroll_up(3);
    t.scroll_to_bottom();
    assert_eq!(t.view_offset(), 0);
    assert!(t.at_bottom());
}

// --- R2: a scrolled-up viewport stays pinned as new output arrives --------------

#[test]
fn view_offset_auto_increments_when_scrolled() {
    let mut t = term(4, 3);
    feed_lines(&mut t, 6); // history_len == 3
    t.scroll_up(1); // viewing "2" at top, offset 1
    assert_eq!(top_row(&t), "2");

    // New output scrolls the live view; the pinned content must not drift.
    t.advance(b"\r\n6\r\n7");
    assert_eq!(t.history_len(), 5, "two more rows retired to history");
    assert!(!t.at_bottom(), "still scrolled up after new output");
    assert_eq!(
        top_row(&t),
        "2",
        "offset auto-incremented to keep the same content pinned"
    );
}

// --- R1: history is a bounded ring; oldest rows are evicted ----------------------

#[test]
fn history_is_bounded_and_evicts_oldest() {
    // The default 100k cap can't be hit cheaply here, so this integration test
    // proves FIFO ordering and that len tracks pushes at the grid level; the ring
    // *cap* (pop_front eviction) is exercised directly by the History unit tests
    // in scrollback.rs (`evicts_oldest_over_the_cap_fifo`). Here: 200 lines, 3 rows.
    let mut t = term(6, 3);
    feed_lines(&mut t, 200);
    assert_eq!(t.history_len(), 197, "200 lines - 3 live rows retained");

    // Oldest retained row is line "0"; scroll fully up to see it.
    t.scroll_up(usize::MAX / 2);
    assert!(t.at_top());
    assert_eq!(top_row(&t), "0", "FIFO: line 0 is still the oldest");
}

// --- R5: the alternate screen freezes primary history ---------------------------

#[test]
fn alt_screen_does_not_grow_history() {
    let mut t = term(4, 3);
    feed_lines(&mut t, 6); // history_len == 3 on the primary screen
    let before = t.history_len();
    assert_eq!(before, 3);

    // Enter the alternate screen (?1049h) and scroll a full-screen app.
    t.advance(b"\x1b[?1049h");
    assert!(t.alt_screen());
    feed_lines(&mut t, 10); // vim/tmux churn — must NOT retain into history

    assert_eq!(
        t.history_len(),
        before,
        "alt-screen output is not retained (R5)"
    );

    // Leaving the alt screen restores the frozen primary history intact.
    t.advance(b"\x1b[?1049l");
    assert!(!t.alt_screen());
    assert_eq!(t.history_len(), before);
}
