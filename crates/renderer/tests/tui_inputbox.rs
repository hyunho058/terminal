use renderer::row_chars;
use term_core::{Color, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, FG, BG)
}

#[test]
fn r0_1_renderer_row_chars_consecutive_columns_after_cjk_prompt() {
    let mut t = term(80, 24);
    // When: Terminal consumes \r[한국어] field\x1b[K
    t.advance("\r[한국어] field\x1b[K".as_bytes());

    let snap = t.snapshot();
    let row = row_chars(&snap, 0);

    // Characters up to column 13
    assert_eq!(
        &row.chars[..11],
        &['[', '한', '국', '어', ']', ' ', 'f', 'i', 'e', 'l', 'd']
    );
    // Exact column anchors: wide characters skip spacer columns (1->3->5->7),
    // and subsequent ASCII characters occupy strictly consecutive columns: 9, 10, 11, 12, 13
    assert_eq!(&row.char_col[..11], &[0, 1, 3, 5, 7, 8, 9, 10, 11, 12, 13]);
}

#[test]
fn r0_2_renderer_row_chars_prompt_line_redraw_no_stray_chars() {
    let mut t = term(80, 24);
    // Initial long prompt input
    t.advance("\r[한국어] longer_previous_input\x1b[K".as_bytes());

    // Redraw with shorter prompt input
    t.advance("\r[한국어] short\x1b[K".as_bytes());

    let snap = t.snapshot();
    let row = row_chars(&snap, 0);

    // Only "[한국어] short" appears; no trailing characters from "longer_previous_input"
    assert_eq!(
        &row.chars[..11],
        &['[', '한', '국', '어', ']', ' ', 's', 'h', 'o', 'r', 't']
    );
    assert_eq!(&row.char_col[..11], &[0, 1, 3, 5, 7, 8, 9, 10, 11, 12, 13]);

    // All trailing columns hold blank spaces
    for (i, &ch) in row.chars.iter().enumerate().skip(11) {
        assert_eq!(ch, ' ', "column {} should be space", row.char_col[i]);
    }
}

#[test]
fn tui_cursor_moves_and_backspace_preserve_column_alignment() {
    let mut t = term(80, 24);
    // Simulate interactive prompt: typing prompt, typing input, moving back, editing
    t.advance("\r> [한국어] abcd\x1b[K".as_bytes());
    // Move cursor left 2 cells (over 'cd')
    t.advance(b"\x1b[2D");
    // Overwrite with 'xy'
    t.advance(b"xy");

    let snap = t.snapshot();
    let row = row_chars(&snap, 0);

    // Expected text: "> [한국어] abxy"
    // Columns:
    // '>' at 0
    // ' ' at 1
    // '[' at 2
    // '한' at 3 (spacer at 4)
    // '국' at 5 (spacer at 6)
    // '어' at 7 (spacer at 8)
    // ']' at 9
    // ' ' at 10
    // 'a' at 11
    // 'b' at 12
    // 'x' at 13
    // 'y' at 14
    let expected_chars = vec!['>', ' ', '[', '한', '국', '어', ']', ' ', 'a', 'b', 'x', 'y'];
    let expected_cols = vec![0, 1, 2, 3, 5, 7, 9, 10, 11, 12, 13, 14];

    assert_eq!(&row.chars[..12], &expected_chars[..]);
    assert_eq!(&row.char_col[..12], &expected_cols[..]);
}
