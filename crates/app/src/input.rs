//! Keyboard translation: tab chords (D5, R3.2) and shell key encoding.
//!
//! Both are pure functions over winit's logical `Key` + `ModifiersState`, so the
//! chord table is unit-tested without a window.

use renderer::sidebar_layout::{self, ChromeRegion, SidebarLayoutInput, SidebarRegion};
use term_core::MouseEncoding;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

/// A tab-management action triggered by a Cmd chord (D5). Intercepted before
/// `encode_key`, so these keys never reach the shell (R3.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabAction {
    New,
    CloseActive,
    Next,
    Prev,
    /// Jump to tab index (0-based; Cmd+1 → 0).
    Jump(usize),
}

/// Map a keypress to a tab action, or `None` if it isn't a tab chord. macOS
/// chords use Cmd (the `super` modifier): Cmd+T/W, Cmd+Shift+[ / ], Cmd+1..9.
///
/// Bracket vs. brace is accepted either way (`[`/`{`, `]`/`}`) so the chord works
/// whether or not Shift alters the produced character across layouts.
pub fn tab_action(key: &Key, mods: &ModifiersState) -> Option<TabAction> {
    if !mods.super_key() {
        return None;
    }
    let Key::Character(s) = key else {
        return None;
    };
    let c = s.chars().next()?;
    match c {
        't' | 'T' => Some(TabAction::New),
        'w' | 'W' => Some(TabAction::CloseActive),
        '[' | '{' => Some(TabAction::Prev),
        ']' | '}' => Some(TabAction::Next),
        '1'..='9' => Some(TabAction::Jump(c as usize - '1' as usize)),
        _ => None,
    }
}

/// What the app does about a pointer press (D1). Distinct from the renderer's *region*
/// enum: several regions map to one action, and two map to "consumed" (`Inert`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidebarAction {
    /// Switch to the card at this list index.
    Switch(usize),
    /// Switch to the session behind waiting-band row `i` (D12/R2.1). The renderer's
    /// hit-test only knows the band-row index; the app resolves it to a `TabId` → tab
    /// index (the band's order differs from the tab order), then switches — which
    /// acknowledges and clears the item.
    SwitchWaiting(usize),
    NewTab,
    Collapse,
    Expand,
    /// Open the card menu for the card at this list index (sticky-tab-title R3.3) — the
    /// hamburger was pressed. The app resolves the index to a `TabId` and decides the
    /// item set (`Reset to auto` only for a tab that has a user title, D4).
    OpenMenu(usize),
    /// Activate item `i` of the currently open card menu (sticky-tab-title R3.3). The
    /// renderer knows only the item's position; which action that position means is the
    /// app's to decide, since the app chose the item set.
    MenuItem(usize),
    /// Begin a drag-to-resize on the sidebar's right edge (tab-resize D3/R1.7). The press
    /// landed in the grab band; the app arms the drag and owns the pointer from here.
    BeginResize,
    Inert,
}

/// A wheel delta in one of winit 0.30's two unit systems (D10) — a small app-side enum so
/// the translation is testable without constructing a winit event.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WheelDelta {
    /// `MouseScrollDelta::LineDelta` — abstract lines; one line is one card (D10).
    Lines(f32),
    /// `MouseScrollDelta::PixelDelta` — already physical px (trackpad).
    Pixels(f32),
}

/// Resolve a left-button press to an app action through the sibling's ONE ordered
/// `hit_test` (D1) — the app performs no geometry (SD2). `Panel` (sidebar dead space) and
/// the inert filter are swallowed as `Inert`; `None` (over the grid) stays `None`.
pub fn sidebar_press(
    cursor: (f64, f64),
    inp: &SidebarLayoutInput,
    scroll: f32,
    collapsed: bool,
) -> Option<SidebarAction> {
    let p = [cursor.0 as f32, cursor.1 as f32];
    sidebar_layout::hit_test(p, scroll, inp, collapsed).map(|region| match region {
        SidebarRegion::Card(i) => SidebarAction::Switch(i),
        SidebarRegion::WaitingRow(i) => SidebarAction::SwitchWaiting(i),
        SidebarRegion::MenuButton(i) => SidebarAction::OpenMenu(i),
        SidebarRegion::MenuItem(i) => SidebarAction::MenuItem(i),
        SidebarRegion::Chrome(ChromeRegion::NewTab) => SidebarAction::NewTab,
        SidebarRegion::Chrome(ChromeRegion::HideSidebar) => SidebarAction::Collapse,
        SidebarRegion::Chrome(ChromeRegion::Restore) => SidebarAction::Expand,
        SidebarRegion::GrabEdge => SidebarAction::BeginResize,
        SidebarRegion::Panel => SidebarAction::Inert,
    })
}

/// An in-flight drag-to-resize of the sidebar's right edge (tab-resize D9/D17).
///
/// The `grab_offset` is captured at press and **never re-derived** — that is what lets the
/// gesture survive D12's mid-drag collapse, where there is no edge left to measure against.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ResizeDrag {
    /// `pointer_x − edge_x` at the moment of the press, **physical** px. Fixed for the
    /// whole gesture, so the edge stays welded to the point the user grabbed.
    pub grab_offset: f32,
}

/// Is this the **Cmd+Shift+S** sidebar toggle chord (tab-resize D17a)?
///
/// Chosen against the live inventory: `tab_action`'s Cmd+T/W/[/]/1-9, Cmd+C (copy),
/// Cmd+V (paste), Cmd+Shift+M (ai-monitor), Cmd+Shift+R (rename) — and it joins the
/// `Cmd+Shift+<letter>` family both existing chrome toggles already use.
///
/// Must be tested BEFORE `encode_key`: that function swallows `super` only for its
/// `key_seq` table, so a `Key::Character` chord otherwise reaches zsh via the text
/// fallback. Case-insensitive because Shift alters the produced character.
pub fn is_sidebar_toggle_chord(key: &Key, mods: &ModifiersState) -> bool {
    if !(mods.super_key() && mods.shift_key()) {
        return false;
    }
    matches!(key, Key::Character(s) if s.eq_ignore_ascii_case("s"))
}

/// What a delivered config means for the sidebar (tab-resize D15/D15a).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigDelivery {
    /// Our own write coming back through the watcher — drop it entirely.
    Echo,
    /// A real change from outside this app. Apply it; the file wins.
    HumanEdit,
}

/// Whether a delivered config should be dropped without applying it (tab-resize D15, as
/// amended) — **the predicate the event loop actually uses**.
///
/// Suppress only when *nothing* changed. The first cut compared only the
/// `(width_px, collapsed)` pair and was catastrophic: the pair latch is armed on every
/// successful persist and the file then matches it indefinitely, so every later reload of
/// every unrelated field was silently dropped — one click of Hide killed live settings for
/// the rest of the session.
///
/// Extracted as a free function purely so that behaviour is testable; the event loop is
/// otherwise unreachable without a window.
pub fn suppress_delivery(
    delivered: &common::config::Config,
    applied: &common::config::Config,
) -> bool {
    delivered == applied
}

/// How many recent writes are remembered for echo classification.
///
/// More than one is load-bearing: FSEvents delivers asynchronously, so two persists inside
/// one round-trip window (a drag released, then a second drag) leave the FIRST write's echo
/// still in flight while the latch already holds the second. Matching only the newest write
/// classified that stale echo as a human edit — which cleared the override and snapped the
/// sidebar back to the older width before the second echo corrected it, a visible flicker
/// plus two of the glyph-atlas rebuilds D15 exists to prevent.
/// **Known trade-off** (recorded rather than discovered later): this widens D15's "the last
/// value written" to "any of the last four". A hand edit that happens to set the width to a
/// value dragged through earlier in the same session is therefore misread as an echo, so the
/// override is not cleared and the edit has no visible effect until the ring rolls over.
/// Narrow, and strictly better than the flicker it replaces — but real.
pub const RECENT_WRITES: usize = 4;

/// Classify a delivered config's sidebar pair against the values we recently wrote
/// (tab-resize D15) — this decides **override precedence** (D15a), not suppression;
/// [`suppress_delivery`] owns that.
///
/// **Exact match, latched.** Not "differs only in those fields": under that reading a
/// deliberate hand edit to `sidebar.width_px` alone would be dropped forever, killing live
/// reload for the one field this feature owns. And latched rather than one-shot, because
/// FSEvents can deliver a single rename more than once — a "consume the first match" rule
/// would treat the duplicate as a human edit and clear the override after every drag.
///
/// The float comparison is on the **re-parsed** value, which is exact only because the
/// writer emits a round-trip representation (`config::save_sidebar`, R8.7).
pub fn classify_delivery(delivered: (f32, bool), recent: &[(f32, bool)]) -> ConfigDelivery {
    if recent.contains(&delivered) {
        ConfigDelivery::Echo
    } else {
        ConfigDelivery::HumanEdit
    }
}

/// Push a successful write onto the recent-writes ring, capped at [`RECENT_WRITES`].
pub fn remember_write(recent: &mut Vec<(f32, bool)>, pair: (f32, bool)) {
    recent.retain(|p| *p != pair);
    recent.push(pair);
    while recent.len() > RECENT_WRITES {
        recent.remove(0);
    }
}

/// Whether a human edit should clear the runtime sidebar override (tab-resize D15a/D14).
///
/// Normally yes — that is what keeps `config.toml` genuinely hand-editable after the first
/// drag. The exception is a **failed** write: the file and the override are then knowingly
/// out of step, and clearing would snap the sidebar back mid-session on the next unrelated
/// reload, breaking D14's promise that the session keeps the dragged size.
pub fn should_clear_override(delivery: ConfigDelivery, override_authoritative: bool) -> bool {
    delivery == ConfigDelivery::HumanEdit && !override_authoritative
}

/// Whether a width change actually changed the terminal's grid, and therefore whether the
/// drag must call `settings::refit` (tab-resize D13/R5.1).
///
/// Deliberately evaluated at the **drag call site**, never inside `refit` itself: `refit`
/// is shared with startup, `Resized`, and config reload (D6), and moving the guard in
/// there would silently change their behaviour too — including stopping a no-op window
/// resize from clearing the selection, which is a sibling feature's business, not ours.
pub fn needs_refit(before: (u16, u16), after: (u16, u16)) -> bool {
    before != after
}

/// Which in-flight gesture a left-release belongs to (tab-resize D17/R7.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseTarget {
    /// End a sidebar resize: commit the width and persist it.
    Resize,
    /// End a text selection (selection-input's pre-existing behavior).
    Selection,
    /// Nothing armed — the release is not ours.
    Neither,
}

/// Resolve a left-release to exactly one gesture (tab-resize D17/R7.3).
///
/// The event loop's two `Released` arms guard on this rather than relying on their
/// **position** in the `match`. That is deliberate: Rust runs exactly one arm, so if the
/// precedence lived only in the source order, a future reorder would silently change which
/// gesture gets its cleanup — and a `||`-widened single guard would swallow the selection
/// release entirely whenever both were armed, leaving `dragging` stuck `true` forever.
///
/// Resize wins when both are somehow armed. D17's invariant says that cannot happen, but
/// nothing clears `dragging` on `Focused(false)`, so a lost `Released` can leave it stale —
/// and the resize is the gesture holding an uncommitted config write.
pub fn release_target(resize_armed: bool, selection_armed: bool) -> ReleaseTarget {
    if resize_armed {
        ReleaseTarget::Resize
    } else if selection_armed {
        ReleaseTarget::Selection
    } else {
        ReleaseTarget::Neither
    }
}

/// The grab offset to capture on a press (tab-resize D9). Both arguments are physical px.
pub fn resize_grab_offset(pointer_x: f64, edge_x: f32) -> f32 {
    pointer_x as f32 - edge_x
}

