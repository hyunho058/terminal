# Deep Interview Insights: Session Flow Viewer
> Date: 2026-10-18
> Rounds: 6
> Final Ambiguity Score: 0.18

## Core Problem
See, at a glance, every Claude session running inside this terminal: how many there are, which are busy, which are waiting on the user, and what subagents each one has started. Each node is also a fast way to jump to its tab.

## Key Insights & Decisions
- Tree shape: window → tab → pane (only when the tab is split) → Claude session → subagent.
- Scope: only sessions inside this terminal's tabs and panes, not every Claude session on the machine.
- States: the sidebar's three, Working / Waiting for input / Ready. A node must always agree with the sidebar card for the same tab.
- Header count: waiting gets its own number, e.g. `2 working · 1 waiting / 5`.
- A running subagent the log can't see yet: its parent session shows as Working, with no child node until the subagent appears.
- Finished subagents: shown greyed, collapsed behind an "N finished" line.
- Placement: a new panel or overlay that a key toggles, alongside the sidebar and ai-monitor. It replaces neither.
- Interaction: clicking a node jumps to that tab.

## Defined Requirements
- Every tab and pane running Claude appears as a node, and each split pane gets its own node.
- Subagents appear under their parent session once the log shows them.
- Node states and header counts update live.
- The panel opens and closes with a key.
- Done when: the user can watch the working count change live as sessions start and finish.

## Identified Risks & Failure Modes
- Subagents can't be seen while they run. Claude Code 2.1.28x writes a subagent's call to its log only after the subagent finishes. A running subagent exists only as "parent is Working", and its child node appears late.
- Disagreeing with the sidebar. A third view of the same status could contradict the sidebar card. "Must agree" means both views read their status from the same place.
- Clutter in long sessions. Greyed subagents pile up; the "N finished" collapse addresses this.
- Fragile inputs. "Waiting for input" comes from matching Claude Code's on-screen dialog text (G8 in docs/architecture.md). If that wording changes, the viewer goes wrong along with the sidebar.

## Open Questions & Unknowns
- Which key toggles the panel, which side it docks on, and whether it takes terminal columns (like ai-monitor) or floats over them.
- What clicking a pane node or a subagent node does: focus the pane, or only switch to the tab?
- Whether Antigravity sessions count as sessions (the sidebar already tracks them).
- Whether tabs with no Claude session appear as empty nodes or are hidden.
- With more than one window open, does each window's viewer show every window?
- How "live" is measured: no update time was set (e.g. "the same frame as the sidebar").

## Clarity Assessment
Ambiguity Score: 0.18
- Goal Clarity: 0.88 (40%)
- Constraint Clarity: 0.78 (30%)
- Success Criteria: 0.78 (30%)

Maturity: Solid. The tree shape, scope, states, count format and placement are all decided. What's left is detail that /specify can settle.
