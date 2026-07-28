//! The redesigned left sidebar: rounded two-line cards (line 1 mark + title, line 2
//! branch icon + branch), an active highlight, per-agent skins — drawn on the
//! hand-rolled wgpu renderer (SD1). This module holds the real implementation;
//! [`crate::tab_bar`] is a thin source-compatibility façade over it (D1).
//!
//! **The model bus (D3).** Four parties must see one model but only one is handed a
//! handle, so the model is a per-UI-thread `thread_local!` singleton reached by
//! [`TabModelHandle::shared`]. The app single-window/single-UI-thread invariant makes
//! this sound (the layer is `!Send`, exactly like the old `Rc<RefCell<_>>`). Borrow
//! discipline is structural: [`TabModelHandle::with`] / [`TabModelHandle::with_mut`]
//! are the only borrow entry points and each releases its borrow inside the call, so
//! no caller can hold one across another — the `RefCell` double-borrow that would
//! panic at ~60 fps is unrepresentable.
//!
//! **T1 scope.** This lands the model, the plain-data row/meta types, the singleton,
//! the [`MetaSource`] pull trait (D10), and the `TabBar` layer as a **stub** whose
//! `record` returns immediately — the drawing (pure layout T2, text T3, chrome T4)
//! lands next. The façade's legacy hit-test formulas ship here now (D7) so
//! `input.rs`'s out-of-surface test stays green before any layout is published.

use std::cell::RefCell;
use std::rc::Rc;

use bytemuck::{Pod, Zeroable};
use common::config::SidebarConfig;
use wgpu::util::DeviceExt;

use crate::sidebar_icons::{push_icon, Icon};
use crate::sidebar_layout::OpenMenu;
use crate::sidebar_shapes::{Fill, SidebarShape, SidebarShapePipeline};
use crate::sidebar_theme::{linear, AgentSkin, SidebarMetrics, SidebarTheme};
use crate::{DrawLayer, FrameCtx};

/// The z-order of the sidebar layer (above the terminal glyph pass = 0, below overlays).
pub const TAB_BAR_Z: i32 = 10;

/// Fraction of line 2's width the repo/folder name may claim before the git branch when a
/// task label took line 1 (tab-identity, R2.2): the repo is the priority identity on
/// overflow, so it gets the majority share and the branch takes the remainder.
const REPO_LINE2_WIDTH_SHARE: f32 = 0.55;

// ---------------------------------------------------------------------------
// Plain-data row + metadata types (SD3 — the renderer never names app types)
// ---------------------------------------------------------------------------

/// One tab as the app hands it over (D2): a stable id so asynchronous metadata can be
/// joined to it by id (never by index, D10), plus its title. Deliberately carries only
/// what `Tabs` owns; branch / mark / skin live in [`RowMeta`], pulled separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarRow {
    /// The app's `session::TabId` as a plain `u64` — the renderer must not know app types.
    pub tab_id: u64,
    /// The cwd basename — the bottom of the label chain, and line 2's repo/folder name
    /// when line 1 is custom.
    pub title: String,
    /// The **user's** sticky title for this tab, if any (sticky-tab-title R1.1). Set only
    /// by an explicit user action — never by the PTY — and outranks every other source on
    /// line 1, which is what makes a named tab stay named. `None` = never named / reset.
    pub label: Option<String>,
    /// The last OSC 0/2 title a *program* set on this tab's PTY (zsh precmd, vim, ssh),
    /// if any (sticky-tab-title D3). Carried so the app can resolve the full chain in its
    /// `MetaSource::fill`, where the AI-task label is fresh; it ranks below that label
    /// (D12) and is hidden entirely while `label` is set (D3). `None` = none seen.
    pub program: Option<String>,
}

/// The leading mark on a card's first line (D11/D12), drawn as a foundations icon —
/// never a font glyph, so it cannot become tofu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowMark {
    /// No agent — line 1 starts at the padding edge with no mark column.
    #[default]
    None,
    /// An agent is working (the `✳` asterisk).
    Agent,
    /// An agent is idle / ready (the `◇` diamond).
    Ready,
    /// An agent handed its turn back and is waiting for input (the `⏸` pause, in the
    /// waiting colour) — the card-side echo of the waiting band (D6). Mapped from
    /// `AgentState::Waiting` in `sidebar_model::mark_for`.
    Waiting,
}

/// One waiting-band signal's visual kind (D6). Renderer-owned so the renderer never
/// names the app's `WaitingKind`; the app maps `WaitingKind` → `WaitingSignal` when it
/// builds the model. Each maps to a leading icon + a theme colour in [`DrawLayer::record`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitingSignal {
    /// A foreground command exited with an error code (`✕`, error colour).
    Error,
    /// The agent finished its turn and is waiting for input (`⏸`, waiting colour).
    Waiting,
    /// A long clean foreground command completed (`✓`, completion colour).
    Completion,
}

/// One pinned waiting-band row as the app hands it over (D6/D9): the session's stable id
/// (so a click can switch to it, R2.1), its title, and which signal to draw. The app
/// builds these already sorted oldest-wait-first and capped-free (the ≤5 + "+N more" cap
/// is applied at draw time, R1.5/R1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitingRow {
    /// The app's `session::TabId` as a plain `u64` — the renderer must not know app types.
    pub tab_id: u64,
    pub title: String,
    pub signal: WaitingSignal,
}

/// Per-tab metadata pulled from the app each frame (D10). Owned strings + `Copy` marks;
/// `default()` is a plain card (no branch, no task, no mark, [`AgentSkin::Generic`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RowMeta {
    pub branch: Option<String>,
    pub mark: RowMark,
    pub skin: AgentSkin,
    /// Line 1's text, **already resolved by the app** (sticky-tab-title D7/R1.5) by the
    /// precedence user-title > AI-task > program-OSC title > cwd. The renderer holds no
    /// part of that policy: it draws this string. Pulled by `tab_id` like `branch`, so it
    /// refreshes live off the metadata generation — which is why the app resolves it in
    /// its `MetaSource::fill` and not when it assembles the rows.
    ///
    /// Empty for a `default()` row; the layer falls back to the row's cwd title then.
    pub line1: String,
    /// Whether [`line1`](RowMeta::line1) came from something other than the cwd title —
    /// a user title, an AI task, or a program title. Drives line 2's layout: when line 1
    /// is custom, the cwd/repo name moves down beside the branch (tab-identity R2.2).
    /// Decided by the app alongside `line1` so the two can never disagree.
    pub line1_is_custom: bool,
}

impl Default for RowMeta {
    fn default() -> Self {
        RowMeta {
            branch: None,
            mark: RowMark::None,
            skin: AgentSkin::Generic,
            line1: String::new(),
            line1_is_custom: false,
        }
    }
}

/// The renderer-side pull contract (D10): the layer refreshes its `Vec<RowMeta>` only
/// when [`generation`](MetaSource::generation) moves, joining by `tab_id`. Implemented
/// in the app crate (`sidebar_model.rs`), the single place naming `tab-metadata`.
pub trait MetaSource {
    /// Cheap, allocation-free change token.
    fn generation(&self) -> u64;
    /// Clear and refill `out` with one [`RowMeta`] per row, joined by `tab_id`.
    fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>);
}

/// The app-owned tab state the layer renders (D2). Same shape as before; the element
/// type of `titles` changed `String` → [`SidebarRow`], which `Tabs::titles()` matches,
/// so `terminal_app`'s struct literal compiles byte-unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabBarModel {
    /// One row per tab, left to right. (Named `titles` for the frozen call site; it now
    /// carries rows, not titles — a documented cost of not editing `terminal_app.rs`.)
    pub titles: Vec<SidebarRow>,
    /// Index of the active (highlighted) tab.
    pub active: usize,
    /// The pinned waiting-band rows (D9), oldest wait first, one per attention-needing
    /// session (already active-tab-excluded by T3). Empty = no band. The app builds these
    /// by joining T3's waiting items to the session titles.
    pub waiting: Vec<WaitingRow>,
    /// The open card menu (sticky-tab-title D10/R3.2), or `None`. Travels with the model
    /// — like `waiting` — so the row layer, the header layer, and the app's hit-test all
    /// read one value and cannot disagree about whether a popup is on screen (SD2). The
    /// **app** owns both the decision to open it and what it says.
    pub menu: Option<CardMenu>,
    /// The in-progress inline rename, if any (sticky-tab-title R2.1). `None` = nobody is
    /// being renamed.
    pub edit: Option<CardEdit>,
}

