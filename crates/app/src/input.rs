//! Keyboard translation: tab chords (D5, R3.2) and shell key encoding.
//!
//! Both are pure functions over winit's logical `Key` + `ModifiersState`, so the
//! chord table is unit-tested without a window.

use renderer::sidebar_layout::{self, SidebarLayoutInput, SidebarRegion};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

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
/// Matched by **physical** key position first (`KeyCode`), so the chords fire under
/// a non-Latin IME — the Korean 2-beolsik layout maps T→`ㅅ`, W→`ㅈ`, etc., so the
/// logical char is not `t`/`w`. Falls back to the logical char when the platform
/// reports the physical key unidentified; bracket vs. brace is still accepted either
/// way there (`[`/`{`, `]`/`}`).
pub fn tab_action(
    key: &Key,
    physical: Option<KeyCode>,
    mods: &ModifiersState,
) -> Option<TabAction> {
    if !mods.super_key() {
        return None;
    }
    if let Some(action) = physical.and_then(physical_tab_action) {
        return Some(action);
    }
    let Key::Character(s) = key else {
        return None;
    };
    logical_tab_action(s.chars().next()?)
}

/// The tab chord for a physical key position (layout-independent), or `None`.
fn physical_tab_action(code: KeyCode) -> Option<TabAction> {
    Some(match code {
        KeyCode::KeyT => TabAction::New,
        KeyCode::KeyW => TabAction::CloseActive,
        KeyCode::BracketLeft => TabAction::Prev,
        KeyCode::BracketRight => TabAction::Next,
        KeyCode::Digit1 => TabAction::Jump(0),
        KeyCode::Digit2 => TabAction::Jump(1),
        KeyCode::Digit3 => TabAction::Jump(2),
        KeyCode::Digit4 => TabAction::Jump(3),
        KeyCode::Digit5 => TabAction::Jump(4),
        KeyCode::Digit6 => TabAction::Jump(5),
        KeyCode::Digit7 => TabAction::Jump(6),
        KeyCode::Digit8 => TabAction::Jump(7),
        KeyCode::Digit9 => TabAction::Jump(8),
        _ => return None,
    })
}

/// The tab chord for a logical character (the fallback when the physical key is
/// unidentified — e.g. a US layout where the char already equals the key).
fn logical_tab_action(c: char) -> Option<TabAction> {
    Some(match c {
        't' | 'T' => TabAction::New,
        'w' | 'W' => TabAction::CloseActive,
        '[' | '{' => TabAction::Prev,
        ']' | '}' => TabAction::Next,
        '1'..='9' => TabAction::Jump(c as usize - '1' as usize),
        _ => return None,
    })
}

/// What the app does about a pointer press (D1). Distinct from the renderer's *region*
/// enum. The new-tab / collapse / restore controls moved to the title bar
/// (titlebar-tab-controls R5), so the sidebar itself now only switches tabs, swallows
/// dead space (`Inert`), or starts a resize.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidebarAction {
    Switch(usize),
    Inert,
    /// A press on the resize grab-band — begin a live sidebar resize (sidebar-resize D6).
    BeginResize,
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
        SidebarRegion::Panel => SidebarAction::Inert,
        SidebarRegion::ResizeHandle => SidebarAction::BeginResize,
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

/// Whether a physical-pixel x lands on the terminal grid (right of the sidebar
/// band) rather than over the left sidebar. The edge itself counts as grid.
pub fn over_grid(cursor_x: f64, left_inset: f32) -> bool {
    cursor_x as f32 >= left_inset
}

