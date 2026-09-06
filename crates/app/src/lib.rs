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
//! [`feature_handlers`]) rather than written inline, so the registration set is a
//! value a test can inspect instead of a sequence of statements only a running
//! app can observe.

pub mod card_menu;
pub mod clipboard;
pub mod copy;
pub mod dashboard;
pub mod hyperlink;
pub mod input;
pub mod overlay;
pub mod presence_fd;
pub mod prompt;
pub mod rename;
pub mod seam;
pub mod session;
pub mod settings;
pub mod sidebar_model;
pub mod tabmeta;
pub mod tabs;
pub mod tabsession;
pub mod terminal_app;
pub mod waiting;

use winit::event_loop::EventLoopProxy;

use clipboard::{ClipboardHandler, ImagePresenter};
use copy::{CopyHandler, SelectionPresenter};
use overlay::AiMonitorOverlay;
use seam::{InputHandler, Overlay};
use terminal_app::UserEvent;

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
