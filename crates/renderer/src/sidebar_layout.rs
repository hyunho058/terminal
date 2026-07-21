//! Pure, state-free, GPU-free sidebar chrome layout (D7) — the single source of truth
//! (SD2) that this feature's layer *draws* from and `sidebar-interaction` *hit-tests*
//! against, so the two can never diverge. Every function is a `pub fn` of its arguments
//! over the explicitly-enumerated [`SidebarLayoutInput`]; nothing here imports a `wgpu`
//! type, holds state, or allocates.
//!
//! **The seam contract (for `sidebar-interaction`'s implementer):**
//! - Resolve **every** pointer event through [`hit_test`] (R7.1) — it is consulted even
//!   when collapsed reserves no width (pass `collapsed = true`, R7.2).
//! - [`SidebarRegion::Panel`] means "inside the sidebar, on no target" — swallow it;
//!   `None` means "over the grid" (R6.6).
//! - Push the raw cursor to this layer's handle and let it resolve hover via
//!   [`chrome_hit`] (R7.4) — the same function you call on a press, so highlight and
//!   click never disagree.
//! - The header height reaches the cards via the shared model's `list_top` (R7.5), the
//!   scroll offset via `scroll_px` (R7.6, you own it, D6). On a tab switch call
//!   [`scroll_to_reveal`] so the active row is visible (R7.7).
//!
//! **Traps this module deliberately avoids:** it derives no card height (that is
//! `sidebar-row-redesign`'s — `row_h` is an *input*, R4.8); it scales metrics nowhere —
//! `inp.metrics` arrives already scaled (R6.9); and [`restore_button_rect`] takes a scale
//! alone, so the collapsed affordance is reachable with no published card layout (D7).

use crate::sidebar_theme::SidebarMetrics;

/// Tokens the mock defines for surfaces only this feature draws (D15). Logical px,
/// multiplied by `scale` at the single point of use. **Destination note:** the two
/// colours belong in `SidebarTheme` and `ICON_PX` in `SidebarMetrics`; they live here
/// only because `sidebar_theme.rs` is another feature's declared surface.
pub mod tokens {
    /// Restore button fill (`#2c2822`).
    pub const RESTORE_BG: u32 = 0x002C_2822;
    /// Restore button border (`#3b362e`).
    pub const RESTORE_BORDER: u32 = 0x003B_362E;
    /// Restore button top offset from the window top-left, logical px.
    pub const RESTORE_TOP: f32 = 12.0;
    /// Restore button left offset, logical px.
    pub const RESTORE_LEFT: f32 = 14.0;
    /// Padding around the restore icon inside its button, logical px.
    pub const RESTORE_PAD: f32 = 8.0;
    /// Restore button corner radius, logical px.
    pub const RESTORE_RADIUS: f32 = 9.0;
    /// Header-button icon render size, logical px. A judgement, not a transcription —
    /// the mock's 24-unit viewBox was not captured in foundations' research (Known Gap).
    pub const ICON_PX: f32 = 16.0;
}

/// A plain axis-aligned rectangle in physical px, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Half-open containment: `[x, x+w) × [y, y+h)`, so adjacent rects never both claim
    /// a shared edge.
    pub fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x && p[0] < self.x + self.w && p[1] >= self.y && p[1] < self.y + self.h
    }
}

/// A live chrome target (D2 — no `Filter`: the filter is drawn but has no hit region).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeRegion {
    NewTab,
    HideSidebar,
    Restore,
}

/// The result of a full sidebar hit-test (D9). `Panel` = "inside the sidebar, on no
/// target" (swallow); the absence of a hit (over the grid) is `None`, never `Panel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarRegion {
    Chrome(ChromeRegion),
    Card(usize),
    Panel,
}

