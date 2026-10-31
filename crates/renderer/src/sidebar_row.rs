//! The redesigned left sidebar: the weather cards (`sidebar-weather-cards` — title and
//! folder over status and branch, a pixel agent icon, per-agent skins, hover and
//! selected states), drawn on the hand-rolled wgpu renderer (SD1). This module holds the real implementation;
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

use crate::sidebar_card::{build_card, card_state, folder_text, CardInput, CardStatus, TextRun};
use crate::sidebar_icons::{push_icon, Icon};
use crate::sidebar_layout::OpenMenu;
use crate::sidebar_shapes::{Fill, SidebarShape, SidebarShapePipeline};
use crate::sidebar_text::{CardAtlases, CardFace, CardMeasure};
use crate::sidebar_theme::{linear, AgentSkin, CardMetrics, SidebarMetrics, SidebarTheme};
use crate::tab_bar::char_cols;
use crate::{DrawLayer, FrameCtx};

/// The z-order of the sidebar layer (above the terminal glyph pass = 0, below overlays).
pub const TAB_BAR_Z: i32 = 10;

// ---------------------------------------------------------------------------
// Plain-data row + metadata types (SD3 — the renderer never names app types)
// ---------------------------------------------------------------------------

/// One tab as the app hands it over (D2): a stable id so asynchronous metadata can be
/// joined to it by id (never by index, D10), plus what `Tabs` itself owns — its title
/// sources and its shell's command state. Branch / status / skin live in [`RowMeta`],
/// pulled separately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SidebarRow {
    /// The app's `session::TabId` as a plain `u64` — the renderer must not know app types.
    pub tab_id: u64,
    /// The cwd basename **as the app assembled the rows** — the card's pre-publish
    /// fallback, not its live folder name (card-live-cwd D2).
    ///
    /// The rows are rebuilt on tab actions, so this string does not follow a `cd`. The live
    /// directory arrives on the metadata pull as [`RowMeta::cwd_display`]; this field is
    /// what a card shows only while its tab is still unpolled, or when no [`MetaSource`]
    /// is installed.
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
    /// Whether the tab's shell is running a foreground command — OSC 133 `C` seen with no
    /// command end since (sidebar-weather-cards D8). UI-thread state from the `Session`,
    /// so it travels here, on the rows, the way `program` does: the metadata worker that
    /// fills [`RowMeta`] never sees PTY bytes.
    pub running: bool,
    /// The exit code of the tab's last **finished** foreground command, if one has
    /// finished (D8). Raw: whether it counts as a failure is the app's rule (D12).
    pub last_exit: Option<i32>,
    /// Whether the tab's live screen shows a dialog that blocks on the user, as the app
    /// last read it (agent-card-status D7). UI-thread state like [`running`]: it is read
    /// off the tab's own grid, which the metadata worker never sees. What it does to the
    /// card is the app's precedence (D8); this layer only carries it.
    ///
    /// [`running`]: SidebarRow::running
    pub blocking_dialog: bool,
    /// What the dialog asks for, when [`blocking_dialog`](Self::blocking_dialog) is up and
    /// the dialog's subject was read (session-overview-ui D9). Always `None` while
    /// `blocking_dialog` is false. The sidebar card does not draw it; the overview does.
    pub blocking_subject: Option<crate::overview_model::DialogSubject>,
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

/// Per-tab metadata pulled from the app each frame (D10). `default()` is a plain card —
/// no branch, the `zsh` status, [`AgentSkin::Generic`] — for a tab not yet polled.
#[derive(Clone, Debug, PartialEq)]
pub struct RowMeta {
    pub branch: Option<String>,
    /// The bottom-left status, **already resolved by the app** by the precedence agent >
    /// running > failed > shell (sidebar-weather-cards D7). The renderer only draws it.
    pub status: CardStatus,
    pub skin: AgentSkin,
    /// Line 1's text, **already resolved by the app** (sticky-tab-title D7/R1.5) by the
    /// precedence user-title > AI-task > program-OSC title > cwd. The renderer holds no
    /// part of that policy: it draws this string. Pulled by `tab_id` like `branch`, so it
    /// refreshes live off the metadata generation — which is why the app resolves it in
    /// its `MetaSource::fill` and not when it assembles the rows.
    ///
    /// Empty for a `default()` row; the layer falls back to the row's title then.
    pub line1: String,
    /// The folder line: the tab's **live** working directory with `$HOME` shown as `~`
    /// (sidebar-weather-cards D10), resolved by the app on this same pull.
    ///
    /// It travels here rather than on [`SidebarRow`] for the reason `line1` does: the rows
    /// are assembled on tab actions, while the cwd is republished whenever the shell moves
    /// (card-live-cwd D1). It comes from the same `TabMeta::cwd` as line 1's cwd tier and
    /// the same pull as the branch, so the card's folder and branch describe one directory.
    ///
    /// Empty for a `default()` row; the layer falls back to the row's title then.
    pub cwd_display: String,
}

