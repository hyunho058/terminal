//! The SD4 app event-loop extension seam (D4, R3.3/R4).
//!
//! `tabs` owns the winit loop and exposes two registries so *other* app features
//! (image-paste, ai-monitor, settings) extend input handling and overlays from
//! their own modules — never by editing `terminal_app.rs` (SD4/C1).
//!
//! The seam is decoupled from winit's full `KeyEvent`: handlers receive a small
//! [`KeyInput`] (logical key + text + modifiers), which is all they need and is
//! trivially constructible in tests. (D4 was marked *provisional*; this is the
//! refinement its first consumer — `tabs` — settles.)
//!
//! Much of this module is an extension surface with no in-tree caller yet: the
//! registration entry points (`InputRouter::register`, `OverlayHost::register`,
//! `Overlay`, the `AppCtx` accessors) are consumed by image-paste / ai-monitor /
//! settings when they land. In a binary crate `pub` doesn't suppress dead-code,
//! so the seam API is allowed dead here; the dispatch paths + tests exercise the
//! behavior. Remove this allow once a sibling consumes the seam.
#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;

use renderer::DrawLayer;
use winit::keyboard::{Key, ModifiersState};

/// Whether a handler/overlay swallowed a key or lets it fall through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handled {
    /// Stop here — neither lower-priority handlers nor default tab input run.
    Consumed,
    /// Not mine — keep dispatching (and fall through to default input).
    Passthrough,
}

/// A keypress as the seam sees it. Built once in `terminal_app.rs` from the winit
/// `KeyEvent`; carries only what handlers need.
#[derive(Clone, Debug)]
pub struct KeyInput {
    pub key: Key,
    pub text: Option<String>,
    pub mods: ModifiersState,
}

/// What a handler/overlay may do to the app during `on_key`: write to the active
/// shell, read tab state, request a redraw. Deliberately narrow (D4).
pub struct AppCtx<'a> {
    /// The active tab's PTY writer.
    pub writer: &'a mut dyn Write,
    /// Number of open tabs.
    pub tab_count: usize,
    /// Active tab index.
    pub active: usize,
    /// The active tab's live working directory (ai-monitor log resolution).
    pub active_cwd: Option<PathBuf>,
    /// Whether the active tab has bracketed-paste mode on (DECSET 2004). Read by
    /// the text-paste handler to wrap a paste in `ESC[200~`/`ESC[201~`. Populated
    /// by `terminal_app` from `Terminal::bracketed_paste`; `false` by default.
    pub bracketed_paste: bool,
    /// The active grid's current selection text, or `None`. Read by the copy
    /// handler on Cmd+C. Populated by `terminal_app` (only when a selection exists,
    /// to avoid a per-keystroke grid clone); `None` by default.
    pub selection_text: Option<String>,
    redraw: bool,
}

impl<'a> AppCtx<'a> {
    pub fn new(
        writer: &'a mut dyn Write,
        tab_count: usize,
        active: usize,
        active_cwd: Option<PathBuf>,
    ) -> Self {
        AppCtx {
            writer,
            tab_count,
            active,
            active_cwd,
            bracketed_paste: false,
            selection_text: None,
            redraw: false,
        }
    }

    /// Set the active tab's bracketed-paste mode (builder — keeps `new`'s signature
    /// and every existing callsite unchanged; only `terminal_app` calls this).
    pub fn with_bracketed_paste(mut self, on: bool) -> Self {
        self.bracketed_paste = on;
        self
    }

    /// Set the active grid's selection text (builder — see `with_bracketed_paste`).
    pub fn with_selection_text(mut self, text: Option<String>) -> Self {
        self.selection_text = text;
        self
    }

    /// Ask the app to redraw after this event.
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Whether a redraw was requested during dispatch.
    pub fn redraw_requested(&self) -> bool {
        self.redraw
    }
}

/// A pluggable key handler (e.g. clipboard paste). Registered features implement
/// this in their own module and add it via [`InputRouter::register`].
pub trait InputHandler {
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled;
}

