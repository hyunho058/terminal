//! ai-monitor overlay (D4): a Cmd+Shift+M toggle overlay showing the active
//! session's Claude JSONL events, registered through the SD4 `Overlay` seam and
//! drawn via the renderer's `overlay_layer` (SD3). Read-only (C3).
//!
//! Visibility-gated watcher lifecycle (D6): opening resolves the active tab's log
//! (from `cwd`), backfills the recent tail, and starts a tail watcher; hiding
//! drops the watcher. Switching tabs while open re-points (D5/R3.1).

use std::path::PathBuf;

use ai_monitor::{backfill, resolve_log, AiEvent, LogWatcher};
use renderer::overlay_layer::{OverlayHandle, OverlayLayer, OverlayModel, Placement};
use renderer::DrawLayer;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::Key;

use crate::seam::{AppCtx, Handled, KeyInput, Overlay};
use crate::terminal_app::UserEvent;

/// How many recent events to keep/show.
const MAX_EVENTS: usize = 200;

pub struct AiMonitorOverlay {
    visible: bool,
    handle: OverlayHandle,
    events: Vec<AiEvent>,
    cwd: Option<PathBuf>,
    watcher: Option<LogWatcher>,
    proxy: EventLoopProxy<UserEvent>,
}

impl AiMonitorOverlay {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        AiMonitorOverlay {
            visible: false,
            handle: OverlayHandle::default(),
            events: Vec::new(),
            cwd: None,
            watcher: None,
            proxy,
        }
    }

    /// Is this the Cmd+Shift+M toggle chord?
    fn is_toggle(input: &KeyInput) -> bool {
        if !(input.mods.super_key() && input.mods.shift_key()) {
            return false;
        }
        matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("m"))
    }

    /// Open on `cwd`: resolve the log, backfill, and start tailing (D1/D6).
    fn open(&mut self, cwd: Option<PathBuf>) {
        self.cwd = cwd.clone();
        self.events.clear();
        self.watcher = None;

        if let Some(log) = cwd.as_deref().and_then(resolve_log) {
            let (events, offset) = backfill(&log, MAX_EVENTS);
            self.events = events;
            let proxy = self.proxy.clone();
            self.watcher = LogWatcher::spawn(log, offset, move || {
                let _ = proxy.send_event(UserEvent::OverlayPending);
            });
        }
        self.visible = true;
        self.refresh_model();
    }

    /// Hide + stop the watcher (D6).
    fn close(&mut self) {
        self.visible = false;
        self.watcher = None;
        self.refresh_model();
    }

    /// Push the current visibility + rendered lines into the draw layer.
    fn refresh_model(&self) {
        let lines = self
            .events
            .iter()
            .map(|e| format!("{}: {}", e.role, e.summary))
            .collect();
        self.handle.set(OverlayModel {
            visible: self.visible,
            lines,
            // The log pane keeps its bottom-panel placement (sticky-tab-title only added
            // the choice; it did not change this overlay).
            placement: Placement::default(),
        });
    }
}

impl Overlay for AiMonitorOverlay {
    fn priority(&self) -> i32 {
        100
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if Self::is_toggle(input) {
            if self.visible {
                self.close();
            } else {
                self.open(ctx.active_cwd.clone());
            }
            ctx.request_redraw();
            return Handled::Consumed; // R2.1 — never reaches the shell
        }
        // A monitor doesn't capture other keys.
        Handled::Passthrough
    }

    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(OverlayLayer::from_handle(self.handle.clone()))
    }

    fn poll(&mut self) -> bool {
        let Some(watcher) = self.watcher.as_ref() else {
            return false;
        };
        let mut changed = false;
        while let Ok(ev) = watcher.rx.try_recv() {
            self.events.push(ev);
            changed = true;
        }
        if changed {
            if self.events.len() > MAX_EVENTS {
                let drop = self.events.len() - MAX_EVENTS;
                self.events.drain(0..drop);
            }
            self.refresh_model();
        }
        changed
    }

    fn on_active_changed(&mut self, cwd: Option<PathBuf>) {
        // Re-point only while open, and only if the session actually changed (R3.1).
        if self.visible && cwd != self.cwd {
            self.open(cwd);
        }
    }
}
