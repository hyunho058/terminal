//! Headless tests for the selection model (selection-model R2–R4): Grid selection
//! state, GridSnapshot carrying + extracting a selection, and bracketed-paste mode
//! 2004. No GPU, no window — the whole feature is verifiable in-process.

use term_core::{Color, Grid, Selection, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, FG, BG)
}

// ---- R2: Grid holds selection state with set/clear + resize invalidation ----

#[test]
fn grid_default_has_no_selection() {
    // R2.4.
    let g = Grid::new(10, 4, FG, BG);
    assert_eq!(g.selection(), None);
}

#[test]
fn grid_set_then_read_selection() {
    // R2.1.
    let mut g = Grid::new(10, 4, FG, BG);
    let sel = Selection {
        anchor: (2, 0),
        focus: (6, 1),
    };
    g.set_selection(sel);
    assert_eq!(g.selection(), Some(sel));
}

#[test]
fn grid_clear_removes_selection() {
    // R2.2.
    let mut g = Grid::new(10, 4, FG, BG);
    g.set_selection(Selection::new((3, 1)));
    g.clear_selection();
    assert_eq!(g.selection(), None);
}

#[test]
fn resize_invalidates_selection() {
    // R2.3.
    let mut g = Grid::new(10, 4, FG, BG);
    g.set_selection(Selection {
        anchor: (1, 1),
        focus: (8, 3),
    });
    g.resize(6, 2);
    assert_eq!(g.selection(), None, "coords are dimension-bound → cleared");
}

// ---- R3: GridSnapshot carries the selection and extracts its text ----

#[test]
fn live_snapshot_carries_the_selection() {
    // R3.1.
    let mut t = term(10, 3);
    let sel = Selection {
        anchor: (0, 0),
        focus: (4, 0),
    };
    t.set_selection(sel);
    assert_eq!(t.snapshot().selection, Some(sel));
}

#[test]
fn scrolled_snapshot_carries_no_selection() {
    // R3.2 — a selection is a live-viewport concept; hidden while in history.
    let mut t = term(8, 2);
    t.advance(b"aaa\r\nbbb\r\nccc\r\nddd\r\neee");
    t.set_selection(Selection {
        anchor: (0, 0),
        focus: (2, 0),
    });
    t.scroll_up(1);
    assert!(t.view_offset() > 0, "precondition: actually scrolled");
    assert_eq!(t.snapshot().selection, None);
}

#[test]
fn extraction_trims_trailing_and_skips_continuation_cells() {
    // R3.3 — single-row selection over `한글  ` (two wide chars + trailing spaces).
    let mut t = term(8, 1);
    t.advance("한글".as_bytes()); // cols 0-3 (leader+cont ×2); cols 4-7 blank
    t.set_selection(Selection {
        anchor: (0, 0),
        focus: (7, 0),
    });
    assert_eq!(t.snapshot().selection_text().as_deref(), Some("한글"));
}

#[test]
fn soft_wrapped_rows_join_without_newline() {
    // R3.4 (soft-wrap case) — a line that filled the width wraps and joins.
    let mut t = term(5, 3);
    t.advance(b"helloworld"); // row0 "hello" (col4 filled), row1 "world"
    t.set_selection(Selection {
        anchor: (0, 0),
        focus: (4, 1),
    });
    assert_eq!(t.snapshot().selection_text().as_deref(), Some("helloworld"));
}

#[test]
fn hard_break_rows_join_with_newline() {
    // R3.4 (hard-break case) — a row that ends before the last column keeps `\n`.
    let mut t = term(5, 3);
    t.advance(b"hi\r\nyo"); // row0 "hi" (col4 blank → hard break), row1 "yo"
    t.set_selection(Selection {
        anchor: (0, 0),
        focus: (1, 1),
    });
    assert_eq!(t.snapshot().selection_text().as_deref(), Some("hi\nyo"));
}

#[test]
fn no_selection_extracts_none() {
    // R3.5.
    let mut t = term(10, 2);
    t.advance(b"hello");
    assert_eq!(t.snapshot().selection_text(), None);
}

#[test]
fn inverted_coordinates_normalize_without_panicking() {
    // R3.6 — focus precedes anchor; extraction normalizes and does not panic.
    let mut t = term(10, 2);
    t.advance(b"hello");
    t.set_selection(Selection {
        anchor: (4, 0),
        focus: (0, 0),
    });
    assert_eq!(t.snapshot().selection_text().as_deref(), Some("hello"));
}

#[test]
fn out_of_range_rows_extract_without_panicking() {
    // R3.6 (edge half) — both endpoints past the grid bottom, with inverted
    // columns, collapse onto the last row; extraction must clamp and not panic
    // (a subtract-overflow regression in the capacity math).
    let mut t = term(10, 2);
    t.advance(b"hello");
    t.set_selection(Selection {
        anchor: (9, 5),
        focus: (0, 7),
    });
    // Bounded, non-panicking — the exact text is unspecified for a bogus span.
    let _ = t.snapshot().selection_text();
}

// ---- R4: bracketed-paste mode (DECSET/DECRST 2004) ----

#[test]
fn bracketed_paste_defaults_off() {
    // R4.3.
    assert!(!term(10, 2).bracketed_paste());
}

#[test]
fn decset_2004_enables_bracketed_paste() {
    // R4.1.
    let mut t = term(10, 2);
    t.advance(b"\x1b[?2004h");
    assert!(t.bracketed_paste());
}

#[test]
fn decrst_2004_disables_bracketed_paste() {
    // R4.2.
    let mut t = term(10, 2);
    t.advance(b"\x1b[?2004h");
    t.advance(b"\x1b[?2004l");
    assert!(!t.bracketed_paste());
}

#[test]
fn ris_resets_bracketed_paste() {
    // R4.4 — a full reset (ESC c) clears the mode, like DECCKM.
    let mut t = term(10, 2);
    t.advance(b"\x1b[?2004h");
    t.advance(b"\x1bc");
    assert!(!t.bracketed_paste());
}
