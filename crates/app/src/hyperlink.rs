//! ⌘-hover link state (spec `hyperlinks`, D7, D19, D21, D22, D24).
//!
//! The rule the whole feature hangs on is D19: **the highlight is always current,
//! and a release opens exactly what is lit**. "Always current" is this module's job.
//! It is edge-triggered, never level-triggered — five events can change what is
//! under the pointer or what ⌘ is doing, and nothing else recomputes:
//!
//! | Trigger | Where | Why it exists |
//! |---|---|---|
//! | ⌘ down / up | `ModifiersChanged` | the affordance appears and disappears (D7) |
//! | pointer enters a new **cell** | `CursorMoved` | a different link (or none) is now under it |
//! | pointer leaves the window | `CursorLeft` | nothing is under it at all |
//! | a **fresh snapshot** while ⌘ is held | the `snapshot_dirty` branch of `RedrawRequested` | output moves the text with no pointer or modifier event (D24.4) |
//! | window focus lost | `Focused(false)` | macOS delivers `flagsChanged` only to the focused window (D24.5) |
//!
//! That last one guards a platform hazard that the pinned winit currently handles
//! for us: macOS delivers `flagsChanged` to the *view*, not the window delegate, so
//! a ⌘-Tab away with ⌘ held can leave a tracked modifier state that never sees its
//! key-up. winit 0.30.13 papers over it by calling `reset_modifiers()` and queueing
//! a synthetic `ModifiersChanged(empty)` immediately **before** `Focused(false)`
//! (`winit/src/platform_impl/macos/window_delegate.rs:225-227`), so in practice
//! `mods` is already clear by the time we disarm. The disarm is therefore
//! defence-in-depth, not a live bug being patched — it does not depend on winit
//! keeping that behaviour, and if the synthetic event ever stops arriving, a stale
//! `SUPER` would otherwise let a genuinely *plain* click satisfy both "⌘ at press"
//! and "a lit span under the pointer" and open a browser. It uses its own flag
//! rather than clearing the shared `mods` state, so the keyboard chord path is
//! untouched either way.
//!
//! The open half is [`url_to_open`] + [`Opener`]. The decision is a pure function
//! over four booleans-and-a-span, and the side effect sits behind an injectable sink
//! (the `CopyHandler` pattern), so `cargo test` never launches a browser.
//!
//! Everything here is a pure function or a plain struct over plain inputs — no
//! window, no GPU, no event loop — so the gates and the scan-count bounds are unit
//! testable directly.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};
use term_core::{link_at, GridSnapshot, LinkSpan};

/// Everything the hover rules need to decide. Assembled by the app at each trigger.
pub struct HoverInput<'a> {
    /// Is ⌘ currently held?
    pub cmd_held: bool,
    /// Is the pointer inside the window? Tracked explicitly because `last_cursor` is
    /// only ever written by `CursorMoved` and is *not* reset on `CursorLeft` — so
    /// `cursor_cell()` keeps returning a stale `Some` after the pointer leaves, and
    /// gating on it alone would light a link under an absent pointer (R2.8).
    pub pointer_inside: bool,
    /// The grid cell under the pointer, or `None` when the pointer is off the grid
    /// (over the sidebar, in the dead band, or outside).
    pub cell: Option<(u16, u16)>,
    /// The snapshot **on screen** — the app's `last_snapshot`, not a fresh clone.
    /// `None` before the first frame, when there is nothing to hit-test.
    pub snapshot: Option<&'a GridSnapshot>,
}

impl HoverInput<'_> {
    /// The scan gate (D21): all four conditions must hold before a scan is worth
    /// running. ⌘ up is the common case and costs nothing.
    ///
    /// Note what this does **not** claim: a ⌘ chord (Cmd+C/T/W/V/1-9) typed with the
    /// pointer resting over the grid *does* pass this gate and costs one scan of one
    /// row run. If a URL happens to be under the pointer it also lights up — which is
    /// correct under D7, because a ⌘-click right there would in fact open it.
    fn should_scan(&self) -> bool {
        self.cmd_held && self.pointer_inside && self.cell.is_some() && self.snapshot.is_some()
    }
}

/// Which of D24's edges is asking for a recompute. Distinct variants because they
/// carry different authority: only a modifier edge may re-arm the gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTrigger {
    /// `CursorMoved` — skipped entirely when the pointer stays in its cell.
    Pointer,
    /// `ModifiersChanged` — recomputes and re-arms.
    Modifier,
    /// A fresh snapshot in `RedrawRequested` — recomputes, never re-arms.
    Snapshot,
}

