# Spec: ai-monitor-absorbed-notifications

## Meta
- **Created**: 2026-09-28
- **Type**: dev
- **Status**: approved (spec only — not executed)
- **Approved by**: user
- **Approved at**: 2026-09-28
- **Predecessor**: `specs/agent-elapsed-sidechain/spec.md` — its D9 correction deferred exactly
  this ("widening the carrier set") to a follow-up "with its own measurement of
  double-completion and timestamp disagreement". § Research › *The deferred measurement* is
  that measurement.

## Goal
ai-monitor ACTIVE AGENTS never marks background agents done when their task-notification
arrives mid-turn. Claude Code writes those as `type:"attachment"`,
`attachment.type:"queued_command"`, `attachment.commandMode:"task-notification"`, with the
notification text in `attachment.prompt` (a string starting `<task-notification>`), instead
of as a bare-string `message.content`. `SessionTelemetry::apply`
(`crates/ai-monitor/src/telemetry.rs`) only reads `message.content`. So route that
attachment through `parse_task_notification` → `on_task_notification`, keeping the
starts-with check. Use the record's timestamp as the end time. Add a regression fixture
taken from session 903790a2 (all 9 agents stayed `completed=false`). Check coverage with
`cargo mutants -f crates/ai-monitor/src/telemetry.rs`.

**Widened at the L0 mirror (user choice "Include clock fix"):** also stop out-of-order
timestamps moving `last_event_ms` backwards. The absorbed attachment is the most extreme
instance (stamped up to 9.5 h before the line it follows) but not the only one.

## Non-goals
- **`queue-operation` records** as a notification carrier. They are a third copy of the same
  notification; every `absorbed_mid_turn` removal that names an agent has a matching
  attachment (26/26 notification events at reading A, 23/23 at reading C — see § Research ›
  *The record shape* › **Reading the numbers**), so reading the queue too would add a double-completion source and no
  coverage.
- **Notifications delivered into a subagent's own log** (`remove` reason `delivered_to_agent`,
  38 corpus-wide) and **subagent sidechain logs** in general. None of the 38 names an agent in
  its main log; the fold reads the main log only.
- **`first_event_ms`** (UPTIME's origin). It stays first-line-wins. 49 lines corpus-wide are
  stamped before their file's first line: 42 by under a second, 3 more by under a minute, and
  4 by up to 2,071 s — all 4 in one resumed session's replayed history, where "when did this
  session start" has no single right answer. Recorded as Known Gap G2, not changed here.
- **Any change to what the app renders or how** (`crates/app/src/dashboard.rs`). The app
  consumes `completed`/`end_ms`/`ended_ok`/`last_event_ms` exactly as today; only the values
  the fold produces change.
- Distinguishing `failed`/`killed` on the card, counting unmatched notifications, clamping
  never-notified agents — all settled by the predecessor (D8/D9/D13) and untouched.

## Confirmed Goal
Make the Claude fold end a background agent from **every carrier the log uses to say it
ended**, and make the session clock report **the latest moment the log attests something
happened**:

1. A `type:"attachment"` record whose `attachment.type` is `queued_command` and whose
   `attachment.prompt` is a string starting with `<task-notification>` is routed through the
   same `parse_task_notification` → `on_task_notification` path as the bare-string form, with
   the record's top-level `timestamp` as the end time. The starts-with gate is kept; no other
   attachment kind and no other field is read as a notification.
2. `last_event_ms` never moves backwards: a parseable timestamp earlier than the current value
   leaves it unchanged. Applied to both dialects, since it is one field with one meaning.
3. The code comments and the predecessor spec's Known Gaps that describe the bare string as
   "the ONLY signal" and the ~1-in-8 LIVE ghost are corrected to match.

**Done when**
- An excerpt of session 903790a2, checked in as a fixture, folds to **9 of 9** agents
  `completed=true`, each `end_ms` equal to its attachment's `timestamp` (the nine ids and
  times are listed in D7).
- The clock is asserted **mid-sequence**, not only at end of file. Folding the fixture through
  the `bw40b21kz` attachment leaves `last_event_ms` at the preceding `remove`,
  `2026-09-28T02:29:38.020Z` = **1790562578020**, not at the attachment's own
  `2026-09-27T17:02:08.163Z` = 1790528528163. Folding through the last late-written `assistant`
  line (903790a2 line 3997) leaves it at the `enqueue`, `2026-09-27T17:02:08.164Z` =
  **1790528528164**, not at that line's `2026-09-27T16:38:37.906Z` = 1790527117906.
- The fixture's records that merely *quote* the tag stay inert: a `hook_success` (tag in
  `stdout`) and a `prompt_snapshot` (tag in a tool description), both from 903790a2. The
  type gate itself is proven by a synthetic attachment of another kind that *does* carry a
  `prompt` starting with the tag. That also covers `deferred_tools_record`, whose only
  quoting instance is in another session's log (`71ff8342`) and is not imported. Every existing
  starts-with / phantom-skill / first-id / timestamp-less test stays green.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass.
- `cargo mutants` scoped to this change's diff of `telemetry.rs` reports **0 missed**, and the
  whole-file run reports **≤ 43 missed** (the pre-change baseline), with both counts recorded
  (exact commands in D8).

## Research

### The code path today
- `SessionTelemetry::apply` parses one line and dispatches on the fixed dialect
  (`crates/ai-monitor/src/telemetry.rs:584-598`).
- `apply_claude` reads the top-level `timestamp` into `ts` and moves the clock **last-wins**:
  `first_event_ms.get_or_insert(t); last_event_ms = Some(t)` (`telemetry.rs:601-611`). Every
  record type with a parseable timestamp moves it, attachments included.
- The top-level `type` dispatch knows `tool_use`, `tool_result`, `system` only
  (`telemetry.rs:645-661`); `attachment` falls to `_ => {}`.
- The notification branch lives on the **bare-string `message.content`** arm, gated on
  `trim_start().starts_with(TASK_NOTIFICATION_TAG)`, and returns whether or not the parse
  succeeds (predecessor R4.4a) so a malformed notification cannot reach `scan_command_name`
  (`telemetry.rs:680-704`). Its comment says the bare string "is the ONLY signal a background
  agent ever ends by" (`telemetry.rs:682-683`) — false for the corpus below.
- `parse_task_notification` re-checks starts-with itself, requires a non-empty `<task-id>`,
  reads `<status>` optionally, each at first occurrence (`telemetry.rs:2256-2285`).
- `on_task_notification` joins on `agent_id == task-id`, sets `completed = true`
  unconditionally, writes `end_ms` only when `ts.is_some()`, and sets
  `ended_ok = status.is_none_or(|s| s == "completed")`; unmatched ids are ignored and
  uncounted (`telemetry.rs:1435-1491`).
- The Antigravity fold moves the clock the same last-wins way from `created_at`
  (`telemetry.rs:739-746`).
- `restarted()` drops the `subagents` ring on truncation, so a notification after a restart
  finds nothing to join (`telemetry.rs:565-573`) — unchanged by this spec.