/// The drag's **raw**, unclamped width in *logical* px (tab-resize D9/D9a).
///
/// This is the value D12's collapse/re-expand thresholds compare against — NOT the clamped
/// width. Clamping first would pin the value at `SIDEBAR_WIDTH_MIN`, making a threshold
/// below that minimum unreachable and the collapse gesture dead code (D9a).
pub fn resize_raw_width(pointer_x: f64, drag: ResizeDrag, scale: f64) -> f32 {
    let physical = pointer_x as f32 - drag.grab_offset;
    // Logical px, because `SIDEBAR_WIDTH_MIN`/`MAX` and `SidebarConfig::width_px` are
    // logical (D7) — persisting physical px would make the file display-dependent.
    let scale = if scale > 0.0 { scale as f32 } else { 1.0 };
    physical / scale
}

/// How far **below** [`SIDEBAR_WIDTH_MIN`] the raw width must fall before a drag collapses
/// the sidebar (tab-resize D12), and therefore the width of the hysteresis gap.
///
/// [`SIDEBAR_WIDTH_MIN`]: common::config::SIDEBAR_WIDTH_MIN
pub const COLLAPSE_HYSTERESIS_PX: f32 = 40.0;

/// The raw width at or below which a drag collapses the sidebar (tab-resize D12).
pub fn collapse_threshold() -> f32 {
    common::config::SIDEBAR_WIDTH_MIN - COLLAPSE_HYSTERESIS_PX
}

/// The collapsed state a drag should be in, given its **raw** (unclamped) width and where
/// it is now (tab-resize D12).
///
/// Two different thresholds, deliberately: collapse at `MIN − 40`, re-expand at `MIN`. A
/// single threshold would flip the mode on ordinary hand jitter, and under D1 each flip is
/// a full-window grid reflow. The gesture stays live across the change — dragging back
/// right re-expands without releasing (R4.2).
///
/// Reads `raw`, never the clamped width: `clamp(raw, 260, 440)` can never be below 260, so
/// a threshold at 220 tested against it would be unreachable dead code (D9a/R4.4).
pub fn next_collapsed(raw: f32, collapsed: bool) -> bool {
    if collapsed {
        // Re-expand only once the pointer is back at the honest minimum.
        raw < common::config::SIDEBAR_WIDTH_MIN
    } else {
        raw < collapse_threshold()
    }
}

/// The width actually applied and persisted: the raw value clamped to the supported band
/// (tab-resize D9a). Non-finite input falls back to the low end rather than propagating a
/// `NaN` into the renderer's uniforms.
pub fn clamp_sidebar_width(raw: f32) -> f32 {
    if !raw.is_finite() {
        return common::config::SIDEBAR_WIDTH_MIN;
    }
    raw.clamp(
        common::config::SIDEBAR_WIDTH_MIN,
        common::config::SIDEBAR_WIDTH_MAX,
    )
}

/// The ONE `hit_test` call the hover path makes (tab-resize D11). `refresh_hover` calls
/// this once per `CursorMoved` and feeds the result to both [`hover_card`] and
/// [`hover_cursor`] — never calling `hit_test` twice, which is the drift SD2 forbids.
pub fn sidebar_region(
    cursor: (f64, f64),
    inp: &SidebarLayoutInput,
    scroll: f32,
    collapsed: bool,
) -> Option<SidebarRegion> {
    let p = [cursor.0 as f32, cursor.1 as f32];
    sidebar_layout::hit_test(p, scroll, inp, collapsed)
}

/// The card index a resolved region highlights (D8) — the pure hover mapping.
pub fn hover_card(region: Option<SidebarRegion>) -> Option<usize> {
    match region {
        Some(SidebarRegion::Card(i)) => Some(i),
        // The hamburger sits ON a card and resolves ahead of it (sticky-tab-title R3.1),
        // so without this arm the card would lose its hover highlight the moment the
        // pointer reached the very control that card is offering.
        Some(SidebarRegion::MenuButton(i)) => Some(i),
        _ => None,
    }
}

/// The mouse cursor a resolved region asks for (tab-resize D11/R2.1).
///
/// The grab band is the terminal's only resize affordance — D10 draws no grip — so this
/// mapping is the whole of its discoverability.
pub fn hover_cursor(region: Option<SidebarRegion>) -> CursorIcon {
    match region {
        Some(SidebarRegion::GrabEdge) => CursorIcon::ColResize,
        _ => CursorIcon::Default,
    }
}

/// Resolve the card index under the cursor for the row layer's `hovered` highlight (D8) —
/// `hover_card ∘ sidebar_region`, kept for tests that assert hover *semantics* in one call.
///
/// Production no longer calls this: `refresh_hover` resolves the region ONCE and feeds
/// both [`hover_card`] and [`hover_cursor`] from it (tab-resize D11/R2.3), so routing
/// through here would issue a second `hit_test` per pointer move.
#[cfg(test)]
fn sidebar_hover(
    cursor: (f64, f64),
    inp: &SidebarLayoutInput,
    scroll: f32,
    collapsed: bool,
) -> Option<usize> {
    hover_card(sidebar_region(cursor, inp, scroll, collapsed))
}

/// Whether a left-press dismisses the open modal surfaces — the card menu **and** the
/// rename field (sticky-tab-title D8/D8a.1).
///
/// One rule for both surfaces, in one testable place. Enforcing it for the menu alone is
/// what let a press collapse the sidebar while leaving an invisible rename field behind,
/// still consuming every keystroke — so this is the L2 modal-exclusion invariant made
/// checkable rather than argued in a comment.
///
/// The two menu regions are the exception: pressing the hamburger or picking an item is
/// *operating* the menu, not dismissing it. Everything else — another card, chrome, the
/// panel's dead space, or the grid (`None`) — dismisses.
pub fn press_dismisses_modals(action: Option<SidebarAction>) -> bool {
    !matches!(
        action,
        Some(SidebarAction::MenuItem(_)) | Some(SidebarAction::OpenMenu(_))
    )
}

/// How long after a click a second one still counts as a double-click. macOS's own
/// default interval is 500 ms; matching it is what makes the gesture feel native rather
/// than tuned to this app.
pub const DOUBLE_CLICK_MS: u128 = 500;

/// Whether this press is the second half of a double-click on the **same card**
/// (sticky-tab-title): within [`DOUBLE_CLICK_MS`] of the previous press, on the same row.
///
/// Same-row is part of the test, not just timing: two fast clicks on two different cards
/// are two deliberate tab switches, and treating them as a rename gesture would hijack
/// quick navigation. `prev` is `None` when nothing has been clicked yet.
pub fn is_double_click(prev: Option<(usize, u128)>, row: usize, now_ms: u128) -> bool {
    match prev {
        Some((prev_row, prev_ms)) => {
            prev_row == row && now_ms.saturating_sub(prev_ms) <= DOUBLE_CLICK_MS
        }
        None => false,
    }
}

/// The new scroll offset after a wheel event (D10). Gated by `hit_test` (not `x <
/// sidebar_w`, D1): a wheel over the terminal leaves the list unmoved. A line delta
/// advances one card (`row_stride`); the sign follows winit (positive y = content down =
/// scroll increases). Clamping is the sibling's (NaN-safe).
pub fn wheel_scroll(
    delta: WheelDelta,
    cursor: (f64, f64),
    scroll: f32,
    inp: &SidebarLayoutInput,
    collapsed: bool,
) -> f32 {
    let p = [cursor.0 as f32, cursor.1 as f32];
    if sidebar_layout::hit_test(p, scroll, inp, collapsed).is_none() {
        return scroll; // over the grid — do not move an invisible/other list
    }
    let delta_px = match delta {
        WheelDelta::Pixels(y) => y,
        WheelDelta::Lines(y) => y * sidebar_layout::row_stride(inp),
    };
    sidebar_layout::clamp_scroll(scroll - delta_px, inp)
}

/// Grid lines one mouse-wheel notch scrolls (terminal-scroll D2). Three matches
/// Terminal.app/iTerm2's default feel. A `const` on purpose (D10): a config key would
/// have to live in `common::Config`, which is outside this feature's app-only scope.
pub const LINES_PER_NOTCH: f32 = 3.0;

/// Whole grid lines to scroll for one wheel event over the terminal, carrying the
/// sub-line remainder in `accum` (terminal-scroll D2/D3/D9).
///
/// Positive means **into history** (`scroll_up`), following winit's convention that
/// positive y moves content down and reveals content above — the same sign
/// [`wheel_scroll`] follows. macOS applies natural-scrolling upstream, so there is no
/// preference to read here.
///
/// `accum` is why a slow trackpad drag works at all: each event carries a few px, so
/// truncating them independently would round every one to zero lines and the terminal
/// would appear not to scroll. Its unit is **lines**, and both `WheelDelta` variants
/// feed it — a mouse and a trackpad can alternate mid-gesture without a discontinuity.
/// Direction reversal needs no special case: an unspent `+0.6` cancelled by a later
/// `-0.6` nets zero, which is the right answer.
///
/// A non-finite delta or non-positive `cell_h` (possible before the first layout)
/// returns 0 and leaves `accum` **untouched** — folding a NaN in would poison it for
/// the rest of the session, and since `as` saturates NaN to 0 rather than panicking,
/// scrolling would then die silently.
pub fn wheel_lines(delta: WheelDelta, cell_h: f32, accum: &mut f32) -> i32 {
    let lines = match delta {
        WheelDelta::Lines(y) => y * LINES_PER_NOTCH,
        WheelDelta::Pixels(y) => {
            if cell_h <= 0.0 || !cell_h.is_finite() {
                return 0;
            }
            y / cell_h
        }
    };
    if !lines.is_finite() {
        return 0;
    }
    *accum += lines;
    let whole = accum.trunc();
    *accum -= whole;
    // Saturates on overflow; `scroll_up`/`scroll_down` clamp on the other side.
    whole as i32
}

/// Encode one wheel notch as a mouse report for an application that asked for
/// tracking (`alt-screen-scroll`).
///
/// `col`/`row` are 0-based grid cells; the wire format is 1-based. `up` picks the
/// button code: wheel-up is 64, wheel-down is 65 (bit 6 marks a wheel button, and
/// wheel events are press-only — there is no release report).
///
/// Two encodings, chosen by `?1006`:
/// - [`MouseEncoding::Sgr`] → `ESC [ < b ; col ; row M`. No coordinate ceiling.
/// - [`MouseEncoding::Legacy`] → `ESC [ M` then `b + 32`, `col + 32`, `row + 32` as
///   raw bytes. A coordinate past 223 cannot be expressed, so the report is
///   **dropped** rather than sent with a wrapped byte that the app would read as a
///   click somewhere else entirely.
pub fn encode_wheel_report(
    encoding: MouseEncoding,
    up: bool,
    col: u16,
    row: u16,
) -> Option<Vec<u8>> {
    let button: u16 = if up { 64 } else { 65 };
    let (cx, cy) = (col.saturating_add(1), row.saturating_add(1));
    match encoding {
        MouseEncoding::Sgr => Some(format!("\x1b[<{button};{cx};{cy}M").into_bytes()),
        MouseEncoding::Legacy => {
            // Each field is offset by 32 and must fit one byte.
            if cx > 223 || cy > 223 {
                return None;
            }
            Some(vec![
                0x1b,
                b'[',
                b'M',
                (button + 32) as u8,
                (cx + 32) as u8,
                (cy + 32) as u8,
            ])
        }
    }
}

/// Which escape-sequence family a cursor/navigation/function key belongs to
/// (D11/D13). The family decides *both* the unmodified form and where the xterm
/// modifier parameter goes, so the table below stays a one-liner per key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeySeq {
    /// Arrows. Unmodified: `ESC[{final}` normally, `ESC O {final}` under DECCKM
    /// (D1). Modified: `CSI 1;{m}{final}` in both modes (D5).
    Cursor(u8),
    /// F1–F4. Unmodified: always SS3 `ESC O {final}` — DECCKM does not apply.
    /// Modified: `CSI 1;{m}{final}`, same as the cursor family (D5/D13).
    Ss3(u8),
    /// The tilde family: `ESC[{n}~`, modifier in the *second* parameter →
    /// `CSI {n};{m}~`. Never has an application-mode variant (D11).
    Tilde(u8),
}

