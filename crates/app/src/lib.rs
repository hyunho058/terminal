//! mac-terminal application library.
//!
//! Everything the binary is made of lives here; `main.rs` is the thin shell that
//! owns the one thing a library cannot: the winit `EventLoop`, which on macOS may
//! only be built on the process's main thread.
//!
//! That constraint is the reason this crate is a library at all. A `[[bin]]`-only
//! crate has no importable surface, so `crates/app/tests/` could not exist and the
//! app's 900-odd unit tests could only ever reach the pure decision functions
//! their modules happen to expose — never the assembled path. The measured
//! consequence: making [`session::Session::write`] discard every byte, so the
//! terminal accepts no typing at all, left the whole workspace suite at 1460
//! passed / 0 failed. The integration tests next door close that; this split is
//! what lets them run.
//!
//! The wiring `main` performs is likewise extracted here ([`feature_overlays`],
//! [`feature_handlers`], [`feature_registrations`]) rather than written inline, so
//! the registration set is a value a test can inspect instead of a sequence of
//! statements only a running app can observe. `crates/app/tests/wiring.rs` drives
//! it through [`seam::Registrations::install`] into the live registries, which is
//! the step that actually decides whether a feature exists in the running app.

// Visibility here is load-bearing, and `pub` is not the safe default.
//
// rustc's dead-code analysis is reachability-based: an unused item inside a `pub
// mod` of a *library* is reachable from outside the crate, so it is not dead and
// nothing warns. This crate became a library for `crates/app/tests/`, and marking
// every module `pub` in the move silently switched that lint off for the whole
// crate — taking with it the ~20 `#[allow(dead_code)]` markers whose comments
// promise removal "once a consumer lands", which had nothing left to suppress.
// Measured: an unreferenced `pub fn` added to `tabs.rs` passed
// `cargo clippy -p app -- -D warnings` clean.
//
// Narrowing the *modules* was not enough, and the first attempt at this comment
// claimed it was. Reachability does not stop at the module: every `pub` item
// inside a `pub mod` of a library is still reachable, so the six modules left
// `pub` below — `input` alone carries 105 tests' worth of helpers — kept the whole
// crate's largest files exempt. Measured again after that change, with the same
// probe: an unreferenced `pub fn` added to `input.rs` still passed
// `cargo clippy -p app -- -D warnings` clean.
//
// So no module is `pub`. Each is `pub(crate)`, and the handful of items something
// outside the crate genuinely names are re-exported by name below. That list is
// the crate's real public surface, written down in one place, and everything not
// on it is back under the lint.
pub(crate) mod capture;
pub(crate) mod card_menu;
pub(crate) mod clipboard;
pub(crate) mod copy;
pub(crate) mod dashboard;
pub(crate) mod drag;
pub(crate) mod flow;
pub(crate) mod flow_telemetry;
pub(crate) mod hyperlink;
pub(crate) mod ime_chord;
pub(crate) mod input;
pub(crate) mod overlay;
// The two exceptions to the rule above, by design (`session-overview-ui` T4): the overview's
// policy is a set of pure functions whose acceptance tests in `crates/app/tests/` call them by
// module path (`app::overview::build_overview`, `app::overview_detail::build_detail`), so the
// whole module is the public surface rather than a name list.
pub mod overview;
pub mod overview_detail;
pub(crate) mod presence_fd;
pub(crate) mod prompt;
pub(crate) mod rename;
pub(crate) mod seam;
pub(crate) mod session;
pub(crate) mod settings;
pub(crate) mod sidebar_model;
pub(crate) mod tabmeta;
pub(crate) mod tabs;
pub(crate) mod tabsession;
pub(crate) mod terminal_app;
pub(crate) mod waiting;

