//! The pinned sidebar header band, as a [`crate::DrawLayer`] that **nobody registers**
//! (D14 — `sidebar-interaction` calls `add_layer` from `terminal_app.rs`). It draws no
//! text (D12): the opaque `panel_bg` band (D5) and the inert filter icon via [`push_icon`].
//!
//! The new-tab (`+`), hide, and collapsed-restore controls it used to host all moved to
//! the top title bar (`titlebar-tab-controls` R5), so this layer no longer hit-tests a
//! hover cursor — it has no live target left. The band's top and the filter icon are
//! shifted down by `top_inset` so they sit below the title bar (D6).
//!
//! Shape building is a device-free free function ([`build_shapes`]) so every R2/R3
//! assertion runs without a GPU; `record` is a thin wrapper that assembles the input,
//! calls it, draws, and publishes `list_top` to the shared model (D13).

use crate::sidebar_icons::{push_icon, Icon};
use crate::sidebar_layout::{filter_icon_rect, header_h, tokens, SidebarLayoutInput};
use crate::sidebar_row::TabModelHandle;
use crate::sidebar_shapes::{Fill, SidebarShape, SidebarShapePipeline};
use crate::sidebar_theme::{linear, SidebarTheme};
use crate::{DrawLayer, FrameCtx};

/// z-order: above the cards (`TAB_BAR_Z = 10`, so the opaque band clips scrolled cards)
/// and inline images (`IMAGE_Z = 20`), below the ai-monitor overlay (`OVERLAY_Z = 100`)
/// (D4).
pub const SIDEBAR_CHROME_Z: i32 = 30;

// R3.7 / D4 — a compile-time invariant, not a comment: the chrome must sit above the
// cards and inline images and below the ai-monitor overlay. A card layer registered
// above 30 (or this constant moved) would silently invert D5's clip-by-overdraw.
const _: () = assert!(SIDEBAR_CHROME_Z > crate::sidebar_row::TAB_BAR_Z);
const _: () = assert!(SIDEBAR_CHROME_Z > crate::image_layer::IMAGE_Z);
const _: () = assert!(SIDEBAR_CHROME_Z < crate::overlay_layer::OVERLAY_Z);

/// The header draw layer (D14). Reuses one `Vec<SidebarShape>` so a steady frame
/// allocates nothing (D12/SD4).
pub struct SidebarChromeLayer {
    pipe: Option<SidebarShapePipeline>,
    shapes: Vec<SidebarShape>,
}

impl SidebarChromeLayer {
    /// Create the layer. No handle: the header has no interactive target since its
    /// controls moved to the title bar (R5), so there is no hover cursor to feed it.
    pub fn new() -> SidebarChromeLayer {
        SidebarChromeLayer {
            pipe: None,
            shapes: Vec::new(),
        }
    }
}

impl Default for SidebarChromeLayer {
    fn default() -> Self {
        SidebarChromeLayer::new()
    }
}

/// Emit the chrome shapes for `inp` — device-free (D12), so R2/R3 test it directly. When
/// `collapsed`, nothing (the header reserves no width, and restore now lives in the title
/// bar, R5); otherwise the opaque band + the inert filter icon, both shifted down by
/// `top_inset` (D6). The band's alpha is 1.0 by contract, not taste — it is the clip
/// mechanism (D5).
pub fn build_shapes(inp: &SidebarLayoutInput, collapsed: bool, out: &mut Vec<SidebarShape>) {
    if inp.viewport[1] <= 0.0 || !inp.viewport[1].is_finite() || !inp.scale.is_finite() {
        return; // R8.2 / R8.4 — degenerate viewport or scale emits nothing
    }
    // Collapsed reserves no header band; the restore control is retired (R5). Draw nothing.
    if collapsed {
        return;
    }
    if inp.sidebar_w <= 0.0 || !inp.sidebar_w.is_finite() {
        return; // R8.4 — expanded but no band
    }

    let theme = SidebarTheme::DEFAULT;
    let scale = inp.scale as f32;
    let icon_sz = tokens::ICON_PX * scale;

    // The opaque header band — OPAQUE by contract, it is what clips a scrolled card (D5).
    // Its top begins at `top_inset` so it sits below the title bar (D6), matching
    // `header_rect` in the layout module (draw == hit-test, SD3).
    if let Some(band) = SidebarShape::rounded_box(
        [0.0, inp.top_inset, inp.sidebar_w, header_h(inp.scale)],
        0.0,
        0.0,
        Fill::solid(linear(theme.panel_bg, 1.0)),
        [0.0; 4],
    ) {
        out.push(band);
    }

    // The inert filter icon — no hover, no region (D2). Its rect already carries the
    // `top_inset` shift (see `filter_icon_rect`), so it lands inside the band.
    let f = filter_icon_rect(inp);
    push_icon(
        out,
        Icon::Filter,
        f.x + (f.w - icon_sz) * 0.5,
        f.y + (f.h - icon_sz) * 0.5,
        icon_sz,
        theme.icon_fg,
    );
}

