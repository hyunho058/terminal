use renderer::{cell_span, row_chars};
use term_core::{Cell, Color, Grid, GridSnapshot};

fn snapshot(grid: &Grid) -> GridSnapshot {
    GridSnapshot::from_grid(grid)
}

#[test]
fn valid_wide_pair_skips_continuation_column_in_row_chars() {
    // Given: A GridSnapshot row holding '한' at column 0 and Cell::CONTINUATION at column 1
    let mut grid = Grid::new(2, 1, Color(0xFFFFFF), Color(0));
    grid.print('한');
    let snap = snapshot(&grid);
    assert_eq!(snap.cell(0, 0).map(|c| c.ch), Some('한'));
    assert_eq!(snap.cell(1, 0).map(|c| c.ch), Some(Cell::CONTINUATION));

    // When: Public function renderer::row_chars(snapshot, row) processes the row
    let row = row_chars(&snap, 0);

    // Then: RowChars::chars contains ['한'] and RowChars::char_col contains [0], skipping column 1 from shaping input
    assert_eq!(row.chars, vec!['한']);
    assert_eq!(row.char_col, vec![0]);
}

#[test]
fn orphan_continuation_cell_without_leader_is_treated_as_blank_space_in_row_chars() {
    // Given: A GridSnapshot row holding single-width 'a' at column 0 and an orphan Cell::CONTINUATION at column 1
    let mut grid = Grid::new(2, 1, Color(0xFFFFFF), Color(0));
    grid.print('한');
    grid.break_wide_pair_at(1, 0);
    grid.move_to(0, 0);
    grid.print('a');
    let snap = snapshot(&grid);
    assert_eq!(snap.cell(0, 0).map(|c| c.ch), Some('a'));
    assert_eq!(snap.cell(1, 0).map(|c| c.ch), Some(Cell::CONTINUATION));

    // When: Public function renderer::row_chars(snapshot, row) processes the row
    let row = row_chars(&snap, 0);

    // Then: RowChars::chars contains ['a', ' '] and RowChars::char_col contains [0, 1], preventing a column drop from staggering subsequent character positions
    assert_eq!(row.chars, vec!['a', ' ']);
    assert_eq!(row.char_col, vec![0, 1]);
}

#[test]
fn cell_span_returns_2_only_for_double_width_leader_followed_by_continuation_spacer() {
    // Given: A GridSnapshot row holding single-width 'x' at column 2 and Cell::CONTINUATION at column 3
    let mut grid = Grid::new(4, 1, Color(0xFFFFFF), Color(0));
    grid.move_to(2, 0);
    grid.print('한');
    grid.break_wide_pair_at(3, 0);
    grid.move_to(2, 0);
    grid.print('x');
    let snap = snapshot(&grid);
    assert_eq!(snap.cell(2, 0).map(|c| c.ch), Some('x'));
    assert_eq!(snap.cell(3, 0).map(|c| c.ch), Some(Cell::CONTINUATION));

    // When: Public function renderer::cell_span(snapshot, 2, row) is evaluated
    let span = cell_span(&snap, 2, 0);

    // Then: It returns 1, ensuring the cluster layout for 'x' does not receive a 2-cell span
    assert_eq!(span, 1);
}
