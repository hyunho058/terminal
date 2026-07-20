//! Keyboard translation: tab chords (D5, R3.2) and shell key encoding.
//!
//! Both are pure functions over winit's logical `Key` + `ModifiersState`, so the
//! chord table is unit-tested without a window.

use renderer::tab_bar;
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

/// Translate a left-mouse press into the tab it targets in the left sidebar, or
/// `None` if the press is outside the sidebar / below the last tab. This is the
/// pointer analogue of [`tab_action`] (raw device input → a tab intent): it casts
/// winit's `f64` physical cursor to `f32`, derives the per-tab slot height from
/// the cell metrics, and delegates the floor/bounds *geometry* to
/// [`tab_bar::tab_at`] — so the hit-test math lives in exactly one place (SD2),
/// the same functions the sidebar draws with. `sidebar_w` is the renderer's live
/// `left_inset`; `count` is the number of open tabs.
pub fn tab_target(
    cursor: (f64, f64),
    cell: (f32, f32),
    sidebar_w: f32,
    count: usize,
) -> Option<usize> {
    let tab_h = tab_bar::tab_height_px(cell.1);
    tab_bar::tab_at(cursor.0 as f32, cursor.1 as f32, sidebar_w, tab_h, count)
}

/// Translate a key press into the bytes a shell expects. Named keys first, then
/// fall back to the event's text (letters, symbols, IME output). Returns `None`
/// for keys with no shell encoding.
pub fn encode_key(logical: &Key, text: Option<&str>) -> Option<Vec<u8>> {
    match logical {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn encode_named_and_text() {
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Enter), None),
            Some(vec![b'\r'])
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Backspace), None),
            Some(vec![0x7f])
        );
        assert_eq!(encode_key(&ch("a"), Some("a")), Some(b"a".to_vec()));
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

    #[test]
    fn tab_target_maps_click_to_tab_and_delegates_geometry() {
        // cell (8, 20) → tab_h = tab_height_px(20) = 30; sidebar 160px, 3 tabs.
        // A click at y=65 (slot 2), x=40 (inside the band) hits tab 2.
        assert_eq!(tab_target((40.0, 65.0), (8.0, 20.0), 160.0, 3), Some(2));
        // A click past the sidebar width is not a tab click.
        assert_eq!(tab_target((200.0, 65.0), (8.0, 20.0), 160.0, 3), None);
        // With no tabs there is never a target.
        assert_eq!(tab_target((40.0, 10.0), (8.0, 20.0), 160.0, 0), None);
        // The result is exactly the tab_bar geometry (SD2 — one source of truth):
        // tab_target casts the f64 cursor and derives tab_h, then delegates.
        assert_eq!(
            tab_target((40.0, 65.0), (8.0, 20.0), 160.0, 3),
            tab_bar::tab_at(40.0, 65.0, 160.0, tab_bar::tab_height_px(20.0), 3),
        );
    }
}
