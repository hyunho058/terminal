//! Keyboard translation: tab chords (D5, R3.2) and shell key encoding.
//!
//! Both are pure functions over winit's logical `Key` + `ModifiersState`, so the
//! chord table is unit-tested without a window.

use renderer::sidebar_layout::{self, ChromeRegion, SidebarLayoutInput, SidebarRegion};
use winit::keyboard::{Key, ModifiersState, NamedKey};

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
        SidebarRegion::Chrome(ChromeRegion::NewTab) => SidebarAction::NewTab,
        SidebarRegion::Chrome(ChromeRegion::HideSidebar) => SidebarAction::Collapse,
        SidebarRegion::Chrome(ChromeRegion::Restore) => SidebarAction::Expand,
        SidebarRegion::Panel => SidebarAction::Inert,
    })
}

/// Resolve the card index under the cursor for the row layer's `hovered` highlight (D8).
/// Chrome hover is driven separately by the chrome layer's own raw-cursor handle
/// (chrome-collapse D10 as shipped), so this returns only the card index.
pub fn sidebar_hover(
    cursor: (f64, f64),
    inp: &SidebarLayoutInput,
    scroll: f32,
    collapsed: bool,
) -> Option<usize> {
    let p = [cursor.0 as f32, cursor.1 as f32];
    match sidebar_layout::hit_test(p, scroll, inp, collapsed) {
        Some(SidebarRegion::Card(i)) => Some(i),
        _ => None,
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
        }
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
}
