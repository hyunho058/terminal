# Unattended report: mouse-drag-reporting

- **Branch**: `fix/mouse-drag-not-working` (not pushed, no commits)
- **Stopped at**: L4 Spec Review. The verdict was still NEEDS_FIX after the third and last review, so nothing was executed.
- **Spec**: `specs/mouse-drag-reporting/spec.md` (`Approved by` deliberately absent)

## Why the drag does nothing today (diagnosis, measured)
Since #57 (mouse-button-reporting), a plain press over the grid in an app that tracks the mouse goes to the app and arms no terminal selection. But `CursorMoved` never sends **motion while the button is held**, and the release reports at the press cell. So Claude Code sees a click, never a drag.
- Claude Code 2.1.314 enables `?1000 ?1002 ?1003 ?1006` (measured in a PTY on 2026-10-30). Its own copy-on-select (default on) copies through `pbcopy` on macOS, so the fix is to deliver the drag.
- agy's no-flicker mode does the same, according to its embedded changelog. It was not measured live: the probe stopped at sign-in and folder trust.

## What the spec plans
- Report button-held motion under `?1002`/`?1003`, one report per cell, as SGR `ESC[<32+b;c;rM` or the legacy encoding.
- The latch follows the drag, so the release lands at the drag's end.
- An off-grid drag clamps to the grid's edge.
- A reported drag owns the pointer, with no sidebar or link hover during it.
- Shift-drag still gives a terminal selection.

The plan is T1 (pure functions in `input.rs`), then TT1–TT4 (acceptance tests), then T2 (`CursorMoved` wiring).

## Assumed (Status: assumed in the spec)
D1–D4, D6 and D7 were chosen without a person. The main ones:
- no OSC 52;
- motion only under `?1002`/`?1003`;
- clamping;
- a latched release ignores modifiers, which changes the shipped `grid_release_action`.

## What would be parked
Nothing is fenced as a task. One gap is fenced: `L2 fenced: security — whether to honour OSC 52 clipboard writes`. Without OSC 52, a plain drag in a `mouse on` tmux pane highlights but does not reach the clipboard (Shift-drag then ⌘C does).

## Review verdict
The Spec Review returned NEEDS_FIX, PASS, then NEEDS_FIX. The last review's three issues and the L2-reviewer's matching one are applied in the spec, but nobody has reviewed those changes ("Applied after review 3" in `## Spec Review`).

## Next step for you
1. Read `## Decisions` D3 and D6 and `## Spec Review` › Open in the spec.
2. If they are fine, run `/harness-ops:specify` interactively on the existing spec to approve it, or hand it to `/harness-ops:agent-orchestrate` directly: "Implement the approved plan in specs/mouse-drag-reporting/spec.md".
3. Until then, **Shift-drag then ⌘C** selects and copies in Claude Code and agy today.
