# Spec: claude-background-working

## Meta
- **Created**: 2026-10-22
- **Type**: dev
- **Status**: implemented (T1–T5; R0.1 live check not run)
- **Approved by**: user
- **Approved at**: 2026-10-22
- **Branch**: fix/claude-background-working
- **Sibling**: `specs/agy-background-working`, the same rule for Antigravity. It is in PR #54
  (branch `fix/agy-background-working`), not on this branch. Every rule this spec needs from it
  is restated here

## Goal
A Claude Code sidebar card reads "Working", not "Waiting for input", while background work
Claude started in the current turn (a `run_in_background` Bash, an async subagent) has not
reported back. This is the same rule as `specs/agy-background-working` for Antigravity. The
user chose this over leaving `Waiting` or adding a new status (2026-10-22).

## Non-goals
- Antigravity (PR #54), Codex, Gemini and Aider.
- Screen-based dialog detection (`prompt.rs`). A permission or question dialog still reads
  `Waiting`, and the card's precedence (`resolve_status`) is unchanged.
- Carrying pending work across user prompts. Only work started since the last human prompt
  counts (user decision at L0).
- A new card status or `LastMessage` variant.

## Confirmed Goal
When the last meaningful record of a Claude Code session log is an `end_turn` assistant
message, and background work started since the last human prompt has **not yet reported
back**, the card reads **Working** for as long as that stays true. Once everything that turn
started has reported back, behaviour is exactly today's. Done when:
- a log whose last turn ended with a background task still running classifies as
  `AssistantBusy` (→ `Working`), at any age;
- a completion of that work (any form in D3) returns it to today's classification;
- background work started before the last human prompt does not count;
- a re-measurement against the real `~/.claude/projects` corpus confirms the rule (inferred);
- `docs/architecture.md` G8 names the new Claude Code log strings this depends on (inferred).

## Research
- **Today's rule.** `ai_monitor::activity::tail_state` reads the final `TAIL_CAP` (8 KiB) and
  classifies the last record whose `message.role` is `user`/`assistant`
  (`crates/ai-monitor/src/activity.rs:202-235`). `classify_assistant` maps `stop_reason ==
  "end_turn"` to `AssistantEndTurn` unconditionally (`activity.rs:284-296`), which
  `state_from` maps to `Waiting` (`crates/app/src/tabmeta/activity.rs:66-90`). Nothing looks
  at background work. There is no back-scan: unlike Antigravity's `tail.rs` (D14 of
  `agent-card-status`), the Claude tail is the only read `tail_state` makes.