/// The cursor/navigation/function key table (D11). `None` for every other key —
/// those fall through to the legacy arms and the `text` fallback (D6).
///
/// This list is also exactly the set of keys Cmd swallows (D15).
fn key_seq(key: &Key) -> Option<KeySeq> {
    let Key::Named(named) = key else {
        return None;
    };
    Some(match named {
        // Arrows (D1).
        NamedKey::ArrowUp => KeySeq::Cursor(b'A'),
        NamedKey::ArrowDown => KeySeq::Cursor(b'B'),
        NamedKey::ArrowRight => KeySeq::Cursor(b'C'),
        NamedKey::ArrowLeft => KeySeq::Cursor(b'D'),
        // Navigation. Home/End are tilde-family, not `ESC[H`/`ESC[F`: those are
        // bound to nothing in zsh's emacs keymap, `^[[1~`/`^[[4~` are (D11).
        NamedKey::Home => KeySeq::Tilde(1),
        NamedKey::Insert => KeySeq::Tilde(2),
        NamedKey::Delete => KeySeq::Tilde(3),
        NamedKey::End => KeySeq::Tilde(4),
        NamedKey::PageUp => KeySeq::Tilde(5),
        NamedKey::PageDown => KeySeq::Tilde(6),
        // F1–F4 are SS3; F5–F12 are tilde-family with 16 and 22 skipped, which
        // matches terminfo `kf5`..`kf12`.
        NamedKey::F1 => KeySeq::Ss3(b'P'),
        NamedKey::F2 => KeySeq::Ss3(b'Q'),
        NamedKey::F3 => KeySeq::Ss3(b'R'),
        NamedKey::F4 => KeySeq::Ss3(b'S'),
        NamedKey::F5 => KeySeq::Tilde(15),
        NamedKey::F6 => KeySeq::Tilde(17),
        NamedKey::F7 => KeySeq::Tilde(18),
        NamedKey::F8 => KeySeq::Tilde(19),
        NamedKey::F9 => KeySeq::Tilde(20),
        NamedKey::F10 => KeySeq::Tilde(21),
        NamedKey::F11 => KeySeq::Tilde(23),
        NamedKey::F12 => KeySeq::Tilde(24),
        _ => return None,
    })
}

/// The xterm modifier parameter: `m = 1 + Shift·1 + Alt·2 + Ctrl·4` (D4). `1`
/// means "no modifiers", which callers encode as the parameterless form.
fn modifier_param(mods: ModifiersState) -> u8 {
    1 + u8::from(mods.shift_key()) + 2 * u8::from(mods.alt_key()) + 4 * u8::from(mods.control_key())
}

/// Render a [`KeySeq`] to bytes for the current modifiers and DECCKM state.
fn encode_seq(seq: KeySeq, mods: ModifiersState, app_cursor: bool) -> Vec<u8> {
    let m = modifier_param(mods);
    match seq {
        // Unmodified arrows follow DECCKM; unmodified F1–F4 are always SS3.
        KeySeq::Cursor(final_byte) if m == 1 => {
            let intro = if app_cursor { b'O' } else { b'[' };
            vec![0x1b, intro, final_byte]
        }
        KeySeq::Ss3(final_byte) if m == 1 => vec![0x1b, b'O', final_byte],
        // Modified cursor/SS3 keys: `CSI 1;{m}{final}` — even under DECCKM, since
        // SS3 has nowhere to carry a parameter (D5).
        KeySeq::Cursor(final_byte) | KeySeq::Ss3(final_byte) => {
            let mut bytes = format!("\x1b[1;{m}").into_bytes();
            bytes.push(final_byte);
            bytes
        }
        // Tilde family: modifier rides in the second parameter (D13).
        KeySeq::Tilde(n) if m == 1 => format!("\x1b[{n}~").into_bytes(),
        KeySeq::Tilde(n) => format!("\x1b[{n};{m}~").into_bytes(),
    }
}

/// Translate a key press into the bytes a shell expects. Cursor/navigation/
/// function keys first (D11/D13, honouring DECCKM via `app_cursor`), then the
/// legacy named keys, then the event's text (letters, symbols, IME output).
/// Returns `None` for keys with no shell encoding — including Cmd-held cursor
/// and navigation keys, which stay app-level (D15).
pub fn encode_key(
    logical: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    if let Some(seq) = key_seq(logical) {
        // D15: Cmd(super) is an app-level modifier — swallow, don't send. The
        // scope is exactly `key_seq`'s table; the legacy arms below are not
        // swallowed (Cmd+Shift+Tab still sends `\t`), and `Key::Character` never gets
        // here (Cmd+A still reaches the shell through the text fallback).
        if mods.super_key() {
            return None;
        }
        return Some(encode_seq(seq, mods, app_cursor));
    }
    match logical {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        // Shift+Tab is CBT (D13) — the one legacy arm that reads a modifier.
        // Cmd+Shift+Tab is excluded: D15/R6.2 pin it to the pre-existing `\t`
        // (it is the macOS app-switcher chord, not a terminal back-tab).
        Key::Named(NamedKey::Tab) if mods.shift_key() && !mods.super_key() => {
            Some(b"\x1b[Z".to_vec())
        }
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::Space) => Some(vec![b' ']),
        _ => text.map(|t| t.as_bytes().to_vec()),
    }
}

/// Should this keypress be dropped because an IME composition is in flight (D8)?
///
/// True **only** for character-producing keys while composing (R6.3) — the jamo
/// that belong to the IME, not the shell. Named keys (Enter/Backspace/Tab/Escape/
/// arrows) always pass through (R6.4): winit's macOS `doCommandBySelector`
/// deliberately forwards them during `Preedit` (`forward_key_to_app = true`), so
/// an over-broad gate would kill cursor movement mid-composition. With
/// `composing == false` nothing is ever suppressed (R6.5) — the ASCII path is
/// untouched.
///
/// This is **defense-in-depth**, not the primary mechanism: winit 0.30.13 already
/// withholds `KeyboardInput` while `had_ime_input` is set and already swallows the
/// Korean IME's double-send. The gate exists so a future winit regression shows up
/// as a dropped jamo rather than doubled text.
pub fn suppress_key(composing: bool, key: &Key) -> bool {
    composing && matches!(key, Key::Character(_) | Key::Dead(_))
}