/// Map a physical-pixel cursor position to a grid cell `(col, row)`, **clamped**
/// into `[0, cols) × [0, rows)`. The grid origin is offset by the sidebar
/// `left_inset` and `top_inset` (matching the renderer's forward `col*cell + inset`
/// mapping). A cursor left-of / above the grid clamps to column / row 0, and one
/// past the right / bottom edge clamps to the last cell — so a drag that runs off an
/// edge still selects to that edge. Returns `None` only for a zero-size grid.
pub fn cell_at(
    cursor: (f64, f64),
    cell: (f32, f32),
    left_inset: f32,
    top_inset: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 || cell.0 <= 0.0 || cell.1 <= 0.0 {
        return None;
    }
    let x = cursor.0 as f32 - left_inset;
    let y = cursor.1 as f32 - top_inset;
    let col = (x / cell.0).floor().clamp(0.0, (cols - 1) as f32) as u16;
    let row = (y / cell.1).floor().clamp(0.0, (rows - 1) as f32) as u16;
    Some((col, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn cell_at_maps_pixel_to_cell_with_insets() {
        // cell 10×20, left_inset 40, top_inset 30; cursor (65, 75):
        // col = floor((65-40)/10) = 2, row = floor((75-30)/20) = 2.
        assert_eq!(
            cell_at((65.0, 75.0), (10.0, 20.0), 40.0, 30.0, 80, 24),
            Some((2, 2))
        );
    }

    #[test]
    fn cell_at_clamps_off_every_edge() {
        let c = (10.0, 20.0);
        // Left of the grid → column 0 (row still maps: floor(100/20) = 5).
        assert_eq!(cell_at((5.0, 100.0), c, 40.0, 0.0, 80, 24), Some((0, 5)));
        // Far past the right / bottom edge → the last cell.
        assert_eq!(
            cell_at((100_000.0, 100_000.0), c, 40.0, 0.0, 80, 24),
            Some((79, 23))
        );
        // A zero-size grid has no cell.
        assert_eq!(cell_at((10.0, 10.0), c, 0.0, 0.0, 0, 0), None);
    }

    #[test]
    fn over_grid_excludes_the_sidebar_band() {
        assert!(over_grid(200.0, 160.0), "right of the band → grid");
        assert!(!over_grid(100.0, 160.0), "inside the band → sidebar");
        assert!(
            over_grid(160.0, 160.0),
            "exactly at the edge counts as grid"
        );
    }

    #[test]
    fn cmd_chords_map_to_actions() {
        // Logical fallback (physical unidentified) — the US-layout path.
        let cmd = ModifiersState::SUPER;
        let la = |c: &str| tab_action(&ch(c), None, &cmd);
        assert_eq!(la("t"), Some(TabAction::New));
        assert_eq!(la("w"), Some(TabAction::CloseActive));
        assert_eq!(la("]"), Some(TabAction::Next));
        assert_eq!(la("["), Some(TabAction::Prev));
        assert_eq!(la("}"), Some(TabAction::Next));
        assert_eq!(la("{"), Some(TabAction::Prev));
        assert_eq!(la("1"), Some(TabAction::Jump(0)));
        assert_eq!(la("9"), Some(TabAction::Jump(8)));
    }

    #[test]
    fn cmd_chords_map_by_physical_key_under_non_latin_ime() {
        // Korean 2-beolsik: Cmd+T yields the logical char `ㅅ`, Cmd+W → `ㅈ`, etc.
        // The physical key still resolves the chord (the regression this guards).
        let cmd = ModifiersState::SUPER;
        assert_eq!(
            tab_action(&ch("ㅅ"), Some(KeyCode::KeyT), &cmd),
            Some(TabAction::New),
            "Cmd+T must open a tab even when the IME maps T→ㅅ"
        );
        assert_eq!(
            tab_action(&ch("ㅈ"), Some(KeyCode::KeyW), &cmd),
            Some(TabAction::CloseActive)
        );
        assert_eq!(
            tab_action(&ch("`"), Some(KeyCode::BracketRight), &cmd),
            Some(TabAction::Next)
        );
        assert_eq!(
            tab_action(&ch("1"), Some(KeyCode::Digit1), &cmd),
            Some(TabAction::Jump(0))
        );
        // The physical key wins even if the logical char would map elsewhere.
        assert_eq!(
            tab_action(&ch("t"), Some(KeyCode::KeyW), &cmd),
            Some(TabAction::CloseActive)
        );
    }

    #[test]
    fn no_super_means_no_chord() {
        let none = ModifiersState::empty();
        assert_eq!(
            tab_action(&ch("t"), Some(KeyCode::KeyT), &none),
            None,
            "plain 't' types into shell"
        );
        // A non-chord key with Cmd held is also not a tab action.
        assert_eq!(
            tab_action(&ch("z"), Some(KeyCode::KeyZ), &ModifiersState::SUPER),
            None
        );
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
        SidebarLayoutInput {
            sidebar_w,
            viewport: [sidebar_w.max(200.0), 900.0],
            scale: 1.0,
            row_h: 60.0,
            row_count: count,
            metrics: renderer::sidebar_theme::SidebarMetrics::DEFAULT.scaled(1.0),
            top_inset: 0.0,
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
        // The header is now inert (its +/hide controls moved to the title bar, R5): a
        // press in the header band is swallowed, not a collapse.
        let hdr = (
            (inp.metrics.header_pad_x + 1.0) as f64,
            (sidebar_layout::header_h(1.0) / 2.0) as f64,
        );
        assert_eq!(
            sidebar_press(hdr, &inp, 0.0, false),
            Some(SidebarAction::Inert)
        );
        // A point over the grid is not the sidebar's business.
        assert_eq!(sidebar_press((400.0, 100.0), &inp, 0.0, false), None);
        // Collapsed: the sidebar exposes no target at all (restore is a title-bar control).
        assert_eq!(sidebar_press((20.0, 20.0), &inp, 0.0, true), None);
        assert_eq!(sidebar_press((400.0, 400.0), &inp, 0.0, true), None);
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
