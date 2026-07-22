//! The pinned sidebar header + collapsed restore button, as a [`crate::DrawLayer`] that
//! **nobody registers** (D14 — `sidebar-interaction` calls `add_layer` from
//! `terminal_app.rs`). It draws no text (D12): the opaque `panel_bg` band (D5), the three
//! header icons via [`push_icon`], the hover highlight resolved from a raw cursor through
//! [`chrome_hit`], and — while collapsed — the floating restore button (D3/D4).
//!
//! Shape building is a device-free free function ([`build_shapes`]) so every R2/R3
//! assertion runs without a GPU; `record` is a thin wrapper that assembles the input,
//! calls it, draws, and publishes `list_top` to the shared model (D13).

use std::cell::RefCell;
use std::rc::Rc;

use crate::sidebar_icons::{push_icon, Icon};
use crate::sidebar_layout::{
    button_rect, chrome_hit, filter_icon_rect, header_h, restore_button_rect, tokens, ChromeRegion,
    SidebarLayoutInput,
};
use crate::sidebar_row::TabModelHandle;
use crate::sidebar_shapes::{Fill, SidebarShape, SidebarShapePipeline};
use crate::sidebar_theme::{linear, SidebarTheme};
use crate::{DrawLayer, FrameCtx};

/// z-order: above the cards (`TAB_BAR_Z = 10`, so the opaque band clips scrolled cards)
/// and inline images (`IMAGE_Z = 20`, so the restore button is never buried), below the
/// ai-monitor overlay (`OVERLAY_Z = 100`) (D4).
pub const SIDEBAR_CHROME_Z: i32 = 30;

// R3.7 / D4 — a compile-time invariant, not a comment: the chrome must sit above the
// cards and inline images and below the ai-monitor overlay. A card layer registered
// above 30 (or this constant moved) would silently invert D5's clip-by-overdraw.
const _: () = assert!(SIDEBAR_CHROME_Z > crate::sidebar_row::TAB_BAR_Z);
const _: () = assert!(SIDEBAR_CHROME_Z > crate::image_layer::IMAGE_Z);
const _: () = assert!(SIDEBAR_CHROME_Z < crate::overlay_layer::OVERLAY_Z);

/// The one field the app writes (D10/D11): the raw cursor in physical px, `None` when the
/// pointer leaves the window. Resolved to a hover region by the layer, never the app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SidebarChromeModel {
    pub cursor: Option<[f32; 2]>,
}

/// The third instance of the established handle idiom (D11) — `Rc<RefCell<_>>`, single
/// UI thread, never `Send`.
#[derive(Clone, Default)]
pub struct SidebarChromeHandle(Rc<RefCell<SidebarChromeModel>>);

impl SidebarChromeHandle {
    /// Replace the model (the app pushes the cursor each pointer move).
    pub fn set(&self, model: SidebarChromeModel) {
        *self.0.borrow_mut() = model;
    }

    /// Convenience: push just the raw cursor.
    pub fn set_cursor(&self, cursor: Option<[f32; 2]>) {
        self.0.borrow_mut().cursor = cursor;
    }

    fn cursor(&self) -> Option<[f32; 2]> {
        self.0.borrow().cursor
    }
}

/// The header/restore draw layer (D14). Reuses one `Vec<SidebarShape>` so a steady frame
/// allocates nothing (D12/SD4).
pub struct SidebarChromeLayer {
    handle: SidebarChromeHandle,
    pipe: Option<SidebarShapePipeline>,
    shapes: Vec<SidebarShape>,
}

impl SidebarChromeLayer {
    /// Create the layer and its handle (D14 — the app cannot write a cursor into a handle
    /// it was never given). Matches `TabBar::new()`'s tuple shape.
    pub fn new() -> (SidebarChromeLayer, SidebarChromeHandle) {
        let handle = SidebarChromeHandle::default();
        (
            SidebarChromeLayer {
                handle: handle.clone(),
                pipe: None,
                shapes: Vec::new(),
            },
            handle,
        )
    }
}