impl Default for RowMeta {
    fn default() -> Self {
        RowMeta {
            branch: None,
            status: CardStatus::Shell,
            skin: AgentSkin::Generic,
            line1: String::new(),
            cwd_display: String::new(),
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
    /// The text typed so far, with any IME preedit already inserted by the app.
    pub text: String,
    /// The caret's position as a **char** index into `text` (`0..=text.chars().count()`).
    /// The renderer draws the caret here rather than at the end, which is what lets the
    /// middle of a name be edited.
    pub caret: usize,
    /// Whether the whole text is selected — the state a rename field opens in, so the
    /// existing name is visible but the next keystroke replaces it wholesale (the Finder
    /// rename convention). Drawn as a highlight behind the text.
    pub selected: bool,
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

/// The pure card layout (D6/D7). Since sidebar-weather-cards D1 the card height is
/// **fixed** — 104 logical px, scaled — so neither the terminal font (`geom.cell`) nor the
/// branch toggle (`geom.show_branch`) moves it any more: a toggle now empties its zone
/// instead (D13). Cards sit `inset_x` from each edge with a `gap` between them, split as
/// half above and half below each card so the stride bands still tile (D7).
pub fn layout(geom: &SidebarGeom, count: usize) -> SidebarLayout {
    let c = CardMetrics::DEFAULT.scaled(geom.scale);
    SidebarLayout {
        geom: geom.clone(),
        count,
        stride: c.height + c.gap,
        card_h: c.height,
        margin_x: c.inset_x,
        margin_y: c.gap * 0.5,
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

    /// Exact inverse: the card whose **stride band** contains `(x, y)` (D7 — the margins
    /// between cards are visual, so a click in a margin still selects its card). Horizontally
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
/// The second argument is the **row's** title (`SidebarRow::title`), deliberately NOT
/// [`RowMeta::cwd_display`]. They differ: this fallback fires exactly when `RowMeta` is
/// `default()`, so the live name is empty and the row's assembly-time copy is all there is.
/// Named `row_title` rather than `cwd_display` because passing the live field here — the
/// "obvious" consistency edit once both names exist — would blank line 1 on that very row.
pub(crate) fn card_line1_text<'a>(resolved: &'a str, row_title: &'a str) -> &'a str {
    if resolved.is_empty() {
        row_title
    } else {
        resolved
    }
}

/// The caret drawn at the cursor during an inline rename. A thin bar, so it reads as a
/// text cursor rather than as a character in the name.
pub(crate) const EDIT_CARET: char = '▏';

/// What to draw on line 1 while a card is being renamed (sticky-tab-title R2.1): the typed
/// text with the caret **at `caret`**, windowed so the caret is always on screen.
///
/// Editing happens in a card barely wider than a word, so a name longer than the card is
/// the normal case, not the edge case. Two things follow, and both differ from
/// [`truncate_to_width`]'s label behaviour:
///
/// - The caret is inserted at the cursor, not appended — otherwise the middle of a name
///   could be edited but never *seen* being edited.
/// - The visible window **follows the caret**. Anchoring on the tail would hide the cursor
///   the moment the user arrowed back into a long name, which is exactly when they need to
///   see it. An elision on either side is marked with `ellipsis`.
pub(crate) fn edit_line_text(
    text: &str,
    caret: usize,
    advance: f32,
    max_w: f32,
    ellipsis: char,
) -> String {
    if max_w <= 0.0 || advance <= 0.0 {
        return String::new();
    }
    // Budgeted in display columns, not chars (sidebar-weather-cards D3): a Hangul name is
    // two columns a syllable, and a char-counted window would overrun the field.
    let max_cols = (max_w / advance).floor() as usize;
    if max_cols == 0 {
        return String::new();
    }
    // The caret is a drawn glyph, so it occupies one of the visible columns.
    let mut chars: Vec<char> = text.chars().collect();
    let caret_at = caret.min(chars.len());
    chars.insert(caret_at, EDIT_CARET);
    let width = |cs: &[char]| cs.iter().map(|&c| char_cols(c)).sum::<usize>();
    if width(&chars) <= max_cols {
        return chars.into_iter().collect();
    }
    // A window of at most `max_cols`, centred on the caret so there is context on both
    // sides: about half the columns left of it, then fill rightwards, then — at the end
    // of the string — back-fill leftwards. For one-column text this is exactly the
    // `max_chars` window centred on the caret and clamped to the ends.
    let mut start = caret_at;
    let mut used = char_cols(EDIT_CARET);
    while start > 0 && used + char_cols(chars[start - 1]) <= max_cols / 2 + 1 {
        start -= 1;
        used += char_cols(chars[start]);
    }
    let mut end = caret_at + 1;
    while end < chars.len() && used + char_cols(chars[end]) <= max_cols {
        used += char_cols(chars[end]);
        end += 1;
    }
    while start > 0 && used + char_cols(chars[start - 1]) <= max_cols {
        start -= 1;
        used += char_cols(chars[start]);
    }
    let mut window: Vec<char> = chars[start..end].to_vec();
    // Mark each elided side by REPLACING that edge glyph — never by adding one, which
    // would push the window over the card's width (an ellipsis is never wider than the
    // glyph it replaces). The caret is never overwritten: it is the one glyph that must
    // survive, since a field whose cursor is off-screen is exactly the failure this
    // windowing exists to prevent.
    if start > 0 && window[0] != EDIT_CARET {
        window[0] = ellipsis;
    }
    if end < chars.len() {
        let last = window.len() - 1;
        if window[last] != EDIT_CARET {
            window[last] = ellipsis;
        }
    }
    window.into_iter().collect()
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

    /// Set one row's [`SidebarRow::blocking_dialog`] in place, answering whether it changed;
    /// `rows_rev` is bumped only then (agent-card-status D7). The incremental path for a
    /// dialog appearing or clearing: it changes one card's status and nothing else about the
    /// rows, and it can fire on a metadata publication — where rebuilding the rows through
    /// `Tabs::titles` would also re-send the roster to the metadata worker, which then
    /// re-derives and may publish again. An unknown `tab_id` (a tab closed between the scan
    /// and this) changes nothing.
    ///
    /// The dialog's subject (session-overview-ui D9) travels with the level through this one
    /// setter so the two cannot disagree: a false level stores no subject, and a new subject
    /// at an unchanged level is a change (D18), so an open overview rebuilds.
    pub fn set_blocking_dialog(
        &self,
        tab_id: u64,
        level: bool,
        subject: Option<crate::overview_model::DialogSubject>,
    ) -> bool {
        let subject = subject.filter(|_| level);
        let mut m = self.0.borrow_mut();
        let Some(row) = m.rows.iter_mut().find(|r| r.tab_id == tab_id) else {
            return false;
        };
        if row.blocking_dialog == level && row.blocking_subject == subject {
            return false;
        }
        row.blocking_dialog = level;
        row.blocking_subject = subject;
        m.rows_rev = m.rows_rev.wrapping_add(1);
        true
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
// The text pipeline (D9) — a resident-buffer glyph pass over the shared atlas
// ---------------------------------------------------------------------------

/// A textured glyph quad in physical px sampling the atlas coverage.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct TextInstance {
    pub(crate) rect: [f32; 4],
    pub(crate) uv0: [f32; 2],
    pub(crate) uv1: [f32; 2],
    pub(crate) color: [f32; 4],
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

/// Which atlas a text batch samples: the grid's (band, popup, fallback) or one of the
/// sidebar's own card atlases (sidebar-weather-cards D2).
#[derive(Clone, Copy)]
pub(crate) enum TextAtlas<'b> {
    Grid,
    Card(&'b crate::atlas::Atlas),
}

/// The glyph pass. Mirrors foundations' shape-pipeline API; instances ride a resident
/// buffer (`upload_resident`, never `create_buffer_init` per frame — G1). The bind group
/// is rebuilt on the key `(format, atlas.epoch())` so a live `apply_config` atlas swap
/// cannot leave it sampling the replaced texture (atlas-swap-bind-invalidation D2/D4).
pub(crate) struct SidebarTextPipeline {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    quad: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind: Option<wgpu::BindGroup>,
    bind_key: Option<(wgpu::TextureFormat, u64)>,
    /// How many times the bind group has actually been (re)built.
    ///
    /// Key equality alone cannot express R2.3's real claim — *"rebuilds = 0 while the
    /// atlas is unchanged"*. A test that only compares keys still passes when the
    /// short-circuit below is deleted and the bind group is rebuilt every single frame,
    /// which is exactly the G1 idle-CPU regression this spec forbids. Counting the
    /// assignments is what gives that test teeth. `#[cfg(test)]`, so it costs the
    /// shipped binary nothing.
    #[cfg(test)]
    rebuilds: u32,
    /// How many times `draw` wrote the instance buffer — a test hook (R17.1).
    #[cfg(test)]
    pub(crate) uploads: u32,
    instances: Option<wgpu::Buffer>,
}

impl SidebarTextPipeline {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> SidebarTextPipeline {
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
            #[cfg(test)]
            rebuilds: 0,
            #[cfg(test)]
            uploads: 0,
            instances: None,
        }
    }

    pub(crate) fn ensure<'a>(
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

    pub(crate) fn draw(
        &mut self,
        ctx: &mut FrameCtx<'_>,
        source: TextAtlas<'_>,
        text: &[TextInstance],
    ) {
        if text.is_empty() {
            return;
        }
        // Bind key (atlas-swap-bind-invalidation D2/R2.1). The key must mirror what the
        // bind group actually holds — `{uniform, atlas.view(), atlas.sampler()}` — and
        // nothing else, because a key that moves when the contents don't means a rebuild
        // every frame: the G1 idle-CPU debt this project already paid off once.
        //
        // * `epoch` is the load-bearing field: it is the identity of the *texture*, so it
        //   is the only signal that `Renderer::apply_config` swapped the whole atlas out
        //   from under us. Without it the cached bind group keeps sampling the replaced
        //   texture while UVs are computed against the new one — wrong glyphs, silently,
        //   for the rest of the session.
        // * `generation` is deliberately NOT here. It counts evictions *within* one
        //   texture; eviction reuses the same texture, view and sampler, so rebuilding on
        //   a bump would produce a byte-identical bind group. The UV invalidation it
        //   really signals is repaired by re-issuing instances: every atlas's generation
        //   is in `record`'s compose key (sidebar-weather-cards D20), so an eviction makes
        //   the next frame compose and resolve its text again (D5).
        // * `cell` is deliberately NOT here either. `TextUniforms` is `{viewport, _pad}`,
        //   so the cell size never enters the bind group at all.
        // * `format` is retained, but be honest about its status: it is structurally
        //   redundant today. A format change makes `ensure` replace this whole struct via
        //   `SidebarTextPipeline::new`, which starts at `bind: None, bind_key: None`, so
        //   the comparison can never fire. It is kept purely as defence in case this cache
        //   ever moves outside the format-gated struct — not because it guards anything
        //   reachable now.
        //
        // Since sidebar-weather-cards a pipeline samples one of three atlases — the grid's
        // or either card atlas (D2) — so the texture and its epoch come from `source`. The
        // key logic above holds for each: one pipeline per atlas, keyed on that atlas's
        // own epoch (D18).
        let (view, sampler, epoch) = match source {
            TextAtlas::Grid => (ctx.atlas.view(), ctx.atlas.sampler(), ctx.atlas.epoch()),
            TextAtlas::Card(a) => (a.view(), a.sampler(), a.epoch()),
        };
        let key = (ctx.format, epoch);
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
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            }));
            self.bind_key = Some(key);
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
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
        #[cfg(test)]
        {
            self.uploads += 1;
        }
        self.pass(ctx, count);
    }

    /// Draw the first `count` instances the last [`draw`](SidebarTextPipeline::draw)
    /// uploaded, **writing no buffer** and keeping the bind group it built — for a frame
    /// whose text and atlases are unchanged since then (sidebar-weather-cards D20).
    pub(crate) fn redraw(&mut self, ctx: &mut FrameCtx<'_>, count: u32) {
        self.pass(ctx, count);
    }

    /// Record the render pass over the resident instances with the cached bind group.
    fn pass(&self, ctx: &mut FrameCtx<'_>, count: u32) {
        if count == 0 {
            return;
        }
        let (Some(instances), Some(bind)) = (self.instances.as_ref(), self.bind.as_ref()) else {
            return;
        };
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

/// Lay out `text` as grid-atlas glyph instances from `x` at `top` (physical px), scaled
/// by `scale` (1.0 for a band row, `meta_ratio` for its "+N more" summary, D5).
/// Truncated to `max_w` with `ellipsis` (D8).
///
/// Since `sidebar-weather-cards` only the waiting band and the card-menu popup draw
/// through here; card text is resolved from the card atlases by [`push_run`]. A `bold`
/// request is served from the font's real bold face (`specs/real-bold-faces`), with
/// synthesis only the fallback for a font that ships none (R5.1/D9).
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
        // fallback face instead of tofu. The style goes **into** `resolve`, not onto the
        // key afterwards (D3/R3.5): the id it returns belongs to whichever face actually
        // supplies the outline, and stamping a style onto an id resolved from a different
        // face is precisely the collision D15 describes. `resolve` returns an owned key,
        // so the immutable borrow is released before `get_or_insert_styled` takes the
        // mutable one.
        let entry = ctx
            .atlas
            .resolve(c, bold, false)
            .and_then(|key| ctx.atlas.get_or_insert_styled(ctx.queue, key));
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

/// Resolve one card text run (sidebar-weather-cards D2/D3) into glyph instances from
/// `atlas`, laid out by display column: `col` px per column, the line box's baseline at
/// the same `0.8 × line` the other sidebar text uses. The atlas is rasterized at the
/// run's own size, so glyphs are placed at scale 1 — never stretched.
fn push_run(
    out: &mut Vec<TextInstance>,
    atlas: &mut crate::atlas::Atlas,
    queue: &wgpu::Queue,
    run: &TextRun,
    col: f32,
    line: f32,
) {
    let baseline = run.top + line * 0.8;
    crate::tab_bar::layout_line_in(atlas, queue, &run.text, run.x, col, run.bold, |pen, e| {
        out.push(TextInstance {
            rect: [pen + e.left, baseline - e.top, e.px_w, e.px_h],
            uv0: e.uv0,
            uv1: e.uv1,
            color: run.color,
        });
    });
}

/// Where [`resolve_card_runs`] writes: one buffer per card face, plus the grid-atlas
/// fallback for a face whose own atlas could not be built (R2.6).
pub(crate) struct CardGlyphs<'a> {
    pub(crate) title: &'a mut Vec<TextInstance>,
    pub(crate) meta: &'a mut Vec<TextInstance>,
    pub(crate) fallback: &'a mut Vec<TextInstance>,
}

/// Resolve `build_card`'s text runs to glyph instances — the one copy of this step, shared
/// by the sidebar and the session graph (session-flow-graph D14). Returns how many passes
/// it took.
///
/// Each run goes to its face's atlas when that atlas is built, else to `grid` (R2.6, the
/// same `get` that [`CardAtlases::measure`] reads). Guarded against an eviction in the
/// middle of it (D18/R2.5): `pack` evicts when a card atlas fills and invalidates the UVs
/// already resolved this frame, so a generation that moved during the pass means the runs
/// are resolved once more, against the repacked texture, before anything is submitted.
/// `after_run` is called after every run — a test seam for driving that eviction.
pub(crate) fn resolve_card_runs(
    cards: &mut CardAtlases,
    grid: &mut crate::atlas::Atlas,
    queue: &wgpu::Queue,
    runs: &[TextRun],
    measure: CardMeasure,
    out: CardGlyphs<'_>,
    mut after_run: impl FnMut(&mut CardAtlases),
) -> u8 {
    let mut passes = 0;
    for _pass in 0..2 {
        out.title.clear();
        out.meta.clear();
        out.fallback.clear();
        let before = CardFace::ALL.map(|f| cards.generation(f));
        for run in runs {
            let (col, line) = measure.of(run.face);
            let buf = match run.face {
                CardFace::Title => &mut *out.title,
                CardFace::Meta => &mut *out.meta,
            };
            match cards.get_mut(run.face) {
                Some(atlas) => push_run(buf, atlas, queue, run, col, line),
                None => push_run(out.fallback, grid, queue, run, col, line),
            }
            after_run(cards);
        }
        passes += 1;
        if CardFace::ALL.map(|f| cards.generation(f)) == before {
            break;
        }
    }
    passes
}

// ---------------------------------------------------------------------------
// The layer (T3 text + T4 chrome)
// ---------------------------------------------------------------------------

/// Everything a composed sidebar frame depends on (sidebar-weather-cards D20). Equal
/// keys mean the shapes and glyph instances last uploaded are exactly what this frame
/// would compose, so the layer re-submits them instead.
///
/// Split in two so a steady frame can be recognised without allocating: the `Copy`
/// [`KeyScalars`] are compared by value, and the model's band and popup state are
/// compared in place, under a borrow, against the copies kept here — which are cloned
/// only when a frame is composed.
#[derive(Clone, Debug, PartialEq)]
struct ComposeKey {
    scalars: KeyScalars,
    menu: Option<CardMenu>,
    edit: Option<CardEdit>,
    waiting: Vec<WaitingRow>,
}

/// The `Copy` part of a [`ComposeKey`].
///
/// The atlases are keyed by `(epoch, generation)`: a new texture needs a new bind group,
/// and an eviction repacks it under UVs already resolved. The **grid** atlas is in here
/// too, because the band's and the popup's text sample it and it evicts on terminal
/// output, not sidebar state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyScalars {
    rows_rev: u64,
    meta_gen: u64,
    /// The installed `MetaSource`'s identity: a new source may restart its generations.
    source: usize,
    active: usize,
    hovered: Option<usize>,
    list_top: f32,
    scroll_px: f32,
    sidebar_w: f32,
    viewport: [f32; 2],
    scale: f64,
    cell: [f32; 2],
    format: wgpu::TextureFormat,
    show_branch: bool,
    show_agent: bool,
    accent: u32,
    /// `[grid, title, meta]` — `(epoch, generation)`, `None` for an unbuilt card atlas.
    atlases: [Option<(u64, u64)>; 3],
}

/// The sidebar draw layer (D9). Owns six passes — two of foundations' shape pipeline
/// (the panel/band/cards, then the popup over them) and four of the local text pipeline
/// (card titles, card meta, fallback card text, then the grid-atlas band and popup text)
/// — plus reused instance + metadata buffers so a steady frame allocates nothing
/// (SD4/D10, sidebar-weather-cards D20).
pub struct TabBar {
    model: TabModelHandle,
    /// Panel, waiting band and the cards' shapes.
    shape_pipe: Option<SidebarShapePipeline>,
    /// The open card menu's shapes — a pass of its own, submitted **after** the card text,
    /// so the popup covers the cards' titles rather than showing them through it.
    popup_pipe: Option<SidebarShapePipeline>,
    /// Text drawn from the grid atlas over everything else: the waiting band and the
    /// popup's items.
    text_pipe: Option<SidebarTextPipeline>,
    /// Card text whose own atlas could not be built, from the grid atlas at cell size
    /// (sidebar-weather-cards R2.6). A pass of its own, submitted with the card text and
    /// **before** the popup, so a degraded card title cannot draw over an open menu.
    fallback_pipe: Option<SidebarTextPipeline>,
    /// Card titles, from the 17px title atlas (D2).
    title_pipe: Option<SidebarTextPipeline>,
    /// Card folder / status / branch text, from the 12.5px meta atlas (D2).
    meta_pipe: Option<SidebarTextPipeline>,
    /// The sidebar's own glyph atlases for card text (D2/D18).
    cards: CardAtlases,
    shape_buf: Vec<SidebarShape>,
    popup_buf: Vec<SidebarShape>,
    text_buf: Vec<TextInstance>,
    title_buf: Vec<TextInstance>,
    meta_buf: Vec<TextInstance>,
    fallback_buf: Vec<TextInstance>,
    run_buf: Vec<TextRun>,
    /// Pulled from the app via `MetaSource`, refreshed only on a generation change (D10).
    meta: Vec<RowMeta>,
    meta_gen: u64,
    rows_rev_seen: u64,
    /// How many passes the last frame's card-text resolution took: 1 normally, 2 when an
    /// atlas evicted mid-frame and the runs were re-resolved (D18/R2.5). A test hook.
    #[cfg(test)]
    resolve_passes: u8,
    /// Test-only: evict this card atlas right after the first run is resolved, to drive
    /// the mid-frame eviction path deterministically (R2.5).
    #[cfg(test)]
    evict_during_resolve: Option<CardFace>,
    /// Test-only: evict the **grid** atlas once composing has resolved text against it,
    /// to drive the "composing evicted the grid atlas" path (D20).
    #[cfg(test)]
    evict_grid_during_compose: bool,
    /// What the last composed frame depended on (D20); `None` before the first.
    compose_key: Option<ComposeKey>,
    /// How many frames were composed rather than re-submitted — a test hook (R17).
    #[cfg(test)]
    compose_count: u32,
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
                popup_pipe: None,
                text_pipe: None,
                fallback_pipe: None,
                title_pipe: None,
                meta_pipe: None,
                cards: CardAtlases::new(),
                shape_buf: Vec::new(),
                popup_buf: Vec::new(),
                text_buf: Vec::new(),
                title_buf: Vec::new(),
                meta_buf: Vec::new(),
                fallback_buf: Vec::new(),
                run_buf: Vec::new(),
                meta: Vec::new(),
                meta_gen: 0,
                rows_rev_seen: u64::MAX,
                #[cfg(test)]
                resolve_passes: 0,
                #[cfg(test)]
                evict_during_resolve: None,
                #[cfg(test)]
                evict_grid_during_compose: false,
                compose_key: None,
                #[cfg(test)]
                compose_count: 0,
            },
            model,
        )
    }