/// Map a physical-px cursor point to a viewport cell `(col, row)`, inverting the
/// renderer's cursor mapping (`x = left_inset + col*cw`, `y = top_inset + row*ch`).
///
/// Returns `None` when the point is left of the grid (over the sidebar band) or
/// above it (the top inset) — the dead band a grid press must ignore. A point past
/// the right/bottom edge clamps to the last cell, so a drag off the edge selects the
/// edge cell rather than going out of bounds. Pure (no winit) → unit-tested.
pub fn pixel_to_cell(
    cursor: (f64, f64),
    cell: (f32, f32),
    left_inset: f32,
    top_inset: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    let (px, py) = (cursor.0 as f32, cursor.1 as f32);
    let (cw, ch) = cell;
    if px < left_inset || py < top_inset || cw <= 0.0 || ch <= 0.0 || cols == 0 || rows == 0 {
        return None;
    }
    // `px - left_inset >= 0` and `cw > 0` → non-negative quotient; `as u16` floors
    // (saturating on overflow), and `.min` binds to the last cell.
    let col = ((px - left_inset) / cw) as u16;
    let row = ((py - top_inset) / ch) as u16;
    Some((col.min(cols - 1), row.min(rows - 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_to_cell_maps_a_point_inside_the_grid() {
        // R1.1 — cell (8,16), insets (40,24), cursor (64,40) → (3,1).
        assert_eq!(
            pixel_to_cell((64.0, 40.0), (8.0, 16.0), 40.0, 24.0, 100, 40),
            Some((3, 1))
        );
        // Exact top-left corner of the grid (px == insets) → cell (0,0) (strict `<`).
        assert_eq!(
            pixel_to_cell((40.0, 24.0), (8.0, 16.0), 40.0, 24.0, 100, 40),
            Some((0, 0))
        );
    }

    #[test]
    fn pixel_to_cell_rejects_the_sidebar_and_top_band() {
        // R1.2 — left of left_inset and above top_inset → None.
        assert_eq!(
            pixel_to_cell((20.0, 10.0), (8.0, 16.0), 40.0, 24.0, 100, 40),
            None
        );
        // Left band alone (above the grid horizontally) is also None.
        assert_eq!(
            pixel_to_cell((10.0, 100.0), (8.0, 16.0), 40.0, 24.0, 100, 40),
            None
        );
    }

    #[test]
    fn pixel_to_cell_clamps_past_the_far_edge() {
        // R1.3 — far bottom-right clamps to the last cell (9,4), never out of range.
        assert_eq!(
            pixel_to_cell((9_999.0, 9_999.0), (8.0, 16.0), 40.0, 24.0, 10, 5),
            Some((9, 4))
        );
    }

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn cmd_chords_map_to_actions() {
        let cmd = ModifiersState::SUPER;
        assert_eq!(tab_action(&ch("t"), &cmd), Some(TabAction::New));
        assert_eq!(tab_action(&ch("w"), &cmd), Some(TabAction::CloseActive));
        assert_eq!(tab_action(&ch("]"), &cmd), Some(TabAction::Next));
        assert_eq!(tab_action(&ch("["), &cmd), Some(TabAction::Prev));
        assert_eq!(tab_action(&ch("}"), &cmd), Some(TabAction::Next));
        assert_eq!(tab_action(&ch("{"), &cmd), Some(TabAction::Prev));
        assert_eq!(tab_action(&ch("1"), &cmd), Some(TabAction::Jump(0)));
        assert_eq!(tab_action(&ch("9"), &cmd), Some(TabAction::Jump(8)));
    }

    #[test]
    fn no_super_means_no_chord() {
        let none = ModifiersState::empty();
        assert_eq!(
            tab_action(&ch("t"), &none),
            None,
            "plain 't' types into shell"
        );
        // A non-chord char with Cmd held is also not a tab action.
        assert_eq!(tab_action(&ch("z"), &ModifiersState::SUPER), None);
    }

    /// Every cursor/nav test below goes through this: no modifiers, DECCKM off.
    fn enc(key: NamedKey) -> Option<Vec<u8>> {
        encode_key(&Key::Named(key), None, ModifiersState::empty(), false)
    }

    /// Same, with DECCKM on (application cursor key mode).
    fn enc_app(key: NamedKey) -> Option<Vec<u8>> {
        encode_key(&Key::Named(key), None, ModifiersState::empty(), true)
    }

    /// Same, with modifiers held.
    fn enc_mods(key: NamedKey, mods: ModifiersState) -> Option<Vec<u8>> {
        encode_key(&Key::Named(key), None, mods, false)
    }

    fn bytes(s: &str) -> Option<Vec<u8>> {
        Some(s.as_bytes().to_vec())
    }

    #[test]
    fn encode_named_and_text() {
        let none = ModifiersState::empty();
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Enter), None, none, false),
            Some(vec![b'\r'])
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Backspace), None, none, false),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode_key(&ch("a"), Some("a"), none, false),
            Some(b"a".to_vec())
        );
    }

    // R3.1 — normal cursor key mode: arrows are CSI.
    #[test]
    fn arrows_are_csi_in_normal_mode() {
        assert_eq!(enc(NamedKey::ArrowUp), bytes("\x1b[A"));
        assert_eq!(enc(NamedKey::ArrowDown), bytes("\x1b[B"));
        assert_eq!(enc(NamedKey::ArrowRight), bytes("\x1b[C"));
        assert_eq!(
            enc(NamedKey::ArrowLeft),
            bytes("\x1b[D"),
            "DECCKM off is the zsh-prompt default — CSI is the primary path"
        );
    }

    // R3.2 — DECCKM on: arrows switch to SS3.
    #[test]
    fn arrows_are_ss3_in_application_mode() {
        assert_eq!(enc_app(NamedKey::ArrowUp), bytes("\x1bOA"));
        assert_eq!(enc_app(NamedKey::ArrowDown), bytes("\x1bOB"));
        assert_eq!(enc_app(NamedKey::ArrowRight), bytes("\x1bOC"));
        assert_eq!(
            enc_app(NamedKey::ArrowLeft),
            bytes("\x1bOD"),
            "a TUI that set ESC[?1h expects SS3, not CSI"
        );
    }

    // R4.1 — Home/End are tilde-family, the only form zsh binds.
    #[test]
    fn home_and_end_are_tilde_family() {
        assert_eq!(
            enc(NamedKey::Home),
            bytes("\x1b[1~"),
            "zsh binds ^[[1~ to beginning-of-line; ^[[H is bound to nothing"
        );
        assert_eq!(
            enc(NamedKey::End),
            bytes("\x1b[4~"),
            "zsh binds ^[[4~ to end-of-line; ^[[F is bound to nothing"
        );
    }

    // R4.2 / R4.3 — the rest of the navigation block.
    #[test]
    fn nav_keys_encode_to_their_tilde_numbers() {
        assert_eq!(enc(NamedKey::PageUp), bytes("\x1b[5~"));
        assert_eq!(enc(NamedKey::PageDown), bytes("\x1b[6~"));
        assert_eq!(enc(NamedKey::Insert), bytes("\x1b[2~"));
        assert_eq!(
            enc(NamedKey::Delete),
            bytes("\x1b[3~"),
            "macOS Fn+Delete arrives as NamedKey::Delete → delete-char in zsh; \
             the key labelled 'delete' is Backspace and still sends 0x7f"
        );
    }

    // R4.4 — F1–F4 are SS3 even with DECCKM off.
    #[test]
    fn f1_to_f4_are_ss3() {
        assert_eq!(enc(NamedKey::F1), bytes("\x1bOP"));
        assert_eq!(enc(NamedKey::F2), bytes("\x1bOQ"));
        assert_eq!(enc(NamedKey::F3), bytes("\x1bOR"));
        assert_eq!(
            enc(NamedKey::F4),
            bytes("\x1bOS"),
            "F1-F4 use SS3 regardless of DECCKM — the mode covers cursor keys only"
        );
    }

    // R4.5 — F5–F12 are tilde-family and skip 16 and 22.
    #[test]
    fn f5_to_f12_are_tilde_family_with_gaps() {
        assert_eq!(enc(NamedKey::F5), bytes("\x1b[15~"));
        assert_eq!(enc(NamedKey::F6), bytes("\x1b[17~"), "16 is skipped");
        assert_eq!(enc(NamedKey::F7), bytes("\x1b[18~"));
        assert_eq!(enc(NamedKey::F8), bytes("\x1b[19~"));
        assert_eq!(enc(NamedKey::F9), bytes("\x1b[20~"));
        assert_eq!(enc(NamedKey::F10), bytes("\x1b[21~"));
        assert_eq!(enc(NamedKey::F11), bytes("\x1b[23~"), "22 is skipped");
        assert_eq!(
            enc(NamedKey::F12),
            bytes("\x1b[24~"),
            "F5-F12 match terminfo kf5..kf12"
        );
    }

    // R4.6 — DECCKM is a cursor-key mode; the tilde family ignores it.
    #[test]
    fn tilde_family_ignores_application_mode() {
        for key in [
            NamedKey::Home,
            NamedKey::PageUp,
            NamedKey::Delete,
            NamedKey::F5,
        ] {
            assert_eq!(
                enc_app(key),
                enc(key),
                "{key:?} has no application-mode variant — DECCKM applies to cursor keys only"
            );
        }
    }

    // R5.1 / R5.2 — the xterm modifier parameter m = 1 + Shift·1 + Alt·2 + Ctrl·4.
    #[test]
    fn modified_arrows_use_the_xterm_parameter() {
        assert_eq!(
            enc_mods(NamedKey::ArrowRight, ModifiersState::CONTROL),
            bytes("\x1b[1;5C")
        );
        assert_eq!(
            enc_mods(NamedKey::ArrowLeft, ModifiersState::SHIFT),
            bytes("\x1b[1;2D")
        );
        assert_eq!(
            enc_mods(NamedKey::ArrowLeft, ModifiersState::ALT),
            bytes("\x1b[1;3D")
        );
        assert_eq!(
            enc_mods(NamedKey::ArrowLeft, ModifiersState::CONTROL),
            bytes("\x1b[1;5D")
        );
        assert_eq!(
            enc_mods(
                NamedKey::ArrowLeft,
                ModifiersState::CONTROL | ModifiersState::SHIFT
            ),
            bytes("\x1b[1;6D"),
            "Ctrl(4) + Shift(1) + base 1 = 6"
        );
        assert_eq!(
            enc_mods(NamedKey::F1, ModifiersState::CONTROL),
            bytes("\x1b[1;5P"),
            "a modified SS3 key becomes CSI too — SS3 cannot carry a parameter"
        );
    }

    // R5.3 — the tilde family puts the modifier in the *second* parameter.
    #[test]
    fn modified_tilde_keys_put_the_modifier_second() {
        assert_eq!(
            enc_mods(NamedKey::PageUp, ModifiersState::CONTROL),
            bytes("\x1b[5;5~")
        );
        assert_eq!(
            enc_mods(NamedKey::Home, ModifiersState::CONTROL),
            bytes("\x1b[1;5~"),
            "Ctrl+Home stays tilde-family — it is not ESC[1;5H"
        );
        assert_eq!(
            enc_mods(NamedKey::Delete, ModifiersState::CONTROL),
            bytes("\x1b[3;5~")
        );
        assert_eq!(
            enc_mods(NamedKey::Delete, ModifiersState::SHIFT),
            bytes("\x1b[3;2~")
        );
    }

    // R5.4 — modifiers force CSI even under DECCKM (D5).
    #[test]
    fn modified_arrows_stay_csi_in_application_mode() {
        assert_eq!(
            encode_key(
                &Key::Named(NamedKey::ArrowUp),
                None,
                ModifiersState::CONTROL,
                true
            ),
            bytes("\x1b[1;5A"),
            "neither ESC O A (modifier lost) nor ESC O 1;5 A (unparseable)"
        );
    }

    // R5.5 — m == 1 means the plain form, never an explicit `;1`.
    #[test]
    fn no_modifiers_means_no_parameter() {
        assert_eq!(
            enc_mods(NamedKey::ArrowRight, ModifiersState::empty()),
            bytes("\x1b[C"),
            "ESC[1;1C would be a needless non-standard form"
        );
        assert_eq!(
            enc_mods(NamedKey::PageUp, ModifiersState::empty()),
            bytes("\x1b[5~"),
            "ESC[5;1~ would be a needless non-standard form"
        );
    }

    // R5.6 — Shift+Tab is CBT; plain Tab is unchanged.
    #[test]
    fn shift_tab_is_cbt_and_plain_tab_is_unchanged() {
        assert_eq!(
            enc_mods(NamedKey::Tab, ModifiersState::SHIFT),
            bytes("\x1b[Z"),
            "back-tab drives reverse completion and TUI focus movement"
        );
        assert_eq!(enc(NamedKey::Tab), Some(vec![b'\t']));
    }

    // R6.1 — Cmd is app-level for the cursor/nav table (D15).
    #[test]
    fn cmd_swallows_cursor_and_nav_keys() {
        let cmd = ModifiersState::SUPER;
        for key in [
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::ArrowUp,
            NamedKey::ArrowDown,
            NamedKey::Home,
            NamedKey::End,
            NamedKey::PageUp,
            NamedKey::PageDown,
            NamedKey::Insert,
            NamedKey::Delete,
            NamedKey::F1,
            NamedKey::F12,
        ] {
            assert_eq!(
                enc_mods(key, cmd),
                None,
                "{key:?} with Cmd stays app-level — nothing is written to the PTY"
            );
        }
    }

    // R6.2 — the swallow list is exhaustive: characters and Tab are not on it.
    #[test]
    fn cmd_does_not_swallow_characters_or_tab() {
        let cmd = ModifiersState::SUPER;
        assert_eq!(
            encode_key(&ch("a"), Some("a"), cmd, false),
            Some(b"a".to_vec()),
            "Key::Character is outside D15's list — Cmd+A behaves as before"
        );
        assert_eq!(
            encode_key(
                &Key::Named(NamedKey::Tab),
                None,
                cmd | ModifiersState::SHIFT,
                false
            ),
            Some(vec![b'\t']),
            "Tab is not in D15's swallow list, and R6.2 pins Cmd+Shift+Tab to \\t"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Tab), None, cmd, false),
            Some(vec![b'\t']),
            "Cmd+Tab is not swallowed either (existing behaviour preserved)"
        );
    }

    // R6.3 — the text fallback (letters, symbols, IME) is untouched.
    #[test]
    fn text_fallback_still_carries_ime_output() {
        let none = ModifiersState::empty();
        assert_eq!(
            encode_key(&ch("한"), Some("한"), none, false),
            Some("한".as_bytes().to_vec()),
            "Korean composition arrives as KeyEvent::text and must pass through"
        );
        assert_eq!(
            encode_key(&ch("å"), Some("å"), ModifiersState::ALT, false),
            Some("å".as_bytes().to_vec()),
            "macOS Option composes a character — prefixing ESC would break it"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::F13), None, none, false),
            None,
            "an unmapped named key with no text still encodes to nothing"
        );
    }

    // R6.4 — the legacy arms ignore modifiers (Shift+Tab excepted).
    #[test]
    fn legacy_arms_ignore_modifiers() {
        let ctrl = ModifiersState::CONTROL;
        assert_eq!(enc_mods(NamedKey::Enter, ctrl), Some(vec![b'\r']));
        assert_eq!(enc_mods(NamedKey::Backspace, ctrl), Some(vec![0x7f]));
        assert_eq!(enc_mods(NamedKey::Escape, ctrl), Some(vec![0x1b]));
        assert_eq!(
            enc_mods(NamedKey::Space, ctrl),
            Some(vec![b' ']),
            "this spec does not change Enter/Backspace/Escape/Space encoding"
        );
    }

    #[test]
    fn composing_suppresses_character_keys() {
        // R6.3: a jamo typed mid-composition belongs to the IME, not the shell.
        assert!(suppress_key(true, &ch("ㅎ")));
        assert!(suppress_key(true, &ch("a")));
        // Dead keys (accent composition) are character-producing too (D8).
        assert!(suppress_key(true, &Key::Dead(Some('`'))));
    }

    #[test]
    fn composing_lets_named_keys_through() {
        // R6.4: winit's doCommandBySelector forwards these during Preedit — the
        // gate must not eat cursor movement / editing keys mid-composition.
        for named in [
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Enter,
            NamedKey::Backspace,
            NamedKey::Tab,
            NamedKey::Escape,
        ] {
            assert!(
                !suppress_key(true, &Key::Named(named)),
                "{named:?} must reach the shell while composing"
            );
        }
    }

    #[test]
    fn not_composing_suppresses_nothing() {
        // R6.5: the ASCII path is bit-for-bit unchanged when no IME is active.
        assert!(!suppress_key(false, &ch("a")));
        assert!(!suppress_key(false, &ch("ㅎ")));
        assert!(!suppress_key(false, &Key::Dead(Some('`'))));
        assert!(!suppress_key(false, &Key::Named(NamedKey::Enter)));
    }

    fn layout_input(sidebar_w: f32, count: usize) -> SidebarLayoutInput {
        layout_input_w(sidebar_w, count, 0)
    }

    fn layout_input_w(sidebar_w: f32, count: usize, waiting_count: usize) -> SidebarLayoutInput {
        SidebarLayoutInput {
            sidebar_w,
            viewport: [sidebar_w.max(200.0), 900.0],
            scale: 1.0,
            row_h: 60.0,
            row_count: count,
            waiting_count,
            metrics: renderer::sidebar_theme::SidebarMetrics::DEFAULT.scaled(1.0),
            open_menu: None,
        }
    }

    #[test]
    fn every_press_but_the_menu_itself_dismisses_the_modals() {
        // The L2 modal-exclusion invariant, as a test rather than a comment: enforcing it
        // for the menu but not the field is what once left an INVISIBLE rename field
        // eating every keystroke after the sidebar was collapsed.
        for a in [
            SidebarAction::Switch(0),
            SidebarAction::SwitchWaiting(0),
            SidebarAction::NewTab,
            SidebarAction::Collapse,
            SidebarAction::Expand,
            SidebarAction::Inert,
        ] {
            assert!(press_dismisses_modals(Some(a)), "{a:?} must dismiss");
        }
        // A press over the grid (no sidebar region at all) dismisses too.
        assert!(press_dismisses_modals(None));
        // Operating the menu is not dismissing it.
        assert!(!press_dismisses_modals(Some(SidebarAction::OpenMenu(1))));
        assert!(!press_dismisses_modals(Some(SidebarAction::MenuItem(0))));
    }

    #[test]
    fn double_click_needs_the_same_card_and_a_quick_second_press() {
        // sticky-tab-title — a second click on the same card opens the rename field.
        assert!(is_double_click(Some((2, 1_000)), 2, 1_200));
        // Exactly at the boundary still counts.
        assert!(is_double_click(
            Some((2, 1_000)),
            2,
            1_000 + DOUBLE_CLICK_MS
        ));
        // Too slow — two separate clicks, not a rename gesture.
        assert!(!is_double_click(
            Some((2, 1_000)),
            2,
            1_001 + DOUBLE_CLICK_MS
        ));
        // A DIFFERENT card: two fast clicks on two cards are two deliberate tab switches,
        // and hijacking that into a rename would break quick navigation.
        assert!(!is_double_click(Some((2, 1_000)), 3, 1_050));
        // Nothing clicked yet.
        assert!(!is_double_click(None, 0, 10));
    }

    #[test]
    fn hovering_the_hamburger_keeps_its_own_card_highlighted() {
        // sticky-tab-title R3.1 — the button resolves ahead of the card it sits on, so
        // hover must map it back to that card or the highlight drops out from under the
        // pointer the moment it reaches the control.
        let inp = layout_input(340.0, 5);
        let b = renderer::sidebar_layout::menu_button_rect(1, 0.0, &inp);
        let centre = ((b.x + b.w * 0.5) as f64, (b.y + b.h * 0.5) as f64);
        assert_eq!(sidebar_hover(centre, &inp, 0.0, false), Some(1));
        // And the press at that same point still opens the menu rather than switching.
        assert_eq!(
            sidebar_press(centre, &inp, 0.0, false),
            Some(SidebarAction::OpenMenu(1))
        );
    }

    #[test]
    fn sidebar_press_maps_regions_and_matches_hit_test() {
        // SD2 / D1 — the app performs no geometry: every press equals what hit_test says.
        let inp = layout_input(340.0, 5);
        // A point well into the list (below the 54px header) is a card switch.
        let card_y = sidebar_layout::header_h(1.0) + 100.0;
        let press = sidebar_press((170.0, card_y as f64), &inp, 0.0, false);
        let region = sidebar_layout::hit_test([170.0, card_y], 0.0, &inp, false);
        assert!(
            matches!(press, Some(SidebarAction::Switch(_))),
            "list press → Switch"
        );
        assert!(
            matches!(region, Some(SidebarRegion::Card(_))),
            "and hit_test agrees"
        );
        // The hide button (top-right) collapses.
        let hide = sidebar_layout::button_rect(ChromeRegion::HideSidebar, &inp);
        let c = (
            (hide.x + hide.w / 2.0) as f64,
            (hide.y + hide.h / 2.0) as f64,
        );
        assert_eq!(
            sidebar_press(c, &inp, 0.0, false),
            Some(SidebarAction::Collapse)
        );
        // A point over the grid is not the sidebar's business.
        assert_eq!(sidebar_press((400.0, 100.0), &inp, 0.0, false), None);
        // Collapsed: the restore button expands; elsewhere None.
        let r = sidebar_layout::restore_button_rect(1.0);
        let rc = ((r.x + r.w / 2.0) as f64, (r.y + r.h / 2.0) as f64);
        assert_eq!(
            sidebar_press(rc, &inp, 0.0, true),
            Some(SidebarAction::Expand)
        );
        assert_eq!(sidebar_press((400.0, 400.0), &inp, 0.0, true), None);
    }

    #[test]
    fn sidebar_press_maps_waiting_band_rows() {
        // R2.1/D12 — a press in the pinned waiting band resolves to SwitchWaiting(i), not
        // a card; a press below the band still resolves to a card, un-shifted; the band
        // padding / summary area is inert.
        let inp = layout_input_w(340.0, 5, 2); // 2 waiting rows above the cards
                                               // Band content starts at header_h + PAD_TOP = 60; row 0 = [60,90), row 1 = [90,120).
        assert_eq!(
            sidebar_press((170.0, 65.0), &inp, 0.0, false),
            Some(SidebarAction::SwitchWaiting(0))
        );
        assert_eq!(
            sidebar_press((170.0, 95.0), &inp, 0.0, false),
            Some(SidebarAction::SwitchWaiting(1))
        );
        // A press well below the band is a normal card switch.
        let band_bottom = sidebar_layout::header_h(1.0) + sidebar_layout::waiting_band_h(2, 1.0);
        assert!(matches!(
            sidebar_press((170.0, (band_bottom + 40.0) as f64), &inp, 0.0, false),
            Some(SidebarAction::Switch(_))
        ));
    }

    #[test]
    fn sidebar_hover_returns_only_card_index() {
        let inp = layout_input(340.0, 5);
        let card_y = sidebar_layout::header_h(1.0) + 100.0;
        assert!(sidebar_hover((170.0, card_y as f64), &inp, 0.0, false).is_some());
        // A header point is not a card hover.
        assert_eq!(sidebar_hover((170.0, 10.0), &inp, 0.0, false), None);
    }

    #[test]
    fn wheel_scroll_gates_units_and_sign() {
        let inp = layout_input(340.0, 40); // overflows → scrollable
        let over_list = (170.0, (sidebar_layout::header_h(1.0) + 50.0) as f64);
        // Positive line delta scrolls down by one card stride.
        let stride = sidebar_layout::row_stride(&inp);
        assert_eq!(
            wheel_scroll(WheelDelta::Lines(1.0), over_list, 0.0, &inp, false),
            sidebar_layout::clamp_scroll(-stride, &inp),
        );
        // Pixel delta is used directly (sign: new = scroll - delta).
        let after = wheel_scroll(WheelDelta::Pixels(30.0), over_list, 100.0, &inp, false);
        assert_eq!(after, sidebar_layout::clamp_scroll(70.0, &inp));
        // A wheel over the grid leaves the offset unchanged (gated by hit_test).
        assert_eq!(
            wheel_scroll(WheelDelta::Pixels(30.0), (400.0, 400.0), 100.0, &inp, false),
            100.0,
        );
    }

    /// R1.4 — **the discriminator D1 turns on.** In the band above the grid (right of
    /// the sidebar, `y < top_inset`) `hit_test` says "not mine" while `pixel_to_cell`
    /// also says "no cell". Routing the wheel on `cursor_cell` would therefore leave
    /// that visible strip silently inert, so the gate is `hit_test`.
    ///
    /// Scope note: this pins the two predicates' disagreement, which is the *reason*
    /// for the gate. That the `MouseWheel` arm actually routes a `None` to the grid is
    /// arm-only and needs a live window, so it is not reachable here.
    #[test]
    fn the_top_band_routes_to_the_grid_but_has_no_cell() {
        let inp = layout_input(340.0, 5);
        let p = (400.0, 10.0); // right of the sidebar, above the first row
        assert!(
            sidebar_layout::hit_test([p.0 as f32, p.1 as f32], 0.0, &inp, false).is_none(),
            "hit_test routes the top band to the grid"
        );
        assert_eq!(
            pixel_to_cell(p, (8.0, 16.0), inp.sidebar_w, 24.0, 100, 40),
            None,
            "…but there is no cell there — cursor_cell would make the strip inert"
        );
    }

    /// R1.3 — collapsed, everything but the restore button is grid, so the wheel
    /// scrolls the terminal across the whole window.
    ///
    /// Scope note: same as above — this pins what `hit_test` reports, not the arm's
    /// dispatch on it.
    #[test]
    fn collapsed_routes_the_whole_window_to_the_grid() {
        let inp = layout_input(340.0, 5);
        let deep_in_the_band = [170.0, 500.0];
        assert!(
            sidebar_layout::hit_test(deep_in_the_band, 0.0, &inp, true).is_none(),
            "collapsed: this point is grid"
        );
        assert!(
            sidebar_layout::hit_test(deep_in_the_band, 0.0, &inp, false).is_some(),
            "expanded: the very same point belongs to the list"
        );
    }

    /// R2.1/R2.2 — a notch is three lines; trackpad px divide by the cell height so
    /// the text tracks the finger 1:1.
    #[test]
    fn wheel_lines_translates_both_unit_systems() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(WheelDelta::Lines(1.0), 16.0, &mut acc), 3);
        assert_eq!(acc, 0.0, "a whole-line delta leaves no remainder");
        assert_eq!(wheel_lines(WheelDelta::Pixels(32.0), 16.0, &mut acc), 2);
        // The cell height is what makes px→lines 1:1; a taller cell means fewer lines.
        let mut acc = 0.0;
        assert_eq!(wheel_lines(WheelDelta::Pixels(32.0), 32.0, &mut acc), 1);
    }

    /// R2.3 — positive is into history, negative is back toward live.
    #[test]
    fn wheel_lines_sign_follows_winit() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(WheelDelta::Lines(1.0), 16.0, &mut acc), 3);
        assert_eq!(wheel_lines(WheelDelta::Lines(-1.0), 16.0, &mut acc), -3);
        assert_eq!(wheel_lines(WheelDelta::Pixels(-16.0), 16.0, &mut acc), -1);
    }

    /// R3.1/R3.2 — **the reason the accumulator exists.** Three 6px events on a 16px
    /// cell are 0.375 lines each; truncating them independently would move nothing at
    /// all and slow trackpad scrolling would look broken.
    #[test]
    fn wheel_lines_accumulates_sub_line_events() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(WheelDelta::Pixels(6.0), 16.0, &mut acc), 0);
        assert_eq!(wheel_lines(WheelDelta::Pixels(6.0), 16.0, &mut acc), 0);
        assert_eq!(
            wheel_lines(WheelDelta::Pixels(6.0), 16.0, &mut acc),
            1,
            "three 0.375-line events must total one line, not zero"
        );
        // R3.2: the 0.125 remainder survives into the next event (0.125 + 0.875 == 1).
        assert!((acc - 0.125).abs() < 1e-6, "remainder kept, got {acc}");
        assert_eq!(wheel_lines(WheelDelta::Pixels(14.0), 16.0, &mut acc), 1);
        assert!(acc.abs() < 1e-6, "remainder spent, got {acc}");
    }

    /// R3.3 — a reversal cancels unspent residue instead of needing a reset: the user
    /// moved back exactly as far as they moved forward, so nothing should scroll.
    #[test]
    fn wheel_lines_reversal_is_self_correcting() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(WheelDelta::Pixels(10.0), 16.0, &mut acc), 0);
        assert_eq!(wheel_lines(WheelDelta::Pixels(-10.0), 16.0, &mut acc), 0);
        assert!(acc.abs() < 1e-6, "net-zero gesture nets zero, got {acc}");
    }

    /// R8.1 — **the silent-death guard.** A NaN folded into the accumulator would make
    /// every later `trunc()` NaN, and `as i32` saturates NaN to 0 rather than panicking,
    /// so scrolling would stop working for the rest of the session with no error.
    #[test]
    fn wheel_lines_rejects_non_finite_deltas_without_poisoning() {
        let mut acc = 0.5;
        assert_eq!(wheel_lines(WheelDelta::Pixels(f32::NAN), 16.0, &mut acc), 0);
        assert_eq!(acc, 0.5, "NaN must not enter the accumulator");
        assert_eq!(
            wheel_lines(WheelDelta::Pixels(f32::INFINITY), 16.0, &mut acc),
            0
        );
        assert_eq!(wheel_lines(WheelDelta::Lines(f32::NAN), 16.0, &mut acc), 0);
        assert_eq!(acc, 0.5);
        // Still alive afterwards: 0.5 + 0.5 == one line.
        assert_eq!(wheel_lines(WheelDelta::Pixels(8.0), 16.0, &mut acc), 1);
    }

    /// R8.2 — `cell_size()` can report zero before the first layout; dividing by it
    /// would produce an infinity that the finite check would then discard anyway.
    #[test]
    fn wheel_lines_rejects_non_positive_cell_height() {
        let mut acc = 0.25;
        assert_eq!(wheel_lines(WheelDelta::Pixels(40.0), 0.0, &mut acc), 0);
        assert_eq!(wheel_lines(WheelDelta::Pixels(40.0), -16.0, &mut acc), 0);
        assert_eq!(wheel_lines(WheelDelta::Pixels(40.0), f32::NAN, &mut acc), 0);
        assert_eq!(acc, 0.25, "accumulator untouched by a geometry reject");
        // Line deltas do not depend on the cell height, so they still work.
        assert_eq!(wheel_lines(WheelDelta::Lines(1.0), 0.0, &mut acc), 3);
    }

    /// R8.3 — a huge but finite delta saturates on the cast; the viewport clamp on the
    /// other side (`Grid::scroll_up`) is what bounds it to the retained history.
    #[test]
    fn wheel_lines_saturates_on_a_huge_delta() {
        let mut acc = 0.0;
        let n = wheel_lines(WheelDelta::Pixels(1e9), 16.0, &mut acc);
        assert!(n > 0, "a huge scroll goes up, got {n}");
        let n = wheel_lines(WheelDelta::Pixels(-1e30), 16.0, &mut acc);
        assert!(n < 0, "and a huge reverse scroll goes down, got {n}");
    }
    // -----------------------------------------------------------------
    // tab-resize: routing the grab band (D3/D11/D13a)
    // -----------------------------------------------------------------

    /// An x inside the grab band of a `sidebar_w`-wide sidebar at scale 1.
    fn in_band(sidebar_w: f32) -> f64 {
        (sidebar_w - 2.0) as f64
    }

    #[test]
    fn a_press_on_the_edge_begins_a_resize() {
        // R1.7 — the region resolves to an ACTION; the app does no geometry of its own.
        let inp = layout_input(340.0, 5);
        for y in [10.0_f64, 200.0, 700.0] {
            assert_eq!(
                sidebar_press((in_band(340.0), y), &inp, 0.0, false),
                Some(SidebarAction::BeginResize),
                "y={y}"
            );
        }
    }

    #[test]
    fn the_edge_press_does_not_switch_tabs() {
        // The regression this arm exists to prevent: without the band resolving first,
        // a press 2px inside the edge would land on the card behind it.
        let inp = layout_input(340.0, 5);
        let card_y = (sidebar_layout::header_h(1.0) + 100.0) as f64;
        assert_eq!(
            sidebar_press((in_band(340.0), card_y), &inp, 0.0, false),
            Some(SidebarAction::BeginResize)
        );
        // …while a press further left still switches, unchanged.
        assert!(matches!(
            sidebar_press((170.0, card_y), &inp, 0.0, false),
            Some(SidebarAction::Switch(_))
        ));
    }

    #[test]
    fn an_edge_press_dismisses_the_open_modals() {
        // Starting a resize is not "operating the menu", so it dismisses like any other
        // non-menu press (sticky-tab-title D8/D8a.1).
        assert!(press_dismisses_modals(Some(SidebarAction::BeginResize)));
    }

    #[test]
    fn the_band_shows_a_column_resize_cursor() {
        // R2.1 — the band's only discoverability affordance.
        let inp = layout_input(340.0, 5);
        let region = sidebar_region((in_band(340.0), 300.0), &inp, 0.0, false);
        assert_eq!(region, Some(SidebarRegion::GrabEdge));
        assert_eq!(hover_cursor(region), CursorIcon::ColResize);
    }

    #[test]
    fn every_other_region_keeps_the_default_cursor() {
        // R2.2 — leaving the band restores the default, including over the grid (None).
        let inp = layout_input(340.0, 5);
        let card_y = (sidebar_layout::header_h(1.0) + 100.0) as f64;
        for p in [(170.0, card_y), (170.0, 10.0), (600.0, 300.0)] {
            let region = sidebar_region(p, &inp, 0.0, false);
            assert_ne!(region, Some(SidebarRegion::GrabEdge));
            assert_eq!(hover_cursor(region), CursorIcon::Default, "at {p:?}");
        }
    }

    #[test]
    fn a_collapsed_sidebar_offers_no_resize_cursor() {
        // R1.6 / R2.2 — nothing to grab when there is no edge.
        let inp = layout_input(0.0, 5);
        let region = sidebar_region((in_band(340.0), 300.0), &inp, 0.0, true);
        assert_eq!(hover_cursor(region), CursorIcon::Default);
        assert_eq!(
            sidebar_press((in_band(340.0), 300.0), &inp, 0.0, true),
            None
        );
    }

    #[test]
    fn the_band_highlights_no_card() {
        // R6.2 — the band sits over the list, but hovering it must not light a card.
        let inp = layout_input(340.0, 5);
        let card_y = (sidebar_layout::header_h(1.0) + 100.0) as f64;
        let region = sidebar_region((in_band(340.0), card_y), &inp, 0.0, false);
        assert_eq!(hover_card(region), None);
        assert_eq!(
            sidebar_hover((in_band(340.0), card_y), &inp, 0.0, false),
            None
        );
    }

    #[test]
    fn hover_still_resolves_cards_and_the_hamburger() {
        // The refactor to hover_card ∘ sidebar_region must not change what hover means.
        let inp = layout_input(340.0, 5);
        let card_y = (sidebar_layout::header_h(1.0) + 100.0) as f64;
        assert_eq!(
            sidebar_hover((170.0, card_y), &inp, 0.0, false),
            hover_card(sidebar_region((170.0, card_y), &inp, 0.0, false)),
            "the wrapper and the pure mapping agree"
        );
        assert!(sidebar_hover((170.0, card_y), &inp, 0.0, false).is_some());
    }

    // -----------------------------------------------------------------
    // tab-resize: pointer → width mapping (D9/D9a)
    // -----------------------------------------------------------------

    // -----------------------------------------------------------------
    // tab-resize: collapse hysteresis (D12) + refit guard (D13)
    // -----------------------------------------------------------------

    #[test]
    fn dragging_past_the_threshold_collapses() {
        // R4.1 — crossing ~220 collapses mid-drag.
        assert_eq!(collapse_threshold(), 220.0);
        assert!(
            next_collapsed(219.0, false),
            "below the threshold → collapse"
        );
        assert!(!next_collapsed(221.0, false), "above it → stay open");
    }

    #[test]
    fn dragging_back_re_expands_within_the_same_gesture() {
        // R4.2 — the gesture stays live across the mode change.
        assert!(!next_collapsed(260.0, true), "back at MIN → re-expand");
        assert!(!next_collapsed(300.0, true));
        assert!(
            next_collapsed(259.0, true),
            "just under MIN → still collapsed"
        );
    }

    #[test]
    fn the_two_thresholds_differ_so_jitter_cannot_flicker_the_mode() {
        // R4.3 — the whole point of the hysteresis gap. Inside [220, 260) the state is
        // *sticky*: whatever it was, it stays. Under D1 each flip is a full-window reflow,
        // so a single shared threshold would reflow the grid on ordinary hand tremor.
        for raw in [220.0_f32, 230.0, 240.0, 250.0, 259.9] {
            assert!(!next_collapsed(raw, false), "open stays open at {raw}");
            assert!(
                next_collapsed(raw, true),
                "collapsed stays collapsed at {raw}"
            );
        }
        // And a full oscillation inside the gap changes nothing.
        let mut state = false;
        for raw in [240.0_f32, 255.0, 225.0, 250.0, 230.0] {
            state = next_collapsed(raw, state);
            assert!(!state, "no flip anywhere inside the gap");
        }
    }

    #[test]
    fn the_thresholds_read_the_raw_width_not_the_clamped_one() {
        // R4.4 — the regression test for the contradiction the L2 review caught. Feeding
        // the CLAMPED width in would make collapse unreachable, because clamping floors
        // the value at 260 — well above the 220 threshold.
        let drag = ResizeDrag { grab_offset: 0.0 };
        let raw = resize_raw_width(200.0, drag, 1.0);
        assert!(next_collapsed(raw, false), "raw 200 collapses");
        assert!(
            !next_collapsed(clamp_sidebar_width(raw), false),
            "the clamped 260 would NOT — this is why D9a keeps them separate"
        );
    }

    #[test]
    fn a_collapse_survives_the_pointer_running_far_past_it() {
        // The width-0 case D9's fixed grab offset exists for: `raw` keeps advancing while
        // the sidebar is shut, so the return trip is still measured from the same origin.
        let drag = ResizeDrag { grab_offset: 0.0 };
        assert!(next_collapsed(resize_raw_width(50.0, drag, 1.0), false));
        assert!(next_collapsed(resize_raw_width(-200.0, drag, 1.0), true));
        assert!(!next_collapsed(resize_raw_width(300.0, drag, 1.0), true));
    }

    // -----------------------------------------------------------------
    // tab-resize: echo suppression (D15/D15a) + the toggle chord (D17a)
    // -----------------------------------------------------------------

    #[test]
    fn our_own_write_is_recognised_as_an_echo() {
        // R9.1 — the exact pair we wrote comes back through the watcher.
        assert_eq!(
            classify_delivery((400.0, false), &[(400.0, false)]),
            ConfigDelivery::Echo
        );
    }

    #[test]
    fn the_echo_latch_is_not_consumed_by_the_first_match() {
        // R9.2 — FSEvents can report one rename several times. A one-shot rule would call
        // the duplicate a human edit and clear the override after EVERY drag; latched, the
        // classification is stable however many times it arrives.
        let latch: &[(f32, bool)] = &[(400.0, false)];
        for _ in 0..3 {
            assert_eq!(
                classify_delivery((400.0, false), latch),
                ConfigDelivery::Echo
            );
        }
    }

    #[test]
    fn a_different_value_is_a_human_edit() {
        // R9.3 — this is what keeps config.toml hand-editable after the first drag.
        assert_eq!(
            classify_delivery((300.0, false), &[(400.0, false)]),
            ConfigDelivery::HumanEdit
        );
        assert!(should_clear_override(ConfigDelivery::HumanEdit, false));
    }

    #[test]
    fn a_partial_match_is_a_human_edit_not_an_echo() {
        // R9.4 — the pair is compared whole: same width, different collapsed state is a
        // real edit. (Under a "differs only in that field" reading it would be dropped.)
        assert_eq!(
            classify_delivery((400.0, true), &[(400.0, false)]),
            ConfigDelivery::HumanEdit
        );
    }

    #[test]
    fn nothing_written_yet_means_every_delivery_is_a_human_edit() {
        // Before the first drag the override is None anyway, so the file rightly wins.
        assert_eq!(
            classify_delivery((340.0, false), &[]),
            ConfigDelivery::HumanEdit
        );
    }

    #[test]
    fn an_echo_never_clears_the_override() {
        // R9.6 — suppression and precedence are the same distinction, decided once.
        assert!(!should_clear_override(ConfigDelivery::Echo, false));
        assert!(!should_clear_override(ConfigDelivery::Echo, true));
    }

    #[test]
    fn a_failed_write_stops_an_unrelated_reload_snapping_the_sidebar_back() {
        // R9.5 / D14 × D15a — the contradiction the L2 review caught. After a failed write
        // the file still holds the OLD pair, so the next reload for any unrelated reason
        // (a font.size_pt edit) delivers a non-echo. Clearing the override there would undo
        // the gesture D14 just promised to keep.
        assert!(!should_clear_override(ConfigDelivery::HumanEdit, true));
        // …and once a write succeeds again, normal precedence resumes.
        assert!(should_clear_override(ConfigDelivery::HumanEdit, false));
    }

    #[test]
    fn suppression_keys_on_the_whole_config_not_the_sidebar_pair() {
        // **The BUG-1 regression.** Pair-only suppression shipped and killed live settings:
        // the latch is armed on every successful persist, the file then matches it
        // forever, so every later reload of ANY field classified as an echo and was
        // dropped. One click of Hide and your font/theme/grid edits stopped applying.
        //
        // This pins the distinction the fix rests on: the sidebar pair alone CANNOT tell a
        // real change from an echo, so the suppression decision must compare whole configs.
        let mut applied = common::config::Config::default();
        applied.sidebar.width_px = 400.0;
        applied.sidebar.collapsed = false;
        let latch: &[(f32, bool)] = &[(400.0, false)];

        // A font edit arriving after our own sidebar write: the PAIR is unchanged…
        let mut font_edit = applied.clone();
        font_edit.font.size_pt = 18.0;
        assert_eq!(
            classify_delivery(
                (font_edit.sidebar.width_px, font_edit.sidebar.collapsed),
                latch
            ),
            ConfigDelivery::Echo,
            "the pair predicate alone calls this an echo — which is exactly the trap"
        );
        // …but the config as a whole differs, which is what the event loop now tests, so
        // the font change is applied instead of being silently swallowed.
        assert_ne!(
            font_edit, applied,
            "whole-config comparison sees the change"
        );

        // A true echo — nothing changed at all — still compares equal and is suppressed.
        assert_eq!(applied.clone(), applied);

        // And the pair predicate still does its own job: deciding override precedence.
        let mut sidebar_edit = applied.clone();
        sidebar_edit.sidebar.width_px = 300.0;
        assert_eq!(
            classify_delivery(
                (
                    sidebar_edit.sidebar.width_px,
                    sidebar_edit.sidebar.collapsed
                ),
                latch
            ),
            ConfigDelivery::HumanEdit,
            "a hand-edited width still clears the override (D15a)"
        );
    }

    #[test]
    fn the_production_suppression_predicate_drops_only_true_no_ops() {
        // R9.1 against the predicate the event loop ACTUALLY calls. The earlier tests
        // exercised `classify_delivery`, which after D15's amendment decides override
        // precedence, not suppression — so the behaviour that broke live settings had no
        // test covering it at all.
        let mut applied = common::config::Config::default();
        applied.sidebar.width_px = 400.0;

        // A true echo: byte-for-byte the config we already applied.
        assert!(suppress_delivery(&applied.clone(), &applied));

        // Our own sidebar write coming back while ANOTHER field also changed must NOT be
        // suppressed — this is precisely the case that died before.
        let mut font_edit = applied.clone();
        font_edit.font.size_pt = 18.0;
        assert!(
            !suppress_delivery(&font_edit, &applied),
            "a font edit must survive even when the sidebar pair is unchanged"
        );

        // Every other field, too — the bug swallowed all of them.
        let mut theme_edit = applied.clone();
        theme_edit.theme.background = 0x11_22_33;
        assert!(!suppress_delivery(&theme_edit, &applied));
        let mut grid_edit = applied.clone();
        grid_edit.grid.columns = 200;
        assert!(!suppress_delivery(&grid_edit, &applied));

        // And a genuine sidebar edit is applied as well.
        let mut sidebar_edit = applied.clone();
        sidebar_edit.sidebar.width_px = 300.0;
        assert!(!suppress_delivery(&sidebar_edit, &applied));
    }

    #[test]
    fn a_stale_echo_from_an_earlier_write_is_still_an_echo() {
        // Found by the Gate-3 re-check. FSEvents delivers asynchronously, so two persists
        // inside one round-trip window leave the FIRST write's echo in flight while the
        // latch already holds the SECOND. Remembering only the newest write classified
        // that stale echo as a human edit → cleared the override → snapped the sidebar
        // back to the older width before the second echo corrected it: a visible flicker
        // and two of the atlas rebuilds D15 exists to prevent.
        let mut recent = Vec::new();
        remember_write(&mut recent, (400.0, false)); // drag 1 released
        remember_write(&mut recent, (420.0, false)); // drag 2 released, echo 1 still queued
        assert_eq!(
            classify_delivery((400.0, false), &recent),
            ConfigDelivery::Echo,
            "the in-flight echo of the FIRST write is ours, not a human edit"
        );
        assert_eq!(
            classify_delivery((420.0, false), &recent),
            ConfigDelivery::Echo
        );
        // A value we never wrote is still a human edit.
        assert_eq!(
            classify_delivery((300.0, false), &recent),
            ConfigDelivery::HumanEdit
        );
    }

    #[test]
    fn the_write_history_is_bounded_and_deduplicated() {
        let mut recent = Vec::new();
        for w in [300.0, 310.0, 320.0, 330.0, 340.0, 350.0] {
            remember_write(&mut recent, (w, false));
        }
        assert_eq!(recent.len(), RECENT_WRITES, "bounded — cannot grow forever");
        assert!(
            !recent.contains(&(300.0, false)),
            "the oldest write is evicted"
        );
        assert!(recent.contains(&(350.0, false)), "the newest is kept");
        // Re-writing a value already present moves it to newest without growing the ring.
        let before = recent.len();
        remember_write(&mut recent, (340.0, false));
        assert_eq!(recent.len(), before);
        assert_eq!(recent.last(), Some(&(340.0, false)));
    }

    #[test]
    fn cmd_shift_s_toggles_the_sidebar() {
        // R10.4
        let cmd_shift = ModifiersState::SUPER | ModifiersState::SHIFT;
        assert!(is_sidebar_toggle_chord(
            &Key::Character("s".into()),
            &cmd_shift
        ));
        assert!(is_sidebar_toggle_chord(
            &Key::Character("S".into()),
            &cmd_shift
        ));
    }

    #[test]
    fn the_toggle_chord_does_not_collide_or_reach_the_shell() {
        // R10.5 — verified against the live inventory, not by assertion in prose.
        let cmd = ModifiersState::SUPER;
        let cmd_shift = ModifiersState::SUPER | ModifiersState::SHIFT;
        let plain = ModifiersState::empty();

        // Not a tab chord, in either modifier combination.
        assert!(tab_action(&Key::Character("s".into()), &cmd_shift).is_none());
        assert!(tab_action(&Key::Character("s".into()), &cmd).is_none());
        // Every shipped tab chord still resolves — the new chord shadows none of them.
        for (c, want) in [
            ("t", TabAction::New),
            ("w", TabAction::CloseActive),
            ("[", TabAction::Prev),
            ("]", TabAction::Next),
        ] {
            assert_eq!(tab_action(&Key::Character(c.into()), &cmd), Some(want));
        }
        // Plain `s` and Cmd+S are NOT the chord, so ordinary typing is untouched.
        assert!(!is_sidebar_toggle_chord(
            &Key::Character("s".into()),
            &plain
        ));
        assert!(!is_sidebar_toggle_chord(&Key::Character("s".into()), &cmd));
        // And it is distinct from the two existing Cmd+Shift chrome toggles (M, R).
        assert!(!is_sidebar_toggle_chord(
            &Key::Character("m".into()),
            &cmd_shift
        ));
        assert!(!is_sidebar_toggle_chord(
            &Key::Character("r".into()),
            &cmd_shift
        ));
    }

    #[test]
    fn a_sub_cell_width_change_does_not_refit() {
        // R5.1 — the guard that keeps a per-pixel drag from wiping the selection and
        // storming zsh with SIGWINCH.
        assert!(!needs_refit((120, 40), (120, 40)));
    }

    #[test]
    fn crossing_a_column_boundary_refits() {
        // R5.2
        assert!(needs_refit((120, 40), (119, 40)));
        assert!(needs_refit((120, 40), (120, 39)));
    }

    #[test]
    fn a_release_routes_to_exactly_one_gesture() {
        // R7.3 — the precedence the two `Released` arms guard on.
        assert_eq!(release_target(false, false), ReleaseTarget::Neither);
        assert_eq!(release_target(true, false), ReleaseTarget::Resize);
        assert_eq!(release_target(false, true), ReleaseTarget::Selection);
    }

    #[test]
    fn a_resize_release_wins_over_a_stale_selection_drag() {
        // R7.2/R7.3 — the L2 reviewer's b4 case. Nothing clears `dragging` on
        // Focused(false), so it can be stale; the resize is the gesture holding an
        // uncommitted config write, so it must be the one that gets its cleanup.
        assert_eq!(release_target(true, true), ReleaseTarget::Resize);
    }

    #[test]
    fn a_plain_selection_release_is_untouched_by_the_new_arm() {
        // R7.7 — the regression guard: adding the resize arm must not shadow selection.
        assert_eq!(release_target(false, true), ReleaseTarget::Selection);
        assert_ne!(release_target(false, true), ReleaseTarget::Resize);
    }

    #[test]
    fn the_width_tracks_the_pointer_through_a_fixed_grab_offset() {
        // R3.1 — the spec's worked example: armed at pointer 345 against edge 340, so the
        // offset is 5; at pointer 405 the width is 400.
        let drag = ResizeDrag {
            grab_offset: resize_grab_offset(345.0, 340.0),
        };
        assert_eq!(drag.grab_offset, 5.0);
        assert_eq!(resize_raw_width(405.0, drag, 1.0), 400.0);
        assert_eq!(
            clamp_sidebar_width(resize_raw_width(405.0, drag, 1.0)),
            400.0
        );
    }

    #[test]
    fn the_edge_stays_welded_to_the_grabbed_point() {
        // R3.1 — grabbing anywhere in the band tracks without a jump: the width equals the
        // pointer minus the SAME offset throughout.
        for grab_x in [334.0_f64, 337.0, 339.9] {
            let drag = ResizeDrag {
                grab_offset: resize_grab_offset(grab_x, 340.0),
            };
            // Moving the pointer by +50 moves the width by exactly +50, whatever the grab.
            let before = resize_raw_width(grab_x, drag, 1.0);
            let after = resize_raw_width(grab_x + 50.0, drag, 1.0);
            assert_eq!(before, 340.0, "starts at the edge, no jump");
            assert_eq!(after - before, 50.0);
        }
    }

    #[test]
    fn the_applied_width_is_clamped_at_both_ends() {
        // R3.2
        let drag = ResizeDrag { grab_offset: 0.0 };
        assert_eq!(
            clamp_sidebar_width(resize_raw_width(700.0, drag, 1.0)),
            440.0
        );
        assert_eq!(
            clamp_sidebar_width(resize_raw_width(100.0, drag, 1.0)),
            260.0
        );
        // Exactly on the bounds passes through.
        assert_eq!(clamp_sidebar_width(260.0), 260.0);
        assert_eq!(clamp_sidebar_width(440.0), 440.0);
    }

    #[test]
    fn overshoot_is_not_accumulated() {
        // R3.3 — pushing 260px past the maximum then coming back 200px must NOT move the
        // edge: the pointer has to return to where the edge actually is. This is the
        // absolute-mapping property; a delta-accumulating model would have moved it.
        let drag = ResizeDrag { grab_offset: 0.0 };
        assert_eq!(
            clamp_sidebar_width(resize_raw_width(700.0, drag, 1.0)),
            440.0
        );
        assert_eq!(
            clamp_sidebar_width(resize_raw_width(500.0, drag, 1.0)),
            440.0,
            "still pinned — 500 is still past the 440 edge"
        );
        // It resumes only once the pointer is back inside the band.
        assert_eq!(
            clamp_sidebar_width(resize_raw_width(430.0, drag, 1.0)),
            430.0
        );
    }

    #[test]
    fn the_raw_value_is_not_clamped_so_collapse_thresholds_can_see_it() {
        // R4.4 / D9a — the regression test for an outright contradiction the L2 review
        // caught: if `resize_raw_width` clamped, `raw` could never be below 260 and D12's
        // 220 collapse threshold would be unreachable dead code.
        let drag = ResizeDrag { grab_offset: 0.0 };
        let raw = resize_raw_width(200.0, drag, 1.0);
        assert_eq!(raw, 200.0, "raw must stay BELOW the 260 minimum");
        assert!(raw < common::config::SIDEBAR_WIDTH_MIN);
        assert_eq!(
            clamp_sidebar_width(raw),
            260.0,
            "only the applied value clamps"
        );
    }

    #[test]
    fn the_mapping_is_in_logical_px_across_scales() {
        // D7 — the band and the persisted value are logical, so a 2x display must not
        // halve the usable range. At scale 2, physical 800 is logical 400.
        let drag = ResizeDrag { grab_offset: 0.0 };
        assert_eq!(resize_raw_width(800.0, drag, 2.0), 400.0);
        assert_eq!(resize_raw_width(400.0, drag, 1.0), 400.0);
    }

    #[test]
    fn a_degenerate_scale_does_not_poison_the_width() {
        // Defensive: a zero/negative scale must not divide by zero into the uniforms.
        let drag = ResizeDrag { grab_offset: 0.0 };
        assert_eq!(resize_raw_width(400.0, drag, 0.0), 400.0);
        assert!(clamp_sidebar_width(f32::NAN).is_finite());
        assert!(clamp_sidebar_width(f32::INFINITY).is_finite());
    }

    #[test]
    fn a_wheel_over_the_band_scrolls_the_list() {
        // R6.1/D13a — the band lies OVER the card list; a dead stripe there would stop
        // the list scrolling under a pointer the user cannot see is "on" anything.
        let inp = layout_input(340.0, 40); // overflows → scrollable
        let over_band = (in_band(340.0), 400.0);
        // The routing predicate the event loop uses: Some(region) → scroll the sidebar.
        assert!(sidebar_region(over_band, &inp, 0.0, false).is_some());
        let scrolled = wheel_scroll(WheelDelta::Lines(-1.0), over_band, 0.0, &inp, false);
        assert!(scrolled > 0.0, "wheel over the band scrolls the list");
    }
}

