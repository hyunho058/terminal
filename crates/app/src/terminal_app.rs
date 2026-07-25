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

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

use common::config::{Config, SidebarConfig};
use common::watch::ConfigWatcher;
use objc2_app_kit::NSView;
use renderer::cursor_layer::CursorLayer;
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
use winit::window::{Window, WindowId};

use crate::input::{
    encode_key, pixel_to_cell, sidebar_hover, sidebar_press, suppress_key, tab_action,
    wheel_scroll, SidebarAction, TabAction, WheelDelta,
};
use crate::seam::{AppCtx, Handled, InputHandler, InputRouter, KeyInput, Overlay, OverlayHost};
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
    /// Last resolved card-hover index — the region debounce for `set_hovered` (D8).
    last_hit: Option<usize>,
    router: InputRouter,
    overlays: OverlayHost,
    mods: ModifiersState,
    /// Latest cursor position in physical px (raw winit `f64`), updated on
    /// `CursorMoved` and read on a press to hit-test the sidebar and re-resolve hover.
    last_cursor: (f64, f64),
    /// Left-drag selection state (selection-input, the sanctioned event-loop
    /// exception). While `dragging`, `CursorMoved` updates the active grid's
    /// selection focus. `drag_anchor` is the press cell; `drag_moved` records
    /// whether the focus ever left the anchor cell — a plain click (never moved)
    /// clears the selection on release, jitter-safely (cell-granular, not raw px).
    dragging: bool,
    drag_anchor: (u16, u16),
    drag_moved: bool,
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
        let config_watcher = match Config::path() {
            Ok(path) => {
                let proxy = self.proxy.clone();
                ConfigWatcher::spawn(path, move |cfg| {
                    if config_tx.send(cfg).is_ok() {
                        let _ = proxy.send_event(UserEvent::ConfigReloaded);
                    }
                })
                .map_err(|e| tracing::warn!("config watcher disabled: {e}"))
                .ok()
            }
            Err(e) => {
                tracing::warn!("config path unavailable, live settings off: {e}");
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
            last_hit: None,
            router,
            overlays,
            mods: ModifiersState::empty(),
            last_cursor: (0.0, 0.0),
            dragging: false,
            drag_anchor: (0, 0),
            drag_moved: false,
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
                if !text.is_empty() {
                    let composing_tab = self.composing_tab;
                    let target =
                        commit_target(composing_tab, |id| self.tabs.by_id_mut(id).is_some());
                    match target {
                        // The composing tab is still open — write there (R8.3).
                        CommitTarget::Tab(id) => match self.tabs.by_id_mut(id) {
                            Some(session) => {
                                if let Err(e) = session.write(text.as_bytes()) {
                                    tracing::warn!("pty write failed: {e}");
                                }
                            }
                            // Unreachable: `commit_target` only yields `Tab` when
                            // the probe said it exists. Never panic on it.
                            None => tracing::warn!(tab = id.0, "commit target vanished"),
                        },
                        // Plain, non-composed text — the active tab, as always.
                        // This branch is what guarantees no character is ever
                        // lost (R8.5).
                        CommitTarget::Active => {
                            if let Err(e) = self.tabs.active_mut().write(text.as_bytes()) {
                                tracing::warn!("pty write failed: {e}");
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
                self.set_preedit(text);
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
        self.preedit_handle.set_text(self.preedit.clone());
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

    /// Push the current tab rows + active index into the sidebar layer. `Tabs::titles()`
    /// also pushes the metadata roster to the worker (tab-metadata seam).
    fn sync_tab_model(&mut self) {
        let titles = self.tabs.titles();
        let waiting = self.build_waiting_rows(&titles);
        self.tab_model.set(TabBarModel {
            titles,
            active: self.tabs.active_index(),
            waiting,
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
        let active = self.tabs.active().id;
        // A tab with no live agent maps to `Unknown` (not `Waiting`), so an agent
        // that finished (`agent: None`) is seen as leaving Waiting — a falling edge.
        let states: Vec<(TabId, AgentState)> = self
            .tabmeta_snapshot
            .iter()
            .map(|(id, meta)| {
                (
                    id,
                    meta.agent.map(|a| a.state).unwrap_or(AgentState::Unknown),
                )
            })
            .collect();
        self.waiting
            .on_agent_states(&states, active, Instant::now())
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
    }

    /// Re-resolve hover at `last_cursor` after the layout moved under a stationary pointer
    /// (D8): push the raw cursor to the chrome layer (undebounced — it must be exact for a
    /// button edge) and the resolved card index to the row layer (region-debounced).
    fn refresh_hover(&mut self) {
        let inp = self.sidebar_input();
        let collapsed = self.collapsed();
        self.chrome_handle
            .set_cursor(Some([self.last_cursor.0 as f32, self.last_cursor.1 as f32]));
        let hit = sidebar_hover(self.last_cursor, &inp, self.scroll, collapsed);
        if hit != self.last_hit {
            self.last_hit = hit;
            self.tab_model.set_hovered(hit);
        }
        self.window.request_redraw();
    }

    /// Clear all hover state — the pointer left the window or focus was lost with it still
    /// inside (Cmd-Tab fires no `CursorLeft`), else the highlight stays lit (D8).
    fn clear_hover(&mut self) {
        self.chrome_handle.set_cursor(None);
        if self.last_hit.is_some() {
            self.last_hit = None;
            self.tab_model.set_hovered(None);
        }
        self.window.request_redraw();
    }

    /// Handle a tab chord (R3.2). Returns false if the app should exit (last tab
    /// closed).
    fn do_tab_action(&mut self, action: TabAction) -> bool {
        // A tab switch abandons any in-flight composition (R8.1/R8.2): Cmd+T is a
        // Named key + modifier, so it sails straight through the composing gate
        // (R6.3) and is reachable by keyboard, not just by a sidebar click.
        self.cancel_composition();
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
        // Becoming the active tab is the "confirmation" that clears its waiting signals
        // (D12/R2.1). Every path that changes the active tab — click, chord, Cmd+number
        // jump, or a click on a waiting row — funnels through here. Acknowledge BEFORE
        // rebuilding the model so the just-activated session's item is already gone from
        // the band (clicking a waiting row makes it disappear, R2.1). `do_tab_action` ends
        // in `redraw()`, so the cleared band repaints.
        self.waiting.acknowledge(self.tabs.active().id);
        self.sync_tab_model();
        // Reveal the (possibly newly-appended or below-the-fold) active tab and re-clamp
        // the offset — a tab close shrinks the content (D6/D10). Then re-resolve hover.
        let inp = self.sidebar_input();
        self.scroll = sidebar_layout::scroll_to_reveal(self.tabs.active_index(), self.scroll, &inp);
        self.publish_scroll();
        self.refresh_hover();
        // Re-point cwd-derived overlays (ai-monitor) at the new active tab (R3.1).
        let cwd = self.tabs.active_cwd();
        self.overlays.notify_active_changed(cwd);
        self.redraw();
        true
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

    /// Dispatch a key through the seam, then default input (R3.3/R4). Returns
    /// false if the app should exit.
    fn on_key(&mut self, input: KeyInput) -> bool {
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
        // 1) Visible overlays first (R4.1/R4.2).
        {
            let active = self.tabs.active_index();
            let count = self.tabs.len();
            let cwd = self.tabs.active_cwd();
            let mut ctx = AppCtx::new(&mut self.tabs.active_mut().writer, count, active, cwd)
                .with_bracketed_paste(bracketed)
                .with_selection_text(selection_text.clone());
            if self.overlays.dispatch(&input, &mut ctx) == Handled::Consumed {
                if ctx.redraw_requested() {
                    self.redraw();
                }
                return true;
            }
        }
        // 2) Registered input handlers before default (R3.3).
        {
            let active = self.tabs.active_index();
            let count = self.tabs.len();
            let cwd = self.tabs.active_cwd();
            let mut ctx = AppCtx::new(&mut self.tabs.active_mut().writer, count, active, cwd)
                .with_bracketed_paste(bracketed)
                .with_selection_text(selection_text);
            if self.router.dispatch(&input, &mut ctx) == Handled::Consumed {
                if ctx.redraw_requested() {
                    self.redraw();
                }
                return true;
            }
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
            if let Err(e) = self.tabs.active_mut().write(&bytes) {
                tracing::warn!("pty write failed: {e}");
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
                let advanced = inner
                    .tabs
                    .by_id_mut(id)
                    .map(|s| {
                        let got = s.drain();
                        command_end = s.take_command_end();
                        title_changed = s.take_title_dirty();
                        got
                    })
                    .unwrap_or(false);
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
            }

            WindowEvent::CursorMoved { position, .. } => {
                inner.last_cursor = (position.x, position.y);
                inner.refresh_hover(); // one hit_test feeds both hover fields (D8)
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

            WindowEvent::CursorLeft { .. } => inner.clear_hover(),

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
                if let Some(action) =
                    sidebar_press(inner.last_cursor, &inp, inner.scroll, collapsed)
                {
                    match action {
                        SidebarAction::Switch(i) => {
                            if !inner.do_tab_action(TabAction::Jump(i)) {
                                event_loop.exit();
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
                }
            }

            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if inner.dragging => {
                // End the drag (selection-input). A plain click — the focus never
                // left the anchor cell — clears any selection; a real drag leaves the
                // selection standing for Cmd+C.
                inner.dragging = false;
                if !inner.drag_moved {
                    inner.tabs.active_mut().terminal.clear_selection();
                    inner.redraw();
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let wd = match delta {
                    MouseScrollDelta::LineDelta(_, y) => WheelDelta::Lines(y),
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels(p.y as f32),
                };
                let inp = inner.sidebar_input();
                let collapsed = inner.collapsed();
                let new_scroll = wheel_scroll(wd, inner.last_cursor, inner.scroll, &inp, collapsed);
                if new_scroll != inner.scroll {
                    inner.scroll = new_scroll;
                    inner.publish_scroll();
                    inner.refresh_hover(); // the list moved under the pointer (D8)
                    inner.window.request_redraw(); // sidebar-only change (D7)
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
                // Losing the window abandons the composition too (R8.1/R8.2): the
                // preedit is drawn by *our* layer, so leaving it on screen would
                // show text the shell never received.
                inner.cancel_composition();
                // …and clears the sidebar hover, since a Cmd-Tab away fires no
                // `CursorLeft` while the pointer is still inside (D8).
                inner.clear_hover();
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
                if inner.snapshot_dirty || inner.last_snapshot.is_none() {
                    inner.last_snapshot = Some(inner.tabs.active().terminal.snapshot());
                    inner.snapshot_dirty = false;
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
}
