//! mac-terminal binary entry point.
//!
//! Wires the three crates together and runs the winit event loop:
//!   `common` (config + logging) · `term-core` (PTY + grid) · `renderer` (wgpu).
//!
//! Threading model (D7): the PTY reader runs on its own thread and hands bytes
//! to the UI thread over a channel; an `EventLoopProxy` wakes the event loop so
//! the render happens on the winit thread (C6 — no free-running loop).

mod card_menu;
mod clipboard;
mod copy;
mod dashboard;
mod input;
mod overlay;
mod prompt;
mod rename;
mod seam;
mod session;
mod settings;
mod sidebar_model;
mod tabmeta;
mod tabs;
mod tabsession;
mod terminal_app;
mod waiting;

use winit::event_loop::{ControlFlow, EventLoop};

use clipboard::{ClipboardHandler, ImagePresenter};
use copy::{CopyHandler, SelectionPresenter};
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
    // Capture the sidebar display toggles before `cfg` is moved into the app (T5/D12).
    let (show_git_branch, show_agent_status) =
        (cfg.sidebar.show_git_branch, cfg.sidebar.show_agent_status);
    let mut app = App::new(cfg, proxy.clone());
    // Register feature extensions through the SD4 seam (no terminal_app edits):
    // ai-monitor's Cmd+Shift+M overlay, image-paste's Cmd+V handler + inline-image
    // presenter (its DrawLayer draws snapshot images).
    // sticky-tab-title: the Cmd+Shift+R rename field. Registered at priority 200, ahead
    // of the ai-monitor's 100, so an open field owns the keyboard (D8a.1).
    app.register_overlay(Box::new(rename::RenameOverlay::new()));
    app.register_overlay(Box::new(AiMonitorOverlay::new(proxy.clone())));
    app.register_overlay(Box::new(ImagePresenter));
    app.register_handler(Box::new(ClipboardHandler::new()));
    // copy-clipboard: Cmd+C copies the grid selection to the pasteboard, and the
    // selection-highlight DrawLayer is wired in via SelectionPresenter (the same
    // Overlay seam ImagePresenter uses).
    app.register_overlay(Box::new(SelectionPresenter));
    app.register_handler(Box::new(CopyHandler::new()));

    // tab-metadata: an off-thread worker derives each tab's git branch + AI-agent
    // status and wakes the loop via OverlayPending; a headless pump turns that wake
    // into a redraw. Installed as a locator so `Tabs` can push the roster/active tab.
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