/// Should a `RedrawRequested` frame recompute the hover (trigger 4, D24.4)?
///
/// This is the idle-CPU rule, extracted so it is a *tested* rule rather than a
/// property of where a call happens to sit. The loop redraws continuously for cursor
/// blink, so the vast majority of frames take no fresh snapshot; only the ones that
/// re-cloned the grid can have moved the text under a stationary pointer. Gating on
/// ⌘ as well means streaming output with ⌘ up never reaches the scan at all.
///
/// Written as a predicate at the call site (rather than relying on the call being
/// nested inside the `snapshot_dirty` branch) precisely because nesting is invisible
/// to tests: moving the call one line would have silently scanned on every blink
/// frame with every test still green.
pub fn frame_rescans(took_fresh_snapshot: bool, cmd_held: bool) -> bool {
    took_fresh_snapshot && cmd_held
}

/// What the app should do with the link handle after a trigger.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// Nothing changed — do not touch the handle, do not request a frame.
    Unchanged,
    /// Write this value to the handle (`None` clears the affordance) and request a
    /// redraw. The redraw must be the **bare** `window.request_redraw()`: a hover
    /// change is not a content change, and setting `snapshot_dirty` would re-clone
    /// the grid on every ⌘ tap and defeat the idle-CPU cache (R3.6).
    Set(Option<LinkSpan>),
}

/// The app-side hover state machine.
#[derive(Default)]
pub struct HoverState {
    /// The cell the last scan ran against — the cell-granular guard that makes
    /// pixel-level pointer jitter free (R2.4), mirroring the drag path's own
    /// keyed-on-the-cell trick.
    last_cell: Option<(u16, u16)>,
    /// Whether a span is currently lit, so an already-clear state clears again for
    /// free instead of writing the handle and asking for a frame.
    lit: bool,
    /// Cleared by [`disarm`](Self::disarm) on focus loss; re-armed by the next
    /// modifier edge. Kept separate from `mods` so the keyboard path is untouched.
    armed: bool,
    /// How many times [`link_at`] has actually been called. Test-facing: the
    /// "no scan at idle / with ⌘ up / on sub-cell motion" requirements are counting
    /// claims, and this is what makes them assertable rather than aspirational.
    scans: u64,
}

impl HoverState {
    /// Recompute after a **pointer motion**. Skips the scan entirely when the
    /// pointer has not left its cell (R2.4).
    pub fn on_pointer(&mut self, input: HoverInput<'_>) -> Update {
        if input.cell == self.last_cell && self.armed {
            return Update::Unchanged;
        }
        self.resolve(input)
    }

    /// Recompute after a **modifier edge**. Always recomputes — the cell may be
    /// identical while the gate has changed — and **re-arms**, because a real
    /// `ModifiersChanged` is first-hand evidence of what ⌘ is doing.
    pub fn on_modifier(&mut self, input: HoverInput<'_>) -> Update {
        self.armed = true;
        self.resolve(input)
    }

    /// Recompute after a **fresh snapshot** (trigger 4). Always recomputes — the
    /// content under a stationary pointer has moved — but deliberately does **not**
    /// re-arm.
    ///
    /// The distinction is load-bearing. Sharing one arming entry point with
    /// [`on_modifier`] made the [`disarm`](Self::disarm) guard undo itself: after a
    /// focus loss that left `mods` stuck at `SUPER`, the very next PTY burst produced
    /// a fresh snapshot, re-armed, and relit the link — after which a genuinely plain
    /// click satisfied both halves of the open gesture. A snapshot says nothing about
    /// the keyboard, so it may not vouch for it.
    pub fn on_snapshot(&mut self, input: HoverInput<'_>) -> Update {
        if !self.armed {
            return Update::Unchanged;
        }
        self.resolve(input)
    }

    /// The pointer **left the window**: clear the affordance, stay armed (D24.3).
    ///
    /// Deliberately *not* [`disarm`](Self::disarm), which these two triggers shared
    /// at first. The window keeps focus when the pointer merely crosses its edge, so
    /// `flagsChanged` keeps arriving and ⌘ is not stale — there is nothing to disarm
    /// against. Sharing the disarm cost a real gesture: hold ⌘, let the pointer cross
    /// onto the menu bar and come back, and the link stayed dark and unclickable
    /// until the next modifier tap, because nothing but `on_modifier` re-arms.
    ///
    /// `last_cell` still resets, or [`on_pointer`](Self::on_pointer)'s
    /// same-cell early return would swallow the re-entry onto the very cell the
    /// pointer left from, and the link would not relight anyway.
    pub fn on_pointer_left(&mut self) -> Update {
        self.last_cell = None;
        self.clear()
    }

