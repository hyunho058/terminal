//! Headless tests for the selection model: Grid selection state, the snapshot's
//! viewport projection of a content-anchored selection, extraction through
//! `Terminal::selection_text`, and bracketed-paste mode 2004. No GPU, no window —
//! the whole feature is verifiable in-process.
//!
//! Two specs' requirement ids live here and they collide (both have an R2.x), so
//! every comment names its spec: `selection-model Rx.y` for the original model,
//! `scroll-anchor Rx.y` for `specs/selection-scroll-anchor`.

use term_core::{Color, ContentSelection, Grid, Point, Selection, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, FG, BG)
}

/// The spec's fixture: a 10×3 terminal reading `alpha` / `bravo` / `charlie` on
/// rows 0–2, at the live bottom with nothing in history yet.
fn bravo_term() -> Terminal {
    let mut t = term(10, 3);
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

/// The text of content line `line`, read by selecting it across the full width.
/// Replaces any selection — only for tests that have no other use for it.
fn line_text(t: &mut Terminal, line: u64) -> Option<String> {
    let last = t.dimensions().0 - 1;
    t.set_selection(Point { line, col: 0 }, Point { line, col: last });
    t.selection_text()
}

fn pt(line: u64, col: u16) -> Point {
    Point { line, col }
}

fn viewport(anchor: (u16, u16), focus: (u16, u16)) -> Option<Selection> {
    Some(Selection { anchor, focus })
}

// ---- selection-model R2: Grid holds selection state, set/clear + resize ----

#[test]
fn grid_default_has_no_selection() {
    // selection-model R2.4.
    let g = Grid::new(10, 4, FG, BG);
    assert_eq!(g.selection(), None);
}

#[test]
fn grid_set_then_read_selection() {
    // selection-model R2.1 — the grid stores the two content points as set.
    let mut g = Grid::new(10, 4, FG, BG);
    let (anchor, focus) = (g.point_at(2, 0), g.point_at(6, 1));
    g.set_selection(anchor, focus);
    assert_eq!(g.selection(), Some(ContentSelection { anchor, focus }));
}

#[test]
fn grid_clear_removes_selection() {
    // selection-model R2.2.
    let mut g = Grid::new(10, 4, FG, BG);
    let p = g.point_at(3, 1);
    g.set_selection(p, p);
    g.clear_selection();
    assert_eq!(g.selection(), None);
}

#[test]
fn resize_invalidates_selection() {
    // selection-model R2.3, kept by scroll-anchor R7.1 — with no rewrap a line's
    // columns change meaning at a new width, so a resize clears the selection.
    let mut g = Grid::new(10, 4, FG, BG);
    let (anchor, focus) = (g.point_at(1, 1), g.point_at(8, 3));
    g.set_selection(anchor, focus);
    g.resize(6, 2);
    assert_eq!(g.selection(), None, "dimension-bound → cleared");
}

#[test]
fn terminal_resize_clears_the_selection() {
    // scroll-anchor R7.1 — the same rule through the `Terminal` facade.
    let mut t = bravo_term();
    select(&mut t, (0, 1), (4, 1));
    t.resize(6, 2);
    assert_eq!(t.selection(), None);
    assert_eq!(t.selection_text(), None);
}

// ---- scroll-anchor R1: a content line is stable through terminal-owned scrolling ----

#[test]
fn point_at_a_live_cell_is_evicted_plus_history_plus_row() {
    // scroll-anchor R1.2 — live row r is line `evicted + history_len + r`. The
    // eviction half (evicted > 0) needs the test-only history limit, so it is
    // pinned by `lib.rs`'s unit tests.
    let mut t = term(10, 3);
    t.advance(b"l0\r\nl1\r\nl2\r\nl3\r\nl4");
    let (e, h) = (t.evicted(), t.history_len() as u64);
    assert_eq!(h, 2, "precondition: two rows in history");
    assert_eq!(t.point_at(4, 2), pt(e + h + 2, 4));
    assert_eq!(t.point_at(0, 0), pt(e + h, 0), "live row 0 is line e + h");
}

#[test]
fn a_full_screen_scroll_keeps_a_lines_number_on_its_text() {
    // scroll-anchor R1.3 — `bravo` moves from row 1 to row 0; its line number
    // moves with it.
    let mut t = bravo_term();
    let p = t.point_at(0, 1);
    t.advance(b"\r\ndelta");
    assert_eq!(t.point_at(0, 0), p, "the text's row moved up, not its line");
    assert_eq!(line_text(&mut t, p.line).as_deref(), Some("bravo"));
}

#[test]
fn a_scrolled_back_cell_maps_through_view_offset() {
    // scroll-anchor R1.5 — at offset 1, viewport row 0 shows the newest history
    // line (`evicted + history_len - 1`), so row 1 is the next line.
    let mut t = term(10, 3);
    t.advance(b"alpha\r\nbravo\r\ncharlie\r\ndelta\r\necho");
    t.scroll_up(1);
    assert_eq!(t.view_offset(), 1, "precondition: scrolled back by one");
    let live0 = t.evicted() + t.history_len() as u64;
    assert_eq!(t.point_at(0, 1), pt(live0, 0));
    assert_eq!(t.point_at(3, 0), pt(live0 - 1, 3));
    // And the line it names is the text on that viewport row.
    let shown = t.snapshot().to_text();
    assert_eq!(shown.lines().nth(1), Some("charlie"));
    assert_eq!(line_text(&mut t, live0).as_deref(), Some("charlie"));
}

#[test]
fn an_app_owned_scroll_does_not_renumber_lines() {
    // scroll-anchor R1.6 — a scroll on the alt screen never enters history, so
    // the line number stays on the row while the text under it changes.
    let mut t = term(10, 3);
    t.advance(b"\x1b[?1049h\x1b[2J\x1b[1;1Hone\x1b[2;1Htwo\x1b[3;1Hthree");
    let h = t.history_len();
    let p = t.point_at(0, 1);
    assert_eq!(line_text(&mut t, p.line).as_deref(), Some("two"));
    t.advance(b"\x1b[S");
    assert_eq!(t.point_at(0, 1), p, "same row, same line number");
    assert_eq!(t.history_len(), h, "nothing entered history");
    assert_eq!(
        line_text(&mut t, p.line).as_deref(),
        Some("three"),
        "the characters at p changed under it"
    );
}

// ---- scroll-anchor R2: the snapshot projects the selection at every offset ----

#[test]
fn live_projection_equals_the_dragged_cells() {
    // scroll-anchor R2.1 (and selection-model R3.1) — at the live bottom with no
    // output, the projection is exactly the cells the drag covered.
    let mut t = bravo_term();
    select(&mut t, (0, 1), (4, 1));
    assert_eq!(t.snapshot().selection, viewport((0, 1), (4, 1)));
}

#[test]
fn output_moves_the_projection_up_with_the_text() {
    // scroll-anchor R2.2 — one line of output moves `bravo` to row 0, and the
    // highlight goes with it.
    let mut t = bravo_term();
    select(&mut t, (0, 1), (4, 1));
    t.advance(b"\r\ndelta");
    assert_eq!(t.snapshot().selection, viewport((0, 0), (4, 0)));
    assert_eq!(t.selection_text().as_deref(), Some("bravo"));
}

#[test]
fn a_scrolled_back_viewport_shows_the_selection() {
    // scroll-anchor R2.3 — replaces selection-model R3.2's
    // `scrolled_snapshot_carries_no_selection`, the behaviour this reverses.
    let mut t = bravo_term();
    select(&mut t, (0, 1), (4, 1));
    t.advance(b"\r\ndelta");
    t.scroll_up(1);
    assert_eq!(t.view_offset(), 1, "precondition: actually scrolled");
    assert_eq!(t.snapshot().selection, viewport((0, 1), (4, 1)));
}

#[test]
fn a_start_above_the_viewport_clips_to_the_top_left_cell() {
    // scroll-anchor R2.4 — start at (top − 2, col 5), end at (top + 1, col 3).
    let mut t = term(10, 3);
    t.advance(b"l0\r\nl1\r\nl2\r\nl3\r\nl4");
    let top = t.point_at(0, 0).line;
    assert!(top >= 2, "precondition: two lines above the viewport");
    t.set_selection(pt(top - 2, 5), pt(top + 1, 3));
    assert_eq!(t.snapshot().selection, viewport((0, 0), (3, 1)));
}

#[test]
fn an_end_below_the_viewport_clips_to_the_bottom_right_cell() {
    // scroll-anchor R2.5 — selected at the live bottom from live row 0 col 4 to
    // live row 2; scrolled back by one, the start sits on viewport row 1 and the
    // end is below viewport row 2.
    let mut t = term(10, 3);
    t.advance(b"l0\r\nl1\r\nl2\r\nl3\r\nl4");
    select(&mut t, (4, 0), (2, 2));
    t.scroll_up(1);
    assert_eq!(t.snapshot().selection, viewport((4, 1), (9, 2)));
}

#[test]
fn a_span_wholly_outside_the_viewport_projects_to_none() {
    // scroll-anchor R2.6 — on a 10×3 viewport whose row 0 is line `top`: a span
    // ending at `top − 1` and one starting at `top + 3` are invisible; one ending
    // at (top, col 0) is visible and ends at (0, 0). A span starting on the last
    // row is visible too — the boundary on the other side.
    let mut t = term(10, 3);
    t.advance(b"l0\r\nl1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6");
    t.scroll_up(2);
    let top = t.point_at(0, 0).line;
    assert_eq!(top, 2, "precondition: lines exist above and below the view");

    t.set_selection(pt(top - 2, 1), pt(top - 1, 9));
    assert_eq!(t.snapshot().selection, None, "ends above the viewport");

    t.set_selection(pt(top + 3, 0), pt(top + 4, 1));
    assert_eq!(t.snapshot().selection, None, "starts below the viewport");

    t.set_selection(pt(top - 1, 3), pt(top, 0));
    assert_eq!(
        t.snapshot().selection,
        viewport((0, 0), (0, 0)),
        "touches the top edge: clipped start, end at (0, 0)"
    );

    t.set_selection(pt(top + 2, 6), pt(top + 4, 1));
    assert_eq!(
        t.snapshot().selection,
        viewport((6, 2), (9, 2)),
        "starts on the last row: visible, end clipped"
    );
}

#[test]
fn a_backward_drag_projects_like_a_forward_one() {
    // scroll-anchor R2.7 — anchor after focus in reading order.
    let mut back = bravo_term();
    select(&mut back, (4, 2), (1, 0));
    let projected = back.snapshot().selection.expect("visible");
    assert_eq!(projected.normalized(), ((1, 0), (4, 2)));

    let mut fwd = bravo_term();
    select(&mut fwd, (1, 0), (4, 2));
    assert_eq!(
        back.snapshot().selection,
        fwd.snapshot().selection,
        "the same projected span"
    );
    assert_eq!(back.selection_text(), fwd.selection_text());
}

// ---- scroll-anchor R6: one extractor reads the whole selection ----

#[test]
fn copy_returns_a_selection_whose_start_has_scrolled_off_screen() {
    // scroll-anchor R6.1 — two lines of output push `bravo` into history; the
    // visible snapshot no longer holds it, the extractor still does.
    let mut t = bravo_term();
    select(&mut t, (0, 1), (6, 2));
    t.advance(b"\r\ndelta\r\necho");
    assert!(
        !t.snapshot().to_text().contains("bravo"),
        "precondition: `bravo` is off-screen"
    );
    assert_eq!(t.selection_text().as_deref(), Some("bravo\ncharlie"));
}

#[test]
fn extraction_trims_trailing_and_skips_continuation_cells() {
    // scroll-anchor R6.2 (selection-model R3.3) — single-row selection over
    // `한글    ` (two wide chars + trailing blanks).
    let mut t = term(8, 1);
    t.advance("한글".as_bytes()); // cols 0-3 (leader+cont ×2); cols 4-7 blank
    select(&mut t, (0, 0), (7, 0));
    assert_eq!(t.selection_text().as_deref(), Some("한글"));
}

#[test]
fn soft_wrapped_rows_join_without_newline() {
    // scroll-anchor R6.2 (selection-model R3.4, soft-wrap case) — a line that
    // filled the width wraps and joins.
    let mut t = term(5, 3);
    t.advance(b"helloworld"); // row0 "hello" (col4 filled), row1 "world"
    select(&mut t, (0, 0), (4, 1));
    assert_eq!(t.selection_text().as_deref(), Some("helloworld"));
}

#[test]
fn hard_break_rows_join_with_newline() {
    // scroll-anchor R6.2 (selection-model R3.4, hard-break case) — a row that
    // ends before the last column keeps `\n`.
    let mut t = term(5, 3);
    t.advance(b"hi\r\nyo"); // row0 "hi" (col4 blank → hard break), row1 "yo"
    select(&mut t, (0, 0), (1, 1));
    assert_eq!(t.selection_text().as_deref(), Some("hi\nyo"));
}

#[test]
fn a_partial_start_and_end_line_read_only_their_columns() {
    // scroll-anchor R6.2 — the linear span: the start line from `start.col`, the
    // end line up to `end.col`, interior lines full width.
    let mut t = bravo_term();
    select(&mut t, (2, 0), (3, 2));
    assert_eq!(t.selection_text().as_deref(), Some("pha\nbravo\nchar"));
}

#[test]
fn no_selection_extracts_none() {
    // scroll-anchor R6.2 (selection-model R3.5).
    let mut t = term(10, 2);
    t.advance(b"hello");
    assert_eq!(t.selection_text(), None);
}

#[test]
fn inverted_coordinates_normalize_without_panicking() {
    // scroll-anchor R6.2 (selection-model R3.6) — focus precedes anchor;
    // extraction normalizes.
    let mut t = term(10, 2);
    t.advance(b"hello");
    select(&mut t, (4, 0), (0, 0));
    assert_eq!(t.selection_text().as_deref(), Some("hello"));
}

#[test]
fn out_of_range_points_extract_without_panicking() {
    // scroll-anchor R6.2 (selection-model R3.6, edge half) — both endpoints below
    // the live bottom, with inverted columns. Nothing selected exists, so the text
    // is empty; the point is that it is bounded and does not panic.
    let mut t = term(10, 2);
    t.advance(b"hello");
    t.set_selection(pt(5, 9), pt(7, 0));
    assert_eq!(t.selection_text().as_deref(), Some(""));
    // A column past the right margin clamps to it instead of reading off the row.
    t.set_selection(pt(0, 2), pt(0, 400));
    assert_eq!(t.selection_text().as_deref(), Some("llo"));
}

#[test]
fn a_span_past_the_live_bottom_stops_at_the_bottom() {
    // scroll-anchor D6 — lines below the live bottom do not exist, so a span that
    // reaches past it reads the bottom line in full and stops: no trailing blank
    // lines are invented.
    let mut t = term(10, 2);
    t.advance(b"hello\r\nyo");
    t.set_selection(pt(0, 0), pt(7, 3));
    assert_eq!(t.selection_text().as_deref(), Some("hello\nyo"));
}

#[test]
fn a_history_row_is_read_at_the_current_width() {
    // scroll-anchor R6.3 — a row captured at 20 columns, read after a resize to
    // 10, is cut at the current margin exactly as the snapshot shows it.
    let mut t = term(20, 2);
    t.advance(b"abcdefghijklmnopqrst\r\nx\r\ny");
    assert_eq!(
        t.history_len(),
        1,
        "precondition: the wide row is in history"
    );
    t.resize(10, 2);
    t.scroll_up(1);
    select(&mut t, (0, 0), (9, 0));
    assert_eq!(t.selection_text().as_deref(), Some("abcdefghij"));
    let shown = t.snapshot().to_text();
    assert_eq!(
        shown.lines().next(),
        Some("abcdefghij"),
        "same as the snapshot"
    );

    // Read as the *start* line of a longer span, the row is read up to the
    // current last column (not its captured one) and — its last visible cell
    // being non-blank — joins the next line as a soft wrap, as the snapshot's
    // own D8 rule would read it.
    select(&mut t, (2, 0), (0, 1));
    assert_eq!(t.selection_text().as_deref(), Some("cdefghijx"));
}

#[test]
fn a_narrower_history_row_is_padded_and_does_not_soft_wrap() {
    // scroll-anchor R6.3 (the other direction) — a row that filled 5 columns reads
    // as soft-wrapped at width 5, but after a resize to 10 its last column is blank
    // padding, so it is a hard break at the current width.
    let mut t = term(5, 2);
    t.advance(b"hello\r\nab\r\ncd");
    assert_eq!(t.history_len(), 1, "precondition: `hello` is in history");
    t.resize(10, 2);
    t.scroll_up(1);
    select(&mut t, (0, 0), (9, 1));
    assert_eq!(t.selection_text().as_deref(), Some("hello\nab"));
}

// ---- selection-model R4: bracketed-paste mode (DECSET/DECRST 2004) ----

#[test]
fn bracketed_paste_defaults_off() {
    // selection-model R4.3.
    assert!(!term(10, 2).bracketed_paste());
}

#[test]
fn decset_2004_enables_bracketed_paste() {
    // selection-model R4.1.
    let mut t = term(10, 2);
    t.advance(b"\x1b[?2004h");
    assert!(t.bracketed_paste());
}

#[test]
fn decrst_2004_disables_bracketed_paste() {
    // selection-model R4.2.
    let mut t = term(10, 2);
    t.advance(b"\x1b[?2004h");
    t.advance(b"\x1b[?2004l");
    assert!(!t.bracketed_paste());
}

#[test]
fn ris_resets_bracketed_paste() {
    // selection-model R4.4 — a full reset (ESC c) clears the mode, like DECCKM.
    let mut t = term(10, 2);
    t.advance(b"\x1b[?2004h");
    t.advance(b"\x1bc");
    assert!(!t.bracketed_paste());
}
