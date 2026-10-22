use term_core::{Color, Grid};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

/// R2.1: `clear_line_from_cursor` starting on a continuation spacer blanks the orphan leader
/// Given: A grid row holding double-width '글' at column 4 with continuation spacer at column 5, and cursor at column 5
/// When: Function `Grid::clear_line_from_cursor` (triggered by VT escape CSI 0 K) executes
/// Then: Both column 4 (the orphaned leader) and column 5 (the spacer) are overwritten with blank cells, and columns 5..cols are cleared
#[test]
fn r2_1_clear_line_from_cursor_on_continuation_spacer_blanks_orphan_leader() {
    let mut grid = Grid::new(10, 2, FG, BG);
    grid.move_to(4, 0);
    grid.print('글');
    grid.move_to(6, 0);
    grid.print('x');
    grid.move_to(5, 0);

    assert_eq!(grid.cell(4, 0).unwrap().ch, '글');
    assert!(grid.cell(5, 0).unwrap().is_continuation());
    assert_eq!(grid.cursor().col, 5);

    grid.clear_line_from_cursor();

    assert!(grid.cell(4, 0).unwrap().is_blank());
    assert!(!grid.cell(4, 0).unwrap().is_continuation());
    for col in 5..grid.cols() {
        assert!(grid.cell(col, 0).unwrap().is_blank());
        assert!(!grid.cell(col, 0).unwrap().is_continuation());
    }
}

/// R2.2: `clear_line_from_cursor` when `cursor.col >= cols` does not drop clear or corrupt grid
/// Given: A grid where `cursor.col` equals or exceeds `cols - 1` with `pending_wrap` active
/// When: Function `Grid::clear_line_from_cursor` (VT escape CSI 0 K) executes
/// Then: The last column is blanked and wide pairs on the row remain intact without early exit or out-of-bounds indexing
#[test]
fn r2_2_clear_line_from_cursor_at_end_of_line_with_pending_wrap_blanks_last_col() {
    let mut grid = Grid::new(6, 2, FG, BG);
    grid.move_to(0, 0);
    grid.print('한');
    grid.print('a');
    grid.print('b');
    grid.print('c');
    grid.print('d');

    assert_eq!(grid.cursor().col, 5);
    assert!(grid.pending_wrap());
    assert_eq!(grid.cell(0, 0).unwrap().ch, '한');
    assert!(grid.cell(1, 0).unwrap().is_continuation());
    assert_eq!(grid.cell(5, 0).unwrap().ch, 'd');

    grid.clear_line_from_cursor();

    assert!(grid.cell(5, 0).unwrap().is_blank());
    assert_eq!(grid.cell(0, 0).unwrap().ch, '한');
    assert!(grid.cell(1, 0).unwrap().is_continuation());
}

/// R2.3: `clear_from_cursor` (`CSI 0 J`) repairs bisected wide characters on cursor row
/// Given: A grid holding double-width '한' at columns 2..=3 on row 1, with cursor positioned at (3, 1)
/// When: Function `Grid::clear_from_cursor` (VT escape CSI 0 J) executes
/// Then: Column 2 on row 1 is repaired to blank so no orphaned half-glyph remains on screen
#[test]
fn r2_3_clear_from_cursor_repairs_bisected_wide_char_on_cursor_row() {
    let mut grid = Grid::new(10, 3, FG, BG);
    grid.move_to(2, 1);
    grid.print('한');
    grid.move_to(3, 1);

    assert_eq!(grid.cell(2, 1).unwrap().ch, '한');
    assert!(grid.cell(3, 1).unwrap().is_continuation());
    assert_eq!(grid.cursor().col, 3);
    assert_eq!(grid.cursor().row, 1);

    grid.clear_from_cursor();

    assert!(grid.cell(2, 1).unwrap().is_blank());
    assert!(!grid.cell(2, 1).unwrap().is_continuation());
    assert!(grid.cell(3, 1).unwrap().is_blank());
    assert!(!grid.cell(3, 1).unwrap().is_continuation());
}

/// R2.4: `clear_to_cursor` (`CSI 1 J`) repairs bisected wide characters on cursor row
/// Given: A grid holding double-width '한' at columns 2..=3 on row 1, with cursor positioned at (2, 1)
/// When: Function `Grid::clear_to_cursor` (VT escape CSI 1 J) executes
/// Then: Column 3 on row 1 is repaired to blank so no orphaned continuation spacer remains on screen
#[test]
fn r2_4_clear_to_cursor_repairs_bisected_wide_char_on_cursor_row() {
    let mut grid = Grid::new(10, 3, FG, BG);
    grid.move_to(2, 1);
    grid.print('한');
    grid.move_to(2, 1);

    assert_eq!(grid.cell(2, 1).unwrap().ch, '한');
    assert!(grid.cell(3, 1).unwrap().is_continuation());
    assert_eq!(grid.cursor().col, 2);
    assert_eq!(grid.cursor().row, 1);

    grid.clear_to_cursor();

    assert!(grid.cell(3, 1).unwrap().is_blank());
    assert!(!grid.cell(3, 1).unwrap().is_continuation());
    assert!(grid.cell(2, 1).unwrap().is_blank());
}
