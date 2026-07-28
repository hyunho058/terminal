# loop.md — Verification contract: agent-prompt-waiting

**Goal:** A tab whose agent is blocked on a tool-permission dialog appears in the
sidebar waiting band.

## The bug this loop fixed

The detector (`crate::prompt`) was correct and tested, but it was wired to a
trigger that is **structurally silent in exactly the case it targets**.

`push_agent_states` (then `feed_agent_states`) ran only on
`UserEvent::OverlayPending`. The tabmeta worker sends that wake only when
`TabMeta` *changed* — `worker.rs::publish_if_changed` returns early otherwise, on
purpose: an unconditional 2 Hz wake was the G1 idle-CPU debt. But a pending
permission dialog changes **no** metadata by definition: the log is precisely what
this feature is blind to, so `agent.state` stays `Busy`, `branch` and `task` do not
move, and nothing publishes. The event loop is `ControlFlow::Wait` with no
`WaitUntil`, so there is no periodic tick to fall back on. Result: the screen check
never ran while a dialog was up.

**Fix:** also drive it from `UserEvent::PtyOutput`, gated on `advanced` — drawing
the dialog *is* PTY output, so that event always fires when a dialog appears or is
dismissed. The `advanced` gate keeps a no-op chunk from forcing a re-scan.

Because that runs per chunk, the read had to get cheaper: the check now uses the
new `Terminal::viewport_tail_text(rows)` (no cell clone, walks up from the bottom,
stops after `rows` rows of content) instead of `snapshot().to_text()` (full-grid
clone). Reintroducing the G1 debt to fix this would have been trading one bug for
another.

## Gate 1 — Pass/Fail (binary, 100%)
- **build**: `cargo build --workspace`
- **tests**: `cargo test --workspace` — 0 failures
- **lint**: `cargo clippy --workspace --all-targets -- -D warnings`
- **fmt**: `cargo fmt --all -- --check`

## Gate 2 — Quantitative (measured vs threshold)
| metric | threshold | measured |
|--------|-----------|----------|
| test failures | = 0 | 0 |
| clippy warnings | = 0 | 0 |
| new tests this iteration | ≥ 4 | 6 |
| dialog detected through a REAL `Terminal` viewport, multiple sizes | ≥ 4 sizes | 4 (80x24, 100x40, 65x31, 120x50) |
| falling edge proven in a real `Terminal` (answered → not waiting) | = yes | yes |
| per-chunk check clones the grid | = no | no (`viewport_tail_text`) |

## Gate 3 — Qualitative (checker-scored via separate subagent)
- **trigger-completeness**: every state change that can open or close a dialog
  reaches the check, and no path that matters is still silent. This is the exact
  class of bug the loop just fixed — look for others. grounds = file:line; action.
- **idle-cpu**: the per-chunk work is bounded and adds nothing at idle; the G1
  decision (no unconditional periodic wake) still holds. grounds; action.
- **dependency-direction**: `viewport_tail_text` is raw text only — `term-core`
  learns nothing about agents or prompts (D21). grounds; action.
- **false-positive-defense**: the two-marker rule and the `has_agent` gate hold;
  the level self-clears. grounds; action.

## Known limits
- Matches another program's UI text: an upstream wording change silently disables
  it. Patterns are isolated in `crates/app/src/prompt.rs`.
- A tab scrolled back into history shows history, not the live dialog, so a prompt
  can be missed while the user is scrolled up (`viewport_tail_text` honours
  `view_offset` deliberately — it reports what is on screen).
- `has_agent` is process presence, not proof the agent drew the dialog.
- `sidebar.show_agent_status = false` makes `TabMeta::agent` always `None`, which
  disables this signal.

## Lessons
(see `specs/agent-asks-user-waiting/loop.md` for L1/L2)

### L3 — verify the TRIGGER, not just the detector
- cause: escalation
- source: human-approved
- form: gate3-item
- status: active
- check: **trigger-completeness**: for any new signal, name the event that fires in
  the target scenario and prove it is not change-gated, throttled, or otherwise
  silent in exactly that case. grounds = the event + the code path that emits it;
  action.
- note: the prompt detector was unit-tested and correct, and shipped broken,
  because it hung off a wake the tabmeta worker only sends when metadata CHANGED —
  and the whole premise of the feature is that nothing changes. A passing detector
  test says nothing about whether the detector ever runs.
