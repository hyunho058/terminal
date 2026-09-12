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
//! This module used to carry a blanket `#![allow(dead_code)]`, on the grounds
//! that a *binary* crate does not let `pub` suppress the lint and the seam had no
//! in-tree caller yet. Both halves have since expired: `app` is a library, and
//! image-paste, ai-monitor, rename and copy all consume the seam. In a library,
//! `pub` in a `pub mod` makes an item reachable from outside the crate, so the
//! lint would not fire here whatever the allow said — keeping it would only have
//! implied a guard that is not there.

use std::io::Write;
use std::path::PathBuf;

use renderer::DrawLayer;
use winit::keyboard::{Key, ModifiersState, PhysicalKey};

use crate::session::TabId;

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
    /// The key's position on the keyboard, independent of what it produced.
    ///
    /// Carried because a chord over a **digit** cannot be recognised from the logical key
    /// (ai-monitor-panel-scroll D35): on a US layout `Cmd+Opt+1` arrives as `"¡"`, and
    /// unlike letters — where `is_sidebar_toggle_chord` gets away with case-folding, as its
    /// own comment says — digits have no folding to fall back on. Matching the shifted
    /// characters instead (`!@#$%^`) would work on that one layout and break silently on
    /// every other.
    pub physical: PhysicalKey,
}

/// "No position information" — what a synthetic `KeyInput` carries.
///
/// A handler matching on `physical` must read this as *not my key*, never as a wildcard: a
/// chord that fired on an unknown position would be a chord nobody pressed.
///
/// `#[cfg(test)]` because only a test ever builds a `KeyInput` without a position:
/// the one production site (`terminal_app`'s `KeyboardInput` arm) copies
/// `key_event.physical_key` straight from winit. Restoring it to production is
/// deleting this attribute — which is a decision someone makes when a real caller
/// appears, rather than an `#[allow(dead_code)]` promising one indefinitely.
#[cfg(test)]
pub const NO_PHYSICAL: PhysicalKey =
    PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);

/// What a handler/overlay may do to the app during `on_key`: write to the active
/// shell, read tab state, request a redraw. Deliberately narrow (D4).
pub struct AppCtx<'a> {
    /// The active tab's PTY writer.
    ///
    /// **Invariant: user-originated bytes only** (terminal-scroll D4). Anything written
    /// here is treated as "the user typed something and expects to see it echoed", and
    /// snaps a scrolled-back viewport to the live bottom. A handler that needs to write
    /// for a *non-echo* reason — a shell-integration query, a mode probe, an OSC cwd
    /// request — must NOT reuse this: it would silently yank the reader out of the
    /// history they were looking at, with nothing at the call site to hint at it. Add an
    /// `AppCtx::write_silent()` that bypasses [`WriteProbe`] instead. (Not built yet: it
    /// would be dead code until the first such handler exists.)
    pub writer: &'a mut dyn Write,
    /// Number of open tabs.
    pub tab_count: usize,
    /// Active tab index.
    pub active: usize,
    /// The active tab's live working directory (ai-monitor log resolution).
    pub active_cwd: Option<PathBuf>,
    /// The active tab's shell pid. The log directory records no application, window
    /// or tab, so this process tree is the only way to tell this tab's Claude session
    /// from one another app is running in the same project. `None` if the shell has
    /// exited. Populated by `terminal_app`.
    pub active_shell_pid: Option<u32>,
    /// Whether the active tab has bracketed-paste mode on (DECSET 2004). Read by
    /// the text-paste handler to wrap a paste in `ESC[200~`/`ESC[201~`. Populated
    /// by `terminal_app` from `Terminal::bracketed_paste`; `false` by default.
    pub bracketed_paste: bool,
    /// The active grid's current selection text, or `None`. Read by the copy
    /// handler on Cmd+C. Populated by `terminal_app` (only when a selection exists,
    /// to avoid a per-keystroke grid clone); `None` by default.
    pub selection_text: Option<String>,
    /// The active tab's stable id and its current user title (sticky-tab-title R2.1).
    /// `active` above is an *index*, which is not enough to name a tab: the mutation path
    /// is keyed by [`TabId`] (R1.4), and the rename chord needs the current title to
    /// prefill its field. `None` only in tests that build a bare context.
    pub active_tab: Option<TabId>,
    /// The active tab's user title, for prefilling the rename field. `None` = unnamed.
    pub active_user_title: Option<String>,
    redraw: bool,
    requests: Vec<AppRequest>,
}

