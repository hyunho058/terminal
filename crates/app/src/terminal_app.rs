//! The winit `ApplicationHandler` — the tabbed vertical slice (T3).
//!
//! Owns the event loop and wires everything: N independent [`Session`]s as tabs,
//! a registered tab-bar [`DrawLayer`] painting the reserved top inset, tab chords
//! (D5), and the SD4 registration seam ([`InputRouter`]/[`OverlayHost`]) that
//! later features extend without editing this file.
//!
//! Input dispatch order per key (R3.3/R4): composing gate (R6.3) → visible
//! overlays → registered input handlers → tab chords → default `encode_key` to
//! the active shell.
//!
//! IME (R6): the window allows IME (R6.1) so macOS composes 한글 before we ever
//! see it; `WindowEvent::Ime` carries the composition (`Preedit`) and its result
//! (`Commit`), and `suppress_key` keeps composing jamo out of the shell.
//!
//! IME cancellation + commit routing (R8/D11): a composition is owned by the tab
//! it started in (`composing_tab`), so a `Commit` that arrives after a tab switch
//! lands in *that* tab, never in whichever tab is active now. Switching tabs or
//! losing focus clears the preedit inline and cancels the OS composition with
//! `NSTextInputContext::discardMarkedText` — `set_ime_allowed(false)` does **not**
//! do that (it only clears winit's own `marked_text` ivar).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

use common::config::{Config, SidebarConfig};
use common::watch::ConfigWatcher;
use objc2_app_kit::NSView;
use renderer::cursor_layer::CursorLayer;
use renderer::link_layer::{LinkHandle, LinkLayer};
use renderer::preedit_layer::{PreeditHandle, PreeditLayer};
use renderer::sidebar_header::{SidebarChromeHandle, SidebarChromeLayer};
use renderer::sidebar_layout::{self, SidebarLayoutInput};
use renderer::sidebar_row::{self, SidebarGeom};
use renderer::sidebar_theme::SidebarMetrics;
use renderer::tab_bar::{TabBar, TabBarModel, TabModelHandle, WaitingRow, WaitingSignal};
use renderer::Renderer;
use term_core::Color;
use term_core::GridSnapshot;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
// winit 0.30 re-exports raw-window-handle 0.6 (`rwh_06`, a default feature), so
// reaching the `NSView` needs no new dependency on `app` (D11).
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Window, WindowId};

use crate::hyperlink::{
    frame_rescans, url_to_open, ClickInput, HoverInput, HoverState, LinkTrigger, Opener, Update,
};
use crate::input::{
    self, encode_key, hover_card, hover_cursor, is_double_click, is_sidebar_toggle_chord,
    pixel_to_cell, press_dismisses_modals, sidebar_press, sidebar_region, suppress_key, tab_action,
    wheel_bytes, wheel_lines, wheel_scroll, ReleaseTarget, ResizeDrag, SidebarAction, TabAction,
    WheelDelta,
};
use crate::seam::{
    AppCtx, AppRequest, Handled, InputHandler, InputRouter, KeyInput, Overlay, OverlayCommand,
    OverlayHost, WriteProbe,
};
use crate::session::{Session, TabId};
use crate::tabmeta::{AgentState, TabMetaSnapshot};
use crate::tabs::Tabs;
use crate::waiting::{WaitingKind, WaitingState};
use term_core::Selection;

/// Custom event: something happened off the UI thread that needs a wake.
#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    /// PTY output is available for a specific tab (D2 — tagged wake).
    PtyOutput(TabId),
    /// The config file changed; a fresh `Config` is waiting on the channel
    /// (settings feature — background file-watch, not a key event).
    ConfigReloaded,
    /// A registered overlay has pending background work (e.g. ai-monitor's log
    /// tail); the loop should `poll` the overlays and redraw.
    OverlayPending,
}

/// Translate T3's `WaitingKind` to the renderer's `WaitingSignal` (D6). The app owns this
/// mapping so the renderer never names the app's kind enum (dependency direction).
fn waiting_signal(kind: WaitingKind) -> WaitingSignal {
    match kind {
        WaitingKind::Error => WaitingSignal::Error,
        WaitingKind::Waiting => WaitingSignal::Waiting,
        WaitingKind::Completion => WaitingSignal::Completion,
    }
}

/// Top-level app. `inner` is populated on `resumed`.
pub struct App {
    cfg: Config,
    proxy: EventLoopProxy<UserEvent>,
    inner: Option<Running>,
    /// SD4 seam: features register handlers/overlays here (from `main.rs`, in
    /// their own module) *before* `run_app` — never by editing this file (C1).
    /// Consumed into the live registries on `resumed`.
    pending_handlers: Vec<Box<dyn InputHandler>>,
    pending_overlays: Vec<Box<dyn Overlay>>,
}

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        App {
            cfg,
            proxy,
            inner: None,
            pending_handlers: Vec::new(),
            pending_overlays: Vec::new(),
        }
    }

    /// Register an input handler (SD4/R3.3) — the seam entry a sibling feature
    /// (e.g. image-paste's Cmd+V) calls from `main.rs` instead of editing the
    /// event loop. No in-tree consumer yet (first: image-paste).
    #[allow(dead_code)]
    pub fn register_handler(&mut self, handler: Box<dyn InputHandler>) {
        self.pending_handlers.push(handler);
    }

    /// Register an overlay (SD4/R4) — its `on_key` runs before default input and
    /// its `DrawLayer` is drawn above the grid (ai-monitor/settings). No in-tree
    /// consumer yet.
    #[allow(dead_code)]
    pub fn register_overlay(&mut self, overlay: Box<dyn Overlay>) {
        self.pending_overlays.push(overlay);
    }
}

/// Everything that only exists once we have a window + GPU + shells.
struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    tabs: Tabs,
    tab_model: TabModelHandle,
    /// The chrome layer's handle (D10/D11): the raw cursor is pushed here each move; the
    /// header/restore layer resolves its own hover from it.
    chrome_handle: SidebarChromeHandle,
    /// Sidebar config mirror, refreshed by `settings::apply` — read for one field,
    /// `show_git_branch`, when building the layout (D5). `collapsed` is NOT read from here
    /// (the renderer's override is the single source of truth).
    sidebar_cfg: SidebarConfig,
    /// The sidebar list scroll offset in physical px, app-owned (D10). Published to the
    /// row layer via `set_list_scroll`, clamped whenever `max_scroll` shrinks.
    scroll: f32,
    /// The last card press: which row, and when (ms since the app's epoch). Drives
    /// double-click-to-rename (sticky-tab-title) — a second click on the same card within
    /// `DOUBLE_CLICK_MS` opens the rename field on it. `None` until the first card press.
    last_card_click: Option<(usize, u128)>,
    /// The instant the app started, so click times are a plain `u128` of elapsed ms — a
    /// pure number the double-click rule can be unit-tested against without a clock.
    epoch: Instant,
    /// The open card menu's target tab + its item set (sticky-tab-title D10/R3.2).
    /// App-owned state: the renderer is *told* a menu is open so it can lay the popup out
    /// and hit-test it, but the decision to open, what it contains, and what a press means
    /// all live here. Keyed by `TabId`, so a roster shift under an open menu cannot
    /// retarget it. `None` = no menu open.
    open_menu: Option<(TabId, Vec<crate::card_menu::MenuItem>)>,
    /// Unspent sub-line remainder of the in-flight wheel gesture over the *grid*, in
    /// lines (terminal-scroll D3). Without it a slow trackpad drag — a few px per event
    /// — would truncate to zero lines every time and the grid would never move.
    ///
    /// A tab switch is the **only** thing that resets it. Snaps deliberately do not: a
    /// snap can be triggered by an invisible tab (output arriving in a background tab,
    /// or an IME commit routed to the tab it began in), and zeroing it there would let
    /// an async event in a tab the user is not touching silently eat the gesture they
    /// are making in the one they are.
    wheel_accum: f32,
    /// Last resolved card-hover index — the region debounce for `set_hovered` (D8).
    last_hit: Option<usize>,
    router: InputRouter,
    overlays: OverlayHost,
    mods: ModifiersState,
    /// Latest cursor position in physical px (raw winit `f64`), updated on
    /// `CursorMoved` and read on a press to hit-test the sidebar and re-resolve hover.
    last_cursor: (f64, f64),
    /// Whether the pointer is inside the window (hyperlinks R2.8). `last_cursor` is
    /// written only by `CursorMoved` and never reset, so `cursor_cell()` keeps
    /// reporting a stale cell after `CursorLeft`; this is the flag that actually
    /// knows. Set true only by `CursorMoved`, where the position is fresh — not by
    /// `CursorEntered`, which carries no coordinates.
    pointer_inside: bool,
    /// ⌘-hover link state + the handle its layer reads (hyperlinks D15/D24).
    link_hover: HoverState,
    link_handle: LinkHandle,
    /// Was ⌘ held when the left button went down **over the grid** (hyperlinks D22)?
    /// Sampled at press so a ⌘ tapped after an ordinary click cannot retroactively
    /// turn it into an open. Reset by every press, including one the sidebar claims.
    link_cmd_at_press: bool,
    /// Hands a URL to macOS on an opening click (hyperlinks D14).
    opener: Opener,
    /// Left-drag selection state (selection-input, the sanctioned event-loop
    /// exception). While `dragging`, `CursorMoved` updates the active grid's
    /// selection focus. `drag_anchor` is the press cell; `drag_moved` records
    /// whether the focus ever left the anchor cell — a plain click (never moved)
    /// clears the selection on release, jitter-safely (cell-granular, not raw px).
    dragging: bool,
    drag_anchor: (u16, u16),
    drag_moved: bool,
    /// What the in-flight drag swallowed, replayed on release (terminal-scroll D6).
    /// Meaningless while `dragging` is false; reset at each press.
    drag_latch: DragLatch,
    /// The mouse cursor shape currently set on the window (tab-resize D11). Mirrored here
    /// so `set_cursor_icon` can skip a platform call when the shape has not changed —
    /// `refresh_hover` runs on every `CursorMoved`.
    cursor_icon: CursorIcon,
    /// An in-flight sidebar edge resize (tab-resize D17). **Deliberately disjoint from
    /// `dragging`**: the two gestures have different release semantics — a selection
    /// release keeps a selection, a resize release commits a config write — and one
    /// overloaded flag is how they would come to corrupt each other. Invariant:
    /// `resize_drag.is_some()` ⇒ `!dragging`.
    ///
    /// While this is `Some`, the resize owns the pointer: `CursorMoved` drives the width
    /// and skips hover resolution, and `CursorLeft` does not end the gesture.
    resize_drag: Option<ResizeDrag>,
    /// Where the sidebar size is persisted (tab-resize D2). `None` when the platform gave
    /// us no config directory — the session then works normally but does not remember.
    config_path: Option<PathBuf>,
    /// The `(width_px, collapsed)` pair of our last **successful** write (tab-resize D15).
    ///
    /// The latch behind echo suppression: the atomic rename IS a change event, so the
    /// watcher hands our own write straight back, and `settings::apply` would rebuild the
    /// whole glyph atlas for a change we already applied. **Latched, not one-shot** — every
    /// delivery equal to this is suppressed, because FSEvents can report one rename more
    /// than once and a "consume the first match" rule would pay the rebuild on the second.
    written_sidebar: Vec<(f32, bool)>,
    /// The last `Config` actually applied to the running app (tab-resize D15, as amended).
    ///
    /// Echo suppression compares the **whole** delivered config against this, not just the
    /// sidebar pair. Comparing only the pair looked right and was catastrophic: the latch
    /// is armed on every successful persist and the file then matches it indefinitely, so
    /// *every* later reload — of any field — classified as an echo and was dropped. One
    /// click of Hide killed live settings for the rest of the session. Suppress only when
    /// nothing at all changed; the sidebar pair still decides override precedence (D15a).
    last_applied_cfg: Config,
    /// Set when a write FAILED, so the file and the override are knowingly out of step
    /// (tab-resize D14 × D15a). While true, a non-echo delivery must not clear the sidebar
    /// override — otherwise an unrelated reload (a `font.size_pt` edit) would snap the
    /// sidebar back and break D14's promise that the session keeps the dragged width.
    sidebar_override_authoritative: bool,
    /// Fresh configs delivered by the background watcher (settings feature).
    config_rx: Receiver<Config>,
    /// Kept alive so the watch thread keeps running; dropping it stops watching.
    _config_watcher: Option<ConfigWatcher>,
    /// True while the window is fully occluded/hidden — the continuous redraw loop
    /// pauses so a background/minimized terminal doesn't burn the GPU.
    occluded: bool,
    /// Idle-CPU (D7): the last snapshot handed to the renderer, reused on idle
    /// frames so the continuous redraw doesn't re-clone the whole grid each frame.
    last_snapshot: Option<GridSnapshot>,
    /// Set by `redraw()` (the "real content change" entry point) and cleared after
    /// a fresh snapshot is taken. The self-perpetuating continuous
    /// `window.request_redraw()` in `RedrawRequested` deliberately does NOT set it,
    /// so an idle frame reuses `last_snapshot`. Marks stale on PTY advance, tab
    /// switch, resize, config reload, and overlay/router redraws.
    snapshot_dirty: bool,
    /// True while an IME composition is in flight (D8). Set from a non-empty
    /// `Ime::Preedit`, cleared by the empty `Preedit`, `Commit`, and — critically
    /// — `Ime::Disabled` (R6.7): a 한↔영 input-source switch can skip the empty
    /// `Preedit` entirely, and a stuck `true` would swallow every character key
    /// forever.
    composing: bool,
    /// The in-flight composition text (e.g. `"하"` while typing `ㅎ`+`ㅏ`). Held
    /// here so the loop owns exactly one copy of the composition state.
    preedit: String,
    /// Which tab **owns** the in-flight composition (R8/D11). Set from the first
    /// non-empty `Ime::Preedit`, cleared on `Commit`/empty `Preedit`/`Disabled`.
    /// `Ime::Commit` routes by this id instead of "whoever is active now": a
    /// Cmd+T mid-composition is an *internal* switch (no `resignFirstResponder`),
    /// so macOS keeps its marked text and the commit would otherwise land in the
    /// new tab's shell — silent cross-tab data corruption (R8.3). `None` means the
    /// text never went through a composition, and goes to the active tab (R8.5).
    composing_tab: Option<TabId>,
    /// Write end of the [`PreeditLayer`]'s shared model (D9/R7.1) — the only way
    /// the composition reaches the screen. It deliberately never touches
    /// `term-core`: these characters have not been written to the PTY, so putting
    /// them in the grid would pollute the snapshot and the scrollback with text
    /// the shell never received. The handle also carries the theme colors, since
    /// `FrameCtx` has none (D14/R7.5).
    preedit_handle: PreeditHandle,
    /// The waiting-band assembly model (T3): consumes per-session command-end
    /// (OSC 133) + agent-state signals and produces the ordered waiting items T4
    /// renders. Pure policy — the ≥30s threshold, `{1,130}` ignore-list, per-session
    /// precedence, active-tab exclusion, and acknowledge-on-activate all live in it.
    waiting: WaitingState,
    /// Reused buffer for reading each tab's agent state out of the metadata worker
    /// (via the process-global [`TabMetaHandle`]) to feed the waiting model's
    /// rising-edge Waiting detection. Kept across polls so `sync` reuses its
    /// allocation (mirrors `SidebarMetaSource`'s snapshot).
    tabmeta_snapshot: TabMetaSnapshot,
}