/// An in-progress inline rename (sticky-tab-title R2.1): the tab being named and the text
/// typed so far, including any uncommitted IME composition.
///
/// Keyed by `tab_id`, not by row index — the same join `RowMeta` uses — so a roster shift
/// under an open editor cannot retarget it to a different tab.
///
/// The renderer draws this **in place of** the resolved line-1 label on that one card. It
/// is the app's state: the app decides what is being edited and what the text is, and the
/// renderer only paints it, exactly like [`CardMenu`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardEdit {
    /// The tab being renamed (`session::TabId` as a plain `u64` — SD3).
    pub tab_id: u64,
    /// The text typed so far, with any IME preedit already appended by the app.
    pub text: String,
}

/// The open per-card menu as the app hands it over (sticky-tab-title D4/R3.2): which card
/// it is anchored to, and its item labels top to bottom.
///
/// The **labels** live here rather than in [`OpenMenu`] because only drawing needs them;
/// the geometry and hit-test need just the count, and keeping their input `Copy` keeps
/// [`hit_test`](crate::sidebar_layout::hit_test) a cheap pure function. `OpenMenu` is
/// derived from this at frame time, so the two can never disagree about the count.
///
/// The app decides the items — `Reset to auto` appears only for a tab that has a user
/// title, so a menu carries 2 or 3 (D4) — and the renderer draws exactly what it is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMenu {
    /// The card index the menu is anchored to.
    pub row: usize,
    /// Item labels, top to bottom. Never empty.
    pub items: Vec<String>,
}

impl CardMenu {
    /// The layout/hit-test view of this menu — position + item count, no strings.
    pub fn layout(&self) -> OpenMenu {
        OpenMenu {
            row: self.row,
            items: self.items.len(),
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry (T2 fills the pure layout; T1 declares the types the model + shims need)
// ---------------------------------------------------------------------------

/// Every input to the pure card layout, all in physical px (D7). `list_top`/`scroll_px`
/// are 0.0 here; `sidebar-chrome-collapse` supplies them through the model (D13).
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarGeom {
    pub sidebar_w: f32,
    pub viewport_h: f32,
    pub cell: [f32; 2],
    pub scale: f64,
    pub show_branch: bool,
    pub list_top: f32,
    pub scroll_px: f32,
}

/// The computed layout the layer draws into and publishes (D7); the hit-test then reads
/// these exact values, not merely the same formula, so draw and hit-test cannot drift.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarLayout {
    pub geom: SidebarGeom,
    pub count: usize,
    /// Per-card vertical stride (card height + both vertical margins).
    pub stride: f32,
    /// The drawn card height (excludes the vertical margins).
    pub card_h: f32,
    /// Horizontal margin between the sidebar edge and the card box.
    pub margin_x: f32,
    /// Vertical margin between the stride band edge and the card box.
    pub margin_y: f32,
}

/// The pure card layout (D6/D7). One card height per frame — the branch line is reserved
/// by `show_branch`, never by per-tab branch presence, so no card jitters when a branch
/// appears or vanishes (D6). Scales the metrics once; `record` shares that same result
/// via [`layout_with`] so a frame converts exactly once (R8.6).
pub fn layout(geom: &SidebarGeom, count: usize) -> SidebarLayout {
    layout_with(geom, count, &SidebarMetrics::DEFAULT.scaled(geom.scale))
}

/// The pure layout given **already-scaled** metrics — the single `scaled()` result a
/// frame produces (D6/R8.6), shared between the card geometry here and `record`'s chrome
/// so nothing converts logical→physical px twice.
fn layout_with(geom: &SidebarGeom, count: usize, m: &SidebarMetrics) -> SidebarLayout {
    let meta_ratio = if m.title_px > 0.0 {
        m.meta_px / m.title_px
    } else {
        1.0
    };
    let line1_h = geom.cell[1];
    let line2_h = geom.cell[1] * meta_ratio;
    let card_h = 2.0 * (m.border_w + m.card_pad_y)
        + line1_h
        + if geom.show_branch {
            m.card_gap + line2_h
        } else {
            0.0
        };
    let stride = card_h + 2.0 * m.card_margin_y;
    SidebarLayout {
        geom: geom.clone(),
        count,
        stride,
        card_h,
        margin_x: m.card_margin_x,
        margin_y: m.card_margin_y,
    }
}

impl SidebarLayout {
    /// The screen-space top of card `i`'s stride band (list origin + stride, scrolled).
    fn band_top(&self, i: usize) -> f32 {
        self.geom.list_top + i as f32 * self.stride - self.geom.scroll_px
    }

    /// Forward: card `i`'s drawn rect `[x, y, w, h]` in physical px, or `None` past the
    /// last card. The box sits inside its stride band by `margin_y` and spans the sidebar
    /// width less `margin_x` on each side.
    pub fn card_rect(&self, i: usize) -> Option<[f32; 4]> {
        if i >= self.count {
            return None;
        }
        let x = self.margin_x;
        let y = self.band_top(i) + self.margin_y;
        let w = (self.geom.sidebar_w - 2.0 * self.margin_x).max(0.0);
        Some([x, y, w, self.card_h])
    }

    /// Exact inverse: the card whose **stride band** contains `(x, y)` (D7 — the 3px
    /// margins are visual, so a click in a margin still selects its card). Horizontally
    /// the whole `[0, sidebar_w)` band is live. Returns `None` outside the visible range
    /// (D13), so a click below the last visible card is a no-op, not a phantom hit.
    pub fn card_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.stride <= 0.0 || x < 0.0 || x >= self.geom.sidebar_w {
            return None;
        }
        let rel = y - self.geom.list_top + self.geom.scroll_px;
        if rel < 0.0 {
            return None;
        }
        let i = (rel / self.stride).floor() as usize;
        (i < self.count && self.visible_range().contains(&i)).then_some(i)
    }

    /// The index range whose stride bands intersect the visible list region
    /// `[list_top, viewport_h)` (D13) — so a hundred tabs cost a handful of instances,
    /// not a hundred cards drawn off-screen.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        if self.stride <= 0.0 || self.count == 0 {
            return 0..0;
        }
        let g = &self.geom;
        let start = (g.scroll_px / self.stride).floor().max(0.0) as usize;
        let start = start.min(self.count);
        let end_f = ((g.viewport_h - g.list_top + g.scroll_px) / self.stride).ceil();
        let end = (end_f.max(0.0) as usize).min(self.count).max(start);
        start..end
    }
}

/// Truncate `s` to at most `max_w` physical px at a fixed `advance` per char, appending
/// `ellipsis` when anything was cut (D8). Never wider than `max_w`; iterates `chars()`
/// so it cannot split a codepoint; `max_w <= 0` or `advance <= 0` yields `""`.
pub fn truncate_to_width(s: &str, advance: f32, max_w: f32, ellipsis: char) -> String {
    if max_w <= 0.0 || advance <= 0.0 {
        return String::new();
    }
    let max_chars = (max_w / advance).floor() as usize;
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 0 {
        return String::new(); // not even one char (plus its ellipsis) fits
    }
    // Keep one fewer than fits and append the ellipsis, so width == max_chars*advance ≤ max_w.
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push(ellipsis);
    out
}

/// Line 1 of a card: the app-resolved label, or the cwd title when the row has no metadata
/// yet (sticky-tab-title D7/R1.5).
///
/// The precedence itself — user title > AI-task > program-OSC title > cwd — lives app-side
/// in `sidebar_model::resolve_line1`, the one place that sees all four sources with the
/// task label fresh. What remains here is the empty-means-no-metadata fallback, so a card
/// drawn before the first `MetaSource::fill` shows its folder rather than a blank line.
pub(crate) fn card_line1_text<'a>(resolved: &'a str, cwd_title: &'a str) -> &'a str {
    if resolved.is_empty() {
        cwd_title
    } else {
        resolved
    }
}

/// The caret drawn at the end of an inline rename. A thin bar, so it reads as a text
/// cursor rather than as a character in the name.
pub(crate) const EDIT_CARET: char = '▏';

/// What to draw on line 1 while a card is being renamed (sticky-tab-title R2.1): the typed
/// text with the caret appended, **tail-anchored** so the caret is always visible.
///
/// Editing happens in a card barely wider than a word, so a name longer than the card is
/// the normal case, not the edge case. Truncating from the end — what
/// [`truncate_to_width`] does for labels — would hide the very characters being typed, so
/// this keeps the END of the string and drops from the front, marking the elision with a
/// leading ellipsis.
pub(crate) fn edit_line_text(text: &str, advance: f32, max_w: f32, ellipsis: char) -> String {
    if max_w <= 0.0 || advance <= 0.0 {
        return String::new();
    }
    let max_chars = (max_w / advance).floor() as usize;
    if max_chars == 0 {
        return String::new();
    }
    let mut out: String = text.chars().collect();
    out.push(EDIT_CARET);
    let len = out.chars().count();
    if len <= max_chars {
        return out;
    }
    // Keep the last `max_chars - 1` chars (the caret among them) and mark the cut. With
    // room for a single glyph there is nothing to mark — an ellipsis alone would say
    // "something is here" while hiding the cursor, so the caret wins.
    let keep = max_chars - 1;
    if keep == 0 {
        return EDIT_CARET.to_string();
    }
    let tail: String = out.chars().skip(len - keep).collect();
    let mut trimmed = String::with_capacity(tail.len() + ellipsis.len_utf8());
    trimmed.push(ellipsis);
    trimmed.push_str(&tail);
    trimmed
}