    /// Refresh `self.meta` from the pulled source, gated on the source generation and the
    /// row revision (D10). The source is called AFTER the model borrow is released (D3's
    /// reentrancy rule), so a `MetaSource` reaching `shared()` cannot double-borrow.
    fn refresh_meta(&mut self, rows_rev: u64, source: &Option<Rc<dyn MetaSource>>) {
        match source {
            Some(src) => {
                let gen = src.generation();
                if gen != self.meta_gen || rows_rev != self.rows_rev_seen {
                    // Cloned only on a change (a steady frame allocates nothing), and the
                    // borrow is released before `fill` (D3's reentrancy rule).
                    let rows = self.model.with(|m| m.rows.clone());
                    src.fill(&rows, &mut self.meta);
                    self.meta_gen = gen;
                    self.rows_rev_seen = rows_rev;
                }
            }
            None => {
                // No source (renderer tests, pre-install window): plain default cards. A
                // card with no entry draws `RowMeta::default()`, so there is nothing to
                // keep — only to drop, should a source's rows ever outlive it.
                self.meta.clear();
                self.rows_rev_seen = rows_rev;
            }
        }
    }
}

impl TabBar {
    /// `(epoch, generation)` of the grid atlas and both card atlases, for [`ComposeKey`].
    fn atlas_state(&self, ctx: &FrameCtx<'_>) -> [Option<(u64, u64)>; 3] {
        let card = |face| {
            self.cards
                .get(face)
                .map(|a: &crate::atlas::Atlas| (a.epoch(), a.generation()))
        };
        [
            Some((ctx.atlas.epoch(), ctx.atlas.generation())),
            card(CardFace::Title),
            card(CardFace::Meta),
        ]
    }