impl App {
    fn start(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> crate::session::anyhow_lite::Result<Running> {
        let attrs = Window::default_attributes()
            .with_title("mac-terminal")
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 540.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        // Let macOS run the input method (R6.1). Without this winit's `key_down`
        // never calls `interpretKeyEvents` (view.rs:466-468), so a Korean IME never
        // gets to compose and only raw jamo reach the shell. Turning it on also
        // makes winit withhold the duplicate `KeyboardInput` for composing/committing
        // keys (view.rs:479-496); ASCII, which never marks text, is unaffected.
        window.set_ime_allowed(true);

        let size = window.inner_size();
        let scale = window.scale_factor();
        let mut renderer = Renderer::new(window.clone(), size.width, size.height, scale, &self.cfg)
            .map_err(err)?;

        // The renderer already reserved the sidebar band in `assemble` from the config
        // width (row-redesign D4) — no `set_left_inset` here (it is now a no-op). Register
        // the card layer, then the chrome layer (header + collapsed restore button) whose
        // first drawn frame is this one (chrome-collapse D14).
        let (bar, tab_model) = TabBar::new();
        renderer.add_layer(Box::new(bar));
        let (chrome, chrome_handle) = SidebarChromeLayer::new();
        renderer.add_layer(Box::new(chrome));
        // Blinking block cursor, above the glyphs (a DrawLayer so it blinks off the
        // continuous-redraw loop, unaffected by the idle-CPU snapshot cache).
        renderer.add_layer(Box::new(CursorLayer::new()));
        // Inline IME composition, above the cursor block (D18/R7.3): the 0.5-alpha
        // cursor would otherwise wash out the first composing cell. Seed the theme
        // colors now — the layer reads them from the handle, not from `FrameCtx`,
        // which carries no color (D14/R7.5).
        let (preedit_layer, preedit_handle) = PreeditLayer::new();
        preedit_handle.set_theme(self.cfg.theme.background, self.cfg.theme.foreground);
        renderer.add_layer(Box::new(preedit_layer));
        // ⌘-hover link underline (hyperlinks D15). Registered here rather than
        // through an `Overlay` presenter: the seam hands the renderer one *stateless*
        // box at startup, and this layer's state is app-side by D11 — so the app must
        // hold the write end of the handle. No theme to seed; the underline takes each
        // cell's own glyph color.
        let (link_layer, link_handle) = LinkLayer::new();
        renderer.add_layer(Box::new(link_layer));

        // Drain the SD4 seam registrations into the live registries. Each
        // overlay also contributes a DrawLayer drawn above the grid (R4.1).
        let mut router = InputRouter::default();
        for h in self.pending_handlers.drain(..) {
            router.register(h);
        }
        let mut overlays = OverlayHost::default();
        for o in self.pending_overlays.drain(..) {
            renderer.add_layer(o.layer());
            overlays.register(o);
        }

        // grid_size() now reports rows *below* the inset (R2.2).
        let (cols, rows) = renderer.grid_size();
        let fg = Color(self.cfg.theme.foreground);
        let bg = Color(self.cfg.theme.background);

        let first = Session::spawn(TabId(0), cols, rows, fg, bg, self.proxy.clone())?;
        let tabs = Tabs::new(first, fg, bg, self.proxy.clone());

        // Live-config watcher (settings feature): on a valid change it sends the
        // fresh Config and wakes the loop with UserEvent::ConfigReloaded.
        let (config_tx, config_rx) = mpsc::channel::<Config>();
        // Resolved once and kept: the same path the watcher observes is the one
        // tab-resize writes back to (D2), so they can never drift apart.
        let config_path = Config::path()
            .map_err(|e| tracing::warn!("config path unavailable: {e}"))
            .ok();
        let config_watcher = match config_path.clone() {
            Some(path) => {
                let proxy = self.proxy.clone();
                ConfigWatcher::spawn(path, move |cfg| {
                    if config_tx.send(cfg).is_ok() {
                        let _ = proxy.send_event(UserEvent::ConfigReloaded);
                    }
                })
                .map_err(|e| tracing::warn!("config watcher disabled: {e}"))
                .ok()
            }
            None => {
                tracing::warn!("live settings off: no config path");
                None
            }
        };

        tracing::info!(cols, rows, "terminal started: zsh spawned, grid ready");
        let mut running = Running {
            window,
            renderer,
            tabs,
            tab_model,
            chrome_handle,
            sidebar_cfg: self.cfg.sidebar.clone(),
            scroll: 0.0,
            wheel_accum: 0.0,
            last_hit: None,
            router,
            overlays,
            mods: ModifiersState::empty(),
            last_cursor: (0.0, 0.0),
            pointer_inside: false,
            link_hover: HoverState::default(),
            link_handle,
            link_cmd_at_press: false,
            opener: Opener::new(),
            last_card_click: None,
            epoch: Instant::now(),
            open_menu: None,
            dragging: false,
            drag_anchor: (0, 0),
            drag_moved: false,
            drag_latch: DragLatch::default(),
            cursor_icon: CursorIcon::Default,
            resize_drag: None,
            config_path,
            written_sidebar: Vec::new(),
            last_applied_cfg: self.cfg.clone(),
            sidebar_override_authoritative: false,
            config_rx,
            _config_watcher: config_watcher,
            occluded: false,
            last_snapshot: None,
            snapshot_dirty: true,
            composing: false,
            preedit: String::new(),
            composing_tab: None,
            preedit_handle,
            waiting: WaitingState::new(),
            tabmeta_snapshot: TabMetaSnapshot::default(),
        };
        running.sync_tab_model();
        running.redraw();
        Ok(running)
    }
}

impl Running {
    /// Request a redraw for a **real content change**. Marks the cached snapshot
    /// stale (`snapshot_dirty`) so `RedrawRequested` re-clones the grid on the next
    /// frame (D7). Use this — not `window.request_redraw()` directly — for every
    /// content change; the bare call is reserved for the idle continuous frame,
    /// which must NOT re-snapshot.
    fn redraw(&mut self) {
        // A real content change: the cached snapshot is now stale, so the next
        // RedrawRequested re-snapshots (D7). The continuous redraw at the tail of
        // RedrawRequested calls `window.request_redraw()` directly (not this), so it
        // does NOT mark stale — idle frames reuse the cached snapshot.
        self.snapshot_dirty = true;
        self.window.request_redraw();
    }

    /// Handle one `WindowEvent::Ime` (R6.2/R6.6/R6.7, D12). All four variants are
    /// handled explicitly — falling into `_ => {}` for `Disabled` is exactly the
    /// deadlock R6.7 guards against.
    fn on_ime(&mut self, ime: Ime) {
        match ime {
            // The IME finished a syllable: those bytes are the user's text (R6.2).
            // winit queues an empty `Preedit` immediately before every `Commit`
            // (view.rs:411-414), so `composing` is already false here for the plain
            // ASCII path; clear it anyway so the state can never outlive a commit.
            Ime::Commit(text) => {
                // Route by tab identity, not by "whoever is active now" (R8/D11):
                // the composition belongs to the tab it started in. Deliberately
                // *not* a one-shot "discard the next commit" guard — a guard that
                // misfires eats a character that has no other path to the PTY
                // (`had_ime_input` suppresses the duplicate `KeyboardInput`,
                // view.rs:478-489).
                //
                // This read is only meaningful because ownership deliberately
                // OUTLIVES the empty `Preedit` that winit queues immediately before
                // every `Commit` (view.rs:412-413). Clearing the owner there — as
                // the first cut did — made this whole routing inert: `composing_tab`
                // was always `None` by the time we got here, so every commit fell to
                // the active tab and `Tab`/`Drop` were dead code. Ownership is
                // released *here*, after the bytes have been placed, and by
                // `Ime::Disabled` / the start of a new composition.
                // A visible text field claims the composition first (sticky-tab-title
                // R2.6). This branch is the whole reason the rename field can accept
                // Hangul: `Ime::Commit` bypasses `on_key` entirely — the composing gate
                // withholds the duplicate `KeyboardInput` — so without this the syllable
                // would go to the shell and the field would receive nothing at all.
                if !text.is_empty()
                    && self
                        .overlays
                        .command(&OverlayCommand::ImeCommit(text.clone()))
                        == Handled::Consumed
                {
                    self.set_preedit(String::new());
                    self.composing_tab = None;
                    self.redraw();
                    return;
                }
                if !text.is_empty() {
                    let composing_tab = self.composing_tab;
                    let target =
                        commit_target(composing_tab, |id| self.tabs.by_id_mut(id).is_some());
                    match target {
                        // The composing tab is still open — write there (R8.3).
                        CommitTarget::Tab(id) => {
                            let ok = match self.tabs.by_id_mut(id) {
                                Some(session) => match session.write(text.as_bytes()) {
                                    Ok(()) => true,
                                    Err(e) => {
                                        tracing::warn!("pty write failed: {e}");
                                        false
                                    }
                                },
                                // Unreachable: `commit_target` only yields `Tab` when
                                // the probe said it exists. Never panic on it.
                                None => {
                                    tracing::warn!(tab = id.0, "commit target vanished");
                                    false
                                }
                            };
                            // Snap the tab the bytes went to, not the active one
                            // (terminal-scroll D4) — a composition that began in a tab
                            // the user has since left still owns its own viewport.
                            if ok {
                                self.snap_after_input(id);
                            }
                        }
                        // Plain, non-composed text — the active tab, as always.
                        // This branch is what guarantees no character is ever
                        // lost (R8.5).
                        CommitTarget::Active => {
                            // The path `on_key` never sees: winit's composing gate plus
                            // `had_ime_input` mean a committed 한글 syllable arrives
                            // *only* here. Without this snap, typing Korean into a
                            // scrolled-back view would leave the screen frozen and the
                            // terminal looking dead (terminal-scroll D4).
                            match self.tabs.active_mut().write(text.as_bytes()) {
                                Ok(()) => self.snap_active_after_input(),
                                Err(e) => tracing::warn!("pty write failed: {e}"),
                            }
                        }
                        // The tab that owned the composition was closed: those
                        // bytes have no owner, so drop them silently (R8.4).
                        CommitTarget::Drop => {
                            tracing::debug!("ime commit dropped: composing tab closed");
                        }
                    }
                }
                // The bytes are placed: release ownership so the next composition
                // starts clean (R8/D11).
                self.set_preedit(String::new());
                self.composing_tab = None;
            }
            // Composition in flight. Empty text is winit's synthetic clear signal
            // (view.rs:345), so `!is_empty()` is exactly the composing predicate (R6.6).
            Ime::Preedit(text, _cursor) => {
                // Show the in-flight composition inside a capturing field too
                // (sticky-tab-title R2.6), so typing Hangul into the rename prompt looks
                // live rather than frozen until the syllable commits. The grid's own
                // preedit state is still updated: it drives the candidate-window position
                // and the composing gate, which must keep working either way.
                let claimed = self
                    .overlays
                    .command(&OverlayCommand::ImePreedit(text.clone()))
                    == Handled::Consumed;
                self.set_preedit_shown(text, !claimed);
            }
            // Nothing to do: `set_ime_allowed(true)` already put us in the state
            // this announces. Logged so an IME trace is readable end to end.
            Ime::Enabled => tracing::debug!("ime enabled"),
            // Input-source switch (한↔영) can land here with no preceding empty
            // `Preedit` (view.rs:446-452) — clearing here is what keeps `composing`
            // from sticking true and swallowing every later character key (R6.7/D12).
            Ime::Disabled => {
                tracing::debug!("ime disabled");
                self.set_preedit(String::new());
                // The IME is gone, so no `Commit` is coming for this composition —
                // release ownership too, or it would leak into the next one (D11).
                self.composing_tab = None;
            }
        }
    }

    /// Single writer for the composition state: keeps `composing`, `preedit` and
    /// `composing_tab` in lockstep, repositions the candidate window, and redraws
    /// (R6.6/R6.8/R8).
    fn set_preedit(&mut self, text: String) {
        self.set_preedit_shown(text, true);
    }

    /// As [`set_preedit`](Self::set_preedit), but `show_in_grid == false` keeps the
    /// composition **out of the grid's inline preedit** (sticky-tab-title R2.6).
    ///
    /// Needed because a capturing overlay draws the composition itself: without this the
    /// same jamo would appear twice — once in the rename field and once at the shell
    /// cursor underneath it. The composing *state* is still updated either way, since it
    /// drives the `suppress_key` gate and the candidate-window position.
    fn set_preedit_shown(&mut self, text: String, show_in_grid: bool) {
        // The transition itself lives in `next_composition_state` so it can be
        // table-tested without a window or an IME (R6.6/R6.7/D11/D12).
        let (composing, owner) = next_composition_state(
            &text,
            self.composing,
            self.composing_tab,
            self.tabs.active().id,
        );
        self.composing = composing;
        self.composing_tab = owner;
        self.preedit = text;
        // Push it to the renderer's preedit layer so the composition shows inline
        // at the cursor (R7.1/D9). The layer is re-recorded every frame, so this
        // lands on screen even though the grid snapshot has not moved — the
        // composition never enters `term-core`.
        self.preedit_handle.set_text(if show_in_grid {
            self.preedit.clone()
        } else {
            String::new()
        });
        self.update_ime_cursor_area();
        // A composition change is a real content change, not an idle frame, so use
        // `redraw()` (which marks the snapshot cache stale), never the bare
        // `window.request_redraw()`.
        self.redraw();
    }

    /// Park the macOS candidate / Hanja-conversion window on the terminal cursor
    /// cell (R6.8/D19). **Physical px only**: `cell_size()` already folds in the
    /// scale factor and the insets are documented physical, while winit's macOS
    /// impl divides by the scale itself (window_delegate.rs:1553-1561) — handing it
    /// a `LogicalPosition` would place the window at 2× the offset on any Retina Mac.
    ///
    /// Only called while a composition is live. winit's macOS `set_ime_cursor_area`
    /// does `self.inputContext().expect("input context")` (view.rs:901), which
    /// **panics** when the view is not the first responder — exactly the state
    /// `Focused(false)` puts us in, and the very `None` that the sibling
    /// [`discard_marked_text`](Self::discard_marked_text) defends against. There is
    /// also nothing to position when no candidate window is up.
    fn update_ime_cursor_area(&mut self) {
        if !self.composing {
            return;
        }
        let (cw, ch) = self.renderer.cell_size();
        // The cached snapshot is the cursor the user is looking at; before the
        // first frame there is none, so fall back to the grid origin.
        let cursor = self
            .last_snapshot
            .as_ref()
            .map(|s| s.cursor)
            .unwrap_or_default();
        let x = self.renderer.left_inset() + cursor.col as f32 * cw;
        let y = self.renderer.top_inset() + cursor.row as f32 * ch;
        self.window
            .set_ime_cursor_area(PhysicalPosition::new(x, y), PhysicalSize::new(cw, ch));
    }

    /// Abandon an in-flight composition on an *internal* focus change — a tab
    /// switch or a window focus loss (R8.1/R8.2/D11). No-op when not composing.
    ///
    /// Order matters: our own state is cleared **inline, here**, not by waiting
    /// for the `Ime::Disabled` that the D12 path handles — winit `queue_event`s
    /// its IME events and dispatches them later, so `composing`/`preedit` would
    /// still be dirty during the switch itself.
    /// Ownership (`composing_tab`) deliberately **survives** the cancel. If the IME
    /// server ignores our `discardMarkedText` — or had already queued the commit —
    /// the stray `Ime::Commit` still lands in the tab the composition began in
    /// (R8.3) rather than leaking into the tab the user just switched to. It is
    /// released by that commit, by `Ime::Disabled`, or by the next composition.
    fn cancel_composition(&mut self) {
        if !self.composing {
            return;
        }
        // Clears `composing`, `preedit` and the preedit layer, and redraws — the
        // composing text must leave the screen now (R8.1). `composing_tab` is kept
        // on purpose; see the doc comment above.
        self.set_preedit(String::new());
        self.discard_marked_text();
    }