    /// The window **lost focus**: clear *and* disarm (D24.5).
    ///
    /// Disarming is what stops a stale ⌘ — one whose key-up was delivered to another
    /// window — from arming a later plain click. Only a real `ModifiersChanged` can
    /// re-arm, because only it is first-hand evidence about the keyboard.
    pub fn disarm(&mut self) -> Update {
        self.armed = false;
        self.last_cell = None;
        self.clear()
    }

    /// Total scans performed — test-only, because the counting requirements it
    /// serves ("no scan at idle / with ⌘ up / on sub-cell motion") are the only
    /// readers. The counter itself stays in release builds: it is one `u64`
    /// increment on a path that runs at most once per input edge.
    #[cfg(test)]
    pub fn scan_count(&self) -> u64 {
        self.scans
    }

    fn resolve(&mut self, input: HoverInput<'_>) -> Update {
        // Only remember a cell the pointer is actually over. `cursor_cell()` keeps
        // reporting the last position after `CursorLeft`, so recording it blindly let
        // a snapshot or modifier edge arriving *while the pointer is outside* refill
        // `last_cell` with that stale value — and `on_pointer`'s same-cell early
        // return then swallowed the re-entry onto exactly the cell the pointer had
        // left from, leaving the link dark. Same dead window as the one
        // `on_pointer_left` closes, reached one event later.
        self.last_cell = if input.pointer_inside {
            input.cell
        } else {
            None
        };
        if !self.should_scan_armed(&input) {
            return self.clear();
        }
        // Both `unwrap`-free: `should_scan` proved them `Some`.
        let (Some((col, row)), Some(snap)) = (input.cell, input.snapshot) else {
            return self.clear();
        };
        self.scans += 1;
        match link_at(snap, col, row) {
            Some(span) => {
                self.lit = true;
                Update::Set(Some(span))
            }
            None => self.clear(),
        }
    }

    fn should_scan_armed(&self, input: &HoverInput<'_>) -> bool {
        self.armed && input.should_scan()
    }

    fn clear(&mut self) -> Update {
        if !self.lit {
            return Update::Unchanged;
        }
        self.lit = false;
        Update::Set(None)
    }
}

/// Everything the open decision needs, sampled at the moment of the release.
pub struct ClickInput {
    /// Was ⌘ held when the button went **down**? Sampled at press so a ⌘ pressed
    /// *after* an ordinary click cannot turn it into an open (D22).
    pub cmd_at_press: bool,
    /// Is ⌘ still held **now**, at release? Releasing ⌘ mid-gesture cancels: the
    /// highlight is already gone by then (D7), and D19 opens only what is lit — so
    /// without this the user would watch the underline vanish and a browser open
    /// anyway.
    pub cmd_at_release: bool,
    /// Did the pointer ever leave the anchor cell? A real drag is a selection, not a
    /// click (D9) — the same `drag_moved` test the selection path already uses.
    pub drag_moved: bool,
    /// The span the link handle is lit with **right now**. This is the whole of D19:
    /// nothing is re-resolved here, so what opens is byte-identical to what the user
    /// can see underlined (D20).
    pub lit: Option<LinkSpan>,
}

/// The URL this release should open, or `None`.
///
/// Pure and total: every rejection path is a plain boolean, so the "a plain click
/// behaves exactly as today" constraint is one assertion rather than an argument.
pub fn url_to_open(input: ClickInput) -> Option<String> {
    if !input.cmd_at_press || !input.cmd_at_release || input.drag_moved {
        return None;
    }
    input.lit.map(|span| span.url)
}

/// The side effect: hand a URL to macOS (D14).
///
/// `NSWorkspace` rather than spawning `/usr/bin/open` — AppKit is already in the
/// tree for the pasteboard, it costs no process spawn, and it returns a real
/// failure flag. The call sits behind a sink so the decision logic above can be
/// tested without opening anything, exactly as `CopyHandler` does for the
/// pasteboard.
pub struct Opener {
    sink: Box<dyn Fn(&str)>,
}

