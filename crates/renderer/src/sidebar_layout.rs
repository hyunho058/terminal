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
//! - The header height reaches the cards via the shared model's `list_top` (R7.5), the
//!   scroll offset via `scroll_px` (R7.6, you own it, D6). On a tab switch call
//!   [`scroll_to_reveal`] so the active row is visible (R7.7).
//!
//! **Traps this module deliberately avoids:** it derives no card height (that is
//! `sidebar-row-redesign`'s — `row_h` is an *input*, R4.8); and it scales metrics nowhere
//! — `inp.metrics` arrives already scaled (R6.9). (The collapsed restore affordance is
//! gone: that control now lives in the title bar, titlebar-tab-controls R5.)

use crate::sidebar_theme::SidebarMetrics;

/// Tokens the mock defines for surfaces only this feature draws (D15). Logical px,
/// multiplied by `scale` at the single point of use. **Destination note:** `ICON_PX`
/// belongs in `SidebarMetrics`; it lives here only because `sidebar_theme.rs` is another
/// feature's declared surface. (The restore-button tokens were retired with the button —
/// the title bar's toggle now serves restore, titlebar-tab-controls R5.)
pub mod tokens {
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

/// The result of a full sidebar hit-test (D9). `Panel` = "inside the sidebar, on no
/// target" (swallow); the absence of a hit (over the grid) is `None`, never `Panel`.
/// The new-tab / collapse / restore controls now live in the title bar
/// (titlebar-tab-controls R5), so the sidebar exposes no chrome target — the header is
/// inert (`Panel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarRegion {
    /// A pinned waiting-band row (0-based, top = oldest wait). Only the ≤5 real
    /// session rows are live; the "+N more" summary row is swallowed as `Panel`.
    WaitingRow(usize),
    Card(usize),
    Panel,
    /// The resize grab-band on the grid side of the right edge (sidebar-resize D5).
    ResizeHandle,
}

/// Waiting-band layout tokens, logical px (multiplied by `scale` at the point of use,
/// like [`tokens`]). The band is a pinned zone between the header and the card list
/// (D9); these are the only metrics it adds over the shared [`SidebarMetrics`].
pub mod waiting {
    /// The most session rows the band draws before collapsing the rest into a summary
    /// row (R1.6/D11).
    pub const MAX_ROWS: usize = 5;
    /// One band row's height, logical px (a compact single-line row: icon + title).
    pub const ROW_H: f32 = 30.0;
    /// Padding above the first band row, logical px.
    pub const PAD_TOP: f32 = 6.0;
    /// Padding below the last band row, logical px.
    pub const PAD_BOTTOM: f32 = 6.0;
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
    /// Number of pending waiting-band sessions — the count the app pushed as
    /// `TabBarModel.waiting.len()`. Drives the pinned band's height (which pushes
    /// `list_top` down) and the band-row hit region; `0` = no band (R1.2/D9).
    pub waiting_count: usize,
    /// **Already scaled**: `SidebarMetrics::scaled(scale)`, never `SidebarMetrics::DEFAULT`.
    pub metrics: SidebarMetrics,
    /// Physical px reserved by the top title bar (titlebar-tab-controls D6). Every
    /// top-relative sidebar y is shifted down by this from THIS single value, so the
    /// sidebar begins below the title bar and drawn rows stay aligned with hit-test rows.
    /// `0.0` when there is no title bar.
    pub top_inset: f32,
}

