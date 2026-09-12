//! Golden-frame tests — the pixels, not the bookkeeping.
//!
//! Every other renderer test asserts a *decision*: `reshape_count` says the
//! instance builders ran, `Atlas::generation` says a glyph was packed, the layout
//! tests say a rect was computed. All of those stay green while the window shows
//! nothing, because none of them ever looks at a pixel. That gap is not
//! hypothetical — zeroing the glyph instance count in `Renderer::draw_to` (a blank
//! terminal: no prompt, no output, no cursor) left the entire workspace suite at
//! 1460 passed / 0 failed.
//!
//! These tests close it by rendering through the real path ([`Renderer::capture`]
//! wraps `draw_to`, it does not re-implement it) and reading the result back. The
//! contract they pin is deliberately font-independent: *which cells carry ink*,
//! not what the ink looks like. A cell holding a printable character must contain
//! pixels that differ from the background; a cell with no text anywhere near it
//! must not. That survives a font substitution or an OS antialiasing change, and
//! still fails the instant the grid stops reaching the screen, the insets mis-place
//! it, or a pipeline is dropped.
//!
//! "Anywhere near" is the whole of the imprecision, and it is deliberate: a glyph
//! may bleed antialiasing into the cell beside it, so the blank cell *next to* text
//! is the one place these tests assert nothing. `renderer::testing::touches_text`
//! is that rule, shared with `crates/app/tests/vertical_slice.rs` and applied by
//! both the geometric test and the golden map below.
//!
//! Update the ink-map snapshot with `cargo insta review` (see `/insta-review`).

use common::config::Config;
use renderer::testing::{holds_text, is_ink, touches_text, CellGeometry};
use renderer::{CapturedFrame, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

/// Frame size. Large enough that the default cell metrics yield a grid worth
/// asserting over, and a multiple of 4 in both axes so nothing depends on how a
/// partial trailing cell is handled.
const FRAME_W: u32 = 1200;
const FRAME_H: u32 = 800;

/// The headless renderer, or `None` when GPU tests are opted out of.
///
/// The threshold, the cell rect, the ink predicate and this constructor all live
/// in `renderer::testing`, shared with `crates/app/tests/vertical_slice.rs`. They
/// were duplicated here, under comments in both files asserting the two copies
/// "have to agree" — and they had already drifted in two ways. See that module.
fn headless(cfg: &Config) -> Option<Renderer> {
    renderer::testing::headless(FRAME_W, FRAME_H, cfg, "the golden frames")
}

/// A human-readable map of the rendered frame. This is what makes a golden frame
/// reviewable — a byte hash would fail just as reliably and tell you nothing about
/// *how*.
///
/// Three symbols, not two:
///
/// - `#` the cell carries ink
/// - `.` the cell carries none
/// - `~` the cell holds no text but touches text, so **either** is correct and the
///   map records neither
///
/// The `~` is the same one-cell tolerance
/// [`ink_lands_exactly_in_the_cells_that_hold_text`] argues for and the same
/// [`touches_text`] predicate, and it is here because a snapshot that pins those
/// cells pins a fact that is not one. This map used to read `#####.#####` for row
/// 0: col 5 is the space in "hello world", and cols 4 and 6 hold `o` and `w`. The
/// sibling test in this very file declares that exact cell untrustworthy — a glyph
/// may bleed antialiasing past its advance width — so a font substitution (Menlo
/// for SF Mono) or an OS antialiasing change would turn correct rendering into a
/// red snapshot. What the map still pins exactly is every cell that holds text and
/// every cell far from any, which is what catches a dropped row, a mis-derived
/// pitch, an off-by-one inset and a smear.
fn ink_map(
    frame: &CapturedFrame,
    geo: &CellGeometry,
    snap: &GridSnapshot,
    cols: u16,
    rows: u16,
) -> String {
    let bg = frame.modal_pixel();
    let mut out = String::new();
    for row in 0..rows.min(geo.rows()) {
        for col in 0..cols.min(geo.cols()) {
            out.push(
                if !holds_text(snap, col, row) && touches_text(snap, col, row) {
                    '~'
                } else if geo.cell_has_ink(frame, bg, col, row) {
                    '#'
                } else {
                    '.'
                },
            );
        }
        out.push('\n');
    }
    out
}

/// A terminal sized to what the renderer actually reports, filled by `feed`.
fn render(r: &mut Renderer, feed: &[u8]) -> (GridSnapshot, CapturedFrame) {
    let cfg = Config::default();
    let (cols, rows) = r.grid_size();
    let mut t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    t.advance(feed);
    let snap = t.snapshot();
    let frame = r.capture(&snap).expect("read the rendered frame back");
    (snap, frame)
}

/// The regression that started this file: the terminal draws no glyphs at all.
///
/// Asserted as a share of the text's own cells rather than "at least one pixel",
/// so a frame that renders a single stray glyph and drops the rest still fails.
#[test]
fn a_frame_of_text_is_not_blank() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let (_, frame) = render(&mut r, b"hello world");
    let bg = frame.modal_pixel();

    let inked = frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&px| is_ink(px, bg))
        .count();
    assert!(
        inked > 0,
        "the frame is a solid field of {bg:?} — nothing was drawn"
    );

    // "hello world" is 11 cells; a glyph covers a double-digit number of pixels
    // in each, so a real frame clears this by an order of magnitude. The bound
    // exists to reject a frame that drew a cursor block and nothing else.
    assert!(
        inked > 11 * 4,
        "only {inked} ink pixels for 11 cells of text — the grid barely reached the screen"
    );
}

