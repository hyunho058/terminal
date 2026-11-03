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

use common::config::{
    Config, SidebarConfig, FONT_SIZE_DEFAULT, FONT_SIZE_MAX, FONT_SIZE_MIN, FONT_SIZE_STEP,
};
use common::watch::ConfigWatcher;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType, NSView};
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
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
// winit 0.30 re-exports raw-window-handle 0.6 (`rwh_06`, a default feature), so
// reaching the `NSView` needs no new dependency on `app` (D11).
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Window, WindowId};

use crate::drag::{self, SelectionDrag};
use crate::hyperlink::{
    frame_rescans, url_to_open, ClickInput, HoverInput, HoverState, LinkTrigger, Opener, Update,
};
use crate::ime_chord::{self, CmdEventFacts, Replay};
use crate::input::{
    self, drag_owns_pointer, encode_key, font_action, grid_press_action, grid_release_action,
    hover_card, hover_cursor, is_double_click, is_settings_chord, is_sidebar_toggle_chord,
    pixel_to_cell, pixel_to_cell_clamped, press_dismisses_modals, reported_drag_motion, should_snap_on_click,
    sidebar_press, sidebar_region, suppress_key, tab_action, wheel_bytes, wheel_lines,
    wheel_scroll, FontAction, GridPressAction, GridReleaseAction, PersistTarget, PointerOwner,
    PressRoute, ReleaseTarget, ResizeDrag, ResizeEdge, SidebarAction, TabAction, WheelDelta,
};
use crate::seam::{
    AppCtx, AppRequest, Handled, InputRouter, KeyInput, OverlayCommand, OverlayHost, Registrations,
    WriteProbe,
};
use crate::session::{Session, TabId, WakeSource};
use crate::tabmeta::{AgentState, TabMetaSnapshot};
use crate::tabs::Tabs;
use crate::waiting::{WaitingKind, WaitingState};

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

/// AppKit's current event as plain data (korean-input-chords D7) — read-only, never
/// consumed or re-posted. `None` off the main thread or when there is no current event.
///
/// Unfiltered: every event becomes facts, and `ime_chord` decides what counts. Only the
/// key fields are guarded, because `characters` and `keyCode` raise an Objective-C
/// exception on a non-key event (a mouse click can be current when a commit arrives).
fn current_event_facts() -> Option<CmdEventFacts> {
    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    let kind = event.r#type();
    let flags = event.modifierFlags();
    let is_key = kind == NSEventType::KeyDown || kind == NSEventType::KeyUp;
    let string = |s: Option<objc2::rc::Retained<objc2_foundation::NSString>>| {
        s.map(|s| s.to_string()).unwrap_or_default()
    };
    Some(CmdEventFacts {
        is_key_down: kind == NSEventType::KeyDown,
        command: flags.contains(NSEventModifierFlags::Command),
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        option: flags.contains(NSEventModifierFlags::Option),
        key_code: if is_key { event.keyCode() } else { 0 },
        characters: if is_key {
            string(event.characters())
        } else {
            String::new()
        },
        characters_ignoring_modifiers: if is_key {
            string(event.charactersIgnoringModifiers())
        } else {
            String::new()
        },
        timestamp: event.timestamp(),
    })
}

/// One waiting-band row's folder name: the tab's **live** published cwd, falling back to the
/// title the rows were assembled with (card-live-cwd D5/R3.1/R3.3).
///
/// The same degrade rule the card uses (D2), so the two zones can never name one tab's
/// directory differently. Pure, so the rule is unit-tested without a window.
fn band_row_title(meta: Option<&crate::tabmeta::TabMeta>, row_title: &str) -> String {
    crate::sidebar_model::folder_name(meta.and_then(|m| m.cwd.as_deref()))
        .unwrap_or_else(|| row_title.to_string())
}

/// Whether the waiting band must be rebuilt on this wake (card-live-cwd D5/R3.2).
///
/// **A disjunction, and both arms are load-bearing.** `waiting_changed` is the pre-existing
/// trigger, derived by `push_agent_states` from the snapshot *and each tab's screen*, so it
/// fires on transitions the metadata never sees. The generation arm is what this feature
/// adds: the band names each tab by its folder, and a `cd` republishes the cwd without moving
/// any waiting item. Drop either arm and a real repaint stops happening silently.
///
/// Level-triggered against a stored generation rather than edge-triggered on
/// [`TabMetaHandle::sync`]'s `bool` — `displayed_label` syncs the same snapshot on a
/// keystroke and would consume the edge first (`tmux-pane-cwd` D6).
fn band_needs_rebuild(waiting_changed: bool, band_gen: u64, snapshot_gen: u64) -> bool {
    waiting_changed || band_gen != snapshot_gen
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
    /// SD4 seam: the handlers/overlays features registered *before* `run_app` —
    /// never by editing this file (C1). Consumed into the live registries on
    /// `resumed`.
    ///
    /// A [`Registrations`] rather than two bare `Vec`s so the whole
    /// registration→registry step is a value transformation `crates/app/tests/`
    /// can assert; `App` itself is unconstructible in a test (it needs an
    /// `EventLoopProxy`), so anything that stays in here is unreachable by any
    /// test at all.
    pending: Registrations,
}

impl App {
    /// `registrations` is the set `main` assembled — see [`crate::feature_registrations`].
    ///
    /// The **only** way extensions enter the app. There were once
    /// `register_handler` / `register_overlay` methods beside this for registering
    /// after construction, and they were the problem rather than a convenience:
    /// a second path meant part of the set was assembled where nothing could
    /// assert it, and the one feature that used it — the `tabmeta` pump — could be
    /// dropped from `main` with every test still green. Taking the whole set by
    /// value leaves one place to look and one value to assert.
    pub fn new(
        cfg: Config,
        proxy: EventLoopProxy<UserEvent>,
        registrations: Registrations,
    ) -> Self {
        App {
            cfg,
            proxy,
            inner: None,
            pending: registrations,
        }
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
    /// exception). `dragging` is the gesture's armed flag — what the release routing
    /// and the terminal-scroll D6 freeze read. While it is set, `CursorMoved` hands
    /// the pointer's cell to `drag`.
    dragging: bool,
    /// The drag itself (selection-scroll-anchor D3): its tab, its press cell, its
    /// content anchor taken at press time, and `drag_moved` — whether the pointer
    /// ever left the press cell, so a plain click clears the selection on release,
    /// jitter-safely (cell-granular, not raw px). `Some` from a grid press to its
    /// release. See [`SelectionDrag`].
    drag: Option<SelectionDrag>,
    /// What the in-flight drag swallowed, replayed on release (terminal-scroll D6).
    /// Meaningless while `dragging` is false; reset at each press.
    drag_latch: DragLatch,
    /// Active grid mouse button press forwarded to the child application PTY (mouse-button-reporting D4).
    mouse_reported: Option<(MouseButton, u16, u16)>,
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
    /// The same latch for `[ai_monitor]`'s `(width_px, open)` pair (right-panel D6/R6.4).
    ///
    /// A separate ring rather than a widened tuple: the two tables are written by different
    /// gestures at different times, and one pair covering both would classify a sidebar
    /// drag's echo as a *panel* human edit — clearing the panel's override on a gesture that
    /// never touched it.
    written_monitor: Vec<(f32, bool)>,
    /// The most recent `[font].size_pt` this session wrote, for echo suppression
    /// (terminal-font-size D9/R5.1). One value, not a ring like `written_sidebar`: font
    /// sizes are whole-point steps that recur constantly (14→15→14), so a ring would hold
    /// most plausible hand edits and swallow them as echoes. See
    /// [`should_clear_font_override`].
    last_written_font: Option<f32>,
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
    /// The same flag for the panel's width (right-panel D6 × tab-resize D14/D15a): set when
    /// a write failed, so an unrelated reload cannot snap the panel back to the file's width
    /// while the session is knowingly out of step with it.
    monitor_override_authoritative: bool,
    /// The same flag for `[font].size_pt` (terminal-font-size D9/R5.3): set when
    /// `save_font_size` failed so an unrelated reload cannot wipe the runtime font size.
    font_override_authoritative: bool,
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
    /// `NSEvent.timestamp` of the chord the last commit replayed, or `None`
    /// (korean-input-chords D8). The next `KeyboardInput` consumes it: if that event is
    /// the one already replayed, it is dropped, so a winit that one day forwards the key
    /// after a commit cannot run Cmd+T twice.
    replayed_ts: Option<f64>,
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
    /// The metadata generation the waiting band was last built from (card-live-cwd D5).
    ///
    /// The band names each tab by its folder, and that name comes from the snapshot — but the
    /// band is rebuilt only when the waiting *set* changes, and a `cd` moves no waiting item.
    /// Comparing this against `tabmeta_snapshot.generation()` is what makes a publish a
    /// rebuild. Level-triggered on purpose: `displayed_label` syncs the same snapshot on a
    /// keystroke, so an edge (`TabMetaHandle::sync`'s `bool`) can be consumed before the
    /// `OverlayPending` handler ever sees it (`tmux-pane-cwd` D6).
    band_gen: u64,
}

impl App {
    fn start(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> crate::session::anyhow_lite::Result<Running> {
        let attrs = Window::default_attributes()
            .with_title("Crewterm")
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 540.0));
        let window = Arc::new(event_loop.create_window(attrs)?);
        // Let macOS run the input method (R6.1). Without this winit's `key_down`
        // never calls `interpretKeyEvents` (view.rs:466-468), so a Korean IME never
        // gets to compose and only raw jamo reach the shell. Turning it on also
        // makes winit withhold the duplicate `KeyboardInput` for composing/committing
        // keys (view.rs:479-496); ASCII, which never marks text, is unaffected.
        window.set_ime_allowed(true);

        if let Some(mtm) = MainThreadMarker::new() {
            crate::settings_menu::install_app_menu(mtm);
        }

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
        // overlay also contributes a DrawLayer drawn above the grid (R4.1),
        // stacked in registration order.
        let (router, overlays, layers) = self.pending.install();
        for layer in layers {
            renderer.add_layer(layer);
        }

        // grid_size() now reports rows *below* the inset (R2.2).
        let (cols, rows) = renderer.grid_size();
        let fg = Color(self.cfg.theme.foreground);
        let bg = Color(self.cfg.theme.background);

