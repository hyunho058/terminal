//! The left-drag selection gesture, as a value (selection-scroll-anchor D3).
//!
//! The event loop's press, `CursorMoved` and release arms route here and do nothing
//! else with the selection: they resolve the pointer to a viewport cell, hand it to
//! [`SelectionDrag`] with the active tab's [`Terminal`], and redraw when told to. So
//! every rule below is driven by this module's tests with a real `Terminal` and plain
//! cells, and no window — the idiom of `input::release_target` and
//! `terminal_app::drag_release_snap`.
//!
//! The gesture keeps **two** records of where it began, and they answer different
//! questions:
//!
//! - the **press cell**, a viewport `(col, row)`, answers "did the *pointer* move".
//!   That is `drag_moved`, exactly as `selection-input` D8 defines it, and hyperlinks'
//!   `url_to_open` reads the same flag to tell a click from a drag. It is never
//!   compared in content coordinates: under streaming output the text beneath a
//!   motionless pointer moves, so one jitter `CursorMoved` in the same cell would read
//!   as a drag, and a plain click would select instead of clearing (R3.2).
//! - the **anchor**, a content [`Point`] taken with `point_at` **at press time**,
//!   answers "which text did the button go down on". Converting the press cell later —
//!   at the first move, as the stop-gap this replaced did — anchors on whatever line
//!   output has since moved under that cell: the reported defect, reproduced inside a
//!   single drag (R3.1).

use term_core::{Point, Terminal};

use crate::session::TabId;

/// What one `CursorMoved` does to the drag's selection (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragMove {
    /// Write nothing: another tab is in front (R3.5), or the pointer has not yet left
    /// the press cell, so the gesture is still a click.
    Ignore,
    /// The selection this drag started has been cleared under it — a changed-text
    /// check (D4) or a resize (D7) — so the drag writes nothing more (R3.3). The text
    /// the press anchored on is gone, and re-creating from that anchor would highlight
    /// whatever occupies its line now.
    Spent,
    /// Set the drag tab's selection to this span.
    Set { anchor: Point, focus: Point },
}

/// One in-flight left-drag over the grid (selection-scroll-anchor D3).
///
/// Armed by [`press`](Self::press), advanced by [`drive`](Self::drive) on each
/// `CursorMoved`, and ended by the module's [`release`], which consumes it. The next
/// press builds a fresh one, so nothing here outlives its gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionDrag {
    /// The tab the press landed in. Only that tab's grid is ever written (R3.5): a
    /// tab switch with the button held must not carry this anchor — a line number in
    /// *this* tab's content — into another tab's grid, where it names unrelated text.
    tab: TabId,
    /// The viewport cell the press landed on: the reference for `drag_moved`.
    press_cell: (u16, u16),
    /// The content point under `press_cell` when the button went down.
    anchor: Point,
    /// `drag_moved`: the pointer has left `press_cell` at least once. Sticky — a drag
    /// that returns to its start cell is still a drag.
    moved: bool,
    /// `selection_started`: this drag has written a selection. Distinguishes "never
    /// set yet" from "set, then cleared", which `selection().is_none()` alone cannot —
    /// it reads `None` in both (R3.4).
    started: bool,
}

impl SelectionDrag {
    /// Arm a drag for a press on viewport `cell` of tab `tab`, whose terminal is
    /// `terminal`. The cell becomes a content point **now**, at press time (R3.1).
    pub(crate) fn press(tab: TabId, cell: (u16, u16), terminal: &Terminal) -> Self {
        SelectionDrag {
            tab,
            press_cell: cell,
            anchor: terminal.point_at(cell.0, cell.1),
            moved: false,
            started: false,
        }
    }

    /// The D3 decision for one move, over plain values: the active tab, the pointer's
    /// viewport cell, the content point under it, and whether the drag tab's grid holds
    /// a selection right now.
    ///
    /// The order is the specification, and each step guards the next:
    ///
    /// 0. Another tab in front → [`DragMove::Ignore`], before anything is read or
    ///    recorded, so neither tab is written (R3.5).
    /// 1. Started, and the selection is gone → [`DragMove::Spent`] (R3.3). Gated on
    ///    `started`, because a fresh drag also has no selection yet (R3.4).
    /// 2. Off the press cell → `drag_moved`, keyed on the **viewport** cell (R3.2).
    /// 3. Moved → [`DragMove::Set`] from the press-time anchor, and the drag is started.
    fn step(
        &mut self,
        active: TabId,
        cell: (u16, u16),
        focus: Point,
        has_selection: bool,
    ) -> DragMove {
        if active != self.tab {
            return DragMove::Ignore;
        }
        if self.started && !has_selection {
            return DragMove::Spent;
        }
        if cell != self.press_cell {
            self.moved = true;
        }
        if !self.moved {
            return DragMove::Ignore;
        }
        self.started = true;
        DragMove::Set {
            anchor: self.anchor,
            focus,
        }
    }