impl DrawLayer for SidebarChromeLayer {
    fn z_order(&self) -> i32 {
        SIDEBAR_CHROME_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        if ctx.viewport[1] <= 0.0 {
            return;
        }
        // D3: collapsedness is read from the inset (the number grid_dims already uses) —
        // the OPPOSITE guard to the row layer, which stops drawing here as this one starts.
        let collapsed = ctx.left_inset <= 0.0;

        // Row count + waiting count feed the input; the header geometry does not use row_h,
        // so a missing published card layout (collapsed session) is harmless.
        let (row_h, row_count, waiting_count) = TabModelHandle::shared().with(|m| {
            (
                m.layout.as_ref().map(|l| l.card_h).unwrap_or(0.0),
                m.rows.len(),
                m.waiting.len(),
            )
        });
        let inp = SidebarLayoutInput::from_frame(ctx, row_h, row_count, waiting_count);

        let mut shapes = std::mem::take(&mut self.shapes);
        shapes.clear();
        build_shapes(&inp, collapsed, &mut shapes);
        {
            let pipe = SidebarShapePipeline::ensure(&mut self.pipe, ctx);
            pipe.draw(ctx, &shapes);
        }
        self.shapes = shapes;

        // D13/D9 + titlebar D6: publish where the card list begins so the row layer offsets
        // its cards below the title bar, the header band, AND the pinned waiting band. This
        // MUST equal what the app publishes via `set_list_scroll` and what
        // `sidebar_layout::list_viewport` uses — `top_inset + header_h + waiting_band_h` —
        // so the two every-frame writers can't clobber each other and rows never desync.
        // (Omitting `top_inset` here once hid the first row under the header band; omitting
        // `waiting_band_h` would overlap the cards with the waiting band.) Collapsed
        // reserves nothing, so 0.0.
        let list_top = if collapsed {
            0.0
        } else {
            ctx.top_inset
                + header_h(ctx.scale_factor)
                + crate::sidebar_layout::waiting_band_h(waiting_count, ctx.scale_factor)
        };
        TabModelHandle::shared().with_mut(|m| m.list_top = list_top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_theme::SidebarMetrics;

    fn input(sidebar_w: f32, scale: f64) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w,
            viewport: [sidebar_w.max(200.0), 900.0],
            scale,
            row_h: 60.0 * scale as f32,
            row_count: 5,
            waiting_count: 0,
            metrics: SidebarMetrics::DEFAULT.scaled(scale),
            top_inset: 0.0,
        }
    }

    // R3.7's z-order invariant is a compile-time `const _: () = assert!(…)` above; if it
    // is ever violated the crate fails to build, which is stronger than a test.

    #[test]
    fn band_is_opaque_panel_bg() {
        // R2.1 — the first shape is the full-width band, fully opaque, in panel_bg.
        let mut out = Vec::new();
        build_shapes(&input(340.0, 1.0), false, &mut out);
        assert!(!out.is_empty());
        let band = out[0];
        assert_eq!(band.bbox, [0.0, 0.0, 340.0, header_h(1.0)]);
        assert_eq!(band.fill0, linear(SidebarTheme::DEFAULT.panel_bg, 1.0));
        assert_eq!(band.fill0[3], 1.0, "the clip band must be opaque (D5)");
    }

    #[test]
    fn band_and_filter_shift_with_top_inset() {
        // D6 / R5 — with a title bar reserved, the band's top and the filter icon both
        // move down by top_inset (nothing is drawn up in the title-bar strip).
        let mut inp = input(340.0, 1.0);
        inp.top_inset = 40.0;
        let mut out = Vec::new();
        build_shapes(&inp, false, &mut out);
        // The band starts at top_inset.
        assert_eq!(out[0].bbox[1], 40.0, "band top at top_inset");
        // Every drawn shape sits at or below top_inset (nothing above the band).
        assert!(
            out.iter().all(|s| s.bbox[1] >= 40.0 - 0.01),
            "no shape drawn above the title bar"
        );
    }

    #[test]
    fn expanded_emits_band_and_filter_no_restore() {
        // R2.2 / R5 — expanded draws the band + the inert filter, and no floating restore
        // button (there is no restore affordance any more).
        let mut out = Vec::new();
        build_shapes(&input(340.0, 1.0), false, &mut out);
        assert!(out.len() >= 2, "band + at least the filter icon's strokes");
        // The old restore button lived up at (14,12) logical; nothing draws there now.
        assert!(
            !out.iter().any(|s| s.bbox[0] == 14.0 && s.bbox[1] == 12.0),
            "no restore box"
        );
    }

    #[test]
    fn collapsed_emits_nothing() {
        // R5 — collapsed, the sidebar draws no chrome at all (restore moved to the bar).
        let mut out = Vec::new();
        build_shapes(&input(0.0, 1.0), true, &mut out);
        assert!(out.is_empty(), "collapsed draws nothing");
    }

    #[test]
    fn degenerate_inputs_emit_nothing() {
        // R8.2 / R8.4
        let mut out = Vec::new();
        build_shapes(&input(340.0, f64::NAN), false, &mut out);
        assert!(out.is_empty(), "non-finite scale emits nothing");
        let mut zero = input(340.0, 1.0);
        zero.viewport[1] = 0.0;
        let mut out2 = Vec::new();
        build_shapes(&zero, false, &mut out2);
        assert!(out2.is_empty(), "zero viewport emits nothing");
    }
}