/// Everything the layout needs, all physical px except `scale` and `row_count` (D7).
/// **Every field is `pub`**: `sidebar-interaction` assembles this by struct literal from
/// its own accessors, having no `FrameCtx` (R6.8).
#[derive(Clone, Copy, Debug)]
pub struct SidebarLayoutInput {
    /// Sidebar band width, physical px; `0.0` while collapsed.
    pub sidebar_w: f32,
    /// Framebuffer size, physical px.
    pub viewport: [f32; 2],
    /// Window backing scale factor.
    pub scale: f64,
    /// Card height, physical px — from `SidebarLayout.card_h`, **NOT computed here** (D6).
    pub row_h: f32,
    pub row_count: usize,
    /// **Already scaled**: `SidebarMetrics::scaled(scale)`, never `SidebarMetrics::DEFAULT`.
    pub metrics: SidebarMetrics,
}

impl SidebarLayoutInput {
    /// Renderer-side convenience: scale the metrics once from a frame. NOT the app's path
    /// (it has no `FrameCtx`) — the app writes the struct literal directly (D7/R6.8).
    pub fn from_frame(
        ctx: &crate::FrameCtx<'_>,
        row_h: f32,
        row_count: usize,
    ) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w: ctx.left_inset,
            viewport: ctx.viewport,
            scale: ctx.scale_factor,
            row_h,
            row_count,
            metrics: SidebarMetrics::DEFAULT.scaled(ctx.scale_factor),
        }
    }
}

/// The pinned header band's height, physical px — the value `SidebarGeom.list_top` must
/// receive (D1). `14 + (ICON_PX + 2·6) + 12 = 54` logical, × `scale`. Takes `scale`
/// alone (not an input) because it is the designated scaling site for header geometry.
pub fn header_h(scale: f64) -> f32 {
    let m = SidebarMetrics::DEFAULT;
    let button_h = tokens::ICON_PX + 2.0 * m.button_pad; // 28 logical
    (m.header_pad_top + button_h + m.header_pad_bottom) * scale as f32
}

/// The button box side, physical px (square). Derived from `header_h` minus the scaled
/// vertical pads, so `ICON_PX` is scaled only inside `header_h` (R6.9).
fn button_side(inp: &SidebarLayoutInput) -> f32 {
    (header_h(inp.scale) - inp.metrics.header_pad_top - inp.metrics.header_pad_bottom).max(0.0)
}

/// The opaque header band.
pub fn header_rect(inp: &SidebarLayoutInput) -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: inp.sidebar_w,
        h: header_h(inp.scale),
    }
}

/// A live header button's box. `NewTab`/`HideSidebar` are right-aligned at the trailing
/// edge (hide at the corner); `Restore` is the collapsed affordance (D1/D7).
pub fn button_rect(region: ChromeRegion, inp: &SidebarLayoutInput) -> Rect {
    let bw = button_side(inp);
    let gap = inp.metrics.header_gap;
    let right = inp.sidebar_w - inp.metrics.header_pad_x;
    let y = inp.metrics.header_pad_top;
    let x = match region {
        ChromeRegion::HideSidebar => right - bw,
        ChromeRegion::NewTab => right - 2.0 * bw - gap,
        ChromeRegion::Restore => return restore_button_rect(inp.scale),
    };
    Rect { x, y, w: bw, h: bw }
}

/// The inert filter icon's box — one `header_gap` left of `NewTab` (D2). Published so the
/// other icons' positions and the deferred search field can depend on it; it has no hit
/// region.
pub fn filter_icon_rect(inp: &SidebarLayoutInput) -> Rect {
    let bw = button_side(inp);
    let gap = inp.metrics.header_gap;
    let right = inp.sidebar_w - inp.metrics.header_pad_x;
    Rect {
        x: right - 3.0 * bw - 2.0 * gap,
        y: inp.metrics.header_pad_top,
        w: bw,
        h: bw,
    }
}

/// The reserved, empty region left of the icons (D1). This feature draws nothing into it;
/// it is forward-compatible with the deferred search field.
pub fn search_slot_rect(inp: &SidebarLayoutInput) -> Rect {
    let filter = filter_icon_rect(inp);
    let x = inp.metrics.header_pad_x;
    Rect {
        x,
        y: inp.metrics.header_pad_top,
        w: (filter.x - inp.metrics.header_gap - x).max(0.0),
        h: button_side(inp),
    }
}

