//! The top title-bar tab controls, as a [`crate::DrawLayer`] that **nobody registers**
//! here (the `titlebar-tab-controls` feature calls `add_layer` from `terminal_app.rs`).
//! It draws no text: the opaque `panel_bg` band across the top inset, plus three square
//! buttons — a sidebar toggle just right of the native traffic-light gutter (D15) and
//! new-tab / close-tab on the right — built from the shared [`SidebarShape`] primitive and
//! the existing [`Icon`] set. The band merges into the native title bar (D12), so the
//! traffic lights float over its left end and the gutter keeps our controls clear of them.
//!
//! Shape building is a device-free free function ([`build_titlebar_shapes`], mirroring
//! `sidebar_header::build_shapes`) so every layout/colour assertion runs without a GPU;
//! [`TitlebarLayer::record`] is a thin wrapper that reads the collapsed flag from the
//! shared handle, builds the shapes, and draws them through one instanced pipeline.
//!
//! Geometry is shared by the layout helpers ([`titlebar_button_rect`] / [`titlebar_hit`])
//! so the drawn buttons and the app's hit-testing can never diverge — the same idiom the
//! sidebar chrome uses. There is no `×` icon in the icon set, so the close glyph is drawn
//! here as two diagonal capsule strokes.

use std::cell::RefCell;
use std::rc::Rc;

use crate::sidebar_icons::{push_icon, Icon};
use crate::sidebar_layout::{tokens, Rect};
use crate::sidebar_shapes::{Fill, SidebarShape, SidebarShapePipeline};
use crate::sidebar_theme::{linear, SidebarMetrics, SidebarTheme};
use crate::{DrawLayer, FrameCtx};

/// z-order: above the sidebar chrome (`SIDEBAR_CHROME_Z = 30`, so the opaque top band
/// paints over a card that scrolled up under it) and below the ai-monitor overlay
/// (`OVERLAY_Z = 100`, so a monitor panel is never buried by the title bar).
pub const TITLEBAR_Z: i32 = 40;

// Pin the band between the sidebar chrome and the overlay at compile time — a moved
// constant that inverted this ordering would silently change the composite, so make it a
// build error rather than a comment.
const _: () = assert!(TITLEBAR_Z > crate::sidebar_header::SIDEBAR_CHROME_Z);
const _: () = assert!(TITLEBAR_Z < crate::overlay_layer::OVERLAY_Z);

/// Padding around a button inside the band, logical px (× `scale` at the point of use).
const TITLEBAR_PAD: f32 = 8.0;
/// Gap between the two right-aligned buttons, logical px (× `scale` at use).
const TITLEBAR_GAP: f32 = 8.0;
/// Width reserved at the band's LEFT edge for the native macOS traffic lights
/// (close/minimise/zoom), logical px (× `scale` at use). The content band now reaches the
/// true window top (titlebar-tab-controls D12), so the traffic lights float over its left
/// end; `ToggleSidebar` starts just right of this gutter so no drawn control is occluded
/// (D15). ~78 covers the three ~14px buttons plus their leading/inter-button spacing.
const TRAFFIC_LIGHT_GUTTER: f32 = 78.0;
/// The close-glyph stroke width, logical px — the same 1.7 the icon set draws with.
const CLOSE_STROKE_W: f32 = 1.7;
/// Fractional inset of the `×` strokes inside their button rect (~30% each side).
const CLOSE_INSET: f32 = 0.3;

/// The band's physical-px height — the value the app reserves as `top_inset`. `40` logical
/// (`ICON_PX + 2·12`) × `scale`, clamped `>= 0` so a non-finite/negative scale can never
/// hand back a negative inset.
pub fn titlebar_height(scale: f64) -> f32 {
    let logical = tokens::ICON_PX + 2.0 * 12.0; // 16 + 24 = 40 logical
    (logical * scale as f32).max(0.0)
}

/// A live title-bar target. The sidebar toggle sits on the left; new-tab and close-tab are
/// right-aligned (close at the corner, new-tab one gap left).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitlebarButton {
    NewTab,
    CloseTab,
    ToggleSidebar,
}

