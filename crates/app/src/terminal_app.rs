//! The winit `ApplicationHandler` — the tabbed vertical slice (T3).
//!
//! Owns the event loop and wires everything: N independent [`Session`]s as tabs,
//! a registered tab-bar [`DrawLayer`] painting the reserved top inset, tab chords
//! (D5), and the SD4 registration seam ([`InputRouter`]/[`OverlayHost`]) that
//! later features extend without editing this file.
//!
//! Input dispatch order per key (R3.3/R4): visible overlays → registered input
//! handlers → tab chords → default `encode_key` to the active shell.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use common::config::Config;
use common::watch::ConfigWatcher;
use renderer::cursor_layer::CursorLayer;
use renderer::tab_bar::{self, TabBar, TabBarModel, TabModelHandle};
use renderer::Renderer;
use term_core::Color;
use term_core::GridSnapshot;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::input::{encode_key, tab_action, tab_target, TabAction};
use crate::seam::{AppCtx, Handled, InputHandler, InputRouter, KeyInput, Overlay, OverlayHost};
use crate::session::{Session, TabId};
use crate::tabs::Tabs;

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
    router: InputRouter,
    overlays: OverlayHost,
    mods: ModifiersState,
    /// Latest cursor position in physical px (raw winit `f64`), updated on
    /// `CursorMoved` and read on a left-button press to hit-test the tab sidebar.
    last_cursor: (f64, f64),
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

        let size = window.inner_size();
        let scale = window.scale_factor();
        let mut renderer = Renderer::new(window.clone(), size.width, size.height, scale, &self.cfg)
            .map_err(err)?;

        // Reserve the vertical tab-sidebar band on the left, then register the
        // layer (SD3/R2). The grid narrows by this width (top inset stays 0).
        let bar_w = tab_bar::width_px(renderer.cell_size().0);
        renderer.set_left_inset(bar_w);
        let (bar, tab_model) = TabBar::new();
        renderer.add_layer(Box::new(bar));
        // Blinking block cursor, above the glyphs (a DrawLayer so it blinks off the
        // continuous-redraw loop, unaffected by the idle-CPU snapshot cache).
        renderer.add_layer(Box::new(CursorLayer::new()));

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
            router,
            overlays,
            mods: ModifiersState::empty(),
            last_cursor: (0.0, 0.0),
            config_rx,
            _config_watcher: config_watcher,
            occluded: false,
            last_snapshot: None,
            snapshot_dirty: true,
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

    /// Push the current tab titles + active index into the tab-bar layer (D6).
    fn sync_tab_model(&mut self) {
        self.tab_model.set(TabBarModel {
            titles: self.tabs.titles(),
            active: self.tabs.active_index(),
        });
    }

    /// Handle a tab chord (R3.2). Returns false if the app should exit (last tab
    /// closed).
    fn do_tab_action(&mut self, action: TabAction) -> bool {
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
        self.sync_tab_model();
        // Re-point cwd-derived overlays (ai-monitor) at the new active tab (R3.1).
        let cwd = self.tabs.active_cwd();
        self.overlays.notify_active_changed(cwd);
        self.redraw();
        true
    }

    /// Dispatch a key through the seam, then default input (R3.3/R4). Returns
    /// false if the app should exit.
    fn on_key(&mut self, input: KeyInput) -> bool {
        // 1) Visible overlays first (R4.1/R4.2).
        {
            let active = self.tabs.active_index();
            let count = self.tabs.len();
            let cwd = self.tabs.active_cwd();
            let mut ctx = AppCtx::new(&mut self.tabs.active_mut().writer, count, active, cwd);
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
            let mut ctx = AppCtx::new(&mut self.tabs.active_mut().writer, count, active, cwd);
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
                let advanced = inner.tabs.by_id_mut(id).map(|s| s.drain()).unwrap_or(false);
                if advanced && is_active {
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
                    crate::settings::apply(&cfg, &mut inner.renderer, &mut inner.tabs);
                    inner.redraw();
                }
            }
            UserEvent::OverlayPending => {
                // An overlay (ai-monitor) tailed new data — fold it in + redraw (R1.3).
                if inner.overlays.poll_all() {
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
                // Track the raw physical cursor; the f64→f32 cast happens in
                // `tab_target` at press time (R2.1).
                inner.last_cursor = (position.x, position.y);
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // A left-click inside the left sidebar switches to that tab (R2).
                // The hit-test geometry lives in tab_bar (SD2); a press outside the
                // sidebar (or below the last tab) returns None → no-op. On a hit,
                // reuse the exact chord-switch path via `do_tab_action(Jump)`.
                if let Some(i) = tab_target(
                    inner.last_cursor,
                    inner.renderer.cell_size(),
                    inner.renderer.left_inset(),
                    inner.tabs.len(),
                ) {
                    inner.do_tab_action(TabAction::Jump(i));
                }
            }

            WindowEvent::Resized(size) => {
                inner.renderer.resize(size.width, size.height);
                let (cols, rows) = inner.renderer.grid_size();
                inner.tabs.resize_all(cols, rows); // background tabs too (R5.1)
                inner.redraw();
            }

            WindowEvent::Occluded(occluded) => {
                // Pause the continuous redraw loop while hidden; resume on reveal.
                inner.occluded = occluded;
                if !occluded {
                    inner.redraw();
                }
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } if key_event.state == ElementState::Pressed => {
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
