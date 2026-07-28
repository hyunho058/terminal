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

// --- vt-tui-completeness R3: a scroll region must not pollute scrollback ---------

#[test]
fn partial_region_scroll_retains_nothing_in_history() {
    // R3.1 — the decision this whole feature turns on (D1). A streaming TUI scrolls
    // its message pane hundreds of times per response; if each of those pushed a row
    // the 100k ring would fill with redraw garbage and Cmd-scroll would be useless.
    let mut t = term(4, 6);
    t.advance(b"\x1b[2;4r"); // region = rows 1..=3, a partial band
    assert_eq!(t.history_len(), 0);

    for _ in 0..50 {
        t.advance(b"\x1b[1S");
    }
    assert_eq!(t.history_len(), 0, "50 region scrolls retained nothing");

    // A region line feed is the same story — it is what a TUI actually emits.
    t.advance(b"\x1b[4;1H"); // region bottom
    for _ in 0..50 {
        t.advance(b"\n");
    }
    assert_eq!(t.history_len(), 0, "region line feeds retained nothing");
}

#[test]
fn partial_region_scroll_does_not_disturb_a_scrolled_up_viewport() {
    // R3.2 — the user is reading history while a TUI repaints. Their view must not
    // drift, and the offset must not be auto-incremented by frames they can't see.
    let mut t = term(4, 6);
    feed_lines(&mut t, 12); // build real history first
    t.scroll_up(2);
    let offset_before = t.view_offset();
    let top_before = top_row(&t);
    assert_eq!(offset_before, 2);

    t.advance(b"\x1b[2;4r"); // now a TUI claims a partial region
    for _ in 0..20 {
        t.advance(b"\x1b[1S");
    }

    assert_eq!(t.view_offset(), 2, "offset did not drift");
    assert_eq!(top_row(&t), top_before, "the pinned content is unchanged");
}

#[test]
fn partial_region_scroll_does_not_move_image_placements() {
    // R3.3 / D15 — placements are anchored to absolute rows and belong to the
    // primary screen, not to a TUI's private pane.
    use base64::Engine;
    let rgba = [0u8, 255, 0, 255]; // 1x1 green
    let b64 = base64::engine::general_purpose::STANDARD.encode(rgba);
    let apc = format!("\x1b_Gf=32,s=1,v=1,c=1,r=1;{b64}\x1b\\");

    let mut t = term(8, 6);
    t.advance(b"\x1b[5;1H"); // cursor to row 4, below the region-to-be
    t.advance(apc.as_bytes());
    let snap = t.snapshot();
    assert_eq!(snap.images.len(), 1, "one placement to track");
    assert_eq!(snap.images[0].row, 4);

    t.advance(b"\x1b[2;4r"); // region = rows 1..=3
    for _ in 0..3 {
        t.advance(b"\x1b[1S");
    }
    let snap = t.snapshot();
    assert_eq!(snap.images.len(), 1, "placement survived");
    assert_eq!(snap.images[0].row, 4, "and did not scroll with the region");
}

#[test]
fn full_screen_region_behaves_exactly_as_before_the_feature() {
    // R3.4 / D14 — the regression guard. D1 made the history push conditional on a
    // predicate that did not exist before, and the entire scrollback feature rides
    // on that predicate defaulting to true. This asserts the three things the
    // conditional guards, under both "no CSI r ever sent" and "explicitly reset".
    for reset in [None, Some(b"\x1b[r".as_slice())] {
        let mut t = term(4, 3);
        if let Some(seq) = reset {
            t.advance(b"\x1b[2;3r"); // set a region…
            t.advance(seq); // …then hand the screen back
        }

        feed_lines(&mut t, 6);
        assert_eq!(t.history_len(), 3, "history still grows one row per scroll");
        assert_eq!(top_row(&t), "3");

        t.scroll_up(1);
        assert_eq!(top_row(&t), "2", "history content is intact");
        t.advance(b"\r\n6");
        assert_eq!(t.history_len(), 4);
        assert_eq!(
            top_row(&t),
            "2",
            "view_offset still auto-increments to stay pinned"
        );
    }
}

#[test]
fn full_screen_scroll_still_shifts_image_placements() {
    // R3.4, third clause — the one the equivalence guard was NOT protecting.
    // `full_screen_region_behaves_exactly_as_before_the_feature` asserts history
    // growth and view_offset pinning, and the partial-region test asserts images
    // do NOT move; nothing asserted that a full-screen scroll DOES move them, so
    // deleting `images.scroll_up_one()` from the guarded block broke zero tests.
    use base64::Engine;
    let rgba = [0u8, 255, 0, 255];
    let b64 = base64::engine::general_purpose::STANDARD.encode(rgba);
    let apc = format!("\x1b_Gf=32,s=1,v=1,c=1,r=1;{b64}\x1b\\");

    let mut t = term(8, 4);
    t.advance(b"\x1b[3;1H"); // row 2
    t.advance(apc.as_bytes());
    assert_eq!(t.snapshot().images[0].row, 2, "placed at row 2");

    // A full-screen line-feed scroll must carry the placement up with the content.
    t.advance(b"\x1b[4;1H\n");
    assert_eq!(
        t.snapshot().images[0].row,
        1,
        "a full-screen scroll shifts image placements up (R3.4)"
    );

    // And an explicit full-screen SU does the same.
    t.advance(b"\x1b[1S");
    assert_eq!(t.snapshot().images[0].row, 0, "SU shifts placements too");

    // Scrolling past the top drops it, rather than wrapping to a bogus row.
    t.advance(b"\x1b[1S");
    assert!(
        t.snapshot().images.is_empty(),
        "a placement scrolled off the top is dropped"
    );
}

#[test]
fn explicit_su_on_a_full_screen_region_still_retains_history() {
    // The other side of R3.4: SU is not exempt from history when it really is a
    // full-screen scroll — only a *partial* region suppresses retention.
    let mut t = term(4, 3);
    feed_lines(&mut t, 3); // rows 0..2 = "0","1","2"
    let before = t.history_len();
    t.advance(b"\x1b[2S");
    assert_eq!(
        t.history_len(),
        before + 2,
        "a full-screen SU retires its rows to history"
    );
}
