# loop.md — Verification contract: agent-asks-user-waiting

**Goal:** `AskUserQuestion` must trigger the sidebar waiting signal.

`crates/ai-monitor/src/activity.rs` classifies an assistant turn as
`AssistantEndTurn` only when `stop_reason == "end_turn"`, and
`crates/app/src/tabmeta/activity.rs` produces `AgentState::Waiting` only from
that variant. But Claude Code asks questions by calling the `AskUserQuestion`
tool, whose record carries `stop_reason: "tool_use"` (verified in real logs at
`~/.claude/projects/**/*.jsonl`) — so it reads `AssistantBusy` → `Busy`, and
`crates/app/src/waiting.rs` never sees a rising edge.

**Fix:** add a `LastMessage::AssistantAsksUser` variant keyed on a **closed list**
of tools that block on a human by definition (`AskUserQuestion`, `ExitPlanMode`),
checked **after** `end_turn` by scanning the message's `tool_use` block names;
map it to `Waiting`. Nothing downstream changes.

**Hard constraint:** keep the existing conservative bias. This adds a *second
precise signal*; it must NOT loosen the fallback path, which deliberately never
yields `Waiting`. A false "waiting" is the worst failure.

## Gate 1 — Pass/Fail (binary, 100%)
- **build**: `cargo build --workspace`
- **tests**: `cargo test --workspace` — 0 failures
- **lint**: `cargo clippy --workspace --all-targets -- -D warnings`
- **fmt**: `cargo fmt --all -- --check`

## Gate 2 — Quantitative (measured vs threshold)
| metric | threshold |
|--------|-----------|
| test failures | = 0 |
| clippy warnings | = 0 |
| new tests added (ai-monitor classify + app `state_from` + end-to-end fixture + conservative-bias regressions) | ≥ 6 |
| real-log record shape reproduced verbatim in a fixture (`stop_reason: "tool_use"` + `content[].type == "tool_use"` with `name == "AskUserQuestion"`) | = yes |
| files touched outside `crates/ai-monitor/src/activity.rs` + `crates/app/src/tabmeta/activity.rs` (excluding `specs/`) | = 0 |

## Gate 3 — Qualitative (checker-scored via separate subagent)
- **closed-list-discipline**: the blocking-tool list is a genuine closed list
  (named constant, documented rationale), matched exactly, and does not
  accidentally catch non-blocking tools. grounds = file:line; action.
- **conservative-bias-preserved**: the fallback path in `state_from` still never
  yields `Waiting`; `None`/truncated-record handling is unchanged; the new
  signal is precise (an explicit blocking tool call), not heuristic.
  grounds = file:line; action.
- **dependency-direction**: `ai-monitor` supplies the raw classification and
  `app` owns the state policy — the split matches the existing D7/D16 layering;
  `term-core` untouched. grounds; action.
- **downstream-invariance**: `waiting.rs` rising-edge logic, the renderer, and
  every existing test remain unmodified — the claim "nothing downstream changes"
  actually holds. grounds; action.

## Lessons

### L1 — a `common::watch` failure is a load-sensitive flake, not a regression
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: `common::watch::tests::watcher_survives_atomic_rename_save` (a `notify`
  filesystem-watcher timing test) failed once under a full parallel
  `cargo test --workspace`, then passed 6 isolated runs, 8 runs on a stashed
  clean tree, and 3 later full runs. Re-run it in isolation before treating a
  workspace-run failure as a regression — and confirm the diff actually touches
  config/watch code before spending the iteration on it.

### L2 — doc-claims-reproducible
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: **doc-claims-reproducible**: every measured number written into a doc
  comment is either reproducible by a future reader, or explicitly marked as a
  snapshot of a private/mutating corpus. A hard count taken from a one-off scan
  of `~/.claude/projects/**` is neither. grounds = the claim and how it was
  re-measured; action.
- note: this run wrote "449 real records" into a `BLOCKING_TOOLS` doc comment,
  sourced from an earlier agent report; the checker re-measured 432 on the same
  (live, mutating) corpus. The *shape* claim was universal and verifiable; only
  the count was not. Prefer the invariant over the tally.