    /// Apply one `CursorMoved` at viewport `cell` to the **active** tab `active`,
    /// whose terminal is `terminal`. Returns whether the selection was written — the
    /// caller's cue to redraw.
    ///
    /// The focus is converted here, at move time, against the same terminal the
    /// selection is written to. Passing the active tab's terminal rather than looking
    /// the drag tab up is safe because step 0 refuses every move in which they differ,
    /// and in every other move they are the same terminal.
    pub(crate) fn drive(
        &mut self,
        active: TabId,
        cell: (u16, u16),
        terminal: &mut Terminal,
    ) -> bool {
        let focus = terminal.point_at(cell.0, cell.1);
        match self.step(active, cell, focus, terminal.selection().is_some()) {
            DragMove::Set { anchor, focus } => {
                terminal.set_selection(anchor, focus);
                true
            }
            DragMove::Ignore | DragMove::Spent => false,
        }
    }
}

/// `drag_moved` for the drag a release ends — did the pointer ever leave the press
/// cell? What hyperlinks' `url_to_open` reads to tell a click from a drag (hyperlinks
/// D19), and what [`release`] decides on.
///
/// `None` — a release with no drag recorded — reads as a click. The event loop cannot
/// reach that (`dragging` is armed only together with a drag), and it is what the
/// `drag_moved: false` the app started with meant before this type existed.
pub(crate) fn moved(drag: Option<SelectionDrag>) -> bool {
    drag.is_some_and(|d| d.moved)
}