        // The one place the winit proxy is turned into the event-loop-free `Wake`
        // the session layer takes. Everything below this line — sessions, tabs,
        // the whole input→shell→grid path — is constructible without an event
        // loop, which is what lets `crates/app/tests/` drive it.
        //
        // Here rather than in `main`, and that is not an accident to be tidied
        // away: `App::start` is the first moment a window exists, and therefore
        // the first moment there is a grid size to spawn a session against. `main`
        // holds the proxy only long enough to hand it over. Any doc that says
        // otherwise is describing a design that was considered and not built.
        let wake: WakeSource = {
            let proxy = self.proxy.clone();
            std::rc::Rc::new(move || {
                let proxy = proxy.clone();
                Box::new(move |id| {
                    let _ = proxy.send_event(UserEvent::PtyOutput(id));
                })
            })
        };
        let first = Session::spawn(TabId(0), cols, rows, fg, bg, (wake)())?;
        let tabs = Tabs::new(first, fg, bg, wake);

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
        // The first thing to check when Cmd+Shift+G does nothing (region-capture).
        crate::capture::log_screen_capture_access();
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
            drag: None,
            drag_latch: DragLatch::default(),
            mouse_reported: None,
            cursor_icon: CursorIcon::Default,
            resize_drag: None,
            config_path,
            written_sidebar: Vec::new(),
            written_monitor: Vec::new(),
            last_written_font: None,
            last_applied_cfg: self.cfg.clone(),
            sidebar_override_authoritative: false,
            monitor_override_authoritative: false,
            font_override_authoritative: false,
            config_rx,
            _config_watcher: config_watcher,
            occluded: false,
            last_snapshot: None,
            snapshot_dirty: true,
            composing: false,
            preedit: String::new(),
            composing_tab: None,
            replayed_ts: None,
            preedit_handle,
            waiting: WaitingState::new(),
            tabmeta_snapshot: TabMetaSnapshot::default(),
            band_gen: 0,
        };
        running.sync_tab_model();
        // The first point at which both halves of a restore are available: the config has
        // been loaded since `main`, and the active tab now exists, so its cwd resolves to
        // the directory the panel must open on (R6.6).
        running.restore_monitor_panel();
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
    ///
    /// Returns `false` when a chord replayed from a commit asked the app to exit (Cmd+W on
    /// the last tab while composing) — the same contract as `on_key`.
    fn on_ime(&mut self, ime: Ime) -> bool {
        match ime {
            // The IME finished a syllable: those bytes are the user's text (R6.2).
            // winit queues an empty `Preedit` immediately before every `Commit`
            // (view.rs:411-414), so `composing` is already false here for the plain
            // ASCII path; clear it anyway so the state can never outlive a commit.
            Ime::Commit(text) => {
                self.place_commit(text.clone());
                // After BOTH of `place_commit`'s exits — the rename field's early return
                // included — so a mid-composition Cmd+A in the field replays too
                // (korean-input-chords D7).
                return self.replay_lost_chord(&text);
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
        true
    }

    /// Place a committed syllable (R6.2, R8): the rename field first, else the tab the
    /// composition started in. Split out of `on_ime` so the chord replay runs after both of
    /// its exits.
    fn place_commit(&mut self, text: String) {
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
            let target = commit_target(composing_tab, |id| self.tabs.by_id_mut(id).is_some());
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

    /// Replay a Cmd chord the input method consumed while committing (korean-input-chords
    /// D7/D8). Mid-composition, the 2-Set Korean IM answers a Cmd keyDown by committing the
    /// syllable, and winit then emits no `KeyboardInput` for it at all; but this commit is
    /// dispatched synchronously inside that keyDown, so AppKit's current event is the lost
    /// chord (measured in the probe's `insertText`, not inside winit — G9 facts 3 and 4; the
    /// `chord-replay` line's `keyCode` is the in-app check). Runs after the syllable is
    /// placed, so Cmd+T opens the new tab only once `하` is in the old one, and Cmd+V pastes
    /// after it.
    fn replay_lost_chord(&mut self, commit_text: &str) -> bool {
        if commit_text.is_empty() {
            return true;
        }
        let facts = current_event_facts();
        let replay = ime_chord::replay_decision(facts.as_ref(), commit_text);
        self.replayed_ts = ime_chord::guard_after_commit(&replay);
        let key_code = facts.as_ref().map(|f| f.key_code);
        match replay {
            Replay::None => true,
            Replay::Skip(why) => {
                tracing::debug!(?key_code, why, "chord-replay skipped");
                true
            }
            Replay::Key(input, timestamp) => {
                tracing::debug!(
                    ?key_code,
                    timestamp,
                    characters = ?facts.as_ref().map(|f| f.characters.as_str()),
                    key = ?input.key,
                    "chord-replay"
                );
                self.on_key(input)
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

    /// Offer a left press to the visible overlays (the window-covering graph first, else the
    /// right band), with the same `AppCtx` the key path builds (session-flow-viewer D7), and
    /// hand back what they asked for. The caller applies the requests once this returns,
    /// because the context borrows the active tab's writer and `apply_requests` needs
    /// `&mut Tabs`.
    ///
    /// The writer is probed as at both key dispatch points (terminal-scroll D4): no press
    /// handler writes today, but one that did would be sending the user's bytes, and must
    /// snap a scrolled-back viewport like any other input.
    ///
    /// `covering` picks the route (session-flow-graph D6): `true` offers the press to the
    /// open window-covering graph only, `false` to the visible band overlays.
    fn press_overlays(&mut self, p: [f32; 2], covering: bool) -> (Handled, Vec<AppRequest>) {
        let active = self.tabs.active_index();
        let count = self.tabs.len();
        let cwd = self.tabs.active_cwd(&self.tabmeta_snapshot);
        let active_tab = self.tabs.active().id;
        let shell_pid = self.tabs.active_shell_pid();
        let mut probe = WriteProbe::new(&mut self.tabs.active_mut().writer);
        let mut ctx = AppCtx::new(&mut probe, count, active, cwd)
            .with_active_tab(active_tab, None)
            .with_shell_pid(shell_pid);
        let handled = if covering {
            self.overlays.press_covering(p, &mut ctx)
        } else {
            self.overlays.press(p, &mut ctx)
        };
        let requests = ctx.take_requests();
        if probe.wrote() {
            self.snap_active_after_input();
        }
        (handled, requests)
    }

    /// Who the pointer at `last_cursor` belongs to (session-flow-graph D6) — the three
    /// flags of [`input::pointer_owner`], read from the live window. The press, wheel and
    /// cursor-moved arms all ask here, so they cannot disagree about who owns a point.
    fn pointer_owner(&self) -> PointerOwner {
        // An open graph owns every point, so the hit-tests below could not change the
        // answer; skipping them keeps a pointer move under the graph geometry-free.
        if self.overlays.covering_visible() {
            return PointerOwner::Graph;
        }
        let p = [self.last_cursor.0 as f32, self.last_cursor.1 as f32];
        let in_monitor_band = input::in_monitor_panel(
            self.last_cursor.0,
            self.viewport_w(),
            self.renderer.right_inset(),
        );
        let in_sidebar =
            sidebar_layout::hit_test(p, self.scroll, &self.sidebar_input(), self.collapsed())
                .is_some();
        input::pointer_owner(
            self.overlays.covering_visible(),
            in_monitor_band,
            in_sidebar,
        )
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
                AppRequest::SetMonitorOpen(open) => {
                    // The chord decides open-or-closed and nothing else: D6 makes
                    // `[ai_monitor].width_px` plus a runtime override the authority for the
                    // width, and the renderer derives the band from those — which is what
                    // lets a width dragged last week come back on this toggle (R4.5).
                    //
                    // `toggle_monitor`, not `apply_monitor_open`: this arm IS the chord, and
                    // the chord is the one gesture D8 lets write `[ai_monitor].open`
                    // (R6.3). The launch restore calls the other one.
                    self.toggle_monitor(open);
                }
                AppRequest::ActivateTab(id) => {
                    // session-flow-graph D7 — a graph card click. Through
                    // `do_tab_action` so composition, rename and menu teardown happen exactly
                    // as for a sidebar click. A tab that closed between the press and this
                    // apply resolves to nothing, and nothing happens (R8.3).
                    if let Some(i) = self.tabs.position(id) {
                        if !self.do_tab_action(TabAction::Jump(i)) {
                            return false;
                        }
                    }
                }
                AppRequest::CopyToClipboard(text) => {
                    // session-overview-ui D22 — the detail's `Copy resume cmd`.
                    crate::copy::write_clipboard_string(&text);
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
        // Through the shared derivation (card-live-cwd D7); an empty prefill is this
        // caller's own fallback, since there is nothing sensible to type into the field.
        let cwd = crate::sidebar_model::folder_name(
            session.cwd(self.tabmeta_snapshot.get(id)).as_deref(),
        )
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
    /// renderer's (R1.6), so the FULL list is handed over.
    ///
    /// `titles` is passed in rather than recomputed. That used to be justified by
    /// `titles()`' per-tab `proc_pidinfo` — a cost `published_or` has since removed
    /// (`tabs.rs`) — but the reason that matters now is stronger: `Tabs::titles()` sends
    /// `set_roster` + `set_active` to the metadata worker, and this function runs on the
    /// worker's own publish path. Recomputing here would make a publish trigger a
    /// derivation, which is the feedback cycle card-live-cwd D1 rejects and R5.2 forbids.
    /// Each row's *folder name* is still resolved live, from the snapshot, by
    /// [`band_row_title`].
    fn build_waiting_rows(&self, titles: &[sidebar_row::SidebarRow]) -> Vec<WaitingRow> {
        self.waiting_items()
            .into_iter()
            .filter_map(|item| {
                let row = titles.iter().find(|r| r.tab_id == item.tab_id.0)?;
                // The LIVE folder name (card-live-cwd D5), from the snapshot the caller has
                // just synced. `titles` was assembled on the last tab action, so a band row
                // that outlives a `cd` would keep naming a directory its tab has left — and
                // would then disagree with the card for the same tab. The row's own title is
                // the fallback, the same degrade rule the card uses (D2).
                let title = band_row_title(self.tabmeta_snapshot.get(item.tab_id), &row.title);
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
        // The same sync also carries the tab's `claude` pid. The panel's directory is a
        // function of that process's cwd, which is fixed at its launch — so the directory
        // goes stale exactly when the pid changes, and this wake is where that is noticed.
        let agent_pid = self.tabs.active_agent_pid(&self.tabmeta_snapshot);
        // The same publish also carries whether that agent is mid-turn. The panel needs it to
        // decide whether arming an idle deadline would be a lie: a long `cargo build` leaves a
        // genuinely working session silent for minutes, which is the case the five-minute
        // threshold was sized for, and a deadline armed through it would flip the pill to IDLE
        // while the agent works (idle-timestamps D18).
        let agent_state = self.tabs.active_agent_state(&self.tabmeta_snapshot);
        self.overlays
            .notify_active_changed(cwd, shell_pid, agent_pid, agent_state);
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
        // Every tab's level is re-read and stored on its session, and the band is fed each
        // state through the same override its card resolves with (agent-card-status D4/D7).
        let scan = self.tabs.rescan_dialogs(base, active);
        let changed = self
            .waiting
            .on_agent_states(&scan.states, active, Instant::now());
        // A flip changes that tab's card, a background tab's included — so its row is patched
        // in place. Not `sync_tab_model`: this also runs on a metadata publication, and
        // rebuilding the rows through `Tabs::titles` would re-send the roster to the worker,
        // reopening the publish → derive → publish path card-live-cwd D1/R5.2 closed.
        let flipped = patch_dialog_rows(&self.tab_model, &scan.flipped);
        if tracing::enabled!(tracing::Level::DEBUG) {
            tracing::debug!(
                states = ?scan.states,
                active = active.0,
                changed,
                flipped,
                items = self.waiting.items().len(),
                "waiting-model"
            );
        }
        changed || flipped
    }

    /// Tab `id`'s view just returned to the live bottom (agent-card-status D7(b)): re-read
    /// the level it held while scrolled back from the live screen — once, through the same
    /// single-tab path PTY output uses, so the band is fed in the same step as the card.
    ///
    /// Without it a dialog answered while the tab was scrolled back would stay `Waiting` on
    /// both until the agent next drew something; with it, returning to the screen is enough.
    fn rescan_at_live_bottom(&mut self, id: TabId) {
        if self.push_agent_states_for(id) {
            self.refresh_waiting_band();
            self.redraw();
        }
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

    /// Move the ai-monitor panel's docked band and let the grid follow
    /// (ai-monitor-right-panel D21, R0.1–R0.3). The order is `toggle_sidebar`'s and
    /// load-bearing for the same reason: move the inset FIRST, then let `refit` read
    /// `grid_size`, then redraw.
    ///
    /// The only caller of `set_monitor_open`, and it deliberately does **not** persist.
    /// Persistence is a rule about *gestures* (D8), and this is also the path the launch
    /// restore takes (R6.6) — a restore that wrote here would make every start rewrite the
    /// config it just read, which is how a "persisted" value quietly becomes the app's own
    /// echo. [`toggle_monitor`](Self::toggle_monitor) is the half that remembers.
    fn apply_monitor_open(&mut self, open: bool) {
        let before = self.renderer.grid_size();
        self.renderer.set_monitor_open(open);
        let after = self.renderer.grid_size();
        // Refit ONLY when the column count actually moved (R0.2, and `needs_refit`'s own
        // rationale): `Grid::resize` reallocates every tab's cells, clears the selection
        // and fires SIGWINCH at zsh. A toggle normally does move it — but on a window too
        // narrow to fit the panel at all (R4.4) the band stays 0.0 both ways, and an
        // unguarded refit would then wipe the user's selection in exchange for nothing.
        if input::needs_refit(before, after) {
            crate::settings::refit(&self.renderer, &mut self.tabs);
        }
        self.redraw();
    }

    /// The `Cmd+Shift+M` chord: move the band **and remember it**.
    ///
    /// Reached only from `AppRequest::SetMonitorOpen`, which only `Overlay::on_key` can
    /// record — which is what makes D8's "only the chord writes `open`" a property of the
    /// call graph rather than a convention (R6.3). A width-forced close (R4.4) happens
    /// inside `sync_right_inset` and never reaches here, which is what leaves the persisted
    /// flag recording what the user last asked for.
    fn toggle_monitor(&mut self, open: bool) {
        self.apply_monitor_open(open);
        self.persist_monitor();
    }

    /// Reopen the ai-monitor panel at launch when the last session left it open (R6.6).
    ///
    /// Two halves, in one act, because splitting them in time is visible: the overlay's own
    /// open path resolves this tab's log, folds it and starts the watcher, while the band it
    /// paints into is the renderer's. Reserving the band without opening the overlay leaves
    /// an empty strip; opening the overlay without the band paints nothing. Both happen here,
    /// before the first frame, so neither state is ever drawn.
    ///
    /// **Called with the tab's cwd in hand, and that is why it is here rather than earlier.**
    /// `open()` resolves `~/.claude/projects/**` from the directory it is given; run before
    /// the first session exists and it would resolve the wrong project, which looks exactly
    /// like a panel with no data.
    ///
    /// Writes nothing back (D8): a restore is not a gesture. It goes through
    /// `apply_monitor_open`, which has no persistence path at all — and even the value it
    /// would write is the file's own (`monitor_width_logical` reads `[ai_monitor].width_px`
    /// while no drag has set an override, and `monitor_open` is the flag we just read), so
    /// the guard in `persist_monitor` declines it too.
    fn restore_monitor_panel(&mut self) {
        if !self.last_applied_cfg.ai_monitor.open {
            return;
        }
        let cwd = self.tabs.active_cwd(&self.tabmeta_snapshot);
        let shell_pid = self.tabs.active_shell_pid();
        self.overlays
            .command(&OverlayCommand::RestoreMonitor { cwd, shell_pid });
        self.apply_monitor_open(true);
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
        // Nothing changed since the config we last applied — don't touch the file
        // (`input::needs_persist` owns that rule, shared with the panel's persist below).
        let on_disk = (
            self.last_applied_cfg.sidebar.width_px,
            self.last_applied_cfg.sidebar.collapsed,
        );
        if !input::needs_persist(pair, on_disk, self.sidebar_override_authoritative) {
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

    /// Write the ai-monitor panel's size and open state to `config.toml` (D6/D8) — the ONE
    /// persistence site, shared by the chord and the drag release, so the two cannot
    /// remember differently. Mirrors `persist_sidebar` in every respect, including that a
    /// failure warns and is otherwise silent (D14).
    ///
    /// **What is written is the intent, never the drawn band** (D8). `monitor_width_logical`
    /// is the config-plus-override width, which a narrow window's squeeze does not touch, and
    /// `monitor_open` is the flag only the chord moves — so a window narrowed past R4.4's
    /// threshold force-closes the panel on screen and this function is not even called
    /// (nothing re-derives through here), leaving `open = true` on disk to restore from
    /// (R6.2/R6.3).
    fn persist_monitor(&mut self) {
        let Some(path) = self.config_path.clone() else {
            tracing::warn!("no config path; ai-monitor panel size not persisted");
            return;
        };
        let pair = (
            self.renderer.monitor_width_logical(),
            self.renderer.monitor_open(),
        );
        // Nothing changed since the config we last applied — don't touch the file
        // (`input::needs_persist`, shared with `persist_sidebar`). This is also the second
        // line of defence for R6.6: a launch restore never calls this function, and if it
        // ever did, the pair it would write is the pair it just read.
        let on_disk = (
            self.last_applied_cfg.ai_monitor.width_px,
            self.last_applied_cfg.ai_monitor.open,
        );
        if !input::needs_persist(pair, on_disk, self.monitor_override_authoritative) {
            return;
        }
        match common::config::save_ai_monitor(&path, pair.0, pair.1) {
            Ok(()) => {
                // Arm the echo latch ONLY on success (D15/D14×D15a): a failed write leaves
                // the file disagreeing with the override on purpose.
                input::remember_write(&mut self.written_monitor, pair);
                self.monitor_override_authoritative = false;
                // Fold the written values into the applied-config baseline, so the echo this
                // write provokes compares equal and is suppressed — while a delivery that
                // ALSO changes another field still differs, and applies (R6.4).
                self.last_applied_cfg.ai_monitor.width_px = pair.0;
                self.last_applied_cfg.ai_monitor.open = pair.1;
            }
            Err(e) => {
                tracing::warn!("ai-monitor panel size not saved: {e}");
                self.written_monitor.clear();
                self.monitor_override_authoritative = true;
            }
        }
    }

    /// Write `[font].size_pt` to `config.toml` (terminal-font-size D9/R4.3/R5.1–R5.3).
    /// Mirrors `persist_sidebar` and `persist_monitor`: a failure warns and marks
    /// `font_override_authoritative = true` so the session retains the runtime font size
    /// across unrelated reloads.
    fn persist_font_size(&mut self) {
        let Some(path) = self.config_path.clone() else {
            tracing::warn!("no config path; font size not persisted");
            return;
        };
        let size_pt = self.renderer.font_size_logical();
        let on_disk = self.last_applied_cfg.font.size_pt;
        if !input::needs_persist(size_pt, on_disk, self.font_override_authoritative) {
            return;
        }
        match common::config::save_font_size(&path, size_pt) {
            Ok(()) => {
                self.last_written_font = Some(size_pt);
                self.font_override_authoritative = false;
                self.last_applied_cfg.font.size_pt = size_pt;
            }
            Err(e) => {
                tracing::warn!("font size not saved: {e}");
                self.last_written_font = None;
                self.font_override_authoritative = true;
            }
        }
    }

    /// Apply a window-global font size chord (`Cmd+=` / `Cmd++`, `Cmd+-`, `Cmd+0`;
    /// terminal-font-size D7–D11, R0.1, R4.1, R4.3, R4.4).
    ///
    /// Dismisses any open card menu (R4.1). When `Renderer::set_font_size` returns `false`
    /// (boundary no-op at `8.0`, `32.0`, or `14.0` on reset, D10/R4.4), returns early so
    /// no `settings::refit` (which clears active text selection and sends `SIGWINCH`),
    /// redraw, or disk write occurs.
    fn do_font_action(&mut self, action: FontAction) {
        self.close_card_menu();
        if !apply_font_action(&mut self.renderer, &mut self.tabs, action) {
            return;
        }
        let inp = self.sidebar_input();
        self.scroll = sidebar_layout::clamp_scroll(self.scroll, &inp);
        self.publish_scroll();
        self.refresh_hover();
        self.update_ime_cursor_area();
        self.redraw();
        self.persist_font_size();
    }

    /// End an in-flight resize: commit the width and persist it (tab-resize D2/R7.3/R7.6).
    /// Idempotent — clearing `resize_drag` first means a later `Released` after a
    /// focus-loss commit finds nothing armed and does not write a second time.
    fn finish_resize(&mut self) {
        // Which edge was dragged decides which table is written — `input::persist_target`
        // owns that rule, so the "a panel drag must not rewrite `[sidebar]`" half of R6.1
        // is a test rather than a comment on this match.
        match input::persist_target(self.resize_drag.take()) {
            Some(PersistTarget::Sidebar) => self.persist_sidebar(),
            // R6.1: a released panel drag writes `[ai_monitor].width_px`. It writes `open`
            // alongside it, which does not violate D8's "only the chord" rule — the value
            // written is the chord's own intent, read back from the renderer, not a state
            // this gesture decided.
            Some(PersistTarget::AiMonitor) => self.persist_monitor(),
            None => {}
        }
    }

    /// Which gesture a left-release belongs to (tab-resize D17/R7.3) — the guard both
    /// `Released` arms read, so their precedence is a tested rule rather than source order.
    fn release_target(&self) -> ReleaseTarget {
        input::release_target(self.resize_drag.is_some(), self.dragging)
    }

    /// Arm a chrome edge resize at the current pointer (tab-resize D9/D17/R7.1; panel edge
    /// D17/R9.4).
    ///
    /// Captures the grab offset against the edge's *live* position, once — `drive_resize`
    /// never recomputes it (R3.4). Clears `dragging` to uphold the D17 invariant: nothing
    /// clears it on `Focused(false)`, so a lost `Released` can leave it stale, and a stale
    /// selection drag racing a resize is the one way both could be armed at once.
    ///
    /// One slot for both edges (D17): only one of them can be under the pointer when the
    /// button goes down, so which edge was grabbed is a field of the gesture, not a second
    /// gesture.
    fn begin_resize(&mut self, edge: ResizeEdge) {
        let edge_x = match edge {
            ResizeEdge::Sidebar => self.renderer.left_inset(),
            ResizeEdge::Monitor => {
                input::monitor_edge_x(self.viewport_w(), self.renderer.right_inset())
            }
        };
        self.dragging = false;
        self.resize_drag = Some(ResizeDrag {
            grab_offset: input::resize_grab_offset(self.last_cursor.0, edge_x),
            edge,
        });
        debug_assert!(
            self.resize_drag.is_some() && !self.dragging,
            "D17 invariant: a resize drag and a selection drag are never both armed"
        );
    }

    /// The framebuffer width in physical px — the same `inner_size` `sidebar_input` reads,
    /// so the sidebar's geometry, the panel's grab band and `pixel_to_cell`'s right bound
    /// cannot disagree about where the window's right edge is.
    fn viewport_w(&self) -> f32 {
        self.window.inner_size().width as f32
    }

    /// Is the pointer inside the ai-monitor panel's grab band (D17/R9.4)? The panel half of
    /// the press routing, kept beside `sidebar_input` so both hit tests read their geometry
    /// from the same window size.
    fn in_monitor_grab_band(&self) -> bool {
        input::in_monitor_grab_band(
            self.last_cursor.0,
            self.viewport_w(),
            self.renderer.right_inset(),
            self.renderer.scale_factor(),
        )
    }

    /// Advance an in-flight resize to the current pointer — dispatching on the edge the
    /// press grabbed (D17).
    fn drive_resize(&mut self, drag: ResizeDrag) {
        match drag.edge {
            ResizeEdge::Sidebar => self.drive_sidebar_resize(drag),
            ResizeEdge::Monitor => self.drive_monitor_resize(drag),
        }
    }

    /// Advance an in-flight ai-monitor panel resize (D17/R9.6/R9.7).
    ///
    /// Order is `drive_sidebar_resize`'s, and load-bearing for the same reasons: set the
    /// width FIRST so the inset is live, then read `grid_size`, then refit. Hover is not
    /// re-resolved — the drag owns the pointer.
    ///
    /// The width is handed over **raw**; `Renderer::set_monitor_width` owns the 300–760
    /// clamp, so the band the user sees and the number that gets persisted come from one
    /// expression. There is no collapse threshold to compare a raw width against here (the
    /// panel has no collapsed state — D8's `open` flag is the chord's alone), which is the
    /// one place the two gestures genuinely differ rather than mirror.
    fn drive_monitor_resize(&mut self, drag: ResizeDrag) {
        let scale = self.renderer.scale_factor();
        let viewport_w = self.viewport_w();
        let raw = input::resize_raw_monitor_width(self.last_cursor.0, drag, viewport_w, scale);
        // The grid size the tabs are currently fitted to. `settings::refit` is the only
        // site that resizes them (D6), so this is always their true dimension.
        let before = self.renderer.grid_size();
        self.renderer.set_monitor_width(raw);

        // Refit ONLY when the column count actually changed (R9.7). A correctness guard,
        // not an optimisation, and the identical one `drive_sidebar_resize` documents:
        // `Grid::resize` reallocates every tab's cells AND clears the selection, so an
        // unguarded per-pixel refit would wipe the user's selection on every pointer move
        // and fire a SIGWINCH at zsh on every tab. The band still follows the pointer every
        // frame — only the grid steps by whole columns.
        let after = self.renderer.grid_size();
        if input::needs_refit(before, after) {
            crate::settings::refit(&self.renderer, &mut self.tabs);
        }
        // No scroll re-clamp: the sidebar's list did not move — its geometry is measured
        // from the LEFT edge, and nothing on the right can change it.
        self.redraw();
    }

    /// Advance an in-flight sidebar resize to the current pointer (tab-resize D1/D9).
    ///
    /// Order is load-bearing and copied from `toggle_sidebar` (D6): set the width FIRST so
    /// the inset is live, then re-fit the grid, then rebuild the layout from the new width
    /// and re-clamp the scroll. Hover is deliberately NOT re-resolved — the drag owns the
    /// pointer (R7.4).
    fn drive_sidebar_resize(&mut self, drag: ResizeDrag) {
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
        // only fires below 240, so a continuous drag from 400 writes 260 all the way
        // through the [240, 260) hysteresis band before the collapse happens. Restoring
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
        // The panel's grab band gets the same `col-resize` affordance, and is asked
        // second — the same order the press uses (R9.5), so what the cursor promises and
        // what a press does can never disagree at an overlap. D10 draws no grip on either
        // edge, so this mapping is the whole of the band's discoverability.
        let icon = match hover_cursor(region) {
            CursorIcon::Default if self.in_monitor_grab_band() => CursorIcon::ColResize,
            other => other,
        };
        self.set_cursor_icon(icon);
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
    /// chords as dismissals.) The one exception is the window-covering graph, decided by
    /// [`consumed_key_closes_menu`] from `covered_before` and the state after dispatch.
    fn finish_consumed_key(
        &mut self,
        menu_was_open: bool,
        covered_before: bool,
        wants_redraw: bool,
    ) {
        let covered_after = self.overlays.covering_visible();
        if consumed_key_closes_menu(menu_was_open, covered_before, covered_after) {
            self.close_card_menu();
        }
        if cover_ends_resize(covered_before, covered_after, self.resize_drag.is_some()) {
            // Commit the width as it stands; the release then finds nothing armed.
            self.finish_resize();
            self.clear_hover();
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
        let agent_pid = self.tabs.active_agent_pid(&self.tabmeta_snapshot);
        let agent_state = self.tabs.active_agent_state(&self.tabmeta_snapshot);
        self.overlays.notify_active_changed(
            cwd,
            self.tabs.active_shell_pid(),
            agent_pid,
            agent_state,
        );
        self.redraw();
    }

    /// Map the latest cursor position to a live-viewport cell, or `None` over the
    /// sidebar/top band or the docked ai-monitor panel (selection-input; D16). The one
    /// geometry the event loop performs, via the pure `pixel_to_cell` helper + the
    /// renderer's cell/inset accessors.
    ///
    /// The viewport width comes from the window, matching `sidebar_input` — the renderer's
    /// surface is configured from the same `inner_size`, so the two cannot disagree about
    /// where the right edge is.
    fn cursor_cell(&self) -> Option<(u16, u16)> {
        let (cols, rows) = self.tabs.active().terminal.dimensions();
        pixel_to_cell(
            self.last_cursor,
            self.renderer.cell_size(),
            self.renderer.left_inset(),
            self.renderer.top_inset(),
            self.viewport_w(),
            self.renderer.right_inset(),
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
        // ⌘ lights nothing while an app-tracked drag owns the pointer (mouse-drag-reporting
        // D6). A mask, not an early return: `on_modifier` must still re-arm after a focus
        // loss, or a ⌘ pressed mid-drag would leave the link dead after the release.
        let drag_owned = drag_owns_pointer(
            self.mouse_reported,
            self.tabs.active().terminal.mouse_protocol(),
        );
        let input = HoverInput {
            cmd_held: self.mods.super_key() && !drag_owned,
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
                let after = term.view_offset();
                if after != before {
                    self.redraw();
                }
                if returned_to_live(before, after) {
                    let id = self.tabs.active().id;
                    self.rescan_at_live_bottom(id);
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
        // agent-card-status D7(b), for a background tab too: the level this tab held while
        // scrolled back is re-read now that its live screen is showing again. On the
        // output snap this runs one scan ahead of the PTY arm's own — a 20-row text read,
        // only when the view was within a notch of the bottom.
        if moved {
            self.rescan_at_live_bottom(id);
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

    /// Write background-produced bytes to tab `id`'s shell (region-capture D10/R7.4).
    ///
    /// The tab is named, not assumed active: a capture started seconds ago belongs to
    /// the tab that asked for it, and the roster may have moved since (D6). A tab that
    /// has since closed is simply not found — no other tab receives the bytes, nothing
    /// panics, nothing surfaces (R7.7). The file it would have named stays on disk.
    ///
    /// Snapping on success mirrors the typed-key path exactly, including its failure
    /// arm: this is the **input** snap, which has no `SNAP_FLOOR`, so a delivery brings
    /// even a far-scrolled viewport back to where the path will appear (D13/R9). That
    /// is right because the user did originate it, by pressing the chord.
    fn deliver_to_tab(&mut self, id: TabId, bytes: &[u8]) {
        if self.tabs.deliver(id, bytes) {
            self.snap_after_input(id);
        }
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
        // Whether the session-flow graph covered the window when this key arrived — read
        // before dispatch, because the key itself may open or close it (D5).
        let covered_before = self.overlays.covering_visible();
        // Esc dismisses an open card menu before anything else sees it
        // (sticky-tab-title R3.8) — ahead of the overlays and of `encode_key`, which
        // would otherwise send the ESC on to the shell while the popup stayed up. Not
        // under the graph: there Esc is the graph's close key (session-flow-graph D5).
        let is_esc = matches!(
            input.key,
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
        );
        if esc_dismisses_menu(self.open_menu.is_some(), is_esc, covered_before) {
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
        // text (copy-clipboard). The text comes from the grid, not a snapshot
        // (selection-scroll-anchor D6): it reads scrollback + live by line, so it
        // returns a selection that has partly scrolled off-screen, and it clones no
        // grid. With no selection it returns `None` at once, so the common keystroke
        // stays cheap.
        let bracketed = self.tabs.active().terminal.bracketed_paste();
        let selection_text = self.tabs.active().terminal.selection_text();
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
                self.finish_consumed_key(menu_was_open, covered_before, wants_redraw);
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
                self.finish_consumed_key(menu_was_open, covered_before, wants_redraw);
                return true;
            }
        }
        // 3a) Settings chord (Cmd+,; top-settings-menu D4/R3), intercepted before the shell.
        if is_settings_chord(&input.key, &input.mods) {
            crate::settings_menu::open_settings();
            return true;
        }
        // 3b) The sidebar toggle chord (tab-resize D17a), intercepted before the shell for
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
        // 3c) Font size chords (Cmd+= / Cmd++, Cmd+-, Cmd+0; terminal-font-size D7/D11, R4.1),
        // intercepted before the shell alongside sidebar toggle and tab chords.
        if let Some(action) = font_action(&input.key, &input.mods) {
            self.do_font_action(action);
            return true;
        }
        // 3d) Tab chords, intercepted before the shell (D5/R3.2).
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
        // The last tab closed and the loop is on its way out; an event already queued in
        // this iteration has nothing left to act on (`Tabs::is_empty`).
        if inner.tabs.is_empty() {
            return;
        }
        match event {
            UserEvent::PtyOutput(id) => {
                // Advance the originating tab's grid; redraw only if active (R1.2).
                let is_active = inner.tabs.is_active(id);
                let active_id = inner.tabs.active().id;
                // Drain, and pull any OSC 133 command-end the shell emitted on this
                // (possibly background) session's PTY (T1 → T3).
                let mut command_end = None;
                let mut title_changed = false;
                let mut card_changed = false;
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
                        card_changed = s.take_command_state_dirty();
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
                // shows immediately — even for a background tab (tab-identity). A command
                // starting or finishing does the same for the card's status
                // (sidebar-weather-cards D8): `Running` / `Exited N` ride on the rows.
                let (sync_rows, needs_redraw) =
                    drain_effects(advanced, is_active, title_changed, card_changed);
                if sync_rows {
                    inner.sync_tab_model();
                }
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
                //
                // Does NOT advance `band_gen` (card-live-cwd D5): the R3.2 gate lives on the
                // `OverlayPending` arm, and this rebuild is screen-driven. Advancing it here
                // would also be *accurate* — `build_waiting_rows` reads this same snapshot,
                // and `sync` writes a snapshot's generation only alongside its data, so the
                // two can never disagree — but leaving it costs at most one redundant,
                // idempotent rebuild on the next publish. `sync_tab_model` makes the same
                // choice, so this is the convention here rather than a special case.
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
                    // The panel's pair is classified the same way, against its OWN write
                    // history (right-panel D6/R6.4/R6.5). Two independent verdicts, because
                    // the two gestures are independent: a sidebar drag's echo must not clear
                    // the panel's override, and a hand edit to `[ai_monitor].width_px` must
                    // win even though `[sidebar]` in the same file is unchanged.
                    //
                    // Both run BEFORE `settings::apply` — `apply_config` re-derives both
                    // insets from config-plus-override, so clearing afterwards would apply
                    // the override once and the file's value only on the next reload.
                    let delivered_monitor = (cfg.ai_monitor.width_px, cfg.ai_monitor.open);
                    let monitor_delivery =
                        input::classify_delivery(delivered_monitor, &inner.written_monitor);
                    if input::should_clear_override(
                        monitor_delivery,
                        inner.monitor_override_authoritative,
                    ) {
                        inner.renderer.clear_monitor_override();
                    }
                    // Reconcile `[font].size_pt` against `last_written_font` (terminal-font-size
                    // D9/R5.1–R5.3). If a previous save failed (`font_override_authoritative`)
                    // and `cfg.font.size_pt` still equals `last_applied_cfg.font.size_pt`, this
                    // reload is for an unrelated field and must keep the runtime font override;
                    // if the human edited `font.size_pt` to a new value, clear the override.
                    if should_clear_font_override(
                        cfg.font.size_pt,
                        inner.last_applied_cfg.font.size_pt,
                        inner.last_written_font,
                        inner.font_override_authoritative,
                    ) {
                        inner.renderer.clear_font_override();
                        inner.font_override_authoritative = false;
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
                //
                // Since idle-timestamps D2 this does more than fold in tailed data: while the
                // panel is visible it rebuilds the panel's model on **every** wake through
                // here, whatever produced the wake, and reports whether the model differs.
                // Three producers reach this arm (the panel's own watcher, the tabmeta worker,
                // and `refresh_rivals`' probe ping), and on the latter two the old `poll`
                // returned false while the app went on to redraw for the waiting band — a
                // frame drawn from a model nobody had rebuilt.
                //
                // **The cost, stated as it actually is: no new WAKES, which is not the same as
                // free.** An earlier framing called this a free refresh; it is not. It is one
                // `dashboard::model` per wake, plus a full stage-one re-shape on the subset of
                // those wakes where a formatted string actually changed — commonly most of
                // them for a session under an hour old, where the UPTIME and IDLE tiles tick
                // every second, and few for an older one, where `dur()` has gone
                // minute-granular. What it does not do is create a single frame that was not
                // already going to be drawn, and absorbing a re-shape is what the panel's
                // two-stage cache exists for. The weaker claim is the true one, and writing
                // the weaker one down is what stops a later reader trusting the stronger.
                let overlay_changed = inner.overlays.poll_all();
                // A screen capture that finished on its watcher thread hands over the path
                // to type (region-capture D10/R7.4). Immediately after `poll_all`, because
                // that is the call in which the overlay drained its own channel — so
                // anything it has to hand out exists by now.
                //
                // Generic, not capture-specific: the arm knows only "some overlay produced
                // bytes for some tab", which is what keeps SD4's rule (a feature does not
                // edit this file) intact at one drain block rather than one per feature.
                for (id, bytes) in inner.overlays.take_pending_writes() {
                    inner.deliver_to_tab(id, &bytes);
                }
                // The tab-metadata worker also wakes the loop through this path; sync
                // the fresh agent states into the waiting model (rising-edge Waiting
                // detection, T3) and redraw if the waiting band changed.
                let waiting_changed = inner.feed_agent_states();
                // Republish the band (rows + shifted list_top) before the redraw (T4).
                //
                // The generation arm is ADDITIVE, never a replacement (card-live-cwd R3.2).
                // The band names each tab by its folder, which a `cd` changes without moving
                // any waiting item — so `waiting_changed` alone left band and card naming
                // different directories for one tab. But `waiting_changed` comes from
                // `push_agent_states`, which reads each tab's SCREEN as well as the snapshot,
                // so it can fire on a wake where the generation did not move (a timing-driven
                // Waiting edge). Dropping either arm silently stops a real repaint.
                //
                // One condition, not two `if`s: a publish that also flips a tab to Waiting
                // must rebuild the band once, not twice.
                if band_needs_rebuild(
                    waiting_changed,
                    inner.band_gen,
                    inner.tabmeta_snapshot.generation(),
                ) {
                    inner.band_gen = inner.tabmeta_snapshot.generation();
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
        // As in `user_event`: after the last tab closes, a queued redraw or input event would
        // reach `Tabs::active` on an empty set. `exit()` was already called by the close.
        if inner.tabs.is_empty() {
            return;
        }
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
                // An open session-flow graph covers the window, so the hover is its and no one
                // else's (session-flow-graph D6): no card or link lights up under a surface the
                // user cannot see. The continuous redraw (C6) paints the change.
                if inner.pointer_owner() == PointerOwner::Graph {
                    let p = [inner.last_cursor.0 as f32, inner.last_cursor.1 as f32];
                    inner.overlays.hover_covering(Some(p));
                    return;
                }
                // A drag the running app tracks OWNS the pointer (mouse-drag-reporting D6),
                // the same way a resize drag does above: the app gets one motion report per
                // new cell, clamped to the grid's edge (D4), and the latch follows so the
                // release lands where the drag ended (D3). Hover is withheld until then —
                // no card or link lights up under a gesture that belongs to the app.
                let protocol = inner.tabs.active().terminal.mouse_protocol();
                if drag_owns_pointer(inner.mouse_reported, protocol) {
                    let (cols, rows) = inner.tabs.active().terminal.dimensions();
                    let cell = pixel_to_cell_clamped(
                        inner.last_cursor,
                        inner.renderer.cell_size(),
                        inner.renderer.left_inset(),
                        inner.renderer.top_inset(),
                        inner.viewport_w(),
                        inner.renderer.right_inset(),
                        cols,
                        rows,
                    );
                    if let Some(cell) = cell {
                        let encoding = inner.tabs.active().terminal.mouse_encoding();
                        if let Some(bytes) = reported_drag_motion(
                            &mut inner.mouse_reported,
                            protocol,
                            encoding,
                            cell,
                        ) {
                            if let Err(e) = inner.tabs.active_mut().write(&bytes) {
                                tracing::warn!("pty write failed: {e}");
                            }
                        }
                    }
                    return;
                }
                inner.refresh_hover(); // one hit_test feeds both hover fields (D8)
                inner.refresh_link_hover(LinkTrigger::Pointer); // trigger 3 (D24)

                // Left-drag selection (selection-input): once the pointer leaves the
                // press cell, set the drag tab's selection live. Every rule — which tab,
                // when `drag_moved` flips, when a cleared selection spends the drag —
                // is `SelectionDrag::drive`'s (selection-scroll-anchor D3); this arm
                // only resolves the cell and hands over the active tab.
                if inner.dragging {
                    if let (Some(cell), Some(drag)) = (inner.cursor_cell(), inner.drag.as_mut()) {
                        let active = inner.tabs.active_mut();
                        if drag.drive(active.id, cell, &mut active.terminal) {
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
                // The graph's hover ends with the pointer (session-flow-graph D3/D6); a no-op
                // while no window-covering overlay is up.
                inner.overlays.hover_covering(None);
                inner.clear_hover();
                // Trigger 3, the leaving half (hyperlinks D24): nothing is under the
                // pointer, and `last_cursor` keeps its stale value — so the flag is
                // what stops a later ⌘ from lighting a link nobody is pointing at.
                inner.pointer_inside = false;
                inner.clear_link_hover();
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                let p = [inner.last_cursor.0 as f32, inner.last_cursor.1 as f32];
                let owner = inner.pointer_owner();

                if button == MouseButton::Left {
                    // An open session-flow graph takes the press before every other handler, and
                    // keeps it whether or not it lands on a card (session-flow-graph D6/R2.3): a
                    // press falling through would start a selection under a covered screen.
                    if owner == PointerOwner::Graph {
                        let (_, requests) = inner.press_overlays(p, true);
                        if !inner.apply_requests(requests) {
                            event_loop.exit();
                            return;
                        }
                        inner.window.request_redraw();
                        return;
                    }
                    // The ai-monitor panel resolves next, by pointer position alone — the same
                    // order and the same reason as the wheel below (D7/D10). The panel reserves
                    // its columns rather than covering them, so a press inside its band was
                    // never the grid's to begin with.
                    if owner == PointerOwner::Monitor {
                        let (handled, requests) = inner.press_overlays(p, false);
                        // What the press asked for — a graph card's `ActivateTab` (session-flow-
                        // graph D7) — is applied before anything else runs, so it is never
                        // dropped, consumed or not.
                        if !inner.apply_requests(requests) {
                            event_loop.exit();
                            return;
                        }
                        if handled == Handled::Consumed {
                            // A fold changes the document, so the panel must re-lay out; the
                            // grid did not move, so the cheap request is the right one.
                            inner.window.request_redraw();
                            return;
                        }
                    }
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
                    // R9.5/D17: the sidebar-then-panel-then-grid order is `input::press_route`'s
                    // decision, not this `match`'s shape — reordering these arms now changes
                    // nothing, and changing the rule fails a test.
                    match input::press_route(action, inner.in_monitor_grab_band()) {
                        PressRoute::Sidebar(action) => match action {
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
                            SidebarAction::BeginResize => inner.begin_resize(ResizeEdge::Sidebar),
                            SidebarAction::Inert => {}
                        },
                        PressRoute::MonitorEdge => inner.begin_resize(ResizeEdge::Monitor),
                        PressRoute::Grid => {
                            if let Some(cell) = inner.cursor_cell() {
                                let active = inner.tabs.active();
                                let term = &active.terminal;
                                match grid_press_action(
                                    term.mouse_protocol(),
                                    term.mouse_encoding(),
                                    button,
                                    cell,
                                    &inner.mods,
                                ) {
                                    GridPressAction::Report(bytes) => {
                                        if let Err(e) = inner.tabs.active_mut().write(&bytes) {
                                            tracing::warn!("pty write failed: {e}");
                                        }
                                        inner.mouse_reported = Some((button, cell.0, cell.1));
                                    }
                                    GridPressAction::Drag => {
                                        let active = inner.tabs.active();
                                        inner.drag = Some(SelectionDrag::press(
                                            active.id,
                                            cell,
                                            &active.terminal,
                                        ));
                                        inner.dragging = true;
                                        inner.link_cmd_at_press = inner.mods.super_key();
                                        inner.drag_latch = DragLatch::default();
                                    }
                                    GridPressAction::Ignore => {}
                                }
                            }
                        }
                    }
                } else if owner == PointerOwner::Grid {
                    // Right or Middle click over the grid when mouse reporting is active (D5).
                    if let Some(cell) = inner.cursor_cell() {
                        let active = inner.tabs.active();
                        let term = &active.terminal;
                        if let GridPressAction::Report(bytes) = grid_press_action(
                            term.mouse_protocol(),
                            term.mouse_encoding(),
                            button,
                            cell,
                            &inner.mods,
                        ) {
                            if let Err(e) = inner.tabs.active_mut().write(&bytes) {
                                tracing::warn!("pty write failed: {e}");
                            }
                            inner.mouse_reported = Some((button, cell.0, cell.1));
                        }
                    }
                }
            }

            // End a mouse report forward to the PTY (mouse-button-reporting D4).
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } if inner.mouse_reported.is_some_and(|(b, _, _)| b == button) => {
                let rep = inner.mouse_reported.take();
                let active = inner.tabs.active();
                let term = &active.terminal;
                if let GridReleaseAction::Report(bytes) = grid_release_action(
                    rep,
                    term.mouse_protocol(),
                    term.mouse_encoding(),
                    button,
                    &inner.mods,
                ) {
                    if let Err(e) = inner.tabs.active_mut().write(&bytes) {
                        tracing::warn!("pty write failed: {e}");
                    }
                }
                // The drag no longer owns the pointer (mouse-drag-reporting D6): bring back
                // the hover it held off, without waiting for a move. `Snapshot`, not
                // `Pointer` — it resolves past the same-cell early return, and never re-arms
                // on its own, so a ⌘ held across the release lights the link under a
                // stationary pointer while the focus-loss guard stays intact.
                inner.refresh_hover();
                inner.refresh_link_hover(LinkTrigger::Snapshot);
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
                // A release on a tab other than the press's is no click at all
                // (`drag::is_click`): it neither opens the link lit there nor clears
                // that tab's selection.
                let active = inner.tabs.active().id;
                let is_click = drag::is_click(inner.drag, active);
                let url = url_to_open(ClickInput {
                    cmd_at_press: inner.link_cmd_at_press,
                    cmd_at_release: inner.mods.super_key(),
                    drag_moved: !is_click,
                    lit: inner.link_handle.span(),
                });
                let should_snap = should_snap_on_click(
                    is_click,
                    url.is_some(),
                    &inner.mods,
                    inner.tabs.active().terminal.at_bottom(),
                );
                // End the drag (selection-input). A plain click — the pointer never
                // left the press cell — clears any selection; a real drag leaves the
                // selection standing for Cmd+C. Unchanged by the open above: an
                // opening click still clears the selection, exactly as it does today.
                // `drag::release` decides, against the active tab, and the drag is
                // consumed: the next press builds a fresh one.
                inner.dragging = false;
                if drag::release(
                    inner.drag.take(),
                    active,
                    &mut inner.tabs.active_mut().terminal,
                ) {
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
                if should_snap {
                    inner.snap_tab_to_live(active);
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
                let cell_h = inner.renderer.cell_size().1;
                let owner = inner.pointer_owner();
                // An open session-flow graph scrolls, wherever the pointer is — never the
                // monitor, the scrollback or the sidebar under it (session-flow-graph D6).
                // Its `Passthrough` (at an end) falls through to nothing.
                if owner == PointerOwner::Graph {
                    if inner.overlays.wheel_covering(wd, cell_h) == Handled::Consumed {
                        inner.window.request_redraw();
                    }
                // The ai-monitor panel resolves next, and only by where the pointer is
                // (D7/D10). This is a trade, not an addition: wheeling the scrollback with
                // the pointer parked over the panel stops working, and D9 gives it back only
                // at the panel's two ends. `Passthrough` is the panel declining — it is at
                // that end and the gesture pushes further into it — so the event falls
                // through to the grid below.
                } else if owner == PointerOwner::Monitor
                    && inner.overlays.wheel(wd, cell_h) == Handled::Consumed
                {
                    // Panel-only change: the grid's snapshot did not move, so the cheap
                    // request is right — `redraw()` would mark it dirty for nothing.
                    inner.window.request_redraw();
                } else if sidebar_layout::hit_test(p, inner.scroll, &inp, collapsed).is_some() {
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

            WindowEvent::Ime(ime) => {
                // A replayed chord can be Cmd+W on the last tab (korean-input-chords D7).
                let keep_running = inner.on_ime(ime);
                if !keep_running {
                    event_loop.exit();
                }
            }

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
                // Release an in-flight mouse report if focus is lost (mouse-button-reporting D4).
                if let Some((btn, col, row)) = inner.mouse_reported.take() {
                    let active = inner.tabs.active();
                    let term = &active.terminal;
                    if let GridReleaseAction::Report(bytes) = grid_release_action(
                        Some((btn, col, row)),
                        term.mouse_protocol(),
                        term.mouse_encoding(),
                        btn,
                        &inner.mods,
                    ) {
                        let _ = inner.tabs.active_mut().write(&bytes);
                    }
                }
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } if key_event.state == ElementState::Pressed => {
                // The event a commit already replayed, delivered again (D8). One-shot, and
                // ahead of the composing gate; releases never reach this arm.
                if inner.replayed_ts.is_some()
                    && ime_chord::take_duplicate(
                        &mut inner.replayed_ts,
                        current_event_facts().map(|f| f.timestamp),
                    )
                {
                    tracing::debug!("chord-replay duplicate dropped");
                    return;
                }
                // Composing gate, ahead of the whole dispatch chain (R6.3/D8):
                // a character key mid-composition belongs to the IME and must never
                // reach `encode_key`. Named keys still pass (R6.4) — winit forwards
                // arrows/Enter during `Preedit` on purpose.
                if suppress_key(inner.composing, &key_event.logical_key) {
                    return;
                }
                // Under 2-Set Korean winit reports a Cmd chord's logical key as the jamo
                // (`ㅅ` for Cmd+T); `chord_key` swaps in the Roman Command-layer string so
                // every letter chord below matches as it does on ABC, and leaves every
                // other key exactly as winit built it (korean-input-chords D1–D5). The
                // composing gate above deliberately still reads winit's raw key.
                let (key, text) = input::chord_key(
                    &key_event.logical_key,
                    key_event.text.as_deref(),
                    key_event.text_with_all_modifiers(),
                    inner.mods,
                );
                let input = KeyInput {
                    key,
                    text,
                    mods: inner.mods,
                    physical: key_event.physical_key,
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

/// Patch each flipped tab's row with its new dialog level, answering whether any row
/// changed (agent-card-status D7) — the card's half of a scan, as the waiting model's
/// `on_agent_states` is the band's.
/// Each row takes its dialog subject with its level (session-overview-ui D9), so a new
/// subject at an unchanged level is a row change too (D18).
fn patch_dialog_rows(
    model: &TabModelHandle,
    flipped: &[(TabId, bool, Option<renderer::overview_model::DialogSubject>)],
) -> bool {
    flipped.iter().fold(false, |any, (id, level, subject)| {
        model.set_blocking_dialog(id.0, *level, subject.clone()) || any
    })
}

/// Whether `Esc` dismisses an open card menu before any overlay sees it (sticky-tab-title
/// R3.8) — unless the window-covering graph is open, whose close key it is
/// (session-flow-graph D5). The menu waits under the graph and is still there after.
fn esc_dismisses_menu(menu_open: bool, is_esc: bool, covered: bool) -> bool {
    menu_open && is_esc && !covered
}

/// Whether a key an overlay consumed tears down a card menu that was open when it arrived
/// (D8a.1). Not when the window-covering graph was open before the key or is open after
/// it (session-flow-graph D5): the graph does not stack a second modal on the popup, it
/// covers it, and every key while it is open is the graph's — so the menu receives nothing
/// and is still there when the graph closes. Pure, like [`drain_effects`], so the rule is
/// testable without a window.
fn consumed_key_closes_menu(
    menu_was_open: bool,
    covered_before: bool,
    covered_after: bool,
) -> bool {
    menu_was_open && !covered_before && !covered_after
}

/// Whether a key that opened the window-covering graph ends an edge resize in flight
/// (session-flow-graph D6). The graph owns every pointer event while it is open, so a
/// drag left armed would keep resizing the sidebar or panel underneath a surface the user
/// cannot see. Pure, like [`consumed_key_closes_menu`].
fn cover_ends_resize(covered_before: bool, covered_after: bool, resize_armed: bool) -> bool {
    resize_armed && !covered_before && covered_after
}

/// Whether a viewport move landed on the live bottom from somewhere above it — the wheel's
/// share of agent-card-status D7(b). A move that started at the bottom returned from
/// nowhere, and one that stopped short is still history.
fn returned_to_live(before: usize, after: usize) -> bool {
    before != 0 && after == 0
}

/// What one drained PTY chunk asks of the loop: `(sync the sidebar rows, redraw)`.
///
/// A label change or a command starting or ending re-syncs the rows — for a background
/// tab too, since `Running` / `Exited N` ride on them (sidebar-weather-cards D8) — and
/// either one repaints; grid output repaints only the tab on screen. Pure so the rule is
/// testable without a window, like [`snap_on_output`].
fn drain_effects(
    advanced: bool,
    is_active: bool,
    title_changed: bool,
    card_changed: bool,
) -> (bool, bool) {
    let sync_rows = title_changed || card_changed;
    (sync_rows, (advanced && is_active) || sync_rows)
}

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
/// A drag freezes the viewport. The freeze was introduced because `CursorMoved` resolved
/// the drag focus in viewport coords while `Terminal::set_selection` stored live-grid
/// coords, so any offset change mid-drag decoupled "the row under my finger" from "the
/// row I'm selecting". That reason no longer holds — since selection-scroll-anchor D3
/// both ends are content points, the anchor taken at press time (`drag::SelectionDrag`)
/// — but the freeze and this latch are kept unchanged (that spec's D7): lifting the
/// freeze is a separate feature. And suppressing without recording is a trap: `scrollback`
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

/// Compute the clamped target `size_pt` for a [`FontAction`] from the current logical
/// font size (terminal-font-size D7/D10, R4.1–R4.4).
fn target_font_size(current_pt: f32, action: FontAction) -> f32 {
    match action {
        FontAction::Increase => (current_pt + FONT_SIZE_STEP).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX),
        FontAction::Decrease => (current_pt - FONT_SIZE_STEP).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX),
        FontAction::Reset => FONT_SIZE_DEFAULT,
    }
}

/// The renderer-and-tabs half of a font chord (terminal-font-size R0.1, R4.1–R4.4): step
/// the size, and refit every tab when the grid actually changed. Returns `false` on a
/// boundary no-op, before anything is touched. That early return is what keeps `Cmd+0`
/// at the default, or `Cmd+=` at the maximum, from clearing every tab's selection
/// through `refit`.
///
/// Split out of [`Running::do_font_action`] so a test can drive this exact code with a
/// headless renderer, which `Running` (it owns a window) cannot be built around.
fn apply_font_action(
    renderer: &mut Renderer,
    tabs: &mut impl crate::settings::RefitTarget,
    action: FontAction,
) -> bool {
    let target = target_font_size(renderer.font_size_logical(), action);
    let before = renderer.grid_size();
    if !renderer.set_font_size(target) {
        return false;
    }
    if input::needs_refit(before, renderer.grid_size()) {
        crate::settings::refit(renderer, tabs);
    }
    true
}

/// Whether a `ConfigReloaded` delivery should clear the renderer's runtime font override
/// (terminal-font-size D9, R5.1–R5.3).
///
/// When `font_override_authoritative` is `false`, a delivery is an echo only if it equals
/// the **most recent** write; anything else is a human edit and clears the runtime override.
/// Matching against older writes, as the sidebar's ring does, would be wrong here: zooming
/// 14→15→16→15→14 and then hand-editing the file to 16 would read as an echo of the second
/// write, leaving the window at 14 while the file says 16 until restart.
///
/// The cost of keeping only one value: if the watcher read the file between two rapid
/// zoom writes, that stale size reads as a human edit and the font snaps to it for one
/// delivery. The next delivery carries the latest write and settles it, because by then the
/// override is gone and the file is the baseline.
///
/// When `font_override_authoritative` is `true` (a previous `save_font_size` failed), an
/// unrelated reload still delivers the old on-disk `last_applied_pt` and must preserve the
/// runtime override; only an explicit human edit that changes `delivered_pt` away from
/// `last_applied_pt` clears the override.
fn should_clear_font_override(
    delivered_pt: f32,
    last_applied_pt: f32,
    last_written_font: Option<f32>,
    font_override_authoritative: bool,
) -> bool {
    let delivery = input::classify_delivery(delivered_pt, last_written_font.as_slice());
    let authoritative_unchanged = font_override_authoritative && delivered_pt == last_applied_pt;
    input::should_clear_override(delivery, authoritative_unchanged)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- session-flow-graph D5: a card menu survives the graph ----

    /// A band overlay that consumes every key — the shape of rename or the ai-monitor
    /// claiming its chord, which D8a.1 says dismisses a menu underneath.
    struct Claims;
    impl crate::seam::Overlay for Claims {
        fn priority(&self) -> i32 {
            100
        }
        fn visible(&self) -> bool {
            true
        }
        fn on_key(&mut self, _: &KeyInput, _: &mut AppCtx) -> Handled {
            Handled::Consumed
        }
        fn layer(&self) -> Box<dyn renderer::DrawLayer> {
            unreachable!("never drawn in this test")
        }
    }

    /// `on_key`'s card-menu steps, in its order, over a real `OverlayHost`: the Esc
    /// prologue, the overlay dispatch, then `finish_consumed_key`'s decision. `menu_open`
    /// stands in for `open_menu`.
    fn key_with_menu(host: &mut OverlayHost, menu_open: &mut bool, key: &KeyInput) {
        let covered_before = host.covering_visible();
        let is_esc = key.key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape);
        if esc_dismisses_menu(*menu_open, is_esc, covered_before) {
            *menu_open = false;
            return;
        }
        let menu_was_open = *menu_open;
        let mut sink = Vec::new();
        let mut ctx = AppCtx::new(&mut sink, 1, 0, None);
        if host.dispatch(key, &mut ctx) == Handled::Consumed
            && consumed_key_closes_menu(menu_was_open, covered_before, host.covering_visible())
        {
            *menu_open = false;
        }
    }

    fn graph_key(key: winit::keyboard::Key, mods: ModifiersState) -> KeyInput {
        KeyInput {
            key,
            text: None,
            mods,
            physical: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyF),
        }
    }

    fn cmd_shift_f() -> KeyInput {
        graph_key(
            winit::keyboard::Key::Character("F".into()),
            ModifiersState::SUPER | ModifiersState::SHIFT,
        )
    }

    fn esc() -> KeyInput {
        graph_key(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape),
            ModifiersState::empty(),
        )
    }

    fn flow_host() -> OverlayHost {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let mut host = OverlayHost::default();
        host.register(Box::new(crate::flow::FlowOverlay::new(
            wake,
            None,
            renderer::overview_panel::OverviewHandle::new(),
        )));
        host
    }

    #[test]
    fn d5_a_card_menu_is_still_open_after_the_graph_opens_and_closes_over_it() {
        let mut host = flow_host();
        let mut menu = true;
        key_with_menu(&mut host, &mut menu, &cmd_shift_f());
        assert!(host.covering_visible(), "Cmd+Shift+F opened the graph");
        assert!(menu, "opening the graph tore down the card menu under it");
        // Anything typed while covered is the graph's and leaves the menu alone.
        key_with_menu(
            &mut host,
            &mut menu,
            &graph_key(
                winit::keyboard::Key::Character("a".into()),
                ModifiersState::empty(),
            ),
        );
        assert!(menu);
        // Esc is the graph's close key, not the menu's dismissal, while the graph is up.
        key_with_menu(&mut host, &mut menu, &esc());
        assert!(!host.covering_visible(), "Esc closed the graph");
        assert!(menu, "the menu is still there when the graph closes");
        // Uncovered again, Esc dismisses the menu as it always did (sticky-tab-title R3.8).
        key_with_menu(&mut host, &mut menu, &esc());
        assert!(!menu);
    }

    #[test]
    fn d5_closing_the_graph_by_its_chord_also_keeps_the_menu() {
        let mut host = flow_host();
        let mut menu = true;
        key_with_menu(&mut host, &mut menu, &cmd_shift_f());
        key_with_menu(&mut host, &mut menu, &cmd_shift_f());
        assert!(!host.covering_visible());
        assert!(menu);
    }

    #[test]
    fn d8a_1_a_band_overlay_claiming_a_key_still_dismisses_the_menu() {
        // Every overlay but the covering graph is unchanged.
        let mut host = OverlayHost::default();
        host.register(Box::new(Claims));
        let mut menu = true;
        key_with_menu(&mut host, &mut menu, &cmd_shift_f());
        assert!(!menu);
    }

    #[test]
    fn menu_decisions_are_exact() {
        assert!(esc_dismisses_menu(true, true, false));
        assert!(!esc_dismisses_menu(false, true, false));
        assert!(!esc_dismisses_menu(true, false, false));
        assert!(!esc_dismisses_menu(true, true, true));
        assert!(consumed_key_closes_menu(true, false, false));
        assert!(!consumed_key_closes_menu(false, false, false));
        assert!(!consumed_key_closes_menu(true, true, false));
        assert!(!consumed_key_closes_menu(true, false, true));
        assert!(!consumed_key_closes_menu(true, true, true));
    }

    #[test]
    fn only_the_opening_edge_of_the_graph_ends_a_resize() {
        assert!(cover_ends_resize(false, true, true));
        assert!(!cover_ends_resize(false, true, false), "no drag in flight");
        assert!(!cover_ends_resize(false, false, true));
        assert!(
            !cover_ends_resize(true, true, true),
            "already covered: nothing new to end"
        );
        assert!(
            !cover_ends_resize(true, false, true),
            "closing hands the pointer back"
        );
    }

    // ---- closing the last tab: no event runs against an empty tab set ----

    #[test]
    fn every_event_entry_point_stops_once_the_last_tab_is_gone() {
        // Closing the last tab clears the set and calls `event_loop.exit()`, but winit still
        // delivers what was already queued in that iteration. Measured: a debug build closed
        // on its last tab panicked `index out of bounds: the len is 0 but the index is 0` at
        // `Tabs::active` (exit 101). Both handlers need a live window, so no test can call
        // them; this pins the guard at the head of each, the precedent `seam.rs` set for the
        // `OverlayPending` arm. `Tabs::is_empty`'s own contract is tested in `tabs.rs`.
        let src = include_str!("terminal_app.rs");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("terminal_app.rs has a test module to split on");
        let code = production
            .lines()
            .map(|l| l.split("//").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let head = "let Some(inner) = self.inner.as_mut() else { return; }; \
                    if inner.tabs.is_empty() { return; } match event {";
        for handler in ["fn user_event(", "fn window_event("] {
            let body = code
                .split_once(handler)
                .unwrap_or_else(|| panic!("{handler} exists"))
                .1;
            let start = body.find('{').expect("a body") + 1;
            assert!(
                body[start..].trim_start().starts_with(head),
                "{handler} must stop on an empty tab set before touching it: {}",
                &body[start..start + 200]
            );
        }
    }

    // ---- agent-card-status D7 / D4: one stored dialog level, read by band and card ----

    #[test]
    fn a_scans_flips_are_patched_onto_their_rows() {
        // agent-card-status R7.6 / R7.7 — every flipped tab's row takes its new level, and a
        // change is reported when any row moved (a later no-op must not mask an earlier
        // change); an empty scan, or a flip for a closed tab, changes nothing.
        // A fresh model: libtest runs each test on its own thread, and the handle is
        // per-thread.
        let model = TabModelHandle::shared();
        model.set(renderer::sidebar_row::TabBarModel {
            titles: [1, 2]
                .map(|id| renderer::sidebar_row::SidebarRow {
                    tab_id: id,
                    ..Default::default()
                })
                .to_vec(),
            ..Default::default()
        });
        let levels =
            || model.with(|m| m.rows.iter().map(|r| r.blocking_dialog).collect::<Vec<_>>());
        assert!(!patch_dialog_rows(&model, &[]));
        assert!(patch_dialog_rows(
            &model,
            &[(TabId(1), true, None), (TabId(9), true, None)]
        ));
        assert_eq!(levels(), vec![true, false]);
        assert!(patch_dialog_rows(
            &model,
            &[(TabId(2), true, None), (TabId(1), true, None)]
        ));
        assert_eq!(levels(), vec![true, true]);
        assert!(!patch_dialog_rows(&model, &[(TabId(9), false, None)]));
        // Two real flips in one scan: both rows move, not only the first.
        assert!(patch_dialog_rows(
            &model,
            &[(TabId(2), false, None), (TabId(1), false, None)]
        ));
        assert_eq!(levels(), vec![false, false]);

        // session-overview-ui D9 / D18 — the subject rides along, and a new subject at an
        // unchanged level is a row change.
        use renderer::overview_model::DialogSubject::{Command, Question};
        assert!(patch_dialog_rows(
            &model,
            &[(TabId(1), true, Some(Command("ls".into())))]
        ));
        assert!(patch_dialog_rows(
            &model,
            &[(TabId(1), true, Some(Question))]
        ));
        assert_eq!(
            model.with(|m| m.rows[0].blocking_subject.clone()),
            Some(Question)
        );
        assert!(!patch_dialog_rows(
            &model,
            &[(TabId(1), true, Some(Question))]
        ));
    }

    #[test]
    fn the_dialog_level_is_wired_where_no_test_can_call() {
        // agent-card-status D4/D7 — the call sites inside `Running`, which needs a window and
        // so is reachable by no test. The decisions behind them are tested where they live
        // (`Tabs::rescan_dialogs`, `Session::rescan_dialog`, `patch_dialog_rows`,
        // `returned_to_live`); this pins the joins, the precedent `seam.rs` set for the
        // `OverlayPending` arm. Comments are stripped so a join cannot survive as prose.
        let src = include_str!("terminal_app.rs");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("terminal_app.rs has a test module to split on");
        let code = production
            .lines()
            .map(|l| l.split("//").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        let body = |sig: &str| -> String {
            let rest = code
                .split_once(sig)
                .unwrap_or_else(|| panic!("{sig} exists"))
                .1;
            rest.split_once("\n    }\n")
                .unwrap_or_else(|| panic!("{sig} closes"))
                .0
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let push = body("fn push_agent_states_inner(");
        for join in [
            "let scan = self.tabs.rescan_dialogs(base, active);",
            ".on_agent_states(&scan.states, active, Instant::now());",
            "let flipped = patch_dialog_rows(&self.tab_model, &scan.flipped);",
        ] {
            assert!(push.contains(join), "push_agent_states_inner lost `{join}`");
        }
        assert!(push.ends_with("changed || flipped"), "its answer: {push}");
        assert!(
            !push.contains("sync_tab_model") && !push.contains("titles("),
            "the scan must not rebuild the rows through the roster path"
        );
        let rescan = body("fn rescan_at_live_bottom(");
        assert_eq!(
            rescan,
            "&mut self, id: TabId) { if self.push_agent_states_for(id) { self.refresh_waiting_band(); self.redraw(); }"
        );
        let wheel = body("fn wheel_grid(");
        for join in [
            "if after != before { self.redraw(); }",
            "if returned_to_live(before, after) { let id = self.tabs.active().id; self.rescan_at_live_bottom(id); }",
        ] {
            assert!(wheel.contains(join), "wheel_grid lost `{join}`");
        }
        // The three scan events D7(a) rides on — the pre-existing callers, which the level now
        // depends on: a PTY chunk, a metadata publication, a tab switch.
        let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("if advanced && inner.push_agent_states_for(id) {"),
            "the PTY arm no longer scans its tab"
        );
        assert!(
            body("fn feed_agent_states(").ends_with("self.push_agent_states()"),
            "a publication no longer scans"
        );
        assert!(
            body("fn after_roster_change(").contains("self.push_agent_states();"),
            "a tab switch no longer scans"
        );
        let snap = body("fn snap_tab_to_live(");
        assert!(
            snap.contains("if moved { self.rescan_at_live_bottom(id); }"),
            "snap_tab_to_live lost its rescan: {snap}"
        );
    }

    #[test]
    fn only_a_move_onto_the_live_bottom_returns_to_live() {
        // R7.4 — the wheel's share of D7(b): from history to the bottom, and nothing else.
        assert!(returned_to_live(5, 0));
        assert!(returned_to_live(1, 0));
        assert!(!returned_to_live(0, 0), "already live");
        assert!(!returned_to_live(5, 2), "still history");
        assert!(!returned_to_live(0, 3), "a move into history");
    }

    // ---- card-live-cwd R3 + R5.2: the waiting band follows the shell, and its rebuild
    //      path sends the metadata worker nothing ----

    fn meta_at(cwd: &str) -> crate::tabmeta::TabMeta {
        crate::tabmeta::TabMeta {
            graph_agent_cwd: None,
            graph_transcript: None,
            graph_asks_user: false,
            branch: None,
            agent: None,
            cwd: Some(std::path::PathBuf::from(cwd)),
            task: None,
            // Not what these tests are about — the band row's title comes from `cwd`. Present
            // because `TabMeta` is destructured exhaustively by a pin in `tabmeta::worker`, so
            // every literal states every field deliberately.
            agent_pid: None,
            panes: Vec::new(),
            log: None,
            repo: None,
            graph_agent: None,
            graph_task: None,
        }
    }

    #[test]
    fn a_band_row_names_the_live_directory_not_the_assembled_one() {
        // R3.1 — the rows were assembled while the tab sat in `api`; the shell has moved.
        // A band row that kept the old name would disagree with that tab's own card.
        assert_eq!(band_row_title(Some(&meta_at("/Users/x/web")), "api"), "web");
    }

    #[test]
    fn a_band_row_for_an_unpolled_tab_keeps_the_assembled_title() {
        // R3.3 — the same degrade rule the card uses: the row's own title is the only answer
        // during the not-yet-published window, and for a tab whose cwd cannot be resolved.
        assert_eq!(band_row_title(None, "api"), "api");
        assert_eq!(
            band_row_title(Some(&crate::tabmeta::TabMeta::default()), "Tab 3"),
            "Tab 3",
            "a published `cwd: None` (dead pid) is not a name"
        );
    }

    #[test]
    fn the_band_rebuild_gate_keeps_both_arms() {
        // R3.2 — the feature's arm: a `cd` republishes the cwd and moves no waiting item, so
        // the generation is the only signal that the band's folder names went stale.
        assert!(
            band_needs_rebuild(false, 4, 5),
            "a metadata publish must rebuild the band"
        );
        // …and the PRE-EXISTING arm, which this feature must not displace. `waiting_changed`
        // is derived from each tab's screen as well as the snapshot, so it fires on
        // transitions the generation never sees — a permission dialog appearing writes
        // nothing to the session log. Replacing the guard rather than joining it would stop
        // those repaints silently, which is the regression this test exists to catch.
        assert!(
            band_needs_rebuild(true, 5, 5),
            "a waiting-set change with no metadata move must still rebuild"
        );
        // Neither signal → no rebuild, so the idle-CPU property survives (G1).
        assert!(!band_needs_rebuild(false, 5, 5), "a quiet wake stays quiet");
    }

    #[test]
    fn the_band_rebuild_path_sends_no_worker_command() {
        // R5.2 — the constraint that keeps D1's rejected feedback cycle closed: the band now
        // rebuilds on the worker's own publish, so if that path reached `Tabs::titles()` — the
        // sole sender of `set_roster` / `set_active` — a publish would trigger a derivation,
        // which would publish, which would rebuild. `publish_if_changed` is the only brake,
        // and it is exactly what a flapping derivation defeats.
        //
        // A source scan, in the same style as the repaint-timer scan below, because the real
        // property is *reachability* and the alternative — instantiating `Running` — needs a
        // window, a GPU surface and a live PTY. Comment lines are stripped first, so a comment
        // that merely mentions `titles(` cannot fail this.
        let src = include_str!("terminal_app.rs");
        let production = production_half("terminal_app.rs", src);
        let body = production
            .split_once("fn refresh_waiting_band(&mut self)")
            .expect("refresh_waiting_band must exist")
            .1
            .split_once("\n    }\n")
            .expect("its body must be delimited")
            .0;
        // Anchor the scan on something the body definitely contains, so a rename or a
        // reshaped delimiter cannot leave this test scanning an empty string and passing
        // vacuously — the failure mode that makes a source scan worse than no test at all.
        assert!(
            body.contains("build_waiting_rows"),
            "the scan captured no recognisable body: {body:?}"
        );
        for forbidden in ["titles(", "set_roster", "set_active"] {
            assert!(
                !body.contains(forbidden),
                "refresh_waiting_band reached `{forbidden}` — that closes the \
                 publish -> rebuild -> derive -> publish cycle card-live-cwd D1 rejects"
            );
        }
        // The scan above is one level deep — it reads `refresh_waiting_band`'s own body, not
        // its callees. So pin the other end too: `Tabs::titles()` (the sender) must keep
        // exactly one caller, and that caller is `sync_tab_model`, which runs on tab actions
        // rather than on a publish. A new caller anywhere in this file's production half is
        // what would smuggle the sender back onto the publish path without tripping the body
        // scan, so it fails here instead.
        assert_eq!(
            production.matches("self.tabs.titles(").count(),
            1,
            "`Tabs::titles()` must keep exactly one caller (sync_tab_model); a second one \
             risks putting the worker's `set_roster`/`set_active` back on the publish path"
        );
        // …and that the sender is where we think it is. Scanned against tabs.rs's production
        // half for the same reason as above: a mention inside a comment or a test must not
        // satisfy — or trip — a structural claim.
        let tabs = production_half("tabs.rs", include_str!("tabs.rs"));
        assert_eq!(
            tabs.matches("handle.set_active(").count(),
            1,
            "`handle.set_active(` must appear exactly once in tabs.rs (inside Tabs::titles)"
        );
    }

    #[test]
    fn the_band_gate_is_level_triggered_not_edge_triggered() {
        // R3.2 / D5 — the reason the gate compares a stored generation instead of consuming
        // `TabMetaHandle::sync`'s `bool`. `displayed_label` syncs the same snapshot on a
        // Cmd+Shift+R, so an edge can be eaten before `OverlayPending` ever runs. A stored
        // value cannot be eaten: it still differs on the next wake, however many other
        // callers synced in between.
        let (mut band_gen, published) = (7_u64, 9_u64);
        // Another caller already synced the snapshot to `published` — the edge is gone.
        assert!(
            band_needs_rebuild(false, band_gen, published),
            "the rebuild must survive another caller consuming the sync edge"
        );
        band_gen = published;
        assert!(
            !band_needs_rebuild(false, band_gen, published),
            "and must not repeat once the band is caught up"
        );
    }

    // ---- ai-monitor-right-panel R7.4 / D2: the panel introduced no repaint timer ----

    /// The production half of a source file — everything above its test module, with
    /// comment lines dropped so a comment *about* a timer cannot fail the scan.
    ///
    /// Split on the **module** marker at column 0, not on `#[cfg(test)]` anywhere: both
    /// files scanned below carry `#[cfg(test)]` on inline items and inside doc comments,
    /// and splitting on the first occurrence would silently reduce the scan to the first
    /// hundred lines — a scan that passes because it looked at almost nothing. The panic
    /// is the point: a renamed test module must break this loudly, not quietly stop
    /// checking.
    fn production_half(name: &str, src: &str) -> String {
        let (production, tests) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .unwrap_or_else(|| panic!("{name}: no `mod tests` marker to split on"));
        assert!(
            !tests.is_empty() && production.len() > src.len() / 4,
            "{name}: the split found the marker in an implausible place"
        );
        production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// R7.4 — asserted at the **source** level, and that is a deliberate choice rather
    /// than a shortfall.
    ///
    /// The runtime form ("open the panel, wait five seconds, count the frames") needs a
    /// live winit `EventLoop`, which on macOS is main-thread-only and one per process —
    /// `cargo test` cannot host one. What *can* be pinned exactly is the mechanism: a
    /// repaint cadence has to come from somewhere, and there are only two somewheres —
    /// the loop's own control flow, or a wake scheduled against a clock.
    ///
    /// Why this matters enough to pin: the design animates three things (the CONNECTED
    /// dot, the agent ring, the footer dot), and D2 refused all three because the renderer
    /// holds a *tested* idle-CPU guarantee (G1) that a ~2Hz repaint would silently spend.
    /// "Silently" is the operative word — a timer added here breaks no test, changes no
    /// pixel a screenshot would catch, and shows up only as a laptop battery draining
    /// while the terminal sits untouched. Liveness is carried by colour instead (lime for
    /// a live session, grey for a stopped one), which costs nothing per frame because
    /// there are no frames.
    #[test]
    fn r7_4_the_loop_still_waits_and_the_panel_schedules_no_wake() {
        // 1. The loop waits, in exactly one place, and nothing swaps it later. A second
        //    `set_control_flow` anywhere is the whole failure mode: `Poll` spins and
        //    `WaitUntil` is a timer by another name.
        let main_rs = include_str!("main.rs");
        assert_eq!(
            main_rs.matches("set_control_flow").count(),
            1,
            "the control flow is set once, at startup — a second setter is where a \
             repaint cadence would enter"
        );
        assert!(
            main_rs.contains("set_control_flow(ControlFlow::Wait)"),
            "the loop must WAIT for events (docs/architecture.md C6)"
        );
        // `main.rs` is scanned whole — it carries no test module, and asserting that is
        // how this stays true if one is ever added.
        assert!(
            !main_rs.contains("mod tests"),
            "main.rs grew a test module; scan its production half instead"
        );
        for (name, production) in [
            ("main.rs", main_rs.to_string()),
            (
                "terminal_app.rs",
                production_half("terminal_app.rs", include_str!("terminal_app.rs")),
            ),
            (
                "overlay.rs",
                production_half("overlay.rs", include_str!("overlay.rs")),
            ),
        ] {
            for timer in ["ControlFlow::Poll", "ControlFlow::WaitUntil"] {
                assert!(
                    !production.contains(timer),
                    "`{timer}` in {name} — D2 makes the panel fully static; a timed \
                     control flow repaints the whole window, not just the band"
                );
            }
            // Only startup may touch the control flow, which is what makes the single
            // setter asserted above the whole story: a per-event setter would not show up
            // in a startup count at all.
            if name != "main.rs" {
                assert!(
                    !production.contains("set_control_flow"),
                    "{name} sets the control flow outside startup"
                );
            }
        }

        // 2. The panel's app-side module schedules nothing against a clock.
        //
        //    CORRECTED (ai-monitor-idle-timestamps D2/D16). This used to say the panel's
        //    "only two redraw triggers are `on_key` (a keystroke) and `poll` returning true
        //    (a watcher update that actually carried a new snapshot)". The parenthesis is now
        //    false: `poll` rebuilds the model on EVERY wake that reaches it while the panel is
        //    visible — including wakes that carried no snapshot at all, which is the whole of
        //    D2 — and returns whether the rebuilt model differs. A third trigger also exists:
        //    a one-shot deadline the panel arms so a quiet session's pill can stop reading
        //    CONNECTED. The redraw is now gated by the model rather than by the wake's origin.
        //
        //    None of that is a clock in THIS module, which is what the scan below asserts and
        //    why its ban list needs no edit. The deadline is held by `tabmeta::worker`, which
        //    is not scanned and already owns a `Duration` for its own tick; `overlay.rs`
        //    receives a plain `u64` instant from `dashboard::idle_deadline_ms` and passes it
        //    through an injected callback, so it names no `Duration`, no `Instant`, no sleep.
        //
        //    `now_ms` is not a cadence and is not scanned for: the model records *when*
        //    it was built, which is the opposite of a repaint — it is what let D9 drop the
        //    relative age that would have needed one. (D20 describes that timestamp as the
        //    footer's; the footer in fact formats `last_event_ms`, and it is the footer
        //    together with the IDLE tile that recovers the instant the frame was composed.)
        let overlay = production_half("overlay.rs", include_str!("overlay.rs"));
        for scheduler in [
            "Duration",
            "Instant",
            "thread::sleep",
            "recv_timeout",
            "set_timeout",
        ] {
            assert!(
                !overlay.contains(scheduler),
                "`{scheduler}` in overlay.rs — the panel repaints on events only (D2)"
            );
        }
    }

    /// The idle-CPU **frame rate** is pinned here, because nothing else pins it.
    ///
    /// Idle cost is `frames/sec × work/frame`. The renderer already guards the second
    /// factor — `monitor_panel.rs:992` asserts `layouts == 1` across idle frames and
    /// `:1451` asserts no rebuild — so an idle frame re-presents cached buffers and is
    /// cheap. **Nothing guarded the first factor**, and it is the multiplier: a visible
    /// window presents ~60fps forever because this arm asks for the next redraw, which
    /// measured at ~7% of a core in release and ~19% in debug (`docs/architecture.md` C6).
    ///
    /// Why this is a source scan and not a measurement. A real CPU assertion needs a GUI
    /// session, a window, and a release build, none of which `cargo test` has — the same
    /// constraint R7.4 reasons about above. It would also be machine-dependent: the rate
    /// tracks the display, so a 120 Hz panel doubles it, and any threshold would either
    /// flake on a loaded machine or be too loose to catch a regression. What IS stable is
    /// the *shape* that sets the rate, and these are the two edits that would change it:
    ///
    /// 1. **A second `request_redraw()` in this arm doubles the frame rate**, and so
    ///    doubles idle CPU, with every test still green and nothing visibly different.
    /// 2. **Losing the `!occluded` gate** makes a hidden window cost what a visible one
    ///    does. That gate is the only reason a backgrounded terminal is free, and its
    ///    absence is invisible precisely because you cannot see an occluded window.
    ///
    /// This does not pin 60fps itself, which belongs to the display, and it does not stop
    /// anyone changing the design deliberately. It makes the change loud.
    #[test]
    fn the_idle_frame_rate_stays_one_redraw_per_frame_and_stops_when_occluded() {
        let src = production_half("terminal_app.rs", include_str!("terminal_app.rs"));

        // The `RedrawRequested` arm, from its marker to the catch-all that closes the match.
        let arm = src
            .split_once("WindowEvent::RedrawRequested => {")
            .and_then(|(_, rest)| rest.split_once("\n            _ => {}"))
            .map(|(arm, _)| arm)
            .expect(
                "could not isolate the RedrawRequested arm — the match's shape changed, so \
                 this test is no longer reading what it claims to. Re-anchor it before \
                 trusting a pass.",
            );

        // Count CODE, not prose. This arm is heavily commented and the comments name the very
        // call being counted, so counting raw text makes the next explanatory line fail the
        // test with a message about doubled CPU — a false alarm that reads exactly like a real
        // one, on a test whose whole value is that its alarm means something.
        let code = arm
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            code.matches("request_redraw()").count(),
            1,
            "the RedrawRequested arm must request exactly ONE redraw. Two would double the \
             frame rate and double idle CPU (~7% -> ~14% of a core in release) with the \
             whole suite green and nothing on screen looking different."
        );

        assert!(
            code.contains("if !inner.occluded"),
            "the continuous redraw must stay gated on `!inner.occluded`. Without it a hidden \
             window keeps presenting ~60fps, which is invisible by construction — you cannot \
             see the window that is costing you the power."
        );

        // The gate is only meaningful if something still sets the flag.
        assert!(
            src.contains("WindowEvent::Occluded(occluded)") && src.contains("inner.occluded ="),
            "`occluded` must still be written from the Occluded event, or the gate above is \
             dead code that always reads false"
        );
    }

    /// Both re-point sites must forward the agent state to the overlays.
    ///
    /// This is the wiring D18's Busy suppression runs on, and it is wiring rather than logic:
    /// the decision it feeds (`Tabs::active_agent_state`) is mutation-tested on its own, and
    /// so is the seam it feeds into (`OverlayHost::notify_active_changed`). What is untested
    /// between them is the two lines that connect them, in two functions that no test can
    /// execute — `Running` owns an `Arc<Window>` and a `Renderer`, so constructing one needs
    /// a window and a GPU, which is the same wall `wiring.rs` documents.
    ///
    /// **Added because mutation testing found the hole, not because the code looked thin.**
    /// `replace Running::feed_agent_states -> bool with false`, its `true` twin, and
    /// `replace Running::after_roster_change with ()` all survived the full suite. A no-op
    /// `after_roster_change` means switching tabs re-points nothing; a `feed_agent_states`
    /// that returns a constant means the panel never hears that an agent went busy, and the
    /// gate that stops a five-minute build being reported as IDLE never sees its input. Both
    /// are silent: the pill keeps showing a plausible value, just not a true one.
    ///
    /// A source scan reaches what execution cannot because `cargo mutants` rewrites the file
    /// this reads through `include_str!`, so a body replaced by a constant no longer contains
    /// the call and this fails. That also bounds the claim honestly — it pins that the wiring
    /// is present, not that it runs.
    #[test]
    fn both_re_point_sites_forward_the_agent_state() {
        let src = production_half("terminal_app.rs", include_str!("terminal_app.rs"));

        for (name, sig) in [
            (
                "feed_agent_states",
                "fn feed_agent_states(&mut self) -> bool {",
            ),
            ("after_roster_change", "fn after_roster_change(&mut self) {"),
        ] {
            let body = src
                .split_once(sig)
                .and_then(|(_, rest)| rest.split_once("\n    }"))
                .map(|(body, _)| body)
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: could not isolate the body — its signature changed, so this \
                         test is no longer reading what it claims to. Re-anchor it before \
                         trusting a pass."
                    )
                });

            assert!(
                body.contains("active_agent_state"),
                "{name} must read the active tab's agent state. Without it the panel cannot \
                 tell a mid-turn agent from a quiet one, and D18's Busy gate — the thing that \
                 stops a long build being reported IDLE — has no input."
            );
            assert!(
                body.contains("notify_active_changed"),
                "{name} must re-point the overlays. Losing this call leaves the panel on the \
                 previous tab's session with nothing to correct it."
            );
            assert!(
                body.contains("agent_state"),
                "{name} must PASS the state on. Reading it and dropping it type-checks and \
                 leaves the gate reading `None` forever, which fails open — every pill goes \
                 on flipping to IDLE mid-turn."
            );
        }
    }

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

    #[test]
    fn a_command_state_change_resyncs_and_repaints_even_in_the_background() {
        // (advanced, is_active, title_changed, card_changed) → (sync rows, redraw).
        let cases = [
            ((false, false, false, false), (false, false)),
            ((true, false, false, false), (false, false)),
            ((false, true, false, false), (false, false)),
            ((true, true, false, false), (false, true)),
            ((false, false, true, false), (true, true)),
            ((false, false, false, true), (true, true)),
            ((true, false, false, true), (true, true)),
        ];
        for ((adv, active, title, card), want) in cases {
            assert_eq!(
                drain_effects(adv, active, title, card),
                want,
                "advanced={adv} active={active} title={title} card={card}"
            );
        }
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

    // ---- terminal-font-size: R0.1, R4.1–R4.4, R5.1–R5.3 ----

    /// **R4.1 / R4.2 / R4.3 / R4.4 — target_font_size steps and boundary clamping.**
    #[test]
    fn target_font_size_steps_resets_and_clamps_at_bounds() {
        // R4.1: Increase from 14.0 -> 15.0
        assert_eq!(target_font_size(14.0, FontAction::Increase), 15.0);
        // R4.2: Decrease from 15.0 -> 14.0
        assert_eq!(target_font_size(15.0, FontAction::Decrease), 14.0);
        // R4.3: Reset from any modified size -> FONT_SIZE_DEFAULT (14.0)
        assert_eq!(target_font_size(20.0, FontAction::Reset), FONT_SIZE_DEFAULT);
        assert_eq!(target_font_size(9.0, FontAction::Reset), FONT_SIZE_DEFAULT);
        // R4.4: Boundary no-op values match current_pt at min (8.0), max (32.0), and default (14.0)
        assert_eq!(
            target_font_size(FONT_SIZE_MIN, FontAction::Decrease),
            FONT_SIZE_MIN
        );
        assert_eq!(
            target_font_size(FONT_SIZE_MAX, FontAction::Increase),
            FONT_SIZE_MAX
        );
        assert_eq!(
            target_font_size(FONT_SIZE_DEFAULT, FontAction::Reset),
            FONT_SIZE_DEFAULT
        );
    }

    /// **R0.1 / R4.1–R4.4 — a font chord refits every tab, and a boundary no-op refits none.**
    ///
    /// Drives [`apply_font_action`], the code `do_font_action` runs, not a copy of it. The
    /// spy records every `resize_all`, because the no-op half is about a call that must
    /// **not** happen: `refit` is what clears a selection, so one stray call at a bound is
    /// the regression. Comparing the tabs' sizes alone could not see it, since a refit to
    /// the unchanged size leaves them exactly as they were.
    #[test]
    fn window_global_font_refit_updates_all_open_tabs_and_boundary_noop_preserves_selection() {
        struct MultiTabTerminals {
            terms: Vec<term_core::Terminal>,
            refits: Vec<(u16, u16)>,
        }
        impl crate::settings::RefitTarget for MultiTabTerminals {
            fn resize_all(&mut self, cols: u16, rows: u16) {
                self.refits.push((cols, rows));
                for t in &mut self.terms {
                    t.resize(cols, rows);
                }
            }
        }

        let cfg = Config::default();
        let Ok(mut renderer) = Renderer::new_headless(1200, 800, 1.0, &cfg) else {
            eprintln!("skipping window_global_font_refit_updates_all_open_tabs_and_boundary_noop_preserves_selection: no GPU/font");
            return;
        };
        let (init_cols, init_rows) = renderer.grid_size();
        let mut tabs = MultiTabTerminals {
            terms: vec![
                scroll_term(init_cols, init_rows),
                scroll_term(init_cols, init_rows),
            ],
            refits: Vec::new(),
        };
        tabs.terms[1].set_selection(
            term_core::Point { line: 0, col: 0 },
            term_core::Point { line: 0, col: 5 },
        );

        // R4.4: Cmd+0 at the default is a no-op: no refit, so the selection survives.
        assert!(!apply_font_action(
            &mut renderer,
            &mut tabs,
            FontAction::Reset
        ));
        assert_eq!(tabs.refits, Vec::<(u16, u16)>::new());
        assert!(tabs.terms[1].selection().is_some());

        // R0.1 / R4.1: Cmd+= changes the grid and refits ALL tabs to it, once.
        assert!(apply_font_action(
            &mut renderer,
            &mut tabs,
            FontAction::Increase
        ));
        let after = renderer.grid_size();
        assert_ne!(after, (init_cols, init_rows), "15pt must change the grid");
        assert_eq!(tabs.refits, vec![after]);
        for t in &tabs.terms {
            assert_eq!((t.snapshot().cols, t.snapshot().rows), after);
        }

        // R4.4 at the top bound: once at FONT_SIZE_MAX, Cmd+= refits nothing.
        assert!(renderer.set_font_size(FONT_SIZE_MAX));
        tabs.refits.clear();
        assert!(!apply_font_action(
            &mut renderer,
            &mut tabs,
            FontAction::Increase
        ));
        assert_eq!(tabs.refits, Vec::<(u16, u16)>::new());

        // Cmd+- steps back down by one point.
        assert!(apply_font_action(
            &mut renderer,
            &mut tabs,
            FontAction::Decrease
        ));
        assert_eq!(renderer.font_size_logical(), FONT_SIZE_MAX - FONT_SIZE_STEP);
    }

    /// **R4.1–R4.3 / D11 — modal `RenameOverlay` consumes `Cmd+=`, `Cmd+-`, `Cmd+0` before `font_action`.**
    #[test]
    fn modal_rename_overlay_consumes_font_chords_before_font_action_branch() {
        use winit::keyboard::{Key, NativeKeyCode, PhysicalKey};

        let mut overlays = OverlayHost::default();
        overlays.register(Box::new(crate::rename::RenameOverlay::new()));
        // Open the modal rename field on TabId(1).
        assert_eq!(
            overlays.command(&OverlayCommand::BeginRename {
                tab: TabId(1),
                current: Some("tab".to_string()),
            }),
            Handled::Consumed
        );

        for ch in ["=", "+", "-", "0"] {
            let input = KeyInput {
                key: Key::Character(ch.into()),
                physical: PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
                text: Some(ch.to_string()),
                mods: ModifiersState::SUPER,
            };
            let mut sink = Vec::new();
            let mut probe = WriteProbe::new(&mut sink);
            let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
            // In `Running::on_key`, `overlays.dispatch` runs BEFORE `font_action`:
            // `RenameOverlay` is modal at priority 200 and returns `Handled::Consumed`,
            // preventing `do_font_action` from firing while renaming.
            assert_eq!(
                overlays.dispatch(&input, &mut ctx),
                Handled::Consumed,
                "RenameOverlay must consume Cmd+{ch} before font_action runs"
            );
        }
    }

    /// **R5.1 / R5.2 / R5.3 — echo suppression, external human edits, and failed-save authority.**
    #[test]
    fn config_reloaded_reconciles_font_override_across_echo_human_edit_and_failed_save() {
        let last_applied_pt = 14.0f32;

        // R5.1: Successful save records 15.0 as the last write with `authoritative = false`.
        // When watcher delivers 15.0, it is classified as an echo and does NOT clear override.
        let current_pt = 15.0f32;
        assert!(input::needs_persist(current_pt, last_applied_pt, false));
        let last_written = Some(current_pt);
        assert!(!should_clear_font_override(
            15.0,
            last_applied_pt,
            last_written,
            false
        ));

        // R5.2: External human edit to `size_pt = 18.0` (not the last write) clears override.
        assert!(should_clear_font_override(18.0, 15.0, last_written, false));
        // ...and so does any edit when nothing has been written yet this session.
        assert!(should_clear_font_override(18.0, 14.0, None, false));

        // R5.3: Failed save sets `font_override_authoritative = true` while on-disk config is
        // still 14.0 (`last_applied_pt = 14.0`).
        // 1) An unrelated reload that still delivers `size_pt = 14.0` preserves the override:
        assert!(!should_clear_font_override(14.0, 14.0, None, true));
        // 2) A human edit to `[font].size_pt = 20.0` (delivered != last_applied_pt) clears it:
        assert!(should_clear_font_override(20.0, 14.0, None, true));
    }

    /// **R5.2 — a hand edit to a size this session already wrote still wins.**
    ///
    /// Font sizes are whole-point steps, so they recur: zoom 14→15→16→15→14 and the
    /// session has written 16 before. A hand edit to `size_pt = 16` must still read as a
    /// human edit. With a ring of recent writes (the sidebar's scheme) it read as an echo:
    /// the window stayed at 14 while the file said 16, until restart.
    #[test]
    fn a_hand_edit_to_a_previously_written_font_size_still_clears_the_override() {
        // After writing 15, 16, 15, 14 in turn, only the last write is remembered.
        let last_written = Some(14.0_f32);
        assert!(should_clear_font_override(16.0, 14.0, last_written, false));
        assert!(should_clear_font_override(15.0, 14.0, last_written, false));
        // The latest write coming back is still an echo.
        assert!(!should_clear_font_override(14.0, 14.0, last_written, false));
    }
}
