# Spec: agent-elapsed-sidechain

## Meta
- **Created**: 2026-08-16
- **Type**: dev
- **Status**: **implemented and verified** (2026-08-20). L2 approved 2026-08-17, L3 approved
  2026-08-18, L4 approved 2026-08-19. T1–T5 were executed as a Sequential Pipeline and
  verified against a loop contract (`loop.md`) whose gates all pass — Gate 1 9/9, Gate 2 7/7,
  Gate 3 checker-scored across two adversarial passes. Changes are confined to
  `crates/ai-monitor/src/telemetry.rs`, `crates/ai-monitor/tests/telemetry.rs` and
  `crates/app/src/dashboard.rs`. **Not committed.**
- **Beyond the spec as written**: one defect surfaced during verification that no GWT covers —
  a notification with no parseable timestamp wrote `end_ms: None` over a correct end, blanking
  the card. Fixed by decision (guarded write); see the Corrections list, item 10.
- **Verified on real logs, not only fixtures**: folding this session's own transcript through
  the shipped code shows five real background agents at 7m25s–11m48s that the old fold reported
  as `0s`/`2s`; folding an unrelated worktree's transcript exercises the `Rose` failure accent
  on four genuinely failed or killed agents.
- **Approved by**: hyunho058 (L2 gate)
- **Approved at**: 2026-08-17
- **Review history**: three adversarial L2 reviews (the skill's 1 + 2 retries, all spent).
  Round 1 returned 12 issues, round 2 returned 8, round 3 confirmed every measurement and
  returned 3 wording defects plus 5 nits. All were independently re-measured before being
  accepted; one round-1 claim did not reproduce and was recorded rather than applied. The
  round-3 fixes in this file have **not** themselves been re-reviewed — the retry budget
  was exhausted. D5's field name and D10's signature remain `assumed`.
- **Re-verification**: before L3 was derived, every load-bearing measurement was re-run
  against the corpus as it stood on 2026-08-17. Every *structural* claim reproduced; every
  *absolute count* did not, for a reason worth knowing — see Research › "The corpus is a
  rolling 30-day window".

## Goal

The ai-monitor dashboard's `ACTIVE AGENTS` section reports a **background (async)
subagent as `✓ DONE` with ~0s elapsed the instant it is launched**, while the agent
is in fact still running — measured at 24m21s against a displayed 0s in a real log.
Stop the dashboard from claiming completion it has not observed.

## Non-goals

- **Folding subagent token usage into the totals.** `specs/telemetry-model/spec.md:482`
  records the main-thread-only totals as a deliberate v1 decision, not an accident.
  Untouched here.
- **Watching the `subagents/` sibling directory.** `specs/telemetry-watcher/spec.md:313`
  scopes out the second watch target and the two-accumulator merge policy, and this
  spec does not reopen it. It is not needed: the completion signal is already in the
  parent file this spec's watcher reads.
- **Changing a synchronous agent's *initial* completion.** A sync agent still completes
  from its own `tool_result`, at the same instant, with the same elapsed. **But a sync
  agent later resumed as a background task does accumulate** under D6/D11 — its card's
  elapsed grows after the fact. That is intended, and it is where 30 of the 94 completable
  agents come from (D4).
- **Tracking background `Bash` tasks.** They emit the same notification block (12 of 127
  observed) but are not subagents and have no card.
- **The new-session / rotation staleness gap** (`specs/telemetry-watcher/spec.md:314`).
  A separate watcher concern.

## Confirmed Goal

A background subagent's card reflects its real lifecycle, using only signals already
present in the watched parent session file. Two halves:

1. **Do not fold an async launch into a completion.** When the `Agent` `tool_result`
   is a launch acknowledgement rather than an answer, the `Subagent` keeps
   `completed: false` / `end_ms: None`, so `agent_elapsed`
   (`crates/app/src/dashboard.rs:614`) renders it `LIVE` with a growing timer.
2. **Complete it when its task-notification arrives**, joined on the agent's own id, so
   the agent completes at its true end time — 24m21s reads as 24m21s, not 0s.

No new watch target is introduced.

## Research

> **Scope and as-of.** Measured **2026-08-16** over the **114 top-level session files**
> matching `~/.claude/projects/*terminal*/*.jsonl` — deliberately **not** recursive: the
> 276 `<uuid>/subagents/agent-*.jsonl` children are excluded, because the panel reads only
> the parent file. Two measurement hazards bit earlier drafts and are recorded so the next
> person avoids them: those directory names begin with `-`, so `jq`/`grep` treat them as
> options unless prefixed `./`; and jq's `//` operator conflates `false` with absent, so
> boolean fields must be probed with `has(...)`. **The absolute counts below do not
> reproduce over time** — see the next subsection.

### The corpus is a rolling 30-day window

Every count in this section is an as-of reading of a corpus that **shrinks from the back**.
`cleanupPeriodDays` is unset in `~/.claude.json`, so Claude Code's 30-day default applies and
session files age out: on 2026-08-17 the oldest surviving file was dated **Jul 18** and the
newest **Aug 17** — exactly 30 days apart. An earlier draft of this spec assumed counts
"drift upward by ~1 per review". They drift **down**: between 2026-08-16 and 2026-08-17 the
corpus went from 114 files to 115 while `toolUseResult` objects fell 11,187 → 10,554.

This is why the decisions below are stated as *rules and ratios*, never as thresholds on a
count. The 2026-08-17 re-run of every load-bearing measurement:

| claim | as-of 08-16 | as-of 08-17 | verdict |
|---|---|---|---|
| `isAsync` present, value `true` | 73 | 57 | count decayed |
| **`isAsync` present, value `false`** | **0** | **0** | **holds — D1's premise** |
| `status: "async_launched"` | 73 | 57 | co-extensive with the key, both runs |
| `agentId` → distinct `agentId` | 265 → 265 | 245 → 245 | **holds — bijective, D4's premise** |
| route A: `tool-use-id` | 64 agents | 50 agents | count decayed |
| route B: `task-id`, async-only | 64 agents | 50 agents | **holds — set-identical to A** |
| route C: `task-id`, every result | 94 agents | 80 agents | count decayed |
| **C − A** | **+30** | **+30** | **holds — D5's entire justification** |
| async launches never notified | 9 / 73 | 7 / 57 | **holds — 12.3% both runs** |
| notification `<status>` vocabulary | 116 / 8 / 3 | 102 / 8 / 3 | `completed`/`failed`/`killed`, no fourth |

Two readings survive intact and one is corrected. `B = A` reproduced *exactly* on an
independent corpus, so D5's "capture from every result, not just launches" is not an artifact
of one sample. The ghost ratio landing on 12.3% twice is coincidence in the digits, not in the
magnitude. The correction is in the next subsection.

### The defect

- **The fold marks completion unconditionally.** `ToolKind::Agent`'s arm sets
  `a.completed = true; a.end_ms = ts` for *any* `tool_result` bearing the
  `tool_use_id` — it never inspects what the result says
  (`crates/ai-monitor/src/telemetry.rs:659-663`).
- **An async launch is a `tool_result`.** Dispatching a background agent writes a
  result within ~83ms saying the agent was launched and will notify on completion.
  A receipt, not an answer.
- **The render then greys the card.** `a.completed` selects `DONE ◯` over `LIVE ●` and
  `ColorRole::TextFaint` over `Amber` (`dashboard.rs:589-598`, constants `:60-61`), and
  `agent_elapsed` freezes at `end - start` (`:614-620`).
- **There is never a second `tool_result` to correct it.** **265 `Agent` `tool_use` ids,
  265 `tool_result`s — one each, none with two.** For an async agent the notification is
  not merely the better completion source; it is the *only* one.

### Measured evidence

| session | agents | displayed elapsed | result kind |
|---|---|---|---|
| `3fbfa6b3…` | 7 | 7s / 8s / 11s / 11s / 9s / 11s / 10s | real result |
| `3fbfa6b3…` | 1 | **0s** | **async launch** |
| `6d916cec…` | 5 | 258s / 153s / 101s / 69s / 110s | real result |
| `d3192a2a…` | 3 | **0s / 0s / 0s** | **async launch** |

**Ground truth**: `agent-aca4e57f85370b11e.jsonl` spans `05:23:43.792Z → 05:48:04.850Z`
= **24m21s**, against the **0s** shown. Its notification lands at `05:48:04.907Z` against
a launch result at `05:23:43.867Z` — so the notification-minus-launch arithmetic in D6
reproduces 24m21s.

### The discriminator is the *presence* of a typed key, not its value

The line carrying an `Agent` `tool_result` also carries a sibling `toolUseResult` object.
On an async launch it has `isAsync: true` and `status: "async_launched"`.

**`isAsync: false` does not exist.** Probed with `has("isAsync")`: **73 records carry the
key, all `true`; 0 carry `false`; 11,114 `toolUseResult` objects lack it entirely.**

| | `has("isAsync")` | `status` | count |
|---|---|---|---|
| async launch | yes, `true` | `async_launched` | 73 |
| real completion | **absent** | `completed` | 192 |

The test is *"`isAsync` is present and true"*, never *"`isAsync == false` means sync"*.
A fixture containing `"isAsync": false` would encode a shape that never occurs.

- **Current on today's build.** The two L2 reviews of this spec were dispatched as
  background agents and logged `isAsync=true, status=async_launched` on Claude Code
  **2.1.233**.
- **Plumbing caveat.** `toolUseResult` sits at the **top level** of the line, a sibling of
  `message`, while `on_tool_result` is handed `it` — the block from `message.content[]`
  (`telemetry.rs:354-355`), the form real logs use. Reaching it needs D10's signature
  change. The prose witness needs none: `result_text(it)` (`telemetry.rs:899-909`) already
  reads the block and is already called here for `Task` (`telemetry.rs:687`).

### The notification block, and which id actually joins it

When a background task stops, the parent file receives a block carrying `task-id`,
`tool-use-id`, `output-file`, `status`, `summary`, `note`, `result`, and a `usage` group
(`subagent_tokens`, `tool_uses`, `duration_ms`). It **already reaches the fold**: the line
is `type: "user"` with `message.content` a **bare string**, landing at `telemetry.rs:367`.
The fold sees it and discards it.

**The obvious join key is the wrong one.** Over 127 genuine notifications:

- **127 notifications carry 127 distinct `tool-use-id`s — every one unique, zero repeats.**
- They resolve to **64 `Agent`, 50 `SendMessage`, 12 `Bash`, 1 outside the window.**
- But only **107 distinct `task-id`s**; **16 task-ids notify more than once**, each repeat
  under a *different* `tool-use-id`.

`tool-use-id` names **the call that woke the agent**, not the agent. `task-id` is the
agent's own id: it equals the launch's `toolUseResult.agentId`, and that mapping is
**bijective — 265 `Agent` results carry 265 distinct `agentId`s, no collisions.**

**Coverage, measured as distinct agents actually completed:**

| key | notifications matched | **distinct agents completed** |
|---|---|---|
| `tool-use-id` — no contract change | 64 | **64** |
| `task-id`, agentId captured from **async launches only** | 73 | **64** |
| `task-id`, agentId captured from **every** `Agent` result | 114 | **94** |

The middle row is the trap: it pays a versioned-contract change and completes a
**set-identical** 64 agents (`only-A = 0`, `only-B = 0`) — its 9 extra notifications are
resumes of agents already matched. The gain appears only in the third row, and **all 30
additional agents are synchronously-launched agents later resumed via `SendMessage`**.
This is why D5 captures the id from every `Agent` result, and why the Non-goal about sync
agents had to be narrowed to their *initial* completion.

- **Statuses are not binary**: `completed` 116, **`failed` 8**, `killed` 3.
- **`usage.duration_ms` is not a usable substitute**: present on 108 notifications, **all
  of them `completed`** — absent from all 8 `failed`, all 3 `killed`, and 8 `completed`.
- **12.3% of launches never notify**: of 73 async launches, 64 are notified and **9 are
  not**.

### Guarding the parse against echoes

The notification must be parsed in a **dedicated branch on the bare-string arm**, not by
extending `scan_command_name` — `telemetry.rs:358` feeds *arbitrary assistant and user
`text` blocks* into that same function. Carriers of the block:

| carrier | lines | reaches the fold? |
|---|---|---|
| `queue-operation` | 309 | no — payload is top-level `content`, matching neither dispatch arm |
| `attachment` | 160 | no — no `message`, type unmatched |
| `user` | 150 | **yes** — 127 as bare string (genuine), 23 as array text blocks |
| `assistant` | 23 | **yes** — array text blocks |

The array-form lines are prose that merely *mentions* the block. Requiring the content to
**start with** the literal `<task-notification>` separates them.

**A live counterexample now exists, and it is exactly the attack D12 defends against.** The
08-16 reading found 0 bare-string records that embed the tag without starting with it. On
08-17 there is **1** — of 114 bare-string user records carrying the tag, **113 start with it
and 1 embeds it**: a `/compact` continuation summary, which quoted a notification block into
its own recap of the conversation. Under a `contains` test that summary would complete a real
agent at the wrong time. Under `starts_with` it is ignored. The rule was chosen from reasoning
about carriers; it now has a witness.

### The whole file is folded, not a bounded tail

`telemetry.rs:616-617` says results whose `tool_use` sits above the watcher window
"always occur". **Not true of the telemetry path**: `backfill_telemetry`
(`lib.rs:781-791`) does `std::fs::read(log)` and folds **every** line before tailing from
EOF, and it is the seed the panel uses (`overlay.rs:718`). The 8 KiB `activity::TAIL_CAP`
bounds the *sidebar activity* signal, not this accumulator.

This refutes the most damaging failure scenario: a 24-minute agent whose `tool_use`
scrolls out of the window before its notification arrives. It cannot happen.

### `Subagent` is a versioned cross-crate contract

`TelemetrySnapshot` is "the versioned contract the watcher sends and the app formats"
(`telemetry.rs:161`), and `Subagent` is reachable through it. **`Subagent` derives
`Clone, Debug, PartialEq, Eq` — no `Default`** (`telemetry.rs:140`), so `..Default::default()`
is unavailable and every added field must be given an explicit initial value at each site.
The literal sites are exactly four:

`crates/ai-monitor/src/telemetry.rs:580` · `crates/app/src/dashboard.rs:1163` ·
`crates/app/src/dashboard.rs:1256` (in `agent_done`, `:1255`) ·
`crates/app/src/dashboard.rs:2123`

`crates/ai-monitor/tests/telemetry.rs` constructs **no** `Subagent` — it only folds JSONL
and reads fields — so it does not break.

### Test surface

- `crates/ai-monitor/tests/telemetry.rs` — headless fold tests over inline JSONL literals
  via `fold()`; no clock is read, so results are machine-stable (`telemetry-model` R10.1).
  `an_in_flight_agent_is_visible_before_it_completes` (`:522-530`) is the template.
- `crates/app/src/dashboard.rs:1771`
  `a_live_agent_elapsed_grows_and_a_completed_one_is_frozen` covers the render side.
- `crates/ai-monitor/tests/fixtures/session.jsonl` is folded by
  `fixture_transcript_folds_to_expected_telemetry`; adding lines perturbs many assertions,
  so inline literals are the cheaper home for new cases.

### Corrections to earlier drafts of this spec

Kept because the same mistake recurred after being noticed, and because one of them
survived a full review cycle:

1. An early pass reported `isSidechain` absent from parent lines, contradicting
   `specs/telemetry-model/spec.md:73`. jq `//` artifact. Re-probed with `has(...)`:
   present and **uniformly `false`**. **That sibling spec is correct**; no correction owed.
2. A later pass made the identical error one section down, tabulating `isAsync: false`
   for sync completions. No such value exists.
3. The first join-key draft keyed on `tool-use-id`; measurement showed it names the waking
   call, not the agent.
4. The second draft keyed on `task-id` but captured the id only from async launches —
   which measurement showed completes a **set-identical** 64 agents, i.e. a contract change
   for nothing. Only capturing from every `Agent` result yields the 94.
5. D13's first draft coloured the elapsed value Rose. `Card` has **no** colour channel for
   its value (`value_color` belongs to `Row`), so that would have forced a `renderer`
   contract change — the colour moved to the accent bar.
6. Minor drift corrected: `backfill_telemetry` is `lib.rs:781-791`; the carrier list
   omitted `attachment`; the `Subagent` literal sites were misidentified.
7. The as-of preamble claimed counts "drift upward by ~1 per review". Wrong direction:
   the corpus is a rolling 30-day window and sheds old sessions faster than review agents
   add new ones. Every absolute count in this section decays.
8. "0 of the bare-string population merely embed the tag" was true on 08-16 and is false on
   08-17 — a `/compact` summary now quotes one. D12 is unchanged; it simply acquired the
   witness it was reasoned into existence without.
9. **Post-implementation (2026-08-20).** R9.1 and the matching Constraint pinned the number
   **four** at three places. The implementation updated all four, and then this feature's own
   tests added a fifth `Subagent` literal (`dashboard.rs`'s `launched_agent` helper), so a
   count-based reading of R9.1 read as failed while the requirement it stands for was fully
   met. Corrected to state the invariant — every literal names both fields, enforced by the
   absent `Default` derive — rather than a census that any new test invalidates. The same
   mistake was made independently in the `loop.md` verification contract and is recorded there
   as lesson L4: run a metric before making it a threshold.
10. **Post-implementation (2026-08-20).** No sub-requirement covers a notification whose
   `timestamp` is missing or unparseable. `on_task_notification` wrote `end_ms = ts`
   unconditionally, so such a line set `completed: true` with `end_ms: None` — which
   `agent_elapsed` renders as an **empty** card value, and which erased an already-correct end
   on a resumed agent. A blank is worse than the `0s` this spec exists to remove, so the write
   is now guarded on `ts.is_some()` while `completed`/`ended_ok` stay unconditional (an entry
   must never be left permanently un-completable). Live exposure was zero — every observed
   notification carries a parseable timestamp — so this is a latent fix, not a bug that had
   bitten. Found by the Gate-3 checker, not by the 44 GWTs.

11. **Post-implementation (2026-08-20).** D9 attributed the never-notified ghosts to
   sessions that end or are cleared before the notification is written. Measured: of 8
   ghosts, **8 had a notification written and 0 did not** — every one into `attachment` and
   `queue-operation`, carriers Research already knew the fold cannot reach. The carrier table
   and the ghost rate were both correct; the *causal link between them* was never drawn, so
   the decision recorded a cause it had not measured. Found by running the feature against a
   live agent rather than by re-reading the spec.

## Decisions

### D1: An async launch is detected by `toolUseResult.isAsync` **being present and true**, with a scoped prose marker as a second witness
- **Status**: resolved
- **The two witnesses, stated exactly**:
  1. `toolUseResult` has the key `isAsync` and its value is `true`.
  2. `result_text(it)` begins with the literal **`Async agent launched successfully.`**
- **Both are tested only inside the `ToolKind::Agent` arm** (`telemetry.rs:659`). The
  scoping is load-bearing, not incidental: **77 `tool_result`s in the corpus carry that
  literal and 3 of them are `Bash` results** — shell commands that grepped for the string
  while this spec was being researched, echoed back into the log. A prose false positive
  on a non-Agent result is harmless because the arm never sees it; a prose false positive
  *inside* the arm would strand a real agent as a permanent LIVE ghost, which is the
  failure D2 exists to refuse.
- **Rationale**: Key-presence, not value comparison — `isAsync: false` never occurs
  (73 present/true, 0 false, 11,114 absent), so a `== false` test would classify every
  sync completion by accident of absence. Either witness matching classifies the result as
  a launch. Rejected: `isAsync` alone (clearest in intent, but `lib.rs:85` sets the
  doctrine that another program's undocumented on-disk detail "must never be load-bearing
  beyond 'use it when it is there'", and this fix is entirely load-bearing on it — one
  field is one point of silent regression). Rejected: prose alone (drifts with wording).
  Rejected: **`toolUseResult.status == "async_launched"` as the second witness** — it
  discriminates perfectly (192 `completed` vs 73 `async_launched`) and is the obvious
  pick, but it lives in the *same object* as `isAsync` and would drift with it in the same
  upstream edit. The point of a second witness is an independent failure domain, so the
  prose is chosen despite being the weaker signal. Rejected: `isSidechain` (uniformly
  `false` in the parent file).
- **Reviewed 2026-08-23 (run 2, finding 3) — `starts_with` is kept, deliberately.** The review
  raised the matching rule as a false-positive risk: a genuine synchronous answer that merely
  *opened* with that sentence would be read as a receipt, and since no notification describes a
  sync completion, the agent would be stranded as a permanent `● LIVE` ghost — exactly the
  failure D2 refuses. The risk is real and the scoping paragraph above is what bounds it. Two
  measurements decided it against tightening: **zero** false positives were found (every result
  in the corpus opening with the literal was a genuine receipt), and **8** of them were caught
  by the prose witness *alone*, their `toolUseResult` carrying no `isAsync` — so the second
  witness is not decoration, and narrowing the literal risks losing detections that only it
  makes. Rejected: extending the literal to the ~179-character prefix all observed receipts
  share (it would make an accidental match essentially impossible and keep all 8, but it buys
  that by binding the witness harder to upstream *wording* — the precise fragility this witness
  was chosen in spite of, and the opposite direction from `isAsync`'s independence). Rejected:
  dropping the prose witness for `isAsync` alone (loses the 8, and collapses R10.3's
  independence requirement). Both counts are as-of readings of a rolling ~30-day window.

### D2: When neither witness fires, keep today's behaviour
- **Status**: resolved
- **Rationale**: An unrecognised result completes the agent exactly as it does now.
  Conservative failure: a format change degrades the panel to the *current* defect rather
  than to a worse one where nothing ever completes. Rejected: defaulting unknown results
  to "not complete" (a format change would strand every agent as a permanent ghost —
  louder, but it destroys the section instead of regressing it). See D11 for the case
  where this path and a later notification both fire.

### D3: An async launch does not complete the `Subagent`
- **Status**: resolved
- **Rationale**: The acknowledgement is a receipt. Leaving `completed: false` /
  `end_ms: None` standing needs no new state — `tool_use` already opens the entry that way
  (`telemetry.rs:577-590`, `telemetry-model` R6.1) — and it is what makes `agent_elapsed` render a growing
  timer.

### D4: The notification joins on `task-id`, matched against the agent id captured from the `Agent` `tool_result` (D5)
- **Status**: resolved
- **Rationale**: `tool-use-id` names the *waking call*: 127 notifications carry 127 unique
  tool-use-ids, of which only 64 are `Agent` calls (50 `SendMessage`, 12 `Bash`). Keying
  on it makes re-notification unrepresentable. `task-id` equals `toolUseResult.agentId`,
  and the mapping is **bijective (265 results → 265 distinct ids)**, so it is safe as a
  key. Measured coverage: **114 of 127 notifications and 94 of 107 task-ids, versus 64** —
  and, critically, those are **94 distinct agents versus 64**, not merely more
  notifications. Rejected: `tool-use-id` (measured to make D6/D7 unreachable). Rejected:
  capturing the id from async launches only — measured to complete a **set-identical 64**
  agents, spending a versioned-contract change for zero additional agents; this is the
  counterargument an earlier draft failed to answer, and D5 exists to answer it. Rejected:
  matching on the description string (not unique, free text).

### D5: `Subagent` gains the agent id, captured from **every** `Agent` `tool_result`
- **Status**: resolved
- **Rationale**: D4's key must be on the ring entry before any notification arrives, and
  only `toolUseResult.agentId` supplies it. It is captured on **sync results too, not just
  async launches** — that is precisely where the coverage gain lives: all 30 agents beyond
  the 64 are sync-launched agents later resumed in the background. The field is
  `agent_id: Option<String>`, `None` only when the result carried no `agentId`. Note the
  capture point is the **result**, not the use: for a sync agent that is at completion
  (258s after launch in one measured case), which is still before any notification can
  arrive. This is the contract change flagged in Research — deliberate, and the minimum
  that makes D4 pay for itself.

### D6: `end_ms` is last-wins; `start_ms` is never rewritten
- **Status**: resolved
- **Rationale**: A resumed agent should read as cumulative time-alive (12m, then 16m after
  a resume), not as two unrelated runs. Real under D4/D5: 16 task-ids notify more than
  once. Reproduces the independently-known 24m21s. Rejected: first-wins (hides resumed
  work). Rejected: `usage.duration_ms` from the notification — present on 108 notifications
  but **absent from all 11 non-`completed` ones and 8 `completed` ones**, so it cannot be
  the sole source, and summing it across resumes would report *working* time rather than
  the time-alive the card promises. Rejected: watching `subagents/agent-*.jsonl` (a second
  watch target and a merge policy — `telemetry-watcher/spec.md:313` — for a number this
  signal already yields).

### D7: The resume window is not tracked
- **Status**: resolved
- **Rationale**: Between one notification and the next, a resumed agent is genuinely
  running while its card reads DONE. Detecting it means parsing `SendMessage` and matching
  `to:` against an agent id — a new tool in scope for a window the next notification
  corrects anyway. **Bounded under D4/D5**: 16 task-ids demonstrably notify again.
  Accepted as a known gap.

### D8: A notification that resolves to no known agent is ignored, uncounted
- **Status**: resolved
- **Rationale**: The genuinely unmatchable residue is small and specific: **12 `Bash`
  notifications** plus any whose task-id predates the ring (13 task-ids in total resolve to
  no known `agentId`). Under D5 the 50 `SendMessage` notifications largely *do* match, so
  they are not part of this set. A counter would blend "evicted from the ring" with "never
  was an agent" into one unactionable number, and a new `TelemetrySnapshot` field to carry
  it is a contract change bought for nothing. `SUBAGENTS_CAP = 32` keeps its "latest 32"
  contract: an aged-out agent stays out. Rejected: re-inserting the entry (an old agent
  would displace a newer one and reorder the list under the user).

### D9: An **async** agent this fold never sees notified stays LIVE, with no clamp
- **Status**: resolved
- **Rationale**: When no notification reaches the fold, nothing in what the fold reads says
  the agent finished, and the panel does not invent an ending. Roughly one async launch in
  eight lands here. `dur()` handles unbounded growth safely — hours format as `31h00m`,
  `u64` cannot overflow (`dashboard.rs:1013-1022`). Rejected: freezing at `last_event_ms`
  (a plausible number the log never asserted — the same fabrication this spec exists to
  remove).
- **Corrected 2026-08-20 — the cause is not what this decision first claimed.** The original
  rationale read: *"If a session ends or is cleared before the notification is written, the
  log never says the agent finished."* Measured across the corpus that day: of 60 async
  launches, 52 were notified in a form this fold reads and **8 were not — and all 8 had a
  notification written anyway.** Every one of them landed in `attachment` and
  `queue-operation` records, the two carriers Research already lists as *"reaches the fold?
  no"*. **Zero were never written.** A live instance was captured while testing this very
  feature: a background agent whose notification appeared six times in the log, never once as
  a bare-string `user` record, leaving its card reading `● 6h01m`.
- **What this changes.** Nothing about the shipped behaviour — the gap is accepted either
  way, and D12's bare-string gate stays exactly as specified. What it changes is
  *tractability*: under the original rationale the data does not exist and there is nothing
  to recover; in fact the data exists and is merely unread, so the ghost rate is recoverable
  by widening the carrier set rather than inherent. That is deliberately **not** attempted
  here — those two carriers hold several times more occurrences than the bare-string
  population, mostly duplicates of notifications already handled, so widening needs its own
  measurement of double-completion and timestamp disagreement. Recorded as a follow-up, not
  a defect of this change.

### D10: `on_tool_result` takes the top-level line as a distinct parameter
- **Status**: assumed
- **Rationale**: Signature becomes `on_tool_result(&mut self, it: &Value, line: &Value,
  ts: Option<u64>)`, where `it` is the content block and `line` the whole record; the
  top-level arm (`telemetry.rs:346-347`) passes `&v, &v`. Chosen over threading a
  pre-extracted `is_async: bool` because `toolUseResult` also carries `agentId`, which D5
  needs from the same object — a bool would have to grow into a tuple at the first change.
  Rejected: prose-only detection to dodge the change (trades the robust witness for the
  fragile one to avoid a local refactor).

### D11: A notification always overwrites, even on an agent already marked complete
- **Status**: resolved
- **Rationale**: Resolves the D2 × D4 tension. If both witnesses miss a launch, D2
  completes the agent at ~83ms and the notification then arrives for the same agent — the
  card jumps `0s ◯` → `24m21s ◯`. That jump is *correct*: the later signal is the better
  one, and last-wins (D6) already prescribes it. Named explicitly so it is tested rather
  than discovered. Rejected: ignoring a notification for an already-complete agent (would
  freeze the drift-path defect in place permanently).
- **Sub-decision — the sync-resume case.** The same rule fires for a **synchronously**
  completed agent later resumed in the background: its `end_ms` is overwritten and its
  elapsed grows after the fact. This is the mechanism behind D4's 30-agent gain, is
  explicitly in scope per the narrowed Non-goal, and gets its own test.

### D12: The notification is parsed only from a bare-string `message.content` that **starts with** the opening tag, and short-circuits
- **Status**: resolved
- **Rationale**: `telemetry.rs:358` feeds arbitrary assistant/user `text` blocks into
  `scan_command_name`, so implementing the parse there would let prose quoting a
  notification complete a real agent — 46 array-form lines mention the block incidentally.
  A dedicated branch on the bare-string arm gated on `starts_with` separates them: measured
  08-17, **113 of 114 tag-bearing bare-string records start with the literal
  `<task-notification>`, and the 1 that merely embeds it is a `/compact` continuation summary
  quoting a notification** — the exact false positive a `contains` test would admit. A matched notification **returns early**; `scan_command_name` is not also run
  on it (0 of 127 bodies embed a command wrapper, so nothing is lost). **Within the block,
  take the first occurrence of each tag** — the measured field order is `task-id`,
  `tool-use-id`, `output-file`, `status`, `summary`, `note`, `result`, `usage`, so every
  field this spec reads precedes the free-text `<result>`, which is where a quoted inner
  block could otherwise appear. Rejected: extending `scan_command_name` (the obvious
  implementation, and the unsafe one); a `contains` test (admits every quoting line).

### D13: The end is reduced to a boolean initialised `true`; a failed end keeps `◯` and turns the **accent bar** Rose
- **Status**: resolved
- **Rationale**: `Subagent` gains one `bool` — `ended_ok` — **initialised `true` at
  `on_tool_use`** and written wherever an ending is *observed*: set from the notification's
  status when one arrives, and from the result's own `is_error` when an `Agent` result
  completes the entry on the non-launch path. `true` is the only safe initial value: the
  ghosts (D9) and every in-flight agent keep whatever it starts as, and initialising `false`
  would paint all of them as failures.
  *(Corrected 2026-08-23 — see R7.1's amendment. This read "the field is written *only* in the
  notification branch, so … every sync completion and every D2 fallback completion keep
  whatever it starts as". The initial-value argument was right and is kept; the "only the
  notification branch" clause was not. A sync `Agent` result carries `is_error`, which
  `tally_call` was already folding into `failed_tool_calls` one branch earlier, so withholding
  it from the card left the panel contradicting its own counter.)* `Subagent` has no `Default` derive, so this value is stated at every literal
  site, whatever their number (see R9.1). A `bool` keeps the contract change minimal and
  classifies future status vocabulary automatically: `failed` and `killed` need no separate
  handling, and a fourth value invented upstream lands on the safe side.
  *(Counts removed 2026-08-20: they were as-of readings of a rolling ~30-day log window and
  had already decayed — `killed` was 3 and is now 4. The vocabulary is what this rests on,
  not its frequency.)*
- **Rendering**: `ColorRole::Rose` ("a failure", `monitor_model.rs:39-40`) replaces
  `TextFaint` on the **accent bar**, and **only when `completed && !ended_ok`** — the gate
  matters because `agent_elapsed` returns `""` for a live agent, so an ungated colour rule
  would be meaningless for in-flight agents and wrong for sync completions. `agents()`
  already picks that accent from the agent's state (`dashboard.rs:590-594`), and TASKS
  already derives a Card accent from a status string via `task_role` (`:514`), so this is
  the established shape; the bar is rendered at `monitor_layout.rs:893-902`. The glyph stays
  `◯`, keeping the section's `●`/`◯` liveness vocabulary intact. Elapsed still freezes at
  the notification regardless of status — a failed run's duration is as real as a
  successful one.
- **`ended_ok` is last-wins, like `end_ms`.** A failed agent that is resumed and then
  notifies `completed` goes Rose → `TextFaint`. This follows from "set to
  `status == "completed"` when a notification arrives" plus D6, but it is stated here so it
  is tested rather than inferred.
- **Rejected — colouring the elapsed value.** `Card` has no colour channel for its value:
  `value_color` belongs to `Row` (`dashboard.rs:678-681`, `:705-708`), while `Card` carries
  only `accent`/`title`/`value`/`detail` (`monitor_model.rs:183-192`). That route would add
  a field to `Card` — a `renderer`-crate contract change touching every Card literal —
  for a colour the accent already expresses.
- Also rejected: `Option<String>` passthrough (matches `TaskEntry.status`'s
  non-enumerating precedent and preserves `failed` vs `killed`, but buys a distinction
  nothing renders); swapping the glyph to `✗` (loudest, but puts two glyph vocabularies on
  one card).

## Constraints

- `term-core` is untouched — an `ai-monitor` + `app` change; the dependency direction
  (`common ← term-core ← renderer ← app`) must not bend. In particular the `renderer`
  crate's `Card` gains no field (D13).
- No new watch target, no second accumulator, no recursion into `subagents/`.
- `tally_call` stays the single point where a call is counted (`telemetry.rs:624-631`, one
  call site). The notification mutates the `Subagent` ring only.
- The `SUBAGENTS_CAP = 32` ring contract ("the latest 32") is preserved.
- `Subagent`/`TelemetrySnapshot` is a versioned cross-crate contract. Two fields are added
  (D5's `Option<String>`, D13's `bool`), and `Subagent` has **no `Default` derive**, so
  **every** literal must state both fields explicitly. Four existed when this was written
  (`telemetry.rs:580`, `dashboard.rs:1163`, `dashboard.rs:1256`, `dashboard.rs:2123`), but the
  count is not the constraint — the absent derive is, and it makes a missed literal a compile
  error however many there are. As implemented there are five: the new tests added one.
- No fixture may contain `"isAsync": false` — that shape does not occur in reality.
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings`
  must pass.
- Tests stay headless, fixture-only and clock-free (`telemetry-model` R10.1) — inline JSONL
  literals over live logs.
- Edits to `*.rs` must go through Write/Edit so the `rustfmt` PostToolUse hook fires;
  shell-applied edits bypass it and surface later as a `cargo fmt --check` failure.

## Known Gaps

- **The resume window renders DONE while the agent runs** (D7). Bounded by the next
  notification, which restores a correct cumulative total.
- **Roughly one async launch in eight becomes a permanent LIVE ghost** (D9). Accepted
  deliberately; today these render a wrong `0s ◯` instead. Sync agents are unaffected: they
  complete from their own `tool_result` and never depend on a notification. **Their
  notifications are written — just not where this fold reads.** Measured 2026-08-20: 8 of 8
  ghosts had a notification in `attachment` and `queue-operation` records, 0 had none. So
  this gap is *recoverable* by widening the carrier set, not inherent to the data — see D9's
  correction for why that widening is a follow-up with its own measurement rather than a
  line-change here.
- **A sync agent's card can jump long after it finished** (D11 sub-decision) — a background
  resume notifying hours later rewrites its elapsed from e.g. `4s ◯` to `2h13m ◯`.
  Intended, and the mechanism behind D4's 30-agent coverage gain, but surprising in the UI
  and listed here because that is where an implementation reviewer will look for it.
- **`Bash` notifications and pre-ring task-ids are silently ignored** (D8) — 12 `Bash`
  plus a small residue, with no counter to observe them.
- **`failed` and `killed` are not distinguishable on the card** (D13). Recovering the
  distinction later means widening the boolean.
- **Upstream format drift is mitigated, not eliminated** (D1/D2). Two witnesses must both
  fail before the defect returns, and the failure is silent.
- **A log truncation loses live background agents, and this feature widened that window**
  (run 2, finding 2). When the watched `.jsonl` shrinks under the tail, `lib.rs` rebuilds the
  accumulator via `SessionTelemetry::restarted()`, which drops everything folded — the
  `subagents` ring included. Before this feature an agent was completed ~83ms after its
  launch, so the exposure was that sliver; now an agent stays live for its whole run, so a
  truncation mid-run drops a card that will never come back — the notification that would
  have ended it arrives, finds no entry with that `agent_id`, and returns silently and
  uncounted (D8). The window went from milliseconds to an agent's full lifetime, and that
  cost is real.
  **Kept anyway, because the obvious repair points the wrong way down D2.** A file that
  shrank is most likely no longer the same session's content, so carrying its live entries
  into the fresh fold imports agents whose notifications may never arrive — manufacturing
  precisely the permanent `● LIVE` ghost D2 exists to refuse, and doing it to *every*
  truncation rather than the rare one. Dropping degrades to a missing card, which is the
  quieter and more honest of the two failures and matches the direction D2 already chose
  everywhere else. Rejected for the same reason: carrying only `completed == false` entries
  (narrower, same ghost risk). Fixing this properly means giving `restarted()` a way to know
  whether the shrunken file is still the same session — a question this feature does not have
  the evidence to answer, and one that belongs to `restarted()`'s own feature rather than
  to a sidechain-elapsed fix.

## Requirements

> Every sub-requirement is a headless fold assertion in `crates/ai-monitor/tests/telemetry.rs`
> unless it names `agents()` / `agent_elapsed()`, in which case it is a render assertion in
> `crates/app/src/dashboard.rs`'s test module. The fold↔render boundary is decomposed
> explicitly: where the fold produces a state, the render that consumes it has its own
> sub-requirement.

### R0: A background subagent's card reports its real lifecycle (Confirmed Goal)

#### R0.1: The launch → notification round trip renders the true duration
- **Given**: an `Agent` `tool_use` folded at `t`, its `tool_result` at `t+83ms` carrying a top-level `toolUseResult` with `isAsync: true` and an `agentId`, and a bare-string `<task-notification>` at `t+24m21s` whose `<task-id>` equals that `agentId` and whose `<status>` is `completed`
- **When**: the three lines are folded in order and `agents()` renders the snapshot
- **Then**: the single card reads title `◯ <subagent_type>`, value `24m21s`, accent `TextFaint` — the value is never `0s`

#### R0.2: Between launch and notification the card is live and climbing
- **Given**: only the first two lines of R0.1 folded, and `env.now_ms = t + 5m`
- **When**: `agents()` renders
- **Then**: the card reads title `● <subagent_type>`, value `5m00s`, accent `Amber`; re-rendering at `t + 15m` yields `15m00s`

### R1: An `Agent` `tool_result` is classified as a launch or a completion (D1, D2, D10)

#### R1.1: `isAsync` present and true marks a launch
- **Given**: an in-flight `Agent` entry and its `tool_result`, whose line-level `toolUseResult` object contains `isAsync: true`
- **When**: the result is folded
- **Then**: the entry stays `completed: false` with `end_ms: None`

#### R1.2: The prose witness alone marks a launch
- **Given**: an in-flight `Agent` entry and its `tool_result` whose `result_text` begins with the literal `Async agent launched successfully.`, on a line carrying **no** `toolUseResult` object at all
- **When**: the result is folded
- **Then**: the entry stays `completed: false` with `end_ms: None`

#### R1.3: The prose witness on a non-`Agent` result changes nothing
- **Given**: an in-flight `Agent` entry, plus a pending `Bash` call whose `tool_result` text begins with `Async agent launched successfully.` (the shape produced by grepping for that string, which the 08-16 corpus carried 3 times — this spec's own research echoed into the log)
- **When**: the `Bash` result is folded
- **Then**: no `Subagent` is created or mutated, and the `Bash` call is tallied as an ordinary completed tool

#### R1.4: A result with neither witness completes the agent, exactly as today
- **Given**: an in-flight `Agent` entry and a `tool_result` whose `toolUseResult` carries an unrecognised shape (no `isAsync` key) and whose text is an ordinary answer
- **When**: the result is folded
- **Then**: the entry is `completed: true` with `end_ms` set to the result's timestamp — the pre-existing behaviour, preserved as the conservative fallback

#### R1.5: Absence of `isAsync` is a completion, not a launch
- **Given**: a `tool_result` whose `toolUseResult` object exists and has no `isAsync` key — the shape every measured synchronous completion has, in both the 08-16 and 08-17 readings
- **When**: the result is folded
- **Then**: the agent completes; and no test fixture anywhere in the repo contains `"isAsync": false`, because that value does not occur upstream (R10.2)

#### R1.6: The classifier reads the line, not the content block
- **Given**: a real-log-shaped record whose `tool_result` sits inside `message.content[]` while `toolUseResult` sits at the **top level** of the same record
- **When**: the record is folded
- **Then**: `isAsync` is found and the agent is not completed — a reader that looked only at the content block would complete it, so this sub-requirement is what forces D10's signature change

### R2: An async launch leaves the ring entry live (D3)

#### R2.1: The launch does not write completion state
- **Given**: the fold of R1.1
- **When**: the snapshot is taken
- **Then**: the entry's `completed` is `false`, `end_ms` is `None`, and `start_ms` is unchanged from the `tool_use` timestamp

#### R2.2: The launch is still counted as a completed call
- **Given**: the fold of R1.1
- **When**: the snapshot is taken
- **Then**: the call counters `tally_call` maintains are incremented exactly as before this change — the launch is a delivered `tool_result` and remains one for accounting; only the `Subagent` ring's reading of it changes

#### R2.3: The render shows a live agent
- **Given**: the snapshot of R2.1 and `env.now_ms = start_ms + 90s`
- **When**: `agents()` renders
- **Then**: the card's title begins with `●`, its accent is `Amber`, and `agent_elapsed` returns `1m30s`

### R3: The agent id is captured from every `Agent` result (D5)

#### R3.1: An async launch supplies the id
- **Given**: the fold of R1.1, whose `toolUseResult.agentId` is `agent-abc`
- **When**: the snapshot is taken
- **Then**: the entry's `agent_id` is `Some("agent-abc")`, while its `id` remains the `tool_use_id` — the two are different keys and neither replaces the other

#### R3.2: A synchronous completion also supplies the id
- **Given**: an `Agent` `tool_use` and a plain answering `tool_result` 258s later whose `toolUseResult.agentId` is `agent-sync`
- **When**: the result is folded
- **Then**: the entry is `completed: true` **and** carries `agent_id: Some("agent-sync")` — this capture is what makes the 30 later-resumed agents of D4 reachable, and it happens at completion, still long before any notification

#### R3.3: A result with no `agentId` leaves the id unset
- **Given**: an `Agent` `tool_result` whose `toolUseResult` has no `agentId` (or no `toolUseResult` at all)
- **When**: the result is folded
- **Then**: `agent_id` is `None`, and a later notification bearing any `<task-id>` never matches that entry (R6.2 governs what happens to it)

### R4: The notification block is recognised without false positives (D12)

#### R4.1: A bare-string content starting with the tag is parsed
- **Given**: a `type: "user"` record whose `message.content` is a **string** beginning with `<task-notification>` and carrying `<task-id>`, `<tool-use-id>`, `<status>`
- **When**: the record is folded
- **Then**: the notification branch runs and the matching agent is completed per R5.1

#### R4.2: A bare-string record that merely embeds the tag is ignored
- **Given**: a `type: "user"` bare-string record that begins `This session is being continued from a previous conversation…` and quotes a full `<task-notification>` block further down — the `/compact` continuation summary measured in the corpus on 2026-08-17
- **When**: the record is folded
- **Then**: no agent is completed and no `end_ms` is written; a `contains`-based gate would have completed the quoted agent at the summary's timestamp

#### R4.3: An array-form `text` block carrying the tag is ignored
- **Given**: a record whose `message.content` is an **array** containing a `text` block whose text is an entire `<task-notification>` block
- **When**: the record is folded
- **Then**: no agent is completed — the notification branch exists only on the bare-string arm, so `telemetry.rs:358`'s arbitrary-prose path can never reach it

#### R4.4: A matched notification short-circuits `scan_command_name`
- **Given**: a genuine notification whose `<result>` body embeds the text `<command-name>/deploy</command-name>`
- **When**: the record is folded
- **Then**: the agent is completed and **no** `SkillEntry` named `deploy` is pushed — the branch returns before `scan_command_name` runs

#### R4.5: The first occurrence of each tag wins
- **Given**: a genuine notification whose `<result>` body quotes a second, inner `<task-id>` naming a different agent
- **When**: the record is folded
- **Then**: the outer agent — the one named by the first `<task-id>` — is the one completed; the existing `between()` helper (`telemetry.rs:930-934`) already has this first-occurrence semantics, so no new scanner is introduced

### R5: A notification completes its agent at the notified time (D4, D6, D11)

#### R5.1: The join is `<task-id>` against `agent_id`
- **Given**: a ring entry with `agent_id: Some("agent-abc")` and `start_ms: t`, and a notification at `t + 24m21s` whose `<task-id>` is `agent-abc`
- **When**: the notification is folded
- **Then**: the entry becomes `completed: true` with `end_ms = t + 24m21s`, and `agent_elapsed` renders `24m21s`

#### R5.2: `<tool-use-id>` is not the join key
- **Given**: the entry of R5.1, and a notification whose `<task-id>` is `agent-abc` but whose `<tool-use-id>` names an unrelated `SendMessage` call — the majority shape, since only 50 of 113 measured notifications carry an `Agent` `tool-use-id`
- **When**: the notification is folded
- **Then**: the agent is still completed; the `<tool-use-id>` is read by nothing

#### R5.3: Re-notification is last-wins on the end, never on the start
- **Given**: an entry completed by a notification at `start + 12m`, then a second notification for the same `<task-id>` at `start + 16m`
- **When**: both are folded in order
- **Then**: `end_ms` is `start + 16m` and `start_ms` is unchanged, so the card reads `16m00s` — cumulative time alive, not the 4m of the second leg

#### R5.4: A notification overwrites an agent completed by the D2 fallback
- **Given**: an agent whose launch was misclassified by R1.4 and completed at `start + 83ms`, followed by its real notification at `start + 24m21s`
- **When**: both are folded
- **Then**: the card moves from `0s ◯` to `24m21s ◯` — the later signal wins, and this jump is correct behaviour rather than a defect to suppress

#### R5.5: A synchronously completed agent resumed in the background accumulates
- **Given**: an `Agent` completed by its own `tool_result` 4s after launch, carrying `agent_id`, and a notification for that `<task-id>` arriving 2h13m after the launch
- **When**: both are folded
- **Then**: the card's value moves from `4s` to `2h13m` while its glyph stays `◯` — the intended mechanism behind D4's 30-agent gain, and the surprise recorded in Known Gaps

#### R5.6: A notification is not a tool call
- **Given**: any fold containing notifications
- **When**: the snapshot is taken
- **Then**: `tally_call` has not been invoked for them and the call/failure counters are identical to a fold of the same transcript with the notification lines removed

### R6: The ring's existing invariants survive (D3, D8, D9)

#### R6.1: An `Agent` call still opens a not-completed entry
- **Given**: an `Agent` `tool_use` with `input.subagent_type`, and no result
- **When**: it is folded
- **Then**: the ring holds one entry with `completed: false`, `end_ms: None`, `agent_id: None`, `ended_ok: true` — the pre-existing `telemetry-model` R6.1 behaviour, now also pinning the two new fields' initial values

#### R6.2: A notification matching no entry is ignored and uncounted
- **Given**: a notification whose `<task-id>` matches no `agent_id` in the ring — a background `Bash` task, or an agent that predates the watcher's ring
- **When**: it is folded
- **Then**: no entry is created, no existing entry is mutated, no counter moves, and `TelemetrySnapshot` gains no field to report it

#### R6.3: An agent aged out of the ring is not resurrected
- **Given**: 33 `Agent` calls folded so the first has been evicted by `SUBAGENTS_CAP = 32`, then a notification for that evicted agent's `<task-id>`
- **When**: it is folded
- **Then**: the ring still holds the latest 32 in order — the evicted agent is not re-inserted and no newer agent is displaced

#### R6.4: A launched-but-never-notified async agent stays live indefinitely
- **Given**: the fold of R1.1 with no notification ever arriving — the 12.3% case
- **When**: `agents()` renders at `start + 31h`
- **Then**: the card is still `●` / `Amber` with a growing value; no clamp to a last-event time is applied

#### R6.5: Unbounded growth formats safely
- **Given**: the snapshot of R6.4 at `start + 31h`
- **When**: `agent_elapsed` is called
- **Then**: it returns `31h00m` with no panic and no overflow (`dur`, `dashboard.rs:1013-1022`)

### R7: A failed end is visible without a second glyph vocabulary (D13)

#### R7.1: `ended_ok` starts true, and is written wherever an ending is observed
- **Given**: any newly opened ring entry (R6.1)
- **When**: the snapshot is taken
- **Then**: `ended_ok` is `true`, and it is written by every branch that *observes* an ending — the notification branch, and the non-launch `Agent` result branch, which carries `is_error`. In-flight agents and never-notified ghosts retain the opening value because nothing has observed their end; a synchronous completion retains it only when its result reports no error

> **Amended 2026-08-23 (run 2, finding 1).** This sub-requirement previously read "it is
> written **only** by the notification branch, so in-flight agents, sync completions, D2
> fallbacks and never-notified ghosts all retain this value". That was wrong, and D13 carried
> the same error. The reasoning it came from is sound — `true` must be the initial value, or
> every un-notified agent reads as a failure — but it over-generalised from "the initial value
> must be safe" to "nothing but a notification may ever write the field", and a synchronous
> `Agent` result **does** carry an ending and **does** say whether it succeeded. The fold was
> already reading that very bit one branch earlier: `ok` is derived at `telemetry.rs:1166` and
> folded into `failed_tool_calls` by `tally_call`, so the panel counted a failed call while the
> card for the agent it failed on rendered in the success colour — two readings of one result,
> disagreeing. The corrected rule keeps the property that mattered (nothing *unobserved* is
> ever painted a failure) and drops the clause that was merely how it happened to be
> implemented. Kept visible rather than rewritten away, so the next reader can see this was a
> claim that got tested and lost.

#### R7.2: A `completed` notification keeps it true
- **Given**: the fold of R5.1, whose `<status>` is `completed`
- **When**: the snapshot is taken
- **Then**: `ended_ok` is `true`

#### R7.3: `failed` and `killed` both set it false
- **Given**: two agents notified with `<status>failed</status>` and `<status>killed</status>` respectively — the only non-`completed` values measured, 8 and 3
- **When**: both are folded
- **Then**: both entries carry `ended_ok: false` and both carry the notified `end_ms`; a hypothetical fourth upstream status would also land on `false`, which is the safe side

#### R7.3a: An **absent** `<status>` tag is not a failure
- **Given**: a notification that parses — `parse_task_notification` requires only `<task-id>` — but carries no `<status>` tag at all, so `TaskNotification::status` is `None`
- **When**: it is folded
- **Then**: `ended_ok` stays `true` and the agent still completes at the notified `end_ms`; only a status that is **present and not `completed`** sets `false`

> **Added 2026-08-23 (run 2, finding 4).** R7.3's "safe side" rule was written about unknown
> *values* and silently assumed a value was always there. It was implemented as
> `n.status.as_deref() == Some("completed")`, under which `None` compares unequal and an agent
> is painted Rose for the sole reason that its status could not be read. That inverts D2: an
> absent field is the absence of a reading, never a reading of "it failed" — the same rule the
> fold already applies to `is_error`, where only an explicit `true` is a failure. The safe side
> of a *missing* signal and the safe side of an *unrecognised* one point in opposite
> directions, which is why this needs its own sub-requirement rather than a clause on R7.3.
> Measured 2026-08-23: of the bare-string task-notifications then on disk, none lacked the tag
> (`completed`/`failed`/`killed` only) — read as an as-of reading of a rolling ~30-day window;
> the case is reachable by construction, which is what this rests on, not on its frequency.

#### R7.4: A failed end colours the accent bar, not the glyph
- **Given**: the snapshot of R7.3
- **When**: `agents()` renders
- **Then**: the card's accent is `ColorRole::Rose`, its title still begins with `◯`, and its value is the frozen real duration — a failed run's elapsed is as real as a successful one's

#### R7.5: Nothing *unobserved* is ever Rose
- **Given**: a live agent, a synchronously completed agent whose result reported **no** error, and a D2-fallback completion in one snapshot
- **When**: `agents()` renders
- **Then**: their accents are `Amber`, `TextFaint`, `TextFaint` — the Rose rule is gated on `completed && !ended_ok`, so it can never fire on an agent whose elapsed is still growing

#### R7.5a: A **failed** synchronous completion is Rose
- **Given**: an `Agent` result carrying `is_error: true` and neither launch witness (D1), so it completes on the non-launch path
- **When**: `agents()` renders
- **Then**: the card's accent is `ColorRole::Rose`, its title still begins with `◯`, and its value is the frozen real duration — the same treatment R7.4 gives a notified failure, because the fold knows this one failed just as certainly

> **Amended 2026-08-23 (run 2, finding 1).** R7.5 previously listed "a synchronously completed
> agent" flatly among the things that are never Rose, which read as a rule about *sync
> completions* when the property it was actually pinning is that `completed` alone must never
> imply Rose. A sync completion that errored is an observed failure and is coloured like one;
> what stays exempt is an agent whose end nothing has observed. The gate itself is unchanged —
> `completed && !ended_ok` — so no in-flight card can go Rose.

#### R7.6: `ended_ok` is last-wins, like `end_ms`
- **Given**: an agent notified `failed`, then resumed and notified `completed`
- **When**: both are folded
- **Then**: the accent returns from `Rose` to `TextFaint` and `end_ms` is the later notification's

### R8: The resume window is deliberately not tracked (D7)

#### R8.1: A resumed agent reads DONE until its next notification
- **Given**: an agent completed by a notification, then resumed via a `SendMessage` the fold does not inspect
- **When**: `agents()` renders during the resumed run
- **Then**: the card reads `◯` with its previous frozen elapsed — asserted here so the behaviour is a recorded choice with a bound (the next notification corrects it, and 16 task-ids were measured to notify again), not an undiscovered bug

### R9: The contract change is explicit, total, and confined (D5, D13, Constraints)

#### R9.1: Every `Subagent` literal states both new fields
- **Given**: `Subagent` has no `Default` derive, so `..Default::default()` is unavailable
- **When**: `cargo build --workspace` runs after the two fields are added
- **Then**: it succeeds, and **every** `Subagent` literal in the workspace states both fields — the four that existed when this was written (`telemetry.rs:580`, `dashboard.rs:1163`, `dashboard.rs:1256`, `dashboard.rs:2123`) plus any the change itself adds. No count is asserted: a test that legitimately constructs a `Subagent` would falsify one, and the missing `Default` derive already enforces the real requirement

#### R9.2: The renderer's `Card` gains no field
- **Given**: the diff for this feature
- **When**: `crates/renderer/` is inspected
- **Then**: no file under it is modified — the failure colour travels on the existing `accent: Option<ColorRole>` channel, exactly as `task_role` already does at `dashboard.rs:514`

#### R9.3: `term-core` is untouched
- **Given**: the diff for this feature
- **When**: the changed-file list is taken
- **Then**: it contains only files under `crates/ai-monitor/` and `crates/app/`, and no new dependency edge is introduced

### R10: Verification (Constraints)

#### R10.1: Tests are headless, fixture-only and clock-free
- **Given**: the new tests
- **When**: `cargo test --workspace` runs on a machine with no `~/.claude` directory
- **Then**: every one passes, reading only inline literals and `crates/ai-monitor/tests/fixtures/`, and no test reads a wall clock — timestamps are literals so the numbers are identical on every machine and every run

#### R10.2: No fixture encodes a shape that does not occur
- **Given**: the repository after this feature
- **When**: `grep -r '"isAsync": *false' crates/` runs
- **Then**: it matches nothing — `isAsync` is only ever present-and-true or absent (R1.5)

#### R10.3: Each witness is pinned by its own test
- **Given**: R1.1 (key witness) and R1.2 (prose witness) as separate tests
- **When**: upstream drops `isAsync`, or reworded `Async agent launched successfully.`
- **Then**: exactly one of the two tests fails, surfacing which witness died while the other still protects the panel — the failure is loud in CI instead of silent on screen (the drift risk recorded in Known Gaps)

#### R10.4: Lint and the existing suite hold
- **Given**: the workspace after this feature
- **When**: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo test --workspace` run
- **Then**: all three pass, including `fixture_transcript_folds_to_expected_telemetry` and `a_live_agent_elapsed_grows_and_a_completed_one_is_frozen`, which must keep their current assertions unedited

## Tasks

> **This DAG is a chain, and that is a finding, not a failure to parallelise.** Every task
> edits `crates/ai-monitor/src/telemetry.rs` — T1 the `Subagent` struct, T2 `apply`'s dispatch
> and `on_tool_result`, T3 `apply`'s bare-string arm, T4 and T5 the notification branch T3
> creates. The skill's file-overlap rule serialises tasks that touch one file even for
> independent reasons, and here it binds on every pair. Splitting T2/T3 into concurrent
> workers would produce two conflicting edits to `apply` for no wall-clock gain. The split
> points below are therefore chosen as **verification checkpoints**, not as parallel lanes:
> each task ends at a state whose tests can fail for exactly one reason.

### T1: Widen the `Subagent` contract and open every entry honestly `[infra]`
- **Fulfills**: R6.1, R7.1, R9.1, R9.2, R9.3
- **Depends on**: (none)
- **Touches**: `crates/ai-monitor/src/telemetry.rs` (the struct at `:140-149`, the `on_tool_use`
  literal at `:580`), `crates/app/src/dashboard.rs` (`:1163`, `:1256`, `:2123`)
- **Shape**: add `agent_id: Option<String>` and `ended_ok: bool`; initialise `None` / `true` at
  `on_tool_use`; state both at every literal site — four at the time of writing, five once this
  task's own tests land. Nothing reads either field yet, so the
  build breaking is the whole signal — `Subagent` has no `Default` derive, so a missed literal
  is a compile error rather than a silent wrong value.
- **Horizontal by exception**: this is shared infrastructure — a type both crates construct,
  with no single behaviour to slice through. It is also what serialises everything after it.

### T2: Classify an `Agent` result as launch or completion, and capture its id `[vertical]`
- **Fulfills**: R1, R2, R3, R6.4, R6.5, R10.1, R10.2, R10.3
- **Depends on**: T1
- **Touches**: `telemetry.rs` (`apply` dispatch `:346-355`, `on_tool_result` `:607`, the
  `ToolKind::Agent` arm `:659-663`), `crates/ai-monitor/tests/telemetry.rs`,
  `crates/app/src/dashboard.rs` test module
- **Shape**: D10's signature change first (`on_tool_result(&mut self, it: &Value, line: &Value,
  ts: Option<u64>)`, both arms passing the record as `line`), then the two witnesses inside the
  `Agent` arm, then `agent_id` capture from `line.toolUseResult.agentId` on **every** `Agent`
  result. `tally_call` is not moved or re-read.
- **Vertical**: the fold change is proved end-to-end — the render half (R2.3) asserts the card
  is `● Amber` and climbing, which needs no `dashboard.rs` production change but is the
  user-visible half of the same behaviour and must be pinned in the same task.
- **Checkpoint**: after T2 an async launch renders as a growing timer. The primary defect is
  gone here; T3–T5 are about ending it correctly.

### T3: Recognise the notification block without mutating anything `[infra]`
- **Fulfills**: R4
- **Depends on**: T2
- **Touches**: `telemetry.rs` (a new pure parse helper beside `between()` at `:930`, plus the
  bare-string arm at `:367`), `crates/ai-monitor/tests/telemetry.rs`
- **Shape**: a pure `fn parse_task_notification(&str) -> Option<...>` gated on
  `starts_with("<task-notification>")`, extracting `<task-id>` and `<status>` with the existing
  first-occurrence `between()`; the `apply` branch returns early on a match so
  `scan_command_name` never runs on it.
- **Why this is its own task**: the guard behaviours are **negative** assertions — the
  `/compact` summary of R4.2 and the array-form block of R4.3 must produce *no* effect. Tested
  after the join exists, those tests could pass for the wrong reason (the join missed). Tested
  here, where mutation does not yet exist, a false positive can only mean the gate leaked.
  R4.1 and R4.5 are asserted on the helper's return value, not on ring state.

### T4: Join the notification to its agent and end it at the notified time `[vertical]`
- **Fulfills**: R0, R4.1 (ring-level), R5, R6.2, R6.3, R8.1
- **Depends on**: T3
- **Touches**: `telemetry.rs` (the branch T3 created), `crates/ai-monitor/tests/telemetry.rs`,
  `crates/app/src/dashboard.rs` test module
- **Shape**: match `<task-id>` against `agent_id`; on a hit set `completed = true` and
  `end_ms = ts` unconditionally — last-wins, overwriting an already-complete entry (D11), never
  touching `start_ms`. On a miss: return, creating nothing and counting nothing.
- **Vertical**: R0.1 is the whole feature in one assertion — three lines folded, `24m21s` on
  the card. This task is where the goal becomes checkable.

### T5: Surface a failed end on the accent bar `[vertical]`
- **Fulfills**: R7.2, R7.3, R7.4, R7.5, R7.6, R10.4
- **Depends on**: T4
- **Touches**: `telemetry.rs` (the notification branch), `crates/app/src/dashboard.rs`
  (`agents()` accent selection `:589-598`), tests in both
- **Shape**: set `ended_ok = (status == "completed")` in the branch — last-wins with `end_ms`.
  In `agents()`, the accent becomes three-way: `Amber` when not completed, `Rose` when
  `completed && !ended_ok`, else `TextFaint`. The title glyph expression is **not** touched.
- **Vertical**: fold and render in one task by necessity — a `bool` nothing colours is dead
  state, and a colour rule with no source is unreachable.

## External Dependencies

### Pre-work
- **The evidence has an expiry.** `~/.claude/projects/**` is a rolling ~30-day window
  (`cleanupPeriodDays` unset → default 30), so every count in Research ages out of
  reproducibility. Anyone who wants to re-derive a measurement before implementing should do
  it against a corpus captured now, or re-run the probes and expect different absolute
  numbers with the same structure. Nothing in the tasks depends on this — the fixtures are
  inline literals — but a reviewer who tries to reproduce Research and fails should read this
  first rather than conclude the spec was wrong.

### Post-work
- **Eyeball it with a real background agent** (`/run-term`). Every requirement here is a
  headless assertion; none of them proves the Rose accent reads as "failed" rather than as a
  rendering bug at the actual panel width, or that a card jumping from `4s` to `2h13m`
  (R5.5) is legible rather than alarming. This is the check the test suite structurally
  cannot make.
- **A review pass is owed.** The L2 round-3 fixes, and now L3 and L4, have not been through
  an adversarial review — the skill's retry budget was spent during L2. D5's field name and
  D10's signature are still marked `assumed` and are first contacted in T1 and T2.
