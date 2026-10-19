use std::mem::size_of;
use term_core::{Cell, CellAttrs, Color, Grid, GridSnapshot};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn set_grid_cell(grid: &mut Grid, col: u16, row: u16, cell: Cell) {
    let idx = (row * grid.cols() + col) as usize;
    let ptr = grid.cells().as_ptr() as *mut Cell;
    unsafe {
        *ptr.add(idx) = cell;
    }
}

/// R4.1: `GridSnapshot::to_text` extracts orphan continuation cell as `' '` instead of omitting
/// Given: A `GridSnapshot` containing an orphan `Cell::CONTINUATION` at column 1 preceded by single-width `'a'` at column 0 and followed by `'b'` at column 2
/// When: Method `GridSnapshot::to_text` executes
/// Then: The extracted string contains `"a b"`, preserving the column offset and matching the visual rendering
#[test]
fn r4_1_grid_snapshot_to_text_extracts_orphan_continuation_as_space() {
    let mut grid = Grid::new(10, 1, FG, BG);

    set_grid_cell(
        &mut grid,
        0,
        0,
        Cell {
            ch: 'a',
            fg: FG,
            bg: BG,
            attrs: CellAttrs::NONE,
        },
    );
    set_grid_cell(
        &mut grid,
        1,
        0,
        Cell {
            ch: Cell::CONTINUATION,
            fg: FG,
            bg: BG,
            attrs: CellAttrs::NONE,
        },
    );
    set_grid_cell(
        &mut grid,
        2,
        0,
        Cell {
            ch: 'b',
            fg: FG,
            bg: BG,
            attrs: CellAttrs::NONE,
        },
    );

    let snapshot = GridSnapshot::from_grid(&grid);
    assert_eq!(snapshot.cell(0, 0).unwrap().ch, 'a');
    assert_eq!(snapshot.cell(1, 0).unwrap().ch, Cell::CONTINUATION);
    assert_eq!(snapshot.cell(2, 0).unwrap().ch, 'b');

    let text = snapshot.to_text();
    assert!(
        text.contains("a b"),
        "GridSnapshot::to_text must contain \"a b\", preserving the column offset and matching visual rendering. Got: {:?}",
        text
    );
}

/// R4.2: `Grid::selection_text` extracts orphan continuation cell as `' '` instead of omitting
/// Given: A text selection spanning a column holding an orphan `Cell::CONTINUATION`
/// When: Method `Grid::selection_text` extracts the selected string
/// Then: The extracted string contains a space `' '` for the orphan cell, ensuring clipboard copy matches the on-screen rendered column position
#[test]
fn r4_2_grid_selection_text_extracts_orphan_continuation_as_space() {
    let mut grid = Grid::new(10, 1, FG, BG);

    set_grid_cell(
        &mut grid,
        0,
        0,
        Cell {
            ch: 'a',
            fg: FG,
            bg: BG,
            attrs: CellAttrs::NONE,
        },
    );
    set_grid_cell(
        &mut grid,
        1,
        0,
        Cell {
            ch: Cell::CONTINUATION,
            fg: FG,
            bg: BG,
            attrs: CellAttrs::NONE,
        },
    );
    set_grid_cell(
        &mut grid,
        2,
        0,
        Cell {
            ch: 'b',
            fg: FG,
            bg: BG,
            attrs: CellAttrs::NONE,
        },
    );

    let anchor = grid.point_at(0, 0);
    let focus = grid.point_at(2, 0);
    grid.set_selection(anchor, focus);

    let extracted = grid
        .selection_text()
        .expect("selection text should be present");
    assert_eq!(
        extracted, "a b",
        "Grid::selection_text must extract orphan continuation cell as ' ', matching on-screen rendered column position. Got: {:?}",
        extracted
    );
}

/// R4.3: Memory layout of `Cell` is unchanged
/// Given: The `term_core::Cell` struct
/// When: Checked at compile time via `std::mem::size_of::<Cell>()` and field offsets
/// Then: `Cell` remains 16 bytes with `Cell::CONTINUATION == '\0'`, preserving cache alignment and `idle-cpu` hashing
#[test]
fn r4_3_cell_memory_layout_unchanged() {
    assert_eq!(
        size_of::<Cell>(),
        16,
        "Cell memory size must remain exactly 16 bytes"
    );
    assert_eq!(
        Cell::CONTINUATION,
        '\0',
        "Cell::CONTINUATION sentinel char must remain '\\0'"
    );

    // Field offsets match 16-byte layout contract
    assert_eq!(std::mem::offset_of!(Cell, ch), 0);
    assert_eq!(std::mem::offset_of!(Cell, fg), 4);
    assert_eq!(std::mem::offset_of!(Cell, bg), 8);
    assert_eq!(std::mem::offset_of!(Cell, attrs), 12);
}
