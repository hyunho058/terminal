//! Test support for asserting what a rendered frame **actually contains**.
//!
//! One definition, two crates. `renderer/tests/golden_frame.rs` asks "did this
//! constructed snapshot reach the pixels" and `crates/app/tests/vertical_slice.rs`
//! asks it of a real shell's output, and both answers rest on the same three
//! decisions: where a cell's pixels are, how different from the background a pixel
//! has to be to count as ink, and when a missing renderer is a skip rather than a
//! failure.
//!
//! Each of those was written out by hand in both files, with a comment in each
//! saying the two "have to agree" — and they had already drifted: one floored the
//! far edge of the cell rect and the other truncated it, and only one of the two
//! refused to let `SKIP_GPU_TESTS` swallow a missing font. An invariant maintained
//! by comment is not maintained. It lives here instead, where agreement is
//! structural.
//!
//! Not `#[cfg(test)]`: an integration test is a separate crate and could not name
//! any of this if it were.

use common::config::Config;

use term_core::GridSnapshot;

use crate::{CapturedFrame, Renderer, RendererError};

/// A pixel counts as ink when any channel differs from the background by more than
/// this. Well above antialiasing noise on the background side, well below the
/// contrast of any legible glyph — the same margin the ai-monitor panel's readback
/// test uses for "a glyph landed here".
pub const INK_THRESHOLD: i32 = 24;

/// Does this pixel differ from the background by more than [`INK_THRESHOLD`]?
///
/// Widened to `i32` before subtracting: in `u8` the difference of two channels
/// wraps, and a dark glyph on a light background would read as no ink at all.
pub fn is_ink(px: [u8; 4], bg: [u8; 4]) -> bool {
    (0..3).any(|c| (px[c] as i32 - bg[c] as i32).abs() > INK_THRESHOLD)
}

/// Does the grid hold a printable character at `(col, row)`?
///
/// Out of bounds is blank, not a panic — the neighbour walk in [`touches_text`]
/// relies on that.
pub fn holds_text(snap: &GridSnapshot, col: u16, row: u16) -> bool {
    let ch = snap.cell(col, row).map(|c| c.ch).unwrap_or(' ');
    !ch.is_whitespace() && ch != '\0'
}

/// Does `(col, row)` or any of its eight neighbours hold a printable character?
///
/// The one-cell tolerance both crates' pixel assertions are built on. A glyph may
/// put antialiasing past its own advance width into the cell beside it — measured,
/// not assumed: the `%` in a real zsh prompt inks the empty cells on both of its
/// sides, and a feed of `hello% w%o%r%l%d %W%M%@ 100%` reports three blank cells as
/// regressions under the exact rule. That is correct rendering, so "this blank cell
/// is empty" is only a fact where nothing adjacent could have bled into it. A
/// smear, an uncleared target and a wholesale misplacement all still fail, because
/// none of them is confined to one cell of slack.
pub fn touches_text(snap: &GridSnapshot, col: u16, row: u16) -> bool {
    (-1i32..=1).any(|dy| {
        (-1i32..=1).any(|dx| {
            let (c, r) = (col as i32 + dx, row as i32 + dy);
            c >= 0 && r >= 0 && holds_text(snap, c as u16, r as u16)
        })
    })
}

/// The grid's pixel geometry as the shader lays it out: origin at the insets, cells
/// of `cell_size`.
pub struct CellGeometry {
    origin: (f32, f32),
    cell: (f32, f32),
    cols: u16,
    rows: u16,
}