/// The three buttons in a fixed order — iterated by both the hit-test and the draw so the
/// two stay in lockstep.
const BUTTONS: [TitlebarButton; 3] = [
    TitlebarButton::ToggleSidebar,
    TitlebarButton::NewTab,
    TitlebarButton::CloseTab,
];

/// A button's box, physical px (top-left origin). Every button is a square of side
/// `top_inset - 2·pad`, vertically centred in the band. `ToggleSidebar` is left-aligned
/// just right of the traffic-light gutter (`gutter + pad`, D15); `CloseTab`'s right edge
/// sits at `viewport_x - pad`, with `NewTab` one gap left.
pub fn titlebar_button_rect(
    kind: TitlebarButton,
    viewport: [f32; 2],
    top_inset: f32,
    scale: f64,
) -> Rect {
    let s = scale as f32;
    let pad = TITLEBAR_PAD * s;
    let gap = TITLEBAR_GAP * s;
    let gutter = TRAFFIC_LIGHT_GUTTER * s;
    let side = (top_inset - 2.0 * pad).max(0.0);
    let right = viewport[0] - pad; // the close button's right edge
    let x = match kind {
        TitlebarButton::ToggleSidebar => gutter + pad,
        TitlebarButton::CloseTab => right - side,
        TitlebarButton::NewTab => right - 2.0 * side - gap,
    };
    Rect {
        x,
        y: pad,
        w: side,
        h: side,
    }
}

/// The button under `p`, or `None`. `None` for any point below the band (`p.y >=
/// top_inset`) — the title bar owns only its own strip — and for a point inside the band
/// on no button. Containment is half-open via [`Rect::contains`], so adjacent edges never
/// double-claim.
pub fn titlebar_hit(
    p: [f32; 2],
    viewport: [f32; 2],
    top_inset: f32,
    scale: f64,
) -> Option<TitlebarButton> {
    if p[1] >= top_inset {
        return None;
    }
    BUTTONS
        .into_iter()
        .find(|&kind| titlebar_button_rect(kind, viewport, top_inset, scale).contains(p))
}

/// Draw the close `×` as two diagonal capsule strokes across `b`, each inset by
/// [`CLOSE_INSET`] on every side (the icon set has no `×` glyph, so it is geometry here).
fn push_close_cross(out: &mut Vec<SidebarShape>, b: Rect, scale: f32, color: u32) {
    let left = b.x + b.w * CLOSE_INSET;
    let right = b.x + b.w * (1.0 - CLOSE_INSET);
    let top = b.y + b.h * CLOSE_INSET;
    let bottom = b.y + b.h * (1.0 - CLOSE_INSET);
    let sw = CLOSE_STROKE_W * scale;
    let fill = Fill::solid(linear(color, 1.0));
    for seg in [(left, top, right, bottom), (right, top, left, bottom)] {
        if let Some(s) = SidebarShape::capsule(seg.0, seg.1, seg.2, seg.3, sw, fill) {
            out.push(s);
        }
    }
}

