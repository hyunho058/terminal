//! Headless snapshot tests for the VT → grid pipeline.
//!
//! These prove the whole terminal *logic* without a GPU or a real shell: feed
//! bytes, snapshot the grid, compare. This is the payoff of C4 (term-core is
//! GPU-free). Update expected output with `cargo insta review` (see `/insta-review`).

use term_core::{Color, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, FG, BG)
}

#[test]
fn prints_two_lines() {
    let mut t = term(10, 3);
    t.advance(b"hello\r\nworld");
    insta::assert_snapshot!(t.snapshot().to_text());
}

#[test]
fn carriage_return_overwrites() {
    let mut t = term(10, 1);
    t.advance(b"aaaaa\rbb");
    insta::assert_snapshot!(t.snapshot().to_text());
}

#[test]
fn cursor_position_then_write() {
    let mut t = term(6, 3);
    // CUP to row 2, col 3 (1-based), then print.
    t.advance(b"\x1b[2;3HX");
    insta::assert_snapshot!(t.snapshot().to_text());
}

#[test]
fn scrolls_when_grid_full() {
    let mut t = term(4, 2);
    // Three logical lines into a 2-row grid: the first must scroll off.
    t.advance(b"aa\r\nbb\r\ncc");
    insta::assert_snapshot!(t.snapshot().to_text());
}

#[test]
fn erase_display_clears_grid() {
    let mut t = term(4, 2);
    t.advance(b"junk\x1b[2J");
    assert_eq!(t.snapshot().to_text().trim(), "");
}

#[test]
fn sgr_sets_foreground_color() {
    let mut t = term(3, 1);
    t.advance(b"\x1b[31mR\x1b[0mN");
    let snap = t.snapshot();
    let red = snap.cell(0, 0).unwrap();
    let reset = snap.cell(1, 0).unwrap();
    assert_eq!(red.ch, 'R');
    assert_eq!(red.fg, Color(0xCD3131)); // ANSI red
    assert_eq!(reset.ch, 'N');
    assert_eq!(reset.fg, FG); // pen reset to default
}

#[test]
fn line_wraps_at_right_edge() {
    let mut t = term(3, 2);
    t.advance(b"abcd"); // 4 chars into 3 columns → wraps to next row
    insta::assert_snapshot!(t.snapshot().to_text());
}