// The crate's entire public surface. Each line names its one consumer, because
// that consumer is the only thing keeping the item out of the dead-code lint.
/// `crates/app/tests/vertical_slice.rs` drags a selection over a real shell's output.
pub use drag::{is_click as release_is_click, release as release_drag, SelectionDrag};
/// `crates/app/tests/tt1_graph_keys.rs`, `crates/app/tests/tt3_graph_monitor.rs` and the
/// `tt*_overview_overlay.rs` acceptance tests drive the Cmd+Shift+F overlay
/// (session-flow-graph R1, R3; session-overview-ui R2.5, R4.5, R5, R7, R8.5, R10).
pub use flow::FlowOverlay;
/// `crates/app/tests/tt7_flow_telemetry.rs` drives the session graph's subagent thread
/// (session-flow-viewer R7).
pub use flow_telemetry::{spawn as spawn_flow_telemetry, DigestSlot, FlowTelemetry, SessionDigest};
/// `crates/app/tests/vertical_slice.rs` types through the real encoder.
pub use input::encode_key;
/// `crates/app/tests/tt2_graph_pointer.rs` drives the pointer router (session-flow-graph
/// R2.1, R2.2).
pub use input::pointer_owner;
/// `crates/app/tests/tt2_graph_pointer.rs` names the router's answer (session-flow-graph
/// R2.1, R2.2).
pub use input::PointerOwner;
/// `crates/app/tests/tt11_graph_wheel.rs` wheels the open graph (session-flow-graph R11.3).
pub use input::WheelDelta;
/// The reported-drag pieces (mouse-drag-reporting), asserted by
/// `crates/app/tests/tt1_mouse_motion_encoding.rs` … `tt4_drag_bypass.rs`.
pub use input::{
    drag_owns_pointer, encode_mouse_motion_report, pixel_to_cell, pixel_to_cell_clamped,
    reported_drag_motion,
};
pub use input::{
    encode_mouse_button_report, grid_press_action, grid_release_action, mouse_button_bytes,
    mouse_button_route, should_snap_on_click, GridPressAction, GridReleaseAction, MouseButtonRoute,
};
/// `crates/app/tests/tt3_graph_monitor.rs` dispatches the monitor's chord beside a closed
/// graph (session-flow-graph R3.1, R3.2).
pub use overlay::AiMonitorOverlay;
/// The SD4 seam: named in the signatures below and asserted by
/// `crates/app/tests/wiring.rs`.
pub use overlay::{no_deadline, ArmDeadline};
/// session-overview-ui D9 — reachable from `tests/` for TT6.
pub use prompt::blocking_subject;
/// `crates/app/tests/tt3_graph_monitor.rs` / `crates/app/tests/tt9_graph_click.rs` build the
/// dispatch context and read the requests it queued (session-flow-graph R3.1, R9).
pub use seam::{AppCtx, AppRequest, Handled, KeyInput, OverlayCommand, OverlayHost};
pub use seam::{InputHandler, Overlay, Registrations};
/// `crates/app/tests/vertical_slice.rs` drives a real shell through a session.
pub use session::{Session, TabId, Wake};
/// `main.rs` installs the sidebar's metadata source.
pub use sidebar_model::SidebarMetaSource;
/// `crates/app/tests/tt6_graph_status.rs` compares the graph's session status against the
/// card's own join (session-flow-graph R6).
pub use sidebar_model::{resolve_status, row_meta};
/// `crates/app/tests/tt6_tmux_panes.rs` parses and joins tmux output (session-flow-viewer
/// R6.1-R6.4, R6.7).
pub use tabmeta::tmux::{
    panes_of, parse_clients, parse_panes, select_pane, ClientRow, PaneRow, MAX_PANES,
};
/// `crates/app/tests/tt7_graph_header.rs` stands in a published snapshot, readiness included
/// (session-flow-graph R7.2).
pub use tabmeta::TabMetaSnapshot;
/// `crates/app/tests/tt6_tmux_panes.rs` drives the graph pass gate and the published pane
/// shape (session-flow-viewer R6.6); the flow graph's tests name the metadata types.
pub use tabmeta::{graph_pass, Agent, AgentKind, AgentState, PaneMeta, PanePass, TabMeta};
/// `main.rs` spawns the process-global metadata worker and installs its handle.
pub use tabmeta::{install as install_tabmeta, TabMetaHandle};
/// `crates/app/tests/tt4_graph_repo.rs` drives the graph readiness publish step
/// (session-flow-graph R4.7).
pub use tabmeta::{publish_graph_state, GraphPublish};
/// `crates/app/tests/tt4_graph_repo.rs` resolves repository identity (session-flow-graph
/// R4.1-R4.4).
pub use tabmeta::{repo_of, RepoId};
/// `crates/app/tests/tt9_graph_click.rs` resolves a tab id to its strip index
/// (session-flow-graph R9).
pub use tabs::{tab_position, Tabs};
/// `main.rs` builds and runs the app.
pub use terminal_app::{App, UserEvent};

use winit::event_loop::EventLoopProxy;

use clipboard::{ClipboardHandler, ImagePresenter};
use copy::{CopyHandler, SelectionPresenter};

