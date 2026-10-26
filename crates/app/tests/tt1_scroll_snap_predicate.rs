//! Acceptance tests for requirement R1 (D1): pure decision predicate `should_snap_on_click`.
//!
//! Covers R1.1 to R1.5:
//! - R1.1: Unmodified click while scrolled back returns true
//! - R1.2: Moving drag returns false
//! - R1.3: Click over lit hyperlink returns false
//! - R1.4: Modifiers (Shift, Super, Alt, Ctrl) return false
//! - R1.5: Already at bottom returns false

use app::should_snap_on_click;
use winit::keyboard::ModifiersState;

/// R1.1: Unmodified click while scrolled back returns true.
///
/// Given: `is_click = true`, `has_url = false`, `mods = ModifiersState::empty()`, and `at_bottom = false`.
/// When: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called.
/// Then: Returns `true`.
#[test]
fn r1_1_unmodified_click_while_scrolled_back() {
    let mods = ModifiersState::empty();
    assert!(should_snap_on_click(true, false, &mods, false));
}

/// R1.2: Moving drag returns false.
///
/// Given: `is_click = false`, `has_url = false`, `mods = ModifiersState::empty()`, and `at_bottom = false`.
/// When: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called.
/// Then: Returns `false`.
#[test]
fn r1_2_moving_drag_returns_false() {
    let mods = ModifiersState::empty();
    assert!(!should_snap_on_click(false, false, &mods, false));
}

/// R1.3: Click over lit hyperlink returns false.
///
/// Given: `is_click = true`, `has_url = true`, `mods = ModifiersState::SUPER`, and `at_bottom = false`.
/// When: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called.
/// Then: Returns `false`.
#[test]
fn r1_3_hyperlink_click_returns_false() {
    let mut mods = ModifiersState::empty();
    mods.insert(ModifiersState::SUPER);
    assert!(!should_snap_on_click(true, true, &mods, false));

    // Even if no modifiers were reported on release, has_url=true suppresses snap
    assert!(!should_snap_on_click(
        true,
        true,
        &ModifiersState::empty(),
        false
    ));
}

/// R1.4: Modifiers (Shift, Super, Alt, Ctrl) return false.
///
/// Given: `is_click = true`, `has_url = false`, `at_bottom = false`, and `mods` is non-empty.
/// When: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called.
/// Then: Returns `false`.
#[test]
fn r1_4_modifiers_return_false() {
    for modifier in [
        ModifiersState::SHIFT,
        ModifiersState::SUPER,
        ModifiersState::ALT,
        ModifiersState::CONTROL,
    ] {
        let mut mods = ModifiersState::empty();
        mods.insert(modifier);
        assert!(
            !should_snap_on_click(true, false, &mods, false),
            "Expected modifier {:?} to inhibit snap-to-bottom",
            modifier
        );
    }
}

/// R1.5: Already at bottom returns false.
///
/// Given: `is_click = true`, `has_url = false`, `mods = ModifiersState::empty()`, and `at_bottom = true`.
/// When: `should_snap_on_click(is_click, has_url, &mods, at_bottom)` is called.
/// Then: Returns `false`.
#[test]
fn r1_5_already_at_bottom_returns_false() {
    let mods = ModifiersState::empty();
    assert!(!should_snap_on_click(true, false, &mods, true));
}