/// The x-origin (physical px) at which line 2's git-branch icon may start after the repo name,
/// or `None` when even the icon (`meta_px` wide) would overrun `right_edge` (tab-identity,
/// R2.2 — the repo has priority, so a branch that can't fit is dropped rather than clipped).
pub(crate) fn line2_branch_origin(x: f32, meta_px: f32, right_edge: f32) -> Option<f32> {
    (x + meta_px <= right_edge).then_some(x)
}

/// The width (physical px) the repo/folder name may claim on line 2 before the git branch
/// when a task label took line 1 (tab-identity, R2.2): the repo is the priority identity, so
/// it gets [`REPO_LINE2_WIDTH_SHARE`] of `avail` when a branch follows — never less than one
/// glyph `advance` so the repo never vanishes entirely — or all of `avail` when there is no
/// branch. Pure so the repo-first policy is unit-testable without a GPU frame.
pub(crate) fn repo_line2_budget(avail: f32, advance: f32, has_branch: bool) -> f32 {
    if has_branch {
        (avail * REPO_LINE2_WIDTH_SHARE).max(advance)
    } else {
        avail.max(0.0)
    }
}

// ---------------------------------------------------------------------------
// The shared model + its handle (D3)
// ---------------------------------------------------------------------------

/// The one model every party shares (D3). Reached only through [`TabModelHandle`].
#[derive(Default)]
pub struct SidebarModel {
    /// Written by `terminal_app` via [`TabModelHandle::set`] (D2).
    pub rows: Vec<SidebarRow>,
    pub active: usize,
    /// The pinned waiting-band rows (D9), written by `terminal_app` via
    /// [`TabModelHandle::set`] / [`TabModelHandle::set_waiting`]. Empty = no band.
    pub waiting: Vec<WaitingRow>,
    /// Bumped by `set`; D10's second (row-change) pull gate.
    pub rows_rev: u64,
    /// Written by `sidebar-interaction`; `None` here (D12).
    pub hovered: Option<usize>,
    /// Written by `sidebar-chrome-collapse`; `0.0` here (D13).
    pub list_top: f32,
    pub scroll_px: f32,
    /// Published by the layer after drawing (D7).
    pub layout: Option<SidebarLayout>,
    /// The open card menu (sticky-tab-title D10/R3.2), written by the app through
    /// [`TabModelHandle::set`] / [`TabModelHandle::set_menu`]. `None` = none open.
    pub menu: Option<CardMenu>,
    /// The in-progress inline rename (sticky-tab-title R2.1), written by the rename
    /// overlay through [`TabModelHandle::set_edit`]. `None` = nobody is being renamed.
    pub edit: Option<CardEdit>,
    /// Installed once from `main.rs` (D10). Private: only this module's layer reads it.
    meta_source: Option<Rc<dyn MetaSource>>,
}

thread_local! {
    /// The per-UI-thread singleton (D3). Every party clones this same `Rc`.
    static SHARED: Rc<RefCell<SidebarModel>> = Rc::new(RefCell::new(SidebarModel::default()));
}

/// A handle onto the shared [`SidebarModel`] (D3). `Clone`/`Default`/[`TabBar::new`] all
/// yield clones of the one per-thread `Rc`, so `terminal_app`'s wiring is unchanged and
/// every other party reaches the same model through [`shared`](TabModelHandle::shared).
#[derive(Clone)]
pub struct TabModelHandle(Rc<RefCell<SidebarModel>>);

impl Default for TabModelHandle {
    fn default() -> Self {
        TabModelHandle::shared()
    }
}

impl TabModelHandle {
    /// The shared per-thread handle — the public reach for the parties not handed one.
    pub fn shared() -> TabModelHandle {
        SHARED.with(|s| TabModelHandle(s.clone()))
    }

    /// Replace the rows + active index + waiting band, bumping `rows_rev` so the layer's
    /// pull gate fires (D10). The frozen `set(TabBarModel)` signature (D2).
    pub fn set(&self, model: TabBarModel) {
        let mut m = self.0.borrow_mut();
        m.rows = model.titles;
        m.active = model.active;
        m.waiting = model.waiting;
        m.menu = model.menu;
        m.edit = model.edit;
        m.rows_rev = m.rows_rev.wrapping_add(1);
    }

    /// Start / update / end an inline rename (sticky-tab-title R2.1), bumping `rows_rev`
    /// so the layer repaints. Called on every keystroke, so it deliberately avoids
    /// `Tabs::titles`' per-tab `proc_pidinfo` — same reasoning as `set_menu`.
    pub fn set_edit(&self, edit: Option<CardEdit>) {
        let mut m = self.0.borrow_mut();
        m.edit = edit;
        m.rows_rev = m.rows_rev.wrapping_add(1);
    }

    /// The in-progress inline rename, cloned — so a row resync carries it through `set`
    /// rather than cancelling the edit the user is in the middle of.
    pub fn edit(&self) -> Option<CardEdit> {
        self.0.borrow().edit.clone()
    }

    /// Open or close a card's menu (sticky-tab-title R3.2), bumping `rows_rev` so the
    /// layer redraws. The incremental path for the common case — clicking the hamburger
    /// changes nothing about the tabs themselves, so this avoids `Tabs::titles`' per-tab
    /// `proc_pidinfo`, exactly as `set_waiting` does for the band.
    pub fn set_menu(&self, menu: Option<CardMenu>) {
        let mut m = self.0.borrow_mut();
        m.menu = menu;
        m.rows_rev = m.rows_rev.wrapping_add(1);
    }

    /// The open card menu, cloned (sticky-tab-title R3.2) — so a row resync can carry it
    /// through `set` without closing it.
    pub fn menu(&self) -> Option<CardMenu> {
        self.0.borrow().menu.clone()
    }

    /// The open menu's layout view (row + item count), or `None` (sticky-tab-title R3.2).
    /// The one place `SidebarLayoutInput::open_menu` is derived, so the geometry and the
    /// drawn popup always agree on how many items there are.
    pub fn open_menu(&self) -> Option<OpenMenu> {
        self.0.borrow().menu.as_ref().map(CardMenu::layout)
    }

    /// Replace only the waiting-band rows, bumping `rows_rev` so the layer refreshes (D9).
    /// The incremental path for a waiting-set change that did not open/close a tab (a
    /// background command completing, an agent handing its turn back), so the app need not
    /// recompute the tab titles (which re-does per-tab `proc_pidinfo`).
    pub fn set_waiting(&self, waiting: Vec<WaitingRow>) {
        let mut m = self.0.borrow_mut();
        m.waiting = waiting;
        m.rows_rev = m.rows_rev.wrapping_add(1);
    }

    /// Install the metadata pull source once (D10), from `main.rs`.
    pub fn set_meta_source(&self, source: Rc<dyn MetaSource>) {
        self.0.borrow_mut().meta_source = Some(source);
    }

    /// Publish the hovered card index (D12). Written by `sidebar-interaction`.
    pub fn set_hovered(&self, hovered: Option<usize>) {
        self.0.borrow_mut().hovered = hovered;
    }

    /// Publish the list top offset + scroll (D13). Written by `sidebar-chrome-collapse`.
    pub fn set_list_scroll(&self, list_top: f32, scroll_px: f32) {
        let mut m = self.0.borrow_mut();
        m.list_top = list_top;
        m.scroll_px = scroll_px;
    }

    /// The last layout the layer published (D7), owned. `None` before the first frame.
    pub fn layout(&self) -> Option<SidebarLayout> {
        self.0.borrow().layout.clone()
    }

    /// Read the model under a short borrow — the only immutable borrow entry point (D3).
    pub fn with<R>(&self, f: impl FnOnce(&SidebarModel) -> R) -> R {
        f(&self.0.borrow())
    }

    /// Mutate the model under a short borrow — the only mutable borrow entry point (D3).
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut SidebarModel) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

/// Reset the singleton between tests (D3): the `thread_local` leaks a published `layout`
/// or an installed `MetaSource` across serially-run tests, so every test that touches it
/// calls this first.
#[cfg(test)]
pub(crate) fn reset_shared_model() {
    SHARED.with(|s| *s.borrow_mut() = SidebarModel::default());
}

