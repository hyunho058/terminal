//! Acceptance tests for `specs/mouse-drag-reporting/spec.md` R3 (D4): an off-grid
//! pointer during a reported drag clamps to the edge of the grid.
//!
//! Covers:
//! - R3.1: Left of and above the grid maps to column 0 and row 0
//! - R3.2: The monitor band and beyond the bottom map to the last column and row
//! - R3.3: An in-grid pointer matches `pixel_to_cell`
//! - R3.4: Degenerate geometry gives no cell
//! - R3.5: Negative coordinates and the top inset clamp to row 0 and column 0

use app::{pixel_to_cell, pixel_to_cell_clamped};

#[test]
fn r3_1_left_of_and_above_grid_clamps_to_origin() {
    let clamped =
        pixel_to_cell_clamped((50.0, 10.0), (10.0, 20.0), 200.0, 30.0, 1000.0, 0.0, 80, 24);
    assert_eq!(clamped, Some((0, 0)));

    let unclamped = pixel_to_cell((50.0, 10.0), (10.0, 20.0), 200.0, 30.0, 1000.0, 0.0, 80, 24);
    assert_eq!(unclamped, None);
}

#[test]
fn r3_2_monitor_band_and_below_bottom_clamp_to_last_cell() {
    let clamped = pixel_to_cell_clamped(
        (950.0, 5000.0),
        (10.0, 20.0),
        0.0,
        0.0,
        1000.0,
        200.0,
        80,
        24,
    );
    assert_eq!(clamped, Some((79, 23)));
}

#[test]
fn r3_3_in_grid_pointer_matches_pixel_to_cell() {
    let clamped = pixel_to_cell_clamped((125.0, 45.0), (10.0, 20.0), 0.0, 0.0, 1000.0, 0.0, 80, 24);
    let unclamped = pixel_to_cell((125.0, 45.0), (10.0, 20.0), 0.0, 0.0, 1000.0, 0.0, 80, 24);
    assert_eq!(clamped, Some((12, 2)));
    assert_eq!(unclamped, Some((12, 2)));
}

#[test]
fn r3_4_degenerate_geometry_gives_no_cell() {
    let cursors = [
        (0.0, 0.0),
        (125.0, 45.0),
        (-40.0, -15.0),
        (950.0, 5000.0),
        (1e9, 1e9),
    ];
    for cursor in cursors {
        assert_eq!(
            pixel_to_cell_clamped(cursor, (10.0, 20.0), 0.0, 0.0, 1000.0, 0.0, 0, 24),
            None,
            "cols == 0 must give no cell for cursor {cursor:?}"
        );
        assert_eq!(
            pixel_to_cell_clamped(cursor, (0.0, 20.0), 0.0, 0.0, 1000.0, 0.0, 80, 24),
            None,
            "cell width 0.0 must give no cell for cursor {cursor:?}"
        );
    }
}

#[test]
fn r3_5_negative_coordinates_and_top_inset_clamp_to_row_and_column_zero() {
    let negative = pixel_to_cell_clamped(
        (-40.0, -15.0),
        (10.0, 20.0),
        200.0,
        30.0,
        1000.0,
        0.0,
        80,
        24,
    );
    assert_eq!(negative, Some((0, 0)));

    let in_top_inset = pixel_to_cell_clamped(
        (215.0, 29.0),
        (10.0, 20.0),
        200.0,
        30.0,
        1000.0,
        0.0,
        80,
        24,
    );
    assert_eq!(in_top_inset, Some((1, 0)));
}