/// Emit the title-bar shapes — device-free, so the layout/colour tests drive it directly.
/// The opaque band is pushed FIRST (it is what clips a scrolled card), then, when `hover`
/// names a button, its rounded hover background, then the three icons. **R7.3** — the
/// toggle glyph reflects state: `icon_fg_hover` while `collapsed`, `icon_fg` while
/// expanded; that colour is the observable state difference.
pub fn build_titlebar_shapes(
    viewport: [f32; 2],
    top_inset: f32,
    scale: f64,
    collapsed: bool,
    hover: Option<TitlebarButton>,
    out: &mut Vec<SidebarShape>,
) {
    // Degenerate viewport / band / scale emits nothing — never a NaN'd or inverted quad.
    if !(viewport[0].is_finite() && viewport[1].is_finite())
        || viewport[0] <= 0.0
        || !top_inset.is_finite()
        || top_inset <= 0.0
        || !scale.is_finite()
    {
        return;
    }
    let theme = SidebarTheme::DEFAULT;
    let s = scale as f32;
    let icon = tokens::ICON_PX * s;

    // The opaque band — OPAQUE by contract (alpha 1.0): it clips anything drawn under it.
    if let Some(band) = SidebarShape::rounded_box(
        [0.0, 0.0, viewport[0], top_inset],
        0.0,
        0.0,
        Fill::solid(linear(theme.panel_bg, 1.0)),
        [0.0; 4],
    ) {
        out.push(band);
    }

    // The hover highlight sits behind its button's icon (a simple rounded fill).
    if let Some(h) = hover {
        let b = titlebar_button_rect(h, viewport, top_inset, scale);
        if let Some(bg) = SidebarShape::rounded_box(
            [b.x, b.y, b.w, b.h],
            SidebarMetrics::DEFAULT.button_radius * s,
            0.0,
            Fill::solid(linear(theme.icon_bg_hover, 1.0)),
            [0.0; 4],
        ) {
            out.push(bg);
        }
    }

    // ToggleSidebar (left) — the panel glyph, brightened while collapsed (R7.3).
    let toggle = titlebar_button_rect(TitlebarButton::ToggleSidebar, viewport, top_inset, scale);
    let toggle_color = if collapsed {
        theme.icon_fg_hover
    } else {
        theme.icon_fg
    };
    push_icon(
        out,
        Icon::Panel,
        toggle.x + (toggle.w - icon) * 0.5,
        toggle.y + (toggle.h - icon) * 0.5,
        icon,
        toggle_color,
    );

    // NewTab (right) — the plus glyph.
    let newtab = titlebar_button_rect(TitlebarButton::NewTab, viewport, top_inset, scale);
    push_icon(
        out,
        Icon::Plus,
        newtab.x + (newtab.w - icon) * 0.5,
        newtab.y + (newtab.h - icon) * 0.5,
        icon,
        theme.icon_fg,
    );

    // CloseTab (right) — the `×`, drawn as two diagonal strokes across the button.
    let close = titlebar_button_rect(TitlebarButton::CloseTab, viewport, top_inset, scale);
    push_close_cross(out, close, s, theme.icon_fg);
}

/// The shared collapsed-state handle (the established `Rc<RefCell<_>>` idiom, single UI
/// thread, never `Send`). The app flips it on a toggle click; the layer reads it each
/// frame to pick the toggle glyph's colour (R7.3).
#[derive(Clone, Default)]
pub struct TitlebarHandle(Rc<RefCell<bool>>);

impl TitlebarHandle {
    /// Set whether the sidebar is currently collapsed (the app writes this on a toggle).
    pub fn set_collapsed(&self, collapsed: bool) {
        *self.0.borrow_mut() = collapsed;
    }

    /// Read the collapsed flag — internal to the layer's `record`.
    fn collapsed(&self) -> bool {
        *self.0.borrow()
    }
}

/// The title-bar draw layer. Reuses one `Vec<SidebarShape>` so a steady frame allocates
/// nothing once the buffer has grown.
pub struct TitlebarLayer {
    handle: TitlebarHandle,
    pipe: Option<SidebarShapePipeline>,
    shapes: Vec<SidebarShape>,
}

impl TitlebarLayer {
    /// Create the layer and its handle (the app cannot write state into a handle it was
    /// never given). Matches the `SidebarChromeLayer::new()` tuple shape.
    pub fn new() -> (TitlebarLayer, TitlebarHandle) {
        let handle = TitlebarHandle::default();
        (
            TitlebarLayer {
                handle: handle.clone(),
                pipe: None,
                shapes: Vec::new(),
            },
            handle,
        )
    }
}