#[cfg(test)]
mod alt_screen_scroll_tests {
    use super::*;

    fn s(b: Vec<u8>) -> String {
        String::from_utf8_lossy(&b).replace('\x1b', "ESC")
    }

    #[test]
    fn sgr_wheel_reports_use_button_64_and_65_with_1_based_coords() {
        // Wheel-up is button 64, wheel-down 65; SGR coordinates are 1-based, so the
        // 0-based cell (0,0) reports as 1;1. Press-only — always the `M` final byte.
        assert_eq!(
            s(encode_wheel_report(MouseEncoding::Sgr, true, 0, 0).unwrap()),
            "ESC[<64;1;1M"
        );
        assert_eq!(
            s(encode_wheel_report(MouseEncoding::Sgr, false, 0, 0).unwrap()),
            "ESC[<65;1;1M"
        );
        assert_eq!(
            s(encode_wheel_report(MouseEncoding::Sgr, true, 41, 12).unwrap()),
            "ESC[<64;42;13M"
        );
        // SGR has no coordinate ceiling — this is why apps ask for `?1006`.
        assert_eq!(
            s(encode_wheel_report(MouseEncoding::Sgr, false, 999, 500).unwrap()),
            "ESC[<65;1000;501M"
        );
    }

    #[test]
    fn legacy_wheel_reports_offset_every_field_by_32() {
        // `ESC [ M` then three raw bytes, each value + 32.
        let b = encode_wheel_report(MouseEncoding::Legacy, true, 0, 0).unwrap();
        assert_eq!(b, vec![0x1b, b'[', b'M', 64 + 32, 1 + 32, 1 + 32]);
        let b = encode_wheel_report(MouseEncoding::Legacy, false, 9, 4).unwrap();
        assert_eq!(b, vec![0x1b, b'[', b'M', 65 + 32, 10 + 32, 5 + 32]);
    }