// ---------------------------------------------------------------------------
// Façade formulas (D7) — the frozen free functions `input.rs` / `settings.rs` call
// ---------------------------------------------------------------------------

/// Suggested sidebar width for a cell width. Retained for the frozen `set_left_inset`
/// call sites; its value is now advisory — the renderer owns the real width (D4).
pub fn width_px(cell_w: f32) -> f32 {
    (cell_w * 14.0).round().max(1.0)
}

/// The sidebar's **physical**-px width for a config + backing scale (D4): the logical
/// `width_px` scaled, `0.0` when collapsed. Defensive (R5.8) — the last guard before a
/// negative/`NaN` width reaches `grid_dims`: a non-finite width falls back to the mock
/// default (340) and the result is floored at `0.0`.
pub fn sidebar_width_px(cfg: &SidebarConfig, scale: f64) -> f32 {
    if cfg.collapsed {
        return 0.0;
    }
    let logical = if cfg.width_px.is_finite() {
        cfg.width_px
    } else {
        340.0
    };
    (logical * scale as f32).max(0.0)
}

/// The per-card vertical stride. Returns the published layout's stride when one exists,
/// else today's legacy formula (D7) so pre-first-frame hit-testing matches the old code.
pub fn tab_height_px(cell_h: f32) -> f32 {
    TabModelHandle::shared()
        .layout()
        .map(|l| l.stride)
        .unwrap_or_else(|| (cell_h * 1.5).round().max(1.0))
}

/// Hit-test: the tab index at physical `(x, y)`. When a layout is published it delegates
/// to [`SidebarLayout::card_at`] (bounding `x` by `sidebar_w`, capping at `count`,
/// ignoring `tab_h` whose value the caller can no longer compute); otherwise it falls
/// back to today's `floor(y / tab_h)` with today's bounds (D7). The signature is frozen
/// (D1) — it is called from the out-of-surface `input.rs`.
pub fn tab_at(x: f32, y: f32, sidebar_w: f32, tab_h: f32, count: usize) -> Option<usize> {
    match TabModelHandle::shared().layout() {
        Some(layout) => {
            if x < 0.0 || x >= sidebar_w {
                return None;
            }
            layout.card_at(x, y).filter(|&i| i < count)
        }
        None => {
            if x < 0.0 || x >= sidebar_w || y < 0.0 || tab_h <= 0.0 {
                return None;
            }
            let i = (y / tab_h).floor() as usize;
            (i < count).then_some(i)
        }
    }
}

// ---------------------------------------------------------------------------
// Card visual state resolution (D12)
// ---------------------------------------------------------------------------

/// A card's combined visual state (D12). Selection outranks hover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CardState {
    Idle,
    Hovered,
    Selected,
}

/// Resolve a card's `(background fill, border colour)` from its skin + state (D12's
/// table). A `Generic` idle card returns `None` fill — **no instance is emitted at all**,
/// keeping the panel continuous and the instance count low.
fn resolve_card(
    skin: AgentSkin,
    state: CardState,
    theme: &SidebarTheme,
) -> (Option<Fill>, Option<[f32; 4]>) {
    match skin {
        AgentSkin::Generic => match state {
            CardState::Idle => (None, None),
            CardState::Hovered => (Some(Fill::solid(linear(theme.card_bg_hover, 1.0))), None),
            CardState::Selected => (
                Some(Fill::solid(linear(theme.card_bg_active, 1.0))),
                Some(linear(theme.card_border_active, 1.0)),
            ),
        },
        AgentSkin::Claude | AgentSkin::Antigravity => {
            let s = skin.states();
            let (fill, border) = match state {
                CardState::Idle => (s.bg, s.dim),
                CardState::Hovered => (s.hi, s.dim),
                CardState::Selected => (s.hi, s.border),
            };
            (
                Some(Fill::gradient(fill.stops, fill.mid, fill.dir)),
                Some(border),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// The text pipeline (D9) — a resident-buffer glyph pass over the shared atlas
// ---------------------------------------------------------------------------

/// A textured glyph quad in physical px sampling the atlas coverage.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TextInstance {
    rect: [f32; 4],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TextUniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// Premultiplied-alpha blending — glyph coverage is emitted premultiplied by the shader.
const TEXT_PREMULTIPLIED: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// A `VERTEX_FRAGMENT` uniform entry — `crate::uniform_entry` is `VERTEX`-only, but the
/// text bind group's layout is shared with the fragment-stage texture/sampler (D9).
fn text_uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

const TEXT_WGSL: &str = r#"
struct U { viewport: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;
struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@vertex
fn vs_main(in: VsIn) -> VsOut {
    let px = in.rect.xy + in.corner * in.rect.zw;
    let clip = vec2<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(clip, 0.0, 1.0);
    o.uv = mix(in.uv0, in.uv1, in.corner);
    o.color = in.color;
    return o;
}
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(atlas_tex, atlas_samp, in.uv).r;
    return vec4<f32>(in.color.rgb * a, a); // premultiplied
}
"#;

/// The glyph pass. Mirrors foundations' shape-pipeline API; instances ride a resident
/// buffer (`upload_resident`, never `create_buffer_init` per frame — G1). The bind group
/// is rebuilt on a **composite** key `(format, atlas.generation(), cell)` so a live
/// `apply_config` atlas swap cannot leave it sampling a dead texture (D8's R8.4 fix).
struct SidebarTextPipeline {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind: Option<wgpu::BindGroup>,
    bind_key: Option<(wgpu::TextureFormat, u64, [u32; 2])>,
    instances: Option<wgpu::Buffer>,
}

impl SidebarTextPipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> SidebarTextPipeline {
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sidebar-text-quad"),
            contents: bytemuck::cast_slice(&crate::QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sidebar-text-uniform"),
            size: std::mem::size_of::<TextUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sidebar-text-bgl"),
            entries: &[
                text_uniform_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sidebar-text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
        });
        let pipeline = crate::build_pipeline(
            device,
            &shader,
            &bgl,
            format,
            std::mem::size_of::<TextInstance>() as u64,
            &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4],
            TEXT_PREMULTIPLIED,
        );
        SidebarTextPipeline {
            format,
            pipeline,
            bgl,
            quad,
            uniform,
            bind: None,
            bind_key: None,
            instances: None,
        }
    }

    fn ensure<'a>(
        slot: &'a mut Option<SidebarTextPipeline>,
        ctx: &FrameCtx<'_>,
    ) -> &'a mut SidebarTextPipeline {
        let stale = slot
            .as_ref()
            .map(|p| p.format != ctx.format)
            .unwrap_or(true);
        if stale {
            *slot = Some(SidebarTextPipeline::new(ctx.device, ctx.format));
        }
        slot.as_mut().expect("just built")
    }

    fn draw(&mut self, ctx: &mut FrameCtx<'_>, text: &[TextInstance]) {
        if text.is_empty() {
            return;
        }
        // Composite bind key (D8/R8.4): rebuild if the atlas texture may have been
        // swapped (generation) or replaced by a config-driven cell change.
        let key = (
            ctx.format,
            ctx.atlas.generation(),
            [ctx.cell[0].to_bits(), ctx.cell[1].to_bits()],
        );
        if self.bind_key != Some(key) {
            self.bind = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sidebar-text-bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(ctx.atlas.view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(ctx.atlas.sampler()),
                    },
                ],
            }));
            self.bind_key = Some(key);
        }
        ctx.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&TextUniforms {
                viewport: ctx.viewport,
                _pad: [0.0, 0.0],
            }),
        );
        let count = crate::upload_resident(
            ctx.device,
            ctx.queue,
            &mut self.instances,
            text,
            "sidebar-text",
        );
        if count == 0 {
            return;
        }
        let instances = self.instances.as_ref().expect("resident buffer present");
        let bind = self.bind.as_ref().expect("bind group present");
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sidebar-text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.draw(0..6, 0..count);
    }
}