    /// Cancel the composition the *OS* is holding (R8.2/D11).
    ///
    /// `set_ime_allowed(false)` does not do this: in winit 0.30.13 it only clears
    /// winit's own `marked_text` ivar and queues `Ime::Disabled` (view.rs:880-896);
    /// `inputContext.discardMarkedText()` is called only from `unmarkText`
    /// (view.rs:342-343). So the IME server's composition — and its candidate
    /// window — would survive. Call AppKit directly instead.
    fn discard_marked_text(&self) {
        let handle = match self.window.window_handle() {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("no window handle; ime composition not discarded: {e}");
                return;
            }
        };
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            tracing::warn!("window handle is not AppKit; ime composition not discarded");
            return;
        };
        // SAFETY: `ns_view` comes straight from winit's live `NSView` for this
        // window, which outlives the borrow (we hold the `Window` in `self`), and
        // this runs on the winit event loop — i.e. the main thread, as every
        // `NSView` method requires.
        let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
        match view.inputContext() {
            Some(ctx) => ctx.discardMarkedText(),
            // No input context (window not key / no responder): nothing is marked.
            None => tracing::debug!("no NSTextInputContext; nothing to discard"),
        }
    }

    /// Apply the tab mutations recorded during dispatch (sticky-tab-title R1.4/R3.6),
    /// then resync + redraw if anything actually changed.
    ///
    /// Returns `false` when the last tab was closed, so the caller exits the app — the
    /// same signal `do_tab_action` uses for Cmd+W (D4).
    ///
    /// The resync here is what makes a rename visible **at once, on any tab**: an
    /// app-initiated title change never enters `Session::drain`, so it never sets
    /// `title_dirty` and never rides the PTY wake path that normally refreshes the
    /// sidebar (R4.1).
    fn apply_requests(&mut self, requests: Vec<AppRequest>) -> bool {
        if requests.is_empty() {
            return true;
        }
        let mut changed = false;
        for req in requests {
            match req {
                AppRequest::SetUserTitle(id, title) => {
                    if let Some(s) = self.tabs.by_id_mut(id) {
                        changed |= s.set_user_title(title);
                    }
                }
                AppRequest::CloseTab(id) => {
                    // Closing a tab kills any modal surface targeting it (R2.5): a field
                    // still open on a tab that no longer exists would commit into nothing.
                    // Broadcast before the close so the overlay can still see its target.
                    self.overlays.command(&OverlayCommand::Cancel);
                    self.close_card_menu();
                    if !self.tabs.close(id) {
                        return false;
                    }
                    // A close changes the roster, so it owes the FULL bookkeeping, not
                    // just a model resync (R5.3 — parity with Cmd+W).
                    self.after_roster_change();
                    // Safe ONLY because `after_roster_change` is a strict superset of the
                    // `if changed` tail below (it syncs and redraws): clearing the flag
                    // suppresses a redundant second sync, and cannot drop an earlier
                    // `SetUserTitle` in this batch, since that sync re-reads the mutated
                    // sessions. If this method ever stops syncing, this line becomes a bug.
                    changed = false;
                }
            }
        }
        if changed {
            self.sync_tab_model();
            self.redraw();
        }
        true
    }

    /// Open the card menu on the tab at list index `i` (sticky-tab-title R3.6). The item
    /// set depends on that tab — `Reset to auto` only exists where there is a title to
    /// reset — so it is derived here and pushed to the renderer as labels.
    ///
    /// Re-pressing the hamburger of the tab whose menu is already open closes it, which is
    /// what makes the button feel like a toggle rather than a dead click.
    fn open_card_menu(&mut self, i: usize) {
        let Some(id) = self.row_tab_id(i) else {
            return;
        };
        if self.open_menu.as_ref().is_some_and(|(open, _)| *open == id) {
            self.close_card_menu();
            return;
        }
        // Opening a menu closes any open rename field (D8a.1 — never two modal surfaces).
        // Without this, pressing a hamburger while the field is up leaves BOTH on screen
        // with the field still consuming every key, and silently contradicts D8's "a
        // sidebar click cancels the edit".
        self.overlays.command(&OverlayCommand::Cancel);
        let has_title = self
            .tabs
            .by_id(id)
            .is_some_and(|s| s.user_title().is_some());
        let items = crate::card_menu::items_for(has_title);
        self.tab_model
            .set_menu(Some(renderer::sidebar_row::CardMenu {
                row: i,
                items: crate::card_menu::labels(&items),
            }));
        self.open_menu = Some((id, items));
        self.redraw();
    }

    /// Open the rename field on the card at list index `i`, prefilled with its current
    /// user title (sticky-tab-title). Shared by the double-click gesture and the card
    /// menu's `Rename`, so both open the field in exactly the same state — with an
    /// existing name present and selected, waiting for input.
    fn begin_rename_row(&mut self, i: usize) {
        let Some(id) = self.row_tab_id(i) else {
            return;
        };
        let current = self.displayed_label(id);
        self.overlays
            .command(&OverlayCommand::BeginRename { tab: id, current });
        self.redraw();
    }

    /// The label the card is **currently showing** for `id` — what the user is looking at
    /// when they start a rename.
    ///
    /// Prefilling from `user_title` alone was wrong: a tab that has never been named has
    /// no user title, so the field opened EMPTY and the name the user could plainly see
    /// vanished the moment they tried to edit it. You edit the text that is there, so the
    /// field must start from the resolved label — the same four sources, in the same
    /// order, that `sidebar_model::fill` draws from.
    ///
    /// Returns `None` only when even the cwd is unavailable, which leaves an empty field
    /// with nothing sensible to prefill.
    fn displayed_label(&mut self, id: TabId) -> Option<String> {
        // Refresh the metadata snapshot so the AI-task label is the one on screen, not a
        // stale one — the sidebar reads it on a per-frame pull, this path does not.
        if let Some(handle) = crate::tabmeta::global() {
            handle.sync(&mut self.tabmeta_snapshot);
        }
        let session = self.tabs.by_id(id)?;
        // The sync just above makes this the freshest published cwd (tmux-pane-cwd D2), so the
        // prefill names the same directory the card shows.
        let cwd = session
            .cwd(self.tabmeta_snapshot.get(id))
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let task = self
            .tabmeta_snapshot
            .get(id)
            .and_then(|m| m.task.as_deref());
        let label = crate::sidebar_model::resolve_line1_tier(
            session.user_title(),
            task,
            Some(session.program_title()),
            &cwd,
        )
        .0
        .to_string();
        (!label.is_empty()).then_some(label)
    }

    /// Close the card menu if one is open (R3.8). Idempotent, so every dismissal path —
    /// an outside click, Esc, a tab chord, choosing an item — can call it unconditionally.
    fn close_card_menu(&mut self) {
        if self.open_menu.take().is_some() {
            self.tab_model.set_menu(None);
            self.redraw();
        }
    }

    /// Act on menu item `i` of the open menu (R3.5/R3.6), then close the menu.
    ///
    /// The item's *identity* is resolved through the stored list, never from the index
    /// alone: position 1 is `Close` on an unnamed tab and `Reset to auto` on a named one,
    /// so indexing blind would fire the wrong action on half the tabs.
    ///
    /// Returns `false` when the app should exit (the last tab was closed).
    fn activate_menu_item(&mut self, i: usize) -> bool {
        let Some((id, items)) = self.open_menu.take() else {
            return true;
        };
        // Choosing an item always closes the menu FIRST (D8a.1): the rename field opens in
        // the same event, and the two modal surfaces must never be open at once.
        self.tab_model.set_menu(None);
        let Some(item) = items.get(i).copied() else {
            self.redraw();
            return true;
        };
        match item {
            crate::card_menu::MenuItem::Rename => {
                // The app→overlay direction: the menu can target a BACKGROUND tab, which
                // the Cmd+Shift+R chord cannot (it always names the active one). Prefilled
                // from the DISPLAYED label, like every other entry point, so the name you
                // are looking at is the name you start editing.
                let current = self.displayed_label(id);
                self.overlays
                    .command(&OverlayCommand::BeginRename { tab: id, current });
            }
            crate::card_menu::MenuItem::ResetToAuto => {
                if let Some(s) = self.tabs.by_id_mut(id) {
                    s.set_user_title(None);
                }
                self.sync_tab_model();
            }
            crate::card_menu::MenuItem::Close => {
                self.overlays.command(&OverlayCommand::Cancel);
                if !self.tabs.close(id) {
                    return false; // last tab → exit, exactly as Cmd+W does
                }
                // Full bookkeeping, not a bare resync (R5.3): closing the ACTIVE tab from
                // its own menu must leave the app in the same state Cmd+W would — the
                // ai-monitor re-pointed off the dead session, the scroll re-clamped to the
                // shorter list, the new active tab's waiting signal acknowledged.
                self.after_roster_change();
            }
        }
        self.redraw();
        true
    }

    /// The `TabId` of the sidebar row at list index `i` (R3.3) — the app-side join the
    /// renderer cannot do, since it speaks only in row indices. Same pattern as
    /// `waiting_row_tab_index`. `None` if the row vanished between draw and press.
    fn row_tab_id(&self, i: usize) -> Option<TabId> {
        self.tab_model
            .with(|m| m.rows.get(i).map(|r| TabId(r.tab_id)))
    }

    /// Push the current tab rows + active index into the sidebar layer. `Tabs::titles()`
    /// also pushes the metadata roster to the worker (tab-metadata seam).
    fn sync_tab_model(&mut self) {
        let titles = self.tabs.titles(&self.tabmeta_snapshot);
        let waiting = self.build_waiting_rows(&titles);
        self.tab_model.set(TabBarModel {
            titles,
            active: self.tabs.active_index(),
            waiting,
            // The open card menu survives a row resync (sticky-tab-title R3.2): a tab's
            // title changing underneath an open menu must not close it. T6 owns opening
            // and closing it; `set` only carries the current value through.
            menu: self.tab_model.menu(),
            // An in-flight inline rename survives a row resync too (R2.1): a background
            // tab's title changing must not wipe the name the user is mid-way through
            // typing on another card.
            edit: self.tab_model.edit(),
        });
    }

    /// Join T3's per-session waiting items to session titles, producing the renderer's
    /// waiting-band rows (D6/D9). Translates each `WaitingKind` → `WaitingSignal` (the
    /// renderer must not name the app's kind), and drops any item whose session has no
    /// matching title (a tab closed between assembly and draw). Order is preserved — T3
    /// already sorted oldest-wait-first (R1.5) — and the ≤5 + "+N more" visual cap is the
    /// renderer's (R1.6), so the FULL list is handed over. `titles` is passed in (rather
    /// than recomputed) so the frequent-event refresh path avoids `titles()`'
    /// per-tab `proc_pidinfo`.
    fn build_waiting_rows(&self, titles: &[sidebar_row::SidebarRow]) -> Vec<WaitingRow> {
        self.waiting_items()
            .into_iter()
            .filter_map(|item| {
                let title = titles
                    .iter()
                    .find(|r| r.tab_id == item.tab_id.0)
                    .map(|r| r.title.clone())?;
                Some(WaitingRow {
                    tab_id: item.tab_id.0,
                    title,
                    signal: waiting_signal(item.kind),
                })
            })
            .collect()
    }

    /// Rebuild the pinned waiting band from T3's current items + the live tab titles and
    /// republish it (D9). Used when the waiting SET changed without a tab open/close (a
    /// background command completing, an agent handing its turn back), so it reuses the
    /// titles already in the model rather than recomputing them. The band's height shifts
    /// `list_top`, so it also republishes the scroll offset (re-clamped) and re-resolves
    /// hover under the stationary pointer.
    fn refresh_waiting_band(&mut self) {
        let rows = self.tab_model.with(|m| m.rows.clone());
        let waiting = self.build_waiting_rows(&rows);
        self.tab_model.set_waiting(waiting);
        let inp = self.sidebar_input();
        self.scroll = sidebar_layout::clamp_scroll(self.scroll, &inp);
        self.publish_scroll();
        self.refresh_hover();
    }

    /// Resolve pinned waiting-band row `band_i` to its tab's list index (D12): the band's
    /// order (oldest wait first) differs from the tab order, so map through the row's
    /// `tab_id`. `None` if the row or its tab vanished between draw and click.
    fn waiting_row_tab_index(&self, band_i: usize) -> Option<usize> {
        self.tab_model.with(|m| {
            let tab_id = m.waiting.get(band_i)?.tab_id;
            m.rows.iter().position(|r| r.tab_id == tab_id)
        })
    }

    /// Sync each tab's agent state out of the metadata worker and run the waiting
    /// model's rising-edge Waiting detection (T3). Returns whether the waiting item
    /// set changed (so the caller can redraw). No-op returning `false` when the
    /// metadata worker never spawned — then there are simply no agent signals, and
    /// command-end/error signals degrade independently (graceful degrade, R7).
    fn feed_agent_states(&mut self) -> bool {
        let Some(handle) = crate::tabmeta::global() else {
            return false;
        };
        handle.sync(&mut self.tabmeta_snapshot);
        // The sync may have brought a new cwd for the active tab — a tmux window switch moves
        // the pane without touching the tab, so `on_active_tab_changed` (the only other caller
        // of this seam) never fires for it (tmux-pane-cwd D6/R4.1). Without this the panel
        // stays attached to the directory it opened on.
        //
        // Called **unconditionally**, level-triggered: `on_active_changed` already no-ops when
        // nothing moved (`overlay.rs`'s `Repoint::changed` guard), which is cheaper and more
        // robust than tracking an edge here. An edge would be lost outright when
        // `displayed_label` consumes the same generation from its own `sync` first.
        let cwd = self.tabs.active_cwd(&self.tabmeta_snapshot);
        let shell_pid = self.tabs.active_shell_pid();
        self.overlays.notify_active_changed(cwd, shell_pid);
        self.push_agent_states()
    }

    /// Recompute every tab's agent level from the CURRENT tabmeta snapshot plus each tab's
    /// own screen, and feed it to the waiting model. Does not re-sync tabmeta — callers
    /// driven by PTY output use this, since the metadata cannot have changed and the point
    /// is the screen.
    ///
    /// **Why the screen is consulted at all.** While a tool-permission dialog is open,
    /// Claude Code appends NOTHING to its session log, so the log-derived
    /// `meta.agent.state` reads `Busy` for as long as the human takes. The dialog is on the
    /// tab's own grid, which we own, so it is read there (`crate::prompt`). Gated on a
    /// detected agent, so a shell that merely prints similar text can never raise an item.
    fn push_agent_states(&mut self) -> bool {
        self.push_agent_states_inner(None)
    }

    /// As [`push_agent_states`](Self::push_agent_states) but for a single tab — used by the
    /// PTY-output path, where only the originating tab's screen can have changed and the
    /// others' metadata is the same snapshot. `on_agent_states` treats tabs independently,
    /// so this is equivalent to the full sweep for that tab and skips N-1 screen scans per
    /// chunk (a high-throughput tab would otherwise multiply the scan by the tab count).
    fn push_agent_states_for(&mut self, only: TabId) -> bool {
        self.push_agent_states_inner(Some(only))
    }

    fn push_agent_states_inner(&mut self, only: Option<TabId>) -> bool {
        let active = self.tabs.active().id;
        // A tab with no live agent maps to `Unknown` (not `Waiting`), so an agent
        // that finished (`agent: None`) is seen as leaving Waiting — a falling edge.
        let base: Vec<(TabId, AgentState, bool)> = self
            .tabmeta_snapshot
            .iter()
            .filter(|(id, _)| only.is_none_or(|want| *id == want))
            .map(|(id, meta)| {
                (
                    id,
                    meta.agent.map(|a| a.state).unwrap_or(AgentState::Unknown),
                    meta.agent.is_some(),
                )
            })
            .collect();
        let states: Vec<(TabId, AgentState)> = base
            .into_iter()
            .map(|(id, state, has_agent)| {
                if !has_agent {
                    tracing::debug!(tab = id.0, ?state, "waiting-scan: no agent on tab");
                    return (id, state);
                }
                let detected = self.tab_shows_blocking_prompt(id);
                // The one diagnostic that has paid for itself here: when an agent tab is
                // scanned and NOT detected, dump what was on screen. Guessing at which
                // layer was wrong cost two wrong fixes; the tail text says directly whether
                // the dialog was absent, scrolled, or merely unmatched.
                //
                // Explicitly gated rather than relying on the macro's field laziness: this
                // runs per PTY chunk, and `tab_scan_tail` builds a String. An eager
                // evaluation here would be a real cost in release builds with logging off.
                if tracing::enabled!(tracing::Level::DEBUG) {
                    let tail = self.tab_scan_tail(id);
                    tracing::debug!(
                        tab = id.0,
                        ?state,
                        detected,
                        active = (id == active),
                        tail = %tail,
                        "waiting-scan"
                    );
                }
                if detected {
                    (id, AgentState::Waiting)
                } else {
                    (id, state)
                }
            })
            .collect();
        let changed = self
            .waiting
            .on_agent_states(&states, active, Instant::now());
        if tracing::enabled!(tracing::Level::DEBUG) {
            tracing::debug!(
                ?states,
                active = active.0,
                changed,
                items = self.waiting.items().len(),
                "waiting-model"
            );
        }
        changed
    }

    /// The exact text the prompt matcher saw for `id`, condensed to one line for a log.
    /// Debug-only aid; never called unless the `debug` level is enabled.
    fn tab_scan_tail(&mut self, id: TabId) -> String {
        let Some(s) = self.tabs.by_id_mut(id) else {
            return "<no session>".into();
        };
        let off = s.terminal.view_offset();
        let tail = s.terminal.viewport_tail_text(crate::prompt::TAIL_ROWS);
        let last: Vec<&str> = tail.lines().rev().take(6).collect();
        let last: Vec<&str> = last.into_iter().rev().collect();
        format!("off={off} | {}", last.join(" ⏎ "))
    }

    /// Whether tab `id`'s visible screen currently shows a prompt that blocks on a human
    /// (see [`crate::prompt`]).
    ///
    /// Deliberately a **level**, not an event: it is re-evaluated on every trigger, so
    /// answering the prompt makes the dialog disappear and the tab leaves `Waiting` through
    /// the existing falling edge. Nothing to latch, expire, or replay.
    ///
    /// Uses `viewport_tail_text` rather than a full snapshot: this runs per PTY chunk, and
    /// cloning the grid at that rate would be the G1 idle-CPU debt all over again.
    fn tab_shows_blocking_prompt(&mut self, id: TabId) -> bool {
        self.tabs
            .by_id_mut(id)
            .map(|s| {
                // Only the LIVE bottom can hold a live dialog. A tab parked in scrollback
                // is showing history, and history may well contain a dialog that was
                // answered long ago — reading that as "waiting now" is a false positive
                // that would not self-clear (nothing scrolls it away). Also skips the scan
                // entirely for scrolled tabs.
                s.terminal.view_offset() == 0
                    && crate::prompt::shows_blocking_prompt(
                        &s.terminal.viewport_tail_text(crate::prompt::TAIL_ROWS),
                    )
            })
            .unwrap_or(false)
    }

    /// The ordered waiting items for the sidebar band — T4's read point (D2/R1.5).
    /// Oldest wait first; the full list, with no max-5 / "+N more" cap (that is
    /// T4's visual concern, R1.6/D11). Consumed by [`build_waiting_rows`](Self::build_waiting_rows).
    fn waiting_items(&self) -> Vec<crate::waiting::WaitingItem> {
        self.waiting.items()
    }

    /// Build the sidebar layout input from live accessors + the siblings' own functions —
    /// no app-side geometry (SD2/D3). `sidebar_w` is read **live** so a post-toggle build
    /// sees the new width; `row_h` comes from `sidebar_row::layout`, never a local formula.
    fn sidebar_input(&self) -> SidebarLayoutInput {
        let sidebar_w = self.renderer.left_inset();
        let size = self.window.inner_size();
        let viewport = [size.width as f32, size.height as f32];
        let scale = self.renderer.scale_factor();
        let cell = self.renderer.cell_size();
        let (count, waiting_count) = self.tab_model.with(|m| (m.rows.len(), m.waiting.len()));
        // The pinned waiting band pushes the card list down by its height (D9); an empty
        // band adds 0.0, keeping the original list_top (R1.2).
        let list_top =
            sidebar_layout::header_h(scale) + sidebar_layout::waiting_band_h(waiting_count, scale);
        let geom = SidebarGeom {
            sidebar_w,
            viewport_h: viewport[1],
            cell: [cell.0, cell.1],
            scale,
            show_branch: self.sidebar_cfg.show_git_branch,
            list_top,
            scroll_px: self.scroll,
        };
        let row_h = sidebar_row::layout(&geom, count).card_h;
        SidebarLayoutInput {
            sidebar_w,
            viewport,
            scale,
            row_h,
            row_count: count,
            waiting_count,
            metrics: SidebarMetrics::DEFAULT.scaled(scale),
            // The open menu is layout input, not renderer state (sticky-tab-title R3.2):
            // the app owns the decision, the renderer lays it out and hit-tests it.
            open_menu: self.tab_model.open_menu(),
        }
    }

    /// Whether the sidebar is collapsed — the renderer's inset is the single source of
    /// truth (D5), so the app stores no boolean.
    fn collapsed(&self) -> bool {
        self.renderer.left_inset() <= 0.0
    }

    /// Publish the scroll offset (and the header height) to the row layer so the drawn
    /// cards and the hit-test move together (D10). `list_top` is byte-identical to what
    /// the chrome layer writes, so the two writers cannot clobber each other.
    fn publish_scroll(&self) {
        let scale = self.renderer.scale_factor();
        // Byte-identical to what the chrome layer writes each frame (both add
        // `waiting_band_h(m.waiting.len())`), so the two writers cannot clobber each other.
        let waiting_count = self.tab_model.with(|m| m.waiting.len());
        let list_top =
            sidebar_layout::header_h(scale) + sidebar_layout::waiting_band_h(waiting_count, scale);
        self.tab_model.set_list_scroll(list_top, self.scroll);
    }

    /// Collapse or restore the sidebar (D6). The order is load-bearing: set the override
    /// (which re-fits the inset) FIRST, then read `grid_size` inside `refit`, then rebuild
    /// the layout from the now-live width, re-clamp the scroll, re-resolve hover under the
    /// stationary pointer, and redraw (the grid geometry changed).
    fn toggle_sidebar(&mut self, collapsed: bool) {
        self.renderer.set_sidebar_collapsed(collapsed);
        crate::settings::refit(&self.renderer, &mut self.tabs);
        let inp = self.sidebar_input();
        self.scroll = sidebar_layout::clamp_scroll(self.scroll, &inp);
        self.publish_scroll();
        self.refresh_hover();
        self.redraw();
        // Remember it (tab-resize D16). Every way of collapsing — this button, the drag,
        // and the chord — lands here, so two controls doing the same thing can never
        // remember differently. `SidebarConfig::collapsed` was documented as "persisted"
        // long before anything wrote it; this is what makes that true.
        self.persist_sidebar();
    }

    /// Write the sidebar's size to `config.toml` (tab-resize D2/D16) — the ONE persistence
    /// site, shared by the drag release, focus loss, the Hide/Restore button, and the
    /// toggle chord, so no two entry points can remember differently.
    ///
    /// A failure warns and is otherwise silent (D14): the session keeps the size on screen,
    /// it simply will not survive a restart. Never reverts — punishing the user for a
    /// read-only filesystem by undoing a gesture that visibly succeeded is worse than not
    /// remembering it.
    fn persist_sidebar(&mut self) {
        let Some(path) = self.config_path.clone() else {
            tracing::warn!("no config path; sidebar size not persisted");
            return;
        };
        // Logical px, and readable even while collapsed — collapsing must not erase the
        // width a restore brings back (D7).
        let pair = (
            self.renderer.sidebar_width_logical(),
            self.renderer.sidebar_collapsed(),
        );
        // Nothing changed since the config we last applied — don't touch the file. A bare
        // click on the grab band (press + release, no motion) would otherwise rewrite
        // `config.toml` for a value already on disk: on a fresh install that CREATES the
        // file out of nothing, and on an existing one it rewrites `340` as `340.0`.
        //
        // Compared against `last_applied_cfg` rather than the write history, so the guard
        // also covers the FIRST gesture of a session, when nothing has been written yet.
        let on_disk = (
            self.last_applied_cfg.sidebar.width_px,
            self.last_applied_cfg.sidebar.collapsed,
        );
        if on_disk == pair && !self.sidebar_override_authoritative {
            return;
        }
        match common::config::save_sidebar(&path, pair.0, pair.1) {
            Ok(()) => {
                // Arm the echo latch ONLY on success (D15/D14×D15a): a failed write leaves
                // the file disagreeing with the override on purpose.
                input::remember_write(&mut self.written_sidebar, pair);
                self.sidebar_override_authoritative = false;
                // Fold the written values into the applied-config baseline, so the echo
                // this write is about to provoke compares equal and is suppressed — while
                // a delivery that ALSO changes any other field still differs, and applies.
                self.last_applied_cfg.sidebar.width_px = pair.0;
                self.last_applied_cfg.sidebar.collapsed = pair.1;
            }
            Err(e) => {
                tracing::warn!("sidebar size not saved: {e}");
                self.written_sidebar.clear();
                self.sidebar_override_authoritative = true;
            }
        }
    }

    /// End an in-flight resize: commit the width and persist it (tab-resize D2/R7.3/R7.6).
    /// Idempotent — clearing `resize_drag` first means a later `Released` after a
    /// focus-loss commit finds nothing armed and does not write a second time.
    fn finish_resize(&mut self) {
        if self.resize_drag.take().is_some() {
            self.persist_sidebar();
        }
    }

    /// Which gesture a left-release belongs to (tab-resize D17/R7.3) — the guard both
    /// `Released` arms read, so their precedence is a tested rule rather than source order.
    fn release_target(&self) -> ReleaseTarget {
        input::release_target(self.resize_drag.is_some(), self.dragging)
    }

    /// Arm a sidebar edge resize at the current pointer (tab-resize D9/D17/R7.1).
    ///
    /// Captures the grab offset against the edge's *live* position, once — `drive_resize`
    /// never recomputes it (R3.4). Clears `dragging` to uphold the D17 invariant: nothing
    /// clears it on `Focused(false)`, so a lost `Released` can leave it stale, and a stale
    /// selection drag racing a resize is the one way both could be armed at once.
    fn begin_resize(&mut self) {
        let edge_x = self.renderer.left_inset();
        self.dragging = false;
        self.resize_drag = Some(ResizeDrag {
            grab_offset: input::resize_grab_offset(self.last_cursor.0, edge_x),
        });
        debug_assert!(
            self.resize_drag.is_some() && !self.dragging,
            "D17 invariant: a resize drag and a selection drag are never both armed"
        );
    }

    /// Advance an in-flight resize to the current pointer (tab-resize D1/D9).
    ///
    /// Order is load-bearing and copied from `toggle_sidebar` (D6): set the width FIRST so
    /// the inset is live, then re-fit the grid, then rebuild the layout from the new width
    /// and re-clamp the scroll. Hover is deliberately NOT re-resolved — the drag owns the
    /// pointer (R7.4).
    fn drive_resize(&mut self, drag: ResizeDrag) {
        let scale = self.renderer.scale_factor();
        let raw = input::resize_raw_width(self.last_cursor.0, drag, scale);
        // The grid size the tabs are currently fitted to. `settings::refit` is the only
        // site that resizes them (D6), so this is always their true dimension.
        let before = self.renderer.grid_size();

        // Collapse / re-expand mid-gesture on the RAW width (D12/R4.4), before applying
        // the clamped one — the mode change is what makes the width meaningful.
        let collapsed = self.collapsed();
        let next = input::next_collapsed(raw, collapsed);
        if next != collapsed {
            self.renderer.set_sidebar_collapsed(next);
        }
        // Only write the width while the sidebar is OPEN — once collapsed there is no
        // meaningful width to track, and writing one would floor the remembered value.
        //
        // **What this does NOT do**, stated plainly because the first version of this
        // comment claimed otherwise: it does not preserve a pre-drag width across a
        // drag-collapse. `clamp` floors every `raw < 260` at 260 while `next_collapsed`
        // only fires below 220, so a continuous drag from 400 writes 260 all the way
        // through the [220, 260) hysteresis band before the collapse happens. Restoring
        // after a drag-collapse therefore yields **260**, not 400.
        //
        // That is the honest consequence of the pointer having genuinely been dragged to
        // the minimum on its way down — the Hide button behaves identically if you drag to
        // 260 first — but it is a real behavioural choice, recorded in the spec's Known
        // Gaps rather than papered over here.
        if !next {
            self.renderer
                .set_sidebar_width(input::clamp_sidebar_width(raw));
        }

        // Refit ONLY when the column/row count actually changed (D13/R5.1). This is a
        // correctness guard, not an optimisation: `Grid::resize` reallocates every tab's
        // cells AND clears the selection, so an unguarded per-pixel refit would wipe the
        // user's selection on every pointer move and fire a SIGWINCH at zsh on every tab.
        // The band still follows the pointer every frame — only the grid steps by columns.
        let after = self.renderer.grid_size();
        if input::needs_refit(before, after) {
            crate::settings::refit(&self.renderer, &mut self.tabs);
        }

        let inp = self.sidebar_input();
        self.scroll = sidebar_layout::clamp_scroll(self.scroll, &inp);
        self.publish_scroll();
        self.redraw();
    }

    /// Re-resolve hover at `last_cursor` after the layout moved under a stationary pointer
    /// (D8): push the raw cursor to the chrome layer (undebounced — it must be exact for a
    /// button edge) and the resolved card index to the row layer (region-debounced).
    fn refresh_hover(&mut self) {
        let inp = self.sidebar_input();
        let collapsed = self.collapsed();
        self.chrome_handle
            .set_cursor(Some([self.last_cursor.0 as f32, self.last_cursor.1 as f32]));
        // ONE hit_test feeds every hover output (D8, extended by tab-resize D11/R2.3):
        // the card highlight AND the mouse cursor shape. Resolving them separately would
        // let the highlight and the cursor disagree about the same pointer.
        let region = sidebar_region(self.last_cursor, &inp, self.scroll, collapsed);
        let hit = hover_card(region);
        if hit != self.last_hit {
            self.last_hit = hit;
            self.tab_model.set_hovered(hit);
        }
        self.set_cursor_icon(hover_cursor(region));
        self.window.request_redraw();
    }

    /// Set the window's mouse cursor, skipping the call when it is already that shape
    /// (tab-resize D11/R2.1). Every `CursorMoved` passes through here, so the guard keeps
    /// an unchanged hover from issuing a platform call per motion event.
    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.cursor_icon != icon {
            self.cursor_icon = icon;
            self.window.set_cursor(icon);
        }
    }

    /// Clear all hover state — the pointer left the window or focus was lost with it still
    /// inside (Cmd-Tab fires no `CursorLeft`), else the highlight stays lit (D8).
    fn clear_hover(&mut self) {
        self.chrome_handle.set_cursor(None);
        if self.last_hit.is_some() {
            self.last_hit = None;
            self.tab_model.set_hovered(None);
        }
        // The pointer is gone, so the resize affordance is too (R2.4). A drag in flight
        // overrides this — see the `CursorLeft` arm, which does not call us (D17/R7.5).
        self.set_cursor_icon(CursorIcon::Default);
        self.window.request_redraw();
    }

    /// Handle a tab chord (R3.2). Returns false if the app should exit (last tab
    /// closed).
    fn do_tab_action(&mut self, action: TabAction) -> bool {
        // A tab switch abandons any in-flight composition (R8.1/R8.2): Cmd+T is a
        // Named key + modifier, so it sails straight through the composing gate
        // (R6.3) and is reachable by keyboard, not just by a sidebar click.
        self.cancel_composition();
        // A tab switch also abandons an in-flight RENAME (sticky-tab-title R2.5/D8):
        // the user's attention moved, so the half-typed name is discarded rather than
        // committed. Same funnel as the composition cancel above — every switch (chord,
        // click, Cmd+number, waiting row) passes through here, so there is one place this
        // rule has to hold. The switch itself still proceeds; the chord is not swallowed.
        self.overlays.command(&OverlayCommand::Cancel);
        // …and dismisses an open card menu (R3.8), for the same reason: a popup anchored
        // to a card the user just navigated away from is stale chrome.
        self.close_card_menu();
        // A tab switch ends the in-flight wheel gesture, so the unspent sub-line
        // remainder does not follow the user into the next tab (terminal-scroll D3).
        // Load-bearing, not hygiene: an unspent +0.9 from here would make the very
        // first small wheel event over the next grid jump a whole line.
        self.wheel_accum = 0.0;
        match action {
            TabAction::New => {
                let (cols, rows) = self.renderer.grid_size();
                if let Err(e) = self.tabs.open(cols, rows) {
                    tracing::warn!("open tab failed: {e}");
                }
            }
            TabAction::CloseActive => {
                if !self.tabs.close_active() {
                    return false; // last tab → exit
                }
            }
            TabAction::Next => self.tabs.next(),
            TabAction::Prev => self.tabs.prev(),
            TabAction::Jump(i) => self.tabs.jump(i),
        }
        self.after_roster_change();
        true
    }

    /// Common tail for "a registered overlay or handler consumed this key" — shared by
    /// BOTH dispatch branches so the D8a.1 invariant is enforced in one place, not in one
    /// of two symmetric sites.
    ///
    /// If a key was claimed while a card menu was open, that claim opened or owns a modal
    /// interaction, and a stale popup underneath it is exactly the two-modal state D8a
    /// forbids. Unconditional on `consumed` rather than sniffing which chord it was: an
    /// overlay that takes a key is asserting it owns the interaction. (Ordinary typing is
    /// `Passthrough` and leaves the menu up — R3.8 names only outside-click, Esc and tab
    /// chords as dismissals.)
    fn finish_consumed_key(&mut self, menu_was_open: bool, wants_redraw: bool) {
        if menu_was_open {
            self.close_card_menu();
        }
        if wants_redraw {
            self.redraw();
        }
    }

    /// The bookkeeping every roster/active-tab change owes (extracted so the card menu's
    /// `Close` cannot drift from Cmd+W — sticky-tab-title R5.3, which D4 promises parity
    /// with). Called by `do_tab_action` and by every close-by-id path.
    ///
    /// Skipping any of this is silently harmful rather than obviously broken: without the
    /// re-point, the ai-monitor keeps tailing a **dead session's** cwd; without the
    /// re-clamp, the sidebar keeps a scroll offset the shrunken list no longer has;
    /// without the acknowledge, the newly-active tab's waiting signal never clears.
    fn after_roster_change(&mut self) {
        // Becoming the active tab is the "confirmation" that clears its waiting signals
        // (D12/R2.1). Every path that changes the active tab — click, chord, Cmd+number
        // jump, or a click on a waiting row — funnels through here. Acknowledge BEFORE
        // rebuilding the model so the just-activated session's item is already gone from
        // the band (clicking a waiting row makes it disappear, R2.1). This ends in
        // `redraw()`, so the cleared band repaints.
        self.waiting.acknowledge(self.tabs.active().id);
        // Re-evaluate every tab's level now that the active tab changed. Load-bearing for
        // the common flow: you are WATCHING a tab when its permission dialog opens, so the
        // rising edge is suppressed (D15) and left pending; switching away is what makes it
        // raisable. An agent blocked on that dialog emits no further output and changes no
        // metadata, so this is the ONLY event that fires — without it the tab-you-just-left
        // stays invisible for as long as it is blocked.
        self.push_agent_states();
        self.sync_tab_model();
        // Reveal the (possibly newly-appended or below-the-fold) active tab and re-clamp
        // the offset — a tab close shrinks the content (D6/D10). Then re-resolve hover.
        let inp = self.sidebar_input();
        self.scroll = sidebar_layout::scroll_to_reveal(self.tabs.active_index(), self.scroll, &inp);
        self.publish_scroll();
        self.refresh_hover();
        // Re-point cwd-derived overlays (ai-monitor) at the new active tab (R3.1).
        let cwd = self.tabs.active_cwd(&self.tabmeta_snapshot);
        self.overlays
            .notify_active_changed(cwd, self.tabs.active_shell_pid());
        self.redraw();
    }

    /// Map the latest cursor position to a live-viewport cell, or `None` over the
    /// sidebar/top band (selection-input). The one geometry the event loop performs,
    /// via the pure `pixel_to_cell` helper + the renderer's cell/inset accessors.
    fn cursor_cell(&self) -> Option<(u16, u16)> {
        let (cols, rows) = self.tabs.active().terminal.dimensions();
        pixel_to_cell(
            self.last_cursor,
            self.renderer.cell_size(),
            self.renderer.left_inset(),
            self.renderer.top_inset(),
            cols,
            rows,
        )
    }

    /// Recompute the ⌘-hover link highlight for one edge (hyperlinks D24).
    ///
    /// The three triggers are distinct, not interchangeable:
    /// - `Pointer` skips the scan when the pointer never left its cell (R2.4);
    /// - `Modifier` always recomputes **and re-arms** — it is first-hand evidence of
    ///   what ⌘ is doing;
    /// - `Snapshot` always recomputes but must **not** re-arm, or a PTY burst would
    ///   undo the focus-loss disarm (see [`HoverState::on_snapshot`]).
    ///
    /// The redraw is the **bare** `window.request_redraw()`, never [`redraw`](Self::redraw):
    /// a highlight change is not a content change, and marking the snapshot stale
    /// would re-clone the grid on every ⌘ tap — exactly the idle-CPU cache this
    /// feature promised not to disturb (R3.6).
    fn refresh_link_hover(&mut self, trigger: LinkTrigger) {
        let cell = self.cursor_cell();
        let input = HoverInput {
            cmd_held: self.mods.super_key(),
            pointer_inside: self.pointer_inside,
            cell,
            // The snapshot on *screen* — not `terminal.snapshot()`, which would clone
            // the whole cell vector on every pointer move (D11).
            snapshot: self.last_snapshot.as_ref(),
        };
        let update = match trigger {
            LinkTrigger::Pointer => self.link_hover.on_pointer(input),
            LinkTrigger::Modifier => self.link_hover.on_modifier(input),
            LinkTrigger::Snapshot => self.link_hover.on_snapshot(input),
        };
        if let Update::Set(span) = update {
            self.link_handle.set_span(span);
            self.window.request_redraw();
        }
    }

    /// Apply a hover update that came from something other than a recompute.
    fn apply_link_update(&mut self, update: Update) {
        if let Update::Set(span) = update {
            self.link_handle.set_span(span);
            self.window.request_redraw();
        }
    }

    /// The pointer left the window: clear the highlight but keep the gesture armed
    /// (D24.3 — the window still has focus, so ⌘ is not stale).
    fn clear_link_hover(&mut self) {
        let update = self.link_hover.on_pointer_left();
        self.apply_link_update(update);
    }

    /// The window lost focus: clear the highlight **and** disarm (D24.5).
    fn disarm_link_hover(&mut self) {
        let update = self.link_hover.disarm();
        self.apply_link_update(update);
    }

    /// Scroll the **active** tab's scrollback viewport for one wheel event over the
    /// grid (terminal-scroll D2/D3/D7). Display-only — nothing is written to the PTY
    /// and the shell's cursor never moves (`scrollback` D4).
    ///
    /// Apply-then-compare rather than pre-checking the bounds: `scroll_up`/`scroll_down`
    /// already clamp, so re-reading `view_offset` is the cheapest way to learn whether
    /// anything actually moved. Wheeling at the oldest retained row or at the live
    /// bottom is then free — no snapshot re-clone, no frame.
    ///
    /// The redraw goes through [`redraw`](Self::redraw), *not* the bare
    /// `window.request_redraw()` the sidebar path uses: a viewport change is a real
    /// content change, and without marking `snapshot_dirty` the next frame would
    /// re-present the cached snapshot and the screen would never move (D7).
    ///
    /// Swallowed whole while a drag holds the pointer (D6) — **including** the
    /// accumulator, which is deliberately not fed: banking a suppressed event would
    /// make the scroll lurch on the first real one after release.
    fn wheel_grid(&mut self, delta: WheelDelta) {
        if self.dragging {
            return;
        }
        let cell_h = self.renderer.cell_size().1;
        let n = wheel_lines(delta, cell_h, &mut self.wheel_accum);
        if n == 0 {
            return; // sub-line residue banked, or a rejected delta (D9)
        }
        // ONE exhaustive match, deliberately with no early return: the two
        // destinations are mutually exclusive, and an `if consumed { return }` shape
        // made "forget the return" a silent bug that writes to the PTY *and* scrolls
        // the viewport — a bug no test could catch, since `wheel_grid` needs a real
        // window. As a match, that state is simply unrepresentable.
        //
        // An application that asked for the wheel gets it (alt-screen-scroll): that
        // is what makes the wheel work inside Claude Code, which enables `?1000`/
        // `?1002`/`?1003` and scrolls its own transcript from the report. Everyone
        // else — including an untracked alt-screen app — keeps the scrollback
        // viewport (`terminal-scroll` R5.1).
        let term = &self.tabs.active().terminal;
        let cell = self.cursor_cell().unwrap_or((0, 0));
        match wheel_bytes(
            term.mouse_protocol().is_on(),
            term.mouse_encoding(),
            n,
            cell,
        ) {
            Some(bytes) => {
                if let Err(e) = self.tabs.active_mut().write(&bytes) {
                    tracing::warn!("pty write failed: {e}");
                }
            }
            None => {
                let term = &mut self.tabs.active_mut().terminal;
                let before = term.view_offset();
                if n > 0 {
                    term.scroll_up(n as usize); // positive y = into history (D2)
                } else {
                    // `unsigned_abs` so `i32::MIN` cannot overflow the negation.
                    term.scroll_down(n.unsigned_abs() as usize);
                }
                let moved = term.view_offset() != before;
                if moved {
                    self.redraw();
                }
            }
        }
    }

    /// Return tab `id`'s viewport to the live bottom. The raw move — the drag rules
    /// live in the two callers below.
    ///
    /// Routed **by tab id, not "whoever is active"**: an `Ime::Commit` can land in the
    /// tab the composition began in after the user switched away, and it is that tab's
    /// reading position that must follow its own bytes. Only a change to the *visible*
    /// tab is worth a frame.
    ///
    /// Deliberately does **not** touch `wheel_accum` (D3): this can fire for a tab the
    /// user is not looking at, and clearing the accumulator would eat the sub-line
    /// residue of a wheel gesture happening in the tab they *are* looking at.
    fn snap_tab_to_live(&mut self, id: TabId) {
        let moved = match self.tabs.by_id_mut(id) {
            Some(s) if !s.terminal.at_bottom() => {
                s.terminal.scroll_to_bottom();
                true
            }
            _ => false,
        };
        if moved && self.tabs.is_active(id) {
            self.redraw();
        }
    }

    /// User input reached tab `id`'s shell — snap it to live (terminal-scroll D4).
    /// While a drag holds the pointer the move is suppressed (D6) and *latched*, so the
    /// release can replay it; without the latch, typing mid-drag would leave the view
    /// frozen with the command already sent.
    fn snap_after_input(&mut self, id: TabId) {
        if self.dragging {
            self.drag_latch.input_while_dragging = Some(id);
            return;
        }
        self.snap_tab_to_live(id);
    }

    /// [`snap_after_input`](Self::snap_after_input) for the active tab — the common
    /// case (a typed key, a paste).
    fn snap_active_after_input(&mut self) {
        let id = self.tabs.active().id;
        self.snap_after_input(id);
    }

    /// Output advanced tab `id`'s grid from within one notch of the bottom — snap it
    /// (terminal-scroll D12). Suppressed and latched mid-drag, same as D4's snap, since
    /// `scrollback` D2's auto-increment would otherwise drift the offset past the floor
    /// and disarm the self-heal for good.
    fn snap_after_output(&mut self, id: TabId) {
        if self.dragging {
            self.drag_latch.advanced_while_dragging = Some(id);
            return;
        }
        self.snap_tab_to_live(id);
    }

    /// Dispatch a key through the seam, then default input (R3.3/R4). Returns
    /// false if the app should exit.
    fn on_key(&mut self, input: KeyInput) -> bool {
        // Esc dismisses an open card menu before anything else sees it
        // (sticky-tab-title R3.8) — ahead of the overlays and of `encode_key`, which
        // would otherwise send the ESC on to the shell while the popup stayed up.
        if self.open_menu.is_some()
            && matches!(
                input.key,
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
            )
        {
            self.close_card_menu();
            return true;
        }
        // Any OTHER key that an overlay claims also dismisses the menu (D8a.1). A chord
        // that opens a modal surface — Cmd+Shift+R, Cmd+Shift+M — would otherwise stack
        // it on top of a live popup. Checked after dispatch below via `menu_was_open`,
        // because whether a key is claimed is only known once the overlays have seen it.
        let menu_was_open = self.open_menu.is_some();
        // Seam data for handlers, read from the active tab BEFORE the mutable
        // `writer` borrow (D9): bracketed-paste mode (text-paste) + the selection
        // text (copy-clipboard). The full-grid `snapshot()` clone is paid ONLY when
        // a selection actually exists, so the common keystroke stays cheap.
        let bracketed = self.tabs.active().terminal.bracketed_paste();
        let selection_text = if self.tabs.active().terminal.selection().is_some() {
            self.tabs.active().terminal.snapshot().selection_text()
        } else {
            None
        };
        // The seam's writer is wrapped in a `WriteProbe` at both dispatch points
        // (terminal-scroll D4): a handler that sends the user's bytes to the shell —
        // Cmd+V paste is the shipped example — must snap a scrolled-back viewport, and
        // the probe detects that without `clipboard.rs` (or any future handler) knowing
        // this feature exists. Classifying which *key* was pressed cannot work here:
        // paste is consumed two steps before the default path.
        //
        // 1) Visible overlays first (R4.1/R4.2).
        {
            let active = self.tabs.active_index();
            let count = self.tabs.len();
            let cwd = self.tabs.active_cwd(&self.tabmeta_snapshot);
            // The active tab's identity + the label it is CURRENTLY SHOWING, for the
            // rename chord (R2.1) — read before the mutable writer borrow, like
            // `bracketed`/`selection_text`. The displayed label, not `user_title`: a tab
            // that was never named still shows a name, and that is what you edit.
            let active_tab = self.tabs.active().id;
            // Resolving the displayed label costs a `proc_pidinfo` (for the cwd) plus a
            // metadata sync, so it is NOT paid per keystroke: the rename chord is
            // Cmd+Shift+R, so anything without both modifiers cannot be it. The gate is a
            // modifier test, no syscall.
            let active_user_title = if input.mods.super_key() && input.mods.shift_key() {
                self.displayed_label(active_tab)
            } else {
                None
            };
            let shell_pid = self.tabs.active_shell_pid();
            let mut probe = WriteProbe::new(&mut self.tabs.active_mut().writer);
            let mut ctx = AppCtx::new(&mut probe, count, active, cwd)
                .with_active_tab(active_tab, active_user_title)
                .with_shell_pid(shell_pid)
                .with_bracketed_paste(bracketed)
                .with_selection_text(selection_text.clone());
            let consumed = self.overlays.dispatch(&input, &mut ctx) == Handled::Consumed;
            let wants_redraw = ctx.redraw_requested();
            // Tab mutations an overlay asked for (sticky-tab-title R1.4) — collected here
            // and applied below, once the `&mut Tabs` borrow this block holds is released.
            let requests = ctx.take_requests();
            let wrote = probe.wrote();
            if wrote {
                self.snap_active_after_input();
            }
            if !self.apply_requests(requests) {
                return false; // the last tab closed — the app exits
            }
            if consumed {
                self.finish_consumed_key(menu_was_open, wants_redraw);
                return true;
            }
        }
        // 2) Registered input handlers before default (R3.3).
        {
            let active = self.tabs.active_index();
            let count = self.tabs.len();
            let cwd = self.tabs.active_cwd(&self.tabmeta_snapshot);
            // The active tab's identity + the label it is CURRENTLY SHOWING, for the
            // rename chord (R2.1) — read before the mutable writer borrow, like
            // `bracketed`/`selection_text`. The displayed label, not `user_title`: a tab
            // that was never named still shows a name, and that is what you edit.
            let active_tab = self.tabs.active().id;
            // Resolving the displayed label costs a `proc_pidinfo` (for the cwd) plus a
            // metadata sync, so it is NOT paid per keystroke: the rename chord is
            // Cmd+Shift+R, so anything without both modifiers cannot be it. The gate is a
            // modifier test, no syscall.
            let active_user_title = if input.mods.super_key() && input.mods.shift_key() {
                self.displayed_label(active_tab)
            } else {
                None
            };
            let shell_pid = self.tabs.active_shell_pid();
            let mut probe = WriteProbe::new(&mut self.tabs.active_mut().writer);
            let mut ctx = AppCtx::new(&mut probe, count, active, cwd)
                .with_active_tab(active_tab, active_user_title)
                .with_shell_pid(shell_pid)
                .with_bracketed_paste(bracketed)
                .with_selection_text(selection_text);
            let consumed = self.router.dispatch(&input, &mut ctx) == Handled::Consumed;
            let wants_redraw = ctx.redraw_requested();
            let requests = ctx.take_requests();
            let wrote = probe.wrote();
            if wrote {
                self.snap_active_after_input();
            }
            if !self.apply_requests(requests) {
                return false; // the last tab closed — the app exits
            }
            if consumed {
                self.finish_consumed_key(menu_was_open, wants_redraw);
                return true;
            }
        }
        // 3a) The sidebar toggle chord (tab-resize D17a), intercepted before the shell for
        // the same reason tab chords are: `encode_key` swallows Cmd only for its `key_seq`
        // table, so a `Key::Character` chord would otherwise reach zsh through the text
        // fallback. This is the recovery path D16 owes — persisting `collapsed` removes
        // restart-as-escape-hatch, and D12 makes an accidental collapse a one-flick
        // gesture, so a hidden sidebar must be reachable without hunting for a 28px button.
        if is_sidebar_toggle_chord(&input.key, &input.mods) {
            let collapsed = self.collapsed();
            self.toggle_sidebar(!collapsed); // persists, like every other entry point
            return true;
        }
        // 3) Tab chords, intercepted before the shell (D5/R3.2).
        if let Some(action) = tab_action(&input.key, &input.mods) {
            return self.do_tab_action(action);
        }
        // 4) Default: encode to the active shell only (R3.1). DECCKM is
        // per-session (D7), so read the active tab's mode *before* taking the
        // mutable borrow for the write.
        let app_cursor = self.tabs.active().terminal.app_cursor_keys();
        if let Some(bytes) = encode_key(&input.key, input.text.as_deref(), input.mods, app_cursor) {
            match self.tabs.active_mut().write(&bytes) {
                // The user typed something the shell will echo — bring the view back to
                // where that echo will appear (terminal-scroll D4). Unconditional: this
                // rule has no `SNAP_FLOOR`, unlike the output snap. Keys that never get
                // here — Cmd+C, an overlay hotkey, a tab chord, a modifier held alone —
                // leave a scrolled view standing, since those are things a user does
                // *while reading history*.
                Ok(()) => self.snap_active_after_input(),
                Err(e) => tracing::warn!("pty write failed: {e}"),
            }
        }
        true
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(running) => self.inner = Some(running),
            Err(e) => {
                tracing::error!("failed to start terminal: {e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        match event {
            UserEvent::PtyOutput(id) => {
                // Advance the originating tab's grid; redraw only if active (R1.2).
                let is_active = inner.tabs.is_active(id);
                let active_id = inner.tabs.active().id;
                // Drain, and pull any OSC 133 command-end the shell emitted on this
                // (possibly background) session's PTY (T1 → T3).
                let mut command_end = None;
                let mut title_changed = false;
                // Sampled INSIDE the closure and BEFORE the drain (terminal-scroll
                // D12/C8): `scrollback` D2's auto-increment runs in `feed_scroll`, so a
                // viewport parked at 1 that receives a 10-line burst already reads 11 by
                // the time `drain` returns. Reading it afterwards would silently disable
                // the floor for every burst bigger than SNAP_FLOOR — i.e. exactly the
                // bursts that matter.
                let mut was_near_bottom = false;
                let advanced = inner
                    .tabs
                    .by_id_mut(id)
                    .map(|s| {
                        was_near_bottom = snap_on_output(s.terminal.view_offset());
                        let got = s.drain();
                        command_end = s.take_command_end();
                        title_changed = s.take_title_dirty();
                        got
                    })
                    .unwrap_or(false);
                // Output that advanced the grid returns a near-bottom viewport to live
                // (D12). Gated on `advanced` — the flag already in hand — rather than on
                // the bare wake, which fires for chunks that change nothing. A chunk
                // carrying only an OSC 133 marker or an OSC 2 title set still counts as
                // advanced, so this accepts one benign false positive: a viewport within
                // one notch of the bottom snaps on a title set. That is where the user
                // almost certainly meant to be anyway.
                if advanced && was_near_bottom {
                    inner.snap_after_output(id);
                }
                // A manual OSC-2 label change refreshes the sidebar rows so the new label
                // shows immediately — even for a background tab (tab-identity).
                if title_changed {
                    inner.sync_tab_model();
                }
                let needs_redraw = (advanced && is_active) || title_changed;
                // Feed the command-end into the waiting model (T3). A background
                // tab's completion/error must still repaint the sidebar band even
                // though its grid output does not (so redraw when the set changed).
                let mut band_changed = false;
                if let Some(end) = command_end {
                    if inner
                        .waiting
                        .on_command_end(id, active_id, end, Instant::now())
                    {
                        band_changed = true;
                    }
                }
                // Re-evaluate the screen-derived waiting level for THIS tab's new output.
                //
                // This trigger is load-bearing, not belt-and-braces. The other caller of
                // `push_agent_states` runs on `OverlayPending`, which the tabmeta worker
                // only sends when `TabMeta` actually CHANGED (`publish_if_changed` skips the
                // wake otherwise, deliberately — an unconditional 2 Hz wake was the G1
                // idle-CPU debt). A pending permission dialog changes no metadata by
                // definition — the log is exactly what it is blind to — so that path stays
                // silent for as long as the prompt is up. PTY output is the one event that
                // does fire when the dialog appears, because drawing it IS the output.
                //
                // Gated on `advanced`, which `Session::drain` sets for any bytes received
                // (not "cells changed") — deliberately the permissive direction: a dialog
                // is always drawn by bytes, so it can never be gated out. Do NOT tighten
                // `drain` to a cell-diff without revisiting this, or the silent-trigger bug
                // comes straight back.
                if advanced && inner.push_agent_states_for(id) {
                    band_changed = true;
                }
                // Republish the band (rows + shifted list_top) before the redraw (T4).
                if band_changed {
                    inner.refresh_waiting_band();
                }
                if needs_redraw || band_changed {
                    inner.redraw();
                }
            }
            UserEvent::ConfigReloaded => {
                // Drain to the newest config (coalesce rapid saves), then apply.
                let mut latest = None;
                while let Ok(cfg) = inner.config_rx.try_recv() {
                    latest = Some(cfg);
                }
                if let Some(cfg) = latest {
                    // Is this our own write coming back (tab-resize D15, as amended)? The
                    // atomic rename IS a change event, so the round-trip is guaranteed —
                    // and `settings::apply` → `apply_config` rebuilds the entire glyph
                    // atlas. Paying that for values we already applied is absurd.
                    //
                    // The comparison is against the WHOLE applied config, not the sidebar
                    // pair alone. Pair-only suppression shipped once and killed live
                    // settings outright: the latch stays armed, the file keeps matching
                    // it, and every subsequent reload of every unrelated field was
                    // silently dropped for the rest of the session.
                    if input::suppress_delivery(&cfg, &inner.last_applied_cfg) {
                        return;
                    }
                    // Something really changed. The sidebar pair still decides precedence:
                    // if it differs from what we last wrote, a human edited it and the
                    // file wins (D15a); if it matches, this delivery is about some other
                    // field and our override must stand.
                    let delivered = (cfg.sidebar.width_px, cfg.sidebar.collapsed);
                    let delivery = input::classify_delivery(delivered, &inner.written_sidebar);
                    // A real human edit wins: drop the sticky override so the file is
                    // authoritative again (D15a) — otherwise these two fields would be the
                    // only ones in config.toml that stop live-reloading after a drag.
                    if input::should_clear_override(delivery, inner.sidebar_override_authoritative)
                    {
                        inner.renderer.clear_sidebar_override();
                    }
                    crate::settings::apply(
                        &cfg,
                        &mut inner.renderer,
                        &mut inner.tabs,
                        &inner.preedit_handle,
                    );
                    // A reload can change show_git_branch (card height) or width_px (inset),
                    // both of which move max_scroll; refresh the cached section, re-clamp,
                    // and re-resolve hover under the stationary pointer (D10/D8).
                    inner.sidebar_cfg = cfg.sidebar.clone();
                    // The new baseline for echo suppression — set AFTER apply, so a
                    // delivery that fails to apply cannot be mistaken for one that did.
                    inner.last_applied_cfg = cfg.clone();
                    let inp = inner.sidebar_input();
                    inner.scroll = sidebar_layout::clamp_scroll(inner.scroll, &inp);
                    inner.publish_scroll();
                    inner.refresh_hover();
                    inner.redraw();
                }
            }
            UserEvent::OverlayPending => {
                // An overlay (ai-monitor) tailed new data — fold it in + redraw (R1.3).
                let overlay_changed = inner.overlays.poll_all();
                // The tab-metadata worker also wakes the loop through this path; sync
                // the fresh agent states into the waiting model (rising-edge Waiting
                // detection, T3) and redraw if the waiting band changed.
                let waiting_changed = inner.feed_agent_states();
                // Republish the band (rows + shifted list_top) before the redraw (T4).
                if waiting_changed {
                    inner.refresh_waiting_band();
                }
                if overlay_changed || waiting_changed {
                    inner.redraw();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ModifiersChanged(m) => {
                inner.mods = m.state();
                // Trigger 1/2 (hyperlinks D24): ⌘ down lights the link under the
                // pointer, ⌘ up clears it. Also re-arms after a focus loss.
                inner.refresh_link_hover(LinkTrigger::Modifier);
            }

            WindowEvent::CursorMoved { position, .. } => {
                inner.last_cursor = (position.x, position.y);
                inner.pointer_inside = true; // fresh coordinates (hyperlinks R2.8)
                                             // A resize drag OWNS the pointer (tab-resize D17/R7.4): drive the width and
                                             // return. Hover must not be re-resolved mid-gesture — once D12 collapses the
                                             // sidebar, `hit_test` short-circuits to Restore-only and would strip the
                                             // very state the drag is standing on. Link hover is skipped for the same
                                             // reason: while you are dragging the sidebar edge you are not pointing at
                                             // text, so nothing should light up under ⌘.
                                             //
                                             // `pointer_inside` is set FIRST, above — the coordinates really are fresh,
                                             // and that flag is the hyperlink feature's, not this gesture's to withhold.
                if let Some(drag) = inner.resize_drag {
                    inner.drive_resize(drag);
                    return;
                }
                inner.refresh_hover(); // one hit_test feeds both hover fields (D8)
                inner.refresh_link_hover(LinkTrigger::Pointer); // trigger 3 (D24)

                // Left-drag selection (selection-input): once the focus leaves the
                // anchor cell, set the active grid's selection live. Keying `moved`
                // on the *cell* (not raw px) makes an ordinary click jitter-safe.
                if inner.dragging {
                    if let Some(focus) = inner.cursor_cell() {
                        if focus != inner.drag_anchor {
                            inner.drag_moved = true;
                        }
                        if inner.drag_moved {
                            let sel = Selection {
                                anchor: inner.drag_anchor,
                                focus,
                            };
                            inner.tabs.active_mut().terminal.set_selection(sel);
                            inner.redraw();
                        }
                    }
                }
            }

            // A resize drag survives the pointer leaving the window (tab-resize D17/R7.5):
            // macOS keeps delivering held-button moves outside the frame, so tracking
            // continues and the clamp handles the excursion. `clear_hover` is what this arm
            // withholds — it would reset the ColResize cursor mid-gesture, overriding D11
            // for the duration.
            //
            // The hyperlink half still runs: the pointer really has left, so a later ⌘ must
            // not light a link nobody is pointing at (hyperlinks D24). Neither call touches
            // the cursor icon, so the resize is unaffected.
            WindowEvent::CursorLeft { .. } if inner.resize_drag.is_some() => {
                inner.pointer_inside = false;
                inner.clear_link_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                inner.clear_hover();
                // Trigger 3, the leaving half (hyperlinks D24): nothing is under the
                // pointer, and `last_cursor` keeps its stale value — so the flag is
                // what stops a later ⌘ from lighting a link nobody is pointing at.
                inner.pointer_inside = false;
                inner.clear_link_hover();
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Resolve the press through the sibling's ONE ordered hit_test (D1) — the
                // app performs no geometry (SD2). Cards + new-tab route through the same
                // path as chords (D2); hide/restore toggle; inert regions are swallowed.
                let inp = inner.sidebar_input();
                let collapsed = inner.collapsed();
                let action = sidebar_press(inner.last_cursor, &inp, inner.scroll, collapsed);
                // Every press disarms the link gesture first (hyperlinks D22); only
                // the grid fallthrough below re-arms it. A ⌘-click the sidebar claims
                // therefore keeps its existing meaning and opens nothing (R4.12).
                inner.link_cmd_at_press = false;
                // Any press that is not ON the open menu dismisses it (sticky-tab-title
                // R3.8) — including a press over the grid, which resolves to no sidebar
                // action at all. The press still does its own job afterwards: dismissing
                // is not swallowing, the same rule D8a.2 gives tab chords.
                if press_dismisses_modals(action) {
                    inner.close_card_menu();
                    // …and the rename field, which D8 gives the same rule ("a sidebar
                    // click cancels the edit"). Enforcing it only for the menu left the
                    // field open behind a press that dismissed everything else — and
                    // collapsing the sidebar then hid a field that was still consuming
                    // EVERY keystroke, with nothing on screen to explain why. One press,
                    // one dismissal rule, both surfaces.
                    //
                    // Safe for the double-click path: this runs BEFORE the action, and
                    // `begin_rename_row` opens the field afterwards.
                    inner.overlays.command(&OverlayCommand::Cancel);
                }
                if let Some(action) = action {
                    match action {
                        SidebarAction::Switch(i) => {
                            // A second click on the SAME card opens the rename field on it
                            // (sticky-tab-title): the title is edited by clicking the title,
                            // not only through the menu. The first click still switches —
                            // this adds a gesture, it does not replace one.
                            let now = inner.epoch.elapsed().as_millis();
                            let double = is_double_click(inner.last_card_click, i, now);
                            inner.last_card_click = Some((i, now));
                            if !inner.do_tab_action(TabAction::Jump(i)) {
                                event_loop.exit();
                            } else if double {
                                inner.begin_rename_row(i);
                            }
                        }
                        SidebarAction::SwitchWaiting(i) => {
                            // Resolve the band row to its tab index, then switch — which
                            // acknowledges + clears the item (D12/R2.1). A stale row (tab
                            // gone) is a no-op.
                            if let Some(idx) = inner.waiting_row_tab_index(i) {
                                if !inner.do_tab_action(TabAction::Jump(idx)) {
                                    event_loop.exit();
                                }
                            }
                        }
                        SidebarAction::NewTab => {
                            if !inner.do_tab_action(TabAction::New) {
                                event_loop.exit();
                            }
                        }
                        SidebarAction::Collapse => inner.toggle_sidebar(true),
                        SidebarAction::Expand => inner.toggle_sidebar(false),
                        SidebarAction::OpenMenu(i) => inner.open_card_menu(i),
                        SidebarAction::MenuItem(i) => {
                            if !inner.activate_menu_item(i) {
                                event_loop.exit();
                            }
                        }
                        SidebarAction::BeginResize => inner.begin_resize(),
                        SidebarAction::Inert => {}
                    }
                } else if let Some(cell) = inner.cursor_cell() {
                    // Not a sidebar region and over a real grid cell → anchor a
                    // selection and arm the drag (selection-input). Nothing is shown
                    // until the focus leaves this cell; a press in the dead band
                    // (cursor_cell → None) arms nothing.
                    inner.drag_anchor = cell;
                    inner.dragging = true;
                    inner.drag_moved = false;
                    // Sample ⌘ at press (hyperlinks D22). The drag is armed exactly as
                    // before either way — ⌘ adds a gesture here, it removes none.
                    inner.link_cmd_at_press = inner.mods.super_key();
                    // Start a fresh latch (terminal-scroll D6). The drag freezes the
                    // viewport while `scrollback` D2's auto-increment keeps drifting it,
                    // so any snap raised in that window is recorded here and replayed on
                    // release rather than lost.
                    inner.drag_latch = DragLatch::default();
                }
            }

            // End a resize (tab-resize D17/R7.3). A SEPARATE arm placed BEFORE the
            // selection one — not a `||`-widened guard on it: Rust runs exactly one match
            // arm, so a widened guard would swallow the selection release too if both were
            // somehow armed, leaving `dragging` stuck true forever. Two arms, two guards,
            // each clearing only its own state.
            //
            // Fires wherever the pointer is, including outside the window and over the
            // grid, because the drag captured it at press. T7 commits + persists here.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if inner.release_target() == ReleaseTarget::Resize => {
                inner.finish_resize();
                // The pointer may have ended up somewhere new while the drag owned it, and
                // the sidebar edge has moved — re-resolve hover and the cursor icon now
                // that the gesture no longer suppresses them.
                inner.refresh_hover();
                // Deliberately NOT an link-open path: dragging the sidebar edge is not a
                // click on text, so `url_to_open` is never consulted here (hyperlinks D19
                // keys on the selection drag's own state, which a resize never arms).
            }

            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if inner.release_target() == ReleaseTarget::Selection => {
                // Decide the open before anything is mutated (hyperlinks D19): what
                // opens is whatever the handle is lit with *at this instant*, which
                // D24 has kept current through pointer, modifier, and output edges.
                // Nothing is re-resolved here — that is what makes "what is underlined
                // is exactly what opens" true at the moment of the open, not merely by
                // construction (D20).
                let url = url_to_open(ClickInput {
                    cmd_at_press: inner.link_cmd_at_press,
                    cmd_at_release: inner.mods.super_key(),
                    drag_moved: inner.drag_moved,
                    lit: inner.link_handle.span(),
                });
                // End the drag (selection-input). A plain click — the focus never
                // left the anchor cell — clears any selection; a real drag leaves the
                // selection standing for Cmd+C. Unchanged by the open above: an
                // opening click still clears the selection, exactly as it does today.
                inner.dragging = false;
                if !inner.drag_moved {
                    inner.tabs.active_mut().terminal.clear_selection();
                    inner.redraw();
                }
                // Replay whatever the drag swallowed (terminal-scroll D6). Without this
                // the suppression is permanent: output drifts `view_offset` past
                // SNAP_FLOOR while the button is down, so D12 could never fire again —
                // and a command typed mid-drag would sit in a frozen screen.
                // The snap lands on the tab that earned it, which need not be the
                // active one (R9.4): output can arrive in a background tab, and an IME
                // commit routes to the tab its composition began in.
                let latch = std::mem::take(&mut inner.drag_latch);
                if let Some(id) = drag_release_snap(latch) {
                    inner.snap_tab_to_live(id);
                }
                // Last, so the browser handoff cannot reorder anything above it. The
                // URL goes to macOS and nothing else happens: no PTY write, no grid
                // change, and a failure is logged and silent (hyperlinks D10).
                if let Some(url) = url {
                    inner.opener.open(&url);
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let wd = match delta {
                    MouseScrollDelta::LineDelta(_, y) => WheelDelta::Lines(y),
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels(p.y as f32),
                };
                let inp = inner.sidebar_input();
                let collapsed = inner.collapsed();
                // ONE ordered hit-test owns the routing (terminal-scroll D1). `None` is
                // the grid — which deliberately includes the band above `top_inset`
                // (`cursor_cell` would reject it, making a visible strip inert for no
                // visible reason) and the whole window when the sidebar is collapsed.
                let p = [inner.last_cursor.0 as f32, inner.last_cursor.1 as f32];
                if sidebar_layout::hit_test(p, inner.scroll, &inp, collapsed).is_some() {
                    let new_scroll =
                        wheel_scroll(wd, inner.last_cursor, inner.scroll, &inp, collapsed);
                    if new_scroll != inner.scroll {
                        inner.scroll = new_scroll;
                        inner.publish_scroll();
                        inner.refresh_hover(); // the list moved under the pointer (D8)
                        inner.window.request_redraw(); // sidebar-only change (D7)
                    }
                } else {
                    inner.wheel_grid(wd);
                }
            }

            WindowEvent::Resized(size) => {
                inner.renderer.resize(size.width, size.height);
                crate::settings::refit(&inner.renderer, &mut inner.tabs); // shared refit (D6)
                                                                          // A smaller window shrinks list_viewport.h → max_scroll; re-clamp (D10).
                let inp = inner.sidebar_input();
                inner.scroll = sidebar_layout::clamp_scroll(inner.scroll, &inp);
                inner.publish_scroll();
                inner.refresh_hover();
                inner.redraw();
            }

            WindowEvent::Occluded(occluded) => {
                // Pause the continuous redraw loop while hidden; resume on reveal.
                inner.occluded = occluded;
                if !occluded {
                    inner.redraw();
                }
            }

            WindowEvent::Ime(ime) => inner.on_ime(ime),

            WindowEvent::Focused(false) => {
                // A resize in flight COMMITS and ends here (tab-resize D17/R7.6). Cmd-Tab
                // fires no `CursorLeft` (see `clear_hover`), so focus loss is the only
                // signal that the gesture was abandoned; committing what is on screen
                // beats both silently reverting and leaving a drag armed across an app
                // switch. `finish_resize` disarms, so the eventual `Released` — which may
                // never arrive — cannot write a second time.
                inner.finish_resize();
                // Losing the window abandons the composition too (R8.1/R8.2): the
                // preedit is drawn by *our* layer, so leaving it on screen would
                // show text the shell never received.
                inner.cancel_composition();
                // …and clears the sidebar hover, since a Cmd-Tab away fires no
                // `CursorLeft` while the pointer is still inside (D8).
                inner.clear_hover();
                // Trigger 5 (hyperlinks D24.5) — the same quirk, one layer deeper.
                // winit 0.30.13 already queues a synthetic `ModifiersChanged(empty)`
                // just before this event (window_delegate.rs:225-227), so `mods` is
                // normally clear by now and the link is already unlit. This disarms
                // anyway: it costs nothing, and it is what keeps "a plain click
                // behaves exactly as today" true if that synthetic event ever stops
                // arriving.
                inner.disarm_link_hover();
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } if key_event.state == ElementState::Pressed => {
                // Composing gate, ahead of the whole dispatch chain (R6.3/D8):
                // a character key mid-composition belongs to the IME and must never
                // reach `encode_key`. Named keys still pass (R6.4) — winit forwards
                // arrows/Enter during `Preedit` on purpose.
                if suppress_key(inner.composing, &key_event.logical_key) {
                    return;
                }
                let input = KeyInput {
                    key: key_event.logical_key.clone(),
                    text: key_event.text.as_ref().map(|t| t.to_string()),
                    mods: inner.mods,
                };
                if !inner.on_key(input) {
                    event_loop.exit();
                }
            }

            WindowEvent::RedrawRequested => {
                // Re-clone the grid only on a real content change (D7). On an idle
                // continuous frame `snapshot_dirty` is false, so we reuse the cached
                // snapshot — the renderer then sees an equal snapshot and skips the
                // re-shape too (D5). A fresh snapshot is taken on the first frame and
                // whenever `redraw()` marked the cache stale.
                let took_fresh_snapshot = inner.snapshot_dirty || inner.last_snapshot.is_none();
                if took_fresh_snapshot {
                    inner.last_snapshot = Some(inner.tabs.active().terminal.snapshot());
                    inner.snapshot_dirty = false;
                }
                // Trigger 4 (hyperlinks D24.4): the content under a stationary pointer
                // just moved, and that raises neither `CursorMoved` nor
                // `ModifiersChanged`. Without this the highlight goes stale and the
                // release — which opens only what is lit (D19) — would act on a link
                // that is no longer there.
                //
                // The "not on an idle frame" half of R2.6 is the predicate's job, not
                // this call's indentation: an idle continuous frame re-presents the
                // cached snapshot and has nothing new to resolve against, and ⌘ up
                // skips it outright so streaming output costs nothing.
                if frame_rescans(took_fresh_snapshot, inner.mods.super_key()) {
                    inner.refresh_link_hover(LinkTrigger::Snapshot);
                }
                if let Some(snapshot) = inner.last_snapshot.as_ref() {
                    if let Err(e) = inner.renderer.render(snapshot) {
                        tracing::warn!("render failed: {e}");
                    }
                }
                // macOS/winit(0.30): with on-demand redraws the compositor drops the
                // presented frame the moment the run loop sleeps, so the window only
                // holds content while it is actively drawing (winit#2640; every
                // targeted fix — pre_present_notify, presentsWithTransaction,
                // commit-cycle, buffer-refill — failed on this wgpu/winit/macOS
                // stack). Drive a continuous redraw so a frame is always in flight.
                // Paused while the window is occluded (see `Occluded`) to save power.
                //
                // LOAD-BEARING beyond winit#2640 (D17): the renderer's atlas-eviction
                // cache invalidation (`renderer::Renderer::draw_to` clears
                // `last_snapshot` when the atlas `generation` bumps) never requests a
                // frame itself — it relies on this continuous redraw to actually
                // repaint. Reverting to on-demand redraws silently reopens a permanent
                // stale-glyph corruption path; pair any such change with an explicit
                // redraw request from the renderer.
                if !inner.occluded {
                    inner.window.request_redraw();
                }
            }

            _ => {}
        }
    }
}

fn err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    e: E,
) -> Box<dyn std::error::Error + Send + Sync> {
    e.into()
}

/// How close to the live bottom a viewport must be for new output to snap it back
/// (terminal-scroll D12). Three lines — one wheel notch, `input::LINES_PER_NOTCH` —
/// so it means "you didn't mean to be here". Kept a plain `usize` rather than derived
/// from that `f32` so no const float cast is involved.
const SNAP_FLOOR: usize = 3;

/// Whether new output should return this viewport to the live bottom, from the offset
/// **sampled before the drain** (terminal-scroll D12).
///
/// A bounded exception to `scrollback` D2's pinning. tmux's pin is tolerable because
/// copy-mode is modal, marked, and exits with `q`; ours is non-modal with no indicator,
/// so a stray trackpad brush or a few px of inertial residue parks the view at
/// `view_offset == 1` — visually identical to live on a 40-row grid — and from there the
/// terminal is silently frozen: a long build looks hung, and the instinctive "press a
/// key" recovery injects a stray byte into the command line. Snapping from within one
/// notch makes exactly that case self-healing while leaving every deliberate scroll of
/// four lines or more pinned, so the tmux win survives.
fn snap_on_output(offset_before_drain: usize) -> bool {
    // `!= 0` is not part of the floor — it says a viewport already at the live bottom
    // has nothing to snap. It can never *drift* off the bottom either: `feed_scroll`
    // auto-increments only when `view_offset > 0` (`grid.rs:378-385`).
    offset_before_drain != 0 && near_bottom(offset_before_drain)
}

/// Whether `offset` is within one wheel notch of the live bottom — D12's floor proper.
///
/// Split out from [`snap_on_output`] so the floor itself stays one named idea, separate
/// from that rule's additional "and there is something to snap" clause.
fn near_bottom(offset: usize) -> bool {
    offset <= SNAP_FLOOR
}

/// What a left-drag swallowed while it held the pointer (terminal-scroll D6).
///
/// A drag freezes the viewport, because `CursorMoved` resolves the drag focus with
/// `cursor_cell()` (viewport coords) while `Terminal::set_selection` stores live-grid
/// coords — so *any* offset change mid-drag decouples "the row under my finger" from
/// "the row I'm selecting". But suppressing without recording is a trap: `scrollback`
/// D2's auto-increment keeps running in the frozen window, walking `view_offset` past
/// `SNAP_FLOOR`, so a plain suppress would leave both snaps permanently disarmed. These
/// two fields are what the release replays them against.
///
/// Each records **which tab** deferred a snap, and nothing else. In particular there is
/// no press-time floor bit: D12's floor is evaluated once, at the source, against the
/// receiving tab's own pre-drain offset (see the `PtyOutput` arm), so a snap only ever
/// reaches this latch *having already qualified*. Re-applying the floor here — as the
/// first two cuts did — could only ever subtract, and subtract on the wrong tab's
/// position: a background tab whose snap had qualified was discarded whenever the
/// *dragged* tab happened to sit far from the bottom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DragLatch {
    /// The tab whose output snap was swallowed — D12's `advanced` conjunct, which is
    /// otherwise consumed and discarded per-event inside the window.
    ///
    /// Carries the **tab id**, not a bare flag: the deferred snap belongs to the tab
    /// that received the output, which need not be the active one (R9.4 — "no
    /// active-tab special case"). Replaying it on the active tab instead would yank a
    /// tab that needed nothing to live *and* leave the real one drifting past
    /// `SNAP_FLOOR` — the exact disarm this latch exists to prevent, one tab over.
    /// Last swallowed wins; two different tabs deferring inside one drag is possible
    /// but vanishingly rare, and either answer is defensible.
    advanced_while_dragging: Option<TabId>,
    /// The tab whose D4 write-boundary snap was swallowed (the user typed, pasted, or
    /// committed while holding the button). Tab-carrying for the same reason: an
    /// `Ime::Commit` routes to the tab the composition began in, which the user may
    /// have already switched away from.
    input_while_dragging: Option<TabId>,
}