    /// Re-submit last frame's resident instances in the same order `record` draws them,
    /// writing no buffer (D20). The kept `*_buf` vectors are what those uploads held, so
    /// their lengths are the counts.
    fn redraw_cached(&mut self, ctx: &mut FrameCtx<'_>) {
        if let Some(p) = self.shape_pipe.as_mut() {
            p.redraw(ctx, self.shape_buf.len() as u32);
        }
        if self.cards.get(CardFace::Title).is_some() {
            if let Some(p) = self.title_pipe.as_mut() {
                p.redraw(ctx, self.title_buf.len() as u32);
            }
        }
        if self.cards.get(CardFace::Meta).is_some() {
            if let Some(p) = self.meta_pipe.as_mut() {
                p.redraw(ctx, self.meta_buf.len() as u32);
            }
        }
        if let Some(p) = self.fallback_pipe.as_mut() {
            p.redraw(ctx, self.fallback_buf.len() as u32);
        }
        if let Some(p) = self.popup_pipe.as_mut() {
            p.redraw(ctx, self.popup_buf.len() as u32);
        }
        if let Some(p) = self.text_pipe.as_mut() {
            p.redraw(ctx, self.text_buf.len() as u32);
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

        // Copy the model's scalars out under ONE borrow, then release it (D3) before
        // touching the (possibly reentrant) metadata source. The rows and the band and
        // popup state are not copied yet: a steady frame only compares them in place, and
        // they are cloned below only for a frame that is composed (D20, "no per-frame
        // allocation in a steady frame").
        let (active, hovered, rows_rev, list_top, scroll_px, n_rows, source) =
            self.model.with(|m| {
                (
                    m.active,
                    m.hovered,
                    m.rows_rev,
                    m.list_top,
                    m.scroll_px,
                    m.rows.len(),
                    m.meta_source.clone(),
                )
            });
        self.refresh_meta(rows_rev, &source);

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
        let lay = layout(&geom, n_rows);
        let vis = lay.visible_range();

        // ---- Compose only on change (sidebar-weather-cards D20). The window redraws at
        // 60 fps while visible (docs/architecture.md C6); a steady sidebar re-submits last
        // frame's resident instances rather than composing, resolving and uploading them
        // all again. The card atlases are made current first, since a rebuild is a change.
        self.cards.ensure(ctx.device, ctx.fonts, ctx.scale_factor);
        let scalars = KeyScalars {
            rows_rev,
            meta_gen: self.meta_gen,
            source: source
                .as_ref()
                .map_or(0, |s| Rc::as_ptr(s) as *const () as usize),
            active,
            hovered,
            list_top,
            scroll_px,
            sidebar_w,
            viewport: ctx.viewport,
            scale: ctx.scale_factor,
            cell,
            format: ctx.format,
            show_branch,
            show_agent,
            accent: ctx.sidebar.accent,
            atlases: self.atlas_state(ctx),
        };
        let unchanged = self.compose_key.as_ref().is_some_and(|prev| {
            prev.scalars == scalars
                && self.model.with(|m| {
                    prev.waiting == m.waiting && prev.menu == m.menu && prev.edit == m.edit
                })
        });
        if unchanged {
            self.redraw_cached(ctx);
            self.model.with_mut(|mm| mm.layout = Some(lay));
            return;
        }
        #[cfg(test)]
        {
            self.compose_count += 1;
        }
        let (rows, waiting, menu, edit) = self.model.with(|m| {
            (
                m.rows.clone(),
                m.waiting.clone(),
                m.menu.clone(),
                m.edit.clone(),
            )
        });
        let mut key = ComposeKey {
            scalars,
            menu,
            edit,
            waiting,
        };
        let (waiting, menu, edit) = (&key.waiting, key.menu.as_ref(), key.edit.as_ref());

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

        // ---- Cards (sidebar-weather-cards). Each is composed by the pure builder
        // (`sidebar_card::build_card`, D16); this layer only resolves the text runs it
        // returns through the matching atlas, and submits.
        let scale = ctx.scale_factor;
        // A face whose card atlas could not be built draws from the grid atlas at cell
        // size instead (R2.6) — so it also measures by the grid cell.
        let card_measure = self.cards.measure(scale, cell);
        let CardMeasure {
            title_col,
            title_line,
            meta_col,
            meta_line,
        } = card_measure;
        let menu_row = menu.as_ref().map(|open| open.row);
        let default_meta = RowMeta::default();
        let mut runs = std::mem::take(&mut self.run_buf);
        runs.clear();
        let mut edit_zone: Option<[f32; 4]> = None;
        for i in vis {
            let Some(rect) = lay.card_rect(i) else {
                continue;
            };
            let meta = self.meta.get(i).unwrap_or(&default_meta);
            // The rows were read under a later borrow than `n_rows`; a reentrant source
            // that changed them since is caught by `rows_rev` next frame, not by a panic.
            let Some(row) = rows.get(i) else {
                continue;
            };
            // Joined by `tab_id`, not index, so a roster shift cannot move the editor.
            let editing = edit.as_ref().is_some_and(|e| e.tab_id == row.tab_id);
            let hovered_here = hovered == Some(i);
            let card = build_card(&CardInput {
                rect,
                skin: meta.skin,
                state: card_state(i == active, hovered_here),
                status: meta.status,
                // Line 1 is resolved app-side (sticky-tab-title D7); an empty one is a row
                // with no metadata yet, which shows its folder name.
                title: card_line1_text(&meta.line1, &row.title),
                folder: folder_text(&meta.cwd_display, &row.title),
                branch: meta.branch.as_deref(),
                show_branch,
                show_status: show_agent,
                // D11: the pointer is over it, its menu is open, or it is being renamed —
                // an open popup or field is never left without its anchor.
                show_menu: hovered_here || menu_row == Some(i) || editing,
                editing,
                scale,
                title_col,
                title_line,
                meta_col,
                meta_line,
                ellipsis,
            });
            shapes.extend_from_slice(&card.shapes);
            runs.extend(card.runs);
            if editing {
                edit_zone = Some(card.title_zone);
            }
        }

        // ---- The rename field (sticky-tab-title R2.1) over the title line, at the
        // title's size (D15): a field box ending exactly at the title budget, so it
        // always stops short of the `≡` (R14.2), and the text windowed by title columns.
        if let (Some(e), Some(zone)) = (edit.as_ref(), edit_zone) {
            let c = CardMetrics::DEFAULT.scaled(scale);
            let pad = c.title_gap + c.border;
            if let Some(field) = SidebarShape::rounded_box(
                [
                    zone[0] - pad,
                    zone[1] - pad * 0.5,
                    zone[2] + pad,
                    zone[3] + pad,
                ],
                c.radius * 0.5,
                c.border,
                Fill::solid(linear(theme.panel_bg, 1.0)),
                linear(theme.card_border_active, 1.0),
            ) {
                shapes.push(field);
            }
            let line = edit_line_text(&e.text, e.caret, title_col, zone[2], ellipsis);
            // Select-all highlight (the state the field opens in): behind the text but
            // not behind the trailing caret, so the caret stays legible.
            if e.selected {
                let text_cols = line
                    .chars()
                    .filter(|&c| c != EDIT_CARET)
                    .map(char_cols)
                    .sum::<usize>();
                // An empty name has nothing to highlight: its box is zero-wide, which
                // `rounded_box` declines.
                if let Some(hl) = SidebarShape::rounded_box(
                    [zone[0], zone[1], text_cols as f32 * title_col, zone[3]],
                    c.border,
                    0.0,
                    Fill::solid(linear(theme.accent, 0.40)),
                    [0.0; 4],
                ) {
                    shapes.push(hl);
                }
            }
            runs.push(TextRun {
                face: CardFace::Title,
                text: line,
                x: zone[0],
                top: zone[1],
                color: linear(theme.card_title_fg, 1.0),
                bold: true,
            });
        }

        // ---- Resolve the card runs to glyph instances (the step shared with the session
        // graph, `resolve_card_runs`, which also carries the mid-frame eviction guard).
        let mut title_text = std::mem::take(&mut self.title_buf);
        let mut meta_text = std::mem::take(&mut self.meta_buf);
        let mut fallback = std::mem::take(&mut self.fallback_buf);
        #[cfg(test)]
        let mut evict_hook = self.evict_during_resolve.take();
        let _passes = resolve_card_runs(
            &mut self.cards,
            ctx.atlas,
            ctx.queue,
            &runs,
            card_measure,
            CardGlyphs {
                title: &mut title_text,
                meta: &mut meta_text,
                fallback: &mut fallback,
            },
            |_cards| {
                #[cfg(test)]
                if let Some(face) = evict_hook.take() {
                    if let Some(atlas) = _cards.get_mut(face) {
                        atlas.evict_for_test();
                    }
                }
            },
        );
        #[cfg(test)]
        {
            self.resolve_passes = _passes;
        }
        #[cfg(test)]
        if std::mem::take(&mut self.evict_grid_during_compose) {
            ctx.atlas.evict_for_test();
        }

        // ---- The open card menu (sticky-tab-title D4/D10/R3.1), in a shape pass of its
        // own submitted after the card text, so it layers over the cards it floats above.
        // Its rect comes from the same `menu_rect_for` formula the hit-test uses, fed the
        // same button rect, so what the user sees and what a press resolves to cannot
        // disagree (SD2).
        let mut popup_shapes = std::mem::take(&mut self.popup_buf);
        popup_shapes.clear();
        if let Some(menu) = menu {
            if let Some(card) = lay.card_rect(menu.row) {
                let button = crate::sidebar_layout::menu_button_in(
                    crate::sidebar_layout::Rect {
                        x: card[0],
                        y: card[1],
                        w: card[2],
                        h: card[3],
                    },
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
                    popup_shapes.push(bg);
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

        // Submit: cards' shapes, their titles, their meta text, any card text that fell
        // back to the grid atlas (R2.6), then the popup over all of it, then the grid-atlas
        // text that belongs on top (the band's rows, the popup's items).
        {
            let sp = SidebarShapePipeline::ensure(&mut self.shape_pipe, ctx);
            sp.draw(ctx, &shapes);
        }
        if let Some(atlas) = self.cards.get(CardFace::Title) {
            let tp = SidebarTextPipeline::ensure(&mut self.title_pipe, ctx);
            tp.draw(ctx, TextAtlas::Card(atlas), &title_text);
        }
        if let Some(atlas) = self.cards.get(CardFace::Meta) {
            let mp = SidebarTextPipeline::ensure(&mut self.meta_pipe, ctx);
            mp.draw(ctx, TextAtlas::Card(atlas), &meta_text);
        }
        if !fallback.is_empty() {
            let fp = SidebarTextPipeline::ensure(&mut self.fallback_pipe, ctx);
            fp.draw(ctx, TextAtlas::Grid, &fallback);
        }
        if !popup_shapes.is_empty() {
            let pp = SidebarShapePipeline::ensure(&mut self.popup_pipe, ctx);
            pp.draw(ctx, &popup_shapes);
        }
        {
            let tp = SidebarTextPipeline::ensure(&mut self.text_pipe, ctx);
            tp.draw(ctx, TextAtlas::Grid, &text);
        }
        self.title_buf = title_text;
        self.meta_buf = meta_text;
        self.fallback_buf = fallback;
        self.popup_buf = popup_shapes;
        self.run_buf = runs;

        // What this frame depended on (D20). The card atlases as composing left them — an
        // eviction there mid-frame was already resolved again (D18). The grid atlas as it
        // was *before* composing: nothing re-resolves the band's, popup's and fallback
        // text, so if composing evicted it, the next frame must miss and compose again
        // against the repacked texture rather than re-submit stale UVs indefinitely.
        let after = self.atlas_state(ctx);
        key.scalars.atlases[1] = after[1];
        key.scalars.atlases[2] = after[2];
        self.compose_key = Some(key);

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
    fn a_dialog_level_is_set_on_its_row_in_place() {
        // agent-card-status D7 — one row's level changes, the others and the rest of the
        // model do not, and the row-change gate fires only on an actual change.
        reset_shared_model();
        let h = TabModelHandle::shared();
        h.set(TabBarModel {
            titles: vec![
                SidebarRow {
                    tab_id: 7,
                    title: "a".into(),
                    ..Default::default()
                },
                SidebarRow {
                    tab_id: 8,
                    title: "b".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let rev = || h.with(|m| m.rows_rev);
        let levels = || h.with(|m| m.rows.iter().map(|r| r.blocking_dialog).collect::<Vec<_>>());
        let start = rev();

        assert!(h.set_blocking_dialog(8, true, None), "rising");
        assert_eq!(levels(), vec![false, true]);
        assert_eq!(rev(), start + 1);

        assert!(
            !h.set_blocking_dialog(8, true, None),
            "a repeat is not a change"
        );
        assert_eq!(rev(), start + 1, "and bumps nothing");

        assert!(!h.set_blocking_dialog(99, true, None), "a closed tab");
        assert_eq!((levels(), rev()), (vec![false, true], start + 1));

        assert!(h.set_blocking_dialog(8, false, None), "falling");
        assert_eq!((levels(), rev()), (vec![false, false], start + 2));
    }

    #[test]
    fn the_dialog_subject_rides_with_the_level() {
        // session-overview-ui D9 / D18 — a subject change at an unchanged level is a change;
        // a false level carries no subject whatever it is handed.
        use crate::overview_model::DialogSubject::{Command, Plan};
        reset_shared_model();
        let h = TabModelHandle::shared();
        h.set(TabBarModel {
            titles: vec![SidebarRow {
                tab_id: 8,
                ..Default::default()
            }],
            ..Default::default()
        });
        let rev = || h.with(|m| m.rows_rev);
        let row = || {
            h.with(|m| {
                let r = &m.rows[0];
                (r.blocking_dialog, r.blocking_subject.clone())
            })
        };
        let start = rev();

        assert!(h.set_blocking_dialog(8, true, Some(Command("ls".into()))));
        assert_eq!(row(), (true, Some(Command("ls".into()))));
        assert!(!h.set_blocking_dialog(8, true, Some(Command("ls".into()))));
        assert_eq!(rev(), start + 1);

        assert!(
            h.set_blocking_dialog(8, true, Some(Plan)),
            "new subject, same level"
        );
        assert_eq!((row(), rev()), ((true, Some(Plan)), start + 2));
        assert!(h.set_blocking_dialog(8, true, None), "subject lost");
        assert_eq!(row(), (true, None));

        assert!(h.set_blocking_dialog(8, false, Some(Plan)));
        assert_eq!(row(), (false, None), "no level, no subject");
        assert!(!h.set_blocking_dialog(8, false, None));
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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

    fn geom2(sidebar_w: f32, cell: [f32; 2], show_branch: bool) -> SidebarGeom {
        SidebarGeom {
            sidebar_w,
            viewport_h: 4000.0,
            cell,
            scale: 2.0,
            show_branch,
            list_top: 0.0,
            scroll_px: 0.0,
        }
    }

    #[test]
    fn card_height_is_fixed_regardless_of_font_and_toggles() {
        // sidebar-weather-cards R1.1 — 104 logical at 2× for fonts of 10, 14 and 24pt and
        // either branch toggle: the height no longer follows the cell or the config (D1).
        for pt in [10.0_f32, 14.0, 24.0] {
            let cell = [(pt * 2.0 * 0.6).round(), (pt * 2.0 * 1.2).round()];
            for show_branch in [true, false] {
                let l = layout(&geom2(680.0, cell, show_branch), 3);
                assert_eq!(l.card_h, 208.0, "{pt}pt, branch {show_branch}");
                assert_eq!(l.stride, 228.0, "104 + the 10 gap, at 2×");
            }
        }
    }

    #[test]
    fn cards_are_inset_14_and_spaced_10() {
        // R1.2 — a 340-logical sidebar at 2×.
        let l = layout(&geom2(680.0, [17.0, 34.0], true), 3);
        let (a, b) = (l.card_rect(0).expect("0"), l.card_rect(1).expect("1"));
        assert_eq!((a[0], a[2]), (28.0, 624.0));
        assert_eq!(b[1] - a[1], (104.0 + 10.0) * 2.0);
        assert_eq!(b[1] - (a[1] + a[3]), 20.0, "a 10-logical gap between cards");
    }

    /// The hit-test's input for the same sidebar the row layer laid out.
    fn hit_input(l: &SidebarLayout) -> crate::sidebar_layout::SidebarLayoutInput {
        crate::sidebar_layout::SidebarLayoutInput {
            sidebar_w: l.geom.sidebar_w,
            viewport: [l.geom.sidebar_w + 800.0, l.geom.viewport_h],
            scale: l.geom.scale,
            row_h: l.card_h,
            row_count: l.count,
            waiting_count: 0,
            metrics: SidebarMetrics::DEFAULT.scaled(l.geom.scale),
            open_menu: None,
        }
    }

    #[test]
    fn the_hit_test_reads_the_drawn_geometry() {
        // R1.3 / R11.4 — the app's hit-test (`sidebar_layout`) and the row layer agree on
        // every card rect and every `≡` rect, and a point in the gap under a card belongs
        // to its stride band.
        let mut g = geom2(680.0, [17.0, 34.0], true);
        let probe = hit_input(&layout(&g, 4));
        g.list_top = crate::sidebar_layout::list_viewport(&probe).y;
        let l = layout(&g, 4);
        let inp = hit_input(&l);
        for i in 0..4 {
            let drawn = l.card_rect(i).expect("in range");
            let hit = crate::sidebar_layout::row_rect(i, 0.0, &inp);
            assert_eq!([hit.x, hit.y, hit.w, hit.h], drawn, "card {i}");
            let plate = crate::sidebar_card::menu_button(drawn, 2.0);
            let b = crate::sidebar_layout::menu_button_rect(i, 0.0, &inp);
            assert_eq!([b.x, b.y, b.w, b.h], plate, "button {i}");
        }
        let a = l.card_rect(0).expect("0");
        let gap = a[1] + a[3] + 1.0;
        assert_eq!(
            l.card_at(300.0, gap),
            Some(0),
            "the gap below a card is its own"
        );
        let b = l.card_rect(1).expect("1");
        assert_eq!(
            l.card_at(300.0, b[1] - 1.0),
            Some(1),
            "the gap above a card too"
        );
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
        // 200px viewport / 114px stride (104 + 10 at 1×) ⇒ 2 cards visible.
        assert_eq!(vis.end, 2, "got {vis:?}");
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
    fn edit_line_draws_the_caret_at_the_cursor() {
        // sticky-tab-title — the caret goes where the cursor IS, not at the end. Without
        // this the middle of a name could be edited but not seen being edited.
        assert_eq!(edit_line_text("Front", 5, 10.0, 200.0, '…'), "Front▏");
        assert_eq!(edit_line_text("Front", 0, 10.0, 200.0, '…'), "▏Front");
        assert_eq!(edit_line_text("Front", 2, 10.0, 200.0, '…'), "Fr▏ont");
        // An empty name still shows a caret, so an opened field is visibly an input.
        assert_eq!(edit_line_text("", 0, 10.0, 200.0, '…'), "▏");
        // A caret past the end is clamped rather than panicking.
        assert_eq!(edit_line_text("ab", 99, 10.0, 200.0, '…'), "ab▏");
    }

    #[test]
    fn the_visible_window_follows_the_caret_into_a_long_name() {
        // The reason this is not `truncate_to_width`: a card is barely wider than a word,
        // so a name longer than the card is NORMAL while editing. Anchoring on the tail
        // would hide the cursor the moment the user arrowed back — exactly when it matters.
        let long = "Frontend-service-worker";
        let w = 80.0; // 8 columns at advance 10
                      // Caret at the very start: the window shows the HEAD.
        let head = edit_line_text(long, 0, 10.0, w, '…');
        assert!(head.starts_with('▏'), "caret visible at the head: {head}");
        assert!(head.ends_with('…'), "elision marked on the right: {head}");
        // Caret at the end: the window shows the TAIL.
        let tail = edit_line_text(long, long.chars().count(), 10.0, w, '…');
        assert!(tail.ends_with('▏'), "caret visible at the tail: {tail}");
        assert!(tail.starts_with('…'), "elision marked on the left: {tail}");
        // Caret in the middle: the caret is on screen and BOTH sides are elided.
        let mid = edit_line_text(long, 12, 10.0, w, '…');
        assert!(mid.contains('▏'), "caret visible in the middle: {mid}");
        assert!(
            mid.starts_with('…') && mid.ends_with('…'),
            "both cut: {mid}"
        );
        // None of them may overflow the field.
        for out in [&head, &tail, &mid] {
            assert!(
                out.chars().count() as f32 * 10.0 <= w,
                "never wider than the field: {out}"
            );
        }
    }

    #[test]
    fn the_window_is_exact_at_the_tail_and_in_the_middle() {
        // The shapes above, pinned: 8 columns centred on the caret, back-filled leftwards
        // when the caret is at the end, each cut edge replaced (never extended) by `…`.
        let long = "Frontend-service-worker";
        assert_eq!(edit_line_text(long, 23, 10.0, 80.0, '…'), "…worker▏");
        assert_eq!(edit_line_text(long, 12, 10.0, 80.0, '…'), "…ser▏vi…");
        assert_eq!(edit_line_text(long, 0, 10.0, 80.0, '…'), "▏Fronte…");
    }

    #[test]
    fn a_wide_glyph_is_measured_before_it_is_taken() {
        // D3: each step asks whether the NEXT glyph fits, by its own width. Two columns
        // hold the caret and nothing else — `가` would take the field to three.
        assert_eq!(edit_line_text("가", 1, 10.0, 20.0, '…'), "▏");
        // Three columns: `a` fits beside the caret, `가` in front of it does not.
        assert_eq!(edit_line_text("가a", 2, 10.0, 30.0, '…'), "…▏");
    }

    #[test]
    fn no_caret_position_overflows_the_field_by_columns() {
        // D3/R14.3: the field's budget is display COLUMNS. Every caret position, every
        // width from one column to past the whole string, over mixed-width names.
        for name in [
            "가a나b다",
            "ab가나cd",
            "한글이름",
            "a가",
            "Front-가나다-end",
        ] {
            let n = name.chars().count();
            for caret in 0..=n {
                for cols in 1..=16usize {
                    let out = edit_line_text(name, caret, 10.0, cols as f32 * 10.0, '…');
                    let used: usize = out.chars().map(char_cols).sum();
                    assert!(used <= cols, "{name:?} caret {caret} in {cols}: {out:?}");
                    assert!(out.contains(EDIT_CARET), "{name:?} caret {caret}: {out:?}");
                }
            }
        }
    }

    #[test]
    fn edit_line_degenerate_widths_do_not_panic() {
        assert_eq!(edit_line_text("abc", 3, 10.0, 0.0, '…'), "");
        assert_eq!(edit_line_text("abc", 3, 0.0, 100.0, '…'), "");
        // Exactly one glyph of room: show the cursor, not an ellipsis that would hide it.
        assert_eq!(edit_line_text("abc", 3, 10.0, 10.0, '…'), "▏");
        // Two columns still has to fit a caret and stay inside the field.
        let two = edit_line_text("abcdef", 3, 10.0, 20.0, '…');
        assert!(two.contains('▏'));
        assert!(two.chars().count() <= 2, "fits 2 columns: {two}");
    }

    #[test]
    fn a_hangul_name_windows_without_panicking() {
        // The window slices a Vec<char>, so a multi-byte name cannot split a codepoint.
        let name = "한글이름이아주긴경우";
        let out = edit_line_text(name, 5, 10.0, 60.0, '…');
        assert!(out.contains('▏'), "caret survives: {out}");
        assert!(out.chars().count() as f32 * 10.0 <= 60.0);
    }

    #[test]
    fn card_line1_falls_back_to_cwd_before_the_first_meta_fill() {
        // A row drawn before the app's first `MetaSource::fill` has `RowMeta::default()`,
        // whose `line1` is empty. That must show the folder, never a blank line.
        assert_eq!(card_line1_text("", "terminal"), "terminal");
    }

    // ---- atlas-swap-bind-invalidation: the text bind group's invalidation key ----
    //
    // These three pin both directions of R2.2–R2.4. They observe the private `bind_key`
    // directly (D6 contract 4) rather than adding a production accessor, which is why they
    // live in this file. Every one of them must first get past two early returns —
    // `TabBar::record`'s `left_inset <= 0` / zero-viewport guard and
    // `SidebarTextPipeline::draw`'s empty-text guard — or the bind group is never built at
    // all and the assertions prove nothing (R5.3).

    /// Everything a recorded frame needs but the atlas, which the tests vary themselves.
    struct SwapFixture {
        device: wgpu::Device,
        queue: wgpu::Queue,
        fonts: crate::FontSet,
        /// Kept alive for `view`.
        _target: wgpu::Texture,
        view: wgpu::TextureView,
        snapshot: term_core::GridSnapshot,
        sidebar: SidebarConfig,
    }

    #[test]
    fn a_bold_tab_title_resolves_to_the_real_bold_face() {
        // R5.1. `push_glyphs` asks the atlas with `resolve(c, bold, false)`; this
        // pins what that call answers, which is the whole content of the change
        // here. Before it, the row resolved *unstyled* and then stamped
        // `key.bold = bold` onto the result — a regular-face glyph id wearing a
        // bold flag, which is the collision D15 describes.
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Ok(fonts) = crate::FontSet::load() else {
            eprintln!("skipping: no system font");
            return;
        };
        let atlas = crate::atlas::Atlas::new(&device, &fonts, 32.0).expect("atlas");

        // A Latin title: Menlo ships a real bold, so the title uses it.
        let latin = atlas.resolve('A', true, false).expect("A resolves");
        assert_eq!(latin.slot, crate::StyleSlot::Bold);
        assert!(!latin.synth_bold, "Menlo Bold is real, never emboldened");

        // A Hangul path name — the case that motivated the fallback chain, and the
        // one where synthesis used to erode the glyph.
        if atlas.is_degraded() {
            eprintln!("skipping the Hangul half: no fallback face on this machine");
            return;
        }
        let hangul = atlas.resolve('문', true, false).expect("문 resolves");
        assert_eq!(hangul.face, 1, "from the fallback chain entry");
        assert_eq!(hangul.slot, crate::StyleSlot::Bold);
        assert!(!hangul.synth_bold);

        // And the sub-line, which asks for no style at all, is untouched (H-R0.3).
        let sub = atlas.resolve('문', false, false).expect("문 resolves");
        assert_eq!(sub.slot, crate::StyleSlot::Regular);
        assert!(!sub.synth_bold && !sub.synth_italic);
    }

    /// `None` when there is no adapter or no system font — the crate skips, never fails
    /// (R5.5).
    fn swap_fixture() -> Option<SwapFixture> {
        let (device, queue) = crate::headless()?;
        let fonts = crate::FontSet::load().ok()?;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sidebar-swap-offscreen"),
            size: wgpu::Extent3d {
                width: 512,
                height: 512,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let terminal =
            term_core::Terminal::new(80, 24, term_core::Color(0xFFFFFF), term_core::Color(0));
        let snapshot = terminal.snapshot();
        Some(SwapFixture {
            device,
            queue,
            fonts,
            _target: target,
            view,
            snapshot,
            sidebar: SidebarConfig::default(),
        })
    }

    /// Reset the `thread_local!` singleton (R5.4) and put one card in it, plus one waiting
    /// row. The row is what these tests need: since sidebar-weather-cards the card's own
    /// text is drawn from the sidebar's card atlases, so the **grid**-atlas text pipeline —
    /// whose bind key these tests pin — draws only the band and the popup, and would
    /// otherwise see an empty instance list and never build a bind group at all.
    fn install_one_card() {
        reset_shared_model();
        TabModelHandle::shared().set(TabBarModel {
            titles: vec![SidebarRow {
                tab_id: 1,
                title: "project".into(),
                ..Default::default()
            }],
            active: 0,
            waiting: vec![WaitingRow {
                tab_id: 2,
                title: "build".into(),
                signal: WaitingSignal::Error,
            }],
            ..Default::default()
        });
    }

    /// Record one frame against `atlas` and hand back the text pipeline's private bind key.
    fn record_frame(
        bar: &mut TabBar,
        fx: &SwapFixture,
        atlas: &mut crate::atlas::Atlas,
    ) -> Option<(wgpu::TextureFormat, u64)> {
        record_frame_at(bar, fx, atlas, 1.0)
    }

    /// [`record_frame`] at a chosen backing scale factor.
    fn record_frame_at(
        bar: &mut TabBar,
        fx: &SwapFixture,
        atlas: &mut crate::atlas::Atlas,
        scale_factor: f64,
    ) -> Option<(wgpu::TextureFormat, u64)> {
        record_frame_in(bar, fx, atlas, scale_factor, 340.0)
    }

    /// [`record_frame`] at a chosen scale factor and sidebar width.
    fn record_frame_in(
        bar: &mut TabBar,
        fx: &SwapFixture,
        atlas: &mut crate::atlas::Atlas,
        scale_factor: f64,
        left_inset: f32,
    ) -> Option<(wgpu::TextureFormat, u64)> {
        let mut encoder = fx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sidebar-swap-frame"),
            });
        {
            let mut ctx = FrameCtx {
                device: &fx.device,
                queue: &fx.queue,
                encoder: &mut encoder,
                view: &fx.view,
                viewport: [512.0, 512.0],
                cell: [8.0, 16.0],
                top_inset: 0.0,
                // R5.3: a positive band width, or `record` returns before drawing anything.
                left_inset,
                right_inset: 0.0,
                format: wgpu::TextureFormat::Rgba8Unorm,
                sidebar: &fx.sidebar,
                scale_factor,
                snapshot: &fx.snapshot,
                atlas,
                fonts: &fx.fonts,
            };
            bar.record(&mut ctx);
        }
        fx.queue.submit(std::iter::once(encoder.finish()));
        bar.text_pipe.as_ref().and_then(|p| p.bind_key)
    }

    #[test]
    fn an_atlas_swap_rebuilds_the_sidebar_text_bind_group() {
        // R2.2 — the bug itself. `apply_config` replaces the whole `Atlas`, and the old
        // key `(format, generation, cell)` did not move: `generation` restarts at 0 on a
        // fresh atlas and `cell` only tracks font size. The cached bind group therefore
        // kept pointing at the replaced texture while UVs were computed against the new
        // one. Only `epoch` distinguishes the two textures.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (Some(mut first), Some(mut second)) = (
            crate::atlas::Atlas::new(&fx.device, &fx.fonts, 16.0),
            crate::atlas::Atlas::new(&fx.device, &fx.fonts, 16.0),
        ) else {
            eprintln!("skipping: atlas build failed");
            return;
        };
        install_one_card();
        let (mut bar, _handle) = TabBar::new();

        let before = record_frame(&mut bar, &fx, &mut first);
        assert!(
            before.is_some(),
            "the first frame must actually build a bind group — otherwise this test \
             passes vacuously through an early return"
        );

        // The swap: a different `Atlas` instance, exactly as `apply_config` installs.
        let after = record_frame(&mut bar, &fx, &mut second);
        assert!(after.is_some(), "the second frame still draws text");
        assert_ne!(
            before, after,
            "a replaced atlas texture must invalidate the cached bind group"
        );
        // A moved key does NOT imply a moved bind group. Without this, a mutant that
        // advances `bind_key` while leaving `bind` pointing at the replaced texture
        // passes — and that mutant *is* the production bug: no crash, no validation
        // error, just wrong glyphs for the rest of the session.
        assert_eq!(
            bar.text_pipe.as_ref().map(|p| p.rebuilds),
            Some(2),
            "the swap must actually re-point the bind group, not just re-stamp the key"
        );
    }

    #[test]
    fn an_unchanged_atlas_never_rebuilds_the_sidebar_text_bind_group() {
        // R2.3 — the other direction, and the G1 guard. Rebuilding a bind group on every
        // frame would re-open the idle-CPU debt `docs/architecture.md` records as paid.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let Some(mut atlas) = crate::atlas::Atlas::new(&fx.device, &fx.fonts, 16.0) else {
            eprintln!("skipping: atlas build failed");
            return;
        };
        install_one_card();
        let (mut bar, _handle) = TabBar::new();

        let first = record_frame(&mut bar, &fx, &mut atlas);
        assert!(first.is_some(), "the first frame must build a bind group");
        for frame in 0..4 {
            let again = record_frame(&mut bar, &fx, &mut atlas);
            assert_eq!(
                first, again,
                "frame {frame}: an untouched atlas must not churn the bind group"
            );
        }
        // Key equality alone does NOT prove this: it still holds when the bind group is
        // rebuilt every frame with the same key. Count the actual rebuilds, or the G1
        // guarantee is asserted in name only.
        assert_eq!(
            bar.text_pipe.as_ref().map(|p| p.rebuilds),
            Some(1),
            "5 frames on one atlas must build the bind group exactly once"
        );
    }

    #[test]
    fn an_eviction_alone_does_not_rebuild_the_sidebar_text_bind_group() {
        // R2.4 — eviction reuses the *same* texture (and so the same view and sampler),
        // bumping `generation` only. A rebuild would hand back a byte-identical bind
        // group, so the key must not move. What eviction really invalidates is the UVs,
        // and those are re-issued by the next composed frame, which the generation in
        // `record`'s compose key forces (D5, sidebar-weather-cards D20) — no bind-group work.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let Some(mut atlas) = crate::atlas::Atlas::new(&fx.device, &fx.fonts, 16.0) else {
            eprintln!("skipping: atlas build failed");
            return;
        };
        install_one_card();
        let (mut bar, _handle) = TabBar::new();

        let before = record_frame(&mut bar, &fx, &mut atlas);
        assert!(before.is_some(), "the first frame must build a bind group");
        let generation_before = atlas.generation();

        atlas.evict_for_test();
        assert_ne!(
            atlas.generation(),
            generation_before,
            "the fixture must really have evicted, or this test proves nothing"
        );

        let after = record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(
            before, after,
            "eviction reuses the texture, so the bind group stays valid"
        );
        // Same reasoning as the swap test, inverted: an unmoved key must also mean an
        // unrebuilt bind group, not merely a re-stamped one.
        assert_eq!(
            bar.text_pipe.as_ref().map(|p| p.rebuilds),
            Some(1),
            "eviction must not rebuild the bind group at all"
        );
    }

    // ---- sidebar-weather-cards: the layer's own behaviour (T5) ----

    fn grid_atlas(fx: &SwapFixture) -> crate::atlas::Atlas {
        crate::atlas::Atlas::new(&fx.device, &fx.fonts, 16.0).expect("atlas")
    }

    fn install(model: TabBarModel) {
        reset_shared_model();
        TabModelHandle::shared().set(model);
    }

    fn cards(titles: &[&str]) -> Vec<SidebarRow> {
        titles
            .iter()
            .enumerate()
            .map(|(i, t)| SidebarRow {
                tab_id: i as u64 + 1,
                title: (*t).into(),
                ..Default::default()
            })
            .collect()
    }

    fn published(bar: &TabBar) -> SidebarLayout {
        bar.model.layout().expect("record publishes its layout")
    }

    #[test]
    fn card_text_is_drawn_from_the_card_atlases_and_the_band_from_the_grid() {
        // R2.7 — a card's title changing does not move the grid-atlas text by one
        // glyph, while the card atlases carry the card text.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let model = |title: &str| TabBarModel {
            titles: cards(&[title, "api"]),
            active: 0,
            waiting: vec![WaitingRow {
                tab_id: 9,
                title: "build".into(),
                signal: WaitingSignal::Error,
            }],
            menu: Some(CardMenu {
                row: 1,
                items: vec!["Rename".into(), "Close".into()],
            }),
            ..Default::default()
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install(model("refactor"));
        record_frame(&mut bar, &fx, &mut atlas);
        let grid = bar.text_buf.len();
        assert!(grid > 0, "band + popup text");
        assert!(
            !bar.title_buf.is_empty() && !bar.meta_buf.is_empty(),
            "card text"
        );
        install(model("a considerably longer card title"));
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(
            bar.text_buf.len(),
            grid,
            "card titles never reach the grid atlas"
        );
        reset_shared_model();
    }

    #[test]
    fn an_open_menu_or_a_rename_pins_the_button() {
        // R11.3 — pointer over card 0, card 2's menu open: both draw the `≡`; card 1,
        // neither hovered nor anchoring anything, does not. Then a rename pins card 1's.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        let plate_on = |bar: &TabBar, i: usize| {
            let rect = published(bar).card_rect(i).expect("visible");
            let plate = crate::sidebar_card::menu_button(rect, 1.0);
            bar.shape_buf.iter().any(|s| s.bbox == plate)
        };
        install(TabBarModel {
            titles: cards(&["a", "b", "c"]),
            active: 1,
            menu: Some(CardMenu {
                row: 2,
                items: vec!["Rename".into()],
            }),
            ..Default::default()
        });
        TabModelHandle::shared().set_hovered(Some(0));
        record_frame(&mut bar, &fx, &mut atlas);
        assert!(plate_on(&bar, 0), "hovered");
        assert!(plate_on(&bar, 2), "its menu is open");
        assert!(!plate_on(&bar, 1), "the active card alone shows no button");
        // The popup itself reaches its own pass, drawn over the card text.
        assert_eq!(
            bar.popup_pipe.as_ref().map(|p| p.uploads),
            Some(1),
            "the open menu is drawn"
        );

        TabModelHandle::shared().set_menu(None);
        TabModelHandle::shared().set_hovered(None);
        TabModelHandle::shared().set_edit(Some(CardEdit {
            tab_id: 2,
            text: "b".into(),
            caret: 1,
            selected: false,
        }));
        record_frame(&mut bar, &fx, &mut atlas);
        assert!(plate_on(&bar, 1), "being renamed");
        assert!(!plate_on(&bar, 0) && !plate_on(&bar, 2));
        reset_shared_model();
    }

    #[test]
    fn the_rename_field_takes_the_title_line_at_title_size() {
        // R14.1–R14.3 — a Hangul name with the caret at its end: the field's run is the
        // bold title face, the caret sits after 8 title columns, the box ends 10 short
        // of the `≡` (1×), and the folder line is still drawn.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install(TabBarModel {
            titles: cards(&["a", "api"]),
            active: 1,
            edit: Some(CardEdit {
                tab_id: 2,
                text: "사이드바".into(),
                caret: 4,
                selected: false,
            }),
            ..Default::default()
        });
        record_frame(&mut bar, &fx, &mut atlas);
        let rect = published(&bar).card_rect(1).expect("visible");
        let plate = crate::sidebar_card::menu_button(rect, 1.0);
        let field = bar
            .run_buf
            .iter()
            .find(|r| r.text == format!("사이드바{EDIT_CARET}"))
            .expect("the field's run");
        assert_eq!((field.face, field.bold), (CardFace::Title, true));
        let col = CardFace::Title.column_px(1.0);
        let caret_pen = crate::tab_bar::pen_layout(&field.text, field.x, col)
            .last()
            .expect("caret")
            .1;
        assert_eq!(caret_pen, field.x + 8.0 * col, "after 8 title columns");
        let panel = linear(SidebarTheme::DEFAULT.panel_bg, 1.0);
        let boxed = bar
            .shape_buf
            .iter()
            .find(|s| s.fill0 == panel && s.bbox[1] < field.top + 1.0 && s.bbox[1] > rect[1])
            .expect("the field box");
        assert_eq!(
            boxed.bbox[0] + boxed.bbox[2],
            plate[0] - 10.0,
            "stops before the ≡"
        );
        // The whole box, not just its right edge (where the padding cancels): the title
        // zone grown by `title_gap + border` = 4 each way but the top, which takes half.
        let line = CardFace::Title.line_px(1.0);
        assert_eq!(
            [boxed.bbox[0], boxed.bbox[1], boxed.bbox[3]],
            [field.x - 4.0, field.top - 2.0, line + 4.0]
        );
        assert_eq!(boxed.geom, [8.0, 1.0], "half the card radius, a 1px border");
        assert!(
            bar.run_buf
                .iter()
                .any(|r| r.text == "api" && r.face == CardFace::Meta),
            "the folder line stays"
        );
        reset_shared_model();
    }

    #[test]
    fn a_mid_frame_eviction_is_resolved_again_before_submitting() {
        // R2.5 — evict the title atlas right after the first run resolves: the layer sees
        // the generation move, resolves every run once more, and what it keeps is exactly
        // what a clean frame resolves.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install_one_card();
        record_frame(&mut bar, &fx, &mut atlas); // warm: builds the card atlases
        bar.evict_during_resolve = Some(CardFace::Title);
        bar.compose_key = None; // an unchanged frame would be re-submitted, not composed
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(bar.resolve_passes, 2, "the eviction forced a second pass");
        let evicted: Vec<[f32; 4]> = bar.title_buf.iter().map(|t| t.rect).collect();
        let evicted_uv: Vec<[f32; 2]> = bar.title_buf.iter().map(|t| t.uv0).collect();
        bar.compose_key = None;
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(bar.resolve_passes, 1, "a clean frame needs one");
        assert_eq!(
            bar.title_buf.iter().map(|t| t.rect).collect::<Vec<_>>(),
            evicted
        );
        assert_eq!(
            bar.title_buf.iter().map(|t| t.uv0).collect::<Vec<_>>(),
            evicted_uv,
            "no UV from before the eviction survived"
        );
        reset_shared_model();
    }

    #[test]
    fn a_scale_change_rebinds_the_card_text() {
        // R2.3 at the layer — the title pipeline's bind group follows the rebuilt atlas.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install_one_card();
        record_frame_at(&mut bar, &fx, &mut atlas, 1.0);
        let key = |bar: &TabBar| bar.title_pipe.as_ref().and_then(|p| p.bind_key);
        let first = key(&bar).expect("title text was drawn");
        assert_eq!(Some(first.1), bar.cards.epoch(CardFace::Title));
        record_frame_at(&mut bar, &fx, &mut atlas, 2.0);
        let second = key(&bar).expect("still drawn");
        assert_ne!(first, second, "a new atlas, a new bind group");
        assert_eq!(Some(second.1), bar.cards.epoch(CardFace::Title));
        reset_shared_model();
    }

    // ---- sidebar-weather-cards T6: compose only on change (D20) ----

    /// A metadata source whose generation the test moves by hand.
    struct Gen(std::cell::Cell<u64>);
    impl MetaSource for Gen {
        fn generation(&self) -> u64 {
            self.0.get()
        }
        fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>) {
            out.clear();
            out.resize(rows.len(), RowMeta::default());
        }
    }

    /// Instance-buffer writes per pipeline the layer owns: grid text, title text, meta
    /// text, shapes, popup shapes, fallback card text. Kept apart, so one pipeline that
    /// stopped counting cannot hide behind another that still does.
    fn uploads(bar: &TabBar) -> [u32; 6] {
        let text = |p: &Option<SidebarTextPipeline>| p.as_ref().map_or(0, |p| p.uploads);
        let shape = |p: &Option<SidebarShapePipeline>| p.as_ref().map_or(0, |p| p.uploads);
        [
            text(&bar.text_pipe),
            text(&bar.title_pipe),
            text(&bar.meta_pipe),
            shape(&bar.shape_pipe),
            shape(&bar.popup_pipe),
            text(&bar.fallback_pipe),
        ]
    }

    #[test]
    fn a_steady_frame_composes_nothing_and_writes_no_buffer() {
        // R17.1 — the second and later identical frames re-submit what is resident.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install_one_card();
        record_frame(&mut bar, &fx, &mut atlas);
        let (composed, written) = (bar.compose_count, uploads(&bar));
        assert_eq!(composed, 1);
        // One write each — the waiting row's grid text, both card faces, the shapes; no
        // popup is open and both card atlases built, so those two pipelines never are.
        assert_eq!(written, [1, 1, 1, 1, 0, 0], "the first frame uploads");
        for _ in 0..5 {
            record_frame(&mut bar, &fx, &mut atlas);
        }
        assert_eq!(bar.compose_count, composed, "nothing composed");
        assert_eq!(uploads(&bar), written, "no instance buffer written");
        assert!(
            published(&bar).card_rect(0).is_some(),
            "the layout is still published"
        );
        reset_shared_model();
    }

    #[test]
    fn every_key_input_recomposes_exactly_once() {
        // R17.2 — each input, changed alone, costs one composition and then is steady
        // again; a cache that missed an input would show 0 here, one that keyed on
        // something unstable would keep counting.
        let Some(mut fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        let source = Rc::new(Gen(std::cell::Cell::new(1)));
        install(TabBarModel {
            titles: cards(&["a", "b", "c"]),
            active: 0,
            ..Default::default()
        });
        h.set_meta_source(source.clone());
        let (mut scale, mut width) = (1.0, 340.0);
        record_frame_in(&mut bar, &fx, &mut atlas, scale, width);

        type Change<'c> = Box<dyn FnMut(&mut TabBar, &mut crate::atlas::Atlas) + 'c>;
        let steps: Vec<(&str, Change)> = vec![
            ("hovered", Box::new(|_, _| h.set_hovered(Some(1)))),
            (
                "active",
                Box::new(|_, _| {
                    h.set(TabBarModel {
                        titles: cards(&["a", "b", "c"]),
                        active: 2,
                        ..Default::default()
                    })
                }),
            ),
            ("meta generation", Box::new(|_, _| source.0.set(2))),
            ("scroll", Box::new(|_, _| h.set_list_scroll(0.0, 5.0))),
            (
                "menu",
                Box::new(|_, _| {
                    h.set_menu(Some(CardMenu {
                        row: 0,
                        items: vec!["Rename".into()],
                    }))
                }),
            ),
            (
                "edit",
                Box::new(|_, _| {
                    h.set_edit(Some(CardEdit {
                        tab_id: 1,
                        text: "ab".into(),
                        caret: 1,
                        selected: false,
                    }))
                }),
            ),
            (
                "edit caret",
                Box::new(|_, _| {
                    h.set_edit(Some(CardEdit {
                        tab_id: 1,
                        text: "ab".into(),
                        caret: 2,
                        selected: false,
                    }))
                }),
            ),
            (
                "waiting rows",
                Box::new(|_, _| {
                    h.set_waiting(vec![WaitingRow {
                        tab_id: 3,
                        title: "build".into(),
                        signal: WaitingSignal::Error,
                    }])
                }),
            ),
            ("grid atlas generation", Box::new(|_, a| a.evict_for_test())),
            (
                "title atlas generation",
                Box::new(|bar, _| {
                    bar.cards
                        .get_mut(CardFace::Title)
                        .expect("built")
                        .evict_for_test()
                }),
            ),
            (
                "meta atlas generation",
                Box::new(|bar, _| {
                    bar.cards
                        .get_mut(CardFace::Meta)
                        .expect("built")
                        .evict_for_test()
                }),
            ),
        ];
        for (what, mut change) in steps {
            let before = bar.compose_count;
            change(&mut bar, &mut atlas);
            record_frame_in(&mut bar, &fx, &mut atlas, scale, width);
            assert_eq!(bar.compose_count, before + 1, "{what} recomposes once");
            record_frame_in(&mut bar, &fx, &mut atlas, scale, width);
            assert_eq!(bar.compose_count, before + 1, "{what} is then steady");
        }
        // The frame's own inputs.
        for (what, s, w) in [("scale", 2.0, 340.0), ("sidebar width", 2.0, 400.0)] {
            let before = bar.compose_count;
            (scale, width) = (s, w);
            record_frame_in(&mut bar, &fx, &mut atlas, scale, width);
            assert_eq!(bar.compose_count, before + 1, "{what} recomposes once");
            record_frame_in(&mut bar, &fx, &mut atlas, scale, width);
            assert_eq!(bar.compose_count, before + 1, "{what} is then steady");
        }
        // The config the key carries: both display toggles and the accent.
        type Toggle = fn(&mut SidebarConfig);
        let toggles: [(&str, Toggle); 3] = [
            ("show_git_branch", |c| {
                c.show_git_branch = !c.show_git_branch
            }),
            ("show_agent_status", |c| {
                c.show_agent_status = !c.show_agent_status
            }),
            ("accent", |c| c.accent ^= 0x00FF_FFFF),
        ];
        for (what, toggle) in toggles {
            let before = bar.compose_count;
            toggle(&mut fx.sidebar);
            record_frame_in(&mut bar, &fx, &mut atlas, scale, width);
            assert_eq!(bar.compose_count, before + 1, "{what} recomposes once");
            record_frame_in(&mut bar, &fx, &mut atlas, scale, width);
            assert_eq!(bar.compose_count, before + 1, "{what} is then steady");
        }
        reset_shared_model();
    }

    #[test]
    fn a_card_run_sits_on_its_line_box_baseline() {
        // D3: a run's glyphs hang from a baseline at 0.8 × its line box below the run's
        // top, each offset by its own bearing — the same placement the grid text uses.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = grid_atlas(&fx);
        let key = atlas.resolve('j', false, false).expect("j resolves");
        let e = atlas
            .get_or_insert_styled(&fx.queue, key)
            .expect("j rasterizes");
        assert!(
            e.left != 0.0 && e.top != 0.0,
            "a glyph with both bearings: {e:?}"
        );
        let mut out = Vec::new();
        let run = TextRun {
            face: CardFace::Title,
            text: "j".into(),
            x: 10.0,
            top: 20.0,
            color: [1.0; 4],
            bold: false,
        };
        push_run(&mut out, &mut atlas, &fx.queue, &run, 9.0, 25.0);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].rect,
            [10.0 + e.left, 20.0 + 20.0 - e.top, e.px_w, e.px_h]
        );
    }

    #[test]
    fn select_all_highlights_the_name_and_not_the_caret() {
        // sticky-tab-title R2.1 under D3: the highlight covers the name's display columns
        // at title size — `사이드바` is 8 — and stops before the caret; an empty name has
        // no highlight at all.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let accent = linear(SidebarTheme::DEFAULT.accent, 0.40);
        let highlight = |text: &str| {
            let (mut bar, _h) = TabBar::new();
            let mut atlas = grid_atlas(&fx);
            install(TabBarModel {
                titles: cards(&["a"]),
                edit: Some(CardEdit {
                    tab_id: 1,
                    text: text.into(),
                    caret: text.chars().count(),
                    selected: true,
                }),
                ..Default::default()
            });
            record_frame(&mut bar, &fx, &mut atlas);
            let field = bar
                .run_buf
                .iter()
                .find(|r| r.text.ends_with(EDIT_CARET))
                .expect("the field's run")
                .clone();
            let hl: Vec<_> = bar
                .shape_buf
                .iter()
                .filter(|s| s.fill0 == accent)
                .map(|s| (s.bbox, s.geom))
                .collect();
            reset_shared_model();
            (field, hl)
        };
        let col = CardFace::Title.column_px(1.0);
        let (field, hl) = highlight("사이드바");
        assert_eq!(
            hl,
            vec![(
                [field.x, field.top, 8.0 * col, CardFace::Title.line_px(1.0)],
                [1.0, 0.0]
            )]
        );
        assert_eq!(highlight("").1, vec![], "nothing to highlight");
    }

    #[test]
    fn a_card_atlas_that_cannot_be_built_degrades_to_the_grid_atlas() {
        // R2.6 through the layer. The frame's font set has no usable face, so neither card
        // atlas builds; the grid atlas keeps the real fonts. Card text must still be drawn
        // — from the grid atlas, measured by the grid cell — in a pass of its own that is
        // submitted before the popup, and the failed build is not retried per frame.
        let Some(mut fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let mut atlas = grid_atlas(&fx);
        fx.fonts = crate::sidebar_text::broken_fonts();
        let (mut bar, _h) = TabBar::new();
        install(TabBarModel {
            titles: cards(&["alpha"]),
            menu: Some(CardMenu {
                row: 0,
                items: vec!["Rename".into()],
            }),
            ..Default::default()
        });
        record_frame(&mut bar, &fx, &mut atlas);
        assert!(bar.cards.get(CardFace::Title).is_none());
        assert!(bar.cards.get(CardFace::Meta).is_none());
        assert!(bar.title_buf.is_empty() && bar.meta_buf.is_empty());

        // Every card run, resolved from the grid atlas at the grid cell (8×16 here) and
        // nothing else: exactly what `push_run` gives at those metrics.
        let mut want = Vec::new();
        for run in &bar.run_buf {
            push_run(&mut want, &mut atlas, &fx.queue, run, 8.0, 16.0);
        }
        let rects = |v: &[TextInstance]| v.iter().map(|t| t.rect).collect::<Vec<_>>();
        assert_eq!(rects(&bar.fallback_buf), rects(&want));
        // The title band carries ink: the title's five glyphs, each with an area.
        let title = bar
            .run_buf
            .iter()
            .find(|r| r.face == CardFace::Title)
            .expect("the title run")
            .clone();
        assert_eq!(title.text, "alpha");
        let inked = bar
            .fallback_buf
            .iter()
            .filter(|t| t.rect[1] < title.top + 16.0 && t.rect[2] > 0.0 && t.rect[3] > 0.0)
            .count();
        assert_eq!(inked, 5, "the title band still carries ink");

        // Its own pass, not the grid pass that is drawn over the popup: the grid text
        // holds the popup's item and nothing of the card's.
        assert_eq!(uploads(&bar)[5], 1, "the fallback pass is drawn");
        let mut item = Vec::new();
        let menu_item = TextRun {
            face: CardFace::Meta,
            text: "Rename".into(),
            x: 0.0,
            top: 0.0,
            color: [1.0; 4],
            bold: false,
        };
        push_run(&mut item, &mut atlas, &fx.queue, &menu_item, 8.0, 16.0);
        assert_eq!(bar.text_buf.len(), item.len(), "only the popup's item");

        // One build attempt, however many frames compose.
        for hovered in [Some(0), None, Some(0)] {
            TabModelHandle::shared().set_hovered(hovered);
            record_frame(&mut bar, &fx, &mut atlas);
        }
        assert_eq!(bar.cards.builds(), 1, "not retried per frame");
        reset_shared_model();
    }

    #[test]
    fn a_grid_eviction_while_composing_is_composed_again_next_frame() {
        // D20 with the grid atlas: the band's text was resolved against it, then composing
        // evicted it. Keyed on the atlas as composing *left* it, the next frame would hit
        // and re-submit those stale UVs for as long as the sidebar stayed still. Keyed on
        // the atlas as composing *found* it, the next frame misses once and is then steady.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, _h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install_one_card();
        bar.evict_grid_during_compose = true;
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(bar.compose_count, 1);
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(
            bar.compose_count, 2,
            "composed again against the repacked atlas"
        );
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(bar.compose_count, 2, "then steady");
        reset_shared_model();
    }

    #[test]
    fn the_source_is_filled_only_when_its_generation_or_the_rows_move() {
        // D10 under D20: a steady frame never calls `fill` (nor clones the rows for it); a
        // generation change alone refills, and so does a roster change alone.
        struct Counting {
            gen: std::cell::Cell<u64>,
            fills: std::cell::Cell<u32>,
        }
        impl MetaSource for Counting {
            fn generation(&self) -> u64 {
                self.gen.get()
            }
            fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>) {
                self.fills.set(self.fills.get() + 1);
                out.clear();
                out.resize(rows.len(), RowMeta::default());
            }
        }
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install(TabBarModel {
            titles: cards(&["a", "b"]),
            ..Default::default()
        });
        let src = Rc::new(Counting {
            gen: std::cell::Cell::new(1),
            fills: std::cell::Cell::new(0),
        });
        h.set_meta_source(src.clone());
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(src.fills.get(), 1, "the first frame fills");
        for _ in 0..3 {
            record_frame(&mut bar, &fx, &mut atlas);
        }
        assert_eq!(src.fills.get(), 1, "a steady frame does not");
        src.gen.set(2);
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(src.fills.get(), 2, "a new generation alone refills");
        h.set(TabBarModel {
            titles: cards(&["a", "b", "c"]),
            ..Default::default()
        });
        record_frame(&mut bar, &fx, &mut atlas);
        assert_eq!(src.fills.get(), 3, "a roster change alone refills");
        reset_shared_model();
    }

    #[test]
    fn the_band_and_popup_state_are_compared_not_just_their_revision() {
        // D20: every setter bumps `rows_rev`, but the fields are public and `with_mut` is
        // too. A write that skipped the bump must still recompose — the key holds the
        // waiting rows, the menu and the rename state themselves. Each changed alone.
        let Some(fx) = swap_fixture() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let (mut bar, h) = TabBar::new();
        let mut atlas = grid_atlas(&fx);
        install(TabBarModel {
            titles: cards(&["a", "b"]),
            ..Default::default()
        });
        record_frame(&mut bar, &fx, &mut atlas);
        type Write = fn(&mut SidebarModel);
        let writes: [(&str, Write); 3] = [
            ("waiting", |m| {
                m.waiting = vec![WaitingRow {
                    tab_id: 2,
                    title: "b".into(),
                    signal: WaitingSignal::Waiting,
                }]
            }),
            ("menu", |m| {
                m.menu = Some(CardMenu {
                    row: 0,
                    items: vec!["Rename".into()],
                })
            }),
            ("edit", |m| {
                m.edit = Some(CardEdit {
                    tab_id: 1,
                    text: "a".into(),
                    caret: 1,
                    selected: false,
                })
            }),
        ];
        for (what, write) in writes {
            let before = bar.compose_count;
            h.with_mut(write);
            record_frame(&mut bar, &fx, &mut atlas);
            assert_eq!(bar.compose_count, before + 1, "{what}, written directly");
        }
        reset_shared_model();
    }
}