impl Opener {
    pub fn new() -> Self {
        Opener {
            sink: Box::new(open_with_workspace),
        }
    }

    #[cfg(test)]
    fn with_sink(sink: Box<dyn Fn(&str)>) -> Self {
        Opener { sink }
    }

    pub fn open(&self, url: &str) {
        (self.sink)(url);
    }
}

impl Default for Opener {
    fn default() -> Self {
        Self::new()
    }
}

/// Ask macOS to open `url` in whatever app claims its scheme.
///
/// A failure — a URL AppKit will not parse, or no registered handler — is logged and
/// otherwise silent (D10): the grid is untouched, the highlight is untouched, and
/// nothing is written to the PTY. That matches how the rest of the app degrades
/// (shell integration fails the same way).
fn open_with_workspace(url: &str) {
    let ns = NSString::from_str(url);
    let Some(parsed) = NSURL::URLWithString(&ns) else {
        tracing::warn!("hyperlink: NSURL rejected {url:?}");
        return;
    };
    if !NSWorkspace::sharedWorkspace().openURL(&parsed) {
        tracing::warn!("hyperlink: no handler opened {url:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use term_core::{Color, Terminal};

    fn snap(text: &str) -> GridSnapshot {
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(text.as_bytes());
        t.snapshot()
    }

    /// A pointer at `cell` inside the window, with ⌘ optionally held.
    fn input<'a>(cmd: bool, cell: Option<(u16, u16)>, s: &'a GridSnapshot) -> HoverInput<'a> {
        HoverInput {
            cmd_held: cmd,
            pointer_inside: true,
            cell,
            snapshot: Some(s),
        }
    }

    fn lit_url(u: &Update) -> Option<&str> {
        match u {
            Update::Set(Some(span)) => Some(span.url.as_str()),
            _ => None,
        }
    }

    #[test]
    fn pressing_cmd_over_a_link_lights_it() {
        // R2.1.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        let u = h.on_modifier(input(true, Some((8, 0)), &s));
        assert_eq!(lit_url(&u), Some("https://example.com/a"));
        assert_eq!(h.scan_count(), 1);
    }

    #[test]
    fn releasing_cmd_clears_the_highlight_without_scanning() {
        // R2.2: the clear is a gate failure, so it costs no scan.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((8, 0)), &s));
        let before = h.scan_count();
        let u = h.on_modifier(input(false, Some((8, 0)), &s));
        assert_eq!(u, Update::Set(None));
        assert_eq!(h.scan_count(), before, "clearing does not scan");
    }

    #[test]
    fn moving_to_a_different_cell_with_cmd_held_recomputes() {
        // R2.3: link A → link B → no link.
        let s = snap("https://a.com/x and https://b.com/y");
        let mut h = HoverState::default();
        let a = h.on_modifier(input(true, Some((3, 0)), &s));
        assert_eq!(lit_url(&a), Some("https://a.com/x"));
        let b = h.on_pointer(input(true, Some((22, 0)), &s));
        assert_eq!(lit_url(&b), Some("https://b.com/y"));
        let none = h.on_pointer(input(true, Some((16, 0)), &s));
        assert_eq!(none, Update::Set(None), "between the two links");
    }

    #[test]
    fn motion_inside_the_same_cell_recomputes_nothing() {
        // R2.4: the cell-granular guard — three motion events, one scan.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((8, 0)), &s));
        let after_first = h.scan_count();
        assert_eq!(
            h.on_pointer(input(true, Some((8, 0)), &s)),
            Update::Unchanged
        );
        assert_eq!(
            h.on_pointer(input(true, Some((8, 0)), &s)),
            Update::Unchanged
        );
        assert_eq!(h.scan_count(), after_first, "no rescan within one cell");
    }

    #[test]
    fn a_fresh_snapshot_while_cmd_is_held_recomputes_the_highlight() {
        // R2.5 (D24.4): the pointer does not move, but output does. The same cell
        // now holds a different link, and the highlight follows it.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://a.com/x");
        let first = t.snapshot();
        let mut h = HoverState::default();
        assert_eq!(
            lit_url(&h.on_modifier(input(true, Some((3, 0)), &first))),
            Some("https://a.com/x")
        );
        // Output scrolls a different link under the stationary pointer. This is
        // trigger 4, so it goes through `on_snapshot` — the non-arming entry point.
        t.advance(b"\r\nhttps://b.com/y");
        let second = t.snapshot();
        let u = h.on_snapshot(input(true, Some((3, 1)), &second));
        assert_eq!(lit_url(&u), Some("https://b.com/y"));
    }

    #[test]
    fn a_fresh_snapshot_that_removes_the_link_clears_it() {
        // R2.5, the other half: nothing is under the pointer any more.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://a.com/x");
        let first = t.snapshot();
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((3, 0)), &first));
        t.advance(b"\x1b[2J\x1b[H"); // clear screen
        let cleared = t.snapshot();
        assert_eq!(
            h.on_snapshot(input(true, Some((3, 0)), &cleared)),
            Update::Set(None)
        );
    }

    #[test]
    fn no_scan_ever_runs_with_cmd_up() {
        // R2.10: motion all over the grid with ⌘ up costs nothing.
        let s = snap("https://a.com/x and https://b.com/y");
        let mut h = HoverState::default();
        for col in 0..30u16 {
            assert_eq!(
                h.on_pointer(input(false, Some((col, 0)), &s)),
                Update::Unchanged
            );
        }
        assert_eq!(h.scan_count(), 0);
    }

    #[test]
    fn the_pointer_outside_the_window_never_lights_a_link() {
        // R2.8: `cell` is a stale `Some` (last_cursor is never reset), so only the
        // explicit `pointer_inside` flag can stop this.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        let u = h.on_modifier(HoverInput {
            cmd_held: true,
            pointer_inside: false,
            cell: Some((8, 0)),
            snapshot: Some(&s),
        });
        assert_eq!(u, Update::Unchanged, "nothing lit, nothing to clear");
        assert_eq!(h.scan_count(), 0, "and no scan ran");
    }

    #[test]
    fn leaving_the_window_clears_a_lit_link() {
        // R2.8 / CursorLeft.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((8, 0)), &s));
        assert_eq!(h.on_pointer_left(), Update::Set(None));
        assert_eq!(
            h.on_pointer_left(),
            Update::Unchanged,
            "already clear → no frame"
        );
    }

    #[test]
    fn the_pointer_leaving_and_returning_relights_without_a_modifier_tap() {
        // Regression (Gate-3 checker, iteration 3): `CursorLeft` used to share the
        // focus-loss disarm, so crossing the window edge with ⌘ held and coming back
        // left the link dark and unclickable until the next modifier tap. D24.3 says
        // `CursorLeft` *clears* — the window keeps focus, so ⌘ is not stale and there
        // is nothing to disarm against.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        assert!(lit_url(&h.on_modifier(input(true, Some((8, 0)), &s))).is_some());
        assert_eq!(h.on_pointer_left(), Update::Set(None));
        // The pointer comes back onto the *same* cell it left from — the case the
        // same-cell early return would swallow if `last_cell` had not been reset.
        assert_eq!(
            lit_url(&h.on_pointer(input(true, Some((8, 0)), &s))),
            Some("https://example.com/a"),
            "re-entry relights with no modifier edge"
        );
        // A focus loss still disarms, and a return does NOT relight until a real
        // modifier edge arrives — the two triggers stay distinct.
        assert_eq!(h.disarm(), Update::Set(None));
        assert_eq!(
            h.on_pointer(input(true, Some((8, 0)), &s)),
            Update::Unchanged
        );
    }

    #[test]
    fn an_edge_while_the_pointer_is_outside_does_not_wedge_the_re_entry() {
        // Regression (Gate-3 checker, iteration 4): the same dead window, reached one
        // event later. `cursor_cell()` still reports the last position after
        // `CursorLeft`, so a snapshot or modifier edge arriving while the pointer is
        // away used to refill `last_cell` with that stale cell — and the same-cell
        // early return then swallowed the re-entry onto exactly that cell.
        let s = snap("see https://example.com/a now");
        for label in ["snapshot", "modifier"] {
            let mut h = HoverState::default();
            h.on_modifier(input(true, Some((8, 0)), &s));
            assert_eq!(h.on_pointer_left(), Update::Set(None));
            // An edge fires while the pointer is still outside, carrying the stale
            // cell exactly as `cursor_cell()` would report it.
            let outside = HoverInput {
                cmd_held: true,
                pointer_inside: false,
                cell: Some((8, 0)),
                snapshot: Some(&s),
            };
            let u = match label {
                "snapshot" => h.on_snapshot(outside),
                _ => h.on_modifier(outside),
            };
            assert_eq!(u, Update::Unchanged, "{label} edge lights nothing outside");
            // …and the pointer returns to the cell it left from.
            assert_eq!(
                lit_url(&h.on_pointer(input(true, Some((8, 0)), &s))),
                Some("https://example.com/a"),
                "re-entry after a {label} edge must still relight"
            );
        }
    }

    #[test]
    fn losing_focus_disarms_so_a_later_plain_click_finds_nothing_lit() {
        // R2.7 (D24.5): the hazard this guards is a focus loss that delivers no
        // ⌘-up, leaving `cmd_held` true afterwards. winit 0.30.13 synthesises one for
        // us, so this asserts the defence-in-depth directly rather than through the
        // event loop: with `cmd_held` still true, disarming must stop the next
        // pointer motion from relighting — otherwise a plain click would open.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((8, 0)), &s));
        assert_eq!(h.disarm(), Update::Set(None));
        // Pointer moves again; ⌘ still reads as held, but nothing relights.
        assert_eq!(
            h.on_pointer(input(true, Some((9, 0)), &s)),
            Update::Unchanged
        );
        assert_eq!(h.scan_count(), 1, "no scan while disarmed");
        // The next real modifier edge re-arms it.
        assert!(lit_url(&h.on_modifier(input(true, Some((9, 0)), &s))).is_some());
    }

    #[test]
    fn a_snapshot_edge_does_not_re_arm_after_a_focus_loss() {
        // Regression (Gate-3 checker, iteration 2): the disarm used to undo itself.
        // Trigger 4 shared one arming entry point with the modifier edge, so after a
        // focus loss that left ⌘ reading as held, the very next PTY burst re-armed and
        // relit the link — and a genuinely plain click would then have opened it. A
        // snapshot says nothing about the keyboard, so it may not vouch for it.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://a.com/x");
        let first = t.snapshot();
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((3, 0)), &first));
        assert_eq!(h.disarm(), Update::Set(None));

        // Output arrives; ⌘ still reads as held; the pointer has not moved.
        t.advance(b"\r\nhttps://b.com/y");
        let second = t.snapshot();
        assert_eq!(
            h.on_snapshot(input(true, Some((3, 1)), &second)),
            Update::Unchanged,
            "a snapshot edge must not re-arm a disarmed gesture"
        );

        // A real modifier edge is first-hand evidence, and does re-arm.
        assert!(lit_url(&h.on_modifier(input(true, Some((3, 1)), &second))).is_some());
    }

    #[test]
    fn a_snapshot_edge_still_recomputes_while_armed() {
        // The other half: not re-arming must not mean not recomputing.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://a.com/x");
        let first = t.snapshot();
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((3, 0)), &first));
        t.advance(b"\r\nhttps://b.com/y");
        let second = t.snapshot();
        assert_eq!(
            lit_url(&h.on_snapshot(input(true, Some((3, 1)), &second))),
            Some("https://b.com/y")
        );
    }

    #[test]
    fn a_pointer_off_the_grid_lights_nothing() {
        // The sidebar / dead-band case: `cursor_cell()` yields `None`.
        let s = snap("see https://example.com/a now");
        let mut h = HoverState::default();
        assert_eq!(h.on_modifier(input(true, None, &s)), Update::Unchanged);
        assert_eq!(h.scan_count(), 0);
    }

    #[test]
    fn no_snapshot_yet_lights_nothing() {
        // The first-frame case: `last_snapshot` is `None`, so there is nothing on
        // screen to hit-test.
        let mut h = HoverState::default();
        let u = h.on_modifier(HoverInput {
            cmd_held: true,
            pointer_inside: true,
            cell: Some((8, 0)),
            snapshot: None,
        });
        assert_eq!(u, Update::Unchanged);
        assert_eq!(h.scan_count(), 0);
    }

    #[test]
    fn one_edge_costs_at_most_one_scan_of_one_row_run() {
        // R2.9: the bound is structural — `link_at` examines only the hovered row's
        // soft-wrapped run — so what this asserts is the *call* bound: one edge,
        // one call, on a tall grid full of links.
        let mut t = Terminal::new(40, 40, Color(0xE0E0E0), Color(0x101010));
        for _ in 0..40 {
            t.advance(b"https://example.com/x\r\n");
        }
        let s = t.snapshot();
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((3, 20)), &s));
        assert_eq!(h.scan_count(), 1);
    }

    #[test]
    fn an_idle_frame_never_rescans() {
        // R2.6 (Gate-3 checker's push-back): the continuous cursor-blink redraw takes
        // no fresh snapshot, so it must not scan — with ⌘ held or not. Before this
        // predicate the rule lived only in where the call sat, and moving that call
        // one line would have scanned on every blink frame with every test green.
        assert!(!frame_rescans(false, true), "idle frame, ⌘ held");
        assert!(!frame_rescans(false, false), "idle frame, ⌘ up");
        // …and streaming output with ⌘ up is equally free, even though it does take
        // fresh snapshots.
        assert!(!frame_rescans(true, false), "fresh snapshot, ⌘ up");
        // Only the one combination that can actually have moved a lit link.
        assert!(frame_rescans(true, true), "fresh snapshot, ⌘ held");
    }

    #[test]
    fn streaming_output_with_cmd_up_costs_no_scans() {
        // R5.1: the idle-CPU constraint, on the trigger that could most easily
        // violate it. Trigger 4 fires on every fresh snapshot — i.e. on every PTY
        // burst — so if the ⌘ gate did not come first, a `yes`-style flood would
        // scan on each one. This is the regression this feature was most at risk of
        // reintroducing, so it is asserted rather than assumed.
        let mut t = Terminal::new(40, 6, Color(0xE0E0E0), Color(0x101010));
        let mut h = HoverState::default();
        // Arm it first, so a zero scan count is attributable to the ⌘ gate rather
        // than to the state merely sitting disarmed.
        t.advance(b"https://example.com/seed\r\n");
        h.on_modifier(input(true, Some((3, 0)), &t.snapshot()));
        // ⌘ goes up: one clearing update, no scan. Everything after this must be
        // completely inert.
        assert_eq!(
            h.on_snapshot(input(false, Some((3, 0)), &t.snapshot())),
            Update::Set(None)
        );
        let armed_scans = h.scan_count();
        for i in 0..200 {
            t.advance(format!("line {i} https://example.com/{i}\r\n").as_bytes());
            let s = t.snapshot();
            assert_eq!(
                h.on_snapshot(input(false, Some((12, 2)), &s)),
                Update::Unchanged
            );
        }
        assert_eq!(
            h.scan_count(),
            armed_scans,
            "⌘ up → not one further scan in 200 snapshots"
        );
    }

    // --- the open half (R0, R4) ---

    fn span_of(s: &GridSnapshot, col: u16, row: u16) -> LinkSpan {
        link_at(s, col, row).expect("a link is there")
    }

    /// A release with ⌘ held throughout, no drag, and `lit` under the pointer.
    fn click(lit: Option<LinkSpan>) -> ClickInput {
        ClickInput {
            cmd_at_press: true,
            cmd_at_release: true,
            drag_moved: false,
            lit,
        }
    }

    #[test]
    fn cmd_click_on_a_lit_link_opens_it() {
        // R0.1 / R4.1.
        let s = snap("see https://example.com/a now");
        let url = url_to_open(click(Some(span_of(&s, 8, 0))));
        assert_eq!(url.as_deref(), Some("https://example.com/a"));
    }

    #[test]
    fn the_opened_string_is_the_lit_span_verbatim() {
        // R4.2 / D20: nothing is re-derived at release — a trimmed and a wrapped span
        // both open exactly the characters that were underlined.
        let trimmed = snap("(see https://example.com/a/12).");
        let span = span_of(&trimmed, 10, 0);
        assert_eq!(
            url_to_open(click(Some(span.clone()))).as_deref(),
            Some(span.url.as_str())
        );

        let mut t = Terminal::new(18, 4, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://example.com/pull/9 done");
        let wrapped = t.snapshot();
        let span = span_of(&wrapped, 3, 0);
        assert_eq!(span.end.1, 1, "the span really does wrap");
        assert_eq!(
            url_to_open(click(Some(span.clone()))).as_deref(),
            Some(span.url.as_str())
        );
    }

    #[test]
    fn releasing_cmd_before_the_button_cancels_the_open() {
        // R4.3: by then the highlight is gone (D7), so opening would contradict what
        // the user can see.
        let s = snap("see https://example.com/a now");
        let lit = Some(span_of(&s, 8, 0));
        assert_eq!(
            url_to_open(ClickInput {
                cmd_at_release: false,
                ..click(lit)
            }),
            None
        );
    }

    #[test]
    fn pressing_cmd_after_the_button_does_not_open() {
        // R4.4: the press-time sample is what stops a ⌘ tapped mid-click from
        // retroactively turning an ordinary click into an open.
        let s = snap("see https://example.com/a now");
        let lit = Some(span_of(&s, 8, 0));
        assert_eq!(
            url_to_open(ClickInput {
                cmd_at_press: false,
                ..click(lit)
            }),
            None
        );
    }

    #[test]
    fn cmd_drag_selects_and_opens_nothing() {
        // R4.5: a drag is a selection even with ⌘ down (D9).
        let s = snap("see https://example.com/a now");
        let lit = Some(span_of(&s, 8, 0));
        assert_eq!(
            url_to_open(ClickInput {
                drag_moved: true,
                ..click(lit)
            }),
            None
        );
    }

    #[test]
    fn a_plain_click_never_opens() {
        // R4.6 / R6.2: no ⌘ at any point, and nothing lit — the untouched path.
        let s = snap("see https://example.com/a now");
        assert_eq!(
            url_to_open(ClickInput {
                cmd_at_press: false,
                cmd_at_release: false,
                drag_moved: false,
                lit: None,
            }),
            None
        );
        // Even if a stale span were somehow lit, no ⌘ means no open.
        assert_eq!(
            url_to_open(ClickInput {
                cmd_at_press: false,
                cmd_at_release: false,
                drag_moved: false,
                lit: Some(span_of(&s, 8, 0)),
            }),
            None
        );
    }

    #[test]
    fn a_release_over_nothing_lit_opens_nothing() {
        // R4.7: ⌘ held over ordinary text.
        assert_eq!(url_to_open(click(None)), None);
    }

    #[test]
    fn output_that_relights_a_different_link_opens_the_second_one() {
        // R4.8: the specified consequence of "open what is lit" rather than "open
        // what was captured at press". The pointer never moves; output does, D24.4
        // relights, and the release acts on the new span.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://a.com/x");
        let first = t.snapshot();
        let mut h = HoverState::default();
        h.on_modifier(input(true, Some((3, 0)), &first));

        t.advance(b"\r\nhttps://b.com/y");
        let second = t.snapshot();
        let relit = match h.on_snapshot(input(true, Some((3, 1)), &second)) {
            Update::Set(span) => span,
            Update::Unchanged => panic!("the snapshot edge must relight"),
        };
        assert_eq!(
            url_to_open(click(relit)).as_deref(),
            Some("https://b.com/y")
        );
    }

    #[test]
    fn the_open_goes_through_an_injectable_sink() {
        // R4.9: `cargo test` must never launch a browser.
        let seen = Rc::new(RefCell::new(Vec::new()));
        let rec = seen.clone();
        let opener = Opener::with_sink(Box::new(move |u| rec.borrow_mut().push(u.to_string())));
        opener.open("https://example.com/a");
        assert_eq!(seen.borrow().as_slice(), ["https://example.com/a"]);
    }

    #[test]
    fn the_decision_cannot_depend_on_the_mouse_protocol() {
        // R4.11 (D17): links stay live inside a mouse-reporting TUI. Structurally —
        // `ClickInput` has no mouse-protocol field to consult, and the app encodes
        // no button reports at all (only `wheel_bytes` exists), so a ⌘-click can
        // neither be suppressed by nor written to the running application.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"\x1b[?1003h\x1b[?1006h"); // any-event tracking, SGR encoding
        t.advance(b"https://a.com/x");
        assert_ne!(
            t.mouse_protocol(),
            term_core::MouseProtocol::Off,
            "the app really has mouse reporting on"
        );
        let s = t.snapshot();
        assert_eq!(
            url_to_open(click(Some(span_of(&s, 3, 0)))).as_deref(),
            Some("https://a.com/x")
        );
    }

    #[test]
    fn non_allow_listed_schemes_are_never_lit_and_so_never_open() {
        // R6.3, end to end: hovering `file://` and `mailto:` with ⌘ held lights
        // nothing, so the release has nothing to open.
        let s = snap("file:///Users/me/.ssh/id_rsa mailto:a@b.com");
        let mut h = HoverState::default();
        for col in 0..40u16 {
            assert_eq!(
                h.on_modifier(input(true, Some((col, 0)), &s)),
                Update::Unchanged
            );
        }
        assert_eq!(url_to_open(click(None)), None);
    }
}
