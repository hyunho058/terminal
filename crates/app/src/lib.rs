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
pub(crate) mod hyperlink;
pub(crate) mod ime_chord;
pub(crate) mod input;
pub(crate) mod overlay;
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
/// `crates/app/tests/vertical_slice.rs` types through the real encoder.
pub use input::encode_key;
/// The SD4 seam: named in the signatures below and asserted by
/// `crates/app/tests/wiring.rs`.
pub use overlay::{no_deadline, ArmDeadline};
pub use seam::{InputHandler, Overlay, Registrations};
/// `crates/app/tests/vertical_slice.rs` drives a real shell through a session.
pub use session::{Session, TabId, Wake};
/// `main.rs` installs the sidebar's metadata source.
pub use sidebar_model::SidebarMetaSource;
/// `main.rs` spawns the process-global metadata worker and installs its handle.
pub use tabmeta::{install as install_tabmeta, TabMetaHandle};
/// `main.rs` builds and runs the app.
pub use terminal_app::{App, UserEvent};

use winit::event_loop::EventLoopProxy;

use clipboard::{ClipboardHandler, ImagePresenter};
use copy::{CopyHandler, SelectionPresenter};
use overlay::AiMonitorOverlay;

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
    vec![
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
