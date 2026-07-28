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
    /// A pinned waiting-band row (0-based, top = oldest wait). Only the ≤5 real
    /// session rows are live; the "+N more" summary row is swallowed as `Panel`.
    WaitingRow(usize),
    /// The per-card menu button (the hamburger) on card `i` (sticky-tab-title D10/R3.1).
    /// Resolved *before* [`Card`](SidebarRegion::Card) so pressing the button opens the
    /// menu instead of switching to the tab it sits on.
    MenuButton(usize),
    /// Item `i` (0-based, top to bottom) of the currently open card menu
    /// (sticky-tab-title D10/R3.1). Resolved before everything in the list, because an
    /// open popup floats over the cards below it — a press there must never fall through
    /// to whatever card the popup happens to cover.
    MenuItem(usize),
    Card(usize),
    Panel,
}

/// Which card's menu is open and how many items it has (sticky-tab-title D10/R3.2).
///
/// This is **layout input, not renderer state**: the app decides whether a menu is open
/// and what it contains — `Reset to auto` is offered only for a tab that has a user
/// title, so the item count is 2 or 3 — and passes it down so this module can lay the
/// popup out and hit-test it. The renderer never toggles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenMenu {
    /// The card index the menu is anchored to.
    pub row: usize,
    /// How many items the menu draws (≥ 1). The app owns which items those are.
    pub items: usize,
}

/// Card-menu layout tokens, logical px (scaled at the point of use, like [`tokens`]).
pub mod menu {
    /// The hamburger's touch/hit square, logical px — the icon plus its padding.
    pub const BUTTON_PX: f32 = 22.0;
    /// Gap between the hamburger's right edge and the card's inner right edge.
    pub const BUTTON_INSET: f32 = 2.0;
    /// The popup's width, logical px — wide enough for "Reset to auto".
    pub const WIDTH: f32 = 150.0;
    /// One menu item's height, logical px.
    pub const ITEM_H: f32 = 28.0;
    /// Padding above the first item and below the last, logical px.
    pub const PAD_Y: f32 = 5.0;
    /// Gap between the hamburger and the popup's top edge, logical px.
    pub const GAP: f32 = 3.0;
    /// The popup's corner radius, logical px.
    pub const RADIUS: f32 = 10.0;
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
    /// The open card menu, if any (sticky-tab-title D10/R3.2) — pushed by the app exactly
    /// as `waiting_count` is. `None` = no menu open, and then no `MenuItem` region can be
    /// produced. Keeps [`hit_test`] a pure function of its inputs while the *decision* to
    /// open stays app-side.
    pub open_menu: Option<OpenMenu>,
}