/// The band the cards live in — below the pinned header, to the framebuffer bottom.
pub fn list_viewport(inp: &SidebarLayoutInput) -> Rect {
    let top = header_h(inp.scale);
    Rect {
        x: 0.0,
        y: top,
        w: inp.sidebar_w,
        h: (inp.viewport[1] - top).max(0.0),
    }
}

/// Per-card vertical stride (card height + both vertical margins, D6).
pub fn row_stride(inp: &SidebarLayoutInput) -> f32 {
    inp.row_h + 2.0 * inp.metrics.card_margin_y
}

/// Total scrollable content height (D6).
pub fn content_height(inp: &SidebarLayoutInput) -> f32 {
    inp.row_count as f32 * row_stride(inp)
}

/// The largest valid scroll offset — `0.0` when the content fits (no overscroll, D6).
pub fn max_scroll(inp: &SidebarLayoutInput) -> f32 {
    (content_height(inp) - list_viewport(inp).h).max(0.0)
}

/// Clamp a scroll offset into `[0, max_scroll]`; a non-finite offset becomes `0.0` (D6).
pub fn clamp_scroll(s: f32, inp: &SidebarLayoutInput) -> f32 {
    if !s.is_finite() {
        0.0
    } else {
        s.clamp(0.0, max_scroll(inp))
    }
}

/// The minimal scroll offset bringing row `index` fully into the list viewport (D6):
/// unchanged if visible, its top if above, its bottom-minus-viewport if below, clamped.
pub fn scroll_to_reveal(index: usize, s: f32, inp: &SidebarLayoutInput) -> f32 {
    let stride = row_stride(inp);
    let vh = list_viewport(inp).h;
    let top = index as f32 * stride;
    let bottom = top + stride;
    let target = if top < s {
        top
    } else if bottom > s + vh {
        bottom - vh
    } else {
        s
    };
    clamp_scroll(target, inp)
}

/// The **drawn** rect of card `i` at scroll `s` — inset by `card_margin_y` inside its
/// stride band (D8). Not a bijection with [`row_at`] (the margins belong to the nearer
/// card); the two agree on centres and coverage (R6).
pub fn row_rect(i: usize, s: f32, inp: &SidebarLayoutInput) -> Rect {
    let stride = row_stride(inp);
    let band_top = list_viewport(inp).y + i as f32 * stride - s;
    Rect {
        x: inp.metrics.card_margin_x,
        y: band_top + inp.metrics.card_margin_y,
        w: (inp.sidebar_w - 2.0 * inp.metrics.card_margin_x).max(0.0),
        h: inp.row_h,
    }
}

/// The card at list-relative `y` — **stride-tiling**, so the inter-card gap belongs to
/// the nearer card and there is no dead zone (D8). `None` past the last row.
pub fn row_at(y: f32, s: f32, inp: &SidebarLayoutInput) -> Option<usize> {
    let stride = row_stride(inp);
    if stride <= 0.0 {
        return None;
    }
    let rel = y - list_viewport(inp).y + s;
    if rel < 0.0 {
        return None;
    }
    let i = (rel / stride).floor() as usize;
    (i < inp.row_count).then_some(i)
}

/// The index range whose stride bands intersect the list viewport (D5) — a cost measure
/// only; correctness never depends on it.
pub fn visible_rows(s: f32, inp: &SidebarLayoutInput) -> std::ops::Range<usize> {
    let stride = row_stride(inp);
    if stride <= 0.0 || inp.row_count == 0 {
        return 0..0;
    }
    let vh = list_viewport(inp).h;
    let start = (s / stride).floor().max(0.0) as usize;
    let start = start.min(inp.row_count);
    let end = (((s + vh) / stride).ceil().max(0.0) as usize)
        .min(inp.row_count)
        .max(start);
    start..end
}