impl SidebarLayoutInput {
    /// Renderer-side convenience: scale the metrics once from a frame. NOT the app's path
    /// (it has no `FrameCtx`) — the app writes the struct literal directly (D7/R6.8).
    pub fn from_frame(
        ctx: &crate::FrameCtx<'_>,
        row_h: f32,
        row_count: usize,
        waiting_count: usize,
    ) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w: ctx.left_inset,
            viewport: ctx.viewport,
            scale: ctx.scale_factor,
            row_h,
            row_count,
            waiting_count,
            metrics: SidebarMetrics::DEFAULT.scaled(ctx.scale_factor),
            top_inset: ctx.top_inset,
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

/// The number of band rows actually DRAWN for `items` waiting sessions: up to
/// [`waiting::MAX_ROWS`] session rows plus one "+N more" summary row when the cap is
/// exceeded (R1.6/D11). `0` when there is nothing to show.
pub fn waiting_band_rows(items: usize) -> usize {
    if items == 0 {
        0
    } else {
        items.min(waiting::MAX_ROWS) + usize::from(items > waiting::MAX_ROWS)
    }
}

/// One waiting-band row's height, physical px.
pub fn waiting_row_h(scale: f64) -> f32 {
    waiting::ROW_H * scale as f32
}

/// The pinned waiting band's total height, physical px. `0.0` for an empty set so the
/// card list keeps its original `list_top` (R1.2); otherwise top+bottom padding plus one
/// fixed-height row per drawn band row (visible + optional summary, R1.6/D11).
pub fn waiting_band_h(items: usize, scale: f64) -> f32 {
    let rows = waiting_band_rows(items);
    if rows == 0 {
        0.0
    } else {
        (waiting::PAD_TOP + rows as f32 * waiting::ROW_H + waiting::PAD_BOTTOM) * scale as f32
    }
}

/// The y of the first band row's top, physical px — below the title bar, the header, and
/// the top pad. `top_inset` (titlebar-tab-controls D6) shifts the whole band down with the
/// rest of the sidebar.
fn waiting_content_top(top_inset: f32, scale: f64) -> f32 {
    top_inset + header_h(scale) + waiting::PAD_TOP * scale as f32
}

/// The **drawn** rect of band row `i` (0-based), physical px — a full-width row band
/// spanning `[0, sidebar_w)`; the caller insets the leading icon and title itself. Both
/// the band draw (`sidebar_row`) and this module's hit-test derive positions from here,
/// so they cannot drift (SD2). `top_inset` shifts the band below the title bar (D6), so
/// the draw caller MUST pass the live `ctx.top_inset`.
pub fn waiting_row_rect(i: usize, sidebar_w: f32, top_inset: f32, scale: f64) -> Rect {
    Rect {
        x: 0.0,
        y: waiting_content_top(top_inset, scale) + i as f32 * waiting_row_h(scale),
        w: sidebar_w,
        h: waiting_row_h(scale),
    }
}

/// The interactive band row at physical `y` — only the ≤[`waiting::MAX_ROWS`] real
/// session rows are live; the "+N more" summary row and the band's padding resolve to
/// `None` (swallowed as `Panel` by [`hit_test`], so a click there is a no-op, D11/R2.1).
fn waiting_row_at(y: f32, inp: &SidebarLayoutInput) -> Option<usize> {
    let items = inp.waiting_count;
    if items == 0 {
        return None;
    }
    let row_h = waiting_row_h(inp.scale);
    if row_h <= 0.0 {
        return None;
    }
    let rel = y - waiting_content_top(inp.top_inset, inp.scale);
    if rel < 0.0 {
        return None;
    }
    let k = (rel / row_h).floor() as usize;
    (k < items.min(waiting::MAX_ROWS)).then_some(k)
}

/// The button box side, physical px (square). Derived from `header_h` minus the scaled
/// vertical pads, so `ICON_PX` is scaled only inside `header_h` (R6.9).
fn button_side(inp: &SidebarLayoutInput) -> f32 {
    (header_h(inp.scale) - inp.metrics.header_pad_top - inp.metrics.header_pad_bottom).max(0.0)
}

/// The opaque header band. Its top begins at `top_inset` so it sits below the title bar
/// (titlebar-tab-controls D6).
pub fn header_rect(inp: &SidebarLayoutInput) -> Rect {
    Rect {
        x: 0.0,
        y: inp.top_inset,
        w: inp.sidebar_w,
        h: header_h(inp.scale),
    }
}

/// The inert filter icon's box — the trailing icon in the header (D2). Its `y` includes
/// `top_inset` so it sits inside the header band below the title bar (D6). Published so
/// the deferred search field can depend on it; it has no hit region.
pub fn filter_icon_rect(inp: &SidebarLayoutInput) -> Rect {
    let bw = button_side(inp);
    let gap = inp.metrics.header_gap;
    let right = inp.sidebar_w - inp.metrics.header_pad_x;
    Rect {
        x: right - 3.0 * bw - 2.0 * gap,
        y: inp.top_inset + inp.metrics.header_pad_top,
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
        y: inp.top_inset + inp.metrics.header_pad_top,
        w: (filter.x - inp.metrics.header_gap - x).max(0.0),
        h: button_side(inp),
    }
}

/// The band the cards live in — below the title bar, the pinned header, AND the pinned
/// waiting band, to the framebuffer bottom. One origin drives every card: `top_inset`
/// (title bar, titlebar-tab-controls D6) + `header_h` + `waiting_band_h` (waiting area,
/// D9), so drawn rows and hit-test rows move together. An empty waiting band adds `0.0`,
/// so with no waits the cards keep the title-bar+header top (R1.2).
pub fn list_viewport(inp: &SidebarLayoutInput) -> Rect {
    let top = inp.top_inset + header_h(inp.scale) + waiting_band_h(inp.waiting_count, inp.scale);
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

/// Grab-band width for the sidebar resize handle, logical px (× `scale` at use). Kept
/// thin so it intrudes only a few px into the terminal's left margin and the
/// absolute-cursor "jump" on press stays ≤ this (sidebar-resize D5/D7).
pub const RESIZE_GRAB_PX: f32 = 6.0;

/// The resize grab-band on the GRID side of the sidebar's right edge:
/// `x ∈ [sidebar_w, sidebar_w + grab]`, full viewport height. `None` when collapsed
/// (`sidebar_w <= 0`) so the draw layer AND the hit-test both derive "no handle" from
/// this ONE function (sidebar-resize SD3/D5/D10).
pub fn resize_handle_rect(inp: &SidebarLayoutInput) -> Option<Rect> {
    if inp.sidebar_w <= 0.0 {
        return None;
    }
    // Starts at `top_inset` so the band (and its drawn rule) sit below the title bar,
    // not up through it (titlebar-tab-controls D6).
    Some(Rect {
        x: inp.sidebar_w,
        y: inp.top_inset,
        w: RESIZE_GRAB_PX * inp.scale as f32,
        h: (inp.viewport[1] - inp.top_inset).max(0.0),
    })
}

/// The ordered full hit-test (D9): collapsed → resize-handle → outside → header (never
/// falling through to a card) → waiting band → list. `collapsed` is an argument, never a
/// gate on whether to call this (D3).
pub fn hit_test(
    p: [f32; 2],
    s: f32,
    inp: &SidebarLayoutInput,
    collapsed: bool,
) -> Option<SidebarRegion> {
    // 1. Collapsed: the sidebar exposes no target — restore now lives in the title bar
    //    (titlebar-tab-controls R5), so every point is the terminal.
    if collapsed {
        return None;
    }
    // 2. The resize grab-band, grid-side of the right edge (sidebar-resize D5) — checked
    //    BEFORE the outside-band `None` so a press just right of the edge starts a resize,
    //    not a grid action. `resize_handle_rect` is `None` when collapsed, so this is
    //    reached only for an expanded sidebar; being at `x >= sidebar_w` it never overlaps
    //    the header/card checks below (which run only for `x < sidebar_w`).
    if let Some(handle) = resize_handle_rect(inp) {
        if handle.contains(p) {
            return Some(SidebarRegion::ResizeHandle);
        }
    }
    // 3. Outside the sidebar band.
    if p[0] < 0.0 || p[0] >= inp.sidebar_w || p[1] < 0.0 || p[1] >= inp.viewport[1] {
        return None;
    }
    // 4. Inside the header: always Panel — the header hosts no live target now (its
    //    controls moved to the title bar, R5), and it must never fall through to a card
    //    (D9's whole point).
    if header_rect(inp).contains(p) {
        return Some(SidebarRegion::Panel);
    }
    // 5. The pinned waiting band (above the cards, R1.1): a live session row, else Panel —
    // the "+N more" summary row and the band padding are swallowed, never a card. The band
    // sits ABOVE the list origin (below the title bar + header), so it must resolve before
    // the list. `band_bottom` includes `top_inset` (titlebar-tab-controls D6) so it aligns
    // with the top_inset-shifted header and list.
    let band_bottom =
        inp.top_inset + header_h(inp.scale) + waiting_band_h(inp.waiting_count, inp.scale);
    if p[1] < band_bottom {
        return Some(match waiting_row_at(p[1], inp) {
            Some(i) => SidebarRegion::WaitingRow(i),
            None => SidebarRegion::Panel,
        });
    }
    // 6. The list: a card, else Panel (dead space is swallowed, not leaked).
    Some(match row_at(p[1], s, inp) {
        Some(i) => SidebarRegion::Card(i),
        None => SidebarRegion::Panel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(sidebar_w: f32, viewport_h: f32, scale: f64, row_count: usize) -> SidebarLayoutInput {
        input_w(sidebar_w, viewport_h, scale, row_count, 0)
    }

    fn input_w(
        sidebar_w: f32,
        viewport_h: f32,
        scale: f64,
        row_count: usize,
        waiting_count: usize,
    ) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w,
            viewport: [sidebar_w, viewport_h],
            scale,
            row_h: 60.0 * scale as f32,
            row_count,
            waiting_count,
            metrics: SidebarMetrics::DEFAULT.scaled(scale),
            top_inset: 0.0,
        }
    }

    #[test]
    fn header_height_is_the_mock_box() {
        // R1.1
        assert_eq!(header_h(1.0), 54.0);
        assert_eq!(header_h(2.0), 108.0);
    }

    #[test]
    fn top_inset_shifts_rows_and_resize_band_together() {
        // titlebar-tab-controls D6: a title bar of height k shifts every top-relative y
        // by k, from the ONE `top_inset` value — drawn rows AND hit-test rows AND the
        // header/resize band — so nothing desyncs.
        let mut inp = input(300.0, 900.0, 1.0, 5);
        inp.top_inset = 40.0;
        // The list viewport (hence every row) starts at top_inset + header_h.
        assert_eq!(list_viewport(&inp).y, 40.0 + header_h(1.0));
        // A drawn card's rect and the hit-test of its centre agree under the shift.
        let r = row_rect(1, 0.0, &inp);
        let hit = row_at(r.y + r.h / 2.0, 0.0, &inp);
        assert_eq!(
            hit,
            Some(1),
            "drawn row 1 centre hit-tests to row 1 (no desync)"
        );
        // The header band and the resize band both start at top_inset.
        assert_eq!(header_rect(&inp).y, 40.0);
        assert_eq!(resize_handle_rect(&inp).unwrap().y, 40.0);
        // The inert filter icon (the header's only remaining glyph, R5) shifts with the
        // band: top_inset + pad_top, not a bare pad_top.
        assert_eq!(filter_icon_rect(&inp).y, 40.0 + inp.metrics.header_pad_top);
    }

    #[test]
    fn resize_handle_is_grid_side_band_or_none_collapsed() {
        // sidebar-resize R1.1 / R6.1
        let inp = input(300.0, 900.0, 1.0, 5);
        let h = resize_handle_rect(&inp).expect("expanded → a band");
        assert_eq!(h.x, 300.0, "band starts at the right edge (grid side)");
        assert_eq!(h.w, RESIZE_GRAB_PX, "grab width at scale 1.0");
        assert_eq!(h.y, 0.0);
        assert_eq!(h.h, 900.0, "full viewport height");
        // Scale widens the band.
        let inp2 = input(600.0, 900.0, 2.0, 5);
        assert_eq!(resize_handle_rect(&inp2).unwrap().w, RESIZE_GRAB_PX * 2.0);
        // Collapsed (sidebar_w == 0) → no handle — ONE source for draw + hit-test (D10).
        assert_eq!(resize_handle_rect(&input(0.0, 900.0, 1.0, 5)), None);
    }

    #[test]
    fn hit_test_resize_handle_before_grid_and_not_over_body() {
        // sidebar-resize R1.1 / R1.2
        let inp = input(300.0, 900.0, 1.0, 5);
        // Just right of the edge, within the grab band → ResizeHandle…
        assert_eq!(
            hit_test([301.0, 400.0], 0.0, &inp, false),
            Some(SidebarRegion::ResizeHandle)
        );
        // …the left edge x == sidebar_w is included (half-open, grid-side)…
        assert_eq!(
            hit_test([300.0, 10.0], 0.0, &inp, false),
            Some(SidebarRegion::ResizeHandle)
        );
        // …a point in the sidebar body (x < sidebar_w) is NOT the resize handle…
        assert_ne!(
            hit_test([150.0, 400.0], 0.0, &inp, false),
            Some(SidebarRegion::ResizeHandle)
        );
        // …past the band (x > sidebar_w + grab) is the grid → None…
        assert_eq!(hit_test([320.0, 400.0], 0.0, &inp, false), None);
        // …and collapsed never yields a resize handle.
        assert_ne!(
            hit_test([2.0, 2.0], 0.0, &input(0.0, 900.0, 1.0, 5), true),
            Some(SidebarRegion::ResizeHandle)
        );
    }

    #[test]
    fn filter_icon_geometry() {
        // R1.2 (residual) — the inert filter icon keeps its slot after the +/hide
        // controls moved to the title bar (R5): a 28px square at pad_top, inside the band.
        let inp = input(340.0, 900.0, 1.0, 5);
        let filter = filter_icon_rect(&inp);
        assert_eq!(filter.w, 28.0);
        assert_eq!(filter.h, 28.0);
        assert_eq!(filter.y, 14.0);
        assert!(
            filter.x + filter.w <= 340.0 - 12.0 + 0.01,
            "filter stays inside sidebar_w - pad_x"
        );
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
                let icon = filter_icon_rect(&inp);
                assert!(
                    slot.x + slot.w <= icon.x + 0.01,
                    "search slot must not reach the filter icon"
                );
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
    fn filter_is_inert_panel() {
        // R2.4 / R5 — the filter icon has no hit region (D2); a press on it lands in the
        // header and is swallowed as Panel, never a chrome target (those are retired).
        let inp = input(340.0, 900.0, 1.0, 5);
        let f = filter_icon_rect(&inp);
        let c = [f.x + f.w / 2.0, f.y + f.h / 2.0];
        assert_eq!(hit_test(c, 0.0, &inp, false), Some(SidebarRegion::Panel));
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
        // Past the resize grab-band (sidebar_w=340 + grab=6 → ≥346) is the grid → None.
        // (The first ~6px right of the edge is now the resize handle — see
        // `hit_test_resize_handle_before_grid_and_not_over_body`.)
        assert_eq!(hit_test([347.0, 100.0], 0.0, &inp, false), None);
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
    fn collapsed_sidebar_is_all_none() {
        // R5 — collapsed, the sidebar exposes no target (restore moved to the title bar):
        // every point is the terminal (None), including where the old restore box sat.
        let inp = input(0.0, 900.0, 1.0, 5);
        assert_eq!(hit_test([20.0, 20.0], 0.0, &inp, true), None);
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

    // ---- Waiting band geometry (R1.1, R1.2, R1.6, R2.1 / D9, D11) ----

    #[test]
    fn waiting_band_rows_caps_and_summarizes() {
        // R1.6/D11 — 0→0, under the cap → one row each, at the cap → 5, over → 5 + summary.
        assert_eq!(waiting_band_rows(0), 0);
        assert_eq!(waiting_band_rows(1), 1);
        assert_eq!(waiting_band_rows(5), 5);
        assert_eq!(waiting_band_rows(9), 6, "5 shown + 1 summary row");
    }

    #[test]
    fn waiting_band_height_and_list_top_offset() {
        // R1.1/R1.2/D9 — an empty band adds nothing; a populated band pushes list_top down
        // by its exact height (PAD_TOP + rows*ROW_H + PAD_BOTTOM, scaled).
        assert_eq!(waiting_band_h(0, 1.0), 0.0);
        let empty = input_w(340.0, 900.0, 1.0, 5, 0);
        assert_eq!(
            list_viewport(&empty).y,
            header_h(1.0),
            "no band → original list_top"
        );
        // 1 item: 6 + 30 + 6 = 42 logical.
        assert_eq!(waiting_band_h(1, 1.0), 42.0);
        let one = input_w(340.0, 900.0, 1.0, 5, 1);
        assert_eq!(list_viewport(&one).y, header_h(1.0) + 42.0);
        // Scale multiplies the band height too.
        assert_eq!(waiting_band_h(1, 2.0), 84.0);
        // 9 items: 6 drawn rows (5 + summary) → 6 + 180 + 6 = 192 logical.
        assert_eq!(waiting_band_h(9, 1.0), 192.0);
        let nine = input_w(340.0, 900.0, 1.0, 5, 9);
        assert_eq!(list_viewport(&nine).y, header_h(1.0) + 192.0);
    }

    #[test]
    fn hit_test_resolves_band_rows_then_cards() {
        // R2.1/D9 — a click in band row i → WaitingRow(i); a click below the band → Card,
        // never mis-numbered by the band offset.
        let inp = input_w(340.0, 900.0, 1.0, 6, 3);
        // Band content starts at header_h + PAD_TOP = 60; rows are 30 tall.
        assert_eq!(
            hit_test([170.0, 65.0], 0.0, &inp, false),
            Some(SidebarRegion::WaitingRow(0))
        );
        assert_eq!(
            hit_test([170.0, 95.0], 0.0, &inp, false),
            Some(SidebarRegion::WaitingRow(1))
        );
        assert_eq!(
            hit_test([170.0, 125.0], 0.0, &inp, false),
            Some(SidebarRegion::WaitingRow(2))
        );
        // The band bottom == the list origin; a point just below is Card(0), not a band row.
        let band_bottom = header_h(1.0) + waiting_band_h(3, 1.0); // 54 + 102 = 156
        assert_eq!(list_viewport(&inp).y, band_bottom);
        assert_eq!(
            hit_test([170.0, band_bottom + 1.0], 0.0, &inp, false),
            Some(SidebarRegion::Card(0)),
            "the card list starts below the band, un-shifted"
        );
    }

    #[test]
    fn hit_test_summary_row_is_inert() {
        // R1.6/D11 — with 9 items the 6th drawn row is "+N more": inside the band but not
        // an interactive session row, so it resolves to Panel (a no-op), never WaitingRow(5).
        let inp = input_w(340.0, 900.0, 1.0, 4, 9);
        // Row 5 (the summary) spans [60 + 5*30, 60 + 6*30) = [210, 240).
        assert_eq!(
            hit_test([170.0, 220.0], 0.0, &inp, false),
            Some(SidebarRegion::Panel)
        );
        // Row 4 (the last real session row) is still live.
        assert_eq!(
            hit_test([170.0, 190.0], 0.0, &inp, false),
            Some(SidebarRegion::WaitingRow(4))
        );
    }
}
