//! mac-terminal binary entry point.
//!
//! Wires the three crates together and runs the winit event loop:
//!   `common` (config + logging) · `term-core` (PTY + grid) · `renderer` (wgpu).
//!
//! Threading model (D7): the PTY reader runs on its own thread and hands bytes
//! to the UI thread over a channel; an `EventLoopProxy` wakes the event loop so
//! the render happens on the winit thread (C6 — no free-running loop).

mod input;
mod overlay;
mod seam;
mod session;
mod settings;
mod tabs;
mod terminal_app;

use winit::event_loop::{ControlFlow, EventLoop};

use overlay::AiMonitorOverlay;
use terminal_app::{App, UserEvent};

fn main() {
    common::logging::init();

    let cfg = common::config::Config::load().unwrap_or_else(|e| {
        tracing::warn!("config load failed ({e}); using defaults");
        common::config::Config::default()
    });

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("build event loop");
    // Wait for events (PTY wakeups, input, redraw) rather than spinning.
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::new(cfg, proxy.clone());
    // Register feature extensions through the SD4 seam (no terminal_app edits):
    // ai-monitor's Cmd+Shift+M overlay.
    app.register_overlay(Box::new(AiMonitorOverlay::new(proxy)));

    if let Err(e) = event_loop.run_app(&mut app) {
        tracing::error!("event loop exited with error: {e}");
        std::process::exit(1);
    }
}