/// The collapsed restore affordance's box — layout-free, so it is reachable when no card
/// layout has ever been published (D7). Uses only [`tokens`], no `SidebarMetrics`.
pub fn restore_button_rect(scale: f64) -> Rect {
    let s = scale as f32;
    let box_sz = (tokens::ICON_PX + 2.0 * tokens::RESTORE_PAD) * s;
    Rect {
        x: tokens::RESTORE_LEFT * s,
        y: tokens::RESTORE_TOP * s,
        w: box_sz,
        h: box_sz,
    }
}

/// Chrome-only, scroll-free hit-test — what the layer calls to resolve hover, and what
/// `sidebar-interaction` calls on a press (D10). The filter has no region (D2).
pub fn chrome_hit(p: [f32; 2], inp: &SidebarLayoutInput, collapsed: bool) -> Option<ChromeRegion> {
    if collapsed {
        return restore_button_rect(inp.scale)
            .contains(p)
            .then_some(ChromeRegion::Restore);
    }
    if button_rect(ChromeRegion::NewTab, inp).contains(p) {
        return Some(ChromeRegion::NewTab);
    }
    if button_rect(ChromeRegion::HideSidebar, inp).contains(p) {
        return Some(ChromeRegion::HideSidebar);
    }
    None
}

/// The ordered full hit-test (D9): collapsed → outside → header (never falling through to
/// a card) → list. `collapsed` is an argument, never a gate on whether to call this (D3).
pub fn hit_test(
    p: [f32; 2],
    s: f32,
    inp: &SidebarLayoutInput,
    collapsed: bool,
) -> Option<SidebarRegion> {
    // 1. Collapsed: only the restore button is live; every other point is the terminal.
    if collapsed {
        return restore_button_rect(inp.scale)
            .contains(p)
            .then_some(SidebarRegion::Chrome(ChromeRegion::Restore));
    }
    // 2. Outside the sidebar band.
    if p[0] < 0.0 || p[0] >= inp.sidebar_w || p[1] < 0.0 || p[1] >= inp.viewport[1] {
        return None;
    }
    // 3. Inside the header: a button, else Panel — never a card (D9's whole point).
    if header_rect(inp).contains(p) {
        return Some(match chrome_hit(p, inp, false) {
            Some(r) => SidebarRegion::Chrome(r),
            None => SidebarRegion::Panel,
        });
    }
    // 4. The list: a card, else Panel (dead space is swallowed, not leaked).
    Some(match row_at(p[1], s, inp) {
        Some(i) => SidebarRegion::Card(i),
        None => SidebarRegion::Panel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(sidebar_w: f32, viewport_h: f32, scale: f64, row_count: usize) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w,
            viewport: [sidebar_w, viewport_h],
            scale,
            row_h: 60.0 * scale as f32,
            row_count,
            metrics: SidebarMetrics::DEFAULT.scaled(scale),
        }
    }

    #[test]
    fn header_height_is_the_mock_box() {
        // R1.1
        assert_eq!(header_h(1.0), 54.0);
        assert_eq!(header_h(2.0), 108.0);
    }

    #[test]
    fn icons_right_aligned_in_order() {
        // R1.2 — hide at the corner, new-tab one gap left, filter one gap left of that.
        let inp = input(340.0, 900.0, 1.0, 5);
        let hide = button_rect(ChromeRegion::HideSidebar, &inp);
        let newtab = button_rect(ChromeRegion::NewTab, &inp);
        let filter = filter_icon_rect(&inp);
        assert_eq!(
            hide.x + hide.w,
            340.0 - 12.0,
            "hide right edge at sidebar_w - pad_x"
        );
        assert_eq!(hide.w, 28.0);
        assert_eq!(hide.h, 28.0);
        assert_eq!(newtab.x, hide.x - 8.0 - 28.0, "new-tab one header_gap left");
        assert_eq!(
            filter.x,
            newtab.x - 8.0 - 28.0,
            "filter one gap left of new-tab"
        );
        assert_eq!(hide.y, 14.0);
        assert_eq!(newtab.y, 14.0);
        assert_eq!(filter.y, 14.0);
    }

    #[test]
    fn search_slot_reserved_and_disjoint() {
        // R1.3 / R1.4 — non-empty (≥128 logical at 260), disjoint from every icon. The
        // 260–440 band is LOGICAL, so the physical width is logical × scale.
        for logical_w in [260.0_f32, 300.0, 340.0, 440.0] {
            for scale in [1.0_f64, 2.0] {
                let inp = input(logical_w * scale as f32, 900.0, scale, 3);
                let slot = search_slot_rect(&inp);
                assert!(
                    slot.w >= 128.0 * scale as f32,
                    "slot {} at logical w={logical_w}, scale={scale}",
                    slot.w
                );
                for icon in [
                    button_rect(ChromeRegion::HideSidebar, &inp),
                    button_rect(ChromeRegion::NewTab, &inp),
                    filter_icon_rect(&inp),
                ] {
                    assert!(
                        slot.x + slot.w <= icon.x + 0.01,
                        "search slot must not reach the icons"
                    );
                }
            }
        }
    }

    #[test]
    fn header_list_boundary_is_exact() {
        // R1.5
        let inp = input(340.0, 900.0, 2.0, 5);
        assert_eq!(header_rect(&inp).h, list_viewport(&inp).y);
        assert_eq!(header_rect(&inp).h, header_h(inp.scale));
    }

    #[test]
    fn chrome_regions_round_trip() {
        // R6.1 — the centre of each live region resolves to that region.
        for scale in [1.0_f64, 2.0] {
            let inp = input(340.0, 900.0, scale, 5);
            for region in [ChromeRegion::NewTab, ChromeRegion::HideSidebar] {
                let r = button_rect(region, &inp);
                let c = [r.x + r.w / 2.0, r.y + r.h / 2.0];
                assert_eq!(chrome_hit(c, &inp, false), Some(region));
            }
            // Collapsed: the restore button.
            let r = restore_button_rect(scale);
            let c = [r.x + r.w / 2.0, r.y + r.h / 2.0];
            assert_eq!(chrome_hit(c, &inp, true), Some(ChromeRegion::Restore));
        }
    }

    #[test]
    fn filter_is_never_a_hover_target() {
        // R2.4 — the filter icon's centre resolves to nothing (no hit region, D2).
        let inp = input(340.0, 900.0, 1.0, 5);
        let f = filter_icon_rect(&inp);
        let c = [f.x + f.w / 2.0, f.y + f.h / 2.0];
        assert_eq!(chrome_hit(c, &inp, false), None);
    }

    #[test]
    fn rows_round_trip_over_scroll() {
        // R6.2 — each drawn card's centre inverts to its index, at fractional offsets.
        let inp = input(340.0, 900.0, 1.5, 5);
        for &s in &[0.0_f32, 12.3, 40.0, 77.7] {
            for i in 0..inp.row_count {
                let r = row_rect(i, s, &inp);
                let cy = r.y + r.h / 2.0;
                assert_eq!(row_at(cy, s, &inp), Some(i), "row {i} at scroll {s}");
            }
        }
    }

    #[test]
    fn list_band_has_no_dead_zone() {
        // R6.3 — every integer y in the list band hits exactly one row, monotone, no hole.
        let inp = input(340.0, 900.0, 1.0, 6);
        let top = list_viewport(&inp).y as i32;
        let bottom = (list_viewport(&inp).y + content_height(&inp)) as i32;
        let mut last = 0usize;
        for y in top..bottom {
            let hit = row_at(y as f32, 0.0, &inp);
            assert!(hit.is_some(), "no dead zone at y={y}");
            let i = hit.unwrap();
            assert!(i >= last, "monotonic non-decreasing");
            last = i;
        }
    }

    #[test]
    fn header_never_falls_through_to_a_row() {
        // R6.4 — a scroll placing a row under the header still yields no Card in the band.
        let inp = input(340.0, 900.0, 1.0, 40);
        let hh = header_h(inp.scale) as i32;
        for y in 0..hh {
            let region = hit_test([170.0, y as f32], 500.0, &inp, false);
            assert!(
                !matches!(region, Some(SidebarRegion::Card(_))),
                "header must not resolve to a card at y={y}"
            );
        }
    }

    #[test]
    fn outside_and_dead_space() {
        // R6.5 / R6.6
        let inp = input(340.0, 900.0, 1.0, 2);
        assert_eq!(hit_test([341.0, 100.0], 0.0, &inp, false), None);
        assert_eq!(hit_test([170.0, -1.0], 0.0, &inp, false), None);
        // Header dead space and below-last-row both Panel, not None.
        let hdr_dead = [inp.metrics.header_pad_x + 1.0, 20.0];
        assert_eq!(
            hit_test(hdr_dead, 0.0, &inp, false),
            Some(SidebarRegion::Panel)
        );
        assert_eq!(
            hit_test([170.0, 850.0], 0.0, &inp, false),
            Some(SidebarRegion::Panel),
            "below the last row is Panel"
        );
    }

    #[test]
    fn collapsed_only_restore_is_live() {
        // R3.1-ish / R3.5 — collapsed, only the restore button hits; all else is None.
        let inp = input(0.0, 900.0, 1.0, 5);
        let r = restore_button_rect(inp.scale);
        let c = [r.x + r.w / 2.0, r.y + r.h / 2.0];
        assert_eq!(
            hit_test(c, 0.0, &inp, true),
            Some(SidebarRegion::Chrome(ChromeRegion::Restore))
        );
        assert_eq!(hit_test([400.0, 400.0], 0.0, &inp, true), None);
    }

    #[test]
    fn overflow_model_is_total() {
        // R4.1-R4.4 — fit → no scroll; overflow → clamps; non-finite → 0; empty is valid.
        let fits = input(340.0, 900.0, 1.0, 3);
        assert_eq!(max_scroll(&fits), 0.0);
        assert_eq!(
            clamp_scroll(100.0, &fits),
            0.0,
            "cannot scroll a fitting list"
        );
        let many = input(340.0, 400.0, 1.0, 40);
        assert!(max_scroll(&many) > 0.0);
        assert_eq!(
            clamp_scroll(1e9, &many),
            max_scroll(&many),
            "clamps to content"
        );
        assert_eq!(clamp_scroll(f32::NAN, &many), 0.0, "non-finite → 0");
        let empty = input(340.0, 900.0, 1.0, 0);
        assert_eq!(content_height(&empty), 0.0);
        assert_eq!(row_at(500.0, 0.0, &empty), None);
    }

    #[test]
    fn reveal_scrolls_minimally() {
        // R4.5-R4.7
        let inp = input(340.0, 400.0, 1.0, 40);
        let stride = row_stride(&inp);
        // A row below the fold: reveal to (index+1)*stride - viewport.h.
        let below = scroll_to_reveal(20, 0.0, &inp);
        assert!(below > 0.0);
        // The same row is now visible → unchanged.
        assert_eq!(scroll_to_reveal(20, below, &inp), below);
        // A row above the current offset scrolls to its top.
        let up = scroll_to_reveal(2, 10.0 * stride, &inp);
        assert_eq!(up, 2.0 * stride);
    }

    #[test]
    fn degenerate_geometry_never_panics() {
        // R8 — tiny window, zero viewport, narrow band, non-finite, over-scroll.
        let short = input(340.0, 10.0, 1.0, 5); // shorter than the header
        let _ = list_viewport(&short);
        assert!(list_viewport(&short).h >= 0.0);
        let zero = input(340.0, 0.0, 1.0, 5);
        assert_eq!(list_viewport(&zero).h, 0.0);
        let narrow = input(30.0, 900.0, 1.0, 5);
        let _ = search_slot_rect(&narrow); // must not underflow to a huge width
        assert!(search_slot_rect(&narrow).w >= 0.0);
        let inp = input(340.0, 900.0, 1.0, 5);
        // A scroll past the content still yields a valid (or None) row, never negative.
        assert!(row_at(100.0, 1e9, &inp).is_none_or(|i| i < inp.row_count));
    }
}