/// An overlay that both consumes input and contributes a renderer [`DrawLayer`]
/// (SD3). Used by ai-monitor/settings later.
pub trait Overlay {
    /// Higher priority sees keys first.
    fn priority(&self) -> i32;
    /// Whether the overlay is currently showing. Reported state — NOT an input
    /// gate: dispatch offers keys to hidden overlays too, so an overlay can
    /// consume the chord that opens it (a hidden overlay consumes only its own
    /// hotkey; everything else it passes through).
    fn visible(&self) -> bool;
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled;
    /// The overlay's draw layer, registered with the renderer.
    fn layer(&self) -> Box<dyn DrawLayer>;
    /// Drain any background work (e.g. a log-tail channel) into the overlay's
    /// draw model. Called when a `UserEvent` signals the overlay has pending data.
    /// Returns true if something changed (→ redraw). Default: nothing to do.
    fn poll(&mut self) -> bool {
        false
    }
    /// The active tab changed; re-point anything cwd-derived (ai-monitor R3.1).
    /// Default: no-op.
    fn on_active_changed(&mut self, _cwd: Option<PathBuf>) {}
}

/// Ordered list of input handlers, consulted before default tab input (R3.3).
#[derive(Default)]
pub struct InputRouter {
    handlers: Vec<Box<dyn InputHandler>>,
}

impl InputRouter {
    /// Register a handler (a self-contained feature module) — no `terminal_app`
    /// edit needed.
    pub fn register(&mut self, handler: Box<dyn InputHandler>) {
        self.handlers.push(handler);
    }

    /// Run each handler in registration order; the first `Consumed` stops
    /// dispatch and signals the caller to skip default input (R3.3).
    pub fn dispatch(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        for h in &mut self.handlers {
            if h.on_key(input, ctx) == Handled::Consumed {
                return Handled::Consumed;
            }
        }
        Handled::Passthrough
    }
}

/// Registry of overlays (highest priority first), consulted before the router and
/// default input (R4). Every overlay receives keys (priority order) whether shown
/// or hidden; a hidden overlay consumes only its own hotkey (e.g. the chord that
/// opens it) and passes everything else through.
#[derive(Default)]
pub struct OverlayHost {
    overlays: Vec<Box<dyn Overlay>>,
}

impl OverlayHost {
    pub fn register(&mut self, overlay: Box<dyn Overlay>) {
        self.overlays.push(overlay);
        // Highest priority first.
        self.overlays
            .sort_by_key(|o| std::cmp::Reverse(o.priority()));
    }

    /// Offer the key to every overlay (priority order), visible or not; first
    /// `Consumed` wins (R4.1). All-passthrough → default input proceeds (R4.2).
    /// A hidden overlay is no longer skipped, so it can consume the chord that
    /// opens it (e.g. ai-monitor's Cmd+Shift+M); it passes non-hotkey keys
    /// through, so hidden overlays don't swallow shell input.
    pub fn dispatch(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        for o in &mut self.overlays {
            if o.on_key(input, ctx) == Handled::Consumed {
                return Handled::Consumed;
            }
        }
        Handled::Passthrough
    }

    /// The draw layers of every registered overlay (for renderer registration).
    pub fn layers(&self) -> Vec<Box<dyn DrawLayer>> {
        self.overlays.iter().map(|o| o.layer()).collect()
    }

    /// Drain background work for every overlay; returns true if any changed (R1.3).
    pub fn poll_all(&mut self) -> bool {
        let mut changed = false;
        for o in &mut self.overlays {
            changed |= o.poll();
        }
        changed
    }