/// **Which tab** releasing a drag must return to live, or `None` (terminal-scroll D6).
///
/// Replays both snap rules against what the drag swallowed, in order. D4 comes first
/// and **ignores the floor**: its rule is unconditional, so re-applying only D12's
/// (floor-gated) rule would be strictly narrower and could never stand in for it —
/// scroll back 100 lines, press-drag, type a command, release, and the view would
/// otherwise stay at 100 with the command already sent to zsh.
///
/// A deferred snap is replayed as-is because it already qualified where it was raised:
/// `advanced_while_dragging` is set only from the `PtyOutput` arm's `advanced &&
/// snap_on_output(pre_drain_offset)` gate, so "nothing was latched" is exactly how the
/// no-trigger cases stay no-ops — a quiet drag at `view_offset == 2` latches nothing and
/// so cannot jump to live (R6.4), and output during a drag that began 50 lines up fails
/// the source gate and is never latched (R6.5).
///
/// Returns the id rather than a bare `bool` so the deferred snap lands on the tab that
/// actually earned it (R9.4 — "whichever tab received the output, no active-tab special
/// case"), which is not necessarily the tab being dragged.
fn drag_release_snap(latch: DragLatch) -> Option<TabId> {
    // D4 first: its rule is unconditional, so it can never be stood in for by D12's.
    latch.input_while_dragging.or(latch.advanced_while_dragging)
}