    #[test]
    fn legacy_drops_a_report_it_cannot_encode() {
        // Past 223 the byte would wrap and the app would read a click somewhere else
        // entirely — dropping is the only honest option. SGR has no such limit.
        assert!(encode_wheel_report(MouseEncoding::Legacy, true, 300, 0).is_none());
        assert!(encode_wheel_report(MouseEncoding::Legacy, true, 0, 300).is_none());
        assert!(encode_wheel_report(MouseEncoding::Sgr, true, 300, 300).is_some());
    }
}

/// Where a wheel gesture over the grid should go (alt-screen-scroll).
///
/// Extracted as a pure function so the routing rule is unit-testable without a
/// window, a GPU, or a PTY.
///
/// **Only mouse tracking diverts the wheel.** Translating it to arrow keys for an
/// untracked alt-screen app (iTerm2's `alternateScrollMode`) is deliberately NOT
/// done: `terminal-scroll` D5 weighed and rejected it because it writes to the PTY,
/// which `scrollback` D4 forbids outright, and it would break `terminal-scroll`
/// R5.1 — under `?1049h` the wheel must still reveal the frozen primary history.
/// Sending a report to an app that *asked* for one is a different matter: it is
/// additive, and revives a gesture that was previously dead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WheelRoute {
    /// The app asked for mouse events — send a report per line.
    Report,
    /// Nobody asked — the terminal's own scrollback viewport, alt screen included.
    Viewport,
}

