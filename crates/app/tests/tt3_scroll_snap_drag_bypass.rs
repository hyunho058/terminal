//! Acceptance tests for scroll-snap-on-click R3 (D3): preservation of drags and modifier bypasses.
//!
//! Covers:
//! - R3.1: Text selection drag leaves selection standing and does not snap
//! - R3.2: ⌘-click opens link without snapping
//! - R3.3: Shift-click manual selection bypass does not snap

use app::{release_drag, release_is_click, should_snap_on_click, SelectionDrag, TabId};
use term_core::{Color, Terminal};
use winit::keyboard::ModifiersState;

/// Setup a scrolled-back terminal with sample content.
fn scrolled_terminal() -> Terminal {
    let mut term = Terminal::new(80, 24, Color(0xE0E0E0), Color(0x101010));
    for i in 0..50 {
        term.advance(format!("Row {:02} content here\r\n", i).as_bytes());
    }
    term.scroll_up(10);
    assert_eq!(term.view_offset(), 10);
    term
}

/// R3.1: Selection drag (`is_click == false`) does not snap to bottom and leaves selection intact.
#[test]
fn r3_1_text_selection_drag_does_not_snap() {
    let mut term = scrolled_terminal();
    let tab = TabId(1);
    let mut drag = SelectionDrag::press(tab, (5, 5), &term);
    // Pointer moves across cells
    drag.drive(tab, (15, 5), &mut term);

    let is_click = release_is_click(Some(drag), tab);
    assert!(!is_click, "Drag that moved must not be considered a click");

    let mods = ModifiersState::empty();
    let should_snap = should_snap_on_click(is_click, false, &mods, term.at_bottom());
    assert!(!should_snap, "Drag must not trigger snap-to-bottom");

    let cleared = release_drag(Some(drag), tab, &mut term);
    assert!(
        !cleared,
        "A real drag leaves the selection standing for Cmd+C"
    );

    // Terminal selection is set, view_offset is preserved
    assert!(term.selection().is_some());
    assert_eq!(term.view_offset(), 10);
    assert!(!term.at_bottom());
}

/// R3.2: ⌘-click (hyperlink opening) does not snap to bottom.
#[test]
fn r3_2_hyperlink_click_does_not_snap() {
    let term = scrolled_terminal();
    let mut mods = ModifiersState::empty();
    mods.insert(ModifiersState::SUPER);

    // Hyperlink lit, ⌘ held
    let should_snap = should_snap_on_click(true, true, &mods, term.at_bottom());
    assert!(!should_snap, "Hyperlink click must not snap to bottom");
    assert_eq!(term.view_offset(), 10);

    // Even if URL opened without Cmd modifier reported
    let should_snap_no_mods =
        should_snap_on_click(true, true, &ModifiersState::empty(), term.at_bottom());
    assert!(
        !should_snap_no_mods,
        "Any click with an opened URL must not snap"
    );
}

/// R3.3: Shift-click manual selection bypass does not snap to bottom.
#[test]
fn r3_3_shift_click_does_not_snap() {
    let term = scrolled_terminal();
    let mut mods = ModifiersState::empty();
    mods.insert(ModifiersState::SHIFT);

    let should_snap = should_snap_on_click(true, false, &mods, term.at_bottom());
    assert!(!should_snap, "Shift-click must not snap to bottom");
    assert_eq!(term.view_offset(), 10);
}