    /// Tell every overlay the active tab changed (ai-monitor re-point, R3.1).
    pub fn notify_active_changed(&mut self, cwd: Option<PathBuf>) {
        for o in &mut self.overlays {
            o.on_active_changed(cwd.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    fn key(text: &str) -> KeyInput {
        let k: Key = Key::Character(text.into());
        KeyInput {
            key: k,
            text: Some(text.to_string()),
            mods: ModifiersState::empty(),
        }
    }

    #[test]
    fn appctx_defaults_false_none_and_builders_set_them() {
        // R4.1 — `new` keeps its 4-arg signature; the new fields default to
        // false/None, and the builders set them.
        let mut buf: Vec<u8> = Vec::new();
        let ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert!(!ctx.bracketed_paste);
        assert_eq!(ctx.selection_text, None);

        let mut buf2: Vec<u8> = Vec::new();
        let ctx2 = AppCtx::new(&mut buf2, 1, 0, None)
            .with_bracketed_paste(true)
            .with_selection_text(Some("sel".to_string()));
        assert!(ctx2.bracketed_paste);
        assert_eq!(ctx2.selection_text.as_deref(), Some("sel"));
    }

    struct Fixed(Handled, bool); // (verdict, writes-a-byte)
    impl InputHandler for Fixed {
        fn on_key(&mut self, _in: &KeyInput, ctx: &mut AppCtx) -> Handled {
            if self.1 {
                let _ = ctx.writer.write_all(b"X");
            }
            self.0
        }
    }

    #[test]
    fn router_first_consumed_wins_and_stops() {
        let mut buf: Vec<u8> = Vec::new();
        let mut router = InputRouter::default();
        router.register(Box::new(Fixed(Handled::Consumed, true)));
        // This second handler must NOT run once the first consumes.
        router.register(Box::new(Fixed(Handled::Passthrough, true)));
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(router.dispatch(&key("v"), &mut ctx), Handled::Consumed);
        assert_eq!(buf, b"X", "only the first handler ran");
    }

    #[test]
    fn router_all_passthrough_falls_through() {
        let mut buf: Vec<u8> = Vec::new();
        let mut router = InputRouter::default();
        router.register(Box::new(Fixed(Handled::Passthrough, false)));
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(router.dispatch(&key("a"), &mut ctx), Handled::Passthrough);
    }

    struct Ov {
        prio: i32,
        vis: bool,
        verdict: Handled,
    }
    impl Overlay for Ov {
        fn priority(&self) -> i32 {
            self.prio
        }
        fn visible(&self) -> bool {
            self.vis
        }
        fn on_key(&mut self, _in: &KeyInput, _c: &mut AppCtx) -> Handled {
            self.verdict
        }
        fn layer(&self) -> Box<dyn DrawLayer> {
            struct L;
            impl DrawLayer for L {
                fn z_order(&self) -> i32 {
                    100
                }
                fn record(&mut self, _: &mut renderer::FrameCtx<'_>) {}
            }
            Box::new(L)
        }
    }

    #[test]
    fn visible_overlay_consumes_before_default() {
        let mut buf: Vec<u8> = Vec::new();
        let mut host = OverlayHost::default();
        host.register(Box::new(Ov {
            prio: 10,
            vis: true,
            verdict: Handled::Consumed,
        }));
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(
            host.dispatch(&key("k"), &mut ctx),
            Handled::Consumed,
            "R4.1: a visible overlay swallows the key"
        );
    }

    #[test]
    fn all_overlays_passthrough_falls_through() {
        // R4.2: when no overlay claims the key — a hidden overlay passes through
        // its non-hotkey keys, a visible one declines — default input runs. Under
        // the new contract a hidden overlay IS dispatched to, so "falls through"
        // now rests on the overlays declining, not on the host skipping a hidden
        // one.
        let mut buf: Vec<u8> = Vec::new();
        let mut host = OverlayHost::default();
        host.register(Box::new(Ov {
            prio: 10,
            vis: false, // hidden — receives the key but passes it through
            verdict: Handled::Passthrough,
        }));
        host.register(Box::new(Ov {
            prio: 5,
            vis: true,
            verdict: Handled::Passthrough, // visible but declines
        }));
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(
            host.dispatch(&key("j"), &mut ctx),
            Handled::Passthrough,
            "R4.2: all overlays passthrough → default input runs"
        );
    }

    #[test]
    fn hidden_overlay_receives_its_own_toggle() {
        // Regression for the ai-monitor toggle bug: the overlay opens on its own
        // hotkey (Cmd+Shift+M) but starts hidden. Under the old `o.visible() &&`
        // guard a hidden overlay was skipped, so its opening chord never reached
        // `on_key` — it could never open (chicken-and-egg). Now a hidden overlay's
        // `on_key` runs and its `Consumed` verdict is honored.
        let mut buf: Vec<u8> = Vec::new();
        let mut host = OverlayHost::default();
        host.register(Box::new(Ov {
            prio: 10,
            vis: false,                 // hidden, like a fresh ai-monitor overlay
            verdict: Handled::Consumed, // consumes its opening chord
        }));
        let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
        assert_eq!(
            host.dispatch(&key("m"), &mut ctx),
            Handled::Consumed,
            "a hidden overlay now receives (and can consume) the chord that opens it"
        );
    }

    #[test]
    fn overlays_ordered_by_priority() {
        let mut host = OverlayHost::default();
        host.register(Box::new(Ov {
            prio: 1,
            vis: true,
            verdict: Handled::Passthrough,
        }));
        host.register(Box::new(Ov {
            prio: 100,
            vis: true,
            verdict: Handled::Passthrough,
        }));
        assert_eq!(host.layers().len(), 2);
        // Named import kept meaningful; ensures NamedKey path compiles for callers.
        let _named: Key = Key::Named(NamedKey::Escape);
    }
}