- **The app already reads 1 MiB of a changed log.** `tail_role` calls `tail_state` and then
  `recent_prompts` (up to `PROMPT_SCAN_CAP`, 1 MiB) for the tab label, both only when the log's
  `(mtime, len)` changed (`crates/app/src/tabmeta/activity.rs:441-485`, R6.7 memo).
  `probe::LOG_READ_BYTES` accounts `min(len, TAIL_CAP) + min(len, PROMPT_SCAN_CAP)` by
  arithmetic (tab-metadata R6.1's stated honest limit).
- **Record shapes** (96 logs, `~/.claude/projects/*/*.jsonl`, read 2026-10-22 05:34–05:38 UTC).
  Every background start is a `type: "user"` record whose top-level `toolUseResult` object
  carries a structured id. Measured combinations, with how many ids later received a
  `<task-notification>`:

  | `toolUseResult` fields | what | count | notified |
  |---|---|---|---|
  | `isAsync: true`, `status: "async_launched"`, `agentId` | `Agent` with `run_in_background` | 203 | 203 |
  | `backgroundTaskId` | `Bash` with `run_in_background`, or a command moved to the background at its timeout (`timedOutAfterMs`) | 92 | 90 |
  | `resumedAgentId` (14 also `revivedAgentId`) | `SendMessage` resuming a stopped agent | 34 | 34 (+14 revived) |
  | `background: true`, `status: "forked"`, `agentId` | a skill forked into the background (`/code-review`) | 6 | 6 |

  A foreground `Agent` result carries no `agentId` in `toolUseResult`. This table was read at
  ~05:37 UTC. The completion breakdown below was read at 05:34 UTC, while agent `a03fb09c…`
  (form 2, started 05:32) was still running. It had notified by 05:37, which is why the
  `Agent` row reads 203/203 here and the breakdown reads 292 of 295 with one agent still
  running. The live corpus moves between readings.
- **Completion shapes** (same corpus). Per start, which forms closed it:
  - a `queue-operation` record, `operation: "enqueue"`, `content` beginning
    `<task-notification>\n<task-id><id></task-id>`. Present for 292 of 295 starts. Written when
    the result arrives, whatever Claude is doing.
  - a `user` record with `origin: {"kind": "task-notification"}` (`promptSource: "system"`),
    content `<task-notification>…<task-id><id></task-id>`. 144 of 295: the notification woke an
    idle Claude.
  - an `attachment` record, `attachment.type: "queued_command"`, `commandMode:
    "task-notification"`, `prompt` carrying the same `<task-id>`. 153 of 295: it landed
    mid-turn.
  - a `TaskStop` result: `toolUseResult` with `task_id` and `message: "Successfully stopped task:
    <id> …"`. Stopping writes **no** notification; these are 2 of the 3 starts with no
    `<task-notification>`. The third (`a03fb09c…`) was an agent still running at measurement time.
  With those four, every finished start has a completion.
- **`origin.kind` and `isMeta` on non-tool `user` records**: `human` (typed 334,
  `suggestion_accepted` 167, sdk/none 66), `task-notification` 184, `peer` 109 (all `isMeta`,
  "Another Claude session sent a message"), no `origin` 263 (older records; slash-command
  wrappers, `<bash-input>`, continuation summaries), `isMeta` 187.
- **`api-request-blob` records carry `message.role`** (`system` 831, `assistant` 468, `user` 342)
  and no `toolUseResult`/`origin`. `last_meaningful` keys on `message.role` only, so it treats a
  blob as a message. Harmless today: a blob after an `end_turn` was always followed by a real
  `user` record (94/94), and no log ends on one (0/95). The new walk must key on the record's
  `type` (`user`/`assistant`), or a `user`-role blob would read as a human prompt.
- **Measurement of the rule** (scratch `claude_rule_at_end.py`, 2026-10-22 05:37 UTC, 96 logs, 613
  `end_turn` turns with a human prompt before them; only the **last** record of each assistant
  message; state taken **at the `end_turn`**, i.e. completions written after it are not counted;
  outcome = the next `user`/`assistant` record):

  | next record | pending | nothing pending |
  |---|---|---|
  | `task-notification` (Claude woke on its own) | **155** | 21 |
  | `peer` message (an agent or session messaged it) | **94** | 3 |
  | a human prompt | 22 | **489** |
  | end of file | 0 | 85 |
  | other (meta, API error) | 5 | 15 |

  Of the 21 notification turns with nothing pending: 13 had completed (enqueued) before the
  `end_turn` and were only being delivered, and 8 were started before the last human prompt
  (excluded by the L0 decision). The 22 pending-then-human turns are the user typing while
  work ran.
- **How far back.** The pending start sat a median 16 KiB, p90 133 KiB, max 535 KiB before its
  `end_turn`; 190 of 276 were beyond the 8 KiB tail, none beyond 1 MiB. The last human prompt
  sat a median 67 KiB back, p90 725 KiB, and beyond 1 MiB for 46 of 613.
- **Sibling design.** `specs/agy-background-working` D1–D8: pairing, newest-first walk with
  early return on a pending start, completions after the reply count, no time cap, reuse
  `AssistantBusy`, conditional back-scan only after a reply, register strings in G8.
- **Other `tail_state` callers.** Only `crates/app/src/tabmeta/activity.rs` (`tail_role`). The
  ai-monitor panel's telemetry does not use it.
- **Commands.** `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D
  warnings`; `cargo mutants -f crates/ai-monitor/src/activity.rs -j 4`.

## L2 Checkpoints
Complexity: Medium (2 signals): an external format read as data (Claude Code's log record
shapes); a small start/completion pairing state over a conditional second read. Brownfield:
Implementation auto-resolves from L1 and the sibling spec.

| Dimension | Checkpoints | Resolved by |
|---|---|---|
| Core Behavior | pending → which state (D1); what "started" is (D2); what "finished" is (D3); which records end the turn's span (D4) | L0 + L1 + sibling |
| Scope | only work since the last human prompt (L0, D4); which background tools count, and Monitor/cron/wakeups do not (D2); downstream unchanged (D5) | L0 + L1 |
| Error/Edge | work that never reports (D6); a back-scan that cannot reach the prompt (D4); a record type that carries `message.role` without being a message (`api-request-blob`, D4); a renamed field (D8); a stopped task (D3); work completed before the `end_turn` but delivered after it (D3/D4); a resume reusing an id (D3); compaction (D4); a half-written newest line (D4) | L1 + assumed |
| Data Model | ids from structured `toolUseResult` fields, compared as exact strings (D2/D3); sets (D3); no new `LastMessage` variant (D5) | L1 |
| Implementation | where the back-scan lives and what it costs (D7); tests and mutation gate (D9) | L1 + sibling |

Composite 1.0 from L1 (D4, D6 and D7 `assumed`). Inversion probe: "every requirement met, still
fails?":
- (a) Claude Code renames the start fields → nothing pending, today's `Waiting`, the safe
  direction.
- (b) Claude Code renames **all three** notification forms → every background turn reads
  `Working` until the next prompt. Mitigated by three independent recognisers (D3) and named in
  G8 (D8).
- (c) A persistent background process the user is meant to use (a dev server started with
  `run_in_background`) holds `Working` until the next prompt. Accepted as in the sibling (D6).
- (d) A new background tool whose result uses a new field reads as today (closed list, D2).

## Decisions

### D1: An `end_turn` with background work still pending is `AssistantBusy`
- **Status**: resolved (user, 2026-10-22: "same as agy")
- **Decision**: when the last meaningful record is an assistant `end_turn`, the classifier also
  walks the turn (D4) for background **starts** (D2) and **completions** (D3). If at least one
  start has no completion, `last` is `LastMessage::AssistantBusy` (→ `Busy` → card `Working`, at
  any age). Otherwise it is exactly today's `AssistantEndTurn` (→ `Waiting`). No other
  classification changes: `AssistantAsksUser`, `AssistantBusy`, `UserToolResult`, `UserPrompt`
  and `None` are computed as today. This narrows `left-sidebar-waiting-area` D16's "`end_turn`
  → Waiting" for Claude: an `end_turn` means Waiting only when nothing the turn started is still
  out.
- **Rationale**: measured at the `end_turn` (Research): 249 of 273 turns after which Claude woke
  on its own (a notification or a peer message) had work pending, and 489 of 511 turns after
  which the user typed had none.
  - **Rejected: leave `Waiting`** (user's alternative). True in the narrow sense, because Claude
    Code accepts input while background work runs, but the card then cannot tell "done, your
    move" from "waiting on its own work".
  - **Rejected: a new status** ("Waiting (background)"). It needs a new `LastMessage` variant,
    an `AgentState`, a card label and a precedence slot. The user chose the agy rule.
  - **Rejected: pending → `Waiting`-band variant**. Same cost as a new status.
  - **The cost weighed, and accepted.** The two errors are not symmetric. A false `Waiting`
    draws the user to a tab that does not need them yet. A false `Working` hides a tab that
    does, for example "the server's up, try it" after a dev server started in the background.
    D1 trades 22 false `Working` reads (of 511 turns the user answered, D6) for 249 correct
    ones (of 273 turns Claude resumed on its own) that today read `Waiting`. The user chose
    that trade at L0 ("same as agy") with an earlier, coarser split in front of them: 153 turn
    ends with pending work followed by a notification and 147 by the user typing first. That
    pass counted any start since the last prompt and completions up to the next record, and
    only the `user`-record notification form. The table in Research supersedes it.

### D2: A start is one of four structured `toolUseResult` shapes, a closed list
- **Status**: resolved (L1)
- **Decision**: a record with `type: "user"` whose top-level `toolUseResult` is an object starts
  the ids it names, when it has:
  1. `backgroundTaskId` (string): a background `Bash`, including a command moved to the
     background at its timeout;
  2. `agentId` (string) with `isAsync: true` and `status: "async_launched"`: a background
     `Agent`;
  3. `agentId` (string) with `background: true` and `status: "forked"`: a skill forked into the
     background;
  4. `resumedAgentId` and/or `revivedAgentId` (strings): `SendMessage` resuming a stopped agent.
     Each present id is a start.
  Nothing else starts anything. In particular a `Monitor` result (`taskId`, `persistent`), a
  foreground `Agent` (no `agentId`), `ScheduleWakeup` and cron tools start nothing.
- **Rationale**: measured (Research): 335 start records, namely 203 form 2, 92 form 1, 6 form 3
  and 34 form 4 (14 of which also name a `revivedAgentId`). Every id later received a
  notification or a stop, except one agent still running at measurement time. (The completion
  breakdown in Research covers forms 1–2 only, 295 ids. The 284 in D3's rejected option is an
  earlier text-match scan.) These
  are typed JSON fields of the tool result, not text, so command output that quotes them cannot
  start anything. The anchoring problem the sibling solved by position does not arise.
  - **Rejected: matching the result text** (`Command running in background with ID:`). It misses
    the timeout form (`… was moved to the background (ID: …)`, 10 of 92), and text can be quoted.
  - **Rejected: counting `Monitor`**. A persistent monitor lives for the whole session by
    design, so it would hold `Working` after every turn that started one. It is a watcher, not
    work Claude is waiting for. Measured 1 occurrence. A background `Bash` *can* also run for
    the whole session (a dev server, `tail -f`), but measured it almost never does: 90 of 92
    reported back. The tool cannot be told apart from its use, so the long-lived `Bash` is the
    accepted cost of D6, not a reason to drop the 90.
  - **Rejected: an open rule** ("any `toolUseResult` id field"). An unmeasured tool would then
    hold `Working`. A closed list degrades to today's `Waiting` instead.

### D3: A completion is any of four forms, and any status counts
- **Status**: resolved (L1); "any status" is `assumed`
- **Decision**: these complete the id they name:
  1. `type: "queue-operation"`, `operation: "enqueue"`, `content` beginning
     `<task-notification>`;
  2. `type: "user"` with `origin.kind == "task-notification"`, and message text (below)
     beginning `<task-notification>`;
  3. `type: "attachment"` with `attachment.type == "queued_command"` and
     `attachment.commandMode == "task-notification"`, `attachment.prompt` beginning
     `<task-notification>`;
  4. `type: "user"` whose `toolUseResult` has `task_id` (string): `TaskStop`.
  A record's **message text** is `message.content` when it is a string, else the `text` of the
  first block of type `text` in the `message.content` array, else nothing. Leading whitespace
  is ignored when testing a prefix.
  For 1–3 the id is the text of the first `<task-id>…</task-id>` element. Ids are compared as
  exact strings. **A completion clears only starts older than it** (D4's newest-first walk
  gives this by construction). This matters because a resume (D2 form 4) reuses the agent's
  earlier `agentId`: the first run's completion, written before the resume, must not clear the
  resumed run. Two starts of one id with one completion newer than both is one finished id. A completed, failed or killed task are all completions. A `peer` message
  ("Another Claude session sent a message") completes nothing.
- **Rationale**: the enqueue form alone covers 292 of 295 starts. The `user` and `queued_command`
  forms cover the delivery paths (idle and mid-turn) independently of it, and `TaskStop` is the
  only path that writes no notification. Three independent notification recognisers mean a
  rename must break all three before a finished task reads pending (D8).
  - **Rejected: only the `user` record** (what an earlier scan used): 141 of 284, since a
    notification that lands mid-turn never becomes one.
  - **Rejected: `peer` messages as completions**. A subagent messaging back may still be working,
    and the content names no task id.

### D4: Walk the back-scan window newest first, to the last human prompt
- **Status**: assumed
- **Decision**: only when the tail's last meaningful record is an `end_turn`, read the final
  `PROMPT_SCAN_CAP` (1 MiB) and walk its lines from the end. This includes records written
  *after* the `end_turn`, so a completion that lands clears the hold as soon as it is written.
  The walk reads `type` first and considers only `user`, `assistant`, `queue-operation` and
  `attachment` records. Every other type, including `api-request-blob`, is skipped. It collects
  completions and returns pending at the first start not yet completed. It stops at:
  - **the last human prompt**: a `type: "user"` record with no `toolUseResult`, no `isMeta:
    true`, `origin.kind` not `task-notification` or `peer`, and message text (D3) not beginning
    `<task-notification>`, and whose `message.content` has no `tool_result` block (measured: all
    5,936 such records carry `toolUseResult`, so this is a guard, not a measured case). A record
    with no message text (an image-only paste) is a prompt.
    Slash commands, `!` commands, accepted suggestions and **continuation summaries** all count
    as a boundary, so work still pending across a compaction is forgotten and the card reads
    today's `Waiting`, the safe direction. Starts before the boundary do not count (L0).
  - **an unreadable line, or the window's start**. A pending start found before that still reads
    Busy (a completion is always newer than its start). With nothing pending found, the result
    is today's `AssistantEndTurn`. The window ends at the same `stat`ed `len` as the tail (D7),
    and the walk runs only when the tail's newest non-empty line parsed (otherwise
    `last_meaningful` is already `None`: it sets `unreadable_after` on a line that does not parse
    and returns `None` for any older record, `crates/ai-monitor/src/activity.rs:202-235`, and a
    requirement pins this). So the walk never meets a half-written newest line.
    The unreadable line it can meet is the window's first, cut mid-record, which is its oldest.
  - **The trigger stays role-keyed.** Whether the walk runs is decided by today's
    `last_meaningful`, which keys on `message.role`, so a trailing `api-request-blob` with role
    `assistant` would suppress it (the result is `AssistantBusy` from that blob anyway, which
    is no worse). No log was measured ending on a blob (0/95), so this is accepted (Known Gaps).
- **Rationale**: mirrors the sibling's D4. Measured, every pending start sat within 535 KiB of
  its `end_turn`, so 1 MiB loses none. A prompt beyond 1 MiB (46 of 613) only means older
  starts are not seen, which falls back to today. The boundary errs toward "this is a prompt",
  which forgets pending work, the direction that keeps today's behaviour. Keying on `type` is
  required: an `api-request-blob` carries `message.role: "user"` and would otherwise end the walk.
  - **Rejected: carrying pending ids across prompts** (user's L0 decision).
  - **Rejected: `origin.kind == "human"` as the only boundary**. Records older than the `origin`
    field (263) have none, so their prompts would never end the walk.

### D5: Reuse `LastMessage::AssistantBusy`; nothing downstream changes
- **Status**: assumed
- **Decision**: no new variant. `state_from`, `AgentState`, `resolve_status`, the waiting band and
  `prompt.rs` are unchanged. The change is inside `ai_monitor::activity`.
- **Rationale**: the same as the sibling's D5. `AssistantBusy` → `Busy` at any age is the required
  behaviour.

### D6: No time limit, and the residual risk is named
- **Status**: assumed
- **Decision**: a pending start holds `AssistantBusy` until its completion is written, a newer
  meaningful record replaces the `end_turn`, or a human prompt moves the boundary. No age cap.
- **Rationale**: real waits run from seconds to many minutes. **Residual risk, accepted:**
  background work that never reports holds `Working` until the user types: a process killed
  outside `TaskStop`, a Claude Code crash, or a dev server started with `run_in_background` for
  the user to try. Measured: 0 of 85 end-of-file turns pending (one live session excluded), and
  22 of 511 turns the user answered had work still running.
  - **Rejected: cap at `STALE_LIMIT` (100 s)**. That needs a new variant (D5) and cuts real waits.

### D7: The back-scan lives in `tail_state`, behind one byte source, and runs only on an `end_turn`
- **Status**: assumed
- **Decision**: `tail_state` is split, as in `antigravity/tail.rs`, into a function over a byte
  source (`read_last(cap)`) that a test can count. It reads `TAIL_CAP` always, and
  `PROMPT_SCAN_CAP` more only when the tail's last meaningful record is an `end_turn`. The
  back-scan is bounded by the `len` the caller `stat`ed. The real byte count is observed in
  `ai-monitor`'s own unit tests through that byte source. The app is unchanged except its
  `probe::LOG_READ_BYTES` arithmetic, which is an upper bound by construction (tab-metadata
  R6.1's stated honest limit) and becomes `min(len, TAIL_CAP) + 2 × min(len, PROMPT_SCAN_CAP)`.
  The app cannot tell whether the back-scan ran (`TailState` is unchanged), so it accounts the
  worst case.
- **Rationale**: the app memoises on `(mtime, len)` (R6.7). The extra read happens **once per
  `(mtime, len)` change while the tail ends in an `end_turn`**, beside the 1 MiB
  `recent_prompts` read the app already makes on every such change. While work is pending
  after an `end_turn`, writes are rare: the next is usually the completion's
  `queue-operation`, after which Claude resumes and the tail stops ending in `end_turn`. So
  the doubling is bounded by the turn-end writes, and no scan result is cached.
  - **Rejected: sharing one 1 MiB buffer with `recent_prompts`**. It changes the public
    `ai-monitor` API and the app's call shape to save one read per turn end.
  - **Rejected: always back-scanning**. Every tool call would cost 1 MiB.

### D8: Register the fields in `docs/architecture.md` G8
- **Status**: resolved (L1: G8 is where external formats are registered)
- **Decision**: G8 gains a Claude bullet naming the start fields (D2), the completion forms (D3)
  and the human-prompt test (D4), with each rename's degradation:
  - a renamed start field → nothing pending (today's `Waiting`);
  - all three notification forms renamed → every background turn reads `Working` until the next
    prompt;
  - a renamed `origin.kind` value → a notification may read as a human prompt, which ends the
    walk early (today's `Waiting`).
  The oracle is the log: grep the newest `queue-operation` for `<task-notification>`.
  `left-sidebar-waiting-area`'s D16 gets an "Amended by `specs/claude-background-working` D1"
  note.
- **Rationale**: the same kind of fact as G5–G9.

### D9: Tests and the mutation gate
- **Status**: resolved (L1 + sibling)
- **Decision**: acceptance tests through `ai_monitor::activity::tail_state` in new
  `crates/ai-monitor/tests/tt*_claude_*.rs` files. A fixture is cut from a real log (paths
  scrubbed), unit tests in `activity.rs` cover the byte budget, and an `#[ignore]`d corpus test
  re-measures. `cargo mutants -f crates/ai-monitor/src/activity.rs -j 4` ends with 0 survivors
  that are not provably equivalent, and the count is recorded.
- **Rationale**: CLAUDE.md judges coverage by mutation.

## Constraints
- **Never `Waiting` from the new path.** It yields only `AssistantBusy` or today's result.
- **Byte budget**: the back-scan runs only on an `end_turn` tail, at most `PROMPT_SCAN_CAP`, and is
  observed by a counting byte source in tests.
- `ai-monitor` stays read-only over the log; dependency direction unchanged; no `unwrap()`
  outside tests; clippy `-D warnings` clean.
- `recent_prompts`, `state_from` and the `TailState` shape are unchanged.

## Known Gaps
- L2 review: 3 rounds. Round 1 raised 8 issues (7 fixed; the half-written-line one did not
  apply and is now explained in D4), round 2 raised 5 (all fixed), and round 3 raised 1 (a
  count read at two times, now labelled). The round-3 fix was not re-reviewed: the retry
  budget was spent.
- One machine, the Claude Code versions present in the corpus (2026-10-22). The field names are
  undocumented internals (D8).
- A background tool introduced later with a new result shape reads as today's `Waiting` until
  added to D2's list.
- `api-request-blob` records are still classified by `last_meaningful` as messages (Research),
  so a log ending on one would decide the trigger (D4). Harmless as measured (0/95), and out of
  scope here.
- Pending work does not survive a compaction: the continuation summary is a boundary (D4).
- A long-lived background `Bash` the user is meant to use (dev server, `tail -f`) reads
  `Working` until the user types (D6). Measured 2 of 92 `Bash` starts never reported.

## Requirements

Boundary for every code-behaviour sub-requirement below: the public function
`ai_monitor::activity::tail_state(path)`, read as its `TailState::last`, over a log written to a
temp file. In every GWT, "a turn" lists the records after a human prompt `Human("…")` (a
`type: "user"` record with `origin.kind: "human"` and string content). Helpers:
- `Bash(id)`: a `user` record whose `toolUseResult` is `{"stdout": "", "backgroundTaskId": id}`.
- `Agent(id)`: `toolUseResult` `{"isAsync": true, "status": "async_launched", "agentId": id}`.
- `Fork(id)`: `toolUseResult` `{"background": true, "status": "forked", "agentId": id}`.
- `Resume(id)`: `toolUseResult` `{"success": true, "resumedAgentId": id}`.
- `Enq(id)`: `{"type": "queue-operation", "operation": "enqueue", "content":
  "<task-notification>\n<task-id>id</task-id>\n<status>completed</status>…"}`.
- `Notif(id)`: a `user` record with `origin: {"kind": "task-notification"}` and that content.
- `Queued(id)`: an `attachment` record, `{"type": "queued_command", "commandMode":
  "task-notification", "prompt": "<task-notification>\n<task-id>id</task-id>…"}`.
- `Stop(id)`: a `user` record whose `toolUseResult` is `{"task_id": id, "message":
  "Successfully stopped task: id"}`.
- `Tool(name)`/`Result`: an assistant `tool_use` record (`stop_reason: "tool_use"`) and its
  plain `user` tool result (a `toolUseResult` with only `stdout`).
- `End`: an assistant record with `stop_reason: "end_turn"`.

Ids are realistic: `b0zk564sc`-style for `Bash`, 17-hex `aee2b85e39714d85f`-style for agents.

### R0: A Claude card reads Working while Claude waits on its own background work — D1, D8, D9

#### R0.1: A live session with a background task reads Working (manual)
- **Given**: a running Claude Code tab where Claude started a `run_in_background` command that
  takes over a minute, then ended its turn
- **When**: more than 10 s pass
- **Then**: the card reads `Working`. When the notification lands and Claude finishes, the card
  follows Claude as today (`Waiting` at its final `end_turn`)

#### R0.2: The design is registered (manual)
- **Given**: the change is merged
- **When**: a reader opens `docs/architecture.md` G8 and `specs/left-sidebar-waiting-area/spec.md` D16
- **Then**: G8 names the start fields, completion forms and prompt test, with each rename's
  degradation (D8). D16 carries an "Amended by `specs/claude-background-working` D1" note. This
  spec's `## Verification` records the mutation survivor count of `activity.rs`

### R1: Pending work at an `end_turn` reads AssistantBusy, finished work reads as today — D1, D3, D5, D6

#### R1.1: Each start form with no completion is pending
- **Given**: four turns `[Tool, X, End]` where X is `Bash(b1)`, `Agent(a1)`, `Fork(a1)` and `Resume(a1)`
- **When**: `tail_state` reads each
- **Then**: each `last == Some(AssistantBusy)`

#### R1.2: Each completion form clears it
- **Given**: four turns `[Tool, Bash(b1), C, Tool, Result, End]` where C is `Enq(b1)`,
  `Notif(b1)`, `Queued(b1)` and `Stop(b1)` (the completion landed mid-turn and Claude carried on)
- **When**: `tail_state` reads each
- **Then**: each `last == Some(AssistantEndTurn)` (today's result)

#### R1.3: A completion written after the `end_turn` clears it
- **Given**: `[Tool, Bash(b1), End, Enq(b1)]`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantEndTurn)`

#### R1.4: Several starts: any one unreported keeps it pending
- **Given**: a turn starting `Bash(b1)`, `Bash(b2)`, `Agent(a1)`, `Agent(a2)`, then `End`. Case A
  completes three of them (each left out in turn); case B completes all four
- **When**: `tail_state` reads each
- **Then**: every A case returns `Some(AssistantBusy)`; B returns `Some(AssistantEndTurn)`

#### R1.5: A completion for a different id does not clear a start
- **Given**: `[Tool, Bash(b1), End, Enq(b2)]`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R1.6: A completion clears only starts older than it (resume)
- **Given**: `[Agent(a1), Notif(a1), Tool, Resume(a1), End]` (the first run finished, the resumed
  run has not), and the same plus `Enq(a1)` after `End`
- **When**: `tail_state` reads each
- **Then**: the first returns `Some(AssistantBusy)`, the second `Some(AssistantEndTurn)`

#### R1.7: Never Waiting from a pending turn, at any age
- **Given**: each R1 transcript expected `Some(AssistantBusy)`, with `mtime` set 1 hour in the past
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantBusy)`

#### R1.8: A peer message completes nothing
- **Given**: `[Tool, Agent(a1), End]` followed by a `user` record with `isMeta: true`,
  `origin.kind: "peer"` and content "Another Claude session sent a message: …", then a new `End`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R1.9: Only an `end_turn` triggers the walk
- **Given**: `[Tool, Bash(b1), Tool]` (last record a `tool_use`), and `[Tool, Bash(b1), Result]`
- **When**: `tail_state` reads each
- **Then**: `Some(AssistantBusy)` and `Some(UserToolResult)` respectively, today's results

### R2: Only the listed start shapes start anything — D2

#### R2.1: Unlisted background-looking results start nothing
- **Given**: turns `[Tool, X, End]` where X's `toolUseResult` is a `Monitor` result
  (`{"taskId": "m1", "persistent": true}`), a foreground `Agent` result (no `agentId`), an
  `{"isAsync": true, "status": "completed", "agentId": "aee2b85e39714d85f"}` result, and a `Bash` result whose *stdout text* reads
  `Command running in background with ID: b1` with no `backgroundTaskId` field
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantEndTurn)`

#### R2.2: A start field outside a `user` record starts nothing
- **Given**: `[Tool, A, End]` where A is an assistant record whose top level carries
  `"toolUseResult": {"backgroundTaskId": "b1"}`, and the same with an `api-request-blob`
  record (role `user`) carrying it
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantEndTurn)`

#### R2.3: A non-string id starts nothing
- **Given**: `[Tool, X, End]` where X has `"backgroundTaskId": 7`, and `"agentId": null` with
  the async fields
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantEndTurn)`

### R3: Only the listed completion shapes complete anything — D3

#### R3.1: A notification in the wrong record type completes nothing
- **Given**: `[Tool, Bash(b1), End]` followed by a `queue-operation` with `operation:
  "dequeue"` or `"remove"` carrying `<task-id>b1</task-id>`; an `attachment` whose
  `commandMode` is `"prompt"` carrying it; and, as a separate turn `[Tool, Bash(b1), Tool, X,
  End]`, a `user` tool result X whose stdout quotes `<task-notification>\n<task-id>b1</task-id>`
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantBusy)`

#### R3.2: The notification text must begin the content
- **Given**: `[Tool, Bash(b1), End]` followed by an enqueue whose content is
  `"see <task-notification>\n<task-id>b1</task-id>"`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R3.3: Any status completes
- **Given**: `[Tool, Bash(b1), End, Enq(b1)]` where the enqueue's `<status>` is `failed`, and again
  with `killed`
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantEndTurn)`

### R4: The walk's boundaries — D4, D7

#### R4.1: Starts before the last human prompt do not count
- **Given**: `[Human, Tool, Bash(b1), End, Human, Tool, Result, End]` with no completion for b1
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantEndTurn)`

#### R4.2: Each kind of human prompt is a boundary
- **Given**: R4.1 with the second `Human` replaced by each of: a slash-command wrapper
  (`<command-name>/model</command-name>`, no `origin`), a continuation summary ("This session is
  being continued…", no `origin`), an `origin.kind: "human"` record whose content is an array
  with a `text` block, and an image-only array prompt (no `text` block)
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantEndTurn)`

#### R4.3: Notifications, peers, meta records and tool results are not boundaries
- **Given**: `[Human, Tool, Bash(b1), X, Tool, Result, End]` where X is each of: `Notif(b2)`, a
  peer message, an `isMeta: true` user record, a `user` record holding a `tool_result` block
  without `toolUseResult`, and an `api-request-blob` with role `user`
- **When**: `tail_state` reads each
- **Then**: each returns `Some(AssistantBusy)`

#### R4.4: A pending start found before the window's edge reads Busy
- **Given**: a log whose last human prompt lies more than `PROMPT_SCAN_CAP` bytes before the end,
  with `Bash(b1)` inside the final `PROMPT_SCAN_CAP` bytes, then `End`
- **When**: `tail_state` reads it
- **Then**: `Some(AssistantBusy)`. Without the `Bash(b1)` it is `Some(AssistantEndTurn)` (today)

#### R4.5: A start beyond the window is not seen
- **Given**: `[Human, Tool, Bash(b1)]`, then foreground records totalling more than
  `PROMPT_SCAN_CAP` bytes, then `End`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantEndTurn)` (today's result: the start is out of reach)

#### R4.6: A torn newest line gives no signal
- **Given**: `[Tool, Bash(b1), End]` followed by half of a JSON record with no trailing newline
- **When**: `tail_state` reads it
- **Then**: `last == None` (today's `last_meaningful` rule, which keeps the walk from running)

#### R4.7: The byte budget
- **Given**: a log larger than `TAIL_CAP` ending in `End`, and the same log ending in a
  `tool_use`, a tool result and a human prompt
- **When**: the classification runs through a counting byte source (private to `activity.rs`,
  so these are unit tests in its `mod tests`, the one exception to the `tail_state` boundary)
- **Then**: the `End` case reads at most `TAIL_CAP + PROMPT_SCAN_CAP` bytes, each other case at
  most `TAIL_CAP`

### R5: Real shapes — D9

#### R5.1: A fixture cut from a real log reads Busy, then clears
- **Given**: the new fixture `crates/ai-monitor/tests/fixtures/tt5_claude_background_pending.jsonl`,
  cut from a real session (paths scrubbed): a human prompt, a background `Bash` launch, an
  `end_turn`, then the `queue-operation` enqueue and the `task-notification` user record that
  followed
- **When**: `tail_state` reads the prefix ending at the `end_turn`, and the prefix ending at the
  enqueue
- **Then**: the first returns `Some(AssistantBusy)`, the second `Some(AssistantEndTurn)`

#### R5.2: The existing Claude tests keep their results
- **Given**: every existing test in `crates/ai-monitor/src/activity.rs`'s `mod tests` and the
  app's `tabmeta` activity tests
- **When**: the suite runs
- **Then**: all pass unchanged. (No test asserts the app's `LOG_READ_BYTES` today; T4 adds one.)

#### R5.3: The app accounts the conditional back-scan
- **Given**: a changed log of more than `PROMPT_SCAN_CAP` bytes and the app's `tabmeta` cost probe
  reset
- **When**: one activity pass runs `tail_role` on it
- **Then**: `probe::LOG_READ_BYTES` rises by exactly `TAIL_CAP + 2 × PROMPT_SCAN_CAP` (D7's
  upper bound)

#### R5.4: The rule holds on the real corpus (manual)
- **Given**: the `#[ignore]`d test `crates/ai-monitor/tests/corpus_claude_pending.rs` and the
  local `~/.claude/projects` corpus (the test passes without asserting when it is absent)
- **When**: `cargo test -p ai-monitor --test corpus_claude_pending -- --ignored --nocapture` runs
- **Then**: for every `end_turn` that is the last record of its message, `tail_state` on the log
  prefix ending there reads `AssistantBusy` exactly when the test's reference walk finds a
  pending start. The reference walk is written independently in the test, applying D2–D4 over the
  same window: the prefix's final `PROMPT_SCAN_CAP` bytes, newest first, stopping at the human
  prompt, an unreadable line or the window's start. The printed outcome table is recorded with its UTC time in `## Verification`

## Tasks

The acceptance tests for R1–R4 are written before the implementation in T1 and must be shown
red against `main`'s `activity.rs` for every case that expects `Some(AssistantBusy)` where
today reads `Some(AssistantEndTurn)` (R1.1, R1.4 A, R1.5, R1.6 first case, R1.7, R1.8, R3.1,
R3.2, R4.3, R4.4 first case). Every other case is a regression guard and is green before and after.

### T1: pending-background classification in `activity.rs`, test-first [vertical]
- **Fulfills**: R1, R2, R3, R4
- **Depends on**: (none)
- **Files**: new `crates/ai-monitor/tests/tt1_claude_background.rs` (the R1–R4 GWTs, with the
  Requirements' helpers in the file; logs written under `std::env::temp_dir()` per test, as
  `crates/ai-monitor/tests/antigravity.rs` does); `crates/ai-monitor/src/activity.rs`
- **Work**:
  - Write the tests first and record which go red on `main` (above).
  - Split `tail_state` into `tail_state(path)` and a private `state_from_reads(len, mtime,
    read_last)` over a byte source, as `crates/ai-monitor/src/antigravity/tail.rs` does. The
    back-scan is bounded by the `stat`ed `len`. `TailState`, `recent_prompts` and the public
    signature are unchanged.
  - When `last_meaningful` returns `Some(AssistantEndTurn)`, read `PROMPT_SCAN_CAP` and walk it
    (D4): small named functions for the start shapes (D2), the completion forms (D3), message
    text, and the human-prompt test, each with a doc comment citing this spec and its measured
    counts. No regex crate, no new dependency.
  - Unit tests in `activity.rs`'s `mod tests`: R4.7 (byte budget, counting source), and any
    case needed to kill mutants.
- **Done when**: the R1–R4 tests and the existing `activity.rs` tests pass; clippy clean.

### T2: a real-log fixture [vertical]
- **Fulfills**: R5.1
- **Depends on**: T1
- **Files**: new `crates/ai-monitor/tests/fixtures/tt5_claude_background_pending.jsonl`; new
  `crates/ai-monitor/tests/tt5_claude_real_shapes.rs`
- **Work**: cut a short run from a real `~/.claude/projects` log: a human prompt, a background
  `Bash` launch (`toolUseResult.backgroundTaskId`), the `end_turn`, then the `queue-operation`
  enqueue and the `task-notification` user record. Scrub home paths, the session's `cwd` and
  any `serverClassifierContext`, and keep only the fields the classifier reads plus enough
  envelope to look real. If no such run is found locally, build one from the shapes quoted
  in Research and say so in the fixture's test doc comment.
- **Done when**: R5.1's two prefix assertions pass in `tt5_claude_real_shapes.rs`, and no home
  path or classifier context remains in the fixture (`grep` for the home directory returns
  nothing).

### T3: register the design in docs [vertical]
- **Fulfills**: R0
- **Depends on**: (none). Parallel with T1 and T4 (no shared files).
- **Files**: `docs/architecture.md` (G8: a new Claude bullet directly after the existing
  "Antigravity's transcript vocabulary" bullet. If PR #54 has merged first, place it after
  that PR's "Antigravity's background-work pairing" bullet instead), `specs/left-sidebar-waiting-area/spec.md`
  (an "Amended by" note under D16)
- **Work**: D8's fields, forms, prompt test, each rename's degradation, the oracle, and D6's
  residual risk in one sentence.
- **Done when**: G8 has the Claude bullet and D16 has the note (R0.2).

### T4: the app's byte accounting [vertical]
- **Fulfills**: R5.3
- **Depends on**: (none). Parallel with T1 and T3.
- **Files**: `crates/app/src/tabmeta/activity.rs` (the `#[cfg(test)]` `LOG_READ_BYTES` arithmetic
  in `tail_role`). No test asserts that counter today: add one in the `tabmeta` tests, using
  the existing `probe` reset/read pattern
- **Work**: account `min(len, TAIL_CAP) + 2 × min(len, PROMPT_SCAN_CAP)` as an upper bound (D7),
  with a comment saying the app cannot see whether the back-scan ran.
- **Done when**: the new R5.3 test passes, and fails with the old arithmetic.

### T5: mutation gate, corpus re-measurement and verification record [vertical]
- **Fulfills**: R0, R5.2, R5.4
- **Depends on**: T1, T2, T3, T4
- **Files**: this spec (a new `## Verification` section); new `#[ignore]`d
  `crates/ai-monitor/tests/corpus_claude_pending.rs`; `crates/ai-monitor/src/activity.rs` (tests
  or a rewritten condition only)
- **Work**:
  - `cargo mutants -f crates/ai-monitor/src/activity.rs -j 4`. Treat every survivor as a
    missing test unless provably equivalent, and prefer rewriting the condition over an
    `exclude_re`. Record the survivor count, the full test count and clippy's result.
  - The corpus test reads every `~/.claude/projects/*/*.jsonl` (skipping when absent). For each
    `end_turn` that is the last record of its message, it writes the log prefix ending there to
    a temp file and calls `tail_state`. It tallies the result against the next `user`/`assistant`
    record (notification, peer, human, end of file, other) and prints the table. It asserts
    that no prefix reads `AssistantBusy` unless the reference walk (in the test) finds a
    pending start, and that every prefix the reference walk finds pending reads
    `AssistantBusy`. Run with `cargo test -p ai-monitor --test corpus_claude_pending --
    --ignored --nocapture`, and record the table with its UTC time. The reference walk uses the
    same final-`PROMPT_SCAN_CAP` window and stop rules as D4 (R5.4). The equivalence assertions
    are the gate. The table is **recorded, not gated**, because the live corpus moves. Compare it
    with Research's, and if the pending share of notification- and peer-next turns falls below
    80% or the pending share of human-next turns rises above 10%, record why before merging.
  - R0.1, the live check: `/run-term`, a Claude Code tab, ask it to run `sleep 90` in the
    background and end its turn; confirm the card reads `Working` until the notification lands.
    Record the result in `## Verification`. If it cannot be run in the session, record it as
    not run and leave it as the PR's open checkbox.
- **Done when**: `cargo test --workspace` is green with every pre-existing test unmodified
  (R5.2), and the survivor count, the test and clippy results, the corpus table and R0.1's
  result (or "not run") are recorded.

## Verification

Recorded 2026-10-23 (UTC), on branch `fix/claude-background-working`.

- **T1, red on `main`:** 10 of 21 `tt1_claude_background.rs` tests failed against the unchanged
  `activity.rs`: R1.1, R1.4, R1.5, R1.6, R1.7, R1.8, R3.1, R3.2, R4.3 and R4.4. That is exactly
  the red-on-main list in Tasks. The other 11 passed before and after. All 21 pass now.
- **T4:** `a_changed_log_accounts_the_conditional_back_scan` passes, and fails with the old
  arithmetic (`left: 1056768, right: 2105344`).
- **R5.2:** `cargo test --workspace` gave 2093 passed, 0 failed, 19 ignored. No pre-existing
  test was modified. `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all -- --check` are both clean.
- **Mutation (`activity.rs`):** **0 survivors** (`cargo mutants -f crates/ai-monitor/src/activity.rs -j 4
  --test-package ai-monitor`: 101 mutants, 96 caught, 5 unviable). The first pass left 3:
  `message_text`'s blank-string filter, which is pre-existing code with no string-form prompt
  test; `started_ids`'s forked `&&`; and `message_str`'s `text`-block match. Each now has a
  killing test: `prompts_read_the_string_content_form_and_skip_a_blank_one`, and R2.1's two
  half-forked cases and R1.2's array-content notification in `tt1_claude_background.rs`.
  Restricting the run to `ai-monitor`'s tests can only add survivors, never hide one, so
  zero holds for the full workspace suite too.
- **R5.4, corpus re-measurement**, 2026-10-23 16:54 UTC: 80 logs and 876 `end_turn` turns.
  Two whose `end_turn` record is wider than `TAIL_CAP` were not compared. **0 mismatches**
  between `tail_state` and the reference walk.

  | next record | pending | nothing pending |
  |---|---|---|
  | `task-notification` | 160 | 30 |
  | `peer` | 94 | 4 |
  | a human prompt | 22 | 479 |
  | end of file | 0 | 67 |
  | other | 5 | 13 |

  254 of 288 notification/peer-next turns were pending (88%, above the 80% note threshold), and
  22 of 501 human-next turns (4.4%, below 10%). This counts every last-of-message `end_turn`,
  including turns with no human prompt before them. Research's 613 counted only those that had
  one, so the totals differ, but the shape matches.
- **R0.1 (live):** not run in this session. It needs a running app, a Claude Code tab and a
  person watching the card. It stays as the PR's open checkbox.

## External Dependencies

### Pre-work
(none)

### Post-work
- R0.1, if T5 could not run it live.
- Commit, push and PR only when the user asks.

## Spec Review
- **Verdict**: NEEDS_FIX (final review, 3 of 3). Every issue was fixed; the last round's fixes were not re-reviewed.
- **Reviews**: 3
- **Fixed**:
  - [Testability] R3.1: the quoting tool result moved inside the turn (after `End` it became
    the last record and the walk never ran). *Requirement edit after L3 approval.*
  - [Testability] R2.1: the `isAsync` case names an `agentId`, so it exercises the status
    check. *Requirement edit after L3 approval.*
  - [Consistency] Red-on-main list gained R3.1 and R3.2.
  - [Consistency] Test file names follow D9 (`tt1_…`, `tt5_…`).
  - [Coverage] R0.1's live check moved into T5.
  - [Readiness] T2–T4 have Done-when lines.
  - [Readiness] No test asserted `LOG_READ_BYTES`: new R5.3, written by T4. *New requirement.*
  - [Coverage] The corpus re-measurement had no requirement: new R5.4, fulfilled by T5. *New
    requirement.*
  - (round 3, unreviewed) R5.4's reference walk uses the same window and stop rules. The corpus
    table is recorded, not gated, with an 80%/10% note threshold. The sibling's location is
    stated (PR #54) and T3's G8 anchor is a bullet that exists. R5.2 moved to T5 with a
    workspace-test Done-when. R4.7 is no longer marked manual.
- **Open**: (none known; the round-3 fixes are unreviewed)