- The module contract: `apply` is pure over its input, no clock, never panics
  (`telemetry.rs:14-24`).
- `last_event_in_tail` — the resolver's freshness read over the log tail — already takes the
  **max** timestamp, not the last (`crates/ai-monitor/src/lib.rs:662-683`). The fold and the
  resolver therefore already disagree whenever a line regresses.

### Consumers of the changed values (`crates/app/src/dashboard.rs`)
- `agent_elapsed`: `(Some(s), true, Some(e)) if e >= s => dur(e - s)`; a live agent renders
  `now - start` (`dashboard.rs:770-776`). A notification's `end_ms` is always later than the
  launch it ends, so the `e >= s` guard holds for every measured case.
- `last_event_ms` drives the CONNECTED/IDLE pill (`dashboard.rs:248-256`, the #41 feature),
  `idle_deadline_ms` (`:280`), the IDLE tile (`:422`), the "Last update HH:MM" footer
  (`:521-524`), and `Detached::StaleSince` (`:572-575`). A regressed clock flips the pill to
  IDLE and prints a stale footer time.

### Tests and fixture conventions
- Fixtures live in `crates/ai-monitor/tests/fixtures/*.jsonl`, read with `include_str!`;
  "never a live log, and no clock is read" (`crates/ai-monitor/tests/telemetry.rs:1-21`).
- `session.jsonl` is asserted whole by `fixture_transcript_folds_to_expected_telemetry`, so a
  new feature adds a **separate** fixture rather than editing it — precedent
  `context_command.jsonl`, `specs/ai-monitor-context-window/spec.md` D13
  (`tests/telemetry.rs:3014-3022`).
- `no_fixture_encodes_a_shape_that_never_occurs` scans every fixture for the literal
  `"isAsync"` followed by JSON `false` and fails on it (`tests/telemetry.rs:1171-1203`). Real
  903790a2 receipts carry `isAsync: true`, so a verbatim excerpt passes.
- Helpers: `fold` (`:12`), `notification` (`:770`), `bare_string_line` (`:787`),
  `async_launch` (`:955`). Existing notification guards: starts-with vs a quoting `/compact`
  summary (`:817`), first `<task-id>` wins (`:842`), no phantom skill (`:871`), malformed
  notification still short-circuits (`:897`), array arm never reaches the branch (`:923`),
  timestamp-less notification never erases an end (`:1208`).
- Clock tests: timestamp-less line does not move the clock (`:150`), unparseable timestamp
  ignored (`:161`), unknown line type still moves the clock (`:1564`), Antigravity
  `created_at` (`:2575-2585`). **None folds two timestamps out of order**, so no existing
  test pins last-wins against max.
- Mutation config: `.cargo/mutants.toml` (`copy_vcs = true`, `exclude_re = []`, narrowing
  belongs at the call site). `cargo mutants --list -f crates/ai-monitor/src/telemetry.rs`
  = **391** mutants as of 2026-09-28. `crates/app/src/dashboard.rs:2237,3808,3839` also
  construct `SessionTelemetry`, so an ai-monitor-only run can report a survivor that an app
  test would kill — a survivor from a narrowed run must be re-checked before it is called one.

### The record shape (measured 2026-09-28)
Corpus: **80** top-level session logs under `~/.claude/projects/*/*.jsonl` (a rolling window;
read counts as as-of readings, the shapes as the durable facts).

**Reading the numbers.** The corpus moved *while this spec was written*, so three readings of
one population appear in this document. Each is labelled here by when it was taken, using the
times this session's own log records for the tool results:

| reading | taken (UTC) | logs | agents ending only via attachment | agent notification events via attachment |
|---|---|---|---|---|
| **A** | ≤ 07:26:46 | 80 | **22** of 59 | **26** (`absorbed_mid_turn` removals) |
| **B** | 07:35:26 | 80 | — | **27** (attachments, receipt-order check) |
| **C** | 17:50:28 | 79 | **19** of 52 | **23** attachments ↔ 23 removals |

- **A → B (+1 event):** 903790a2 absorbed its **10th** agent's notification
  (`a96de1f53988b6364`, launched 07:32:50Z, attachment stamped 07:34:54.966Z, `remove` at
  07:34:56.843Z). That is between A's result (07:26:46Z) and B's (07:35:26Z).
- **B → C (−4 events):** log `538f4d49` aged out of `~/.claude/projects`. 903790a2 still held
  10 attachment-ended agents at C.
- **A → C (agents: 22 → 19 attachment-only, 59 → 52 overall).** B counted events only, so the
  agent deltas span A to C. They net 903790a2's +1 against 538f4d49's departure, which
  therefore held 4 attachment-only agents (22 + 1 − 19). This is inferred, not measured,
  because the file no longer exists.
- **Units.** *Agents* are distinct `agentId`s (22, 19). *Notification events* (26, 27, 23) count
  every delivery, and exceed the agent count because a resumed agent is notified more than once.
- Every other count in this section (113 attachments, 146 quoting records, the timestamp
  statistics) is from reading A.

No durable claim in this spec depends on which reading is used. Those claims are: zero
double-carried notifications, zero agents ending through neither carrier, the ≤ 2 ms stamp
agreement, and receipt-before-attachment order.

- **Baseline, reproduced.** Folding the whole 903790a2 log through today's `apply`: **9 of 9**
  agents `completed=false, end_ms=None, ended_ok=true`. All 9 were notified only by a
  `queued_command` attachment; the 5 bare-string notifications in that log all name
  background `Bash` tasks (`b…` ids), which have no card.
- **Blast radius.** Of **59** background agents with an `agentId` corpus-wide, **22** end only
  through the attachment, **37** only through the bare string, **0** through both, **0** through
  neither. Shape first seen at Claude Code **2.1.247** (2026-08-28), still current at 2.1.283.
- **The shape.** 113 `queued_command` attachments corpus-wide: **113/113**
  `commandMode:"task-notification"`, `prompt` a **string**, starting with the tag. No other
  `commandMode` value occurs. The record also carries `rendered` / `renderedInHumanTurn`,
  which embed the same block inside a `<system-reminder>` — i.e. *not* at the start.
- **Records that quote the tag.** 146 other attachments contain `<task-notification>`:
  `hook_success` 79, `prompt_snapshot` 66, `deferred_tools_record` 1. **None** starts
  `attachment.prompt` or `attachment.content` with it. They are the attachment-side
  counterpart of the `/compact` summary the starts-with gate already defends against.

### The deferred measurement (predecessor D9's two conditions)
- **Double completion: none.** For all 113 attachments, the same `<task-id>` never appears as
  a bare-string notification in the same log. Every notification is `enqueue`d once and then
  either `remove`d with reason `absorbed_mid_turn` (→ attachment) or `dequeue`d (→ bare-string
  user record). An agent resumed and re-notified yields two notifications. The predecessor's
  D6 already handles that: `on_task_notification` overwrites `completed`/`end_ms`/`ended_ok` on
  every matching call (`telemetry.rs:1441-1491`). That is the *agent* end's last-wins, and it
  is unrelated to this spec's D5, which changes the *session clock* away from last-wins.
- **Timestamp disagreement: none that matters.** Top-level `timestamp` == `attachment.timestamp`
  in **113/113**; and it is within **≤ 2 ms** of the matching `enqueue` in 113/113 (96 at 0 ms,
  16 at 1 ms, 1 at 2 ms). So the record's timestamp is the moment the notification *arrived*,
  i.e. the agent's real end.
- **But the line is written late.** The attachment is appended after the `remove` — when the
  turn *absorbs* it — while keeping the arrival stamp. Delay from stamp to write: median 3 s,
  p90 25 s, max **34,049 s (9.5 h)** (903790a2, task `bw40b21kz`: stamped
  2026-09-27T17:02:08.163Z, written after a `remove` at 2026-09-28T02:29:38.020Z, line 4001).
  Using the record's timestamp for `end_ms` is therefore correct; letting the same line move
  `last_event_ms` is not.

### Out-of-order timestamps in general (the clock half)
- Over **34,041** timestamped lines in the 80 logs, lines stamped earlier than the running max
  occur in many record types: `hook_success` 3,665 (max 7 s back), `assistant` 417 (max
  1,734 s), `total_tokens_reminder` 389 (max 1,024 s), `user` 116 (max 2,071 s),
  `queued_command` 113 (max 34,049 s), plus ~20 other attachment kinds at 0 s.
- **In every large regression inspected, the record that set the max was a real event and
  the regressing lines were written late.** 903790a2 lines 3993-3997: a `queue-operation`
  `enqueue` at 17:02:08 is followed by an `assistant` message stamped 16:38:35-16:38:37 (its
  stream start) and then by nothing timestamped until line 3998 at 02:29:37.889Z the next day — so once those
  lines are written, last-wins holds the clock 23 min in the past until then. Log 686eed5a (a resumed
  session): a `remote_session_change` stamped at the resume moment 17:48:44 precedes four
  replayed history lines stamped 17:14-17:19. The max-setters seen: `enqueue`, `dequeue`,
  `compact_boundary`, `remote_session_change` — all genuine events.
- **No log ends regressed**: in 80/80 files the final timestamped line is within 1 s of the
  file's max. So no outlier ever pins a max ahead of real activity; the two rules differ only
  in transients, and in those the max was the true latest event every time.
- **Antigravity**: 78 transcripts, 3,045 steps, **7** regressions of 1-5 s, all
  `PLANNER_RESPONSE` written after a later-stamped tool step — the same late-write shape.

### Inversion check (L2)
- **Every line reaches `apply`, unfiltered**, in both the whole-file backfill
  (`crates/ai-monitor/src/lib.rs:815-817`) and the live tail (`lib.rs:1231-1234`). A fix in the
  fold is therefore visible in the panel; there is no type pre-filter upstream to widen.
- **Join order holds.** For every attachment notification naming an agent (27 events at reading
  B), the launch receipt carrying `agentId` precedes the attachment in file order, so
  `on_task_notification` finds its ring entry.
- **Only `queued_command` carries a `prompt` field** among attachment kinds (114/114).
- **Both resolver freshness reads already take the max**: `last_event_in_tail`
  (`lib.rs:682`) and Antigravity's `last_step_in_tail` (`antigravity.rs:344`). The fold is
  the only reader of these timestamps still using last-wins.

### Mutation baseline (2026-09-28, before any change)
`cargo mutants -f crates/ai-monitor/src/telemetry.rs -j 4 --cargo-test-arg -p --cargo-test-arg ai-monitor`
→ **391 mutants: 43 missed, 336 caught, 8 unviable, 4 timeouts** (56 min). **None of the 43
is in a function this spec touches** (`apply_claude`, `apply_antigravity`,
`on_task_notification`, `parse_task_notification`). By function: `parse_ts` 16,
`days_from_civil` 8, `task_id_key` 4, `fold_usage` 2, `on_tool_use` 3, `push_capped_id` 2,
`on_task_result` 2, `on_tool_result` 1, `ag_tool` 1, `scrape_skill_invocation` 1,
`is_real_model` 1, `is_async_launch` 1, `result_text` 1. The run is narrowed to ai-monitor's
tests, and narrowing can only *add* survivors, so 43 is an upper bound for the file and the
check is conservative for "zero survivors in touched code".

### Mutation result (2026-09-28, after the change — T6)
- **R6.1, diff-scoped:** `--in-diff` over `git diff main -- crates/ai-monitor/src/telemetry.rs`
  (same narrowing) → **7 mutants: 7 caught, 0 missed** (2 min). They are `observe_ts`→`()`,
  `apply_claude`→`()`, deleting the `"attachment"` arm, deleting the bare-string arm,
  `apply_antigravity`→`()`, `on_attachment`→`()`, and `!=`→`==` in `on_attachment`'s type gate.
- **R6.2, whole file:** the baseline command → **395 mutants: 43 missed, 340 caught, 8
  unviable, 4 timeouts** (58 min). 43 ≤ 43. The survivor set is **identical** to the
  baseline's once line numbers are stripped, so this change added none and removed none.
  The four extra mutants are all caught; they are the new code's.
- Besides mutation, every new guard was **falsified by hand** and restored in the same
  command:
  - deleting the attachment arm turned 8 tests red;
  - disabling the type gate turned R1.3 red;
  - reverting `observe_ts` to last-wins turned R4.1, R4.2, R4.4, R4.5, R4.6 and R4.7 red.
  - R2.2 was first written with the bare-string notification last, and stayed green with the
    arm deleted. It was reordered so the attachment is the later notification, and now
    fails without it.

## Decisions

**Complexity: Simple (1 signal — the external log's record semantics).** The two halves of the
goal are one signal *category*, not two. Where a notification is carried, and what a record's
timestamp means relative to its file position, are both properties of the one external input
this crate parses. Neither adds a subsystem, an async resource, a state machine or an actor.
The tension between them (D3 × D5) is real, and is resolved by D6 rather than by counting it as
a second signal. The L2 reviewer challenged this count; it is kept with this reasoning, and
the checkpoint coverage was re-read against the Medium bar (4-5 per dimension) without finding
an unexplored one. Brownfield; implementation checkpoints resolve from existing patterns. All checkpoints resolved (composite 1.00; three
answered by the user at L2, one at the L0 mirror, the rest auto-resolved from L1 and marked
`assumed`).

### D1: A `queued_command` attachment whose `prompt` is a string is read as a notification
- **Status**: resolved (user, L2 Q1)
- **Rationale**: A record with `type:"attachment"`, `attachment.type:"queued_command"` and a
  **string** `attachment.prompt` has that string passed to `parse_task_notification`, and a
  `Some` result to `on_task_notification`. `commandMode` is **not read**. That is the measured
  shape of all 113 absorbed notifications, and `queued_command` is the only attachment kind
  that carries a `prompt` at all.
  **Rejected: also requiring `commandMode == "task-notification"`.** Every observed value is
  that one, so it discriminates nothing today. A rename would silently bring back the 22-of-59
  LIVE ghosts, and a silent upstream format shift is this bug's own cause. It would also close
  nothing the product does not already accept: the bare-string arm takes any `user` string
  starting with the tag.
  **Rejected: any attachment's `prompt`, no type gate.** It behaves identically today, but
  would read a future attachment kind that merely copies a prompt.
  **Rejected: `contains`, or reading `rendered` / `renderedInHumanTurn`.** Those embed the block
  inside a `<system-reminder>`, and 146 other attachments quote the tag the same way.

### D2: The starts-with gate is `parse_task_notification`'s own; the attachment arm adds none
- **Status**: resolved
- **Rationale**: "Keep the starts-with check" is met by the check `parse_task_notification`
  already performs first (`telemetry.rs:2273`). The bare-string arm needs an *outer*
  starts-with test for one reason only: to return before `scan_command_name` even when the
  parse fails (predecessor R4.4a). The attachment arm has no fall-through to protect: an attachment has
  no `message.content`, and nothing after the new arm reads prose. A duplicated outer check
  there could be deleted with no observable change, which is an **equivalent mutant by
  construction** (CLAUDE.md: rewrite the condition rather than exclude the survivor). Rejected:
  mirroring the bare-string arm's shape for symmetry.

### D3: The end time is the record's top-level `timestamp` — the `ts` `apply_claude` already parses
- **Status**: resolved (user, goal)
- **Rationale**: The same `ts` every other handler receives (`telemetry.rs:603-606`), so there
  is one timestamp read per record. `attachment.timestamp` is identical in 113/113 and is not
  read, because a second field would be a second thing to agree with. The matching
  `enqueue`'s timestamp is not read either (≤ 2 ms apart in 113/113, and it lives in a
  non-goal carrier). A record whose timestamp is missing or unparseable still completes the
  agent without touching `end_ms`: `on_task_notification`'s existing `ts.is_some()` guard.

### D4: The route is a new `attachment` arm in the top-level `type` dispatch
- **Status**: assumed
- **Rationale**: `apply_claude` already dispatches on `type` for `tool_use`, `tool_result` and
  `system` (`telemetry.rs:645-661`); an attachment is a record type, not a content block, so it
  gets an arm there that calls one dedicated function (provisional name `on_attachment(v, ts)`;
  the name is the implementer's, the arm's position is not). Rejected: extending the
  `message.content` match (attachments have no `message`); rejected: folding it into
  `on_system` or `scan_command_name` (the predecessor's D12/R4.3 keeps notification parsing out
  of any function that reads prose).

### D5: `last_event_ms` is the latest parseable timestamp seen, not the last one; both dialects
- **Status**: resolved (user at L0 mirror; "both dialects" assumed from the Antigravity measurement)
- **Rationale**: One private helper (provisional name `observe_ts(t)`), called from both
  `apply_claude` and `apply_antigravity` in place of their two-line clock blocks, keeps
  `first_event_ms`'s `get_or_insert` and updates `last_event_ms` to the max of its current value
  and the new timestamp.
  `first_event_ms` keeps first-wins (Non-goals). The helper is written with `.max()` rather
  than `if t >= last` so that it has no comparison operator whose `>=`→`>` mutation is
  equivalent (assigning an equal value is a no-op). This makes the fold agree with both
  resolver freshness reads, which already take the max.
  **Rejected: exempt only `queued_command` from the clock.** It fixes 113 of the ~5,100
  regressing lines, and leaves the 23-minute `enqueue`→`assistant` case (903790a2 lines
  3994-3997) holding the clock in the past for as long as the session waits.
  **Rejected: keep last-wins** (the status quo the user chose to change).
  **Steelman against max, answered.** Last-wins self-corrects: a line stamped in the future (a
  skewed clock, a buggy writer) is overwritten by the next honest line, while max keeps it
  until wall time passes it, holding the pill at CONNECTED and IDLE near zero. Measured: in
  80/80 logs the final timestamped line is within 1 s of the file's max, so no observed line
  ever sat ahead of later activity. Every large regression inspected had a genuine event as
  its max-setter (`enqueue`, `dequeue`, `compact_boundary`, `remote_session_change`). And both
  resolvers already trust max, so this removes a disagreement rather than adding a new trust.
  **What the measurement does not answer is recovery, and the answer is that there is none in
  the fold.** Once a future-stamped line is folded, the clock is held for exactly as long as
  that stamp's lead over real activity: minutes for a skewed writer, indefinitely for a
  far-future stamp. The fold cannot bound this. It reads no clock (`telemetry.rs:14-24`), so it
  cannot tell "stamped ahead" from "just happened", and a clamp there would break the purity
  every fixture test rests on. A bound belongs to the app, which holds `now`, and is out of
  scope (D10). Only `restarted()` or a new session clears it. This is accepted as the price of
  max, by name, in G1 — not argued away by the frequency figure.

### D6: The end time and the clock are independent readings of the same `ts`
- **Status**: resolved (Tier-3 tension D3 × D5)
- **Rationale**: The absorbed attachment is exactly the line whose timestamp the clock must
  *ignore* (D5) and the notification must *use* (D3). So every handler receives the record's
  own `ts` whether or not it moved the clock, and no handler ever reads `last_event_ms` as its
  timestamp. Rejected: computing `ts` after the clock update and passing the clock (that
  would stamp every absorbed agent with the absorb time — up to 9.5 h late).

### D7: A new fixture excerpted from 903790a2, with trimmed prose and verbatim structure
- **Status**: resolved (user, L2 Q2)
- **Rationale**: `crates/ai-monitor/tests/fixtures/absorbed_notifications.jsonl`; a separate file
  so `session.jsonl`'s whole-file assertion does not move (precedent: context-window D13).
  Contents, in original file order:
  - the 9 `Agent` launches (`tool_use`) and their 9 receipts (`toolUseResult.agentId`,
    `isAsync: true`, `status: "async_launched"`), frozen to the nine the goal names, even
    though the live log has since gained a tenth. Each agent and its attachment `timestamp`
    (= expected `end_ms`):
    `afd416a8f7546697f` 2026-09-27T04:15:08.124Z · `a247b7427fa150b0d` 04:20:21.050Z ·
    `adfbed2bb27982ba3` 04:22:09.879Z · `a9a2ad3f29d169432` 15:42:51.182Z ·
    `a365b66b6c3ae93e3` 16:21:53.575Z · `a3428c6053180c66b` 16:37:38.556Z ·
    `a532c4219ce60ec6d` 2026-09-28T02:43:48.839Z · `ae2e7100cb930ae73` 06:41:56.392Z ·
    `acbbc9c3f6509c951` 06:52:37.869Z;
  - the 9 agent `queued_command` attachments, including their `rendered` copies;
  - those notifications' `queue-operation` `enqueue`/`remove` lines;
  - at least one `Bash` notification by each carrier (bare-string `user` and attachment);
  - one `hook_success` record quoting the tag in `stdout` (903790a2 has four: lines 2884,
    3000, 3024, 3963) and one `prompt_snapshot` quoting it in `tools[12].description`
    (lines 32, 2423), each trimmed to the quoting field;
  - lines 3993-4001 intact as a sequence: the `enqueue`, the 16:38-stamped `assistant`
    lines written after it, and the `bw40b21kz` attachment stamped 9.5 h before the line it
    follows.
  Kept verbatim: `type`, `timestamp`, `uuid`/`parentUuid`, ids, `attachment.type`,
  `commandMode`, `toolUseResult` keys, tag names and their order. Replaced with short
  placeholders: `input.prompt`, `tool_result` bodies, `<summary>`/`<result>` bodies, and
  `rendered*` prose except the embedded tag block. Rejected: a verbatim excerpt (hundreds of KB
  of conversation) and the whole 12 MB log (drifting assertions).

### D8: Mutation gate — zero survivors in touched code; the file's total does not rise
- **Status**: resolved (user, L2 Q3)
- **Rationale**: "Touched code" is defined **mechanically, by the diff**, not by a list of
  function names, which could not name functions this change creates (`on_attachment`,
  `observe_ts`, or whatever the implementer calls them). Two runs, both narrowed exactly as
  the baseline was:
  1. `git diff main -- crates/ai-monitor/src/telemetry.rs > <scratch>/change.diff`, then
     `cargo mutants -f crates/ai-monitor/src/telemetry.rs --in-diff <scratch>/change.diff -j 4
     --cargo-test-arg -p --cargo-test-arg ai-monitor` → **0 missed**. `--in-diff` includes
     only mutants in lines the diff touches, which covers new helpers and edited lines of
     existing functions alike.
  2. The baseline command, `cargo mutants -f crates/ai-monitor/src/telemetry.rs -j 4
     --cargo-test-arg -p --cargo-test-arg ai-monitor` → **≤ 43 missed**. Record the count
     beside the baseline, per CLAUDE.md's "record the survivor count, not a ratio".
  Narrowing can only add survivors, so a narrowed 0 is conservative for gate 1. The 43
  pre-existing survivors are Known Gap G4, left for a separate spec. Rejected: also clearing
  `parse_ts` (16), and driving the file to 0 (both user-declined as out of scope).

### D9: Prose that this change makes false is corrected where it stands
- **Status**: assumed
- **Rationale**: Three places:
  - the bare-string arm's "the ONLY signal a background agent ever ends by"
    (`telemetry.rs:682-683`);
  - the `last_event_ms` field and module docs, if they imply last-wins;
  - `specs/agent-elapsed-sidechain/spec.md`, whose D9 correction and Known Gaps ("roughly one
    async launch in eight becomes a permanent LIVE ghost") get a dated pointer to this spec
    rather than a rewrite. They record what was true when that spec shipped.
  Rejected: leaving them. A comment that says "ONLY" is the kind a later reader trusts over
  the code.

### D10: No contract change — `TelemetrySnapshot` and the app are untouched
- **Status**: assumed
- **Rationale**: The fix changes values the fold already emits (`completed`, `end_ms`,
  `ended_ok`, `last_event_ms`), not their types or meaning to the app. `dashboard.rs`'s
  `agent_elapsed` `e >= s` guard holds because a notification's time is always after its launch.
  Rejected: a counter of attachment-routed notifications (the predecessor's D8 refused an
  unmatched-notification counter on the same grounds: it widens the versioned contract for a
  number nothing renders).

## Constraints
- `SessionTelemetry::apply` stays pure over `&str`, reads no clock or filesystem, and never
  panics; no `unwrap()` outside tests (`telemetry.rs:14-24`, CLAUDE.md).
- Changes stay inside `crates/ai-monitor` (source, tests, fixture) plus spec/doc prose. No new
  dependency, so dependency direction is unaffected.
- Notification recognition is starts-with, never `contains`; each tag is read at its first
  occurrence (predecessor D12/R4.5).
- The bare-string arm's observable behaviour is byte-identical; every existing predecessor R4.x test
  passes unmodified.
- `session.jsonl`, `context_command.jsonl` and `antigravity_transcript.jsonl` are not edited;
  the new fixture contains no `isAsync` key with JSON `false` (`tests/telemetry.rs:1171`).
- Handlers receive the record's own `ts`, never `last_event_ms` (D6).
- Tests read fixtures via `include_str!` only: no live log, no clock (`tests/telemetry.rs:4`).
- `.rs` edits go through Edit/Write so the rustfmt PostToolUse hook runs.

## Known Gaps
- **G1 — max trusts a future-stamped line, with no recovery in the fold** (D5's steelman).
  The clock stays held for as long as the bogus stamp leads real activity. That is unbounded
  for a far-future stamp, and only `restarted()` or a new session clears it. Measured 0
  occurrences over 80 logs. If it happens, the symptom is a pill stuck on CONNECTED and an IDLE
  tile near zero for a quiet session. The bound, if one is ever wanted, belongs in the app
  (it holds `now`); the fold cannot hold one without reading a clock.
- **G2 — `first_event_ms` stays first-line-wins.** 49 lines in 25 logs are stamped before their
  file's first line; 45 by under a minute, and 4 by up to 2,071 s, all in one resumed session
  (686eed5a) whose replayed history precedes its resume marker.
- **G3 — the route rests on external strings**: `attachment.type:"queued_command"`, the
  `prompt` field name, and the tag. If any is renamed, the LIVE ghosts return silently. This
  is the same kind of fact as architecture.md G5/G6: owned upstream, and nothing here would be
  told. Symptom to look for: agents stuck `●` while the log holds `queue-operation` `remove`
  records with reason `absorbed_mid_turn`.
- **G4 — 43 pre-existing mutation survivors** in `telemetry.rs` (per-function list in
  § Mutation baseline), none in touched code. Out of scope by D8; a candidate for its own spec.
- **G5 — unread carriers**: `queue-operation` records and notifications `delivered_to_agent`
  (Non-goals). Neither is needed for any agent measured today.
- **G6 — truncation still drops live agents**: `restarted()` clears the ring, so a
  notification after a truncation finds nothing to join. Predecessor's gap, unchanged.

## Requirements

Test locations: fold-level GWTs go in `crates/ai-monitor/tests/telemetry.rs`, in a new section
after the three `agent-elapsed-sidechain` sections (ending before `// ---- R8 robustness`, currently `:1554`), using its `fold` / `notification` /
`async_launch` helpers. Fixture GWTs read `fixtures/absorbed_notifications.jsonl` via
`include_str!`. Requirement ids below are **this spec's**; the predecessor's are always written
"predecessor R*n*".

### R0: The reported session folds to nine finished agents (Confirmed Goal)

#### R0.1: Every fixture agent completes at its attachment's timestamp
- **Behavior**: Folding the 903790a2 fixture ends all nine agents at their attachment times
- **Given**: `absorbed_notifications.jsonl` (D7), holding the nine launches, receipts and `queued_command` attachments
- **When**: the whole fixture is folded
- **Then**: `subagents` holds exactly 9 entries, every one `completed == true`, and each `end_ms` equals the epoch-ms of that agent's attachment `timestamp` as listed in D7 (e.g. `a532c4219ce60ec6d` → 1790563428839)

#### R0.2: Every fixture agent reads as a success
- **Behavior**: Nine `<status>completed</status>` attachments yield nine successes
- **Given**: the same fixture, all nine agent attachments carrying `<status>completed</status>`
- **When**: it is folded
- **Then**: all 9 entries have `ended_ok == true`

#### R0.3: The fixture is not vacuous — only the attachments end the agents
- **Behavior**: Removing the `queued_command` lines leaves all nine agents live
- **Given**: the fixture's text with every line containing `"queued_command"` filtered out, in the test itself
- **When**: the remainder is folded
- **Then**: all 9 entries have `completed == false` and `end_ms == None` — the pre-change baseline measured on the real log — proving no other line in the fixture ends them

### R1: A `queued_command` attachment is a notification carrier (D1, D2, D3, D4)

#### R1.1: A matching attachment ends its agent at the record's timestamp
- **Behavior**: An attachment naming a ring agent completes it with that record's time
- **Given**: `async_launch("a1", "agent-11")` folded, then an attachment line `{"type":"attachment","timestamp":T,"attachment":{"type":"queued_command","commandMode":"task-notification","prompt":notification("agent-11","completed",…),"timestamp":T}}`
- **When**: the attachment line is folded
- **Then**: the entry has `completed == true`, `end_ms == Some(epoch(T))`, `ended_ok == true`

#### R1.2: `commandMode` is not part of the gate
- **Behavior**: An attachment with no or a different `commandMode` still ends the agent
- **Given**: the R1.1 setup, run twice: once with the `commandMode` key absent, once with `"commandMode":"background-task"`
- **When**: each attachment line is folded into a fresh accumulator
- **Then**: in both, the entry has `completed == true` and `end_ms == Some(epoch(T))`

#### R1.3: Another attachment kind carrying a notification-shaped `prompt` is inert
- **Behavior**: The type gate rejects a non-`queued_command` attachment
- **Given**: the R1.1 setup with `attachment.type` set to `"hook_success"` (the `prompt` string unchanged, starting with the tag)
- **When**: it is folded
- **Then**: the entry stays `completed == false`, `end_ms == None`

#### R1.4: A non-string or missing `prompt` is inert and never an error
- **Behavior**: A `queued_command` without a string prompt changes nothing
- **Given**: the R1.1 setup, with `prompt` in turn absent, `null`, an array `[notification(…)]`, and an object
- **When**: each is folded into a fresh accumulator
- **Then**: the entry stays `completed == false`, `parse_errors == 0`, and no panic occurs

#### R1.5: A `prompt` that quotes the tag without starting with it is inert
- **Behavior**: Starts-with, not contains, on the attachment carrier too
- **Given**: the R1.1 setup whose `prompt` is `"<system-reminder>\n[SYSTEM NOTIFICATION]\n\n" + notification("agent-11","completed",…)` — the shape of the record's own `rendered` field
- **When**: it is folded
- **Then**: the entry stays `completed == false`, `end_ms == None`

#### R1.6: Leading whitespace before the tag is tolerated, as on the bare-string arm
- **Behavior**: `trim_start` applies to the attachment `prompt` as well
- **Given**: the R1.1 setup whose `prompt` is `"\n  " + notification("agent-11","completed",…)`
- **When**: it is folded
- **Then**: the entry has `completed == true`

#### R1.7: A tagged `prompt` with no `<task-id>` is inert and registers nothing
- **Behavior**: A malformed notification on the attachment carrier moves no state
- **Given**: the R1.1 setup whose `prompt` is the predecessor R4.4a `malformed` string (tag, `<status>`, a `<result>` quoting `<command-name>/deploy</command-name>`, no `<task-id>`)
- **When**: it is folded
- **Then**: the entry stays `completed == false`, `skills` contains no `deploy`, and `tool_calls` / `failed_tool_calls` equal their values before the line

#### R1.8: A notification for an id with no ring entry is ignored and uncounted
- **Behavior**: A background `Bash` task's attachment creates nothing
- **Given**: a ring holding `agent-11` only, and an attachment naming `bk1goe5ix`
- **When**: it is folded
- **Then**: `subagents` is unchanged (one entry, not completed) and every counter in the snapshot equals its value before the line

#### R1.9: `<status>` maps to `ended_ok` exactly as on the bare-string arm
- **Behavior**: `failed` reads as a failure; an absent status does not
- **Given**: two runs of the R1.1 setup, one with `<status>failed</status>`, one whose `prompt` has no `<status>` tag at all
- **When**: each is folded
- **Then**: the first has `completed == true, ended_ok == false`; the second `completed == true, ended_ok == true`

#### R1.10: A timestamp-less attachment completes without erasing an end
- **Behavior**: The `ts.is_some()` guard holds on the new carrier
- **Given**: `agent-11` launched and already ended by a bare-string notification at `T1`; then an attachment for `agent-11` with no top-level `timestamp` (and, in a second run, `"timestamp":"not-a-date"`)
- **When**: the attachment is folded
- **Then**: `completed == true` and `end_ms == Some(epoch(T1))` — unchanged

#### R1.11: The attachment is not a tool call and not a skill invocation
- **Behavior**: Folding attachments moves no call counter and registers no skill
- **Given**: the R1.1 setup whose `<result>` body contains `<command-name>/deploy</command-name>`
- **When**: it is folded
- **Then**: `tool_calls` and `failed_tool_calls` equal those of the same fold without the attachment line, and `skills` contains no `deploy`

#### R1.12: The fixture's quoting records are inert
- **Behavior**: `hook_success` and `prompt_snapshot` records that contain the tag end nothing
- **Given**: the fixture, and the same fixture with its `hook_success` and `prompt_snapshot` lines filtered out in the test
- **When**: both are folded
- **Then**: the two snapshots have equal `subagents` and `skills`

### R2: The carriers compose without double counting (D1, Non-goals)

#### R2.1: `queue-operation` records do not end an agent
- **Behavior**: The queue carrier stays unread
- **Given**: `async_launch("a1","agent-11")`, then the fixture-shaped `{"type":"queue-operation","operation":"enqueue",…,"content":notification("agent-11",…)}` and its `"operation":"remove","reason":"absorbed_mid_turn"` line
- **When**: they are folded
- **Then**: the entry stays `completed == false`, `end_ms == None`

#### R2.2: An agent notified by both carriers ends once, at the later notification
- **Behavior**: Attachment then bare-string for one id is a re-notification, not a duplicate
- **Given**: `agent-11` launched, then an attachment notification at `T1`, then a bare-string notification for `agent-11` at `T2 > T1`
- **When**: both are folded
- **Then**: `subagents` has exactly one entry for `agent-11`, `completed == true`, `end_ms == Some(epoch(T2))`

### R3: The bare-string arm is unchanged (Constraints)

#### R3.1: The predecessor's notification tests pass without modification
- **Behavior**: Existing starts-with, first-id, phantom-skill and timestamp-less guards stay green
- **Given**: the three `agent-elapsed-sidechain` test sections at `tests/telemetry.rs:767-1553` (predecessor R4-R8) with no edits to their bodies (`git diff main` shows none inside those functions)
- **When**: `cargo test -p ai-monitor` runs
- **Then**: every one of them passes

### R4: The session clock never runs backwards (D5, D6)

#### R4.1: An earlier timestamp does not lower `last_event_ms` (Claude)
- **Behavior**: A late-written line cannot move the clock into the past
- **Given**: a Claude fold whose last line was stamped `2026-09-28T02:29:38.020Z`
- **When**: a line stamped `2026-09-27T17:02:08.163Z` is folded (any record type)
- **Then**: `last_event_ms == Some(1790562578020)`

#### R4.2: An earlier `created_at` does not lower `last_event_ms` (Antigravity)
- **Behavior**: The same rule on the other dialect
- **Given**: an Antigravity fold whose last step has `created_at` `T + 5 s`
- **When**: a step with `created_at` `T + 2 s` is folded (the measured `PLANNER_RESPONSE` shape)
- **Then**: `last_event_ms == Some(epoch(T + 5 s))`, and `turns_*` still count the step

#### R4.3: A later timestamp still advances the clock, in both dialects
- **Behavior**: Max does not freeze forward progress
- **Given**: a fold whose clock reads `T`
- **When**: a line (Claude) / step (Antigravity) stamped `T + 1 ms` is folded
- **Then**: `last_event_ms == Some(T + 1 ms)`

#### R4.4: `first_event_ms` stays first-line-wins
- **Behavior**: The non-goal is pinned so it cannot drift
- **Given**: a fold whose first line was stamped `T`
- **When**: a line stamped `T − 60 s` is folded
- **Then**: `first_event_ms == Some(epoch(T))`

#### R4.5: The fixture's clock, mid-sequence, at the 9.5-hour line
- **Behavior**: The absorbed `bw40b21kz` attachment leaves the clock at the line before it
- **Given**: the fixture's lines up to and including the `bw40b21kz` `queued_command` attachment
- **When**: they are folded
- **Then**: `last_event_ms == Some(1790562578020)` (the preceding `remove`), not 1790528528163

#### R4.6: The fixture's clock, mid-sequence, after the late-written `assistant` lines
- **Behavior**: Assistant lines written after the 17:02 `enqueue` do not pull the clock back
- **Given**: the fixture's lines up to and including 903790a2's line 3997 (the last 16:38-stamped `assistant` line)
- **When**: they are folded
- **Then**: `last_event_ms == Some(1790528528164)` (the `enqueue`), not 1790527117906

#### R4.7: One late-written line both ends an agent in the past and leaves the clock alone
- **Behavior**: End time and clock read the same `ts` independently (D6)
- **Given**: the fixture's lines up to and including `a532c4219ce60ec6d`'s attachment (903790a2 line 4235), which follows its own `remove` stamped `2026-09-28T02:44:10.891Z`
- **When**: they are folded
- **Then**: that agent's `end_ms == Some(1790563428839)` **and** `last_event_ms == Some(1790563450891)`

#### R4.8: The clock tests are falsified before they are trusted
- **Behavior**: Reverting the helper to last-wins turns the clock tests red
- **Given**: the clock helper temporarily rewritten to assign unconditionally (the pre-change rule)
- **When**: R4.1, R4.2, R4.5, R4.6 and R4.7 run
- **Then**: each fails; the helper is restored in the same command and all pass again (recorded in the implementation notes, not committed)

#### R4.9: Timestamp-less and unparseable lines still leave the clock alone
- **Behavior**: The existing `timestampless_line_does_not_move_the_clock` / `unparseable_timestamp_is_ignored_not_defaulted` behaviour survives the helper
- **Given**: the tests at `tests/telemetry.rs:150` and `:161`, unmodified
- **When**: they run
- **Then**: both pass

### R5: The fixture is a faithful, trimmed excerpt (D7)

#### R5.1: Every fixture line is valid JSON and folds cleanly
- **Behavior**: The fixture parses without errors
- **Given**: `absorbed_notifications.jsonl`
- **When**: it is folded
- **Then**: `parse_errors == 0`

#### R5.2: The fixture carries the sequences the clock requirements need
- **Behavior**: R4.5-R4.7's prefixes exist in the file, in order
- **Given**: the fixture's lines in file order
- **When**: they are scanned in the test (by `uuid` or by `task-id` / timestamp)
- **Then**: the 17:02:08.164Z `enqueue` precedes the three 16:38-stamped `assistant` lines, which precede the `bw40b21kz` attachment; and `a532c4219ce60ec6d`'s `remove` immediately precedes its attachment

#### R5.3: The fixture obeys the existing fixture guard
- **Behavior**: No shape that never occurs is encoded
- **Given**: the new fixture in `tests/fixtures/`
- **When**: `no_fixture_encodes_a_shape_that_never_occurs` runs
- **Then**: it passes and its `checked` count includes the new file

#### R5.4: The existing fixtures are untouched
- **Behavior**: No other fixture's assertions move
- **Given**: the finished change
- **When**: `git diff main -- crates/ai-monitor/tests/fixtures/session.jsonl crates/ai-monitor/tests/fixtures/context_command.jsonl crates/ai-monitor/tests/fixtures/antigravity_transcript.jsonl` runs
- **Then**: the output is empty

### R6: Mutation coverage (D8)

#### R6.1: No survivor in the changed lines
- **Behavior**: Every mutant the diff introduces is killed
- **Given**: `git diff main -- crates/ai-monitor/src/telemetry.rs` saved to a scratch file
- **When**: `cargo mutants -f crates/ai-monitor/src/telemetry.rs --in-diff <that file> -j 4 --cargo-test-arg -p --cargo-test-arg ai-monitor` runs
- **Then**: it reports **0 missed** (timeouts and unviables are reported, not gated)

#### R6.2: The file's survivor total does not rise
- **Behavior**: The whole-file count is at or below the baseline and is recorded
- **Given**: the baseline of 43 missed (2026-09-28, § Research › *Mutation baseline*)
- **When**: `cargo mutants -f crates/ai-monitor/src/telemetry.rs -j 4 --cargo-test-arg -p --cargo-test-arg ai-monitor` runs on the finished change
- **Then**: it reports **≤ 43 missed**, and the count, date and mutant total are written into this spec beside the baseline

### R7: Prose matches the code (D9)

#### R7.1: The "ONLY signal" comment is corrected
- **Behavior**: The bare-string arm's comment names both carriers
- **Given**: the finished `telemetry.rs`
- **When**: it is searched for `ONLY signal`
- **Then**: there is no match, and the comment on the bare-string arm names the `queued_command` attachment as the other carrier

#### R7.2: The clock's documentation states the max rule
- **Behavior**: `last_event_ms`'s field docs describe latest-seen, not last-folded
- **Given**: the finished `telemetry.rs`
- **When**: the `last_event_ms` field doc (snapshot and accumulator) is read
- **Then**: it states that the value is the latest timestamp seen and that earlier-stamped lines do not lower it

#### R7.3: The predecessor spec points here
- **Behavior**: The predecessor's deferred follow-up is marked as taken up
- **Given**: `specs/agent-elapsed-sidechain/spec.md`
- **When**: its D9 correction and its Known Gaps entry on the LIVE ghost are read
- **Then**: each carries a dated (2026-09-28) line pointing to `specs/ai-monitor-absorbed-notifications/spec.md`, and their original text is otherwise unchanged

### R8: No contract change (D10)

#### R8.1: The app crate and the snapshot's shape are untouched
- **Behavior**: Only values change, not the versioned contract
- **Given**: the finished change
- **When**: `git diff main -- crates/app` runs, and `TelemetrySnapshot`'s field list is compared with `main`
- **Then**: the diff is empty and the field list is identical

#### R8.2: The consumer side of the boundary already renders what the fold now produces
- **Behavior**: A completed agent with an `end_ms` renders a frozen duration, and the pill reads the clock it is given
- **Given**: the unmodified app tests `a_live_agent_elapsed_grows_and_a_completed_one_is_frozen` (`crates/app/src/dashboard.rs:2164`) — the render half of R0.1's `completed == true, end_ms == Some(…)` — and `r5_2_the_status_pill_has_exactly_three_states` (`dashboard.rs:3239`), which drives the pill from `last_event_ms`
- **When**: `cargo test -p app` runs on the finished change
- **Then**: they pass unmodified; this spec adds no app test because it changes no app code (R8.1), and the fold-side sub-requirements (R0, R1, R4) are the producer half of the same contract

### R9: Workspace gates (Done)

#### R9.1: The workspace builds, tests and lints clean
- **Behavior**: The standard gates pass
- **Given**: the finished change
- **When**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` run
- **Then**: both exit 0 (a failure in a live-machine test is attributed against `main` in a worktree before it is called a regression)

## Tasks

Files each task touches are listed so the parallelism claims can be checked:
`F` = `crates/ai-monitor/tests/fixtures/absorbed_notifications.jsonl`,
`S` = `crates/ai-monitor/src/telemetry.rs`, `U` = `crates/ai-monitor/tests/telemetry.rs`,
`P` = `specs/agent-elapsed-sidechain/spec.md`, `Z` = this spec.

### T1: Extract the 903790a2 fixture [infra]
- **Fulfills**: R5.3, R5.4
- **Depends on**: (none) — parallel with T2 (touches only `F`)
- **Files**: `F` (new)
- **Notes**: Build it with a throwaway `jq` script in the scratchpad (not committed), from the
  source log named in § External Dependencies. D7 lists the lines, the nine agents and what to
  trim. Before handing off, check it with `jq` alone: every line parses; the R5.2 order holds;
  there is no `isAsync` key with JSON `false`; and `grep -c queued_command` = the 9 agent
  attachments plus the included `Bash` ones. The file's *tests* are T4's.

### T2: Route `queued_command` attachments through the notification path [vertical]
- **Fulfills**: R1.1-R1.11, R2.1, R2.2, R3.1
- **Depends on**: (none) — parallel with T1 (touches `S`, `U`; no fixture)
- **Files**: `S`, `U`
- **Notes**: The `Some("attachment")` arm in `apply_claude`'s `type` dispatch, plus one
  function reading `attachment.type` / `attachment.prompt` (D1, D2, D4). It passes the
  record's own `ts` (D3, D6). All GWTs here use synthetic lines built with the existing
  helpers, not the fixture. Its slice runs end to end: log line → fold → `Subagent` fields the
  app already renders (R8.2 is the consumer half, verified in T6).

### T3: Make `last_event_ms` monotonic in both dialects [vertical]
- **Fulfills**: R4.1-R4.4, R4.9, R4.8 (synthetic half: R4.1, R4.2)
- **Depends on**: T2 (same files `S`, `U`)
- **Files**: `S`, `U`
- **Notes**: One `.max()`-based helper replaces the two clock blocks (`telemetry.rs:608-611`,
  `:743-746`) (D5). `first_event_ms` keeps `get_or_insert`. Falsify R4.1/R4.2 against a
  temporary last-wins body, restoring it in the same command.

### T4: Fixture-level GWTs [vertical]
- **Fulfills**: R0.1-R0.3, R1.12, R4.5-R4.7, R4.8 (fixture half: R4.5-R4.7), R5.1, R5.2
- **Depends on**: T1, T2, T3
- **Files**: `U`
- **Notes**: Prefix folds (R4.5-R4.7) cut the fixture at a line found by content (`uuid` or
  task-id), never by a hard-coded line number, so a re-trimmed fixture does not silently move
  the cut. Falsify R4.5-R4.7 against last-wins, as in T3.

### T5: Correct the prose this change falsifies [infra]
- **Fulfills**: R7.1, R7.2, R7.3
- **Depends on**: T3 (same file `S`) — parallel with T4 (T5 touches `S` comments and `P`;
  T4 touches only `U`)
- **Files**: `S` (comments and docs only), `P`
- **Notes**: Doc and comment edits only; no code token in `S` changes. `P` gets dated pointer
  lines, not rewrites.

### T6: Mutation and contract gates, with counts recorded [infra]
- **Fulfills**: R6.1, R6.2, R8.1, R8.2, R9.1
- **Depends on**: T4, T5
- **Files**: `U` (only if a survivor needs a killing test), `Z` (the recorded counts)
- **Notes**: R6.1's `--in-diff` run first. A survivor there is a missing test, added to `U`,
  unless it is shown equivalent by falsification, in which case the condition is rewritten
  rather than excluded (CLAUDE.md). Then R6.2's whole-file run (about an hour), R8's
  `git diff main -- crates/app` and the two app tests, then R9's workspace test and clippy.
  Record both mutation counts beside § Research › *Mutation baseline*.

## External Dependencies

### Pre-work
- **The fixture's source log must still exist when T1 runs.**
  `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal-feat-improve-card-ui/903790a2-4d8c-4f21-ba37-9e619cddbcf7.jsonl`
  is in Claude Code's rolling ~30-day window; during this spec another log (`538f4d49`) aged
  out of it. Build T1 well before ~2026-10-27, or copy the raw log somewhere durable first.
  The session is also still live and appending; the fixture freezes the nine agents D7 names,
  so later growth does not matter.

### Post-work
- (none) — the mutation counts are recorded by T6 inside this spec. Committing, opening a PR
  and merging are the user's decisions and are not part of the plan.