/// Lay out `text` as glyph instances from `x` at `top` (physical px), scaled by `scale`
/// (1.0 title, `meta_ratio` sub-line, D5). Truncated to `max_w` with `ellipsis` (D8). The
/// title uses the atlas's synthesized bold; the sub-line the plain glyph.
#[allow(clippy::too_many_arguments)]
fn push_glyphs(
    out: &mut Vec<TextInstance>,
    ctx: &mut FrameCtx<'_>,
    text: &str,
    x: f32,
    top: f32,
    cell: [f32; 2],
    scale: f32,
    bold: bool,
    color: [f32; 4],
    max_w: f32,
    ellipsis: char,
) {
    let advance = cell[0] * scale;
    let line_h = cell[1] * scale;
    let ascent = line_h * 0.8; // preserve tab_bar.rs's baseline constant
    let baseline = top + ascent;
    let clipped = truncate_to_width(text, advance, max_w, ellipsis);
    let mut pen = x;
    for c in clipped.chars() {
        // Resolve through the fallback chain (#6) so a Hangul/CJK basename renders on the
        // fallback face instead of tofu, then carry the title's bold synthesis on the
        // resolved key. `resolve` returns an owned key, so the immutable borrow is released
        // before `get_or_insert_styled` takes the mutable one.
        let entry = ctx.atlas.resolve(c).and_then(|mut key| {
            key.bold = bold;
            ctx.atlas.get_or_insert_styled(ctx.queue, key)
        });
        if let Some(e) = entry {
            out.push(TextInstance {
                rect: [
                    pen + e.left * scale,
                    baseline - e.top * scale,
                    e.px_w * scale,
                    e.px_h * scale,
                ],
                uv0: e.uv0,
                uv1: e.uv1,
                color,
            });
        }
        pen += advance;
    }
}

// ---------------------------------------------------------------------------
// The layer (T3 text + T4 chrome)
// ---------------------------------------------------------------------------

/// The sidebar draw layer (D9). Owns two pipelines — foundations' shape pipeline for the
/// panel/cards/marks/icons and the local text pipeline for titles/branches — plus reused
/// instance + metadata buffers so a steady frame allocates nothing (SD4/D10).
pub struct TabBar {
    model: TabModelHandle,
    shape_pipe: Option<SidebarShapePipeline>,
    text_pipe: Option<SidebarTextPipeline>,
    shape_buf: Vec<SidebarShape>,
    text_buf: Vec<TextInstance>,
    /// Pulled from the app via `MetaSource`, refreshed only on a generation change (D10).
    meta: Vec<RowMeta>,
    meta_gen: u64,
    rows_rev_seen: u64,
}

impl TabBar {
    /// Create the layer and a write handle for the app (frozen tuple, D1). Both share the
    /// one per-thread model (D3).
    pub fn new() -> (TabBar, TabModelHandle) {
        let model = TabModelHandle::shared();
        (
            TabBar {
                model: model.clone(),
                shape_pipe: None,
                text_pipe: None,
                shape_buf: Vec::new(),
                text_buf: Vec::new(),
                meta: Vec::new(),
                meta_gen: 0,
                rows_rev_seen: u64::MAX,
            },
            model,
        )
    }

    /// Refresh `self.meta` from the pulled source, gated on the source generation and the
    /// row revision (D10). The source is called AFTER the model borrow is released (D3's
    /// reentrancy rule), so a `MetaSource` reaching `shared()` cannot double-borrow.
    fn refresh_meta(
        &mut self,
        rows: &[SidebarRow],
        rows_rev: u64,
        source: &Option<Rc<dyn MetaSource>>,
    ) {
        match source {
            Some(src) => {
                let gen = src.generation();
                if gen != self.meta_gen || rows_rev != self.rows_rev_seen {
                    src.fill(rows, &mut self.meta);
                    self.meta_gen = gen;
                    self.rows_rev_seen = rows_rev;
                }
            }
            None => {
                // No source (renderer tests, pre-install window): plain default cards.
                if rows_rev != self.rows_rev_seen || self.meta.len() != rows.len() {
                    self.meta.clear();
                    self.meta.resize(rows.len(), RowMeta::default());
                    self.rows_rev_seen = rows_rev;
                }
            }
        }
    }
}