/// Where an `Ime::Commit`'s bytes go (R8/D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitTarget {
    /// The tab that owns the composition, which is still open (R8.3).
    Tab(TabId),
    /// No composition was in flight — the active tab, as always (R8.5).
    Active,
    /// The owning tab is gone; the bytes have nowhere to land (R8.4).
    Drop,
}

/// The commit-routing decision, factored out of the event loop so it is testable
/// without a window or a PTY (R8.3/R8.4/R8.5).
///
/// `tab_exists` is `FnMut` only because the sole id probe on [`Tabs`] is
/// `by_id_mut`, whose `&mut` borrow makes the caller's closure `FnMut`.
fn commit_target(
    composing_tab: Option<TabId>,
    mut tab_exists: impl FnMut(TabId) -> bool,
) -> CommitTarget {
    match composing_tab {
        Some(id) if tab_exists(id) => CommitTarget::Tab(id),
        Some(_) => CommitTarget::Drop,
        None => CommitTarget::Active,
    }
}

/// The composition-state transition, factored out of [`Running::set_preedit`] so the
/// state machine is testable without a window or an IME (R6.6/R6.7/D11/D12).
///
/// Returns the new `(composing, composing_tab)` for an incoming preedit `text`.
///
/// **The two outputs deliberately do not clear together.**
///
/// - `composing` — the [`suppress_key`](crate::input::suppress_key) gate. Empty text
///   is winit's synthetic clear signal (view.rs:345), so "empty ⇒ `false`" is what
///   keeps the gate from sticking `true` and swallowing every later character key
///   (R6.7/D12 — the deadlock guard).
/// - `composing_tab` — the commit *owner*, which must **outlive** that clear. winit
///   queues `Ime::Preedit("")` immediately before every `Ime::Commit`
///   (view.rs:412-413); clearing the owner on the empty preedit (the first cut) left
///   it `None` at every commit, making the tab-identity routing inert. Ownership is
///   released by the `Commit` arm once the bytes are placed, by `Ime::Disabled`, and
///   by the start of a *new* composition — never by the empty preedit.
///
/// `was_composing` distinguishes "continuing this composition" (keep the first owner,
/// so a mid-composition tab switch cannot re-point it) from "starting a new one"
/// (adopt the currently-active tab).
fn next_composition_state(
    text: &str,
    was_composing: bool,
    prev_owner: Option<TabId>,
    active: TabId,
) -> (bool, Option<TabId>) {
    if text.is_empty() {
        // Gate off, ownership preserved for the `Commit` that winit queues next.
        (false, prev_owner)
    } else if was_composing {
        // Same composition continuing — the first owner wins.
        (true, prev_owner.or(Some(active)))
    } else {
        // A new composition begins here; it belongs to the tab that is active now.
        (true, Some(active))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R6.6: a non-empty preedit composes; winit's synthetic empty one clears the
    /// gate — but NOT the ownership (see `ownership_survives_the_pre_commit_preedit`).
    #[test]
    fn preedit_toggles_composing() {
        let a = TabId(1);
        let (composing, owner) = next_composition_state("한", false, None, a);
        assert!(composing, "non-empty preedit composes");
        assert_eq!(owner, Some(a), "first preedit records its tab");

        let (composing, owner) = next_composition_state("", composing, owner, a);
        assert!(!composing, "empty preedit clears the suppress_key gate");
        assert_eq!(owner, Some(a), "but ownership is still held for the commit");
    }

    /// **The regression this whole amendment exists for (D11/R8.3).**
    ///
    /// winit queues `Ime::Preedit("")` immediately before every `Ime::Commit`
    /// (view.rs:412-413). The first implementation cleared `composing_tab` on that
    /// empty preedit, so by the time the `Commit` arm read it, it was always `None`
    /// — every commit fell through to the active tab and `CommitTarget::Tab`/`Drop`
    /// were unreachable. This pins the ordering that made the routing inert.
    #[test]
    fn ownership_survives_the_pre_commit_preedit() {
        let a = TabId(4);
        // Compose in tab a.
        let (composing, owner) = next_composition_state("한", false, None, a);
        assert_eq!(owner, Some(a));
        // winit's synthetic clear, queued right before the Commit.
        let (composing, owner) = next_composition_state("", composing, owner, a);
        assert!(!composing);
        // The Commit arm reads this: it MUST still name the originating tab, or the
        // tab-identity routing degenerates to "whoever is active now".
        assert_eq!(
            owner,
            Some(a),
            "commit routing would be inert if this were None"
        );
        assert_eq!(
            commit_target(owner, |id| id == a),
            CommitTarget::Tab(a),
            "routing must reach the Tab arm in the real event order"
        );
    }

    /// R6.7/D12: an input-source switch (한↔영) delivers `Ime::Disabled` with no
    /// preceding empty `Preedit`. That path feeds `""` through this transition, so
    /// `composing` must not stick `true` — if it did, `suppress_key` would swallow
    /// every character key from then on (a permanent deadlock).
    #[test]
    fn disabled_clears_composing_without_a_preceding_empty_preedit() {
        let a = TabId(3);
        // Mid-composition, then straight to the Disabled path.
        let (composing, owner) = next_composition_state("하", false, None, a);
        assert!(composing);
        let (composing, _owner) = next_composition_state("", composing, owner, a);
        assert!(!composing, "no deadlock: the gate is released");
        // The `Ime::Disabled` arm additionally nulls ownership; a fresh composition
        // afterwards adopts the active tab either way.
        let (composing, owner) = next_composition_state("가", false, None, a);
        assert!(composing);
        assert_eq!(owner, Some(a));
    }

    /// D11: ownership sticks to the tab the composition *began* in — a later
    /// preedit of the SAME composition must not re-point it at whatever tab is
    /// active now.
    #[test]
    fn ownership_keeps_the_first_tab() {
        let (a, b) = (TabId(1), TabId(2));
        let (composing, owner) = next_composition_state("ㅎ", false, None, a);
        assert_eq!(owner, Some(a));
        // Same composition continues while tab b is somehow active.
        let (composing, owner) = next_composition_state("하", composing, owner, b);
        assert!(composing);
        assert_eq!(owner, Some(a), "still owned by the tab it started in");
    }

    /// A *new* composition adopts the tab that is active when it starts — otherwise
    /// a stale owner from a cancelled composition would capture it.
    #[test]
    fn a_new_composition_adopts_the_active_tab() {
        let (a, b) = (TabId(1), TabId(2));
        // Stale ownership left over from a cancelled composition in tab a.
        let stale = Some(a);
        // A brand-new composition starts while tab b is active (was_composing false).
        let (composing, owner) = next_composition_state("ㄱ", false, stale, b);
        assert!(composing);
        assert_eq!(owner, Some(b), "new composition belongs to the active tab");
    }

    /// The full sequence R6.6 → R6.7 states, table-driven: each step's text and the
    /// expected `(composing, owner)` after it. Models the real winit event order,
    /// including the empty `Preedit` that precedes a `Commit`.
    #[test]
    fn composition_sequence_never_deadlocks() {
        let a = TabId(5);
        // (text, want_composing, want_owner) — owner threads through.
        let steps: &[(&str, bool, Option<TabId>)] = &[
            ("ㅎ", true, Some(a)), // compose
            ("하", true, Some(a)), // continue
            ("한", true, Some(a)), // continue
            ("", false, Some(a)),  // winit's pre-commit clear: gate off, owner kept
            ("ㄱ", true, Some(a)), // compose again (owner still a — same tab)
            ("", false, Some(a)),  // clear again
            ("", false, Some(a)),  // idempotent
            ("a", true, Some(a)),  // still able to compose afterwards
        ];
        let mut owner = None;
        let mut composing = false;
        for (text, want_composing, want_owner) in steps {
            let (next_composing, next) = next_composition_state(text, composing, owner, a);
            assert_eq!(next_composing, *want_composing, "composing after {text:?}");
            assert_eq!(next, *want_owner, "owner after {text:?}");
            owner = next;
            composing = next_composing;
        }
    }

    /// R8.3: a commit that began in tab A goes to tab A, even after switching to B.
    #[test]
    fn commit_routes_to_the_composing_tab() {
        let a = TabId(7);
        assert_eq!(
            commit_target(Some(a), |id| id == a || id == TabId(9)),
            CommitTarget::Tab(a)
        );
    }

    /// R8.4: the composing tab was closed → the bytes are dropped, not misdelivered.
    #[test]
    fn commit_is_dropped_when_the_composing_tab_is_gone() {
        assert_eq!(
            commit_target(Some(TabId(7)), |id| id == TabId(9)),
            CommitTarget::Drop
        );
    }

    /// R8.5: no composition in flight → the active tab. This is the branch that
    /// guarantees plain (non-composed) characters are never lost.
    #[test]
    fn plain_commit_goes_to_the_active_tab() {
        // The probe must not even be consulted when there is no owner.
        assert_eq!(
            commit_target(None, |_| unreachable!("no owner: existence is irrelevant")),
            CommitTarget::Active
        );
    }

    // --- T5: the engine guarantees the wheel path leans on -----------------------
    //
    // These live in the app crate, not `crates/term-core/tests/`, because C1/R11.4
    // require that crate's diff to stay empty. They read as consumer-contract tests:
    // each is something `wheel_grid` / the snap paths silently depend on, and a
    // term-core change that broke one would break scrolling with no app test noticing.

    fn scroll_term(cols: u16, rows: u16) -> term_core::Terminal {
        term_core::Terminal::new(
            cols,
            rows,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        )
    }

    /// Feed `n` numbered lines ("0\r\n1\r\n…").
    fn feed_lines(t: &mut term_core::Terminal, n: usize) {
        for i in 0..n {
            if i > 0 {
                t.advance(b"\r\n");
            }
            t.advance(i.to_string().as_bytes());
        }
    }

    fn top_row(t: &term_core::Terminal) -> String {
        let snap = t.snapshot();
        (0..snap.cols)
            .map(|c| snap.cell(c, 0).map(|c| c.ch).unwrap_or(' '))
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// R5.1 — **the concrete win over Warp**, where the wheel under tmux did nothing at
    /// all. While `?1049h` is active the wheel still reaches the primary-screen history
    /// captured before the full-screen program took over.
    ///
    /// Scrolled a full screen here (`view_offset == rows`), which is the only offset
    /// that yields an unspliced frame: below it the snapshot stacks frozen history on
    /// the still-repainting alt screen, the artifact recorded in Known Gaps.
    #[test]
    fn the_wheel_reaches_frozen_primary_history_under_alt_screen() {
        let mut t = scroll_term(4, 3);
        feed_lines(&mut t, 6); // history: "0","1","2" — live: "3","4","5"
        assert_eq!(t.history_len(), 3);

        t.advance(b"\x1b[?1049h");
        assert!(t.alt_screen());
        feed_lines(&mut t, 10); // vim/tmux churn — retained by nothing

        t.scroll_up(3);
        assert_eq!(t.view_offset(), 3);
        assert_eq!(
            top_row(&t),
            "0",
            "the pre-tmux shell output is still reachable"
        );
    }

    /// R5.2 — scrolling is display-only (`scrollback` D4). `Terminal` exposes no write
    /// path from the scroll API at all, so the observable half is that the shell's
    /// cursor never moves: were it otherwise, the shell and the screen would disagree
    /// about where the next character lands.
    #[test]
    fn scrolling_never_moves_the_shell_cursor() {
        let mut t = scroll_term(4, 3);
        feed_lines(&mut t, 6);
        let before = t.snapshot().cursor;
        t.scroll_up(2);
        assert_eq!(t.snapshot().cursor, before);
        t.scroll_down(1);
        assert_eq!(t.snapshot().cursor, before);
        t.scroll_to_bottom();
        assert_eq!(t.snapshot().cursor, before);
    }

    /// R10.1 — each tab owns its own `Terminal`, so a reading position is per-tab for
    /// free. This is what makes the tab-switch accumulator reset (D3) the *only* code
    /// a tab switch needs.
    ///
    /// Scope note: this asserts the *engine* property the per-tab guarantee rests on —
    /// two `Terminal`s hold independent offsets. Driving a real A→B→A switch needs
    /// `Tabs`, which spawns a PTY, so it is not reachable from a unit test.
    #[test]
    fn terminals_own_their_own_view_offset() {
        let (mut a, mut b) = (scroll_term(4, 3), scroll_term(4, 3));
        feed_lines(&mut a, 6);
        feed_lines(&mut b, 6);
        a.scroll_up(2);
        assert_eq!(a.view_offset(), 2);
        assert_eq!(b.view_offset(), 0, "the other tab stays where it was");
    }

    /// R10.2 — a resize while scrolled back keeps the position and cannot panic or read
    /// out of bounds: `Grid::resize` leaves `view_offset` alone and the snapshot reads
    /// history cells through `.get().unwrap_or(blank)`. History is not *reflowed* to the
    /// new width — a documented gap owned by `specs/scrollback`, not this feature.
    #[test]
    fn resize_while_scrolled_keeps_the_position() {
        let mut t = scroll_term(8, 4);
        feed_lines(&mut t, 20);
        t.scroll_up(5);
        let before = t.view_offset();
        assert_eq!(before, 5);

        t.resize(4, 4); // narrower than the history was captured at
        let snap = t.snapshot();
        assert_eq!(snap.cols, 4);
        assert_eq!(t.view_offset(), before, "the reading position survives");

        t.resize(20, 6); // and wider
        let snap = t.snapshot();
        assert_eq!(snap.cols, 20);
        assert_eq!(t.view_offset(), before);
    }

    /// terminal-scroll R9.1/R9.2/R9.5 — the whole floor rule, as a table.
    #[test]
    fn output_snaps_only_from_within_one_notch_of_the_bottom() {
        // R9.5: already live — nothing to snap.
        assert!(!snap_on_output(0));
        // R9.1: the stray-brush band, where the screen looks identical to live.
        assert!(snap_on_output(1));
        assert!(snap_on_output(2));
        assert!(snap_on_output(SNAP_FLOOR));
        // R9.2: a deliberate scroll stays pinned — `scrollback` D2 keeps the content.
        assert!(!snap_on_output(SNAP_FLOOR + 1));
        assert!(!snap_on_output(40));
        assert!(!snap_on_output(100_000));
    }

    /// terminal-scroll R11.2 — the release verdict over **every** latch state.
    ///
    /// Two distinct ids so the table pins *which* tab comes back, not merely that one
    /// does (R9.4). There is no floor bit to enumerate: D12's floor is applied once, at
    /// the source, so a latched `advanced` has already qualified and the "shouldn't
    /// snap" cases are represented by `None` rather than by a false conjunct — see
    /// [`the_source_gate_is_what_keeps_unqualified_output_out_of_the_latch`].
    #[test]
    fn drag_release_verdict_truth_table() {
        let (out_tab, in_tab) = (TabId(11), TabId(22));
        // (output latched, input latched, expected)
        let cases = [
            (false, false, None),         // R6.4/R6.5 — nothing was deferred
            (false, true, Some(in_tab)),  // R6.2 — typed mid-drag, no floor applies
            (true, false, Some(out_tab)), // R6.3/R6.6 — the deferred output snap's tab
            (true, true, Some(in_tab)),   // both: D4 is checked first
        ];
        for (advanced, input, want) in cases {
            let latch = DragLatch {
                advanced_while_dragging: advanced.then_some(out_tab),
                input_while_dragging: input.then_some(in_tab),
            };
            assert_eq!(
                drag_release_snap(latch),
                want,
                "advanced={advanced} input={input}"
            );
        }
    }

    /// **R6.4/R6.5 — where the no-snap cases are actually enforced.** They are not a
    /// false conjunct at release; they are the source gate refusing to latch at all.
    /// Pinning it here keeps the truth table above from reading two ways: a `None` row
    /// means "nothing qualified", never "something qualified and was then discarded".
    ///
    /// The mirror image is the bug this replaced: re-testing the floor at release used
    /// the *dragged* tab's position, so a background tab's already-qualified snap was
    /// thrown away whenever the dragged tab sat far from the bottom (R6.6).
    #[test]
    fn the_source_gate_is_what_keeps_unqualified_output_out_of_the_latch() {
        // R6.5 — a viewport 50 lines up never qualifies, so nothing is ever latched.
        assert!(
            !snap_on_output(50),
            "far from the bottom: no snap is raised"
        );
        // R6.4 — a quiet drag raises nothing at all; the latch stays empty.
        assert_eq!(drag_release_snap(DragLatch::default()), None);
        // R6.3/R6.6 — a viewport within one notch qualifies at the source, and whatever
        // qualifies is replayed on release for *its own* tab.
        assert!(snap_on_output(1));
        let background = TabId(7);
        assert_eq!(
            drag_release_snap(DragLatch {
                advanced_while_dragging: Some(background),
                input_while_dragging: None,
            }),
            Some(background),
            "a qualified snap is replayed regardless of where the dragged tab sits"
        );
    }

    /// **R9.4 — the tab-identity regression.** The latch defers a snap; replaying it on
    /// whoever happens to be *active* was the first cut, and it fails twice over: the
    /// active tab is yanked to live having earned nothing, while the tab that actually
    /// received the output stays scrolled and drifts past `SNAP_FLOOR` — the very disarm
    /// D6 exists to prevent, reproduced one tab over. Reachable by scrolling tab B back,
    /// switching to A, and drag-selecting in A while B keeps printing.
    #[test]
    fn the_release_snaps_the_tab_that_earned_it_not_the_active_one() {
        let background = TabId(7);
        let latch = DragLatch {
            advanced_while_dragging: Some(background), // output landed in a background tab
            input_while_dragging: None,
        };
        assert_eq!(
            drag_release_snap(latch),
            Some(background),
            "the deferred snap belongs to the tab that received the output"
        );

        // Same for D4: an IME commit routes to the tab its composition began in, which
        // the user may already have switched away from.
        let composing_tab = TabId(3);
        let latch = DragLatch {
            advanced_while_dragging: None,
            input_while_dragging: Some(composing_tab),
        };
        assert_eq!(drag_release_snap(latch), Some(composing_tab));
    }

    /// **R6.2 — the one-directional gap that makes replaying D12's rule alone wrong.**
    ///
    /// D4's snap has no floor; D12's is gated at `SNAP_FLOOR`. So a release that only
    /// re-applied the floor-gated rule could never stand in for the unconditional one:
    /// scroll back 100 lines, press-drag, type a command, release — and the view would
    /// stay at 100 with the command already sent to zsh, which is exactly the "typed
    /// into a frozen screen, terminal looks dead" failure D4 exists to prevent.
    #[test]
    fn typing_mid_drag_snaps_on_release_even_from_far_up() {
        let tab = TabId(5);
        let far_from_bottom = DragLatch {
            // Pressed at view_offset == 100 — irrelevant to D4, which has no floor.
            advanced_while_dragging: None,
            input_while_dragging: Some(tab), // typed a command while holding
        };
        assert_eq!(
            drag_release_snap(far_from_bottom),
            Some(tab),
            "D4 is unconditional — the floor must not gate it"
        );
    }

    /// **R6.3 — the disarm this latch exists to prevent.** A stray brush parks the view
    /// at 1 (indistinguishable from live); the user drags to select and nothing
    /// highlights, because selections don't render off-bottom; output streams and D2's
    /// auto-increment walks the offset past the floor. Re-testing the floor at release
    /// would judge the *drifted* value, decline, and leave the terminal silently frozen
    /// for good — so the release replays the qualified snap instead of re-deciding it.
    ///
    /// Scope note: this pins the **verdict function** over a hand-built latch. That the
    /// floor was applied at the source, pre-drain, is a property of the `PtyOutput` call
    /// site, which needs a live PTY and is not reachable here.
    #[test]
    fn a_snap_deferred_by_a_drag_survives_the_drift() {
        let tab = TabId(2);
        let qualified_at_offset_1 = DragLatch {
            advanced_while_dragging: Some(tab), // qualified at the source, then deferred
            input_while_dragging: None,
        };
        assert_eq!(drag_release_snap(qualified_at_offset_1), Some(tab));
        // What re-deciding on the drifted offset would have concluded instead.
        assert!(
            !snap_on_output(15),
            "the drifted value fails the floor — which is why it is not re-tested"
        );
    }

    /// **R9.3 — the predicate half of the pre-drain rule.** `scrollback` D2 auto-increments
    /// `view_offset` in `feed_scroll` to pin the content being read, so a viewport parked
    /// at 2 that receives a 10-line burst reads 12 once the drain returns: the same
    /// situation yields opposite verdicts depending on which value is fed in.
    ///
    /// Scope note: this pins the *predicate*, not the sampling **location**. That the
    /// call site reads `view_offset()` before `s.drain()` is enforced by the ordering
    /// inside the `by_id_mut` closure in the `PtyOutput` arm, which needs a live PTY and
    /// cannot be reached from a unit test — so this test would still pass if that line
    /// moved. C8 is the standing guard there.
    #[test]
    fn snap_on_output_rejects_a_drifted_offset() {
        let parked = 2;
        let burst = 10;
        assert!(
            snap_on_output(parked),
            "pre-drain: within the floor, so the view self-heals"
        );
        assert!(
            !snap_on_output(parked + burst),
            "post-drain: the very same situation now reads as a deliberate scroll"
        );
    }

    /// The floor means "one wheel notch" (D12). Nothing enforces that at the type level,
    /// so retuning `LINES_PER_NOTCH` would silently desynchronise it — this is the check
    /// that fails loudly instead.
    #[test]
    fn the_snap_floor_stays_one_wheel_notch() {
        assert_eq!(
            SNAP_FLOOR as f32,
            crate::input::LINES_PER_NOTCH,
            "SNAP_FLOOR must track LINES_PER_NOTCH — D12 defines it as one notch"
        );
    }
}
