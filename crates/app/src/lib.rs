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
// So a module is `pub` only where something outside the crate genuinely names it
// — `main.rs` or a test — and `pub(crate)` otherwise, which puts the lint back.
pub(crate) mod card_menu;
pub(crate) mod clipboard;
pub(crate) mod copy;
pub(crate) mod dashboard;
pub(crate) mod hyperlink;
/// `crates/app/tests/vertical_slice.rs` types through the real `encode_key`.
pub mod input;
pub(crate) mod overlay;
pub(crate) mod presence_fd;
pub(crate) mod prompt;
pub(crate) mod rename;
/// The SD4 seam. Named in `terminal_app`'s and this module's public signatures,
/// and asserted by `crates/app/tests/wiring.rs`.
pub mod seam;
/// `crates/app/tests/vertical_slice.rs` drives a real shell through it.
pub mod session;
pub(crate) mod settings;
/// `main.rs` installs `SidebarMetaSource` into the shared sidebar model.
pub mod sidebar_model;
/// `main.rs` spawns and installs the process-global metadata worker.
pub mod tabmeta;
pub(crate) mod tabs;
pub(crate) mod tabsession;
/// `main.rs` builds and runs the `App`.
pub mod terminal_app;
pub(crate) mod waiting;

use winit::event_loop::EventLoopProxy;

use clipboard::{ClipboardHandler, ImagePresenter};
use copy::{CopyHandler, SelectionPresenter};
use overlay::AiMonitorOverlay;
use seam::{InputHandler, Overlay, Registrations};
use terminal_app::UserEvent;

/// Everything `main` hands to [`terminal_app::App::new`], assembled.
///
/// The installation loops used to live inline in `main`, where no test could run
/// them: `main` builds a winit `EventLoop`, which macOS creates only on the
/// process's main thread while the harness runs every test on a spawned one. A
/// loop that dropped its items — or was deleted outright — would have removed
/// every feature from the running app with the whole suite still green.
///
/// As a function it is an ordinary value, and `crates/app/tests/wiring.rs` drives
/// it through [`Registrations::install`] to the live registries. What is left in
/// `main` is one call plus the `tabmeta` worker, which cannot be a value because
/// it installs process-global state.
///
/// `proxy` is `None` only in tests; see [`feature_overlays`].
pub fn feature_registrations(proxy: Option<EventLoopProxy<UserEvent>>) -> Registrations {
    let mut set = Registrations::default();
    for overlay in feature_overlays(proxy) {
        set.overlay(overlay);
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
pub fn feature_overlays(proxy: Option<EventLoopProxy<UserEvent>>) -> Vec<Box<dyn Overlay>> {
    vec![
        // sticky-tab-title: the Cmd+Shift+R rename field. Ahead of the ai-monitor
        // so an open field owns the keyboard (D8a.1).
        Box::new(rename::RenameOverlay::new()),
        // ai-monitor's Cmd+Shift+M panel.
        Box::new(AiMonitorOverlay::with_waker(proxy)),
        // image-paste: the inline-image presenter, whose DrawLayer draws snapshot images.
        Box::new(ImagePresenter),
        // copy-clipboard: the selection-highlight DrawLayer, on the same seam.
        Box::new(SelectionPresenter),
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