impl DrawLayer for TabBar {
    fn z_order(&self) -> i32 {
        TAB_BAR_Z
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // D14/R5.9: nothing to draw for a collapsed band (inset ≤ 0) or a zero viewport.
        let sidebar_w = ctx.left_inset; // D4 — the band width is the inset, not ctx.sidebar
        if sidebar_w <= 0.0 || ctx.viewport[1] <= 0.0 {
            return;
        }

        // Copy everything out of the model under ONE borrow, then release it (D3) before
        // touching the (possibly reentrant) metadata source.
        let (rows, active, hovered, rows_rev, list_top, scroll_px, waiting, menu, edit, source) =
            self.model.with(|m| {
                (
                    m.rows.clone(),
                    m.active,
                    m.hovered,
                    m.rows_rev,
                    m.list_top,
                    m.scroll_px,
                    m.waiting.clone(),
                    m.menu.clone(),
                    m.edit.clone(),
                    m.meta_source.clone(),
                )
            });
        self.refresh_meta(&rows, rows_rev, &source);

        let show_branch = ctx.sidebar.show_git_branch;
        let show_agent = ctx.sidebar.show_agent_status;
        let cell = ctx.cell;
        let theme = SidebarTheme::from_config(ctx.sidebar);
        let m = SidebarMetrics::DEFAULT.scaled(ctx.scale_factor);
        let meta_ratio = if m.title_px > 0.0 {
            m.meta_px / m.title_px
        } else {
            1.0
        };
        let ellipsis = if ctx.atlas.glyph_id('…') != 0 {
            '…'
        } else {
            '.'
        };

        let geom = SidebarGeom {
            sidebar_w,
            viewport_h: ctx.viewport[1],
            cell,
            scale: ctx.scale_factor,
            show_branch,
            list_top,
            scroll_px,
        };
        // Reuse the single `scaled()` result `m` (R8.6) — do not convert a second time.
        let lay = layout_with(&geom, rows.len(), &m);
        let vis = lay.visible_range();

        let mut shapes = std::mem::take(&mut self.shape_buf);
        let mut text = std::mem::take(&mut self.text_buf);
        shapes.clear();
        text.clear();

        // Panel background + right border (the whole band).
        if let Some(bg) = SidebarShape::rounded_box(
            [0.0, 0.0, sidebar_w, ctx.viewport[1]],
            0.0,
            0.0,
            Fill::solid(linear(theme.panel_bg, 1.0)),
            [0.0; 4],
        ) {
            shapes.push(bg);
        }
        if let Some(border) = SidebarShape::rounded_box(
            [sidebar_w - m.border_w, 0.0, m.border_w, ctx.viewport[1]],
            0.0,
            0.0,
            Fill::solid(linear(theme.panel_border, 1.0)),
            [0.0; 4],
        ) {
            shapes.push(border);
        }

        // ---- Pinned waiting band (D9/R1): a distinct zone between the header and the
        // cards. Drawn at a FIXED position (unaffected by `scroll_px`) with the oldest
        // wait at the top; the card list's `list_top` was already pushed below it (the
        // chrome layer + `publish_scroll` both add `waiting_band_h`). Each row is a leading
        // signal icon + the session title only — no timer, no error snippet (R1.3).
        if !waiting.is_empty() {
            let scale = ctx.scale_factor;
            let band_top = crate::sidebar_layout::header_h(scale);
            let band_h = crate::sidebar_layout::waiting_band_h(waiting.len(), scale);
            // Inset by the border so the panel's right border stays continuous through the
            // band (the band fill is drawn AFTER the border).
            let band_w = (sidebar_w - m.border_w).max(0.0);
            // Subtle elevated fill so the band reads as its own pinned zone…
            if let Some(bg) = SidebarShape::rounded_box(
                [0.0, band_top, band_w, band_h],
                0.0,
                0.0,
                Fill::solid(linear(theme.waiting_band_bg, 1.0)),
                [0.0; 4],
            ) {
                shapes.push(bg);
            }
            // …and a 1px separator along its bottom edge, dividing it from the cards.
            if let Some(sep) = SidebarShape::rounded_box(
                [0.0, band_top + band_h - m.border_w, band_w, m.border_w],
                0.0,
                0.0,
                Fill::solid(linear(theme.waiting_band_border, 1.0)),
                [0.0; 4],
            ) {
                shapes.push(sep);
            }

            let shown = waiting.len().min(crate::sidebar_layout::waiting::MAX_ROWS);
            let left = m.header_pad_x;
            let right_edge = (sidebar_w - m.header_pad_x).max(left);
            // Up to 5 session rows (R1.6): icon + colour per signal (R1.4), title only.
            for (i, wr) in waiting.iter().take(shown).enumerate() {
                let rect = crate::sidebar_layout::waiting_row_rect(i, sidebar_w, scale);
                let (icon, color) = match wr.signal {
                    WaitingSignal::Error => (Icon::Cross, theme.error_fg),
                    WaitingSignal::Waiting => (Icon::Pause, theme.waiting_fg),
                    WaitingSignal::Completion => (Icon::Check, theme.completion_fg),
                };
                let iy = rect.y + (rect.h - m.mark_px) * 0.5;
                push_icon(&mut shapes, icon, left, iy.max(rect.y), m.mark_px, color);
                let title_x = left + m.mark_px + m.card_gap;
                let text_top = rect.y + ((rect.h - cell[1]) * 0.5).max(0.0);
                push_glyphs(
                    &mut text,
                    ctx,
                    &wr.title,
                    title_x,
                    text_top,
                    cell,
                    1.0,
                    false,
                    linear(theme.title_fg, 1.0),
                    (right_edge - title_x).max(0.0),
                    ellipsis,
                );
            }
            // "+N more" summary row (R1.6/D11) — non-interactive (its hit resolves to Panel).
            if waiting.len() > shown {
                let n = waiting.len() - shown;
                let rect = crate::sidebar_layout::waiting_row_rect(shown, sidebar_w, scale);
                let text_top = rect.y + ((rect.h - cell[1] * meta_ratio) * 0.5).max(0.0);
                let summary = format!("+{n} more");
                push_glyphs(
                    &mut text,
                    ctx,
                    &summary,
                    left,
                    text_top,
                    cell,
                    meta_ratio,
                    false,
                    linear(theme.meta_fg, 1.0),
                    (right_edge - left).max(0.0),
                    ellipsis,
                );
            }
        }

        let default_meta = RowMeta::default();
        let icon_gap = m.card_gap;
        for i in vis {
            let rect = match lay.card_rect(i) {
                Some(r) => r,
                None => continue,
            };
            let meta = self.meta.get(i).unwrap_or(&default_meta);
            let state = if i == active {
                CardState::Selected
            } else if hovered == Some(i) {
                CardState::Hovered
            } else {
                CardState::Idle
            };
            let (fill, border) = resolve_card(meta.skin, state, &theme);
            if let Some(fill) = fill {
                let bw = if border.is_some() { m.border_w } else { 0.0 };
                if let Some(card) = SidebarShape::rounded_box(
                    rect,
                    m.card_radius,
                    bw,
                    fill,
                    border.unwrap_or([0.0; 4]),
                ) {
                    shapes.push(card);
                }
            }

            let inner_x = rect[0] + m.border_w + m.card_pad_x;
            let inner_top = rect[1] + m.border_w + m.card_pad_y;
            let right_edge = rect[0] + rect[2] - m.border_w - m.card_pad_x;

            // Line 1: optional mark, then the bold title.
            let mut title_x = inner_x;
            if show_agent {
                let mark = match meta.mark {
                    RowMark::Agent => Some((Icon::Asterisk, theme.mark_agent_fg)),
                    RowMark::Ready => Some((Icon::Diamond, theme.mark_ready_fg)),
                    RowMark::Waiting => Some((Icon::Pause, theme.waiting_fg)),
                    RowMark::None => None,
                };
                if let Some((icon, color)) = mark {
                    let my = inner_top + (cell[1] - m.mark_px) * 0.5;
                    push_icon(
                        &mut shapes,
                        icon,
                        inner_x,
                        my.max(inner_top),
                        m.mark_px,
                        color,
                    );
                    title_x = inner_x + m.mark_px + icon_gap;
                }
            }
            // Line 1 is resolved app-side (sticky-tab-title D7) by the precedence
            // user > AI-task > program-OSC > cwd; the renderer draws what it was handed
            // and owns none of that policy. An empty `line1` means a row with no metadata
            // yet (`RowMeta::default()`), which falls back to the cwd title.
            // The per-card menu affordance (sticky-tab-title D4/R3.1) — always drawn, so
            // the control is discoverable without hovering. Its rect comes from the shared
            // `menu_button_in` formula fed THIS card's rect, so the drawn button and the
            // hit-tested one are the same square (SD2).
            let button = crate::sidebar_layout::menu_button_in(
                crate::sidebar_layout::Rect {
                    x: rect[0],
                    y: rect[1],
                    w: rect[2],
                    h: rect[3],
                },
                &m,
                ctx.scale_factor,
            );
            push_icon(
                &mut shapes,
                Icon::Hamburger,
                button.x,
                button.y,
                button.w,
                theme.icon_fg,
            );
            let title_text = card_line1_text(&meta.line1, rows[i].title.as_str());
            // Line 1 stops short of the hamburger (sticky-tab-title R3.1) — line 2 keeps
            // the full width, since the button only occupies the title line.
            let line1_right = (button.x - icon_gap).min(right_edge);
            // Is THIS card being renamed (sticky-tab-title R2.1)? Joined by `tab_id`, not
            // by index, so a roster shift cannot move the editor to another tab.
            let editing = edit.as_ref().filter(|e| e.tab_id == rows[i].tab_id);
            if let Some(e) = editing {
                // A field treatment behind the title line, so the card reads as an input
                // rather than as a label that mysteriously changed.
                let field = [
                    title_x - m.card_gap * 0.5,
                    inner_top - m.card_gap * 0.25,
                    (line1_right - title_x + m.card_gap).max(0.0),
                    cell[1] + m.card_gap * 0.5,
                ];
                if let Some(bg) = SidebarShape::rounded_box(
                    field,
                    m.card_radius * 0.5,
                    m.border_w,
                    Fill::solid(linear(theme.panel_bg, 1.0)),
                    linear(theme.card_border_active, 1.0),
                ) {
                    shapes.push(bg);
                }
                // Tail-anchored so the caret stays visible while typing a long name.
                let line =
                    edit_line_text(&e.text, cell[0], (line1_right - title_x).max(0.0), ellipsis);
                push_glyphs(
                    &mut text,
                    ctx,
                    &line,
                    title_x,
                    inner_top,
                    cell,
                    1.0,
                    true,
                    linear(theme.title_fg, 1.0),
                    (line1_right - title_x).max(0.0),
                    ellipsis,
                );
            } else {
                push_glyphs(
                    &mut text,
                    ctx,
                    title_text,
                    title_x,
                    inner_top,
                    cell,
                    1.0,
                    true,
                    linear(theme.title_fg, 1.0),
                    (line1_right - title_x).max(0.0),
                    ellipsis,
                );
            }

            // Line 2 (D6-reserved). When a task label took line 1, the repo/folder name
            // (the cwd title) moves here as the secondary identity, ahead of the git branch
            // and given width priority on overflow (repo-first, R2.2); otherwise line 2 is
            // the branch alone, exactly as before.
            if show_branch {
                let line2_top = inner_top + cell[1] + m.card_gap;
                let advance = cell[0] * meta_ratio;
                let branch = meta.branch.as_deref();
                let mut x = inner_x;
                // When line 1 is a custom label (a user title, an AI task, or a program
                // title), the cwd/repo name moves to line 2 as the secondary identity; a
                // plain cwd card keeps the branch-only line 2 as before. The app decides
                // this alongside `line1` so the two can never disagree (sticky-tab-title).
                let line1_is_custom = meta.line1_is_custom;
                if line1_is_custom {
                    let repo_budget = repo_line2_budget(right_edge - x, advance, branch.is_some());
                    let repo = truncate_to_width(&rows[i].title, advance, repo_budget, ellipsis);
                    push_glyphs(
                        &mut text,
                        ctx,
                        &repo,
                        x,
                        line2_top,
                        cell,
                        meta_ratio,
                        false,
                        linear(theme.meta_fg, 1.0),
                        repo_budget,
                        ellipsis,
                    );
                    x += repo.chars().count() as f32 * advance + icon_gap;
                }
                // Draw the branch (icon + text) only if at least its icon still fits after the
                // repo name (R2.2 — the repo has priority; a branch that can't fit is dropped).
                if let (Some(branch), Some(bx)) =
                    (branch, line2_branch_origin(x, m.meta_px, right_edge))
                {
                    let iy = line2_top + (cell[1] * meta_ratio - m.meta_px) * 0.5;
                    push_icon(
                        &mut shapes,
                        Icon::GitBranch,
                        bx,
                        iy.max(line2_top),
                        m.meta_px,
                        theme.icon_fg,
                    );
                    let branch_x = bx + m.meta_px + icon_gap;
                    push_glyphs(
                        &mut text,
                        ctx,
                        branch,
                        branch_x,
                        line2_top,
                        cell,
                        meta_ratio,
                        false,
                        linear(theme.meta_fg, 1.0),
                        (right_edge - branch_x).max(0.0),
                        ellipsis,
                    );
                }
            }
        }

        // ---- The open card menu (sticky-tab-title D4/D10/R3.1). Drawn LAST so it layers
        // over the cards it floats above — shapes and glyphs are each submitted in push
        // order, so "last" is "on top". Its rect comes from the same `menu_rect_for`
        // formula the hit-test uses, fed the same button rect, so what the user sees and
        // what a press resolves to cannot disagree (SD2).
        if let Some(menu) = menu {
            if let Some(card) = lay.card_rect(menu.row) {
                let button = crate::sidebar_layout::menu_button_in(
                    crate::sidebar_layout::Rect {
                        x: card[0],
                        y: card[1],
                        w: card[2],
                        h: card[3],
                    },
                    &m,
                    ctx.scale_factor,
                );
                let popup = crate::sidebar_layout::menu_rect_for(
                    button,
                    menu.items.len(),
                    ctx.viewport,
                    ctx.scale_factor,
                );
                // Panel: the active card's fill (a raised surface) with the selected
                // card's border, so the popup reads as chrome rather than as a card.
                if let Some(bg) = SidebarShape::rounded_box(
                    [popup.x, popup.y, popup.w, popup.h],
                    crate::sidebar_layout::menu::RADIUS * ctx.scale_factor as f32,
                    m.border_w,
                    Fill::solid(linear(theme.card_bg_active, 1.0)),
                    linear(theme.card_border_active, 1.0),
                ) {
                    shapes.push(bg);
                }
                for (i, label) in menu.items.iter().enumerate() {
                    let item = crate::sidebar_layout::menu_item_in(popup, i, ctx.scale_factor);
                    let text_left = item.x + m.card_pad_x;
                    let text_top = item.y + ((item.h - cell[1]) * 0.5).max(0.0);
                    push_glyphs(
                        &mut text,
                        ctx,
                        label,
                        text_left,
                        text_top,
                        cell,
                        1.0,
                        false,
                        linear(theme.title_fg, 1.0),
                        (item.x + item.w - m.card_pad_x - text_left).max(0.0),
                        ellipsis,
                    );
                }
            }
        }

        {
            let sp = SidebarShapePipeline::ensure(&mut self.shape_pipe, ctx);
            sp.draw(ctx, &shapes);
        }
        {
            let tp = SidebarTextPipeline::ensure(&mut self.text_pipe, ctx);
            tp.draw(ctx, &text);
        }

        // Publish the exact layout we drew, so the hit-test reads these values (D7).
        self.model.with_mut(|mm| mm.layout = Some(lay));

        self.shape_buf = shapes;
        self.text_buf = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_at_fallback_matches_legacy_formula() {
        // R4.6/R4.7 — before any layout is published, tab_at reproduces the old
        // floor(y/tab_h) behaviour, including input.rs's frozen assertion.
        reset_shared_model();
        assert_eq!(tab_at(40.0, 65.0, 160.0, 30.0, 3), Some(2));
        // Out of the x band, below the last tab, and negative y all reject.
        assert_eq!(tab_at(200.0, 65.0, 160.0, 30.0, 3), None);
        assert_eq!(tab_at(40.0, 500.0, 160.0, 30.0, 3), None);
        assert_eq!(tab_at(40.0, -1.0, 160.0, 30.0, 3), None);
    }

    #[test]
    fn tab_height_px_fallback_is_legacy_stride() {
        // R4.7 — with no layout, the stride is today's (cell_h*1.5).round().
        reset_shared_model();
        assert_eq!(tab_height_px(20.0), 30.0);
    }

    #[test]
    fn set_replaces_rows_and_bumps_rev() {
        // R6.2 — set() writes rows/active and bumps the row-change gate.
        reset_shared_model();
        let h = TabModelHandle::shared();
        h.set(TabBarModel {
            titles: vec![SidebarRow {
                tab_id: 7,
                title: "proj".into(),
                label: None,
                program: None,
            }],
            active: 0,
            ..Default::default()
        });
        h.with(|m| {
            assert_eq!(m.rows.len(), 1);
            assert_eq!(m.rows[0].tab_id, 7);
            assert_eq!(m.active, 0);
            assert_eq!(m.rows_rev, 1);
        });
        h.set(TabBarModel {
            titles: Vec::new(),
            active: 0,
            ..Default::default()
        });
        h.with(|m| assert_eq!(m.rows_rev, 2, "each set bumps rows_rev"));
    }

    #[test]
    fn shared_handles_see_one_model() {
        // R6.3 — every party reaches the same singleton.
        reset_shared_model();
        let a = TabModelHandle::shared();
        let b = TabModelHandle::shared();
        a.set(TabBarModel {
            titles: vec![SidebarRow {
                tab_id: 1,
                title: "x".into(),
                label: None,
                program: None,
            }],
            active: 0,
            ..Default::default()
        });
        b.with(|m| assert_eq!(m.rows.len(), 1, "b sees a's write"));
    }

    struct ReentrantSource;
    impl MetaSource for ReentrantSource {
        fn generation(&self) -> u64 {
            // Re-enters shared() — legal because the caller must not hold a borrow across
            // this call (D3). Proves the reentrancy discipline is satisfiable.
            TabModelHandle::shared().layout().map_or(0, |_| 1)
        }
        fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>) {
            out.clear();
            out.extend(rows.iter().map(|_| RowMeta::default()));
        }
    }

