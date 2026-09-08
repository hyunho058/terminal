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
//! pixels that differ from the background; an empty cell must not. That survives a
//! font substitution or an OS antialiasing change, and still fails the instant the
//! grid stops reaching the screen, the insets mis-place it, or a pipeline is
//! dropped.
//!
//! Update the ink-map snapshot with `cargo insta review` (see `/insta-review`).

use common::config::Config;
use renderer::{CapturedFrame, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

/// A pixel counts as ink when any channel differs from the background by more
/// than this. Well above antialiasing noise on the background side, well below
/// the contrast of any legible glyph — the same margin the ai-monitor panel's
/// readback test uses for "a glyph landed here".
const INK_THRESHOLD: i32 = 24;

/// Frame size. Large enough that the default cell metrics yield a grid worth
/// asserting over, and a multiple of 4 in both axes so nothing depends on how a
/// partial trailing cell is handled.
const FRAME_W: u32 = 1200;
const FRAME_H: u32 = 800;

/// Build the headless renderer, or fail loudly.
///
/// The crate's older GPU tests `return` when `new_headless` fails, which turns a
/// missing adapter or a missing font into a **passing** test — the failure mode
/// these tests exist to detect, reproduced in the harness. So this panics
/// instead. `SKIP_GPU_TESTS=1` opts out explicitly for an environment that
/// genuinely has no GPU; the point is that skipping has to be someone's decision
/// rather than a silent default.
fn headless(cfg: &Config) -> Option<Renderer> {
    match Renderer::new_headless(FRAME_W, FRAME_H, 1.0, cfg) {
        Ok(r) => Some(r),
        Err(e) => {
            if std::env::var_os("SKIP_GPU_TESTS").is_some() {
                eprintln!("SKIP_GPU_TESTS set; skipping golden frame: {e}");
                return None;
            }
            panic!(
                "headless renderer unavailable ({e}). The golden-frame tests are the \
                 only check that anything reaches the screen, so this fails rather \
                 than skipping. Set SKIP_GPU_TESTS=1 to opt out deliberately."
            );
        }
    }
}

/// The grid's pixel geometry as the shader lays it out: origin at the insets,
/// cells of `cell_size`.
struct Geometry {
    origin: (f32, f32),
    cell: (f32, f32),
    cols: u16,
    rows: u16,
}

impl Geometry {
    fn of(r: &Renderer) -> Geometry {
        let (cols, rows) = r.grid_size();
        Geometry {
            origin: (r.left_inset(), r.top_inset()),
            cell: r.cell_size(),
            cols,
            rows,
        }
    }

    /// Half-open pixel bounds of one cell, clamped to the frame.
    ///
    /// `floor` on **both** edges, so consecutive cells tile exactly: this cell's
    /// `x1` is the next one's `x0`. With `ceil` on the far edge — as this had —
    /// neighbouring rects overlap by a pixel whenever the cell size is
    /// fractional, and one glyph's outermost column reads as ink in two cells.
    /// `crates/app/tests/vertical_slice.rs` derives the same rect the same way;
    /// the two definitions have to agree or the "does this cell hold ink"
    /// question means something different on each side.
    fn cell_rect(&self, col: u16, row: u16, frame: &CapturedFrame) -> (u32, u32, u32, u32) {
        let x0 = self.origin.0 + col as f32 * self.cell.0;
        let y0 = self.origin.1 + row as f32 * self.cell.1;
        let x1 = (x0 + self.cell.0).min(frame.width() as f32);
        let y1 = (y0 + self.cell.1).min(frame.height() as f32);
        (
            x0.max(0.0).floor() as u32,
            y0.max(0.0).floor() as u32,
            x1.max(0.0).floor() as u32,
            y1.max(0.0).floor() as u32,
        )
    }
}

fn is_ink(px: [u8; 4], bg: [u8; 4]) -> bool {
    (0..3).any(|c| (px[c] as i32 - bg[c] as i32).abs() > INK_THRESHOLD)
}

/// Does this cell contain any pixel that is not the background?
fn cell_has_ink(frame: &CapturedFrame, geo: &Geometry, bg: [u8; 4], col: u16, row: u16) -> bool {
    let (x0, y0, x1, y1) = geo.cell_rect(col, row, frame);
    (y0..y1).any(|y| (x0..x1).any(|x| is_ink(frame.pixel(x, y), bg)))
}

/// A human-readable map of the rendered frame: `#` where a cell carries ink,
/// `.` where it does not. This is what makes a golden frame reviewable — a byte
/// hash would fail just as reliably and tell you nothing about *how*.
fn ink_map(frame: &CapturedFrame, geo: &Geometry, cols: u16, rows: u16) -> String {
    let bg = frame.modal_pixel();
    let mut out = String::new();
    for row in 0..rows.min(geo.rows) {
        for col in 0..cols.min(geo.cols) {
            out.push(if cell_has_ink(frame, geo, bg, col, row) {
                '#'
            } else {
                '.'
            });
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
        .chunks_exact(4)
        .filter(|px| is_ink([px[0], px[1], px[2], px[3]], bg))
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
    let geo = Geometry::of(&r);
    let bg = frame.modal_pixel();

    let printable = |col: u16, row: u16| {
        let ch = snap.cell(col, row).map(|c| c.ch).unwrap_or(' ');
        !ch.is_whitespace() && ch != '\0'
    };
    // Does (col, row) or any of its eight neighbours hold a printable character?
    let touches_text = |col: u16, row: u16| {
        (-1i32..=1).any(|dy| {
            (-1i32..=1).any(|dx| {
                let (c, r) = (col as i32 + dx, row as i32 + dy);
                (0..geo.cols as i32).contains(&c)
                    && (0..geo.rows as i32).contains(&r)
                    && printable(c as u16, r as u16)
            })
        })
    };

    let mut wrong = Vec::new();
    for row in 0..geo.rows {
        for col in 0..geo.cols {
            let has_ink = cell_has_ink(&frame, &geo, bg, col, row);
            if printable(col, row) {
                if !has_ink {
                    wrong.push(format!("({col},{row}) holds text but drew nothing"));
                }
            } else if has_ink && !touches_text(col, row) {
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
    let (_, frame) = render(&mut r, b"hello world\r\n\r\nsecond line\r\n  indented");
    let geo = Geometry::of(&r);
    insta::assert_snapshot!(ink_map(&frame, &geo, 24, 5));
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
    let geo = Geometry::of(&r);

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
    let (_, frame) = render(&mut r, b"");
    let geo = Geometry::of(&r);
    let bg = frame.modal_pixel();

    let inked = frame
        .pixels()
        .chunks_exact(4)
        .filter(|px| is_ink([px[0], px[1], px[2], px[3]], bg))
        .count();
    assert_eq!(
        inked, 0,
        "an empty grid painted {inked} non-background pixels; \
         the ink metric is measuring something other than drawn content"
    );
    assert_eq!(
        ink_map(&frame, &geo, 8, 3),
        "........\n........\n........\n",
        "an empty grid must map to no ink"
    );
}