/// The geometric contract: ink appears in the cells that hold a printable
/// character, and nowhere near the ones that do not.
///
/// Deliberately asymmetric, because the two directions are not equally true.
///
/// **Every printable cell carries ink** is exact, and it is the half that catches
/// a dropped row, an off-by-one inset, a wrong cell size, and a glyph pipeline
/// drawing into the wrong place — none of which move any counter the other tests
/// read.
///
/// **Every blank cell is empty** is *not* exact, and asserting it was a latent
/// false failure. A glyph may put antialiasing past its own advance width into
/// the cell beside it; `crates/app/tests/vertical_slice.rs` records that same
/// measurement from the other side, where the `%` in a real zsh prompt inks the
/// empty cells on both of its sides. That is correct rendering, not a defect.
/// This feed ("hello world", "second line") happens to contain no overhanging
/// glyph, which is the only reason the exact form passed — measured, not assumed:
/// re-run with `hello% w%o%r%l%d %W%M%@ 100%` and the exact form reports three
/// blank cells as regressions. So the reverse direction carries a one-cell
/// tolerance: a blank cell **none of whose neighbours holds text** must be empty.
/// A smear, an uncleared target and a wholesale misplacement all still fail here,
/// because none of them is confined to one cell of slack.
#[test]
fn ink_lands_exactly_in_the_cells_that_hold_text() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    // Row 0 and row 2 carry text; row 1 is deliberately blank, so a renderer that
    // smears rows or mis-derives the row pitch fails here rather than looking fine.
    let (snap, frame) = render(&mut r, b"hello world\r\n\r\nsecond line");
    let geo = CellGeometry::of(&r);
    let bg = frame.modal_pixel();

    let mut wrong = Vec::new();
    for row in 0..geo.rows() {
        for col in 0..geo.cols() {
            let has_ink = geo.cell_has_ink(&frame, bg, col, row);
            if holds_text(&snap, col, row) {
                if !has_ink {
                    wrong.push(format!("({col},{row}) holds text but drew nothing"));
                }
            } else if has_ink && !touches_text(&snap, col, row) {
                wrong.push(format!(
                    "({col},{row}) drew ink, and neither it nor any cell touching it holds text"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} cells disagree between the grid and the pixels:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The reviewable golden: the shape of a real frame, as a per-cell ink map.
#[test]
fn the_rendered_frame_matches_its_golden_ink_map() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let (snap, frame) = render(&mut r, b"hello world\r\n\r\nsecond line\r\n  indented");
    let geo = CellGeometry::of(&r);
    insta::assert_snapshot!(ink_map(&frame, &geo, &snap, 24, 5));
}

/// The background pipeline, and the channel order [`Renderer::capture`]
/// normalises to. A cell with a red background must read red — if the readback
/// (or the shader) transposed R and B this passes as blue, so the assertion is
/// on the ordering, not merely on "some color".
#[test]
fn a_red_background_cell_paints_red_pixels() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    // SGR 41 (red bg) on a run of spaces: background quads only, no glyph ink to
    // confuse the sample.
    let (_, frame) = render(&mut r, b"\x1b[41m    \x1b[0m");
    let geo = CellGeometry::of(&r);

    let (x0, y0, x1, y1) = geo.cell_rect(1, 0, &frame);
    let mid = frame.pixel((x0 + x1) / 2, (y0 + y1) / 2);
    // Widened to i32 before the comparison. `mid[1] + 32` is `u8` arithmetic and
    // panics with "attempt to add with overflow" as soon as green or blue exceeds
    // 223 — which is precisely a light or white readback, one of the regressions
    // this test exists to name. An overflow panic there would replace the
    // diagnostic below with a message about the test's own arithmetic.
    let [r_ch, g_ch, b_ch] = [mid[0] as i32, mid[1] as i32, mid[2] as i32];
    assert!(
        r_ch > g_ch + 32 && r_ch > b_ch + 32,
        "cell (1,0) has a red background but read back as {mid:?} (RGBA)"
    );
}

/// The control. Without it the two tests above could both pass on a frame where
/// *everything* reads as ink — a background that never got cleared, say — since
/// "differs from the modal pixel" would then be true almost everywhere.
#[test]
fn an_empty_grid_paints_no_ink_at_all() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let (snap, frame) = render(&mut r, b"");
    let geo = CellGeometry::of(&r);
    let bg = frame.modal_pixel();

    let inked = frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&px| is_ink(px, bg))
        .count();
    assert_eq!(
        inked, 0,
        "an empty grid painted {inked} non-background pixels; \
         the ink metric is measuring something other than drawn content"
    );
    assert_eq!(
        ink_map(&frame, &geo, &snap, 8, 3),
        "........\n........\n........\n",
        "an empty grid must map to no ink"
    );
}
