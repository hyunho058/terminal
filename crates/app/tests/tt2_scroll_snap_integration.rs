//! Acceptance tests for scroll-snap-on-click R0 and R2 (D2).
//!
//! Covers:
//! - R0.1, R0.2: `Terminal` scroll-to-bottom mechanics (`scroll_to_bottom`, `at_bottom`, `view_offset`).
//! - R2.1, R2.2: Event loop structural invariants in `terminal_app.rs` under `ReleaseTarget::Selection`:
//!   1. `drag::is_click(inner.drag, active)` is read before `inner.drag.take()`.
//!   2. `should_snap_on_click` is evaluated before `drag::release`.
//!   3. `inner.snap_tab_to_live(active)` is executed under `if should_snap` independently after `drag_release_snap(latch)`.

use app::should_snap_on_click;
use term_core::{Color, Terminal};
use winit::keyboard::ModifiersState;

/// R0.1 & R0.2: Verify `Terminal` scrollback offset and snap-to-bottom behavior.
#[test]
fn r0_terminal_scroll_to_bottom_semantics() {
    let mut term = Terminal::new(80, 24, Color(0xE0E0E0), Color(0x101010));
    // Write 100 lines to produce scrollback
    for i in 0..100 {
        term.advance(format!("line {}\r\n", i).as_bytes());
    }

    assert!(term.at_bottom(), "Initially at live bottom");
    assert_eq!(term.view_offset(), 0);

    // Scroll back 15 lines
    term.scroll_up(15);
    assert!(!term.at_bottom(), "Scrolled back");
    assert_eq!(term.view_offset(), 15);

    // should_snap_on_click returns true
    let mods = ModifiersState::empty();
    assert!(should_snap_on_click(true, false, &mods, term.at_bottom()));

    // Snapping resets view_offset to 0 and restores at_bottom
    term.scroll_to_bottom();
    assert!(term.at_bottom());
    assert_eq!(term.view_offset(), 0);

    // Once at bottom, should_snap_on_click returns false
    assert!(!should_snap_on_click(true, false, &mods, term.at_bottom()));
}

/// Helper extracting an arm body from source text (mirroring drag.rs).
fn extract_arm<'a>(source: &'a str, pat: &str) -> &'a str {
    let start = source
        .find(pat)
        .unwrap_or_else(|| panic!("missing arm pattern: {pat}"));
    &source[start..]
}

/// R2.1 & R2.2: Structural verification of the event loop under `ReleaseTarget::Selection`.
///
/// Invariants:
/// 1. `let is_click = drag::is_click(inner.drag, active)` MUST precede `inner.drag.take()`.
/// 2. `should_snap_on_click` MUST be evaluated before `drag::release`.
/// 3. `inner.snap_tab_to_live(active)` MUST be called in an independent `if should_snap` block
///    following `drag_release_snap(latch)`.
#[test]
fn r2_selection_release_event_loop_invariants() {
    let src = include_str!("../src/terminal_app.rs");
    let (production, _) = src
        .split_once("\n#[cfg(test)]\nmod tests {")
        .expect("terminal_app.rs has a test module to split on");

    // Comments stripped
    let code = production
        .lines()
        .map(|l| l.split("//").next().unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");

    let arm = extract_arm(&code, "ReleaseTarget::Selection =>");

    // Invariant 1: is_click is bound before inner.drag.take()
    let is_click_at = arm
        .find("let is_click = drag::is_click(inner.drag, active);")
        .expect("must bind `let is_click = drag::is_click(inner.drag, active);`");
    let drag_take_at = arm
        .find("inner.drag.take()")
        .expect("must take inner.drag into release");
    assert!(
        is_click_at < drag_take_at,
        "`is_click` must be evaluated before `inner.drag.take()` consumes the drag"
    );

    // Invariant 2: should_snap is evaluated
    let should_snap_at = arm
        .find("should_snap_on_click(")
        .expect("must call `should_snap_on_click`");
    let drag_release_at = arm
        .find("drag::release(")
        .expect("must call `drag::release`");
    assert!(
        should_snap_at < drag_release_at,
        "`should_snap_on_click` must be evaluated before `drag::release`"
    );

    // Invariant 3: both latch snap and should_snap are executed independently
    let latch_snap_at = arm
        .find("drag_release_snap(latch)")
        .expect("must evaluate `drag_release_snap(latch)`");
    let active_snap_at = arm
        .find("inner.snap_tab_to_live(active)")
        .expect("must call `inner.snap_tab_to_live(active)`");
    assert!(
        latch_snap_at < active_snap_at,
        "latch snap must precede active tab click snap"
    );

    assert!(
        arm.contains("if should_snap {\n                    inner.snap_tab_to_live(active);\n                }")
            || arm.contains("if should_snap {\n                    inner.snap_tab_to_live(active);"),
        "active tab snap must be governed by `if should_snap`"
    );
}