/// Emit the chrome shapes for `inp` — device-free (D12), so R2/R3 test it directly. When
/// `collapsed`, only the restore button; otherwise the opaque band + three icons, the two
/// live ones brightened with a hover background when `hover` names them (D10). The band's
/// alpha is 1.0 by contract, not taste — it is the clip mechanism (D5).
pub fn build_shapes(
    inp: &SidebarLayoutInput,
    collapsed: bool,
    hover: Option<ChromeRegion>,
    out: &mut Vec<SidebarShape>,
) {
    if inp.viewport[1] <= 0.0 || !inp.viewport[1].is_finite() || !inp.scale.is_finite() {
        return; // R8.2 / R8.4 — degenerate viewport or scale emits nothing
    }
    let theme = SidebarTheme::DEFAULT;
    let scale = inp.scale as f32;
    let icon_sz = tokens::ICON_PX * scale;
    let border_w = inp.metrics.border_w;

    // Collapsed: only the floating restore button, over the terminal (D3/D4).
    if collapsed {
        let r = restore_button_rect(inp.scale);
        if let Some(s) = SidebarShape::rounded_box(
            [r.x, r.y, r.w, r.h],
            tokens::RESTORE_RADIUS * scale,
            border_w,
            Fill::solid(linear(tokens::RESTORE_BG, 1.0)),
            linear(tokens::RESTORE_BORDER, 1.0),
        ) {
            out.push(s);
        }
        push_icon(
            out,
            Icon::Panel,
            r.x + (r.w - icon_sz) * 0.5,
            r.y + (r.h - icon_sz) * 0.5,
            icon_sz,
            theme.icon_fg,
        );
        return;
    }

    if inp.sidebar_w <= 0.0 || !inp.sidebar_w.is_finite() {
        return; // R8.4 — expanded but no band
    }

    // The opaque header band — OPAQUE by contract, it is what clips a scrolled card (D5).
    if let Some(band) = SidebarShape::rounded_box(
        [0.0, 0.0, inp.sidebar_w, header_h(inp.scale)],
        0.0,
        0.0,
        Fill::solid(linear(theme.panel_bg, 1.0)),
        [0.0; 4],
    ) {
        out.push(band);
    }

    // The inert filter icon — no hover, no region (D2).
    let f = filter_icon_rect(inp);
    push_icon(
        out,
        Icon::Filter,
        f.x + (f.w - icon_sz) * 0.5,
        f.y + (f.h - icon_sz) * 0.5,
        icon_sz,
        theme.icon_fg,
    );

    // The two live buttons — new-tab (plus) and hide (panel), hover-aware (D10).
    for (region, icon) in [
        (ChromeRegion::NewTab, Icon::Plus),
        (ChromeRegion::HideSidebar, Icon::Panel),
    ] {
        let b = button_rect(region, inp);
        let color = if hover == Some(region) {
            if let Some(bg) = SidebarShape::rounded_box(
                [b.x, b.y, b.w, b.h],
                inp.metrics.button_radius,
                0.0,
                Fill::solid(linear(theme.icon_bg_hover, 1.0)),
                [0.0; 4],
            ) {
                out.push(bg);
            }
            theme.icon_fg_hover
        } else {
            theme.icon_fg
        };
        push_icon(
            out,
            icon,
            b.x + (b.w - icon_sz) * 0.5,
            b.y + (b.h - icon_sz) * 0.5,
            icon_sz,
            color,
        );
    }
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

        // Row count feeds the input; the header/restore geometry does not use row_h, so a
        // missing published card layout (collapsed session) is harmless.
        let (row_h, row_count, waiting_count) = TabModelHandle::shared().with(|m| {
            (
                m.layout.as_ref().map(|l| l.card_h).unwrap_or(0.0),
                m.rows.len(),
                m.waiting.len(),
            )
        });
        let inp = SidebarLayoutInput::from_frame(ctx, row_h, row_count, waiting_count);
        let hover = self
            .handle
            .cursor()
            .and_then(|p| chrome_hit(p, &inp, collapsed));

        let mut shapes = std::mem::take(&mut self.shapes);
        shapes.clear();
        build_shapes(&inp, collapsed, hover, &mut shapes);
        {
            let pipe = SidebarShapePipeline::ensure(&mut self.pipe, ctx);
            pipe.draw(ctx, &shapes);
        }
        self.shapes = shapes;

        // D13/D9: publish the header height PLUS the pinned waiting band's height so the
        // row layer offsets its cards below both. One write per frame; collapsed reserves
        // no header/band, so 0.0. This value is kept byte-identical to what the app writes
        // via `set_list_scroll` (both add `waiting_band_h(waiting_count)`), so the two
        // writers cannot clobber each other.
        let list_top = if collapsed {
            0.0
        } else {
            header_h(ctx.scale_factor)
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
        }
    }

    // R3.7's z-order invariant is a compile-time `const _: () = assert!(…)` above; if it
    // is ever violated the crate fails to build, which is stronger than a test.

    #[test]
    fn band_is_opaque_panel_bg() {
        // R2.1 — the first shape is the full-width band, fully opaque, in panel_bg.
        let mut out = Vec::new();
        build_shapes(&input(340.0, 1.0), false, None, &mut out);
        assert!(!out.is_empty());
        let band = out[0];
        assert_eq!(band.bbox, [0.0, 0.0, 340.0, header_h(1.0)]);
        assert_eq!(band.fill0, linear(SidebarTheme::DEFAULT.panel_bg, 1.0));
        assert_eq!(band.fill0[3], 1.0, "the clip band must be opaque (D5)");
    }

    #[test]
    fn expanded_emits_band_and_icons_no_restore() {
        // R2.2 / R3.2 — expanded draws the band + icon strokes and no restore button.
        let mut out = Vec::new();
        build_shapes(&input(340.0, 1.0), false, None, &mut out);
        // The band plus multiple icon-stroke capsules for the three icons.
        assert!(out.len() > 3, "band + three icons' strokes");
        // No shape sits at the collapsed restore button's origin.
        let r = restore_button_rect(1.0);
        assert!(
            !out.iter()
                .any(|s| s.bbox[0] == r.x && s.bbox[1] == r.y && s.kind == 0),
            "no restore box while expanded"
        );
    }

    #[test]
    fn hover_adds_a_button_background() {
        // R2.3 — hovering a live button emits one extra shape (its hover bg).
        let inp = input(340.0, 1.0);
        let mut plain = Vec::new();
        build_shapes(&inp, false, None, &mut plain);
        let mut hovered = Vec::new();
        build_shapes(&inp, false, Some(ChromeRegion::NewTab), &mut hovered);
        assert_eq!(
            hovered.len(),
            plain.len() + 1,
            "hover adds exactly the button background"
        );
    }

    #[test]
    fn collapsed_emits_only_the_restore_button() {
        // R3.1 — collapsed draws the restore box + its icon, and no full-width band.
        let mut out = Vec::new();
        build_shapes(&input(0.0, 1.0), true, None, &mut out);
        assert!(!out.is_empty());
        let r = restore_button_rect(1.0);
        assert_eq!(out[0].bbox, [r.x, r.y, r.w, r.h], "the restore box");
        // No opaque full-width band (its bbox would start at 0,0 with a large width).
        assert!(
            !out.iter().any(|s| s.bbox == [0.0, 0.0, 0.0, header_h(1.0)]),
            "no header band while collapsed"
        );
    }

    #[test]
    fn degenerate_inputs_emit_nothing() {
        // R8.2 / R8.4
        let mut out = Vec::new();
        build_shapes(&input(340.0, f64::NAN), false, None, &mut out);
        assert!(out.is_empty(), "non-finite scale emits nothing");
        let mut zero = input(340.0, 1.0);
        zero.viewport[1] = 0.0;
        let mut out2 = Vec::new();
        build_shapes(&zero, false, None, &mut out2);
        assert!(out2.is_empty(), "zero viewport emits nothing");
    }

    #[test]
    fn handle_is_reachable_at_construction() {
        // R7.8 — new() returns the handle; a cursor pushed to it is visible.
        let (_layer, handle) = SidebarChromeLayer::new();
        handle.set_cursor(Some([10.0, 20.0]));
        assert_eq!(handle.cursor(), Some([10.0, 20.0]));
    }
}
