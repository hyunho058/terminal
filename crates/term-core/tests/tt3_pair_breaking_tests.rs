use term_core::{Cell, Color, Grid};
use unicode_width::UnicodeWidthChar;

/// R3.1: `break_wide_pair_at` called before printing narrow char over spacer blanks the leader
/// - Given: A grid cell at `(col, row)` holding `Cell::CONTINUATION` whose leader at `(col - 1, row)` is double-width
/// - When: Function `Grid::print` executes with a single-width character at `(col, row)`
/// - Then: The leader at `(col - 1, row)` is replaced with a blank cell carrying the active pen background before writing `(col, row)`
#[test]
fn test_break_wide_pair_narrow_over_spacer_blanks_leader() {
    let default_fg = Color(0xE0E0E0);
    let default_bg = Color(0x101010);
    let mut grid = Grid::new(10, 2, default_fg, default_bg);

    let (col, row) = (3, 0);
    grid.move_to(col - 1, row);
    grid.print('한');
    assert_eq!(grid.cell(col - 1, row).map(|c| c.ch), Some('한'));
    assert_eq!(grid.cell(col, row).map(|c| c.ch), Some(Cell::CONTINUATION));

    let active_bg = Color(0x224466);
    grid.set_pen(None, Some(active_bg));
    grid.move_to(col, row);

    grid.print('x');

    let leader = grid.cell(col - 1, row).expect("leader cell exists");
    assert_eq!(leader.ch, ' ');
    assert_eq!(leader.bg, active_bg);

    let written = grid.cell(col, row).expect("written cell exists");
    assert_eq!(written.ch, 'x');
}

/// R3.2: `break_wide_pair_at` called before printing narrow char over leader blanks the spacer
/// - Given: A grid cell at `(col, row)` holding a double-width character with its spacer at `(col + 1, row)`
/// - When: Function `Grid::print` executes with a single-width character at `(col, row)`
/// - Then: The spacer at `(col + 1, row)` is replaced with a blank cell carrying the active pen background before writing `(col, row)`
#[test]
fn test_break_wide_pair_narrow_over_leader_blanks_spacer() {
    let default_fg = Color(0xE0E0E0);
    let default_bg = Color(0x101010);
    let mut grid = Grid::new(10, 2, default_fg, default_bg);

    let (col, row) = (2, 0);
    grid.move_to(col, row);
    grid.print('한');
    assert_eq!(grid.cell(col, row).map(|c| c.ch), Some('한'));
    assert_eq!(
        grid.cell(col + 1, row).map(|c| c.ch),
        Some(Cell::CONTINUATION)
    );

    let active_bg = Color(0x335577);
    grid.set_pen(None, Some(active_bg));
    grid.move_to(col, row);

    grid.print('y');

    let spacer = grid.cell(col + 1, row).expect("spacer cell exists");
    assert_eq!(spacer.ch, ' ');
    assert_eq!(spacer.bg, active_bg);

    let written = grid.cell(col, row).expect("written cell exists");
    assert_eq!(written.ch, 'y');
}

/// R3.3: `break_wide_pair_at` called when printing double-width char cleans both target cells and their partners
/// - Given: A grid row with double-width characters at columns 0..=1 and 2..=3, and cursor at column 1
/// - When: Function `Grid::print` executes with a double-width character at column 1
/// - Then: Column 0 is blanked, column 1 receives the new wide leader, column 2 receives the new continuation spacer, and column 3 is blanked
#[test]
fn test_break_wide_pair_wide_over_split_cleans_both_and_partners() {
    let default_fg = Color(0xE0E0E0);
    let default_bg = Color(0x101010);
    let mut grid = Grid::new(10, 2, default_fg, default_bg);

    grid.move_to(0, 0);
    grid.print('가');
    grid.move_to(2, 0);
    grid.print('나');
    assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('가'));
    assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(Cell::CONTINUATION));
    assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('나'));
    assert_eq!(grid.cell(3, 0).map(|c| c.ch), Some(Cell::CONTINUATION));

    grid.move_to(1, 0);
    grid.print('다');

    assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some(' '));
    assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('다'));
    assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some(Cell::CONTINUATION));
    assert_eq!(grid.cell(3, 0).map(|c| c.ch), Some(' '));
}

/// R3.4: `repair_wide_pairs` eliminates orphan spacers and orphan leaders across a row
/// - Given: A row containing an orphan `Cell::CONTINUATION` at column 2 (preceded by single-width `'a'`) and an orphan wide leader at column 6 (followed by `'b'`)
/// - When: Public method `Grid::repair_wide_pairs(row, fill)` executes
/// - Then: Both column 2 and column 6 are replaced with `fill`, leaving no invalid continuation or orphan leader on the row
#[test]
fn test_repair_wide_pairs_eliminates_orphan_spacers_and_leaders() {
    let default_fg = Color(0xE0E0E0);
    let default_bg = Color(0x101010);
    let mut grid = Grid::new(10, 2, default_fg, default_bg);
    let row = 0;

    // Given: A row containing an orphan Cell::CONTINUATION at column 2 (preceded by single-width 'a')
    // and an orphan wide leader at column 6 (followed by 'b')
    grid.move_to(1, row);
    grid.print('한');
    grid.break_wide_pair_at(2, row);
    grid.move_to(1, row);
    grid.print('a');

    grid.move_to(6, row);
    grid.print('글');
    grid.break_wide_pair_at(6, row);
    grid.move_to(7, row);
    grid.print('b');

    assert_eq!(grid.cell(1, row).map(|c| c.ch), Some('a'));
    assert_eq!(grid.cell(2, row).map(|c| c.ch), Some(Cell::CONTINUATION));
    assert_eq!(grid.cell(6, row).map(|c| c.ch), Some('글'));
    assert_eq!(grid.cell(7, row).map(|c| c.ch), Some('b'));

    // When: Public method Grid::repair_wide_pairs(row, fill) executes
    let fill = Cell::blank(Color(0xFFFFFF), Color(0x000000));
    grid.repair_wide_pairs(row, fill);

    // Then: Both column 2 and column 6 are replaced with fill, leaving no invalid continuation or orphan leader on the row
    assert_eq!(grid.cell(2, row), Some(&fill));
    assert_eq!(grid.cell(6, row), Some(&fill));

    assert_eq!(grid.cell(1, row).map(|c| c.ch), Some('a'));
    assert_eq!(grid.cell(7, row).map(|c| c.ch), Some('b'));

    for col in 0..grid.cols() {
        let cell = grid.cell(col, row).unwrap();
        if cell.is_continuation() {
            assert!(col > 0, "continuation at col 0 is invalid");
            let prev = grid.cell(col - 1, row).unwrap();
            assert_eq!(
                prev.ch.width(),
                Some(2),
                "continuation must follow a 2-width leader"
            );
            assert!(!prev.is_continuation(), "leader cannot be a continuation");
        } else if cell.ch.width() == Some(2) {
            assert!(col + 1 < grid.cols(), "leader at right edge is orphaned");
            let next = grid.cell(col + 1, row).unwrap();
            assert!(
                next.is_continuation(),
                "leader must be followed by a continuation spacer"
            );
        }
    }
}