pub fn wheel_route(mouse_tracking: bool, _alt_screen: bool) -> WheelRoute {
    if mouse_tracking {
        WheelRoute::Report
    } else {
        WheelRoute::Viewport
    }
}

#[cfg(test)]
mod wheel_route_tests {
    use super::*;

    #[test]
    fn tracking_wins_over_alt_screen() {
        // The case that motivated the feature: Claude Code is on the alt screen AND
        // has tracking on. It must get reports — arrows would move its input cursor
        // instead of scrolling the transcript.
        assert_eq!(wheel_route(true, true), WheelRoute::Report);
        // Tracking on the primary screen (rare, but legal) also wins.
        assert_eq!(wheel_route(true, false), WheelRoute::Report);
    }

    #[test]
    fn alt_screen_without_tracking_still_uses_the_scrollback_viewport() {
        // `terminal-scroll` R5.1: under `?1049h` the wheel must keep revealing the
        // frozen primary history. An earlier revision routed this to arrow keys
        // (iTerm2's alternateScrollMode) — which `terminal-scroll` D5 had already
        // weighed and rejected, because it writes to the PTY and `scrollback` D4
        // forbids scrolling from writing at all. This pins the rejection.
        assert_eq!(wheel_route(false, true), WheelRoute::Viewport);
    }