impl CellGeometry {
    /// Read it off the renderer that drew the frame.
    pub fn of(r: &Renderer) -> CellGeometry {
        let (cols, rows) = r.grid_size();
        CellGeometry {
            origin: (r.left_inset(), r.top_inset()),
            cell: r.cell_size(),
            cols,
            rows,
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Half-open pixel bounds of one cell, clamped to the frame.
    ///
    /// `floor` on **both** edges, so consecutive cells tile exactly: this cell's
    /// `x1` is the next one's `x0`. With `ceil` on the far edge — as one of the two
    /// callers had — neighbouring rects overlap by a pixel whenever the cell size
    /// is fractional, and one glyph's outermost column reads as ink in two cells.
    pub fn cell_rect(&self, col: u16, row: u16, frame: &CapturedFrame) -> (u32, u32, u32, u32) {
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

    /// Does this cell contain any pixel that is not the background?
    pub fn cell_has_ink(&self, frame: &CapturedFrame, bg: [u8; 4], col: u16, row: u16) -> bool {
        let (x0, y0, x1, y1) = self.cell_rect(col, row, frame);
        (y0..y1).any(|y| (x0..x1).any(|x| is_ink(frame.pixel(x, y), bg)))
    }
}

/// Build a headless renderer for a pixel test, or `None` when GPU tests are opted
/// out of.
///
/// `what` names the leg being skipped, for the diagnostic (e.g. "the golden
/// frames", "the pixel leg").
///
/// A helper that turns "the renderer would not start" into a **passing** test
/// reproduces in the harness the exact failure these tests exist to detect. Both
/// callers had that shape once — every `RendererError` a bare `return` with a line
/// to stderr, gated by nothing — so the skip was nobody's decision. This panics
/// instead, and `SKIP_GPU_TESTS=1` is the explicit opt-out for a machine that
/// genuinely has no GPU.
///
/// The two failures are split because they are not the same kind of fact.
/// [`Renderer::new_headless`] loads the `FontSet` *before* it requests an adapter,
/// so `NoFont` arrives on every machine, GPU or not: it says the Menlo / SF Mono /
/// AppleSDGothicNeo chain stopped resolving, which is a regression in the code
/// under test rather than a fact about the hardware. `SKIP_GPU_TESTS` therefore
/// does not cover it. That split is the reason this function is shared rather than
/// copied: one copy honoured it and the other did not, which on a headless runner —
/// the only machine that sets the variable — left a broken font pipeline reported
/// by nothing at all.
pub fn headless(width: u32, height: u32, cfg: &Config, what: &str) -> Option<Renderer> {
    headless_scaled(width, height, 1.0, cfg, what)
}

/// [`headless`] at a backing `scale_factor` — the same skip-or-fail policy, so a 2×
/// (Retina) golden frame does not copy it.
pub fn headless_scaled(
    width: u32,
    height: u32,
    scale_factor: f64,
    cfg: &Config,
    what: &str,
) -> Option<Renderer> {
    match Renderer::new_headless(width, height, scale_factor, cfg) {
        Ok(r) => Some(r),
        Err(RendererError::NoFont(tried)) => panic!(
            "no usable monospace font (tried {tried}). This is the font pipeline \
             failing, not a missing GPU — it is decided before any adapter is \
             requested — so SKIP_GPU_TESTS deliberately does not opt out of it."
        ),
        Err(e) => {
            if std::env::var_os("SKIP_GPU_TESTS").is_some() {
                eprintln!("SKIP_GPU_TESTS set; skipping {what}: {e}");
                return None;
            }
            panic!(
                "headless renderer unavailable ({e}). {what} is the only check that \
                 anything reaches the screen, so this fails rather than skipping. \
                 Set SKIP_GPU_TESTS=1 to opt out deliberately."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_present_gpu_is_handed_a_renderer_not_a_skip() {
        // The other half of the policy: unless a machine opts out, the pixel tests get a
        // renderer or fail. A `headless` that answered `None` here would turn every
        // golden frame into a silent pass.
        if std::env::var_os("SKIP_GPU_TESTS").is_some() {
            return;
        }
        let cfg = common::config::Config::default();
        let r = super::headless_scaled(64, 64, 2.0, &cfg, "the policy test")
            .expect("a renderer, or a panic naming why not");
        assert_eq!(r.scale_factor(), 2.0);
        assert!(super::headless(64, 64, &cfg, "the policy test").is_some());
    }
}
