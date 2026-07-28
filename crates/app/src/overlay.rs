//! ai-monitor overlay (D4): a Cmd+Shift+M toggle overlay showing the active
//! session's folded Claude JSONL telemetry, registered through the SD4 `Overlay` seam and
//! drawn via the renderer's `overlay_layer` (SD3). Read-only (C3).
//!
//! Visibility-gated watcher lifecycle (D6): opening resolves the active tab's log
//! (from `cwd`), folds the existing file into a `SessionTelemetry`, and hands that
//! accumulator to a tail watcher which sends back `Arc<TelemetrySnapshot>`s; hiding
//! drops the watcher. Switching tabs while open re-points (D5/R3.1).

use std::path::PathBuf;

use std::sync::Arc;

use ai_monitor::telemetry::{SessionTelemetry, TelemetrySnapshot};
use ai_monitor::{backfill_telemetry, newest_jsonl, project_dir, LogWatcher};
use renderer::overlay_layer::{OverlayHandle, OverlayLayer, OverlayModel};
use renderer::DrawLayer;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::Key;

use crate::dashboard;
use crate::seam::{AppCtx, Handled, KeyInput, Overlay};
use crate::terminal_app::UserEvent;

pub struct AiMonitorOverlay {
    visible: bool,
    handle: OverlayHandle,
    /// The newest snapshot the watcher folded. `None` means exactly one thing: no
    /// log was resolved for this tab. Because `open()` seeds it from the backfill
    /// before spawning, it is `Some` immediately after a successful open — never
    /// "opened but nothing folded yet".
    latest: Option<Arc<TelemetrySnapshot>>,
    cwd: Option<PathBuf>,
    watcher: Option<LogWatcher>,
    proxy: EventLoopProxy<UserEvent>,
    /// This window's start time, and the floor the dashboard applies: a log whose
    /// last event precedes it stopped being written before the window existed, so it
    /// belongs to some other session. Captured here rather than at open, because
    /// open happens *after* the tab's own session may have started writing.
    started_ms: u64,
    /// `[ai_monitor] context_window`, re-read on each open.
    context_window: Option<u64>,
}

impl AiMonitorOverlay {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        AiMonitorOverlay {
            visible: false,
            handle: OverlayHandle::default(),
            latest: None,
            cwd: None,
            watcher: None,
            proxy,
            started_ms: now_ms(),
            context_window: None,
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
        // Re-read on open rather than holding a live handle: `settings::apply` does
        // not reach the overlays, and a value that takes effect the next time the
        // panel is opened is honest about that. A malformed config keeps the table.
        self.context_window = common::config::Config::load()
            .ok()
            .and_then(|c| c.ai_monitor.context_window);
        self.cwd = cwd.clone();
        self.latest = None;
        self.watcher = None;

        if let Some(dir) = cwd.as_deref().and_then(project_dir) {
            // Resolve to the newest log *here* rather than latching one for the
            // watcher's lifetime: it may not be this tab's session, and the tab's own
            // session may not have written a file yet. The watcher re-resolves.
            let log = newest_jsonl(&dir);
            // Seed on THIS thread, exactly where `backfill` ran before: publish the
            // seeded snapshot first, then hand the accumulator to the watch thread.
            let (telemetry, offset) = match log.as_deref() {
                Some(p) => backfill_telemetry(p),
                None => (SessionTelemetry::new(), 0),
            };
            self.latest = Some(Arc::new(telemetry.snapshot()));
            let proxy = self.proxy.clone();
            // The same floor the dashboard applies, so the watcher does not latch
            // onto a log the display layer will then refuse to show.
            self.watcher =
                LogWatcher::spawn(dir, log, telemetry, offset, self.started_ms, move || {
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

    /// Push the current visibility + composed lines into the draw layer.
    ///
    /// All layout lives in [`crate::dashboard`] (F5) — this is wiring: supply the
    /// clock, hand over the snapshot, publish the result.
    fn refresh_model(&self) {
        let env = dashboard::Env {
            now_ms: now_ms(),
            floor_ms: self.started_ms,
            context_window: self.context_window,
        };
        let lines = dashboard::compose(self.latest.as_deref(), &env);
        self.handle.set(OverlayModel {
            visible: self.visible,
            lines,
        });
    }
}

/// Wall-clock millis since the epoch, saturating.
///
/// Both failure modes are impossible-in-practice but neither may `unwrap`: a clock
/// set before 1970 yields `0`, and a `u128` past `u64::MAX` millis (year 584942417)
/// clamps. Either way the dashboard reads a `now_ms` at or below `first_event_ms`,
/// which saturating subtraction renders as `0s` rather than a wrapped duration.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Drain a snapshot channel, keeping only the newest value.
///
/// Snapshots are CUMULATIVE, so every one but the last is already stale by the time
/// the UI thread wakes — rendering each would cost N model writes and N redraws for
/// one visible state. Split out of `poll` so the coalescing is unit-testable:
/// `AiMonitorOverlay` needs an `EventLoopProxy`, which needs a real winit
/// `EventLoop` (main-thread-only, once per process on macOS) and so cannot be
/// constructed under `cargo test`.
fn drain_to_newest<T>(rx: &std::sync::mpsc::Receiver<T>) -> Option<T> {
    let mut newest = None;
    while let Ok(v) = rx.try_recv() {
        newest = Some(v);
    }
    newest
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
        let newest = drain_to_newest(&watcher.rx);
        let changed = newest.is_some();
        if let Some(snap) = newest {
            self.latest = Some(snap);
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

#[cfg(test)]
mod tests {
    use super::drain_to_newest;
    use std::sync::mpsc;

    #[test]
    fn poll_coalesces_a_burst_to_the_newest_snapshot() {
        // R4.1 — three batches queued while the UI thread was busy must produce ONE
        // model write, from the last. Uses a plain std channel: no `insta`, so no
        // dev-dependency and `crates/app/Cargo.toml` stays out of this surface.
        let (tx, rx) = mpsc::channel::<u32>();
        for v in [1, 2, 3] {
            tx.send(v).unwrap();
        }
        assert_eq!(
            drain_to_newest(&rx),
            Some(3),
            "newest wins, earlier are stale"
        );
        assert_eq!(drain_to_newest(&rx), None, "channel fully drained");
    }

    #[test]
    fn poll_reports_no_change_on_an_empty_channel() {
        // R4.2 — nothing queued must not rewrite the model or request a redraw.
        let (_tx, rx) = mpsc::channel::<u32>();
        assert_eq!(drain_to_newest(&rx), None);
    }
}
