//! Crewterm binary entry point.
//!
//! Deliberately thin. Everything else lives in the `app` library beside it, so it
//! is importable by `crates/app/tests/`; what stays here is only what a library
//! cannot own — the winit `EventLoop`, which on macOS may be built only on the
//! process's main thread.
//!
//! Threading model (D7): the PTY reader runs on its own thread and hands bytes
//! to the UI thread over a channel; an `EventLoopProxy` wakes the event loop so
//! the render happens on the winit thread (C6).
//!
//! That parenthesis used to read "C6 — no free-running loop", which is true of the
//! control flow set below and false of the frames: while the window is visible the
//! `RedrawRequested` arm requests the next redraw, so the app presents ~60fps forever
//! and costs ~7% of a core in release at idle. It is deliberate (winit#2640 would
//! otherwise blank the window, and atlas eviction relies on it) and it is described in
//! `docs/architecture.md` C6. `ControlFlow::Wait` still means no *spin*; it does not
//! mean no frames.

use winit::event_loop::{ControlFlow, EventLoop};

use app::{feature_registrations, App, SidebarMetaSource, TabMetaHandle, UserEvent};

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

    if let Some(mtm) = objc2::MainThreadMarker::new() {
        app::install_app_menu(mtm);
    }

    let proxy = event_loop.create_proxy();
    // Capture the sidebar display toggles before `cfg` is moved into the app (T5/D12).
    let (show_git_branch, show_agent_status) =
        (cfg.sidebar.show_git_branch, cfg.sidebar.show_agent_status);
    // tab-metadata: an off-thread worker derives each tab's git branch + AI-agent
    // status and wakes the loop via OverlayPending. Started *before* the app,
    // because only the process-global half belongs in this file.
    //
    // What stays here is what a library genuinely cannot own: `install_tabmeta`
    // and `TabModelHandle::shared` install singletons, so a test that ran this
    // would leak them into every other test in the binary. What does *not* stay
    // here is the pump's registration — that is an ordinary decision, it now
    // travels in the value below, and `crates/app/tests/wiring.rs` asserts it.
    let meta = TabMetaHandle::spawn({
        let proxy = proxy.clone();
        move || {
            let _ = proxy.send_event(UserEvent::OverlayPending);
        }
    });
    if let Some(handle) = &meta {
        app::install_tabmeta(handle.clone());
        // Push the display toggles to the worker (D12 of tab-metadata), and install the
        // metadata pull adapter into the shared sidebar model (D10) so the sidebar shows
        // each tab's git branch + agent state.
        handle.set_enabled(show_git_branch, show_agent_status);
        renderer::sidebar_row::TabModelHandle::shared()
            .set_meta_source(std::rc::Rc::new(SidebarMetaSource::new(handle.clone())));
    }

    // Feature extensions arrive through the SD4 seam (no terminal_app edits), as
    // one value assembled in the library. `App::new` is the only way in, so there
    // is no second registration path for a feature to go missing down: nothing
    // about which extensions the app has is decided in this file.
    let mut app = App::new(cfg, proxy.clone(), feature_registrations(Some(proxy), meta));

    if let Err(e) = event_loop.run_app(&mut app) {
        tracing::error!("event loop exited with error: {e}");
        std::process::exit(1);
    }
}