/// End `drag` on release, against `terminal` — the **active** tab's, exactly as before
/// this type existed (selection-input R3.4, unchanged; D3: "Release behaves as it does
/// today"). A plain click clears the selection; a real drag leaves it standing for
/// Cmd+C. Returns whether it cleared — the caller's cue to redraw.
pub(crate) fn release(drag: Option<SelectionDrag>, terminal: &mut Terminal) -> bool {
    if moved(drag) {
        return false;
    }
    terminal.clear_selection();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::{Color, Selection};

    const A: TabId = TabId(1);
    const B: TabId = TabId(2);

    /// The spec's fixture shape: `alpha` / `bravo` / `charlie` on rows 0–2 of a 10×3
    /// terminal, at the live bottom with no history.
    fn fixture() -> Terminal {
        let mut t = Terminal::new(10, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"alpha\r\nbravo\r\ncharlie");
        t
    }

    // selection-scroll-anchor R3.1
    #[test]
    fn the_press_anchors_on_the_text_under_the_pointer_at_press_time() {
        let mut t = fixture();
        let bravo = t.point_at(0, 1);
        let mut drag = SelectionDrag::press(A, (0, 1), &t);
        // Output lands between the press and the first move: `bravo` rides up to row 0
        // and `charlie` now sits under the press cell.
        t.advance(b"\r\ndelta");
        assert_eq!(t.snapshot().to_text().lines().next(), Some("bravo"));

        assert!(drag.drive(A, (4, 0), &mut t));

        let sel = t
            .selection()
            .expect("the first move off the press cell selects");
        // The anchor is the line that reads `bravo` — on viewport row 0 now — col 0,
        // and the focus is the content point under the move's cell.
        assert_eq!(sel.anchor, bravo);
        assert_eq!(sel.anchor, t.point_at(0, 0));
        assert_eq!(sel.focus, t.point_at(4, 0));
        // Not the line now on the press cell's row: converting the press cell at move
        // time would anchor on `charlie` and select across two lines.
        assert_ne!(sel.anchor, t.point_at(0, 1));
        assert_eq!(t.selection_text().as_deref(), Some("bravo"));
    }

    // selection-scroll-anchor R3.2
    #[test]
    fn a_motionless_pointer_over_moving_content_is_still_a_click() {
        let mut t = fixture();
        // A selection from an earlier drag, which this click must clear on release.
        let earlier = (t.point_at(0, 0), t.point_at(4, 0));
        t.set_selection(earlier.0, earlier.1);

        let mut drag = SelectionDrag::press(A, (2, 2), &t);
        t.advance(b"\r\ndelta"); // output streams: different text under (2,2) now
        assert_ne!(
            t.point_at(2, 2),
            drag.anchor,
            "the content under the cell moved"
        );

        // A jitter move reported in the same cell.
        assert!(!drag.drive(A, (2, 2), &mut t));
        assert!(
            !moved(Some(drag)),
            "drag_moved is keyed on the viewport cell"
        );
        let sel = t.selection().expect("the earlier selection is untouched");
        assert_eq!((sel.anchor, sel.focus), earlier, "no selection was set");

        assert!(release(Some(drag), &mut t), "a click clears on release");
        assert_eq!(t.selection(), None);
    }

    // selection-scroll-anchor R3.3
    #[test]
    fn a_started_selection_that_gets_cleared_spends_the_drag() {
        let mut t = fixture();
        let mut drag = SelectionDrag::press(A, (0, 1), &t);
        assert!(drag.drive(A, (2, 1), &mut t));
        assert!(t.selection().is_some());

        // The D4 change check (T2) clearing mid-drag, simulated directly.
        t.clear_selection();

        assert_eq!(
            drag.step(A, (4, 1), t.point_at(4, 1), false),
            DragMove::Spent
        );
        assert!(!drag.drive(A, (4, 2), &mut t));
        assert_eq!(t.selection(), None, "a spent drag never re-creates");

        assert!(
            !release(Some(drag), &mut t),
            "a real drag does not clear on release"
        );
        assert_eq!(
            t.selection(),
            None,
            "and nothing comes back through release"
        );
    }

    // selection-scroll-anchor R3.4
    #[test]
    fn the_first_move_is_never_mistaken_for_a_spent_drag() {
        let mut t = fixture();
        let mut drag = SelectionDrag::press(A, (0, 1), &t);
        assert!(!drag.started && t.selection().is_none(), "a fresh press");

        assert!(drag.drive(A, (4, 1), &mut t));
        assert!(drag.started);
        assert!(moved(Some(drag)));
        assert_eq!(t.selection_text().as_deref(), Some("bravo"));
    }

    // selection-scroll-anchor R3.4 — the started drag keeps extending while its
    // selection stands, including back over the press cell (`drag_moved` is sticky).
    #[test]
    fn a_started_drag_keeps_extending_its_selection() {
        let mut t = fixture();
        let mut drag = SelectionDrag::press(A, (0, 1), &t);
        assert!(drag.drive(A, (2, 1), &mut t));
        assert_eq!(t.selection_text().as_deref(), Some("bra"));

        assert!(drag.drive(A, (6, 2), &mut t));
        assert_eq!(t.selection_text().as_deref(), Some("bravo\ncharlie"));

        assert!(drag.drive(A, (0, 1), &mut t), "back on the press cell");
        assert_eq!(t.selection_text().as_deref(), Some("b"));
        assert!(!release(Some(drag), &mut t));
        assert_eq!(t.selection_text().as_deref(), Some("b"));
    }

    // selection-scroll-anchor R3.4 — the decision itself, before any write.
    #[test]
    fn a_move_on_the_press_cell_is_ignored_and_one_off_it_sets() {
        let t = fixture();
        let mut drag = SelectionDrag::press(A, (0, 1), &t);
        let anchor = t.point_at(0, 1);

        assert_eq!(drag.step(A, (0, 1), anchor, false), DragMove::Ignore);
        let focus = t.point_at(3, 1);
        assert_eq!(
            drag.step(A, (3, 1), focus, false),
            DragMove::Set { anchor, focus }
        );
    }

    // selection-scroll-anchor R3.5
    #[test]
    fn a_move_after_a_tab_switch_does_nothing() {
        let mut tab_a = fixture();
        let mut tab_b = fixture();
        let mut drag = SelectionDrag::press(A, (0, 1), &tab_a);

        // Tab B is in front, the button still held.
        assert_eq!(
            drag.step(B, (4, 1), tab_b.point_at(4, 1), false),
            DragMove::Ignore
        );
        assert!(!drag.drive(B, (4, 1), &mut tab_b));
        assert_eq!(tab_b.selection(), None, "the anchor never reaches tab B");
        assert_eq!(tab_a.selection(), None, "and tab A is not written either");
        assert!(!moved(Some(drag)), "an ignored move records nothing");

        // Back on tab A, the same drag acts again: the check is per move.
        assert!(drag.drive(A, (4, 1), &mut tab_a));
        assert_eq!(tab_a.selection_text().as_deref(), Some("bravo"));
        assert_eq!(tab_b.selection(), None);
    }

    // selection-scroll-anchor R3.2 — a release with no drag recorded is a click, which
    // is what the app's `drag_moved: false` starting value meant before this type.
    #[test]
    fn a_release_with_no_drag_recorded_is_a_click() {
        let mut t = fixture();
        t.set_selection(t.point_at(0, 1), t.point_at(4, 1));
        assert!(!moved(None));
        assert!(release(None, &mut t));
        assert_eq!(t.selection(), None);
    }

    // selection-scroll-anchor R3.6
    #[test]
    fn a_drag_while_scrolled_back_makes_a_visible_selection() {
        let mut t = Terminal::new(10, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"alpha\r\nbravo\r\ncharlie\r\ndelta");
        t.scroll_up(1);
        assert_eq!(t.snapshot().to_text().lines().next(), Some("alpha"));

        let mut drag = SelectionDrag::press(A, (0, 0), &t);
        assert!(drag.drive(A, (3, 0), &mut t));

        assert_eq!(
            t.snapshot().selection,
            Some(Selection {
                anchor: (0, 0),
                focus: (3, 0)
            })
        );
        assert_eq!(t.selection_text().as_deref(), Some("alph"));
    }

    /// The match arm of `code` that starts at `start`, up to the next `WindowEvent::`
    /// arm — bounded, so a call found later in the file cannot satisfy a guard about
    /// this arm.
    fn arm<'a>(code: &'a str, start: &str) -> &'a str {
        let (_, rest) = code
            .split_once(start)
            .unwrap_or_else(|| panic!("terminal_app.rs has no `{start}`"));
        rest.split("WindowEvent::").next().unwrap_or(rest)
    }

    // selection-scroll-anchor R3.1/R3.3/R3.5 — the window-bound join, pinned at the
    // source because no test can host the event loop that runs it.
    //
    // Every test above drives this module, and all of them stay green if the event loop
    // stops calling it — or calls `point_at` itself again, which is how the T1 stop-gap
    // converted the press cell at move time and anchored on the wrong line. So: the app's
    // event loop converts no cell and writes no selection of its own, each arm hands its
    // gesture to this module, and the release reads `drag_moved` for the link decision
    // BEFORE `release` consumes the drag — after it, `moved` reads `None`, every drag
    // reads as a click, and a ⌘-drag ending on a link would open it (hyperlinks D19).
    #[test]
    fn the_event_loop_arms_only_delegate_to_the_drag() {
        let src = include_str!("terminal_app.rs");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("terminal_app.rs has a test module to split on");
        // Comments stripped: a guard a comment can satisfy is not a guard.
        let code = production
            .lines()
            .map(|l| l.split("//").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");

        for call in [".point_at(", ".set_selection(", ".clear_selection("] {
            assert!(
                !code.contains(call),
                "terminal_app.rs calls `{call}` itself; the drag's conversions and \
                 writes belong to `drag.rs`, where they are tested"
            );
        }

        let moved = arm(&code, "WindowEvent::CursorMoved {");
        assert_eq!(
            moved.matches(".drive(").count(),
            1,
            "the CursorMoved arm must hand each move to `SelectionDrag::drive`: {moved}"
        );

        let press = arm(&code, "PressRoute::Grid =>");
        assert_eq!(
            press.matches("SelectionDrag::press(").count(),
            1,
            "a grid press must arm the drag through `SelectionDrag::press`: {press}"
        );

        let release = arm(&code, "ReleaseTarget::Selection =>");
        let moved_at = release
            .find("drag::moved(")
            .expect("the release reads `drag_moved` from the drag");
        let release_at = release
            .find("drag::release(")
            .expect("the release ends the drag through `drag::release`");
        assert!(
            moved_at < release_at,
            "`drag_moved` must be read before `drag::release` consumes the drag: {release}"
        );
    }
}
