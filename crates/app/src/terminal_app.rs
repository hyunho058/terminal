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
use renderer::tab_bar::{self, TabBar, TabBarModel, TabModelHandle};
use renderer::Renderer;
use term_core::Color;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::input::{encode_key, tab_action, TabAction};
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
    /// Fresh configs delivered by the background watcher (settings feature).
    config_rx: Receiver<Config>,
    /// Kept alive so the watch thread keeps running; dropping it stops watching.
    _config_watcher: Option<ConfigWatcher>,
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

        // Reserve the tab-bar band up top, then register the layer (SD3/R2).
        let bar_h = tab_bar::height_px(renderer.cell_size().1);
        renderer.set_top_inset(bar_h);
        let (bar, tab_model) = TabBar::new();
        renderer.add_layer(Box::new(bar));

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
            config_rx,
            _config_watcher: config_watcher,
        };
        running.sync_tab_model();
        running.window.request_redraw();
        Ok(running)
    }
}

impl Running {
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
        self.window.request_redraw();
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
                    self.window.request_redraw();
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
                    self.window.request_redraw();
                }
                return true;
            }
        }
        // 3) Tab chords, intercepted before the shell (D5/R3.2).
        if let Some(action) = tab_action(&input.key, &input.mods) {
            return self.do_tab_action(action);
        }
        // 4) Default: encode to the active shell only (R3.1).
        if let Some(bytes) = encode_key(&input.key, input.text.as_deref()) {
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
                if let Some(session) = inner.tabs.by_id_mut(id) {
                    if session.drain() && is_active {
                        inner.window.request_redraw();
                    }
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
                    inner.window.request_redraw();
                }
            }
            UserEvent::OverlayPending => {
                // An overlay (ai-monitor) tailed new data — fold it in + redraw (R1.3).
                if inner.overlays.poll_all() {
                    inner.window.request_redraw();
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

            WindowEvent::Resized(size) => {
                inner.renderer.resize(size.width, size.height);
                let (cols, rows) = inner.renderer.grid_size();
                inner.tabs.resize_all(cols, rows); // background tabs too (R5.1)
                inner.window.request_redraw();
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
                let snapshot = inner.tabs.active().terminal.snapshot();
                if let Err(e) = inner.renderer.render(&snapshot) {
                    tracing::warn!("render failed: {e}");
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
