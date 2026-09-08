//! mac-terminal binary entry point.
//!
//! Deliberately thin. Everything else lives in the `app` library beside it, so it
//! is importable by `crates/app/tests/`; what stays here is only what a library
//! cannot own — the winit `EventLoop`, which on macOS may be built only on the
//! process's main thread.
//!
//! Threading model (D7): the PTY reader runs on its own thread and hands bytes
//! to the UI thread over a channel; an `EventLoopProxy` wakes the event loop so
//! the render happens on the winit thread (C6 — no free-running loop).

use winit::event_loop::{ControlFlow, EventLoop};

use app::terminal_app::{App, UserEvent};
use app::{feature_registrations, sidebar_model, tabmeta};

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
    // Capture the sidebar display toggles before `cfg` is moved into the app (T5/D12).
    let (show_git_branch, show_agent_status) =
        (cfg.sidebar.show_git_branch, cfg.sidebar.show_agent_status);
    // Feature extensions arrive through the SD4 seam (no terminal_app edits). The
    // set *and its assembly* live in the library so a test can drive both; nothing
    // about which features the app has is decided in this file.
    let mut app = App::new(
        cfg,
        proxy.clone(),
        feature_registrations(Some(proxy.clone())),
    );

    // tab-metadata: an off-thread worker derives each tab's git branch + AI-agent
    // status and wakes the loop via OverlayPending; a headless pump turns that wake
    // into a redraw. Installed as a locator so `Tabs` can push the roster/active tab.
    //
    // Stays here rather than in `feature_overlays`: unlike the others this installs
    // *process-global* state (`tabmeta::install`, `TabModelHandle::shared`) and
    // spawns a worker, so a test that built it would leak that state into every
    // other test in the binary.
    if let Some(handle) = tabmeta::TabMetaHandle::spawn(move || {
        let _ = proxy.send_event(UserEvent::OverlayPending);
    }) {
        tabmeta::install(handle.clone());
        // Push the display toggles to the worker (D12 of tab-metadata), and install the
        // metadata pull adapter into the shared sidebar model (D10) so the sidebar shows
        // each tab's git branch + agent state.
        handle.set_enabled(show_git_branch, show_agent_status);
        renderer::sidebar_row::TabModelHandle::shared().set_meta_source(std::rc::Rc::new(
            sidebar_model::SidebarMetaSource::new(handle.clone()),
        ));
        app.register_overlay(Box::new(tabmeta::TabMetaPump::new(handle)));
    }

    if let Err(e) = event_loop.run_app(&mut app) {
        tracing::error!("event loop exited with error: {e}");
        std::process::exit(1);
    }
}
