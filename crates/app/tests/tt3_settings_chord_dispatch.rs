//! Acceptance tests for requirement R3 (top-settings-menu D4/R3): Cmd+, chord identification.
//!
//! Covers R3.1:
//! - Given: A keyboard input event with modifier `ModifiersState::SUPER` and key `,`
//! - When: `app::is_settings_chord(&key, &mods)` is called
//! - Then: Returns `true` for `Cmd+,` and `false` for `,` without Cmd, `Cmd+Shift+,`,
//!   `Cmd+Alt+,`, `Cmd+Ctrl+,`, and other characters with Cmd (e.g. `Cmd+s`, `Cmd+t`, `Cmd+w`).

use app::is_settings_chord;
use winit::keyboard::{Key, ModifiersState};

/// R3.1.1: Cmd+, triggers settings chord.
///
/// Given: Key `,` and modifiers containing only `ModifiersState::SUPER`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `true`.
#[test]
fn r3_1_cmd_comma_triggers_settings_chord() {
    let key = Key::Character(",".into());
    let mods = ModifiersState::SUPER;
    assert!(
        is_settings_chord(&key, &mods),
        "Cmd+, must be recognized as the settings chord"
    );
}

/// R3.1.2: Bare comma `,` without Cmd modifier does not trigger settings chord.
///
/// Given: Key `,` and `ModifiersState::empty()`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_comma_without_cmd_returns_false() {
    let key = Key::Character(",".into());
    let mods = ModifiersState::empty();
    assert!(
        !is_settings_chord(&key, &mods),
        "Bare comma without Cmd must not trigger settings chord"
    );
}

/// R3.1.3: Cmd+Shift+, does not trigger settings chord.
///
/// Given: Key `,` and modifiers `SUPER | SHIFT`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_cmd_shift_comma_returns_false() {
    let key = Key::Character(",".into());
    let mods = ModifiersState::SUPER | ModifiersState::SHIFT;
    assert!(
        !is_settings_chord(&key, &mods),
        "Cmd+Shift+, must not trigger settings chord"
    );
}

/// R3.1.4: Cmd+Alt+, does not trigger settings chord.
///
/// Given: Key `,` and modifiers `SUPER | ALT`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_cmd_alt_comma_returns_false() {
    let key = Key::Character(",".into());
    let mods = ModifiersState::SUPER | ModifiersState::ALT;
    assert!(
        !is_settings_chord(&key, &mods),
        "Cmd+Alt+, must not trigger settings chord"
    );
}

/// R3.1.5: Cmd+Ctrl+, does not trigger settings chord.
///
/// Given: Key `,` and modifiers `SUPER | CONTROL`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_cmd_ctrl_comma_returns_false() {
    let key = Key::Character(",".into());
    let mods = ModifiersState::SUPER | ModifiersState::CONTROL;
    assert!(
        !is_settings_chord(&key, &mods),
        "Cmd+Ctrl+, must not trigger settings chord"
    );
}

/// R3.1.6: Cmd+s does not trigger settings chord.
///
/// Given: Key `s` and modifier `SUPER`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_cmd_s_returns_false() {
    let key = Key::Character("s".into());
    let mods = ModifiersState::SUPER;
    assert!(
        !is_settings_chord(&key, &mods),
        "Cmd+s must not trigger settings chord"
    );
}

/// R3.1.7: Cmd+t does not trigger settings chord.
///
/// Given: Key `t` and modifier `SUPER`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_cmd_t_returns_false() {
    let key = Key::Character("t".into());
    let mods = ModifiersState::SUPER;
    assert!(
        !is_settings_chord(&key, &mods),
        "Cmd+t must not trigger settings chord"
    );
}

/// R3.1.8: Cmd+w does not trigger settings chord.
///
/// Given: Key `w` and modifier `SUPER`.
/// When: `is_settings_chord(&key, &mods)` is called.
/// Then: Returns `false`.
#[test]
fn r3_1_cmd_w_returns_false() {
    let key = Key::Character("w".into());
    let mods = ModifiersState::SUPER;
    assert!(
        !is_settings_chord(&key, &mods),
        "Cmd+w must not trigger settings chord"
    );
}