impl DrawLayer for TitlebarLayer {
    fn z_order(&self) -> i32 {
        TITLEBAR_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // No band reserved (or a degenerate viewport) → nothing to draw, no GPU work.
        if ctx.top_inset <= 0.0 || ctx.viewport[0] <= 0.0 {
            return;
        }
        let collapsed = self.handle.collapsed();
        // Hover is not modelled by this layer yet; the parameter is kept on the builder so
        // wiring a cursor later needs no signature change.
        let hover = None;

        let mut shapes = std::mem::take(&mut self.shapes);
        shapes.clear();
        build_titlebar_shapes(
            ctx.viewport,
            ctx.top_inset,
            ctx.scale_factor,
            collapsed,
            hover,
            &mut shapes,
        );
        {
            let pipe = SidebarShapePipeline::ensure(&mut self.pipe, ctx);
            pipe.draw(ctx, &shapes);
        }
        self.shapes = shapes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: [f32; 2] = [800.0, 600.0];

    // The z-order invariant is a compile-time `const _: () = assert!(…)` above; violating
    // it fails the build, which is stronger than a test.

    #[test]
    fn hit_returns_each_button_and_none_off_target() {
        let top = titlebar_height(1.0); // 40
        for kind in BUTTONS {
            let r = titlebar_button_rect(kind, VP, top, 1.0);
            let c = [r.x + r.w * 0.5, r.y + r.h * 0.5];
            assert_eq!(titlebar_hit(c, VP, top, 1.0), Some(kind), "{kind:?} centre");
        }
        // An empty point mid-band, between the left toggle and the right pair.
        assert_eq!(
            titlebar_hit([VP[0] * 0.5, top * 0.5], VP, top, 1.0),
            None,
            "empty band point"
        );
        // Below the band (over the toggle's column) is never a hit — the bar owns its strip.
        let toggle = titlebar_button_rect(TitlebarButton::ToggleSidebar, VP, top, 1.0);
        assert_eq!(
            titlebar_hit([toggle.x + toggle.w * 0.5, top + 5.0], VP, top, 1.0),
            None,
            "p.y >= top_inset yields None"
        );
    }

    #[test]
    fn buttons_are_right_aligned_and_gapped() {
        let top = titlebar_height(1.0);
        let close = titlebar_button_rect(TitlebarButton::CloseTab, VP, top, 1.0);
        let newtab = titlebar_button_rect(TitlebarButton::NewTab, VP, top, 1.0);
        let toggle = titlebar_button_rect(TitlebarButton::ToggleSidebar, VP, top, 1.0);
        // Close hugs the right edge; new-tab is exactly one gap to its left.
        assert_eq!(
            close.x + close.w,
            VP[0] - 8.0,
            "close right edge at vp - pad"
        );
        assert_eq!(newtab.x, close.x - 8.0 - newtab.w, "new-tab one gap left");
        // Toggle sits just right of the traffic-light gutter; all three share the square
        // side and centre line.
        assert_eq!(
            toggle.x,
            TRAFFIC_LIGHT_GUTTER + 8.0,
            "toggle at gutter + pad"
        );
        assert_eq!(toggle.w, top - 2.0 * 8.0, "side = top_inset - 2·pad");
        assert!(
            close.w == newtab.w && newtab.w == toggle.w,
            "all square, equal"
        );
    }

    #[test]
    fn toggle_clears_traffic_light_gutter() {
        // R8.2 / D15 — the toggle never sits under the native traffic lights: its left
        // edge is at or past the reserved gutter, at every scale.
        for scale in [1.0_f64, 2.0] {
            let top = titlebar_height(scale);
            let toggle = titlebar_button_rect(TitlebarButton::ToggleSidebar, VP, top, scale);
            let gutter = TRAFFIC_LIGHT_GUTTER * scale as f32;
            assert!(
                toggle.x >= gutter,
                "toggle.x {} must clear gutter {} at scale {scale}",
                toggle.x,
                gutter
            );
        }
    }

    #[test]
    fn no_button_intersects_the_gutter() {
        // R8.2 — no drawn control overlaps the traffic-light band [0, gutter), at any scale.
        for scale in [1.0_f64, 2.0] {
            let top = titlebar_height(scale);
            let gutter = TRAFFIC_LIGHT_GUTTER * scale as f32;
            for kind in BUTTONS {
                let b = titlebar_button_rect(kind, VP, top, scale);
                assert!(
                    b.x >= gutter,
                    "{kind:?} left edge {} must clear gutter {} at scale {scale}",
                    b.x,
                    gutter
                );
            }
        }
    }

    #[test]
    fn height_is_positive_and_scales() {
        assert!(titlebar_height(1.0) > 0.0);
        assert_eq!(titlebar_height(1.0), 40.0);
        assert_eq!(titlebar_height(2.0), 80.0);
        assert!(
            titlebar_height(2.0) > titlebar_height(1.0),
            "scales with scale"
        );
        assert_eq!(titlebar_height(-1.0), 0.0, "clamped >= 0");
    }

    #[test]
    fn expanded_emits_band_and_buttons() {
        let mut out = Vec::new();
        build_titlebar_shapes(VP, 40.0, 1.0, false, None, &mut out);
        assert!(!out.is_empty());
        // The band is first, full width, opaque panel_bg.
        assert_eq!(out[0].bbox, [0.0, 0.0, VP[0], 40.0]);
        assert_eq!(out[0].fill0, linear(SidebarTheme::DEFAULT.panel_bg, 1.0));
        assert_eq!(out[0].fill0[3], 1.0, "the band must be opaque");
        // Band + the three buttons' strokes (Panel 2 + Plus 2 + cross 2).
        assert!(out.len() > 3, "band plus button strokes");
    }

    #[test]
    fn degenerate_inputs_emit_nothing() {
        let mut out = Vec::new();
        build_titlebar_shapes(VP, 0.0, 1.0, false, None, &mut out);
        assert!(out.is_empty(), "top_inset <= 0 emits nothing");
        out.clear();
        build_titlebar_shapes(VP, -5.0, 1.0, false, None, &mut out);
        assert!(out.is_empty(), "negative top_inset emits nothing");
        out.clear();
        build_titlebar_shapes([0.0, 600.0], 40.0, 1.0, false, None, &mut out);
        assert!(out.is_empty(), "zero viewport width emits nothing");
        out.clear();
        build_titlebar_shapes(VP, 40.0, f64::NAN, false, None, &mut out);
        assert!(out.is_empty(), "non-finite scale emits nothing");
    }

    #[test]
    fn toggle_glyph_reflects_collapsed_state() {
        // R7.3 — the observable state difference: the toggle glyph brightens to
        // icon_fg_hover while collapsed and is icon_fg while expanded. The panel glyph's
        // stroke (a capsule) carries the icon colour in its fill.
        let fg = linear(SidebarTheme::DEFAULT.icon_fg, 1.0);
        let fg_hover = linear(SidebarTheme::DEFAULT.icon_fg_hover, 1.0);
        let mut expanded = Vec::new();
        build_titlebar_shapes(VP, 40.0, 1.0, false, None, &mut expanded);
        let mut collapsed = Vec::new();
        build_titlebar_shapes(VP, 40.0, 1.0, true, None, &mut collapsed);
        assert_eq!(
            expanded.len(),
            collapsed.len(),
            "only the toggle colour changes"
        );
        assert!(
            collapsed.iter().any(|s| s.fill0 == fg_hover),
            "collapsed toggle is brightened"
        );
        assert!(
            !expanded.iter().any(|s| s.fill0 == fg_hover),
            "expanded toggle is not brightened"
        );
        assert!(
            expanded.iter().any(|s| s.fill0 == fg),
            "expanded toggle uses icon_fg"
        );
    }

    #[test]
    fn hover_adds_one_button_background() {
        let mut plain = Vec::new();
        build_titlebar_shapes(VP, 40.0, 1.0, false, None, &mut plain);
        let mut hovered = Vec::new();
        build_titlebar_shapes(
            VP,
            40.0,
            1.0,
            false,
            Some(TitlebarButton::NewTab),
            &mut hovered,
        );
        assert_eq!(
            hovered.len(),
            plain.len() + 1,
            "hover adds exactly the button background"
        );
    }

    #[test]
    fn new_returns_a_reachable_handle() {
        let (_layer, handle) = TitlebarLayer::new();
        handle.set_collapsed(true);
        assert!(handle.collapsed());
        handle.set_collapsed(false);
        assert!(!handle.collapsed());
    }
}