/// A mutation an overlay asks the app to perform (sticky-tab-title D9/R1.4).
///
/// **Deferred, not immediate, and that is forced by the borrow graph**: `AppCtx` already
/// holds `&mut` the active session's writer, so it cannot also hold `&mut Tabs` — nor
/// `&mut Renderer`. Handlers therefore *record* what they want and `terminal_app` applies
/// it after dispatch returns — the same shape as [`AppCtx::request_redraw`] /
/// [`AppCtx::redraw_requested`], which solved the identical problem for redraws.
///
/// Every *tab* variant names a [`TabId`], never an index: the card menu can act on a
/// background tab, and an index would silently mean a different tab if the roster shifted
/// between the press and the apply.
///
/// Every payload here is comparable by identity — a [`TabId`], an `Option<String>`, a
/// `bool` — so `Eq` is derived. It was briefly absent while the ai-monitor variant carried
/// a width in logical px, and a float has no total equality; that width turned out never to
/// cross this seam at all (see [`AppRequest::SetMonitorOpen`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRequest {
    /// Name a tab, or reset it to its auto label with `None` (R2.2/R2.3).
    SetUserTitle(TabId, Option<String>),
    /// Close a tab (R3.6). Closing the last one exits the app, exactly as Cmd+W does.
    CloseTab(TabId),
    /// Open or close the band the docked ai-monitor panel occupies on the window's right
    /// edge (ai-monitor-right-panel D21).
    ///
    /// **Open/closed is the whole payload, and that is the point.** The band's *width* is
    /// not the overlay's to choose: D6 makes `[ai_monitor].width_px` plus the runtime
    /// override the authority, and the renderer derives the band from those — which is what
    /// lets a width dragged last week come back on this toggle (R4.5). A variant carrying a
    /// width would give the panel a second author for a value only the drag and the config
    /// may set.
    ///
    /// A window property, not a tab property — the panel is one per window and re-points
    /// on a tab switch (D13), so this names no [`TabId`].
    SetMonitorOpen(bool),
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
            active_shell_pid: None,
            bracketed_paste: false,
            selection_text: None,
            active_tab: None,
            active_user_title: None,
            redraw: false,
            requests: Vec::new(),
        }
    }

    /// Set the active tab's id + current user title (builder — see `with_bracketed_paste`).
    pub fn with_active_tab(mut self, id: TabId, user_title: Option<String>) -> Self {
        self.active_tab = Some(id);
        self.active_user_title = user_title;
        self
    }

    /// Attach the active tab's shell pid (ai-monitor tab binding).
    pub fn with_shell_pid(mut self, pid: Option<u32>) -> Self {
        self.active_shell_pid = pid;
        self
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

    /// Ask the app to set (or with `None`, clear) a tab's user title
    /// (sticky-tab-title R1.4/R2.2). Applied after dispatch returns — see [`AppRequest`].
    pub fn set_user_title(&mut self, tab: TabId, title: Option<String>) {
        self.requests.push(AppRequest::SetUserTitle(tab, title));
    }

    /// Ask the app to close a tab (sticky-tab-title R3.6). Applied after dispatch returns.
    pub fn close_tab(&mut self, tab: TabId) {
        self.requests.push(AppRequest::CloseTab(tab));
    }

    /// Ask the app to open or close the ai-monitor panel's docked band
    /// (ai-monitor-right-panel D21). Applied after dispatch returns — see [`AppRequest`]
    /// for why an overlay cannot touch the renderer directly, and for why the width does
    /// not travel with it.
    pub fn set_monitor_open(&mut self, open: bool) {
        self.requests.push(AppRequest::SetMonitorOpen(open));
    }

    /// Drain the requests recorded during dispatch, in the order they were made. Called
    /// by `terminal_app` once the `AppCtx` borrow is released.
    pub fn take_requests(&mut self) -> Vec<AppRequest> {
        std::mem::take(&mut self.requests)
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
    /// One wheel event over the overlay's **own** surface.
    ///
    /// Called only after the app's single ordered hit-test has resolved the pointer to this
    /// overlay's band, so an overlay performs no geometry of its own — the same division
    /// `sidebar_press` already keeps for the pointer (SD2).
    ///
    /// `Consumed` means the overlay moved; `Passthrough` means it did not and the grid
    /// should get the event instead. That is how the ai-monitor panel's edge fall-through
    /// (D9) is expressed: at the end of its document, in the direction that pushes further
    /// into that end, the panel simply declines. **It declines by direction, not by
    /// position** — an overlay that returned `Passthrough` whenever it sat at an edge would
    /// never receive the wheel again, since scrolling back up leaves it at that same edge.
    ///
    /// Default: not mine.
    fn on_wheel(&mut self, _delta: crate::input::WheelDelta, _cell_h: f32) -> Handled {
        Handled::Passthrough
    }
    /// One left press on the overlay's **own** surface, in window px.
    ///
    /// Called only after the app's single ordered hit-test has put the pointer in this
    /// overlay's band, so the overlay resolves the point against its own published geometry
    /// and the app performs none (SD2) — the same division `on_wheel` keeps.
    ///
    /// `Consumed` means the overlay acted on it; `Passthrough` means the press landed on
    /// nothing it owns and the surface below should get it.
    ///
    /// Default: not mine.
    fn on_press(&mut self, _point: [f32; 2]) -> Handled {
        Handled::Passthrough
    }
    /// The overlay's draw layer, registered with the renderer.
    ///
    /// Default: a no-op layer, for an overlay that draws through **someone else's**
    /// model rather than a layer of its own — the inline rename field paints onto the
    /// sidebar card it is renaming, so it owns no surface to register.
    fn layer(&self) -> Box<dyn DrawLayer> {
        struct NoLayer;
        impl DrawLayer for NoLayer {
            fn z_order(&self) -> i32 {
                0
            }
            fn record(&mut self, _: &mut renderer::FrameCtx<'_>) {}
        }
        Box::new(NoLayer)
    }
    /// Drain any background work (e.g. a log-tail channel) into the overlay's
    /// draw model. Called when a `UserEvent` signals the overlay has pending data.
    /// Returns true if something changed (→ redraw). Default: nothing to do.
    fn poll(&mut self) -> bool {
        false
    }
    /// The active tab changed; re-point anything cwd-derived (ai-monitor R3.1).
    /// Carries the new tab's shell pid alongside its cwd, since the two together are
    /// what identify a session. Default: no-op.
    ///
    /// `agent_pid` is the tab's **Claude** process as the tabmeta worker last derived it
    /// (`TabMeta::agent_pid`). Carried because a panel whose directory is a function of that
    /// process's cwd goes stale exactly when the *pid* changes, and the worker already
    /// computes it — so this seam is the cheapest place to notice. `None` means **no
    /// information**, never "a different pid": a consumer must not treat an absent value as a
    /// change, or a toggle flip or a transient walk failure becomes a re-point storm.
    fn on_active_changed(
        &mut self,
        _cwd: Option<PathBuf>,
        _shell_pid: Option<u32>,
        _agent_pid: Option<u32>,
    ) {
    }

    /// The app is driving this overlay (sticky-tab-title D13/R3.4) — e.g. the card menu's
    /// `Rename` item opening the rename field on a specific tab. Return `Consumed` to stop
    /// the broadcast. Default: no-op passthrough, so no existing overlay changes.
    fn on_command(&mut self, _cmd: &OverlayCommand) -> Handled {
        Handled::Passthrough
    }
}

/// A `Write` that records whether anything was written through it (terminal-scroll D4).
///
/// This is how "user input reached the shell" is detected without classifying
/// keystrokes. Classifying was tried first and is provably wrong: `Ime::Commit` writes
/// from a different `WindowEvent` arm that `on_key` never sees (so every CJK keystroke
/// would type into a frozen screen), and Cmd+V paste writes from a *registered handler*,
/// two dispatch steps before the default path. Wrapping the writer catches both, and any
/// future handler, with no change to the handlers themselves.
///
/// Its predicate ("bytes reached the PTY") is a proxy for the real one ("the user typed
/// something"). They come apart for a handler that writes for a non-echo reason — see
/// the invariant documented on [`AppCtx::writer`], which is what keeps them aligned.
pub struct WriteProbe<'w> {
    inner: &'w mut dyn Write,
    wrote: bool,
}

impl<'w> WriteProbe<'w> {
    pub fn new(inner: &'w mut dyn Write) -> Self {
        WriteProbe {
            inner,
            wrote: false,
        }
    }

    /// Whether any byte was written through this probe during the dispatch.
    pub fn wrote(&self) -> bool {
        self.wrote
    }
}

impl Write for WriteProbe<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        // Only a write that actually moved bytes counts — a zero-length write is not
        // the user typing.
        self.wrote |= n > 0;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
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

    /// How many handlers are registered.
    ///
    /// The registries are otherwise write-only: nothing can ask a router whether
    /// anything reached it, which is exactly why an installer that dropped every
    /// handler left the whole suite green. This is the read side of that.
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
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

    /// Offer one wheel event to the **visible** overlays, in priority order.
    ///
    /// Visibility gates this where it deliberately does not gate `dispatch`: a key can open a
    /// hidden overlay, but a wheel event over a band that is not on screen belongs to
    /// whatever *is* — there is nothing there to scroll.
    pub fn wheel(&mut self, delta: crate::input::WheelDelta, cell_h: f32) -> Handled {
        for o in &mut self.overlays {
            if o.visible() && o.on_wheel(delta, cell_h) == Handled::Consumed {
                return Handled::Consumed;
            }
        }
        Handled::Passthrough
    }

    /// Offer one left press to the **visible** overlays, in priority order.
    ///
    /// Visibility gates this for the reason it gates `wheel`: a band that is not on screen
    /// owns no pixels, so a press there belongs to whatever is drawn instead.
    pub fn press(&mut self, point: [f32; 2]) -> Handled {
        for o in &mut self.overlays {
            if o.visible() && o.on_press(point) == Handled::Consumed {
                return Handled::Consumed;
            }
        }
        Handled::Passthrough
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

    /// The registered overlays' priorities, in the order [`Self::dispatch`]
    /// offers keys to them.
    ///
    /// Registration is otherwise unobservable — an overlay that never arrived,
    /// or arrived behind one that eats its keys, raises no error anywhere and the
    /// chord simply does nothing. This lets a test state what the live registry
    /// holds instead of what the list handed to it did.
    pub fn priorities(&self) -> Vec<i32> {
        self.overlays.iter().map(|o| o.priority()).collect()
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
    pub fn notify_active_changed(
        &mut self,
        cwd: Option<PathBuf>,
        shell_pid: Option<u32>,
        agent_pid: Option<u32>,
    ) {
        for o in &mut self.overlays {
            o.on_active_changed(cwd.clone(), shell_pid, agent_pid);
        }
    }

    /// Send a command from the **app to the overlays** (sticky-tab-title D13/R3.4), in
    /// priority order; the first `Consumed` wins, exactly like [`dispatch`].
    ///
    /// This direction did not exist before: the seam could hand overlays keys and
    /// active-tab changes, and (with `AppRequest`) overlays could ask the app for tab
    /// mutations — but the app had no way to *drive* an overlay. The card menu needs
    /// precisely that: choosing `Rename` is a mouse press handled in `terminal_app`, and
    /// it must open the rename field on a chosen tab in the same event.
    ///
    /// [`dispatch`]: OverlayHost::dispatch
    /// `Cancel` is a **fan-out**: it means "close any open modal surface", so every
    /// overlay must see it. The others are first-wins like [`dispatch`] — `BeginRename`
    /// is addressed to whichever overlay owns renaming, and stopping at the first taker
    /// is correct.
    ///
    /// The distinction is load-bearing: short-circuiting `Cancel` would mean a future
    /// higher-priority overlay that consumes it silently strands an open field below it.
    pub fn command(&mut self, cmd: &OverlayCommand) -> Handled {
        let fan_out = matches!(cmd, OverlayCommand::Cancel);
        let mut handled = Handled::Passthrough;
        for o in &mut self.overlays {
            if o.on_command(cmd) == Handled::Consumed {
                handled = Handled::Consumed;
                if !fan_out {
                    break;
                }
            }
        }
        handled
    }
}

/// The seam's registrations, held as a value until there is a window to install
/// them into.
///
/// Features register before `resumed` — there is no renderer to hand a
/// [`DrawLayer`] to yet — so the set has to be carried. It is a named type rather
/// than two `Vec` fields on `App` for one reason: `App` cannot be constructed in
/// a test. It needs an `EventLoopProxy`, which needs an `EventLoop`, which macOS
/// builds only on the process's main thread while the harness runs every test on
/// a spawned one. So as long as the registration→registry step lived inside
/// `App`, no test could reach it, and replacing it with a no-op left the entire
/// suite green while every feature silently vanished from the running app.
///
/// Pulled out here it is an ordinary value transformation, and
/// `crates/app/tests/wiring.rs` asserts it. What remains outside any test's reach
/// is the two lines that store this on `App` and call [`Self::install`] from
/// `resumed` — a named limit, not a hidden one.
#[derive(Default)]
pub struct Registrations {
    handlers: Vec<Box<dyn InputHandler>>,
    overlays: Vec<Box<dyn Overlay>>,
}

impl Registrations {
    /// Add an input handler (SD4/R3.3) — the seam entry a sibling feature calls
    /// instead of editing the event loop.
    pub fn handler(&mut self, handler: Box<dyn InputHandler>) {
        self.handlers.push(handler);
    }

    /// Add an overlay (SD4/R4): its `on_key` runs before default input and its
    /// [`DrawLayer`] is drawn above the grid.
    pub fn overlay(&mut self, overlay: Box<dyn Overlay>) {
        self.overlays.push(overlay);
    }

    /// How many handlers and overlays are pending, in that order.
    pub fn counts(&self) -> (usize, usize) {
        (self.handlers.len(), self.overlays.len())
    }

    /// Drain the pending set into live registries, plus the draw layers for the
    /// renderer to stack.
    ///
    /// The layers come back in **registration** order, not the priority order
    /// [`OverlayHost`] sorts itself into: priority decides who is offered a key
    /// first, stacking decides who is painted over whom, and the two are
    /// unrelated. Registration order is what the previous inline loop produced,
    /// and changing it would silently restack the screen.
    pub fn install(&mut self) -> (InputRouter, OverlayHost, Vec<Box<dyn DrawLayer>>) {
        let mut router = InputRouter::default();
        for h in self.handlers.drain(..) {
            router.register(h);
        }
        let mut host = OverlayHost::default();
        let mut layers = Vec::with_capacity(self.overlays.len());
        for o in self.overlays.drain(..) {
            layers.push(o.layer());
            host.register(o);
        }
        (router, host, layers)
    }
}

/// Something the app asks an overlay to do (sticky-tab-title D13/R3.4). The counterpart
/// of [`AppRequest`]: that type flows overlay → app, this one app → overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayCommand {
    /// Open the rename field targeting `tab`, prefilled with `current` (its user title,
    /// or `None` for a tab that has never been named).
    BeginRename { tab: TabId, current: Option<String> },
    /// Close any open modal surface without committing anything (R2.5/R3.8) — a tab
    /// switch, a click elsewhere, or a session that died under an open field.
    Cancel,
    /// A committed IME syllable (sticky-tab-title R2.6). Offered to overlays **before**
    /// the PTY: `Ime::Commit` otherwise writes straight to a session, so with a text field
    /// open every Hangul syllable would land in zsh while the field received nothing.
    /// `Consumed` means an overlay took the text and it must NOT reach the shell.
    ImeCommit(String),
    /// The in-flight IME composition (sticky-tab-title R2.6) — the uncommitted jamo, so a
    /// capturing field can show what is being composed instead of appearing frozen until
    /// the syllable lands. Empty text is winit's clear signal.
    ImePreedit(String),
    /// Reopen the ai-monitor panel at launch because `[ai_monitor].open` says the last
    /// session left it open (ai-monitor-right-panel R6.6).
    ///
    /// It travels this direction — app → overlay — because the app is what holds the
    /// config, and it carries the active tab's `cwd` + `shell_pid` because the overlay's
    /// own open path needs both to resolve *this tab's* session: at launch nothing has
    /// handed them over yet, since the first `on_active_changed` is still a metadata sync
    /// away. A restore that arrived without them would resolve the wrong project directory,
    /// which looks exactly like an empty panel.
    ///
    /// Deliberately **not** a keystroke and not an [`AppRequest`]: `on_command` is handed no
    /// [`AppCtx`], so a restore has no route to the config and cannot be mistaken for the
    /// gesture D8 reserves the right to persist.
    RestoreMonitor {
        cwd: Option<PathBuf>,
        shell_pid: Option<u32>,
    },
}

#[cfg(test)]
mod tests {

    // ---- ai-monitor-panel-scroll T7 / R5.6: what an overlay's refusal routes to ----

    #[test]
    fn r5_6_an_overlay_that_declines_the_wheel_leaves_it_for_the_grid() {
        // D9's fall-through is expressed as a REFUSAL, not as a redirect: the panel answers
        // `Passthrough` and the caller's ordered hit-test then runs `wheel_grid`, which is
        // what keeps a `?1000` application's claim on the wheel intact (`alt-screen-scroll`).
        // Routing the event to the grid from inside the overlay would bypass that.
        //
        // What is pinned here is the seam's half — that a decline actually reaches the
        // caller. The one line that turns it into `wheel_grid` lives in `terminal_app`'s
        // event arm and needs a window to run.
        struct Declines(bool);
        impl Overlay for Declines {
            fn priority(&self) -> i32 {
                0
            }
            fn visible(&self) -> bool {
                true
            }
            fn on_key(&mut self, _: &KeyInput, _: &mut AppCtx) -> Handled {
                Handled::Passthrough
            }
            fn on_wheel(&mut self, _: crate::input::WheelDelta, _: f32) -> Handled {
                if self.0 {
                    Handled::Consumed
                } else {
                    Handled::Passthrough
                }
            }
        }
        let mut host = OverlayHost::default();
        host.register(Box::new(Declines(false)));
        assert_eq!(
            host.wheel(crate::input::WheelDelta::Lines(-1.0), 16.0),
            Handled::Passthrough,
            "a decline must reach the caller so the grid path can run"
        );

        let mut host = OverlayHost::default();
        host.register(Box::new(Declines(true)));
        assert_eq!(
            host.wheel(crate::input::WheelDelta::Lines(-1.0), 16.0),
            Handled::Consumed,
            "and a claim must stop there, or the grid scrolls too"
        );
    }
    use super::*;
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    fn key(text: &str) -> KeyInput {
        let k: Key = Key::Character(text.into());
        KeyInput {
            key: k,
            text: Some(text.to_string()),
            mods: ModifiersState::empty(),
            physical: crate::seam::NO_PHYSICAL,
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

    /// terminal-scroll R4.3/R11.3 — a handler that sends the user's bytes to the shell
    /// is detected at the **write boundary**, with the handler knowing nothing about
    /// scrollback. This is what makes Cmd+V paste snap the viewport without editing
    /// `clipboard.rs`, and what a keystroke-classification rule could never do: paste
    /// is consumed two dispatch steps before the default path ever runs.
    #[test]
    fn write_probe_sees_a_handler_write_without_the_handler_knowing() {
        let mut buf: Vec<u8> = Vec::new();
        let mut router = InputRouter::default();
        router.register(Box::new(Fixed(Handled::Consumed, true)));
        let mut probe = WriteProbe::new(&mut buf);
        let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
        assert_eq!(router.dispatch(&key("v"), &mut ctx), Handled::Consumed);
        assert!(probe.wrote(), "a paste-shaped write must be seen");
        assert_eq!(buf, b"X", "and must still reach the PTY unchanged");
    }

    /// The other half: a handler that consumes the key *without* writing — Cmd+C copy,
    /// an overlay toggle — must not trip the probe. Those are things a user does while
    /// reading history, and snapping there would fight them.
    #[test]
    fn write_probe_ignores_a_consuming_handler_that_writes_nothing() {
        let mut buf: Vec<u8> = Vec::new();
        let mut router = InputRouter::default();
        router.register(Box::new(Fixed(Handled::Consumed, false)));
        let mut probe = WriteProbe::new(&mut buf);
        let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
        assert_eq!(router.dispatch(&key("c"), &mut ctx), Handled::Consumed);
        assert!(
            !probe.wrote(),
            "consuming a key is not writing to the shell"
        );
        assert!(buf.is_empty());
    }

    /// A write that moved no bytes is not the user typing.
    #[test]
    fn write_probe_ignores_a_zero_byte_write() {
        let mut buf: Vec<u8> = Vec::new();
        let mut probe = WriteProbe::new(&mut buf);
        assert_eq!(probe.write(b"").unwrap(), 0);
        assert!(!probe.wrote());
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
        // Priority order, not registration order: the 100 was registered second.
        assert_eq!(host.priorities(), vec![100, 1]);
        // Named import kept meaningful; ensures NamedKey path compiles for callers.
        let _named: Key = Key::Named(NamedKey::Escape);
    }

    // ---- sticky-tab-title: the two new seam directions (R1.4/R3.4) ----

    #[test]
    fn app_ctx_records_tab_requests_in_order_and_drains_once() {
        // R1.4 — an overlay cannot hold `&mut Tabs` (the ctx already borrows the active
        // writer), so it records requests the app applies afterwards.
        let mut sink = Vec::new();
        let mut ctx = AppCtx::new(&mut sink, 3, 0, None);
        ctx.set_user_title(TabId(7), Some("Frontend".into()));
        ctx.close_tab(TabId(9));
        ctx.set_user_title(TabId(7), None);
        let reqs = ctx.take_requests();
        assert_eq!(
            reqs,
            vec![
                AppRequest::SetUserTitle(TabId(7), Some("Frontend".into())),
                AppRequest::CloseTab(TabId(9)),
                AppRequest::SetUserTitle(TabId(7), None),
            ],
            "order is preserved — a later set must win over an earlier one"
        );
        assert!(
            ctx.take_requests().is_empty(),
            "draining twice must not replay a close"
        );
    }

    #[test]
    fn app_ctx_records_a_monitor_toggle_through_the_same_deferred_path() {
        // ai-monitor-right-panel: the ai-monitor overlay cannot hold `&mut Renderer` for
        // the reason it cannot hold `&mut Tabs`, so the chord records its intent and
        // `terminal_app` applies it through `Renderer::set_monitor_open` — the one place
        // `sync_right_inset` is driven from (D21).
        //
        // What crosses is open-or-closed, never a width: the renderer derives the band from
        // `[ai_monitor].width_px` plus the runtime override, so this seam has no width to
        // carry and cannot become a second author of one (R10.4).
        let mut sink = Vec::new();
        let mut ctx = AppCtx::new(&mut sink, 1, 0, None);
        ctx.set_monitor_open(true);
        ctx.set_monitor_open(false); // the close half of the same chord
        assert_eq!(
            ctx.take_requests(),
            vec![
                AppRequest::SetMonitorOpen(true),
                AppRequest::SetMonitorOpen(false),
            ],
            "order is preserved — the last state recorded is the one that is applied"
        );
    }

    #[test]
    fn app_ctx_records_nothing_by_default() {
        // Every existing handler keeps working unchanged: no requests unless one is made.
        let mut sink = Vec::new();
        let mut ctx = AppCtx::new(&mut sink, 1, 0, None);
        assert!(ctx.take_requests().is_empty());
    }

    /// An overlay that answers `on_command` — the app→overlay direction (R3.4).
    /// `seen` is shared so a test can count how many overlays a command actually reached.
    struct Commandable {
        got: Option<OverlayCommand>,
        verdict: Handled,
        seen: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Overlay for Commandable {
        fn priority(&self) -> i32 {
            50
        }
        fn visible(&self) -> bool {
            false
        }
        fn on_key(&mut self, _in: &KeyInput, _c: &mut AppCtx) -> Handled {
            Handled::Passthrough
        }
        fn layer(&self) -> Box<dyn DrawLayer> {
            struct L;
            impl DrawLayer for L {
                fn z_order(&self) -> i32 {
                    0
                }
                fn record(&mut self, _: &mut renderer::FrameCtx<'_>) {}
            }
            Box::new(L)
        }
        fn on_command(&mut self, cmd: &OverlayCommand) -> Handled {
            self.got = Some(cmd.clone());
            self.seen.set(self.seen.get() + 1);
            self.verdict
        }
    }

    #[test]
    fn app_can_command_an_overlay() {
        // R3.4 — the direction that did not exist: the card menu's `Rename` must be able
        // to open the rename field on a chosen tab from a mouse press in `terminal_app`.
        let mut host = OverlayHost::default();
        host.register(Box::new(Commandable {
            got: None,
            verdict: Handled::Consumed,
            seen: std::rc::Rc::new(std::cell::Cell::new(0)),
        }));
        let cmd = OverlayCommand::BeginRename {
            tab: TabId(4),
            current: Some("Frontend".into()),
        };
        assert_eq!(host.command(&cmd), Handled::Consumed);
    }

    #[test]
    fn cancel_reaches_every_overlay_but_begin_rename_stops_at_the_first() {
        // `Cancel` means "close any open modal surface", so a higher-priority overlay
        // consuming it must not strand a field below. `BeginRename` is addressed to one
        // owner, so first-wins is right there.
        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut host = OverlayHost::default();
        for _ in 0..2 {
            host.register(Box::new(Commandable {
                got: None,
                verdict: Handled::Consumed,
                seen: seen.clone(),
            }));
        }
        assert_eq!(host.command(&OverlayCommand::Cancel), Handled::Consumed);
        assert_eq!(
            seen.get(),
            2,
            "Cancel fans out to every overlay, not just the first taker"
        );

        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut host = OverlayHost::default();
        for _ in 0..2 {
            host.register(Box::new(Commandable {
                got: None,
                verdict: Handled::Consumed,
                seen: seen.clone(),
            }));
        }
        let rename = OverlayCommand::BeginRename {
            tab: TabId(1),
            current: None,
        };
        assert_eq!(host.command(&rename), Handled::Consumed);
        assert_eq!(seen.get(), 1, "BeginRename stops at the first taker");
    }

    #[test]
    fn the_launch_restore_reaches_an_overlay_through_the_command_seam() {
        // ai-monitor-right-panel R6.6 — the restore's only route. `[ai_monitor].open` is
        // read by the app, which holds the config; the panel is opened by the overlay,
        // which holds the watcher. This command is the bridge, and it carries the active
        // tab's cwd + pid because at launch nothing else has handed them over yet.
        //
        // First-wins like `BeginRename` (it is addressed to one owner), not a fan-out.
        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut host = OverlayHost::default();
        for _ in 0..2 {
            host.register(Box::new(Commandable {
                got: None,
                verdict: Handled::Consumed,
                seen: seen.clone(),
            }));
        }
        let restore = OverlayCommand::RestoreMonitor {
            cwd: Some(PathBuf::from("/proj/a")),
            shell_pid: Some(8060),
        };
        assert_eq!(host.command(&restore), Handled::Consumed);
        assert_eq!(seen.get(), 1, "one panel per window, so one taker");
    }

    #[test]
    fn a_command_no_overlay_wants_is_passthrough() {
        // The default `on_command` is a no-op, so registering an existing overlay (the
        // ai-monitor) changes nothing — commands it does not understand fall through.
        let mut host = OverlayHost::default();
        host.register(Box::new(Ov {
            prio: 100,
            vis: true,
            verdict: Handled::Consumed, // consumes KEYS, but has no on_command
        }));
        assert_eq!(host.command(&OverlayCommand::Cancel), Handled::Passthrough);
    }
}