    #[test]
    fn primary_screen_without_tracking_keeps_the_scrollback_viewport() {
        // The regression direction that matters most: the shipped terminal-scroll
        // behavior must be untouched, and nothing may be written to the PTY.
        assert_eq!(wheel_route(false, false), WheelRoute::Viewport);
    }
}

/// The bytes a wheel gesture should write to the PTY, or `None` when the terminal
/// keeps the gesture for its own scrollback viewport (alt-screen-scroll).
///
/// This is the whole decision — routing *and* encoding — in one pure function, so
/// `TerminalApp::wheel_to_app` is reduced to "ask, then write" and the behavior is
/// testable without a window, a GPU or a PTY.
///
/// `None` means the caller must fall through to the viewport. That covers both "no
/// app asked for the wheel" and "the legacy encoding cannot express this column",
/// because in each case scrolling the viewport is better than silently eating the
/// gesture.
pub fn wheel_bytes(
    mouse_tracking: bool,
    encoding: MouseEncoding,
    n: i32,
    (col, row): (u16, u16),
) -> Option<Vec<u8>> {
    if n == 0 || wheel_route(mouse_tracking, false) != WheelRoute::Report {
        return None;
    }
    let up = n > 0;
    // Clamped: a pathological delta must not emit thousands of reports in one
    // gesture. Real winit deltas give n <= ~10.
    let lines = n.unsigned_abs().min(50);
    let one = encode_wheel_report(encoding, up, col, row)?;
    let mut out = Vec::with_capacity(one.len() * lines as usize);
    for _ in 0..lines {
        out.extend_from_slice(&one);
    }
    Some(out)
}

#[cfg(test)]
mod wheel_bytes_tests {
    use super::*;

    fn s(b: Vec<u8>) -> String {
        String::from_utf8_lossy(&b).replace('\x1b', "ESC")
    }

    #[test]
    fn claude_codes_mode_set_produces_sgr_reports() {
        // The end-to-end decision for the reported symptom: tracking on + SGR
        // encoding → one report per line, at the pointer cell.
        let b = wheel_bytes(true, MouseEncoding::Sgr, 3, (41, 12)).expect("reports");
        assert_eq!(s(b), "ESC[<64;42;13MESC[<64;42;13MESC[<64;42;13M");
        let b = wheel_bytes(true, MouseEncoding::Sgr, -1, (0, 0)).expect("reports");
        assert_eq!(s(b), "ESC[<65;1;1M", "negative n is wheel-down");
    }

    #[test]
    fn no_tracking_keeps_the_gesture_for_the_viewport() {
        // The regression direction that matters most: with no app asking, the wheel
        // must produce NO bytes at all, so the caller scrolls scrollback instead.
        assert!(wheel_bytes(false, MouseEncoding::Sgr, 3, (0, 0)).is_none());
        assert!(wheel_bytes(false, MouseEncoding::Legacy, -3, (0, 0)).is_none());
    }

    #[test]
    fn a_zero_delta_writes_nothing() {
        assert!(wheel_bytes(true, MouseEncoding::Sgr, 0, (0, 0)).is_none());
    }

    #[test]
    fn an_unencodable_column_falls_back_to_the_viewport() {
        // Legacy encoding past 223 columns: returning None makes the caller scroll
        // rather than swallowing the gesture, which would leave the right-hand side
        // of a wide window inert for no visible reason.
        assert!(wheel_bytes(true, MouseEncoding::Legacy, 1, (300, 0)).is_none());
        assert!(wheel_bytes(true, MouseEncoding::Sgr, 1, (300, 0)).is_some());
    }

    #[test]
    fn the_per_gesture_line_count_is_clamped() {
        let b = wheel_bytes(true, MouseEncoding::Sgr, 100_000, (0, 0)).expect("reports");
        assert_eq!(b.len(), "\x1b[<64;1;1M".len() * 50, "clamped at 50 reports");
    }
}