    #[test]
    fn meta_source_can_reenter_shared_after_borrow_released() {
        // R6.5 (discipline) — the layer copies out under `with`, drops the borrow, then
        // calls the source (which itself reaches shared()). No double-borrow panic.
        reset_shared_model();
        let h = TabModelHandle::shared();
        h.set_meta_source(Rc::new(ReentrantSource));
        h.set(TabBarModel {
            titles: vec![SidebarRow {
                tab_id: 3,
                title: "a".into(),
                label: None,
                program: None,
            }],
            active: 0,
            ..Default::default()
        });
        // Copy rows + the source Rc out under one borrow (the record() pattern)…
        let (rows, source) = h.with(|m| (m.rows.clone(), m.meta_source.clone()));
        // …then call the source AFTER the borrow is released.
        let mut meta = Vec::new();
        if let Some(src) = source {
            let _ = src.generation();
            src.fill(&rows, &mut meta);
        }
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0], RowMeta::default());
    }

    // ---- T2: pure layout, hit-test, truncation ----

    fn geom(sidebar_w: f32, viewport_h: f32, show_branch: bool) -> SidebarGeom {
        SidebarGeom {
            sidebar_w,
            viewport_h,
            cell: [8.0, 16.0],
            scale: 1.0,
            show_branch,
            list_top: 0.0,
            scroll_px: 0.0,
        }
    }

    #[test]
    fn card_height_reserves_the_branch_line_by_config() {
        // R3 / D6 — the second line is reserved by show_branch, for every card, so a
        // per-tab branch appearing never changes the stride.
        let two = layout(&geom(340.0, 2000.0, true), 5);
        let one = layout(&geom(340.0, 2000.0, false), 5);
        assert!(
            two.card_h > one.card_h,
            "a branch-line card is taller than a one-line card"
        );
        assert_eq!(two.stride, two.card_h + 6.0, "stride adds both 3px margins");
        // Height is independent of count — one card height per frame.
        let two_many = layout(&geom(340.0, 2000.0, true), 40);
        assert_eq!(two.card_h, two_many.card_h);
        assert_eq!(two.stride, two_many.stride);
    }

    #[test]
    fn card_rect_and_card_at_are_exact_inverses() {
        // R4.1 — the redesign's replacement for slot_y/tab_at exact-inverse (SD2). Over
        // the whole stride band (margins included), card_at recovers the drawn index.
        let l = layout(&geom(340.0, 4000.0, true), 6);
        for i in 0..l.count {
            let rect = l.card_rect(i).expect("card in range");
            let cx = rect[0] + rect[2] / 2.0;
            let cy = rect[1] + rect[3] / 2.0;
            assert_eq!(l.card_at(cx, cy), Some(i), "card center round-trips");
            // Anywhere in the stride band selects the same card, margins included.
            let band_top = i as f32 * l.stride;
            assert_eq!(l.card_at(170.0, band_top), Some(i), "band top");
            assert_eq!(
                l.card_at(170.0, band_top + l.stride - 0.01),
                Some(i),
                "band bottom"
            );
        }
        // Outside the x band and below the last card → None.
        assert_eq!(l.card_at(-1.0, 10.0), None);
        assert_eq!(l.card_at(340.0, 10.0), None);
        assert_eq!(l.card_at(170.0, l.count as f32 * l.stride + 1.0), None);
    }

    #[test]
    fn visible_range_culls_offscreen_cards() {
        // R4.2/R4.3 / D13 — only the bands intersecting the viewport are visible, and
        // card_at returns None for a culled index even though it is < count.
        let l = layout(&geom(340.0, 200.0, true), 20);
        let vis = l.visible_range();
        assert_eq!(vis.start, 0);
        // 200px viewport / ~67px stride ⇒ 3 cards visible.
        assert!(vis.end >= 3 && vis.end <= 4, "got {vis:?}");
        assert!(vis.end < l.count, "most cards are culled");
        // A click deep past the viewport bottom is a no-op (D13).
        assert_eq!(l.card_at(170.0, 190.0 + 5.0 * l.stride), None);
    }

    #[test]
    fn published_layout_makes_shims_delegate() {
        // R4.5 — once a layout is published, tab_at delegates to card_at and
        // tab_height_px returns the published stride.
        reset_shared_model();
        let l = layout(&geom(340.0, 4000.0, true), 4);
        let stride = l.stride;
        TabModelHandle::shared().with_mut(|m| m.layout = Some(l));
        assert_eq!(
            tab_height_px(999.0),
            stride,
            "delegates to published stride"
        );
        // tab_at delegates: a point in band 2 → Some(2) (tab_h arg ignored).
        assert_eq!(tab_at(170.0, 2.5 * stride, 340.0, 0.0, 4), Some(2));
        reset_shared_model();
    }

    #[test]
    fn d12_card_state_table() {
        // R1.x / D12 — the six combinations resolve as the table says; a Generic idle
        // card emits no instance (None fill).
        let t = SidebarTheme::DEFAULT;
        assert!(resolve_card(AgentSkin::Generic, CardState::Idle, &t)
            .0
            .is_none());
        let (f, b) = resolve_card(AgentSkin::Generic, CardState::Hovered, &t);
        assert!(f.is_some() && b.is_none(), "generic hover: bg, no border");
        let (f, b) = resolve_card(AgentSkin::Generic, CardState::Selected, &t);
        assert!(f.is_some() && b.is_some(), "generic selected: bg + border");
        // Agent cards always carry a skin fill + border, idle border is the dim tint.
        for skin in [AgentSkin::Claude, AgentSkin::Antigravity] {
            for state in [CardState::Idle, CardState::Hovered, CardState::Selected] {
                let (f, b) = resolve_card(skin, state, &t);
                assert!(
                    f.is_some() && b.is_some(),
                    "{skin:?} {state:?} draws a card"
                );
            }
        }
    }

    #[test]
    fn truncate_to_width_fits_and_elides() {
        // D8 — px-based, per line, ellipsis on cut, never wider than max_w, char-safe.
        assert_eq!(truncate_to_width("hello", 10.0, 100.0, '…'), "hello");
        // 11 chars at advance 10 into 50px ⇒ 5 fit ⇒ keep 4 + ellipsis.
        let cut = truncate_to_width("hello world", 10.0, 50.0, '…');
        assert_eq!(cut, "hell…");
        assert!(cut.chars().count() as f32 * 10.0 <= 50.0);
        // Degenerate inputs.
        assert_eq!(truncate_to_width("x", 10.0, 0.0, '…'), "");
        assert_eq!(truncate_to_width("x", 0.0, 10.0, '…'), "");
        // Char-safe on multibyte, no panic, respects boundaries.
        let jp = truncate_to_width("日本語テスト", 10.0, 35.0, '…');
        assert_eq!(jp, "日本…");
    }

    // ---- T3: tab-identity card line-1 / line-2 decisions (renderer/sidebar_row.rs) ----

    #[test]
    fn card_line1_draws_what_the_app_resolved() {
        // sticky-tab-title D7/R1.5 — precedence is the app's job (and is tested there, in
        // `sidebar_model::resolve_line1`). All the renderer promises is to draw the string
        // it was handed, whichever tier it came from.
        assert_eq!(card_line1_text("Frontend", "terminal"), "Frontend");
        assert_eq!(
            card_line1_text("refactor the auth module", "terminal"),
            "refactor the auth module"
        );
    }

    #[test]
    fn edit_line_shows_the_caret_at_the_end() {
        // sticky-tab-title R2.1 — the inline field is the card's own title line, so the
        // caret has to be part of the drawn string.
        assert_eq!(edit_line_text("Front", 10.0, 200.0, '…'), "Front▏");
        // An empty name still shows a caret, so an opened field is visibly an input.
        assert_eq!(edit_line_text("", 10.0, 200.0, '…'), "▏");
    }

    #[test]
    fn edit_line_keeps_the_tail_visible_when_the_name_outgrows_the_card() {
        // The whole reason this is not `truncate_to_width`: a sidebar card is barely
        // wider than a word, so a name longer than the card is NORMAL while typing.
        // Truncating from the end would hide the characters being typed and the caret.
        let long = "Frontend-service-worker";
        // 6 chars fit at advance 10 in 60px.
        let out = edit_line_text(long, 10.0, 60.0, '…');
        assert!(out.ends_with('▏'), "caret stays visible: {out}");
        assert!(out.starts_with('…'), "elision marked at the FRONT: {out}");
        assert!(
            out.chars().count() as f32 * 10.0 <= 60.0,
            "never wider than the field"
        );
        // It is the tail of the name that survives, not the head.
        assert!(out.contains("rker"), "kept the end of the string: {out}");
        assert!(!out.contains("Front"), "dropped the head: {out}");
    }

    #[test]
    fn edit_line_degenerate_widths_do_not_panic() {
        assert_eq!(edit_line_text("abc", 10.0, 0.0, '…'), "");
        assert_eq!(edit_line_text("abc", 0.0, 100.0, '…'), "");
        // Exactly one char of room: the caret alone wins, since it is the tail.
        assert_eq!(edit_line_text("abc", 10.0, 10.0, '…'), "▏");
    }

    #[test]
    fn card_line1_falls_back_to_cwd_before_the_first_meta_fill() {
        // A row drawn before the app's first `MetaSource::fill` has `RowMeta::default()`,
        // whose `line1` is empty. That must show the folder, never a blank line.
        assert_eq!(card_line1_text("", "terminal"), "terminal");
    }

    #[test]
    fn repo_budget_prioritizes_repo_when_branch_present() {
        // R2.2 — with a branch to follow, the repo/folder name gets the majority share.
        let avail = 100.0;
        let b = repo_line2_budget(avail, 8.0, true);
        assert_eq!(b, avail * REPO_LINE2_WIDTH_SHARE);
        assert!(
            b > 0.0 && b < avail,
            "repo takes a majority but leaves room for the branch"
        );
    }

    #[test]
    fn repo_budget_takes_full_width_without_branch() {
        // R2.2 — no branch → the repo/folder name may use the whole line.
        assert_eq!(repo_line2_budget(100.0, 8.0, false), 100.0);
        // Negative avail (degenerate) clamps to 0, never panics.
        assert_eq!(repo_line2_budget(-5.0, 8.0, false), 0.0);
    }

    #[test]
    fn repo_budget_never_below_one_glyph() {
        // R2.2 robustness — on a very narrow card the repo still keeps at least one glyph
        // of budget (the advance floor), so it never silently vanishes behind the branch.
        assert_eq!(repo_line2_budget(4.0, 8.0, true), 8.0);
    }

    #[test]
    fn repo_line2_truncates_within_its_budget() {
        // R1.5 — the repo name on line 2 is elided to fit the budget the policy hands it, so
        // a long repo name never overruns into the branch segment.
        let advance = 8.0;
        let budget = repo_line2_budget(80.0, advance, true); // 44px ⇒ 5 glyphs
        let drawn = truncate_to_width("very-long-repository-name", advance, budget, '…');
        assert!(drawn.chars().count() as f32 * advance <= budget);
        assert!(drawn.ends_with('…'), "a long repo name is elided");
    }

    #[test]
    fn line2_branch_fits_when_icon_within_edge() {
        // R2.2 — the branch icon starts at x when it (meta_px wide) fits before right_edge.
        assert_eq!(line2_branch_origin(50.0, 10.0, 100.0), Some(50.0));
        // Exactly flush against the edge still counts as fitting.
        assert_eq!(line2_branch_origin(90.0, 10.0, 100.0), Some(90.0));
    }

    #[test]
    fn line2_branch_dropped_when_icon_overruns_edge() {
        // R2.2 — once the repo name pushed x past (right_edge - icon), the branch is dropped,
        // not clipped: repo has priority.
        assert_eq!(line2_branch_origin(95.0, 10.0, 100.0), None);
    }

    #[test]
    fn card_height_is_independent_of_the_task_label() {
        // R2.3 — the task label is a horizontal, line-1 text swap; card height is governed by
        // `layout` (show_branch only), which takes no per-tab content, so a labelled card and a
        // plain card share the exact same stride/height. (Draw uses this same `layout`.)
        let g = geom(160.0, 600.0, true);
        let a = layout(&g, 5);
        let b = layout(&g, 5);
        assert_eq!(a.card_h, b.card_h);
        assert_eq!(a.stride, b.stride);
        // And the reserved height matches the two-line (title + branch) formula, not three.
        assert!(
            a.card_h > g.cell[1],
            "card reserves at least the two card lines"
        );
    }
}
