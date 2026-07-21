//! Source-compatibility façade (redesign D1) **plus** the shared text-layout helpers
//! every non-grid layer draws with (Hangul fix #6, D10/R5.4).
//!
//! The tab UI moved to a left sidebar whose real implementation lives in
//! [`crate::sidebar_row`]; the re-export below keeps the module path `renderer::tab_bar`
//! and the item names the app still uses (`terminal_app.rs`) valid verbatim.
//!
//! The column-based text helpers ([`char_cols`], [`advance_px`], [`pen_layout`],
//! [`truncate_cols`], [`layout_line`]) stay here because [`crate::overlay_layer`] and
//! [`crate::preedit_layer`] consume them — so the font-fallback + display-width rules
//! live in exactly one place and cannot drift into copies. Layout is **column-based**,
//! not char-based (R5.2/R5.3): a Hangul basename is 2 columns per char, so a fixed
//! per-char advance overlapped glyphs and a char-counting truncate overflowed the width.

use unicode_width::UnicodeWidthChar;

use crate::atlas::GlyphEntry;
use crate::FrameCtx;

pub use crate::sidebar_row::{
    tab_at, tab_height_px, width_px, SidebarRow, TabBar, TabBarModel, TabModelHandle, TAB_BAR_Z,
};

/// Display width of `c` in terminal **columns** (R5.2).
///
/// `unicode-width` (same crate + version `term-core` uses, so the engine and the
/// renderer can never disagree) reports 2 for Hangul/CJK. `None` means a control char,
/// which we lay out as one column rather than collapsing it.
pub(crate) fn char_cols(c: char) -> usize {
    c.width().unwrap_or(1)
}

/// Pen advance in px for `c` at cell width `cell_w` (R5.2) — the replacement for the old
/// fixed `pen += cell_w`, which overlapped every wide glyph.
pub(crate) fn advance_px(c: char, cell_w: f32) -> f32 {
    char_cols(c) as f32 * cell_w
}

/// `(char, pen_x)` for each char of `s`, starting at `start_x` and advancing by
/// [`advance_px`] (R5.2). Pure and allocation-free so the advance rule is unit testable
/// without a GPU.
pub(crate) fn pen_layout(
    s: &str,
    start_x: f32,
    cell_w: f32,
) -> impl Iterator<Item = (char, f32)> + '_ {
    let mut pen = start_x;
    s.chars().map(move |c| {
        let at = pen;
        pen += advance_px(c, cell_w);
        (c, at)
    })
}

/// The prefix of `s` that fits in a **column** budget of `max_cols` (R5.3).
///
/// Counts display width, not chars: `truncate_cols("한글abc", 5) == "한글a"` (2+2+1). A
/// char is taken only when it fits whole, so a wide char is never half-drawn past the
/// budget and a codepoint is never split.
pub(crate) fn truncate_cols(s: &str, max_cols: usize) -> String {
    let mut used = 0usize;
    let mut out = String::new();
    for c in s.chars() {
        let w = char_cols(c);
        if used + w > max_cols {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// Lay `s` out left-to-right from `start_x`, calling `emit(pen_x, entry)` once per
/// drawable char (R5.1/R5.4).
///
/// Glyph lookup goes through [`Atlas::resolve`](crate::atlas::Atlas::resolve) +
/// `get_or_insert_styled`, so layers pick up the font-fallback chain (Hangul resolves to
/// the fallback face instead of the primary's `.notdef`). A char no face covers emits
/// nothing **but still advances the pen**, keeping the rest of the line in its columns.
pub(crate) fn layout_line(
    ctx: &mut FrameCtx<'_>,
    s: &str,
    start_x: f32,
    cell_w: f32,
    mut emit: impl FnMut(f32, GlyphEntry),
) {
    for (c, pen) in pen_layout(s, start_x, cell_w) {
        if let Some(key) = ctx.atlas.resolve(c) {
            if let Some(e) = ctx.atlas.get_or_insert_styled(ctx.queue, key) {
                emit(pen, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_cols_spends_a_column_budget_not_a_char_count() {
        // R5.3: the spec's exact case — 2 + 2 + 1 = 5 columns.
        assert_eq!(truncate_cols("한글abc", 5), "한글a");
        // ASCII: one column each, so it still behaves like the old char count.
        assert_eq!(truncate_cols("hello", 3), "hel");
        assert_eq!(truncate_cols("hi", 9), "hi");
        // Never splits a codepoint (multi-byte but 1 column).
        assert_eq!(truncate_cols("café", 4), "café");
        // A budget that would split a wide char drops it whole rather than letting it
        // bleed one column past the sidebar.
        assert_eq!(truncate_cols("한글", 3), "한");
        assert_eq!(truncate_cols("한", 1), "");
        // Edges: empty input, zero budget.
        assert_eq!(truncate_cols("", 5), "");
        assert_eq!(truncate_cols("한글abc", 0), "");
    }

    #[test]
    fn pen_advances_by_display_width() {
        // R5.2: `프` is 2 columns, `a` is 1 — the pen positions are 0 then 2*cw, not the
        // old fixed 1*cw stride that overlapped the two glyphs.
        let cw = 8.0;
        let xs: Vec<f32> = pen_layout("프a", 0.0, cw).map(|(_, x)| x).collect();
        assert_eq!(xs, vec![0.0, 2.0 * cw]);
        // The advance past the whole string is 3 columns.
        let end = pen_layout("프a", 0.0, cw)
            .map(|(c, _)| advance_px(c, cw))
            .sum::<f32>();
        assert_eq!(end, 3.0 * cw);
        // A start offset shifts every pen.
        let pad = 8.0;
        let xs: Vec<f32> = pen_layout("프a", pad, cw).map(|(_, x)| x).collect();
        assert_eq!(xs, vec![pad, pad + 2.0 * cw]);
    }

    #[test]
    fn char_cols_matches_the_grid_width_rule() {
        assert_eq!(char_cols('a'), 1);
        assert_eq!(char_cols('프'), 2);
        // Control chars have no width; lay them out as one column, not zero.
        assert_eq!(char_cols('\u{7}'), 1);
    }
}