impl SidebarLayoutInput {
    /// Renderer-side convenience: scale the metrics once from a frame. NOT the app's path
    /// (it has no `FrameCtx`) — the app writes the struct literal directly (D7/R6.8).
    pub fn from_frame(
        ctx: &crate::FrameCtx<'_>,
        row_h: f32,
        row_count: usize,
        waiting_count: usize,
        open_menu: Option<OpenMenu>,
    ) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w: ctx.left_inset,
            viewport: ctx.viewport,
            scale: ctx.scale_factor,
            row_h,
            row_count,
            waiting_count,
            metrics: SidebarMetrics::DEFAULT.scaled(ctx.scale_factor),
            open_menu,
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

/// The y of the first band row's top, physical px — below the header and the top pad.
fn waiting_content_top(scale: f64) -> f32 {
    header_h(scale) + waiting::PAD_TOP * scale as f32
}

/// The **drawn** rect of band row `i` (0-based), physical px — a full-width row band
/// spanning `[0, sidebar_w)`; the caller insets the leading icon and title itself. Both
/// the band draw (`sidebar_row`) and this module's hit-test derive positions from here,
/// so they cannot drift (SD2).
pub fn waiting_row_rect(i: usize, sidebar_w: f32, scale: f64) -> Rect {
    Rect {
        x: 0.0,
        y: waiting_content_top(scale) + i as f32 * waiting_row_h(scale),
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
    let rel = y - waiting_content_top(inp.scale);
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

/// The band the cards live in — below the pinned header AND the pinned waiting band, to
/// the framebuffer bottom. The waiting band pushes `list_top` down by its height (D9); an
/// empty band adds `0.0`, so the cards keep their original top (R1.2).
pub fn list_viewport(inp: &SidebarLayoutInput) -> Rect {
    let top = header_h(inp.scale) + waiting_band_h(inp.waiting_count, inp.scale);
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

/// The hamburger's rect **inside a given card rect** (sticky-tab-title R3.1): a square at
/// the card's top-right, within the card padding, vertically centred on line 1.
///
/// Takes the card rect rather than an index because the two callers reach a card rect by
/// different routes — the hit-test via [`row_rect`], the row layer via its own published
/// `SidebarLayout::card_rect` — and this keeps ONE formula for where the button sits
/// inside a card (SD2). Feed it either; they already agree on card rects, and now the
/// button cannot drift from the card it belongs to.
pub fn menu_button_in(card: Rect, metrics: &SidebarMetrics, scale: f64) -> Rect {
    let side = menu::BUTTON_PX * scale as f32;
    let inset = menu::BUTTON_INSET * scale as f32;
    Rect {
        x: card.x + card.w - metrics.card_pad_x - side + inset,
        y: card.y + metrics.card_pad_y + (metrics.title_px - side) * 0.5,
        w: side,
        h: side,
    }
}

/// The hamburger's rect on card `i` — the hit-test's route into [`menu_button_in`].
pub fn menu_button_rect(i: usize, s: f32, inp: &SidebarLayoutInput) -> Rect {
    menu_button_in(row_rect(i, s, inp), &inp.metrics, inp.scale)
}

/// The popup's rect for a given hamburger rect (sticky-tab-title R3.1/R3.2) — the shared
/// formula behind [`menu_rect`], so the drawn popup and the hit-tested one are the same
/// rectangle even though the two callers reach the button by different routes (SD2).
///
/// Anchored under the button, right-aligned to it, and **clamped into the viewport**: a
/// menu on the last card would otherwise hang off the bottom of the window, so it flips
/// to sit above its button instead.
pub fn menu_rect_for(button: Rect, items: usize, viewport: [f32; 2], scale: f64) -> Rect {
    let s = scale as f32;
    let w = menu::WIDTH * s;
    let h = items as f32 * menu::ITEM_H * s + 2.0 * menu::PAD_Y * s;
    let gap = menu::GAP * s;
    // Right-align with the button, then keep it inside the window's left edge (a narrow
    // sidebar would otherwise push it negative).
    let x = (button.x + button.w - w).max(0.0);
    let below = button.y + button.h + gap;
    // Prefer below the button; flip above when that would overflow the bottom.
    let preferred = if below + h <= viewport[1] {
        below
    } else {
        button.y - gap - h
    };
    // Then clamp unconditionally. The flip alone is not enough: a card scrolled far past
    // the bottom of a short window has no room on EITHER side, and a popup drawn off the
    // window is a popup the user cannot reach. Clamping keeps it on screen in every case
    // (and collapses to the top edge when the menu is taller than the window).
    let y = preferred.clamp(0.0, (viewport[1] - h).max(0.0));
    Rect { x, y, w, h }
}

/// Item `i`'s rect inside a popup rect — the shared formula behind [`menu_item_rect`].
pub fn menu_item_in(popup: Rect, i: usize, scale: f64) -> Rect {
    let item_h = menu::ITEM_H * scale as f32;
    Rect {
        x: popup.x,
        y: popup.y + menu::PAD_Y * scale as f32 + i as f32 * item_h,
        w: popup.w,
        h: item_h,
    }
}

/// The open menu's popup rect (sticky-tab-title R3.1/R3.2), or `None` when no menu is
/// open — the hit-test's route into [`menu_rect_for`].
pub fn menu_rect(s: f32, inp: &SidebarLayoutInput) -> Option<Rect> {
    let open = inp.open_menu?;
    let button = menu_button_rect(open.row, s, inp);
    Some(menu_rect_for(button, open.items, inp.viewport, inp.scale))
}

/// Item `i`'s rect inside the open popup (sticky-tab-title R3.1), or `None` when no menu
/// is open or `i` is past the item count.
pub fn menu_item_rect(i: usize, s: f32, inp: &SidebarLayoutInput) -> Option<Rect> {
    let open = inp.open_menu?;
    if i >= open.items {
        return None;
    }
    Some(menu_item_in(menu_rect(s, inp)?, i, inp.scale))
}

/// The menu item at physical `p`, or `None` when no menu is open or the point misses
/// every item (sticky-tab-title R3.1). The popup's own padding resolves to `None` — the
/// caller swallows it as `Panel` rather than letting it fall through to a card.
fn menu_item_at(p: [f32; 2], s: f32, inp: &SidebarLayoutInput) -> Option<usize> {
    let open = inp.open_menu?;
    (0..open.items).find(|&i| menu_item_rect(i, s, inp).is_some_and(|r| r.contains(p)))
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
    // 1.5. An OPEN CARD MENU floats above everything in the sidebar, so it resolves
    // before the band/header/list — otherwise a press on an item would fall through to
    // whatever the popup happens to cover (sticky-tab-title D10/R3.1). Its own padding
    // resolves to `Panel`: swallowed, not leaked to the card underneath.
    //
    // Deliberately ahead of the "outside the sidebar band" test below, because the popup
    // is right-aligned to a card and may extend past `sidebar_w` over the terminal.
    if let Some(popup) = menu_rect(s, inp) {
        if popup.contains(p) {
            return Some(match menu_item_at(p, s, inp) {
                Some(i) => SidebarRegion::MenuItem(i),
                None => SidebarRegion::Panel,
            });
        }
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
    // 3.5. The pinned waiting band (above the cards, R1.1): a live session row, else
    // Panel — the "+N more" summary row and the band padding are swallowed, never a
    // card (the band sits ABOVE the list origin, so it must resolve before step 4).
    let band_bottom = header_h(inp.scale) + waiting_band_h(inp.waiting_count, inp.scale);
    if p[1] < band_bottom {
        return Some(match waiting_row_at(p[1], inp) {
            Some(i) => SidebarRegion::WaitingRow(i),
            None => SidebarRegion::Panel,
        });
    }
    // 4. The list: a card, else Panel (dead space is swallowed, not leaked). The card's
    // hamburger resolves ahead of the card itself (sticky-tab-title R3.1) — pressing the
    // button must open the menu, not switch to the tab it sits on.
    Some(match row_at(p[1], s, inp) {
        Some(i) if menu_button_rect(i, s, inp).contains(p) => SidebarRegion::MenuButton(i),
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
            open_menu: None,
        }
    }

    /// An input with card `row`'s menu open, carrying `items` items.
    fn input_menu(row: usize, items: usize) -> SidebarLayoutInput {
        SidebarLayoutInput {
            open_menu: Some(OpenMenu { row, items }),
            ..input(340.0, 900.0, 1.0, 5)
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

    // ---- sticky-tab-title: the per-card menu (R3.1/R3.2) ----

    #[test]
    fn menu_button_sits_inside_its_card_top_right() {
        // R3.1 — the hamburger belongs to the card: inside its right padding, on line 1.
        let inp = input(340.0, 900.0, 1.0, 5);
        let card = row_rect(2, 0.0, &inp);
        let b = menu_button_rect(2, 0.0, &inp);
        assert!(b.x > card.x + card.w * 0.5, "right-aligned within the card");
        assert!(b.x + b.w <= card.x + card.w, "never overhangs the card");
        assert!(
            b.y >= card.y && b.y + b.h <= card.y + card.h,
            "vertically inside"
        );
    }

    #[test]
    fn menu_button_beats_the_card_it_sits_on() {
        // R3.1 — the whole point of ordering it ahead of `Card(i)`: pressing the button
        // must open the menu, not switch to the tab underneath it.
        let inp = input(340.0, 900.0, 1.0, 5);
        let b = menu_button_rect(1, 0.0, &inp);
        let centre = [b.x + b.w * 0.5, b.y + b.h * 0.5];
        assert_eq!(
            hit_test(centre, 0.0, &inp, false),
            Some(SidebarRegion::MenuButton(1))
        );
        // A point on the same card but away from the button is still the card.
        let card = row_rect(1, 0.0, &inp);
        assert_eq!(
            hit_test([card.x + 8.0, centre[1]], 0.0, &inp, false),
            Some(SidebarRegion::Card(1))
        );
    }

    #[test]
    fn open_menu_items_beat_the_cards_they_float_over() {
        // R3.1 — a popup floats above the list; a press on an item must never fall through
        // to whatever card happens to be underneath.
        let inp = input_menu(0, 3);
        for i in 0..3 {
            let item = menu_item_rect(i, 0.0, &inp).expect("item exists");
            let p = [item.x + item.w * 0.5, item.y + item.h * 0.5];
            assert_eq!(
                hit_test(p, 0.0, &inp, false),
                Some(SidebarRegion::MenuItem(i))
            );
        }
    }

    #[test]
    fn menu_padding_is_swallowed_not_leaked() {
        // R3.1 — the popup's own padding is inside the popup, so it must resolve to Panel
        // (a no-op), never to the card it covers.
        let inp = input_menu(0, 2);
        let popup = menu_rect(0.0, &inp).expect("menu open");
        let in_pad = [popup.x + popup.w * 0.5, popup.y + 1.0];
        assert_eq!(
            hit_test(in_pad, 0.0, &inp, false),
            Some(SidebarRegion::Panel)
        );
    }

    #[test]
    fn no_menu_open_yields_no_menu_regions() {
        // R3.2 — `open_menu: None` means no popup exists, so no point can resolve to a
        // MenuItem. The app owning the state is what makes this true.
        let inp = input(340.0, 900.0, 1.0, 5);
        assert_eq!(menu_rect(0.0, &inp), None);
        assert_eq!(menu_item_rect(0, 0.0, &inp), None);
    }

    #[test]
    fn menu_item_count_bounds_the_items() {
        // R3.2 — the app decides 2 or 3 items (Reset-to-auto is conditional); asking for
        // one past the count is None, never a phantom rect.
        let inp = input_menu(1, 2);
        assert!(menu_item_rect(0, 0.0, &inp).is_some());
        assert!(menu_item_rect(1, 0.0, &inp).is_some());
        assert_eq!(menu_item_rect(2, 0.0, &inp), None);
    }

    #[test]
    fn menu_items_stack_without_gaps_inside_the_popup() {
        // R3.1 — items tile the popup's inner height in order, so there is no dead strip
        // between two items where a press would silently do nothing.
        let inp = input_menu(0, 3);
        let popup = menu_rect(0.0, &inp).expect("menu open");
        let r0 = menu_item_rect(0, 0.0, &inp).unwrap();
        let r1 = menu_item_rect(1, 0.0, &inp).unwrap();
        let r2 = menu_item_rect(2, 0.0, &inp).unwrap();
        assert_eq!(r1.y, r0.y + r0.h);
        assert_eq!(r2.y, r1.y + r1.h);
        assert!(
            r0.y >= popup.y,
            "first item starts below the popup's top pad"
        );
        assert!(
            r2.y + r2.h <= popup.y + popup.h,
            "last item ends above the bottom pad"
        );
    }

    #[test]
    fn menu_flips_above_its_button_when_it_would_overflow_the_window() {
        // R3.1 — a card low in a short window: the popup flips above its button instead
        // of hanging off the bottom.
        let short = SidebarLayoutInput {
            open_menu: Some(OpenMenu { row: 2, items: 3 }),
            ..input(340.0, 300.0, 1.0, 5)
        };
        let button = menu_button_rect(2, 0.0, &short);
        let popup = menu_rect(0.0, &short).expect("menu open");
        assert!(
            popup.y + popup.h <= short.viewport[1],
            "popup stays inside the window"
        );
        assert!(popup.y < button.y, "flipped above its button");
    }

    #[test]
    fn menu_is_clamped_on_screen_even_with_no_room_either_side() {
        // R3.1 — the flip alone cannot save a card scrolled far past the bottom of a short
        // window (no room below, and above is off-screen too). The popup must still land
        // somewhere reachable rather than being drawn outside the window.
        let offscreen = SidebarLayoutInput {
            open_menu: Some(OpenMenu { row: 4, items: 3 }),
            ..input(340.0, 300.0, 1.0, 5)
        };
        let popup = menu_rect(0.0, &offscreen).expect("menu open");
        assert!(popup.y >= 0.0);
        assert!(popup.y + popup.h <= offscreen.viewport[1]);
    }

    #[test]
    fn menu_stays_inside_the_left_edge() {
        // R3.1 — right-aligning to the button on a narrow sidebar must not push the popup
        // off-screen to the left.
        let narrow = SidebarLayoutInput {
            open_menu: Some(OpenMenu { row: 0, items: 3 }),
            ..input(90.0, 900.0, 1.0, 3)
        };
        let popup = menu_rect(0.0, &narrow).expect("menu open");
        assert!(popup.x >= 0.0, "never negative x");
    }

    #[test]
    fn menu_scrolls_with_its_card() {
        // R3.1 — the popup is anchored to a card, so scrolling the list moves both; a
        // stale popup floating over a different card would mis-target the action.
        let inp = input_menu(2, 2);
        let a = menu_rect(0.0, &inp).expect("menu open");
        let b = menu_rect(40.0, &inp).expect("menu open");
        assert_eq!(b.y, a.y - 40.0);
    }
}
