//! Clipboard **copy** (copy-clipboard D6–D9): a Cmd+C `InputHandler` that writes
//! the active grid's selection text to the system clipboard, plus the passthrough
//! overlay that wires the selection-highlight `DrawLayer`.
//!
//! `CopyHandler` **consumes the Cmd+C chord unconditionally** — Cmd+C is a macOS app
//! chord (Copy) and must never fall through to `encode_key`, whose text fallback
//! would type a literal `c` (the same path that makes Cmd+A reach the shell as `a`).
//! It writes only when a selection exists. Ctrl+C (SIGINT) is a `Control` chord, not
//! matched here, so interrupt handling is untouched. The NSPasteboard write sits
//! behind an injectable sink so the consume/write decision is unit-testable without
//! touching the real pasteboard (SD3).

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;
use renderer::selection_layer::SelectionLayer;
use renderer::DrawLayer;
use winit::keyboard::Key;

use crate::seam::{AppCtx, Handled, InputHandler, KeyInput, Overlay};

/// Is this Cmd+C? Case-folded like `is_paste`, so Cmd+Shift+C / caps-lock match.
fn is_copy(input: &KeyInput) -> bool {
    input.mods.super_key() && matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("c"))
}

/// Write a string to the general pasteboard: `clearContents` then `setString`
/// (SD3) — the write mirror of `clipboard::read_clipboard_image_png`'s read. Runs
/// on the UI thread.
fn write_clipboard_string(s: &str) {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns = NSString::from_str(s);
        pb.setString_forType(&ns, NSPasteboardTypeString);
    }
}

/// Cmd+C copy handler (R1). Registered through the SD4 router in `main.rs`.
pub struct CopyHandler {
    /// The clipboard write, injectable so tests substitute a recorder (D7).
    sink: Box<dyn Fn(&str)>,
}

impl CopyHandler {
    pub fn new() -> Self {
        CopyHandler {
            sink: Box::new(write_clipboard_string),
        }
    }

    #[cfg(test)]
    fn with_sink(sink: Box<dyn Fn(&str)>) -> Self {
        CopyHandler { sink }
    }
}

impl Default for CopyHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl InputHandler for CopyHandler {
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if !is_copy(input) {
            return Handled::Passthrough;
        }
        // Consume the chord unconditionally; write only when a selection exists.
        if let Some(text) = &ctx.selection_text {
            (self.sink)(text);
        }
        Handled::Consumed
    }
}

/// A passthrough overlay that registers the selection-highlight `DrawLayer` (D8)
/// through the SD4 overlay seam — the exact structural twin of image-paste's
/// `ImagePresenter`. Never input-capturing; only its draw layer matters.
#[derive(Default)]
pub struct SelectionPresenter;

impl Overlay for SelectionPresenter {
    fn priority(&self) -> i32 {
        0
    }
    fn visible(&self) -> bool {
        false
    }
    fn on_key(&mut self, _input: &KeyInput, _ctx: &mut AppCtx) -> Handled {
        Handled::Passthrough
    }
    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(SelectionLayer::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use winit::keyboard::ModifiersState;

    fn keyinput(ch: &str, cmd: bool) -> KeyInput {
        KeyInput {
            key: Key::Character(ch.into()),
            text: Some(ch.to_string()),
            mods: if cmd {
                ModifiersState::SUPER
            } else {
                ModifiersState::empty()
            },
        }
    }

    #[test]
    fn is_copy_recognizes_cmd_c_case_insensitively() {
        // R0.2 — Cmd+C (any case) yes; plain c and Cmd+V no.
        assert!(is_copy(&keyinput("c", true)));
        assert!(is_copy(&keyinput("C", true)));
        assert!(!is_copy(&keyinput("c", false)));
        assert!(!is_copy(&keyinput("v", true)));
    }

    type Recorded = Rc<RefCell<Vec<String>>>;
    type Sink = Box<dyn Fn(&str)>;

    fn recorder() -> (Recorded, Sink) {
        let rec: Recorded = Rc::new(RefCell::new(Vec::new()));
        let r2 = rec.clone();
        (
            rec,
            Box::new(move |s: &str| r2.borrow_mut().push(s.to_string())),
        )
    }

    #[test]
    fn cmd_c_with_selection_writes_and_consumes() {
        // R1.1.
        let (rec, sink) = recorder();
        let mut h = CopyHandler::with_sink(sink);
        let mut buf: Vec<u8> = Vec::new();
        let mut ctx =
            AppCtx::new(&mut buf, 1, 0, None).with_selection_text(Some("hello world".to_string()));
        assert_eq!(h.on_key(&keyinput("c", true), &mut ctx), Handled::Consumed);
        assert_eq!(rec.borrow().as_slice(), ["hello world".to_string()]);
    }

    #[test]
    fn cmd_c_without_selection_consumes_without_writing() {
        // R1.2 — chord swallowed (no stray `c`), sink not called.
        let (rec, sink) = recorder();
        let mut h = CopyHandler::with_sink(sink);
        let mut buf: Vec<u8> = Vec::new();
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None); // selection_text == None
        assert_eq!(h.on_key(&keyinput("c", true), &mut ctx), Handled::Consumed);
        assert!(rec.borrow().is_empty());
    }

    #[test]
    fn non_cmd_c_passes_through() {
        // R1.3 — Ctrl+C (no super) and Cmd+V pass through untouched.
        let (_rec, sink) = recorder();
        let mut h = CopyHandler::with_sink(sink);
        let mut buf: Vec<u8> = Vec::new();
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(
            h.on_key(&keyinput("c", false), &mut ctx),
            Handled::Passthrough
        );
        assert_eq!(
            h.on_key(&keyinput("v", true), &mut ctx),
            Handled::Passthrough
        );
    }

    #[test]
    fn selection_presenter_is_passthrough_at_selection_z() {
        // R3.1 — the overlay contributes the SelectionLayer and captures no input.
        let mut p = SelectionPresenter;
        assert_eq!(p.priority(), 0);
        assert!(!p.visible());
        let mut buf: Vec<u8> = Vec::new();
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(
            p.on_key(&keyinput("k", false), &mut ctx),
            Handled::Passthrough
        );
        assert_eq!(p.layer().z_order(), renderer::selection_layer::SELECTION_Z);
    }
}