/// Everything `main` hands to [`terminal_app::App::new`], assembled.
///
/// The installation loops used to live inline in `main`, where no test could run
/// them: `main` builds a winit `EventLoop`, which macOS creates only on the
/// process's main thread while the harness runs every test on a spawned one. A
/// loop that dropped its items — or was deleted outright — would have removed
/// every feature from the running app with the whole suite still green.
///
/// As a function it is an ordinary value, and `crates/app/tests/wiring.rs` drives
/// it through [`Registrations::install`] to the live registries.
///
/// `tabmeta` is the metadata worker's handle, when the worker started. It arrives
/// as a **parameter** rather than being spawned here because starting the worker
/// installs process-global state (`tabmeta::install`,
/// `sidebar_row::TabModelHandle::shared`) that a test must not touch. Whether its
/// pump joins the overlay set, and at what priority, is not global — it is an
/// ordinary decision, and keeping it in `main` is what left the app with a fifth
/// overlay that no test could see: deleting that one line stopped the sidebar's
/// git branch and agent status from ever refreshing, with the suite green and
/// this file's own tests asserting a set of four.
///
/// `proxy` is `None` only in tests; see [`feature_overlays`].
pub fn feature_registrations(
    proxy: Option<EventLoopProxy<UserEvent>>,
    tabmeta: Option<tabmeta::TabMetaHandle>,
) -> Registrations {
    let mut set = Registrations::default();
    // The ai-monitor panel's route to the metadata worker (idle-timestamps D7): one verb,
    // captured here where both halves exist, rather than a handle threaded into the overlay or
    // a `tabmeta::global()` lookup inside it. The global was rejected for a concrete reason —
    // it answers `None` until `install`, which no test performs, so the whole arming path
    // would be unreachable under `cargo test`. With no worker there is nothing to arm, and the
    // panel simply never gets its wake.
    let arm: overlay::ArmDeadline = match tabmeta.clone() {
        Some(handle) => Box::new(move |at| handle.set_panel_deadline(at)),
        None => overlay::no_deadline(),
    };
    for overlay in feature_overlays(proxy, arm) {
        set.overlay(overlay);
    }
    // tab-metadata's pump, at `i32::MIN`: headless, invisible, consumes no key.
    // It exists to turn the worker's wake into a repaint, so losing it is silent
    // by construction — which is exactly why it belongs in the asserted set.
    if let Some(handle) = tabmeta {
        set.overlay(Box::new(tabmeta::TabMetaPump::new(handle)));
    }
    for handler in feature_handlers() {
        set.handler(handler);
    }
    set
}

/// The feature extensions the app registers through the SD4 seam, in
/// registration order.
///
/// Extracted from `main` so the set is a **value**. Registration is otherwise
/// unobservable: an overlay that is never registered, or registered at a priority
/// that puts it behind another, produces no error anywhere — the feature simply
/// does nothing, and every unit test of that feature's own logic keeps passing.
/// The relative priorities are load-bearing (D8a.1: the rename field at 200 sits
/// ahead of the ai-monitor at 100 so an open field owns the keyboard), and a
/// value can be asserted about where a statement sequence cannot.
///
/// `proxy` is `None` only in tests. Production always passes `Some`: the
/// ai-monitor's log watcher wakes the loop through it, and an overlay built
/// without one folds its log without ever asking for a repaint.
pub fn feature_overlays(
    proxy: Option<EventLoopProxy<UserEvent>>,
    arm: overlay::ArmDeadline,
) -> Vec<Box<dyn Overlay>> {
    // region-capture D10: the capture's watcher thread asks for a look the moment it
    // has an outcome — success or not. A callback rather than the proxy itself, for the
    // reason `session::Wake` is one: it keeps the whole capture path constructible, and
    // so observable, without an event loop.
    let capture_proxy = proxy.clone();
    let capture_wake: capture::Wake = std::sync::Arc::new(move || {
        if let Some(p) = capture_proxy.as_ref() {
            let _ = p.send_event(UserEvent::OverlayPending);
        }
    });
    let flow_proxy = proxy.clone();
    let flow_wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        if let Some(p) = flow_proxy.as_ref() {
            let _ = p.send_event(UserEvent::OverlayPending);
        }
    });
    vec![
        // session-overview-ui: the Cmd+Shift+F full-window Agent Monitor (overview and
        // detail). First, at 300: open, it covers the rename field and the ai-monitor and owns
        // every key (carried D5). Its digest thread wakes the loop the capture way, and it reaches the metadata worker through
        // the installed global — `None` in tests, where it builds from a stood-in snapshot.
        Box::new(flow::FlowOverlay::new(
            flow_wake,
            tabmeta::global().cloned(),
            renderer::overview_panel::OverviewHandle::new(),
        )),
        // sticky-tab-title: the Cmd+Shift+R rename field. Ahead of the ai-monitor
        // so an open field owns the keyboard (D8a.1).
        Box::new(rename::RenameOverlay::new()),
        // ai-monitor's Cmd+Shift+M panel.
        Box::new(AiMonitorOverlay::with_waker(proxy, arm)),
        // image-paste: the inline-image presenter, whose DrawLayer draws snapshot images.
        Box::new(ImagePresenter),
        // copy-clipboard: the selection-highlight DrawLayer, on the same seam.
        Box::new(SelectionPresenter),
        // region-capture: the Cmd+Shift+G screen capture. An overlay rather than a
        // handler for one reason — `feature_handlers()` takes no arguments, so a
        // handler cannot obtain a proxy, and a capture that completes seconds later on
        // a thread has no way to wake the loop. It draws nothing (D10).
        Box::new(capture::CaptureOverlay::new(capture_wake)),
    ]
}

/// The keyboard handlers the app registers through the SD4 seam, in registration
/// order. The input-only half of [`feature_overlays`]; these contribute no layer.
pub fn feature_handlers() -> Vec<Box<dyn InputHandler>> {
    vec![
        // image-paste: Cmd+V.
        Box::new(ClipboardHandler::new()),
        // copy-clipboard: Cmd+C copies the grid selection to the pasteboard.
        Box::new(CopyHandler::new()),
    ]
}
